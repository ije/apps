use crate::{
    App, Backdrop, BorderStyle, BoxShadow, Child, Clip, Color, Cursor, Edges, Event, Key,
    Modifiers, NodeKey, PaintEffect, Pixels, Point, Radii, Rect, RenderImage, ScrollRef, Size,
    Snapshot, Tree, Utf16Selection, WheelPhase, WindowChange, WindowControl, WindowId, WindowState,
    atlas::TileKey,
    geometry::{Plane, compose, invert, plane_of, project, snap, snap_width},
    platform, point, px,
    scene::{self, BackdropState, EffectState, Scene},
    size,
    text::{DecorationStyle, FontId, GlyphId, GlyphRaster, SUBPIXEL_STEPS, TextSystem},
};
use anyhow::Result;
use raw_window_handle::HandleError;
use rustc_hash::FxHashMap;
use std::{
    any::{Any, TypeId},
    borrow::Cow,
    cell::{Cell, RefCell},
    mem,
    ops::Range,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

#[path = "paint.rs"]
mod paint;
pub use paint::*;

/// Text input calls use UTF-16 offsets.
pub trait View: 'static {
    fn render(&mut self, tree: &Tree, window: &mut Window, app: &mut App) -> Child;
    fn window_changed(&mut self, window: &mut Window, app: &mut App);
    /// Pointer events go to hit regions, not here.
    fn event(&mut self, event: &Event, window: &mut Window, app: &mut App);
    /// Runs after each frame, with the layers of `Window::with_snapshot` that changed.
    fn drawn(&mut self, _snapshots: Vec<Snapshot>, _window: &mut Window, _app: &mut App) {}

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
    // The window clip when the outermost transform began, and the scroll viewports around it, which a scroll
    // doesn't move. Points outside them miss.
    area: Rect,
    view: Rect,
    // Equal for transforms that place boxes alike, so a box under a new one that maps the same stays kept.
    id: u64,
}

impl HitTransform {
    // The same transform for content that a scroll moved by `by`. `moved` holds the ones made so far.
    fn shifted(
        self: &Rc<Self>,
        by: Point<Pixels>,
        moved: &mut Vec<(*const Self, Rc<Self>)>,
    ) -> Rc<Self> {
        if let Some((_, done)) = moved.iter().find(|(old, _)| *old == Rc::as_ptr(self)) {
            return done.clone();
        }
        let [x, y] = [by.x.0, by.y.0];
        let turn = |plane: Plane| {
            let there = compose(&[1., 0., 0., 0., 1., 0., x, y, 1.], &plane);
            compose(&there, &[1., 0., 0., 0., 1., 0., -x, -y, 1.])
        };
        let to_window = turn(self.to_window.get());
        let area = match self.area.size.width.0 >= OPEN {
            true => self.area,
            false => self.area + by,
        };
        let shifted = Rc::new(HitTransform {
            key: self.key,
            outer: (self.outer.as_ref()).map(|outer| outer.shifted(by, moved)),
            local: Cell::new(turn(self.local.get())),
            to_window: Cell::new(to_window),
            from_window: Cell::new(turn(self.from_window.get())),
            area,
            view: self.view,
            id: HitTransform::id(self.key, &to_window, area, self.view),
        });
        moved.push((Rc::as_ptr(self), shifted.clone()));
        shifted
    }

    fn id(key: Option<u64>, to_window: &Plane, area: Rect, view: Rect) -> u64 {
        use std::hash::{Hash, Hasher};
        static KEYED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        if key.is_some() {
            // Composited animations change keyed transforms in place.
            return KEYED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) | 1 << 63;
        }
        let mut hasher = rustc_hash::FxHasher::default();
        to_window.map(f32::to_bits).hash(&mut hasher);
        for rect in [area, view] {
            [
                rect.origin.x,
                rect.origin.y,
                rect.size.width,
                rect.size.height,
            ]
            .map(|value| value.0.to_bits())
            .hash(&mut hasher);
        }
        hasher.finish() | 1
    }

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

#[derive(Clone)]
struct Placed {
    region: Region,
    // The content mask, which moves with a scroll, and its part in the scroll viewports around it.
    clip: Clip<Pixels>,
    view: Rect,
    transform: Option<Rc<HitTransform>>,
    // The rounded corners of the region's own box.
    corners: Radii,
}

