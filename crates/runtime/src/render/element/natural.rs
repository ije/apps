use super::super::layout::size::dimension;
use super::Build;
use super::cx::{Cx, Frame};
use super::kept::{Query, hashed};
use super::margin::shrinks_around;
use super::query::only_auto_ratio;
use crate::css::properties::Prop;
use renderer::px;

/// Sizes a box with a natural size, like an image, from that size, its ratio and its constraints.
#[inline(never)]
pub(super) fn natural_box(
    cx: &Cx<'_>,
    fr: &Frame,
    mut el: renderer::Node,
    natural: [f32; 2],
    line_base: Option<bool>,
) -> renderer::Node {
    let Cx {
        reader,
        id,
        node,
        style,
        units,
        position,
        containing,
        broken,
        ..
    } = *cx;
    let Build {
        doc,
        styles,
        flex_parent,
        ..
    } = *reader;
    let Frame {
        padding,
        borders,
        margin,
        extra,
        natural_ratio,
        ..
    } = *fr;
    let chrome = [
        padding[1] + padding[3] + borders[1] + borders[3],
        padding[0] + padding[2] + borders[0] + borders[2],
    ];
    let props = [Prop::WIDTH, Prop::HEIGHT];
    let hint = [0, 1].map(|axis| {
        node.attrs
            .get(["width", "height"][axis])
            .filter(|_| {
                style.get(props[axis]).is_none()
                    && matches!(node.tag.as_str(), "img" | "video")
                    && !(broken && natural == [16., 16.])
            })
            .and_then(|value| dimension(value, false))
            .map(|length| length + extra[axis])
    });
    if let Some(width) = hint[0] {
        el = el.w(px(width));
    }
    if let Some(height) = hint[1] {
        el = el.h(px(height));
    }
    let set = [0, 1].map(|axis| {
        hint[axis].is_some()
            || style.get(props[axis]).is_some_and(|value| {
                value != "auto" && !(axis == 1 && line_base == Some(false) && value.contains('%'))
            })
    });
    let [content_width, content_height] = [0, 1].map(|axis| {
        hint[axis]
            .or_else(|| {
                style
                    .get(props[axis])
                    .filter(|value| !value.contains('%'))
                    .and_then(|value| units.resolve(value, containing[axis]))
                    .map(|length| length + extra[axis])
            })
            .map(|length| (length - chrome[axis]).max(0.))
    });
    // A flex item's automatic minimum is the smaller of its specified and natural widths.
    if flex_parent
        && style
            .get(Prop::MIN_WIDTH)
            .is_none_or(|value| value == "auto")
        && reader
            .parent(id, Prop::FLEX_DIRECTION)
            .is_none_or(|value| !value.starts_with("column"))
        && let Some(percent) = style
            .get(Prop::WIDTH)
            .and_then(|value| value.strip_suffix('%'))
            .and_then(|value| value.parse::<f32>().ok())
    {
        el = el.min_w(px(
            (percent.max(0.) / 100. * containing[0] + extra[0]).min(natural[0] + chrome[0])
        ));
    }
    let stretch = [cx.stretched(0), cx.stretched(1)];
    let css = el.style().aspect_ratio.is_some();
    let line = (stretch[1]
        && reader
            .parent(id, Prop::FLEX_WRAP)
            .is_none_or(|value| value == "nowrap")
        && reader
            .parent(id, Prop::HEIGHT)
            .is_some_and(|value| value != "auto" && !value.contains('%')))
    .then(|| (containing[1] - margin[0] - margin[2] - chrome[1]).max(0.));
    let sized = !set[0] && !stretch[0] && !(css && set[1]);
    let width = if set[0] {
        content_width
    } else if !sized {
        None
    } else {
        match natural_ratio.filter(|_| !css) {
            Some(ratio) if set[1] => content_height.map(|height| height * ratio),
            Some(ratio) => Some(line.map_or(natural[0], |line| line * ratio)),
            None => Some(natural[0]),
        }
    };
    // Like Chrome, a video without a frame has no natural size when CSS gives it a ratio, so it fills the line.
    if sized
        && css
        && natural_ratio.is_none()
        && node.tag == "video"
        && !matches!(position, "absolute" | "fixed")
    {
        match el.style().aspect_ratio {
            Some(ratio) => {
                // A box around it that shrinks to its content makes it as narrow as its frame.
                let shrinks = shrinks_around(doc, styles, id);
                reader.log(Query::Shrinks(id as u32), 0, hashed(shrinks));
                let limit = |prop| {
                    style
                        .get(prop)
                        .and_then(|value| units.resolve(value, containing[0]))
                        .map(|value| value + extra[0])
                };
                let width = if shrinks {
                    chrome[0]
                } else {
                    containing[0] - margin[1] - margin[3]
                }
                .min(limit(Prop::MAX_WIDTH).unwrap_or(f32::INFINITY))
                .max(limit(Prop::MIN_WIDTH).unwrap_or(0.));
                // The ratio of the width and height attributes is for the content box.
                let height = if style.get(Prop::BOX_SIZING) == Some("border-box")
                    && style
                        .get(Prop::ASPECT_RATIO)
                        .is_some_and(|v| !v.contains("auto"))
                {
                    width / ratio
                } else {
                    (width - chrome[0]).max(0.) / ratio + chrome[1]
                };
                el = el.w(px(width)).h(px(height));
            }
            None => el = el.w(renderer::Length::Calc(1., px(-margin[1] - margin[3]))),
        }
    } else if sized {
        if let Some(width) = width {
            el = el.w(px(width + chrome[0]));
            if let Some(min) = fr.floor(cx, 0, width + chrome[0]) {
                el = el.min_w(px(min));
            }
        } else {
            el.style().aspect_ratio = natural_ratio;
            el = only_auto_ratio(el, style);
        }
    }
    if let Some(height) = match natural_ratio {
        _ if set[1] || line.is_some() || css => None,
        None => Some(natural[1]),
        Some(ratio) if matches!(position, "absolute" | "fixed") => width.map(|width| width / ratio),
        Some(_) => None,
    } {
        let height = height + chrome[1];
        if stretch[1]
            && style
                .get(Prop::MIN_HEIGHT)
                .is_none_or(|value| value == "auto")
        {
            el = el.min_h(px(height));
        } else {
            el = el.h(px(height));
            if let Some(min) = fr.floor(cx, 1, height) {
                el = el.min_h(px(min));
            }
        }
    }
    el
}
