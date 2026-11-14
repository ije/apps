use super::super::native_sync;
use super::d3d::{Atlas, Renderer};
use super::{MAIN, keys, monitor_info, monitor_scale, native_post, on_main, points};
use super::{current_cursor, extent, run_after, setting, want_frames};
use crate::app::Callbacks;
use crate::{Button, Cursor, Event, Pixels, Point, Rect, Scene, Size, Timer, WheelPhase};
use crate::{WindowChange, WindowOptions, WindowState, point, px};
use anyhow::{Context as _, Result};
use raw_window_handle as rwh;
use std::cell::{Cell, RefCell};
use std::num::NonZeroIsize;
use std::ops::Deref;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::*;
use windows::Win32::System::SystemServices::*;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::Ime::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, HSTRING, PCWSTR, w};

static CLASS: AtomicU16 = AtomicU16::new(0);

// A window that stops drawing releases its layers after this.
const QUIET: Duration = Duration::from_secs(1);

struct Shared {
    hwnd: HWND,
    callbacks: Callbacks,
    chromeless: bool,
    scale: Cell<f32>,
    wants_frames: Cell<bool>,
    movable: Cell<bool>,
    closable: Cell<bool>,
    limits: Cell<(Size<Pixels>, Size<Pixels>)>,
    windowed: Cell<Option<(isize, RECT)>>,
    sizing_loop: Cell<bool>,
    resizing: Cell<bool>,
    tracking: Cell<bool>,
    pointer: Cell<Point<Pixels>>,
    button_down: Cell<Option<u32>>,
    ime: Cell<bool>,
    press: Cell<(Button, Instant, POINT, usize)>,
    surrogate: Cell<Option<u16>>,
    seen: keys::Seen,
    quiet: Cell<Option<Timer>>,
    drawing: RefCell<Renderer>,
}

/// Acts on the main thread, at once when called there.
#[derive(Clone)]
pub struct WindowControl {
    hwnd: isize,
}

pub(crate) struct Window {
    control: WindowControl,
    shared: Rc<Shared>,
    atlas: Rc<Atlas>,
}

impl Deref for Window {
    type Target = WindowControl;
    fn deref(&self) -> &WindowControl {
        &self.control
    }
}

fn shared_of(hwnd: isize) -> Option<Rc<Shared>> {
    let hwnd = HWND(hwnd as _);
    // SAFETY: only windows of our class on the main thread hold a `Shared` in their user data, which lives
    // until `WM_NCDESTROY` clears it.
    unsafe {
        if GetWindowThreadProcessId(hwnd, None) != MAIN.load(Ordering::Acquire)
            || GetClassLongPtrW(hwnd, GCW_ATOM) as u16 != CLASS.load(Ordering::Relaxed)
        {
            return None;
        }
        let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Shared;
        if raw.is_null() {
            return None;
        }
        Rc::increment_strong_count(raw);
        Some(Rc::from_raw(raw))
    }
}

fn register_class() -> Result<HINSTANCE> {
    // SAFETY: registers the window class once, on the main thread.
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        if CLASS.load(Ordering::Relaxed) == 0 {
            // The app icon is resource 1 of the executable.
            let icon = LoadIconW(Some(instance), PCWSTR(1 as _)).unwrap_or_default();
            let atom = RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                hIcon: icon,
                lpszClassName: w!("AppsWindow"),
                ..Default::default()
            });
            CLASS.store(atom, Ordering::Relaxed);
        }
        Ok(instance)
    }
}

