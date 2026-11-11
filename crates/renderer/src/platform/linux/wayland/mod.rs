mod window;

use super::input::{self, Clicks, Composer, Ime, NOTCH_LINES};
use super::{Display, Native, run_after, soon, watch};
use crate::{Button, Cursor, Event, Key, Modifiers, Pixels, Point, Rect, WindowOptions};
use crate::{app::Callbacks, point, px, size};
use anyhow::{Context as _, Result};
use rustc_hash::FxHashMap;
use std::io::{ErrorKind, Read as _, Write as _};
use std::os::fd::{AsFd as _, AsRawFd as _, OwnedFd, RawFd};
use std::{cell::RefCell, rc::Rc, rc::Weak, time::Duration};
use wayland_backend::client::{ObjectId, ReadEventsGuard, WaylandError};
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::*;
use wayland_client::{Connection, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_cursor::CursorTheme;
use wayland_protocols::wp::cursor_shape::v1::client as shapes;
use wayland_protocols::wp::fractional_scale::v1::client as fractions;
use wayland_protocols::wp::text_input::zv3::client as texts;
use wayland_protocols::wp::viewporter::client as views;
use wayland_protocols::xdg::activation::v1::client as activation;
use wayland_protocols::xdg::decoration::zv1::client as decorations;
use wayland_protocols::xdg::shell::client::xdg_wm_base::{self, XdgWmBase};
use window::Surface;
use xkbcommon::xkb;

const URI_LIST: &str = "text/uri-list";
const TEXT_TYPES: [&str; 3] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"];
// The cursor shape protocol numbers its shapes from 1 in this order, by their CSS names.
const SHAPES: [&str; 31] = [
    "default",
    "context-menu",
    "help",
    "pointer",
    "progress",
    "wait",
    "cell",
    "crosshair",
    "text",
    "vertical-text",
    "alias",
    "copy",
    "move",
    "no-drop",
    "not-allowed",
    "grab",
    "grabbing",
    "e-resize",
    "n-resize",
    "ne-resize",
    "nw-resize",
    "s-resize",
    "se-resize",
    "sw-resize",
    "w-resize",
    "ew-resize",
    "ns-resize",
    "nesw-resize",
    "nwse-resize",
    "col-resize",
    "row-resize",
];

// The dispatch target of the event queue. Handlers only change the client, and queue calls into the app with
// `soon`, so the app never runs inside a dispatch.
pub(super) struct Hub(Weak<Client>);

macro_rules! route {
    ($($object:ty, $data:ty => $method:ident $(+ $opcode:path => $child:ty)?;)*) => {$(
        impl wayland_client::Dispatch<$object, $data> for Hub {
            fn event(
                hub: &mut Self,
                object: &$object,
                message: <$object as wayland_client::Proxy>::Event,
                data: &$data,
                _: &wayland_client::Connection,
                _: &wayland_client::QueueHandle<Self>,
            ) {
                if let Some(client) = hub.0.upgrade() {
                    client.$method(object, message, data);
                }
            }
            $(wayland_client::event_created_child!(Hub, $object, [$opcode => ($child, ())]);)?
        }
    )*};
}
pub(super) use route;

route! {
    wl_registry::WlRegistry, GlobalListContents => on_registry;
    XdgWmBase, () => on_base;
    wl_output::WlOutput, () => on_output;
    wl_seat::WlSeat, () => on_seat;
    wl_keyboard::WlKeyboard, () => on_keyboard;
    ZwpTextInputV3, () => on_text_input;
    wl_pointer::WlPointer, () => on_pointer;
    wl_data_device::WlDataDevice, () => on_device + wl_data_device::EVT_DATA_OFFER_OPCODE => WlDataOffer;
    WlDataOffer, () => on_offer;
    wl_data_source::WlDataSource, () => on_source;
    Token, ObjectId => on_token;
}

delegate_noop!(Hub: ignore wl_compositor::WlCompositor);
delegate_noop!(Hub: ignore wl_shm::WlShm);
delegate_noop!(Hub: ignore wl_shm_pool::WlShmPool);
delegate_noop!(Hub: ignore wl_buffer::WlBuffer);
delegate_noop!(Hub: ignore wl_region::WlRegion);
delegate_noop!(Hub: ignore wl_data_device_manager::WlDataDeviceManager);
delegate_noop!(Hub: ignore shapes::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1);
delegate_noop!(Hub: ignore Shaper);
delegate_noop!(Hub: ignore views::wp_viewporter::WpViewporter);
delegate_noop!(Hub: ignore views::wp_viewport::WpViewport);
delegate_noop!(Hub: ignore fractions::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1);
delegate_noop!(Hub: ignore decorations::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1);
delegate_noop!(Hub: ignore texts::zwp_text_input_manager_v3::ZwpTextInputManagerV3);
delegate_noop!(Hub: ignore activation::xdg_activation_v1::XdgActivationV1);

type Shaper = shapes::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1;
type ZwpTextInputV3 = texts::zwp_text_input_v3::ZwpTextInputV3;
type WlDataOffer = wl_data_offer::WlDataOffer;
type WlSurface = wl_surface::WlSurface;
type Token = activation::xdg_activation_token_v1::XdgActivationTokenV1;

fn maybe<I: Proxy + 'static>(list: &GlobalList, qh: &QueueHandle<Hub>, newest: u32) -> Option<I>
where
    Hub: wayland_client::Dispatch<I, ()>,
{
    list.bind(qh, 1..=newest, ()).ok()
}

struct Globals {
    compositor: wl_compositor::WlCompositor,
    wm_base: XdgWmBase,
    devices: Option<wl_data_device_manager::WlDataDeviceManager>,
    shapes: Option<shapes::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1>,
    viewporter: Option<views::wp_viewporter::WpViewporter>,
    fractions: Option<fractions::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1>,
    decorations: Option<decorations::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>,
    activation: Option<activation::xdg_activation_v1::XdgActivationV1>,
    typing: Option<texts::zwp_text_input_manager_v3::ZwpTextInputManagerV3>,
}

#[derive(Default)]
struct Screen {
    at: Option<Point<i32>>,
    pixels: Option<crate::Size<i32>>,
    scale: i32,
}

struct Keys {
    xkb: Option<xkb::State>,
    composer: Option<Composer>,
    modifiers: Modifiers,
    // Keys per second, 0 for none, and the wait before the first repeat.
    rate: u32,
    delay: Duration,
    // Bumped to stop the running repeat.
    repeat: u64,
    held: Option<xkb::Keycode>,
}

#[derive(Default)]
struct Typing {
    ime: Option<ZwpTextInputV3>,
    preedit: Option<String>,
    commit: Option<String>,
    shown: Option<String>,
}

struct Mouse {
    pointer: Option<wl_pointer::WlPointer>,
    shaper: Option<Shaper>,
    focus: Option<ObjectId>,
    at: Point<Pixels>,
    held: Option<Button>,
    clicks: Clicks,
    wheel: bool,
    pixels: Option<Point<f32>>,
    lines: Option<Point<f32>>,
    entered: u32,
    cursor: Cursor,
    // For compositors without cursor shapes: a theme at a scale, and the surface that shows it.
    theme: Option<(CursorTheme, i32)>,
    theme_size: u32,
    image: WlSurface,
}

struct Offer {
    offer: WlDataOffer,
    types: Vec<String>,
    // The compositor chose a drag action, without which `finish` is a protocol error.
    action: bool,
}

#[derive(Default)]
struct Drag {
    offer: Option<WlDataOffer>,
    reading: Option<WlDataOffer>,
    window: Option<ObjectId>,
    at: Point<Pixels>,
    // Bumped by every enter and leave, so a late read of an old drag is dropped.
    round: u64,
}

struct Live {
    windows: FxHashMap<ObjectId, Weak<Surface>>,
    screens: FxHashMap<u32, (wl_output::WlOutput, Screen)>,
    seat: wl_seat::WlSeat,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    focus: Option<ObjectId>,
    keys: Keys,
    typing: Typing,
    mouse: Mouse,
    // Compositors decline moves, activation and selections without the serial of a recent press.
    press: Option<(u32, WlSurface)>,
    device: Option<wl_data_device::WlDataDevice>,
    offers: Vec<Offer>,
    selection: Option<WlDataOffer>,
    copied: Option<String>,
    pasted: Option<String>,
    drag: Drag,
    startup: Option<String>,
}

impl Live {
    fn window(&self, id: Option<&ObjectId>) -> Option<Rc<Surface>> {
        self.windows.get(id?)?.upgrade()
    }

    fn focused(&self) -> Option<Callbacks> {
        Some(self.window(self.focus.as_ref())?.calls)
    }

    fn hovered(&self) -> Option<Callbacks> {
        Some(self.window(self.mouse.focus.as_ref())?.calls)
    }

    fn types(&self, offer: &WlDataOffer) -> &[String] {
        let found = self.offers.iter().find(|kept| kept.offer == *offer);
        found.map_or(&[], |kept| &kept.types)
    }

    // Destroys offers that nothing uses, keeping the newest, which waits for its selection or drag event.
    fn prune(&mut self) {
        let newest = self.offers.len().saturating_sub(1);
        let used = [&self.selection, &self.drag.offer, &self.drag.reading];
        let mut at = 0;
        self.offers.retain(|kept| {
            at += 1;
            let keep =
                at - 1 == newest || used.iter().any(|used| used.as_ref() == Some(&kept.offer));
            if !keep {
                kept.offer.destroy();
            }
            keep
        });
    }
}

pub(super) struct Client {
    me: Weak<Client>,
    conn: Connection,
    queue: RefCell<EventQueue<Hub>>,
    reading: RefCell<Option<ReadEventsGuard>>,
    qh: QueueHandle<Hub>,
    globals: Globals,
    shm: wl_shm::WlShm,
    live: RefCell<Live>,
    // Marks offers of this process, whose text is read without a pipe.
    own_type: String,
}

fn later(calls: Option<Callbacks>, event: Event) {
    if let Some(calls) = calls {
        soon(move || {
            calls.input(event);
        });
    }
}

impl Client {
    pub fn connect(needs_decorations: bool) -> Result<Rc<Self>> {
        let conn = Connection::connect_to_env().context("connecting to the Wayland compositor")?;
        let (list, queue) =
            registry_queue_init::<Hub>(&conn).context("reading the Wayland globals")?;
        let qh = queue.handle();
        let registry = list.registry();
        let mut seat = None;
        let mut screens = FxHashMap::default();
        list.contents().with_list(|globals| {
            for global in globals {
                let (name, version) = (global.name, global.version);
                match (global.interface.as_str(), version) {
                    ("wl_seat", 5..) => seat = Some(registry.bind(name, version.min(9), &qh, ())),
                    ("wl_output", 2..) => {
                        let output = registry.bind(name, version.min(4), &qh, ());
                        screens.insert(name, (output, Screen::default()));
                    }
                    _ => {}
                }
            }
        });
        let seat: wl_seat::WlSeat = seat.context("the Wayland compositor has no seat")?;
        let viewporter = maybe(&list, &qh, 1);
        let globals = Globals {
            compositor: list
                .bind(&qh, 4..=6, ())
                .context("the compositor has no wl_compositor")?,
            wm_base: list
                .bind(&qh, 1..=6, ())
                .context("the compositor has no xdg_wm_base")?,
            devices: list.bind(&qh, 3..=3, ()).ok(),
            shapes: maybe(&list, &qh, 1),
            // A fractional scale sizes the buffer through a viewport.
            fractions: viewporter.as_ref().and_then(|_| maybe(&list, &qh, 1)),
            viewporter,
            decorations: maybe(&list, &qh, 1),
            activation: maybe(&list, &qh, 1),
            typing: maybe(&list, &qh, 1),
        };
        anyhow::ensure!(
            !needs_decorations || globals.decorations.is_some(),
            "the Wayland compositor draws no window decorations"
        );
        let shm = list
            .bind(&qh, 1..=1, ())
            .context("the compositor has no wl_shm")?;
        let devices = globals.devices.as_ref();
        let device = devices.map(|manager| manager.get_data_device(&seat, &qh, ()));
        let image = globals.compositor.create_surface(&qh, ());
        let theme_size = std::env::var("XCURSOR_SIZE")
            .ok()
            .and_then(|size| size.parse().ok());
        let keys = Keys {
            xkb: None,
            composer: None,
            modifiers: Modifiers::NONE,
            rate: 25,
            delay: Duration::from_millis(600),
            repeat: 0,
            held: None,
        };
        let mouse = Mouse {
            pointer: None,
            shaper: None,
            focus: None,
            at: Point::default(),
            held: None,
            clicks: Clicks::default(),
            wheel: true,
            pixels: None,
            lines: None,
            entered: 0,
            cursor: Cursor::Default,
            theme: None,
            theme_size: theme_size.unwrap_or(24),
            image,
        };
        let startup = std::env::var("XDG_ACTIVATION_TOKEN")
            .ok()
            .filter(|token| !token.is_empty());
        let live = Live {
            windows: FxHashMap::default(),
            screens,
            seat,
            keyboard: None,
            focus: None,
            keys,
            typing: Typing::default(),
            mouse,
            press: None,
            device,
            offers: Vec::new(),
            selection: None,
            copied: None,
            pasted: None,
            drag: Drag::default(),
            startup,
        };
        let client = Rc::new_cyclic(|me| Self {
            me: me.clone(),
            live: RefCell::new(live),
            own_type: format!("application/x-apps-{}", std::process::id()),
            reading: RefCell::new(None),
            queue: RefCell::new(queue),
            shm,
            conn,
            qh,
            globals,
        });
        let mut hub = Hub(Rc::downgrade(&client));
        client
            .queue
            .borrow_mut()
            .roundtrip(&mut hub)
            .context("reading the Wayland state")?;
        Ok(client)
    }

    fn dispatch(&self) -> bool {
        let result = self
            .queue
            .borrow_mut()
            .dispatch_pending(&mut Hub(self.me.clone()));
        result
            .inspect_err(|error| eprintln!("apps: Wayland: {error}"))
            .is_ok()
    }

    fn forget(&self, id: &ObjectId) {
        let mut live = self.live.borrow_mut();
        live.windows.remove(id);
        if live.focus.as_ref() == Some(id) {
            live.focus = None;
        }
        if live.mouse.focus.as_ref() == Some(id) {
            live.mouse.focus = None;
        }
    }

    fn press_serial(&self) -> Option<u32> {
        self.live.borrow().press.as_ref().map(|(serial, _)| *serial)
    }

    fn activate(&self, surface: &WlSurface) {
        let Some(activation) = &self.globals.activation else {
            return;
        };
        let token = activation.get_activation_token(&self.qh, surface.id());
        let live = self.live.borrow();
        match &live.press {
            Some((serial, pressed)) if pressed.is_alive() => {
                token.set_serial(*serial, &live.seat);
                token.set_surface(pressed);
            }
            _ => token.set_surface(surface),
        }
        token.commit();
    }

    // It asks the app, so no dispatch may run it.
    fn enable_ime(&self) {
        let (ime, calls) = {
            let live = self.live.borrow();
            (live.typing.ime.clone(), live.focused())
        };
        let Some(ime) = ime else {
            return;
        };
        if calls.is_some_and(|calls| !calls.accepts_text_input()) {
            return;
        }
        ime.enable();
        use texts::zwp_text_input_v3::{ContentHint, ContentPurpose};
        let (hint, purpose) = (ContentHint::None, ContentPurpose::Normal);
        ime.set_content_type(hint, purpose);
        if let Some(caret) = calls.and_then(input::caret) {
            place(&ime, caret);
        }
        ime.commit();
    }

    // text-input v3 has no reset, so the input method goes off and on again.
    fn restart_ime(&self, ime: ZwpTextInputV3) {
        ime.disable();
        ime.commit();
        let client = self.me.clone();
        soon(move || {
            if let Some(client) = client.upgrade() {
                client.enable_ime();
            }
        });
    }

    fn cancel_composition(&self) {
        let ime = {
            let live = &mut *self.live.borrow_mut();
            if let Some(composer) = &mut live.keys.composer {
                composer.reset();
            }
            let typing = &mut live.typing;
            typing.preedit = None;
            typing.commit = None;
            typing.shown.take().and(typing.ime.clone())
        };
        if let Some(ime) = ime {
            self.restart_ime(ime);
        }
    }

    fn show_cursor(&self) {
        let live = &mut *self.live.borrow_mut();
        let serial = live.mouse.entered;
        let hovered = live.window(live.mouse.focus.as_ref());
        let window_scale = hovered.map_or(1, |window| window.scale().ceil() as i32);
        let mouse = &mut live.mouse;
        let Some(pointer) = &mouse.pointer else {
            return;
        };
        if mouse.cursor == Cursor::None {
            pointer.set_cursor(serial, None, 0, 0);
            return;
        }
        let names = input::cursor_names(mouse.cursor);
        if let Some(shaper) = &mouse.shaper {
            let css = names.split(' ').next().unwrap_or("default");
            let index = SHAPES.iter().position(|name| *name == css).unwrap_or(0);
            if let Ok(shape) = shapes::wp_cursor_shape_device_v1::Shape::try_from(index as u32 + 1)
            {
                shaper.set_shape(serial, shape);
            }
            return;
        }
        if mouse
            .theme
            .as_ref()
            .is_none_or(|(_, scale)| *scale != window_scale)
        {
            let size = mouse.theme_size * window_scale as u32;
            let theme = match std::env::var("XCURSOR_THEME") {
                Ok(name) => CursorTheme::load_from_name(&self.conn, self.shm.clone(), &name, size),
                Err(_) => CursorTheme::load(&self.conn, self.shm.clone(), size),
            };
            mouse.theme = theme.ok().map(|theme| (theme, window_scale));
        }
        let Some((theme, scale)) = &mut mouse.theme else {
            return;
        };
        let mut names = names.split(' ').chain(["default", "left_ptr"]);
        let Some(name) = names.find(|name| theme.get_cursor(name).is_some()) else {
            return;
        };
        let Some(frames) = theme.get_cursor(name) else {
            return;
        };
        let picture = &frames[0];
        let ([width, height], [x, y]) = (picture.dimensions().into(), picture.hotspot().into());
        let [width, height, x, y] = [width, height, x, y].map(|value: u32| value as i32);
        mouse.image.set_buffer_scale(*scale);
        pointer.set_cursor(serial, Some(&mouse.image), x / *scale, y / *scale);
        mouse.image.attach(Some(picture), 0, 0);
        mouse.image.damage(0, 0, width, height);
        mouse.image.commit();
    }

    fn read_drop(&self, offer: WlDataOffer, window: ObjectId, at: Point<Pixels>, round: u64) {
        let Ok((mut pipe, writer)) = std::io::pipe() else {
            return;
        };
        offer.receive(URI_LIST.to_owned(), writer.as_fd());
        drop(writer);
        self.live.borrow_mut().drag.reading = Some(offer.clone());
        let client = self.me.clone();
        let mut list = Vec::new();
        watch(pipe.as_raw_fd(), false, move || {
            let mut chunk = [0; 4096];
            match pipe.read(&mut chunk) {
                Ok(0) => {}
                Ok(count) => {
                    list.extend_from_slice(&chunk[..count]);
                    return true;
                }
                Err(error)
                    if matches!(error.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock) =>
                {
                    return true;
                }
                Err(_) => {}
            }
            let Some(client) = client.upgrade() else {
                return false;
            };
            let paths = input::uri_paths(&String::from_utf8_lossy(&list));
            let mut live = client.live.borrow_mut();
            live.drag.reading = None;
            if live.drag.round != round || paths.is_empty() {
                offer.destroy();
                live.offers.retain(|kept| kept.offer != offer);
                return false;
            }
            live.drag.offer = Some(offer.clone());
            live.drag.window = Some(window.clone());
            let calls = live.window(Some(&window)).map(|target| target.calls);
            drop(live);
            if let Some(calls) = calls {
                calls.input(Event::FileEnter { at, paths });
            }
            false
        });
    }
}

fn place(ime: &ZwpTextInputV3, caret: Rect) {
    let corner = caret.origin.map(|value| value.0 as i32);
    let extent = caret.size.map(|value| value.0 as i32);
    ime.set_cursor_rectangle(corner.x, corner.y, extent.width, extent.height);
}

fn send(fd: OwnedFd, data: Vec<u8>) {
    // SAFETY: fcntl on a pipe that this process owns.
    unsafe {
        let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
        libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
    let raw = fd.as_raw_fd();
    let mut file = std::fs::File::from(fd);
    let mut sent = 0;
    watch(raw, true, move || {
        loop {
            match file.write(&data[sent..]) {
                Ok(0) => return false,
                Ok(count) => {
                    sent += count;
                    if sent == data.len() {
                        return false;
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => return true,
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(_) => return false,
            }
        }
    });
}

fn repeat(
    client: Weak<Client>,
    round: u64,
    wait: Duration,
    every: Duration,
    key: Key,
    modifiers: Modifiers,
) {
    run_after(wait, move || {
        let Some(strong) = client.upgrade() else {
            return;
        };
        let calls = {
            let live = strong.live.borrow();
            if live.keys.repeat != round || live.keys.held.is_none() {
                return;
            }
            live.focused()
        };
        if let Some(calls) = calls {
            input::press(calls, key.clone(), modifiers, true, Vec::new());
            repeat(client, round, every, every, key, modifiers);
        }
    });
}

// Shift turns vertical scrolls horizontal.
fn scroll(delta: &mut Option<Point<f32>>, axis: WEnum<wl_pointer::Axis>, amount: f32, shift: bool) {
    let delta = delta.get_or_insert_default();
    match axis == WEnum::Value(wl_pointer::Axis::HorizontalScroll) || shift {
        true => delta.x += amount,
        false => delta.y += amount,
    }
}

impl Client {
    fn on_registry(
        &self,
        registry: &wl_registry::WlRegistry,
        message: wl_registry::Event,
        _: &GlobalListContents,
    ) {
        let mut live = self.live.borrow_mut();
        match message {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == "wl_output" && version >= 2 => {
                let output = registry.bind(name, version.min(4), &self.qh, ());
                live.screens.insert(name, (output, Screen::default()));
            }
            wl_registry::Event::GlobalRemove { name } => {
                if let Some((output, _)) = live.screens.remove(&name)
                    && output.version() >= 3
                {
                    output.release();
                }
            }
            _ => {}
        }
    }

    fn on_base(&self, base: &xdg_wm_base::XdgWmBase, message: xdg_wm_base::Event, _: &()) {
        if let xdg_wm_base::Event::Ping { serial } = message {
            base.pong(serial);
        }
    }

    fn on_output(&self, output: &wl_output::WlOutput, message: wl_output::Event, _: &()) {
        use wl_output::Event as Out;
        let mut live = self.live.borrow_mut();
        let Some((_, screen)) = live.screens.values_mut().find(|(kept, _)| kept == output) else {
            return;
        };
        match message {
            Out::Geometry { x, y, .. } => screen.at = Some(point(x, y)),
            Out::Scale { factor } => screen.scale = factor,
            Out::Mode {
                flags: WEnum::Value(flags),
                width,
                height,
                ..
            } if flags.contains(wl_output::Mode::Current) => {
                screen.pixels = Some(size(width, height));
            }
            _ => {}
        }
    }

    fn on_seat(&self, seat: &wl_seat::WlSeat, message: wl_seat::Event, _: &()) {
        let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(has),
        } = message
        else {
            return;
        };
        let (qh, globals) = (&self.qh, &self.globals);
        let live = &mut *self.live.borrow_mut();
        if has.contains(wl_seat::Capability::Keyboard) && live.keyboard.is_none() {
            live.keyboard = Some(seat.get_keyboard(qh, ()));
            live.typing.ime = globals
                .typing
                .as_ref()
                .map(|manager| manager.get_text_input(seat, qh, ()));
        }
        if has.contains(wl_seat::Capability::Pointer) && live.mouse.pointer.is_none() {
            let made = seat.get_pointer(qh, ());
            live.mouse.shaper = globals
                .shapes
                .as_ref()
                .map(|shapes| shapes.get_pointer(&made, qh, ()));
            live.mouse.pointer = Some(made);
        }
    }

    fn on_keyboard(&self, _: &wl_keyboard::WlKeyboard, message: wl_keyboard::Event, _: &()) {
        use wl_keyboard::Event as Kb;
        let live = &mut *self.live.borrow_mut();
        match message {
            Kb::RepeatInfo { rate, delay } => {
                live.keys.rate = rate.max(0) as u32;
                live.keys.delay = Duration::from_millis(delay.max(0) as u64);
            }
            Kb::Keymap {
                format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
                fd,
                size,
            } => {
                let Ok(context) = input::xkb_context() else {
                    return;
                };
                let (format, flags) = (xkb::KEYMAP_FORMAT_TEXT_V1, xkb::KEYMAP_COMPILE_NO_FLAGS);
                // SAFETY: the compositor shares a keymap of this size through the descriptor.
                match unsafe {
                    xkb::Keymap::new_from_fd(&context, fd, size as usize, format, flags)
                } {
                    Ok(Some(keymap)) => {
                        live.keys.xkb = Some(xkb::State::new(&keymap));
                        live.keys.composer = Some(Composer::new(&context));
                    }
                    _ => eprintln!("apps: the Wayland keymap can't be read"),
                }
            }
            Kb::Enter { surface, .. } => {
                live.focus = Some(surface.id());
                if let Some(window) = live.window(live.focus.as_ref()) {
                    window.focus(true);
                }
            }
            Kb::Leave { surface, .. } => {
                live.focus = None;
                live.keys.repeat += 1;
                if let Some(composer) = &mut live.keys.composer {
                    composer.reset();
                }
                if let Some(window) = live.window(Some(&surface.id())) {
                    window.focus(false);
                }
            }
            Kb::Modifiers {
                mods_depressed: down,
                mods_latched: latched,
                mods_locked: locked,
                group,
                ..
            } => {
                let Some(xkb) = &mut live.keys.xkb else {
                    return;
                };
                xkb.update_mask(down, latched, locked, 0, 0, group);
                let (modifiers, caps_lock) = input::held(xkb);
                live.keys.modifiers = modifiers;
                later(
                    live.focused(),
                    Event::ModifiersChanged {
                        modifiers,
                        caps_lock,
                    },
                );
            }
            Kb::Key {
                serial,
                key,
                state: WEnum::Value(direction),
                ..
            } => {
                let Some(window) = live.window(live.focus.as_ref()) else {
                    return;
                };
                let down = direction == wl_keyboard::KeyState::Pressed;
                if down {
                    live.press = Some((serial, window.surface().clone()));
                }
                let keys = &mut live.keys;
                let Some(xkb) = &keys.xkb else {
                    return;
                };
                // Wayland key codes are evdev codes, which XKB numbers from 8.
                let code = xkb::Keycode::new(key + 8);
                let sym = xkb.key_get_one_sym(code);
                if sym.is_modifier_key() {
                    return;
                }
                let (mut pressed, modifiers) = input::key(xkb, code, keys.modifiers);
                let calls = window.calls;
                if !down {
                    if keys.held == Some(code) {
                        keys.held = None;
                    }
                    soon(move || input::release(calls, pressed, modifiers));
                    return;
                }
                let (before, changes) = match &mut keys.composer {
                    Some(composer) => composer.feed(sym, &mut pressed),
                    None => (None, Vec::new()),
                };
                keys.repeat += 1;
                keys.held = Some(code);
                if keys.rate > 0 && changes.is_empty() {
                    let every = Duration::from_secs(1) / keys.rate;
                    repeat(
                        self.me.clone(),
                        keys.repeat,
                        keys.delay,
                        every,
                        pressed.clone(),
                        modifiers,
                    );
                }
                soon(move || {
                    if let Some(text) = before {
                        Ime::Commit(text).apply(calls);
                    }
                    input::press(calls, pressed, modifiers, false, changes);
                });
            }
            _ => {}
        }
    }

    fn on_text_input(
        &self,
        ime: &ZwpTextInputV3,
        message: texts::zwp_text_input_v3::Event,
        _: &(),
    ) {
        use texts::zwp_text_input_v3::Event as Im;
        let live = &mut *self.live.borrow_mut();
        match message {
            Im::Enter { .. } => {
                let client = self.me.clone();
                soon(move || {
                    if let Some(client) = client.upgrade() {
                        client.enable_ime();
                    }
                });
            }
            Im::Leave { .. } => {
                live.typing.shown = None;
                ime.disable();
                ime.commit();
            }
            Im::PreeditString { text, .. } => live.typing.preedit = text,
            Im::CommitString { text } => live.typing.commit = text,
            Im::Done { .. } => {
                let typing = &mut live.typing;
                let commit = typing.commit.take();
                let preedit = typing.preedit.take().filter(|text| !text.is_empty());
                let shown = std::mem::replace(&mut typing.shown, preedit.clone());
                let (Some(calls), ime) = (live.focused(), ime.clone()) else {
                    return;
                };
                soon(move || {
                    // One byte typed outside a composition is a plain key, which shortcuts and the page see.
                    if let (None, None, Some(text)) = (
                        &shown,
                        &preedit,
                        commit.as_ref().filter(|text| text.len() == 1),
                    ) {
                        let key = Key {
                            name: text.clone(),
                            text: Some(text.clone()),
                        };
                        input::press(calls, key, Modifiers::NONE, false, Vec::new());
                        return;
                    }
                    let mut changes: Vec<_> = commit.map(Ime::Commit).into_iter().collect();
                    match &preedit {
                        Some(text) if !changes.is_empty() || shown.as_ref() != Some(text) => {
                            changes.push(Ime::Compose(text.clone()));
                        }
                        None if changes.is_empty() && shown.is_some() => changes.push(Ime::Clear),
                        _ => {}
                    }
                    if changes.is_empty() {
                        return;
                    }
                    input::press(calls, Key::default(), Modifiers::NONE, false, changes);
                    if preedit.is_some()
                        && let Some(caret) = input::caret(calls)
                    {
                        place(&ime, caret);
                        ime.commit();
                    }
                });
            }
            _ => {}
        }
    }

    fn on_pointer(&self, _: &wl_pointer::WlPointer, message: wl_pointer::Event, _: &()) {
        use wl_pointer::Event as Ptr;
        let mut live = self.live.borrow_mut();
        let modifiers = live.keys.modifiers;
        let shift = modifiers.shift();
        let place = |x: f64, y: f64| point(px(x as f32), px(y as f32));
        match message {
            Ptr::Enter {
                serial,
                surface,
                surface_x,
                surface_y,
            } => {
                let mouse = &mut live.mouse;
                (mouse.entered, mouse.held) = (serial, None);
                mouse.at = place(surface_x, surface_y);
                mouse.focus = Some(surface.id());
                let at = mouse.at;
                later(
                    live.hovered(),
                    Event::PointerMove {
                        at,
                        held: None,
                        modifiers,
                    },
                );
                drop(live);
                self.show_cursor();
            }
            Ptr::Leave { .. } => {
                later(live.hovered(), Event::PointerLeave { at: live.mouse.at });
                (live.mouse.focus, live.mouse.held) = (None, None);
            }
            Ptr::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                let mouse = &mut live.mouse;
                mouse.at = place(surface_x, surface_y);
                let (at, held) = (mouse.at, mouse.held);
                later(
                    live.hovered(),
                    Event::PointerMove {
                        at,
                        held,
                        modifiers,
                    },
                );
            }
            Ptr::Button {
                serial,
                button,
                state: WEnum::Value(direction),
                ..
            } => {
                let Some(window) = live.window(live.mouse.focus.as_ref()) else {
                    return;
                };
                // Linux input codes: BTN_LEFT, BTN_RIGHT, BTN_MIDDLE, then side and extra buttons.
                let button = match button {
                    0x110 => Button::Left,
                    0x111 => Button::Right,
                    0x112 => Button::Middle,
                    0x113 | 0x116 => Button::Back,
                    0x114 | 0x115 => Button::Forward,
                    _ => return,
                };
                let (calls, at) = (window.calls, live.mouse.at);
                if direction == wl_pointer::ButtonState::Released {
                    live.mouse.held = None;
                    let clicks = live.mouse.clicks.count;
                    later(
                        Some(calls),
                        Event::PointerUp {
                            at,
                            button,
                            modifiers,
                            clicks,
                        },
                    );
                    return;
                }
                // Only press serials count: compositors decline requests with release serials.
                live.press = Some((serial, window.surface().clone()));
                live.mouse.held = Some(button);
                let clicks = live.mouse.clicks.press(button, at);
                // A click ends a composition, or commits a waiting dead key.
                let typing = &mut live.typing;
                let restart = typing.shown.take().and(typing.ime.clone());
                let pending = live.keys.composer.as_mut().and_then(Composer::reset);
                drop(live);
                let ended = restart.is_some();
                if let Some(ime) = restart {
                    self.restart_ime(ime);
                }
                soon(move || {
                    if ended {
                        Ime::End.apply(calls);
                    } else if let Some(text) = pending {
                        Ime::Commit(text).apply(calls);
                    }
                    calls.input(Event::PointerDown {
                        at,
                        button,
                        modifiers,
                        clicks,
                    });
                });
            }
            Ptr::AxisSource {
                axis_source: WEnum::Value(source),
            } => {
                live.mouse.wheel = source == wl_pointer::AxisSource::Wheel;
            }
            // Touchpads and other smooth sources scroll by pixels.
            Ptr::Axis { axis, value, .. } if !live.mouse.wheel => {
                scroll(&mut live.mouse.pixels, axis, -value as f32 * 3., shift);
            }
            // Seats from version 8 send only `value120`.
            Ptr::AxisDiscrete { axis, discrete } => {
                scroll(
                    &mut live.mouse.lines,
                    axis,
                    -discrete as f32 * NOTCH_LINES,
                    shift,
                );
            }
            Ptr::AxisValue120 { axis, value120 } => {
                scroll(
                    &mut live.mouse.lines,
                    axis,
                    -value120 as f32 / 120. * NOTCH_LINES,
                    shift,
                );
            }
            Ptr::Frame => {
                let mouse = &mut live.mouse;
                let (delta, lines) = match (mouse.pixels.take(), mouse.lines.take()) {
                    (Some(pixels), _) => (pixels, false),
                    (None, Some(lines)) => (lines, true),
                    (None, None) => return,
                };
                let (at, phase) = (mouse.at, Default::default());
                later(
                    live.hovered(),
                    Event::Wheel {
                        at,
                        delta,
                        lines,
                        phase,
                        modifiers,
                    },
                );
            }
            _ => {}
        }
    }

    fn on_device(&self, _: &wl_data_device::WlDataDevice, message: wl_data_device::Event, _: &()) {
        use wl_data_device::Event as Dev;
        let mut live = self.live.borrow_mut();
        let place = |x: f64, y: f64| point(px(x as f32), px(y as f32));
        match message {
            Dev::DataOffer { id } => live.offers.push(Offer {
                offer: id,
                types: Vec::new(),
                action: false,
            }),
            Dev::Selection { id } => {
                live.pasted = None;
                live.selection = id;
                live.prune();
            }
            Dev::Enter {
                serial,
                surface,
                x,
                y,
                id: Some(offer),
            } => {
                live.drag.round += 1;
                live.prune();
                let files = live.types(&offer).iter().any(|kind| kind == URI_LIST);
                if !files || !live.windows.contains_key(&surface.id()) {
                    offer.accept(serial, None);
                    return;
                }
                offer.accept(serial, Some(URI_LIST.to_owned()));
                let copy = wl_data_device_manager::DndAction::Copy;
                offer.set_actions(copy, copy);
                let round = live.drag.round;
                drop(live);
                self.read_drop(offer, surface.id(), place(x, y), round);
            }
            Dev::Motion { x, y, .. } => {
                live.drag.at = place(x, y);
                let calls = live
                    .window(live.drag.window.as_ref())
                    .map(|window| window.calls);
                later(calls, Event::FileOver { at: live.drag.at });
            }
            Dev::Leave | Dev::Drop => {
                let dropped = matches!(message, Dev::Drop);
                live.drag.round += 1;
                let id = live.drag.window.take();
                let calls = live.window(id.as_ref()).map(|window| window.calls);
                let (offer, at) = (live.drag.offer.take(), live.drag.at);
                let action = offer.as_ref().is_some_and(|offer| {
                    let kept = live.offers.iter().find(|kept| kept.offer == *offer);
                    kept.is_some_and(|kept| kept.action)
                });
                // The offer leaves the list first, so the prune doesn't destroy it a second time.
                live.offers
                    .retain(|kept| Some(&kept.offer) != offer.as_ref());
                live.prune();
                let (Some(calls), Some(offer)) = (calls, offer) else {
                    return;
                };
                if dropped && action {
                    offer.finish();
                }
                offer.destroy();
                later(
                    Some(calls),
                    if dropped {
                        Event::FileDrop { at }
                    } else {
                        Event::FileLeave
                    },
                );
            }
            _ => {}
        }
    }

    fn on_offer(&self, offer: &WlDataOffer, message: wl_data_offer::Event, _: &()) {
        let mut live = self.live.borrow_mut();
        let Some(kept) = live.offers.iter_mut().find(|kept| kept.offer == *offer) else {
            return;
        };
        match message {
            wl_data_offer::Event::Offer { mime_type } => kept.types.push(mime_type),
            wl_data_offer::Event::Action {
                dnd_action: WEnum::Value(action),
            } => kept.action = !action.is_empty(),
            _ => {}
        }
    }

    fn on_source(
        &self,
        source: &wl_data_source::WlDataSource,
        message: wl_data_source::Event,
        _: &(),
    ) {
        match message {
            wl_data_source::Event::Send { fd, .. } => {
                if let Some(copied) = self.live.borrow().copied.clone() {
                    send(fd, copied.into_bytes());
                }
            }
            wl_data_source::Event::Cancelled => source.destroy(),
            _ => {}
        }
    }

    fn on_token(
        &self,
        token: &Token,
        message: activation::xdg_activation_token_v1::Event,
        surface: &ObjectId,
    ) {
        if let activation::xdg_activation_token_v1::Event::Done { token: granted } = message
            && let Some(activation) = &self.globals.activation
            && let Some(window) = self.live.borrow().window(Some(surface))
        {
            activation.activate(granted, window.surface());
        }
        token.destroy();
    }
}

impl Display for Client {
    fn fd(&self) -> RawFd {
        self.conn.backend().poll_fd().as_raw_fd()
    }

    // Dispatches what was read, then takes the read lock that `finish` uses after `poll`.
    fn prepare(&self) -> bool {
        let guard = loop {
            if !self.dispatch() {
                return false;
            }
            if let Some(guard) = self.queue.borrow().prepare_read() {
                break guard;
            }
        };
        *self.reading.borrow_mut() = Some(guard);
        match self.conn.flush() {
            Err(WaylandError::Io(error)) if error.kind() == ErrorKind::WouldBlock => true,
            Err(error) => {
                eprintln!("apps: Wayland: {error}");
                false
            }
            Ok(()) => true,
        }
    }

    fn finish(&self, readable: bool) -> bool {
        let Some(guard) = self.reading.borrow_mut().take() else {
            return true;
        };
        // Dropping the guard without reading cancels the read.
        if readable {
            match guard.read() {
                Err(WaylandError::Io(error)) if error.kind() == ErrorKind::WouldBlock => {}
                Err(error) => {
                    eprintln!("apps: Wayland: {error}");
                    return false;
                }
                Ok(_) => {}
            }
        }
        self.dispatch()
    }

    fn read_clipboard(&self) -> Option<String> {
        let live = self.live.borrow();
        let offer = live.selection.clone()?;
        let types = live.types(&offer);
        if types.contains(&self.own_type) {
            return live.copied.clone();
        }
        if let Some(pasted) = &live.pasted {
            return Some(pasted.clone());
        }
        let kind = TEXT_TYPES
            .iter()
            .find(|kind| types.iter().any(|offered| offered == *kind))?;
        let (pipe, writer) = std::io::pipe().ok()?;
        offer.receive((*kind).to_owned(), writer.as_fd());
        drop((writer, live));
        let _ = self.conn.flush();
        let read =
            input::read_pipe(pipe).inspect_err(|error| eprintln!("apps: clipboard: {error:#}"));
        let text = String::from_utf8(read.ok()?).ok()?.replace("\r\n", "\n");
        self.live.borrow_mut().pasted = Some(text.clone());
        Some(text)
    }

    fn write_clipboard(&self, text: &str) {
        let mut live = self.live.borrow_mut();
        let (Some(manager), Some(device), Some((serial, _))) =
            (&self.globals.devices, &live.device, &live.press)
        else {
            return;
        };
        let source = manager.create_data_source(&self.qh, ());
        for kind in TEXT_TYPES.into_iter().chain([self.own_type.as_str()]) {
            source.offer(kind.to_owned());
        }
        device.set_selection(Some(&source), *serial);
        live.copied = Some(text.to_owned());
    }

    fn set_cursor(&self, cursor: Cursor) {
        let old = std::mem::replace(&mut self.live.borrow_mut().mouse.cursor, cursor);
        if old != cursor {
            self.show_cursor();
        }
    }

    fn display_bounds(&self) -> Option<Rect> {
        // Window states ask for this inside other calls, so it gives up instead of panicking on a borrow.
        let live = self.live.try_borrow().ok()?;
        live.screens.values().find_map(|(_, screen)| {
            let (at, pixels) = (screen.at?, screen.pixels?);
            let scale = screen.scale.max(1) as f32;
            let extent = size(
                px(pixels.width as f32 / scale),
                px(pixels.height as f32 / scale),
            );
            Some(Rect::new(point(px(at.x as f32), px(at.y as f32)), extent))
        })
    }

    fn open(self: Rc<Self>, options: &WindowOptions, calls: Callbacks) -> Result<Rc<dyn Native>> {
        let window = Surface::open(&self, options, calls)?;
        let startup = self.live.borrow_mut().startup.take();
        if let (Some(activation), Some(token)) = (&self.globals.activation, startup) {
            activation.activate(token, window.surface());
        }
        Ok(window)
    }
}
