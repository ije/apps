use super::{
    NSRange, NSStringExt, Sendable, TISCopyCurrentKeyboardInputSource, TISGetInputSourceProperty,
    ask_within,
    display_link::WindowFrameSource,
    events::platform_input_from_native,
    kTISPropertyInputSourceIsASCIICapable, kTISPropertyInputSourceType, kTISTypeKeyboardInputMode,
    metal::{MetalAtlas, Present, Renderer},
    native_after, native_post, native_sync, ns_string, post, post_local,
};
use crate::{
    Bounds, CursorStyle, FileDropEvent, KeyDownEvent, Keystroke, Modifiers, ModifiersChangedEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, NavigationDirection, Pixels,
    PlatformInput, Point, Scene, ScrollDelta, ScrollPhase, Size, WindowChange, WindowOptions,
    WindowState,
    app::{Callbacks, dispatch, send_ime_key},
    point, px, size,
};
use anyhow::{Context as _, Result};
use core_foundation::base::{CFRelease, CFTypeRef};
use core_foundation_sys::{
    base::CFEqual,
    number::{CFBooleanGetValue, CFBooleanRef},
};
use core_graphics::display::CGDirectDisplayID;
use objc2::{
    AnyThread, MainThreadMarker, class, msg_send,
    rc::{Retained, autoreleasepool},
    runtime::{AnyClass, AnyObject, AnyProtocol, Bool, ClassBuilder, MessageReceiver, Sel},
    sel,
};
#[expect(
    deprecated,
    reason = "file drops are decoded from a property list of paths"
)]
use objc2_app_kit::NSFilenamesPboardType;
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBackingStoreType, NSButton, NSColor, NSDragOperation, NSScreen,
    NSTrackingArea, NSTrackingAreaOptions, NSView, NSViewLayerContentsPlacement,
    NSViewLayerContentsRedrawPolicy, NSWindow, NSWindowButton, NSWindowCollectionBehavior,
    NSWindowOcclusionState, NSWindowOrderingMode, NSWindowSharingType, NSWindowStyleMask,
    NSWindowTitleVisibility,
};
use objc2_core_foundation::CGSize;
use objc2_foundation::{
    NSArray, NSInteger, NSNotFound, NSOperatingSystemVersion, NSPoint, NSProcessInfo, NSRect,
    NSSize, NSString, NSUInteger,
};
use objc2_quartz_core::{CALayer, CAMetalLayer};
use raw_window_handle as rwh;
use std::{
    cell::RefCell,
    ffi::{CStr, c_void},
    path::PathBuf,
    ptr::{self, NonNull},
    rc::{Rc, Weak},
    sync::{
        Arc, Mutex, OnceLock, PoisonError,
        atomic::{AtomicBool, AtomicPtr, Ordering},
    },
    time::Duration,
};

const STATE_IVAR: &CStr = c"windowState";
const VISUAL_EFFECT_STATE_IVAR: &CStr = c"visualEffectState";

#[allow(non_upper_case_globals)]
const NSNormalWindowLevel: NSInteger = 0;
#[allow(non_upper_case_globals)]
const NSFloatingWindowLevel: NSInteger = 3;

struct Classes {
    window: usize,
    view: usize,
    overlay: usize,
}

fn classes() -> (&'static AnyClass, &'static AnyClass, &'static AnyClass) {
    static CLASSES: OnceLock<Classes> = OnceLock::new();
    let classes = CLASSES.get_or_init(|| {
        extern "C" fn no_hit(_: &AnyObject, _: Sel, _: NSPoint) -> *mut AnyObject {
            ptr::null_mut()
        }
        extern "C" fn flipped(_: &AnyObject, _: Sel) -> Bool {
            Bool::YES
        }
        // SAFETY: the methods have the signatures AppKit calls them with.
        unsafe {
            let mut overlay = ClassBuilder::new(c"AppsOverlayView", class!(NSView)).unwrap();
            overlay.add_method(
                sel!(hitTest:),
                no_hit as extern "C" fn(_, Sel, NSPoint) -> *mut AnyObject,
            );
            overlay.add_method(sel!(isFlipped), flipped as extern "C" fn(_, Sel) -> Bool);
            Classes {
                window: build_window_class() as usize,
                view: build_view_class() as usize,
                overlay: overlay.register() as *const AnyClass as usize,
            }
        }
    });
    // SAFETY: registered classes live for the whole process.
    unsafe {
        (
            &*(classes.window as *const AnyClass),
            &*(classes.view as *const AnyClass),
            &*(classes.overlay as *const AnyClass),
        )
    }
}

unsafe fn build_view_class() -> *const AnyClass {
    unsafe {
        let mut decl = ClassBuilder::new(c"AppsView", class!(NSView)).unwrap();
        decl.add_ivar::<*mut c_void>(STATE_IVAR);
        decl.add_method(sel!(dealloc), dealloc_view as extern "C" fn(_, Sel));
        decl.add_method(
            sel!(performKeyEquivalent:),
            handle_key_equivalent as extern "C" fn(_, Sel, *mut AnyObject) -> Bool,
        );
        decl.add_method(
            sel!(keyDown:),
            handle_key_down as extern "C" fn(_, Sel, *mut AnyObject),
        );
        decl.add_method(
            sel!(keyUp:),
            handle_key_down as extern "C" fn(_, Sel, *mut AnyObject),
        );
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
            decl.add_method(
                selector,
                handle_view_event as extern "C" fn(_, Sel, *mut AnyObject),
            );
        }
        decl.add_method(
            sel!(resetCursorRects),
            reset_cursor_rects as extern "C" fn(_, Sel),
        );
        decl.add_method(
            sel!(makeBackingLayer),
            make_backing_layer as extern "C" fn(_, Sel) -> *mut AnyObject,
        );
        decl.add_protocol(AnyProtocol::get(c"CALayerDelegate").unwrap());
        decl.add_method(
            sel!(viewDidChangeBackingProperties),
            view_did_change_backing_properties as extern "C" fn(_, Sel),
        );
        decl.add_method(
            sel!(setFrameSize:),
            set_frame_size as extern "C" fn(_, Sel, NSSize),
        );
        decl.add_method(
            sel!(displayLayer:),
            display_layer as extern "C" fn(_, Sel, *mut AnyObject),
        );
        decl.add_protocol(AnyProtocol::get(c"NSTextInputClient").unwrap());
        decl.add_method(
            sel!(validAttributesForMarkedText),
            valid_attributes_for_marked_text as extern "C" fn(_, Sel) -> *mut AnyObject,
        );
        decl.add_method(
            sel!(hasMarkedText),
            has_marked_text as extern "C" fn(_, Sel) -> Bool,
        );
        decl.add_method(
            sel!(markedRange),
            marked_range as extern "C" fn(_, Sel) -> NSRange,
        );
        decl.add_method(
            sel!(selectedRange),
            selected_range as extern "C" fn(_, Sel) -> NSRange,
        );
        decl.add_method(
            sel!(firstRectForCharacterRange:actualRange:),
            first_rect_for_character_range
                as extern "C" fn(_, Sel, NSRange, *mut NSRange) -> NSRect,
        );
        decl.add_method(
            sel!(insertText:replacementRange:),
            insert_text as extern "C" fn(_, Sel, *mut AnyObject, NSRange),
        );
        decl.add_method(
            sel!(setMarkedText:selectedRange:replacementRange:),
            set_marked_text as extern "C" fn(_, Sel, *mut AnyObject, NSRange, NSRange),
        );
        decl.add_method(sel!(unmarkText), unmark_text as extern "C" fn(_, Sel));
        decl.add_method(
            sel!(attributedSubstringForProposedRange:actualRange:),
            attributed_substring_for_proposed_range
                as extern "C" fn(_, Sel, NSRange, *mut NSRange) -> *mut AnyObject,
        );
        decl.add_method(
            sel!(viewDidChangeEffectiveAppearance),
            view_did_change_effective_appearance as extern "C" fn(_, Sel),
        );
        // WARNING: keeps AppKit from beeping on keys with modifiers.
        decl.add_method(
            sel!(doCommandBySelector:),
            do_command_by_selector as extern "C" fn(_, Sel, Sel),
        );
        decl.add_method(
            sel!(acceptsFirstMouse:),
            accepts_first_mouse as extern "C" fn(_, Sel, *mut AnyObject) -> Bool,
        );
        decl.add_method(
            sel!(_opaqueRectForWindowMoveWhenInTitlebar),
            opaque_rect_for_window_move_when_in_titlebar as extern "C" fn(_, Sel) -> NSRect,
        );
        decl.add_method(
            sel!(characterIndexForPoint:),
            character_index_for_point as extern "C" fn(_, Sel, NSPoint) -> NSUInteger,
        );
        decl.register()
    }
}

unsafe fn build_window_class() -> *const AnyClass {
    unsafe {
        let mut decl = ClassBuilder::new(c"AppsWindow", class!(NSWindow)).unwrap();
        decl.add_ivar::<*mut c_void>(STATE_IVAR);
        decl.add_ivar::<isize>(VISUAL_EFFECT_STATE_IVAR);
        // WARNING: Liquid Glass needs this private getter. Its ivar stays readable when AppKit reenters with State borrowed.
        if class!(NSWindow)
            .instance_method(sel!(_hasActiveAppearance))
            .is_some()
        {
            decl.add_method(
                sel!(_hasActiveAppearance),
                has_active_appearance as extern "C" fn(_, Sel) -> Bool,
            );
        }
        decl.add_method(sel!(dealloc), dealloc_window as extern "C" fn(_, Sel));
        decl.add_method(
            sel!(canBecomeMainWindow),
            can_become_key as extern "C" fn(_, Sel) -> Bool,
        );
        decl.add_method(
            sel!(canBecomeKeyWindow),
            can_become_key as extern "C" fn(_, Sel) -> Bool,
        );
        decl.add_method(
            sel!(windowDidResize:),
            window_did_resize as extern "C" fn(_, Sel, *mut AnyObject),
        );
        decl.add_method(
            sel!(windowWillResize:toSize:),
            window_will_resize as extern "C" fn(_, Sel, *mut AnyObject, NSSize) -> NSSize,
        );
        decl.add_method(
            sel!(windowDidChangeOcclusionState:),
            window_did_change_occlusion_state as extern "C" fn(_, Sel, *mut AnyObject),
        );
        decl.add_method(
            sel!(windowWillEnterFullScreen:),
            window_will_enter_fullscreen as extern "C" fn(_, Sel, *mut AnyObject),
        );
        decl.add_method(
            sel!(windowWillExitFullScreen:),
            window_will_exit_fullscreen as extern "C" fn(_, Sel, *mut AnyObject),
        );
        decl.add_method(
            sel!(windowDidFailToExitFullScreen:),
            window_did_fail_to_exit_fullscreen as extern "C" fn(_, Sel, *mut AnyObject),
        );
        decl.add_method(
            sel!(windowDidExitFullScreen:),
            window_did_exit_fullscreen as extern "C" fn(_, Sel, *mut AnyObject),
        );
        for selector in [
            sel!(windowDidMove:),
            sel!(windowDidMiniaturize:),
            sel!(windowDidDeminiaturize:),
            sel!(windowDidEndLiveResize:),
            sel!(windowDidEnterFullScreen:),
        ] {
            decl.add_method(
                selector,
                window_did_change_state as extern "C" fn(_, Sel, *mut AnyObject),
            );
        }
        decl.add_method(
            sel!(windowDidChangeScreen:),
            window_did_change_screen as extern "C" fn(_, Sel, *mut AnyObject),
        );
        decl.add_method(
            sel!(windowDidBecomeKey:),
            window_did_change_key_status as extern "C" fn(_, Sel, *mut AnyObject),
        );
        decl.add_method(
            sel!(windowDidResignKey:),
            window_did_change_key_status as extern "C" fn(_, Sel, *mut AnyObject),
        );
        decl.add_method(
            sel!(windowShouldClose:),
            window_should_close as extern "C" fn(_, Sel, *mut AnyObject) -> Bool,
        );
        decl.add_method(sel!(close), close_window as extern "C" fn(_, Sel));
        decl.add_method(
            sel!(draggingEntered:),
            dragging_entered as extern "C" fn(_, Sel, *mut AnyObject) -> NSDragOperation,
        );
        decl.add_method(
            sel!(draggingUpdated:),
            dragging_updated as extern "C" fn(_, Sel, *mut AnyObject) -> NSDragOperation,
        );
        decl.add_method(
            sel!(draggingExited:),
            dragging_exited as extern "C" fn(_, Sel, *mut AnyObject),
        );
        decl.add_method(
            sel!(performDragOperation:),
            perform_drag_operation as extern "C" fn(_, Sel, *mut AnyObject) -> Bool,
        );
        decl.add_method(
            sel!(concludeDragOperation:),
            dragging_exited as extern "C" fn(_, Sel, *mut AnyObject),
        );
        decl.register()
    }
}

