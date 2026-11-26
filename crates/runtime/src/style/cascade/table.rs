use crate::css::{properties::BORDERS, properties::Prop, properties::Style};
use crate::dom::Document;
use crate::style::cascade::TreeWalk;
use crate::style::cascade::computed::{
    CAPTION, CELL, ComputedStyles, DISPLAY_CONTENTS, DISPLAY_NONE, HEADER, ROW,
};
use crate::style::cascade::memo::StyleRef;
use crate::style::used::border_spacing;
use crate::style::{Chunked, Shadows, StyleCache};
use rustc_hash::FxHashMap;
use std::rc::Rc;

pub(crate) type CellKey = (StyleRef, [(u32, Option<[Option<Rc<str>>; 2]>); 4]);

#[derive(PartialEq)]
pub(crate) struct Line {
    pub(crate) row: (usize, StyleRef),
    pub(crate) group: Option<(usize, StyleRef)>,
    // Each cell with its first column, columns and rows.
    pub(crate) cells: Vec<(usize, StyleRef, [usize; 3])>,
}

#[derive(Default)]
pub(crate) struct CollapsedTable {
    pub(crate) pass: u64,
    pub(crate) size: [f32; 2],
    pub(crate) table: Option<(StyleRef, [u32; 4], Rc<Style>)>,
    pub(crate) lines: Vec<Line>,
    // The count of columns, or 0 when cells span many rows or columns have borders of their own.
    pub(crate) columns: usize,
    pub(crate) adjusted: Vec<(Vec<Rc<Style>>, [f32; 4])>,
    pub(crate) styles: FxHashMap<CellKey, Rc<Style>>,
}

// The columns or rows of a cell, where 0 stands for all rows to the end of the row group.
pub(crate) fn span(node: &crate::dom::Node, name: &str) -> usize {
    let span = node
        .attrs
        .get(name)
        .and_then(|value| value.trim().parse::<usize>().ok());
    match (span, name) {
        (Some(0), "rowspan") => 4095,
        (Some(span), "rowspan") => span.min(4095),
        (Some(span), _) if span > 0 => span.min(1000),
        _ => 1,
    }
}

// The columns of the cells of a table, row by row.
#[derive(Default)]
pub(crate) struct Slots {
    // The columns that cells of rows above take, and the row after these cells.
    taken: Vec<[usize; 3]>,
    fresh: Vec<[usize; 3]>,
    row: usize,
    next: usize,
    column: usize,
    // Cells start in the columns before `dense` and in `starts`.
    dense: usize,
    starts: Vec<usize>,
}

impl Slots {
    pub(crate) fn place(&mut self, row: usize, columns: usize, rows: usize) -> usize {
        if row != self.row {
            self.taken.retain(|run| run[2] > row);
            for run in self.fresh.drain(..).filter(|run| run[2] > row) {
                let at = self.taken.partition_point(|taken| taken[0] < run[0]);
                self.taken.insert(at, run);
            }
            (self.row, self.next, self.column) = (row, 0, 0);
        }
        while let Some(run) = self
            .taken
            .get(self.next)
            .filter(|run| run[0] <= self.column)
        {
            self.column = self.column.max(run[1]);
            self.next += 1;
        }
        let column = self.column;
        self.column += columns;
        if rows > 1 {
            self.fresh.push([column, self.column, row + rows]);
        }
        if column == self.dense {
            self.dense += 1;
        } else if column > self.dense
            && let Err(at) = self.starts.binary_search(&column)
        {
            self.starts.insert(at, column);
        }
        column
    }

