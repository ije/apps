use std::{
    fmt,
    hash::{Hash, Hasher},
    iter::Sum,
    ops::{Add, AddAssign, Div, Mul, MulAssign, Neg, Sub, SubAssign},
};

// A CSS pixel. Device space uses plain `f32` and `i32`.
#[derive(Clone, Copy, Default, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Pixels(pub f32);

pub const fn px(value: f32) -> Pixels {
    Pixels(value)
}

macro_rules! forward {
    ($($name:ident)*) => {
        $(pub fn $name(self) -> Self {
            Self(self.0.$name())
        })*
    };
}

impl Pixels {
    pub const ZERO: Self = Self(0.);
    pub const MAX: Self = Self(f32::MAX);

    forward!(floor ceil round abs);

    pub fn max(self, other: Self) -> Self {
        Self(self.0.max(other.0))
    }
    pub fn min(self, other: Self) -> Self {
        Self(self.0.min(other.0))
    }
    pub fn clamp(self, low: Self, high: Self) -> Self {
        self.max(low).min(high)
    }
    pub fn signum(self) -> f32 {
        self.0.signum()
    }
    pub fn is_zero(self) -> bool {
        self.0 == 0.
    }
    pub fn scale(self, factor: f32) -> f32 {
        self.0 * factor
    }
}

impl Eq for Pixels {}
#[allow(clippy::derive_ord_xor_partial_ord)]
impl Ord for Pixels {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}
impl Hash for Pixels {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u32(self.0.to_bits());
    }
}
impl fmt::Debug for Pixels {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}
macro_rules! pixel_ops {
    ($($op:ident $name:ident($rhs:ty) -> $out:ty = |$a:ident, $b:ident| $value:expr;)*) => {
        $(impl $op<$rhs> for Pixels {
            type Output = $out;
            fn $name(self, $b: $rhs) -> $out {
                let $a = self;
                $value
            }
        })*
    };
}

pixel_ops! {
    Add add(Pixels) -> Pixels = |a, b| Pixels(a.0 + b.0);
    Sub sub(Pixels) -> Pixels = |a, b| Pixels(a.0 - b.0);
    Mul mul(f32) -> Pixels = |a, b| Pixels(a.0 * b);
    Mul mul(usize) -> Pixels = |a, b| Pixels(a.0 * b as f32);
    Div div(f32) -> Pixels = |a, b| Pixels(a.0 / b);
    Div div(Pixels) -> f32 = |a, b| a.0 / b.0;
}

impl Mul<Pixels> for f32 {
    type Output = Pixels;
    fn mul(self, value: Pixels) -> Pixels {
        value * self
    }
}
impl Mul<Pixels> for usize {
    type Output = Pixels;
    fn mul(self, value: Pixels) -> Pixels {
        value * self
    }
}
impl Neg for Pixels {
    type Output = Self;
    fn neg(self) -> Self {
        Self(-self.0)
    }
}
impl AddAssign for Pixels {
    fn add_assign(&mut self, other: Self) {
        self.0 += other.0;
    }
}
impl SubAssign for Pixels {
    fn sub_assign(&mut self, other: Self) {
        self.0 -= other.0;
    }
}
impl MulAssign<f32> for Pixels {
    fn mul_assign(&mut self, factor: f32) {
        self.0 *= factor;
    }
}
impl Sum for Pixels {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        Self(iter.map(|value| value.0).sum())
    }
}
impl From<f32> for Pixels {
    fn from(value: f32) -> Self {
        Self(value)
    }
}
impl From<f64> for Pixels {
    fn from(value: f64) -> Self {
        Self(value as f32)
    }
}
impl From<Pixels> for f32 {
    fn from(value: Pixels) -> Self {
        value.0
    }
}
impl From<Pixels> for f64 {
    fn from(value: Pixels) -> Self {
        value.0 as f64
    }
}

