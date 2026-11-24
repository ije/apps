use super::super::Build;
use super::super::kept::{Query, hashed};
use super::super::margin::{float_side, in_flow};
use super::super::query::parent_font;
use super::cx::{Cx, Frame};
use crate::css;
use crate::css::properties::Prop;
use crate::style::{self};
use renderer::px;

/// Places an inline-level box on its line by `vertical-align`, with the margins that keep the line box tall enough.
#[allow(clippy::too_many_arguments)]
pub(super) fn vertical_align(
    cx: &Cx<'_>,
    fr: &Frame,
    mut el: renderer::Node,
    parent: usize,
    replaced: bool,
    check: bool,
    input: Option<&str>,
    native: &Option<crate::render::paint::control::Look>,
) -> renderer::Node {
    let Cx {
        reader,
        id,
        node,
        style,
        units,
        display,
        position,
        depth,
        editable,
        alt,
        ..
    } = *cx;
    let Build {
        doc,
        styles,
        text,
        pixel_ratio,
        info,
        ..
    } = *reader;
    let Frame {
        padding,
        borders,
        margin,
        ..
    } = *fr;
    if display != "inline"
        && el.style().size.width == renderer::Length::Auto
        && (!replaced || node.tag == "button")
        && float_side(style).is_none()
    {
        el.style().flex_shrink = 1.;
    }
    reader.log(
        Query::ParentFont,
        depth,
        hashed(parent_font(doc, styles, id)),
    );
    let align = style.get(Prop::VERTICAL_ALIGN).unwrap_or("baseline");
    let middle = match el.style().size.height {
        _ if align != "middle" => None,
        _ if display == "inline" => Some(units.font.lh),
        renderer::Length::Px(height) => Some(f32::from(height) + margin[0] - margin[2]),
        _ => None,
    };
    let sized = matches!(el.style().size.height, renderer::Length::Px(_));
    // Like Chrome, a middle box puts the middle of its margin box half an x-height above the baseline.
    let centered = align == "middle" && middle.is_none();
    el.style().align_self = match align {
        "top" => Some(renderer::AlignItems::FlexStart),
        "text-top" if !sized && display != "inline" => Some(renderer::AlignItems::FlexStart),
        "bottom" => Some(renderer::AlignItems::FlexEnd),
        _ => None,
    };
    let below = |style: &css::properties::Style, font: css::value::units::FontUnits| {
        let face = text.face(&style::font(style));
        font.lh - f32::from(text.baseline(face, px(font.em), px(font.lh), pixel_ratio))
    };
    let line = styles.lengths[parent].font;
    let replaced = matches!(
        node.tag.as_str(),
        "img"
            | "svg"
            | "canvas"
            | "video"
            | "audio"
            | "iframe"
            | "embed"
            | "object"
            | "progress"
            | "meter"
            | "textarea"
    ) || check
        || display == "inline-block"
            && [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
                .into_iter()
                .any(|prop| style.get(prop).is_some_and(|value| value != "visible"))
        || display != "inline"
            && !node.tag.starts_with('#')
            && !input.is_some_and(|kind| {
                ["button", "submit", "reset", "file"]
                    .iter()
                    .any(|other| kind.eq_ignore_ascii_case(other))
            })
            && cx.blank();
    let replaced = replaced && alt.is_none();
    let field = editable && node.tag == "input";
    let swatch = native
        .as_ref()
        .is_some_and(|look| look.control == crate::render::paint::control::Control::Color);
    let parent_style = &styles.values[parent];
    let parent = below(parent_style, line);
    let edge = el.style().align_self.is_none()
        && !centered
        && (matches!(align, "text-top" | "text-bottom")
            || middle.is_some()
            || swatch
            || replaced && !field
            || display == "inline-grid"
                && !node.children.iter().any(|&child| in_flow(doc, info, child)));
    if centered {
        el.style().baseline = renderer::Baseline::Middle;
    } else if edge {
        el.style().baseline = renderer::Baseline::None;
    }
    let lift = if centered {
        line.ex / 2. + (margin[2] - margin[0]) / 2.
    } else if !edge {
        0.
    } else if matches!(align, "text-top" | "text-bottom") {
        let face = text.face(&style::font(parent_style));
        let [ascent, descent, _] = text
            .line_metrics(face, px(line.em * pixel_ratio))
            .map(|value| f32::from(value) / pixel_ratio);
        // The inline box of a span is as tall as its line height.
        match el.style().size.height {
            renderer::Length::Px(height) if align == "text-top" => {
                ascent - margin[0] - f32::from(height)
            }
            _ if align == "text-top" => ascent - units.font.lh,
            _ => margin[2] - descent,
        }
    } else if let Some(height) = middle {
        line.ex / 2. - height / 2.
    } else if swatch {
        -padding[2] - borders[2] - 4.
    } else if node.tag == "button"
        && node.children.iter().all(|&child| {
            doc.nodes[child].tag == "#text" && doc.nodes[child].text.trim_ascii().is_empty()
        })
    {
        -padding[2] - borders[2]
    } else if native.as_ref().is_some_and(|look| {
        // Like Chrome, a range sits on the bottom of its border box, a progress bar without its native look
        // on the bottom of its margin box.
        !look.plain || look.control == crate::render::paint::control::Control::Range
    }) {
        0.
    } else {
        margin[2]
    };
    let inset = |prop| style.get(prop).filter(|value| *value != "auto");
    let offset = match (inset(Prop::TOP), inset(Prop::BOTTOM)) {
        _ if position != "relative" => Some(0.),
        (Some(top), _) => units.resolve(top, f32::NAN),
        (None, Some(bottom)) => units.resolve(bottom, f32::NAN).map(|bottom| -bottom),
        (None, None) => Some(0.),
    };
    let raise = match align {
        _ if offset.is_none() => 0.,
        "super" => line.em / 3. + 1. / pixel_ratio + lift,
        "sub" => -line.em / 5. - 1. / pixel_ratio + lift,
        "baseline" => lift,
        value => units.resolve(value, units.font.lh).unwrap_or(0.) + lift,
    };
    // A box with a line of text has its baseline there, also when the text overflows its height.
    let reserve = match (el.style().size.height, el.style().align_self) {
        (renderer::Length::Px(height), None) if display != "inline" && replaced => {
            line.lh - parent - f32::from(height)
        }
        (renderer::Length::Auto, None) if field => {
            line.lh - parent - units.font.lh - padding[0] - borders[0] + below(style, units.font)
        }
        _ => f32::NEG_INFINITY,
    };
    let floor = margin[2].min(0.) + parent
        - match () {
            _ if centered
                || matches!(display, "inline-block" | "inline-flex" | "inline-grid")
                    && !replaced =>
            {
                f32::INFINITY
            }
            // A top or bottom box sits on a line edge, so the strut below the baseline does not reach it.
            _ if matches!(
                el.style().align_self,
                Some(renderer::AlignItems::FlexStart | renderer::AlignItems::FlexEnd)
            ) =>
            {
                f32::INFINITY
            }
            _ if edge => 0.,
            _ if display == "inline" => below(style, units.font),
            _ => padding[2] + borders[2] + below(style, units.font),
        };
    if !crate::render::inline::makes_no_line(doc, styles, id)
        && (raise != 0. || reserve > margin[0] || margin[2] < floor)
    {
        el = el
            .mt(px((margin[0] + raise).max(reserve)))
            .mb(px((margin[2] - raise).max(floor)));
    }
    if let Some(offset) = offset
        && (raise != 0. || offset != 0.)
    {
        el = el.top(px(offset - raise));
    }
    el
}
