use crate::{
    dom::{Document, Node},
    style::{self, Prop},
};
use renderer::{ControlEvent, ControlKind, ControlSink, NativeControl, Pixels, Rect};
use std::sync::{Arc, Mutex, PoisonError};

const FALLBACKS: [(&str, &str); 7] = [
    ("ns-button", "button"),
    ("ns-switch", "checkbox"),
    ("ns-range", "range"),
    ("ns-text", "text"),
    ("ns-search", "search"),
    ("ns-color", "color"),
    ("ns-date", "date"),
];

impl Node {
    pub(crate) fn input_type(&self) -> &str {
        let kind = self.attrs.get("type").map_or("text", String::as_str);
        FALLBACKS
            .iter()
            .find(|(name, _)| kind.eq_ignore_ascii_case(name))
            .map_or(kind, |(_, fallback)| fallback)
    }

    pub(crate) fn native_control(&self) -> Option<ControlKind> {
        if !cfg!(target_os = "macos") || self.tag != "input" {
            return None;
        }
        let kind = self.attrs.get("type")?;
        let index = FALLBACKS
            .iter()
            .position(|(name, _)| kind.eq_ignore_ascii_case(name))?;
        Some(match index {
            0 => ControlKind::Button,
            1 => ControlKind::Switch,
            2 if self.attrs.contains_key("step") => ControlKind::Stepper,
            2 => ControlKind::Slider,
            3 => ControlKind::Text,
            4 => ControlKind::Search,
            5 => ControlKind::Color,
            _ => ControlKind::Date,
        })
    }
}

pub(super) fn size(kind: ControlKind) -> [f32; 2] {
    let [width, height] = renderer::control_size(kind);
    match kind {
        ControlKind::Slider => [129., height],
        ControlKind::Text | ControlKind::Search => [160., height],
        _ => [width, height],
    }
}

#[derive(Default)]
pub(crate) struct Controls {
    shown: rustc_hash::FxHashMap<usize, (u64, u64)>,
    next: u64,
    events: Arc<Mutex<Vec<ControlEvent>>>,
    sink: Option<ControlSink>,
}

pub(super) fn register(controls: &std::cell::RefCell<Controls>, id: usize) {
    let mut controls = controls.borrow_mut();
    let controls = &mut *controls;
    if !controls.shown.contains_key(&id) {
        controls.next += 1;
        controls.shown.insert(id, (controls.next, 0));
    }
    if controls.sink.is_none() {
        let (events, waker) = (controls.events.clone(), crate::host::Waker::current());
        controls.sink = Some(Arc::new(move |event| {
            events
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(event);
            waker.wake();
        }));
    }
}

pub(super) fn sync(controls: &std::cell::RefCell<Controls>, doc: &Document) {
    let mut controls = controls.borrow_mut();
    if !controls.shown.is_empty() {
        controls.shown.retain(|&id, _| {
            doc.nodes
                .get(id)
                .is_some_and(|node| node.native_control().is_some())
        });
    }
}

pub(super) fn paint(
    view: &super::PageState,
    id: usize,
    bounds: Rect<Pixels>,
    style: &style::Style,
    window: &mut renderer::Window,
) {
    let controls = view.controls.borrow();
    if controls.shown.is_empty() {
        return;
    }
    let (Some(&(key, seen)), Some(sink)) = (controls.shown.get(&id), &controls.sink) else {
        return;
    };
    let host = view.host.borrow();
    let Some(node) = host
        .pages
        .get(&view.id)
        .and_then(|page| page.doc.nodes.get(id))
    else {
        return;
    };
    let Some(kind) = node.native_control() else {
        return;
    };
    let attr = |name: &str| node.attrs.get(name).cloned().unwrap_or_default();
    let scheme = style.get(Prop::COLOR_SCHEME).unwrap_or("");
    let control = NativeControl {
        key,
        kind,
        bounds,
        clip: bounds,
        enabled: !node.attrs.contains_key("disabled"),
        editable: !node.attrs.contains_key("readonly"),
        checked: node.checked(),
        dark: match (scheme.contains("light"), scheme.contains("dark")) {
            (true, true) => None,
            (_, dark) => Some(dark),
        },
        value: match kind {
            ControlKind::Button => attr("value"),
            _ => node.current_value().unwrap_or("").to_owned(),
        },
        placeholder: attr("placeholder"),
        min: attr("min"),
        max: attr("max"),
        step: attr("step"),
        seen,
    };
    window.native_control(control, sink);
}

pub(crate) fn events(view: &super::PageState) -> Vec<(usize, ControlEvent)> {
    let mut controls = view.controls.borrow_mut();
    let events = std::mem::take(
        &mut *controls
            .events
            .lock()
            .unwrap_or_else(PoisonError::into_inner),
    );
    events
        .into_iter()
        .filter_map(|event| {
            let (&id, entry) = controls
                .shown
                .iter_mut()
                .find(|(_, (key, _))| *key == event.key)?;
            entry.1 = entry.1.max(event.seq);
            Some((id, event))
        })
        .collect()
}
