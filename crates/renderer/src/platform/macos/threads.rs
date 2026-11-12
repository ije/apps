use super::{Sendable, lock};
use core_foundation_sys::{
    base::{CFRelease, kCFAllocatorDefault},
    date::CFAbsoluteTimeGetCurrent,
    runloop::{
        CFRunLoopAddSource, CFRunLoopAddTimer, CFRunLoopGetMain, CFRunLoopSourceContext,
        CFRunLoopSourceCreate, CFRunLoopSourceSignal, CFRunLoopTimerContext, CFRunLoopTimerCreate,
        CFRunLoopTimerRef, CFRunLoopTimerSetTolerance, CFRunLoopWakeUp, kCFRunLoopCommonModes,
    },
};
use dispatch2::DispatchQueue;
use objc2::{MainThreadMarker, rc::autoreleasepool};
use objc2_quartz_core::CATransaction;
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, VecDeque},
    ffi::c_void,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

pub(crate) type Task = Box<dyn FnOnce() + Send>;

struct Inbox {
    tasks: VecDeque<Task>,
    // Set when the thread ended. Later tasks are dropped.
    closed: bool,
}

pub(crate) struct Queue {
    inbox: Mutex<Inbox>,
    ready: Condvar,
    pub(super) late: AtomicUsize,
}

impl Queue {
    const fn new() -> Self {
        let inbox = Inbox {
            tasks: VecDeque::new(),
            closed: false,
        };
        Self {
            inbox: Mutex::new(inbox),
            ready: Condvar::new(),
            late: AtomicUsize::new(0),
        }
    }
}

// Queues are never freed: native view callbacks can still reach one after its thread ended.
pub(crate) type AppThread = &'static Queue;

// While main waits in `ask`, it runs these tasks, so a page that waits for main cannot deadlock it.
static MAIN: Queue = Queue::new();
static MAIN_SIGNALED: AtomicBool = AtomicBool::new(false);
static LIVE: Mutex<Vec<AppThread>> = Mutex::new(Vec::new());
static LEFT: Condvar = Condvar::new();

const ASK_LIMIT: Duration = Duration::from_millis(100);

type Timers = BTreeMap<(Instant, u64), Box<dyn FnOnce()>>;

struct Local {
    queue: Cell<Option<AppThread>>,
    ending: Cell<bool>,
    timers: RefCell<Timers>,
    arrivals: Cell<u64>,
}

thread_local! {
    static LOCAL: Local = const {
        Local {
            queue: Cell::new(None),
            ending: Cell::new(false),
            timers: RefCell::new(BTreeMap::new()),
            arrivals: Cell::new(0),
        }
    };
}

pub(crate) fn post(thread: AppThread, task: Task) {
    let mut inbox = lock(&thread.inbox);
    if !inbox.closed {
        inbox.tasks.push_back(task);
    }
    drop(inbox);
    thread.ready.notify_all();
}

// A source in the common modes: AppKit does not drain the main dispatch queue in menus and modal panels.
pub(crate) fn main_source() -> usize {
    static SOURCE: OnceLock<usize> = OnceLock::new();
    extern "C" fn drain(_: *const c_void) {
        MAIN_SIGNALED.store(false, Ordering::Release);
        loop {
            let Some(task) = lock(&MAIN.inbox).tasks.pop_front() else {
                return;
            };
            autoreleasepool(|_| task());
        }
    }
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
            perform: drain,
        };
        // SAFETY: the source lives as long as the process, and run loop calls are thread safe.
        unsafe {
            let source = CFRunLoopSourceCreate(kCFAllocatorDefault, 0, &mut context);
            CFRunLoopAddSource(CFRunLoopGetMain(), source, kCFRunLoopCommonModes);
            source as usize
        }
    })
}

pub(crate) fn native_post(task: Task) {
    post(&MAIN, task);
    if !MAIN_SIGNALED.swap(true, Ordering::AcqRel) {
        // SAFETY: the source lives as long as the process, and signaling is thread safe.
        unsafe {
            CFRunLoopSourceSignal(main_source() as _);
            CFRunLoopWakeUp(CFRunLoopGetMain());
        }
    }
}

