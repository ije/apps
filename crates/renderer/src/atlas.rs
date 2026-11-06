use crate::{
    DevicePixels, Point, Size,
    scene::{AtlasTextureKind, AtlasTile},
    text::RenderGlyphParams,
};
use anyhow::Result;
use rustc_hash::FxHashMap;
use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
};

const MAX_GLYPH_PIXELS: usize = 4 << 20;
const MAX_AGE: u32 = 60;
// WARNING: removed tiles are freed this many frames later, when no frame in flight reads them.
// WARNING: a window has at most 3 drawables on macOS, so the frame before the removal, the last
// one to read a tile, is done before the frame after the free paints. More drawables need more.
const IN_FLIGHT: u32 = 2;
pub(crate) const MIN_TEXTURE_SIDE: i32 = 1024;

#[derive(PartialEq, Eq, Hash, Clone, Debug)]
pub(crate) enum AtlasKey {
    Glyph(RenderGlyphParams),
    Image(u64),
}

impl AtlasKey {
    fn kind(&self) -> AtlasTextureKind {
        match self {
            Self::Glyph(params) if !params.is_emoji => AtlasTextureKind::Monochrome,
            _ => AtlasTextureKind::Polychrome,
        }
    }
}

pub(crate) trait AtlasBackend {
    fn insert(
        &mut self,
        kind: AtlasTextureKind,
        size: Size<DevicePixels>,
        bytes: &[u8],
    ) -> Result<AtlasTile>;
    fn remove(&mut self, tile: AtlasTile);
}

struct Entry {
    tile: Option<AtlasTile>,
    origin: Point<DevicePixels>,
    used: Cell<u32>,
}

pub(crate) struct Atlas<B> {
    tiles: RefCell<FxHashMap<AtlasKey, Entry>>,
    retired: RefCell<Vec<(u32, AtlasTile)>>,
    frame: Cell<u32>,
    glyph_pixels: Cell<usize>,
    pub(crate) backend: RefCell<B>,
}

pub(crate) type Raster<B> = (Point<DevicePixels>, Size<DevicePixels>, B);

pub(crate) type Build<'a> = dyn FnMut() -> Result<Option<Raster<Cow<'a, [u8]>>>> + 'a;

fn pixels(tile: &AtlasTile) -> usize {
    tile.bounds.size.width.0 as usize * tile.bounds.size.height.0 as usize
}

impl<B: AtlasBackend> Atlas<B> {
    pub fn new(backend: B) -> Self {
        Self {
            tiles: RefCell::default(),
            retired: RefCell::default(),
            frame: Cell::new(0),
            glyph_pixels: Cell::new(0),
            backend: RefCell::new(backend),
        }
    }

    pub fn get_or_insert_with(
        &self,
        key: &AtlasKey,
        build: &mut Build<'_>,
    ) -> Result<Option<(AtlasTile, Point<DevicePixels>)>> {
        if let Some(entry) = self.tiles.borrow().get(key) {
            entry.used.set(self.frame.get());
            return Ok(entry.tile.map(|tile| (tile, entry.origin)));
        }
        let (tile, origin) = match build()? {
            Some((origin, size, bytes)) => {
                let tile = self.backend.borrow_mut().insert(key.kind(), size, &bytes)?;
                if let AtlasKey::Glyph(_) = key {
                    self.glyph_pixels
                        .set(self.glyph_pixels.get() + pixels(&tile));
                }
                (Some(tile), origin)
            }
            None => (None, Point::default()),
        };
        self.tiles.borrow_mut().insert(
            key.clone(),
            Entry {
                tile,
                origin,
                used: Cell::new(self.frame.get()),
            },
        );
        Ok(tile.map(|tile| (tile, origin)))
    }

    pub fn remove(&self, key: &AtlasKey) {
        if let Some(Entry {
            tile: Some(tile), ..
        }) = self.tiles.borrow_mut().remove(key)
        {
            self.retired.borrow_mut().push((self.frame.get(), tile));
        }
    }

