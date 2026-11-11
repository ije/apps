use super::{Client, Hub, route};
use crate::platform::linux::gpu::{Atlas, Capture, Drawn, Renderer};
use crate::platform::linux::{Action, Display as _, Native, run_after, soon};
use crate::{Pixels, Rect, Scene, Size, WindowOptions, WindowState, app::Callbacks, point, px};
use anyhow::Result;
use raw_window_handle::{RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle};
use std::{cell::Cell, cell::RefCell, ptr::NonNull, rc::Rc, rc::Weak, time::Duration};
use wayland_backend::client::ObjectId;
use wayland_client::protocol::{wl_callback, wl_surface, wl_surface::WlSurface};
use wayland_client::{Proxy as _, WEnum};
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::xdg::decoration::zv1::client::zxdg_toplevel_decoration_v1 as decoration;
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel};

// A frame that drew nothing gets no frame callback, so the next one waits this long instead.
const RETRY: Duration = Duration::from_micros(16_667);

route! {
    WlSurface, () => on_surface;
    wl_callback::WlCallback, ObjectId => on_callback;
    xdg_surface::XdgSurface, ObjectId => on_xdg;
    xdg_toplevel::XdgToplevel, ObjectId => on_toplevel;
    decoration::ZxdgToplevelDecorationV1, ObjectId => on_decoration;
    wp_fractional_scale_v1::WpFractionalScaleV1, ObjectId => on_fraction;
}

// What a toplevel configure asks for, applied when its surface configure comes.
#[derive(Default)]
struct Asked {
    extent: Option<Size<Pixels>>,
    maximized: bool,
    fullscreen: bool,
    resizing: bool,
    suspended: bool,
}

struct Shape {
    extent: Size<Pixels>,
    windowed: Size<Pixels>,
    scale: f32,
    // A buffer scale takes effect with the buffer that matches it, so it waits for the next present.
    buffer_scale: Option<i32>,
    applied_scale: i32,
    outputs: Vec<ObjectId>,
    asked: Option<Asked>,
    maximized: bool,
    fullscreen: bool,
    resizing: bool,
    suspended: bool,
    active: bool,
}

#[derive(Default)]
struct Pace {
    wants: Cell<bool>,
    waiting: Cell<bool>,
    queued: Cell<bool>,
    configured: Cell<bool>,
    shown: Cell<bool>,
    presented: Cell<bool>,
}

// Dropping them destroys the roles before their surface.
struct Roles {
    surface: WlSurface,
    xdg: xdg_surface::XdgSurface,
    role: xdg_toplevel::XdgToplevel,
    frame: Option<decoration::ZxdgToplevelDecorationV1>,
    viewport: Option<WpViewport>,
    fraction: Option<wp_fractional_scale_v1::WpFractionalScaleV1>,
}

impl Drop for Roles {
    fn drop(&mut self) {
        self.frame
            .iter()
            .for_each(decoration::ZxdgToplevelDecorationV1::destroy);
        self.role.destroy();
        self.xdg.destroy();
        self.viewport.iter().for_each(WpViewport::destroy);
        self.fraction
            .iter()
            .for_each(wp_fractional_scale_v1::WpFractionalScaleV1::destroy);
        self.surface.destroy();
    }
}

pub(super) struct Surface {
    me: Weak<Surface>,
    pub calls: Callbacks,
    client: Weak<Client>,
    transparent: bool,
    // Fields drop in order: GPU objects must go before the surface.
    renderer: RefCell<Option<Renderer>>,
    roles: Roles,
    atlas: Rc<Atlas>,
    shape: RefCell<Shape>,
    pace: Pace,
}

fn handle(surface: &WlSurface) -> RawWindowHandle {
    let pointer = NonNull::new(surface.id().as_ptr().cast()).expect("a live Wayland surface");
    RawWindowHandle::Wayland(WaylandWindowHandle::new(pointer))
}

