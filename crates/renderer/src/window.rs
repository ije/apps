use crate::{
    App, Backdrop, BorderStyle, Bounds, BoxShadow, Child, ContentMask, CursorStyle, DevicePixels,
    Edges, ElementId, Modifiers, MouseEvent, PaintEffect, Pixels, PlatformInput, Point, Radii,
    RenderImage, Rgba, ScaledPixels, Size, Tree, Utf16Selection,
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

/// The root of a window's content.
pub trait View: 'static {
    /// Describes the content. Subtrees that did not change can be linked with [`Child::Kept`]
    /// when `tree` still holds them.
    fn render(&mut self, tree: &Tree, window: &mut Window, cx: &mut App) -> Child;

    /// The window was resized, moved, activated, minimized or otherwise changed.
    fn window_changed(&mut self, _window: &mut Window, _cx: &mut App) {}

    /// The user asked to close the window; `false` keeps it open.
    fn should_close(&mut self, _window: &mut Window, _cx: &mut App) -> bool {
        true
    }

    /// Key, modifier and file drop events. Call [`App::stop_propagation`] to keep the event from
    /// the input method.
    fn event(&mut self, _event: &PlatformInput, _window: &mut Window, _cx: &mut App) {}

    // The text input of the focused field, for input methods. Ranges are UTF-16 code units.
    fn accepts_text_input(&mut self, _window: &mut Window, _cx: &mut App) -> bool {
        true
    }
    fn selected_text_range(
        &mut self,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<Utf16Selection> {
        None
    }
    fn marked_text_range(&mut self, _window: &mut Window, _cx: &mut App) -> Option<Range<usize>> {
        None
    }
    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        _adjusted: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<String> {
        None
    }
    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        _text: &str,
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        _text: &str,
        _selected: Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }
    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut App) {}
    fn bounds_for_range(
        &mut self,
        _range: Range<usize>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        None
    }
    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<usize> {
        None
    }
}

#[derive(Clone, Debug)]
pub struct WindowOptions {
    /// The content bounds, or the outer frame with `outer_size`, in screen points.
    pub bounds: Bounds<Pixels>,
    pub outer_size: bool,
    pub title: String,
    /// The content extends under a transparent title bar.
    pub titlebar_hidden: bool,
    pub traffic_light_position: Option<Point<Pixels>>,
    /// No title bar or border at all.
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

/// The state of a native window. Rectangles are in screen points with the origin at the
/// top-left of the primary display.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WindowState {
    /// The outer frame, including the title bar.
    pub frame: Bounds<Pixels>,
    pub content: Bounds<Pixels>,
    pub minimized: bool,
    /// Maximized, or zoomed on macOS.
    pub maximized: bool,
    pub fullscreen: bool,
    /// Shown. A minimized window is not.
    pub visible: bool,
    pub resizing: bool,
}

/// A change to a native window. Platforms ignore the changes they don't support.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowChange {
    /// Moves and resizes the outer frame, in the coordinates of [`WindowState::frame`].
    Frame(Bounds<Pixels>),
    Center,
    /// Shows the window, and focuses it when `true`.
    Show(bool),
    Hide,
    /// Moves the window behind the other windows.
    Blur,
    /// Restores a minimized window.
    Restore,
    /// Limits the outer size to a minimum and a maximum. A zero maximum has no limit.
    SizeLimits(Size<Pixels>, Size<Pixels>),
    AlwaysOnTop(bool),
    Resizable(bool),
    Minimizable(bool),
    Maximizable(bool),
    Closable(bool),
    Movable(bool),
    /// The color shown behind the content.
    Background(Rgba),
    /// The alpha of the whole window, from 0 to 1.
    Opacity(f32),
    Shadow(bool),
    /// Lets mouse events pass through to the windows below. With the second flag, mouse moves
    /// still reach the window while it is the key window.
    IgnoreMouse(bool, bool),
    /// Shows the window on every workspace, and over fullscreen windows with the second flag.
    AllWorkspaces(bool, bool),
    /// Keeps the content out of screenshots and screen sharing.
    ContentProtection(bool),
    /// Whether the window can become the key window. It keeps its focus when turned off.
    Focusable(bool),
    /// Keeps the content width / height at a ratio, after taking an extra size away, while the
    /// user resizes. A ratio of 0 turns it off.
    AspectRatio(f32, Size<Pixels>),
    /// Makes the window a child of another window, or its sheet with the second flag. `None`
    /// detaches it. A child shows above its parent and moves with it.
    Parent(Option<raw_window_handle::RawWindowHandle>, bool),
    /// An `NSVisualEffectMaterial` behind the content, or `None`, with an `NSVisualEffectState`.
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
    /// Doesn't change how other hitboxes are hit.
    #[default]
    Normal,
    /// Hitboxes behind this one are not hovered, but still scroll.
    BlockMouseExceptScroll,
}

