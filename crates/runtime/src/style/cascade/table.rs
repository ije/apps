use crate::css::{BORDERS, Prop, Style};
use crate::dom::Document;
use crate::style::cascade::TreeWalk;
use crate::style::cascade::memo::StyleRef;
use crate::style::cascade::used::border_spacing;
use crate::style::computed::{
    CAPTION, CELL, ComputedStyles, DISPLAY_CONTENTS, DISPLAY_NONE, HEADER, ROW,
};
use crate::style::{Chunked, Shadows, StyleCache};
use rustc_hash::FxHashMap;
use std::rc::Rc;
pub(crate) type CellKey = (StyleRef, [(u32, Option<[Option<Rc<str>>; 2]>); 4]);

#[derive(PartialEq)]
pub(crate) struct Line {
    pub(crate) row: (usize, StyleRef),
    pub(crate) group: Option<(usize, StyleRef)>,
    pub(crate) cells: Vec<(usize, StyleRef, usize)>,
}

#[derive(Default)]
pub(crate) struct CollapsedTable {
    pub(crate) pass: u64,
    pub(crate) size: [f32; 2],
    pub(crate) table: Option<(StyleRef, [u32; 4], Rc<Style>)>,
    pub(crate) lines: Vec<Line>,
    pub(crate) adjusted: Vec<(Vec<Rc<Style>>, [f32; 4])>,
    pub(crate) styles: FxHashMap<CellKey, Rc<Style>>,
}