pub trait Scalar: Copy + Default + PartialOrd + Add<Output = Self> + Sub<Output = Self> {
    fn half(self) -> Self;
}
impl Scalar for Pixels {
    fn half(self) -> Self {
        Self(self.0 / 2.)
    }
}
impl Scalar for f32 {
    fn half(self) -> Self {
        self / 2.
    }
}
impl Scalar for i32 {
    fn half(self) -> Self {
        self / 2
    }
}

fn lower<T: PartialOrd>(a: T, b: T) -> T {
    if b < a { b } else { a }
}

fn upper<T: PartialOrd>(a: T, b: T) -> T {
    if b > a { b } else { a }
}

macro_rules! pair {
    ($name:ident, $make:ident, $a:ident, $b:ident) => {
        #[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash)]
        #[repr(C)]
        pub struct $name<T = Pixels> {
            pub $a: T,
            pub $b: T,
        }

        pub const fn $make<T>($a: T, $b: T) -> $name<T> {
            $name { $a, $b }
        }

        impl<T> $name<T> {
            pub const fn new($a: T, $b: T) -> Self {
                Self { $a, $b }
            }
            pub fn map<U>(self, f: impl Fn(T) -> U) -> $name<U> {
                $make(f(self.$a), f(self.$b))
            }
        }

        impl $name {
            pub fn scale(self, factor: f32) -> $name<f32> {
                self.map(|value| value.0 * factor)
            }
        }

        pair!($name, $make, $a, $b, Add add(Self), Sub sub(Self));
        pair!($name, $make, $a, $b, Mul mul(f32), Div div(f32));
    };
    ($name:ident, $make:ident, $a:ident, $b:ident, $($op:ident $method:ident(Self)),*) => {
        $(impl<T: $op<Output = T>> $op for $name<T> {
            type Output = Self;
            fn $method(self, other: Self) -> Self {
                $make(self.$a.$method(other.$a), self.$b.$method(other.$b))
            }
        })*
    };
    ($name:ident, $make:ident, $a:ident, $b:ident, $($op:ident $method:ident(f32)),*) => {
        $(impl<T: $op<f32, Output = T>> $op<f32> for $name<T> {
            type Output = Self;
            fn $method(self, factor: f32) -> Self {
                $make(self.$a.$method(factor), self.$b.$method(factor))
            }
        })*
    };
}

pair!(Point, point, x, y);
pair!(Size, size, width, height);

impl<T: PartialOrd> Point<T> {
    pub fn max(self, other: Self) -> Self {
        point(upper(self.x, other.x), upper(self.y, other.y))
    }
    pub fn min(self, other: Self) -> Self {
        point(lower(self.x, other.x), lower(self.y, other.y))
    }
}

impl<T: Add<Output = T> + Copy> AddAssign for Point<T> {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}
impl<T: Sub<Output = T> + Copy> SubAssign for Point<T> {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}
impl<T: Neg<Output = T>> Neg for Point<T> {
    type Output = Self;
    fn neg(self) -> Self {
        point(-self.x, -self.y)
    }
}

impl Size {
    pub fn to_device(self, factor: f32) -> Size<i32> {
        self.map(|value| (value.0 * factor).round() as i32)
    }
}

