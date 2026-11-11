use crate::{Button, Event, Key, Modifiers, Pixels, WheelPhase, point, px};
use core_foundation_sys::{
    data::{CFDataGetBytePtr, CFDataRef},
    string::CFStringRef,
};
use objc2::{msg_send, runtime::AnyObject};
use objc2_app_kit::{
    self as kit, NSEvent, NSEventModifierFlags as Flags, NSEventPhase as Phase, NSEventType as Kind,
};
use std::ffi::c_void;

// Modifier bits of UCKeyTranslate: the Carbon modifier mask shifted right by 8.
const UC_COMMAND: u32 = 1;
const UC_SHIFT: u32 = 2;
const UC_OPTION: u32 = 8;

fn held(flags: Flags) -> Modifiers {
    [
        (Flags::Control, Modifiers::CTRL),
        (Flags::Option, Modifiers::ALT),
        (Flags::Shift, Modifiers::SHIFT),
        (Flags::Command, Modifiers::COMMAND),
        (Flags::Function, Modifiers::FN),
    ]
    .into_iter()
    .fold(Modifiers::NONE, |held, (flag, bit)| {
        held.with(bit, flags.contains(flag))
    })
}

// AppKit numbers buttons left, right, middle, back, forward.
fn button(number: isize) -> Option<Button> {
    [
        Button::Left,
        Button::Right,
        Button::Middle,
        Button::Back,
        Button::Forward,
    ]
    .get(usize::try_from(number).ok()?)
    .copied()
}

/// Positions are from the top of a content view `height` points tall.
pub(crate) fn translate(native: &NSEvent, height: Pixels) -> Option<Event> {
    let kind = native.r#type();
    // Read flags and locations only for the kinds below.
    let flags = || native.modifierFlags();
    let modifiers = || held(flags());
    let at = || {
        let location = native.locationInWindow();
        point(px(location.x as f32), height - px(location.y as f32))
    };
    Some(match kind {
        Kind::FlagsChanged => Event::ModifiersChanged {
            modifiers: modifiers(),
            caps_lock: flags().contains(Flags::CapsLock),
        },
        Kind::KeyDown | Kind::KeyUp => {
            let (key, modifiers) = key(native, flags());
            match kind {
                Kind::KeyDown => Event::KeyDown {
                    key,
                    modifiers,
                    repeat: native.isARepeat(),
                    ime: false,
                },
                _ => Event::KeyUp {
                    key,
                    modifiers,
                    ime: false,
                },
            }
        }
        Kind::LeftMouseDown | Kind::RightMouseDown | Kind::OtherMouseDown => Event::PointerDown {
            at: at(),
            button: button(native.buttonNumber())?,
            modifiers: modifiers(),
            clicks: native.clickCount() as usize,
        },
        Kind::LeftMouseUp | Kind::RightMouseUp | Kind::OtherMouseUp => Event::PointerUp {
            at: at(),
            button: button(native.buttonNumber())?,
            modifiers: modifiers(),
            clicks: native.clickCount() as usize,
        },
        Kind::LeftMouseDragged | Kind::RightMouseDragged | Kind::OtherMouseDragged => {
            Event::PointerMove {
                at: at(),
                held: Some(button(native.buttonNumber())?),
                modifiers: modifiers(),
            }
        }
        Kind::MouseMoved => Event::PointerMove {
            at: at(),
            held: None,
            modifiers: modifiers(),
        },
        Kind::MouseExited => Event::PointerLeave { at: at() },
        Kind::Swipe if native.phase() == Phase::Ended => {
            let dx = native.deltaX();
            Event::PointerDown {
                at: at(),
                button: match dx {
                    _ if dx > 0. => Button::Back,
                    _ if dx < 0. => Button::Forward,
                    _ => return None,
                },
                modifiers: modifiers(),
                clicks: 1,
            }
        }
        Kind::ScrollWheel => {
            let (fingers, momentum) = (native.phase(), native.momentumPhase());
            let over = Phase::Ended | Phase::Cancelled;
            let phase = if !momentum.is_empty() {
                match momentum.intersects(over) {
                    true => WheelPhase::MomentumEnded,
                    false => WheelPhase::Momentum,
                }
            } else if fingers.intersects(Phase::Began | Phase::MayBegin) {
                WheelPhase::Began
            } else if fingers.intersects(over) {
                WheelPhase::Ended
            } else if fingers.is_empty() {
                WheelPhase::None
            } else {
                WheelPhase::Changed
            };
            Event::Wheel {
                at: at(),
                delta: point(
                    native.scrollingDeltaX() as f32,
                    native.scrollingDeltaY() as f32,
                ),
                lines: !native.hasPreciseScrollingDeltas(),
                phase,
                modifiers: modifiers(),
            }
        }
        _ => return None,
    })
}

