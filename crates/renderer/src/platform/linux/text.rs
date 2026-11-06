use crate::{
    Bounds, DevicePixels, Pixels, Rgba, Size, point, px, size,
    text::{
        Font, FontFeatures, FontId, FontMetrics, FontRun, FontStyle, GlyphId, LineLayout,
        RenderGlyphParams, SUBPIXEL_VARIANTS_X, ShapedGlyph, ShapedRun, SharedString,
    },
};
use anyhow::{Context as _, Result};
use cosmic_text::{
    Attrs, AttrsList, Ellipsize, Family, FontFeatures as CosmicFeatures, FontSystem, Metrics,
    ShapeBuffer, ShapeLine, fontdb,
};
use rustc_hash::FxHashMap;
use std::{borrow::Cow, cell::RefCell, ops::Range, sync::Arc, thread::JoinHandle};
use swash::{
    scale::{Render, ScaleContext, Source, StrikeWith, image::Content},
    zeno::{Format, Vector},
};

pub(crate) struct TextSystem(RefCell<State>);

#[derive(Clone, PartialEq, Eq, Hash)]
struct FontKey {
    family: SharedString,
    features: FontFeatures,
}

enum Fonts {
    /// WARNING: scanning the system fonts can take hundreds of milliseconds, so it runs off the
    /// launch path until the first font is needed.
    Loading(Option<JoinHandle<(String, fontdb::Database)>>),
    Ready(Box<FontSystem>),
}

struct LoadedFont {
    font: Arc<cosmic_text::Font>,
    features: CosmicFeatures,
    emoji: bool,
    family: String,
    stretch: fontdb::Stretch,
    style: fontdb::Style,
    weight: fontdb::Weight,
}

struct State {
    fonts: Fonts,
    scratch: ShapeBuffer,
    scale: ScaleContext,
    pending: FxHashMap<RenderGlyphParams, swash::scale::image::Image>,
    loaded: Vec<LoadedFont>,
    loaded_ids: FxHashMap<(fontdb::ID, FontKey), FontId>,
    families: FxHashMap<FontKey, Vec<FontId>>,
}

impl Fonts {
    fn get(&mut self) -> &mut FontSystem {
        if let Self::Loading(thread) = self {
            let (locale, db) = thread
                .take()
                .and_then(|thread| thread.join().ok())
                .unwrap_or_else(|| FontSystem::new().into_locale_and_db());
            *self = Self::Ready(Box::new(FontSystem::new_with_locale_and_db(locale, db)));
        }
        match self {
            Self::Ready(system) => system,
            Self::Loading(_) => unreachable!(),
        }
    }
}

impl TextSystem {
    pub fn new() -> Self {
        let thread = std::thread::Builder::new()
            .name("fonts".into())
            .spawn(|| FontSystem::new().into_locale_and_db())
            .ok();
        Self(RefCell::new(State {
            fonts: Fonts::Loading(thread),
            scratch: ShapeBuffer::default(),
            scale: ScaleContext::new(),
            pending: FxHashMap::default(),
            loaded: Vec::new(),
            loaded_ids: FxHashMap::default(),
            families: FxHashMap::default(),
        }))
    }