/// A region that receives the mouse.
#[derive(Clone, Debug)]
pub struct Hitbox {
    pub id: HitboxId,
    /// In the coordinates of `transform`.
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

/// A projective map of the plane z = 0 from content coordinates to the window: its own map,
/// composed with the outer ones. The map of a keyed effect changes between frames, see
/// [`Window::patch_effect`].
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
    /// Composes the maps again after the local map of `key` became `local`; `done` holds the
    /// transforms composed already.
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

/// The 3x3 homography of a 4x4 column-major transform on the plane z = 0: columns 0, 1 and 3 of
/// rows 0, 1 and 3.
fn plane(m: &[f32; 16]) -> [f32; 9] {
    [m[0], m[1], m[3], m[4], m[5], m[7], m[12], m[13], m[15]]
}

/// Conjugates a 4x4 column-major transform in logical pixels with the scale factor: device = S * M * S^-1.
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
    /// The sides of the border it paints (top 1, right 2, bottom 4, left 8), or 0 for all. A side ends where
    /// the line from the outer corner to the inner one meets it, so sides of other colors join like in CSS.
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

    /// Maps hitboxes under the transform keyed `key` with the new transform `matrix`.
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

    /// Fills `ids` with the hit hitboxes, front first, and returns how many of them are hovered.
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

/// A native window and the state of the frame it draws.
pub struct Window {
    pub(crate) id: WindowId,
    pub(crate) platform: platform::Window,
    pub(crate) view: Option<Box<dyn View>>,
    text_system: Rc<TextSystem>,
    /// Device pixels per pixel, with the page zoom.
    pub(crate) scale_factor: f32,
    /// The content size in pixels, with the page zoom.
    pub(crate) viewport_size: Size<Pixels>,
    /// The page zoom, and the content size and scale factor the platform reported.
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
    /// Light text of the box being painted gets thicker strokes.
    pub(crate) font_smoothing: bool,
    hit_transform: Option<Rc<HitTransform>>,
    opacity: f32,
    states: FxHashMap<(ElementId, TypeId), Box<dyn Any>>,
    next_states: FxHashMap<(ElementId, TypeId), Box<dyn Any>>,
    next_frame_callbacks: Vec<FrameCallback>,
    pub(crate) captures: Vec<CaptureCallback>,
    pub(crate) dirty: bool,
    redraw: [bool; 2],
    drawing: bool,
    pub(crate) mouse_position: Point<Pixels>,
    pub(crate) modifiers: Modifiers,
    pub(crate) removed: bool,
    cursor: Option<CursorStyle>,
    drawn_at: std::time::Instant,
    /// The transforms the compositor plays for promoted layers: key, start, step and transforms,
    /// so hit testing follows them.
    tracks: Vec<(u64, std::time::Instant, f32, Vec<[f32; 16]>)>,
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
            tracks: Vec::new(),
            captures: Vec::new(),
            dirty: false,
            redraw: [false; 2],
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

    /// Zooms the page like a browser: pixels grow by `zoom`, so the viewport shrinks by it and
    /// everything draws again at the new scale. Input from the platform is mapped back.
    pub fn set_zoom(&mut self, zoom: f32) {
        if self.zoom != zoom {
            self.mouse_position = self.mouse_position * (self.zoom / zoom);
            self.zoom = zoom;
            self.mark_resized(self.native.0, self.native.1);
        }
    }

    /// Draws the window on the next frame. Does nothing while drawing; see [`Self::redraw`].
    pub fn refresh(&mut self) {
        if !self.drawing && !self.dirty {
            self.dirty = true;
            self.platform.request_frame();
        }
    }

    /// Draws the window again before the frame is shown, when called during a draw. A frame
    /// draws again at most once; later calls wait for the next frame.
    pub fn redraw(&mut self) {
        if self.drawing {
            self.redraw[0] = true;
        } else {
            self.refresh();
        }
    }

    /// Whether this draw replaces one that called [`Self::redraw`].
    pub fn redrawing(&self) -> bool {
        self.redraw[1]
    }

    pub fn will_draw(&self) -> bool {
        self.drawing || self.dirty || !self.captures.is_empty()
    }

    /// Sets the opacity of the effect layer that the last frame painted with `key`, and its transform
    /// unless `None`, for [`Self::present`]. Hit testing follows the transform. Returns `false` when
    /// the last frame has no such layer or the transform needs a new frame.
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

