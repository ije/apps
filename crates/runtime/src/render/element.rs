use super::{
    PageState,
    background::css_background,
    cull,
    paint::{DomBox, Turned},
    scrollbar,
    text::InlineText,
};
use crate::style::Prop;
use crate::{
    dom::Document,
    style::{self, ComputedStyles},
};
use renderer::{Bounds, Child, MouseButton, SharedString, img, point, px, relative, rgba, size};
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

pub(super) struct Build<'a> {
    pub(super) inputs: &'a rustc_hash::FxHashMap<usize, u64>,
    pub(super) doc: &'a Document,
    pub(super) styles: &'a ComputedStyles,
    pub(super) text: &'a renderer::TextSystem,
    pub(super) pixel_ratio: f32,
    pub(super) view: &'a Rc<PageState>,
    pub(super) ancestors: Rc<[usize]>,
    pub(super) flex_parent: bool,
    pub(super) measured: &'a RefCell<super::grid::Measurements>,
    pub(super) inline_text:
        &'a RefCell<rustc_hash::FxHashMap<usize, Option<Rc<style::InlineText>>>>,
    pub(super) perspective: Option<(f32, String)>,
    pub(super) siblings: &'a RefCell<HashMap<(usize, bool), Rc<Siblings>>>,
    pub(super) margins: &'a RefCell<Margins>,
    pub(super) blocks: &'a RefCell<rustc_hash::FxHashMap<usize, [f32; 2]>>,
    pub(super) ids: &'a Rc<super::effects::Ids>,
    pub(super) info: &'a [cull::Info],
    pub(super) zone: Option<Rc<cull::Zone>>,
    pub(super) top_layer: &'a [usize],
    pub(super) tree: &'a renderer::Tree,
    pub(super) culls: &'a std::cell::Cell<usize>,
    pub(super) resized: bool,
    pub(super) settled: bool,
}

#[derive(PartialEq)]
pub(super) struct Reuse {
    subtree: u64,
    containing: [f32; 2],
    container: [f32; 2],
    flex_parent: bool,
    grid_parent: bool,
    margin: [f32; 4],
    scrolled: bool,
    inputs: u64,
    cell: Option<([f32; 2], bool, bool, u64)>,
}

