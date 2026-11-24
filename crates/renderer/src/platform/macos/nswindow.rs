use super::{
    controls,
    events::{
        BUTTONS, NOT_FOUND, TISCopyCurrentKeyboardInputSource, TISGetInputSourceProperty,
        kTISPropertyInputSourceIsASCIICapable, kTISPropertyInputSourceType,
        kTISTypeKeyboardInputMode, merge, ns_range, plain, span, translate,
    },
    glass, lock, native_after,
    screen::{Shared, Snapshot, cg_size, flip, primary_top},
    threads::ask_within,
    vsync::Pacer,
};
use crate::{
    Button, Cursor, Event, Key, Modifiers, Pixels, Point, Size, WindowChange, WindowOptions,
    WindowState,
    platform::{Callbacks, Motion, events},
    point, px, size,
};
use anyhow::{Context as _, Result};
use core_foundation_sys::{
    base::{CFEqual, CFRelease},
    number::{CFBooleanGetValue, CFBooleanRef},
};
use objc2::{
    AllocAnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message,
    define_class, msg_send,
    rc::{Retained, Weak},
    runtime::{AnyObject, MessageReceiver, NSObjectProtocol, ProtocolObject, Sel},
};
use objc2_app_kit::{
    NSApp, NSAutoresizingMaskOptions, NSBackingStoreType, NSButton, NSColor, NSCursor,
    NSDisplayGamut, NSDragOperation, NSDraggingInfo, NSEvent, NSResponder, NSScreen,
    NSTextInputClient, NSTrackingArea, NSTrackingAreaOptions, NSView, NSViewLayerContentsPlacement,
    NSViewLayerContentsRedrawPolicy, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindow, NSWindowButton, NSWindowCollectionBehavior,
    NSWindowDelegate, NSWindowOcclusionState, NSWindowOrderingMode, NSWindowSharingType,
    NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_core_foundation::{CGPoint, CGSize};
use objc2_foundation::{
    NSArray, NSAttributedString, NSOperatingSystemVersion, NSProcessInfo, NSRange, NSRect,
    NSString, NSUInteger, NSUserDefaults,
};
use objc2_quartz_core::{CALayer, CALayerDelegate, CAMetalLayer};
use raw_window_handle as rwh;
use std::{
    cell::{Cell, OnceCell, RefCell},
    ffi::c_void,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

// Title bar container, then the close, minimize and zoom buttons.
type ButtonFrames = [NSRect; 4];

pub(crate) struct Chrome {
    callbacks: Callbacks,
    view: OnceCell<Retained<AppsView>>,
    snapshot: Shared<Snapshot>,
    pacer: Shared<Pacer>,
    pub closed: Cell<bool>,
    focusable: Cell<bool>,
    // Width over height, and the extra size around the content that the ratio leaves out.
    aspect: Cell<Option<(f64, CGSize)>>,
    parent: RefCell<Option<Weak<NSWindow>>>,
    sheet: Cell<bool>,
    // The NSVisualEffectState of vibrancy: 1 always active, 2 never, else as the window.
    effect: Cell<isize>,
    pub buttons_at: Cell<Option<Point<Pixels>>>,
    buttons_home: Cell<Option<ButtonFrames>>,
    windowed_home: Cell<Option<ButtonFrames>>,
    leaving_fullscreen: Cell<bool>,
    clear_titlebar: bool,
}

define_class!(
    // SAFETY: NSWindow has no subclassing rules, and the class has no Drop.
    #[unsafe(super(NSWindow))]
    #[thread_kind = MainThreadOnly]
    #[name = "AppsWindow"]
    #[ivars = Chrome]
    pub(crate) struct AppsWindow;

    impl AppsWindow {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool {
            self.ivars().focusable.get()
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool {
            self.ivars().focusable.get()
        }

        // Liquid Glass reads this private getter to draw a window as active or not.
        #[unsafe(method(_hasActiveAppearance))]
        fn has_active_appearance(&self) -> bool {
            match self.ivars().effect.get() {
                1 => true,
                2 => false,
                // SAFETY: AppKit calls this only on systems whose NSWindow has it.
                _ => unsafe { msg_send![super(self), _hasActiveAppearance] },
            }
        }

        #[unsafe(method(close))]
        fn close_now(&self) {
            self.ivars().closed.set(true);
            lock(&self.ivars().pacer).close();
            // SAFETY: the NSWindow implementation.
            unsafe { msg_send![super(self), close] }
        }

        #[unsafe(method(draggingEntered:))]
        fn drag_enter(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            let board = info.draggingPasteboard();
            #[expect(deprecated, reason = "file drops come as a property list of paths")]
            // SAFETY: the type is an AppKit constant, and its property list is an array of strings.
            let names: Option<Retained<NSArray<NSString>>> =
                unsafe { msg_send![&board, propertyListForType: objc2_app_kit::NSFilenamesPboardType] };
            let Some(names) = names else {
                return NSDragOperation::None;
            };
            let paths = names.iter().map(|name| PathBuf::from(name.to_string())).collect();
            self.files(Event::FileEnter { at: self.drop_point(info), paths })
        }

        #[unsafe(method(draggingUpdated:))]
        fn drag_over(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            self.files(Event::FileOver { at: self.drop_point(info) })
        }

        #[unsafe(method(draggingExited:))]
        fn drag_exit(&self, _: Option<&AnyObject>) {
            self.files(Event::FileLeave);
        }

        #[unsafe(method(concludeDragOperation:))]
        fn drag_end(&self, _: Option<&AnyObject>) {
            self.files(Event::FileLeave);
        }

        #[unsafe(method(performDragOperation:))]
        fn drag_drop(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            self.files(Event::FileDrop { at: self.drop_point(info) });
            true
        }
    }

    unsafe impl NSObjectProtocol for AppsWindow {}

    unsafe impl NSWindowDelegate for AppsWindow {
        #[unsafe(method(windowShouldClose:))]
        fn should_close(&self, _: &AnyObject) -> bool {
            events().should_close(self.ivars().callbacks)
        }

        #[unsafe(method(windowDidResize:))]
        fn resized(&self, _: &AnyObject) {
            self.place_buttons();
        }

        #[unsafe(method(windowWillResize:toSize:))]
        fn will_resize(&self, _: &AnyObject, proposed: CGSize) -> CGSize {
            let frame = self.frame();
            let size = match self.ivars().aspect.get() {
                Some((ratio, extra)) => {
                    let inner = self.contentRectForFrameRect(frame).size;
                    let pad_x = frame.size.width - inner.width + extra.width;
                    let pad_y = frame.size.height - inner.height + extra.height;
                    let width = (pad_x + (proposed.height - pad_y) * ratio).round();
                    CGSize::new(width, (pad_y + (width - pad_x) / ratio).round())
                }
                None => proposed,
            };
            let edge = match (size.width - frame.size.width).abs() > (size.height - frame.size.height).abs() {
                true => "right",
                false => "bottom",
            };
            let bounds = flip(NSRect::new(frame.origin, size), primary_top(self.mtm()));
            match events().will_change(self.ivars().callbacks, edge, bounds) {
                true => frame.size,
                false => size,
            }
        }

        #[unsafe(method(windowWillMove:))]
        fn will_move(&self, _: &AnyObject) {
            let bounds = flip(self.frame(), primary_top(self.mtm()));
            events().will_change(self.ivars().callbacks, "move", bounds);
        }

        #[unsafe(method(windowDidChangeOcclusionState:))]
        fn occlusion_changed(&self, _: &AnyObject) {
            self.locate();
            if self.on_screen() {
                self.place_buttons();
            } else {
                events().release(self.ivars().callbacks);
            }
            self.report();
        }

        #[unsafe(method(windowWillEnterFullScreen:))]
        fn entering_fullscreen(&self, _: &AnyObject) {
            let ivars = self.ivars();
            ivars.leaving_fullscreen.set(false);
            ivars.windowed_home.set(ivars.buttons_home.get());
            self.reset_buttons();
            if os_at_least(15, 3) {
                self.setTitlebarAppearsTransparent(false);
            }
        }

        #[unsafe(method(windowWillExitFullScreen:))]
        fn leaving_fullscreen(&self, _: &AnyObject) {
            self.ivars().leaving_fullscreen.set(true);
            if os_at_least(15, 3) && self.ivars().clear_titlebar {
                self.setTitlebarAppearsTransparent(true);
            }
            self.place_buttons();
        }

        #[unsafe(method(windowDidFailToExitFullScreen:))]
        fn stayed_fullscreen(&self, _: &AnyObject) {
            self.ivars().leaving_fullscreen.set(false);
            self.reset_buttons();
        }

        #[unsafe(method(windowDidExitFullScreen:))]
        fn left_fullscreen(&self, _: &AnyObject) {
            let ivars = self.ivars();
            ivars.leaving_fullscreen.set(false);
            // Buttons that moved during the animation would keep their fullscreen frames as home.
            ivars.buttons_home.set(ivars.windowed_home.take());
            self.place_buttons();
            self.report();
        }

        #[unsafe(method(windowDidEnterFullScreen:))]
        fn entered_fullscreen(&self, _: &AnyObject) {
            self.report();
        }

        #[unsafe(method(windowDidMove:))]
        fn moved(&self, _: &AnyObject) {
            self.report();
        }

        #[unsafe(method(windowDidMiniaturize:))]
        fn minimized(&self, _: &AnyObject) {
            self.report();
        }

        #[unsafe(method(windowDidDeminiaturize:))]
        fn restored(&self, _: &AnyObject) {
            self.report();
        }

        #[unsafe(method(windowDidEndLiveResize:))]
        fn resize_ended(&self, _: &AnyObject) {
            self.report();
        }

        #[unsafe(method(windowDidChangeScreen:))]
        fn screen_changed(&self, _: &AnyObject) {
            self.locate();
            self.update_size();
            self.report();
        }

        #[unsafe(method(windowDidBecomeKey:))]
        fn became_key(&self, _: &AnyObject) {
            // When a popup opens in an inactive app, AppKit can make the previous key window key again for a
            // moment. Undo that and report nothing.
            if !self.isKeyWindow() {
                self.resignKeyWindow();
                return;
            }
            self.place_buttons();
            self.report();
        }

        #[unsafe(method(windowDidResignKey:))]
        fn resigned_key(&self, _: &AnyObject) {
            if self.isKeyWindow() {
                self.place_buttons();
            }
            self.report();
        }
    }
);

fn os_at_least(major: isize, minor: isize) -> bool {
    let version = NSOperatingSystemVersion {
        majorVersion: major,
        minorVersion: minor,
        patchVersion: 0,
    };
    NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(version)
}

pub(crate) fn open(
    options: &WindowOptions,
    callbacks: Callbacks,
    layer: Retained<CAMetalLayer>,
    overlay: Retained<CALayer>,
    pacer: Shared<Pacer>,
    snapshot: Shared<Snapshot>,
) -> Result<(Retained<AppsWindow>, Retained<AppsView>)> {
    let mtm = MainThreadMarker::new().unwrap();
    let bare = options.titlebar_hidden || options.frameless;
    NSWindow::setAllowsAutomaticWindowTabbing(false, mtm);
    let mut style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable;
    style.set(NSWindowStyleMask::Resizable, options.resizable);
    style.set(NSWindowStyleMask::Miniaturizable, options.minimizable);
    style.set(NSWindowStyleMask::FullSizeContentView, bare);
    let screen = NSScreen::screens(mtm)
        .firstObject()
        .or_else(|| NSScreen::mainScreen(mtm));
    let area = screen
        .as_ref()
        .map(|screen| screen.frame())
        .unwrap_or_default();
    let bounds = options.bounds;
    let mut inner = CGSize::new(bounds.size.width.0 as f64, bounds.size.height.0 as f64);
    if options.outer_size {
        inner = NSWindow::contentRectForFrameRect_styleMask(
            NSRect::new(CGPoint::ZERO, inner),
            style,
            mtm,
        )
        .size;
    }
    // The top left corner, in AppKit coordinates.
    let corner = CGPoint::new(
        area.origin.x + bounds.origin.x.0 as f64,
        area.origin.y + area.size.height - bounds.origin.y.0 as f64,
    );
    let chrome = Chrome {
        callbacks,
        view: OnceCell::new(),
        snapshot,
        pacer,
        closed: Cell::new(false),
        focusable: Cell::new(true),
        aspect: Cell::new(None),
        parent: RefCell::new(None),
        sheet: Cell::new(false),
        effect: Cell::new(0),
        buttons_at: Cell::new(options.buttons_at.filter(|_| options.titlebar_hidden)),
        buttons_home: Cell::new(None),
        windowed_home: Cell::new(None),
        leaving_fullscreen: Cell::new(false),
        clear_titlebar: bare,
    };
    let window = AppsWindow::alloc(mtm).set_ivars(chrome);
    // SAFETY: the designated initializer of NSWindow.
    let window: Option<Retained<AppsWindow>> = unsafe {
        msg_send![super(window), initWithContentRect: NSRect::new(corner, inner), styleMask: style, backing: NSBackingStoreType::Buffered, defer: false, screen: screen.as_deref()]
    };
    let window = window.context("AppKit did not create the window")?;
    #[expect(deprecated, reason = "file drops come as a property list of paths")]
    // SAFETY: the type is an AppKit constant.
    window.registerForDraggedTypes(&NSArray::from_slice(&[unsafe {
        objc2_app_kit::NSFilenamesPboardType
    }]));
    // SAFETY: the window is held by `Retained`, not by AppKit's close.
    unsafe { window.setReleasedWhenClosed(false) };
    let content = window
        .contentView()
        .context("The window has no content view")?;
    let view = AppsView::new(mtm, content.bounds(), callbacks, layer, bare);
    let _ = window.ivars().view.set(view.clone());
    if options.transparent {
        window.setOpaque(false);
        // Not `clearColor`: a fully clear background loses the window shadow.
        window.setBackgroundColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(
            0., 0., 0., 0.0001,
        )));
    }
    window.setDelegate(Some(ProtocolObject::from_ref(&*window)));
    window.set_title(&options.title);
    window.setMovable(options.movable);
    if bare {
        window.setTitlebarAppearsTransparent(true);
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
    }
    let fill =
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable;
    view.setAutoresizingMask(fill);
    // Layer backing must be on before the view joins the window, or AppKit can swap in its own layer.
    view.setWantsLayer(true);
    view.setLayerContentsRedrawPolicy(NSViewLayerContentsRedrawPolicy::DuringViewResize);
    // AppKit owns the gravity of the layer, and resize frames can come at an old size, so pin the top left.
    view.setLayerContentsPlacement(NSViewLayerContentsPlacement::TopLeft);
    // AppKit reorders sublayers added by hand, so the overlay gets a view. Subviews stay in this order:
    // page frame, glass, overlay, web views.
    let cover = AppsOverlayView::new(mtm, content.bounds());
    // SAFETY: a plain layer property.
    let () = unsafe { msg_send![&cover, setLayer: &*overlay] };
    cover.setWantsLayer(true);
    cover.setAutoresizingMask(fill);
    view.addSubview(&cover);
    content.addSubview(&view);
    window.makeFirstResponder(Some(&view));
    let tracking = NSTrackingAreaOptions::MouseEnteredAndExited
        | NSTrackingAreaOptions::MouseMoved
        | NSTrackingAreaOptions::ActiveAlways
        | NSTrackingAreaOptions::InVisibleRect;
    // SAFETY: the view keeps the area, which reports to the view.
    let area = unsafe {
        NSTrackingArea::initWithRect_options_owner_userInfo(
            NSTrackingArea::alloc(),
            NSRect::ZERO,
            tracking,
            Some(&view),
            None,
        )
    };
    view.addTrackingArea(&area);
    if options.show {
        window.makeKeyAndOrderFront(None);
    }
    // `initWithContentRect` can place the window wrong when the key screen is not the primary one.
    window.setFrameTopLeftPoint(corner);
    if options.frameless {
        window.show_buttons(false);
    }
    window.locate();
    window.place_buttons();
    Ok((window, view))
}

pub(crate) fn detach(window: &NSWindow) {
    if let Some(parent) = window.sheetParent() {
        parent.endSheet(window);
    }
    if let Some(parent) = window.parentWindow() {
        parent.removeChildWindow(window);
    }
}

impl AppsWindow {
    pub fn view(&self) -> &AppsView {
        self.ivars().view.get().expect("The window has a view")
    }

    // The frame of the page view in the content view. Panels docked at its sides make it narrower.
    fn page_frame(&self) -> NSRect {
        self.view().frame()
    }

    pub fn content_size(&self) -> Size<Pixels> {
        let inner = self.page_frame().size;
        size(px(inner.width as f32), px(inner.height as f32))
    }

    fn scale(&self) -> f32 {
        // AppKit can say 0 for a window without a screen.
        let scale = self
            .screen()
            .map_or(0., |screen| screen.backingScaleFactor()) as f32;
        if scale == 0. { 2. } else { scale }
    }

    fn fullscreen(&self) -> bool {
        self.styleMask().contains(NSWindowStyleMask::FullScreen)
    }

    fn on_screen(&self) -> bool {
        self.occlusionState()
            .contains(NSWindowOcclusionState::Visible)
    }

    fn refresh(&self) {
        let mtm = self.mtm();
        let top = primary_top(mtm);
        let frame = self.frame();
        let minimized = self.isMiniaturized();
        let fullscreen = self.fullscreen();
        let zoomed = match self.styleMask().contains(NSWindowStyleMask::Resizable) {
            true => self.isZoomed(),
            false => {
                let work = self
                    .screen()
                    .map(|screen| screen.visibleFrame())
                    .unwrap_or_default();
                frame.size == work.size
            }
        };
        // A window outside every display has no screen.
        let screen = self
            .screen()
            .or_else(|| NSScreen::screens(mtm).firstObject());
        let screen_rect = |rect: fn(&NSScreen) -> NSRect| {
            screen
                .as_deref()
                .map(|screen| flip(rect(screen), top))
                .unwrap_or_default()
        };
        let (mut content, page) = (self.contentRectForFrameRect(frame), self.page_frame());
        content.origin.x += page.origin.x;
        content.size.width = page.size.width;
        let state = WindowState {
            frame: flip(frame, top),
            content: flip(content, top),
            minimized,
            maximized: zoomed && !minimized && !fullscreen,
            fullscreen,
            visible: self.isVisible(),
            resizing: self.inLiveResize(),
            display: screen_rect(NSScreen::frame),
            work_area: screen_rect(NSScreen::visibleFrame),
        };
        *lock(&self.ivars().snapshot) = Snapshot {
            scale: self.scale(),
            active: self.isKeyWindow(),
            state,
        };
    }

    pub fn report(&self) {
        self.refresh();
        events().changed(self.ivars().callbacks);
    }

    fn locate(&self) {
        let display = self.screen().and_then(|screen| {
            // SAFETY: a screen's description holds its display number.
            unsafe {
                let about: Option<Retained<AnyObject>> = msg_send![&screen, deviceDescription];
                let number: Option<Retained<AnyObject>> =
                    msg_send![&about?, objectForKey: &*NSString::from_str("NSScreenNumber")];
                let number: NSUInteger = msg_send![&number?, unsignedIntegerValue];
                Some(number as u32)
            }
        });
        // AppKit can report no screen while displays change.
        lock(&self.ivars().pacer).place(self.on_screen(), display);
    }

    pub fn update_size(&self) {
        let (scale, content) = (self.scale(), self.content_size());
        let layer = &self.view().ivars().layer;
        layer.setContentsScale(scale as f64);
        let p3 = (self.screen())
            .is_some_and(|screen| screen.canRepresentDisplayGamut(NSDisplayGamut::P3));
        super::metal::set_gamut(layer, p3);
        self.refresh();
        events().resized(self.ivars().callbacks, content, scale);
    }

    pub fn set_title(&self, title: &str) {
        let title = NSString::from_str(title);
        NSApp(self.mtm()).changeWindowsItem_title_filename(self, &title, false);
        self.setTitle(&title);
        self.place_buttons();
    }

    // AppKit can make new buttons at any time, so this reads them again each time.
    fn buttons(&self) -> Option<([Retained<NSButton>; 3], Retained<NSView>)> {
        let close = self.standardWindowButton(NSWindowButton::CloseButton)?;
        let shrink = self.standardWindowButton(NSWindowButton::MiniaturizeButton)?;
        let grow = self.standardWindowButton(NSWindowButton::ZoomButton)?;
        // SAFETY: plain view tree reads.
        let bar = unsafe { close.superview()?.superview()? };
        Some(([close, shrink, grow], bar))
    }

    pub fn show_buttons(&self, visible: bool) {
        if let Some((buttons, _)) = self.buttons() {
            buttons.iter().for_each(|button| button.setHidden(!visible));
        }
    }

    pub fn place_buttons(&self) {
        let ivars = self.ivars();
        let Some(at) = ivars.buttons_at.get() else {
            return;
        };
        if self.fullscreen() && !ivars.leaving_fullscreen.get() {
            return self.reset_buttons();
        }
        let Some((buttons, bar)) = self.buttons() else {
            return;
        };
        let [close, next, last] = buttons.each_ref().map(|button| button.frame());
        if ivars.buttons_home.get().is_none() {
            ivars
                .buttons_home
                .set(Some([bar.frame(), close, next, last]));
        }
        let step = next.origin.x - close.origin.x;
        let (x, y) = (at.x.0 as f64, at.y.0 as f64);
        let mut strip = bar.frame();
        strip.size.height = close.size.height + 2. * y;
        strip.origin.y = self.frame().size.height - strip.size.height;
        bar.setFrame(strip);
        for (index, button) in buttons.iter().enumerate() {
            button.setFrameOrigin(CGPoint::new(x + index as f64 * step, y));
        }
        bar.updateTrackingAreas();
        buttons
            .iter()
            .for_each(|button| button.updateTrackingAreas());
    }

    pub fn reset_buttons(&self) {
        let Some([strip, frames @ ..]) = self.ivars().buttons_home.take() else {
            return;
        };
        let Some((buttons, bar)) = self.buttons() else {
            return;
        };
        for (button, frame) in buttons.iter().zip(frames) {
            button.setFrame(frame);
        }
        bar.setFrame(strip);
        bar.updateTrackingAreas();
        buttons
            .iter()
            .for_each(|button| button.updateTrackingAreas());
    }

    pub fn double_click_title(&self) {
        // SAFETY: plain NSUserDefaults and NSDictionary reads.
        let action: Option<Retained<NSString>> = unsafe {
            let defaults = NSUserDefaults::standardUserDefaults();
            let global: Option<Retained<AnyObject>> = msg_send![&defaults, persistentDomainForName: &*NSString::from_str("NSGlobalDomain")];
            global.and_then(|global| {
                msg_send![&global, objectForKey: &*NSString::from_str("AppleActionOnDoubleClick")]
            })
        };
        let style = self.styleMask();
        match action.map(|action| action.to_string()).as_deref() {
            Some("None") => {}
            Some("Minimize") => {
                if style.contains(NSWindowStyleMask::Miniaturizable) {
                    self.miniaturize(None);
                }
            }
            _ if style.contains(NSWindowStyleMask::Resizable) => self.zoom(None),
            _ => {}
        }
    }

    fn files(&self, event: Event) -> NSDragOperation {
        let over = match event {
            Event::FileEnter { .. } => Some(true),
            Event::FileLeave => Some(false),
            _ => None,
        };
        events().input_later(self.ivars().callbacks, event);
        if let Some(over) = over {
            self.view().ivars().files_over.set(over);
        }
        NSDragOperation::Copy
    }

    fn drop_point(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> Point<Pixels> {
        let at = info.draggingLocation();
        Point::new(
            px((at.x - self.page_frame().origin.x) as f32),
            self.content_size().height - px(at.y as f32),
        )
    }

    // True when the window became a sheet, which shows it.
    fn attach(&self) -> bool {
        let Some(parent) = self.ivars().parent.borrow().as_ref().and_then(Weak::load) else {
            return false;
        };
        if self.ivars().sheet.get() {
            if self.sheetParent().is_none() {
                parent.beginSheet_completionHandler(self, None);
            }
            return true;
        }
        if self.parentWindow().as_deref() != Some(&*parent) {
            // Joining a parent resets the parent's level.
            let level = parent.level();
            // SAFETY: both windows are live, and a window never becomes its own ancestor here.
            unsafe { parent.addChildWindow_ordered(self, NSWindowOrderingMode::Above) };
            parent.setLevel(level);
        }
        false
    }

    pub fn apply(&self, change: WindowChange) {
        let mtm = self.mtm();
        let ivars = self.ivars();
        let zoom_button = || self.standardWindowButton(NSWindowButton::ZoomButton);
        let style = |flag: NSWindowStyleMask, on: bool| {
            // Setting the style mask enables the zoom button again.
            let zoomable = zoom_button().is_none_or(|button| button.isEnabled());
            let mut mask = self.styleMask();
            mask.set(flag, on);
            self.setStyleMask(mask);
            if let Some(button) = zoom_button() {
                button.setEnabled(zoomable);
            }
            self.place_buttons();
        };
        match change {
            WindowChange::Frame(bounds) => {
                let bottom = primary_top(mtm) - (bounds.origin.y.0 + bounds.size.height.0) as f64;
                let origin = CGPoint::new(bounds.origin.x.0 as f64, bottom);
                self.setFrame_display(NSRect::new(origin, cg_size(bounds.size)), true);
            }
            WindowChange::Center => self.center(),
            WindowChange::Show(focus) => {
                if !self.isVisible() {
                    self.view().setNeedsDisplay(true);
                }
                if !self.attach() {
                    match focus {
                        true => self.makeKeyAndOrderFront(None),
                        false => self.orderFrontRegardless(),
                    }
                }
            }
            WindowChange::Hide => {
                detach(self);
                self.orderOut(None);
            }
            WindowChange::Blur => self.orderBack(None),
            WindowChange::Restore => self.deminiaturize(None),
            WindowChange::SizeLimits(min, max) => {
                let most = |value: Pixels| match value > px(0.) {
                    true => value.0 as f64,
                    false => f32::MAX as f64,
                };
                self.setMinSize(cg_size(min));
                self.setMaxSize(CGSize::new(most(max.width), most(max.height)));
            }
            WindowChange::AlwaysOnTop(level) => self.setLevel(level),
            WindowChange::Resizable(on) => style(NSWindowStyleMask::Resizable, on),
            WindowChange::Minimizable(on) => style(NSWindowStyleMask::Miniaturizable, on),
            WindowChange::Closable(on) => style(NSWindowStyleMask::Closable, on),
            WindowChange::Maximizable(on) => {
                if let Some(button) = zoom_button() {
                    button.setEnabled(on);
                }
            }
            WindowChange::Movable(on) => self.setMovable(on),
            WindowChange::Background(color) => {
                let [r, g, b, a] = [color.r, color.g, color.b, color.a].map(f64::from);
                self.setBackgroundColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(
                    r, g, b, a,
                )));
            }
            WindowChange::Opacity(alpha) => self.setAlphaValue(alpha as f64),
            WindowChange::Shadow(on) => self.setHasShadow(on),
            WindowChange::IgnoreMouse(ignore, forward) => {
                self.setIgnoresMouseEvents(ignore);
                self.setAcceptsMouseMovedEvents(ignore && forward);
            }
            WindowChange::AllWorkspaces(all, fullscreen) => {
                let mut behavior = self.collectionBehavior();
                behavior.set(NSWindowCollectionBehavior::CanJoinAllSpaces, all);
                behavior.set(NSWindowCollectionBehavior::FullScreenAuxiliary, fullscreen);
                self.setCollectionBehavior(behavior);
            }
            WindowChange::ContentProtection(on) => self.setSharingType(match on {
                true => NSWindowSharingType::None,
                false => NSWindowSharingType::ReadOnly,
            }),
            WindowChange::Focusable(on) => ivars.focusable.set(on),
            WindowChange::Edited(on) => self.setDocumentEdited(on),
            WindowChange::CancelComposition => {
                if let Some(input) = self.view().inputContext() {
                    input.discardMarkedText();
                }
                return;
            }
            WindowChange::AspectRatio(ratio, extra) => {
                // Resize increments clear the aspect ratio.
                match ratio > 0. {
                    true => self.setContentAspectRatio(CGSize::new(ratio as f64, 1.)),
                    false => self.setResizeIncrements(CGSize::new(1., 1.)),
                }
                ivars
                    .aspect
                    .set((ratio > 0.).then(|| (ratio as f64, cg_size(extra))));
            }
            WindowChange::Parent(handle, sheet) => {
                detach(self);
                let parent = match handle {
                    // SAFETY: the runtime passes the view of a live window.
                    Some(rwh::RawWindowHandle::AppKit(handle)) => {
                        unsafe { handle.ns_view.cast::<NSView>().as_ref() }.window()
                    }
                    _ => None,
                };
                *ivars.parent.borrow_mut() = parent.as_ref().map(Weak::from_retained);
                ivars.sheet.set(sheet);
                if self.isVisible() {
                    self.attach();
                }
            }
            WindowChange::Vibrancy(material, state) => {
                ivars.effect.set(state);
                let Some(content) = self.contentView() else {
                    return;
                };
                // The effect view, when there is one, is the bottom subview.
                let current = content.subviews().firstObject().and_then(|view| {
                    view.isKindOfClass(NSVisualEffectView::class())
                        // SAFETY: checked just above.
                        .then(|| unsafe { Retained::cast_unchecked::<NSVisualEffectView>(view) })
                });
                match (material, current) {
                    (None, Some(effect)) => effect.removeFromSuperview(),
                    (None, None) => {}
                    (Some(material), current) => {
                        let effect = current.unwrap_or_else(|| {
                            let effect = NSVisualEffectView::initWithFrame(
                                NSVisualEffectView::alloc(mtm),
                                content.bounds(),
                            );
                            effect.setAutoresizingMask(
                                NSAutoresizingMaskOptions::ViewWidthSizable
                                    | NSAutoresizingMaskOptions::ViewHeightSizable,
                            );
                            effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
                            content.addSubview_positioned_relativeTo(
                                &effect,
                                NSWindowOrderingMode::Below,
                                None,
                            );
                            effect
                        });
                        effect.setMaterial(NSVisualEffectMaterial(material));
                        effect.setState(NSVisualEffectState(state));
                    }
                }
            }
        }
        self.report();
    }
}

