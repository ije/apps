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

/// Past this many glyph pixels, glyphs unused for `MAX_AGE` frames are dropped.
const MAX_GLYPH_PIXELS: usize = 4 << 20;
const MAX_AGE: u32 = 60;

/// What an atlas tile holds.
#[derive(PartialEq, Eq, Hash, Clone, Debug)]
pub(crate) enum AtlasKey {
    Glyph(RenderGlyphParams),
    /// A [`crate::RenderImage`] by id.
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

/// The GPU textures of an atlas.
pub(crate) trait AtlasBackend {
    fn insert(
        &mut self,
        kind: AtlasTextureKind,
        size: Size<DevicePixels>,
        bytes: &[u8],
    ) -> Result<AtlasTile>;
    fn remove(&mut self, tile: AtlasTile);
}

/// A tile, `None` for a glyph without pixels, with the offset of its pixels from the glyph
/// origin and the frame it was last used in.
struct Entry {
    tile: Option<AtlasTile>,
    origin: Point<DevicePixels>,
    used: Cell<u32>,
}

/// Glyphs and images packed into GPU textures, by key.
pub(crate) struct Atlas<B> {
    tiles: RefCell<FxHashMap<AtlasKey, Entry>>,
    frame: Cell<u32>,
    glyph_pixels: Cell<usize>,
    pub(crate) backend: RefCell<B>,
}

/// The offset of a tile's pixels from the glyph origin, their size and bytes.
pub(crate) type Raster<B> = (Point<DevicePixels>, Size<DevicePixels>, B);

pub(crate) type Build<'a> = dyn FnMut() -> Result<Option<Raster<Cow<'a, [u8]>>>> + 'a;

fn pixels(tile: &AtlasTile) -> usize {
    tile.bounds.size.width.0 as usize * tile.bounds.size.height.0 as usize
}

impl<B: AtlasBackend> Atlas<B> {
    pub fn new(backend: B) -> Self {
        Self {
            tiles: RefCell::default(),
            frame: Cell::new(0),
            glyph_pixels: Cell::new(0),
            backend: RefCell::new(backend),
        }
    }

    /// The tile and the offset of its pixels. `build` returns `None` for a glyph without pixels.
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
            self.backend.borrow_mut().remove(tile);
        }
    }

    /// Ends a frame, and drops the glyphs unused for a while once there are too many.
    pub fn finish_frame(&self) {
        let frame = self.frame.replace(self.frame.get().wrapping_add(1));
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

    /// Drops every tile, for a lost device.
    #[allow(dead_code)]
    pub fn clear(&self, reset: impl FnOnce(&mut B)) {
        self.tiles.borrow_mut().clear();
        self.glyph_pixels.set(0);
        reset(&mut self.backend.borrow_mut());
    }
}

/// Textures of one kind, with slots of freed textures reused.
pub(crate) struct TextureList<T> {
    pub textures: Vec<Option<T>>,
    pub free: Vec<usize>,
}

impl<T> Default for TextureList<T> {
    fn default() -> Self {
        Self {
            textures: Vec::new(),
            free: Vec::new(),
        }
    }
}

impl<T> TextureList<T> {
    pub fn get(&self, index: usize) -> Option<&T> {
        self.textures.get(index)?.as_ref()
    }

    /// Stores a texture and returns its index.
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

/// One atlas texture's space, packed by etagere.
pub(crate) struct Packer {
    allocator: etagere::BucketedAtlasAllocator,
    live: usize,
}

impl Packer {
    pub fn new(size: Size<DevicePixels>) -> Self {
        Self {
            allocator: etagere::BucketedAtlasAllocator::new(etagere::size2(
                size.width.0,
                size.height.0,
            )),
            live: 0,
        }
    }

    /// The tile bounds and id, or `None` when the texture is full.
    pub fn allocate(
        &mut self,
        size: Size<DevicePixels>,
    ) -> Option<(crate::Bounds<DevicePixels>, u32)> {
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

    /// Frees a tile and returns whether the texture is empty.
    pub fn free(&mut self, id: u32) -> bool {
        self.allocator.deallocate(etagere::AllocId::deserialize(id));
        self.live = self.live.saturating_sub(1);
        self.live == 0
    }
}
