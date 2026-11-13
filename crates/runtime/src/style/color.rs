use super::parser::{Kind, Lengths};

#[derive(Clone, Copy, PartialEq)]
enum Space {
    Srgb,
    SrgbLinear,
    DisplayP3,
    A98,
    ProPhoto,
    Rec2020,
    XyzD50,
    XyzD65,
    Lab,
    Lch,
    Oklab,
    Oklch,
    Hsl,
    Hwb,
}

const SPACES: [(&str, Space); 15] = [
    ("srgb", Space::Srgb),
    ("srgb-linear", Space::SrgbLinear),
    ("display-p3", Space::DisplayP3),
    ("a98-rgb", Space::A98),
    ("prophoto-rgb", Space::ProPhoto),
    ("rec2020", Space::Rec2020),
    ("xyz", Space::XyzD65),
    ("xyz-d65", Space::XyzD65),
    ("xyz-d50", Space::XyzD50),
    ("lab", Space::Lab),
    ("lch", Space::Lch),
    ("oklab", Space::Oklab),
    ("oklch", Space::Oklch),
    ("hsl", Space::Hsl),
    ("hwb", Space::Hwb),
];

const SYSTEM_COLORS: [(&str, u32, &str); 19] = [
    ("canvas", 0xffffff, "#121212"),
    ("canvastext", 0x000000, "#ffffff"),
    ("linktext", 0x0000ee, "#9e9eff"),
    ("visitedtext", 0x551a8b, "#d0adf0"),
    ("activetext", 0xff0000, "#ff0000"),
    ("buttonface", 0xefefef, "#6b6b6b"),
    ("buttontext", 0x000000, "#ffffff"),
    ("buttonborder", 0x000000, "#ffffff"),
    ("field", 0xffffff, "#3b3b3b"),
    ("fieldtext", 0x000000, "#ffffff"),
    ("highlight", 0xb3d7ff, "#b3d7ffcc"),
    ("highlighttext", 0x000000, "#000000"),
    ("selecteditem", 0xe0e0e0, "#99c8ff"),
    ("selecteditemtext", 0x000000, "#3b3b3b"),
    ("mark", 0xffff00, "#ffff00"),
    ("marktext", 0x000000, "#000000"),
    ("graytext", 0x808080, "#808080"),
    ("accentcolor", 0x0075ff, "#0075ff"),
    ("accentcolortext", 0xffffff, "#ffffff"),
];

pub(crate) fn serialize(value: &str) -> Option<String> {
    thread_local! {
        static SERIALIZED: std::cell::RefCell<rustc_hash::FxHashMap<Box<str>, Option<String>>> =
            Default::default();
    }
    if let Some(text) = SERIALIZED.with_borrow(|memo| memo.get(value).cloned()) {
        return text;
    }
    let text = (|| {
        let lower = value.trim().to_ascii_lowercase();
        let (mut space, mut channels, alpha) = parse(&lower, 0)?;
        let function = lower.split_once('(').map_or("", |(name, _)| name);
        let own = matches!(
            function,
            "color" | "color-mix" | "lab" | "lch" | "oklab" | "oklch"
        ) || lower.contains("(from ");
        if matches!(space, Space::Hsl | Space::Hwb) {
            channels = convert(space, channels, Space::Srgb);
            space = Space::Srgb;
        }
        let number = |value: f64| {
            if value.is_nan() {
                "none".to_owned()
            } else {
                super::value::number(value)
            }
        };
        let name = match space {
            Space::Srgb => "color(srgb ",
            Space::SrgbLinear => "color(srgb-linear ",
            Space::DisplayP3 => "color(display-p3 ",
            Space::A98 => "color(a98-rgb ",
            Space::ProPhoto => "color(prophoto-rgb ",
            Space::Rec2020 => "color(rec2020 ",
            Space::XyzD50 => "color(xyz-d50 ",
            Space::XyzD65 => "color(xyz-d65 ",
            Space::Lab => "lab(",
            Space::Lch => "lch(",
            Space::Oklab => "oklab(",
            Space::Oklch => "oklch(",
            Space::Hsl | Space::Hwb => "",
        };
        if !own {
            let (rgb, alpha) = color(value)?;
            let fraction = f32::from(alpha) / 255.;
            let short = (fraction * 100.).round() / 100.;
            return Some(match alpha {
                255 => format!("rgb({}, {}, {})", rgb >> 16, rgb >> 8 & 255, rgb & 255),
                _ => format!(
                    "rgba({}, {}, {}, {})",
                    rgb >> 16,
                    rgb >> 8 & 255,
                    rgb & 255,
                    if (short * 255.).round() as u8 == alpha {
                        short
                    } else {
                        (fraction * 1000.).round() / 1000.
                    }
                ),
            });
        }
        let channels = channels.map(number).join(" ");
        Some(if alpha < 1. {
            format!("{name}{channels} / {})", number(alpha.max(0.)))
        } else {
            format!("{name}{channels})")
        })
    })();
    SERIALIZED.with_borrow_mut(|memo| {
        if memo.len() >= 1024 {
            memo.clear();
        }
        memo.insert(value.into(), text.clone());
    });
    text
}

