use super::*;
use ::wry::{WebViewBuilderExtUnix, WebViewExtUnix};
use gtk::{gdk, gio, glib, prelude::*};
use std::cell::Cell;
use webkit2gtk::{
    LoadEvent, SettingsExt, SnapshotOptions, SnapshotRegion, WebViewExt,
    WebsiteDataManagerExtManual, WebsiteDataTypes,
};

pub(super) struct Window {
    window: gtk::Window,
    attached: Cell<bool>,
    visible: bool,
    shown: Cell<bool>,
    bounds: Cell<Option<[f64; 4]>>,
    closed: Rc<Cell<bool>>,
}

impl Window {
    pub fn new(
        parent: Option<RawWindowHandle>,
        options: &Value,
        events: Arc<Events>,
    ) -> Result<Self> {
        if let Some(parent) = parent {
            ensure!(
                matches!(parent, RawWindowHandle::Xlib(_) | RawWindowHandle::Xcb(_)),
                "WebKitGTK embedding requires X11 or XWayland; start the app with WAYLAND_DISPLAY unset and GDK_BACKEND=x11"
            );
        }
        if !gtk::is_initialized() {
            gdk::set_allowed_backends("x11");
        }
        gtk::init().context("WebKitGTK requires an X11 display and GTK on the main thread")?;
        ensure!(
            gdk::Display::default().is_some_and(|display| display.is::<gdkx11::X11Display>()),
            "WebKitGTK embedding requires the GDK X11 backend"
        );
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_title(options["title"].as_str().unwrap_or("Browser"));
        window.set_default_size(900, 650);
        window.realize();
        let closed = Rc::new(Cell::new(false));
        let close = closed.clone();
        window.connect_delete_event(move |window, _| {
            close.set(true);
            window.hide();
            events.push(json!({"type":"closed"}));
            glib::Propagation::Stop
        });
        let result = Self {
            window,
            attached: Cell::new(false),
            visible: options["show"] != false,
            shown: Cell::new(true),
            bounds: Cell::new(None),
            closed,
        };
        if let Some(parent) = parent {
            result.attach(parent)?;
        } else if options["show"] != false {
            result.window.show();
        }
        Ok(result)
    }

    pub fn build(&self, builder: WebViewBuilder<'_>, events: Arc<Events>) -> Result<WebView> {
        let view = builder.build_gtk(&self.window)?;
        let webkit = view.webview();
        let failed = Rc::new(Cell::new(false));
        let failure = failed.clone();
        let failures = events.clone();
        webkit.connect_load_failed(move |_, _, url, error| {
            failure.set(true);
            failures.push(json!({"type":"did-fail-load", "url":url, "error":error.to_string()}));
            true
        });
        let loads = events.clone();
        webkit.connect_load_changed(move |view, event| {
            match event {
                LoadEvent::Started => {
                    failed.set(false);
                    loads.push(json!({"type":"did-start-loading", "url":view.uri().unwrap_or_default().as_str()}));
                }
                LoadEvent::Finished if !failed.get() => loads.push(json!({"type":"did-finish-load", "url":view.uri().unwrap_or_default().as_str(), "title":view.title().unwrap_or_default().as_str(), "canGoBack":view.can_go_back(), "canGoForward":view.can_go_forward()})),
                _ => {}
            }
        });
        let titles = events.clone();
        webkit.connect_title_notify(move |view| {
            titles.push(
                json!({"type":"page-title-updated", "title":view.title().unwrap_or_default().as_str()}),
            )
        });
        webkit.connect_web_process_terminated(move |_, _| {
            events.push(json!({"type":"render-process-gone"}))
        });
        webkit.show();
        Ok(view)
    }

    pub fn attach(&self, parent: RawWindowHandle) -> Result<()> {
        let xid = match parent {
            RawWindowHandle::Xlib(parent) => parent.window,
            RawWindowHandle::Xcb(parent) => parent.window.get() as _,
            _ => bail!(
                "WebKitGTK embedding requires X11 or XWayland; start the app with WAYLAND_DISPLAY unset and GDK_BACKEND=x11"
            ),
        };
        let display = self
            .window
            .display()
            .downcast::<gdkx11::X11Display>()
            .map_err(|_| anyhow::anyhow!("Expected an X11 display"))?;
        let parent = gdkx11::X11Window::foreign_new_for_display(&display, xid);
        let native = self
            .window
            .window()
            .context("Browser window is unavailable")?;
        self.window.hide();
        self.window.set_decorated(false);
        native.set_override_redirect(true);
        native.reparent(parent.upcast_ref(), 0, 0);
        native.raise();
        self.attached.set(true);
        let bounds = self.bounds.replace(None).unwrap_or([0., 0., 800., 600.]);
        self.set_bounds(bounds)
    }

