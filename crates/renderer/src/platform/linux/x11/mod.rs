mod clipboard;
mod window;

use super::input::{self, Clicks, Composer, Ime, NOTCH_LINES};
use super::{Display, Opened};
use crate::{Button, Cursor, Event, Modifiers, Pixels, Point, Rect, WindowOptions};
use crate::{app::Callbacks, point, px, size};
use anyhow::{Context as _, Result, ensure};
use rustc_hash::FxHashMap;
use std::os::fd::{AsRawFd as _, RawFd};
use std::{cell::RefCell, rc::Rc, rc::Weak, time::Duration};
use window::Toplevel;
use x11rb::connection::{Connection as _, RequestConnection as _};
use x11rb::protocol::xinput::{self, ConnectionExt as _};
use x11rb::protocol::xkb::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{self, ConnectionExt as _, EventMask};
use x11rb::protocol::{ErrorKind, Event as XEvent, randr, randr::ConnectionExt as _};
use x11rb::xcb_ffi::XCBConnection;
use xim::{AttributeName as Field, Client as _, InputStyle, x11rb::X11rbClient};
use xkbcommon::xkb as xkbc;

x11rb::atom_manager! {
    pub(crate) Atoms: AtomsCookie {
        UTF8_STRING,
        Protocols: b"WM_PROTOCOLS",
        DeleteWindow: b"WM_DELETE_WINDOW",
        ChangeState: b"WM_CHANGE_STATE",
        Pid: b"_NET_WM_PID",
        Name: b"_NET_WM_NAME",
        State: b"_NET_WM_STATE",
        Tall: b"_NET_WM_STATE_MAXIMIZED_VERT",
        Wide: b"_NET_WM_STATE_MAXIMIZED_HORZ",
        Fullscreen: b"_NET_WM_STATE_FULLSCREEN",
        Hidden: b"_NET_WM_STATE_HIDDEN",
        Focused: b"_NET_WM_STATE_FOCUSED",
        ActiveWindow: b"_NET_ACTIVE_WINDOW",
        SyncRequest: b"_NET_WM_SYNC_REQUEST",
        SyncCounter: b"_NET_WM_SYNC_REQUEST_COUNTER",
        MoveResize: b"_NET_WM_MOVERESIZE",
        MotifHints: b"_MOTIF_WM_HINTS",
        DndAware: b"XdndAware",
        DndEnter: b"XdndEnter",
        DndLeave: b"XdndLeave",
        DndPosition: b"XdndPosition",
        DndStatus: b"XdndStatus",
        DndDrop: b"XdndDrop",
        DndFinished: b"XdndFinished",
        DndSelection: b"XdndSelection",
        DndTypes: b"XdndTypeList",
        DndCopy: b"XdndActionCopy",
        UriList: b"text/uri-list",
        DropData: b"XDND_DATA",
    }
}

type Xcb = Rc<XCBConnection>;
type Im = X11rbClient<Xcb>;
type Failure = xim::ClientError;

enum ImeOut {
    Key(XEvent),
    Preedit(xproto::Window, String),
    Commit(xproto::Window, String),
}

#[derive(Default)]
struct ImeLink {
    im: u16,
    ic: u16,
    open: bool,
    ready: bool,
    window: xproto::Window,
    preedit: Vec<char>,
    out: Option<ImeOut>,
}

impl ImeLink {
    fn attributes(&self, client: &Im, spot: Option<xim::Point>) -> Vec<xim::Attribute> {
        let mut list = client.build_ic_attributes();
        list = list.push(Field::InputStyle, InputStyle::PREEDIT_CALLBACKS);
        list = list
            .push(Field::ClientWindow, self.window)
            .push(Field::FocusWindow, self.window);
        if let Some(spot) = spot {
            list = list.nested_list(Field::PreeditAttributes, |inner| {
                inner.push(Field::SpotLocation, spot);
            });
        }
        list.build()
    }
}

impl xim::ClientHandler<Im> for ImeLink {
    fn handle_connect(&mut self, client: &mut Im) -> Result<(), Failure> {
        client.open("C")
    }

    fn handle_open(&mut self, client: &mut Im, im: u16) -> Result<(), Failure> {
        self.im = im;
        client.get_im_values(im, &[Field::QueryInputStyle])
    }

    fn handle_get_im_values(
        &mut self,
        client: &mut Im,
        im: u16,
        _: xim::AHashMap<Field, Vec<u8>>,
    ) -> Result<(), Failure> {
        // A context needs a window, so it waits for the first focus.
        self.open = true;
        match self.window {
            0 => Ok(()),
            _ => client.create_ic(im, self.attributes(client, None)),
        }
    }

    // A new context is for the focused window.
    fn handle_create_ic(&mut self, client: &mut Im, im: u16, ic: u16) -> Result<(), Failure> {
        (self.ic, self.ready) = (ic, true);
        client.set_focus(im, ic)
    }

    fn handle_commit(&mut self, _: &mut Im, _: u16, _: u16, text: &str) -> Result<(), Failure> {
        self.preedit.clear();
        self.out = Some(ImeOut::Commit(self.window, text.to_owned()));
        Ok(())
    }