    /// Takes the promoted layers back into the frames that [`Self::present`] shows.
    pub fn demote(&mut self) {
        #[cfg(target_os = "macos")]
        self.platform.demote();
        self.tracks.clear();
    }

    /// Hands the effect layer that the last frame painted with `key` to the compositor, which plays
    /// `opacities` and, unless `None`, `transforms` (in logical pixels, like [`PaintEffect`]) every
    /// `step` seconds from now and holds the last ones, so the view draws no frames for it. Returns
    /// `false` where the platform can't, or right after a drawn frame, or when the layer has other
    /// effects, is clipped or painted over in its reach; [`Self::patch_effect`] animates it then. A
    /// new promoted layer shows with the next [`Self::present`], and the next drawn frame takes it back.
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
        let [x, y, right, bottom] = [
            mask.rounded.origin.x,
            mask.rounded.origin.y,
            mask.rounded.origin.x + mask.rounded.size.width,
            mask.rounded.origin.y + mask.rounded.size.height,
        ];
        let (rx, ry) = (&mask.corner_radii.x, &mask.corner_radii.y);
        let corners = [
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
        .map(|(x, y, width, height)| Bounds::new(point(x, y), size(width, height)));
        if reach.is_none_or(|reach| {
            mask.bounds.intersect(&reach) != reach
                || corners.iter().any(|corner| corner.intersects(&reach))
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

    /// Shows the last frame again with its patched layers, without laying out or painting the
    /// view. Returns `false` when the window draws anyway.
    pub fn present(&mut self) -> bool {
        if self.will_draw() {
            return false;
        }
        // WARNING: the atlas and text caches keep their frame, so the tiles this scene reads stay.
        let _ = self.platform.draw(&self.scene, false);
        true
    }

    /// Runs `callback` at the start of the next frame, whether or not the window draws.
    pub fn on_next_frame(&mut self, callback: impl FnOnce(&mut Window, &mut App) + 'static) {
        self.next_frame_callbacks.push(Box::new(callback));
        self.platform.request_frame();
    }

    pub fn request_animation_frame(&mut self) {
        self.on_next_frame(|window, _| window.refresh());
    }

    /// Draws the window again and passes that frame to `done`: its size in device pixels and RGBA rows.
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

    /// Closes the window after the current event.
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

    pub fn is_fullscreen(&self) -> bool {
        self.platform.state().fullscreen
    }

    pub fn toggle_fullscreen(&self) {
        self.platform.toggle_fullscreen();
    }

    pub fn minimize_window(&self) {
        self.platform.minimize();
    }

    pub fn zoom_window(&self) {
        self.platform.zoom();
    }

    pub fn activate_window(&self) {
        self.platform.activate();
    }

    pub fn set_window_title(&mut self, title: &str) {
        self.platform.set_title(title);
    }

    /// Resizes the content area.
    pub fn resize(&mut self, size: Size<Pixels>) {
        self.platform.resize(size);
    }

    pub fn start_window_move(&self) {
        self.platform.start_window_move();
    }

    pub fn titlebar_double_click(&self) {
        self.platform.titlebar_double_click();
    }

    /// Shows or hides the traffic lights of a macOS window.
    pub fn set_window_button_visibility(&self, visible: bool) {
        self.platform.set_button_visibility(visible);
    }

    pub fn raw_window_handle(&self) -> Result<raw_window_handle::RawWindowHandle> {
        self.platform.raw_window_handle()
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

    /// Lays out and paints the view, then shows the frame.
    pub(crate) fn draw(&mut self, cx: &mut App) {
        #[cfg(target_os = "macos")]
        self.platform.demote();
        self.tracks.clear();
        self.drawn_at = std::time::Instant::now();
        loop {
            self.dirty = false;
            self.drawing = true;
            self.next.clear();
            self.scene.clear();
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
                tree.begin(self.scale_factor, self.text_system.generation());
                let root = view.render(&tree, self, cx);
                self.view = Some(view);
                if let Some(root) = tree.commit_root(root, self.viewport_size, self.scale_factor) {
                    tree.layout(root, self.viewport_size, self);
                    tree.prepaint(root, self);
                    self.hover_count = self.next.hit_test(self.mouse_position, &mut self.hit_test);
                    tree.paint(root, self);
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
        self.scene.finish();
        let captures = mem::take(&mut self.captures);
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

    /// Commits, lays out and prepaints `render`'s content without painting or showing it, so a
    /// view can read current bounds between frames. Like a draw, a pass that calls
    /// [`Self::redraw`] runs once more. Returns `false` while the window draws.
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
            tree.begin(self.scale_factor, self.text_system.generation());
            let root = render(&tree, self, cx);
            if let Some(root) = tree.commit_root(root, self.viewport_size, self.scale_factor) {
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

    /// Whether the mouse is over a drag area, from the last frame.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn in_drag_area(&self) -> bool {
        self.hit_test
            .first()
            .is_some_and(|top| self.rendered.drag_areas.contains(top))
    }

    /// Sends an input event to the mouse listeners or the view. Returns whether it was handled.
    pub fn dispatch_event(&mut self, event: PlatformInput, cx: &mut App) -> bool {
        if let Some(position) = event.mouse_position() {
            self.mouse_position = position;
        }
        match &event {
            PlatformInput::MouseDown(event) => self.modifiers = event.modifiers,
            PlatformInput::MouseUp(event) => self.modifiers = event.modifiers,
            PlatformInput::MouseMove(event) => self.modifiers = event.modifiers,
            PlatformInput::ScrollWheel(event) => self.modifiers = event.modifiers,
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
            // Listeners added while dispatching belong to a newer frame.
            if self.rendered.mouse_listeners.is_empty() {
                self.rendered.mouse_listeners = listeners;
            }
        } else if let Some(mut view) = self.view.take() {
            view.event(&event, self, cx);
            if self.view.is_none() {
                self.view = Some(view);
            }
        }
        !cx.propagate_event
    }

    /// Sends a key press like a keyboard would, typing its text when nothing handles it.
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

    /// Runs `f` with the view taken out of the window.
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

    /// Listens to mouse events of this frame.
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

    /// Keeps state for `id` from one frame to the next.
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

    /// Whether light text of the box being painted gets thicker strokes.
    pub fn font_smoothing(&self) -> bool {
        self.font_smoothing
    }

    pub fn content_mask(&self) -> ContentMask<Pixels> {
        self.content_masks.last().map_or_else(
            || ContentMask::from(Bounds::new(Point::default(), self.viewport_size)),
            |(mask, _)| *mask,
        )
    }

    /// Pushes a content mask with its device pixel form, which primitives copy.
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

    /// Maps bounds from the current hit transform's coordinates to the window, as a bounding box.
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

    /// Runs `f` with hit testing mapped through a 4x4 column-major transform from the content's
    /// coordinates to the window, in logical pixels. Use it in prepaint around content painted
    /// with a transformed [`PaintEffect`].
    pub fn with_hit_transform<R>(&mut self, m: [f32; 16], f: impl FnOnce(&mut Self) -> R) -> R {
        self.with_keyed_hit_transform(m, None, f)
    }

    /// Like [`Self::with_hit_transform`], for content painted with an effect keyed by `key`, whose
    /// transform [`Self::patch_effect`] can change.
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

    /// Rounds half toward zero, but a non-zero stroke keeps at least one device pixel.
    fn snap_stroke(&self, value: Pixels) -> ScaledPixels {
        ScaledPixels(round_stroke_to_device_pixel(value.0, self.scale_factor))
    }

    /// Floors the near edges and ceils the far ones, so the result covers `bounds`.
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

    /// Paints the primitives inside `f` at one draw order, so they keep their paint order.
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
        // A border-only quad is split around its empty interior, so the inside is not shaded.
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

    /// Paints the drop shadows; inset ones are skipped.
    pub fn paint_drop_shadows(
        &mut self,
        bounds: Bounds<Pixels>,
        corner_radii: Radii<Pixels>,
        shadows: &[BoxShadow],
    ) {
        self.paint_shadows(bounds, corner_radii, shadows, false);
    }

    /// Paints the inset shadows, after the background.
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

    /// Paints everything inside `f` into an offscreen layer, then composites it with the effect.
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

    /// Splits double lines and cuts dots and dashes out of a solid line, with WebKit's lengths.
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

    /// Paints a glyph with its baseline at `origin.y`.
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
        let quantized = point(
            round_half_toward_zero(origin.x.0 * variants) / variants,
            round_half_toward_zero(origin.y.0),
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

    /// Paints an emoji glyph with its baseline at `origin.y`.
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

    /// The glyph's tile and the offset of its pixels from the glyph origin.
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

    /// Paints the part of an image placed at `image_bounds` that `bounds` shows, with `corner_radii`
    /// applied to that part.
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

    /// Frees the GPU copy of an image.
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

/// The largest rectangle inside a border-only quad that holds no border pixels.
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
    // Rounded corners need only be excluded on one axis, so the larger band wins.
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
