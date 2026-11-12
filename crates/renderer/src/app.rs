use crate::{
    Event, Pixels, Point, Rect, Size, TextSystem, Utf16Selection, View, Window, WindowId,
    WindowOptions, platform,
};
use anyhow::Result;
use std::{
    cell::{Cell, RefCell},
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

thread_local! {
    static HOME: RefCell<Option<Rc<Home>>> = const { RefCell::new(None) };
}

fn home() -> Option<Rc<Home>> {
    HOME.with_borrow(Option::clone)
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
    let home = Home {
        app: RefCell::new(app),
        backlog: RefCell::default(),
    };
    HOME.with_borrow_mut(|slot| *slot = Some(Rc::new(home)));
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
        || HOME.with_borrow_mut(|slot| *slot = None),
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
    HOME.with_borrow_mut(|slot| *slot = None);
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
    let cancelled = Rc::new(Cell::new(false));
    let seen = cancelled.clone();
    platform::run_after(delay, move || {
        if !seen.get() {
            update(job);
        }
    });
    Timer {
        cancelled,
        detached: false,
    }
}

/// Cancels its callback when dropped, unless detached.
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
        self.platform.quit();
    }

    pub fn activate(&self, force: bool) {
        self.platform.activate(force);
    }

    pub fn read_clipboard(&self) -> Option<String> {
        self.platform.read_clipboard()
    }

    pub fn write_clipboard(&self, text: String) {
        self.platform.write_clipboard(&text);
    }

    pub fn primary_display_bounds(&self) -> Option<Rect> {
        self.platform.primary_display_bounds()
    }

    pub fn centered(&self, size: Size<Pixels>) -> Rect {
        let display = self.primary_display_bounds().unwrap_or_default();
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

#[derive(Clone, Copy)]
pub(crate) struct Callbacks {
    pub id: WindowId,
    pub thread: platform::AppThread,
}

pub(crate) fn dispatch(window: &mut Window, app: &mut App, mut event: Event) -> bool {
    if window.zoom != 1. {
        event.unzoom(window.zoom);
    }
    window.dispatch_event(event, app)
}

// `ime` is set when the key typed more than one character or composes, and then the page ignores it.
pub(crate) fn send_ime_key(window: &mut Window, app: &mut App, typed: bool) -> bool {
    let Some((mut key, composing)) = window.ime_key.take() else {
        return false;
    };
    let ime = typed || composing;
    if let Event::KeyDown { ime: held, .. } = &mut key {
        *held = ime;
    }
    dispatch(window, app, key) && !ime
}

impl Callbacks {
    // Runs on the app thread and waits for the answer, at most 100 ms from the main thread on macOS.
    pub(crate) fn with<R: Send + 'static>(
        &self,
        job: impl FnOnce(&mut Window, &mut App) -> R + Send + 'static,
    ) -> Option<R> {
        let id = self.id;
        let run = move || try_update(|app| app.update_window(id, job)).flatten();
        match home() {
            Some(_) => run(),
            None => platform::ask(self.thread, run).flatten(),
        }
    }

    pub(crate) fn queue(&self, job: impl FnOnce(&mut Window, &mut App) + Send + 'static) {
        let id = self.id;
        let run = move || {
            update(move |app| {
                app.update_window(id, job);
            })
        };
        match home() {
            Some(_) => run(),
            None => platform::post(self.thread, Box::new(run)),
        }
    }

    pub fn input(&self, event: Event) -> bool {
        self.with(move |window, app| {
            window.ime_key = None;
            dispatch(window, app, event)
        })
        .unwrap_or(false)
    }

    #[cfg(target_os = "macos")]
    pub fn input_later(&self, event: Event) {
        self.queue(move |window, app| {
            dispatch(window, app, event);
        });
    }

    // Frames stop when this returns `Some(false)`, so an idle window gets no calls.
    pub fn frame(&self) -> Option<bool> {
        self.with(|window, app| {
            for callback in window.take_frame_callbacks() {
                callback(window, app);
            }
            if window.dirty || !window.captures.is_empty() {
                window.draw(app);
            }
            window.frame_at = None;
        })?;
        let home = home()?;
        let app = home.app.borrow();
        let window = app.windows.get(&self.id)?.as_deref();
        Some(window.is_some_and(Window::wants_frame))
    }

    pub fn resized(&self, size: Size<Pixels>, scale_factor: f32) {
        native::window_event(self.id, native::WindowEvent::Changed);
        self.queue(move |window, app| {
            window.mark_resized(size, scale_factor);
            window.with_view(app, |view, window, app| view.window_changed(window, app));
        });
    }

    pub fn changed(&self) {
        native::window_event(self.id, native::WindowEvent::Changed);
        self.queue(|window, app| {
            window.refresh();
            window.with_view(app, |view, window, app| view.window_changed(window, app));
        });
    }

    #[cfg(target_os = "macos")]
    pub fn will_change(&self, kind: &str, bounds: Rect) -> bool {
        native::will_change(self.id, kind, bounds)
    }

    // Main decides, and closes the window itself.
    pub fn should_close(&self) -> bool {
        native::window_event(self.id, native::WindowEvent::CloseRequested);
        false
    }

    pub fn appearance_changed(&self) {
        self.queue(|window, _| window.refresh());
    }

    pub fn release(&self) {
        self.queue(|window, _| window.platform.release());
    }

    #[cfg(windows)]
    pub fn in_drag_area(&self, at: Point<Pixels>) -> bool {
        self.with(move |window, _| window.in_drag_area(at / window.zoom))
            .unwrap_or(false)
    }

    pub fn accepts_text_input(&self) -> bool {
        self.with(|window, app| window.with_view(app, |_, _, _| ()).is_some())
            .unwrap_or(false)
    }

    pub fn selection(&self) -> Option<Utf16Selection> {
        self.with(|window, app| {
            window.with_view(app, |view, window, app| view.selection(window, app))
        })
        .flatten()
        .flatten()
    }

    pub fn composition(&self) -> Option<Range<usize>> {
        self.with(|window, app| {
            window.with_view(app, |view, window, app| view.composition(window, app))
        })
        .flatten()
        .flatten()
    }

    #[cfg(not(target_os = "macos"))]
    pub fn clear_composition(&self) {
        if let Some(composed) = self.composition() {
            self.insert(Some(composed), "");
        }
    }

    #[cfg(target_os = "macos")]
    pub fn text_in(&self, range: Range<usize>) -> Option<(String, Option<Range<usize>>)> {
        self.with(move |window, app| {
            let mut actual = None;
            let text = window.with_view(app, |view, window, app| {
                view.text_in(range, &mut actual, window, app)
            });
            Some((text.flatten()?, actual))
        })
        .flatten()
    }

    pub fn insert(&self, range: Option<Range<usize>>, text: &str) {
        let text = text.to_owned();
        self.queue(move |window, app| {
            if !send_ime_key(window, app, text.encode_utf16().count() > 1) {
                window.with_view(app, |view, window, app| {
                    view.insert(range, &text, window, app)
                });
            }
        });
    }

    pub fn compose(&self, range: Option<Range<usize>>, text: &str, selected: Option<Range<usize>>) {
        let text = text.to_owned();
        self.queue(move |window, app| {
            send_ime_key(window, app, true);
            window.with_view(app, |view, window, app| {
                view.compose(range, &text, selected, window, app)
            });
        });
    }

    #[cfg(not(windows))]
    pub fn end_composition(&self) {
        self.queue(|window, app| {
            send_ime_key(window, app, false);
            window.with_view(app, |view, window, app| view.end_composition(window, app));
        });
    }

    pub fn rect_of(&self, range: Range<usize>) -> Option<Rect> {
        self.with(move |window, app| {
            let zoom = window.zoom;
            let rect = window.with_view(app, |view, window, app| view.rect_of(range, window, app));
            Some(rect.flatten()? * zoom)
        })
        .flatten()
    }

    #[cfg(target_os = "macos")]
    pub fn index_at(&self, at: Point<Pixels>) -> Option<usize> {
        self.with(move |window, app| {
            let at = at / window.zoom;
            window.with_view(app, |view, window, app| view.index_at(at, window, app))
        })
        .flatten()
        .flatten()
    }
}

