use std::{
    ffi::{CStr, c_void},
    fmt::Write as _,
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::process::{CommandExt, ExitStatusExt},
    },
    process::Command,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

pub const ABI: u32 = 3;

pub type Alloc = extern "C" fn(output: *mut c_void, length: usize) -> *mut u8;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Host {
    pub waker: extern "C" fn() -> *mut c_void,
    pub wake: extern "C" fn(waker: *mut c_void),
    pub drop_waker: extern "C" fn(waker: *mut c_void),
    /// Decodes a PNG image for the terminal. It writes the width and the height to `size` and RGBA pixels
    /// to the memory from `alloc`, which can be null.
    pub decode_png: extern "C" fn(
        data: *const u8,
        length: usize,
        size: *mut u32,
        output: *mut c_void,
        alloc: Alloc,
    ) -> bool,
}

/// One cell of a row. Colors are `0x01RRGGBB`, or 0 for the default color.
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Cell {
    pub ch: u32,
    pub fg: u32,
    pub bg: u32,
    pub underline: u32,
    /// Bits 0 to 7: bold, italic, faint, blink, inverse, invisible, strikethrough, overline. Bits 8 to 10:
    /// the underline style. Bit 11: wide. Bit 12: the spacer of a wide cell. Bit 13: selected.
    pub flags: u16,
    /// The count of codepoints after `ch`. They are in `Frame::extras`, in cell order.
    pub extra: u16,
}

/// A rectangle of an image on the screen.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Placement {
    /// The generation of the image. It is the key of the pixels for `apps_terminal_image`, and no other
    /// image ever has it.
    pub image: u64,
    pub id: u32,
    /// 0: below the backgrounds of the cells, 1: below the text, 2: above the text.
    pub layer: u32,
    pub z: i32,
    /// The top left cell, which can be outside the grid.
    pub col: i32,
    pub row: i32,
    /// In pixels: the offset in that cell, the size, and the part of the image to show.
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub source: [u32; 4],
}

/// The rows that changed since the last frame. The pointers are valid until the next frame.
#[repr(C)]
pub struct Frame {
    pub cols: u16,
    pub rows: u16,
    pub cursor_x: u16,
    pub cursor_y: u16,
    /// 0: no cursor. Else 1 bar, 2 block, 3 underline, 4 hollow block.
    pub cursor: u8,
    pub cursor_blinks: u8,
    pub cursor_wide: u8,
    /// All rows are in the frame.
    pub full: u8,
    pub foreground: u32,
    pub background: u32,
    pub cursor_color: u32,
    /// The count of rows. Row `i` is `ys[i]` and has the cells from `i * cols`.
    pub count: u32,
    pub ys: *const u16,
    pub cells: *const Cell,
    pub extras: *const u32,
    /// All images of the screen, in the order to draw them.
    pub placements: *const Placement,
    pub placed: u32,
    /// The time in ms until an animated image has its next picture, or 0.
    pub wait: u32,
    /// In rows: all rows with the scrollback, the first row of the screen, and the rows of the screen.
    pub scroll: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Str {
    ptr: *const u8,
    len: usize,
}

impl Str {
    fn text(&self) -> String {
        if self.len == 0 {
            return String::new();
        }
        String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(self.ptr, self.len) })
            .into_owned()
    }
}

#[repr(C)]
struct VtHost {
    user: *mut c_void,
    reply: extern "C" fn(*mut c_void, *const u8, usize),
    event: extern "C" fn(*mut c_void, u32, *const u8, usize),
    status: extern "C" fn(*mut c_void, u32, u32, i32, *const Str),
}

#[repr(C)]
struct Info {
    cols: u16,
    rows: u16,
    cursor_x: u16,
    cursor_y: u16,
    cursor_visible: u8,
    alternate: u8,
    mouse: u8,
    bottom: u8,
    total: u64,
    offset: u64,
    title: Str,
    pwd: Str,
}

enum Vt {}

