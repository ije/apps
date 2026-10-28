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

#[test]
fn svg_and_bundled_images_load_through_the_host() {
    use apps_runtime::host::{Host, Page, Role};
    let mut doc = Document::parse("<body><img></body>");
    let mut state = dom::State::default();
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="30" height="12"><rect width="30" height="12"/></svg>"#;
    let result = dom::call(
        "dom.image",
        &json!([1, svg.as_slice()]),
        &mut doc,
        &mut state,
        [640., 480.],
    );
    assert_eq!(result.unwrap().unwrap(), json!([30., 12.]));
    assert!(!state.images.contains_key(&1));
    assert!(
        dom::call(
            "dom.image",
            &json!([1, b"not an image".as_slice()]),
            &mut doc,
            &mut state,
            [640., 480.]
        )
        .unwrap()
        .is_err()
    );

    let directory = tempfile::tempdir().unwrap();
    let png = directory.path().join("sample.png");
    std::fs::write(&png, include_bytes!("images/sample.png")).unwrap();
    let mut host = Host::default();
    host.assets.insert("assets/sample.png".into(), png.clone());
    host.pages.insert(
        1,
        Page {
            doc: Document::parse("<body><img></body>"),
            dirty: false,
        },
    );
    host.dom.entry(1).or_default().base = Some(url::Url::parse("apps:///page-0.html").unwrap());
    let mut load = |source: &str| {
        host.call(
            Role::Renderer(1),
            json!({"op":"dom.imageURL","args":[1, source]}),
        )
    };
    assert_eq!(load("assets/sample.png").unwrap(), json!([4, 3]));
    assert!(load("apps:///assets/missing.png").is_err());
    assert!(load(png.to_str().unwrap()).is_err());
    assert!(load(url::Url::from_file_path(&png).unwrap().as_str()).is_err());
    assert!(host.pages[&1].dirty && host.dom[&1].images.contains_key(&1));
    host.dom.get_mut(&1).unwrap().base =
        Some(url::Url::from_directory_path(directory.path()).unwrap());
    assert_eq!(
        host.call(
            Role::Renderer(1),
            json!({"op":"dom.imageURL","args":[1, "sample.png"]})
        )
        .unwrap(),
        json!([4, 3])
    );
}
