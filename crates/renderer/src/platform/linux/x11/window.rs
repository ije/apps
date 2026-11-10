use super::{ALL_DEVICE_GROUPS, ALL_DEVICES, Client, check, reply};
use crate::{
    Bounds, Pixels, Scene, Size, WindowOptions, WindowState,
    app::Callbacks,
    platform::linux::{
        gpu::{Capture, Drawn, GpuAtlas, Renderer},
        post_local, with_loop,
    },
    point, px, size,
};
use anyhow::{Context as _, Result};
use calloop::{
    RegistrationToken,
    timer::{TimeoutAction, Timer},
};
use raw_window_handle as rwh;
use std::{
    cell::{Cell, RefCell},
    num::NonZeroU32,
    ptr::NonNull,
    rc::{Rc, Weak},
    time::Instant,
};
use x11rb::{
    connection::Connection as _,
    properties::WmSizeHints,
    protocol::{
        sync,
        xinput::{self, ConnectionExt as _},
        xproto::{self, ConnectionExt as _},
    },
    wrapper::ConnectionExt as _,
};

pub(super) struct Shared {
    pub id: xproto::Window,
    pub callbacks: Callbacks,
    client: Weak<Client>,
    state: RefCell<State>,
    mapped: Cell<bool>,
    obscured: Cell<bool>,
    wants: Cell<bool>,
    timer: Cell<Option<RegistrationToken>>,
}

struct State {
    // Taken when the window closes: GPU objects must not outlive the X window.
    renderer: Option<Renderer>,
    bounds: Bounds<Pixels>,
    active: bool,
    fullscreen: bool,
    maximized: [bool; 2],
    hidden: bool,
    counter: sync::Counter,
    sync: Option<sync::Int64>,
}

pub(crate) struct Window {
    shared: Rc<Shared>,
    client: Rc<Client>,
    atlas: Rc<GpuAtlas>,
    visual: xproto::Visualid,
}

