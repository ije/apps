use crate::{Bounds, Pixels};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ControlKind {
    Button,
    Switch,
    Slider,
    Stepper,
    Text,
    Search,
    Color,
    Date,
}

// WARNING: a native control the page shows in a box, in window points. `seen` is the last of the control's own
// events that the page took, so the page's value doesn't undo edits that it has not seen yet.
#[derive(Clone, PartialEq, Debug)]
pub struct NativeControl {
    pub key: u64,
    pub kind: ControlKind,
    pub bounds: Bounds<Pixels>,
    pub clip: Bounds<Pixels>,
    pub enabled: bool,
    pub editable: bool,
    pub checked: bool,
    pub dark: Option<bool>,
    pub value: String,
    pub placeholder: String,
    pub min: String,
    pub max: String,
    pub step: String,
    pub seen: u64,
}

// WARNING: `value` is `None` for a press, which the page runs as a click. `done` ends an edit, like a released
// slider or a text field that lost focus.
#[derive(Clone, Debug)]
pub struct ControlEvent {
    pub key: u64,
    pub seq: u64,
    pub value: Option<String>,
    pub done: bool,
}

pub type ControlSink = Arc<dyn Fn(ControlEvent) + Send + Sync>;

// WARNING: the natural size of a control, measured once from AppKit. A button gives its padding beside the title
// in place of a width, and sliders and text fields only their height.
pub fn control_size(kind: ControlKind) -> [f32; 2] {
    #[cfg(target_os = "macos")]
    {
        crate::platform::control_size(kind)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = kind;
        [0., 0.]
    }
}
