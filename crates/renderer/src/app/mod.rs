pub(crate) mod window;

use crate::{
    Event, Pixels, Point, Rect, Size, TextSystem, Utf16Selection, View, Window, WindowId,
    WindowOptions, platform,
    platform::{Callbacks, Timer, WindowEvents, native},
};
use anyhow::Result;
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    ops::Range,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

type Job = Box<dyn FnOnce(&mut App)>;
type Wake = Rc<RefCell<dyn FnMut(&mut App)>>;

pub struct App {
    pub(crate) platform: Rc<platform::Platform>,
    text_system: Rc<TextSystem>,
    // A window is `None` while `update_window` lends it out.
    windows: BTreeMap<WindowId, Option<Box<Window>>>,
    deferred: Vec<Job>,
    pub(crate) event_stopped: bool,
    on_wake: Option<Wake>,
    wake_pending: Arc<AtomicBool>,
}

struct Home {
    app: RefCell<App>,
    backlog: RefCell<VecDeque<Job>>,
}

struct Slot(RefCell<Option<Rc<Home>>>);

// Runs only when the process exits inside the loop, like `process::exit()`. GPU objects can't drop then,
// because their destructors use thread locals that are already gone, so the app is leaked.
impl Drop for Slot {
    fn drop(&mut self) {
        std::mem::forget(self.0.get_mut().take());
    }
}

thread_local! {
    static HOME: Slot = const { Slot(RefCell::new(None)) };
}

fn set_home(home: Option<Rc<Home>>) {
    let old = HOME.with(|slot| slot.0.replace(home));
    drop(old);
}

fn home() -> Option<Rc<Home>> {
    HOME.with(|slot| slot.0.borrow().clone())
}

impl Home {
    fn enter<R, F: FnOnce(&mut App) -> R>(&self, job: F) -> Result<R, F> {
        let Ok(mut app) = self.app.try_borrow_mut() else {
            return Err(job);
        };
        let result = job(&mut app);
        app.flush();
        drop(app);
        loop {
            let job = self.backlog.borrow_mut().pop_front();
            let Some(job) = job else {
                break;
            };
            let Ok(mut app) = self.app.try_borrow_mut() else {
                self.backlog.borrow_mut().push_front(job);
                break;
            };
            job(&mut app);
            app.flush();
        }
        Ok(result)
    }
}

pub(crate) fn update(job: impl FnOnce(&mut App) + 'static) {
    if let Some(home) = home()
        && let Err(job) = home.enter(job)
    {
        home.backlog.borrow_mut().push_back(Box::new(job));
    }
}

pub(crate) fn try_update<R>(job: impl FnOnce(&mut App) -> R) -> Option<R> {
    home()?.enter(job).ok()
}

fn install(platform: Rc<platform::Platform>) {
    let app = App {
        platform,
        text_system: Rc::new(TextSystem::new()),
        windows: BTreeMap::new(),
        deferred: Vec::new(),
        event_stopped: false,
        on_wake: None,
        wake_pending: Arc::default(),
    };
    platform::set_events(&Events);
    let home = Home {
        app: RefCell::new(app),
        backlog: RefCell::default(),
    };
    set_home(Some(Rc::new(home)));
}

pub const THREAD_PER_APP: bool = cfg!(target_os = "macos");

pub fn spawn(init: impl FnOnce(&mut App) + Send + 'static) {
    platform::spawn_render(
        move || {
            if home().is_none() {
                install(Rc::new(platform::Platform::new()));
            }
            update(init);
        },
        // Drop windows before thread-local teardown: their destructors need other thread locals.
        || set_home(None),
    );
}

pub fn run(native: impl FnOnce() + 'static) {
    let platform = Rc::new(platform::Platform::new());
    if !THREAD_PER_APP {
        install(platform.clone());
    }
    platform.run(Box::new(native));
    // Close native views before the app: their callbacks can still access it.
    let windows = home().map(|home| std::mem::take(&mut home.app.borrow_mut().windows));
    drop(windows);
    set_home(None);
}

