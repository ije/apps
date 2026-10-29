use crate::{
    html::Document,
    render::{Declarations, Stylesheet},
};
use anyhow::{Context, Result, ensure};
use bbtree::unsync::BBTreeMap;
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

const HEADER: &[u8; 8] = b"H2O\0\x01\0\0\0";

#[derive(Default, Deserialize, Serialize)]
pub struct Bundle {
    #[serde(skip)]
    pub packaged: bool,
    #[serde(skip)]
    pub root: std::path::PathBuf,
    #[serde(with = "map")]
    pub files: BBTreeMap<String, String>,
    #[serde(with = "map")]
    pub pages: BBTreeMap<String, Document>,
}

pub fn package_metadata(source: Option<&str>) -> Result<(String, String, String)> {
    let package: serde_json::Value =
        serde_json::from_str(source.unwrap_or("{}")).context("Invalid package.json")?;
    ensure!(package.is_object(), "Expected an object in package.json");
    let name = package
        .get("name")
        .map(|value| value.as_str().context("package.json name must be a string"))
        .transpose()?
        .unwrap_or("App");
    ensure!(
        name.len() >= 2
            && name.as_bytes()[0].is_ascii_alphabetic()
            && name
                .bytes()
                .skip(1)
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
        "Invalid executable name {name:?}; expected ^[a-zA-Z][a-zA-Z0-9_]+$"
    );
    let product_name = package
        .get("productName")
        .map(|value| {
            value
                .as_str()
                .context("package.json productName must be a string")
        })
        .transpose()?
        .unwrap_or(name);
    ensure!(
        !product_name.is_empty()
            && !product_name.ends_with([' ', '.'])
            && !product_name
                .chars()
                .any(|c| c.is_control() || "/\\:*?\"<>|".contains(c)),
        "Invalid productName {product_name:?}; expected a valid filename"
    );
    let version = package
        .get("version")
        .map(|value| {
            value
                .as_str()
                .context("package.json version must be a string")
        })
        .transpose()?
        .unwrap_or("0.0.0");
    let (core, suffix) = version
        .split_once('-')
        .map_or((version, None), |(core, suffix)| (core, Some(suffix)));
    let mut parts = core.split('.');
    ensure!(
        (0..3).all(|_| {
            parts.next().is_some_and(|part| {
                !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
            })
        }) && parts.next().is_none()
            && suffix.is_none_or(|suffix| !suffix.is_empty()
                && suffix.bytes().all(|byte| byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'_' | b'-'))),
        r"Invalid package version {version:?}; expected ^\d+\.\d+\.\d+(-[a-z0-9._-]+)?$"
    );
    Ok((name.into(), product_name.into(), version.into()))
}

pub fn write(path: &Path, files: &BBTreeMap<String, String>) -> Result<usize> {
    ensure!(files.contains_key("app.js"), "Missing app.js");
    let mut bundle = Bundle::default();
    for (name, source) in files {
        if name.ends_with(".html") {
            let mut doc = Document::parse(source);
            for node in &mut doc.nodes {
                if node.tag == "style" {
                    node.stylesheet = Some(std::rc::Rc::new(Stylesheet::parse(&node.text)));
                    node.text.clear();
                }
                if let Some(source) = node.attrs.remove("style") {
                    node.inline = Some(Declarations::parse(&source));
                }
            }
            bundle.pages.insert(name.clone(), doc);
        } else if name == "native.json" {
            bundle.files.insert(
                name.clone(),
                copy_native(path.parent().context("Missing bundle directory")?, source)?,
            );
        } else if name == "assets.json" {
            bundle.files.insert(
                name.clone(),
                copy_assets(path.parent().context("Missing bundle directory")?, source)?,
            );
        } else {
            ensure!(
                name.ends_with(".js") || name == "package.json",
                "Unsupported bundle file: {name}"
            );
            bundle.files.insert(name.clone(), source.clone());
        }
    }
    let mut bytes = HEADER.to_vec();
    bytes.extend(postcard::to_allocvec(&bundle)?);
    fs::write(path, &bytes).with_context(|| format!("Cannot write {}", path.display()))?;
    Ok(bytes.len())
}

