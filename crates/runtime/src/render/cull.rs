use crate::style::Prop;
use crate::{dom::Document, style};
use renderer::{AnyElement, prelude::*, px};
use std::{collections::HashMap, rc::Rc};

/// The subtree paints outside its box or keeps native state, so it is never replaced by a spacer.
pub(crate) const BLOCKED: u16 = 1;
/// The box's own style, tag, text or attributes changed this frame in a way that can resize boxes.
pub(crate) const CHANGED: u16 = 2;
/// The box moves its content at paint time, so nothing below it is culled.
pub(crate) const MOVED: u16 = 4;
/// The box is not in the flow: not displayed, or absolutely positioned.
pub(crate) const OUT: u16 = 8;
pub(crate) const HIDDEN: u16 = 16;
/// A block box in the flow without auto vertical margins or baseline alignment, whose height
/// does not depend on its parent's.
pub(crate) const SOLID: u16 = 32;
/// The box has a definite height or flex basis, so a column flex container could shrink it.
pub(crate) const SIZED: u16 = 64;
/// The box's geometry changed this frame, so its cached measurements are dropped.
pub(crate) const STALE: u16 = 128;
/// The box's children in the flow are not the ones of the last frame.
pub(crate) const RELISTED: u16 = 256;
/// The subtree sizes something in viewport units, so a frame that resizes the viewport lays it out.
pub(crate) const VIEWPORT: u16 = 512;

#[derive(Clone, Copy, Default)]
pub(crate) struct Info {
    /// The geometry revision of the subtree.
    pub(crate) revision: u64,
    pub(crate) flags: u16,
}

#[derive(Default)]
pub(crate) struct Cull {
    /// This frame's subtree revisions and flags, by node.
    pub(crate) info: Vec<Info>,
    /// By node, the subtree revision when its bounds were recorded, its size with margins (the
    /// unsnapped cell size for a grid item), and its margins.
    pub(crate) placed: Vec<(u64, [f32; 2], [f32; 4])>,
    /// The content box width of each box at its last layout.
    pub(crate) widths: rustc_hash::FxHashMap<usize, f32>,
    /// Set when a parent of a spacer changed its width, so the next frame lays everything out.
    pub(crate) off: bool,
    /// Whether this frame culls, kept for the container query pass of the same frame.
    pub(crate) enabled: bool,
    /// The viewport size, scale and font loads of the last frame.
    pub(crate) frame: [f32; 4],
    /// Whether this frame has another viewport size than the last one.
    pub(crate) resized: bool,
    /// The scroll offsets of the last frame, by scroll container, `0` for the page.
    pub(crate) offsets: HashMap<usize, renderer::Point<renderer::Pixels>>,
    /// The nodes that are not hidden, built with `info`.
    pub(crate) visible: Vec<bool>,
    /// The newest node revision, focus and styles that `info` was built from, without animated paint.
    pub(crate) seen: Option<(u64, Option<usize>, Rc<crate::style::ComputedStyles>)>,
}

/// The visible vertical range of a subtree, from the last frame's bounds of its clipping ancestors.
pub(super) struct Zone {
    pub(super) top: f32,
    pub(super) bottom: f32,
    pub(super) clips: Vec<usize>,
    /// The viewport changed size since the last frame.
    pub(super) resized: bool,
}

impl Zone {
    /// The visible vertical range in the window, from this frame's bounds of the clipping ancestors.
    fn visible(&self, bounds: &crate::dom::LayoutMirror, window: &renderer::Window) -> [f32; 2] {
        self.clips.iter().filter_map(|clip| bounds.get(clip)).fold(
            [0., f32::from(window.viewport_size().height)],
            |[low, high], rect| [low.max(rect[1]), high.min(rect[1] + rect[3])],
        )
    }
}

/// Consecutive culled children, replaced by one spacer.
#[derive(Debug, PartialEq)]
pub(super) struct Run {
    pub(super) members: Vec<usize>,
    pub(super) top: f32,
    pub(super) height: f32,
    pub(super) width: f32,
    /// The top margin of the first member and the bottom margin of the last.
    pub(super) margins: [f32; 2],
    /// Changed boxes before the run in a wrapping container, with their width with margins at
    /// the last layout. A new width may move boxes to other lines.
    pub(super) watch: Vec<(usize, f32)>,
}

