//! The FPS HUD that `apps run` shows on Cmd+Alt+F. The apps_perfhud library next to the runtime measures and
//! lays it out. `apps build` doesn't ship the library, so built apps can't show it.

use super::ViewData;
use anyhow::{Context, Result, ensure};
use renderer::{Bounds, TextAlign, TextRun, Timer, Window, point, px, size};
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    ptr::NonNull,
    rc::{Rc, Weak},
    sync::OnceLock,
    time::{Duration, Instant},
};

const ABI: u32 = 1;
const FONT: &str = if cfg!(target_os = "macos") {
    "Menlo"
} else if cfg!(windows) {
    "Consolas"
} else {
    "DejaVu Sans Mono"
};

/// A box when `len` is 0, else a line of text, as the library lays them out.
#[repr(C)]
struct Item {
    bounds: [f32; 4],
    color: u32,
    /// The corner radius of a box, or the font size of text.
    size: f32,
    align: u32,
    text: *const u8,
    len: usize,
}

type Paint = extern "C" fn(*mut c_void, &Item);

struct Library {
    new: extern "C" fn() -> *mut c_void,
    free: unsafe extern "C" fn(*mut c_void),
    tick: unsafe extern "C" fn(*mut c_void) -> bool,
    paint: unsafe extern "C" fn(*mut c_void, u64, bool, f32, f32, f32, *mut c_void, Paint),
}

pub(crate) struct Hud {
    raw: NonNull<c_void>,
    library: &'static Library,
    /// When the frame that is drawn now started to render.
    pub(crate) started: Cell<Instant>,
    timer: RefCell<Option<Timer>>,
}

impl Hud {
    /// Shows or hides the HUD of `view`.
    pub(crate) fn toggle(view: &Rc<ViewData>, window: &mut Window) {
        if view.hud.take().is_none() {
            static LIBRARY: OnceLock<Result<Library, String>> = OnceLock::new();
            match LIBRARY.get_or_init(|| load().map_err(|error| format!("{error:#}"))) {
                Ok(library) => {
                    let hud = NonNull::new((library.new)()).map(|raw| Hud {
                        raw,
                        library,
                        started: Cell::new(Instant::now()),
                        timer: RefCell::new(Some(tick(Rc::downgrade(view), window))),
                    });
                    view.hud.replace(hud);
                }
                Err(error) => eprintln!("apps: {error}"),
            }
        }
        window.refresh();
    }

    /// Records the frame and paints the HUD over the page.
    pub(crate) fn paint(&self, window: &mut Window) {
        let size = window.viewport_size();
        unsafe {
            (self.library.paint)(
                self.raw.as_ptr(),
                self.started.get().elapsed().as_nanos() as u64,
                window.redrawing(),
                size.width.into(),
                size.height.into(),
                window.scale_factor(),
                (window as *mut Window).cast(),
                paint,
            );
        }
    }
}

impl Drop for Hud {
    fn drop(&mut self) {
        unsafe { (self.library.free)(self.raw.as_ptr()) }
    }
}

fn load() -> Result<Library> {
    let path = std::env::current_exe()?
        .canonicalize()?
        .with_file_name(libloading::library_filename("apps_perfhud"));
    unsafe {
        let library = libloading::Library::new(&path).with_context(|| {
            format!(
                "Cannot load the FPS HUD from {}. Build the workspace with cargo build --workspace --release.",
                path.display()
            )
        })?;
        let abi: libloading::Symbol<extern "C" fn() -> u32> = library.get(b"apps_perfhud_abi\0")?;
        ensure!(
            abi() == ABI,
            "{} does not match this runtime",
            path.display()
        );
        let functions = Library {
            new: *library.get(b"apps_perfhud_new\0")?,
            free: *library.get(b"apps_perfhud_free\0")?,
            tick: *library.get(b"apps_perfhud_tick\0")?,
            paint: *library.get(b"apps_perfhud_paint\0")?,
        };
        // WARNING: the usage thread of a hidden HUD can still run, so the library is never unloaded.
        std::mem::forget(library);
        Ok(functions)
    }
}

/// Ticks the HUD of `view` every half second while it is shown, and draws a frame when the page is idle.
fn tick(view: Weak<ViewData>, window: &Window) -> Timer {
    window.timer(Duration::from_millis(500), move |window, _| {
        let Some(data) = view.upgrade() else {
            return;
        };
        if let Some(hud) = &*data.hud.borrow() {
            if unsafe { (hud.library.tick)(hud.raw.as_ptr()) } {
                window.refresh();
            }
            hud.timer.replace(Some(tick(view, window)));
        }
    })
}

extern "C" fn paint(window: *mut c_void, item: &Item) {
    let window = unsafe { &mut *window.cast::<Window>() };
    let [x, y, width, height] = item.bounds;
    let bounds = Bounds::new(point(px(x), px(y)), size(px(width), px(height)));
    let color = renderer::rgba(item.color);
    if item.len == 0 {
        window.paint_quad(renderer::fill(bounds, color).corner_radii(px(item.size)));
        return;
    }
    let text = unsafe { std::slice::from_raw_parts(item.text, item.len) };
    let text = String::from_utf8_lossy(text).into_owned();
    let run = TextRun {
        len: text.len(),
        font: renderer::font(FONT),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
        letter_spacing: px(0.),
        word_spacing: px(0.),
        font_size: None,
    };
    let line = window
        .text_system()
        .shape_line(text.into(), px(item.size), &[run], None);
    let align = match item.align {
        1 => TextAlign::Center,
        2 => TextAlign::Right,
        _ => TextAlign::Left,
    };
    line.paint(
        bounds.origin,
        bounds.size.height,
        align,
        Some(bounds.size.width),
        window,
    );
}
