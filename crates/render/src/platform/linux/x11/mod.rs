//! X11 through xcb, with XInput 2 for the pointer, XKB for keys and XIM for input methods.

mod clipboard;
mod window;

pub(crate) use window::Window;

use super::{
    Clicks, DEFAULT_CURSOR, Ime, SCROLL_LINES, capslock_from_xkb, compose, compose_state,
    cursor_names, file_paths, ime_area, key_down, keystroke_from_xkb, modifiers_from_xkb,
    new_xkb_context,
};
use crate::{
    Bounds, Capslock, CursorStyle, FileDropEvent, KeyUpEvent, Modifiers, ModifiersChangedEvent,
    MouseButton, MouseDownEvent, MouseExitEvent, MouseMoveEvent, MouseUpEvent, NavigationDirection,
    Pixels, PlatformInput, Point, ScrollDelta, ScrollWheelEvent, TouchPhase, app::Callbacks, point,
    px, size,
};
use anyhow::{Context as _, Result, anyhow};
use calloop::{
    LoopHandle,
    generic::{FdWrapper, Generic},
};
use rustc_hash::FxHashMap;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::{Rc, Weak},
    time::Duration,
};
use x11rb::{
    connection::{Connection as _, RequestConnection as _},
    cookie::{Cookie, VoidCookie},
    errors::ConnectionError,
    protocol::{
        Event, randr,
        randr::ConnectionExt as _,
        xinput::{self, ConnectionExt as _},
        xkb::{self, ConnectionExt as _},
        xproto::{self, ConnectionExt as _},
    },
    resource_manager::Database,
    xcb_ffi::XCBConnection,
};
use xim::{AttributeName, Client as _, InputStyle, x11rb::X11rbClient};
use xkbcommon::xkb::{
    self as xkbc,
    x11::ffi::{XKB_X11_MIN_MAJOR_XKB_VERSION, XKB_X11_MIN_MINOR_XKB_VERSION},
};

x11rb::atom_manager! {
    pub(crate) Atoms: AtomsCookie {
        UTF8_STRING,
        WM_PROTOCOLS,
        WM_DELETE_WINDOW,
        WM_CHANGE_STATE,
        _NET_WM_PID,
        _NET_WM_NAME,
        _NET_WM_STATE,
        _NET_WM_STATE_MAXIMIZED_VERT,
        _NET_WM_STATE_MAXIMIZED_HORZ,
        _NET_WM_STATE_FULLSCREEN,
        _NET_WM_STATE_HIDDEN,
        _NET_WM_STATE_FOCUSED,
        _NET_ACTIVE_WINDOW,
        _NET_WM_SYNC_REQUEST,
        _NET_WM_SYNC_REQUEST_COUNTER,
        _NET_WM_MOVERESIZE,
        _MOTIF_WM_HINTS,
        XdndAware,
        XdndEnter,
        XdndLeave,
        XdndPosition,
        XdndStatus,
        XdndDrop,
        XdndFinished,
        XdndSelection,
        XdndTypeList,
        XdndActionCopy,
        TextUriList: b"text/uri-list",
        XDND_DATA,
    }
}

/// Selects every pointer device.
const ALL_DEVICES: xinput::DeviceId = 0;
/// Selects every device group, which reports the events of its devices.
const ALL_DEVICE_GROUPS: xinput::DeviceId = 1;

pub(crate) fn check<C: x11rb::connection::RequestConnection>(
    result: Result<VoidCookie<'_, C>, ConnectionError>,
) -> Result<()> {
    result?.check()?;
    Ok(())
}

pub(crate) fn reply<C: x11rb::connection::RequestConnection, O: x11rb::x11_utils::TryParse>(
    result: Result<Cookie<'_, C, O>, ConnectionError>,
) -> Result<O> {
    Ok(result?.reply()?)
}

/// The smooth scrolling axes of a pointer device.
#[derive(Default)]
struct Pointer {
    horizontal: Axis,
    vertical: Axis,
}

#[derive(Default)]
struct Axis {
    valuator: Option<u16>,
    /// Lines per scroll unit.
    multiplier: f32,
    /// The last value, `None` when it may be stale, like after the pointer left.
    value: Option<f32>,
}

/// An XDND drag of files onto one of our windows.
#[derive(Default)]
struct Drag {
    source: xproto::Window,
    supported: bool,
    retrieved: bool,
    position: Point<Pixels>,
}

/// Input method state, from the XIM callbacks.
#[derive(Default)]
struct XimHandler {
    im_id: u16,
    ic_id: u16,
    connected: bool,
    window: xproto::Window,
    event: Option<XimEvent>,
}

enum XimEvent {
    Key(Event),
    Preedit(xproto::Window, String),
    Commit(xproto::Window, String),
}

impl<C: xim::Client<XEvent = xproto::KeyPressEvent>> xim::ClientHandler<C> for XimHandler {
    fn handle_connect(&mut self, client: &mut C) -> Result<(), xim::ClientError> {
        client.open("C")
    }

    fn handle_open(&mut self, client: &mut C, im_id: u16) -> Result<(), xim::ClientError> {
        self.im_id = im_id;
        client.get_im_values(im_id, &[AttributeName::QueryInputStyle])
    }

    fn handle_get_im_values(
        &mut self,
        client: &mut C,
        im_id: u16,
        _: xim::AHashMap<AttributeName, Vec<u8>>,
    ) -> Result<(), xim::ClientError> {
        let attributes = client
            .build_ic_attributes()
            .push(AttributeName::InputStyle, InputStyle::PREEDIT_CALLBACKS)
            .push(AttributeName::ClientWindow, self.window)
            .push(AttributeName::FocusWindow, self.window)
            .build();
        client.create_ic(im_id, attributes)
    }

