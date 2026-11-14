//! The `<terminal>` element and its sessions: the terminal library, and one terminal for each session of a page.

use super::gpu::{Alloc, Output, alloc, drop_waker, wake, waker};
use anyhow::{Context, Result, anyhow, bail, ensure};
use serde_json::{Value, json};
use std::{ffi::c_void, ptr::NonNull, sync::OnceLock};

const ABI: u32 = 3;

pub const LICENSE: &str = "Ghostty.LICENSE";

#[repr(C)]
struct Host {
    waker: extern "C" fn() -> *mut c_void,
    wake: extern "C" fn(*mut c_void),
    drop_waker: extern "C" fn(*mut c_void),
    decode_png: extern "C" fn(*const u8, usize, *mut u32, *mut c_void, Alloc) -> bool,
}

static HOST: Host = Host {
    waker,
    wake,
    drop_waker,
    decode_png,
};

// Decodes a PNG image of the Kitty graphics protocol. The terminal keeps RGBA pixels, and its memory has
// a limit, so `alloc` can return null.
extern "C" fn decode_png(
    data: *const u8,
    length: usize,
    size: *mut u32,
    output: *mut c_void,
    alloc: Alloc,
) -> bool {
    let bytes = unsafe { std::slice::from_raw_parts(data, length) };
    if !bytes.starts_with(b"\x89PNG") {
        return false;
    }
    let Ok((pixels, width, height)) = crate::render::image::pixels(bytes) else {
        return false;
    };
    let target = alloc(output, pixels.len());
    if target.is_null() {
        return false;
    }
    let target = unsafe { std::slice::from_raw_parts_mut(target, pixels.len()) };
    for (to, &[b, g, r, a]) in target
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(pixels.as_chunks::<4>().0)
    {
        *to = [r, g, b, a];
    }
    unsafe { size.copy_from([width as u32, height as u32].as_ptr(), 2) };
    true
}

/// One cell of a row. Colors are `0x01RRGGBB`, or 0 for the default color.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Cell {
    pub ch: u32,
    pub fg: u32,
    pub bg: u32,
    pub underline: u32,
    pub flags: u16,
    pub extra: u16,
}

pub(crate) const BOLD: u16 = 1;
pub(crate) const ITALIC: u16 = 1 << 1;
pub(crate) const FAINT: u16 = 1 << 2;
pub(crate) const INVERSE: u16 = 1 << 4;
pub(crate) const INVISIBLE: u16 = 1 << 5;
pub(crate) const STRIKETHROUGH: u16 = 1 << 6;
pub(crate) const OVERLINE: u16 = 1 << 7;
pub(crate) const UNDERLINE: u16 = 7 << 8;
pub(crate) const SPACER: u16 = 1 << 12;
pub(crate) const SELECTED: u16 = 1 << 13;

/// A rectangle of an image on the screen.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Placement {
    /// The generation of the image, which is the key of its pixels.
    pub image: u64,
    pub id: u32,
    /// 0: below the backgrounds of the cells, 1: below the text, 2: above the text.
    pub layer: u32,
    pub z: i32,
    /// The top left cell, which can be outside the grid.
    pub col: i32,
    pub row: i32,
    /// In the pixels that the program sees: the offset in that cell, the size, and the part of the image.
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub source: [u32; 4],
}

/// The rows that changed since the last frame. The pointers are valid until the next frame.
#[repr(C)]
pub(crate) struct Frame {
    pub cols: u16,
    pub rows: u16,
    pub cursor_x: u16,
    pub cursor_y: u16,
    /// 0: no cursor. Else 1 bar, 2 block, 3 underline, 4 hollow block.
    pub cursor: u8,
    pub cursor_blinks: u8,
    pub cursor_wide: u8,
    pub full: u8,
    pub foreground: u32,
    pub background: u32,
    pub cursor_color: u32,
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

type Raw = *mut c_void;

struct Library {
    new: unsafe extern "C" fn(*const Host, u16, u16) -> Raw,
    free: unsafe extern "C" fn(Raw),
    spawn: unsafe extern "C" fn(
        Raw,
        *const u8,
        usize,
        *const u8,
        usize,
        *const u8,
        usize,
        *mut c_void,
        Alloc,
    ) -> i64,
    write: unsafe extern "C" fn(Raw, u32, *const u8, usize),
    key: unsafe extern "C" fn(Raw, u32, *const u8, usize, u32, *const u8, usize) -> bool,
    mouse: unsafe extern "C" fn(Raw, u32, u32, u32, f64, f64, bool) -> bool,
    wheel: unsafe extern "C" fn(Raw, i32, u32, f64, f64),
    control: unsafe extern "C" fn(Raw, u32, f64),
    resize: unsafe extern "C" fn(Raw, u16, u16, u32, u32),
    theme: unsafe extern "C" fn(Raw, *const u32, bool),
    poll: unsafe extern "C" fn(Raw, *mut c_void, Alloc) -> bool,
    info: unsafe extern "C" fn(Raw, *mut c_void, Alloc),
    text: unsafe extern "C" fn(Raw, bool, *mut c_void, Alloc) -> bool,
    frame: unsafe extern "C" fn(Raw, *mut Frame) -> bool,
    image: unsafe extern "C" fn(Raw, u32, u64, *mut u32, *mut c_void, Alloc) -> bool,
}

pub fn library_name(platform: &str) -> &'static str {
    match platform {
        "macos" => "libGhostty.dylib",
        "windows" => "Ghostty.dll",
        _ => "libGhostty.so",
    }
}