    pub fn finish_frame(&self) {
        let frame = self.frame.replace(self.frame.get().wrapping_add(1));
        let mut retired = self.retired.borrow_mut();
        if !retired.is_empty() {
            let mut backend = self.backend.borrow_mut();
            retired.retain(|(removed, tile)| {
                let keep = frame.wrapping_sub(*removed) < IN_FLIGHT;
                if !keep {
                    backend.remove(*tile);
                }
                keep
            });
        }
        drop(retired);
        let mut glyph_pixels = self.glyph_pixels.get();
        if glyph_pixels <= MAX_GLYPH_PIXELS {
            return;
        }
        // WARNING: frames in flight may still read a tile, so only long unused tiles are freed.
        let mut backend = self.backend.borrow_mut();
        self.tiles.borrow_mut().retain(|key, entry| {
            if !matches!(key, AtlasKey::Glyph(_)) || frame.wrapping_sub(entry.used.get()) < MAX_AGE
            {
                return true;
            }
            if let Some(tile) = entry.tile {
                glyph_pixels -= pixels(&tile);
                backend.remove(tile);
            }
            false
        });
        self.glyph_pixels.set(glyph_pixels);
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn free_retired(&self, wait: impl FnOnce()) {
        let mut retired = self.retired.borrow_mut();
        if retired.is_empty() {
            return;
        }
        wait();
        let mut backend = self.backend.borrow_mut();
        for (_, tile) in retired.drain(..) {
            backend.remove(tile);
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn clear(&self, reset: impl FnOnce(&mut B)) {
        self.tiles.borrow_mut().clear();
        self.retired.borrow_mut().clear();
        self.glyph_pixels.set(0);
        reset(&mut self.backend.borrow_mut());
    }
}

pub(crate) struct TextureList<T> {
    pub textures: Vec<Option<T>>,
    pub free: Vec<usize>,
    spare: Option<usize>,
}

impl<T> Default for TextureList<T> {
    fn default() -> Self {
        Self {
            textures: Vec::new(),
            free: Vec::new(),
            spare: None,
        }
    }
}

impl<T> TextureList<T> {
    pub fn get(&self, index: usize) -> Option<&T> {
        self.textures.get(index)?.as_ref()
    }

    pub fn emptied(&mut self, index: usize, empty: impl Fn(&T) -> bool) {
        if let Some(old) = self.spare.replace(index)
            && old != index
            && self.get(old).is_some_and(empty)
        {
            self.textures[old] = None;
            self.free.push(old);
        }
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn trim(&mut self, empty: impl Fn(&T) -> bool) {
        if let Some(index) = self.spare.take()
            && self.get(index).is_some_and(empty)
        {
            self.textures[index] = None;
            self.free.push(index);
        }
    }

    pub fn push(&mut self, texture: impl FnOnce(usize) -> T) -> usize {
        match self.free.pop() {
            Some(index) => {
                self.textures[index] = Some(texture(index));
                index
            }
            None => {
                let index = self.textures.len();
                self.textures.push(Some(texture(index)));
                index
            }
        }
    }
}

pub(crate) struct Packer {
    allocator: etagere::BucketedAtlasAllocator,
    size: Size<DevicePixels>,
    live: usize,
}

impl Packer {
    pub fn new(size: Size<DevicePixels>) -> Self {
        Self {
            allocator: etagere::BucketedAtlasAllocator::new(etagere::size2(
                size.width.0,
                size.height.0,
            )),
            size,
            live: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    pub fn allocate(
        &mut self,
        size: Size<DevicePixels>,
    ) -> Option<(crate::Bounds<DevicePixels>, u32)> {
        if self.live == 0
            && size.map(|side| DevicePixels(side.0.max(MIN_TEXTURE_SIDE))) != self.size
        {
            return None;
        }
        let allocation = self
            .allocator
            .allocate(etagere::size2(size.width.0, size.height.0))?;
        self.live += 1;
        let origin = allocation.rectangle.min;
        Some((
            crate::Bounds::new(
                crate::point(DevicePixels(origin.x), DevicePixels(origin.y)),
                size,
            ),
            allocation.id.serialize(),
        ))
    }

    pub fn free(&mut self, id: u32) -> bool {
        self.allocator.deallocate(etagere::AllocId::deserialize(id));
        self.live = self.live.saturating_sub(1);
        self.live == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::AtlasTextureId;

    #[derive(Default)]
    struct Fake {
        list: TextureList<Packer>,
        made: usize,
        freed: Vec<AtlasTile>,
    }

    impl AtlasBackend for Fake {
        fn insert(
            &mut self,
            kind: AtlasTextureKind,
            size: Size<DevicePixels>,
            _: &[u8],
        ) -> Result<AtlasTile> {
            let hit = self
                .list
                .textures
                .iter_mut()
                .enumerate()
                .rev()
                .find_map(|(index, packer)| Some((index, packer.as_mut()?.allocate(size)?)));
            let (index, (bounds, tile_id)) = match hit {
                Some(hit) => hit,
                None => {
                    self.made += 1;
                    let side = size.map(|side| DevicePixels(side.0.max(MIN_TEXTURE_SIDE)));
                    let index = self.list.push(|_| Packer::new(side));
                    let tile = self.list.textures[index].as_mut().unwrap().allocate(size);
                    (index, tile.unwrap())
                }
            };
            Ok(AtlasTile {
                texture_id: AtlasTextureId {
                    index: index as u32,
                    kind,
                },
                tile_id,
                padding: 0,
                bounds,
            })
        }

        fn remove(&mut self, tile: AtlasTile) {
            self.freed.push(tile);
            let index = tile.texture_id.index as usize;
            if self.list.textures[index]
                .as_mut()
                .is_some_and(|packer| packer.free(tile.tile_id))
            {
                self.list.emptied(index, Packer::is_empty);
            }
        }
    }

    #[test]
    fn replaced_frames_reuse_textures_after_frames_in_flight() {
        let atlas = Atlas::new(Fake::default());
        let size = Size::new(DevicePixels(1920), DevicePixels(1080));
        let mut inserted = Vec::new();
        for frame in 0..30u64 {
            if frame > 0 {
                atlas.remove(&AtlasKey::Image(frame - 1));
            }
            let tile = atlas
                .get_or_insert_with(&AtlasKey::Image(frame), &mut || {
                    Ok(Some((Point::default(), size, Cow::Borrowed(&[]))))
                })
                .unwrap()
                .unwrap()
                .0;
            inserted.push(tile);
            atlas.finish_frame();
            let backend = atlas.backend.borrow();
            assert!(backend.freed.len() as u64 <= frame.saturating_sub(IN_FLIGHT as u64));
            assert_eq!(&backend.freed[..], &inserted[..backend.freed.len()]);
        }
        assert!(atlas.backend.borrow().made <= IN_FLIGHT as usize + 2);
        let glyph = Size::new(DevicePixels(10), DevicePixels(10));
        let tile = atlas
            .get_or_insert_with(&AtlasKey::Image(1000), &mut || {
                Ok(Some((Point::default(), glyph, Cow::Borrowed(&[]))))
            })
            .unwrap()
            .unwrap()
            .0;
        let backend = atlas.backend.borrow();
        let texture = backend.list.get(tile.texture_id.index as usize).unwrap();
        assert_eq!(
            texture.size,
            Size::new(DevicePixels(1024), DevicePixels(1024))
        );
    }
}