pub mod native {
    use super::*;

    #[derive(Clone, Copy)]
    pub enum WindowEvent {
        Changed,
        CloseRequested,
    }

    // A handler, missing while it runs.
    type Slot<H> = Option<Box<H>>;
    type KeyFilter = dyn FnMut(WindowId, &Event, &dyn Fn() -> bool) -> bool;
    type FrameFilter = dyn FnMut(WindowId, &str, Rect) -> bool;

    #[derive(Default)]
    struct Handlers {
        window: Slot<dyn FnMut(WindowId, WindowEvent)>,
        key: Slot<KeyFilter>,
        will_change: Slot<FrameFilter>,
        reopen: Slot<dyn FnMut(bool)>,
        should_quit: Slot<dyn FnMut() -> bool>,
        open_urls: Slot<dyn FnMut(Vec<String>)>,
        pending_urls: Vec<String>,
        quit: Slot<dyn FnOnce()>,
    }

    thread_local! { static HANDLERS: RefCell<Handlers> = RefCell::default(); }

    // Calls a handler outside the borrow, so it can set handlers. A call from inside the same handler finds
    // none.
    fn call<H: ?Sized, R>(
        slot: fn(&mut Handlers) -> &mut Slot<H>,
        run: impl FnOnce(&mut H) -> R,
    ) -> Option<R> {
        let mut handler = HANDLERS.with_borrow_mut(|handlers| slot(handlers).take())?;
        let result = run(&mut handler);
        HANDLERS.with_borrow_mut(|handlers| {
            slot(handlers).get_or_insert(handler);
        });
        Some(result)
    }

