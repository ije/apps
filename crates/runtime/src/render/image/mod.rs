#[cfg(not(target_os = "macos"))]
pub mod avif;

use anyhow::Context;
use std::io::Cursor;

fn fits(width: usize, height: usize) -> bool {
    width <= 16384 && height <= 16384 && width * height <= 64 << 20
}

fn bgra(mut pixels: Vec<u8>, channels: usize) -> Vec<u8> {
    if channels == 4 {
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        return pixels;
    }
    let mut out = vec![255; pixels.len() / channels * 4];
    let out_pixels = out.as_chunks_mut::<4>().0.iter_mut();
    match channels {
        3 => {
            for (out, &[r, g, b]) in out_pixels.zip(pixels.as_chunks::<3>().0) {
                *out = [b, g, r, 255];
            }
        }
        2 => {
            for (out, &[luma, alpha]) in out_pixels.zip(pixels.as_chunks::<2>().0) {
                *out = [luma, luma, luma, alpha];
            }
        }
        _ => {
            for (out, &luma) in out_pixels.zip(&pixels) {
                *out = [luma, luma, luma, 255];
            }
        }
    }
    out
}

pub fn decode(bytes: &[u8]) -> anyhow::Result<renderer::RenderImage> {
    let (pixels, width, height) = pixels(bytes)?;
    Ok(renderer::RenderImage::new(
        width as u32,
        height as u32,
        pixels,
    ))
}

pub(crate) fn pixels(bytes: &[u8]) -> anyhow::Result<(Vec<u8>, usize, usize)> {
    Ok(match bytes {
        [0xFF, 0xD8, 0xFF, ..] => {
            use zune_jpeg::JpegDecoder;
            use zune_jpeg::zune_core::{colorspace::ColorSpace, options::DecoderOptions};
            let mut decoder = JpegDecoder::new(Cursor::new(bytes));
            decoder.decode_headers()?;
            let (width, height) = decoder.dimensions().context("Invalid JPEG")?;
            anyhow::ensure!(fits(width, height), "Image is too large");
            // zune-jpeg converts only YCbCr to BGRA, and picks the converter when it reads the headers.
            let ycbcr = decoder.input_colorspace() == Some(ColorSpace::YCbCr);
            if ycbcr {
                decoder = JpegDecoder::new_with_options(
                    Cursor::new(bytes),
                    DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::BGRA),
                );
            }
            let pixels = decoder.decode()?;
            (if ycbcr { pixels } else { bgra(pixels, 3) }, width, height)
        }
        [0x89, b'P', b'N', b'G', ..] => {
            let mut decoder = png::Decoder::new(Cursor::new(bytes));
            decoder.set_transformations(png::Transformations::normalize_to_color8());
            let mut reader = decoder.read_info()?;
            let (width, height) = reader.info().size();
            let (width, height) = (width as usize, height as usize);
            anyhow::ensure!(fits(width, height), "Image is too large");
            let mut pixels = vec![0; reader.output_buffer_size().context("Image is too large")?];
            let frame = reader.next_frame(&mut pixels)?;
            pixels.truncate(frame.buffer_size());
            (bgra(pixels, frame.color_type.samples()), width, height)
        }
        [b'G', b'I', b'F', b'8', ..] => {
            let mut options = gif::DecodeOptions::new();
            options.set_color_output(gif::ColorOutput::RGBA);
            options.set_memory_limit(gif::MemoryLimit::Bytes(
                const { std::num::NonZeroU64::new(256 << 20).unwrap() },
            ));
            let mut decoder = options.read_info(bytes)?;
            let (width, height) = (decoder.width() as usize, decoder.height() as usize);
            anyhow::ensure!(fits(width, height), "Image is too large");
            let frame = decoder.read_next_frame()?.context("Empty GIF")?;
            let mut pixels = vec![0; width * height * 4];
            let left = (frame.left as usize).min(width);
            let (top, columns) = (frame.top as usize, frame.width as usize);
            let row = columns.min(width - left) * 4;
            for y in 0..(frame.height as usize).min(height.saturating_sub(top)) {
                let at = ((top + y) * width + left) * 4;
                pixels[at..at + row].copy_from_slice(&frame.buffer[y * columns * 4..][..row]);
            }
            (bgra(pixels, 4), width, height)
        }
        #[cfg(target_os = "macos")]
        _ => {
            type Image = (Vec<u8>, [usize; 2]);
            #[allow(improper_ctypes)]
            unsafe extern "C" {
                fn apps_image_decode(
                    bytes: *const u8,
                    length: usize,
                    allocate: extern "C" fn(&mut Image, usize, usize) -> *mut u8,
                    image: &mut Image,
                ) -> bool;
            }
            extern "C" fn allocate(image: &mut Image, width: usize, height: usize) -> *mut u8 {
                if !fits(width, height) {
                    return std::ptr::null_mut();
                }
                *image = (vec![0; width * height * 4], [width, height]);
                image.0.as_mut_ptr()
            }
            let mut image = Image::default();
            anyhow::ensure!(
                unsafe { apps_image_decode(bytes.as_ptr(), bytes.len(), allocate, &mut image) },
                "Unsupported or too large image"
            );
            let (mut pixels, [width, height]) = image;
            for pixel in pixels.as_chunks_mut::<4>().0 {
                let alpha = u32::from(pixel[3]);
                if alpha != 0 && alpha != 255 {
                    for value in &mut pixel[..3] {
                        *value = ((u32::from(*value) * 255 + alpha / 2) / alpha) as u8;
                    }
                }
            }
            (pixels, width, height)
        }
        #[cfg(not(target_os = "macos"))]
        _ => {
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(16384);
            limits.max_image_height = Some(16384);
            limits.max_alloc = Some(256 * 1024 * 1024);
            let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
            let image = if reader.format() == Some(image::ImageFormat::Avif) {
                avif::decode_avif(bytes, limits)?
            } else {
                reader.limits(limits);
                reader.decode()?.into_rgba8()
            };
            let (width, height) = image.dimensions();
            (bgra(image.into_raw(), 4), width as usize, height as usize)
        }
    })
}

