use super::*;
use crate::geometry::before_viewer;

// How far a point can be from a plane that holds it, in device pixels.
const FLAT: f64 = 0.05;
// The most pieces that the planes of a context split into.
const PIECES: usize = 256;
// The most planes that a context sorts by their sides.
const PLANES: usize = 512;

// A plane of a 3D rendering context, or a part of one.
struct Piece {
    layer: usize,
    drawn: bool,
    // The part of the layer that draws, in its own pixels.
    area: Rect<f32>,
    cut: bool,
    // The corners in the window, before the viewer, and their bounds.
    corners: Vec<[f64; 4]>,
    reach: [f64; 4],
    // `matrix` takes points of the layer to (x, y, z, w), and `front` is the distance of a point before the plane.
    matrix: [f64; 16],
    front: [f64; 4],
}

fn dot(a: &[f64; 4], [x, y, z, w]: [f64; 4]) -> f64 {
    a[0] * x + a[1] * y + a[2] * z + a[3] * w
}

impl Piece {
    // `None` for a plane on its edge.
    fn new(layer: usize, area: Rect<f32>, cut: bool, matrix: [f64; 16]) -> Option<Self> {
        let column = |at: usize| -> [f64; 4] { std::array::from_fn(|row| matrix[at * 4 + row]) };
        let (a, b, c) = (column(0), column(1), column(3));
        // The plane through the points and directions that the layer spans: a product of three columns.
        let minor = |i: usize, j: usize, k: usize| {
            a[i] * (b[j] * c[k] - b[k] * c[j]) - a[j] * (b[i] * c[k] - b[k] * c[i])
                + a[k] * (b[i] * c[j] - b[j] * c[i])
        };
        let mut front = [
            minor(1, 2, 3),
            -minor(0, 2, 3),
            minor(0, 1, 3),
            -minor(0, 1, 2),
        ];
        let length = (front[..3].iter().map(|n| n * n).sum::<f64>()).sqrt();
        if length.is_nan() || front[2].abs() <= length * 1e-6 {
            return None;
        }
        front = front.map(|n| n / length * front[2].signum());
        let [x0, y0] = [area.left(), area.top()].map(f64::from);
        let [x1, y1] = [area.right(), area.bottom()].map(f64::from);
        let points = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
            .map(|[x, y]| std::array::from_fn::<f64, 4, _>(|row| a[row] * x + b[row] * y + c[row]));
        let corners = before_viewer(&points);
        let reach = corners.iter().fold(
            [f64::MAX, f64::MAX, f64::MIN, f64::MIN],
            |[left, top, right, bottom], &[x, y, ..]| {
                [left.min(x), top.min(y), right.max(x), bottom.max(y)]
            },
        );
        (!corners.is_empty()).then_some(Self {
            layer,
            drawn: true,
            area,
            cut,
            corners,
            reach,
            matrix,
            front,
        })
    }

    // The place of the center before the viewer.
    fn depth(&self) -> f64 {
        self.corners.iter().map(|corner| corner[2]).sum::<f64>() / self.corners.len() as f64
    }

    // The least and the most distance of the corners of `other` before this plane.
    fn sides(&self, other: &Piece) -> [f64; 2] {
        (other.corners.iter())
            .map(|&corner| dot(&self.front, corner))
            .fold([f64::MAX, f64::MIN], |[low, high], side| {
                [low.min(side), high.max(side)]
            })
    }

    fn overlaps(&self, other: &Piece) -> bool {
        let (a, b) = (&self.reach, &other.reach);
        if a[0] >= b[2] - FLAT || b[0] >= a[2] - FLAT || a[1] >= b[3] - FLAT || b[1] >= a[3] - FLAT
        {
            return false;
        }
        // Convex shapes overlap when no side of one parts them.
        ![(self, other), (other, self)].iter().any(|(one, two)| {
            let turn = one.turn();
            (0..one.corners.len()).any(|at| {
                let (p, q) = (one.corners[at], one.corners[(at + 1) % one.corners.len()]);
                let [dx, dy] = [q[0] - p[0], q[1] - p[1]];
                let length = dx.hypot(dy);
                length > 1e-6
                    && two
                        .corners
                        .iter()
                        .all(|r| (dx * (r[1] - p[1]) - dy * (r[0] - p[0])) * turn <= FLAT * length)
            })
        })
    }

