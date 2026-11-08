mod window;

pub(crate) use window::Window;

use super::{
    Clicks, DEFAULT_CURSOR, Ime, SCROLL_LINES, capslock_from_xkb, compose, compose_state,
    cursor_names, file_paths, ime_area, key_down, key_up, keystroke_from_xkb, modifiers_from_xkb,
    new_xkb_context, read_pipe, with_loop,
};
use crate::{
    Bounds, Capslock, CursorStyle, FileDropEvent, Keystroke, Modifiers, ModifiersChangedEvent,
    MouseButton, MouseDownEvent, MouseExitEvent, MouseMoveEvent, MouseUpEvent, NavigationDirection,
    Pixels, PlatformInput, Point, ScrollDelta, ScrollWheelEvent, point, px, size,
};
use anyhow::{Context as _, Result};
use calloop::{
    LoopHandle, PostAction,
    generic::Generic,
    timer::{TimeoutAction, Timer},
};
use calloop_wayland_source::WaylandSource;
use rustc_hash::FxHashMap;
use std::{
    cell::RefCell,
    fs::File,
    io::{ErrorKind, Read as _, Write as _},
    os::fd::{AsFd as _, AsRawFd as _, OwnedFd},
    rc::{Rc, Weak},
    time::Duration,
};
use wayland_backend::client::ObjectId;
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop, event_created_child,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{
        wl_buffer, wl_callback, wl_compositor, wl_data_device, wl_data_device_manager,
        wl_data_offer, wl_data_source, wl_keyboard, wl_output, wl_pointer, wl_region, wl_registry,
        wl_seat, wl_shm, wl_shm_pool, wl_surface,
    },
};
use wayland_cursor::CursorTheme;
use wayland_protocols::{
    wp::{
        cursor_shape::v1::client::{
            wp_cursor_shape_device_v1::{self, Shape},
            wp_cursor_shape_manager_v1,
        },
        fractional_scale::v1::client::{wp_fractional_scale_manager_v1, wp_fractional_scale_v1},
        text_input::zv3::client::{
            zwp_text_input_manager_v3,
            zwp_text_input_v3::{self, ContentHint, ContentPurpose},
        },
        viewporter::client::{wp_viewport, wp_viewporter},
    },
    xdg::{
        activation::v1::client::{xdg_activation_token_v1, xdg_activation_v1},
        decoration::zv1::client::{zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1},
        shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base},
    },
};
use xkbcommon::xkb;

const MIN_KEYCODE: u32 = 8;
const TEXT_MIME_TYPES: [&str; 3] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"];
const READ_MIME_TYPES: [&str; 2] = ["text/plain;charset=utf-8", "UTF8_STRING"];
const FILE_LIST_MIME_TYPE: &str = "text/uri-list";

#[derive(Clone)]
pub(crate) struct Handle(Weak<Client>);

pub(super) struct Globals {
    compositor: wl_compositor::WlCompositor,
    wm_base: xdg_wm_base::XdgWmBase,
    data_device_manager: Option<wl_data_device_manager::WlDataDeviceManager>,
    cursor_shape_manager: Option<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1>,
    viewporter: Option<wp_viewporter::WpViewporter>,
    fractional_scale_manager: Option<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1>,
    decoration_manager: Option<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>,
    text_input_manager: Option<zwp_text_input_manager_v3::ZwpTextInputManagerV3>,
    activation: Option<xdg_activation_v1::XdgActivationV1>,
}

#[derive(Clone, Default)]
pub(super) struct Output {
    position: Option<Point<i32>>,
    size: Option<crate::Size<i32>>,
    scale: i32,
}

struct Offer {
    offer: wl_data_offer::WlDataOffer,
    mime_types: Vec<String>,
}

#[derive(Default)]
struct Drag {
    offer: Option<wl_data_offer::WlDataOffer>,
    window: Option<ObjectId>,
    position: Point<Pixels>,
    generation: u64,
}

struct Repeat {
    rate: u32,
    delay: Duration,
    id: u64,
    key: Option<xkb::Keycode>,
}

struct ThemeCursor {
    theme: Option<(CursorTheme, i32)>,
    size: u32,
    shm: wl_shm::WlShm,
    surface: wl_surface::WlSurface,
}

struct State {
    windows: FxHashMap<ObjectId, Weak<window::Shared>>,
    outputs: FxHashMap<u32, Output>,
    output_globals: FxHashMap<u32, wl_output::WlOutput>,
    seat: wl_seat::WlSeat,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    cursor_shape: Option<wp_cursor_shape_device_v1::WpCursorShapeDeviceV1>,
    theme_cursor: ThemeCursor,
    cursor: CursorStyle,
    data_device: Option<wl_data_device::WlDataDevice>,
    text_input: Option<zwp_text_input_v3::ZwpTextInputV3>,
    enter_serial: u32,
    press: Option<(u32, wl_surface::WlSurface)>,
    keymap: Option<xkb::State>,
    compose: Option<xkb::compose::State>,
    pre_edit: Option<String>,
    ime_pre_edit: Option<String>,
    ime_commit: Option<String>,
    composing: Option<String>,
    repeat: Repeat,
    modifiers: Modifiers,
    capslock: Capslock,
    keyboard_focus: Option<ObjectId>,
    mouse_focus: Option<ObjectId>,
    mouse: Point<Pixels>,
    button: Option<MouseButton>,
    clicks: Clicks,
    axis_source: wl_pointer::AxisSource,
    continuous: Option<Point<Pixels>>,
    discrete: Option<Point<f32>>,
    offers: Vec<Offer>,
    selection: Option<usize>,
    clipboard: Option<String>,
    cached: Option<String>,
    drag: Drag,
    startup_token: Option<String>,
}

