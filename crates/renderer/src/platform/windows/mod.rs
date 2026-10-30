//! Windows: Win32 windows, Direct3D 11 through DirectComposition, and DirectWrite.
//!
//! Screen coordinates are physical pixels with the origin at the top left of the primary
//! display. Logical pixels divide them by the window's scale factor.

mod directx;
mod events;
mod text;
mod window;

use crate::{Bounds, CursorStyle, Pixels, WindowOptions, app::Callbacks, point, px, size};
use anyhow::Result;
use std::{
    cell::Cell,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicIsize, Ordering},
    },
    thread::Thread,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{Dwm::*, Gdi::*},
        System::{DataExchange::*, LibraryLoader::GetModuleHandleW, Memory::*, Ole::*},
        UI::{HiDpi::*, WindowsAndMessaging::*},
    },
    core::w,
};

pub(crate) use text::TextSystem;
pub(crate) use window::Window;

/// Runs the `Box<Box<dyn FnOnce() + Send>>` in the lparam.
const WM_POST: u32 = WM_USER + 1;
/// Runs the `Box<Box<dyn FnOnce()>>` in the lparam.
const WM_LOCAL: u32 = WM_USER + 2;

/// The hidden window that runs posted work and timers, and gets session messages.
static PLATFORM: AtomicIsize = AtomicIsize::new(0);
/// The windows that want a frame on each display refresh.
static WANTED: Mutex<Vec<isize>> = Mutex::new(Vec::new());
static VSYNC: OnceLock<Thread> = OnceLock::new();

thread_local! {
    /// The cursor shown over the content of our windows.
    static CURSOR: Cell<Option<HCURSOR>> = const { Cell::new(None) };
}

fn platform_window() -> HWND {
    HWND(PLATFORM.load(Ordering::Acquire) as _)
}

pub(crate) struct Platform;

impl Platform {
    pub fn new() -> Self {
        // SAFETY: plain Win32 setup on the main thread.
        unsafe {
            // Fails when a manifest already set the awareness.
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            // WebView2 needs a single threaded COM apartment on this thread.
            let _ = OleInitialize(None);
            let instance = GetModuleHandleW(None).unwrap_or_default();
            let class = WNDCLASSW {
                lpfnWndProc: Some(platform_proc),
                hInstance: instance.into(),
                lpszClassName: w!("AppsPlatform"),
                ..Default::default()
            };
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("AppsPlatform"),
                None,
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .expect("the platform window must be created");
            PLATFORM.store(hwnd.0 as isize, Ordering::Release);
        }
        CURSOR.set(window::load_cursor(CursorStyle::Arrow));
        Self
    }

    pub fn run(&self, on_launch: Box<dyn FnOnce()>) {
        on_launch();
        let mut msg = MSG::default();
        // SAFETY: the standard message loop.
        unsafe {
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                if matches!(msg.message, WM_KEYDOWN | WM_SYSKEYDOWN) && window::key_down(&msg) {
                    continue;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        crate::app::events::quit();
    }

    pub fn quit(&self) {
        // WARNING: quitting asks the app first, which is busy while this runs.
        run_after(Duration::ZERO, || {
            if crate::app::events::should_quit() {
                // SAFETY: ends the message loop of this thread.
                unsafe { PostQuitMessage(0) };
            }
        });
    }

    pub fn activate(&self, _ignoring_other_apps: bool) {}

    pub fn read_clipboard(&self) -> Option<String> {
        // SAFETY: the clipboard data is locked while it is read, within its size.
        unsafe {
            OpenClipboard(Some(platform_window())).ok()?;
            let text = GetClipboardData(CF_UNICODETEXT.0 as u32)
                .ok()
                .and_then(|data| {
                    let global = HGLOBAL(data.0);
                    let chars = GlobalLock(global) as *const u16;
                    if chars.is_null() {
                        return None;
                    }
                    let max = GlobalSize(global) / 2;
                    let len = (0..max).take_while(|&index| *chars.add(index) != 0).count();
                    let text = String::from_utf16_lossy(std::slice::from_raw_parts(chars, len));
                    let _ = GlobalUnlock(global);
                    Some(text)
                });
            let _ = CloseClipboard();
            text
        }
    }

    pub fn write_clipboard(&self, text: &str) {
        // Clipboard text ends at the first NUL.
        let wide: Vec<u16> = text
            .replace('\0', " ")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // SAFETY: the copy fits the allocation, which the clipboard owns once set.
        unsafe {
            if OpenClipboard(Some(platform_window())).is_err() {
                return;
            }
            let _ = EmptyClipboard();
            if let Ok(global) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) {
                let data = GlobalLock(global);
                let set = !data.is_null() && {
                    std::ptr::copy_nonoverlapping(wide.as_ptr(), data.cast(), wide.len());
                    let _ = GlobalUnlock(global);
                    SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(global.0))).is_ok()
                };
                if !set {
                    let _ = GlobalFree(Some(global));
                }
            }
            let _ = CloseClipboard();
        }
    }

    pub fn set_cursor(&self, style: CursorStyle) {
        let cursor = window::load_cursor(style);
        CURSOR.set(cursor);
        // SAFETY: shows the cursor now; WM_SETCURSOR keeps it over the content.
        unsafe { SetCursor(cursor) };
    }

    pub fn primary_display_bounds(&self) -> Option<Bounds<Pixels>> {
        // SAFETY: plain monitor queries.
        unsafe {
            let monitor = MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY);
            let mut info = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            GetMonitorInfoW(monitor, &mut info).ok().ok()?;
            Some(logical_rect(info.rcWork, monitor_scale(monitor)))
        }
    }

    pub fn open_window(&self, options: &WindowOptions, callbacks: Callbacks) -> Result<Window> {
        Window::open(options, callbacks)
    }
}

