use crate::css;
use crate::{
    css::properties::Prop,
    dom::{Document, Node},
    style,
};
use renderer::{ControlEvent, ControlKind, ControlSink, NativeControl, Pixels, Rect};
use std::sync::{Arc, Mutex, PoisonError};

impl Node {
    pub(crate) fn button_label(&self) -> &str {
        self.attrs.get("value").map_or(
            match self.input_type().to_ascii_lowercase().as_str() {
                "submit" => "Submit",
                "reset" => "Reset",
                _ => "",
            },
            String::as_str,
        )
    }

    /// The AppKit control of an input with `appearance: system`, on macOS.
    pub(crate) fn native_control(&self, style: &css::properties::Style) -> Option<ControlKind> {
        if style.get(Prop::APPEARANCE) != Some("system") {
            return None;
        }
        self.system_control()
    }

    /// The AppKit control that `appearance: system` gives this element, on macOS.
    pub(crate) fn system_control(&self) -> Option<ControlKind> {
        if !cfg!(target_os = "macos") || self.tag != "input" {
            return None;
        }
        Some(match self.input_type().to_ascii_lowercase().as_str() {
            "button" | "submit" | "reset" => ControlKind::Button,
            "checkbox" if self.attrs.contains_key("switch") => ControlKind::Switch,
            "range" => ControlKind::Slider,
            "number" => ControlKind::Stepper,
            "text" | "email" | "url" | "tel" => ControlKind::Text,
            "search" => ControlKind::Search,
            "color" => ControlKind::Color,
            "date" => ControlKind::Date,
            _ => return None,
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
        let (events, waker) = (controls.events.clone(), crate::jsc::Waker::current());
        controls.sink = Some(Arc::new(move |event| {
            events
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(event);
            waker.wake();
        }));
    }
}

pub(super) fn sync(
    controls: &std::cell::RefCell<Controls>,
    doc: &Document,
    styles: &style::Chunked<std::rc::Rc<css::properties::Style>>,
) {
    let mut controls = controls.borrow_mut();
    if !controls.shown.is_empty() {
        controls.shown.retain(|&id, _| {
            doc.nodes
                .get(id)
                .zip(styles.get(id))
                .is_some_and(|(node, style)| node.native_control(style).is_some())
        });
    }
}

pub(super) fn paint(
    view: &super::ViewState,
    id: usize,
    bounds: Rect<Pixels>,
    style: &css::properties::Style,
    window: &mut renderer::Window,
) {
    let controls = view.controls.borrow();
    let (Some(&(key, seen)), Some(sink)) = (controls.shown.get(&id), &controls.sink) else {
        return;
    };
    // Native controls are placed again in each frame.
    window.paint_volatile();
    let host = view.host.borrow();
    let Some(node) = host
        .pages
        .get(&view.id)
        .and_then(|page| page.doc.nodes.get(id))
    else {
        return;
    };
    let Some(kind) = node.native_control(style) else {
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
            ControlKind::Button => node.button_label().to_owned(),
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

pub(crate) fn events(view: &super::ViewState) -> Vec<(usize, ControlEvent)> {
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