// How long a live resize waits for the app thread to draw the new size.
const RESIZE_WAIT: Duration = Duration::from_millis(8);
const DRAG_REPEAT: Duration = Duration::from_millis(16);

pub(crate) struct Input {
    callbacks: Callbacks,
    pub layer: Retained<CAMetalLayer>,
    owns_titlebar_drag: bool,
    glass: RefCell<Vec<glass::GlassView>>,
    controls: RefCell<controls::Controls>,
    cursor: Cell<Cursor>,
    motion: RefCell<Option<Motion>>,
    press: RefCell<Option<Retained<NSEvent>>>,
    pub files_over: Cell<bool>,
    drag_serial: Cell<usize>,
    flags: RefCell<Option<Event>>,
    equivalent: RefCell<Option<Event>>,
    command: RefCell<Option<(Key, Modifiers)>>,
    command_taken: Cell<Option<bool>>,
}

define_class!(
    // SAFETY: NSView has no subclassing rules, and the class has no Drop.
    #[unsafe(super(NSView, NSResponder))]
    #[thread_kind = MainThreadOnly]
    #[name = "AppsView"]
    #[ivars = Input]
    pub(crate) struct AppsView;

    impl AppsView {
        #[unsafe(method(performKeyEquivalent:))]
        fn key_equivalent(&self, event: &NSEvent) -> bool {
            // AppKit offers key equivalents to every view. Keys belong to a focused web view inside this one.
            let focus = self.window().and_then(|window| window.firstResponder());
            let inner = focus
                .as_deref()
                .and_then(|focus| focus.downcast_ref::<NSView>())
                .is_some_and(|view| !std::ptr::eq(view, &**self) && view.isDescendantOf(self));
            match inner {
                // SAFETY: the NSView implementation.
                true => unsafe { msg_send![super(self), performKeyEquivalent: event] },
                false => self.key(event, true),
            }
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            self.key(event, false);
        }

        #[unsafe(method(keyUp:))]
        fn key_up(&self, event: &NSEvent) {
            self.key(event, false);
        }

        #[unsafe(method(mouseDown:))]
        fn left_down(&self, event: &NSEvent) { self.pointer(event) }
        #[unsafe(method(mouseUp:))]
        fn left_up(&self, event: &NSEvent) { self.pointer(event) }
        #[unsafe(method(rightMouseDown:))]
        fn right_down(&self, event: &NSEvent) { self.pointer(event) }
        #[unsafe(method(rightMouseUp:))]
        fn right_up(&self, event: &NSEvent) { self.pointer(event) }
        #[unsafe(method(otherMouseDown:))]
        fn other_down(&self, event: &NSEvent) { self.pointer(event) }
        #[unsafe(method(otherMouseUp:))]
        fn other_up(&self, event: &NSEvent) { self.pointer(event) }
        #[unsafe(method(mouseMoved:))]
        fn moved(&self, event: &NSEvent) { self.pointer(event) }
        #[unsafe(method(mouseExited:))]
        fn exited(&self, event: &NSEvent) { self.pointer(event) }
        #[unsafe(method(mouseDragged:))]
        fn left_drag(&self, event: &NSEvent) { self.pointer(event) }
        #[unsafe(method(rightMouseDragged:))]
        fn right_drag(&self, event: &NSEvent) { self.pointer(event) }
        #[unsafe(method(otherMouseDragged:))]
        fn other_drag(&self, event: &NSEvent) { self.pointer(event) }
        #[unsafe(method(scrollWheel:))]
        fn wheel(&self, event: &NSEvent) { self.pointer(event) }
        #[unsafe(method(swipeWithEvent:))]
        fn swipe(&self, event: &NSEvent) { self.pointer(event) }
        #[unsafe(method(flagsChanged:))]
        fn flags(&self, event: &NSEvent) { self.pointer(event) }

        #[unsafe(method(acceptsFirstMouse:))]
        fn first_mouse(&self, _: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(resetCursorRects))]
        fn cursor_rects(&self) {
            // SAFETY: the NSView implementation.
            let () = unsafe { msg_send![super(self), resetCursorRects] };
            let name = match self.ivars().cursor.get() {
                Cursor::None => return NSCursor::setHiddenUntilMouseMoves(true),
                Cursor::Default => c"arrowCursor",
                Cursor::Text => c"IBeamCursor",
                Cursor::VerticalText => c"IBeamCursorForVerticalLayout",
                Cursor::Pointer => c"pointingHandCursor",
                Cursor::Crosshair => c"crosshairCursor",
                Cursor::Grab => c"openHandCursor",
                Cursor::Grabbing => c"closedHandCursor",
                Cursor::NotAllowed => c"operationNotAllowedCursor",
                Cursor::EwResize | Cursor::ColResize => c"resizeLeftRightCursor",
                Cursor::NsResize | Cursor::RowResize => c"resizeUpDownCursor",
                Cursor::EResize => c"resizeRightCursor",
                Cursor::WResize => c"resizeLeftCursor",
                Cursor::NResize => c"resizeUpCursor",
                Cursor::SResize => c"resizeDownCursor",
                // Private, but the ones AppKit shows on window corners.
                Cursor::NwseResize => c"_windowResizeNorthWestSouthEastCursor",
                Cursor::NeswResize => c"_windowResizeNorthEastSouthWestCursor",
                Cursor::Alias => c"dragLinkCursor",
                Cursor::Copy => c"dragCopyCursor",
                Cursor::ContextMenu => c"contextualMenuCursor",
            };
            // SAFETY: each name is an NSCursor class method that returns a cursor.
            let cursor: *mut NSCursor = unsafe { NSCursor::class().send_message(Sel::register(name), ()) };
            // SAFETY: as above; the cursors are shared and live for the process.
            if let Some(cursor) = unsafe { cursor.as_ref() } {
                self.addCursorRect_cursor(self.bounds(), cursor);
            }
        }

        #[unsafe(method_id(makeBackingLayer))]
        fn backing_layer(&self) -> Retained<CALayer> {
            Retained::into_super(self.ivars().layer.clone())
        }

        #[unsafe(method(viewDidChangeBackingProperties))]
        fn backing_changed(&self) {
            if let Some(window) = self.host() {
                window.update_size();
            }
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, new: CGSize) {
            if self.frame().size == new {
                return;
            }
            // SAFETY: the NSView implementation.
            let () = unsafe { msg_send![super(self), setFrameSize: new] };
            if let Some(window) = self.host() {
                window.update_size();
            }
        }

        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn appearance_changed(&self) {
            events().appearance_changed(self.ivars().callbacks);
            if let Some(window) = self.host() {
                window.place_buttons();
            }
        }

        #[unsafe(method(_opaqueRectForWindowMoveWhenInTitlebar))]
        fn titlebar_opaque_rect(&self) -> NSRect {
            match self.ivars().owns_titlebar_drag {
                true => self.bounds(),
                false => NSRect::ZERO,
            }
        }
    }

    unsafe impl NSObjectProtocol for AppsView {}

    unsafe impl CALayerDelegate for AppsView {
        // Live resize. A frame that is ready in time presents inside AppKit's transaction, so the page and the
        // window frame change together; a late one waits for the commit, or it would shift the window. A
        // hidden window presents outside the transaction until it is ordered in.
        #[unsafe(method(displayLayer:))]
        fn display_layer(&self, _: &CALayer) {
            let callbacks = self.ivars().callbacks;
            let visible = self.window().is_some_and(|window| window.isVisible());
            if callbacks.thread.late.load(std::sync::atomic::Ordering::Acquire) != 0 {
                return;
            }
            let present = ask_within(callbacks.thread, RESIZE_WAIT, move || {
                events().present_live(callbacks, visible)
            });
            // Presents here, on main, inside the transaction.
            drop(present);
        }
    }

    unsafe impl NSTextInputClient for AppsView {
        #[unsafe(method(hasMarkedText))]
        fn has_marked_text(&self) -> bool {
            events().composition(self.ivars().callbacks).is_some()
        }

        #[unsafe(method(markedRange))]
        fn marked_range(&self) -> NSRange {
            ns_range(events().composition(self.ivars().callbacks))
        }

        #[unsafe(method(selectedRange))]
        fn selected_range(&self) -> NSRange {
            ns_range(events().selection(self.ivars().callbacks).map(|selection| selection.range))
        }

        #[unsafe(method_id(validAttributesForMarkedText))]
        fn marked_attributes(&self) -> Retained<NSArray<NSString>> {
            NSArray::new()
        }

        #[unsafe(method(insertText:replacementRange:))]
        fn insert_text(&self, text: &AnyObject, replacing: NSRange) {
            events().insert(self.ivars().callbacks, span(replacing), &plain(text));
        }

        #[unsafe(method(setMarkedText:selectedRange:replacementRange:))]
        fn set_marked_text(&self, text: &AnyObject, selected: NSRange, replacing: NSRange) {
            events().compose(self.ivars().callbacks, span(replacing), &plain(text), span(selected));
        }

        #[unsafe(method(unmarkText))]
        fn unmark_text(&self) {
            events().end_composition(self.ivars().callbacks);
        }

        #[unsafe(method_id(attributedSubstringForProposedRange:actualRange:))]
        fn substring(&self, proposed: NSRange, actual: *mut NSRange) -> Option<Retained<NSAttributedString>> {
            let callbacks = self.ivars().callbacks;
            span(proposed).filter(|range| !range.is_empty()).and_then(|range| {
                let (text, adjusted) = events().text_in(callbacks, range.clone())?;
                if let Some(adjusted) = adjusted.filter(|adjusted| *adjusted != range)
                    && !actual.is_null()
                {
                    // SAFETY: AppKit passes a writable range or null.
                    unsafe { actual.write(NSRange::from(adjusted)) };
                }
                let text = NSString::from_str(&text);
                Some(NSAttributedString::initWithString(NSAttributedString::alloc(), &text))
            })
        }

        #[unsafe(method(firstRectForCharacterRange:actualRange:))]
        fn first_rect(&self, range: NSRange, _: *mut NSRange) -> NSRect {
            let top_left = self.content_corner();
            let Some(rect) = span(range).and_then(|range| events().rect_of(self.ivars().callbacks, range)) else {
                return NSRect::ZERO;
            };
            let [x, y, width, height] = [rect.origin.x, rect.origin.y, rect.size.width, rect.size.height]
                .map(|value| f64::from(value.0));
            NSRect::new(CGPoint::new(top_left.x + x, top_left.y - y - height), CGSize::new(width, height))
        }

        #[unsafe(method(characterIndexForPoint:))]
        fn index_at(&self, at: CGPoint) -> NSUInteger {
            let top_left = self.content_corner();
            let at = point(px((at.x - top_left.x) as f32), px((top_left.y - at.y) as f32));
            events().index_at(self.ivars().callbacks, at).unwrap_or(NOT_FOUND)
        }

        // AppKit sends the commands of keys that the input method did not take here. Without this method it
        // beeps on keys with modifiers.
        #[unsafe(method(doCommandBySelector:))]
        fn do_command(&self, _: Sel) {
            let Some((key, modifiers)) = self.ivars().command.take() else {
                return;
            };
            let callbacks = self.ivars().callbacks;
            let taken = events().input(callbacks, Event::KeyDown {
                key,
                modifiers,
                repeat: false,
                ime: false,
            });
            self.ivars().command_taken.set(Some(taken));
        }
    }
);

