use super::{Font, FontId, FontStyle, Fonts, TextRun, TextSystem, best};
use anyhow::Result;
use rustc_hash::FxHashMap;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

/// The descriptors of a face that the app adds, as in `@font-face`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FaceRules {
    /// Without it, the font says its weights, and matching takes it as normal. So do the style and the stretch.
    pub weight: Option<[f32; 2]>,
    pub style: Option<FontStyle>,
    pub stretch: Option<[f32; 2]>,
    /// The first and last code points of `unicode-range`. Empty for all.
    pub ranges: Vec<[u32; 2]>,
}

impl FaceRules {
    fn key(&self) -> ([f32; 2], FontStyle, [f32; 2]) {
        (
            self.weight.unwrap_or([400.; 2]),
            self.style.unwrap_or_default(),
            self.stretch.unwrap_or([1.; 2]),
        )
    }

    fn has(&self, ch: char) -> bool {
        self.ranges.is_empty()
            || (self.ranges.iter()).any(|&[low, high]| (low..=high).contains(&u32::from(ch)))
    }
}

pub(super) struct WebFace {
    pub(super) family: Rc<str>,
    /// The family that finds the face in the backend.
    name: String,
    rules: FaceRules,
    /// Its place among the rules of the page. A later one wins.
    order: usize,
}

/// Starts the family of one added face, before its index.
const ONE: char = '\u{1}';

impl Fonts {
    /// Adds a face to `family`, and returns the name for `add_face`.
    pub fn add_font(
        &self,
        bytes: Vec<u8>,
        family: &str,
        rules: FaceRules,
        order: usize,
    ) -> Result<String> {
        let own = format!("\u{2}{}", self.loaded.replace(self.loaded.get() + 1));
        let name = self.backend.add_font(bytes, &own)?;
        self.add_face(family, &name, rules, order);
        Ok(name)
    }

    /// Adds a loaded font to `family`. False when it is there already, in the same place.
    pub fn add_face(&self, family: &str, name: &str, rules: FaceRules, order: usize) -> bool {
        let mut faces = self.web.borrow_mut();
        let known = faces
            .iter()
            .position(|face| *face.family == *family && face.name == name && face.rules == rules);
        match known {
            Some(at) if faces[at].order == order => return false,
            Some(at) => faces[at].order = order,
            None => {
                (self.segmented).set(self.segmented.get() || !rules.ranges.is_empty());
                faces.push(WebFace {
                    family: family.into(),
                    name: name.to_owned(),
                    rules,
                    order,
                });
            }
        }
        self.resolved.borrow_mut().clear();
        self.recent.borrow_mut().clear();
        self.sources.borrow_mut().clear();
        self.generation.set(self.generation.get() + 1);
        true
    }

    /// The added faces of `family` that match best, the one with the last rule first.
    fn matches(&self, family: &str, font: &Font) -> Vec<usize> {
        let faces = self.web.borrow();
        let own: Vec<usize> = (0..faces.len())
            .filter(|&at| *faces[at].family == *family)
            .collect();
        let clamp = |value: f32, [low, high]: [f32; 2]| value.clamp(low, high);
        let traits: Vec<_> = own
            .iter()
            .map(|&at| {
                let (weight, style, stretch) = faces[at].rules.key();
                (
                    clamp(font.weight, weight),
                    style,
                    clamp(font.stretch, stretch),
                )
            })
            .collect();
        let Some(chosen) = best(&traits, font.weight, font.style, font.stretch) else {
            return Vec::new();
        };
        let chosen = faces[own[chosen]].rules.key();
        let mut matching: Vec<usize> = Vec::new();
        for at in own {
            if faces[at].rules.key() == chosen {
                let order = |at: usize| (faces[at].order, at);
                let place = matching.partition_point(|&other| order(other) > order(at));
                matching.insert(place, at);
            }
        }
        matching
    }

