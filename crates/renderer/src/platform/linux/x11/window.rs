use super::Client;
use crate::platform::linux::gpu::{Atlas, Capture, Drawn, Renderer};
use crate::platform::linux::{Action, Display as _, Native, run_after};
use crate::{
    Pixels, Rect, Scene, Size, WindowOptions, WindowState, app::Callbacks, point, px, size,
};
use anyhow::Result;
use raw_window_handle::{RawDisplayHandle, RawWindowHandle, XcbDisplayHandle, XcbWindowHandle};
use std::{
    cell::Cell, cell::RefCell, num::NonZeroU32, ptr::NonNull, rc::Rc, rc::Weak, time::Instant,
};
use x11rb::connection::Connection as _;
use x11rb::properties::WmSizeHints;
use x11rb::protocol::sync;
use x11rb::protocol::xinput::{ConnectionExt as _, XIEventMask};
use x11rb::protocol::xproto::{self, AtomEnum, ConnectionExt as _, EventMask, PropMode};
use x11rb::wrapper::ConnectionExt as _;

struct Shape {
    bounds: Rect,
    active: bool,
    fullscreen: bool,
    // Vertical and horizontal.
    maximized: [bool; 2],
    hidden: bool,
    // The window manager waits for this counter after a resize, until the frame of the new size is drawn.
    counter: sync::Counter,
    synced: Option<sync::Int64>,
}

pub(super) struct Toplevel {
    me: Weak<Toplevel>,
    id: xproto::Window,
    visual: xproto::Visualid,
    // The server keeps a colormap and a counter until the client frees them, not until the window closes.
    colormap: Option<xproto::Colormap>,
    pub calls: Callbacks,
    client: Weak<Client>,
    // Taken first when the window closes: GPU objects must not outlive the X window.
    renderer: RefCell<Option<Renderer>>,
    atlas: Rc<Atlas>,
    shape: RefCell<Shape>,
    mapped: Cell<bool>,
    obscured: Cell<bool>,
    wants: Cell<bool>,
    // Frames come from a timer at the display refresh while this is set. Changing `round` stops a timer.
    ticking: Cell<bool>,
    round: Cell<u64>,
}

fn handle(id: xproto::Window, visual: xproto::Visualid) -> RawWindowHandle {
    let mut handle =
        XcbWindowHandle::new(NonZeroU32::new(id).expect("X11 window ids are not zero"));
    handle.visual_id = NonZeroU32::new(visual);
    RawWindowHandle::Xcb(handle)
}

