// Copyright 2020 the Resvg Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Based on http://www.getreuer.info/home/gaussianiir
// Licensed under 'Simplified BSD License'.

use super::ImageRefMut;

struct BlurData {
    width: usize,
    height: usize,
    sigma_x: f64,
    sigma_y: f64,
    steps: usize,
}

pub fn apply(sigma_x: f64, sigma_y: f64, src: ImageRefMut) {
    let buf_size = (src.width * src.height) as usize;
    let mut buf = vec![0.0; buf_size];
    let buf = &mut buf;

    let d = BlurData {
        width: src.width as usize,
        height: src.height as usize,
        sigma_x,
        sigma_y,
        steps: 4,
    };

    let data: &mut [u8] = rgb::bytemuck::cast_slice_mut(src.data);
    gaussian_channel(data, &d, 0, buf);
    gaussian_channel(data, &d, 1, buf);
    gaussian_channel(data, &d, 2, buf);
    gaussian_channel(data, &d, 3, buf);
}

fn gaussian_channel(data: &mut [u8], d: &BlurData, channel: usize, buf: &mut [f64]) {
    for i in 0..data.len() / 4 {
        buf[i] = data[i * 4 + channel] as f64 / 255.0;
    }

    gaussianiir2d(d, buf);

    for i in 0..data.len() / 4 {
        data[i * 4 + channel] = (buf[i] * 255.0) as u8;
    }
}

fn gaussianiir2d(d: &BlurData, buf: &mut [f64]) {
    let (lambda_x, dnu_x) = if d.sigma_x > 0.0 {
        let (lambda, dnu) = gen_coefficients(d.sigma_x, d.steps);

        for y in 0..d.height {
            for _ in 0..d.steps {
                let idx = d.width * y;

                for x in 1..d.width {
                    buf[idx + x] += dnu * buf[idx + x - 1];
                }

                let mut x = d.width - 1;

                while x > 0 {
                    buf[idx + x - 1] += dnu * buf[idx + x];
                    x -= 1;
                }
            }
        }

        (lambda, dnu)
    } else {
        (1.0, 1.0)
    };

    let (lambda_y, dnu_y) = if d.sigma_y > 0.0 {
        let (lambda, dnu) = gen_coefficients(d.sigma_y, d.steps);
        for x in 0..d.width {
            for _ in 0..d.steps {
                let idx = x;

                let mut y = d.width;
                while y < buf.len() {
                    buf[idx + y] += dnu * buf[idx + y - d.width];
                    y += d.width;
                }

                y = buf.len() - d.width;

                while y > 0 {
                    buf[idx + y - d.width] += dnu * buf[idx + y];
                    y -= d.width;
                }
            }
        }

        (lambda, dnu)
    } else {
        (1.0, 1.0)
    };

    let post_scale =
        ((dnu_x * dnu_y).sqrt() / (lambda_x * lambda_y).sqrt()).powi(2 * d.steps as i32);
    buf.iter_mut().for_each(|v| *v *= post_scale);
}

fn gen_coefficients(sigma: f64, steps: usize) -> (f64, f64) {
    let lambda = (sigma * sigma) / (2.0 * steps as f64);
    let dnu = (1.0 + 2.0 * lambda - (1.0 + 4.0 * lambda).sqrt()) / (2.0 * lambda);
    (lambda, dnu)
}
