//! Keyboard and IME translation.

use crate::{
    Capslock, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, ModifiersChangedEvent, PlatformInput,
};
use windows::Win32::{
    Foundation::{LPARAM, WPARAM},
    UI::Input::{Ime::*, KeyboardAndMouse::*},
};

/// The last modifier state sent, so repeated modifier key downs send nothing.
#[derive(Default)]
pub(super) struct KeyState {
    pub modifiers: Option<Modifiers>,
    pub capslock: Option<Capslock>,
}

pub(super) fn modifiers() -> Modifiers {
    let pressed = |key: VIRTUAL_KEY| unsafe { GetKeyState(key.0 as i32) } < 0;
    Modifiers {
        control: pressed(VK_CONTROL),
        alt: pressed(VK_MENU),
        shift: pressed(VK_SHIFT),
        platform: pressed(VK_LWIN) || pressed(VK_RWIN),
        function: false,
    }
}

pub(super) fn capslock() -> Capslock {
    Capslock {
        // SAFETY: plain key state query.
        on: unsafe { GetKeyState(VK_CAPITAL.0 as i32) } & 1 != 0,
    }
}

/// The input of a key message, or `None` when it sends nothing.
pub(super) fn key_event(
    state: &mut KeyState,
    wparam: WPARAM,
    lparam: LPARAM,
    down: bool,
) -> Option<PlatformInput> {
    let vkey = VIRTUAL_KEY(wparam.0 as u16);
    let mut modifiers = modifiers();
    match vkey {
        VK_SHIFT | VK_CONTROL | VK_MENU | VK_LMENU | VK_RMENU | VK_LWIN | VK_RWIN | VK_CAPITAL => {
            let capslock = capslock();
            if state.modifiers == Some(modifiers) && state.capslock == Some(capslock) {
                return None;
            }
            state.modifiers = Some(modifiers);
            state.capslock = Some(capslock);
            Some(PlatformInput::ModifiersChanged(ModifiersChangedEvent {
                modifiers,
                capslock,
            }))
        }
        VK_PACKET => None,
        _ => {
            let scan_code = (lparam.0 >> 16) as u32 & 0xFF;
            let key_char = if down {
                key_char(vkey, scan_code)
            } else {
                None
            };
            let key = named_key(vkey).map(str::to_owned).or_else(|| {
                if modifiers.shift && shifts_to_symbol(vkey) {
                    // `shift-1` is sent as `!`.
                    let shifted = translate(vkey, scan_code, true);
                    if shifted.is_some() {
                        modifiers.shift = false;
                    }
                    shifted
                } else {
                    // SAFETY: plain keyboard layout query.
                    let char = unsafe { MapVirtualKeyW(vkey.0 as u32, MAPVK_VK_TO_CHAR) };
                    char::from_u32(char & 0xFFFF)
                        .filter(|char| *char != '\0')
                        .map(|char| char.to_ascii_lowercase().to_string())
                }
            })?;
            let keystroke = Keystroke {
                modifiers,
                key,
                key_char,
            };
            Some(if down {
                PlatformInput::KeyDown(KeyDownEvent {
                    keystroke,
                    is_held: lparam.0 & (1 << 30) != 0,
                })
            } else {
                PlatformInput::KeyUp(KeyUpEvent { keystroke })
            })
        }
    }
}

fn named_key(vkey: VIRTUAL_KEY) -> Option<&'static str> {
    Some(match vkey {
        VK_SPACE => "space",
        VK_BACK => "backspace",
        VK_RETURN => "enter",
        VK_TAB => "tab",
        VK_UP => "up",
        VK_DOWN => "down",
        VK_RIGHT => "right",
        VK_LEFT => "left",
        VK_HOME => "home",
        VK_END => "end",
        VK_PRIOR => "pageup",
        VK_NEXT => "pagedown",
        VK_BROWSER_BACK => "back",
        VK_BROWSER_FORWARD => "forward",
        VK_ESCAPE => "escape",
        VK_INSERT => "insert",
        VK_DELETE => "delete",
        VK_APPS => "menu",
        VK_F1 => "f1",
        VK_F2 => "f2",
        VK_F3 => "f3",
        VK_F4 => "f4",
        VK_F5 => "f5",
        VK_F6 => "f6",
        VK_F7 => "f7",
        VK_F8 => "f8",
        VK_F9 => "f9",
        VK_F10 => "f10",
        VK_F11 => "f11",
        VK_F12 => "f12",
        VK_F13 => "f13",
        VK_F14 => "f14",
        VK_F15 => "f15",
        VK_F16 => "f16",
        VK_F17 => "f17",
        VK_F18 => "f18",
        VK_F19 => "f19",
        VK_F20 => "f20",
        VK_F21 => "f21",
        VK_F22 => "f22",
        VK_F23 => "f23",
        VK_F24 => "f24",
        _ => return None,
    })
}

