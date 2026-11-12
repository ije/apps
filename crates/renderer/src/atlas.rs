use crate::{Rect, Size, text::GlyphRaster};
use anyhow::{Context, Result};
use rustc_hash::FxHashMap;
use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
};

const MAX_GLYPH_PIXELS: usize = 4 << 20;
pub(crate) const MAX_GLYPH_SIDE: i32 = 2048;
const MAX_AGE: u32 = 60;
// Keep retired tiles until in-flight frames finish. Increase this delay if a window uses more than 3
// drawables.
const IN_FLIGHT: u32 = 2;
const MIN_TEXTURE_SIDE: i32 = 1024;

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) enum TileKey {
    Glyph(GlyphRaster),
    Image(u64),
    #[cfg(target_os = "macos")]
    Symbol(u64),
}

impl TileKey {
    fn kind(&self) -> PageKind {
        match self {
            #[cfg(target_os = "macos")]
            Self::Symbol(_) => PageKind::Mask,
            Self::Glyph(params) if !params.emoji => PageKind::Mask,
            _ => PageKind::Color,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum PageKind {
    Mask,
    Color,
}

#[derive(Clone, Copy)]
pub(crate) struct Tile {
    pub page: u32,
    pub kind: PageKind,
    pub id: u32,
    pub rect: Rect<i32>,
}

pub(crate) trait PageStore {
    fn insert(
        &mut self,
        kind: PageKind,
        size: Size<i32>,
        bytes: &[u8],
    ) -> Result<(usize, Rect<i32>, u32)>;
    fn remove(&mut self, tile: Tile);
    fn max_side(&self) -> i32;
}

struct Entry {
    tile: Option<Tile>,
    bounds: Rect<i32>,
    used: Cell<u32>,
}

pub(crate) struct Atlas<B> {
    tiles: RefCell<FxHashMap<TileKey, Entry>>,
    retired: RefCell<Vec<(u32, Tile)>>,
    frame: Cell<u32>,
    glyph_pixels: Cell<usize>,
    swept: Cell<u32>,
    pub(crate) pages: RefCell<B>,
}

pub(crate) type Raster<B> = (Rect<i32>, Size<i32>, B);

pub(crate) type Build<'a> = dyn FnMut() -> Result<Option<Raster<Cow<'a, [u8]>>>> + 'a;

fn pixels(tile: &Tile) -> usize {
    tile.rect.size.width as usize * tile.rect.size.height as usize
}

impl<B: PageStore> Atlas<B> {
    pub fn new(pages: B) -> Self {
        Self {
            tiles: RefCell::default(),
            retired: RefCell::default(),
            frame: Cell::new(0),
            glyph_pixels: Cell::new(0),
            swept: Cell::new(0),
            pages: RefCell::new(pages),
        }
    }

    pub fn get_or_insert_with(
        &self,
        key: &TileKey,
        build: &mut Build<'_>,
    ) -> Option<(Tile, Rect<i32>)> {
        if let Some(entry) = self.tiles.borrow().get(key) {
            entry.used.set(self.frame.get());
            return entry.tile.map(|tile| (tile, entry.bounds));
        }
        // Cache failed or oversized tiles as empty to prevent allocation retries every frame.
        let (tile, bounds) = match build() {
            Ok(Some((bounds, size, bytes))) => {
                let (max, kind) = (self.pages.borrow().max_side(), key.kind());
                let pixel = match kind {
                    PageKind::Mask => 1,
                    PageKind::Color => 4,
                };
                let tile =
                    (size.width > 0 && size.height > 0 && size.width <= max && size.height <= max)
                        .then(|| bytes.get(..size.width as usize * pixel * size.height as usize))
                        .flatten()
                        .and_then(|bytes| self.pages.borrow_mut().insert(kind, size, bytes).ok())
                        .map(|(page, rect, id)| Tile {
                            page: page as u32,
                            kind,
                            id,
                            rect,
                        });
                if let (Some(tile), TileKey::Glyph(_)) = (tile, key) {
                    self.glyph_pixels
                        .set(self.glyph_pixels.get() + pixels(&tile));
                }
                (tile, bounds)
            }
            _ => (None, Rect::default()),
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

    pub fn remove(&self, key: &TileKey) {
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
            let mut pages = self.pages.borrow_mut();
            retired.retain(|(removed, tile)| {
                let keep = frame.wrapping_sub(*removed) < IN_FLIGHT;
                if !keep {
                    pages.remove(*tile);
                }
                keep
            });
        }
        drop(retired);
        let mut glyph_pixels = self.glyph_pixels.get();
        // Glyphs used in the last frames stay, so a sweep that frees too little isn't repeated at once.
        if glyph_pixels <= MAX_GLYPH_PIXELS || frame.wrapping_sub(self.swept.get()) < MAX_AGE / 4 {
            return;
        }
        self.swept.set(frame);
        let mut pages = self.pages.borrow_mut();
        self.tiles.borrow_mut().retain(|key, entry| {
            if !matches!(key, TileKey::Glyph(_)) || frame.wrapping_sub(entry.used.get()) < MAX_AGE {
                return true;
            }
            if let Some(tile) = entry.tile {
                glyph_pixels -= pixels(&tile);
                pages.remove(tile);
            }
            false
        });
        self.glyph_pixels.set(glyph_pixels);
    }

    #[cfg(target_os = "macos")]
    pub fn free_retired(&self, wait: impl FnOnce()) {
        let mut retired = self.retired.borrow_mut();
        if retired.is_empty() {
            return;
        }
        wait();
        let mut pages = self.pages.borrow_mut();
        for (_, tile) in retired.drain(..) {
            pages.remove(tile);
        }
    }

    // The scene still reads the tiles of the last frame, so they must stay.
    #[cfg(target_os = "macos")]
    pub fn release_images(&self) {
        let frame = self.frame.get();
        let mut retired = self.retired.borrow_mut();
        self.tiles.borrow_mut().retain(|key, entry| {
            if !matches!(key, TileKey::Image(_)) || frame.wrapping_sub(entry.used.get()) <= 1 {
                return true;
            }
            retired.extend(entry.tile.map(|tile| (frame, tile)));
            false
        });
    }

    #[cfg(not(target_os = "macos"))]
    pub fn clear(&self, pages: B) {
        self.tiles.borrow_mut().clear();
        self.retired.borrow_mut().clear();
        self.glyph_pixels.set(0);
        *self.pages.borrow_mut() = pages;
    }
}

pub(crate) struct PageList<T> {
    pages: Vec<Option<(Packer, T)>>,
    free: Vec<usize>,
    spare: Option<usize>,
}

impl<T> Default for PageList<T> {
    fn default() -> Self {
        Self {
            pages: Vec::new(),
            free: Vec::new(),
            spare: None,
        }
    }
}

impl<T> PageList<T> {
    pub fn get(&self, index: usize) -> Option<&T> {
        self.pages.get(index)?.as_ref().map(|(_, page)| page)
    }

    fn is_empty(&self, index: usize) -> bool {
        self.pages
            .get(index)
            .and_then(Option::as_ref)
            .is_some_and(|(packer, _)| packer.live == 0)
    }

    pub fn free(&mut self, index: usize, id: u32) -> bool {
        let Some((packer, _)) = self.pages.get_mut(index).and_then(Option::as_mut) else {
            return false;
        };
        packer
            .allocator
            .deallocate(etagere::AllocId::deserialize(id));
        packer.live = packer.live.saturating_sub(1);
        if packer.live != 0 {
            return false;
        }
        if let Some(old) = self.spare.replace(index)
            && old != index
            && self.is_empty(old)
        {
            self.pages[old] = None;
            self.free.push(old);
        }
        true
    }

    #[cfg(target_os = "macos")]
    pub fn trim(&mut self) {
        if let Some(index) = self.spare.take()
            && self.is_empty(index)
        {
            self.pages[index] = None;
            self.free.push(index);
        }
    }

    pub fn allocate(
        &mut self,
        size: Size<i32>,
        page: impl FnOnce(Size<i32>) -> Result<T>,
    ) -> Result<(usize, &T, Rect<i32>, u32)> {
        let found = (0..self.pages.len()).rev().find_map(|index| {
            let (packer, _) = self.pages[index].as_mut()?;
            packer.allocate(size).map(|(rect, id)| (index, rect, id))
        });
        let (index, rect, id) = match found {
            Some(found) => found,
            None => {
                let side = page_size(size);
                let mut packer = Packer::new(side);
                let (rect, id) = packer.allocate(size).context("a new page has no room")?;
                let page = Some((packer, page(side)?));
                let index = match self.free.pop() {
                    Some(index) => {
                        self.pages[index] = page;
                        index
                    }
                    None => {
                        self.pages.push(page);
                        self.pages.len() - 1
                    }
                };
                (index, rect, id)
            }
        };
        let (_, page) = self.pages[index].as_ref().context("atlas page")?;
        Ok((index, page, rect, id))
    }
}

struct Packer {
    allocator: etagere::BucketedAtlasAllocator,
    size: Size<i32>,
    live: usize,
}

fn page_size(tile: Size<i32>) -> Size<i32> {
    if tile.width <= MIN_TEXTURE_SIDE && tile.height <= MIN_TEXTURE_SIDE {
        Size::new(MIN_TEXTURE_SIDE, MIN_TEXTURE_SIDE)
    } else {
        tile
    }
}

impl Packer {
    fn new(size: Size<i32>) -> Self {
        Self {
            allocator: etagere::BucketedAtlasAllocator::new(etagere::size2(
                size.width,
                size.height,
            )),
            size,
            live: 0,
        }
    }

    fn allocate(&mut self, size: Size<i32>) -> Option<(crate::Rect<i32>, u32)> {
        // Keep small tiles off large pages so an icon cannot retain a large texture.
        if page_size(size) != self.size {
            return None;
        }
        let allocation = self
            .allocator
            .allocate(etagere::size2(size.width, size.height))?;
        self.live += 1;
        let origin = allocation.rectangle.min;
        Some((
            crate::Rect::new(crate::point(origin.x, origin.y), size),
            allocation.id.serialize(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        list: PageList<Size<i32>>,
        made: usize,
        freed: Vec<Tile>,
    }

    fn raster(width: i32, height: i32) -> Option<Raster<Cow<'static, [u8]>>> {
        let size = Size::new(width, height);
        let bytes = vec![0; width.max(0) as usize * height.max(0) as usize * 4];
        Some((Rect::new(Default::default(), size), size, bytes.into()))
    }

    impl PageStore for Fake {
        fn insert(
            &mut self,
            _: PageKind,
            size: Size<i32>,
            _: &[u8],
        ) -> Result<(usize, Rect<i32>, u32)> {
            let made = &mut self.made;
            let (index, _, rect, id) = self.list.allocate(size, |size| {
                *made += 1;
                Ok(size)
            })?;
            Ok((index, rect, id))
        }

        fn max_side(&self) -> i32 {
            16384
        }

        fn remove(&mut self, tile: Tile) {
            self.freed.push(tile);
            self.list.free(tile.page as usize, tile.id);
        }
    }

    #[test]
    fn small_tiles_stay_out_of_pages_made_for_big_ones() {
        let atlas = Atlas::new(Fake::default());
        let insert = |id, width, height| {
            atlas
                .get_or_insert_with(&TileKey::Image(id), &mut || Ok(raster(width, height)))
                .unwrap()
                .0
        };
        let big = insert(1, 1686, 1024);
        let icon = insert(2, 32, 32);
        assert_ne!(big.page, icon.page);
        assert_eq!(atlas.pages.borrow().made, 2);
        for (id, width, height) in [(3, 1100, 37), (4, 1100, 1), (5, 37, 1100), (6, 16384, 3)] {
            let tile = insert(id, width, height);
            assert_eq!(tile.rect.size, Size::new(width, height));
            let backend = atlas.pages.borrow();
            let page = backend.list.get(tile.page as usize).unwrap();
            assert_eq!(*page, Size::new(width, height));
        }
        atlas.remove(&TileKey::Image(1));
        atlas.finish_frame();
        atlas.finish_frame();
        atlas.finish_frame();
        let backend = atlas.pages.borrow();
        assert!(backend.list.is_empty(big.page as usize));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn quiet_release_keeps_the_tiles_of_the_last_frame() {
        let atlas = Atlas::new(Fake::default());
        let insert = |id| {
            atlas
                .get_or_insert_with(&TileKey::Image(id), &mut || Ok(raster(8, 8)))
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
        let freed = &atlas.pages.borrow().freed;
        assert_eq!(freed.len(), 1);
        assert_ne!(freed[0].id, shown.id);
        assert!(atlas.tiles.borrow().get(&TileKey::Image(1)).is_none());
    }

    #[test]
    fn replaced_frames_reuse_textures_after_frames_in_flight() {
        let atlas = Atlas::new(Fake::default());
        let mut inserted = Vec::new();
        for frame in 0..30u64 {
            if frame > 0 {
                atlas.remove(&TileKey::Image(frame - 1));
            }
            let tile = atlas
                .get_or_insert_with(&TileKey::Image(frame), &mut || Ok(raster(1920, 1080)))
                .unwrap()
                .0;
            inserted.push(tile);
            atlas.finish_frame();
            let backend = atlas.pages.borrow();
            assert!(backend.freed.len() as u64 <= frame.saturating_sub(IN_FLIGHT as u64));
            let key = |tile: &Tile| (tile.page, tile.id, tile.rect);
            assert!(
                backend
                    .freed
                    .iter()
                    .map(key)
                    .eq(inserted.iter().map(key).take(backend.freed.len()))
            );
        }
        assert!(atlas.pages.borrow().made <= IN_FLIGHT as usize + 2);
        let tile = atlas
            .get_or_insert_with(&TileKey::Image(1000), &mut || Ok(raster(10, 10)))
            .unwrap()
            .0;
        let backend = atlas.pages.borrow();
        let texture = backend.list.get(tile.page as usize).unwrap();
        assert_eq!(*texture, Size::new(1024, 1024));
    }

    #[test]
    fn glyph_sweeps_that_free_nothing_wait() {
        let atlas = Atlas::new(Fake::default());
        atlas.glyph_pixels.set(MAX_GLYPH_PIXELS + 1);
        let mut sweeps = Vec::new();
        for _ in 0..46 {
            let before = atlas.swept.get();
            atlas.finish_frame();
            if atlas.swept.get() != before {
                sweeps.push(atlas.swept.get());
            }
        }
        assert_eq!(sweeps, [MAX_AGE / 4, MAX_AGE / 2, MAX_AGE / 4 * 3]);
    }

    #[test]
    fn failed_and_oversized_tiles_build_once_and_make_no_texture() {
        let atlas = Atlas::new(Fake::default());
        let builds = Cell::new(0);
        for _ in 0..3 {
            let tile = atlas.get_or_insert_with(&TileKey::Image(1), &mut || {
                builds.set(builds.get() + 1);
                Ok(raster(16385, 10))
            });
            assert!(tile.is_none());
            let tile = atlas.get_or_insert_with(&TileKey::Image(2), &mut || {
                builds.set(builds.get() + 1);
                Err(anyhow::anyhow!("no pixels"))
            });
            assert!(tile.is_none());
            atlas.finish_frame();
        }
        assert_eq!(builds.get(), 2);
        assert_eq!(atlas.pages.borrow().made, 0);
        atlas.remove(&TileKey::Image(1));
        assert!(
            atlas
                .get_or_insert_with(&TileKey::Image(1), &mut || Ok(raster(16384, 10)))
                .is_some()
        );
    }
}