fn load() -> Result<Library> {
    let library = super::gpu::library(
        library_name(std::env::consts::OS),
        "terminal",
        b"apps_terminal_abi\0",
        ABI,
    )
    .context("The terminal library needs libghostty-vt: run bash scripts/build-ghostty.sh")?;
    unsafe {
        Ok(Library {
            new: *library.get(b"apps_terminal_new\0")?,
            free: *library.get(b"apps_terminal_free\0")?,
            spawn: *library.get(b"apps_terminal_spawn\0")?,
            write: *library.get(b"apps_terminal_write\0")?,
            key: *library.get(b"apps_terminal_key\0")?,
            mouse: *library.get(b"apps_terminal_mouse\0")?,
            wheel: *library.get(b"apps_terminal_wheel\0")?,
            control: *library.get(b"apps_terminal_control\0")?,
            resize: *library.get(b"apps_terminal_resize\0")?,
            theme: *library.get(b"apps_terminal_theme\0")?,
            poll: *library.get(b"apps_terminal_poll\0")?,
            info: *library.get(b"apps_terminal_info\0")?,
            text: *library.get(b"apps_terminal_text\0")?,
            frame: *library.get(b"apps_terminal_frame\0")?,
            image: *library.get(b"apps_terminal_image\0")?,
        })
    }
}

pub(crate) const FOCUS: u32 = 0;
pub(crate) const SCROLL_ROW: u32 = 8;
const KILL: u32 = 7;

pub(crate) struct Terminal {
    raw: NonNull<c_void>,
    library: &'static Library,
    /// Columns and rows.
    pub size: [u16; 2],
    /// The size of a cell in CSS pixels, and in the pixels that the program sees.
    pub cell: [f32; 2],
    pub cell_pixels: [u32; 2],
    /// The top left of the grid in the page, as of the last frame.
    pub origin: [f32; 2],
    pub focused: bool,
    /// The text that an input method composes.
    pub preedit: String,
    /// Events from this side for the scripts.
    events: Vec<Value>,
    /// The contexts that have an object for the session: bit 0 is the page, and bit 1 is its preload script.
    worlds: u8,
    title: Value,
    status: Value,
    wheel: f32,
    pub drawn: crate::render::terminal::Drawn,
}

impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe { (self.library.free)(self.raw.as_ptr()) };
    }
}

impl Terminal {
    pub fn new() -> Result<Self> {
        static LIBRARY: OnceLock<Result<Library, String>> = OnceLock::new();
        let library = LIBRARY
            .get_or_init(|| load().map_err(|error| format!("{error:#}")))
            .as_ref()
            .map_err(|error| anyhow!("{error}"))?;
        let raw = NonNull::new(unsafe { (library.new)(&HOST, 80, 24) })
            .context("Cannot create the terminal")?;
        Ok(Self {
            raw,
            library,
            size: [80, 24],
            cell: [8., 16.],
            cell_pixels: [8, 16],
            origin: [0.; 2],
            focused: false,
            preedit: String::new(),
            events: Vec::new(),
            worlds: 0,
            title: json!(""),
            status: json!([]),
            wheel: 0.,
            drawn: Default::default(),
        })
    }

