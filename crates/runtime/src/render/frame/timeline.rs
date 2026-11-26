// The progress of scroll and view timelines, from the scroll offsets and the layout mirror.
use crate::css::properties::Prop;
use crate::css::value::timeline::{AXES, Scroll, Timeline, insets};
use crate::css::value::tokens::components;
use crate::dom::{Document, Geometry, Tree, geometry::scroll_offset};
use crate::render::element::query::{escape_step, scroller};
use crate::style::ComputedStyles;
use rustc_hash::FxHashMap;
use std::cell::RefCell;

// A timeline: its scroller, its axis, and for a view timeline its subject and insets.
type Found = (usize, u8, Option<(usize, [String; 2])>);

pub(crate) struct Timelines<'a> {
    doc: &'a Document,
    styles: &'a ComputedStyles,
    tree: &'a Tree,
    geometry: &'a Geometry,
    // The names that each `timeline-scope` box takes from the boxes in it, with the one box that declares each, or
    // None when several do.
    scopes: RefCell<FxHashMap<usize, FxHashMap<String, Option<usize>>>>,
}

impl<'a> Timelines<'a> {
    pub(crate) fn new(
        doc: &'a Document,
        styles: &'a ComputedStyles,
        tree: &'a Tree,
        geometry: &'a Geometry,
    ) -> Self {
        let scopes = RefCell::default();
        Self {
            doc,
            styles,
            tree,
            geometry,
            scopes,
        }
    }

    fn parent(&self, id: usize) -> Option<usize> {
        // A shadow root goes on to its host.
        (self.tree.flat_parent(self.doc, &self.styles.children, id))
            .or_else(|| self.tree.hosts.get(&id).copied())
    }

    // The scroll container around a box, in the flat tree and the chain of containing blocks.
    fn scroller(&self, id: usize) -> usize {
        let escape = escape_step(self.styles, id, 0).unwrap_or(0);
        let ancestors = std::iter::successors(self.parent(id), |&at| self.parent(at));
        scroller(self.doc, self.styles, ancestors, escape)
    }