    fn handle_create_ic(&mut self, _: &mut C, _: u16, ic_id: u16) -> Result<(), xim::ClientError> {
        self.connected = true;
        self.ic_id = ic_id;
        Ok(())
    }

    fn handle_commit(
        &mut self,
        _: &mut C,
        _: u16,
        _: u16,
        text: &str,
    ) -> Result<(), xim::ClientError> {
        self.event = Some(XimEvent::Commit(self.window, text.to_owned()));
        Ok(())
    }

    fn handle_forward_event(
        &mut self,
        _: &mut C,
        _: u16,
        _: u16,
        _: xim::ForwardEventFlag,
        event: C::XEvent,
    ) -> Result<(), xim::ClientError> {
        self.event = match event.response_type {
            xproto::KEY_PRESS_EVENT => Some(XimEvent::Key(Event::KeyPress(event))),
            xproto::KEY_RELEASE_EVENT => Some(XimEvent::Key(Event::KeyRelease(event))),
            _ => self.event.take(),
        };
        Ok(())
    }

    fn handle_close(&mut self, client: &mut C, _: u16) -> Result<(), xim::ClientError> {
        client.disconnect()
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_preedit_draw(
        &mut self,
        _: &mut C,
        _: u16,
        _: u16,
        _: i32,
        _: i32,
        _: i32,
        _: xim::PreeditDrawStatus,
        text: &str,
        _: Vec<xim::Feedback>,
    ) -> Result<(), xim::ClientError> {
        self.event = Some(XimEvent::Preedit(self.window, text.to_owned()));
        Ok(())
    }
}

struct State {
    windows: FxHashMap<xproto::Window, Weak<window::Shared>>,
    keyboard_focus: Option<xproto::Window>,
    mouse_focus: Option<xproto::Window>,
    xkb_context: xkbc::Context,
    xkb_device: i32,
    xkb: xkbc::State,
    compose: Option<xkbc::compose::State>,
    pre_edit: Option<String>,
    composing: bool,
    xim: Option<(X11rbClient<Rc<XCBConnection>>, XimHandler)>,
    modifiers: Modifiers,
    last_modifiers: (Modifiers, Capslock),
    cursor_handle: x11rb::cursor::Handle,
    cursors: FxHashMap<CursorStyle, Option<xproto::Cursor>>,
    cursor: CursorStyle,
    pointers: BTreeMap<xinput::DeviceId, Pointer>,
    /// Keys held down, so X11 key repeats are seen as held.
    pressed: Vec<u8>,
    clicks: Clicks,
    drag: Drag,
}

pub(crate) struct Client {
    xcb: Rc<XCBConnection>,
    atoms: Atoms,
    screen: usize,
    root: xproto::Window,
    scale_factor: f32,
    /// How long one frame of the display takes.
    refresh: Duration,
    state: RefCell<State>,
    clipboard: clipboard::Clipboard,
}

impl Client {
    pub fn new(handle: &LoopHandle<'static, ()>) -> Result<Rc<Self>> {
        let (xcb, screen) = XCBConnection::connect(None).context("connecting to the X server")?;
        for extension in [
            xkb::X11_EXTENSION_NAME,
            randr::X11_EXTENSION_NAME,
            xinput::X11_EXTENSION_NAME,
        ] {
            xcb.prefetch_extension_information(extension)?;
        }
        let version = reply(xcb.xinput_xi_query_version(2, 2)).context("XInput is missing")?;
        anyhow::ensure!(version.major_version >= 2, "XInput 2 is required");
        let atoms = Atoms::new(&xcb)?.reply()?;
        let root = xcb.setup().roots[screen].root;

        let xkb_reply = reply(
            xcb.xkb_use_extension(XKB_X11_MIN_MAJOR_XKB_VERSION, XKB_X11_MIN_MINOR_XKB_VERSION),
        )?;
        anyhow::ensure!(xkb_reply.supported, "the X server does not support XKB");
        let parts = xkb::MapPart::KEY_TYPES
            | xkb::MapPart::KEY_SYMS
            | xkb::MapPart::MODIFIER_MAP
            | xkb::MapPart::EXPLICIT_COMPONENTS
            | xkb::MapPart::KEY_ACTIONS
            | xkb::MapPart::KEY_BEHAVIORS
            | xkb::MapPart::VIRTUAL_MODS
            | xkb::MapPart::VIRTUAL_MOD_MAP;
        check(xcb.xkb_select_events(
            xkb::ID::USE_CORE_KBD.into(),
            0u8.into(),
            xkb::EventType::STATE_NOTIFY
                | xkb::EventType::MAP_NOTIFY
                | xkb::EventType::NEW_KEYBOARD_NOTIFY,
            parts,
            parts,
            &xkb::SelectEventsAux::new(),
        ))?;
        let xkb_context = new_xkb_context()?;
        let xkb_device = xkbc::x11::get_core_keyboard_device_id(&xcb);
        let keymap = xkbc::x11::keymap_new_from_device(
            &xkb_context,
            &xcb,
            xkb_device,
            xkbc::KEYMAP_COMPILE_NO_FLAGS,
        );
        let xkb_state = xkbc::x11::state_new_from_device(&keymap, &xcb, xkb_device);
        let compose = compose_state(&xkb_context);

        let database = x11rb::resource_manager::new_from_default(&xcb)?;
        let scale_factor = scale_factor(&xcb, &database, root);
        let cursor_handle = x11rb::cursor::Handle::new(&xcb, screen, &database)?.reply()?;
        let refresh = refresh_interval(&xcb, root);
        let pointers = pointers(&xcb, &BTreeMap::new()).unwrap_or_default();
        let xcb = Rc::new(xcb);
        let xim = X11rbClient::init(xcb.clone(), screen, None)
            .ok()
            .map(|client| (client, XimHandler::default()));
        let client = Rc::new(Self {
            clipboard: clipboard::Clipboard::new()?,
            atoms,
            screen,
            root,
            scale_factor,
            refresh,
            state: RefCell::new(State {
                windows: FxHashMap::default(),
                keyboard_focus: None,
                mouse_focus: None,
                xkb_context,
                xkb_device,
                xkb: xkb_state,
                compose,
                pre_edit: None,
                composing: false,
                xim,
                modifiers: Modifiers::default(),
                last_modifiers: Default::default(),
                cursor_handle,
                cursors: FxHashMap::default(),
                cursor: CursorStyle::Arrow,
                pointers,
                pressed: Vec::new(),
                clicks: Clicks::default(),
                drag: Drag::default(),
            }),
            xcb: xcb.clone(),
        });
        // SAFETY: the connection's file descriptor lives as long as the connection.
        let fd = unsafe { FdWrapper::new(xcb) };
        let weak = Rc::downgrade(&client);
        handle
            .insert_source(
                Generic::new(fd, calloop::Interest::READ, calloop::Mode::Level),
                move |_, _, _| {
                    if let Some(client) = weak.upgrade() {
                        client.process_events();
                    }
                    Ok(calloop::PostAction::Continue)
                },
            )
            .map_err(|error| anyhow!("registering the X11 connection: {error}"))?;
        client.xcb.flush()?;
        Ok(client)
    }