    fn handle_forward_event(
        &mut self,
        _: &mut Im,
        _: u16,
        _: u16,
        _: xim::ForwardEventFlag,
        key: xproto::KeyPressEvent,
    ) -> Result<(), Failure> {
        self.out = match key.response_type {
            xproto::KEY_PRESS_EVENT => Some(ImeOut::Key(XEvent::KeyPress(key))),
            xproto::KEY_RELEASE_EVENT => Some(ImeOut::Key(XEvent::KeyRelease(key))),
            _ => self.out.take(),
        };
        Ok(())
    }

    fn handle_close(&mut self, client: &mut Im, _: u16) -> Result<(), Failure> {
        client.disconnect()
    }

    // A draw replaces `length` characters at `first` of the composition.
    #[allow(clippy::too_many_arguments)]
    fn handle_preedit_draw(
        &mut self,
        _: &mut Im,
        _: u16,
        _: u16,
        _: i32,
        first: i32,
        length: i32,
        _: xim::PreeditDrawStatus,
        text: &str,
        _: Vec<xim::Feedback>,
    ) -> Result<(), Failure> {
        let start = (first.max(0) as usize).min(self.preedit.len());
        let end = (start + length.max(0) as usize).min(self.preedit.len());
        self.preedit.splice(start..end, text.chars());
        self.out = Some(ImeOut::Preedit(self.window, self.preedit.iter().collect()));
        Ok(())
    }

    fn handle_preedit_done(&mut self, _: &mut Im, _: u16, _: u16) -> Result<(), Failure> {
        if !self.preedit.is_empty() {
            self.preedit.clear();
            self.out = Some(ImeOut::Preedit(self.window, String::new()));
        }
        Ok(())
    }
}

#[derive(Default, Clone, Copy)]
struct Valuator {
    number: Option<u16>,
    lines_per_unit: f32,
    last: Option<f32>,
}

// The scroll valuators of a pointer device. Their values are absolute, so the first event of a scroll only sets them.
#[derive(Default, Clone, Copy)]
struct Scroller {
    across: Valuator,
    down: Valuator,
}

#[derive(Default)]
struct Drag {
    source: xproto::Window,
    files: bool,
    read: bool,
    at: Point<Pixels>,
}

struct Live {
    windows: FxHashMap<xproto::Window, Weak<Toplevel>>,
    focus: Option<xproto::Window>,
    hover: Option<xproto::Window>,
    context: xkbc::Context,
    device: i32,
    xkb: xkbc::State,
    composer: Composer,
    reported: (Modifiers, bool),
    held: Vec<u8>,
    ime: Option<(Im, ImeLink)>,
    composing: bool,
    // A press that went to the input method, sent with the change it makes.
    waiting_press: Option<xproto::KeyPressEvent>,
    cursor: Cursor,
    cursors: FxHashMap<Cursor, Option<xproto::Cursor>>,
    themes: x11rb::cursor::Handle,
    scrollers: FxHashMap<xinput::DeviceId, Scroller>,
    clicks: Clicks,
    drag: Drag,
    // The last source of a drop, which can be gone when it gets an answer.
    dropper: xproto::Window,
}

pub(super) struct Client {
    xcb: Xcb,
    atoms: Atoms,
    screen: usize,
    root: xproto::Window,
    scale: f32,
    refresh: Duration,
    live: RefCell<Live>,
    clipboard: clipboard::Selection,
}

fn fixed(value: xinput::Fp3232) -> f32 {
    (f64::from(value.integral) + f64::from(value.frac) / 4_294_967_296.) as f32
}

fn scrollers(
    xcb: &XCBConnection,
    old: &FxHashMap<xinput::DeviceId, Scroller>,
) -> FxHashMap<xinput::DeviceId, Scroller> {
    let Some(devices) = xcb
        .xinput_xi_query_device(0u16)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
    else {
        return FxHashMap::default();
    };
    let pointers = devices
        .infos
        .iter()
        .filter(|info| info.type_ == xinput::DeviceType::SLAVE_POINTER);
    pointers
        .filter_map(|info| {
            let kept = old.get(&info.deviceid).copied().unwrap_or_default();
            let axis = |kind, kept: Valuator| {
                let classes = info
                    .classes
                    .iter()
                    .rev()
                    .filter_map(|entry| entry.data.as_scroll());
                let scroll = classes
                    .into_iter()
                    .find(|scroll| scroll.scroll_type == kind)?;
                let lines_per_unit = NOTCH_LINES / fixed(scroll.increment);
                Some(Valuator {
                    number: Some(scroll.number),
                    lines_per_unit,
                    last: kept.last,
                })
            };
            let across = axis(xinput::ScrollType::HORIZONTAL, kept.across);
            let down = axis(xinput::ScrollType::VERTICAL, kept.down);
            let found = across.is_some() || down.is_some();
            let scroller = Scroller {
                across: across.unwrap_or_default(),
                down: down.unwrap_or_default(),
            };
            found.then_some((info.deviceid, scroller))
        })
        .collect()
}