    pub fn detach(&self) -> Result<()> {
        self.window.hide();
        let native = self
            .window
            .window()
            .context("Browser window is unavailable")?;
        native.reparent(
            &native
                .screen()
                .root_window()
                .context("Browser root window is unavailable")?,
            0,
            0,
        );
        self.attached.set(false);
        Ok(())
    }

    pub fn set_visible(&self, visible: bool) -> Result<()> {
        self.shown.set(visible);
        let bounds = self.bounds.replace(None).unwrap_or([0., 0., 800., 600.]);
        self.set_bounds(bounds)
    }

    pub fn set_bounds(&self, bounds: [f64; 4]) -> Result<()> {
        if self.bounds.get() == Some(bounds) {
            return Ok(());
        }
        let [x, y, width, height] = bounds;
        self.window.set_visible(
            (self.attached.get() || self.visible)
                && self.shown.get()
                && !self.closed.get()
                && width > 0.
                && height > 0.,
        );
        let width = (width as i32).max(1);
        let height = (height as i32).max(1);
        self.window.resize(width, height);
        if self.attached.get() {
            self.window
                .window()
                .context("Browser window is unavailable")?
                .move_resize(x as i32, y as i32, width, height);
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
    ) -> Result<()> {
        let webkit = view.webview();
        match operation {
            "loadHTML" => webkit.load_html(
                arguments["html"].as_str().unwrap(),
                arguments["baseURL"].as_str(),
            ),
            "inspect" => {
                WebViewExt::settings(&webkit)
                    .context("Browser settings are unavailable")?
                    .set_enable_developer_extras(true);
                view.open_devtools();
            }
            "back" => webkit.go_back(),
            "forward" => webkit.go_forward(),
            "reload" => webkit.reload(),
            "stop" => webkit.stop_loading(),
            "clearStorage" => {
                let manager = webkit
                    .website_data_manager()
                    .context("Browser data store is unavailable")?;
                let events = events.clone();
                let id = arguments["id"].clone();
                manager.clear(
                    WebsiteDataTypes::ALL,
                    glib::TimeSpan::from_seconds(0),
                    None::<&gio::Cancellable>,
                    move |result| {
                        events.complete(&id, result.map(|_| Value::Null).map_err(Into::into))
                    },
                );
            }
            "capture" => {
                let events = events.clone();
                let arguments = arguments.clone();
                let scale = webkit.scale_factor() as f64;
                webkit.snapshot(
                    SnapshotRegion::Visible,
                    SnapshotOptions::NONE,
                    None::<&gio::Cancellable>,
                    move |result| {
                        let png = (|| -> Result<Vec<u8>> {
                            let surface = gtk::cairo::ImageSurface::try_from(result?)
                                .map_err(|_| anyhow::anyhow!("Browser snapshot is not an image"))?;
                            let pixels = gdk::pixbuf_get_from_surface(
                                &surface,
                                0,
                                0,
                                surface.width(),
                                surface.height(),
                            )
                            .context("Browser snapshot is empty")?;
                            Ok(pixels.save_to_bufferv("png", &[])?)
                        })();
                        events.capture(&arguments, png, scale);
                    },
                );
            }
            _ => bail!("Unknown browser operation: {operation}"),
        }
        Ok(())
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            self.window.destroy();
        }
    }
}

#[cfg(test)]
impl raw_window_handle::HasWindowHandle for Window {
    fn window_handle(
        &self,
    ) -> std::result::Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError>
    {
        let native = self
            .window
            .window()
            .ok_or(raw_window_handle::HandleError::Unavailable)?
            .downcast::<gdkx11::X11Window>()
            .map_err(|_| raw_window_handle::HandleError::Unavailable)?;
        let handle = raw_window_handle::XlibWindowHandle::new(native.xid());
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(RawWindowHandle::Xlib(handle)) })
    }
}
