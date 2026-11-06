use crate::{
    dom::Document,
    style::{Declarations, Stylesheet},
};
use anyhow::{Context, Result, ensure};
use bbtree::unsync::BBTreeMap;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, path::Path};

const HEADER: &[u8; 8] = b"APP\0\x01\0\0\0";

pub const LINK_SHEET: &str = "apps-sheet";

pub fn page(source: &str) -> Document {
    let mut doc = Document::parse(source);
    for node in &mut doc.nodes {
        if node.tag == "link"
            && let Some(css) = node.attrs.remove(LINK_SHEET)
        {
            node.stylesheet = Some(std::rc::Rc::new(Stylesheet::parse(&css)));
        }
    }
    doc
}

// WARNING: packs a page for other threads, because its documents can't cross threads.
pub(crate) fn pack_page(doc: &Document) -> Result<Vec<u8>> {
    Ok(postcard::to_allocvec(doc)?)
}

pub(crate) fn unpack_page(bytes: &[u8]) -> Result<Document> {
    let mut doc: Document = postcard::from_bytes(bytes)?;
    for node in &mut doc.nodes {
        if let Some(sheet) = &mut node.stylesheet {
            std::rc::Rc::make_mut(sheet).bind_parents()?;
        }
    }
    Ok(doc)
}

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
    #[serde(skip)]
    pub scripts: HashMap<String, bytes::Bytes>,
}

// WARNING: module code must stay outside the loader function, or it could see loader internals such as
// `compile`. Definitions run in a second function whose only binding is `$appsDefine`.
pub fn script_source(files: &BBTreeMap<String, String>, execute: bool) -> String {
    let modules = js!("modules.js");
    let app = files.get("app.js").map_or("", String::as_str);
    let mut source = String::with_capacity(modules.len() + app.len() + 64);
    source.push_str("(()=>{");
    source.push_str(&modules);
    source.push_str("})()(($appsDefine)=>{\n");
    source.push_str(app);
    source.push_str(if execute { "\n},true);" } else { "\n},false);" });
    source
}

pub fn package_metadata(source: Option<&str>) -> Result<(String, String, String, String)> {
    let package: serde_json::Value =
        serde_json::from_str(source.unwrap_or("{}")).context("Invalid package.json")?;
    ensure!(package.is_object(), "Expected an object in package.json");
    // WARNING: like electron-builder, `executableName` wins, because npm names like `@scope/app` can't name a file.
    let name = package
        .get("executableName")
        .or_else(|| package.get("name"))
        .map(|value| value.as_str().context("package.json name must be a string"))
        .transpose()?
        .unwrap_or("App");
    let rust = package["type"] == "rust";
    ensure!(
        name.len() >= 2
            && name.as_bytes()[0].is_ascii_alphabetic()
            && name
                .bytes()
                .skip(1)
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || rust && byte == b'-'),
        "Invalid executable name {name:?}; expected ^[a-zA-Z][a-zA-Z0-9_{}]+$",
        if rust { "-" } else { "" }
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
    let identifier = match package.get("identifier") {
        Some(value) => value
            .as_str()
            .context("package.json identifier must be a string")?
            .to_owned(),
        None => format!("com.apps.{}", name.replace('_', "-")),
    };
    ensure!(
        identifier.contains('.')
            && identifier.split('.').all(|part| {
                !part.is_empty()
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            }),
        r"Invalid identifier {identifier:?}; expected ^[a-zA-Z0-9-]+(\.[a-zA-Z0-9-]+)+$"
    );
    Ok((name.into(), product_name.into(), version.into(), identifier))
}

pub const FEATURES: [(&str, bool); 6] = [
    ("webgpu", false),
    ("video", false),
    ("audio", false),
    ("jsExecute", false),
    ("httpImport", false),
    ("localNetwork", false),
];

pub fn feature_bit(feature: &str) -> u32 {
    1 << FEATURES
        .iter()
        .position(|(name, _)| *name == feature)
        .expect("Known feature")
}

