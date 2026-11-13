use crate::{
    App, Backdrop, BorderStyle, BoxShadow, Child, Clip, Color, Cursor, Edges, Event, Key,
    Modifiers, NodeKey, PaintEffect, Pixels, Point, Radii, Rect, RenderImage, ScrollRef, Size,
    Tree, Utf16Selection, WheelPhase,
    atlas::TileKey,
    geometry::{snap, snap_width},
    platform, point, px,
    scene::{BackdropState, EffectState, Scene},
    size,
    text::{DecorationStyle, FontId, GlyphId, GlyphRaster, SUBPIXEL_STEPS, TextSystem},
};
use anyhow::Result;
use raw_window_handle::HandleError;
use rustc_hash::FxHashMap;
use std::{
    any::{Any, TypeId},
    borrow::Cow,
    cell::Cell,
    mem,
    ops::Range,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

#[path = "paint.rs"]
mod paint;
pub use paint::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub u64);

/// Text input calls use UTF-16 offsets.
pub trait View: 'static {
    fn render(&mut self, tree: &Tree, window: &mut Window, app: &mut App) -> Child;
    fn window_changed(&mut self, window: &mut Window, app: &mut App);
    /// Pointer events go to hit regions, not here.
    fn event(&mut self, event: &Event, window: &mut Window, app: &mut App);

    fn selection(&mut self, window: &mut Window, app: &mut App) -> Option<Utf16Selection>;
    fn composition(&mut self, window: &mut Window, app: &mut App) -> Option<Range<usize>>;
    fn text_in(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        window: &mut Window,
        app: &mut App,
    ) -> Option<String>;
    fn insert(
        &mut self,
        replacing: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        app: &mut App,
    );
    fn compose(
        &mut self,
        replacing: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        app: &mut App,
    );
    fn end_composition(&mut self, window: &mut Window, app: &mut App);
    fn rect_of(&mut self, range: Range<usize>, window: &mut Window, app: &mut App) -> Option<Rect>;
    fn index_at(&mut self, at: Point<Pixels>, window: &mut Window, app: &mut App) -> Option<usize>;
}

#[derive(Clone, Debug)]
pub struct WindowOptions {
    pub bounds: Rect,
    // `bounds` includes the title bar and frame.
    pub outer_size: bool,
    pub title: String,
    pub titlebar_hidden: bool,
    pub buttons_at: Option<Point<Pixels>>,
    pub frameless: bool,
    pub transparent: bool,
    pub show: bool,
    pub resizable: bool,
    pub minimizable: bool,
    pub movable: bool,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            bounds: crate::rect(px(0.), px(0.), px(800.), px(600.)),
            outer_size: true,
            title: String::new(),
            titlebar_hidden: false,
            buttons_at: None,
            frameless: false,
            transparent: false,
            show: true,
            resizable: true,
            minimizable: true,
            movable: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WindowState {
    pub frame: Rect,
    pub content: Rect,
    pub minimized: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    pub visible: bool,
    pub resizing: bool,
    pub display: Rect,
    pub work_area: Rect,
}

pub use crate::platform::WindowControl;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowChange {
    Frame(Rect),
    Center,
    Show(bool),
    Hide,
    Blur,
    Restore,
    SizeLimits(Size<Pixels>, Size<Pixels>),
    /// An `NSWindowLevel`, where 0 is normal.
    AlwaysOnTop(isize),
    Resizable(bool),
    Minimizable(bool),
    Maximizable(bool),
    Closable(bool),
    Movable(bool),
    Background(Color),
    Opacity(f32),
    Shadow(bool),
    IgnoreMouse(bool, bool),
    AllWorkspaces(bool, bool),
    ContentProtection(bool),
    Focusable(bool),
    AspectRatio(f32, Size<Pixels>),
    Parent(Option<raw_window_handle::RawWindowHandle>, bool),
    Vibrancy(Option<isize>, isize),
    Edited(bool),
    CancelComposition,
}

#[derive(Clone, Copy, Debug)]
pub enum Scope {
    Clip(Option<Clip<Pixels>>),
    Offset(Point<Pixels>),
    Opacity(Option<f32>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    Pass,
    /// Regions below it are not hovered, but they still take the wheel.
    Shield,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    key: u64,
    pub bounds: Rect,
    pub hit: Hit,
}

impl Region {
    pub fn hovered(&self, window: &Window) -> bool {
        window.under[..window.hovered].contains(&self.key)
    }
    /// True even behind a shield, so it can scroll.
    pub fn under_pointer(&self, window: &Window) -> bool {
        window.under.contains(&self.key)
    }
}

// Column-major 3x3 matrices of the z = 0 plane of a CSS transform.
type Plane = [f32; 9];

fn plane_of(m: &[f32; 16]) -> Plane {
    [m[0], m[1], m[3], m[4], m[5], m[7], m[12], m[13], m[15]]
}

fn compose(outer: &Plane, inner: &Plane) -> Plane {
    let mut out = [0.; 9];
    for (index, out) in out.iter_mut().enumerate() {
        let (v, row) = (&inner[index - index % 3..], index % 3);
        *out = outer[row] * v[0] + outer[3 + row] * v[1] + outer[6 + row] * v[2];
    }
    out
}

fn invert(m: &Plane) -> Option<Plane> {
    let cross = |a: &[f32], b: &[f32]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let (c0, c1, c2) = (&m[0..3], &m[3..6], &m[6..9]);
    let rows = [cross(c1, c2), cross(c2, c0), cross(c0, c1)];
    let det = c0[0] * rows[0][0] + c0[1] * rows[0][1] + c0[2] * rows[0][2];
    (det.abs() > 1e-9).then(|| std::array::from_fn(|i| rows[i % 3][i / 3] / det))
}

fn project(m: &Plane, at: Point<Pixels>) -> Option<Point<Pixels>> {
    let (x, y) = (at.x.0, at.y.0);
    let [u, v, w] = [0, 1, 2].map(|row| m[row] * x + m[3 + row] * y + m[6 + row]);
    (w.abs() > 1e-6).then(|| point(px(u / w), px(v / w)))
}

// CSS-pixel matrices become device-pixel matrices at this scale.
fn device_matrix(mut matrix: [f32; 16], scale: f32) -> [f32; 16] {
    for i in 0..3 {
        matrix[i * 4 + 3] /= scale;
        matrix[12 + i] *= scale;
    }
    matrix
}

/// Composited animations change `local` without a new frame.
#[derive(Debug, PartialEq)]
struct HitTransform {
    key: Option<u64>,
    outer: Option<Rc<HitTransform>>,
    local: Cell<Plane>,
    to_window: Cell<Plane>,
    from_window: Cell<Plane>,
    // The window clip when the outermost transform began. Points outside it miss.
    area: Rect,
}

impl HitTransform {
    fn chain(self: &Rc<Self>) -> impl Iterator<Item = &Rc<Self>> {
        std::iter::successors(Some(self), |inner| inner.outer.as_ref())
    }