    /// Starts a program in a PTY. An empty `argv` is the login shell of the user, and an empty `env` is the
    /// environment of the app.
    pub fn spawn(&mut self, argv: &[String], env: &[String], cwd: &str) -> Result<u32> {
        let list = |items: &[String]| {
            let mut bytes = Vec::new();
            for item in items {
                bytes.extend_from_slice(item.as_bytes());
                bytes.push(0);
            }
            bytes
        };
        let (argv, env) = (list(argv), list(env));
        let mut output = Output::default();
        let pid = unsafe {
            (self.library.spawn)(
                self.raw.as_ptr(),
                argv.as_ptr(),
                argv.len(),
                env.as_ptr(),
                env.len(),
                cwd.as_ptr(),
                cwd.len(),
                (&raw mut output).cast(),
                alloc,
            )
        };
        ensure!(
            pid >= 0,
            "Cannot start the program: {}",
            String::from_utf8_lossy(&output.finish())
        );
        Ok(pid as u32)
    }

    /// `kind` is 0 for typed input, 1 for a paste, and 2 for output of the program.
    pub fn write(&mut self, kind: u32, bytes: &[u8]) {
        unsafe { (self.library.write)(self.raw.as_ptr(), kind, bytes.as_ptr(), bytes.len()) };
    }

    pub fn control(&mut self, op: u32, value: f64) {
        unsafe { (self.library.control)(self.raw.as_ptr(), op, value) };
    }

    pub fn resize(&mut self, size: [u16; 2], cell: [f32; 2], scale: f32) {
        self.size = size;
        self.cell = cell;
        self.cell_pixels = cell.map(|side| (side * scale).round().max(1.) as u32);
        let [width, height] = self.cell_pixels;
        unsafe { (self.library.resize)(self.raw.as_ptr(), size[0], size[1], width, height) };
        self.events.push(json!(["resize"]));
    }

    /// Sets the default colors: foreground, background, cursor and the 16 palette colors.
    pub fn theme(&mut self, colors: &[u32; 19], dark: bool) {
        unsafe { (self.library.theme)(self.raw.as_ptr(), colors.as_ptr(), dark) };
    }

    pub fn frame(&mut self, frame: &mut Frame) -> bool {
        unsafe { (self.library.frame)(self.raw.as_ptr(), frame) }
    }

    /// The pixels of an image of the last frame.
    pub fn image(&self, placement: &Placement) -> Option<renderer::RenderImage> {
        let (mut size, mut output) = ([0; 2], Output::default());
        unsafe {
            (self.library.image)(
                self.raw.as_ptr(),
                placement.id,
                placement.image,
                size.as_mut_ptr(),
                (&raw mut output).cast(),
                alloc,
            )
        }
        .then(|| renderer::RenderImage::new(size[0], size[1], output.finish()))
    }

    // A position of the page in the cell pixels that the program sees.
    fn at(&self, x: f64, y: f64) -> [f64; 2] {
        [0, 1].map(|axis| {
            ([x, y][axis] - self.origin[axis] as f64) / self.cell[axis] as f64
                * self.cell_pixels[axis] as f64
        })
    }

    /// Scrolls by a distance in CSS pixels, down when it is positive.
    pub fn wheel(&mut self, delta: f32, mods: u32, x: f32, y: f32) {
        self.wheel += delta / self.cell[1];
        let rows = self.wheel.trunc();
        if rows == 0. {
            return;
        }
        self.wheel -= rows;
        let [x, y] = self.at(x as f64, y as f64);
        unsafe { (self.library.wheel)(self.raw.as_ptr(), rows as i32, mods, x, y) };
    }

    fn json(&mut self, read: impl FnOnce(&Library, Raw, *mut c_void) -> bool) -> (bool, Vec<u8>) {
        let mut output = Output::default();
        let result = read(self.library, self.raw.as_ptr(), (&raw mut output).cast());
        (result, output.finish())
    }
}

/// The sessions of a page by ID, and the session that each `<terminal>` element shows.
#[derive(Default)]
pub(crate) struct Terminals {
    sessions: rustc_hash::FxHashMap<u32, Terminal>,
    /// The session of an element, and whether the element was in the document.
    views: rustc_hash::FxHashMap<usize, (u32, bool)>,
    /// The bar of an element: its scroll position, and the offset that the last frame gave it.
    pub bars: rustc_hash::FxHashMap<usize, (renderer::ScrollRef, renderer::Pixels)>,
    /// The events for the page, and for its preload script.
    events: [Vec<Value>; 2],
    last: u32,
    /// The revision of the document when the views were last checked.
    revision: u64,
}

impl Terminals {
    /// The terminal that an element shows.
    pub fn shown(&mut self, node: usize) -> Option<&mut Terminal> {
        self.sessions.get_mut(&self.views.get(&node)?.0)
    }

