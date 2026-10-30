/// A color with straight alpha, all channels from 0 to 1, in sRGB.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
#[repr(C)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

/// A color from `0xRRGGBBAA`.
pub fn rgba(hex: u32) -> Rgba {
    let channel = |shift: u32| ((hex >> shift) & 0xff) as f32 / 255.;
    Rgba {
        r: channel(24),
        g: channel(16),
        b: channel(8),
        a: channel(0),
    }
}

/// An opaque color from `0xRRGGBB`.
pub fn rgb(hex: u32) -> Rgba {
    rgba(hex << 8 | 0xff)
}

pub const fn black() -> Rgba {
    Rgba {
        r: 0.,
        g: 0.,
        b: 0.,
        a: 1.,
    }
}

pub const fn white() -> Rgba {
    Rgba {
        r: 1.,
        g: 1.,
        b: 1.,
        a: 1.,
    }
}

pub const fn transparent_black() -> Rgba {
    Rgba {
        r: 0.,
        g: 0.,
        b: 0.,
        a: 0.,
    }
}

impl Rgba {
    pub fn opacity(self, factor: f32) -> Self {
        Self {
            a: self.a * factor.clamp(0., 1.),
            ..self
        }
    }
    pub fn alpha(self, a: f32) -> Self {
        Self {
            a: a.clamp(0., 1.),
            ..self
        }
    }
    pub fn is_transparent(&self) -> bool {
        self.a == 0.
    }
    pub fn is_opaque(&self) -> bool {
        self.a >= 1.
    }
    /// Paints `other` over this color.
    pub fn blend(self, other: Self) -> Self {
        if other.a >= 1. {
            return other;
        }
        let a = other.a + self.a * (1. - other.a);
        if a <= 0. {
            return transparent_black();
        }
        let mix = |top: f32, bottom: f32| (top * other.a + bottom * self.a * (1. - other.a)) / a;
        Self {
            r: mix(other.r, self.r),
            g: mix(other.g, self.g),
            b: mix(other.b, self.b),
            a,
        }
    }
}

impl From<Rgba> for u32 {
    fn from(color: Rgba) -> Self {
        let channel = |value: f32| (value.clamp(0., 1.) * 255.).round() as u32;
        channel(color.r) << 24 | channel(color.g) << 16 | channel(color.b) << 8 | channel(color.a)
    }
}
