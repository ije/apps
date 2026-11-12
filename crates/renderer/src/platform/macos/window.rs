use super::{
    Sendable, lock,
    metal::{Atlas, Present, Renderer},
    nsview::AppsView,
    nswindow::{self, AppsWindow},
    threads::{native_post, native_sync, post, post_local},
    vsync::{Pacer, step},
};
use crate::{
    Pixels, Point, Rect, Scene, Size, WindowChange, WindowOptions, WindowState, app::Callbacks,
    point, px, rect, size,
};
use anyhow::{Context as _, Result};
use objc2::{DefinedClass, MainThreadMarker, rc::Retained};
use objc2_app_kit::{NSScreen, NSWindow, NSWindowStyleMask};
use objc2_core_foundation::CGSize;
use objc2_foundation::NSRect;
use raw_window_handle as rwh;
use std::{
    ptr::NonNull,
    rc::Rc,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

// After this long without frames, the window gives back its spare GPU memory.
const QUIET: Duration = Duration::from_secs(1);

// The display link stays on for this many ticks after the last frame. A start takes 4 to 11 ms, so
// frames that come one by one, like a 60 Hz scroll on a 120 Hz display, would land unevenly.
const LINGER: u32 = 30;

#[derive(Clone, Copy, Default)]
pub(crate) struct Snapshot {
    pub scale: f32,
    pub active: bool,
    pub state: WindowState,
}

pub(crate) type Shared<T> = Arc<Mutex<T>>;

/// Flips an AppKit screen rectangle to the core's top-left origin on the primary display.
pub(crate) fn flip(area: NSRect, top: f64) -> Rect {
    let y = top - area.origin.y - area.size.height;
    let [x, y, width, height] =
        [area.origin.x, y, area.size.width, area.size.height].map(|v| px(v as f32));
    rect(x, y, width, height)
}

pub(crate) fn cg_size(size: Size<Pixels>) -> CGSize {
    CGSize::new(size.width.0.into(), size.height.0.into())
}

pub(crate) fn primary_top(mtm: MainThreadMarker) -> f64 {
    NSScreen::screens(mtm)
        .firstObject()
        .map_or(0., |screen| screen.frame().size.height)
}

#[derive(Clone, Copy)]
struct Display {
    titlebar: Pixels,
    scale: f32,
    frame: Rect,
    work_area: Rect,
}

static DISPLAY: Mutex<Option<Display>> = Mutex::new(None);

pub(crate) fn measure_screens() {
    let mtm = MainThreadMarker::new().unwrap();
    let Some(screen) = NSScreen::screens(mtm)
        .firstObject()
        .or_else(|| NSScreen::mainScreen(mtm))
    else {
        return;
    };
    let top = primary_top(mtm);
    let inner = NSRect::new(Default::default(), CGSize::new(100., 100.));
    let outer = NSWindow::frameRectForContentRect_styleMask(
        inner,
        NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
        mtm,
    );
    let scale = screen.backingScaleFactor() as f32;
    *lock(&DISPLAY) = Some(Display {
        titlebar: px((outer.size.height - inner.size.height) as f32),
        scale: if scale == 0. { 2. } else { scale },
        frame: flip(screen.frame(), top),
        work_area: flip(screen.visibleFrame(), top),
    });
}

// The app thread draws before main opened the window, so it starts from this guess.
fn guess(options: &WindowOptions) -> Snapshot {
    let known = *lock(&DISPLAY);
    let Some(display) = known.or_else(|| {
        native_sync(measure_screens);
        *lock(&DISPLAY)
    }) else {
        return Snapshot::default();
    };
    let bar = match options.titlebar_hidden || options.frameless {
        true => px(0.),
        false => display.titlebar,
    };
    let mut frame = options.bounds;
    frame.origin += display.frame.origin;
    if !options.outer_size {
        frame.size.height += bar;
    }
    let content = Rect::new(
        frame.origin + point(px(0.), bar),
        size(frame.size.width, frame.size.height - bar),
    );
    Snapshot {
        scale: display.scale,
        active: false,
        state: WindowState {
            frame,
            content,
            visible: options.show,
            display: display.frame,
            work_area: display.work_area,
            ..WindowState::default()
        },
    }
}

type Opener = Box<dyn FnOnce() -> Option<(Retained<AppsWindow>, Retained<AppsView>)> + Send>;

struct Handles {
    window: NonNull<AppsWindow>,
    view: NonNull<AppsView>,
}

// SAFETY: the pointers are dereferenced on main only; the view pointer goes out as a raw window handle.
unsafe impl Send for Handles {}
unsafe impl Sync for Handles {}

/// The app thread waits for it before it draws or hands out the view.
struct Native {
    opener: Mutex<Option<Opener>>,
    // Unset until the opener has run or was dropped, then `None` for a window that did not open.
    handles: OnceLock<Option<Handles>>,
}

impl Native {
    // Main only.
    fn get(&self) -> Option<&AppsWindow> {
        let opener = lock(&self.opener).take();
        if let Some(open) = opener {
            let handles = open().map(|(window, view)| Handles {
                window: NonNull::new(Retained::into_raw(window)).unwrap(),
                view: NonNull::from(&*view),
            });
            let _ = self.handles.set(handles);
        }
        // SAFETY: the window stays retained until `Drop` releases it on main.
        self.opened()
            .map(|handles| unsafe { handles.window.as_ref() })
    }

    fn opened(&self) -> Option<&Handles> {
        self.handles.get().and_then(Option::as_ref)
    }

    fn wait(self: &Arc<Self>) {
        // Also while main runs the opener: it has taken it by then, but the handles are not there yet.
        if self.handles.get().is_none() {
            let native = self.clone();
            native_sync(move || {
                native.get();
            });
        }
    }

    // Main only. Skips windows that closed.
    fn with(&self, job: impl FnOnce(&AppsWindow)) {
        if let Some(window) = self.get().filter(|window| !window.ivars().closed.get()) {
            job(window);
        }
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        if let Some(handles) = self.handles.take().flatten() {
            let window = Sendable::new(handles.window);
            // SAFETY: balances `into_raw` in `get`.
            native_post(Box::new(move || {
                drop(unsafe { Retained::from_raw(window.take().as_ptr()) })
            }));
        }
    }
}

#[derive(Clone)]
pub struct WindowControl {
    native: Arc<Native>,
    snapshot: Shared<Snapshot>,
}

// Main gets only the newest of these, so a busy main cannot run out of drawables or fall behind.
struct Latest<T> {
    value: Mutex<Option<T>>,
    posted: AtomicBool,
}

impl<T> Default for Latest<T> {
    fn default() -> Self {
        Self {
            value: Mutex::new(None),
            posted: AtomicBool::new(false),
        }
    }
}

impl<T: Send + 'static> Latest<T> {
    fn send(self: &Arc<Self>, value: T, apply: impl FnOnce(&Arc<Self>) + Send + 'static) {
        *lock(&self.value) = Some(value);
        self.post(apply);
    }

    fn post(self: &Arc<Self>, apply: impl FnOnce(&Arc<Self>) + Send + 'static) {
        if !self.posted.swap(true, Ordering::AcqRel) {
            let latest = self.clone();
            native_post(Box::new(move || {
                latest.posted.store(false, Ordering::Release);
                apply(&latest);
            }));
        }
    }
}