unsafe extern "C" {
    fn vt_new(cols: u16, rows: u16, host: *const VtHost) -> *mut Vt;
    fn vt_free(vt: *mut Vt);
    fn vt_write(vt: *mut Vt, data: *const u8, len: usize);
    fn vt_resize(vt: *mut Vt, cols: u16, rows: u16, cell_w: u32, cell_h: u32);
    fn vt_theme(vt: *mut Vt, colors: *const u32, dark: bool);
    fn vt_key(
        vt: *mut Vt,
        action: u32,
        name: *const u8,
        name_len: usize,
        mods: u32,
        text: *const u8,
        text_len: usize,
        out: *mut u8,
        cap: usize,
    ) -> usize;
    fn vt_paste(vt: *mut Vt, data: *mut u8, len: usize);
    fn vt_focus(vt: *mut Vt, focused: bool);
    fn vt_scroll(vt: *mut Vt, kind: u32, rows: i32);
    fn vt_mouse(
        vt: *mut Vt,
        kind: u32,
        button: u32,
        mods: u32,
        x: f64,
        y: f64,
        held: bool,
        time: u64,
    ) -> bool;
    fn vt_wheel(vt: *mut Vt, rows: i32, mods: u32, x: f64, y: f64);
    fn vt_select(vt: *mut Vt, all: bool);
    fn vt_reset(vt: *mut Vt);
    fn vt_text(vt: *mut Vt, selected: bool, text: *mut *mut u8, len: *mut usize) -> bool;
    fn vt_text_free(text: *mut u8, len: usize);
    fn vt_info(vt: *mut Vt, info: *mut Info);
    fn vt_frame_begin(vt: *mut Vt, now: u64);
    fn vt_frame_end(vt: *mut Vt, frame: *mut Frame) -> bool;
    fn vt_image(
        vt: *mut Vt,
        id: u32,
        generation: u64,
        size: *mut u32,
        out: *mut c_void,
        alloc: Alloc,
    ) -> bool;
    fn vt_png(decode: extern "C" fn(*const u8, usize, *mut u32, *mut c_void, Alloc) -> bool);
}

struct Waker(*mut c_void, Host);

unsafe impl Send for Waker {}
unsafe impl Sync for Waker {}

impl Drop for Waker {
    fn drop(&mut self) {
        (self.1.drop_waker)(self.0);
    }
}

const WORKING: u32 = 1;
const DONE: u32 = 2;
const BLOCKED: u32 = 3;
const ERROR: u32 = 4;
const CLEAR: u32 = 5;
const STATES: [&str; 5] = ["idle", "working", "done", "blocked", "error"];
const KINDS: [&str; 4] = ["", "permission", "question", "auth"];
// The protocol allows 256 records at most.
const RECORDS: usize = 256;

struct Record {
    id: String,
    state: u32,
    kind: u32,
    progress: i32,
    app: String,
    title: String,
    message: String,
}

// What the terminal asks for while it reads output. The callbacks of the C side fill it.
#[derive(Default)]
struct Effects {
    replies: Vec<u8>,
    events: Vec<(u32, String)>,
    reports: Vec<Record>,
}

extern "C" fn on_reply(user: *mut c_void, data: *const u8, len: usize) {
    let effects = unsafe { &mut *user.cast::<Effects>() };
    effects
        .replies
        .extend_from_slice(unsafe { std::slice::from_raw_parts(data, len) });
}

extern "C" fn on_event(user: *mut c_void, kind: u32, text: *const u8, len: usize) {
    let effects = unsafe { &mut *user.cast::<Effects>() };
    effects.events.push((kind, Str { ptr: text, len }.text()));
}

extern "C" fn on_status(
    user: *mut c_void,
    state: u32,
    kind: u32,
    progress: i32,
    strings: *const Str,
) {
    let effects = unsafe { &mut *user.cast::<Effects>() };
    let strings = unsafe { std::slice::from_raw_parts(strings, 4) };
    effects.reports.push(Record {
        id: strings[0].text(),
        state,
        kind,
        progress,
        app: strings[1].text(),
        title: strings[2].text(),
        message: strings[3].text(),
    });
}

struct State {
    vt: *mut Vt,
    // Boxed, because the C side keeps its address.
    effects: Box<Effects>,
    input: Option<mpsc::Sender<Vec<u8>>>,
    // The side of the PTY that the terminal reads and writes.
    master: Option<File>,
    // Of the program, until it is reaped.
    pid: Option<u32>,
    // Counts the programs, so the threads of an old program stop.
    program: u64,
    // The newest record is last.
    records: Vec<Record>,
    reported: bool,
    // JSON events for the page, each with a comma after it.
    events: String,
    // The program holds the screen for a synchronized update.
    held: Option<Instant>,
    // The screen changed since the last poll.
    changed: bool,
    size: [u32; 4],
}

unsafe impl Send for State {}

impl Drop for State {
    fn drop(&mut self) {
        unsafe { vt_free(self.vt) };
    }
}

