use super::*;
use ::windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::{
            Com::{STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET},
            LibraryLoader::GetModuleHandleW,
        },
        UI::{HiDpi::GetDpiForWindow, Shell::SHCreateMemStream, WindowsAndMessaging::*},
    },
    core::{BOOL, HSTRING, Interface, PWSTR, w},
};
use ::wry::WebViewExtWindows;
use raw_window_handle::{HandleError, HasWindowHandle, Win32WindowHandle, WindowHandle};
use std::{
    cell::Cell,
    num::NonZeroIsize,
    sync::atomic::{AtomicBool, Ordering},
};
use webview2_com::{Microsoft::Web::WebView2::Win32::*, *};

struct WindowState {
    events: Arc<Events>,
    closed: AtomicBool,
    loading: Arc<AtomicBool>,
}

pub(super) struct Window {
    handle: HWND,
    state: Box<WindowState>,
    bounds: Cell<Option<[f64; 4]>>,
    attached: Cell<bool>,
    visible: bool,
    shown: Cell<bool>,
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        if message == WM_CLOSE {
            let state = GetWindowLongPtrW(window, GWLP_USERDATA) as *const WindowState;
            if let Some(state) = state.as_ref() {
                state.closed.store(true, Ordering::Relaxed);
                state.events.push(json!({"type":"closed"}));
            }
            let _ = ShowWindow(window, SW_HIDE);
            return LRESULT(0);
        }
        if message == WM_NCDESTROY {
            SetWindowLongPtrW(window, GWLP_USERDATA, 0);
        }
        DefWindowProcW(window, message, wparam, lparam)
    }
}

fn navigation(view: &ICoreWebView2) -> ::windows::core::Result<Value> {
    unsafe {
        let mut url = PWSTR::null();
        view.Source(&mut url)?;
        let url = CoTaskMemPWSTR::from(url).to_string();
        let mut title = PWSTR::null();
        view.DocumentTitle(&mut title)?;
        let title = CoTaskMemPWSTR::from(title).to_string();
        let mut back = BOOL::default();
        let mut forward = BOOL::default();
        view.CanGoBack(&mut back)?;
        view.CanGoForward(&mut forward)?;
        Ok(
            json!({"url":url,"title":title,"canGoBack":back.as_bool(),"canGoForward":forward.as_bool()}),
        )
    }
}

impl HasWindowHandle for Window {
    fn window_handle(&self) -> std::result::Result<WindowHandle<'_>, HandleError> {
        let handle = Win32WindowHandle::new(
            NonZeroIsize::new(self.handle.0 as isize).ok_or(HandleError::Unavailable)?,
        );
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(handle)) })
    }
}