pub(crate) struct Client {
    conn: Connection,
    qh: QueueHandle<Handle>,
    globals: Globals,
    state: RefCell<State>,
    self_mime: String,
}

impl Client {
    pub fn new(handle: &LoopHandle<'static, ()>, need_decorations: bool) -> Result<Rc<Self>> {
        let conn = Connection::connect_to_env().context("connecting to the Wayland compositor")?;
        let (globals, mut queue) =
            registry_queue_init::<Handle>(&conn).context("reading the Wayland globals")?;
        let qh = queue.handle();
        let mut seat = None;
        let mut output_globals = FxHashMap::default();
        globals.contents().with_list(|list| {
            for global in list {
                match &global.interface[..] {
                    "wl_seat" if global.version >= 5 => {
                        seat = Some(globals.registry().bind::<wl_seat::WlSeat, _, _>(
                            global.name,
                            global.version.min(9),
                            &qh,
                            (),
                        ));
                    }
                    "wl_output" if global.version >= 2 => {
                        let output = globals.registry().bind::<wl_output::WlOutput, _, _>(
                            global.name,
                            global.version.min(4),
                            &qh,
                            (),
                        );
                        output_globals.insert(global.name, output);
                    }
                    _ => {}
                }
            }
        });
        let seat = seat.context("the Wayland compositor has no seat")?;
        let compositor: wl_compositor::WlCompositor = globals
            .bind(
                &qh,
                wl_surface::REQ_SET_BUFFER_SCALE_SINCE
                    ..=wl_surface::EVT_PREFERRED_BUFFER_SCALE_SINCE,
                (),
            )
            .context("the Wayland compositor has no wl_compositor")?;
        let shm: wl_shm::WlShm = globals
            .bind(&qh, 1..=1, ())
            .context("the Wayland compositor has no wl_shm")?;
        let data_device_manager: Option<wl_data_device_manager::WlDataDeviceManager> =
            globals.bind(&qh, 3..=3, ()).ok();
        let data_device = data_device_manager
            .as_ref()
            .map(|manager| manager.get_data_device(&seat, &qh, ()));
        let theme_cursor = ThemeCursor {
            theme: None,
            size: std::env::var("XCURSOR_SIZE")
                .ok()
                .and_then(|size| size.parse().ok())
                .unwrap_or(24),
            shm,
            surface: compositor.create_surface(&qh, ()),
        };
        let client = Rc::new(Self {
            globals: Globals {
                wm_base: globals
                    .bind(&qh, 1..=6, ())
                    .context("the Wayland compositor has no xdg_wm_base")?,
                data_device_manager,
                cursor_shape_manager: globals.bind(&qh, 1..=1, ()).ok(),
                viewporter: globals.bind(&qh, 1..=1, ()).ok(),
                fractional_scale_manager: globals.bind(&qh, 1..=1, ()).ok(),
                decoration_manager: globals.bind(&qh, 1..=1, ()).ok(),
                text_input_manager: globals.bind(&qh, 1..=1, ()).ok(),
                activation: globals.bind(&qh, 1..=1, ()).ok(),
                compositor,
            },
            state: RefCell::new(State {
                windows: FxHashMap::default(),
                outputs: output_globals
                    .values()
                    .map(|output| (output.id().protocol_id(), Output::default()))
                    .collect(),
                output_globals,
                seat,
                pointer: None,
                keyboard: None,
                cursor_shape: None,
                theme_cursor,
                cursor: CursorStyle::Arrow,
                data_device,
                text_input: None,
                enter_serial: 0,
                press: None,
                keymap: None,
                compose: None,
                pre_edit: None,
                ime_pre_edit: None,
                ime_commit: None,
                composing: None,
                repeat: Repeat {
                    rate: 16,
                    delay: Duration::from_millis(500),
                    id: 0,
                    key: None,
                },
                modifiers: Modifiers::default(),
                capslock: Capslock::default(),
                keyboard_focus: None,
                mouse_focus: None,
                mouse: Point::default(),
                button: None,
                clicks: Clicks::default(),
                axis_source: wl_pointer::AxisSource::Wheel,
                continuous: None,
                discrete: None,
                offers: Vec::new(),
                selection: None,
                clipboard: None,
                cached: None,
                drag: Drag::default(),
                // WARNING: removed from the environment below, so child processes don't take it.
                startup_token: std::env::var("XDG_ACTIVATION_TOKEN")
                    .ok()
                    .filter(|token| !token.is_empty()),
            }),
            self_mime: format!("pid/{}", std::process::id()),
            conn: conn.clone(),
            qh,
        });
        // SAFETY: the platform is made before any other thread starts.
        unsafe { std::env::remove_var("XDG_ACTIVATION_TOKEN") };
        anyhow::ensure!(
            !need_decorations || client.globals.decoration_manager.is_some(),
            "the Wayland compositor has no server decorations"
        );
        let mut dispatch = Handle(Rc::downgrade(&client));
        queue
            .roundtrip(&mut dispatch)
            .context("reading the Wayland state")?;
        handle
            .insert_source(WaylandSource::new(conn, queue), move |_, queue, _| {
                queue.dispatch_pending(&mut dispatch)
            })
            .map_err(|error| anyhow::anyhow!("registering the Wayland connection: {error}"))?;
        Ok(client)
    }

    fn window(&self, id: &ObjectId) -> Option<Rc<window::Shared>> {
        self.state.borrow().windows.get(id)?.upgrade()
    }