fn quote(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

// Shells report the directory as a `file://host/path` URL (OSC 7) or as a path.
fn path_of(pwd: &str) -> String {
    let Some(rest) = pwd.strip_prefix("file://") else {
        return pwd.to_owned();
    };
    let path = &rest.as_bytes()[rest.find('/').unwrap_or(rest.len())..];
    let mut bytes = Vec::with_capacity(path.len());
    let mut index = 0;
    while index < path.len() {
        let hex = |at: usize| path.get(at).and_then(|digit| (*digit as char).to_digit(16));
        if path[index] == b'%'
            && let (Some(high), Some(low)) = (hex(index + 1), hex(index + 2))
        {
            bytes.push((high * 16 + low) as u8);
            index += 3;
        } else {
            bytes.push(path[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

impl State {
    fn send(&mut self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        match &self.input {
            Some(input) => {
                let _ = input.send(bytes);
            }
            // Without a program, the page gets the input.
            None => {
                self.events.push_str("[\"data\",");
                quote(&mut self.events, &String::from_utf8_lossy(&bytes));
                self.events.push_str("],");
            }
        }
    }

    // Removes the records in these states.
    fn forget(&mut self, states: [u32; 2]) {
        let count = self.records.len();
        self.records
            .retain(|record| !states.contains(&record.state));
        self.reported |= count != self.records.len();
    }

    // Applies what the terminal asked for in the last call.
    fn settle(&mut self) {
        let effects = std::mem::take(&mut *self.effects);
        self.send(effects.replies);
        for (kind, text) in effects.events {
            match kind {
                0 => self.events.push_str("[\"bell\"],"),
                1 | 2 => {
                    self.events.push_str(if kind == 1 {
                        "[\"title\","
                    } else {
                        "[\"cwd\","
                    });
                    let text = if kind == 1 { text } else { path_of(&text) };
                    quote(&mut self.events, &text);
                    self.events.push_str("],");
                }
                // A full reset removes the title and the directory without their events.
                3 => self.events.push_str("[\"title\",\"\"],[\"cwd\",\"\"],"),
                4 => self.forget([WORKING, BLOCKED]),
                5 => self.held = Some(Instant::now()),
                _ => self.held = None,
            }
        }
        for report in effects.reports {
            let id = &report.id;
            if report.state == CLEAR {
                let count = self.records.len();
                self.records.retain(|record| {
                    !id.is_empty()
                        && record.id != *id
                        && !(record.id.starts_with(id.as_str())
                            && record.id.as_bytes()[id.len()] == b'/')
                });
                self.reported |= count != self.records.len();
                continue;
            }
            self.reported = true;
            self.records.retain(|record| record.id != *id);
            if self.records.len() == RECORDS {
                self.records.remove(0);
            }
            self.records.push(report);
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        unsafe { vt_write(self.vt, bytes.as_ptr(), bytes.len()) };
        self.changed = true;
        self.settle();
    }

    // A record without an app takes it from its nearest ancestor that has one.
    fn app<'a>(&'a self, record: &'a Record) -> &'a str {
        let mut id = record.id.as_str();
        let mut app = record.app.as_str();
        while app.is_empty() && !id.is_empty() {
            id = id.rfind('/').map_or("", |end| &id[..end]);
            app = self
                .records
                .iter()
                .find(|other| other.id == id)
                .map_or("", |other| &other.app);
        }
        app
    }

    fn poll(&mut self) -> String {
        let mut json = String::from("[");
        json.push_str(&std::mem::take(&mut self.events));
        if std::mem::take(&mut self.reported) {
            json.push_str("[\"status\",[");
            for record in &self.records {
                json.push_str("{\"id\":");
                quote(&mut json, &record.id);
                let _ = write!(
                    json,
                    ",\"state\":\"{}\",\"kind\":",
                    STATES.get(record.state as usize).unwrap_or(&"idle")
                );
                match KINDS.get(record.kind as usize) {
                    Some(kind) if record.kind > 0 && record.state == BLOCKED => {
                        quote(&mut json, kind)
                    }
                    _ => json.push_str("null"),
                }
                json.push_str(",\"progress\":");
                if record.progress >= 0 {
                    let _ = write!(json, "{}", record.progress);
                } else {
                    json.push_str("null");
                }
                for (key, text) in [
                    ("app", self.app(record)),
                    ("title", &record.title),
                    ("message", &record.message),
                ] {
                    let _ = write!(json, ",\"{key}\":");
                    quote(&mut json, text);
                }
                json.push_str("},");
            }
            if json.ends_with(',') {
                json.pop();
            }
            json.push_str("]],");
        }
        if json.ends_with(',') {
            json.pop();
        }
        json.push(']');
        json
    }

    // Sends the hangup signal to a program that still runs.
    fn hang_up(&mut self) {
        self.program += 1;
        self.held = None;
        self.input = None;
        self.master = None;
        if let Some(pid) = self.pid.take() {
            unsafe { libc::kill(pid as i32, libc::SIGHUP) };
        }
    }
}

struct Shared {
    state: Mutex<State>,
    waker: Waker,
    // The count of threads of the app that wait for the lock.
    waiting: AtomicU32,
    // The app has a wake that it did not answer with a poll yet.
    woken: AtomicBool,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.waiting.fetch_add(1, Ordering::Relaxed);
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.waiting.fetch_sub(1, Ordering::Relaxed);
        state
    }

    // The lock for the thread that reads the PTY. A program with much output takes the lock again and
    // again, and the lock is not fair, so this thread lets the app go first.
    fn reading(&self) -> MutexGuard<'_, State> {
        while self.waiting.load(Ordering::Relaxed) > 0 {
            std::thread::yield_now();
        }
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    // One wake is enough until the app polls.
    fn wake(&self) {
        if !self.woken.swap(true, Ordering::AcqRel) {
            (self.waker.1.wake)(self.waker.0);
        }
    }
}

pub struct Terminal {
    shared: Arc<Shared>,
    // The C side reads frames without the lock, so it has its own pointer.
    vt: *mut Vt,
    start: Instant,
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.shared.state().hang_up();
    }
}

// The name and the working directory of a process.
#[cfg(target_os = "macos")]
fn process(pid: i32) -> (String, String) {
    let text = |bytes: &[libc::c_char]| {
        let bytes: Vec<u8> = bytes
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| *byte as u8)
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let mut name = [0 as libc::c_char; 64];
    unsafe { libc::proc_name(pid, name.as_mut_ptr().cast(), name.len() as u32) };
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = size_of::<libc::proc_vnodepathinfo>() as i32;
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    let cwd = if read == size {
        text(info.pvi_cdir.vip_path.as_flattened())
    } else {
        String::new()
    };
    (text(&name), cwd)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn process(pid: i32) -> (String, String) {
    let name = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
    let cwd = std::fs::read_link(format!("/proc/{pid}/cwd"))
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    (name.trim_end().to_owned(), cwd)
}

impl Terminal {
    fn new(host: Host, cols: u16, rows: u16) -> Option<Self> {
        let mut effects = Box::<Effects>::default();
        let vt_host = VtHost {
            user: (&raw mut *effects).cast(),
            reply: on_reply,
            event: on_event,
            status: on_status,
        };
        static DECODER: std::sync::Once = std::sync::Once::new();
        DECODER.call_once(|| unsafe { vt_png(host.decode_png) });
        let vt = unsafe { vt_new(cols.max(1), rows.max(1), &vt_host) };
        if vt.is_null() {
            return None;
        }
        Some(Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    vt,
                    effects,
                    input: None,
                    master: None,
                    pid: None,
                    program: 0,
                    records: Vec::new(),
                    reported: false,
                    events: String::new(),
                    held: None,
                    changed: false,
                    size: [cols.max(1) as u32, rows.max(1) as u32, 8, 16],
                }),
                waker: Waker((host.waker)(), host),
                waiting: AtomicU32::new(0),
                woken: AtomicBool::new(false),
            }),
            vt,
            start: Instant::now(),
        })
    }

    // Starts a program in a new PTY. Without arguments it is the login shell of the user.
    fn spawn(&self, argv: &[&str], env: &[&str], cwd: &str) -> Result<u32, String> {
        let error = |error: &dyn std::fmt::Display| error.to_string();
        let mut state = self.shared.state();
        state.hang_up();
        let (mut master, mut slave) = (0, 0);
        let size = window_size(state.size);
        if unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &size as *const _ as *mut _,
            )
        } != 0
        {
            return Err(error(&std::io::Error::last_os_error()));
        }
        let (master, slave) = unsafe { (File::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        // Other programs of the app must not get the PTY.
        for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        }
        let home = std::env::var("HOME").unwrap_or_default();
        let mut command = match argv {
            // The login shell of the user. A shell is a login shell when its name starts with `-`.
            [] => {
                let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
                let mut command = Command::new(&shell);
                command.arg0(format!("-{}", shell.rsplit('/').next().unwrap_or("sh")));
                command
            }
            [program, args @ ..] => {
                let mut command = Command::new(program);
                command.args(args);
                command
            }
        };
        if !cwd.is_empty() {
            command.current_dir(cwd);
        } else if std::path::Path::new(&home).is_dir() {
            command.current_dir(&home);
        }
        if !env.is_empty() {
            command.env_clear();
        }
        for pair in env {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            command.env(key, value);
        }
        if !env.iter().any(|pair| pair.starts_with("TERM=")) {
            command.env("TERM", "xterm-256color");
            command.env("COLORTERM", "truecolor");
        }
        let files = unsafe { libc::getdtablesize() }.min(65536);
        let stdio = || slave.try_clone().map_err(|e| error(&e));
        command.stdin(stdio()?).stdout(stdio()?).stderr(slave);
        // WARNING: this runs in the new process before the program, where only system calls are safe.
        unsafe {
            command.pre_exec(move || {
                // The program gets the default signals, its own session with the PTY as its terminal, and
                // no other file of the app.
                for signal in [
                    libc::SIGCHLD,
                    libc::SIGHUP,
                    libc::SIGINT,
                    libc::SIGQUIT,
                    libc::SIGTERM,
                    libc::SIGALRM,
                ] {
                    libc::signal(signal, libc::SIG_DFL);
                }
                let mut none: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut none);
                libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
                if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                for fd in 3..files {
                    libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                }
                Ok(())
            })
        };
        let mut child = command.spawn().map_err(|e| error(&e))?;
        // The program has the other side of the PTY now.
        drop(command);
        let pid = child.id();
        let mut reader = master.try_clone().map_err(|e| error(&e))?;
        let mut writer = master.try_clone().map_err(|e| error(&e))?;
        let (input, bytes) = mpsc::channel::<Vec<u8>>();
        state.master = Some(master);
        state.input = Some(input);
        state.pid = Some(pid);
        let program = state.program;
        let thread = |name: &str, run: Box<dyn FnOnce() + Send>| {
            std::thread::Builder::new()
                .name(name.into())
                .spawn(run)
                .map_err(|e| error(&e))
        };
        let shared = self.shared.clone();
        thread(
            "apps-terminal-read",
            Box::new(move || {
                let mut bytes = vec![0; 65536];
                while let Ok(count @ 1..) = reader.read(&mut bytes) {
                    let mut state = shared.reading();
                    if state.program != program {
                        return;
                    }
                    state.write(&bytes[..count]);
                    drop(state);
                    shared.wake();
                }
            }),
        )?;
        thread(
            "apps-terminal-write",
            Box::new(move || {
                while let Ok(bytes) = bytes.recv() {
                    if writer.write_all(&bytes).is_err() {
                        break;
                    }
                }
            }),
        )?;
        let shared = self.shared.clone();
        thread(
            "apps-terminal-wait",
            Box::new(move || {
                let status = child.wait();
                let mut state = shared.state();
                if state.program != program {
                    return;
                }
                // Never signal a reaped child; its PID may belong to another process.
                state.pid = None;
                state.forget([WORKING, BLOCKED]);
                let _ = match status
                    .as_ref()
                    .map(|status| (status.signal(), status.code()))
                {
                    Ok((Some(signal), _)) => {
                        // The name of the signal, like `Terminated: 15`.
                        let name = unsafe { libc::strsignal(signal) };
                        let name = if name.is_null() {
                            format!("Signal {signal}")
                        } else {
                            unsafe { CStr::from_ptr(name) }
                                .to_string_lossy()
                                .into_owned()
                        };
                        write!(state.events, "[\"exit\",null,\"{name}\"],")
                    }
                    Ok((_, code)) => write!(state.events, "[\"exit\",{},null],", code.unwrap_or(1)),
                    Err(_) => write!(state.events, "[\"exit\",null,null],"),
                };
                drop(state);
                shared.wake();
            }),
        )?;
        Ok(pid)
    }

    fn info(&self) -> String {
        let state = self.shared.state();
        let mut info: Info = unsafe { std::mem::zeroed() };
        unsafe { vt_info(state.vt, &mut info) };
        let mut json = format!(
            "{{\"cursor\":[{},{},{}],\"alternate\":{},\"mouse\":{},\"bottom\":{},\"total\":{},\"offset\":{},\"title\":",
            info.cursor_x,
            info.cursor_y,
            info.cursor_visible != 0,
            info.alternate != 0,
            info.mouse != 0,
            info.bottom != 0,
            info.total,
            info.offset,
        );
        quote(&mut json, &info.title.text());
        let mut cwd = path_of(&info.pwd.text());
        json.push_str(",\"process\":");
        let foreground = state
            .master
            .as_ref()
            .filter(|_| state.pid.is_some())
            .map(|master| unsafe { libc::tcgetpgrp(master.as_raw_fd()) })
            .filter(|pid| *pid > 0);
        match foreground {
            Some(pid) => {
                let (name, directory) = process(pid);
                let _ = write!(json, "{{\"pid\":{pid},\"name\":");
                quote(&mut json, &name);
                json.push('}');
                // Shells that don't report their directory still have one.
                if cwd.is_empty() {
                    cwd = directory;
                }
            }
            _ => json.push_str("null"),
        }
        json.push_str(",\"cwd\":");
        quote(&mut json, &cwd);
        let _ = match state.pid {
            Some(pid) => write!(json, ",\"pid\":{pid}}}"),
            None => write!(json, ",\"pid\":null}}"),
        };
        json
    }
}