fn refresh_of(xcb: &XCBConnection, root: xproto::Window) -> Duration {
    let resources = xcb
        .randr_get_screen_resources_current(root)
        .ok()
        .and_then(|cookie| cookie.reply().ok());
    let mode = resources.and_then(|resources| {
        resources.crtcs.iter().find_map(|&crtc| {
            let info = xcb
                .randr_get_crtc_info(crtc, x11rb::CURRENT_TIME)
                .ok()?
                .reply()
                .ok()?;
            resources
                .modes
                .iter()
                .find(|mode| mode.id == info.mode)
                .copied()
        })
    });
    match mode {
        Some(mode) if mode.dot_clock > 0 && mode.htotal > 0 && mode.vtotal > 0 => {
            let pixels = u64::from(mode.htotal) * u64::from(mode.vtotal);
            Duration::from_nanos(pixels * 1_000_000_000 / u64::from(mode.dot_clock))
        }
        _ => Duration::from_micros(16_667),
    }
}

// Xft.dpi when set, else the density of the primary monitor in twelfths, from 1 to 20.
fn scale_of(xcb: &XCBConnection, root: xproto::Window) -> f32 {
    let database = x11rb::resource_manager::new_from_default(xcb).ok();
    if let Some(Ok(Some(dpi))) =
        database.map(|database| database.get_value::<f32>("Xft.dpi", "Xft.dpi"))
    {
        return dpi / 96.;
    }
    let monitors = xcb
        .randr_get_monitors(root, true)
        .ok()
        .and_then(|cookie| cookie.reply().ok());
    let monitors = monitors.map(|reply| reply.monitors).unwrap_or_default();
    let measured = monitors
        .iter()
        .filter(|monitor| monitor.width_in_millimeters > 0 && monitor.height_in_millimeters > 0);
    let Some(monitor) = measured.min_by_key(|monitor| !monitor.primary) else {
        return 1.;
    };
    let area = f64::from(monitor.width) * f64::from(monitor.height);
    let millimeters =
        f64::from(monitor.width_in_millimeters) * f64::from(monitor.height_in_millimeters);
    let per_inch = (area / millimeters).sqrt() * 25.4;
    match ((per_inch / 96. * 12.).round() / 12.).max(1.) {
        scale if scale <= 20. => scale as f32,
        _ => 1.,
    }
}

fn mask_modifiers(mask: u16) -> Modifiers {
    let on = |bit: xproto::KeyButMask| mask & u16::from(bit) != 0;
    Modifiers::NONE
        .with(Modifiers::CTRL, on(xproto::KeyButMask::CONTROL))
        .with(Modifiers::ALT, on(xproto::KeyButMask::MOD1))
        .with(Modifiers::SHIFT, on(xproto::KeyButMask::SHIFT))
        .with(Modifiers::COMMAND, on(xproto::KeyButMask::MOD4))
}

fn keymap_state(context: &xkbc::Context, xcb: &XCBConnection, device: i32) -> xkbc::State {
    let keymap =
        xkbc::x11::keymap_new_from_device(context, xcb, device, xkbc::KEYMAP_COMPILE_NO_FLAGS);
    xkbc::x11::state_new_from_device(&keymap, xcb, device)
}

impl Client {
    pub fn connect() -> Result<Rc<Self>> {
        let (xcb, screen) = XCBConnection::connect(None).context("connecting to the X server")?;
        for name in [
            xkb::X11_EXTENSION_NAME,
            randr::X11_EXTENSION_NAME,
            xinput::X11_EXTENSION_NAME,
        ] {
            xcb.prefetch_extension_information(name)?;
        }
        let version = xcb
            .xinput_xi_query_version(2, 2)?
            .reply()
            .context("XInput is missing")?;
        ensure!(version.major_version >= 2, "the X server has no XInput 2");
        let atoms = Atoms::new(&xcb)?.reply()?;
        let root = xcb.setup().roots[screen].root;
        let (major, minor) = (
            xkbc::x11::ffi::XKB_X11_MIN_MAJOR_XKB_VERSION,
            xkbc::x11::ffi::XKB_X11_MIN_MINOR_XKB_VERSION,
        );
        ensure!(
            xcb.xkb_use_extension(major, minor)?.reply()?.supported,
            "the X server has no XKB"
        );
        // Every part of the keymap, so changes to any of them arrive.
        let parts = xkb::MapPart::from(0xffu16);
        let kinds = xkb::EventType::STATE_NOTIFY
            | xkb::EventType::MAP_NOTIFY
            | xkb::EventType::NEW_KEYBOARD_NOTIFY;
        let core = xkb::DeviceSpec::from(xkb::ID::USE_CORE_KBD);
        xcb.xkb_select_events(core, 0u8.into(), kinds, parts, parts, &Default::default())?
            .check()?;
        x11rb::protocol::sync::initialize(&xcb, 3, 1)?
            .reply()
            .context("X11 sync")?;
        let context = input::xkb_context()?;
        let device = xkbc::x11::get_core_keyboard_device_id(&xcb);
        let xkb = keymap_state(&context, &xcb, device);
        let database = x11rb::resource_manager::new_from_default(&xcb)?;
        let themes = x11rb::cursor::Handle::new(&xcb, screen, &database)?.reply()?;
        let (scale, refresh) = (scale_of(&xcb, root), refresh_of(&xcb, root));
        let found = scrollers(&xcb, &FxHashMap::default());
        let xcb = Rc::new(xcb);
        let clipboard = clipboard::Selection::open()?;
        let ime = X11rbClient::init(xcb.clone(), screen, None)
            .ok()
            .map(|client| (client, ImeLink::default()));
        let live = Live {
            windows: FxHashMap::default(),
            focus: None,
            hover: None,
            composer: Composer::new(&context),
            context,
            device,
            xkb,
            reported: (Modifiers::NONE, false),
            held: Vec::new(),
            ime,
            composing: false,
            waiting_press: None,
            cursor: Cursor::Default,
            cursors: FxHashMap::default(),
            themes,
            scrollers: found,
            clicks: Clicks::default(),
            drag: Drag::default(),
            dropper: x11rb::NONE,
        };
        xcb.flush()?;
        Ok(Rc::new(Self {
            clipboard,
            live: RefCell::new(live),
            xcb,
            atoms,
            screen,
            root,
            scale,
            refresh,
        }))
    }

