use super::lock;
use super::threads::native_sync;
use crate::{Pixels, Rect, Size, WindowOptions, WindowState, point, px, rect, size};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSScreen, NSWindow, NSWindowStyleMask};
use objc2_core_foundation::CGSize;
use objc2_foundation::NSRect;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Default)]
pub(crate) struct Snapshot {
    pub scale: f32,
    pub active: bool,
    pub state: WindowState,
}

pub(crate) type Shared<T> = Arc<Mutex<T>>;

/// Flips an AppKit screen rectangle to the core's top-left origin on the primary display.
pub(crate) fn flip(area: NSRect, top: f64) -> Rect {
    let y = top - area.origin.y - area.size.height;
    let [x, y, width, height] =
        [area.origin.x, y, area.size.width, area.size.height].map(|v| px(v as f32));
    rect(x, y, width, height)
}

pub(crate) fn cg_size(size: Size<Pixels>) -> CGSize {
    CGSize::new(size.width.0.into(), size.height.0.into())
}

pub(crate) fn primary_top(mtm: MainThreadMarker) -> f64 {
    NSScreen::screens(mtm)
        .firstObject()
        .map_or(0., |screen| screen.frame().size.height)
}

#[derive(Clone, Copy)]
struct Display {
    titlebar: Pixels,
    scale: f32,
    frame: Rect,
    work_area: Rect,
}

static DISPLAY: Mutex<Option<Display>> = Mutex::new(None);

// The work area of the primary screen as of the launch or the last window that opened.
pub(crate) fn work_area() -> Option<Rect> {
    lock(&DISPLAY).map(|display| display.work_area)
}

pub(crate) fn measure_screens() {
    let mtm = MainThreadMarker::new().unwrap();
    let Some(screen) = NSScreen::screens(mtm)
        .firstObject()
        .or_else(|| NSScreen::mainScreen(mtm))
    else {
        return;
    };
    let top = primary_top(mtm);
    let inner = NSRect::new(Default::default(), CGSize::new(100., 100.));
    let outer = NSWindow::frameRectForContentRect_styleMask(
        inner,
        NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
        mtm,
    );
    let scale = screen.backingScaleFactor() as f32;
    *lock(&DISPLAY) = Some(Display {
        titlebar: px((outer.size.height - inner.size.height) as f32),
        scale: if scale == 0. { 2. } else { scale },
        frame: flip(screen.frame(), top),
        work_area: flip(screen.visibleFrame(), top),
    });
}

// The app thread draws before main opened the window, so it starts from this guess.
pub(crate) fn guess(options: &WindowOptions) -> Snapshot {
    let known = *lock(&DISPLAY);
    let Some(display) = known.or_else(|| {
        native_sync(measure_screens);
        *lock(&DISPLAY)
    }) else {
        return Snapshot::default();
    };
    let bar = match options.titlebar_hidden || options.frameless {
        true => px(0.),
        false => display.titlebar,
    };
    let mut frame = options.bounds;
    frame.origin += display.frame.origin;
    if !options.outer_size {
        frame.size.height += bar;
    }
    let content = Rect::new(
        frame.origin + point(px(0.), bar),
        size(frame.size.width, frame.size.height - bar),
    );
    Snapshot {
        scale: display.scale,
        active: false,
        state: WindowState {
            frame,
            content,
            visible: options.show,
            display: display.frame,
            work_area: display.work_area,
            ..WindowState::default()
        },
    }
}
