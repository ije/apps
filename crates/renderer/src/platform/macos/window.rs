use super::{
    NSRange, NSStringExt, TISCopyCurrentKeyboardInputSource, TISGetInputSourceProperty,
    display_link::WindowFrameSource,
    events::platform_input_from_native,
    kTISPropertyInputSourceIsASCIICapable, kTISPropertyInputSourceType, kTISTypeKeyboardInputMode,
    metal::{MetalAtlas, Renderer},
    ns_string, post_local,
};
use crate::{
    Bounds, CursorStyle, FileDropEvent, KeyDownEvent, Keystroke, Modifiers, ModifiersChangedEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, NavigationDirection, Pixels,
    PlatformInput, Point, Scene, Size, WindowChange, WindowOptions, WindowState, app::Callbacks,
    point, px, size,
};
use anyhow::Result;
use core_foundation::base::{CFRelease, CFTypeRef};
use core_foundation_sys::{
    base::CFEqual,
    number::{CFBooleanGetValue, CFBooleanRef},
};
use core_graphics::display::CGDirectDisplayID;
use objc2::{
    AnyThread, MainThreadMarker, class, msg_send,
    rc::{Retained, autoreleasepool},
    runtime::{AnyClass, AnyObject, AnyProtocol, Bool, ClassBuilder, Sel},
    sel,
};
#[expect(
    deprecated,
    reason = "file drops are decoded from a property list of paths"
)]
use objc2_app_kit::NSFilenamesPboardType;
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBackingStoreType, NSButton, NSColor, NSDragOperation, NSScreen,
    NSTrackingArea, NSTrackingAreaOptions, NSView, NSViewLayerContentsRedrawPolicy, NSWindow,
    NSWindowButton, NSWindowCollectionBehavior, NSWindowOcclusionState, NSWindowOrderingMode,
    NSWindowSharingType, NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_foundation::{
    NSArray, NSInteger, NSNotFound, NSOperatingSystemVersion, NSPoint, NSProcessInfo, NSRect,
    NSSize, NSString, NSUInteger,
};
use raw_window_handle as rwh;
use std::{
    cell::RefCell,
    ffi::{CStr, c_void},
    ops::Range,
    path::PathBuf,
    ptr::{self, NonNull},
    rc::{Rc, Weak},
    sync::OnceLock,
    time::Duration,
};

const STATE_IVAR: &CStr = c"windowState";

#[allow(non_upper_case_globals)]
const NSNormalWindowLevel: NSInteger = 0;
#[allow(non_upper_case_globals)]
const NSFloatingWindowLevel: NSInteger = 3;

struct Classes {
    window: usize,
    view: usize,
}

