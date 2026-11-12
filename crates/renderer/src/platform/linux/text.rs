use crate::{
    Color, Pixels, Rect, Size, point, px, size,
    text::{
        FaceMetrics, Features, Font, FontId, FontStyle, Glyph, GlyphId, GlyphRaster, Line, Span,
        best,
    },
};
use anyhow::{Context as _, Result, bail};
use cosmic_text::{
    Attrs, AttrsList, Ellipsize, Family, FontFeatures, FontSystem, Hinting, LayoutLine, Metrics,
    ShapeBuffer, ShapeLine, Shaping, Wrap, fontdb,
};
use rustc_hash::FxHashMap;
use std::{cell::RefCell, sync::Arc, thread::JoinHandle};
use swash::{
    scale::{Render, ScaleContext, Source, StrikeWith, image::Content},
    tag_from_bytes,
    zeno::{Format, Vector},
};

pub(crate) struct TextSystem(RefCell<Shelf>);

// Scanning the installed fonts can take hundreds of milliseconds, so a thread does it while the app starts.
enum Library {
    Scanning(Option<JoinHandle<(String, fontdb::Database)>>),
    Ready(Box<FontSystem>),
}

impl Library {
    fn get(&mut self) -> &mut FontSystem {
        if let Self::Scanning(scan) = self {
            let (locale, db) = scan
                .take()
                .and_then(|scan| scan.join().ok())
                .unwrap_or_else(|| FontSystem::new().into_locale_and_db());
            *self = Self::Ready(Box::new(FontSystem::new_with_locale_and_db(locale, db)));
        }
        match self {
            Self::Ready(system) => system,
            Self::Scanning(_) => unreachable!(),
        }
    }
}

struct Face {
    font: Arc<cosmic_text::Font>,
    family: String,
    style: fontdb::Style,
    stretch: fontdb::Stretch,
    weight: fontdb::Weight,
    features: FontFeatures,
    coords: Vec<i16>,
    wght: Option<f32>,
    color: bool,
}

struct Shelf {
    library: Library,
    faces: Vec<Face>,
    known: FxHashMap<(fontdb::ID, u16, Features), FontId>,
    added: Vec<fontdb::ID>,
    scaler: ScaleContext,
    scratch: ShapeBuffer,
    lines: Vec<LayoutLine>,
    // The raster that `raster_bounds` made, which `rasterize` usually asks for next.
    last: Option<(GlyphRaster, Option<swash::scale::image::Image>)>,
}

impl Default for TextSystem {
    fn default() -> Self {
        let scan = std::thread::Builder::new()
            .name("apps-fonts".into())
            .spawn(|| FontSystem::new().into_locale_and_db())
            .ok();
        Self(RefCell::new(Shelf {
            library: Library::Scanning(scan),
            faces: Vec::new(),
            known: FxHashMap::default(),
            added: Vec::new(),
            scaler: ScaleContext::new(),
            scratch: ShapeBuffer::default(),
            lines: Vec::new(),
            last: None,
        }))
    }
}

fn read_i16(table: &[u8], at: usize) -> Option<f32> {
    Some(f32::from(i16::from_be_bytes(
        table.get(at..at + 2)?.try_into().ok()?,
    )))
}

impl Shelf {
    fn face(&mut self, id: fontdb::ID, weight: f32, features: &Features) -> Option<FontId> {
        let key = (id, weight as u16, features.clone());
        if let Some(&known) = self.known.get(&key) {
            return Some(known);
        }
        let system = self.library.get();
        let info = system.db().face(id)?;
        let (family, style, stretch, fixed) = (
            info.families.first()?.0.clone(),
            info.style,
            info.stretch,
            info.weight,
        );
        let font = system.get_font(id, fontdb::Weight(weight as u16))?;
        let swash = font.as_swash();
        let wght = swash.variations().find_by_tag(tag_from_bytes(b"wght"));
        let wght = wght.map(|axis| weight.clamp(axis.min_value(), axis.max_value()));
        let coords = match wght {
            Some(value) => swash
                .variations()
                .normalized_coords([("wght", value)])
                .collect(),
            None => Vec::new(),
        };
        let color = swash.table(tag_from_bytes(b"COLR")).is_some()
            || swash.color_strikes().next().is_some();
        let mut settings = FontFeatures::new();
        for (tag, value) in features.iter() {
            if let Ok(tag) = <[u8; 4]>::try_from(tag.as_bytes()) {
                settings.set(cosmic_text::FeatureTag::new(&tag), *value);
            }
        }
        let face = FontId::try_from(self.faces.len()).ok()?;
        self.faces.push(Face {
            weight: fontdb::Weight(wght.map_or(fixed.0, |value| value as u16)),
            font,
            family,
            style,
            stretch,
            features: settings,
            coords,
            wght,
            color,
        });
        self.known.insert(key, face);
        Some(face)
    }