#[derive(Clone, Copy)]
struct TrafficLightFrames {
    titlebar: NSRect,
    close: NSRect,
    minimize: NSRect,
    zoom: NSRect,
}

struct TrafficLightButtons {
    close: Retained<NSButton>,
    minimize: Retained<NSButton>,
    zoom: Retained<NSButton>,
}

#[derive(Clone, Copy, Default)]
struct Mirror {
    scale: f32,
    content: Size<Pixels>,
    active: bool,
    state: WindowState,
}

struct Frames {
    source: WindowFrameSource,
    wants: bool,
    visible: bool,
    display: Option<CGDirectDisplayID>,
    closed: bool,
}

impl Frames {
    fn sync(&mut self) {
        match self
            .display
            .filter(|_| self.wants && self.visible && !self.closed)
        {
            Some(display) => {
                if let Err(error) = self.source.start(display) {
                    eprintln!("apps: display link: {error:#}");
                }
            }
            None => self.source.stop(),
        }
    }
}

const RESIZE_WAIT: Duration = Duration::from_millis(8);
const QUIET: Duration = Duration::from_secs(1);

struct Tick {
    callbacks: Callbacks,
    pending: AtomicBool,
}

extern "C" fn tick(context: *mut c_void) {
    // SAFETY: the context is an `Arc` that the frame source owns until it frees it.
    let tick = unsafe {
        Arc::increment_strong_count(context as *const Tick);
        Arc::from_raw(context as *const Tick)
    };
    if !tick.pending.swap(true, Ordering::AcqRel) {
        post(
            tick.callbacks.thread,
            Box::new(move || {
                tick.pending.store(false, Ordering::Release);
                step(tick.callbacks);
            }),
        );
    }
}

extern "C" fn free_tick(context: *mut c_void) {
    // SAFETY: the frame source gives up its `Arc` once.
    drop(unsafe { Arc::from_raw(context as *const Tick) });
}

fn step(callbacks: Callbacks) {
    if callbacks.frame() == Some(false) {
        callbacks.with(|window, _| window.platform.stop_frames());
    }
}

struct State {
    callbacks: Callbacks,
    native_window: *mut AnyObject,
    native_view: NonNull<AnyObject>,
    layer: Retained<CAMetalLayer>,
    mirror: Arc<Mutex<Mirror>>,
    frames: Arc<Mutex<Frames>>,
    glass: Vec<super::glass::GlassView>,
    controls: super::controls::Controls,
    motion: Option<Arc<Mutex<Option<PlatformInput>>>>,
    cursor: CursorStyle,
    last_key_equivalent: Option<KeyDownEvent>,
    last_modifiers: Option<ModifiersChangedEvent>,
    keystroke_for_do_command: Option<Keystroke>,
    do_command_handled: Option<bool>,
    external_files_dragged: bool,
    synthetic_drag_counter: usize,
    first_mouse: bool,
    last_press: Option<Retained<AnyObject>>,
    app_owns_titlebar_drag: bool,
    traffic_light_position: Option<Point<Pixels>>,
    traffic_light_frames: Option<TrafficLightFrames>,
    pre_fullscreen_traffic_light_frames: Option<TrafficLightFrames>,
    transparent_titlebar: bool,
    is_exiting_fullscreen: bool,
    closed: bool,
    focusable: bool,
    aspect: Option<(f64, NSSize)>,
    parent: Option<objc2::rc::Weak<NSWindow>>,
    sheet: bool,
}

impl State {
    fn native_window(&self) -> &NSWindow {
        // SAFETY: `Window::open` stores an NSWindow, released only after the state.
        unsafe { &*self.native_window.cast::<NSWindow>() }
    }

    fn content_size(&self) -> Size<Pixels> {
        let NSSize { width, height } = self
            .native_window()
            .contentView()
            .map_or_else(Default::default, |view| view.frame().size);
        size(px(width as f32), px(height as f32))
    }

    fn scale_factor(&self) -> f32 {
        let factor = self
            .native_window()
            .screen()
            .map_or(2., |screen| screen.backingScaleFactor() as f32);
        // WARNING: AppKit can report 0 for a window without a screen.
        if factor == 0. { 2. } else { factor }
    }

    fn is_fullscreen(&self) -> bool {
        self.native_window()
            .styleMask()
            .contains(NSWindowStyleMask::FullScreen)
    }

    fn is_visible(&self) -> bool {
        self.native_window()
            .occlusionState()
            .contains(NSWindowOcclusionState::Visible)
    }

    fn window_state(&self) -> WindowState {
        let window = self.native_window();
        let top = primary_screen_top(MainThreadMarker::new().unwrap());
        let flip = |rect: NSRect| {
            Bounds::new(
                point(
                    px(rect.origin.x as f32),
                    px((top - rect.origin.y - rect.size.height) as f32),
                ),
                size(px(rect.size.width as f32), px(rect.size.height as f32)),
            )
        };
        let frame = window.frame();
        let minimized = window.isMiniaturized();
        let fullscreen = self.is_fullscreen();
        let zoomed = if window.styleMask().contains(NSWindowStyleMask::Resizable) {
            window.isZoomed()
        } else {
            let visible = window
                .screen()
                .map_or_else(Default::default, |screen| screen.visibleFrame());
            frame.size.width == visible.size.width && frame.size.height == visible.size.height
        };
        // WARNING: a window outside all displays has no screen.
        let screen = window.screen().or_else(|| {
            NSScreen::screens(MainThreadMarker::new().unwrap())
                .iter()
                .next()
        });
        WindowState {
            frame: flip(frame),
            content: flip(window.contentRectForFrameRect(frame)),
            minimized,
            maximized: zoomed && !minimized && !fullscreen,
            fullscreen,
            visible: window.isVisible(),
            resizing: window.inLiveResize(),
            display: screen
                .as_ref()
                .map_or_else(Default::default, |screen| flip(screen.frame())),
            work_area: screen
                .as_ref()
                .map_or_else(Default::default, |screen| flip(screen.visibleFrame())),
        }
    }

    fn update_mirror(&self) {
        *self.mirror.lock().unwrap_or_else(PoisonError::into_inner) = Mirror {
            scale: self.scale_factor(),
            content: self.content_size(),
            active: self.native_window().isKeyWindow(),
            state: self.window_state(),
        };
    }

    fn locate_frames(&self) {
        // SAFETY: plain property reads on a live window.
        let screen: *mut AnyObject = unsafe { msg_send![self.native_window, screen] };
        let mut frames = self.frames.lock().unwrap_or_else(PoisonError::into_inner);
        frames.visible = self.is_visible();
        // WARNING: AppKit can report no screen while displays are reconfigured.
        frames.display = display_id_for_screen(screen);
        frames.sync();
    }

    fn traffic_light_buttons(&self) -> Option<TrafficLightButtons> {
        let window = self.native_window();
        Some(TrafficLightButtons {
            close: window.standardWindowButton(NSWindowButton::CloseButton)?,
            minimize: window.standardWindowButton(NSWindowButton::MiniaturizeButton)?,
            zoom: window.standardWindowButton(NSWindowButton::ZoomButton)?,
        })
    }

    fn titlebar_container(close: &NSButton) -> Option<Retained<NSView>> {
        // SAFETY: AppKit returns the button's ancestors as retained views.
        unsafe { close.superview()?.superview() }
    }

    fn capture_traffic_light_frames(&self) -> Option<TrafficLightFrames> {
        let buttons = self.traffic_light_buttons()?;
        let titlebar = Self::titlebar_container(&buttons.close)?;
        Some(TrafficLightFrames {
            titlebar: titlebar.frame(),
            close: buttons.close.frame(),
            minimize: buttons.minimize.frame(),
            zoom: buttons.zoom.frame(),
        })
    }

    fn move_traffic_light(&mut self) {
        let Some(position) = self.traffic_light_position else {
            return;
        };
        if self.is_fullscreen() && !self.is_exiting_fullscreen {
            self.restore_traffic_light();
            return;
        }
        if self.traffic_light_frames.is_none() {
            self.traffic_light_frames = self.capture_traffic_light_frames();
        }
        // WARNING: AppKit can recreate the standard buttons, so each layout reads the live views.
        let Some(buttons) = self.traffic_light_buttons() else {
            return;
        };
        let Some(titlebar) = Self::titlebar_container(&buttons.close) else {
            return;
        };
        let window_height = self.native_window().frame().size.height;
        let close = buttons.close.frame();
        let minimize = buttons.minimize.frame();
        let padding = minimize.origin.x - close.origin.x - close.size.width;
        let (x, y) = (position.x.0 as f64, position.y.0 as f64);
        let container_height = close.size.height + y + y;
        let mut frame = titlebar.frame();
        frame.size.height = container_height;
        frame.origin.y = window_height - container_height;
        titlebar.setFrame(frame);
        let minimize_x = x + close.size.width + padding;
        buttons.close.setFrameOrigin(NSPoint::new(x, y));
        buttons.minimize.setFrameOrigin(NSPoint::new(minimize_x, y));
        buttons
            .zoom
            .setFrameOrigin(NSPoint::new(minimize_x + close.size.width + padding, y));
        titlebar.updateTrackingAreas();
        buttons.close.updateTrackingAreas();
        buttons.minimize.updateTrackingAreas();
        buttons.zoom.updateTrackingAreas();
    }

    fn restore_traffic_light(&mut self) {
        let Some(frames) = self.traffic_light_frames.take() else {
            return;
        };
        let Some(buttons) = self.traffic_light_buttons() else {
            return;
        };
        let Some(titlebar) = Self::titlebar_container(&buttons.close) else {
            return;
        };
        buttons.close.setFrame(frames.close);
        buttons.minimize.setFrame(frames.minimize);
        buttons.zoom.setFrame(frames.zoom);
        titlebar.setFrame(frames.titlebar);
        titlebar.updateTrackingAreas();
        buttons.close.updateTrackingAreas();
        buttons.minimize.updateTrackingAreas();
        buttons.zoom.updateTrackingAreas();
    }
}

type Shared = Rc<RefCell<State>>;

// What `estimate` reads about the screen that windows open on.
#[derive(Clone, Copy)]
struct Screens {
    titlebar: f32,
    scale: f32,
    frame: Bounds<Pixels>,
    work_area: Bounds<Pixels>,
}

static SCREENS: Mutex<Option<Screens>> = Mutex::new(None);

// Runs on the main thread, at launch and after each window opens.
pub(crate) fn measure_screens() {
    let marker = MainThreadMarker::new().unwrap();
    let screens = NSScreen::screens(marker);
    let Some(screen) = screens
        .iter()
        .next()
        .or_else(|| NSScreen::mainScreen(marker))
    else {
        return;
    };
    let top = primary_screen_top(marker);
    let flip = |rect: NSRect| {
        Bounds::new(
            point(
                px(rect.origin.x as f32),
                px((top - rect.origin.y - rect.size.height) as f32),
            ),
            size(px(rect.size.width as f32), px(rect.size.height as f32)),
        )
    };
    let content = NSRect::new(NSPoint::new(0., 0.), NSSize::new(100., 100.));
    let frame = NSWindow::frameRectForContentRect_styleMask(
        content,
        NSWindowStyleMask::Closable | NSWindowStyleMask::Titled,
        marker,
    );
    let scale = screen.backingScaleFactor() as f32;
    *SCREENS.lock().unwrap_or_else(PoisonError::into_inner) = Some(Screens {
        titlebar: (frame.size.height - content.size.height) as f32,
        scale: if scale == 0. { 2. } else { scale },
        frame: flip(screen.frame()),
        work_area: flip(screen.visibleFrame()),
    });
}

