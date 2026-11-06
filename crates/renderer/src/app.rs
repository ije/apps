use crate::{
    Bounds, FileDropEvent, Pixels, PlatformInput, Point, ScrollDelta, Size, TextSystem,
    Utf16Selection, View, Window, WindowId, WindowOptions, platform,
};
use anyhow::Result;
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, VecDeque},
    ops::Range,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

type Callback = Box<dyn FnOnce(&mut App)>;
type WakeCallback = Rc<RefCell<dyn FnMut(&mut App)>>;

/// The app state, owned by the thread that draws: its own thread on macOS, the main thread on
/// other platforms.
pub struct App {
    pub(crate) platform: Rc<platform::Platform>,
    text_system: Rc<TextSystem>,
    windows: BTreeMap<WindowId, Option<Box<Window>>>,
    deferred: Vec<Callback>,
    pub(crate) propagate_event: bool,
    on_wake: Option<WakeCallback>,
    waker: Arc<AtomicBool>,
}

struct AppCell {
    app: RefCell<App>,
    /// Work that arrived while the app was busy, like events of a nested native loop.
    queue: RefCell<VecDeque<Callback>>,
}

thread_local! {
    static APP: RefCell<Option<Rc<AppCell>>> = const { RefCell::new(None) };
}

fn cell() -> Option<Rc<AppCell>> {
    APP.with_borrow(Clone::clone)
}

/// Runs `f` with the app, or queues it while the app is busy.
pub(crate) fn update(f: impl FnOnce(&mut App) + 'static) {
    let Some(cell) = cell() else {
        return;
    };
    match cell.app.try_borrow_mut() {
        Ok(mut app) => {
            f(&mut app);
            app.flush();
        }
        Err(_) => {
            cell.queue.borrow_mut().push_back(Box::new(f));
            return;
        }
    }
    drain(&cell);
}

/// Runs `f` with the app, or returns `None` while the app is busy.
pub(crate) fn try_update<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    let cell = cell()?;
    let result = {
        let mut app = cell.app.try_borrow_mut().ok()?;
        let result = f(&mut app);
        app.flush();
        result
    };
    drain(&cell);
    Some(result)
}

fn drain(cell: &AppCell) {
    loop {
        let Some(next) = cell.queue.borrow_mut().pop_front() else {
            return;
        };
        let Ok(mut app) = cell.app.try_borrow_mut() else {
            cell.queue.borrow_mut().push_front(next);
            return;
        };
        next(&mut app);
        app.flush();
    }
}

fn install(platform: Rc<platform::Platform>) {
    let app = App {
        platform,
        text_system: Rc::new(TextSystem::new()),
        windows: BTreeMap::new(),
        deferred: Vec::new(),
        propagate_event: true,
        on_wake: None,
        waker: Arc::default(),
    };
    APP.with_borrow_mut(|slot| {
        *slot = Some(Rc::new(AppCell {
            app: RefCell::new(app),
            queue: RefCell::default(),
        }))
    });
}

/// Whether each [`spawn`] gets its own thread, so its windows don't share a thread with others.
pub const THREAD_PER_APP: bool = cfg!(target_os = "macos");

/// Starts an `App` with `init`: on its own thread on macOS, with one thread and event loop until
/// [`App::end_thread`]. Other platforms have one `App`, on the main thread.
pub fn spawn(init: impl FnOnce(&mut App) + Send + 'static) {
    #[cfg(target_os = "macos")]
    platform::spawn_render(
        move || {
            install(Rc::new(platform::Platform::new()));
            update(init);
        },
        // WARNING: windows and their state can hold things that need other thread locals.
        || {
            APP.with_borrow_mut(|slot| *slot = None);
        },
    );
    #[cfg(not(target_os = "macos"))]
    update(init);
}

/// Starts the platform loop, and calls `native` on the main thread once the app has launched.
/// Returns after quitting.
pub fn run(native: impl FnOnce() + 'static) {
    let platform = Rc::new(platform::Platform::new());
    #[cfg(target_os = "macos")]
    platform.run(Box::new(native));
    #[cfg(not(target_os = "macos"))]
    {
        install(platform.clone());
        platform.run(Box::new(native));
        // WARNING: windows hold native views whose callbacks reach for the app, so they close first.
        let windows = cell().map(|cell| std::mem::take(&mut cell.app.borrow_mut().windows));
        drop(windows);
        APP.with_borrow_mut(|slot| *slot = None);
    }
}

