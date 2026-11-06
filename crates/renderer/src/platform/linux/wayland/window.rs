use super::{Client, Output};
use crate::{
    Bounds, Pixels, Scene, Size, WindowOptions, WindowState,
    app::Callbacks,
    platform::linux::{
        RETRY_INTERVAL,
        gpu::{Capture, Drawn, GpuAtlas, Renderer},
        post_local, run_after,
    },
    point, px, size,
};
use anyhow::Result;
use raw_window_handle as rwh;
use rustc_hash::FxHashMap;
use std::{
    cell::{Cell, RefCell},
    ptr::NonNull,
    rc::{Rc, Weak},
};
use wayland_client::{Proxy as _, WEnum, protocol::wl_surface};
use wayland_protocols::{
    wp::{fractional_scale::v1::client::wp_fractional_scale_v1, viewporter::client::wp_viewport},
    xdg::{
        decoration::zv1::client::zxdg_toplevel_decoration_v1::{self, Mode},
        shell::client::{xdg_surface, xdg_toplevel},
    },
};

struct Configure {
    size: Option<Size<Pixels>>,
    maximized: bool,
    fullscreen: bool,
    resizing: bool,
    suspended: bool,
}

pub(super) struct Shared {
    pub surface: wl_surface::WlSurface,
    pub callbacks: Callbacks,
    client: Weak<Client>,
    state: RefCell<State>,
    transparent: bool,
    wants: Cell<bool>,
    callback: Cell<bool>,
    scheduled: Cell<bool>,
    configured: Cell<bool>,
    shown: Cell<bool>,
    presented: Cell<bool>,
}

struct State {
    // WARNING: taken when the window closes: GPU objects must not outlive the surface.
    renderer: Option<Renderer>,
    xdg: xdg_surface::XdgSurface,
    toplevel: xdg_toplevel::XdgToplevel,
    decoration: Option<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1>,
    viewport: Option<wp_viewport::WpViewport>,
    fractional: Option<wp_fractional_scale_v1::WpFractionalScaleV1>,
    size: Size<Pixels>,
    windowed: Size<Pixels>,
    scale: f32,
    outputs: Vec<u32>,
    pending: Option<Configure>,
    maximized: bool,
    fullscreen: bool,
    resizing: bool,
    suspended: bool,
    active: bool,
}

pub(crate) struct Window {
    shared: Rc<Shared>,
    client: Rc<Client>,
    atlas: Rc<GpuAtlas>,
}