    fn forget(&self, id: &ObjectId) {
        let mut state = self.state.borrow_mut();
        state.windows.remove(id);
        if state.keyboard_focus.as_ref() == Some(id) {
            state.keyboard_focus = None;
        }
        if state.mouse_focus.as_ref() == Some(id) {
            state.mouse_focus = None;
        }
    }

    fn press_serial(&self) -> Option<u32> {
        self.state
            .borrow()
            .press
            .as_ref()
            .map(|(serial, _)| *serial)
    }

    fn consume_startup_token(&self, surface: &wl_surface::WlSurface) {
        if let (Some(activation), Some(token)) = (
            &self.globals.activation,
            self.state.borrow_mut().startup_token.take(),
        ) {
            activation.activate(token, surface);
        }
    }

    fn activate(&self, surface: &wl_surface::WlSurface) {
        let Some(activation) = &self.globals.activation else {
            return;
        };
        let token = activation.get_activation_token(&self.qh, surface.id());
        let state = self.state.borrow();
        match &state.press {
            Some((serial, pressed)) if pressed.is_alive() => {
                token.set_serial(*serial, &state.seat);
                token.set_surface(pressed);
            }
            _ => token.set_surface(surface),
        }
        token.commit();
    }

    fn enable_ime(&self) {
        let (text_input, window) = {
            let state = self.state.borrow();
            (
                state.text_input.clone(),
                state.focused(&state.keyboard_focus),
            )
        };
        let Some(text_input) = text_input.filter(|_| {
            window
                .as_ref()
                .is_none_or(|window| window.callbacks.accepts_text_input())
        }) else {
            return;
        };
        text_input.enable();
        text_input.set_content_type(ContentHint::None, ContentPurpose::Normal);
        if let Some(area) = window.and_then(|window| ime_area(window.callbacks)) {
            set_cursor_rectangle(&text_input, area);
        }
        text_input.commit();
    }

    fn cancel_composition(&self) {
        let text_input = {
            let mut state = self.state.borrow_mut();
            state.pre_edit = None;
            if let Some(compose) = &mut state.compose {
                compose.reset();
            }
            state.ime_pre_edit = None;
            state.ime_commit = None;
            state.composing.take().and(state.text_input.clone())
        };
        // WARNING: text-input-v3 has no reset, so the input method is turned off and on.
        if let Some(text_input) = text_input {
            text_input.disable();
            text_input.commit();
            self.enable_ime();
        }
    }

    pub fn primary_display_bounds(&self) -> Option<Bounds<Pixels>> {
        // WARNING: `Window::state()` calls this every frame, and a panic ends the app.
        let state = self.state.try_borrow().ok()?;
        let output = state
            .outputs
            .values()
            .find(|output| output.position.is_some() && output.size.is_some())?;
        let (position, size) = (output.position?, output.size?);
        let scale = output.scale.max(1) as f32;
        Some(Bounds::new(
            point(px(position.x as f32), px(position.y as f32)),
            crate::size(
                px(size.width as f32 / scale),
                px(size.height as f32 / scale),
            ),
        ))
    }

    pub fn set_cursor(&self, style: CursorStyle) {
        let mut state = self.state.borrow_mut();
        if state.cursor != style {
            state.cursor = style;
            state.apply_cursor(&self.conn);
        }
    }

    pub fn read_clipboard(&self) -> Option<String> {
        let mut state = self.state.borrow_mut();
        let offer = &state.offers[state.selection?];
        if offer.mime_types.contains(&self.self_mime) {
            return state.clipboard.clone();
        }
        if let Some(cached) = &state.cached {
            return Some(cached.clone());
        }
        let mime_type = offer
            .mime_types
            .iter()
            .find(|mime_type| READ_MIME_TYPES.contains(&mime_type.as_str()))?;
        let (reader, writer) = std::io::pipe().ok()?;
        offer.offer.receive(mime_type.clone(), writer.as_fd());
        drop(writer);
        let _ = self.conn.flush();
        let text = read_pipe(reader)
            .map_err(|error| eprintln!("apps: clipboard: {error:#}"))
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())?
            .replace("\r\n", "\n");
        state.cached = Some(text.clone());
        Some(text)
    }

    pub fn write_clipboard(&self, text: &str) {
        let mut state = self.state.borrow_mut();
        let (Some(manager), Some(device), Some((serial, _))) = (
            &self.globals.data_device_manager,
            &state.data_device,
            &state.press,
        ) else {
            return;
        };
        let source = manager.create_data_source(&self.qh, ());
        for mime_type in TEXT_MIME_TYPES {
            source.offer(mime_type.to_owned());
        }
        source.offer(self.self_mime.clone());
        device.set_selection(Some(&source), *serial);
        state.clipboard = Some(text.to_owned());
    }
}