pub fn disabled_features(source: Option<&str>) -> Result<u32> {
    let package: serde_json::Value =
        serde_json::from_str(source.unwrap_or("{}")).context("Invalid package.json")?;
    let mut disabled = FEATURES
        .iter()
        .enumerate()
        .filter(|(_, (_, enabled))| !enabled)
        .fold(0, |bits, (index, _)| bits | 1 << index);
    let Some(features) = package.get("features") else {
        return Ok(disabled);
    };
    for feature in features
        .as_array()
        .context("package.json features must be an array")?
    {
        let feature = feature
            .as_str()
            .context("package.json features must be strings")?;
        let (name, enable) = feature
            .strip_prefix('-')
            .map_or((feature, true), |name| (name, false));
        let bits = if name == "*" {
            (1 << FEATURES.len()) - 1
        } else {
            1 << FEATURES
                .iter()
                .position(|(known, _)| *known == name)
                .with_context(|| {
                    format!(
                        "Unknown feature {name:?} in package.json; expected *, {}",
                        FEATURES.map(|(name, _)| name).join(", ")
                    )
                })?
        };
        disabled = if enable {
            disabled & !bits
        } else {
            disabled | bits
        };
    }
    Ok(disabled)
}

fn check_entry(files: &BBTreeMap<String, String>) -> Result<()> {
    if !files.contains_key("app.js") {
        let package: serde_json::Value =
            serde_json::from_str(files.get("package.json").map_or("{}", String::as_str))?;
        ensure!(package["type"] == "rust", "Missing app.js");
    }
    Ok(())
}

pub fn write(path: &Path, files: &BBTreeMap<String, String>) -> Result<usize> {
    #[derive(Serialize)]
    struct Packed<'a> {
        files: Vec<(&'a str, &'a [u8])>,
        #[serde(with = "map")]
        pages: BBTreeMap<String, Document>,
    }
    check_entry(files)?;
    let mut owned = Vec::new();
    let mut bundle = Packed {
        files: Vec::new(),
        pages: BBTreeMap::new(),
    };
    for (name, source) in files {
        if name.ends_with(".html") {
            let mut doc = page(source);
            for id in 0..doc.nodes.len() {
                if doc.nodes[id].tag == "style"
                    && let [text] = doc.nodes[id].children[..]
                {
                    let node = &mut doc.nodes[text];
                    node.stylesheet = Some(std::rc::Rc::new(Stylesheet::parse(&node.text)));
                    node.text.clear();
                }
                let node = &mut doc.nodes[id];
                if let Some(source) = node.attrs.remove("style") {
                    node.inline = Some(Declarations::parse(&source));
                }
            }
            bundle.pages.insert(name.clone(), doc);
        } else if name == "native.json" {
            let paths: std::collections::BTreeMap<String, std::path::PathBuf> =
                serde_json::from_str(source)?;
            let mut output = std::collections::BTreeMap::new();
            for (name, source) in paths {
                let relative = format!("node_modules/{name}");
                validate_native_path(&relative)?;
                owned.push((
                    relative.clone(),
                    fs::read(&source).with_context(|| {
                        format!("Cannot read native addon {}", source.display())
                    })?,
                ));
                output.insert(name, relative);
            }
            owned.push((name.clone(), serde_json::to_vec(&output)?));
        } else if name == "assets.json" {
            owned.push((
                name.clone(),
                copy_assets(path.parent().context("Missing bundle directory")?, source)?
                    .into_bytes(),
            ));
        } else {
            ensure!(
                name.ends_with(".js") || name == "package.json",
                "Unsupported bundle file: {name}"
            );
            bundle.files.push((name.as_str(), source.as_bytes()));
        }
    }
    bundle.files.extend(
        owned
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice())),
    );
    let mut bytes = HEADER.to_vec();
    bytes.extend(postcard::to_allocvec(&bundle)?);
    // WARNING: running apps map app.bundle, so a new bundle replaces the file instead of rewriting it.
    let temporary = path.with_extension("bin.tmp");
    fs::write(&temporary, &bytes)
        .and_then(|()| fs::rename(&temporary, path))
        .with_context(|| format!("Cannot write {}", path.display()))?;
    Ok(bytes.len())
}