    fn window(&self, id: xproto::Window) -> Option<Rc<Toplevel>> {
        self.live.borrow().windows.get(&id)?.upgrade()
    }

    fn at(&self, x: xinput::Fp1616, y: xinput::Fp1616) -> Point<Pixels> {
        let unit = 65536. * self.scale;
        point(px(x as f32 / unit), px(y as f32 / unit))
    }

    // Reads all queued events. A release right before a press of the same key is an X11 key repeat and goes, so a
    // keymap change waits until no release is left that a press could still take.
    fn drain(&self) -> bool {
        loop {
            let (mut batch, mut exposed) = (Vec::new(), Vec::new());
            let (mut release, mut keymap) = (None, None);
            loop {
                let event = match self.xcb.poll_for_event() {
                    Ok(Some(event)) => event,
                    Ok(None) => break,
                    Err(error) => {
                        eprintln!("apps: X11: {error}");
                        // A broken connection ends the app, other errors only this read.
                        if matches!(error, x11rb::errors::ConnectionError::IoError(_)) {
                            return false;
                        }
                        break;
                    }
                };
                match event {
                    XEvent::Expose(exposure) => exposed.push(exposure.window),
                    XEvent::XkbNewKeyboardNotify(_) | XEvent::XkbMapNotify(_) => {
                        batch.extend(release.take());
                        keymap = Some(event);
                    }
                    XEvent::KeyPress(press) => {
                        if let Some(change) = keymap.take() {
                            batch.extend(release.take());
                            batch.push(change);
                        }
                        let repeated = |up: &xproto::KeyReleaseEvent| {
                            up.detail == press.detail && press.time.wrapping_sub(up.time) <= 20
                        };
                        match release.take() {
                            Some(XEvent::KeyRelease(up)) if repeated(&up) => {}
                            other => batch.extend(other),
                        }
                        batch.push(XEvent::KeyPress(press));
                    }
                    XEvent::KeyRelease(_) => {
                        if let Some(change) = keymap.take() {
                            batch.extend(release.take());
                            batch.push(change);
                        }
                        batch.extend(release.replace(event));
                    }
                    _ => {
                        batch.extend(release.take());
                        batch.push(event);
                    }
                }
            }
            batch.extend(release);
            batch.extend(keymap);
            if batch.is_empty() && exposed.is_empty() {
                return true;
            }
            for event in batch {
                self.route(event);
            }
            exposed.sort_unstable();
            exposed.dedup();
            for window in exposed.into_iter().filter_map(|id| self.window(id)) {
                window.expose();
            }
        }
    }

    fn route(&self, event: XEvent) {
        let Some((mut im, mut link)) = self.live.borrow_mut().ime.take() else {
            self.handle(event);
            return;
        };
        let ready = link.ready;
        let filtered = im.filter_event(&event, &mut link);
        let out = link.out.take();
        self.live.borrow_mut().ime = Some((im, link));
        let change = match out {
            Some(ImeOut::Key(key)) => {
                if matches!(key, XEvent::KeyPress(_)) {
                    self.live.borrow_mut().waiting_press = None;
                }
                self.handle(key);
                None
            }
            Some(ImeOut::Commit(window, text)) => {
                self.live.borrow_mut().composing = false;
                Some((window, Ime::Commit(text), false))
            }
            Some(ImeOut::Preedit(window, text)) => {
                let shows = !text.is_empty();
                let showed = std::mem::replace(&mut self.live.borrow_mut().composing, shows);
                (showed || shows).then_some((window, Ime::Compose(text), true))
            }
            None => None,
        };
        if let Some((id, change, preedit)) = change
            && let Some(window) = self.window(id)
        {
            let press = {
                let mut live = self.live.borrow_mut();
                let waiting = live.waiting_press.take();
                waiting.map(|press| {
                    let mask = u16::from(press.state);
                    input::key(
                        &event_state(&live.xkb, mask),
                        press.detail.into(),
                        mask_modifiers(mask),
                    )
                })
            };
            match press {
                Some((mut key, modifiers)) => {
                    key.text = None;
                    input::press(window.calls, key, modifiers, false, vec![change]);
                }
                None => change.apply(window.calls),
            }
            if preedit {
                self.place_ime(window.calls);
            }
        }
        match filtered {
            Ok(true) => {}
            Ok(false) if ready => self.forward(event),
            Ok(false) => {
                self.handle(event);
            }
            Err(error) => {
                eprintln!("apps: XIM: {error}");
                self.live.borrow_mut().ime = None;
                self.handle(event);
            }
        }
    }