impl Toplevel {
    pub fn open(
        client: &Rc<Client>,
        options: &WindowOptions,
        calls: Callbacks,
    ) -> Result<Rc<Self>> {
        let xcb = &*client.xcb;
        let atoms = &client.atoms;
        let screen = &xcb.setup().roots[client.screen];
        // Transparent windows need a 32-bit visual with alpha.
        let rgb = |visual: &&xproto::Visualtype| {
            (visual.red_mask, visual.green_mask, visual.blue_mask) == (0xff0000, 0xff00, 0xff)
        };
        let deep = screen
            .allowed_depths
            .iter()
            .filter(|depth| depth.depth == 32 && options.transparent);
        let argb = deep
            .flat_map(|depth| {
                depth
                    .visuals
                    .iter()
                    .find(rgb)
                    .map(|visual| (visual.visual_id, depth.depth))
            })
            .next();
        let own_colormap = match argb {
            Some((visual, _)) => {
                let colormap = xcb.generate_id()?;
                xcb.create_colormap(xproto::ColormapAlloc::NONE, colormap, screen.root, visual)?
                    .check()?;
                Some(colormap)
            }
            None => None,
        };
        let colormap = own_colormap.unwrap_or(screen.default_colormap);
        let (visual, depth) = argb.unwrap_or((screen.root_visual, screen.root_depth));
        let scale = client.scale;
        let mut pixels = options.bounds.to_device(scale);
        if pixels.size.width <= 0 || pixels.size.height <= 0 {
            pixels.size = size(800, 600);
        }
        let heard = [
            EventMask::EXPOSURE,
            EventMask::STRUCTURE_NOTIFY,
            EventMask::FOCUS_CHANGE,
            EventMask::KEY_PRESS,
            EventMask::KEY_RELEASE,
            EventMask::PROPERTY_CHANGE,
            EventMask::VISIBILITY_CHANGE,
        ];
        let heard = heard
            .into_iter()
            .fold(EventMask::NO_EVENT, |all, one| all | one);
        let aux = xproto::CreateWindowAux::new()
            .border_pixel(screen.black_pixel)
            .colormap(colormap)
            .event_mask(heard);
        let window = xcb.generate_id()?;
        let (x, y) = (pixels.origin.x as i16, pixels.origin.y as i16);
        let (width, height) = (pixels.size.width as u16, pixels.size.height as u16);
        let class = xproto::WindowClass::INPUT_OUTPUT;
        xcb.create_window(
            depth,
            window,
            screen.root,
            x,
            y,
            width,
            height,
            0,
            class,
            visual,
            &aux,
        )?
        .check()?;
        let made = (|| -> Result<(sync::Counter, Renderer)> {
            let put = |property, kind: AtomEnum, values: &[u32]| -> Result<()> {
                xcb.change_property32(PropMode::REPLACE, window, property, kind, values)?
                    .check()?;
                Ok(())
            };
            put(atoms.Pid, AtomEnum::CARDINAL, &[std::process::id()])?;
            put(
                atoms.Protocols,
                AtomEnum::ATOM,
                &[atoms.DeleteWindow, atoms.SyncRequest],
            )?;
            let counter = xcb.generate_id()?;
            sync::create_counter(xcb, counter, sync::Int64 { lo: 0, hi: 0 })?.check()?;
            put(atoms.SyncCounter, AtomEnum::CARDINAL, &[counter])?;
            // Drops speak XDND version 5.
            put(atoms.DndAware, AtomEnum::ATOM, &[5])?;
            if options.titlebar_hidden || options.frameless {
                // Motif hints with only the decorations flag, and no decorations.
                let hints = [2, 0, 0, 0, 0];
                xcb.change_property32(
                    PropMode::REPLACE,
                    window,
                    atoms.MotifHints,
                    atoms.MotifHints,
                    &hints,
                )?
                .check()?;
            }
            let pointer =
                XIEventMask::MOTION | XIEventMask::BUTTON_PRESS | XIEventMask::BUTTON_RELEASE;
            let pointer = pointer | XIEventMask::ENTER | XIEventMask::LEAVE;
            let devices = XIEventMask::HIERARCHY | XIEventMask::DEVICE_CHANGED;
            // Device 1 is all master devices, and 0 all devices.
            for (deviceid, mask) in [(1u16, pointer), (0, devices)] {
                let masks = [x11rb::protocol::xinput::EventMask {
                    deviceid,
                    mask: vec![mask],
                }];
                xcb.xinput_xi_select_events(window, &masks)?.check()?;
            }
            xcb.flush()?;
            let raw = NonNull::new(xcb.get_raw_xcb_connection());
            let display = RawDisplayHandle::Xcb(XcbDisplayHandle::new(raw, client.screen as i32));
            let pixels = [u32::from(width), u32::from(height)];
            let renderer = Renderer::new(
                display,
                handle(window, visual),
                pixels,
                options.transparent,
                false,
            )?;
            let mut hints = WmSizeHints::new();
            let max = renderer.max_side() as i32;
            hints.max_size = Some((max, max));
            if !options.resizable {
                hints.min_size = Some((i32::from(width), i32::from(height)));
                hints.max_size = hints.min_size;
            }
            hints.set_normal_hints(xcb, window)?.check()?;
            Ok((counter, renderer))
        })();
        let (counter, renderer) = match made {
            Ok(made) => made,
            Err(error) => {
                let _ = xcb.destroy_window(window);
                if let Some(colormap) = own_colormap {
                    let _ = xcb.free_colormap(colormap);
                }
                let _ = xcb.flush();
                return Err(error);
            }
        };
        let bounds = pixels.map(|value| px(value as f32 / scale));
        let shape = Shape {
            bounds,
            active: false,
            fullscreen: false,
            maximized: [false; 2],
            hidden: false,
            counter,
            synced: None,
        };
        let made = Rc::new_cyclic(|me| Self {
            me: me.clone(),
            id: window,
            visual,
            colormap: own_colormap,
            calls,
            client: Rc::downgrade(client),
            atlas: renderer.atlas().clone(),
            renderer: RefCell::new(Some(renderer)),
            shape: RefCell::new(shape),
            mapped: Cell::new(false),
            obscured: Cell::new(false),
            wants: Cell::new(false),
            ticking: Cell::new(false),
            round: Cell::new(0),
        });
        client
            .live
            .borrow_mut()
            .windows
            .insert(window, Rc::downgrade(&made));
        made.act(Action::Title(options.title.clone()));
        let name = super::super::app_name();
        let class = [name.as_bytes(), b"\0", name.as_bytes(), b"\0"].concat();
        let _ = xcb.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            &class,
        );
        if options.show {
            let _ = xcb.map_window(window);
        }
        let _ = xcb.flush();
        Ok(made)
    }

    fn visible(&self) -> bool {
        self.mapped.get() && !self.obscured.get()
    }

    fn start(&self) {
        if self.ticking.get() || !self.visible() {
            return;
        }
        self.ticking.set(true);
        tick(self.me.clone(), self.round.get(), Instant::now());
    }

    fn stop(&self) {
        self.ticking.set(false);
        self.round.set(self.round.get() + 1);
    }

    fn visibility_changed(&self) {
        if !self.visible() {
            self.stop();
        } else if self.wants.get() {
            self.start();
        }
        self.calls.changed();
    }

    pub fn set_mapped(&self, mapped: bool) {
        self.mapped.set(mapped);
        self.visibility_changed();
    }

    pub fn set_obscured(&self, obscured: bool) {
        if self.obscured.replace(obscured) != obscured {
            self.visibility_changed();
        }
    }

    pub fn expose(&self) {
        if self.mapped.get() {
            self.calls.appearance_changed();
        }
    }

    pub fn focus(&self, active: bool) {
        self.shape.borrow_mut().active = active;
        self.calls.changed();
    }

    pub fn sync_request(&self, value: sync::Int64) {
        self.shape.borrow_mut().synced = Some(value);
    }

    // A configure in device pixels.
    pub fn configure(&self, x: i32, y: i32, width: i32, height: i32) {
        let Some(scale) = self.client.upgrade().map(|client| client.scale) else {
            return;
        };
        let extent = size(px(width as f32 / scale), px(height as f32 / scale));
        let resized = {
            let mut shape = self.shape.borrow_mut();
            let resized = extent != shape.bounds.size;
            // The origin that comes with a resize can be wrong, so only moves set it.
            if resized {
                shape.bounds.size = extent;
                if let Some(renderer) = &mut *self.renderer.borrow_mut() {
                    renderer.resize([width.max(1) as u32, height.max(1) as u32]);
                }
            } else {
                shape.bounds.origin = point(px(x as f32 / scale), px(y as f32 / scale));
            }
            resized
        };
        match resized {
            true => self.calls.resized(extent, scale),
            false => self.calls.changed(),
        }
    }

    pub fn read_wm_state(&self) {
        let Some(client) = self.client.upgrade() else {
            return;
        };
        let atoms = &client.atoms;
        let asked =
            client
                .xcb
                .get_property(false, self.id, atoms.State, AtomEnum::ATOM, 0, u32::MAX);
        let Some(property) = asked.ok().and_then(|cookie| cookie.reply().ok()) else {
            return;
        };
        {
            let mut shape = self.shape.borrow_mut();
            (shape.fullscreen, shape.maximized, shape.hidden) = (false, [false; 2], false);
            for atom in property.value32().into_iter().flatten() {
                match atom {
                    _ if atom == atoms.Focused => shape.active = true,
                    _ if atom == atoms.Fullscreen => shape.fullscreen = true,
                    _ if atom == atoms.Tall => shape.maximized[0] = true,
                    _ if atom == atoms.Wide => shape.maximized[1] = true,
                    _ if atom == atoms.Hidden => shape.hidden = true,
                    _ => {}
                }
            }
        }
        self.calls.changed();
    }
}

