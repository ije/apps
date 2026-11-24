use crate::css::{BORDERS, Prop, Style};
impl Lengths {
    pub fn edges(&self, style: &Style, sides: [Prop; 4], base: f32) -> [f32; 4] {
        sides.map(|side| {
            style
                .get(side)
                .map_or(0., |value| self.resolve(value, base).unwrap_or(0.))
        })
    }

    pub fn borders(&self, style: &Style, base: f32) -> [f32; 4] {
        BORDERS.map(|[width, kind, _]| {
            if style
                .get(kind)
                .is_none_or(|kind| matches!(kind, "none" | "hidden"))
            {
                return 0.;
            }
            style
                .get(width)
                .map_or(Some(3.), |width| self.resolve(width, base))
                .unwrap_or(0.)
                .max(0.)
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FontUnits {
    pub em: f32,
    pub ch: f32,
    pub ex: f32,
    pub cap: f32,
    pub ic: f32,
    pub lh: f32,
    pub absolute: bool,
}

impl FontUnits {
    pub fn new(em: f32) -> Self {
        Self {
            em,
            ch: em * 0.5,
            ex: em * 0.5,
            cap: em * 0.8,
            ic: em,
            lh: em * 1.2,
            absolute: false,
        }
    }

    pub fn bits(self) -> [u32; 7] {
        let [em, ch, ex, cap, ic, lh] =
            [self.em, self.ch, self.ex, self.cap, self.ic, self.lh].map(f32::to_bits);
        [em, ch, ex, cap, ic, lh, u32::from(self.absolute)]
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Lengths {
    pub font: FontUnits,
    pub root: FontUnits,
    pub viewport: [f32; 2],
    pub container: Option<[f32; 2]>,
}

impl Lengths {
    pub fn new(font: f32, vw: f32, vh: f32) -> Self {
        Self {
            font: FontUnits::new(font),
            root: FontUnits::new(16.),
            viewport: [vw, vh],
            container: None,
        }
    }

    pub fn parse(&self, source: &str, base: f32) -> Option<(f32, bool)> {
        let (value, kind) = self.math(source, base, &[])?;
        let value = value as f32;
        (matches!(kind, Kind::Number | Kind::Length) && value.is_finite())
            .then_some((value, kind == Kind::Length))
    }

    pub fn math(&self, source: &str, base: f32, vars: &[(&str, f64)]) -> Option<(f64, Kind)> {
        self.math_unclamped(source, base, vars)
            .map(|(value, kind)| (value.clamp(-33_554_428., 33_554_428.), kind))
    }

    pub fn math_unclamped(
        &self,
        source: &str,
        base: f32,
        vars: &[(&str, f64)],
    ) -> Option<(f64, Kind)> {
        let mut parser = Math {
            rest: source.trim(),
            units: self,
            base,
            depth: 0,
            vars,
        };
        let value = parser.atom()?;
        (parser.rest.trim().is_empty() && !value.0.is_nan()).then_some(value)
    }

    pub fn resolve(&self, source: &str, base: f32) -> Option<f32> {
        let (value, dimensional) = self.parse(source, base)?;
        (dimensional || value == 0.).then_some(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Kind {
    Number,
    Length,
    Angle,
    Time,
}

struct Math<'a> {
    rest: &'a str,
    units: &'a Lengths,
    base: f32,
    depth: usize,
    vars: &'a [(&'a str, f64)],
}

impl Math<'_> {
    pub(crate) fn expression(&mut self, minimum: u8) -> Option<(f64, Kind)> {
        self.depth += 1;
        if self.depth > 64 {
            return None;
        }
        let mut left = self.atom()?;
        loop {
            let rest = self.rest.trim_start();
            let operator = rest.as_bytes().first().copied().unwrap_or(0);
            let precedence = match operator {
                b'+' | b'-' => 1,
                b'*' | b'/' => 2,
                _ => break,
            };
            if precedence < minimum {
                break;
            }
            if precedence == 1
                && (rest.len() == self.rest.len()
                    || !rest.as_bytes().get(1).is_some_and(u8::is_ascii_whitespace))
            {
                return None;
            }
            self.rest = &rest[1..];
            let right = self.expression(precedence + 1)?;
            left = match (operator, left.1, right.1) {
                (b'+', a, b) if a == b => (left.0 + right.0, a),
                (b'-', a, b) if a == b => (left.0 - right.0, a),
                (b'*', kind, Kind::Number) | (b'*', Kind::Number, kind) => (left.0 * right.0, kind),
                (b'/', kind, Kind::Number) if right.0 != 0. => (left.0 / right.0, kind),
                (b'/', a, b) if a == b && right.0 != 0. => (left.0 / right.0, Kind::Number),
                _ => return None,
            };
            if left.0.is_nan() {
                return None;
            }
        }
        self.depth -= 1;
        Some(left)
    }

    pub(crate) fn atom(&mut self) -> Option<(f64, Kind)> {
        self.rest = self.rest.trim_start();
        if let Some(rest) = self.rest.strip_prefix('-')
            && rest.starts_with(|c: char| c.is_ascii_alphabetic() || c == '(')
        {
            self.rest = rest;
            let (value, kind) = self.atom()?;
            return Some((-value, kind));
        }
        let end = if self.rest.starts_with(|c: char| c.is_ascii_alphabetic()) {
            self.rest
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '-')
                .unwrap_or(self.rest.len())
        } else {
            0
        };
        let name = &self.rest[..end];
        let lower;
        let name = if name.bytes().any(|c| c.is_ascii_uppercase()) {
            lower = name.to_ascii_lowercase();
            lower.as_str()
        } else {
            name
        };
        if end > 0 && !self.rest[end..].starts_with('(') {
            self.rest = &self.rest[end..];
            if let Some((_, value)) = self.vars.iter().find(|(var, _)| *var == name) {
                return Some((*value, Kind::Number));
            }
            let value = match name {
                "e" if self.depth > 0 => std::f64::consts::E,
                "pi" if self.depth > 0 => std::f64::consts::PI,
                "infinity" if self.depth > 0 => f64::INFINITY,
                "nan" if self.depth > 0 => f64::NAN,
                _ => return None,
            };
            return Some((value, Kind::Number));
        }
        if end > 0 || self.rest.starts_with('(') {
            if end == 0 && self.depth == 0 {
                return None;
            }
            self.rest = &self.rest[end + 1..];
            let strategy = ["nearest", "up", "down", "to-zero"]
                .into_iter()
                .filter(|_| name == "round")
                .find_map(|keyword| {
                    let rest = self.rest.trim_start();
                    let rest = rest
                        .get(..keyword.len())
                        .filter(|word| word.eq_ignore_ascii_case(keyword))
                        .and(rest.get(keyword.len()..))?;
                    self.rest = rest.trim_start().strip_prefix(',')?;
                    Some(keyword)
                })
                .unwrap_or("nearest");
            let mut args = [self.expression(0)?; 3];
            let mut count = 1;
            loop {
                self.rest = self.rest.trim_start();
                if let Some(rest) = self.rest.strip_prefix(')') {
                    self.rest = rest;
                    break;
                }
                self.rest = self.rest.strip_prefix(',')?;
                let value = self.expression(0)?;
                match name {
                    "min" | "max" | "hypot" if value.1 == args[0].1 => {
                        args[0].0 = match name {
                            "min" => args[0].0.min(value.0),
                            "max" => args[0].0.max(value.0),
                            _ => args[0].0.hypot(value.0),
                        }
                    }
                    "clamp" | "round" | "mod" | "rem" | "atan2" | "pow" | "log" if count < 3 => {
                        args[count] = value;
                        count += 1;
                    }
                    _ => return None,
                }
            }
            let [(a, kind), (b, other), (c, last)] = args;
            let same = kind == other;
            let number = kind == Kind::Number;
            return Some(match (name, count) {
                ("" | "calc" | "min" | "max", _) => args[0],
                ("hypot", _) => (a.abs(), kind),
                ("clamp", 3) if same && kind == last => (a.max(b.min(c)), kind),
                ("round", 1 | 2) => {
                    let step = if count == 2 && same {
                        b.abs()
                    } else if count == 1 {
                        1.
                    } else {
                        return None;
                    };
                    let value = a / step;
                    let value = match strategy {
                        "up" => value.ceil(),
                        "down" => value.floor(),
                        "to-zero" => value.trunc(),
                        _ => (value + 0.5).floor(),
                    };
                    (value * step, kind)
                }
                ("mod", 2) if same => (a - b * (a / b).floor(), kind),
                ("rem", 2) if same => (a % b, kind),
                ("abs", 1) => (a.abs(), kind),
                ("sign", 1) => (f64::from(i8::from(a > 0.) - i8::from(a < 0.)), Kind::Number),
                ("sin" | "cos" | "tan", 1) if matches!(kind, Kind::Number | Kind::Angle) => {
                    let radians = if number { a } else { a.to_radians() };
                    let value = match name {
                        "sin" => radians.sin(),
                        "cos" => radians.cos(),
                        _ => radians.tan(),
                    };
                    (value, Kind::Number)
                }
                ("asin", 1) if number => (a.asin().to_degrees(), Kind::Angle),
                ("acos", 1) if number => (a.acos().to_degrees(), Kind::Angle),
                ("atan", 1) if number => (a.atan().to_degrees(), Kind::Angle),
                ("atan2", 2) if same => (a.atan2(b).to_degrees(), Kind::Angle),
                ("pow", 2) if number && same => (a.powf(b), Kind::Number),
                ("sqrt", 1) if number => (a.sqrt(), Kind::Number),
                ("exp", 1) if number => (a.exp(), Kind::Number),
                ("log", 1) if number => (a.ln(), Kind::Number),
                ("log", 2) if number && same => (a.ln() / b.ln(), Kind::Number),
                _ => return None,
            })
            .filter(|value| !value.0.is_nan());
        }
        let bytes = self.rest.as_bytes();
        let mut end = usize::from(bytes.first().is_some_and(|b| matches!(b, b'+' | b'-')));
        let start = end;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        let mut digits = end - start;
        if bytes.get(end) == Some(&b'.') {
            end += 1;
            let start = end;
            while bytes.get(end).is_some_and(u8::is_ascii_digit) {
                end += 1;
            }
            if start == end {
                return None;
            }
            digits += end - start;
        }
        if digits == 0 {
            return None;
        }
        if bytes.get(end).is_some_and(|b| matches!(b, b'e' | b'E')) {
            let mut exponent = end + 1;
            if bytes
                .get(exponent)
                .is_some_and(|b| matches!(b, b'+' | b'-'))
            {
                exponent += 1;
            }
            if bytes.get(exponent).is_some_and(u8::is_ascii_digit) {
                end = exponent + 1;
                while bytes.get(end).is_some_and(u8::is_ascii_digit) {
                    end += 1;
                }
            }
        }
        let value = self.rest[..end].parse::<f64>().ok()?;
        self.rest = &self.rest[end..];
        let end = if self.rest.starts_with('%') {
            1
        } else {
            self.rest
                .find(|c: char| !c.is_ascii_alphabetic())
                .unwrap_or(self.rest.len())
        };
        let unit = &self.rest[..end];
        self.rest = &self.rest[end..];
        let lower;
        let unit = if unit.bytes().any(|c| c.is_ascii_uppercase()) {
            lower = unit.to_ascii_lowercase();
            lower.as_str()
        } else {
            unit
        };
        let font = self.units.font;
        let root = self.units.root;
        let [vw, vh] = self.units.viewport;
        let (scale, kind) = match unit {
            "" => (1., Kind::Number),
            "deg" => (1., Kind::Angle),
            "grad" => (0.9, Kind::Angle),
            "rad" => (180. / std::f64::consts::PI, Kind::Angle),
            "turn" => (360., Kind::Angle),
            "s" => (1., Kind::Time),
            "ms" => (0.001, Kind::Time),
            "%" => (f64::from(self.base) / 100., Kind::Length),
            _ => {
                let scale = match unit {
                    "px" => 1.,
                    "em" => font.em,
                    "rem" => root.em,
                    "ch" => font.ch,
                    "rch" => root.ch,
                    "ex" => font.ex,
                    "rex" => root.ex,
                    "cap" => font.cap,
                    "rcap" => root.cap,
                    "ic" => font.ic,
                    "ric" => root.ic,
                    "lh" => font.lh,
                    "rlh" => root.lh,
                    "vw" | "svw" | "lvw" | "dvw" | "vi" | "svi" | "lvi" | "dvi" => vw / 100.,
                    "vh" | "svh" | "lvh" | "dvh" | "vb" | "svb" | "lvb" | "dvb" => vh / 100.,
                    "cqw" | "cqi" => self.units.container.unwrap_or(self.units.viewport)[0] / 100.,
                    "cqh" | "cqb" => self.units.container.unwrap_or(self.units.viewport)[1] / 100.,
                    "cqmin" => {
                        self.units
                            .container
                            .unwrap_or(self.units.viewport)
                            .into_iter()
                            .fold(f32::INFINITY, f32::min)
                            / 100.
                    }
                    "cqmax" => {
                        self.units
                            .container
                            .unwrap_or(self.units.viewport)
                            .into_iter()
                            .fold(0., f32::max)
                            / 100.
                    }
                    "vmin" | "svmin" | "lvmin" | "dvmin" => vw.min(vh) / 100.,
                    "vmax" | "svmax" | "lvmax" | "dvmax" => vw.max(vh) / 100.,
                    "in" => 96.,
                    "cm" => 96. / 2.54,
                    "mm" => 96. / 25.4,
                    "q" => 96. / 101.6,
                    "pt" => 96. / 72.,
                    "pc" => 16.,
                    _ => return None,
                };
                (f64::from(scale), Kind::Length)
            }
        };
        let value = value * scale;
        value.is_finite().then_some((value, kind))
    }
}
