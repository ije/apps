use crate::{Pixels, Rect};

#[derive(Clone, Copy, Debug, PartialEq)]
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

/// `seen` is the last page change that the view took, so a value the page did not see yet is not overwritten.
#[derive(Clone, PartialEq)]
pub struct NativeControl {
    pub key: u64,
    pub kind: ControlKind,
    pub bounds: Rect<Pixels>,
    pub clip: Rect<Pixels>,
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

pub struct ControlEvent {
    pub key: u64,
    pub seq: u64,
    pub value: Option<String>,
    pub done: bool,
}

/// Called on the main thread.
pub type ControlSink = std::sync::Arc<dyn Fn(ControlEvent) + Send + Sync>;

pub fn control_size(kind: ControlKind) -> [f32; 2] {
    crate::platform::control_size(kind)
}
