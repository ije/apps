use crate::{
    App, Backdrop, BorderStyle, BoxShadow, Child, Clip, Color, Cursor, Edges, Event, Key,
    Modifiers, NodeKey, PaintEffect, Pixels, Point, Radii, Rect, RenderImage, ScrollRef, Size,
    Snapshot, Space, Tree, Utf16Selection, WheelPhase, WindowChange, WindowControl, WindowId,
    WindowState,
    geometry::{
        Plane, before_viewer, compose, faces_away, invert, multiply, plane_of, project, singular,
        snap, snap_width,
    },
    gpu::atlas::TileKey,
    gpu::scene::{self, BackdropState, EffectState, Scene},
    platform, point, px, size,
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
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

const REBARS: u8 = 4;
// With `APPS_TRACE_FCP`, the first frame with text or images prints its Unix time in ms, for `scripts/fcp.mjs`.
static TRACE_FCP: LazyLock<AtomicBool> =
    LazyLock::new(|| AtomicBool::new(std::env::var_os("APPS_TRACE_FCP").is_some()));

pub(crate) mod node;
mod paint;
mod retain;
pub(crate) mod scroll;
pub(crate) mod snap;
pub(crate) mod tree;
pub use paint::*;

/// Text input calls use UTF-16 offsets.
pub trait View: 'static {
    fn render(&mut self, tree: &Tree, window: &mut Window, app: &mut App) -> Child;
    fn window_changed(&mut self, window: &mut Window, app: &mut App);
    /// Pointer events go to hit regions, not here.
    fn event(&mut self, event: &Event, window: &mut Window, app: &mut App);
    /// Runs after each frame, with the layers of `Window::with_snapshot` that changed.
    fn drawn(&mut self, _snapshots: Vec<Snapshot>, _window: &mut Window, _app: &mut App) {}
    /// Runs at the start of each frame, after the steps of running scrolls and before the frame callbacks.
    fn frame(&mut self, _window: &mut Window, _app: &mut App) {}
    /// While true, frames draw nothing, so the window keeps its last frame.
    fn suppressed(&self) -> bool {
        false
    }

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
    // In a plane of a 3D rendering context.
    deep: bool,
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
            deep: self.deep,
            outer: (self.outer.as_ref()).map(|outer| outer.shifted(by, moved)),
            local: Cell::new(turn(self.local.get())),
            to_window: Cell::new(to_window),
            from_window: Cell::new(turn(self.from_window.get())),
            area,
            view: self.view,
            // A scroll keeps the id of a transform without a key.
            id: match self.key {
                Some(_) => {
                    HitTransform::id(self.key, &to_window, area, self.view, Point::default())
                }
                None => self.id,
            },
        });
        moved.push((Rc::as_ptr(self), shifted.clone()));
        shifted
    }

    // `offset` is the offset of the window where the transform starts. A scroll by `by` adds `by` to it, moves the
    // area, and turns an affine transform `M` into `T(by)·M·T(-by)`. So these hash as the content of the offset
    // sees them, on a grid that hides the rounding of the turn, and boxes under a transform stay kept in a scroll.
    fn id(
        key: Option<u64>,
        to_window: &Plane,
        area: Rect,
        view: Rect,
        offset: Point<Pixels>,
    ) -> u64 {
        use std::hash::{Hash, Hasher};
        static KEYED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        if key.is_some() {
            // Composited animations change keyed transforms in place.
            return KEYED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) | 1 << 63;
        }
        let mut hasher = rustc_hash::FxHasher::default();
        let [x, y] = [offset.x.0, offset.y.0];
        let near = |value: f32| (value * 16.).round() as i64;
        let m = to_window;
        let affine = (m[2], m[5], m[8]) == (0., 0., 1.);
        match affine {
            true => {
                [m[0], m[1], m[3], m[4]].map(f32::to_bits).hash(&mut hasher);
                [
                    near(m[6] + (m[0] - 1.) * x + m[3] * y),
                    near(m[7] + m[1] * x + (m[4] - 1.) * y),
                ]
                .hash(&mut hasher);
            }
            false => m.map(f32::to_bits).hash(&mut hasher),
        }
        match affine && area.size.width.0 < OPEN {
            true => [near(area.origin.x.0 - x), near(area.origin.y.0 - y)].hash(&mut hasher),
            false => [area.origin.x, area.origin.y]
                .map(|value| value.0.to_bits())
                .hash(&mut hasher),
        }
        [
            area.size.width,
            area.size.height,
            view.origin.x,
            view.origin.y,
            view.size.width,
            view.size.height,
        ]
        .map(|value| value.0.to_bits())
        .hash(&mut hasher);
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

