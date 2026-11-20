use crate::style::{self, Prop};
use renderer::Child;
use std::{fmt::Write, rc::Rc};

#[derive(Clone, Copy, PartialEq, Default)]
pub(super) enum Control {
    Button,
    #[default]
    Field,
    Menu,
    Check,
    Radio,
    Range,
    Progress,
    Meter,
    Color,
    Disclosure,
    Resizer,
}

/// The control, whether it draws its native box, and whether it has `appearance: none` and draws its parts
/// from pseudo-element styles, as ranges and progress bars do.
pub(super) fn kind(
    tag: &str,
    input: Option<&str>,
    list: bool,
    style: &style::Style,
) -> Option<(Control, bool, bool)> {
    let plain = [Prop::APPEARANCE, Prop::WEBKIT_APPEARANCE]
        .into_iter()
        .any(|prop| style.get(prop).is_some_and(|value| value == "none"));
    let input = input.map(str::to_ascii_lowercase);
    let control = match (tag, input.as_deref()) {
        ("button", _) | (_, Some("button" | "submit" | "reset")) => Control::Button,
        ("select", _) if list => Control::Field,
        ("select", _) => Control::Menu,
        ("textarea", _) => Control::Field,
        ("progress", _) => Control::Progress,
        ("meter", _) => Control::Meter,
        (_, Some("checkbox")) => Control::Check,
        (_, Some("radio")) => Control::Radio,
        (_, Some("range")) => Control::Range,
        (_, Some("color")) => Control::Color,
        (_, Some("hidden" | "file" | "image")) => return None,
        ("input", _) => Control::Field,
        _ => return None,
    };
    if plain {
        return matches!(control, Control::Range | Control::Progress)
            .then_some((control, false, true));
    }
    let (border, backgrounds): (&str, &[u32]) = match control {
        Control::Button | Control::Color => (
            if control == Control::Color {
                "solid"
            } else {
                "outset"
            },
            &[0xefefef, 0x6b6b6b, 0x130101],
        ),
        Control::Field if tag == "input" => ("inset", &[0xffffff, 0x3b3b3b, 0xefefef]),
        Control::Field | Control::Menu => ("solid", &[0xffffff, 0x3b3b3b, 0xefefef]),
        _ => return Some((control, false, false)),
    };
    let edges: &[u32] = if matches!(control, Control::Button | Control::Color) {
        &[0x000000, 0xffffff, 0x767676, 0xc3c3c3]
    } else {
        &[0x767676, 0x858585]
    };
    let boxed = style::BORDERS.iter().all(|[_, kind, color]| {
        style.get(*kind) == Some(border)
            && style
                .get(*color)
                .and_then(style::color)
                .is_some_and(|(ink, _)| edges.contains(&ink))
    }) && [
        Prop::BORDER_TOP_LEFT_RADIUS,
        Prop::BORDER_TOP_RIGHT_RADIUS,
        Prop::BORDER_BOTTOM_RIGHT_RADIUS,
        Prop::BORDER_BOTTOM_LEFT_RADIUS,
    ]
    .into_iter()
    .all(|corner| {
        style
            .get(corner)
            .is_none_or(|value| matches!(value, "0" | "0px"))
    }) && style
        .get(Prop::BACKGROUND_COLOR)
        .and_then(style::color)
        .is_some_and(|(ink, _)| backgrounds.contains(&ink))
        && style
            .get(Prop::BACKGROUND_IMAGE)
            .is_none_or(|value| value == "none");
    (boxed || control == Control::Menu).then_some((control, boxed, false))
}

#[derive(Default)]
pub(super) struct Look {
    pub control: Control,
    pub boxed: bool,
    pub plain: bool,
    pub checked: bool,
    pub disabled: bool,
    pub hover: bool,
    pub active: bool,
    pub dark: bool,
    pub accent: u32,
    pub ink: u32,
    pub value: f32,
}