    /// The terminal that an element shows, with the bar of the element. Without a session, the bar has no range.
    pub fn drawn(
        &mut self,
        node: usize,
    ) -> Option<(&mut Terminal, &mut (renderer::ScrollRef, renderer::Pixels))> {
        let bar = self.bars.get_mut(&node)?;
        let terminal = self
            .views
            .get(&node)
            .and_then(|view| self.sessions.get_mut(&view.0));
        if terminal.is_none() {
            bar.0.set_range(Default::default(), Default::default());
            bar.0.set_offset(Default::default());
            bar.1 = Default::default();
        }
        Some((terminal?, bar))
    }

    /// Ends the session of an element that is gone.
    pub fn release(&mut self, node: usize) {
        self.bars.remove(&node);
        if let Some((session, _)) = self.views.remove(&node) {
            self.close(session);
        }
    }

    fn close(&mut self, session: u32) {
        let Some(terminal) = self.sessions.remove(&session) else {
            return;
        };
        self.views.retain(|_, view| view.0 != session);
        for (world, events) in self.events.iter_mut().enumerate() {
            if terminal.worlds >> world & 1 != 0 {
                events.push(json!([session, "close"]));
            }
        }
    }
}

const SIGNALS: [(&str, i32); 5] = [
    ("SIGHUP", 1),
    ("SIGINT", 2),
    ("SIGQUIT", 3),
    ("SIGKILL", 9),
    ("SIGTERM", 15),
];