    fn forward(&self, event: XEvent) {
        let (XEvent::KeyPress(key) | XEvent::KeyRelease(key)) = event else {
            self.handle(event);
            return;
        };
        let press = matches!(event, XEvent::KeyPress(_));
        let live = &mut *self.live.borrow_mut();
        let Some((im, link)) = &mut live.ime else {
            return;
        };
        if let Err(error) = im.forward_event(link.im, link.ic, xim::ForwardEventFlag::empty(), &key)
        {
            eprintln!("apps: XIM: {error}");
        }
        if press
            && !live
                .xkb
                .key_get_one_sym(key.detail.into())
                .is_modifier_key()
        {
            live.waiting_press = Some(key);
        } else if !press
            && live
                .waiting_press
                .is_some_and(|waiting| waiting.detail == key.detail)
        {
            // An input method can take a key without an answer, so its change waits only until the release.
            live.waiting_press = None;
        }
    }

    fn place_ime(&self, calls: Callbacks) {
        let Some(caret) = input::caret(calls) else {
            return;
        };
        let live = &mut *self.live.borrow_mut();
        let Some((im, link)) = &mut live.ime else {
            return;
        };
        let corner = (caret.origin + point(caret.size.width, caret.size.height)).scale(self.scale);
        let spot = xim::Point {
            x: corner.x as i16,
            y: corner.y as i16,
        };
        let attributes = link.attributes(im, Some(spot));
        let _ = im.set_ic_values(link.im, link.ic, attributes);
    }

    // A context keeps its window, so another window gets a new one.
    fn focus_ime(&self, window: xproto::Window, calls: Callbacks) {
        {
            let live = &mut *self.live.borrow_mut();
            let Some((im, link)) = &mut live.ime else {
                return;
            };
            if link.ready && link.window == window {
                return;
            }
            if link.ready {
                let _ = im.destroy_ic(link.im, link.ic);
            }
            (link.ready, link.window) = (false, window);
            link.preedit.clear();
            // Before the input method opens, its answer about input styles makes the context.
            if link.open {
                let attributes = link.attributes(im, None);
                let _ = im.create_ic(link.im, attributes);
            }
        }
        self.place_ime(calls);
    }

    fn reset_ime(&self) {
        let live = &mut *self.live.borrow_mut();
        (live.composing, live.waiting_press) = (false, None);
        live.composer.reset();
        // The XIM client has no reset, so the context is made again, which drops its composition.
        if let Some((im, link)) = &mut live.ime
            && link.ready
        {
            let _ = im.destroy_ic(link.im, link.ic);
            link.ready = false;
            link.preedit.clear();
            let attributes = link.attributes(im, None);
            let _ = im.create_ic(link.im, attributes);
        }
    }

    fn forget_scrolls(&self) {
        for scroller in self.live.borrow_mut().scrollers.values_mut() {
            (scroller.across.last, scroller.down.last) = (None, None);
        }
    }

