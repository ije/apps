// One thread runs everything here: the event loop, main code and every window. Display servers, other threads and
// timers wake it through `poll`.

mod gpu;
mod input;
mod text;
mod wayland;
mod x11;

use super::single::Task;
use crate::{Cursor, Pixels, Point, Rect, Scene, Size, WindowChange, WindowOptions};
use crate::{WindowState, app::Callbacks};
use anyhow::Result;
use gpu::{Atlas, Capture};
use raw_window_handle::RawWindowHandle;
use rustc_hash::FxHashMap;
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd, RawFd};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::{cell::Cell, cell::RefCell, collections::BTreeMap, collections::VecDeque};
use std::{rc::Rc, rc::Weak, time::Duration, time::Instant};

pub(crate) use text::TextSystem;

// A window that stops drawing releases its spare GPU resources after this.
const QUIET: Duration = Duration::from_secs(1);

type Job = Box<dyn FnOnce()>;
type Watcher = Box<dyn FnMut() -> bool>;

static INBOX: Mutex<Vec<Task>> = Mutex::new(vec![]);
static BELL: OnceLock<OwnedFd> = OnceLock::new();

fn inbox() -> MutexGuard<'static, Vec<Task>> {
    INBOX.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
struct Turn {
    main: Cell<bool>,
    done: Cell<bool>,
    soon: RefCell<VecDeque<Job>>,
    due: RefCell<BTreeMap<(Instant, u64), Job>>,
    count: Cell<u64>,
    // The call is `None` while it runs.
    watches: RefCell<Vec<(RawFd, i16, Option<Watcher>)>>,
    windows: RefCell<FxHashMap<u64, Weak<dyn Native>>>,
}

thread_local! {
    static TURN: Turn = Turn::default();
}

pub(crate) fn on_main() -> bool {
    TURN.with(|turn| turn.main.get())
}

/// Runs `f` in the next turn of the loop, outside the event being handled.
pub(super) fn soon(f: impl FnOnce() + 'static) {
    TURN.with(|turn| turn.soon.borrow_mut().push_back(Box::new(f)));
}

pub(super) fn watch(fd: RawFd, write: bool, f: impl FnMut() -> bool + 'static) {
    let events = if write { libc::POLLOUT } else { libc::POLLIN };
    TURN.with(|turn| {
        turn.watches
            .borrow_mut()
            .push((fd, events, Some(Box::new(f))))
    });
}

pub(crate) fn run_after(delay: Duration, f: impl FnOnce() + 'static) {
    TURN.with(|turn| {
        let number = turn.count.replace(turn.count.get() + 1);
        let at = Instant::now() + delay;
        turn.due.borrow_mut().insert((at, number), Box::new(f));
    });
}

pub(crate) fn native_post(task: Task) {
    inbox().push(task);
    if let Some(bell) = BELL.get() {
        let one = 1u64;
        // SAFETY: an eventfd takes 8 bytes. A full counter still wakes the loop, so the result doesn't matter.
        unsafe { libc::write(bell.as_raw_fd(), (&raw const one).cast(), 8) };
    }
}

pub(crate) use run_after as native_after;

trait Display {
    fn fd(&self) -> RawFd;
    /// False when the connection broke.
    fn prepare(&self) -> bool;
    fn finish(&self, readable: bool) -> bool;
    fn read_clipboard(&self) -> Option<String>;
    fn write_clipboard(&self, text: &str);
    fn set_cursor(&self, cursor: Cursor);
    fn display_bounds(&self) -> Option<Rect>;
    fn open(self: Rc<Self>, options: &WindowOptions, calls: Callbacks) -> Result<Opened>;
}

type Opened = (Rc<dyn Native>, Rc<Atlas>);

enum Action {
    Title(String),
    Minimize,
    Zoom,
    Fullscreen,
    Raise,
    Resize(Size<Pixels>),
    Move,
    Parent(Option<RawWindowHandle>),
    CancelComposition,
}

/// `scale` is device pixels per point, `extent` the content in points.
trait Native {
    fn scale(&self) -> f32;
    fn extent(&self) -> Size<Pixels>;
    fn state(&self) -> WindowState;
    fn active(&self) -> bool;
    fn want_frames(&self);
    fn draw(&self, scene: &Scene, capture: bool) -> Capture;
    fn take_snapshots(&self) -> Vec<crate::Snapshot>;
    fn release(&self);
    fn act(&self, action: Action);
    fn handle(&self) -> RawWindowHandle;
}

pub(crate) struct Platform {
    display: Rc<dyn Display>,
}