impl Surface {
    pub fn open(
        client: &Rc<Client>,
        options: &WindowOptions,
        calls: Callbacks,
    ) -> Result<Rc<Self>> {
        let (qh, globals) = (&client.qh, &client.globals);
        let surface = globals.compositor.create_surface(qh, ());
        let id = surface.id();
        let xdg = globals.wm_base.get_xdg_surface(&surface, qh, id.clone());
        let role = xdg.get_toplevel(qh, id.clone());
        role.set_title(options.title.clone());
        role.set_app_id(super::super::app_name());
        let frame = globals.decorations.as_ref().map(|manager| {
            let frame = manager.get_toplevel_decoration(&role, qh, id.clone());
            let bare = options.titlebar_hidden || options.frameless;
            frame.set_mode(if bare {
                decoration::Mode::ClientSide
            } else {
                decoration::Mode::ServerSide
            });
            frame
        });
        let fraction = globals
            .fractions
            .as_ref()
            .map(|maker| maker.get_fractional_scale(&surface, qh, id.clone()));
        let viewport = globals
            .viewporter
            .as_ref()
            .map(|maker| maker.get_viewport(&surface, qh, ()));
        let roles = Roles {
            surface,
            xdg,
            role,
            frame,
            viewport,
            fraction,
        };
        let extent = options.bounds.size;
        let pixels = [extent.width, extent.height].map(|side| side.0.round().max(1.) as u32);
        let display = NonNull::new(client.conn.backend().display_ptr().cast())
            .map(|display| {
                raw_window_handle::RawDisplayHandle::Wayland(WaylandDisplayHandle::new(display))
            })
            .ok_or_else(|| anyhow::anyhow!("no Wayland display"));
        let renderer = display.and_then(|display| {
            Renderer::new(
                display,
                handle(&roles.surface),
                pixels,
                options.transparent,
                true,
            )
        })?;
        let role = &roles.role;
        if options.resizable {
            let max = renderer.max_side() as i32;
            role.set_max_size(max, max);
        } else {
            let [width, height] = pixels.map(|side| side as i32);
            role.set_min_size(width, height);
            role.set_max_size(width, height);
        }
        let shape = Shape {
            extent,
            windowed: extent,
            scale: 1.,
            buffer_scale: None,
            applied_scale: 1,
            outputs: Vec::new(),
            asked: None,
            maximized: false,
            fullscreen: false,
            resizing: false,
            suspended: false,
            active: false,
        };
        let window = Rc::new_cyclic(|me| Self {
            me: me.clone(),
            calls,
            client: Rc::downgrade(client),
            atlas: renderer.atlas().clone(),
            renderer: RefCell::new(Some(renderer)),
            roles,
            transparent: options.transparent,
            shape: RefCell::new(shape),
            pace: Pace::default(),
        });
        window.pace.shown.set(options.show);
        client
            .live
            .borrow_mut()
            .windows
            .insert(id, Rc::downgrade(&window));
        // The first commit has no buffer and asks for the first configure.
        window.roles.surface.commit();
        Ok(window)
    }

    pub fn surface(&self) -> &WlSurface {
        &self.roles.surface
    }

    pub fn focus(&self, active: bool) {
        self.shape.borrow_mut().active = active;
        let calls = self.calls;
        soon(move || calls.changed());
    }

    fn kick(&self) {
        let pace = &self.pace;
        let ready = pace.wants.get() && pace.configured.get() && pace.shown.get();
        if !ready || pace.waiting.get() || pace.queued.replace(true) {
            return;
        }
        let me = self.me.clone();
        soon(move || {
            if let Some(window) = me.upgrade() {
                window.pace.queued.set(false);
                window.tick();
            }
        });
    }

    fn tick(&self) {
        let pace = &self.pace;
        if self.calls.frame() == Some(false) {
            pace.wants.set(false);
            return;
        }
        if pace.waiting.get() || pace.queued.replace(true) {
            return;
        }
        let me = self.me.clone();
        run_after(RETRY, move || {
            if let Some(window) = me.upgrade() {
                window.pace.queued.set(false);
                if window.pace.wants.get() {
                    window.tick();
                }
            }
        });
    }

    fn set_size(&self, extent: Option<Size<Pixels>>, scale: Option<f32>) {
        let Some(client) = self.client.upgrade() else {
            return;
        };
        let mut shape = self.shape.borrow_mut();
        let extent = extent.unwrap_or(shape.extent);
        let scale = scale.unwrap_or(shape.scale);
        let changed = extent != shape.extent || scale != shape.scale;
        let [width, height] =
            [extent.width, extent.height].map(|side| (side.0.round() as i32).max(1));
        if changed {
            (shape.extent, shape.scale) = (extent, scale);
            if let Some(renderer) = &mut *self.renderer.borrow_mut() {
                renderer.resize([width, height].map(|side| (side as f32 * scale).round() as u32));
            }
            if let Some(viewport) = &self.roles.viewport {
                viewport.set_destination(width, height);
            }
        }
        drop(shape);
        self.roles.xdg.set_window_geometry(0, 0, width, height);
        if !self.transparent {
            let region = client.globals.compositor.create_region(&client.qh, ());
            region.add(0, 0, width, height);
            self.roles.surface.set_opaque_region(Some(&region));
            region.destroy();
        }
        if changed {
            let calls = self.calls;
            soon(move || calls.resized(extent, scale));
        }
    }