impl State {
    fn apply_cursor(&mut self, conn: &Connection) {
        let serial = self.enter_serial;
        let Some(pointer) = &self.pointer else {
            return;
        };
        if self.cursor == CursorStyle::None {
            pointer.set_cursor(serial, None, 0, 0);
            return;
        }
        if let Some(device) = &self.cursor_shape {
            device.set_shape(serial, shape(self.cursor));
            return;
        }
        let scale = self
            .mouse_focus
            .as_ref()
            .and_then(|id| self.windows.get(id)?.upgrade())
            .map_or(1, |window| window.scale().ceil() as i32);
        let cursor = &mut self.theme_cursor;
        if cursor
            .theme
            .as_ref()
            .is_none_or(|(_, loaded)| *loaded != scale)
        {
            let size = cursor.size * scale as u32;
            let theme = match std::env::var("XCURSOR_THEME") {
                Ok(name) => CursorTheme::load_from_name(conn, cursor.shm.clone(), &name, size),
                Err(_) => CursorTheme::load(conn, cursor.shm.clone(), size),
            };
            cursor.theme = theme.ok().map(|theme| (theme, scale));
        }
        let Some((theme, _)) = &mut cursor.theme else {
            return;
        };
        let Some(name) = cursor_names(self.cursor)
            .iter()
            .chain([&DEFAULT_CURSOR])
            .find(|name| theme.get_cursor(name).is_some())
        else {
            return;
        };
        let Some(image) = theme.get_cursor(name).map(|cursor| &cursor[0]) else {
            return;
        };
        let (width, height) = image.dimensions();
        let (x, y) = image.hotspot();
        cursor.surface.set_buffer_scale(scale);
        pointer.set_cursor(
            serial,
            Some(&cursor.surface),
            x as i32 / scale,
            y as i32 / scale,
        );
        cursor.surface.attach(Some(image), 0, 0);
        cursor.surface.damage(0, 0, width as i32, height as i32);
        cursor.surface.commit();
    }

    fn focused(&self, id: &Option<ObjectId>) -> Option<Rc<window::Shared>> {
        self.windows.get(id.as_ref()?)?.upgrade()
    }
}

fn shape(style: CursorStyle) -> Shape {
    match style {
        CursorStyle::Arrow | CursorStyle::None => Shape::Default,
        CursorStyle::IBeam => Shape::Text,
        CursorStyle::Crosshair => Shape::Crosshair,
        CursorStyle::ClosedHand => Shape::Grabbing,
        CursorStyle::OpenHand => Shape::Grab,
        CursorStyle::PointingHand => Shape::Pointer,
        CursorStyle::ResizeLeft => Shape::WResize,
        CursorStyle::ResizeRight => Shape::EResize,
        CursorStyle::ResizeLeftRight => Shape::EwResize,
        CursorStyle::ResizeUp => Shape::NResize,
        CursorStyle::ResizeDown => Shape::SResize,
        CursorStyle::ResizeUpDown => Shape::NsResize,
        CursorStyle::ResizeUpLeftDownRight => Shape::NwseResize,
        CursorStyle::ResizeUpRightDownLeft => Shape::NeswResize,
        CursorStyle::ResizeColumn => Shape::ColResize,
        CursorStyle::ResizeRow => Shape::RowResize,
        CursorStyle::IBeamCursorForVerticalLayout => Shape::VerticalText,
        CursorStyle::OperationNotAllowed => Shape::NotAllowed,
        CursorStyle::DragLink => Shape::Alias,
        CursorStyle::DragCopy => Shape::Copy,
        CursorStyle::ContextualMenu => Shape::ContextMenu,
    }
}

fn send_bytes(fd: OwnedFd, bytes: Vec<u8>) {
    // SAFETY: fcntl on a pipe this process owns.
    unsafe {
        let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
        libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
    let mut written = 0;
    with_loop(|handle| {
        let _ = handle.insert_source(
            Generic::new(
                File::from(fd),
                calloop::Interest::WRITE,
                calloop::Mode::Level,
            ),
            move |_, file, _| {
                // SAFETY: the file is not dropped while it is registered.
                let file = unsafe { file.get_mut() };
                loop {
                    match file.write(&bytes[written..]) {
                        Ok(count) => {
                            written += count;
                            if written == bytes.len() || count == 0 {
                                return Ok(PostAction::Remove);
                            }
                        }
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {
                            return Ok(PostAction::Continue);
                        }
                        Err(_) => return Ok(PostAction::Remove),
                    }
                }
            },
        );
    });
}

impl Handle {
    fn client(&self) -> Option<Rc<Client>> {
        self.0.upgrade()
    }

    fn read_drag(
        &self,
        offer: wl_data_offer::WlDataOffer,
        window: ObjectId,
        position: Point<Pixels>,
        generation: u64,
    ) {
        let Ok((reader, writer)) = std::io::pipe() else {
            return;
        };
        offer.receive(FILE_LIST_MIME_TYPE.to_owned(), writer.as_fd());
        drop(writer);
        let handle = self.clone();
        let mut list = Vec::new();
        with_loop(|loop_handle| {
            let _ =
                loop_handle.insert_source(
                    Generic::new(reader, calloop::Interest::READ, calloop::Mode::Level),
                    move |_, reader, _| {
                        let mut chunk = [0; 8192];
                        // SAFETY: the reader is not dropped while it is registered.
                        match unsafe { reader.get_mut() }.read(&mut chunk) {
                            Ok(0) => {}
                            Ok(count) => {
                                list.extend_from_slice(&chunk[..count]);
                                return Ok(PostAction::Continue);
                            }
                            Err(error)
                                if error.kind() == ErrorKind::Interrupted
                                    || error.kind() == ErrorKind::WouldBlock =>
                            {
                                return Ok(PostAction::Continue);
                            }
                            Err(_) => return Ok(PostAction::Remove),
                        }
                        let Some(client) = handle.client() else {
                            return Ok(PostAction::Remove);
                        };
                        let paths = file_paths(&String::from_utf8_lossy(&list));
                        let mut state = client.state.borrow_mut();
                        if state.drag.generation != generation || paths.is_empty() {
                            offer.destroy();
                            return Ok(PostAction::Remove);
                        }
                        state.drag.offer = Some(offer.clone());
                        state.drag.window = Some(window.clone());
                        let window = state.focused(&state.drag.window);
                        drop(state);
                        if let Some(window) = window {
                            window.callbacks.input(PlatformInput::FileDrop(
                                FileDropEvent::Entered { position, paths },
                            ));
                        }
                        Ok(PostAction::Remove)
                    },
                );
        });
    }
}

fn set_cursor_rectangle(text_input: &zwp_text_input_v3::ZwpTextInputV3, area: Bounds<Pixels>) {
    text_input.set_cursor_rectangle(
        area.origin.x.0 as i32,
        area.origin.y.0 as i32,
        area.size.width.0 as i32,
        area.size.height.0 as i32,
    );
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Handle {
    fn event(
        this: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let Some(client) = this.client() else {
            return;
        };
        let mut state = client.state.borrow_mut();
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == "wl_output" && version >= 2 => {
                let output =
                    registry.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, ());
                state
                    .outputs
                    .insert(output.id().protocol_id(), Output::default());
                state.output_globals.insert(name, output);
            }
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(output) = state.output_globals.remove(&name) {
                    state.outputs.remove(&output.id().protocol_id());
                    if output.version() >= wl_output::REQ_RELEASE_SINCE {
                        output.release();
                    }
                }
            }
            _ => {}
        }
    }
}