pub(super) fn layer(id: usize, look: Look, view: Rc<super::PageState>) -> Child {
    let drawn = std::cell::Cell::new([0u32; 3]);
    renderer::canvas(
        |_, _| (),
        move |bounds, _, window| {
            let [width, height] = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
            if width <= 0. || height <= 0. {
                return;
            }
            let scale = window.scale_factor();
            let mut host = view.host.borrow_mut();
            let backgrounds = &mut host.dom.entry(view.id).or_default().backgrounds;
            let key = (id, usize::MAX - look.control as usize);
            let seen = [width, height, scale].map(f32::to_bits);
            if drawn.replace(seen) != seen || !backgrounds.contains_key(&key) {
                let Some(source) = svg(&look, width, height) else {
                    return;
                };
                let limit = super::canvas::RASTER_MAX as f32;
                let [rw, rh] =
                    [width, height].map(|side| (side * scale).ceil().clamp(1., limit) as u32);
                if backgrounds.get(&key).is_none_or(|(old, image)| {
                    old != &source
                        || [image.size().width, image.size().height].map(|side| side as u32)
                            != [rw, rh]
                }) && let Ok(image) = super::canvas::svg(&source, rw, rh)
                    && let Some((_, old)) = backgrounds.insert(key, (source, image))
                    && std::sync::Arc::strong_count(&old) == 1
                {
                    window.release_image(old);
                }
            }
            if let Some((_, image)) = backgrounds.get(&key) {
                window.draw_image(
                    bounds,
                    bounds,
                    renderer::Radii::default(),
                    image.clone(),
                    false,
                    false,
                );
            }
        },
    )
    .absolute()
    .left_0()
    .top_0()
    .size_full()
    .into()
}