pub fn read(directory: &Path) -> Result<Bundle> {
    let directory = if directory.join("Contents/Resources").is_dir() {
        directory.join("Contents/Resources")
    } else if !directory.join("app.h2o").try_exists()? && !directory.join("app.js").try_exists()? {
        let mut resources = None;
        for entry in fs::read_dir(directory).context("Cannot read bundle directory")? {
            let path = entry?.path();
            if path.extension().is_some_and(|extension| extension == "app")
                && path.join("Contents/Resources").is_dir()
            {
                ensure!(
                    resources.is_none(),
                    "Multiple app bundles; specify the .app directory"
                );
                resources = Some(path.join("Contents/Resources"));
            }
        }
        resources.unwrap_or_else(|| directory.to_owned())
    } else {
        directory.to_owned()
    };
    let directory = directory
        .canonicalize()
        .context("Cannot resolve bundle directory")?;
    let path = directory.join("app.h2o");
    let mut bundle = if path.try_exists()? {
        let bytes = fs::read(&path).context("Cannot read app.h2o")?;
        ensure!(
            bytes.starts_with(HEADER),
            "Invalid or unsupported app.h2o format"
        );
        let (mut bundle, rest): (Bundle, _) =
            postcard::take_from_bytes(&bytes[HEADER.len()..]).context("Invalid app.h2o data")?;
        ensure!(rest.is_empty(), "Trailing app.h2o data");
        for (_, doc) in &mut bundle.pages {
            ensure!(!doc.nodes.is_empty(), "Missing document root");
            ensure!(
                doc.nodes[0].tag == "#document" && doc.nodes[0].parent.is_none(),
                "Invalid document root"
            );
            let mut seen = vec![false; doc.nodes.len()];
            seen[0] = true;
            for (id, node) in doc.nodes.iter().enumerate() {
                for &child in &node.children {
                    ensure!(
                        child > id && child < doc.nodes.len(),
                        "Invalid document child"
                    );
                    ensure!(
                        !seen[child] && doc.nodes[child].parent == Some(id),
                        "Invalid document parent"
                    );
                    seen[child] = true;
                }
            }
            ensure!(
                seen.into_iter().all(|value| value),
                "Detached document node"
            );
            for node in &mut doc.nodes {
                if let Some(sheet) = &mut node.stylesheet {
                    std::rc::Rc::make_mut(sheet).bind_parents()?;
                }
            }
        }
        bundle
    } else {
        let mut bundle = Bundle::default();
        for entry in fs::read_dir(&directory).context("Cannot read bundle directory")? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("Invalid bundle filename"))?;
            if (name.ends_with(".html")
                || matches!(
                    name.as_str(),
                    "app.js" | "deps.js" | "native.json" | "package.json" | "assets.json"
                ))
                && entry.file_type()?.is_file()
            {
                bundle.files.insert(name, fs::read_to_string(entry.path())?);
            }
        }
        bundle
    };
    if let Some(source) = bundle.files.get_mut("native.json") {
        let mut paths: std::collections::BTreeMap<String, std::path::PathBuf> =
            serde_json::from_str(source)?;
        for (name, path) in &mut paths {
            let relative = format!("node_modules/{name}");
            validate_native_path(&relative)?;
            ensure!(*path == Path::new(&relative), "Invalid native addon path");
            *path = directory.join(relative);
        }
        *source = serde_json::to_string(&paths)?;
    }
    if let Some(source) = bundle.files.get_mut("assets.json") {
        let mut paths: std::collections::BTreeMap<String, std::path::PathBuf> =
            serde_json::from_str(source)?;
        for (name, path) in &mut paths {
            validate_asset_path(name)?;
            ensure!(*path == Path::new(name), "Invalid asset path");
            *path = directory.join(name.as_str());
        }
        *source = serde_json::to_string(&paths)?;
    }
    bundle.packaged = path.is_file();
    bundle.root = directory;
    ensure!(bundle.files.contains_key("app.js"), "Missing app.js");
    Ok(bundle)
}