// WARNING: the window opens on the main thread after `Window::open` returns, so until then its mirror holds what
// `open_native` will make of the options on the first screen. An open that finds another size reports a resize.
fn estimate(options: &WindowOptions) -> Mirror {
    let mut screens = *SCREENS.lock().unwrap_or_else(PoisonError::into_inner);
    if screens.is_none() {
        native_sync(measure_screens);
        screens = *SCREENS.lock().unwrap_or_else(PoisonError::into_inner);
    }
    let Some(screens) = screens else {
        return Mirror::default();
    };
    let titlebar = if options.titlebar_hidden || options.frameless {
        px(0.)
    } else {
        px(screens.titlebar)
    };
    let mut frame = options.bounds;
    frame.origin += screens.frame.origin;
    if !options.outer_size {
        frame.size.height += titlebar;
    }
    let mut content = frame;
    content.origin.y += titlebar;
    content.size.height -= titlebar;
    Mirror {
        scale: screens.scale,
        content: content.size,
        active: false,
        state: WindowState {
            frame,
            content,
            visible: options.show,
            display: screens.frame,
            work_area: screens.work_area,
            ..WindowState::default()
        },
    }
}

fn open_native(
    options: &WindowOptions,
    callbacks: Callbacks,
    layer: Retained<CAMetalLayer>,
    overlay: Retained<CALayer>,
    frames: Arc<Mutex<Frames>>,
    mirror: Arc<Mutex<Mirror>>,
) -> Result<(*mut AnyObject, NonNull<AnyObject>)> {
    let marker = MainThreadMarker::new().unwrap();
    let (window_class, view_class, overlay_class) = classes();
    autoreleasepool(|_| unsafe {
        let () = msg_send![class!(NSWindow), setAllowsAutomaticWindowTabbing: Bool::NO];
        let mut style_mask = NSWindowStyleMask::Closable | NSWindowStyleMask::Titled;
        if options.resizable {
            style_mask |= NSWindowStyleMask::Resizable;
        }
        if options.minimizable {
            style_mask |= NSWindowStyleMask::Miniaturizable;
        }
        if options.titlebar_hidden || options.frameless {
            style_mask |= NSWindowStyleMask::FullSizeContentView;
        }
        let screens = NSScreen::screens(marker);
        let screen = screens
            .iter()
            .next()
            .or_else(|| NSScreen::mainScreen(marker));
        let screen_frame = screen
            .as_ref()
            .map_or_else(Default::default, |screen| screen.frame());
        let bounds = options.bounds;
        let mut content = NSSize::new(bounds.size.width.0 as f64, bounds.size.height.0 as f64);
        if options.outer_size {
            content = NSWindow::contentRectForFrameRect_styleMask(
                NSRect::new(NSPoint::new(0., 0.), content),
                style_mask,
                marker,
            )
            .size;
        }
        let rect = NSRect::new(
            NSPoint::new(
                screen_frame.origin.x + bounds.origin.x.0 as f64,
                screen_frame.origin.y + screen_frame.size.height - bounds.origin.y.0 as f64,
            ),
            content,
        );
        let native_window: *mut AnyObject = msg_send![window_class, alloc];
        let native_window: *mut AnyObject = msg_send![
            native_window,
            initWithContentRect: rect,
            styleMask: style_mask,
            backing: NSBackingStoreType::Buffered,
            defer: Bool::NO,
            screen: screen.as_deref()
        ];
        anyhow::ensure!(!native_window.is_null(), "AppKit did not create the window");
        #[expect(
            deprecated,
            reason = "file drops are decoded from a property list of paths"
        )]
        let () = msg_send![native_window, registerForDraggedTypes: &*NSArray::from_slice(&[NSFilenamesPboardType])];
        let () = msg_send![native_window, setReleasedWhenClosed: Bool::NO];
        let content_view: *mut AnyObject = msg_send![native_window, contentView];
        let content_bounds: NSRect = msg_send![content_view, bounds];
        let native_view: *mut AnyObject = msg_send![view_class, alloc];
        let native_view: *mut AnyObject = msg_send![native_view, initWithFrame: content_bounds];
        anyhow::ensure!(!native_view.is_null(), "AppKit did not create the view");
        if options.transparent {
            let () = msg_send![native_window, setOpaque: Bool::NO];
            // WARNING: not `clearColor`, which breaks the window shadow.
            let color = NSColor::colorWithSRGBRed_green_blue_alpha(0., 0., 0., 0.0001);
            let () = msg_send![native_window, setBackgroundColor: &*color];
        }
        let state = Rc::new(RefCell::new(State {
            callbacks,
            native_window,
            native_view: NonNull::new_unchecked(native_view),
            layer,
            mirror,
            frames,
            glass: Vec::new(),
            controls: Default::default(),
            motion: None,
            cursor: CursorStyle::Arrow,
            last_key_equivalent: None,
            last_modifiers: None,
            keystroke_for_do_command: None,
            do_command_handled: None,
            external_files_dragged: false,
            synthetic_drag_counter: 0,
            first_mouse: false,
            last_press: None,
            app_owns_titlebar_drag: options.titlebar_hidden || options.frameless,
            traffic_light_position: options
                .traffic_light_position
                .filter(|_| options.titlebar_hidden),
            traffic_light_frames: None,
            pre_fullscreen_traffic_light_frames: None,
            transparent_titlebar: options.titlebar_hidden || options.frameless,
            is_exiting_fullscreen: false,
            closed: false,
            focusable: true,
            aspect: None,
            parent: None,
            sheet: false,
        }));
        for object in [native_window, native_view] {
            (*object)
                .class()
                .instance_variable(STATE_IVAR)
                .unwrap()
                .load_ptr::<*mut c_void>(&*object)
                .write(Rc::into_raw(state.clone()).cast_mut().cast());
        }
        let () = msg_send![native_window, setDelegate: native_window];
        set_title(native_window, &state, &options.title);
        let () = msg_send![native_window, setMovable: Bool::new(options.movable)];
        if options.titlebar_hidden || options.frameless {
            let () = msg_send![native_window, setTitlebarAppearsTransparent: Bool::YES];
            let () = msg_send![native_window, setTitleVisibility: NSWindowTitleVisibility::Hidden];
        }
        let () = msg_send![native_view, setAutoresizingMask: NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable];
        // WARNING: views become layer backed on their own after joining a window, which breaks the
        // Metal layer, so this one is layer backed from the start.
        let () = msg_send![native_view, setWantsLayer: Bool::YES];
        let () = msg_send![native_view, setLayerContentsRedrawPolicy: NSViewLayerContentsRedrawPolicy::DuringViewResize];
        // WARNING: during a resize a frame can show with the bounds of another size. Here it keeps its
        // place, where the default placement would stretch the whole window for that frame. AppKit
        // owns the gravity of this layer, so setting it on the layer does nothing.
        let () = msg_send![native_view, setLayerContentsPlacement: NSViewLayerContentsPlacement::TopLeft];
        // WARNING: AppKit owns the sublayers of the frame layer: it puts the layers of subviews over the ones
        // added by hand whenever a subview changes. So promoted layers and the caret go in the layer this view
        // hosts, and the order from the bottom is: frame, glass views, this view, web views and other subviews.
        // Native glass has nothing painted over it, and web views stay on top, as the renderer can't see them.
        let overlay_view: *mut AnyObject = msg_send![overlay_class, alloc];
        let overlay_view: *mut AnyObject = msg_send![overlay_view, initWithFrame: content_bounds];
        anyhow::ensure!(!overlay_view.is_null(), "AppKit did not create the view");
        let () = msg_send![overlay_view, setLayer: &*overlay];
        let () = msg_send![overlay_view, setWantsLayer: Bool::YES];
        let () = msg_send![overlay_view, setAutoresizingMask: NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable];
        let () = msg_send![native_view, addSubview: overlay_view];
        let () = msg_send![overlay_view, release];
        let native_view: *mut AnyObject = msg_send![native_view, autorelease];
        let () = msg_send![content_view, addSubview: native_view];
        let _: Bool = msg_send![native_window, makeFirstResponder: native_view];
        let () = msg_send![native_window, setLevel: NSNormalWindowLevel];
        let () = msg_send![native_window, setAcceptsMouseMovedEvents: Bool::NO];
        add_mouse_tracking_area(&*native_view.cast::<NSView>());
        if options.show {
            let () = msg_send![native_window, makeKeyAndOrderFront: ptr::null_mut::<AnyObject>()];
        }
        // WARNING: the origin from `initWithContentRect` can be off when the key screen is not the primary one.
        let () = msg_send![native_window, setFrameTopLeftPoint: rect.origin];
        if options.frameless {
            set_button_visibility(&state, false);
        }
        let lock = state.borrow();
        lock.update_mirror();
        lock.locate_frames();
        drop(lock);
        state.borrow_mut().move_traffic_light();
        // WARNING: the two strong references live in the ivars of the window and the view.
        drop(state);
        Ok((native_window, NonNull::new_unchecked(native_view)))
    })
}

fn on_main(native: &NativeRef, f: impl FnOnce(&NSWindow, &Shared)) {
    let window = native.window();
    if window.is_null() {
        return;
    }
    // SAFETY: `native` keeps the window, and so its state, alive.
    let Some(state) = (unsafe { get_state(&*window) }) else {
        return;
    };
    if state.try_borrow().is_ok_and(|state| state.closed) {
        return;
    }
    // SAFETY: the object is an NSWindow.
    f(unsafe { &*window.cast::<NSWindow>() }, &state);
}

fn set_title(window: *mut AnyObject, state: &Shared, title: &str) {
    // SAFETY: plain AppKit calls on a live window.
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let title = ns_string(title);
        let () = msg_send![app, changeWindowsItem: window, title: title, filename: false];
        let () = msg_send![window, setTitle: title];
    }
    if let Ok(mut state) = state.try_borrow_mut() {
        state.move_traffic_light();
    }
}

fn set_button_visibility(state: &Shared, visible: bool) {
    if let Some(buttons) = state.borrow().traffic_light_buttons() {
        for button in [buttons.close, buttons.minimize, buttons.zoom] {
            button.setHidden(!visible);
        }
    }
}

type Open = Box<dyn FnOnce(&NativeRef) + Send>;

// WARNING: the strong reference to a native window, which the main thread opens after `Window::open` returns, so
// the render thread runs page scripts meanwhile. The main thread opens it at once if it needs it first, and the
// render thread waits for it before it draws or hands out the view. AppKit must release it on the main thread.
#[derive(Default)]
struct NativeRef {
    window: AtomicPtr<AnyObject>,
    view: AtomicPtr<AnyObject>,
    open: Mutex<Option<Open>>,
}

impl NativeRef {
    // On the main thread: the window, opened now if it isn't yet, or null if AppKit didn't make it.
    fn window(&self) -> *mut AnyObject {
        let open = self
            .open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(open) = open {
            open(self);
        }
        self.window.load(Ordering::Acquire)
    }

    // Waits until the main thread opened the window, or on main opens it now.
    fn wait(self: &Arc<Self>) {
        if self
            .open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
        {
            let native = self.clone();
            native_sync(move || {
                native.window();
            });
        }
    }
}

impl Drop for NativeRef {
    fn drop(&mut self) {
        let window = *self.window.get_mut();
        if window.is_null() {
            return;
        }
        let window = Sendable::new(window);
        // SAFETY: this balances the `alloc` of `open_native`.
        native_post(Box::new(move || unsafe {
            let () = msg_send![window.take(), release];
        }));
    }
}

#[derive(Clone)]
pub struct WindowControl {
    native: Arc<NativeRef>,
    mirror: Arc<Mutex<Mirror>>,
}

// WARNING: a frame that the main thread has to show keeps its drawables. Only the newest one waits, and a newer
// frame drops it unseen, so a busy main thread never makes the render thread wait for a drawable.
#[derive(Default)]
struct Waiting {
    present: Mutex<Option<Present>>,
    queued: AtomicBool,
}

type ControlList = (Vec<crate::NativeControl>, Option<crate::ControlSink>);

// WARNING: the newest list of native controls, which the main thread takes whole, so a busy main thread skips lists.
#[derive(Default)]
struct Controls {
    sent: std::cell::RefCell<Vec<crate::NativeControl>>,
    next: Arc<Mutex<Option<ControlList>>>,
    queued: Arc<AtomicBool>,
}

pub(crate) struct Window {
    control: WindowControl,
    callbacks: Callbacks,
    renderer: RefCell<Renderer>,
    atlas: Rc<MetalAtlas>,
    frames: Arc<Mutex<Frames>>,
    quiet: std::cell::Cell<Option<crate::app::Timer>>,
    waiting: Arc<Waiting>,
    controls: Controls,
    transparent: bool,
}

