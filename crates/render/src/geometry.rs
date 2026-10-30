use std::{
    fmt::{self, Debug},
    ops::{Add, AddAssign, Div, Mul, MulAssign, Neg, Sub, SubAssign},
};

/// A value that geometry types can hold.
pub trait Unit:
    Copy + Debug + Default + PartialEq + PartialOrd + Add<Output = Self> + Sub<Output = Self> + Half
{
}
impl<T> Unit for T where
    T: Copy + Debug + Default + PartialEq + PartialOrd + Add<Output = T> + Sub<Output = T> + Half
{
}

pub trait Half {
    fn half(&self) -> Self;
}

macro_rules! unit {
    ($name:ident, $inner:ty) => {
        impl Add for $name {
            type Output = Self;
            fn add(self, other: Self) -> Self {
                Self(self.0 + other.0)
            }
        }
        impl AddAssign for $name {
            fn add_assign(&mut self, other: Self) {
                self.0 += other.0;
            }
        }
        impl Sub for $name {
            type Output = Self;
            fn sub(self, other: Self) -> Self {
                Self(self.0 - other.0)
            }
        }
        impl SubAssign for $name {
            fn sub_assign(&mut self, other: Self) {
                self.0 -= other.0;
            }
        }
        impl Neg for $name {
            type Output = Self;
            fn neg(self) -> Self {
                Self(-self.0)
            }
        }
        impl std::iter::Sum for $name {
            fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
                iter.fold(Self::default(), |a, b| a + b)
            }
        }
        impl From<$inner> for $name {
            fn from(value: $inner) -> Self {
                Self(value)
            }
        }
        impl From<$name> for $inner {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

macro_rules! float_unit {
    ($name:ident) => {
        unit!($name, f32);
        impl $name {
            pub const ZERO: Self = Self(0.);
            pub fn floor(self) -> Self {
                Self(self.0.floor())
            }
            pub fn ceil(self) -> Self {
                Self(self.0.ceil())
            }
            pub fn round(self) -> Self {
                Self(self.0.round())
            }
            pub fn abs(self) -> Self {
                Self(self.0.abs())
            }
            pub fn max(self, other: Self) -> Self {
                Self(self.0.max(other.0))
            }
            pub fn min(self, other: Self) -> Self {
                Self(self.0.min(other.0))
            }
            pub fn clamp(self, low: Self, high: Self) -> Self {
                Self(self.0.max(low.0).min(high.0))
            }
            pub fn signum(self) -> f32 {
                self.0.signum()
            }
            pub fn is_zero(self) -> bool {
                self.0 == 0.
            }
        }
        impl Half for $name {
            fn half(&self) -> Self {
                Self(self.0 / 2.)
            }
        }
        impl Mul<f32> for $name {
            type Output = Self;
            fn mul(self, other: f32) -> Self {
                Self(self.0 * other)
            }
        }
        impl Mul<$name> for f32 {
            type Output = $name;
            fn mul(self, other: $name) -> $name {
                $name(self * other.0)
            }
        }
        impl Mul<$name> for usize {
            type Output = $name;
            fn mul(self, other: $name) -> $name {
                $name(self as f32 * other.0)
            }
        }
        impl Mul<usize> for $name {
            type Output = Self;
            fn mul(self, other: usize) -> Self {
                Self(self.0 * other as f32)
            }
        }
        impl MulAssign<f32> for $name {
            fn mul_assign(&mut self, other: f32) {
                self.0 *= other;
            }
        }
        impl Div for $name {
            type Output = f32;
            fn div(self, other: Self) -> f32 {
                self.0 / other.0
            }
        }
        impl Div<f32> for $name {
            type Output = Self;
            fn div(self, other: f32) -> Self {
                Self(self.0 / other)
            }
        }
        impl Eq for $name {}
        #[allow(clippy::derive_ord_xor_partial_ord)]
        impl Ord for $name {
            fn cmp(&self, other: &Self) -> std::cmp::Ordering {
                self.0.total_cmp(&other.0)
            }
        }
        impl std::hash::Hash for $name {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
                self.0.to_bits().hash(state);
            }
        }
        impl From<f64> for $name {
            fn from(value: f64) -> Self {
                Self(value as f32)
            }
        }
        impl From<$name> for f64 {
            fn from(value: $name) -> Self {
                value.0 as f64
            }
        }
        impl Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

/// Logical pixels.
#[derive(Clone, Copy, Default, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Pixels(pub f32);
float_unit!(Pixels);

impl Pixels {
    pub const MAX: Self = Self(f32::MAX);
    pub fn scale(self, factor: f32) -> ScaledPixels {
        ScaledPixels(self.0 * factor)
    }
}

/// Device pixels as floats.
#[derive(Clone, Copy, Default, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct ScaledPixels(pub f32);
float_unit!(ScaledPixels);

/// Whole device pixels.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct DevicePixels(pub i32);
unit!(DevicePixels, i32);

impl Half for DevicePixels {
    fn half(&self) -> Self {
        Self(self.0 / 2)
    }
}
impl Half for f32 {
    fn half(&self) -> Self {
        self / 2.
    }
}
impl Mul<i32> for DevicePixels {
    type Output = Self;
    fn mul(self, other: i32) -> Self {
        Self(self.0 * other)
    }
}
impl From<u32> for DevicePixels {
    fn from(value: u32) -> Self {
        Self(value as i32)
    }
}
impl From<DevicePixels> for u32 {
    fn from(value: DevicePixels) -> Self {
        value.0 as u32
    }
}
impl From<usize> for DevicePixels {
    fn from(value: usize) -> Self {
        Self(value as i32)
    }
}
impl From<DevicePixels> for usize {
    fn from(value: DevicePixels) -> Self {
        value.0 as usize
    }
}
impl From<DevicePixels> for u64 {
    fn from(value: DevicePixels) -> Self {
        value.0 as u64
    }
}
impl From<ScaledPixels> for DevicePixels {
    fn from(value: ScaledPixels) -> Self {
        Self(value.0.ceil() as i32)
    }
}
impl From<DevicePixels> for ScaledPixels {
    fn from(value: DevicePixels) -> Self {
        Self(value.0 as f32)
    }
}

pub const fn px(value: f32) -> Pixels {
    Pixels(value)
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct Point<T> {
    pub x: T,
    pub y: T,
}

pub const fn point<T>(x: T, y: T) -> Point<T> {
    Point { x, y }
}

impl<T: Copy> Point<T> {
    pub const fn new(x: T, y: T) -> Self {
        Self { x, y }
    }
    pub fn map<U>(self, f: impl Fn(T) -> U) -> Point<U> {
        Point {
            x: f(self.x),
            y: f(self.y),
        }
    }
}

impl<T: Copy + PartialOrd> Point<T> {
    pub fn max(self, other: Self) -> Self {
        Self {
            x: if self.x >= other.x { self.x } else { other.x },
            y: if self.y >= other.y { self.y } else { other.y },
        }
    }
    pub fn min(self, other: Self) -> Self {
        Self {
            x: if self.x <= other.x { self.x } else { other.x },
            y: if self.y <= other.y { self.y } else { other.y },
        }
    }
}

impl Point<Pixels> {
    pub fn scale(self, factor: f32) -> Point<ScaledPixels> {
        self.map(|value| value.scale(factor))
    }
}

impl<T: Add<Output = T>> Add for Point<T> {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        point(self.x + other.x, self.y + other.y)
    }
}
impl<T: Add<Output = T> + Copy> AddAssign for Point<T> {
    fn add_assign(&mut self, other: Self) {
        *self = point(self.x + other.x, self.y + other.y);
    }
}
impl<T: Sub<Output = T>> Sub for Point<T> {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        point(self.x - other.x, self.y - other.y)
    }
}
impl<T: Sub<Output = T> + Copy> SubAssign for Point<T> {
    fn sub_assign(&mut self, other: Self) {
        *self = point(self.x - other.x, self.y - other.y);
    }
}
impl<T: Neg<Output = T>> Neg for Point<T> {
    type Output = Self;
    fn neg(self) -> Self {
        point(-self.x, -self.y)
    }
}
impl<T: Mul<f32, Output = T>> Mul<f32> for Point<T> {
    type Output = Self;
    fn mul(self, factor: f32) -> Self {
        point(self.x * factor, self.y * factor)
    }
}
impl<T: Div<f32, Output = T>> Div<f32> for Point<T> {
    type Output = Self;
    fn div(self, factor: f32) -> Self {
        point(self.x / factor, self.y / factor)
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct Size<T> {
    pub width: T,
    pub height: T,
}

pub const fn size<T>(width: T, height: T) -> Size<T> {
    Size { width, height }
}

impl<T: Copy> Size<T> {
    pub const fn new(width: T, height: T) -> Self {
        Self { width, height }
    }
    pub fn map<U>(self, f: impl Fn(T) -> U) -> Size<U> {
        Size {
            width: f(self.width),
            height: f(self.height),
        }
    }
}

impl<T: Copy + PartialOrd> Size<T> {
    pub fn max(self, other: Self) -> Self {
        Self {
            width: if self.width >= other.width {
                self.width
            } else {
                other.width
            },
            height: if self.height >= other.height {
                self.height
            } else {
                other.height
            },
        }
    }
}

impl Size<Pixels> {
    pub fn scale(self, factor: f32) -> Size<ScaledPixels> {
        self.map(|value| value.scale(factor))
    }
    pub fn to_device_pixels(self, factor: f32) -> Size<DevicePixels> {
        self.map(|value| DevicePixels((value.0 * factor).round() as i32))
    }
}

impl<T: Add<Output = T>> Add for Size<T> {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        size(self.width + other.width, self.height + other.height)
    }
}
impl<T: Sub<Output = T>> Sub for Size<T> {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        size(self.width - other.width, self.height - other.height)
    }
}
impl<T: Mul<f32, Output = T>> Mul<f32> for Size<T> {
    type Output = Self;
    fn mul(self, factor: f32) -> Self {
        size(self.width * factor, self.height * factor)
    }
}
impl<T: Div<f32, Output = T>> Div<f32> for Size<T> {
    type Output = Self;
    fn div(self, factor: f32) -> Self {
        size(self.width / factor, self.height / factor)
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
pub struct Bounds<T> {
    pub origin: Point<T>,
    pub size: Size<T>,
}

pub fn bounds<T>(origin: Point<T>, size: Size<T>) -> Bounds<T> {
    Bounds { origin, size }
}

impl<T: Unit> Bounds<T> {
    pub const fn new(origin: Point<T>, size: Size<T>) -> Self {
        Self { origin, size }
    }
    pub fn from_corners(top_left: Point<T>, bottom_right: Point<T>) -> Self {
        Self {
            origin: top_left,
            size: size(bottom_right.x - top_left.x, bottom_right.y - top_left.y),
        }
    }
    pub fn centered_at(center: Point<T>, size: Size<T>) -> Self {
        Self {
            origin: point(center.x - size.width.half(), center.y - size.height.half()),
            size,
        }
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
    pub fn top_left(&self) -> Point<T> {
        self.origin
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
            self.origin.x + self.size.width.half(),
            self.origin.y + self.size.height.half(),
        )
    }
    pub fn contains(&self, point: &Point<T>) -> bool {
        point.x >= self.left()
            && point.x < self.right()
            && point.y >= self.top()
            && point.y < self.bottom()
    }
    pub fn intersects(&self, other: &Self) -> bool {
        self.left() < other.right()
            && other.left() < self.right()
            && self.top() < other.bottom()
            && other.top() < self.bottom()
    }
    pub fn intersect(&self, other: &Self) -> Self {
        let top_left = self.origin.max(other.origin);
        let bottom_right = self.bottom_right().min(other.bottom_right());
        Self::from_corners(top_left, top_left.max(bottom_right))
    }
    pub fn union(&self, other: &Self) -> Self {
        Self::from_corners(
            self.origin.min(other.origin),
            self.bottom_right().max(other.bottom_right()),
        )
    }
    pub fn is_empty(&self) -> bool {
        self.size.width <= T::default() || self.size.height <= T::default()
    }
    pub fn half_perimeter(&self) -> T {
        self.size.width + self.size.height
    }
    pub fn dilate(&self, amount: T) -> Self {
        Self {
            origin: point(self.origin.x - amount, self.origin.y - amount),
            size: size(
                self.size.width + amount + amount,
                self.size.height + amount + amount,
            ),
        }
    }
    pub fn inset(&self, amount: T) -> Self {
        Self {
            origin: point(self.origin.x + amount, self.origin.y + amount),
            size: size(
                self.size.width - amount - amount,
                self.size.height - amount - amount,
            ),
        }
    }
    pub fn extend(&self, edges: Edges<T>) -> Self {
        Self {
            origin: point(self.origin.x - edges.left, self.origin.y - edges.top),
            size: size(
                self.size.width + edges.left + edges.right,
                self.size.height + edges.top + edges.bottom,
            ),
        }
    }
    pub fn map<U>(self, f: impl Fn(T) -> U) -> Bounds<U> {
        Bounds {
            origin: point(f(self.origin.x), f(self.origin.y)),
            size: size(f(self.size.width), f(self.size.height)),
        }
    }
}

impl Bounds<Pixels> {
    pub fn scale(&self, factor: f32) -> Bounds<ScaledPixels> {
        self.map(|value| value.scale(factor))
    }
    pub fn to_device_pixels(&self, factor: f32) -> Bounds<DevicePixels> {
        self.map(|value| DevicePixels((value.0 * factor).round() as i32))
    }
}

impl Bounds<ScaledPixels> {
    pub fn to_device_pixels(&self) -> Bounds<DevicePixels> {
        let origin = self
            .origin
            .map(|value| DevicePixels(value.0.floor() as i32));
        let far = self
            .bottom_right()
            .map(|value| DevicePixels(value.0.ceil() as i32));
        Bounds::from_corners(origin, far)
    }
}

impl<T: Unit> Add<Point<T>> for Bounds<T> {
    type Output = Self;
    fn add(self, offset: Point<T>) -> Self {
        Self {
            origin: self.origin + offset,
            size: self.size,
        }
    }
}
impl<T: Unit> Sub<Point<T>> for Bounds<T> {
    type Output = Self;
    fn sub(self, offset: Point<T>) -> Self {
        Self {
            origin: self.origin - offset,
            size: self.size,
        }
    }
}
impl<T: Unit + Mul<f32, Output = T>> Mul<f32> for Bounds<T> {
    type Output = Self;
    fn mul(self, factor: f32) -> Self {
        Self {
            origin: self.origin * factor,
            size: self.size * factor,
        }
    }
}
impl<T: Unit + Div<f32, Output = T>> Div<f32> for Bounds<T> {
    type Output = Self;
    fn div(self, factor: f32) -> Self {
        Self {
            origin: self.origin / factor,
            size: self.size / factor,
        }
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct Edges<T> {
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

impl Edges<Pixels> {
    pub fn scale(&self, factor: f32) -> Edges<ScaledPixels> {
        self.map(|value| value.scale(factor))
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct Corners<T> {
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
}

impl Corners<Pixels> {
    pub fn scale(&self, factor: f32) -> Corners<ScaledPixels> {
        self.map(|value| value.scale(factor))
    }
    /// Shrinks the radii so that adjacent corners fit the quad, like CSS.
    pub fn clamp_radii_for_quad_size(self, size: Size<Pixels>) -> Self {
        let max = size.width.min(size.height) / 2.;
        self.map(|radius| radius.min(max).max(Pixels::ZERO))
    }
}

impl<T: Copy + PartialEq + Default> Corners<T> {
    pub fn is_zero(&self) -> bool {
        [
            self.top_left,
            self.top_right,
            self.bottom_right,
            self.bottom_left,
        ]
        .iter()
        .all(|value| *value == T::default())
    }
}

impl From<Pixels> for Corners<Pixels> {
    fn from(value: Pixels) -> Self {
        Self::all(value)
    }
}
impl From<Pixels> for Edges<Pixels> {
    fn from(value: Pixels) -> Self {
        Self::all(value)
    }
}

/// A rectangle clip, rounded when `corner_radii` is not zero.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct ContentMask<P> {
    pub bounds: Bounds<P>,
    pub rounded: Bounds<P>,
    pub corner_radii: Corners<P>,
}

impl ContentMask<Pixels> {
    pub fn rounded(bounds: Bounds<Pixels>, corner_radii: Corners<Pixels>) -> Self {
        Self {
            bounds,
            rounded: bounds,
            corner_radii: corner_radii.clamp_radii_for_quad_size(bounds.size),
        }
    }
    pub fn scale(&self, factor: f32) -> ContentMask<ScaledPixels> {
        ContentMask {
            bounds: self.bounds.scale(factor),
            rounded: self.rounded.scale(factor),
            corner_radii: self.corner_radii.scale(factor),
        }
    }
    /// The overlap of two masks. When both are rounded, the rounded rectangle of `self` wins.
    pub fn intersect(&self, other: &Self) -> Self {
        let rounded = if self.corner_radii.is_zero() {
            other
        } else {
            self
        };
        Self {
            bounds: self.bounds.intersect(&other.bounds),
            rounded: rounded.rounded,
            corner_radii: rounded.corner_radii,
        }
    }
}

impl<P: Default> From<Bounds<P>> for ContentMask<P> {
    fn from(bounds: Bounds<P>) -> Self {
        Self {
            bounds,
            rounded: Bounds::default(),
            corner_radii: Corners::default(),
        }
    }
}

/// Rounds midpoints toward zero, so a 1px line at 150% stays 1 device pixel.
pub(crate) fn round_half_toward_zero(value: f32) -> f32 {
    (value.abs() - 0.5).ceil().copysign(value)
}

pub(crate) fn round_to_device_pixel(logical: f32, scale_factor: f32) -> f32 {
    round_half_toward_zero(logical * scale_factor)
}

pub(crate) fn round_stroke_to_device_pixel(logical: f32, scale_factor: f32) -> f32 {
    if logical == 0. {
        0.
    } else {
        round_to_device_pixel(logical.max(0.), scale_factor).max(1.)
    }
}
