use super::super::element::{self, Build, element};
use super::Cx;
use crate::css;
use crate::css::properties::{Prop, Style};
use crate::render::paint::{background, scrollbar, spacer};
use crate::render::{PageState, ViewState, inspect};
use crate::render::{effects, paint};
use crate::style::ComputedStyles;
use renderer::Event;
use std::rc::Rc;

// The `html` and `body` nodes, and the color of the page canvas: the background of the root, the color that the
// app set for the window, or the color of the color scheme.
pub(super) fn canvas(
    cx: &Cx,
    styles: &ComputedStyles,
    set: impl FnOnce() -> Option<(u32, u8)>,
) -> (Option<usize>, Option<usize>, (u32, u8)) {
    let html = cx.doc.nodes[0]
        .children
        .iter()
        .copied()
        .find(|&id| cx.doc.nodes[id].tag == "html");
    let body = html.and_then(|html| {
        cx.doc.nodes[html]
            .children
            .iter()
            .copied()
            .find(|&id| cx.doc.nodes[id].tag == "body")
    });
    let canvas = |id: usize| {
        styles.values[id]
            .get(Prop::BACKGROUND_COLOR)
            .and_then(css::value::color::color)
            .filter(|color| color.1 > 0)
    };
    let dark = html
        .and_then(|id| styles.values[id].get(Prop::COLOR_SCHEME))
        .is_some_and(|scheme| {
            scheme.contains("dark")
                && (!scheme.contains("light") || crate::native::theme_state() & 1 != 0)
        });
    let source = background::canvas_source(cx.doc, styles);
    // The html and body boxes read each other's background, which their kept builds don't track.
    let layers = source.map(|id| {
        let style = &styles.values[id];
        element::kept::hashed((
            id,
            [
                Prop::BACKGROUND_IMAGE,
                Prop::BACKGROUND_COLOR,
                Prop::BACKGROUND_SIZE,
                Prop::BACKGROUND_POSITION,
                Prop::BACKGROUND_REPEAT,
                Prop::BACKGROUND_ORIGIN,
                Prop::BACKGROUND_ATTACHMENT,
                Prop::BACKGROUND_BLEND_MODE,
            ]
            .map(|prop| style.get(prop)),
            style.get(Prop::FONT_SIZE),
        ))
    });
    if cx.view.canvas_layers.replace(layers) != layers {
        let mut kept = cx.view.kept.borrow_mut();
        for id in [0].into_iter().chain(html).chain(body) {
            kept.remove(&id);
        }
    }
    let background = source
        .and_then(canvas)
        .or_else(|| match set() {
            Some(color) if color.1 == 0 => Some(color),
            _ if dark => Some((0x121212, 255)),
            color => color,
        })
        .unwrap_or((0xffffff, 255));
    (html, body, background)
}

// The scroll of the page viewport, which takes the overflow of the root or of the body.
pub(super) struct Viewport {
    pub(super) scroll: renderer::ScrollRef,
    // Whether the page scrolls on the x and y axes.
    pub(super) scrolls: [bool; 2],
    pub(super) rtl: bool,
    pub(super) overscroll: renderer::Point<renderer::Overscroll>,
    pub(super) bar: Option<scrollbar::Look>,
    // The room of the scroll bars at the bottom and at the right.
    pub(super) space: [f32; 2],
    // The color of the page canvas.
    pub(super) background: (u32, u8),
}