delegate_noop!(Handle: ignore wl_compositor::WlCompositor);
delegate_noop!(Handle: ignore wl_shm::WlShm);
delegate_noop!(Handle: ignore wl_shm_pool::WlShmPool);
delegate_noop!(Handle: ignore wl_buffer::WlBuffer);
delegate_noop!(Handle: ignore wl_region::WlRegion);
delegate_noop!(Handle: ignore wl_data_device_manager::WlDataDeviceManager);
delegate_noop!(Handle: ignore wp_cursor_shape_manager_v1::WpCursorShapeManagerV1);
delegate_noop!(Handle: ignore wp_cursor_shape_device_v1::WpCursorShapeDeviceV1);
delegate_noop!(Handle: ignore wp_viewporter::WpViewporter);
delegate_noop!(Handle: ignore wp_viewport::WpViewport);
delegate_noop!(Handle: ignore wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1);
delegate_noop!(Handle: ignore zxdg_decoration_manager_v1::ZxdgDecorationManagerV1);
delegate_noop!(Handle: ignore zwp_text_input_manager_v3::ZwpTextInputManagerV3);
delegate_noop!(Handle: ignore xdg_activation_v1::XdgActivationV1);

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Handle {
    fn event(
        _: &mut Self,
        wm_base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for Handle {
    fn event(
        this: &mut Self,
        output: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(client) = this.client() else {
            return;
        };
        let mut state = client.state.borrow_mut();
        let Some(entry) = state.outputs.get_mut(&output.id().protocol_id()) else {
            return;
        };
        match event {
            wl_output::Event::Geometry { x, y, .. } => entry.position = Some(point(x, y)),
            wl_output::Event::Scale { factor } => entry.scale = factor,
            wl_output::Event::Mode {
                flags: WEnum::Value(flags),
                width,
                height,
                ..
            } if flags.contains(wl_output::Mode::Current) => {
                entry.size = Some(size(width, height));
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for Handle {
    fn event(
        this: &mut Self,
        surface: &wl_surface::WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(client) = this.client() else {
            return;
        };
        let Some(window) = client.window(&surface.id()) else {
            return;
        };
        let outputs = client.state.borrow().outputs.clone();
        window.handle_surface(event, &outputs);
    }
}

impl Dispatch<wl_callback::WlCallback, ObjectId> for Handle {
    fn event(
        this: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        surface: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event
            && let Some(window) = this.client().and_then(|client| client.window(surface))
        {
            window.frame_done();
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ObjectId> for Handle {
    fn event(
        this: &mut Self,
        _: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        surface: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event
            && let Some(window) = this.client().and_then(|client| client.window(surface))
        {
            window.handle_configure(serial);
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ObjectId> for Handle {
    fn event(
        this: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        surface: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let Some(window) = this.client().and_then(|client| client.window(surface)) {
            window.handle_toplevel(event);
        }
    }
}

impl Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, ObjectId> for Handle {
    fn event(
        this: &mut Self,
        _: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
        event: zxdg_toplevel_decoration_v1::Event,
        surface: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let Some(window) = this.client().and_then(|client| client.window(surface)) {
            window.handle_decoration(event);
        }
    }
}

impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, ObjectId> for Handle {
    fn event(
        this: &mut Self,
        _: &wp_fractional_scale_v1::WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        surface: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let Some(window) = this.client().and_then(|client| client.window(surface)) {
            window.handle_fractional(event);
        }
    }
}

impl Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, ObjectId> for Handle {
    fn event(
        this: &mut Self,
        token: &xdg_activation_token_v1::XdgActivationTokenV1,
        event: xdg_activation_token_v1::Event,
        surface: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_activation_token_v1::Event::Done { token: value } = event
            && let Some(client) = this.client()
            && let (Some(activation), Some(window)) =
                (&client.globals.activation, client.window(surface))
        {
            activation.activate(value, &window.surface);
        }
        token.destroy();
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Handle {
    fn event(
        this: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
        else {
            return;
        };
        let Some(client) = this.client() else {
            return;
        };
        let mut state = client.state.borrow_mut();
        if capabilities.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
            state.keyboard = Some(seat.get_keyboard(qh, ()));
            state.text_input = client
                .globals
                .text_input_manager
                .as_ref()
                .map(|manager| manager.get_text_input(seat, qh, ()));
        }
        if capabilities.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
            let pointer = seat.get_pointer(qh, ());
            state.cursor_shape = client
                .globals
                .cursor_shape_manager
                .as_ref()
                .map(|manager| manager.get_pointer(&pointer, qh, ()));
            state.pointer = Some(pointer);
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for Handle {
    fn event(
        this: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(client) = this.client() else {
            return;
        };
        let mut state = client.state.borrow_mut();
        match event {
            wl_keyboard::Event::RepeatInfo { rate, delay } => {
                state.repeat.rate = rate.max(0) as u32;
                state.repeat.delay = Duration::from_millis(delay.max(0) as u64);
            }
            wl_keyboard::Event::Keymap {
                format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
                fd,
                size,
            } => {
                let Ok(context) = new_xkb_context() else {
                    return;
                };
                // SAFETY: the compositor maps the keymap into the fd with this size.
                let keymap = unsafe {
                    xkb::Keymap::new_from_fd(
                        &context,
                        fd,
                        size as usize,
                        xkb::ffi::XKB_KEYMAP_FORMAT_TEXT_V1,
                        xkb::KEYMAP_COMPILE_NO_FLAGS,
                    )
                };
                match keymap {
                    Ok(Some(keymap)) => {
                        state.keymap = Some(xkb::State::new(&keymap));
                        state.compose = compose_state(&context);
                    }
                    _ => eprintln!("apps: the Wayland keymap can't be read"),
                }
            }
            wl_keyboard::Event::Enter { surface, .. } => {
                state.keyboard_focus = Some(surface.id());
                let window = state.focused(&state.keyboard_focus);
                drop(state);
                if let Some(window) = window {
                    window.set_active(true);
                }
            }
            wl_keyboard::Event::Leave { surface, .. } => {
                state.keyboard_focus = None;
                state.repeat.id += 1;
                if let Some(compose) = &mut state.compose {
                    compose.reset();
                }
                state.pre_edit = None;
                let window = state.windows.get(&surface.id()).and_then(Weak::upgrade);
                drop(state);
                // WARNING: like Chrome, the page ends its composition and keeps the text when the window goes.
                if let Some(window) = window {
                    window.set_active(false);
                }
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                let Some(keymap) = &mut state.keymap else {
                    return;
                };
                keymap.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                let (modifiers, capslock) = (modifiers_from_xkb(keymap), capslock_from_xkb(keymap));
                state.modifiers = modifiers;
                state.capslock = capslock;
                let window = state.focused(&state.keyboard_focus);
                drop(state);
                if let Some(window) = window {
                    window.callbacks.input(PlatformInput::ModifiersChanged(
                        ModifiersChangedEvent {
                            modifiers,
                            capslock,
                        },
                    ));
                }
            }
            wl_keyboard::Event::Key {
                serial,
                key,
                state: WEnum::Value(key_state),
                ..
            } => {
                let Some(window) = state.focused(&state.keyboard_focus) else {
                    return;
                };
                if key_state == wl_keyboard::KeyState::Pressed {
                    state.press = Some((serial, window.surface.clone()));
                }
                let Some(keymap) = &state.keymap else {
                    return;
                };
                let keycode = xkb::Keycode::from(key + MIN_KEYCODE);
                let keysym = keymap.key_get_one_sym(keycode);
                if keysym.is_modifier_key() {
                    return;
                }
                let mut keystroke = keystroke_from_xkb(keymap, state.modifiers, keycode);
                if key_state == wl_keyboard::KeyState::Released {
                    if state.repeat.key == Some(keycode) {
                        state.repeat.key = None;
                    }
                    drop(state);
                    key_up(window.callbacks, keystroke);
                    return;
                }
                let (commit, ime) = match state.compose.take() {
                    Some(mut composer) => {
                        let ime =
                            compose(&mut composer, &mut state.pre_edit, keysym, &mut keystroke);
                        state.compose = Some(composer);
                        ime
                    }
                    None => (None, Vec::new()),
                };
                state.repeat.id += 1;
                state.repeat.key = Some(keycode);
                let (id, rate, delay) = (state.repeat.id, state.repeat.rate, state.repeat.delay);
                drop(state);
                if rate > 0 && ime.is_empty() {
                    start_repeat(
                        this.clone(),
                        keystroke.clone(),
                        id,
                        delay,
                        Duration::from_secs(1) / rate,
                    );
                }
                if let Some(text) = commit {
                    Ime::Commit(text).apply(window.callbacks);
                }
                key_down(window.callbacks, keystroke, false, ime);
            }
            _ => {}
        }
    }
}

fn start_repeat(
    handle: Handle,
    keystroke: Keystroke,
    id: u64,
    delay: Duration,
    interval: Duration,
) {
    with_loop(|loop_handle| {
        let _ = loop_handle.insert_source(Timer::from_duration(delay), move |deadline, _, _| {
            let Some(client) = handle.client() else {
                return TimeoutAction::Drop;
            };
            let state = client.state.borrow();
            if state.repeat.id != id || state.repeat.key.is_none() {
                return TimeoutAction::Drop;
            }
            let Some(window) = state.focused(&state.keyboard_focus) else {
                return TimeoutAction::Drop;
            };
            drop(state);
            key_down(window.callbacks, keystroke.clone(), true, Vec::new());
            TimeoutAction::ToInstant(deadline + interval)
        });
    });
}

impl Dispatch<zwp_text_input_v3::ZwpTextInputV3, ()> for Handle {
    fn event(
        this: &mut Self,
        text_input: &zwp_text_input_v3::ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(client) = this.client() else {
            return;
        };
        match event {
            zwp_text_input_v3::Event::Enter { .. } => client.enable_ime(),
            zwp_text_input_v3::Event::Leave { .. } => {
                client.state.borrow_mut().composing = None;
                text_input.disable();
                text_input.commit();
            }
            zwp_text_input_v3::Event::CommitString { text } => {
                client.state.borrow_mut().ime_commit = text;
            }
            zwp_text_input_v3::Event::PreeditString { text, .. } => {
                client.state.borrow_mut().ime_pre_edit = text;
            }
            zwp_text_input_v3::Event::Done { .. } => {
                let (window, commit, preedit, shown) = {
                    let mut state = client.state.borrow_mut();
                    let commit = state.ime_commit.take();
                    let preedit = state.ime_pre_edit.take().filter(|text| !text.is_empty());
                    let shown = std::mem::replace(&mut state.composing, preedit.clone());
                    (state.focused(&state.keyboard_focus), commit, preedit, shown)
                };
                let Some(window) = window else {
                    return;
                };
                if shown.is_none()
                    && preedit.is_none()
                    && let Some(text) = commit.as_ref().filter(|text| text.len() == 1)
                {
                    let keystroke = Keystroke {
                        modifiers: Modifiers::default(),
                        key: text.clone(),
                        key_char: Some(text.clone()),
                    };
                    key_down(window.callbacks, keystroke, false, Vec::new());
                    return;
                }
                // WARNING: the input method takes its keys from the compositor, so like Chrome each change of
                // the text is a `Process` key without a code.
                let mut ime: Vec<_> = commit.map(Ime::Commit).into_iter().collect();
                match &preedit {
                    Some(text) if !ime.is_empty() || shown.as_ref() != Some(text) => {
                        ime.push(Ime::Preedit(text.clone()))
                    }
                    None if ime.is_empty() && shown.is_some() => ime.push(Ime::Clear),
                    _ => {}
                }
                if ime.is_empty() {
                    return;
                }
                key_down(window.callbacks, Keystroke::default(), false, ime);
                if preedit.is_some()
                    && let Some(area) = ime_area(window.callbacks)
                {
                    set_cursor_rectangle(text_input, area);
                    text_input.commit();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for Handle {
    fn event(
        this: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(client) = this.client() else {
            return;
        };
        let mut state = client.state.borrow_mut();
        match event {
            wl_pointer::Event::Enter {
                serial,
                surface,
                surface_x,
                surface_y,
            } => {
                state.enter_serial = serial;
                state.mouse = point(px(surface_x as f32), px(surface_y as f32));
                state.button = None;
                state.mouse_focus = Some(surface.id());
                state.apply_cursor(&client.conn);
                let (window, modifiers, position) = (
                    state.focused(&state.mouse_focus),
                    state.modifiers,
                    state.mouse,
                );
                drop(state);
                if let Some(window) = window {
                    window
                        .callbacks
                        .input(PlatformInput::MouseMove(MouseMoveEvent {
                            position,
                            pressed_button: None,
                            modifiers,
                        }));
                }
            }
            wl_pointer::Event::Leave { .. } => {
                let window = state.focused(&state.mouse_focus);
                state.button = None;
                let input = PlatformInput::MouseExited(MouseExitEvent {
                    position: state.mouse,
                });
                state.mouse_focus = None;
                drop(state);
                if let Some(window) = window {
                    window.callbacks.input(input);
                }
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                state.mouse = point(px(surface_x as f32), px(surface_y as f32));
                let window = state.focused(&state.mouse_focus);
                let input = PlatformInput::MouseMove(MouseMoveEvent {
                    position: state.mouse,
                    pressed_button: state.button,
                    modifiers: state.modifiers,
                });
                drop(state);
                if let Some(window) = window {
                    window.callbacks.input(input);
                }
            }
            wl_pointer::Event::Button {
                serial,
                button,
                state: WEnum::Value(button_state),
                ..
            } => {
                let Some(window) = state.focused(&state.mouse_focus) else {
                    return;
                };
                // WARNING: only presses count: requests that name a release serial are declined.
                if button_state == wl_pointer::ButtonState::Pressed {
                    state.press = Some((serial, window.surface.clone()));
                }
                let button = match button {
                    0x110 => MouseButton::Left,
                    0x111 => MouseButton::Right,
                    0x112 => MouseButton::Middle,
                    0x113 | 0x116 => MouseButton::Navigate(NavigationDirection::Back),
                    0x114 | 0x115 => MouseButton::Navigate(NavigationDirection::Forward),
                    _ => return,
                };
                let position = state.mouse;
                let modifiers = state.modifiers;
                if button_state == wl_pointer::ButtonState::Released {
                    state.button = None;
                    let click_count = state.clicks.count;
                    drop(state);
                    window.callbacks.input(PlatformInput::MouseUp(MouseUpEvent {
                        button,
                        position,
                        modifiers,
                        click_count,
                    }));
                    return;
                }
                state.button = Some(button);
                let click_count = state.clicks.press(button, position);
                let composing = state.composing.take().is_some() && state.text_input.is_some();
                let pre_edit = state.pre_edit.take();
                if pre_edit.is_some()
                    && let Some(compose) = &mut state.compose
                {
                    compose.reset();
                }
                let text_input = state.text_input.clone();
                drop(state);
                if composing && let Some(text_input) = text_input {
                    // WARNING: text-input-v3 has no reset, so the input method is turned off and on.
                    text_input.disable();
                    text_input.commit();
                    client.enable_ime();
                    Ime::Unmark.apply(window.callbacks);
                } else if let Some(text) = pre_edit {
                    Ime::Commit(text).apply(window.callbacks);
                }
                window
                    .callbacks
                    .input(PlatformInput::MouseDown(MouseDownEvent {
                        button,
                        position,
                        modifiers,
                        click_count,
                        first_mouse: false,
                    }));
            }
            wl_pointer::Event::AxisSource {
                axis_source: WEnum::Value(source),
            } => state.axis_source = source,
            wl_pointer::Event::Axis {
                axis: WEnum::Value(axis),
                value,
                ..
            } if state.axis_source != wl_pointer::AxisSource::Wheel => {
                let horizontal =
                    axis == wl_pointer::Axis::HorizontalScroll || state.modifiers.shift;
                let delta = state.continuous.get_or_insert_default();
                let value = px(-value as f32 * 3.);
                if horizontal {
                    delta.x += value;
                } else {
                    delta.y += value;
                }
            }
            wl_pointer::Event::AxisDiscrete {
                axis: WEnum::Value(axis),
                discrete,
            } => {
                state.add_discrete(axis, -discrete as f32);
            }
            wl_pointer::Event::AxisValue120 {
                axis: WEnum::Value(axis),
                value120,
            } => {
                state.add_discrete(axis, -value120 as f32 / 120.);
            }
            wl_pointer::Event::Frame => {
                let delta = match (state.continuous.take(), state.discrete.take()) {
                    (Some(pixels), _) => ScrollDelta::Pixels(pixels),
                    (None, Some(lines)) => ScrollDelta::Lines(lines),
                    (None, None) => return,
                };
                let window = state.focused(&state.mouse_focus);
                let input = PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position: state.mouse,
                    delta,
                    phase: Default::default(),
                    modifiers: state.modifiers,
                });
                drop(state);
                if let Some(window) = window {
                    window.callbacks.input(input);
                }
            }
            _ => {}
        }
    }
}

impl State {
    fn add_discrete(&mut self, axis: wl_pointer::Axis, steps: f32) {
        // WARNING: compositors send both kinds of steps; seats from version 8 only use value120.
        let horizontal = axis == wl_pointer::Axis::HorizontalScroll || self.modifiers.shift;
        let delta = self.discrete.get_or_insert_default();
        if horizontal {
            delta.x += steps * SCROLL_LINES;
        } else {
            delta.y += steps * SCROLL_LINES;
        }
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for Handle {
    fn event(
        this: &mut Self,
        _: &wl_data_device::WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(client) = this.client() else {
            return;
        };
        let mut state = client.state.borrow_mut();
        match event {
            wl_data_device::Event::DataOffer { id } => {
                if state.offers.len() >= 2 {
                    let old = state.offers.remove(0);
                    if state.selection == Some(0) {
                        state.selection = None;
                    } else if let Some(selection) = &mut state.selection {
                        *selection -= 1;
                    }
                    if state.drag.offer.as_ref() != Some(&old.offer) {
                        old.offer.destroy();
                    }
                }
                state.offers.push(Offer {
                    offer: id,
                    mime_types: Vec::new(),
                });
            }
            wl_data_device::Event::Selection { id } => {
                state.cached = None;
                state.selection =
                    id.and_then(|id| state.offers.iter().position(|offer| offer.offer == id));
            }
            wl_data_device::Event::Enter {
                serial,
                surface,
                x,
                y,
                id: Some(offer),
            } => {
                state.drag.generation += 1;
                let has_files = state.offers.iter().any(|entry| {
                    entry.offer == offer
                        && entry
                            .mime_types
                            .iter()
                            .any(|mime| mime == FILE_LIST_MIME_TYPE)
                });
                if !has_files || !state.windows.contains_key(&surface.id()) {
                    offer.accept(serial, None);
                    return;
                }
                offer.accept(serial, Some(FILE_LIST_MIME_TYPE.to_owned()));
                let copy = wl_data_device_manager::DndAction::Copy;
                offer.set_actions(copy, copy);
                let generation = state.drag.generation;
                drop(state);
                this.read_drag(
                    offer,
                    surface.id(),
                    point(px(x as f32), px(y as f32)),
                    generation,
                );
            }
            wl_data_device::Event::Motion { x, y, .. } => {
                let position = point(px(x as f32), px(y as f32));
                state.drag.position = position;
                let window = state.focused(&state.drag.window);
                drop(state);
                if let Some(window) = window {
                    window
                        .callbacks
                        .input(PlatformInput::FileDrop(FileDropEvent::Pending { position }));
                }
            }
            wl_data_device::Event::Leave | wl_data_device::Event::Drop => {
                state.drag.generation += 1;
                let id = state.drag.window.take();
                let window = state.focused(&id);
                let offer = state.drag.offer.take();
                let position = state.drag.position;
                drop(state);
                let (Some(window), Some(offer)) = (window, offer) else {
                    return;
                };
                let input = if matches!(event, wl_data_device::Event::Drop) {
                    offer.finish();
                    FileDropEvent::Submit { position }
                } else {
                    FileDropEvent::Exited
                };
                offer.destroy();
                window.callbacks.input(PlatformInput::FileDrop(input));
            }
            _ => {}
        }
    }

    event_created_child!(Handle, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ()),
    ]);
}

impl Dispatch<wl_data_offer::WlDataOffer, ()> for Handle {
    fn event(
        this: &mut Self,
        offer: &wl_data_offer::WlDataOffer,
        event: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event
            && let Some(client) = this.client()
            && let Some(entry) = client
                .state
                .borrow_mut()
                .offers
                .iter_mut()
                .find(|entry| entry.offer == *offer)
        {
            entry.mime_types.push(mime_type);
        }
    }
}

impl Dispatch<wl_data_source::WlDataSource, ()> for Handle {
    fn event(
        this: &mut Self,
        source: &wl_data_source::WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { fd, .. } => {
                if let Some(text) = this
                    .client()
                    .and_then(|client| client.state.borrow().clipboard.clone())
                {
                    send_bytes(fd, text.into_bytes());
                }
            }
            wl_data_source::Event::Cancelled => source.destroy(),
            _ => {}
        }
    }
}
