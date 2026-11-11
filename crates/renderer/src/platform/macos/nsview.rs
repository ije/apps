use super::{
    controls,
    events::{TISGetInputSourceProperty, translate},
    glass, lock, native_after,
    nswindow::AppsWindow,
    threads::ask_within,
    vsync::step,
};
use crate::{
    Button, Cursor, Event, Key, Modifiers, Pixels, WheelPhase,
    app::{Callbacks, dispatch, send_ime_key},
    point, px,
};
use core_foundation_sys::{
    base::{CFEqual, CFRelease},
    number::{CFBooleanGetValue, CFBooleanRef},
    string::CFStringRef,
};
use objc2::{
    AllocAnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message,
    define_class, msg_send,
    rc::{Retained, Weak},
    runtime::{AnyObject, MessageReceiver, NSObjectProtocol, Sel},
};
use objc2_app_kit::{
    NSApp, NSCursor, NSEvent, NSResponder, NSTextInputClient, NSView, NSWindowStyleMask,
};
use objc2_core_foundation::{CGPoint, CGSize};
use objc2_foundation::{
    NSArray, NSAttributedString, NSNotFound, NSProcessInfo, NSRange, NSRect, NSString, NSUInteger,
    NSUserDefaults,
};
use objc2_quartz_core::{CALayer, CALayerDelegate, CAMetalLayer};
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    ops::Range,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentKeyboardInputSource() -> *mut AnyObject;
    static kTISPropertyInputSourceType: CFStringRef;
    static kTISTypeKeyboardInputMode: CFStringRef;
    static kTISPropertyInputSourceIsASCIICapable: CFStringRef;
}

// How long a live resize waits for the app thread to draw the new size.
const RESIZE_WAIT: Duration = Duration::from_millis(8);
const DRAG_REPEAT: Duration = Duration::from_millis(16);

// The newest pointer move or wheel the app thread has not taken yet, to merge more into it, and
// when its device sent it.
type Motion = Arc<Mutex<Option<(Event, Instant)>>>;

