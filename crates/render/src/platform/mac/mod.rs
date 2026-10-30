//! macOS: AppKit windows, Metal rendering and CoreText.
//!
//! macOS screens have a y axis that goes up from the bottom, with the origin at the bottom left
//! of the main display.

mod display_link;
mod events;
mod metal;
mod open_type;
mod text;
mod window;

use crate::{Bounds, CursorStyle, Pixels, WindowOptions, app::Callbacks, point, px, size};
use anyhow::Result;
use core_foundation::string::CFStringRef;
use dispatch2::{DispatchQueue, DispatchTime};
use objc2::{
    Encode, Encoding, MainThreadMarker, RefEncode, class, msg_send,
    rc::{Retained, autoreleasepool},
    runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel},
    sel,
};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSPasteboard, NSPasteboardTypeString, NSScreen,
};
use objc2_foundation::{NSArray, NSNotFound, NSString, NSUInteger, NSURL};
use std::{
    cell::RefCell,
    ffi::{CStr, c_void},
    ops::Range,
    sync::OnceLock,
    time::Duration,
};

pub(crate) use text::TextSystem;
pub(crate) use window::Window;

thread_local! {
    static LAUNCH: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

fn delegate_class() -> &'static AnyClass {
    static CLASS: OnceLock<usize> = OnceLock::new();
    let class = *CLASS.get_or_init(|| {
        let mut decl = ClassBuilder::new(c"AppsApplicationDelegate", class!(NSResponder)).unwrap();
        // SAFETY: the methods have the signatures AppKit calls them with.
        unsafe {
            decl.add_method(
                sel!(applicationWillFinishLaunching:),
                will_finish_launching as extern "C-unwind" fn(_, Sel, *mut AnyObject),
            );
            decl.add_method(
                sel!(applicationDidFinishLaunching:),
                did_finish_launching as extern "C-unwind" fn(_, Sel, *mut AnyObject),
            );
            decl.add_method(
                sel!(applicationShouldHandleReopen:hasVisibleWindows:),
                should_handle_reopen as extern "C-unwind" fn(_, Sel, *mut AnyObject, Bool) -> Bool,
            );
            decl.add_method(
                sel!(applicationShouldTerminate:),
                should_terminate as extern "C-unwind" fn(_, Sel, *mut AnyObject) -> usize,
            );
            decl.add_method(
                sel!(applicationWillTerminate:),
                will_terminate as extern "C-unwind" fn(_, Sel, *mut AnyObject),
            );
            decl.add_method(
                sel!(application:openURLs:),
                open_urls as extern "C-unwind" fn(_, Sel, *mut AnyObject, *mut AnyObject),
            );
        }
        decl.register() as *const AnyClass as usize
    });
    // SAFETY: registered classes live for the whole process.
    unsafe { &*(class as *const AnyClass) }
}

pub(crate) struct Platform {
    marker: MainThreadMarker,
}

impl Platform {
    pub fn new() -> Self {
        Self {
            marker: MainThreadMarker::new().expect("the app must start on the main thread"),
        }
    }

    pub fn run(&self, on_launch: Box<dyn FnOnce()>) {
        LAUNCH.with_borrow_mut(|launch| *launch = Some(on_launch));
        let app = NSApplication::sharedApplication(self.marker);
        // SAFETY: the delegate is a fresh instance of a registered NSResponder subclass.
        unsafe {
            let delegate: *mut AnyObject = msg_send![delegate_class(), new];
            let _: () = msg_send![&app, setDelegate: delegate];
        }
        autoreleasepool(|_| app.run());
    }

    pub fn quit(&self) {
        // WARNING: terminating closes windows, whose callbacks need the app, so it runs after
        // the current borrow of the app ends.
        post_local(|| {
            let app = NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
            app.terminate(None);
        });
    }

    pub fn activate(&self, ignoring_other_apps: bool) {
        #[allow(deprecated)]
        NSApplication::sharedApplication(self.marker)
            .activateIgnoringOtherApps(ignoring_other_apps);
    }