define_class!(
    // SAFETY: NSView has no subclassing rules, and the class has no Drop.
    #[unsafe(super(NSView, NSResponder))]
    #[thread_kind = MainThreadOnly]
    #[name = "AppsOverlayView"]
    pub(crate) struct AppsOverlayView;

    impl AppsOverlayView {
        // The pointer goes through to the page view below.
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _: CGPoint) -> *mut NSView {
            std::ptr::null_mut()
        }

        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl AppsOverlayView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        // SAFETY: the designated initializer of NSView.
        unsafe { msg_send![super(Self::alloc(mtm).set_ivars(())), initWithFrame: frame] }
    }
}

fn repeat_drag(view: Weak<AppsView>, serial: usize, held: Button, event: Event) {
    native_after(DRAG_REPEAT, move || {
        let Some(view) = view.load() else {
            return;
        };
        let ivars = view.ivars();
        if ivars.drag_serial.get() != serial {
            return;
        }
        // A menu can take the button up.
        let bit = BUTTONS
            .iter()
            .position(|button| *button == held)
            .unwrap_or(0);
        if NSEvent::pressedMouseButtons() & (1 << bit) == 0 {
            ivars.drag_serial.set(serial + 1);
            return;
        }
        view.send_pointer(event.clone(), Instant::now());
        repeat_drag(Weak::new(&*view), serial, held, event);
    });
}