pub(crate) fn system(name: &str, dark: bool) -> Option<String> {
    let (_, light, night) = SYSTEM_COLORS
        .iter()
        .find(|(system, ..)| system.eq_ignore_ascii_case(name))?;
    Some(if dark {
        (*night).to_owned()
    } else {
        format!("#{light:06x}")
    })
}

const SRGB_TO_XYZ: [[f64; 3]; 3] = [
    [0.41239079926595934, 0.357584339383878, 0.1804807884018343],
    [0.21263900587151027, 0.715168678767756, 0.07219231536073371],
    [0.01933081871559182, 0.11919477979462598, 0.9505321522496607],
];
const XYZ_TO_SRGB: [[f64; 3]; 3] = [
    [3.2409699419045226, -1.537383177570094, -0.4986107602930034],
    [-0.9692436362808796, 1.8759675015077202, 0.04155505740717559],
    [
        0.05563007969699366,
        -0.20397695888897652,
        1.0569715142428786,
    ],
];
const P3_TO_XYZ: [[f64; 3]; 3] = [
    [0.4865709486482162, 0.26566769316909306, 0.1982172852343625],
    [0.2289745640697488, 0.6917385218365064, 0.079286914093745],
    [0., 0.04511338185890264, 1.043944368900976],
];
const XYZ_TO_P3: [[f64; 3]; 3] = [
    [2.493496911941425, -0.9313836179191239, -0.40271078445071684],
    [
        -0.8294889695615747,
        1.7626640603183463,
        0.023624685841943577,
    ],
    [
        0.03584583024378447,
        -0.07617238926804182,
        0.9568845240076872,
    ],
];
const A98_TO_XYZ: [[f64; 3]; 3] = [
    [0.5766690429101305, 0.1855582379065463, 0.1882286462349947],
    [0.29734497525053605, 0.6273635662554661, 0.07529145849399788],
    [0.02703136138641234, 0.07068885253582723, 0.9913375368376388],
];
const XYZ_TO_A98: [[f64; 3]; 3] = [
    [
        2.0415879038107465,
        -0.5650069742788596,
        -0.34473135077832956,
    ],
    [-0.9692436362808795, 1.8759675015077202, 0.04155505740717557],
    [
        0.013444280632031142,
        -0.11836239223101838,
        1.0151749943912054,
    ],
];
// ProPhoto RGB is relative to D50.
const PROPHOTO_TO_XYZ: [[f64; 3]; 3] = [
    [0.7977604896723027, 0.13518583717574031, 0.0313493495815248],
    [
        0.2880711282292934,
        0.7118432178101014,
        0.00008565396060525902,
    ],
    [0., 0., 0.8251046025104601],
];
const XYZ_TO_PROPHOTO: [[f64; 3]; 3] = [
    [
        1.3457989731028281,
        -0.25558010007997534,
        -0.05110628506753401,
    ],
    [-0.5446224939028347, 1.5082327413132781, 0.02053603239147973],
    [0., 0., 1.2119675456389454],
];
const REC2020_TO_XYZ: [[f64; 3]; 3] = [
    [0.6369580483012914, 0.14461690358620832, 0.1688809751287843],
    [0.2627002120112671, 0.6779980715188708, 0.05930171646986196],
    [0., 0.028072693049087428, 1.060985057710791],
];
const XYZ_TO_REC2020: [[f64; 3]; 3] = [
    [
        1.7166511879712674,
        -0.35567078377639233,
        -0.25336628137365974,
    ],
    [-0.6666843518324892, 1.6164812366349395, 0.01576854581391113],
    [
        0.017639857445310783,
        -0.042770613257808524,
        0.9421031212354738,
    ],
];
const REC2020_ALPHA: f64 = 1.09929682680944;
const REC2020_BETA: f64 = 0.018053968510807;
const D65_TO_D50: [[f64; 3]; 3] = [
    [
        1.0479297925449969,
        0.022946870601609652,
        -0.05019226628920524,
    ],
    [
        0.02962780877005599,
        0.9904344267538799,
        -0.017073799063418826,
    ],
    [
        -0.009243040646204504,
        0.015055191490298152,
        0.7518742814281371,
    ],
];
const D50_TO_D65: [[f64; 3]; 3] = [
    [0.955473421488075, -0.02309845494876471, 0.06325924320057072],
    [
        -0.0283697093338637,
        1.0099953980813041,
        0.021041441191917323,
    ],
    [
        0.012314014864481998,
        -0.020507649298898964,
        1.330365926242124,
    ],
];
const XYZ_TO_LMS: [[f64; 3]; 3] = [
    [0.819022437996703, 0.3619062600528904, -0.1288737815209879],
    [0.0329836539323885, 0.9292868615863434, 0.0361446663506424],
    [0.0481771893596242, 0.2642395317527308, 0.6335478284694309],
];
const LMS_TO_OKLAB: [[f64; 3]; 3] = [
    [0.210454268309314, 0.7936177747023054, -0.0040720430116193],
    [1.9779985324311684, -2.42859224204858, 0.450593709617411],
    [0.0259040424655478, 0.7827717124575296, -0.8086757549230774],
];
const OKLAB_TO_LMS: [[f64; 3]; 3] = [
    [1., 0.3963377773761749, 0.2158037573099136],
    [1., -0.1055613458156586, -0.0638541728258133],
    [1., -0.0894841775298119, -1.2914855480194092],
];
const LMS_TO_XYZ: [[f64; 3]; 3] = [
    [1.2268798758459243, -0.5578149944602171, 0.2813910456659647],
    [-0.0405757452148008, 1.112286803280317, -0.0717110580655164],
    [-0.0763729366746601, -0.4214933324022432, 1.5869240198367816],
];
const D50: [f64; 3] = [0.3457 / 0.3585, 1., (1. - 0.3457 - 0.3585) / 0.3585];
const KAPPA: f64 = 24389. / 27.;
const EPSILON: f64 = 216. / 24389.;

