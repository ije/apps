use crate::css::properties::Prop;
use crate::dom::Document;
use crate::render::{PageState, ViewState};
use anyhow::{Context, Result};
use renderer::{Hooks, NodeCx, Window};
use rustc_hash::FxHashMap;
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

// The keys of the old and the new capture of a node, apart from the snapshots of drawable elements.
const OLD: u64 = 1 << 40;
const NEW: u64 = 1 << 41;

// The captures of a view transition. In mode 1 the page draws as before and its captures are read once. In
// mode 2 the page draws only into its captures, which the pseudo-elements show. A new state is a new `Capture`.
#[derive(Default)]
pub(crate) struct Capture {
    mode: u8,
    // The captured nodes and their keys. The capture of the root is the whole page.
    keys: FxHashMap<usize, u64>,
    root: Option<usize>,
    // The image pseudo-elements, with the key of the capture that each shows and the size it has until then.
    images: Vec<(usize, u64, [f32; 2])>,
    suppressed: bool,
}

impl Capture {
    pub(crate) fn size(&self, node: usize) -> Option<[f32; 2]> {
        Some(self.images.iter().find(|image| image.0 == node)?.2)
    }
}

// The capture that the frames of a view draw.
#[derive(Default)]
pub(crate) struct Captured {
    capture: RefCell<Option<Rc<Capture>>>,
    pub(crate) fresh: Cell<bool>,
}

// `dom.viewTransition`: `names` lists the captured elements in tree order, `capture` sets the mode, the captured
// nodes and the image nodes, `suppress` holds the frames and `end` drops it all.
pub(crate) fn call(
    args: &Value,
    doc: &mut Document,
    state: &mut PageState,
    viewport: [f32; 2],
) -> Result<Value> {
    let command = args[0].as_str().context("Expected a command")?;
    let list = |at: usize| (args[at].as_array().into_iter().flatten()).filter_map(Value::as_u64);
    match command {
        "names" => {
            let styles = state.restyle(doc, viewport);
            // Like Chrome, an element is captured when it has a box of its own in the document tree, outside SVG
            // and skipped content.
            let shown = |id: usize| {
                let (mut at, mut own) = (id, true);
                loop {
                    let (style, tag) = (&styles.values[at], &doc.nodes[at].tag);
                    let display = style.get(Prop::DISPLAY);
                    if display == Some("none")
                        || own && display == Some("contents")
                        || !own
                            && (tag == "svg"
                                || style.get(Prop::CONTENT_VISIBILITY) == Some("hidden"))
                    {
                        return false;
                    }
                    match doc.nodes[at].parent {
                        Some(parent) => (at, own) = (parent, false),
                        None => return at == 0,
                    }
                }
            };
            let mut found = Vec::new();
            for &(id, _) in state.styles.walk.snapshot().iter() {
                let (id, style) = (id as usize, &styles.values[id as usize]);
                if let Some(name) = (style.get(Prop::VIEW_TRANSITION_NAME))
                    .filter(|name| *name != "none" && !doc.nodes[id].tag.starts_with([':', '#']))
                    && shown(id)
                {
                    let classes = style.get(Prop::VIEW_TRANSITION_CLASS).unwrap_or("none");
                    found.push(json!([id, name, classes]));
                }
            }
            return Ok(json!(found));
        }
        "capture" => {
            let mode = args[1].as_u64().unwrap_or(0) as u8;
            let root = args[2].as_u64().map(|id| id as usize);
            let bit = if mode == 1 { OLD } else { NEW };
            let keys = list(3).chain(root.map(|id| id as u64));
            let images: Vec<u64> = list(4).collect();
            state.transition = Some(Rc::new(Capture {
                mode,
                keys: keys.map(|id| (id as usize, id | bit)).collect(),
                root,
                images: (images.as_chunks::<4>().0.iter())
                    .map(|four| (four[0] as usize, four[1], [four[2] as f32, four[3] as f32]))
                    .collect(),
                suppressed: false,
            }));
            state.paint_requested = true;
            show(state, |key| key & OLD != 0);
        }
        "suppress" => {
            state.transition = Some(Rc::new(Capture {
                suppressed: true,
                ..Default::default()
            }));
        }
        _ => {
            for &(node, ..) in state
                .transition
                .take()
                .iter()
                .flat_map(|capture| &capture.images)
            {
                state.media.retired.extend(state.media.images.remove(&node));
            }
            state
                .media
                .snapshots
                .retain(|&key, _| key as u64 & (OLD | NEW) == 0);
        }
    }
    state.dirty = true;
    Ok(Value::Null)
}

