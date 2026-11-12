use crate::{Event, Key, Modifiers};
use std::cell::Cell;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Input::Ime::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;

pub(super) type Seen = Cell<Option<(Modifiers, bool)>>;

fn down(key: VIRTUAL_KEY) -> bool {
    // SAFETY: a plain key state query.
    unsafe { GetKeyState(i32::from(key.0)) < 0 }
}

pub(super) fn held() -> Modifiers {
    Modifiers::NONE
        .with(Modifiers::CTRL, down(VK_CONTROL))
        .with(Modifiers::ALT, down(VK_MENU))
        .with(Modifiers::SHIFT, down(VK_SHIFT))
        .with(Modifiers::COMMAND, down(VK_LWIN) || down(VK_RWIN))
}

pub(super) fn caps_lock() -> bool {
    // SAFETY: a plain key state query. The low bit is the toggle.
    unsafe { GetKeyState(i32::from(VK_CAPITAL.0)) & 1 != 0 }
}

pub(super) fn key(seen: &Seen, wparam: WPARAM, lparam: LPARAM, pressed: bool) -> Option<Event> {
    let scan = (lparam.0 as u32 >> 16) & 0xFF;
    let mut code = VIRTUAL_KEY(wparam.0 as u16);
    // A key that the input method takes arrives as VK_PROCESSKEY, and its scan code still tells which key.
    let ime = code == VK_PROCESSKEY;
    if ime {
        let extended = if lparam.0 & (1 << 24) != 0 { 0xE000 } else { 0 };
        // SAFETY: a plain keyboard layout query.
        code = VIRTUAL_KEY(unsafe { MapVirtualKeyW(scan | extended, MAPVK_VSC_TO_VK_EX) } as u16);
    }
    let mut modifiers = held();
    if MODIFIER_KEYS.contains(&code) {
        let now = (modifiers, caps_lock());
        if seen.replace(Some(now)) == Some(now) {
            return None;
        }
        return Some(Event::ModifiersChanged { modifiers: now.0 });
    }
    if code == VK_PACKET {
        return None;
    }
    let text = (pressed && !ime)
        .then(|| {
            let mut state = [0u8; 256];
            // SAFETY: fills a local buffer of the documented size.
            unsafe { GetKeyboardState(&mut state) }.ok()?;
            typed(code, scan, &state)
        })
        .flatten();
    let name = if let Some(&(_, name)) = NAMES.iter().find(|(key, _)| *key == code) {
        Some(name.to_owned())
    } else if (VK_F1.0..=VK_F24.0).contains(&code.0) {
        Some(format!("f{}", code.0 - VK_F1.0 + 1))
    } else if modifiers.shift() && !ime && symbol(code) {
        // Shifted symbols go by what they type, without the shift: shift-1 on a US layout is `!`.
        let mut state = [0u8; 256];
        state[usize::from(VK_SHIFT.0)] = 0x80;
        let shifted = typed(code, scan, &state);
        if shifted.is_some() {
            modifiers = modifiers.with(Modifiers::SHIFT, false);
        }
        shifted
    } else {
        // SAFETY: a plain keyboard layout query.
        let unit = unsafe { MapVirtualKeyW(u32::from(code.0), MAPVK_VK_TO_CHAR) } & 0xFFFF;
        char::from_u32(unit)
            .filter(|&c| c != '\0')
            .map(|c| c.to_ascii_lowercase().to_string())
    };
    let key = Key {
        name: match name {
            Some(name) => name,
            None if ime => String::new(),
            None => return None,
        },
        text,
    };
    Some(match pressed {
        true => Event::KeyDown {
            key,
            modifiers,
            repeat: lparam.0 & (1 << 30) != 0,
            ime,
        },
        false => Event::KeyUp {
            key,
            modifiers,
            ime,
        },
    })
}

// A key that the input method takes maps to the left or right code, so both forms are listed.
const MODIFIER_KEYS: [VIRTUAL_KEY; 12] = [
    VK_SHIFT,
    VK_LSHIFT,
    VK_RSHIFT,
    VK_CONTROL,
    VK_LCONTROL,
    VK_RCONTROL,
    VK_MENU,
    VK_LMENU,
    VK_RMENU,
    VK_LWIN,
    VK_RWIN,
    VK_CAPITAL,
];

const NAMES: [(VIRTUAL_KEY, &str); 18] = [
    (VK_SPACE, "space"),
    (VK_BACK, "backspace"),
    (VK_RETURN, "enter"),
    (VK_TAB, "tab"),
    (VK_ESCAPE, "escape"),
    (VK_UP, "up"),
    (VK_DOWN, "down"),
    (VK_LEFT, "left"),
    (VK_RIGHT, "right"),
    (VK_HOME, "home"),
    (VK_END, "end"),
    (VK_PRIOR, "pageup"),
    (VK_NEXT, "pagedown"),
    (VK_INSERT, "insert"),
    (VK_DELETE, "delete"),
    (VK_APPS, "menu"),
    (VK_BROWSER_BACK, "back"),
    (VK_BROWSER_FORWARD, "forward"),
];

fn symbol(code: VIRTUAL_KEY) -> bool {
    (VK_0.0..=VK_9.0).contains(&code.0)
        || matches!(
            code,
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
        )
}

// Control characters type nothing.
fn typed(code: VIRTUAL_KEY, scan: u32, state: &[u8; 256]) -> Option<String> {
    let mut units = [0u16; 8];
    // SAFETY: the buffers are local. Flag 4 leaves the dead key state of the keyboard as it is.
    let count = unsafe { ToUnicode(u32::from(code.0), scan, Some(state), &mut units, 4) };
    let text = String::from_utf16(&units[..count.unsigned_abs() as usize]).ok()?;
    text.chars()
        .next()
        .is_some_and(|c| !c.is_control())
        .then_some(text)
}

/// In UTF-16.
pub(super) fn composition_text(context: HIMC, which: IME_COMPOSITION_STRING) -> Option<Vec<u16>> {
    // SAFETY: the first call returns the size in bytes, and the second fills a buffer of that size.
    unsafe {
        let bytes = u32::try_from(ImmGetCompositionStringW(context, which, None, 0)).ok()?;
        let mut units = vec![0u16; bytes as usize / 2];
        ImmGetCompositionStringW(context, which, Some(units.as_mut_ptr().cast()), bytes);
        Some(units)
    }
}

pub(super) fn caret(context: HIMC, len: usize) -> usize {
    // SAFETY: plain input method queries. The attributes fill a local buffer of the size asked for.
    unsafe {
        let cursor = ImmGetCompositionStringW(context, GCS_CURSORPOS, None, 0).max(0) as usize;
        let Ok(size) = usize::try_from(ImmGetCompositionStringW(context, GCS_COMPATTR, None, 0))
        else {
            return len;
        };
        let mut marks = vec![0u8; size];
        if size == 0
            || ImmGetCompositionStringW(
                context,
                GCS_COMPATTR,
                Some(marks.as_mut_ptr().cast()),
                size as u32,
            ) <= 0
        {
            return len;
        }
        let typing = |at: usize| marks.get(at) == Some(&(ATTR_INPUT as u8));
        if typing(cursor) || cursor.checked_sub(1).is_some_and(typing) {
            cursor
        } else {
            len
        }
    }
}