    fn window(&self, id: xproto::Window) -> Option<Rc<window::Shared>> {
        self.state.borrow().windows.get(&id)?.upgrade()
    }

    pub fn read_clipboard(&self) -> Option<String> {
        self.clipboard
            .read()
            .map_err(|error| eprintln!("apps: clipboard: {error:#}"))
            .ok()
    }

    pub fn write_clipboard(&self, text: &str) {
        if let Err(error) = self.clipboard.write(text) {
            eprintln!("apps: clipboard: {error:#}");
        }
    }

    pub fn primary_display_bounds(&self) -> Option<Bounds<Pixels>> {
        let screen = &self.xcb.setup().roots[self.screen];
        Some(Bounds::new(
            point(px(0.), px(0.)),
            size(
                px(screen.width_in_pixels as f32 / self.scale_factor),
                px(screen.height_in_pixels as f32 / self.scale_factor),
            ),
        ))
    }

    pub fn set_cursor(&self, style: CursorStyle) {
        let mut state = self.state.borrow_mut();
        if state.cursor == style {
            return;
        }
        state.cursor = style;
        if let Some(window) = state.mouse_focus {
            state.apply_cursor(&self.xcb, window);
        }
    }

    /// Handles the events in the connection's queue.
    pub fn process_events(&self) {
        loop {
            let mut events = Vec::new();
            let mut exposed = Vec::new();
            let mut last_release = None;
            // Keymap changes refresh the whole state, so one of them is enough.
            let mut keymap_change = None;
            loop {
                let event = match self.xcb.poll_for_event() {
                    Ok(Some(event)) => event,
                    Ok(None) => break,
                    Err(error) => {
                        eprintln!("apps: X11: {error}");
                        if matches!(error, ConnectionError::IoError(_)) {
                            // WARNING: a lost display connection can't be recovered.
                            std::process::exit(1);
                        }
                        break;
                    }
                };
                match event {
                    Event::Expose(event) => exposed.push(event.window),
                    Event::KeyRelease(_) => {
                        if let Some(change) = keymap_change.take() {
                            events.extend(last_release.take());
                            events.push(change);
                        }
                        last_release = Some(event);
                    }
                    Event::KeyPress(press) => {
                        if let Some(change) = keymap_change.take() {
                            events.extend(last_release.take());
                            events.push(change);
                        }
                        // A release right before a press of the same key is X11 key repeat.
                        if let Some(Event::KeyRelease(release)) = last_release.take()
                            && (release.detail != press.detail
                                || press.time.saturating_sub(release.time) > 20)
                        {
                            events.push(Event::KeyRelease(release));
                        }
                        events.push(Event::KeyPress(press));
                    }
                    Event::XkbNewKeyboardNotify(_) | Event::XkbMapNotify(_) => {
                        events.extend(last_release.take());
                        keymap_change = Some(event);
                    }
                    _ => {
                        events.extend(last_release.take());
                        events.push(event);
                    }
                }
            }
            events.extend(last_release.take());
            events.extend(keymap_change.take());
            if events.is_empty() && exposed.is_empty() {
                return;
            }
            for event in events {
                self.filter_event(event);
            }
            exposed.dedup();
            for id in exposed {
                if let Some(window) = self.window(id) {
                    window.expose();
                }
            }
        }
    }