impl Window {
    pub fn new(
        parent: Option<RawWindowHandle>,
        options: &Value,
        events: Arc<Events>,
    ) -> anyhow::Result<Self> {
        let parent = match parent {
            Some(RawWindowHandle::Win32(parent)) => Some(HWND(parent.hwnd.get() as _)),
            Some(_) => bail!("WebView2 requires a Win32 window"),
            None => None,
        };
        let mut state = Box::new(WindowState {
            events,
            closed: AtomicBool::new(false),
            loading: Arc::new(AtomicBool::new(false)),
        });
        let handle = unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance.into(),
                lpszClassName: w!("AppsSystemWebView"),
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                ..Default::default()
            };
            RegisterClassW(&class);
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class.lpszClassName,
                &HSTRING::from(options["title"].as_str().unwrap_or("Browser")),
                (if parent.is_some() {
                    WS_CHILD
                } else {
                    WS_OVERLAPPEDWINDOW
                }) | WS_CLIPCHILDREN
                    | WS_CLIPSIBLINGS,
                0,
                0,
                900,
                650,
                parent,
                None,
                Some(instance.into()),
                Some((&mut *state as *mut WindowState).cast()),
            )?
        };
        if parent.is_some() || options["show"] != false {
            unsafe {
                let _ = ShowWindow(handle, SW_SHOW);
            }
        }
        Ok(Self {
            handle,
            state,
            bounds: Cell::new(None),
            attached: Cell::new(parent.is_some()),
            visible: options["show"] != false,
            shown: Cell::new(true),
        })
    }

    pub fn build(
        &self,
        builder: WebViewBuilder<'_>,
        events: Arc<Events>,
    ) -> anyhow::Result<WebView> {
        let view = builder.build(self).context("Cannot start the installed Microsoft Edge WebView2 Runtime; install the Evergreen Runtime if it is missing")?;
        let native = view.webview();
        let mut token = 0;
        let started = events.clone();
        let loading = self.state.loading.clone();
        unsafe {
            native.add_NavigationStarting(
                &NavigationStartingEventHandler::create(Box::new(move |view, _| {
                    loading.store(true, Ordering::Relaxed);
                    if let Some(view) = view {
                        let mut event = navigation(&view)?;
                        event["type"] = json!("did-start-loading");
                        started.push(event);
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            let completed = events.clone();
            let loading = self.state.loading.clone();
            native.add_NavigationCompleted(
                &NavigationCompletedEventHandler::create(Box::new(move |view, args| {
                    loading.store(false, Ordering::Relaxed);
                    if let (Some(view), Some(args)) = (view, args) {
                        let mut success = BOOL::default();
                        args.IsSuccess(&mut success)?;
                        let mut event = navigation(&view)?;
                        if success.as_bool() {
                            event["type"] = json!("did-finish-load");
                        } else {
                            let mut status = COREWEBVIEW2_WEB_ERROR_STATUS::default();
                            args.WebErrorStatus(&mut status)?;
                            event["type"] = json!("did-fail-load");
                            event["error"] =
                                json!(format!("WebView2 navigation failed ({})", status.0));
                            event["code"] = json!(status.0);
                        }
                        completed.push(event);
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            let titles = events.clone();
            native.add_DocumentTitleChanged(
                &DocumentTitleChangedEventHandler::create(Box::new(move |view, _| {
                    if let Some(view) = view {
                        let mut event = navigation(&view)?;
                        event["type"] = json!("page-title-updated");
                        titles.push(event);
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            let history = events.clone();
            native.add_HistoryChanged(
                &HistoryChangedEventHandler::create(Box::new(move |view, _| {
                    if let Some(view) = view {
                        let mut event = navigation(&view)?;
                        event["type"] = json!("did-navigate");
                        history.push(event);
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            native.add_ProcessFailed(
                &ProcessFailedEventHandler::create(Box::new(move |_, _| {
                    events.push(json!({"type":"render-process-gone"}));
                    Ok(())
                })),
                &mut token,
            )?;
        }
        Ok(view)
    }

    pub fn attach(&self, parent: RawWindowHandle) -> anyhow::Result<()> {
        let RawWindowHandle::Win32(parent) = parent else {
            bail!("WebView2 requires a Win32 window");
        };
        unsafe {
            SetParent(self.handle, Some(HWND(parent.hwnd.get() as _)))?;
            SetWindowLongPtrW(
                self.handle,
                GWL_STYLE,
                (WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS).0 as isize,
            );
            SetWindowPos(
                self.handle,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            )?;
        }
        self.attached.set(true);
        let bounds = self.bounds.replace(None).unwrap_or([0., 0., 800., 600.]);
        self.set_bounds(bounds)
    }

    pub fn detach(&self) -> anyhow::Result<()> {
        unsafe {
            let _ = ShowWindow(self.handle, SW_HIDE);
            SetParent(self.handle, None)?;
        }
        self.attached.set(false);
        Ok(())
    }

    pub fn set_visible(&self, visible: bool) -> anyhow::Result<()> {
        self.shown.set(visible);
        let bounds = self.bounds.replace(None).unwrap_or([0., 0., 800., 600.]);
        self.set_bounds(bounds)
    }

    pub fn set_bounds(&self, bounds: [f64; 4]) -> anyhow::Result<()> {
        if self.bounds.get() == Some(bounds) {
            return Ok(());
        }
        let [x, y, width, height] = bounds;
        unsafe {
            let scale = GetDpiForWindow(self.handle).max(96) as f64 / 96.;
            SetWindowPos(
                self.handle,
                None,
                (x * scale).round() as i32,
                (y * scale).round() as i32,
                ((width * scale).round() as i32).max(1),
                ((height * scale).round() as i32).max(1),
                SWP_NOZORDER | SWP_NOACTIVATE,
            )?;
            let _ = ShowWindow(
                self.handle,
                if (self.attached.get() || self.visible)
                    && self.shown.get()
                    && !self.state.closed.load(Ordering::Relaxed)
                    && width > 0.
                    && height > 0.
                {
                    SW_SHOWNA
                } else {
                    SW_HIDE
                },
            );
        }
        self.bounds.set(Some(bounds));
        Ok(())
    }

    pub fn command(
        &self,
        view: &WebView,
        operation: &str,
        arguments: &Value,
        events: &Arc<Events>,
    ) -> anyhow::Result<Value> {
        let native = view.webview();
        unsafe {
            match operation {
                "loadHTML" => {
                    let html = arguments["html"].as_str().unwrap();
                    if let Some(base) = arguments["baseURL"]
                        .as_str()
                        .filter(|base| *base != "about:blank")
                    {
                        let document = dom_query::Document::from(html);
                        let base = base
                            .replace('&', "&amp;")
                            .replace('"', "&quot;")
                            .replace('<', "&lt;");
                        document
                            .select("head")
                            .prepend_html(format!("<base href=\"{base}\">"));
                        view.load_html(&document.html())?;
                    } else {
                        view.load_html(html)?;
                    }
                }
                "inspect" => {
                    native.Settings()?.SetAreDevToolsEnabled(true)?;
                    native.OpenDevToolsWindow()?;
                }
                "back" => native.GoBack()?,
                "forward" => native.GoForward()?,
                "reload" => native.Reload()?,
                "stop" => native.Stop()?,
                "clearStorage" => {
                    let events = events.clone();
                    let id = arguments["id"].clone();
                    native
                        .cast::<ICoreWebView2_13>()?
                        .Profile()?
                        .cast::<ICoreWebView2Profile2>()?
                        .ClearBrowsingDataAll(&ClearBrowsingDataCompletedHandler::create(
                            Box::new(move |result| {
                                events
                                    .complete(&id, result.map(|_| Value::Null).map_err(Into::into));
                                Ok(())
                            }),
                        ))?;
                }
                "capture" => {
                    let stream = SHCreateMemStream(None)
                        .context("Cannot allocate browser snapshot stream")?;
                    let output = stream.clone();
                    let events = events.clone();
                    let arguments = arguments.clone();
                    let scale = GetDpiForWindow(self.handle).max(96) as f64 / 96.;
                    native.CapturePreview(
                        COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
                        &stream,
                        &CapturePreviewCompletedHandler::create(Box::new(move |result| {
                            let png = (|| -> anyhow::Result<Vec<u8>> {
                                result?;
                                let mut stat = STATSTG::default();
                                output.Stat(&mut stat, STATFLAG_NONAME)?;
                                ensure!(
                                    stat.cbSize <= 64 * 1024 * 1024,
                                    "Browser snapshot exceeds 64 MiB"
                                );
                                output.Seek(0, STREAM_SEEK_SET, None)?;
                                let mut bytes = vec![0; stat.cbSize as usize];
                                let mut read = 0;
                                output
                                    .Read(
                                        bytes.as_mut_ptr().cast(),
                                        bytes.len() as u32,
                                        Some(&mut read),
                                    )
                                    .ok()?;
                                bytes.truncate(read as usize);
                                Ok(bytes)
                            })();
                            events.capture(&arguments, png, scale);
                            Ok(())
                        })),
                    )?;
                }
                "state" => {
                    let mut state = navigation(&native)?;
                    let mut zoom = 1.;
                    view.controller().ZoomFactor(&mut zoom)?;
                    state["zoomFactor"] = json!(zoom);
                    state["loading"] = json!(self.state.loading.load(Ordering::Relaxed));
                    state["closed"] = json!(self.state.closed.load(Ordering::Relaxed));
                    return Ok(state);
                }
                _ => bail!("Unknown browser operation: {operation}"),
            }
        }
        Ok(Value::Null)
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.handle);
        }
    }
}
