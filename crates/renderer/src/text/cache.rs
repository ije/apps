use super::{FontId, Line, Wrapping, breaks::breaks};
use crate::{Pixels, platform, px};
use rustc_hash::{FxBuildHasher, FxHashMap};
use std::{
    cell::{Cell, RefCell},
    hash::BuildHasher,
    mem,
    rc::Rc,
};

#[derive(Clone, Copy, PartialEq, Hash)]
pub struct Span {
    pub len: usize,
    pub font: FontId,
    pub size: Pixels,
    pub letter_spacing: Pixels,
    pub word_spacing: Pixels,
    pub inset: [Pixels; 2],
    pub nowrap: bool,
}

const KEEP_FRAMES: u32 = 60;
const BUDGET: usize = 4 << 20;

#[derive(Default)]
pub(super) struct Cache(RefCell<State>);

#[derive(Default)]
struct State {
    frame: u32,
    bytes: usize,
    entries: FxHashMap<u64, Entry>,
}

struct Entry {
    text: Rc<str>,
    font_size: Pixels,
    spans: Box<[Span]>,
    wrap: Option<Wrapping>,
    line: Rc<Line>,
    breaks: Rc<[u32]>,
    used: Cell<u32>,
    bytes: usize,
}

type Found = (Rc<str>, Rc<Line>, Rc<[u32]>);

impl State {
    fn evict(&mut self) {
        let frame = self.frame;
        let keep = if self.bytes > BUDGET { 1 } else { KEEP_FRAMES };
        let mut freed = 0;
        self.entries.retain(|_, entry| {
            let kept = frame.wrapping_sub(entry.used.get()) < keep;
            freed += if kept { 0 } else { entry.bytes };
            kept
        });
        self.bytes -= freed;
        self.frame = frame.wrapping_add(1);
    }

    fn find(
        &self,
        hash: u64,
        text: &str,
        font_size: Pixels,
        spans: &[Span],
        wrap: Option<&Wrapping>,
    ) -> Option<Found> {
        let entry = self.entries.get(&hash)?;
        let same = *entry.text == *text
            && entry.font_size == font_size
            && *entry.spans == *spans
            && entry.wrap.as_ref() == wrap;
        same.then(|| {
            entry.used.set(self.frame);
            (entry.text.clone(), entry.line.clone(), entry.breaks.clone())
        })
    }

    fn keep(&mut self, hash: u64, entry: Entry) {
        entry.used.set(self.frame);
        self.bytes += entry.bytes;
        if let Some(old) = self.entries.insert(hash, entry) {
            self.bytes -= old.bytes;
        }
    }
}

impl Cache {
    pub(super) fn clear(&self) {
        self.0.take();
    }

    pub(super) fn end_frame(&self) {
        self.0.borrow_mut().evict();
    }

    pub(super) fn get(
        &self,
        platform: &platform::TextSystem,
        text: &str,
        font_size: Pixels,
        spans: &[Span],
        wrap: Option<&Wrapping>,
    ) -> Found {
        let hash = FxBuildHasher.hash_one((text, font_size, spans, wrap));
        if let Some(found) = self.0.borrow().find(hash, text, font_size, spans, wrap) {
            return found;
        }
        let (text, line, breaks) = match wrap {
            Some(wrap) => {
                let (text, line, _) = self.get(platform, text, font_size, spans, None);
                let mut end = 0;
                let nowrap: Vec<_> = spans
                    .iter()
                    .filter_map(|span| {
                        let start = end;
                        end += span.len;
                        span.nowrap.then_some(start..end)
                    })
                    .collect();
                let breaks: Rc<[u32]> = breaks(&line, &text, wrap, &nowrap).into();
                (text, line, breaks)
            }
            None => {
                let mut line = platform.shape(text, font_size, spans);
                if spans.iter().any(|span| {
                    span.letter_spacing != px(0.)
                        || span.word_spacing != px(0.)
                        || span.inset != [px(0.); 2]
                }) {
                    space(&mut line, text, spans);
                }
                line.sort_check();
                (Rc::<str>::from(text), Rc::new(line), Rc::default())
            }
        };
        let bytes = text.len()
            + line.glyphs.len() * mem::size_of::<super::Glyph>()
            + breaks.len() * mem::size_of::<u32>();
        let entry = Entry {
            text: text.clone(),
            font_size,
            spans: spans.into(),
            wrap: wrap.copied(),
            line: line.clone(),
            breaks: breaks.clone(),
            used: Cell::new(0),
            bytes,
        };
        self.0.borrow_mut().keep(hash, entry);
        (text, line, breaks)
    }
}