impl Window {
    pub fn open(options: &WindowOptions, callbacks: Callbacks) -> Result<Self> {
        let instance = register_class()?;
        let mut style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
        if options.resizable {
            style |= WS_THICKFRAME | WS_MAXIMIZEBOX;
        }
        if options.minimizable {
            style |= WS_MINIMIZEBOX;
        }
        // SAFETY: window setup on the main thread. The window holds a strong count of its state until
        // `WM_NCDESTROY`.
        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_APPWINDOW | WS_EX_NOREDIRECTIONBITMAP,
                w!("AppsWindow"),
                &HSTRING::from(options.title.as_str()),
                style,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                None,
                None,
                Some(instance),
                None,
            )?;
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            let renderer = Renderer::new(
                hwnd,
                [client.right as u32, client.bottom as u32],
                options.transparent,
            );
            let renderer = match renderer {
                Ok(renderer) => renderer,
                Err(error) => {
                    let _ = DestroyWindow(hwnd);
                    return Err(error);
                }
            };
            let atlas = renderer.atlas().clone();
            let shared = Rc::new(Shared {
                hwnd,
                callbacks,
                chromeless: options.titlebar_hidden || options.frameless,
                scale: Cell::new(GetDpiForWindow(hwnd) as f32 / USER_DEFAULT_SCREEN_DPI as f32),
                wants_frames: Cell::new(false),
                movable: Cell::new(options.movable),
                closable: Cell::new(true),
                limits: Cell::default(),
                windowed: Cell::new(None),
                sizing_loop: Cell::new(false),
                resizing: Cell::new(false),
                tracking: Cell::new(false),
                pointer: Cell::default(),
                button_down: Cell::new(None),
                ime: Cell::new(true),
                press: Cell::new((Button::Left, Instant::now(), POINT::default(), 0)),
                surrogate: Cell::new(None),
                seen: Cell::new(None),
                quiet: Cell::new(None),
                drawing: RefCell::new(renderer),
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Rc::into_raw(shared.clone()) as isize);
            if shared.chromeless {
                frame_changed(hwnd);
            }
            polite(hwnd, true);
            DragAcceptFiles(hwnd, true);
            follow_theme(hwnd);
            let primary = MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY);
            let bounds = options.bounds.scale(monitor_scale(primary));
            let [left, top, right, bottom] = match options.outer_size {
                true => [0; 4],
                false => insets(hwnd),
            };
            let [x, y, width, height] = bounds.to_array().map(|value| value.round() as i32);
            let outer = [x, y, width + left + right, height + top + bottom];
            reposition(hwnd, outer, SET_WINDOW_POS_FLAGS::default());
            if options.show {
                let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            }
            Ok(Self {
                control: WindowControl {
                    hwnd: hwnd.0 as isize,
                },
                shared,
                atlas,
            })
        }
    }

    pub(crate) fn control(&self) -> &WindowControl {
        &self.control
    }

    pub fn atlas(&self) -> &Rc<Atlas> {
        &self.atlas
    }

    pub fn scale_factor(&self) -> f32 {
        self.shared.scale.get()
    }

    pub fn content_size(&self) -> Size<Pixels> {
        self.shared.client().size
    }

    pub fn request_frame(&self) {
        let shared = &self.shared;
        shared.quiet.take();
        shared.wants_frames.set(true);
        want_frames(shared.hwnd, true);
    }

    pub fn force_frame(&self) {
        let shared = Rc::downgrade(&self.shared);
        run_after(Duration::ZERO, move || {
            if let Some(shared) = shared.upgrade() {
                shared.frame();
            }
        });
    }

    pub fn take_snapshots(&self) -> Vec<crate::Snapshot> {
        match self.shared.drawing.try_borrow_mut() {
            Ok(mut renderer) => std::mem::take(&mut renderer.snapshots),
            Err(_) => Vec::new(),
        }
    }

    pub fn draw(&self, scene: &Scene, capture: bool) -> Option<Result<([u32; 2], Vec<u8>)>> {
        self.shared.quiet.take();
        match self.shared.drawing.try_borrow_mut() {
            Ok(mut renderer) => renderer.draw(scene, capture),
            Err(_) => capture.then(|| Err(anyhow::anyhow!("The window is busy"))),
        }
    }

    pub fn release(&self) {
        if let Ok(mut renderer) = self.shared.drawing.try_borrow_mut() {
            renderer.release();
        }
    }

    pub fn state(&self) -> WindowState {
        self.shared.state()
    }

    pub fn change(&self, change: WindowChange) {
        self.shared.apply(change);
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: the window procedure drops the window's count of the state at `WM_NCDESTROY`.
        let _ = unsafe { DestroyWindow(self.shared.hwnd) };
    }
}

// Carries a change to the main thread. Its only handle is a window handle number.
struct Handed(WindowChange);
// SAFETY: as above, and a handle number means the same window on any thread.
unsafe impl Send for Handed {}

impl WindowControl {
    fn act(&self, f: impl FnOnce(&Shared) + Send + 'static) {
        let hwnd = self.hwnd;
        let run = move || {
            if let Some(shared) = shared_of(hwnd) {
                f(&shared);
            }
        };
        match on_main() {
            true => run(),
            false => native_post(Box::new(run)),
        }
    }

    fn query<R: Send>(&self, f: impl FnOnce(&Shared) -> R + Send) -> Option<R> {
        let hwnd = self.hwnd;
        native_sync(move || shared_of(hwnd).map(|shared| f(&shared)))
    }

    pub fn change(&self, change: WindowChange) {
        let handed = Handed(change);
        self.act(move |shared| {
            let handed = handed;
            shared.apply(handed.0)
        });
    }

    pub fn set_title(&self, title: &str) {
        let title = HSTRING::from(title);
        // SAFETY: a plain window call.
        self.act(move |shared| unsafe {
            let _ = SetWindowTextW(shared.hwnd, &title);
        });
    }

    pub fn set_button_visibility(&self, _: bool) {}

    pub fn set_button_position(&self, _: Option<Point<Pixels>>) {}

    pub fn minimize(&self) {
        // SAFETY: a plain window call, which runs after the current message.
        self.act(|shared| unsafe {
            let _ = ShowWindowAsync(shared.hwnd, SW_MINIMIZE);
        });
    }

    pub fn zoom(&self) {
        self.act(|shared| shared.zoom());
    }

    pub fn titlebar_double_click(&self) {
        self.zoom();
    }

    pub fn toggle_fullscreen(&self) {
        self.act(|shared| shared.switch_fullscreen());
    }

    pub fn activate(&self) {
        self.act(|shared| shared.activate());
    }

    pub fn resize(&self, content: Size<Pixels>) {
        self.act(move |shared| {
            let size = content.scale(shared.scale.get());
            let [left, top, right, bottom] = insets(shared.hwnd);
            let [width, height] = [size.width, size.height].map(|value| value.round() as i32);
            let outer = [0, 0, width + left + right, height + top + bottom];
            reposition(shared.hwnd, outer, SWP_NOMOVE);
        });
    }

    pub fn start_window_move(&self) {
        // SAFETY: the posted command moves the window with the pressed button.
        self.act(|shared| unsafe {
            let _ = ReleaseCapture();
            let _ = PostMessageW(
                Some(shared.hwnd),
                WM_SYSCOMMAND,
                WPARAM((SC_MOVE | HTCAPTION) as usize),
                LPARAM(0),
            );
        });
    }

    pub fn is_active(&self) -> bool {
        self.query(|shared| shared.focused()).unwrap_or(false)
    }

    pub fn state(&self) -> Option<WindowState> {
        self.query(|shared| shared.state())
    }

