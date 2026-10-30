use crate::{
    DevicePixels, Size,
    scene::{AtlasTextureKind, AtlasTile},
    text::RenderGlyphParams,
};
use anyhow::Result;
use rustc_hash::FxHashMap;
use std::{borrow::Cow, cell::RefCell};

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

/// Glyphs and images packed into GPU textures, by key.
pub(crate) struct Atlas<B> {
    tiles: RefCell<FxHashMap<AtlasKey, AtlasTile>>,
    pub(crate) backend: RefCell<B>,
}

pub(crate) type Build<'a> = dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>> + 'a;

impl<B: AtlasBackend> Atlas<B> {
    pub fn new(backend: B) -> Self {
        Self {
            tiles: RefCell::default(),
            backend: RefCell::new(backend),
        }
    }

    pub fn get_or_insert_with(
        &self,
        key: &AtlasKey,
        build: &mut Build<'_>,
    ) -> Result<Option<AtlasTile>> {
        if let Some(tile) = self.tiles.borrow().get(key) {
            return Ok(Some(*tile));
        }
        let Some((size, bytes)) = build()? else {
            return Ok(None);
        };
        let tile = self.backend.borrow_mut().insert(key.kind(), size, &bytes)?;
        self.tiles.borrow_mut().insert(key.clone(), tile);
        Ok(Some(tile))
    }

    pub fn remove(&self, key: &AtlasKey) {
        if let Some(tile) = self.tiles.borrow_mut().remove(key) {
            self.backend.borrow_mut().remove(tile);
        }
    }

    /// Drops every tile, for a lost device.
    #[allow(dead_code)]
    pub fn clear(&self, reset: impl FnOnce(&mut B)) {
        self.tiles.borrow_mut().clear();
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