impl Window {
    pub fn open(
        client: &Rc<Client>,
        options: &WindowOptions,
        callbacks: Callbacks,
    ) -> Result<Self> {
        let xcb = &client.xcb;
        let atoms = &client.atoms;
        let screen = &xcb.setup().roots[client.screen];
        let visual = options
            .transparent
            .then(|| {
                screen.allowed_depths.iter().find_map(|depth| {
                    let visual = depth.visuals.iter().find(|visual| {
                        (visual.red_mask, visual.green_mask, visual.blue_mask)
                            == (0xFF0000, 0xFF00, 0xFF)
                            && depth.depth == 32
                    })?;
                    Some((visual.visual_id, depth.depth))
                })
            })
            .flatten();
        let colormap = match visual {
            Some((visual, _)) => {
                let colormap = xcb.generate_id()?;
                check(xcb.create_colormap(
                    xproto::ColormapAlloc::NONE,
                    colormap,
                    screen.root,
                    visual,
                ))?;
                colormap
            }
            None => screen.default_colormap,
        };
        let (visual, depth) = visual.unwrap_or((screen.root_visual, screen.root_depth));
        let scale = client.scale_factor;
        let mut bounds = options.bounds.to_device_pixels(scale);
        if bounds.size.width.0 <= 0 || bounds.size.height.0 <= 0 {
            bounds.size = size(800.into(), 600.into());
        }
        let id = xcb.generate_id()?;
        check(
            xcb.create_window(
                depth,
                id,
                screen.root,
                bounds.origin.x.0 as i16,
                bounds.origin.y.0 as i16,
                bounds.size.width.0 as u16,
                bounds.size.height.0 as u16,
                0,
                xproto::WindowClass::INPUT_OUTPUT,
                visual,
                &xproto::CreateWindowAux::new()
                    .border_pixel(screen.black_pixel)
                    .colormap(colormap)
                    .event_mask(
                        xproto::EventMask::EXPOSURE
                            | xproto::EventMask::STRUCTURE_NOTIFY
                            | xproto::EventMask::FOCUS_CHANGE
                            | xproto::EventMask::KEY_PRESS
                            | xproto::EventMask::KEY_RELEASE
                            | xproto::EventMask::PROPERTY_CHANGE
                            | xproto::EventMask::VISIBILITY_CHANGE,
                    ),
            ),
        )?;
        let setup = (|| -> Result<(sync::Counter, Renderer)> {
            check(xcb.change_property32(
                xproto::PropMode::REPLACE,
                id,
                atoms._NET_WM_PID,
                xproto::AtomEnum::CARDINAL,
                &[std::process::id()],
            ))?;
            check(xcb.change_property32(
                xproto::PropMode::REPLACE,
                id,
                atoms.WM_PROTOCOLS,
                xproto::AtomEnum::ATOM,
                &[atoms.WM_DELETE_WINDOW, atoms._NET_WM_SYNC_REQUEST],
            ))?;
            reply(sync::initialize(&**xcb, 3, 1)).context("X11 sync")?;
            let counter = xcb.generate_id()?;
            check(sync::create_counter(
                &**xcb,
                counter,
                sync::Int64 { lo: 0, hi: 0 },
            ))?;
            check(xcb.change_property32(
                xproto::PropMode::REPLACE,
                id,
                atoms._NET_WM_SYNC_REQUEST_COUNTER,
                xproto::AtomEnum::CARDINAL,
                &[counter],
            ))?;
            check(xcb.change_property32(
                xproto::PropMode::REPLACE,
                id,
                atoms.XdndAware,
                xproto::AtomEnum::ATOM,
                &[5],
            ))?;
            if options.titlebar_hidden || options.frameless {
                check(xcb.change_property32(
                    xproto::PropMode::REPLACE,
                    id,
                    atoms._MOTIF_WM_HINTS,
                    atoms._MOTIF_WM_HINTS,
                    &[1 << 1, 0, 0, 0, 0],
                ))?;
            }
            check(xcb.xinput_xi_select_events(
                id,
                &[xinput::EventMask {
                    deviceid: ALL_DEVICE_GROUPS,
                    mask: vec![
                        xinput::XIEventMask::MOTION
                            | xinput::XIEventMask::BUTTON_PRESS
                            | xinput::XIEventMask::BUTTON_RELEASE
                            | xinput::XIEventMask::ENTER
                            | xinput::XIEventMask::LEAVE,
                    ],
                }],
            ))?;
            check(xcb.xinput_xi_select_events(
                id,
                &[xinput::EventMask {
                    deviceid: ALL_DEVICES,
                    mask: vec![
                        xinput::XIEventMask::HIERARCHY | xinput::XIEventMask::DEVICE_CHANGED,
                    ],
                }],
            ))?;
            xcb.flush()?;
            let renderer = Renderer::new(
                display_handle(client),
                window_handle(id, visual),
                bounds.size,
                options.transparent,
                false,
            )?;
            let mut hints = WmSizeHints::new();
            let max = renderer.max_texture_size() as i32;
            hints.max_size = Some((max, max));
            if !options.resizable {
                hints.min_size = Some((bounds.size.width.0, bounds.size.height.0));
                hints.max_size = hints.min_size;
            }
            check(hints.set_normal_hints(&**xcb, id))?;
            Ok((counter, renderer))
        })();
        let (counter, renderer) = match setup {
            Ok(setup) => setup,
            Err(error) => {
                let _ = xcb.destroy_window(id);
                let _ = xcb.flush();
                return Err(error);
            }
        };
        let atlas = renderer.atlas().clone();
        let shared = Rc::new(Shared {
            id,
            callbacks,
            client: Rc::downgrade(client),
            state: RefCell::new(State {
                renderer: Some(renderer),
                bounds: bounds.map(|value| px(value.0 as f32 / scale)),
                active: false,
                fullscreen: false,
                maximized: [false; 2],
                hidden: false,
                counter,
                sync: None,
            }),
            mapped: Cell::new(false),
            obscured: Cell::new(false),
            wants: Cell::new(false),
            timer: Cell::new(None),
        });
        client
            .state
            .borrow_mut()
            .windows
            .insert(id, Rc::downgrade(&shared));
        let window = Self {
            shared,
            client: client.clone(),
            atlas,
            visual,
        };
        window.set_title(&options.title);
        let name = super::super::app_name();
        let class = [name.as_bytes(), b"\0", name.as_bytes(), b"\0"].concat();
        let _ = xcb.change_property8(
            xproto::PropMode::REPLACE,
            id,
            xproto::AtomEnum::WM_CLASS,
            xproto::AtomEnum::STRING,
            &class,
        );
        if options.show {
            let _ = xcb.map_window(id);
        }
        let _ = xcb.flush();
        Ok(window)
    }

