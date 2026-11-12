use apps_runtime::dom::{self, Document};
use serde_json::{Value, json};

fn decoded(doc: &mut Document, state: &mut dom::State) -> Value {
    let started = std::time::Instant::now();
    loop {
        let results = dom::call("dom.decoded", &json!([]), doc, state, [640., 480.]).unwrap();
        if let Some(result) = results.get(0) {
            return result[1].clone();
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn web_images_decode_in_the_dom() {
    let mut doc = Document::parse("<body><img></body>");
    let mut state = dom::State::default();
    for (bytes, size) in [
        (include_bytes!("images/sample.png").as_slice(), [4, 3]),
        (include_bytes!("images/sample.jpg"), [4, 3]),
        (include_bytes!("images/sample.gif"), [4, 3]),
        (include_bytes!("images/sample.webp"), [4, 3]),
        (include_bytes!("images/sample.bmp"), [4, 3]),
        (include_bytes!("images/sample.ico"), [16, 12]),
        (include_bytes!("images/sample.avif"), [4, 3]),
        (include_bytes!("images/alpha.avif"), [4, 3]),
        (include_bytes!("images/gray.jpg"), [4, 3]),
        (include_bytes!("images/cmyk.jpg"), [4, 3]),
    ] {
        dom::call(
            "dom.image",
            &json!([1, bytes]),
            &mut doc,
            &mut state,
            [640., 480.],
        )
        .unwrap();
        assert_eq!(decoded(&mut doc, &mut state), json!(size));
        assert!(state.images.contains_key(&1));
    }
    for (bytes, color) in [
        (
            include_bytes!("images/alpha.avif").as_slice(),
            [224, 128, 32, 85],
        ),
        (include_bytes!("images/gray.jpg"), [110, 110, 110, 255]),
        (include_bytes!("images/cmyk.jpg"), [224, 128, 32, 255]),
    ] {
        let image = apps_runtime::render::image::decode(bytes).unwrap();
        for pixel in image.bytes().as_chunks::<4>().0 {
            for (value, expected) in pixel.iter().zip(color) {
                assert!(value.abs_diff(expected) <= 3, "{pixel:?}");
            }
        }
    }
}

#[test]
fn svg_raster_images_decode_through_the_runtime() {
    use apps_runtime::render::canvas::{svg_pixels, svg_tree};
    use base64::Engine;
    for (kind, bytes) in [
        ("png", include_bytes!("images/sample.png").as_slice()),
        ("jpeg", include_bytes!("images/sample.jpg")),
        ("gif", include_bytes!("images/sample.gif")),
        ("webp", include_bytes!("images/sample.webp")),
    ] {
        let source = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="3"><image href="data:image/{kind};base64,{}" width="4" height="3"/></svg>"#,
            base64::engine::general_purpose::STANDARD.encode(bytes)
        );
        let pixels = svg_pixels(&svg_tree(&source, false).unwrap(), 4, 3, None).unwrap();
        for pixel in pixels.data().as_chunks::<4>().0 {
            for (value, expected) in pixel.iter().zip([32, 128, 224, 255]) {
                assert!(value.abs_diff(expected) <= 3, "{kind} {pixel:?}");
            }
        }
    }
    // An image that names a file is read only in SVG that the runtime wrote itself, never in page content.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sample.png");
    std::fs::write(&path, include_bytes!("images/sample.png")).unwrap();
    let source = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="3"><image href="{}" width="4" height="3"/></svg>"#,
        path.display()
    );
    let drawn = |files| {
        let pixels = svg_pixels(&svg_tree(&source, files).unwrap(), 4, 3, None).unwrap();
        pixels.data().iter().any(|byte| *byte != 0)
    };
    assert!(drawn(true));
    assert!(!drawn(false));
}

#[cfg(not(target_os = "macos"))]
#[test]
fn web_image_formats_are_enabled() {
    use image::ImageFormat;
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
    use apps_runtime::render::image::decode;
    let mut bytes = include_bytes!("images/corrupt-base.avif").to_vec();
    assert!(decode(&bytes).is_ok());
    bytes[280] ^= 1;
    assert!(decode(&bytes).is_err() || cfg!(target_os = "macos"));
    for length in [0, 8, 64, 128] {
        assert!(decode(&bytes[..length]).is_err());
    }
    #[cfg(not(target_os = "macos"))]
    for allocation in [false, true] {
        let mut limits = image::Limits::default();
        if allocation {
            limits.max_alloc = Some(32);
        } else {
            limits.max_image_width = Some(2);
        }
        assert!(matches!(
            apps_runtime::render::image::avif::decode_avif(
                include_bytes!("images/sample.avif"),
                limits
            ),
            Err(image::ImageError::Limits(_))
        ));
    }
}

#[test]
fn svg_and_bundled_images_load_through_the_host() {
    use apps_runtime::host::{Host, Page, Role};
    use std::io::Write;
    let mut doc = Document::parse("<body><img></body>");
    let mut state = dom::State::default();
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="30" height="12"><rect width="30" height="12"/></svg>"#;
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(svg).unwrap();
    let svgz = gzip.finish().unwrap();
    for bytes in [svg.as_slice(), svgz.as_slice()] {
        dom::call(
            "dom.image",
            &json!([1, bytes]),
            &mut doc,
            &mut state,
            [640., 480.],
        )
        .unwrap();
        assert_eq!(decoded(&mut doc, &mut state), json!([30., 12.]));
        assert!(!state.images.contains_key(&1));
    }
    dom::call(
        "dom.image",
        &json!([1, b"not an image".as_slice()]),
        &mut doc,
        &mut state,
        [640., 480.],
    )
    .unwrap();
    assert!(decoded(&mut doc, &mut state).is_string());

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
    let load = |host: &mut Host, source: &str| {
        host.call(Role::Renderer(1), "dom.imageURL", json!([1, source]))
    };
    let decoded = |host: &mut Host| {
        let started = std::time::Instant::now();
        loop {
            let results = host
                .call(Role::Renderer(1), "dom.decoded", json!([]))
                .unwrap();
            if let Some(result) = results.get(0) {
                return result[1].clone();
            }
            assert!(started.elapsed() < std::time::Duration::from_secs(10));
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    };
    assert!(load(&mut host, "assets/sample.png").unwrap().is_u64());
    assert!(!host.pages[&1].dirty && !host.dom[&1].images.contains_key(&1));
    assert_eq!(decoded(&mut host), json!([4, 3]));
    assert!(host.pages[&1].dirty && host.dom[&1].images.contains_key(&1));
    assert!(load(&mut host, "apps:///assets/missing.png").is_err());
    assert!(load(&mut host, png.to_str().unwrap()).is_err());
    assert!(load(&mut host, url::Url::from_file_path(&png).unwrap().as_str()).is_err());
    host.dom.get_mut(&1).unwrap().base =
        Some(url::Url::from_directory_path(directory.path()).unwrap());
    load(&mut host, "sample.png").unwrap();
    assert_eq!(decoded(&mut host), json!([4, 3]));
    load(&mut host, "missing.png").unwrap();
    assert!(decoded(&mut host).is_string());
}