impl crate::host::Host {
    /// Runs an operation of a terminal session for the page, or for its preload script.
    pub fn terminal(&mut self, id: u64, preload: bool, op: &str, args: &Value) -> Result<Value> {
        ensure!(
            self.enabled("terminal"),
            "The terminal feature is disabled. Add \"terminal\" to \"features\" in package.json"
        );
        // Only a preload script starts programs, and only the page has elements.
        ensure!(
            if preload {
                op != "attach"
            } else {
                !matches!(op, "create" | "spawn")
            },
            "terminal.{op} is unavailable in this context"
        );
        let page = self.pages.get_mut(&id).context("Window is closed")?;
        let state = self.dom.entry(id).or_default();
        let terminals = state.terminals.clone();
        let mut terminals = terminals.borrow_mut();
        let terminals = &mut *terminals;
        let world = usize::from(preload);
        if op == "poll" {
            // An element that left the document ends its session. One that moved in a task is still there.
            let revision = page.doc.nodes.latest();
            if std::mem::replace(&mut terminals.revision, revision) != revision {
                let mut gone = Vec::new();
                for (&node, view) in &mut terminals.views {
                    if crate::host::reaches(&page.doc, &state.hosts, node, 0) {
                        view.1 = true;
                    } else if view.1 {
                        gone.push(node);
                    }
                }
                for node in gone {
                    terminals.release(node);
                }
            }
            for (session, terminal) in &mut terminals.sessions {
                let (changed, json) = terminal
                    .json(|library, raw, output| unsafe { (library.poll)(raw, output, alloc) });
                page.dirty |= changed;
                let polled: Vec<Value> = serde_json::from_slice(&json)?;
                for mut event in std::mem::take(&mut terminal.events)
                    .into_iter()
                    .chain(polled)
                {
                    match event[0].as_str() {
                        Some("title") => terminal.title = event[1].take(),
                        Some("status") => terminal.status = event[1].take(),
                        _ => {}
                    }
                    if let Some(event) = event.as_array_mut() {
                        event.insert(0, json!(session));
                    }
                    for (world, events) in terminals.events.iter_mut().enumerate() {
                        if terminal.worlds >> world & 1 != 0 {
                            events.push(event.clone());
                        }
                    }
                }
            }
            return Ok(json!(std::mem::take(&mut terminals.events[world])));
        }
        let number = |index: usize| args[index].as_f64().unwrap_or(0.);
        // A string is a command line for the login shell of the user.
        let program = || -> Result<_> {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
            let argv = match &args[1] {
                Value::String(command) if cfg!(windows) => vec!["cmd.exe", "/C", command],
                Value::String(command) => vec![&shell, "-l", "-c", command],
                list => list
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|arg| arg.as_str().context("Expected argument strings"))
                    .collect::<Result<Vec<_>>>()?,
            };
            let argv: Vec<String> = argv.into_iter().map(str::to_owned).collect();
            let env = args[2]["env"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(key, value)| {
                    let value = value.as_str().context("Expected environment strings")?;
                    Ok(format!("{key}={value}"))
                })
                .collect::<Result<Vec<_>>>()?;
            Ok((argv, env, args[2]["cwd"].as_str().unwrap_or("")))
        };
        match op {
            "create" => {
                let mut terminal = Terminal::new()?;
                terminal.worlds = 2;
                if args[0] != true {
                    let (argv, env, cwd) = program()?;
                    terminal.spawn(&argv, &env, cwd)?;
                }
                terminals.last += 1;
                terminals.sessions.insert(terminals.last, terminal);
                return Ok(json!(terminals.last));
            }
            "attach" => {
                let node = args[0].as_u64().context("Invalid node")? as usize;
                ensure!(
                    page.doc
                        .nodes
                        .get(node)
                        .is_some_and(|node| node.tag == "terminal"),
                    "Expected a terminal element"
                );
                terminals.views.remove(&node);
                if let Some(session) = args[1].as_u64() {
                    let session = session as u32;
                    ensure!(
                        terminals.sessions.contains_key(&session),
                        "The terminal session is closed"
                    );
                    // A session shows in one element.
                    terminals.views.retain(|_, view| view.0 != session);
                    terminals.views.insert(node, (session, false));
                    terminals.revision = 0;
                }
                page.dirty = true;
                return Ok(Value::Null);
            }
            _ => {}
        }
        let session = args[0].as_u64().unwrap_or(0) as u32;
        if op == "close" {
            terminals.close(session);
            page.dirty = true;
            return Ok(Value::Null);
        }
        let terminal = terminals.sessions.get_mut(&session);
        if op == "open" {
            return Ok(json!(
                terminal
                    .map(|terminal| terminal.worlds |= 1 << world)
                    .is_some()
            ));
        }
        let terminal = terminal.context("The terminal session is closed")?;
        let raw = terminal.raw.as_ptr();
        // Reads draw no frame, and neither does a pointer that only moves above the terminal.
        page.dirty |=
            !matches!(op, "info" | "text") && !(op == "mouse" && args[1] == 2 && args[6] != true);
        match op {
            "spawn" => {
                let (argv, env, cwd) = program()?;
                return Ok(json!(terminal.spawn(&argv, &env, cwd)?));
            }
            "write" => terminal.write(number(1) as u32, crate::node::bytes(&args[2])?),
            "key" => {
                let name = args[2].as_str().unwrap_or("");
                let text = args[3].as_str().unwrap_or("");
                return Ok(json!(unsafe {
                    (terminal.library.key)(
                        raw,
                        number(1) as u32,
                        name.as_ptr(),
                        name.len(),
                        number(4) as u32,
                        text.as_ptr(),
                        text.len(),
                    )
                }));
            }
            "mouse" => {
                let [x, y] = terminal.at(number(4), number(5));
                return Ok(json!(unsafe {
                    (terminal.library.mouse)(
                        raw,
                        number(1) as u32,
                        number(2) as u32,
                        number(3) as u32,
                        x,
                        y,
                        args[6].as_bool().unwrap_or(false),
                    )
                }));
            }
            "control" => {
                let op = number(1) as u32;
                let value = match args[2].as_str() {
                    Some(signal) if op == KILL => {
                        SIGNALS
                            .iter()
                            .find(|(name, _)| *name == signal)
                            .with_context(|| format!("Unsupported signal {signal}"))?
                            .1 as f64
                    }
                    _ => number(2),
                };
                terminal.control(op, value);
            }
            "info" => {
                let (_, json) = terminal.json(|library, raw, output| unsafe {
                    (library.info)(raw, output, alloc);
                    true
                });
                let mut info: Value = serde_json::from_slice(&json)?;
                info["cols"] = json!(terminal.size[0]);
                info["rows"] = json!(terminal.size[1]);
                info["title"] = terminal.title.clone();
                info["status"] = terminal.status.clone();
                return Ok(info);
            }
            "text" => {
                let (found, text) = terminal.json(|library, raw, output| unsafe {
                    (library.text)(raw, args[1].as_bool().unwrap_or(false), output, alloc)
                });
                return Ok(if found {
                    json!(String::from_utf8_lossy(&text))
                } else {
                    Value::Null
                });
            }
            "preedit" => terminal.preedit = args[1].as_str().unwrap_or("").to_owned(),
            _ => bail!("Unknown terminal operation: {op}"),
        }
        Ok(Value::Null)
    }
}