    pub fn on_window_event(f: impl FnMut(WindowId, WindowEvent) + 'static) {
        HANDLERS.with_borrow_mut(|handlers| handlers.window = Some(Box::new(f)));
    }

    pub(crate) fn window_event(id: WindowId, event: WindowEvent) {
        call(
            |handlers| &mut handlers.window,
            |handler| handler(id, event),
        );
    }

    // Return true to keep the key from the page. Call `composing` only when needed: it blocks on the page thread.
    pub fn on_key(f: impl FnMut(WindowId, &Event, &dyn Fn() -> bool) -> bool + 'static) {
        HANDLERS.with_borrow_mut(|handlers| handlers.key = Some(Box::new(f)));
    }

    // Keys from nested loops bypass a filter already in progress.
    pub(crate) fn key(id: WindowId, event: &Event, composing: &dyn Fn() -> bool) -> bool {
        call(
            |handlers| &mut handlers.key,
            |filter| filter(id, event, composing),
        )
        .unwrap_or(false)
    }

    // `kind` is `move`, or the edge of a resize: `right` or `bottom`. Return true to cancel a resize.
    pub fn on_will_change(f: impl FnMut(WindowId, &str, Rect) -> bool + 'static) {
        HANDLERS.with_borrow_mut(|handlers| handlers.will_change = Some(Box::new(f)));
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn will_change(id: WindowId, kind: &str, bounds: Rect) -> bool {
        call(
            |handlers| &mut handlers.will_change,
            |filter| filter(id, kind, bounds),
        )
        .unwrap_or(false)
    }

    pub fn on_reopen(f: impl FnMut(bool) + 'static) {
        HANDLERS.with_borrow_mut(|handlers| handlers.reopen = Some(Box::new(f)));
    }

    pub fn on_should_quit(f: impl FnMut() -> bool + 'static) {
        HANDLERS.with_borrow_mut(|handlers| handlers.should_quit = Some(Box::new(f)));
    }

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

    pub fn post(f: Box<dyn FnOnce() + Send>) {
        platform::native_post(f);
    }

    pub fn after(delay: Duration, f: impl FnOnce() + 'static) {
        platform::native_after(delay, f);
    }

    pub fn sync<R: Send>(f: impl FnOnce() -> R + Send) -> R {
        platform::native_sync(f)
    }

    pub fn before_exit() {
        platform::park_render();
    }

    pub fn activate(force: bool) {
        match THREAD_PER_APP {
            true => platform::Platform::new().activate(force),
            false => update(move |app| app.activate(force)),
        }
    }

    pub fn quit() {
        match THREAD_PER_APP {
            true => platform::Platform::new().quit(),
            false => update(|app| app.quit()),
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn reopen(visible: bool) {
        call(|handlers| &mut handlers.reopen, |handler| handler(visible));
    }

    pub(crate) fn should_quit() -> bool {
        call(|handlers| &mut handlers.should_quit, |handler| handler()).unwrap_or(true)
    }

    pub(crate) fn quit_handler() {
        if let Some(handler) = HANDLERS.with_borrow_mut(|handlers| handlers.quit.take()) {
            handler();
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn open_urls(urls: Vec<String>) {
        let mut urls = Some(urls);
        call(
            |handlers| &mut handlers.open_urls,
            |handler| handler(urls.take().unwrap_or_default()),
        );
        if let Some(urls) = urls {
            HANDLERS.with_borrow_mut(|handlers| handlers.pending_urls.extend(urls));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::native;
    use crate::{Event, Key, Modifiers, WindowId};

    #[test]
    fn the_key_filter_blocks_keys_but_not_the_ones_of_a_loop_nested_in_it() {
        let event = Event::KeyUp {
            key: Key::default(),
            modifiers: Modifiers::NONE,
            ime: false,
        };
        assert!(!native::key(WindowId(1), &event, &|| true));
        native::on_key(|id, event, composing| {
            assert!(!native::key(id, event, composing));
            id == WindowId(1) && composing()
        });
        assert!(native::key(WindowId(1), &event, &|| true));
        assert!(!native::key(WindowId(1), &event, &|| false));
        assert!(!native::key(WindowId(2), &event, &|| true));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn urls_wait_for_their_handler() {
        native::open_urls(vec!["apps://a".into()]);
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let log = seen.clone();
        native::on_open_urls(move |urls| log.borrow_mut().extend(urls));
        native::open_urls(vec!["apps://b".into()]);
        assert_eq!(*seen.borrow(), ["apps://a", "apps://b"]);
    }
}