    // The sign of the area of the corners.
    fn turn(&self) -> f64 {
        let next = self.corners.iter().cycle().skip(1);
        (self.corners.iter().zip(next))
            .map(|(p, q)| p[0] * q[1] - p[1] * q[0])
            .sum::<f64>()
            .signum()
    }

    // The two parts of this piece on the sides of another plane, when that is a straight cut along an axis.
    fn split(&self, plane: &Piece) -> Option<[Piece; 2]> {
        // The distance before the plane is linear in the places of the layer.
        let [a, b, c] = [0, 1, 3].map(|at| {
            dot(
                &plane.front,
                std::array::from_fn(|row| self.matrix[at * 4 + row]),
            )
        });
        let area = self.area;
        let [width, height] = [area.size.width, area.size.height].map(f64::from);
        let (first, second) = if (b * height).abs() < 0.5 * a.abs() {
            let at = (-(c + b * f64::from(area.center().y)) / a) as f32;
            (
                Rect::between(area.origin, point(at, area.bottom())),
                Rect::between(point(at, area.top()), area.bottom_right()),
            )
        } else if (a * width).abs() < 0.5 * b.abs() {
            let at = (-(c + a * f64::from(area.center().x)) / b) as f32;
            (
                Rect::between(area.origin, point(area.right(), at)),
                Rect::between(point(area.left(), at), area.bottom_right()),
            )
        } else {
            return None;
        };
        let part = |area: Rect<f32>| {
            let whole = area.size.width > 0.01 && area.size.height > 0.01;
            (whole.then(|| Piece::new(self.layer, area, true, self.matrix))).flatten()
        };
        Some([part(first)?, part(second)?])
    }
}

enum Order {
    Before,
    After,
    Through,
}

// Which of two pieces draws first, when they overlap. Pieces in one plane keep their paint order.
fn order(a: &Piece, b: &Piece) -> Option<Order> {
    if a.layer == b.layer || !a.overlaps(b) {
        return None;
    }
    let [low, high] = a.sides(b);
    if low >= -FLAT {
        return Some(Order::Before);
    }
    if high <= FLAT {
        return Some(Order::After);
    }
    let [low, high] = b.sides(a);
    Some(if low >= -FLAT {
        Order::After
    } else if high <= FLAT {
        Order::Before
    } else {
        Order::Through
    })
}