// The size of the PTY, from columns, rows and the pixels of a cell.
fn window_size([cols, rows, cell_w, cell_h]: [u32; 4]) -> libc::winsize {
    libc::winsize {
        ws_row: rows as u16,
        ws_col: cols as u16,
        ws_xpixel: (cols * cell_w).min(65535) as u16,
        ws_ypixel: (rows * cell_h).min(65535) as u16,
    }
}

fn output(out: *mut c_void, alloc: Alloc, bytes: &[u8]) {
    let target = alloc(out, bytes.len());
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len()) };
}

unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(ptr, len) }
    }
}

// Strings in a list end with a NUL byte.
unsafe fn list<'a>(ptr: *const u8, len: usize) -> Vec<&'a str> {
    unsafe { bytes(ptr, len) }
        .split(|byte| *byte == 0)
        .filter(|text| !text.is_empty())
        .filter_map(|text| std::str::from_utf8(text).ok())
        .collect()
}

#[unsafe(no_mangle)]
pub extern "C" fn apps_terminal_abi() -> u32 {
    ABI
}

/// Makes a terminal with no program. Returns null when it cannot.
///
/// # Safety
/// `host` points to a `Host`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_terminal_new(
    host: *const Host,
    cols: u16,
    rows: u16,
) -> *mut Terminal {
    Terminal::new(unsafe { *host }, cols, rows).map_or(std::ptr::null_mut(), |terminal| {
        Box::into_raw(Box::new(terminal))
    })
}