    pub fn atlas(&self) -> &Rc<GpuAtlas> {
        &self.atlas
    }

    pub fn scale_factor(&self) -> f32 {
        self.client.scale_factor
    }

    pub fn content_size(&self) -> Size<Pixels> {
        self.shared.state.borrow().bounds.size
    }

    pub fn request_frame(&self) {
        self.shared.wants.set(true);
        self.shared.start_frames();
    }

    pub fn force_frame(&self) {
        let shared = Rc::downgrade(&self.shared);
        post_local(move || {
            if let Some(shared) = shared.upgrade() {
                shared.callbacks.frame();
            }
        });
    }

    pub fn draw(&self, scene: &Scene, capture: bool) -> Capture {
        let (drawn, capture) = {
            let mut state = self.shared.state.borrow_mut();
            let (drawn, capture) = state.renderer.as_mut()?.draw(scene, capture);
            if let Some(value) = state.sync.take() {
                let _ = sync::set_counter(&*self.client.xcb, state.counter, value);
                let _ = self.client.xcb.flush();
            }
            (drawn, capture)
        };
        if drawn == Drawn::Repaint {
            self.shared.callbacks.appearance_changed();
        }
        capture
    }

    pub fn is_active(&self) -> bool {
        self.shared.state.borrow().active
    }

    pub fn set_title(&self, title: &str) {
        let xcb = &self.client.xcb;
        let id = self.shared.id;
        let _ = xcb.change_property8(
            xproto::PropMode::REPLACE,
            id,
            xproto::AtomEnum::WM_NAME,
            xproto::AtomEnum::STRING,
            title.as_bytes(),
        );
        let _ = xcb.change_property8(
            xproto::PropMode::REPLACE,
            id,
            self.client.atoms._NET_WM_NAME,
            self.client.atoms.UTF8_STRING,
            title.as_bytes(),
        );
        let _ = xcb.flush();
    }

    pub fn set_parent(&self, parent: Option<rwh::RawWindowHandle>) {
        let (xcb, id) = (&self.client.xcb, self.shared.id);
        let _ = match parent {
            Some(rwh::RawWindowHandle::Xcb(parent)) => xcb.change_property32(
                xproto::PropMode::REPLACE,
                id,
                xproto::AtomEnum::WM_TRANSIENT_FOR,
                xproto::AtomEnum::WINDOW,
                &[parent.window.get()],
            ),
            _ => xcb.delete_property(id, xproto::AtomEnum::WM_TRANSIENT_FOR.into()),
        };
        let _ = xcb.flush();
    }

    pub fn state(&self) -> WindowState {
        let state = self.shared.state.borrow();
        let display = self
            .shared
            .client
            .upgrade()
            .and_then(|client| client.primary_display_bounds())
            .unwrap_or_default();
        WindowState {
            frame: state.bounds,
            content: state.bounds,
            minimized: state.hidden,
            maximized: state.maximized == [true; 2] && !state.hidden && !state.fullscreen,
            fullscreen: state.fullscreen,
            visible: self.shared.mapped.get(),
            resizing: false,
            display,
            work_area: display,
        }
    }