impl Window {
    pub fn open(
        client: &Rc<Client>,
        options: &WindowOptions,
        callbacks: Callbacks,
    ) -> Result<Self> {
        let qh = &client.qh;
        let globals = &client.globals;
        let surface = globals.compositor.create_surface(qh, ());
        let id = surface.id();
        let xdg = globals.wm_base.get_xdg_surface(&surface, qh, id.clone());
        let toplevel = xdg.get_toplevel(qh, id.clone());
        toplevel.set_title(options.title.clone());
        toplevel.set_app_id(super::super::app_name());
        let decoration = globals.decoration_manager.as_ref().map(|manager| {
            let decoration = manager.get_toplevel_decoration(&toplevel, qh, id.clone());
            decoration.set_mode(if options.titlebar_hidden || options.frameless {
                Mode::ClientSide
            } else {
                Mode::ServerSide
            });
            decoration
        });
        let fractional = globals
            .fractional_scale_manager
            .as_ref()
            .map(|manager| manager.get_fractional_scale(&surface, qh, id.clone()));
        let viewport = globals
            .viewporter
            .as_ref()
            .map(|viewporter| viewporter.get_viewport(&surface, qh, ()));
        let size = options.bounds.size;
        let display = rwh::RawDisplayHandle::Wayland(rwh::WaylandDisplayHandle::new(
            NonNull::new(client.conn.backend().display_ptr().cast()).expect("a Wayland display"),
        ));
        let renderer = match Renderer::new(
            display,
            window_handle(&surface),
            size.to_device_pixels(1.),
            options.transparent,
            true,
        ) {
            Ok(renderer) => renderer,
            Err(error) => {
                if let Some(decoration) = &decoration {
                    decoration.destroy();
                }
                if let Some(viewport) = &viewport {
                    viewport.destroy();
                }
                if let Some(fractional) = &fractional {
                    fractional.destroy();
                }
                toplevel.destroy();
                xdg.destroy();
                surface.destroy();
                return Err(error);
            }
        };
        if options.resizable {
            let max = renderer.max_texture_size() as i32;
            toplevel.set_max_size(max, max);
        } else {
            let (width, height) = (size.width.0 as i32, size.height.0 as i32);
            toplevel.set_min_size(width, height);
            toplevel.set_max_size(width, height);
        }
        let atlas = renderer.atlas().clone();
        let shared = Rc::new(Shared {
            surface: surface.clone(),
            callbacks,
            client: Rc::downgrade(client),
            transparent: options.transparent,
            state: RefCell::new(State {
                renderer: Some(renderer),
                xdg,
                toplevel,
                decoration,
                viewport,
                fractional,
                size,
                windowed: size,
                scale: 1.,
                outputs: Vec::new(),
                pending: None,
                maximized: false,
                fullscreen: false,
                resizing: false,
                suspended: false,
                active: false,
            }),
            wants: Cell::new(false),
            callback: Cell::new(false),
            scheduled: Cell::new(false),
            configured: Cell::new(false),
            shown: Cell::new(options.show),
            presented: Cell::new(false),
        });
        client
            .state
            .borrow_mut()
            .windows
            .insert(id, Rc::downgrade(&shared));
        surface.commit();
        client.consume_startup_token(&surface);
        Ok(Self {
            shared,
            client: client.clone(),
            atlas,
        })
    }

    pub fn atlas(&self) -> &Rc<GpuAtlas> {
        &self.atlas
    }

    pub fn scale_factor(&self) -> f32 {
        self.shared.state.borrow().scale
    }

    pub fn content_size(&self) -> Size<Pixels> {
        self.shared.state.borrow().size
    }

    pub fn request_frame(&self) {
        self.shared.wants.set(true);
        self.shared.schedule();
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
        let shared = &self.shared;
        let (drawn, capture) = {
            let mut state = shared.state.borrow_mut();
            if !shared.callback.replace(true) {
                shared.surface.frame(&self.client.qh, shared.surface.id());
            }
            state.renderer.as_mut()?.draw(scene, capture)
        };
        match drawn {
            Drawn::Presented => shared.presented.set(true),
            _ if shared.presented.get() => shared.surface.commit(),
            _ => shared.callback.set(false),
        }
        if drawn == Drawn::Repaint {
            shared.callbacks.appearance_changed();
        }
        capture
    }

    pub fn is_active(&self) -> bool {
        self.shared.state.borrow().active
    }

    pub fn set_title(&self, title: &str) {
        self.shared
            .state
            .borrow()
            .toplevel
            .set_title(title.to_owned());
    }

    pub fn state(&self) -> WindowState {
        let state = self.shared.state.borrow();
        let bounds = Bounds::new(point(px(0.), px(0.)), state.size);
        let display = self
            .shared
            .client
            .upgrade()
            .and_then(|client| client.primary_display_bounds())
            .unwrap_or_default();
        WindowState {
            frame: bounds,
            content: bounds,
            minimized: false,
            maximized: state.maximized && !state.fullscreen,
            fullscreen: state.fullscreen,
            visible: self.shared.shown.get() && !state.suspended,
            resizing: state.resizing,
            display,
            work_area: display,
        }
    }

    pub fn minimize(&self) {
        self.shared.state.borrow().toplevel.set_minimized();
    }

    pub fn zoom(&self) {
        let state = self.shared.state.borrow();
        if state.maximized {
            state.toplevel.unset_maximized();
        } else {
            state.toplevel.set_maximized();
        }
    }