/// # Safety
/// `terminal` comes from `apps_terminal_new` and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_terminal_free(terminal: *mut Terminal) {
    drop(unsafe { Box::from_raw(terminal) });
}

/// Starts a program in a PTY, and ends the program before it. `argv` and `env` are lists of strings that
/// each end with a NUL byte. An empty `argv` is the login shell, an empty `env` is the environment of the
/// app. Returns the process ID, or -1 with the error text in `out`.
///
/// # Safety
/// `terminal` comes from `apps_terminal_new`, and the pointers hold their lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_terminal_spawn(
    terminal: *mut Terminal,
    argv: *const u8,
    argv_len: usize,
    env: *const u8,
    env_len: usize,
    cwd: *const u8,
    cwd_len: usize,
    out: *mut c_void,
    alloc: Alloc,
) -> i64 {
    let terminal = unsafe { &*terminal };
    let cwd = std::str::from_utf8(unsafe { bytes(cwd, cwd_len) }).unwrap_or("");
    match terminal.spawn(
        &unsafe { list(argv, argv_len) },
        &unsafe { list(env, env_len) },
        cwd,
    ) {
        Ok(pid) => pid as i64,
        Err(error) => {
            output(out, alloc, error.as_bytes());
            -1
        }
    }
}

/// What `apps_terminal_write` does with the bytes.
pub const INPUT: u32 = 0;
pub const PASTE: u32 = 1;
pub const OUTPUT: u32 = 2;