type Rgb = Option<(u32, u8)>;

thread_local! {
    static COLORS: std::cell::RefCell<rustc_hash::FxHashMap<Box<str>, Rgb>> = Default::default();
}

pub fn rgba((rgb, alpha): (u32, u8)) -> renderer::Color {
    renderer::Color::hex(rgb << 8 | alpha as u32)
}

pub fn color(value: &str) -> Option<(u32, u8)> {
    let value = value.trim();
    if let Some(color) = COLORS.with_borrow(|colors| colors.get(value).copied()) {
        return color;
    }
    let color = parse(value, 0).map(|(space, channels, alpha)| {
        let rgb = convert(space, channels, Space::Srgb);
        // Browsers keep channels as f32, so 0.1 * 255 rounds up to 26.
        let [r, g, b] = rgb.map(|v| ((v as f32).clamp(0., 1.) * 255.).round() as u32);
        (
            r << 16 | g << 8 | b,
            (alpha.clamp(0., 1.) * 255.).round() as u8,
        )
    });
    COLORS.with_borrow_mut(|colors| {
        if colors.len() >= 1024 {
            colors.clear();
        }
        colors.insert(value.into(), color);
    });
    color
}

// Returns whether a color is a legacy sRGB color (hex, named, rgb(), hsl() or hwb()), and its Oklab channels and alpha.
// Missing components stay NaN.
pub(crate) fn oklab(value: &str) -> Option<(bool, [f64; 4])> {
    let value = value.trim();
    let (space, channels, alpha) = parse(value, 0)?;
    let legacy = value.split_once('(').is_none_or(|(name, body)| {
        ["rgb", "rgba", "hsl", "hsla", "hwb"]
            .iter()
            .any(|legacy| name.eq_ignore_ascii_case(legacy))
            && !body
                .trim_start()
                .get(..5)
                .is_some_and(|word| word.eq_ignore_ascii_case("from "))
    });
    let [l, a, b] = convert(space, channels, Space::Oklab);
    Some((legacy, [l, a, b, alpha]))
}

