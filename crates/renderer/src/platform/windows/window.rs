use super::{
    CURSOR,
    directx::{DirectXAtlas, Renderer},
    events, logical_rect, monitor_scale, run_after, start_frames, stop_frames,
};
use crate::{
    CursorStyle, FileDropEvent, ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseExitEvent,
    MouseMoveEvent, MouseUpEvent, NavigationDirection, Pixels, PlatformInput, Point, Scene,
    ScrollDelta, ScrollWheelEvent, Size, TouchPhase, WindowChange, WindowControlArea,
    WindowOptions, WindowState, app::Callbacks, point, px,
};
use anyhow::{Context as _, Result};
use raw_window_handle as rwh;
use std::{
    cell::{Cell, RefCell},
    num::NonZeroIsize,
    path::PathBuf,
    rc::Rc,
    sync::atomic::{AtomicU16, Ordering},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{Dwm::*, Gdi::*},
        System::{LibraryLoader::GetModuleHandleW, Registry::*, SystemServices::*},
        UI::{
            Controls::{HOVER_DEFAULT, WM_MOUSELEAVE},
            HiDpi::*,
            Input::{Ime::*, KeyboardAndMouse::*},
            Shell::*,
            WindowsAndMessaging::*,
        },
    },
    core::{BOOL, HSTRING, PCWSTR, w},
};

static CLASS: AtomicU16 = AtomicU16::new(0);

/// The input state of a window.
struct State {
    renderer: Renderer,
    keys: events::KeyState,
    /// The last press: button, time, position and click count.
    click: (MouseButton, Instant, POINT, usize),
    /// A pending high surrogate of `WM_CHAR`.
    surrogate: Option<u16>,
}

struct Inner {
    hwnd: HWND,
    callbacks: Callbacks,
    /// The title bar is drawn by the app, which extends the content to the top edge.
    hide_title_bar: bool,
    scale: Cell<f32>,
    /// The app wants frames, also while the window is hidden or minimized.
    wants_frames: Cell<bool>,
    movable: Cell<bool>,
    closable: Cell<bool>,
    /// The minimum and maximum content size; zero is no limit.
    limits: Cell<(Size<Pixels>, Size<Pixels>)>,
    /// The style and frame to restore after fullscreen.
    fullscreen: Cell<Option<(isize, RECT)>>,
    size_move: Cell<bool>,
    resizing: Cell<bool>,
    hovered: Cell<bool>,
    position: Cell<Point<Pixels>>,
    /// The non-client button that was pressed, clicked once it is released over it.
    nc_pressed: Cell<Option<u32>>,
    ime: Cell<bool>,
    state: RefCell<State>,
}

/// A native window and its swap chain.
pub(crate) struct Window {
    inner: Rc<Inner>,
    atlas: Rc<DirectXAtlas>,
}