/// Wakes the app's thread from any thread, to run the app's [`App::on_wake`] callback.
#[derive(Clone)]
pub struct Waker {
    pending: Arc<AtomicBool>,
    #[cfg(target_os = "macos")]
    thread: platform::AppThread,
}

impl Waker {
    pub fn wake(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            let pending = self.pending.clone();
            let wake = Box::new(move || {
                pending.store(false, Ordering::Release);
                update(|app| {
                    if let Some(callback) = app.on_wake.clone() {
                        (callback.borrow_mut())(app);
                    }
                });
            });
            #[cfg(target_os = "macos")]
            platform::post(self.thread, wake);
            #[cfg(not(target_os = "macos"))]
            platform::post(wake);
        }
    }
}

pub(crate) fn timer(delay: Duration, f: impl FnOnce(&mut App) + 'static) -> Timer {
    let cancelled = Rc::new(Cell::new(false));
    let flag = cancelled.clone();
    platform::run_after(delay, move || {
        if !flag.get() {
            update(f);
        }
    });
    Timer {
        cancelled,
        detached: false,
    }
}

/// A pending [`App::timer`]. Dropping it cancels the timer unless it was detached.
pub struct Timer {
    cancelled: Rc<Cell<bool>>,
    detached: bool,
}

impl Timer {
    pub fn detach(mut self) {
        self.detached = true;
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        if !self.detached {
            self.cancelled.set(true);
        }
    }
}

impl App {
    /// Keeps the current mouse or key event from the listeners after this one.
    pub fn stop_propagation(&mut self) {
        self.propagate_event = false;
    }

    /// Runs `f` after the current event, with the app.
    pub fn defer(&mut self, f: impl FnOnce(&mut App) + 'static) {
        self.deferred.push(Box::new(f));
    }

    /// Runs `f` after `delay`, on the app's thread.
    pub fn timer(&mut self, delay: Duration, f: impl FnOnce(&mut App) + 'static) -> Timer {
        timer(delay, f)
    }

    pub fn waker(&self) -> Waker {
        Waker {
            pending: self.waker.clone(),
            #[cfg(target_os = "macos")]
            thread: platform::app_thread(),
        }
    }

    /// Ends this app's thread after the current work, once its window has closed.
    pub fn end_thread(&self) {
        #[cfg(target_os = "macos")]
        platform::end_thread();
    }

    /// Runs on the app's thread after any [`Waker::wake`].
    pub fn on_wake(&mut self, f: impl FnMut(&mut App) + 'static) {
        self.on_wake = Some(Rc::new(RefCell::new(f)));
    }

    pub fn quit(&mut self) {
        self.platform.quit();
    }

    pub fn activate(&self, ignoring_other_apps: bool) {
        self.platform.activate(ignoring_other_apps);
    }

    pub fn read_from_clipboard(&self) -> Option<String> {
        self.platform.read_clipboard()
    }

    pub fn write_to_clipboard(&self, text: String) {
        self.platform.write_clipboard(&text);
    }

    /// The visible area of the main display, in screen points.
    pub fn primary_display_bounds(&self) -> Option<Bounds<Pixels>> {
        self.platform.primary_display_bounds()
    }

    /// The bounds of `size` centered on the main display.
    pub fn centered(&self, size: Size<Pixels>) -> Bounds<Pixels> {
        let display = self.primary_display_bounds().unwrap_or_default();
        Bounds::new(
            Point::new(
                display.origin.x + (display.size.width - size.width) / 2.,
                display.origin.y + (display.size.height - size.height) / 2.,
            ),
            size,
        )
    }

    pub fn open_window(
        &mut self,
        options: WindowOptions,
        build: impl FnOnce(&mut Window, &mut App) -> Box<dyn View>,
    ) -> Result<WindowId> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = WindowId(NEXT.fetch_add(1, Ordering::Relaxed));
        let platform = self.platform.open_window(
            &options,
            Callbacks {
                id,
                #[cfg(target_os = "macos")]
                thread: platform::app_thread(),
            },
        )?;
        let mut window = Box::new(Window::new(
            id,
            platform,
            Rc::new(self.text_system.for_window()),
        ));
        let view = build(&mut window, self);
        window.view = Some(view);
        window.refresh();
        self.windows.insert(id, Some(window));
        Ok(id)
    }

    pub fn window_ids(&self) -> Vec<WindowId> {
        self.windows.keys().copied().collect()
    }

    /// Runs `f` with the window, or returns `None` when it is closed or busy.
    pub fn update_window<R>(
        &mut self,
        id: WindowId,
        f: impl FnOnce(&mut Window, &mut App) -> R,
    ) -> Option<R> {
        let mut window = self.windows.get_mut(&id)?.take()?;
        let result = f(&mut window, self);
        if let Some(slot) = self.windows.get_mut(&id) {
            *slot = Some(window);
        }
        Some(result)
    }

    /// Redraws the window on the next frame.
    pub fn refresh_window(&mut self, id: WindowId) {
        self.update_window(id, |window, _| window.refresh());
    }

    fn flush(&mut self) {
        for _ in 0..1024 {
            let deferred = std::mem::take(&mut self.deferred);
            self.windows
                .retain(|_, window| !window.as_ref().is_some_and(|window| window.removed));
            if deferred.is_empty() {
                return;
            }
            for callback in deferred {
                callback(self);
            }
        }
    }
}

