use super::snap::ScrollSnaps;
use crate::{
    App, Length, Overflow, Overscroll, Pixels, Point, Rect, Region, Size, Style, WheelPhase,
    Window, point, px,
};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

pub(super) struct Wheel {
    pub(super) handle: ScrollRef,
    pub(super) region: Region,
    pub(super) overflow: Point<Overflow>,
    pub(super) overscroll: Point<Overscroll>,
    pub(super) one_axis: bool,
    pub(super) line_height: Pixels,
}

impl Wheel {
    pub(super) fn scroll(
        &self,
        delta: Point<f32>,
        lines: bool,
        phase: WheelPhase,
        window: &mut Window,
        app: &mut App,
    ) {
        let handle = &self.handle;
        let latched = window
            .gesture
            .latch
            .as_ref()
            .map(|latch| Rc::ptr_eq(&latch.0, &handle.0));
        let elsewhere = || {
            !self.region.under_pointer(window)
                || window
                    .scroll_chain
                    .as_ref()
                    .is_some_and(|chain| !chain.iter().any(|other| Rc::ptr_eq(&other.0, &handle.0)))
        };
        if latched == Some(false) || latched.is_none() && elsewhere() {
            return;
        }
        let mut delta = match lines {
            true => delta.map(|value| self.line_height * value),
            false => delta.map(px),
        };
        if !lines {
            match window.rails.0 {
                [x, y] if y >= x => delta.x = Pixels::ZERO,
                _ => delta.y = Pixels::ZERO,
            }
        }
        let (overflow, overscroll) = (self.overflow, self.overscroll);
        let along = |own: Overflow, delta: Pixels, across: Pixels, other: Overflow| {
            if own != Overflow::Scroll {
                Pixels::ZERO
            } else if !delta.is_zero() {
                delta
            } else if !self.one_axis && other != Overflow::Scroll {
                across
            } else {
                Pixels::ZERO
            }
        };
        let mut moved = point(
            along(overflow.x, delta.x, delta.y, overflow.y),
            along(overflow.y, delta.y, delta.x, overflow.x),
        );
        if !moved.x.is_zero() && !moved.y.is_zero() {
            match moved.x.abs() > moved.y.abs() {
                true => moved.y = Pixels::ZERO,
                false => moved.x = Pixels::ZERO,
            }
        }
        let extent = handle.extent();
        let stops = [(overscroll.x, delta.x), (overscroll.y, delta.y)]
            .iter()
            .any(|(own, delta)| !delta.is_zero() && *own != Overscroll::Auto);
        if lines {
            let from = handle.target();
            let to = handle.clamped(from + moved);
            if to != from {
                app.stop_event();
                handle.animate_to(to, true);
                handle.settle(window);
                handle.drive(window);
            } else if stops {
                app.stop_event();
            }
            return;
        }
        let momentum = phase == WheelPhase::Momentum;
        if momentum && window.gesture.spent {
            app.stop_event();
            return;
        }
        if momentum && latched == Some(true) && handle.fling(window.gesture.speed, moved) {
            window.gesture.spent = true;
            handle.drive(window);
            app.stop_event();
            return;
        }
        let held = phase == WheelPhase::Began && handle.hold();
        let (panned, over) = handle.pan(moved, window.event_at);
        let latched = latched == Some(true) || held || panned;
        if latched {
            window.gesture.latch = Some(handle.clone());
            window.gesture.edge = None;
        }
        let bounce = |own: Overscroll, max: Pixels, over: Pixels| match own {
            Overscroll::None => px(0.),
            _ if max > px(0.) => over,
            _ => px(0.),
        };
        let rest = point(
            bounce(overscroll.x, extent.x, over.x),
            bounce(overscroll.y, extent.y, over.y),
        );
        if rest != Point::default() {
            // An outer scroller may take the rest first. It comes back here if none does.
            if !latched && !stops {
                window.gesture.edge = Some((handle.clone(), rest));
                return;
            }
            window.overscroll(handle.clone(), rest, phase);
        } else if momentum && latched && over != Point::default() {
            window.gesture.spent = true;
        }
        if latched || stops {
            app.stop_event();
        }
        if panned {
            handle.grab();
            if phase == WheelPhase::None {
                handle.loosen(window);
                handle.settle(window);
            }
            handle.drive(window);
            window.refresh();
        }
    }
}

