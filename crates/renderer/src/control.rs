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

#[derive(Clone, Debug)]
pub struct ControlEvent {
    pub key: u64,
    pub seq: u64,
    pub value: Option<String>,
    pub done: bool,
}

pub type ControlSink = Arc<dyn Fn(ControlEvent) + Send + Sync>;

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
