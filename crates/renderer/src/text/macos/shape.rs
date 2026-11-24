use super::*;

impl Book {
    pub(super) fn shape(&mut self, text: &str, font_size: Pixels, spans: &[Span]) -> Line {
        if spans.is_empty() {
            return self.shape_whole(text, font_size, spans, None);
        }
        if text.is_ascii()
            && !text.contains(['\t', '\n', '\r'])
            && spans.iter().all(|span| self.face(span.font).mono)
        {
            return self.shape_pieces(text, font_size, spans);
        }
        let plain = |ch: char| {
            matches!(ch, ' '..='~' | '\u{a0}'..='\u{58f}' | '\u{1e00}'..='\u{1fff}' | '\u{2010}'..='\u{2027}'
                | '\u{2030}'..='\u{205e}' | '\u{20a0}'..='\u{20cf}')
        };
        if text.chars().all(plain) {
            let mut parts = mem::take(&mut self.words.parts);
            parts.clear();
            let line = self.shape_words(text, font_size, spans, &mut parts);
            self.words.parts = parts;
            if let Some(line) = line {
                debug_assert!(
                    same(&self.shape_whole(text, font_size, spans, None), &line),
                    "words of {text:?} shape unlike the whole line"
                );
                return line;
            }
        }
        self.shape_whole(text, font_size, spans, None)
    }

    fn shape_pieces(&mut self, text: &str, font_size: Pixels, spans: &[Span]) -> Line {
        self.words.trim(true);
        let mut glyphs = Vec::with_capacity(text.len());
        let mut x = 0f32;
        let mut start = 0;
        for span in spans {
            let key = (span.font, span.size.0.to_bits());
            for (offset, piece) in pieces(&text[start..start + span.len]) {
                let found = self
                    .words
                    .pieces
                    .get(&key)
                    .and_then(|cached| cached.get(piece))
                    .cloned();
                let shaped = match found {
                    Some(shaped) => shaped,
                    None => {
                        let alone = Span {
                            len: piece.len(),
                            ..*span
                        };
                        let shaped = Rc::new(
                            match self.digits(span.font, span.size, piece, 0..piece.len()) {
                                Some(word) => {
                                    let line = Line {
                                        glyphs: word
                                            .glyphs
                                            .iter()
                                            .map(|piece| piece.glyph(span.size, 0., 0))
                                            .collect(),
                                        width: px(word.width as f32),
                                        len: piece.len(),
                                        font_size: span.size,
                                        ..Default::default()
                                    };
                                    debug_assert!(
                                        same(
                                            &line,
                                            &self.shape_whole(piece, span.size, &[alone], None)
                                        ),
                                        "digits of {piece:?} shape unlike the whole piece"
                                    );
                                    line
                                }
                                None => self.shape_whole(piece, span.size, &[alone], None),
                            },
                        );
                        self.words.count += 1;
                        self.words
                            .pieces
                            .entry(key)
                            .or_default()
                            .insert(piece.into(), shaped.clone());
                        shaped
                    }
                };
                let at = (start + offset) as u32;
                glyphs.extend(shaped.glyphs.iter().map(|glyph| Glyph {
                    x: glyph.x + px(x),
                    index: glyph.index + at,
                    ..*glyph
                }));
                x += shaped.width.0;
            }
            start += span.len;
        }
        Line {
            glyphs,
            width: px(x),
            len: text.len(),
            font_size,
            ..Default::default()
        }
    }