// A 3D rendering context in prepaint: its matrix, and the transform and the areas around it.
struct HitSpace {
    matrix: [f32; 16],
    base: Option<Rc<HitTransform>>,
    area: (Rect, Rect),
}

// The layer and the box of a plane, and where its paint records start.
type PlaneStart = (usize, Rect<f32>, [u32; 4]);

// A 3D rendering context in paint: its matrix, the layer of its open plane, and the layer depth there.
struct PlaneSpace {
    matrix: [f32; 16],
    plane: EffectState,
    bounds: Rect<f32>,
    depth: usize,
    hidden: bool,
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
                // Points behind the viewer miss.
                let m = transform.from_window.get();
                (m[2] * at.x.0 + m[5] * at.y.0 + m[8] > 0.)
                    .then(|| project(&m, at))
                    .flatten()
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
    // For each 3D rendering context, inner ones first, the cursors and the marks of its planes from back to front.
    sorted: Vec<Vec<[[u32; 2]; 2]>>,
    // The paint pass that made the paint records, and the prepaint pass that made the regions.
    pass: Option<u32>,
    prepass: Option<u32>,
}

// The counts of `unsettle` calls and of hit transforms at a place in prepaint.
#[derive(Clone, Copy)]
pub(crate) struct PrepaintMark {
    unsettled: u32,
    transforms: u32,
}

// Where a paint span starts: in the scene and in the paint records of the input.
#[derive(Clone, Copy)]
pub(crate) struct PaintMark {
    scene: scene::Mark,
    input: [u32; 4],
    volatile: u32,
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
    pub(crate) offset: Point<Pixels>,
    opacity: f32,
    smoothing: bool,
    scale: f32,
}

impl PaintSpan {
    // Where this span, inside the span `from`, is in `to`, a copy of `from`.
    pub(crate) fn rebased(&self, from: &PaintSpan, to: &PaintSpan) -> Option<PaintSpan> {
        let mut input = self.input;
        for ((own, was), now) in input.iter_mut().zip(from.input).zip(to.input) {
            *own = scene::rebased(*own, was, now)?;
        }
        Some(PaintSpan {
            scene: self.scene.rebased(&from.scene, &to.scene)?,
            input,
        })
    }
}