#[derive(Default)]
struct ScrollState {
    offset: Point<Pixels>,
    // The offset of the last prepaint, in whole device pixels like Chrome, so a scroll moves content by whole pixels.
    painted: Point<Pixels>,
    // The lowest and the highest offset. Content that scrolls from its end has positive offsets.
    range: [Point<Pixels>; 2],
    // The scroll size less the client size before they round to device pixels, which `scrollWidth` rounds.
    overflow: Point<Pixels>,
    bounds: Rect,
    animation: Option<Animation>,
    driven: bool,
    stretch: Point<Pixels>,
    spring: Option<Spring>,
    loose: bool,
    glide: Option<Glide>,
    anchor: Option<Anchor>,
    snaps: Option<Rc<ScrollSnaps>>,
    // The areas that the box is at on each axis.
    snapped: [Option<u64>; 2],
    // A gesture moves the box, which snaps when it ends.
    grabbed: bool,
    // The box snaps when it comes to rest.
    settling: bool,
    // The areas of the last `scrollsnapchange` and `scrollsnapchanging`.
    reported: [Option<u64>; 2],
    aimed: [Option<u64>; 2],
    // A scroll, and not only layouts, moved the box since it was at rest.
    scrolled: bool,
}

// The scroll anchor: a box that keeps its place in the scroll viewport when the content above it changes size.
#[derive(Clone, Copy)]
pub(crate) struct Anchor {
    pub(crate) key: (u64, u32),
    // The top of the box in the viewport, in device pixels.
    pub(crate) at: f32,
    // The offset that the anchor holds for. Any other move of the offset drops it.
    pub(crate) offset: Point<Pixels>,
    // A box from the anchor to the scroll container changed its place or size by style, so the next layout keeps
    // the offset and drops the anchor.
    pub(crate) suppressed: bool,
}

const STIFFNESS: f32 = 20.;

// A trackpad delta that carries the time its device sent it reaches the offset spread over the time to
// the next event. Each frame then moves by its share of time, whatever the rate of the events and
// wherever they fall between frames.
struct Glide {
    to: Point<Pixels>,
    from: Instant,
    until: Instant,
    event: Instant,
    pace: Duration,
}

const PACE: Duration = Duration::from_micros(16_667);

struct Spring {
    from: Point<Pixels>,
    speed: Point<f32>,
    start: Instant,
}

impl Spring {
    fn at(&self, now: Instant) -> Option<Point<Pixels>> {
        let t = now.saturating_duration_since(self.start).as_secs_f32();
        let damping = (-12.5 * t).exp();
        let axis = |from: Pixels, speed: f32| px((f32::from(from) + 0.31 * speed * t) * damping);
        let at = point(
            axis(self.from.x, self.speed.x),
            axis(self.from.y, self.speed.y),
        );
        (t < 0.08 || at.x.abs() >= px(0.25) || at.y.abs() >= px(0.25)).then_some(at)
    }
}

const WHEEL: Duration = Duration::from_millis(150);

struct Animation {
    from: Point<Pixels>,
    to: Point<Pixels>,
    slope: Point<f32>,
    start: Instant,
    duration: Duration,
}

impl Animation {
    fn at(&self, now: Instant) -> (Point<Pixels>, Point<f32>) {
        let duration = self.duration.as_secs_f32();
        let u = (now.saturating_duration_since(self.start).as_secs_f32() / duration).min(1.);
        let axis = |from: Pixels, to: Pixels, s: f32| {
            let delta = f32::from(to - from);
            (
                from + px(delta * u * (s + u * (3. - 2. * s + u * (s - 2.)))),
                delta * (1. - u) * (s + (6. - 3. * s) * u) / duration,
            )
        };
        let (x, speed_x) = axis(self.from.x, self.to.x, self.slope.x);
        let (y, speed_y) = axis(self.from.y, self.to.y, self.slope.y);
        (point(x, y), point(speed_x, speed_y))
    }
}

