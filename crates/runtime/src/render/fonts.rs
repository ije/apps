use crate::css::{sheet::parser::FONT_FORMATS, sheet::parser::FONT_TECHS};
use crate::dom::{Document, State};
use std::{collections::HashMap, path::PathBuf};

pub(crate) fn sources(src: &str) -> Vec<(&'static str, String)> {
    crate::css::value::tokens::components(src, ',')
        .into_iter()
        .filter_map(|part| {
            let (function, rest) = part.trim().split_once('(')?;
            let rest = rest.trim_start();
            let (value, hints) = match rest.as_bytes().first()? {
                &quote @ (b'"' | b'\'') => {
                    let (value, tail) = rest[1..].split_once(quote as char)?;
                    (value, tail.split_once(')')?.1)
                }
                _ => {
                    let (value, tail) = rest.split_once(')')?;
                    (value.trim(), tail)
                }
            };
            let hints = hints.to_ascii_lowercase();
            let known = |function: &str, known: &[&str], all: bool| {
                hints.split_once(function).is_none_or(|(_, tail)| {
                    let mut items = tail.split(')').next().unwrap_or("").split(',').map(|item| {
                        let item = item.trim().trim_matches(['"', '\'']);
                        known.contains(&item.strip_suffix("-variations").unwrap_or(item))
                    });
                    if all {
                        items.all(|known| known)
                    } else {
                        items.any(|known| known)
                    }
                })
            };
            match function.trim().to_ascii_lowercase().as_str() {
                "local" => Some(("local", value.to_owned())),
                "url"
                    if known("format(", &FONT_FORMATS, false)
                        && known("tech(", &FONT_TECHS, true) =>
                {
                    Some(("url", value.to_owned()))
                }
                _ => None,
            }
        })
        .collect()
}