impl StyleCache {
    // Collapses the borders of tables. This adjusts styles after caching, so cached styles stay unadjusted.
    pub(crate) fn collapse_tables(
        &mut self,
        doc: &Document,
        walk: &TreeWalk,
        out: &mut ComputedStyles,
        [width, height]: [f32; 2],
        previous: Option<&ComputedStyles>,
        fast: bool,
    ) {
        let ComputedStyles {
            values: styles,
            lengths,
            authored,
            ..
        } = out;
        let _count = doc.nodes.len();
        // This adjusts styles after caching, so cached styles stay unadjusted.
        const SIDES: [[Prop; 4]; 4] = [
            [
                Prop::BORDER_TOP_STYLE,
                Prop::BORDER_TOP_WIDTH,
                Prop::MARGIN_TOP,
                Prop::PADDING_TOP,
            ],
            [
                Prop::BORDER_RIGHT_STYLE,
                Prop::BORDER_RIGHT_WIDTH,
                Prop::MARGIN_RIGHT,
                Prop::PADDING_RIGHT,
            ],
            [
                Prop::BORDER_BOTTOM_STYLE,
                Prop::BORDER_BOTTOM_WIDTH,
                Prop::MARGIN_BOTTOM,
                Prop::PADDING_BOTTOM,
            ],
            [
                Prop::BORDER_LEFT_STYLE,
                Prop::BORDER_LEFT_WIDTH,
                Prop::MARGIN_LEFT,
                Prop::PADDING_LEFT,
            ],
        ];
        let mut edges = FxHashMap::<*const Style, (Rc<Style>, [(f32, u8); 4])>::default();
        let own_style = |styles: &Chunked<Rc<Style>>, id: usize| {
            let style = &styles[id];
            previous
                .filter(|_| fast)
                .filter(|previous| {
                    previous
                        .values
                        .get(id)
                        .is_some_and(|old| Rc::ptr_eq(old, style))
                })
                .and_then(|previous| previous.authored.get(&id))
                .unwrap_or(style)
                .clone()
        };
        for &table in &walk.tables {
            let table_style = own_style(styles, table);
            if table_style
                .get(Prop::BORDER_COLLAPSE)
                .is_none_or(|value| value != "collapse")
                || table_style
                    .get(Prop::DISPLAY)
                    .is_none_or(|value| value != "grid")
            {
                continue;
            }
            let lines: Vec<Line> = doc.nodes[table]
                .children
                .iter()
                .flat_map(|child| {
                    let group =
                        matches!(doc.nodes[*child].tag.as_str(), "thead" | "tbody" | "tfoot");
                    let rows = if group {
                        &doc.nodes[*child].children[..]
                    } else {
                        std::slice::from_ref(child)
                    };
                    rows.iter().map(move |&row| (row, group.then_some(*child)))
                })
                .filter(|&(row, _)| doc.nodes[row].tag == "tr")
                .map(|(row, group)| Line {
                    row: (row, StyleRef(own_style(styles, row))),
                    group: group.map(|group| (group, StyleRef(own_style(styles, group)))),
                    cells: doc.nodes[row]
                        .children
                        .iter()
                        .filter(|&&cell| matches!(doc.nodes[cell].tag.as_str(), "td" | "th"))
                        .map(|&cell| {
                            let span = doc.nodes[cell]
                                .attrs
                                .get("colspan")
                                .and_then(|value| value.parse::<usize>().ok())
                                .unwrap_or(1)
                                .clamp(1, 1000);
                            (cell, StyleRef(own_style(styles, cell)), span)
                        })
                        .collect(),
                })
                .filter(|line| !line.cells.is_empty())
                .collect();
            type Edge = (f32, u8, usize, usize);
            let wins = |a: Edge, b: Edge| if (b.0, b.1) > (a.0, a.1) { b } else { a };
            let mut edge = |style: &Rc<Style>, id: usize, side: usize| -> Edge {
                let sides = match edges.get(&Rc::as_ptr(style)) {
                    Some((_, sides)) => *sides,
                    None => {
                        let sides = SIDES.map(|[kind, width, ..]| {
                            match style.get(kind).unwrap_or("none") {
                                "none" | "hidden" => (0., 0),
                                kind => (
                                    style
                                        .get(width)
                                        .map_or(Some(3.), |value| lengths[id].resolve(value, 0.))
                                        .unwrap_or(0.)
                                        .max(0.),
                                    match kind {
                                        "double" => 8,
                                        "solid" => 7,
                                        "dashed" => 6,
                                        "dotted" => 5,
                                        "ridge" => 4,
                                        "outset" => 3,
                                        "groove" => 2,
                                        "inset" => 1,
                                        _ => 0,
                                    },
                                ),
                            }
                        });
                        edges.insert(Rc::as_ptr(style), (style.clone(), sides));
                        sides
                    }
                };
                (sides[side].0, sides[side].1, id, side)
            };
            let frame = [0, 1, 2, 3].map(|side| edge(&table_style, table, side));
            let mut own = |index: usize| -> Vec<[Edge; 4]> {
                let line = &lines[index];
                let group = line.group.as_ref().map(|group| group.0);
                let first =
                    index == 0 || lines[index - 1].group.as_ref().map(|group| group.0) != group;
                let last = lines
                    .get(index + 1)
                    .is_none_or(|next| next.group.as_ref().map(|group| group.0) != group);
                line.cells
                    .iter()
                    .enumerate()
                    .map(|(column, (cell, style, _))| {
                        let mut sides = [0, 1, 2, 3].map(|side| edge(&style.0, *cell, side));
                        for (position, (owner, style)) in [Some(&line.row), line.group.as_ref()]
                            .into_iter()
                            .flatten()
                            .enumerate()
                        {
                            let whole = position == 0;
                            for (side, applies) in [
                                (0, whole || first),
                                (1, column + 1 == line.cells.len()),
                                (2, whole || last),
                                (3, column == 0),
                            ] {
                                if applies {
                                    sides[side] = wins(sides[side], edge(&style.0, *owner, side));
                                }
                            }
                        }
                        sides
                    })
                    .collect()
            };
            let share = |above: &mut [[Edge; 4]], a: &Line, below: &mut [[Edge; 4]], b: &Line| {
                let ends = |line: &Line| {
                    line.cells
                        .iter()
                        .scan(0, |end, cell| {
                            *end += cell.2;
                            Some(*end)
                        })
                        .collect::<Vec<_>>()
                };
                let (top, bottom) = (ends(a), ends(b));
                let (mut i, mut j) = (0, 0);
                while i < above.len() && j < below.len() {
                    if top[i] - a.cells[i].2 < bottom[j] && bottom[j] - b.cells[j].2 < top[i] {
                        let shared = wins(above[i][2], below[j][0]);
                        (above[i][2], below[j][0]) = (shared, shared);
                    }
                    if top[i] <= bottom[j] {
                        i += 1;
                    } else {
                        j += 1;
                    }
                }
            };
            let mut old = self.tables.remove(&table).unwrap_or_default();
            let whole = old.size != [width, height]
                || old
                    .table
                    .as_ref()
                    .is_none_or(|(style, ..)| !Rc::ptr_eq(&style.0, &table_style));
            let mut moved: Option<FxHashMap<usize, usize>> = None;
            let mut shared = std::mem::take(&mut old.styles);
            let mut adjusted: Vec<(Vec<Rc<Style>>, [f32; 4])> = Vec::with_capacity(lines.len());
            let mut widest = [0f32; 4];
            for index in 0..lines.len() {
                let row = lines[index].row.0;
                let before = match old.lines.get(index) {
                    Some(line) if line.row.0 == row => Some(index),
                    _ => moved
                        .get_or_insert_with(|| {
                            old.lines
                                .iter()
                                .enumerate()
                                .map(|(index, line)| (line.row.0, index))
                                .collect()
                        })
                        .get(&row)
                        .copied(),
                };
                let same = before.filter(|&before| {
                    !whole
                        && (0..3).all(|step| {
                            lines.get((index + step).wrapping_sub(1))
                                == old.lines.get((before + step).wrapping_sub(1))
                        })
                });
                let line = match same {
                    Some(before) => std::mem::take(&mut old.adjusted[before]),
                    None => {
                        let mut cells = own(index);
                        if index > 0 {
                            share(
                                &mut own(index - 1),
                                &lines[index - 1],
                                &mut cells,
                                &lines[index],
                            );
                        }
                        if index + 1 < lines.len() {
                            share(
                                &mut cells,
                                &lines[index],
                                &mut own(index + 1),
                                &lines[index + 1],
                            );
                        }
                        for column in 1..cells.len() {
                            let shared = wins(cells[column - 1][1], cells[column][3]);
                            (cells[column - 1][1], cells[column][3]) = (shared, shared);
                        }
                        let mut outer = [0f32; 4];
                        let count = cells.len();
                        for (column, sides) in cells.iter_mut().enumerate() {
                            for (side, outer) in outer.iter_mut().enumerate() {
                                let at = match side {
                                    0 => index == 0,
                                    1 => column + 1 == count,
                                    2 => index + 1 == lines.len(),
                                    _ => column == 0,
                                };
                                if at {
                                    sides[side] = wins(sides[side], frame[side]);
                                    *outer = outer.max(sides[side].0);
                                }
                            }
                        }
                        let cells = lines[index]
                            .cells
                            .iter()
                            .zip(cells)
                            .map(|((cell, own, _), sides)| {
                                let side = |side: usize| {
                                    let (width, _, owner, from) = sides[side];
                                    let [_, kind, color] = BORDERS[from];
                                    (
                                        width,
                                        (owner != *cell || from != side).then(|| {
                                            let owner = own_style(styles, owner);
                                            [
                                                if width > 0. {
                                                    owner.value(kind).cloned()
                                                } else {
                                                    Some("none".into())
                                                },
                                                owner.value(color).cloned(),
                                            ]
                                        }),
                                    )
                                };
                                let sides = [side(0), side(1), side(2), side(3)];
                                let key = (
                                    own.clone(),
                                    sides.clone().map(|(width, taken)| (width.to_bits(), taken)),
                                );
                                shared
                                    .entry(key)
                                    .or_insert_with(|| {
                                        let mut style = (*own.0).clone();
                                        for (side, ([_, _, margin, _], (width, taken))) in
                                            SIDES.into_iter().zip(sides).enumerate()
                                        {
                                            if let Some([kind, color]) = taken {
                                                for (prop, value) in
                                                    BORDERS[side].into_iter().zip([
                                                        Some(format!("{width}px").into()),
                                                        kind,
                                                        color,
                                                    ])
                                                {
                                                    if let Some(value) = value {
                                                        style.insert(prop, value);
                                                    }
                                                }
                                            }
                                            style.insert(margin, format!("{}px", -width / 2.));
                                        }
                                        Rc::new(style)
                                    })
                                    .clone()
                            })
                            .collect();
                        (cells, outer)
                    }
                };
                for (widest, outer) in widest.iter_mut().zip(line.1) {
                    *widest = widest.max(outer);
                }
                adjusted.push(line);
            }
            let count: usize = lines.iter().map(|line| line.cells.len()).sum();
            if shared.len() > 2 * count + 64 {
                shared.retain(|_, style| Rc::strong_count(style) > 1);
            }
            let key = (StyleRef(table_style.clone()), widest.map(f32::to_bits));
            let style = match old.table.take() {
                Some((style, bits, adjusted)) if (style.clone(), bits) == key => adjusted,
                _ => {
                    let mut style = (*table_style).clone();
                    for ([_, width, _, padding], widest) in SIDES.into_iter().zip(widest) {
                        style.insert(width, "0");
                        style.insert(padding, format!("{}px", widest / 2.));
                    }
                    Rc::new(style)
                }
            };
            // Writes only where the style changed, as each write copies a shared chunk of the styles.
            authored.reserve(adjusted.iter().map(|line| line.0.len()).sum::<usize>() + 1);
            for (id, own, style) in lines
                .iter()
                .zip(&adjusted)
                .flat_map(|(line, (cells, _))| {
                    line.cells
                        .iter()
                        .zip(cells)
                        .map(|((cell, own, _), style)| (*cell, &own.0, style))
                })
                .chain(std::iter::once((table, &table_style, &style)))
            {
                if !Rc::ptr_eq(&styles[id], style) {
                    styles[id] = style.clone();
                }
                authored.insert(id, own.clone());
            }
            self.tables.insert(
                table,
                CollapsedTable {
                    pass: self.pass,
                    size: [width, height],
                    table: Some((key.0, key.1, style)),
                    lines,
                    adjusted,
                    styles: shared,
                },
            );
        }
        for (&table, old) in self.tables.iter().filter(|(_, old)| old.pass != self.pass) {
            for id in old
                .lines
                .iter()
                .flat_map(|line| line.cells.iter().map(|cell| cell.0))
                .chain([table])
            {
                let own = own_style(styles, id);
                if !Rc::ptr_eq(&styles[id], &own) {
                    styles[id] = own;
                }
            }
        }
    }
}

