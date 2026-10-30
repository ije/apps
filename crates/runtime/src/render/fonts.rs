use crate::dom::{Document, State};
use std::{collections::HashMap, path::PathBuf};

/// Loads the TrueType and OpenType sources of `@font-face` rules, once each.
/// Returns true when a font was added, so styles must be computed again.
pub(super) fn load(doc: &Document, state: &mut State, assets: &HashMap<String, PathBuf>) -> bool {
    let faces: Vec<_> = doc
        .nodes
        .iter()
        .filter_map(|node| node.stylesheet.as_ref())
        .chain(state.sheets.values())
        .flat_map(|sheet| sheet.font_faces())
        .collect();
    let mut added = false;
    for (family, source) in faces {
        // WARNING: WOFF and WOFF2 sources need decompression, which is not supported.
        let Some(url) = crate::style::components(&source, ',')
            .into_iter()
            .find_map(|part| {
                let url = part
                    .strip_prefix("url(")?
                    .split(')')
                    .next()?
                    .trim_matches(['\'', '"']);
                let format = part.split("format(").nth(1).unwrap_or("");
                (url.ends_with(".ttf")
                    || url.ends_with(".otf")
                    || format.contains("truetype")
                    || format.contains("opentype"))
                .then(|| url.to_owned())
            })
        else {
            continue;
        };
        if !state.fonts.insert(url.clone()) {
            continue;
        }
        let path = match url.strip_prefix("apps:///") {
            Some(name) => assets.get(name).cloned(),
            None => url::Url::parse(&url)
                .ok()
                .and_then(|url| url.to_file_path().ok()),
        };
        let result = path
            .ok_or_else(|| anyhow::anyhow!("Font is not a bundled asset or local file: {url}"))
            .and_then(|path| Ok(std::fs::read(path)?))
            .and_then(|bytes| crate::dom::add_font(state, &family, bytes));
        match result {
            Ok(()) => added = true,
            Err(error) => eprintln!("apps: @font-face {family}: {error}"),
        }
    }
    added
}
