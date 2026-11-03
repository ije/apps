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
type ReopenCallback = Box<dyn FnMut(&mut App, bool)>;
type ShouldQuitCallback = Box<dyn FnMut(&mut App) -> bool>;
type OpenUrlsCallback = Box<dyn FnMut(&mut App, Vec<String>)>;

/// The app state, owned by the main thread.
pub struct App {
    pub(crate) platform: Rc<platform::Platform>,
    text_system: Rc<TextSystem>,
    windows: BTreeMap<WindowId, Option<Box<Window>>>,
    next_window: u64,
    deferred: Vec<Callback>,
    pub(crate) propagate_event: bool,
    on_wake: Option<WakeCallback>,
    pub(crate) on_reopen: Option<ReopenCallback>,
    pub(crate) on_should_quit: Option<ShouldQuitCallback>,
    pub(crate) on_open_urls: Option<OpenUrlsCallback>,
    pending_urls: Vec<String>,
    pub(crate) on_quit: Option<Callback>,
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

/// Starts the platform loop and calls `init` once the app has launched. Returns after quitting.
pub fn run(init: impl FnOnce(&mut App) + 'static) {
    let platform = Rc::new(platform::Platform::new());
    let app = App {
        platform: platform.clone(),
        text_system: Rc::new(TextSystem::new()),
        windows: BTreeMap::new(),
        next_window: 1,
        deferred: Vec::new(),
        propagate_event: true,
        on_wake: None,
        on_reopen: None,
        on_should_quit: None,
        on_open_urls: None,
        pending_urls: Vec::new(),
        on_quit: None,
        waker: Arc::default(),
    };
    APP.with_borrow_mut(|slot| {
        *slot = Some(Rc::new(AppCell {
            app: RefCell::new(app),
            queue: RefCell::default(),
        }))
    });
    platform.run(Box::new(move || update(init)));
    // WARNING: windows hold native views whose callbacks reach for the app, so they close first.
    let windows = cell().map(|cell| std::mem::take(&mut cell.app.borrow_mut().windows));
    drop(windows);
    APP.with_borrow_mut(|slot| *slot = None);
}

/// Wakes the main thread from any thread, to run the app's [`App::on_wake`] callback.
#[derive(Clone)]
pub struct Waker {
    pending: Arc<AtomicBool>,
}

impl Waker {
    pub fn wake(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            let pending = self.pending.clone();
            platform::post(Box::new(move || {
                pending.store(false, Ordering::Release);
                update(|app| {
                    if let Some(callback) = app.on_wake.clone() {
                        (callback.borrow_mut())(app);
                    }
                });
            }));
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

    /// Runs `f` after `delay`, on the main thread.
    pub fn timer(&mut self, delay: Duration, f: impl FnOnce(&mut App) + 'static) -> Timer {
        timer(delay, f)
    }

    pub fn waker(&self) -> Waker {
        Waker {
            pending: self.waker.clone(),
        }
    }

    /// Runs on the main thread after any [`Waker::wake`].
    pub fn on_wake(&mut self, f: impl FnMut(&mut App) + 'static) {
        self.on_wake = Some(Rc::new(RefCell::new(f)));
    }

    /// The Dock icon was clicked; `true` when windows are visible.
    pub fn on_reopen(&mut self, f: impl FnMut(&mut App, bool) + 'static) {
        self.on_reopen = Some(Box::new(f));
    }

    /// Asked before quitting; `false` cancels.
    pub fn on_should_quit(&mut self, f: impl FnMut(&mut App) -> bool + 'static) {
        self.on_should_quit = Some(Box::new(f));
    }

    /// URLs or files the app was asked to open.
    pub fn on_open_urls(&mut self, f: impl FnMut(&mut App, Vec<String>) + 'static) {
        self.on_open_urls = Some(Box::new(f));
        let urls = std::mem::take(&mut self.pending_urls);
        if !urls.is_empty() {
            self.defer(move |app| {
                if let Some(mut callback) = app.on_open_urls.take() {
                    callback(app, urls);
                    app.on_open_urls.get_or_insert(callback);
                }
            });
        }
    }

    pub fn on_quit(&mut self, f: impl FnOnce(&mut App) + 'static) {
        self.on_quit = Some(Box::new(f));
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
        let id = WindowId(self.next_window);
        self.next_window += 1;
        let platform = self.platform.open_window(&options, Callbacks { id })?;
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
}

impl Callbacks {
    fn with<R>(&self, f: impl FnOnce(&mut Window, &mut App) -> R) -> Option<R> {
        let id = self.id;
        try_update(|app| app.update_window(id, f)).flatten()
    }

    fn queue(&self, f: impl FnOnce(&mut Window, &mut App) + 'static) {
        let id = self.id;
        update(move |app| {
            app.update_window(id, f);
        });
    }

    /// Sends platform input, whose positions are in window points, so the page zoom maps them.
    pub fn input(&self, mut input: PlatformInput) -> bool {
        self.with(|window, app| {
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
        })
        .unwrap_or(false)
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
            window.wants_frame()
        })
    }

