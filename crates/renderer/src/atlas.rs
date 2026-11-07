use crate::{
    Bounds, DevicePixels, Size,
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
// WARNING: a glyph rasters at most this many device pixels on a side. A bigger one rasters smaller and stretches.
pub(crate) const MAX_GLYPH_SIDE: i32 = 2048;
const MAX_AGE: u32 = 60;
// WARNING: removed tiles are freed this many frames later, when no frame in flight reads them.
// WARNING: a window has at most 3 drawables on macOS, so the frame before the removal, the last
// one to read a tile, is done before the frame after the free paints. More drawables need more.
const IN_FLIGHT: u32 = 2;
const MIN_TEXTURE_SIDE: i32 = 1024;

#[derive(PartialEq, Eq, Hash, Clone, Debug)]
pub(crate) enum AtlasKey {
    Glyph(RenderGlyphParams),
    Image(u64),
    #[cfg(target_os = "macos")]
    Symbol(u64),
}

impl AtlasKey {
    fn kind(&self) -> AtlasTextureKind {
        match self {
            #[cfg(target_os = "macos")]
            Self::Symbol(_) => AtlasTextureKind::Monochrome,
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
    fn max_side(&self) -> i32;
}

struct Entry {
    tile: Option<AtlasTile>,
    bounds: Bounds<DevicePixels>,
    used: Cell<u32>,
}

pub(crate) struct Atlas<B> {
    tiles: RefCell<FxHashMap<AtlasKey, Entry>>,
    retired: RefCell<Vec<(u32, AtlasTile)>>,
    frame: Cell<u32>,
    glyph_pixels: Cell<usize>,
    pub(crate) backend: RefCell<B>,
}

// WARNING: the bounds say where the tile shows. A stretched glyph shows bigger than its pixels.
pub(crate) type Raster<B> = (Bounds<DevicePixels>, Size<DevicePixels>, B);

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
    ) -> Option<(AtlasTile, Bounds<DevicePixels>)> {
        if let Some(entry) = self.tiles.borrow().get(key) {
            entry.used.set(self.frame.get());
            return entry.tile.map(|tile| (tile, entry.bounds));
        }
        // WARNING: a tile that fails or is too big stays as an empty entry, or each frame would build it again
        // and the backend could keep a texture for it each time.
        let (tile, bounds) = match build() {
            Ok(Some((bounds, size, bytes))) => {
                let max = self.backend.borrow().max_side();
                let tile = if size.width.0 <= max && size.height.0 <= max {
                    self.backend
                        .borrow_mut()
                        .insert(key.kind(), size, &bytes)
                        .ok()
                } else {
                    None
                };
                if let (Some(tile), AtlasKey::Glyph(_)) = (tile, key) {
                    self.glyph_pixels
                        .set(self.glyph_pixels.get() + pixels(&tile));
                }
                (tile, bounds)
            }
            _ => (None, Bounds::default()),
        };
        self.tiles.borrow_mut().insert(
            key.clone(),
            Entry {
                tile,
                bounds,
                used: Cell::new(self.frame.get()),
            },
        );
        tile.map(|tile| (tile, bounds))
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

    // WARNING: images keep their pixels, so a frame that shows one again only uploads it. The scene reads the
    // tiles of the last frame, so they stay.
    #[cfg(target_os = "macos")]
    pub fn release_images(&self) {
        let frame = self.frame.get();
        let mut retired = self.retired.borrow_mut();
        self.tiles.borrow_mut().retain(|key, entry| {
            if !matches!(key, AtlasKey::Image(_)) || frame.wrapping_sub(entry.used.get()) <= 1 {
                return true;
            }
            retired.extend(entry.tile.map(|tile| (frame, tile)));
            false
        });
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

pub(crate) fn page_size(tile: Size<DevicePixels>) -> Size<DevicePixels> {
    if tile.width.0 <= MIN_TEXTURE_SIDE && tile.height.0 <= MIN_TEXTURE_SIDE {
        Size::new(
            DevicePixels(MIN_TEXTURE_SIDE),
            DevicePixels(MIN_TEXTURE_SIDE),
        )
    } else {
        tile
    }
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
        // WARNING: a page made for a big tile takes no small ones, or an icon keeps it after the big tile is gone.
        if page_size(size) != self.size {
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

    fn raster(width: i32, height: i32) -> Option<Raster<Cow<'static, [u8]>>> {
        let size = Size::new(DevicePixels(width), DevicePixels(height));
        Some((
            Bounds::new(Default::default(), size),
            size,
            Cow::Borrowed(&[]),
        ))
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
                    let index = self.list.push(|_| Packer::new(page_size(size)));
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

        fn max_side(&self) -> i32 {
            16384
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
    fn small_tiles_stay_out_of_pages_made_for_big_ones() {
        let atlas = Atlas::new(Fake::default());
        let insert = |id, width, height| {
            atlas
                .get_or_insert_with(&AtlasKey::Image(id), &mut || Ok(raster(width, height)))
                .unwrap()
                .0
        };
        let big = insert(1, 1686, 1024);
        let icon = insert(2, 32, 32);
        assert_ne!(big.texture_id.index, icon.texture_id.index);
        assert_eq!(atlas.backend.borrow().made, 2);
        for (id, width, height) in [(3, 1100, 37), (4, 1100, 1), (5, 37, 1100), (6, 16384, 3)] {
            let tile = insert(id, width, height);
            assert_eq!(
                tile.bounds.size,
                Size::new(DevicePixels(width), DevicePixels(height))
            );
            let backend = atlas.backend.borrow();
            let page = backend.list.get(tile.texture_id.index as usize).unwrap();
            assert_eq!(
                page.size,
                Size::new(DevicePixels(width), DevicePixels(height))
            );
        }
        atlas.remove(&AtlasKey::Image(1));
        atlas.finish_frame();
        atlas.finish_frame();
        atlas.finish_frame();
        let backend = atlas.backend.borrow();
        assert!(
            backend
                .list
                .get(big.texture_id.index as usize)
                .unwrap()
                .is_empty()
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn quiet_release_keeps_the_tiles_of_the_last_frame() {
        let atlas = Atlas::new(Fake::default());
        let insert = |id| {
            atlas
                .get_or_insert_with(&AtlasKey::Image(id), &mut || Ok(raster(8, 8)))
                .unwrap()
                .0
        };
        insert(1);
        insert(2);
        atlas.finish_frame();
        let shown = insert(2);
        atlas.finish_frame();
        atlas.release_images();
        atlas.free_retired(|| {});
        let freed = &atlas.backend.borrow().freed;
        assert_eq!(freed.len(), 1);
        assert_ne!(freed[0].tile_id, shown.tile_id);
        assert!(atlas.tiles.borrow().get(&AtlasKey::Image(1)).is_none());
    }

    #[test]
    fn replaced_frames_reuse_textures_after_frames_in_flight() {
        let atlas = Atlas::new(Fake::default());
        let mut inserted = Vec::new();
        for frame in 0..30u64 {
            if frame > 0 {
                atlas.remove(&AtlasKey::Image(frame - 1));
            }
            let tile = atlas
                .get_or_insert_with(&AtlasKey::Image(frame), &mut || Ok(raster(1920, 1080)))
                .unwrap()
                .0;
            inserted.push(tile);
            atlas.finish_frame();
            let backend = atlas.backend.borrow();
            assert!(backend.freed.len() as u64 <= frame.saturating_sub(IN_FLIGHT as u64));
            assert_eq!(&backend.freed[..], &inserted[..backend.freed.len()]);
        }
        assert!(atlas.backend.borrow().made <= IN_FLIGHT as usize + 2);
        let tile = atlas
            .get_or_insert_with(&AtlasKey::Image(1000), &mut || Ok(raster(10, 10)))
            .unwrap()
            .0;
        let backend = atlas.backend.borrow();
        let texture = backend.list.get(tile.texture_id.index as usize).unwrap();
        assert_eq!(
            texture.size,
            Size::new(DevicePixels(1024), DevicePixels(1024))
        );
    }

    #[test]
    fn failed_and_oversized_tiles_build_once_and_make_no_texture() {
        let atlas = Atlas::new(Fake::default());
        let builds = Cell::new(0);
        for _ in 0..3 {
            let tile = atlas.get_or_insert_with(&AtlasKey::Image(1), &mut || {
                builds.set(builds.get() + 1);
                Ok(raster(16385, 10))
            });
            assert!(tile.is_none());
            let tile = atlas.get_or_insert_with(&AtlasKey::Image(2), &mut || {
                builds.set(builds.get() + 1);
                Err(anyhow::anyhow!("no pixels"))
            });
            assert!(tile.is_none());
            atlas.finish_frame();
        }
        assert_eq!(builds.get(), 2);
        assert_eq!(atlas.backend.borrow().made, 0);
        atlas.remove(&AtlasKey::Image(1));
        assert!(
            atlas
                .get_or_insert_with(&AtlasKey::Image(1), &mut || Ok(raster(16384, 10)))
                .is_some()
        );
    }
}