pub fn read(directory: &Path) -> Result<Bundle> {
    let directory = if directory.join("Contents/Resources").is_dir() {
        directory.join("Contents/Resources")
    } else if !directory.join("app.bundle").try_exists()?
        && !directory.join("app.js").try_exists()?
    {
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
    let path = directory.join("app.bundle");
    let mut bundle = if path.try_exists()? {
        #[derive(Deserialize)]
        struct Mapped<'a> {
            #[serde(borrow)]
            files: Vec<(String, &'a [u8])>,
            #[serde(with = "map")]
            pages: BBTreeMap<String, Document>,
        }
        let file = fs::File::open(&path).context("Cannot read app.bundle")?;
        // WARNING: script chunks stay in the mapped file until they run, so app.bundle must not change
        // while the app runs.
        let bytes = bytes::Bytes::from_owner(
            unsafe { memmap2::Mmap::map(&file) }.context("Cannot read app.bundle")?,
        );
        ensure!(
            bytes.starts_with(HEADER),
            "Invalid or unsupported app.bundle format"
        );
        let (mapped, rest): (Mapped, _) =
            postcard::take_from_bytes(&bytes[HEADER.len()..]).context("Invalid app.bundle data")?;
        ensure!(rest.is_empty(), "Trailing app.bundle data");
        let mut bundle = Bundle {
            pages: mapped.pages,
            ..Bundle::default()
        };
        for (name, source) in mapped.files {
            // WARNING: packed addons stay mapped next to the script chunks, keyed by their node_modules path.
            let duplicate = if name.starts_with("script-") || name.ends_with(".node") {
                bundle
                    .scripts
                    .insert(name, bytes.slice_ref(source))
                    .is_some()
            } else {
                let source =
                    String::from_utf8(source.to_vec()).context("Invalid app.bundle data")?;
                bundle.files.insert(name, source).is_some()
            };
            ensure!(!duplicate, "Duplicate bundle key");
        }
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
                || name.starts_with("script-") && name.ends_with(".js")
                || matches!(
                    name.as_str(),
                    "app.js" | "native.json" | "package.json" | "assets.json"
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
            if !bundle.scripts.contains_key(&relative) {
                *path = directory.join(relative);
            }
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
    check_entry(&bundle.files)?;
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
        style,
    };
    use serde_json::json;

    #[test]
    fn package_metadata_uses_product_name_and_defaults() {
        for (source, name, product_name, version, identifier) in [
            (None, "App", "App", "0.0.0", "com.apps.App"),
            (Some("{}"), "App", "App", "0.0.0", "com.apps.App"),
            (
                Some(r#"{"name":"package_name","version":"1.2.3"}"#),
                "package_name",
                "package_name",
                "1.2.3",
                "com.apps.package-name",
            ),
            (
                Some(r#"{"productName":"My App","name":"package_name","version":"2.0.0"}"#),
                "package_name",
                "My App",
                "2.0.0",
                "com.apps.package-name",
            ),
            (
                Some(r#"{"name":"package_name","identifier":"dev.Tea-Code.app2"}"#),
                "package_name",
                "package_name",
                "0.0.0",
                "dev.Tea-Code.app2",
            ),
            (
                Some(r#"{"productName":"My App"}"#),
                "App",
                "My App",
                "0.0.0",
                "com.apps.App",
            ),
        ] {
            assert_eq!(
                package_metadata(source).unwrap(),
                (
                    name.into(),
                    product_name.into(),
                    version.into(),
                    identifier.into()
                )
            );
        }
        for identifier in [
            "",
            "app",
            ".app",
            "com.",
            "com..app",
            "com.my_app",
            "com.my app",
            "com.aé",
            "com/app",
        ] {
            let error =
                package_metadata(Some(&json!({"identifier":identifier}).to_string())).unwrap_err();
            assert!(
                error.to_string().contains("Invalid identifier"),
                "{identifier:?}"
            );
        }
        for value in [json!(null), json!(42), json!([])] {
            assert!(package_metadata(Some(&json!({"identifier":value}).to_string())).is_err());
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
    fn package_features_enable_and_disable_in_order() {
        for (features, disabled) in [
            (None, 63),
            (Some(json!([])), 63),
            (Some(json!(["webgpu"])), 62),
            (Some(json!(["*"])), 0),
            (Some(json!(["-*"])), 63),
            (Some(json!(["*", "-webgpu"])), 1),
            (Some(json!(["-*", "webgpu"])), 62),
            (Some(json!(["webgpu", "-webgpu"])), 63),
            (Some(json!(["video", "audio"])), 57),
            (Some(json!(["*", "-audio"])), 4),
            (Some(json!(["jsExecute", "httpImport"])), 39),
            (Some(json!(["localNetwork"])), 31),
        ] {
            let package = features.map_or(json!({}), |features| json!({ "features": features }));
            assert_eq!(
                disabled_features(Some(&package.to_string())).unwrap(),
                disabled,
                "{package}"
            );
        }
        for features in [json!("webgpu"), json!([1]), json!(["sqlite"]), json!(["-"])] {
            assert!(disabled_features(Some(&json!({ "features": features }).to_string())).is_err());
        }
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
            ("app.js".into(), "$appsDefine({id:0});".into()),
            ("page-0.html".into(), html.into()),
        ]);
        write(&dir.path().join("app.bundle"), &files).unwrap();
        let mut bundle = read(dir.path()).unwrap();
        assert_eq!(bundle.files.len(), 1);
        let doc = bundle.pages.remove("page-0.html").unwrap();
        let raw = Document::parse(html);
        let sheet = doc.nodes[doc.nodes.iter().position(|n| n.tag == "style").unwrap()].children[0];
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
            let actual = style::styles(&doc, width, height);
            let expected = style::styles(&raw, width, height);
            for (actual, expected) in actual.iter().zip(&expected) {
                assert!(actual.iter().eq(expected.iter()));
            }
            let serialized = Document::parse(&doc.serialize(0));
            let roundtrip = style::styles(&serialized, width, height);
            for (actual, expected) in roundtrip.iter().zip(&expected) {
                assert!(actual.iter().eq(expected.iter()));
            }
        }
        let mut host = Host::default();
        host.pages.insert(1, Page { doc, dirty: false });
        let tree = host
            .call(Role::Renderer(1), "dom.tree", json!([card]))
            .unwrap();
        let tree = tree.as_array().unwrap();
        let style = tree.iter().position(|value| value == "style").unwrap();
        assert!(
            tree[style + 1]
                .as_str()
                .unwrap()
                .contains("height:30px !important")
        );
        for (codes, text, height) in [
            ([4, card as i32, 5, 11], "styleheight:42px", "42px"),
            ([5, card as i32, 5, 0], "style", "20px"),
            ([1, sheet as i32, 19, 0], "#card {height:55px}", "55px"),
        ] {
            let codes: Vec<u8> = codes
                .iter()
                .flat_map(|code: &i32| code.to_ne_bytes())
                .collect();
            assert!(host.batch(1, &codes, text).unwrap().is_null());
            assert_eq!(
                style::styles(&host.pages[&1].doc, 800., 600.)[card]["height"],
                height
            );
        }
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
        let size = write(&dir.path().join("app.bundle"), &files).unwrap();
        assert!(
            size < 10_000,
            "Nested parent selectors were duplicated: {size}"
        );
        let bundle = read(dir.path()).unwrap();
        assert_eq!(
            style::styles(&bundle.pages["page-0.html"], 800., 600.)
                .last()
                .unwrap()["width"],
            "1px"
        );
    }

    #[test]
    fn script_chunks_stay_in_the_mapped_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.bundle");
        let files = BBTreeMap::from([
            ("app.js".into(), "$appsDefine({id:0});".into()),
            ("script-0.js".into(), "globalThis.chunk = '你好';".into()),
        ]);
        write(&path, &files).unwrap();
        let bundle = read(dir.path()).unwrap();
        assert_eq!(bundle.files.keys().collect::<Vec<_>>(), ["app.js"]);
        assert_eq!(
            &bundle.scripts["script-0.js"][..],
            files["script-0.js"].as_bytes()
        );
        write(&path, &BBTreeMap::from([("app.js".into(), String::new())])).unwrap();
        assert_eq!(
            &bundle.scripts["script-0.js"][..],
            files["script-0.js"].as_bytes()
        );
    }

    #[test]
    fn rejects_corrupt_bundles_and_invalid_document_links() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.bundle");
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
