use super::{AdjustMetric, FaceMetrics, FontId, Fonts, SizeAdjust};

/// Set in the ids of faces with a `font-size-adjust`, which the backends don't know. Glyphs never hold such an id.
pub(super) const ADJUSTED: FontId = 1 << 31;

impl Fonts {
  /// The face of `font` for the backend, and the scale of its sizes.
  pub(super) fn unadjusted(&self, font: FontId) -> (FontId, f32) {
    match font & ADJUSTED {
      0 => (font, 1.),
      _ => (self.adjusted.borrow().get((font & !ADJUSTED) as usize))
        .copied()
        .unwrap_or((0, 1.)),
    }
  }

  pub(super) fn adjust(&self, base: FontId, adjust: SizeAdjust) -> FontId {
    let metrics = self.metrics(base);
    let own = match adjust.metric {
      AdjustMetric::XHeight => metrics.x_height,
      AdjustMetric::CapHeight => metrics.cap_height,
      AdjustMetric::Advance => {
        let zero = self.backend.glyph(base, '0');
        zero.map_or(0., |glyph| self.backend.advance(base, glyph))
      }
    };
    let entry = (base, if own > 0. { adjust.value / own } else { 1. });
    let mut adjusted = self.adjusted.borrow_mut();
    let at = adjusted.iter().position(|&known| known == entry);
    let at = at.unwrap_or_else(|| {
      // An animated value makes many faces. The text of the old ones shapes again.
      if adjusted.len() >= 1024 {
        adjusted.clear();
        self.resolved.borrow_mut().clear();
        self.recent.borrow_mut().clear();
        self.sources.borrow_mut().clear();
        self.generation.set(self.generation.get() + 1);
      }
      adjusted.push(entry);
      adjusted.len() - 1
    });
    ADJUSTED | at as FontId
  }
}

impl FaceMetrics {
  pub(super) fn scale(self, scale: f32) -> Self {
    Self {
      ascent: self.ascent * scale,
      descent: self.descent * scale,
      line_gap: self.line_gap * scale,
      cap_height: self.cap_height * scale,
      x_height: self.x_height * scale,
      bounds: self.bounds * scale,
      ..self
    }
  }
}
