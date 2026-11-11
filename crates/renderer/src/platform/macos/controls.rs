use super::native_sync;
use crate::{ControlEvent, ControlKind, ControlSink, NativeControl, Pixels, Rect};
use objc2::{
    class, msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject, Bool, ClassBuilder, MessageReceiver, Sel},
    sel,
};
use objc2_app_kit::NSView;
use objc2_foundation::{NSEdgeInsets, NSPoint, NSRect, NSSize, NSString};
use objc2_quartz_core::CATransaction;
use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::{CStr, c_void},
    ptr,
    sync::OnceLock,
};

const SHARED: &CStr = c"appsControls";
const RING: f64 = 8.;

struct Entry {
    kind: ControlKind,
    sent: u64,
    dirty: bool,
}

#[derive(Default)]
struct Shared {
    sink: Option<ControlSink>,
    entries: HashMap<u64, Entry>,
}

struct View {
    clip: Retained<NSView>,
    control: Retained<AnyObject>,
    state: NativeControl,
}

#[derive(Default)]
pub(crate) struct Controls {
    target: Option<Retained<AnyObject>>,
    shared: Box<RefCell<Shared>>,
    views: Vec<View>,
}

impl Drop for Controls {
    fn drop(&mut self) {
        if let Some(target) = &self.target {
            set_shared(target, ptr::null());
        }
        for view in self.views.drain(..) {
            detach(&view);
        }
    }
}

fn detach(view: &View) {
    unsafe {
        let () = msg_send![&*view.control, setTarget: ptr::null_mut::<AnyObject>()];
        if matches!(view.state.kind, ControlKind::Text | ControlKind::Search) {
            let () = msg_send![&*view.control, setDelegate: ptr::null_mut::<AnyObject>()];
        }
    }
    view.clip.removeFromSuperview();
}

fn registered(
    cell: &'static OnceLock<usize>,
    build: impl FnOnce() -> &'static AnyClass,
) -> &'static AnyClass {
    let class = *cell.get_or_init(|| build() as *const AnyClass as usize);
    unsafe { &*(class as *const AnyClass) }
}

fn clip_class() -> &'static AnyClass {
    static CLASS: OnceLock<usize> = OnceLock::new();
    registered(&CLASS, || {
        let mut class = ClassBuilder::new(c"AppsControlClip", class!(NSView)).unwrap();
        unsafe {
            class.add_method(sel!(isFlipped), flipped as extern "C" fn(_, _) -> Bool);
            class.add_method(
                sel!(hitTest:),
                hit_test as extern "C" fn(_, _, NSPoint) -> *mut AnyObject,
            );
        }
        class.register()
    })
}

fn target_class() -> &'static AnyClass {
    static CLASS: OnceLock<usize> = OnceLock::new();
    registered(&CLASS, || {
        let mut class = ClassBuilder::new(c"AppsControlTarget", class!(NSObject)).unwrap();
        unsafe {
            class.add_ivar::<*const c_void>(SHARED);
            class.add_method(
                sel!(controlAction:),
                action as extern "C" fn(_, _, *mut AnyObject),
            );
            class.add_method(
                sel!(controlTextDidChange:),
                text_changed as extern "C" fn(_, _, *mut AnyObject),
            );
            class.add_method(
                sel!(controlTextDidEndEditing:),
                text_ended as extern "C" fn(_, _, *mut AnyObject),
            );
        }
        class.register()
    })
}

extern "C" fn flipped(_: &AnyObject, _: Sel) -> Bool {
    Bool::YES
}

// The clip is wider than its control, and that margin belongs to the page below.
extern "C" fn hit_test(this: &AnyObject, _: Sel, point: NSPoint) -> *mut AnyObject {
    let hit: *mut AnyObject = unsafe { msg_send![super(this, class!(NSView)), hitTest: point] };
    if ptr::eq(hit, this) {
        ptr::null_mut()
    } else {
        hit
    }
}

fn set_shared(target: &AnyObject, shared: *const RefCell<Shared>) {
    unsafe {
        target
            .class()
            .instance_variable(SHARED)
            .unwrap()
            .load_ptr::<*const c_void>(target)
            .write(shared.cast());
    }
}

