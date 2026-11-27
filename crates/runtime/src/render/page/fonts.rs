use super::PageState;
use crate::dom::Document;
use crate::render::fonts::{Face, ranges, rules, sources};
use crate::style;
use anyhow::{Context, Result, ensure};
use std::{collections::HashMap, path::PathBuf};

// The `@font-face` rules and font calls of a page.
impl PageState {
    pub(crate) fn load_fonts(
        &mut self,
        doc: &Document,
        assets: &HashMap<String, PathBuf>,
    ) -> (bool, Vec<Face>) {
        if self.fonts.system.is_none() {
            return (false, Vec::new());
        }
        let loads = self.fonts.loads;
        let faces: Vec<_> = self
            .styles
            .cache
            .font_faces()
            .into_iter()
            .map(|face| (sources(&face.src), face))
            .collect();
        let pending: Vec<_> = faces
            .iter()
            .map(|(sources, face)| {
                let pending = sources
                    .iter()
                    .filter(|(kind, _)| *kind == "url")
                    .map(|(_, url)| self.fonts.files.get(url))
                    .find(|tried| tried.is_none_or(Option::is_some));
                face.range.as_deref().filter(|_| pending == Some(None))
            })
            .collect();
        let revision = doc.nodes.latest();
        let scanned = pending.iter().any(Option::is_some) && self.fonts.text.0 != revision;
        if pending.iter().all(Option::is_none) {
            self.fonts.text = Default::default();
        } else if scanned {
            let (last, chars) = &mut self.fonts.text;
            if chars.is_empty() {
                (*last, *chars) = (0, vec![0; 0x110000 / 64]);
            }
            for id in doc.nodes.changed_since(*last) {
                let node = &doc.nodes[id];
                if node.tag == "#text" {
                    for code in node.text.chars().map(|code| code as usize) {
                        chars[code / 64] |= 1 << (code % 64);
                    }
                }
            }
            *last = revision;
        }
        let chars = &self.fonts.text.1;
        let mut order: Vec<_> = pending
            .into_iter()
            .enumerate()
            .map(|(index, range)| {
                let Some(range) = range else {
                    return (usize::MAX, index);
                };
                if chars.is_empty() {
                    return (0, index);
                }
                let used = ranges(range)
                    .into_iter()
                    .map(|range| range.map(|code| code as usize))
                    .flat_map(|[low, high]| {
                        (low / 64..=high / 64).map(move |word| (word, low, high))
                    })
                    .map(|(word, low, high)| {
                        let mut bits = chars[word];
                        if word == low / 64 {
                            bits &= !0 << (low % 64);
                        }
                        if word == high / 64 {
                            bits &= !0 >> (63 - high % 64);
                        }
                        bits.count_ones() as usize
                    })
                    .sum();
                (used, index)
            })
            .collect();
        order.sort_by_key(|&(used, _)| std::cmp::Reverse(used));
        let mut statuses = vec!["unloaded"; faces.len()];
        for (_, index) in order.into_iter().filter(|&(used, _)| used > 0) {
            let (sources, face) = &faces[index];
            let family = &face.family;
            let rules = rules(
                face.range.as_deref(),
                [&face.weight, &face.style, &face.stretch].map(|value| value.as_deref()),
            );
            statuses[index] = 'status: {
                for (kind, value) in sources {
                    if *kind == "local" {
                        if self.local_font(family, value) {
                            break 'status "loaded";
                        }
                        continue;
                    }
                    let name = match self.fonts.files.get(value).cloned() {
                        Some(name) => {
                            if let Some(name) = &name {
                                let placed = self.fonts.placed.get(index);
                                if placed.is_none_or(|placed| placed.as_ref() != Some(face)) {
                                    self.add_face(family, name, rules.clone(), index);
                                    if self.fonts.placed.len() <= index {
                                        self.fonts.placed.resize(index + 1, None);
                                    }
                                    self.fonts.placed[index] = Some(face.clone());
                                }
                            }
                            name
                        }
                        None => {
                            let result = match value
                                .strip_prefix("data:")
                                .and_then(|data| data.split_once(";base64,"))
                            {
                                Some((_, payload)) => base64::Engine::decode(
                                    &base64::engine::general_purpose::STANDARD,
                                    payload.trim(),
                                )
                                .map_err(anyhow::Error::from),
                                None => {
                                    crate::bundle::local_file(assets, self.base.as_ref(), value)
                                        .and_then(|path| {
                                            path.ok_or_else(|| {
                                            anyhow::anyhow!(
                                                "Font is not a bundled asset or local file: {value}"
                                            )
                                        })
                                        })
                                        .and_then(|path| Ok(std::fs::read(path)?))
                                }
                            }
                            .and_then(|bytes| self.add_font(family, bytes, rules.clone(), index));
                            if let Err(error) = &result {
                                eprintln!("apps: @font-face {family}: {error}");
                            }
                            let name = result.ok();
                            self.fonts.files.insert(value.clone(), name.clone());
                            name
                        }
                    };
                    if name.is_some() {
                        break 'status "loaded";
                    }
                }
                "error"
            };
        }
        let faces = faces
            .into_iter()
            .zip(statuses)
            .map(|((_, face), status)| {
                let descriptors = [face.weight, face.style, face.stretch];
                (face.family, face.src, face.range, status, descriptors)
            })
            .collect();
        (self.fonts.loads != loads, faces)
    }

    pub(crate) fn add_font(
        &mut self,
        family: &str,
        bytes: Vec<u8>,
        rules: renderer::FaceRules,
        order: usize,
    ) -> Result<String> {
        const LIMIT: usize = 32 * 1024 * 1024;
        let system = self
            .fonts
            .system
            .as_ref()
            .context("Font loading requires a displayed window")?;
        ensure!(bytes.len() <= LIMIT, "Font exceeds 32 MB limit");
        let bytes = match bytes.get(..4) {
            Some(b"\0\x01\0\0" | b"true" | b"OTTO" | b"ttcf") => bytes,
            #[cfg(target_os = "macos")]
            Some(b"wOFF" | b"wOF2") => {
                #[allow(improper_ctypes)]
                unsafe extern "C" {
                    fn apps_font_sfnt(
                        bytes: *const u8,
                        length: usize,
                        allocate: extern "C" fn(&mut Vec<u8>, usize) -> *mut u8,
                        font: &mut Vec<u8>,
                    ) -> bool;
                }
                extern "C" fn allocate(font: &mut Vec<u8>, length: usize) -> *mut u8 {
                    if length > LIMIT {
                        return std::ptr::null_mut();
                    }
                    *font = vec![0; length];
                    font.as_mut_ptr()
                }
                let mut font = Vec::new();
                ensure!(
                    unsafe { apps_font_sfnt(bytes.as_ptr(), bytes.len(), allocate, &mut font) },
                    "Invalid or too large WOFF font"
                );
                font
            }
            #[cfg(not(target_os = "macos"))]
            Some(magic @ (b"wOFF" | b"wOF2")) => {
                let font = if magic == b"wOFF" {
                    wuff::decompress_woff1(&bytes)
                } else {
                    wuff::decompress_woff2(&bytes)
                }
                .map_err(|_| anyhow::anyhow!("Invalid WOFF font"))?;
                ensure!(font.len() <= LIMIT, "Font exceeds 32 MB limit");
                font
            }
            _ => anyhow::bail!("Unsupported font format"),
        };
        let mut fonts = crate::render::raster::svg::fonts(false);
        let database = std::sync::Arc::make_mut(&mut fonts.0);
        let ids = database.load_font_source(usvg::fontdb::Source::Binary(std::sync::Arc::new(
            bytes.clone(),
        )));
        let mut info = ids
            .first()
            .and_then(|id| database.face(*id))
            .context("Unsupported or invalid font data")?
            .clone();
        let name = system.add_font(bytes, family, rules.clone(), order)?;
        // Faces are found by the family of their `@font-face`, as in CSS.
        for &id in &ids {
            database.remove_face(id);
        }
        info.families = vec![(
            family.to_owned(),
            usvg::fontdb::Language::English_UnitedStates,
        )];
        if let Some([weight, _]) = rules.weight {
            info.weight = usvg::fontdb::Weight(weight as u16);
        }
        info.style = match rules.style {
            Some(renderer::FontStyle::Normal) => usvg::fontdb::Style::Normal,
            Some(renderer::FontStyle::Italic) => usvg::fontdb::Style::Italic,
            Some(renderer::FontStyle::Oblique) => usvg::fontdb::Style::Oblique,
            None => info.style,
        };
        database.push_face_info(info);
        self.fonts_changed();
        Ok(name)
    }

    // A file that another face uses too.
    fn add_face(&mut self, family: &str, name: &str, rules: renderer::FaceRules, order: usize) {
        let system = self.fonts.system.as_ref();
        if system.is_some_and(|system| system.add_face(family, name, rules, order)) {
            self.fonts_changed();
        }
    }

    fn fonts_changed(&mut self) {
        self.fonts.loads += 1;
        style::FONT_NAMES.with(|names| *names.borrow_mut() = Default::default());
        // Invalidate only font-dependent values. A new style cache loses constructed and CSSOM-edited sheets.
        self.styles.cache.environment.metrics = None;
        self.styles.invalidate();
        self.dirty = true;
    }

    pub(crate) fn local_font(&mut self, family: &str, name: &str) -> bool {
        let Some(system) = self.fonts.system.clone() else {
            return false;
        };
        style::font_names(&system, &self.fonts.aliases);
        let Some(found) = style::FONT_NAMES
            .with(|names| names.borrow().0.get(&name.to_ascii_lowercase()).cloned())
        else {
            return false;
        };
        let same = found.eq_ignore_ascii_case(family);
        if !same && !self.fonts.aliases.contains_key(family) {
            self.fonts.aliases.insert(family.to_owned(), found);
            self.fonts_changed();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_fall_back_and_load_by_unicode_range() {
        let system = std::rc::Rc::new(renderer::TextSystem::new());
        let fixture = |name: &str| {
            PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fonts")).join(name)
        };
        let assets = HashMap::from(
            [
                ("test.woff", "test.woff"),
                ("test.woff2", "test.woff2"),
                ("cyrillic.woff2", "test.woff2"),
                ("latin.woff2", "test.woff2"),
                ("arrows.woff2", "test-cff.woff2"),
            ]
            .map(|(name, file)| (name.to_owned(), fixture(file))),
        );
        let mut doc = Document::parse(
            r#"<style>
            @font-face { font-family: Fallback; src: url(missing.woff2) format("woff2"), url(test.woff) format("woff") }
            @font-face { font-family: Skipped; src: url(test.eot) format("embedded-opentype") }
            @font-face { font-family: Brand; src: local("No Such Font"), url(test.woff2) }
            @font-face { font-family: Cyrillic; src: url(cyrillic.woff2); unicode-range: U+0400-04FF }
            @font-face { font-family: Ranked; src: url(arrows.woff2); unicode-range: U+2190-21FF }
            @font-face { font-family: Ranked; src: url(latin.woff2); unicode-range: U+0000-00FF, U+0131 }
            </style><p>AB →</p>"#,
        );
        let mut state = PageState::default();
        state.fonts.system = Some(system.clone());
        state.pixel_ratio = 1.;
        state.base = url::Url::parse("apps:///index.html").ok();
        let load = |doc: &mut Document, state: &mut PageState| {
            state.restyle(doc, [800., 600.]);
            state.load_fonts(doc, &assets)
        };
        let (added, faces) = load(&mut doc, &mut state);
        assert!(added);
        assert_eq!(
            faces
                .iter()
                .map(|(family, _, _, status, _)| format!("{family} {status}"))
                .collect::<Vec<_>>(),
            [
                "Fallback loaded",
                "Skipped error",
                "Brand loaded",
                "Cyrillic unloaded",
                "Ranked loaded",
                "Ranked loaded"
            ]
        );
        assert_eq!(state.fonts.files["missing.woff2"], None);
        assert!(!state.fonts.files.contains_key("cyrillic.woff2"));
        assert!(!state.fonts.aliases.contains_key("Ranked"));
        let width = |state: &PageState, font: &str, text: &str| {
            crate::render::raster::canvas::text::line(
                &system,
                &state.fonts.aliases,
                font,
                text,
                [0.; 2],
            )
            .0
            .width
        };
        for family in ["Fallback", "Brand", "Ranked"] {
            assert_eq!(
                width(&state, &format!("10px {family}"), "AB"),
                renderer::px(15.)
            );
        }
        crate::style::font_names(&system, &state.fonts.aliases);
        let font = crate::style::font(&crate::css::properties::Style::from_iter([(
            crate::css::properties::Prop::FONT_FAMILY,
            "Missing, Brand, sans-serif".into(),
        )]));
        // The later installed families stay for the glyphs that the first lacks.
        assert!(font.family.starts_with("Brand\n"), "{}", font.family);
        assert!(!load(&mut doc, &mut state).0);
        assert_eq!(state.fonts.text.1.len(), 0x110000 / 64);
        let text = doc
            .nodes
            .iter()
            .position(|node| node.tag == "#text" && node.text.contains('B'))
            .unwrap();
        let revision = doc.nodes.revision(text);
        doc.nodes[text].text = "Ж".into();
        let (latest, tree) = (doc.nodes.latest(), doc.nodes.tree());
        doc.nodes.restore(&[(text, revision)], latest, tree);
        let (added, faces) = load(&mut doc, &mut state);
        assert!(!added && faces[3].3 == "unloaded");
        doc.nodes[text].text = "Ж".into();
        let (added, faces) = load(&mut doc, &mut state);
        assert!(added && faces[3].3 == "loaded");
        load(&mut doc, &mut state);
        assert!(state.fonts.text.1.is_empty());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn faces_of_one_family_match_by_their_declared_weight() {
        let system = std::rc::Rc::new(renderer::TextSystem::new());
        let assets = HashMap::from(["test.woff2", "test-medium.woff2"].map(|name| {
            let fonts = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fonts");
            (name.to_owned(), PathBuf::from(fonts).join(name))
        }));
        let mut doc = Document::parse(
            "<style>@font-face { font-family: Brand; src: url(test.woff2); font-weight: 700 }
            @font-face { font-family: Brand; src: url(test-medium.woff2); font-weight: 100 500 }</style><p>AB</p>",
        );
        let mut state = PageState::default();
        state.fonts.system = Some(system.clone());
        state.pixel_ratio = 1.;
        state.base = url::Url::parse("apps:///index.html").ok();
        state.restyle(&mut doc, [800., 600.]);
        state.load_fonts(&doc, &assets);
        for (weight, width) in [(100, 7.5), (500, 7.5), (600, 15.), (700, 15.)] {
            let font = format!("{weight} 10px Brand");
            assert_eq!(
                crate::render::raster::canvas::text::line(
                    &system,
                    &state.fonts.aliases,
                    &font,
                    "AB",
                    [0.; 2]
                )
                .0
                .width,
                renderer::px(width),
                "{font}"
            );
        }
    }

    #[test]
    fn ranged_faces_from_inserted_rules_load_without_a_dom_change() {
        let system = std::rc::Rc::new(renderer::TextSystem::new());
        let assets = HashMap::from(["cyrillic.woff2", "latin.woff2"].map(|name| {
            let fonts = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fonts");
            (name.to_owned(), PathBuf::from(fonts).join("test.woff2"))
        }));
        let mut doc = Document::parse(
            "<style>@font-face { font-family: Cyrillic; src: url(cyrillic.woff2); unicode-range: U+0400-04FF }</style><p>AB</p>",
        );
        let mut state = PageState::default();
        state.fonts.system = Some(system);
        state.pixel_ratio = 1.;
        state.base = url::Url::parse("apps:///index.html").ok();
        state.restyle(&mut doc, [800., 600.]);
        state.load_fonts(&doc, &assets);
        let style = doc
            .nodes
            .iter()
            .position(|node| node.tag == "style")
            .unwrap();
        let rule =
            "@font-face { font-family: Latin; src: url(latin.woff2); unicode-range: U+0000-00FF }";
        let styles = &mut state.styles;
        styles
            .cache
            .edit_sheet(
                &doc,
                style,
                &mut styles.sheets,
                Some((1, true)),
                |sheet, _| sheet.edit(&[1], crate::css::sheet::Edit::Insert(rule)),
            )
            .unwrap();
        state.styles.invalidate();
        state.restyle(&mut doc, [800., 600.]);
        let (added, faces) = state.load_fonts(&doc, &assets);
        assert!(added);
        assert_eq!(
            faces
                .iter()
                .map(|(family, _, _, status, _)| format!("{family} {status}"))
                .collect::<Vec<_>>(),
            ["Cyrillic unloaded", "Latin loaded"]
        );
    }
}