    fn handle(&self, event: XEvent) -> Option<()> {
        match event {
            XEvent::MapNotify(map) => self.window(map.window)?.set_mapped(true),
            XEvent::UnmapNotify(unmap) => self.window(unmap.window)?.set_mapped(false),
            XEvent::VisibilityNotify(seen) => {
                let covered = seen.state == xproto::Visibility::FULLY_OBSCURED;
                self.window(seen.window)?.set_obscured(covered);
            }
            XEvent::ConfigureNotify(change) => {
                let [x, y] = [change.x, change.y].map(i32::from);
                let [width, height] = [change.width, change.height].map(i32::from);
                self.window(change.window)?.configure(x, y, width, height);
            }
            XEvent::PropertyNotify(change) if change.atom == self.atoms.State => {
                self.window(change.window)?.read_wm_state();
            }
            XEvent::ClientMessage(message) => self.message(message)?,
            XEvent::SelectionNotify(answer) if answer.property == self.atoms.DropData => {
                let window = self.window(answer.requestor)?;
                let any = xproto::AtomEnum::ANY;
                let asked = self.xcb.get_property(
                    false,
                    answer.requestor,
                    self.atoms.DropData,
                    any,
                    0,
                    1 << 20,
                );
                let data = asked.ok()?.reply().ok()?;
                let paths = input::uri_paths(std::str::from_utf8(&data.value).ok()?);
                let at = {
                    let mut live = self.live.borrow_mut();
                    live.drag.read = true;
                    live.drag.at
                };
                window.calls.input(Event::FileEnter { at, paths });
            }
            XEvent::FocusIn(focus) => {
                let window = self.window(focus.event)?;
                self.live.borrow_mut().focus = Some(focus.event);
                window.focus(true);
                self.focus_ime(focus.event, window.calls);
            }
            XEvent::FocusOut(focus) => {
                let window = self.window(focus.event)?;
                // Scroll valuators are global, so a scroll in another window must not arrive as one big step.
                self.forget_scrolls();
                {
                    let mut live = self.live.borrow_mut();
                    live.focus = None;
                    live.held.clear();
                }
                self.reset_ime();
                window.focus(false);
            }
            XEvent::XkbNewKeyboardNotify(_) | XEvent::XkbMapNotify(_) => {
                let live = &mut *self.live.borrow_mut();
                live.xkb = keymap_state(&live.context, &self.xcb, live.device);
            }
            XEvent::XkbStateNotify(change) => {
                let mut live = self.live.borrow_mut();
                let [base, latched] =
                    [change.base_group, change.latched_group].map(|group| group as u32);
                let (depressed, locked_mods) = (change.base_mods.into(), change.locked_mods.into());
                live.xkb.update_mask(
                    depressed,
                    change.latched_mods.into(),
                    locked_mods,
                    base,
                    latched,
                    change.locked_group.into(),
                );
                let now = input::held(&live.xkb);
                if std::mem::replace(&mut live.reported, now) == now {
                    return Some(());
                }
                let focus = live.focus?;
                drop(live);
                self.window(focus)?
                    .calls
                    .input(Event::ModifiersChanged { modifiers: now.0 });
            }
            XEvent::KeyPress(press) => {
                let window = self.window(press.event)?;
                let mut live = self.live.borrow_mut();
                let mask = u16::from(press.state);
                let xkb = event_state(&live.xkb, mask);
                let code = press.detail.into();
                let sym = xkb.key_get_one_sym(code);
                if sym.is_modifier_key() {
                    return Some(());
                }
                let (mut key, modifiers) = input::key(&xkb, code, mask_modifiers(mask));
                let repeat = live.held.contains(&press.detail);
                if !repeat {
                    live.held.push(press.detail);
                }
                let (before, changes) = live.composer.feed(sym, &mut key);
                drop(live);
                if let Some(text) = before {
                    Ime::Commit(text).apply(window.calls);
                }
                input::press(window.calls, key, modifiers, repeat, changes);
            }
            XEvent::KeyRelease(release) => {
                let window = self.window(release.event)?;
                let mut live = self.live.borrow_mut();
                live.held.retain(|held| *held != release.detail);
                let mask = u16::from(release.state);
                let xkb = event_state(&live.xkb, mask);
                let code = release.detail.into();
                if xkb.key_get_one_sym(code).is_modifier_key() {
                    return Some(());
                }
                let (key, modifiers) = input::key(&xkb, code, mask_modifiers(mask));
                drop(live);
                input::release(window.calls, key, modifiers);
            }
            XEvent::XinputButtonPress(press) => {
                let window = self.window(press.event)?;
                let modifiers = mask_modifiers(press.mods.effective as u16);
                let at = self.at(press.event_x, press.event_y);
                // Buttons 4 to 7 are wheel notches: up, down, left, right.
                let notch = (4..=7).contains(&press.detail);
                // A click ends a composition, or commits a waiting dead key. A wheel notch is no click.
                let (composing, pending) = if notch {
                    (false, None)
                } else {
                    let mut live = self.live.borrow_mut();
                    (live.composing && live.ime.is_some(), live.composer.reset())
                };
                if composing {
                    self.reset_ime();
                    Ime::End.apply(window.calls);
                } else if let Some(text) = pending {
                    Ime::Commit(text).apply(window.calls);
                }
                let wheel = match press.detail {
                    4 => point(0., NOTCH_LINES),
                    5 => point(0., -NOTCH_LINES),
                    6 => point(NOTCH_LINES, 0.),
                    7 => point(-NOTCH_LINES, 0.),
                    detail => {
                        let button = button(detail)?;
                        let clicks = self.live.borrow_mut().clicks.press(button, at);
                        window.calls.input(Event::PointerDown {
                            at,
                            button,
                            modifiers,
                            clicks,
                        });
                        return Some(());
                    }
                };
                // Smooth scroll motions carry the same scroll as the emulated wheel presses.
                if !press
                    .flags
                    .contains(xinput::PointerEventFlags::POINTER_EMULATED)
                {
                    window.calls.input(wheel_event(at, wheel, modifiers));
                }
            }
            XEvent::XinputButtonRelease(release) => {
                let window = self.window(release.event)?;
                let button = button(release.detail)?;
                let clicks = self.live.borrow().clicks.count;
                let at = self.at(release.event_x, release.event_y);
                let modifiers = mask_modifiers(release.mods.effective as u16);
                window.calls.input(Event::PointerUp {
                    at,
                    button,
                    modifiers,
                    clicks,
                });
            }
            XEvent::XinputMotion(motion) => {
                let window = self.window(motion.event)?;
                let at = self.at(motion.event_x, motion.event_y);
                let modifiers = mask_modifiers(motion.mods.effective as u16);
                let mut live = self.live.borrow_mut();
                let scroll = live
                    .scrollers
                    .get_mut(&motion.sourceid)
                    .and_then(|scroller| scrolled(scroller, &motion));
                drop(live);
                // Valuators 0 and 1 are the pointer position.
                if motion
                    .valuator_mask
                    .first()
                    .is_some_and(|bits| bits & 3 != 0)
                {
                    let buttons = motion.button_mask.first().copied().unwrap_or(0);
                    let pressed = [(2, Button::Left), (4, Button::Middle), (8, Button::Right)];
                    let held = pressed
                        .into_iter()
                        .find_map(|(bit, button)| (buttons & bit != 0).then_some(button));
                    window.calls.input(Event::PointerMove {
                        at,
                        held,
                        modifiers,
                    });
                }
                if let Some(scroll) = scroll {
                    window.calls.input(wheel_event(at, scroll, modifiers));
                }
            }
            XEvent::XinputEnter(enter) if enter.mode == xinput::NotifyMode::NORMAL => {
                self.live.borrow_mut().hover = Some(enter.event);
                self.show_cursor();
            }
            XEvent::XinputLeave(leave) if leave.mode == xinput::NotifyMode::NORMAL => {
                self.forget_scrolls();
                self.live.borrow_mut().hover = None;
                let at = self.at(leave.event_x, leave.event_y);
                self.window(leave.event)?
                    .calls
                    .input(Event::PointerLeave { at });
            }
            XEvent::XinputHierarchy(_) => {
                let old = std::mem::take(&mut self.live.borrow_mut().scrollers);
                let found = scrollers(&self.xcb, &old);
                self.live.borrow_mut().scrollers = found;
            }
            XEvent::XinputDeviceChanged(change) => {
                if let Some(scroller) = self.live.borrow_mut().scrollers.get_mut(&change.sourceid) {
                    (scroller.across.last, scroller.down.last) = (None, None);
                }
            }
            // Sending to the input method fails when its window is gone. Keys then go to the app again.
            XEvent::Error(error)
                if error.error_kind == ErrorKind::Window
                    && error.major_opcode == xproto::SEND_EVENT_REQUEST =>
            {
                let mut live = self.live.borrow_mut();
                let ours =
                    live.windows.contains_key(&error.bad_value) || error.bad_value == live.dropper;
                if live.ime.is_some() && !ours {
                    eprintln!("apps: XIM: the input method is gone");
                    (live.ime, live.composing) = (None, false);
                }
            }
            _ => {}
        }
        Some(())
    }