fn shared(target: &AnyObject) -> Option<&RefCell<Shared>> {
    let shared = unsafe {
        *target
            .class()
            .instance_variable(SHARED)
            .unwrap()
            .load_ptr::<*const c_void>(target)
    };
    // SAFETY: `Controls` clears the pointer before it frees the box.
    (!shared.is_null()).then(|| unsafe { &*shared.cast::<RefCell<Shared>>() })
}

fn send(shared: &RefCell<Shared>, key: u64, value: Option<String>, done: bool) {
    let mut state = shared.borrow_mut();
    let Some(entry) = state.entries.get_mut(&key) else {
        return;
    };
    entry.sent += 1;
    let seq = entry.sent;
    let sink = state.sink.clone();
    drop(state);
    if let Some(sink) = sink {
        sink(ControlEvent {
            key,
            seq,
            value,
            done,
        });
    }
}

fn tag(control: &AnyObject) -> u64 {
    let tag: isize = unsafe { msg_send![control, tag] };
    tag as u64
}

extern "C" fn action(this: &AnyObject, _: Sel, sender: *mut AnyObject) {
    let (Some(shared), Some(sender)) = (shared(this), unsafe { sender.as_ref() }) else {
        return;
    };
    let key = tag(sender);
    let Some(kind) = shared.borrow().entries.get(&key).map(|entry| entry.kind) else {
        return;
    };
    let (value, done) = match kind {
        ControlKind::Button | ControlKind::Switch => (None, true),
        ControlKind::Slider => {
            let app: *mut AnyObject =
                unsafe { msg_send![class!(NSApplication), sharedApplication] };
            let event: *mut AnyObject = unsafe { msg_send![app, currentEvent] };
            let kind: usize = if event.is_null() {
                0
            } else {
                unsafe { event.send_message(Sel::register(c"type"), ()) }
            };
            (Some(number(sender)), !matches!(kind, 1 | 6))
        }
        ControlKind::Stepper => (Some(number(sender)), true),
        ControlKind::Text | ControlKind::Search => (Some(string(sender)), true),
        ControlKind::Color => match color(sender) {
            Some(value) => (Some(value), true),
            None => return,
        },
        ControlKind::Date => (Some(date(sender)), true),
    };
    if done && let Some(entry) = shared.borrow_mut().entries.get_mut(&key) {
        entry.dirty = false;
    }
    send(shared, key, value, done);
}

fn field(notification: *mut AnyObject) -> Option<&'static AnyObject> {
    if notification.is_null() {
        return None;
    }
    let field: *mut AnyObject = unsafe { msg_send![notification, object] };
    unsafe { field.as_ref() }
}

extern "C" fn text_changed(this: &AnyObject, _: Sel, notification: *mut AnyObject) {
    let (Some(shared), Some(field)) = (shared(this), field(notification)) else {
        return;
    };
    let key = tag(field);
    if let Some(entry) = shared.borrow_mut().entries.get_mut(&key) {
        entry.dirty = true;
    }
    send(shared, key, Some(string(field)), false);
}

extern "C" fn text_ended(this: &AnyObject, _: Sel, notification: *mut AnyObject) {
    let (Some(shared), Some(field)) = (shared(this), field(notification)) else {
        return;
    };
    let key = tag(field);
    let dirty = shared
        .borrow_mut()
        .entries
        .get_mut(&key)
        .is_some_and(|entry| std::mem::take(&mut entry.dirty));
    if dirty {
        send(shared, key, Some(string(field)), true);
    }
}

fn number(control: &AnyObject) -> String {
    let value: f64 = unsafe { msg_send![control, doubleValue] };
    value.to_string()
}

fn string(control: &AnyObject) -> String {
    let value: Retained<NSString> = unsafe { msg_send![control, stringValue] };
    value.to_string()
}