/// Wakes that arrive before the app runs merge into one.
#[derive(Clone)]
pub struct Waker {
    pending: Arc<AtomicBool>,
    thread: platform::AppThread,
}

impl Waker {
    pub fn wake(&self) {
        if self.pending.swap(true, Ordering::AcqRel) {
            return;
        }
        let pending = self.pending.clone();
        let wake = Box::new(move || {
            pending.store(false, Ordering::Release);
            update(|app| {
                if let Some(on_wake) = app.on_wake.clone() {
                    (on_wake.borrow_mut())(app);
                }
            });
        });
        platform::post(self.thread, wake);
    }
}

pub(crate) fn timer(delay: Duration, job: impl FnOnce(&mut App) + 'static) -> Timer {
    platform::timer(delay, move || update(job))
}

impl App {
    pub fn stop_event(&mut self) {
        self.event_stopped = true;
    }

    pub fn defer(&mut self, job: impl FnOnce(&mut App) + 'static) {
        self.deferred.push(Box::new(job));
    }

    pub fn timer(&mut self, delay: Duration, job: impl FnOnce(&mut App) + 'static) -> Timer {
        timer(delay, job)
    }

    pub fn waker(&self) -> Waker {
        Waker {
            pending: self.wake_pending.clone(),
            thread: platform::app_thread(),
        }
    }

    pub fn end_thread(&self) {
        platform::end_thread();
    }

    pub fn on_wake(&mut self, handler: impl FnMut(&mut App) + 'static) {
        self.on_wake = Some(Rc::new(RefCell::new(handler)));
    }

    pub fn quit(&mut self) {
        platform::Platform::quit();
    }

    pub fn activate(&self, force: bool) {
        platform::Platform::activate(force);
    }

    pub fn read_clipboard(&self) -> Option<String> {
        self.platform.read_clipboard()
    }

    pub fn write_clipboard(&self, text: String) {
        self.platform.write_clipboard(&text);
    }

    pub fn centered(&self, size: Size<Pixels>) -> Rect {
        let display = self.platform.primary_display_bounds().unwrap_or_default();
        let room = display.size - size;
        Rect::new(
            display.origin + Point::new(room.width / 2., room.height / 2.),
            size,
        )
    }

    pub fn open_window(
        &mut self,
        options: WindowOptions,
        build: impl FnOnce(&mut Window, &mut App) -> Box<dyn View>,
    ) -> Result<WindowId> {
        static LAST: AtomicU64 = AtomicU64::new(0);
        let id = WindowId(LAST.fetch_add(1, Ordering::Relaxed) + 1);
        let callbacks = Callbacks {
            id,
            thread: platform::app_thread(),
        };
        let native = self.platform.open_window(&options, callbacks)?;
        let text_system = Rc::new(self.text_system.for_window());
        let mut window = Box::new(Window::new(id, native, text_system));
        window.view = Some(build(&mut window, self));
        window.refresh();
        self.windows.insert(id, Some(window));
        Ok(id)
    }

    pub fn window_ids(&self) -> Vec<WindowId> {
        self.windows.keys().copied().collect()
    }

    /// `None` when the window is closed or already lent.
    pub fn update_window<R>(
        &mut self,
        id: WindowId,
        lend: impl FnOnce(&mut Window, &mut App) -> R,
    ) -> Option<R> {
        let mut window = self.windows.get_mut(&id)?.take()?;
        let result = lend(&mut window, self);
        if let Some(slot) = self.windows.get_mut(&id) {
            *slot = Some(window);
        }
        Some(result)
    }

    pub fn refresh_window(&mut self, id: WindowId) {
        self.update_window(id, |window, _| window.refresh());
    }