pub(crate) type Face = (String, String, Option<String>, &'static str);

pub(crate) fn load(
    doc: &Document,
    state: &mut State,
    assets: &HashMap<String, PathBuf>,
) -> (bool, Vec<Face>) {
    if state.fonts.system.is_none() {
        return (false, Vec::new());
    }
    let loads = state.fonts.loads;
    let faces: Vec<_> = state
        .styles
        .cache
        .font_faces()
        .into_iter()
        .map(|(family, src, range)| {
            // Declarations keep `U+0400-04FF` as tokens joined by `/**/`.
            let range = range.map(|range| range.replace("/**/", ""));
            (family, sources(&src), src, range)
        })
        .collect();
    let ranges: Vec<_> = faces
        .iter()
        .map(|(_, sources, _, range)| {
            let pending = sources
                .iter()
                .filter(|(kind, _)| *kind == "url")
                .map(|(_, url)| state.fonts.files.get(url))
                .find(|tried| tried.is_none_or(Option::is_some));
            range.as_ref().filter(|_| pending == Some(None))
        })
        .collect();
    let revision = doc.nodes.latest();
    let scanned = ranges.iter().any(Option::is_some) && state.fonts.text.0 != revision;
    if ranges.iter().all(Option::is_none) {
        state.fonts.text = Default::default();
    } else if scanned {
        let (last, chars) = &mut state.fonts.text;
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
    let chars = &state.fonts.text.1;
    let mut order: Vec<_> = ranges
        .into_iter()
        .enumerate()
        .map(|(index, range)| {
            let Some(range) = range else {
                return (usize::MAX, index);
            };
            if !scanned {
                return (0, index);
            }
            let used = range
                .split(',')
                .filter_map(|range| {
                    let range = range.trim().strip_prefix(['U', 'u'])?.strip_prefix('+')?;
                    let hex = |text: &str| usize::from_str_radix(text, 16).ok();
                    let (low, high) = match range.split_once('-') {
                        Some((low, high)) => (hex(low)?, hex(high)?),
                        None => (
                            hex(&range.replace('?', "0"))?,
                            hex(&range.replace('?', "f"))?,
                        ),
                    };
                    Some((low, high.min(0x10ffff)))
                })
                .filter(|(low, high)| low <= high)
                .flat_map(|(low, high)| (low / 64..=high / 64).map(move |word| (word, low, high)))
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
        let (family, sources, ..) = &faces[index];
        statuses[index] = 'status: {
            for (kind, value) in sources {
                if *kind == "local" {
                    if crate::dom::local_font(state, family, value) {
                        break 'status "loaded";
                    }
                    continue;
                }
                let name = match state.fonts.files.get(value).cloned() {
                    Some(name) => {
                        if let Some(name) = &name {
                            crate::dom::use_font(state, family, name, false);
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
                            None => crate::bundle::local_file(assets, state.base.as_ref(), value)
                                .and_then(|path| {
                                    path.ok_or_else(|| {
                                        anyhow::anyhow!(
                                            "Font is not a bundled asset or local file: {value}"
                                        )
                                    })
                                })
                                .and_then(|path| Ok(std::fs::read(path)?)),
                        }
                        .and_then(|bytes| crate::dom::add_font(state, family, bytes));
                        if let Err(error) = &result {
                            eprintln!("apps: @font-face {family}: {error}");
                        }
                        let name = result.ok();
                        state.fonts.files.insert(value.clone(), name.clone());
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
        .map(|((family, _, src, range), status)| (family, src, range, status))
        .collect();
    (state.fonts.loads != loads, faces)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_skip_unsupported_formats_and_techs() {
        let src = r#"local("Inter Regular"), url(a.woff2) format("woff2"), url('b.eot') format("embedded-opentype"),
            url(c.svg#c) format(svg), url(d.ttf) format("truetype") tech(color-COLRv1),
            url(e.woff2) format("woff2-variations"), URL(f.otf) tech(variations, features-opentype),
            url("g).woff") format(woff), local(Arial   Bold), url(h.otc) format(collection)"#;
        assert_eq!(
            sources(src),
            [
                ("local", "Inter Regular"),
                ("url", "a.woff2"),
                ("url", "e.woff2"),
                ("url", "f.otf"),
                ("url", "g).woff"),
                ("local", "Arial   Bold"),
                ("url", "h.otc"),
            ]
            .map(|(kind, value)| (kind, value.to_owned()))
        );
    }

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
        let mut state = State::default();
        state.fonts.system = Some(system.clone());
        state.pixel_ratio = 1.;
        state.base = url::Url::parse("apps:///index.html").ok();
        let load = |doc: &mut Document, state: &mut State| {
            crate::dom::styles(doc, state, [800., 600.]);
            load(doc, state, &assets)
        };
        let (added, faces) = load(&mut doc, &mut state);
        assert!(added);
        assert_eq!(
            faces
                .iter()
                .map(|(family, _, _, status)| format!("{family} {status}"))
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
        // Added faces go under their CSS family, so no alias is needed.
        assert!(!state.fonts.aliases.contains_key("Ranked"));
        let width = |state: &State, font: &str, text: &str| {
            crate::render::canvas::line(&system, &state.fonts.aliases, font, text, [0.; 2])
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
    fn faces_of_one_family_match_by_their_weight_class() {
        let system = std::rc::Rc::new(renderer::TextSystem::new());
        let assets = HashMap::from(["test.woff2", "test-medium.woff2"].map(|name| {
            let fonts = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fonts");
            (name.to_owned(), PathBuf::from(fonts).join(name))
        }));
        let mut doc = Document::parse(
            "<style>@font-face { font-family: Brand; src: url(test.woff2) }
            @font-face { font-family: Brand; src: url(test-medium.woff2) }</style><p>AB</p>",
        );
        let mut state = State::default();
        state.fonts.system = Some(system.clone());
        state.pixel_ratio = 1.;
        state.base = url::Url::parse("apps:///index.html").ok();
        crate::dom::styles(&mut doc, &mut state, [800., 600.]);
        load(&doc, &mut state, &assets);
        for (weight, width) in [(400, 15.), (500, 7.5), (600, 7.5)] {
            let font = format!("{weight} 10px Brand");
            assert_eq!(
                crate::render::canvas::line(&system, &state.fonts.aliases, &font, "AB", [0.; 2])
                    .0
                    .width,
                renderer::px(width),
                "{font}"
            );
        }
    }
}

#[derive(Default)]
pub struct Fonts {
    pub(crate) aliases: HashMap<String, String>,
    pub(crate) files: HashMap<String, Option<String>>,
    pub(crate) loads: u32,
    pub(crate) text: (u64, Vec<u64>),
    pub(crate) system: Option<std::rc::Rc<renderer::TextSystem>>,
}
