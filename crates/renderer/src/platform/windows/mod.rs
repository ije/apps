// Screen coordinates are physical pixels from the top left of the primary display. Window points are those
// pixels divided by the scale of the window's display.
//
// One thread runs everything: the message loop, the app and its windows. Work from other threads arrives
// through the inbox, which a hidden top-level window drains.

mod d3d;
mod keys;
mod window;

use super::single::Task;
use crate::text::windows::{extent, setting};
use crate::{Cursor, Rect, WindowOptions, platform::Callbacks, platform::lock, point, px, size};
use anyhow::Result;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering::*};
use std::sync::{Condvar, Mutex, Once, OnceLock, PoisonError};
use std::{cell::RefCell, ffi::c_void, thread, time::Duration, time::Instant};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::{Dwm::*, Gdi::*};
use windows::Win32::System::{DataExchange::*, Memory::*, Ole::*, Threading::*};
use windows::Win32::UI::{HiDpi::*, WindowsAndMessaging::*};
use windows::{Win32::System::LibraryLoader::GetModuleHandleW, core::w};

pub(crate) use d3d::Atlas;
pub(crate) use window::Window;
pub use window::WindowControl;

const WM_WAKE: u32 = WM_APP + 1;

static HOST: AtomicIsize = AtomicIsize::new(0);
static MAIN: AtomicU32 = AtomicU32::new(0);
static INBOX: Mutex<VecDeque<Task>> = Mutex::new(VecDeque::new());
static WOKEN: AtomicBool = AtomicBool::new(false);
static ALARM: OnceLock<PTP_TIMER> = OnceLock::new();
static POINTER_SHAPE: AtomicIsize = AtomicIsize::new(0);

#[derive(Default)]
struct Timers {
  count: u64,
  due: BTreeMap<(Instant, u64), Box<dyn FnOnce()>>,
}

thread_local! {
    static TIMERS: RefCell<Timers> = RefCell::default();
}

fn host() -> HWND {
  HWND(HOST.load(Acquire) as _)
}

pub(crate) fn on_main() -> bool {
  // SAFETY: a plain thread query.
  unsafe { GetCurrentThreadId() == MAIN.load(Acquire) }
}

fn current_cursor() -> Option<HCURSOR> {
  let handle = POINTER_SHAPE.load(Relaxed);
  (handle != 0).then_some(HCURSOR(handle as _))
}

pub(crate) fn native_post(task: Task) {
  lock(&INBOX).push_back(task);
  wake();
}

// Posts one wake for any number of tasks. A failed post leaves the flag clear for the next try.
fn wake() {
  if WOKEN.swap(true, AcqRel) {
    return;
  }
  // SAFETY: posting to a window of this process.
  if unsafe { PostMessageW(Some(host()), WM_WAKE, WPARAM(0), LPARAM(0)) }.is_err() {
    WOKEN.store(false, Release);
  }
}

// Runs the work that is due now. Work that it adds waits for the next wake, so input and frames get a turn.
fn drain() {
  WOKEN.store(false, Release);
  let count = lock(&INBOX).len();
  for _ in 0..count {
    let mut inbox = lock(&INBOX);
    let Some(task) = inbox.pop_front() else {
      break;
    };
    let more = !inbox.is_empty();
    drop(inbox);
    // A task can run a message loop of its own, which takes the rest.
    if more {
      wake();
    }
    task();
  }
  let (now, end) = (Instant::now(), TIMERS.with_borrow(|timers| timers.count));
  while let Some(f) = TIMERS.with_borrow_mut(|timers| {
    let timer = timers.due.first_entry()?;
    let (at, number) = *timer.key();
    (at <= now && number < end).then(|| timer.remove())
  }) {
    set_alarm();
    f();
  }
  set_alarm();
}

// `WM_TIMER` comes only when no other message waits, not even a paint, so busy windows would hold it back.
fn set_alarm() {
  let next = TIMERS.with_borrow(|timers| timers.due.first_key_value().map(|(key, _)| key.0));
  let Some(next) = next else {
    return;
  };
  let wait = next.saturating_duration_since(Instant::now());
  if wait.is_zero() {
    return wake();
  }
  // SAFETY: the timer is never closed, and its callback only posts a message.
  let alarm = ALARM.get_or_init(|| unsafe {
    CreateThreadpoolTimer(Some(alarm), None, None).expect("the timer must open")
  });
  // A negative time counts from now, in units of 100 ns.
  let time = -i64::try_from(wait.as_nanos().div_ceil(100)).unwrap_or(i64::MAX);
  let time = FILETIME {
    dwLowDateTime: time as u32,
    dwHighDateTime: (time >> 32) as u32,
  };
  // SAFETY: the timer is live, and the time is a local.
  unsafe { SetThreadpoolTimer(*alarm, Some(&time), 0, None) };
}

unsafe extern "system" fn alarm(_: PTP_CALLBACK_INSTANCE, _: *mut c_void, _: PTP_TIMER) {
  wake();
}

pub(crate) use native_after as run_after;

