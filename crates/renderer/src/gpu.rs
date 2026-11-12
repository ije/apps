use crate::{Clip, Rect, point, size};
use std::{mem, slice};

// GPU layouts. `shaders.wgsl` declares the same layouts: change both together.

pub(crate) const BOX: u32 = 0;
pub(crate) const SHADOW: u32 = 1;
pub(crate) const LINE: u32 = 2;
pub(crate) const GLYPH: u32 = 3;
pub(crate) const IMAGE: u32 = 4;

// One box, shadow, line, glyph or image in device pixels. The shader reads it as 13 `vec4` vertex
// attributes, one step per instance. Radii hold the x radius of the four corners, then the y radius,
// in the order top left, top right, bottom right, bottom left. Colors have straight alpha.
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct Instance {
    pub rect: [f32; 4],
    pub clip: [f32; 4],
    // The rounded clip, used when `round_radii` is not zero.
    pub round: [f32; 4],
    pub round_radii: [f32; 8],
    pub radii: [f32; 8],
    pub color: [f32; 4],
    pub border_color: [f32; 4],
    // Box: border widths top, right, bottom, left. Glyph and image: the tile in texels. Shadow: the
    // caster box.
    pub aux: [f32; 4],
    // Shadow: the caster radii.
    pub aux_radii: [f32; 8],
    // Kind, flags, then two values that depend on the kind:
    // box: flags 0-7 border style, 8-11 sides to draw (0 draws all)
    // shadow: flag 1 inset, blur sigma
    // line: flag 1 wavy, thickness
    // glyph: -, atlas page
    // image: flag 1 grayscale, flag 2 premultiplied source, opacity, atlas page
    pub info: [f32; 4],
}

pub(crate) const ATTRIBUTES: usize = mem::size_of::<Instance>() / 16;

impl Instance {
    pub fn new(kind: u32, rect: Rect<f32>, clip: Clip<f32>) -> Self {
        Self {
            rect: rect.to_array(),
            clip: clip.bounds.to_array(),
            round: clip.rounded.to_array(),
            round_radii: clip.radii.to_array(),
            info: [kind as f32, 0., 0., 0.],
            ..Default::default()
        }
    }

    pub fn kind(&self) -> u32 {
        self.info[0] as u32
    }

    // Glyphs read a mask page and images a color page.
    pub fn page(&self) -> Option<(bool, u32)> {
        match self.kind() {
            GLYPH => Some((false, self.info[3] as u32)),
            IMAGE => Some((true, self.info[3] as u32)),
            _ => None,
        }
    }

    pub fn reach(&self) -> Rect<f32> {
        let area = match self.kind() {
            SHADOW if self.info[1] as u32 & 1 != 0 => self.aux,
            // The shader draws the blur up to 3 sigma outside the shadow box.
            SHADOW => {
                let [x, y, w, h] = self.rect;
                let spread = self.info[2] * 3.;
                [x - spread, y - spread, w + spread * 2., h + spread * 2.]
            }
            _ => self.rect,
        };
        rect(area).intersect(rect(self.clip))
    }
}

pub(crate) fn rect([x, y, width, height]: [f32; 4]) -> Rect<f32> {
    Rect::new(point(x, y), size(width, height))
}

#[repr(C)]
pub struct Globals {
    // Origin and size of the target in window device pixels.
    pub viewport: [f32; 4],
    // Glyph coverage moves toward the ink by this much. Targets that blend in sRGB need it.
    pub text_contrast: f32,
    pub pad: [f32; 3],
}

pub(crate) const COMPOSITE_MATRIX: u32 = 1;
pub(crate) const COMPOSITE_CLIP: u32 = 2;
pub(crate) const COMPOSITE_MASK: u32 = 4;
pub(crate) const COMPOSITE_SHADOW: u32 = 8;
pub(crate) const COMPOSITE_BACKDROP: u32 = 16;
pub(crate) const COMPOSITE_BLEND: u32 = 32;

#[derive(Clone, Copy)]
#[repr(C)]
pub struct Composite {
    pub transform: [f32; 16],
    pub color_matrix: [f32; 16],
    pub color_offset: [f32; 4],
    pub shadow_color: [f32; 4],
    pub bounds: [f32; 4],
    pub clip: [f32; 4],
    pub clip_radii: [f32; 8],
    pub mask_rect: [f32; 4],
    pub mask_tile: [f32; 4],
    pub content_mask: [f32; 4],
    pub rounded_mask: [f32; 4],
    pub rounded_radii: [f32; 8],
    pub shadow: [f32; 4],
    pub source_rect: [f32; 4],
    pub backdrop_rect: [f32; 4],
    pub opacity: f32,
    pub blur: f32,
    pub flags: u32,
    pub blend: u32,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct Blur {
    pub region: [f32; 4],
    pub source_rect: [f32; 4],
    pub direction: [f32; 2],
    pub sigma: f32,
    pub scale: f32,
}

/// Data that `bytes` can hand to the GPU.
///
/// # Safety
///
/// Implement only for `repr(C)` structs of 4-byte numbers, which have no padding.
pub unsafe trait Plain {}
// SAFETY: as above.
unsafe impl Plain for Instance {}
// SAFETY: as above.
unsafe impl Plain for Globals {}
// SAFETY: as above.
unsafe impl Plain for Composite {}
// SAFETY: as above.
unsafe impl Plain for Blur {}

pub fn bytes<T: Plain>(items: &[T]) -> &[u8] {
    let (start, length) = (items.as_ptr().cast::<u8>(), mem::size_of_val(items));
    // SAFETY: plain data can be read as bytes.
    unsafe { slice::from_raw_parts(start, length) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use naga::{
        front::wgsl,
        valid::{Capabilities, ValidationFlags, Validator},
    };

    // Instances, uniforms and the shader agree on every size and attribute.
    #[test]
    fn shader_layouts_match() {
        let module = wgsl::parse_str(include_str!("shaders.wgsl")).unwrap();
        let mut validator = Validator::new(ValidationFlags::all(), Capabilities::empty());
        validator.validate(&module).unwrap();
        let mut layouter = naga::proc::Layouter::default();
        layouter.update(module.to_ctx()).unwrap();
        for (name, size) in [
            ("Globals", mem::size_of::<Globals>()),
            ("Composite", mem::size_of::<Composite>()),
            ("Blur", mem::size_of::<Blur>()),
        ] {
            let (handle, _) = module
                .types
                .iter()
                .find(|(_, ty)| ty.name.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("missing struct {name}"));
            assert_eq!(layouter[handle].size as usize, size, "{name}");
        }
        let draw = module
            .entry_points
            .iter()
            .find(|entry| entry.name == "draw_vertex")
            .unwrap();
        let mut locations = Vec::new();
        for argument in &draw.function.arguments {
            let members = match &module.types[argument.ty].inner {
                naga::TypeInner::Struct { members, .. } => members.clone(),
                _ => continue,
            };
            for member in members {
                let Some(naga::Binding::Location { location, .. }) = member.binding else {
                    continue;
                };
                assert!(matches!(
                    module.types[member.ty].inner,
                    naga::TypeInner::Vector {
                        size: naga::VectorSize::Quad,
                        scalar: naga::Scalar::F32
                    }
                ));
                locations.push(location);
            }
        }
        locations.sort_unstable();
        assert!(locations.into_iter().eq(0..ATTRIBUTES as u32));
        assert_eq!(mem::size_of::<Instance>(), 13 * 16);
    }
}