// WARNING: png's default level and its adaptive filter are several times slower than these, and
// `capturePage()` and `toDataURL()` encode a whole frame per call.
pub(crate) fn encode_png(image: &tiny_skia::Pixmap) -> anyhow::Result<Vec<u8>> {
    let mut straight = image.data().to_vec();
    for (pixel, out) in (image.pixels().iter()).zip(straight.as_chunks_mut::<4>().0) {
        if pixel.alpha() != 255 {
            let color = pixel.demultiply();
            *out = [color.red(), color.green(), color.blue(), color.alpha()];
        }
    }
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, image.width(), image.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    encoder.set_filter(png::Filter::Sub);
    encoder.write_header()?.write_image_data(&straight)?;
    Ok(bytes)
}

pub(crate) fn decode_later(job: Box<dyn FnOnce() + Send>) {
    use std::sync::{Mutex, PoisonError};
    type Job = Box<dyn FnOnce() + Send>;
    static JOBS: Mutex<(std::collections::VecDeque<Job>, usize)> =
        Mutex::new((std::collections::VecDeque::new(), 0));
    let mut jobs = JOBS.lock().unwrap_or_else(PoisonError::into_inner);
    jobs.0.push_back(job);
    if jobs.1 < 2
        && std::thread::Builder::new()
            .name("apps-image".into())
            .spawn(|| {
                while let Some(job) = {
                    let mut jobs = JOBS.lock().unwrap_or_else(PoisonError::into_inner);
                    let job = jobs.0.pop_front();
                    jobs.1 -= usize::from(job.is_none());
                    job
                } {
                    job();
                }
            })
            .is_ok()
    {
        jobs.1 += 1;
    }
}

pub(crate) fn url_file(
    source: &str,
    base: Option<&url::Url>,
    assets: &std::collections::HashMap<String, std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    let source = source.trim().trim_matches(['"', '\'']);
    data_file(source).or_else(|| {
        let base = base?;
        crate::host::local_path(assets, base, &base.join(source).ok()?)
    })
}

pub(crate) fn data_file(url: &str) -> Option<std::path::PathBuf> {
    use std::hash::{Hash, Hasher};
    let (header, payload) = url.strip_prefix("data:")?.split_once(',')?;
    let bytes = if header.ends_with(";base64") {
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, payload.trim()).ok()?
    } else {
        let input = payload.as_bytes();
        let mut bytes = Vec::with_capacity(input.len());
        let mut index = 0;
        while index < input.len() {
            let hex = (input[index] == b'%')
                .then(|| input.get(index + 1..index + 3))
                .flatten()
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok());
            bytes.push(hex.unwrap_or(input[index]));
            index += if hex.is_some() { 3 } else { 1 };
        }
        bytes
    };
    let mut hasher = std::hash::DefaultHasher::new();
    bytes.hash(&mut hasher);
    let directory = std::env::temp_dir().join("apps-data-images");
    let extension = if header.contains("svg") { "svg" } else { "bin" };
    let path = directory.join(format!("{:016x}.{extension}", hasher.finish()));
    if !path.exists() {
        std::fs::create_dir_all(&directory).ok()?;
        // Other windows and other apps read this file, so it must appear complete.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let count = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let partial = path.with_extension(format!("{}-{count}.tmp", std::process::id()));
        if std::fs::write(&partial, bytes).is_err() || std::fs::rename(&partial, &path).is_err() {
            let _ = std::fs::remove_file(&partial);
            if !path.exists() {
                return None;
            }
        }
    }
    Some(path)
}