// Names keys by the character they type without modifiers, so shortcuts work across layouts. Shift stays held
// only on letters: `cmd-shift-7` on a US layout becomes `cmd-&`.
#[allow(non_upper_case_globals)]
fn key(native: &NSEvent, flags: Flags) -> (Key, Modifiers) {
    let mut modifiers = held(flags);
    let typed = native
        .charactersIgnoringModifiers()
        .map(|characters| characters.to_string())
        .unwrap_or_default();
    let unit = typed.chars().next().map(|c| u32::from(c as u16));
    // Arrows and other function-key characters set the fn flag themselves.
    if unit.is_some_and(|unit| {
        (kit::NSUpArrowFunctionKey..=kit::NSModeSwitchFunctionKey).contains(&unit)
    }) {
        modifiers = modifiers.with(Modifiers::FN, false);
    }
    let named = |name: &str, text: Option<&str>| Key {
        name: name.to_owned(),
        text: text.map(str::to_owned),
    };
    let key = match unit {
        Some(0x20) => named("space", Some(" ")),
        Some(0x09) => named("tab", Some("\t")),
        // Return, and Enter on the keypad.
        Some(0x0d | 0x03) => named("enter", Some("\n")),
        Some(0x7f) => named("backspace", None),
        Some(0x1b) => named("escape", None),
        // Shift-tab.
        Some(0x19) => named("tab", None),
        Some(kit::NSUpArrowFunctionKey) => named("up", None),
        Some(kit::NSDownArrowFunctionKey) => named("down", None),
        Some(kit::NSLeftArrowFunctionKey) => named("left", None),
        Some(kit::NSRightArrowFunctionKey) => named("right", None),
        Some(kit::NSPageUpFunctionKey) => named("pageup", None),
        Some(kit::NSPageDownFunctionKey) => named("pagedown", None),
        Some(kit::NSHomeFunctionKey) => named("home", None),
        Some(kit::NSEndFunctionKey) => named("end", None),
        Some(kit::NSDeleteFunctionKey) => named("delete", None),
        // The Insert key sends kit::NSHelpFunctionKey, not NSInsertFunctionKey.
        Some(kit::NSHelpFunctionKey) => named("insert", None),
        Some(unit @ kit::NSF1FunctionKey..=kit::NSF35FunctionKey) => Key {
            name: format!("f{}", unit - kit::NSF1FunctionKey + 1),
            text: None,
        },
        _ => return by_layout(native.keyCode(), modifiers),
    };
    (key, modifiers)
}

// WARNING: key names depend on the layout. A key pressed with cmd, and any key of a layout that types no
// ASCII letters, takes its name from the cmd layer. Shift stays in the modifiers only for a-z. When changing
// this, check a non-Latin layout, a layout that switches with cmd, and layouts with digits on the shift layer.
fn by_layout(code: u16, mut modifiers: Modifiers) -> (Key, Modifiers) {
    let layout = Layout::current();
    let chars = |modifiers| {
        layout
            .as_ref()
            .map_or_else(String::new, |layout| layout.chars(code, modifiers))
    };
    let command = modifiers.command();
    // Layouts that type no ASCII letters use their cmd layer for shortcuts. Key code 0 is A on QWERTY.
    let latin = || {
        layout.as_ref().is_none_or(|layout| {
            layout.chars(0, 0).is_ascii() || !layout.chars(0, UC_COMMAND).is_ascii()
        })
    };
    let (mut base, mut shifted) = (chars(0), chars(UC_SHIFT));
    if command || !latin() {
        let with_command = chars(UC_COMMAND);
        let with_both = chars(UC_COMMAND | UC_SHIFT);
        if with_both != with_command {
            shifted = with_both;
        } else if with_command.to_ascii_uppercase() != with_command {
            shifted = with_command.to_ascii_uppercase();
        }
        base = with_command;
    }
    let text = (!modifiers.ctrl() && !command && !modifiers.function()).then(|| {
        let shift = if modifiers.shift() { UC_SHIFT } else { 0 };
        let option = if modifiers.alt() { UC_OPTION } else { 0 };
        chars(shift | option)
    });
    let name = if !modifiers.shift() || base.chars().all(|c| c.is_ascii_lowercase()) {
        base
    } else {
        modifiers = modifiers.with(Modifiers::SHIFT, false);
        shifted
    };
    (Key { name, text }, modifiers)
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentKeyboardLayoutInputSource() -> *mut AnyObject;
    pub(super) fn TISGetInputSourceProperty(
        source: *mut AnyObject,
        key: *const c_void,
    ) -> *mut AnyObject;
    static kTISPropertyUnicodeKeyLayoutData: CFStringRef;
    fn LMGetKbdType() -> u16;
    fn UCKeyTranslate(
        layout: *const c_void,
        code: u16,
        action: u16,
        modifiers: u32,
        keyboard_type: u32,
        options: u32,
        dead_key_state: *mut u32,
        max_length: usize,
        length: *mut usize,
        string: *mut u16,
    ) -> u32;
}

struct Layout {
    source: *mut AnyObject,
    data: *const c_void,
    keyboard: u32,
}

impl Layout {
    fn current() -> Option<Self> {
        // SAFETY: plain Text Input Sources calls. The source is released in drop.
        unsafe {
            let source = TISCopyCurrentKeyboardLayoutInputSource();
            if source.is_null() {
                return None;
            }
            let data = TISGetInputSourceProperty(
                source,
                kTISPropertyUnicodeKeyLayoutData as *const c_void,
            ) as CFDataRef;
            if data.is_null() {
                let () = msg_send![source, release];
                return None;
            }
            Some(Self {
                source,
                data: CFDataGetBytePtr(data).cast(),
                keyboard: u32::from(LMGetKbdType()),
            })
        }
    }

    // A dead key types itself, as it does before a space.
    fn chars(&self, code: u16, modifiers: u32) -> String {
        const SPACE: u16 = 49;
        let (mut dead, mut length, mut buffer) = (0, 0, [0u16; 4]);
        for code in [code, SPACE] {
            // SAFETY: the layout data lives as long as its source, and the buffer holds `buffer.len()` units.
            unsafe {
                UCKeyTranslate(
                    self.data,
                    code,
                    0,
                    modifiers,
                    self.keyboard,
                    0,
                    &mut dead,
                    buffer.len(),
                    &mut length,
                    buffer.as_mut_ptr(),
                );
            }
            if dead == 0 {
                break;
            }
        }
        String::from_utf16(&buffer[..length]).unwrap_or_default()
    }
}

impl Drop for Layout {
    fn drop(&mut self) {
        // SAFETY: the source came from a Copy call.
        unsafe {
            let () = msg_send![self.source, release];
        }
    }
}