pub(crate) fn native_sync<R: Send>(job: impl FnOnce() -> R + Send) -> R {
    if MainThreadMarker::new().is_some() {
        return job();
    }
    // Wakes the caller even when main drops the task without running it.
    struct Done<'a>(&'a (Mutex<bool>, Condvar));
    impl Drop for Done<'_> {
        fn drop(&mut self) {
            // Notify under the lock: the caller frees the pair as soon as it sees the flag.
            let mut finished = lock(&self.0.0);
            *finished = true;
            self.0.1.notify_all();
        }
    }
    let signal = (Mutex::new(false), Condvar::new());
    let mut out = None;
    let task: Box<dyn FnOnce() + Send + '_> = {
        let (done, out) = (Done(&signal), &mut out);
        Box::new(move || {
            *out = Some(job());
            drop(done);
        })
    };
    // SAFETY: this thread waits below until the task ran or was dropped, so its borrows outlive it.
    native_post(unsafe { std::mem::transmute::<Box<dyn FnOnce() + Send + '_>, Task>(task) });
    let mut finished = lock(&signal.0);
    while !*finished {
        finished = signal.1.wait(finished).unwrap_or_else(|e| e.into_inner());
    }
    drop(finished);
    out.expect("The main thread dropped its work")
}

pub(crate) fn native_after(delay: Duration, job: impl FnOnce() + 'static) {
    extern "C" fn fire(_: CFRunLoopTimerRef, info: *mut c_void) {
        // SAFETY: `info` is the box below, and a one-shot timer fires once.
        let job = unsafe { Box::from_raw(info.cast::<Box<dyn FnOnce()>>()) };
        job();
    }
    let job: Box<Box<dyn FnOnce()>> = Box::new(Box::new(job));
    let mut context = CFRunLoopTimerContext {
        version: 0,
        info: Box::into_raw(job).cast(),
        retain: None,
        release: None,
        copyDescription: None,
    };
    let seconds = delay.as_secs_f64();
    // SAFETY: a one-shot timer that the main run loop keeps until it fires.
    unsafe {
        let timer = CFRunLoopTimerCreate(
            kCFAllocatorDefault,
            CFAbsoluteTimeGetCurrent() + seconds,
            0.,
            0,
            0,
            fire,
            &mut context,
        );
        // Common modes run it in nested loops too, and a small tolerance keeps coalescing from delaying it.
        CFRunLoopTimerSetTolerance(timer, (seconds / 10.).min(0.001));
        CFRunLoopAddTimer(CFRunLoopGetMain(), timer, kCFRunLoopCommonModes);
        CFRelease(timer.cast());
    }
}

// Runs on the main dispatch queue, which AppKit serves only outside nested loops.
pub(crate) fn post_local(job: impl FnOnce() + 'static) {
    let job = Sendable::new(job);
    DispatchQueue::main().exec_async(move || job.take()());
}

enum Answer<R> {
    Pending,
    Ready(R),
    Dropped,
}

pub(crate) fn ask<R: Send + 'static>(
    thread: AppThread,
    job: impl FnOnce() -> R + Send + 'static,
) -> Option<R> {
    ask_within(thread, ASK_LIMIT, job)
}