fn tick(window: Weak<Toplevel>, round: u64, at: Instant) {
    run_after(at.saturating_duration_since(Instant::now()), move || {
        let Some(strong) = window.upgrade() else {
            return;
        };
        if strong.round.get() != round {
            return;
        }
        let more = strong.calls.frame();
        if strong.round.get() != round {
            return;
        }
        if more == Some(false) {
            strong.wants.set(false);
            strong.stop();
            return;
        }
        let Some(refresh) = strong.client.upgrade().map(|client| client.refresh) else {
            return;
        };
        let now = Instant::now();
        let mut next = at + refresh;
        while next < now {
            next += refresh;
        }
        tick(window, round, next);
    });
}

impl Native for Toplevel {
    fn scale(&self) -> f32 {
        self.client.upgrade().map_or(1., |client| client.scale)
    }

    fn extent(&self) -> Size<Pixels> {
        self.shape.borrow().bounds.size
    }

    fn state(&self) -> WindowState {
        let shape = self.shape.borrow();
        let display = self
            .client
            .upgrade()
            .and_then(|client| client.display_bounds());
        let display = display.unwrap_or_default();
        WindowState {
            frame: shape.bounds,
            content: shape.bounds,
            minimized: shape.hidden,
            maximized: shape.maximized == [true; 2] && !shape.hidden && !shape.fullscreen,
            fullscreen: shape.fullscreen,
            visible: self.mapped.get(),
            resizing: false,
            display,
            work_area: display,
        }
    }

    fn active(&self) -> bool {
        self.shape.borrow().active
    }