    /// Passes an event through the input method first.
    fn filter_event(&self, event: Event) {
        let Some((mut xim, mut handler)) = self.state.borrow_mut().xim.take() else {
            self.handle_event(event);
            return;
        };
        let connected = handler.connected;
        let filtered = xim.filter_event(&event, &mut handler);
        let callback = handler.event.take();
        self.state.borrow_mut().xim = Some((xim, handler));
        match callback {
            Some(XimEvent::Key(event)) => {
                self.handle_event(event);
            }
            Some(XimEvent::Commit(window, text)) => {
                self.state.borrow_mut().composing = false;
                if let Some(window) = self.window(window) {
                    Ime::Commit(text).apply(window.callbacks);
                }
            }
            Some(XimEvent::Preedit(window, text)) => {
                self.state.borrow_mut().composing = !text.is_empty();
                if let Some(window) = self.window(window) {
                    Ime::Preedit(text).apply(window.callbacks);
                    self.update_ime_position(window.callbacks);
                }
            }
            None => {}
        }
        match filtered {
            Ok(true) => {}
            Ok(false) if connected => self.forward_to_xim(event),
            Ok(false) => {
                self.handle_event(event);
            }
            // The input method crashed: its events are lost, and keys go straight to the windows.
            Err(error) => {
                eprintln!("apps: XIM: {error}");
                self.state.borrow_mut().xim = None;
                self.handle_event(event);
            }
        }
    }

    fn forward_to_xim(&self, event: Event) {
        let (Event::KeyPress(key) | Event::KeyRelease(key)) = event else {
            self.handle_event(event);
            return;
        };
        let Some((mut xim, mut handler)) = self.state.borrow_mut().xim.take() else {
            return;
        };
        handler.window = key.event;
        if let Err(error) = xim.forward_event(
            handler.im_id,
            handler.ic_id,
            xim::ForwardEventFlag::empty(),
            &key,
        ) {
            eprintln!("apps: XIM: {error}");
        }
        self.state.borrow_mut().xim = Some((xim, handler));
    }

    /// Moves the input method's candidates to the selection.
    fn update_ime_position(&self, callbacks: Callbacks) {
        let Some(area) = ime_area(callbacks) else {
            return;
        };
        let Some((mut xim, handler)) = self.state.borrow_mut().xim.take() else {
            return;
        };
        let spot =
            (area.origin + point(area.size.width, area.size.height)).scale(self.scale_factor);
        let attributes = xim
            .build_ic_attributes()
            .push(AttributeName::InputStyle, InputStyle::PREEDIT_CALLBACKS)
            .push(AttributeName::ClientWindow, handler.window)
            .push(AttributeName::FocusWindow, handler.window)
            .nested_list(AttributeName::PreeditAttributes, |builder| {
                builder.push(
                    AttributeName::SpotLocation,
                    xim::Point {
                        x: spot.x.0 as i16,
                        y: spot.y.0 as i16,
                    },
                );
            })
            .build();
        let _ = xim.set_ic_values(handler.im_id, handler.ic_id, attributes);
        self.state.borrow_mut().xim = Some((xim, handler));
    }

    fn enable_ime(&self, window: xproto::Window, callbacks: Callbacks) {
        let Some((mut xim, mut handler)) = self.state.borrow_mut().xim.take() else {
            return;
        };
        handler.window = window;
        let attributes = xim
            .build_ic_attributes()
            .push(AttributeName::InputStyle, InputStyle::PREEDIT_CALLBACKS)
            .push(AttributeName::ClientWindow, window)
            .push(AttributeName::FocusWindow, window)
            .build();
        let _ = xim.create_ic(handler.im_id, attributes);
        self.state.borrow_mut().xim = Some((xim, handler));
        self.update_ime_position(callbacks);
    }

    fn reset_ime(&self) {
        let mut state = self.state.borrow_mut();
        state.composing = false;
        if let Some((xim, handler)) = &mut state.xim {
            let _ = xim.reset_ic(handler.im_id, handler.ic_id);
        }
    }

    fn position(&self, x: xinput::Fp1616, y: xinput::Fp1616) -> Point<Pixels> {
        point(
            px(x as f32 / 65536. / self.scale_factor),
            px(y as f32 / 65536. / self.scale_factor),
        )
    }