fn svg(look: &Look, w: f32, h: f32) -> Option<String> {
    let hex = |color: u32| format!("#{color:06x}");
    let solid =
        look.disabled && look.dark && matches!(look.control, Control::Check | Control::Radio);
    let edge = if solid {
        0x626262
    } else {
        box_colors(look)
            .0
            .unwrap_or(if look.dark { 0x858585 } else { 0x767676 })
    };
    let accent = if solid {
        0x757575
    } else if (look.hover || look.active) && !look.disabled {
        // Like Chrome, hover moves the HSL lightness away from the field and a press toward it.
        let by = if look.dark == look.active {
            -0.11
        } else {
            0.11
        };
        lighten(look.accent, by)
    } else {
        look.accent
    };
    let field = hex(if look.dark { 0x3b3b3b } else { 0xffffff });
    let mut body = String::new();
    match look.control {
        Control::Check | Control::Radio if look.checked => {
            let s = w.min(h);
            let [x, y] = [(w - s) / 2., (h - s) / 2.];
            if look.control == Control::Radio {
                let [cx, cy] = [x + s / 2., y + s / 2.];
                write!(
                    body,
                    r#"<circle cx="{cx}" cy="{cy}" r="{}" fill="{field}" stroke="{}"/><circle cx="{cx}" cy="{cy}" r="{}" fill="{}"/>"#,
                    s / 2. - 0.5,
                    hex(accent),
                    s * 0.3,
                    hex(accent),
                )
            } else {
                write!(
                    body,
                    r#"<rect x="{x}" y="{y}" width="{s}" height="{s}" rx="2" fill="{}"/><path d="M{} {}l{} {}L{} {}" fill="none" stroke="{field}" stroke-width="{}"/>"#,
                    hex(accent),
                    x + s * 0.2,
                    y + s * 0.5,
                    s * 0.2,
                    s * 0.2,
                    x + s * 0.8,
                    y + s * 0.2,
                    s * 0.16,
                )
            }
            .ok()?;
        }
        Control::Check | Control::Radio => {
            let s = w.min(h);
            let radius = if look.control == Control::Radio {
                s / 2.
            } else {
                2.
            };
            write!(
                body,
                r#"<rect x="{}" y="{}" width="{}" height="{}" rx="{}" fill="{field}" stroke="{}"/>"#,
                (w - s) / 2. + 0.5,
                (h - s) / 2. + 0.5,
                s - 1.,
                s - 1.,
                radius - 0.5,
                hex(edge),
            )
            .ok()?;
        }
        Control::Range | Control::Progress | Control::Meter => {
            let bar = h.min(8.);
            let thumb = h.min(16.);
            let x = if look.control == Control::Range {
                1.
            } else {
                0.
            };
            let y = ((h - bar) / 2. * 2.).floor() / 2.;
            let length = w - 2. * x;
            let meter = look.control == Control::Meter;
            // Like Chrome, a range and a progress bar change their edges and light track on hover and press.
            let stateful =
                matches!(look.control, Control::Range | Control::Progress) && !look.disabled;
            let state_edge = (box_colors(look).0.filter(|_| stateful)).unwrap_or(if look.dark {
                0x858585
            } else {
                0x767676
            });
            // A light edge is half-transparent: `up` rounds the mix up.
            let mix = |a: u32, b: u32, up: u32| {
                let channel =
                    |shift: u32| (((a >> shift & 0xff) + (b >> shift & 0xff) + up) / 2) << shift;
                channel(16) | channel(8) | channel(0)
            };
            let value_edge = if look.dark {
                state_edge
            } else {
                mix(accent, state_edge, 1)
            };
            let track = match (look.dark, stateful && look.active, stateful && look.hover) {
                (true, ..) => 0x3b3b3b,
                _ if meter => 0xefefef,
                (_, true, _) => 0xf5f5f5,
                (_, _, true) => 0xe5e5e5,
                _ => 0xefefef,
            };
            let edge = match look.dark {
                true => state_edge,
                _ if meter => 0xcbcbcb,
                _ => mix(state_edge, track, 0),
            };
            let [track, edge] = [track, edge].map(hex);
            write!(
                body,
                r#"<rect x="{}" y="{}" width="{}" height="{}" rx="{}" fill="{track}" stroke="{edge}"/>"#,
                x + 0.5,
                y + 0.5,
                length - 1.,
                bar - 1.,
                bar / 2. - 0.5,
            )
            .ok()?;
            if look.value >= 0. {
                let end = match look.control {
                    Control::Range => thumb / 2. + (w - thumb) * look.value,
                    Control::Meter => x + 1. + (length - 2.) * look.value,
                    _ => x + length * look.value,
                };
                write!(
                    body,
                    r#"<clipPath id="v"><rect x="{x}" y="{y}" width="{}" height="{bar}"/></clipPath><g clip-path="url(#v)">"#,
                    (end - x).max(0.),
                )
                .ok()?;
                if !meter {
                    write!(
                        body,
                        r#"<rect x="{x}" y="{y}" width="{length}" height="{bar}" rx="{}" fill="{}"/>"#,
                        bar / 2.,
                        hex(value_edge),
                    )
                    .ok()?;
                }
                write!(
                    body,
                    r#"<rect x="{}" y="{}" width="{}" height="{}" rx="{}" fill="{}"/></g>"#,
                    x + 1.,
                    y + 1.,
                    length - 2.,
                    bar - 2.,
                    bar / 2. - 1.,
                    hex(if meter { look.accent } else { accent }),
                )
                .ok()?;
                if look.control == Control::Range {
                    write!(
                        body,
                        r#"<circle cx="{end}" cy="{}" r="{}" fill="{}"/>"#,
                        h / 2.,
                        thumb / 2.,
                        // Chrome hovers the thumb only under the pointer, which the look doesn't know.
                        hex(if look.active { accent } else { look.accent }),
                    )
                    .ok()?;
                }
            }
        }
        Control::Menu => {
            write!(
                body,
                r#"<path d="M{} {}l3.5 3.5l3.5 -3.5" fill="none" stroke="{}" stroke-width="1.5"/>"#,
                w - 9.5,
                h / 2. - 1.75,
                hex(look.ink),
            )
            .ok()?;
        }
        Control::Color => {
            write!(
                body,
                r##"<rect x="4.5" y="5.5" width="{}" height="{}" fill="{}" stroke="#777"/>"##,
                (w - 9.).max(0.),
                (h - 11.).max(0.),
                hex(look.accent),
            )
            .ok()?;
        }
        Control::Disclosure => {
            let points = if look.checked {
                [[0., 0.07], [0.5, 0.93], [1., 0.07]]
            } else {
                [[0., 0.], [0.86, 0.5], [0., 1.]]
            };
            let [[ax, ay], [bx, by], [cx, cy]] = points.map(|[x, y]| [x * w, y * h]);
            write!(
                body,
                r#"<path d="M{ax} {ay}L{bx} {by}L{cx} {cy}Z" fill="{}"/>"#,
                hex(look.ink)
            )
            .ok()?;
        }
        Control::Resizer => {
            for (color, drop) in [("#000", 0.), ("#fff", 0.5)] {
                write!(
                    body,
                    r#"<path d="M{} {}L{} {}M{} {}L{} {}" stroke="{color}" stroke-opacity=".6" stroke-width="1"/>"#,
                    w - 1.25,
                    h - 8.25 + drop,
                    w - 7.75,
                    h - 1.75 + drop,
                    w - 1.25,
                    h - 4.25 + drop,
                    w - 3.75,
                    h - 1.75 + drop,
                )
                .ok()?;
            }
        }
        Control::Button | Control::Field => return None,
    }
    let opacity = if look.disabled && !solid {
        r#" opacity=".3""#
    } else {
        ""
    };
    Some(format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}"><g{opacity}>{body}</g></svg>"#
    ))
}

