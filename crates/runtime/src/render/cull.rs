use crate::style::Prop;
use crate::{dom::Document, style};
use renderer::{Child, px};
use std::{collections::HashMap, rc::Rc};

pub(crate) const UNCULLABLE: u16 = 1;
pub(crate) const CHANGED: u16 = 2;
pub(crate) const MOVES_CONTENT: u16 = 4;
pub(crate) const OUT_OF_FLOW: u16 = 8;
pub(crate) const HIDDEN: u16 = 16;
pub(crate) const PLAIN_BLOCK: u16 = 32;
pub(crate) const DEFINITE_SIZE: u16 = 64;
pub(crate) const GEOMETRY_CHANGED: u16 = 128;
pub(crate) const CHILDREN_CHANGED: u16 = 256;
pub(crate) const READS_VIEWPORT: u16 = 512;
pub(crate) const COMPOSITED: u16 = 1024;
pub(crate) const TRANSFORMED: u16 = 2048;
pub(crate) const ALWAYS_REBUILT: u16 = 4096;
pub(crate) const ANIMATED: u16 = 8192;
pub(crate) const READS_LAST_FRAME: u16 = 16384;
pub(crate) const READS_HEIGHT: u16 = 32768;
pub(crate) const INPUT: u16 = 1;

pub(crate) fn reads_height(style: &style::Style) -> bool {
    [
        Prop::HEIGHT,
        Prop::MIN_HEIGHT,
        Prop::MAX_HEIGHT,
        Prop::FLEX_BASIS,
        Prop::TOP,
        Prop::BOTTOM,
        Prop::ROW_GAP,
        Prop::BORDER_TOP_LEFT_RADIUS,
        Prop::BORDER_TOP_RIGHT_RADIUS,
        Prop::BORDER_BOTTOM_LEFT_RADIUS,
        Prop::BORDER_BOTTOM_RIGHT_RADIUS,
    ]
    .into_iter()
    .any(|prop| style.get(prop).is_some_and(|value| value.contains('%')))
        || style
            .get(Prop::POSITION)
            .is_some_and(|value| matches!(value, "absolute" | "fixed"))
            && [Prop::TOP, Prop::BOTTOM]
                .into_iter()
                .all(|prop| style.get(prop).is_some_and(|value| value != "auto"))
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Info {
    pub(crate) revision: u64,
    pub(crate) subtree: u64,
    pub(crate) flags: u16,
}

#[derive(Default)]
pub(crate) struct Cull {
    pub(crate) info: Vec<Info>,
    pub(crate) placed: Vec<(u64, [f32; 2], [f32; 4])>,
    pub(crate) widths: rustc_hash::FxHashMap<usize, [f32; 2]>,
    pub(crate) off: bool,
    pub(crate) enabled: bool,
    pub(crate) frame: [f32; 4],
    pub(crate) resized: bool,
    pub(crate) offsets: HashMap<usize, renderer::Point<renderer::Pixels>>,
    pub(crate) visible: Vec<bool>,
    pub(crate) walk: crate::dom::Walk,
    pub(crate) cells: Vec<usize>,
    pub(crate) inputs: Vec<usize>,
    pub(crate) seen: Option<(u64, Option<usize>, Rc<crate::style::ComputedStyles>)>,
    pub(crate) subtrees: Subtrees,
}

pub(crate) type Subtrees = (u64, rustc_hash::FxHashMap<usize, Vec<usize>>);

impl Cull {
    pub(crate) fn trim(&mut self, len: usize) {
        self.placed.truncate(len);
        self.placed.shrink_to_fit();
        self.walk = Default::default();
        self.cells = Vec::new();
        if self.info.len() > len {
            self.info.truncate(len);
            self.info.shrink_to_fit();
            self.visible.truncate(len);
            self.visible.shrink_to_fit();
            self.seen = None;
        }
    }
}

pub(super) struct Zone {
    pub(super) top: f32,
    pub(super) bottom: f32,
    pub(super) clips: Vec<usize>,
    pub(super) resized: bool,
}

impl Zone {
    fn visible(&self, bounds: &crate::dom::LayoutMirror, window: &renderer::Window) -> [f32; 2] {
        self.clips.iter().filter_map(|clip| bounds.get(clip)).fold(
            [0., f32::from(window.viewport_size().height)],
            |[low, high], rect| [low.max(rect[1]), high.min(rect[1] + rect[3])],
        )
    }
}

#[derive(Debug, PartialEq)]
pub(super) struct Run {
    pub(super) members: Vec<usize>,
    pub(super) top: f32,
    pub(super) height: f32,
    pub(super) width: f32,
    pub(super) margins: [f32; 2],
    pub(super) watch: Vec<(usize, f32)>,
}

pub(super) fn cosmetic(old: &style::Style, new: &style::Style) -> bool {
    let paint = |prop: Prop| {
        let name = prop.name();
        prop.is_custom()
            || [
                "background",
                "outline",
                "text-decoration",
                "transition",
                "animation",
                "mask",
                "-webkit-mask",
                "fill",
                "stroke",
            ]
            .iter()
            .any(|prefix| name.starts_with(prefix))
            || name.starts_with("border-") && name.ends_with("-color")
            || matches!(
                prop,
                Prop::COLOR
                    | Prop::OPACITY
                    | Prop::VISIBILITY
                    | Prop::CURSOR
                    | Prop::BOX_SHADOW
                    | Prop::TEXT_SHADOW
                    | Prop::CARET_COLOR
                    | Prop::ACCENT_COLOR
                    | Prop::FILTER
                    | Prop::BACKDROP_FILTER
                    | Prop::WEBKIT_BACKDROP_FILTER
                    | Prop::MIX_BLEND_MODE
                    | Prop::ISOLATION
                    | Prop::CLIP_PATH
                    | Prop::POINTER_EVENTS
                    | Prop::USER_SELECT
                    | Prop::WEBKIT_USER_SELECT
                    | Prop::Z_INDEX
                    | Prop::TRANSFORM
                    | Prop::TRANSFORM_ORIGIN
                    | Prop::TRANSLATE
                    | Prop::ROTATE
                    | Prop::SCALE
                    | Prop::PERSPECTIVE
                    | Prop::PERSPECTIVE_ORIGIN
                    | Prop::BACKFACE_VISIBILITY
                    | Prop::WILL_CHANGE
                    | Prop::WEBKIT_TAP_HIGHLIGHT_COLOR
                    | Prop::WEBKIT_FONT_SMOOTHING
                    | Prop::MOZ_OSX_FONT_SMOOTHING
            )
    };
    let (mut a, mut b) = (old.iter().peekable(), new.iter().peekable());
    loop {
        let name = match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return true,
            (Some((x, u)), Some((y, v))) if x == y => {
                a.next();
                b.next();
                if u == v {
                    continue;
                }
                x
            }
            (Some((x, _)), Some((y, _))) if x < y => {
                a.next();
                x
            }
            (Some((x, _)), None) => {
                a.next();
                x
            }
            (_, Some((y, _))) => {
                b.next();
                y
            }
        };
        if !paint(name) {
            return false;
        }
    }
}

