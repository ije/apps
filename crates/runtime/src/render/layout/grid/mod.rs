pub(crate) mod intrinsic;
pub(crate) mod layout;
pub(crate) mod track;

use crate::css;
use crate::css::properties::Prop;
use crate::css::value::units::Lengths;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Grid {
    pub columns: Vec<f32>,
    pub rows: Vec<f32>,
    pub cells: Vec<(usize, [usize; 4], [f32; 4], bool)>,
    pub gap: [f32; 2],
    pub baseline: bool,
    pub first_item: Option<usize>,
    pub first_baseline: Option<f32>,
    // The height of the content when it is not the sum of the rows: flexible rows that sum to less than `1fr`
    // size again in the height that they first gave the grid.
    pub height: Option<f32>,
}

impl Grid {
    /// The size of the tracks with their gaps in an axis, or the height of the content.
    pub(crate) fn extent(&self, axis: usize) -> f32 {
        let tracks = [&self.columns, &self.rows][axis];
        self.height.filter(|_| axis == 1).unwrap_or_else(|| {
            tracks.iter().sum::<f32>() + tracks.len().saturating_sub(1) as f32 * self.gap[axis]
        })
    }

    /// The width and height of the grid area of a cell placement: column, row, columns, rows.
    pub(crate) fn area(&self, place: [usize; 4]) -> [f32; 2] {
        [0, 1].map(|axis| {
            let span = place[axis + 2];
            [&self.columns, &self.rows][axis]
                .get(place[axis]..place[axis] + span)
                .map_or(0., |tracks| tracks.iter().sum())
                + self.gap[axis] * span.saturating_sub(1) as f32
        })
    }
}

// The content size of a box that skips its contents: its `contain-intrinsic-size`, or the size it remembers for
// `auto`.
pub(crate) fn intrinsic_size(
    style: &css::properties::Style,
    units: &Lengths,
    remembered: Option<[f32; 2]>,
) -> [f32; 2] {
    [0, 1].map(|axis| {
        let value = style
            .get(
                [
                    Prop::CONTAIN_INTRINSIC_WIDTH,
                    Prop::CONTAIN_INTRINSIC_HEIGHT,
                ][axis],
            )
            .unwrap_or("none");
        match value.strip_prefix("auto") {
            Some(length) => remembered
                .map(|size| size[axis])
                .or_else(|| units.resolve(length.trim(), 0.)),
            None => units.resolve(value, 0.),
        }
        .unwrap_or(0.)
    })
}

pub(crate) type Baselines = rustc_hash::FxHashMap<usize, [Option<f32>; 2]>;