impl PaintKey {
    // The key of paint that moved with its offset, in the mask `mask`.
    pub(crate) fn moved(&self, by: Point<Pixels>, mask: Clip<Pixels>) -> Self {
        Self {
            mask,
            offset: self.offset + by,
            ..*self
        }
    }

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
        self.sorted.clear();
        self.pass = None;
        self.prepass = None;
    }

    // A list of paint records with the planes of 3D rendering contexts from back to front.
    fn by_depth<'a, T: Copy>(&self, list: &'a [T], which: usize) -> Cow<'a, [T]> {
        let mut list = Cow::Borrowed(list);
        for planes in &self.sorted {
            let ranges = planes.iter().map(|plane| plane[which]);
            let Some(start) = ranges.clone().map(|[start, _]| start as usize).min() else {
                continue;
            };
            let sorted: Vec<T> = ranges
                .flat_map(|[start, end]| &list[start as usize..end as usize])
                .copied()
                .collect();
            list.to_mut()[start..start + sorted.len()].copy_from_slice(&sorted);
        }
        list
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
    pub(super) latch: Option<ScrollRef>,
    pub(super) spent: bool,
    pub(super) speed: Point<f32>,
    pub(super) edge: Option<(ScrollRef, Point<Pixels>)>,
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
    // The glyph and image tiles that paint takes, which the GPU backend of the window holds. Declared before
    // `platform`, so the backend drops the last handle, with its GPU device.
    atlas: Rc<platform::Atlas>,
    // A buffer that the atlas and the scene trade the removed atlas tiles in.
    removed: Vec<(bool, u32, Rect<i32>)>,
    platform: platform::Window,
    pub(super) view: Option<Box<dyn View>>,
    text_system: Rc<TextSystem>,
    scale_factor: f32,
    viewport_size: Size<Pixels>,
    zoom: f32,
    native: (Size<Pixels>, f32),
    tree: Option<Tree>,
    scene: Scene,
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
    unsettled: u32,
    // The count of hit transforms that prepaint made.
    transforms: u32,
    generation: usize,
    under: Vec<u64>,
    hovered: usize,
    next_region: u64,
    content_masks: Vec<(Clip<Pixels>, Clip<f32>)>,
    // The clips of `with_group_clip` in the space of the content masks.
    group_clips: Vec<Clip<Pixels>>,
    // The viewports of the scrolls whose content paints in a larger interest area, or `None` for other scrolls.
    viewports: Vec<Option<Rect<Pixels>>>,
    offsets: Vec<Point<Pixels>>,
    font_smoothing: bool,
    hit_transform: Option<Rc<HitTransform>>,
    hit_space: Option<Rc<HitSpace>>,
    // The 3D rendering context that paint is in, and the planes of the open contexts: their layers, their boxes and
    // where their paint records start.
    space: Option<Box<PlaneSpace>>,
    planes: Vec<Vec<PlaneStart>>,
    // The transforms and the clip of each `with_hit_base` around the current box.
    hit_bases: Vec<(Option<Rc<HitTransform>>, Rect)>,
    // The digest and the size of each snapshot layer of the last frame.
    shots: FxHashMap<u64, (u64, [i32; 2])>,
    opacity: f32,
    // The scale of the transforms of the effects around the current paint.
    transform_scale: f32,
    // The mask layer and effect depth of `with_text_clip`.
    text_clip: Option<(usize, usize)>,
    states: FxHashMap<(NodeKey, TypeId), Box<dyn Any>>,
    next_states: FxHashMap<(NodeKey, TypeId), Box<dyn Any>>,
    frame_callbacks: Vec<FrameCallback>,
    pub(super) drives: Vec<ScrollRef>,
    // A patch frame waits for the end of the frame, where a full draw can take its place.
    patch: bool,
    captures: Vec<CaptureCallback>,
    ime_key: Option<(Event, bool)>,
    #[cfg(target_os = "macos")]
    caret: Option<Caret>,
    #[cfg(target_os = "macos")]
    caret_covered: bool,
    #[cfg(target_os = "macos")]
    controls: (Vec<crate::NativeControl>, Option<crate::ControlSink>),
    dirty: bool,
    drawing: bool,
    // `again`: the page asked for another pass while one was built. `settling`: this is that extra pass.
    again: bool,
    settling: bool,
    // `rebar`: the bars of this pass are not the ones it was built with. `rebars`: the extra passes for bars.
    rebar: bool,
    rebars: u8,
    // `rebars` can come to this in a frame.
    most_rebars: u8,
    // The marks that texts placed in this prepaint.
    marks: Vec<(u64, Point<Pixels>, Option<bool>)>,
    restart: bool,
    restarts: u8,
    rebuild: bool,
    patches: Vec<crate::Node>,
    // The boxes whose style moved or sized them since the last layout, which keeps the offset for scroll anchors in
    // or below them.
    unanchored: rustc_hash::FxHashSet<u64>,
    pointer: Point<Pixels>,
    pub(super) closed: bool,
    cursor: Option<Cursor>,
    drawn_at: Instant,
    frames_drawn: u64,
    tracks: Vec<Track>,
    rails: ([f32; 2], Instant),
    // The time of a frame that a display tick started, and when its device sent the event in dispatch.
    frame_at: Option<Instant>,
    event_at: Option<Instant>,
    scroll_chain: Option<Vec<ScrollRef>>,
    gesture: ScrollGesture,
}

