use image::{DynamicImage, ImageFormat, ImageReader, ImageResult, Limits};
use std::io::Cursor;

pub fn decode(bytes: &[u8], limits: Limits) -> ImageResult<DynamicImage> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    if reader.format() == Some(ImageFormat::Avif) {
        return gpui::decode_avif(bytes, limits).map(DynamicImage::ImageRgba8);
    }
    reader.limits(limits);
    reader.decode()
}