impl std::ops::Deref for Window {
    type Target = WindowControl;
    fn deref(&self) -> &WindowControl {
        &self.control
    }
}

impl Window {
    pub fn open(options: &WindowOptions, callbacks: Callbacks) -> Result<Self> {
        let mut renderer = Renderer::new(options.transparent)?;
        if options.transparent {
            renderer.update_transparency(true);
        }
        let atlas = renderer.atlas().clone();
        let frames = Arc::new(Mutex::new(Frames {
            source: {
                let tick = Arc::new(Tick {
                    callbacks,
                    pending: AtomicBool::new(false),
                });
                WindowFrameSource::new(Arc::into_raw(tick) as *mut c_void, self::tick, free_tick)
            },
            wants: false,
            visible: false,
            display: None,
            closed: false,
        }));
        let mirror = Arc::new(Mutex::new(estimate(options)));
        let layers = Sendable::new((renderer.layer().clone(), renderer.overlay().clone()));
        let transparent = options.transparent;
        let (options, shared) = (options.clone(), (frames.clone(), mirror.clone()));
        let open: Open = Box::new(move |native| {
            let (layer, overlay) = layers.take();
            let (frames, mirror) = shared;
            let opened = autoreleasepool(|_| {
                open_native(&options, callbacks, layer, overlay, frames, mirror)
            });
            let (window, view) = match opened {
                Ok(opened) => opened,
                Err(error) => return eprintln!("apps: {error:#}"),
            };
            native.view.store(view.as_ptr(), Ordering::Release);
            native.window.store(window, Ordering::Release);
            measure_screens();
            // SAFETY: the window was just made. WARNING: the render thread laid out with the guess, and main waits for
            // a change to send the first state, so both hear of the real size.
            if let Some(state) = unsafe { get_state(&*window) } {
                update_size(&state);
            }
        });
        let native = Arc::new(NativeRef::default());
        *native.open.lock().unwrap_or_else(PoisonError::into_inner) = Some(open);
        let posted = native.clone();
        native_post(Box::new(move || {
            posted.window();
        }));
        Ok(Self {
            control: WindowControl { native, mirror },
            callbacks,
            renderer: RefCell::new(renderer),
            atlas,
            frames,
            quiet: std::cell::Cell::new(None),
            waiting: Arc::default(),
            controls: Controls::default(),
            transparent,
        })
    }

    pub(crate) fn control(&self) -> &WindowControl {
        &self.control
    }

    pub fn atlas(&self) -> &Rc<MetalAtlas> {
        &self.atlas
    }

    pub fn scale_factor(&self) -> f32 {
        self.control.mirror().scale
    }

    pub fn content_size(&self) -> Size<Pixels> {
        self.control.mirror().content
    }

    pub fn request_frame(&self) {
        let mut frames = self.frames.lock().unwrap_or_else(PoisonError::into_inner);
        frames.wants = true;
        frames.sync();
    }

    pub(crate) fn stop_frames(&self) {
        {
            let mut frames = self.frames.lock().unwrap_or_else(PoisonError::into_inner);
            frames.wants = false;
            frames.sync();
        }
        self.renderer.borrow_mut().trim();
        let callbacks = self.callbacks;
        self.quiet.set(Some(crate::app::timer(QUIET, move |_| {
            callbacks.release();
        })));
    }

    pub(crate) fn release(&self) {
        self.renderer.borrow_mut().release();
    }

    pub(crate) fn set_drawable_size(&self, size: Size<Pixels>, scale: f32) {
        let pixels = size.to_device_pixels(scale);
        self.renderer
            .borrow_mut()
            .set_drawable_size(CGSize::new(pixels.width.0 as f64, pixels.height.0 as f64));
    }

    pub(crate) fn take_present(&self) -> Option<Present> {
        self.renderer.borrow_mut().take_present()
    }

    pub(crate) fn set_presents_with_transaction(&self, on: bool) {
        self.renderer.borrow_mut().set_presents_with_transaction(on);
    }

    pub fn force_frame(&self) {
        let callbacks = self.callbacks;
        post(callbacks.thread, Box::new(move || step(callbacks)));
    }

    pub fn draw(&self, scene: &Scene, capture: bool) -> Option<Result<([u32; 2], Vec<u8>)>> {
        self.control.native.wait();
        self.quiet.take();
        let mut renderer = self.renderer.borrow_mut();
        let waiting = &self.waiting;
        let stale = waiting
            .present
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(stale) = stale {
            renderer.discard(stale);
        }
        let result = renderer.draw(scene, capture);
        let native = self.control.native.clone();
        let update = renderer.take_glass_update(move |surfaces| {
            on_main(&native, |_, state| {
                // WARNING: a caller up the stack can hold the state, and the next update brings the views up to date.
                if let Ok(mut state) = state.try_borrow_mut() {
                    // SAFETY: the window retains its view.
                    let parent = unsafe { state.native_view.cast::<NSView>().as_ref() };
                    super::glass::update(&mut state.glass, parent, surfaces);
                }
            })
        });
        if let Some(update) = update {
            *waiting
                .present
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(update);
            if !waiting.queued.swap(true, Ordering::AcqRel) {
                let waiting = waiting.clone();
                native_post(Box::new(move || {
                    waiting.queued.store(false, Ordering::Release);
                    let present = waiting
                        .present
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .take();
                    drop(present);
                }));
            }
        }
        result
    }

    pub fn promote(
        &self,
        key: u64,
        index: usize,
        region: [i32; 4],
        timing: [f64; 3],
        opacities: &[f32],
        transforms: &[[f64; 16]],
    ) -> bool {
        self.renderer
            .borrow_mut()
            .promote(key, index, region, timing, opacities, transforms)
    }

    pub fn demote(&self) {
        self.renderer.borrow_mut().demote();
    }

    pub fn set_caret(
        &self,
        caret: Option<(Bounds<crate::ScaledPixels>, crate::Rgba, std::time::Instant)>,
    ) {
        self.renderer.borrow_mut().set_caret(caret);
    }

    pub fn is_active(&self) -> bool {
        self.control.mirror().active
    }

    pub fn set_controls(&self, list: Vec<crate::NativeControl>, sink: Option<crate::ControlSink>) {
        let controls = &self.controls;
        if *controls.sent.borrow() == list {
            return;
        }
        controls.sent.replace(list.clone());
        *controls.next.lock().unwrap_or_else(PoisonError::into_inner) = Some((list, sink));
        if !controls.queued.swap(true, Ordering::AcqRel) {
            post_controls(
                self.control.native.clone(),
                controls.next.clone(),
                controls.queued.clone(),
            );
        }
    }

    pub fn state(&self) -> WindowState {
        self.control.mirror().state
    }

    pub fn change(&self, change: WindowChange) {
        if let WindowChange::Vibrancy(material, _) = change {
            self.renderer
                .borrow_mut()
                .update_transparency(material.is_some() || self.transparent);
        }
        self.control.change(change);
    }
}