fn parse(value: &str, depth: u8) -> Option<(Space, [f64; 3], f64)> {
    if let Some(hex) = value.strip_prefix('#') {
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let number = u32::from_str_radix(hex, 16).ok()?;
        let (rgb, alpha) = match hex.len() {
            3 | 4 => {
                let number = if hex.len() == 3 {
                    number << 4 | 15
                } else {
                    number
                };
                let [r, g, b, a] = [12, 8, 4, 0].map(|shift| (number >> shift & 15) * 17);
                (r << 16 | g << 8 | b, a)
            }
            6 => (number, 255),
            8 => (number >> 8, number & 255),
            _ => return None,
        };
        return Some((
            Space::Srgb,
            [16, 8, 0].map(|shift| f64::from(rgb >> shift & 255) / 255.),
            f64::from(alpha) / 255.,
        ));
    }
    let Some((name, body)) = value.split_once('(') else {
        if value.eq_ignore_ascii_case("transparent") {
            return Some((Space::Srgb, [0.; 3], 0.));
        }
        let rgb = match cssparser::color::parse_named_color(value) {
            Ok((r, g, b)) => u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b),
            Err(()) => {
                SYSTEM_COLORS
                    .iter()
                    .find(|(name, ..)| name.eq_ignore_ascii_case(value))?
                    .1
            }
        };
        return Some((
            Space::Srgb,
            [16, 8, 0].map(|shift| f64::from(rgb >> shift & 255) / 255.),
            1.,
        ));
    };
    let body = body.strip_suffix(')')?;
    if depth > 16 {
        return None;
    }
    let lower;
    let name = if name.bytes().any(|c| c.is_ascii_uppercase()) {
        lower = name.to_ascii_lowercase();
        lower.as_str()
    } else {
        name
    };
    let units = Lengths::new(16., 0., 0.);
    let find = |spaces: &[(&str, Space)], name: &str| {
        spaces
            .iter()
            .find(|(space, _)| space.eq_ignore_ascii_case(name))
            .map(|(_, space)| *space)
    };
    if name == "color-mix" {
        let parts = super::components(body, ',');
        let (method, colors) = match parts.as_slice() {
            [method, first, second] => (*method, [*first, *second]),
            [first, second] => ("in oklab", [*first, *second]),
            _ => return None,
        };
        let mut words = method.split_ascii_whitespace();
        if !words.next()?.eq_ignore_ascii_case("in") {
            return None;
        }
        let space = find(&SPACES, words.next()?)?;
        let hue = match space {
            Space::Hsl | Space::Hwb => Some(0),
            Space::Lch | Space::Oklch => Some(2),
            _ => None,
        };
        let method = match (words.next(), words.next(), words.next()) {
            (None, ..) => 0,
            (Some(method), Some(word), None)
                if hue.is_some() && word.eq_ignore_ascii_case("hue") =>
            {
                ["shorter", "longer", "increasing", "decreasing"]
                    .iter()
                    .position(|name| name.eq_ignore_ascii_case(method))?
            }
            _ => return None,
        };
        let mut mixed = [([0.; 3], 0., None); 2];
        for (index, part) in colors.into_iter().enumerate() {
            let (color, weight) = match super::components(part, ' ').as_slice() {
                [color] => (parse(color, depth + 1)?, None),
                [first, second] => match parse(first, depth + 1) {
                    Some(color) => (color, Some(*second)),
                    None => (parse(second, depth + 1)?, Some(*first)),
                },
                _ => return None,
            };
            let weight = match weight {
                // A weight is a percentage. Lengths resolve the same way, so they are refused by their text.
                Some(text) => match units.math(text, 100., &[])? {
                    (weight, Kind::Length)
                        if text.contains('%') && (0. ..=100.).contains(&weight) =>
                    {
                        Some(weight)
                    }
                    _ => return None,
                },
                None => None,
            };
            mixed[index] = (convert(color.0, color.1, space), color.2, weight);
        }
        let [
            (mut first, mut first_alpha, p1),
            (mut second, mut second_alpha, p2),
        ] = mixed;
        let (p1, p2) = match (p1, p2) {
            (Some(p1), Some(p2)) => (p1, p2),
            (Some(p1), None) => (p1, 100. - p1),
            (None, Some(p2)) => (100. - p2, p2),
            (None, None) => (50., 50.),
        };
        let sum = p1 + p2;
        if sum <= 0. {
            return None;
        }
        let (p1, p2) = (p1 / sum, p2 / sum);
        for (a, b) in first
            .iter_mut()
            .chain([&mut first_alpha])
            .zip(second.iter_mut().chain([&mut second_alpha]))
        {
            if a.is_nan() {
                *a = *b;
            } else if b.is_nan() {
                *b = *a;
            }
        }
        if let Some(index) = hue {
            let (mut h1, mut h2) = (
                first[index].rem_euclid(360.),
                second[index].rem_euclid(360.),
            );
            let delta = h2 - h1;
            match method {
                0 if delta > 180. => h1 += 360.,
                0 if delta < -180. => h2 += 360.,
                1 if delta > 0. && delta < 180. => h1 += 360.,
                1 if delta > -180. && delta <= 0. => h2 += 360.,
                2 if delta < 0. => h2 += 360.,
                3 if delta > 0. => h1 += 360.,
                _ => {}
            }
            first[index] = h1;
            second[index] = h2;
        }
        let alpha = first_alpha * p1 + second_alpha * p2;
        let mut channels = [0.; 3];
        for index in 0..3 {
            channels[index] = if hue == Some(index) {
                first[index] * p1 + second[index] * p2
            } else {
                let value = first[index] * first_alpha * p1 + second[index] * second_alpha * p2;
                if alpha == 0. { value } else { value / alpha }
            };
        }
        return Some((space, channels, alpha * (sum / 100.).min(1.)));
    }
    let mut level = 0i32;
    let separator = |c: char| c.is_ascii_whitespace() || c == ',' || c == '/';
    let mut parts = body
        .split_inclusive(|c: char| {
            level += i32::from(c == '(') - i32::from(c == ')');
            level == 0 && separator(c)
        })
        .flat_map(|part| {
            let (value, separator) =
                part.split_at(part.len() - usize::from(part.ends_with(separator)));
            [value, separator.trim()]
        })
        .filter(|part| !part.is_empty())
        .peekable();
    let origin = if parts
        .peek()
        .is_some_and(|part| part.eq_ignore_ascii_case("from"))
    {
        parts.next();
        Some(parse(parts.next()?, depth + 1)?)
    } else {
        None
    };
    let space = match name {
        "rgb" | "rgba" => Space::Srgb,
        "hsla" => Space::Hsl,
        "color" => find(&SPACES[..9], parts.next()?)?,
        _ => find(&SPACES[9..], name)?,
    };
    let (references, keys) = match space {
        Space::Hsl => ([0., 100., 100.], ["h", "s", "l"]),
        Space::Hwb => ([0., 100., 100.], ["h", "w", "b"]),
        Space::Lab => ([100., 125., 125.], ["l", "a", "b"]),
        Space::Lch => ([100., 150., 0.], ["l", "c", "h"]),
        Space::Oklab => ([1., 0.4, 0.4], ["l", "a", "b"]),
        Space::Oklch => ([1., 0.4, 0.], ["l", "c", "h"]),
        Space::XyzD50 | Space::XyzD65 => ([1.; 3], ["x", "y", "z"]),
        _ => ([1.; 3], ["r", "g", "b"]),
    };
    let scale = if name.starts_with("rgb") { 255. } else { 1. };
    let mut items = [""; 7];
    let mut count = 0;
    for part in parts {
        *items.get_mut(count)? = part;
        count += 1;
    }
    let legacy =
        items[1] == "," && origin.is_none() && matches!(name, "rgb" | "rgba" | "hsl" | "hsla");
    let alpha = match (count, legacy) {
        (3, false) => None,
        (5, false) if items[3] == "/" => Some(items[4]),
        (5, true) if items[3] == "," => None,
        (7, true) if items[3] == "," && items[5] == "," => Some(items[6]),
        _ => return None,
    };
    let channels = if legacy {
        [items[0], items[2], items[4]]
    } else {
        [items[0], items[1], items[2]]
    };
    let mut vars = [("alpha", 1.); 4];
    if let Some((origin, values, alpha)) = origin {
        let values = convert(origin, values, space);
        for index in 0..3 {
            vars[index] = (keys[index], values[index] * scale);
        }
        vars[3].1 = alpha;
        for var in &mut vars {
            if var.1.is_nan() {
                var.1 = 0.;
            }
        }
    }
    let vars = if origin.is_some() { &vars[..] } else { &[] };
    // Percentages are lengths to the math parser, so a length like `5px` is told apart by scaling the base.
    let percentage = |text: &str, reference: f64, value: f64| {
        text.contains('%')
            && units
                .math(text, (reference * 2.) as f32, vars)
                .is_some_and(|(double, _)| {
                    (double - value * 2.).abs() <= 1e-6 * value.abs().max(1.)
                })
    };
    let mut values = [0.; 3];
    let mut percentages = [false; 3];
    for index in 0..3 {
        values[index] = if channels[index].eq_ignore_ascii_case("none") && !legacy {
            f64::NAN
        } else {
            let reference = references[index] * scale;
            match units.math(channels[index], reference as f32, vars)? {
                (value, Kind::Number | Kind::Angle) if reference == 0. => value,
                (value, Kind::Number) if reference != 0. => value / scale,
                (value, Kind::Length)
                    if reference != 0. && percentage(channels[index], reference, value) =>
                {
                    percentages[index] = true;
                    value / scale
                }
                _ => return None,
            }
        };
    }
    if legacy
        && match space {
            Space::Hsl => !percentages[1] || !percentages[2],
            _ => percentages[0] != percentages[1] || percentages[1] != percentages[2],
        }
    {
        return None;
    }
    if matches!(space, Space::Lab | Space::Lch | Space::Oklab | Space::Oklch) {
        values[0] = values[0].clamp(0., references[0]);
    }
    if matches!(space, Space::Lch | Space::Oklch) && values[1] < 0. {
        values[1] = 0.;
    }
    let alpha = match alpha {
        Some(alpha) if alpha.eq_ignore_ascii_case("none") && !legacy => f64::NAN,
        Some(alpha) => match units.math(alpha, 1., vars)? {
            (value, Kind::Number) => value.clamp(0., 1.),
            (value, Kind::Length) if percentage(alpha, 1., value) => value.clamp(0., 1.),
            _ => return None,
        },
        None => vars.get(3).map_or(1., |var| var.1),
    };
    Some((space, values, alpha))
}