type Controls = (Vec<crate::NativeControl>, Option<crate::ControlSink>);

pub(crate) struct Window {
    control: WindowControl,
    callbacks: Callbacks,
    pub(crate) renderer: Renderer,
    pacer: Shared<Pacer>,
    idle: u32,
    quiet: Option<crate::app::Timer>,
    presents: Arc<Latest<Present>>,
    controls_sent: Vec<crate::NativeControl>,
    controls: Arc<Latest<Controls>>,
    transparent: bool,
}

impl std::ops::Deref for Window {
    type Target = WindowControl;
    fn deref(&self) -> &WindowControl {
        &self.control
    }
}

impl Window {
    pub fn open(options: &WindowOptions, callbacks: Callbacks) -> Result<Self> {
        let renderer = Renderer::new(options.transparent)?;
        let pacer = Pacer::new(callbacks);
        let snapshot = Arc::new(Mutex::new(guess(options)));
        let layers = Sendable::new((renderer.layer().clone(), renderer.overlay().clone()));
        let transparent = options.transparent;
        let (options, shared) = (options.clone(), (pacer.clone(), snapshot.clone()));
        let opener: Opener = Box::new(move || {
            let (layer, overlay) = layers.take();
            let (pacer, snapshot) = shared;
            let opened = nswindow::open(&options, callbacks, layer, overlay, pacer, snapshot);
            let (window, view) = opened
                .inspect_err(|error| eprintln!("apps: {error:#}"))
                .ok()?;
            measure_screens();
            // The app thread drew with the guess, and main reports state only after a change.
            window.update_size();
            Some((window, view))
        });
        let native = Arc::new(Native {
            opener: Mutex::new(Some(opener)),
            handles: OnceLock::new(),
        });
        let opening = native.clone();
        native_post(Box::new(move || {
            opening.get();
        }));
        Ok(Self {
            control: WindowControl { native, snapshot },
            callbacks,
            renderer,
            pacer,
            idle: 0,
            quiet: None,
            presents: Arc::default(),
            controls_sent: Vec::new(),
            controls: Arc::default(),
            transparent,
        })
    }

