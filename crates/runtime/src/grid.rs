use crate::{
    html::Document,
    render::{self, ComputedStyles},
};

pub(crate) type Measurements = std::collections::HashMap<(usize, u32, Option<u32>), [f32; 2]>;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Grid {
    pub columns: Vec<f32>,
    pub rows: Vec<f32>,
    pub cells: Vec<(usize, [usize; 4])>,
    pub gap: [f32; 2],
}

impl Grid {
    pub fn bounds(&self, cell: [usize; 4]) -> [f32; 4] {
        let [column, row, columns, rows] = cell;
        [
            self.columns[..column].iter().sum::<f32>() + column as f32 * self.gap[0],
            self.rows[..row].iter().sum::<f32>() + row as f32 * self.gap[1],
            self.columns[column..column + columns].iter().sum::<f32>()
                + columns.saturating_sub(1) as f32 * self.gap[0],
            self.rows[row..row + rows].iter().sum::<f32>()
                + rows.saturating_sub(1) as f32 * self.gap[1],
        ]
    }
}

fn placement(value: Option<&String>, count: usize) -> (Option<usize>, usize) {
    let Some(value) = value else { return (None, 1) };
    let parts: Vec<_> = value.split('/').map(str::trim).collect();
    let line = |value: &str| {
        value.parse::<isize>().ok().filter(|n| *n != 0).map(|n| {
            if n < 0 {
                (count as isize + n + 1).clamp(0, 4095) as usize
            } else {
                (n - 1).min(4095) as usize
            }
        })
    };
    let span = |value: &str| {
        value
            .strip_prefix("span ")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1)
            .clamp(1, 4096)
    };
    let start = line(parts[0]);
    let length = parts
        .get(1)
        .and_then(|v| line(v))
        .zip(start)
        .map(|(end, start)| end.saturating_sub(start).max(1))
        .unwrap_or_else(|| span(parts.get(1).copied().unwrap_or(parts[0])));
    (start, length)
}

fn tracks(value: &str) -> Vec<String> {
    let mut result = Vec::new();
    for part in render::components(value, ' ') {
        if let Some(body) = part
            .strip_prefix("repeat(")
            .and_then(|v| v.strip_suffix(')'))
        {
            if let Some((count, value)) = body.split_once(',') {
                let repeated = tracks(value.trim());
                for _ in 0..count.trim().parse::<usize>().unwrap_or(1).min(4096) {
                    result.extend(repeated.iter().take(4096 - result.len()).cloned());
                    if result.len() == 4096 {
                        break;
                    }
                }
            }
        } else if !matches!(part, "none" | "subgrid") && !part.starts_with('[') {
            result.push(part.into());
        }
    }
    result
}