    /// The font for the backend when the family of `font` is one of an added face.
    pub(super) fn web_font(&self, font: &Font) -> Option<Font> {
        if self.web.borrow().is_empty() {
            return None;
        }
        // Like Chrome, the first face is the one for a space.
        let first = |line: &str, spaced: bool| {
            if let Some(index) = line.strip_prefix(ONE) {
                return index.parse().ok();
            }
            let found = self.matches(line, font);
            let faces = self.web.borrow();
            (found.iter().copied())
                .find(|&at| faces[at].rules.has(' '))
                .or(found.first().copied().filter(|_| !spaced))
        };
        let lines: Vec<&str> = font.family.split('\n').collect();
        let installed = |line: &str| {
            !line.starts_with(ONE) && self.web.borrow().iter().all(|face| *face.family != *line)
        };
        let head = (lines.iter())
            .position(|line| installed(line) || first(line, true).is_some())
            .unwrap_or(0);
        let mut request = font.clone();
        let names: Vec<String> = lines[head..]
            .iter()
            .map(|line| match first(line, false) {
                Some(at) => self.web.borrow()[at].name.clone(),
                None => line.to_string(),
            })
            .collect();
        request.family = names.join("\n").into();
        if let Some(at) = first(lines[head], false) {
            let rules = &self.web.borrow()[at].rules;
            if let Some([low, high]) = rules.weight {
                request.weight = font.weight.clamp(low, high);
                request.synthetic[0] =
                    Some(font.synthesis[0] && font.weight >= 600. && high < 600.);
            }
            if let Some([low, high]) = rules.stretch {
                request.stretch = font.stretch.clamp(low, high);
            }
            if let Some(style) = rules.style {
                request.style = style;
                request.synthetic[1] = Some(
                    font.synthesis[1]
                        && font.style != FontStyle::Normal
                        && style == FontStyle::Normal,
                );
            }
        }
        Some(request)
    }
}

type Made = ((usize, Option<usize>), Font, FontId);

/// Where the characters of a font with a `unicode-range` come from.
pub(super) struct Sources {
    font: Font,
    /// The face of the whole font.
    whole: FontId,
    /// The families, with the added faces of each.
    groups: Vec<(String, Vec<usize>)>,
    /// The fonts of the sources, and their faces.
    made: RefCell<Vec<Made>>,
    /// The source of each ASCII character, `UNKNOWN` until known.
    ascii: RefCell<[u16; 128]>,
    others: RefCell<FxHashMap<char, u16>>,
    /// The ASCII characters that the face of the whole font draws, as bits, when known.
    plain: Cell<u128>,
}

const UNKNOWN: u16 = u16::MAX;

impl Fonts {
    /// Whether characters of `font` can come from several faces by their `unicode-range`.
    pub fn segmented(&self, font: &Font) -> bool {
        self.segmented.get() && self.sources(font).is_some()
    }

    fn sources(&self, font: &Font) -> Option<Rc<Sources>> {
        let mut all = self.sources.borrow_mut();
        let same = |known: &Font| {
            Rc::ptr_eq(&known.family, &font.family)
                && known.weight == font.weight
                && known.style == font.style
                && known == font
        };
        if let Some((_, sources)) = all.iter().find(|(known, _)| same(known)) {
            return sources.clone();
        }
        let faces = self.web.borrow();
        let ranged = |line: &str| {
            (faces.iter()).any(|face| *face.family == *line && !face.rules.ranges.is_empty())
        };
        let sources = font.family.split('\n').any(ranged).then(|| {
            let groups = font.family.split('\n');
            Rc::new(Sources {
                font: font.clone(),
                whole: self.face(font),
                groups: groups
                    .map(|line| (line.to_owned(), self.matches(line, font)))
                    .collect(),
                made: RefCell::default(),
                ascii: RefCell::new([UNKNOWN; 128]),
                others: RefCell::default(),
                plain: Cell::default(),
            })
        });
        drop(faces);
        if all.len() >= 64 {
            all.clear();
        }
        all.push((font.clone(), sources.clone()));
        sources
    }
}

impl Sources {
    fn make(&self, fonts: &Fonts, key: (usize, Option<usize>), family: String) -> u16 {
        let mut made = self.made.borrow_mut();
        let at = made.iter().position(|(known, font, _)| {
            *known == key && (key.0 < self.groups.len() || *font.family == *family)
        });
        at.unwrap_or_else(|| {
            let font = Font {
                family: family.into(),
                ..self.font.clone()
            };
            let id = fonts.face(&font);
            made.push((key, font, id));
            made.len() - 1
        }) as u16
    }