    fn flush(&mut self) {
        for _ in 0..1024 {
            self.windows
                .retain(|_, window| window.as_ref().is_none_or(|window| !window.closed));
            let deferred = std::mem::take(&mut self.deferred);
            if deferred.is_empty() {
                return;
            }
            for job in deferred {
                job(self);
            }
        }
    }
}

// Runs on the app thread and waits for the answer, at most 100 ms from the main thread on macOS.
fn with<R: Send + 'static>(
    at: Callbacks,
    job: impl FnOnce(&mut Window, &mut App) -> R + Send + 'static,
) -> Option<R> {
    let id = at.id;
    let run = move || try_update(|app| app.update_window(id, job)).flatten();
    match home() {
        Some(_) => run(),
        None => platform::ask(at.thread, run).flatten(),
    }
}

fn queue(at: Callbacks, job: impl FnOnce(&mut Window, &mut App) + Send + 'static) {
    let id = at.id;
    let run = move || {
        update(move |app| {
            app.update_window(id, job);
        })
    };
    match home() {
        Some(_) => run(),
        None => platform::post(at.thread, Box::new(run)),
    }
}

// Frames stop when this returns `Some(false)`, so an idle window gets no calls.
fn frame(at: Callbacks) -> Option<bool> {
    with(at, |window, app| window.frame(app))?;
    let home = home()?;
    let app = home.app.borrow();
    let window = app.windows.get(&at.id)?.as_deref();
    Some(window.is_some_and(Window::wants_frame))
}

struct Events;

impl WindowEvents for Events {
    fn input(&self, at: Callbacks, event: Event) -> bool {
        with(at, move |window, app| {
            window.dispatch_native(event, app, true)
        })
        .unwrap_or(false)
    }

    #[cfg(target_os = "macos")]
    fn input_later(&self, at: Callbacks, event: Event) {
        queue(at, move |window, app| {
            window.dispatch_native(event, app, false);
        });
    }

    #[cfg(target_os = "macos")]
    fn input_motion(&self, at: Callbacks, motion: platform::Motion) {
        queue(at, move |window, app| {
            if let Some((input, sent)) = platform::lock(&motion).take() {
                window.dispatch_at(input, sent, app);
            }
        });
    }

    #[cfg(target_os = "macos")]
    fn hold_key(&self, at: Callbacks, key: Event, composing: bool) {
        queue(at, move |window, _| window.hold_key(key, composing));
    }

    #[cfg(target_os = "macos")]
    fn ime_took_key(&self, at: Callbacks) {
        queue(at, |window, app| {
            window.send_ime_key(app, false);
        });
    }

    #[cfg(not(target_os = "macos"))]
    fn frame(&self, at: Callbacks) -> Option<bool> {
        frame(at)
    }

    #[cfg(target_os = "macos")]
    fn tick(&self, at: Callbacks, time: Option<std::time::Instant>) {
        // A tick without a frame takes one frame out of a queue that is too long.
        if let Some(time) = time
            && with(at, move |window, _| window.start_tick(time)) != Some(true)
        {
            return;
        }
        if frame(at) == Some(false) {
            with(at, |window, _| window.rest());
        }
    }

    #[cfg(target_os = "macos")]
    fn present_live(&self, at: Callbacks, visible: bool) -> Option<platform::Present> {
        with(at, move |window, _| window.live_resize(visible));
        self.tick(at, None);
        with(at, |window, _| window.end_live_resize()).flatten()
    }

    fn resized(&self, at: Callbacks, size: Size<Pixels>, scale_factor: f32) {
        native::window_event(at.id, native::WindowEvent::Changed);
        queue(at, move |window, app| {
            window.mark_resized(size, scale_factor);
            window.with_view(app, |view, window, app| view.window_changed(window, app));
        });
    }

    fn changed(&self, at: Callbacks) {
        native::window_event(at.id, native::WindowEvent::Changed);
        queue(at, |window, app| {
            window.refresh();
            window.with_view(app, |view, window, app| view.window_changed(window, app));
        });
    }

