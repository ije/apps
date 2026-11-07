use crate::{
    App, Backdrop, BorderStyle, Bounds, BoxShadow, Child, ContentMask, CursorStyle, DevicePixels,
    Edges, ElementId, Modifiers, MouseEvent, PaintEffect, Pixels, PlatformInput, Point, Radii,
    RenderImage, Rgba, ScaledPixels, ScrollDelta, ScrollHandle, ScrollPhase, Size, Tree,
    Utf16Selection,
    atlas::AtlasKey,
    geometry::{round_half_toward_zero, round_stroke_to_device_pixel, round_to_device_pixel},
    input::{DispatchPhase, KeyDownEvent, KeyUpEvent, Keystroke},
    platform, point, px,
    scene::{
        BackdropState, EffectState, MonochromeSprite, PolychromeSprite, Primitive, Quad, Scene,
        Shadow, Underline,
    },
    size,
    text::{
        DecorationStyle, FontId, GlyphId, RenderGlyphParams, SUBPIXEL_VARIANTS_X,
        StrikethroughStyle, TextSystem, UnderlineStyle,
    },
};
use anyhow::Result;
use rustc_hash::FxHashMap;
use std::{
    any::{Any, TypeId},
    borrow::Cow,
    cell::Cell,
    mem,
    ops::Range,
    rc::Rc,
    sync::Arc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub u64);

pub trait View: 'static {
    fn render(&mut self, tree: &Tree, window: &mut Window, cx: &mut App) -> Child;

    fn window_changed(&mut self, window: &mut Window, cx: &mut App);

    fn event(&mut self, event: &PlatformInput, window: &mut Window, cx: &mut App);

    fn selected_text_range(&mut self, window: &mut Window, cx: &mut App) -> Option<Utf16Selection>;
    fn marked_text_range(&mut self, window: &mut Window, cx: &mut App) -> Option<Range<usize>>;
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<String>;
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    );
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    );
    fn unmark_text(&mut self, window: &mut Window, cx: &mut App);
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>>;
    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<usize>;
}

#[derive(Clone, Debug)]
pub struct WindowOptions {
    pub bounds: Bounds<Pixels>,
    pub outer_size: bool,
    pub title: String,
    pub titlebar_hidden: bool,
    pub traffic_light_position: Option<Point<Pixels>>,
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
            bounds: Bounds::new(point(px(0.), px(0.)), size(px(800.), px(600.))),
            outer_size: true,
            title: String::new(),
            titlebar_hidden: false,
            traffic_light_position: None,
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
    pub frame: Bounds<Pixels>,
    pub content: Bounds<Pixels>,
    pub minimized: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    pub visible: bool,
    pub resizing: bool,
    pub display: Bounds<Pixels>,
    pub work_area: Bounds<Pixels>,
}

#[cfg(target_os = "macos")]
pub use crate::platform::WindowControl;

#[cfg(not(target_os = "macos"))]
#[derive(Clone, Copy)]
pub struct WindowControl(WindowId);