fn color(control: &AnyObject) -> Option<String> {
    unsafe {
        let color: *mut AnyObject = msg_send![control, color];
        let space: *mut AnyObject = msg_send![class!(NSColorSpace), sRGBColorSpace];
        let color: *mut AnyObject = msg_send![color, colorUsingColorSpace: space];
        let color = color.as_ref()?;
        let channel = |value: f64| (value.clamp(0., 1.) * 255.).round() as u8;
        let [r, g, b]: [f64; 3] = [
            msg_send![color, redComponent],
            msg_send![color, greenComponent],
            msg_send![color, blueComponent],
        ];
        Some(format!(
            "#{:02x}{:02x}{:02x}",
            channel(r),
            channel(g),
            channel(b)
        ))
    }
}

// Date pickers use UTC, so a day is a whole number of days since 1970.
fn date(control: &AnyObject) -> String {
    let seconds: f64 = unsafe {
        let date: *mut AnyObject = msg_send![control, dateValue];
        msg_send![date, timeIntervalSince1970]
    };
    let z = (seconds / 86400.).floor() as i64 + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (day, month) = (
        doy - (153 * mp + 2) / 5 + 1,
        if mp < 10 { mp + 3 } else { mp - 9 },
    );
    format!(
        "{:04}-{month:02}-{day:02}",
        yoe + era * 400 + i64::from(month <= 2)
    )
}