    fn update(&self, key: u64, local: &Plane, seen: &mut Vec<*const Self>) {
        if seen.contains(&(self as *const Self)) {
            return;
        }
        if let Some(outer) = &self.outer {
            outer.update(key, local, seen);
        }
        if self.key == Some(key) {
            self.local.set(*local);
        }
        let to_window = match &self.outer {
            Some(outer) => compose(&outer.to_window.get(), &self.local.get()),
            None => self.local.get(),
        };
        if let Some(from_window) = invert(&to_window) {
            self.to_window.set(to_window);
            self.from_window.set(from_window);
        }
        seen.push(self);
    }
}

struct Placed {
    region: Region,
    clip: Clip<Pixels>,
    transform: Option<Rc<HitTransform>>,
}

impl Placed {
    fn contains(&self, at: Point<Pixels>) -> bool {
        let local = match &self.transform {
            None => Some(at),
            Some(transform) if transform.area.contains(at) => {
                project(&transform.from_window.get(), at)
            }
            Some(_) => None,
        };
        local.is_some_and(|local| {
            self.region
                .bounds
                .intersect(self.clip.bounds)
                .contains(local)
        })
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Placement {
    pub(crate) offset: Point<Pixels>,
    pub(crate) mask: Clip<Pixels>,
    pub(crate) transform: usize,
}

type Listener = Box<dyn FnMut(&Event, &mut Window, &mut App)>;
pub(crate) type FrameCallback = Box<dyn FnOnce(&mut Window, &mut App)>;
pub(crate) type CaptureCallback = Box<dyn FnOnce(Result<([u32; 2], Vec<u8>)>, &mut App)>;

#[derive(Default)]
struct Input {
    regions: Vec<Placed>,
    listeners: Vec<(bool, Listener)>,
    cursors: Vec<(u64, Cursor)>,
    drag_areas: Vec<u64>,
}

impl Input {
    fn clear(&mut self) {
        self.regions.clear();
        self.listeners.clear();
        self.cursors.clear();
        self.drag_areas.clear();
    }

    fn turn(&self, key: u64, matrix: &[f32; 16]) {
        let local = plane_of(matrix);
        let mut seen = Vec::new();
        for transform in self
            .regions
            .iter()
            .filter_map(|placed| placed.transform.as_ref())
        {
            if transform.chain().any(|inner| inner.key == Some(key)) {
                transform.update(key, &local, &mut seen);
            }
        }
    }

    fn probe(&self, at: Point<Pixels>, under: &mut Vec<u64>) -> usize {
        under.clear();
        let mut hovered = None;
        for placed in self
            .regions
            .iter()
            .rev()
            .filter(|placed| placed.contains(at))
        {
            under.push(placed.region.key);
            if placed.region.hit == Hit::Shield {
                hovered.get_or_insert(under.len());
            }
        }
        hovered.unwrap_or(under.len())
    }
}

#[derive(Default)]
pub(crate) struct ScrollGesture {
    pub(crate) latch: Option<ScrollRef>,
    pub(crate) spent: bool,
    pub(crate) speed: Point<f32>,
    pub(crate) edge: Option<(ScrollRef, Point<Pixels>)>,
}

pub const CARET_BLINK: Duration = Duration::from_millis(500);

#[cfg(target_os = "macos")]
struct Caret {
    bounds: Rect<f32>,
    color: Color,
    since: Instant,
    order: u32,
    painted: bool,
    overlay: bool,
}

fn round_corners(mask: &Clip<f32>) -> [Rect<f32>; 4] {
    let (r, x, y) = (mask.rounded, &mask.radii.x, &mask.radii.y);
    let (right, bottom) = (r.origin.x + r.size.width, r.origin.y + r.size.height);
    [
        crate::rect(r.origin.x, r.origin.y, x.top_left, y.top_left),
        crate::rect(right - x.top_right, r.origin.y, x.top_right, y.top_right),
        crate::rect(
            right - x.bottom_right,
            bottom - y.bottom_right,
            x.bottom_right,
            y.bottom_right,
        ),
        crate::rect(
            r.origin.x,
            bottom - y.bottom_left,
            x.bottom_left,
            y.bottom_left,
        ),
    ]
}

struct Track {
    key: u64,
    start: Instant,
    step: f32,
    repeat: f32,
    frames: Vec<[f32; 16]>,
}

impl Track {
    fn current(&self) -> Option<&[f32; 16]> {
        let count = self.frames.len().saturating_sub(1).max(1);
        let end = self.repeat * count as f32;
        let at = (self.start.elapsed().as_secs_f32() / self.step)
            .min(end)
            .round();
        let index = if at >= end {
            (end.round() as usize).saturating_sub(1) % count + 1
        } else {
            at as usize % count
        };
        self.frames.get(index).or(self.frames.last())
    }
}

pub struct Window {
    id: WindowId,
    pub(crate) platform: platform::Window,
    pub(crate) view: Option<Box<dyn View>>,
    text_system: Rc<TextSystem>,
    scale_factor: f32,
    viewport_size: Size<Pixels>,
    pub(crate) zoom: f32,
    native: (Size<Pixels>, f32),
    tree: Option<Tree>,
    scene: Scene,
    // `pending` is being built; `live` is on screen and takes the input.
    pending: Input,
    live: Input,
    under: Vec<u64>,
    hovered: usize,
    next_region: u64,
    content_masks: Vec<(Clip<Pixels>, Clip<f32>)>,
    offsets: Vec<Point<Pixels>>,
    pub(crate) font_smoothing: bool,
    hit_transform: Option<Rc<HitTransform>>,
    opacity: f32,
    // The mask layer and effect depth of `with_text_clip`.
    pub(crate) text_clip: Option<(usize, usize)>,
    states: FxHashMap<(NodeKey, TypeId), Box<dyn Any>>,
    next_states: FxHashMap<(NodeKey, TypeId), Box<dyn Any>>,
    frame_callbacks: Vec<FrameCallback>,
    pub(crate) captures: Vec<CaptureCallback>,
    pub(crate) ime_key: Option<(Event, bool)>,
    #[cfg(target_os = "macos")]
    caret: Option<Caret>,
    #[cfg(target_os = "macos")]
    caret_covered: bool,
    #[cfg(target_os = "macos")]
    symbols: platform::symbols::Cache,
    #[cfg(target_os = "macos")]
    controls: (Vec<crate::NativeControl>, Option<crate::ControlSink>),
    pub(crate) dirty: bool,
    drawing: bool,
    // `again`: the page asked for another pass while one was built. `settling`: this is that extra pass.
    again: bool,
    settling: bool,
    restart: bool,
    restarts: u8,
    rebuild: bool,
    patches: Vec<crate::Node>,
    pub(crate) pointer: Point<Pixels>,
    pub(crate) closed: bool,
    cursor: Option<Cursor>,
    drawn_at: Instant,
    frames_drawn: u64,
    tracks: Vec<Track>,
    pub(crate) rails: ([f32; 2], Instant),
    // The time of a frame that a display tick started, and when its device sent the event in dispatch.
    pub(crate) frame_at: Option<Instant>,
    pub(crate) event_at: Option<Instant>,
    pub(crate) scroll_chain: Option<Vec<ScrollRef>>,
    pub(crate) gesture: ScrollGesture,
}

impl Window {
    pub(crate) fn new(
        id: WindowId,
        platform: platform::Window,
        text_system: Rc<TextSystem>,
    ) -> Self {
        let (scale_factor, viewport_size) = (platform.scale_factor(), platform.content_size());
        Self {
            id,
            platform,
            view: None,
            text_system,
            scale_factor,
            viewport_size,
            zoom: 1.,
            native: (viewport_size, scale_factor),
            tree: Some(Tree::default()),
            scene: Scene::default(),
            pending: Input::default(),
            live: Input::default(),
            under: Vec::new(),
            hovered: 0,
            next_region: 0,
            content_masks: Vec::new(),
            offsets: Vec::new(),
            font_smoothing: true,
            hit_transform: None,
            opacity: 1.,
            text_clip: None,
            states: FxHashMap::default(),
            next_states: FxHashMap::default(),
            frame_callbacks: Vec::new(),
            captures: Vec::new(),
            ime_key: None,
            #[cfg(target_os = "macos")]
            caret: None,
            #[cfg(target_os = "macos")]
            caret_covered: false,
            #[cfg(target_os = "macos")]
            symbols: platform::symbols::Cache::default(),
            #[cfg(target_os = "macos")]
            controls: Default::default(),
            dirty: false,
            drawing: false,
            again: false,
            settling: false,
            restart: false,
            restarts: 0,
            rebuild: false,
            patches: Vec::new(),
            pointer: Point::default(),
            closed: false,
            cursor: None,
            drawn_at: Instant::now(),
            frames_drawn: 0,
            tracks: Vec::new(),
            rails: ([0.; 2], Instant::now()),
            frame_at: None,
            event_at: None,
            scroll_chain: None,
            gesture: ScrollGesture::default(),
        }
    }

    pub fn id(&self) -> WindowId {
        self.id
    }

    pub fn text_system(&self) -> &Rc<TextSystem> {
        &self.text_system
    }

    /// Includes the zoom.
    pub fn scale_factor(&self) -> f32 {
        self.scale_factor
    }

    /// In CSS pixels, with the zoom.
    pub fn viewport_size(&self) -> Size<Pixels> {
        self.viewport_size
    }

    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    pub fn set_zoom(&mut self, zoom: f32) {
        if self.zoom == zoom {
            return;
        }
        self.pointer = self.pointer * (self.zoom / zoom);
        self.zoom = zoom;
        let (size, scale) = self.native;
        self.mark_resized(size, scale);
    }

    pub fn refresh(&mut self) {
        if !self.drawing && !self.dirty {
            self.dirty = true;
            self.platform.request_frame();
        }
    }

    pub fn redraw(&mut self) {
        match self.drawing {
            true => self.again = true,
            false => self.refresh(),
        }
    }

    // The page changed while this pass was built. The pass is built again, a few times per frame at most, and this doesn't use up the extra pass that sizes settle in.
    pub fn restart(&mut self) {
        match self.drawing {
            true => self.restart = true,
            false => self.refresh(),
        }
    }

    pub fn rebuild(&mut self) {
        self.rebuild = true;
    }

    // The node needs its key, or the patch is ignored.
    pub fn patch(&mut self, node: crate::Node) {
        self.patches.push(node);
    }

    pub fn redrawing(&self) -> bool {
        self.settling
    }

    pub fn will_draw(&self) -> bool {
        self.drawing || self.dirty || !self.captures.is_empty()
    }

    pub fn patch_effect(&mut self, key: u64, opacity: f32, transform: Option<[f32; 16]>) -> bool {
        let device = transform.map(|matrix| device_matrix(matrix, self.scale_factor));
        if !self.scene.patch(key, opacity.clamp(0., 1.), device) {
            return false;
        }
        if let Some(matrix) = &transform {
            self.live.turn(key, matrix);
        }
        true
    }

    pub fn demote(&mut self) {
        #[cfg(target_os = "macos")]
        self.platform.renderer.demote();
        self.tracks.clear();
    }

    // When `repeat` is not 1, the last sample must equal the first.
    pub fn promote(
        &mut self,
        key: u64,
        start: Instant,
        step: f32,
        repeat: f32,
        opacities: &[f32],
        transforms: Option<&[[f32; 16]]>,
    ) -> bool {
        if self.will_draw() || self.drawn_at.elapsed() < Duration::from_millis(250) {
            return false;
        }
        let Some(index) = self.scene.keyed(key) else {
            return false;
        };
        let layer = &self.scene.layers[index];
        let (Some(effect), Some(content)) = (layer.effect.as_ref(), layer.bounds) else {
            return false;
        };
        if effect.color_matrix.is_some()
            || effect.blur > 0.
            || effect.shadow.is_some()
            || effect.blend_mode != crate::BlendMode::Normal
            || effect.clip.is_some()
            || effect.mask.is_some()
            || effect.backdrop.is_some()
            || effect.glass.is_some()
            || transforms.is_some() != effect.transform.is_some()
        {
            return false;
        }
        let scale = self.scale_factor;
        let reach = match transforms {
            Some(transforms) => transforms
                .iter()
                .map(|matrix| {
                    crate::scene::transform_bounds(&device_matrix(*matrix, scale), content)
                })
                .reduce(|all, bounds| all.union(bounds)),
            None => Some(content),
        };
        let region = crate::frame::region(content);
        // Promoted layers bypass window clips. Keep them inside the clip with no overlapping content above.
        let window = Rect::new(Point::default(), self.viewport_size.scale(scale));
        let reach = reach.map(|reach| reach.intersect(window));
        if reach.is_some_and(|reach| reach.is_empty()) {
            return true;
        }
        let mask = &effect.content_mask;
        if reach.is_none_or(|reach| {
            mask.bounds.intersect(reach) != reach
                || round_corners(mask)
                    .iter()
                    .any(|corner| corner.intersects(reach))
                || self.scene.paints_over(index, reach, |_| false)
        }) || region[2] > 2048
            || region[3] > 2048
        {
            return false;
        }
        #[cfg(target_os = "macos")]
        {
            // Core Animation works in window points, so the zoom moves into the matrices.
            let zoom = f64::from(self.zoom);
            let points: Vec<[f64; 16]> = transforms
                .unwrap_or_default()
                .iter()
                .map(|matrix| {
                    let mut matrix = matrix.map(f64::from);
                    for i in 0..3 {
                        matrix[i * 4 + 3] /= zoom;
                        matrix[12 + i] *= zoom;
                    }
                    matrix
                })
                .collect();
            let begin = start.elapsed().as_secs_f64();
            self.platform.renderer.promote(
                key,
                index,
                region,
                [f64::from(step), f64::from(repeat), begin],
                opacities,
                &points,
            );
            self.tracks.retain(|track| track.key != key);
            if let Some(transforms) = transforms {
                self.tracks.push(Track {
                    key,
                    start,
                    step,
                    repeat,
                    frames: transforms.to_vec(),
                });
            }
            true
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (start, step, repeat, opacities);
            false
        }
    }

    pub fn present(&mut self) -> bool {
        if self.will_draw() {
            return false;
        }
        // The atlas and text caches keep their frame, so the tiles this scene reads stay.
        let _ = self.platform.draw(&self.scene, false);
        true
    }

    pub fn next_frame(&mut self, job: impl FnOnce(&mut Window, &mut App) + 'static) {
        self.frame_callbacks.push(Box::new(job));
        self.platform.request_frame();
    }

    pub fn frames_drawn(&self) -> u64 {
        self.frames_drawn
    }

    pub fn request_animation_frame(&mut self) {
        self.next_frame(|window, _| window.refresh());
    }

    pub fn capture(&mut self, done: impl FnOnce(Result<([u32; 2], Vec<u8>)>, &mut App) + 'static) {
        self.captures.push(Box::new(done));
        self.dirty = true;
        self.platform.force_frame();
    }

    pub fn timer(
        &self,
        delay: Duration,
        f: impl FnOnce(&mut Window, &mut App) + 'static,
    ) -> crate::Timer {
        let id = self.id;
        crate::app::timer(delay, move |app| {
            app.update_window(id, f);
        })
    }

    pub fn close(&mut self) {
        self.closed = true;
    }

    pub fn is_active(&self) -> bool {
        self.platform.is_active()
    }

    pub fn window_state(&self) -> WindowState {
        self.platform.state()
    }

    pub fn change_window(&mut self, change: WindowChange) {
        self.platform.change(change);
    }

    pub fn control(&self) -> &WindowControl {
        self.platform.control()
    }

    pub(crate) fn mark_resized(&mut self, size: Size<Pixels>, scale_factor: f32) {
        self.native = (size, scale_factor);
        self.viewport_size = size / self.zoom;
        self.scale_factor = scale_factor * self.zoom;
        self.dirty = false;
        self.refresh();
    }

    pub(crate) fn take_frame_callbacks(&mut self) -> Vec<FrameCallback> {
        mem::take(&mut self.frame_callbacks)
    }

    pub(crate) fn wants_frame(&self) -> bool {
        self.dirty || !self.frame_callbacks.is_empty()
    }

    // Core Animation avoids about 126 MB of Metal memory for caret blinking. Use it only without effects,
    // rounded clips or overlaps.
    pub fn overlay_caret(
        &mut self,
        bounds: Rect,
        color: Color,
        since: Instant,
        painted: bool,
    ) -> bool {
        #[cfg(target_os = "macos")]
        {
            let mask = self.snapped_content_mask();
            let visible = self.snap_bounds(bounds).intersect(mask.bounds);
            if !self.scene.at_root()
                || self.opacity < 1.
                || !self.captures.is_empty()
                || round_corners(&mask)
                    .iter()
                    .any(|corner| corner.intersects(visible))
            {
                return false;
            }
            if visible.is_empty() {
                return true;
            }
            let overlay = !self.caret_covered;
            self.caret = Some(Caret {
                bounds: visible,
                color,
                since,
                order: self.scene.probe(),
                painted: painted && !overlay,
                overlay,
            });
            overlay
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (bounds, color, since, painted);
            false
        }
    }

    pub fn native_control(&mut self, control: crate::NativeControl, sink: &crate::ControlSink) {
        #[cfg(target_os = "macos")]
        {
            // Native views are placed in window points.
            let mut control = control;
            control.clip = self.content_mask().bounds * self.zoom;
            control.bounds = control.bounds * self.zoom;
            self.controls.0.push(control);
            self.controls.1.get_or_insert_with(|| sink.clone());
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (control, sink);
    }

    pub(crate) fn draw(&mut self, app: &mut App) {
        self.demote();
        self.drawn_at = Instant::now();
        self.frames_drawn += 1;
        loop {
            self.dirty = false;
            self.drawing = true;
            self.pending.clear();
            self.scene.clear();
            #[cfg(target_os = "macos")]
            {
                self.caret = None;
            }
            // Keyed state survives the passes of one frame. Between frames, state that was not used goes.
            if self.settling || self.restarts > 0 {
                let states = mem::take(&mut self.next_states);
                self.states.extend(states);
            } else {
                self.next_states.clear();
            }
            if let Some(mut view) = self.view.take() {
                let mut tree = self.tree.take().unwrap_or_default();
                let root = self.build(&mut tree, app, |tree, window, app| {
                    view.render(tree, window, app)
                });
                self.view = Some(view);
                if let Some(root) = root {
                    tree.layout(root, self.viewport_size, self);
                    tree.prepaint(root, self);
                    self.hovered = self.pending.probe(self.pointer, &mut self.under);
                    let restarting = self.restart && self.restarts < 4;
                    let again = self.again || self.restart;
                    if !restarting && (!again || self.settling) {
                        tree.paint(root, self);
                    }
                }
                self.tree = Some(tree);
            }
            self.drawing = false;
            let restart = mem::take(&mut self.restart);
            if restart && self.restarts < 4 {
                self.restarts += 1;
                self.again = false;
                continue;
            }
            let again = mem::take(&mut self.again) || restart;
            if again && !self.settling {
                self.settling = true;
                continue;
            }
            self.settling = false;
            self.restarts = 0;
            if again {
                self.refresh();
            }
            break;
        }
        mem::swap(&mut self.live, &mut self.pending);
        mem::swap(&mut self.states, &mut self.next_states);
        #[cfg(target_os = "macos")]
        {
            let caret = self.caret.take();
            let covered = caret.as_ref().is_some_and(|caret| {
                self.scene
                    .covers(caret.order + u32::from(caret.painted), caret.bounds)
            });
            if covered && caret.as_ref().is_some_and(|caret| caret.overlay) {
                self.refresh();
            }
            self.caret_covered = covered;
            self.platform.renderer.set_caret(
                caret
                    .filter(|caret| caret.overlay && !covered)
                    .map(|caret| (caret.bounds, caret.color, caret.since)),
            );
            let controls = mem::take(&mut self.controls.0);
            self.platform
                .set_controls(controls, self.controls.1.clone());
        }
        let captures = mem::take(&mut self.captures);
        #[cfg(target_os = "macos")]
        self.platform
            .set_drawable_size(self.native.0, self.native.1);
        let mut frame = self.platform.draw(&self.scene, !captures.is_empty());
        if let Some(tree) = &mut self.tree {
            tree.bury();
        }
        self.text_system.end_frame();
        self.platform.atlas().finish_frame();
        let last = captures.len().wrapping_sub(1);
        for (index, capture) in captures.into_iter().enumerate() {
            let result = match &mut frame {
                Some(Ok(frame)) if index == last => Ok(mem::take(frame)),
                Some(Ok(frame)) => Ok(frame.clone()),
                Some(Err(error)) => Err(anyhow::anyhow!("{error:#}")),
                None => Err(anyhow::anyhow!("The frame was not drawn")),
            };
            capture(result, app);
        }
        self.update_cursor(app);
    }

    fn build(
        &mut self,
        tree: &mut Tree,
        app: &mut App,
        mut render: impl FnMut(&Tree, &mut Self, &mut App) -> Child,
    ) -> Option<taffy::NodeId> {
        loop {
            tree.begin(self.scale_factor, self.text_system.generation());
            let child = render(tree, self, app);
            let root = tree.commit_root(child, self.viewport_size, self.scale_factor);
            for node in mem::take(&mut self.patches) {
                tree.patch(node, self.scale_factor);
            }
            if !mem::take(&mut self.rebuild) {
                return root;
            }
        }
    }

    pub fn layout(
        &mut self,
        app: &mut App,
        mut render: impl FnMut(&Tree, &mut Self, &mut App) -> Child,
    ) -> bool {
        if self.drawing {
            return false;
        }
        self.drawing = true;
        let mut again = false;
        for _ in 0..2 {
            self.pending.clear();
            let mut tree = self.tree.take().unwrap_or_default();
            if let Some(root) = self.build(&mut tree, app, &mut render) {
                tree.layout(root, self.viewport_size, self);
                tree.prepaint(root, self);
            }
            self.tree = Some(tree);
            let states = mem::take(&mut self.next_states);
            self.states.extend(states);
            again = mem::take(&mut self.again);
            if !again {
                break;
            }
        }
        self.drawing = false;
        if again {
            self.refresh();
        }
        true
    }

    fn update_cursor(&mut self, app: &mut App) {
        let hovered = &self.under[..self.hovered];
        let cursor = self
            .live
            .cursors
            .iter()
            .rev()
            .find_map(|(key, cursor)| hovered.contains(key).then_some(*cursor))
            .unwrap_or_default();
        if self.cursor != Some(cursor) && self.platform.is_active() {
            self.cursor = Some(cursor);
            app.platform.set_cursor(cursor);
        }
    }

    #[cfg(windows)]
    pub(crate) fn in_drag_area(&self, at: Point<Pixels>) -> bool {
        (self.live.regions.iter().rev())
            .find(|placed| placed.contains(at))
            .is_some_and(|top| self.live.drag_areas.contains(&top.region.key))
    }

    fn track_wheel(&mut self, delta: Point<f32>, lines: bool, phase: WheelPhase) {
        self.scroll_chain = None;
        self.gesture.edge = None;
        if lines {
            if let Some(handle) = self.gesture.latch.take() {
                handle.release(Point::default(), self);
            }
            return;
        }
        let now = Instant::now();
        let elapsed = now - self.rails.1;
        let paused = elapsed > Duration::from_millis(100);
        let kept = if paused { 0. } else { 0.8 };
        let [x, y] = self.rails.0;
        self.rails = ([x * kept + delta.x.abs(), y * kept + delta.y.abs()], now);
        if paused {
            self.gesture.speed = Point::default();
        } else if delta != Point::default() {
            let seconds = elapsed.as_secs_f32().max(1. / 120.);
            let speed = self.gesture.speed;
            self.gesture.speed = point(
                (speed.x + delta.x / seconds) / 2.,
                (speed.y + delta.y / seconds) / 2.,
            );
        }
        if phase == WheelPhase::Began || phase == WheelPhase::None && paused {
            if let Some(handle) = self.gesture.latch.take() {
                handle.release(Point::default(), self);
            }
            self.gesture.spent = false;
        }
        if matches!(phase, WheelPhase::Ended | WheelPhase::MomentumEnded)
            && let Some(handle) = self.gesture.latch.clone()
            && handle.release(self.gesture.speed, self)
        {
            self.gesture.spent = true;
        }
        if phase == WheelPhase::MomentumEnded {
            self.gesture.latch = None;
        }
    }

    // A trackpad scroll with the time its device sent it moves evenly between events.
    pub fn dispatch_event_at(&mut self, event: Event, sent: Instant, app: &mut App) -> bool {
        self.event_at = Some(sent);
        let stopped = self.dispatch_event(event, app);
        self.event_at = None;
        stopped
    }

    pub fn dispatch_event(&mut self, event: Event, app: &mut App) -> bool {
        if let Some(at) = event.at() {
            self.pointer = at;
        }
        if let Event::Wheel {
            delta,
            lines,
            phase,
            ..
        } = &event
        {
            self.track_wheel(*delta, *lines, *phase);
        }
        app.event_stopped = false;
        if !event.targets_regions() {
            if let Some(mut view) = self.view.take() {
                view.event(&event, self, app);
                self.view.get_or_insert(view);
            }
            return app.event_stopped;
        }
        for track in &self.tracks {
            if let Some(matrix) = track.current() {
                self.live.turn(track.key, matrix);
            }
        }
        self.hovered = self.live.probe(self.pointer, &mut self.under);
        self.update_cursor(app);
        // Capture listeners run in paint order, then bubble listeners in reverse, until one takes the event.
        let mut listeners = mem::take(&mut self.live.listeners);
        let count = listeners.len();
        let order = (0..count).map(|i| (i, true));
        for (i, capture) in order.chain((0..count).rev().map(|i| (i, false))) {
            let (phase, listener) = &mut listeners[i];
            if *phase == capture {
                listener(&event, self, app);
                if app.event_stopped {
                    break;
                }
            }
        }
        // A frame drawn by a listener has its own listeners.
        if self.live.listeners.is_empty() {
            self.live.listeners = listeners;
        }
        if let Event::Wheel { phase, .. } = event
            && let Some((handle, rest)) = self.gesture.edge.take()
        {
            self.overscroll(handle, rest, phase);
        }
        app.event_stopped
    }

    pub(crate) fn overscroll(&mut self, handle: ScrollRef, rest: Point<Pixels>, phase: WheelPhase) {
        handle.stretch_by(rest);
        self.gesture.edge = None;
        self.gesture.latch = Some(handle.clone());
        match phase {
            WheelPhase::Momentum => {
                self.gesture.spent = true;
                handle.release(self.gesture.speed, self);
            }
            WheelPhase::None => handle.loosen(self),
            _ => {}
        }
        self.refresh();
    }

    pub fn scroll_only(&mut self, chain: Vec<ScrollRef>) {
        self.scroll_chain = Some(chain);
    }

    // These keys pass main's key filter like real ones, so this blocks until main answers.
    pub fn press_key(&mut self, key: Key, modifiers: Modifiers, app: &mut App) -> bool {
        let id = self.id;
        let blocked =
            |event: Event| crate::native::sync(move || crate::native::key(id, &event, &|| false));
        let text = key.text.clone();
        let down = Event::KeyDown {
            key: key.clone(),
            modifiers,
            repeat: false,
            ime: false,
        };
        let handled = blocked(down.clone()) || self.dispatch_event(down, app);
        if !handled && let Some(text) = &text {
            self.with_view(app, |view, window, app| {
                view.insert(None, text, window, app)
            });
        }
        let up = Event::KeyUp {
            key,
            modifiers,
            ime: false,
        };
        if !blocked(up.clone()) {
            self.dispatch_event(up, app);
        }
        handled
    }

    pub(crate) fn with_view<R>(
        &mut self,
        app: &mut App,
        f: impl FnOnce(&mut dyn View, &mut Window, &mut App) -> R,
    ) -> Option<R> {
        let mut view = self.view.take()?;
        let result = f(view.as_mut(), self, app);
        self.view.get_or_insert(view);
        Some(result)
    }

    fn place(&mut self, region: Region) -> Region {
        self.pending.regions.push(Placed {
            region,
            clip: self.content_mask(),
            transform: self.hit_transform.clone(),
        });
        region
    }

    pub fn add_region(&mut self, bounds: Rect, hit: Hit) -> Region {
        self.next_region += 1;
        let key = self.next_region;
        self.place(Region { key, bounds, hit })
    }

    pub fn move_region(&mut self, region: &Region, bounds: Rect) -> Region {
        self.place(Region { bounds, ..*region })
    }

    pub fn set_cursor(&mut self, region: &Region, cursor: Cursor) {
        self.pending.cursors.push((region.key, cursor));
    }

    pub fn add_drag_area(&mut self, region: &Region) {
        self.pending.drag_areas.push(region.key);
    }

    pub fn on_capture(&mut self, listener: impl FnMut(&Event, &mut Window, &mut App) + 'static) {
        self.pending.listeners.push((true, Box::new(listener)));
    }

    pub fn on_bubble(&mut self, listener: impl FnMut(&Event, &mut Window, &mut App) + 'static) {
        self.pending.listeners.push((false, Box::new(listener)));
    }

    // Kept subtrees reuse their regions only while this is unchanged.
    pub(crate) fn placement(&self) -> Placement {
        let transform = self
            .hit_transform
            .as_ref()
            .map_or(0, |transform| Rc::as_ptr(transform) as usize);
        Placement {
            offset: self.offset(),
            mask: self.content_mask(),
            transform,
        }
    }

    #[cfg(debug_assertions)]
    pub(crate) fn region_places(&self) -> Vec<(Rect, Clip<Pixels>, Hit, bool)> {
        let place = |placed: &Placed| {
            let region = placed.region;
            (
                region.bounds,
                placed.clip,
                region.hit,
                placed.transform.is_some(),
            )
        };
        self.pending.regions.iter().map(place).collect()
    }

    #[cfg(debug_assertions)]
    pub(crate) fn redraws(&self) -> bool {
        self.again
    }

    #[cfg(debug_assertions)]
    pub(crate) fn truncate_regions(&mut self, len: usize) {
        self.pending.regions.truncate(len);
    }

    pub fn keyed<S: Default + 'static>(&mut self, key: NodeKey) -> &mut S {
        let slot = (key, TypeId::of::<S>());
        let (states, next) = (&mut self.states, &mut self.next_states);
        next.entry(slot)
            .or_insert_with(|| {
                states
                    .remove(&slot)
                    .unwrap_or_else(|| Box::new(S::default()))
            })
            .downcast_mut()
            .expect("the slot type matches its key")
    }

    pub fn offset(&self) -> Point<Pixels> {
        self.offsets.last().copied().unwrap_or_default()
    }

    pub fn content_mask(&self) -> Clip<Pixels> {
        self.content_masks.last().map_or_else(
            || Clip::from(Rect::new(Point::default(), self.viewport_size)),
            |(mask, _)| *mask,
        )
    }

    // Pair with pop_content_mask.
    pub(crate) fn push_content_mask(&mut self, mask: Clip<Pixels>) {
        let snapped = self.snap_mask(mask);
        self.content_masks.push((mask, snapped));
    }

    pub(crate) fn pop_content_mask(&mut self) {
        self.content_masks.pop();
    }

    pub fn scope<R>(&mut self, scope: Scope, f: impl FnOnce(&mut Self) -> R) -> R {
        let outer = self.opacity;
        match scope {
            Scope::Clip(Some(clip)) => self.push_content_mask(clip.intersect(&self.content_mask())),
            Scope::Offset(by) if by != Point::default() => self.offsets.push(self.offset() + by),
            Scope::Opacity(Some(opacity)) => self.opacity *= opacity,
            _ => return f(self),
        }
        let result = f(self);
        match scope {
            Scope::Clip(_) => drop(self.content_masks.pop()),
            Scope::Offset(_) => drop(self.offsets.pop()),
            Scope::Opacity(_) => self.opacity = outer,
        }
        result
    }

    pub fn transformed_bounds(&self, bounds: Rect) -> Rect {
        let Some(transform) = &self.hit_transform else {
            return bounds;
        };
        let to_window = transform.to_window.get();
        let corners = [
            bounds.origin,
            bounds.top_right(),
            bounds.bottom_left(),
            bounds.bottom_right(),
        ];
        let mut mapped = corners.iter().map(|corner| project(&to_window, *corner));
        let Some(Some(first)) = mapped.next() else {
            return bounds;
        };
        mapped
            .try_fold((first, first), |(min, max), corner| {
                corner.map(|corner| (min.min(corner), max.max(corner)))
            })
            .map_or(bounds, |(min, max)| Rect::between(min, max))
    }

    pub fn with_keyed_hit_transform<R>(
        &mut self,
        m: [f32; 16],
        key: Option<u64>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let local = plane_of(&m);
        let outer = self.hit_transform.clone();
        let to_window = match &outer {
            Some(outer) => compose(&outer.to_window.get(), &local),
            None => local,
        };
        let Some(from_window) = invert(&to_window) else {
            return f(self);
        };
        let area = match &outer {
            Some(outer) => outer.area,
            None => self.content_mask().bounds,
        };
        self.hit_transform = Some(Rc::new(HitTransform {
            key,
            outer,
            local: Cell::new(local),
            to_window: Cell::new(to_window),
            from_window: Cell::new(from_window),
            area,
        }));
        self.push_content_mask(unbounded_mask());
        let result = f(self);
        self.content_masks.pop();
        self.hit_transform = self
            .hit_transform
            .take()
            .and_then(|inner| inner.outer.clone());
        result
    }
}

impl raw_window_handle::HasWindowHandle for Window {
    fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, HandleError> {
        let raw = self
            .platform
            .raw_window_handle()
            .map_err(|_| HandleError::Unavailable)?;
        // SAFETY: the native window lives as long as this window.
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(raw) })
    }
}

fn unbounded_mask() -> Clip<Pixels> {
    Clip::from(crate::rect(px(-1e6), px(-1e6), px(2e6), px(2e6)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed(bounds: Rect, hit: Hit, clip: Rect, transform: Option<Rc<HitTransform>>) -> Placed {
        Placed {
            region: Region {
                key: 0,
                bounds,
                hit,
            },
            clip: Clip::from(clip),
            transform,
        }
    }

    fn transform(m: [f32; 16], outer: Option<Rc<HitTransform>>, area: Rect) -> Rc<HitTransform> {
        let local = plane_of(&m);
        let to_window = outer
            .as_ref()
            .map_or(local, |outer| compose(&outer.to_window.get(), &local));
        Rc::new(HitTransform {
            key: Some(7),
            outer,
            local: Cell::new(local),
            to_window: Cell::new(to_window),
            from_window: Cell::new(invert(&to_window).unwrap()),
            area,
        })
    }

    fn translate(x: f32, y: f32) -> [f32; 16] {
        [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., x, y, 0., 1.]
    }

    fn at(x: f32, y: f32) -> Point<Pixels> {
        point(px(x), px(y))
    }

    fn r(x: f32, y: f32, width: f32, height: f32) -> Rect {
        crate::rect(px(x), px(y), px(width), px(height))
    }

    #[test]
    fn regions_hit_inside_their_clip_and_shields_end_the_hover() {
        let window = r(0., 0., 500., 500.);
        let mut input = Input::default();
        for (key, (bounds, hit, clip)) in [
            (r(0., 0., 100., 100.), Hit::Pass, window),
            (r(0., 0., 100., 100.), Hit::Shield, window),
            (r(0., 0., 100., 100.), Hit::Pass, r(0., 0., 50., 50.)),
        ]
        .into_iter()
        .enumerate()
        {
            let mut placed = placed(bounds, hit, clip, None);
            placed.region.key = key as u64;
            input.regions.push(placed);
        }
        let mut under = Vec::new();
        assert_eq!(input.probe(at(10., 10.), &mut under), 2);
        assert_eq!(under, [2, 1, 0]);
        assert_eq!(input.probe(at(70., 70.), &mut under), 1);
        assert_eq!(under, [1, 0]);
        assert_eq!(input.probe(at(170., 70.), &mut under), 0);
        assert!(under.is_empty());
    }

    #[test]
    fn transformed_regions_hit_where_they_are_drawn_and_follow_patches() {
        let window = r(0., 0., 300., 300.);
        let bounds = r(0., 0., 20., 20.);
        let outer = transform(translate(100., 0.), None, window);
        let mut scale = translate(0., 50.);
        scale[0] = 2.;
        scale[5] = 2.;
        let inner = transform(scale, Some(outer), window);
        let mut input = Input::default();
        input
            .regions
            .push(placed(bounds, Hit::Pass, window, Some(inner)));
        let mut under = Vec::new();
        // Drawn at x 100..140 and y 50..90.
        assert_eq!(input.probe(at(130., 85.), &mut under), 1);
        assert_eq!(input.probe(at(30., 85.), &mut under), 0);
        input.turn(7, &translate(0., 0.));
        // Both planes have key 7, so the scale is gone and the box is at x 0..20.
        assert_eq!(input.probe(at(10., 10.), &mut under), 1);
        assert_eq!(input.probe(at(130., 85.), &mut under), 0);
        // Points outside the area of the outermost transform miss.
        assert_eq!(input.probe(at(10., 310.), &mut under), 0);
    }

    #[test]
    fn planes_invert_and_compose() {
        let mut m = translate(3., -4.);
        m[0] = 2.;
        m[1] = 1.;
        m[3] = 0.001;
        let plane = plane_of(&m);
        let inverse = invert(&plane).unwrap();
        let identity = compose(&plane, &inverse);
        for (i, value) in identity.iter().enumerate() {
            let expected = if i % 4 == 0 { 1. } else { 0. };
            assert!((value - expected).abs() < 1e-5, "{identity:?}");
        }
        let point = project(&plane, at(5., 6.)).unwrap();
        let back = project(&inverse, point).unwrap();
        assert!((back.x.0 - 5.).abs() < 1e-4 && (back.y.0 - 6.).abs() < 1e-4);
        assert!(invert(&[0.; 9]).is_none());
    }
}
