mod app;
mod color;
mod control;
mod geometry;
mod gpu;
mod input;
mod platform;
mod style;
mod text;

pub use app::window::node::{
    Child, Hooks, Node, NodeCx, NodeKey, canvas, canvas_watching, canvas_within, layers, node,
    shared, text,
};
pub use app::window::scroll::ScrollEvent;
pub use app::window::scroll::ScrollRef;
pub use app::window::snap::{ScrollSnaps, Snap, SnapAlign, SnapArea};
pub use app::window::tree::Tree;
pub use app::window::*;
pub use app::{App, THREAD_PER_APP, run, spawn};
pub use color::*;
pub use control::*;
pub use geometry::*;
pub use gpu::Snapshot;
pub(crate) use gpu::scene::Scene;
pub use gpu::scene::{Backdrop, BlendMode, ColorMatrix, Glass, PaintEffect, Space};
pub use gpu::scene::{Planes, RenderImage, Surface};
pub use input::*;
pub use platform::{
    CARET_BLINK, Timer, WindowChange, WindowControl, WindowId, WindowOptions, WindowState,
    activate, before_exit, control_size, native, post_main, post_main_after, prewarm, quit,
    sync_main,
};
#[cfg(target_os = "linux")]
pub use platform::{PREFER_X11, clipboard};
pub use style::*;
pub use text::{
    AdjustMetric, Caps, Decoration, DecorationStyle, Embedding, Emphasis, FaceRules, Font, FontId,
    FontStyle, FontWeight, Glyph, GlyphId, Hyphen, Hyphenation, Indent, InlineEdges, Line,
    Paragraph, Part, SOFT_HYPHEN, SUBPIXEL_STEPS, SharedString, SizeAdjust, TextAlign, TextLine,
    TextOverrides, TextRun, TextStyle, TextSystem, TextWrap, WhiteSpace, WordBreak, Wrapping,
    reorders, right_to_left, warm, widest_piece,
};

/// True when images can be surfaces.
pub extern "C" fn supports_surfaces() -> bool {
    #[cfg(target_os = "linux")]
    {
        platform::SURFACES.load(std::sync::atomic::Ordering::Relaxed)
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

pub fn supports_glass() -> bool {
    #[cfg(target_os = "macos")]
    {
        platform::supports_glass()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}