impl WindowControl {
    fn mirror(&self) -> Mirror {
        *self.mirror.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn is_active(&self) -> bool {
        self.mirror().active
    }

    pub fn set_title(&self, title: &str) {
        let title = title.to_owned();
        self.later(move |window, state| {
            set_title(ptr::from_ref(window).cast_mut().cast(), state, &title)
        });
    }

    pub fn set_button_visibility(&self, visible: bool) {
        self.later(move |_, state| set_button_visibility(state, visible));
    }

    pub fn set_button_position(&self, position: Option<Point<Pixels>>) {
        self.later(move |_, state| {
            let mut state = state.borrow_mut();
            state.traffic_light_position = position;
            if position.is_some() {
                state.move_traffic_light();
            } else {
                state.restore_traffic_light();
            }
        });
    }

    // WARNING: until the window opened, its mirror holds a guess, so on main the window opens first.
    pub fn state(&self) -> Option<WindowState> {
        if MainThreadMarker::new().is_some() {
            self.native.window();
        }
        Some(self.mirror().state)
    }

    fn later(&self, f: impl FnOnce(&NSWindow, &Shared) + Send + 'static) {
        let native = self.native.clone();
        native_post(Box::new(move || on_main(&native, f)));
    }

    pub fn change(&self, change: WindowChange) {
        let change = Sendable::new(change);
        self.later(move |window, state| {
            let change = change.take();
            let marker = MainThreadMarker::new().unwrap();
            let Ok(view) = state.try_borrow().map(|state| state.native_view) else {
                return;
            };
            let zoom = || window.standardWindowButton(NSWindowButton::ZoomButton);
            let style = |flag: NSWindowStyleMask, on: bool| {
                // WARNING: a style mask change resets the zoom button.
                let enabled = zoom().is_none_or(|zoom| zoom.isEnabled());
                let mut mask = window.styleMask();
                mask.set(flag, on);
                window.setStyleMask(mask);
                if let Some(zoom) = zoom() {
                    zoom.setEnabled(enabled);
                }
                if let Ok(mut state) = state.try_borrow_mut() {
                    state.move_traffic_light();
                }
            };
            match change {
                WindowChange::Frame(bounds) => {
                    let top = primary_screen_top(marker);
                    let (x, y) = (bounds.origin.x.0 as f64, bounds.origin.y.0 as f64);
                    let (width, height) = (bounds.size.width.0 as f64, bounds.size.height.0 as f64);
                    window.setFrame_display(
                        NSRect::new(
                            NSPoint::new(x, top - (y + height)),
                            NSSize::new(width, height),
                        ),
                        true,
                    );
                }
                WindowChange::Center => window.center(),
                WindowChange::Show(focus) => {
                    if !window.isVisible() {
                        // SAFETY: the view is retained by the window.
                        unsafe { view.cast::<NSView>().as_ref() }.setNeedsDisplay(true);
                    }
                    if !attach(window, state) {
                        if focus {
                            window.makeKeyAndOrderFront(None);
                        } else {
                            window.orderFrontRegardless();
                        }
                    }
                }
                WindowChange::Hide => {
                    detach(window);
                    window.orderOut(None)
                }
                WindowChange::Blur => window.orderBack(None),
                WindowChange::Restore => window.deminiaturize(None),
                WindowChange::SizeLimits(min, max) => {
                    let limit = |value: Pixels| {
                        if value > px(0.) {
                            value.0 as f64
                        } else {
                            f32::MAX as f64
                        }
                    };
                    window.setMinSize(NSSize::new(min.width.0 as f64, min.height.0 as f64));
                    window.setMaxSize(NSSize::new(limit(max.width), limit(max.height)));
                }
                WindowChange::AlwaysOnTop(on) => window.setLevel(if on {
                    NSFloatingWindowLevel
                } else {
                    NSNormalWindowLevel
                }),
                WindowChange::Resizable(on) => style(NSWindowStyleMask::Resizable, on),
                WindowChange::Minimizable(on) => style(NSWindowStyleMask::Miniaturizable, on),
                WindowChange::Closable(on) => style(NSWindowStyleMask::Closable, on),
                WindowChange::Maximizable(on) => {
                    if let Some(zoom) = zoom() {
                        zoom.setEnabled(on);
                    }
                }
                WindowChange::Movable(on) => window.setMovable(on),
                WindowChange::Background(color) => {
                    window.setBackgroundColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(
                        color.r as f64,
                        color.g as f64,
                        color.b as f64,
                        color.a as f64,
                    )))
                }
                WindowChange::Opacity(alpha) => window.setAlphaValue(alpha as f64),
                WindowChange::Shadow(on) => window.setHasShadow(on),
                WindowChange::IgnoreMouse(ignore, forward) => {
                    window.setIgnoresMouseEvents(ignore);
                    window.setAcceptsMouseMovedEvents(ignore && forward);
                }
                WindowChange::AllWorkspaces(all, fullscreen) => {
                    let mut behavior = window.collectionBehavior();
                    behavior.set(NSWindowCollectionBehavior::CanJoinAllSpaces, all);
                    behavior.set(NSWindowCollectionBehavior::FullScreenAuxiliary, fullscreen);
                    window.setCollectionBehavior(behavior);
                }
                WindowChange::ContentProtection(on) => window.setSharingType(if on {
                    NSWindowSharingType::None
                } else {
                    NSWindowSharingType::ReadOnly
                }),
                WindowChange::Focusable(on) => {
                    if let Ok(mut state) = state.try_borrow_mut() {
                        state.focusable = on;
                    }
                }
                WindowChange::CancelComposition => {
                    // SAFETY: the window retains its view, and discarding needs no answer.
                    unsafe {
                        let context: *mut AnyObject = msg_send![view.as_ptr(), inputContext];
                        if !context.is_null() {
                            let () = msg_send![context, discardMarkedText];
                        }
                    }
                    return;
                }
                WindowChange::AspectRatio(ratio, extra) => {
                    // WARNING: resize increments clear the aspect ratio.
                    if ratio > 0. {
                        window.setContentAspectRatio(NSSize::new(ratio as f64, 1.));
                    } else {
                        window.setResizeIncrements(NSSize::new(1., 1.));
                    }
                    if let Ok(mut state) = state.try_borrow_mut() {
                        state.aspect = (ratio > 0.).then(|| {
                            let extra = NSSize::new(extra.width.0 as f64, extra.height.0 as f64);
                            (ratio as f64, extra)
                        });
                    }
                }
                WindowChange::Parent(parent, sheet) => {
                    detach(window);
                    let parent = match parent {
                        // SAFETY: the runtime passes the view of a live window.
                        Some(rwh::RawWindowHandle::AppKit(handle)) => {
                            unsafe { handle.ns_view.cast::<NSView>().as_ref() }.window()
                        }
                        _ => None,
                    };
                    if let Ok(mut state) = state.try_borrow_mut() {
                        state.parent = parent.as_ref().map(objc2::rc::Weak::from_retained);
                        state.sheet = sheet;
                    }
                    if window.isVisible() {
                        attach(window, state);
                    }
                }
                WindowChange::Vibrancy(material, effect) => {
                    unsafe {
                        let object: &AnyObject = window;
                        object
                            .class()
                            .instance_variable(VISUAL_EFFECT_STATE_IVAR)
                            .unwrap()
                            .load_ptr::<isize>(object)
                            .write(effect);
                    }
                    let Some(content) = window.contentView() else {
                        return;
                    };
                    // SAFETY: plain AppKit calls; the effect view is always the bottom subview.
                    unsafe {
                        let bottom = content.subviews().firstObject().filter(|view| {
                            let effect: Bool =
                                msg_send![&**view, isKindOfClass: class!(NSVisualEffectView)];
                            effect.as_bool()
                        });
                        match (material, bottom) {
                            (None, Some(view)) => view.removeFromSuperview(),
                            (None, None) => {}
                            (Some(material), view) => {
                                let view = match view {
                                    Some(view) => view,
                                    None => {
                                        let view: *mut AnyObject =
                                            msg_send![class!(NSVisualEffectView), alloc];
                                        let view: *mut AnyObject =
                                            msg_send![view, initWithFrame: content.bounds()];
                                        let Some(view) = Retained::from_raw(view.cast::<NSView>())
                                        else {
                                            return;
                                        };
                                        view.setAutoresizingMask(
                                            NSAutoresizingMaskOptions::ViewWidthSizable
                                                | NSAutoresizingMaskOptions::ViewHeightSizable,
                                        );
                                        let () = msg_send![&*view, setBlendingMode: 0isize];
                                        content.addSubview_positioned_relativeTo(
                                            &view,
                                            NSWindowOrderingMode::Below,
                                            None,
                                        );
                                        view
                                    }
                                };
                                let () = msg_send![&*view, setMaterial: material];
                                let () = msg_send![&*view, setState: effect];
                            }
                        }
                    }
                }
            }
            report_change(state);
        });
    }

    pub fn minimize(&self) {
        self.later(|window, _| window.miniaturize(None));
    }

    pub fn zoom(&self) {
        self.later(|window, _| window.zoom(None));
    }

    pub fn toggle_fullscreen(&self) {
        self.later(|window, _| window.toggleFullScreen(None));
    }

    pub fn activate(&self) {
        self.change(WindowChange::Show(true));
    }

    pub fn resize(&self, size: Size<Pixels>) {
        self.later(move |window, _| {
            window.setContentSize(NSSize::new(size.width.0 as f64, size.height.0 as f64))
        });
    }

    pub fn start_window_move(&self) {
        self.later(|window, state| {
            let Some(event) = state
                .try_borrow()
                .ok()
                .and_then(|state| state.last_press.clone())
            else {
                return;
            };
            // SAFETY: the event was this window's, and it starts the drag loop. WARNING: the page
            // asks late, so the button can be up by now, and a drag without a button never ends.
            unsafe {
                let pressed: NSUInteger = msg_send![class!(NSEvent), pressedMouseButtons];
                if pressed & 1 != 0 {
                    let () = msg_send![window, performWindowDragWithEvent: &*event];
                }
            }
        });
    }

    pub fn titlebar_double_click(&self) {
        self.later(|window, _| {
            // SAFETY: NSUserDefaults reads and plain window actions.
            unsafe {
                let defaults: *mut AnyObject =
                    msg_send![class!(NSUserDefaults), standardUserDefaults];
                let domain: *mut AnyObject =
                    msg_send![defaults, persistentDomainForName: ns_string("NSGlobalDomain")];
                let action: *mut AnyObject = if domain.is_null() {
                    ptr::null_mut()
                } else {
                    msg_send![domain, objectForKey: ns_string("AppleActionOnDoubleClick")]
                };
                let style = window.styleMask();
                match action.to_str() {
                    "None" => {}
                    "Minimize" if style.contains(NSWindowStyleMask::Miniaturizable) => {
                        window.miniaturize(None)
                    }
                    "Minimize" => {}
                    _ if style.contains(NSWindowStyleMask::Resizable) => window.zoom(None),
                    _ => {}
                }
            }
        });
    }

    pub fn raw_window_handle(&self) -> Result<rwh::RawWindowHandle> {
        self.native.wait();
        let view = NonNull::new(self.native.view.load(Ordering::Acquire).cast())
            .context("The window did not open")?;
        Ok(rwh::RawWindowHandle::AppKit(rwh::AppKitWindowHandle::new(
            view,
        )))
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        {
            let mut frames = self.frames.lock().unwrap_or_else(PoisonError::into_inner);
            frames.closed = true;
            frames.sync();
        }
        let control = self.control.clone();
        native_post(Box::new(move || {
            // WARNING: a window that closes before it opened never opens.
            drop(
                control
                    .native
                    .open
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take(),
            );
            let native = control.native.window.load(Ordering::Acquire);
            if native.is_null() {
                return;
            }
            // SAFETY: the window stays retained until the posted close.
            unsafe {
                if let Some(state) = get_state(&*native)
                    && let Ok(mut state) = state.try_borrow_mut()
                {
                    state.closed = true;
                }
                let () = msg_send![native, setDelegate: ptr::null_mut::<AnyObject>()];
                // WARNING: a sheet that is not ended blocks its parent, a sheet shown next and quitting,
                // which can already be posted, so it ends now.
                detach(&*native.cast::<NSWindow>());
            }
            post_local(move || unsafe {
                let () = msg_send![control.native.window.load(Ordering::Acquire), close];
            });
        }));
    }
}

fn post_controls(
    native: Arc<NativeRef>,
    next: Arc<Mutex<Option<ControlList>>>,
    queued: Arc<AtomicBool>,
) {
    native_post(Box::new(move || {
        queued.store(false, Ordering::Release);
        on_main(&native, |_, state| {
            // WARNING: a caller up the stack can hold the state, so the list waits for the next pass.
            let Ok(mut state) = state.try_borrow_mut() else {
                if !queued.swap(true, Ordering::AcqRel) {
                    post_controls(native.clone(), next.clone(), queued.clone());
                }
                return;
            };
            let Some((list, sink)) = next.lock().unwrap_or_else(PoisonError::into_inner).take()
            else {
                return;
            };
            let state = &mut *state;
            // SAFETY: the window retains its view.
            let parent = unsafe { state.native_view.cast::<NSView>().as_ref() };
            super::controls::update(&mut state.controls, parent, list, sink);
        });
    }));
}

fn attach(window: &NSWindow, state: &Shared) -> bool {
    let Some((parent, sheet)) = state
        .try_borrow()
        .ok()
        .and_then(|state| Some((state.parent.as_ref()?.load()?, state.sheet)))
    else {
        return false;
    };
    if sheet {
        if window.sheetParent().is_none() {
            parent.beginSheet_completionHandler(window, None);
        }
    } else if window.parentWindow().as_deref() != Some(&*parent) {
        // WARNING: joining a parent resets its level.
        let level = parent.level();
        // SAFETY: both are live windows, and a window never becomes its own ancestor.
        unsafe { parent.addChildWindow_ordered(window, NSWindowOrderingMode::Above) };
        parent.setLevel(level);
    }
    sheet
}

fn detach(window: &NSWindow) {
    if let Some(parent) = window.sheetParent() {
        parent.endSheet(window);
    }
    if let Some(parent) = window.parentWindow() {
        parent.removeChildWindow(window);
    }
}

fn report_change(state: &Shared) {
    let callbacks = state.try_borrow().map(|state| {
        state.update_mirror();
        state.callbacks
    });
    if let Ok(callbacks) = callbacks {
        callbacks.changed();
    }
}

fn merge_motion(old: &mut PlatformInput, new: &PlatformInput) -> bool {
    match (old, new) {
        (PlatformInput::MouseMove(old), PlatformInput::MouseMove(new))
            if old.modifiers == new.modifiers && old.pressed_button == new.pressed_button =>
        {
            *old = new.clone();
            true
        }
        (PlatformInput::ScrollWheel(old), PlatformInput::ScrollWheel(new))
            if old.modifiers == new.modifiers
                && (old.phase == new.phase
                    || (old.phase == ScrollPhase::Began && new.phase == ScrollPhase::Changed)) =>
        {
            let phase = old.phase;
            let delta = match (old.delta, new.delta) {
                (ScrollDelta::Pixels(a), ScrollDelta::Pixels(b)) => ScrollDelta::Pixels(a + b),
                (ScrollDelta::Lines(a), ScrollDelta::Lines(b)) => ScrollDelta::Lines(a + b),
                _ => return false,
            };
            *old = new.clone();
            old.delta = delta;
            old.phase = phase;
            true
        }
        _ => false,
    }
}

fn send_pointer(state: &Shared, callbacks: Callbacks, input: PlatformInput) {
    let motion = matches!(
        input,
        PlatformInput::MouseMove(_) | PlatformInput::ScrollWheel(_)
    );
    let Ok(mut lock) = state.try_borrow_mut() else {
        callbacks.input_later(input);
        return;
    };
    if !motion {
        lock.motion = None;
        drop(lock);
        callbacks.input_later(input);
        return;
    }
    if let Some(slot) = &lock.motion
        && slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
            .is_some_and(|waiting| merge_motion(waiting, &input))
    {
        return;
    }
    let slot = Arc::new(Mutex::new(Some(input)));
    lock.motion = Some(slot.clone());
    drop(lock);
    callbacks.queue(move |window, app| {
        let input = slot.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(input) = input {
            dispatch(window, app, input);
        }
    });
}

pub(crate) fn set_cursor(style: CursorStyle) {
    // SAFETY: main thread AppKit reads; the windows are checked to be ours.
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let key: *mut AnyObject = msg_send![app, keyWindow];
        let main: *mut AnyObject = msg_send![app, mainWindow];
        let Some(window) = [key, main]
            .into_iter()
            .find(|window| !window.is_null() && is_ours(*window))
        else {
            return;
        };
        let Some(state) = get_state(&*window) else {
            return;
        };
        let Ok(mut state) = state.try_borrow_mut() else {
            return;
        };
        if state.cursor != style {
            state.cursor = style;
            let () = msg_send![state.native_window, invalidateCursorRectsForView: state.native_view.as_ptr()];
        }
    }
}

fn primary_screen_top(marker: MainThreadMarker) -> f64 {
    NSScreen::screens(marker)
        .iter()
        .next()
        .map_or(0., |screen| screen.frame().size.height)
}

unsafe fn is_ours(window: *mut AnyObject) -> bool {
    unsafe { msg_send![window, isKindOfClass: classes().0] }
}

unsafe fn get_state(object: &AnyObject) -> Option<Shared> {
    unsafe {
        let raw = *object
            .class()
            .instance_variable(STATE_IVAR)?
            .load::<*mut c_void>(object);
        if raw.is_null() {
            return None;
        }
        let state = Rc::from_raw(raw as *const RefCell<State>);
        let clone = state.clone();
        std::mem::forget(state);
        Some(clone)
    }
}