fn classes() -> (&'static AnyClass, &'static AnyClass) {
    static CLASSES: OnceLock<Classes> = OnceLock::new();
    let classes = CLASSES.get_or_init(|| {
        // SAFETY: the methods have the signatures AppKit calls them with.
        unsafe {
            Classes {
                window: build_window_class() as usize,
                view: build_view_class() as usize,
            }
        }
    });
    // SAFETY: registered classes live for the whole process.
    unsafe {
        (
            &*(classes.window as *const AnyClass),
            &*(classes.view as *const AnyClass),
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
        // Keeps AppKit from beeping on keys with modifiers.
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

struct State {
    callbacks: Callbacks,
    native_window: *mut AnyObject,
    native_view: NonNull<AnyObject>,
    renderer: Renderer,
    frame_source: Option<WindowFrameSource>,
    /// The app wants display link frames.
    wants_frames: bool,
    cursor: CursorStyle,
    last_key_equivalent: Option<KeyDownEvent>,
    last_modifiers: Option<ModifiersChangedEvent>,
    keystroke_for_do_command: Option<Keystroke>,
    do_command_handled: Option<bool>,
    external_files_dragged: bool,
    synthetic_drag_counter: usize,
    /// The next left click also focuses the window.
    first_mouse: bool,
    /// The whole view is app-owned title bar content, so AppKit neither drags the window from it
    /// nor delays title bar clicks. The app moves the window with `start_window_move`.
    app_owns_titlebar_drag: bool,
    traffic_light_position: Option<Point<Pixels>>,
    traffic_light_frames: Option<TrafficLightFrames>,
    pre_fullscreen_traffic_light_frames: Option<TrafficLightFrames>,
    transparent_titlebar: bool,
    is_exiting_fullscreen: bool,
    closed: bool,
    focusable: bool,
    /// The content ratio and the extra size it leaves out, kept while the user resizes.
    aspect: Option<(f64, NSSize)>,
    /// The parent window, joined while this one is shown.
    parent: Option<objc2::rc::Weak<NSWindow>>,
    /// Shown as a sheet of the parent.
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
        // AppKit can report 0 for a window without a screen.
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

    fn request_frames(&mut self) {
        if !self.wants_frames
            || !self
                .frame_source
                .as_ref()
                .is_some_and(WindowFrameSource::is_running)
        {
            self.wants_frames = true;
            self.start_display_link();
        }
    }

    fn start_display_link(&mut self) {
        if !self.wants_frames || !self.is_visible() {
            return;
        }
        // SAFETY: plain property reads on a live window.
        let screen: *mut AnyObject = unsafe { msg_send![self.native_window, screen] };
        // AppKit can report no screen while displays are reconfigured.
        let Some(display) = display_id_for_screen(screen) else {
            return;
        };
        let view = self.native_view.as_ptr() as *mut c_void;
        let source = self
            .frame_source
            .get_or_insert_with(|| WindowFrameSource::new(view, step));
        if let Err(error) = source.start(display) {
            eprintln!("apps: display link: {error:#}");
        }
    }

    fn stop_display_link(&mut self) {
        if let Some(source) = &mut self.frame_source {
            source.stop();
        }
        self.renderer.trim();
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
        // AppKit can recreate the standard buttons, so each layout reads the live views.
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

/// A native window and its Metal layer.
pub(crate) struct Window {
    state: Shared,
    atlas: Rc<MetalAtlas>,
    marker: MainThreadMarker,
}

impl Window {
    pub fn open(
        options: &WindowOptions,
        callbacks: Callbacks,
        marker: MainThreadMarker,
    ) -> Result<Self> {
        let (window_class, view_class) = classes();
        autoreleasepool(|_| unsafe {
            let mut renderer = Renderer::new(options.transparent)?;
            let atlas = renderer.atlas().clone();
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
                renderer.update_transparency(true);
                let () = msg_send![native_window, setOpaque: Bool::NO];
                // Not `clearColor`, which breaks the window shadow.
                let color = NSColor::colorWithSRGBRed_green_blue_alpha(0., 0., 0., 0.0001);
                let () = msg_send![native_window, setBackgroundColor: &*color];
            }
            let state = Rc::new(RefCell::new(State {
                callbacks,
                native_window,
                native_view: NonNull::new_unchecked(native_view),
                renderer,
                frame_source: None,
                wants_frames: false,
                cursor: CursorStyle::Arrow,
                last_key_equivalent: None,
                last_modifiers: None,
                keystroke_for_do_command: None,
                do_command_handled: None,
                external_files_dragged: false,
                synthetic_drag_counter: 0,
                first_mouse: false,
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
            let window = Self {
                state,
                atlas,
                marker,
            };
            window.set_title(&options.title);
            let () = msg_send![native_window, setMovable: Bool::new(options.movable)];
            if options.titlebar_hidden || options.frameless {
                let () = msg_send![native_window, setTitlebarAppearsTransparent: Bool::YES];
                let () =
                    msg_send![native_window, setTitleVisibility: NSWindowTitleVisibility::Hidden];
            }
            let () = msg_send![native_view, setAutoresizingMask: NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable];
            // Views become layer backed on their own after joining a window, which breaks the
            // Metal layer, so this one is layer backed from the start.
            let () = msg_send![native_view, setWantsLayer: Bool::YES];
            let () = msg_send![native_view, setLayerContentsRedrawPolicy: NSViewLayerContentsRedrawPolicy::DuringViewResize];
            let native_view: *mut AnyObject = msg_send![native_view, autorelease];
            let () = msg_send![content_view, addSubview: native_view];
            let _: Bool = msg_send![native_window, makeFirstResponder: native_view];
            let () = msg_send![native_window, setLevel: NSNormalWindowLevel];
            let () = msg_send![native_window, setAcceptsMouseMovedEvents: Bool::NO];
            add_mouse_tracking_area(&*native_view.cast::<NSView>());
            if options.show {
                let () =
                    msg_send![native_window, makeKeyAndOrderFront: ptr::null_mut::<AnyObject>()];
            }
            // The origin from `initWithContentRect` can be off when the key screen is not the primary one.
            let () = msg_send![native_window, setFrameTopLeftPoint: rect.origin];
            if options.frameless {
                window.set_button_visibility(false);
            }
            window.state.borrow_mut().move_traffic_light();
            Ok(window)
        })
    }

    fn native_window(&self) -> *mut AnyObject {
        self.state.borrow().native_window
    }

    pub fn atlas(&self) -> &Rc<MetalAtlas> {
        &self.atlas
    }

    pub fn scale_factor(&self) -> f32 {
        self.state.borrow().scale_factor()
    }

    pub fn content_size(&self) -> Size<Pixels> {
        self.state.borrow().content_size()
    }

    /// Starts display link frames until a frame wants no more.
    pub fn request_frame(&self) {
        match self.state.try_borrow_mut() {
            Ok(mut state) => state.request_frames(),
            // WARNING: an AppKit callback up the stack holds the state, and a dropped request would
            // leave the window dirty without frames, so it runs after that callback.
            Err(_) => self.later(|_, state| {
                if let Ok(mut state) = state.try_borrow_mut() {
                    state.request_frames();
                }
            }),
        }
    }

    /// Draws once even when the window is hidden or covered, for captures.
    pub fn force_frame(&self) {
        let view = self.state.borrow().native_view.as_ptr();
        // SAFETY: the view is retained until the posted frame ran.
        unsafe {
            let _: *mut AnyObject = msg_send![view, retain];
        }
        post_local(move || {
            step(view as *mut c_void);
            // SAFETY: balances the retain above.
            unsafe {
                let _: () = msg_send![view, release];
            }
        });
    }

    pub fn draw(&self, scene: &Scene, capture: bool) -> Option<Result<([u32; 2], Vec<u8>)>> {
        self.state.borrow_mut().renderer.draw(scene, capture)
    }

    /// See [`super::metal::Renderer::promote`].
    pub fn promote(
        &self,
        key: u64,
        index: usize,
        region: [i32; 4],
        step: f64,
        opacities: &[f32],
        transforms: &[[f64; 16]],
    ) -> bool {
        self.state
            .borrow_mut()
            .renderer
            .promote(key, index, region, step, opacities, transforms)
    }

    pub fn demote(&self) {
        self.state.borrow_mut().renderer.demote();
    }

    pub fn is_active(&self) -> bool {
        self.state.borrow().native_window().isKeyWindow()
    }

    pub fn set_title(&self, title: &str) {
        let window = self.native_window();
        // SAFETY: plain AppKit calls on a live window.
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let title = ns_string(title);
            let () = msg_send![app, changeWindowsItem: window, title: title, filename: false];
            let () = msg_send![window, setTitle: title];
        }
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.move_traffic_light();
        }
    }

    pub fn set_button_visibility(&self, visible: bool) {
        if let Some(buttons) = self.state.borrow().traffic_light_buttons() {
            for button in [buttons.close, buttons.minimize, buttons.zoom] {
                button.setHidden(!visible);
            }
        }
    }

    pub fn state(&self) -> WindowState {
        let state = self.state.borrow();
        let window = state.native_window();
        let top = primary_screen_top(self.marker);
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
        let fullscreen = state.is_fullscreen();
        let zoomed = if window.styleMask().contains(NSWindowStyleMask::Resizable) {
            window.isZoomed()
        } else {
            let visible = window
                .screen()
                .map_or_else(Default::default, |screen| screen.visibleFrame());
            frame.size.width == visible.size.width && frame.size.height == visible.size.height
        };
        WindowState {
            frame: flip(frame),
            content: flip(window.contentRectForFrameRect(frame)),
            minimized,
            maximized: zoomed && !minimized && !fullscreen,
            fullscreen,
            visible: window.isVisible(),
            resizing: window.inLiveResize(),
        }
    }

    /// Runs `f` with the window after the current event, unless it closed by then.
    fn later(&self, f: impl FnOnce(&NSWindow, &Shared) + 'static) {
        let state = Rc::downgrade(&self.state);
        post_local(move || {
            let Some(state) = state.upgrade() else {
                return;
            };
            let window = {
                let state = state.borrow();
                if state.closed {
                    return;
                }
                state.native_window
            };
            // SAFETY: the window lives while the state does.
            f(unsafe { &*window.cast::<NSWindow>() }, &state);
        });
    }

    pub fn change(&self, change: WindowChange) {
        // The layer turns transparent now, so the next frame clears to transparent.
        if let WindowChange::Vibrancy(material, _) = change
            && let Ok(mut state) = self.state.try_borrow_mut()
        {
            let transparent = material.is_some() || !state.native_window().isOpaque();
            state.renderer.update_transparency(transparent);
        }
        let marker = self.marker;
        let view = self.state.borrow().native_view;
        self.later(move |window, state| {
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
                                        // NSVisualEffectBlendingModeBehindWindow
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

    /// Resizes the content area.
    pub fn resize(&self, size: Size<Pixels>) {
        self.later(move |window, _| {
            window.setContentSize(NSSize::new(size.width.0 as f64, size.height.0 as f64))
        });
    }

    pub fn start_window_move(&self) {
        let window = self.native_window();
        // SAFETY: the current event drives the drag loop.
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let event: *mut AnyObject = msg_send![app, currentEvent];
            let () = msg_send![window, performWindowDragWithEvent: event];
        }
    }

    /// Does what the user set for a double click on a title bar.
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
        Ok(rwh::RawWindowHandle::AppKit(rwh::AppKitWindowHandle::new(
            self.state.borrow().native_view.cast(),
        )))
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        let window = {
            let mut state = self.state.borrow_mut();
            state.frame_source.take();
            state.closed = true;
            state.native_window
        };
        // SAFETY: the window stays retained until the posted close.
        unsafe {
            let () = msg_send![window, setDelegate: ptr::null_mut::<AnyObject>()];
            // WARNING: a sheet that is not ended blocks its parent, a sheet shown next and quitting,
            // which can already be posted, so it ends now.
            detach(&*window.cast::<NSWindow>());
        }
        post_local(move || unsafe {
            let () = msg_send![window, close];
            let _: *mut AnyObject = msg_send![window, autorelease];
        });
    }
}

/// Joins the parent window, as its sheet or as a child above it. Returns whether it is a sheet,
/// which AppKit shows itself.
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

/// Tells the view about a window change after the current event.
fn report_change(state: &Shared) {
    // WARNING: the callback can run app code that closes the window, so it runs without a borrow.
    let callbacks = state.try_borrow().map(|state| state.callbacks);
    if let Ok(callbacks) = callbacks {
        callbacks.changed();
    }
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

/// The window state of one of our windows or views.
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

/// The callbacks of a window, read without holding its state.
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
        // Inactive windows still see the mouse, for hover.
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
            // Undocumented private class methods.
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
    handle_key_event(this, event, true)
}

extern "C" fn handle_key_down(this: &AnyObject, _: Sel, event: *mut AnyObject) {
    handle_key_event(this, event, false);
}

/// Whether the input source composes text, like Japanese or Chinese input methods, as opposed
/// to keyboard layouts and ASCII modes like Japanese Romaji.
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

// Things to test when changing this:
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
    match input {
        PlatformInput::KeyDown(down) => {
            // AppKit sends some keys as a key equivalent, then again as a key down when that was
            // not handled. The second one is skipped.
            let same = |a: &KeyDownEvent, b: &KeyDownEvent| {
                a.keystroke == b.keystroke && a.is_held == b.is_held
            };
            if key_equivalent {
                lock.last_key_equivalent = Some(down.clone());
            } else if lock
                .last_key_equivalent
                .take()
                .is_some_and(|last| same(&last, &down))
            {
                return Bool::NO;
            }
            drop(lock);
            let keystroke = &down.keystroke;
            let modifiers = keystroke.modifiers;
            let plain = !modifiers.control && !modifiers.function && !modifiers.platform;
            let composing = callbacks.marked_text_range().is_some();
            // Composed keys, keys without text (arrows, escape) and printable keys of a composing
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
            if composing || ime_printable || (keystroke.key_char.is_none() && plain) {
                {
                    let mut lock = state.borrow_mut();
                    lock.keystroke_for_do_command = Some(keystroke.clone());
                    lock.do_command_handled = None;
                }
                let handled = input_context_handles(this, event);
                let mut lock = state.borrow_mut();
                lock.keystroke_for_do_command = None;
                if let Some(handled) = lock.do_command_handled.take() {
                    return Bool::new(handled);
                } else if handled {
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
            // Key equivalents with modifiers other than fn stay with AppKit, or cmd-` stops working.
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
        PlatformInput::KeyUp(_) => {
            drop(lock);
            Bool::new(callbacks.input(input))
        }
        _ => Bool::NO,
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
        // A ctrl-left click is a right click on macOS.
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
        // The matching left up of a ctrl-left click is a right up.
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
    match &input {
        PlatformInput::MouseDown(_) => {
            drop(lock);
            input_context_handles(this, event);
        }
        PlatformInput::MouseMove(
            moved @ MouseMoveEvent {
                pressed_button: Some(_),
                ..
            },
        ) => {
            // Drags repeat while the button is held, so a selection keeps scrolling when the
            // mouse rests outside the window. File drags make their own events.
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
            // Only real changes are sent.
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
    callbacks.input(input);
}

fn synthetic_drag(state: Weak<RefCell<State>>, id: usize, event: MouseMoveEvent) {
    super::run_after(Duration::from_millis(16), move || {
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
            // A menu can take the mouse up, so the button must still be down.
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
        callbacks.input(PlatformInput::MouseMove(event.clone()));
        synthetic_drag(Rc::downgrade(&state), id, event);
    });
}

extern "C" fn window_did_change_occlusion_state(this: &AnyObject, _: Sel, _: *mut AnyObject) {
    // SAFETY: AppKit calls this on our windows.
    let Some(state) = (unsafe { get_state(this) }) else {
        return;
    };
    if let Ok(mut lock) = state.try_borrow_mut() {
        if lock.is_visible() {
            lock.move_traffic_light();
            lock.start_display_link();
        } else {
            lock.stop_display_link();
        }
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

/// Keeps the aspect ratio with its extra size, after AppKit applied the content aspect ratio.
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
        // Buttons moved during the transition hold their fullscreen frames.
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

/// Matches the drawable to the view's size and scale, then tells the app.
fn update_drawable(state: &Shared) {
    let Ok(mut lock) = state.try_borrow_mut() else {
        return;
    };
    let scale = lock.scale_factor();
    let content = lock.content_size();
    let layer = lock.renderer.layer_ptr().cast::<AnyObject>();
    // SAFETY: the layer belongs to the renderer.
    unsafe {
        let () = msg_send![layer, setContentsScale: scale as f64];
    }
    lock.renderer
        .update_drawable_size(content.to_device_pixels(scale));
    let callbacks = lock.callbacks;
    drop(lock);
    callbacks.resized(content, scale);
}

extern "C" fn window_did_change_screen(this: &AnyObject, _: Sel, _: *mut AnyObject) {
    // SAFETY: AppKit calls this on our windows.
    let Some(state) = (unsafe { get_state(this) }) else {
        return;
    };
    if let Ok(mut lock) = state.try_borrow_mut() {
        // The display link follows the new screen.
        lock.stop_display_link();
        lock.start_display_link();
    }
    update_drawable(&state);
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
    // AppKit can send a spurious `windowDidBecomeKey` to the previous key window when a popup
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
            lock.frame_source.take();
        }
        let () = msg_send![super(this, class!(NSWindow)), close];
    }
}

extern "C" fn make_backing_layer(this: &AnyObject, _: Sel) -> *mut AnyObject {
    // SAFETY: AppKit calls this on our views.
    let Some(state) = (unsafe { get_state(this) }) else {
        return ptr::null_mut();
    };

    state.borrow().renderer.layer_ptr() as *mut AnyObject
}

extern "C" fn view_did_change_backing_properties(this: &AnyObject, _: Sel) {
    // SAFETY: AppKit calls this on our views.
    if let Some(state) = unsafe { get_state(this) } {
        update_drawable(&state);
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
    update_drawable(&state);
}

/// AppKit asks for the layer's contents, during a live resize for one. The frame is drawn now and
/// presented in the same Core Animation transaction.
extern "C" fn display_layer(this: &AnyObject, _: Sel, _: *mut AnyObject) {
    // SAFETY: AppKit calls this on our views.
    let Some(state) = (unsafe { get_state(this) }) else {
        return;
    };
    let callbacks = {
        let Ok(mut lock) = state.try_borrow_mut() else {
            return;
        };
        lock.renderer.set_presents_with_transaction(true);
        lock.callbacks
    };
    let more = callbacks.frame();
    if let Ok(mut lock) = state.try_borrow_mut() {
        lock.renderer.set_presents_with_transaction(false);
        if more == Some(false) {
            lock.wants_frames = false;
            lock.stop_display_link();
        }
    }
}

/// A display link tick.
extern "C" fn step(view: *mut c_void) {
    // SAFETY: the context is our view, retained while the frame source lives.
    let Some(state) = (unsafe { get_state(&*(view as *mut AnyObject)) }) else {
        return;
    };
    let Some(callbacks) = state.try_borrow().ok().map(|state| state.callbacks) else {
        return;
    };
    // An idle window stops its display link, so it gets no callbacks until it changes.
    if callbacks.frame() == Some(false)
        && let Ok(mut lock) = state.try_borrow_mut()
    {
        lock.wants_frames = false;
        lock.stop_display_link();
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

/// The window frame, with the content area at the top for windows with a title bar.
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

/// The text of an NSString or NSAttributedString.
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
    let mut adjusted: Option<Range<usize>> = None;
    let Some(text) = callbacks(this)
        .and_then(|callbacks| callbacks.text_for_range(range.clone(), &mut adjusted))
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

// The selector is ignored, since the app may bind the key to something else.
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
            prefer_character_input: false,
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
    // AppKit can lay the traffic lights out again for the new appearance.
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

/// The part of the view that is app-owned title bar content. Windows that own their title bar
/// claim the whole view, so AppKit neither drags the window from it nor delays the clicks to
/// tell double clicks apart.
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
    callbacks.input(PlatformInput::FileDrop(event));
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