impl Scene {
    // Draws the planes after `start` in the current layer from back to front, and returns their layers in this
    // order, with the planes of `listed` that draw nothing: layers and boxes. Planes that go through each other
    // split where that is a cut along an axis of one of them.
    pub(crate) fn sort_planes(
        &mut self,
        start: usize,
        listed: impl Iterator<Item = (usize, Rect<f32>)>,
    ) -> Vec<usize> {
        let parent = self.active.last().copied().unwrap_or(0);
        let planes = self.layers[parent].nested.split_off(start);
        let find = |layer: usize| planes.iter().find(|nested| nested.layer == layer);
        let blank = listed.filter(|(layer, _)| find(*layer).is_none());
        let mut all: Vec<_> = (planes.iter())
            .map(|nested| (nested.layer, None))
            .chain(blank.map(|(layer, area)| (layer, Some(area))))
            .collect();
        all.sort_unstable_by_key(|plane| plane.0);
        // A plane without a piece is on its edge, and draws nothing.
        let mut pieces: Vec<Piece> = (all.into_iter())
            .filter_map(|(index, blank)| {
                let layer = &self.layers[index];
                let matrix = layer.effect.as_ref()?.transform?.map(f64::from);
                let area = blank.or(layer.bounds)?;
                let mut piece = Piece::new(index, area, false, matrix)?;
                piece.drawn = blank.is_none();
                Some(piece)
            })
            .collect();
        // Frames patch keyed layers, and not their copies.
        let keyed = planes.first().is_some_and(|first| {
            (self.keyed.last()).is_some_and(|&(_, layer)| layer >= first.layer)
        });
        // WARNING: the order of all pairs takes time and memory for each pair, so many planes sort by their centers.
        let many = pieces.len() > PLANES;
        let mut edges: Vec<[usize; 2]> = Vec::new();
        let mut at = 0;
        'pieces: while !many && at < pieces.len() {
            let end = at;
            for other in 0..end {
                let edge = match order(&pieces[other], &pieces[end]) {
                    None => continue,
                    Some(Order::Before) => [other, at],
                    Some(Order::After) => [at, other],
                    Some(Order::Through) => {
                        let through = pieces[other].drawn && pieces[at].drawn;
                        let split = (through && !keyed && pieces.len() < PIECES)
                            .then(|| {
                                [(at, other), (other, at)]
                                    .into_iter()
                                    .find_map(|(one, two)| {
                                        (self.splits(pieces[one].layer))
                                            .then(|| pieces[one].split(&pieces[two]))
                                            .flatten()
                                            .map(|parts| (one, parts))
                                    })
                            })
                            .flatten();
                        if let Some((one, [first, second])) = split {
                            // The new pieces meet the others again.
                            pieces[one] = first;
                            pieces.push(second);
                            edges.retain(|edge| edge.iter().all(|&piece| piece < one));
                            at = one;
                            continue 'pieces;
                        }
                        match pieces[other].depth() <= pieces[at].depth() {
                            true => [other, at],
                            false => [at, other],
                        }
                    }
                };
                edges.push(edge);
            }
            at += 1;
        }
        let mut waits = vec![0u32; pieces.len()];
        let mut after = vec![Vec::new(); pieces.len()];
        for [first, then] in edges {
            waits[then] += 1;
            after[first].push(then);
        }
        // Of the pieces that can draw now, the first in paint order draws. In a ring, the first of all does.
        let mut left: Vec<usize> = (0..pieces.len()).collect();
        if many {
            left.sort_by(|&a, &b| pieces[a].depth().total_cmp(&pieces[b].depth()));
        }
        let mut drawn = Vec::with_capacity(pieces.len());
        if many {
            drawn.append(&mut left);
        }
        while !left.is_empty() {
            let free = left
                .iter()
                .position(|&piece| waits[piece] == 0)
                .unwrap_or(0);
            let piece = left.remove(free);
            for &then in &after[piece] {
                waits[then] = waits[then].saturating_sub(1);
            }
            drawn.push(piece);
        }
        let mut layers = Vec::with_capacity(drawn.len());
        for piece in drawn {
            let Piece {
                layer, area, cut, ..
            } = pieces[piece];
            layers.push(layer);
            let Some(mut nested) = find(layer).copied() else {
                continue;
            };
            if cut {
                if layers[..layers.len() - 1].contains(&layer) {
                    nested.layer = self.copy_layer(layer);
                }
                // Parts reach over the cut, so no line shows between them.
                let whole = self.layers[nested.layer].bounds.unwrap_or(area);
                let edge = |part: f32, whole: f32, by: f32| match part == whole {
                    true => part + by,
                    false => part + by.signum() * 0.5,
                };
                let clip = Rect::between(
                    point(
                        edge(area.left(), whole.left(), -4.),
                        edge(area.top(), whole.top(), -4.),
                    ),
                    point(
                        edge(area.right(), whole.right(), 4.),
                        edge(area.bottom(), whole.bottom(), 4.),
                    ),
                );
                if let Some(effect) = &mut self.layers[nested.layer].effect {
                    effect.clip = Some((clip, Radii::default()));
                }
            }
            self.layers[parent].nested.push(nested);
        }
        layers
    }

    // Whether a plane can draw in parts: its effects keep to its content.
    fn splits(&self, layer: usize) -> bool {
        (self.layers[layer].effect.as_ref()).is_some_and(|effect| {
            effect.clip.is_none()
                && effect.blur == 0.
                && effect.shadow.is_none()
                && effect.backdrop.is_none()
                && effect.glass.is_none()
                && effect.text_mask.is_none()
                && effect.blend_mode == BlendMode::Normal
        })
    }

    fn copy_layer(&mut self, from: usize) -> usize {
        let mut layer = self.spare.pop().unwrap_or_default();
        let index = self.layers.len();
        let source = &self.layers[from];
        layer.instances.extend_from_slice(&source.instances);
        layer.nested.extend_from_slice(&source.nested);
        layer.effect = source.effect.clone();
        layer.bounds = source.bounds;
        self.layers.push(layer);
        for at in 0..self.layers[index].nested.len() {
            let inner = self.copy_layer(self.layers[index].nested[at].layer);
            self.layers[index].nested[at].layer = inner;
        }
        index
    }
}