pub(crate) fn set_cursor(cursor: Cursor) {
    let app = NSApp(MainThreadMarker::new().unwrap());
    let ours = [app.keyWindow(), app.mainWindow()]
        .into_iter()
        .flatten()
        .find_map(|window| window.downcast::<AppsWindow>().ok());
    let Some(window) = ours.filter(|window| !window.ivars().closed.get()) else {
        return;
    };
    let view = window.view();
    if view.ivars().cursor.replace(cursor) != cursor {
        window.invalidateCursorRectsForView(view);
    }
}

impl AppsView {
    pub fn new(
        mtm: MainThreadMarker,
        frame: NSRect,
        callbacks: Callbacks,
        layer: Retained<CAMetalLayer>,
        owns_titlebar_drag: bool,
    ) -> Retained<Self> {
        let input = Input {
            callbacks,
            layer,
            owns_titlebar_drag,
            glass: RefCell::default(),
            controls: RefCell::default(),
            cursor: Cell::new(Cursor::Default),
            motion: RefCell::new(None),
            press: RefCell::new(None),
            files_over: Cell::new(false),
            drag_serial: Cell::new(0),
            flags: RefCell::new(None),
            equivalent: RefCell::new(None),
            command: RefCell::new(None),
            command_taken: Cell::new(None),
        };
        // SAFETY: the designated initializer of NSView.
        unsafe { msg_send![super(Self::alloc(mtm).set_ivars(input)), initWithFrame: frame] }
    }