fn validate_native_path(name: &str) -> Result<()> {
    let path = Path::new(name);
    ensure!(
        name.starts_with("node_modules/")
            && !name.contains('\\')
            && path
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
            && path.components().count() >= 3
            && path
                .extension()
                .is_some_and(|extension| extension == "node"),
        "Invalid native addon path: {name}"
    );
    Ok(())
}

fn validate_asset_path(name: &str) -> Result<()> {
    let path = Path::new(name);
    ensure!(
        name.starts_with("assets/")
            && !name.contains('\\')
            && path.components().count() == 2
            && path
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_))),
        "Invalid asset path: {name}"
    );
    Ok(())
}

pub fn copy_assets(directory: &Path, source: &str) -> Result<String> {
    let paths: std::collections::BTreeMap<String, std::path::PathBuf> =
        serde_json::from_str(source)?;
    let mut output = std::collections::BTreeMap::new();
    for (name, source) in paths {
        validate_asset_path(&name)?;
        let destination = directory.join(&name);
        fs::create_dir_all(destination.parent().unwrap())?;
        fs::copy(&source, &destination)
            .with_context(|| format!("Cannot copy asset {}", source.display()))?;
        output.insert(name.clone(), name);
    }
    Ok(serde_json::to_string(&output)?)
}

pub fn copy_native(directory: &Path, source: &str) -> Result<String> {
    let paths: std::collections::BTreeMap<String, std::path::PathBuf> =
        serde_json::from_str(source)?;
    let mut output = std::collections::BTreeMap::new();
    for (name, source) in paths {
        let relative = format!("node_modules/{name}");
        validate_native_path(&relative)?;
        let destination = directory.join(&relative);
        fs::create_dir_all(destination.parent().unwrap())?;
        fs::copy(&source, &destination)
            .with_context(|| format!("Cannot copy native addon {}", source.display()))?;
        output.insert(name, relative);
    }
    Ok(serde_json::to_string(&output)?)
}

pub(crate) mod map {
    use bbtree::unsync::BBTreeMap;
    use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

    pub fn serialize<K: Ord + Serialize, V: Serialize, S: Serializer>(
        map: &BBTreeMap<K, V>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(map)
    }