/// The platform's way into one window.
#[derive(Clone, Copy)]
pub(crate) struct Callbacks {
    pub id: WindowId,
    /// The thread of the app that owns the window.
    #[cfg(target_os = "macos")]
    pub thread: platform::AppThread,
}

/// Sends platform input, whose positions are in window points, so the page zoom maps them.
pub(crate) fn dispatch(window: &mut Window, app: &mut App, mut input: PlatformInput) -> bool {
    let zoom = window.zoom;
    if zoom != 1. {
        match &mut input {
            PlatformInput::MouseDown(event) => event.position = event.position / zoom,
            PlatformInput::MouseUp(event) => event.position = event.position / zoom,
            PlatformInput::MouseMove(event) => event.position = event.position / zoom,
            PlatformInput::MouseExited(event) => event.position = event.position / zoom,
            PlatformInput::ScrollWheel(event) => {
                event.position = event.position / zoom;
                if let ScrollDelta::Pixels(delta) = &mut event.delta {
                    *delta = *delta / zoom;
                }
            }
            PlatformInput::FileDrop(
                FileDropEvent::Entered { position, .. }
                | FileDropEvent::Pending { position }
                | FileDropEvent::Submit { position },
            ) => *position = *position / zoom,
            _ => {}
        }
    }
    window.dispatch_event(input, app)
}