    fn render(&mut self, glyph: &GlyphRaster) -> Option<swash::scale::image::Image> {
        let face = &self.faces[glyph.font as usize];
        let swash = face.font.as_swash();
        let mut builder = self
            .scaler
            .builder(swash)
            .size(glyph.size.0 * glyph.scale)
            .hint(true);
        if let Some(value) = face.wght {
            builder = builder.variations([("wght", value)]);
        }
        let mut scaler = builder.build();
        let sources: &[Source] = match glyph.emoji {
            true => &[
                Source::ColorOutline(0),
                Source::ColorBitmap(StrikeWith::BestFit),
                Source::Outline,
            ],
            false => &[Source::Bitmap(StrikeWith::ExactSize), Source::Outline],
        };
        let shift = f32::from(glyph.subpixel) / f32::from(crate::SUBPIXEL_STEPS);
        Render::new(sources)
            .format(Format::Alpha)
            .offset(Vector::new(shift, 0.))
            .render(&mut scaler, glyph.glyph)
    }

    // Appends after the glyphs already in `line`.
    fn paragraph(&mut self, text: &str, start: usize, end: usize, spans: &[Span], line: &mut Line) {
        if start == end {
            return;
        }
        let mut attrs = AttrsList::new(&Attrs::new());
        let mut at = 0;
        for span in spans {
            let (from, to) = (at.max(start), (at + span.len).min(end));
            at += span.len;
            if from >= to {
                continue;
            }
            let face = &self.faces[span.font as usize];
            let size = span.size.0;
            attrs.add_span(
                from - start..to - start,
                &Attrs::new()
                    .metadata(span.font as usize)
                    .family(Family::Name(&face.family))
                    .style(face.style)
                    .stretch(face.stretch)
                    .weight(face.weight)
                    .font_features(face.features.clone())
                    .metrics(Metrics::new(size, size)),
            );
        }
        let piece = &text[start..end];
        let shaped = ShapeLine::new(self.library.get(), piece, &attrs, Shaping::Advanced, 4);
        let mut lines = std::mem::take(&mut self.lines);
        lines.clear();
        shaped.layout_to_buffer(
            &mut self.scratch,
            line.font_size.0,
            None,
            Wrap::None,
            Ellipsize::None,
            None,
            &mut lines,
            None,
            Hinting::Disabled,
        );
        if let Some(laid) = lines.first() {
            let pen = line.width;
            for glyph in &laid.glyphs {
                // A variation selector that no font joined to its base can't be drawn.
                if piece[glyph.start..glyph.end]
                    .chars()
                    .all(|c| matches!(c, '\u{fe00}'..='\u{fe0f}' | '\u{e0100}'..='\u{e01ef}'))
                {
                    continue;
                }
                let own = FontId::try_from(glyph.metadata).ok().filter(|&id| {
                    self.faces
                        .get(id as usize)
                        .is_some_and(|face| face.font.id() == glyph.font_id)
                });
                let Some(font) = own.or_else(|| {
                    self.face(
                        glyph.font_id,
                        f32::from(glyph.font_weight.0),
                        &Features::default(),
                    )
                }) else {
                    continue;
                };
                line.glyphs.push(Glyph {
                    x: pen + px(glyph.x + glyph.font_size * glyph.x_offset),
                    y: px(glyph.y - glyph.font_size * glyph.y_offset),
                    size: px(glyph.font_size),
                    font,
                    index: (glyph.start + start) as u32,
                    id: glyph.glyph_id,
                    emoji: self.faces[font as usize].color,
                });
            }
            line.width += px(laid.w);
        }
        self.lines = lines;
    }
}