pub(super) fn own_inputs(
    doc: &Document,
    styles: &ComputedStyles,
    dom: &crate::dom::State,
    ids: &super::effects::Ids,
    id: usize,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = rustc_hash::FxHasher::default();
    if let Some(size) = dom.atoms.get(&id) {
        size.map(f32::to_bits).hash(&mut hasher);
    }
    if doc.nodes[id].tag == "svg" {
        let mut pending = vec![id];
        let mut visited = rustc_hash::FxHashSet::default();
        while let Some(node) = pending.pop() {
            if !visited.insert(node) {
                continue;
            }
            (
                node,
                doc.nodes.revision(node),
                Rc::as_ptr(&styles.values[node]),
            )
                .hash(&mut hasher);
            let current = &doc.nodes[node];
            pending.extend(&current.children);
            if current.tag == "use"
                && let Some(name) = current
                    .attrs
                    .get("href")
                    .or_else(|| current.attrs.get("xlink:href"))
                    .and_then(|value| value.strip_prefix('#'))
            {
                pending.extend(
                    super::effects::index(ids, doc)
                        .get(name)
                        .into_iter()
                        .flatten(),
                );
            }
        }
        return hasher.finish();
    }
    if styles.values[id]
        .get(Prop::DISPLAY)
        .is_some_and(|value| matches!(value, "grid" | "inline-grid"))
    {
        // WARNING: grids read only the axes they track, and a size that prepaint would not draw again for is
        // the size they were built with.
        if let (Some(axes), Some(sizes)) = (dom.flexed_grids.get(&id), dom.cull.widths.get(&id)) {
            for (axis, size) in axes.iter().zip(sizes) {
                if let Some((used, _)) = axis {
                    let size = if (size - used).abs() <= 0.5 {
                        used
                    } else {
                        size
                    };
                    size.to_bits().hash(&mut hasher);
                }
            }
        }
        for child in &doc.nodes[id].children {
            for baseline in dom.baselines.get(child).into_iter().flatten() {
                baseline.map(f32::to_bits).hash(&mut hasher);
            }
        }
    }
    if !styles.values[id]
        .get(Prop::POSITION)
        .is_some_and(|value| matches!(value, "absolute" | "fixed" | "sticky"))
    {
        return hasher.finish();
    }
    let mut ancestor = doc.nodes[id].parent;
    if let Some(parent) = ancestor {
        Rc::as_ptr(&styles.values[parent]).hash(&mut hasher);
    }
    while let Some(block) = ancestor {
        if let Some(bounds) = dom.blocks.get(&block) {
            (block, f32::from(bounds.size.width).to_bits()).hash(&mut hasher);
            f32::from(bounds.size.height).to_bits().hash(&mut hasher);
        }
        ancestor = doc.nodes[block]
            .parent
            .or_else(|| dom.hosts.get(&block).copied());
    }
    for value in styles.lengths[id].viewport {
        value.to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

type Siblings = (Vec<usize>, rustc_hash::FxHashMap<usize, usize>);

const SVG_SOURCE: [Prop; 13] = [
    Prop::TRANSLATE,
    Prop::ROTATE,
    Prop::SCALE,
    Prop::TRANSFORM_BOX,
    Prop::CX,
    Prop::CY,
    Prop::R,
    Prop::RX,
    Prop::RY,
    Prop::X,
    Prop::Y,
    Prop::WIDTH,
    Prop::HEIGHT,
];

struct SvgSource<'a> {
    doc: &'a Document,
    styles: &'a ComputedStyles,
    root: usize,
    scale: f32,
    view: [f32; 2],
    header: String,
    ids: &'a super::effects::Ids,
}

impl SvgSource<'_> {
    fn attrs(&self, element: usize, paint: bool, transform: bool) -> Vec<(&'static str, String)> {
        let (doc, root) = (self.doc, self.root);
        let own = &self.styles.values[element];
        let units = &self.styles.lengths[element];
        let tag = doc.nodes[element].tag.as_str();
        let mut attrs = Vec::new();
        if element != root {
            for prop in &SVG_SOURCE[4..] {
                let applies = match *prop {
                    Prop::CX | Prop::CY => matches!(tag, "circle" | "ellipse"),
                    Prop::R => tag == "circle",
                    Prop::RX | Prop::RY => matches!(tag, "ellipse" | "rect"),
                    _ => matches!(tag, "rect" | "image" | "foreignObject"),
                };
                if let Some(value) = own.get(*prop).filter(|value| applies && *value != "auto") {
                    let value = if value.ends_with('%') || value.parse::<f32>().is_ok() {
                        value.to_owned()
                    } else {
                        units
                            .resolve(value, f32::NAN)
                            .map_or_else(|| value.to_owned(), |length| length.to_string())
                    };
                    attrs.push((prop.name(), value));
                }
            }
        }
        // WARNING: resvg doesn't take CSS transforms, `transform-box` or `transform-origin`, so they become a matrix.
        if transform
            && element != root
            && [Prop::TRANSFORM, Prop::TRANSLATE, Prop::ROTATE, Prop::SCALE]
                .iter()
                .any(|&prop| own.get(prop).is_some())
        {
            let fill = own
                .get(Prop::TRANSFORM_BOX)
                .is_some_and(|value| value != "view-box");
            let [x, y, w, h] = fill
                .then(|| self.shape(element))
                .flatten()
                .map_or([0., 0., self.view[0], self.view[1]], |(rect, _)| rect);
            let bounds = Bounds::new(point(px(x), px(y)), size(px(w), px(h)));
            attrs.push((
                "transform",
                super::effects::transform(own, units, bounds, None).map_or(String::new(), |m| {
                    format!(
                        "matrix({} {} {} {} {} {})",
                        m[0], m[1], m[4], m[5], m[12], m[13]
                    )
                }),
            ));
        }
        if !paint {
            return attrs;
        }
        let parent = doc.nodes[element]
            .parent
            .map(|parent| &self.styles.values[parent]);
        // WARNING: `vector-effect: non-scaling-stroke` draws strokes in screen pixels, which resvg doesn't do, so their
        // widths and dashes scale back by the viewBox scale. resvg ignores `pathLength`, so dashes scale by the
        // real length over it.
        let width = if element != root
            && self.scale != 1.
            && own.get(Prop::VECTOR_EFFECT) == Some("non-scaling-stroke")
        {
            1. / self.scale
        } else {
            1.
        };
        let dashed = own
            .get(Prop::STROKE_DASHARRAY)
            .is_some_and(|value| value != "none")
            || own
                .get(Prop::STROKE_DASHOFFSET)
                .is_some_and(|value| value.trim_end_matches("px").parse::<f32>() != Ok(0.));
        let dashes = width
            * doc.nodes[element]
                .attrs
                .get("pathLength")
                .and_then(|value| value.trim().parse::<f32>().ok())
                .filter(|length| *length > 0. && dashed && element != root)
                .and_then(|length| Some(self.shape(element)?.1 / length))
                .filter(|ratio| *ratio > 0.)
                .unwrap_or(1.);
        let mut result = String::new();
        for prop in style::SVG {
            let Some(value) = own.get(prop) else { continue };
            let factor = match prop {
                Prop::TRANSFORM | Prop::TRANSFORM_ORIGIN => continue,
                Prop::STROKE_WIDTH => width,
                Prop::STROKE_DASHARRAY | Prop::STROKE_DASHOFFSET => dashes,
                _ => 1.,
            };
            if if element == root {
                !prop.inherited()
            } else {
                prop.inherited()
                    && factor == 1.
                    && !doc.nodes[element].attrs.contains_key(prop.name())
                    && parent.and_then(|parent| parent.get(prop)) == Some(value)
            } {
                continue;
            }
            let value = match prop {
                Prop::DISPLAY if value != "none" => continue,
                Prop::CLIP_PATH | Prop::MASK | Prop::FILTER if !value.starts_with("url(") => {
                    continue;
                }
                Prop::VECTOR_EFFECT => continue,
                Prop::FONT_SIZE => format!("{}px", units.font.em),
                // WARNING: resvg resolves `currentColor` with the `color` the element inherits in the source, which for
                // `<use>` content is the `<use>`'s color, not the color where the content is defined.
                Prop::FILL
                | Prop::STROKE
                | Prop::STOP_COLOR
                | Prop::FLOOD_COLOR
                | Prop::LIGHTING_COLOR
                    if value.eq_ignore_ascii_case("currentcolor") =>
                {
                    "currentColor".to_owned()
                }
                Prop::FILL
                | Prop::STROKE
                | Prop::COLOR
                | Prop::STOP_COLOR
                | Prop::FLOOD_COLOR
                | Prop::LIGHTING_COLOR => style::color(value)
                    .map_or(value.to_owned(), |(rgb, alpha)| {
                        format!("#{rgb:06x}{alpha:02x}")
                    }),
                _ if factor != 1. => value
                    .split(|c: char| c == ',' || c.is_ascii_whitespace())
                    .filter(|part| !part.is_empty())
                    .map(|part| {
                        part.strip_suffix("px")
                            .unwrap_or(part)
                            .parse::<f32>()
                            .map_or(part.to_owned(), |length| (length * factor).to_string())
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
                _ => value.to_owned(),
            };
            result.push_str(prop.name());
            result.push(':');
            result.push_str(&value);
            result.push(';');
        }
        if !result.is_empty() {
            attrs.push(("style", result));
        }
        attrs
    }

    fn shape(&self, element: usize) -> Option<([f32; 4], f32)> {
        let attrs = |node| {
            let mut attrs = self.attrs(node, false, node != element);
            // WARNING: resvg drops shapes without paint.
            attrs.push(("fill", "#000".into()));
            if node == element {
                attrs.extend([("id", "apps-shape".into()), ("transform", String::new())]);
                if self.doc.nodes[node].tag == "use" {
                    attrs.extend([("x", String::new()), ("y", String::new())]);
                }
            }
            attrs
        };
        let (mut targets, mut pending) = (Vec::new(), vec![element]);
        while let Some(node) = pending.pop() {
            let node = &self.doc.nodes[node];
            if node.tag == "use"
                && let Some(target) = node
                    .attrs
                    .get("href")
                    .or_else(|| node.attrs.get("xlink:href"))
                    .and_then(|value| value.strip_prefix('#'))
                    .and_then(|name| {
                        super::effects::index(self.ids, self.doc)
                            .get(name)?
                            .first()
                            .copied()
                    })
                && !targets.contains(&target)
            {
                targets.push(target);
                pending.push(target);
            }
            pending.extend(&node.children);
        }
        let mut source = format!("{}><defs>", self.header);
        for target in targets {
            source.push_str(&self.doc.serialize_svg(target, false, &attrs));
        }
        source.push_str("</defs>");
        source.push_str(&self.doc.serialize_svg(element, false, &attrs));
        source.push_str("</svg>");
        super::canvas::svg_shape(&source)
    }
}
pub(super) type Svg = (
    Vec<Rc<style::Style>>,
    ([f32; 2], u64),
    String,
    crate::dom::SvgBoxes,
);
pub(super) type Margins = (
    rustc_hash::FxHashMap<usize, Rc<rustc_hash::FxHashMap<usize, [f32; 2]>>>,
    rustc_hash::FxHashMap<usize, ([f32; 2], [f32; 2], bool)>,
);

/// The geometry revision and width an `adjoining` result was made with.
pub(super) type Adjoined = ((u64, u32), ([f32; 2], [f32; 2], bool));

pub(super) type Walked = (
    (u64, u32, [bool; 2]),
    Rc<rustc_hash::FxHashMap<usize, [f32; 2]>>,
);

thread_local! {
    // WARNING: the lowest stack address that building boxes may reach: a quarter of the stack stays
    // for layout and paint, which recurse as deep.
    static STACK_LIMIT: usize = {
        let marker = 0u8;
        let here = &raw const marker as usize;
        #[cfg(target_os = "macos")]
        let (low, size) = unsafe {
            let size = libc::pthread_get_stacksize_np(libc::pthread_self());
            (libc::pthread_get_stackaddr_np(libc::pthread_self()) as usize - size, size)
        };
        #[cfg(target_os = "linux")]
        let (low, size) = unsafe {
            let mut attr = std::mem::zeroed();
            let (mut low, mut size) = (std::ptr::null_mut(), 0);
            if libc::pthread_getattr_np(libc::pthread_self(), &mut attr) == 0 {
                libc::pthread_attr_getstack(&attr, &mut low, &mut size);
                libc::pthread_attr_destroy(&mut attr);
            }
            (low as usize, size)
        };
        // WARNING: Windows builds link a 64 MB main stack, see build.rs.
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let (low, size) = (here.saturating_sub(60 << 20), 64 << 20);
        (low + size / 4).min(here)
    };
}

pub(super) fn element(
    build: &Build<'_>,
    id: usize,
    containing: [f32; 2],
    container: [f32; 2],
    slot: Option<([f32; 4], super::grid::Grid)>,
) -> Child {
    let info = build.info[id];
    // WARNING: a 3D parent resolves the box's transform with its own units, so such boxes build again.
    if build.perspective.is_some()
        || build.doc.nodes[id].tag.starts_with('#')
        || build.ancestors.len() >= 256
        || info.flags
            & (cull::ALWAYS_REBUILT
                | cull::ANIMATED
                | cull::HIDDEN
                | cull::MOVES_CONTENT
                | cull::COMPOSITED
                | cull::TRANSFORMED
                | if build.resized {
                    cull::READS_VIEWPORT
                } else {
                    0
                })
            != 0
    {
        return create(build, id, containing, container, slot);
    }
    // WARNING: a kept subtree culls nothing, so while culling is on only subtrees that were within the
    // culling margin of the visible range keep their boxes. Others build again and cull their offscreen children.
    if let Some(zone) = &build.zone {
        let margin = build.styles.lengths[id].viewport[1];
        let host = build.view.host.borrow();
        let inside = host
            .dom
            .get(&build.view.id)
            .and_then(|dom| dom.bounds.get(&id))
            .is_some_and(|&[_, y, _, height]| {
                y >= zone.top - margin && y + height <= zone.bottom + margin
            });
        drop(host);
        if !inside {
            return create(build, id, containing, container, slot);
        }
    }
    let mut units = build.styles.lengths[id];
    units.container = Some(container);
    let key = Reuse {
        subtree: info.subtree,
        containing: if info.flags & cull::READS_HEIGHT != 0 {
            containing
        } else {
            [containing[0], 0.]
        },
        container,
        flex_parent: build.flex_parent,
        grid_parent: build.doc.nodes[id].parent.is_some_and(|parent| {
            build.styles.values[parent]
                .get(Prop::DISPLAY)
                .is_some_and(|value| matches!(value, "grid" | "inline-grid"))
        }),
        margin: margins(
            build,
            id,
            &units,
            containing[0],
            build.flex_parent,
            slot.is_some(),
        ),
        scrolled: [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
            .into_iter()
            .any(|prop| {
                build.styles.values[id]
                    .get(prop)
                    .is_some_and(|value| value != "visible")
            })
            && build
                .view
                .host
                .borrow()
                .dom
                .get(&build.view.id)
                .is_some_and(|dom| dom.scroll.contains_key(&id)),
        inputs: build.inputs.get(&id).copied().unwrap_or(0),
        cell: slot.as_ref().map(|(bounds, grid)| {
            use std::hash::{Hash, Hasher};
            let mut tracks = rustc_hash::FxHasher::default();
            for value in grid.columns.iter().chain(&grid.rows).chain(&grid.gap) {
                value.to_bits().hash(&mut tracks);
            }
            (
                [bounds[2], bounds[3]],
                grid.baseline,
                grid.first_item.is_some(),
                tracks.finish(),
            )
        }),
    };
    if (build.settled || info.flags & cull::READS_LAST_FRAME == 0)
        && build.tree.contains(id)
        && build.view.kept.borrow().get(&id) == Some(&key)
    {
        return Child::Kept(id.into());
    }
    let culls = build.culls.get();
    let child = create(build, id, containing, container, slot);
    let mut kept = build.view.kept.borrow_mut();
    // WARNING: culled runs depend on the scroll position, so a subtree with spacers is built again.
    if build.culls.get() == culls {
        kept.insert(id, key);
    } else {
        kept.remove(&id);
    }
    child
}

fn create(
    build: &Build<'_>,
    id: usize,
    containing: [f32; 2],
    container: [f32; 2],
    slot: Option<([f32; 4], super::grid::Grid)>,
) -> Child {
    let Build {
        blocks,
        inputs,
        doc,
        styles,
        text,
        pixel_ratio,
        view,
        ref ancestors,
        flex_parent,
        measured,
        inline_text,
        perspective: ref parent_perspective,
        siblings,
        margins: margin_cache,
        ids,
        info,
        ref zone,
        top_layer,
        tree,
        culls,
        resized,
        settled,
    } = *build;
    let node = &doc.nodes[id];
    let style = &styles.values[id];
    let display =
        style
            .get(Prop::DISPLAY)
            .unwrap_or(if node.tag.starts_with('#') && node.tag != "#text" {
                "block"
            } else {
                "inline"
            });
    let display = if display == "inline" && style::holds_block_box(doc, styles, id) {
        "block"
    } else {
        display
    };
    let order = style
        .get(Prop::Z_INDEX)
        .and_then(|value| value.parse::<i32>().ok());
    let positioned = style
        .get(Prop::POSITION)
        .is_some_and(|value| value != "static");
    let layered = positioned
        || flex_parent
        || node.parent.is_some_and(|parent| {
            styles.values[parent]
                .get(Prop::DISPLAY)
                .is_some_and(|value| matches!(value, "grid" | "inline-grid"))
        });
    // WARNING: a box whose opacity animates keeps one stacking context and effect layer at every
    // value, so animation frames only patch the layer's opacity.
    let composited = info[id].flags & cull::COMPOSITED != 0;
    let stacking = id == 0
        || composited
        || style
            .get(Prop::ISOLATION)
            .is_some_and(|value| value == "isolate")
        || style
            .get(Prop::POSITION)
            .is_some_and(|value| matches!(value, "fixed" | "sticky"))
        || layered && order.is_some()
        || style
            .get(Prop::TRANSFORM)
            .is_some_and(|value| value != "none")
        || [Prop::FILTER, Prop::CLIP_PATH]
            .into_iter()
            .any(|key| style.get(key).is_some_and(|value| value != "none"))
        || style
            .get(Prop::OPACITY)
            .and_then(|value| value.parse::<f32>().ok())
            .is_some_and(|value| value < 1.);
    let chain: Rc<[usize]> = ancestors.iter().copied().chain([id]).collect();
    let child_build = Build {
        inputs,
        doc,
        styles,
        text,
        pixel_ratio,
        view,
        measured,
        inline_text,
        siblings,
        margins: margin_cache,
        blocks,
        ids,
        info,
        top_layer,
        tree,
        culls,
        resized,
        settled,
        zone: zone
            .as_ref()
            .filter(|_| info[id].flags & (cull::CHANGED | cull::MOVES_CONTENT) == 0)
            .and_then(|zone| {
                if style
                    .get(Prop::OVERFLOW_Y)
                    .is_none_or(|value| value == "visible")
                {
                    return Some(zone.clone());
                }
                let rect = *view.host.borrow().dom.get(&view.id)?.bounds.get(&id)?;
                Some(Rc::new(cull::Zone {
                    top: zone.top.max(rect[1]),
                    bottom: zone.bottom.min(rect[1] + rect[3]),
                    clips: zone.clips.iter().copied().chain([id]).collect(),
                    resized: zone.resized,
                }))
            }),
        ancestors: chain.clone(),
        flex_parent: matches!(display, "flex" | "inline-flex"),
        perspective: style
            .get(Prop::PERSPECTIVE)
            .filter(|value| *value != "none")
            .and_then(|value| styles.lengths[id].resolve(value, 0.))
            .map(|distance| {
                (
                    distance,
                    style
                        .get(Prop::PERSPECTIVE_ORIGIN)
                        .map(str::to_owned)
                        .unwrap_or_else(|| "50% 50%".into()),
                )
            }),
    };
    let build = &child_build;
    let mut units = styles.lengths[id];
    units.container = Some(container);
    let position = style.get(Prop::POSITION).unwrap_or("static");
    let mut parent = node.parent;
    while let Some(ancestor) = parent
        && styles.values[ancestor]
            .get(Prop::DISPLAY)
            .is_some_and(|value| value == "contents")
    {
        parent = doc.nodes[ancestor].parent;
    }
    let contains = |style: &style::Style, fixed: bool| {
        !fixed
            && style
                .get(Prop::POSITION)
                .is_some_and(|value| value != "static")
            || [
                Prop::TRANSFORM,
                Prop::FILTER,
                Prop::PERSPECTIVE,
                Prop::BACKDROP_FILTER,
            ]
            .into_iter()
            .any(|prop| style.get(prop).is_some_and(|value| value != "none"))
            || style.get(Prop::CONTAIN).is_some_and(|value| {
                ["paint", "layout", "strict", "content"]
                    .iter()
                    .any(|kind| value.contains(kind))
            })
            || style
                .get(Prop::WILL_CHANGE)
                .is_some_and(|value| value.contains("transform"))
    };
    let positioner = matches!(position, "absolute" | "fixed")
        .then(|| {
            ancestors
                .iter()
                .rev()
                .copied()
                .find(|&ancestor| contains(&styles.values[ancestor], position == "fixed"))
                .or((position == "absolute" && id != 0).then_some(0))
        })
        .flatten();
    let block_size = if position == "fixed" && positioner.is_none() {
        Some(units.viewport)
    } else {
        positioner
            .filter(|&block| Some(block) != parent)
            .and_then(|block| {
                if block == 0 {
                    return Some(units.viewport);
                }
                if let Some(&size) = blocks.borrow().get(&block) {
                    return Some(size);
                }
                let host = view.host.borrow();
                let bounds = host.dom.get(&view.id)?.blocks.get(&block)?.size;
                Some([f32::from(bounds.width), f32::from(bounds.height)])
            })
    };
    let containing = block_size.unwrap_or(containing);
    // WARNING: element building and layout recurse per level, so deep pages must not overflow the stack.
    if chain.len() > 256 || style.get(Prop::DISPLAY).is_some_and(|s| s == "none") {
        return renderer::node().hidden().into();
    }
    let opacity = super::effects::opacity(style);
    let effect_layer = ([
        Prop::FILTER,
        Prop::CLIP_PATH,
        Prop::MASK_IMAGE,
        Prop::WEBKIT_MASK_IMAGE,
        Prop::BACKDROP_FILTER,
        Prop::WEBKIT_BACKDROP_FILTER,
    ]
    .into_iter()
    .any(|name| style.get(name).is_some_and(|value| value != "none"))
        || style
            .get(Prop::MIX_BLEND_MODE)
            .is_some_and(|value| value != "normal")
        || style
            .get(Prop::ISOLATION)
            .is_some_and(|value| value == "isolate")
        || cfg!(target_os = "macos")
            && matches!(style.get(Prop::MACOS_GLASS), Some("regular" | "clear"))
        || opacity < 1. && !node.children.is_empty()
        || composited)
        .then_some(opacity);
    let selection_colors = style::selection_colors(style, styles.selections.get(&id));
    let face = style::font(style);
    let line_height = match style::line_height(style) {
        style::LineHeight::Fixed(_) => (units.font.lh * pixel_ratio * 64.).round(),
        _ => (units.font.lh * pixel_ratio * 64.).floor(),
    } / 64.
        / pixel_ratio;
    let above = || {
        f32::from(text.baseline(
            text.resolve_font(&style::font(style)),
            px(units.font.em),
            px(line_height),
            pixel_ratio,
        ))
    };
    let strut = || {
        let above = above();
        renderer::node()
            .w(px(0.))
            .h(px(above))
            .mb(px(line_height - above))
            .flex_shrink_0()
            .baseline(renderer::Baseline::Bottom)
    };
    let mut el = renderer::node()
        .id(id)
        .flex()
        .flex_col()
        .text_size(px(units.font.em))
        .font_family(face.family)
        .font_weight(face.weight)
        .line_height(px(line_height));
    el.style().flex_shrink = if flex_parent { 1. } else { 0. };
    el.style().baseline = match display {
        "inline-block" if !flex_parent && slot.is_none() => renderer::Baseline::Last,
        "flex" | "inline-flex" | "grid" | "inline-grid" => renderer::Baseline::First,
        "table" | "inline-table" => renderer::Baseline::FirstOnly,
        _ => renderer::Baseline::Auto,
    };
    {
        let text = el.text_style();
        text.letter_spacing = Some(style::spacing(style, &units, Prop::LETTER_SPACING));
        text.word_spacing = Some(style::spacing(style, &units, Prop::WORD_SPACING));
        text.font_smoothing = Some(
            ![Prop::WEBKIT_FONT_SMOOTHING, Prop::MOZ_OSX_FONT_SMOOTHING]
                .into_iter()
                .any(|name| {
                    style
                        .get(name)
                        .is_some_and(|v| v == "antialiased" || v == "grayscale")
                }),
        );
        text.text_indent = Some(px(style
            .get(Prop::TEXT_INDENT)
            .and_then(|value| units.resolve(value, containing[0]))
            .unwrap_or(0.)));
    }
    if face.style != renderer::FontStyle::Normal {
        el.text_style().font_style = Some(face.style);
    }
    if node.tag == "br" {
        let empty = node.parent.is_none_or(|parent| {
            let siblings = &doc.nodes[parent].children;
            siblings[..siblings.iter().position(|&child| child == id).unwrap_or(0)]
                .iter()
                .rfind(|&&child| {
                    let node = &doc.nodes[child];
                    node.tag != "#comment"
                        && !(node.tag == "#text" && node.text.trim_ascii().is_empty())
                })
                .is_none_or(|&child| doc.nodes[child].tag == "br")
        });
        let painter = view.clone();
        let ink = [Prop::CARET_COLOR, Prop::COLOR]
            .into_iter()
            .find_map(|prop| style::color(style.get(prop)?))
            .map_or(0x000000ff, |(color, alpha)| color << 8 | alpha as u32);
        let view = view.clone();
        let above = above();
        let mut br = renderer::canvas(
            move |bounds, _| {
                let view = &*view;
                view.host
                    .borrow_mut()
                    .dom
                    .entry(view.id)
                    .or_default()
                    .bounds
                    .insert(
                        id,
                        [
                            f32::from(bounds.left()),
                            f32::from(bounds.top()),
                            0.,
                            line_height,
                        ],
                    );
            },
            move |bounds, _, window| {
                let caret = painter.host.try_borrow().is_ok_and(|host| {
                    host.dom
                        .get(&painter.id)
                        .is_some_and(|dom| dom.selection_caret == Some(id))
                });
                if caret {
                    super::input::paint_caret(
                        &painter,
                        [id, 0, 0],
                        Bounds::new(bounds.origin, size(px(1.), px(line_height))),
                        rgba(ink),
                        window,
                    );
                }
            },
        )
        .h(px(above))
        .mb(px(line_height - above))
        .flex_shrink_0()
        .baseline(renderer::Baseline::Bottom);
        br = if empty { br.w_full() } else { br.w(px(0.)) };
        return br.into();
    }
    if stacking {
        el = el.stacking().child(renderer::layers(true));
    }
    let flow = !matches!(display, "flex" | "inline-flex" | "grid" | "inline-grid");
    let mut flat = Vec::new();
    let mut pending: Vec<usize> = if matches!(node.tag.as_str(), "svg" | "canvas" | "iframe") {
        Vec::new()
    } else {
        node.children.iter().rev().copied().collect()
    };
    while let Some(child) = pending.pop() {
        if styles.values[child]
            .get(Prop::DISPLAY)
            .is_some_and(|value| value == "contents")
        {
            pending.extend(doc.nodes[child].children.iter().rev());
        } else if !matches!(doc.nodes[child].tag.as_str(), "#comment" | "#doctype")
            && !(doc.nodes[child].tag == "br"
                && styles.values[child].get(Prop::DISPLAY) == Some("none"))
        {
            flat.push(child);
        }
    }
    let floated = |child: usize| {
        flow && float_side(&styles.values[child]).is_some()
            && styles.values[child]
                .get(Prop::POSITION)
                .is_none_or(|value| !matches!(value, "absolute" | "fixed"))
    };
    let inline_level = |child: usize| {
        doc.nodes[child].tag.starts_with('#')
            || doc.nodes[child].tag == "br"
            || !floated(child)
                && styles.values[child]
                    .get(Prop::DISPLAY)
                    .is_none_or(|value| value.starts_with("inline"))
                && !style::holds_block_box(doc, styles, child)
            || styles.values[child]
                .get(Prop::POSITION)
                .is_some_and(|value| matches!(value, "absolute" | "fixed"))
    };
    let breaks = display != "inline"
        && flat.iter().any(|&child| doc.nodes[child].tag == "br")
        && inline_text
            .borrow_mut()
            .entry(id)
            .or_insert_with(|| style::inline_text(doc, styles, id).map(Rc::new))
            .is_none();
    let all_inline = flow
        && !flat.is_empty()
        && !breaks
        && flat.iter().all(|&child| {
            inline_level(child) || styles.values[child].get(Prop::DISPLAY) == Some("none")
        });
    let rtl = style.get(Prop::DIRECTION) == Some("rtl");
    let line_justify = match (style.get(Prop::TEXT_ALIGN), rtl) {
        (Some("right" | "-webkit-right"), _)
        | (Some("end"), false)
        | (None | Some("start"), true) => Some(renderer::JustifyContent::End),
        (Some("center" | "-webkit-center"), _) => Some(renderer::JustifyContent::Center),
        _ => None,
    };
    let line_align = Some(renderer::AlignItems::Baseline);
    if all_inline || display == "inline" {
        el = el.flex_row().flex_wrap();
        el.style().justify_content = line_justify;
        el.style().align_items = line_align;
        let merged = !text_field(node)
            && !matches!(node.tag.as_str(), "svg" | "canvas" | "iframe")
            && inline_text
                .borrow_mut()
                .entry(id)
                .or_insert_with(|| style::inline_text(doc, styles, id).map(Rc::new))
                .is_some();
        if display != "inline"
            && !merged
            && flat.iter().any(|&child| {
                !doc.nodes[child].tag.starts_with('#')
                    && doc.nodes[child].tag != "br"
                    && styles.values[child].get(Prop::DISPLAY) != Some("inline")
                    && in_flow(doc, info, child)
                    && !style::makes_no_line(doc, styles, child)
            })
        {
            el = el.child(strut());
        }
    }
    if node.tag == "canvas" {
        let size = |name: &str, default: f32| {
            node.attrs
                .get(name)
                .and_then(|value| dimension(value, true))
                .unwrap_or(default)
        };
        el = el.w(px(size("width", 300.))).h(px(size("height", 150.)));
        let host = view.host.borrow();
        let state = host.dom.get(&view.id);
        if let Some(image) = view
            .images
            .borrow()
            .get(&id)
            .or_else(|| state?.images.get(&id))
        {
            el = el.child(img(image.clone()).size_full());
        }
        if let Some(canvas) = state.and_then(|state| state.canvases.get(&id)) {
            el = el.child(img(canvas.image.clone()).absolute().size_full());
        }
        drop(host);
    }
    let mut image_child = None;
    let mut cropped = None;
    if node.tag == "img" || node.tag == "video" {
        let (image, vector) = view
            .host
            .borrow()
            .dom
            .get(&view.id)
            .map_or((None, None), |state| {
                (
                    state.images.get(&id).cloned(),
                    state.svg_images.get(&id).cloned(),
                )
            });
        if let Some(natural) = image
            .as_ref()
            .map(|image| {
                let size = image.size();
                [u32::from(size.width) as f32, u32::from(size.height) as f32]
            })
            .or(vector.as_ref().map(|(_, natural)| *natural))
            .or((node.tag == "video").then_some([300., 150.]))
        {
            let attr = |name: &str| {
                node.attrs
                    .get(name)
                    .and_then(|value| dimension(value, false))
            };
            let auto = |prop| style.get(prop).is_none_or(|value| value == "auto");
            if image.is_none() && vector.is_none() {
                if auto(Prop::HEIGHT) && attr("height").is_none() {
                    el = el.h(px(natural[1]));
                }
            } else if natural[1] > 0. {
                el.style().aspect_ratio = Some(natural[0] / natural[1]);
                el = only_auto_ratio(el, style);
            }
            match (
                auto(Prop::WIDTH),
                auto(Prop::HEIGHT),
                attr("width"),
                attr("height"),
            ) {
                (true, _, Some(width), _) => el = el.w(px(width)),
                (true, true, None, None) => el = el.w(px(natural[0])),
                _ => {}
            }
            if auto(Prop::HEIGHT)
                && let Some(height) = attr("height")
            {
                el = el.h(px(height));
            }
            let fit = match style.get(Prop::OBJECT_FIT) {
                Some("contain") => renderer::ObjectFit::Contain,
                Some("cover") => renderer::ObjectFit::Cover,
                Some("none") => renderer::ObjectFit::None,
                Some("scale-down") => renderer::ObjectFit::ScaleDown,
                _ => renderer::ObjectFit::Fill,
            };
            if let Some(image) = image {
                if matches!(fit, renderer::ObjectFit::Cover | renderer::ObjectFit::None) {
                    cropped = Some((image, fit));
                } else {
                    image_child = Some(img(image).size_full().object_fit(fit));
                }
            } else if vector.is_some() {
                el = el.child(svg_layer(id, false, view.clone()));
            }
        } else {
            // WARNING: images that are loading or failed to decode keep only their attribute size.
            for (name, prop, horizontal) in [
                ("width", Prop::WIDTH, true),
                ("height", Prop::HEIGHT, false),
            ] {
                if style.get(prop).is_none_or(|value| value == "auto")
                    && let Some(length) = node
                        .attrs
                        .get(name)
                        .and_then(|value| dimension(value, false))
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
    if node.tag == "svg" {
        let [width, height] = [(Prop::WIDTH, "width"), (Prop::HEIGHT, "height")]
            .into_iter()
            .enumerate()
            .map(|(axis, (prop, attr))| match style.get(prop) {
                Some(value) => units.resolve(value, containing[axis]),
                None => node.attrs.get(attr).and_then(|v| dimension(v, false)),
            })
            .collect::<Vec<_>>()[..]
        else {
            unreachable!()
        };
        let ratio = node.attrs.get("viewBox").and_then(|value| {
            let parts: Vec<f32> = value
                .split(|c: char| c == ',' || c.is_ascii_whitespace())
                .filter_map(|part| part.parse().ok())
                .collect();
            (parts.len() == 4 && parts[2] > 0. && parts[3] > 0.).then(|| parts[2] / parts[3])
        });
        let [width, height] = match (width, height, ratio) {
            (None, Some(height), Some(ratio)) => [height * ratio, height],
            (Some(width), None, Some(ratio)) => [width, width / ratio],
            _ => [width.unwrap_or(24.), height.unwrap_or(24.)],
        }
        .map(|value| value.clamp(1., 4096.));
        // WARNING: a source embeds the SVG's subtree with the SVG properties of each element, so it is built
        // again when a node, the size or one of those values changes. The `<svg>` itself passes only inherited
        // values, so its own animated transform or opacity keeps the source.
        let mut scope = id;
        while let Some(parent) = doc.nodes[scope].parent {
            scope = parent;
        }
        let mut pending = vec![id];
        let mut visited = std::collections::HashSet::new();
        let mut targets = Vec::new();
        let mut painted = Vec::new();
        let mut hasher = rustc_hash::FxHasher::default();
        while let Some(child) = pending.pop() {
            if !visited.insert(child) {
                continue;
            }
            std::hash::Hash::hash(&(child, doc.nodes.revision(child)), &mut hasher);
            painted.push(styles.values[child].clone());
            let child = &doc.nodes[child];
            pending.extend(&child.children);
            if child.tag == "use"
                && let Some(reference) = child
                    .attrs
                    .get("href")
                    .or_else(|| child.attrs.get("xlink:href"))
                    .and_then(|value| value.strip_prefix('#'))
                && let Some(target) =
                    super::effects::index(ids, doc)
                        .get(reference)
                        .and_then(|targets| {
                            targets.iter().copied().find(|&target| {
                                let mut root = target;
                                while let Some(parent) = doc.nodes[root].parent {
                                    if parent == id {
                                        return false;
                                    }
                                    root = parent;
                                }
                                root == scope
                            })
                        })
                && !visited.contains(&target)
            {
                targets.push(target);
                pending.push(target);
            }
        }
        let key = ([width, height], std::hash::Hasher::finish(&hasher));
        let props = || style::SVG.iter().chain(&SVG_SOURCE);
        let kept = view
            .svgs
            .borrow()
            .get(&id)
            .and_then(|(old, at, source, boxes)| {
                (*at == key
                    && old.len() == painted.len()
                    && old
                        .iter()
                        .zip(&painted)
                        .enumerate()
                        .all(|(index, (old, new))| {
                            Rc::ptr_eq(old, new)
                                || props().all(|&prop| {
                                    index == 0 && !prop.inherited()
                                        || old.get(prop) == new.get(prop)
                                })
                        }))
                .then(|| (source.clone(), boxes.clone()))
            });
        let built = kept.is_none();
        let (source, boxes) = kept.unwrap_or_else(|| {
            let view_box = node
                .attrs
                .get("viewBox")
                .map(|value| {
                    value
                        .split(|c: char| c == ',' || c.is_ascii_whitespace())
                        .filter_map(|part| part.parse::<f32>().ok())
                        .collect::<Vec<_>>()
                })
                .filter(|parts| parts.len() == 4 && parts[2] > 0. && parts[3] > 0.);
            let mut header = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\""
            );
            for (key, value) in &node.attrs {
                if !matches!(
                    key.as_str(),
                    "width" | "height" | "xmlns" | "xmlns:xlink" | "style"
                ) && style::presentation(key).is_none()
                {
                    header.push_str(&format!(" {key}=\"{}\"", crate::dom::escape(value)));
                }
            }
            header.push_str(" xmlns:xlink=\"http://www.w3.org/1999/xlink\"");
            let svg = SvgSource {
                doc,
                styles,
                root: id,
                scale: view_box
                    .as_ref()
                    .map_or(1., |parts| (width / parts[2]).min(height / parts[3])),
                view: view_box
                    .as_ref()
                    .map_or([width, height], |parts| [parts[2], parts[3]]),
                header,
                ids,
            };
            let attrs = |element| svg.attrs(element, true, true);
            let mut source = svg.header.clone();
            for (name, value) in attrs(id) {
                source.push_str(&format!(" {name}=\"{}\"", crate::dom::escape(&value)));
            }
            source.push('>');
            source.push_str("<defs>");
            for &target in &targets {
                source.push_str(&doc.serialize_svg(target, false, &attrs));
            }
            source.push_str("</defs>");
            for child in &node.children {
                source.push_str(&doc.serialize_svg(*child, true, &attrs));
            }
            source.push_str("</svg>");
            // WARNING: marks count from 0 in the source, so a picture has one source on all nodes and shares its
            // raster. `names` starts with their nodes.
            let mut names = Vec::new();
            let mut counted = String::with_capacity(source.len());
            let mut rest = &*source;
            const MARK: &str = " id=\"apps-";
            while let Some(start) = rest.find(MARK) {
                let (before, after) = rest.split_at(start + MARK.len());
                counted.push_str(before);
                rest = after;
                if let Some(end) = after.find('"')
                    && let Ok(marked) = after[..end].parse()
                {
                    counted.push_str(&names.len().to_string());
                    names.push((String::new(), marked));
                    rest = &after[end..];
                }
            }
            counted.push_str(rest);
            let source = counted;
            let mut pending = node.children.clone();
            while let Some(child) = pending.pop() {
                if let Some(name) = doc.nodes[child].attrs.get("id") {
                    names.push((name.clone(), child));
                }
                pending.extend(&doc.nodes[child].children);
            }
            // WARNING: CSS keywords ignore case, but resvg only reads `currentColor`.
            let lower = source.to_ascii_lowercase();
            let mut end = 0;
            let mut replaced = String::with_capacity(source.len());
            for (start, _) in lower.match_indices("currentcolor") {
                replaced.push_str(&source[end..start]);
                replaced.push_str("currentColor");
                end = start + "currentcolor".len();
            }
            replaced.push_str(&source[end..]);
            let boxes = (String::new(), names, [0.; 2], Vec::new(), [0.; 4]);
            view.svgs
                .borrow_mut()
                .insert(id, (painted, key, replaced.clone(), boxes.clone()));
            (replaced, boxes)
        });
        let mut host = view.host.borrow_mut();
        let state = host.dom.entry(view.id).or_default();
        if built || !state.svg_sources.contains_key(&id) {
            // WARNING: paint only checks the raster size of inline SVGs, so a changed source drops the raster here.
            if state.svgs.get(&id).is_some_and(|(old, _)| *old != source)
                && let Some((_, image)) = state.svgs.remove(&id)
                && Arc::strong_count(&image) == 1
            {
                state.retired.push(image);
            }
            state.svg_boxes.insert(id, boxes);
            state.svg_sources.insert(id, source);
        }
        drop(host);
        el = el
            .w(px(width))
            .h(px(height))
            .child(svg_layer(id, true, view.clone()));
    }
    if node.tag == "iframe" {
        let auto = |prop: Prop| style.get(prop).is_none_or(|value| value == "auto");
        let ratio = style
            .get(Prop::ASPECT_RATIO)
            .and_then(aspect_ratio)
            .is_some();
        let [width, height] = [("width", 300.), ("height", 150.)].map(|(name, default)| {
            node.attrs
                .get(name)
                .and_then(|value| dimension(value, false))
                .unwrap_or(default)
        });
        if auto(Prop::WIDTH) && (!ratio || auto(Prop::HEIGHT)) {
            el = el.w(px(width));
        }
        if auto(Prop::HEIGHT) && !ratio {
            el = el.h(px(height));
        }
    }
    let input =
        (node.tag == "input").then(|| node.attrs.get("type").map_or("text", String::as_str));
    let check = input.is_some_and(|kind| {
        kind.eq_ignore_ascii_case("checkbox") || kind.eq_ignore_ascii_case("radio")
    });
    let editable = text_field(node);
    if input.is_some_and(|kind| kind.eq_ignore_ascii_case("file")) {
        let dark = style::dark(style);
        let [fill, edge] = if dark {
            [0x6b6b6b, 0x858585]
        } else {
            [0xefefef, 0x767676]
        };
        let mut button = renderer::node()
            .flex_shrink_0()
            .mr(px(4.))
            .pl(px(7.))
            .pr(px(7.))
            .pt(px(2.))
            .pb(px(2.))
            .border(px(1.))
            .border_color(rgba(edge << 8 | 0xff))
            .bg(rgba(fill << 8 | 0xff))
            .child("Choose File");
        button.style().corner_radii = renderer::Corners::all(px(2.).into()).into();
        el = el
            .flex_row()
            .items_center()
            .overflow_hidden()
            .child(button)
            .child(renderer::node().whitespace_nowrap().child("No file chosen"));
        if style.get(Prop::WIDTH).is_none_or(|value| value == "auto") {
            let face = text.resolve_font(&style::font(style));
            let em = px(units.font.em);
            let width = |label: &str| {
                let run = renderer::TextRun {
                    len: label.len(),
                    font: style::font(style),
                    color: renderer::black(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                    letter_spacing: px(0.),
                    word_spacing: px(0.),
                    font_size: None,
                    inset: Default::default(),
                    nowrap: false,
                };
                f32::from(text.shape_line(label, em, &[run], None).width)
            };
            let zero = text
                .ch_advance(face, em)
                .map_or(units.font.em / 2., f32::from);
            let content = width("Choose File") + 16. + 4. + width("No file chosen");
            el = el.w(px(layout_ceil((zero * 34.).max(content), pixel_ratio)));
        }
    }
    if input.is_some_and(|kind| {
        ["button", "submit", "reset"]
            .iter()
            .any(|other| kind.eq_ignore_ascii_case(other))
    }) {
        el = el.items_center().justify_center().child(SharedString::from(
            node.attrs.get("value").cloned().unwrap_or_else(|| {
                if input == Some("reset") {
                    "Reset".into()
                } else if input == Some("submit") {
                    "Submit".into()
                } else {
                    String::new()
                }
            }),
        ));
    }
    if editable && node.tag != "textarea" {
        let above = f32::from(text.baseline(
            text.resolve_font(&style::font(style)),
            px(units.font.em),
            px(units.font.lh),
            pixel_ratio,
        ));
        el.style().justify_content = Some(renderer::JustifyContent::Center);
        el = el.child(
            renderer::node()
                .w(px(0.))
                .h(px(above))
                .mb(px(units.font.lh - above))
                .flex_shrink_0()
                .baseline(renderer::Baseline::Bottom),
        );
    }
    if editable {
        let text = node.shown_text(
            node.current_value()
                .map_or_else(|| doc.text(id), str::to_owned),
        );
        let placeholder = node.attrs.get("placeholder").cloned().unwrap_or_default();
        let face = style::font(style);
        let font_size = units.font.em;
        let line_height = units.font.lh;
        let text_color = style
            .get(Prop::COLOR)
            .and_then(style::color)
            .map(|(c, a)| rgba((c << 8) | a as u32))
            .unwrap_or(renderer::black());
        let caret_color = style
            .get(Prop::CARET_COLOR)
            .and_then(style::color)
            .map(|(color, alpha)| rgba((color << 8) | alpha as u32))
            .unwrap_or(text_color);
        let placeholder_color = styles
            .text_pseudos
            .get(&id)
            .and_then(|pseudos| style::color(pseudos[3].as_ref()?.get(Prop::COLOR)?))
            .map(|(color, alpha)| rgba((color << 8) | alpha as u32))
            .unwrap_or(rgba(0x757575ff));
        let single_line = node.tag != "textarea";
        let padding = units.edges(style, style::PADDING, containing[0]);
        let empty = text.is_empty();
        let shown: Rc<str> = if empty { placeholder } else { text }.into();
        let run = renderer::TextRun {
            len: shown.len(),
            font: face,
            color: if empty { placeholder_color } else { text_color },
            background_color: None,
            underline: None,
            strikethrough: None,
            letter_spacing: style::spacing(style, &units, Prop::LETTER_SPACING),
            word_spacing: px(0.),
            font_size: None,
            inset: Default::default(),
            nowrap: false,
        };
        if !single_line && style.get(Prop::FIELD_SIZING) == Some("content") {
            let mut sizer = renderer::text(shown.clone(), Some(vec![run.clone()]))
                .min_w(px(0.))
                .min_h(px(line_height))
                .flex_shrink_0();
            sizer.style().visibility = renderer::Visibility::Hidden;
            el = el.child(sizer);
        }
        // WARNING: the box that clips the text scrolls with the field's handle, so the field itself does not track it.
        let handle = view
            .host
            .borrow_mut()
            .dom
            .entry(view.id)
            .or_default()
            .scroll
            .entry(id)
            .or_default()
            .clone();
        let field = super::input::FieldText {
            selected: Default::default(),
            id,
            view: view.clone(),
            placeholder: empty,
            password: (node.password() && !empty)
                .then(|| node.current_value().unwrap_or("").to_owned()),
            text: shown,
            run,
            single_line,
            caret: caret_color,
            selection: selection_colors,
            handle: handle.clone(),
        };
        let mut clip = renderer::node()
            .absolute()
            .track_scroll(&handle)
            .text_size(px(font_size))
            .line_height(px(line_height));
        clip.style().restrict_scroll_to_axis = true;
        clip.style().overscroll = overscroll(style);
        el = el
            .cursor(renderer::CursorStyle::IBeam)
            .child(if single_line {
                let line = renderer::node().flex_shrink_0().child(field);
                clip.top(px(padding[0]))
                    .right(px(padding[1]))
                    .bottom(px(padding[2]))
                    .left(px(padding[3]))
                    .overflow_hidden()
                    .flex()
                    .flex_row()
                    .items_center()
                    .whitespace_nowrap()
                    .child(match line_justify {
                        Some(renderer::JustifyContent::End) => line.ml_auto(),
                        Some(renderer::JustifyContent::Center) => line.ml_auto().mr_auto(),
                        _ => line,
                    })
            } else {
                clip.style().overflow.x = renderer::Overflow::Scroll;
                clip.top_0()
                    .right(px(0.))
                    .bottom(px(0.))
                    .left_0()
                    .pt(px(padding[0]))
                    .pr(px(padding[1]))
                    .pb(px(padding[2]))
                    .pl(px(padding[3]))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .child(field)
            });
    }
    if let Some((ink, alpha)) = style.get(Prop::COLOR).and_then(style::color) {
        el = el.text_color(rgba((ink << 8) | alpha as u32));
    }
    let background = |style: &style::Style| {
        style
            .get(Prop::BACKGROUND_COLOR)
            .and_then(style::color)
            .filter(|(_, alpha)| *alpha > 0)
    };
    let inline_box = display == "inline"
        && !node.tag.starts_with('#')
        && !flex_parent
        && slot.is_none()
        && !matches!(
            node.tag.as_str(),
            "img" | "svg" | "canvas" | "input" | "button" | "select" | "textarea" | "iframe"
        )
        && !style
            .get(Prop::POSITION)
            .is_some_and(|value| matches!(value, "absolute" | "fixed"));
    let mut inline_fill = renderer::transparent_black();
    let mut cell_background = background(style);
    if cell_background.is_none() && matches!(node.tag.as_str(), "td" | "th") {
        let mut parent = node.parent;
        while let Some(row) = parent.filter(|&row| {
            matches!(
                doc.nodes[row].tag.as_str(),
                "tr" | "thead" | "tbody" | "tfoot"
            )
        }) {
            cell_background = background(&styles.values[row]);
            if cell_background.is_some() {
                break;
            }
            parent = doc.nodes[row].parent;
        }
    }
    if let Some(color) = cell_background {
        let color = rgba((color.0 << 8) | color.1 as u32);
        let blended = style
            .get(Prop::BACKGROUND_BLEND_MODE)
            .is_some_and(|value| value.split(',').any(|mode| mode.trim() != "normal"))
            && style
                .get(Prop::BACKGROUND_IMAGE)
                .is_some_and(|value| value != "none");
        if blended {
        } else if inline_box {
            inline_fill = color;
        } else {
            el = el.bg(color);
        }
    }
    let overflow = [Prop::OVERFLOW_X, Prop::OVERFLOW_Y].map(|prop| {
        style.get(prop).unwrap_or(if node.tag == "textarea" {
            "auto"
        } else {
            "visible"
        })
    });
    let page = matches!(node.tag.as_str(), "html" | "body");
    let boxed = !page
        && overflow
            .iter()
            .any(|value| !matches!(*value, "visible" | "clip"));
    let overflow = overflow.map(|value| match value {
        "auto" | "scroll" => renderer::Overflow::Scroll,
        "hidden" => renderer::Overflow::Hidden,
        "clip" if boxed => renderer::Overflow::Hidden,
        "clip" => renderer::Overflow::Clip,
        _ if boxed => renderer::Overflow::Scroll,
        _ => renderer::Overflow::Visible,
    });
    // WARNING: a form field's text clips and scrolls in its own box, so the field keeps its outline unclipped.
    if overflow != [renderer::Overflow::Visible; 2] && !editable {
        el.style().overflow.x = overflow[0];
        el.style().overflow.y = overflow[1];
    }
    let scrolls = overflow.map(|axis| axis == renderer::Overflow::Scroll);
    let look = if scrolls.contains(&true) {
        scrollbar::Look::new(
            style,
            styles.scrollbars.get(&id).map(|parts| &**parts),
            &units,
            chain.iter().rev().find_map(|&id| {
                let (ink, alpha) = style::color(styles.values[id].get(Prop::BACKGROUND_COLOR)?)?;
                (alpha >= 128).then_some(ink)
            }),
        )
        .map(|mut look| {
            if page {
                look.forced = [false; 2];
            }
            look
        })
    } else {
        None
    };
    let reserved = look.as_ref().map_or([0.; 2], |look| {
        let max = view
            .host
            .borrow()
            .dom
            .get(&view.id)
            .and_then(|dom| dom.scroll.get(&id))
            .map(renderer::ScrollHandle::max_offset)
            .unwrap_or_default();
        look.space(look.shown(scrolls, max))
    });
    let mut padding = units
        .edges(style, style::PADDING, containing[0])
        .map(|value| value.max(0.));
    padding[1] += reserved[1];
    padding[2] += reserved[0];
    let opposite = if style
        .get(Prop::SCROLLBAR_GUTTER)
        .is_some_and(|value| value.contains("both-edges"))
    {
        reserved[1]
    } else {
        0.
    };
    padding[3] += opposite;
    el = el
        .pt(px(padding[0]))
        .pr(px(padding[1]))
        .pb(px(padding[2]))
        .pl(px(padding[3]));
    let mut margin = margins(
        build,
        id,
        &units,
        containing[0],
        flex_parent,
        slot.is_some(),
    );
    el = el
        .mt(px(margin[0]))
        .mr(px(margin[1]))
        .mb(px(margin[2]))
        .ml(px(margin[3]));
    if let Some(fieldset) = node.parent
        && let Some((legend, height, _)) = legend(doc, styles, fieldset)
        && legend == id
    {
        let left =
            styles.lengths[fieldset].edges(&styles.values[fieldset], style::PADDING, containing[0])
                [3];
        el = el.absolute().top(px(-height)).left(px(left));
    }
    for (side, prop) in style::MARGIN[1..].iter().enumerate() {
        if style.get(*prop).is_some_and(|s| s == "auto") {
            el = match side {
                0 => el.mt_auto(),
                1 => el.mr_auto(),
                2 => el.mb_auto(),
                _ => el.ml_auto(),
            };
        }
    }
    // WARNING: right-to-left blocks put an over-constrained child at the right, so its left margin gives way.
    if !flex_parent
        && slot.is_none()
        && node
            .parent
            .is_some_and(|parent| styles.values[parent].get(Prop::DIRECTION) == Some("rtl"))
        && style.get(Prop::DISPLAY).is_some_and(|value| {
            matches!(
                value,
                "block" | "list-item" | "table" | "flex" | "grid" | "flow-root"
            )
        })
        && style.get(Prop::FLOAT).is_none_or(|value| value == "none")
        && style
            .get(Prop::POSITION)
            .is_none_or(|value| !matches!(value, "absolute" | "fixed"))
        && style.get(Prop::WIDTH).is_some_and(|value| value != "auto")
        && [Prop::MARGIN_LEFT, Prop::MARGIN_RIGHT]
            .into_iter()
            .all(|prop| style.get(prop).is_none_or(|value| value != "auto"))
    {
        el = el.ml_auto();
    }
    let ink = style
        .get(Prop::COLOR)
        .and_then(style::color)
        .map_or(rgba(0x000000ff), |(ink, alpha)| {
            rgba(ink << 8 | alpha as u32)
        });
    let sides = style::BORDERS.map(|[width, kind, color]| (width, kind, color));
    let mut borders = sides.map(|(width, kind, _)| {
        if style
            .get(kind)
            .is_none_or(|kind| matches!(kind, "none" | "hidden"))
        {
            return 0.;
        }
        style
            .get(width)
            .map_or(Some(3.), |width| units.resolve(width, containing[0]))
            .unwrap_or(0.)
            .max(0.)
    });
    let mut border_colors = sides.map(|(_, _, name)| {
        style
            .get(name)
            .and_then(style::color)
            .map_or(ink, |(ink, alpha)| rgba(ink << 8 | alpha as u32))
    });
    let mut border_kinds = sides.map(|(_, kind, _)| style.get(kind).unwrap_or("none"));
    if inline_box {
        let [above, below] = [padding[0] + borders[0], padding[2] + borders[2]];
        // WARNING: like browsers, padding and borders of an inline box hang outside its line.
        margin[0] = -above;
        margin[2] = -below;
        el = el.mt(px(margin[0])).mb(px(margin[2]));
        let same = border_colors.iter().all(|color| *color == border_colors[0]);
        let border = if same && borders.iter().any(|width| *width > 0.) {
            border_colors[0]
        } else {
            renderer::transparent_black()
        };
        if border.a > 0. || inline_fill.a > 0. {
            let font = text.resolve_font(&style::font(style));
            let size_px = px(units.font.em);
            let height = f32::from(text.ascent(font, size_px) - text.descent(font, size_px));
            let line_height = units.font.lh;
            let radii = super::effects::radii(style, &units, || containing);
            let (sides, pad_y, fill) = (borders, padding[0] + padding[2], inline_fill);
            let dashed = matches!(border_kinds[0], "dashed");
            el = el.child(
                renderer::canvas(
                    |_, _| (),
                    move |bounds, _, window| {
                        let step = px(line_height.max(1.));
                        let mask = window.content_mask().bounds;
                        let quad = size(
                            bounds.size.width + px(sides[1] + sides[3]),
                            px(height + above + below),
                        );
                        let start = bounds.top() + px((line_height - height) / 2. - sides[0]);
                        let lines =
                            (((bounds.size.height - px(pad_y)) / step).round() as usize).max(1);
                        for line in 0..lines {
                            let top = start + step * line as f32;
                            if top > mask.bottom() {
                                break;
                            }
                            if top + quad.height < mask.top() {
                                continue;
                            }
                            window.paint_quad(renderer::quad(
                                Bounds::new(point(bounds.left() - px(sides[3]), top), quad),
                                radii.resolve(quad),
                                fill,
                                renderer::Edges {
                                    top: px(sides[0]),
                                    right: px(sides[1]),
                                    bottom: px(sides[2]),
                                    left: px(sides[3]),
                                },
                                border,
                                if dashed {
                                    renderer::BorderStyle::Dashed
                                } else {
                                    renderer::BorderStyle::Solid
                                },
                            ));
                        }
                    },
                )
                .absolute()
                .left(px(0.))
                .top(px(0.))
                .size_full(),
            );
            if same {
                border_colors = [renderer::transparent_black(); 4];
            }
        }
    }
    let list = (node.tag == "select")
        .then(|| {
            match node
                .attrs
                .get("size")
                .and_then(|size| size.trim().parse::<usize>().ok())
            {
                Some(rows) if rows > 1 => Some(rows),
                _ => node.attrs.contains_key("multiple").then_some(4),
            }
        })
        .flatten();
    let native =
        super::control::kind(&node.tag, input, list.is_some(), style).map(|(control, boxed)| {
            let number = |name: &str| {
                node.attrs
                    .get(name)
                    .and_then(|value| value.trim().parse::<f32>().ok())
                    .filter(|value| value.is_finite())
            };
            let [min, max] = match control {
                super::control::Control::Range => {
                    [number("min").unwrap_or(0.), number("max").unwrap_or(100.)]
                }
                super::control::Control::Meter => {
                    [number("min").unwrap_or(0.), number("max").unwrap_or(1.)]
                }
                _ => [0., number("max").filter(|&max| max > 0.).unwrap_or(1.)],
            };
            let max = max.max(min);
            let value = match control {
                super::control::Control::Range => node
                    .current_value()
                    .and_then(|value| value.trim().parse::<f32>().ok())
                    .unwrap_or((min + max) / 2.),
                _ => number("value").unwrap_or(if control == super::control::Control::Progress {
                    -1.
                } else {
                    0.
                }),
            };
            let dark = style::dark(style);
            let accent = match control {
                super::control::Control::Meter => {
                    let low = number("low").unwrap_or(min).clamp(min, max);
                    let high = number("high").unwrap_or(max).clamp(low, max);
                    let region = |value: f32| u8::from(value >= low) + u8::from(value > high);
                    let optimum = region(
                        number("optimum")
                            .unwrap_or((min + max) / 2.)
                            .clamp(min, max),
                    );
                    let current = region(value.clamp(min, max));
                    let colors = if dark {
                        [0x74b374, 0xf2c812, 0xe98f6d]
                    } else {
                        [0x107c10, 0xffb900, 0xd83b01]
                    };
                    if optimum == current {
                        colors[0]
                    } else if optimum == 1 || current == 1 {
                        colors[1]
                    } else {
                        colors[2]
                    }
                }
                super::control::Control::Color => node
                    .current_value()
                    .and_then(style::color)
                    .map_or(0, |(color, _)| color),
                _ if node.attrs.contains_key("disabled") => 0x767676,
                _ => style
                    .get(Prop::ACCENT_COLOR)
                    .and_then(style::color)
                    .map_or(if dark { 0x99c8ff } else { 0x0075ff }, |(color, _)| color),
            };
            super::control::Look {
                control,
                boxed,
                checked: node.checked(),
                disabled: node.attrs.contains_key("disabled"),
                hover: node.state & crate::dom::HOVER != 0,
                active: node.state & crate::dom::ACTIVE != 0,
                dark,
                accent,
                ink: style
                    .get(Prop::COLOR)
                    .and_then(style::color)
                    .map_or(0, |(color, _)| color),
                value: if value < min || max <= min && control != super::control::Control::Progress
                {
                    if control == super::control::Control::Progress {
                        -1.
                    } else {
                        0.
                    }
                } else {
                    ((value - min) / (max - min).max(f32::EPSILON)).clamp(0., 1.)
                },
            }
        });
    if let Some(look) = &native {
        if look.boxed {
            let (edge, fill) = super::control::box_colors(look);
            for side in 0..4 {
                padding[side] += (borders[side] - 1.).max(0.);
                borders[side] = borders[side].min(1.);
                border_kinds[side] = "solid";
                if let Some(edge) = edge {
                    border_colors[side] = rgba(edge << 8 | 0xff);
                }
            }
            if let Some(fill) = fill {
                el = el.bg(rgba(fill << 8 | 0xff));
            }
        }
        if let Some(rows) = list
            && style.get(Prop::HEIGHT).is_none_or(|value| value == "auto")
        {
            el = el.h(px(rows as f32 * (units.font.lh + 1.).ceil()
                + padding[0]
                + padding[2]
                + borders[0]
                + borders[2]));
        }
        if look.control == super::control::Control::Menu {
            padding[1] += 16.;
            padding[3] += 4.;
            if list.is_none() && style.get(Prop::WIDTH).is_none_or(|value| value == "auto") {
                let (font, size) = (style::font(style), px(units.font.em));
                let mut widest = 0f32;
                let mut stack = node.children.clone();
                while let Some(child) = stack.pop() {
                    match doc.nodes[child].tag.as_str() {
                        "optgroup" => stack.extend(&doc.nodes[child].children),
                        "option" => {
                            let label = doc
                                .text(child)
                                .split_ascii_whitespace()
                                .collect::<Vec<_>>()
                                .join(" ");
                            let run = renderer::TextRun {
                                len: label.len(),
                                font: font.clone(),
                                color: renderer::black(),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                                letter_spacing: px(0.),
                                word_spacing: px(0.),
                                font_size: None,
                                inset: Default::default(),
                                nowrap: false,
                            };
                            widest = widest
                                .max(f32::from(text.shape_line(&label, size, &[run], None).width));
                        }
                        _ => {}
                    }
                }
                el = el.min_w(px((widest * pixel_ratio).ceil() / pixel_ratio
                    + padding[1]
                    + padding[3]
                    + borders[1]
                    + borders[3]));
            }
            if !node.attrs.contains_key("multiple")
                && node
                    .attrs
                    .get("size")
                    .is_none_or(|size| size.trim().parse::<u32>().is_ok_and(|size| size <= 1))
                && style.get(Prop::HEIGHT).is_none_or(|value| value == "auto")
            {
                let face = text.resolve_font(&style::font(style));
                let [ascent, descent, _] = text
                    .line_metrics(face, px(units.font.em * pixel_ratio))
                    .map(f32::from);
                el = el
                    .h(px((ascent + descent) / pixel_ratio
                        + 2.
                        + padding[0]
                        + padding[2]
                        + borders[0]
                        + borders[2]))
                    .justify_center();
            }
        }
        el = el
            .pt(px(padding[0]))
            .pr(px(padding[1]))
            .pb(px(padding[2]))
            .pl(px(padding[3]));
    }
    let legend = (node.tag == "fieldset")
        .then(|| legend(doc, styles, id))
        .flatten()
        .filter(|&(_, height, _)| height > borders[0]);
    if let Some((legend, height, middle)) = legend {
        el = el.child(fieldset_border(
            legend,
            height,
            middle,
            borders,
            border_colors,
            border_kinds,
            view.clone(),
        ));
        borders[0] = height;
        border_colors = [renderer::transparent_black(); 4];
        border_kinds = ["solid"; 4];
    }
    el = el
        .border_t(px(borders[0]))
        .border_r(px(borders[1]))
        .border_b(px(borders[2]))
        .border_l(px(borders[3]));
    let visible = (0..4)
        .filter(|&side| borders[side] > 0.)
        .collect::<Vec<_>>();
    let shaded = visible
        .iter()
        .any(|&side| matches!(border_kinds[side], "groove" | "ridge" | "inset" | "outset"));
    if let Some(&first) = visible.first() {
        if !shaded
            && visible
                .iter()
                .all(|&side| border_colors[side] == border_colors[first])
        {
            el = el.border_color(border_colors[first]);
        } else {
            el = el.border_color(renderer::transparent_black());
        }
        if visible
            .iter()
            .all(|&side| border_kinds[side] == border_kinds[first])
        {
            el.style().border_style = border_style(border_kinds[first]);
        } else if visible
            .iter()
            .any(|&side| matches!(border_kinds[side], "dashed" | "dotted"))
        {
            el.style().border_style = renderer::BorderStyle::Dashed;
        }
    }
    let extra = if style
        .get(Prop::BOX_SIZING)
        .is_some_and(|value| value == "border-box")
    {
        [0.; 2]
    } else {
        [
            padding[1] + padding[3] + borders[1] + borders[3] - reserved[1] - opposite,
            padding[0] + padding[2] + borders[0] + borders[2] - reserved[0],
        ]
    };
    if editable {
        let chrome = [
            padding[1] + padding[3] + borders[1] + borders[3],
            padding[0] + padding[2] + borders[0] + borders[2],
        ];
        let auto =
            [Prop::WIDTH, Prop::HEIGHT].map(|prop| style.get(prop).is_none_or(|v| v == "auto"));
        let percent = style.get(Prop::WIDTH).is_some_and(|v| v.ends_with('%'));
        let [width, height] = field_size(
            doc,
            id,
            style,
            &units,
            text,
            pixel_ratio,
            auto[0] || percent,
            None,
        );
        if auto[0] {
            el = el.w(px(width + chrome[0]));
        } else if percent {
            el = el.child(super::input::natural_width(px(width)));
        }
        if auto[1] && (node.tag != "textarea" || style.get(Prop::FIELD_SIZING) != Some("content")) {
            el = el.min_h(px(height + chrome[1]));
        }
    }
    let flexbox = matches!(display, "flex" | "inline-flex");
    let basis = || {
        [Prop::WIDTH, Prop::HEIGHT].map(|prop| {
            let base = containing[usize::from(prop == Prop::HEIGHT)];
            style
                .get(prop)
                .and_then(|length| units.resolve(length, base))
                .unwrap_or(base)
        })
    };
    let radius = super::effects::radii(style, &units, basis);
    el.style().corner_radii = radius;
    if native.as_ref().is_some_and(|look| look.boxed) {
        el.style().corner_radii = renderer::Corners::all(px(2.).into()).into();
    }
    let mut inner_shadows = Vec::new();
    let mut intrinsic_width = None;
    let stretched = flex_parent
        && node.parent.is_some_and(|parent| {
            let parent = &styles.values[parent];
            parent
                .get(Prop::FLEX_DIRECTION)
                .is_some_and(|value| value.starts_with("column"))
                && style
                    .get(Prop::ALIGN_SELF)
                    .filter(|value| *value != "auto")
                    .or_else(|| parent.get(Prop::ALIGN_ITEMS))
                    .is_none_or(|value| matches!(value, "normal" | "stretch"))
        });
    for (prop, value) in style.iter() {
        let value: &str = value;
        let vertical = matches!(
            prop,
            Prop::HEIGHT | Prop::MIN_HEIGHT | Prop::MAX_HEIGHT | Prop::TOP | Prop::BOTTOM
        );
        let base = containing[usize::from(vertical)];
        let length = || units.resolve(value, base);
        match prop {
            Prop::WIDTH
            | Prop::HEIGHT
            | Prop::MIN_WIDTH
            | Prop::MAX_WIDTH
            | Prop::MIN_HEIGHT
            | Prop::MAX_HEIGHT => {
                let axis = usize::from(vertical);
                let length: renderer::Length = if let Some(percent) = value
                    .strip_suffix('%')
                    .and_then(|s| s.parse::<f32>().ok())
                    .filter(|v| v.is_finite())
                {
                    match block_size {
                        Some(_) => px(percent.max(0.) / 100. * base + extra[axis]).into(),
                        None => renderer::Length::Calc(percent.max(0.) / 100., px(extra[axis])),
                    }
                } else if let Some((fraction, offset)) =
                    percent_calc(&units, value).filter(|_| block_size.is_none())
                {
                    renderer::Length::Calc(fraction, px(offset + extra[axis]))
                } else if let Some(length) = length() {
                    px(length.max(0.) + extra[axis]).into()
                } else if matches!(prop, Prop::WIDTH | Prop::HEIGHT)
                    && value == "auto"
                    && node.tag != "svg"
                    && !(editable && prop == Prop::WIDTH)
                    || prop == Prop::WIDTH
                        && node.tag == "table"
                        && value == "fit-content"
                        && stretched
                    || prop == Prop::WIDTH
                        && value == "fit-content"
                        && matches!(position, "absolute" | "fixed")
                {
                    renderer::Length::Auto
                } else if matches!(value, "stretch" | "-webkit-fill-available") {
                    relative(1.)
                } else if matches!(value, "fit-content" | "max-content" | "min-content")
                    && axis == 0
                {
                    let [min, max, ..] = super::grid::intrinsic(
                        doc,
                        id,
                        styles,
                        Some(containing[0]),
                        None,
                        text,
                        &mut measured.borrow_mut(),
                    );
                    let size = match value {
                        "min-content" => min,
                        "max-content" => max,
                        _ => max.min(min.max(containing[0])),
                    };
                    let size = layout_ceil((size - margin[1] - margin[3]).max(0.), pixel_ratio);
                    intrinsic_width = Some(size);
                    // WARNING: `containing` is only an estimate, so layout keeps a `fit-content` box inside its parent.
                    if value == "fit-content" && style.get(Prop::MAX_WIDTH).is_none() {
                        el = el.max_w(renderer::Length::Calc(1., px(-margin[1] - margin[3])));
                    }
                    px(size).into()
                } else {
                    continue;
                };
                el = match prop {
                    Prop::WIDTH => el.w(length),
                    Prop::HEIGHT => el.h(length),
                    Prop::MIN_WIDTH => el.min_w(length),
                    Prop::MAX_WIDTH => el.max_w(length),
                    Prop::MIN_HEIGHT => el.min_h(length),
                    _ => el.max_h(length),
                };
            }
            Prop::ROW_GAP | Prop::COLUMN_GAP if flexbox => {
                let Some(length) = length() else { continue };
                el = if prop == Prop::ROW_GAP {
                    el.gap_y(px(length.max(0.)))
                } else {
                    el.gap_x(px(length.max(0.)))
                };
            }
            Prop::BOX_SHADOW => {
                el.style().box_shadow = units.shadows(value, false);
                inner_shadows = units.shadows(value, true);
            }
            Prop::ASPECT_RATIO => {
                if let Some(ratio) = aspect_ratio(value) {
                    el.style().aspect_ratio = Some(ratio);
                    el = only_auto_ratio(el, style);
                }
            }
            Prop::DISPLAY if matches!(value, "grid" | "inline-grid") => el = el.grid(),
            Prop::DISPLAY if matches!(value, "flex" | "inline-flex") => {
                el = el.flex().flex_row();
                if rtl {
                    el.style().flex_direction = renderer::FlexDirection::RowReverse;
                }
            }
            Prop::FLEX_DIRECTION if flexbox => {
                el.style().flex_direction = match (value, rtl) {
                    ("row-reverse", false) | ("row", true) => renderer::FlexDirection::RowReverse,
                    ("column", _) => renderer::FlexDirection::Column,
                    ("column-reverse", _) => renderer::FlexDirection::ColumnReverse,
                    _ => renderer::FlexDirection::Row,
                }
            }
            Prop::FLEX_WRAP if flexbox => {
                el.style().flex_wrap = match value {
                    "wrap" => renderer::FlexWrap::Wrap,
                    "wrap-reverse" => renderer::FlexWrap::WrapReverse,
                    _ => renderer::FlexWrap::NoWrap,
                }
            }
            Prop::FLEX_GROW | Prop::FLEX_SHRINK if flex_parent => {
                if let Some(factor) = value
                    .parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite() && *v >= 0.)
                {
                    if prop == Prop::FLEX_GROW {
                        el.style().flex_grow = factor;
                    } else {
                        el.style().flex_shrink = factor;
                    }
                }
            }
            Prop::FLEX_BASIS if flex_parent => {
                let axis = usize::from(node.parent.is_some_and(|parent| {
                    styles.values[parent]
                        .get(Prop::FLEX_DIRECTION)
                        .is_some_and(|v| v.starts_with("column"))
                }));
                let base = containing[axis];
                el.style().flex_basis = if let Some(percent) = value
                    .strip_suffix('%')
                    .and_then(|v| v.parse::<f32>().ok())
                    .filter(|v| v.is_finite())
                {
                    renderer::Length::Calc(percent.max(0.) / 100., px(extra[axis]))
                } else if let Some(length) = units.resolve(value, base) {
                    px(length.max(0.) + extra[axis]).into()
                } else {
                    renderer::Length::Auto
                };
            }
            Prop::ALIGN_ITEMS | Prop::ALIGN_SELF if flex_parent || flexbox => {
                let align = match value {
                    "center" => renderer::AlignItems::Center,
                    "flex-start" | "start" | "self-start" => renderer::AlignItems::FlexStart,
                    "flex-end" | "end" | "self-end" => renderer::AlignItems::FlexEnd,
                    "baseline" | "first baseline" | "last baseline" => {
                        renderer::AlignItems::Baseline
                    }
                    "stretch" | "normal" => renderer::AlignItems::Stretch,
                    _ => continue,
                };
                if prop == Prop::ALIGN_SELF {
                    el.style().align_self = Some(align);
                } else if flexbox {
                    el.style().align_items = Some(align);
                }
            }
            Prop::ALIGN_CONTENT | Prop::JUSTIFY_CONTENT if flexbox => {
                let inline = prop == Prop::JUSTIFY_CONTENT
                    && !style
                        .get(Prop::FLEX_DIRECTION)
                        .is_some_and(|v| v.starts_with("column"));
                let align = match value {
                    "center" => renderer::AlignContent::Center,
                    "flex-end" => renderer::AlignContent::FlexEnd,
                    // WARNING: these ignore flex-direction reversal. `Start` is the left or top
                    // edge (rtl rows are `RowReverse`), and `left`/`right` act as `start` on a column.
                    "start" | "end" | "left" | "right" => {
                        if match value {
                            "left" => false,
                            "right" => inline,
                            _ => (value == "end") != (inline && rtl),
                        } {
                            renderer::AlignContent::End
                        } else {
                            renderer::AlignContent::Start
                        }
                    }
                    "space-between" => renderer::AlignContent::SpaceBetween,
                    "space-around" => renderer::AlignContent::SpaceAround,
                    "space-evenly" => renderer::AlignContent::SpaceEvenly,
                    "stretch" => renderer::AlignContent::Stretch,
                    _ => renderer::AlignContent::FlexStart,
                };
                if prop == Prop::JUSTIFY_CONTENT {
                    el.style().justify_content = Some(align);
                } else {
                    el.style().align_content = Some(align);
                }
            }
            Prop::TEXT_ALIGN => {
                el = match (value, rtl) {
                    ("center" | "-webkit-center", _) => el.text_center(),
                    ("right" | "-webkit-right", _) | ("end", false) | ("start", true) => {
                        el.text_right()
                    }
                    _ => el.text_left(),
                }
            }
            Prop::POSITION if matches!(value, "absolute" | "fixed") => el = el.absolute(),
            Prop::LEFT | Prop::RIGHT | Prop::TOP | Prop::BOTTOM
                if matches!(position, "relative" | "absolute") =>
            {
                let length: renderer::Length = if let Some(percent) = value
                    .strip_suffix('%')
                    .and_then(|v| v.parse::<f32>().ok())
                    .filter(|v| v.is_finite())
                {
                    match block_size {
                        Some(_) => px(percent / 100. * base).into(),
                        None => relative(percent / 100.),
                    }
                } else if let Some((fraction, offset)) =
                    percent_calc(&units, value).filter(|_| block_size.is_none())
                {
                    renderer::Length::Calc(fraction, px(offset))
                } else if let Some(length) = length() {
                    px(length).into()
                } else {
                    continue;
                };
                el = match prop {
                    Prop::LEFT => el.left(length),
                    Prop::RIGHT => el.right(length),
                    Prop::TOP => el.top(length),
                    _ => el.bottom(length),
                };
            }
            Prop::VISIBILITY => {
                el.style().visibility = if value == "hidden" || value == "collapse" {
                    renderer::Visibility::Hidden
                } else {
                    renderer::Visibility::Visible
                }
            }
            Prop::OPACITY if effect_layer.is_none() => el = el.opacity(opacity),
            Prop::WHITE_SPACE if matches!(value, "nowrap" | "pre") => {
                el = el.whitespace_nowrap().flex_nowrap()
            }
            Prop::TEXT_WRAP if value.contains("nowrap") => {
                el = el.whitespace_nowrap().flex_nowrap()
            }
            Prop::WORD_BREAK | Prop::OVERFLOW_WRAP => {
                el.text_style().word_break = Some(style::word_break(style));
            }
            Prop::TEXT_WRAP => {
                el.text_style().text_wrap = Some(if value.contains("balance") {
                    renderer::TextWrap::Balance
                } else if value.contains("pretty") {
                    renderer::TextWrap::Pretty
                } else {
                    renderer::TextWrap::Wrap
                })
            }
            Prop::CURSOR => {
                if let Some(cursor) = style::cursor(value) {
                    el = el.cursor(cursor);
                }
            }
            _ => {}
        }
    }
    if block_size.is_some() {
        for (axis, size, sides) in [(0, Prop::WIDTH, [3, 1]), (1, Prop::HEIGHT, [0, 2])] {
            let [start, end] = sides.map(|side| {
                style
                    .get([Prop::TOP, Prop::RIGHT, Prop::BOTTOM, Prop::LEFT][side])
                    .and_then(|value| units.resolve(value, containing[axis]))
            });
            if style.get(size).is_none_or(|value| value == "auto")
                && let (Some(start), Some(end)) = (start, end)
            {
                let length =
                    px(
                        (containing[axis] - start - end - margin[sides[0]] - margin[sides[1]])
                            .max(0.),
                    );
                el = if axis == 0 {
                    el.w(length)
                } else {
                    el.h(length)
                };
            }
        }
    }
    let auto = [Prop::MARGIN_LEFT, Prop::MARGIN_RIGHT]
        .map(|prop| style.get(prop).is_some_and(|value| value == "auto"));
    if auto.contains(&true)
        && !flex_parent
        && slot.is_none()
        && matches!(
            display,
            "block" | "flex" | "grid" | "list-item" | "flow-root"
        )
        && style.get(Prop::WIDTH).is_none_or(|value| value == "auto")
        && !matches!(position, "absolute" | "fixed")
        && style.get(Prop::FLOAT).is_none_or(|value| value == "none")
        && !matches!(
            node.tag.as_str(),
            "img"
                | "svg"
                | "canvas"
                | "video"
                | "iframe"
                | "input"
                | "button"
                | "select"
                | "textarea"
        )
    {
        if auto == [true, true] || margin[1] + margin[3] == 0. {
            el = el.w(relative(1.));
        } else {
            let style = el.style();
            (style.margin.left, style.margin.right) = (px(margin[3]).into(), px(margin[1]).into());
        }
    }
    if flex_parent
        && matches!(
            node.tag.as_str(),
            "img" | "video" | "canvas" | "iframe" | "svg" | "embed" | "object"
        )
        && [Prop::WIDTH, Prop::MAX_WIDTH]
            .into_iter()
            .any(|prop| style.get(prop).is_some_and(|value| value.contains('%')))
        && style
            .get(Prop::FLEX_BASIS)
            .is_none_or(|value| value == "auto")
        && node.parent.is_some_and(|parent| {
            styles.values[parent]
                .get(Prop::FLEX_DIRECTION)
                .is_none_or(|value| value.starts_with("row"))
        })
        && let width @ (renderer::Length::Px(_) | renderer::Length::Calc(..)) =
            el.style().size.width
    {
        el.style().flex_basis = width;
        el.style().size.width = renderer::Length::Auto;
    }
    if !flex_parent
        && slot.is_none()
        && !matches!(position, "absolute" | "fixed")
        && display.starts_with("inline")
        && let Some(parent) = node.parent
    {
        let align = style.get(Prop::VERTICAL_ALIGN).unwrap_or("baseline");
        let middle = match el.style().size.height {
            _ if align != "middle" => None,
            _ if display == "inline" => Some(units.font.lh),
            renderer::Length::Px(height) => Some(f32::from(height) + margin[0] - margin[2]),
            _ => None,
        };
        let sized = matches!(el.style().size.height, renderer::Length::Px(_));
        el.style().align_self = match align {
            "middle" if middle.is_none() => Some(renderer::AlignItems::Center),
            "top" => Some(renderer::AlignItems::FlexStart),
            "text-top" if !sized => Some(renderer::AlignItems::FlexStart),
            "bottom" => Some(renderer::AlignItems::FlexEnd),
            _ => None,
        };
        let below = |style: &style::Style, font: style::FontUnits| {
            let face = text.resolve_font(&style::font(style));
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
                && node.children.iter().all(|&child| {
                    doc.nodes[child].tag == "#text" && doc.nodes[child].text.trim_ascii().is_empty()
                });
        let field = editable && node.tag == "input";
        let swatch = native
            .as_ref()
            .is_some_and(|look| look.control == super::control::Control::Color);
        let parent_style = &styles.values[parent];
        let parent = below(parent_style, line);
        let edge = el.style().align_self.is_none()
            && (matches!(align, "text-top" | "text-bottom")
                || middle.is_some()
                || swatch
                || replaced && !field
                || display == "inline-grid"
                    && !node.children.iter().any(|&child| in_flow(doc, info, child)));
        if edge {
            el.style().baseline = renderer::Baseline::None;
        }
        let lift = if !edge {
            0.
        } else if matches!(align, "text-top" | "text-bottom") {
            let face = text.resolve_font(&style::font(parent_style));
            let [ascent, descent, _] = text
                .line_metrics(face, px(line.em * pixel_ratio))
                .map(|value| f32::from(value) / pixel_ratio);
            match el.style().size.height {
                renderer::Length::Px(height) if align == "text-top" => {
                    ascent - margin[0] - f32::from(height)
                }
                _ => margin[2] - descent,
            }
        } else if let Some(height) = middle {
            line.ex / 2. - height / 2.
        } else if swatch {
            -padding[2] - borders[2] - 4.
        } else if native.is_some() {
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
        let reserve = match (el.style().size.height, el.style().align_self) {
            (renderer::Length::Px(height), None) if display != "inline" => {
                line.lh - parent - f32::from(height)
            }
            (renderer::Length::Auto, None) if field => {
                line.lh - parent - units.font.lh - padding[0] - borders[0]
                    + below(style, units.font)
            }
            _ => f32::NEG_INFINITY,
        };
        let floor = margin[2].min(0.) + parent
            - match () {
                _ if matches!(display, "inline-block" | "inline-grid") && !replaced => {
                    f32::INFINITY
                }
                _ if edge => 0.,
                _ if display == "inline" => below(style, units.font),
                _ => padding[2] + borders[2] + below(style, units.font),
            };
        // WARNING: the margin reserves the shift so the line box grows instead of overlapping its neighbors.
        if !style::makes_no_line(doc, styles, id)
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
    }
    if style
        .get(Prop::BACKGROUND_IMAGE)
        .is_some_and(|value| value != "none")
    {
        let base = view
            .host
            .borrow()
            .dom
            .get(&view.id)
            .and_then(|state| state.base.clone());
        el = el.children(css_background(
            style,
            units,
            id,
            0,
            radius,
            view.clone(),
            base.as_ref(),
            &view.host.borrow().assets,
        ));
    }
    if !inner_shadows.is_empty() {
        el = el.child(inset_shadows(inner_shadows, radius, borders));
    }
    if cfg!(target_os = "macos")
        && node.tag == "span"
        && node.children.iter().all(|&child| {
            let child = &doc.nodes[child];
            child.tag == "#comment" || child.tag == "#text" && child.text.trim().is_empty()
        })
        && let Some(symbol) = style
            .get(Prop::APP_SF_SYMBOL)
            .filter(|name| *name != "none")
    {
        let symbol = symbol.trim_matches(['\'', '"']).to_owned();
        el = el.child(
            renderer::canvas(
                |_, _| (),
                move |bounds, _, window| window.paint_symbol(&symbol, bounds, ink),
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        );
    }
    if image_child.is_some() || cropped.is_some() {
        let inset = |side: usize| px(borders[side] + padding[side]);
        let corners = radius
            .to_pixels(size(px(basis()[0]), px(basis()[1])))
            .inset(inset(0), inset(1), inset(2), inset(3));
        if let Some(mut image) = image_child {
            image.style().corner_radii = renderer::Radii {
                x: corners.x.map(renderer::Length::Px),
                y: corners.y.map(renderer::Length::Px),
            };
            el = el.child(image);
        } else if let Some((image, fit)) = cropped {
            el = el.child(
                renderer::canvas(
                    |_, _| (),
                    move |bounds, _, window| {
                        let natural = image.size();
                        if u32::from(natural.width) > 0 && u32::from(natural.height) > 0 {
                            let fitted = fit.get_bounds(bounds, natural);
                            window.paint_image(bounds, fitted, corners, image.clone(), false);
                        }
                    },
                )
                .size_full(),
            );
        }
    }
    if let Some(lines) = style
        .get(Prop::WEBKIT_LINE_CLAMP)
        .or_else(|| style.get(Prop::LINE_CLAMP))
        .and_then(|value| value.parse::<usize>().ok())
    {
        let text = el.text_style();
        text.white_space = Some(renderer::WhiteSpace::Normal);
        text.line_clamp = Some(lines.max(1));
        text.text_overflow = Some("…".into());
    } else if style.get(Prop::TEXT_OVERFLOW) == Some("ellipsis")
        && matches!(style.get(Prop::WHITE_SPACE), Some("nowrap" | "pre"))
        && el.style().overflow.x != renderer::Overflow::Visible
    {
        el.text_style().text_overflow = Some("…".into());
    }
    if position == "fixed" {
        let viewport = units.viewport;
        let inset = [Prop::LEFT, Prop::RIGHT, Prop::TOP, Prop::BOTTOM].map(|prop| {
            style.get(prop).and_then(|v| {
                units.resolve(
                    v,
                    viewport[usize::from(matches!(prop, Prop::TOP | Prop::BOTTOM))],
                )
            })
        });
        if style.get(Prop::WIDTH).is_none_or(|v| v == "auto")
            && let [Some(left), Some(right), ..] = inset
        {
            el = el.w(px((viewport[0] - left - right).max(0.)));
        }
        if style.get(Prop::HEIGHT).is_none_or(|v| v == "auto")
            && let [_, _, Some(top), Some(bottom)] = inset
        {
            el = el.h(px((viewport[1] - top - bottom).max(0.)));
        }
    }
    if let Some(width) = units
        .resolve(
            style.get(Prop::OUTLINE_WIDTH).unwrap_or("3px"),
            containing[0],
        )
        .filter(|width| {
            *width > 0.
                && style
                    .get(Prop::OUTLINE_STYLE)
                    .is_some_and(|kind| !matches!(kind, "none" | "hidden"))
        })
    {
        let grow = width
            + style
                .get(Prop::OUTLINE_OFFSET)
                .and_then(|v| units.resolve(v, containing[0]))
                .unwrap_or(0.);
        let color = style
            .get(Prop::OUTLINE_COLOR)
            .and_then(style::color)
            .map_or(ink, |(ink, alpha)| rgba(ink << 8 | alpha as u32));
        let mut outline = renderer::node()
            .absolute()
            .top(px(-grow - borders[0]))
            .right(px(-grow - borders[1]))
            .bottom(px(-grow - borders[2]))
            .left(px(-grow - borders[3]))
            .border(px(width))
            .border_color(color);
        let corners = radius.to_pixels(size(px(basis()[0]), px(basis()[1])));
        let grown = |radius: renderer::Pixels| renderer::Length::Px(radius + px(grow));
        outline.style().corner_radii = renderer::Radii {
            x: corners.x.map(grown),
            y: corners.y.map(grown),
        };
        outline.style().border_style =
            border_style(style.get(Prop::OUTLINE_STYLE).unwrap_or("solid"));
        el = el.child(outline);
    }
    if shaded
        || visible
            .iter()
            .any(|&side| border_colors[side] != border_colors[visible[0]])
    {
        let mut bands = vec![([0.; 4], borders, [renderer::transparent_black(); 4])];
        let split = (0..4).any(|side| matches!(border_kinds[side], "groove" | "ridge"));
        if split {
            bands.push(([0.; 4], [0.; 4], [renderer::transparent_black(); 4]));
        }
        for side in 0..4 {
            let color = border_colors[side];
            let raised = side == 0 || side == 3;
            let (outer, inner) = match border_kinds[side] {
                "inset" => (shade_border(color, raised), None),
                "outset" => (shade_border(color, !raised), None),
                "groove" => (
                    shade_border(color, raised),
                    Some(shade_border(color, !raised)),
                ),
                "ridge" => (
                    shade_border(color, !raised),
                    Some(shade_border(color, raised)),
                ),
                _ => (color, None),
            };
            bands[0].2[side] = outer;
            if let Some(inner) = inner {
                let half = borders[side] / 2.;
                bands[0].1[side] = half;
                bands[1].0[side] = half;
                bands[1].1[side] = half;
                bands[1].2[side] = inner;
            }
        }
        if split {
            bands[1].0 = std::array::from_fn(|side| {
                if bands[1].1[side] > 0. {
                    bands[1].0[side]
                } else {
                    borders[side]
                }
            });
        }
        let kinds = border_kinds.map(border_style);
        el = el.child(
            renderer::canvas(
                |_, _| (),
                move |bounds, _, window| {
                    let outer = Bounds::new(
                        point(
                            bounds.left() - px(borders[3]),
                            bounds.top() - px(borders[0]),
                        ),
                        size(
                            bounds.size.width + px(borders[1] + borders[3]),
                            bounds.size.height + px(borders[0] + borders[2]),
                        ),
                    );
                    for (inset, widths, colors) in &bands {
                        let bounds = Bounds::from_corners(
                            outer.origin + point(px(inset[3]), px(inset[0])),
                            outer.bottom_right() - point(px(inset[1]), px(inset[2])),
                        );
                        let corners = radius
                            .resolve(outer.size)
                            .inset(px(inset[0]), px(inset[1]), px(inset[2]), px(inset[3]))
                            .clamp_radii_for_quad_size(bounds.size);
                        for side in 0..4 {
                            if widths[side] <= 0. {
                                continue;
                            }
                            window.paint_quad(
                                renderer::quad(
                                    bounds,
                                    corners,
                                    renderer::transparent_black(),
                                    renderer::Edges {
                                        top: px(widths[0]),
                                        right: px(widths[1]),
                                        bottom: px(widths[2]),
                                        left: px(widths[3]),
                                    },
                                    colors[side],
                                    kinds[side],
                                )
                                .sides(1 << side),
                            );
                        }
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        );
    }
    let resizer = (node.tag == "textarea"
        && style.get(Prop::RESIZE).is_none_or(|value| value != "none"))
    .then_some(super::control::Look {
        control: super::control::Control::Resizer,
        ..Default::default()
    });
    if let Some(look) = native
        .filter(|look| {
            !matches!(
                look.control,
                super::control::Control::Button | super::control::Control::Field
            )
        })
        .or(resizer)
    {
        el = el.child(super::control::layer(id, look, view.clone()));
    }
    let overscroll = overscroll(style);
    // WARNING: `overflow: hidden` boxes track a scroll handle only once a script scrolls them, or
    // when `overscroll-behavior` stops scrolling at them.
    let handle = {
        let mut host = view.host.borrow_mut();
        let scroll = &mut host.dom.entry(view.id).or_default().scroll;
        if scrolls.contains(&true)
            || !page
                && overflow.contains(&renderer::Overflow::Hidden)
                && overscroll != renderer::Point::default()
        {
            Some(scroll.entry(id).or_default().clone())
        } else if overflow.contains(&renderer::Overflow::Hidden) {
            scroll.get(&id).cloned()
        } else {
            None
        }
    };
    let mut bars = None;
    if let Some(handle) = handle {
        if !editable {
            el = el.track_scroll(&handle);
            el.style().restrict_scroll_to_axis = true;
            el.style().overscroll = overscroll;
        }
        bars = Some(scrollbar::Scrollbar {
            id,
            handle,
            look,
            axes: scrolls,
            reserved,
            borders,
            origin: [
                padding[3] - borders[1] - borders[3],
                padding[0] - borders[0] - borders[2],
            ],
        });
    }
    let marker_style = styles
        .text_pseudos
        .get(&id)
        .and_then(|pseudos| pseudos[0].as_ref());
    let content = marker_style
        .and_then(|marker| marker.get(Prop::CONTENT))
        .filter(|value| *value != "normal");
    if display == "list-item"
        && content.is_none_or(|value| value != "none")
        && let Some(kind) = style
            .get(Prop::LIST_STYLE_TYPE)
            .filter(|kind| *kind != "none" || content.is_some())
    {
        let list = node.parent.map(|parent| &doc.nodes[parent]);
        let items = node.parent.map(|parent| {
            siblings
                .borrow_mut()
                .entry((parent, true))
                .or_insert_with(|| {
                    let items: Vec<usize> = doc.nodes[parent]
                        .children
                        .iter()
                        .copied()
                        .filter(|&child| {
                            styles.values[child]
                                .get(Prop::DISPLAY)
                                .is_some_and(|value| value == "list-item")
                        })
                        .collect();
                    let index = items.iter().enumerate().map(|(index, &item)| (item, index));
                    let index = index.collect();
                    Rc::new((items, index))
                })
                .clone()
        });
        let position = items
            .as_ref()
            .and_then(|items| items.1.get(&id).copied())
            .unwrap_or(0) as i64;
        let count = items.as_ref().map_or(1, |items| items.0.len()) as i64;
        let start = list
            .and_then(|list| list.attrs.get("start"))
            .and_then(|value| value.parse::<i64>().ok());
        let number = node
            .attrs
            .get("value")
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(
                if list.is_some_and(|list| list.attrs.contains_key("reversed")) {
                    start.unwrap_or(count) - position
                } else {
                    start.unwrap_or(1) + position
                },
            );
        let number = styles.list_items.get(&id).copied().unwrap_or(number);
        let marker = match kind {
            "disc" | "circle" | "square" => style::counter_text(kind, number),
            "disclosure-closed" => "▸".to_owned(),
            "disclosure-open" => "▾".to_owned(),
            other if other.starts_with(['"', '\'']) => other.trim_matches(['"', '\'']).to_owned(),
            _ => format!("{}.", style::counter_text(kind, number)),
        };
        let marker = content.map_or(marker, |content| {
            let mut text = String::new();
            for part in style::components(content, ' ') {
                if part.starts_with("counter(") || part.starts_with("counters(") {
                    text.push_str(&number.to_string());
                    continue;
                }
                let mut input = cssparser::Parser::new(part);
                if let Ok(string) = input.expect_string() {
                    text.push_str(string);
                }
            }
            text
        });
        let inside = style
            .get(Prop::LIST_STYLE_POSITION)
            .is_some_and(|value| value == "inside");
        let face = text.resolve_font(&style::font(marker_style.unwrap_or(style)));
        let size = marker_style
            .and_then(|marker| marker.get(Prop::FONT_SIZE))
            .and_then(|value| units.resolve(value, units.font.em))
            .unwrap_or(units.font.em);
        let metrics = text
            .line_metrics(face, px(size * pixel_ratio))
            .map(f32::from);
        let line = match marker_style.map(style::line_height) {
            Some(style::LineHeight::Fixed(line)) => line,
            Some(style::LineHeight::Scale(scale)) => scale * size,
            Some(style::LineHeight::Normal) => metrics.iter().sum::<f32>() / pixel_ratio,
            None => units.font.lh,
        };
        let mut top = padding[0];
        if !inside && marker_style.is_some() {
            let own = text.resolve_font(&style::font(style));
            let rise = f32::from(text.baseline(face, px(size), px(line), pixel_ratio))
                - f32::from(text.baseline(own, px(units.font.em), px(units.font.lh), pixel_ratio));
            if rise > 0. {
                el = el.pt(px(padding[0] + rise));
            } else {
                top -= rise;
            }
        }
        let mut marker_box = if content.is_none() && kind.starts_with("disclosure-") {
            let [ascent, descent, _] = metrics;
            let side = (ascent * 2. / 3.).floor();
            let leading = ((line * pixel_ratio - ascent - descent) / 2.).floor();
            let space = text
                .advance(face, px(size), ' ')
                .map_or(size / 4., |space| f32::from(space.width));
            let look = super::control::Look {
                control: super::control::Control::Disclosure,
                checked: kind == "disclosure-open",
                ink: marker_style
                    .unwrap_or(style)
                    .get(Prop::COLOR)
                    .and_then(style::color)
                    .map_or(0, |(ink, _)| ink),
                ..Default::default()
            };
            let triangle = renderer::node()
                .absolute()
                .left_0()
                .top(px((leading + ascent - side) / pixel_ratio))
                .size(px(side / pixel_ratio))
                .child(super::control::layer(id, look, view.clone()));
            let above = (leading + ascent) / pixel_ratio;
            let marker = renderer::node()
                .flex_shrink_0()
                .w(px(side / pixel_ratio + space))
                .h(px(above))
                .mb(px(line - above))
                .baseline(renderer::Baseline::Bottom)
                .child(triangle);
            if inside {
                marker
            } else {
                marker.absolute().top(px(top)).right(relative(1.))
            }
        } else if inside {
            renderer::node()
                .flex_shrink_0()
                .mr(px(units.font.em * 0.3))
                .child(marker)
        } else if content.is_none() && matches!(kind, "disc" | "circle" | "square") {
            let color = marker_style
                .unwrap_or(style)
                .get(Prop::COLOR)
                .and_then(style::color)
                .map_or(rgba(0x000000ff), |(ink, alpha)| {
                    rgba(ink << 8 | alpha as u32)
                });
            let [ascent, descent, _] = metrics;
            let offset = (ascent * 2. / 3.).floor();
            let leading = ((line * pixel_ratio - ascent - descent) / 2.).floor();
            let dot = renderer::node()
                .absolute()
                .left(px(padding[3] - (offset + 7.) / pixel_ratio))
                .top(px(top
                    + (leading + (3. * (ascent - offset) / 2.).floor())
                        / pixel_ratio))
                .size(px(((offset + 1.) / 2.).floor() / pixel_ratio));
            match kind {
                "disc" => dot.rounded_full().bg(color),
                "circle" => dot
                    .rounded_full()
                    .border(px(1. / pixel_ratio))
                    .border_color(color),
                _ => dot.bg(color),
            }
        } else {
            renderer::node()
                .absolute()
                .top(px(top))
                .right(relative(1.))
                .pr(px(units.font.em * 0.5))
                .whitespace_nowrap()
                .child(marker)
        };
        if let Some(marker) = marker_style {
            let face = style::font(marker);
            marker_box = marker_box
                .font_family(face.family)
                .font_weight(face.weight)
                .text_size(px(size))
                .line_height(px(line));
            marker_box.text_style().font_style = Some(face.style);
            if let Some((ink, alpha)) = marker.get(Prop::COLOR).and_then(style::color) {
                marker_box = marker_box.text_color(rgba(ink << 8 | alpha as u32));
            }
        }
        el = el.child(marker_box);
    }
    let mut after = None;
    if let Some(pseudos) = styles.pseudos.get(&id) {
        for (index, pseudo) in pseudos.iter().enumerate() {
            let Some(content) = pseudo
                .get(Prop::CONTENT)
                .filter(|v| !matches!(*v, "none" | "normal"))
            else {
                continue;
            };
            if pseudo
                .get(Prop::DISPLAY)
                .is_some_and(|value| value == "none")
            {
                continue;
            }
            let text = pseudo_content(doc, id, pseudo, content);
            let mut pseudo_units = units;
            if let Some(size) = pseudo
                .get(Prop::FONT_SIZE)
                .and_then(|v| units.resolve(v, units.font.em))
            {
                pseudo_units.font = style::FontUnits::new(size);
            }
            let face = style::font(pseudo);
            let mut decoration = renderer::node()
                .flex()
                .flex_row()
                .flex_shrink_0()
                .text_size(px(pseudo_units.font.em))
                .font_family(face.family)
                .font_weight(face.weight);
            match style::line_height(pseudo) {
                style::LineHeight::Fixed(height) => decoration = decoration.line_height(px(height)),
                style::LineHeight::Scale(scale) => {
                    decoration = decoration.line_height(px(scale * pseudo_units.font.em))
                }
                style::LineHeight::Normal => {}
            }
            if let Some((ink, alpha)) = pseudo.get(Prop::COLOR).and_then(style::color) {
                decoration = decoration.text_color(rgba(ink << 8 | alpha as u32));
            }
            let absolute = pseudo
                .get(Prop::POSITION)
                .is_some_and(|v| matches!(v, "absolute" | "fixed"));
            if absolute {
                decoration = decoration.absolute();
            } else if pseudo
                .get(Prop::DISPLAY)
                .is_some_and(|value| !value.starts_with("inline"))
                && (all_inline || display == "inline")
            {
                decoration = decoration.w_full();
            }
            let pseudo_padding = pseudo_units
                .edges(pseudo, style::PADDING, containing[0])
                .map(|v| v.max(0.));
            let pseudo_margin = pseudo_units.edges(pseudo, style::MARGIN, containing[0]);
            let pseudo_borders = sides.map(|(width, kind, _)| {
                if pseudo
                    .get(kind)
                    .is_none_or(|kind| matches!(kind, "none" | "hidden"))
                {
                    return 0.;
                }
                pseudo
                    .get(width)
                    .map_or(Some(3.), |width| pseudo_units.resolve(width, containing[0]))
                    .unwrap_or(0.)
                    .max(0.)
            });
            decoration = decoration
                .pt(px(pseudo_padding[0]))
                .pr(px(pseudo_padding[1]))
                .pb(px(pseudo_padding[2]))
                .pl(px(pseudo_padding[3]))
                .mt(px(pseudo_margin[0]))
                .mr(px(pseudo_margin[1]))
                .mb(px(pseudo_margin[2]))
                .ml(px(pseudo_margin[3]))
                .border_t(px(pseudo_borders[0]))
                .border_r(px(pseudo_borders[1]))
                .border_b(px(pseudo_borders[2]))
                .border_l(px(pseudo_borders[3]));
            let pseudo_radius = super::effects::radii(pseudo, &pseudo_units, || containing);
            decoration.style().corner_radii = pseudo_radius;
            let colors = sides.map(|(_, _, color)| {
                pseudo
                    .get(color)
                    .and_then(style::color)
                    .map_or(ink, |(ink, alpha)| rgba(ink << 8 | alpha as u32))
            });
            let shown: Vec<usize> = (0..4).filter(|&side| pseudo_borders[side] > 0.).collect();
            if let Some(&first) = shown.first() {
                if shown.iter().all(|&side| colors[side] == colors[first]) {
                    decoration = decoration.border_color(colors[first]);
                } else {
                    decoration = decoration.border_color(renderer::transparent_black());
                    let kinds =
                        sides.map(|(_, kind, _)| border_style(pseudo.get(kind).unwrap_or("solid")));
                    let (borders, radius) = (pseudo_borders, pseudo_radius);
                    decoration = decoration.child(
                        renderer::canvas(
                            |_, _| (),
                            move |bounds, _, window| {
                                let outer = Bounds::new(
                                    point(
                                        bounds.left() - px(borders[3]),
                                        bounds.top() - px(borders[0]),
                                    ),
                                    size(
                                        bounds.size.width + px(borders[1] + borders[3]),
                                        bounds.size.height + px(borders[0] + borders[2]),
                                    ),
                                );
                                let corners = radius
                                    .resolve(outer.size)
                                    .clamp_radii_for_quad_size(outer.size);
                                for side in 0..4 {
                                    if borders[side] <= 0. {
                                        continue;
                                    }
                                    window.paint_quad(
                                        renderer::quad(
                                            outer,
                                            corners,
                                            renderer::transparent_black(),
                                            renderer::Edges {
                                                top: px(borders[0]),
                                                right: px(borders[1]),
                                                bottom: px(borders[2]),
                                                left: px(borders[3]),
                                            },
                                            colors[side],
                                            kinds[side],
                                        )
                                        .sides(1 << side),
                                    );
                                }
                            },
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    );
                }
            }
            let mut pseudo_shadows = Vec::new();
            for (prop, value) in pseudo.iter() {
                let value: &str = value;
                let vertical = matches!(
                    prop,
                    Prop::HEIGHT | Prop::MIN_HEIGHT | Prop::MAX_HEIGHT | Prop::TOP | Prop::BOTTOM
                );
                let base = containing[usize::from(vertical)];
                let length = || -> Option<renderer::Length> {
                    match value
                        .strip_suffix('%')
                        .and_then(|v| v.parse::<f32>().ok())
                        .filter(|v| v.is_finite())
                    {
                        Some(percent) => Some(relative(percent / 100.)),
                        None => match percent_calc(&pseudo_units, value) {
                            Some((fraction, offset)) => {
                                Some(renderer::Length::Calc(fraction, px(offset)))
                            }
                            None => pseudo_units
                                .resolve(value, base)
                                .map(|value| px(value).into()),
                        },
                    }
                };
                match prop {
                    Prop::WIDTH
                    | Prop::HEIGHT
                    | Prop::MIN_WIDTH
                    | Prop::MAX_WIDTH
                    | Prop::MIN_HEIGHT
                    | Prop::MAX_HEIGHT
                    | Prop::LEFT
                    | Prop::TOP
                    | Prop::RIGHT
                    | Prop::BOTTOM => {
                        let Some(length) = length() else { continue };
                        decoration = match prop {
                            Prop::WIDTH => decoration.w(length),
                            Prop::HEIGHT => decoration.h(length),
                            Prop::MIN_WIDTH => decoration.min_w(length),
                            Prop::MAX_WIDTH => decoration.max_w(length),
                            Prop::MIN_HEIGHT => decoration.min_h(length),
                            Prop::MAX_HEIGHT => decoration.max_h(length),
                            Prop::LEFT if absolute => decoration.left(length),
                            Prop::TOP if absolute => decoration.top(length),
                            Prop::RIGHT if absolute => decoration.right(length),
                            Prop::BOTTOM if absolute => decoration.bottom(length),
                            _ => decoration,
                        };
                    }
                    Prop::BACKGROUND_COLOR => {
                        if let Some((color, alpha)) = style::color(value) {
                            decoration = decoration.bg(rgba(color << 8 | alpha as u32));
                        }
                    }
                    Prop::OPACITY => {
                        if let Ok(opacity) = value.parse::<f32>() {
                            decoration = decoration.opacity(opacity.clamp(0., 1.));
                        }
                    }
                    Prop::BOX_SHADOW => {
                        decoration.style().box_shadow = pseudo_units.shadows(value, false);
                        pseudo_shadows = pseudo_units.shadows(value, true);
                    }
                    _ => {}
                }
            }
            if pseudo
                .get(Prop::BACKGROUND_IMAGE)
                .is_some_and(|value| value != "none")
            {
                let base = view
                    .host
                    .borrow()
                    .dom
                    .get(&view.id)
                    .and_then(|state| state.base.clone());
                decoration = decoration.children(css_background(
                    pseudo,
                    units,
                    id,
                    index + 1,
                    pseudo_radius,
                    view.clone(),
                    base.as_ref(),
                    &view.host.borrow().assets,
                ));
            }
            if !pseudo_shadows.is_empty() {
                decoration =
                    decoration.child(inset_shadows(pseudo_shadows, pseudo_radius, pseudo_borders));
            }
            let collapsed = text.trim_ascii().is_empty()
                && pseudo
                    .get(Prop::DISPLAY)
                    .is_some_and(|value| !value.starts_with("inline"))
                && pseudo.get(Prop::WHITE_SPACE).is_none_or(|value| {
                    !matches!(value, "pre" | "pre-wrap" | "pre-line" | "break-spaces")
                });
            if !text.is_empty() && !collapsed {
                decoration = decoration.child(SharedString::from(text));
            }
            let decoration = match [Prop::BACKDROP_FILTER, Prop::WEBKIT_BACKDROP_FILTER]
                .into_iter()
                .find_map(|prop| pseudo.get(prop))
                .filter(|_| {
                    pseudo
                        .get(Prop::VISIBILITY)
                        .is_none_or(|value| value == "visible")
                })
                .and_then(|value| super::effects::backdrop(value, &pseudo_units))
            {
                Some((blur, color_matrix)) => decoration.hooks(super::paint::Effected {
                    effect: renderer::PaintEffect {
                        backdrop: Some(renderer::Backdrop {
                            bounds: Bounds::default(),
                            corner_radii: Default::default(),
                            blur,
                            color_matrix,
                        }),
                        ..Default::default()
                    },
                    backdrop: Some(pseudo_radius),
                }),
                None => decoration,
            };
            let decoration = if [Prop::TRANSFORM, Prop::TRANSLATE, Prop::ROTATE, Prop::SCALE]
                .iter()
                .any(|&prop| pseudo.get(prop).is_some_and(|value| value != "none"))
            {
                decoration.hooks(super::paint::Transformed {
                    style: Rc::new(pseudo.clone()),
                    units: pseudo_units,
                    matrix: Default::default(),
                })
            } else {
                decoration
            };
            let order = pseudo
                .get(Prop::Z_INDEX)
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(0);
            let decoration = if order != 0
                && pseudo
                    .get(Prop::POSITION)
                    .is_some_and(|value| value != "static")
            {
                decoration.z_index(order)
            } else {
                decoration
            };
            if index == 0 {
                el = el.child(decoration);
            } else {
                after = Some(decoration);
            }
        }
    }
    let preserve = style
        .get(Prop::WHITE_SPACE)
        .is_some_and(|v| matches!(v, "pre" | "pre-wrap" | "break-spaces"))
        || node.tag == "pre";
    if (node.tag == "#text" || display == "inline" && !flex_parent && slot.is_none())
        && style
            .get(Prop::WHITE_SPACE)
            .is_none_or(|value| !matches!(value, "pre" | "nowrap"))
    {
        el = el.min_w(px(0.)).max_w(relative(1.)).flex_shrink(1.);
    }
    if preserve && node.text == "\n" {
        el = el.min_h(px(units.font.lh));
    }
    let inline = if editable || matches!(node.tag.as_str(), "svg" | "canvas" | "iframe") {
        None
    } else {
        inline_text
            .borrow_mut()
            .entry(id)
            .or_insert_with(|| style::inline_text(doc, styles, id).map(Rc::new))
            .clone()
            .filter(|data| {
                !data.text.is_empty()
                    || std::iter::successors(Some(id), |&node| doc.nodes[node].parent)
                        .any(|node| doc.nodes[node].attrs.contains_key("contenteditable"))
            })
    };
    let hit_test = inline.is_some();
    let drag = style
        .get(Prop::APP_REGION)
        .is_some_and(|value| value == "drag");
    if (hit_test || node.tag == "#text")
        && !drag
        && styles.user_select[id].0 != style::UserSelect::None
        && style.get(Prop::CURSOR).is_none_or(|value| value == "auto")
    {
        el = el.cursor(renderer::CursorStyle::IBeam);
    }
    if let Some(data) = inline {
        if style
            .get(Prop::CONTAINER_TYPE)
            .is_some_and(|v| matches!(v, "inline-size" | "size"))
        {
            let size = [0, 1].map(|axis| {
                (style
                    .get([Prop::WIDTH, Prop::HEIGHT][axis])
                    .and_then(|v| units.resolve(v, containing[axis]))
                    .map_or(
                        containing[axis] - [margin[1] + margin[3], 0.][axis],
                        |value| value + extra[axis],
                    )
                    - padding[1 - axis]
                    - padding[3 - axis]
                    - borders[1 - axis]
                    - borders[3 - axis])
                    .max(0.)
            });
            let mut host = view.host.borrow_mut();
            let dom = host.dom.entry(view.id).or_default();
            let size = dom.cull.widths.get(&id).copied().unwrap_or(size);
            dom.containers.insert(id, size);
        }
        let atom_slots: Rc<[super::text::AtomSlot]> = data
            .atoms
            .iter()
            .map(|atom| {
                let last = view
                    .host
                    .borrow()
                    .dom
                    .get(&view.id)
                    .and_then(|dom| dom.atoms.get(&atom.node).copied());
                let size = last.unwrap_or_else(|| {
                    let [_, max, _, height] = super::grid::intrinsic(
                        doc,
                        atom.node,
                        styles,
                        Some(containing[0]),
                        None,
                        text,
                        &mut measured.borrow_mut(),
                    );
                    let line = styles.lengths[atom.node].font.lh;
                    [max, height.max(line), line * 0.8]
                });
                super::text::AtomSlot {
                    size,
                    at: std::cell::Cell::new(point(px(0.), px(0.))),
                }
            })
            .collect();
        // WARNING: `text-box` trims the block by moving its text with negative margins, so the glyphs stay where they would be.
        let text_box = style.get(Prop::TEXT_BOX).unwrap_or("");
        let trim = style.get(Prop::TEXT_BOX_TRIM).unwrap_or(text_box);
        let edge = style.get(Prop::TEXT_BOX_EDGE).unwrap_or(text_box);
        let (over, under) = (
            trim.contains("trim-both") || trim.contains("trim-start"),
            trim.contains("trim-both") || trim.contains("trim-end"),
        );
        let (mut trim_top, mut trim_bottom) = (0., 0.);
        if over || under {
            let system = build.text;
            let font = system.resolve_font(&style::font(style));
            let (size, height) = (px(units.font.em), px(units.font.lh));
            let baseline = f32::from(system.baseline(font, size, height, build.pixel_ratio));
            let [ascent, descent, _] = system.line_metrics(font, size).map(f32::from);
            if over {
                trim_top = baseline
                    - if edge.contains("cap") {
                        units.font.cap
                    } else if edge.contains("ex") {
                        units.font.ex
                    } else {
                        ascent
                    };
            }
            if under {
                trim_bottom = units.font.lh
                    - baseline
                    - if edge.contains("alphabetic") {
                        0.
                    } else {
                        descent
                    };
            }
        }
        el = el.child(
            renderer::node()
                .min_w(px(0.))
                .max_w(relative(1.))
                .flex_shrink(1.)
                .mt(px(-trim_top))
                .mb(px(-trim_bottom))
                .child(InlineText {
                    data: data.clone(),
                    atoms: atom_slots.clone(),
                    view: view.clone(),
                    shaped: Default::default(),
                    prepainted: Default::default(),
                    justify: style
                        .get(Prop::TEXT_ALIGN)
                        .is_some_and(|value| value == "justify")
                        .then(|| {
                            style
                                .get(Prop::TEXT_ALIGN_LAST)
                                .is_some_and(|value| value == "justify")
                        }),
                }),
        );
        let atom_containing = [
            (containing[0]
                - margin[1]
                - margin[3]
                - padding[1]
                - padding[3]
                - borders[1]
                - borders[3])
                .max(0.),
            containing[1],
        ];
        for (index, atom) in data.atoms.iter().enumerate() {
            let mut wrapper = renderer::node()
                .absolute()
                .left(px(padding[3]))
                .top(px(padding[0]))
                .flex()
                .flex_row()
                .max_w(relative(1.));
            wrapper.style().align_items = Some(renderer::AlignItems::Baseline);
            el = el.child(
                wrapper
                    .hooks(super::text::AtomBox {
                        slots: atom_slots.clone(),
                        index,
                        node: atom.node,
                        view: view.clone(),
                    })
                    .child(element(build, atom.node, atom_containing, container, None)),
            );
        }
    } else if !node.children.is_empty()
        && node.tag != "canvas"
        && node.tag != "iframe"
        && node.tag != "svg"
        && !editable
    {
        let mut content = containing;
        for (axis, (prop, min, max)) in [
            (Prop::WIDTH, Prop::MIN_WIDTH, Prop::MAX_WIDTH),
            (Prop::HEIGHT, Prop::MIN_HEIGHT, Prop::MAX_HEIGHT),
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(value) = style
                .get(prop)
                .and_then(|v| units.resolve(v, containing[axis]))
                .map(|value| value + extra[axis])
                .or(intrinsic_width.filter(|_| axis == 0))
            {
                content[axis] = value;
            } else if axis == 0 {
                content[axis] -= margin[1] + margin[3];
            }
            if let Some(value) = style
                .get(max)
                .and_then(|v| units.resolve(v, containing[axis]))
            {
                content[axis] = content[axis].min(value + extra[axis]);
            }
            if let Some(value) = style
                .get(min)
                .and_then(|v| units.resolve(v, containing[axis]))
            {
                content[axis] = content[axis].max(value + extra[axis]);
            }
            content[axis] = (content[axis]
                - padding[1 - axis]
                - padding[3 - axis]
                - borders[1 - axis]
                - borders[3 - axis])
                .max(0.);
        }
        if let Some((bounds, _)) = &slot {
            content[0] = (bounds[2]
                - padding[1]
                - padding[3]
                - margin[1]
                - margin[3]
                - borders[1]
                - borders[3])
                .max(0.);
            content[1] = (bounds[3]
                - padding[0]
                - padding[2]
                - margin[0]
                - margin[2]
                - borders[0]
                - borders[2])
                .max(0.);
        }
        let container = if style
            .get(Prop::CONTAINER_TYPE)
            .is_some_and(|v| matches!(v, "inline-size" | "size"))
        {
            // WARNING: `content` is an estimate, so the size from the last layout wins.
            let mut host = view.host.borrow_mut();
            let dom = host.dom.entry(view.id).or_default();
            let size = dom.cull.widths.get(&id).copied().unwrap_or(content);
            dom.containers.insert(id, size);
            size
        } else {
            container
        };
        let flexed = style
            .get(Prop::DISPLAY)
            .is_some_and(|v| v.ends_with("grid"))
            && slot.is_none()
            && style.get(Prop::HEIGHT).is_none_or(|v| v == "auto")
            && node.parent.is_some_and(|parent| {
                let parent = &styles.values[parent];
                parent
                    .get(Prop::DISPLAY)
                    .is_some_and(|v| v.ends_with("flex"))
                    && parent
                        .get(Prop::FLEX_DIRECTION)
                        .is_some_and(|v| v.starts_with("column"))
            })
            && (style
                .get(Prop::FLEX_GROW)
                .and_then(|v| v.parse::<f32>().ok())
                .is_some_and(|v| v > 0.)
                || style
                    .get(Prop::FLEX_BASIS)
                    .is_some_and(|v| v != "auto" && v != "content"));
        let shrunk = style.get(Prop::DISPLAY).is_some_and(|v| v == "grid")
            && slot.is_none()
            && style.get(Prop::WIDTH).is_none_or(|v| v == "auto")
            && {
                let mut child = id;
                loop {
                    let own = &styles.values[child];
                    let inset = |prop| own.get(prop).is_some_and(|v| v != "auto");
                    if float_side(own).is_some()
                        || own
                            .get(Prop::POSITION)
                            .is_some_and(|v| matches!(v, "absolute" | "fixed"))
                            && !(inset(Prop::LEFT) && inset(Prop::RIGHT))
                    {
                        break true;
                    }
                    let Some(parent) = doc.nodes[child].parent else {
                        break false;
                    };
                    let parent = &styles.values[parent];
                    let display = parent.get(Prop::DISPLAY).unwrap_or("block");
                    if display.ends_with("flex")
                        && parent
                            .get(Prop::FLEX_DIRECTION)
                            .is_none_or(|v| v.starts_with("row"))
                    {
                        break child == id
                            || own
                                .get(Prop::FLEX_GROW)
                                .is_none_or(|v| v.parse::<f32>().is_ok_and(|grow| grow == 0.));
                    }
                    // WARNING: a column stretches to its own width, which can come from its content, as in a popup.
                    if !matches!(display, "block" | "inline-block" | "flex" | "inline-flex")
                        || parent.get(Prop::WIDTH).is_some_and(|v| v != "auto")
                    {
                        break false;
                    }
                    if display == "inline-block" {
                        break true;
                    }
                    child = doc.nodes[child].parent.unwrap_or(child);
                }
            };
        let mut widest = None;
        let fills = style.get(Prop::DISPLAY).is_some_and(|v| v == "grid")
            && !shrunk
            && slot.is_none()
            && !flexed
            && style
                .get(Prop::WIDTH)
                .is_none_or(|v| matches!(v, "auto" | "100%"))
            && !matches!(position, "absolute" | "fixed")
            && node.parent.is_some_and(|parent| {
                let parent = &styles.values[parent];
                match parent.get(Prop::DISPLAY).unwrap_or("block") {
                    "block" | "flow-root" | "list-item" => true,
                    "flex" | "inline-flex" => {
                        parent
                            .get(Prop::FLEX_DIRECTION)
                            .is_some_and(|v| v.starts_with("column"))
                            && parent
                                .get(Prop::ALIGN_ITEMS)
                                .is_none_or(|v| matches!(v, "normal" | "stretch"))
                            && style
                                .get(Prop::ALIGN_SELF)
                                .is_none_or(|v| matches!(v, "auto" | "stretch"))
                    }
                    _ => false,
                }
            });
        let grid = if let Some(display) = style
            .get(Prop::DISPLAY)
            .filter(|v| matches!(*v, "grid" | "inline-grid"))
        {
            if display == "inline-grid"
                && slot.is_none()
                && style.get(Prop::WIDTH).is_none_or(|v| v == "auto")
            {
                let extent =
                    padding[1] + padding[3] + borders[1] + borders[3] + margin[1] + margin[3];
                let [min, max, ..] = super::grid::intrinsic(
                    doc,
                    id,
                    styles,
                    None,
                    None,
                    text,
                    &mut measured.borrow_mut(),
                );
                content[0] = (max.min(content[0] + extent).max(min) - extent).max(0.);
            }
            let horizontal = padding[1] + padding[3] + borders[1] + borders[3];
            let vertical = padding[0] + padding[2] + borders[0] + borders[2];
            let mut tracked = [None; 2];
            if shrunk {
                let [min, max, ..] = super::grid::intrinsic(
                    doc,
                    id,
                    styles,
                    None,
                    None,
                    text,
                    &mut measured.borrow_mut(),
                );
                let [min, max] =
                    [min, max].map(|value| (value - horizontal - margin[1] - margin[3]).max(0.));
                widest = Some([min, max]);
                // WARNING: tracks size the untransformed box, so they read its laid-out size, not its window bounds.
                // New content makes the last size meaningless. Most content fits, so tracks take its full width,
                // and prepaint draws again when the layout disagrees.
                let last = {
                    let mut host = view.host.borrow_mut();
                    let dom = host.dom.entry(view.id).or_default();
                    let same = dom.grid_widest.insert(id, max) == Some(max);
                    same.then(|| dom.cull.widths.get(&id).map(|size| size[0].max(0.)))
                        .flatten()
                };
                // WARNING: laid-out bounds are in layout units, so a fitting grid may look narrower.
                content[0] = last.map_or(content[0].min(max), |last| {
                    if last + 0.5 >= max {
                        last.max(max)
                    } else {
                        last
                    }
                });
                tracked[0] = Some((last.unwrap_or(content[0]), horizontal));
            }
            // WARNING: `containing` is an estimate that doesn't know flex siblings. A grid that fills its parent lets
            // layout size it, and its tracks read that width from the last layout. Prepaint draws again when it changes.
            if fills {
                if let Some(last) = view
                    .host
                    .borrow()
                    .dom
                    .get(&view.id)
                    .and_then(|dom| dom.cull.widths.get(&id))
                {
                    content[0] = last[0].max(0.);
                }
                tracked[0] = Some((content[0], horizontal));
            }
            // WARNING: a positioned box with both insets takes its height from its containing block, as with a set height.
            let inset = |prop| {
                style
                    .get(prop)
                    .and_then(|value| units.resolve(value, containing[1]))
            };
            let stretched = matches!(position, "absolute" | "fixed")
                .then(|| Some(inset(Prop::TOP)? + inset(Prop::BOTTOM)?))
                .flatten();
            let height =
                if slot.is_some() || style.get(Prop::HEIGHT).is_some_and(|v| v != "auto") {
                    Some(content[1])
                } else if let Some(insets) = stretched {
                    Some((containing[1] - insets - margin[0] - margin[2] - vertical).max(0.))
                } else if flexed {
                    let height =
                        view.host.borrow().dom.get(&view.id).and_then(|state| {
                            state.cull.widths.get(&id).map(|size| size[1].max(0.))
                        });
                    tracked[1] = Some((height.unwrap_or(f32::NAN), vertical));
                    height
                } else {
                    None
                };
            // WARNING: entries stay while the grid is kept, so its size is checked in each frame.
            let mut host = view.host.borrow_mut();
            let flexed = &mut host.dom.entry(view.id).or_default().flexed_grids;
            if tracked.iter().any(Option::is_some) {
                flexed.insert(id, tracked);
            } else {
                flexed.remove(&id);
            }
            drop(host);
            let cached = view
                .host
                .borrow()
                .dom
                .get(&view.id)
                .and_then(|state| state.grids.get(&id))
                .filter(|(width, cached, inherited, _)| {
                    *width == content[0]
                        && *cached == height
                        && inherited.as_ref() == slot.as_ref().map(|(_, grid)| grid)
                })
                .map(|(_, _, _, grid)| grid.clone());
            Some(cached.unwrap_or_else(|| {
                let grid = super::grid::layout(
                    doc,
                    id,
                    styles,
                    Some(content[0]),
                    height,
                    slot.as_ref().map(|(_, grid)| grid),
                    view.host
                        .borrow()
                        .dom
                        .get(&view.id)
                        .map(|dom| &dom.baselines),
                    text,
                    &mut measured.borrow_mut(),
                );
                view.host
                    .borrow_mut()
                    .dom
                    .entry(view.id)
                    .or_default()
                    .grids
                    .insert(
                        id,
                        (
                            content[0],
                            height,
                            slot.as_ref().map(|(_, grid)| grid.clone()),
                            grid.clone(),
                        ),
                    );
                grid
            }))
        } else {
            None
        };
        if let Some(grid) = &grid {
            el = el.flex().flex_col();
            el = match widest {
                Some(widest) => el.child(renderer::node().measure(
                    widest,
                    |&[min, max], _, known, available, _| {
                        let width = known.width.unwrap_or(match available.width {
                            renderer::AvailableSpace::MinContent => px(min),
                            renderer::AvailableSpace::MaxContent => px(max),
                            renderer::AvailableSpace::Definite(width) => {
                                width.min(px(max)).max(px(min))
                            }
                        });
                        (renderer::size(width, px(0.)), None)
                    },
                )),
                None if fills => el,
                None => el.w(px(content[0]
                    + padding[1]
                    + padding[3]
                    + borders[1]
                    + borders[3])),
            };
            let auto = style.get(Prop::HEIGHT).is_none_or(|v| v == "auto");
            let rows = grid.rows.iter().sum::<f32>()
                + grid.rows.len().saturating_sub(1) as f32 * grid.gap[1];
            if auto && !flexed {
                el = el.h(px(rows + padding[0] + padding[2] + borders[0] + borders[2]));
            }
            if !flexed && contains(style, false) {
                blocks.borrow_mut().insert(
                    id,
                    [
                        content[0] + padding[1] + padding[3],
                        if auto { rows } else { content[1] } + padding[0] + padding[2],
                    ],
                );
            }
        }
        let mut children = if grid.is_some() {
            node.children.clone()
        } else {
            flat.clone()
        };
        // WARNING: each level of boxes is one large native frame, so levels past the stack budget are not drawn.
        let marker = 0u8;
        if (&raw const marker as usize) < STACK_LIMIT.with(|limit| *limit) {
            children.clear();
        }
        if grid.is_some() {
            children.sort_by_key(|child| {
                styles.values[*child]
                    .get(Prop::Z_INDEX)
                    .and_then(|v| v.parse::<i32>().ok())
                    .unwrap_or(0)
            });
        } else if flexbox {
            children.sort_by_key(|child| {
                styles.values[*child]
                    .get(Prop::ORDER)
                    .and_then(|v| v.parse::<i32>().ok())
                    .unwrap_or(0)
            });
        }
        let inline_element = |child: Option<&usize>| {
            child.is_some_and(|&child| {
                let node = &doc.nodes[child];
                node.tag == "#text" && !node.text.trim_ascii().is_empty()
                    || !node.tag.starts_with('#') && node.tag != "br" && inline_level(child)
            })
        };
        let multicol = columns(style, &units, content[0]);
        let mut line: Option<renderer::Node> = None;
        let mut floating = Vec::new();
        let mut group: Option<(Vec<Child>, Vec<Child>, Vec<Child>)> = None;
        let columns = multicol.filter(|_| flow && grid.is_none() && !all_inline);
        let mut collected: Vec<(Option<usize>, Child)> = Vec::new();
        let shown = |index: usize| {
            let child = &doc.nodes[children[index]];
            preserve
                || !(child.tag == "#text" && child.text.trim_ascii().is_empty())
                || flow
                    && grid.is_none()
                    && inline_element(index.checked_sub(1).and_then(|index| children.get(index)))
                    && inline_element(children.get(index + 1))
        };
        // WARNING: wrapped lines are culled whole, and only when the container's height is its content's, so `align-content` has no space to share.
        let wrap = display == "flex"
            && style.get(Prop::FLEX_DIRECTION).is_none_or(|v| v == "row")
            && style.get(Prop::FLEX_WRAP).is_some_and(|v| v == "wrap")
            && style
                .get(Prop::ALIGN_CONTENT)
                .is_none_or(|v| matches!(v, "normal" | "stretch" | "flex-start" | "start"))
            && style
                .get(Prop::ALIGN_ITEMS)
                .is_none_or(|v| !v.contains("baseline"))
            && [Prop::HEIGHT, Prop::MIN_HEIGHT, Prop::MAX_HEIGHT]
                .into_iter()
                .all(|name| {
                    style
                        .get(name)
                        .is_none_or(|v| matches!(v, "auto" | "none" | "0" | "0px"))
                })
            && !flex_parent
            && slot.is_none()
            && info[id].flags & cull::CHILDREN_CHANGED == 0;
        let stack = build.zone.clone().filter(|_| {
            grid.is_none()
                && columns.is_none()
                && !all_inline
                && display != "inline"
                && if flexbox {
                    wrap || style
                        .get(Prop::FLEX_DIRECTION)
                        .is_some_and(|v| v == "column")
                        && style.get(Prop::FLEX_WRAP).is_none_or(|v| v == "nowrap")
                        && style.get(Prop::JUSTIFY_CONTENT).is_none_or(|v| {
                            matches!(v, "normal" | "flex-start" | "start" | "stretch")
                        })
                } else {
                    flow && !children.iter().any(|&child| floated(child))
                }
        });
        let mut plan = None;
        if let Some(zone) = stack {
            let host = view.host.borrow();
            if let Some(dom) = host.dom.get(&view.id)
                && let Some(&[left, ..]) = dom.bounds.get(&id)
                && let Some(&[width, _]) = dom.cull.widths.get(&id)
            {
                let gap = if flexbox {
                    style
                        .get(Prop::ROW_GAP)
                        .and_then(|v| units.resolve(v, containing[0]))
                        .unwrap_or(0.)
                        .max(0.)
                } else {
                    0.
                };
                let (slots, mut runs) = if wrap {
                    cull::lines(
                        &children,
                        |index| {
                            shown(index) && info[children[index]].flags & cull::OUT_OF_FLOW == 0
                        },
                        |child| {
                            let placed = *dom.cull.placed.get(child)?;
                            let bounds = dom.bounds.get(&child)?;
                            let [top, _, bottom, left] = placed.2;
                            let member = cull::member(
                                info[child],
                                placed,
                                Some(bounds),
                                &zone,
                                units.viewport[1],
                                false,
                            );
                            Some((
                                member.is_some(),
                                [
                                    bounds[0] - left,
                                    bounds[1] - top,
                                    bounds[0] - left + placed.1[0],
                                    bounds[1] + bounds[3] + bottom,
                                ],
                            ))
                        },
                        gap,
                    )
                } else {
                    cull::plan(
                        &children,
                        shown,
                        |child| {
                            cull::member(
                                info[child],
                                dom.cull.placed.get(child).copied().unwrap_or_default(),
                                dom.bounds.get(&child),
                                &zone,
                                units.viewport[1],
                                flexbox,
                            )
                        },
                        gap,
                    )
                };
                if wrap {
                    let mut watch = Vec::new();
                    for (index, &child) in children.iter().enumerate() {
                        match slots[index] {
                            0 => {
                                let placed =
                                    dom.cull.placed.get(child).copied().unwrap_or_default();
                                if placed.0 != info[child].revision {
                                    watch.push((child, placed.1[0]));
                                }
                            }
                            slot => {
                                if let Some(run) = runs[slot - 1]
                                    .as_mut()
                                    .filter(|run| run.members[0] == child)
                                {
                                    run.width = width;
                                    run.watch = std::mem::take(&mut watch);
                                }
                            }
                        }
                    }
                }
                // WARNING: the first and last box of a run collapse their margins with siblings outside it, which may have changed.
                if !flexbox {
                    let collapsed = |child: usize, side: usize| {
                        let mut units = styles.lengths[child];
                        units.container = Some(container);
                        margins(build, child, &units, content[0], false, false)[side]
                    };
                    for run in runs.iter_mut().flatten() {
                        run.margins = [
                            collapsed(run.members[0], 0),
                            collapsed(run.members[run.members.len() - 1], 2),
                        ];
                    }
                }
                plan = (!runs.is_empty()).then_some((slots, runs, [left, width], zone));
            }
        }
        let mut culled = Vec::new();
        if let (Some(grid), Some(zone)) = (&grid, &build.zone) {
            if let Some(dom) = view.host.borrow().dom.get(&view.id) {
                for &(child, _, [x, y, width, height], _) in &grid.cells {
                    let placed = dom.cull.placed.get(child).copied().unwrap_or_default();
                    let [top, right, bottom, left] = placed.2;
                    // WARNING: the cell sizes come before layout snaps boxes to pixels, so they compare with the unsnapped cell sizes of the last layout.
                    if placed.1 == [width, height]
                        && cull::member(
                            info[child],
                            placed,
                            dom.bounds.get(&child),
                            zone,
                            units.viewport[1],
                            false,
                        )
                        .is_some()
                    {
                        // WARNING: layout truncates each inset and margin to layout units, so the offsets from the anchor do too.
                        let snap =
                            |value: f32| renderer::to_layout_unit(value, pixel_ratio) / pixel_ratio;
                        culled.push((
                            child,
                            [
                                snap(x + padding[3]) + snap(left),
                                snap(y + padding[0]) + snap(top),
                                width - left - right,
                                height - top - bottom,
                            ],
                        ));
                    }
                }
            }
            if !culled.is_empty() {
                culls.set(culls.get() + 1);
                el = el.child(cull::anchor(culled.clone(), zone.clone(), view.clone()));
            }
            if let Some(baseline) = grid.first_baseline.filter(|_| {
                grid.first_item
                    .is_some_and(|item| culled.binary_search_by_key(&item, |&(id, _)| id).is_ok())
            }) {
                let baseline = baseline.max(0.);
                let mut line = renderer::node()
                    .w(px(0.))
                    .h(px(baseline))
                    .mb(px(-baseline))
                    .flex_shrink_0()
                    .baseline(renderer::Baseline::Bottom);
                line.style().baseline_source = true;
                el = el.child(line);
            }
        }
        macro_rules! place {
            ($element:expr, $child:expr) => {{
                let element = $element.into();
                match group.as_mut() {
                    Some((_, _, content)) => content.push(element),
                    None if columns.is_some() => collected.push(($child, element)),
                    None => el = el.child(element),
                }
            }};
        }
        macro_rules! close_group {
            () => {{
                if let Some(open) = line.take() {
                    place!(open, None);
                }
                if let Some((lefts, rights, content)) = group.take() {
                    let row = renderer::node()
                        .flex()
                        .flex_row()
                        .items_start()
                        .w_full()
                        .children(lefts)
                        .child(
                            renderer::node()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w(px(0.))
                                .children(content),
                        )
                        .children(rights.into_iter().rev());
                    if columns.is_some() {
                        collected.push((None, row.into()));
                    } else {
                        el = el.child(row);
                    }
                }
            }};
        }
        let line_break = || {
            let mut el = renderer::node().w_full();
            el.style().align_self = Some(renderer::AlignSelf::FlexStart);
            el
        };
        for (index, &child) in children.iter().enumerate() {
            if shown(index) {
                if let Some((slots, runs, then, zone)) = plan.as_mut()
                    && let Some(&slot) = slots.get(index).filter(|&&slot| slot > 0)
                {
                    if let Some(run) = runs[slot - 1].take() {
                        culls.set(culls.get() + 1);
                        if let Some(open) = line.take() {
                            place!(open, None);
                        }
                        place!(
                            cull::spacer(run, id, *then, zone.clone(), view.clone()),
                            None
                        );
                    }
                    continue;
                }
                if culled.binary_search_by_key(&child, |&(id, _)| id).is_ok()
                    || top_layer.contains(&child)
                {
                    continue;
                }
                let placement = grid.as_ref().and_then(|grid| {
                    let index = grid
                        .cells
                        .binary_search_by_key(&child, |(id, ..)| *id)
                        .ok()?;
                    let (item, [column, row, columns, rows], mut bounds, baseline) =
                        *grid.cells.get(index)?;
                    bounds[0] += padding[3];
                    bounds[1] += padding[0];
                    Some((
                        bounds,
                        super::grid::Grid {
                            columns: grid.columns.get(column..column + columns)?.to_vec(),
                            rows: grid.rows.get(row..row + rows)?.to_vec(),
                            gap: grid.gap,
                            baseline,
                            first_item: grid.first_item.filter(|&first| first == item),
                            ..Default::default()
                        },
                    ))
                });
                let cell = placement
                    .as_ref()
                    .map(|(bounds, grid)| (*bounds, grid.first_item.is_some()));
                let child_element = element(build, child, content, container, placement);
                let child_element: renderer::Child = match cell {
                    Some((bounds, first)) => {
                        let mut cell = renderer::node()
                            .absolute()
                            .left(px(bounds[0]))
                            .top(px(bounds[1]))
                            .w(px(bounds[2]))
                            .h(px(bounds[3]));
                        cell.style().baseline_source = first;
                        cell.child(child_element).into()
                    }
                    None => child_element,
                };
                let out_of_flow = styles.values[child]
                    .get(Prop::POSITION)
                    .is_some_and(|value| matches!(value, "absolute" | "fixed"));
                let child_style = &styles.values[child];
                if grid.is_none() && !out_of_flow {
                    let clear = child_style.get(Prop::CLEAR).unwrap_or("none");
                    if group.is_some() && clear != "none" {
                        close_group!();
                    }
                    if floated(child) {
                        if let Some(open) = line.take() {
                            place!(open, None);
                        }
                        let open = group.get_or_insert_with(Default::default);
                        let float = if child_style
                            .get(Prop::WIDTH)
                            .is_none_or(|value| value == "auto")
                        {
                            renderer::node()
                                .flex()
                                .flex_col()
                                .flex_shrink(1.)
                                .min_w(px(0.))
                                .child(child_element)
                                .into()
                        } else {
                            child_element
                        };
                        if float_side(child_style) == Some(true) {
                            open.1.push(float);
                        } else {
                            open.0.push(float);
                        }
                        continue;
                    }
                }
                if flow
                    && grid.is_none()
                    && child_style
                        .get(Prop::POSITION)
                        .is_some_and(|value| value == "absolute")
                    && child_style
                        .get(Prop::DISPLAY)
                        .is_none_or(|value| value != "none")
                    && [[Prop::LEFT, Prop::RIGHT], [Prop::TOP, Prop::BOTTOM]]
                        .iter()
                        .any(|axis| {
                            axis.iter().all(|prop| {
                                child_style.get(*prop).is_none_or(|value| value == "auto")
                            })
                        })
                {
                    // WARNING: the placeholder must come before the box, which reads its bounds in prepaint.
                    let block = child_style
                        .get(Prop::DISPLAY)
                        .is_some_and(|value| !value.starts_with("inline"));
                    let pending = flat
                        [..flat.iter().position(|&other| other == child).unwrap_or(0)]
                        .iter()
                        .rev()
                        .find(|&&other| in_flow(doc, info, other) && !floated(other))
                        .filter(|&&other| {
                            block && line.is_none() && !all_inline && self::block(build, other)
                        })
                        .map_or(0., |&other| {
                            let (_, [positive, negative], _) = adjoining(build, other, content[0]);
                            let used = margins(
                                build,
                                other,
                                &styles.lengths[other],
                                content[0],
                                false,
                                false,
                            )[2];
                            (positive + negative - used).max(0.)
                        });
                    let entity = view.clone();
                    let mut placeholder = renderer::canvas(
                        move |mut bounds, _| {
                            bounds.origin.y += px(pending);
                            let view = &*entity;
                            view.host
                                .borrow_mut()
                                .dom
                                .entry(view.id)
                                .or_default()
                                .anchors
                                .insert(child, bounds);
                        },
                        |_, _, _| {},
                    )
                    .w(px(0.))
                    .flex_shrink_0()
                    .h(px(
                        if block
                            && (line.is_some()
                                || all_inline
                                    && flat
                                        .iter()
                                        .any(|&other| other != child && in_flow(doc, info, other)))
                        {
                            units.font.lh
                        } else {
                            0.
                        },
                    ));
                    placeholder.style().align_self = Some(renderer::AlignSelf::FlexStart);
                    match line.take() {
                        Some(open) => line = Some(open.child(placeholder)),
                        None if all_inline || block => place!(placeholder, None),
                        None => {
                            let mut next = renderer::node().flex().flex_row().flex_wrap();
                            next.style().justify_content = line_justify;
                            line = Some(next.child(placeholder));
                        }
                    }
                }
                if flow
                    && !all_inline
                    && grid.is_none()
                    && line.is_none()
                    && !out_of_flow
                    && style::makes_no_line(doc, styles, child)
                {
                    place!(
                        renderer::node().flex().flex_row().child(child_element),
                        None
                    );
                } else if flow
                    && !all_inline
                    && grid.is_none()
                    && inline_level(child)
                    && !out_of_flow
                {
                    let mut next = line.take().unwrap_or_else(|| {
                        renderer::node()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .child(strut())
                    });
                    next.style().justify_content = line_justify;
                    next.style().align_items = line_align;
                    next = next.child(child_element);
                    if doc.nodes[child].tag == "br" {
                        place!(next, None);
                    } else {
                        line = Some(next);
                    }
                } else if out_of_flow {
                    floating.push(child_element);
                } else {
                    if let Some(line) = line.take() {
                        place!(line, None);
                    }
                    place!(child_element, Some(child));
                    if flow && all_inline && doc.nodes[child].tag == "br" {
                        place!(line_break(), None);
                    }
                }
            }
        }
        close_group!();
        if let Some((count, gap, rule)) = columns {
            let width = ((content[0] - gap * (count - 1) as f32) / count as f32).max(0.);
            let heights: Vec<f32> = collected
                .iter()
                .map(|(child, _)| {
                    child.map_or(units.font.lh, |child| {
                        super::grid::intrinsic(
                            doc,
                            child,
                            styles,
                            Some(width),
                            None,
                            text,
                            &mut measured.borrow_mut(),
                        )[3]
                    })
                })
                .collect();
            let target = heights.iter().sum::<f32>() / count as f32;
            let mut columns: Vec<Vec<Child>> = (0..count).map(|_| Vec::new()).collect();
            let (mut column, mut filled) = (0, 0.);
            for ((_, item), height) in collected.into_iter().zip(heights) {
                if filled > 0. && filled + height / 2. > target && column + 1 < count {
                    column += 1;
                    filled = 0.;
                }
                filled += height;
                columns[column].push(item);
            }
            let mut row = renderer::node()
                .flex()
                .flex_row()
                .items_start()
                .w_full()
                .gap_x(px(gap));
            for items in columns {
                row = row.child(
                    renderer::node()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w(px(0.))
                        .children(items),
                );
            }
            if let Some((rule_width, color, kind)) = rule {
                row = row.child(column_rules(count, gap, rule_width, color, kind));
            }
            el = el.child(row);
        }
        el = el.children(floating);
    }
    if let Some(after) = after {
        el = el.child(after);
    }
    if slot.is_some() && flow && matches!(node.tag.as_str(), "td" | "th") {
        let align = match style.get(Prop::VERTICAL_ALIGN) {
            Some("middle") => Some(renderer::AlignContent::Center),
            Some("bottom") => Some(renderer::AlignContent::FlexEnd),
            _ => None,
        };
        if all_inline {
            el.style().align_content = align;
        } else {
            el.style().justify_content = align;
        }
    }
    if let Some((bounds, grid)) = &slot {
        el.style().baseline_source = grid.first_item.is_some();
        el = el
            .absolute()
            .left(px(0.))
            .top(px(0.))
            .mt(px(margin[0]))
            .mr(px(margin[1]))
            .mb(px(margin[2]))
            .ml(px(margin[3]))
            .w(px(layout_ceil(
                (bounds[2] - margin[1] - margin[3]).max(0.),
                pixel_ratio,
            )))
            .h(px((bounds[3] - margin[0] - margin[2]).max(0.)));
    }
    // WARNING: Windows moves the window in the native hit test (`window_drag_area`), macOS and Linux on the first drag.
    if drag {
        el = el.window_drag_area().on_mouse_down(MouseButton::Left, {
            let view = view.clone();
            move |event: &renderer::MouseDownEvent, window, cx| {
                // WARNING: a drag box gets the presses of all its children, so the box under the pointer
                // decides: the nearest one that sets a region, as a `no-drag` button must get its click.
                let dragged = view.host.try_borrow().is_ok_and(|host| {
                    let viewport = *host.viewports.get(&view.id).unwrap_or(&[800., 600.]);
                    let (Some(page), Some(state)) =
                        (host.pages.get(&view.id), host.dom.get(&view.id))
                    else {
                        return true;
                    };
                    let Some(computed) = &state.computed else {
                        return true;
                    };
                    let at = [event.position.x, event.position.y].map(f32::from);
                    crate::dom::hit_test(&page.doc, state, computed, viewport, at, true, false)
                        .as_u64()
                        .is_none_or(|target| {
                            std::iter::successors(Some(target as usize), |&node| {
                                page.doc.nodes[node].parent
                            })
                            .find_map(|node| {
                                computed.values[node]
                                    .get(Prop::APP_REGION)
                                    .filter(|value| *value != "none")
                            }) == Some("drag")
                        })
                });
                if !dragged {
                    return;
                }
                cx.stop_propagation();
                if event.click_count != 2 {
                    view.moving.set(true);
                } else if cfg!(target_os = "macos") {
                    window.titlebar_double_click();
                } else {
                    window.zoom_window();
                }
            }
        });
    }
    let parent_display = parent
        .and_then(|parent| styles.values[parent].get(Prop::DISPLAY))
        .unwrap_or("block");
    let axes = [[Prop::LEFT, Prop::RIGHT], [Prop::TOP, Prop::BOTTOM]].map(|axis| {
        axis.iter()
            .all(|prop| style.get(*prop).is_none_or(|v| v == "auto"))
    });
    let mut anchor = None;
    if position == "absolute" && slot.is_none() && axes.contains(&true) {
        if matches!(parent_display, "grid" | "inline-grid") {
            if axes[0] {
                el = el.left(px(0.));
            }
            if axes[1] {
                el = el.top(px(0.));
            }
        } else if !matches!(parent_display, "flex" | "inline-flex") {
            let block = !display.starts_with("inline");
            if block
                && axes[0]
                && let Some(parent) = parent
            {
                el = el.left(px(styles.lengths[parent].edges(
                    &styles.values[parent],
                    style::PADDING,
                    containing[0],
                )[3]
                .max(0.)));
            }
            anchor = Some((axes, block, [margin[3], margin[0]]));
        }
    }
    if stacking {
        el = el.child(renderer::layers(false));
    }
    el = el.children(bars);
    let sticky = (style.get(Prop::POSITION).is_some_and(|v| v == "sticky"))
        .then(|| {
            let host = view.host.borrow();
            let insets = |parent: usize, padding: bool| {
                let (style, units) = (&styles.values[parent], &styles.lengths[parent]);
                let padding = if padding {
                    units.edges(style, style::PADDING, containing[0])
                } else {
                    [0.; 4]
                };
                std::array::from_fn::<f32, 4, _>(|side| {
                    let [width, kind, _] = style::BORDERS[side];
                    padding[side]
                        + if style
                            .get(kind)
                            .is_none_or(|kind| matches!(kind, "none" | "hidden"))
                        {
                            0.
                        } else {
                            style
                                .get(width)
                                .map_or(Some(3.), |v| units.resolve(v, containing[0]))
                                .unwrap_or(0.)
                                .max(0.)
                        }
                })
            };
            ancestors.iter().rev().copied().find_map(|parent| {
                let scroll = host.dom.get(&view.id)?.scroll.get(&parent)?.clone();
                let block = ancestors
                    .last()
                    .map(|&parent| (parent, insets(parent, true)));
                Some(super::paint::Sticky {
                    scroll,
                    border: insets(parent, false),
                    block,
                })
            })
        })
        .flatten();
    let vertical = style
        .get(Prop::WRITING_MODE)
        .filter(|mode| mode.starts_with("vertical") || mode.starts_with("sideways"))
        .filter(|_| id != 0 && !node.tag.starts_with('#') && slot.is_none());
    let el = match vertical {
        Some(mode) => {
            let [_, widest, ..] = super::grid::intrinsic(
                doc,
                id,
                styles,
                None,
                None,
                text,
                &mut measured.borrow_mut(),
            );
            let inline = style
                .get(Prop::HEIGHT)
                .and_then(|value| units.resolve(value, containing[1]))
                .map(|height| height + extra[1])
                .unwrap_or_else(|| widest.min(units.viewport[1]))
                .max(1.);
            let block = super::grid::intrinsic(
                doc,
                id,
                styles,
                Some(inline),
                None,
                text,
                &mut measured.borrow_mut(),
            )[3];
            renderer::node()
                .id(("vertical", id))
                .relative()
                .flex_shrink_0()
                .w(px(block))
                .h(px(inline))
                .child(Turned::node(
                    el.absolute().top_0().left_0().w(px(inline)).into(),
                    mode != "sideways-lr",
                    block,
                    inline,
                ))
        }
        None => el,
    };
    let element = el.hooks(DomBox {
        placed: Default::default(),
        anchor,
        block: contains(style, false).then_some(borders),
        container: positioner.filter(|&block| position == "fixed" || Some(block) != parent),
        sized: positioner
            .filter(|&block| block != 0 && Some(block) != parent)
            .map(|_| containing),
        style: style.clone(),
        effects: effect_layer,
        composited,
        transformed: info[id].flags & cull::TRANSFORMED != 0,
        perspective: parent_perspective.clone().zip(parent),
        units,
        revision: info[id].revision,
        sides: [
            padding[1] + padding[3] + borders[1] + borders[3],
            padding[0] + padding[2] + borders[0] + borders[2],
        ],
        margin,
        cell: slot.as_ref().map(|(bounds, _)| [bounds[2], bounds[3]]),
        grid: slot
            .as_ref()
            .filter(|(_, grid)| grid.baseline || grid.first_item.is_some())
            .and_then(|(_, grid)| Some((node.parent?, grid.baseline))),
        shared: (matches!(node.tag.as_str(), "td" | "th")
            && std::iter::successors(node.parent, |&parent| doc.nodes[parent].parent)
                .find(|&parent| doc.nodes[parent].tag == "table")
                .is_some_and(|table| {
                    styles.values[table].get(Prop::BORDER_COLLAPSE) == Some("collapse")
                }))
        .then(|| borders.map(|width| width / 2.)),
        boxless: node.tag == "option"
            && std::iter::successors(node.parent, |&parent| doc.nodes[parent].parent)
                .take(2)
                .find(|&parent| doc.nodes[parent].tag == "select")
                .is_some_and(|select| {
                    let attrs = &doc.nodes[select].attrs;
                    !attrs.contains_key("multiple")
                        && !attrs.get("size").is_some_and(|size| {
                            size.trim().parse::<u32>().is_ok_and(|size| size > 1)
                        })
                }),
        text: (display == "inline" && !node.tag.starts_with('#'))
            .then(|| {
                let mut texts = node.children.iter().copied().filter(|&child| {
                    doc.nodes[child].tag == "#text"
                        && !doc.nodes[child].text.trim_ascii().is_empty()
                });
                let first = texts.next()?;
                Some((
                    [first, texts.next_back().unwrap_or(first)],
                    [padding[0] + borders[0], padding[2] + borders[2]],
                ))
            })
            .flatten(),
        leading: (display == "inline" && !node.tag.starts_with('#')).then(|| {
            let face = text.resolve_font(&style::font(style));
            let [ascent, descent, _] = text
                .line_metrics(face, px(units.font.em * pixel_ratio))
                .map(f32::from);
            let line = units.font.lh * pixel_ratio;
            let above = ((line - ascent - descent) / 2.).floor();
            let edges = padding[0] + padding[2] + borders[0] + borders[2];
            [
                units.font.lh + edges,
                above / pixel_ratio,
                (line - above - ascent - descent) / pixel_ratio,
            ]
        }),
        parts: if node.tag == "table" {
            let mut rows: Vec<(usize, Vec<usize>)> = Vec::new();
            let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
            for &cell in &node.children {
                let Some(row) = doc.nodes[cell]
                    .parent
                    .filter(|&row| row != id && doc.nodes[row].tag == "tr")
                else {
                    continue;
                };
                match rows.last_mut() {
                    Some((last, cells)) if *last == row => cells.push(cell),
                    _ => {
                        rows.push((row, vec![cell]));
                        if let Some(group) = doc.nodes[row].parent.filter(|&group| group != id) {
                            match groups.last_mut() {
                                Some((last, members)) if *last == group => members.push(row),
                                _ => groups.push((group, vec![row])),
                            }
                        }
                    }
                }
            }
            rows.extend(groups);
            rows.into()
        } else {
            Rc::from([])
        },
        id,
        ids: ids.clone(),
        view: view.clone(),
        sticky,
        fixed: position == "fixed" && positioner.is_none(),
        inset: [Prop::TOP, Prop::RIGHT, Prop::BOTTOM, Prop::LEFT].map(|prop| {
            let axis = usize::from(matches!(prop, Prop::TOP | Prop::BOTTOM));
            style
                .get(prop)
                .and_then(|v| units.resolve(v, containing[axis]))
        }),
    });
    // WARNING: `z-index` does nothing in the top layer, whose boxes paint in their own order.
    if layered
        && let Some(order) = order.filter(|order| *order != 0)
        && !top_layer.contains(&id)
    {
        element.z_index(order).into()
    } else {
        element.into()
    }
}

pub(super) fn pseudo_content(
    doc: &Document,
    id: usize,
    pseudo: &style::Style,
    content: &str,
) -> String {
    let node = &doc.nodes[id];
    let mut text = String::new();
    for part in style::components(content, ' ') {
        if let Some(name) = part.strip_prefix("attr(").and_then(|v| v.strip_suffix(')')) {
            text.push_str(node.attrs.get(name.trim()).map_or("", String::as_str));
        } else if let Some(close) = match part {
            "open-quote" => Some(false),
            "close-quote" => Some(true),
            _ => None,
        } {
            let depth = std::iter::successors(node.parent, |&parent| doc.nodes[parent].parent)
                .filter(|&parent| doc.nodes[parent].tag == "q")
                .count();
            let quotes: Vec<String> = match pseudo.get(Prop::QUOTES) {
                None | Some("auto") => ["\u{201c}", "\u{201d}", "\u{2018}", "\u{2019}"]
                    .map(str::to_owned)
                    .to_vec(),
                Some(value) => style::components(value, ' ')
                    .into_iter()
                    .filter_map(|part| {
                        let mut input = cssparser::Parser::new(part);
                        input.expect_string().ok().map(|string| string.to_string())
                    })
                    .collect(),
            };
            let pair = (depth * 2).min(quotes.len().saturating_sub(2));
            text.push_str(
                quotes
                    .get(pair + usize::from(close))
                    .map_or("", String::as_str),
            );
        } else if let Some(string) = part
            .strip_prefix(['"', '\''])
            .and_then(|v| v.strip_suffix(['"', '\'']))
        {
            let mut chars = string.chars().peekable();
            while let Some(c) = chars.next() {
                if c != '\\' {
                    text.push(c);
                    continue;
                }
                let mut code = String::new();
                while code.len() < 6
                    && let Some(digit) = chars.next_if(char::is_ascii_hexdigit)
                {
                    code.push(digit);
                }
                if code.is_empty() {
                    text.extend(chars.next());
                    continue;
                }
                chars.next_if_eq(&' ');
                text.push(
                    u32::from_str_radix(&code, 16)
                        .ok()
                        .and_then(char::from_u32)
                        .unwrap_or('\u{fffd}'),
                );
            }
        }
    }
    text
}

/// The ratio of an `aspect-ratio` value, like `5/4`, `1.25` or `auto 5 / 4`.
/// WARNING: taffy passes min and max sizes through the aspect ratio even when the other side is set.
/// CSS does that only for an auto size, so `max-width: 100%` must not limit the height of a sized image.
/// A set limit replaces these in the style loop.
fn only_auto_ratio(mut el: renderer::Node, style: &style::Style) -> renderer::Node {
    let set = |prop| style.get(prop).is_some_and(|value| value != "auto");
    let limited = |prop| style.get(prop).is_some_and(|value| value != "none");
    if set(Prop::HEIGHT) {
        if !limited(Prop::MAX_HEIGHT) {
            el = el.max_h(px(1e7));
        }
        if !set(Prop::MIN_HEIGHT) {
            el = el.min_h(px(0.));
        }
    }
    if set(Prop::WIDTH) {
        if !limited(Prop::MAX_WIDTH) {
            el = el.max_w(px(1e7));
        }
        if !set(Prop::MIN_WIDTH) {
            el = el.min_w(px(0.));
        }
    }
    el
}

pub(crate) fn aspect_ratio(value: &str) -> Option<f32> {
    let value = value.replace('/', " / ");
    let parts = style::components(&value, ' ');
    let numbers: Vec<f32> = parts.iter().filter_map(|part| part.parse().ok()).collect();
    let ratio = match numbers.as_slice() {
        [a, b, ..] if parts.contains(&"/") => a / b,
        [a, ..] => *a,
        _ => return None,
    };
    (ratio.is_finite() && ratio > 0.).then_some(ratio)
}

pub(crate) fn dimension(value: &str, integer: bool) -> Option<f32> {
    let value = value.trim_start_matches(['\t', '\n', '\x0c', '\r', ' ']);
    let value = if integer {
        value.strip_prefix('+').unwrap_or(value)
    } else {
        value
    };
    let digits = |from: usize| {
        value.as_bytes()[from..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count()
    };
    let whole = digits(0);
    let end = match value.as_bytes().get(whole) {
        Some(b'.') if !integer => whole + 1 + digits(whole + 1),
        _ => whole,
    };
    if whole == 0 || !integer && value.as_bytes().get(end) == Some(&b'%') {
        return None;
    }
    if integer {
        return value[..end]
            .parse::<u32>()
            .ok()
            .filter(|&value| value <= i32::MAX as u32)
            .map(|value| value as f32);
    }
    value[..end]
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
}

pub(super) fn text_field(node: &crate::dom::Node) -> bool {
    node.tag == "textarea"
        || node.tag == "input"
            && !node.attrs.get("type").is_some_and(|kind| {
                [
                    "checkbox", "radio", "button", "submit", "reset", "image", "file", "color",
                    "range", "hidden",
                ]
                .iter()
                .any(|other| kind.eq_ignore_ascii_case(other))
            })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn field_size(
    doc: &Document,
    id: usize,
    style: &style::Style,
    units: &style::Lengths,
    text: &renderer::TextSystem,
    pixel_ratio: f32,
    width: bool,
    wrap_width: Option<f32>,
) -> [f32; 2] {
    let node = &doc.nodes[id];
    let textarea = node.tag == "textarea";
    if style.get(Prop::FIELD_SIZING) == Some("content") {
        let value = node.shown_text(
            node.current_value()
                .map_or_else(|| doc.text(id), str::to_owned),
        );
        let shown = if value.is_empty() {
            node.attrs.get("placeholder").cloned().unwrap_or_default()
        } else {
            value
        };
        let run = style::text_run(style, units, shown.len());
        let lines = text
            .shape_text(
                shown.into(),
                px(units.font.em),
                &[run],
                wrap_width
                    .filter(|_| {
                        textarea && !matches!(style.get(Prop::WHITE_SPACE), Some("pre" | "nowrap"))
                    })
                    .map(px),
                None,
                style::word_break(style),
            )
            .unwrap_or_default();
        return [
            if width {
                lines
                    .iter()
                    .map(|line| f32::from(line.unwrapped_layout.width))
                    .fold(0., f32::max)
                    + 1.
            } else {
                0.
            },
            if textarea {
                lines
                    .iter()
                    .map(|line| 1 + line.wrap_boundaries.len())
                    .sum::<usize>()
                    .max(1) as f32
                    * units.font.lh
            } else {
                units.font.lh
            },
        ];
    }
    let count = |name, default| {
        node.attrs
            .get(name)
            .and_then(|value| value.trim().parse::<u32>().ok())
            .filter(|&value| value > 0)
            .unwrap_or(default) as f32
    };
    let width = width.then(|| {
        let face = style::font(style);
        let em = px(units.font.em);
        let family = style
            .get(Prop::FONT_FAMILY)
            .and_then(|value| value.split(',').next())
            .map(|value| value.trim().trim_matches(['"', '\'']).to_ascii_lowercase())
            .unwrap_or_default();
        let id = text.resolve_font(&face);
        let scale = pixel_ratio;
        let (char_width, extra) = if "|american typewriter|arial hebrew|chalkboard|cochin|\
            corsiva hebrew|courier|euphemia ucas|geneva|gill sans|hei|helvetica|hoefler text|\
            inaimathi|kai|lucida grande|marker felt|monaco|mshtakan|new peninim mt|osaka|\
            raanana|stheiti|symbol|times|apple braille|apple ligothic|apple lisung|\
            apple symbols|applegothic|applemyungjo|#gungseo|#headlinea|#pcmyungjo|#pilgi|"
            .contains(&format!("|{family}|"))
        {
            let zero = text
                .ch_advance(id, em)
                .map_or(units.font.em / 2., f32::from);
            (zero * scale, 0.)
        } else {
            let average = if face.family.as_ref() == ".SystemUIFont" {
                (f32::from(text.x_height(id, em)) * scale).round()
            } else {
                text.advance(id, em, 'x')
                    .map_or(units.font.em / 2., |size| f32::from(size.width))
                    * scale
            };
            let ascent = f32::from(text.line_metrics(id, em * scale)[0]);
            (average, if textarea { 0. } else { ascent - average })
        };
        if textarea {
            let bar = match style.get(Prop::SCROLLBAR_WIDTH) {
                _ if style.get(Prop::OVERFLOW_Y).is_some_and(|v| v == "hidden") => 0.,
                Some("none") => 0.,
                Some("thin") => 14.,
                _ => 16.,
            };
            (char_width * count("cols", 20)).ceil() / scale + bar
        } else {
            (char_width * count("size", 20) + extra).ceil() / scale
        }
    });
    [
        width.unwrap_or(0.),
        if textarea { count("rows", 2) } else { 1. } * units.font.lh,
    ]
}

// WARNING: rounds a size up to layout units, so the content it was measured from doesn't wrap.
fn layout_ceil(size: f32, pixel_ratio: f32) -> f32 {
    (size * pixel_ratio * 64.).ceil() / 64. / pixel_ratio
}

fn in_flow(doc: &Document, info: &[cull::Info], id: usize) -> bool {
    let node = &doc.nodes[id];
    !matches!(node.tag.as_str(), "#comment" | "#doctype")
        && !(node.tag == "#text" && node.text.trim_ascii().is_empty())
        && info[id].flags & cull::OUT_OF_FLOW == 0
}

// WARNING: culled spacers take the margins of their first and last box from here, so these must stay the margins the box is laid out with.
pub(super) fn margins(
    build: &Build<'_>,
    id: usize,
    units: &style::Lengths,
    width: f32,
    flex_parent: bool,
    slot: bool,
) -> [f32; 4] {
    let mut margin = units.edges(&build.styles.values[id], style::MARGIN, width);
    let Some(parent) = build.doc.nodes[id].parent.filter(|_| !flex_parent && !slot) else {
        return margin;
    };
    let walked = build.margins.borrow().0.get(&parent).cloned();
    let walked = walked.unwrap_or_else(|| {
        let collapses = container(build, parent);
        let open =
            [0, 2].map(|side| collapses && margin_collapses_through(build, parent, side, width));
        // WARNING: animated values aren't in the revisions. Fonts, the viewport and container units change these
        // margins without a new revision, so `render_page` clears the kept walks with its measurements.
        let key = (build.info[parent].revision, width.to_bits(), open);
        let animated = build.info[parent].flags & cull::ANIMATED != 0;
        if let Some((old, used)) = build.view.walked.borrow().get(&parent)
            && *old == key
            && !animated
        {
            build.margins.borrow_mut().0.insert(parent, used.clone());
            return used.clone();
        }
        let mut used = rustc_hash::FxHashMap::default();
        if collapses {
            let children = flow(build, parent);
            let value = |set: [f32; 2]| set[0] + set[1];
            let mut pending = [0f32; 2];
            let mut emitted = 0.;
            let mut leading = open[0];
            for &child in &children {
                if !block(build, child) {
                    (leading, pending, emitted) = (false, [0.; 2], 0.);
                    continue;
                }
                let (top, bottom, empty) = adjoining(build, child, width);
                let mut sides = [0f32; 2];
                if leading {
                    if empty {
                        used.insert(child, sides);
                        continue;
                    }
                    leading = false;
                } else {
                    pending = [pending[0].max(top[0]), pending[1].min(top[1])];
                    sides[0] = value(pending) - emitted;
                    emitted += sides[0];
                }
                if empty {
                    pending = [pending[0].max(bottom[0]), pending[1].min(bottom[1])];
                    sides[1] = value(pending) - emitted;
                    emitted += sides[1];
                } else {
                    pending = bottom;
                    sides[1] = value(bottom);
                    emitted = sides[1];
                }
                used.insert(child, sides);
            }
            if open[1] {
                for child in children.into_iter().rev() {
                    let empty = block(build, child) && adjoining(build, child, width).2;
                    let Some(sides) = used.get_mut(&child) else {
                        break;
                    };
                    if empty {
                        *sides = [0.; 2];
                        continue;
                    }
                    sides[1] = 0.;
                    break;
                }
            }
        }
        let used = Rc::new(used);
        build.margins.borrow_mut().0.insert(parent, used.clone());
        if !animated {
            build
                .view
                .walked
                .borrow_mut()
                .insert(parent, (key, used.clone()));
        }
        used
    });
    if let Some(&[top, bottom]) = walked.get(&id) {
        margin[0] = top;
        margin[2] = bottom;
    }
    margin
}

fn flow(build: &Build<'_>, id: usize) -> Vec<usize> {
    build.doc.nodes[id]
        .children
        .iter()
        .copied()
        .filter(|&child| {
            in_flow(build.doc, build.info, child)
                && float_side(&build.styles.values[child]).is_none()
        })
        .collect()
}

fn block(build: &Build<'_>, id: usize) -> bool {
    !build.doc.nodes[id].tag.starts_with('#')
        && build.doc.nodes[id].tag != "br"
        && build.styles.values[id]
            .get(Prop::DISPLAY)
            .is_some_and(|value| !value.starts_with("inline") && value != "contents")
        || style::holds_block_box(build.doc, build.styles, id)
}

fn container(build: &Build<'_>, id: usize) -> bool {
    build.styles.values[id]
        .get(Prop::DISPLAY)
        .is_none_or(|value| {
            !value.contains("flex") && !value.contains("grid") && value != "contents"
        })
}

fn margin_collapses_through(build: &Build<'_>, id: usize, side: usize, width: f32) -> bool {
    let (doc, styles) = (build.doc, build.styles);
    let style = &styles.values[id];
    let edge = |prop: Prop| {
        style
            .get(prop)
            .and_then(|value| styles.lengths[id].resolve(value, width))
            .unwrap_or(0.)
    };
    let [padding, border, kind] = if side == 0 {
        [
            Prop::PADDING_TOP,
            Prop::BORDER_TOP_WIDTH,
            Prop::BORDER_TOP_STYLE,
        ]
    } else {
        [
            Prop::PADDING_BOTTOM,
            Prop::BORDER_BOTTOM_WIDTH,
            Prop::BORDER_BOTTOM_STYLE,
        ]
    };
    id != 0
        && doc.nodes[id].tag != "html"
        && doc.nodes[id]
            .parent
            .is_none_or(|parent| container(build, parent))
        && style
            .get(Prop::DISPLAY)
            .is_some_and(|value| matches!(value, "block" | "list-item"))
        && float_side(style).is_none()
        && [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
            .into_iter()
            .all(|key| style.get(key).is_none_or(|value| value == "visible"))
        && style
            .get(Prop::POSITION)
            .is_none_or(|value| !matches!(value, "absolute" | "fixed"))
        && edge(padding) == 0.
        && (edge(border) == 0.
            || style
                .get(kind)
                .is_none_or(|value| matches!(value, "none" | "hidden")))
        && (side == 0
            || style.get(Prop::HEIGHT).is_none_or(|value| value == "auto")
                && style
                    .get(Prop::MIN_HEIGHT)
                    .is_none_or(|value| value == "auto" || edge(Prop::MIN_HEIGHT) == 0.))
}

// WARNING: `None` when `min()` or `max()` inside make the value not linear in the base.
fn percent_calc(units: &style::Lengths, value: &str) -> Option<(f32, f32)> {
    if !value.starts_with("calc(") || !value.contains('%') {
        return None;
    }
    let low = units.resolve(value, 1000.)?;
    let high = units.resolve(value, 2000.)?;
    let check = units.resolve(value, 3000.)?;
    ((check - high - (high - low)).abs() < 0.01).then(|| {
        let fraction = (high - low) / 1000.;
        (fraction, low - fraction * 1000.)
    })
}

fn adjoining(build: &Build<'_>, id: usize, width: f32) -> ([f32; 2], [f32; 2], bool) {
    if let Some(&sets) = build.margins.borrow().1.get(&id) {
        return sets;
    }
    // WARNING: like the kept walks, this keys on the geometry revision, which animated values don't move.
    let key = (build.info[id].revision, width.to_bits());
    let animated = build.info[id].flags & cull::ANIMATED != 0;
    if !animated
        && let Some(&(old, sets)) = build.view.adjoined.borrow().get(&id)
        && old == key
    {
        build.margins.borrow_mut().1.insert(id, sets);
        return sets;
    }
    let (style, units) = (&build.styles.values[id], build.styles.lengths[id]);
    let own = |prop: Prop| {
        let value = style
            .get(prop)
            .and_then(|value| units.resolve(value, width))
            .unwrap_or(0.);
        [value.max(0.), value.min(0.)]
    };
    let (mut top, mut bottom) = (own(Prop::MARGIN_TOP), own(Prop::MARGIN_BOTTOM));
    let children = flow(build, id);
    // WARNING: this recurses into first and last children, so margins stop collapsing past the stack budget.
    let marker = 0u8;
    let deep = (&raw const marker as usize) < STACK_LIMIT.with(|limit| *limit);
    let blocks = container(build, id) && !deep;
    let [first, last] =
        [0, 2].map(|side| margin_collapses_through(build, id, side, width) && blocks);
    for (open, set, order) in [(first, &mut top, false), (last, &mut bottom, true)] {
        if !open {
            continue;
        }
        let mut list: Vec<usize> = children.clone();
        if order {
            list.reverse();
        }
        for child in list {
            if !block(build, child) {
                break;
            }
            let (child_top, child_bottom, empty) = adjoining(build, child, width);
            for adjoining in if order {
                [child_bottom, child_top]
            } else {
                [child_top, child_bottom]
            }
            .into_iter()
            .take(if empty { 2 } else { 1 })
            {
                *set = [set[0].max(adjoining[0]), set[1].min(adjoining[1])];
            }
            if !empty {
                break;
            }
        }
    }
    let empty = first
        && last
        && style
            .get(Prop::HEIGHT)
            .is_none_or(|value| value == "auto" || units.resolve(value, width) == Some(0.))
        && children
            .iter()
            .all(|&child| block(build, child) && adjoining(build, child, width).2);
    if empty {
        top = [top[0].max(bottom[0]), top[1].min(bottom[1])];
        bottom = top;
    }
    build
        .margins
        .borrow_mut()
        .1
        .insert(id, (top, bottom, empty));
    if !animated && !deep {
        build
            .view
            .adjoined
            .borrow_mut()
            .insert(id, (key, (top, bottom, empty)));
    }
    (top, bottom, empty)
}

pub(super) fn overscroll(style: &style::Style) -> renderer::Point<renderer::Overscroll> {
    let axis = |prop| match style.get(prop) {
        Some("contain") => renderer::Overscroll::Contain,
        Some("none") => renderer::Overscroll::None,
        _ => renderer::Overscroll::Auto,
    };
    point(
        axis(Prop::OVERSCROLL_BEHAVIOR_X),
        axis(Prop::OVERSCROLL_BEHAVIOR_Y),
    )
}

fn float_side(style: &style::Style) -> Option<bool> {
    let rtl = style.get(Prop::DIRECTION) == Some("rtl");
    match style.get(Prop::FLOAT) {
        Some("left") => Some(false),
        Some("right") => Some(true),
        Some("inline-start") => Some(rtl),
        Some("inline-end") => Some(!rtl),
        _ => None,
    }
}

type ColumnRule = (f32, renderer::Rgba, renderer::BorderStyle);

fn columns(
    style: &style::Style,
    units: &style::Lengths,
    width: f32,
) -> Option<(usize, f32, Option<ColumnRule>)> {
    let mut count = style
        .get(Prop::COLUMN_COUNT)
        .and_then(|value| value.parse::<usize>().ok());
    let mut column_width = style
        .get(Prop::COLUMN_WIDTH)
        .and_then(|value| units.resolve(value, width));
    for part in style::components(style.get(Prop::COLUMNS).unwrap_or(""), ' ') {
        if let Ok(value) = part.parse::<usize>() {
            count = count.or(Some(value));
        } else if let Some(value) = units.resolve(part, width) {
            column_width = column_width.or(Some(value));
        }
    }
    let gap = style
        .get(Prop::COLUMN_GAP)
        .filter(|value| *value != "normal")
        .and_then(|value| units.resolve(value, width))
        .unwrap_or(units.font.em)
        .max(0.);
    let fit = column_width
        .filter(|value| *value > 0.)
        .map(|value| (((width + gap) / (value + gap)).floor() as usize).max(1));
    let count = match (count, fit) {
        (Some(count), Some(fit)) => count.min(fit),
        (count, fit) => count.or(fit)?,
    }
    .clamp(1, 64);
    if count < 2 {
        return None;
    }
    let (mut rule_width, mut rule_style, mut rule_color) = (
        style.get(Prop::COLUMN_RULE_WIDTH),
        style.get(Prop::COLUMN_RULE_STYLE),
        style.get(Prop::COLUMN_RULE_COLOR),
    );
    for part in style::components(style.get(Prop::COLUMN_RULE).unwrap_or(""), ' ') {
        if matches!(
            part,
            "none"
                | "hidden"
                | "dotted"
                | "dashed"
                | "solid"
                | "double"
                | "groove"
                | "ridge"
                | "inset"
                | "outset"
        ) {
            rule_style = rule_style.or(Some(part));
        } else if style::color(part).is_some() {
            rule_color = rule_color.or(Some(part));
        } else {
            rule_width = rule_width.or(Some(part));
        }
    }
    let rule = rule_style
        .filter(|kind| !matches!(*kind, "none" | "hidden"))
        .map(|kind| {
            let width = rule_width
                .map_or(Some(3.), |value| units.resolve(value, width))
                .unwrap_or(0.);
            let color = rule_color
                .or_else(|| style.get(Prop::COLOR))
                .and_then(style::color)
                .map_or(renderer::black(), |(ink, alpha)| {
                    rgba(ink << 8 | alpha as u32)
                });
            (width, color, border_style(kind))
        })
        .filter(|(width, ..)| *width > 0.);
    Some((count, gap, rule))
}

fn column_rules(
    count: usize,
    gap: f32,
    width: f32,
    color: renderer::Rgba,
    kind: renderer::BorderStyle,
) -> Child {
    renderer::canvas(
        |_, _| (),
        move |bounds, _, window| {
            let column = (f32::from(bounds.size.width) - gap * (count - 1) as f32) / count as f32;
            for index in 1..count {
                let x = bounds.left() + px(index as f32 * (column + gap) - gap / 2. - width / 2.);
                window.paint_quad(renderer::quad(
                    Bounds::new(point(x, bounds.top()), size(px(width), bounds.size.height)),
                    renderer::Corners::default(),
                    renderer::transparent_black(),
                    renderer::Edges {
                        left: px(width),
                        ..Default::default()
                    },
                    color,
                    kind,
                ));
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into()
}

pub(super) fn legend(
    doc: &Document,
    styles: &style::ComputedStyles,
    fieldset: usize,
) -> Option<(usize, f32, f32)> {
    let corners = [
        Prop::BORDER_TOP_LEFT_RADIUS,
        Prop::BORDER_TOP_RIGHT_RADIUS,
        Prop::BORDER_BOTTOM_RIGHT_RADIUS,
        Prop::BORDER_BOTTOM_LEFT_RADIUS,
    ];
    if corners.into_iter().any(|corner| {
        styles.values[fieldset]
            .get(corner)
            .is_some_and(|value| !matches!(value, "0" | "0px"))
    }) {
        return None;
    }
    let legend = doc.nodes[fieldset]
        .children
        .iter()
        .copied()
        .find(|&child| {
            !doc.nodes[child].tag.starts_with('#')
                && styles.values[child].get(Prop::DISPLAY) != Some("none")
        })?;
    let style = &styles.values[legend];
    if doc.nodes[legend].tag != "legend"
        || style.get(Prop::FLOAT).is_some_and(|value| value != "none")
        || style
            .get(Prop::POSITION)
            .is_some_and(|value| matches!(value, "absolute" | "fixed"))
    {
        return None;
    }
    let units = styles.lengths[legend];
    let padding = units.edges(style, style::PADDING, 0.);
    let border = |[width, kind, _]: [Prop; 3]| {
        if style
            .get(kind)
            .is_none_or(|kind| matches!(kind, "none" | "hidden"))
        {
            0.
        } else {
            style
                .get(width)
                .and_then(|value| units.resolve(value, 0.))
                .unwrap_or(3.)
        }
    };
    let height = units.font.lh
        + padding[0]
        + padding[2]
        + border(style::BORDERS[0])
        + border(style::BORDERS[2]);
    let margin = units.edges(style, style::MARGIN, 0.);
    Some((
        legend,
        margin[0] + height + margin[2],
        margin[0] + height / 2.,
    ))
}

fn fieldset_border(
    legend: usize,
    height: f32,
    middle: f32,
    widths: [f32; 4],
    colors: [renderer::Rgba; 4],
    kinds: [&str; 4],
    view: Rc<super::PageState>,
) -> Child {
    let kinds = kinds.map(str::to_owned);
    renderer::canvas(
        |_, _| (),
        move |bounds, _, window| {
            let gap = view
                .host
                .borrow()
                .dom
                .get(&view.id)
                .and_then(|dom| dom.bounds.get(&legend).copied())
                .map(|[x, _, width, _]| [px(x), px(x + width)]);
            let top = bounds.top() + px(middle - widths[0] / 2.);
            let [left, right, bottom] = [bounds.left(), bounds.right(), bounds.bottom()];
            let sides = [
                Bounds::from_corners(point(left, top), point(right, top + px(widths[0]))),
                Bounds::from_corners(point(right - px(widths[1]), top), point(right, bottom)),
                Bounds::from_corners(point(left, bottom - px(widths[2])), point(right, bottom)),
                Bounds::from_corners(point(left, top), point(left + px(widths[3]), bottom)),
            ];
            for (side, rect) in sides.into_iter().enumerate() {
                if widths[side] <= 0. {
                    continue;
                }
                let raised = side == 0 || side == 3;
                let (outer, inner) = match kinds[side].as_str() {
                    "groove" => (
                        shade_border(colors[side], raised),
                        shade_border(colors[side], !raised),
                    ),
                    "ridge" => (
                        shade_border(colors[side], !raised),
                        shade_border(colors[side], raised),
                    ),
                    "inset" => (
                        shade_border(colors[side], raised),
                        shade_border(colors[side], raised),
                    ),
                    "outset" => (
                        shade_border(colors[side], !raised),
                        shade_border(colors[side], !raised),
                    ),
                    _ => (colors[side], colors[side]),
                };
                let half = px(widths[side] / 2.);
                let halves = match side {
                    0 => [
                        (rect.top(), rect.top() + half),
                        (rect.top() + half, rect.bottom()),
                    ],
                    2 => [
                        (rect.bottom() - half, rect.bottom()),
                        (rect.top(), rect.bottom() - half),
                    ],
                    1 => [
                        (rect.right() - half, rect.right()),
                        (rect.left(), rect.right() - half),
                    ],
                    _ => [
                        (rect.left(), rect.left() + half),
                        (rect.left() + half, rect.right()),
                    ],
                };
                for ((start, end), color) in halves.into_iter().zip([outer, inner]) {
                    let part = if side % 2 == 0 {
                        Bounds::from_corners(point(rect.left(), start), point(rect.right(), end))
                    } else {
                        Bounds::from_corners(point(start, rect.top()), point(end, rect.bottom()))
                    };
                    let pieces = match gap.filter(|_| side == 0) {
                        Some([from, to]) => vec![
                            Bounds::from_corners(
                                part.origin,
                                point(from.max(part.left()), part.bottom()),
                            ),
                            Bounds::from_corners(
                                point(to.min(part.right()), part.top()),
                                part.bottom_right(),
                            ),
                        ],
                        None => vec![part],
                    };
                    for piece in pieces.into_iter().filter(|piece| piece.size.width > px(0.)) {
                        window.paint_quad(renderer::fill(piece, color));
                    }
                }
            }
        },
    )
    .absolute()
    .top(px(-height))
    .left(px(-widths[3]))
    .right(px(-widths[1]))
    .bottom(px(-widths[2]))
    .into()
}

fn border_style(kind: &str) -> renderer::BorderStyle {
    match kind {
        "dashed" => renderer::BorderStyle::Dashed,
        "dotted" => renderer::BorderStyle::Dotted,
        "double" => renderer::BorderStyle::Double,
        _ => renderer::BorderStyle::Solid,
    }
}

fn shade_border(color: renderer::Rgba, dark: bool) -> renderer::Rgba {
    let rgba = color;
    let value = rgba.r.max(rgba.g).max(rgba.b);
    if value <= 0. {
        let gray = if dark { 0.33 } else { 0.66 };
        return renderer::Rgba {
            r: gray,
            g: gray,
            b: gray,
            a: rgba.a,
        };
    }
    let end = if dark { 0. } else { 1. };
    let distance = (rgba.r - end).powi(2) + (rgba.g - end).powi(2) + (rgba.b - end).powi(2);
    if distance <= 3. * (if dark { 32. } else { 20. } / 255.0f32).powi(2) {
        return color;
    }
    let factor = if dark {
        ((value - 0.33) / value).max(0.)
    } else {
        (value + 0.33).min(1.) / value
    };
    renderer::Rgba {
        r: (rgba.r * factor).min(1.),
        g: (rgba.g * factor).min(1.),
        b: (rgba.b * factor).min(1.),
        a: rgba.a,
    }
}

fn inset_shadows(
    shadows: Vec<renderer::BoxShadow>,
    radius: renderer::Radii<renderer::Length>,
    borders: [f32; 4],
) -> Child {
    renderer::canvas(
        |_, _| (),
        move |bounds, _, window| {
            let outer = size(
                bounds.size.width + px(borders[1] + borders[3]),
                bounds.size.height + px(borders[0] + borders[2]),
            );
            let radii = radius.resolve(outer).inset(
                px(borders[0]),
                px(borders[1]),
                px(borders[2]),
                px(borders[3]),
            );
            window.paint_inset_shadows(bounds, radii, &shadows)
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into()
}

fn svg_layer(id: usize, inline: bool, view: Rc<super::PageState>) -> Child {
    let geometry = view.clone();
    renderer::canvas(
        move |bounds, window| {
            let [width, height] = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
            if !inline || width <= 0. || height <= 0. {
                return;
            }
            let mut host = geometry.host.borrow_mut();
            let state = host.dom.entry(geometry.id).or_default();
            if let Some(source) = state.svg_sources.get(&id)
                && let Some(entry) = state.svg_boxes.get_mut(&id)
            {
                if entry.0 != *source {
                    // WARNING: paint draws the tree parsed here, so a changed source is parsed once.
                    // WARNING: a draw that repeats skips paint, so a tree of the old source can still wait here.
                    state.svg_trees.remove(&id);
                    let (size, content, boxes) = match state.svg_cache.get(source) {
                        Some((geometry, _)) => geometry.clone(),
                        None => {
                            let tree = super::canvas::svg_tree(source).ok();
                            let geometry = tree
                                .as_ref()
                                .map(super::canvas::svg_boxes)
                                .unwrap_or_default();
                            if let Some(tree) = tree {
                                state.svg_trees.insert(id, tree);
                                state
                                    .svg_cache
                                    .insert(source.clone(), (geometry.clone(), Vec::new()));
                            }
                            geometry
                        }
                    };
                    let names = &entry.1;
                    entry.2 = size;
                    entry.4 = content;
                    entry.3 = boxes
                        .into_iter()
                        .filter_map(|(name, canvas, object)| {
                            let node = match name.strip_prefix("apps-") {
                                Some(mark) => names.get(mark.parse::<usize>().ok()?)?.1,
                                None => names.iter().find(|(other, _)| *other == name)?.1,
                            };
                            Some((node, canvas, object))
                        })
                        .collect();
                    entry.0 = source.clone();
                    if let Some(kept) = geometry
                        .svgs
                        .borrow_mut()
                        .get_mut(&id)
                        .filter(|kept| kept.2 == *source)
                    {
                        kept.3 = entry.clone();
                    }
                }
                let [scale_x, scale_y] =
                    [0, 1].map(|axis| [width, height][axis] / entry.2[axis].max(f32::EPSILON));
                for &(node, [x, y, w, h], _) in &entry.3 {
                    let rect = window.transformed_bounds(Bounds::new(
                        point(
                            bounds.left() + px(x * scale_x),
                            bounds.top() + px(y * scale_y),
                        ),
                        size(px(w * scale_x), px(h * scale_y)),
                    ));
                    state.bounds.insert(
                        node,
                        [
                            rect.origin.x.into(),
                            rect.origin.y.into(),
                            rect.size.width.into(),
                            rect.size.height.into(),
                        ],
                    );
                }
            }
        },
        move |bounds, _, window| {
            let [width, height] = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
            if width <= 0. || height <= 0. {
                return;
            }
            let scale = window.scale_factor();
            let raster =
                [width, height].map(|extent| (extent * scale).ceil().clamp(1., 4096.) as u32);
            let view = &*view;
            let mut host = view.host.borrow_mut();
            let state = host.dom.entry(view.id).or_default();
            let source = if inline {
                state.svg_sources.get(&id).map(String::as_str)
            } else {
                state.svg_images.get(&id).map(|(source, _)| &**source)
            };
            let Some(source) = source else { return };
            if state.svgs.get(&id).is_none_or(|(old, image)| {
                !inline && old != source
                    || u32::from(image.size().width) != raster[0]
                    || u32::from(image.size().height) != raster[1]
            }) {
                let shared = state
                    .svg_cache
                    .get(source)
                    .filter(|_| inline)
                    .and_then(|(_, images)| {
                        images.iter().find(|image| {
                            [
                                u32::from(image.size().width),
                                u32::from(image.size().height),
                            ] == raster
                        })
                    })
                    .cloned();
                let image = match (&shared, state.svg_trees.remove(&id).filter(|_| inline)) {
                    (Some(image), _) => Ok(image.clone()),
                    (_, Some(tree)) => super::canvas::render_svg(&tree, raster[0], raster[1], None),
                    _ => super::canvas::svg(source, raster[0], raster[1], None),
                };
                match image {
                    Ok(image) => {
                        // WARNING: only icon sizes stay, so at most 256 of them hold under 10 MB.
                        if inline && shared.is_none() && raster[0] <= 96 && raster[1] <= 96 {
                            if let Some((_, images)) = state.svg_cache.get_mut(source) {
                                images.push(image.clone());
                            }
                            if state.svg_cache.len() > 256 {
                                for image in state.svg_cache.drain().flat_map(|(_, entry)| entry.1)
                                {
                                    if Arc::strong_count(&image) == 1 {
                                        window.drop_image(image);
                                    }
                                }
                            }
                        }
                        if let Some((_, old)) = state.svgs.insert(id, (source.to_owned(), image))
                            && Arc::strong_count(&old) == 1
                        {
                            window.drop_image(old);
                        }
                    }
                    Err(error) => eprintln!("apps: SVG: {error}"),
                }
            }
            let image = state.svgs.get(&id).map(|(_, image)| image.clone());
            drop(host);
            if let Some(image) = image {
                window.paint_image(bounds, bounds, renderer::Corners::default(), image, false);
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into()
}

#[cfg(test)]
mod tests {
    #[test]
    fn dimension_attributes_parse_like_html() {
        for (value, integer, expected) in [
            ("300", true, Some(300.)),
            (" 100px", true, Some(100.)),
            ("1e9", true, Some(1.)),
            ("2.5", true, Some(2.)),
            ("2.5", false, Some(2.5)),
            ("NaN", true, None),
            ("inf", false, None),
            ("-50", true, None),
            ("50%", false, None),
            ("50%", true, Some(50.)),
            ("+7", true, Some(7.)),
            ("+7", false, None),
            ("2147483648", true, None),
            ("", true, None),
            ("99999999999", true, None),
        ] {
            assert_eq!(super::dimension(value, integer), expected, "{value}");
        }
    }
}