#[cfg(not(target_os = "macos"))]
impl WindowControl {
    fn run(&self, f: impl FnOnce(&platform::Window) + 'static) {
        let id = self.0;
        crate::app::update(move |app| {
            app.update_window(id, |window, _| f(&window.platform));
        });
    }
    pub fn change(&self, change: WindowChange) {
        self.run(move |window| window.change(change));
    }
    pub fn set_title(&self, title: &str) {
        let title = title.to_owned();
        self.run(move |window| window.set_title(&title));
    }
    pub fn set_button_visibility(&self, visible: bool) {
        self.run(move |window| window.set_button_visibility(visible));
    }
    pub fn set_button_position(&self, position: Option<Point<Pixels>>) {
        self.run(move |window| window.set_button_position(position));
    }
    pub fn minimize(&self) {
        self.run(|window| window.minimize());
    }
    pub fn zoom(&self) {
        self.run(|window| window.zoom());
    }
    pub fn toggle_fullscreen(&self) {
        self.run(|window| window.toggle_fullscreen());
    }
    pub fn activate(&self) {
        self.run(|window| window.activate());
    }
    pub fn resize(&self, size: Size<Pixels>) {
        self.run(move |window| window.resize(size));
    }
    pub fn is_active(&self) -> bool {
        let id = self.0;
        crate::app::try_update(|app| app.update_window(id, |window, _| window.platform.is_active()))
            .flatten()
            .unwrap_or(false)
    }
    pub fn state(&self) -> Option<WindowState> {
        let id = self.0;
        crate::app::try_update(|app| app.update_window(id, |window, _| window.platform.state()))
            .flatten()
    }
    pub fn raw_window_handle(&self) -> Result<raw_window_handle::RawWindowHandle> {
        let id = self.0;
        crate::app::try_update(|app| {
            app.update_window(id, |window, _| window.platform.raw_window_handle())
        })
        .flatten()
        .ok_or_else(|| anyhow::anyhow!("The window is closed"))?
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowChange {
    Frame(Bounds<Pixels>),
    Center,
    Show(bool),
    Hide,
    Blur,
    Restore,
    SizeLimits(Size<Pixels>, Size<Pixels>),
    AlwaysOnTop(bool),
    Resizable(bool),
    Minimizable(bool),
    Maximizable(bool),
    Closable(bool),
    Movable(bool),
    Background(Rgba),
    Opacity(f32),
    Shadow(bool),
    IgnoreMouse(bool, bool),
    AllWorkspaces(bool, bool),
    ContentProtection(bool),
    Focusable(bool),
    AspectRatio(f32, Size<Pixels>),
    Parent(Option<raw_window_handle::RawWindowHandle>, bool),
    Vibrancy(Option<isize>, isize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HitboxId(u64);

impl HitboxId {
    pub fn is_hovered(self, window: &Window) -> bool {
        window.hit_test[..window.hover_count].contains(&self)
    }
    pub fn should_handle_scroll(self, window: &Window) -> bool {
        window.hit_test.contains(&self)
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum HitboxBehavior {
    #[default]
    Normal,
    BlockMouseExceptScroll,
}

#[derive(Clone, Debug)]
pub struct Hitbox {
    pub id: HitboxId,
    pub bounds: Bounds<Pixels>,
    pub content_mask: ContentMask<Pixels>,
    pub behavior: HitboxBehavior,
    transform: Option<Rc<HitTransform>>,
}

impl std::ops::Deref for Hitbox {
    type Target = Bounds<Pixels>;
    fn deref(&self) -> &Bounds<Pixels> {
        &self.bounds
    }
}

impl Hitbox {
    pub fn is_hovered(&self, window: &Window) -> bool {
        self.id.is_hovered(window)
    }
    pub fn should_handle_scroll(&self, window: &Window) -> bool {
        self.id.should_handle_scroll(window)
    }
}

#[derive(Debug, PartialEq)]
struct HitTransform {
    local: Cell<[f32; 9]>,
    key: Option<u64>,
    outer: Option<Rc<HitTransform>>,
    forward: Cell<[f32; 9]>,
    inverse: Cell<[f32; 9]>,
    window_mask: Bounds<Pixels>,
}

impl HitTransform {
    fn refresh(&self, key: u64, local: [f32; 9], done: &mut Vec<*const Self>) {
        if done.contains(&(self as *const Self)) {
            return;
        }
        if let Some(outer) = &self.outer {
            outer.refresh(key, local, done);
        }
        if self.key == Some(key) {
            self.local.set(local);
        }
        let forward = match &self.outer {
            Some(outer) => mul3(&outer.forward.get(), &self.local.get()),
            None => self.local.get(),
        };
        if let Some(inverse) = invert3(&forward) {
            self.forward.set(forward);
            self.inverse.set(inverse);
        }
        done.push(self);
    }

    fn map(m: &[f32; 9], position: Point<Pixels>) -> Option<Point<Pixels>> {
        let [x, y] = [position.x.0, position.y.0];
        let w = m[2] * x + m[5] * y + m[8];
        (w.abs() > 1e-6).then(|| {
            point(
                px((m[0] * x + m[3] * y + m[6]) / w),
                px((m[1] * x + m[4] * y + m[7]) / w),
            )
        })
    }
}

fn plane(m: &[f32; 16]) -> [f32; 9] {
    [m[0], m[1], m[3], m[4], m[5], m[7], m[12], m[13], m[15]]
}

fn device_matrix(mut matrix: [f32; 16], scale: f32) -> [f32; 16] {
    for row in [3, 7, 11] {
        matrix[row] /= scale;
    }
    for column in [12, 13, 14] {
        matrix[column] *= scale;
    }
    matrix
}

fn mul3(a: &[f32; 9], b: &[f32; 9]) -> [f32; 9] {
    std::array::from_fn(|index| {
        let (column, row) = (index / 3, index % 3);
        (0..3).map(|k| a[k * 3 + row] * b[column * 3 + k]).sum()
    })
}

fn invert3(m: &[f32; 9]) -> Option<[f32; 9]> {
    let [a, b, c, d, e, f, g, h, i] = *m;
    let det = a * (e * i - f * h) - d * (b * i - c * h) + g * (b * f - c * e);
    (det.abs() > 1e-9).then(|| {
        [
            (e * i - f * h) / det,
            (c * h - b * i) / det,
            (b * f - c * e) / det,
            (f * g - d * i) / det,
            (a * i - c * g) / det,
            (c * d - a * f) / det,
            (d * h - e * g) / det,
            (b * g - a * h) / det,
            (a * e - b * d) / det,
        ]
    })
}

#[derive(Clone, Debug)]
pub struct PaintQuad {
    pub bounds: Bounds<Pixels>,
    pub corner_radii: Radii<Pixels>,
    pub background: Rgba,
    pub border_widths: Edges<Pixels>,
    pub border_color: Rgba,
    pub border_style: BorderStyle,
    pub border_sides: u8,
}

impl PaintQuad {
    pub fn sides(self, border_sides: u8) -> Self {
        Self {
            border_sides,
            ..self
        }
    }

    pub fn corner_radii(self, radii: impl Into<Radii<Pixels>>) -> Self {
        Self {
            corner_radii: radii.into(),
            ..self
        }
    }
}

pub fn quad(
    bounds: Bounds<Pixels>,
    corner_radii: impl Into<Radii<Pixels>>,
    background: Rgba,
    border_widths: impl Into<Edges<Pixels>>,
    border_color: Rgba,
    border_style: BorderStyle,
) -> PaintQuad {
    PaintQuad {
        bounds,
        corner_radii: corner_radii.into(),
        background,
        border_widths: border_widths.into(),
        border_color,
        border_style,
        border_sides: 0,
    }
}

pub fn fill(bounds: Bounds<Pixels>, background: Rgba) -> PaintQuad {
    quad(
        bounds,
        px(0.),
        background,
        px(0.),
        crate::transparent_black(),
        BorderStyle::Solid,
    )
}

pub(crate) type MouseListener =
    Box<dyn FnMut(&PlatformInput, DispatchPhase, &mut Window, &mut App)>;
type FrameCallback = Box<dyn FnOnce(&mut Window, &mut App)>;
pub(crate) type CaptureCallback = Box<dyn FnOnce(Result<([u32; 2], Vec<u8>)>, &mut App)>;

#[derive(Default)]
struct Frame {
    hitboxes: Vec<Hitbox>,
    mouse_listeners: Vec<Option<MouseListener>>,
    cursors: Vec<(HitboxId, CursorStyle)>,
    drag_areas: Vec<HitboxId>,
}

impl Frame {
    fn clear(&mut self) {
        self.hitboxes.clear();
        self.mouse_listeners.clear();
        self.cursors.clear();
        self.drag_areas.clear();
    }

    fn turn(&self, key: u64, matrix: &[f32; 16]) {
        let local = plane(matrix);
        let mut done = Vec::new();
        for hitbox in &self.hitboxes {
            if let Some(transform) = &hitbox.transform
                && std::iter::successors(Some(transform), |inner| inner.outer.as_ref())
                    .any(|inner| inner.key == Some(key))
            {
                transform.refresh(key, local, &mut done);
            }
        }
    }

    fn hit_test(&self, position: Point<Pixels>, ids: &mut Vec<HitboxId>) -> usize {
        ids.clear();
        let mut hovered = None;
        for hitbox in self.hitboxes.iter().rev() {
            let bounds = hitbox.bounds.intersect(&hitbox.content_mask.bounds);
            let local = match &hitbox.transform {
                Some(transform) if transform.window_mask.contains(&position) => {
                    HitTransform::map(&transform.inverse.get(), position)
                }
                Some(_) => None,
                None => Some(position),
            };
            if local.is_some_and(|local| bounds.contains(&local)) {
                ids.push(hitbox.id);
                match hitbox.behavior {
                    HitboxBehavior::BlockMouseExceptScroll if hovered.is_none() => {
                        hovered = Some(ids.len())
                    }
                    _ => {}
                }
            }
        }
        hovered.unwrap_or(ids.len())
    }
}

#[derive(Default)]
pub(crate) struct ScrollGesture {
    pub(crate) latch: Option<ScrollHandle>,
    pub(crate) spent: bool,
    pub(crate) speed: Point<f32>,
    pub(crate) edge: Option<(ScrollHandle, Point<Pixels>)>,
}

pub const CARET_BLINK: std::time::Duration = std::time::Duration::from_millis(500);

#[cfg(target_os = "macos")]
struct Caret {
    bounds: Bounds<ScaledPixels>,
    color: Rgba,
    since: std::time::Instant,
    order: crate::scene::DrawOrder,
    painted: bool,
    overlay: bool,
}

fn round_corners(mask: &ContentMask<ScaledPixels>) -> [Bounds<ScaledPixels>; 4] {
    let [x, y, right, bottom] = [
        mask.rounded.origin.x,
        mask.rounded.origin.y,
        mask.rounded.origin.x + mask.rounded.size.width,
        mask.rounded.origin.y + mask.rounded.size.height,
    ];
    let (rx, ry) = (&mask.corner_radii.x, &mask.corner_radii.y);
    [
        (x, y, rx.top_left, ry.top_left),
        (right - rx.top_right, y, rx.top_right, ry.top_right),
        (
            right - rx.bottom_right,
            bottom - ry.bottom_right,
            rx.bottom_right,
            ry.bottom_right,
        ),
        (x, bottom - ry.bottom_left, rx.bottom_left, ry.bottom_left),
    ]
    .map(|(x, y, width, height)| Bounds::new(point(x, y), size(width, height)))
}

pub struct Window {
    pub(crate) id: WindowId,
    pub(crate) platform: platform::Window,
    pub(crate) view: Option<Box<dyn View>>,
    text_system: Rc<TextSystem>,
    pub(crate) scale_factor: f32,
    pub(crate) viewport_size: Size<Pixels>,
    pub(crate) zoom: f32,
    native: (Size<Pixels>, f32),
    tree: Option<Tree>,
    scene: Scene,
    next: Frame,
    rendered: Frame,
    hit_test: Vec<HitboxId>,
    hover_count: usize,
    next_hitbox: u64,
    content_masks: Vec<(ContentMask<Pixels>, ContentMask<ScaledPixels>)>,
    offsets: Vec<Point<Pixels>>,
    pub(crate) font_smoothing: bool,
    hit_transform: Option<Rc<HitTransform>>,
    opacity: f32,
    states: FxHashMap<(ElementId, TypeId), Box<dyn Any>>,
    next_states: FxHashMap<(ElementId, TypeId), Box<dyn Any>>,
    next_frame_callbacks: Vec<FrameCallback>,
    pub(crate) captures: Vec<CaptureCallback>,
    #[cfg(target_os = "macos")]
    caret: Option<Caret>,
    #[cfg(target_os = "macos")]
    caret_covered: bool,
    #[cfg(target_os = "macos")]
    symbols: platform::symbols::Cache,
    pub(crate) dirty: bool,
    redraw: [bool; 2],
    rebuild: bool,
    drawing: bool,
    pub(crate) mouse_position: Point<Pixels>,
    pub(crate) modifiers: Modifiers,
    pub(crate) removed: bool,
    cursor: Option<CursorStyle>,
    drawn_at: std::time::Instant,
    frames_drawn: u64,
    tracks: Vec<(u64, std::time::Instant, f32, Vec<[f32; 16]>)>,
    pub(crate) rails: ([f32; 2], std::time::Instant),
    pub(crate) scroll_chain: Option<Vec<ScrollHandle>>,
    pub(crate) gesture: ScrollGesture,
}

impl Window {
    pub(crate) fn new(
        id: WindowId,
        platform: platform::Window,
        text_system: Rc<TextSystem>,
    ) -> Self {
        let scale_factor = platform.scale_factor();
        let viewport_size = platform.content_size();
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
            next: Frame::default(),
            rendered: Frame::default(),
            hit_test: Vec::new(),
            hover_count: 0,
            next_hitbox: 0,
            content_masks: Vec::new(),
            offsets: Vec::new(),
            font_smoothing: true,
            hit_transform: None,
            opacity: 1.,
            states: FxHashMap::default(),
            next_states: FxHashMap::default(),
            next_frame_callbacks: Vec::new(),
            drawn_at: std::time::Instant::now(),
            frames_drawn: 0,
            tracks: Vec::new(),
            rails: ([0.; 2], std::time::Instant::now()),
            scroll_chain: None,
            gesture: ScrollGesture::default(),
            captures: Vec::new(),
            #[cfg(target_os = "macos")]
            caret: None,
            #[cfg(target_os = "macos")]
            caret_covered: false,
            #[cfg(target_os = "macos")]
            symbols: platform::symbols::Cache::default(),
            dirty: false,
            redraw: [false; 2],
            rebuild: false,
            drawing: false,
            mouse_position: Point::default(),
            modifiers: Modifiers::default(),
            removed: false,
            cursor: None,
        }
    }

    pub fn id(&self) -> WindowId {
        self.id
    }

    pub fn text_system(&self) -> &Rc<TextSystem> {
        &self.text_system
    }

    pub fn scale_factor(&self) -> f32 {
        self.scale_factor
    }

    pub fn viewport_size(&self) -> Size<Pixels> {
        self.viewport_size
    }

    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    pub fn set_zoom(&mut self, zoom: f32) {
        if self.zoom != zoom {
            self.mouse_position = self.mouse_position * (self.zoom / zoom);
            self.zoom = zoom;
            self.mark_resized(self.native.0, self.native.1);
        }
    }

    pub fn refresh(&mut self) {
        if !self.drawing && !self.dirty {
            self.dirty = true;
            self.platform.request_frame();
        }
    }

    pub fn redraw(&mut self) {
        if self.drawing {
            self.redraw[0] = true;
        } else {
            self.refresh();
        }
    }

    pub fn rebuild(&mut self) {
        self.rebuild = true;
    }

    pub fn redrawing(&self) -> bool {
        self.redraw[1]
    }

    pub fn will_draw(&self) -> bool {
        self.drawing || self.dirty || !self.captures.is_empty()
    }

    pub fn patch_effect(&mut self, key: u64, opacity: f32, transform: Option<[f32; 16]>) -> bool {
        let device = transform.map(|matrix| device_matrix(matrix, self.scale_factor));
        if !self.scene.patch(key, opacity.clamp(0., 1.), device) {
            return false;
        }
        if let Some(matrix) = transform {
            self.rendered.turn(key, &matrix);
        }
        true
    }

    pub fn demote(&mut self) {
        #[cfg(target_os = "macos")]
        self.platform.demote();
        self.tracks.clear();
    }

    pub fn promote(
        &mut self,
        key: u64,
        step: f32,
        opacities: &[f32],
        transforms: Option<&[[f32; 16]]>,
    ) -> bool {
        // WARNING: a promoted layer costs a frame to set up, so frames that keep drawing don't promote.
        if self.will_draw() || self.drawn_at.elapsed() < std::time::Duration::from_millis(250) {
            return false;
        }
        let Some(&(_, index)) = self.scene.keyed.iter().find(|(keyed, _)| *keyed == key) else {
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
                .reduce(|all, bounds| all.union(&bounds)),
            None => Some(content),
        };
        let region = crate::frame::region(content);
        // WARNING: the compositor draws the layer over the window without its clip, so it must stay
        // inside the clip and nothing may paint over it there.
        let mask = &effect.content_mask;
        if reach.is_none_or(|reach| {
            mask.bounds.intersect(&reach) != reach
                || round_corners(mask)
                    .iter()
                    .any(|corner| corner.intersects(&reach))
                || self.scene.paints_over(index, reach)
        }) || region[2] > 2048
            || region[3] > 2048
        {
            return false;
        }
        #[cfg(target_os = "macos")]
        {
            let zoom = f64::from(self.zoom);
            let points: Vec<[f64; 16]> = transforms
                .unwrap_or_default()
                .iter()
                .map(|matrix| {
                    let mut matrix = matrix.map(f64::from);
                    for row in [3, 7, 11] {
                        matrix[row] /= zoom;
                    }
                    for column in [12, 13, 14] {
                        matrix[column] *= zoom;
                    }
                    matrix
                })
                .collect();
            self.platform
                .promote(key, index, region, f64::from(step), opacities, &points);
            self.tracks.retain(|track| track.0 != key);
            if let Some(transforms) = transforms {
                self.tracks
                    .push((key, std::time::Instant::now(), step, transforms.to_vec()));
            }
            true
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (step, opacities);
            false
        }
    }

    pub fn present(&mut self) -> bool {
        if self.will_draw() {
            return false;
        }
        // WARNING: the atlas and text caches keep their frame, so the tiles this scene reads stay.
        let _ = self.platform.draw(&self.scene, false);
        true
    }

    pub fn on_next_frame(&mut self, callback: impl FnOnce(&mut Window, &mut App) + 'static) {
        self.next_frame_callbacks.push(Box::new(callback));
        self.platform.request_frame();
    }

    pub fn frames_drawn(&self) -> u64 {
        self.frames_drawn
    }

    pub fn request_animation_frame(&mut self) {
        self.on_next_frame(|window, _| window.refresh());
    }

    pub fn capture(&mut self, done: impl FnOnce(Result<([u32; 2], Vec<u8>)>, &mut App) + 'static) {
        self.captures.push(Box::new(done));
        self.dirty = true;
        self.platform.force_frame();
    }

    pub fn timer(
        &self,
        delay: std::time::Duration,
        f: impl FnOnce(&mut Window, &mut App) + 'static,
    ) -> crate::Timer {
        let id = self.id;
        crate::app::timer(delay, move |cx| {
            cx.update_window(id, f);
        })
    }

    pub fn remove_window(&mut self) {
        self.removed = true;
    }

    pub fn is_window_active(&self) -> bool {
        self.platform.is_active()
    }

    pub fn window_state(&self) -> WindowState {
        self.platform.state()
    }

    pub fn change_window(&mut self, change: WindowChange) {
        self.platform.change(change);
    }

    pub fn control(&self) -> WindowControl {
        #[cfg(target_os = "macos")]
        return self.platform.control().clone();
        #[cfg(not(target_os = "macos"))]
        WindowControl(self.id)
    }

    pub fn is_fullscreen(&self) -> bool {
        self.platform.state().fullscreen
    }

    pub fn toggle_fullscreen(&self) {
        self.platform.toggle_fullscreen();
    }

    pub fn zoom_window(&self) {
        self.platform.zoom();
    }

    pub fn set_window_title(&mut self, title: &str) {
        self.platform.set_title(title);
    }

    pub fn start_window_move(&self) {
        self.platform.start_window_move();
    }

    pub fn titlebar_double_click(&self) {
        self.platform.titlebar_double_click();
    }

    pub(crate) fn mark_resized(&mut self, size: Size<Pixels>, scale_factor: f32) {
        self.native = (size, scale_factor);
        self.viewport_size = size / self.zoom;
        self.scale_factor = scale_factor * self.zoom;
        self.dirty = false;
        self.refresh();
    }

    pub(crate) fn take_next_frame_callbacks(&mut self) -> Vec<FrameCallback> {
        mem::take(&mut self.next_frame_callbacks)
    }

    pub(crate) fn wants_frame(&self) -> bool {
        self.dirty || !self.next_frame_callbacks.is_empty()
    }

    // WARNING: a caret that blinks by drawing keeps about 126 MB of Metal driver memory alive and wakes the GPU twice
    // a second. Core Animation can blink a box alone, but it draws over the window, so the box must be at the top
    // level, clear of round corners, with nothing painted over it. Otherwise this returns false, and the caller
    // draws the blink. `painted` says that the caller then paints the caret in this frame.
    pub fn overlay_caret(
        &mut self,
        bounds: Bounds<Pixels>,
        color: Rgba,
        since: std::time::Instant,
        painted: bool,
    ) -> bool {
        #[cfg(target_os = "macos")]
        {
            let mask = self.snapped_content_mask();
            let visible = self.snap_bounds(bounds).intersect(&mask.bounds);
            if !self.scene.at_root()
                || self.opacity < 1.
                || !self.captures.is_empty()
                || round_corners(&mask)
                    .iter()
                    .any(|corner| corner.intersects(&visible))
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
                order: self.scene.probe(visible),
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

    pub(crate) fn draw(&mut self, cx: &mut App) {
        #[cfg(target_os = "macos")]
        self.platform.demote();
        self.tracks.clear();
        self.drawn_at = std::time::Instant::now();
        self.frames_drawn += 1;
        loop {
            self.dirty = false;
            self.drawing = true;
            self.next.clear();
            self.scene.clear();
            #[cfg(target_os = "macos")]
            {
                self.caret = None;
            }
            self.next_hitbox = 0;
            if self.redraw[1] {
                // WARNING: the first pass moved its states out of `states`, so they go back for this one.
                let states = mem::take(&mut self.next_states);
                self.states.extend(states);
            } else {
                self.next_states.clear();
            }
            if let Some(mut view) = self.view.take() {
                let mut tree = self.tree.take().unwrap_or_default();
                let root = self.build(&mut tree, cx, |tree, window, cx| {
                    view.render(tree, window, cx)
                });
                self.view = Some(view);
                if let Some(root) = root {
                    tree.layout(root, self.viewport_size, self);
                    tree.prepaint(root, self);
                    self.hover_count = self.next.hit_test(self.mouse_position, &mut self.hit_test);
                    // WARNING: a pass that draws again replaces this scene, so it is not painted.
                    if !self.redraw[0] || self.redraw[1] {
                        tree.paint(root, self);
                    }
                }
                self.tree = Some(tree);
            }
            self.drawing = false;
            let again = mem::take(&mut self.redraw[0]);
            if again && !self.redraw[1] {
                self.redraw[1] = true;
                continue;
            }
            self.redraw[1] = false;
            if again {
                self.refresh();
            }
            break;
        }
        mem::swap(&mut self.rendered, &mut self.next);
        mem::swap(&mut self.states, &mut self.next_states);
        #[cfg(target_os = "macos")]
        {
            let caret = self.caret.take();
            let covered = caret.as_ref().is_some_and(|caret| {
                self.scene
                    .covers(caret.order + u32::from(caret.painted), caret.bounds)
            });
            // WARNING: this frame left a covered caret out, so the next one has to paint it.
            if covered && caret.as_ref().is_some_and(|caret| caret.overlay) {
                self.refresh();
            }
            self.caret_covered = covered;
            self.platform.set_caret(
                caret
                    .filter(|caret| caret.overlay && !covered)
                    .map(|caret| (caret.bounds, caret.color, caret.since)),
            );
        }
        self.scene.finish();
        let captures = mem::take(&mut self.captures);
        #[cfg(target_os = "macos")]
        self.platform
            .set_drawable_size(self.native.0, self.native.1);
        let mut frame = self.platform.draw(&self.scene, !captures.is_empty());
        self.text_system.finish_frame();
        self.platform.atlas().finish_frame();
        let last = captures.len().wrapping_sub(1);
        for (index, capture) in captures.into_iter().enumerate() {
            capture(
                match &mut frame {
                    Some(Ok(frame)) if index == last => Ok(mem::take(frame)),
                    Some(Ok(frame)) => Ok(frame.clone()),
                    Some(Err(error)) => Err(anyhow::anyhow!("{error:#}")),
                    None => Err(anyhow::anyhow!("The frame was not drawn")),
                },
                cx,
            );
        }
        self.update_cursor(cx);
    }

    fn build(
        &mut self,
        tree: &mut Tree,
        cx: &mut App,
        mut render: impl FnMut(&Tree, &mut Self, &mut App) -> Child,
    ) -> Option<taffy::NodeId> {
        loop {
            tree.begin(self.scale_factor, self.text_system.generation());
            let child = render(tree, self, cx);
            let root = tree.commit_root(child, self.viewport_size, self.scale_factor);
            if !mem::take(&mut self.rebuild) {
                return root;
            }
        }
    }

    pub fn layout(
        &mut self,
        cx: &mut App,
        mut render: impl FnMut(&Tree, &mut Self, &mut App) -> Child,
    ) -> bool {
        if self.drawing {
            return false;
        }
        self.drawing = true;
        let mut again = false;
        for _ in 0..2 {
            self.next.clear();
            self.next_hitbox = 0;
            let mut tree = self.tree.take().unwrap_or_default();
            if let Some(root) = self.build(&mut tree, cx, &mut render) {
                tree.layout(root, self.viewport_size, self);
                tree.prepaint(root, self);
            }
            self.tree = Some(tree);
            // WARNING: the next draw starts from `states`, so states taken by this pass go back there.
            let states = mem::take(&mut self.next_states);
            self.states.extend(states);
            again = mem::take(&mut self.redraw[0]);
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

    fn update_cursor(&mut self, cx: &mut App) {
        let style = self
            .rendered
            .cursors
            .iter()
            .rev()
            .find(|(hitbox, _)| hitbox.is_hovered(self))
            .map_or(CursorStyle::Arrow, |(_, style)| *style);
        if self.cursor != Some(style) && self.platform.is_active() {
            self.cursor = Some(style);
            cx.platform.set_cursor(style);
        }
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn in_drag_area(&self) -> bool {
        self.hit_test
            .first()
            .is_some_and(|top| self.rendered.drag_areas.contains(top))
    }

    pub fn dispatch_event(&mut self, event: PlatformInput, cx: &mut App) -> bool {
        if let Some(position) = event.mouse_position() {
            self.mouse_position = position;
        }
        match &event {
            PlatformInput::MouseDown(event) => self.modifiers = event.modifiers,
            PlatformInput::MouseUp(event) => self.modifiers = event.modifiers,
            PlatformInput::MouseMove(event) => self.modifiers = event.modifiers,
            PlatformInput::ScrollWheel(event) => {
                self.modifiers = event.modifiers;
                self.scroll_chain = None;
                self.gesture.edge = None;
                if let ScrollDelta::Pixels(delta) = event.delta {
                    let now = std::time::Instant::now();
                    let elapsed = now - self.rails.1;
                    let paused = elapsed > std::time::Duration::from_millis(100);
                    let [x, y] = if paused {
                        [0.; 2]
                    } else {
                        self.rails.0.map(|value| value * 0.8)
                    };
                    self.rails = (
                        [x + f32::from(delta.x).abs(), y + f32::from(delta.y).abs()],
                        now,
                    );
                    if paused {
                        self.gesture.speed = Point::default();
                    } else if delta != Point::default() {
                        let seconds = elapsed.as_secs_f32().max(1. / 120.);
                        let speed =
                            |old: f32, delta: Pixels| (old + f32::from(delta) / seconds) / 2.;
                        self.gesture.speed = point(
                            speed(self.gesture.speed.x, delta.x),
                            speed(self.gesture.speed.y, delta.y),
                        );
                    }
                    let phase = event.phase;
                    if phase == ScrollPhase::Began || phase == ScrollPhase::None && paused {
                        if let Some(handle) = self.gesture.latch.take() {
                            handle.release(Point::default(), self);
                        }
                        self.gesture.spent = false;
                    }
                    if matches!(phase, ScrollPhase::Ended | ScrollPhase::MomentumEnded)
                        && let Some(handle) = self.gesture.latch.clone()
                        && handle.release(self.gesture.speed, self)
                    {
                        self.gesture.spent = true;
                    }
                    if phase == ScrollPhase::MomentumEnded {
                        self.gesture.latch = None;
                    }
                } else if let Some(handle) = self.gesture.latch.take() {
                    handle.release(Point::default(), self);
                }
            }
            PlatformInput::ModifiersChanged(event) => self.modifiers = event.modifiers,
            PlatformInput::KeyDown(event) => self.modifiers = event.keystroke.modifiers,
            PlatformInput::KeyUp(event) => self.modifiers = event.keystroke.modifiers,
            _ => {}
        }
        cx.propagate_event = true;
        let mouse = matches!(
            event,
            PlatformInput::MouseDown(_)
                | PlatformInput::MouseUp(_)
                | PlatformInput::MouseMove(_)
                | PlatformInput::MouseExited(_)
                | PlatformInput::ScrollWheel(_)
        );
        if mouse {
            // WARNING: the compositor turns promoted layers without frames, so hit testing takes their
            // transform of now.
            for (key, start, step, transforms) in &self.tracks {
                let at = (start.elapsed().as_secs_f32() / step).round() as usize;
                if let Some(matrix) = transforms.get(at).or(transforms.last()) {
                    self.rendered.turn(*key, matrix);
                }
            }
            self.hover_count = self
                .rendered
                .hit_test(self.mouse_position, &mut self.hit_test);
            self.update_cursor(cx);
            let mut listeners = mem::take(&mut self.rendered.mouse_listeners);
            for listener in listeners.iter_mut().flatten() {
                listener(&event, DispatchPhase::Capture, self, cx);
                if !cx.propagate_event {
                    break;
                }
            }
            if cx.propagate_event {
                for listener in listeners.iter_mut().rev().flatten() {
                    listener(&event, DispatchPhase::Bubble, self, cx);
                    if !cx.propagate_event {
                        break;
                    }
                }
            }
            if self.rendered.mouse_listeners.is_empty() {
                self.rendered.mouse_listeners = listeners;
            }
            if let (PlatformInput::ScrollWheel(event), Some((handle, rest))) =
                (&event, self.gesture.edge.take())
            {
                self.overscroll(handle, rest, event.phase);
            }
        } else if let Some(mut view) = self.view.take() {
            view.event(&event, self, cx);
            if self.view.is_none() {
                self.view = Some(view);
            }
        }
        !cx.propagate_event
    }

    pub(crate) fn overscroll(
        &mut self,
        handle: ScrollHandle,
        rest: Point<Pixels>,
        phase: ScrollPhase,
    ) {
        handle.stretch_by(rest);
        self.gesture.edge = None;
        self.gesture.latch = Some(handle.clone());
        match phase {
            ScrollPhase::Momentum => {
                self.gesture.spent = true;
                handle.release(self.gesture.speed, self);
            }
            ScrollPhase::None => handle.loosen(self),
            _ => {}
        }
        self.refresh();
    }

    pub fn scroll_only(&mut self, chain: Vec<ScrollHandle>) {
        self.scroll_chain = Some(chain);
    }

    pub fn dispatch_keystroke(&mut self, keystroke: Keystroke, cx: &mut App) -> bool {
        let handled = self.dispatch_event(
            PlatformInput::KeyDown(KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
            }),
            cx,
        );
        if !handled && let Some(text) = &keystroke.key_char {
            self.with_view(cx, |view, window, cx| {
                view.replace_text_in_range(None, text, window, cx)
            });
        }
        self.dispatch_event(PlatformInput::KeyUp(KeyUpEvent { keystroke }), cx);
        handled
    }

    pub(crate) fn with_view<R>(
        &mut self,
        cx: &mut App,
        f: impl FnOnce(&mut dyn View, &mut Window, &mut App) -> R,
    ) -> Option<R> {
        let mut view = self.view.take()?;
        let result = f(view.as_mut(), self, cx);
        if self.view.is_none() {
            self.view = Some(view);
        }
        Some(result)
    }

    pub fn insert_hitbox(&mut self, bounds: Bounds<Pixels>, behavior: HitboxBehavior) -> Hitbox {
        let id = HitboxId(self.next_hitbox);
        self.next_hitbox += 1;
        let hitbox = Hitbox {
            id,
            bounds,
            content_mask: self.content_mask(),
            behavior,
            transform: self.hit_transform.clone(),
        };
        self.next.hitboxes.push(hitbox.clone());
        hitbox
    }

    pub fn insert_window_drag_hitbox(&mut self, hitbox: &Hitbox) {
        self.next.drag_areas.push(hitbox.id);
    }

    pub fn set_cursor_style(&mut self, style: CursorStyle, hitbox: &Hitbox) {
        self.next.cursors.push((hitbox.id, style));
    }

    pub fn on_mouse_event<E: MouseEvent>(
        &mut self,
        mut listener: impl FnMut(&E, DispatchPhase, &mut Window, &mut App) + 'static,
    ) {
        self.next
            .mouse_listeners
            .push(Some(Box::new(move |event, phase, window, cx| {
                if let Some(event) = E::from_input(event) {
                    listener(event, phase, window, cx);
                }
            })));
    }

    pub fn with_element_state<S: 'static, R>(
        &mut self,
        id: ElementId,
        f: impl FnOnce(Option<S>, &mut Self) -> (R, S),
    ) -> R {
        let key = (id, TypeId::of::<S>());
        let mut slot = self
            .next_states
            .remove(&key)
            .or_else(|| self.states.remove(&key))
            .and_then(|state| state.downcast::<Option<S>>().ok())
            .unwrap_or_default();
        let (result, state) = f(slot.take(), self);
        *slot = Some(state);
        self.next_states.insert(key, slot);
        result
    }

    pub fn font_smoothing(&self) -> bool {
        self.font_smoothing
    }

    pub fn content_mask(&self) -> ContentMask<Pixels> {
        self.content_masks.last().map_or_else(
            || ContentMask::from(Bounds::new(Point::default(), self.viewport_size)),
            |(mask, _)| *mask,
        )
    }

    fn push_content_mask(&mut self, mask: ContentMask<Pixels>) {
        let snapped = self.snap_mask(mask);
        self.content_masks.push((mask, snapped));
    }

    pub fn with_content_mask<R>(
        &mut self,
        mask: Option<ContentMask<Pixels>>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let Some(mask) = mask else {
            return f(self);
        };
        self.push_content_mask(mask.intersect(&self.content_mask()));
        let result = f(self);
        self.content_masks.pop();
        result
    }

    pub fn element_offset(&self) -> Point<Pixels> {
        self.offsets.last().copied().unwrap_or_default()
    }

    pub fn with_element_offset<R>(
        &mut self,
        offset: Point<Pixels>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        if offset == Point::default() {
            return f(self);
        }
        let offset = self.element_offset() + offset;
        self.with_absolute_element_offset(offset, f)
    }

    pub fn with_absolute_element_offset<R>(
        &mut self,
        offset: Point<Pixels>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.offsets.push(offset);
        let result = f(self);
        self.offsets.pop();
        result
    }

    pub(crate) fn with_element_opacity<R>(
        &mut self,
        opacity: Option<f32>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let Some(opacity) = opacity else {
            return f(self);
        };
        let previous = self.opacity;
        self.opacity = previous * opacity;
        let result = f(self);
        self.opacity = previous;
        result
    }

    pub fn transformed_bounds(&self, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        let Some(transform) = &self.hit_transform else {
            return bounds;
        };
        let corners = [
            bounds.origin,
            bounds.top_right(),
            bounds.bottom_left(),
            bounds.bottom_right(),
        ]
        .map(|corner| HitTransform::map(&transform.forward.get(), corner));
        if corners.iter().any(Option::is_none) {
            return bounds;
        }
        let corners = corners.map(Option::unwrap);
        let min = corners.iter().fold(corners[0], |min, p| min.min(*p));
        let max = corners.iter().fold(corners[0], |max, p| max.max(*p));
        Bounds::from_corners(min, max)
    }

    pub fn with_hit_transform<R>(&mut self, m: [f32; 16], f: impl FnOnce(&mut Self) -> R) -> R {
        self.with_keyed_hit_transform(m, None, f)
    }

    pub fn with_keyed_hit_transform<R>(
        &mut self,
        m: [f32; 16],
        key: Option<u64>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let local = plane(&m);
        let outer = self.hit_transform.clone();
        let forward = match &outer {
            Some(outer) => mul3(&outer.forward.get(), &local),
            None => local,
        };
        let Some(inverse) = invert3(&forward) else {
            return f(self);
        };
        let window_mask = outer
            .as_ref()
            .map_or_else(|| self.content_mask().bounds, |outer| outer.window_mask);
        self.hit_transform = Some(Rc::new(HitTransform {
            local: local.into(),
            key,
            outer,
            forward: forward.into(),
            inverse: inverse.into(),
            window_mask,
        }));
        self.push_content_mask(unbounded_mask());
        let result = f(self);
        self.content_masks.pop();
        self.hit_transform = self.hit_transform.take().and_then(|own| own.outer.clone());
        result
    }

    fn snap_bounds(&self, bounds: Bounds<Pixels>) -> Bounds<ScaledPixels> {
        let scale = self.scale_factor;
        let left = round_to_device_pixel(bounds.left().0, scale);
        let top = round_to_device_pixel(bounds.top().0, scale);
        let right = round_to_device_pixel(bounds.right().0, scale).max(left);
        let bottom = round_to_device_pixel(bounds.bottom().0, scale).max(top);
        Bounds::from_corners(
            point(ScaledPixels(left), ScaledPixels(top)),
            point(ScaledPixels(right), ScaledPixels(bottom)),
        )
    }

    fn snap_stroke(&self, value: Pixels) -> ScaledPixels {
        ScaledPixels(round_stroke_to_device_pixel(value.0, self.scale_factor))
    }

    fn cover_bounds(&self, bounds: Bounds<Pixels>) -> Bounds<ScaledPixels> {
        let scale = self.scale_factor;
        let left = (bounds.left().0 * scale).floor();
        let top = (bounds.top().0 * scale).floor();
        let right = (bounds.right().0 * scale).ceil().max(left);
        let bottom = (bounds.bottom().0 * scale).ceil().max(top);
        Bounds::from_corners(
            point(ScaledPixels(left), ScaledPixels(top)),
            point(ScaledPixels(right), ScaledPixels(bottom)),
        )
    }

    fn snapped_content_mask(&self) -> ContentMask<ScaledPixels> {
        match self.content_masks.last() {
            Some((_, snapped)) => *snapped,
            None => self.snap_mask(self.content_mask()),
        }
    }

    fn snap_mask(&self, mask: ContentMask<Pixels>) -> ContentMask<ScaledPixels> {
        ContentMask {
            bounds: self.cover_bounds(mask.bounds),
            rounded: mask.rounded.scale(self.scale_factor),
            corner_radii: mask.corner_radii.scale(self.scale_factor),
        }
    }

    pub fn paint_layer<R>(&mut self, bounds: Bounds<Pixels>, f: impl FnOnce(&mut Self) -> R) -> R {
        let clipped = bounds.intersect(&self.content_mask().bounds);
        if !clipped.is_empty() {
            let bounds = self.cover_bounds(clipped);
            self.scene.push_layer(bounds);
        }
        let result = f(self);
        if !clipped.is_empty() {
            self.scene.pop_layer();
        }
        result
    }

    pub fn paint_quad(&mut self, quad: PaintQuad) {
        let opacity = self.opacity;
        let quad = Quad {
            order: 0,
            border_style: quad.border_style as u32 | u32::from(quad.border_sides) << 8,
            bounds: self.snap_bounds(quad.bounds),
            content_mask: self.snapped_content_mask(),
            background: quad.background.opacity(opacity),
            border_color: quad.border_color.opacity(opacity),
            corner_radii: quad.corner_radii.scale(self.scale_factor),
            border_widths: quad.border_widths.map(|width| self.snap_stroke(width)),
        };
        if !quad.background.is_transparent() {
            self.scene.insert(Primitive::Quad(quad));
            return;
        }
        let outer = quad.bounds;
        let inner = largest_border_interior(&quad);
        if inner.is_empty() {
            self.scene.insert(Primitive::Quad(quad));
            return;
        }
        for strip in [
            Bounds::from_corners(outer.origin, point(outer.right(), inner.top())),
            Bounds::from_corners(point(outer.left(), inner.bottom()), outer.bottom_right()),
            Bounds::from_corners(point(outer.left(), inner.top()), inner.bottom_left()),
            Bounds::from_corners(inner.top_right(), point(outer.right(), inner.bottom())),
        ] {
            let bounds = quad.content_mask.bounds.intersect(&strip);
            if !bounds.is_empty() {
                self.scene.insert(Primitive::Quad(Quad {
                    content_mask: ContentMask {
                        bounds,
                        ..quad.content_mask
                    },
                    ..quad
                }));
            }
        }
    }

    pub fn paint_drop_shadows(
        &mut self,
        bounds: Bounds<Pixels>,
        corner_radii: Radii<Pixels>,
        shadows: &[BoxShadow],
    ) {
        self.paint_shadows(bounds, corner_radii, shadows, false);
    }

    pub fn paint_inset_shadows(
        &mut self,
        bounds: Bounds<Pixels>,
        corner_radii: Radii<Pixels>,
        shadows: &[BoxShadow],
    ) {
        self.paint_shadows(bounds, corner_radii, shadows, true);
    }

    fn paint_shadows(
        &mut self,
        bounds: Bounds<Pixels>,
        corner_radii: Radii<Pixels>,
        shadows: &[BoxShadow],
        inset: bool,
    ) {
        let scale = self.scale_factor;
        let content_mask = self.snapped_content_mask();
        let element_bounds = self.cover_bounds(bounds);
        let element_corner_radii = corner_radii.scale(scale);
        for shadow in shadows.iter().filter(|shadow| shadow.inset == inset) {
            let (shape, radii) = if inset {
                let spread = shadow.spread_radius;
                (
                    (bounds + shadow.offset).dilate(-spread),
                    corner_radii.inset(spread, spread, spread, spread),
                )
            } else {
                let spread = f32::from(shadow.spread_radius);
                let grow = |radius: Pixels| {
                    let radius = f32::from(radius);
                    Pixels::from(if radius <= 0. || spread == 0. {
                        radius
                    } else if spread < 0. || radius >= spread {
                        (radius + spread).max(0.)
                    } else {
                        radius + spread * (1. + (radius / spread - 1.).powi(3))
                    })
                };
                (
                    (bounds + shadow.offset).dilate(shadow.spread_radius),
                    Radii {
                        x: corner_radii.x.map(grow),
                        y: corner_radii.y.map(grow),
                    },
                )
            };
            self.scene.insert(Primitive::Shadow(Shadow {
                order: 0,
                blur_radius: shadow.blur_radius.scale(scale),
                bounds: self.cover_bounds(shape),
                corner_radii: radii.scale(scale),
                content_mask,
                color: shadow.color.opacity(self.opacity),
                element_bounds,
                element_corner_radii,
                inset: inset as u32,
                pad: 0,
            }));
        }
    }

    pub fn paint_effect<R>(&mut self, effect: PaintEffect, f: impl FnOnce(&mut Self) -> R) -> R {
        let scale = self.scale_factor;
        let content_mask = self.content_mask().scale(scale);
        let mask = effect.mask.and_then(|(image, bounds)| {
            let tile = self.image_tile(&image)?;
            Some((tile, bounds.scale(scale)))
        });
        let state = EffectState {
            transform: effect.transform.map(|matrix| device_matrix(matrix, scale)),
            opacity: effect.opacity.clamp(0., 1.),
            color_matrix: effect.color_matrix,
            blur: effect.blur.0 * scale,
            shadow: effect.shadow.map(|(offset, blur, color)| {
                (
                    point(offset.x.0 * scale, offset.y.0 * scale),
                    blur.0 * scale,
                    color,
                )
            }),
            blend_mode: effect.blend_mode,
            clip: effect.clip.map(|(bounds, radii)| {
                (
                    bounds.scale(scale),
                    radii.clamp_radii_for_quad_size(bounds.size).scale(scale),
                )
            }),
            mask,
            backdrop: effect.backdrop.map(
                |Backdrop {
                     bounds,
                     corner_radii,
                     blur,
                     color_matrix,
                 }| BackdropState {
                    bounds: bounds.scale(scale),
                    corner_radii: corner_radii
                        .clamp_radii_for_quad_size(bounds.size)
                        .scale(scale),
                    blur: blur.0 * scale,
                    color_matrix,
                },
            ),
            glass: effect.glass.map(|glass| crate::scene::GlassState {
                key: glass.key,
                bounds: glass.bounds.scale(scale),
                corner_radius: glass.corner_radius.0 * scale,
                tint: glass.tint,
                clear: glass.clear,
                interactive: glass.interactive,
            }),
            content_mask,
        };
        self.scene.push_effect(state, effect.key);
        // WARNING: ancestor clips apply after the effect, in window space, so the layer starts unclipped.
        self.push_content_mask(unbounded_mask());
        let result = f(self);
        self.content_masks.pop();
        self.scene.pop_effect();
        result
    }

    pub fn paint_underline(
        &mut self,
        origin: Point<Pixels>,
        width: Pixels,
        style: &UnderlineStyle,
    ) {
        let line = self.underline(origin, width, style);
        self.paint_decoration(line, style.style);
    }

    pub fn paint_strikethrough(
        &mut self,
        origin: Point<Pixels>,
        width: Pixels,
        style: &StrikethroughStyle,
    ) {
        let line = self.underline(
            origin,
            width,
            &UnderlineStyle {
                thickness: style.thickness,
                color: style.color,
                style: style.style,
                offset: None,
            },
        );
        self.paint_decoration(line, style.style);
    }

    fn underline(&self, origin: Point<Pixels>, width: Pixels, style: &UnderlineStyle) -> Underline {
        let thickness = self.snap_stroke(style.thickness);
        let height = match style.style {
            DecorationStyle::Wavy | DecorationStyle::Double => ScaledPixels(thickness.0 * 3.),
            _ => thickness,
        };
        Underline {
            order: 0,
            wavy: (style.style == DecorationStyle::Wavy) as u32,
            bounds: Bounds {
                origin: origin
                    .map(|value| ScaledPixels(round_to_device_pixel(value.0, self.scale_factor))),
                size: size(self.snap_stroke(width), height),
            },
            content_mask: self.snapped_content_mask(),
            color: style.color.unwrap_or_default().opacity(self.opacity),
            thickness,
            pad: 0,
        }
    }

    fn paint_decoration(&mut self, line: Underline, style: DecorationStyle) {
        let thickness = line.thickness.0;
        let mut second = None;
        let mut first = line;
        if style == DecorationStyle::Double {
            first.bounds.size.height = line.thickness;
            let mut next = first;
            next.bounds.origin.y = ScaledPixels(first.bounds.origin.y.0 + thickness * 2.);
            second = Some(next);
        }
        let dash = match style {
            DecorationStyle::Dotted => thickness,
            DecorationStyle::Dashed => thickness * 2.,
            _ => 0.,
        };
        for line in [Some(first), second].into_iter().flatten() {
            let bounds = line.bounds.intersect(&line.content_mask.bounds);
            if bounds.is_empty() {
                continue;
            }
            if dash <= 0. {
                self.scene.insert(Primitive::Underline(line));
                continue;
            }
            let mut start = line.bounds.left().0;
            while start < bounds.right().0 {
                let end = (start + dash).min(bounds.right().0);
                let left = start.max(bounds.left().0);
                if left < end {
                    self.scene.insert(Primitive::Underline(Underline {
                        content_mask: ContentMask {
                            bounds: Bounds::from_corners(
                                point(ScaledPixels(left), bounds.top()),
                                point(ScaledPixels(end), bounds.bottom()),
                            ),
                            ..line.content_mask
                        },
                        ..line
                    }));
                }
                start += dash * 2.;
            }
        }
    }

    pub fn paint_glyph(
        &mut self,
        origin: Point<Pixels>,
        font_id: FontId,
        glyph_id: GlyphId,
        font_size: Pixels,
        color: Rgba,
        smoothing: bool,
    ) {
        let scale = self.scale_factor;
        let origin = origin.scale(scale);
        let variants = SUBPIXEL_VARIANTS_X as f32;
        // WARNING: like Skia, halves round up, so a baseline at y.5 draws on the same row as in Chromium.
        let quantized = point(
            (origin.x.0 * variants + 0.5).floor() / variants,
            (origin.y.0 + 0.5).floor(),
        );
        let subpixel_variant = point((quantized.x.fract() * variants) as u8, 0);
        let params = RenderGlyphParams {
            font_id,
            glyph_id,
            font_size,
            subpixel_variant,
            scale_factor: scale,
            is_emoji: false,
            dilation: if smoothing {
                self.text_system.platform.glyph_dilation_for_color(color)
            } else {
                0
            },
        };
        let Some((tile, raster)) = self.glyph_tile(params) else {
            return;
        };
        let bounds = Bounds {
            origin: quantized.map(|value| ScaledPixels(value.trunc()))
                + raster.map(ScaledPixels::from),
            size: tile.bounds.size.map(ScaledPixels::from),
        };
        self.scene.insert(Primitive::Mono(MonochromeSprite {
            order: 0,
            pad: 0,
            tile,
            bounds,
            content_mask: self.snapped_content_mask(),
            color: color.opacity(self.opacity),
        }));
    }

    pub fn paint_emoji(
        &mut self,
        origin: Point<Pixels>,
        font_id: FontId,
        glyph_id: GlyphId,
        font_size: Pixels,
    ) {
        let scale = self.scale_factor;
        let origin = origin
            .scale(scale)
            .map(|value| ScaledPixels(round_half_toward_zero(value.0)));
        let params = RenderGlyphParams {
            font_id,
            glyph_id,
            font_size,
            subpixel_variant: Point::default(),
            scale_factor: scale,
            is_emoji: true,
            dilation: 0,
        };
        let Some((tile, raster)) = self.glyph_tile(params) else {
            return;
        };
        self.scene.insert(Primitive::Poly(PolychromeSprite {
            order: 0,
            grayscale: 0,
            tile,
            opacity: self.opacity,
            pad: 0,
            bounds: Bounds {
                origin: origin + raster.map(ScaledPixels::from),
                size: tile.bounds.size.map(ScaledPixels::from),
            },
            content_mask: self.snapped_content_mask(),
            corner_radii: Radii::default(),
        }));
    }

    fn glyph_tile(
        &self,
        params: RenderGlyphParams,
    ) -> Option<(crate::scene::AtlasTile, Point<DevicePixels>)> {
        let text = &self.text_system;
        self.platform
            .atlas()
            .get_or_insert_with(&AtlasKey::Glyph(params), &mut || {
                Ok(text
                    .rasterize_glyph(&params)?
                    .map(|(origin, size, bytes)| (origin, size, Cow::Owned(bytes))))
            })
            .ok()
            .flatten()
    }

    fn image_tile(&self, image: &RenderImage) -> Option<crate::scene::AtlasTile> {
        self.platform
            .atlas()
            .get_or_insert_with(&AtlasKey::Image(image.id), &mut || {
                Ok(Some((
                    Point::default(),
                    image.size(),
                    Cow::Borrowed(image.bytes()),
                )))
            })
            .ok()
            .flatten()
            .map(|(tile, _)| tile)
    }

    pub fn paint_symbol(&mut self, name: &str, bounds: Bounds<Pixels>, color: Rgba) {
        #[cfg(target_os = "macos")]
        {
            if bounds.is_empty() || color.a <= 0. {
                return;
            }
            let bounds = self.snap_bounds(bounds);
            let width = bounds.size.width.0;
            let height = bounds.size.height.0;
            if !width.is_finite() || !height.is_finite() || width <= 0. || height <= 0. {
                return;
            }
            let scale = (1024. / width.max(height)).min(1.);
            let size = [
                (width * scale).ceil() as u16,
                (height * scale).ceil() as u16,
            ];
            let image = self.symbols.get(name, size);
            for id in self.symbols.retired.drain(..) {
                self.platform.atlas().remove(&AtlasKey::Symbol(id));
            }
            let Some(image) = image else { return };
            let tile = self
                .platform
                .atlas()
                .get_or_insert_with(&AtlasKey::Symbol(image.id), &mut || {
                    Ok(Some((
                        Point::default(),
                        crate::size(
                            DevicePixels(i32::from(image.size[0])),
                            DevicePixels(i32::from(image.size[1])),
                        ),
                        Cow::Borrowed(&image.bytes),
                    )))
                })
                .ok()
                .flatten();
            if let Some((tile, _)) = tile {
                self.scene.insert(Primitive::Mono(MonochromeSprite {
                    order: 0,
                    pad: 0,
                    tile,
                    bounds,
                    content_mask: self.snapped_content_mask(),
                    color: color.opacity(self.opacity),
                }));
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (name, bounds, color);
    }

    pub fn paint_image(
        &mut self,
        bounds: Bounds<Pixels>,
        image_bounds: Bounds<Pixels>,
        corner_radii: impl Into<Radii<Pixels>>,
        image: Arc<RenderImage>,
        grayscale: bool,
    ) {
        let visible = bounds.intersect(&image_bounds);
        if visible.is_empty() || image_bounds.is_empty() {
            return;
        }
        let Some(tile) = self.image_tile(&image) else {
            return;
        };
        let tile = if visible == image_bounds {
            tile
        } else {
            let ratio = |a: Pixels, b: Pixels| a / b;
            let [x, y, width, height] = [
                ratio(
                    visible.origin.x - image_bounds.origin.x,
                    image_bounds.size.width,
                ),
                ratio(
                    visible.origin.y - image_bounds.origin.y,
                    image_bounds.size.height,
                ),
                ratio(visible.size.width, image_bounds.size.width),
                ratio(visible.size.height, image_bounds.size.height),
            ];
            let origin = tile.bounds.origin;
            let full = tile.bounds.size;
            let left = (origin.x.0 + (x * full.width.0 as f32).round() as i32)
                .clamp(origin.x.0, origin.x.0 + full.width.0);
            let top = (origin.y.0 + (y * full.height.0 as f32).round() as i32)
                .clamp(origin.y.0, origin.y.0 + full.height.0);
            let width = ((width * full.width.0 as f32).round() as i32)
                .min(origin.x.0 + full.width.0 - left)
                .max(0);
            let height = ((height * full.height.0 as f32).round() as i32)
                .min(origin.y.0 + full.height.0 - top)
                .max(0);
            crate::scene::AtlasTile {
                bounds: Bounds::new(
                    point(DevicePixels(left), DevicePixels(top)),
                    size(DevicePixels(width), DevicePixels(height)),
                ),
                ..tile
            }
        };
        let corner_radii = corner_radii
            .into()
            .clamp_radii_for_quad_size(visible.size)
            .scale(self.scale_factor);
        self.scene.insert(Primitive::Poly(PolychromeSprite {
            order: 0,
            grayscale: grayscale as u32,
            tile,
            opacity: self.opacity,
            pad: 0,
            bounds: self.snap_bounds(visible),
            content_mask: self.snapped_content_mask(),
            corner_radii,
        }));
    }

    pub fn drop_image(&mut self, image: Arc<RenderImage>) {
        self.platform.atlas().remove(&AtlasKey::Image(image.id));
    }
}

impl raw_window_handle::HasWindowHandle for Window {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        let raw = self
            .platform
            .raw_window_handle()
            .map_err(|_| raw_window_handle::HandleError::Unavailable)?;
        // SAFETY: the native window lives as long as this window.
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(raw) })
    }
}

fn unbounded_mask() -> ContentMask<Pixels> {
    ContentMask::from(Bounds::new(
        point(px(-1e6), px(-1e6)),
        size(px(2e6), px(2e6)),
    ))
}

fn largest_border_interior(quad: &Quad) -> Bounds<ScaledPixels> {
    let radii = &quad.corner_radii;
    let widths = &quad.border_widths;
    let edge = Edges {
        top: radii.y.top_left.max(radii.y.top_right),
        right: radii.x.top_right.max(radii.x.bottom_right),
        bottom: radii.y.bottom_left.max(radii.y.bottom_right),
        left: radii.x.top_left.max(radii.x.bottom_left),
    };
    let antialias = point(ScaledPixels(1.), ScaledPixels(1.));
    let inset = |near: Point<ScaledPixels>, far: Point<ScaledPixels>| {
        Bounds::from_corners(
            quad.bounds.origin + near + antialias,
            quad.bounds.bottom_right() - far - antialias,
        )
    };
    let horizontal = inset(
        point(widths.left, widths.top.max(edge.top)),
        point(widths.right, widths.bottom.max(edge.bottom)),
    );
    let vertical = inset(
        point(widths.left.max(edge.left), widths.top),
        point(widths.right.max(edge.right), widths.bottom),
    );
    let area =
        |bounds: &Bounds<ScaledPixels>| bounds.size.width.0.max(0.) * bounds.size.height.0.max(0.);
    if area(&horizontal) >= area(&vertical) {
        horizontal
    } else {
        vertical
    }
}
