// CVDisplayLink, not CADisplayLink: the build targets macOS 10.15, and the NSScreen and NSView display links
// need macOS 14 and fire on the main run loop, which a busy main script would block.

// CVDisplayLink is deprecated since macOS 15, for the display links above.
#![allow(deprecated)]

use super::{lock, threads::post};
use crate::app::Callbacks;
use block2::RcBlock;
use objc2_core_foundation::CFRetained;
use objc2_core_video::{CVDisplayLink, CVTimeStamp, kCVReturnSuccess};
use std::{
    cell::Cell,
    ptr::NonNull,
    sync::{Arc, Mutex, atomic},
    time::{Duration, Instant},
};

// One per display that ever had a window, never freed: CVDisplayLinkStop can return while the output
// handler still runs, and that handler reads the screen.
struct Screen {
    id: u32,
    // Start and stop under this lock only. The handler never takes it, because a stop can wait for the handler.
    link: Mutex<Option<Link>>,
    windows: Mutex<Vec<Arc<Pulse>>>,
}

struct Link {
    display: CFRetained<CVDisplayLink>,
    running: bool,
}

// SAFETY: CoreVideo display links can be driven from any thread.
unsafe impl Send for Link {}

static SCREENS: Mutex<Vec<&'static Screen>> = Mutex::new(Vec::new());

impl Screen {
    fn of(id: u32) -> &'static Self {
        let mut screens = lock(&SCREENS);
        match screens.iter().find(|screen| screen.id == id) {
            Some(screen) => screen,
            None => {
                let screen: &'static Screen = Box::leak(Box::new(Screen {
                    id,
                    link: Mutex::default(),
                    windows: Mutex::default(),
                }));
                screens.push(screen);
                screen
            }
        }
    }

    fn make_link(&'static self) -> Result<Link, String> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: `raw` is a valid out pointer, and a created link comes with one reference for us.
        let display = unsafe {
            let made = CVDisplayLink::create_with_cg_display(self.id, NonNull::from(&mut raw));
            NonNull::new(raw)
                .filter(|_| made == kCVReturnSuccess)
                .map(|raw| CFRetained::from_raw(raw))
        };
        let display = display.ok_or_else(|| format!("no display link for display {}", self.id))?;
        let last = Cell::new(None::<Instant>);
        let handler = RcBlock::new(move |_, _, out: NonNull<CVTimeStamp>, _, _| {
            // Ticks come up to 1 ms early or late. Frame times follow them in even steps of one refresh.
            // SAFETY: CoreVideo passes a live timestamp.
            let out = unsafe { out.as_ref() };
            let period = out.videoRefreshPeriod as f64 / f64::from(out.videoTimeScale);
            let period = Duration::try_from_secs_f64(period).unwrap_or_default();
            let now = Instant::now();
            let at = match last.get().map(|last| last + period) {
                Some(even) if now.max(even) - now.min(even) < period / 2 => {
                    even + now.saturating_duration_since(even) / 8
                        - even.saturating_duration_since(now) / 8
                }
                _ => now,
            };
            last.set(Some(at));
            lock(&self.windows).iter().for_each(|pulse| pulse.beat(at));
            kCVReturnSuccess
        });
        // SAFETY: CoreVideo copies the block, which only reads a screen that lives for good.
        match unsafe { display.set_output_handler(RcBlock::as_ptr(&handler)) } {
            code if code == kCVReturnSuccess => Ok(Link {
                display,
                running: false,
            }),
            code => Err(format!(
                "the display link of display {} takes no handler: error {code}",
                self.id
            )),
        }
    }

    fn join(&'static self, pulse: &Arc<Pulse>) -> Result<(), String> {
        let mut link = lock(&self.link);
        if link.is_none() {
            *link = Some(self.make_link()?);
        }
        let link = link.as_mut().unwrap();
        lock(&self.windows).push(pulse.clone());
        if !link.running {
            let code = link.display.start();
            if code != kCVReturnSuccess {
                lock(&self.windows).retain(|other| !Arc::ptr_eq(other, pulse));
                return Err(format!(
                    "the display link of display {} did not start: error {code}",
                    self.id
                ));
            }
            link.running = true;
        }
        Ok(())
    }

    fn part(&self, pulse: &Arc<Pulse>) {
        let mut link = lock(&self.link);
        let mut windows = lock(&self.windows);
        windows.retain(|other| !Arc::ptr_eq(other, pulse));
        let idle = windows.is_empty();
        drop(windows);
        if let Some(link) = link.as_mut().filter(|link| idle && link.running) {
            // A last handler call can still come. It finds no windows.
            link.display.stop();
            link.running = false;
        }
    }
}

// At most one frame call waits at a time.
struct Pulse {
    callbacks: Callbacks,
    waiting: atomic::AtomicBool,
}

impl Pulse {
    fn beat(self: &Arc<Self>, at: Instant) {
        if self.waiting.swap(true, atomic::Ordering::AcqRel) {
            return;
        }
        let pulse = self.clone();
        post(
            self.callbacks.thread,
            Box::new(move || {
                pulse.waiting.store(false, atomic::Ordering::Release);
                let callbacks = pulse.callbacks;
                // A tick without a frame takes one frame out of a queue that is too long.
                let free = callbacks.with(move |window, _| {
                    let free = !window.platform.renderer.take_congested();
                    window.frame_at = free.then_some(at);
                    free
                });
                if free == Some(true) {
                    step(callbacks);
                }
            }),
        );
    }
}

pub(crate) fn step(callbacks: Callbacks) {
    if callbacks.frame() == Some(false) {
        callbacks.with(|window, _| window.platform.rest());
    }
}

pub(crate) struct Pacer {
    pulse: Arc<Pulse>,
    on: Option<&'static Screen>,
    pub wants: bool,
    pub visible: bool,
    pub display: Option<u32>,
    pub closed: bool,
}

impl Pacer {
    pub fn new(callbacks: Callbacks) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            pulse: Arc::new(Pulse {
                callbacks,
                waiting: atomic::AtomicBool::new(false),
            }),
            on: None,
            wants: false,
            visible: false,
            display: None,
            closed: false,
        }))
    }

    pub fn sync(&mut self) {
        let target = self
            .display
            .filter(|_| self.wants && self.visible && !self.closed);
        if self.on.map(|screen| screen.id) == target {
            return;
        }
        if let Some(screen) = self.on.take() {
            screen.part(&self.pulse);
        }
        let Some(id) = target else {
            return;
        };
        let screen = Screen::of(id);
        match screen.join(&self.pulse) {
            Ok(()) => self.on = Some(screen),
            Err(error) => eprintln!("apps: display link: {error}"),
        }
    }
}

impl Drop for Pacer {
    fn drop(&mut self) {
        if let Some(screen) = self.on.take() {
            screen.part(&self.pulse);
        }
    }
}