// Gives blocks whose children are all table parts outside a table the anonymous table around those parts.
#[allow(clippy::too_many_arguments)]
pub(crate) fn anonymous_tables(
    doc: &Document,
    walk: &TreeWalk,
    shadows: &Shadows,
    out: &mut ComputedStyles,
    previous: Option<&ComputedStyles>,
    fast: bool,
    orphans: &mut Vec<usize>,
) {
    let ComputedStyles {
        values: styles,
        flags: style_flags,
        authored,
        anonymous,
        ..
    } = out;
    let count = doc.nodes.len();
    // A block whose children are all table parts outside a table lays out as their anonymous table, and its rows
    // and row groups pass their cells on to it. Blocks with other content keep their parts as they are.
    orphans.sort_unstable();
    orphans.dedup();
    orphans.retain(|&host| {
        let part = |child: usize| matches!(style_flags[child] >> 8, CELL | CAPTION | 10..=12);
        if host >= count
            || walk.walk.place(host).is_none()
            || !doc.nodes[host].children.iter().any(|&child| part(child))
        {
            return false;
        }
        let role = match styles[host].get(Prop::DISPLAY) {
            Some("block") => 13,
            Some("inline-block") => 14,
            _ => return true,
        };
        if style_flags[host] >> 8 != 0
            || shadows.roots.contains_key(&host)
            || !doc.nodes[host].children.iter().all(|&child| {
                let (node, style) = (&doc.nodes[child], &styles[child]);
                match node.tag.as_str() {
                    "#text" => node.text.trim_ascii().is_empty(),
                    "#comment" => true,
                    _ => {
                        part(child)
                            || style_flags[child] & DISPLAY_NONE != 0
                            || style
                                .get(Prop::DISPLAY)
                                .is_some_and(|value| value.starts_with("table-column"))
                            || style
                                .get(Prop::POSITION)
                                .is_some_and(|value| matches!(value, "absolute" | "fixed"))
                    }
                }
            })
        {
            return true;
        }
        let rows = doc.nodes[host]
            .children
            .iter()
            .copied()
            .filter(|&child| matches!(style_flags[child] >> 8, 10..=12))
            .collect::<Vec<_>>();
        for id in std::iter::once(host).chain(rows) {
            let (own, flags) = (styles[id].clone(), style_flags[id]);
            let (role, display) = match flags >> 8 {
                10 => (ROW, "contents"),
                11 => (5, "contents"),
                12 => (HEADER, "contents"),
                _ => (role, ["grid", "inline-grid"][usize::from(role == 14)]),
            };
            let kept = previous
                .filter(|previous| fast && previous.anonymous.contains_key(&id))
                .filter(|previous| {
                    previous
                        .authored
                        .get(&id)
                        .is_some_and(|old| Rc::ptr_eq(old, &own))
                })
                .and_then(|previous| previous.values.get(id).cloned());
            styles[id] = kept.unwrap_or_else(|| {
                let mut style = (*own).clone();
                style.insert(Prop::DISPLAY, display);
                if id == host {
                    style.insert(Prop::JUSTIFY_CONTENT, "start");
                    style.insert(Prop::ALIGN_CONTENT, "start");
                    border_spacing(&mut style);
                }
                Rc::new(style)
            });
            style_flags[id] =
                (flags & 0xff) | role << 8 | (u16::from(display == "contents") * DISPLAY_CONTENTS);
            authored.insert(id, own);
            anonymous.insert(id, flags);
        }
        true
    });
}