unsafe fn drop_state(object: &AnyObject) {
    unsafe {
        let ivar = object.class().instance_variable(STATE_IVAR).unwrap();
        let raw = *ivar.load::<*mut c_void>(object);
        if !raw.is_null() {
            drop(Rc::from_raw(raw as *const RefCell<State>));
            ivar.load_ptr::<*mut c_void>(object).write(ptr::null_mut());
        }
    }
}

fn callbacks(this: &AnyObject) -> Option<Callbacks> {
    // SAFETY: AppKit calls these methods on our classes only.
    let state = unsafe { get_state(this) }?;
    state.try_borrow().ok().map(|state| state.callbacks)
}

extern "C" fn can_become_key(this: &AnyObject, _: Sel) -> Bool {
    // SAFETY: AppKit calls this on our windows.
    Bool::new(
        unsafe { get_state(this) }
            .is_none_or(|state| state.try_borrow().map_or(true, |state| state.focusable)),
    )
}

extern "C" fn has_active_appearance(this: &AnyObject, selector: Sel) -> Bool {
    unsafe {
        match *this
            .class()
            .instance_variable(VISUAL_EFFECT_STATE_IVAR)
            .unwrap()
            .load::<isize>(this)
        {
            1 => Bool::YES,
            2 => Bool::NO,
            _ => this.send_super_message(class!(NSWindow), selector, ()),
        }
    }
}

extern "C" fn dealloc_window(this: &AnyObject, _: Sel) {
    unsafe {
        drop_state(this);
        let () = msg_send![super(this, class!(NSWindow)), dealloc];
    }
}

extern "C" fn dealloc_view(this: &AnyObject, _: Sel) {
    unsafe {
        drop_state(this);
        let () = msg_send![super(this, class!(NSView)), dealloc];
    }
}

fn add_mouse_tracking_area(view: &NSView) {
    let options = NSTrackingAreaOptions::MouseEnteredAndExited
        | NSTrackingAreaOptions::MouseMoved
        | NSTrackingAreaOptions::ActiveAlways
        | NSTrackingAreaOptions::InVisibleRect;
    // SAFETY: the view owns and retains the tracking area.
    let area = unsafe {
        NSTrackingArea::initWithRect_options_owner_userInfo(
            NSTrackingArea::alloc(),
            NSRect::ZERO,
            options,
            Some(view),
            None,
        )
    };
    view.addTrackingArea(&area);
}

extern "C" fn reset_cursor_rects(this: &AnyObject, _: Sel) {
    unsafe {
        let () = msg_send![super(this, class!(NSView)), resetCursorRects];
        let Some(state) = get_state(this) else {
            return;
        };
        let style = state
            .try_borrow()
            .map_or(CursorStyle::Arrow, |state| state.cursor);
        let cursor: *mut AnyObject = match style {
            CursorStyle::Arrow => msg_send![class!(NSCursor), arrowCursor],
            CursorStyle::IBeam => msg_send![class!(NSCursor), IBeamCursor],
            CursorStyle::Crosshair => msg_send![class!(NSCursor), crosshairCursor],
            CursorStyle::ClosedHand => msg_send![class!(NSCursor), closedHandCursor],
            CursorStyle::OpenHand => msg_send![class!(NSCursor), openHandCursor],
            CursorStyle::PointingHand => msg_send![class!(NSCursor), pointingHandCursor],
            CursorStyle::ResizeLeftRight | CursorStyle::ResizeColumn => {
                msg_send![class!(NSCursor), resizeLeftRightCursor]
            }
            CursorStyle::ResizeUpDown | CursorStyle::ResizeRow => {
                msg_send![class!(NSCursor), resizeUpDownCursor]
            }
            CursorStyle::ResizeLeft => msg_send![class!(NSCursor), resizeLeftCursor],
            CursorStyle::ResizeRight => msg_send![class!(NSCursor), resizeRightCursor],
            CursorStyle::ResizeUp => msg_send![class!(NSCursor), resizeUpCursor],
            CursorStyle::ResizeDown => msg_send![class!(NSCursor), resizeDownCursor],
            CursorStyle::ResizeUpLeftDownRight => {
                msg_send![class!(NSCursor), _windowResizeNorthWestSouthEastCursor]
            }
            CursorStyle::ResizeUpRightDownLeft => {
                msg_send![class!(NSCursor), _windowResizeNorthEastSouthWestCursor]
            }
            CursorStyle::IBeamCursorForVerticalLayout => {
                msg_send![class!(NSCursor), IBeamCursorForVerticalLayout]
            }
            CursorStyle::OperationNotAllowed => {
                msg_send![class!(NSCursor), operationNotAllowedCursor]
            }
            CursorStyle::DragLink => msg_send![class!(NSCursor), dragLinkCursor],
            CursorStyle::DragCopy => msg_send![class!(NSCursor), dragCopyCursor],
            CursorStyle::ContextualMenu => msg_send![class!(NSCursor), contextualMenuCursor],
            CursorStyle::None => {
                let () = msg_send![class!(NSCursor), setHiddenUntilMouseMoves: Bool::YES];
                return;
            }
        };
        let bounds: NSRect = msg_send![this, bounds];
        let () = msg_send![this, addCursorRect: bounds, cursor: cursor];
    }
}

extern "C" fn handle_key_equivalent(this: &AnyObject, _: Sel, event: *mut AnyObject) -> Bool {
    // WARNING: AppKit offers key equivalents to every view of the window. The web views of `<webview>`,
    // `<iframe>` and `WebContentsView` are subviews of this one, so their keys stay with them while one has focus.
    // SAFETY: plain AppKit calls on this view, its window and its subviews.
    unsafe {
        let window: *mut AnyObject = msg_send![this, window];
        let responder: *mut AnyObject = if window.is_null() {
            ptr::null_mut()
        } else {
            msg_send![window, firstResponder]
        };
        if !responder.is_null() && !ptr::eq(responder, this) {
            let view: Bool = msg_send![responder, isKindOfClass: class!(NSView)];
            if view.as_bool() {
                let inside: Bool = msg_send![responder, isDescendantOf: this];
                if inside.as_bool() {
                    return msg_send![super(this, class!(NSView)), performKeyEquivalent: event];
                }
            }
        }
    }
    handle_key_event(this, event, true)
}

extern "C" fn handle_key_down(this: &AnyObject, _: Sel, event: *mut AnyObject) {
    handle_key_event(this, event, false);
}

unsafe fn is_ime_input_source_active() -> bool {
    unsafe {
        let source = TISCopyCurrentKeyboardInputSource();
        if source.is_null() {
            return false;
        }
        let kind = TISGetInputSourceProperty(source, kTISPropertyInputSourceType as *const c_void);
        let input_mode = !kind.is_null()
            && CFEqual(kind as CFTypeRef, kTISTypeKeyboardInputMode as CFTypeRef) != 0;
        let ascii = TISGetInputSourceProperty(
            source,
            kTISPropertyInputSourceIsASCIICapable as *const c_void,
        );
        let ascii = !ascii.is_null() && CFBooleanGetValue(ascii as CFBooleanRef);
        CFRelease(source as CFTypeRef);
        input_mode && !ascii
    }
}

fn press_and_hold_enabled() -> bool {
    // SAFETY: an NSUserDefaults read.
    unsafe {
        let defaults: *mut AnyObject = msg_send![class!(NSUserDefaults), standardUserDefaults];
        let value: *mut AnyObject =
            msg_send![defaults, objectForKey: ns_string("ApplePressAndHoldEnabled")];
        value.is_null() || {
            let on: Bool = msg_send![value, boolValue];
            on.as_bool()
        }
    }
}

fn input_context_handles(this: &AnyObject, event: *mut AnyObject) -> bool {
    // SAFETY: AppKit calls these with the view's own key events.
    unsafe {
        let context: *mut AnyObject = msg_send![this, inputContext];
        if context.is_null() {
            return false;
        }
        let handled: Bool = msg_send![context, handleEvent: event];
        handled.as_bool()
    }
}

// WARNING: things to test when changing this:
//  U.S. layout: `alt-t` reaches the app; typing `j k` in a field inserts both characters.
//  Brazilian layout: `" space` types an unmarked quote, `" backspace` deletes the marked quote.
//  Japanese input: `a i left down up enter enter` types 愛, and `j i` types じ.
fn handle_key_event(this: &AnyObject, event: *mut AnyObject, key_equivalent: bool) -> Bool {
    // SAFETY: AppKit calls this on our views.
    let Some(state) = (unsafe { get_state(this) }) else {
        return Bool::NO;
    };
    let Ok(mut lock) = state.try_borrow_mut() else {
        return Bool::NO;
    };
    let height = lock.content_size().height;
    let callbacks = lock.callbacks;
    // SAFETY: AppKit passes an NSEvent.
    let Some(input) = (unsafe { platform_input_from_native(event, Some(height)) }) else {
        return Bool::NO;
    };
    let down = match &input {
        PlatformInput::KeyDown(down) => {
            // WARNING: AppKit sends some keys as a key equivalent, then again as a key down when that was
            // not handled. The second one is skipped.
            let same = |a: &KeyDownEvent, b: &KeyDownEvent| {
                a.keystroke == b.keystroke && a.is_held == b.is_held
            };
            if key_equivalent {
                lock.last_key_equivalent = Some(down.clone());
            } else if lock
                .last_key_equivalent
                .take()
                .is_some_and(|last| same(&last, down))
            {
                return Bool::NO;
            }
            true
        }
        PlatformInput::KeyUp(_) => false,
        _ => return Bool::NO,
    };
    drop(lock);
    let composing = down && callbacks.marked_text_range().is_some();
    // WARNING: the main thread's filter sees each key once, before the page. As in Electron, a key it blocks
    // still goes to a composing input method, which only can't send it on to the page.
    let blocked = crate::app::native::key(callbacks.id, &input, &|| {
        composing || !down && callbacks.marked_text_range().is_some()
    });
    match input {
        PlatformInput::KeyDown(down) => {
            let keystroke = &down.keystroke;
            let modifiers = keystroke.modifiers;
            let plain = !modifiers.control && !modifiers.function && !modifiers.platform;
            // WARNING: composed keys, keys without text (arrows, escape) and printable keys of a composing
            // input method go to the input method first. It calls `doCommandBySelector:` for keys
            // it does not want, which sends them to the app.
            let ime_printable = !composing
                && plain
                && keystroke
                    .key_char
                    .as_ref()
                    .is_some_and(|text| text.chars().all(|c| !c.is_control()))
                && unsafe { is_ime_input_source_active() }
                && callbacks.accepts_text_input();
            if blocked && !composing && !ime_printable {
                return Bool::YES;
            }
            if composing || ime_printable || (keystroke.key_char.is_none() && plain) {
                {
                    let mut lock = state.borrow_mut();
                    lock.keystroke_for_do_command = (!blocked).then(|| keystroke.clone());
                    lock.do_command_handled = None;
                }
                // WARNING: the page gets a key that the input method takes only with the first change it makes,
                // or after it when it made none. A key that comes back through `doCommandBySelector:` drops it.
                let held = !blocked && (composing || ime_printable);
                if held {
                    let down = down.clone();
                    callbacks.queue(move |window, _| window.ime_key = Some((down, composing)));
                }
                let handled = input_context_handles(this, event);
                let mut lock = state.borrow_mut();
                lock.keystroke_for_do_command = None;
                if blocked {
                    return Bool::YES;
                }
                if let Some(handled) = lock.do_command_handled.take() {
                    return Bool::new(handled);
                } else if handled {
                    if held {
                        callbacks.queue(|window, app| {
                            send_ime_key(window, app, false);
                        });
                    }
                    return Bool::YES;
                }
                drop(lock);
                return Bool::new(callbacks.input(PlatformInput::KeyDown(down)));
            }
            if callbacks.input(PlatformInput::KeyDown(down.clone())) {
                return Bool::YES;
            }
            if down.is_held
                && let Some(text) = &down.keystroke.key_char
                && !press_and_hold_enabled()
            {
                callbacks.replace_text_in_range(None, text);
                return Bool::YES;
            }
            // WARNING: key equivalents with modifiers other than fn stay with AppKit, or cmd-` stops working.
            if key_equivalent
                && modifiers
                    != (Modifiers {
                        function: true,
                        ..Default::default()
                    })
            {
                return Bool::NO;
            }
            Bool::new(input_context_handles(this, event))
        }
        input => Bool::new(blocked || callbacks.input(input)),
    }
}