/// Runs `f` on the main thread, from any thread.
pub(crate) fn post(f: Box<dyn FnOnce() + Send>) {
    let raw = Box::into_raw(Box::new(f));
    // SAFETY: the platform window takes back the box.
    if unsafe {
        PostMessageW(
            Some(platform_window()),
            WM_POST,
            WPARAM(0),
            LPARAM(raw as isize),
        )
    }
    .is_err()
    {
        // SAFETY: the message was not posted, so the box is still ours.
        drop(unsafe { Box::from_raw(raw) });
    }
}

/// Runs `f` on the main thread after `delay`.
pub(crate) fn run_after(delay: Duration, f: impl FnOnce() + 'static) {
    let f: Box<dyn FnOnce()> = Box::new(f);
    let raw = Box::into_raw(Box::new(f));
    let millis = delay.as_millis().min(USER_TIMER_MAXIMUM as u128) as u32;
    // SAFETY: the platform window takes back the box. The timer id is the box address.
    unsafe {
        if millis == 0 {
            let _ = PostMessageW(
                Some(platform_window()),
                WM_LOCAL,
                WPARAM(0),
                LPARAM(raw as isize),
            );
        } else {
            SetTimer(Some(platform_window()), raw as usize, millis, None);
        }
    }
}

unsafe extern "system" fn platform_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: the boxes come from `post` and `run_after`, and each is taken back once.
    unsafe {
        match msg {
            WM_POST => Box::from_raw(lparam.0 as *mut Box<dyn FnOnce() + Send>)(),
            WM_LOCAL => Box::from_raw(lparam.0 as *mut Box<dyn FnOnce()>)(),
            WM_TIMER => {
                let _ = KillTimer(Some(hwnd), wparam.0);
                Box::from_raw(wparam.0 as *mut Box<dyn FnOnce()>)();
            }
            WM_QUERYENDSESSION => return LRESULT(1),
            WM_ENDSESSION if wparam.0 != 0 => {
                // Windows can end the process once this returns.
                crate::app::events::quit();
                std::process::exit(0);
            }
            _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
    LRESULT(0)
}

/// Paints `hwnd` on each display refresh until `stop_frames`.
fn start_frames(hwnd: HWND) {
    let mut wanted = WANTED.lock().unwrap();
    if !wanted.contains(&(hwnd.0 as isize)) {
        wanted.push(hwnd.0 as isize);
    }
    drop(wanted);
    VSYNC
        .get_or_init(|| {
            std::thread::Builder::new()
                .name("vsync".into())
                .spawn(vsync)
                .expect("the vsync thread must start")
                .thread()
                .clone()
        })
        .unpark();
}

fn stop_frames(hwnd: HWND) {
    WANTED
        .lock()
        .unwrap()
        .retain(|&wanted| wanted != hwnd.0 as isize);
}

/// Invalidates the windows that want frames after each composition, and sleeps while none do.
fn vsync() {
    let mut timing = DWM_TIMING_INFO {
        cbSize: size_of::<DWM_TIMING_INFO>() as u32,
        ..Default::default()
    };
    // SAFETY: plain DWM query.
    let interval = match unsafe { DwmGetCompositionTimingInfo(HWND::default(), &mut timing) } {
        Ok(()) if timing.rateRefresh.uiNumerator > 0 => Duration::from_secs_f64(
            timing.rateRefresh.uiDenominator as f64 / timing.rateRefresh.uiNumerator as f64,
        ),
        _ => Duration::from_micros(16_667),
    };
    let mut targets = Vec::new();
    loop {
        {
            let mut wanted = WANTED.lock().unwrap();
            // SAFETY: plain window queries. Minimized and hidden windows get frames again once shown.
            wanted.retain(|&hwnd| unsafe {
                IsWindowVisible(HWND(hwnd as _)).as_bool() && !IsIconic(HWND(hwnd as _)).as_bool()
            });
            targets.clone_from(&wanted);
        }
        if targets.is_empty() {
            std::thread::park();
            continue;
        }
        let start = Instant::now();
        // SAFETY: waits for the next composition.
        if unsafe { DwmFlush() }.is_err() || start.elapsed() < Duration::from_millis(1) {
            std::thread::sleep(interval);
        }
        for &hwnd in &targets {
            // SAFETY: invalidating a closed window fails harmlessly.
            let _ = unsafe { InvalidateRect(Some(HWND(hwnd as _)), None, false) };
        }
    }
}

/// The scale factor of a monitor.
fn monitor_scale(monitor: HMONITOR) -> f32 {
    let (mut x, mut y) = (USER_DEFAULT_SCREEN_DPI, USER_DEFAULT_SCREEN_DPI);
    // SAFETY: plain monitor query.
    let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y) };
    x as f32 / USER_DEFAULT_SCREEN_DPI as f32
}

fn logical_rect(rect: RECT, scale: f32) -> Bounds<Pixels> {
    Bounds::new(
        point(px(rect.left as f32 / scale), px(rect.top as f32 / scale)),
        size(
            px((rect.right - rect.left) as f32 / scale),
            px((rect.bottom - rect.top) as f32 / scale),
        ),
    )
}