    fn message(&self, message: xproto::ClientMessageEvent) -> Option<()> {
        let window = self.window(message.window)?;
        let [first, flags, third, fourth, fifth] = message.data.as_data32();
        let atoms = &self.atoms;
        let kind = message.type_;
        if kind == atoms.Protocols && first == atoms.DeleteWindow {
            window.calls.should_close();
        } else if kind == atoms.Protocols && first == atoms.SyncRequest {
            window.sync_request(x11rb::protocol::sync::Int64 {
                lo: third,
                hi: fourth as i32,
            });
        } else if kind == atoms.DndEnter {
            // More than three types are in a property of the source.
            let files = match flags & 1 {
                1 => {
                    let any = xproto::AtomEnum::ANY;
                    let asked = self
                        .xcb
                        .get_property(false, first, atoms.DndTypes, any, 0, 1024);
                    let list = asked.ok().and_then(|cookie| cookie.reply().ok());
                    let has = |list: xproto::GetPropertyReply| {
                        list.value32()?
                            .any(|kind| kind == atoms.UriList)
                            .then_some(())
                    };
                    list.and_then(has).is_some()
                }
                _ => [third, fourth, fifth].contains(&atoms.UriList),
            };
            let mut live = self.live.borrow_mut();
            (live.drag, live.dropper) = (
                Drag {
                    source: first,
                    files,
                    ..Drag::default()
                },
                first,
            );
        } else if kind == atoms.DndPosition {
            // The position is in root coordinates, packed in one word.
            let (x, y) = ((third >> 16) as i16, third as i16);
            let place = self
                .xcb
                .translate_coordinates(self.root, message.window, x, y)
                .ok();
            let place = place.and_then(|cookie| cookie.reply().ok());
            let mut live = self.live.borrow_mut();
            if let Some(place) = place {
                live.drag.at = point(px(place.dst_x as f32), px(place.dst_y as f32)) / self.scale;
            }
            if live.drag.files && !live.drag.read {
                let (into, list) = (atoms.DropData, atoms.UriList);
                let _ = self.xcb.convert_selection(
                    message.window,
                    atoms.DndSelection,
                    list,
                    into,
                    fourth,
                );
            }
            let (source, files, at) = (live.drag.source, live.drag.files, live.drag.at);
            drop(live);
            let action = if files { atoms.DndCopy } else { 0 };
            self.tell(
                source,
                atoms.DndStatus,
                [message.window, u32::from(files), 0, 0, action],
            );
            window.calls.input(Event::FileOver { at });
        } else if kind == atoms.DndLeave {
            let at = std::mem::take(&mut self.live.borrow_mut().drag).at;
            window.calls.input(Event::FileOver { at });
            window.calls.input(Event::FileLeave);
        } else if kind == atoms.DndDrop {
            let drag = std::mem::take(&mut self.live.borrow_mut().drag);
            self.tell(
                drag.source,
                atoms.DndFinished,
                [message.window, u32::from(drag.read), atoms.DndCopy, 0, 0],
            );
            window.calls.input(if drag.read {
                Event::FileDrop { at: drag.at }
            } else {
                Event::FileLeave
            });
        }
        Some(())
    }

    fn tell(&self, target: xproto::Window, kind: xproto::Atom, data: [u32; 5]) {
        let message = xproto::ClientMessageEvent::new(32, target, kind, data);
        let _ = self
            .xcb
            .send_event(false, target, EventMask::NO_EVENT, message);
        let _ = self.xcb.flush();
    }

    fn ask_manager(&self, window: xproto::Window, kind: xproto::Atom, data: [u32; 5]) {
        let message = xproto::ClientMessageEvent::new(32, window, kind, data);
        let to = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
        let _ = self.xcb.send_event(false, self.root, to, message);
        let _ = self.xcb.flush();
    }