    fn handle_event(&self, event: Event) -> Option<()> {
        match event {
            Event::MapNotify(event) => self.window(event.window)?.set_mapped(true),
            Event::UnmapNotify(event) => self.window(event.window)?.set_mapped(false),
            Event::VisibilityNotify(event) => self
                .window(event.window)?
                .set_obscured(event.state == xproto::Visibility::FULLY_OBSCURED),
            Event::ConfigureNotify(event) => {
                self.window(event.window)?.configure(
                    event.x.into(),
                    event.y.into(),
                    event.width.into(),
                    event.height.into(),
                );
            }
            Event::PropertyNotify(event) if event.atom == self.atoms._NET_WM_STATE => {
                self.window(event.window)?.read_wm_state();
            }
            Event::ClientMessage(event) => self.client_message(event)?,
            Event::SelectionNotify(event) if event.property == self.atoms.XDND_DATA => {
                let window = self.window(event.requestor)?;
                let data = reply(self.xcb.get_property(
                    false,
                    event.requestor,
                    self.atoms.XDND_DATA,
                    xproto::AtomEnum::ANY,
                    0,
                    1024 * 1024,
                ))
                .ok()?;
                let paths = file_paths(std::str::from_utf8(&data.value).ok()?);
                let position = {
                    let mut state = self.state.borrow_mut();
                    state.drag.retrieved = true;
                    state.drag.position
                };
                window
                    .callbacks
                    .input(PlatformInput::FileDrop(FileDropEvent::Entered {
                        position,
                        paths,
                    }));
            }
            Event::FocusIn(event) => {
                let window = self.window(event.event)?;
                self.state.borrow_mut().keyboard_focus = Some(event.event);
                window.set_active(true);
                self.enable_ime(event.event, window.callbacks);
            }
            Event::FocusOut(event) => {
                let window = self.window(event.event)?;
                {
                    let mut state = self.state.borrow_mut();
                    // The scroll valuators are global, so a scroll elsewhere must not become one big delta.
                    for pointer in state.pointers.values_mut() {
                        pointer.horizontal.value = None;
                        pointer.vertical.value = None;
                    }
                    state.keyboard_focus = None;
                    state.pressed.clear();
                    if let Some(compose) = &mut state.compose {
                        compose.reset();
                    }
                    state.pre_edit = None;
                }
                self.reset_ime();
                Ime::Clear.apply(window.callbacks);
                window.set_active(false);
            }
            Event::XkbNewKeyboardNotify(_) | Event::XkbMapNotify(_) => {
                let mut state = self.state.borrow_mut();
                let keymap = xkbc::x11::keymap_new_from_device(
                    &state.xkb_context,
                    &self.xcb,
                    state.xkb_device,
                    xkbc::KEYMAP_COMPILE_NO_FLAGS,
                );
                state.xkb = xkbc::x11::state_new_from_device(&keymap, &self.xcb, state.xkb_device);
            }
            Event::XkbStateNotify(event) => {
                let mut state = self.state.borrow_mut();
                state.xkb.update_mask(
                    event.base_mods.into(),
                    event.latched_mods.into(),
                    event.locked_mods.into(),
                    event.base_group as u32,
                    event.latched_group as u32,
                    event.locked_group.into(),
                );
                let modifiers = modifiers_from_xkb(&state.xkb);
                let capslock = capslock_from_xkb(&state.xkb);
                if state.last_modifiers == (modifiers, capslock) {
                    return Some(());
                }
                state.modifiers = modifiers;
                state.last_modifiers = (modifiers, capslock);
                let focus = state.keyboard_focus?;
                drop(state);
                self.window(focus)?
                    .callbacks
                    .input(PlatformInput::ModifiersChanged(ModifiersChangedEvent {
                        modifiers,
                        capslock,
                    }));
            }
            Event::KeyPress(event) => {
                let window = self.window(event.event)?;
                let mut state = self.state.borrow_mut();
                let modifiers = modifiers_from_state(event.state);
                state.modifiers = modifiers;
                let key_state = key_event_state(&state.xkb, event.state);
                let code = event.detail.into();
                let keysym = key_state.key_get_one_sym(code);
                if keysym.is_modifier_key() {
                    return Some(());
                }
                let mut keystroke = keystroke_from_xkb(&key_state, modifiers, code);
                let held = state.pressed.contains(&event.detail);
                if !held {
                    state.pressed.push(event.detail);
                }
                let ime = match state.compose.take() {
                    Some(mut composer) => {
                        let ime =
                            compose(&mut composer, &mut state.pre_edit, keysym, &mut keystroke);
                        state.compose = Some(composer);
                        ime
                    }
                    None => Vec::new(),
                };
                drop(state);
                for ime in ime {
                    ime.apply(window.callbacks);
                }
                key_down(window.callbacks, keystroke, held);
            }
            Event::KeyRelease(event) => {
                let window = self.window(event.event)?;
                let mut state = self.state.borrow_mut();
                let modifiers = modifiers_from_state(event.state);
                state.modifiers = modifiers;
                state.pressed.retain(|key| *key != event.detail);
                let key_state = key_event_state(&state.xkb, event.state);
                let code = event.detail.into();
                if key_state.key_get_one_sym(code).is_modifier_key() {
                    return Some(());
                }
                let keystroke = keystroke_from_xkb(&key_state, modifiers, code);
                drop(state);
                window
                    .callbacks
                    .input(PlatformInput::KeyUp(KeyUpEvent { keystroke }));
            }
            Event::XinputButtonPress(event) => {
                let window = self.window(event.event)?;
                let modifiers = modifiers_from_xinput(event.mods);
                let position = self.position(event.event_x, event.event_y);
                let (composing, pre_edit) = {
                    let mut state = self.state.borrow_mut();
                    state.modifiers = modifiers;
                    let pre_edit = state.pre_edit.take();
                    if pre_edit.is_some()
                        && let Some(compose) = &mut state.compose
                    {
                        compose.reset();
                    }
                    (state.composing && state.xim.is_some(), pre_edit)
                };
                // A click ends a composition.
                if composing {
                    self.reset_ime();
                    Ime::Unmark.apply(window.callbacks);
                } else if let Some(text) = pre_edit {
                    Ime::Commit(text).apply(window.callbacks);
                }
                let scroll = match event.detail {
                    4 => point(0., SCROLL_LINES),
                    5 => point(0., -SCROLL_LINES),
                    6 => point(SCROLL_LINES, 0.),
                    7 => point(-SCROLL_LINES, 0.),
                    detail => {
                        let button = button_from_detail(detail)?;
                        let click_count = self.state.borrow_mut().clicks.press(button, position);
                        window
                            .callbacks
                            .input(PlatformInput::MouseDown(MouseDownEvent {
                                button,
                                position,
                                modifiers,
                                click_count,
                                first_mouse: false,
                            }));
                        return Some(());
                    }
                };
                // Emulated wheel presses come with the smooth scroll motion events, which scroll instead.
                if !event
                    .flags
                    .contains(xinput::PointerEventFlags::POINTER_EMULATED)
                {
                    window
                        .callbacks
                        .input(scroll_event(position, scroll, modifiers));
                }
            }
            Event::XinputButtonRelease(event) => {
                let window = self.window(event.event)?;
                let button = button_from_detail(event.detail)?;
                let modifiers = modifiers_from_xinput(event.mods);
                let click_count = {
                    let mut state = self.state.borrow_mut();
                    state.modifiers = modifiers;
                    state.clicks.count
                };
                window.callbacks.input(PlatformInput::MouseUp(MouseUpEvent {
                    button,
                    position: self.position(event.event_x, event.event_y),
                    modifiers,
                    click_count,
                }));
            }
            Event::XinputMotion(event) => {
                let window = self.window(event.event)?;
                let position = self.position(event.event_x, event.event_y);
                let modifiers = modifiers_from_xinput(event.mods);
                let scroll = {
                    let mut state = self.state.borrow_mut();
                    state.modifiers = modifiers;
                    state
                        .pointers
                        .get_mut(&event.sourceid)
                        .and_then(|pointer| scroll_delta(pointer, &event))
                };
                if event.valuator_mask[0] & 3 != 0 {
                    window
                        .callbacks
                        .input(PlatformInput::MouseMove(MouseMoveEvent {
                            position,
                            pressed_button: pressed_button(event.button_mask[0]),
                            modifiers,
                        }));
                }
                if let Some(scroll) = scroll {
                    window
                        .callbacks
                        .input(scroll_event(position, scroll, modifiers));
                }
            }
            Event::XinputEnter(event) if event.mode == xinput::NotifyMode::NORMAL => {
                let mut state = self.state.borrow_mut();
                state.mouse_focus = Some(event.event);
                state.apply_cursor(&self.xcb, event.event);
            }
            Event::XinputLeave(event) if event.mode == xinput::NotifyMode::NORMAL => {
                let modifiers = modifiers_from_xinput(event.mods);
                {
                    let mut state = self.state.borrow_mut();
                    for pointer in state.pointers.values_mut() {
                        pointer.horizontal.value = None;
                        pointer.vertical.value = None;
                    }
                    state.mouse_focus = None;
                    state.modifiers = modifiers;
                }
                self.window(event.event)?
                    .callbacks
                    .input(PlatformInput::MouseExited(MouseExitEvent {
                        position: self.position(event.event_x, event.event_y),
                        pressed_button: pressed_button(event.buttons[0]),
                        modifiers,
                    }));
            }
            Event::XinputHierarchy(event) => {
                let mut state = self.state.borrow_mut();
                // A changed device's scroll values are stale.
                for info in event.infos {
                    if info.type_ == xinput::DeviceType::SLAVE_POINTER {
                        state.pointers.remove(&info.deviceid);
                    }
                }
                if let Some(pointers) = pointers(&self.xcb, &state.pointers) {
                    state.pointers = pointers;
                }
            }
            Event::XinputDeviceChanged(event) => {
                if let Some(pointer) = self.state.borrow_mut().pointers.get_mut(&event.sourceid) {
                    pointer.horizontal.value = None;
                    pointer.vertical.value = None;
                }
            }
            _ => {}
        }
        Some(())
    }