/// Like Chrome, a checked box, a range or a progress bar flips its color scheme when its `accent-color` is hard
/// to see on it.
pub(super) fn accent_dark(control: Control, checked: bool, accent: u32, dark: bool) -> bool {
    let light = match control {
        Control::Range | Control::Progress => 0xefefef,
        Control::Check | Control::Radio if checked => 0xffffff,
        _ => return dark,
    };
    let luminance = |color: u32| {
        let [r, g, b] = [16, 8, 0].map(|shift| {
            let c = (color >> shift & 0xff) as f32 / 255.;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        });
        0.2126 * r + 0.7152 * g + 0.0722 * b + 0.05
    };
    let [accent, light, dark_field] = [accent, light, 0x3b3b3b].map(luminance);
    let contrast = |other: f32| accent.max(other) / accent.min(other);
    let [with_light, with_dark] = [contrast(light), contrast(dark_field)];
    if dark {
        !(with_dark < 3. && with_dark < with_light)
    } else {
        with_light < 3. && with_light < with_dark
    }
}

fn lighten(color: u32, by: f32) -> u32 {
    let [r, g, b] = [16, 8, 0].map(|shift| (color >> shift & 0xff) as f32 / 255.);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let light = (max + min) / 2.;
    let to = (light + by).clamp(0., 1.);
    // Keep hue and saturation: each channel keeps its place between the gray of the lightness and the extreme.
    let [r, g, b] = [r, g, b].map(|c| {
        let span = |l: f32| if l <= 0.5 { l } else { 1. - l };
        let value = match span(light) {
            s if s > 0. => to + (c - light) * span(to) / s,
            _ => to,
        };
        (value * 255.).round().clamp(0., 255.) as u32
    });
    r << 16 | g << 8 | b
}

pub(super) fn box_colors(look: &Look) -> (Option<u32>, Option<u32>) {
    if look.disabled {
        return (None, None);
    }
    let edge = match (look.dark, look.hover, look.active) {
        (false, _, true) => Some(0x8d8d8d),
        (false, true, _) => Some(0x4f4f4f),
        (true, _, true) => Some(0x6e6e6e),
        (true, true, _) => Some(0xacacac),
        (dark, ..) if matches!(look.control, Control::Button | Control::Color) => {
            Some(if dark { 0x858585 } else { 0x767676 })
        }
        _ => None,
    };
    let fill = match (look.control, look.dark, look.hover, look.active) {
        (Control::Button, false, _, true) => Some(0xf5f5f5),
        (Control::Button, false, true, _) => Some(0xe5e5e5),
        (Control::Button, true, _, true) => Some(0x636363),
        (Control::Button, true, true, _) => Some(0x7b7b7b),
        _ => None,
    };
    (edge, fill)
}

#[cfg(test)]
mod tests {
    #[test]
    fn hover_and_press_shift_the_accent_like_chrome() {
        // Chrome's colors for these accents: (accent, light hover, light press).
        for (accent, hover, press) in [
            (0x2060d0, 0x194a9f, 0x467fe2),
            (0xffd000, 0xc7a200, 0xffda38),
            (0xff8a3d, 0xff6805, 0xffac75),
            (0x99c8ff, 0x61aaff, 0xd1e6ff),
        ] {
            assert_eq!(super::lighten(accent, -0.11), hover, "{accent:06x}");
            assert_eq!(super::lighten(accent, 0.11), press, "{accent:06x}");
        }
    }

    #[test]
    fn progress_tracks_follow_hover_and_press_like_chrome() {
        for (hover, active, dark, track, edge) in [
            (false, false, false, "#efefef", "#b2b2b2"),
            (true, false, false, "#e5e5e5", "#9a9a9a"),
            (true, true, false, "#f5f5f5", "#c1c1c1"),
            (true, false, true, "#3b3b3b", "#acacac"),
            (true, true, true, "#3b3b3b", "#6e6e6e"),
        ] {
            let look = super::Look {
                control: super::Control::Progress,
                hover,
                active,
                dark,
                accent: 0x2060d0,
                value: 0.5,
                ..Default::default()
            };
            let svg = super::svg(&look, 200., 16.).unwrap();
            assert!(
                svg.contains(&format!(r#"fill="{track}" stroke="{edge}""#)),
                "{svg}"
            );
        }
    }
}