    pub fn raw_window_handle(&self) -> Result<rwh::RawWindowHandle> {
        let hwnd = NonZeroIsize::new(self.hwnd).context("the window is closed")?;
        Ok(rwh::RawWindowHandle::Win32(rwh::Win32WindowHandle::new(
            hwnd,
        )))
    }
}

pub(super) fn cursor(cursor: Cursor) -> Option<HCURSOR> {
    let name = match cursor {
        Cursor::None => return None,
        Cursor::Text | Cursor::VerticalText => IDC_IBEAM,
        Cursor::Pointer | Cursor::Grab | Cursor::Grabbing | Cursor::Alias => IDC_HAND,
        Cursor::Crosshair => IDC_CROSS,
        Cursor::NotAllowed => IDC_NO,
        Cursor::ColResize | Cursor::EwResize | Cursor::EResize | Cursor::WResize => IDC_SIZEWE,
        Cursor::RowResize | Cursor::NsResize | Cursor::NResize | Cursor::SResize => IDC_SIZENS,
        Cursor::NwseResize => IDC_SIZENWSE,
        Cursor::NeswResize => IDC_SIZENESW,
        Cursor::Default | Cursor::Copy | Cursor::ContextMenu => IDC_ARROW,
    };
    // SAFETY: system cursors are shared and never freed.
    unsafe { LoadCursorW(None, name) }.ok()
}

pub(super) fn key_down(message: &MSG) -> bool {
    let Some(shared) = shared_of(message.hwnd.0 as isize) else {
        return false;
    };
    let Some(event) = keys::key(&shared.seen, message.wParam, message.lParam, true) else {
        return false;
    };
    let callbacks = shared.callbacks;
    let taken = crate::native::key(callbacks.id, &event, &|| callbacks.composition().is_some())
        || callbacks.input(event);
    // The input method takes its keys in TranslateMessage, so those always go on.
    taken && message.wParam.0 as u16 != VK_PROCESSKEY.0
}

// The width of the frame edges on each side: left, top, right, bottom.
fn insets(hwnd: HWND) -> [i32; 4] {
    let (mut frame, mut client, mut origin) = (RECT::default(), RECT::default(), POINT::default());
    // SAFETY: plain window queries into locals.
    unsafe {
        let _ = GetWindowRect(hwnd, &mut frame);
        let _ = GetClientRect(hwnd, &mut client);
        let _ = ClientToScreen(hwnd, &mut origin);
    }
    [
        origin.x - frame.left,
        origin.y - frame.top,
        frame.right - origin.x - client.right,
        frame.bottom - origin.y - client.bottom,
    ]
}

fn border(hwnd: HWND) -> (i32, i32) {
    // SAFETY: plain metric queries.
    unsafe {
        let dpi = GetDpiForWindow(hwnd);
        let padding = GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
        (
            GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi) + padding,
            GetSystemMetricsForDpi(SM_CYSIZEFRAME, dpi) + padding,
        )
    }
}

fn frame_changed(hwnd: HWND) {
    reposition(hwnd, [0; 4], SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE);
}

fn reposition(hwnd: HWND, [x, y, width, height]: [i32; 4], also: SET_WINDOW_POS_FLAGS) {
    let flags = also | SWP_NOZORDER | SWP_NOACTIVATE;
    // SAFETY: a plain window call.
    let _ = unsafe { SetWindowPos(hwnd, None, x, y, width, height, flags) };
}

// Without this mark, a window without a title bar can keep the taskbar above it. Fullscreen windows go
// without it, so the taskbar goes below them.
fn polite(hwnd: HWND, on: bool) {
    let name = w!("NonRudeHWND");
    // SAFETY: a plain window property, which holds a number rather than a handle.
    unsafe {
        let _ = match on {
            true => SetPropW(hwnd, name, Some(HANDLE(1 as _))),
            false => RemovePropW(hwnd, name).map(drop),
        };
    }
}

fn follow_theme(hwnd: HWND) {
    let (mut light, mut size) = (1u32, 4u32);
    // SAFETY: the value is a DWORD read into a local, and the attribute is a BOOL.
    unsafe {
        let read = RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut light).cast()),
            Some(&mut size),
        );
        let dark = BOOL::from(read == ERROR_SUCCESS && light == 0);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&raw const dark).cast(),
            size_of::<BOOL>() as u32,
        );
    }
}

fn word(value: isize) -> POINT {
    POINT {
        x: value as i16 as i32,
        y: (value >> 16) as i16 as i32,
    }
}

impl Shared {
    fn points(&self, at: POINT) -> Point<Pixels> {
        let scale = self.scale.get();
        point(px(at.x as f32 / scale), px(at.y as f32 / scale))
    }

    // A point of the message in screen pixels, from the top left of the content.
    fn screen_point(&self, lparam: LPARAM) -> POINT {
        let mut at = word(lparam.0);
        // SAFETY: a plain window query into a local.
        let _ = unsafe { ScreenToClient(self.hwnd, &mut at) };
        at
    }

