mod open_type;
mod raster;
mod shape;

use raster::{SKEW, embolden};
use shape::Words;

use crate::{
  Color, Pixels, Rect, Size, point, px, size,
  text::{
    FaceMetrics, Features, Font, FontId, FontStyle, Glyph, GlyphId, GlyphRaster, Line,
    SUBPIXEL_STEPS, SharedString, Span, best,
    intercept::{Segment, intercept},
  },
};
use anyhow::{Context as _, Result, bail};
use objc2_core_foundation::{
  CFArray, CFData, CFDictionary, CFIndex, CFLocale, CFMutableAttributedString, CFNumber, CFRange,
  CFRetained, CFString, CFType, CGAffineTransform, CGFloat, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{
  CGBitmapContextCreate, CGColorSpace, CGContext, CGPathDrawingMode, CGPathElement,
  CGPathElementType, CGTextDrawingMode,
};
use objc2_core_text::{
  CTFont, CTFontCollection, CTFontDescriptor, CTFontManagerCopyAvailableFontFamilyNames,
  CTFontManagerCreateFontDescriptorsFromData, CTFontOrientation, CTFontSymbolicTraits,
  CTFontTableOptions, CTLine, CTRun, kCTFontAttributeName, kCTFontCascadeListAttribute,
  kCTFontFamilyNameAttribute, kCTFontSlantTrait, kCTFontSymbolicTrait, kCTFontWeightTrait,
  kCTFontWidthTrait,
};
use rustc_hash::FxHashMap;
use std::{
  cell::{OnceCell, RefCell},
  ffi::c_void,
  hash::Hash,
  mem,
  ops::Range,
  ptr::{self, NonNull},
  rc::Rc,
  sync::{Arc, LazyLock, Mutex, OnceLock, PoisonError},
};

#[derive(Clone)]
struct Shared(CFRetained<CTFont>);

// SAFETY: CoreText fonts are immutable, and Apple documents them as safe to use from any thread at once.
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

/// Each value is made outside the map lock, so a slow load does not block other lookups.
struct Process<K, V>(LazyLock<Mutex<FxHashMap<K, Arc<OnceLock<V>>>>>);

impl<K: Hash + Eq, V: Clone> Process<K, V> {
  const fn new() -> Self {
    Self(LazyLock::new(Default::default))
  }

  fn get(&self, key: K, make: impl FnOnce() -> V) -> V {
    let slot = (self.0.lock().unwrap_or_else(PoisonError::into_inner))
      .entry(key)
      .or_default()
      .clone();
    slot.get_or_init(make).clone()
  }
}

type Faces = Option<Vec<(Shared, String)>>;

static SYSTEM_FACES: Process<(String, Vec<(String, u32)>), Faces> = Process::new();
/// Keyed by font pointer. The faces stay alive in `SYSTEM_FACES`.
static AXES: Process<usize, Option<f32>> = Process::new();
/// Axis tags and value bits.
type Coords = Vec<(u32, u32)>;
/// Tags with minimum, default and maximum.
type Axes = Rc<[(u32, [f32; 3])]>;

static INSTANCES: Process<(usize, Coords), (Shared, String)> = Process::new();

static FAMILIES: LazyLock<Vec<String>> = LazyLock::new(|| {
  // SAFETY: CoreText returns an array of strings.
  let names = unsafe { CTFontManagerCopyAvailableFontFamilyNames() };
  let names = unsafe { names.cast_unchecked::<CFString>() };
  // macOS leaves these out of the list, though it has them and browsers find them.
  names
    .iter()
    .map(|name| name.to_string())
    .chain(["Times".into(), "Courier".into()])
    .collect()
});

pub(crate) struct TextSystem(RefCell<Book>);

struct Face {
  font: CFRetained<CTFont>,
  local: bool,
  emoji: bool,
  mono: bool,
  /// Like Chrome, a face without the asked bold or slant draws its glyphs emboldened or skewed.
  bold: bool,
  oblique: bool,
  units: OnceCell<Units>,
  matching: OnceCell<(f32, FontStyle, f32)>,
  axes: OnceCell<Axes>,
}

/// In font units, computed as font-kit did, so layout keeps its numbers.
#[derive(Clone, Copy)]
struct Units {
  per_em: u32,
  /// Font units per point at the face's size.
  per_point: f32,
  ascent: f32,
  descent: f32,
  gap: f32,
  cap: f32,
  x: f32,
  /// `[min x, min y, max x, max y]`.
  bounds: [f32; 4],
}

impl Face {
  fn new(font: CFRetained<CTFont>, name: &str, local: bool) -> Self {
    // SAFETY: plain CoreText queries.
    let traits = unsafe { font.symbolic_traits() };
    Self {
      emoji: name == "AppleColorEmoji" || name == ".AppleColorEmojiUI",
      mono: traits.contains(CTFontSymbolicTraits::TraitMonoSpace),
      bold: false,
      oblique: false,
      font,
      local,
      units: OnceCell::new(),
      matching: OnceCell::new(),
      axes: OnceCell::new(),
    }
  }

  fn axes(&self) -> Axes {
    self
      .axes
      .get_or_init(|| open_type::axes(&self.font).into())
      .clone()
  }

  fn units(&self) -> Units {
    *self.units.get_or_init(|| {
      let font = &self.font;
      // SAFETY: plain CoreText queries.
      unsafe {
        let per_em = font.units_per_em();
        let per_point = f64::from(per_em) / font.size();
        let scaled = |value: CGFloat| (value * per_point) as f32;
        let rect = font.bounding_box();
        let (x, y) = (rect.origin.x as f32, rect.origin.y as f32);
        let bounds = [
          x,
          y,
          x + rect.size.width as f32,
          y + rect.size.height as f32,
        ]
        .map(|v| v * per_point as f32);
        Units {
          per_em,
          per_point: per_point as f32,
          ascent: scaled(font.ascent()),
          descent: scaled(-font.descent()),
          gap: scaled(font.leading()),
          cap: scaled(font.cap_height()),
          x: scaled(font.x_height()),
          bounds,
        }
      }
    })
  }

  fn metrics(&self) -> FaceMetrics {
    let units = self.units();
    let per_em = units.per_em as f32;
    let [x0, y0, x1, y1] = units.bounds;
    let typo = open_type::typo_metrics(&self.font);
    // SAFETY: a plain CoreText query.
    let family = unsafe { self.font.family_name() }.to_string();
    FaceMetrics {
      ascent: units.ascent / per_em,
      descent: units.descent / per_em,
      line_gap: units.gap / per_em,
      cap_height: units.cap / per_em,
      x_height: units.x / per_em,
      bounds: Rect::new(point(x0, y0), size(x1 - x0, y1 - y0)) / per_em,
      em_ascent: match typo {
        Some([ascent, descent]) => ascent / (ascent - descent),
        None => units.ascent / (units.ascent - units.descent),
      },
      tall: matches!(family.as_str(), "Times" | "Helvetica" | "Courier"),
    }
  }

  fn glyph(&self, ch: char) -> Option<GlyphId> {
    let (mut units, mut glyphs) = ([0u16; 2], [0u16; 2]);
    ch.encode_utf16(&mut units);
    // SAFETY: both buffers hold two values.
    unsafe {
      self.font.glyphs_for_characters(
        NonNull::from(&mut units).cast(),
        NonNull::from(&mut glyphs).cast(),
        2,
      )
    };
    Some(glyphs[0]).filter(|&glyph| glyph != 0)
  }

  fn advance(&self, glyph: GlyphId) -> f32 {
    let (mut id, mut advance) = (glyph, CGSize::default());
    // SAFETY: one glyph in, one advance out.
    unsafe {
      self.font.advances_for_glyphs(
        CTFontOrientation::Default,
        NonNull::from(&mut id),
        &mut advance,
        1,
      )
    };
    let units = self.units();
    advance.width as f32 * units.per_point / units.per_em as f32
  }

  /// `[min x, min y, max x, max y]` in font units.
  fn outline(&self, glyph: GlyphId) -> [f32; 4] {
    let mut id = glyph;
    // SAFETY: one glyph in, the overall rect out.
    let rect = unsafe {
      self.font.bounding_rects_for_glyphs(
        CTFontOrientation::Default,
        NonNull::from(&mut id),
        ptr::null_mut(),
        1,
      )
    };
    let (x, y) = (rect.origin.x as f32, rect.origin.y as f32);
    let per_point = self.units().per_point;
    [
      x,
      y,
      x + rect.size.width as f32,
      y + rect.size.height as f32,
    ]
    .map(|value| value * per_point)
  }

  /// `[min x, min y, max x, max y]` of the drawn glyph at `size`, with the synthetic bold and slant.
  fn drawn(&self, glyph: GlyphId, size: f32) -> [f32; 4] {
    let k = size / self.units().per_em as f32;
    let [x0, y0, x1, y1] = self.outline(glyph).map(|value| value * k);
    let (x0, x1) = if self.oblique {
      (x0 + SKEW * y0.min(0.), x1 + SKEW * y1.max(0.))
    } else {
      (x0, x1)
    };
    let out = if self.bold {
      embolden(size, 1.) / 2.
    } else {
      0.
    };
    [x0 - out, y0 - out, x1 + out, y1 + out]
  }

  fn matching(&self) -> (f32, FontStyle, f32) {
    *self.matching.get_or_init(|| {
      // SAFETY: plain CoreText queries.
      let (traits, symbolic) = unsafe { (self.font.traits(), self.font.symbolic_traits()) };
      let traits = unsafe { traits.cast_unchecked::<CFString, CFType>() };
      let number = |key: &CFString| traits.get(key)?.downcast::<CFNumber>().ok();
      // SAFETY: the keys are CoreText constants.
      let [weight, width, slant] =
        unsafe { [kCTFontWeightTrait, kCTFontWidthTrait, kCTFontSlantTrait] }
          .map(|key| number(key).and_then(|value| value.as_f64()).unwrap_or(0.));
      let style = if symbolic.contains(CTFontSymbolicTraits::TraitItalic) {
        FontStyle::Italic
      } else if slant > 0. {
        FontStyle::Oblique
      } else {
        FontStyle::Normal
      };
      let class = self
        .local
        .then(|| open_type::weight_class(&self.font))
        .flatten();
      // SAFETY: the key is a CoreText constant.
      let trait_weight = number(unsafe { kCTFontWeightTrait }).and_then(|value| value.as_f32());
      let weight = class
        .or_else(|| trait_weight.map(css_weight))
        .unwrap_or_else(|| find_index(weight as f32, &KIT_WEIGHTS) * 100. + 100.);
      (weight, style, css_stretch(width as f32))
    })
  }
}

/// CoreText weights of the CSS weights 100 to 900 for Apple's faces. A plain linear scale maps Medium to 530,
/// so CSS 500 would pick Regular.
const APPLE_WEIGHTS: [f32; 9] = [-0.8, -0.6, -0.4, 0., 0.23, 0.3, 0.4, 0.56, 0.62];
/// The scale font-kit used when a face has no numeric weight trait.
const KIT_WEIGHTS: [f32; 9] = [-0.7, -0.5, -0.23, 0., 0.2, 0.3, 0.4, 0.6, 0.8];
/// CSS stretch at CoreText widths -1 to 1 in quarters.
const STRETCHES: [f32; 9] = [0.5, 0.625, 0.75, 0.875, 1., 1.125, 1.25, 1.5, 2.];

fn css_weight(weight: f32) -> f32 {
  let below = APPLE_WEIGHTS
    .iter()
    .rposition(|&knot| weight >= knot)
    .unwrap_or(0);
  let fraction = match APPLE_WEIGHTS.get(below + 1) {
    Some(next) if weight >= APPLE_WEIGHTS[0] => {
      (weight - APPLE_WEIGHTS[below]) / (next - APPLE_WEIGHTS[below])
    }
    _ => 0.,
  };
  100. * (below as f32 + 1. + fraction)
}

fn css_stretch(width: f32) -> f32 {
  let at = (width + 1.) * 4.;
  let (low, high) = (
    STRETCHES[at.floor() as usize],
    STRETCHES[at.ceil() as usize],
  );
  low + (high - low) * at.fract()
}

fn find_index(value: f32, knots: &[f32]) -> f32 {
  let above = match knots
    .binary_search_by(|knot| knot.partial_cmp(&value).unwrap_or(std::cmp::Ordering::Less))
  {
    Ok(at) => return at as f32,
    Err(above) => above,
  };
  if above == 0 || above >= knots.len() {
    return above as f32;
  }
  let (low, high) = (knots[above - 1], knots[above]);
  (above - 1) as f32 + (value - low) / (high - low)
}

struct Added {
  family: String,
  descriptor: CFRetained<CTFontDescriptor>,
}

#[derive(Default)]
struct Book {
  faces: Vec<Face>,
  /// PostScript names, plus axis values for instances.
  names: FxHashMap<String, FontId>,
  families: FxHashMap<(SharedString, Features), Rc<[FontId]>>,
  /// Sorted by family, stable.
  added: Vec<Added>,
  // Each sized copy rebuilds CoreText caches at a cost, so the copies stay.
  sized: FxHashMap<(FontId, u32), CFRetained<CTFont>>,
  /// Base face and axis values, to the instance face.
  instances: FxHashMap<(FontId, Coords), FontId>,
  // Keyed by pointer; the kept font keeps the address from being reused.
  natives: FxHashMap<usize, (CFRetained<CTFont>, FontId)>,
  words: Words,
  scratch: Scratch,
  /// The last language that hyphenated, with its locale.
  locale: Option<(String, Option<CFRetained<CFLocale>>)>,
}

#[derive(Default)]
struct Scratch {
  ids: Vec<u16>,
  points: Vec<CGPoint>,
  indices: Vec<CFIndex>,
}

impl Default for TextSystem {
  fn default() -> Self {
    // Listing the font families takes about 20 ms, so it starts here, off the first frame's path.
    static LISTING: std::sync::Once = std::sync::Once::new();
    LISTING.call_once(|| {
      let _ = std::thread::Builder::new()
        .name("font families".into())
        .spawn(|| LazyLock::force(&FAMILIES).len());
    });
    Self(RefCell::default())
  }
}

impl TextSystem {
  /// Adds a face to `family`, as CSS groups faces by the family of their `@font-face`. Returns the family.
  pub fn add_font(&self, bytes: Vec<u8>, family: &str) -> Result<String> {
    self.0.borrow_mut().add(bytes, family)
  }

  pub fn families(&self) -> Vec<String> {
    self.0.borrow().families()
  }

  pub fn font(&self, font: &Font) -> Option<FontId> {
    self.0.borrow_mut().font(font)
  }

  pub fn metrics(&self, font: FontId) -> FaceMetrics {
    self.0.borrow().face(font).metrics()
  }

  /// The `GSUB` table, and the tags of other small capitals that CoreText knows.
  pub fn substitutions(&self, font: FontId) -> (Vec<u8>, Vec<[u8; 4]>) {
    let tag = u32::from_be_bytes(*b"GSUB");
    let book = self.0.borrow();
    let font = &book.face(font).font;
    // SAFETY: the table is copied out of the font.
    let table = unsafe { font.table(tag, CTFontTableOptions::NoOptions) };
    let table = table.map_or_else(Vec::new, |table| table.to_vec());
    (table, open_type::small_caps(font))
  }

  pub fn glyph(&self, font: FontId, ch: char) -> Option<GlyphId> {
    self.0.borrow().face(font).glyph(ch)
  }

  pub fn advance(&self, font: FontId, glyph: GlyphId) -> f32 {
    self.0.borrow().face(font).advance(glyph)
  }

  pub fn ink(&self, font: FontId, glyph: GlyphId, size: f32) -> Option<[f32; 4]> {
    let book = self.0.borrow();
    let [x0, y0, x1, y1] = book.face(font).drawn(glyph, size);
    Some([x0, x1, y1, y0])
  }

  pub fn intercept(
    &self,
    font: FontId,
    glyph: GlyphId,
    size: f32,
    top: f32,
    bottom: f32,
  ) -> Option<[f32; 2]> {
    let mut book = self.0.borrow_mut();
    let sized = book.sized(font, size);
    let face = book.face(font);
    let skew = if face.oblique { SKEW } else { 0. };
    let matrix = CGAffineTransform {
      a: 1.,
      b: 0.,
      c: skew.into(),
      d: 1.,
      tx: 0.,
      ty: 0.,
    };
    // SAFETY: a path of one glyph of a live font.
    let path = unsafe { sized.path_for_glyph(glyph, &matrix) }?;
    let out = if face.bold {
      embolden(size, 1.) / 2.
    } else {
      0.
    };
    struct Walk {
      segments: Vec<Segment>,
      start: [f32; 2],
      at: [f32; 2],
    }
    extern "C-unwind" fn add(info: *mut c_void, element: NonNull<CGPathElement>) {
      // SAFETY: CoreGraphics passes `info` back, and each element with as many points as its type has.
      let (walk, element) = unsafe { (&mut *info.cast::<Walk>(), element.as_ref()) };
      let point = |at: usize| {
        let point = unsafe { *element.points.as_ptr().add(at) };
        [point.x as f32, -point.y as f32]
      };
      let from = walk.at;
      let kind = element.r#type;
      let (segment, to) = match kind {
        CGPathElementType::MoveToPoint => {
          (walk.start, walk.at) = (point(0), point(0));
          return;
        }
        CGPathElementType::AddLineToPoint => (Segment::Line([from, point(0)]), point(0)),
        CGPathElementType::AddQuadCurveToPoint => {
          (Segment::quad([from, point(0), point(1)]), point(1))
        }
        CGPathElementType::AddCurveToPoint => (
          Segment::Cubic([from, point(0), point(1), point(2)]),
          point(2),
        ),
        _ => (Segment::Line([from, walk.start]), walk.start),
      };
      walk.segments.push(segment);
      walk.at = to;
    }
    let mut walk = Walk {
      segments: Vec::new(),
      start: [0.; 2],
      at: [0.; 2],
    };
    // SAFETY: `add` gets the live walk.
    unsafe { objc2_core_graphics::CGPath::apply(Some(&path), (&raw mut walk).cast(), Some(add)) };
    intercept(walk.segments, top - out, bottom + out).map(|[left, right]| [left - out, right + out])
  }

  pub fn shape(&self, text: &str, font_size: Pixels, spans: &[Span]) -> Line {
    self.0.borrow_mut().shape(text, font_size, spans)
  }

  /// The byte offsets where a word of `language` can break.
  pub fn hyphenate(&self, word: &str, language: &str) -> Vec<usize> {
    let mut book = self.0.borrow_mut();
    let locale = match &book.locale {
      Some((known, locale)) if known == language => locale.clone(),
      _ => {
        let locale = CFLocale::new(None, Some(&CFString::from_str(language)))
          .filter(|locale| CFString::is_hyphenation_available_for_locale(Some(locale)));
        book.locale = Some((language.to_owned(), locale.clone()));
        locale
      }
    };
    let Some(locale) = locale else {
      return Vec::new();
    };
    let string = CFString::from_str(word);
    let length = string.length();
    let (mut places, mut end) = (Vec::new(), length);
    while end > 0 {
      // SAFETY: the range is the whole string, and the hyphen character is not asked for.
      end = unsafe {
        string.hyphenation_location_before_index(
          end,
          CFRange::new(0, length),
          0,
          Some(&locale),
          ptr::null_mut(),
        )
      };
      if end > 0 {
        places.push(end);
      }
    }
    let mut utf16 = 0;
    let mut offsets = Vec::new();
    for (count, (at, ch)) in word.char_indices().enumerate() {
      if count > 1 && places.contains(&utf16) && word[at..].chars().nth(1).is_some() {
        offsets.push(at);
      }
      utf16 += ch.len_utf16() as CFIndex;
    }
    offsets
  }

  pub fn warm(lists: Vec<Vec<String>>) {
    let load = move || {
      let mut names = FAMILIES.clone();
      names.sort_unstable();
      names.dedup();
      let names: FxHashMap<String, String> = names
        .into_iter()
        .map(|name| (name.to_lowercase(), name))
        .collect();
      let text = Self::default();
      for list in lists {
        if let Some(name) = list
          .iter()
          .find_map(|family| names.get(&family.to_lowercase()))
        {
          text.font(&Font::new(name.as_str()));
        }
      }
    };
    let _ = std::thread::Builder::new()
      .name("apps-fonts".into())
      .spawn(load);
  }
}

impl Book {
  fn families(&self) -> Vec<String> {
    let mut names = FAMILIES.clone();
    names.extend(self.added.iter().map(|added| added.family.clone()));
    names
  }

  fn add(&mut self, bytes: Vec<u8>, family: &str) -> Result<String> {
    // SAFETY: CoreText parses the data and returns an array of descriptors.
    let descriptor = unsafe {
      let all = CTFontManagerCreateFontDescriptorsFromData(&CFData::from_bytes(&bytes));
      all.cast_unchecked::<CTFontDescriptor>().get(0)
    }
    .context("Unsupported or invalid font data")?;
    self.added.push(Added {
      family: family.to_owned(),
      descriptor,
    });
    self.added.sort_by(|a, b| a.family.cmp(&b.family));
    self.families.clear();
    Ok(family.to_owned())
  }

  fn face(&self, font: FontId) -> &Face {
    &self.faces[font as usize]
  }

  fn register(&mut self, font: CFRetained<CTFont>, name: String, local: bool) -> FontId {
    let id = self.faces.len() as FontId;
    self.faces.push(Face::new(font, &name, local));
    self.names.insert(name, id);
    id
  }

  fn font(&mut self, font: &Font) -> Option<FontId> {
    if let Some((first, rest)) = font.family.split_once('\n') {
      let id = self.font(&Font {
        family: first.into(),
        ..font.clone()
      })?;
      return Some(self.cascade(id, rest, font));
    }
    let id = self.matched(font)?;
    let face = self.face(id);
    let weight = face.matching().0;
    let [own_bold, own_oblique] = font.synthetic;
    let bold = font.synthesis[0]
      && font.weight >= 600.
      && weight < 600.
      && !face.axes().iter().any(|&(tag, _)| tag == open_type::WGHT);
    // Like Chrome, faces that only lean, without the italic trait, slant more.
    // SAFETY: plain CoreText query.
    let italic = unsafe { face.font.symbolic_traits() }.contains(CTFontSymbolicTraits::TraitItalic);
    let oblique = font.synthesis[1] && font.style != FontStyle::Normal && !italic;
    let bold = own_bold.unwrap_or(bold);
    let oblique = own_oblique.unwrap_or(oblique);
    if face.emoji || !bold && !oblique {
      return Some(id);
    }
    let name = format!("{id}#synthetic{}{}", u8::from(bold), u8::from(oblique));
    if let Some(&known) = self.names.get(&name) {
      return Some(known);
    }
    let (copy, local) = (face.font.clone(), face.local);
    let synthetic = self.register(copy, name, local);
    let face = &mut self.faces[synthetic as usize];
    (face.bold, face.oblique) = (bold, oblique);
    Some(synthetic)
  }

  fn matched(&mut self, font: &Font) -> Option<FontId> {
    let key = (font.family.clone(), font.features.clone());
    let candidates = match self.families.get(&key) {
      Some(found) => found.clone(),
      None => {
        let found: Rc<[FontId]> = self.load(&font.family, &font.features)?.into();
        self.families.insert(key, found.clone());
        found
      }
    };
    let matching: Vec<_> = candidates
      .iter()
      .map(|&id| self.face(id).matching())
      .collect();
    let id = candidates[best(&matching, font.weight, font.style, font.stretch)?];
    let face = self.face(id);
    let axes = face.axes();
    if axes.is_empty() {
      return Some(id);
    }
    let default = if face.local {
      open_type::default_weight(&face.font)
    } else {
      let pointer = CFRetained::as_ptr(&face.font).as_ptr() as usize;
      AXES.get(pointer, || open_type::default_weight(&face.font))
    };
    let current = open_type::variation(&face.font);
    let mut wanted = Vec::new();
    let mut same = true;
    for &(tag, [min, initial, max]) in axes.iter() {
      let set = font
        .variations
        .iter()
        .find(|(name, _)| u32::from_be_bytes(*name) == tag)
        .map(|&(_, value)| value);
      let value = set.or(match tag {
        open_type::WGHT if default.is_some() => Some(font.weight),
        // Old Apple fonts like Skia have another scale.
        open_type::WDTH if max >= 50. && (face.local || font.stretch != 1.) => {
          Some(font.stretch * 100.)
        }
        // Like Chromium, web fonts follow the font size. CoreText sizes system fonts itself.
        open_type::OPSZ if face.local => font.optical_size,
        _ => None,
      });
      if let Some(value) = value {
        // Each value makes a font, so a transition steps in values that look the same.
        let value = match tag {
          open_type::WGHT | open_type::WDTH => value.round(),
          _ if max > min => min + ((value - min) * 256. / (max - min)).round() * (max - min) / 256.,
          _ => value,
        }
        .clamp(min, max);
        let now = current
          .iter()
          .find(|(name, _)| *name == tag)
          .map_or(initial, |&(_, value)| value);
        same &= value == now;
        wanted.push((tag, value));
      }
    }
    if wanted.is_empty() || same && !face.local {
      return Some(id);
    }
    Some(self.instance(id, wanted))
  }

  /// A copy of `id` that falls back to the faces of `families`, one per line, before the system fonts.
  fn cascade(&mut self, id: FontId, families: &str, font: &Font) -> FontId {
    let name = format!("{id}#cascade\n{families}");
    if let Some(&known) = self.names.get(&name) {
      return known;
    }
    let descriptors: Vec<_> = families
      .split('\n')
      .filter_map(|family| {
        let id = self.font(&Font {
          family: family.into(),
          synthetic: [None; 2],
          ..font.clone()
        })?;
        // SAFETY: plain CoreText query.
        Some(unsafe { self.face(id).font.font_descriptor() })
      })
      .collect();
    if descriptors.is_empty() {
      return id;
    }
    let list = CFArray::from_retained_objects(&descriptors);
    // SAFETY: a descriptor with a cascade list of descriptors, and a copy of a live font with it.
    let copy = unsafe {
      let attributes = CFDictionary::<CFString, CFType>::from_slices(
        &[kCTFontCascadeListAttribute],
        &[list.as_ref()],
      );
      let descriptor = CTFontDescriptor::with_attributes(attributes.as_opaque());
      self
        .face(id)
        .font
        .copy_with_attributes(0., ptr::null(), Some(&descriptor))
    };
    let (local, bold, oblique) = {
      let face = self.face(id);
      (face.local, face.bold, face.oblique)
    };
    let cascade = self.register(copy, name, local);
    let face = &mut self.faces[cascade as usize];
    (face.bold, face.oblique) = (bold, oblique);
    cascade
  }

  fn load(&mut self, family: &str, features: &[(String, u32)]) -> Option<Vec<FontId>> {
    let family = if family == ".SystemUIFont" {
      ".AppleSystemUIFont"
    } else {
      family
    };
    let start = self
      .added
      .partition_point(|added| added.family.as_str() < family);
    let added: Vec<_> = self.added[start..]
      .iter()
      .take_while(|added| added.family == family)
      .map(|added| {
        // SAFETY: the descriptor came from CoreText.
        unsafe { CTFont::with_font_descriptor(&added.descriptor, 16., ptr::null()) }
      })
      .collect();
    let (faces, local) = if added.is_empty() {
      let faces = SYSTEM_FACES.get((family.to_owned(), features.to_vec()), || {
        Some(
          usable(installed(family)?.into_iter(), features)
            .map(|(font, name)| (Shared(font), name))
            .collect(),
        )
      })?;
      (
        faces
          .into_iter()
          .map(|(font, name)| (font.0, name))
          .collect::<Vec<_>>(),
        false,
      )
    } else {
      (usable(added.into_iter(), features).collect(), true)
    };
    Some(
      faces
        .into_iter()
        .map(|(font, name)| self.register(font, name, local))
        .collect(),
    )
  }

  fn instance(&mut self, id: FontId, axes: Vec<(u32, f32)>) -> FontId {
    let key: Coords = axes
      .iter()
      .map(|&(tag, value)| (tag, value.to_bits()))
      .collect();
    if let Some(&known) = self.instances.get(&(id, key.clone())) {
      return known;
    }
    let face = self.face(id);
    let local = face.local;
    let pointer = CFRetained::as_ptr(&face.font).as_ptr() as usize;
    let make = || {
      let font = open_type::with_variations(&face.font, &axes);
      // The base pointer keeps instances of faces with other features apart.
      let mut name = format!("{}#{pointer:x}", unsafe { font.post_script_name() });
      for (tag, value) in &axes {
        name += &format!("#{tag:x}={value}");
      }
      (Shared(font), name)
    };
    let (font, name) = if local {
      make()
    } else {
      let mut all = INSTANCES.0.lock().unwrap_or_else(PoisonError::into_inner);
      if all.len() >= 4096 {
        all.clear();
      }
      drop(all);
      INSTANCES.get((pointer, key.clone()), make)
    };
    let instance = match self.names.get(&name) {
      Some(&known) => known,
      None => self.register(font.0, name, local),
    };
    self.instances.insert((id, key), instance);
    instance
  }

  fn native(&mut self, font: &CTFont) -> FontId {
    // SAFETY: plain CoreText queries.
    let mut name = unsafe { font.post_script_name() }.to_string();
    for (tag, value) in open_type::variation(font) {
      name += &format!("#native{tag:x}={value}");
    }
    if let Some(&id) = self.names.get(&name) {
      return id;
    }
    // SAFETY: the graphics font comes from a live CoreText font.
    let face = unsafe {
      CTFont::with_graphics_font(&font.graphics_font(ptr::null_mut()), 16., ptr::null(), None)
    };
    self.register(face, name, false)
  }

  fn sized(&mut self, font: FontId, size: f32) -> CFRetained<CTFont> {
    if self.sized.len() >= 512 {
      self.sized.clear();
    }
    let faces = &self.faces;
    self
      .sized
      .entry((font, size.to_bits()))
      // SAFETY: a copy at another size. CoreText keeps the size of the font for a size of 0, so text of size 0
      // draws at a size too small to show.
      .or_insert_with(|| unsafe {
        faces[font as usize]
          .font
          .copy_with_attributes(size.max(1e-4) as CGFloat, ptr::null(), None)
      })
      .clone()
  }
}

fn installed(family: &str) -> Option<Vec<CFRetained<CTFont>>> {
  let name = CFString::from_str(family);
  // SAFETY: a descriptor and collection query with valid attributes.
  unsafe {
    let attributes =
      CFDictionary::<CFString, CFType>::from_slices(&[kCTFontFamilyNameAttribute], &[&name]);
    let query = CTFontDescriptor::with_attributes(attributes.as_opaque());
    let collection = CTFontCollection::with_font_descriptors(
      Some(CFArray::from_objects(&[&*query]).as_opaque()),
      None,
    );
    let found = collection.matching_font_descriptors()?;
    let found = found.cast_unchecked::<CTFontDescriptor>();
    Some(
      found
        .iter()
        .map(|descriptor| CTFont::with_font_descriptor(&descriptor, 16., ptr::null()))
        .collect(),
    )
    .filter(|faces: &Vec<_>| !faces.is_empty())
  }
}

fn usable<'a>(
  faces: impl Iterator<Item = CFRetained<CTFont>> + 'a,
  features: &'a [(String, u32)],
) -> impl Iterator<Item = (CFRetained<CTFont>, String)> + 'a {
  let mut seen = Vec::<String>::new();
  faces.filter_map(move |face| {
    let face = open_type::with_features(&face, features);
    // SAFETY: plain CoreText queries.
    let traits = unsafe { face.traits() };
    let traits = unsafe { traits.cast_unchecked::<CFString, CFType>() };
    // SAFETY: the keys are CoreText constants.
    let keys = unsafe {
      [
        kCTFontSymbolicTrait,
        kCTFontWidthTrait,
        kCTFontWeightTrait,
        kCTFontSlantTrait,
      ]
    };
    if !keys.iter().all(|key| {
      traits
        .get(key)
        .is_some_and(|value| value.downcast::<CFNumber>().is_ok())
    }) {
      return None;
    }
    let name = unsafe { face.post_script_name() }.to_string();
    // Duplicates drop only within one family load, so a second load never ends up without faces.
    if seen.contains(&name) {
      return None;
    }
    seen.push(name.clone());
    Some((face, name))
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  fn name(text: &TextSystem, id: FontId) -> String {
    let book = text.0.borrow();
    book
      .names
      .iter()
      .find(|(_, kept)| **kept == id)
      .unwrap()
      .0
      .clone()
  }

  #[test]
  fn system_fonts_load_once_per_process() {
    let names = |text: &TextSystem, weight: f32| {
      let id = text
        .font(&Font {
          weight,
          ..Font::new(".SystemUIFont")
        })
        .unwrap();
      name(text, id)
    };
    let [first, second] = [TextSystem::default(), TextSystem::default()];
    let bold = names(&first, 600.);
    assert!(bold.ends_with("#77676874=600"), "{bold}");
    assert_eq!(
      [names(&second, 400.), names(&second, 600.)],
      [names(&first, 400.), bold]
    );
    let missing = Font::new("No Such Family h2o");
    assert!(first.font(&missing).is_none() && second.font(&missing).is_none());
    let faces = SYSTEM_FACES.0.lock().unwrap();
    assert!(faces.iter().any(
      |(key, faces)| key.0 == "No Such Family h2o" && faces.get().is_some_and(Option::is_none)
    ));
    let system = faces
      .iter()
      .find(|(key, _)| key.0 == ".AppleSystemUIFont")
      .unwrap();
    let face = &system.1.get().unwrap().as_ref().unwrap()[0].0.0;
    let shared = second
      .0
      .borrow()
      .faces
      .iter()
      .any(|kept| CFRetained::as_ptr(&kept.font) == CFRetained::as_ptr(face));
    assert!(shared, "the second text system uses the process's faces");
  }

  #[test]
  fn later_families_fill_in_missing_glyphs_before_the_system_fonts() {
    let text = TextSystem::default();
    let fonts = |family: &str| {
      let id = text.font(&Font::new(family)).unwrap();
      let line = text.shape(
        "aא",
        px(16.),
        &[Span {
          len: "aא".len(),
          font: id,
          size: px(16.),
          letter_spacing: px(0.),
          word_spacing: px(0.),
          inset: [px(0.); 2],
          nowrap: false,
        }],
      );
      line
        .glyphs
        .iter()
        .map(|glyph| name(&text, glyph.font))
        .collect::<Vec<_>>()
    };
    let alone = fonts("Helvetica");
    let listed = fonts("Helvetica\nTimes New Roman");
    assert!(
      alone[0].starts_with("Helvetica") && !alone[1].starts_with("TimesNewRoman"),
      "{alone:?}"
    );
    assert!(
      listed[0].contains("#cascade") && listed[1].starts_with("TimesNewRoman"),
      "{listed:?}"
    );
  }

  #[test]
  fn faces_without_bold_or_italic_get_synthetic_ones_like_chrome() {
    let text = TextSystem::default();
    let face = |weight, style, synthesis| {
      let id = text
        .font(&Font {
          weight,
          style,
          synthesis,
          ..Font::new("Herculanum")
        })
        .unwrap();
      let book = text.0.borrow();
      (id, book.face(id).bold, book.face(id).oblique)
    };
    let on = [true; 3];
    let (regular, ..) = face(400., FontStyle::Normal, on);
    assert_eq!(face(500., FontStyle::Normal, on).0, regular);
    let (bold, true, false) = face(600., FontStyle::Normal, on) else {
      panic!("600 is bold")
    };
    let (slanted, false, true) = face(400., FontStyle::Italic, on) else {
      panic!("italic slants")
    };
    assert!(matches!(
      face(700., FontStyle::Oblique, on),
      (_, true, true)
    ));
    // `font-synthesis` turns each off.
    assert_eq!(face(700., FontStyle::Italic, [false; 3]).0, regular);
    assert!(matches!(
      face(700., FontStyle::Italic, [false, true, true]),
      (_, false, true)
    ));
    // Helvetica has its own bold and italic faces.
    let helvetica = text.font(&Font {
      weight: 700.,
      style: FontStyle::Oblique,
      ..Font::new("Helvetica")
    });
    let book = text.0.borrow();
    let face = book.face(helvetica.unwrap());
    assert!(!face.bold && !face.oblique);
    drop(book);
    let glyph = text.glyph(regular, 'H').unwrap();
    let raster = |font, size, scale| GlyphRaster {
      font,
      glyph,
      size: px(size),
      scale,
      subpixel: 0,
      emoji: false,
      dilation: 0,
      stroke: 0.,
    };
    let bounds = |font| text.raster_bounds(&raster(font, 48., 2.)).unwrap();
    let [plain, bold_box, slanted_box] = [regular, bold, slanted].map(bounds);
    assert!(bold_box.size.width > plain.size.width && bold_box.size.height > plain.size.height);
    assert!(
      slanted_box.size.width > plain.size.width && slanted_box.size.height == plain.size.height
    );
    // Like Chrome, the advances stay those of the face.
    assert_eq!(text.advance(bold, glyph), text.advance(regular, glyph));
    // Small text at 1x gets bolder too.
    for (size, scale) in [(48., 2.), (12., 1.)] {
      let ink = |font| {
        let glyph = raster(font, size, scale);
        let (_, pixels) = text
          .rasterize(&glyph, text.raster_bounds(&glyph).unwrap())
          .unwrap();
        pixels.iter().map(|&value| u32::from(value)).sum::<u32>()
      };
      assert!(ink(bold) > ink(regular) * 11 / 10, "{size}px at {scale}x");
    }
  }

  #[test]
  fn fonts_from_memory_stay_with_their_text_system() {
    let bytes = std::fs::read("/System/Library/Fonts/Supplemental/Chalkduster.ttf").unwrap();
    let text = TextSystem::default();
    text.add_font(bytes, "Chalkduster").unwrap();
    assert!(text.families().iter().any(|family| family == "Chalkduster"));
    let id = text.font(&Font::new("Chalkduster")).unwrap();
    assert!(text.0.borrow().face(id).local);
    assert!(
      !SYSTEM_FACES
        .0
        .lock()
        .unwrap()
        .keys()
        .any(|key| key.0 == "Chalkduster")
    );
  }

  #[test]
  fn variable_weights_reuse_their_instances() {
    let bytes = std::fs::read("/System/Library/Fonts/SFNS.ttf").unwrap();
    let text = TextSystem::default();
    text.add_font(bytes, "Variable").unwrap();
    let font = |weight| {
      text
        .font(&Font {
          weight,
          ..Font::new("Variable")
        })
        .unwrap()
    };
    let bold = font(600.);
    assert_eq!(font(600.2), bold);
    let faces = text.0.borrow().faces.len();
    // A transition steps through many values, each time it runs.
    for _ in 0..3 {
      for step in 0..=300 {
        font(400. + step as f32 * 0.37);
      }
    }
    assert!(text.0.borrow().faces.len() - faces <= 112);
    assert_eq!(font(600.), bold);
  }

  #[test]
  fn css_matching_prefers_the_spec_order() {
    let faces = [
      (400., FontStyle::Normal, 1.),
      (700., FontStyle::Normal, 1.),
      (300., FontStyle::Italic, 1.),
    ];
    assert_eq!(best(&faces, 400., FontStyle::Normal, 1.), Some(0));
    assert_eq!(best(&faces, 600., FontStyle::Normal, 1.), Some(1));
    assert_eq!(best(&faces, 500., FontStyle::Normal, 1.), Some(0));
    assert_eq!(best(&faces, 900., FontStyle::Italic, 1.), Some(2));
    let narrow = [
      (400., FontStyle::Normal, 0.75),
      (400., FontStyle::Normal, 1.25),
    ];
    assert_eq!(best(&narrow, 400., FontStyle::Normal, 1.), Some(0));
    assert_eq!(
      (css_weight(0.), css_weight(0.23), css_weight(-1.)),
      (400., 500., 100.)
    );
    assert_eq!((css_stretch(0.), css_stretch(-1.)), (1., 0.5));
    assert_eq!(find_index(0.1, &KIT_WEIGHTS), 3.5);
  }
}
