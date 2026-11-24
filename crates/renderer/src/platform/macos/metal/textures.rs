use super::*;

// Sides must be within 1..=16384.
pub(super) fn new_texture(
    device: &ProtocolObject<dyn MTLDevice>,
    [width, height]: [usize; 2],
    format: MTLPixelFormat,
    usage: MTLTextureUsage,
    storage: MTLStorageMode,
) -> Result<Texture> {
    let spec = MTLTextureDescriptor::new();
    // SAFETY: callers keep both sides within the limits of a 2D texture.
    unsafe {
        spec.setWidth(width);
        spec.setHeight(height);
    }
    spec.setPixelFormat(format);
    spec.setUsage(usage);
    spec.setStorageMode(storage);
    device
        .newTextureWithDescriptor(&spec)
        .with_context(|| format!("no {width}x{height} texture"))
}

pub(super) fn upload(
    texture: &ProtocolObject<dyn MTLTexture>,
    rect: Rect<i32>,
    bytes: &[u8],
    row: usize,
) {
    let at = |value: i32| value as usize;
    // SAFETY: callers pass bytes that cover the rectangle, which lies within the CPU-accessible texture.
    unsafe {
        texture.replaceRegion_mipmapLevel_withBytes_bytesPerRow(
            MTLRegion {
                origin: MTLOrigin {
                    x: at(rect.origin.x),
                    y: at(rect.origin.y),
                    z: 0,
                },
                size: MTLSize {
                    width: at(rect.size.width),
                    height: at(rect.size.height),
                    depth: 1,
                },
            },
            0,
            NonNull::from(bytes).cast(),
            row,
        );
    }
}

pub(super) fn page_texture(
    device: &ProtocolObject<dyn MTLDevice>,
    shared: bool,
    kind: PageKind,
    size: Size<i32>,
) -> Result<Texture> {
    new_texture(
        device,
        [size.width as usize, size.height as usize],
        match kind {
            PageKind::Mask => MTLPixelFormat::R8Unorm,
            PageKind::Color => FORMAT,
        },
        MTLTextureUsage::ShaderRead,
        if shared {
            MTLStorageMode::Shared
        } else {
            MTLStorageMode::Managed
        },
    )
}
