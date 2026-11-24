use super::super::Frame;
use super::Cx;
use crate::css::properties::{Prop, Style};
use crate::render::effects;
use crate::style::ComputedStyles;
use std::rc::Rc;

// Each shown `<iframe>` has a system web view, which loads its source and hides with its opacity.
pub(super) fn sync(
    cx: &Cx,
    iframes: &[usize],
    styles: &ComputedStyles,
    paint: &[(usize, Rc<Style>)],
    window: &mut renderer::Window,
    inspectable: bool,
) {
    let frame_nodes = || iframes.iter().copied().filter(|&id| cx.visible[id]);
    let mut frames = cx.view.frames.borrow_mut();
    frames.retain(|id, _| frame_nodes().any(|node| node == *id));
    for node in frame_nodes() {
        let (attrs, revision) = (&cx.doc.nodes[node].attrs, cx.doc.nodes.revision(node));
        let frame = match frames.entry(node) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                match apps_webview::Browser::new(
                    window,
                    &serde_json::json!({"javascript":attrs.get("sandbox").is_none_or(|s|s.split_whitespace().any(|s|s=="allow-scripts")),"inspectable":inspectable}),
                ) {
                    Ok(browser) => entry.insert(Frame {
                        browser,
                        loaded: String::new(),
                        bounds: [f64::NAN; 4],
                        revision: 0,
                        shown: true,
                    }),
                    Err(error) => {
                        eprintln!("apps: iframe: {error}");
                        continue;
                    }
                }
            }
        };
        let mut shown = styles.values[node]
            .get(Prop::VISIBILITY)
            .is_none_or(|value| value == "visible");
        let mut current = Some(node);
        while let Some(id) = current
            && shown
        {
            let style = paint
                .binary_search_by_key(&id, |(id, _)| *id)
                .ok()
                .map_or(&styles.values[id], |at| &paint[at].1);
            shown = effects::opacity(style) > 0.;
            current = cx.doc.nodes[id].parent;
        }
        if frame.shown != shown {
            frame.shown = shown;
            let _ = frame.browser.command("visible", &serde_json::json!(shown));
        }
        if frame.revision == revision {
            continue;
        }
        frame.revision = revision;
        let html = attrs.get("srcdoc");
        let url = attrs.get("src").map_or("about:blank", String::as_str);
        let source = html.map_or(url, String::as_str);
        if source != frame.loaded {
            let result = match html {
                Some(html) => frame.browser.load_html(html, "about:blank"),
                None => frame.browser.load_url(url),
            };
            if let Err(error) = result {
                eprintln!("apps: iframe: {error}");
            }
            frame.loaded = source.to_owned();
        }
    }
}