impl Platform {
    pub fn new() -> Self {
        TURN.with(|turn| turn.main.set(true));
        // SAFETY: a plain eventfd, owned below.
        let bell = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        if bell >= 0 {
            // SAFETY: the descriptor is new and has no other owner.
            let _ = BELL.set(unsafe { OwnedFd::from_raw_fd(bell) });
        }
        let named = |name| std::env::var(name).is_ok_and(|value| !value.is_empty());
        let x11 = named("DISPLAY");
        let wayland = named("WAYLAND_DISPLAY") || std::env::var_os("WAYLAND_SOCKET").is_some();
        // Wayland first, and X11 when Wayland is missing or fails.
        let mut reasons = Vec::new();
        let mut attempt = |wanted: bool, connect: &dyn Fn() -> Result<Rc<dyn Display>>| {
            let connected = wanted.then(connect)?;
            connected
                .inspect_err(|error| reasons.push(format!("{error:#}")))
                .ok()
        };
        let display = attempt(wayland, &|| Ok(wayland::Client::connect(x11)?))
            .or_else(|| attempt(x11, &|| Ok(x11::Client::connect()?)));
        let Some(display) = display else {
            eprintln!("apps: no display: {}", reasons.join("; "));
            std::process::exit(1);
        };
        Self { display }
    }

    pub fn run(&self, on_launch: Box<dyn FnOnce()>) {
        soon(on_launch);
        let display = &self.display;
        let bell = BELL.get().map_or(-1, |bell| bell.as_raw_fd());
        let mut polled: Vec<libc::pollfd> = Vec::new();
        TURN.with(|turn| {
            loop {
                // Bound first: a temporary in a `for` header lives through the loop, and tasks post and queue more.
                let posted = std::mem::take(&mut *inbox());
                for task in posted {
                    task();
                }
                // Only what was queued before this turn runs, so a call that queues itself can't starve input.
                let queued = turn.soon.borrow().len();
                for _ in 0..queued {
                    let Some(task) = turn.soon.borrow_mut().pop_front() else {
                        break;
                    };
                    task();
                }
                let now = Instant::now();
                loop {
                    let task = match turn.due.borrow_mut().first_entry() {
                        Some(first) if first.key().0 <= now => first.remove(),
                        _ => break,
                    };
                    task();
                }
                if turn.done.get() || !display.prepare() {
                    break;
                }
                let busy = !turn.soon.borrow().is_empty() || !inbox().is_empty();
                let next = turn
                    .due
                    .borrow()
                    .first_key_value()
                    .map(|(first, _)| first.0);
                let timeout = match next {
                    _ if busy => 0,
                    Some(when) => {
                        let wait = when.saturating_duration_since(Instant::now());
                        wait.as_micros().div_ceil(1000).min(i32::MAX as u128) as i32
                    }
                    None => -1,
                };
                polled.clear();
                let listen = |fd, events| libc::pollfd {
                    fd,
                    events,
                    revents: 0,
                };
                polled.push(listen(display.fd(), libc::POLLIN));
                polled.push(listen(bell, libc::POLLIN));
                polled.extend(
                    turn.watches
                        .borrow()
                        .iter()
                        .map(|(fd, events, _)| listen(*fd, *events)),
                );
                // SAFETY: the descriptors stay open while their watches exist, and the length is the list's.
                let count = unsafe {
                    libc::poll(polled.as_mut_ptr(), polled.len() as libc::nfds_t, timeout)
                };
                let failure = (count < 0).then(std::io::Error::last_os_error);
                if let Some(failure) =
                    failure.filter(|failure| failure.kind() != std::io::ErrorKind::Interrupted)
                {
                    eprintln!("apps: poll: {failure}");
                    break;
                }
                let ready = |at: usize| count > 0 && polled[at].revents != 0;
                if ready(1) {
                    let mut rung = 0u64;
                    // SAFETY: an eventfd gives 8 bytes.
                    unsafe { libc::read(bell, (&raw mut rung).cast(), 8) };
                }
                if !display.finish(ready(0)) {
                    break;
                }
                // Watches added during these calls have no entry in `polled`, and wait for the next turn.
                for at in (2..polled.len()).filter(|&at| ready(at)) {
                    let fd = polled[at].fd;
                    let mut watches = turn.watches.borrow_mut();
                    let taken = watches
                        .iter_mut()
                        .find(|watch| watch.0 == fd)
                        .and_then(|watch| watch.2.take());
                    drop(watches);
                    let Some(mut call) = taken else {
                        continue;
                    };
                    let keep = call();
                    let mut watches = turn.watches.borrow_mut();
                    if let Some(index) = watches
                        .iter()
                        .position(|watch| watch.0 == fd && watch.2.is_none())
                    {
                        match keep {
                            true => watches[index].2 = Some(call),
                            false => drop(watches.swap_remove(index)),
                        }
                    }
                }
            }
        });
        crate::native::quit_handler();
    }

    pub fn quit(&self) {
        // Main decides, and it can't run while the app calls this.
        soon(|| {
            if crate::native::should_quit() {
                TURN.with(|turn| turn.done.set(true));
            }
        });
    }

    pub fn activate(&self, _: bool) {}

    pub fn read_clipboard(&self) -> Option<String> {
        self.display.read_clipboard()
    }