    fn shape_words(
        &mut self,
        text: &str,
        font_size: Pixels,
        spans: &[Span],
        parts: &mut Vec<Part>,
    ) -> Option<Line> {
        self.words.trim(false);
        let (mut start, mut nudge, mut missing) = (0, false, 0);
        for (index, span) in spans.iter().enumerate() {
            // Every other span shapes a hair larger, as in `shape_whole`, or CoreText joins the spans and forms
            // ligatures across them.
            let size = if nudge {
                px(span.size.0.next_up())
            } else {
                span.size
            };
            let key = (span.font, size.0.to_bits());
            let source = &text[start..start + span.len];
            for (offset, piece) in pieces(source) {
                let range = start + offset..start + offset + piece.len();
                let around = u8::from(offset > 0) | u8::from(range.end < start + source.len()) << 1;
                if piece.starts_with(' ')
                    && around & 2 != 0
                    && let Some(next) = text[range.end..].chars().next()
                    && !self.spaced(span.font, size, next)
                {
                    return None;
                }
                let word = (self.words.faces.get(&key)).and_then(|face| {
                    face.words.get(&text[range.clone()])?[usize::from(around)].clone()
                });
                missing += usize::from(word.is_none());
                parts.push(Part {
                    span: index,
                    size,
                    range,
                    around,
                    word,
                });
            }
            nudge ^= span.len > 0;
            start += span.len;
        }
        if missing > 2 {
            let mut pens = Pens::default();
            let whole = self.shape_whole(text, font_size, spans, Some(&mut pens));
            self.harvest(text, 0, &whole, &pens, spans, parts);
            return Some(whole);
        }
        for at in 0..parts.len() {
            if parts[at].word.is_some() {
                continue;
            }
            let Part {
                span,
                size,
                ref range,
                around,
                ..
            } = parts[at];
            let before = (around & 1 != 0)
                .then(|| text[..range.start].chars().next_back())
                .flatten();
            let after = (around & 2 != 0)
                .then(|| text[range.end..].chars().next())
                .flatten();
            let context = range.start - before.map_or(0, char::len_utf8)
                ..range.end + after.map_or(0, char::len_utf8);
            let inner = range.start - context.start..range.end - context.start;
            if let Some(word) = self.digits(spans[span].font, size, &text[context.clone()], inner) {
                parts[at].word = Some(Rc::new(word));
                continue;
            }
            let alone = Span {
                len: context.len(),
                size,
                ..spans[span]
            };
            let mut pens = Pens::default();
            let shaped = self.shape_whole(&text[context.clone()], size, &[alone], Some(&mut pens));
            self.harvest(
                text,
                context.start,
                &shaped,
                &pens,
                spans,
                &mut parts[at..at + 1],
            );
            parts[at].word.as_ref()?;
        }
        let count = parts
            .iter()
            .map(|part| part.word.as_ref().map_or(0, |word| word.glyphs.len()))
            .sum();
        let mut glyphs = Vec::with_capacity(count);
        let mut x = 0f64;
        let mut last: Option<(usize, Pixels)> = None;
        for part in parts.iter() {
            let Some(word) = &part.word else {
                continue;
            };
            let size = match last {
                Some((span, size)) if span == part.span => size,
                _ => nearest(spans, part.size.0, font_size),
            };
            last = Some((part.span, size));
            glyphs.extend(
                word.glyphs
                    .iter()
                    .map(|piece| piece.glyph(size, x, part.range.start)),
            );
            x += word.width;
        }
        Some(Line {
            glyphs,
            width: px(x as f32),
            len: text.len(),
            font_size,
            ..Default::default()
        })
    }