impl Callbacks {
    /// Runs `f` with the window and returns its result. On macOS the main thread asks the render
    /// thread and gets `None` when that takes too long.
    pub(crate) fn with<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Window, &mut App) -> R + Send + 'static,
    ) -> Option<R> {
        let id = self.id;
        let run = move || try_update(|app| app.update_window(id, f)).flatten();
        #[cfg(target_os = "macos")]
        if cell().is_none() {
            return platform::ask(self.thread, run).flatten();
        }
        run()
    }

    pub(crate) fn queue(&self, f: impl FnOnce(&mut Window, &mut App) + Send + 'static) {
        let id = self.id;
        let run = move || {
            update(move |app| {
                app.update_window(id, f);
            })
        };
        #[cfg(target_os = "macos")]
        if cell().is_none() {
            return platform::post(self.thread, Box::new(run));
        }
        run();
    }

    /// Sends input and returns whether the view handled it.
    pub fn input(&self, input: PlatformInput) -> bool {
        self.with(move |window, app| dispatch(window, app, input))
            .unwrap_or(false)
    }

    /// Sends input whose result nothing needs, so a busy view holds up nothing.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn input_later(&self, input: PlatformInput) {
        self.queue(move |window, app| {
            dispatch(window, app, input);
        });
    }

    /// The display is ready for a frame. Returns whether more frames are wanted, or `None` while
    /// the app is busy.
    pub fn frame(&self) -> Option<bool> {
        self.with(|window, app| {
            for callback in window.take_next_frame_callbacks() {
                callback(window, app);
            }
            if window.dirty || !window.captures.is_empty() {
                window.draw(app);
            }
        })?;
        // WARNING: deferred work can request another frame after drawing, before the display link stops.
        let cell = cell()?;
        let app = cell.app.borrow();
        Some(
            app.windows
                .get(&self.id)
                .and_then(Option::as_deref)
                .is_some_and(Window::wants_frame),
        )
    }

    pub fn resized(&self, size: Size<Pixels>, scale_factor: f32) {
        native::window_event(self.id, native::WindowEvent::Changed);
        self.queue(move |window, app| {
            window.mark_resized(size, scale_factor);
            window.with_view(app, |view, window, app| view.window_changed(window, app));
        });
    }

    /// The window moved, was activated, minimized, or changed in another way the view may track.
    pub fn changed(&self) {
        native::window_event(self.id, native::WindowEvent::Changed);
        self.queue(|window, app| {
            window.refresh();
            window.with_view(app, |view, window, app| view.window_changed(window, app));
        });
    }

    /// The user asked to close the window. The main side decides, so the native window stays for now.
    pub fn should_close(&self) -> bool {
        native::window_event(self.id, native::WindowEvent::CloseRequested);
        false
    }

    pub fn appearance_changed(&self) {
        self.queue(|window, _| window.refresh());
    }

    /// The window stopped drawing, so its renderer can free what the last frame didn't use.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn trim(&self) {
        #[cfg(target_os = "macos")]
        self.queue(|window, _| window.platform.trim());
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn in_drag_area(&self) -> bool {
        self.with(|window, _| window.in_drag_area())
            .unwrap_or(false)
    }

    pub fn accepts_text_input(&self) -> bool {
        self.with(|window, app| {
            window.with_view(app, |view, window, app| {
                view.accepts_text_input(window, app)
            })
        })
        .flatten()
        .unwrap_or(false)
    }

    pub fn selected_text_range(&self) -> Option<Utf16Selection> {
        self.with(|window, app| {
            window.with_view(app, |view, window, app| {
                view.selected_text_range(window, app)
            })
        })
        .flatten()
        .flatten()
    }

    #[cfg_attr(target_os = "windows", allow(dead_code))]
    pub fn marked_text_range(&self) -> Option<Range<usize>> {
        self.with(|window, app| {
            window.with_view(app, |view, window, app| view.marked_text_range(window, app))
        })
        .flatten()
        .flatten()
    }

    /// The text of `range`, and the range it really covers when that differs.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn text_for_range(&self, range: Range<usize>) -> Option<(String, Option<Range<usize>>)> {
        self.with(move |window, app| {
            let mut adjusted = None;
            window
                .with_view(app, |view, window, app| {
                    view.text_for_range(range, &mut adjusted, window, app)
                })
                .flatten()
                .map(|text| (text, adjusted))
        })
        .flatten()
    }

    pub fn replace_text_in_range(&self, range: Option<Range<usize>>, text: &str) {
        let text = text.to_owned();
        self.queue(move |window, app| {
            window.with_view(app, |view, window, app| {
                view.replace_text_in_range(range, &text, window, app)
            });
        });
    }

    pub fn replace_and_mark_text_in_range(
        &self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
    ) {
        let text = text.to_owned();
        self.queue(move |window, app| {
            window.with_view(app, |view, window, app| {
                view.replace_and_mark_text_in_range(range, &text, selected, window, app)
            });
        });
    }

    #[cfg_attr(target_os = "windows", allow(dead_code))]
    pub fn unmark_text(&self) {
        self.queue(|window, app| {
            window.with_view(app, |view, window, app| view.unmark_text(window, app));
        });
    }

    pub fn bounds_for_range(&self, range: Range<usize>) -> Option<Bounds<Pixels>> {
        self.with(move |window, app| {
            let zoom = window.zoom;
            window
                .with_view(app, |view, window, app| {
                    view.bounds_for_range(range, window, app)
                })
                .flatten()
                .map(|bounds| bounds * zoom)
        })
        .flatten()
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn character_index_for_point(&self, point: Point<Pixels>) -> Option<usize> {
        self.with(move |window, app| {
            let point = point / window.zoom;
            window.with_view(app, |view, window, app| {
                view.character_index_for_point(point, window, app)
            })
        })
        .flatten()
        .flatten()
    }
}

/// The app as the OS sees it. These run on the main thread, unless they say otherwise.
pub mod native {
    use super::*;

    /// What a native window tells the main side, without the thread that draws it.
    #[derive(Clone, Copy)]
    pub enum WindowEvent {
        /// It moved, resized, was activated, minimized or otherwise changed.
        Changed,
        /// The user asked to close it.
        CloseRequested,
    }

    #[derive(Default)]
    struct Handlers {
        window: Option<Box<dyn FnMut(WindowId, WindowEvent)>>,
        reopen: Option<Box<dyn FnMut(bool)>>,
        should_quit: Option<Box<dyn FnMut() -> bool>>,
        open_urls: Option<Box<dyn FnMut(Vec<String>)>>,
        pending_urls: Vec<String>,
        quit: Option<Box<dyn FnOnce()>>,
    }

    thread_local! { static HANDLERS: RefCell<Handlers> = RefCell::default(); }

    pub fn on_window_event(f: impl FnMut(WindowId, WindowEvent) + 'static) {
        HANDLERS.with_borrow_mut(|handlers| handlers.window = Some(Box::new(f)));
    }