    pub fn minimize(&self) {
        self.client.send_wm_message(
            self.shared.id,
            self.client.atoms.WM_CHANGE_STATE,
            [3, 0, 0, 0, 0],
        );
    }

    pub fn zoom(&self) {
        let atoms = &self.client.atoms;
        self.client.send_wm_message(
            self.shared.id,
            atoms._NET_WM_STATE,
            [
                2,
                atoms._NET_WM_STATE_MAXIMIZED_VERT,
                atoms._NET_WM_STATE_MAXIMIZED_HORZ,
                1,
                0,
            ],
        );
    }

    pub fn toggle_fullscreen(&self) {
        let atoms = &self.client.atoms;
        self.client.send_wm_message(
            self.shared.id,
            atoms._NET_WM_STATE,
            [2, atoms._NET_WM_STATE_FULLSCREEN, 0, 1, 0],
        );
    }

    pub fn cancel_composition(&self) {
        self.client.reset_ime();
    }

    pub fn activate(&self) {
        if !self.shared.mapped.get() {
            let _ = self.client.xcb.map_window(self.shared.id);
        }
        self.client.send_wm_message(
            self.shared.id,
            self.client.atoms._NET_ACTIVE_WINDOW,
            [1, x11rb::CURRENT_TIME, 0, 0, 0],
        );
    }

    pub fn resize(&self, size: Size<Pixels>) {
        let size = size.to_device_pixels(self.client.scale_factor);
        let _ = self.client.xcb.configure_window(
            self.shared.id,
            &xproto::ConfigureWindowAux::new()
                .width(size.width.0.max(1) as u32)
                .height(size.height.0.max(1) as u32),
        );
        let _ = self.client.xcb.flush();
    }

    pub fn start_window_move(&self) {
        let xcb = &self.client.xcb;
        let _ = xcb.ungrab_pointer(x11rb::CURRENT_TIME);
        let Ok(pointer) = reply(xcb.query_pointer(self.shared.id)) else {
            return;
        };
        const MOVE: u32 = 8;
        self.client.send_wm_message(
            self.shared.id,
            self.client.atoms._NET_WM_MOVERESIZE,
            [pointer.root_x as u32, pointer.root_y as u32, MOVE, 1, 1],
        );
    }

    pub fn raw_window_handle(&self) -> rwh::RawWindowHandle {
        window_handle(self.shared.id, self.visual)
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        self.shared.stop_frames();
        self.shared.state.borrow_mut().renderer.take();
        self.client
            .state
            .borrow_mut()
            .windows
            .remove(&self.shared.id);
        let mut state = self.client.state.borrow_mut();
        if state.keyboard_focus == Some(self.shared.id) {
            state.keyboard_focus = None;
        }
        if state.mouse_focus == Some(self.shared.id) {
            state.mouse_focus = None;
        }
        drop(state);
        let _ = self.client.xcb.destroy_window(self.shared.id);
        let _ = self.client.xcb.flush();
    }
}

impl Shared {
    fn visible(&self) -> bool {
        self.mapped.get() && !self.obscured.get()
    }

    fn start_frames(self: &Rc<Self>) {
        if self.timer.get().is_some() || !self.visible() {
            return;
        }
        let Some(refresh) = self.client.upgrade().map(|client| client.refresh) else {
            return;
        };
        let shared = Rc::downgrade(self);
        let token = with_loop(|handle| {
            handle.insert_source(Timer::immediate(), move |deadline, _, _| {
                let Some(shared) = shared.upgrade() else {
                    return TimeoutAction::Drop;
                };
                let more = shared.callbacks.frame();
                if shared.timer.get().is_none() {
                    return TimeoutAction::Drop;
                }
                if more == Some(false) {
                    shared.wants.set(false);
                    shared.timer.set(None);
                    return TimeoutAction::Drop;
                }
                let now = Instant::now();
                let mut next = deadline + refresh;
                while next < now {
                    next += refresh;
                }
                TimeoutAction::ToInstant(next)
            })
        });
        self.timer.set(token.ok());
    }

