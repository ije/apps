// WARNING: macOS screens have a y axis that goes up from the bottom, with the origin at the
// bottom left of the main display.

mod display_link;
mod events;
mod glass;
mod metal;
mod open_type;
pub(crate) mod symbols;
mod text;
mod window;

use crate::{Bounds, CursorStyle, Pixels, WindowOptions, app::Callbacks, point, px, size};
use anyhow::Result;
use core_foundation::string::CFStringRef;
use core_foundation_sys::{
    base::{CFRelease, kCFAllocatorDefault},
    date::CFAbsoluteTimeGetCurrent,
    runloop::{
        CFRunLoopAddSource, CFRunLoopAddTimer, CFRunLoopGetMain, CFRunLoopSourceContext,
        CFRunLoopSourceCreate, CFRunLoopSourceRef, CFRunLoopSourceSignal, CFRunLoopTimerContext,
        CFRunLoopTimerCreate, CFRunLoopTimerRef, CFRunLoopTimerSetTolerance, CFRunLoopWakeUp,
        kCFRunLoopCommonModes,
    },
};
use dispatch2::DispatchQueue;
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
use objc2_quartz_core::CATransaction;
use std::{
    cell::{Cell, RefCell},
    collections::{BinaryHeap, VecDeque},
    ffi::{CStr, c_void},
    ops::Range,
    sync::{
        Arc, Condvar, Mutex, OnceLock, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

pub(crate) use glass::available as supports_glass;
pub(crate) use text::TextSystem;
pub(crate) use window::Window;
pub use window::WindowControl;

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

type Task = Box<dyn FnOnce() + Send>;

pub(crate) struct Inbox {
    tasks: Mutex<VecDeque<Task>>,
    ready: Condvar,
    late: AtomicUsize,
    closed: AtomicBool,
}

impl Inbox {
    const fn new() -> Self {
        Self {
            tasks: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
            late: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
        }
    }

    fn push(&self, task: Task) {
        let mut tasks = self.tasks.lock().unwrap_or_else(PoisonError::into_inner);
        if !self.closed.load(Ordering::Relaxed) {
            tasks.push_back(task);
        }
        drop(tasks);
        self.ready.notify_all();
    }

    fn pop(&self) -> Option<Task> {
        self.tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
    }
}

// WARNING: A render thread, which owns one `App`: a window and its page scripts. It is leaked, because
// `Callbacks` of native views can name it after it ends, and it is small.
pub(crate) type AppThread = &'static Inbox;

// WARNING: the work of the main thread, which owns AppKit. `ask` runs it too, so a page that waits for the
// main thread never waits for an `ask` of that thread to time out.
static MAIN: Inbox = Inbox::new();
static MAIN_QUEUED: AtomicBool = AtomicBool::new(false);
static RENDERS: Mutex<Vec<AppThread>> = Mutex::new(Vec::new());
static RENDER_LEFT: Condvar = Condvar::new();

// WARNING: how long the main thread waits for the render thread to answer AppKit. A busy page must not
// freeze the menus and the window, so AppKit then gets the answer for a page that ignores it.
const ASK_LIMIT: Duration = Duration::from_millis(100);

pub(crate) struct Sendable<T>(T);
// SAFETY: each use hands the value over whole, or runs it on the thread that may touch it.
unsafe impl<T> Send for Sendable<T> {}
impl<T> Sendable<T> {
    pub fn new(value: T) -> Self {
        Self(value)
    }
    pub fn take(self) -> T {
        self.0
    }
}

// WARNING: the main thread's work runs from a run loop source in the common modes, and not from the
// main dispatch queue, which AppKit doesn't serve in a loop nested in one of its blocks. So menus,
// popups and modal panels that a main script opens don't stop windows, IPC and timers.
fn main_source() -> CFRunLoopSourceRef {
    static SOURCE: OnceLock<usize> = OnceLock::new();
    *SOURCE.get_or_init(|| {
        let mut context = CFRunLoopSourceContext {
            version: 0,
            info: std::ptr::null_mut(),
            retain: None,
            release: None,
            copyDescription: None,
            equal: None,
            hash: None,
            schedule: None,
            cancel: None,
            perform: run_main,
        };
        // SAFETY: the source lives for the whole process, and run loop calls are thread safe.
        unsafe {
            let source = CFRunLoopSourceCreate(kCFAllocatorDefault, 0, &mut context);
            CFRunLoopAddSource(CFRunLoopGetMain(), source, kCFRunLoopCommonModes);
            source as usize
        }
    }) as CFRunLoopSourceRef
}

pub(crate) fn native_post(f: Task) {
    MAIN.push(f);
    if !MAIN_QUEUED.swap(true, Ordering::AcqRel) {
        // SAFETY: the source is valid, and signaling it is thread safe.
        unsafe {
            CFRunLoopSourceSignal(main_source());
            CFRunLoopWakeUp(CFRunLoopGetMain());
        }
    }
}

extern "C" fn run_main(_: *const c_void) {
    MAIN_QUEUED.store(false, Ordering::Release);
    while let Some(task) = MAIN.pop() {
        autoreleasepool(|_| task());
    }
}

pub(crate) fn native_sync<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    if MainThreadMarker::new().is_some() {
        return f();
    }
    let (mut f, mut result) = (Some(f), None);
    wait_for_main(&mut || result = f.take().map(|f| f()));
    result.expect("The main thread dropped its work")
}

fn wait_for_main(task: &mut (dyn FnMut() + Send)) {
    let done = Arc::new((Mutex::new(false), Condvar::new()));
    let finished = done.clone();
    // SAFETY: this thread waits for the task below, so the borrow outlives it.
    let task = Sendable::new(unsafe {
        std::mem::transmute::<&mut (dyn FnMut() + Send), &'static mut (dyn FnMut() + Send)>(task)
    });
    native_post(Box::new(move || {
        task.take()();
        *finished.0.lock().unwrap_or_else(PoisonError::into_inner) = true;
        finished.1.notify_all();
    }));
    let mut finished = done.0.lock().unwrap_or_else(PoisonError::into_inner);
    while !*finished {
        finished = done
            .1
            .wait(finished)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

pub(crate) fn park_render() {
    let threads = RENDERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    for thread in threads {
        post(
            thread,
            Box::new(move || {
                leave_renders(thread);
                loop {
                    std::thread::park();
                }
            }),
        );
    }
    let guard = RENDERS.lock().unwrap_or_else(PoisonError::into_inner);
    let _ = RENDER_LEFT.wait_timeout_while(guard, 3 * ASK_LIMIT, |threads| !threads.is_empty());
}

fn leave_renders(thread: AppThread) {
    RENDERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|other| !std::ptr::eq(*other, thread));
    RENDER_LEFT.notify_all();
}

enum Asked<R> {
    Waiting,
    Answered(R),
    Abandoned,
}

/// Runs `f` on the render thread and waits for its answer on the main thread, for at most
/// `ASK_LIMIT`, while it runs the work of the main thread. `f` always runs, but while an earlier
/// `f` is late nothing waits, so a hung page costs AppKit one wait and not one per question.
pub(crate) fn ask<R: Send + 'static>(
    thread: AppThread,
    f: impl FnOnce() -> R + Send + 'static,
) -> Option<R> {
    ask_within(thread, ASK_LIMIT, f)
}

pub(crate) fn ask_within<R: Send + 'static>(
    thread: AppThread,
    limit: Duration,
    f: impl FnOnce() -> R + Send + 'static,
) -> Option<R> {
    if thread.closed.load(Ordering::Relaxed) {
        return None;
    }
    let slot = Arc::new(Mutex::new(Asked::Waiting));
    let answer = slot.clone();
    post(
        thread,
        Box::new(move || {
            let value = f();
            let mut answer = answer.lock().unwrap_or_else(PoisonError::into_inner);
            if matches!(*answer, Asked::Abandoned) {
                thread.late.fetch_sub(1, Ordering::AcqRel);
            } else {
                *answer = Asked::Answered(value);
            }
            drop(answer);
            // WARNING: the waiter checks the slot while it holds this lock, so taking it here keeps the
            // notification from falling between that check and its wait.
            drop(MAIN.tasks.lock().unwrap_or_else(PoisonError::into_inner));
            MAIN.ready.notify_all();
        }),
    );
    let deadline = Instant::now() + limit;
    let waits = thread.late.load(Ordering::Acquire) == 0;
    let mut tasks = MAIN.tasks.lock().unwrap_or_else(PoisonError::into_inner);
    loop {
        {
            let mut answer = slot.lock().unwrap_or_else(PoisonError::into_inner);
            if matches!(*answer, Asked::Answered(_)) {
                let Asked::Answered(value) = std::mem::replace(&mut *answer, Asked::Waiting) else {
                    unreachable!()
                };
                return Some(value);
            }
            if !waits || Instant::now() >= deadline {
                *answer = Asked::Abandoned;
                thread.late.fetch_add(1, Ordering::AcqRel);
                return None;
            }
        }
        if let Some(task) = tasks.pop_front() {
            drop(tasks);
            autoreleasepool(|_| task());
            tasks = MAIN.tasks.lock().unwrap_or_else(PoisonError::into_inner);
            continue;
        }
        tasks = MAIN
            .ready
            .wait_timeout(tasks, deadline.saturating_duration_since(Instant::now()))
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
}

struct Due(Instant, u64, Box<dyn FnOnce()>);

impl PartialEq for Due {
    fn eq(&self, other: &Self) -> bool {
        (self.0, self.1) == (other.0, other.1)
    }
}
impl Eq for Due {}
impl PartialOrd for Due {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Due {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (other.0, other.1).cmp(&(self.0, self.1))
    }
}

thread_local! {
    static TIMERS: RefCell<BinaryHeap<Due>> = const { RefCell::new(BinaryHeap::new()) };
    static NEXT_TIMER: Cell<u64> = const { Cell::new(0) };
    static THREAD: Cell<Option<AppThread>> = const { Cell::new(None) };
    static ENDED: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn app_thread() -> AppThread {
    THREAD.get().expect("Not a render thread")
}

pub(crate) fn end_thread() {
    ENDED.set(true);
}

pub(crate) fn spawn_render(
    f: impl FnOnce() + Send + 'static,
    done: impl FnOnce() + Send + 'static,
) {
    let thread: AppThread = Box::leak(Box::new(Inbox::new()));
    post(thread, Box::new(f));
    RENDERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(thread);
    // WARNING: page boxes recurse as deep as the stack allows, and the main thread has this size.
    std::thread::Builder::new()
        .name("apps-render".into())
        .stack_size(64 << 20)
        .spawn(move || {
            // SAFETY: sets the class of this thread, which draws for the user like the main thread.
            unsafe {
                libc::pthread_set_qos_class_self_np(
                    libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE,
                    0,
                );
            }
            THREAD.set(Some(thread));
            while !ENDED.get() {
                let task = {
                    let deadline = TIMERS.with_borrow(|timers| timers.peek().map(|due| due.0));
                    let mut tasks = thread.tasks.lock().unwrap_or_else(PoisonError::into_inner);
                    loop {
                        if let Some(task) = tasks.pop_front() {
                            break Some(task);
                        }
                        match deadline {
                            None => {
                                tasks = thread
                                    .ready
                                    .wait(tasks)
                                    .unwrap_or_else(PoisonError::into_inner);
                            }
                            Some(deadline) => {
                                let now = Instant::now();
                                if now >= deadline {
                                    break None;
                                }
                                tasks = thread
                                    .ready
                                    .wait_timeout(tasks, deadline - now)
                                    .unwrap_or_else(PoisonError::into_inner)
                                    .0;
                            }
                        }
                    }
                };
                autoreleasepool(|_| {
                    if let Some(task) = task {
                        task();
                    }
                    let now = Instant::now();
                    while let Some(due) = TIMERS.with_borrow_mut(|timers| {
                        timers
                            .peek()
                            .is_some_and(|due| due.0 <= now)
                            .then(|| timers.pop())?
                    }) {
                        (due.2)();
                    }
                    // WARNING: this thread has no run loop to commit Core Animation changes.
                    CATransaction::flush();
                });
            }
            let left = {
                let mut tasks = thread.tasks.lock().unwrap_or_else(PoisonError::into_inner);
                thread.closed.store(true, Ordering::Relaxed);
                std::mem::take(&mut *tasks)
            };
            thread.late.store(0, Ordering::Release);
            drop(left);
            // WARNING: timers can hold the page side, whose drop needs the other thread locals.
            autoreleasepool(|_| {
                TIMERS.take();
                done();
                CATransaction::flush();
            });
            // WARNING: the process must not exit under the teardown above, so a thread that ends
            // counts as gone only now.
            leave_renders(thread);
        })
        .expect("Cannot start a render thread");
}

pub(crate) struct Platform;

impl Platform {
    pub fn new() -> Self {
        Self
    }

    pub fn run(&self, on_launch: Box<dyn FnOnce()>) {
        LAUNCH.with_borrow_mut(|launch| *launch = Some(on_launch));
        main_source();
        let marker = MainThreadMarker::new().expect("the app must start on the main thread");
        let app = NSApplication::sharedApplication(marker);
        // SAFETY: the delegate is a fresh instance of a registered NSResponder subclass.
        unsafe {
            let delegate: *mut AnyObject = msg_send![delegate_class(), new];
            let _: () = msg_send![&app, setDelegate: delegate];
        }
        autoreleasepool(|_| app.run());
    }

    pub fn quit(&self) {
        // WARNING: terminating closes windows, whose callbacks need the app, so it runs after
        // the current event.
        native_post(Box::new(|| {
            post_local(|| {
                let app = NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
                app.terminate(None);
            })
        }));
    }

    pub fn activate(&self, ignoring_other_apps: bool) {
        native_post(Box::new(move || {
            #[allow(deprecated)]
            NSApplication::sharedApplication(MainThreadMarker::new().unwrap())
                .activateIgnoringOtherApps(ignoring_other_apps);
        }));
    }

    pub fn read_clipboard(&self) -> Option<String> {
        native_sync(|| {
            // SAFETY: the type is an AppKit constant.
            let text =
                unsafe { NSPasteboard::generalPasteboard().stringForType(NSPasteboardTypeString) }?;
            Some(text.to_string())
        })
    }

    pub fn write_clipboard(&self, text: &str) {
        native_sync(|| {
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            // SAFETY: the type is an AppKit constant.
            unsafe {
                pasteboard.setString_forType(&NSString::from_str(text), NSPasteboardTypeString)
            };
        });
    }

    pub fn set_cursor(&self, style: CursorStyle) {
        native_post(Box::new(move || window::set_cursor(style)));
    }

    pub fn primary_display_bounds(&self) -> Option<Bounds<Pixels>> {
        native_sync(|| {
            let screens = NSScreen::screens(MainThreadMarker::new().unwrap());
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
        })
    }

    pub fn open_window(&self, options: &WindowOptions, callbacks: Callbacks) -> Result<Window> {
        Window::open(options, callbacks)
    }
}

extern "C" fn local(context: *mut c_void) {
    // SAFETY: the context is the box made by `run_after` or `post_local`.
    let f = unsafe { Box::from_raw(context as *mut Box<dyn FnOnce()>) };
    f();
}

pub(crate) fn post(thread: AppThread, f: Task) {
    thread.push(f);
}

pub(crate) fn run_after(delay: Duration, f: impl FnOnce() + 'static) {
    let order = NEXT_TIMER.replace(NEXT_TIMER.get().wrapping_add(1));
    TIMERS.with_borrow_mut(|timers| timers.push(Due(Instant::now() + delay, order, Box::new(f))));
}

pub(crate) fn native_after(delay: Duration, f: impl FnOnce() + 'static) {
    // WARNING: a run loop timer in the common modes fires in nested loops too, like `native_post`.
    // `dispatch_after` would also fire a tenth of its delay late (300 ms for a 3 s `setTimeout`).
    let f: Box<dyn FnOnce()> = Box::new(f);
    let mut context = CFRunLoopTimerContext {
        version: 0,
        info: Box::into_raw(Box::new(f)) as *mut c_void,
        retain: None,
        release: None,
        copyDescription: None,
    };
    // SAFETY: a one-shot timer on this thread's run loop, which keeps it until it fires.
    unsafe {
        let timer = CFRunLoopTimerCreate(
            kCFAllocatorDefault,
            CFAbsoluteTimeGetCurrent() + delay.as_secs_f64(),
            0.,
            0,
            0,
            fire,
            &mut context,
        );
        CFRunLoopTimerSetTolerance(timer, 0.001);
        CFRunLoopAddTimer(CFRunLoopGetMain(), timer, kCFRunLoopCommonModes);
        CFRelease(timer as _);
    }
}

extern "C" fn fire(_: CFRunLoopTimerRef, context: *mut c_void) {
    // SAFETY: the context is the box made by `native_after`, and a one-shot timer fires once.
    let f = unsafe { Box::from_raw(context as *mut Box<dyn FnOnce()>) };
    f();
}

pub(crate) fn post_local(f: impl FnOnce() + 'static) {
    let f: Box<dyn FnOnce()> = Box::new(f);
    let context = Box::into_raw(Box::new(f)) as *mut c_void;
    // SAFETY: `local` takes back the box on the main queue, which runs on this thread.
    unsafe { DispatchQueue::main().exec_async_f(context, local) };
}

extern "C-unwind" fn will_finish_launching(_: &AnyObject, _: Sel, _: *mut AnyObject) {
    // SAFETY: plain NSUserDefaults calls.
    unsafe {
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

extern "C-unwind" fn should_terminate(_: &AnyObject, _: Sel, _: *mut AnyObject) -> usize {
    crate::app::events::should_quit() as usize
}

extern "C-unwind" fn will_terminate(_: &AnyObject, _: Sel, _: *mut AnyObject) {
    park_render();
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