    fn harvest(
        &mut self,
        text: &str,
        at: usize,
        line: &Line,
        pens: &Pens,
        spans: &[Span],
        parts: &mut [Part],
    ) {
        let glyphs = &line.glyphs;
        if glyphs.iter().any(|glyph| glyph.emoji) {
            return;
        }
        for part in parts {
            let first =
                glyphs.partition_point(|glyph| glyph.index as usize + at < part.range.start);
            let count = glyphs[first..]
                .partition_point(|glyph| (glyph.index as usize + at) < part.range.end);
            let own = &glyphs[first..first + count];
            if part.word.is_some()
                || own
                    .first()
                    .is_none_or(|head| own.iter().any(|glyph| glyph.font != head.font))
            {
                continue;
            }
            // CoreText adjusts glyphs at the edges of its runs, so a word without a space beside it takes its
            // edge from a run boundary.
            let edge = |index: usize| match index - at {
                end if end == line.len => Some(pens.xs[glyphs.len()]),
                index => pens
                    .starts
                    .iter()
                    .find(|start| start.0 == index)
                    .map(|start| start.1),
            };
            let left = if part.around & 1 == 0 {
                edge(part.range.start)
            } else {
                Some(pens.xs[first])
            };
            let right = if part.around & 2 == 0 {
                edge(part.range.end)
            } else {
                Some(pens.xs[first + count])
            };
            let (Some(left), Some(right)) = (left, right) else {
                continue;
            };
            let word = Rc::new(Word {
                glyphs: own
                    .iter()
                    .zip(&pens.xs[first..])
                    .map(|(glyph, x)| Piece {
                        x: x - left,
                        font: glyph.font,
                        id: glyph.id,
                        y: glyph.y.0,
                        offset: (glyph.index as usize + at - part.range.start) as u32,
                    })
                    .collect(),
                width: right - left,
            });
            self.words.count += 1;
            let face = self
                .words
                .faces
                .entry((spans[part.span].font, part.size.0.to_bits()))
                .or_default();
            face.words
                .entry(text[part.range.clone()].into())
                .or_default()[usize::from(part.around)] = Some(word.clone());
            part.word = Some(word);
        }
    }

    fn spaced(&mut self, font: FontId, size: Pixels, next: char) -> bool {
        let key = (font, size.0.to_bits());
        if let Some(&spaced) = self
            .words
            .faces
            .get(&key)
            .and_then(|face| face.spaced.get(&next))
        {
            return spaced;
        }
        let span = Span {
            len: 1,
            font,
            size,
            letter_spacing: px(0.),
            word_spacing: px(0.),
            inset: [px(0.); 2],
            nowrap: false,
        };
        let space = self.shape_whole(" ", size, &[span], None).width;
        let pair = format!(" {next}");
        let both = self.shape_whole(
            &pair,
            size,
            &[Span {
                len: pair.len(),
                ..span
            }],
            None,
        );
        let spaced = (both.glyphs.iter().find(|glyph| glyph.index > 0))
            .is_some_and(|glyph| (glyph.x - space).abs() < px(1e-4));
        self.words
            .faces
            .entry(key)
            .or_default()
            .spaced
            .insert(next, spaced);
        spaced
    }

