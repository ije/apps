use crate::{Button, Cursor, Event, Key, Modifiers, Pixels, Point, Rect, app::Callbacks, px};
use anyhow::{Result, bail, ensure};
use std::io::{ErrorKind::Interrupted, Read as _};
use std::os::{fd::AsRawFd as _, unix::ffi::OsStringExt as _};
use std::{path::PathBuf, time::Duration, time::Instant};
use xkbcommon::xkb::{self, Keycode, Keysym, Status as Step, compose};

pub(super) const NOTCH_LINES: f32 = 3.;

#[derive(Default)]
pub(super) struct Clicks {
    last: Option<(Button, Instant, Point<Pixels>)>,
    pub count: usize,
}

impl Clicks {
    pub fn press(&mut self, button: Button, at: Point<Pixels>) -> usize {
        let now = Instant::now();
        let near = |last: Point<Pixels>| {
            (last.x - at.x).abs() <= px(5.) && (last.y - at.y).abs() <= px(5.)
        };
        self.count = match self.last {
            Some((kept, when, place))
                if kept == button && now - when < Duration::from_millis(400) && near(place) =>
            {
                self.count + 1
            }
            _ => 1,
        };
        self.last = Some((button, now, at));
        self.count
    }
}

pub(super) enum Ime {
    Commit(String),
    Compose(String),
    Clear,
    End,
}

impl Ime {
    pub fn apply(self, callbacks: Callbacks) {
        match self {
            Self::Commit(text) => callbacks.insert(None, &text),
            Self::Compose(text) if !text.is_empty() => callbacks.compose(None, &text, None),
            Self::Compose(_) | Self::Clear => callbacks.clear_composition(),
            Self::End => callbacks.end_composition(),
        }
    }
}

pub(super) fn caret(callbacks: Callbacks) -> Option<Rect> {
    callbacks.rect_of(callbacks.selection()?.range)
}

/// The key filter of main goes first, then the page, then the input changes. A key that nobody took types its text.
pub(super) fn press(
    callbacks: Callbacks,
    key: Key,
    modifiers: Modifiers,
    repeat: bool,
    changes: Vec<Ime>,
) {
    let typed = key.text.clone().filter(|_| {
        !modifiers.ctrl() && !modifiers.alt() && !modifiers.command() && !modifiers.function()
    });
    let event = Event::KeyDown {
        key,
        modifiers,
        repeat,
        ime: !changes.is_empty(),
    };
    let composing = || callbacks.composition().is_some();
    let taken = crate::native::key(callbacks.id, &event, &composing) || callbacks.input(event);
    for change in changes {
        change.apply(callbacks);
    }
    if !taken && let Some(text) = typed {
        callbacks.insert(None, &text);
    }
}

pub(super) fn release(callbacks: Callbacks, key: Key, modifiers: Modifiers) {
    let event = Event::KeyUp {
        key,
        modifiers,
        ime: false,
    };
    let composing = || callbacks.composition().is_some();
    if !crate::native::key(callbacks.id, &event, &composing) {
        callbacks.input(event);
    }
}

pub(super) fn xkb_context() -> Result<xkb::Context> {
    let context = xkb::Context::new(xkb::CONTEXT_NO_DEFAULT_INCLUDES);
    ensure!(
        !context.get_raw_ptr().is_null(),
        "libxkbcommon has no context"
    );
    Ok(context)
}

pub(super) fn held(state: &xkb::State) -> (Modifiers, bool) {
    let on = |name| state.mod_name_is_active(name, xkb::STATE_MODS_EFFECTIVE);
    let modifiers = Modifiers::NONE
        .with(Modifiers::CTRL, on(xkb::MOD_NAME_CTRL))
        .with(Modifiers::ALT, on(xkb::MOD_NAME_ALT))
        .with(Modifiers::SHIFT, on(xkb::MOD_NAME_SHIFT))
        .with(Modifiers::COMMAND, on(xkb::MOD_NAME_LOGO));
    (modifiers, on(xkb::MOD_NAME_CAPS))
}

// What a pending dead key shows, from `dead_grave` (0xfe50) to `dead_belowcomma` (0xfe6e).
const DEAD: &str = "`\u{b4}^~\u{af}\u{2d8}\u{2d9}\u{a8}\u{2da}\u{2dd}\u{2c7}\u{b8}\u{2db}\u{37a}\u{309b}\u{309c}\
    \u{323}\u{309}\u{31b}\u{335}\u{313}\u{314}\u{30f}\u{2f3}\u{2cd}\u{a788}\u{2f7}\u{32e}\u{324}\u{32f}\u{326}";