fn intrinsic(
    doc: &Document,
    id: usize,
    styles: &ComputedStyles,
    width: f32,
    height: Option<f32>,
    text: &gpui::WindowTextSystem,
    measured: &mut Measurements,
) -> [f32; 2] {
    let key = (id, width.to_bits(), height.map(f32::to_bits));
    if let Some(value) = measured.get(&key) {
        return *value;
    }
    let node = &doc.nodes[id];
    let style = &styles.values[id];
    let units = &styles.lengths[id];
    if style.get("display").is_some_and(|v| v == "none") {
        return [0.; 2];
    }
    let padding = units.edges(style, "padding", width);
    let margin = units.edges(style, "margin", width);
    let mut extent = [
        padding[1] + padding[3] + margin[1] + margin[3],
        padding[0] + padding[2] + margin[0] + margin[2],
    ];
    for (index, side) in ["top", "right", "bottom", "left"].iter().enumerate() {
        extent[1 - index % 2] += style
            .get(&format!("border-{side}-width"))
            .or_else(|| style.get("border-width"))
            .and_then(|v| units.resolve(v, width))
            .unwrap_or(0.);
    }
    let mut result = [0f32; 2];
    let child_height = style
        .get("height")
        .filter(|v| height.is_some() || !v.contains('%'))
        .and_then(|v| units.resolve(v, height.unwrap_or(0.)));
    if node.tag == "svg" {
        result = [
            node.attrs
                .get("width")
                .and_then(|v| v.parse().ok())
                .unwrap_or(16.),
            node.attrs
                .get("height")
                .and_then(|v| v.parse().ok())
                .unwrap_or(16.),
        ];
    } else if !node.text.is_empty() {
        let preserve = style
            .get("white-space")
            .is_some_and(|v| matches!(v.as_str(), "pre" | "pre-wrap" | "break-spaces"));
        let content = if preserve {
            node.text.clone()
        } else {
            node.text.split_whitespace().collect::<Vec<_>>().join(" ")
        };
        if !content.is_empty() {
            let run = gpui::TextRun {
                len: content.len(),
                font: render::font(style),
                color: gpui::black(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            for line in content.trim_end_matches('\n').split('\n') {
                let mut run = run.clone();
                run.len = line.len();
                result[0] = result[0].max(f32::from(
                    text.shape_line(
                        line.to_owned().into(),
                        gpui::px(units.font.em),
                        &[run],
                        None,
                    )
                    .width,
                ));
                result[1] += units.font.lh;
            }
        }
    } else if let Some(inline) = render::inline_text(doc, styles, id) {
        result = [
            f32::from(
                text.shape_line(
                    inline.text.to_string().into(),
                    gpui::px(units.font.em),
                    &inline.runs,
                    None,
                )
                .width,
            ),
            units.font.lh,
        ];
    } else if style.get("display").is_some_and(|v| v == "grid")
        && !style
            .get("grid-template-rows")
            .is_some_and(|v| v == "subgrid")
    {
        let grid = layout(doc, id, styles, width, child_height, None, text, measured);
        result = [
            grid.columns.iter().sum::<f32>()
                + grid.columns.len().saturating_sub(1) as f32 * grid.gap[0],
            grid.rows.iter().sum::<f32>() + grid.rows.len().saturating_sub(1) as f32 * grid.gap[1],
        ];
    } else {
        let inline = !node.children.is_empty()
            && node.children.iter().all(|child| {
                matches!(
                    doc.nodes[*child].tag.as_str(),
                    "#text" | "span" | "strong" | "b" | "em" | "i" | "a" | "code" | "small" | "br"
                )
            });
        let row = style
            .get("display")
            .is_some_and(|v| v == "flex" || v == "inline-flex")
            && !style.get("flex-direction").is_some_and(|v| v == "column")
            || inline;
        for &child in &node.children {
            if styles.values[child]
                .get("position")
                .is_some_and(|v| matches!(v.as_str(), "absolute" | "fixed"))
            {
                continue;
            }
            let child = intrinsic(doc, child, styles, width, child_height, text, measured);
            result[usize::from(!row)] += child[usize::from(!row)];
            result[usize::from(row)] = result[usize::from(row)].max(child[usize::from(row)]);
        }
    }
    for (axis, name) in ["width", "height"].into_iter().enumerate() {
        if let Some(value) = style
            .get(name)
            .filter(|v| axis == 0 || height.is_some() || !v.contains('%'))
            .and_then(|v| {
                units.resolve(
                    v,
                    if axis == 0 {
                        width
                    } else {
                        height.unwrap_or(0.)
                    },
                )
            })
        {
            result[axis] = value;
        }
        if let Some(value) = style
            .get(&format!("min-{name}"))
            .filter(|v| axis == 0 || height.is_some() || !v.contains('%'))
            .and_then(|v| {
                units.resolve(
                    v,
                    if axis == 0 {
                        width
                    } else {
                        height.unwrap_or(0.)
                    },
                )
            })
        {
            result[axis] = result[axis].max(value);
        }
        if let Some(value) = style
            .get(&format!("max-{name}"))
            .filter(|v| axis == 0 || height.is_some() || !v.contains('%'))
            .and_then(|v| {
                units.resolve(
                    v,
                    if axis == 0 {
                        width
                    } else {
                        height.unwrap_or(0.)
                    },
                )
            })
        {
            result[axis] = result[axis].min(value);
        }
        result[axis] += extent[axis];
    }
    measured.insert(key, result);
    result
}

pub(crate) fn layout(
    doc: &Document,
    id: usize,
    styles: &ComputedStyles,
    width: f32,
    height: Option<f32>,
    inherited: Option<&Grid>,
    text: &gpui::WindowTextSystem,
    measured: &mut Measurements,
) -> Grid {
    let style = &styles.values[id];
    let units = &styles.lengths[id];
    let names = ["grid-template-columns", "grid-template-rows"];
    let templates = names.map(|name| tracks(style.get(name).map_or("none", String::as_str)));
    let shared =
        names.map(|name| inherited.is_some() && style.get(name).is_some_and(|v| v == "subgrid"));
    let mut grid = Grid {
        columns: vec![
            0.;
            if shared[0] {
                inherited.unwrap().columns.len()
            } else {
                templates[0].len().max(1)
            }
        ],
        rows: vec![
            0.;
            if shared[1] {
                inherited.unwrap().rows.len()
            } else {
                templates[1].len()
            }
        ],
        cells: Vec::new(),
        gap: ["column-gap", "row-gap"].map(|name| {
            style
                .get(name)
                .and_then(|v| units.resolve(v, width))
                .unwrap_or(0.)
        }),
    };
    let mut occupied = std::collections::HashSet::new();
    let mut cursor = [0, 0];
    for &child in &doc.nodes[id].children {
        let child_style = &styles.values[child];
        if child_style.get("display").is_some_and(|v| v == "none")
            || child_style
                .get("position")
                .is_some_and(|v| matches!(v.as_str(), "absolute" | "fixed"))
            || doc.nodes[child].tag == "#text" && doc.nodes[child].text.trim().is_empty()
        {
            continue;
        }
        let (column, columns) = placement(child_style.get("grid-column"), grid.columns.len());
        let (row, rows) = placement(child_style.get("grid-row"), grid.rows.len());
        grid.columns
            .resize(grid.columns.len().max(column.unwrap_or(0) + columns), 0.);
        if style
            .get("grid-auto-flow")
            .is_some_and(|v| v.contains("dense"))
        {
            cursor = [0, 0];
        }
        let mut cell = [
            column.unwrap_or(cursor[0]),
            row.unwrap_or(cursor[1]),
            columns,
            rows,
        ];
        while (column.is_none() || row.is_none())
            && (cell[0]..cell[0] + columns)
                .any(|x| (cell[1]..cell[1] + rows).any(|y| occupied.contains(&(x, y))))
        {
            if column.is_some() {
                cell[1] += 1;
            } else {
                cell[0] += 1;
                if cell[0] + columns > grid.columns.len() {
                    cell[0] = 0;
                    cell[1] += 1;
                }
            }
            if cell[1] > 4096 {
                break;
            }
        }
        for axis in 0..2 {
            if shared[axis] {
                let count = if axis == 0 {
                    inherited.unwrap().columns.len()
                } else {
                    inherited.unwrap().rows.len()
                };
                cell[axis] = cell[axis].min(count.saturating_sub(1));
                cell[axis + 2] = cell[axis + 2].min(count.saturating_sub(cell[axis]));
            }
        }
        grid.rows.resize(grid.rows.len().max(cell[1] + cell[3]), 0.);
        for x in cell[0]..cell[0] + columns {
            for y in cell[1]..cell[1] + rows {
                occupied.insert((x, y));
            }
        }
        cursor = [cell[0] + columns, cell[1]];
        if cursor[0] >= grid.columns.len() {
            cursor = [0, cursor[1] + 1];
        }
        grid.cells.push((child, cell));
    }
    for axis in 0..2 {
        let sizes = if axis == 0 {
            &mut grid.columns
        } else {
            &mut grid.rows
        };
        if shared[axis] {
            *sizes = if axis == 0 {
                inherited.unwrap().columns.clone()
            } else {
                inherited.unwrap().rows.clone()
            };
            continue;
        }
        let mut flexible = vec![0.; sizes.len()];
        let mut automatic = vec![false; sizes.len()];
        let mut fixed = vec![false; sizes.len()];
        for (index, size) in sizes.iter_mut().enumerate() {
            let value = templates[axis].get(index).map_or("auto", String::as_str);
            let minimum = value
                .strip_prefix("minmax(")
                .and_then(|v| v.strip_suffix(')'))
                .and_then(|v| v.split_once(','));
            let maximum = minimum.map_or(value, |(_, max)| max.trim());
            automatic[index] = maximum == "auto";
            if let Some(value) = maximum
                .strip_suffix("fr")
                .and_then(|v| v.parse::<f32>().ok())
            {
                flexible[index] = value.max(0.);
            }
            if let Some(value) = units.resolve(
                minimum.map_or(value, |(min, _)| min.trim()),
                if axis == 0 {
                    width
                } else {
                    height.unwrap_or(0.)
                },
            ) {
                *size = value;
                fixed[index] = flexible[index] == 0.;
            }
        }
        for &(child, cell) in &grid.cells {
            if (cell[axis]..cell[axis] + cell[axis + 2]).all(|index| fixed[index]) {
                continue;
            }
            let child_style = &styles.values[child];
            let mut contributions = vec![(
                cell[axis],
                cell[axis + 2],
                intrinsic(doc, child, styles, width, height, text, measured)[axis],
            )];
            if child_style.get(names[axis]).is_some_and(|v| v == "subgrid") {
                contributions.clear();
                let mut offset = 0;
                for &grandchild in &doc.nodes[child].children {
                    if styles.values[grandchild]
                        .get("display")
                        .is_some_and(|v| v == "none")
                    {
                        continue;
                    }
                    let (_, span) = placement(
                        styles.values[grandchild].get(if axis == 0 {
                            "grid-column"
                        } else {
                            "grid-row"
                        }),
                        cell[axis + 2],
                    );
                    let start = cell[axis] + if axis == 0 { 0 } else { offset };
                    if start + span <= sizes.len() {
                        contributions.push((
                            start,
                            span,
                            intrinsic(doc, grandchild, styles, width, height, text, measured)[axis],
                        ));
                    }
                    if axis == 1 {
                        offset += span;
                    }
                }
            } else if axis == 0
                && (child_style
                    .get("overflow-x")
                    .is_some_and(|v| matches!(v.as_str(), "auto" | "scroll" | "hidden"))
                    || child_style
                        .get("min-width")
                        .is_some_and(|v| v == "0" || v == "0px"))
            {
                contributions[0].2 = 0.;
            }
            for (start, span, contribution) in contributions {
                let current = sizes[start..start + span].iter().sum::<f32>()
                    + span.saturating_sub(1) as f32 * grid.gap[axis];
                let count = (start..start + span).filter(|&i| !fixed[i]).count();
                if contribution > current && count > 0 {
                    for i in start..start + span {
                        if !fixed[i] {
                            sizes[i] += (contribution - current) / count as f32;
                        }
                    }
                }
            }
        }
        if let Some(available) = if axis == 0 { Some(width) } else { height } {
            let free = (available
                - sizes.iter().sum::<f32>()
                - sizes.len().saturating_sub(1) as f32 * grid.gap[axis])
                .max(0.);
            let sum: f32 = flexible.iter().sum();
            if sum > 0. {
                for (size, factor) in sizes.iter_mut().zip(flexible) {
                    *size += free * factor / sum;
                }
            } else if style
                .get(if axis == 0 {
                    "justify-content"
                } else {
                    "align-content"
                })
                .is_none_or(|v| matches!(v.as_str(), "normal" | "stretch"))
            {
                let count = automatic.iter().filter(|value| **value).count();
                if count > 0 {
                    for (size, auto) in sizes.iter_mut().zip(automatic) {
                        if auto {
                            *size += free / count as f32;
                        }
                    }
                }
            }
        }
    }
    grid
}