pub(super) fn viewport(
    dom: &mut PageState,
    styles: &ComputedStyles,
    html: Option<usize>,
    body: Option<usize>,
    background: (u32, u8),
) -> Viewport {
    let page_scroll = dom.geometry.scroll.entry(0).or_default().clone();
    let source = html
        .filter(|&id| {
            [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
                .into_iter()
                .any(|name| {
                    styles.values[id]
                        .get(name)
                        .is_some_and(|value| value != "visible")
                })
        })
        .or(body);
    let [overflow_x, overflow] = [Prop::OVERFLOW_X, Prop::OVERFLOW_Y].map(|prop| {
        source
            .and_then(|id| styles.values[id].get(prop))
            .unwrap_or("visible")
    });
    let [page_scrolls_x, page_scrolls] =
        [overflow_x, overflow].map(|value| !matches!(value, "hidden" | "clip"));
    // Like Chrome, the viewport takes the direction of the root element, so an RTL page scrolls from its right.
    let page_rtl = html.is_some_and(|html| styles.values[html].get(Prop::DIRECTION) == Some("rtl"));
    let page_overscroll = html.map_or_else(Default::default, |html| {
        element::margin::overscroll(&styles.values[html])
    });
    let page_bar = html
        .filter(|_| page_scrolls || page_scrolls_x)
        .and_then(|html| {
            let parts = body
                .and_then(|body| styles.extras(body).scrollbars.as_ref())
                .filter(|parts| parts[0].is_some())
                .or_else(|| styles.extras(html).scrollbars.as_ref());
            let mut look = scrollbar::Look::new(
                &styles.values[html],
                parts.map(|parts| &**parts),
                &styles.lengths[html],
                (background.1 >= 128).then_some(background.0),
            )?;
            look.forced = [
                overflow_x == "scroll" || look.forced[0],
                overflow == "scroll" || look.forced[1],
            ];
            Some(look)
        });
    let page_space = page_bar.as_ref().map_or([0.; 2], |look| {
        look.space(look.shown([page_scrolls_x, page_scrolls], page_scroll.extent()))
    });
    Viewport {
        scroll: page_scroll,
        scrolls: [page_scrolls_x, page_scrolls],
        rtl: page_rtl,
        overscroll: page_overscroll,
        bar: page_bar,
        space: page_space,
        background,
    }
}

// The box of the page: the viewport scroll with the page, its scroll bars and the boxes that paint above it.
pub(super) fn node(
    view: &Rc<ViewState>,
    viewport: Viewport,
    pointer_events: renderer::Node,
    root: renderer::Child,
    overlay: Vec<renderer::Child>,
    autos: bool,
    started: std::time::Instant,
) -> renderer::Child {
    let id = view.id;
    let page = renderer::node().id(("apps-page", 0)).size_full();
    let mut page = if viewport.scrolls[1] {
        page.overflow_y_scroll()
    } else {
        page.overflow_y_hidden()
    };
    page.style().overflow.x = match viewport.scrolls[0] {
        true => renderer::Overflow::Scroll,
        false => renderer::Overflow::Hidden,
    };
    if viewport.rtl {
        page.style().flow |= renderer::Flow::RTL_BLOCK;
    }
    page.style().overscroll = viewport.overscroll;
    let page = page
        .pr(renderer::px(viewport.space[1]))
        .pb(renderer::px(viewport.space[0]))
        .restrict_scroll_to_axis()
        .track_scroll(&viewport.scroll)
        .bg(renderer::Color::hex(
            (viewport.background.0 << 8) | viewport.background.1 as u32,
        ))
        .on_pointer({
            let view = view.clone();
            move |event, window, _| {
                if let Event::PointerMove { held, .. } = event
                    && view.moving.take()
                    && *held == Some(renderer::Button::Left)
                {
                    window.control().start_window_move();
                }
            }
        })
        .child(pointer_events)
        .child(root)
        .child({
            // Refresh the layout mirror after prepaint: scroll ranges change after the initial
            // sync_scroll.
            let data = view.clone();
            renderer::canvas(
                move |_, _| {
                    if let Ok(mut store) = data.store.try_borrow_mut()
                        && let Some(dom) = store.page_state_mut(data.id)
                    {
                        dom.geometry.sync_scroll();
                        dom.geometry.prepainted(dom.pixel_ratio);
                    }
                },
                |_, _, _| (),
            )
            .absolute()
        })
        .children(autos.then(|| spacer::autos(view.clone())))
        .children(viewport.bar.map(|look| scrollbar::Scrollbar {
            id: 0,
            handle: viewport.scroll.clone(),
            look: Some(look),
            axes: [viewport.scrolls[0], viewport.scrolls[1]],
            reserved: viewport.space,
            borders: [0.; 4],
            origin: [0.; 2],
            quiet: false,
        }))
        .children(crate::native::devtools::profiling().then(|| {
            renderer::canvas(
                |_, _| (),
                move |_, _, window| {
                    window.paint_volatile();
                    crate::native::devtools::frame(id, started)
                },
            )
            .absolute()
        }))
        .children({
            let inspect = view.inspect.get();
            (inspect.hover.is_some() || inspect.selected.is_some()).then(|| {
                let data = view.clone();
                renderer::canvas(
                    |_, _| (),
                    move |_, _, window| {
                        window.paint_volatile();
                        inspect::paint(&data, window)
                    },
                )
                .absolute()
            })
        })
        .children(view.hud.borrow().is_some().then(|| {
            let data = view.clone();
            renderer::canvas(
                |_, _| (),
                move |_, _, window| {
                    window.paint_volatile();
                    if let Some(hud) = &*data.hud.borrow() {
                        hud.paint(window);
                    }
                },
            )
            .absolute()
        }));
    if overlay.is_empty() {
        page.into()
    } else {
        renderer::node()
            .size_full()
            .child(page)
            .children(overlay)
            .into()
    }
}

// The boxes of the top layer, each above its backdrop.
pub(super) fn overlay(
    build: &Build,
    backdrops: &[(usize, Rc<Style>)],
    size: [f32; 2],
) -> Vec<renderer::Child> {
    let mut overlay: Vec<renderer::Child> = Vec::new();
    for &id in build.top_layer {
        if let Some(backdrop) = backdrops
            .iter()
            .find_map(|(node, style)| (*node == id).then_some(style))
            .or_else(|| build.styles.extras(id).backdrop.as_ref())
            .filter(|style| style.get(Prop::DISPLAY) != Some("none"))
        {
            let units = build.styles.lengths[id];
            let mut el = renderer::node().absolute().top_0().left_0().size_full();
            if backdrop
                .get(Prop::VISIBILITY)
                .is_none_or(|value| value == "visible")
            {
                if let Some(color) = backdrop
                    .get(Prop::BACKGROUND_COLOR)
                    .and_then(css::value::color::paint)
                {
                    el = el.bg(color);
                }
                if backdrop.get(Prop::OPACITY).is_some() {
                    el = el.opacity(effects::opacity(backdrop));
                }
                if backdrop
                    .get(Prop::BACKGROUND_IMAGE)
                    .is_some_and(|value| value != "none")
                {
                    let store = build.view.store.borrow();
                    el = el.children(background::css_background(
                        backdrop,
                        units,
                        id,
                        3,
                        Default::default(),
                        [[0.; 4]; 3],
                        build.view.clone(),
                        store
                            .page_state(build.view.id)
                            .and_then(|state| state.base.as_ref()),
                        &store.app.assets,
                        false,
                    ));
                }
            }
            overlay.push(
                match [Prop::BACKDROP_FILTER, Prop::WEBKIT_BACKDROP_FILTER]
                    .into_iter()
                    .find_map(|prop| backdrop.get(prop))
                    .and_then(|value| effects::filter::backdrop(value, &units))
                {
                    Some((blur, color_matrix)) => el.hooks(paint::Effected {
                        effect: renderer::PaintEffect {
                            backdrop: Some(renderer::Backdrop {
                                bounds: renderer::Rect::default(),
                                corner_radii: Default::default(),
                                blur,
                                color_matrix,
                            }),
                            ..Default::default()
                        },
                        backdrop: Some(Default::default()),
                    }),
                    None => el,
                }
                .into(),
            );
        }
        overlay.push(
            renderer::node()
                .absolute()
                .size_full()
                .stacking()
                .child(renderer::layers(true))
                .child(element(build, id, size, size, None))
                .child(renderer::layers(false))
                .into(),
        );
    }
    overlay
}