/// Whether the styles differ only in properties that do not move or resize boxes.
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

/// Whether the attributes differ only in ones that act through the style or not at all.
pub(super) fn cosmetic_attrs(old: &crate::dom::Attrs, new: &crate::dom::Attrs) -> bool {
    old.iter().chain(new.iter()).all(|(name, _)| {
        matches!(name.as_str(), "class" | "style" | "id" | "title")
            || name.starts_with("data-")
            || name.starts_with("aria-")
            || old.get(name) == new.get(name)
    })
}

/// The flags of a box's own style.
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
    [
        (moved, MOVED),
        (blocked, BLOCKED),
        (
            display == "none" || matches!(position, "absolute" | "fixed"),
            OUT,
        ),
        (display == "none", HIDDEN),
        (solid, SOLID),
        (!auto(Prop::HEIGHT) || !auto(Prop::FLEX_BASIS), SIZED),
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
            VIEWPORT,
        ),
    ]
    .into_iter()
    .fold(0, |flags, (on, bit)| if on { flags | bit } else { flags })
}

/// The bounds of a child that can be culled, with its width including horizontal margins,
/// and its vertical margins.
pub(super) fn member(
    info: Info,
    (placed, [width, _], margins): (u64, [f32; 2], [f32; 4]),
    bounds: Option<&[f32; 4]>,
    zone: &Zone,
    margin: f32,
    flex: bool,
) -> Option<([f32; 4], [f32; 2])> {
    let fits = info.flags & (BLOCKED | SOLID) == SOLID
        && (!flex || info.flags & SIZED == 0)
        && (!zone.resized || info.flags & VIEWPORT == 0);
    if !fits || info.revision == 0 || placed != info.revision {
        return None;
    }
    let [x, y, _, height] = *bounds?;
    (y + height < zone.top - margin || y > zone.bottom + margin)
        .then_some(([x, y, width, height], [margins[0], margins[2]]))
}

/// Groups the children that `member` accepts into runs of boxes that touch through their
/// margins and the gap. Children that `shown` rejects take no space; any other child ends a
/// run. Returns the run of each child.
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

/// Groups whole lines of a wrapping row flex container into runs of lines that touch through
/// the gap. `item` gives a child's margin box at the last layout and whether `member` accepts
/// it; a line with any other child ends a run, and a child without a box ends the plan.
/// Returns the run of each child.
pub(super) fn lines(
    children: &[usize],
    shown: impl Fn(usize) -> bool,
    mut item: impl FnMut(usize) -> Option<(bool, [f32; 4])>,
    gap: f32,
) -> (Vec<usize>, Vec<Option<Run>>) {
    // Each line: the indexes of its children, whether all are members, and its top and bottom.
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

/// Moves the recorded bounds of culled subtrees with their spacer.
pub(super) fn shift(
    doc: &Document,
    shadows: &rustc_hash::FxHashMap<usize, usize>,
    bounds: &mut crate::dom::LayoutMirror,
    roots: &[usize],
    [x, y]: [f32; 2],
) {
    let mut pending = roots.to_vec();
    while let Some(id) = pending.pop() {
        if let Some(rect) = bounds.get_mut(&id) {
            rect[0] += x;
            rect[1] += y;
        }
        pending.extend(
            doc.nodes
                .get(id)
                .into_iter()
                .flat_map(|node| &node.children),
        );
        pending.extend(shadows.get(&id));
    }
}

/// A fixed size box in place of a run. After layout it moves the recorded bounds of the run
/// with it, and draws the frame again when the run came into view or the parent changed its
/// width, so the wrong frame is never shown.
pub(super) fn spacer(
    run: Run,
    parent: usize,
    [left, width]: [f32; 2],
    zone: Rc<Zone>,
    view: Rc<super::ViewData>,
) -> AnyElement {
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
                    .is_none_or(|now| (now - width).abs() > 0.01);
            // WARNING: a changed box before a run of wrapped lines may move boxes to other lines, which the spacer cannot follow.
            let reflowed = watch.iter().any(|&(id, width)| {
                dom.cull
                    .placed
                    .get(id)
                    .is_none_or(|placed| (placed.1[0] - width).abs() > 0.01)
            });
            if resized || reflowed || y < high && y + height > low {
                dom.cull.off |= resized || reflowed;
                window.redraw();
            }
            let delta = [x.unwrap_or(left) - left, y - top];
            if delta != [0., 0.]
                && let Some(page) = host.pages.get(&view.id)
            {
                shift(&page.doc, &dom.shadows, &mut dom.bounds, &members, delta);
            }
        },
        |_, _, _| {},
    )
    .w(px(size))
    .h(px(height))
    .mt(px(margins[0]))
    .mb(px(margins[1]))
    .flex_shrink_0()
    .into_any_element()
}