    #[cfg(target_os = "macos")]
    fn will_change(&self, at: Callbacks, kind: &str, bounds: Rect) -> bool {
        native::will_change(at.id, kind, bounds)
    }

    // Main decides, and closes the window itself.
    fn should_close(&self, at: Callbacks) -> bool {
        native::window_event(at.id, native::WindowEvent::CloseRequested);
        false
    }

    fn appearance_changed(&self, at: Callbacks) {
        queue(at, |window, _| window.refresh());
    }

    fn release(&self, at: Callbacks) {
        queue(at, |window, _| window.release_gpu());
    }

    #[cfg(windows)]
    fn device_lost(&self) {
        update(|app| {
            for id in app.window_ids() {
                app.refresh_window(id);
            }
        })
    }

    #[cfg(windows)]
    fn in_drag_area(&self, at: Callbacks, point: Point<Pixels>) -> bool {
        with(at, move |window, _| {
            window.in_drag_area(point / window.zoom())
        })
        .unwrap_or(false)
    }

    fn accepts_text_input(&self, at: Callbacks) -> bool {
        with(at, |window, app| {
            window.with_view(app, |_, _, _| ()).is_some()
        })
        .unwrap_or(false)
    }

    fn selection(&self, at: Callbacks) -> Option<Utf16Selection> {
        with(at, |window, app| {
            window.with_view(app, |view, window, app| view.selection(window, app))
        })
        .flatten()
        .flatten()
    }

    fn composition(&self, at: Callbacks) -> Option<Range<usize>> {
        with(at, |window, app| {
            window.with_view(app, |view, window, app| view.composition(window, app))
        })
        .flatten()
        .flatten()
    }

    #[cfg(not(target_os = "macos"))]
    fn clear_composition(&self, at: Callbacks) {
        if let Some(composed) = self.composition(at) {
            self.insert(at, Some(composed), "");
        }
    }

    #[cfg(target_os = "macos")]
    fn text_in(
        &self,
        at: Callbacks,
        range: Range<usize>,
    ) -> Option<(String, Option<Range<usize>>)> {
        with(at, move |window, app| {
            let mut actual = None;
            let text = window.with_view(app, |view, window, app| {
                view.text_in(range, &mut actual, window, app)
            });
            Some((text.flatten()?, actual))
        })
        .flatten()
    }

    fn insert(&self, at: Callbacks, range: Option<Range<usize>>, text: &str) {
        let text = text.to_owned();
        queue(at, move |window, app| {
            if !window.send_ime_key(app, text.encode_utf16().count() > 1) {
                window.with_view(app, |view, window, app| {
                    view.insert(range, &text, window, app)
                });
            }
        });
    }

    fn compose(
        &self,
        at: Callbacks,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
    ) {
        let text = text.to_owned();
        queue(at, move |window, app| {
            window.send_ime_key(app, true);
            window.with_view(app, |view, window, app| {
                view.compose(range, &text, selected, window, app)
            });
        });
    }

    #[cfg(not(windows))]
    fn end_composition(&self, at: Callbacks) {
        queue(at, |window, app| {
            window.send_ime_key(app, false);
            window.with_view(app, |view, window, app| view.end_composition(window, app));
        });
    }

    fn rect_of(&self, at: Callbacks, range: Range<usize>) -> Option<Rect> {
        with(at, move |window, app| {
            let zoom = window.zoom();
            let rect = window.with_view(app, |view, window, app| view.rect_of(range, window, app));
            Some(rect.flatten()? * zoom)
        })
        .flatten()
    }

    #[cfg(target_os = "macos")]
    fn index_at(&self, at: Callbacks, point: Point<Pixels>) -> Option<usize> {
        with(at, move |window, app| {
            let point = point / window.zoom();
            window.with_view(app, |view, window, app| view.index_at(point, window, app))
        })
        .flatten()
        .flatten()
    }
}
