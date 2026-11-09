mod gpu;
mod text;
mod wayland;
mod x11;

use crate::{
    Bounds, Capslock, CursorStyle, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, MouseButton,
    Pixels, PlatformInput, Point, Scene, Size, WindowChange, WindowOptions, WindowState,
    app::Callbacks, px,
};
use anyhow::{Context as _, Result};
use calloop::{
    EventLoop, LoopHandle, LoopSignal,
    timer::{TimeoutAction, Timer},
};
use raw_window_handle as rwh;
use std::{
    cell::{OnceCell, RefCell},
    io::Read as _,
    os::fd::{AsFd as _, AsRawFd as _},
    path::PathBuf,
    rc::Rc,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use xkbcommon::xkb::{self, Keycode, Keysym};

pub(crate) use gpu::GpuAtlas;
pub(crate) use text::TextSystem;

type Posted = Box<dyn FnOnce() + Send>;

static POST: OnceLock<Mutex<calloop::channel::Sender<Posted>>> = OnceLock::new();

thread_local! {
    static LOOP: OnceCell<LoopHandle<'static, ()>> = const { OnceCell::new() };
}

const SCROLL_LINES: f32 = 3.;
const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(400);
const DOUBLE_CLICK_DISTANCE: Pixels = px(5.);
const DEFAULT_CURSOR: &str = "left_ptr";
const PIPE_READ_TIMEOUT: Duration = Duration::from_secs(4);
const RETRY_INTERVAL: Duration = Duration::from_micros(16_667);

#[derive(Clone)]
enum Client {
    X11(Rc<x11::Client>),
    Wayland(Rc<wayland::Client>),
}

pub(crate) struct Platform {
    event_loop: RefCell<EventLoop<'static, ()>>,
    signal: LoopSignal,
    client: Client,
}

impl Platform {
    pub fn new() -> Self {
        let event_loop = EventLoop::try_new().expect("creating the event loop");
        let handle = event_loop.handle();
        let (sender, channel) = calloop::channel::channel::<Posted>();
        handle
            .insert_source(channel, |event, _, _| {
                if let calloop::channel::Event::Msg(f) = event {
                    f();
                }
            })
            .expect("registering the main thread queue");
        let _ = POST.set(Mutex::new(sender));
        LOOP.with(|cell| cell.set(handle.clone()).ok());
        let wayland = std::env::var_os("WAYLAND_DISPLAY")
            .is_some_and(|display| !display.is_empty())
            || std::env::var_os("WAYLAND_SOCKET").is_some();
        let x11 = std::env::var_os("DISPLAY").is_some_and(|display| !display.is_empty());
        let mut errors = Vec::new();
        let mut client = None;
        if wayland {
            match wayland::Client::new(&handle, x11) {
                Ok(wayland) => client = Some(Client::Wayland(wayland)),
                Err(error) => errors.push(format!("{error:#}")),
            }
        }
        if client.is_none() && x11 {
            match x11::Client::new(&handle) {
                Ok(x11) => client = Some(Client::X11(x11)),
                Err(error) => errors.push(format!("{error:#}")),
            }
        }
        let Some(client) = client else {
            eprintln!("apps: no display: {}", errors.join("; "));
            std::process::exit(1);
        };
        Self {
            signal: event_loop.get_signal(),
            event_loop: RefCell::new(event_loop),
            client,
        }
    }

    pub fn run(&self, on_launch: Box<dyn FnOnce()>) {
        post_local(on_launch);
        let client = self.client.clone();
        let result = self.event_loop.borrow_mut().run(None, &mut (), |_| {
            // WARNING: requests with replies can read X events into xcb's queue, which doesn't wake the loop.
            if let Client::X11(client) = &client {
                client.process_events();
            }
        });
        if let Err(error) = result {
            eprintln!("apps: event loop: {error}");
        }
        crate::app::events::quit();
    }

    pub fn quit(&self) {
        // WARNING: quitting asks the app, which is busy while it calls this.
        let signal = self.signal.clone();
        post_local(move || {
            if crate::app::events::should_quit() {
                signal.stop();
                signal.wakeup();
            }
        });
    }

    pub fn activate(&self, _ignoring_other_apps: bool) {}

    pub fn read_clipboard(&self) -> Option<String> {
        match &self.client {
            Client::X11(client) => client.read_clipboard(),
            Client::Wayland(client) => client.read_clipboard(),
        }
    }

    pub fn write_clipboard(&self, text: &str) {
        match &self.client {
            Client::X11(client) => client.write_clipboard(text),
            Client::Wayland(client) => client.write_clipboard(text),
        }
    }

    pub fn set_cursor(&self, style: CursorStyle) {
        match &self.client {
            Client::X11(client) => client.set_cursor(style),
            Client::Wayland(client) => client.set_cursor(style),
        }
    }

    pub fn primary_display_bounds(&self) -> Option<Bounds<Pixels>> {
        match &self.client {
            Client::X11(client) => client.primary_display_bounds(),
            Client::Wayland(client) => client.primary_display_bounds(),
        }
    }

    pub fn open_window(&self, options: &WindowOptions, callbacks: Callbacks) -> Result<Window> {
        Ok(match &self.client {
            Client::X11(client) => Window::X11(x11::Window::open(client, options, callbacks)?),
            Client::Wayland(client) => {
                Window::Wayland(wayland::Window::open(client, options, callbacks)?)
            }
        })
    }
}

fn with_loop<R>(f: impl FnOnce(&LoopHandle<'static, ()>) -> R) -> R {
    LOOP.with(|cell| f(cell.get().expect("the event loop runs on the main thread")))
}

pub(crate) fn post(f: Box<dyn FnOnce() + Send>) {
    if let Some(sender) = POST.get() {
        let _ = sender.lock().map(|sender| sender.send(f));
    }
}

pub(crate) fn run_after(delay: Duration, f: impl FnOnce() + 'static) {
    let mut f = Some(f);
    with_loop(|handle| {
        let _ = handle.insert_source(Timer::from_duration(delay), move |_, _, _| {
            if let Some(f) = f.take() {
                f();
            }
            TimeoutAction::Drop
        });
    });
}

fn post_local(f: impl FnOnce() + 'static) {
    with_loop(|handle| {
        handle.insert_idle(move |_| f());
    });
}

pub(crate) enum Window {
    X11(x11::Window),
    Wayland(wayland::Window),
}

macro_rules! each {
    ($window:expr, $inner:ident => $body:expr) => {
        match $window {
            Window::X11($inner) => $body,
            Window::Wayland($inner) => $body,
        }
    };
}

impl Window {
    pub fn atlas(&self) -> &Rc<GpuAtlas> {
        each!(self, window => window.atlas())
    }

    pub fn scale_factor(&self) -> f32 {
        each!(self, window => window.scale_factor())
    }

    pub fn content_size(&self) -> Size<Pixels> {
        each!(self, window => window.content_size())
    }

    pub fn request_frame(&self) {
        each!(self, window => window.request_frame())
    }

    pub fn force_frame(&self) {
        each!(self, window => window.force_frame())
    }

    pub fn draw(&self, scene: &Scene, capture: bool) -> gpu::Capture {
        each!(self, window => window.draw(scene, capture))
    }

    pub fn is_active(&self) -> bool {
        each!(self, window => window.is_active())
    }

    pub fn set_title(&self, title: &str) {
        each!(self, window => window.set_title(title))
    }

    pub fn set_button_visibility(&self, _visible: bool) {}
    pub fn set_button_position(&self, _position: Option<Point<Pixels>>) {}

    pub fn state(&self) -> WindowState {
        each!(self, window => window.state())
    }

    pub fn change(&self, change: WindowChange) {
        match change {
            WindowChange::Show(_) | WindowChange::Restore => self.activate(),
            WindowChange::Parent(parent, _) => {
                if let Window::X11(window) = self {
                    window.set_parent(parent);
                }
            }
            WindowChange::CancelComposition => each!(self, window => window.cancel_composition()),
            _ => {}
        }
    }

    pub fn minimize(&self) {
        each!(self, window => window.minimize())
    }

    pub fn zoom(&self) {
        each!(self, window => window.zoom())
    }

    pub fn toggle_fullscreen(&self) {
        each!(self, window => window.toggle_fullscreen())
    }

    pub fn activate(&self) {
        each!(self, window => window.activate())
    }

    pub fn resize(&self, size: Size<Pixels>) {
        each!(self, window => window.resize(size))
    }

    pub fn start_window_move(&self) {
        each!(self, window => window.start_window_move())
    }

    pub fn titlebar_double_click(&self) {
        self.zoom();
    }

    pub fn raw_window_handle(&self) -> Result<rwh::RawWindowHandle> {
        Ok(each!(self, window => window.raw_window_handle()))
    }
}

fn app_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| Some(path.file_stem()?.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "apps".to_owned())
}

#[derive(Default)]
struct Clicks {
    last: Option<(MouseButton, Instant, Point<Pixels>)>,
    count: usize,
}

impl Clicks {
    fn press(&mut self, button: MouseButton, position: Point<Pixels>) -> usize {
        let now = Instant::now();
        let again = self.last.is_some_and(|(last, time, at)| {
            last == button
                && now - time < DOUBLE_CLICK_INTERVAL
                && (at.x - position.x).abs() <= DOUBLE_CLICK_DISTANCE
                && (at.y - position.y).abs() <= DOUBLE_CLICK_DISTANCE
        });
        self.count = if again { self.count + 1 } else { 1 };
        self.last = Some((button, now, position));
        self.count
    }
}

fn key_down(callbacks: Callbacks, keystroke: Keystroke, is_held: bool, ime: Vec<Ime>) {
    let modifiers = keystroke.modifiers;
    let text = keystroke.key_char.clone().filter(|_| {
        !modifiers.control && !modifiers.alt && !modifiers.platform && !modifiers.function
    });
    let input = PlatformInput::KeyDown(KeyDownEvent {
        keystroke,
        is_held,
        ime: !ime.is_empty(),
    });
    let handled = crate::app::native::key(callbacks.id, &input, &|| {
        callbacks.marked_text_range().is_some()
    }) || callbacks.input(input);
    for ime in ime {
        ime.apply(callbacks);
    }
    if !handled && let Some(text) = text {
        callbacks.replace_text_in_range(None, &text);
    }
}

fn key_up(callbacks: Callbacks, keystroke: Keystroke) {
    let input = PlatformInput::KeyUp(KeyUpEvent {
        keystroke,
        ime: false,
    });
    if !crate::app::native::key(callbacks.id, &input, &|| {
        callbacks.marked_text_range().is_some()
    }) {
        callbacks.input(input);
    }
}

enum Ime {
    Commit(String),
    Preedit(String),
    Unmark,
    Clear,
}

impl Ime {
    fn apply(self, callbacks: Callbacks) {
        match self {
            Self::Commit(text) => callbacks.replace_text_in_range(None, &text),
            Self::Preedit(text) if !text.is_empty() => {
                callbacks.replace_and_mark_text_in_range(None, &text, None)
            }
            Self::Preedit(_) | Self::Clear => callbacks.clear_marked_text(),
            Self::Unmark => callbacks.unmark_text(),
        }
    }
}

fn ime_area(callbacks: Callbacks) -> Option<Bounds<Pixels>> {
    callbacks.bounds_for_range(callbacks.selected_text_range()?.range)
}

fn compose(
    state: &mut xkb::compose::State,
    pre_edit: &mut Option<String>,
    keysym: Keysym,
    keystroke: &mut Keystroke,
) -> (Option<String>, Vec<Ime>) {
    state.feed(keysym);
    match state.status() {
        xkb::Status::Composed => {
            let composed = state.utf8();
            if pre_edit.take().is_some() {
                keystroke.key_char = None;
                return (None, vec![composed.map_or(Ime::Clear, Ime::Commit)]);
            }
            keystroke.key_char = composed;
            if let Some(keysym) = state.keysym() {
                keystroke.key = xkb::keysym_get_name(keysym);
            }
            (None, Vec::new())
        }
        xkb::Status::Composing => {
            keystroke.key_char = None;
            *pre_edit = state.utf8().or_else(|| dead_key(keysym));
            (
                None,
                vec![Ime::Preedit(pre_edit.clone().unwrap_or_default())],
            )
        }
        xkb::Status::Cancelled => {
            let previous = pre_edit.take();
            *pre_edit = dead_key(keysym);
            state.feed(keysym);
            let Some(text) = pre_edit.clone() else {
                return (previous, Vec::new());
            };
            keystroke.key_char = None;
            let ime = previous.map(Ime::Commit).into_iter();
            (None, ime.chain([Ime::Preedit(text)]).collect())
        }
        _ => (None, Vec::new()),
    }
}

fn new_xkb_context() -> Result<xkb::Context> {
    let context = xkb::Context::new(xkb::CONTEXT_NO_DEFAULT_INCLUDES);
    anyhow::ensure!(
        !context.get_raw_ptr().is_null(),
        "libxkbcommon failed to create an XKB context"
    );
    Ok(context)
}

fn compose_state(context: &xkb::Context) -> Option<xkb::compose::State> {
    let locale = std::env::var_os("LC_CTYPE");
    [locale.as_deref(), Some("C".as_ref())]
        .into_iter()
        .flatten()
        .find_map(|locale| {
            let table = xkb::compose::Table::new_from_locale(
                context,
                locale,
                xkb::compose::COMPILE_NO_FLAGS,
            )
            .ok()?;
            Some(xkb::compose::State::new(
                &table,
                xkb::compose::STATE_NO_FLAGS,
            ))
        })
}

fn modifiers_from_xkb(state: &xkb::State) -> Modifiers {
    let active = |name| state.mod_name_is_active(name, xkb::STATE_MODS_EFFECTIVE);
    Modifiers {
        shift: active(xkb::MOD_NAME_SHIFT),
        alt: active(xkb::MOD_NAME_ALT),
        control: active(xkb::MOD_NAME_CTRL),
        platform: active(xkb::MOD_NAME_LOGO),
        function: false,
    }
}

fn capslock_from_xkb(state: &xkb::State) -> Capslock {
    Capslock {
        on: state.mod_name_is_active(xkb::MOD_NAME_CAPS, xkb::STATE_MODS_EFFECTIVE),
    }
}

fn keystroke_from_xkb(state: &xkb::State, mut modifiers: Modifiers, keycode: Keycode) -> Keystroke {
    let key_utf32 = state.key_get_utf32(keycode);
    let key_utf8 = state.key_get_utf8(keycode);
    let key_sym = state.key_get_one_sym(keycode);
    let key = match key_sym {
        Keysym::Return => "enter",
        Keysym::Prior | Keysym::KP_Prior => "pageup",
        Keysym::Next | Keysym::KP_Next => "pagedown",
        Keysym::ISO_Left_Tab | Keysym::Tab => "tab",
        Keysym::XF86_Back => "back",
        Keysym::XF86_Forward => "forward",
        Keysym::XF86_Cut => "cut",
        Keysym::XF86_Copy => "copy",
        Keysym::XF86_Paste => "paste",
        Keysym::XF86_New => "new",
        Keysym::XF86_Open => "open",
        Keysym::XF86_Save => "save",
        Keysym::comma => ",",
        Keysym::period => ".",
        Keysym::less => "<",
        Keysym::greater => ">",
        Keysym::slash => "/",
        Keysym::question => "?",
        Keysym::semicolon => ";",
        Keysym::colon => ":",
        Keysym::apostrophe => "'",
        Keysym::quotedbl => "\"",
        Keysym::bracketleft => "[",
        Keysym::braceleft => "{",
        Keysym::bracketright => "]",
        Keysym::braceright => "}",
        Keysym::backslash => "\\",
        Keysym::bar => "|",
        Keysym::grave => "`",
        Keysym::asciitilde => "~",
        Keysym::exclam => "!",
        Keysym::at => "@",
        Keysym::numbersign => "#",
        Keysym::dollar => "$",
        Keysym::percent => "%",
        Keysym::asciicircum => "^",
        Keysym::ampersand => "&",
        Keysym::asterisk => "*",
        Keysym::parenleft => "(",
        Keysym::parenright => ")",
        Keysym::minus => "-",
        Keysym::underscore => "_",
        Keysym::equal => "=",
        Keysym::plus => "+",
        Keysym::space => "space",
        Keysym::BackSpace => "backspace",
        Keysym::Delete => "delete",
        Keysym::Escape => "escape",
        Keysym::Left => "left",
        Keysym::Right => "right",
        Keysym::Up => "up",
        Keysym::Down => "down",
        Keysym::Home => "home",
        Keysym::End => "end",
        Keysym::Insert => "insert",
        _ => "",
    };
    let key = if !key.is_empty() {
        key.to_owned()
    } else {
        let name = xkb::keysym_get_name(key_sym).to_lowercase();
        if key_sym.is_keypad_key() {
            name.replace("kp_", "")
        } else if let Some(key) = key_utf8.chars().next()
            && key_utf8.len() == 1
            && key.is_ascii()
        {
            if key.is_ascii_graphic() {
                key_utf8.to_lowercase()
            } else if key_utf32 <= 0x1f && !name.chars().next().is_some_and(|c| c.is_ascii_digit())
            {
                ((key_utf32 as u8 + 0x40) as char)
                    .to_ascii_lowercase()
                    .to_string()
            } else {
                name
            }
        } else if let Some(key) = guess_ascii(keycode, modifiers.shift) {
            key.to_string()
        } else {
            name
        }
    };
    if modifiers.shift && key.chars().count() == 1 && key.to_lowercase() == key.to_uppercase() {
        modifiers.shift = false;
    }
    let key_char =
        (key_utf32 >= 32 && key_utf32 != 127 && !key_utf8.is_empty()).then_some(key_utf8);
    Keystroke {
        modifiers,
        key,
        key_char,
    }
}

fn guess_ascii(keycode: Keycode, shift: bool) -> Option<char> {
    Some(match (keycode.raw(), shift) {
        (24, _) => 'q',
        (25, _) => 'w',
        (26, _) => 'e',
        (27, _) => 'r',
        (28, _) => 't',
        (29, _) => 'y',
        (30, _) => 'u',
        (31, _) => 'i',
        (32, _) => 'o',
        (33, _) => 'p',
        (34, false) => '[',
        (34, true) => '{',
        (35, false) => ']',
        (35, true) => '}',
        (38, _) => 'a',
        (39, _) => 's',
        (40, _) => 'd',
        (41, _) => 'f',
        (42, _) => 'g',
        (43, _) => 'h',
        (44, _) => 'j',
        (45, _) => 'k',
        (46, _) => 'l',
        (47, false) => ';',
        (47, true) => ':',
        (48, false) => '\'',
        (48, true) => '"',
        (49, false) => '`',
        (49, true) => '~',
        (51, false) => '\\',
        (51, true) => '|',
        (52, _) => 'z',
        (53, _) => 'x',
        (54, _) => 'c',
        (55, _) => 'v',
        (56, _) => 'b',
        (57, _) => 'n',
        (58, _) => 'm',
        (59, false) => ',',
        (59, true) => '>',
        (60, false) => '.',
        (60, true) => '<',
        (61, false) => '/',
        (61, true) => '?',
        _ => return None,
    })
}

fn dead_key(keysym: Keysym) -> Option<String> {
    let symbol = match keysym {
        Keysym::dead_grave => "`",
        Keysym::dead_acute => "´",
        Keysym::dead_circumflex => "^",
        Keysym::dead_tilde => "~",
        Keysym::dead_macron => "¯",
        Keysym::dead_breve => "˘",
        Keysym::dead_abovedot => "˙",
        Keysym::dead_diaeresis => "¨",
        Keysym::dead_abovering => "˚",
        Keysym::dead_doubleacute => "˝",
        Keysym::dead_caron => "ˇ",
        Keysym::dead_cedilla => "¸",
        Keysym::dead_ogonek => "˛",
        Keysym::dead_iota => "ͅ",
        Keysym::dead_voiced_sound => "゙",
        Keysym::dead_semivoiced_sound => "゚",
        Keysym::dead_belowdot => "̣̣",
        Keysym::dead_hook => "̡",
        Keysym::dead_horn => "̛",
        Keysym::dead_stroke => "̶̶",
        Keysym::dead_abovecomma => "̓̓",
        Keysym::dead_abovereversedcomma => "ʽ",
        Keysym::dead_doublegrave => "̏",
        Keysym::dead_belowring => "˳",
        Keysym::dead_belowmacron => "̱",
        Keysym::dead_belowcircumflex => "ꞈ",
        Keysym::dead_belowtilde => "̰",
        Keysym::dead_belowbreve => "̮",
        Keysym::dead_belowdiaeresis => "̤",
        Keysym::dead_invertedbreve => "̯",
        Keysym::dead_belowcomma => "̦",
        Keysym::dead_small_schwa => "ə",
        Keysym::dead_capital_schwa => "Ə",
        _ => return None,
    };
    Some(symbol.to_owned())
}

fn cursor_names(style: CursorStyle) -> &'static [&'static str] {
    match style {
        CursorStyle::Arrow | CursorStyle::None => &[DEFAULT_CURSOR],
        CursorStyle::IBeam => &["text", "xterm"],
        CursorStyle::Crosshair => &["crosshair", "cross"],
        CursorStyle::ClosedHand => &["closedhand", "grabbing", "hand2"],
        CursorStyle::OpenHand => &["openhand", "grab", "hand1"],
        CursorStyle::PointingHand => &["pointer", "hand", "hand2"],
        CursorStyle::ResizeLeft => &["w-resize", "left_side"],
        CursorStyle::ResizeRight => &["e-resize", "right_side"],
        CursorStyle::ResizeLeftRight => &["ew-resize", "sb_h_double_arrow"],
        CursorStyle::ResizeUp => &["n-resize", "top_side"],
        CursorStyle::ResizeDown => &["s-resize", "bottom_side"],
        CursorStyle::ResizeUpDown => &["sb_v_double_arrow", "ns-resize"],
        CursorStyle::ResizeUpLeftDownRight => &["size_fdiag", "bd_double_arrow", "nwse-resize"],
        CursorStyle::ResizeUpRightDownLeft => &["size_bdiag", "nesw-resize", "fd_double_arrow"],
        CursorStyle::ResizeColumn => &["col-resize", "sb_h_double_arrow"],
        CursorStyle::ResizeRow => &["row-resize", "sb_v_double_arrow"],
        CursorStyle::IBeamCursorForVerticalLayout => &["vertical-text"],
        CursorStyle::OperationNotAllowed => &["not-allowed", "crossed_circle"],
        CursorStyle::DragLink => &["alias"],
        CursorStyle::DragCopy => &["copy"],
        CursorStyle::ContextualMenu => &["context-menu"],
    }
}

