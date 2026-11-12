use super::{
    lock,
    nsview::{AppsOverlayView, AppsView},
    vsync::Pacer,
    window::{Shared, Snapshot, cg_size, flip, primary_top},
};
use crate::{
    Event, Pixels, Point, Size, WindowChange, WindowOptions, WindowState, app::Callbacks, px, size,
};
use anyhow::{Context as _, Result};
use objc2::{
    AllocAnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class,
    msg_send,
    rc::{Retained, Weak},
    runtime::{AnyObject, NSObjectProtocol, ProtocolObject},
};
use objc2_app_kit::{
    NSApp, NSAutoresizingMaskOptions, NSBackingStoreType, NSButton, NSColor, NSDragOperation,
    NSDraggingInfo, NSScreen, NSTrackingArea, NSTrackingAreaOptions, NSView,
    NSViewLayerContentsPlacement, NSViewLayerContentsRedrawPolicy, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindow, NSWindowButton,
    NSWindowCollectionBehavior, NSWindowDelegate, NSWindowOcclusionState, NSWindowOrderingMode,
    NSWindowSharingType, NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_core_foundation::{CGPoint, CGSize};
use objc2_foundation::{
    NSArray, NSOperatingSystemVersion, NSProcessInfo, NSRect, NSString, NSUInteger, NSUserDefaults,
};
use objc2_quartz_core::{CALayer, CAMetalLayer};
use raw_window_handle as rwh;
use std::{
    cell::{Cell, OnceCell, RefCell},
    path::PathBuf,
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
            {
                let mut pacer = lock(&self.ivars().pacer);
                pacer.closed = true;
                pacer.sync();
            }
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
            self.ivars().callbacks.should_close()
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
            match self.ivars().callbacks.will_change(edge, bounds) {
                true => frame.size,
                false => size,
            }
        }

        #[unsafe(method(windowWillMove:))]
        fn will_move(&self, _: &AnyObject) {
            let bounds = flip(self.frame(), primary_top(self.mtm()));
            self.ivars().callbacks.will_change("move", bounds);
        }

        #[unsafe(method(windowDidChangeOcclusionState:))]
        fn occlusion_changed(&self, _: &AnyObject) {
            self.locate();
            if self.on_screen() {
                self.place_buttons();
            } else {
                self.ivars().callbacks.release();
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
        self.ivars().callbacks.changed();
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
        let mut pacer = lock(&self.ivars().pacer);
        pacer.visible = self.on_screen();
        // AppKit can report no screen while displays change.
        pacer.display = display;
        pacer.sync();
    }

    pub fn update_size(&self) {
        let (scale, content) = (self.scale(), self.content_size());
        self.view().ivars().layer.setContentsScale(scale as f64);
        self.refresh();
        self.ivars().callbacks.resized(content, scale);
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
        self.ivars().callbacks.input_later(event);
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