    pub fn toggle_fullscreen(&self) {
        let state = self.shared.state.borrow();
        if state.fullscreen {
            state.toplevel.unset_fullscreen();
        } else {
            state.toplevel.set_fullscreen(None);
        }
    }

    pub fn activate(&self) {
        if !self.shared.shown.replace(true) {
            self.request_frame();
        }
        self.client.activate(&self.shared.surface);
    }

    pub fn resize(&self, size: Size<Pixels>) {
        self.shared.set_size_and_scale(Some(size), None);
    }

    pub fn start_window_move(&self) {
        if let Some(serial) = self.client.press_serial() {
            self.shared
                .state
                .borrow()
                .toplevel
                ._move(&self.client.state.borrow().seat, serial);
        }
    }

    pub fn raw_window_handle(&self) -> rwh::RawWindowHandle {
        window_handle(&self.shared.surface)
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        let mut state = self.shared.state.borrow_mut();
        state.renderer.take();
        // WARNING: roles go before their surfaces.
        if let Some(decoration) = &state.decoration {
            decoration.destroy();
        }
        state.toplevel.destroy();
        state.xdg.destroy();
        if let Some(viewport) = &state.viewport {
            viewport.destroy();
        }
        if let Some(fractional) = &state.fractional {
            fractional.destroy();
        }
        self.shared.surface.destroy();
        drop(state);
        self.client.forget(&self.shared.surface.id());
    }
}

impl Shared {
    fn schedule(self: &Rc<Self>) {
        if !self.configured.get()
            || !self.shown.get()
            || self.callback.get()
            || self.scheduled.replace(true)
        {
            return;
        }
        let shared = Rc::downgrade(self);
        post_local(move || {
            if let Some(shared) = shared.upgrade() {
                shared.scheduled.set(false);
                shared.tick();
            }
        });
    }

    fn tick(self: &Rc<Self>) {
        if self.callbacks.frame() == Some(false) {
            self.wants.set(false);
            return;
        }
        if self.callback.get() || self.scheduled.replace(true) {
            return;
        }
        let shared = Rc::downgrade(self);
        run_after(RETRY_INTERVAL, move || {
            if let Some(shared) = shared.upgrade() {
                shared.scheduled.set(false);
                if shared.wants.get() {
                    shared.tick();
                }
            }
        });
    }

    pub fn scale(&self) -> f32 {
        self.state.borrow().scale
    }

    pub fn frame_done(self: &Rc<Self>) {
        self.callback.set(false);
        if self.wants.get() && !self.scheduled.get() {
            self.tick();
        }
    }

    pub fn set_active(&self, active: bool) {
        self.state.borrow_mut().active = active;
        self.callbacks.changed();
    }

    fn set_size_and_scale(&self, size: Option<Size<Pixels>>, scale: Option<f32>) {
        let Some(client) = self.client.upgrade() else {
            return;
        };
        let (size, scale, changed) = {
            let mut state = self.state.borrow_mut();
            let changed = size.is_some_and(|size| size != state.size)
                || scale.is_some_and(|scale| scale != state.scale);
            let size = size.unwrap_or(state.size);
            let scale = scale.unwrap_or(state.scale);
            let (width, height) = (
                (size.width.0.round() as i32).max(1),
                (size.height.0.round() as i32).max(1),
            );
            if changed {
                state.size = size;
                state.scale = scale;
                if let Some(renderer) = &mut state.renderer {
                    renderer.update_drawable_size(size.to_device_pixels(scale));
                }
                if let Some(viewport) = &state.viewport {
                    viewport.set_destination(width, height);
                }
            }
            state.xdg.set_window_geometry(0, 0, width, height);
            if !self.transparent {
                let region = client.globals.compositor.create_region(&client.qh, ());
                region.add(0, 0, width, height);
                self.surface.set_opaque_region(Some(&region));
                region.destroy();
            }
            (size, scale, changed)
        };
        if changed {
            self.callbacks.resized(size, scale);
        }
    }