    fn atlas(&self) -> Rc<Atlas> {
        self.atlas.clone()
    }

    fn want_frames(&self) {
        self.wants.set(true);
        self.start();
    }

    fn draw(&self, scene: &Scene, capture: bool) -> Capture {
        let (drawn, capture) = match &mut *self.renderer.borrow_mut() {
            Some(renderer) => renderer.draw(scene, capture),
            None => return capture.then(|| Err(anyhow::anyhow!("The window is closed"))),
        };
        let (counter, synced) = {
            let mut shape = self.shape.borrow_mut();
            (shape.counter, shape.synced.take())
        };
        if let (Some(value), Some(client)) = (synced, self.client.upgrade()) {
            let _ = sync::set_counter(&*client.xcb, counter, value);
            let _ = client.xcb.flush();
        }
        if drawn == Drawn::Again {
            self.calls.appearance_changed();
        }
        capture
    }

    fn release(&self) {
        if let Some(renderer) = &mut *self.renderer.borrow_mut() {
            renderer.release();
        }
    }

    fn act(&self, action: Action) {
        let Some(client) = self.client.upgrade() else {
            return;
        };
        let (xcb, atoms, id) = (&client.xcb, &client.atoms, self.id);
        match action {
            Action::Title(title) => {
                let (name, string) = (AtomEnum::WM_NAME.into(), AtomEnum::STRING.into());
                for (property, kind) in [(name, string), (atoms.Name, atoms.UTF8_STRING)] {
                    let _ = xcb.change_property8(
                        PropMode::REPLACE,
                        id,
                        property,
                        kind,
                        title.as_bytes(),
                    );
                }
            }
            // ICCCM iconic state.
            Action::Minimize => client.ask_manager(id, atoms.ChangeState, [3, 0, 0, 0, 0]),
            // Toggles of window manager states, from a normal application.
            Action::Zoom => client.ask_manager(id, atoms.State, [2, atoms.Tall, atoms.Wide, 1, 0]),
            Action::Fullscreen => {
                client.ask_manager(id, atoms.State, [2, atoms.Fullscreen, 0, 1, 0])
            }
            Action::Raise => {
                if !self.mapped.get() {
                    let _ = xcb.map_window(id);
                }
                client.ask_manager(id, atoms.ActiveWindow, [1, x11rb::CURRENT_TIME, 0, 0, 0]);
            }
            Action::Resize(extent) => {
                let pixels = extent.to_device(client.scale);
                let [width, height] = [pixels.width, pixels.height].map(|side| side.max(1) as u32);
                let _ = xcb.configure_window(
                    id,
                    &xproto::ConfigureWindowAux::new()
                        .width(width)
                        .height(height),
                );
            }
            Action::Move => {
                // The window manager can't grab the pointer while the press keeps it grabbed for this window.
                let _ = xcb.ungrab_pointer(x11rb::CURRENT_TIME);
                let Some(pointer) = xcb
                    .query_pointer(id)
                    .ok()
                    .and_then(|cookie| cookie.reply().ok())
                else {
                    return;
                };
                // A move with button 1, from a normal application.
                let (x, y) = (pointer.root_x as u32, pointer.root_y as u32);
                client.ask_manager(id, atoms.MoveResize, [x, y, 8, 1, 1]);
            }
            Action::Parent(parent) => {
                let transient = AtomEnum::WM_TRANSIENT_FOR;
                let _ = match parent {
                    Some(RawWindowHandle::Xcb(parent)) => {
                        let owner = [parent.window.get()];
                        xcb.change_property32(
                            PropMode::REPLACE,
                            id,
                            transient,
                            AtomEnum::WINDOW,
                            &owner,
                        )
                    }
                    _ => xcb.delete_property(id, transient.into()),
                };
            }
            Action::CancelComposition => client.reset_ime(),
        }
        let _ = xcb.flush();
    }

    fn handle(&self) -> RawWindowHandle {
        handle(self.id, self.visual)
    }
}

impl Drop for Toplevel {
    fn drop(&mut self) {
        self.stop();
        self.renderer.take();
        let Some(client) = self.client.upgrade() else {
            return;
        };
        {
            let mut live = client.live.borrow_mut();
            live.windows.remove(&self.id);
            if live.focus == Some(self.id) {
                live.focus = None;
            }
            if live.hover == Some(self.id) {
                live.hover = None;
            }
        }
        let _ = client.xcb.destroy_window(self.id);
        let _ = sync::destroy_counter(&*client.xcb, self.shape.borrow().counter);
        if let Some(colormap) = self.colormap {
            let _ = client.xcb.free_colormap(colormap);
        }
        let _ = client.xcb.flush();
    }
}