    fn client_message(&self, event: xproto::ClientMessageEvent) -> Option<()> {
        let window = self.window(event.window)?;
        let [atom, flags, arg2, arg3, arg4] = event.data.as_data32();
        let atoms = &self.atoms;
        if event.type_ == atoms.WM_PROTOCOLS && atom == atoms.WM_DELETE_WINDOW {
            if window.callbacks.should_close() {
                let id = window.callbacks.id;
                crate::app::update(move |app| {
                    app.update_window(id, |window, _| window.remove_window());
                });
            }
        } else if event.type_ == atoms.WM_PROTOCOLS && atom == atoms._NET_WM_SYNC_REQUEST {
            window.sync_request(x11rb::protocol::sync::Int64 {
                lo: arg2,
                hi: arg3 as i32,
            });
        } else if event.type_ == atoms.XdndEnter {
            let supported = if flags & 1 == 1 {
                reply(self.xcb.get_property(
                    false,
                    atom,
                    atoms.XdndTypeList,
                    xproto::AtomEnum::ANY,
                    0,
                    1024,
                ))
                .ok()
                .and_then(|types| {
                    types
                        .value32()
                        .map(|mut types| types.any(|kind| kind == atoms.TextUriList))
                })
                .unwrap_or(false)
            } else {
                [arg2, arg3, arg4].contains(&atoms.TextUriList)
            };
            self.state.borrow_mut().drag = Drag {
                source: atom,
                supported,
                ..Drag::default()
            };
        } else if event.type_ == atoms.XdndPosition {
            let mut state = self.state.borrow_mut();
            let (x, y) = ((arg2 >> 16) as i16, arg2 as i16);
            if let Ok(position) =
                reply(
                    self.xcb
                        .translate_coordinates(self.root, event.window, x, y),
                )
            {
                state.drag.position =
                    point(px(position.dst_x as f32), px(position.dst_y as f32)) / self.scale_factor;
            }
            if state.drag.supported && !state.drag.retrieved {
                let _ = self.xcb.convert_selection(
                    event.window,
                    atoms.XdndSelection,
                    atoms.TextUriList,
                    atoms.XDND_DATA,
                    arg3,
                );
            }
            let accept = state.drag.supported as u32;
            let (source, position) = (state.drag.source, state.drag.position);
            drop(state);
            self.send_xdnd(
                source,
                atoms.XdndStatus,
                [
                    event.window,
                    accept,
                    0,
                    0,
                    if accept == 1 { atoms.XdndActionCopy } else { 0 },
                ],
            );
            window
                .callbacks
                .input(PlatformInput::FileDrop(FileDropEvent::Pending { position }));
        } else if event.type_ == atoms.XdndLeave {
            let position = std::mem::take(&mut self.state.borrow_mut().drag).position;
            window
                .callbacks
                .input(PlatformInput::FileDrop(FileDropEvent::Pending { position }));
            window
                .callbacks
                .input(PlatformInput::FileDrop(FileDropEvent::Exited));
        } else if event.type_ == atoms.XdndDrop {
            let drag = std::mem::take(&mut self.state.borrow_mut().drag);
            self.send_xdnd(
                drag.source,
                atoms.XdndFinished,
                [
                    event.window,
                    drag.retrieved as u32,
                    atoms.XdndActionCopy,
                    0,
                    0,
                ],
            );
            let event = if drag.retrieved {
                FileDropEvent::Submit {
                    position: drag.position,
                }
            } else {
                FileDropEvent::Exited
            };
            window.callbacks.input(PlatformInput::FileDrop(event));
        }
        Some(())
    }