    fn pick(&self, fonts: &Fonts, ch: char) -> u16 {
        let rest = |line: usize| {
            let rest = self.groups[line + 1..].iter();
            rest.fold(String::new(), |list, (line, _)| list + "\n" + line)
        };
        for (line, (name, group)) in self.groups.iter().enumerate() {
            let options = group.iter().copied().map(Some);
            let installed = group.is_empty().then_some(None);
            for at in options.chain(installed) {
                if at.is_some_and(|at| !fonts.web.borrow()[at].rules.has(ch)) {
                    continue;
                }
                let family = match at {
                    Some(at) => format!("{ONE}{at}{}", rest(line)),
                    None => format!("{name}{}", rest(line)),
                };
                let source = self.make(fonts, (line, at), family);
                if fonts.covers(self.made.borrow()[usize::from(source)].2, ch) {
                    return source;
                }
            }
        }
        // The system finds a font for other characters, unless the face has a glyph that is out of its range.
        let family = match fonts.covers(self.whole, ch) {
            true => super::SYSTEM_UI_FONT.with(|name| name.to_string()),
            false => self.font.family.to_string(),
        };
        self.make(fonts, (self.groups.len(), None), family)
    }

    fn of(&self, fonts: &Fonts, ch: char) -> u16 {
        let at = ch as usize;
        let known = match at < 128 {
            true => self.ascii.borrow()[at],
            false => self.others.borrow().get(&ch).copied().unwrap_or(UNKNOWN),
        };
        if known != UNKNOWN {
            return known;
        }
        let source = self.pick(fonts, ch);
        match at < 128 {
            true => {
                self.ascii.borrow_mut()[at] = source;
                if self.made.borrow()[usize::from(source)].2 == self.whole {
                    self.plain.set(self.plain.get() | 1 << at);
                }
            }
            false => {
                self.others.borrow_mut().insert(ch, source);
            }
        }
        source
    }
}

