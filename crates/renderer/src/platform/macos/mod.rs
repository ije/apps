// AppKit screen coordinates grow upward from the bottom left of the primary display. The window code flips
// them to the top-left origin of the core.

mod controls;
mod events;
mod glass;
mod metal;
mod nsview;
mod nswindow;
mod open_type;
pub(crate) mod symbols;
mod text;
mod threads;
mod vsync;
mod window;

use crate::{Cursor, Rect, WindowOptions, app::Callbacks};
use anyhow::Result;
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::{Retained, autoreleasepool},
    runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject},
};
use objc2_app_kit::{
    NSApp, NSApplicationActivationPolicy, NSApplicationDelegate, NSApplicationTerminateReply,
    NSPasteboard, NSPasteboardTypeString, NSScreen,
};
use objc2_foundation::{NSArray, NSString, NSURL, NSUserDefaults};
use std::{
    cell::RefCell,
    sync::{Mutex, MutexGuard},
};

pub(crate) use controls::control_size;
pub(crate) use glass::available as supports_glass;
pub(crate) use text::TextSystem;
pub(crate) use threads::{
    AppThread, app_thread, ask, end_thread, native_after, native_post, native_sync, park_render,
    post, run_after, spawn_render,
};
pub(crate) use window::Window;
pub use window::WindowControl;

/// Moves a value to another thread whole, or carries it to the one thread that may use it.
pub(crate) struct Sendable<T>(T);

// SAFETY: see the type's doc: every use hands the value over or runs it where it belongs.
unsafe impl<T> Send for Sendable<T> {}

impl<T> Sendable<T> {
    pub fn new(value: T) -> Self {
        Self(value)
    }
    pub fn take(self) -> T {
        self.0
    }
}

// A panic while a lock is held leaves its data usable here.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing rules, and the class has no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AppsApplicationDelegate"]
    #[ivars = RefCell<Option<Box<dyn FnOnce()>>>]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSApplicationDelegate for Delegate {
        #[unsafe(method(applicationWillFinishLaunching:))]
        fn will_launch(&self, _: &AnyObject) {
            let defaults = NSUserDefaults::standardUserDefaults();
            let key = NSString::from_str("NSAutoFillHeuristicControllerEnabled");
            if defaults.objectForKey(&key).is_none() {
                defaults.setBool_forKey(false, &key);
            }
        }

        #[unsafe(method(applicationDidFinishLaunching:))]
        fn launched(&self, _: &AnyObject) {
            // Only after the run loop started: an unbundled app set up earlier can get a menu bar that takes no clicks.
            NSApp(self.mtm()).setActivationPolicy(NSApplicationActivationPolicy::Regular);
            window::measure_screens();
            let launch = self.ivars().take();
            if let Some(launch) = launch {
                launch();
            }
        }

        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn reopen(&self, _: &AnyObject, visible: bool) -> bool {
            crate::native::reopen(visible);
            true
        }

        #[unsafe(method(applicationShouldTerminate:))]
        fn may_quit(&self, _: &AnyObject) -> NSApplicationTerminateReply {
            match crate::native::should_quit() {
                true => NSApplicationTerminateReply::TerminateNow,
                false => NSApplicationTerminateReply::TerminateCancel,
            }
        }

        #[unsafe(method(applicationWillTerminate:))]
        fn quitting(&self, _: &AnyObject) {
            park_render();
            crate::native::quit_handler();
        }

        #[unsafe(method(application:openURLs:))]
        fn open(&self, _: &AnyObject, urls: &NSArray<NSURL>) {
            let urls = urls.iter().filter_map(|url| Some(url.absoluteString()?.to_string()));
            crate::native::open_urls(urls.collect());
        }
    }
);

pub(crate) struct Platform;

impl Platform {
    pub fn new() -> Self {
        Self
    }

    pub fn run(&self, on_launch: Box<dyn FnOnce()>) {
        threads::main_source();
        let mtm = MainThreadMarker::new().expect("the app must start on the main thread");
        let delegate = Delegate::alloc(mtm).set_ivars(RefCell::new(Some(on_launch)));
        // SAFETY: a plain NSObject init.
        let delegate: Retained<Delegate> = unsafe { msg_send![super(delegate), init] };
        // AppKit keeps only a weak reference, and `delegate` lives until the loop ends.
        NSApp(mtm).setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        autoreleasepool(|_| NSApp(mtm).run());
    }

    pub fn quit(&self) {
        // After the current event: terminating closes windows, and their callbacks need the app.
        native_post(Box::new(|| {
            threads::post_local(|| NSApp(MainThreadMarker::new().unwrap()).terminate(None))
        }));
    }

    pub fn activate(&self, ignoring_other_apps: bool) {
        native_post(Box::new(move || {
            #[allow(deprecated)]
            NSApp(MainThreadMarker::new().unwrap()).activateIgnoringOtherApps(ignoring_other_apps);
        }));
    }

    pub fn read_clipboard(&self) -> Option<String> {
        // SAFETY: the type is an AppKit constant.
        native_sync(|| unsafe {
            NSPasteboard::generalPasteboard()
                .stringForType(NSPasteboardTypeString)
                .map(|text| text.to_string())
        })
    }

    pub fn write_clipboard(&self, text: &str) {
        native_sync(|| {
            let board = NSPasteboard::generalPasteboard();
            board.clearContents();
            // SAFETY: the type is an AppKit constant.
            unsafe { board.setString_forType(&NSString::from_str(text), NSPasteboardTypeString) };
        });
    }

    pub fn set_cursor(&self, cursor: Cursor) {
        native_post(Box::new(move || nsview::set_cursor(cursor)));
    }

    pub fn primary_display_bounds(&self) -> Option<Rect> {
        native_sync(|| {
            let primary = NSScreen::screens(MainThreadMarker::new().unwrap()).firstObject()?;
            Some(window::flip(
                primary.visibleFrame(),
                primary.frame().size.height,
            ))
        })
    }

    pub fn open_window(&self, options: &WindowOptions, callbacks: Callbacks) -> Result<Window> {
        Window::open(options, callbacks)
    }
}
