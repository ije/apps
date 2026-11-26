use super::super::Build;
use super::super::kept::Query;
use super::super::query::{insets, pictured};
use super::super::svg::svg_layer;
use super::cx::Cx;
use crate::css;
use crate::css::properties::Prop;
use crate::render::layout::size::{dimension, natural_size};
use crate::render::paint::scrollbar;
use crate::style::{self};
use renderer::{Child, Color, Rect, point, px, size};
use std::{cell::RefCell, rc::Rc, sync::Arc};

/// Builds the content of replaced elements: canvas, terminal, image, video and broken image.
#[inline(never)]
pub(super) fn replaced(
    cx: &Cx<'_>,
    mut el: renderer::Node,
    natural: &mut Option<[f32; 2]>,
    natural_ratio: &mut Option<f32>,
    content_layers: &mut Vec<Child>,
    grid_bar: &mut Option<scrollbar::Scrollbar>,
    picture: &mut Option<renderer::ObjectFit>,
) -> renderer::Node {
    let Cx {
        reader,
        id,
        node,
        style,
        units,
        containing,
        broken,
        alt,
        sizeless,
        ..
    } = *cx;
    let Build {
        doc, styles, view, ..
    } = *reader;
    if node.tag == "canvas" {
        let picture = |source| {
            renderer::node()
                .size_full()
                .hooks(crate::render::paint::Picture {
                    id,
                    view: view.clone(),
                    source,
                    fit: renderer::ObjectFit::Fill,
                    cropped: None,
                    position: None,
                    pixelated: matches!(
                        style.get(Prop::IMAGE_RENDERING),
                        Some("pixelated" | "crisp-edges")
                    ),
                })
        };
        let mut presented = picture(crate::render::paint::Source::Presented)
            .child(picture(crate::render::paint::Source::Drawn).absolute());
        presented.style().aspect_ratio = *natural_ratio;
        content_layers.push(presented.into());
    }
    if node.tag == "terminal" {
        let store = view.store.borrow();
        static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !store.app.features.has(crate::bundle::Feature::Terminal)
            && !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            eprintln!(
                "apps: The terminal feature is disabled. Add \"terminal\" to \"features\" in package.json"
            );
        }
        if let Some(state) = store
            .page_state(view.id)
            .filter(|_| store.app.features.has(crate::bundle::Feature::Terminal))
        {
            let look = crate::render::paint::terminal::look(
                style,
                styles.extras(id).selection.as_ref(),
                &units,
            );
            // The bar of the scrollback.
            *grid_bar = Some(scrollbar::Scrollbar {
                id,
                handle: (state
                    .media
                    .terminals
                    .borrow_mut()
                    .bars
                    .entry(id)
                    .or_default()
                    .0)
                    .clone(),
                look: scrollbar::Look::new(style, None, &units, Some(look.background())),
                axes: [false, true],
                reserved: [0.; 2],
                borders: insets(styles, id, false, containing[0]),
                origin: [0.; 2],
                quiet: true,
            });
            content_layers.push(
                renderer::node()
                    .size_full()
                    .hooks(crate::render::paint::terminal::Grid {
                        id,
                        view: view.clone(),
                        terminals: state.media.terminals.clone(),
                        look: Rc::new(look),
                        edges: insets(styles, id, true, containing[0]),
                    })
                    .into(),
            );
        }
    }
    if node.tag == "img" || node.tag == "video" {
        let (image, vector) = view
            .store
            .borrow()
            .page_state(view.id)
            .filter(|_| !broken)
            .map_or((None, None), |state| {
                (
                    state.media.images.get(&id).cloned(),
                    state.media.svg_images.get(&id).cloned(),
                )
            });
        reader.log(
            Query::Image(id as u32),
            0,
            pictured(image.as_ref(), vector.as_ref()),
        );
        (*natural, *natural_ratio) = natural_size(
            doc,
            id,
            image
                .as_ref()
                .map(|image| {
                    let size = image.size();
                    [size.width as u32 as f32, size.height as u32 as f32]
                })
                .or(vector.as_ref().map(|(_, natural)| *natural)),
            style,
        )
        .map_or((None, None), |(size, ratio)| (Some(size), ratio));
        if image.is_some() || vector.is_some() {
            let fit = match style.get(Prop::OBJECT_FIT) {
                Some("contain") => renderer::ObjectFit::Contain,
                Some("cover") => renderer::ObjectFit::Cover,
                Some("none") => renderer::ObjectFit::None,
                Some("scale-down") => renderer::ObjectFit::ScaleDown,
                _ => renderer::ObjectFit::Fill,
            };
            if image.is_some() {
                *picture = Some(fit);
            } else {
                let padding = units.edges(style, css::properties::PADDING, containing[0]);
                content_layers.push(svg_layer(id, false, false, view.clone(), padding));
            }
        }
        if image.is_none() && natural_ratio.is_some() {
            let mut sized = renderer::node().size_full();
            sized.style().aspect_ratio = *natural_ratio;
            el = el.child(sized);
        } else if natural.is_none() && !sizeless {
            for (name, prop, horizontal) in [
                ("width", Prop::WIDTH, true),
                ("height", Prop::HEIGHT, false),
            ] {
                if style.get(prop).is_none_or(|value| value == "auto")
                    && let Some(length) = node
                        .attrs
                        .get(name)
                        .and_then(|value| dimension(value, false))
                        .map(|length| length * units.font.zoom)
                {
                    el = if horizontal {
                        el.w(px(length))
                    } else {
                        el.h(px(length))
                    };
                }
            }
        }
    }
    if broken {
        let icon = || {
            renderer::canvas(
                |_, _| (),
                |bounds, _, window| paint_broken_icon(bounds.origin, window),
            )
            .w(px(16.))
            .h(px(16.))
            .flex_shrink_0()
        };
        let framed = alt.is_none()
            && natural.is_some_and(|size| size != [0., 0.])
            && [("width", Prop::WIDTH), ("height", Prop::HEIGHT)]
                .iter()
                .all(|&(name, prop)| {
                    style.get(prop).is_some_and(|value| value != "auto")
                        || node.attrs.contains_key(name)
                });
        el = if let Some(alt) = alt {
            let mut text = renderer::text(
                alt.clone(),
                Some(Rc::new([style::text_run(style, &units, alt.len())])),
            );
            text.text_style().set_float(px(16.));
            el.child(
                renderer::node()
                    .min_h(px(16.))
                    .child(text)
                    .child(icon().absolute().top_0().left_0()),
            )
        } else if framed {
            el.child(
                renderer::canvas(
                    |_, _| (),
                    |bounds, _, window| {
                        window.draw_box(
                            bounds,
                            px(0.),
                            renderer::Color::TRANSPARENT,
                            renderer::Border {
                                widths: px(1.).into(),
                                color: Color::hex(0xc0c0c0ff),
                                style: renderer::BorderStyle::Solid,
                                sides: 0,
                            },
                        );
                        if bounds.size.width >= px(18.) && bounds.size.height >= px(18.) {
                            paint_broken_icon(bounds.origin + point(px(2.), px(2.)), window);
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
        } else if *natural == Some([16., 16.]) {
            el.child(icon())
        } else {
            el
        };
    }
    el
}

pub(super) fn paint_broken_icon(
    origin: renderer::Point<renderer::Pixels>,
    window: &mut renderer::Window,
) {
    thread_local! {
        static ICON: RefCell<Option<(u32, Arc<renderer::RenderImage>)>> = const { RefCell::new(None) };
    }
    const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><defs><linearGradient id="s" x2="0" y2="1"><stop offset="0" stop-color="#ebf2fd"/><stop offset="1" stop-color="#c3d4f3"/></linearGradient><clipPath id="c"><path d="M.25.25h10l5.5 5.5v10H.25z"/></clipPath></defs><g clip-path="url(#c)"><rect width="16" height="16" fill="url(#s)"/><ellipse cx="6" cy="4.5" rx="2.5" ry="1.25" fill="#fff"/><ellipse cx="7" cy="17" rx="7.5" ry="6.5" fill="#58ae39"/></g><path d="M.25.25h10l5.5 5.5v10H.25z" fill="none" stroke="#999" stroke-width=".5"/><path d="M10.25.25v5.5h5.5" fill="#f2f2f2" stroke="#999" stroke-width=".5"/><path d="M16.5 8 7.5 16.5" stroke="#fff" stroke-width="1.25"/></svg>"##;
    let raster = (16. * window.scale_factor()).ceil() as u32;
    let Some(image) = ICON.with_borrow_mut(|icon| {
        if icon.as_ref().is_none_or(|(size, _)| *size != raster) {
            *icon = crate::render::raster::svg::svg(SVG, raster, raster)
                .ok()
                .map(|image| (raster, image));
        }
        icon.as_ref().map(|(_, image)| image.clone())
    }) else {
        return;
    };
    let bounds = Rect::new(origin, size(px(16.), px(16.)));
    window.draw_image(bounds, bounds, px(0.), image, false, false);
}