extern "C" fn handle_view_event(this: &AnyObject, _: Sel, event: *mut AnyObject) {
    // SAFETY: AppKit calls this on our views.
    let Some(state) = (unsafe { get_state(this) }) else {
        return;
    };
    let Ok(mut lock) = state.try_borrow_mut() else {
        return;
    };
    let height = lock.content_size().height;
    let callbacks = lock.callbacks;
    // SAFETY: AppKit passes an NSEvent.
    let Some(mut input) = (unsafe { platform_input_from_native(event, Some(height)) }) else {
        return;
    };
    match &mut input {
        PlatformInput::MouseDown(
            down @ MouseDownEvent {
                button: MouseButton::Left,
                modifiers: Modifiers { control: true, .. },
                ..
            },
        ) => {
            down.button = MouseButton::Right;
            down.modifiers.control = false;
            down.click_count = 1;
        }
        PlatformInput::MouseDown(
            down @ MouseDownEvent {
                button: MouseButton::Left,
                ..
            },
        ) if lock.first_mouse => {
            down.first_mouse = true;
            lock.first_mouse = false;
        }
        PlatformInput::MouseUp(
            up @ MouseUpEvent {
                button: MouseButton::Left,
                modifiers: Modifiers { control: true, .. },
                ..
            },
        ) => {
            up.button = MouseButton::Right;
            up.modifiers.control = false;
            up.click_count = 1;
        }
        _ => {}
    }
    if matches!(
        input,
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            ..
        }) | PlatformInput::MouseMove(MouseMoveEvent {
            pressed_button: Some(MouseButton::Left),
            ..
        })
    ) {
        // SAFETY: AppKit passes a live NSEvent.
        lock.last_press = unsafe { Retained::retain(event) };
    }
    match &input {
        PlatformInput::MouseDown(_) => {
            drop(lock);
            // WARNING: this view takes key focus back from a web view, also for presses that glass views forward.
            // SAFETY: plain AppKit calls on this view and its window.
            unsafe {
                let window: *mut AnyObject = msg_send![this, window];
                if !window.is_null() {
                    let responder: *mut AnyObject = msg_send![window, firstResponder];
                    if !ptr::eq(responder, this) {
                        let _: Bool = msg_send![window, makeFirstResponder: this];
                    }
                }
            }
            input_context_handles(this, event);
        }
        PlatformInput::MouseMove(
            moved @ MouseMoveEvent {
                pressed_button: Some(_),
                ..
            },
        ) => {
            if !lock.external_files_dragged {
                lock.synthetic_drag_counter += 1;
                synthetic_drag(
                    Rc::downgrade(&state),
                    lock.synthetic_drag_counter,
                    moved.clone(),
                );
            }
            drop(lock);
        }
        PlatformInput::MouseUp(_) => {
            lock.synthetic_drag_counter += 1;
            drop(lock);
        }
        PlatformInput::ModifiersChanged(changed) => {
            if lock.last_modifiers.as_ref().is_some_and(|last| {
                last.modifiers == changed.modifiers && last.capslock == changed.capslock
            }) {
                return;
            }
            lock.last_modifiers = Some(changed.clone());
            drop(lock);
        }
        _ => drop(lock),
    }
    send_pointer(&state, callbacks, input);
}

fn synthetic_drag(state: Weak<RefCell<State>>, id: usize, event: MouseMoveEvent) {
    native_after(Duration::from_millis(16), move || {
        let Some(state) = state.upgrade() else {
            return;
        };
        let callbacks = {
            let Ok(mut lock) = state.try_borrow_mut() else {
                return;
            };
            if lock.synthetic_drag_counter != id {
                return;
            }
            // WARNING: A menu can take the mouse up, so the button must still be down.
            let pressed: NSUInteger = unsafe { msg_send![class!(NSEvent), pressedMouseButtons] };
            let bit = match event.pressed_button {
                Some(MouseButton::Left) | None => 0,
                Some(MouseButton::Right) => 1,
                Some(MouseButton::Middle) => 2,
                Some(MouseButton::Navigate(NavigationDirection::Back)) => 3,
                Some(MouseButton::Navigate(NavigationDirection::Forward)) => 4,
            };
            if pressed & (1 << bit) == 0 {
                lock.synthetic_drag_counter += 1;
                return;
            }
            lock.callbacks
        };
        send_pointer(&state, callbacks, PlatformInput::MouseMove(event.clone()));
        synthetic_drag(Rc::downgrade(&state), id, event);
    });
}

extern "C" fn window_did_change_occlusion_state(this: &AnyObject, _: Sel, _: *mut AnyObject) {
    // SAFETY: AppKit calls this on our windows.
    let Some(state) = (unsafe { get_state(this) }) else {
        return;
    };
    if let Ok(lock) = state.try_borrow() {
        lock.locate_frames();
        if !lock.is_visible() {
            lock.callbacks.release();
        }
    }
    if let Ok(mut lock) = state.try_borrow_mut()
        && lock.is_visible()
    {
        lock.move_traffic_light();
    }
    report_change(&state);
}

extern "C" fn window_did_resize(this: &AnyObject, _: Sel, _: *mut AnyObject) {
    // SAFETY: AppKit calls this on our windows.
    if let Some(state) = unsafe { get_state(this) }
        && let Ok(mut lock) = state.try_borrow_mut()
    {
        lock.move_traffic_light();
    }
}

extern "C" fn window_will_resize(
    this: &AnyObject,
    _: Sel,
    _: *mut AnyObject,
    proposed: NSSize,
) -> NSSize {
    // SAFETY: AppKit calls this on our windows.
    let Some((ratio, extra)) =
        unsafe { get_state(this) }.and_then(|state| state.try_borrow().ok()?.aspect)
    else {
        return proposed;
    };
    // SAFETY: the receiver is an NSWindow.
    let window = unsafe { &*(this as *const AnyObject).cast::<NSWindow>() };
    let frame = window.frame();
    let content = window.contentRectForFrameRect(frame).size;
    let extra_width = frame.size.width - content.width + extra.width;
    let extra_height = frame.size.height - content.height + extra.height;
    let width = ((proposed.height - extra_height) * ratio + extra_width).round();
    NSSize::new(
        width,
        ((width - extra_width) / ratio + extra_height).round(),
    )
}

fn is_macos_at_least(major: NSInteger, minor: NSInteger) -> bool {
    NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(NSOperatingSystemVersion {
        majorVersion: major,
        minorVersion: minor,
        patchVersion: 0,
    })
}

extern "C" fn window_will_enter_fullscreen(this: &AnyObject, _: Sel, _: *mut AnyObject) {
    // SAFETY: AppKit calls this on our windows.
    let Some(state) = (unsafe { get_state(this) }) else {
        return;
    };
    let Ok(mut lock) = state.try_borrow_mut() else {
        return;
    };
    lock.is_exiting_fullscreen = false;
    lock.pre_fullscreen_traffic_light_frames = lock.traffic_light_frames;
    lock.restore_traffic_light();
    if is_macos_at_least(15, 3) {
        // SAFETY: a plain window property.
        unsafe {
            let () = msg_send![lock.native_window, setTitlebarAppearsTransparent: Bool::NO];
        }
    }
}

extern "C" fn window_will_exit_fullscreen(this: &AnyObject, _: Sel, _: *mut AnyObject) {
    // SAFETY: AppKit calls this on our windows.
    let Some(state) = (unsafe { get_state(this) }) else {
        return;
    };
    let Ok(mut lock) = state.try_borrow_mut() else {
        return;
    };
    lock.is_exiting_fullscreen = true;
    if is_macos_at_least(15, 3) && lock.transparent_titlebar {
        // SAFETY: a plain window property.
        unsafe {
            let () = msg_send![lock.native_window, setTitlebarAppearsTransparent: Bool::YES];
        }
    }
    lock.move_traffic_light();
}

extern "C" fn window_did_fail_to_exit_fullscreen(this: &AnyObject, _: Sel, _: *mut AnyObject) {
    // SAFETY: AppKit calls this on our windows.
    if let Some(state) = unsafe { get_state(this) }
        && let Ok(mut lock) = state.try_borrow_mut()
    {
        lock.is_exiting_fullscreen = false;
        lock.restore_traffic_light();
    }
}

extern "C" fn window_did_exit_fullscreen(this: &AnyObject, _: Sel, _: *mut AnyObject) {
    // SAFETY: AppKit calls this on our windows.
    let Some(state) = (unsafe { get_state(this) }) else {
        return;
    };
    if let Ok(mut lock) = state.try_borrow_mut() {
        lock.is_exiting_fullscreen = false;
        // WARNING: buttons moved during the transition hold their fullscreen frames.
        lock.traffic_light_frames = lock.pre_fullscreen_traffic_light_frames.take();
        lock.move_traffic_light();
    }
    report_change(&state);
}

extern "C" fn window_did_change_state(this: &AnyObject, _: Sel, _: *mut AnyObject) {
    // SAFETY: AppKit calls this on our windows.
    if let Some(state) = unsafe { get_state(this) } {
        report_change(&state);
    }
}

fn update_size(state: &Shared) {
    let Ok(lock) = state.try_borrow() else {
        return;
    };
    let scale = lock.scale_factor();
    let content = lock.content_size();
    lock.layer.setContentsScale(scale as f64);
    lock.update_mirror();
    let callbacks = lock.callbacks;
    drop(lock);
    callbacks.resized(content, scale);
}

extern "C" fn window_did_change_screen(this: &AnyObject, _: Sel, _: *mut AnyObject) {
    // SAFETY: AppKit calls this on our windows.
    let Some(state) = (unsafe { get_state(this) }) else {
        return;
    };
    if let Ok(lock) = state.try_borrow() {
        lock.locate_frames();
    }
    update_size(&state);
    report_change(&state);
}

extern "C" fn window_did_change_key_status(this: &AnyObject, selector: Sel, _: *mut AnyObject) {
    // SAFETY: AppKit calls this on our windows.
    let Some(state) = (unsafe { get_state(this) }) else {
        return;
    };
    let Ok(lock) = state.try_borrow() else {
        return;
    };
    let active = lock.native_window().isKeyWindow();
    // WARNING: AppKit can send a spurious `windowDidBecomeKey` to the previous key window when a popup
    // opens while the app is inactive, which later keeps that window from being activated.
    if selector == sel!(windowDidBecomeKey:) && !active {
        let window = lock.native_window;
        drop(lock);
        // SAFETY: balances the spurious key status.
        unsafe {
            let () = msg_send![window, resignKeyWindow];
        }
        return;
    }
    drop(lock);
    if active && let Ok(mut lock) = state.try_borrow_mut() {
        lock.move_traffic_light();
    }
    report_change(&state);
}

extern "C" fn window_should_close(this: &AnyObject, _: Sel, _: *mut AnyObject) -> Bool {
    Bool::new(callbacks(this).is_none_or(|callbacks| callbacks.should_close()))
}

extern "C" fn close_window(this: &AnyObject, _: Sel) {
    // SAFETY: AppKit calls this on our windows.
    unsafe {
        if let Some(state) = get_state(this)
            && let Ok(mut lock) = state.try_borrow_mut()
        {
            lock.closed = true;
            let mut frames = lock.frames.lock().unwrap_or_else(PoisonError::into_inner);
            frames.closed = true;
            frames.sync();
        }
        let () = msg_send![super(this, class!(NSWindow)), close];
    }
}

extern "C" fn make_backing_layer(this: &AnyObject, _: Sel) -> *mut AnyObject {
    // SAFETY: AppKit calls this on our views.
    let Some(state) = (unsafe { get_state(this) }) else {
        return ptr::null_mut();
    };

    Retained::as_ptr(&state.borrow().layer) as *mut AnyObject
}

extern "C" fn view_did_change_backing_properties(this: &AnyObject, _: Sel) {
    // SAFETY: AppKit calls this on our views.
    if let Some(state) = unsafe { get_state(this) } {
        update_size(&state);
    }
}

extern "C" fn set_frame_size(this: &AnyObject, _: Sel, new: NSSize) {
    // SAFETY: AppKit calls this on our views.
    let Some(state) = (unsafe { get_state(this) }) else {
        return;
    };
    // SAFETY: a plain view property.
    let old: NSRect = unsafe { msg_send![this, frame] };
    if old.size.width == new.width && old.size.height == new.height {
        return;
    }
    // SAFETY: the NSView implementation.
    unsafe {
        let () = msg_send![super(this, class!(NSView)), setFrameSize: new];
    }
    update_size(&state);
}