    fn host(&self) -> Option<Retained<AppsWindow>> {
        self.window()?.downcast::<AppsWindow>().ok()
    }

    // In screen coordinates.
    fn content_corner(&self) -> CGPoint {
        let Some(window) = self.host() else {
            return CGPoint::ZERO;
        };
        let frame = window.frame();
        let height = match window
            .styleMask()
            .contains(NSWindowStyleMask::FullSizeContentView)
        {
            true => frame.size.height,
            false => window.contentLayoutRect().size.height,
        };
        CGPoint::new(
            frame.origin.x + self.frame().origin.x,
            frame.origin.y + height,
        )
    }

    fn handle_in_context(&self, event: &NSEvent) -> bool {
        self.inputContext()
            .is_some_and(|context| context.handleEvent(event))
    }

    // Things to try after a change here:
    //  U.S. layout: `alt-t` reaches the app; typing `j k` in a field inserts both characters.
    //  Brazilian layout: `" space` types an unmarked quote, `" backspace` deletes the marked quote.
    //  Japanese input: `a i left down up enter enter` types 愛, and `j i` types じ.
    fn key(&self, event: &NSEvent, equivalent: bool) -> bool {
        let ivars = self.ivars();
        let callbacks = ivars.callbacks;
        let Some(input) = translate(event, self.frame()) else {
            return false;
        };
        let down = match &input {
            Event::KeyDown { .. } if equivalent => {
                ivars.equivalent.replace(Some(input.clone()));
                true
            }
            // AppKit sends a key equivalent that nobody took again as a key down.
            Event::KeyDown { .. } if ivars.equivalent.take().as_ref() == Some(&input) => {
                return false;
            }
            Event::KeyDown { .. } => true,
            Event::KeyUp { .. } => false,
            _ => return false,
        };
        let composing = down && events().composition(callbacks).is_some();
        // Main can block the key. A blocked key still goes to an active input method, which must not pass it on.
        let blocked = crate::native::key(callbacks.id, &input, &|| {
            composing || !down && events().composition(callbacks).is_some()
        });
        let Event::KeyDown {
            key,
            modifiers,
            repeat,
            ..
        } = &input
        else {
            return blocked || events().input(callbacks, input);
        };
        let modifiers = *modifiers;
        let plain = !modifiers.ctrl() && !modifiers.function() && !modifiers.command();
        let printable = key
            .text
            .as_deref()
            .is_some_and(|text| !text.chars().any(char::is_control));
        // An input method that composes, like Japanese, gets printable keys before the page.
        let composer = || {
            // SAFETY: Text Input Sources reads of the current source, released below.
            unsafe {
                let current = TISCopyCurrentKeyboardInputSource();
                if current.is_null() {
                    return false;
                }
                let kind = TISGetInputSourceProperty(current, kTISPropertyInputSourceType.cast());
                let ascii = TISGetInputSourceProperty(
                    current,
                    kTISPropertyInputSourceIsASCIICapable.cast(),
                );
                let mode =
                    !kind.is_null() && CFEqual(kind.cast(), kTISTypeKeyboardInputMode.cast()) != 0;
                let latin = !ascii.is_null() && CFBooleanGetValue(ascii as CFBooleanRef);
                CFRelease(current.cast::<c_void>());
                mode && !latin
            }
        };
        let to_composer = !composing
            && plain
            && printable
            && composer()
            && events().accepts_text_input(callbacks);
        if blocked && !composing && !to_composer {
            return true;
        }
        if composing || to_composer || (key.text.is_none() && plain) {
            // The input method goes first. Keys it does not take come back through `doCommandBySelector:`.
            ivars
                .command
                .replace((!blocked).then(|| (key.clone(), modifiers)));
            ivars.command_taken.set(None);
            let held = !blocked && (composing || to_composer);
            if held {
                let key = input.clone();
                events().hold_key(callbacks, key, composing);
            }
            let taken = self.handle_in_context(event);
            ivars.command.replace(None);
            if blocked {
                return true;
            }
            if let Some(taken) = ivars.command_taken.take() {
                return taken;
            }
            if taken {
                if held {
                    events().ime_took_key(callbacks);
                }
                return true;
            }
            return events().input(callbacks, input);
        }
        if events().input(callbacks, input.clone()) {
            return true;
        }
        if *repeat && let Some(text) = &key.text {
            // With press and hold off, a held key types again. When the setting is missing it is on.
            let defaults = NSUserDefaults::standardUserDefaults();
            let hold = defaults.objectForKey(&NSString::from_str("ApplePressAndHoldEnabled"));
            // SAFETY: the setting is a boolean.
            if !hold.is_none_or(|hold| unsafe { msg_send![&hold, boolValue] }) {
                events().insert(callbacks, None, text);
                return true;
            }
        }
        // Key equivalents with modifiers other than fn belong to AppKit, or cmd-` stops working.
        if equivalent && modifiers != Modifiers::FN {
            return false;
        }
        self.handle_in_context(event)
    }

