use super::super::Build;
use super::super::kept::{Query, hashed};
use super::super::margin::overscroll;
use super::super::query::LOOK;
use super::cx::Cx;
use crate::css;
use crate::css::properties::Prop;
use crate::render::layout::size::{field_line, file_size, layout_ceil};
use crate::render::layout::temporal::{icon, parts, temporal};
use crate::style::{self};
use renderer::{Color, SharedString, px, relative};
use std::rc::Rc;

/// Builds the parts of form controls: the file button, button labels, and the text of fields.
#[inline(never)]
pub(super) fn controls(
    cx: &Cx<'_>,
    mut el: renderer::Node,
    input: Option<&str>,
    appkit: Option<renderer::ControlKind>,
) -> (renderer::Node, Option<renderer::Node>) {
    let Cx {
        reader,
        id,
        node,
        style,
        units,
        parsed,
        containing,
        rtl,
        line_justify,
        editable,
        selection_colors,
        ..
    } = *cx;
    let Build {
        doc,
        styles,
        text,
        pixel_ratio,
        view,
        ..
    } = *reader;
    if input.is_some_and(|kind| kind.eq_ignore_ascii_case("file"))
        && let Some(part) = styles
            .extras(id)
            .scrollbars
            .as_ref()
            .and_then(|parts| parts[1].as_ref())
    {
        // The button takes its box from `::file-selector-button`, which the agent sheet always gives.
        let [padding, margin] = [css::properties::PADDING, css::properties::MARGIN]
            .map(|sides| units.edges(part, sides, 0.));
        let borders = units.borders(part, 0.);
        let color = |prop| part.get(prop).and_then(css::value::color::paint);
        let radius = part
            .get(Prop::BORDER_TOP_LEFT_RADIUS)
            .and_then(|value| units.resolve(value, 0.))
            .unwrap_or(0.);
        let mut button = renderer::node()
            .flex_shrink_0()
            .mt(px(margin[0]))
            .mr(px(margin[1]))
            .mb(px(margin[2]))
            .ml(px(margin[3]))
            .pt(px(padding[0]))
            .pr(px(padding[1]))
            .pb(px(padding[2]))
            .pl(px(padding[3]))
            .border_t(px(borders[0]))
            .border_r(px(borders[1]))
            .border_b(px(borders[2]))
            .border_l(px(borders[3]))
            .border_color(color(Prop::BORDER_TOP_COLOR).unwrap_or(renderer::Color::TRANSPARENT))
            .bg(color(Prop::BACKGROUND_COLOR).unwrap_or(renderer::Color::TRANSPARENT))
            .child("Choose File");
        if let Some(ink) = color(Prop::COLOR) {
            button = button.text_color(ink);
        }
        if let Some(size) = part
            .get(Prop::FONT_SIZE)
            .and_then(|value| units.resolve(value, units.font.em))
            .filter(|size| *size != units.font.em)
        {
            let text = button.text_style();
            text.set_font_size(px(size));
            text.set_line_height(px(units.font.lh * size / units.font.em));
        }
        if let Some(weight) = part
            .get(Prop::FONT_WEIGHT)
            .filter(|weight| Some(*weight) != style.get(Prop::FONT_WEIGHT))
            .and_then(|weight| match weight {
                "bold" => Some(700.),
                "normal" => Some(400.),
                weight => weight.parse().ok(),
            })
        {
            button.text_style().set_font_weight(weight);
        }
        button.style().corner_radii = renderer::Corners::all(px(radius).into()).into();
        el = el
            .flex_row()
            .items_center()
            .overflow_hidden()
            .child(button)
            .child(renderer::node().whitespace_nowrap().child("No file chosen"));
        if style.get(Prop::WIDTH).is_none_or(|value| value == "auto") {
            el = el.w(px(file_size(style, part, &units, text, pixel_ratio)[0]));
        }
    }
    if appkit == Some(renderer::ControlKind::Button) {
        let [padding, height] =
            crate::render::layout::size::native_size(renderer::ControlKind::Button);
        if style.get(Prop::WIDTH).is_none_or(|value| value == "auto") {
            let label = node.button_label();
            let run = style::text_run(style, &units, label.len());
            let width = text.shape_line(label, px(units.font.em), &run).line.width;
            el = el.w(px(layout_ceil(f32::from(width) + padding, pixel_ratio)));
        }
        if style.get(Prop::HEIGHT).is_none_or(|value| value == "auto") {
            el = el.h(px(height));
        }
    }
    if appkit.is_none()
        && input.is_some_and(|kind| {
            ["button", "submit", "reset"]
                .iter()
                .any(|other| kind.eq_ignore_ascii_case(other))
        })
    {
        el = el.items_center().justify_center().child(SharedString::from(
            node.attrs.get("value").cloned().unwrap_or_else(|| {
                if input.is_some_and(|kind| kind.eq_ignore_ascii_case("reset")) {
                    "Reset".into()
                } else if input.is_some_and(|kind| kind.eq_ignore_ascii_case("submit")) {
                    "Submit".into()
                } else {
                    String::new()
                }
            }),
        ));
    }
    let field_line_height = if editable {
        field_line(node, style, &units, text, pixel_ratio)
    } else {
        0.
    };
    if editable && node.tag != "textarea" {
        let above = f32::from(text.baseline(
            text.face(&parsed.face),
            px(units.font.em),
            px(field_line_height),
            pixel_ratio,
        ));
        el.style().justify_content = Some(renderer::AlignContent::Center);
        el = el.child(
            renderer::node()
                .w(px(0.))
                .h(px(above))
                .mb(px(field_line_height - above))
                .flex_shrink_0()
                .baseline(renderer::Baseline::Bottom),
        );
    }
    let (mut field_layer, mut picker) = (None, None);
    if editable {
        // A date or time input shows its fields until it has the focus. Then its value edits as text.
        let fields = temporal(node);
        if fields || style.get(Prop::FIELD_SIZING) == Some("content") {
            reader.log(Query::Value(id as u32), 0, hashed(&node.value));
        }
        let text = node.shown_text(
            node.current_value()
                .map_or_else(|| doc.text(id), str::to_owned),
        );
        let editing = node.state & crate::css::selector::FOCUS != 0;
        if fields {
            reader.log(Query::Look(id as u32), 0, hashed(node.state & LOOK));
        }
        let placeholder = match input.filter(|_| fields) {
            Some(kind) => match kind.to_ascii_lowercase().as_str() {
                "date" => "yyyy-mm-dd",
                "time" => "hh:mm",
                "month" => "yyyy-mm",
                "week" => "yyyy-Www",
                _ => "yyyy-mm-ddThh:mm",
            }
            .to_owned(),
            None => node.attrs.get("placeholder").cloned().unwrap_or_default(),
        };
        let font_size = units.font.em;
        let text_color = style
            .get(Prop::COLOR)
            .and_then(css::value::color::paint)
            .unwrap_or(renderer::Color::BLACK);
        let pseudo = styles
            .extras(id)
            .text
            .as_ref()
            .and_then(|pseudos| pseudos[3].as_ref());
        let mut placeholder_color = pseudo
            .and_then(|pseudo| css::value::color::paint(pseudo.get(Prop::COLOR)?))
            .unwrap_or(Color::hex(0x757575ff));
        placeholder_color.a *= pseudo
            .and_then(|pseudo| pseudo.get(Prop::OPACITY)?.parse::<f32>().ok())
            .map_or(1., |opacity| opacity.clamp(0., 1.));
        let single_line = node.tag != "textarea";
        let mut padding = units.edges(style, css::properties::PADDING, containing[0]);
        // The native look has a border of 1px.
        if fields
            && crate::render::paint::control::kind(&node.tag, input, false, style)
                .is_some_and(|(_, boxed, _)| boxed)
        {
            padding = padding.map(|side| side + 1.);
        }
        let empty = text.is_empty();
        let face = match pseudo {
            Some(pseudo) if empty => style::font(pseudo),
            _ => parsed.face.clone(),
        };
        let mut shown = if empty { placeholder } else { text };
        if let Some(mode) = pseudo
            .filter(|_| empty)
            .unwrap_or(style)
            .get(Prop::TEXT_TRANSFORM)
            .filter(|mode| *mode != "none")
        {
            let len = shown.len();
            style::text_transform(&mut shown, 0..len, mode);
        }
        let shown: Rc<str> = shown.into();
        let run = renderer::TextRun {
            len: shown.len(),
            font: face,
            color: if empty { placeholder_color } else { text_color },
            background_color: None,
            underline: None,
            strikethrough: None,
            overline: None,
            letter_spacing: style::spacing(style, &units, Prop::LETTER_SPACING),
            word_spacing: px(0.),
            font_size: None,
            inset: Default::default(),
            nowrap: false,
            preserve: true,
            hyphenation: None,
            rtl,
            plaintext: style.get(Prop::UNICODE_BIDI) == Some("plaintext"),
            embedding: None,
            clear: 0,
            tab_size: 8.,
            stroke: None,
            emphasis: None,
        };
        if !single_line && style.get(Prop::FIELD_SIZING) == Some("content") {
            // The full width of the content box, so the text wraps as in the field.
            let mut sizer = renderer::text(shown.clone(), Some(Rc::new([run.clone()])))
                .w(relative(1.))
                .min_h(px(field_line_height))
                .flex_shrink_0();
            sizer.style().visibility = renderer::Visibility::Hidden;
            el = el.child(sizer);
        }
        let handle = view
            .store
            .borrow_mut()
            .state(view.id)
            .geometry
            .scroll
            .entry(id)
            .or_default()
            .clone();
        let field = crate::render::paint::field::FieldText {
            selected: Default::default(),
            id,
            view: view.clone(),
            placeholder: empty,
            password: (node.password() && !empty)
                .then(|| node.current_value().unwrap_or("").to_owned()),
            text: shown,
            run,
            single_line,
            rtl: rtl && line_justify == Some(renderer::AlignContent::End),
            rows: Default::default(),
            caret: crate::render::paint::text::CaretLook::new(style),
            selection: selection_colors,
            handle: handle.clone(),
        };
        let mut clip = renderer::node()
            .absolute()
            .track_scroll(&handle)
            .text_size(px(font_size))
            .line_height(px(field_line_height));
        clip.style().restrict_scroll_to_axis = true;
        clip.style().overscroll = overscroll(style);
        el = el.cursor(renderer::Cursor::Text);
        // Added after the background layers, which are absolute too and would paint over the text.
        field_layer = Some(if single_line {
            let mut line = renderer::node().flex_shrink_0();
            let mut room = 0.;
            if fields {
                let ([width, _], size, inset) = icon(node, units.font.em);
                room = width;
                let dark = cx.dark();
                let look = crate::render::paint::control::Look {
                    zoom: units.font.zoom,
                    control: match input.is_some_and(|kind| kind.eq_ignore_ascii_case("time")) {
                        true => crate::render::paint::control::Control::Time,
                        false => crate::render::paint::control::Control::Date,
                    },
                    ink: if dark { 0xffffff } else { 0 },
                    ..Default::default()
                };
                if !node.attrs.contains_key("disabled") && !node.attrs.contains_key("readonly") {
                    picker = Some(
                        renderer::node()
                            .absolute()
                            .top(px(padding[0]))
                            .right(px(padding[1]))
                            .bottom(px(padding[2]))
                            .w(px(size + 2. * inset))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                renderer::node()
                                    .relative()
                                    .flex_shrink_0()
                                    .size(px(size))
                                    .child(crate::render::paint::control::layer(
                                        id,
                                        look,
                                        view.clone(),
                                    )),
                            ),
                    );
                }
            }
            line = match fields && !editing {
                true => line
                    .flex()
                    .flex_row()
                    .children(parts(node).into_iter().map(|part| {
                        let side = if part.field { 1. } else { 0. };
                        let run = renderer::TextRun {
                            len: part.text.len(),
                            color: text_color,
                            ..field.run.clone()
                        };
                        renderer::text(part.text, Some([run].into()))
                            .flex_shrink_0()
                            .pl(px(side))
                            .pr(px(side))
                    })),
                false => line.child(field),
            };
            // A right-to-left row starts at the right, so text that overflows goes to the left and scrolls from there.
            let line = match (line_justify, rtl) {
                (Some(renderer::AlignContent::Center), _) => line.ml_auto().mr_auto(),
                (Some(renderer::AlignContent::End), false) => line.ml_auto(),
                (None, true) => line.mr_auto(),
                _ => line,
            };
            let mut clip = clip
                .top(px(padding[0]))
                .right(px(padding[1] + room))
                .bottom(px(padding[2]))
                .left(px(padding[3]))
                .overflow_hidden()
                .flex()
                .flex_row()
                .items_center()
                .whitespace_nowrap();
            if rtl && !fields {
                clip.style().flex_direction = renderer::FlexDirection::RowReverse;
            }
            let field = clip.child(line);
            match picker {
                Some(picker) => renderer::node()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .child(field)
                    .child(picker),
                None => field,
            }
        } else {
            if rtl && line_justify == Some(renderer::AlignContent::End) {
                clip.style().flow |= renderer::Flow::RTL_BLOCK;
            }
            clip.style().overflow.x = renderer::Overflow::Scroll;
            clip.top_0()
                .right(px(0.))
                .bottom(px(0.))
                .left_0()
                .pt(px(padding[0]))
                .pb(px(padding[2]))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                // The text takes the side padding, so like the inner editor of Chrome, the end padding doesn't
                // follow text that overflows.
                .child(
                    renderer::Node::from(field)
                        .pl(px(padding[3]))
                        .pr(px(padding[1])),
                )
        });
    }
    (el, field_layer)
}
