pub fn color(value: &str) -> Option<(u32, u8)> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        let hex = if hex.len() == 3 || hex.len() == 4 {
            hex.chars().flat_map(|c| [c, c]).collect::<String>()
        } else {
            hex.to_owned()
        };
        let number = u32::from_str_radix(&hex, 16).ok()?;
        return match hex.len() {
            6 => Some((number, 255)),
            8 => Some((number >> 8, number as u8)),
            _ => None,
        };
    }
    if let Some(body) = value
        .strip_prefix("color-mix(")
        .and_then(|v| v.strip_suffix(')'))
    {
        let parts = super::components(body, ',');
        if parts.len() != 3 {
            return None;
        }
        let space = parts[0].trim().strip_prefix("in ")?;
        if !matches!(space, "lab" | "srgb" | "srgb-linear") {
            return None;
        }
        let mut colors = [[0.; 3]; 2];
        let mut alphas = [0.; 2];
        let mut weights = [None; 2];
        for index in 0..2 {
            let components = super::components(parts[index + 1].trim(), ' ');
            let (rgb, alpha) = color(components[0])?;
            colors[index] = [rgb >> 16, (rgb >> 8) & 255, rgb & 255].map(|v| v as f64 / 255.);
            alphas[index] = alpha as f64 / 255.;
            weights[index] = components
                .get(1)
                .and_then(|v| v.strip_suffix('%'))
                .and_then(|v| v.parse::<f64>().ok())
                .map(|v| v / 100.);
            if space != "srgb" {
                colors[index] = colors[index].map(|v| {
                    if v <= 0.04045 {
                        v / 12.92
                    } else {
                        ((v + 0.055) / 1.055).powf(2.4)
                    }
                });
                if space == "lab" {
                    let xyz = matrix(
                        [
                            [0.436065742824811, 0.3851514688337912, 0.14307845442264197],
                            [0.22249319175623702, 0.7168870538238823, 0.06061979053616537],
                            [
                                0.013923904500943465,
                                0.09708128566574634,
                                0.7140993584005155,
                            ],
                        ],
                        colors[index],
                    );
                    let [x, y, z] = [
                        xyz[0] / 0.9642956764295677,
                        xyz[1],
                        xyz[2] / 0.8251046025104601,
                    ]
                    .map(|v| {
                        if v > 216. / 24389. {
                            v.cbrt()
                        } else {
                            ((24389. / 27.) * v + 16.) / 116.
                        }
                    });
                    colors[index] = [116. * y - 16., 500. * (x - y), 200. * (y - z)];
                }
            }
        }
        let weights = [
            weights[0].unwrap_or_else(|| 1. - weights[1].unwrap_or(0.5)),
            weights[1].unwrap_or_else(|| 1. - weights[0].unwrap_or(0.5)),
        ];
        let sum = weights[0] + weights[1];
        if sum <= 0. || weights.iter().any(|v| !(0. ..=1.).contains(v)) {
            return None;
        }
        let alpha = (alphas[0] * weights[0] + alphas[1] * weights[1]) / sum;
        let mut mixed = [0.; 3];
        if alpha > 0. {
            for index in 0..3 {
                mixed[index] = (colors[0][index] * alphas[0] * weights[0]
                    + colors[1][index] * alphas[1] * weights[1])
                    / (sum * alpha);
            }
        }
        if space == "lab" {
            let y = (mixed[0] + 16.) / 116.;
            let mut xyz = [y + mixed[1] / 500., y, y - mixed[2] / 200.].map(|v| {
                if v.powi(3) > 216. / 24389. {
                    v.powi(3)
                } else {
                    (116. * v - 16.) / (24389. / 27.)
                }
            });
            xyz[0] *= 0.9642956764295677;
            xyz[2] *= 0.8251046025104601;
            mixed = matrix(
                [
                    [3.1341359569958707, -1.6173863321612538, -0.4906619460083532],
                    [-0.978795502912089, 1.916254567259524, 0.03344273116131949],
                    [0.07195537988411677, -0.2289768264158322, 1.405386058324125],
                ],
                xyz,
            );
        }
        if space != "srgb" {
            mixed = mixed.map(|v| {
                if v <= 0.0031308 {
                    12.92 * v
                } else {
                    1.055 * v.powf(1. / 2.4) - 0.055
                }
            });
        }
        let [r, g, b] = mixed.map(|v| (v.clamp(0., 1.) * 255.).round() as u32);
        return Some((
            (r << 16) | (g << 8) | b,
            (alpha * sum.min(1.) * 255.).round() as u8,
        ));
    }
    if let Some(body) = value
        .strip_prefix("rgb(from ")
        .and_then(|v| v.strip_suffix(')'))
    {
        let parts = super::components(body, ' ');
        if parts.len() == 6 && parts[1..5] == ["r", "g", "b", "/"] {
            let (rgb, _) = color(parts[0])?;
            let alpha = parts[5].parse::<f64>().ok()?;
            return Some((rgb, (alpha.clamp(0., 1.) * 255.).round() as u8));
        }
        return None;
    }
    if let Some((space, body)) = value.split_once('(') {
        let body = body.strip_suffix(')')?;
        let parts: Vec<_> = body
            .split(|c: char| c.is_ascii_whitespace() || c == ',' || c == '/')
            .filter(|s| !s.is_empty())
            .collect();
        if !(3..=4).contains(&parts.len()) {
            return None;
        }
        let number = |value: &str, percent: f64| -> Option<f64> {
            let value = if let Some(value) = value.strip_suffix('%') {
                value.parse::<f64>().ok()? * percent / 100.
            } else {
                value.parse().ok()?
            };
            value.is_finite().then_some(value)
        };
        let alpha = number(parts.get(3).copied().unwrap_or("1"), 1.)?.clamp(0., 1.);
        let rgb = match space {
            "rgb" | "rgba" => [
                number(parts[0], 255.)? / 255.,
                number(parts[1], 255.)? / 255.,
                number(parts[2], 255.)? / 255.,
            ],
            "oklch" => {
                let lightness = number(parts[0], 1.)?.clamp(0., 1.);
                let chroma = number(parts[1], 0.4)?.max(0.);
                let hue = if let Some(value) = parts[2].strip_suffix("turn") {
                    number(value, 1.)? * std::f64::consts::TAU
                } else if let Some(value) = parts[2].strip_suffix("grad") {
                    number(value, 1.)? * std::f64::consts::PI / 200.
                } else if let Some(value) = parts[2].strip_suffix("rad") {
                    number(value, 1.)?
                } else {
                    number(parts[2].strip_suffix("deg").unwrap_or(parts[2]), 1.)?.to_radians()
                };
                let a = chroma * hue.cos();
                let b = chroma * hue.sin();
                let l = (lightness + 0.3963377774 * a + 0.2158037573 * b).powi(3);
                let m = (lightness - 0.1055613458 * a - 0.0638541728 * b).powi(3);
                let s = (lightness - 0.0894841775 * a - 1.2914855480 * b).powi(3);
                [
                    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
                    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
                    -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
                ]
                .map(|v| {
                    if v <= 0.0031308 {
                        12.92 * v
                    } else {
                        1.055 * v.powf(1. / 2.4) - 0.055
                    }
                })
            }
            _ => return None,
        };
        let [r, g, b] = rgb.map(|v| (v.clamp(0., 1.) * 255.).round() as u32);
        return Some(((r << 16) | (g << 8) | b, (alpha * 255.).round() as u8));
    }
    if value.eq_ignore_ascii_case("transparent") {
        return Some((0, 0));
    }
    let (r, g, b) = cssparser::color::parse_named_color(value).ok()?;
    Some(((r as u32) << 16 | (g as u32) << 8 | b as u32, 255))
}

fn matrix(matrix: [[f64; 3]; 3], values: [f64; 3]) -> [f64; 3] {
    matrix.map(|row| row.into_iter().zip(values).map(|(a, b)| a * b).sum())
}