fn space(line: &mut Line, text: &str, spans: &[Span]) {
    let ends: Vec<usize> = spans
        .iter()
        .scan(0, |end, span| {
            *end += span.len;
            Some(*end)
        })
        .collect();
    let span_at = |index: usize| {
        ends.partition_point(|&end| end <= index)
            .min(spans.len() - 1)
    };
    let after = |index: usize| {
        let span = &spans[span_at(index)];
        let space = matches!(text[index..].chars().next(), Some(' ' | '\u{a0}'));
        span.letter_spacing + if space { span.word_spacing } else { px(0.) }
    };
    let mut shift = px(0.);
    let mut previous: Option<usize> = None;
    for glyph in &mut line.glyphs {
        let index = glyph.index as usize;
        match previous {
            None => shift += spans[span_at(index)].inset[0],
            Some(last) if last != index => {
                shift += after(last);
                let (from, to) = (span_at(last), span_at(index));
                if from != to {
                    shift += spans[from].inset[1];
                    shift += spans
                        .get(from + 1..=to)
                        .unwrap_or_default()
                        .iter()
                        .fold(px(0.), |sum, span| sum + span.inset[0]);
                }
            }
            Some(_) => {}
        }
        previous = Some(index);
        glyph.x += shift;
    }
    if let Some(last) = previous {
        shift += spans[span_at(last)].inset[1];
        shift += after(last);
    }
    line.width += shift;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::Glyph;

    #[test]
    fn spacing_and_insets_shift_following_glyphs() {
        let text = "ab cd";
        let mut line = Line {
            glyphs: (0..5)
                .map(|index| Glyph {
                    x: px(index as f32 * 10.),
                    y: px(0.),
                    size: px(10.),
                    font: 0,
                    index,
                    id: 0,
                    emoji: false,
                })
                .collect(),
            width: px(50.),
            len: 5,
            ..Default::default()
        };
        let span = |len, inset| Span {
            len,
            font: 0,
            size: px(10.),
            letter_spacing: px(1.),
            word_spacing: px(4.),
            inset,
            nowrap: false,
        };
        space(
            &mut line,
            text,
            &[span(3, [px(2.), px(0.)]), span(2, [px(3.), px(5.)])],
        );
        let xs: Vec<f32> = line.glyphs.iter().map(|glyph| glyph.x.0).collect();
        assert_eq!(xs, [2., 13., 24., 42., 53.]);
        assert_eq!(line.width, px(69.));
    }

    #[test]
    fn unused_lines_leave_the_cache() {
        let text = crate::TextSystem::new();
        let (cache, platform) = (&text.cache, &text.platform);
        let font = text.face(&crate::Font::new("Helvetica"));
        let spans = [Span {
            len: 5,
            font,
            size: px(16.),
            letter_spacing: px(0.),
            word_spacing: px(0.),
            inset: [px(0.); 2],
            nowrap: false,
        }];
        let (_, first, _) = cache.get(platform, "hello", px(16.), &spans, None);
        let (_, again, _) = cache.get(platform, "hello", px(16.), &spans, None);
        assert!(Rc::ptr_eq(&first, &again));
        let wrap = Wrapping {
            width: Some(px(1.)),
            ..Default::default()
        };
        cache.get(platform, "hello", px(16.), &spans, Some(&wrap));
        assert_eq!(cache.0.borrow().entries.len(), 2);
        for _ in 0..=KEEP_FRAMES {
            cache.end_frame();
        }
        assert!(cache.0.borrow().entries.is_empty());
        assert_eq!(cache.0.borrow().bytes, 0);
    }

    #[test]
    fn lines_shape_once_per_window_until_fonts_change() {
        let text = crate::TextSystem::new();
        let window = text.for_window();
        let first = window.shape_line("cached", px(16.), &run(0));
        let again = window.shape_line("cached", px(16.), &run(0xff0000ff));
        assert!(Rc::ptr_eq(&first.line, &again.line));
        let bytes = std::fs::read("/System/Library/Fonts/Supplemental/Chalkduster.ttf").unwrap();
        text.add_font(bytes).unwrap();
        assert!(text.families().iter().any(|family| family == "Chalkduster"));
        let fresh = window.shape_line("cached", px(16.), &run(0));
        assert!(!Rc::ptr_eq(&first.line, &fresh.line));
        assert_eq!(first.line.width, fresh.line.width);
    }

    fn run(color: u32) -> crate::TextRun {
        crate::TextRun {
            len: 6,
            color: crate::Color::hex(color),
            ..crate::TextRun::default()
        }
    }

    fn styled(len: usize, color: u32) -> crate::TextRun {
        crate::TextRun {
            len,
            color: crate::Color::hex(color),
            font: crate::Font::new("Helvetica"),
            ..crate::TextRun::default()
        }
    }

    #[test]
    fn runs_split_at_newlines_and_spans_split_where_paint_changes() {
        let text = crate::TextSystem::new();
        let runs = [
            styled(3, 0xff0000ff),
            styled(0, 0),
            styled(5, 0x0000ffff),
            styled(4, 0x0000ffff),
        ];
        let lines = text.shape_text("ab\ncdefg\nhi", px(16.), &runs, &Wrapping::default());
        let (red, blue) = (crate::Color::hex(0xff0000ff), crate::Color::hex(0x0000ffff));
        for (line, (source, look)) in
            lines
                .iter()
                .zip([("ab", (2, red)), ("cdefg", (5, blue)), ("hi", (2, blue))])
        {
            assert!(*line.text == *source && line.looks.len() == 1);
            assert!((line.looks[0].len, line.looks[0].color) == look);
        }
        // Spans split where paint changes, even at an empty run, so no ligature forms across.
        let spans = text.spans(runs.iter().map(|run| (run.len, run)), px(16.), true);
        assert!(spans.len() == 3 && spans[0].len == 3 && spans[2].len == 9);
        let wrapped = text.shape_text(
            "one two three four",
            px(16.),
            &[styled(18, 0)],
            &Wrapping {
                width: Some(px(40.)),
                max_lines: Some(2),
                ..Wrapping::default()
            },
        );
        let extent = wrapped[0].extent(px(20.));
        assert!(wrapped[0].rows() == 2 && extent.width == px(40.) && extent.height == px(40.));
        let clamp = |source: &str| {
            let wrapping = Wrapping {
                width: Some(px(1000.)),
                max_lines: Some(2),
                ..Wrapping::default()
            };
            let lines = text.shape_text(source, px(16.), &[styled(source.len(), 0)], &wrapping);
            lines.iter().map(|line| line.rows()).sum::<usize>()
        };
        assert!(clamp("a\nb\nc\nd") == 2 && clamp("a") == 1);
    }
}