fn convert(from: Space, values: [f64; 3], to: Space) -> [f64; 3] {
    match (from, to) {
        _ if from == to => values,
        // A round trip through XYZ moves channels like 25.5 below the rounding point.
        (Space::Hsl | Space::Hwb, Space::Srgb) => hsl(from, values),
        _ => from_xyz(to, to_xyz(from, values)),
    }
}

fn hsl(space: Space, values: [f64; 3]) -> [f64; 3] {
    let [a, b, c] = values.map(|v| if v.is_nan() { 0. } else { v });
    let (white, black, saturation, light) = if space == Space::Hwb {
        (b / 100., c / 100., 1., 0.5)
    } else {
        (0., 0., b / 100., c / 100.)
    };
    let hue = a.rem_euclid(360.);
    [0., 8., 4.].map(|n| {
        let k = (n + hue / 30.) % 12.;
        let value =
            light - saturation * light.min(1. - light) * (k - 3.).min(9. - k).clamp(-1., 1.);
        if white + black >= 1. {
            white / (white + black)
        } else {
            value * (1. - white - black) + white
        }
    })
}

fn to_xyz(space: Space, values: [f64; 3]) -> [f64; 3] {
    let [a, b, c] = values.map(|v| if v.is_nan() { 0. } else { v });
    let linear = |v: f64| {
        let abs = v.abs();
        if abs <= 0.04045 {
            v / 12.92
        } else {
            ((abs + 0.055) / 1.055).powf(2.4).copysign(v)
        }
    };
    let rectangular = |l: f64, chroma: f64, hue: f64| {
        let (sin, cos) = hue.to_radians().sin_cos();
        [l, chroma * cos, chroma * sin]
    };
    match space {
        Space::Srgb => matrix(SRGB_TO_XYZ, [a, b, c].map(linear)),
        Space::SrgbLinear => matrix(SRGB_TO_XYZ, [a, b, c]),
        Space::DisplayP3 => matrix(P3_TO_XYZ, [a, b, c].map(linear)),
        Space::A98 => matrix(
            A98_TO_XYZ,
            [a, b, c].map(|v| v.abs().powf(563. / 256.).copysign(v)),
        ),
        Space::ProPhoto => matrix(
            D50_TO_D65,
            matrix(
                PROPHOTO_TO_XYZ,
                [a, b, c].map(|v| {
                    if v.abs() <= 16. / 512. {
                        v / 16.
                    } else {
                        v.abs().powf(1.8).copysign(v)
                    }
                }),
            ),
        ),
        Space::Rec2020 => matrix(
            REC2020_TO_XYZ,
            [a, b, c].map(|v| {
                if v.abs() < REC2020_BETA * 4.5 {
                    v / 4.5
                } else {
                    ((v.abs() + REC2020_ALPHA - 1.) / REC2020_ALPHA)
                        .powf(1. / 0.45)
                        .copysign(v)
                }
            }),
        ),
        Space::XyzD65 => [a, b, c],
        Space::XyzD50 => matrix(D50_TO_D65, [a, b, c]),
        Space::Lab | Space::Lch => {
            let [l, a, b] = if space == Space::Lch {
                rectangular(a, b, c)
            } else {
                [a, b, c]
            };
            let y = (l + 16.) / 116.;
            let [x, z] = [y + a / 500., y - b / 200.].map(|v| {
                if v.powi(3) > EPSILON {
                    v.powi(3)
                } else {
                    (116. * v - 16.) / KAPPA
                }
            });
            let y = if l > KAPPA * EPSILON {
                y.powi(3)
            } else {
                l / KAPPA
            };
            matrix(D50_TO_D65, [x * D50[0], y, z * D50[2]])
        }
        Space::Oklab | Space::Oklch => {
            let lab = if space == Space::Oklch {
                rectangular(a, b, c)
            } else {
                [a, b, c]
            };
            matrix(LMS_TO_XYZ, matrix(OKLAB_TO_LMS, lab).map(|v| v.powi(3)))
        }
        Space::Hsl | Space::Hwb => to_xyz(Space::Srgb, hsl(space, [a, b, c])),
    }
}