    fn configure(&self, serial: u32) {
        let extent = {
            let shape = &mut *self.shape.borrow_mut();
            shape.asked.take().and_then(|asked| {
                let restored = shape.maximized && !asked.maximized;
                shape.maximized = asked.maximized;
                shape.fullscreen = asked.fullscreen;
                shape.resizing = asked.resizing;
                shape.suspended = asked.suspended;
                if asked.maximized || asked.fullscreen {
                    return asked.extent;
                }
                let extent = if restored {
                    Some(shape.windowed)
                } else {
                    asked.extent
                };
                shape.windowed = extent.unwrap_or(shape.windowed);
                extent
            })
        };
        // The ack goes first, so the next commit answers this configure.
        self.roles.xdg.ack_configure(serial);
        self.set_size(extent, None);
        let first = !self.pace.configured.replace(true);
        let calls = self.calls;
        soon(move || {
            if !first {
                calls.appearance_changed();
            }
            calls.changed();
        });
        if first {
            self.kick();
        }
    }

    fn integer_scale(&self, scale: i32) {
        let scale = scale.max(1);
        self.shape.borrow_mut().buffer_scale = Some(scale);
        self.set_size(None, Some(scale as f32));
    }
}

impl Native for Surface {
    fn scale(&self) -> f32 {
        self.shape.borrow().scale
    }

    fn extent(&self) -> Size<Pixels> {
        self.shape.borrow().extent
    }