pub(crate) struct Input {
    callbacks: Callbacks,
    pub layer: Retained<CAMetalLayer>,
    owns_titlebar_drag: bool,
    glass: RefCell<Vec<glass::GlassView>>,
    controls: RefCell<controls::Controls>,
    cursor: Cell<Cursor>,
    motion: RefCell<Option<Motion>>,
    first_mouse: Cell<bool>,
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
            self.ivars().first_mouse.set(true);
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
            self.ivars().callbacks.appearance_changed();
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
                callbacks.with(move |window, _| window.platform.set_presents_with_transaction(visible));
                step(callbacks);
                callbacks.with(|window, _| {
                    window.platform.set_presents_with_transaction(false);
                    window.platform.take_present()
                })
            });
            // Presents here, on main, inside the transaction.
            drop(present);
        }
    }

    unsafe impl NSTextInputClient for AppsView {
        #[unsafe(method(hasMarkedText))]
        fn has_marked_text(&self) -> bool {
            self.ivars().callbacks.composition().is_some()
        }

        #[unsafe(method(markedRange))]
        fn marked_range(&self) -> NSRange {
            ns_range(self.ivars().callbacks.composition())
        }

        #[unsafe(method(selectedRange))]
        fn selected_range(&self) -> NSRange {
            ns_range(self.ivars().callbacks.selection().map(|selection| selection.range))
        }

        #[unsafe(method_id(validAttributesForMarkedText))]
        fn marked_attributes(&self) -> Retained<NSArray<NSString>> {
            NSArray::new()
        }

        #[unsafe(method(insertText:replacementRange:))]
        fn insert_text(&self, text: &AnyObject, replacing: NSRange) {
            self.ivars().callbacks.insert(span(replacing), &plain(text));
        }

        #[unsafe(method(setMarkedText:selectedRange:replacementRange:))]
        fn set_marked_text(&self, text: &AnyObject, selected: NSRange, replacing: NSRange) {
            self.ivars().callbacks.compose(span(replacing), &plain(text), span(selected));
        }

        #[unsafe(method(unmarkText))]
        fn unmark_text(&self) {
            self.ivars().callbacks.end_composition();
        }

        #[unsafe(method_id(attributedSubstringForProposedRange:actualRange:))]
        fn substring(&self, proposed: NSRange, actual: *mut NSRange) -> Option<Retained<NSAttributedString>> {
            let callbacks = self.ivars().callbacks;
            span(proposed).filter(|range| !range.is_empty()).and_then(|range| {
                let (text, adjusted) = callbacks.text_in(range.clone())?;
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
            let Some(rect) = span(range).and_then(|range| self.ivars().callbacks.rect_of(range)) else {
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
            self.ivars().callbacks.index_at(at).unwrap_or(NOT_FOUND)
        }

        // AppKit sends the commands of keys that the input method did not take here. Without this method it
        // beeps on keys with modifiers.
        #[unsafe(method(doCommandBySelector:))]
        fn do_command(&self, _: Sel) {
            let Some((key, modifiers)) = self.ivars().command.take() else {
                return;
            };
            let callbacks = self.ivars().callbacks;
            let taken = callbacks.input(Event::KeyDown {
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

const NOT_FOUND: usize = NSNotFound as usize;

fn span(range: NSRange) -> Option<Range<usize>> {
    (range.location != NOT_FOUND).then(|| range.location..range.location + range.length)
}

fn ns_range(range: Option<Range<usize>>) -> NSRange {
    range.map_or(NSRange::new(NOT_FOUND, 0), NSRange::from)
}

// Input methods send plain or attributed strings.
fn plain(text: &AnyObject) -> String {
    match text.downcast_ref::<NSAttributedString>() {
        Some(text) => text.string().to_string(),
        None => text
            .downcast_ref::<NSString>()
            .map(NSString::to_string)
            .unwrap_or_default(),
    }
}

// A merge keeps the order of events: only moves with the same buttons and modifiers, and wheel deltas of
// one gesture, combine.
fn merge(waiting: &mut Event, next: &Event) -> bool {
    match (waiting, next) {
        (
            waiting @ Event::PointerMove { .. },
            Event::PointerMove {
                held, modifiers, ..
            },
        ) => {
            let Event::PointerMove {
                held: was_held,
                modifiers: was_modifiers,
                ..
            } = waiting
            else {
                unreachable!()
            };
            if was_held != held || was_modifiers != modifiers {
                return false;
            }
            *waiting = next.clone();
            true
        }
        (
            Event::Wheel {
                at,
                delta,
                lines,
                phase,
                modifiers,
            },
            Event::Wheel {
                at: next_at,
                delta: next_delta,
                lines: next_lines,
                phase: next_phase,
                modifiers: next_modifiers,
            },
        ) => {
            let same_gesture = *phase == *next_phase
                || (*phase == WheelPhase::Began && *next_phase == WheelPhase::Changed);
            if modifiers != next_modifiers || lines != next_lines || !same_gesture {
                return false;
            }
            *at = *next_at;
            *delta += *next_delta;
            true
        }
        _ => false,
    }
}

fn repeat_drag(view: Weak<AppsView>, serial: usize, event: Event) {
    native_after(DRAG_REPEAT, move || {
        let Some(view) = view.load() else {
            return;
        };
        let ivars = view.ivars();
        if ivars.drag_serial.get() != serial {
            return;
        }
        let Event::PointerMove { held, .. } = &event else {
            return;
        };
        // AppKit numbers the buttons left, right, middle, back, forward. A menu can take the button up.
        let bit = match held {
            Some(Button::Right) => 1,
            Some(Button::Middle) => 2,
            Some(Button::Back) => 3,
            Some(Button::Forward) => 4,
            _ => 0,
        };
        if NSEvent::pressedMouseButtons() & (1 << bit) == 0 {
            ivars.drag_serial.set(serial + 1);
            return;
        }
        view.send_pointer(event.clone(), Instant::now());
        repeat_drag(Weak::from_retained(&view), serial, event);
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
            first_mouse: Cell::new(false),
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

    fn height(&self) -> Pixels {
        self.host()
            .map_or(px(0.), |window| window.content_size().height)
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
        CGPoint::new(frame.origin.x, frame.origin.y + height)
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
        // SAFETY: AppKit passes a live NSEvent.
        let Some(input) =
            (unsafe { translate(std::ptr::from_ref(event).cast_mut().cast(), self.height()) })
        else {
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
        let composing = down && callbacks.composition().is_some();
        // Main can block the key. A blocked key still goes to an active input method, which must not pass it on.
        let blocked = crate::app::native::key(callbacks.id, &input, &|| {
            composing || !down && callbacks.composition().is_some()
        });
        let Event::KeyDown {
            key,
            modifiers,
            repeat,
            ..
        } = &input
        else {
            return blocked || callbacks.input(input);
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
        let to_composer =
            !composing && plain && printable && composer() && callbacks.accepts_text_input();
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
                callbacks.queue(move |window, _| window.ime_key = Some((key, composing)));
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
                    callbacks.queue(|window, app| {
                        send_ime_key(window, app, false);
                    });
                }
                return true;
            }
            return callbacks.input(input);
        }
        if callbacks.input(input.clone()) {
            return true;
        }
        if *repeat && let Some(text) = &key.text {
            // With press and hold off, a held key types again. When the setting is missing it is on.
            let defaults = NSUserDefaults::standardUserDefaults();
            let hold = defaults.objectForKey(&NSString::from_str("ApplePressAndHoldEnabled"));
            // SAFETY: the setting is a boolean.
            if !hold.is_none_or(|hold| unsafe { msg_send![&hold, boolValue] }) {
                callbacks.insert(None, text);
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
        // SAFETY: AppKit passes a live NSEvent.
        let Some(mut input) =
            (unsafe { translate(std::ptr::from_ref(event).cast_mut().cast(), self.height()) })
        else {
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
            Event::PointerDown {
                button: Button::Left,
                first_click,
                ..
            } if ivars.first_mouse.get() => {
                *first_click = true;
                ivars.first_mouse.set(false);
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
            Event::PointerMove { held: Some(_), .. } if !ivars.files_over.get() => {
                let serial = ivars.drag_serial.get() + 1;
                ivars.drag_serial.set(serial);
                repeat_drag(Weak::from_retained(&self.retain()), serial, input.clone());
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
            return callbacks.input_later(input);
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
        callbacks.queue(move |window, app| {
            if let Some((input, sent)) = lock(&slot).take() {
                window.event_at = Some(sent);
                dispatch(window, app, input);
                window.event_at = None;
            }
        });
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