    pub fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        let mut state = self.0.borrow_mut();
        state.families.clear();
        let db = state.fonts.get().db_mut();
        for bytes in fonts {
            db.load_font_source(fontdb::Source::Binary(Arc::new(bytes)));
        }
        Ok(())
    }

    pub fn all_font_names(&self) -> Vec<String> {
        let mut state = self.0.borrow_mut();
        state
            .fonts
            .get()
            .db()
            .faces()
            .filter_map(|face| face.families.first().map(|family| family.0.clone()))
            .collect()
    }

    pub fn font_id(&self, font: &Font) -> Result<FontId> {
        let mut state = self.0.borrow_mut();
        let key = FontKey {
            family: font.family.clone(),
            features: font.features.clone(),
        };
        let candidates = match state.families.get(&key) {
            Some(ids) => ids.clone(),
            None => {
                let ids = state.load_family(&key)?;
                state.families.insert(key, ids.clone());
                ids
            }
        };
        let italic = font.style != FontStyle::Normal;
        candidates
            .iter()
            .min_by_key(|id| {
                let face = &state.loaded[id.0];
                let style_penalty = if (face.style != fontdb::Style::Normal) == italic {
                    0
                } else {
                    1000
                };
                style_penalty + (face.weight.0 as i32 - font.weight.0 as i32).unsigned_abs()
            })
            .copied()
            .context("requested font family contains no font matching the other parameters")
    }

    pub fn font_metrics(&self, font_id: FontId) -> FontMetrics {
        let metrics = self.0.borrow().loaded[font_id.0]
            .font
            .as_swash()
            .metrics(&[]);
        FontMetrics {
            units_per_em: metrics.units_per_em as u32,
            ascent: metrics.ascent,
            descent: -metrics.descent,
            line_gap: metrics.leading,
            cap_height: metrics.cap_height,
            x_height: metrics.x_height,
            tall_ascent: false,
            em_ascent: metrics.ascent / (metrics.ascent + metrics.descent),
            bounding_box: Bounds {
                origin: point(0., 0.),
                size: size(metrics.max_width, metrics.ascent + metrics.descent),
            },
        }
    }

    pub fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Size<f32>> {
        let state = self.0.borrow();
        let metrics = state.loaded[font_id.0].font.as_swash().glyph_metrics(&[]);
        let glyph = glyph_id.0 as u16;
        Ok(size(
            metrics.advance_width(glyph),
            metrics.advance_height(glyph),
        ))
    }

    pub fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
        let glyph = self.0.borrow().loaded[font_id.0]
            .font
            .as_swash()
            .charmap()
            .map(ch);
        (glyph != 0).then(|| GlyphId(glyph.into()))
    }

    pub fn glyph_raster_bounds(&self, params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
        let mut state = self.0.borrow_mut();
        let image = state.render_glyph(params)?;
        let bounds = Bounds {
            origin: point(
                DevicePixels(image.placement.left),
                DevicePixels(-image.placement.top),
            ),
            size: size(image.placement.width.into(), image.placement.height.into()),
        };
        if bounds.size.width.0 > 0 && bounds.size.height.0 > 0 {
            state.pending.insert(*params, image);
        }
        Ok(bounds)
    }

    pub fn rasterize_glyph(
        &self,
        params: &RenderGlyphParams,
        bounds: Bounds<DevicePixels>,
    ) -> Result<(Size<DevicePixels>, Vec<u8>)> {
        anyhow::ensure!(
            bounds.size.width.0 > 0 && bounds.size.height.0 > 0,
            "glyph bounds are empty"
        );
        let mut state = self.0.borrow_mut();
        let mut image = match state.pending.remove(params) {
            Some(image) => image,
            None => state.render_glyph(params)?,
        };
        if image.content != Content::Mask {
            let premultiplied = matches!(image.source, Source::ColorOutline(_));
            for pixel in image.data.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
                let alpha = pixel[3] as u32;
                if premultiplied && alpha > 0 {
                    for channel in &mut pixel[..3] {
                        *channel = (*channel as u32 * 255 / alpha).min(255) as u8;
                    }
                }
            }
        }
        Ok((bounds.size, image.data))
    }

    pub fn layout_line(&self, text: &str, font_size: Pixels, font_runs: &[FontRun]) -> LineLayout {
        let mut state = self.0.borrow_mut();
        let mut layout = LineLayout {
            font_size,
            len: text.len(),
            ..Default::default()
        };
        // WARNING: paragraph separators break cosmic-text lines, so each paragraph is shaped on its own.
        let mut start = 0;
        if text
            .bytes()
            .any(|byte| matches!(byte, b'\n' | b'\r' | 0x1c | 0x1d | 0x1e))
            || (!text.is_ascii() && text.chars().any(is_paragraph_separator))
        {
            for (index, separator) in text
                .char_indices()
                .filter(|(_, ch)| is_paragraph_separator(*ch))
            {
                state.shape_segment(text, start..index, font_size, font_runs, &mut layout);
                start = index + separator.len_utf8();
                state.shape_segment(text, index..start, font_size, font_runs, &mut layout);
            }
        }
        state.shape_segment(text, start..text.len(), font_size, font_runs, &mut layout);
        layout
    }

    pub fn glyph_dilation_for_color(&self, _: Rgba) -> u8 {
        0
    }
}

fn is_paragraph_separator(ch: char) -> bool {
    unicode_bidi::bidi_class(ch) == unicode_bidi::BidiClass::B
}

impl State {
    fn load_family(&mut self, key: &FontKey) -> Result<Vec<FontId>> {
        let mut features = CosmicFeatures::new();
        for (tag, value) in key.features.tag_value_list() {
            let tag: [u8; 4] = tag
                .as_bytes()
                .try_into()
                .context("Incorrect feature flag format")?;
            features.set(cosmic_text::FeatureTag::new(&tag), *value);
        }
        let system = self.fonts.get();
        let faces: Vec<_> = system
            .db()
            .faces()
            .filter(|face| face.families.iter().any(|family| *key.family == family.0))
            .map(|face| face.id)
            .collect();
        let mut ids = Vec::new();
        for id in faces {
            if let Some(&font_id) = self.loaded_ids.get(&(id, key.clone())) {
                ids.push(font_id);
                continue;
            }
            let font = system
                .get_font(id, fontdb::Weight::NORMAL)
                .context("Could not load font")?;
            let font_id = FontId(self.loaded.len());
            ids.push(font_id);
            self.loaded
                .push(loaded_font(system, font, features.clone()).context("font face not found")?);
            self.loaded_ids.insert((id, key.clone()), font_id);
        }
        Ok(ids)
    }