    /// `word` is the part of `text` to return; the rest is context.
    fn digits(
        &mut self,
        font: FontId,
        size: Pixels,
        text: &str,
        word: Range<usize>,
    ) -> Option<Word> {
        const CHARS: &[u8; 11] = b"0123456789 ";
        let bytes = text.as_bytes();
        if !bytes.iter().all(|byte| CHARS.contains(byte)) {
            return None;
        }
        let slot = |byte: u8| {
            if byte == b' ' {
                10
            } else {
                usize::from(byte - b'0')
            }
        };
        let key = (font, size.0.to_bits());
        let face = self.words.faces.entry(key).or_default();
        face.misses = face.misses.saturating_add(1);
        let misses = face.misses;
        let mut table = match face.digits.take() {
            Some(table) => table,
            None if misses == 32 => self.digit_table(font, size, CHARS)?,
            None => return None,
        };
        let stitched = 'stitch: {
            if bytes.windows(3).any(|three| {
                !table.triples[slot(three[0]) * 121 + slot(three[1]) * 11 + slot(three[2])]
            }) {
                break 'stitch None;
            }
            let n = bytes.len();
            for (a, b) in [(0, 1), (n.wrapping_sub(2), n.wrapping_sub(1))] {
                let Some((&a, &b)) = bytes.get(a).zip(bytes.get(b)) else {
                    break;
                };
                let (a, b) = (slot(a), slot(b));
                if table.edges[a][b] == 0 {
                    let (glyphs, xs) = self.shape_digits(font, size, CHARS, &[a, b]);
                    let advance = table.advances[a][b];
                    let kept = matches!(glyphs[..], [(f, x, 0, true), (g, y, 1, true)]
                        if f == table.font && g == table.font && x == table.glyphs[a] && y == table.glyphs[b])
                        && xs[0] == 0.
                        && (xs[1] - advance).abs() < 1e-9
                        && (xs[2] - advance - table.ends[b]).abs() < 1e-9;
                    table.edges[a][b] = if kept { 1 } else { 2 };
                }
                if table.edges[a][b] != 1 {
                    break 'stitch None;
                }
            }
            let (mut x, mut left) = (0., 0.);
            let mut glyphs = Vec::with_capacity(word.len());
            for (at, &byte) in bytes.iter().enumerate().take(word.end) {
                if at == word.start {
                    left = x;
                }
                if at >= word.start {
                    glyphs.push(Piece {
                        x: x - left,
                        font: table.font,
                        id: table.glyphs[slot(byte)],
                        y: 0.,
                        offset: (at - word.start) as u32,
                    });
                }
                x += match bytes.get(at + 1) {
                    Some(&next) => table.advances[slot(byte)][slot(next)],
                    None => table.ends[slot(byte)],
                };
            }
            Some(Word {
                glyphs: glyphs.into(),
                width: x - left,
            })
        };
        self.words.faces.get_mut(&key)?.digits = Some(table);
        stitched
    }

    fn digit_table(&mut self, font: FontId, size: Pixels, chars: &[u8; 11]) -> Option<Box<Digits>> {
        let mut table = Box::new(Digits {
            font: FontId::MAX,
            glyphs: [0; 11],
            ends: [0.; 11],
            advances: [[f64::NAN; 11]; 11],
            triples: vec![false; 11 * 11 * 11].into(),
            edges: [[0; 11]; 11],
        });
        for at in 0..11 {
            let (glyphs, xs) = self.shape_digits(font, size, chars, &[at]);
            let [(face, id, 0, true)] = glyphs[..] else {
                return None;
            };
            if xs[0] != 0. || at > 0 && face != table.font {
                return None;
            }
            (table.font, table.glyphs[at], table.ends[at]) = (face, id, xs[1]);
        }
        // The concatenated Lyndon words of lengths 1 and 3 in order: the least de Bruijn sequence.
        let mut order = Vec::with_capacity(11 * 11 * 11 + 2);
        let mut lyndon = vec![0];
        loop {
            if 3 % lyndon.len() == 0 {
                order.extend_from_slice(&lyndon);
            }
            let period = lyndon.len();
            while lyndon.len() < 3 {
                lyndon.push(lyndon[lyndon.len() - period]);
            }
            while lyndon.last() == Some(&10) {
                lyndon.pop();
            }
            match lyndon.last_mut() {
                Some(last) => *last += 1,
                None => break,
            }
        }
        order.extend([order[0], order[1]]);
        let (glyphs, xs) = self.shape_digits(font, size, chars, &order);
        if glyphs.len() != order.len() {
            return None;
        }
        let mut moved = vec![false; order.len()];
        for (at, &(face, id, index, plain)) in glyphs.iter().enumerate() {
            moved[at] |=
                !plain || face != table.font || id != table.glyphs[order[at]] || index != at;
            if let Some(&next) = order.get(at + 1) {
                let step = xs[at + 1] - xs[at];
                let advance = &mut table.advances[order[at]][next];
                if advance.is_nan() {
                    *advance = step;
                } else if (*advance - step).abs() > 1e-9 {
                    moved[at] = true;
                    moved[at + 1] = true;
                }
            }
        }
        for (at, three) in order.windows(3).enumerate() {
            table.triples[three[0] * 121 + three[1] * 11 + three[2]] =
                !moved[at..at + 3].contains(&true);
        }
        Some(table)
    }

    fn shape_digits(
        &mut self,
        font: FontId,
        size: Pixels,
        chars: &[u8; 11],
        slots: &[usize],
    ) -> (Vec<Plain>, Vec<f64>) {
        let text: String = slots.iter().map(|&slot| char::from(chars[slot])).collect();
        let span = Span {
            len: text.len(),
            font,
            size,
            letter_spacing: px(0.),
            word_spacing: px(0.),
            inset: [px(0.); 2],
            nowrap: false,
        };
        let mut pens = Pens::default();
        let line = self.shape_whole(&text, size, &[span], Some(&mut pens));
        let glyphs = (line.glyphs.iter())
            .map(|glyph| {
                (
                    glyph.font,
                    glyph.id,
                    glyph.index as usize,
                    glyph.y == px(0.) && !glyph.emoji,
                )
            })
            .collect();
        (glyphs, pens.xs)
    }

    fn shape_whole(
        &mut self,
        text: &str,
        font_size: Pixels,
        spans: &[Span],
        mut pens: Option<&mut Pens>,
    ) -> Line {
        let string =
            CFMutableAttributedString::new(None, 0).expect("CFAttributedStringCreateMutable");
        let (mut rest, mut nudge) = (text, false);
        // Instances share a PostScript name, so runs map back to their span font by pointer.
        let mut own = Vec::with_capacity(spans.len());
        for span in spans {
            let piece;
            (piece, rest) = rest.split_at(span.len);
            let start = string.length();
            // SAFETY: the string is live and the range is its end.
            // a CFString drops some code points, like a leading byte order mark, so measure the length after the append.
            unsafe {
                CFMutableAttributedString::replace_string(
                    Some(&string),
                    CFRange::new(start, 0),
                    Some(&CFString::from_str(piece)),
                )
            };
            let range = CFRange::new(start, string.length() - start);
            // Alternate spans shape a hair larger so CoreText keeps them apart and forms no ligatures across
            // them. The first stays exact for the tracking of SF.
            let size = if nudge {
                span.size.0.next_up()
            } else {
                span.size.0
            };
            let font = self.sized(span.font, size);
            own.push((CFRetained::as_ptr(&font).as_ptr() as usize, span.font));
            // SAFETY: the range is inside the string, and the value is a font.
            unsafe {
                CFMutableAttributedString::set_attribute(
                    Some(&string),
                    range,
                    Some(kCTFontAttributeName),
                    Some(&font),
                )
            };
            if range.length > 0 {
                nudge = !nudge;
            }
        }
        // SAFETY: CoreText lays out a live attributed string, and its runs are CTRuns with font attributes.
        let line = unsafe { CTLine::with_attributed_string(&string) };
        let runs = unsafe { line.glyph_runs() };
        let runs = unsafe { runs.cast_unchecked::<CTRun>() };
        let mut glyphs = Vec::with_capacity(unsafe { line.glyph_count() } as usize);
        let mut scratch = mem::take(&mut self.scratch);
        let mut cursor = Utf16Cursor {
            text,
            utf8: 0,
            utf16: 0,
        };
        let (mut shift, mut emoji_end, mut pen) = (0f32, None::<f32>, 0f64);
        for run in runs.iter() {
            let font = unsafe {
                let attributes = run.attributes();
                let attributes = attributes.cast_unchecked::<CFString, CFType>();
                CFRetained::cast_unchecked::<CTFont>(
                    attributes.get(kCTFontAttributeName).expect("run font"),
                )
            };
            let run_size = nearest(spans, unsafe { font.size() } as f32, font_size);
            if self.natives.len() >= 512 {
                self.natives.clear();
            }
            let key = CFRetained::as_ptr(&font).as_ptr() as usize;
            let id = match own.iter().find(|(pointer, _)| *pointer == key) {
                Some(&(_, id)) => id,
                None => match self.natives.get(&key) {
                    Some(&(_, id)) => id,
                    None => {
                        let id = self.native(&font);
                        self.natives.insert(key, (font.clone(), id));
                        id
                    }
                },
            };
            let emoji = self.face(id).emoji;
            // The emoji font that system fonts fall back to has wider advances than Chromium's.
            let wide = emoji.then(|| unsafe {
                CTFont::with_name(
                    &CFString::from_static_str("AppleColorEmoji"),
                    f64::from(run_size.0) * 2.,
                    ptr::null(),
                )
            });
            let count = unsafe { run.glyph_count() } as usize;
            let all = CFRange::new(0, 0);
            scratch.ids.resize(count, 0);
            scratch.points.resize(count, CGPoint::ZERO);
            scratch.indices.resize(count, 0);
            // SAFETY: each buffer holds `count` values.
            unsafe {
                if count > 0 {
                    run.glyphs(all, NonNull::new_unchecked(scratch.ids.as_mut_ptr()));
                    run.positions(all, NonNull::new_unchecked(scratch.points.as_mut_ptr()));
                    run.string_indices(all, NonNull::new_unchecked(scratch.indices.as_mut_ptr()));
                }
            }
            let mut first = true;
            for k in 0..count {
                cursor.seek(scratch.indices[k] as usize);
                let at = scratch.points[k];
                if let Some(end) = emoji_end.take() {
                    shift = end - at.x as f32;
                }
                let x = at.x as f32 + shift;
                if let Some(pens) = pens.as_deref_mut() {
                    if mem::take(&mut first) {
                        pens.starts.push((cursor.utf8, pen));
                    }
                    pens.xs.push(at.x + f64::from(shift));
                }
                let mut id_raw = scratch.ids[k];
                glyphs.push(Glyph {
                    x: px(x),
                    y: px(at.y as f32),
                    size: run_size,
                    font: id,
                    index: cursor.utf8 as u32,
                    id: id_raw,
                    emoji,
                });
                if let Some(wide) = &wide {
                    // SAFETY: one glyph; the advances out pointer may be null.
                    let mut drawn = unsafe {
                        wide.advances_for_glyphs(
                            CTFontOrientation::Default,
                            NonNull::from(&mut id_raw),
                            ptr::null_mut(),
                            1,
                        )
                    } as f32;
                    // Where CoreText enlarges emoji, Chromium's advance is one device pixel less.
                    if drawn > run_size.0 * 2. {
                        drawn -= 1.;
                    }
                    emoji_end = Some(x + drawn / 2.);
                }
            }
            if pens.is_some() {
                // SAFETY: the null pointers ask for the width only.
                pen += unsafe {
                    run.typographic_bounds(all, ptr::null_mut(), ptr::null_mut(), ptr::null_mut())
                };
            }
        }
        self.scratch = scratch;
        // SAFETY: the null pointers ask for the width only.
        let width =
            unsafe { line.typographic_bounds(ptr::null_mut(), ptr::null_mut(), ptr::null_mut()) };
        if let Some(pens) = pens {
            pens.xs.push(width + f64::from(shift));
        }
        Line {
            glyphs,
            width: px(emoji_end.unwrap_or(width as f32 + shift)),
            len: text.len(),
            font_size,
            ..Default::default()
        }
    }
}

