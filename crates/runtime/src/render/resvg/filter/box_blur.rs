// Copyright 2020 the Resvg Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Based on https://github.com/fschutt/fastblur

use super::ImageRefMut;

const STEPS: usize = 5;

pub fn apply(sigma_x: f64, sigma_y: f64, src: ImageRefMut) {
    let boxes_horz = create_box_gauss(sigma_x as f32);
    let boxes_vert = create_box_gauss(sigma_y as f32);
    let width = src.width as usize;
    let stride = width * 4;
    let data: &mut [u8] = rgb::bytemuck::cast_slice_mut(src.data);
    let mut back = vec![0; data.len()];
    let mut sums = vec![0; stride];
    let zero = vec![0; stride];
    let mut prefix = Vec::new();

    // WARNING: each pass writes `round(sum * 1 / (2r + 1))` of the window with zero padding, as fastblur does. The
    // vertical pass keeps one sum per column and walks rows in memory order, and the horizontal one takes window
    // sums from prefix sums, so both loops vectorize.
    for (box_size_horz, box_size_vert) in boxes_horz.iter().zip(boxes_vert.iter()) {
        let radius = ((box_size_vert - 1) / 2) as usize;
        if radius == 0 {
            back.copy_from_slice(data);
        } else {
            let iarr = 1.0 / (radius + radius + 1) as f32;
            sums.fill(0);
            for row in data.chunks_exact(stride).take(radius) {
                for (sum, &c) in sums.iter_mut().zip(row) {
                    *sum += c as i32;
                }
            }
            for (y, out) in back.chunks_exact_mut(stride).enumerate() {
                let add = data
                    .get((y + radius) * stride..(y + radius + 1) * stride)
                    .unwrap_or(&zero);
                let sub = match y.checked_sub(radius) {
                    Some(y) => &data[y * stride..][..stride],
                    None => &zero,
                };
                for (((out, sum), &add), &sub) in out.iter_mut().zip(&mut sums).zip(add).zip(sub) {
                    *sum += add as i32;
                    *out = round(*sum as f32 * iarr) as u8;
                    *sum -= sub as i32;
                }
            }
        }

        let radius = ((box_size_horz - 1) / 2) as usize;
        if radius == 0 {
            data.copy_from_slice(&back);
            continue;
        }
        let iarr = 1.0 / (radius + radius + 1) as f32;
        // `prefix[i]` sums the pixels before `clamp(i - span, 0, width)`, so the window of `x` is
        // `prefix[x + 2 * span + 1] - prefix[x]`. A radius past the width sums the whole row either way.
        let span = radius.min(width);
        prefix.resize((width + 2 * span + 1) * 4, 0);
        for (from, to) in back.chunks_exact(stride).zip(data.chunks_exact_mut(stride)) {
            let mut sum = [0i32; 4];
            prefix[..(span + 1) * 4].fill(0);
            for (pixel, out) in from
                .as_chunks::<4>()
                .0
                .iter()
                .zip(prefix[(span + 1) * 4..].as_chunks_mut::<4>().0)
            {
                for ((sum, &c), out) in sum.iter_mut().zip(pixel).zip(out) {
                    *sum += c as i32;
                    *out = *sum;
                }
            }
            for out in prefix[(span + 1 + width) * 4..].as_chunks_mut::<4>().0 {
                *out = sum;
            }
            let (high, low) = (&prefix[(2 * span + 1) * 4..][..stride], &prefix[..stride]);
            for ((out, &high), &low) in to.iter_mut().zip(high).zip(low) {
                *out = round((high - low) as f32 * iarr) as u8;
            }
        }
    }
}

#[inline(never)]
fn create_box_gauss(sigma: f32) -> [i32; STEPS] {
    if sigma > 0.0 {
        let n_float = STEPS as f32;

        let w_ideal = (12.0 * sigma * sigma / n_float).sqrt() + 1.0;
        let mut wl = w_ideal.floor() as i32;
        if wl % 2 == 0 {
            wl -= 1;
        }

        let wu = wl + 2;

        let wl_float = wl as f32;
        let m_ideal = (12.0 * sigma * sigma
            - n_float * wl_float * wl_float
            - 4.0 * n_float * wl_float
            - 3.0 * n_float)
            / (-4.0 * wl_float - 4.0);
        let m = m_ideal.round() as usize;

        let mut sizes = [0; STEPS];
        for (i, size) in sizes.iter_mut().enumerate() {
            *size = if i < m { wl } else { wu };
        }

        sizes
    } else {
        [1; STEPS]
    }
}

// WARNING: exact only for x <= 2^23, but much faster than `f32::round`.
#[inline]
fn round(mut x: f32) -> f32 {
    x += 12582912.0;
    x -= 12582912.0;
    x
}