/// Sends bytes to the program as typed input (`INPUT`) or as a paste (`PASTE`), or shows bytes on the
/// screen as output of the program (`OUTPUT`).
///
/// # Safety
/// `terminal` comes from `apps_terminal_new`, and `data` points to `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_terminal_write(
    terminal: *mut Terminal,
    kind: u32,
    data: *const u8,
    len: usize,
) {
    let mut state = unsafe { &*terminal }.shared.state();
    let data = unsafe { bytes(data, len) };
    match kind {
        OUTPUT => state.write(data),
        PASTE => {
            let mut data = data.to_vec();
            unsafe { vt_paste(state.vt, data.as_mut_ptr(), data.len()) };
            state.settle();
        }
        _ => state.send(data.to_vec()),
    }
    if kind != OUTPUT {
        unsafe { vt_scroll(state.vt, 1, 0) };
    }
}

/// Sends a key to the program. `action` is 0 for a release, 1 for a press and 2 for a repeat. `name` is
/// the key name of the renderer, `text` is the text that the key types, and `mods` has the bits shift 1,
/// ctrl 2, alt 4 and super 8. Bit 16 makes the Option key of macOS an Alt key. Returns false when the key
/// sends nothing.
///
/// # Safety
/// `terminal` comes from `apps_terminal_new`, and the pointers hold their lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_terminal_key(
    terminal: *mut Terminal,
    action: u32,
    name: *const u8,
    name_len: usize,
    mods: u32,
    text: *const u8,
    text_len: usize,
) -> bool {
    let mut state = unsafe { &*terminal }.shared.state();
    let mut encoded = [0; 128];
    let len = unsafe {
        vt_key(
            state.vt,
            action,
            name,
            name_len,
            mods,
            text,
            text_len,
            encoded.as_mut_ptr(),
            encoded.len(),
        )
    };
    if len == 0 {
        return false;
    }
    if action != 0 {
        // The user is at the terminal, so they have seen what finished.
        state.forget([DONE, ERROR]);
        unsafe { vt_scroll(state.vt, 1, 0) };
    }
    state.send(encoded[..len].to_vec());
    true
}

