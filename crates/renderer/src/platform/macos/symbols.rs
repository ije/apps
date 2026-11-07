use objc2::{
    class, msg_send,
    rc::{Retained, autoreleasepool},
    runtime::{AnyObject, Bool},
    sel,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use rustc_hash::FxHashMap;
use std::{ptr, rc::Rc};

const MAX_BYTES: usize = 2 << 20;
const MAX_ENTRIES: usize = 128;

pub(crate) struct Symbol {
    pub id: u64,
    pub size: [u16; 2],
    pub bytes: Vec<u8>,
}

struct Entry {
    size: [u16; 2],
    used: u64,
    image: Option<Rc<Symbol>>,
}

#[derive(Default)]
pub(crate) struct Cache {
    images: FxHashMap<String, Vec<Entry>>,
    bytes: usize,
    entries: usize,
    clock: u64,
    pub retired: Vec<u64>,
}

impl Cache {
    pub fn get(&mut self, name: &str, size: [u16; 2]) -> Option<Rc<Symbol>> {
        self.clock += 1;
        if let Some(entry) = self
            .images
            .get_mut(name)
            .and_then(|entries| entries.iter_mut().find(|entry| entry.size == size))
        {
            entry.used = self.clock;
            return entry.image.clone();
        }
        let image = super::native_sync(|| rasterize(name, size)).map(|bytes| {
            Rc::new(Symbol {
                id: self.clock,
                size,
                bytes,
            })
        });
        let bytes = image.as_ref().map_or(0, |image| image.bytes.len());
        while self.entries >= MAX_ENTRIES || self.bytes + bytes > MAX_BYTES {
            let Some((name, index)) = self
                .images
                .iter()
                .flat_map(|(name, entries)| {
                    entries
                        .iter()
                        .enumerate()
                        .map(move |(index, entry)| (name, index, entry.used))
                })
                .min_by_key(|entry| entry.2)
                .map(|(name, index, _)| (name.clone(), index))
            else {
                break;
            };
            let entries = self.images.get_mut(&name).unwrap();
            if let Some(image) = entries.swap_remove(index).image {
                self.bytes -= image.bytes.len();
                self.retired.push(image.id);
            }
            self.entries -= 1;
            if entries.is_empty() {
                self.images.remove(&name);
            }
        }
        self.images.entry(name.to_owned()).or_default().push(Entry {
            size,
            used: self.clock,
            image: image.clone(),
        });
        self.bytes += bytes;
        self.entries += 1;
        image
    }
}

fn rasterize(name: &str, [width, height]: [u16; 2]) -> Option<Vec<u8>> {
    if width == 0 || height == 0 {
        return None;
    }
    autoreleasepool(|_| unsafe {
        let supported: Bool = msg_send![class!(NSImage), respondsToSelector: sel!(imageWithSystemSymbolName:accessibilityDescription:)];
        if !supported.as_bool() {
            return None;
        }
        let name = NSString::from_str(name);
        let mut image: *mut AnyObject = msg_send![class!(NSImage), imageWithSystemSymbolName: &*name, accessibilityDescription: ptr::null::<AnyObject>()];
        if image.is_null() {
            return None;
        }
        let mono: Bool = msg_send![class!(NSImageSymbolConfiguration), respondsToSelector: sel!(configurationPreferringMonochrome)];
        if mono.as_bool() {
            let configuration: *mut AnyObject = msg_send![
                class!(NSImageSymbolConfiguration),
                configurationPreferringMonochrome
            ];
            let configured: *mut AnyObject =
                msg_send![image, imageWithSymbolConfiguration: configuration];
            if !configured.is_null() {
                image = configured;
            }
        }
        let intrinsic: NSSize = msg_send![image, size];
        if intrinsic.width <= 0. || intrinsic.height <= 0. {
            return None;
        }
        let bitmap: *mut AnyObject = msg_send![class!(NSBitmapImageRep), alloc];
        let bitmap: *mut AnyObject = msg_send![bitmap,
            initWithBitmapDataPlanes: ptr::null_mut::<*mut u8>(),
            pixelsWide: width as isize, pixelsHigh: height as isize,
            bitsPerSample: 8isize, samplesPerPixel: 4isize,
            hasAlpha: Bool::YES, isPlanar: Bool::NO,
            colorSpaceName: &*NSString::from_str("NSDeviceRGBColorSpace"),
            bitmapFormat: 0usize, bytesPerRow: width as isize * 4,
            bitsPerPixel: 32isize
        ];
        let bitmap = Retained::from_raw(bitmap)?;
        let pixels: *mut u8 = msg_send![&*bitmap, bitmapData];
        let stride: isize = msg_send![&*bitmap, bytesPerRow];
        if pixels.is_null() || stride < width as isize * 4 {
            return None;
        }
        ptr::write_bytes(pixels, 0, stride as usize * usize::from(height));
        let context: *mut AnyObject =
            msg_send![class!(NSGraphicsContext), graphicsContextWithBitmapImageRep: &*bitmap];
        if context.is_null() {
            return None;
        }
        let ratio = (f64::from(width) / intrinsic.width).min(f64::from(height) / intrinsic.height);
        let size = NSSize::new(intrinsic.width * ratio, intrinsic.height * ratio);
        let bounds = NSRect::new(
            NSPoint::new(
                (f64::from(width) - size.width) * 0.5,
                (f64::from(height) - size.height) * 0.5,
            ),
            size,
        );
        let () = msg_send![class!(NSGraphicsContext), saveGraphicsState];
        let () = msg_send![class!(NSGraphicsContext), setCurrentContext: context];
        let () = msg_send![image, drawInRect: bounds];
        let () = msg_send![class!(NSGraphicsContext), restoreGraphicsState];
        let mut alpha = Vec::with_capacity(usize::from(width) * usize::from(height));
        for y in 0..usize::from(height) {
            let row =
                std::slice::from_raw_parts(pixels.add(y * stride as usize), usize::from(width) * 4);
            alpha.extend(row.chunks_exact(4).map(|pixel| pixel[3]));
        }
        Some(alpha)
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn system_symbols_have_pixels_and_unknown_names_are_empty() {
        let pixels = super::rasterize("folder", [64, 64]).unwrap();
        assert_eq!(pixels.len(), 64 * 64);
        assert!(pixels.iter().any(|alpha| *alpha == 255));
        assert!(pixels.iter().any(|alpha| *alpha == 0));
        let arrow = super::rasterize("arrow.up", [64, 64]).unwrap();
        let top: u32 = arrow[..32 * 64].iter().map(|alpha| u32::from(*alpha)).sum();
        let bottom: u32 = arrow[32 * 64..].iter().map(|alpha| u32::from(*alpha)).sum();
        assert!(
            top > bottom,
            "the arrow head must be above its stem: {top} <= {bottom}"
        );
        assert!(super::rasterize("apps.unknown.symbol.name", [64, 64]).is_none());
        assert!(super::rasterize("folder", [0, 64]).is_none());
    }
}
