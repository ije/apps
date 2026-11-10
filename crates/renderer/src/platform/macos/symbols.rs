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
    // Keep every symbol used this frame, even over capacity, to avoid rasterizing it again each frame.
    pub fn get(&mut self, name: &str, size: [u16; 2], frame: u64) -> Option<Rc<Symbol>> {
        if let Some(entry) = self
            .images
            .get_mut(name)
            .and_then(|entries| entries.iter_mut().find(|entry| entry.size == size))
        {
            entry.used = frame;
            return entry.image.clone();
        }
        self.clock += 1;
        let image = rasterize(name, size).map(|bytes| {
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
                .filter(|entry| entry.2 != frame)
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
            used: frame,
            image: image.clone(),
        });
        self.bytes += bytes;
        self.entries += 1;
        image
    }
}

// This runs on the render thread. NSImage draws on any thread into a graphics context of that thread.
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
            alpha.extend(row.as_chunks::<4>().0.iter().map(|pixel| pixel[3]));
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
        assert!(pixels.contains(&255));
        assert!(pixels.contains(&0));
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

    #[test]
    fn threads_rasterize_the_same_pixels() {
        let names = ["folder", "arrow.up", "star.fill", "gearshape"];
        let expected: Vec<_> = names
            .iter()
            .map(|name| super::rasterize(name, [48, 40]).unwrap())
            .collect();
        let expected = &expected;
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(move || {
                    for _ in 0..20 {
                        for (name, pixels) in names.iter().zip(expected) {
                            assert_eq!(super::rasterize(name, [48, 40]).as_ref(), Some(pixels));
                        }
                    }
                });
            }
        });
    }

    #[test]
    fn a_frame_keeps_every_symbol_it_used() {
        let mut cache = super::Cache::default();
        let sizes = 8..8 + super::MAX_ENTRIES as u16 + 40;
        for _ in 0..2 {
            for side in sizes.clone() {
                assert!(cache.get("circle", [side, side], 1).is_some());
            }
        }
        assert_eq!(cache.clock, sizes.len() as u64);
        assert!(cache.retired.is_empty());
        cache.get("circle", [8, 8], 2);
        cache.get("square", [8, 8], 2);
        assert_eq!(cache.entries, super::MAX_ENTRIES);
        assert!(cache.get("circle", [8, 8], 3).is_some());
        assert_eq!(cache.clock, sizes.len() as u64 + 1);
    }
}