/// A pointer event at a position in cell pixels from the top left of the grid. `kind` is 0 for a press,
/// 1 for a release and 2 for a move. `button` is 1 left, 2 right, 3 middle, or 0. `held` says that the left
/// button is down. Returns true when the program got the event, and false when it changed the selection.
///
/// # Safety
/// `terminal` comes from `apps_terminal_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_terminal_mouse(
    terminal: *mut Terminal,
    kind: u32,
    button: u32,
    mods: u32,
    x: f64,
    y: f64,
    held: bool,
) -> bool {
    let terminal = unsafe { &*terminal };
    let mut state = terminal.shared.state();
    let time = terminal.start.elapsed().as_nanos() as u64;
    let taken = unsafe { vt_mouse(state.vt, kind.min(2), button, mods, x, y, held, time) };
    state.settle();
    taken
}

/// Scrolls by rows, down when `rows` is positive. The program gets the wheel when it tracks the mouse.
///
/// # Safety
/// `terminal` comes from `apps_terminal_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_terminal_wheel(
    terminal: *mut Terminal,
    rows: i32,
    mods: u32,
    x: f64,
    y: f64,
) {
    let mut state = unsafe { &*terminal }.shared.state();
    unsafe { vt_wheel(state.vt, rows, mods, x, y) };
    state.settle();
}

pub const FOCUS: u32 = 0;
pub const SCROLL_TOP: u32 = 1;
pub const SCROLL_BOTTOM: u32 = 2;
pub const SCROLL_ROWS: u32 = 3;
pub const SELECT_ALL: u32 = 4;
pub const SELECT_NONE: u32 = 5;
pub const RESET: u32 = 6;
pub const KILL: u32 = 7;
pub const SCROLL_ROW: u32 = 8;

/// Runs one of the operations above. `value` is the focus state, the row count, the row from the top of the
/// scrollback, or the signal number.
///
/// # Safety
/// `terminal` comes from `apps_terminal_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_terminal_control(terminal: *mut Terminal, op: u32, value: f64) {
    let mut state = unsafe { &*terminal }.shared.state();
    let vt = state.vt;
    unsafe {
        match op {
            FOCUS => vt_focus(vt, value != 0.),
            SCROLL_TOP => vt_scroll(vt, 0, 0),
            SCROLL_BOTTOM => vt_scroll(vt, 1, 0),
            SCROLL_ROWS => vt_scroll(vt, 2, value as i32),
            SCROLL_ROW => vt_scroll(vt, 3, value as i32),
            SELECT_ALL => vt_select(vt, true),
            SELECT_NONE => vt_select(vt, false),
            RESET => vt_reset(vt),
            KILL => {
                if let Some(pid) = state.pid.filter(|pid| *pid > 0) {
                    // The program leads its process group.
                    libc::kill(-(pid as i32), value as i32);
                }
            }
            _ => {}
        }
    }
    state.settle();
}