impl Placed {
    fn contains(&self, at: Point<Pixels>) -> bool {
        let local = match &self.transform {
            None => Some(at),
            Some(transform) if transform.area.intersect(transform.view).contains(at) => {
                project(&transform.from_window.get(), at)
            }
            Some(_) => None,
        };
        local.is_some_and(|local| {
            self.region
                .bounds
                .intersect(self.clip.bounds)
                .intersect(self.view)
                .contains(local)
                && (self.clip.radii.is_zero() || self.clip.radii.contains(self.clip.rounded, local))
                && (self.corners.is_zero() || self.corners.contains(self.region.bounds, local))
        })
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Placement {
    pub(crate) offset: Point<Pixels>,
    pub(crate) mask: Clip<Pixels>,
    // What the window shows of the mask, which hit regions take.
    pub(crate) visible: Rect<Pixels>,
    // The count of scroll viewports around it.
    pub(crate) depth: u32,
    pub(crate) transform: u64,
}

type Listener = Rc<RefCell<dyn FnMut(&Event, &mut Window, &mut App)>>;
pub(crate) type FrameCallback = Box<dyn FnOnce(&mut Window, &mut App)>;
pub(crate) type CaptureCallback = Box<dyn FnOnce(Result<([u32; 2], Vec<u8>)>, &mut App)>;

#[derive(Default)]
struct Input {
    regions: Vec<Placed>,
    // Shared, so an event can run the listeners of its frame without a copy while listeners draw new frames.
    listeners: Rc<Vec<(bool, Listener)>>,
    cursors: Vec<(u64, Cursor)>,
    drag_areas: Vec<u64>,
    // The tokens of `Window::mark` in paint order.
    marks: Vec<u64>,
    // The paint pass that made the paint records, and the prepaint pass that made the regions.
    pass: Option<u32>,
    prepass: Option<u32>,
}

// Where a paint span starts: in the scene and in the paint records of the input.
#[derive(Clone, Copy)]
pub(crate) struct PaintMark {
    scene: scene::Mark,
    input: [u32; 4],
    volatile: u32,
    culled: u32,
}

// The output of a paint span, which a later paint can copy while it paints under the same `PaintKey`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PaintSpan {
    scene: scene::Span,
    input: [[u32; 2]; 4],
}

// What paint reads from the window around a span.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PaintKey {
    pub(crate) mask: Clip<Pixels>,
    offset: Point<Pixels>,
    opacity: f32,
    smoothing: bool,
    scale: f32,
}

impl PaintKey {
    // Paint under both keys differs only by the place and clip of the content.
    pub(crate) fn same_paint(&self, other: &Self) -> bool {
        (self.opacity, self.smoothing, self.scale) == (other.opacity, other.smoothing, other.scale)
    }
}

impl Input {
    fn clear(&mut self) {
        self.regions.clear();
        match Rc::get_mut(&mut self.listeners) {
            Some(listeners) => listeners.clear(),
            None => self.listeners = Rc::default(),
        }
        self.cursors.clear();
        self.drag_areas.clear();
        self.marks.clear();
        self.pass = None;
        self.prepass = None;
    }