impl ScrollState {
    fn pan(&mut self, delta: Point<Pixels>, sent: Option<Instant>) -> (bool, Point<Pixels>) {
        let axis = |offset: &mut Pixels,
                    stretch: &mut Pixels,
                    [low, high]: [Pixels; 2],
                    mut delta: Pixels| {
            let mut moved = false;
            if !stretch.is_zero() && !delta.is_zero() {
                moved = true;
                let next = *stretch + delta / STIFFNESS;
                if next.signum() == stretch.signum() {
                    *stretch = next;
                    return (moved, px(0.));
                }
                *stretch = px(0.);
                delta = next * STIFFNESS;
            }
            let to = (*offset + delta).clamp(low, high);
            let rest = *offset + delta - to;
            moved |= to != *offset;
            *offset = to;
            (moved, rest)
        };
        let [low, high] = self.range;
        let mut aim = self.glide.as_ref().map_or(self.offset, |glide| glide.to);
        let resting = aim == self.offset;
        let (moved_x, x) = axis(&mut aim.x, &mut self.stretch.x, [low.x, high.x], delta.x);
        let (moved_y, y) = axis(&mut aim.y, &mut self.stretch.y, [low.y, high.y], delta.y);
        let moved = moved_x || moved_y;
        if moved {
            self.animation = None;
            self.spring = None;
        }
        match sent {
            _ if !moved => {}
            Some(now) => {
                let last = self.glide.as_ref().map(|glide| (now - glide.event, glide));
                let (from, pace) = match last {
                    Some((gap, glide)) if gap < Duration::from_millis(100) => {
                        let gap = gap.clamp(PACE / 2, PACE * 6 / 5);
                        (
                            if resting { now } else { glide.from },
                            // Slower events count at once, or the scroll would run ahead and then stop.
                            gap.max((glide.pace + gap) / 2),
                        )
                    }
                    _ => (now, PACE),
                };
                self.glide = Some(Glide {
                    to: aim,
                    from,
                    until: now + pace,
                    event: now,
                    pace,
                });
            }
            None => (self.offset, self.glide) = (aim, None),
        }
        (moved, point(x, y))
    }

    fn animate_to(&mut self, offset: Point<Pixels>, wheel: bool) {
        let now = Instant::now();
        let running = self
            .animation
            .as_ref()
            .filter(|animation| now < animation.start + animation.duration);
        let (from, speed) = running.map_or((self.offset, Point::default()), |animation| {
            animation.at(now)
        });
        let distance = f32::from((offset.x - from.x).abs().max((offset.y - from.y).abs()));
        let duration = if wheel {
            running.map_or(WHEEL, |animation| {
                ((now - animation.start) * 2).clamp(Duration::from_millis(50), WHEEL)
            })
        } else {
            Duration::from_secs_f32((distance.sqrt() / 60.).clamp(1. / 60., 0.7))
        };
        let seconds = duration.as_secs_f32();
        let slope = |delta: Pixels, speed: f32| match running {
            None if wheel => 3.,
            _ if delta.is_zero() => 0.,
            _ => (speed * seconds / f32::from(delta)).clamp(if wheel { 1.5 } else { 0. }, 3.),
        };
        self.animation = (distance > 0.).then(|| Animation {
            from,
            to: offset,
            slope: point(
                slope(offset.x - from.x, speed.x),
                slope(offset.y - from.y, speed.y),
            ),
            start: now,
            duration,
        });
        self.offset = from;
        self.glide = None;
    }

    // Where the box is when the running scroll ends.
    fn end(&self) -> Point<Pixels> {
        match (&self.animation, &self.glide) {
            (Some(animation), _) => animation.to,
            (None, Some(glide)) => glide.to,
            (None, None) => self.offset,
        }
    }

    fn busy(&self) -> bool {
        self.grabbed
            || self.settling
            || self.animation.is_some()
            || self.stretch != Point::default()
            || (self.glide.as_ref()).is_some_and(|glide| glide.to != self.offset)
    }

    fn snap(&mut self) {
        if let Some(snaps) = &self.snaps {
            let (to, ids) = snaps.find(self.offset, None, self.range);
            self.snapped = ids;
            if to != self.offset {
                self.animate_to(to, false);
            }
        }
    }