fn file_paths(list: &str) -> Vec<PathBuf> {
    use std::os::unix::ffi::OsStringExt as _;
    list.lines()
        .filter_map(|line| line.trim().strip_prefix("file://"))
        .filter_map(|rest| rest.find('/').map(|slash| &rest[slash..]))
        .map(|path| {
            let mut bytes = Vec::with_capacity(path.len());
            let mut rest = path.as_bytes();
            while let Some((&byte, tail)) = rest.split_first() {
                let hex = (byte == b'%')
                    .then(|| tail.get(..2))
                    .flatten()
                    .and_then(|hex| u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok());
                match hex {
                    Some(decoded) => {
                        bytes.push(decoded);
                        rest = &tail[2..];
                    }
                    None => {
                        bytes.push(byte);
                        rest = tail;
                    }
                }
            }
            PathBuf::from(std::ffi::OsString::from_vec(bytes))
        })
        .collect()
}

fn read_pipe(mut reader: std::io::PipeReader) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let mut poll = libc::pollfd {
            fd: reader.as_fd().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd.
        let ready = unsafe { libc::poll(&mut poll, 1, PIPE_READ_TIMEOUT.as_millis() as i32) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error).context("waiting for the pipe");
        }
        anyhow::ensure!(
            ready > 0,
            "timed out waiting for data on pipe after {PIPE_READ_TIMEOUT:?}"
        );
        match reader.read(&mut chunk) {
            Ok(0) => return Ok(buffer),
            Ok(len) => buffer.extend_from_slice(&chunk[..len]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
}
