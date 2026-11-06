use crate::dom::{Document, State};
use crate::style::{FONT_FORMATS, FONT_TECHS};
use std::{collections::HashMap, path::PathBuf};

/// The `src` entries of `@font-face` and `FontFace` that can load, in order: `("local", name)` or
/// `("url", url)`. URLs with a `format()` or `tech()` that `@pierre/apps` doesn't support are left out.
pub(crate) fn sources(src: &str) -> Vec<(&'static str, String)> {
    crate::style::components(src, ',')
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
                        // WARNING: like in browsers, `woff2-variations` and such mean the format with `tech(variations)`.
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

/// A face's family, `src`, `unicode-range` and status.
pub(crate) type Face = (String, String, Option<String>, &'static str);

/// Loads the `@font-face` fonts that the latest style pass used, once per URL. Each face tries its
/// sources in order until one loads. A face with a `unicode-range` loads only when the page text
/// uses one of its characters, and faces that cover more of them load first, since the first face
/// of several with one name is the one that draws. Returns whether fonts changed, and the faces.
pub(crate) fn load(
    doc: &Document,
    state: &mut State,
    assets: &HashMap<String, PathBuf>,
) -> (bool, Vec<Face>) {
    if state.text_system.is_none() {
        return (false, Vec::new());
    }
    let loads = state.font_loads;
    let faces: Vec<_> = state
        .style_cache
        .font_faces()
        .into_iter()
        .map(|(family, src, range)| {
            // WARNING: declarations keep `U+0400-04FF` as tokens joined by `/**/`.
            let range = range.map(|range| range.replace("/**/", ""));
            (family, sources(&src), src, range)
        })
        .collect();
    let revision = doc.nodes.latest();
    let mut chars: Option<Vec<u64>> = None;
    let mut order: Vec<_> = faces
        .iter()
        .enumerate()
        .map(|(index, (_, sources, _, range))| {
            let pending = sources
                .iter()
                .filter(|(kind, _)| *kind == "url")
                .map(|(_, url)| state.fonts.get(url))
                .find(|tried| tried.is_none_or(Option::is_some));
            let Some(range) = range.as_ref().filter(|_| pending == Some(None)) else {
                return (usize::MAX, index);
            };
            if state.font_text == Some(revision) {
                return (0, index);
            }
            let chars = chars.get_or_insert_with(|| {
                let mut chars = vec![0u64; 0x110000 / 64];
                for node in doc.nodes.iter().filter(|node| node.tag == "#text") {
                    for code in node.text.chars().map(|code| code as usize) {
                        chars[code / 64] |= 1 << (code % 64);
                    }
                }
                chars
            });
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
    if chars.is_some() {
        state.font_text = Some(revision);
    }
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
                let name = match state.fonts.get(value).cloned() {
                    Some(name) => {
                        if let Some(name) = &name {
                            crate::dom::use_font(state, family, name, false);
                        }
                        name
                    }
                    None => {
                        let result = crate::host::local_file(assets, state, value)
                            .and_then(|path| {
                                path.ok_or_else(|| {
                                    anyhow::anyhow!(
                                        "Font is not a bundled asset or local file: {value}"
                                    )
                                })
                            })
                            .and_then(|path| Ok(std::fs::read(path)?))
                            .and_then(|bytes| crate::dom::add_font(state, family, bytes));
                        if let Err(error) = &result {
                            eprintln!("apps: @font-face {family}: {error}");
                        }
                        let name = result.ok();
                        state.fonts.insert(value.clone(), name.clone());
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
    (state.font_loads != loads, faces)
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
        state.text_system = Some(system.clone());
        state.base = url::Url::parse("apps:///index.html").ok();
        let load = |doc: &mut Document, state: &mut State| {
            crate::dom::styles(doc, state, [800., 600.], Some((&system, 1.)));
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
        assert_eq!(state.fonts["missing.woff2"], None);
        assert!(!state.fonts.contains_key("cyrillic.woff2"));
        // The face that covers more of the page text loads first, so the other one loads last.
        assert_eq!(state.font_aliases["Ranked"], "H2O Test CFF");
        let width = |state: &State, font: &str, text: &str| {
            crate::render::canvas::line(&system, &state.font_aliases, font, text, [0.; 2])
                .0
                .width
        };
        for family in ["Fallback", "Brand", "Ranked"] {
            assert_eq!(
                width(&state, &format!("10px {family}"), "AB"),
                renderer::px(15.)
            );
        }
        crate::style::font_names(&system, &state.font_aliases);
        let font = crate::style::font(&crate::style::Style::from_iter([(
            crate::style::Prop::FONT_FAMILY,
            "Missing, Brand, sans-serif".into(),
        )]));
        assert_eq!(&*font.family, "H2O Test");
        assert!(!load(&mut doc, &mut state).0);
        let text = doc
            .nodes
            .iter()
            .position(|node| node.tag == "#text" && node.text.contains('B'))
            .unwrap();
        doc.nodes[text].text = "Ж".into();
        let (added, faces) = load(&mut doc, &mut state);
        assert!(added && faces[3].3 == "loaded");
    }
}