    pub fn handle_toplevel(&self, event: xdg_toplevel::Event) {
        match event {
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => {
                let mut configure = Configure {
                    size: (width > 0 && height > 0)
                        .then(|| size(px(width as f32), px(height as f32))),
                    maximized: false,
                    fullscreen: false,
                    resizing: false,
                    suspended: false,
                };
                for state in states
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|bytes| u32::from_ne_bytes(*bytes))
                    .filter_map(|value| xdg_toplevel::State::try_from(value).ok())
                {
                    match state {
                        xdg_toplevel::State::Maximized => configure.maximized = true,
                        xdg_toplevel::State::Fullscreen => configure.fullscreen = true,
                        xdg_toplevel::State::Resizing => configure.resizing = true,
                        xdg_toplevel::State::Suspended => configure.suspended = true,
                        _ => {}
                    }
                }
                self.state.borrow_mut().pending = Some(configure);
            }
            xdg_toplevel::Event::Close if self.callbacks.should_close() => {
                let id = self.callbacks.id;
                crate::app::update(move |app| {
                    app.update_window(id, |window, _| window.remove_window());
                });
            }
            _ => {}
        }
    }

    pub fn handle_configure(self: &Rc<Self>, serial: u32) {
        let size = {
            let mut state = self.state.borrow_mut();
            let size = state.pending.take().and_then(|configure| {
                let unmaximized = state.maximized && !configure.maximized;
                state.maximized = configure.maximized;
                state.fullscreen = configure.fullscreen;
                state.resizing = configure.resizing;
                state.suspended = configure.suspended;
                if configure.maximized || configure.fullscreen {
                    return configure.size;
                }
                let size = if unmaximized {
                    Some(state.windowed)
                } else {
                    configure.size
                };
                if let Some(size) = size {
                    state.windowed = size;
                }
                size
            });
            state.xdg.ack_configure(serial);
            size
        };
        self.set_size_and_scale(size, None);
        if self.configured.replace(true) {
            self.callbacks.appearance_changed();
        } else {
            self.schedule();
        }
        self.callbacks.changed();
    }

    pub fn handle_decoration(&self, event: zxdg_toplevel_decoration_v1::Event) {
        if let zxdg_toplevel_decoration_v1::Event::Configure {
            mode: WEnum::Value(_),
        } = event
        {
            self.callbacks.appearance_changed();
        }
    }

    pub fn handle_fractional(&self, event: wp_fractional_scale_v1::Event) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            self.set_size_and_scale(None, Some(scale as f32 / 120.));
        }
    }

    pub fn handle_surface(&self, event: wl_surface::Event, outputs: &FxHashMap<u32, Output>) {
        let Some(client) = self.client.upgrade() else {
            return;
        };
        let fractional = client.globals.fractional_scale_manager.is_some();
        let scale = match &event {
            wl_surface::Event::PreferredBufferScale { factor } if !fractional => Some(*factor),
            wl_surface::Event::Enter { output } | wl_surface::Event::Leave { output }
                if !fractional
                    && self.surface.version() < wl_surface::EVT_PREFERRED_BUFFER_SCALE_SINCE =>
            {
                let mut state = self.state.borrow_mut();
                let id = output.id().protocol_id();
                if matches!(event, wl_surface::Event::Enter { .. }) {
                    state.outputs.push(id);
                } else {
                    state.outputs.retain(|entered| *entered != id);
                }
                Some(
                    state
                        .outputs
                        .iter()
                        .filter_map(|id| outputs.get(id))
                        .map(|output| output.scale)
                        .max()
                        .unwrap_or(1),
                )
            }
            _ => None,
        };
        if let Some(scale) = scale {
            self.surface.set_buffer_scale(scale);
            self.set_size_and_scale(None, Some(scale as f32));
        }
    }
}

fn window_handle(surface: &wl_surface::WlSurface) -> rwh::RawWindowHandle {
    rwh::RawWindowHandle::Wayland(rwh::WaylandWindowHandle::new(
        NonNull::new(surface.id().as_ptr().cast()).expect("a live Wayland surface"),
    ))
}
