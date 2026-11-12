use crate::{Pixels, Point};
use std::{ops::Range, path::PathBuf};

/// `COMMAND` is Cmd on macOS and the Windows or Super key elsewhere.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const NONE: Self = Self(0);
    pub const CTRL: Self = Self(1);
    pub const ALT: Self = Self(2);
    pub const SHIFT: Self = Self(4);
    pub const COMMAND: Self = Self(8);
    pub const FN: Self = Self(16);

    pub fn ctrl(self) -> bool {
        self.has(Self::CTRL)
    }
    pub fn alt(self) -> bool {
        self.has(Self::ALT)
    }
    pub fn shift(self) -> bool {
        self.has(Self::SHIFT)
    }
    pub fn command(self) -> bool {
        self.has(Self::COMMAND)
    }
    pub fn function(self) -> bool {
        self.has(Self::FN)
    }
    pub fn has(self, keys: Self) -> bool {
        self.0 & keys.0 == keys.0
    }
    pub const fn plus(self, keys: Self) -> Self {
        Self(self.0 | keys.0)
    }
    pub fn with(self, keys: Self, on: bool) -> Self {
        Self(if on {
            self.0 | keys.0
        } else {
            self.0 & !keys.0
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Key {
    pub name: String,
    pub text: Option<String>,
}

impl Key {
    /// `ctrl-` is the minus key. A single uppercase letter adds shift.
    pub fn parse(source: &str) -> Option<(Self, Modifiers)> {
        let modifier = |word: &str| match word {
            "ctrl" => Some(Modifiers::CTRL),
            "alt" => Some(Modifiers::ALT),
            "shift" => Some(Modifiers::SHIFT),
            "fn" => Some(Modifiers::FN),
            "cmd" | "super" | "win" => Some(Modifiers::COMMAND),
            "secondary" if cfg!(target_os = "macos") => Some(Modifiers::COMMAND),
            "secondary" => Some(Modifiers::CTRL),
            _ => None,
        };
        let (head, last) = source.rsplit_once('-').unwrap_or(("", source));
        let mut held = Modifiers::NONE;
        if source.contains('-') {
            for word in head.split('-') {
                held = held.plus(modifier(&word.to_ascii_lowercase())?);
            }
        }
        let name = match last {
            "" if !source.is_empty() => "-".to_owned(),
            _ if modifier(&last.to_ascii_lowercase()).is_some() => return None,
            _ => {
                if last.len() == 1 && last.as_bytes()[0].is_ascii_uppercase() {
                    held = held.plus(Modifiers::SHIFT);
                }
                last.to_ascii_lowercase()
            }
        };
        let text =
            (name.chars().count() == 1 && !held.ctrl() && !held.command()).then(|| {
                match held.shift() {
                    true => name.to_uppercase(),
                    false => name.clone(),
                }
            });
        Some((Self { name, text }, held))
    }
}

/// Numbered like the DOM `button` field.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum Button {
    Left,
    Middle,
    Right,
    Back,
    Forward,
}

/// `None` is a wheel without gestures.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum WheelPhase {
    #[default]
    None,
    Began,
    Changed,
    Ended,
    Momentum,
    MomentumEnded,
}

/// Positions are in window pixels from the top left of the content.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    KeyDown {
        key: Key,
        modifiers: Modifiers,
        repeat: bool,
        // An input method takes the key. The page sees it as `Process`.
        ime: bool,
    },
    KeyUp {
        key: Key,
        modifiers: Modifiers,
        ime: bool,
    },
    ModifiersChanged {
        modifiers: Modifiers,
    },
    PointerDown {
        at: Point<Pixels>,
        button: Button,
        modifiers: Modifiers,
        clicks: usize,
    },
    PointerUp {
        at: Point<Pixels>,
        button: Button,
        modifiers: Modifiers,
        clicks: usize,
    },
    PointerMove {
        at: Point<Pixels>,
        held: Option<Button>,
        modifiers: Modifiers,
    },
    PointerLeave {
        at: Point<Pixels>,
    },
    // `delta` is in lines when `lines` is set, else in pixels from a precise device like a trackpad.
    Wheel {
        at: Point<Pixels>,
        delta: Point<f32>,
        lines: bool,
        phase: WheelPhase,
        modifiers: Modifiers,
    },
    FileEnter {
        at: Point<Pixels>,
        paths: Vec<PathBuf>,
    },
    FileOver {
        at: Point<Pixels>,
    },
    FileDrop {
        at: Point<Pixels>,
    },
    FileLeave,
}