    fn lens(&self) -> [u32; 4] {
        [
            self.listeners.len(),
            self.cursors.len(),
            self.drag_areas.len(),
            self.marks.len(),
        ]
        .map(|len| len as u32)
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
    pub(crate) scene: Scene,
    // `pending` is being built; `live` is on screen and takes the input.
    pending: Input,
    live: Input,
    // The regions of an earlier prepaint that `pending` held, which a skip in the next prepaint copies.
    spare: (Option<u32>, Vec<Placed>),
    paints: u32,
    // Paint calls `paint_volatile` for content that it can't copy into the next frame.
    volatile: u32,
    // The next paint draws all boxes again.
    repaint: bool,
    pub(crate) unsettled: u32,
    // The count of hit transforms that prepaint made.
    pub(crate) transforms: u32,
    generation: usize,
    under: Vec<u64>,
    hovered: usize,
    next_region: u64,
    content_masks: Vec<(Clip<Pixels>, Clip<f32>)>,
    // The clips of `with_group_clip` in the space of the content masks.
    pub(crate) group_clips: Vec<Clip<Pixels>>,
    // The viewports of the scrolls whose content paints in a larger interest area, or `None` for other scrolls.
    viewports: Vec<Option<Rect<Pixels>>>,
    offsets: Vec<Point<Pixels>>,
    pub(crate) font_smoothing: bool,
    hit_transform: Option<Rc<HitTransform>>,
    // The transforms and the clip of each `with_hit_base` around the current box.
    hit_bases: Vec<(Option<Rc<HitTransform>>, Rect)>,
    // The digest and the size of each snapshot layer of the last frame.
    shots: FxHashMap<u64, (u64, [i32; 2])>,
    opacity: f32,
    // The mask layer and effect depth of `with_text_clip`.
    pub(crate) text_clip: Option<(usize, usize)>,
    states: FxHashMap<(NodeKey, TypeId), Box<dyn Any>>,
    next_states: FxHashMap<(NodeKey, TypeId), Box<dyn Any>>,
    frame_callbacks: Vec<FrameCallback>,
    // A patch frame waits for the end of the frame, where a full draw can take its place.
    pub(crate) patch: bool,
    pub(crate) captures: Vec<CaptureCallback>,
    pub(crate) ime_key: Option<(Event, bool)>,
    #[cfg(target_os = "macos")]
    caret: Option<Caret>,
    #[cfg(target_os = "macos")]
    caret_covered: bool,
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
    // The boxes whose style moved or sized them since the last layout, which keeps the offset for scroll anchors in
    // or below them.
    pub(crate) unanchored: rustc_hash::FxHashSet<u64>,
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
            spare: (None, Vec::new()),
            paints: 0,
            volatile: 0,
            repaint: true,
            generation: 0,
            under: Vec::new(),
            hovered: 0,
            next_region: 0,
            content_masks: Vec::new(),
            group_clips: Vec::new(),
            viewports: Vec::new(),
            unsettled: 0,
            transforms: 0,
            offsets: Vec::new(),
            font_smoothing: true,
            hit_transform: None,
            hit_bases: Vec::new(),
            shots: FxHashMap::default(),
            opacity: 1.,
            text_clip: None,
            states: FxHashMap::default(),
            next_states: FxHashMap::default(),
            frame_callbacks: Vec::new(),
            patch: false,
            captures: Vec::new(),
            ime_key: None,
            #[cfg(target_os = "macos")]
            caret: None,
            #[cfg(target_os = "macos")]
            caret_covered: false,
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
            unanchored: Default::default(),
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

    /// A change of the style of the box moves it or sizes it, so the next layout doesn't move scroll offsets for
    /// the scroll anchors in it.
    pub fn suppress_anchoring(&mut self, id: impl Into<crate::NodeKey>) {
        self.unanchored
            .insert(crate::tree::identity(Some(id.into()), 0));
    }

    // The node needs its key, or the patch is ignored.
    pub fn patch(&mut self, node: crate::Node) {
        self.patches.push(node);
    }

    pub fn redrawing(&self) -> bool {
        self.settling
    }

    // The CoreGraphics id of the display that shows the window, on macOS.
    pub fn display(&self) -> Option<u32> {
        #[cfg(target_os = "macos")]
        return self.platform.display();
        #[cfg(not(target_os = "macos"))]
        None
    }

    pub fn will_draw(&self) -> bool {
        self.drawing || self.dirty || !self.captures.is_empty()
    }

    // `blurs` are new radii of the filter blur and the backdrop blur, in pixels.
    pub fn patch_effect(
        &mut self,
        key: u64,
        opacity: f32,
        transform: Option<[f32; 16]>,
        blurs: [Option<f32>; 2],
    ) -> bool {
        let device = transform.map(|matrix| device_matrix(matrix, self.scale_factor));
        let blurs = blurs.map(|blur| blur.map(|blur| blur.max(0.) * self.scale_factor));
        if !self.scene.patch(key, opacity.clamp(0., 1.), device, blurs) {
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
                    crate::geometry::transform_bounds(&device_matrix(*matrix, scale), content)
                })
                .reduce(|all, bounds| all.union(bounds)),
            None => Some(content),
        };
        let region = crate::geometry::region(content);
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
        // WARNING: two presents in one tick fill the drawable queue, and each later frame then waits a
        // refresh for a drawable, with the input it handles.
        self.patch = true;
        self.platform.request_frame();
        true
    }

    pub(crate) fn draw_patch(&mut self) {
        if mem::take(&mut self.patch) {
            // The atlas and text caches keep their frame, so the tiles this scene reads stay.
            let _ = self.platform.draw(&self.scene, false);
            self.scene.drawn();
        }
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
        self.dirty || self.patch || !self.frame_callbacks.is_empty()
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
            let mask = self.snap_mask(self.visible_mask());
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
            control.clip = self.visible_mask().bounds * self.zoom;
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
            self.clear_pending();
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
        if self.live.pass.is_none() {
            self.scene.forget();
        }
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
        // A snapshot layer is read only when it draws otherwise than in the last frame.
        let mut shots = FxHashMap::default();
        for at in 0..self.scene.shots.len() {
            let (key, layer) = (self.scene.shots[at].key, self.scene.shots[at].layer);
            let [x, y, width, height] = self.scene.shots[at].region;
            let mut hasher = rustc_hash::FxHasher::default();
            let region = crate::rect(x as f32, y as f32, width as f32, height as f32);
            self.scene.digest(layer, region, &mut hasher);
            let now = (std::hash::Hasher::finish(&hasher), [width, height]);
            self.scene.shots[at].read = self.shots.get(&key) != Some(&now);
            shots.insert(key, now);
        }
        self.shots = shots;
        let fresh = self.platform.atlas().take_fresh();
        self.scene.diff(fresh.as_deref());
        let mut frame = self.platform.draw(&self.scene, !captures.is_empty());
        self.scene.drawn();
        let snapshots = self.platform.take_snapshots();
        // A layer that the backend could not read is read with the next frame.
        for shot in &mut self.scene.shots {
            if mem::take(&mut shot.read) && !snapshots.iter().any(|read| read.key == shot.key) {
                self.shots.remove(&shot.key);
            }
        }
        if let Some(tree) = &mut self.tree {
            tree.bury();
        }
        self.text_system.end_frame();
        self.platform.atlas().finish_frame(self.scene.texels());
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
        self.with_view(app, |view, window, app| view.drawn(snapshots, window, app));
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
            self.clear_pending();
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
        // A frame drawn by a listener has its own listeners, and the next paint copies from them.
        let listeners = self.live.listeners.clone();
        let count = listeners.len();
        let order = (0..count).map(|i| (i, true));
        for (i, capture) in order.chain((0..count).rev().map(|i| (i, false))) {
            let (phase, listener) = &listeners[i];
            // A listener that runs again inside itself is skipped.
            if *phase == capture
                && let Ok(mut listener) = listener.try_borrow_mut()
            {
                listener(&event, self, app);
                if app.event_stopped {
                    break;
                }
            }
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
            // Under a transform, places are local and the transform's area holds the viewports.
            view: match self.hit_transform {
                Some(_) => unbounded_mask().bounds,
                None => self.visible_mask().bounds,
            },
            transform: self.hit_transform.clone(),
            corners: Radii::default(),
        });
        region
    }

    /// Gives the last placed region the rounded corners of its box.
    pub fn round_region(&mut self, corners: Radii) {
        if let Some(placed) = self.pending.regions.last_mut() {
            placed.corners = corners;
        }
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
        Rc::make_mut(&mut self.pending.listeners).push((true, Rc::new(RefCell::new(listener))));
    }

    pub fn on_bubble(&mut self, listener: impl FnMut(&Event, &mut Window, &mut App) + 'static) {
        Rc::make_mut(&mut self.pending.listeners).push((false, Rc::new(RefCell::new(listener))));
    }

    /// Records a token in paint order, which `marks` returns after the frame. Copied paint keeps its tokens.
    pub fn mark(&mut self, token: u64) {
        self.pending.marks.push(token);
    }

    /// The tokens of `mark` in the last frame.
    pub fn marks(&self) -> &[u64] {
        &self.live.marks
    }

    /// The content that paint draws now reads state outside the tree, so it can't be copied into later frames.
    pub fn paint_volatile(&mut self) {
        self.volatile += 1;
    }

    /// Paint left out or placed content by the content mask, so a copy can't move with its box.
    pub fn cull(&mut self) {
        self.scene.culled += 1;
    }

    // The frame starts again after this paint, which the next paint doesn't copy.
    #[cfg_attr(debug_assertions, allow(dead_code))]
    pub(crate) fn restart_frame(&mut self) {
        self.restart = true;
        self.scene.abort();
    }

    /// The prepaint of the current box read state outside the tree that changed, so its last paint can't be copied.
    pub fn unsettle(&mut self) {
        self.unsettled = self.unsettled.wrapping_add(1);
    }

    /// The next frame paints all boxes again, as state that paint reads outside the tree changed.
    pub fn repaint(&mut self) {
        self.repaint = true;
    }

    // Whether the next paint can copy spans of the last one.
    pub(crate) fn copies(&self) -> bool {
        !self.repaint
            && !self.platform.atlas().evicted()
            && self.text_system.generation() == self.generation
            && self.live.pass.is_some()
            && self.scene.last_pass == self.live.pass
    }

    // Returns the pass, and the pass whose spans can be copied.
    pub(crate) fn begin_paint(&mut self) -> (u32, Option<u32>) {
        self.paints = self.paints.wrapping_add(1);
        let generation = self.text_system.generation();
        let atlas = self.platform.atlas();
        atlas.take_removed(&mut self.scene.removed);
        let evicted = atlas.take_evicted();
        let fresh = mem::take(&mut self.repaint) || evicted || generation != self.generation;
        self.generation = generation;
        self.scene.begin_paint(self.paints);
        self.pending.pass = Some(self.paints);
        let last = self
            .live
            .pass
            .filter(|_| !fresh && self.scene.last_pass == self.live.pass);
        (self.paints, last)
    }

    pub(crate) fn paint_key(&self) -> PaintKey {
        PaintKey {
            mask: self.content_mask(),
            offset: self.offset(),
            opacity: self.opacity,
            smoothing: self.font_smoothing,
            scale: self.scale_factor,
        }
    }

    pub(crate) fn paint_mark(&self) -> PaintMark {
        PaintMark {
            scene: self.scene.mark(),
            input: self.pending.lens(),
            volatile: self.volatile,
            culled: self.scene.culled,
        }
    }

    // The area in device pixels that a span drew in `mask`, when the span can move with its box: its paint cut
    // nothing and drew nothing outside the mask or in layers.
    pub(crate) fn plain(
        &self,
        span: &PaintSpan,
        mark: PaintMark,
        mask: Clip<Pixels>,
    ) -> Option<Rect<f32>> {
        (self.scene.culled == mark.culled)
            .then(|| self.scene.plain(&span.scene, &self.snap_mask(mask)))
            .flatten()
    }

    // Whether a plain span that drew `area` can move by `by` and draw in `mask`, as a paint there would draw.
    pub(crate) fn movable(&self, area: Rect<f32>, by: Point<Pixels>, mask: Clip<Pixels>) -> bool {
        let by = by.scale(self.scale_factor);
        let whole = |value: f32| value == value.round();
        whole(by.x) && whole(by.y) && scene::within(area + by, self.snap_mask(mask).bounds)
    }

    // Whether `now` is the mask `old` moved by whole device pixels.
    pub(crate) fn translated(
        &self,
        old: Clip<Pixels>,
        now: Clip<Pixels>,
        by: Point<Pixels>,
    ) -> bool {
        let by = by.scale(self.scale_factor);
        let [old, now] = [old, now].map(|mask| self.snap_mask(mask));
        let near =
            |a: Point<f32>, b: Point<f32>| (a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3;
        let whole = |value: f32| value == value.round();
        whole(by.x)
            && whole(by.y)
            && near(old.bounds.origin + by, now.bounds.origin)
            && near(old.bounds.bottom_right() + by, now.bounds.bottom_right())
            && (old.rounded.size == Size::default() && now.rounded.size == Size::default()
                || near(old.rounded.origin + by, now.rounded.origin)
                    && near(old.rounded.bottom_right() + by, now.rounded.bottom_right()))
            && old.radii == now.radii
    }

    // Whether a copy of a span can move with its box: its layers don't read pixels below or a mask layer, or
    // turn in perspective.
    pub(crate) fn translatable(&self, span: &PaintSpan) -> bool {
        self.scene.translatable(&span.scene)
    }

    // What a span drew. `last` reads a span of the last paint.
    #[cfg(debug_assertions)]
    pub(crate) fn span_records(
        &self,
        span: &PaintSpan,
        last: bool,
        moved: Option<(Point<Pixels>, Clip<Pixels>, Clip<Pixels>)>,
    ) -> Vec<String> {
        let moved = moved.map(|(by, old, now)| {
            (
                by.scale(self.scale_factor),
                self.snap_mask(old),
                self.snap_mask(now),
            )
        });
        let mut out = self.scene.records(&span.scene, last, moved);
        let input = if last { &self.live } else { &self.pending };
        let range = |[start, end]: [u32; 2]| start as usize..end as usize;
        let [listeners, cursors, drags, marks] = span.input.map(range);
        let phases: Vec<_> = input.listeners[listeners]
            .iter()
            .map(|(phase, _)| *phase)
            .collect();
        out.push(format!("listeners {phases:?}"));
        // The prepaint check of debug builds gives regions new keys.
        let kinds: Vec<_> = input.cursors[cursors]
            .iter()
            .map(|(_, cursor)| *cursor)
            .collect();
        out.push(format!("cursors {kinds:?}"));
        out.push(format!("drag areas {}", input.drag_areas[drags].len()));
        out.push(format!("marks {:?}", &input.marks[marks]));
        out
    }

    // `None` when paint read state outside the tree, or wrote where a copy can't follow.
    pub(crate) fn paint_span(&self, mark: PaintMark) -> Option<PaintSpan> {
        if self.volatile != mark.volatile || self.text_clip.is_some() {
            return None;
        }
        let ends = self.pending.lens();
        Some(PaintSpan {
            scene: self.scene.span(mark.scene)?,
            input: std::array::from_fn(|at| [mark.input[at], ends[at]]),
        })
    }

    // Copies a span of the last paint.
    // Debug builds paint instead, to check the copy.
    #[cfg_attr(debug_assertions, allow(dead_code))]
    pub(crate) fn replay(
        &mut self,
        span: &PaintSpan,
        moved: Option<(Point<Pixels>, Clip<Pixels>, Clip<Pixels>)>,
    ) -> Option<PaintSpan> {
        let ([listeners, cursors, drags, marks], live) = (span.input, &self.live);
        let range = |[start, end]: [u32; 2]| start as usize..end as usize;
        if self.text_clip.is_some()
            || live
                .lens()
                .iter()
                .zip(&span.input)
                .any(|(len, [_, end])| end > len)
        {
            return None;
        }
        let start = self.pending.lens();
        let scene = match moved {
            Some((by, old, now)) => {
                let (old, now) = (self.snap_mask(old), self.snap_mask(now));
                (self.scene).replay_moved(&span.scene, by.scale(self.scale_factor), &old, &now)?
            }
            None => self.scene.replay(&span.scene)?,
        };
        let pending = &mut self.pending;
        Rc::make_mut(&mut pending.listeners).extend_from_slice(&live.listeners[range(listeners)]);
        pending
            .cursors
            .extend_from_slice(&live.cursors[range(cursors)]);
        pending
            .drag_areas
            .extend_from_slice(&live.drag_areas[range(drags)]);
        pending.marks.extend_from_slice(&live.marks[range(marks)]);
        let ends = pending.lens();
        Some(PaintSpan {
            scene,
            input: std::array::from_fn(|at| [start[at], ends[at]]),
        })
    }

    // Kept subtrees reuse their regions only while this is unchanged.
    pub(crate) fn placement(&self) -> Placement {
        let transform = (self.hit_transform.as_ref()).map_or(0, |transform| transform.id);
        Placement {
            offset: self.offset(),
            mask: self.content_mask(),
            visible: self.visible_mask().bounds,
            depth: self.depth() as u32,
            transform,
        }
    }

    #[cfg(debug_assertions)]
    pub(crate) fn region_places(&self) -> Vec<(Rect, Clip<Pixels>, Hit, bool)> {
        let place = |placed: &Placed| {
            let region = placed.region;
            (
                region.bounds,
                Clip {
                    bounds: placed.clip.bounds.intersect(placed.view),
                    ..placed.clip
                },
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

    pub(crate) fn truncate_regions(&mut self, len: usize) {
        self.pending.regions.truncate(len);
    }

    fn clear_pending(&mut self) {
        if self.pending.prepass.is_some() {
            mem::swap(&mut self.spare.1, &mut self.pending.regions);
            self.spare.0 = self.pending.prepass;
        }
        self.pending.clear();
    }

    pub(crate) fn begin_prepaint(&mut self, pass: u32) {
        self.pending.prepass = Some(pass);
    }

    pub(crate) fn regions_len(&self) -> u32 {
        self.pending.regions.len() as u32
    }

    // Copies regions of the prepaint pass `pass` moved by a scroll offset `by`, and returns where they start.
    pub(crate) fn shift_regions(
        &mut self,
        pass: u32,
        range: [u32; 2],
        by: Point<Pixels>,
    ) -> Option<u32> {
        let at = self.copy_regions(pass, range)?;
        // The viewports of scrolls around the box can move with an outer scroll, so its regions see them anew.
        let viewports = self.viewport_bounds();
        let mut moved = Vec::new();
        for placed in &mut self.pending.regions[at as usize..] {
            placed.region.bounds.origin += by;
            // The open mask inside a transform stays.
            if placed.clip.bounds.size.width.0 < OPEN {
                placed.clip.bounds.origin += by;
            }
            if placed.transform.is_none() {
                placed.view = placed.clip.bounds.intersect(viewports);
            }
            if placed.clip.rounded.size != Size::default() {
                placed.clip.rounded.origin += by;
            }
            if let Some(transform) = &placed.transform {
                placed.transform = Some(transform.shifted(by, &mut moved));
            }
        }
        Some(at)
    }

    // Copies regions of the prepaint pass `pass`, and returns where they start.
    pub(crate) fn copy_regions(&mut self, pass: u32, [start, end]: [u32; 2]) -> Option<u32> {
        let source = match (self.live.prepass, self.spare.0) {
            (Some(live), _) if live == pass => &self.live.regions,
            (_, Some(spare)) if spare == pass => &self.spare.1,
            _ => return None,
        };
        let copied = source.get(start as usize..end as usize)?;
        let at = self.pending.regions.len() as u32;
        self.pending.regions.extend_from_slice(copied);
        Some(at)
    }

    /// Drops the state that `keyed` keeps, as for a new document whose nodes reuse the old keys.
    pub fn forget_states(&mut self) {
        self.states.clear();
        self.next_states.clear();
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

    // The part of the content mask that the window shows. In a scroll the content mask is a larger area.
    pub(crate) fn visible_mask(&self) -> Clip<Pixels> {
        let mut mask = self.content_mask();
        mask.bounds = mask.bounds.intersect(self.viewport_bounds());
        mask
    }

    // The viewports of the scrolls around the current box, whose content paints in a larger area.
    fn viewport_bounds(&self) -> Rect {
        (self.viewports.iter().flatten()).fold(unbounded_mask().bounds, |all, viewport| {
            all.intersect(*viewport)
        })
    }

    // The count of scroll viewports around the current box.
    pub(crate) fn depth(&self) -> usize {
        self.viewports.len()
    }

    // The content mask cut by the scroll viewports inside the first `depth`.
    pub(crate) fn visible_below(&self, depth: usize) -> Clip<Pixels> {
        let mut mask = self.content_mask();
        for viewport in self.viewports[depth.min(self.viewports.len())..]
            .iter()
            .flatten()
        {
            mask.bounds = mask.bounds.intersect(*viewport);
        }
        mask
    }

    // `mask` cut by the scroll viewports from `from` to `to`.
    pub(crate) fn cut_between(
        &self,
        mut mask: Clip<Pixels>,
        from: usize,
        to: usize,
    ) -> Clip<Pixels> {
        for viewport in self.viewports[from..to.min(self.viewports.len())]
            .iter()
            .flatten()
        {
            mask.bounds = mask.bounds.intersect(*viewport);
        }
        mask
    }

    // Stops the cuts of the scroll viewports inside the first `depth` until `resume_cuts`.
    pub(crate) fn pause_cuts(&mut self, depth: usize) -> Vec<Option<Rect<Pixels>>> {
        self.scene.pause_cuts(depth);
        self.hide_viewports(depth)
    }

    pub(crate) fn resume_cuts(&mut self, paused: Vec<Option<Rect<Pixels>>>) {
        self.show_viewports(paused);
        self.scene.resume_cuts();
    }

    // Takes the scroll viewports inside the first `depth` out of the clips until `show_viewports`.
    pub(crate) fn hide_viewports(&mut self, depth: usize) -> Vec<Option<Rect<Pixels>>> {
        let depth = depth.min(self.viewports.len());
        self.viewports[depth..]
            .iter_mut()
            .map(Option::take)
            .collect()
    }

    pub(crate) fn show_viewports(&mut self, hidden: Vec<Option<Rect<Pixels>>>) {
        let depth = self.viewports.len() - hidden.len();
        for (viewport, old) in self.viewports[depth..].iter_mut().zip(hidden) {
            *viewport = old;
        }
    }

    // Pair with pop_content_mask.
    pub(crate) fn push_content_mask(&mut self, mask: Clip<Pixels>) {
        let snapped = self.snap_mask(mask);
        self.content_masks.push((mask, snapped));
    }

    pub(crate) fn pop_content_mask(&mut self) {
        self.content_masks.pop();
    }

    /// Clips what `f` paints, also the out-of-flow boxes that escape the clips around them, as `clip-path`.
    pub fn with_group_clip<R>(&mut self, clip: Clip<Pixels>, f: impl FnOnce(&mut Self) -> R) -> R {
        self.group_clips.push(clip);
        let result = self.scope(Scope::Clip(Some(clip)), f);
        self.group_clips.pop();
        result
    }

    // The mask of an out-of-flow box inside the group clips.
    pub(crate) fn group_clipped(&self, mask: Clip<Pixels>) -> Clip<Pixels> {
        (self.group_clips.iter()).fold(mask, |mask, clip| clip.intersect(&mask))
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

    /// Draws what `f` paints into a layer of its own, which the window doesn't show. With a key, the pixels of
    /// `bounds` go to `View::drawn` after each frame in which they changed, or after this frame when `fresh`.
    pub fn with_snapshot<R>(
        &mut self,
        key: Option<(u64, bool)>,
        bounds: Rect,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let key = key.map(|(key, fresh)| {
            if fresh {
                self.shots.remove(&key);
            }
            key
        });
        self.scene.push_offscreen();
        // The layer is no part of the page, so the clips and the opacity around it don't apply.
        self.push_content_mask(unbounded_mask());
        let (opacity, text_clip) = (mem::replace(&mut self.opacity, 1.), self.text_clip.take());
        let groups = mem::take(&mut self.group_clips);
        let result = f(self);
        (self.opacity, self.text_clip, self.group_clips) = (opacity, text_clip, groups);
        self.content_masks.pop();
        let shot = key.map(|key| (key, self.snap_bounds(bounds)));
        self.scene.pop_offscreen(shot);
        result
    }

    /// Makes the transforms of this place the space of `with_hits_at`, where regions miss outside `clip`.
    pub fn with_hit_base<R>(&mut self, clip: Rect, f: impl FnOnce(&mut Self) -> R) -> R {
        let clip = clip.intersect(self.visible_mask().bounds);
        self.hit_bases.push((self.hit_transform.clone(), clip));
        let result = f(self);
        self.hit_bases.pop();
        result
    }

    /// The regions that `f` adds take the pointer where `matrix` puts them in the space of `with_hit_base`,
    /// whatever transforms and clips lie between. Without a matrix they stay where they are and take no pointer.
    pub fn with_hits_at<R>(
        &mut self,
        matrix: Option<[f32; 16]>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        const SAME: Plane = [1., 0., 0., 0., 1., 0., 0., 0., 1.];
        let (base, clip) = self.hit_bases.last().cloned().unwrap_or_default();
        let placed = matrix.and_then(|matrix| {
            let local = plane_of(&matrix);
            let to_window = match &base {
                Some(base) => compose(&base.to_window.get(), &local),
                None => local,
            };
            Some((local, to_window, invert(&to_window)?))
        });
        let placed = match placed {
            Some((local, to_window, from_window)) => HitTransform {
                key: None,
                id: HitTransform::id(
                    None,
                    &to_window,
                    base.as_ref().map_or(clip, |base| base.area),
                    base.as_ref()
                        .map_or(unbounded_mask().bounds, |base| base.view),
                ),
                area: base.as_ref().map_or(clip, |base| base.area),
                view: base
                    .as_ref()
                    .map_or(unbounded_mask().bounds, |base| base.view),
                outer: base,
                local: Cell::new(local),
                to_window: Cell::new(to_window),
                from_window: Cell::new(from_window),
            },
            // An empty area takes no pointer.
            None => HitTransform {
                key: None,
                id: HitTransform::id(None, &[0.; 9], Rect::default(), Rect::default()),
                area: Rect::default(),
                view: Rect::default(),
                local: Cell::new(SAME),
                to_window: Cell::new(
                    self.hit_transform
                        .as_ref()
                        .map_or(SAME, |outer| outer.to_window.get()),
                ),
                from_window: Cell::new(
                    self.hit_transform
                        .as_ref()
                        .map_or(SAME, |outer| outer.from_window.get()),
                ),
                outer: self.hit_transform.clone(),
            },
        };
        self.transforms = self.transforms.wrapping_add(1);
        let outer = self.hit_transform.replace(Rc::new(placed));
        self.push_content_mask(unbounded_mask());
        let result = f(self);
        self.content_masks.pop();
        self.hit_transform = outer;
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
        let (area, view) = match &outer {
            Some(outer) => (outer.area, outer.view),
            None => (self.content_mask().bounds, self.viewport_bounds()),
        };
        // Composited animations change a keyed transform in place.
        if key.is_some() {
            self.transforms = self.transforms.wrapping_add(1);
        }
        self.hit_transform = Some(Rc::new(HitTransform {
            key,
            outer,
            local: Cell::new(local),
            to_window: Cell::new(to_window),
            from_window: Cell::new(from_window),
            area,
            view,
            id: HitTransform::id(key, &to_window, area, view),
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

// Masks at least this wide are open.
const OPEN: f32 = 1e6;

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
            view: clip,
            transform,
            corners: Radii::default(),
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
            view: unbounded_mask().bounds,
            id: 1,
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