    pub(crate) fn control(&self) -> &WindowControl {
        &self.control
    }

    pub fn atlas(&self) -> &Rc<Atlas> {
        self.renderer.atlas()
    }

    pub fn scale_factor(&self) -> f32 {
        self.control.snapshot().scale
    }

    pub fn content_size(&self) -> Size<Pixels> {
        self.control.snapshot().state.content.size
    }

    pub fn state(&self) -> WindowState {
        self.control.snapshot().state
    }

    pub fn request_frame(&mut self) {
        self.idle = 0;
        let mut pacer = lock(&self.pacer);
        pacer.wants = true;
        pacer.sync();
    }

    pub fn force_frame(&self) {
        let callbacks = self.callbacks;
        post(callbacks.thread, Box::new(move || step(callbacks)));
    }

    pub(crate) fn rest(&mut self) {
        self.idle += 1;
        if self.idle < LINGER {
            return;
        }
        {
            let mut pacer = lock(&self.pacer);
            pacer.wants = false;
            pacer.sync();
        }
        self.renderer.trim();
        let callbacks = self.callbacks;
        let release = crate::app::timer(QUIET, move |_| callbacks.release());
        self.quiet = Some(release);
    }

    pub(crate) fn release(&mut self) {
        self.renderer.release();
    }

    pub(crate) fn set_drawable_size(&mut self, size: Size<Pixels>, scale: f32) {
        let pixels = size.to_device(scale);
        let pixels = CGSize::new(pixels.width.into(), pixels.height.into());
        self.renderer.set_drawable_size(pixels);
    }

    pub fn draw(&mut self, scene: &Scene, capture: bool) -> Option<Result<([u32; 2], Vec<u8>)>> {
        self.control.native.wait();
        self.quiet = None;
        let renderer = &mut self.renderer;
        let stale = lock(&self.presents.value).take();
        if let Some(stale) = stale {
            renderer.discard(stale);
        }
        let frame = renderer.draw(scene, capture);
        let native = self.control.native.clone();
        let glass = renderer.take_glass_update(move |surfaces| {
            native.with(|window| window.view().update_glass(surfaces));
        });
        if let Some(present) = glass {
            // Dropping a present on main shows it there.
            self.presents
                .send(present, |latest| drop(lock(&latest.value).take()));
        }
        frame
    }