    fn stop_frames(&self) {
        if let Some(token) = self.timer.take() {
            with_loop(|handle| handle.remove(token));
        }
    }

    fn visibility_changed(self: &Rc<Self>) {
        if !self.visible() {
            self.stop_frames();
        } else if self.wants.get() {
            self.start_frames();
        }
        self.callbacks.changed();
    }

    pub fn set_mapped(self: &Rc<Self>, mapped: bool) {
        self.mapped.set(mapped);
        self.visibility_changed();
    }

    pub fn set_obscured(self: &Rc<Self>, obscured: bool) {
        if self.obscured.replace(obscured) != obscured {
            self.visibility_changed();
        }
    }

    pub fn expose(&self) {
        if self.mapped.get() {
            self.callbacks.appearance_changed();
        }
    }

    pub fn set_active(&self, active: bool) {
        self.state.borrow_mut().active = active;
        self.callbacks.changed();
    }

    pub fn sync_request(&self, value: sync::Int64) {
        self.state.borrow_mut().sync = Some(value);
    }

    pub fn configure(&self, x: i32, y: i32, width: i32, height: i32) {
        let Some(scale) = self.client.upgrade().map(|client| client.scale_factor) else {
            return;
        };
        let logical = Bounds::new(
            point(px(x as f32 / scale), px(y as f32 / scale)),
            size(px(width as f32 / scale), px(height as f32 / scale)),
        );
        let resized = {
            let mut state = self.state.borrow_mut();
            let resized = logical.size != state.bounds.size;
            // The origin of a resize event can be wrong, so only moves update it.
            if resized {
                state.bounds.size = logical.size;
                if let Some(renderer) = &mut state.renderer {
                    renderer.update_drawable_size(size(width.into(), height.into()));
                }
            } else {
                state.bounds.origin = logical.origin;
            }
            resized
        };
        if resized {
            self.callbacks.resized(logical.size, scale);
        } else {
            self.callbacks.changed();
        }
    }

    pub fn read_wm_state(&self) {
        let Some(client) = self.client.upgrade() else {
            return;
        };
        let Ok(property) = reply(client.xcb.get_property(
            false,
            self.id,
            client.atoms._NET_WM_STATE,
            xproto::AtomEnum::ATOM,
            0,
            u32::MAX,
        )) else {
            return;
        };
        let atoms = &client.atoms;
        {
            let mut state = self.state.borrow_mut();
            state.fullscreen = false;
            state.maximized = [false; 2];
            state.hidden = false;
            for atom in property.value32().into_iter().flatten() {
                if atom == atoms._NET_WM_STATE_FOCUSED {
                    state.active = true;
                } else if atom == atoms._NET_WM_STATE_FULLSCREEN {
                    state.fullscreen = true;
                } else if atom == atoms._NET_WM_STATE_MAXIMIZED_VERT {
                    state.maximized[0] = true;
                } else if atom == atoms._NET_WM_STATE_MAXIMIZED_HORZ {
                    state.maximized[1] = true;
                } else if atom == atoms._NET_WM_STATE_HIDDEN {
                    state.hidden = true;
                }
            }
        }
        self.callbacks.changed();
    }
}

fn window_handle(id: xproto::Window, visual: xproto::Visualid) -> rwh::RawWindowHandle {
    let mut handle =
        rwh::XcbWindowHandle::new(NonZeroU32::new(id).expect("X11 window ids are not zero"));
    handle.visual_id = NonZeroU32::new(visual);
    rwh::RawWindowHandle::Xcb(handle)
}

fn display_handle(client: &Client) -> rwh::RawDisplayHandle {
    let connection = as_raw_xcb_connection::AsRawXcbConnection::as_raw_xcb_connection(&*client.xcb);
    rwh::RawDisplayHandle::Xcb(rwh::XcbDisplayHandle::new(
        NonNull::new(connection.cast()),
        client.screen as i32,
    ))
}