/// Sets the grid size in cells and the cell size in pixels.
///
/// # Safety
/// `terminal` comes from `apps_terminal_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_terminal_resize(
    terminal: *mut Terminal,
    cols: u16,
    rows: u16,
    cell_w: u32,
    cell_h: u32,
) {
    let mut state = unsafe { &*terminal }.shared.state();
    let (cols, rows) = (cols.max(1), rows.max(1));
    state.size = [cols as u32, rows as u32, cell_w.max(1), cell_h.max(1)];
    unsafe { vt_resize(state.vt, cols, rows, cell_w.max(1), cell_h.max(1)) };
    state.settle();
    if let Some(master) = &state.master {
        let size = window_size(state.size);
        unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ as _, &size) };
    }
}

/// Sets the default colors: foreground, background, cursor, and the 16 colors of the palette, each as
/// `0x01RRGGBB`. A cursor or palette color of 0 keeps the default of the terminal.
///
/// # Safety
/// `terminal` comes from `apps_terminal_new`, and `colors` points to 19 colors.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_terminal_theme(
    terminal: *mut Terminal,
    colors: *const u32,
    dark: bool,
) {
    let mut state = unsafe { &*terminal }.shared.state();
    unsafe { vt_theme(state.vt, colors, dark) };
    state.settle();
}

/// Writes the events since the last poll as a JSON array of `[kind, ...values]`. The kinds are `title`,
/// `cwd`, `bell`, `exit` with the exit code and the signal, `status` with all program status records, and
/// `data` with the input for the program when there is no program. Returns true when the program changed
/// the screen since the last poll.
///
/// # Safety
/// `terminal` comes from `apps_terminal_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_terminal_poll(
    terminal: *mut Terminal,
    out: *mut c_void,
    alloc: Alloc,
) -> bool {
    let shared = &unsafe { &*terminal }.shared;
    shared.woken.store(false, Ordering::Release);
    let mut state = shared.state();
    output(out, alloc, state.poll().as_bytes());
    std::mem::take(&mut state.changed)
}

/// Writes the state of the terminal and of its foreground process as a JSON object.
///
/// # Safety
/// `terminal` comes from `apps_terminal_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_terminal_info(
    terminal: *mut Terminal,
    out: *mut c_void,
    alloc: Alloc,
) {
    output(out, alloc, unsafe { &*terminal }.info().as_bytes());
}

/// Writes the selected text, or all text with the scrollback. Returns false when there is none.
///
/// # Safety
/// `terminal` comes from `apps_terminal_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_terminal_text(
    terminal: *mut Terminal,
    selected: bool,
    out: *mut c_void,
    alloc: Alloc,
) -> bool {
    let state = unsafe { &*terminal }.shared.state();
    let (mut text, mut len) = (std::ptr::null_mut(), 0);
    if !unsafe { vt_text(state.vt, selected, &mut text, &mut len) } {
        return false;
    }
    output(out, alloc, unsafe { bytes(text, len) });
    unsafe { vt_text_free(text, len) };
    true
}

/// Reads the rows that changed since the last frame. Returns false when the frame has no data, which is
/// also the case while the program holds the screen for a synchronized update.
///
/// # Safety
/// `terminal` comes from `apps_terminal_new`, `frame` points to a `Frame`, and only one thread calls this.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_terminal_frame(terminal: *mut Terminal, frame: *mut Frame) -> bool {
    let terminal = unsafe { &*terminal };
    let state = terminal.shared.state();
    // A program that never ends its update must not freeze the screen.
    if state
        .held
        .is_some_and(|since| since.elapsed() < Duration::from_secs(1))
    {
        return false;
    }
    unsafe { vt_frame_begin(state.vt, terminal.start.elapsed().as_millis() as u64) };
    drop(state);
    unsafe { vt_frame_end(terminal.vt, frame) }
}

/// Writes the pixels of an image of a frame as BGRA, and its width and height to `size`. Returns false
/// when the terminal no longer has the image with this generation.
///
/// # Safety
/// `terminal` comes from `apps_terminal_new`, and `size` points to two numbers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_terminal_image(
    terminal: *mut Terminal,
    id: u32,
    generation: u64,
    size: *mut u32,
    out: *mut c_void,
    alloc: Alloc,
) -> bool {
    let state = unsafe { &*terminal }.shared.state();
    unsafe { vt_image(state.vt, id, generation, size, out, alloc) }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