fn days(value: &str) -> Option<i64> {
    let mut parts = value.trim().splitn(3, '-').map(str::parse::<i64>);
    let (Some(Ok(year)), Some(Ok(month)), Some(Ok(day))) =
        (parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    Some(era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468)
}

fn hex(value: &str) -> Option<[f64; 3]> {
    let value = value.trim().strip_prefix('#')?;
    if value.len() != 6 || !value.is_ascii() {
        return None;
    }
    let channel = |at: usize| {
        u8::from_str_radix(&value[at..at + 2], 16)
            .ok()
            .map(|v| f64::from(v) / 255.)
    };
    Some([channel(0)?, channel(2)?, channel(4)?])
}

fn nsdate(days: i64) -> *mut AnyObject {
    unsafe { msg_send![class!(NSDate), dateWithTimeIntervalSince1970: days as f64 * 86400.] }
}

fn create(kind: ControlKind, target: Option<&AnyObject>) -> Retained<AnyObject> {
    unsafe {
        let control: Retained<AnyObject> = match kind {
            ControlKind::Button => {
                msg_send![class!(NSButton), buttonWithTitle: &*NSString::from_str(""), target: target, action: sel!(controlAction:)]
            }
            ControlKind::Switch => msg_send![class!(NSSwitch), new],
            ControlKind::Slider => {
                msg_send![class!(NSSlider), sliderWithTarget: target, action: sel!(controlAction:)]
            }
            ControlKind::Stepper => msg_send![class!(NSStepper), new],
            ControlKind::Text => {
                msg_send![class!(NSTextField), textFieldWithString: &*NSString::from_str("")]
            }
            ControlKind::Search => msg_send![class!(NSSearchField), new],
            ControlKind::Color => msg_send![class!(NSColorWell), new],
            ControlKind::Date => msg_send![class!(NSDatePicker), new],
        };
        match kind {
            ControlKind::Stepper => {
                let () = msg_send![&*control, setValueWraps: Bool::NO];
                let () = msg_send![&*control, setAutorepeat: Bool::YES];
            }
            ControlKind::Search => {
                let () = msg_send![&*control, setSendsWholeSearchString: Bool::YES];
            }
            ControlKind::Date => {
                let () = msg_send![&*control, setDatePickerStyle: 0usize];
                let () = msg_send![&*control, setDatePickerElements: 0x00e0usize];
                let () = msg_send![&*control, setBezeled: Bool::YES];
                let zone: *mut AnyObject =
                    msg_send![class!(NSTimeZone), timeZoneForSecondsFromGMT: 0isize];
                let () = msg_send![&*control, setTimeZone: zone];
                let calendar: *mut AnyObject = msg_send![class!(NSCalendar), calendarWithIdentifier: &*NSString::from_str("gregorian")];
                let () = msg_send![&*control, setCalendar: calendar];
            }
            _ => {}
        }
        control
    }
}

pub(crate) fn control_size(kind: ControlKind) -> [f32; 2] {
    static SIZES: OnceLock<[[f32; 2]; 8]> = OnceLock::new();
    SIZES.get_or_init(|| native_sync(measure))[kind as usize]
}

fn measure() -> [[f32; 2]; 8] {
    let kinds = [
        ControlKind::Button,
        ControlKind::Switch,
        ControlKind::Slider,
        ControlKind::Stepper,
        ControlKind::Text,
        ControlKind::Search,
        ControlKind::Color,
        ControlKind::Date,
    ];
    kinds.map(|kind| unsafe {
        let control = create(kind, None);
        if kind == ControlKind::Button {
            let () = msg_send![&*control, setTitle: &*NSString::from_str("Button")];
        }
        let size: NSSize = msg_send![&*control, intrinsicContentSize];
        let width = if kind == ControlKind::Button {
            let font: *mut AnyObject = msg_send![class!(NSFont), systemFontOfSize: 13f64];
            let attributes: Retained<AnyObject> = msg_send![class!(NSDictionary), dictionaryWithObject: font, forKey: &*NSString::from_str("NSFont")];
            let text: NSSize = msg_send![&*NSString::from_str("Button"), sizeWithAttributes: &*attributes];
            size.width - text.width
        } else {
            size.width.max(0.)
        };
        [width as f32, size.height.max(0.) as f32]
    })
}

fn rect([x, y, width, height]: [f64; 4]) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

fn edges(bounds: Rect<Pixels>) -> [f64; 4] {
    [
        bounds.origin.x,
        bounds.origin.y,
        bounds.origin.x + bounds.size.width,
        bounds.origin.y + bounds.size.height,
    ]
    .map(|value| f64::from(f32::from(value)))
}

pub(crate) fn update(
    controls: &mut Controls,
    parent: &NSView,
    list: Vec<NativeControl>,
    sink: Option<ControlSink>,
) {
    CATransaction::setDisableActions(true);
    let target = controls
        .target
        .get_or_insert_with(|| unsafe {
            let target: Retained<AnyObject> = msg_send![target_class(), new];
            set_shared(&target, &*controls.shared);
            target
        })
        .clone();
    let mut shared = controls.shared.borrow_mut();
    shared.sink = sink;
    controls.views.retain(|view| {
        let keep = list
            .iter()
            .any(|state| state.key == view.state.key && state.kind == view.state.kind);
        if !keep {
            detach(view);
            shared.entries.remove(&view.state.key);
        }
        keep
    });
    let height = parent.bounds().size.height;
    for state in list {
        let index = controls
            .views
            .iter()
            .position(|view| view.state.key == state.key);
        let fresh = index.is_none();
        let index = index.unwrap_or_else(|| unsafe {
            let control = create(state.kind, Some(&target));
            let () = msg_send![&*control, setTarget: &*target];
            let () = msg_send![&*control, setAction: sel!(controlAction:)];
            let () = msg_send![&*control, setTag: state.key as isize];
            if matches!(state.kind, ControlKind::Text | ControlKind::Search) {
                let () = msg_send![&*control, setDelegate: &*target];
            }
            let clip: Retained<NSView> = msg_send![clip_class(), new];
            let () = msg_send![&*clip, setWantsLayer: Bool::YES];
            let layer: *mut AnyObject = msg_send![&*clip, layer];
            let () = msg_send![layer, setMasksToBounds: Bool::YES];
            let () = msg_send![&*clip, addSubview: &*control];
            parent.addSubview(&clip);
            shared.entries.insert(
                state.key,
                Entry {
                    kind: state.kind,
                    sent: 0,
                    dirty: false,
                },
            );
            controls.views.push(View {
                clip,
                control,
                state: state.clone(),
            });
            controls.views.len() - 1
        });
        let view = &mut controls.views[index];
        let old = &view.state;
        let control = &*view.control;
        unsafe {
            if fresh || old.bounds != state.bounds || old.clip != state.clip {
                let [left, top, right, bottom] = edges(state.bounds);
                let clip = edges(state.clip);
                let outer = [
                    (left - RING).max(clip[0]),
                    (top - RING).max(clip[1]),
                    (right + RING).min(clip[2]),
                    (bottom + RING).min(clip[3]),
                ];
                let shown = outer[2] > outer[0] && outer[3] > outer[1];
                view.clip.setHidden(!shown);
                if shown {
                    let y = if parent.isFlipped() {
                        outer[1]
                    } else {
                        height - outer[3]
                    };
                    view.clip.setFrame(rect([
                        outer[0],
                        y,
                        outer[2] - outer[0],
                        outer[3] - outer[1],
                    ]));
                    let insets: NSEdgeInsets = msg_send![control, alignmentRectInsets];
                    let frame = rect([
                        left - outer[0] - insets.left,
                        top - outer[1] - insets.top,
                        right - left + insets.left + insets.right,
                        bottom - top + insets.top + insets.bottom,
                    ]);
                    let () = msg_send![control, setFrame: frame];
                }
            }
            if fresh || old.dark != state.dark {
                let appearance: *mut AnyObject = match state.dark {
                    Some(dark) => {
                        msg_send![class!(NSAppearance), appearanceNamed: &*NSString::from_str(if dark { "NSAppearanceNameDarkAqua" } else { "NSAppearanceNameAqua" })]
                    }
                    None => ptr::null_mut(),
                };
                let () = msg_send![control, setAppearance: appearance];
            }
            if fresh || old.enabled != state.enabled {
                let () = msg_send![control, setEnabled: Bool::new(state.enabled)];
            }
            // Apply page values only after it acknowledges all native edits, or stale values can undo them.
            let current = shared
                .entries
                .get(&state.key)
                .is_none_or(|entry| state.seen >= entry.sent);
            let changed = |old: &NativeControl| {
                fresh
                    || old.value != state.value
                    || old.checked != state.checked
                    || old.min != state.min
                    || old.max != state.max
                    || old.step != state.step
                    || old.seen != state.seen
            };
            if current && changed(old) {
                let number = |text: &str| {
                    text.trim()
                        .parse::<f64>()
                        .ok()
                        .filter(|value| value.is_finite())
                };
                match state.kind {
                    ControlKind::Button => {
                        let () = msg_send![control, setTitle: &*NSString::from_str(&state.value)];
                    }
                    ControlKind::Switch => {
                        let () = msg_send![control, setState: isize::from(state.checked)];
                    }
                    ControlKind::Slider | ControlKind::Stepper => {
                        let min = number(&state.min).unwrap_or(0.);
                        let max = number(&state.max).unwrap_or(100.).max(min);
                        let value = number(&state.value)
                            .unwrap_or((min + max) / 2.)
                            .clamp(min, max);
                        let () = msg_send![control, setMinValue: min];
                        let () = msg_send![control, setMaxValue: max];
                        if state.kind == ControlKind::Stepper {
                            let step = number(&state.step).filter(|step| *step > 0.).unwrap_or(1.);
                            let () = msg_send![control, setIncrement: step];
                        }
                        let () = msg_send![control, setDoubleValue: value];
                    }
                    ControlKind::Text | ControlKind::Search => {
                        if string(control) != *state.value {
                            let () = msg_send![control, setStringValue: &*NSString::from_str(&state.value)];
                        }
                    }
                    ControlKind::Color => {
                        let [r, g, b] = hex(&state.value).unwrap_or([0.; 3]);
                        let color: *mut AnyObject = msg_send![class!(NSColor), colorWithSRGBRed: r, green: g, blue: b, alpha: 1f64];
                        let () = msg_send![control, setColor: color];
                    }
                    ControlKind::Date => {
                        let limit = |value: &str| days(value).map_or(ptr::null_mut(), nsdate);
                        let () = msg_send![control, setMinDate: limit(&state.min)];
                        let () = msg_send![control, setMaxDate: limit(&state.max)];
                        if let Some(day) = days(&state.value) {
                            let () = msg_send![control, setDateValue: nsdate(day)];
                        }
                    }
                }
            }
            if matches!(state.kind, ControlKind::Text | ControlKind::Search)
                && (fresh || old.placeholder != state.placeholder || old.editable != state.editable)
            {
                let () = msg_send![control, setPlaceholderString: &*NSString::from_str(&state.placeholder)];
                let () = msg_send![control, setEditable: Bool::new(state.editable)];
                let () = msg_send![control, setSelectable: Bool::YES];
            }
        }
        view.state = state;
    }
}