    pub fn set_controls(
        &mut self,
        list: Vec<crate::NativeControl>,
        sink: Option<crate::ControlSink>,
    ) {
        if self.controls_sent == list {
            return;
        }
        self.controls_sent.clone_from(&list);
        let native = self.control.native.clone();
        self.controls.send((list, sink), move |latest| {
            show_controls(native, latest.clone())
        });
    }

    pub fn change(&mut self, change: WindowChange) {
        if let WindowChange::Vibrancy(material, _) = change {
            let clear = material.is_some() || self.transparent;
            self.renderer.update_transparency(clear);
        }
        self.control.change(change);
    }
}

fn show_controls(native: Arc<Native>, latest: Arc<Latest<Controls>>) {
    native.with(|window| {
        // Code up the stack can be using the controls: try again in the next pass.
        if !window.view().update_controls(|| lock(&latest.value).take()) {
            let native = native.clone();
            latest.post(move |latest| show_controls(native, latest.clone()));
        }
    });
}

impl Drop for Window {
    fn drop(&mut self) {
        {
            let mut pacer = lock(&self.pacer);
            pacer.closed = true;
            pacer.sync();
        }
        let native = self.control.native.clone();
        native_post(Box::new(move || {
            if lock(&native.opener).take().is_some() {
                let _ = native.handles.set(None);
            }
            let Some(handles) = native.opened() else {
                return;
            };
            // SAFETY: `native` keeps the window retained until the close below ran.
            let window = unsafe { handles.window.as_ref() };
            window.ivars().closed.set(true);
            window.setDelegate(None);
            // End a sheet now: it can block its parent, the next sheet and a quit already on its way.
            nswindow::detach(window);
            post_local(move || {
                if let Some(handles) = native.opened() {
                    // SAFETY: as above.
                    unsafe { handles.window.as_ref() }.close();
                }
            });
        }));
    }
}

impl WindowControl {
    fn snapshot(&self) -> Snapshot {
        *lock(&self.snapshot)
    }

    fn later(&self, job: impl FnOnce(&AppsWindow) + Send + 'static) {
        let native = self.native.clone();
        native_post(Box::new(move || native.with(job)));
    }

    pub fn is_active(&self) -> bool {
        self.snapshot().active
    }

    // On main, open the window first, so the state is real and not the guess.
    pub fn state(&self) -> Option<WindowState> {
        if MainThreadMarker::new().is_some() {
            self.native.get();
        }
        Some(self.snapshot().state)
    }

    pub fn change(&self, change: WindowChange) {
        let change = Sendable::new(change);
        self.later(move |window| window.apply(change.take()));
    }

    pub fn set_title(&self, title: &str) {
        let title = title.to_owned();
        self.later(move |window| window.set_title(&title));
    }

    pub fn set_button_visibility(&self, visible: bool) {
        self.later(move |window| window.show_buttons(visible));
    }

    pub fn set_button_position(&self, position: Option<Point<Pixels>>) {
        self.later(move |window| {
            window.ivars().buttons_at.set(position);
            match position {
                Some(_) => window.place_buttons(),
                None => window.reset_buttons(),
            }
        });
    }

    pub fn minimize(&self) {
        self.later(|window| window.miniaturize(None));
    }

    pub fn zoom(&self) {
        self.later(|window| window.zoom(None));
    }

    pub fn toggle_fullscreen(&self) {
        self.later(|window| window.toggleFullScreen(None));
    }

    pub fn activate(&self) {
        self.change(WindowChange::Show(true));
    }

    pub fn resize(&self, size: Size<Pixels>) {
        self.later(move |window| window.setContentSize(cg_size(size)));
    }

    pub fn start_window_move(&self) {
        self.later(|window| window.view().drag_window());
    }

    pub fn titlebar_double_click(&self) {
        self.later(|window| window.double_click_title());
    }

    pub fn raw_window_handle(&self) -> Result<rwh::RawWindowHandle> {
        self.native.wait();
        let view = self
            .native
            .opened()
            .context("The window did not open")?
            .view;
        Ok(rwh::AppKitWindowHandle::new(view.cast()).into())
    }
}
