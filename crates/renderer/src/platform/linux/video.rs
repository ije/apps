use super::gpu::{Gpu, extent};
use crate::{ColorMatrix, Planes, Rect, Surface};
use anyhow::{Context as _, Result, bail, ensure};
use ash::vk;
use std::os::fd::BorrowedFd;
use std::sync::{Arc, atomic::AtomicBool};
use wgpu::{BindGroupEntry, BindGroupLayoutEntry, BindingResource, BindingType, BufferUsages};
use wgpu::{RenderPipeline, ShaderStages, Texture, TextureFormat, TextureUsages, TextureView};

/// The GPU can read dma-bufs.
pub(crate) static SURFACES: AtomicBool = AtomicBool::new(false);

const FORMATS: [(TextureFormat, vk::Format); 2] = [
    (TextureFormat::R8Unorm, vk::Format::R8_UNORM),
    (TextureFormat::Rg8Unorm, vk::Format::R8G8_UNORM),
];

pub(super) fn pipeline(gpu: &Gpu) -> RenderPipeline {
    let device = &gpu.device;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(include_str!("video.wgsl").into()),
    });
    let plane = BindingType::Texture {
        sample_type: wgpu::TextureSampleType::Float { filterable: true },
        view_dimension: wgpu::TextureViewDimension::D2,
        multisampled: false,
    };
    let uniform = BindingType::Buffer {
        ty: wgpu::BufferBindingType::Uniform,
        has_dynamic_offset: false,
        min_binding_size: None,
    };
    let filtering = BindingType::Sampler(wgpu::SamplerBindingType::Filtering);
    let mut binding = 0;
    let entries = [uniform, filtering, plane, plane].map(|ty| {
        binding += 1;
        BindGroupLayoutEntry {
            binding: binding - 1,
            visibility: ShaderStages::FRAGMENT,
            ty,
            count: None,
        }
    });
    let group = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &entries,
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&group)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("video"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("video_vertex"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("video_fragment"),
            targets: &[Some(gpu.color.into())],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

// Imports with a modifier that the driver doesn't list are undefined behavior.
fn readable(hal: &wgpu::hal::vulkan::Device, format: vk::Format, modifier: u64) -> bool {
    let instance = hal.shared_instance().raw_instance();
    let physical = hal.raw_physical_device();
    let query = |list: &mut vk::DrmFormatModifierPropertiesListEXT| {
        let mut properties = vk::FormatProperties2::default().push_next(list);
        // SAFETY: the instance and the physical device are alive.
        unsafe {
            instance.get_physical_device_format_properties2(physical, format, &mut properties)
        };
    };
    let mut list = vk::DrmFormatModifierPropertiesListEXT::default();
    query(&mut list);
    let mut found = vec![Default::default(); list.drm_format_modifier_count as usize];
    let mut list = vk::DrmFormatModifierPropertiesListEXT::default()
        .drm_format_modifier_properties(&mut found);
    query(&mut list);
    let wanted =
        vk::FormatFeatureFlags::SAMPLED_IMAGE | vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR;
    found
        .iter()
        .any(|kind: &vk::DrmFormatModifierPropertiesEXT| {
            kind.drm_format_modifier == modifier
                && kind.drm_format_modifier_plane_count == 1
                && kind.drm_format_modifier_tiling_features.contains(wanted)
        })
}

fn import(gpu: &Gpu, planes: &Planes) -> Result<[Texture; 2]> {
    // SAFETY: the device is not destroyed here.
    let hal = unsafe { gpu.device.as_hal::<wgpu::hal::api::Vulkan>() }.context("no Vulkan")?;
    let plane = |at: usize| {
        let (format, raw) = FORMATS[at];
        let modifier = planes.modifiers[at];
        ensure!(readable(&hal, raw, modifier), "the modifier {modifier:x}");
        let [width, height] = planes.size.map(|side| side.div_ceil(1 << at));
        let size = extent(width as i32, height as i32);
        let described = wgpu::hal::TextureDescriptor {
            label: None,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUses::RESOURCE,
            memory_flags: wgpu::hal::MemoryFlags::empty(),
            view_formats: Vec::new(),
        };
        // SAFETY: the surface owns the descriptor, and exported the layout with it.
        unsafe {
            let fd = BorrowedFd::borrow_raw(planes.fds[at]).try_clone_to_owned()?;
            let (pitch, offset) = (planes.pitches[at].into(), planes.offsets[at].into());
            let made = hal.texture_from_dmabuf_fd(fd, &described, modifier, pitch, offset)?;
            let described = wgpu::TextureDescriptor {
                label: None,
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            };
            // A change from the undefined state could drop the contents.
            let state = wgpu::TextureUses::RESOURCE;
            Ok((gpu.device)
                .create_texture_from_hal::<wgpu::hal::api::Vulkan>(made, &described, state))
        }
    };
    Ok([plane(0)?, plane(1)?])
}

fn uniforms(planes: &Planes, place: Rect<i32>) -> [f32; 16] {
    let (red, blue) = match planes.matrix {
        ColorMatrix::Bt601 => (0.299, 0.114),
        ColorMatrix::Bt709 => (0.2126, 0.0722),
        ColorMatrix::Bt2020 => (0.2627, 0.0593),
    };
    let green = 1. - red - blue;
    let (base, luma, chroma) = match planes.full_range {
        true => (0., 1., 1.),
        false => (16. / 255., 255. / 219., 255. / 224.),
    };
    let (v_red, u_blue) = (2. * (1. - red) * chroma, 2. * (1. - blue) * chroma);
    let (u_green, v_green) = (-u_blue * blue / green, -v_red * red / green);
    let row = |u: f32, v: f32| [luma, u, v, -luma * base - (u + v) * 128. / 255.];
    let rows = [
        place.map(|value| value as f32).to_array(),
        row(0., v_red),
        row(u_green, v_green),
        row(u_blue, 0.),
    ];
    std::array::from_fn(|at| rows[at / 4][at % 4])
}

// Draws the surface at `place` of `target`, and holds it until the GPU has read it.
pub(super) fn draw(
    gpu: &Gpu,
    surface: &Arc<dyn Surface>,
    target: &TextureView,
    place: Rect<i32>,
) -> Result<()> {
    let planes = surface.planes();
    let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let textures = import(gpu, &planes);
    if let Some(error) = pollster::block_on(scope.pop()) {
        bail!("importing a surface: {error}");
    }
    let views = textures?.map(|texture| texture.create_view(&Default::default()));
    let pipeline = gpu.video.get_or_init(|| pipeline(gpu));
    let data = uniforms(&planes, place);
    let data: Vec<u8> = data.iter().flat_map(|value| value.to_ne_bytes()).collect();
    let buffer = gpu.buffer(data.len() as u64, BufferUsages::UNIFORM);
    gpu.queue.write_buffer(&buffer, 0, &data);
    let resources = [
        buffer.as_entire_binding(),
        BindingResource::Sampler(&gpu.sampler),
        BindingResource::TextureView(&views[0]),
        BindingResource::TextureView(&views[1]),
    ];
    let mut binding = 0;
    let entries = resources.map(|resource| {
        binding += 1;
        BindGroupEntry {
            binding: binding - 1,
            resource,
        }
    });
    let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &entries,
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    });
    let [x, y, width, height] = place.map(|value| value as f32).to_array();
    pass.set_viewport(x, y, width, height, 0., 1.);
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, &group, &[]);
    pass.draw(0..4, 0..1);
    drop(pass);
    gpu.queue.submit([encoder.finish()]);
    let surface = surface.clone();
    gpu.queue.on_submitted_work_done(move || drop(surface));
    Ok(())
}