    fn advance(&mut self, now: Instant, idle: bool) -> bool {
        self.driven = false;
        let mut moved = false;
        if let Some(animation) = &self.animation {
            let offset = if now < animation.start + animation.duration {
                animation.at(now).0
            } else {
                let to = animation.to;
                self.animation = None;
                to
            };
            // The snap events wait for a frame at rest.
            moved = offset != self.offset || self.animation.is_none() && self.snaps.is_some();
            self.offset = offset;
        }
        if let Some(glide) = &mut self.glide
            && glide.to != self.offset
        {
            let span = (glide.until - glide.from).as_secs_f32();
            let done = now.saturating_duration_since(glide.from).as_secs_f32();
            let (from, to) = (self.offset, glide.to);
            self.offset = match done < span {
                true => point(
                    from.x + (to.x - from.x) * (done / span),
                    from.y + (to.y - from.y) * (done / span),
                ),
                false => to,
            };
            glide.from = glide.from.max(now);
            moved |= self.offset != from;
        }
        let resting = self.animation.is_none()
            && self
                .glide
                .as_ref()
                .is_none_or(|glide| glide.to == self.offset);
        if self.settling && resting && idle && self.stretch == Point::default() {
            self.settling = false;
            self.snap();
            moved = true;
        }
        if self.loose && idle {
            self.loose = false;
            self.spring = (self.stretch != Point::default()).then_some(Spring {
                from: self.stretch,
                speed: Point::default(),
                start: now,
            });
        }
        if let Some(spring) = &self.spring {
            let stretch = spring.at(now);
            if stretch.is_none() {
                self.spring = None;
            }
            self.stretch = stretch.unwrap_or_default();
            moved = true;
        }
        moved
    }
}

fn within([low, high]: [Point<Pixels>; 2], at: Point<Pixels>) -> Point<Pixels> {
    point(at.x.clamp(low.x, high.x), at.y.clamp(low.y, high.y))
}

// Where layout places scrolled content: the offset with its stretch, snapped to device pixels.
fn snap_offset(at: Point<Pixels>, scale: f32) -> Point<Pixels> {
    at.map(|value| px((value.0 * scale).round() / scale))
}

#[derive(Clone, Default)]
pub struct ScrollRef(Rc<RefCell<ScrollState>>);

impl std::fmt::Debug for ScrollRef {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_tuple("ScrollRef").field(&self.offset()).finish()
    }
}

impl ScrollRef {
    pub fn offset(&self) -> Point<Pixels> {
        self.0.borrow().offset
    }

    pub fn stretch(&self) -> Point<Pixels> {
        self.0.borrow().stretch
    }

    // The place that layout gives the content for `offset`, before a layout with it.
    pub fn painted_at(&self, offset: Point<Pixels>, scale: f32) -> Point<Pixels> {
        snap_offset(offset + self.stretch(), scale)
    }

    pub fn painted(&self) -> Point<Pixels> {
        self.0.borrow().painted
    }

    /// The lowest and the highest offset.
    pub fn range(&self) -> [Point<Pixels>; 2] {
        self.0.borrow().range
    }

    /// How far the content can scroll on each axis.
    pub fn extent(&self) -> Point<Pixels> {
        let [low, high] = self.range();
        high - low
    }

    pub fn overflow(&self) -> Point<Pixels> {
        self.0.borrow().overflow
    }

    pub fn clamped(&self, at: Point<Pixels>) -> Point<Pixels> {
        within(self.range(), at)
    }

    pub fn bounds(&self) -> Rect {
        self.0.borrow().bounds
    }

    pub fn target(&self) -> Point<Pixels> {
        let scroll = self.0.borrow();
        scroll
            .animation
            .as_ref()
            .map_or(scroll.offset, |animation| animation.to)
    }

    pub fn set_offset(&self, offset: Point<Pixels>) {
        let scroll = &mut *self.0.borrow_mut();
        (scroll.offset, scroll.stretch) = (offset, Point::default());
        (scroll.animation, scroll.spring, scroll.loose) = (None, None, false);
        scroll.glide = None;
    }

    pub fn animate_to(&self, offset: Point<Pixels>, wheel: bool) {
        self.0.borrow_mut().animate_to(offset, wheel);
    }

