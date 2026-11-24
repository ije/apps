// Copyright 2020 the Resvg Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use super::ImageRefMut;
use rgb::RGBA8;
use usvg::filter::MorphologyOperator;

pub fn apply(operator: MorphologyOperator, rx: f32, ry: f32, src: ImageRefMut) {
    let columns = std::cmp::min(rx.ceil() as u32 * 2, src.width) as usize;
    let rows = std::cmp::min(ry.ceil() as u32 * 2, src.height) as usize;
    let (width, height) = (src.width as usize, src.height as usize);
    let erode = operator == MorphologyOperator::Erode;
    if columns == 0 || rows == 0 {
        let fill = if erode { 255 } else { 0 };
        src.data.fill(RGBA8::new(fill, fill, fill, fill));
        return;
    }

    // Keep resvg's rectangular min/max result. Separable van Herk/Gil-Werman passes make cost independent of
    // radius.
    let (mut g, mut h) = (Vec::new(), Vec::new());
    for y in 0..height {
        pass(
            src.data,
            y * width,
            width,
            1,
            1,
            columns,
            erode,
            &mut g,
            &mut h,
        );
    }
    for x in (0..width).step_by(128) {
        let lanes = (width - x).min(128);
        pass(
            src.data, x, height, width, lanes, rows, erode, &mut g, &mut h,
        );
    }
}

// Item `i` is the `lanes` pixels at `offset + i * stride`. It takes the min or max of the items
// `i - k / 2 .. i - k / 2 + k` that are in `0..n`.
fn pass(
    data: &mut [RGBA8],
    offset: usize,
    n: usize,
    stride: usize,
    lanes: usize,
    k: usize,
    erode: bool,
    g: &mut Vec<RGBA8>,
    h: &mut Vec<RGBA8>,
) {
    let before = k / 2;
    let blocks = (n + k - 1).div_ceil(k);
    let identity = if erode {
        RGBA8::new(255, 255, 255, 255)
    } else {
        RGBA8::default()
    };
    let op = |a: RGBA8, b: RGBA8| {
        if erode {
            RGBA8::new(a.r.min(b.r), a.g.min(b.g), a.b.min(b.b), a.a.min(b.a))
        } else {
            RGBA8::new(a.r.max(b.r), a.g.max(b.g), a.b.max(b.b), a.a.max(b.a))
        }
    };
    g.clear();
    g.resize(blocks * k * lanes, identity);
    for i in 0..n {
        g[(i + before) * lanes..][..lanes].copy_from_slice(&data[offset + i * stride..][..lanes]);
    }
    h.clear();
    h.extend_from_slice(g);
    for block in 0..blocks {
        let start = block * k;
        for t in start + 1..start + k {
            let (done, rest) = g.split_at_mut(t * lanes);
            for (out, prev) in rest[..lanes].iter_mut().zip(&done[(t - 1) * lanes..]) {
                *out = op(*out, *prev);
            }
        }
        for t in (start..start + k - 1).rev() {
            let (rest, done) = h.split_at_mut((t + 1) * lanes);
            for (out, next) in rest[t * lanes..].iter_mut().zip(&done[..lanes]) {
                *out = op(*out, *next);
            }
        }
    }
    for i in 0..n {
        let out = &mut data[offset + i * stride..][..lanes];
        let left = &h[i * lanes..][..lanes];
        let right = &g[(i + k - 1) * lanes..][..lanes];
        for ((out, a), b) in out.iter_mut().zip(left).zip(right) {
            *out = op(*a, *b);
        }
    }
}
