use super::{FontId, Fonts, GlyphId};
use crate::Pixels;

/// A line or a cubic of an outline, y down from the baseline.
#[cfg(not(windows))]
pub(super) enum Segment {
  Line([[f32; 2]; 2]),
  Cubic([[f32; 2]; 4]),
}

#[cfg(not(windows))]
impl Segment {
  pub(super) fn quad([a, b, c]: [[f32; 2]; 3]) -> Self {
    let lift = |end: [f32; 2]| [0, 1].map(|axis| end[axis] + (b[axis] - end[axis]) * 2. / 3.);
    Self::Cubic([a, lift(a), lift(c), c])
  }
}

pub(super) type Key = (FontId, GlyphId, [u32; 3]);

/// Like Skia's text intercepts: the left and right ends of the outline between `top` and `bottom`.
#[cfg(not(windows))]
pub(super) fn intercept(
  outline: impl IntoIterator<Item = Segment>,
  top: f32,
  bottom: f32,
) -> Option<[f32; 2]> {
  let (mut left, mut right) = (f32::INFINITY, f32::NEG_INFINITY);
  let mut expand = |x: f32| {
    left = left.min(x);
    right = right.max(x);
  };
  let mut cross = |from: [f32; 2], to: [f32; 2]| {
    for y in [top, bottom] {
      let t = (y - from[1]) / (to[1] - from[1]);
      if (0. ..1.).contains(&t) {
        expand(from[0] + t * (to[0] - from[0]));
      }
    }
  };
  let mut inside = Vec::new();
  for segment in outline {
    let points: &[[f32; 2]] = match &segment {
      Segment::Line(points) => {
        cross(points[0], points[1]);
        points
      }
      Segment::Cubic(points) => {
        const STEPS: u8 = 16;
        let at = |step: u8| {
          let t = f32::from(step) / f32::from(STEPS);
          let u = 1. - t;
          let weights = [u * u * u, 3. * u * u * t, 3. * u * t * t, t * t * t];
          [0, 1].map(|axis| (0..4).map(|k| weights[k] * points[k][axis]).sum())
        };
        for step in 0..STEPS {
          cross(at(step), at(step + 1));
        }
        points
      }
    };
    inside.extend(
      (points.iter())
        .filter(|point| top < point[1] && point[1] < bottom)
        .map(|point| point[0]),
    );
  }
  inside.into_iter().for_each(expand);
  (left < right).then_some([left, right])
}

impl Fonts {
  /// The left and right ends of the ink of a glyph between `top` and `bottom`, y down from the baseline.
  pub fn intercept(
    &self,
    font: FontId,
    glyph: GlyphId,
    size: Pixels,
    [top, bottom]: [f32; 2],
  ) -> Option<[f32; 2]> {
    let key = (font, glyph, [size.0, top, bottom].map(f32::to_bits));
    if let Some(&known) = self.intercepts.borrow().get(&key) {
      return known;
    }
    let found = (self.backend.ink(font, glyph, size.0))
      .filter(|ink| -ink[2] <= bottom && -ink[3] >= top)
      .and_then(|_| self.backend.intercept(font, glyph, size.0, top, bottom));
    let mut kept = self.intercepts.borrow_mut();
    if kept.len() >= 4096 {
      kept.clear();
    }
    kept.insert(key, found);
    found
  }
}

#[cfg(test)]
mod tests {
  use crate::{Font, TextSystem, px};

  #[test]
  fn outlines_report_the_ink_in_a_band() {
    let text = TextSystem::new();
    let font = text.face(&Font::new("Helvetica"));
    let glyph = |ch| text.backend.glyph(font, ch).unwrap();
    let band = |ch, band| text.intercept(font, glyph(ch), px(100.), band);
    // Under the baseline only a descender has ink, and its stem is far narrower than the letter.
    assert_eq!(band('x', [5., 10.]), None);
    let [left, right] = band('p', [5., 10.]).unwrap();
    assert!(left > 0. && right - left > 5. && right - left < 15.);
    let [a, b] = band('p', [-30., -20.]).unwrap();
    assert!(b - a > 40.);
    assert_eq!(band('p', [5., 10.]), Some([left, right]));
  }
}