impl TextSystem {
    pub fn add_font(&self, bytes: Vec<u8>) -> Result<()> {
        let shelf = &mut *self.0.borrow_mut();
        let db = shelf.library.get().db_mut();
        let ids = db.load_font_source(fontdb::Source::Binary(Arc::new(bytes)));
        let (&first, rest) = ids
            .split_first()
            .context("Unsupported or invalid font data")?;
        for &other in rest {
            db.remove_face(other);
        }
        shelf.added.push(first);
        Ok(())
    }

    pub fn families(&self) -> Vec<String> {
        let mut shelf = self.0.borrow_mut();
        shelf
            .library
            .get()
            .db()
            .faces()
            .filter_map(|face| Some(face.families.first()?.0.clone()))
            .collect()
    }

    pub fn font(&self, font: &Font) -> Option<FontId> {
        let shelf = &mut *self.0.borrow_mut();
        let db = shelf.library.get().db();
        let family = match &*font.family {
            ".SystemUIFont" => db.family_name(&Family::SansSerif).to_owned(),
            name => name.to_owned(),
        };
        let mut faces: Vec<_> = db
            .faces()
            .filter(|face| {
                face.families
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case(&family))
            })
            .map(|face| face.id)
            .collect();
        // A family that the app added hides the installed one of the same name.
        if faces.iter().any(|id| shelf.added.contains(id)) {
            faces.retain(|id| shelf.added.contains(id));
        }
        let traits: Vec<_> = faces
            .iter()
            .filter_map(|&id| {
                let face = db.face(id)?;
                let range = db.with_face_data(id, |data, index| {
                    let axis = swash::FontRef::from_index(data, index as usize)?
                        .variations()
                        .find_by_tag(tag_from_bytes(b"wght"))?;
                    Some((axis.min_value(), axis.max_value()))
                });
                let weight = match range.flatten() {
                    Some((low, high)) => font.weight.clamp(low, high),
                    None => f32::from(face.weight.0),
                };
                Some((
                    weight,
                    match face.style {
                        fontdb::Style::Normal => FontStyle::Normal,
                        fontdb::Style::Italic => FontStyle::Italic,
                        fontdb::Style::Oblique => FontStyle::Oblique,
                    },
                    f32::from(face.stretch.to_number()) / 5.,
                ))
            })
            .collect();
        let at = best(&traits, font.weight, font.style)?;
        shelf.face(faces[at], traits[at].0, &font.features)
    }

    pub fn metrics(&self, font: FontId) -> FaceMetrics {
        let shelf = self.0.borrow();
        let face = &shelf.faces[font as usize];
        let swash = face.font.as_swash();
        let metrics = swash.metrics(&face.coords);
        let em = f32::from(metrics.units_per_em.max(1));
        let head = swash.table(tag_from_bytes(b"head"));
        let corner = |at| head.and_then(|head| read_i16(head, at)).unwrap_or(0.) / em;
        let [left, bottom, right, top] = [36, 38, 40, 42].map(corner);
        let typo = swash
            .table(tag_from_bytes(b"OS/2"))
            .and_then(|table| Some((read_i16(table, 68)?, read_i16(table, 70)?)))
            .filter(|(ascent, descent)| ascent - descent > 0.);
        FaceMetrics {
            ascent: metrics.ascent / em,
            descent: -metrics.descent / em,
            line_gap: metrics.leading / em,
            cap_height: metrics.cap_height / em,
            x_height: metrics.x_height / em,
            bounds: Rect::new(point(left, bottom), size(right - left, top - bottom)),
            em_ascent: match typo {
                Some((ascent, descent)) => ascent / (ascent - descent),
                None => metrics.ascent / (metrics.ascent + metrics.descent).max(1.),
            },
            tall: false,
        }
    }

    pub fn glyph(&self, font: FontId, ch: char) -> Option<GlyphId> {
        let shelf = self.0.borrow();
        let glyph = shelf.faces[font as usize].font.as_swash().charmap().map(ch);
        (glyph != 0).then_some(glyph)
    }

    pub fn advance(&self, font: FontId, glyph: GlyphId) -> f32 {
        let shelf = self.0.borrow();
        let face = &shelf.faces[font as usize];
        let swash = face.font.as_swash();
        let em = f32::from(swash.metrics(&face.coords).units_per_em.max(1));
        swash.glyph_metrics(&face.coords).advance_width(glyph) / em
    }

    pub fn ink(&self, font: FontId, glyph: GlyphId, size: f32) -> Option<[f32; 4]> {
        let shelf = &mut *self.0.borrow_mut();
        let face = &shelf.faces[font as usize];
        let mut builder = shelf.scaler.builder(face.font.as_swash()).size(size);
        if let Some(value) = face.wght {
            builder = builder.variations([("wght", value)]);
        }
        let outline = builder.build().scale_outline(glyph)?;
        let bounds = outline.bounds();
        Some([bounds.min.x, bounds.max.x, bounds.max.y, bounds.min.y])
    }

    pub fn shape(&self, text: &str, font_size: Pixels, spans: &[Span]) -> Line {
        let shelf = &mut *self.0.borrow_mut();
        let mut line = Line {
            len: text.len(),
            font_size,
            ..Default::default()
        };
        // cosmic-text panics on paragraphs of different directions in one line, so each is shaped alone.
        let mut start = 0;
        for (at, ch) in text.char_indices() {
            if matches!(
                ch,
                '\n' | '\r' | '\u{1c}'..='\u{1e}' | '\u{85}' | '\u{2029}'
            ) {
                shelf.paragraph(text, start, at, spans, &mut line);
                start = at + ch.len_utf8();
                shelf.paragraph(text, at, start, spans, &mut line);
            }
        }
        shelf.paragraph(text, start, text.len(), spans, &mut line);
        line
    }

    pub fn raster_bounds(&self, glyph: &GlyphRaster) -> Result<Rect<i32>> {
        let shelf = &mut *self.0.borrow_mut();
        let image = shelf.render(glyph);
        let bounds = image
            .as_ref()
            .map(|image| image.placement)
            .filter(|place| place.width > 0 && place.height > 0)
            .map_or_else(Rect::default, |place| {
                Rect::new(
                    point(place.left, -place.top),
                    size(place.width as i32, place.height as i32),
                )
                .outset(1)
            });
        shelf.last = Some((*glyph, image));
        Ok(bounds)
    }

    pub fn rasterize(
        &self,
        glyph: &GlyphRaster,
        bounds: Rect<i32>,
    ) -> Result<(Size<i32>, Vec<u8>)> {
        if bounds.is_empty() {
            bail!("glyph bounds are empty");
        }
        let shelf = &mut *self.0.borrow_mut();
        let image = match shelf.last.take() {
            Some((kept, image)) if kept == *glyph => image,
            _ => shelf.render(glyph),
        }
        .context("the glyph has no outline or bitmap")?;
        let mut canvas = bounds.size;
        canvas.width += i32::from(glyph.subpixel > 0);
        let channels = if glyph.emoji { 4 } else { 1 };
        let (width, height) = (canvas.width as usize, canvas.height as usize);
        let mut pixels = vec![0u8; width * height * channels];
        let place = image.placement;
        let (dx, dy) = (place.left - bounds.origin.x, -place.top - bounds.origin.y);
        let color = image.content == Content::Color;
        let unpremultiply = matches!(image.source, Source::ColorOutline(_));
        for row in 0..place.height as i32 {
            let y = row + dy;
            if !(0..height as i32).contains(&y) {
                continue;
            }
            for column in 0..place.width as i32 {
                let x = column + dx;
                if !(0..width as i32).contains(&x) {
                    continue;
                }
                let from = (row * place.width as i32 + column) as usize;
                let to = (y as usize * width + x as usize) * channels;
                match (glyph.emoji, color) {
                    (false, false) => pixels[to] = image.data[from],
                    (false, true) => pixels[to] = image.data[from * 4 + 3],
                    (true, false) => pixels[to + 3] = image.data[from],
                    (true, true) => {
                        let [r, g, b, a] = image.data[from * 4..from * 4 + 4] else {
                            continue;
                        };
                        let undo = |channel: u8| match unpremultiply && a > 0 {
                            true => (u32::from(channel) * 255 / u32::from(a)).min(255) as u8,
                            false => channel,
                        };
                        // Color pages hold BGRA.
                        pixels[to..to + 4].copy_from_slice(&[undo(b), undo(g), undo(r), a]);
                    }
                }
            }
        }
        Ok((canvas, pixels))
    }

    pub fn warm(_: Vec<Vec<String>>) {}

    pub fn dilation(&self, _: Color) -> u8 {
        0
    }
}