pub(super) fn content(node: &crate::dom::Node) -> (u64, u64) {
    use std::hash::{BuildHasher, Hash, Hasher};
    let mut shape = rustc_hash::FxHasher::default();
    node.tag.hash(&mut shape);
    node.text.hash(&mut shape);
    // WARNING: a form control's value and checked state draw like its text.
    node.value.hash(&mut shape);
    (node.state & (crate::dom::CHECKED | crate::dom::DIRTY)).hash(&mut shape);
    let mut own = shape.finish();
    let mut attrs = rustc_hash::FxHasher::default();
    for pair in node.attrs.iter() {
        pair.hash(&mut attrs);
        let name = pair.0.as_str();
        if !(matches!(name, "class" | "style" | "id" | "title")
            || name.starts_with("data-")
            || name.starts_with("aria-"))
        {
            own = own.wrapping_add(rustc_hash::FxBuildHasher.hash_one(pair));
        }
    }
    (own, attrs.finish())
}

pub(super) fn flags(tag: &str, style: &style::Style, units: &style::Lengths, margin: f32) -> u16 {
    let display = style.get(Prop::DISPLAY).unwrap_or("inline");
    let position = style.get(Prop::POSITION).unwrap_or("static");
    let auto = |prop| style.get(prop).is_none_or(|value| value == "auto");
    let moved = [Prop::TRANSFORM, Prop::TRANSLATE, Prop::ROTATE, Prop::SCALE]
        .into_iter()
        .any(|name| style.get(name).is_some_and(|value| value != "none"))
        || style
            .get(Prop::WRITING_MODE)
            .is_some_and(|value| value.starts_with("vertical") || value.starts_with("sideways"));
    let blocked = moved
        || matches!(
            tag,
            "iframe" | "webview" | "canvas" | "video" | "embed" | "object"
        )
        || matches!(position, "absolute" | "fixed" | "sticky")
        || style
            .get(Prop::CONTAINER_TYPE)
            .is_some_and(|value| value != "normal")
        || style.get(Prop::BOX_SHADOW).is_some_and(|value| {
            units.shadows(value, false).iter().any(|shadow| {
                f32::from(shadow.offset.y.abs() + shadow.blur_radius + shadow.spread_radius)
                    >= margin
            })
        });
    let solid = !tag.starts_with('#')
        && !display.starts_with("inline")
        && !matches!(display, "contents" | "none")
        && style.get(Prop::FLOAT).is_none_or(|value| value == "none")
        && (position == "static"
            || position == "relative"
                && [Prop::TOP, Prop::RIGHT, Prop::BOTTOM, Prop::LEFT]
                    .into_iter()
                    .all(auto))
        && [Prop::MARGIN_TOP, Prop::MARGIN_BOTTOM]
            .into_iter()
            .all(|name| style.get(name).is_none_or(|value| value != "auto"))
        && style
            .get(Prop::FLEX_GROW)
            .is_none_or(|value| value.parse::<f32>().is_ok_and(|grow| grow == 0.))
        && style
            .get(Prop::ALIGN_SELF)
            .is_none_or(|value| !value.contains("baseline"))
        && ![
            Prop::HEIGHT,
            Prop::MIN_HEIGHT,
            Prop::MAX_HEIGHT,
            Prop::FLEX_BASIS,
        ]
        .into_iter()
        .any(|name| style.get(name).is_some_and(|value| value.contains('%')));
    // WARNING: scroll boxes build their bars from the scroll state, which changes without a new revision.
    let volatile = display != "none"
        && [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
            .into_iter()
            .any(|prop| {
                style
                    .get(prop)
                    .is_some_and(|value| matches!(value, "scroll" | "auto"))
            });
    let framed = !matches!(
        display,
        "block"
            | "inline"
            | "inline-block"
            | "flex"
            | "inline-flex"
            | "grid"
            | "inline-grid"
            | "flow-root"
            | "contents"
            | "none"
    ) || style.get(Prop::FLOAT).is_some_and(|value| value != "none")
        || [
            Prop::FILTER,
            Prop::WEBKIT_FILTER,
            Prop::CLIP_PATH,
            Prop::WEBKIT_CLIP_PATH,
            Prop::MASK_IMAGE,
            Prop::WEBKIT_MASK_IMAGE,
            Prop::BACKDROP_FILTER,
            Prop::WEBKIT_BACKDROP_FILTER,
        ]
        .into_iter()
        .any(|prop| style.get(prop).is_some_and(|value| value.contains("url(")))
        || [
            Prop::COUNTER_RESET,
            Prop::COUNTER_INCREMENT,
            Prop::COUNTER_SET,
        ]
        .into_iter()
        .any(|prop| style.get(prop).is_some_and(|value| value != "none"))
        || [Prop::COLUMN_COUNT, Prop::COLUMN_WIDTH, Prop::COLUMNS]
            .into_iter()
            .any(|prop| style.get(prop).is_some_and(|value| value != "auto"));
    [
        (moved, MOVES_CONTENT),
        (blocked, UNCULLABLE),
        (volatile, ALWAYS_REBUILT),
        (framed, READS_LAST_FRAME),
        (
            display == "none" || matches!(position, "absolute" | "fixed"),
            OUT_OF_FLOW,
        ),
        (display == "none", HIDDEN),
        (solid, PLAIN_BLOCK),
        (
            !auto(Prop::HEIGHT) || !auto(Prop::FLEX_BASIS),
            DEFINITE_SIZE,
        ),
        (reads_height(style), READS_HEIGHT),
        (
            style.iter().any(|(prop, value)| {
                let bytes = value.as_bytes();
                !prop.is_custom()
                    && bytes.iter().enumerate().any(|(index, &byte)| {
                        byte == b'v'
                            && matches!(
                                bytes.get(index + 1),
                                Some(b'w' | b'h' | b'i' | b'b' | b'm')
                            )
                            && bytes[..index]
                                .iter()
                                .rev()
                                .find(|byte| !matches!(byte, b's' | b'l' | b'd'))
                                .is_some_and(|byte| byte.is_ascii_digit() || *byte == b'.')
                    })
            }),
            READS_VIEWPORT,
        ),
    ]
    .into_iter()
    .fold(0, |flags, (on, bit)| if on { flags | bit } else { flags })
}

pub(super) fn member(
    info: Info,
    (placed, [width, _], margins): (u64, [f32; 2], [f32; 4]),
    bounds: Option<&[f32; 4]>,
    zone: &Zone,
    margin: f32,
    flex: bool,
) -> Option<([f32; 4], [f32; 2])> {
    let fits = info.flags & (UNCULLABLE | PLAIN_BLOCK) == PLAIN_BLOCK
        && (!flex || info.flags & DEFINITE_SIZE == 0)
        && (!zone.resized || info.flags & READS_VIEWPORT == 0);
    if !fits || info.revision == 0 || placed != info.revision {
        return None;
    }
    let [x, y, _, height] = *bounds?;
    (y + height < zone.top - margin || y > zone.bottom + margin)
        .then_some(([x, y, width, height], [margins[0], margins[2]]))
}

pub(super) fn plan(
    children: &[usize],
    shown: impl Fn(usize) -> bool,
    mut member: impl FnMut(usize) -> Option<([f32; 4], [f32; 2])>,
    gap: f32,
) -> (Vec<usize>, Vec<Option<Run>>) {
    let mut slots = vec![0; children.len()];
    let mut runs: Vec<Option<Run>> = Vec::new();
    let mut open = false;
    for (index, &child) in children.iter().enumerate() {
        if !shown(index) {
            continue;
        }
        let Some(([_, y, width, height], margins)) = member(child) else {
            open = false;
            continue;
        };
        match runs.last_mut().and_then(Option::as_mut).filter(|run| {
            open && (y - run.top - run.height - run.margins[1] - gap - margins[0]).abs() <= 0.5
        }) {
            Some(run) => {
                run.members.push(child);
                run.height = y + height - run.top;
                run.width = run.width.max(width);
                run.margins[1] = margins[1];
            }
            None => runs.push(Some(Run {
                members: vec![child],
                top: y,
                height,
                width,
                margins,
                watch: Vec::new(),
            })),
        }
        open = true;
        slots[index] = runs.len();
    }
    (slots, runs)
}

pub(super) fn lines(
    children: &[usize],
    shown: impl Fn(usize) -> bool,
    mut item: impl FnMut(usize) -> Option<(bool, [f32; 4])>,
    gap: f32,
) -> (Vec<usize>, Vec<Option<Run>>) {
    let mut lines: Vec<(Vec<usize>, bool, [f32; 2])> = Vec::new();
    let mut right = f32::MIN;
    for (index, &child) in children.iter().enumerate() {
        if !shown(index) {
            continue;
        }
        let Some((member, [left, top, end, bottom])) =
            item(child).filter(|(_, [left, _, end, _])| end > left)
        else {
            // WARNING: the open line may end elsewhere, and a zero width box could share a line with a spacer.
            lines.pop();
            break;
        };
        match lines.last_mut().filter(|_| left >= right - 0.5) {
            Some(line) => {
                line.0.push(index);
                line.1 &= member;
                line.2 = [line.2[0].min(top), line.2[1].max(bottom)];
            }
            None => lines.push((vec![index], member, [top, bottom])),
        }
        right = end;
    }
    let mut slots = vec![0; children.len()];
    let mut runs: Vec<Option<Run>> = Vec::new();
    let mut open = false;
    for (indexes, members, [top, bottom]) in lines {
        if !members {
            open = false;
            continue;
        }
        let ids = indexes.iter().map(|&index| children[index]);
        match runs
            .last_mut()
            .and_then(Option::as_mut)
            .filter(|run| open && (top - run.top - run.height - gap).abs() <= 0.5)
        {
            Some(run) => {
                run.members.extend(ids);
                run.height = bottom - run.top;
            }
            None => runs.push(Some(Run {
                members: ids.collect(),
                top,
                height: bottom - top,
                width: 0.,
                margins: [0.; 2],
                watch: Vec::new(),
            })),
        }
        open = true;
        for index in indexes {
            slots[index] = runs.len();
        }
    }
    (slots, runs)
}

pub(super) fn shift(
    doc: &Document,
    shadows: &rustc_hash::FxHashMap<usize, usize>,
    subtrees: &mut Subtrees,
    bounds: &mut crate::dom::LayoutMirror,
    roots: &[usize],
    [x, y]: [f32; 2],
) {
    // WARNING: a scroll moves every culled subtree, so their nodes are listed once per document revision.
    if subtrees.0 != doc.nodes.latest() {
        subtrees.0 = doc.nodes.latest();
        subtrees.1.clear();
    }
    for &root in roots {
        let nodes = subtrees.1.entry(root).or_insert_with(|| {
            let (mut nodes, mut pending) = (Vec::new(), vec![root]);
            while let Some(id) = pending.pop() {
                nodes.push(id);
                pending.extend(
                    doc.nodes
                        .get(id)
                        .into_iter()
                        .flat_map(|node| &node.children),
                );
                pending.extend(shadows.get(&id));
            }
            nodes
        });
        for id in nodes {
            if let Some(rect) = bounds.get_mut(id) {
                rect[0] += x;
                rect[1] += y;
            }
        }
    }
}

pub(super) fn spacer(
    run: Run,
    parent: usize,
    [left, width]: [f32; 2],
    zone: Rc<Zone>,
    view: Rc<super::PageState>,
) -> Child {
    let Run {
        members,
        top,
        height,
        width: size,
        margins,
        watch,
    } = run;
    renderer::canvas(
        move |bounds, window| {
            let view = &*view;
            let mut host = view.host.borrow_mut();
            let host = &mut *host;
            let Some(dom) = host.dom.get_mut(&view.id) else {
                return;
            };
            let [low, high] = zone.visible(&dom.bounds, window);
            let y = f32::from(bounds.top());
            let x = dom.bounds.get(&parent).map(|rect| rect[0]);
            // WARNING: the members were laid out at the old parent width, so a new width lays them out again.
            let resized = x.is_none()
                || dom
                    .cull
                    .widths
                    .get(&parent)
                    .is_none_or(|now| (now[0] - width).abs() > 0.01);
            // WARNING: a changed box before a run of wrapped lines may move boxes to other lines, which the spacer cannot follow.
            let reflowed = watch.iter().any(|&(id, width)| {
                dom.cull
                    .placed
                    .get(id)
                    .is_none_or(|placed| (placed.1[0] - width).abs() > 0.01)
            });
            if resized || reflowed || y < high && y + height > low {
                // WARNING: a kept subtree keeps this spacer and its old width, so asking for a frame alone would
                // repeat this in every frame, forever. The next frame builds the path down to it again.
                if let Some(page) = host.pages.get(&view.id) {
                    let mut kept = view.kept.borrow_mut();
                    let mut node = Some(parent);
                    while let Some(id) = node {
                        kept.remove(&id);
                        node = page.doc.nodes[id].parent;
                    }
                }
                dom.cull.off |= resized || reflowed;
                window.redraw();
            }
            let delta = [x.unwrap_or(left) - left, y - top];
            if delta != [0., 0.]
                && let Some(page) = host.pages.get(&view.id)
            {
                shift(
                    &page.doc,
                    &dom.shadows,
                    &mut dom.cull.subtrees,
                    &mut dom.bounds,
                    &members,
                    delta,
                );
            }
        },
        |_, _, _| {},
    )
    .w(px(size))
    .h(px(height))
    .mt(px(margins[0]))
    .mb(px(margins[1]))
    .flex_shrink_0()
    .into()
}

pub(super) fn anchor(
    items: Vec<(usize, [f32; 4])>,
    zone: Rc<Zone>,
    view: Rc<super::PageState>,
) -> Child {
    renderer::canvas(
        move |bounds, window| {
            let view = &*view;
            let mut host = view.host.borrow_mut();
            let host = &mut *host;
            let (Some(dom), Some(page)) = (host.dom.get_mut(&view.id), host.pages.get(&view.id))
            else {
                return;
            };
            let [low, high] = zone.visible(&dom.bounds, window);
            let [left, top] = [f32::from(bounds.left()), f32::from(bounds.top())];
            for &(id, [x, y, _, height]) in &items {
                let [x, y] = [left + x, top + y];
                if y < high && y + height > low {
                    window.redraw();
                }
                if let Some(&[old_x, old_y, ..]) = dom.bounds.get(&id)
                    && [old_x, old_y] != [x, y]
                {
                    shift(
                        &page.doc,
                        &dom.shadows,
                        &mut dom.cull.subtrees,
                        &mut dom.bounds,
                        &[id],
                        [x - old_x, y - old_y],
                    );
                }
            }
        },
        |_, _, _| {},
    )
    .absolute()
    .left(px(0.))
    .top(px(0.))
    .size(px(0.))
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(html: &str) -> (Document, Rc<style::ComputedStyles>, impl Fn(&str) -> usize) {
        let mut doc = Document::parse(html);
        let styles = crate::dom::styles(&mut doc, &mut Default::default(), [800., 600.], None);
        let ids: HashMap<String, usize> = (0..doc.nodes.len())
            .filter_map(|id| Some((doc.nodes[id].attrs.get("id")?.clone(), id)))
            .collect();
        (doc, styles, move |name: &str| ids[name])
    }

    #[test]
    fn flags_follow_the_box_style() {
        let (doc, styles, id) = page(
            "<style>#s{box-shadow:0 0 0 900px red}#t{box-shadow:0 2px 4px red}</style>\
             <div id=a></div><div id=b style='transform:rotate(1deg)'></div>\
             <div id=c style='position:absolute'></div><div id=d style='display:none'></div>\
             <canvas id=e style='display:block'></canvas><div id=f style='container-type:inline-size'></div>\
             <div id=s></div><div id=t></div><div id=g style='position:sticky'></div>\
             <div id=h style='writing-mode:vertical-rl'></div><div id=i style='margin:0 10px 0 6px'></div>\
             <div id=j style='margin-top:4px'></div><span id=k></span><div id=l style='float:left'></div>\
             <div id=m style='position:relative;top:2px'></div><div id=n style='position:relative'></div>\
             <div id=o style='flex-grow:1'></div><div id=p style='height:50%'></div>\
             <div id=q style='height:20px'></div><div id=r style='margin:auto'></div>\
             <div id=u style='margin-bottom:2%'></div><ul><li id=v></li></ul>\
             <div id=w style='min-height:100dvh'></div><div id=x style='padding:0 .5vw;overflow:visible'></div>\
             <div id=y style='overflow:auto'></div>",
        );
        let flags = |name: &str| {
            let node = id(name);
            super::flags(
                &doc.nodes[node].tag,
                &styles.values[node],
                &styles.lengths[node],
                600.,
            )
        };
        assert_eq!(flags("w") & READS_VIEWPORT, READS_VIEWPORT);
        assert_eq!(flags("x") & READS_VIEWPORT, READS_VIEWPORT);
        assert_eq!(flags("a") & READS_VIEWPORT, 0);
        assert_eq!(flags("n") & READS_VIEWPORT, 0);
        let text = doc.nodes[id("v")]
            .parent
            .map_or(0, |list| doc.nodes[list].children[0]);
        assert_eq!(
            super::flags("#text", &styles.values[text], &styles.lengths[text], 600.) & PLAIN_BLOCK,
            0
        );
        for (name, expected) in [
            ("a", PLAIN_BLOCK),
            ("b", MOVES_CONTENT | UNCULLABLE | PLAIN_BLOCK),
            ("c", UNCULLABLE | OUT_OF_FLOW),
            ("d", OUT_OF_FLOW | HIDDEN),
            ("e", UNCULLABLE | PLAIN_BLOCK),
            ("f", UNCULLABLE | PLAIN_BLOCK),
            ("s", UNCULLABLE | PLAIN_BLOCK),
            ("t", PLAIN_BLOCK),
            ("g", UNCULLABLE),
            ("h", MOVES_CONTENT | UNCULLABLE | PLAIN_BLOCK),
            ("i", PLAIN_BLOCK),
            ("j", PLAIN_BLOCK),
            ("k", 0),
            ("l", 0),
            ("m", 0),
            ("n", PLAIN_BLOCK),
            ("o", 0),
            ("p", DEFINITE_SIZE | READS_HEIGHT),
            ("q", PLAIN_BLOCK | DEFINITE_SIZE),
            ("r", 0),
            ("u", PLAIN_BLOCK),
            ("v", PLAIN_BLOCK),
        ] {
            assert_eq!(
                flags(name) & !(ALWAYS_REBUILT | READS_LAST_FRAME),
                expected,
                "{name}"
            );
        }
        for (name, framed) in [
            ("a", false),
            ("b", false),
            ("c", false),
            ("d", false),
            ("f", false),
            ("g", false),
            ("k", false),
            ("l", true),
            ("m", false),
            ("v", true),
        ] {
            assert_eq!(flags(name) & READS_LAST_FRAME != 0, framed, "{name}");
        }
        assert_eq!(flags("y") & ALWAYS_REBUILT, ALWAYS_REBUILT);
        assert_eq!(flags("a") & ALWAYS_REBUILT, 0);
    }

    #[test]
    fn paint_only_changes_keep_the_geometry() {
        let style = |source: &str| -> style::Style {
            source
                .split(';')
                .filter_map(|pair| pair.split_once(':'))
                .map(|(name, value)| (Prop::new(name), value.into()))
                .collect()
        };
        let base = style("display:flex;color:red;padding-top:4px;width:10px");
        for (other, expected) in [
            ("display:flex;color:blue;padding-top:4px;width:10px", true),
            (
                "display:flex;color:red;padding-top:4px;width:10px;background-color:#ffd;--x:1",
                true,
            ),
            (
                "display:flex;padding-top:4px;width:10px;border-top-color:red;opacity:0.5",
                true,
            ),
            (
                "display:flex;color:red;padding-top:4px;width:10px;transform:scale(2)",
                true,
            ),
            ("display:flex;color:red;padding-top:5px;width:10px", false),
            ("display:flex;color:red;padding-top:4px", false),
            (
                "display:flex;color:red;padding-top:4px;width:10px;border-top-width:1px",
                false,
            ),
            ("display:block;color:red;padding-top:4px;width:10px", false),
        ] {
            assert_eq!(cosmetic(&base, &style(other)), expected, "{other}");
            assert_eq!(cosmetic(&style(other), &base), expected, "{other}");
        }
        let (doc, ..) = page(
            "<p id=a class=x data-v=1 hidden></p><p id=b class=y data-v=2 hidden></p><p class=x></p><img src=a><img src=b>",
        );
        let shape = |index: usize| {
            content(
                doc.nodes
                    .iter()
                    .filter(|node| !node.attrs.iter().next().is_none())
                    .nth(index)
                    .unwrap(),
            )
            .0
        };
        assert_eq!(shape(0), shape(1));
        assert_ne!(shape(0), shape(2));
        assert_ne!(shape(3), shape(4));
    }

    #[test]
    fn content_hash_tells_what_can_resize_a_box() {
        let (mut doc, ..) = page("<p class=x data-v=1 id=a>text</p>");
        let id = doc.nodes.iter().position(|node| node.tag == "p").unwrap();
        let start = content(&doc.nodes[id]);
        doc.nodes[id].attrs.set("class", "y");
        doc.nodes[id].attrs.set("data-v", "2");
        let cosmetic = content(&doc.nodes[id]);
        assert_eq!(cosmetic.0, start.0);
        assert_ne!(cosmetic.1, start.1);
        doc.nodes[id].attrs.set("colspan", "2");
        assert_ne!(content(&doc.nodes[id]).0, start.0);
        doc.nodes[id].attrs.remove("colspan");
        assert_eq!(content(&doc.nodes[id]), cosmetic);
        doc.nodes[id].value = Some("v".into());
        assert_ne!(content(&doc.nodes[id]).0, start.0);
    }

    #[test]
    fn members_are_unchanged_solid_boxes_out_of_view() {
        let zone = Zone {
            top: 0.,
            bottom: 600.,
            clips: Vec::new(),
            resized: false,
        };
        let info = Info {
            revision: 7,
            flags: PLAIN_BLOCK,
            ..Info::default()
        };
        let below = [0., 1300., 100., 20.];
        let member = |info: Info, placed: u64, bounds: [f32; 4], flex: bool| {
            super::member(
                info,
                (placed, [116., 20.], [0.; 4]),
                Some(&bounds),
                &zone,
                600.,
                flex,
            )
            .map(|(bounds, _)| bounds)
        };
        assert_eq!(member(info, 7, below, false), Some([0., 1300., 116., 20.]));
        let viewport = Info {
            revision: 7,
            flags: PLAIN_BLOCK | READS_VIEWPORT,
            ..Info::default()
        };
        assert!(member(viewport, 7, below, false).is_some());
        let resized = Zone {
            resized: true,
            ..Zone {
                top: 0.,
                bottom: 600.,
                clips: Vec::new(),
                resized: false,
            }
        };
        let bounds = Some(&below);
        assert!(
            super::member(
                viewport,
                (7, [116., 20.], [0.; 4]),
                bounds,
                &resized,
                600.,
                false
            )
            .is_none()
        );
        assert!(
            super::member(
                info,
                (7, [116., 20.], [0.; 4]),
                bounds,
                &resized,
                600.,
                false
            )
            .is_some()
        );
        assert_eq!(
            member(info, 7, [0., -621., 100., 20.], false),
            Some([0., -621., 116., 20.])
        );
        assert_eq!(member(info, 6, below, false), None);
        assert_eq!(member(info, 7, [0., 1100., 100., 20.], false), None);
        assert_eq!(member(info, 7, [0., -619., 100., 20.], false), None);
        assert_eq!(
            member(
                Info {
                    revision: 0,
                    flags: PLAIN_BLOCK,
                    ..Info::default()
                },
                0,
                below,
                false
            ),
            None
        );
        assert_eq!(
            member(
                Info {
                    flags: PLAIN_BLOCK | UNCULLABLE,
                    ..info
                },
                7,
                below,
                false
            ),
            None
        );
        assert_eq!(member(Info { flags: 0, ..info }, 7, below, false), None);
        assert_eq!(
            member(
                Info {
                    flags: PLAIN_BLOCK | DEFINITE_SIZE,
                    ..info
                },
                7,
                below,
                false
            ),
            Some([0., 1300., 116., 20.])
        );
        assert_eq!(
            member(
                Info {
                    flags: PLAIN_BLOCK | DEFINITE_SIZE,
                    ..info
                },
                7,
                below,
                true
            ),
            None
        );
        assert_eq!(
            super::member(info, (7, [116., 20.], [0.; 4]), None, &zone, 600., false),
            None
        );
        let margins = (7, [1., 30.], [4., 3., 6., 2.]);
        for flex in [false, true] {
            assert_eq!(
                super::member(info, margins, Some(&below), &zone, 600., flex),
                Some(([0., 1300., 1., 20.], [4., 6.]))
            );
        }
        let scrolled = Zone {
            top: 2000.,
            bottom: 2400.,
            clips: vec![3],
            resized: false,
        };
        assert_eq!(
            super::member(
                info,
                (7, [1., 20.], [0.; 4]),
                Some(&below),
                &scrolled,
                600.,
                false
            ),
            Some((below.map(|v| if v == 100. { 1. } else { v }), [0.; 2]))
        );
        assert_eq!(
            super::member(
                info,
                (7, [1., 20.], [0.; 4]),
                Some(&[0., 1500., 1., 20.]),
                &scrolled,
                600.,
                false
            ),
            None
        );
    }

    #[test]
    fn runs_join_touching_members_and_size_the_spacer() {
        let bounds: HashMap<usize, [f32; 4]> = [
            (1, [0., 1000., 300., 20.]),
            (2, [0., 1024., 310., 30.]),
            (4, [0., 1058., 300., 20.]),
            (5, [0., 2000., 300., 20.]),
            (6, [0., 2024., 300., 20.]),
            (8, [0., 2048., 300., 20.]),
        ]
        .into();
        let children = [1, 2, 3, 4, 5, 6, 7, 8];
        let (slots, runs) = plan(
            &children,
            |index| children[index] != 3,
            |child| {
                bounds
                    .get(&child)
                    .copied()
                    .filter(|_| child != 7)
                    .map(|bounds| (bounds, [0.; 2]))
            },
            4.,
        );
        assert_eq!(slots, [1, 1, 0, 1, 2, 2, 0, 3]);
        let run = |members: Vec<usize>, top: f32, height: f32, width: f32, margins: [f32; 2]| {
            Some(Run {
                members,
                top,
                height,
                width,
                margins,
                watch: Vec::new(),
            })
        };
        assert_eq!(
            runs,
            [
                run(vec![1, 2, 4], 1000., 78., 310., [0.; 2]),
                run(vec![5, 6], 2000., 44., 300., [0.; 2]),
                run(vec![8], 2048., 20., 300., [0.; 2]),
            ]
        );
        let spaced: HashMap<usize, ([f32; 4], [f32; 2])> = [
            (1, ([0., 1000., 300., 20.], [3., 5.])),
            (2, ([0., 1030., 300., 20.], [1., 2.])),
            (4, ([0., 1059., 300., 20.], [3., 5.])),
            (5, ([0., 1090., 300., 20.], [3., 5.])),
        ]
        .into();
        let (slots, runs) = plan(
            &[1, 2, 4, 5],
            |_| true,
            |child| spaced.get(&child).copied(),
            4.,
        );
        assert_eq!(slots, [1, 1, 1, 2]);
        assert_eq!(
            runs,
            [
                run(vec![1, 2, 4], 1000., 79., 300., [3., 5.]),
                run(vec![5], 1090., 20., 300., [3., 5.])
            ]
        );
        let (slots, runs) = plan(&children, |_| true, |_| None, 0.);
        assert_eq!((slots, runs), (vec![0; 8], Vec::new()));
    }

    #[test]
    fn wrapped_lines_are_culled_whole() {
        let boxes: HashMap<usize, (bool, [f32; 4])> = [
            (1, (true, [0., 0., 100., 20.])),
            (2, (true, [104., 0., 204., 18.])),
            (3, (true, [208., 2., 308., 20.])),
            (4, (true, [0., 24., 100., 44.])),
            (5, (true, [104., 24., 204., 44.])),
            (6, (true, [208., 24., 308., 44.])),
            (7, (true, [0., 48., 100., 60.])),
            (8, (false, [104., 48., 204., 60.])),
            (9, (true, [0., 64., 300., 84.])),
            (10, (true, [0., 88., 100., 100.])),
        ]
        .into();
        let children = [1, 2, 3, 4, 5, 6, 0, 7, 8, 9, 10, 11];
        let (slots, runs) = lines(
            &children,
            |index| children[index] != 0,
            |child| boxes.get(&child).copied(),
            4.,
        );
        assert_eq!(slots, [1, 1, 1, 1, 1, 1, 0, 0, 0, 2, 0, 0]);
        let tops: Vec<_> = runs
            .iter()
            .flatten()
            .map(|run| (run.members.clone(), run.top, run.height))
            .collect();
        assert_eq!(
            tops,
            [(vec![1, 2, 3, 4, 5, 6], 0., 44.), (vec![9], 64., 20.)]
        );
        let (slots, runs) = lines(
            &[1, 2],
            |_| true,
            |child| Some((true, [0., child as f32 * 30., 0., child as f32 * 30. + 20.])),
            0.,
        );
        assert_eq!((slots, runs.len()), (vec![0, 0], 0));
    }

    #[test]
    fn shifting_moves_culled_subtrees_and_their_shadow_trees() {
        let (doc, _, id) = page(
            "<div id=a><p id=b>text</p></div><div id=c></div><div id=h></div><div id=r><i id=i></i></div>",
        );
        let text = doc.nodes[id("b")].children[0];
        let shadows = rustc_hash::FxHashMap::from_iter([(id("h"), id("r"))]);
        let mut bounds = crate::dom::LayoutMirror::default();
        for node in [id("a"), id("b"), text, id("c"), id("h"), id("i")] {
            bounds.insert(node, [1., 2., 3., 4.]);
        }
        let mut subtrees = Subtrees::default();
        for _ in 0..2 {
            shift(
                &doc,
                &shadows,
                &mut subtrees,
                &mut bounds,
                &[id("a"), id("h"), 999],
                [10., -100.],
            );
        }
        for node in [id("a"), id("b"), text, id("h"), id("i")] {
            assert_eq!(bounds[&node], [21., -198., 3., 4.]);
        }
        assert_eq!(bounds[&id("c")], [1., 2., 3., 4.]);
        assert_eq!(subtrees.1[&id("a")].len(), 3);
    }
}
