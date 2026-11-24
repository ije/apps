use super::super::measure::Measurements;
use super::intrinsic::{intrinsic, pseudo_item};
use super::track::{Lines, Sizing, Track, placement, size, tracks};
use super::{Baselines, Grid};
use crate::css;
use crate::css::properties::Prop;
use crate::{
    css::value::units::Lengths,
    dom::Document,
    style::{self, ComputedStyles},
};
use std::collections::HashMap;

/// Margin, border and padding of each side: top, right, bottom, left.
pub(super) fn frame(style: &css::properties::Style, units: &Lengths, base: f32) -> [f32; 4] {
    let [margin, padding] = [css::properties::MARGIN, css::properties::PADDING]
        .map(|sides| units.edges(style, sides, base));
    std::array::from_fn(|side| {
        let [size, kind, _] = css::properties::BORDERS[side];
        let border = if style
            .get(kind)
            .is_none_or(|v| matches!(v, "none" | "hidden"))
        {
            0.
        } else {
            style
                .get(size)
                .map_or(Some(3.), |v| units.resolve(v, base))
                .unwrap_or(0.)
        };
        margin[side] + padding[side] + border.max(0.)
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn layout(
    doc: &Document,
    id: usize,
    styles: &ComputedStyles,
    width: Option<f32>,
    height: Option<f32>,
    inherited: Option<&Grid>,
    baselines: Option<&Baselines>,
    text: &renderer::TextSystem,
    measured: &mut Measurements,
) -> Grid {
    use Sizing::*;
    let style = &styles.values[id];
    let units = &styles.lengths[id];
    if style
        .get(Prop::DISPLAY)
        .is_some_and(|value| value.ends_with("grid"))
    {
        measured.note_runs(doc, styles, id, &doc.nodes[id].children, false);
    }
    // An anonymous table is as wide as its content, inside its block.
    let block = width;
    let width = match width {
        Some(space) if space > 0. && matches!(style::role(styles, id), 13 | 14) => {
            let [min, max] = [Some(0.), None].map(|width| {
                let grid = layout(doc, id, styles, width, None, None, None, text, measured);
                grid.columns.iter().sum::<f32>()
                    + grid.columns.len().saturating_sub(1) as f32 * grid.gap[0]
            });
            Some(space.min(max).max(min))
        }
        width => width,
    };
    let available = [width, height];
    let get = |prop| style.get(prop);
    let outermost = measured.edges.is_none();
    if outermost {
        measured.edges = Some(Default::default());
    }
    let template = [
        get(Prop::GRID_TEMPLATE_COLUMNS),
        get(Prop::GRID_TEMPLATE_ROWS),
    ];
    let areas = get(Prop::GRID_TEMPLATE_AREAS);
    let flow = get(Prop::GRID_AUTO_FLOW).unwrap_or("row");
    let implicit =
        [get(Prop::GRID_AUTO_COLUMNS), get(Prop::GRID_AUTO_ROWS)].map(|v| v.unwrap_or("auto"));
    let shared = [0, 1].map(|axis| {
        inherited
            .map(|grid| if axis == 0 { &grid.columns } else { &grid.rows })
            .filter(|sizes| {
                !sizes.is_empty() && template[axis].is_some_and(|v| v.starts_with("subgrid"))
            })
    });
    let gap = [0, 1].map(|axis| {
        match (
            get([Prop::COLUMN_GAP, Prop::ROW_GAP][axis]).filter(|v| *v != "normal"),
            shared[axis],
        ) {
            (None, Some(_)) => inherited.map_or(0., |grid| grid.gap[axis]),
            (value, _) => value
                .filter(|v| available[axis].is_some() || !v.contains('%'))
                .and_then(|v| units.resolve(v, available[axis].unwrap_or(0.)))
                .unwrap_or(0.)
                .max(0.),
        }
    });
    let area_rows: Vec<Vec<&str>> = areas
        .unwrap_or_default()
        .split(['"', '\''])
        .skip(1)
        .step_by(2)
        .map(css::value::area_cells)
        .collect();
    let mut regions: HashMap<&str, [usize; 4]> = HashMap::new();
    for (y, row) in area_rows.iter().enumerate() {
        for (x, &name) in row.iter().enumerate() {
            if name != "." {
                let region = regions.entry(name).or_insert([x, y, x + 1, y + 1]);
                *region = [
                    region[0].min(x),
                    region[1].min(y),
                    region[2].max(x + 1),
                    region[3].max(y + 1),
                ];
            }
        }
    }
    let mut names: [Lines; 2] = Default::default();
    for (name, [left, top, right, bottom]) in regions {
        names[0].extend([
            (format!("{name}-start"), left),
            (format!("{name}-end"), right),
        ]);
        names[1].extend([
            (format!("{name}-start"), top),
            (format!("{name}-end"), bottom),
        ]);
    }
    let mut defined: [Vec<Track>; 2] = Default::default();
    let mut explicit = [0; 2];
    let mut fit = [None; 2];
    for axis in 0..2 {
        if let Some(sizes) = shared[axis] {
            // The parent tracks hold the subgrid's margin, border and padding at both ends.
            let frame = frame(style, units, width.unwrap_or(0.));
            let [start, end] = if axis == 0 { [3, 1] } else { [0, 2] }.map(|side| frame[side]);
            let last = sizes.len() - 1;
            defined[axis] = sizes
                .iter()
                .enumerate()
                .map(|(i, &size)| {
                    let size =
                        (size - if i == 0 { start } else { 0. } - if i == last { end } else { 0. })
                            .max(0.);
                    (Fixed(size), Fixed(size))
                })
                .collect();
            explicit[axis] = sizes.len();
            continue;
        }
        let (list, lines, collapse) = tracks(
            template[axis].unwrap_or("none"),
            units,
            available[axis],
            gap[axis],
        );
        let extent = if axis == 0 {
            area_rows.iter().map(Vec::len).max().unwrap_or(0)
        } else {
            area_rows.len()
        };
        explicit[axis] = list.len().max(extent).min(4096);
        names[axis].extend(lines);
        defined[axis] = list;
        fit[axis] = collapse;
    }
    let automatic = [0, 1].map(|axis| tracks(implicit[axis], units, available[axis], 0.).0);
    // The minimum contribution of an item without a preferred size: the outer size of its min size, or of 0 for a
    // scroll container. Other items contribute their min-content size.
    let least = |style: &css::properties::Style, units: &Lengths, axis: usize| -> Option<f32> {
        // A percentage depends on the track, so it counts as `auto` here.
        if style
            .get([Prop::WIDTH, Prop::HEIGHT][axis])
            .is_some_and(|v| v != "auto" && !v.contains('%'))
        {
            return None;
        }
        let base = available[0].unwrap_or(0.);
        let size = match style.get([Prop::MIN_WIDTH, Prop::MIN_HEIGHT][axis]) {
            Some(value) if value.contains('%') => 0.,
            Some(value) if let Some(size) = units.resolve(value, 0.) => size,
            // Like Chrome, content keywords act as `auto` in columns and as 0 in rows.
            Some(value) if axis == 1 && value.contains("content") => 0.,
            _ if style
                .get([Prop::OVERFLOW_X, Prop::OVERFLOW_Y][axis])
                .is_some_and(|v| !matches!(v, "visible" | "clip")) =>
            {
                0.
            }
            _ => return None,
        };
        let frame = frame(style, units, base);
        let margin = units.edges(style, css::properties::MARGIN, base);
        let [start, end] = if axis == 0 { [3, 1] } else { [0, 2] };
        let (sides, margins) = (frame[start] + frame[end], margin[start] + margin[end]);
        Some(if style.get(Prop::BOX_SIZING) == Some("border-box") {
            (size + margins).max(sides)
        } else {
            size + sides
        })
    };
    let alignment = |own: &css::properties::Style, axis: usize, replaced: bool| {
        let margins = [
            [Prop::MARGIN_LEFT, Prop::MARGIN_RIGHT],
            [Prop::MARGIN_TOP, Prop::MARGIN_BOTTOM],
        ][axis]
            .map(|name| own.get(name).is_some_and(|v| v == "auto"));
        let mut value = "normal";
        for (style, name) in [
            (own, [Prop::JUSTIFY_SELF, Prop::ALIGN_SELF][axis]),
            (style, [Prop::JUSTIFY_ITEMS, Prop::ALIGN_ITEMS][axis]),
        ] {
            if let Some(found) = style.get(name).filter(|v| *v != "auto") {
                value = found;
                break;
            }
        }
        let last = value.starts_with("last");
        match (margins, value.split_whitespace().last().unwrap_or(value)) {
            ([true, true], _) => (Some(0.5), None),
            ([true, false], _) => (Some(1.), None),
            ([false, true], _) => (Some(0.), None),
            (_, "normal") if replaced => (Some(0.), None),
            // Like Chrome, a box with a ratio and an auto height keeps the height of its ratio.
            (_, "normal")
                if axis == 1
                    && own.get(Prop::ASPECT_RATIO).is_some_and(|v| v != "auto")
                    && own.get(Prop::HEIGHT).is_none_or(|v| v == "auto") =>
            {
                (Some(0.), None)
            }
            (_, "normal" | "stretch") => (None, None),
            (_, "center") => (Some(0.5), None),
            (_, "end" | "flex-end" | "self-end" | "right") => (Some(1.), None),
            (_, "baseline") => (Some(f32::from(u8::from(last))), (axis == 1).then_some(last)),
            _ => (Some(0.), None),
        }
    };
    #[derive(Clone, Copy)]
    struct Item<'a> {
        flowed: bool,
        order: i32,
        placed: ([Option<usize>; 2], [usize; 2]),
        alignment: [Option<f32>; 2],
        baseline: Option<bool>,
        auto_width: bool,
        height: Option<&'a str>,
        subgrid: [bool; 2],
    }
    let mut memo: rustc_hash::FxHashMap<(*const css::properties::Style, bool), Item> =
        Default::default();
    let table = style::table_box(style::role(styles, id));
    let legend = (doc.nodes[id].tag == "fieldset")
        .then(|| super::super::size::legend(doc, styles, id))
        .flatten()
        .map(|(legend, ..)| legend);
    let pseudos = styles.generated(id).filter(|_| !table);
    let item_style = |child: usize| match pseudo_item(child) {
        Some((_, index)) => &pseudos.expect("a pseudo-element item")[index],
        None => &*styles.values[child],
    };
    let mut read = |child: usize| -> Item {
        let style = item_style(child);
        let replaced = pseudo_item(child).is_none()
            && (doc.nodes[child].tag == "svg"
                || doc.nodes[child].tag != "input"
                    && super::super::size::natural_size(
                        doc,
                        child,
                        measured.naturals.get(&child).copied(),
                        &styles.values[child],
                    )
                    .is_some());
        let mut item = *memo
            .entry((std::ptr::from_ref(style), replaced))
            .or_insert_with(|| {
                let flowed = !style.get(Prop::DISPLAY).is_some_and(|v| v == "none")
                    && !style
                        .get(Prop::POSITION)
                        .is_some_and(|v| matches!(v, "absolute" | "fixed"));
                let order = style
                    .get(Prop::ORDER)
                    .and_then(|v| v.parse::<i32>().ok())
                    .unwrap_or(0);
                let placed = {
                    let (mut position, mut span) = ([None; 2], [1; 2]);
                    for axis in 0..2 {
                        let [start, end] = [
                            [Prop::GRID_COLUMN_START, Prop::GRID_COLUMN_END],
                            [Prop::GRID_ROW_START, Prop::GRID_ROW_END],
                        ][axis];
                        (position[axis], span[axis]) = placement(
                            style.get(start).unwrap_or("auto"),
                            style.get(end).unwrap_or("auto"),
                            &names[axis],
                            explicit[axis],
                        );
                        if shared[axis].is_some() {
                            let start = position[axis].map(|v| v.min(explicit[axis] - 1));
                            position[axis] = start;
                            span[axis] = span[axis].min(explicit[axis] - start.unwrap_or(0));
                        }
                    }
                    (position, span)
                };
                let subgrid = [0, 1].map(|axis| {
                    pseudo_item(child).is_none()
                        && style
                            .get([Prop::GRID_TEMPLATE_COLUMNS, Prop::GRID_TEMPLATE_ROWS][axis])
                            .is_some_and(|v| v.starts_with("subgrid"))
                });
                let [(column, _), (row, baseline)] =
                    [0, 1].map(|axis| alignment(style, axis, replaced));
                Item {
                    flowed,
                    order,
                    placed,
                    alignment: [column, row],
                    baseline,
                    auto_width: style.get(Prop::WIDTH).is_none_or(|v| v == "auto"),
                    height: style.get(Prop::HEIGHT),
                    subgrid,
                }
            });
        item.flowed &= legend != Some(child)
            && !(table
                && (matches!(doc.nodes[child].tag.as_str(), "col" | "colgroup")
                    || style
                        .get(Prop::DISPLAY)
                        .is_some_and(|v| v.starts_with("table-column"))));
        item
    };
    // Pseudo-elements are the first and last items, keyed by `pseudo_item`.
    let shown = |index: usize| {
        pseudos.is_some_and(|pseudos| {
            pseudos[index]
                .get(Prop::CONTENT)
                .is_some_and(|value| !matches!(value, "none" | "normal"))
        })
    };
    let mut children: Vec<(usize, Item)> = (shown(0).then_some(!(id * 2)).into_iter())
        .chain(doc.nodes[id].children.iter().copied().filter(|&child| {
            doc.nodes[child].tag != "#text" || measured.text_item(doc, child, false)
        }))
        .chain(shown(1).then_some(!(id * 2 + 1)))
        .map(|child| (child, read(child)))
        .filter(|(_, item)| item.flowed)
        .collect();
    if table {
        // Captions and header rows go first and footer rows last, whatever their place in the DOM.
        children.sort_by_key(|&(child, item)| {
            let section =
                std::iter::successors(doc.nodes[child].parent, |&parent| doc.nodes[parent].parent)
                    .take(2)
                    .find_map(|parent| match style::role(styles, parent) {
                        style::HEADER => Some(-1),
                        style::FOOTER => Some(1),
                        _ => None,
                    });
            let rank = match style::role(styles, child) {
                style::CAPTION
                    if styles.values[child].get(Prop::CAPTION_SIDE) == Some("bottom") =>
                {
                    2
                }
                style::CAPTION => -2,
                _ => section.unwrap_or(0),
            };
            (rank, item.order)
        });
        // Each row, caption and run of cells outside rows is a lane of the grid.
        let mut lane = None;
        for (child, item) in &mut children {
            let key = if style::role(styles, *child) == style::CAPTION {
                Some(*child)
            } else {
                doc.nodes[*child]
                    .parent
                    .filter(|&row| (style::ROW..=style::FOOTER).contains(&style::role(styles, row)))
            };
            let index = match lane {
                Some((index, last)) if last == key => index,
                Some((index, _)) => index + 1,
                None => 0,
            };
            lane = Some((index, key));
            item.placed.0[1] = item.placed.0[1].or(Some(index));
        }
    } else {
        children.sort_by_key(|(_, item)| item.order);
    }
    let (children, derived): (Vec<usize>, Vec<Item>) = children.into_iter().unzip();
    let items: Vec<([Option<usize>; 2], [usize; 2])> =
        derived.iter().map(|item| item.placed).collect();
    let across = usize::from(flow.contains("column"));
    let down = 1 - across;
    let dense = flow.contains("dense");
    let mut count = explicit;
    for (position, span) in &items {
        count[across] = count[across].max(position[across].unwrap_or(0) + span[across]);
    }
    count[across] = count[across].max(1);
    let fits = |occupied: &Vec<Vec<bool>>, at: [usize; 2], span: [usize; 2]| {
        (at[down]..at[down] + span[down]).all(|y| {
            occupied.get(y).is_none_or(|row| {
                row.get(at[across]..(at[across] + span[across]).min(row.len()))
                    .is_none_or(|cells| !cells.contains(&true))
            })
        })
    };
    let mark = |occupied: &mut Vec<Vec<bool>>, at: [usize; 2], span: [usize; 2]| {
        let end = [at[0] + span[0], at[1] + span[1]];
        if occupied.len() < end[down] {
            occupied.resize(end[down], Vec::new());
        }
        for row in &mut occupied[at[down]..end[down]] {
            if row.len() < end[across] {
                row.resize(end[across], false);
            }
            row[at[across]..end[across]].fill(true);
        }
    };
    let mut occupied = Vec::new();
    let mut placed = vec![[0; 2]; items.len()];
    for (index, (position, span)) in items.iter().enumerate() {
        if let [Some(column), Some(row)] = *position {
            placed[index] = [column, row];
            mark(&mut occupied, placed[index], *span);
        }
    }
    // Bound dense-placement cell scans, then fall back to sparse placement.
    let mut budget = 1usize << 24;
    let mut lanes = HashMap::new();
    for (index, (position, span)) in items.iter().enumerate() {
        if let (None, Some(lane)) = (position[across], position[down]) {
            let mut at = [0; 2];
            at[down] = lane;
            at[across] = if dense && budget > 0 {
                0
            } else {
                lanes.get(&lane).copied().unwrap_or(0)
            };
            while budget > 0 && !fits(&occupied, at, *span) && at[across] + span[across] < 4096 {
                budget = budget.saturating_sub(span[0] * span[1]);
                at[across] += 1;
            }
            lanes.insert(lane, at[across] + span[across]);
            count[across] = count[across].max(at[across] + span[across]);
            placed[index] = at;
            mark(&mut occupied, at, *span);
        }
    }
    let mut cursor = [0; 2];
    let mut free = 0;
    for (index, (position, span)) in items.iter().enumerate() {
        if position[down].is_some() {
            continue;
        }
        let dense = dense && budget > 0;
        let mut at = cursor;
        if dense {
            while occupied.get(free).is_some_and(|row: &Vec<bool>| {
                row.len() >= count[across] && !row[..count[across]].contains(&false)
            }) {
                free += 1;
            }
            budget = budget.saturating_sub(count[across]);
            at = [0; 2];
            at[down] = free;
        }
        if let Some(fixed) = position[across] {
            if !dense && fixed < at[across] {
                at[down] += 1;
            }
            at[across] = fixed;
            while budget > 0 && !fits(&occupied, at, *span) && at[down] < 4096 {
                budget = budget.saturating_sub(span[0] * span[1]);
                at[down] += 1;
            }
        } else {
            loop {
                if at[across] + span[across] > count[across] {
                    at[across] = 0;
                    at[down] += 1;
                }
                if at[down] >= 4096 || budget == 0 || fits(&occupied, at, *span) {
                    break;
                }
                budget = budget.saturating_sub(span[0] * span[1]);
                at[across] += 1;
            }
        }
        at[down] = at[down].min(4096 - span[down]);
        placed[index] = at;
        mark(&mut occupied, at, *span);
        cursor = at;
        cursor[across] += span[across];
    }
    for (at, (_, span)) in placed.iter().zip(&items) {
        for axis in 0..2 {
            count[axis] = count[axis].max(at[axis] + span[axis]);
        }
    }
    let mut cells: Vec<[usize; 4]> = placed
        .iter()
        .zip(&items)
        .zip(&children)
        .map(|((at, (_, span)), &child)| {
            let mut cell = [at[0], at[1], span[0], span[1]];
            if table && style::role(styles, child) == style::CAPTION {
                cell[2] = count[0];
            }
            for axis in 0..2 {
                if shared[axis].is_some() {
                    cell[axis] = cell[axis].min(explicit[axis] - 1);
                    cell[axis + 2] = cell[axis + 2].min(explicit[axis] - cell[axis]);
                }
            }
            cell
        })
        .collect();
    let mut definitions: [Vec<Track>; 2] = [0, 1].map(|axis| {
        let count = if shared[axis].is_some() {
            explicit[axis]
        } else {
            count[axis].min(4096)
        };
        (0..count)
            .map(|i| {
                defined[axis].get(i).copied().unwrap_or_else(|| {
                    let list = &automatic[axis];
                    list.get((i - defined[axis].len()) % list.len().max(1))
                        .copied()
                        .unwrap_or((Auto, Auto))
                })
            })
            .collect()
    });
    for axis in 0..2 {
        let Some((start, length)) = fit[axis] else {
            continue;
        };
        let mut used: Vec<bool> = (0..definitions[axis].len())
            .map(|i| !(start..start + length).contains(&i))
            .collect();
        for cell in &cells {
            if let Some(range) = used.get_mut(cell[axis]..cell[axis] + cell[axis + 2]) {
                range.fill(true);
            }
        }
        let mut next = 0;
        let remap: Vec<usize> = used
            .iter()
            .map(|&used| {
                next += usize::from(used);
                next - usize::from(used)
            })
            .collect();
        for cell in &mut cells {
            cell[axis] = remap.get(cell[axis]).copied().unwrap_or(0);
        }
        definitions[axis] = definitions[axis]
            .iter()
            .zip(&used)
            .filter(|(_, used)| **used)
            .map(|(track, _)| *track)
            .collect();
    }
    // Widths of `<col>`, `<colgroup>` and cells size the columns, only of the first row with `table-layout: fixed`.
    let mut columns = Vec::new();
    let mut percents: Vec<f32> = Vec::new();
    if table {
        let count = definitions[0].len();
        let base = available[0].map_or(0., |width| width - count.saturating_sub(1) as f32 * gap[0]);
        let width = |node: usize| {
            styles.values[node]
                .get(Prop::WIDTH)
                .filter(|v| available[0].is_some() || !v.contains('%'))
                .and_then(|v| styles.lengths[node].resolve(v, base))
        };
        columns.resize(count, None);
        let mut at = 0;
        for &group in doc.nodes[id].children.iter() {
            if doc.nodes[group].tag != "colgroup" {
                continue;
            }
            let cols = doc.nodes[group].children.iter().copied();
            let cols: Vec<usize> = cols.filter(|&col| doc.nodes[col].tag == "col").collect();
            for col in if cols.is_empty() { vec![group] } else { cols } {
                let span = doc.nodes[col]
                    .attrs
                    .get("span")
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(1)
                    .clamp(1, 1000);
                let size = width(col).or_else(|| width(group).filter(|_| col == group));
                for slot in columns.iter_mut().skip(at).take(span) {
                    *slot = size;
                }
                at += span;
            }
        }
        let fixed = style.get(Prop::TABLE_LAYOUT) == Some("fixed")
            && available[0].is_some()
            && style
                .get(Prop::WIDTH)
                .is_some_and(|v| !matches!(v, "auto" | "fit-content"));
        percents.resize(count, 0.);
        for (&child, cell) in children.iter().zip(&cells) {
            let Some(value) = styles.values[child]
                .get(Prop::WIDTH)
                .filter(|v| *v != "auto")
            else {
                continue;
            };
            if cell[2] != 1 || fixed && cell[1] != 0 || style::role(styles, child) == style::CAPTION
            {
                continue;
            }
            let size = match value.strip_suffix('%').and_then(|v| v.parse::<f32>().ok()) {
                Some(percent) => {
                    percents[cell[0]] = percents[cell[0]].max(percent / 100.);
                    match available[0] {
                        Some(_) => percent / 100. * base,
                        None => continue,
                    }
                }
                None => intrinsic(doc, child, styles, None, None, text, measured)[1],
            };
            if let Some(slot) = columns.get_mut(cell[0]) {
                *slot = match *slot {
                    Some(old) if fixed => Some(old),
                    Some(old) => Some(old.max(size)),
                    None => Some(size),
                };
            }
        }
        if fixed {
            for (track, size) in definitions[0].iter_mut().zip(&columns) {
                *track = match *size {
                    Some(size) => (Fixed(size), Fixed(size)),
                    None => (Fixed(0.), Flex(1.)),
                };
            }
        } else if columns.iter().any(Option::is_none)
            || percents.iter().any(|&percent| percent > 0.)
        {
            for (track, size) in definitions[0].iter_mut().zip(&columns) {
                if let Some(size) = *size {
                    *track = (Auto, Fixed(size));
                }
            }
        }
    }
    let starts = |sizes: &[f32]| {
        let mut sum = 0f32;
        std::iter::once(0.)
            .chain(sizes.iter().map(|size| {
                sum += size;
                sum
            }))
            .collect::<Vec<_>>()
    };
    let area = |sizes: &[f32], starts: &[f32], offset: f32, gap: f32, start: usize, span: usize| {
        [
            offset + starts[start.min(sizes.len())] + start as f32 * gap,
            sizes.iter().skip(start).take(span).sum::<f32>() + span.saturating_sub(1) as f32 * gap,
        ]
    };
    let mut grid = Grid {
        gap,
        ..Grid::default()
    };
    let mut offsets = [0.; 2];
    let mut boxes = vec![[0f32; 4]; cells.len()];
    let mut shims = Vec::new();
    for axis in 0..2 {
        if axis == 1 {
            let column_starts = starts(&grid.columns);
            let mut aligned = Vec::new();
            for (index, (&child, cell)) in children.iter().zip(&cells).enumerate() {
                let [x, area] = area(
                    &grid.columns,
                    &column_starts,
                    offsets[0],
                    grid.gap[0],
                    cell[0],
                    cell[2],
                );
                let [min, max, used, height] =
                    intrinsic(doc, child, styles, Some(area), None, text, measured);
                let fraction = derived[index].alignment[0];
                let caption = table && style::role(styles, child) == style::CAPTION;
                // A caption sits outside the border spacing of the table.
                let [x, area] = if caption {
                    [x - gap[0], area + 2. * gap[0]]
                } else {
                    [x, area]
                };
                let size = if table {
                    area
                // Like Chrome, a table item of `auto` width stretches.
                } else if fraction.is_none()
                    && style::table_box(style::role(styles, child))
                    && styles.values[child].get(Prop::WIDTH) == Some("fit-content")
                {
                    area.max(min)
                } else if fraction.is_some() && derived[index].auto_width {
                    max.min(area).max(min)
                } else {
                    used
                };
                let mut height = if size == used && !caption {
                    height
                } else {
                    intrinsic(doc, child, styles, Some(size), None, text, measured)[3]
                };
                // A subgrid of columns is as tall as its rows on the parent's columns.
                if derived[index].subgrid == [true, false]
                    && derived[index].height.is_none_or(|v| v == "auto")
                {
                    let frame = frame(&styles.values[child], &styles.lengths[child], area);
                    let inner = layout(
                        doc,
                        child,
                        styles,
                        Some(size - frame[1] - frame[3]),
                        None,
                        Some(&Grid {
                            columns: grid.columns[cell[0]..cell[0] + cell[2]].to_vec(),
                            gap: grid.gap,
                            ..Grid::default()
                        }),
                        None,
                        text,
                        measured,
                    );
                    height = inner.extent(1) + frame[0] + frame[2];
                }
                boxes[index] = [x + (area - size) * fraction.unwrap_or(0.), 0., size, height];
                if let (Some(last), Some(&[first, end])) = (
                    derived[index].baseline,
                    baselines.and_then(|baselines| baselines.get(&child)),
                ) {
                    let [top, _, bottom, _] = styles.lengths[child].edges(
                        &styles.values[child],
                        css::properties::MARGIN,
                        area,
                    );
                    let border = height - top - bottom;
                    let offset = if last {
                        bottom + border - end.unwrap_or(border)
                    } else {
                        top + first.unwrap_or(border)
                    };
                    let row = if last { cell[1] + cell[3] - 1 } else { cell[1] };
                    aligned.push((index, (row, last), offset));
                }
            }
            if !aligned.is_empty() {
                let mut shared = rustc_hash::FxHashMap::default();
                for &(_, key, offset) in &aligned {
                    let max = shared.entry(key).or_insert(offset);
                    *max = offset.max(*max);
                }
                shims = vec![0.; cells.len()];
                for (index, key, offset) in aligned {
                    shims[index] = shared[&key] - offset;
                }
            }
        }
        let sized = |cell: &[usize; 4]| {
            available[axis].is_some()
                && definitions[axis]
                    .get(cell[axis]..cell[axis] + cell[axis + 2])
                    .is_some_and(|tracks| {
                        tracks.iter().all(|&(min, max)| {
                            !matches!(min, MinContent | MaxContent | Auto)
                                && !matches!(max, MinContent | MaxContent | Auto | FitContent(_))
                        })
                    })
        };
        let mut contributions = Vec::new();
        for (index, (&child, cell)) in children.iter().zip(&cells).enumerate() {
            if shared[axis].is_some() {
                break;
            }
            if sized(cell) {
                continue;
            }
            if !derived[index].subgrid[axis] {
                let [min, max] = if axis == 0 {
                    let value = intrinsic(doc, child, styles, None, None, text, measured);
                    // A caption widens the table to its min-content width only.
                    let caption = table && style::role(styles, child) == style::CAPTION;
                    [value[0], if caption { value[0] } else { value[1] }]
                } else {
                    // A caption takes the border spacing beside it from its row.
                    let spacing = if table && style::role(styles, child) == style::CAPTION {
                        gap[1]
                    } else {
                        0.
                    };
                    [boxes[index][3] + shims.get(index).copied().unwrap_or(0.) - spacing; 2]
                };
                contributions.push((
                    cell[axis],
                    cell[axis + 2],
                    [
                        least(
                            item_style(child),
                            &styles.lengths[pseudo_item(child).map_or(child, |(parent, _)| parent)],
                            axis,
                        )
                        .map_or(min, |least| least.min(min)),
                        min,
                        max,
                    ],
                ));
                continue;
            }
            let mut offset = 0;
            for &grandchild in &doc.nodes[child].children {
                let style = &styles.values[grandchild];
                if style.get(Prop::DISPLAY).is_some_and(|v| v == "none")
                    || doc.nodes[grandchild].tag == "#text"
                        && doc.nodes[grandchild].text.trim_ascii().is_empty()
                {
                    continue;
                }
                let start = cell[axis] + offset % cell[axis + 2];
                let span = [
                    [Prop::GRID_COLUMN_START, Prop::GRID_COLUMN_END],
                    [Prop::GRID_ROW_START, Prop::GRID_ROW_END],
                ][axis]
                    .iter()
                    .find_map(|&prop| {
                        style
                            .get(prop)?
                            .trim()
                            .strip_prefix("span")?
                            .trim()
                            .parse::<usize>()
                            .ok()
                    })
                    .unwrap_or(1)
                    .clamp(1, cell[axis] + cell[axis + 2] - start);
                let value = intrinsic(
                    doc,
                    grandchild,
                    styles,
                    (axis == 1).then_some(boxes[index][2]),
                    None,
                    text,
                    measured,
                );
                let [min, max] = if axis == 0 {
                    [value[0], value[1]]
                } else {
                    [value[3]; 2]
                };
                let least = least(style, &styles.lengths[grandchild], axis)
                    .map_or(min, |least| least.min(min));
                contributions.push((start, span, [least, min, max]));
                offset += span;
            }
        }
        if axis == 0 {
            // Percent columns grow to their share of the table, beyond the content of their cells only when there
            // is room.
            contributions.extend(
                (columns.iter().enumerate())
                    .filter(|&(i, _)| percents.get(i).is_none_or(|&percent| percent == 0.))
                    .filter_map(|(i, size)| size.map(|size| (i, 1, [0., 0., size]))),
            );
        } else if table {
            // The height of a row is the least height of its cells.
            for (&child, cell) in children.iter().zip(&cells) {
                if let Some(row) = doc.nodes[child]
                    .parent
                    .filter(|&row| style::role(styles, row) == style::ROW)
                    && let Some(height) = styles.values[row]
                        .get(Prop::HEIGHT)
                        .filter(|v| !v.contains('%'))
                        .and_then(|v| styles.lengths[row].resolve(v, 0.))
                {
                    contributions.push((cell[1], 1, [height; 3]));
                }
            }
        }
        let content = get([Prop::JUSTIFY_CONTENT, Prop::ALIGN_CONTENT][axis]).unwrap_or("normal");
        let stretch = matches!(content, "normal" | "stretch");
        let mut space = available[axis];
        let mut sizes = size(
            &definitions[axis],
            &contributions,
            gap[axis],
            space,
            stretch,
            axis == 0 && table,
            axis == 1,
        );
        // Like Chrome, the rows size again in a definite height: the min-height or max-height of the grid when they
        // leave it, or the height that flexible rows below `1fr` first gave the grid, so `0.5fr` takes half of it.
        if axis == 1 && space.is_none() && !table {
            let sides = if get(Prop::BOX_SIZING) == Some("border-box") {
                let base = available[0].unwrap_or(0.);
                let [edges, margin] = [
                    frame(style, units, base),
                    units.edges(style, css::properties::MARGIN, base),
                ];
                edges[0] + edges[2] - margin[0] - margin[2]
            } else {
                0.
            };
            let [least, most] = [Prop::MIN_HEIGHT, Prop::MAX_HEIGHT].map(|prop| {
                get(prop)
                    .filter(|v| !v.contains('%'))
                    .and_then(|v| units.resolve(v, 0.))
                    .map(|v| v - sides)
            });
            let factors: f32 = (definitions[1].iter())
                .filter_map(|track| match track.1 {
                    Flex(factor) => Some(factor),
                    _ => None,
                })
                .sum();
            let used = sizes.iter().sum::<f32>() + sizes.len().saturating_sub(1) as f32 * gap[1];
            let height = match (least, most) {
                (Some(least), _) if used < least => Some(least),
                (_, Some(most)) if factors > 0. && used > most => {
                    Some(most.max(least.unwrap_or(0.)))
                }
                _ if factors > 0. && factors < 1. => Some(used),
                _ => None,
            };
            if let Some(height) = height {
                grid.height = Some(height);
                space = Some(height);
                sizes = size(
                    &definitions[1],
                    &contributions,
                    gap[1],
                    space,
                    stretch,
                    false,
                    true,
                );
            }
        }
        if let Some(space) = space
            && !sizes.is_empty()
        {
            let count = sizes.len() as f32;
            let free = space - sizes.iter().sum::<f32>() - (count - 1.) * gap[axis];
            let (offset, extra) = match content.split_whitespace().last().unwrap_or(content) {
                _ if free < 0. && content.starts_with("safe") => (0., 0.),
                "center" => (free / 2., 0.),
                "end" | "flex-end" | "right" => (free, 0.),
                "space-between" if free > 0. && count > 1. => (0., free / (count - 1.)),
                "space-around" if free > 0. => (free / count / 2., free / count),
                "space-evenly" if free > 0. => (free / (count + 1.), free / (count + 1.)),
                "space-around" | "space-evenly" => (free / 2., 0.),
                _ => (0., 0.),
            };
            offsets[axis] = offset;
            grid.gap[axis] += extra;
        }
        // Like Chrome, the widest content of a table makes room for the share of its percent columns, and
        // columns of 100% with others beside them make it as wide as it can be.
        if axis == 0 && available[0].is_none() && percents.iter().any(|&percent| percent > 0.) {
            let mut sizes = sizes;
            let rest: f32 = sizes
                .iter()
                .zip(&percents)
                .filter(|(_, p)| **p == 0.)
                .map(|(size, _)| size)
                .sum();
            let total: f32 = percents.iter().sum();
            let width = sizes
                .iter()
                .zip(&percents)
                .filter(|(_, p)| **p > 0.)
                .map(|(size, p)| size / p)
                .fold(rest, f32::max);
            let width = match total < 1. {
                true => width.max(rest / (1. - total)),
                false if rest > 0. => 1e6,
                false => width,
            };
            for (size, percent) in sizes.iter_mut().zip(&percents) {
                *size = size.max(percent * width);
            }
            grid.columns = sizes;
        } else if axis == 0 {
            grid.columns = sizes;
        } else {
            grid.rows = sizes;
        }
    }
    let row_starts = starts(&grid.rows);
    let extent = block.unwrap_or_else(|| {
        grid.columns.iter().sum::<f32>()
            + grid.gap[0] * grid.columns.len().saturating_sub(1) as f32
            + offsets[0] * 2.
    });
    let rtl = style.get(Prop::DIRECTION) == Some("rtl");
    for (index, (&child, cell)) in children.iter().zip(&cells).enumerate() {
        let [y, area] = area(
            &grid.rows,
            &row_starts,
            offsets[1],
            grid.gap[1],
            cell[1],
            cell[3],
        );
        let [x, _, width, mut height] = boxes[index];
        let fraction = derived[index].alignment[1];
        let value = derived[index].height;
        if value.is_some_and(|v| v.contains('%')) {
            height = intrinsic(doc, child, styles, Some(width), Some(area), text, measured)[3];
        }
        let caption = table && style::role(styles, child) == style::CAPTION;
        if if table {
            !caption
        } else {
            fraction.is_none() && value.is_none_or(|v| v == "auto")
        } {
            // A min-height of a content keyword keeps the content height in a smaller row.
            let least = item_style(child)
                .get(Prop::MIN_HEIGHT)
                .is_some_and(|v| v.contains("content"));
            height = if least { area.max(height) } else { area };
        }
        let y = if caption && styles.values[child].get(Prop::CAPTION_SIDE) != Some("bottom") {
            y - gap[1]
        } else {
            y
        };
        let shim = shims.get(index).copied().unwrap_or(0.);
        grid.cells.push((
            child,
            *cell,
            [
                if rtl { extent - x - width } else { x },
                y + (area - height) * fraction.unwrap_or(0.)
                    + if derived[index].baseline == Some(true) {
                        -shim
                    } else {
                        shim
                    },
                width,
                height,
            ],
            derived[index].baseline.is_some(),
        ));
    }
    let top = cells.iter().map(|cell| cell[1]).min();
    if let Some((index, cell)) = cells
        .iter()
        .enumerate()
        .filter(|(_, cell)| Some(cell[1]) == top)
        .min_by_key(|(index, cell)| (derived[*index].baseline != Some(false), cell[0]))
    {
        let (child, _, [_, y, _, height], _) = grid.cells[index];
        grid.first_item = Some(child);
        if let Some(&[first, _]) = baselines.and_then(|baselines| baselines.get(&child)) {
            let area = grid.columns.iter().skip(cell[0]).take(cell[2]).sum::<f32>()
                + cell[2].saturating_sub(1) as f32 * grid.gap[0];
            let [top, _, bottom, _] =
                styles.lengths[child].edges(&styles.values[child], css::properties::MARGIN, area);
            grid.first_baseline = Some(y + top + first.unwrap_or(height - top - bottom));
        }
    }
    grid.cells.sort_unstable_by_key(|(child, ..)| *child);
    if outermost {
        measured.edges = None;
    }
    grid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_and_dotted_area_rows_size_the_grid() {
        let text = renderer::TextSystem::new();
        let mut doc = Document::parse(
            "<style>main{display:grid;grid-template-columns:repeat(4,auto);grid-template-areas:'t f m n''.r a.'}div{display:flex;width:200px}input{width:100%;line-height:24px;padding:0 6px;border:1px solid}</style><main><b style='grid-area:t'></b><div style='grid-area:f'><input></div><div style='grid-area:r'><input></div><b style='grid-area:a'></b></main>",
        );
        let mut state = crate::render::PageState::default();
        let styles = state.restyle(&mut doc, [800., 600.]);
        let main = doc
            .nodes
            .iter()
            .position(|node| node.tag == "main")
            .unwrap();
        let grid = layout(
            &doc,
            main,
            &styles,
            Some(800.),
            None,
            None,
            None,
            &text,
            &mut Default::default(),
        );
        assert_eq!(grid.rows, [26., 26.]);
        let cells: Vec<_> = grid.cells.iter().map(|(_, cell, ..)| *cell).collect();
        assert_eq!(cells[2..], [[1, 1, 1, 1], [2, 1, 1, 1]]);
    }

    #[test]
    fn items_of_one_style_resolve_percentages_against_each_width() {
        let text = renderer::TextSystem::new();
        let mut doc = Document::parse(
            "<style>main{display:grid;grid-template-columns:100px 300px}i{display:block;padding:10% 0 0;height:0}</style><main><i></i><i></i></main>",
        );
        let mut state = crate::render::PageState::default();
        let styles = state.restyle(&mut doc, [800., 600.]);
        let main = doc
            .nodes
            .iter()
            .position(|node| node.tag == "main")
            .unwrap();
        let grid = layout(
            &doc,
            main,
            &styles,
            Some(800.),
            None,
            None,
            None,
            &text,
            &mut Default::default(),
        );
        let heights: Vec<_> = grid.cells.iter().map(|(.., bounds, _)| bounds[3]).collect();
        assert_eq!(heights, [10., 30.]);
        assert_eq!(grid.rows, [30.]);
    }

    #[test]
    fn aspect_ratio_children_give_rows_their_height() {
        let text = renderer::TextSystem::new();
        let mut doc = Document::parse(
            "<style>main{display:grid;grid-template-columns:300px 200px}i{display:block;aspect-ratio:5/4}.b{box-sizing:border-box;padding:10px}.c{overflow:hidden}</style><main><div><i style='width:100%'></i></div><div><i class=b></i><p style='height:20px;margin:0'></p></div></main><main><div><i class=c style='aspect-ratio:10'><p style='height:90px;margin:0'></p></i></div><div><i style='aspect-ratio:10'><p style='height:90px;margin:0'></p></i></div></main>",
        );
        let mut state = crate::render::PageState::default();
        let styles = state.restyle(&mut doc, [800., 600.]);
        let rows: Vec<_> = (0..doc.nodes.len())
            .filter(|&id| doc.nodes[id].tag == "main")
            .map(|main| {
                layout(
                    &doc,
                    main,
                    &styles,
                    Some(800.),
                    None,
                    None,
                    None,
                    &text,
                    &mut Default::default(),
                )
                .rows
            })
            .collect();
        assert_eq!(rows, [vec![240.], vec![90.]]);
    }

    #[test]
    fn dense_packing_fills_holes_and_stays_bounded() {
        let text = renderer::TextSystem::new();
        let place = |html: &str| {
            let mut doc = Document::parse(html);
            let mut state = crate::render::PageState::default();
            let styles = state.restyle(&mut doc, [800., 600.]);
            let grid = doc
                .nodes
                .iter()
                .position(|node| node.tag == "main")
                .unwrap();
            layout(
                &doc,
                grid,
                &styles,
                Some(800.),
                None,
                None,
                None,
                &text,
                &mut Default::default(),
            )
            .cells
            .into_iter()
            .map(|(_, cell, ..)| cell)
            .collect::<Vec<_>>()
        };
        let cells = place(
            "<style>main{display:grid;grid-template-columns:repeat(3,10px);grid-auto-flow:row dense}.w{grid-column:span 2}</style><main><i class=w></i><i class=w></i><i></i><i></i></main>",
        );
        assert_eq!(
            cells,
            [[0, 0, 2, 1], [0, 1, 2, 1], [2, 0, 1, 1], [2, 1, 1, 1]]
        );
        let cells = place(&format!(
            "<style>main{{display:grid;grid-template-columns:repeat(4096,1px);grid-auto-flow:dense}}i{{grid-column:span 4096}}</style><main>{}</main>",
            "<i></i>".repeat(3000)
        ));
        assert!(
            cells
                .iter()
                .enumerate()
                .all(|(row, cell)| *cell == [0, row, 4096, 1])
        );
    }
}
