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
pub(crate) use linux::*;
#[cfg(target_os = "linux")]
pub use linux::{PREFER_X11, WindowControl, clipboard};

pub mod native;

use crate::{Color, Event, Pixels, Point, Rect, Size, px};
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

/// A native window's link to its app: the window and the thread of the app.
#[derive(Clone, Copy)]
pub(crate) struct Callbacks {
  pub id: WindowId,
  pub thread: AppThread,
}

/// The newest pointer move or wheel the app thread has not taken yet, to merge more into it, and when its
/// device sent it.
#[cfg(target_os = "macos")]
pub(crate) type Motion = std::sync::Arc<std::sync::Mutex<Option<(Event, std::time::Instant)>>>;

/// What native windows ask of the app that owns them. The app sets it once, with `set_events`.
pub(crate) trait WindowEvents: Sync {
  fn input(&self, at: Callbacks, event: Event) -> bool;
  #[cfg(target_os = "macos")]
  fn input_later(&self, at: Callbacks, event: Event);
  #[cfg(target_os = "macos")]
  fn input_motion(&self, at: Callbacks, motion: Motion);
  #[cfg(target_os = "macos")]
  fn hold_key(&self, at: Callbacks, key: Event, composing: bool);
  /// The input method took the held key.
  #[cfg(target_os = "macos")]
  fn ime_took_key(&self, at: Callbacks);
  /// `Some(false)` when the window wants no more frames.
  #[cfg(not(target_os = "macos"))]
  fn frame(&self, at: Callbacks) -> Option<bool>;
  /// A display tick at `time`, or a forced frame with `None`.
  #[cfg(target_os = "macos")]
  fn tick(&self, at: Callbacks, time: Option<std::time::Instant>);
  /// Draws a frame of a live resize, and returns its present.
  #[cfg(target_os = "macos")]
  fn present_live(&self, at: Callbacks, visible: bool) -> Option<Present>;
  fn resized(&self, at: Callbacks, size: Size<Pixels>, scale_factor: f32);
  fn changed(&self, at: Callbacks);
  fn appearance_changed(&self, at: Callbacks);
  fn release(&self, at: Callbacks);
  /// The GPU device is gone, so all windows draw again.
  #[cfg(windows)]
  fn device_lost(&self);
  #[cfg(windows)]
  fn in_drag_area(&self, at: Callbacks, point: Point<Pixels>) -> bool;
  fn accepts_text_input(&self, at: Callbacks) -> bool;
  fn selection(&self, at: Callbacks) -> Option<crate::Utf16Selection>;
  fn composition(&self, at: Callbacks) -> Option<std::ops::Range<usize>>;
  #[cfg(not(target_os = "macos"))]
  fn clear_composition(&self, at: Callbacks);
  #[cfg(target_os = "macos")]
  #[allow(clippy::type_complexity)]
  fn text_in(
    &self,
    at: Callbacks,
    range: std::ops::Range<usize>,
  ) -> Option<(String, Option<std::ops::Range<usize>>)>;
  fn insert(&self, at: Callbacks, range: Option<std::ops::Range<usize>>, text: &str);
  fn compose(
    &self,
    at: Callbacks,
    range: Option<std::ops::Range<usize>>,
    text: &str,
    selected: Option<std::ops::Range<usize>>,
  );
  #[cfg(not(windows))]
  fn end_composition(&self, at: Callbacks);
  fn rect_of(&self, at: Callbacks, range: std::ops::Range<usize>) -> Option<Rect>;
  #[cfg(target_os = "macos")]
  fn index_at(&self, at: Callbacks, point: Point<Pixels>) -> Option<usize>;
}

static EVENTS: std::sync::OnceLock<&'static dyn WindowEvents> = std::sync::OnceLock::new();

pub(crate) fn set_events(events: &'static dyn WindowEvents) {
  let _ = EVENTS.set(events);
}

pub(crate) fn events() -> &'static dyn WindowEvents {
  *EVENTS
    .get()
    .expect("the app sets its window events before it opens windows")
}

/// Runs `f` on the main thread, which owns the native windows.
pub fn post_main(f: Box<dyn FnOnce() + Send>) {
  native_post(f);
}

pub fn post_main_after(delay: Duration, f: impl FnOnce() + 'static) {
  native_after(delay, f);
}

/// Runs `f` on the main thread and waits for its result.
pub fn sync_main<R: Send>(f: impl FnOnce() -> R + Send) -> R {
  native_sync(f)
}

/// Builds the GPU device and pipelines on this thread, so the first window doesn't wait for them.
pub fn prewarm() {
  #[cfg(target_os = "macos")]
  macos::prewarm();
}

pub fn before_exit() {
  park_render();
}

pub fn activate(force: bool) {
  Platform::activate(force);
}

pub fn quit() {
  Platform::quit();
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

// After this long without frames, a window gives back its spare GPU resources.
pub(crate) const QUIET: Duration = Duration::from_secs(1);

// A panic while a lock is held leaves its data usable here.
pub(crate) fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
  mutex
    .lock()
    .unwrap_or_else(std::sync::PoisonError::into_inner)
}

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