    /// Where a scroll to `to` ends. A scroll from `from` snaps in its direction.
    pub fn snap(&self, to: Point<Pixels>, from: Option<Point<Pixels>>) -> Point<Pixels> {
        let scroll = &mut *self.0.borrow_mut();
        let Some(snaps) = &scroll.snaps else {
            return to;
        };
        let (mut found, mut ids) = snaps.find(to, from, scroll.range);
        // Like Chrome, an axis that the scroll doesn't move keeps its area.
        let now = scroll.end();
        if to.x == now.x && scroll.snapped[0].is_some() {
            (found.x, ids[0]) = (to.x, scroll.snapped[0]);
        }
        if to.y == now.y && scroll.snapped[1].is_some() {
            (found.y, ids[1]) = (to.y, scroll.snapped[1]);
        }
        (scroll.snapped, scroll.grabbed, scroll.settling) = (ids, false, false);
        scroll.scrolled = true;
        found
    }

    pub fn snaps(&self) -> bool {
        self.0.borrow().snaps.is_some()
    }

    /// Takes the snap areas of a new layout. True when the offset moved to one.
    pub fn set_snaps(&self, snaps: Option<ScrollSnaps>) -> bool {
        let scroll = &mut *self.0.borrow_mut();
        if scroll.snaps.as_deref() == snaps.as_ref() {
            return false;
        }
        scroll.snaps = snaps.map(Rc::new);
        let Some(snaps) = &scroll.snaps else {
            (scroll.snapped, scroll.settling) = ([None; 2], false);
            return false;
        };
        if scroll.busy() {
            scroll.settling |= !scroll.grabbed;
            return false;
        }
        let (to, ids) = snaps.follow(scroll.offset, scroll.snapped, scroll.range);
        scroll.snapped = ids;
        let moved = to != scroll.offset;
        scroll.offset = to;
        scroll.glide = None;
        moved
    }

    /// A box without a layout snaps to nothing, which is no event.
    pub fn forget_snaps(&self) {
        let scroll = &mut *self.0.borrow_mut();
        (scroll.snaps, scroll.settling, scroll.scrolled) = (None, false, false);
        (scroll.snapped, scroll.reported, scroll.aimed) = Default::default();
    }

    /// Whether the box still scrolls to a snap area, or has the event for it to fire.
    pub fn snapping(&self) -> bool {
        let scroll = self.0.borrow();
        scroll.snaps.is_some() && (scroll.busy() || scroll.snapped != scroll.reported)
    }

    /// The areas, on the x and y axes, of the `scrollsnapchanging` and the `scrollsnapchange` event to fire.
    pub fn snap_events(&self) -> [Option<[Option<u64>; 2]>; 2] {
        let scroll = &mut *self.0.borrow_mut();
        if scroll.snaps.is_none() && scroll.reported == [None; 2] {
            return [None; 2];
        }
        let aim = match &scroll.snaps {
            Some(snaps) if scroll.grabbed || scroll.settling => {
                snaps.find(scroll.end(), None, scroll.range).1
            }
            _ => scroll.snapped,
        };
        let changing = scroll.scrolled && aim != scroll.aimed;
        if changing {
            scroll.aimed = aim;
        }
        let rests = !scroll.busy();
        let change = rests && scroll.snapped != scroll.reported;
        if rests {
            (scroll.reported, scroll.aimed) = (scroll.snapped, scroll.snapped);
            scroll.scrolled = false;
        }
        [changing.then_some(aim), change.then_some(scroll.snapped)]
    }

    pub fn grab(&self) {
        let scroll = &mut *self.0.borrow_mut();
        scroll.scrolled = true;
        (scroll.grabbed, scroll.settling) = (true, false);
        scroll.snapped = [None; 2];
    }

    /// The user let the box go, so it snaps when it comes to rest.
    pub fn settle(&self, window: &mut Window) {
        {
            let scroll = &mut *self.0.borrow_mut();
            scroll.grabbed = false;
            scroll.settling = scroll.snaps.is_some();
            scroll.scrolled = true;
        }
        self.drive(window);
    }