    fn render_glyph(&mut self, params: &RenderGlyphParams) -> Result<swash::scale::image::Image> {
        let font = self.loaded[params.font_id.0].font.as_swash();
        let offset = Vector::new(
            params.subpixel_variant.x as f32 / SUBPIXEL_VARIANTS_X as f32 / params.scale_factor,
            params.subpixel_variant.y as f32 / params.scale_factor,
        );
        let mut scaler = self
            .scale
            .builder(font)
            .size(f32::from(params.font_size) * params.scale_factor)
            .hint(true)
            .build();
        let sources: &[Source] = if params.is_emoji {
            &[
                Source::ColorOutline(0),
                Source::ColorBitmap(StrikeWith::BestFit),
                Source::Outline,
            ]
        } else {
            &[Source::Bitmap(StrikeWith::ExactSize), Source::Outline]
        };
        let glyph: u16 = params.glyph_id.0.try_into()?;
        Render::new(sources)
            .format(Format::Alpha)
            .offset(offset)
            .render(&mut scaler, glyph)
            .with_context(|| format!("unable to render glyph via swash for {params:?}"))
    }

    fn font_id_for_cosmic_id(&mut self, id: fontdb::ID) -> Option<FontId> {
        if let Some(index) = self.loaded.iter().position(|font| font.font.id() == id) {
            return Some(FontId(index));
        }
        let system = self.fonts.get();
        let font = system.get_font(id, fontdb::Weight::NORMAL)?;
        self.loaded
            .push(loaded_font(system, font, CosmicFeatures::new())?);
        Some(FontId(self.loaded.len() - 1))
    }

    fn shape_segment(
        &mut self,
        text: &str,
        range: Range<usize>,
        font_size: Pixels,
        font_runs: &[FontRun],
        layout: &mut LineLayout,
    ) {
        if range.is_empty() {
            return;
        }
        let segment = &text[range.clone()];
        let mut attrs = AttrsList::new(&Attrs::new());
        let mut offset = 0;
        for run in font_runs {
            let (start, end) = (offset.max(range.start), (offset + run.len).min(range.end));
            offset += run.len;
            if start >= end {
                continue;
            }
            let font = &self.loaded[run.font_id.0];
            let size = f32::from(run.font_size);
            attrs.add_span(
                start - range.start..end - range.start,
                &Attrs::new()
                    .metadata(run.font_id.0)
                    .family(Family::Name(&font.family))
                    .stretch(font.stretch)
                    .style(font.style)
                    .weight(font.weight)
                    .font_features(font.features.clone())
                    .metrics(Metrics::new(size, size)),
            );
        }
        let system = self.fonts.get();
        let line = ShapeLine::new(system, segment, &attrs, cosmic_text::Shaping::Advanced, 4);
        let mut lines = Vec::with_capacity(1);
        line.layout_to_buffer(
            &mut self.scratch,
            f32::from(font_size),
            None,
            cosmic_text::Wrap::None,
            Ellipsize::None,
            None,
            &mut lines,
            None,
            cosmic_text::Hinting::Disabled,
        );
        let Some(line) = lines.first() else {
            return;
        };
        let x = layout.width;
        for glyph in &line.glyphs {
            let mut font_id = FontId(glyph.metadata);
            if self
                .loaded
                .get(font_id.0)
                .is_none_or(|font| font.font.id() != glyph.font_id)
            {
                match self.font_id_for_cosmic_id(glyph.font_id) {
                    Some(id) => font_id = id,
                    None => continue,
                }
            }
            let emoji = self.loaded[font_id.0].emoji;
            // WARNING: variation selectors shape to glyph 3 in emoji fonts, which can't be drawn.
            if glyph.glyph_id == 3 && emoji {
                continue;
            }
            let shaped = ShapedGlyph {
                id: GlyphId(glyph.glyph_id as u32),
                position: point(px(glyph.x) + x, px(glyph.y)),
                index: glyph.start + range.start,
                is_emoji: emoji,
            };
            let font_size = px(glyph.font_size);
            match layout.runs.last_mut() {
                Some(run) if run.font_id == font_id && run.font_size == font_size => {
                    run.glyphs.push(shaped)
                }
                _ => layout.runs.push(ShapedRun {
                    font_id,
                    font_size,
                    glyphs: vec![shaped],
                }),
            }
        }
        layout.width += px(line.w);
        layout.ascent = layout.ascent.max(px(line.max_ascent));
        layout.descent = layout.descent.max(px(line.max_descent));
    }
}

fn loaded_font(
    system: &FontSystem,
    font: Arc<cosmic_text::Font>,
    features: CosmicFeatures,
) -> Option<LoadedFont> {
    let face = system.db().face(font.id())?;
    Some(LoadedFont {
        emoji: face.post_script_name == "NotoColorEmoji",
        family: face.families.first()?.0.clone(),
        stretch: face.stretch,
        style: face.style,
        weight: face.weight,
        features,
        font,
    })
}