fn dead(sym: Keysym) -> Option<String> {
    match sym.raw() {
        0xfe8a => Some("ə".into()),
        0xfe8b => Some("Ə".into()),
        raw => Some(DEAD.chars().nth(raw.checked_sub(0xfe50)? as usize)?.into()),
    }
}

// Rows of a US keyboard by evdev key code, for shortcuts on layouts without Latin letters.
fn qwerty(code: u32, shift: bool) -> Option<char> {
    let rows: [(u32, &str, &str); 3] = [
        (24, "qwertyuiop[]", "qwertyuiop{}"),
        (38, "asdfghjkl;'`", "asdfghjkl:\"~"),
        (51, "\\zxcvbnm,./", "|zxcvbnm<>?"),
    ];
    rows.iter().find_map(|(first, plain, shifted)| {
        let row = if shift { shifted } else { plain };
        row.chars().nth(code.checked_sub(*first)? as usize)
    })
}

/// The returned modifiers drop a shift that the key name already holds.
pub(super) fn key(state: &xkb::State, code: Keycode, mut modifiers: Modifiers) -> (Key, Modifiers) {
    let sym = state.key_get_one_sym(code);
    let symbol = xkb::keysym_get_name(sym).to_lowercase();
    let unshifted =
        |sym: Keysym| char::from_u32(xkb::keysym_to_utf32(sym)).filter(char::is_ascii_graphic);
    let alias = |name: &str| match name {
        "return" | "enter" => "enter".to_owned(),
        "prior" | "page_up" => "pageup".to_owned(),
        "next" | "page_down" => "pagedown".to_owned(),
        "iso_left_tab" => "tab".to_owned(),
        _ => name.to_owned(),
    };
    let name = if let Some(rest) = symbol.strip_prefix("kp_") {
        alias(rest)
    } else if let Some(rest) = symbol.strip_prefix("xf86") {
        rest.to_owned()
    } else if let Some(ch) = unshifted(sym) {
        ch.to_ascii_lowercase().to_string()
    } else if xkb::keysym_to_utf32(sym) > 0x7f {
        // Another layout of the keymap can have the Latin letter, else the US position names it.
        let keymap = state.get_keymap();
        (0..keymap.num_layouts_for_key(code))
            .flat_map(|layout| {
                keymap
                    .key_get_syms_by_level(code, layout, 0)
                    .iter()
                    .copied()
            })
            .find_map(unshifted)
            .or_else(|| qwerty(code.raw(), modifiers.shift()))
            .map_or(symbol, |ch| ch.to_ascii_lowercase().to_string())
    } else {
        alias(&symbol)
    };
    if modifiers.shift() && name.chars().count() == 1 && name.to_lowercase() == name.to_uppercase()
    {
        modifiers = modifiers.with(Modifiers::SHIFT, false);
    }
    let code_point = state.key_get_utf32(code);
    let text =
        (code_point >= 0x20 && code_point != 0x7f && !modifiers.ctrl() && !modifiers.command())
            .then(|| state.key_get_utf8(code))
            .filter(|text| !text.is_empty());
    (Key { name, text }, modifiers)
}

pub(super) struct Composer {
    state: Option<compose::State>,
    pending: Option<String>,
}

impl Composer {
    pub fn new(context: &xkb::Context) -> Self {
        let locale = std::env::var_os("LC_ALL")
            .or_else(|| std::env::var_os("LC_CTYPE"))
            .or_else(|| std::env::var_os("LANG"));
        let state = [locale.as_deref(), Some("C".as_ref())]
            .into_iter()
            .flatten()
            .find_map(|locale| {
                compose::Table::new_from_locale(context, locale, compose::COMPILE_NO_FLAGS).ok()
            })
            .map(|table| compose::State::new(&table, compose::STATE_NO_FLAGS));
        Self {
            state,
            pending: None,
        }
    }

    pub fn reset(&mut self) -> Option<String> {
        self.state.iter_mut().for_each(compose::State::reset);
        self.pending.take()
    }