impl Window {
    pub fn open(options: &WindowOptions, callbacks: Callbacks) -> Result<Self> {
        // SAFETY: Win32 window setup on the main thread; the window keeps a strong reference to
        // its state until `WM_NCDESTROY`.
        unsafe {
            let instance = GetModuleHandleW(None)?;
            if CLASS.load(Ordering::Relaxed) == 0 {
                // The icon of a built app is its first resource.
                let icon = LoadImageW(
                    Some(instance.into()),
                    PCWSTR(1 as _),
                    IMAGE_ICON,
                    0,
                    0,
                    LR_DEFAULTSIZE | LR_SHARED,
                )
                .map(|icon| HICON(icon.0))
                .unwrap_or_default();
                let class = WNDCLASSW {
                    lpfnWndProc: Some(window_proc),
                    hInstance: instance.into(),
                    hIcon: icon,
                    lpszClassName: w!("AppsWindow"),
                    ..Default::default()
                };
                CLASS.store(RegisterClassW(&class), Ordering::Relaxed);
            }
            let mut style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
            if options.resizable {
                style |= WS_THICKFRAME | WS_MAXIMIZEBOX;
            }
            if options.minimizable {
                style |= WS_MINIMIZEBOX;
            }
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
                Some(instance.into()),
                None,
            )?;
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            let renderer = match Renderer::new(
                hwnd,
                [client.right as u32, client.bottom as u32],
                options.transparent,
            ) {
                Ok(renderer) => renderer,
                Err(error) => {
                    let _ = DestroyWindow(hwnd);
                    return Err(error);
                }
            };
            let atlas = renderer.atlas().clone();
            let inner = Rc::new(Inner {
                hwnd,
                callbacks,
                hide_title_bar: options.titlebar_hidden || options.frameless,
                scale: Cell::new(GetDpiForWindow(hwnd) as f32 / USER_DEFAULT_SCREEN_DPI as f32),
                wants_frames: Cell::new(false),
                movable: Cell::new(options.movable),
                closable: Cell::new(true),
                limits: Cell::default(),
                fullscreen: Cell::new(None),
                size_move: Cell::new(false),
                resizing: Cell::new(false),
                hovered: Cell::new(false),
                position: Cell::default(),
                nc_pressed: Cell::new(None),
                ime: Cell::new(true),
                state: RefCell::new(State {
                    renderer,
                    keys: events::KeyState::default(),
                    click: (MouseButton::Left, Instant::now(), POINT::default(), 0),
                    surrogate: None,
                }),
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Rc::into_raw(inner.clone()) as isize);
            let window = Self { inner, atlas };
            if window.inner.hide_title_bar {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
                );
            }
            // Without this, a window without a title bar can keep the taskbar behind it.
            let _ = SetPropW(hwnd, w!("NonRudeHWND"), Some(HANDLE(1 as _)));
            DragAcceptFiles(hwnd, true);
            set_dark_mode(hwnd);
            // The bounds are in points of the primary display, with the frame's top left at the origin.
            let scale = monitor_scale(MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY));
            let bounds = options.bounds.scale(scale);
            let [left, top, right, bottom] = if options.outer_size {
                [0; 4]
            } else {
                frame_insets(hwnd)
            };
            SetWindowPos(
                hwnd,
                None,
                bounds.origin.x.0.round() as i32,
                bounds.origin.y.0.round() as i32,
                bounds.size.width.0.round() as i32 + left + right,
                bounds.size.height.0.round() as i32 + top + bottom,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )?;
            if options.show {
                let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            }
            Ok(window)
        }
    }

    pub fn atlas(&self) -> &Rc<DirectXAtlas> {
        &self.atlas
    }

    pub fn scale_factor(&self) -> f32 {
        self.inner.scale.get()
    }

    pub fn content_size(&self) -> Size<Pixels> {
        let mut client = RECT::default();
        // SAFETY: plain window query.
        let _ = unsafe { GetClientRect(self.inner.hwnd, &mut client) };
        logical_rect(client, self.inner.scale.get()).size
    }

    /// Paints on each display refresh until a frame wants no more.
    pub fn request_frame(&self) {
        self.inner.wants_frames.set(true);
        start_frames(self.inner.hwnd);
    }

    /// Draws once even when the window is hidden or covered, for captures.
    pub fn force_frame(&self) {
        let inner = Rc::downgrade(&self.inner);
        run_after(Duration::ZERO, move || {
            if let Some(inner) = inner.upgrade() {
                frame(&inner);
            }
        });
    }

    pub fn draw(&self, scene: &Scene, capture: bool) -> Option<Result<([u32; 2], Vec<u8>)>> {
        match self.inner.state.try_borrow_mut() {
            Ok(mut state) => state.renderer.draw(scene, capture),
            Err(_) => capture.then(|| Err(anyhow::anyhow!("The window is busy"))),
        }
    }

    pub fn is_active(&self) -> bool {
        // SAFETY: plain window query.
        unsafe { GetActiveWindow() == self.inner.hwnd }
    }

    pub fn set_title(&self, title: &str) {
        // SAFETY: plain window call.
        let _ = unsafe { SetWindowTextW(self.inner.hwnd, &HSTRING::from(title)) };
    }

    pub fn set_button_visibility(&self, _visible: bool) {}

    pub fn state(&self) -> WindowState {
        let hwnd = self.inner.hwnd;
        let scale = self.inner.scale.get();
        let (mut frame, mut client, mut origin) =
            (RECT::default(), RECT::default(), POINT::default());
        // SAFETY: plain window queries.
        unsafe {
            let _ = GetWindowRect(hwnd, &mut frame);
            let _ = GetClientRect(hwnd, &mut client);
            let _ = ClientToScreen(hwnd, &mut origin);
            let fullscreen = self.inner.fullscreen.get().is_some();
            WindowState {
                frame: logical_rect(frame, scale),
                content: logical_rect(
                    RECT {
                        left: origin.x,
                        top: origin.y,
                        right: origin.x + client.right,
                        bottom: origin.y + client.bottom,
                    },
                    scale,
                ),
                minimized: IsIconic(hwnd).as_bool(),
                maximized: IsZoomed(hwnd).as_bool() && !fullscreen,
                fullscreen,
                visible: IsWindowVisible(hwnd).as_bool(),
                resizing: self.inner.resizing.get(),
            }
        }
    }

    pub fn change(&self, change: WindowChange) {
        let hwnd = self.inner.hwnd;
        let style = |flag: WINDOW_STYLE, on: bool| {
            // SAFETY: plain window calls; the frame change applies the new style.
            unsafe {
                let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
                SetWindowLongPtrW(
                    hwnd,
                    GWL_STYLE,
                    if on {
                        style | flag.0 as isize
                    } else {
                        style & !(flag.0 as isize)
                    },
                );
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        };
        // SAFETY: plain window calls.
        let order = |after: HWND| unsafe {
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
        // SAFETY: plain window calls.
        unsafe {
            match change {
                WindowChange::Frame(bounds) => {
                    let bounds = bounds.scale(self.inner.scale.get());
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        bounds.origin.x.0.round() as i32,
                        bounds.origin.y.0.round() as i32,
                        bounds.size.width.0.round() as i32,
                        bounds.size.height.0.round() as i32,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                WindowChange::Center => {
                    let mut info = MONITORINFO {
                        cbSize: size_of::<MONITORINFO>() as u32,
                        ..Default::default()
                    };
                    let mut frame = RECT::default();
                    if GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info)
                        .as_bool()
                        && GetWindowRect(hwnd, &mut frame).is_ok()
                    {
                        let (work, width, height) = (
                            info.rcWork,
                            frame.right - frame.left,
                            frame.bottom - frame.top,
                        );
                        let x = work.left + (work.right - work.left - width) / 2;
                        let y = work.top + (work.bottom - work.top - height) / 2;
                        let _ = SetWindowPos(
                            hwnd,
                            None,
                            x,
                            y,
                            0,
                            0,
                            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                    }
                }
                WindowChange::Show(true) => self.activate(),
                WindowChange::Show(false) => {
                    let _ = ShowWindow(hwnd, SW_SHOWNA);
                }
                WindowChange::Hide => {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
                WindowChange::Blur => order(HWND_BOTTOM),
                WindowChange::Restore => {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                }
                WindowChange::SizeLimits(min, max) => self.inner.limits.set((min, max)),
                WindowChange::AlwaysOnTop(on) => {
                    order(if on { HWND_TOPMOST } else { HWND_NOTOPMOST })
                }
                WindowChange::Resizable(on) => style(WS_THICKFRAME, on),
                WindowChange::Minimizable(on) => style(WS_MINIMIZEBOX, on),
                WindowChange::Maximizable(on) => style(WS_MAXIMIZEBOX, on),
                WindowChange::Closable(on) => {
                    self.inner.closable.set(on);
                    let _ = EnableMenuItem(
                        GetSystemMenu(hwnd, false),
                        SC_CLOSE,
                        MF_BYCOMMAND | if on { MF_ENABLED } else { MF_GRAYED },
                    );
                }
                WindowChange::Movable(on) => self.inner.movable.set(on),
                WindowChange::Background(color) => {
                    if let Ok(mut state) = self.inner.state.try_borrow_mut() {
                        state.renderer.set_background(color);
                    }
                }
                WindowChange::ContentProtection(on) => {
                    let affinity = if on { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE };
                    let _ = SetWindowDisplayAffinity(hwnd, affinity);
                }
                // An owned window stays above its owner.
                WindowChange::Parent(parent, _) => {
                    let owner = match parent {
                        Some(rwh::RawWindowHandle::Win32(handle)) => handle.hwnd.get(),
                        _ => 0,
                    };
                    SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, owner);
                }
                _ => {}
            }
        }
        self.inner.callbacks.changed();
    }

    pub fn minimize(&self) {
        // SAFETY: plain window call, run after the current message.
        let _ = unsafe { ShowWindowAsync(self.inner.hwnd, SW_MINIMIZE) };
    }

    pub fn zoom(&self) {
        let hwnd = self.inner.hwnd;
        // SAFETY: plain window calls, run after the current message.
        let _ = unsafe {
            ShowWindowAsync(
                hwnd,
                if IsZoomed(hwnd).as_bool() {
                    SW_RESTORE
                } else {
                    SW_MAXIMIZE
                },
            )
        };
    }

    pub fn toggle_fullscreen(&self) {
        let hwnd = self.inner.hwnd;
        // SAFETY: plain window calls. The saved state is set before the frame changes, which
        // `WM_NCCALCSIZE` reads.
        unsafe {
            let (style, rect) = match self.inner.fullscreen.take() {
                Some(saved) => {
                    let _ = SetPropW(hwnd, w!("NonRudeHWND"), Some(HANDLE(1 as _)));
                    saved
                }
                None => {
                    let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
                    let mut frame = RECT::default();
                    let _ = GetWindowRect(hwnd, &mut frame);
                    let mut info = MONITORINFO {
                        cbSize: size_of::<MONITORINFO>() as u32,
                        ..Default::default()
                    };
                    if !GetMonitorInfoW(
                        MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
                        &mut info,
                    )
                    .as_bool()
                    {
                        return;
                    }
                    self.inner.fullscreen.set(Some((style, frame)));
                    let _ = RemovePropW(hwnd, w!("NonRudeHWND"));
                    let chrome =
                        WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MAXIMIZEBOX | WS_MINIMIZEBOX;
                    (style & !(chrome.0 as isize), info.rcMonitor)
                }
            };
            SetWindowLongPtrW(hwnd, GWL_STYLE, style);
            let _ = SetWindowPos(
                hwnd,
                None,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOZORDER,
            );
        }
        self.inner.callbacks.changed();
    }

    pub fn activate(&self) {
        let hwnd = self.inner.hwnd;
        // SAFETY: plain window and input calls.
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            } else if !IsWindowVisible(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_SHOW);
            }
            if GetForegroundWindow() != hwnd {
                // Windows only lets a process that just got input take the foreground, so this
                // sends an Alt press.
                let key = |flags| INPUT {
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
                    &[key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)],
                    size_of::<INPUT>() as i32,
                );
                let _ = SetForegroundWindow(hwnd);
            }
        }
    }

    /// Resizes the content area.
    pub fn resize(&self, size: Size<Pixels>) {
        let size = size.scale(self.inner.scale.get());
        let [left, top, right, bottom] = frame_insets(self.inner.hwnd);
        // SAFETY: plain window call.
        let _ = unsafe {
            SetWindowPos(
                self.inner.hwnd,
                None,
                0,
                0,
                size.width.0.round() as i32 + left + right,
                size.height.0.round() as i32 + top + bottom,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
    }

    pub fn start_window_move(&self) {
        // SAFETY: the posted command moves the window with the pressed mouse.
        unsafe {
            let _ = ReleaseCapture();
            let _ = PostMessageW(
                Some(self.inner.hwnd),
                WM_SYSCOMMAND,
                WPARAM((SC_MOVE | HTCAPTION) as usize),
                LPARAM(0),
            );
        }
    }

    pub fn titlebar_double_click(&self) {
        self.zoom();
    }

    pub fn raw_window_handle(&self) -> Result<rwh::RawWindowHandle> {
        let hwnd = NonZeroIsize::new(self.inner.hwnd.0 as isize).context("the window is closed")?;
        Ok(rwh::RawWindowHandle::Win32(rwh::Win32WindowHandle::new(
            hwnd,
        )))
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        stop_frames(self.inner.hwnd);
        // SAFETY: the window procedure drops its reference at `WM_NCDESTROY`.
        let _ = unsafe { DestroyWindow(self.inner.hwnd) };
    }
}

pub(super) fn load_cursor(style: CursorStyle) -> Option<HCURSOR> {
    let name = match style {
        CursorStyle::None => return None,
        CursorStyle::IBeam | CursorStyle::IBeamCursorForVerticalLayout => IDC_IBEAM,
        CursorStyle::Crosshair => IDC_CROSS,
        CursorStyle::PointingHand
        | CursorStyle::DragLink
        | CursorStyle::OpenHand
        | CursorStyle::ClosedHand => IDC_HAND,
        CursorStyle::ResizeLeft
        | CursorStyle::ResizeRight
        | CursorStyle::ResizeLeftRight
        | CursorStyle::ResizeColumn => IDC_SIZEWE,
        CursorStyle::ResizeUp
        | CursorStyle::ResizeDown
        | CursorStyle::ResizeUpDown
        | CursorStyle::ResizeRow => IDC_SIZENS,
        CursorStyle::ResizeUpLeftDownRight => IDC_SIZENWSE,
        CursorStyle::ResizeUpRightDownLeft => IDC_SIZENESW,
        CursorStyle::OperationNotAllowed => IDC_NO,
        _ => IDC_ARROW,
    };
    // SAFETY: system cursors are shared and never freed.
    unsafe { LoadCursorW(None, name) }.ok()
}

/// Sends a key down to the window before it is translated into characters. Returns whether the
/// app handled it, so it types nothing.
pub(super) fn key_down(msg: &MSG) -> bool {
    // SAFETY: only our windows have our class, whose user data is the window state.
    let inner = unsafe {
        if GetClassLongPtrW(msg.hwnd, GCW_ATOM) as u16 != CLASS.load(Ordering::Relaxed) {
            return false;
        }
        let raw = GetWindowLongPtrW(msg.hwnd, GWLP_USERDATA) as *const Inner;
        if raw.is_null() {
            return false;
        }
        Rc::increment_strong_count(raw);
        Rc::from_raw(raw)
    };
    let input = match inner.state.try_borrow_mut() {
        Ok(mut state) => events::key_event(&mut state.keys, msg.wParam, msg.lParam, true),
        Err(_) => None,
    };
    input.is_some_and(|input| inner.callbacks.input(input))
}

/// Draws when the app changed something, and stops frames once it wants no more. Returns
/// `false` while the app is busy.
fn frame(inner: &Inner) -> bool {
    match inner.callbacks.frame() {
        Some(more) => {
            if !more {
                inner.wants_frames.set(false);
                stop_frames(inner.hwnd);
            }
            true
        }
        None => false,
    }
}

/// The frame around the content: left, top, right and bottom, in physical pixels.
fn frame_insets(hwnd: HWND) -> [i32; 4] {
    let (mut frame, mut client, mut origin) = (RECT::default(), RECT::default(), POINT::default());
    // SAFETY: plain window queries.
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

/// The invisible resize border and padding of a frame.
fn frame_thickness(hwnd: HWND) -> (i32, i32) {
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

/// Matches the title bar to the system's dark mode.
fn set_dark_mode(hwnd: HWND) {
    let (mut light, mut size) = (1u32, 4u32);
    // SAFETY: the value is a DWORD read into a local.
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

/// Places the IME windows at the caret.
fn ime_position(inner: &Inner) {
    let callbacks = inner.callbacks;
    let Some(bounds) = callbacks
        .selected_text_range()
        .and_then(|selection| callbacks.bounds_for_range(selection.range))
    else {
        return;
    };
    let scale = inner.scale.get();
    let caret = POINT {
        x: (bounds.origin.x.0 * scale) as i32,
        y: ((bounds.origin.y.0 + bounds.size.height.0 / 2.) * scale) as i32,
    };
    // SAFETY: the context is released after use.
    unsafe {
        let context = ImmGetContext(inner.hwnd);
        if context.is_invalid() {
            return;
        }
        let _ = ImmSetCompositionWindow(
            context,
            &COMPOSITIONFORM {
                dwStyle: CFS_POINT,
                ptCurrentPos: caret,
                ..Default::default()
            },
        );
        let _ = ImmSetCandidateWindow(
            context,
            &CANDIDATEFORM {
                dwStyle: CFS_CANDIDATEPOS,
                ptCurrentPos: caret,
                ..Default::default()
            },
        );
        let _ = ImmReleaseContext(inner.hwnd, context);
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: the user data is the state set by `Window::open`, until `WM_NCDESTROY`.
    unsafe {
        let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Inner;
        if raw.is_null() {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        // WARNING: the window can close while its message runs, so the message keeps the state alive.
        Rc::increment_strong_count(raw);
        let inner = Rc::from_raw(raw);
        let result = handle(&inner, msg, wparam, lparam)
            .map_or_else(|| DefWindowProcW(hwnd, msg, wparam, lparam), LRESULT);
        if msg == WM_NCDESTROY {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Rc::from_raw(raw));
        }
        result
    }
}

/// Handles a window message, or returns `None` for the default handling.
fn handle(inner: &Inner, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<isize> {
    let hwnd = inner.hwnd;
    let callbacks = inner.callbacks;
    let scale = inner.scale.get();
    let logical =
        |point: POINT| crate::point(px(point.x as f32 / scale), px(point.y as f32 / scale));
    let client = || POINT {
        x: lparam.0 as i16 as i32,
        y: (lparam.0 >> 16) as i16 as i32,
    };
    let screen = || {
        let mut point = client();
        // SAFETY: plain window query.
        let _ = unsafe { ScreenToClient(hwnd, &mut point) };
        point
    };
    let style = || unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) };
    let has = |flag: WINDOW_STYLE| style() & flag.0 as isize != 0;
    let track = |flags: TRACKMOUSEEVENT_FLAGS| {
        if !inner.hovered.replace(true) {
            // SAFETY: plain input call.
            let _ = unsafe {
                TrackMouseEvent(&mut TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: flags,
                    hwndTrack: hwnd,
                    dwHoverTime: HOVER_DEFAULT,
                })
            };
        }
    };
    let click = |button: MouseButton, at: POINT| -> usize {
        let Ok(mut state) = inner.state.try_borrow_mut() else {
            return 1;
        };
        let (last, time, position, count) = state.click;
        // SAFETY: plain system queries.
        let double = unsafe {
            last == button
                && time.elapsed() < Duration::from_millis(GetDoubleClickTime() as u64)
                && (at.x - position.x).abs() <= GetSystemMetrics(SM_CXDOUBLECLK)
                && (at.y - position.y).abs() <= GetSystemMetrics(SM_CYDOUBLECLK)
        };
        let count = if double { count + 1 } else { 1 };
        state.click = (button, Instant::now(), at, count);
        count
    };
    let button = || match msg {
        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK | WM_NCLBUTTONUP => {
            Some(MouseButton::Left)
        }
        WM_RBUTTONDOWN | WM_RBUTTONUP | WM_NCRBUTTONDOWN | WM_NCRBUTTONUP => {
            Some(MouseButton::Right)
        }
        WM_MBUTTONDOWN | WM_MBUTTONUP | WM_NCMBUTTONDOWN | WM_NCMBUTTONUP => {
            Some(MouseButton::Middle)
        }
        _ => match (wparam.0 >> 16) as u16 {
            XBUTTON1 => Some(MouseButton::Navigate(NavigationDirection::Back)),
            XBUTTON2 => Some(MouseButton::Navigate(NavigationDirection::Forward)),
            _ => None,
        },
    };
    // SAFETY: the messages carry the documented parameters, and window calls are made without
    // a borrow of the state.
    unsafe {
        match msg {
            // A click on the title bar activates the window before the app sees the click.
            WM_MOUSEACTIVATE => Some(MA_ACTIVATE as isize),
            WM_ACTIVATE => {
                if wparam.0 & 0xFFFF != WA_INACTIVE as usize {
                    // Alt-tab can leave a stale Alt, since the key up went to another window.
                    if let Ok(mut state) = inner.state.try_borrow_mut() {
                        state.keys = events::KeyState::default();
                    }
                    callbacks.input(PlatformInput::ModifiersChanged(ModifiersChangedEvent {
                        modifiers: events::modifiers(),
                        capslock: events::capslock(),
                    }));
                }
                callbacks.changed();
                None
            }
            WM_MOVE => {
                if !inner.size_move.get() {
                    callbacks.changed();
                }
                Some(0)
            }
            WM_ENTERSIZEMOVE => {
                inner.size_move.set(true);
                None
            }
            WM_EXITSIZEMOVE => {
                inner.size_move.set(false);
                inner.resizing.set(false);
                callbacks.changed();
                None
            }
            WM_SIZING => {
                inner.resizing.set(true);
                None
            }
            WM_SIZE => {
                if wparam.0 == SIZE_MINIMIZED as usize {
                    callbacks.changed();
                    return Some(0);
                }
                let size = [lparam.0 as u32 & 0xFFFF, (lparam.0 >> 16) as u32 & 0xFFFF];
                if let Ok(mut state) = inner.state.try_borrow_mut()
                    && let Err(error) = state.renderer.resize(size)
                {
                    eprintln!("apps: resizing the swap chain: {error:#}");
                }
                callbacks.resized(
                    crate::size(px(size[0] as f32 / scale), px(size[1] as f32 / scale)),
                    scale,
                );
                if inner.wants_frames.get() {
                    start_frames(hwnd);
                }
                // Drawn now, so a live resize shows content of the new size.
                frame(inner);
                Some(0)
            }
            WM_WINDOWPOSCHANGED => {
                let flags = (*(lparam.0 as *const WINDOWPOS)).flags;
                if flags.contains(SWP_SHOWWINDOW) && inner.wants_frames.get() {
                    start_frames(hwnd);
                }
                if flags.contains(SWP_SHOWWINDOW) || flags.contains(SWP_HIDEWINDOW) {
                    callbacks.changed();
                }
                None
            }
            WM_GETMINMAXINFO => {
                let (min, max) = inner.limits.get();
                let [left, top, right, bottom] = frame_insets(hwnd);
                let info = &mut *(lparam.0 as *mut MINMAXINFO);
                let side = |value: Pixels, inset: i32| (value.0 * scale).round() as i32 + inset;
                if min.width.0 > 0. || min.height.0 > 0. {
                    info.ptMinTrackSize.x =
                        info.ptMinTrackSize.x.max(side(min.width, left + right));
                    info.ptMinTrackSize.y =
                        info.ptMinTrackSize.y.max(side(min.height, top + bottom));
                }
                if max.width.0 > 0. {
                    info.ptMaxTrackSize.x = side(max.width, left + right);
                }
                if max.height.0 > 0. {
                    info.ptMaxTrackSize.y = side(max.height, top + bottom);
                }
                Some(0)
            }
            WM_NCCALCSIZE
                if inner.hide_title_bar && inner.fullscreen.get().is_none() && wparam.0 != 0 =>
            {
                // The default frame without its title bar.
                let params = lparam.0 as *mut NCCALCSIZE_PARAMS;
                let top = (*params).rgrc[0].top;
                let result = DefWindowProcW(hwnd, msg, wparam, lparam);
                (*params).rgrc[0].top = top;
                if IsZoomed(hwnd).as_bool() {
                    // A maximized window reaches past the screen by its frame.
                    (*params).rgrc[0].top += frame_thickness(hwnd).1;
                }
                Some(result.0)
            }
            WM_DPICHANGED => {
                let scale = (wparam.0 & 0xFFFF) as f32 / USER_DEFAULT_SCREEN_DPI as f32;
                inner.scale.set(scale);
                let mut rect = *(lparam.0 as *const RECT);
                let mut info = MONITORINFO {
                    cbSize: size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                if IsZoomed(hwnd).as_bool()
                    && GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info)
                        .as_bool()
                {
                    rect = info.rcWork;
                }
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                let mut client = RECT::default();
                let _ = GetClientRect(hwnd, &mut client);
                callbacks.resized(logical_rect(client, scale).size, scale);
                Some(0)
            }
            WM_NCHITTEST => {
                if inner.fullscreen.get().is_some() {
                    return None;
                }
                let movable = inner.movable.get();
                let area = callbacks.control_area().and_then(|area| match area {
                    WindowControlArea::Drag => movable.then_some(HTCAPTION),
                    WindowControlArea::Close => Some(HTCLOSE),
                    WindowControlArea::Max if has(WS_MAXIMIZEBOX) => Some(HTMAXBUTTON),
                    WindowControlArea::Min if has(WS_MINIMIZEBOX) => Some(HTMINBUTTON),
                    _ if movable => Some(HTCAPTION),
                    _ => Some(HTNOWHERE),
                });
                if !inner.hide_title_bar {
                    return area.map(|area| area as isize);
                }
                // Without a title bar the default handling has no top resize edge.
                let (frame_x, frame_y) = frame_thickness(hwnd);
                let point = screen();
                if has(WS_THICKFRAME)
                    && !IsZoomed(hwnd).as_bool()
                    && (0..=frame_y).contains(&point.y)
                {
                    let mut frame = RECT::default();
                    let _ = GetWindowRect(hwnd, &mut frame);
                    return Some(if point.x <= 0 {
                        HTTOPLEFT
                    } else if frame.right - frame.left - 1 - 2 * frame_x <= point.x {
                        HTTOPRIGHT
                    } else {
                        HTTOP
                    } as isize);
                }
                area.map(|area| area as isize)
            }
            WM_PAINT => {
                if frame(inner) {
                    let enabled = callbacks.accepts_text_input();
                    if enabled != inner.ime.replace(enabled) {
                        if enabled {
                            let _ = ImmAssociateContextEx(hwnd, HIMC::default(), IACE_DEFAULT);
                        } else {
                            // The IME context is per thread, so only this window's composition ends.
                            let context = ImmGetContext(hwnd);
                            if GetFocus() == hwnd && !context.is_invalid() {
                                let _ = ImmNotifyIME(context, NI_COMPOSITIONSTR, CPS_COMPLETE, 0);
                            }
                            let _ = ImmReleaseContext(hwnd, context);
                            let _ = ImmAssociateContextEx(hwnd, HIMC::default(), 0);
                        }
                    }
                }
                let _ = ValidateRect(Some(hwnd), None);
                Some(0)
            }
            WM_ERASEBKGND => Some(1),
            WM_CLOSE => {
                if inner.closable.get() && callbacks.should_close() {
                    let id = callbacks.id;
                    crate::app::update(move |app| {
                        app.update_window(id, |window, _| window.remove_window());
                    });
                }
                Some(0)
            }
            WM_DESTROY => {
                stop_frames(hwnd);
                Some(0)
            }
            WM_SYSCOMMAND => {
                let command = wparam.0 as u32 & 0xFFF0;
                ((command == SC_MOVE && !inner.movable.get())
                    || (command == SC_CLOSE && !inner.closable.get()))
                .then_some(0)
            }
            WM_MOUSEMOVE => {
                track(TME_LEAVE);
                let flags = MODIFIERKEYS_FLAGS(wparam.0 as u32);
                let pressed_button = [
                    (MK_LBUTTON, MouseButton::Left),
                    (MK_RBUTTON, MouseButton::Right),
                    (MK_MBUTTON, MouseButton::Middle),
                    (
                        MK_XBUTTON1,
                        MouseButton::Navigate(NavigationDirection::Back),
                    ),
                    (
                        MK_XBUTTON2,
                        MouseButton::Navigate(NavigationDirection::Forward),
                    ),
                ]
                .into_iter()
                .find(|(flag, _)| flags.contains(*flag))
                .map(|(_, button)| button);
                let position = logical(client());
                inner.position.set(position);
                callbacks.input(PlatformInput::MouseMove(MouseMoveEvent {
                    position,
                    pressed_button,
                    modifiers: events::modifiers(),
                }));
                Some(0)
            }
            WM_NCMOUSEMOVE => {
                track(TME_LEAVE | TME_NONCLIENT);
                let position = logical(screen());
                inner.position.set(position);
                callbacks
                    .input(PlatformInput::MouseMove(MouseMoveEvent {
                        position,
                        pressed_button: None,
                        modifiers: events::modifiers(),
                    }))
                    .then_some(0)
            }
            WM_MOUSELEAVE | WM_NCMOUSELEAVE => {
                inner.hovered.set(false);
                callbacks.input(PlatformInput::MouseExited(MouseExitEvent {
                    position: inner.position.get(),
                    pressed_button: None,
                    modifiers: events::modifiers(),
                }));
                Some(0)
            }
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN => {
                let button = button()?;
                SetCapture(hwnd);
                let at = client();
                callbacks.input(PlatformInput::MouseDown(MouseDownEvent {
                    button,
                    position: logical(at),
                    modifiers: events::modifiers(),
                    click_count: click(button, at),
                    first_mouse: false,
                }));
                Some((msg == WM_XBUTTONDOWN) as isize)
            }
            WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP | WM_XBUTTONUP => {
                let button = button()?;
                let _ = ReleaseCapture();
                let click_count = inner.state.try_borrow().map_or(1, |state| state.click.3);
                callbacks.input(PlatformInput::MouseUp(MouseUpEvent {
                    button,
                    position: logical(client()),
                    modifiers: events::modifiers(),
                    click_count,
                }));
                Some((msg == WM_XBUTTONUP) as isize)
            }
            // Windows moves the window from the title bar, zooms it on a double click and shows
            // its menu on a right click.
            WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK | WM_NCRBUTTONDOWN | WM_NCMBUTTONDOWN
            | WM_NCLBUTTONUP | WM_NCRBUTTONUP | WM_NCMBUTTONUP
                if wparam.0 as u32 == HTCAPTION =>
            {
                None
            }
            WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK | WM_NCRBUTTONDOWN | WM_NCMBUTTONDOWN => {
                let button = button()?;
                let at = screen();
                if callbacks.input(PlatformInput::MouseDown(MouseDownEvent {
                    button,
                    position: logical(at),
                    modifiers: events::modifiers(),
                    click_count: click(button, at),
                    first_mouse: false,
                })) {
                    return Some(0);
                }
                // The buttons act on release, as the default handling would.
                let hit = wparam.0 as u32;
                (button == MouseButton::Left && matches!(hit, HTMINBUTTON | HTMAXBUTTON | HTCLOSE))
                    .then(|| {
                        inner.nc_pressed.set(Some(hit));
                        0
                    })
            }
            WM_NCLBUTTONUP | WM_NCRBUTTONUP | WM_NCMBUTTONUP => {
                let button = button()?;
                if callbacks.input(PlatformInput::MouseUp(MouseUpEvent {
                    button,
                    position: logical(screen()),
                    modifiers: events::modifiers(),
                    click_count: 1,
                })) {
                    return Some(0);
                }
                let hit = wparam.0 as u32;
                if button != MouseButton::Left || inner.nc_pressed.take() != Some(hit) {
                    return None;
                }
                match hit {
                    HTMINBUTTON if has(WS_MINIMIZEBOX) => {
                        let _ = ShowWindowAsync(hwnd, SW_MINIMIZE);
                    }
                    HTMAXBUTTON if has(WS_MAXIMIZEBOX) => {
                        let _ = ShowWindowAsync(
                            hwnd,
                            if IsZoomed(hwnd).as_bool() {
                                SW_RESTORE
                            } else {
                                SW_MAXIMIZE
                            },
                        );
                    }
                    HTCLOSE => {
                        let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                    }
                    _ => {}
                }
                Some(0)
            }
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                let modifiers = events::modifiers();
                let horizontal = msg == WM_MOUSEHWHEEL || modifiers.shift;
                let mut amount = 3u32;
                let _ = SystemParametersInfoW(
                    if horizontal {
                        SPI_GETWHEELSCROLLCHARS
                    } else {
                        SPI_GETWHEELSCROLLLINES
                    },
                    0,
                    Some((&raw mut amount).cast()),
                    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                );
                let mut distance =
                    (wparam.0 >> 16) as i16 as f32 / WHEEL_DELTA as f32 * amount as f32;
                if msg == WM_MOUSEHWHEEL {
                    distance = -distance;
                }
                callbacks.input(PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position: logical(screen()),
                    delta: ScrollDelta::Lines(if horizontal {
                        point(distance, 0.)
                    } else {
                        point(0., distance)
                    }),
                    modifiers,
                    touch_phase: TouchPhase::Moved,
                }));
                Some(0)
            }
            WM_KEYUP | WM_SYSKEYUP => {
                let input = match inner.state.try_borrow_mut() {
                    Ok(mut state) => events::key_event(&mut state.keys, wparam, lparam, false),
                    Err(_) => None,
                };
                if let Some(input) = input {
                    callbacks.input(input);
                }
                // Also keeps a released Alt from opening the window menu.
                Some(0)
            }
            WM_CHAR => {
                let unit = wparam.0 as u16;
                let text = {
                    let mut state = inner.state.try_borrow_mut().ok()?;
                    match unit {
                        0xD800..=0xDBFF => {
                            state.surrogate = Some(unit);
                            None
                        }
                        0xDC00..=0xDFFF => state
                            .surrogate
                            .take()
                            .and_then(|high| String::from_utf16(&[high, unit]).ok()),
                        _ => {
                            state.surrogate = None;
                            char::from_u32(unit as u32)
                                .filter(|char| !char.is_control())
                                .map(String::from)
                        }
                    }
                };
                if let Some(text) = text {
                    callbacks.replace_text_in_range(None, &text);
                }
                Some(0)
            }
            WM_IME_STARTCOMPOSITION => {
                ime_position(inner);
                Some(0)
            }
            WM_IME_COMPOSITION => {
                let context = ImmGetContext(hwnd);
                if context.is_invalid() {
                    return None;
                }
                let flags = lparam.0 as u32;
                let result = if flags == 0 {
                    // Some IMEs cancel the composition this way.
                    callbacks.replace_text_in_range(None, "");
                    Some(0)
                } else {
                    if flags & GCS_RESULTSTR.0 != 0
                        && let Some(text) = events::composition(context, GCS_RESULTSTR)
                    {
                        callbacks.replace_text_in_range(None, &String::from_utf16_lossy(&text));
                    }
                    if flags & GCS_COMPSTR.0 != 0
                        && let Some(text) = events::composition(context, GCS_COMPSTR)
                    {
                        let caret = (!text.is_empty() && flags & GCS_CURSORPOS.0 != 0).then(|| {
                            let caret = events::composition_caret(context, text.len());
                            caret..caret
                        });
                        callbacks.replace_and_mark_text_in_range(
                            None,
                            &String::from_utf16_lossy(&text),
                            caret,
                        );
                    }
                    (flags & (GCS_RESULTSTR.0 | GCS_COMPSTR.0) != 0).then_some(0)
                };
                let _ = ImmReleaseContext(hwnd, context);
                ime_position(inner);
                result
            }
            WM_SETCURSOR if (lparam.0 & 0xFFFF) as u32 == HTCLIENT => {
                SetCursor(CURSOR.get());
                Some(1)
            }
            WM_SETTINGCHANGE => {
                let area = PCWSTR(lparam.0 as *const u16);
                if !area.is_null()
                    && area
                        .to_string()
                        .is_ok_and(|area| area == "ImmersiveColorSet")
                {
                    set_dark_mode(hwnd);
                    callbacks.appearance_changed();
                }
                None
            }
            WM_DROPFILES => {
                let drop = HDROP(wparam.0 as _);
                let paths = (0..DragQueryFileW(drop, u32::MAX, None))
                    .filter_map(|index| {
                        let mut name = vec![0u16; DragQueryFileW(drop, index, None) as usize + 1];
                        let len = DragQueryFileW(drop, index, Some(&mut name)) as usize;
                        (len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&name[..len])))
                    })
                    .collect();
                let mut at = POINT::default();
                let _ = DragQueryPoint(drop, &mut at);
                DragFinish(drop);
                let position = logical(at);
                callbacks.input(PlatformInput::FileDrop(FileDropEvent::Entered {
                    position,
                    paths,
                }));
                callbacks.input(PlatformInput::FileDrop(FileDropEvent::Submit { position }));
                Some(0)
            }
            _ => None,
        }
    }
}