fn pieces(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let bytes = text.as_bytes();
    let mut start = 0;
    std::iter::from_fn(move || {
        let first = *bytes.get(start)?;
        let len = bytes[start..]
            .iter()
            .position(|&byte| (byte == b' ') != (first == b' '))
            .unwrap_or(bytes.len() - start);
        let piece = (start, &text[start..start + len]);
        start += len;
        Some(piece)
    })
}

fn nearest(spans: &[Span], shaped: f32, fallback: Pixels) -> Pixels {
    spans
        .iter()
        .map(|span| span.size)
        .min_by(|a, b| (a.0 - shaped).abs().total_cmp(&(b.0 - shaped).abs()))
        .unwrap_or(fallback)
}

/// Maps UTF-16 offsets from CoreText to UTF-8 offsets, walking either way.
struct Utf16Cursor<'a> {
    text: &'a str,
    utf8: usize,
    utf16: usize,
}

impl Utf16Cursor<'_> {
    fn seek(&mut self, utf16: usize) {
        while self.utf16 > utf16
            && let Some(ch) = self.text[..self.utf8].chars().next_back()
        {
            self.utf8 -= ch.len_utf8();
            self.utf16 -= ch.len_utf16();
        }
        for ch in self.text[self.utf8..].chars() {
            if self.utf16 >= utf16 {
                return;
            }
            self.utf8 += ch.len_utf8();
            self.utf16 += ch.len_utf16();
        }
    }
}