// Timers run on the thread that set them, which is the main thread.
pub(crate) fn native_after(delay: Duration, f: impl FnOnce() + 'static) {
  let first = TIMERS.with_borrow_mut(|timers| {
    timers.count += 1;
    let key = (Instant::now() + delay, timers.count - 1);
    timers.due.insert(key, Box::new(f));
    timers
      .due
      .first_key_value()
      .is_some_and(|(first, _)| *first == key)
  });
  if first {
    set_alarm();
  }
}

pub(crate) struct Platform;

impl Platform {
  pub fn new() -> Self {
    if HOST.load(Acquire) != 0 {
      return Self;
    }
    // SAFETY: process setup and a hidden window on this thread, which becomes the main thread.
    unsafe {
      let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
      let _ = OleInitialize(None);
      let instance = GetModuleHandleW(None).unwrap_or_default();
      let class = WNDCLASSW {
        lpfnWndProc: Some(host_proc),
        hInstance: instance.into(),
        lpszClassName: w!("AppsHost"),
        ..Default::default()
      };
      RegisterClassW(&class);
      // A top-level window, unlike a message-only one, also gets the session end and setting changes.
      let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW,
        class.lpszClassName,
        None,
        WINDOW_STYLE::default(),
        0,
        0,
        0,
        0,
        None,
        None,
        Some(class.hInstance),
        None,
      )
      .expect("the host window must open");
      MAIN.store(GetCurrentThreadId(), Release);
      HOST.store(hwnd.0 as isize, Release);
    }
    self::set_cursor(Cursor::Default);
    // Tasks posted before the host existed wait for this wake.
    WOKEN.store(false, Release);
    if !lock(&INBOX).is_empty() {
      wake();
    }
    Self
  }

  pub fn run(&self, on_launch: Box<dyn FnOnce()>) {
    on_launch();
    let mut next = MSG::default();
    // SAFETY: the message loop of this thread.
    unsafe {
      // A negative result is an error, which also ends the loop.
      'run: while GetMessageW(&mut next, None, 0, 0).0 > 0 {
        let woken = next.message == WM_WAKE;
        dispatch(&next);
        // Posted messages come before input and paints, so each wake lets one of both through.
        for kind in [PM_QS_INPUT, PM_QS_PAINT] {
          if woken && PeekMessageW(&mut next, None, 0, 0, PM_REMOVE | kind).as_bool() {
            if next.message == WM_QUIT {
              break 'run;
            }
            dispatch(&next);
          }
        }
      }
    }
    crate::native::quit_handler();
  }

  pub fn quit() {
    // The quit handlers need the app, which is in use while this runs.
    run_after(Duration::ZERO, || {
      if crate::native::should_quit() {
        // SAFETY: ends the message loop of this thread.
        unsafe {
          PostQuitMessage(0);
        }
      }
    });
  }

  pub fn activate(_: bool) {}

  pub fn read_clipboard(&self) -> Option<String> {
    // SAFETY: the clipboard handle stays locked while its text is copied, and the copy stops at the
    // terminator or the end of the allocation.
    unsafe {
      OpenClipboard(Some(host())).ok()?;
      let text = GetClipboardData(u32::from(CF_UNICODETEXT.0))
        .ok()
        .and_then(|handle| {
          let memory = HGLOBAL(handle.0);
          let locked = GlobalLock(memory).cast::<u16>();
          if locked.is_null() {
            return None;
          }
          let all = std::slice::from_raw_parts(locked, GlobalSize(memory) / 2);
          let text = all.split(|&unit| unit == 0).next().unwrap_or(all);
          let text = String::from_utf16_lossy(text);
          let _ = GlobalUnlock(memory);
          Some(text)
        });
      let _ = CloseClipboard();
      text
    }
  }

  pub fn write_clipboard(&self, text: &str) {
    // A NUL would end the text early, so it becomes a space.
    let wide = text
      .encode_utf16()
      .map(|unit| if unit == 0 { 0x20 } else { unit })
      .chain([0])
      .collect::<Vec<u16>>();
    // SAFETY: the allocation holds the units, and the clipboard owns it once it takes it.
    unsafe {
      if OpenClipboard(Some(host())).is_err() {
        return;
      }
      let _ = EmptyClipboard();
      if let Ok(memory) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) {
        let locked = GlobalLock(memory).cast::<u16>();
        let taken = !locked.is_null() && {
          locked.copy_from_nonoverlapping(wide.as_ptr(), wide.len());
          let _ = GlobalUnlock(memory);
          SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(memory.0))).is_ok()
        };
        if !taken {
          let _ = GlobalFree(Some(memory));
        }
      }
      let _ = CloseClipboard();
    }
  }

  pub fn set_cursor(&self, cursor: Cursor) {
    let handle = self::set_cursor(cursor);
    // SAFETY: shows the cursor now, and WM_SETCURSOR keeps it over the content.
    unsafe { SetCursor(handle) };
  }

  pub fn primary_display_bounds(&self) -> Option<Rect> {
    // SAFETY: a plain monitor query.
    let primary = unsafe { MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY) };
    Some(points(
      monitor_info(primary)?.rcWork,
      monitor_scale(primary),
    ))
  }

  pub fn open_window(&self, options: &WindowOptions, callbacks: Callbacks) -> Result<Window> {
    Window::open(options, callbacks)
  }
}