impl Event {
    pub fn at(&self) -> Option<Point<Pixels>> {
        let (Self::PointerDown { at, .. }
        | Self::PointerUp { at, .. }
        | Self::PointerMove { at, .. }
        | Self::PointerLeave { at }
        | Self::Wheel { at, .. }
        | Self::FileEnter { at, .. }
        | Self::FileOver { at }
        | Self::FileDrop { at }) = self
        else {
            return None;
        };
        Some(*at)
    }

    pub fn modifiers(&self) -> Option<Modifiers> {
        match self {
            Self::KeyDown { modifiers, .. }
            | Self::KeyUp { modifiers, .. }
            | Self::ModifiersChanged { modifiers, .. }
            | Self::PointerDown { modifiers, .. }
            | Self::PointerUp { modifiers, .. }
            | Self::PointerMove { modifiers, .. }
            | Self::Wheel { modifiers, .. } => Some(*modifiers),
            _ => None,
        }
    }

    pub(crate) fn targets_regions(&self) -> bool {
        matches!(
            self,
            Self::PointerDown { .. }
                | Self::PointerUp { .. }
                | Self::PointerMove { .. }
                | Self::PointerLeave { .. }
                | Self::Wheel { .. }
        )
    }

    // Window pixels become page pixels at this zoom.
    pub(crate) fn unzoom(&mut self, zoom: f32) {
        if let Self::Wheel {
            delta,
            lines: false,
            ..
        } = self
        {
            *delta = delta.map(|value| value / zoom);
        }
        if let Self::PointerDown { at, .. }
        | Self::PointerUp { at, .. }
        | Self::PointerMove { at, .. }
        | Self::PointerLeave { at }
        | Self::Wheel { at, .. }
        | Self::FileEnter { at, .. }
        | Self::FileOver { at }
        | Self::FileDrop { at } = self
        {
            *at = *at / zoom;
        }
    }
}

/// A selection in UTF-16 offsets. `reversed` when it was made backward, so its focus is at the start.
#[derive(Clone, Debug, PartialEq)]
pub struct Utf16Selection {
    pub range: Range<usize>,
    pub reversed: bool,
}

#[cfg(test)]
mod tests {
    use super::{Key, Modifiers};

    fn parse(source: &str) -> Option<(String, Option<String>, Modifiers)> {
        Key::parse(source).map(|(key, held)| (key.name, key.text, held))
    }

    #[test]
    fn accelerators_give_the_key_its_text_and_modifiers() {
        let key = |name: &str, text: Option<&str>, held| {
            Some((name.to_owned(), text.map(str::to_owned), held))
        };
        assert_eq!(parse("a"), key("a", Some("a"), Modifiers::NONE));
        assert_eq!(parse("A"), key("a", Some("A"), Modifiers::SHIFT));
        assert_eq!(
            parse("Shift-Alt-x"),
            key("x", Some("X"), Modifiers::SHIFT.plus(Modifiers::ALT))
        );
        assert_eq!(parse("cmd-s"), key("s", None, Modifiers::COMMAND));
        assert_eq!(parse("super-s"), parse("win-s"));
        assert_eq!(parse("ctrl-"), key("-", None, Modifiers::CTRL));
        assert_eq!(parse("alt-"), key("-", Some("-"), Modifiers::ALT));
        assert_eq!(parse("fn-f12"), key("f12", None, Modifiers::FN));
        assert_eq!(parse("enter"), key("enter", None, Modifiers::NONE));
        let secondary = if cfg!(target_os = "macos") {
            Modifiers::COMMAND
        } else {
            Modifiers::CTRL
        };
        assert_eq!(parse("secondary-c"), key("c", None, secondary));
        for bad in ["ctrl", "a-b", "-", "ctrl--", "ctrl--a", "shift-SHIFT"] {
            assert_eq!(parse(bad), None, "{bad}");
        }
        assert_eq!(parse(""), key("", None, Modifiers::NONE));
    }

    #[test]
    fn modifier_bits_combine_and_clear() {
        let held = Modifiers::CTRL.plus(Modifiers::SHIFT);
        assert!(held.ctrl() && held.shift() && !held.alt() && !held.command());
        assert!(held.has(Modifiers::CTRL) && !held.has(Modifiers::CTRL.plus(Modifiers::ALT)));
        assert_eq!(held.with(Modifiers::CTRL, false), Modifiers::SHIFT);
        assert_eq!(Modifiers::default(), Modifiers::NONE);
    }
}