    // Like Chrome, columns past `kept` where no cell starts are no columns of their own.
    pub(crate) fn merge(mut self, kept: usize) -> impl Fn(usize) -> usize {
        let kept = kept.max(self.dense);
        self.starts.retain(|&start| start >= kept);
        move |column| column.min(kept) + self.starts.partition_point(|&start| start < column)
    }
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
            // Headers come first and footers last, like the rows of the grid.
            let mut lines: Vec<Line> = (0..3)
                .flat_map(|rank| {
                    doc.nodes[table]
                        .children
                        .iter()
                        .map(move |child| (rank, child))
                })
                .filter(|&(rank, &child)| {
                    rank == match doc.nodes[child].tag.as_str() {
                        "thead" => 0,
                        "tfoot" => 2,
                        _ => 1,
                    }
                })
                .flat_map(|(_, child)| {
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
                        .filter(|&&cell| {
                            matches!(doc.nodes[cell].tag.as_str(), "td" | "th")
                                && styles[cell].get(Prop::DISPLAY) != Some("none")
                        })
                        .map(|&cell| {
                            let node = &doc.nodes[cell];
                            let place = [0, span(node, "colspan"), span(node, "rowspan")];
                            (cell, StyleRef(own_style(styles, cell)), place)
                        })
                        .collect(),
                })
                .filter(|line| !line.cells.is_empty())
                .collect();
            let group_of = |line: &Line| line.group.as_ref().map(|group| group.0);
            let mut slots = Slots::default();
            let mut tallest = 1;
            let mut end = 0;
            for index in 0..lines.len() {
                if index == end {
                    let group = group_of(&lines[index]);
                    let same = |line: &&Line| group_of(line) == group;
                    end += lines[index..].iter().take_while(same).count();
                }
                for (.., [column, columns, rows]) in &mut lines[index].cells {
                    *rows = (*rows).min(end - index);
                    tallest = tallest.max(*rows);
                    *column = slots.place(index, *columns, *rows);
                }
            }
            let real = slots.merge(0);
            let columns = real(usize::MAX);
            let rtl = table_style.get(Prop::DIRECTION) == Some("rtl");
            let range = |start: usize, end: usize| match [real(start), real(end)] {
                [start, end] if rtl => [columns - end, columns - start],
                range => range,
            };
            for line in &mut lines {
                for (.., [column, span, _]) in &mut line.cells {
                    let [start, end] = range(*column, *column + *span);
                    [*column, *span] = [start, end - start];
                }
                if rtl {
                    line.cells.reverse();
                }
            }
            // The `<col>` and `<colgroup>` with borders: the node, its style and its columns.
            let mut cols: [Vec<(usize, StyleRef, [usize; 2])>; 2] = Default::default();
            let mut add = |list: usize, node: usize, [start, end]: [usize; 2]| {
                let style = own_style(styles, node);
                let bordered = SIDES
                    .iter()
                    .any(|side| style.get(side[0]).is_some_and(|kind| kind != "none"));
                let range = range(start, end);
                if bordered && range[0] < range[1] {
                    cols[list].push((node, StyleRef(style), range));
                }
            };
            let mut at = 0;
            for &group in &doc.nodes[table].children {
                if doc.nodes[group].tag != "colgroup" {
                    continue;
                }
                let start = at;
                for &col in &doc.nodes[group].children {
                    if doc.nodes[col].tag == "col" {
                        let end = at + span(&doc.nodes[col], "span");
                        add(0, col, [at, end]);
                        at = end;
                    }
                }
                if at == start {
                    at += span(&doc.nodes[group], "span");
                }
                add(1, group, [start, at]);
            }
            if rtl {
                cols.iter_mut().for_each(|list| list.reverse());
            }
            // The cells that span rows, by column: the rows and the place in the first row.
            let mut tall: Vec<Vec<[usize; 3]>> = Vec::new();
            if tallest > 1 {
                tall = vec![Vec::new(); columns];
                for (index, line) in lines.iter().enumerate() {
                    for (at, (.., [column, columns, rows])) in line.cells.iter().enumerate() {
                        if *rows > 1 {
                            for list in &mut tall[*column..*column + *columns] {
                                list.push([index, index + rows, at]);
                            }
                        }
                    }
                }
            }
            // The width, where a hidden border is infinite, the rank of the style, the owner and its side.
            type Edge = (f32, u8, usize, usize);
            let wins = |a: Edge, b: Edge| if (b.0, b.1) > (a.0, a.1) { b } else { a };
            let mut edge = |style: &Rc<Style>, id: usize, side: usize| -> Edge {
                let sides = match edges.get(&Rc::as_ptr(style)) {
                    Some((_, sides)) => *sides,
                    None => {
                        let sides = SIDES.map(|[kind, width, ..]| {
                            match style.get(kind).unwrap_or("none") {
                                "none" => (0., 0),
                                "hidden" => (f32::INFINITY, 0),
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
            // The border of a side of a cell, before its neighbors and the table.
            let mut own = |index: usize, at: usize, side: usize| -> Edge {
                let (cell, style, [column, span, rows]) = &lines[index].cells[at];
                let row = if side == 2 { index + rows - 1 } else { index };
                let line = &lines[row];
                let mut found = edge(&style.0, *cell, side);
                let outer = match side {
                    1 => column + span >= columns,
                    3 => *column == 0,
                    _ => true,
                };
                if outer {
                    found = wins(found, edge(&line.row.1.0, line.row.0, side));
                }
                if let Some((group, style)) = &line.group {
                    let next = match side {
                        0 => row.checked_sub(1),
                        _ => Some(row + 1),
                    };
                    let outer = match side {
                        0 | 2 => next
                            .and_then(|next| lines.get(next))
                            .is_none_or(|next| group_of(next) != Some(*group)),
                        _ => outer,
                    };
                    if outer {
                        found = wins(found, edge(&style.0, *group, side));
                    }
                }
                for (list, inside) in cols.iter().zip([true, false]) {
                    let last = column + span - 1;
                    let at = if side == 1 { last } else { *column };
                    let found_at = list.partition_point(|(.., [_, end])| *end <= at);
                    let applies = |[start, end]: &[usize; 2]| match side {
                        0 => row == 0 && start <= column,
                        // Each column of a `<col>` has its borders.
                        1 => *start <= last && (inside || *end == column + span),
                        2 => row + 1 == lines.len() && start <= column,
                        _ => start == column || inside && start < column,
                    };
                    if let Some((col, style, _)) =
                        list.get(found_at).filter(|(.., range)| applies(range))
                    {
                        found = wins(found, edge(&style.0, *col, side));
                    }
                }
                found
            };
            // The cell at a row and column.
            let find = |row: usize, column: usize| -> Option<(usize, usize)> {
                let cells = &lines.get(row)?.cells;
                let at = cells.partition_point(|(.., [start, span, _])| start + span <= column);
                if cells
                    .get(at)
                    .is_some_and(|(.., [start, ..])| *start <= column)
                {
                    return Some((row, at));
                }
                let list = tall.get(column)?;
                let &[start, end, at] = list[..list.partition_point(|run| run[0] <= row)].last()?;
                (end > row).then_some((start, at))
            };
            // The borders of the cells of a line. Of equal ones, the border of the cell at the left or top wins.
            let mut resolve = |index: usize| -> Vec<[Edge; 4]> {
                (0..lines[index].cells.len())
                    .map(|at| {
                        let (.., [column, span, rows]) = lines[index].cells[at];
                        std::array::from_fn(|side| {
                            let mut found = own(index, at, side);
                            let (outer, along) = match side {
                                0 => (index == 0, column..column + span),
                                1 => (column + span >= columns, index..index + rows),
                                2 => (index + rows >= lines.len(), column..column + span),
                                _ => (column == 0, index..index + rows),
                            };
                            if outer {
                                return wins(found, frame[side]);
                            }
                            let mut last = None;
                            for place in along {
                                let other = match side {
                                    0 => find(index - 1, place),
                                    1 => find(place, column + span),
                                    2 => find(index + rows, place),
                                    _ => find(place, column - 1),
                                };
                                let Some(other) = other.filter(|_| other != last) else {
                                    continue;
                                };
                                last = Some(other);
                                let theirs = own(other.0, other.1, (side + 2) % 4);
                                found = match side {
                                    0 | 3 => wins(theirs, found),
                                    _ => wins(found, theirs),
                                };
                            }
                            found
                        })
                    })
                    .collect()
            };
            let mut old = self.tables.remove(&table).unwrap_or_default();
            // The lines that the borders of a line depend on, before and after it.
            let reach = if tallest > 1 { 2 * tallest } else { 1 };
            let plain = if reach > 8 || cols.iter().any(|list| !list.is_empty()) {
                0
            } else {
                columns
            };
            let whole = plain == 0
                || old.columns != plain
                || old.size != [width, height]
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
                        && (0..=2 * reach).all(|step| {
                            lines.get((index + step).wrapping_sub(reach))
                                == old.lines.get((before + step).wrapping_sub(reach))
                        })
                });
                let line = match same {
                    Some(before) => std::mem::take(&mut old.adjusted[before]),
                    None => {
                        let mut cells = resolve(index);
                        let mut outer = [0f32; 4];
                        for (sides, (.., [column, span, rows])) in
                            cells.iter_mut().zip(&lines[index].cells)
                        {
                            for (side, outer) in outer.iter_mut().enumerate() {
                                if sides[side].0 == f32::INFINITY {
                                    sides[side].0 = 0.;
                                }
                                let at = match side {
                                    0 => index == 0,
                                    1 => column + span >= columns,
                                    2 => index + rows >= lines.len(),
                                    _ => *column == 0,
                                };
                                if at {
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
                                    let (width, rank, owner, from) = sides[side];
                                    let [_, kind, color] = BORDERS[from];
                                    (
                                        width,
                                        (owner != *cell || from != side || matches!(rank, 1 | 3))
                                            .then(|| {
                                                let owner = own_style(styles, owner);
                                                [
                                                    // Collapsed borders draw `inset` as `ridge` and `outset` as `groove`.
                                                    match rank {
                                                        _ if width <= 0. => Some("none".into()),
                                                        1 => Some("ridge".into()),
                                                        3 => Some("groove".into()),
                                                        _ => owner.value(kind).cloned(),
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
                    columns: plain,
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