    fn send_xdnd(&self, target: xproto::Window, kind: xproto::Atom, data: [u32; 5]) {
        let message = xproto::ClientMessageEvent::new(32, target, kind, data);
        let _ = self
            .xcb
            .send_event(false, target, xproto::EventMask::NO_EVENT, message);
        let _ = self.xcb.flush();
    }

    /// Sends a `_NET_WM_STATE` or other root window message for `window`.
    fn send_wm_message(&self, window: xproto::Window, kind: xproto::Atom, data: [u32; 5]) {
        let message = xproto::ClientMessageEvent::new(32, window, kind, data);
        let _ = self.xcb.send_event(
            false,
            self.root,
            xproto::EventMask::SUBSTRUCTURE_REDIRECT | xproto::EventMask::SUBSTRUCTURE_NOTIFY,
            message,
        );
        let _ = self.xcb.flush();
    }
}

impl State {
    fn apply_cursor(&mut self, xcb: &XCBConnection, window: xproto::Window) {
        let style = self.cursor;
        let cursor = *self.cursors.entry(style).or_insert_with(|| {
            if style == CursorStyle::None {
                return invisible_cursor(xcb).ok();
            }
            cursor_names(style)
                .iter()
                .chain([&DEFAULT_CURSOR])
                .find_map(|name| {
                    self.cursor_handle
                        .load_cursor(xcb, name)
                        .ok()
                        .filter(|cursor| *cursor != x11rb::NONE)
                })
        });
        if let Some(cursor) = cursor {
            let _ = xcb.change_window_attributes(
                window,
                &xproto::ChangeWindowAttributesAux::new().cursor(cursor),
            );
            let _ = xcb.flush();
        }
    }
}

fn invisible_cursor(xcb: &XCBConnection) -> Result<xproto::Cursor> {
    let pixmap = xcb.generate_id()?;
    xcb.create_pixmap(1, pixmap, xcb.setup().roots[0].root, 1, 1)?;
    let cursor = xcb.generate_id()?;
    xcb.create_cursor(cursor, pixmap, pixmap, 0, 0, 0, 0, 0, 0, 0, 0)?;
    xcb.free_pixmap(pixmap)?;
    Ok(cursor)
}

fn scroll_event(position: Point<Pixels>, delta: Point<f32>, modifiers: Modifiers) -> PlatformInput {
    // Shift turns vertical scrolling horizontal.
    let delta = if modifiers.shift {
        point(delta.y, 0.)
    } else {
        delta
    };
    PlatformInput::ScrollWheel(ScrollWheelEvent {
        position,
        delta: ScrollDelta::Lines(delta),
        modifiers,
        touch_phase: TouchPhase::default(),
    })
}

fn button_from_detail(detail: u32) -> Option<MouseButton> {
    Some(match detail {
        1 => MouseButton::Left,
        2 => MouseButton::Middle,
        3 => MouseButton::Right,
        8 => MouseButton::Navigate(NavigationDirection::Back),
        9 => MouseButton::Navigate(NavigationDirection::Forward),
        _ => return None,
    })
}

fn pressed_button(mask: u32) -> Option<MouseButton> {
    Some(match mask {
        mask if mask & 2 != 0 => MouseButton::Left,
        mask if mask & 4 != 0 => MouseButton::Middle,
        mask if mask & 8 != 0 => MouseButton::Right,
        _ => return None,
    })
}

fn modifiers_from_state(state: xproto::KeyButMask) -> Modifiers {
    Modifiers {
        control: state.contains(xproto::KeyButMask::CONTROL),
        alt: state.contains(xproto::KeyButMask::MOD1),
        shift: state.contains(xproto::KeyButMask::SHIFT),
        platform: state.contains(xproto::KeyButMask::MOD4),
        function: false,
    }
}

fn modifiers_from_xinput(info: xinput::ModifierInfo) -> Modifiers {
    let mods = info.effective as u16;
    let has = |mask: xproto::ModMask| mods & u16::from(mask) == u16::from(mask);
    Modifiers {
        control: has(xproto::ModMask::CONTROL),
        alt: has(xproto::ModMask::M1),
        shift: has(xproto::ModMask::SHIFT),
        platform: has(xproto::ModMask::M4),
        function: false,
    }
}

/// The XKB state of a key event, whose modifiers can differ from the tracked state.
fn key_event_state(xkb: &xkbc::State, event: xproto::KeyButMask) -> xkbc::State {
    let mut state = xkbc::State::new(&xkb.get_keymap());
    let latched = xkb.serialize_mods(xkbc::STATE_MODS_LATCHED);
    let locked = xkb.serialize_mods(xkbc::STATE_MODS_LOCKED);
    let active: xkbc::ModMask = u16::from(
        event
            & (xproto::KeyButMask::SHIFT
                | xproto::KeyButMask::LOCK
                | xproto::KeyButMask::CONTROL
                | xproto::KeyButMask::MOD1
                | xproto::KeyButMask::MOD2
                | xproto::KeyButMask::MOD3
                | xproto::KeyButMask::MOD4
                | xproto::KeyButMask::MOD5),
    )
    .into();
    state.update_mask(
        active & !(latched | locked),
        latched,
        locked,
        xkb.serialize_layout(xkbc::STATE_LAYOUT_DEPRESSED),
        xkb.serialize_layout(xkbc::STATE_LAYOUT_LATCHED),
        xkb.serialize_layout(xkbc::STATE_LAYOUT_LOCKED),
    );
    state
}

fn fp3232(value: xinput::Fp3232) -> f32 {
    value.integral as f32 + value.frac as f32 / u32::MAX as f32
}

/// The pointer devices and their scroll axes, keeping the scroll values of `previous`.
fn pointers(
    xcb: &XCBConnection,
    previous: &BTreeMap<xinput::DeviceId, Pointer>,
) -> Option<BTreeMap<xinput::DeviceId, Pointer>> {
    let devices = reply(xcb.xinput_xi_query_device(ALL_DEVICES)).ok()?;
    Some(
        devices
            .infos
            .iter()
            .filter(|info| info.type_ == xinput::DeviceType::SLAVE_POINTER)
            .filter_map(|info| {
                let old = previous.get(&info.deviceid);
                let axis = |kind, old: Option<&Axis>| {
                    info.classes
                        .iter()
                        .rev()
                        .filter_map(|class| class.data.as_scroll())
                        .find(|scroll| scroll.scroll_type == kind)
                        .map(|scroll| Axis {
                            valuator: Some(scroll.number),
                            multiplier: SCROLL_LINES / fp3232(scroll.increment),
                            value: old.and_then(|axis| axis.value),
                        })
                };
                let horizontal = axis(
                    xinput::ScrollType::HORIZONTAL,
                    old.map(|old| &old.horizontal),
                );
                let vertical = axis(xinput::ScrollType::VERTICAL, old.map(|old| &old.vertical));
                (horizontal.is_some() || vertical.is_some()).then(|| {
                    (
                        info.deviceid,
                        Pointer {
                            horizontal: horizontal.unwrap_or_default(),
                            vertical: vertical.unwrap_or_default(),
                        },
                    )
                })
            })
            .collect(),
    )
}

/// The smooth scroll delta of a motion event, in lines.
fn scroll_delta(pointer: &mut Pointer, event: &xinput::MotionEvent) -> Option<Point<f32>> {
    let axis = |axis: &mut Axis| {
        let number = axis.valuator?;
        // The axis values hold one value for each set bit of the valuator mask, in order.
        let word = event.valuator_mask.get(number as usize / 32)?;
        if word & (1 << (number % 32)) == 0 {
            return None;
        }
        let before: u32 = event.valuator_mask[..number as usize / 32]
            .iter()
            .map(|bits| bits.count_ones())
            .sum::<u32>()
            + (word & ((1u32 << (number % 32)) - 1)).count_ones();
        let value = fp3232(*event.axisvalues.get(before as usize)?);
        let delta = axis.value.map(|old| (old - value) * axis.multiplier);
        axis.value = Some(value);
        delta
    };
    let x = axis(&mut pointer.horizontal);
    let y = axis(&mut pointer.vertical);
    (x.is_some() || y.is_some()).then(|| point(x.unwrap_or(0.), y.unwrap_or(0.)))
}

/// The display's refresh interval, from the first active RandR mode.
fn refresh_interval(xcb: &XCBConnection, root: xproto::Window) -> Duration {
    let mode = reply(xcb.randr_get_screen_resources_current(root))
        .ok()
        .and_then(|resources| {
            resources.crtcs.iter().find_map(|crtc| {
                let info = reply(xcb.randr_get_crtc_info(*crtc, x11rb::CURRENT_TIME)).ok()?;
                resources
                    .modes
                    .iter()
                    .find(|mode| mode.id == info.mode)
                    .copied()
            })
        });
    match mode {
        Some(mode) if mode.dot_clock != 0 && mode.htotal != 0 && mode.vtotal != 0 => {
            let millihertz =
                mode.dot_clock as u64 * 1000 / (mode.htotal as u64 * mode.vtotal as u64);
            Duration::from_micros(1_000_000_000 / millihertz.max(1))
        }
        _ => Duration::from_micros(16_667),
    }
}

/// `Xft.dpi`, or the DPI of the primary monitor, in steps of 1/12.
fn scale_factor(xcb: &XCBConnection, database: &Database, root: xproto::Window) -> f32 {
    if let Ok(Some(dpi)) = database.get_value::<f32>("Xft.dpi", "Xft.dpi") {
        return dpi / 96.;
    }
    let monitors = reply(xcb.randr_get_monitors(root, true))
        .map(|reply| reply.monitors)
        .unwrap_or_default();
    let mut monitors: Vec<_> = monitors
        .into_iter()
        .filter(|monitor| monitor.width_in_millimeters > 0 && monitor.height_in_millimeters > 0)
        .collect();
    monitors.sort_by_key(|monitor| !monitor.primary);
    let Some(monitor) = monitors.first() else {
        return 1.;
    };
    let pixels_per_mm = ((monitor.width as f64 * monitor.height as f64)
        / (monitor.width_in_millimeters as f64 * monitor.height_in_millimeters as f64))
        .sqrt();
    let scale = ((pixels_per_mm * 12. * 25.4 / 96.).round() / 12.).max(1.);
    if scale <= 20. { scale as f32 } else { 1. }
}