fn shifts_to_symbol(vkey: VIRTUAL_KEY) -> bool {
    matches!(
        vkey,
        VK_OEM_1
            | VK_OEM_2
            | VK_OEM_3
            | VK_OEM_4
            | VK_OEM_5
            | VK_OEM_6
            | VK_OEM_7
            | VK_OEM_8
            | VK_OEM_102
            | VK_OEM_PLUS
            | VK_OEM_MINUS
            | VK_OEM_COMMA
            | VK_OEM_PERIOD
            | VK_ABNT_C1
    ) || (VK_0.0..=VK_9.0).contains(&vkey.0)
}

/// The text of a key with only shift held, without changing dead key state.
fn translate(vkey: VIRTUAL_KEY, scan_code: u32, shift: bool) -> Option<String> {
    let mut state = [0u8; 256];
    if shift {
        state[VK_SHIFT.0 as usize] = 0x80;
    }
    let mut buffer = [0u16; 8];
    // SAFETY: flag 4 keeps the keyboard state; the buffer is local.
    let len = unsafe { ToUnicode(vkey.0 as u32, scan_code, Some(&state), &mut buffer, 0x4) };
    String::from_utf16(&buffer[..len.unsigned_abs() as usize])
        .ok()
        .filter(|text| text.chars().next().is_some_and(|char| !char.is_control()))
}

/// The text a key types.
fn key_char(vkey: VIRTUAL_KEY, scan_code: u32) -> Option<String> {
    let mut state = [0u8; 256];
    // SAFETY: plain keyboard state query into a local buffer.
    unsafe { GetKeyboardState(&mut state) }.ok()?;
    let mut buffer = [0u16; 8];
    // SAFETY: flag 4 keeps the keyboard state; the buffer is local.
    let len = unsafe { ToUnicode(vkey.0 as u32, scan_code, Some(&state), &mut buffer, 0x4) };
    String::from_utf16(&buffer[..len.unsigned_abs() as usize])
        .ok()
        .filter(|text| text.chars().next().is_some_and(|char| !char.is_control()))
}

/// A composition string of the IME.
pub(super) fn composition(context: HIMC, kind: IME_COMPOSITION_STRING) -> Option<Vec<u16>> {
    // SAFETY: the first call returns the byte length, the second fills a buffer of that size.
    unsafe {
        let len = ImmGetCompositionStringW(context, kind, None, 0);
        if len < 0 {
            return None;
        }
        let mut text = vec![0u16; len as usize / 2];
        ImmGetCompositionStringW(context, kind, Some(text.as_mut_ptr().cast()), len as u32);
        Some(text)
    }
}

/// The caret of the composition string, kept next to the text still being converted.
pub(super) fn composition_caret(context: HIMC, len: usize) -> usize {
    // SAFETY: plain IME queries into a local buffer.
    unsafe {
        let caret = ImmGetCompositionStringW(context, GCS_CURSORPOS, None, 0).max(0) as usize;
        let size = ImmGetCompositionStringW(context, GCS_COMPATTR, None, 0);
        if size <= 0 {
            return len;
        }
        let mut attributes = vec![0u8; size as usize];
        if ImmGetCompositionStringW(
            context,
            GCS_COMPATTR,
            Some(attributes.as_mut_ptr().cast()),
            size as u32,
        ) <= 0
        {
            return len;
        }
        let input = |index: usize| attributes.get(index) == Some(&(ATTR_INPUT as u8));
        if input(caret) || (caret > 0 && input(caret - 1)) {
            caret
        } else {
            len
        }
    }
}
