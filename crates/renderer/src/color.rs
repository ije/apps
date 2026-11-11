// Straight alpha. All channels go from 0 to 1.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
#[repr(C)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Self = Self::new(0., 0., 0., 0.);
    pub const BLACK: Self = Self::new(0., 0., 0., 1.);
    pub const WHITE: Self = Self::new(1., 1., 1., 1.);

    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub fn hex(rgba: u32) -> Self {
        let [r, g, b, a] = rgba.to_be_bytes().map(|byte| byte as f32 / 255.);
        Self { r, g, b, a }
    }

    pub fn to_hex(self) -> u32 {
        u32::from_be_bytes(
            self.to_array()
                .map(|channel| (channel.clamp(0., 1.) * 255.).round() as u8),
        )
    }

    pub fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    pub fn fade(self, factor: f32) -> Self {
        Self {
            a: self.a * factor.clamp(0., 1.),
            ..self
        }
    }

    pub fn with_alpha(self, alpha: f32) -> Self {
        Self {
            a: alpha.clamp(0., 1.),
            ..self
        }
    }

    pub fn is_transparent(self) -> bool {
        self.a == 0.
    }

    pub fn is_opaque(self) -> bool {
        self.a >= 1.
    }

    pub fn under(self, top: Self) -> Self {
        if top.a >= 1. {
            return top;
        }
        let rest = self.a * (1. - top.a);
        let a = top.a + rest;
        if a <= 0. {
            return Self::TRANSPARENT;
        }
        Self {
            r: (top.r * top.a + self.r * rest) / a,
            g: (top.g * top.a + self.g * rest) / a,
            b: (top.b * top.a + self.b * rest) / a,
            a,
        }
    }
}