fn from_xyz(space: Space, xyz: [f64; 3]) -> [f64; 3] {
    let gamma = |v: f64| {
        let abs = v.abs();
        if abs > 0.0031308 {
            (1.055 * abs.powf(1. / 2.4) - 0.055).copysign(v)
        } else {
            12.92 * v
        }
    };
    let polar = |[l, a, b]: [f64; 3], epsilon: f64| {
        let chroma = a.hypot(b);
        let hue = b.atan2(a).to_degrees().rem_euclid(360.);
        [l, chroma, if chroma < epsilon { f64::NAN } else { hue }]
    };
    match space {
        Space::Srgb => matrix(XYZ_TO_SRGB, xyz).map(gamma),
        Space::SrgbLinear => matrix(XYZ_TO_SRGB, xyz),
        Space::DisplayP3 => matrix(XYZ_TO_P3, xyz).map(gamma),
        Space::A98 => matrix(XYZ_TO_A98, xyz).map(|v| v.abs().powf(256. / 563.).copysign(v)),
        Space::ProPhoto => matrix(XYZ_TO_PROPHOTO, matrix(D65_TO_D50, xyz)).map(|v| {
            if v.abs() < 1. / 512. {
                v * 16.
            } else {
                v.abs().powf(1. / 1.8).copysign(v)
            }
        }),
        Space::Rec2020 => matrix(XYZ_TO_REC2020, xyz).map(|v| {
            if v.abs() > REC2020_BETA {
                (REC2020_ALPHA * v.abs().powf(0.45) - (REC2020_ALPHA - 1.)).copysign(v)
            } else {
                4.5 * v
            }
        }),
        Space::XyzD65 => xyz,
        Space::XyzD50 => matrix(D65_TO_D50, xyz),
        Space::Lab | Space::Lch => {
            let [x, y, z] = matrix(D65_TO_D50, xyz);
            let [x, y, z] = [x / D50[0], y, z / D50[2]].map(|v| {
                if v > EPSILON {
                    v.cbrt()
                } else {
                    (KAPPA * v + 16.) / 116.
                }
            });
            let lab = [116. * y - 16., 500. * (x - y), 200. * (y - z)];
            if space == Space::Lch {
                polar(lab, 0.0015)
            } else {
                lab
            }
        }
        Space::Oklab | Space::Oklch => {
            let lab = matrix(LMS_TO_OKLAB, matrix(XYZ_TO_LMS, xyz).map(f64::cbrt));
            if space == Space::Oklch {
                polar(lab, 0.000004)
            } else {
                lab
            }
        }
        Space::Hsl | Space::Hwb => {
            let [r, g, b] = from_xyz(Space::Srgb, xyz);
            let max = r.max(g).max(b);
            let min = r.min(g).min(b);
            let light = (min + max) / 2.;
            let delta = max - min;
            let mut hue = f64::NAN;
            let mut saturation = 0.;
            if delta > 1e-9 {
                saturation = if light <= 0. || light >= 1. {
                    0.
                } else {
                    (max - light) / light.min(1. - light)
                };
                hue = 60.
                    * if max == r {
                        (g - b) / delta + if g < b { 6. } else { 0. }
                    } else if max == g {
                        (b - r) / delta + 2.
                    } else {
                        (r - g) / delta + 4.
                    };
                if saturation < 0. {
                    hue += 180.;
                    saturation = -saturation;
                }
                hue = hue.rem_euclid(360.);
            }
            if space == Space::Hsl {
                [hue, saturation * 100., light * 100.]
            } else if min + 1. - max >= 1. - 1e-9 {
                [f64::NAN, min * 100., (1. - max) * 100.]
            } else {
                [hue, min * 100., (1. - max) * 100.]
            }
        }
    }
}