    pub fn read_clipboard(&self) -> Option<String> {
        // SAFETY: the type is an AppKit constant.
        let text =
            unsafe { NSPasteboard::generalPasteboard().stringForType(NSPasteboardTypeString) }?;
        Some(text.to_string())
    }

    pub fn write_clipboard(&self, text: &str) {
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        // SAFETY: the type is an AppKit constant.
        unsafe { pasteboard.setString_forType(&NSString::from_str(text), NSPasteboardTypeString) };
    }

    pub fn set_cursor(&self, style: CursorStyle) {
        window::set_cursor(style);
    }

    pub fn primary_display_bounds(&self) -> Option<Bounds<Pixels>> {
        let screens = NSScreen::screens(self.marker);
        let primary = screens.iter().next()?;
        let visible = primary.visibleFrame();
        let height = primary.frame().size.height;
        Some(Bounds::new(
            point(
                px(visible.origin.x as f32),
                px((height - visible.origin.y - visible.size.height) as f32),
            ),
            size(
                px(visible.size.width as f32),
                px(visible.size.height as f32),
            ),
        ))
    }

    pub fn open_window(&self, options: &WindowOptions, callbacks: Callbacks) -> Result<Window> {
        Window::open(options, callbacks, self.marker)
    }
}

extern "C" fn local(context: *mut c_void) {
    // SAFETY: the context is the box made by `run_after` or `post_local`.
    let f = unsafe { Box::from_raw(context as *mut Box<dyn FnOnce()>) };
    f();
}

extern "C" fn remote(context: *mut c_void) {
    // SAFETY: the context is the box made by `post`.
    let f = unsafe { Box::from_raw(context as *mut Box<dyn FnOnce() + Send>) };
    f();
}

/// Runs `f` on the main thread, from any thread.
pub(crate) fn post(f: Box<dyn FnOnce() + Send>) {
    let context = Box::into_raw(Box::new(f)) as *mut c_void;
    // SAFETY: `remote` takes back the box on the main queue.
    unsafe { DispatchQueue::main().exec_async_f(context, remote) };
}

/// Runs `f` on the main thread after `delay`.
pub(crate) fn run_after(delay: Duration, f: impl FnOnce() + 'static) {
    let f: Box<dyn FnOnce()> = Box::new(f);
    let context = Box::into_raw(Box::new(f)) as *mut c_void;
    let when = DispatchTime::NOW.time(delay.as_nanos().min(i64::MAX as u128) as i64);
    // SAFETY: `local` takes back the box on the main queue, which runs on this thread.
    unsafe { DispatchQueue::exec_after_f(when, DispatchQueue::main(), context, local) };
}

/// Runs `f` on the main thread after the current event.
pub(crate) fn post_local(f: impl FnOnce() + 'static) {
    let f: Box<dyn FnOnce()> = Box::new(f);
    let context = Box::into_raw(Box::new(f)) as *mut c_void;
    // SAFETY: `local` takes back the box on the main queue, which runs on this thread.
    unsafe { DispatchQueue::main().exec_async_f(context, local) };
}

extern "C-unwind" fn will_finish_launching(_: &AnyObject, _: Sel, _: *mut AnyObject) {
    // SAFETY: plain NSUserDefaults calls.
    unsafe {
        // The autofill heuristic controller costs CPU on every key press.
        let defaults: *mut AnyObject = msg_send![class!(NSUserDefaults), standardUserDefaults];
        let name = ns_string("NSAutoFillHeuristicControllerEnabled");
        let existing: *mut AnyObject = msg_send![defaults, objectForKey: name];
        if existing.is_null() {
            let value: *mut AnyObject = msg_send![class!(NSNumber), numberWithBool: Bool::NO];
            let _: () = msg_send![defaults, setObject: value, forKey: name];
        }
    }
}

