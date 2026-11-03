use crate::{Pixels, Point, point};
use std::{ops::Range, path::PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    /// Command on macOS, the Windows key on Windows, Super on Linux.
    pub platform: bool,
    pub function: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capslock {
    pub on: bool,
}

/// A key press: the key name, like `a` or `enter`, and the text it types, if any.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Keystroke {
    pub modifiers: Modifiers,
    pub key: String,
    pub key_char: Option<String>,
}

impl Keystroke {
    /// Parses `cmd-shift-a`, `ctrl-enter` or `a`. `secondary` is cmd on macOS and ctrl elsewhere.
    pub fn parse(source: &str) -> Option<Self> {
        let mut modifiers = Modifiers::default();
        let mut key = None;
        let mut parts = source.split('-').peekable();
        while let Some(part) = parts.next() {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" => modifiers.control = true,
                "alt" => modifiers.alt = true,
                "shift" => modifiers.shift = true,
                "fn" => modifiers.function = true,
                "cmd" | "super" | "win" => modifiers.platform = true,
                "secondary" if cfg!(target_os = "macos") => modifiers.platform = true,
                "secondary" => modifiers.control = true,
                "" if parts.peek().is_none() && source.ends_with('-') => key = Some("-".to_owned()),
                _ if parts.peek().is_some() => return None,
                lower => {
                    if part.len() == 1 && part.as_bytes()[0].is_ascii_uppercase() {
                        modifiers.shift = true;
                    }
                    key = Some(lower.to_owned());
                }
            }
        }
        let key = key?;
        let key_char = (key.chars().count() == 1 && !modifiers.control && !modifiers.platform)
            .then(|| {
                if modifiers.shift {
                    key.to_uppercase()
                } else {
                    key.clone()
                }
            });
        Some(Self {
            modifiers,
            key,
            key_char,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Navigate(NavigationDirection),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NavigationDirection {
    Back,
    Forward,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScrollDelta {
    /// Precise pixels, from a trackpad.
    Pixels(Point<Pixels>),
    /// Lines, from a mouse wheel.
    Lines(Point<f32>),
}

impl Default for ScrollDelta {
    fn default() -> Self {
        Self::Lines(Point::default())
    }
}

impl ScrollDelta {
    pub fn precise(&self) -> bool {
        matches!(self, Self::Pixels(_))
    }
    pub fn pixel_delta(&self, line_height: Pixels) -> Point<Pixels> {
        match self {
            Self::Pixels(delta) => *delta,
            Self::Lines(delta) => point(line_height * delta.x, line_height * delta.y),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct KeyDownEvent {
    pub keystroke: Keystroke,
    /// The key is held down and repeating.
    pub is_held: bool,
}

#[derive(Clone, Debug, Default)]
pub struct KeyUpEvent {
    pub keystroke: Keystroke,
}

#[derive(Clone, Debug, Default)]
pub struct ModifiersChangedEvent {
    pub modifiers: Modifiers,
    pub capslock: Capslock,
}

#[derive(Clone, Debug)]
pub struct MouseDownEvent {
    pub button: MouseButton,
    pub position: Point<Pixels>,
    pub modifiers: Modifiers,
    pub click_count: usize,
    /// The click activated the window.
    pub first_mouse: bool,
}

#[derive(Clone, Debug)]
pub struct MouseUpEvent {
    pub button: MouseButton,
    pub position: Point<Pixels>,
    pub modifiers: Modifiers,
    pub click_count: usize,
}

#[derive(Clone, Debug, Default)]
pub struct MouseMoveEvent {
    pub position: Point<Pixels>,
    pub pressed_button: Option<MouseButton>,
    pub modifiers: Modifiers,
}

impl MouseMoveEvent {
    pub fn dragging(&self) -> bool {
        self.pressed_button == Some(MouseButton::Left)
    }
}

#[derive(Clone, Debug, Default)]
pub struct MouseExitEvent {
    pub position: Point<Pixels>,
}

#[derive(Clone, Debug, Default)]
pub struct ScrollWheelEvent {
    pub position: Point<Pixels>,
    pub delta: ScrollDelta,
    pub modifiers: Modifiers,
}

/// Files dragged onto a window from another app.
#[derive(Clone, Debug)]
pub enum FileDropEvent {
    Entered {
        position: Point<Pixels>,
        paths: Vec<PathBuf>,
    },
    Pending {
        position: Point<Pixels>,
    },
    Submit {
        position: Point<Pixels>,
    },
    Exited,
}

#[derive(Clone, Debug)]
pub enum PlatformInput {
    KeyDown(KeyDownEvent),
    KeyUp(KeyUpEvent),
    ModifiersChanged(ModifiersChangedEvent),
    MouseDown(MouseDownEvent),
    MouseUp(MouseUpEvent),
    MouseMove(MouseMoveEvent),
    MouseExited(MouseExitEvent),
    ScrollWheel(ScrollWheelEvent),
    FileDrop(FileDropEvent),
}

impl PlatformInput {
    pub(crate) fn mouse_position(&self) -> Option<Point<Pixels>> {
        match self {
            Self::MouseDown(event) => Some(event.position),
            Self::MouseUp(event) => Some(event.position),
            Self::MouseMove(event) => Some(event.position),
            Self::MouseExited(event) => Some(event.position),
            Self::ScrollWheel(event) => Some(event.position),
            Self::FileDrop(FileDropEvent::Entered { position, .. })
            | Self::FileDrop(FileDropEvent::Pending { position })
            | Self::FileDrop(FileDropEvent::Submit { position }) => Some(*position),
            _ => None,
        }
    }
}

pub trait MouseEvent: Sized + 'static {
    fn from_input(input: &PlatformInput) -> Option<&Self>;
}

macro_rules! mouse_event {
    ($ty:ident, $variant:ident) => {
        impl MouseEvent for $ty {
            fn from_input(input: &PlatformInput) -> Option<&Self> {
                match input {
                    PlatformInput::$variant(event) => Some(event),
                    _ => None,
                }
            }
        }
    };
}

mouse_event!(MouseDownEvent, MouseDown);
mouse_event!(MouseUpEvent, MouseUp);
mouse_event!(MouseMoveEvent, MouseMove);
mouse_event!(MouseExitEvent, MouseExited);
mouse_event!(ScrollWheelEvent, ScrollWheel);

/// Mouse listeners run twice: front to back in the capture phase, then back to front.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchPhase {
    Bubble,
    Capture,
}

impl DispatchPhase {
    pub fn bubble(self) -> bool {
        self == Self::Bubble
    }
    pub fn capture(self) -> bool {
        self == Self::Capture
    }
}

/// A text selection in UTF-16 code units.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Utf16Selection {
    pub range: Range<usize>,
    pub reversed: bool,
}