    pub fn write_clipboard(&self, text: &str) {
        self.display.write_clipboard(text);
    }

    pub fn set_cursor(&self, cursor: Cursor) {
        self.display.set_cursor(cursor);
    }

    pub fn primary_display_bounds(&self) -> Option<Rect> {
        self.display.display_bounds()
    }

    pub fn open_window(&self, options: &WindowOptions, calls: Callbacks) -> Result<Window> {
        let (native, atlas) = self.display.clone().open(options, calls)?;
        let key = TURN.with(|turn| {
            let key = turn.count.replace(turn.count.get() + 1);
            turn.windows
                .borrow_mut()
                .insert(key, Rc::downgrade(&native));
            key
        });
        Ok(Window {
            native,
            atlas,
            control: WindowControl(key),
            calls,
            quiet: Cell::new(None),
        })
    }
}

pub(crate) struct Window {
    native: Rc<dyn Native>,
    atlas: Rc<Atlas>,
    control: WindowControl,
    calls: Callbacks,
    // `None` without a timer, else whether a frame came since the timer started.
    quiet: Cell<Option<bool>>,
}

// The control's calls work on the window too, as on macOS.
impl std::ops::Deref for Window {
    type Target = WindowControl;
    fn deref(&self) -> &WindowControl {
        &self.control
    }
}

impl Window {
    pub fn control(&self) -> &WindowControl {
        &self.control
    }

    pub fn atlas(&self) -> &Rc<Atlas> {
        &self.atlas
    }

    pub fn scale_factor(&self) -> f32 {
        self.native.scale()
    }

    pub fn content_size(&self) -> Size<Pixels> {
        self.native.extent()
    }

    pub fn state(&self) -> WindowState {
        self.native.state()
    }

    pub fn request_frame(&self) {
        self.native.want_frames();
    }

    // Draws once even while the window is hidden, like for a capture.
    pub fn force_frame(&self) {
        let calls = self.calls;
        soon(move || {
            calls.frame();
        });
    }

    pub fn take_snapshots(&self) -> Vec<crate::Snapshot> {
        self.native.take_snapshots()
    }

    pub fn draw(&self, scene: &Scene, capture: bool) -> Capture {
        if self.quiet.replace(Some(true)).is_none() {
            self.wait();
        }
        self.native.draw(scene, capture)
    }

    fn wait(&self) {
        let calls = self.calls;
        crate::app::timer(QUIET, move |_| calls.release()).detach();
    }

    pub fn release(&self) {
        // Frames came since the timer started, so the quiet second starts again.
        if self.quiet.replace(Some(false)) == Some(true) {
            return self.wait();
        }
        self.quiet.set(None);
        self.native.release();
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        TURN.with(|turn| turn.windows.borrow_mut().remove(&self.control.0));
    }
}

/// Calls on other threads do nothing, as every window lives on main.
#[derive(Clone)]
pub struct WindowControl(u64);

macro_rules! pass_on {
    ($($name:ident => $action:ident,)*) => {$(
        pub fn $name(&self) {
            self.act(Action::$action);
        }
    )*};
}

impl WindowControl {
    fn native(&self) -> Option<Rc<dyn Native>> {
        TURN.with(|turn| turn.windows.borrow().get(&self.0)?.upgrade())
    }

    fn act(&self, action: Action) {
        if let Some(native) = self.native() {
            native.act(action);
        }
    }

    pub fn change(&self, change: WindowChange) {
        match change {
            WindowChange::Show(_) | WindowChange::Restore => self.act(Action::Raise),
            WindowChange::Parent(parent, _) => self.act(Action::Parent(parent)),
            WindowChange::CancelComposition => self.act(Action::CancelComposition),
            _ => {}
        }
    }

    pub fn set_title(&self, title: &str) {
        self.act(Action::Title(title.to_owned()));
    }

    pub fn set_button_visibility(&self, _: bool) {}

    pub fn set_button_position(&self, _: Option<Point<Pixels>>) {}

    pass_on! {
        minimize => Minimize,
        zoom => Zoom,
        titlebar_double_click => Zoom,
        toggle_fullscreen => Fullscreen,
        activate => Raise,
        start_window_move => Move,
    }

    pub fn resize(&self, extent: Size<Pixels>) {
        self.act(Action::Resize(extent));
    }

    pub fn is_active(&self) -> bool {
        self.native().is_some_and(|native| native.active())
    }

    pub fn state(&self) -> Option<WindowState> {
        Some(self.native()?.state())
    }

    pub fn raw_window_handle(&self) -> Result<RawWindowHandle> {
        let native = self
            .native()
            .ok_or_else(|| anyhow::anyhow!("The window is closed"))?;
        Ok(native.handle())
    }
}

fn app_name() -> String {
    let executable = std::env::current_exe().ok();
    let stem = executable.and_then(|path| Some(path.file_stem()?.to_string_lossy().into_owned()));
    stem.unwrap_or_else(|| "apps".to_owned())
}