// During a transition the pseudo-elements cover the page, and the root takes the pointer.
pub(crate) fn covered(doc: &Document, state: &PageState) -> Option<usize> {
    state
        .transition
        .as_ref()
        .filter(|capture| capture.mode == 2)?;
    (doc.nodes[0].children.iter().copied()).find(|&id| doc.nodes[id].tag == "html")
}

// Gives the image pseudo-elements the captures of `keys` that are read.
fn show(state: &mut PageState, keys: impl Fn(u64) -> bool) {
    let Some(capture) = state.transition.clone() else {
        return;
    };
    for &(node, key, _) in capture.images.iter().filter(|image| keys(image.1)) {
        if let Some((pixels, _)) = state.media.snapshots.remove(&(key as usize)) {
            let image = crate::render::raster::render_image(&pixels, Vec::new());
            state
                .media
                .retired
                .extend(state.media.images.insert(node, image));
            state.layout.relayout(node);
        }
    }
}

impl Captured {
    // Takes the capture of the page at the start of a frame.
    pub(crate) fn update(&self, state: &PageState, window: &mut Window) {
        let new = state.transition.clone().filter(|capture| capture.mode != 0);
        let mut capture = self.capture.borrow_mut();
        if capture.as_ref().map(Rc::as_ptr) != new.as_ref().map(Rc::as_ptr) {
            *capture = new;
            self.fresh.set(true);
            window.repaint();
        }
    }

    // The update callback of a transition runs, and the window keeps the frame of the old state.
    pub(crate) fn suppressed(view: &ViewState) -> bool {
        (view.store.try_borrow().ok())
            .and_then(|store| Some(store.page_state(view.id)?.transition.as_ref()?.suppressed))
            .unwrap_or(false)
    }

    pub(crate) fn active(&self) -> bool {
        self.capture.borrow().is_some()
    }

    // Paints a captured node, or the page with `None`, into its capture. False when it has none.
    pub(crate) fn paint(
        &self,
        id: Option<usize>,
        bounds: renderer::Rect<renderer::Pixels>,
        window: &mut Window,
        draw: &mut dyn FnMut(&mut Window),
    ) -> bool {
        let found = self.capture.borrow().as_ref().and_then(|capture| {
            let id = match id {
                Some(id) if Some(id) != capture.root => id,
                Some(_) => return None,
                None => capture.root?,
            };
            Some((*capture.keys.get(&id)?, capture.mode))
        });
        match found {
            Some((key, 1)) => window.with_shown_snapshot(key, bounds, draw),
            Some((key, _)) => window.with_snapshot(Some((key, self.fresh.get())), bounds, draw),
            None => return false,
        }
        true
    }

    // The frame drew its captures, and read those of `changed`.
    pub(crate) fn drawn(&self, state: &mut PageState, changed: &[u64]) {
        if self
            .capture
            .borrow()
            .as_ref()
            .is_some_and(|capture| capture.mode == 2)
        {
            show(state, |key| changed.contains(&key));
        }
    }
}

// The page and its top layer, which the root captures.
pub(crate) struct Root(pub(crate) Rc<ViewState>);

impl Hooks for Root {
    fn volatile(&self) -> bool {
        true
    }

    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        if !self.0.captured.paint(None, cx.bounds, cx.window, inner) {
            inner(cx.window);
        }
    }
}