    fn client(&self) -> Rect {
        let (mut client, mut origin) = (RECT::default(), POINT::default());
        // SAFETY: plain window queries into locals.
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut client);
            let _ = ClientToScreen(self.hwnd, &mut origin);
        }
        let rect = RECT {
            left: origin.x,
            top: origin.y,
            right: origin.x + client.right,
            bottom: origin.y + client.bottom,
        };
        points(rect, self.scale.get())
    }

    fn style(&self) -> isize {
        // SAFETY: a plain window query.
        unsafe { GetWindowLongPtrW(self.hwnd, GWL_STYLE) }
    }

    fn has(&self, flag: WINDOW_STYLE) -> bool {
        self.style() & flag.0 as isize != 0
    }

    fn set_style(&self, flag: WINDOW_STYLE, on: bool) {
        let style = match on {
            true => self.style() | flag.0 as isize,
            false => self.style() & !(flag.0 as isize),
        };
        // SAFETY: a plain window call. The frame change applies the style.
        unsafe { SetWindowLongPtrW(self.hwnd, GWL_STYLE, style) };
        frame_changed(self.hwnd);
    }

    fn zoomed(&self) -> bool {
        // SAFETY: a plain window query.
        unsafe { IsZoomed(self.hwnd) }.as_bool()
    }

    fn focused(&self) -> bool {
        // SAFETY: a plain query of this thread's active window.
        unsafe { GetActiveWindow() == self.hwnd }
    }

    fn state(&self) -> WindowState {
        let hwnd = self.hwnd;
        let scale = self.scale.get();
        let mut frame = RECT::default();
        // SAFETY: plain window and monitor queries into locals.
        unsafe {
            let _ = GetWindowRect(hwnd, &mut frame);
            let monitor =
                monitor_info(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST)).unwrap_or_default();
            let fullscreen = self.windowed.get().is_some();
            WindowState {
                frame: points(frame, scale),
                content: self.client(),
                minimized: IsIconic(hwnd).as_bool(),
                maximized: self.zoomed() && !fullscreen,
                fullscreen,
                visible: IsWindowVisible(hwnd).as_bool(),
                resizing: self.resizing.get(),
                display: points(monitor.rcMonitor, scale),
                work_area: points(monitor.rcWork, scale),
            }
        }
    }

    fn apply(&self, change: WindowChange) {
        let hwnd = self.hwnd;
        // SAFETY: plain window calls.
        let place = |after: HWND| unsafe {
            let _ = SetWindowPos(
                hwnd,
                Some(after),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        };
        // SAFETY: plain window calls with the documented arguments.
        unsafe {
            match change {
                WindowChange::Frame(bounds) => {
                    let pixels = bounds.scale(self.scale.get()).to_array();
                    reposition(
                        hwnd,
                        pixels.map(|value| value.round() as i32),
                        SET_WINDOW_POS_FLAGS::default(),
                    );
                }
                WindowChange::Center => {
                    let mut frame = RECT::default();
                    if let Some(info) =
                        monitor_info(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST))
                        && GetWindowRect(hwnd, &mut frame).is_ok()
                    {
                        let work = info.rcWork;
                        let x = (work.left + work.right - (frame.right - frame.left)) / 2;
                        let y = (work.top + work.bottom - (frame.bottom - frame.top)) / 2;
                        reposition(hwnd, [x, y, 0, 0], SWP_NOSIZE);
                    }
                }
                WindowChange::Show(true) => self.activate(),
                WindowChange::Show(false) => {
                    let _ = ShowWindow(hwnd, SW_SHOWNA);
                }
                WindowChange::Hide => {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
                WindowChange::Blur => place(HWND_BOTTOM),
                WindowChange::Restore => {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                }
                WindowChange::SizeLimits(min, max) => self.limits.set((min, max)),
                WindowChange::AlwaysOnTop(level) => place(if level != 0 {
                    HWND_TOPMOST
                } else {
                    HWND_NOTOPMOST
                }),
                WindowChange::Resizable(on) => self.set_style(WS_THICKFRAME, on),
                WindowChange::Minimizable(on) => self.set_style(WS_MINIMIZEBOX, on),
                WindowChange::Maximizable(on) => self.set_style(WS_MAXIMIZEBOX, on),
                WindowChange::Closable(on) => {
                    self.closable.set(on);
                    let state = if on { MF_ENABLED } else { MF_GRAYED };
                    let _ =
                        EnableMenuItem(GetSystemMenu(hwnd, false), SC_CLOSE, MF_BYCOMMAND | state);
                }
                WindowChange::Movable(on) => self.movable.set(on),
                WindowChange::Background(color) => {
                    if let Ok(mut renderer) = self.drawing.try_borrow_mut() {
                        renderer.set_background(color);
                    }
                }
                WindowChange::ContentProtection(on) => {
                    let _ = SetWindowDisplayAffinity(
                        hwnd,
                        if on { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE },
                    );
                }
                WindowChange::Parent(parent, _) => {
                    let owner = match parent {
                        Some(rwh::RawWindowHandle::Win32(handle)) => handle.hwnd.get(),
                        _ => 0,
                    };
                    SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, owner);
                }
                WindowChange::CancelComposition => {
                    let context = ImmGetContext(hwnd);
                    if !context.is_invalid() {
                        let _ = ImmNotifyIME(context, NI_COMPOSITIONSTR, CPS_CANCEL, 0);
                    }
                    let _ = ImmReleaseContext(hwnd, context);
                    return;
                }
                // Not supported on Windows; the runtime rejects them.
                WindowChange::Opacity(_)
                | WindowChange::Shadow(_)
                | WindowChange::IgnoreMouse(..)
                | WindowChange::AllWorkspaces(..)
                | WindowChange::Focusable(_)
                | WindowChange::AspectRatio(..)
                | WindowChange::Vibrancy(..)
                | WindowChange::Edited(_) => {}
            }
        }
        self.callbacks.changed();
    }

    fn zoom(&self) {
        let show = if self.zoomed() {
            SW_RESTORE
        } else {
            SW_MAXIMIZE
        };
        // SAFETY: a plain window call, which runs after the current message.
        let _ = unsafe { ShowWindowAsync(self.hwnd, show) };
    }

    fn switch_fullscreen(&self) {
        let hwnd = self.hwnd;
        // `windowed` changes before the frame does, since WM_NCCALCSIZE reads it.
        let (style, rect) = match self.windowed.take() {
            Some(windowed) => windowed,
            None => {
                let mut frame = RECT::default();
                // SAFETY: a plain window query into a local.
                let _ = unsafe { GetWindowRect(hwnd, &mut frame) };
                // SAFETY: a plain monitor query.
                let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
                let Some(info) = monitor_info(monitor) else {
                    return;
                };
                let kept = self.style();
                self.windowed.set(Some((kept, frame)));
                let chrome =
                    WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MAXIMIZEBOX | WS_MINIMIZEBOX;
                (kept & !(chrome.0 as isize), info.rcMonitor)
            }
        };
        // A fullscreen window is rude, so the taskbar goes behind it.
        polite(hwnd, self.windowed.get().is_none());
        // SAFETY: a plain window call. The frame change below applies the style.
        unsafe { SetWindowLongPtrW(hwnd, GWL_STYLE, style) };
        reposition(hwnd, extent(rect), SWP_FRAMECHANGED);
        self.callbacks.changed();
    }

    fn activate(&self) {
        let hwnd = self.hwnd;
        // SAFETY: plain window and input calls.
        unsafe {
            let show = match (IsIconic(hwnd).as_bool(), IsWindowVisible(hwnd).as_bool()) {
                (true, _) => Some(SW_RESTORE),
                (false, false) => Some(SW_SHOW),
                (false, true) => None,
            };
            if let Some(show) = show {
                let _ = ShowWindow(hwnd, show);
            }
            if GetForegroundWindow() == hwnd {
                return;
            }
            // Windows lets a process take the foreground only after input, so an Alt press comes first.
            let alt = |flags| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_MENU,
                        dwFlags: flags,
                        ..Default::default()
                    },
                },
            };
            SendInput(
                &[alt(KEYBD_EVENT_FLAGS(0)), alt(KEYEVENTF_KEYUP)],
                size_of::<INPUT>() as i32,
            );
            let _ = SetForegroundWindow(hwnd);
        }
    }

    // False when the app was busy and drew nothing.
    fn frame(&self) -> bool {
        let Some(more) = self.callbacks.frame() else {
            return false;
        };
        if !more {
            self.wants_frames.set(false);
            want_frames(self.hwnd, false);
            let callbacks = self.callbacks;
            self.quiet
                .set(Some(crate::app::timer(QUIET, move |_| callbacks.release())));
        }
        true
    }

    fn press(&self, button: Button, at: POINT) -> usize {
        let (last, when, place, count) = self.press.get();
        // SAFETY: plain system queries.
        let again = unsafe {
            last == button
                && when.elapsed() <= Duration::from_millis(u64::from(GetDoubleClickTime()))
                && (at.x - place.x).abs() <= GetSystemMetrics(SM_CXDOUBLECLK)
                && (at.y - place.y).abs() <= GetSystemMetrics(SM_CYDOUBLECLK)
        };
        let count = if again { count + 1 } else { 1 };
        self.press.set((button, Instant::now(), at, count));
        count
    }

    fn track(&self, flags: TRACKMOUSEEVENT_FLAGS) {
        if self.tracking.replace(true) {
            return;
        }
        let mut request = TRACKMOUSEEVENT {
            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: flags,
            hwndTrack: self.hwnd,
            dwHoverTime: 0,
        };
        // SAFETY: a sized request for this window.
        let _ = unsafe { TrackMouseEvent(&mut request) };
    }

    fn place_ime(&self) {
        let callbacks = self.callbacks;
        let Some(caret) = callbacks
            .selection()
            .and_then(|selection| callbacks.rect_of(selection.range))
        else {
            return;
        };
        let scale = self.scale.get();
        let at = POINT {
            x: (caret.origin.x.0 * scale) as i32,
            y: ((caret.origin.y.0 + caret.size.height.0 / 2.) * scale) as i32,
        };
        // SAFETY: the context is released after use, and the forms are locals.
        unsafe {
            let context = ImmGetContext(self.hwnd);
            if context.is_invalid() {
                return;
            }
            let _ = ImmSetCompositionWindow(
                context,
                &COMPOSITIONFORM {
                    dwStyle: CFS_POINT,
                    ptCurrentPos: at,
                    ..Default::default()
                },
            );
            let _ = ImmSetCandidateWindow(
                context,
                &CANDIDATEFORM {
                    dwStyle: CFS_CANDIDATEPOS,
                    ptCurrentPos: at,
                    ..Default::default()
                },
            );
            let _ = ImmReleaseContext(self.hwnd, context);
        }
    }

    // The input method works only while the page has an editable focus.
    fn sync_ime(&self) {
        let enabled = self.callbacks.accepts_text_input();
        if enabled == self.ime.replace(enabled) {
            return;
        }
        let hwnd = self.hwnd;
        // SAFETY: plain input method calls, and the context is released after use.
        unsafe {
            if enabled {
                let _ = ImmAssociateContextEx(hwnd, HIMC::default(), IACE_DEFAULT);
                return;
            }
            // The context belongs to the thread, so only a composition in this window ends.
            let context = ImmGetContext(hwnd);
            if GetFocus() == hwnd && !context.is_invalid() {
                let _ = ImmNotifyIME(context, NI_COMPOSITIONSTR, CPS_COMPLETE, 0);
            }
            let _ = ImmReleaseContext(hwnd, context);
            let _ = ImmAssociateContextEx(hwnd, HIMC::default(), 0);
        }
    }

    fn composition(&self, flags: u32) -> Option<isize> {
        let hwnd = self.hwnd;
        // SAFETY: the context is released after use.
        let context = unsafe { ImmGetContext(hwnd) };
        if context.is_invalid() {
            return None;
        }
        let callbacks = self.callbacks;
        if flags & GCS_RESULTSTR.0 != 0
            && let Some(text) = keys::composition_text(context, GCS_RESULTSTR)
        {
            callbacks.insert(None, &String::from_utf16_lossy(&text));
        }
        // A message without flags cancels the composition.
        let composing = match flags {
            0 => Some(Vec::new()),
            _ if flags & GCS_COMPSTR.0 != 0 => keys::composition_text(context, GCS_COMPSTR),
            _ => None,
        };
        match composing {
            Some(text) if !text.is_empty() => {
                let selected = (flags & GCS_CURSORPOS.0 != 0).then(|| {
                    let at = keys::caret(context, text.len());
                    at..at
                });
                callbacks.compose(None, &String::from_utf16_lossy(&text), selected);
            }
            Some(_) => callbacks.clear_composition(),
            None => {}
        }
        // SAFETY: releases the context taken above.
        let _ = unsafe { ImmReleaseContext(hwnd, context) };
        self.place_ime();
        (flags == 0 || flags & (GCS_RESULTSTR.0 | GCS_COMPSTR.0) != 0).then_some(0)
    }

    // Characters come as UTF-16 units, so a surrogate pair waits for its second half.
    fn typed(&self, unit: u16) {
        let units = match self.surrogate.take() {
            Some(high) if (0xDC00..=0xDFFF).contains(&unit) => vec![high, unit],
            _ if (0xD800..=0xDBFF).contains(&unit) => {
                self.surrogate.set(Some(unit));
                return;
            }
            _ => vec![unit],
        };
        let Ok(text) = String::from_utf16(&units) else {
            return;
        };
        if text.chars().any(char::is_control) {
            return;
        }
        self.callbacks.insert(None, &text);
    }

    fn wheel(&self, msg: u32, wparam: WPARAM, lparam: LPARAM) {
        let modifiers = keys::held();
        let sideways = msg == WM_MOUSEHWHEEL || modifiers.shift();
        let mut per_notch = 3u32;
        let action = if sideways {
            SPI_GETWHEELSCROLLCHARS
        } else {
            SPI_GETWHEELSCROLLLINES
        };
        setting(action, &mut per_notch);
        let notches = f32::from((wparam.0 >> 16) as i16) / WHEEL_DELTA as f32;
        // A tilt to the right scrolls right, which is the opposite sign of a wheel turn.
        let lines = notches * per_notch as f32 * if msg == WM_MOUSEHWHEEL { -1. } else { 1. };
        self.callbacks.input(Event::Wheel {
            at: self.points(self.screen_point(lparam)),
            delta: if sideways {
                point(lines, 0.)
            } else {
                point(0., lines)
            },
            lines: true,
            phase: WheelPhase::None,
            modifiers,
        });
    }

    fn files(&self, drop: HDROP) {
        // SAFETY: the drop handle is valid until DragFinish, and each name fills a buffer of its length.
        let (paths, mut at) = unsafe {
            let count = DragQueryFileW(drop, u32::MAX, None);
            let paths = (0..count)
                .filter_map(|index| {
                    let mut name = vec![0u16; DragQueryFileW(drop, index, None) as usize + 1];
                    let len = DragQueryFileW(drop, index, Some(&mut name)) as usize;
                    (len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&name[..len])))
                })
                .collect();
            (paths, POINT::default())
        };
        // SAFETY: as above, and the handle is freed last.
        unsafe {
            let _ = DragQueryPoint(drop, &mut at);
            DragFinish(drop);
        }
        let at = self.points(at);
        self.callbacks.input(Event::FileEnter { at, paths });
        self.callbacks.input(Event::FileDrop { at });
    }

    // `None` leaves the message to the default handling.
    fn message(&self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<isize> {
        let hwnd = self.hwnd;
        let callbacks = self.callbacks;
        let scale = self.scale.get();
        let button = || match msg {
            WM_LBUTTONDOWN | WM_LBUTTONUP | WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK
            | WM_NCLBUTTONUP => Some(Button::Left),
            WM_RBUTTONDOWN | WM_RBUTTONUP | WM_NCRBUTTONDOWN | WM_NCRBUTTONUP => {
                Some(Button::Right)
            }
            WM_MBUTTONDOWN | WM_MBUTTONUP | WM_NCMBUTTONDOWN | WM_NCMBUTTONUP => {
                Some(Button::Middle)
            }
            _ => match (wparam.0 >> 16) as u16 {
                XBUTTON1 => Some(Button::Back),
                XBUTTON2 => Some(Button::Forward),
                _ => None,
            },
        };
        // SAFETY: each message carries the documented parameters, and no borrow of the state lasts across a
        // window call.
        unsafe {
            match msg {
                WM_MOUSEACTIVATE => Some(MA_ACTIVATE as isize),
                WM_ACTIVATE => {
                    if u32::from(wparam.0 as u16) != WA_INACTIVE {
                        // Alt-tab can leave Alt held for us, since its release went to another window.
                        let now = (keys::held(), keys::caps_lock());
                        self.seen.set(Some(now));
                        callbacks.input(Event::ModifiersChanged { modifiers: now.0 });
                    }
                    callbacks.changed();
                    None
                }
                WM_MOVE => {
                    if !self.sizing_loop.get() {
                        callbacks.changed();
                    }
                    Some(0)
                }
                WM_ENTERSIZEMOVE => {
                    self.sizing_loop.set(true);
                    None
                }
                WM_EXITSIZEMOVE => {
                    self.sizing_loop.set(false);
                    self.resizing.set(false);
                    callbacks.changed();
                    None
                }
                WM_SIZING => {
                    self.resizing.set(true);
                    None
                }
                WM_SIZE if wparam.0 == SIZE_MINIMIZED as usize => {
                    callbacks.changed();
                    Some(0)
                }
                WM_SIZE => {
                    let pixels = [lparam.0 as u32 & 0xFFFF, (lparam.0 as u32 >> 16) & 0xFFFF];
                    if let Ok(mut renderer) = self.drawing.try_borrow_mut()
                        && let Err(error) = renderer.resize(pixels)
                    {
                        eprintln!("apps: resizing the swap chain: {error:#}");
                    }
                    callbacks.resized(
                        crate::size(px(pixels[0] as f32), px(pixels[1] as f32)) / scale,
                        scale,
                    );
                    if self.wants_frames.get() {
                        want_frames(hwnd, true);
                    }
                    // Live resizing draws at once, so the content keeps up with the frame.
                    self.frame();
                    Some(0)
                }
                WM_WINDOWPOSCHANGED => {
                    let flags = (*(lparam.0 as *const WINDOWPOS)).flags;
                    let shown = flags.contains(SWP_SHOWWINDOW);
                    if shown && self.wants_frames.get() {
                        want_frames(hwnd, true);
                    }
                    if shown || flags.contains(SWP_HIDEWINDOW) {
                        callbacks.changed();
                    }
                    None
                }
                WM_GETMINMAXINFO => {
                    let (min, max) = self.limits.get();
                    let [left, top, right, bottom] = insets(hwnd);
                    let info = &mut *(lparam.0 as *mut MINMAXINFO);
                    let side =
                        |length: Pixels, inset: i32| (length.0 * scale).round() as i32 + inset;
                    if min.width.0 > 0. || min.height.0 > 0. {
                        let track = &mut info.ptMinTrackSize;
                        track.x = track.x.max(side(min.width, left + right));
                        track.y = track.y.max(side(min.height, top + bottom));
                    }
                    if max.width.0 > 0. {
                        info.ptMaxTrackSize.x = side(max.width, left + right);
                    }
                    if max.height.0 > 0. {
                        info.ptMaxTrackSize.y = side(max.height, top + bottom);
                    }
                    Some(0)
                }
                // Without a title bar the client area starts at the top edge. A maximized window hangs its
                // border off the display, so its content moves down by that much.
                WM_NCCALCSIZE
                    if self.chromeless && self.windowed.get().is_none() && wparam.0 != 0 =>
                {
                    let params = &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS);
                    let top = params.rgrc[0].top;
                    let result = DefWindowProcW(hwnd, msg, wparam, lparam);
                    params.rgrc[0].top = top + if self.zoomed() { border(hwnd).1 } else { 0 };
                    Some(result.0)
                }
                WM_DPICHANGED => {
                    let scale = f32::from(wparam.0 as u16) / USER_DEFAULT_SCREEN_DPI as f32;
                    self.scale.set(scale);
                    let mut rect = *(lparam.0 as *const RECT);
                    if self.zoomed()
                        && let Some(info) =
                            monitor_info(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST))
                    {
                        rect = info.rcWork;
                    }
                    reposition(hwnd, extent(rect), SET_WINDOW_POS_FLAGS::default());
                    callbacks.resized(self.client().size, scale);
                    Some(0)
                }
                WM_NCHITTEST => {
                    if self.windowed.get().is_some() {
                        return None;
                    }
                    let at = self.screen_point(lparam);
                    let drag = (self.movable.get() && callbacks.in_drag_area(self.points(at)))
                        .then_some(HTCAPTION as isize);
                    if !self.chromeless {
                        return drag;
                    }
                    // The default hit test has no top resize edge once the title bar is gone.
                    let (side, top) = border(hwnd);
                    if !self.has(WS_THICKFRAME) || self.zoomed() || !(0..=top).contains(&at.y) {
                        return drag;
                    }
                    let mut frame = RECT::default();
                    let _ = GetWindowRect(hwnd, &mut frame);
                    let edge = if at.x <= 0 {
                        HTTOPLEFT
                    } else if at.x >= frame.right - frame.left - 1 - 2 * side {
                        HTTOPRIGHT
                    } else {
                        HTTOP
                    };
                    Some(edge as isize)
                }
                WM_PAINT => {
                    if self.frame() {
                        self.sync_ime();
                    }
                    let _ = ValidateRect(Some(hwnd), None);
                    Some(0)
                }
                WM_ERASEBKGND => Some(1),
                // Main decides about closing and closes the window itself.
                WM_CLOSE => {
                    if self.closable.get() {
                        callbacks.should_close();
                    }
                    Some(0)
                }
                WM_DESTROY => {
                    want_frames(hwnd, false);
                    Some(0)
                }
                WM_SYSCOMMAND => {
                    let command = wparam.0 as u32 & 0xFFF0;
                    let blocked = command == SC_MOVE && !self.movable.get()
                        || command == SC_CLOSE && !self.closable.get();
                    blocked.then_some(0)
                }
                WM_MOUSEMOVE => {
                    self.track(TME_LEAVE);
                    let flags = MODIFIERKEYS_FLAGS(wparam.0 as u32);
                    let held = [
                        (MK_LBUTTON, Button::Left),
                        (MK_RBUTTON, Button::Right),
                        (MK_MBUTTON, Button::Middle),
                        (MK_XBUTTON1, Button::Back),
                        (MK_XBUTTON2, Button::Forward),
                    ]
                    .into_iter()
                    .find_map(|(flag, button)| flags.contains(flag).then_some(button));
                    let at = self.points(word(lparam.0));
                    self.pointer.set(at);
                    callbacks.input(Event::PointerMove {
                        at,
                        held,
                        modifiers: keys::held(),
                    });
                    Some(0)
                }
                WM_NCMOUSEMOVE => {
                    self.track(TME_LEAVE | TME_NONCLIENT);
                    let at = self.points(self.screen_point(lparam));
                    self.pointer.set(at);
                    let taken = callbacks.input(Event::PointerMove {
                        at,
                        held: None,
                        modifiers: keys::held(),
                    });
                    taken.then_some(0)
                }
                WM_MOUSELEAVE | WM_NCMOUSELEAVE => {
                    self.tracking.set(false);
                    callbacks.input(Event::PointerLeave {
                        at: self.pointer.get(),
                    });
                    Some(0)
                }
                WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN => {
                    let button = button()?;
                    SetCapture(hwnd);
                    let at = word(lparam.0);
                    callbacks.input(Event::PointerDown {
                        at: self.points(at),
                        button,
                        modifiers: keys::held(),
                        clicks: self.press(button, at),
                    });
                    // X buttons report that they were handled.
                    Some(isize::from(msg == WM_XBUTTONDOWN))
                }
                WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP | WM_XBUTTONUP => {
                    let button = button()?;
                    let _ = ReleaseCapture();
                    callbacks.input(Event::PointerUp {
                        at: self.points(word(lparam.0)),
                        button,
                        modifiers: keys::held(),
                        clicks: self.press.get().3,
                    });
                    Some(isize::from(msg == WM_XBUTTONUP))
                }
                // The caption moves the window, as the drag areas of the page ask.
                WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK | WM_NCRBUTTONDOWN | WM_NCMBUTTONDOWN
                | WM_NCLBUTTONUP | WM_NCRBUTTONUP | WM_NCMBUTTONUP
                    if wparam.0 as u32 == HTCAPTION =>
                {
                    None
                }
                WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK | WM_NCRBUTTONDOWN | WM_NCMBUTTONDOWN => {
                    let button = button()?;
                    let at = self.screen_point(lparam);
                    let taken = callbacks.input(Event::PointerDown {
                        at: self.points(at),
                        button,
                        modifiers: keys::held(),
                        clicks: self.press(button, at),
                    });
                    let hit = wparam.0 as u32;
                    if taken {
                        return Some(0);
                    }
                    // The buttons act on release, like the default ones.
                    let title_button = matches!(hit, HTMINBUTTON | HTMAXBUTTON | HTCLOSE);
                    (button == Button::Left && title_button).then(|| {
                        self.button_down.set(Some(hit));
                        0
                    })
                }
                WM_NCLBUTTONUP | WM_NCRBUTTONUP | WM_NCMBUTTONUP => {
                    let button = button()?;
                    let taken = callbacks.input(Event::PointerUp {
                        at: self.points(self.screen_point(lparam)),
                        button,
                        modifiers: keys::held(),
                        clicks: 1,
                    });
                    if taken {
                        return Some(0);
                    }
                    let hit = wparam.0 as u32;
                    if button != Button::Left || self.button_down.take() != Some(hit) {
                        return None;
                    }
                    match hit {
                        HTMINBUTTON if self.has(WS_MINIMIZEBOX) => {
                            let _ = ShowWindowAsync(hwnd, SW_MINIMIZE);
                        }
                        HTMAXBUTTON if self.has(WS_MAXIMIZEBOX) => self.zoom(),
                        HTCLOSE => {
                            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                        }
                        _ => {}
                    }
                    Some(0)
                }
                WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                    self.wheel(msg, wparam, lparam);
                    Some(0)
                }
                WM_KEYUP | WM_SYSKEYUP => {
                    if let Some(event) = keys::key(&self.seen, wparam, lparam, false)
                        && !crate::native::key(callbacks.id, &event, &|| {
                            callbacks.composition().is_some()
                        })
                    {
                        callbacks.input(event);
                    }
                    // Taking the release also keeps Alt from opening the window menu.
                    Some(0)
                }
                WM_CHAR => {
                    self.typed(wparam.0 as u16);
                    Some(0)
                }
                WM_IME_STARTCOMPOSITION => {
                    self.place_ime();
                    Some(0)
                }
                WM_IME_COMPOSITION => self.composition(lparam.0 as u32),
                WM_IME_ENDCOMPOSITION => {
                    callbacks.clear_composition();
                    None
                }
                WM_SETCURSOR if lparam.0 as u32 & 0xFFFF == HTCLIENT => {
                    SetCursor(current_cursor());
                    Some(1)
                }
                WM_SETTINGCHANGE => {
                    let area = PCWSTR(lparam.0 as *const u16);
                    if !area.is_null() && area.as_wide() == w!("ImmersiveColorSet").as_wide() {
                        follow_theme(hwnd);
                        callbacks.appearance_changed();
                    }
                    None
                }
                WM_DROPFILES => {
                    self.files(HDROP(wparam.0 as _));
                    Some(0)
                }
                _ => None,
            }
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: the user data is the state that `Window::open` set, until `WM_NCDESTROY` takes it back.
    unsafe {
        let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Shared;
        if raw.is_null() {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        // The window can close while a message runs, so each message holds a count of the state.
        Rc::increment_strong_count(raw);
        let shared = Rc::from_raw(raw);
        let result = match shared.message(msg, wparam, lparam) {
            Some(result) => LRESULT(result),
            None => DefWindowProcW(hwnd, msg, wparam, lparam),
        };
        // After this message the window is gone, and so is its count of the state.
        if let WM_NCDESTROY = msg {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Rc::from_raw(raw));
        }
        result
    }
}