    /// The key loses its text when it is part of a sequence.
    pub fn feed(&mut self, sym: Keysym, key: &mut Key) -> (Option<String>, Vec<Ime>) {
        let Some(state) = &mut self.state else {
            return (None, Vec::new());
        };
        state.feed(sym);
        match state.status() {
            Step::Composing => {
                key.text = None;
                let shown = state.utf8().or_else(|| dead(sym));
                self.pending = shown.clone();
                (None, vec![Ime::Compose(shown.unwrap_or_default())])
            }
            Step::Composed => {
                let text = state.utf8();
                if self.pending.take().is_some() {
                    key.text = None;
                    return (None, vec![text.map_or(Ime::Clear, Ime::Commit)]);
                }
                if let Some(sym) = state.keysym() {
                    key.name = xkb::keysym_get_name(sym);
                }
                key.text = text;
                (None, Vec::new())
            }
            Step::Cancelled => {
                // The key that broke the sequence starts the next one.
                let before = self.pending.take();
                state.feed(sym);
                self.pending = dead(sym);
                match &self.pending {
                    None => (before, Vec::new()),
                    Some(shown) => {
                        key.text = None;
                        let mut changes: Vec<_> = before.into_iter().map(Ime::Commit).collect();
                        changes.push(Ime::Compose(shown.clone()));
                        (None, changes)
                    }
                }
            }
            _ => (None, Vec::new()),
        }
    }
}

/// Space separated cursor names: the CSS name first, which cursor themes and the cursor shape protocol share.
pub(super) fn cursor_names(cursor: Cursor) -> &'static str {
    match cursor {
        Cursor::Default | Cursor::None => "default left_ptr",
        Cursor::Text => "text xterm",
        Cursor::VerticalText => "vertical-text",
        Cursor::Pointer => "pointer hand2 hand",
        Cursor::Crosshair => "crosshair cross",
        Cursor::Grab => "grab openhand hand1",
        Cursor::Grabbing => "grabbing closedhand hand2",
        Cursor::NotAllowed => "not-allowed crossed_circle",
        Cursor::ColResize => "col-resize sb_h_double_arrow",
        Cursor::RowResize => "row-resize sb_v_double_arrow",
        Cursor::EwResize => "ew-resize sb_h_double_arrow",
        Cursor::NsResize => "ns-resize sb_v_double_arrow",
        Cursor::EResize => "e-resize right_side",
        Cursor::WResize => "w-resize left_side",
        Cursor::NResize => "n-resize top_side",
        Cursor::SResize => "s-resize bottom_side",
        Cursor::NwseResize => "nwse-resize size_fdiag bd_double_arrow",
        Cursor::NeswResize => "nesw-resize size_bdiag fd_double_arrow",
        Cursor::Alias => "alias",
        Cursor::Copy => "copy",
        Cursor::ContextMenu => "context-menu",
    }
}

pub(super) fn uri_paths(list: &str) -> Vec<PathBuf> {
    list.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("file://")?;
            let path = &rest.as_bytes()[rest.find('/')?..];
            let mut bytes = Vec::with_capacity(path.len());
            let mut at = 0;
            while let Some(&byte) = path.get(at) {
                let escaped = path
                    .get(at + 1..at + 3)
                    .filter(|_| byte == b'%')
                    .and_then(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok());
                bytes.push(escaped.unwrap_or(byte));
                at += if escaped.is_some() { 3 } else { 1 };
            }
            Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
        })
        .collect()
}

/// Gives up when no data comes for 4 seconds.
pub(super) fn read_pipe(mut pipe: std::io::PipeReader) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    loop {
        let mut wait = libc::pollfd {
            fd: pipe.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd.
        let ready = unsafe { libc::poll(&mut wait, 1, 4000) };
        let failure = (ready < 0).then(std::io::Error::last_os_error);
        match failure {
            Some(failure) if failure.kind() == Interrupted => continue,
            Some(failure) => bail!("waiting for the pipe: {failure}"),
            None if ready == 0 => bail!("no data came through the pipe in 4 seconds"),
            None => {}
        }
        let start = data.len();
        data.resize(start + 4096, 0);
        let read = pipe.read(&mut data[start..]);
        data.truncate(start + *read.as_ref().unwrap_or(&0));
        match read {
            Ok(0) => return Ok(data),
            Err(failure) if failure.kind() != Interrupted => bail!("reading the pipe: {failure}"),
            _ => {}
        }
    }
}