    fn state(&self) -> WindowState {
        let shape = self.shape.borrow();
        let bounds = Rect::new(point(px(0.), px(0.)), shape.extent);
        let display = self
            .client
            .upgrade()
            .and_then(|client| client.display_bounds());
        let display = display.unwrap_or_default();
        WindowState {
            frame: bounds,
            content: bounds,
            minimized: false,
            maximized: shape.maximized && !shape.fullscreen,
            fullscreen: shape.fullscreen,
            visible: self.pace.shown.get() && !shape.suspended,
            resizing: shape.resizing,
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
        self.pace.wants.set(true);
        self.kick();
    }

    fn draw(&self, scene: &Scene, capture: bool) -> Capture {
        let Some(client) = self.client.upgrade() else {
            return capture.then(|| Err(anyhow::anyhow!("The display is gone")));
        };
        let pace = &self.pace;
        // A buffer before the first configure is a protocol error, and a buffer on a hidden window shows it.
        if !pace.configured.get() || !pace.shown.get() {
            return capture.then(|| Err(anyhow::anyhow!("The window is not on screen")));
        }
        // The present commits this request with its buffer.
        if !pace.waiting.replace(true) {
            self.roles
                .surface
                .frame(&client.qh, self.roles.surface.id());
        }
        let scale = self.shape.borrow_mut().buffer_scale.take();
        if let Some(scale) = scale {
            self.roles.surface.set_buffer_scale(scale);
        }
        let (drawn, capture) = match &mut *self.renderer.borrow_mut() {
            Some(renderer) => renderer.draw(scene, capture),
            None => (Drawn::Skipped, None),
        };
        let mut shape = self.shape.borrow_mut();
        match (drawn, scale) {
            (Drawn::Shown, Some(scale)) => shape.applied_scale = scale,
            // The old buffer stays, so its scale must too.
            (_, Some(scale)) => {
                self.roles.surface.set_buffer_scale(shape.applied_scale);
                shape.buffer_scale = Some(scale);
            }
            _ => {}
        }
        drop(shape);
        match drawn {
            Drawn::Shown => pace.presented.set(true),
            // A commit without a buffer still gets the frame callback, once a buffer is on screen.
            _ if pace.presented.get() => self.roles.surface.commit(),
            _ => pace.waiting.set(false),
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
        let client = self.client.upgrade();
        let role = &self.roles.role;
        match action {
            Action::Title(title) => role.set_title(title),
            Action::Minimize => role.set_minimized(),
            Action::Zoom if self.shape.borrow().maximized => role.unset_maximized(),
            Action::Zoom => role.set_maximized(),
            Action::Fullscreen if self.shape.borrow().fullscreen => role.unset_fullscreen(),
            Action::Fullscreen => role.set_fullscreen(None),
            Action::Raise => {
                if !self.pace.shown.replace(true) {
                    self.want_frames();
                }
                if let Some(client) = client {
                    client.activate(&self.roles.surface);
                }
            }
            Action::Resize(extent) => self.set_size(Some(extent), None),
            Action::Move => {
                if let Some(client) = client
                    && let Some(serial) = client.press_serial()
                {
                    role._move(&client.live.borrow().seat, serial);
                }
            }
            // A Wayland toplevel gets no parent here.
            Action::Parent(_) => {}
            Action::CancelComposition => {
                if let Some(client) = client {
                    client.cancel_composition();
                }
            }
        }
    }

    fn handle(&self) -> RawWindowHandle {
        handle(&self.roles.surface)
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        if let Some(client) = self.client.upgrade() {
            client.forget(&self.roles.surface.id());
        }
    }
}

impl Client {
    fn window_of(&self, id: &ObjectId) -> Option<Rc<Surface>> {
        self.live.borrow().window(Some(id))
    }

    fn on_surface(&self, surface: &WlSurface, message: wl_surface::Event, _: &()) {
        let Some(window) = self
            .window_of(&surface.id())
            .filter(|window| window.roles.fraction.is_none())
        else {
            return;
        };
        let entered = matches!(message, wl_surface::Event::Enter { .. });
        match message {
            wl_surface::Event::PreferredBufferScale { factor } => window.integer_scale(factor),
            // Before version 6 the scale is the largest of the outputs the surface is on.
            wl_surface::Event::Enter { output } | wl_surface::Event::Leave { output }
                if surface.version() < 6 =>
            {
                let largest = {
                    let mut shape = window.shape.borrow_mut();
                    shape.outputs.retain(|kept| *kept != output.id());
                    if entered {
                        shape.outputs.push(output.id());
                    }
                    let live = self.live.borrow();
                    let on = live
                        .screens
                        .values()
                        .filter(|(kept, _)| shape.outputs.contains(&kept.id()));
                    on.map(|(_, screen)| screen.scale).max().unwrap_or(1)
                };
                window.integer_scale(largest);
            }
            _ => {}
        }
    }

    fn on_callback(
        &self,
        _: &wl_callback::WlCallback,
        message: wl_callback::Event,
        surface: &ObjectId,
    ) {
        if let wl_callback::Event::Done { .. } = message
            && let Some(window) = self.window_of(surface)
        {
            window.pace.waiting.set(false);
            window.kick();
        }
    }

    fn on_xdg(&self, _: &xdg_surface::XdgSurface, message: xdg_surface::Event, surface: &ObjectId) {
        if let xdg_surface::Event::Configure { serial } = message
            && let Some(window) = self.window_of(surface)
        {
            window.configure(serial);
        }
    }

    fn on_toplevel(
        &self,
        _: &xdg_toplevel::XdgToplevel,
        message: xdg_toplevel::Event,
        surface: &ObjectId,
    ) {
        let Some(window) = self.window_of(surface) else {
            return;
        };
        match message {
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => {
                let given = width > 0 && height > 0;
                let extent = given.then(|| crate::size(px(width as f32), px(height as f32)));
                let mut asked = Asked {
                    extent,
                    ..Asked::default()
                };
                // The states come as an array of 32-bit words.
                for word in states.as_chunks::<4>().0 {
                    match xdg_toplevel::State::try_from(u32::from_ne_bytes(*word)) {
                        Ok(xdg_toplevel::State::Maximized) => asked.maximized = true,
                        Ok(xdg_toplevel::State::Fullscreen) => asked.fullscreen = true,
                        Ok(xdg_toplevel::State::Resizing) => asked.resizing = true,
                        Ok(xdg_toplevel::State::Suspended) => asked.suspended = true,
                        _ => {}
                    }
                }
                window.shape.borrow_mut().asked = Some(asked);
            }
            xdg_toplevel::Event::Close => {
                let calls = window.calls;
                soon(move || {
                    calls.should_close();
                });
            }
            _ => {}
        }
    }

    fn on_decoration(
        &self,
        _: &decoration::ZxdgToplevelDecorationV1,
        message: decoration::Event,
        surface: &ObjectId,
    ) {
        if let decoration::Event::Configure {
            mode: WEnum::Value(_),
        } = message
            && let Some(window) = self.window_of(surface)
        {
            let calls = window.calls;
            soon(move || calls.appearance_changed());
        }
    }

    fn on_fraction(
        &self,
        _: &wp_fractional_scale_v1::WpFractionalScaleV1,
        message: wp_fractional_scale_v1::Event,
        surface: &ObjectId,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = message
            && let Some(window) = self.window_of(surface)
        {
            window.set_size(None, Some(scale as f32 / 120.));
        }
    }
}
