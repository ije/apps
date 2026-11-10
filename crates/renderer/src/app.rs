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
    queue: RefCell<VecDeque<Callback>>,
}

thread_local! {
    static APP: RefCell<Option<Rc<AppCell>>> = const { RefCell::new(None) };
}

fn cell() -> Option<Rc<AppCell>> {
    APP.with_borrow(Clone::clone)
}

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

pub const THREAD_PER_APP: bool = cfg!(target_os = "macos");

pub fn spawn(init: impl FnOnce(&mut App) + Send + 'static) {
    #[cfg(target_os = "macos")]
    platform::spawn_render(
        move || {
            install(Rc::new(platform::Platform::new()));
            update(init);
        },
        // Drop windows before thread-local teardown: their destructors need other thread locals.
        || {
            APP.with_borrow_mut(|slot| *slot = None);
        },
    );
    #[cfg(not(target_os = "macos"))]
    update(init);
}

pub fn run(native: impl FnOnce() + 'static) {
    let platform = Rc::new(platform::Platform::new());
    #[cfg(target_os = "macos")]
    platform.run(Box::new(native));
    #[cfg(not(target_os = "macos"))]
    {
        install(platform.clone());
        platform.run(Box::new(native));
        // Close native views before the app: their callbacks can still access it.
        let windows = cell().map(|cell| std::mem::take(&mut cell.app.borrow_mut().windows));
        drop(windows);
        APP.with_borrow_mut(|slot| *slot = None);
    }
}

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
    pub fn stop_propagation(&mut self) {
        self.propagate_event = false;
    }

    pub fn defer(&mut self, f: impl FnOnce(&mut App) + 'static) {
        self.deferred.push(Box::new(f));
    }

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

    pub fn end_thread(&self) {
        #[cfg(target_os = "macos")]
        platform::end_thread();
    }

    pub fn on_wake(&mut self, f: impl FnMut(&mut App) + 'static) {
        self.on_wake = Some(Rc::new(RefCell::new(f)));
    }

    pub fn quit(&mut self) {
        self.platform.quit();
    }

    pub fn activate(&self, ignoring_other_apps: bool) {
        self.platform.activate(ignoring_other_apps);
    }

    pub fn read_clipboard(&self) -> Option<String> {
        self.platform.read_clipboard()
    }

    pub fn write_clipboard(&self, text: String) {
        self.platform.write_clipboard(&text);
    }

    pub fn primary_display_bounds(&self) -> Option<Bounds<Pixels>> {
        self.platform.primary_display_bounds()
    }

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

#[derive(Clone, Copy)]
pub(crate) struct Callbacks {
    pub id: WindowId,
    #[cfg(target_os = "macos")]
    pub thread: platform::AppThread,
}

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

pub(crate) fn send_ime_key(window: &mut Window, app: &mut App, ime: bool) -> bool {
    let Some((mut down, composing)) = window.ime_key.take() else {
        return false;
    };
    down.ime = ime || composing;
    let ime = down.ime;
    dispatch(window, app, PlatformInput::KeyDown(down)) && !ime
}

impl Callbacks {
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

