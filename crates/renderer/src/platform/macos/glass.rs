use crate::Color;
use objc2::{
    DefinedClass, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyClass, Bool},
    sel,
};
use objc2_app_kit::{
    NSColor, NSEvent, NSResponder, NSView, NSViewLayerContentsPlacement, NSWindowOrderingMode,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use objc2_quartz_core::{CAMetalLayer, CATransaction};
use std::sync::OnceLock;

#[derive(Clone)]
pub(crate) struct GlassSurface {
    pub key: u64,
    pub layer: Retained<CAMetalLayer>,
    pub bounds: [f64; 4],
    pub radius: f64,
    pub tint: Option<Color>,
    pub clear: bool,
    pub interactive: bool,
}

pub(crate) struct GlassView {
    view: Retained<NSView>,
    content: Retained<GlassContent>,
    surface: GlassSurface,
}

pub(crate) fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| AnyClass::get(c"NSGlassEffectView").is_some())
}

define_class!(
    // SAFETY: NSView has no subclassing rules, and the class has no Drop.
    #[unsafe(super(NSView, NSResponder))]
    #[thread_kind = MainThreadOnly]
    #[name = "AppsGlassContentView"]
    #[ivars = *const NSView]
    struct GlassContent;

    // Glass sits over the page view, so its pointer events go on to that view.
    impl GlassContent {
        #[unsafe(method(mouseDown:))]
        fn left_down(&self, event: &NSEvent) { self.page().mouseDown(event) }
        #[unsafe(method(mouseUp:))]
        fn left_up(&self, event: &NSEvent) { self.page().mouseUp(event) }
        #[unsafe(method(rightMouseDown:))]
        fn right_down(&self, event: &NSEvent) { self.page().rightMouseDown(event) }
        #[unsafe(method(rightMouseUp:))]
        fn right_up(&self, event: &NSEvent) { self.page().rightMouseUp(event) }
        #[unsafe(method(otherMouseDown:))]
        fn other_down(&self, event: &NSEvent) { self.page().otherMouseDown(event) }
        #[unsafe(method(otherMouseUp:))]
        fn other_up(&self, event: &NSEvent) { self.page().otherMouseUp(event) }
        #[unsafe(method(mouseMoved:))]
        fn moved(&self, event: &NSEvent) { self.page().mouseMoved(event) }
        #[unsafe(method(mouseExited:))]
        fn exited(&self, event: &NSEvent) { self.page().mouseExited(event) }
        #[unsafe(method(mouseDragged:))]
        fn left_drag(&self, event: &NSEvent) { self.page().mouseDragged(event) }
        #[unsafe(method(rightMouseDragged:))]
        fn right_drag(&self, event: &NSEvent) { self.page().rightMouseDragged(event) }
        #[unsafe(method(otherMouseDragged:))]
        fn other_drag(&self, event: &NSEvent) { self.page().otherMouseDragged(event) }
        #[unsafe(method(scrollWheel:))]
        fn wheel(&self, event: &NSEvent) { self.page().scrollWheel(event) }
        #[unsafe(method(swipeWithEvent:))]
        fn swipe(&self, event: &NSEvent) { self.page().swipeWithEvent(event) }
        #[unsafe(method(flagsChanged:))]
        fn flags(&self, event: &NSEvent) { self.page().flagsChanged(event) }
        #[unsafe(method(acceptsFirstMouse:))]
        fn first_mouse(&self, event: Option<&NSEvent>) -> bool { self.page().acceptsFirstMouse(event) }
    }
);

impl GlassContent {
    fn page(&self) -> &NSView {
        // SAFETY: the page view belongs to the same window and outlives its glass subviews.
        unsafe { &**self.ivars() }
    }
}

pub(crate) fn update(views: &mut Vec<GlassView>, parent: &NSView, surfaces: Vec<GlassSurface>) {
    let Some(class) = AnyClass::get(c"NSGlassEffectView") else {
        return;
    };
    CATransaction::setDisableActions(true);
    let restack = views.len() != surfaces.len()
        || views
            .iter()
            .zip(&surfaces)
            .any(|(native, surface)| native.surface.key != surface.key);
    views.retain(|native| {
        let keep = surfaces
            .iter()
            .any(|surface| surface.key == native.surface.key);
        if !keep {
            native.view.removeFromSuperview();
        }
        keep
    });
    for (position, surface) in surfaces.into_iter().enumerate() {
        let index = views
            .iter()
            .position(|native| native.surface.key == surface.key);
        let fresh = index.is_none();
        let index = index.unwrap_or_else(|| unsafe {
            let view: *mut NSView = msg_send![class, alloc];
            let view: *mut NSView = msg_send![view, initWithFrame: NSRect::default()];
            let view = Retained::from_raw(view).unwrap();
            let content = GlassContent::alloc(parent.mtm()).set_ivars(std::ptr::from_ref(parent));
            let content: Retained<GlassContent> = msg_send![super(content), initWithFrame: NSRect::default()];
            let () = msg_send![&*content, setLayer: &*surface.layer];
            let () = msg_send![&*content, setWantsLayer: Bool::YES];
            let () = msg_send![&*content, setLayerContentsPlacement: NSViewLayerContentsPlacement::TopLeft];
            let () = msg_send![&*view, setContentView: &*content];
            parent.addSubview_positioned_relativeTo(&view, NSWindowOrderingMode::Below, None);
            views.push(GlassView { view, content, surface: surface.clone() });
            views.len() - 1
        });
        views.swap(position, index);
        let native = &mut views[position];
        let [x, y, width, height] = surface.bounds;
        let y = if parent.isFlipped() {
            y
        } else {
            parent.bounds().size.height - y - height
        };
        let frame = NSRect::new(NSPoint::new(x, y), NSSize::new(width, height));
        if fresh || native.view.frame() != frame {
            native.view.setFrame(frame);
            native
                .content
                .setFrame(NSRect::new(NSPoint::new(0., 0.), frame.size));
        }
        unsafe {
            if !fresh && Retained::as_ptr(&native.surface.layer) != Retained::as_ptr(&surface.layer)
            {
                let () = msg_send![&*native.content, setLayer: &*surface.layer];
            }
            if fresh || native.surface.radius != surface.radius {
                let () = msg_send![&*native.view, setCornerRadius: surface.radius];
            }
            if fresh || native.surface.clear != surface.clear {
                let () = msg_send![&*native.view, setStyle: isize::from(surface.clear)];
            }
            if fresh || native.surface.tint != surface.tint {
                let tint = surface.tint.map(|color| {
                    NSColor::colorWithSRGBRed_green_blue_alpha(
                        color.r.into(),
                        color.g.into(),
                        color.b.into(),
                        color.a.into(),
                    )
                });
                let () = msg_send![&*native.view, setTintColor: tint.as_deref()];
            }
            if fresh || native.surface.interactive != surface.interactive {
                let supports: Bool =
                    msg_send![&*native.view, respondsToSelector: sel!(setEffectIsInteractive:)];
                if supports.as_bool() {
                    let () = msg_send![&*native.view, setEffectIsInteractive: Bool::new(surface.interactive)];
                }
            }
        }
        native.surface = surface;
    }
    // Keep glass below the overlay and web views. Glass above promoted layers or the caret stops them.
    if restack {
        for native in views.iter().rev() {
            parent.addSubview_positioned_relativeTo(
                &native.view,
                NSWindowOrderingMode::Below,
                None,
            );
        }
    }
}