    fn show_cursor(&self) {
        let live = &mut *self.live.borrow_mut();
        let Some(window) = live.hover else {
            return;
        };
        let (xcb, themes, cursor) = (&*self.xcb, &live.themes, live.cursor);
        let loaded = *live.cursors.entry(cursor).or_insert_with(|| {
            if cursor == Cursor::None {
                return blank_cursor(xcb, self.root).ok();
            }
            let mut names = input::cursor_names(cursor)
                .split(' ')
                .chain(["default", "left_ptr"]);
            names.find_map(|name| {
                themes
                    .load_cursor(xcb, name)
                    .ok()
                    .filter(|id| *id != x11rb::NONE)
            })
        });
        if let Some(loaded) = loaded {
            let _ = xcb.change_window_attributes(
                window,
                &xproto::ChangeWindowAttributesAux::new().cursor(loaded),
            );
            let _ = xcb.flush();
        }
    }
}

// The XKB state of a key event, from the modifiers the event carries: the shared state can lag behind the event.
fn event_state(xkb: &xkbc::State, mask: u16) -> xkbc::State {
    let mut state = xkbc::State::new(&xkb.get_keymap());
    let latched = xkb.serialize_mods(xkbc::STATE_MODS_LATCHED);
    let locked = xkb.serialize_mods(xkbc::STATE_MODS_LOCKED);
    // The low eight bits are the modifiers, the rest are buttons.
    let pressed = xkbc::ModMask::from(mask & 0xff) & !(latched | locked);
    let layout = xkb.serialize_layout(xkbc::STATE_LAYOUT_EFFECTIVE);
    state.update_mask(pressed, latched, locked, 0, 0, layout);
    state
}

fn button(detail: u32) -> Option<Button> {
    let buttons = [Button::Left, Button::Middle, Button::Right];
    match detail {
        1..=3 => Some(buttons[detail as usize - 1]),
        8 => Some(Button::Back),
        9 => Some(Button::Forward),
        _ => None,
    }
}

// Shift turns a vertical scroll into a horizontal one.
fn wheel_event(at: Point<Pixels>, delta: Point<f32>, modifiers: Modifiers) -> Event {
    let delta = if modifiers.shift() {
        point(delta.y, 0.)
    } else {
        delta
    };
    Event::Wheel {
        at,
        delta,
        lines: true,
        phase: Default::default(),
        modifiers,
    }
}

fn scrolled(scroller: &mut Scroller, motion: &xinput::MotionEvent) -> Option<Point<f32>> {
    let step = |valuator: &mut Valuator| {
        let number = usize::from(valuator.number?);
        let word = *motion.valuator_mask.get(number / 32)?;
        let bit = 1u32 << (number % 32);
        if word & bit == 0 {
            return None;
        }
        // Values come in the order of the set bits.
        let earlier: u32 = motion.valuator_mask[..number / 32]
            .iter()
            .map(|word| word.count_ones())
            .sum();
        let value = fixed(
            *motion
                .axisvalues
                .get((earlier + (word & (bit - 1)).count_ones()) as usize)?,
        );
        let lines = valuator
            .last
            .map(|last| (last - value) * valuator.lines_per_unit);
        valuator.last = Some(value);
        lines
    };
    let (x, y) = (step(&mut scroller.across), step(&mut scroller.down));
    (x.is_some() || y.is_some()).then(|| point(x.unwrap_or(0.), y.unwrap_or(0.)))
}

fn blank_cursor(xcb: &XCBConnection, root: xproto::Window) -> Result<xproto::Cursor> {
    let (pixmap, cursor) = (xcb.generate_id()?, xcb.generate_id()?);
    xcb.create_pixmap(1, pixmap, root, 1, 1)?;
    xcb.create_cursor(cursor, pixmap, pixmap, 0, 0, 0, 0, 0, 0, 0, 0)?;
    xcb.free_pixmap(pixmap)?;
    Ok(cursor)
}

impl Display for Client {
    fn fd(&self) -> RawFd {
        self.xcb.as_raw_fd()
    }

    // Replies to requests can read events into the queue of xcb, which doesn't wake `poll`, so each turn drains it.
    fn prepare(&self) -> bool {
        self.drain() && self.xcb.flush().is_ok()
    }

    fn finish(&self, _: bool) -> bool {
        self.drain() && self.xcb.flush().is_ok()
    }

    fn read_clipboard(&self) -> Option<String> {
        self.clipboard.paste()
    }

    fn write_clipboard(&self, text: &str) {
        self.clipboard.copy(text);
    }

    fn set_cursor(&self, cursor: Cursor) {
        let old = std::mem::replace(&mut self.live.borrow_mut().cursor, cursor);
        if old != cursor {
            self.show_cursor();
        }
    }

    fn display_bounds(&self) -> Option<Rect> {
        let screen = &self.xcb.setup().roots[self.screen];
        let [width, height] = [screen.width_in_pixels, screen.height_in_pixels]
            .map(|side| px(f32::from(side) / self.scale));
        Some(Rect::new(point(px(0.), px(0.)), size(width, height)))
    }

    fn open(self: Rc<Self>, options: &WindowOptions, calls: Callbacks) -> Result<Opened> {
        let (window, atlas) = Toplevel::open(&self, options, calls)?;
        Ok((window, atlas))
    }
}