impl Window {
    pub(crate) fn new(
        id: WindowId,
        platform: platform::Window,
        text_system: Rc<TextSystem>,
    ) -> Self {
        let (scale_factor, viewport_size) = (platform.scale_factor(), platform.content_size());
        text_system.scale.set(scale_factor);
        let atlas = platform.atlas().clone();
        Self {
            id,
            platform,
            view: None,
            text_system,
            atlas,
            removed: Vec::new(),
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
            hit_space: None,
            space: None,
            planes: Vec::new(),
            hit_bases: Vec::new(),
            shots: FxHashMap::default(),
            opacity: 1.,
            transform_scale: 1.,
            text_clip: None,
            states: FxHashMap::default(),
            next_states: FxHashMap::default(),
            frame_callbacks: Vec::new(),
            drives: Vec::new(),
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
            rebar: false,
            rebars: 0,
            most_rebars: REBARS,
            marks: Vec::new(),
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

    /// How much the transforms around the current paint scale it, without the device scale. A 3D or
    /// skewed transform gives the larger axis.
    pub fn transform_scale(&self) -> f32 {
        self.transform_scale
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

    /// Scroll bars came or went, which can make other bars come or go, or a text moved a mark, so this pass is
    /// followed by another one, up to a few in a frame.
    pub fn rebar(&mut self) {
        self.redraw();
        self.rebar = self.drawing;
    }

    /// The boxes with anchors moved, and take more passes than scroll bars until they settle: the boxes they are
    /// anchored to can move, and they try positions.
    pub fn reanchor(&mut self) {
        self.rebar();
        self.most_rebars = 64;
    }

    /// A text put the mark `key` in a line, `by` from where layout placed it. Text of a direction
    /// has the box at that side of the mark.
    pub fn place_mark(&mut self, key: u64, by: Point<Pixels>, rtl: Option<bool>) {
        self.marks.push((key, by, rtl));
    }

    pub(crate) fn take_marks(&mut self) -> Vec<(u64, Point<Pixels>, Option<bool>)> {
        mem::take(&mut self.marks)
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
            .insert(crate::app::window::tree::identity(Some(id.into()), 0));
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
        self.platform.demote();
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
            self.platform.promote(
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
            let (_, snapshots) = self.platform.draw(&self.scene, None, false);
            debug_assert!(
                snapshots.is_empty(),
                "a patch frame reads no snapshot layers"
            );
            self.scene.drawn();
        }
    }

    pub fn next_frame(&mut self, job: impl FnOnce(&mut Window, &mut App) + 'static) {
        self.frame_callbacks.push(Box::new(job));
        self.platform.request_frame();
    }

    pub fn request_frame(&mut self) {
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
        self.text_system.scale.set(self.scale_factor);
        self.dirty = false;
        self.refresh();
    }

    // The frame of a display tick or a forced frame.
    pub(crate) fn frame(&mut self, app: &mut App) {
        for handle in mem::take(&mut self.drives) {
            handle.step(self);
        }
        if let Some(mut view) = self.view.take() {
            view.frame(self, app);
            self.view.get_or_insert(view);
        }
        for callback in mem::take(&mut self.frame_callbacks) {
            callback(self, app);
        }
        // A capture draws also while frames are suppressed, as nothing would draw it later.
        let suppressed = self.view.as_ref().is_some_and(|view| view.suppressed());
        if self.dirty && !suppressed || !self.captures.is_empty() {
            // The diff damage holds the regions of a patch that was not drawn.
            self.patch = false;
            self.draw(app);
        }
        self.draw_patch();
        self.frame_at = None;
    }

    // A native event, in zoomed pixels. `drop_held` forgets a key that waits for the input method.
    pub(crate) fn dispatch_native(
        &mut self,
        mut event: Event,
        app: &mut App,
        drop_held: bool,
    ) -> bool {
        if drop_held {
            self.ime_key = None;
        }
        if self.zoom != 1. {
            event.unzoom(self.zoom);
        }
        self.dispatch_event(event, app)
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn dispatch_at(&mut self, event: Event, sent: Instant, app: &mut App) {
        self.event_at = Some(sent);
        self.dispatch_native(event, app, false);
        self.event_at = None;
    }

    // A key that the input method may take. The text it types or composes says what the key was.
    #[cfg(target_os = "macos")]
    pub(crate) fn hold_key(&mut self, key: Event, composing: bool) {
        self.ime_key = Some((key, composing));
    }

    // `ime` is set when the key typed more than one character or composes, and then the page ignores it.
    pub(crate) fn send_ime_key(&mut self, app: &mut App, typed: bool) -> bool {
        let Some((mut key, composing)) = self.ime_key.take() else {
            return false;
        };
        let ime = typed || composing;
        if let Event::KeyDown { ime: held, .. } = &mut key {
            *held = ime;
        }
        self.dispatch_native(key, app, false) && !ime
    }

    // Gives back the GPU memory that the window does not need while it is idle.
    pub(crate) fn release_gpu(&mut self) {
        self.platform.release(&self.scene);
    }

    // A display tick at `at`, or a forced frame with `None`. A tick while the GPU queue is full draws no frame,
    // which takes one frame out of the queue. A window that wants no more frames rests.
    #[cfg(target_os = "macos")]
    pub(crate) fn tick(&mut self, at: Option<Instant>, app: &mut App) {
        if let Some(at) = at {
            if !self.platform.tick() {
                return;
            }
            self.frame_at = Some(at);
        }
        self.frame(app);
        if !self.wants_frame() {
            self.platform.rest();
        }
    }

    // A frame of a live resize, which presents inside the transaction of AppKit when `visible`. Returns its present.
    #[cfg(target_os = "macos")]
    pub(crate) fn present_live(
        &mut self,
        visible: bool,
        app: &mut App,
    ) -> Option<platform::Present> {
        self.platform.begin_live(visible);
        self.tick(None, app);
        self.platform.end_live()
    }

    pub(crate) fn wants_frame(&self) -> bool {
        self.dirty || self.patch || !self.frame_callbacks.is_empty() || !self.drives.is_empty()
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
                    tree.layout(
                        root,
                        self.viewport_size,
                        mem::take(&mut self.unanchored),
                        self,
                    );
                    tree.prepaint(root, self);
                    self.hovered = self.pending.probe(self.pointer, &mut self.under);
                    let restarting = self.restart && self.restarts < 4;
                    let again = self.again || self.restart;
                    let rebar = self.rebar && self.rebars < self.most_rebars;
                    if !restarting && (!again || self.settling && !rebar) {
                        tree.paint(root, self);
                    }
                }
                self.tree = Some(tree);
            }
            self.drawing = false;
            let restart = mem::take(&mut self.restart);
            if restart && self.restarts < 4 {
                self.restarts += 1;
                (self.again, self.rebar) = (false, false);
                continue;
            }
            let again = mem::take(&mut self.again) || restart;
            let rebar = mem::take(&mut self.rebar) && self.rebars < self.most_rebars;
            if again && (rebar || !self.settling) {
                self.rebars += u8::from(rebar);
                self.settling = true;
                continue;
            }
            self.settling = false;
            self.rebars = 0;
            self.most_rebars = REBARS;
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
            self.platform.set_caret(
                caret
                    .filter(|caret| caret.overlay && !covered)
                    .map(|caret| (caret.bounds, caret.color, caret.since)),
            );
            let controls = mem::take(&mut self.controls.0);
            self.platform
                .set_controls(controls, self.controls.1.clone());
        }
        let captures = mem::take(&mut self.captures);
        self.scene.read_shots(&mut self.shots);
        let fresh = self.atlas.take_fresh();
        // Patches of composited animations before a paint in the same frame can't place their damage, but the diff
        // finds it.
        self.scene.rediff_patches();
        self.scene.diff(fresh.as_deref());
        let (mut frame, snapshots) =
            (self.platform).draw(&self.scene, Some(self.native), !captures.is_empty());
        if TRACE_FCP.load(Relaxed) && self.scene.contentful() && TRACE_FCP.swap(false, Relaxed) {
            let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
            eprintln!(
                "apps: first contentful paint at {:.3}",
                at.unwrap_or_default().as_secs_f64() * 1000.
            );
        }
        self.scene.drawn();
        self.scene.keep_unread(&snapshots, &mut self.shots);
        if let Some(tree) = &mut self.tree {
            tree.bury();
        }
        self.text_system.end_frame();
        self.atlas.finish_frame(self.scene.texels());
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
        let mut again;
        loop {
            self.clear_pending();
            let mut tree = self.tree.take().unwrap_or_default();
            if let Some(root) = self.build(&mut tree, app, &mut render) {
                tree.layout(
                    root,
                    self.viewport_size,
                    mem::take(&mut self.unanchored),
                    self,
                );
                tree.prepaint(root, self);
            }
            self.tree = Some(tree);
            let states = mem::take(&mut self.next_states);
            self.states.extend(states);
            again = mem::take(&mut self.again);
            let rebar = mem::take(&mut self.rebar) && self.rebars < self.most_rebars;
            if !again || self.settling && !rebar {
                break;
            }
            self.rebars += u8::from(rebar);
            self.settling = true;
        }
        self.drawing = false;
        self.settling = false;
        self.rebars = 0;
        self.most_rebars = REBARS;
        if again {
            self.refresh();
        }
        true
    }

    fn update_cursor(&mut self, app: &mut App) {
        let hovered = &self.under[..self.hovered];
        let cursor = self
            .live
            .by_depth(&self.live.cursors, 0)
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
        {
            if handle.release(self.gesture.speed, self) {
                self.gesture.spent = true;
            }
            handle.settle(self);
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
            |event: Event| crate::sync_main(move || crate::native::key(id, &event, &|| false));
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
    pub fn marks(&self) -> Cow<'_, [u64]> {
        self.live.by_depth(&self.live.marks, 1)
    }

    /// The content that paint draws now reads state outside the tree, so it can't be copied into later frames.
    pub fn paint_volatile(&mut self) {
        self.volatile += 1;
    }

    /// Paint left out or placed content by the content mask, so a copy can't move with its box.
    pub fn cull(&mut self) {
        self.scene.cull();
    }

    // The frame starts again after this paint, which the next paint doesn't copy.
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

    fn clear_pending(&mut self) {
        if self.pending.prepass.is_some() {
            mem::swap(&mut self.spare.1, &mut self.pending.regions);
            self.spare.0 = self.pending.prepass;
        }
        self.pending.clear();
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
    // An out-of-flow box paints in `f` outside the cuts and viewports of the scrolls inside the first `depth`.
    pub(crate) fn with_cuts_paused<R>(
        &mut self,
        depth: usize,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.scene.pause_cuts(depth);
        let paused = self.hide_viewports(depth);
        let result = f(self);
        self.show_viewports(paused);
        self.scene.resume_cuts();
        result
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

    /// Clips what `f` paints to `mask` inside the content mask. A mask with radii takes the place of the radii of
    /// the content mask.
    pub fn with_content_mask<R>(
        &mut self,
        mask: Clip<Pixels>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.push_content_mask(mask.intersect(&self.content_mask()));
        let result = f(self);
        self.pop_content_mask();
        result
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

    /// Like `with_snapshot` with a fresh key, but the window shows what `f` paints too. The pixels of the
    /// snapshots inside are not part of this one.
    pub fn with_shown_snapshot(&mut self, key: u64, bounds: Rect, f: &mut dyn FnMut(&mut Self)) {
        self.shots.remove(&key);
        let layer = self.scene.next_layer();
        self.with_effect(PaintEffect::default(), f);
        let bounds = self.snap_bounds(bounds);
        self.scene.shoot(layer, key, bounds);
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
        let offset = self.offset();
        let placed = match placed {
            Some((local, to_window, from_window)) => HitTransform {
                key: None,
                deep: base.as_ref().is_some_and(|base| base.deep),
                id: HitTransform::id(
                    None,
                    &to_window,
                    base.as_ref().map_or(clip, |base| base.area),
                    base.as_ref()
                        .map_or(unbounded_mask().bounds, |base| base.view),
                    offset,
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
                deep: false,
                id: HitTransform::id(None, &[0.; 9], Rect::default(), Rect::default(), offset),
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
        let (area, view) = self.hit_area(&outer);
        // Composited animations change a keyed transform in place.
        if key.is_some() {
            self.transforms = self.transforms.wrapping_add(1);
        }
        self.hit_transform = Some(Rc::new(HitTransform {
            key,
            deep: outer.as_ref().is_some_and(|outer| outer.deep),
            outer,
            local: Cell::new(local),
            to_window: Cell::new(to_window),
            from_window: Cell::new(from_window),
            area,
            view,
            id: HitTransform::id(key, &to_window, area, view, self.offset()),
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

    fn hit_area(&self, outer: &Option<Rc<HitTransform>>) -> (Rect, Rect) {
        match outer {
            Some(outer) => (outer.area, outer.view),
            None => (self.content_mask().bounds, self.viewport_bounds()),
        }
    }

    /// Like `with_keyed_hit_transform` without a key, also for a box in a 3D rendering context. A box that hides
    /// its back takes no pointer when it faces away.
    pub fn with_space_hits<R>(
        &mut self,
        m: [f32; 16],
        space: Space,
        hides_back: bool,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let around = self.hit_space.take();
        let (m, base, (mut area, view)) = match around.as_ref().filter(|_| space.joins) {
            Some(space) => (multiply(&space.matrix, &m), space.base.clone(), space.area),
            None => {
                let base = self.hit_transform.clone();
                let area = self.hit_area(&base);
                (m, base, area)
            }
        };
        if space.preserves {
            self.hit_space = Some(Rc::new(HitSpace {
                matrix: m,
                base: base.clone(),
                area: (area, view),
            }));
        }
        let local = plane_of(&m);
        let to_window =
            (base.as_ref()).map_or(local, |base| compose(&base.to_window.get(), &local));
        let hidden = hides_back && faces_away(&m) || singular(&m);
        if hidden {
            area = Rect::default();
        }
        let placed = HitTransform {
            key: None,
            deep: space.joins
                || space.preserves
                || hidden
                || (to_window[2], to_window[5], to_window[8]) != (0., 0., 1.)
                || base.as_ref().is_some_and(|base| base.deep),
            outer: base,
            local: Cell::new(local),
            to_window: Cell::new(to_window),
            // A plane on its edge takes no pointer.
            from_window: Cell::new(invert(&to_window).unwrap_or_default()),
            area,
            view,
            id: HitTransform::id(None, &to_window, area, view, self.offset()),
        };
        let outer = self.hit_transform.replace(Rc::new(placed));
        self.push_content_mask(unbounded_mask());
        let result = f(self);
        self.content_masks.pop();
        self.hit_transform = outer;
        self.hit_space = around;
        result
    }

    /// The corners in the window of the part of a box before the viewer, for boxes in 3D. None when it takes no
    /// pointer.
    pub fn hit_shape(&self, bounds: Rect) -> Option<Box<[Point<Pixels>]>> {
        let transform = (self.hit_transform.as_ref()).filter(|transform| transform.deep)?;
        let to_window = transform.to_window.get();
        if transform.area.is_empty() || invert(&to_window).is_none() {
            return Some(Box::default());
        }
        let m = to_window.map(f64::from);
        let corners = [
            bounds.origin,
            bounds.top_right(),
            bounds.bottom_right(),
            bounds.bottom_left(),
        ]
        .map(|at| {
            let [x, y] = [at.x.0, at.y.0].map(f64::from);
            [0, 1, 2].map(|row| m[row] * x + m[3 + row] * y + m[6 + row])
        });
        let shape = before_viewer(&corners).into_iter();
        Some(
            shape
                .map(|[x, y, _]| point(px(x as f32), px(y as f32)))
                .collect(),
        )
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
            deep: false,
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
