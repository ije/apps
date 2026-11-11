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
    // Returns `[x, y, width, height]`.
    pub fn cover(&self) -> [i32; 4] {
        let [x, y] = [self.left().floor(), self.top().floor()];
        [
            x as i32,
            y as i32,
            (self.right().ceil() - x) as i32,
            (self.bottom().ceil() - y) as i32,
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
}

impl<T: Copy> From<Corners<T>> for Radii<T> {
    fn from(x: Corners<T>) -> Self {
        Self { x, y: x }
    }
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
        }
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
        }
    }

    pub fn inset(self, top: Pixels, right: Pixels, bottom: Pixels, left: Pixels) -> Self {
        let less = |radius: Pixels, by: Pixels| (radius - by).max(Pixels::ZERO);
        Self {
            x: Corners {
                top_left: less(self.x.top_left, left),
                top_right: less(self.x.top_right, right),
                bottom_right: less(self.x.bottom_right, right),
                bottom_left: less(self.x.bottom_left, left),
            },
            y: Corners {
                top_left: less(self.y.top_left, top),
                top_right: less(self.y.top_right, top),
                bottom_right: less(self.y.bottom_right, bottom),
                bottom_left: less(self.y.bottom_left, bottom),
            },
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