/// A zero size box at the padding box origin of a grid, in place of its culled items, which
/// come with their bounds from the origin. After layout it moves their recorded bounds, and
/// draws the frame again when one came into view.
pub(super) fn anchor(
    items: Vec<(usize, [f32; 4])>,
    zone: Rc<Zone>,
    view: Rc<super::ViewData>,
) -> AnyElement {
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
    .into_any_element()
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
             <div id=w style='min-height:100dvh'></div><div id=x style='padding:0 .5vw;overflow:visible'></div>",
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
        assert_eq!(flags("w") & VIEWPORT, VIEWPORT);
        assert_eq!(flags("x") & VIEWPORT, VIEWPORT);
        assert_eq!(flags("a") & VIEWPORT, 0);
        assert_eq!(flags("n") & VIEWPORT, 0);
        let text = doc.nodes[id("v")]
            .parent
            .map_or(0, |list| doc.nodes[list].children[0]);
        assert_eq!(
            super::flags("#text", &styles.values[text], &styles.lengths[text], 600.) & SOLID,
            0
        );
        for (name, expected) in [
            ("a", SOLID),
            ("b", MOVED | BLOCKED | SOLID),
            ("c", BLOCKED | OUT),
            ("d", OUT | HIDDEN),
            ("e", BLOCKED | SOLID),
            ("f", BLOCKED | SOLID),
            ("s", BLOCKED | SOLID),
            ("t", SOLID),
            ("g", BLOCKED),
            ("h", MOVED | BLOCKED | SOLID),
            ("i", SOLID),
            ("j", SOLID),
            ("k", 0),
            ("l", 0),
            ("m", 0),
            ("n", SOLID),
            ("o", 0),
            ("p", SIZED),
            ("q", SOLID | SIZED),
            ("r", 0),
            ("u", SOLID),
            ("v", SOLID),
        ] {
            assert_eq!(flags(name), expected, "{name}");
        }
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
        let attrs = |index: usize| {
            &doc.nodes
                .iter()
                .filter(|node| !node.attrs.iter().next().is_none())
                .nth(index)
                .unwrap()
                .attrs
        };
        assert!(cosmetic_attrs(attrs(0), attrs(1)));
        assert!(!cosmetic_attrs(attrs(0), attrs(2)));
        assert!(!cosmetic_attrs(attrs(3), attrs(4)));
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
            flags: SOLID,
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
            flags: SOLID | VIEWPORT,
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
                    flags: SOLID
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
                    flags: SOLID | BLOCKED,
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
                    flags: SOLID | SIZED,
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
                    flags: SOLID | SIZED,
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
        // Lines of margin boxes: 1 2 3 | 4 5 6 | 7 8 | 9 | 10, where 8 changed and 11 is new.
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
        shift(
            &doc,
            &shadows,
            &mut bounds,
            &[id("a"), id("h"), 999],
            [10., -100.],
        );
        for node in [id("a"), id("b"), text, id("h"), id("i")] {
            assert_eq!(bounds[&node], [11., -98., 3., 4.]);
        }
        assert_eq!(bounds[&id("c")], [1., 2., 3., 4.]);
    }
}