/// A shaped glyph of digits: face, glyph, source index, and whether it sits on the baseline and is no emoji.
type Plain = (FontId, GlyphId, usize, bool);

#[derive(Default)]
struct Pens {
    xs: Vec<f64>,
    starts: Vec<(usize, f64)>,
}

#[derive(Default)]
pub(super) struct Words {
    faces: FxHashMap<(FontId, u32), FaceWords>,
    pieces: FxHashMap<(FontId, u32), FxHashMap<Box<str>, Rc<Line>>>,
    count: usize,
    parts: Vec<Part>,
}

impl Words {
    fn trim(&mut self, pieces: bool) {
        if self.count > 16384 {
            self.faces.clear();
            if pieces {
                self.pieces.clear();
            }
            self.count = 0;
        }
    }
}

#[derive(Default)]
struct FaceWords {
    /// By text, then by whether text comes before (bit 0) and after (bit 1) it in its span.
    words: FxHashMap<Box<str>, [Option<Rc<Word>>; 4]>,
    spaced: FxHashMap<char, bool>,
    misses: u32,
    digits: Option<Box<Digits>>,
}

struct Digits {
    font: FontId,
    glyphs: [GlyphId; 11],
    ends: [f64; 11],
    advances: [[f64; 11]; 11],
    triples: Box<[bool]>,
    /// Pairs at word edges: 0 unknown, 1 stitch, 2 shape.
    edges: [[u8; 11]; 11],
}

