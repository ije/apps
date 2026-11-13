use super::{Line, TextWrap, WordBreak};
use crate::{Pixels, px};
use std::ops::Range;

#[derive(Clone, Copy, Default, PartialEq, Hash)]
pub struct Wrapping {
    /// No breaks without a width.
    pub width: Option<Pixels>,
    pub max_lines: Option<usize>,
    pub indent: Pixels,
    pub wrap: TextWrap,
    pub word_break: WordBreak,
}

#[derive(Clone, Copy)]
struct Chance {
    glyph: u32,
    x: f32,
    words_end: f32,
}

/// Returns the glyph index that starts each row after the first.
pub(crate) fn breaks(
    line: &Line,
    text: &str,
    wrapping: &Wrapping,
    nowrap: &[Range<usize>],
) -> Vec<u32> {
    let width = wrapping.width.unwrap_or_default();
    let held = |index: usize| nowrap.iter().any(|range| range.contains(&index));
    let even = wrapping.wrap != TextWrap::Wrap && wrapping.max_lines.is_none();
    let glyphs = &line.glyphs;
    let mut rows = Vec::new();
    let mut chances = Vec::new();
    if even {
        chances.push(Chance {
            glyph: 0,
            x: 0.,
            words_end: 0.,
        });
    }
    let mut chance: Option<(u32, Pixels)> = None;
    let (mut row_glyph, mut row_x) = (0, px(0.));
    let (mut started, mut words_end, mut previous_index) = (false, 0., 0);
    let (mut previous, mut after_hyphen) = ('\0', false);
    for (at, glyph) in glyphs.iter().enumerate() {
        let index = glyph.index as usize;
        let Some(ch) = text[index..].chars().next() else {
            continue;
        };
        if ch == '\n' {
            continue;
        }
        let joined = !nowrap.is_empty() && held(index) && held(previous_index);
        previous_index = index;
        let at = at as u32;
        if started
            && ch != ' '
            && !joined
            && (matches!(previous, ' ' | '\u{fffc}')
                || !is_word_char(ch)
                    && !(wrapping.word_break == WordBreak::KeepAll
                        && ch.is_alphanumeric()
                        && previous.is_alphanumeric())
                || wrapping.word_break == WordBreak::BreakAll
                || after_hyphen && ch.is_alphabetic())
        {
            chance = Some((at, glyph.x));
            if even {
                chances.push(Chance {
                    glyph: at,
                    x: glyph.x.0,
                    words_end,
                });
            }
        }
        started |= ch != ' ';
        let next_x = glyphs
            .get(at as usize + 1)
            .map_or(line.width, |next| next.x);
        if ch != ' ' {
            words_end = next_x.0;
        }
        let room = if rows.is_empty() {
            width - wrapping.indent
        } else {
            width
        };
        if ch != ' '
            && next_x - row_x > room + px(0.01)
            && at > row_glyph
            && (chance.is_some()
                || !matches!(wrapping.word_break, WordBreak::Normal | WordBreak::KeepAll)
                    && !joined)
        {
            if wrapping
                .max_lines
                .is_some_and(|max| rows.len() >= max.saturating_sub(1))
            {
                break;
            }
            (row_glyph, row_x) = chance.take().unwrap_or((at, glyph.x));
            rows.push(row_glyph);
        }
        after_hyphen = ch == '-' && previous.is_alphanumeric();
        previous = ch;
    }
    if !even || rows.is_empty() {
        return rows;
    }
    chances.push(Chance {
        glyph: row_glyph,
        x: line.width.0,
        words_end,
    });
    even_out(rows, &chances, width.0, wrapping)
}

fn even_out(rows: Vec<u32>, chances: &[Chance], max: f32, wrapping: &Wrapping) -> Vec<u32> {
    let (wrap, indent) = (wrapping.wrap, wrapping.indent.0);
    let last = chances.len() - 1;
    let span = |from: usize, to: usize| {
        chances[to].words_end - chances[from].x + if from == 0 { indent } else { 0. }
    };
    let mut greedy = vec![0];
    for row in &rows {
        match chances.iter().position(|chance| chance.glyph == *row) {
            Some(at) => greedy.push(at),
            None => return rows,
        }
    }
    greedy.push(last);
    let count = greedy.len() - 1;
    let ideal = match wrap {
        TextWrap::Balance => {
            greedy
                .windows(2)
                .map(|pair| span(pair[0], pair[1]))
                .sum::<f32>()
                / count as f32
        }
        _ => max - 45.,
    };
    let pretty = wrap == TextWrap::Pretty;
    if pretty && (1..=last).any(|at| span(at - 1, at) > ideal) {
        return rows;
    }
    // Balance keeps the row count of the greedy result: a layer per row count. Pretty needs only "reached".
    let exact = wrap == TextWrap::Balance && count <= 12;
    let layers = if exact { count + 1 } else { 2 };
    let cell = |at: usize, layer: usize| at * layers + layer;
    let mut cost = vec![f32::INFINITY; chances.len() * layers];
    let mut came = vec![0; chances.len() * layers];
    cost[0] = 0.;
    let (mut first, mut reached) = (0, 0);
    for to in 1..=last {
        while first < to && span(first, to) > max {
            first += 1;
        }
        if pretty && first > reached {
            // Nothing fits from the last reached chance, so take its longest overfull row.
            let end = (reached + 2..=last)
                .take_while(|&end| span(reached, end) <= max)
                .last()
                .unwrap_or(reached + 1);
            cost[cell(end, 1)] = cost[cell(reached, 1)].min(cost[cell(reached, 0)]);
            came[cell(end, 1)] = reached;
            reached = end;
        }
        for from in first..to {
            let width = span(from, to);
            let row = if pretty && to + 5 <= last && (width - ideal).abs() > 45. {
                f32::INFINITY
            } else {
                100. * ((width - ideal) / 15.).abs().powi(3)
            };
            for layer in 0..if exact { layers - 1 } else { layers } {
                let next = if exact { layer + 1 } else { 1 };
                let total = cost[cell(from, layer)] + row;
                let current = cost[cell(to, next)];
                if total < current
                    || exact && total.is_finite() && total - current <= current * 1e-6
                {
                    cost[cell(to, next)] = total;
                    came[cell(to, next)] = from;
                    reached = to;
                }
            }
        }
    }
    let mut layer = layers - 1;
    if !cost[cell(last, layer)].is_finite() {
        return rows;
    }
    let mut picked = Vec::new();
    let mut at = came[cell(last, layer)];
    while at > 0 {
        picked.push(chances[at].glyph);
        layer = if exact { layer - 1 } else { 1 };
        at = came[cell(at, layer)];
    }
    picked.reverse();
    picked
}

