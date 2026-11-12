use super::PageState;
use anyhow::Result;
use renderer::{Rect, TextAlign, TextRun, Timer, Window, point, px, size};
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    ptr::NonNull,
    rc::{Rc, Weak},
    sync::OnceLock,
    time::{Duration, Instant},
};

const FONT: &str = if cfg!(target_os = "macos") {
    "Menlo"
} else if cfg!(windows) {
    "Consolas"
} else {
    "DejaVu Sans Mono"
};

#[repr(C)]
struct Item {
    bounds: [f32; 4],
    color: u32,
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
    pub(crate) started: Cell<Instant>,
    timer: RefCell<Timer>,
}

impl Hud {
    pub(crate) fn toggle(view: &Rc<PageState>, window: &mut Window) {
        if view.hud.take().is_none() {
            static LIBRARY: OnceLock<Result<Library, String>> = OnceLock::new();
            match LIBRARY.get_or_init(|| load().map_err(|error| format!("{error:#}"))) {
                Ok(library) => {
                    let hud = NonNull::new((library.new)()).map(|raw| Hud {
                        raw,
                        library,
                        started: Cell::new(Instant::now()),
                        timer: RefCell::new(tick(Rc::downgrade(view), window)),
                    });
                    view.hud.replace(hud);
                }
                Err(error) => eprintln!("apps: {error}"),
            }
        }
        window.refresh();
    }

    pub(super) fn paint(&self, window: &mut Window) {
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
    let library = crate::desktop::devtools::load()?;
    unsafe {
        Ok(Library {
            new: *library.get(b"apps_devtools_hud_new\0")?,
            free: *library.get(b"apps_devtools_hud_free\0")?,
            tick: *library.get(b"apps_devtools_hud_tick\0")?,
            paint: *library.get(b"apps_devtools_hud_paint\0")?,
        })
    }
}

fn tick(view: Weak<PageState>, window: &Window) -> Timer {
    window.timer(Duration::from_millis(500), move |window, _| {
        let Some(data) = view.upgrade() else {
            return;
        };
        if let Some(hud) = &*data.hud.borrow() {
            if unsafe { (hud.library.tick)(hud.raw.as_ptr()) } {
                window.refresh();
            }
            hud.timer.replace(tick(view, window));
        }
    })
}

extern "C" fn paint(window: *mut c_void, item: &Item) {
    let window = unsafe { &mut *window.cast::<Window>() };
    let [x, y, width, height] = item.bounds;
    let bounds = Rect::new(point(px(x), px(y)), size(px(width), px(height)));
    let color = renderer::Color::hex(item.color);
    if item.len == 0 {
        window.draw_box(bounds, px(item.size), color, Default::default());
        return;
    }
    let text = unsafe { std::slice::from_raw_parts(item.text, item.len) };
    let text = String::from_utf8_lossy(text);
    let run = TextRun {
        len: text.len(),
        font: renderer::Font::new(FONT),
        color,
        ..Default::default()
    };
    let line = window.text_system().shape_line(&text, px(item.size), &run);
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
