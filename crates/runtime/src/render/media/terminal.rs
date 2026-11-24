//! The terminal library, and one terminal for each session of a page.

use super::gpu::{Alloc, Output, alloc, drop_waker, wake, waker};
use anyhow::{Context, Result, anyhow, ensure};
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
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let Ok(mut reader) = decoder.read_info() else {
        return false;
    };
    let (width, height) = reader.info().size();
    if width > 16384 || height > 16384 || width as usize * height as usize > 64 << 20 {
        return false;
    }
    let channels = reader.output_color_type().0.samples();
    let length = width as usize * height as usize * 4;
    let Some(needed) = reader.output_buffer_size() else {
        return false;
    };
    let target = alloc(output, length);
    if target.is_null() {
        return false;
    }
    let target = unsafe { std::slice::from_raw_parts_mut(target, length) };
    // RGBA decodes in place. Other layouts expand to RGBA in one pass.
    if channels == 4 && needed == length {
        if reader.next_frame(target).is_err() {
            return false;
        }
    } else {
        let mut pixels = vec![0; needed];
        if reader.next_frame(&mut pixels).is_err() {
            return false;
        }
        let to = target.as_chunks_mut::<4>().0.iter_mut();
        match channels {
            4 => to
                .zip(pixels.as_chunks::<4>().0)
                .for_each(|(to, from)| *to = *from),
            3 => to
                .zip(pixels.as_chunks::<3>().0)
                .for_each(|(to, &[r, g, b])| *to = [r, g, b, 255]),
            2 => to
                .zip(pixels.as_chunks::<2>().0)
                .for_each(|(to, &[l, a])| *to = [l, l, l, a]),
            _ => to.zip(&pixels).for_each(|(to, &l)| *to = [l, l, l, 255]),
        }
    }
    unsafe { size.copy_from([width, height].as_ptr(), 2) };
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
    /// The time in ms until an animated image has its next picture, or 0. In a frame without data, the
    /// time until the program stops holding the screen.
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
    let library = crate::native::library::load(
        library_name(std::env::consts::OS),
        "terminal",
        b"apps_terminal_abi\0",
        ABI,
        "",
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
pub(crate) const KILL: u32 = 7;

pub(crate) struct Terminal {
    raw: NonNull<c_void>,
    library: &'static Library,
    /// Columns and rows.
    size: [u16; 2],
    /// The size of a cell in CSS pixels, and in the pixels that the program sees.
    cell: [f32; 2],
    cell_pixels: [u32; 2],
    /// The scale of the window, as of the last frame.
    scale: f32,
    /// The top left of the grid in the page, as of the last frame.
    origin: [f32; 2],
    focused: bool,
    /// The text that an input method composes.
    preedit: String,
    /// Events from this side for the scripts.
    events: Vec<Value>,
    /// The contexts that have an object for the session: bit 0 is the page, and bit 1 is its preload script.
    worlds: u8,
    title: Value,
    status: Value,
    wheel: f32,
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
            scale: 0.,
            origin: [0.; 2],
            focused: false,
            preedit: String::new(),
            events: Vec::new(),
            worlds: 0,
            title: json!(""),
            status: json!([]),
            wheel: 0.,
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

    /// Fits the grid to a frame: its top left in the page, its columns and rows, and the size of a cell in CSS
    /// pixels at a scale. Returns whether the grid has a new size.
    pub fn fit(&mut self, origin: [f32; 2], size: [u16; 2], cell: [f32; 2], scale: f32) -> bool {
        self.origin = origin;
        if size == self.size && cell == self.cell && scale == self.scale {
            return false;
        }
        self.size = size;
        self.cell = cell;
        self.scale = scale;
        // The program sees cells of whole pixels.
        self.cell_pixels = cell.map(|side| (side * scale).round().max(1.) as u32);
        let [width, height] = self.cell_pixels;
        unsafe { (self.library.resize)(self.raw.as_ptr(), size[0], size[1], width, height) };
        self.events.push(json!(["resize"]));
        true
    }

    pub fn focus(&mut self, focused: bool) {
        if focused != self.focused {
            self.focused = focused;
            self.control(FOCUS, f64::from(u8::from(focused)));
        }
    }

    pub fn preedit(&self) -> &str {
        &self.preedit
    }

    pub(crate) fn set_preedit(&mut self, text: &str) {
        text.clone_into(&mut self.preedit);
    }

    /// A context of `world` got an object for the session.
    pub(crate) fn open_in(&mut self, world: usize) {
        self.worlds |= 1 << world;
    }

    fn opened_in(&self, world: usize) -> bool {
        self.worlds >> world & 1 != 0
    }

    pub(crate) fn key(&mut self, action: u32, name: &str, mods: u32, text: &str) -> bool {
        unsafe {
            (self.library.key)(
                self.raw.as_ptr(),
                action,
                name.as_ptr(),
                name.len(),
                mods,
                text.as_ptr(),
                text.len(),
            )
        }
    }

    pub(crate) fn mouse(
        &mut self,
        action: u32,
        button: u32,
        mods: u32,
        x: f64,
        y: f64,
        any: bool,
    ) -> bool {
        let [x, y] = self.at(x, y);
        unsafe { (self.library.mouse)(self.raw.as_ptr(), action, button, mods, x, y, any) }
    }

    /// The state for `info`: the library's fields, with the size, title and status.
    pub(crate) fn info(&mut self) -> Result<Value> {
        let (_, json) = self.json(|library, raw, output| unsafe {
            (library.info)(raw, output, alloc);
            true
        });
        let mut info: Value = serde_json::from_slice(&json)?;
        info["cols"] = json!(self.size[0]);
        info["rows"] = json!(self.size[1]);
        info["title"] = self.title.clone();
        info["status"] = self.status.clone();
        Ok(info)
    }

    /// The text of the screen and the scrollback, or of the selection.
    pub(crate) fn text(&mut self, selected: bool) -> Option<String> {
        let (found, text) = self
            .json(|library, raw, output| unsafe { (library.text)(raw, selected, output, alloc) });
        found.then(|| String::from_utf8_lossy(&text).into_owned())
    }

    /// Events for the scripts, from this side and from the library. Returns whether the screen changed.
    fn poll(&mut self) -> Result<(bool, Vec<Value>)> {
        let (changed, json) =
            self.json(|library, raw, output| unsafe { (library.poll)(raw, output, alloc) });
        let polled: Vec<Value> = serde_json::from_slice(&json)?;
        let mut events = std::mem::take(&mut self.events);
        for mut event in polled {
            match event[0].as_str() {
                Some("title") => self.title = event[1].take(),
                Some("status") => self.status = event[1].take(),
                _ => {}
            }
            events.push(event);
        }
        Ok((changed, events))
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
    /// The revision of the document when the views were last checked.
    revision: u64,
}

impl Terminals {
    /// The session that an element shows.
    pub fn session(&self, node: usize) -> Option<u32> {
        self.views.get(&node).map(|view| view.0)
    }

    /// The terminal that an element shows.
    pub fn shown(&mut self, node: usize) -> Option<&mut Terminal> {
        self.sessions.get_mut(&self.views.get(&node)?.0)
    }

    /// The session that an element shows, with its terminal and the bar of the element. Without a session, the
    /// bar has no range.
    pub fn drawn(
        &mut self,
        node: usize,
    ) -> Option<(
        u32,
        &mut Terminal,
        &mut (renderer::ScrollRef, renderer::Pixels),
    )> {
        let bar = self.bars.get_mut(&node)?;
        let session = self.views.get(&node).map(|view| view.0);
        let terminal = session.and_then(|session| self.sessions.get_mut(&session));
        if terminal.is_none() {
            bar.0.set_range(Default::default(), Default::default());
            bar.0.set_offset(Default::default());
            bar.1 = Default::default();
        }
        Some((session?, terminal?, bar))
    }

    pub(crate) fn get(&mut self, session: u32) -> Option<&mut Terminal> {
        self.sessions.get_mut(&session)
    }

    pub fn contains(&self, session: u32) -> bool {
        self.sessions.contains_key(&session)
    }

    pub(crate) fn create(&mut self, mut terminal: Terminal) -> u32 {
        // Only a preload script creates sessions.
        terminal.open_in(1);
        // IDs are unique in the process, so a reloaded page never takes the drawing of an old session.
        static LAST: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let session = LAST.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        self.sessions.insert(session, terminal);
        session
    }

    // Shows a session in an element, or nothing. A session shows in one element.
    pub(crate) fn attach(&mut self, node: usize, session: Option<u32>) -> Result<()> {
        self.views.remove(&node);
        if let Some(session) = session {
            ensure!(
                self.sessions.contains_key(&session),
                "The terminal session is closed"
            );
            self.views.retain(|_, view| view.0 != session);
            self.views.insert(node, (session, false));
            self.revision = 0;
        }
        Ok(())
    }

    /// The events of all sessions for one world. At a new document `revision`, the sessions of elements that
    /// left the document end. One that moved in a task is still there.
    pub(crate) fn poll(
        &mut self,
        revision: u64,
        world: usize,
        attached: impl Fn(usize) -> bool,
    ) -> Result<(bool, Vec<Value>)> {
        if std::mem::replace(&mut self.revision, revision) != revision {
            let mut gone = Vec::new();
            for (&node, view) in &mut self.views {
                if attached(node) {
                    view.1 = true;
                } else if view.1 {
                    gone.push(node);
                }
            }
            for node in gone {
                self.release(node);
            }
        }
        let mut dirty = false;
        for (session, terminal) in &mut self.sessions {
            let (changed, events) = terminal.poll()?;
            dirty |= changed;
            for mut event in events {
                if let Some(event) = event.as_array_mut() {
                    event.insert(0, json!(session));
                }
                for (world, events) in self.events.iter_mut().enumerate() {
                    if terminal.opened_in(world) {
                        events.push(event.clone());
                    }
                }
            }
        }
        Ok((dirty, std::mem::take(&mut self.events[world])))
    }

    /// Ends the session of an element that is gone.
    pub fn release(&mut self, node: usize) {
        self.bars.remove(&node);
        if let Some((session, _)) = self.views.remove(&node) {
            self.close(session);
        }
    }

    pub(crate) fn close(&mut self, session: u32) {
        let Some(terminal) = self.sessions.remove(&session) else {
            return;
        };
        self.views.retain(|_, view| view.0 != session);
        for (world, events) in self.events.iter_mut().enumerate() {
            if terminal.opened_in(world) {
                events.push(json!([session, "close"]));
            }
        }
    }
}