impl TextSystem {
    /// The runs split where the characters come from other faces of `unicode-range`.
    pub(super) fn segments(
        &self,
        text: &str,
        pieces: &[(usize, &TextRun)],
    ) -> Option<Vec<TextRun>> {
        if !self.segmented.get() {
            return None;
        }
        let mut runs: Option<Vec<TextRun>> = None;
        let mut start = 0;
        for (index, &(len, run)) in pieces.iter().enumerate() {
            let piece = &text[start..start + len];
            start += len;
            let whole = |run: &TextRun| run.part(len, run.inset);
            let Some(sources) = self.sources(&run.font).filter(|_| len > 0) else {
                runs.iter_mut().for_each(|runs| runs.push(whole(run)));
                continue;
            };
            let mask = sources.plain.get();
            let plain = (piece.bytes())
                .take_while(|&byte| byte < 128 && mask >> byte & 1 == 1)
                .count();
            if plain == len {
                runs.iter_mut().for_each(|runs| runs.push(whole(run)));
                continue;
            }
            let mut parts: Vec<(usize, u16)> = Vec::new();
            if let Some(last) = piece[..plain].chars().next_back() {
                parts.push((plain, sources.of(&self.fonts, last)));
            }
            for ch in piece[plain..].chars() {
                // Marks stay with their letter.
                let joins = matches!(ch, '\u{300}'..='\u{36f}' | '\u{200c}' | '\u{200d}' | '\u{fe00}'..='\u{fe0f}');
                let last = parts.last().filter(|_| joins).map(|part| part.1);
                let last =
                    last.filter(|&at| self.covers(sources.made.borrow()[usize::from(at)].2, ch));
                let source = last.unwrap_or_else(|| sources.of(&self.fonts, ch));
                match parts.last_mut().filter(|part| part.1 == source) {
                    Some(part) => part.0 += ch.len_utf8(),
                    None => parts.push((ch.len_utf8(), source)),
                }
            }
            let made = sources.made.borrow();
            // The face of the whole font draws these characters too.
            if parts
                .iter()
                .all(|part| made[usize::from(part.1)].2 == sources.whole)
            {
                runs.iter_mut().for_each(|runs| runs.push(whole(run)));
                continue;
            }
            let runs = runs.get_or_insert_with(|| {
                pieces[..index]
                    .iter()
                    .map(|&(len, run)| run.part(len, run.inset))
                    .collect()
            });
            let last = parts.len() - 1;
            for (at, (len, source)) in parts.into_iter().enumerate() {
                let edge = |here: bool, inset| if here { inset } else { crate::px(0.) };
                let inset = [edge(at == 0, run.inset[0]), edge(at == last, run.inset[1])];
                let mut run = run.part(len, inset);
                run.font = made[usize::from(source)].1.clone();
                runs.push(run);
            }
        }
        runs
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::px;

    #[test]
    fn added_faces_match_by_their_rules_and_combine_by_range() {
        let text = TextSystem::new();
        let bytes = |name: &str| {
            std::fs::read(format!("/System/Library/Fonts/Supplemental/{name}.ttf")).unwrap()
        };
        let rules = |weight: f32, ranges: &[[u32; 2]]| FaceRules {
            weight: Some([weight; 2]),
            ranges: ranges.to_vec(),
            ..FaceRules::default()
        };
        let add = |name: &str, family: &str, rules| {
            text.add_font(bytes(name), family, rules, 0).unwrap();
        };
        // A face is the weight that its rule declares.
        add("Georgia", "Brand", rules(700., &[]));
        add("Arial", "Brand", rules(400., &[]));
        let font = |family: &str, weight| Font {
            weight,
            ..Font::new(family)
        };
        let width = |font: Font, sample: &str| {
            let run = TextRun {
                len: sample.len(),
                font,
                ..TextRun::default()
            };
            text.shape_line(sample, px(20.), &run).line.width
        };
        assert_eq!(
            width(font("Brand", 700.), "Hamburg"),
            width(Font::new("Georgia"), "Hamburg")
        );
        assert_eq!(
            width(font("Brand", 400.), "Hamburg"),
            width(Font::new("Arial"), "Hamburg")
        );
        assert!(text.families().contains(&"Brand".to_owned()));
        // A font can have more faces than a byte counts.
        let code = |at: u32| if at < 95 { 0x20 + at } else { 0xa0 + at - 95 };
        for at in 0..300 {
            let range = [code(at); 2];
            add(
                if at == 299 { "Arial" } else { "Georgia" },
                "Many",
                rules(400., &[range]),
            );
        }
        let many: String = (0..300).filter_map(|at| char::from_u32(code(at))).collect();
        let run = TextRun {
            len: many.len(),
            font: Font::new("Many"),
            ..TextRun::default()
        };
        assert!(
            text.segments(&many, &[(many.len(), &run)])
                .is_some_and(|runs| runs.len() > 255)
        );
        // A later rule wins, whatever loads first, and the system draws what no face has.
        text.add_font(bytes("Georgia"), "Late", rules(400., &[[0x20, 0x7f]]), 2)
            .unwrap();
        text.add_font(bytes("Arial"), "Late", rules(400., &[[0x20, 0x7f]]), 1)
            .unwrap();
        assert_eq!(
            width(Font::new("Late"), "Hamburg"),
            width(Font::new("Georgia"), "Hamburg")
        );
        let run = |len| TextRun {
            len,
            font: Font::new("Late"),
            ..TextRun::default()
        };
        let split = |sample: &str| text.segments(sample, &[(sample.len(), &run(sample.len()))]);
        let parts = split("aé").unwrap();
        assert!(parts.len() == 2 && &*parts[1].font.family == ".SystemUIFont");
        assert!(split("aa").is_none() && split("a日").is_none());
        // Latin capitals from one face, the rest from another.
        add("Georgia", "Mixed", rules(400., &[[0x41, 0x5a]]));
        add("Arial", "Mixed", rules(400., &[[0x20, 0x40], [0x61, 0x7a]]));
        let mixed = width(Font::new("Mixed"), "ABab");
        let parts = width(Font::new("Georgia"), "AB") + width(Font::new("Arial"), "ab");
        assert!((mixed - parts).abs() < px(0.01), "{mixed:?} {parts:?}");
        let face = text.metrics(text.face(&Font::new("Mixed")));
        assert_eq!(
            face.ascent,
            text.metrics(text.face(&Font::new("Arial"))).ascent
        );
    }
}