    // The timeline that node `at` declares with `name`.
    fn declared(&self, at: usize, name: &str) -> Option<Found> {
        let style = self
            .styles
            .values
            .get(at)
            .filter(|_| !self.doc.nodes[at].tag.starts_with('#'))?;
        // A timeline of a box that is not rendered is inactive.
        let inactive = (usize::MAX, 0, None);
        let lists = [
            Prop::SCROLL_TIMELINE_NAME,
            Prop::SCROLL_TIMELINE_AXIS,
            Prop::VIEW_TIMELINE_NAME,
            Prop::VIEW_TIMELINE_AXIS,
            Prop::VIEW_TIMELINE_INSET,
        ]
        .map(|prop| components(style.get(prop).unwrap_or(""), ','));
        fn item<'a>(list: &[&'a str], at: usize) -> &'a str {
            list.get(at % list.len().max(1))
                .map_or("", |item| item.trim())
        }
        for names in [0, 2] {
            let Some(index) = lists[names].iter().position(|item| item.trim() == name) else {
                continue;
            };
            let axis = item(&lists[names + 1], index);
            let axis = AXES.iter().position(|word| *word == axis).unwrap_or(0) as u8;
            if style.get(Prop::DISPLAY) == Some("none") {
                return Some(inactive);
            }
            if names == 0 {
                return Some((at, axis, None));
            }
            let inset = insets(&components(item(&lists[4], index), ' '));
            let inset = inset.unwrap_or_else(|| ["auto".into(), "auto".into()]);
            return Some((self.scroller(at), axis, Some((at, inset))));
        }
        None
    }

    // The names that the boxes in `at` declare, and the one box that declares each.
    fn scope(&self, at: usize) -> FxHashMap<String, Option<usize>> {
        let mut names: FxHashMap<String, Option<usize>> = FxHashMap::default();
        let mut pending = self.doc.nodes[at].children.to_vec();
        while let Some(child) = pending.pop() {
            let style = self.styles.values.get(child);
            for prop in [Prop::SCROLL_TIMELINE_NAME, Prop::VIEW_TIMELINE_NAME] {
                for item in components(style.and_then(|style| style.get(prop)).unwrap_or(""), ',') {
                    if item.trim().starts_with("--") {
                        let one = names.entry(item.trim().into()).or_insert(Some(child));
                        *one = one.filter(|&one| one == child);
                    }
                }
            }
            pending.extend(self.doc.nodes[child].children.iter().copied());
        }
        names
    }

    fn resolve(&self, id: usize, timeline: &Timeline) -> Option<Found> {
        let name = match timeline {
            &Timeline::Scroll(scroller, axis) => {
                let scroller = match scroller {
                    0 => self.scroller(id),
                    1 => 0,
                    _ => id,
                };
                return Some((scroller, axis, None));
            }
            Timeline::View(axis, inset) => {
                return Some((self.scroller(id), *axis, Some((id, inset.clone()))));
            }
            Timeline::Named(name) => name,
            _ => return None,
        };
        // The nearest node on the path that declares the name, or that takes it into its scope from the one node
        // in it that declares it.
        let mut node = Some(id);
        while let Some(at) = node {
            if let Some(found) = self.declared(at, name) {
                return Some(found);
            }
            let scope =
                (self.styles.values.get(at)).and_then(|style| style.get(Prop::TIMELINE_SCOPE));
            if scope.is_some_and(|scope| {
                components(scope, ',')
                    .iter()
                    .any(|item| item.trim() == name)
            }) {
                let mut scopes = self.scopes.borrow_mut();
                let one = scopes
                    .entry(at)
                    .or_insert_with(|| self.scope(at))
                    .get(name)
                    .copied()
                    .flatten();
                drop(scopes);
                // A scope without one box that declares the name gives an inactive timeline.
                return Some(one.and_then(|one| self.declared(one, name)).unwrap_or((
                    usize::MAX,
                    0,
                    None,
                )));
            }
            node = self.parent(at);
        }
        None
    }

    /// The progress of a scroll-driven effect whose timeline is read from node `id`: 1/0 when it has no timeline, 1
    /// when the timeline is at its end, then the timeline, the start and the end of the range in percent of the
    /// timeline. None while the timeline is inactive.
    pub(crate) fn progress(&self, id: usize, scroll: &Scroll) -> Option<[f64; 5]> {
        let (styles, geometry) = (self.styles, self.geometry);
        styles.values.get(id)?;
        let Some((scroller, axis, view)) = self.resolve(id, &scroll.timeline) else {
            return Some([f64::INFINITY, 0., f64::NAN, f64::NAN, f64::NAN]);
        };
        let handle = geometry.scroll.get(&scroller)?;
        let vertical =
            (styles.values.get(scroller)).and_then(|style| style.get(Prop::WRITING_MODE));
        let x = match axis {
            2 => true,
            3 => false,
            axis => (axis == 1) != vertical.is_some_and(|mode| !mode.starts_with("horizontal")),
        };
        // The handle of a box that lays out turned has the axes of its turned frame.
        let turned = geometry.turned.contains(&scroller);
        let inner = x != turned;
        let pick = |point: renderer::Point<renderer::Pixels>| {
            f64::from(if inner { point.x } else { point.y })
        };
        let [low, high] = handle.range().map(pick);
        let max = high - low;
        // Offsets count from 0 at the start of the scroll, down for boxes that scroll from their start.
        let at = pick(scroll_offset(handle, handle.offset())).abs();
        if max <= 0. {
            return None;
        }
        let size = |rect: [f32; 4]| f64::from(rect[2 + usize::from(!x)]);
        // The cover range, and the edges where the subject has entered and where it starts to leave.
        let [cover, entered, leaving] = match view {
            None => [[0., max]; 3],
            Some((subject, inset)) => {
                let bounds = handle.bounds();
                let port = match turned {
                    true => *geometry.mirror.get(&scroller)?,
                    false => [
                        bounds.origin.x,
                        bounds.origin.y,
                        bounds.size.width,
                        bounds.size.height,
                    ]
                    .map(f32::from),
                };
                let rect = *geometry.mirror.get(&subject)?;
                let start = |rect: [f32; 4]| f64::from(rect[usize::from(!x)]);
                let reverse = match turned {
                    true => x && vertical == Some("vertical-rl"),
                    false => high > 0.,
                };
                let room = size(port);
                // Where the subject starts in the scrolled content.
                let from = match reverse {
                    true => start(port) + room - start(rect) - size(rect),
                    false => start(rect) - start(port),
                } + pick(geometry.mirror_offset(scroller)).abs();
                let sides = match (x, reverse) {
                    (false, _) => [Prop::SCROLL_PADDING_TOP, Prop::SCROLL_PADDING_BOTTOM],
                    (true, false) => [Prop::SCROLL_PADDING_LEFT, Prop::SCROLL_PADDING_RIGHT],
                    (true, true) => [Prop::SCROLL_PADDING_RIGHT, Prop::SCROLL_PADDING_LEFT],
                };
                let [near, far] = [0, 1].map(|side| {
                    let word = match inset[side].as_str() {
                        "auto" => {
                            (styles.values.get(scroller)).and_then(|style| style.get(sides[side]))
                        }
                        word => Some(word),
                    };
                    let word = word.unwrap_or("0px");
                    f64::from(
                        styles.lengths[scroller]
                            .resolve(word, room as f32)
                            .unwrap_or(0.),
                    )
                });
                let length = size(rect);
                let [enter, leave] = [from + length - room + far, from - near];
                [
                    [from - room + far, from + length - near],
                    [enter.min(leave), enter.max(leave)],
                    [enter, leave],
                ]
            }
        };
        let [from, to] = [0, 1].map(|end| {
            let edge = &scroll.range[end];
            let [low, high] = match edge.0 {
                2 => entered,
                3 => [cover[0], entered[0]],
                4 => [entered[1], cover[1]],
                5 => [cover[0], leaving[0]],
                6 => [leaving[1], cover[1]],
                _ => cover,
            };
            let offset = match edge.1.as_str() {
                "" => ["0%", "100%"][end],
                offset => offset,
            };
            low + f64::from(
                styles.lengths[id]
                    .resolve(offset, (high - low) as f32)
                    .unwrap_or(0.),
            )
        });
        let percent = |at: f64| (at - cover[0]) / (cover[1] - cover[0]) * 100.;
        let end = f64::from(u8::from(at >= max - 1e-3));
        Some([0., end, percent(at), percent(from), percent(to)])
    }

    /// What scripts read of a timeline at node `id`: the progress as `progress` gives it or nulls, then its scroller,
    /// its subject or -1, and its axis.
    pub(crate) fn read(&self, id: usize, scroll: &Scroll) -> serde_json::Value {
        let mut out: Vec<serde_json::Value> = match self.progress(id, scroll) {
            Some(values) => values.iter().map(|&value| value.into()).collect(),
            None => vec![serde_json::Value::Null; 5],
        };
        if let Some((scroller, axis, view)) = self
            .resolve(id, &scroll.timeline)
            .filter(|found| found.0 != usize::MAX)
        {
            let subject = view.map_or(-1, |view| view.0 as i64);
            out.extend([
                scroller.into(),
                subject.into(),
                AXES[usize::from(axis)].into(),
            ]);
        }
        out.into()
    }
}
