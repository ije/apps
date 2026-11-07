use crate::Rgba;
use objc2::{
    class, msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject, Bool, ClassBuilder, MessageReceiver, Sel},
    sel,
};
use objc2_app_kit::{NSColor, NSView, NSViewLayerContentsPlacement, NSWindowOrderingMode};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use objc2_quartz_core::{CAMetalLayer, CATransaction};
use std::{ffi::CStr, ptr, sync::OnceLock};

const TARGET: &CStr = c"appsInputView";

#[derive(Clone)]
pub(crate) struct GlassSurface {
    pub key: u64,
    pub layer: Retained<CAMetalLayer>,
    pub bounds: [f64; 4],
    pub radius: f64,
    pub tint: Option<Rgba>,
    pub clear: bool,
    pub interactive: bool,
}

pub(crate) struct GlassView {
    view: Retained<NSView>,
    content: Retained<NSView>,
    surface: GlassSurface,
}

pub(crate) fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| AnyClass::get(c"NSGlassEffectView").is_some())
}

fn content_class() -> &'static AnyClass {
    static CLASS: OnceLock<usize> = OnceLock::new();
    let class = *CLASS.get_or_init(|| {
        let mut class = ClassBuilder::new(c"AppsGlassContentView", class!(NSView)).unwrap();
        // WARNING: the input target belongs to the same window and must outlive these subviews.
        unsafe {
            class.add_ivar::<*mut AnyObject>(TARGET);
            for selector in [
                sel!(mouseDown:),
                sel!(mouseUp:),
                sel!(rightMouseDown:),
                sel!(rightMouseUp:),
                sel!(otherMouseDown:),
                sel!(otherMouseUp:),
                sel!(mouseMoved:),
                sel!(mouseExited:),
                sel!(mouseDragged:),
                sel!(rightMouseDragged:),
                sel!(otherMouseDragged:),
                sel!(scrollWheel:),
                sel!(swipeWithEvent:),
                sel!(flagsChanged:),
            ] {
                class.add_method(
                    selector,
                    forward_event as extern "C" fn(_, Sel, *mut AnyObject),
                );
            }
            class.add_method(
                sel!(acceptsFirstMouse:),
                first_mouse as extern "C" fn(_, Sel, *mut AnyObject) -> Bool,
            );
        }
        class.register() as *const AnyClass as usize
    });
    unsafe { &*(class as *const AnyClass) }
}

extern "C" fn forward_event(this: &AnyObject, selector: Sel, event: *mut AnyObject) {
    unsafe {
        let target = *this
            .class()
            .instance_variable(TARGET)
            .unwrap()
            .load_ptr::<*mut AnyObject>(this);
        if !target.is_null() {
            let (): () = target.send_message(selector, (event,));
        }
    }
}

extern "C" fn first_mouse(this: &AnyObject, _: Sel, event: *mut AnyObject) -> Bool {
    unsafe {
        let target = *this
            .class()
            .instance_variable(TARGET)
            .unwrap()
            .load_ptr::<*mut AnyObject>(this);
        msg_send![target, acceptsFirstMouse: event]
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
            let content: *mut NSView = msg_send![content_class(), alloc];
            let content: *mut NSView = msg_send![content, initWithFrame: NSRect::default()];
            let content = Retained::from_raw(content).unwrap();
            let object: &AnyObject = &content;
            object.class().instance_variable(TARGET).unwrap()
                .load_ptr::<*mut AnyObject>(object)
                .write(ptr::from_ref(parent).cast_mut().cast());
            let () = msg_send![&*content, setLayer: &*surface.layer];
            let () = msg_send![&*content, setWantsLayer: Bool::YES];
            let () = msg_send![&*content, setLayerContentsPlacement: NSViewLayerContentsPlacement::TopLeft];
            let () = msg_send![&*view, setContentView: &*content];
            parent.addSubview(&view);
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
    if restack {
        for native in views {
            parent.addSubview_positioned_relativeTo(
                &native.view,
                NSWindowOrderingMode::Above,
                None,
            );
        }
    }
}
