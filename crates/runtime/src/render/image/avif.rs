use image::{ImageError, ImageFormat, ImageResult, Limits, RgbaImage};
use rav1d::{PlanarImageComponent as Plane, pixel::MatrixCoefficients as Matrix};
use std::io::Cursor;

pub fn decode_avif(bytes: &[u8], limits: Limits) -> ImageResult<RgbaImage> {
    // WARNING: rav1d builds large frame contexts on the stack, which overflows small worker stacks.
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("avif".into())
            .stack_size(8 << 20)
            .spawn_scoped(scope, || decode(bytes, limits))
            .map_err(ImageError::IoError)?
            .join()
            .unwrap_or_else(|_| {
                Err(ImageError::Decoding(image::error::DecodingError::new(
                    ImageFormat::Avif.into(),
                    "AVIF decoder panicked",
                )))
            })
    })
}

fn decode(bytes: &[u8], mut limits: Limits) -> ImageResult<RgbaImage> {
    let error = |message: String| {
        ImageError::Decoding(image::error::DecodingError::new(
            ImageFormat::Avif.into(),
            message,
        ))
    };
    let data = avif_parse::read_avif(&mut Cursor::new(bytes)).map_err(|e| error(e.to_string()))?;
    limits.check_support(&image::LimitSupport::default())?;
    let mut pictures = Vec::with_capacity(2);
    for item in std::iter::once(data.primary_item.as_slice()).chain(data.alpha_item.as_deref()) {
        let metadata =
            avif_parse::AV1Metadata::parse_av1_bitstream(item).map_err(|e| error(e.to_string()))?;
        let width = metadata.max_frame_width.get();
        let height = metadata.max_frame_height.get();
        limits.check_dimensions(width, height)?;
        limits.reserve(u64::from(width) * u64::from(height) * 16)?;
        let mut settings = rav1d::Settings::new();
        settings.set_n_threads(2);
        settings.set_max_frame_delay(1);
        settings.set_frame_size_limit(width.saturating_mul(height));
        let mut decoder =
            rav1d::Decoder::with_settings(&settings).map_err(|e| error(e.to_string()))?;
        let mut sent = decoder.send_data(item.into(), None, None, None);
        let mut picture = None;
        loop {
            match sent {
                Ok(()) | Err(rav1d::Rav1dError::TryAgain) => {}
                Err(e) => return Err(error(e.to_string())),
            }
            match decoder.get_picture() {
                Ok(next) => picture = Some(next),
                Err(rav1d::Rav1dError::TryAgain) if sent.is_ok() => break,
                Err(e) => return Err(error(e.to_string())),
            }
            if sent.is_err() {
                sent = decoder.send_pending_data();
            }
        }
        pictures.push(picture.ok_or_else(|| error("Missing AVIF frame".into()))?);
    }
    let color = &pictures[0];
    let alpha = pictures.get(1);
    let width = color.width();
    let height = color.height();
    if alpha.is_some_and(|p| p.width() != width || p.height() != height) {
        return Err(error("Invalid AVIF alpha dimensions".into()));
    }
    let matrix = color.matrix_coefficients();
    let (kr, kb) = match matrix {
        Matrix::BT709 => (0.2126, 0.0722),
        Matrix::BT470M => (0.30, 0.11),
        Matrix::ST240M => (0.212, 0.087),
        Matrix::BT2020NonConstantLuminance => (0.2627, 0.0593),
        Matrix::Identity
        | Matrix::YCgCo
        | Matrix::Unspecified
        | Matrix::BT470BG
        | Matrix::ST170M => (0.299, 0.114),
        _ => return Err(error("Unsupported AVIF color matrix".into())),
    };
    let mut output = image::RgbaImage::new(width, height);
    for (x, y, pixel) in output.enumerate_pixels_mut() {
        let luma = sample(color, Plane::Y, x, y, false);
        let rgb = if color.pixel_layout() == rav1d::PixelLayout::I400 {
            [luma; 3]
        } else {
            let (sx, sy) = match color.pixel_layout() {
                rav1d::PixelLayout::I420 => (1, 1),
                rav1d::PixelLayout::I422 => (1, 0),
                _ => (0, 0),
            };
            let identity = matrix == Matrix::Identity;
            let u = sample(color, Plane::U, x >> sx, y >> sy, !identity);
            let v = sample(color, Plane::V, x >> sx, y >> sy, !identity);
            if identity {
                [v, luma, u]
            } else if matrix == Matrix::YCgCo {
                [luma - u + v, luma + u, luma - u - v]
            } else {
                let r = luma + 2. * (1. - kr) * v;
                let b = luma + 2. * (1. - kb) * u;
                [r, (luma - kr * r - kb * b) / (1. - kr - kb), b]
            }
        };
        let a = alpha.map_or(1., |p| sample(p, Plane::Y, x, y, false).clamp(0., 1.));
        for (dst, value) in pixel.0[..3].iter_mut().zip(rgb) {
            let value = if data.premultiplied_alpha && a > 0. {
                value / a
            } else {
                value
            };
            *dst = (value.clamp(0., 1.) * 255.).round() as u8;
        }
        pixel.0[3] = (a * 255.).round() as u8;
    }
    Ok(output)
}

fn sample(picture: &rav1d::Picture, plane: Plane, x: u32, y: u32, chroma: bool) -> f32 {
    let depth = picture.bits_per_component().map_or(8, |depth| depth.0);
    let bytes = if depth > 8 { 2 } else { 1 };
    let index = (y * picture.stride(plane) + x * bytes) as usize;
    let data = picture.plane(plane);
    let value = if bytes == 2 {
        u16::from_ne_bytes([data[index], data[index + 1]])
    } else {
        u16::from(data[index])
    } as f32;
    let scale = (1u32 << (depth - 8)) as f32;
    let limited = picture.color_range() == rav1d::pixel::YUVRange::Limited;
    if chroma {
        (value - 128. * scale)
            / if limited {
                224. * scale
            } else {
                ((1u32 << depth) - 1) as f32
            }
    } else if limited {
        (value - 16. * scale) / (219. * scale)
    } else {
        value / ((1u32 << depth) - 1) as f32
    }
}