struct Word {
    glyphs: Box<[Piece]>,
    width: f64,
}

#[derive(Clone, Copy)]
struct Piece {
    x: f64,
    font: FontId,
    id: GlyphId,
    y: f32,
    offset: u32,
}

impl Piece {
    fn glyph(&self, size: Pixels, pen: f64, start: usize) -> Glyph {
        Glyph {
            x: px((pen + self.x) as f32),
            y: px(self.y),
            size,
            font: self.font,
            index: start as u32 + self.offset,
            id: self.id,
            emoji: false,
        }
    }
}

struct Part {
    span: usize,
    size: Pixels,
    range: Range<usize>,
    around: u8,
    word: Option<Rc<Word>>,
}

pub(super) fn same(a: &Line, b: &Line) -> bool {
    (a.width - b.width).abs() < px(1e-3)
        && a.glyphs.len() == b.glyphs.len()
        && a.glyphs.iter().zip(&b.glyphs).all(|(a, b)| {
            (a.font, a.size, a.id, a.index, a.emoji, a.y)
                == (b.font, b.size, b.id, b.index, b.emoji, b.y)
                && (a.x - b.x).abs() < px(1e-3)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_and_digit_caches_shape_like_whole_lines() {
        let text = TextSystem::default();
        let id = text.font(&Font::new("Helvetica")).unwrap();
        let span = |len| Span {
            len,
            font: id,
            size: px(16.),
            letter_spacing: px(0.),
            word_spacing: px(0.),
            inset: [px(0.); 2],
            nowrap: false,
        };
        for sample in [
            "Hello world, again",
            "Type 1234 and 5678 then 90",
            "AV Wa To. fi fl",
        ] {
            let line = text.shape(sample, px(16.), &[span(sample.len())]);
            for _ in 0..40 {
                let again = text.shape(sample, px(16.), &[span(sample.len())]);
                assert!(same(&line, &again), "{sample}");
            }
            let whole =
                text.0
                    .borrow_mut()
                    .shape_whole(sample, px(16.), &[span(sample.len())], None);
            assert!(same(&line, &whole), "{sample}");
        }
        assert!(text.0.borrow().words.count > 0);
    }
}
