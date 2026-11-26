use super::{Caps, FontId, Fonts, TextRun, TextSystem};
use crate::Pixels;

// The features for a kind of `font-variant-caps`, best first, and whether lowercase letters and other characters get
// synthetic small capitals without them.
fn rules(caps: Caps) -> (&'static [&'static [&'static str]], [bool; 2]) {
    match caps {
        Caps::Normal => (&[], [false; 2]),
        Caps::Small => (&[&["smcp"]], [true, false]),
        Caps::AllSmall => (&[&["smcp", "c2sc"]], [true; 2]),
        Caps::Petite => (&[&["pcap"], &["smcp"]], [true, false]),
        Caps::AllPetite => (&[&["pcap", "c2pc"], &["smcp", "c2sc"]], [true; 2]),
        Caps::Unicase => (&[&["unic"]], [false, true]),
        Caps::Titling => (&[&["titl"]], [false; 2]),
    }
}

impl TextRun {
    fn add(&mut self, tags: &[&str]) {
        let mut features = self.font.features.to_vec();
        features.extend(tags.iter().map(|tag| (tag.to_string(), 1)));
        self.font.features = features.into();
    }
}

impl Fonts {
    /// Whether a face has a feature that replaces glyphs.
    #[cold]
    fn substitutes(&self, font: FontId, tag: &str) -> bool {
        let mut all = self.substitutions.borrow_mut();
        let at = all.iter().position(|(known, _)| *known == font);
        let at = at.unwrap_or_else(|| {
            let (table, mut tags) = self.backend.substitutions(font);
            let short = |at: usize| {
                let bytes = table.get(at..at + 2)?;
                Some(usize::from(u16::from_be_bytes([bytes[0], bytes[1]])))
            };
            let list = short(6).unwrap_or(0);
            for at in 0..short(list).unwrap_or(0) {
                tags.extend(
                    table
                        .get(list + 2 + at * 6..)
                        .and_then(|tag| tag.first_chunk()),
                );
            }
            all.push((font, tags));
            all.len() - 1
        });
        all[at].1.iter().any(|feature| feature == tag.as_bytes())
    }
}

impl TextSystem {
    /// The text and runs with the features or the synthetic small capitals of `font-variant-caps`.
    #[inline(never)]
    pub(super) fn capitals(
        &self,
        text: &str,
        pieces: &[(usize, &TextRun)],
        font_size: Pixels,
    ) -> Option<(String, Vec<TextRun>)> {
        if pieces.iter().all(|(_, run)| run.font.caps == Caps::Normal) {
            return None;
        }
        let mut out = String::with_capacity(text.len());
        let mut runs: Vec<TextRun> = Vec::new();
        for &(len, run) in pieces {
            let piece = &text[out.len()..out.len() + len];
            let (kinds, [lower, others]) = rules(run.font.caps);
            let mut plain = run.part(len, run.inset);
            plain.font.caps = Caps::Normal;
            let (face, _) = self.unadjusted(self.face(&plain.font));
            let found =
                (kinds.iter()).find(|tags| tags.iter().all(|tag| self.substitutes(face, tag)));
            if let Some(tags) = found {
                plain.add(tags);
            }
            // Like Chrome, without its own feature, unicase draws capitals as the small capitals of their lowercase.
            let unicase = run.font.caps == Caps::Unicase && self.substitutes(face, "smcp");
            let synthetic = run.font.synthesis[2] && (lower || others);
            if found.is_some() || !unicase && !synthetic || len == 0 {
                out.push_str(piece);
                runs.push(plain);
                continue;
            }
            let mut small = plain.part(len, run.inset);
            if unicase {
                small.add(&["smcp"]);
            } else {
                // Like Chrome, synthetic small capitals are at 70% of the size, in whole device pixels, without its
                // adjustment.
                let scale = match self.scale.get() {
                    0. => 1.,
                    scale => scale,
                };
                let size = run.font_size.unwrap_or(font_size);
                small.font.size_adjust = None;
                small.font_size = Some((size * 0.7 * scale).round() / scale);
            }
            let (first, mut before) = (runs.len(), None);
            for ch in piece.chars() {
                let single = |mapped: Option<char>| {
                    mapped.filter(|&mapped| mapped != ch && mapped.len_utf8() == ch.len_utf8())
                };
                let (mapped, smaller) = if unicase {
                    let mapped = single(ch.to_lowercase().next());
                    (mapped, mapped.is_some())
                } else {
                    let cased = ch.to_uppercase().ne([ch]);
                    let smaller = if cased { lower } else { others };
                    let mapped = ch.to_uppercase();
                    let whole = mapped.len() == 1;
                    (single(mapped.last()).filter(|_| smaller && whole), smaller)
                };
                out.push(mapped.unwrap_or(ch));
                match runs.last_mut().filter(|_| before == Some(smaller)) {
                    Some(last) => last.len += ch.len_utf8(),
                    None => {
                        let run = if smaller { &small } else { &plain };
                        runs.push(run.part(ch.len_utf8(), Default::default()));
                    }
                }
                before = Some(smaller);
            }
            let last = runs.len() - 1;
            runs[first].inset[0] = run.inset[0];
            runs[last].inset[1] = run.inset[1];
        }
        Some((out, runs))
    }
}

#[cfg(test)]
mod tests {
    use crate::{Caps, Font, TextRun, TextSystem, px};

    fn smcp(system: &TextSystem, family: &str) -> bool {
        let (face, _) = system.unadjusted(system.face(&Font::new(family)));
        system.substitutes(face, "smcp")
    }

    #[test]
    fn capitals_use_the_feature_or_smaller_letters() {
        let system = TextSystem::new();
        let capitals = |family: &str, caps, text: &str| {
            let run = TextRun {
                len: text.len(),
                font: Font {
                    caps,
                    ..Font::new(family)
                },
                ..TextRun::default()
            };
            let (text, runs) = system.capitals(text, &[(run.len, &run)], px(20.)).unwrap();
            let runs = runs.into_iter().map(|run| {
                let small = run.font_size.map(f32::from);
                (run.len, small, run.font.features.len())
            });
            (text, runs.collect::<Vec<_>>())
        };
        // On Linux, Helvetica can be a face with real small capitals, which needs no synthetic ones.
        if !smcp(&system, "Helvetica") {
            assert_eq!(
                capitals("Helvetica", Caps::Small, "Ab c1"),
                (
                    "AB C1".into(),
                    vec![
                        (1, None, 0),
                        (1, Some(14.), 0),
                        (1, None, 0),
                        (1, Some(14.), 0),
                        (1, None, 0)
                    ]
                )
            );
            assert_eq!(
                capitals("Helvetica", Caps::AllSmall, "Ab 1"),
                ("AB 1".into(), vec![(4, Some(14.), 0)])
            );
            assert_eq!(
                capitals("Helvetica", Caps::Unicase, "Aß"),
                ("Aß".into(), vec![(1, Some(14.), 0), (2, None, 0)])
            );
        }
        assert_eq!(
            capitals("Helvetica", Caps::Titling, "Ab"),
            ("Ab".into(), vec![(2, None, 0)])
        );
        if cfg!(target_os = "macos") {
            for family in [".AppleSystemUIFont", "Hoefler Text", "Baskerville"] {
                assert_eq!(
                    capitals(family, Caps::Small, "Ab"),
                    ("Ab".into(), vec![(2, None, 1)]),
                    "{family}"
                );
            }
            assert_eq!(
                capitals("Baskerville", Caps::Unicase, "Ab"),
                ("ab".into(), vec![(1, None, 1), (1, None, 0)])
            );
        }
    }

    #[test]
    fn adjusted_faces_with_capitals_shape_and_stay_bounded() {
        let system = TextSystem::new();
        let mut run = TextRun {
            len: 2,
            font: Font {
                caps: Caps::Small,
                ..Font::new("Helvetica")
            },
            ..TextRun::default()
        };
        for step in 0..1500 {
            run.font.size_adjust = Some(crate::SizeAdjust {
                metric: crate::AdjustMetric::XHeight,
                value: 0.5 + step as f32 / 1000.,
            });
            let line = system.shape_line("Ab", px(10.), &run).line;
            let face = system.face(&run.font);
            assert!(line.glyphs.iter().all(|glyph| glyph.font != face));
            assert!(system.line_metrics(face, px(10.))[0] > px(0.));
        }
        assert!(system.adjusted.borrow().len() <= 1024);
        if smcp(&system, "Helvetica") {
            return;
        }
        // The adjustment doesn't scale the synthetic small capitals.
        let (_, runs) = system.capitals("Ab", &[(2, &run)], px(10.)).unwrap();
        assert!(runs[0].font.size_adjust.is_some());
        assert!(runs[1].font.size_adjust.is_none() && runs[1].font_size == Some(px(7.)));
    }

    #[test]
    fn adjusted_sizes_scale_glyphs_and_metrics() {
        let system = TextSystem::new();
        let plain = Font::new("Helvetica");
        let x = system.metrics(system.face(&plain)).x_height;
        let adjusted = Font {
            size_adjust: Some(crate::SizeAdjust {
                metric: crate::AdjustMetric::XHeight,
                value: x * 2.,
            }),
            ..plain.clone()
        };
        let width = |font: &Font, size| {
            let run = TextRun {
                len: 3,
                font: font.clone(),
                ..TextRun::default()
            };
            system.shape_line("Abc", px(size), &run).line.width
        };
        assert!((width(&adjusted, 10.) - width(&plain, 20.)).abs() < px(0.01));
        let [ascent, ..] = system.line_metrics(system.face(&adjusted), px(10.));
        assert_eq!(ascent, system.line_metrics(system.face(&plain), px(20.))[0]);
    }
}