    // The first event of the momentum tells where it ends. True when the box goes to a snap area there instead.
    fn fling(&self, speed: Point<f32>, moved: Point<Pixels>) -> bool {
        // The time that macOS momentum takes to lose most of its speed.
        const DECAY: f32 = 0.325;
        let scroll = &mut *self.0.borrow_mut();
        let Some(snaps) = scroll.snaps.clone() else {
            return false;
        };
        if scroll.stretch != Point::default() || moved == Point::default() {
            return false;
        }
        let speed = point(
            if moved.x.is_zero() { 0. } else { speed.x },
            if moved.y.is_zero() { 0. } else { speed.y },
        );
        let from = scroll.offset;
        let end = from + speed.map(|value| px(value * DECAY));
        let (to, ids) = snaps.find(end, Some(from), scroll.range);
        let axis = usize::from(moved.x.is_zero());
        if ids[axis].is_none() || to == from {
            return false;
        }
        scroll.animate_to(to, false);
        if let Some(animation) = &mut scroll.animation {
            let distance = (to.x - from.x).abs().max((to.y - from.y).abs()).0;
            let seconds = (distance.sqrt() / 40.).clamp(0.15, 0.6);
            let slope = |delta: Pixels, speed: f32| match delta.is_zero() {
                true => 0.,
                false => (speed * seconds / delta.0).clamp(0., 3.),
            };
            animation.duration = Duration::from_secs_f32(seconds);
            animation.slope = point(slope(to.x - from.x, speed.x), slope(to.y - from.y, speed.y));
        }
        (scroll.snapped, scroll.grabbed, scroll.settling) = (ids, false, false);
        true
    }

    pub(crate) fn anchor(&self) -> Option<Anchor> {
        self.0.borrow().anchor
    }

    pub(crate) fn set_anchor(&self, anchor: Option<Anchor>) {
        self.0.borrow_mut().anchor = anchor;
    }

    // Moves the offset for the anchor. A smooth scroll keeps its target, as in Chrome, and a trackpad scroll moves
    // on from the new offset.
    pub(crate) fn anchor_by(&self, by: Pixels) {
        let scroll = &mut *self.0.borrow_mut();
        scroll.offset.y += by;
        if let Some(glide) = &mut scroll.glide {
            glide.to.y += by;
        }
        if let Some(anchor) = &mut scroll.anchor {
            anchor.offset = scroll.offset;
        }
    }

    /// Sets the range of a handle that no box tracks, like the scrollback of a terminal.
    pub fn set_range(&self, bounds: Rect, max_offset: Point<Pixels>) {
        let scroll = &mut *self.0.borrow_mut();
        (scroll.bounds, scroll.range) = (bounds, [-max_offset, Point::default()]);
        scroll.overflow = max_offset;
    }

    fn pan(&self, delta: Point<Pixels>, sent: Option<Instant>) -> (bool, Point<Pixels>) {
        self.0.borrow_mut().pan(delta, sent)
    }

    pub(crate) fn stretch_by(&self, rest: Point<Pixels>) {
        let scroll = &mut *self.0.borrow_mut();
        scroll.stretch += rest.map(|value| value / STIFFNESS);
        scroll.spring = None;
    }

    pub(crate) fn loosen(&self, window: &mut Window) {
        let mut scroll = self.0.borrow_mut();
        scroll.loose = scroll.stretch != Point::default();
        drop(scroll);
        self.drive(window);
    }

    fn hold(&self) -> bool {
        let scroll = &mut *self.0.borrow_mut();
        scroll.spring = None;
        scroll.stretch != Point::default()
    }

    pub(crate) fn release(&self, speed: Point<f32>, window: &mut Window) -> bool {
        let mut scroll = self.0.borrow_mut();
        let stretch = scroll.stretch;
        if stretch == Point::default() || scroll.spring.is_some() {
            return stretch != Point::default();
        }
        let outward = |stretch: Pixels, speed: f32| match f32::from(stretch) * speed > 0. {
            true => speed,
            false => 0.,
        };
        scroll.loose = false;
        scroll.spring = Some(Spring {
            from: stretch,
            speed: point(outward(stretch.x, speed.x), outward(stretch.y, speed.y)),
            start: Instant::now(),
        });
        drop(scroll);
        self.drive(window);
        true
    }

    pub(crate) fn drive(&self, window: &mut Window) {
        {
            let scroll = &mut *self.0.borrow_mut();
            let gliding = (scroll.glide.as_ref()).is_some_and(|glide| glide.to != scroll.offset);
            let busy = scroll.animation.is_some()
                || scroll.spring.is_some()
                || scroll.loose
                || scroll.settling
                || gliding;
            if scroll.driven || !busy {
                return;
            }
            scroll.driven = true;
        }
        let handle = self.clone();
        window.next_frame(move |window, _| {
            let now = window.frame_at.unwrap_or_else(Instant::now);
            let idle = now.saturating_duration_since(window.rails.1) > Duration::from_millis(100);
            if handle.0.borrow_mut().advance(now, idle) {
                window.refresh();
            }
            handle.drive(window);
        });
    }