extern "C-unwind" fn did_finish_launching(_: &AnyObject, _: Sel, _: *mut AnyObject) {
    let app = NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
    // WARNING: the policy is set here and not before the run loop, or the menu bar of an
    // unbundled app started from a terminal can't be clicked.
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    if let Some(launch) = LAUNCH.with_borrow_mut(Option::take) {
        launch();
    }
}

extern "C-unwind" fn should_handle_reopen(
    _: &AnyObject,
    _: Sel,
    _: *mut AnyObject,
    visible: Bool,
) -> Bool {
    crate::app::events::reopen(visible.as_bool());
    Bool::YES
}

/// Returns `NSTerminateNow` (1) or `NSTerminateCancel` (0).
extern "C-unwind" fn should_terminate(_: &AnyObject, _: Sel, _: *mut AnyObject) -> usize {
    crate::app::events::should_quit() as usize
}

extern "C-unwind" fn will_terminate(_: &AnyObject, _: Sel, _: *mut AnyObject) {
    crate::app::events::quit();
}

extern "C-unwind" fn open_urls(_: &AnyObject, _: Sel, _: *mut AnyObject, urls: *mut AnyObject) {
    // SAFETY: AppKit passes an NSArray of NSURL.
    let urls = unsafe { urls.cast::<NSArray<NSURL>>().as_ref() }
        .into_iter()
        .flat_map(|urls| urls.iter())
        .filter_map(|url| Some(url.absoluteString()?.to_string()))
        .collect::<Vec<_>>();
    crate::app::events::open_urls(urls);
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentKeyboardLayoutInputSource() -> *mut AnyObject;
    fn TISCopyCurrentKeyboardInputSource() -> *mut AnyObject;
    fn TISGetInputSourceProperty(source: *mut AnyObject, key: *const c_void) -> *mut AnyObject;
    fn UCKeyTranslate(
        layout: *const c_void,
        code: u16,
        action: u16,
        modifiers: u32,
        keyboard_type: u32,
        options: u32,
        dead_key_state: *mut u32,
        max_length: usize,
        length: *mut usize,
        string: *mut u16,
    ) -> u32;
    fn LMGetKbdType() -> u16;
    static kTISPropertyUnicodeKeyLayoutData: CFStringRef;
    static kTISPropertyInputSourceIsASCIICapable: CFStringRef;
    static kTISPropertyInputSourceType: CFStringRef;
    static kTISTypeKeyboardInputMode: CFStringRef;
}

trait NSStringExt {
    unsafe fn to_str(&self) -> &str;
}

impl NSStringExt for *mut AnyObject {
    unsafe fn to_str(&self) -> &str {
        // SAFETY: the caller passes an NSString or nil.
        unsafe {
            let Some(string) = self.cast::<NSString>().as_ref() else {
                return "";
            };
            let text = string.UTF8String();
            if text.is_null() {
                ""
            } else {
                CStr::from_ptr(text).to_str().unwrap_or("")
            }
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
struct NSRange {
    location: NSUInteger,
    length: NSUInteger,
}

impl NSRange {
    fn invalid() -> Self {
        Self {
            location: NSNotFound as NSUInteger,
            length: 0,
        }
    }

    fn to_range(self) -> Option<Range<usize>> {
        (self.location != NSNotFound as NSUInteger)
            .then(|| self.location..self.location + self.length)
    }
}

impl From<Range<usize>> for NSRange {
    fn from(range: Range<usize>) -> Self {
        Self {
            location: range.start as NSUInteger,
            length: range.len() as NSUInteger,
        }
    }
}

// SAFETY: the layout matches Foundation's NSRange.
unsafe impl Encode for NSRange {
    const ENCODING: Encoding = objc2_foundation::NSRange::ENCODING;
}

// SAFETY: the layout matches Foundation's NSRange.
unsafe impl RefEncode for NSRange {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}

fn ns_string(string: &str) -> *mut AnyObject {
    Retained::autorelease_ptr(NSString::from_str(string)).cast()
}