impl<T> From<Point<T>> for Size<T> {
    fn from(point: Point<T>) -> Self {
        size(point.x, point.y)
    }
}
impl<T> From<Size<T>> for Point<T> {
    fn from(size: Size<T>) -> Self {
        point(size.width, size.height)
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct Rect<T = Pixels> {
    pub origin: Point<T>,
    pub size: Size<T>,
}

pub const fn rect<T>(x: T, y: T, width: T, height: T) -> Rect<T> {
    Rect {
        origin: point(x, y),
        size: size(width, height),
    }
}

impl<T> Rect<T> {
    pub const fn new(origin: Point<T>, size: Size<T>) -> Self {
        Self { origin, size }
    }
    pub fn map<U>(self, f: impl Fn(T) -> U) -> Rect<U> {
        Rect::new(self.origin.map(&f), self.size.map(&f))
    }
}

impl<T: Scalar> Rect<T> {
    pub fn between(min: Point<T>, max: Point<T>) -> Self {
        rect(min.x, min.y, max.x - min.x, max.y - min.y)
    }
    pub fn left(&self) -> T {
        self.origin.x
    }
    pub fn top(&self) -> T {
        self.origin.y
    }
    pub fn right(&self) -> T {
        self.origin.x + self.size.width
    }
    pub fn bottom(&self) -> T {
        self.origin.y + self.size.height
    }
    pub fn top_right(&self) -> Point<T> {
        point(self.right(), self.top())
    }
    pub fn bottom_left(&self) -> Point<T> {
        point(self.left(), self.bottom())
    }
    pub fn bottom_right(&self) -> Point<T> {
        point(self.right(), self.bottom())
    }
    pub fn center(&self) -> Point<T> {
        point(
            self.left() + self.size.width.half(),
            self.top() + self.size.height.half(),
        )
    }
    pub fn is_empty(&self) -> bool {
        let zero = T::default();
        self.size.width <= zero || self.size.height <= zero
    }
    // The right and bottom sides are outside.
    pub fn contains(&self, point: Point<T>) -> bool {
        self.left() <= point.x
            && point.x < self.right()
            && self.top() <= point.y
            && point.y < self.bottom()
    }
    pub fn intersects(&self, other: Self) -> bool {
        self.left() < other.right()
            && other.left() < self.right()
            && self.top() < other.bottom()
            && other.top() < self.bottom()
    }
    // Boxes that don't meet give an empty box, never a negative size.
    pub fn intersect(&self, other: Self) -> Self {
        let min = self.origin.max(other.origin);
        Self::between(min, self.bottom_right().min(other.bottom_right()).max(min))
    }
    pub fn union(&self, other: Self) -> Self {
        Self::between(
            self.origin.min(other.origin),
            self.bottom_right().max(other.bottom_right()),
        )
    }
    pub fn outset(&self, by: T) -> Self {
        Self::between(
            point(self.left() - by, self.top() - by),
            point(self.right() + by, self.bottom() + by),
        )
    }
    pub fn inset(&self, by: T) -> Self {
        Self::between(
            point(self.left() + by, self.top() + by),
            point(self.right() - by, self.bottom() - by),
        )
    }
}

impl Rect {
    pub fn scale(&self, factor: f32) -> Rect<f32> {
        self.map(|value| value.0 * factor)
    }
    pub fn to_device(&self, factor: f32) -> Rect<i32> {
        self.map(|value| (value.0 * factor).round() as i32)
    }
}

impl Rect<f32> {
    pub fn to_array(&self) -> [f32; 4] {
        [
            self.origin.x,
            self.origin.y,
            self.size.width,
            self.size.height,
        ]
    }
}

impl<T: Add<Output = T>> Add<Point<T>> for Rect<T> {
    type Output = Self;
    fn add(self, offset: Point<T>) -> Self {
        Self::new(self.origin + offset, self.size)
    }
}
impl<T: Sub<Output = T>> Sub<Point<T>> for Rect<T> {
    type Output = Self;
    fn sub(self, offset: Point<T>) -> Self {
        Self::new(self.origin - offset, self.size)
    }
}
impl<T: Mul<f32, Output = T>> Mul<f32> for Rect<T> {
    type Output = Self;
    fn mul(self, factor: f32) -> Self {
        Self::new(self.origin * factor, self.size * factor)
    }
}
impl<T: Div<f32, Output = T>> Div<f32> for Rect<T> {
    type Output = Self;
    fn div(self, divisor: f32) -> Self {
        Self::new(self.origin / divisor, self.size / divisor)
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct Edges<T = Pixels> {
    pub top: T,
    pub right: T,
    pub bottom: T,
    pub left: T,
}

impl<T: Copy> Edges<T> {
    pub const fn all(value: T) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }
    pub fn map<U>(&self, f: impl Fn(T) -> U) -> Edges<U> {
        Edges {
            top: f(self.top),
            right: f(self.right),
            bottom: f(self.bottom),
            left: f(self.left),
        }
    }
    pub fn any(&self, f: impl Fn(T) -> bool) -> bool {
        f(self.top) || f(self.right) || f(self.bottom) || f(self.left)
    }
}

impl From<Pixels> for Edges {
    fn from(value: Pixels) -> Self {
        Self::all(value)
    }
}

// Clockwise from the top left, like CSS.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct Corners<T = Pixels> {
    pub top_left: T,
    pub top_right: T,
    pub bottom_right: T,
    pub bottom_left: T,
}

impl<T: Copy> Corners<T> {
    pub const fn all(value: T) -> Self {
        Self {
            top_left: value,
            top_right: value,
            bottom_right: value,
            bottom_left: value,
        }
    }
    pub fn map<U>(&self, f: impl Fn(T) -> U) -> Corners<U> {
        Corners {
            top_left: f(self.top_left),
            top_right: f(self.top_right),
            bottom_right: f(self.bottom_right),
            bottom_left: f(self.bottom_left),
        }
    }
    pub fn to_array(&self) -> [T; 4] {
        [
            self.top_left,
            self.top_right,
            self.bottom_right,
            self.bottom_left,
        ]
    }
}

impl<T: Copy + Default + PartialEq> Corners<T> {
    pub fn is_zero(&self) -> bool {
        *self == Self::all(T::default())
    }
}

impl Corners {
    pub fn scale(&self, factor: f32) -> Corners<f32> {
        self.map(|value| value.0 * factor)
    }
}

impl From<Pixels> for Corners {
    fn from(value: Pixels) -> Self {
        Self::all(value)
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct Radii<T = Pixels> {
    pub x: Corners<T>,
    pub y: Corners<T>,
    /// The CSS `corner-shape` of each corner, made by `corner_shape()`. 0 is round.
    pub shape: Corners<u16>,
}

impl<T: Copy> From<Corners<T>> for Radii<T> {
    fn from(x: Corners<T>) -> Self {
        Self {
            x,
            y: x,
            shape: Corners::default(),
        }
    }
}

/// The code of a corner shape with the superellipse parameter `k`: 1 is round, 2 squircle, 0 bevel, -1
/// scoop. Codes keep `k` in steps of 1/128 from -16 to 16, and 0 stands for round.
pub fn corner_shape(k: f32) -> u16 {
    if k == 1. {
        return 0;
    }
    (2048. + (k.clamp(-16., 16.) * 128.).round()).clamp(1., 4095.) as u16
}

// The superellipse parameter of a code that is not round.
fn shape_parameter(code: u16) -> f64 {
    (f64::from(code) - 2048.) / 128.
}

impl From<Pixels> for Radii {
    fn from(value: Pixels) -> Self {
        Corners::all(value).into()
    }
}

impl<T: Copy + Default + PartialEq> Radii<T> {
    pub fn is_zero(&self) -> bool {
        self.x.is_zero() && self.y.is_zero()
    }
}

impl<T> Radii<T> {
    // The shapes as the shaders read them: two corners of 12 bits in each float, exact up to 2^24.
    pub(crate) fn shape_array(&self) -> [f32; 2] {
        let s = &self.shape;
        [
            (u32::from(s.top_left) | u32::from(s.top_right) << 12) as f32,
            (u32::from(s.bottom_right) | u32::from(s.bottom_left) << 12) as f32,
        ]
    }
}

impl Radii<f32> {
    pub fn to_array(&self) -> [f32; 8] {
        let ([a, b, c, d], [e, f, g, h]) = (self.x.to_array(), self.y.to_array());
        [a, b, c, d, e, f, g, h]
    }
}

impl Radii {
    pub fn scale(&self, factor: f32) -> Radii<f32> {
        Radii {
            x: self.x.scale(factor),
            y: self.y.scale(factor),
            shape: self.shape,
        }
    }

    /// Whether `at` is inside `bounds` with these corners. Every corner shape counts as round.
    pub fn contains(&self, bounds: Rect, at: Point<Pixels>) -> bool {
        let (x, y) = (self.x.to_array(), self.y.to_array());
        let [left, top] = [bounds.origin.x.0, bounds.origin.y.0];
        let [right, bottom] = [left + bounds.size.width.0, top + bounds.size.height.0];
        let [px, py] = [at.x.0, at.y.0];
        (0..4).all(|corner| {
            let [rx, ry] = [x[corner].0, y[corner].0];
            let cx = if corner == 0 || corner == 3 {
                left + rx
            } else {
                right - rx
            };
            let cy = if corner < 2 { top + ry } else { bottom - ry };
            let [dx, dy] = [(px - cx) / rx, (py - cy) / ry];
            let outward = (corner == 0 || corner == 3) == (dx < 0.) && (corner < 2) == (dy < 0.);
            rx <= 0. || ry <= 0. || !outward || dx * dx + dy * dy <= 1.
        })
    }

    pub fn fit(self, size: Size) -> Self {
        let (mut x, mut y) = (self.x.to_array(), self.y.to_array());
        for corner in 0..4 {
            if x[corner].0 <= 0. || y[corner].0 <= 0. {
                (x[corner], y[corner]) = (Pixels::ZERO, Pixels::ZERO);
            }
        }
        let mut factor = 1f32;
        for (side, a, b) in [
            (size.width, x[0], x[1]),
            (size.width, x[3], x[2]),
            (size.height, y[0], y[3]),
            (size.height, y[1], y[2]),
        ] {
            let sum = a + b;
            if sum > side {
                factor = factor.min(side / sum);
            }
        }
        let factor = factor.max(0.);
        let corners = |[top_left, top_right, bottom_right, bottom_left]: [Pixels; 4]| Corners {
            top_left: top_left * factor,
            top_right: top_right * factor,
            bottom_right: bottom_right * factor,
            bottom_left: bottom_left * factor,
        };
        Self {
            x: corners(x),
            y: corners(y),
            shape: self.shape,
        }
    }

    // `grown` with its bevel and concave corners moved out by `by` the way `inset` moves them in, like Chromium
    // does for shadow spreads and outlines.
    pub fn moved_out(self, by: Pixels, grown: Self) -> Self {
        let shapes = self.shape.to_array();
        let angled = |at: usize| (1..=2048).contains(&shapes[at]);
        if by.is_zero() || !(0..4).any(angled) {
            return grown;
        }
        let moved = self.inset(-by, -by, -by, -by);
        let pick = |grown: Corners<Pixels>, moved: Corners<Pixels>| {
            let ([a, b, c, d], [e, f, g, h]) = (grown.to_array(), moved.to_array());
            Corners {
                top_left: if angled(0) { e } else { a },
                top_right: if angled(1) { f } else { b },
                bottom_right: if angled(2) { g } else { c },
                bottom_left: if angled(3) { h } else { d },
            }
        };
        Self {
            x: pick(grown.x, moved.x),
            y: pick(grown.y, moved.y),
            shape: self.shape,
        }
    }

    // The radii of the box inside borders of these widths. Like Chrome, a curve that meets its sides at an
    // angle (bevel and concave shapes) keeps an even thickness, so its radii shrink less or grow.
    pub fn inset(self, top: Pixels, right: Pixels, bottom: Pixels, left: Pixels) -> Self {
        let (mut x, mut y) = (self.x.to_array(), self.y.to_array());
        let shapes = self.shape.to_array();
        for corner in 0..4 {
            let (side, edge) = match corner {
                0 => (left, top),
                1 => (right, top),
                2 => (right, bottom),
                _ => (left, bottom),
            };
            let (a, b, k) = (x[corner].0, y[corner].0, shape_parameter(shapes[corner]));
            if shapes[corner] == 0 || k > 0. || a <= 0. || b <= 0. {
                x[corner] = (x[corner] - side).max(Pixels::ZERO);
                y[corner] = (y[corner] - edge).max(Pixels::ZERO);
                continue;
            }
            let (a, b, side, edge) = (
                f64::from(a),
                f64::from(b),
                f64::from(side.0),
                f64::from(edge.0),
            );
            if k == 0. {
                // A bevel moves as a line, which can cross the inner sides outside its corner box.
                let reach = a.recip().hypot(b.recip());
                x[corner] = Pixels((a * (1. + edge * reach - edge / b) - side).max(0.) as f32);
                y[corner] = Pixels((b * (1. + side * reach - side / a) - edge).max(0.) as f32);
                continue;
            }
            // A concave curve is (X/a)^n + (Y/b)^n = 1 around the outer point, with `X` along the horizontal
            // side and `Y` along the vertical one. Its point at angle `t` moves in along its normal by `by`, and
            // bisection finds where that crosses the inner side.
            let n = 2f64.powf(k.abs().min(9.97));
            let cross = |by: f64, depth: f64, vertical: bool| {
                let along = |t: f64| {
                    let (c, s) = (t.cos().max(0.).powf(2. / n), t.sin().max(0.).powf(2. / n));
                    let (nx, ny) = (c.powf(n - 1.) / a, s.powf(n - 1.) / b);
                    let length = nx.hypot(ny).max(1e-12);
                    let (px, py) = (a * c + by * nx / length, b * s + by * ny / length);
                    if vertical { (px, py) } else { (py, px) }
                };
                let (mut low, mut high) = (0., std::f64::consts::FRAC_PI_2);
                let rising = along(high).0 > along(low).0;
                for _ in 0..40 {
                    let middle = (low + high) * 0.5;
                    if (along(middle).0 < depth) == rising {
                        low = middle;
                    } else {
                        high = middle;
                    }
                }
                along((low + high) * 0.5).1
            };
            x[corner] = Pixels((cross(edge, edge, false) - side).max(0.) as f32);
            y[corner] = Pixels((cross(side, side, true) - edge).max(0.) as f32);
        }
        let corners = |[top_left, top_right, bottom_right, bottom_left]: [Pixels; 4]| Corners {
            top_left,
            top_right,
            bottom_right,
            bottom_left,
        };
        Self {
            x: corners(x),
            y: corners(y),
            shape: self.shape,
        }
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq)]
#[repr(C)]
pub struct Clip<T = Pixels> {
    pub bounds: Rect<T>,
    pub rounded: Rect<T>,
    pub radii: Radii<T>,
}

impl Clip {
    pub fn rounded(bounds: Rect, radii: impl Into<Radii>) -> Self {
        Self {
            bounds,
            rounded: bounds,
            radii: radii.into().fit(bounds.size),
        }
    }
    pub fn scale(&self, factor: f32) -> Clip<f32> {
        Clip {
            bounds: self.bounds.scale(factor),
            rounded: self.rounded.scale(factor),
            radii: self.radii.scale(factor),
        }
    }
    pub fn intersect(&self, other: &Self) -> Self {
        let round = if self.radii.is_zero() { other } else { self };
        Self {
            bounds: self.bounds.intersect(other.bounds),
            rounded: round.rounded,
            radii: round.radii,
        }
    }
}

impl<T: Default> From<Rect<T>> for Clip<T> {
    fn from(bounds: Rect<T>) -> Self {
        Self {
            bounds,
            rounded: Rect::default(),
            radii: Radii::default(),
        }
    }
}

// Snaps a device coordinate to a whole pixel. A tie goes toward zero, so a 1px line at 150% stays
// 1 device pixel.
pub(crate) fn snap(device: f32) -> f32 {
    let whole = device.trunc();
    if (device - whole).abs() > 0.5 {
        whole + device.signum()
    } else {
        whole
    }
}

pub(crate) fn round_half_up(value: f32) -> f32 {
    (value + 0.5).floor()
}

// Match Chrome's 1/64-pixel truncation. The bias prevents float error from subtracting a layout unit.
pub fn to_layout_unit(logical: f32, scale_factor: f32) -> f32 {
    let units = logical * scale_factor * 64.;
    // Keep the bias below one unit: a relative bias adds 10px to a 10,000,000px length.
    (units + (units.abs() * 1e-6).min(0.5).copysign(units)).trunc() / 64.
}

// A stroke that exists is at least one device pixel wide.
pub(crate) fn snap_width(logical: f32, scale_factor: f32) -> f32 {
    if logical == 0. {
        return 0.;
    }
    snap(logical.max(0.) * scale_factor).max(1.)
}

// Column-major 3x3 matrices of the z = 0 plane of a CSS transform.
pub(crate) type Plane = [f32; 9];

pub(crate) fn plane_of(m: &[f32; 16]) -> Plane {
    [m[0], m[1], m[3], m[4], m[5], m[7], m[12], m[13], m[15]]
}

pub(crate) fn compose(outer: &Plane, inner: &Plane) -> Plane {
    let mut out = [0.; 9];
    for (index, out) in out.iter_mut().enumerate() {
        let (v, row) = (&inner[index - index % 3..], index % 3);
        *out = outer[row] * v[0] + outer[3 + row] * v[1] + outer[6 + row] * v[2];
    }
    out
}

pub(crate) fn invert(m: &Plane) -> Option<Plane> {
    let cross = |a: &[f32], b: &[f32]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let (c0, c1, c2) = (&m[0..3], &m[3..6], &m[6..9]);
    let rows = [cross(c1, c2), cross(c2, c0), cross(c0, c1)];
    let det = c0[0] * rows[0][0] + c0[1] * rows[0][1] + c0[2] * rows[0][2];
    (det.abs() > 1e-9).then(|| std::array::from_fn(|i| rows[i % 3][i / 3] / det))
}

pub(crate) fn project(m: &Plane, at: Point<Pixels>) -> Option<Point<Pixels>> {
    let (x, y) = (at.x.0, at.y.0);
    let [u, v, w] = [0, 1, 2].map(|row| m[row] * x + m[3 + row] * y + m[6 + row]);
    (w.abs() > 1e-6).then(|| point(px(u / w), px(v / w)))
}

pub(crate) fn transform_bounds(matrix: &[f32; 16], bounds: Rect<f32>) -> Rect<f32> {
    let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
    for corner in [
        bounds.origin,
        bounds.top_right(),
        bounds.bottom_left(),
        bounds.bottom_right(),
    ] {
        let [x, y] = [corner.x, corner.y];
        let w = matrix[3] * x + matrix[7] * y + matrix[15];
        if w <= 1e-4 {
            return Rect::new(point(-1e6, -1e6), Size::new(2e6, 2e6));
        }
        let px = (matrix[0] * x + matrix[4] * y + matrix[12]) / w;
        let py = (matrix[1] * x + matrix[5] * y + matrix[13]) / w;
        min = [min[0].min(px), min[1].min(py)];
        max = [max[0].max(px), max[1].max(py)];
    }
    Rect::between(point(min[0], min[1]), point(max[0], max[1]))
}

pub(crate) const IDENTITY: [f32; 16] = [
    1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
];

pub(crate) fn region(bounds: Rect<f32>) -> [i32; 4] {
    let [x, y] = [bounds.origin.x.floor(), bounds.origin.y.floor()];
    let [right, bottom] = [
        (bounds.origin.x + bounds.size.width).ceil(),
        (bounds.origin.y + bounds.size.height).ceil(),
    ];
    [x as i32, y as i32, (right - x) as i32, (bottom - y) as i32]
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub enum AvailableSpace {
    Definite(Pixels),
    #[default]
    MinContent,
    MaxContent,
}

impl AvailableSpace {
    pub(crate) fn unscaled(space: taffy::AvailableSpace, factor: f32) -> Self {
        match space {
            taffy::AvailableSpace::Definite(value) => Self::Definite(Pixels(value / factor)),
            taffy::AvailableSpace::MinContent => Self::MinContent,
            taffy::AvailableSpace::MaxContent => Self::MaxContent,
        }
    }
}

impl From<Pixels> for AvailableSpace {
    fn from(value: Pixels) -> Self {
        Self::Definite(value)
    }
}