    pub(crate) fn clamp(
        &self,
        style: &Style,
        bounds: Rect,
        client: Size<Pixels>,
        content: Size<Pixels>,
        scale: f32,
    ) -> Point<Pixels> {
        // Padding was truncated to layout units, so it is truncated the same way here.
        let unit = |value: Pixels| px(crate::geometry::to_layout_unit(value.0, scale) / scale);
        let side = |length: Length| match length {
            Length::Px(value) => unit(value),
            Length::Fraction(fraction) => bounds.size.width * fraction,
            Length::Calc(fraction, offset) => bounds.size.width * fraction + unit(offset),
            Length::Bound(_) | Length::Auto => Pixels::ZERO,
        };
        let padding = Size::new(
            side(style.padding.left) + side(style.padding.right),
            side(style.padding.top) + side(style.padding.bottom),
        );
        // Like Chrome, the range is the scroll size less the client size, both from the layout before it snaps to
        // device pixels, and each rounded to them. Taffy and these sums differ after a few decimals, so they first
        // round to two places.
        let exact = |value: Pixels| (value.0 * scale * 100.).round() / 100.;
        let round = |value: Pixels| px(exact(value).round() / scale);
        let max =
            Point::from((content + padding).map(round) - client.map(round)).max(Point::default());
        // A box that scrolls from its end starts at the overflow on that side, which Chrome floors to device pixels.
        let start = |reverse: bool, content: Pixels, client: Pixels, max: Pixels| match reverse {
            true => px((exact(content) - exact(client)).floor() / scale).clamp(px(0.), max),
            false => px(0.),
        };
        let over = |content: Pixels, client: Pixels| {
            px(((exact(content) - exact(client)) / scale).max(0.))
        };
        let overflow = point(
            over(content.width + padding.width, client.width),
            over(content.height + padding.height, client.height),
        );
        let [x, y] = style.scrolls_from_end();
        let high = point(
            start(x, content.width + padding.width, client.width, max.x),
            start(y, content.height + padding.height, client.height, max.y),
        );
        let range = [high - max, high];
        let scroll = &mut *self.0.borrow_mut();
        let offset = within(range, scroll.offset);
        if let Some(anchor) = &mut scroll.anchor
            && anchor.offset == scroll.offset
        {
            anchor.offset = offset;
        }
        scroll.offset = offset;
        if let Some(glide) = &mut scroll.glide {
            glide.to = within(range, glide.to);
        }
        (scroll.range, scroll.overflow) = (range, overflow);
        scroll.bounds = bounds;
        scroll.painted = snap_offset(scroll.offset + scroll.stretch, scale);
        scroll.painted
    }
}

#[cfg(test)]
mod tests {
    use super::{Animation, ScrollRef, Spring};
    use crate::{Pixels, Point, point, px};
    use std::time::{Duration, Instant};

    #[test]
    fn stretch_follows_the_pan_and_gives_way_to_scrolling() {
        let delta = |y: f32| point(px(0.), px(y));
        let handle = ScrollRef::default();
        handle.0.borrow_mut().range = [delta(-100.), delta(0.)];

        let pan = |by: Point<Pixels>| handle.pan(by, None);
        assert_eq!(pan(delta(-30.)), (true, delta(0.)));
        assert_eq!(pan(delta(50.)), (true, delta(20.)));
        assert_eq!(handle.offset().y, px(0.));
        assert_eq!(pan(delta(10.)), (false, delta(10.)));

        handle.stretch_by(delta(40.));
        assert_eq!(handle.stretch().y, px(2.));
        assert_eq!(pan(delta(20.)), (true, delta(0.)));
        assert_eq!(handle.stretch().y, px(3.));
        assert_eq!(pan(delta(-80.)), (true, delta(0.)));
        assert_eq!(handle.stretch().y, px(0.));
        assert_eq!(handle.offset().y, px(-20.));

        assert_eq!(pan(delta(-100.)), (true, delta(-20.)));
        handle.stretch_by(delta(-20.));
        assert_eq!(handle.stretch().y, px(-1.));
        handle.set_offset(delta(-50.));
        assert_eq!(handle.stretch(), Default::default());
    }

