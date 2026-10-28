use apps_runtime::{dom, html::Document};
use image::{ImageFormat, Limits};
use serde_json::json;

#[test]
fn web_images_decode_in_the_dom() {
    let mut doc = Document::parse("<body><img></body>");
    let mut state = dom::State::default();
    for bytes in [
        include_bytes!("images/sample.png").as_slice(),
        include_bytes!("images/sample.jpg"),
        include_bytes!("images/sample.gif"),
        include_bytes!("images/sample.webp"),
        include_bytes!("images/sample.bmp"),
        include_bytes!("images/sample.ico"),
        include_bytes!("images/sample.avif"),
        include_bytes!("images/alpha.avif"),
    ] {
        let result = dom::call(
            "dom.image",
            &json!([1, bytes]),
            &mut doc,
            &mut state,
            [640., 480.],
        )
        .unwrap()
        .unwrap();
        assert_eq!(result, json!([4, 3]));
        assert!(state.images.contains_key(&1));
    }
    let image =
        apps_runtime::images::decode(include_bytes!("images/alpha.avif"), Limits::default())
            .unwrap()
            .into_rgba8();
    for pixel in image.pixels() {
        for (value, expected) in pixel.0.into_iter().zip([32, 128, 224, 85]) {
            assert!(value.abs_diff(expected) <= 3, "{pixel:?}");
        }
    }
}

#[test]
fn web_image_formats_are_enabled() {
    for format in [
        ImageFormat::Png,
        ImageFormat::Jpeg,
        ImageFormat::Gif,
        ImageFormat::WebP,
        ImageFormat::Bmp,
        ImageFormat::Ico,
    ] {
        assert!(format.reading_enabled(), "{format:?}");
        assert!(format.writing_enabled(), "{format:?}");
    }
}

#[test]
fn avif_rejects_corrupt_data_and_respects_limits() {
    let mut bytes = include_bytes!("images/corrupt-base.avif").to_vec();
    assert!(apps_runtime::images::decode(&bytes, Limits::default()).is_ok());
    bytes[280] ^= 1;
    assert!(apps_runtime::images::decode(&bytes, Limits::default()).is_err());
    for length in [0, 8, 64, 128] {
        assert!(apps_runtime::images::decode(&bytes[..length], Limits::default()).is_err());
    }
    for allocation in [false, true] {
        let mut limits = Limits::default();
        if allocation {
            limits.max_alloc = Some(32);
        } else {
            limits.max_image_width = Some(2);
        }
        assert!(matches!(
            apps_runtime::images::decode(include_bytes!("images/sample.avif"), limits),
            Err(image::ImageError::Limits(_))
        ));
    }
}