extern "C" fn display_layer(this: &AnyObject, _: Sel, _: *mut AnyObject) {
    let Some(callbacks) = callbacks(this) else {
        return;
    };
    // WARNING: the window server takes the new window size at once. Until the commit of this
    // transaction it shows the old layers displaced by the resize, so a frame that takes long moves
    // the whole window, even with its frame in sync. A late frame shows after the commit instead,
    // and only the bottom edge can tell. While one is on its way, more would only queue up.
    // A window that is not ordered in, like one created hidden, shows nothing, so its frame skips the
    // transaction and its wait. Ordering it in asks for a display again, which then presents in it.
    // SAFETY: plain view and window properties.
    let visible = unsafe {
        let window: *mut AnyObject = msg_send![this, window];
        !window.is_null() && {
            let visible: Bool = msg_send![window, isVisible];
            visible.as_bool()
        }
    };
    if callbacks.thread.late.load(Ordering::Acquire) == 0 {
        drop(ask_within(callbacks.thread, RESIZE_WAIT, move || {
            callbacks.with(move |window, _| window.platform.set_presents_with_transaction(visible));
            step(callbacks);
            callbacks.with(|window, _| {
                window.platform.set_presents_with_transaction(false);
                window.platform.take_present()
            })
        }));
    }
}

extern "C" fn valid_attributes_for_marked_text(_: &AnyObject, _: Sel) -> *mut AnyObject {
    // SAFETY: an empty autoreleased array.
    unsafe { msg_send![class!(NSArray), array] }
}

extern "C" fn has_marked_text(this: &AnyObject, _: Sel) -> Bool {
    Bool::new(
        callbacks(this)
            .and_then(|callbacks| callbacks.marked_text_range())
            .is_some(),
    )
}

extern "C" fn marked_range(this: &AnyObject, _: Sel) -> NSRange {
    callbacks(this)
        .and_then(|callbacks| callbacks.marked_text_range())
        .map_or(NSRange::invalid(), NSRange::from)
}

extern "C" fn selected_range(this: &AnyObject, _: Sel) -> NSRange {
    callbacks(this)
        .and_then(|callbacks| callbacks.selected_text_range())
        .map_or(NSRange::invalid(), |selection| selection.range.into())
}

fn get_frame(this: &AnyObject) -> NSRect {
    // SAFETY: AppKit calls this on our views.
    let Some(state) = (unsafe { get_state(this) }) else {
        return NSRect::ZERO;
    };
    let Ok(lock) = state.try_borrow() else {
        return NSRect::ZERO;
    };
    let mut frame = lock.native_window().frame();
    // SAFETY: plain window properties.
    unsafe {
        let layout: NSRect = msg_send![lock.native_window, contentLayoutRect];
        let style: NSWindowStyleMask = msg_send![lock.native_window, styleMask];
        if !style.contains(NSWindowStyleMask::FullSizeContentView) {
            frame.origin.y -= frame.size.height - layout.size.height;
        }
    }
    frame
}

extern "C" fn first_rect_for_character_range(
    this: &AnyObject,
    _: Sel,
    range: NSRange,
    _: *mut NSRange,
) -> NSRect {
    let frame = get_frame(this);
    callbacks(this)
        .and_then(|callbacks| callbacks.bounds_for_range(range.to_range()?))
        .map_or(NSRect::ZERO, |bounds| {
            NSRect::new(
                NSPoint::new(
                    frame.origin.x + bounds.origin.x.0 as f64,
                    frame.origin.y + frame.size.height
                        - bounds.origin.y.0 as f64
                        - bounds.size.height.0 as f64,
                ),
                NSSize::new(bounds.size.width.0 as f64, bounds.size.height.0 as f64),
            )
        })
}

unsafe fn text_of(text: *mut AnyObject) -> String {
    unsafe {
        let attributed: Bool = msg_send![text, isKindOfClass: class!(NSAttributedString)];
        let text: *mut AnyObject = if attributed.as_bool() {
            msg_send![text, string]
        } else {
            text
        };
        text.to_str().to_owned()
    }
}

extern "C" fn insert_text(this: &AnyObject, _: Sel, text: *mut AnyObject, replacement: NSRange) {
    // SAFETY: AppKit passes a string.
    let text = unsafe { text_of(text) };
    if let Some(callbacks) = callbacks(this) {
        callbacks.replace_text_in_range(replacement.to_range(), &text);
    }
}

extern "C" fn set_marked_text(
    this: &AnyObject,
    _: Sel,
    text: *mut AnyObject,
    selected: NSRange,
    replacement: NSRange,
) {
    // SAFETY: AppKit passes a string.
    let text = unsafe { text_of(text) };
    if let Some(callbacks) = callbacks(this) {
        callbacks.replace_and_mark_text_in_range(
            replacement.to_range(),
            &text,
            selected.to_range(),
        );
    }
}

extern "C" fn unmark_text(this: &AnyObject, _: Sel) {
    if let Some(callbacks) = callbacks(this) {
        callbacks.unmark_text();
    }
}

extern "C" fn attributed_substring_for_proposed_range(
    this: &AnyObject,
    _: Sel,
    range: NSRange,
    actual: *mut NSRange,
) -> *mut AnyObject {
    let Some(range) = range.to_range().filter(|range| !range.is_empty()) else {
        return ptr::null_mut();
    };
    let Some((text, adjusted)) =
        callbacks(this).and_then(|callbacks| callbacks.text_for_range(range.clone()))
    else {
        return ptr::null_mut();
    };
    // SAFETY: AppKit passes a writable range, and the string is autoreleased.
    unsafe {
        if let Some(adjusted) = adjusted
            && adjusted != range
            && !actual.is_null()
        {
            actual.write(NSRange::from(adjusted));
        }
        let string: *mut AnyObject = msg_send![class!(NSAttributedString), alloc];
        let string: *mut AnyObject = msg_send![string, initWithString: ns_string(&text)];
        msg_send![string, autorelease]
    }
}

extern "C" fn do_command_by_selector(this: &AnyObject, _: Sel, _: Sel) {
    // SAFETY: AppKit calls this on our views.
    let Some(state) = (unsafe { get_state(this) }) else {
        return;
    };
    let (keystroke, callbacks) = {
        let Ok(mut lock) = state.try_borrow_mut() else {
            return;
        };
        (lock.keystroke_for_do_command.take(), lock.callbacks)
    };
    if let Some(keystroke) = keystroke {
        let handled = callbacks.input(PlatformInput::KeyDown(KeyDownEvent {
            keystroke,
            is_held: false,
            ime: false,
        }));
        if let Ok(mut lock) = state.try_borrow_mut() {
            lock.do_command_handled = Some(handled);
        }
    }
}

extern "C" fn view_did_change_effective_appearance(this: &AnyObject, _: Sel) {
    // SAFETY: AppKit calls this on our views.
    let Some(state) = (unsafe { get_state(this) }) else {
        return;
    };
    let callbacks = state.try_borrow().ok().map(|state| state.callbacks);
    if let Some(callbacks) = callbacks {
        callbacks.appearance_changed();
    }
    if let Ok(mut lock) = state.try_borrow_mut() {
        lock.move_traffic_light();
    }
}

extern "C" fn accepts_first_mouse(this: &AnyObject, _: Sel, _: *mut AnyObject) -> Bool {
    // SAFETY: AppKit calls this on our views.
    if let Some(state) = unsafe { get_state(this) }
        && let Ok(mut lock) = state.try_borrow_mut()
    {
        lock.first_mouse = true;
    }
    Bool::YES
}

extern "C" fn opaque_rect_for_window_move_when_in_titlebar(this: &AnyObject, _: Sel) -> NSRect {
    // SAFETY: AppKit calls this on our views.
    let owned = unsafe { get_state(this) }.is_some_and(|state| {
        state
            .try_borrow()
            .is_ok_and(|state| state.app_owns_titlebar_drag)
    });
    if owned {
        // SAFETY: a plain view property.
        unsafe { msg_send![this, bounds] }
    } else {
        NSRect::ZERO
    }
}

extern "C" fn character_index_for_point(this: &AnyObject, _: Sel, position: NSPoint) -> NSUInteger {
    let frame = get_frame(this);
    let position = point(
        px((position.x - frame.origin.x) as f32),
        px((frame.size.height - (position.y - frame.origin.y)) as f32),
    );
    callbacks(this)
        .and_then(|callbacks| callbacks.character_index_for_point(position))
        .map_or(NSNotFound as NSUInteger, |index| index as NSUInteger)
}

fn drag_position(this: &AnyObject, info: *mut AnyObject) -> Point<Pixels> {
    // SAFETY: AppKit passes an NSDraggingInfo.
    let location: NSPoint = unsafe { msg_send![info, draggingLocation] };
    // SAFETY: AppKit calls this on our windows.
    let height = unsafe { get_state(this) }
        .and_then(|state| {
            state
                .try_borrow()
                .ok()
                .map(|state| state.content_size().height)
        })
        .unwrap_or_default();
    point(px(location.x as f32), height - px(location.y as f32))
}

fn send_drop(this: &AnyObject, event: FileDropEvent) -> bool {
    // SAFETY: AppKit calls this on our windows.
    let Some(state) = (unsafe { get_state(this) }) else {
        return false;
    };
    let dragged = match &event {
        FileDropEvent::Entered { .. } => Some(true),
        FileDropEvent::Exited => Some(false),
        _ => None,
    };
    let Some(callbacks) = state.try_borrow().ok().map(|state| state.callbacks) else {
        return false;
    };
    callbacks.input_later(PlatformInput::FileDrop(event));
    if let Some(dragged) = dragged
        && let Ok(mut lock) = state.try_borrow_mut()
    {
        lock.external_files_dragged = dragged;
    }
    true
}

fn dragged_paths(info: *mut AnyObject) -> Option<Vec<PathBuf>> {
    // SAFETY: AppKit passes an NSDraggingInfo, whose pasteboard holds a list of paths.
    unsafe {
        let pasteboard: *mut AnyObject = msg_send![info, draggingPasteboard];
        if pasteboard.is_null() {
            return None;
        }
        #[expect(
            deprecated,
            reason = "file drops are decoded from a property list of paths"
        )]
        let names: *mut NSArray<NSString> =
            msg_send![pasteboard, propertyListForType: NSFilenamesPboardType];
        let names = names.as_ref()?;
        Some(
            names
                .iter()
                .map(|name| PathBuf::from(name.to_string()))
                .collect(),
        )
    }
}

extern "C" fn dragging_entered(this: &AnyObject, _: Sel, info: *mut AnyObject) -> NSDragOperation {
    let position = drag_position(this, info);
    match dragged_paths(info) {
        Some(paths) => {
            if send_drop(this, FileDropEvent::Entered { position, paths }) {
                NSDragOperation::Copy
            } else {
                NSDragOperation::None
            }
        }
        None => NSDragOperation::None,
    }
}

extern "C" fn dragging_updated(this: &AnyObject, _: Sel, info: *mut AnyObject) -> NSDragOperation {
    let position = drag_position(this, info);
    if send_drop(this, FileDropEvent::Pending { position }) {
        NSDragOperation::Copy
    } else {
        NSDragOperation::None
    }
}

extern "C" fn dragging_exited(this: &AnyObject, _: Sel, _: *mut AnyObject) {
    send_drop(this, FileDropEvent::Exited);
}

extern "C" fn perform_drag_operation(this: &AnyObject, _: Sel, info: *mut AnyObject) -> Bool {
    let position = drag_position(this, info);
    Bool::new(send_drop(this, FileDropEvent::Submit { position }))
}

fn display_id_for_screen(screen: *mut AnyObject) -> Option<CGDirectDisplayID> {
    if screen.is_null() {
        return None;
    }
    // SAFETY: AppKit screens describe their display number.
    unsafe {
        let description: *mut AnyObject = msg_send![screen, deviceDescription];
        if description.is_null() {
            return None;
        }
        let number: *mut AnyObject =
            msg_send![description, objectForKey: ns_string("NSScreenNumber")];
        if number.is_null() {
            return None;
        }
        let number: NSUInteger = msg_send![number, unsignedIntegerValue];
        Some(number as CGDirectDisplayID)
    }
}