    #[test]
    fn pans_move_evenly_between_events() {
        let delta = |y: f32| point(px(0.), px(y));
        let handle = ScrollRef::default();
        handle.0.borrow_mut().range = [delta(-1000.), delta(0.)];
        let (start, frame) = (Instant::now(), Duration::from_micros(8_333));
        let mut last = 0.;
        let mut steps = Vec::new();
        // Events at 60 Hz, frames at 120 Hz, 3 ms after the events.
        for tick in 0..24 {
            let now = start + frame * tick;
            if tick % 2 == 0 && tick < 16 {
                handle.pan(delta(-10.), Some(now));
                // An event that moves nothing, like the end of a gesture, leaves the rest to come.
                handle.pan(delta(0.), Some(now));
            }
            handle
                .0
                .borrow_mut()
                .advance(now + Duration::from_millis(3), false);
            let y = f32::from(handle.offset().y);
            steps.push(last - y);
            last = y;
        }
        assert!(
            steps[4..16].iter().all(|step| (step - 5.).abs() < 0.01),
            "{steps:?}"
        );
        assert_eq!(handle.offset().y, px(-80.));

        handle.pan(delta(-10.), Some(start + frame * 30));
        handle.set_offset(delta(-50.));
        assert!(!handle.0.borrow_mut().advance(start + frame * 40, false));
        assert_eq!(handle.offset().y, px(-50.));
    }

    #[test]
    fn springs_go_further_with_speed_and_end_at_the_edge() {
        let start = Instant::now();
        let spring = |from: f32, speed: f32| Spring {
            from: point(px(0.), px(from)),
            speed: point(0., speed),
            start,
        };
        let at = |spring: &Spring, ms: u64| {
            spring
                .at(start + Duration::from_millis(ms))
                .map(|at| f32::from(at.y))
        };
        assert!((at(&spring(100., 0.), 50).unwrap() - 53.53).abs() < 0.01);
        assert!((at(&spring(0., 4000.), 100).unwrap() - 35.53).abs() < 0.01);
        assert_eq!(at(&spring(0., 4000.), 0), Some(0.));
        assert_eq!(at(&spring(100., 0.), 600), None);
        assert_eq!(at(&spring(0., 4000.), 800), None);
    }

    #[test]
    fn smooth_scrolls_keep_position_and_speed() {
        let (start, duration) = (Instant::now(), Duration::from_millis(200));
        let curve = |slope: f32| Animation {
            from: point(px(0.), px(0.)),
            to: point(px(0.), px(-100.)),
            slope: point(0., slope),
            start,
            duration,
        };
        for slope in [0., 1.5, 3.] {
            let animation = curve(slope);
            assert_eq!(animation.at(start).0.y, px(0.));
            assert!((animation.at(start).1.y + slope * 500.).abs() < 1e-3);
            assert!((f32::from(animation.at(start + duration).0.y) + 100.).abs() < 1e-3);
            assert_eq!(animation.at(start + duration).1.y, 0.);
            let mut last = 0.;
            for step in 1..=20 {
                let y = f32::from(animation.at(start + duration * step / 20).0.y);
                assert!(y <= last, "slope {slope} moves back at step {step}");
                last = y;
            }
        }

        let handle = ScrollRef::default();
        handle.animate_to(point(px(0.), px(-100.)), true);
        assert_eq!(handle.target().y, px(-100.));
        assert_eq!(handle.0.borrow().animation.as_ref().unwrap().slope.y, 3.);
        handle.set_offset(point(px(0.), px(-10.)));
        assert!(handle.0.borrow().animation.is_none());

        let past = Instant::now() - Duration::from_millis(50);
        handle.0.borrow_mut().animation = Some(Animation {
            start: past,
            ..curve(0.)
        });
        handle.animate_to(point(px(0.), px(-400.)), false);
        let state = handle.0.borrow();
        let new = state.animation.as_ref().unwrap();
        let old = Animation {
            start: past,
            ..curve(0.)
        }
        .at(new.start);
        assert_eq!(state.offset, new.from);
        assert!((f32::from(new.from.y - old.0.y)).abs() < 1e-3);
        assert!((new.at(new.start).1.y - old.1.y).abs() < 1e-2);
    }
}