    pub(crate) fn window_event(id: WindowId, event: WindowEvent) {
        if let Some(mut callback) = HANDLERS.with_borrow_mut(|handlers| handlers.window.take()) {
            callback(id, event);
            HANDLERS.with_borrow_mut(|handlers| {
                handlers.window.get_or_insert(callback);
            });
        }
    }

    /// The Dock icon was clicked; `true` when windows are visible.
    pub fn on_reopen(f: impl FnMut(bool) + 'static) {
        HANDLERS.with_borrow_mut(|handlers| handlers.reopen = Some(Box::new(f)));
    }

    /// Asked before quitting; `false` cancels.
    pub fn on_should_quit(f: impl FnMut() -> bool + 'static) {
        HANDLERS.with_borrow_mut(|handlers| handlers.should_quit = Some(Box::new(f)));
    }

    /// URLs or files the app was asked to open.
    pub fn on_open_urls(mut f: impl FnMut(Vec<String>) + 'static) {
        let urls = HANDLERS.with_borrow_mut(|handlers| std::mem::take(&mut handlers.pending_urls));
        if !urls.is_empty() {
            f(urls);
        }
        HANDLERS.with_borrow_mut(|handlers| handlers.open_urls = Some(Box::new(f)));
    }

    pub fn on_quit(f: impl FnOnce() + 'static) {
        HANDLERS.with_borrow_mut(|handlers| handlers.quit = Some(Box::new(f)));
    }

    /// Runs `f` on the main thread, from any thread.
    pub fn post(f: Box<dyn FnOnce() + Send>) {
        #[cfg(target_os = "macos")]
        platform::native_post(f);
        #[cfg(not(target_os = "macos"))]
        platform::post(f);
    }

    /// Runs `f` on the main thread after `delay`.
    pub fn after(delay: Duration, f: impl FnOnce() + 'static) {
        #[cfg(target_os = "macos")]
        platform::native_after(delay, f);
        #[cfg(not(target_os = "macos"))]
        platform::run_after(delay, f);
    }

    /// Runs `f` on the main thread and waits for it, from any thread.
    pub fn sync<R: Send>(f: impl FnOnce() -> R + Send) -> R {
        #[cfg(target_os = "macos")]
        return platform::native_sync(f);
        #[cfg(not(target_os = "macos"))]
        f()
    }

    /// Stops the thread that draws, before the process exits without AppKit.
    pub fn before_exit() {
        #[cfg(target_os = "macos")]
        platform::park_render();
    }

    /// Brings the app to the front, from any thread.
    pub fn activate(ignoring_other_apps: bool) {
        #[cfg(target_os = "macos")]
        platform::Platform::new().activate(ignoring_other_apps);
        #[cfg(not(target_os = "macos"))]
        update(move |app| app.activate(ignoring_other_apps));
    }

    /// Ends the app, from any thread.
    pub fn quit() {
        #[cfg(target_os = "macos")]
        platform::Platform::new().quit();
        #[cfg(not(target_os = "macos"))]
        update(|app| app.quit());
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn reopen(visible: bool) {
        if let Some(mut callback) = HANDLERS.with_borrow_mut(|handlers| handlers.reopen.take()) {
            callback(visible);
            HANDLERS.with_borrow_mut(|handlers| {
                handlers.reopen.get_or_insert(callback);
            });
        }
    }

    /// Returns `false` to cancel quitting.
    pub(crate) fn should_quit() -> bool {
        match HANDLERS.with_borrow_mut(|handlers| handlers.should_quit.take()) {
            Some(mut callback) => {
                let quit = callback();
                HANDLERS.with_borrow_mut(|handlers| {
                    handlers.should_quit.get_or_insert(callback);
                });
                quit
            }
            None => true,
        }
    }

    pub(crate) fn quit_handler() {
        if let Some(callback) = HANDLERS.with_borrow_mut(|handlers| handlers.quit.take()) {
            callback();
        }
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn open_urls(urls: Vec<String>) {
        match HANDLERS.with_borrow_mut(|handlers| handlers.open_urls.take()) {
            Some(mut callback) => {
                callback(urls);
                HANDLERS.with_borrow_mut(|handlers| {
                    handlers.open_urls.get_or_insert(callback);
                });
            }
            None => HANDLERS.with_borrow_mut(|handlers| handlers.pending_urls.extend(urls)),
        }
    }
}

pub(crate) mod events {
    pub(crate) use super::native::should_quit;
    #[cfg_attr(not(target_os = "macos"), allow(unused_imports))]
    pub(crate) use super::native::{open_urls, reopen};
    pub fn quit() {
        super::native::quit_handler();
    }
}