// Waits only while the thread owes no earlier answer, so a hung page cannot block main again and again.
pub(crate) fn ask_within<R: Send + 'static>(
    thread: AppThread,
    limit: Duration,
    job: impl FnOnce() -> R + Send + 'static,
) -> Option<R> {
    if lock(&thread.inbox).closed {
        return None;
    }
    let answer = Arc::new(Mutex::new(Answer::Pending));
    let slot = answer.clone();
    post(
        thread,
        Box::new(move || {
            let value = job();
            {
                let mut slot = lock(&slot);
                match *slot {
                    Answer::Dropped => {
                        thread.late.fetch_sub(1, Ordering::AcqRel);
                    }
                    _ => *slot = Answer::Ready(value),
                }
            }
            // Take the lock the waiter holds between its check and its wait, so the wake cannot fall between.
            drop(lock(&MAIN.inbox));
            MAIN.ready.notify_all();
        }),
    );
    let patient = thread.late.load(Ordering::Acquire) == 0;
    let deadline = Instant::now() + limit;
    let mut inbox = lock(&MAIN.inbox);
    loop {
        {
            let mut slot = lock(&answer);
            if let Answer::Ready(_) = *slot {
                let Answer::Ready(value) = std::mem::replace(&mut *slot, Answer::Pending) else {
                    unreachable!()
                };
                return Some(value);
            }
            if !patient || Instant::now() >= deadline {
                *slot = Answer::Dropped;
                thread.late.fetch_add(1, Ordering::AcqRel);
                return None;
            }
        }
        match inbox.tasks.pop_front() {
            Some(task) => {
                drop(inbox);
                autoreleasepool(|_| task());
                inbox = lock(&MAIN.inbox);
            }
            None => {
                let left = deadline.saturating_duration_since(Instant::now());
                inbox = MAIN
                    .ready
                    .wait_timeout(inbox, left)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        }
    }
}

pub(crate) fn app_thread() -> AppThread {
    LOCAL
        .with(|local| local.queue.get())
        .expect("Not a render thread")
}

pub(crate) fn end_thread() {
    LOCAL.with(|local| local.ending.set(true));
}

pub(crate) fn run_after(delay: Duration, job: impl FnOnce() + 'static) {
    LOCAL.with(|local| {
        let order = local.arrivals.get();
        local.arrivals.set(order.wrapping_add(1));
        local
            .timers
            .borrow_mut()
            .insert((Instant::now() + delay, order), Box::new(job));
    });
}

fn leave(thread: AppThread) {
    lock(&LIVE).retain(|live| !std::ptr::eq(*live, thread));
    LEFT.notify_all();
}

pub(crate) fn spawn_render(
    init: impl FnOnce() + Send + 'static,
    teardown: impl FnOnce() + Send + 'static,
) {
    let queue: AppThread = Box::leak(Box::new(Queue::new()));
    post(queue, Box::new(init));
    lock(&LIVE).push(queue);
    // Page boxes recurse as deep as the stack allows, so this matches the main thread's stack.
    let thread = std::thread::Builder::new()
        .name("apps-render".into())
        .stack_size(64 << 20);
    let spawned = thread.spawn(move || {
        // SAFETY: changes only the class of this thread, which draws for the user like main.
        unsafe {
            libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
        }
        LOCAL.with(|local| local.queue.set(Some(queue)));
        while !LOCAL.with(|local| local.ending.get()) {
            let next = LOCAL.with(|local| {
                local
                    .timers
                    .borrow()
                    .first_key_value()
                    .map(|(due, _)| due.0)
            });
            let mut inbox = lock(&queue.inbox);
            let task = loop {
                if let Some(task) = inbox.tasks.pop_front() {
                    break Some(task);
                }
                let Some(next) = next else {
                    inbox = queue.ready.wait(inbox).unwrap_or_else(|e| e.into_inner());
                    continue;
                };
                let Some(left) = next
                    .checked_duration_since(Instant::now())
                    .filter(|left| !left.is_zero())
                else {
                    break None;
                };
                inbox = queue
                    .ready
                    .wait_timeout(inbox, left)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            };
            drop(inbox);
            autoreleasepool(|_| {
                if let Some(task) = task {
                    task();
                }
                let now = Instant::now();
                while let Some(job) = LOCAL.with(|local| {
                    let mut timers = local.timers.borrow_mut();
                    let entry = timers.first_entry().filter(|entry| entry.key().0 <= now)?;
                    Some(entry.remove())
                }) {
                    job();
                }
                // No run loop commits Core Animation changes on this thread.
                CATransaction::flush();
            });
        }
        let rest = {
            let mut inbox = lock(&queue.inbox);
            inbox.closed = true;
            std::mem::take(&mut inbox.tasks)
        };
        queue.late.store(0, Ordering::Release);
        drop(rest);
        // Timers can own page state that needs other thread locals, so they go first.
        autoreleasepool(|_| {
            LOCAL.with(|local| local.timers.take());
            teardown();
            CATransaction::flush();
        });
        // The thread counts as stopped only now, or the process could exit during teardown.
        leave(queue);
    });
    spawned.expect("Cannot start a render thread");
}

pub(crate) fn park_render() {
    let live = lock(&LIVE).clone();
    for queue in live {
        post(
            queue,
            Box::new(move || {
                leave(queue);
                loop {
                    std::thread::park();
                }
            }),
        );
    }
    let live = lock(&LIVE);
    let _ = LEFT.wait_timeout_while(live, 3 * ASK_LIMIT, |live| !live.is_empty());
}
