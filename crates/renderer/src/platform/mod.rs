#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos::*;
#[cfg(target_os = "macos")]
pub use macos::{WindowControl, control_size};

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::WindowControl;
#[cfg(target_os = "windows")]
pub(crate) use windows::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::WindowControl;
#[cfg(target_os = "linux")]
pub(crate) use linux::*;

pub mod native;

use crate::{Color, Pixels, Point, Rect, Size, px};
use std::{cell::Cell, rc::Rc, time::Duration};

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

pub(crate) fn timer(delay: Duration, job: impl FnOnce() + 'static) -> Timer {
    let cancelled = Rc::new(Cell::new(false));
    let seen = cancelled.clone();
    run_after(delay, move || {
        if !seen.get() {
            job();
        }
    });
    Timer {
        cancelled,
        detached: false,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub u64);

#[derive(Clone, Debug)]
pub struct WindowOptions {
    pub bounds: Rect,
    // `bounds` includes the title bar and frame.
    pub outer_size: bool,
    pub title: String,
    pub titlebar_hidden: bool,
    pub buttons_at: Option<Point<Pixels>>,
    pub frameless: bool,
    pub transparent: bool,
    pub show: bool,
    pub resizable: bool,
    pub minimizable: bool,
    pub movable: bool,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            bounds: crate::rect(px(0.), px(0.), px(800.), px(600.)),
            outer_size: true,
            title: String::new(),
            titlebar_hidden: false,
            buttons_at: None,
            frameless: false,
            transparent: false,
            show: true,
            resizable: true,
            minimizable: true,
            movable: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WindowState {
    pub frame: Rect,
    pub content: Rect,
    pub minimized: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    pub visible: bool,
    pub resizing: bool,
    pub display: Rect,
    pub work_area: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowChange {
    Frame(Rect),
    Center,
    Show(bool),
    Hide,
    Blur,
    Restore,
    SizeLimits(Size<Pixels>, Size<Pixels>),
    /// An `NSWindowLevel`, where 0 is normal.
    AlwaysOnTop(isize),
    Resizable(bool),
    Minimizable(bool),
    Maximizable(bool),
    Closable(bool),
    Movable(bool),
    Background(Color),
    Opacity(f32),
    Shadow(bool),
    IgnoreMouse(bool, bool),
    AllWorkspaces(bool, bool),
    ContentProtection(bool),
    Focusable(bool),
    AspectRatio(f32, Size<Pixels>),
    Parent(Option<raw_window_handle::RawWindowHandle>, bool),
    Vibrancy(Option<isize>, isize),
    Edited(bool),
    CancelComposition,
}

pub const CARET_BLINK: Duration = Duration::from_millis(500);

// On Windows and Linux one thread runs the app and every window.
#[cfg(not(target_os = "macos"))]
mod single {
    use super::{native_post, on_main};
    use crate::ControlKind;
    use std::{sync::mpsc, time::Duration};

    pub(crate) type Task = Box<dyn FnOnce() + Send>;

    #[derive(Clone, Copy)]
    pub(crate) struct AppThread;

    pub(crate) fn app_thread() -> AppThread {
        AppThread
    }

    pub(crate) fn post(_: AppThread, task: Task) {
        native_post(task);
    }

    pub(crate) fn ask<R: Send + 'static>(
        _: AppThread,
        f: impl FnOnce() -> R + Send + 'static,
    ) -> Option<R> {
        if on_main() {
            return Some(f());
        }
        let (answer, wait) = mpsc::sync_channel(1);
        native_post(Box::new(move || {
            let _ = answer.send(f());
        }));
        wait.recv_timeout(Duration::from_millis(100)).ok()
    }

    pub(crate) fn native_sync<R: Send>(f: impl FnOnce() -> R + Send) -> R {
        if on_main() {
            return f();
        }
        let (answer, wait) = mpsc::sync_channel(1);
        let job: Box<dyn FnOnce() + Send + '_> = Box::new(move || {
            let _ = answer.send(f());
        });
        // SAFETY: this thread waits until the job ran or was dropped, so what the job borrows outlives it.
        native_post(unsafe { std::mem::transmute::<Box<dyn FnOnce() + Send + '_>, Task>(job) });
        wait.recv().expect("the main thread dropped its work")
    }

    // The app runs on this thread, and `app::run` drops it after the loop.
    pub(crate) fn spawn_render(
        init: impl FnOnce() + Send + 'static,
        _: impl FnOnce() + Send + 'static,
    ) {
        init();
    }

    pub(crate) fn end_thread() {}

    pub(crate) fn park_render() {}

    pub fn control_size(_: ControlKind) -> [f32; 2] {
        [0.; 2]
    }
}
#[cfg(not(target_os = "macos"))]
pub use single::control_size;
#[cfg(not(target_os = "macos"))]
pub(crate) use single::*;