fn dispatch(message: &MSG) {
  // Keys go to the key filter and the page before they turn into characters.
  if matches!(message.message, WM_KEYDOWN | WM_SYSKEYDOWN) && window::key_down(message) {
    return;
  }
  // SAFETY: a message of this thread.
  unsafe {
    let _ = TranslateMessage(message);
    DispatchMessageW(message);
  }
}

fn set_cursor(cursor: Cursor) -> Option<HCURSOR> {
  let handle = window::cursor(cursor);
  let raw = handle.map_or(0, |handle| handle.0 as isize);
  POINTER_SHAPE.store(raw, Relaxed);
  handle
}

unsafe extern "system" fn host_proc(
  hwnd: HWND,
  msg: u32,
  wparam: WPARAM,
  lparam: LPARAM,
) -> LRESULT {
  match msg {
    WM_WAKE => drain(),
    // Agrees to end the session, then quits when it ends.
    WM_QUERYENDSESSION => return LRESULT(1),
    WM_ENDSESSION if wparam.0 != 0 => {
      crate::native::quit_handler();
      std::process::exit(0);
    }
    // SAFETY: the default handling of a message that this window got.
    _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
  }
  LRESULT(0)
}

fn monitor_info(monitor: HMONITOR) -> Option<MONITORINFO> {
  let mut info = MONITORINFO::default();
  info.cbSize = size_of_val(&info) as u32;
  // SAFETY: the struct has its size set.
  let found = unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool();
  found.then_some(info)
}

fn monitor_scale(monitor: HMONITOR) -> f32 {
  let mut dpi = [USER_DEFAULT_SCREEN_DPI; 2];
  let [x, y] = &mut dpi;
  // SAFETY: a plain monitor query into locals.
  let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, x, y) };
  dpi[0] as f32 / USER_DEFAULT_SCREEN_DPI as f32
}

fn points(rect: RECT, scale: f32) -> Rect {
  let corner = point(px(rect.left as f32), px(rect.top as f32));
  let extent = size(
    px((rect.right - rect.left) as f32),
    px((rect.bottom - rect.top) as f32),
  );
  Rect::new(corner, extent) / scale
}

// Windows that want frames. A pacing thread waits for each composition of the desktop and then invalidates
// them, so frames come as WM_PAINT, which yields to input, and an idle window gets none.
struct Pacing {
  wanting: Mutex<Vec<isize>>,
  wanted: Condvar,
}

static PACING: Pacing = Pacing {
  wanting: Mutex::new(vec![]),
  wanted: Condvar::new(),
};
static PACER: Once = Once::new();

fn want_frames(hwnd: HWND, on: bool) {
  let key = hwnd.0 as isize;
  let mut windows = lock(&PACING.wanting);
  if !on {
    windows.retain(|&other| other != key);
  } else if !windows.contains(&key) {
    windows.push(key);
  }
  drop(windows);
  if on {
    PACER.call_once(|| drop(thread::spawn(pace)));
    PACING.wanted.notify_one();
  }
}

fn pace() {
  let mut timing = DWM_TIMING_INFO::default();
  timing.cbSize = size_of_val(&timing) as u32;
  // SAFETY: a plain DWM query into a sized struct.
  let rate = unsafe { DwmGetCompositionTimingInfo(HWND::default(), &mut timing) }
    .map(|()| timing.rateRefresh)
    .ok()
    .filter(|rate| rate.uiNumerator > 0);
  let refresh = rate.map_or(Duration::from_micros(16_667), |rate| {
    Duration::from_secs_f64(f64::from(rate.uiDenominator) / f64::from(rate.uiNumerator))
  });
  let mut due = Vec::new();
  loop {
    {
      let all = lock(&PACING.wanting);
      let mut windows = PACING
        .wanted
        .wait_while(all, |list| list.is_empty())
        .unwrap_or_else(PoisonError::into_inner);
      // Hidden and minimized windows drop out, and ask again once shown.
      windows.retain(|&raw| {
        let hwnd = HWND(raw as _);
        // SAFETY: plain window queries, which fail harmlessly for closed windows.
        let (shown, minimized) = unsafe { (IsWindowVisible(hwnd), IsIconic(hwnd)) };
        shown.as_bool() && !minimized.as_bool()
      });
      due.clone_from(&windows);
    }
    let asked = Instant::now();
    // Without composition DwmFlush returns at once, so a sleep keeps the rate.
    // SAFETY: waits for the next composition.
    if unsafe { DwmFlush() }.is_err() || asked.elapsed() < Duration::from_millis(1) {
      thread::sleep(refresh);
    }
    for &raw in &due {
      // SAFETY: invalidating a window of this process, which fails harmlessly once it closed.
      let _ = unsafe { InvalidateRect(Some(HWND(raw as _)), None, false) };
    }
  }
}