pub(crate) fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c,
            '\u{c0}'..='\u{24f}' | '\u{300}'..='\u{36f}' | '\u{400}'..='\u{4ff}' | '\u{980}'..='\u{9ff}'
            | '\u{1e00}'..='\u{1eff}' | '\u{a0}' | '\u{2011}' | '\u{202f}'
            | '-' | '_' | '.' | ',' | ':' | ';' | '\'' | '"' | '!' | '$' | '%' | '@' | '#' | '^' | '~' | '='
            | ')' | ']' | '}' | '‘' | '’' | '”' | '»' | '…' | '⋯')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::Glyph;

    /// A line of 10 px wide glyphs, one per character.
    fn line(text: &str) -> Line {
        let mut line = Line {
            glyphs: text
                .char_indices()
                .enumerate()
                .map(|(at, (index, _))| Glyph {
                    x: px(at as f32 * 10.),
                    y: px(0.),
                    size: px(10.),
                    font: 0,
                    index: index as u32,
                    id: 0,
                    emoji: false,
                })
                .collect(),
            width: px(text.len() as f32 * 10.),
            len: text.len(),
            ..Default::default()
        };
        line.sort_check();
        line
    }

    fn wrapping(width: f32, wrap: TextWrap) -> Wrapping {
        Wrapping {
            width: Some(px(width)),
            wrap,
            ..Default::default()
        }
    }

    #[test]
    fn objects_break_on_both_sides_and_nowrap_runs_hold_together() {
        let find = |text: &str, nowrap: &[Range<usize>]| {
            breaks(&line(text), text, &wrapping(0., TextWrap::Wrap), nowrap)
        };
        assert_eq!(find("ab\u{fffc}cd", &[]), [2, 3]);
        assert_eq!(find("ab cd ef", &[]), [3, 6]);
        assert_eq!(find("ab cd ef", std::slice::from_ref(&(0..5))), [6]);
    }

    #[test]
    fn hyphens_break_after_inside_words() {
        let text = "ab-cd x-12 -ef";
        assert_eq!(
            breaks(&line(text), text, &wrapping(0., TextWrap::Wrap), &[]),
            [3, 6, 11]
        );
    }

    #[test]
    fn keep_all_holds_ideographs_together_like_chrome() {
        let text = "ab 这是一个 中文";
        let mut wrapping = wrapping(0., TextWrap::Wrap);
        assert_eq!(
            breaks(&line(text), text, &wrapping, &[]),
            [3, 4, 5, 6, 8, 9]
        );
        wrapping.word_break = WordBreak::KeepAll;
        assert_eq!(breaks(&line(text), text, &wrapping, &[]), [3, 8]);
    }

    #[test]
    fn balance_and_pretty_even_out_lines() {
        let text = ["aaaa"; 9].join(" ");
        let line = line(&text);
        for (wrap, starts) in [
            (TextWrap::Wrap, [20, 40]),
            (TextWrap::Balance, [15, 30]),
            (TextWrap::Pretty, [15, 30]),
        ] {
            assert_eq!(
                breaks(&line, &text, &wrapping(200., wrap), &[]),
                starts,
                "{wrap:?}"
            );
        }
    }

    #[test]
    fn word_breaks_split_words_and_clamps_stop_early() {
        let text = "abcdefgh";
        let line = line(text);
        let mut wrapping = wrapping(30., TextWrap::Wrap);
        assert!(breaks(&line, text, &wrapping, &[]).is_empty());
        wrapping.word_break = WordBreak::BreakAll;
        assert_eq!(breaks(&line, text, &wrapping, &[]), [3, 6]);
        wrapping.max_lines = Some(2);
        assert_eq!(breaks(&line, text, &wrapping, &[]), [3]);
        wrapping.indent = px(10.);
        wrapping.max_lines = None;
        assert_eq!(breaks(&line, text, &wrapping, &[]), [2, 5]);
    }
}
