// Copyright 2020 the Resvg Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use super::{ImageRefMut, f32_bound};
use usvg::filter::{ComponentTransfer, TransferFunction};

pub fn apply(fe: &ComponentTransfer, src: ImageRefMut) {
    let tables = [fe.func_r(), fe.func_g(), fe.func_b(), fe.func_a()].map(|func| {
        std::array::from_fn::<u8, 256, _>(|c| match is_dummy(func) {
            true => c as u8,
            false => transfer(func, c as u8),
        })
    });
    for pixel in src.data {
        pixel.r = tables[0][pixel.r as usize];
        pixel.g = tables[1][pixel.g as usize];
        pixel.b = tables[2][pixel.b as usize];
        pixel.a = tables[3][pixel.a as usize];
    }
}

fn is_dummy(func: &TransferFunction) -> bool {
    match func {
        TransferFunction::Identity => true,
        TransferFunction::Table(values) => values.is_empty(),
        TransferFunction::Discrete(values) => values.is_empty(),
        TransferFunction::Linear { .. } => false,
        TransferFunction::Gamma { .. } => false,
    }
}

fn transfer(func: &TransferFunction, c: u8) -> u8 {
    let c = c as f32 / 255.0;
    let c = match func {
        TransferFunction::Identity => c,
        TransferFunction::Table(values) => {
            let n = values.len() - 1;
            let k = (c * (n as f32)).floor() as usize;
            let k = std::cmp::min(k, n);
            if k == n {
                values[k]
            } else {
                let vk = values[k];
                let vk1 = values[k + 1];
                let k = k as f32;
                let n = n as f32;
                vk + (c - k / n) * n * (vk1 - vk)
            }
        }
        TransferFunction::Discrete(values) => {
            let n = values.len();
            let k = (c * (n as f32)).floor() as usize;
            values[std::cmp::min(k, n - 1)]
        }
        TransferFunction::Linear { slope, intercept } => slope * c + intercept,
        TransferFunction::Gamma {
            amplitude,
            exponent,
            offset,
        } => amplitude * c.powf(*exponent) + offset,
    };

    (f32_bound(0.0, c, 1.0) * 255.0) as u8
}
