//! The image functions `image-set()`, `image()` and `-webkit-cross-fade()`.
use crate::css::value::{tokens::components, units::Kind, units::Lengths};
use std::borrow::Cow;

fn body<'a>(value: &'a str, names: &[&str]) -> Option<&'a str> {
    let (name, body) = value.strip_suffix(')')?.split_once('(')?;
    (names.iter().any(|known| name.eq_ignore_ascii_case(known))).then_some(body)
}

/// A resolution in `dppx`.
pub(crate) fn resolution(value: &str) -> Option<f64> {
    // Math knows no resolutions, so they go through it as lengths.
    let text = value.to_ascii_lowercase().replace("dppx", "x");
    if text.contains("px") || !text.contains('x') && !text.contains("dp") {
        return None;
    }
    let text = text
        .replace('x', "px")
        .replace("dpi", "px / 96")
        .replace("dpcm", "px * 2.54 / 96");
    let units = Lengths::new(16., 0., 0.);
    let (number, kind) = units.math(&format!("calc({text})"), f32::NAN, &[])?;
    (kind == Kind::Length && (value.ends_with(')') || number >= 0.)).then_some(number.max(0.))
}

// The options of an `image-set()`: the image, the resolution and whether the runtime has its type.
fn options(body: &str) -> Option<Vec<(Cow<'_, str>, f64, bool)>> {
    if body.trim_end().ends_with(',') {
        return None;
    }
    let mut out = Vec::new();
    for option in components(body, ',') {
        let parts = components(option, ' ');
        let (&image, rest) = parts.split_first()?;
        let quoted = image.starts_with(['"', '\'']);
        if !quoted
            && (image.eq_ignore_ascii_case("none")
                || body_is_set(image)
                || !super::grammar::syntax_matches("<image>", image))
        {
            return None;
        }
        let (mut density, mut kind) = (None, None);
        for part in rest {
            if let Some(value) = resolution(part).filter(|_| density.is_none()) {
                density = Some(value);
            } else if let Some(name) = crate::css::value::image::body(part, &["type"])
                && kind.is_none()
                && name.trim().starts_with(['"', '\''])
            {
                let name = name.trim().trim_matches(['"', '\'']);
                kind = Some(matches!(
                    &*name.to_ascii_lowercase(),
                    "image/png"
                        | "image/jpeg"
                        | "image/jpg"
                        | "image/gif"
                        | "image/webp"
                        | "image/svg+xml"
                        | "image/bmp"
                        | "image/avif"
                        | "image/x-icon"
                        | "image/vnd.microsoft.icon"
                ));
            } else {
                return None;
            }
        }
        let image = match quoted {
            true => Cow::Owned(format!("url({image})")),
            false => Cow::Borrowed(image),
        };
        out.push((image, density.unwrap_or(1.), kind.unwrap_or(true)));
    }
    (!out.is_empty()).then_some(out)
}

const SETS: [&str; 2] = ["image-set", "-webkit-image-set"];

fn body_is_set(value: &str) -> bool {
    body(value, &SETS).is_some()
}

/// Whether the arguments of an image function are valid, or None when it is another function.
pub(crate) fn valid(value: &str) -> Option<bool> {
    if let Some(body) = body(value, &SETS) {
        return Some(options(body).is_some());
    }
    if let Some(body) = body(value, &["image"]) {
        return Some(super::grammar::syntax_matches("<color>", body.trim()));
    }
    if body(value, &["-webkit-cross-fade"]).is_some() {
        return Some(cross_fade(value).is_some());
    }
    None
}

/// The two images of a `-webkit-cross-fade()` and the share of the second.
pub(crate) fn cross_fade(value: &str) -> Option<([&str; 2], f32)> {
    let parts = components(body(value, &["-webkit-cross-fade"])?, ',');
    let [a, b, share] = parts[..] else {
        return None;
    };
    let image = |part: &str| {
        part.eq_ignore_ascii_case("none") || super::grammar::syntax_matches("<image>", part)
    };
    let units = Lengths::new(16., 0., 0.);
    let share = match share.strip_suffix('%') {
        Some(percent) => percent.parse::<f64>().ok()? / 100.,
        None => {
            units
                .math(share, f32::NAN, &[])
                .filter(|(_, kind)| *kind == Kind::Number)?
                .0
        }
    };
    (image(a) && image(b)).then_some(([a, b], share.clamp(0., 1.) as f32))
}

/// The image that an `image-set()` or `image()` shows on a screen of `scale`, and its resolution.
pub(crate) fn shown(value: &str, scale: f32) -> (Cow<'_, str>, f32) {
    if let Some(body) = body(value, &["image"]) {
        return (Cow::Owned(format!("linear-gradient({body} 0 0)")), 1.);
    }
    let Some(options) = body(value, &SETS).and_then(options) else {
        return (Cow::Borrowed(value), 1.);
    };
    // The lowest resolution that is enough, or else the highest one. The first of equals wins.
    let best = options
        .into_iter()
        .filter(|option| option.2)
        .reduce(|best, option| {
            let enough = |option: &(Cow<str>, f64, bool)| option.1 >= f64::from(scale);
            let better = match (enough(&best), enough(&option)) {
                (false, _) => option.1 > best.1,
                (true, true) => option.1 < best.1,
                (true, false) => false,
            };
            if better { option } else { best }
        });
    match best {
        Some((image, density, _)) => match image {
            Cow::Borrowed(image) if image.starts_with("image(") => {
                (shown(image, scale).0, density as f32)
            }
            image => (image, density as f32),
        },
        None => (Cow::Borrowed("none"), 1.),
    }
}

#[test]
fn image_sets_pick_the_best_resolution() {
    let set = "image-set(url(a.png) 1x, \"b.png\" 2dppx, url(c.avif) type('image/nope') 3x, linear-gradient(red, blue) 288dpi)";
    assert_eq!(shown(set, 1.), ("url(a.png)".into(), 1.));
    assert_eq!(shown(set, 1.5), ("url(\"b.png\")".into(), 2.));
    assert_eq!(shown(set, 4.), ("linear-gradient(red, blue)".into(), 3.));
    assert_eq!(shown("url(a.png)", 2.), ("url(a.png)".into(), 1.));
    assert_eq!(shown("image(red)", 2.0).0, "linear-gradient(red 0 0)");
    assert_eq!(resolution("96dpi"), Some(1.));
    assert_eq!(resolution("calc(1x*2 + 48dpi)"), Some(2.5));
    for value in ["-1x", "1", "1px", "x", "1em"] {
        assert_eq!(resolution(value), None, "{value}");
    }
    for value in [
        "image-set(url(a.png) 1x,)",
        "image-set(none 1x)",
        "image-set(red 1x)",
        "image-set(url(a.png) -1x)",
        "image-set(url(a.png) 1x 2x)",
        "image-set(image-set(url(a.png) 1x))",
        "image-set()",
        "image(1px)",
        "-webkit-cross-fade(url(a.png), url(b.png))",
        "-webkit-cross-fade(red, url(b.png), 50%)",
    ] {
        assert_eq!(valid(value), Some(false), "{value}");
    }
    assert_eq!(
        cross_fade("-webkit-cross-fade(none, url(b.png), 150%)"),
        Some((["none", "url(b.png)"], 1.))
    );
    assert_eq!(valid("url(a.png)"), None);
}