    fn pointer(&self, event: &NSEvent) {
        let ivars = self.ivars();
        let Some(mut input) = translate(event, self.frame()) else {
            return;
        };
        match &mut input {
            // A control-click is a right click on macOS.
            Event::PointerDown {
                button,
                modifiers,
                clicks,
                ..
            }
            | Event::PointerUp {
                button,
                modifiers,
                clicks,
                ..
            } if *button == Button::Left && modifiers.ctrl() => {
                *button = Button::Right;
                *modifiers = modifiers.with(Modifiers::CTRL, false);
                *clicks = 1;
            }
            _ => {}
        }
        let left_held = matches!(
            input,
            Event::PointerDown {
                button: Button::Left,
                ..
            } | Event::PointerMove {
                held: Some(Button::Left),
                ..
            }
        );
        if left_held {
            // Kept to start a window drag later.
            ivars.press.replace(Some(event.retain()));
        }
        match &input {
            Event::PointerDown { .. } => {
                if let Some(window) = self.window()
                    && window
                        .firstResponder()
                        .is_none_or(|focus| !std::ptr::eq(&*focus, self.as_ref()))
                {
                    window.makeFirstResponder(Some(self));
                }
                self.handle_in_context(event);
            }
            Event::PointerMove {
                held: Some(held), ..
            } if !ivars.files_over.get() => {
                let serial = ivars.drag_serial.get() + 1;
                ivars.drag_serial.set(serial);
                repeat_drag(Weak::new(self), serial, *held, input.clone());
            }
            Event::PointerUp { .. } => ivars.drag_serial.set(ivars.drag_serial.get() + 1),
            Event::ModifiersChanged { .. } => {
                if ivars.flags.borrow().as_ref() == Some(&input) {
                    return;
                }
                ivars.flags.replace(Some(input.clone()));
            }
            _ => {}
        }
        // When the device sent the event. The page gets it later, after waits on two threads, and
        // a scroll needs the even times of the device.
        let age = NSProcessInfo::processInfo().systemUptime() - event.timestamp();
        let age = Duration::try_from_secs_f64(age).unwrap_or_default();
        let now = Instant::now();
        self.send_pointer(input, now.checked_sub(age).unwrap_or(now));
    }