fn matrix(matrix: [[f64; 3]; 3], values: [f64; 3]) -> [f64; 3] {
    matrix.map(|row| row.into_iter().zip(values).map(|(a, b)| a * b).sum())
}

#[cfg(test)]
mod tests {
    use super::color;

    #[test]
    fn color_functions_spaces_and_keywords() {
        for (value, expected) in [
            ("#abc", Some((0xaabbcc, 255))),
            ("#ABCD", Some((0xaabbcc, 0xdd))),
            ("#aabbcc80", Some((0xaabbcc, 0x80))),
            ("#+12345", None),
            ("#12345", None),
            ("RED", Some((0xff0000, 255))),
            ("Transparent", Some((0, 0))),
            ("Canvas", Some((0xffffff, 255))),
            ("CanvasText", Some((0, 255))),
            ("LinkText", Some((0x0000ee, 255))),
            ("AccentColor", Some((0x0075ff, 255))),
            ("Mark", Some((0xffff00, 255))),
            ("RGB(1 2 3)", Some((0x010203, 255))),
            ("rgb(1, 2, 3, 50%)", Some((0x010203, 128))),
            ("rgba(0 128 255 / .5)", Some((0x0080ff, 128))),
            ("rgb(255 0 0 / none)", Some((0xff0000, 0))),
            ("rgb(none 255 0)", Some((0x00ff00, 255))),
            ("rgb(calc(255 / 2) 0 0)", Some((0x800000, 255))),
            ("rgb(1, 2 3)", None),
            ("rgb(none, 2, 3)", None),
            ("rgb(1 2 3) x)", None),
            ("rgb(1 2)", None),
            ("rgb(1 2 3 4)", None),
            ("hsl(120 100% 50%)", Some((0x00ff00, 255))),
            ("hsl(120, 100%, 25%)", Some((0x008000, 255))),
            ("hsl(164.31436498259058 80% 50%)", Some((0x1ae6b0, 255))),
            ("HSLA(240deg 100% 50% / 0.5)", Some((0x0000ff, 128))),
            ("hsl(0.5turn 100% 50%)", Some((0x00ffff, 255))),
            ("hsl(3.14159265rad 100 50)", Some((0x00ffff, 255))),
            ("hsl(200grad 100% 50%)", Some((0x00ffff, 255))),
            ("hsl(-120 100% 50%)", Some((0x0000ff, 255))),
            ("hsl(none 0% 50%)", Some((0x808080, 255))),
            ("hsl(120 100% 50% / 25%)", Some((0x00ff00, 64))),
            ("hsl(120% 100% 50%)", None),
            ("hwb(0 0% 0%)", Some((0xff0000, 255))),
            ("hwb(120 20% 30%)", Some((0x33b333, 255))),
            ("hwb(0 60% 60%)", Some((0x808080, 255))),
            ("lab(54.29 80.8 69.89)", Some((0xff0000, 255))),
            ("lab(54.29% 64.64% 55.912%)", Some((0xff0000, 255))),
            ("lch(54.29 106.84 40.85)", Some((0xff0000, 255))),
            ("oklab(0.62796 0.22486 0.12585)", Some((0xff0000, 255))),
            ("oklab(62.796% 56.215% 31.4625%)", Some((0xff0000, 255))),
            ("oklch(0.62796 0.25768 29.2339)", Some((0xff0000, 255))),
            ("oklch(0.7 none 120)", Some((0x9e9e9e, 255))),
            ("color(srgb 1 0 0)", Some((0xff0000, 255))),
            ("color(srgb 100% 0% 0% / 50%)", Some((0xff0000, 128))),
            ("color(srgb-linear 0.5 0.5 0.5)", Some((0xbcbcbc, 255))),
            ("color(display-p3 1 0 0)", Some((0xff0000, 255))),
            ("color(xyz-d65 0.4124 0.2126 0.0193)", Some((0xff0000, 255))),
            ("color(xyz-d50 0.4360 0.2225 0.0139)", Some((0xff0000, 255))),
            ("color(xyz 0.9505 1 1.089)", Some((0xffffff, 255))),
            ("color(rec2020 1 0 0)", Some((0xff0000, 255))),
            ("color(a98-rgb 1 0 0)", Some((0xff0000, 255))),
            ("color(prophoto-rgb .5 .5 .5)", Some((0x929292, 255))),
            ("rgb(from red r g b / 50%)", Some((0xff0000, 128))),
            ("rgb(from #d52c36 r g b / .15)", Some((0xd52c36, 38))),
            (
                "rgb(from rgb(10 20 30 / 40%) calc(r * 2) g b)",
                Some((0x14141e, 102)),
            ),
            ("rgb(from red b g r)", Some((0x0000ff, 255))),
            ("hsl(from red calc(h + 120) s l)", Some((0x00ff00, 255))),
            ("hsl(from white h s l)", Some((0xffffff, 255))),
            (
                "hwb(from red h w b / calc(alpha / 2))",
                Some((0xff0000, 128)),
            ),
            ("lab(from white l 0 0)", Some((0xffffff, 255))),
            ("oklch(from red l c h)", Some((0xff0000, 255))),
            ("oklch(from red 0 0 0)", Some((0, 255))),
            ("color(from red srgb r g b)", Some((0xff0000, 255))),
            ("rgb(from red r g)", None),
            ("rgb(from red q g b)", None),
            ("color-mix(in srgb, red, blue)", Some((0x800080, 255))),
            ("color-mix(in srgb, red 25%, blue)", Some((0x4000bf, 255))),
            ("color-mix(in srgb, 25% red, blue)", Some((0x4000bf, 255))),
            (
                "color-mix(in srgb, red 30%, blue 30%)",
                Some((0x800080, 153)),
            ),
            (
                "color-mix(in srgb, red 40%, transparent)",
                Some((0xff0000, 102)),
            ),
            ("color-mix(in srgb, red 0%, blue 0%)", None),
            ("color-mix(in srgb, red 50px, blue)", None),
            (
                "color-mix(in srgb, red calc(20% + 5%), blue)",
                Some((0x4000bf, 255)),
            ),
            ("color-mix(in srgb, red 120%, blue)", None),
            (
                "color-mix(in srgb-linear, red, blue)",
                Some((0xbc00bc, 255)),
            ),
            ("color-mix(in xyz, red, blue)", Some((0xbc00bc, 255))),
            ("color-mix(in lab, white 50%, black)", Some((0x777777, 255))),
            ("color-mix(in oklab, red, blue)", Some((0x8c53a2, 255))),
            ("color-mix(red, blue)", Some((0x8c53a2, 255))),
            ("color-mix(in oklch, red, blue)", Some((0xba00c2, 255))),
            (
                "color-mix(in oklch longer hue, red, blue)",
                Some((0x009300, 255)),
            ),
            ("color-mix(in hsl, red, blue)", Some((0xff00ff, 255))),
            (
                "color-mix(in hsl increasing hue, blue, red)",
                Some((0xff00ff, 255)),
            ),
            (
                "color-mix(in hsl decreasing hue, blue, red)",
                Some((0x00ff00, 255)),
            ),
            ("color-mix(in hwb, red, lime)", Some((0xffff00, 255))),
            ("color-mix(in lch, white, blue)", Some((0xaf89ff, 255))),
            (
                "color-mix(in hsl, hsl(none 100% 50%), blue)",
                Some((0x0000ff, 255)),
            ),
            (
                "color-mix(in srgb, color-mix(in srgb, red, blue), white)",
                Some((0xbf80bf, 255)),
            ),
            ("color-mix(in srgb longer hue, red, blue)", None),
            ("color-mix(in srgb, red)", None),
            ("currentColor", None),
            ("1px", None),
        ] {
            assert_eq!(color(value), expected, "{value}");
        }
        let nested = format!(
            "{}red{}",
            "color-mix(in srgb, ".repeat(64),
            ", blue)".repeat(64)
        );
        assert_eq!(color(&nested), None);
    }
}