    pub fn resized(&self, size: Size<Pixels>, scale_factor: f32) {
        self.queue(move |window, app| {
            window.mark_resized(size, scale_factor);
            window.with_view(app, |view, window, app| view.window_changed(window, app));
        });
    }

    /// The window moved, was activated, minimized, or changed in another way the view may track.
    pub fn changed(&self) {
        self.queue(|window, app| {
            window.refresh();
            window.with_view(app, |view, window, app| view.window_changed(window, app));
        });
    }

    /// The user asked to close the window; `false` keeps it open.
    pub fn should_close(&self) -> bool {
        self.with(|window, app| {
            window.with_view(app, |view, window, app| view.should_close(window, app))
        })
        .flatten()
        .unwrap_or(false)
    }

    pub fn appearance_changed(&self) {
        self.queue(|window, _| window.refresh());
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

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn text_for_range(
        &self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
    ) -> Option<String> {
        self.with(|window, app| {
            window.with_view(app, |view, window, app| {
                view.text_for_range(range, adjusted, window, app)
            })
        })
        .flatten()
        .flatten()
    }

    pub fn replace_text_in_range(&self, range: Option<Range<usize>>, text: &str) {
        self.with(|window, app| {
            window.with_view(app, |view, window, app| {
                view.replace_text_in_range(range, text, window, app)
            })
        });
    }

    pub fn replace_and_mark_text_in_range(
        &self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
    ) {
        self.with(|window, app| {
            window.with_view(app, |view, window, app| {
                view.replace_and_mark_text_in_range(range, text, selected, window, app)
            })
        });
    }

    #[cfg_attr(target_os = "windows", allow(dead_code))]
    pub fn unmark_text(&self) {
        self.with(|window, app| {
            window.with_view(app, |view, window, app| view.unmark_text(window, app))
        });
    }

    pub fn bounds_for_range(&self, range: Range<usize>) -> Option<Bounds<Pixels>> {
        self.with(|window, app| {
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
        self.with(|window, app| {
            let point = point / window.zoom;
            window.with_view(app, |view, window, app| {
                view.character_index_for_point(point, window, app)
            })
        })
        .flatten()
        .flatten()
    }
}

pub(crate) mod events {
    use super::*;

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn reopen(visible: bool) {
        update(move |app| {
            if let Some(mut callback) = app.on_reopen.take() {
                callback(app, visible);
                app.on_reopen.get_or_insert(callback);
            }
        });
    }

    /// Returns `false` to cancel quitting.
    pub fn should_quit() -> bool {
        try_update(|app| match app.on_should_quit.take() {
            Some(mut callback) => {
                let quit = callback(app);
                app.on_should_quit.get_or_insert(callback);
                quit
            }
            None => true,
        })
        .unwrap_or(true)
    }

    pub fn quit() {
        try_update(|app| {
            if let Some(callback) = app.on_quit.take() {
                callback(app);
            }
        });
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn open_urls(urls: Vec<String>) {
        update(move |app| match app.on_open_urls.take() {
            Some(mut callback) => {
                callback(app, urls);
                app.on_open_urls.get_or_insert(callback);
            }
            None => app.pending_urls.extend(urls),
        });
    }
}