    pub fn input(&self, input: PlatformInput) -> bool {
        self.with(move |window, app| {
            window.ime_key = None;
            dispatch(window, app, input)
        })
        .unwrap_or(false)
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn input_later(&self, input: PlatformInput) {
        self.queue(move |window, app| {
            dispatch(window, app, input);
        });
    }

    pub fn frame(&self) -> Option<bool> {
        self.with(|window, app| {
            for callback in window.take_next_frame_callbacks() {
                callback(window, app);
            }
            if window.dirty || !window.captures.is_empty() {
                window.draw(app);
            }
        })?;
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

    pub fn changed(&self) {
        native::window_event(self.id, native::WindowEvent::Changed);
        self.queue(|window, app| {
            window.refresh();
            window.with_view(app, |view, window, app| view.window_changed(window, app));
        });
    }

    pub fn should_close(&self) -> bool {
        native::window_event(self.id, native::WindowEvent::CloseRequested);
        false
    }

    pub fn appearance_changed(&self) {
        self.queue(|window, _| window.refresh());
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn release(&self) {
        #[cfg(target_os = "macos")]
        self.queue(|window, _| window.platform.release());
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn in_drag_area(&self) -> bool {
        self.with(|window, _| window.in_drag_area())
            .unwrap_or(false)
    }

    pub fn accepts_text_input(&self) -> bool {
        self.with(|window, app| window.with_view(app, |_, _, _| true))
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

    pub fn marked_text_range(&self) -> Option<Range<usize>> {
        self.with(|window, app| {
            window.with_view(app, |view, window, app| view.marked_text_range(window, app))
        })
        .flatten()
        .flatten()
    }

    #[cfg(not(target_os = "macos"))]
    pub fn clear_marked_text(&self) {
        if let Some(marked) = self.marked_text_range() {
            self.replace_text_in_range(Some(marked), "");
        }
    }

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
            if send_ime_key(window, app, text.encode_utf16().count() > 1) {
                return;
            }
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
            send_ime_key(window, app, true);
            window.with_view(app, |view, window, app| {
                view.replace_and_mark_text_in_range(range, &text, selected, window, app)
            });
        });
    }

    #[cfg_attr(target_os = "windows", allow(dead_code))]
    pub fn unmark_text(&self) {
        self.queue(|window, app| {
            send_ime_key(window, app, false);
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

pub mod native {
    use super::*;

    #[derive(Clone, Copy)]
    pub enum WindowEvent {
        Changed,
        CloseRequested,
    }

    type KeyFilter = Box<dyn FnMut(WindowId, &PlatformInput, &dyn Fn() -> bool) -> bool>;

    #[derive(Default)]
    struct Handlers {
        window: Option<Box<dyn FnMut(WindowId, WindowEvent)>>,
        key: Option<KeyFilter>,
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

    // Call `composing` only when needed: it blocks on the page thread.
    pub fn on_key(f: impl FnMut(WindowId, &PlatformInput, &dyn Fn() -> bool) -> bool + 'static) {
        HANDLERS.with_borrow_mut(|handlers| handlers.key = Some(Box::new(f)));
    }

    // Keys from nested loops bypass a filter already in progress.
    pub(crate) fn key(id: WindowId, input: &PlatformInput, composing: &dyn Fn() -> bool) -> bool {
        let Some(mut callback) = HANDLERS.with_borrow_mut(|handlers| handlers.key.take()) else {
            return false;
        };
        let blocked = callback(id, input, composing);
        HANDLERS.with_borrow_mut(|handlers| {
            handlers.key.get_or_insert(callback);
        });
        blocked
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
        #[cfg(target_os = "macos")]
        platform::native_post(f);
        #[cfg(not(target_os = "macos"))]
        platform::post(f);
    }

    pub fn after(delay: Duration, f: impl FnOnce() + 'static) {
        #[cfg(target_os = "macos")]
        platform::native_after(delay, f);
        #[cfg(not(target_os = "macos"))]
        platform::run_after(delay, f);
    }

    pub fn sync<R: Send>(f: impl FnOnce() -> R + Send) -> R {
        #[cfg(target_os = "macos")]
        return platform::native_sync(f);
        #[cfg(not(target_os = "macos"))]
        f()
    }

    pub fn before_exit() {
        #[cfg(target_os = "macos")]
        platform::park_render();
    }

    pub fn activate(ignoring_other_apps: bool) {
        #[cfg(target_os = "macos")]
        platform::Platform::new().activate(ignoring_other_apps);
        #[cfg(not(target_os = "macos"))]
        update(move |app| app.activate(ignoring_other_apps));
    }

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

#[cfg(test)]
mod tests {
    use super::native;
    use crate::{KeyUpEvent, PlatformInput, WindowId};

    #[test]
    fn the_key_filter_blocks_keys_but_not_the_ones_of_a_loop_nested_in_it() {
        let input = PlatformInput::KeyUp(KeyUpEvent::default());
        assert!(!native::key(WindowId(1), &input, &|| true));
        native::on_key(|id, input, composing| {
            assert!(!native::key(id, input, composing));
            id == WindowId(1) && composing()
        });
        assert!(native::key(WindowId(1), &input, &|| true));
        assert!(!native::key(WindowId(1), &input, &|| false));
        assert!(!native::key(WindowId(2), &input, &|| true));
    }
}
