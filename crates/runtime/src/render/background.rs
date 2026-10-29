use super::{WebView, paint::Effected};
use crate::style;
use gpui::{AnyElement, div, img, prelude::*, px, rgba};
use std::collections::HashMap;

#[allow(clippy::too_many_arguments)]
pub(super) fn css_background(
    style: &style::Style,
    units: style::Lengths,
    id: usize,
    slot: usize,
    radius: [f32; 4],
    view: gpui::Entity<WebView>,
    base: Option<&url::Url>,
    assets: &HashMap<String, std::path::PathBuf>,
) -> Vec<AnyElement> {
    let list = |name: &str| {
        style::components(style.get(name).map_or("", String::as_str), ',')
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let (sizes, positions, repeats) = (
        list("background-size"),
        list("background-position"),
        list("background-repeat"),
    );
    let pick = |values: &[String], index: usize| {
        values
            .get(index % values.len().max(1))
            .cloned()
            .unwrap_or_default()
    };
    let corners = gpui::Corners {
        top_left: px(radius[0]),
        top_right: px(radius[1]),
        bottom_right: px(radius[2]),
        bottom_left: px(radius[3]),
    };
    let mut layers = Vec::new();
    let mut indexes = Vec::new();
    for (index, image) in list("background-image").into_iter().enumerate().rev() {
        let (size, position, repeat) = (
            pick(&sizes, index),
            pick(&positions, index),
            pick(&repeats, index),
        );
        if let Some(source) = image.strip_prefix("url(").and_then(|v| v.strip_suffix(')')) {
            if let Some(path) = base
                .and_then(|base| base.join(source.trim().trim_matches(['"', '\''])).ok())
                .and_then(|url| match url.scheme() {
                    "file" => url.to_file_path().ok(),
                    "apps" => assets.get(url.path().trim_start_matches('/')).cloned(),
                    _ => None,
                })
            {
                layers.push(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .overflow_hidden()
                        .child(img(path).size_full().object_fit(match size.as_str() {
                            "cover" => gpui::ObjectFit::Cover,
                            "contain" => gpui::ObjectFit::Contain,
                            "" | "auto" | "auto auto" => gpui::ObjectFit::None,
                            _ => gpui::ObjectFit::Fill,
                        }))
                        .into_any_element(),
                );
                indexes.push(index);
            }
            continue;
        }
        let Some((kind, body)) = image
            .split_once('(')
            .filter(|(kind, _)| kind.ends_with("-gradient"))
            .and_then(|(kind, body)| Some((kind.to_owned(), body.strip_suffix(')')?.to_owned())))
        else {
            continue;
        };
        let view = view.clone();
        layers.push(
            gpui::canvas(
                |_, _, _| (),
                move |bounds, _, window, cx| {
                    let [width, height] =
                        [f32::from(bounds.size.width), f32::from(bounds.size.height)];
                    if width <= 0. || height <= 0. {
                        return;
                    }
                    let Some(source) = super::effects::gradient_svg(
                        &kind, &body, &size, &position, &repeat, units, width, height,
                    ) else {
                        return;
                    };
                    let view = view.read(cx);
                    let mut host = view.host.borrow_mut();
                    let backgrounds = &mut host.dom.entry(view.id).or_default().backgrounds;
                    let scale = window.scale_factor();
                    let rw = (width * scale).ceil().clamp(1., 4096.) as u32;
                    let rh = (height * scale).ceil().clamp(1., 4096.) as u32;
                    let key = (id, slot * 64 + index);
                    if backgrounds.get(&key).is_none_or(|(old, image)| {
                        old != &source
                            || u32::from(image.size(0).width) != rw
                            || u32::from(image.size(0).height) != rh
                    }) && let Ok(image) = super::canvas::svg(&source, rw, rh, None)
                        && let Some((_, old)) = backgrounds.insert(key, (source, image))
                    {
                        let _ = window.drop_image(old);
                    }
                    if let Some((_, image)) = backgrounds.get(&key) {
                        let _ =
                            window.paint_image(bounds, bounds, corners, image.clone(), 0, false);
                    }
                },
            )
            .absolute()
            .left_0()
            .top_0()
            .size_full()
            .into_any_element(),
        );
        indexes.push(index);
    }
    let modes = list("background-blend-mode");
    if modes
        .iter()
        .all(|mode| matches!(mode.trim(), "" | "normal"))
    {
        return layers;
    }
    // Blended layers mix with the layers and color below them, inside an isolated group.
    let mut group = div().absolute().top_0().left_0().size_full();
    if let Some((ink, alpha)) = style
        .get("background-color")
        .and_then(|value| style::color(value))
    {
        group = group.child(
            div()
                .absolute()
                .size_full()
                .rounded_tl(px(radius[0]))
                .rounded_tr(px(radius[1]))
                .rounded_br(px(radius[2]))
                .rounded_bl(px(radius[3]))
                .bg(rgba(ink << 8 | alpha as u32)),
        );
    }
    for (layer, index) in layers.into_iter().zip(indexes) {
        let blend_mode = super::effects::blend_mode(&pick(&modes, index));
        group = group.child(Effected {
            effect: gpui::PaintEffect {
                blend_mode,
                ..Default::default()
            },
            child: layer,
        });
    }
    vec![
        Effected {
            effect: gpui::PaintEffect::default(),
            child: group.into_any_element(),
        }
        .into_any_element(),
    ]
}