    pub fn deserialize<
        'de,
        K: Ord + Deserialize<'de>,
        V: Deserialize<'de>,
        D: Deserializer<'de>,
    >(
        deserializer: D,
    ) -> Result<BBTreeMap<K, V>, D::Error> {
        let entries = Vec::<(K, V)>::deserialize(deserializer)?;
        let mut map = BBTreeMap::new();
        for (key, value) in entries {
            if map.insert(key, value).is_some() {
                return Err(D::Error::custom("Duplicate bundle key"));
            }
        }
        Ok(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        host::{Host, Page, Role},
        render,
    };
    use serde_json::json;

    #[test]
    fn package_metadata_uses_product_name_and_defaults() {
        for (source, name, product_name, version) in [
            (None, "App", "App", "0.0.0"),
            (Some("{}"), "App", "App", "0.0.0"),
            (
                Some(r#"{"name":"package_name","version":"1.2.3"}"#),
                "package_name",
                "package_name",
                "1.2.3",
            ),
            (
                Some(r#"{"productName":"My App","name":"package_name","version":"2.0.0"}"#),
                "package_name",
                "My App",
                "2.0.0",
            ),
            (
                Some(r#"{"name":"package_name"}"#),
                "package_name",
                "package_name",
                "0.0.0",
            ),
            (
                Some(r#"{"productName":"My App"}"#),
                "App",
                "My App",
                "0.0.0",
            ),
        ] {
            assert_eq!(
                package_metadata(source).unwrap(),
                (name.into(), product_name.into(), version.into())
            );
        }
        for name in ["Aa", "a_", "A1", "my_App12"] {
            assert_eq!(
                package_metadata(Some(&json!({"name":name}).to_string()))
                    .unwrap()
                    .0,
                name
            );
        }
        for name in [
            "",
            "A",
            "_app",
            "1app",
            "app-name",
            "My App",
            " app",
            "app ",
            "aé",
            "../outside",
            "/absolute",
            "a/b",
            "a\\b",
            ".",
            "..",
            "a\0b",
            "app\n",
        ] {
            let error = package_metadata(Some(&json!({"name":name}).to_string())).unwrap_err();
            assert!(
                error.to_string().contains("^[a-zA-Z][a-zA-Z0-9_]+$"),
                "{name:?}"
            );
        }
        for value in [json!(null), json!(42), json!([])] {
            assert!(package_metadata(Some(&json!({"name":value}).to_string())).is_err());
        }
        for name in [
            "",
            "../outside",
            "/absolute",
            "a/b",
            "a\\b",
            ".",
            "..",
            "app ",
            "a\0b",
            "app\n",
        ] {
            assert!(package_metadata(Some(&json!({"productName":name}).to_string())).is_err());
        }
        for value in [json!(null), json!(42), json!([])] {
            assert!(package_metadata(Some(&json!({"productName":value}).to_string())).is_err());
        }
        assert!(package_metadata(Some("{")).is_err());
        assert!(package_metadata(Some("[]")).is_err());
    }

    #[test]
    fn package_versions_match_the_complete_pattern() {
        for version in [
            "0.0.0",
            "1.2.3",
            "01.002.0003",
            "123456789012345678901234567890.0.0",
            "1.2.3-a",
            "1.2.3-alpha.1",
            "1.2.3-beta_test-2",
            "1.2.3-._-",
            "12.34.56-1",
        ] {
            assert_eq!(
                package_metadata(Some(&json!({"version":version}).to_string()))
                    .unwrap()
                    .2,
                version
            );
        }
        for version in [
            "",
            " ",
            "1",
            "1.2",
            "1.2.3.4",
            "v1.2.3",
            "+1.2.3",
            "1.2.d",
            "1..3",
            "1.2.",
            "-1.2.3",
            "1.2.-3",
            "1.2.3-",
            "1.2.3-Beta",
            "1.2.3+build.1",
            "1.2.3-a/b",
            "1.2.3-alpha!",
            " 1.2.3",
            "1.2.3 ",
            "1.2.3\n",
            "１.2.3",
            "1.2.3-α",
        ] {
            let error =
                package_metadata(Some(&json!({"version":version}).to_string())).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(r"^\d+\.\d+\.\d+(-[a-z0-9._-]+)?$"),
                "{version:?}"
            );
        }
        for version in [json!(null), json!(123), json!([]), json!({})] {
            assert!(
                package_metadata(Some(&json!({"version":version}).to_string()))
                    .unwrap_err()
                    .to_string()
                    .contains("must be a string")
            );
        }
    }

    #[test]
    fn binary_trees_preserve_cascade_resize_and_dom_mutations() {
        let html = r#"<html><head><style id="sheet">
            @import 'tailwindcss';
            @theme { --color-mint-500: #123456; }
            .card, #card { --space: 3px; padding:var(--space); color:blue;
                &&.active { @apply bg-mint-500; }
                @media (min-width:600px) and (min-height:200px) {
                    width:calc(50vw - 1px);
                    > .label { color:red; }
                }
                height:20px;
            }
            @media print, (max-height:200px) { .label { margin:4px; } }
            @supports (display:unsupported) { #card { height:999px; } }
        </style></head><body><div id="card" class="card active" style="height:30px !important; @apply p-4;"><span class="label">你好 &amp; hello</span></div></body></html>"#;
        let dir = tempfile::tempdir().unwrap();
        let files = BBTreeMap::from([
            ("app.js".into(), "define({id:0});".into()),
            ("page-0.html".into(), html.into()),
        ]);
        write(&dir.path().join("app.h2o"), &files).unwrap();
        let mut bundle = read(dir.path()).unwrap();
        assert_eq!(bundle.files.len(), 1);
        let doc = bundle.pages.remove("page-0.html").unwrap();
        let raw = Document::parse(html);
        let sheet = doc.nodes.iter().position(|n| n.tag == "style").unwrap();
        let card = doc
            .nodes
            .iter()
            .position(|n| n.attrs.get("id").is_some_and(|id| id == "card"))
            .unwrap();
        assert!(doc.nodes[sheet].text.is_empty());
        assert!(doc.nodes[sheet].stylesheet.is_some());
        assert!(!doc.nodes[card].attrs.contains_key("style"));
        assert!(doc.nodes[card].inline.is_some());
        assert!(doc.text(card).contains("你好 & hello"));
        for (width, height) in [(400., 600.), (800., 600.), (800., 100.)] {
            let actual = render::styles(&doc, width, height);
            let expected = render::styles(&raw, width, height);
            for (actual, expected) in actual.iter().zip(&expected) {
                assert!(actual.iter().eq(expected.iter()));
            }
            let serialized = Document::parse(&doc.serialize(0));
            let roundtrip = render::styles(&serialized, width, height);
            for (actual, expected) in roundtrip.iter().zip(&expected) {
                assert!(actual.iter().eq(expected.iter()));
            }
        }
        let mut host = Host::default();
        host.pages.insert(1, Page { doc, dirty: false });
        let style = host
            .call(
                Role::Renderer(1),
                json!({"op":"dom.attr","args":[card,"style"]}),
            )
            .unwrap();
        assert!(style.as_str().unwrap().contains("height:30px !important"));
        host.call(
            Role::Renderer(1),
            json!({"op":"dom.setAttr","args":[card,"style","height:42px"]}),
        )
        .unwrap();
        assert_eq!(
            render::styles(&host.pages[&1].doc, 800., 600.)[card]["height"],
            "42px"
        );
        host.call(
            Role::Renderer(1),
            json!({"op":"dom.removeAttr","args":[card,"style"]}),
        )
        .unwrap();
        assert_eq!(
            render::styles(&host.pages[&1].doc, 800., 600.)[card]["height"],
            "20px"
        );
        host.call(
            Role::Renderer(1),
            json!({"op":"dom.setText","args":[sheet,"#card {height:55px}"]}),
        )
        .unwrap();
        assert_eq!(
            render::styles(&host.pages[&1].doc, 800., 600.)[card]["height"],
            "55px"
        );
    }

    #[test]
    fn nested_selectors_stay_compact_and_keep_parent_links() {
        let html = format!(
            "<style>.card, #card {{{}&.active {{width:1px}}{}}}</style><div id='card' class='card active'></div>",
            "&& {".repeat(80),
            "}".repeat(80)
        );
        let dir = tempfile::tempdir().unwrap();
        let files = BBTreeMap::from([
            ("app.js".into(), String::new()),
            ("page-0.html".into(), html),
        ]);
        let size = write(&dir.path().join("app.h2o"), &files).unwrap();
        assert!(
            size < 10_000,
            "Nested parent selectors were duplicated: {size}"
        );
        let bundle = read(dir.path()).unwrap();
        assert_eq!(
            render::styles(&bundle.pages["page-0.html"], 800., 600.)
                .last()
                .unwrap()["width"],
            "1px"
        );
    }

    #[test]
    fn rejects_corrupt_bundles_and_invalid_document_links() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.h2o");
        let files = BBTreeMap::from([
            ("app.js".into(), "const x = '你好';".into()),
            ("page-0.html".into(), "<body>hello</body>".into()),
        ]);
        write(&path, &files).unwrap();
        let bytes = fs::read(&path).unwrap();
        for length in 0..bytes.len() {
            fs::write(&path, &bytes[..length]).unwrap();
            assert!(read(dir.path()).is_err(), "Accepted truncation at {length}");
        }
        for index in [0, 4] {
            let mut damaged = bytes.clone();
            damaged[index] ^= 1;
            fs::write(&path, damaged).unwrap();
            assert!(read(dir.path()).is_err());
        }
        let mut damaged = bytes.clone();
        damaged.push(0);
        fs::write(&path, damaged).unwrap();
        assert!(read(dir.path()).is_err());
        fs::write(&path, bytes).unwrap();
        let mut bundle = read(dir.path()).unwrap();
        bundle.pages.get_mut("page-0.html").unwrap().nodes[0]
            .children
            .push(usize::MAX);
        let mut bytes = HEADER.to_vec();
        bytes.extend(postcard::to_allocvec(&bundle).unwrap());
        fs::write(&path, bytes).unwrap();
        assert!(read(dir.path()).is_err());
    }
}