    fn send_pointer(&self, input: Event, sent: Instant) {
        let ivars = self.ivars();
        let callbacks = ivars.callbacks;
        if !matches!(input, Event::PointerMove { .. } | Event::Wheel { .. }) {
            ivars.motion.replace(None);
            return events().input_later(callbacks, input);
        }
        let merged = ivars.motion.borrow().as_ref().is_some_and(|slot| {
            lock(slot).as_mut().is_some_and(|(waiting, at)| {
                let merged = merge(waiting, &input);
                if merged {
                    *at = sent;
                }
                merged
            })
        });
        if merged {
            return;
        }
        let slot = Arc::new(Mutex::new(Some((input, sent))));
        ivars.motion.replace(Some(slot.clone()));
        events().input_motion(callbacks, slot);
    }

    pub fn drag_window(&self) {
        let Some(press) = self.ivars().press.borrow().clone() else {
            return;
        };
        // The page asks late, so check that the button is still down: a drag without one never ends.
        if NSEvent::pressedMouseButtons() & 1 != 0
            && let Some(window) = self.window()
        {
            window.performWindowDragWithEvent(&press);
        }
    }

    pub fn update_glass(&self, surfaces: Vec<glass::GlassSurface>) {
        // Code up the stack can be updating the glass. The next update catches up.
        if let Ok(mut views) = self.ivars().glass.try_borrow_mut() {
            glass::update(&mut views, self, surfaces);
        }
    }

    /// False when the controls are in use up the stack, so the caller tries again later.
    pub fn update_controls(
        &self,
        take: impl FnOnce() -> Option<(Vec<crate::NativeControl>, Option<crate::ControlSink>)>,
    ) -> bool {
        let Ok(mut controls) = self.ivars().controls.try_borrow_mut() else {
            return false;
        };
        if let Some((list, sink)) = take() {
            controls::update(&mut controls, self, list, sink);
        }
        true
    }
}
