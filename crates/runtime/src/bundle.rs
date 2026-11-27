use crate::{
    css::{sheet::Declarations, sheet::Stylesheet},
    dom::Document,
};
use anyhow::{Context, Result, ensure};
use bbtree::unsync::BBTreeMap;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, path::Path};

const HEADER: &[u8; 8] = b"APP\0\x04\0\0\0";

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

pub fn unpack_page(bytes: &[u8]) -> Result<Document> {
    let mut doc: Document = postcard::from_bytes(bytes).context("Invalid page data")?;
    ensure!(!doc.nodes.is_empty(), "Missing document root");
    ensure!(
        doc.nodes[0].tag == "#document" && doc.nodes[0].parent.is_none(),
        "Invalid document root"
    );
    // The adoption agency can give a child a lower id than its parent, so walk the tree from the root.
    let mut seen = vec![false; doc.nodes.len()];
    seen[0] = true;
    let mut stack = vec![0];
    while let Some(id) = stack.pop() {
        for &child in &doc.nodes[id].children {
            ensure!(child < doc.nodes.len(), "Invalid document child");
            ensure!(
                !seen[child] && doc.nodes[child].parent == Some(id),
                "Invalid document parent"
            );
            seen[child] = true;
            stack.push(child);
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
    // Packed pages of a built app. A window unpacks its page when it loads it.
    #[serde(skip)]
    pub pages: HashMap<String, bytes::Bytes>,
    #[serde(skip)]
    pub scripts: HashMap<String, bytes::Bytes>,
    // The main script of a built app, ready to run.
    #[serde(skip)]
    pub main: Option<bytes::Bytes>,
}

// The fields of package.json that the runtime and the build use.
pub struct Package {
    pub name: String,
    pub product_name: String,
    pub version: String,
    pub identifier: String,
    pub features: Features,
    // The bundler found system web views in the app code.
    pub webview: bool,
}

impl Package {
    pub fn parse(source: Option<&str>) -> Result<Self> {
        let package: serde_json::Value =
            serde_json::from_str(source.unwrap_or("{}")).context("Invalid package.json")?;
        ensure!(package.is_object(), "Expected an object in package.json");
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
                && name.bytes().skip(1).all(|byte| byte.is_ascii_alphanumeric()
                    || byte == b'_'
                    || rust && byte == b'-'),
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
        let features = Features::read(&package)?;
        Ok(Self {
            name: name.into(),
            product_name: product_name.into(),
            version: version.into(),
            identifier,
            features,
            webview: package["webview"] == true,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Feature {
    WebGpu,
    Media,
    JsExecute,
    HttpImport,
    LocalNetwork,
    ReactCompiler,
    Terminal,
}

impl Feature {
    pub const ALL: [Self; 7] = [
        Self::WebGpu,
        Self::Media,
        Self::JsExecute,
        Self::HttpImport,
        Self::LocalNetwork,
        Self::ReactCompiler,
        Self::Terminal,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::WebGpu => "webgpu",
            Self::Media => "media",
            Self::JsExecute => "jsExecute",
            Self::HttpImport => "httpImport",
            Self::LocalNetwork => "localNetwork",
            Self::ReactCompiler => "reactCompiler",
            Self::Terminal => "terminal",
        }
    }
}

// The enabled features. Every feature is off unless package.json turns it on. The default, for hosts without a
// package, has all of them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Features(u32);

impl Default for Features {
    fn default() -> Self {
        Self((1 << Feature::ALL.len()) - 1)
    }
}

impl Features {
    pub const NONE: Self = Self(0);

    pub fn has(self, feature: Feature) -> bool {
        self.0 & 1 << feature as u32 != 0
    }

    pub fn without(self, feature: Feature) -> Self {
        Self(self.0 & !(1 << feature as u32))
    }

    // The features of a package.json, without checking its other fields.
    pub fn parse(source: Option<&str>) -> Result<Self> {
        let package: serde_json::Value =
            serde_json::from_str(source.unwrap_or("{}")).context("Invalid package.json")?;
        Self::read(&package)
    }

    fn read(package: &serde_json::Value) -> Result<Self> {
        let all = (1 << Feature::ALL.len()) - 1;
        let mut enabled = 0;
        let Some(features) = package.get("features") else {
            return Ok(Self(enabled));
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
                all
            } else {
                1 << Feature::ALL
                    .iter()
                    .position(|known| known.name() == name)
                    .with_context(|| {
                        format!(
                            "Unknown feature {name:?} in package.json; expected *, {}",
                            Feature::ALL.map(Feature::name).join(", ")
                        )
                    })?
            };
            enabled = if enable {
                enabled | bits
            } else {
                enabled & !bits
            };
        }
        Ok(Self(enabled))
    }
}

// What the runtime knows about the app from its package.
#[derive(Clone, Default)]
pub struct AppInfo {
    // The product name, which `app.getName()` returns.
    pub name: Option<String>,
    pub version: Option<String>,
    pub path: std::path::PathBuf,
    pub packaged: bool,
    pub rust_main: bool,
    pub features: Features,
    pub webview: bool,
    pub scripts: std::sync::Arc<HashMap<String, bytes::Bytes>>,
    pub assets: std::sync::Arc<HashMap<String, std::path::PathBuf>>,
    pub native_modules: HashMap<String, std::path::PathBuf>,
}

impl AppInfo {
    // Takes the scripts of the bundle. `files` are its other files.
    pub fn new(bundle: &mut Bundle, package: &Package, rust_main: bool) -> Result<Self> {
        let assets = bundle
            .files
            .get("assets.json")
            .map(|source| serde_json::from_str(source))
            .transpose()
            .context("Invalid assets.json")?
            .unwrap_or_default();
        let mut app = Self {
            name: Some(package.product_name.clone()),
            version: Some(package.version.clone()),
            path: if bundle.root.as_os_str().is_empty() {
                std::env::current_dir().unwrap_or_default()
            } else {
                bundle.root.clone()
            },
            packaged: bundle.packaged,
            rust_main,
            features: package.features,
            webview: package.webview,
            scripts: std::sync::Arc::new(std::mem::take(&mut bundle.scripts)),
            assets: std::sync::Arc::new(assets),
            native_modules: HashMap::new(),
        };
        app.install(&bundle.files)?;
        Ok(app)
    }

    // Adds the native addons and script chunks of a development bundle.
    pub fn install(&mut self, files: &BBTreeMap<String, String>) -> Result<()> {
        let paths: HashMap<String, std::path::PathBuf> =
            serde_json::from_str(files.get("native.json").map_or("{}", String::as_str))?;
        self.native_modules.extend(paths);
        let mut scripts = files
            .iter()
            .filter(|(name, _)| name.starts_with("script-"))
            .peekable();
        if scripts.peek().is_some() {
            std::sync::Arc::make_mut(&mut self.scripts)
                .extend(scripts.map(|(name, source)| (name.clone(), source.clone().into())));
        }
        Ok(())
    }
}

// The files a page may name: its bundled assets, and local files only for a page that is a local file itself. A
// file URL with a host is a network share, which is never read.
pub(crate) fn local_path(
    assets: &HashMap<String, std::path::PathBuf>,
    base: &url::Url,
    url: &url::Url,
) -> Option<std::path::PathBuf> {
    match url.scheme() {
        "apps" => assets.get(url.path().trim_start_matches('/')).cloned(),
        "file" if base.scheme() == "file" && url.host_str().is_none_or(str::is_empty) => {
            url.to_file_path().ok()
        }
        _ => None,
    }
}

pub(crate) fn local_file(
    assets: &HashMap<String, std::path::PathBuf>,
    base: Option<&url::Url>,
    url: &str,
) -> Result<Option<std::path::PathBuf>> {
    let base = base.context("Missing page URL")?;
    Ok(local_path(assets, base, &base.join(url)?))
}

fn check_entry(main: bool, package: Option<&String>) -> Result<()> {
    if !main {
        let package: serde_json::Value =
            serde_json::from_str(package.map_or("{}", String::as_str))?;
        ensure!(package["type"] == "rust", "Missing app.js");
    }
    Ok(())
}

pub fn write(
    path: &Path,
    files: &BBTreeMap<String, String>,
    bytecode: &[(&str, &[u8])],
) -> Result<usize> {
    #[derive(Serialize)]
    struct Packed<'a> {
        files: Vec<(&'a str, &'a [u8])>,
        pages: Vec<(&'a str, Vec<u8>)>,
    }
    check_entry(files.contains_key("app.js"), files.get("package.json"))?;
    let mut owned = Vec::new();
    let mut bundle = Packed {
        files: Vec::new(),
        pages: Vec::new(),
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
                if let Some(source) = node.attrs.get("style") {
                    node.inline = Some(Declarations::parse(source));
                }
            }
            bundle.pages.push((name, postcard::to_allocvec(&doc)?));
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
        } else if name == "app.js" {
            // The bundle holds the main script as it runs, so the app reads it in place.
            owned.push((
                name.clone(),
                crate::jsc::script_source(files, true).into_bytes(),
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
    // Align bytecode to 8 bytes: JSC reads it in place.
    if !bytecode.is_empty() {
        let index: Vec<_> = bytecode
            .iter()
            .map(|(url, data)| (*url, data.len()))
            .collect();
        bytes.extend(postcard::to_allocvec(&index)?);
        for (_, data) in bytecode {
            bytes.resize(bytes.len().next_multiple_of(8), 0);
            bytes.extend_from_slice(data);
        }
    }
    // Running apps map app.bundle, so a new bundle replaces the file instead of rewriting it.
    let temporary = path.with_extension("bin.tmp");
    fs::write(&temporary, &bytes)
        .and_then(|()| fs::rename(&temporary, path))
        .with_context(|| format!("Cannot write {}", path.display()))?;
    Ok(bytes.len())
}

// Replace stdin with /dev/null so child processes cannot read bundles or keep the bundle pipe open.
pub fn from_stdin() -> Result<(Bundle, impl std::io::BufRead + Send + 'static)> {
    #[cfg(unix)]
    let input = unsafe {
        use std::os::fd::FromRawFd;
        let fd = libc::fcntl(0, libc::F_DUPFD_CLOEXEC, 64);
        ensure!(fd >= 0, "Cannot read stdin");
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY);
        if null >= 0 {
            libc::dup2(null, 0);
            libc::close(null);
        }
        fs::File::from_raw_fd(fd)
    };
    #[cfg(not(unix))]
    let input = std::io::stdin();
    let mut input = std::io::BufReader::new(input);
    let mut line = Vec::new();
    std::io::BufRead::read_until(&mut input, b'\n', &mut line)?;
    Ok((serde_json::from_slice(&line)?, input))
}

pub fn read(directory: &Path) -> Result<Bundle> {
    let directory = if directory.join("Contents/Resources").is_dir() {
        directory.join("Contents/Resources")
    } else if !directory.join("app.bundle").try_exists()? {
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
    #[derive(Deserialize)]
    struct Mapped<'a> {
        #[serde(borrow)]
        files: Vec<(String, &'a [u8])>,
        #[serde(borrow)]
        pages: Vec<(String, &'a [u8])>,
    }
    struct Owner(std::sync::Arc<memmap2::Mmap>);
    impl AsRef<[u8]> for Owner {
        fn as_ref(&self) -> &[u8] {
            &self.0
        }
    }
    let file = fs::File::open(&path).context("Cannot read app.bundle")?;
    // Do not modify app.bundle while it runs: scripts and pages reference the mapped file.
    let map = std::sync::Arc::new(
        unsafe { memmap2::Mmap::map(&file) }.context("Cannot read app.bundle")?,
    );
    crate::jsc::add_mapped(std::sync::Arc::downgrade(&map));
    let bytes = bytes::Bytes::from_owner(Owner(map));
    ensure!(
        bytes.starts_with(HEADER),
        "Invalid or unsupported app.bundle format"
    );
    let (mapped, rest): (Mapped, _) =
        postcard::take_from_bytes(&bytes[HEADER.len()..]).context("Invalid app.bundle data")?;
    let mut bytecode = Vec::new();
    if !rest.is_empty() {
        let (index, rest): (Vec<(&str, usize)>, _) =
            postcard::take_from_bytes(rest).context("Invalid app.bundle data")?;
        ensure!(!index.is_empty(), "Trailing app.bundle data");
        let mut at = bytes.len() - rest.len();
        for (name, size) in index {
            at = at.next_multiple_of(8);
            let data = bytes.get(at..).and_then(|data| data.get(..size));
            let data = data.context("Invalid app.bundle data")?;
            ensure!(
                data.as_ptr().cast::<u64>().is_aligned(),
                "Invalid app.bundle data"
            );
            bytecode.push((name, at..at + size));
            at += size;
        }
        ensure!(at == bytes.len(), "Trailing app.bundle data");
    }
    let mut bundle = Bundle::default();
    for (name, page) in mapped.pages {
        ensure!(
            bundle.pages.insert(name, bytes.slice_ref(page)).is_none(),
            "Duplicate bundle key"
        );
    }
    for (name, source) in mapped.files {
        let duplicate = if name == "app.js" {
            bundle.main.replace(bytes.slice_ref(source)).is_some()
        } else if name.starts_with("script-") || name.ends_with(".node") {
            bundle
                .scripts
                .insert(name, bytes.slice_ref(source))
                .is_some()
        } else {
            let source = String::from_utf8(source.to_vec()).context("Invalid app.bundle data")?;
            bundle.files.insert(name, source).is_some()
        };
        ensure!(!duplicate, "Duplicate bundle key");
    }
    // JSC reads bytecode and script chunks in place, so the mapping stays for the rest of the process.
    let mapped = |data: &[u8]| unsafe { std::slice::from_raw_parts(data.as_ptr(), data.len()) };
    let pinned = !bytecode.is_empty();
    for (name, range) in bytecode {
        let source = if name == "app.js" {
            bundle.main.as_ref()
        } else {
            bundle.scripts.get(name)
        };
        let source = source.and_then(|source| std::str::from_utf8(mapped(source)).ok());
        crate::jsc::add_bytecode(
            Box::leak(name.into()),
            source.unwrap_or(""),
            mapped(&bytes[range]),
        );
    }
    if pinned {
        std::mem::forget(bytes.clone());
    }
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
    bundle.packaged = true;
    bundle.root = directory;
    check_entry(bundle.main.is_some(), bundle.files.get("package.json"))?;
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

fn copy_assets(directory: &Path, source: &str) -> Result<String> {
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
    use crate::{host::Host, jsc::Role, render::Page, style};
    use serde_json::json;

    fn metadata(source: Option<&str>) -> Result<(String, String, String, String)> {
        Package::parse(source).map(|p| (p.name, p.product_name, p.version, p.identifier))
    }

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
                metadata(source).unwrap(),
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
            let error = metadata(Some(&json!({"identifier":identifier}).to_string())).unwrap_err();
            assert!(
                error.to_string().contains("Invalid identifier"),
                "{identifier:?}"
            );
        }
        for value in [json!(null), json!(42), json!([])] {
            assert!(metadata(Some(&json!({"identifier":value}).to_string())).is_err());
        }
        for name in ["Aa", "a_", "A1", "my_App12"] {
            assert_eq!(
                metadata(Some(&json!({"name":name}).to_string())).unwrap().0,
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
            let error = metadata(Some(&json!({"name":name}).to_string())).unwrap_err();
            assert!(
                error.to_string().contains("^[a-zA-Z][a-zA-Z0-9_]+$"),
                "{name:?}"
            );
        }
        for value in [json!(null), json!(42), json!([])] {
            assert!(metadata(Some(&json!({"name":value}).to_string())).is_err());
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
            assert!(metadata(Some(&json!({"productName":name}).to_string())).is_err());
        }
        for value in [json!(null), json!(42), json!([])] {
            assert!(metadata(Some(&json!({"productName":value}).to_string())).is_err());
        }
        assert!(metadata(Some("{")).is_err());
        assert!(metadata(Some("[]")).is_err());
    }

    #[test]
    fn package_features_enable_and_disable_in_order() {
        for (features, disabled) in [
            (None, 127),
            (Some(json!([])), 127),
            (Some(json!(["webgpu"])), 126),
            (Some(json!(["*"])), 0),
            (Some(json!(["-*"])), 127),
            (Some(json!(["*", "-webgpu"])), 1),
            (Some(json!(["-*", "webgpu"])), 126),
            (Some(json!(["webgpu", "-webgpu"])), 127),
            (Some(json!(["webgpu", "media"])), 124),
            (Some(json!(["*", "-media"])), 2),
            (Some(json!(["jsExecute", "httpImport"])), 115),
            (Some(json!(["localNetwork"])), 111),
            (Some(json!(["reactCompiler"])), 95),
            (Some(json!(["terminal"])), 63),
        ] {
            let package = features.map_or(json!({}), |features| json!({ "features": features }));
            assert_eq!(
                127 & !Package::parse(Some(&package.to_string()))
                    .unwrap()
                    .features
                    .0,
                disabled,
                "{package}"
            );
        }
        for features in [json!("webgpu"), json!([1]), json!(["sqlite"]), json!(["-"])] {
            assert!(Package::parse(Some(&json!({ "features": features }).to_string())).is_err());
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
                metadata(Some(&json!({"version":version}).to_string()))
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
            let error = metadata(Some(&json!({"version":version}).to_string())).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(r"^\d+\.\d+\.\d+(-[a-z0-9._-]+)?$"),
                "{version:?}"
            );
        }
        for version in [json!(null), json!(123), json!([]), json!({})] {
            assert!(
                metadata(Some(&json!({"version":version}).to_string()))
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
            [style*=height] > .label { margin-left:2px; }
        </style></head><body><div id="card" class="card active" style="height:30px !important; @apply p-4;"><span class="label">你好 &amp; hello</span></div></body></html>"#;
        let dir = tempfile::tempdir().unwrap();
        let files = BBTreeMap::from([
            ("app.js".into(), "$appsDefine({id:0});".into()),
            ("page-0.html".into(), html.into()),
        ]);
        write(&dir.path().join("app.bundle"), &files, &[]).unwrap();
        let bundle = read(dir.path()).unwrap();
        assert!(bundle.files.is_empty() && bundle.main.is_some());
        let doc = unpack_page(&bundle.pages["page-0.html"]).unwrap();
        let raw = Document::parse(html);
        let sheet = doc.nodes[doc.nodes.iter().position(|n| n.tag == "style").unwrap()].children[0];
        let card = doc
            .nodes
            .iter()
            .position(|n| n.attrs.get("id").is_some_and(|id| id == "card"))
            .unwrap();
        assert!(doc.nodes[sheet].text.is_empty());
        assert!(doc.nodes[sheet].stylesheet.is_some());
        assert!(doc.nodes[card].attrs.contains_key("style"));
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
        host.store.borrow_mut().pages.insert(
            1,
            Page {
                doc,
                dirty: false,
                ..Default::default()
            },
        );
        let tree = host
            .call(Role::Renderer(1), "dom.tree", json!([card]))
            .unwrap();
        let tree: Vec<serde_json::Value> = serde_json::from_str(tree.as_str().unwrap()).unwrap();
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
                style::styles(&host.store.borrow().pages[&1].doc, 800., 600.)[card]["height"],
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
        write(&dir.path().join("app.bundle"), &files, &[]).unwrap();
        let page = &read(dir.path()).unwrap().pages["page-0.html"];
        assert!(
            page.len() < 10_000,
            "Nested parent selectors were duplicated: {}",
            page.len()
        );
        assert_eq!(
            style::styles(&unpack_page(page).unwrap(), 800., 600.)
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
        write(&path, &files, &[]).unwrap();
        let bundle = read(dir.path()).unwrap();
        assert!(bundle.files.is_empty());
        assert_eq!(
            &bundle.main.as_ref().unwrap()[..],
            crate::jsc::script_source(&files, true).as_bytes()
        );
        assert_eq!(
            &bundle.scripts["script-0.js"][..],
            files["script-0.js"].as_bytes()
        );
        write(
            &path,
            &BBTreeMap::from([("app.js".into(), String::new())]),
            &[],
        )
        .unwrap();
        assert_eq!(
            &bundle.main.as_ref().unwrap()[..],
            crate::jsc::script_source(&files, true).as_bytes()
        );
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
        write(&path, &files, &[]).unwrap();
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
        let page = read(dir.path())
            .unwrap()
            .pages
            .remove("page-0.html")
            .unwrap();
        for length in 0..page.len() {
            assert!(unpack_page(&page[..length]).is_err(), "Accepted {length}");
        }
        let mut doc = unpack_page(&page).unwrap();
        doc.nodes[0].children.push(usize::MAX);
        assert!(unpack_page(&postcard::to_allocvec(&doc).unwrap()).is_err());
    }

    #[test]
    fn misnested_formatting_tags_survive_the_bundle() {
        let html = "<body><b><i><div>x</b></body>";
        let dir = tempfile::tempdir().unwrap();
        let files = BBTreeMap::from([
            ("app.js".into(), "$appsDefine({id:0});".into()),
            ("page-0.html".into(), html.into()),
        ]);
        write(&dir.path().join("app.bundle"), &files, &[]).unwrap();
        let page = read(dir.path())
            .unwrap()
            .pages
            .remove("page-0.html")
            .unwrap();
        let doc = unpack_page(&page).unwrap();
        let raw = Document::parse(html);
        assert_eq!(doc.nodes.len(), raw.nodes.len());
        for (a, b) in doc.nodes.iter().zip(&raw.nodes) {
            assert_eq!(
                (&a.tag, a.parent, &a.children),
                (&b.tag, b.parent, &b.children)
            );
        }
        let div = doc.nodes.iter().position(|n| n.tag == "div").unwrap();
        assert!(doc.nodes[div].parent.unwrap() > div);
        let mut cycle = unpack_page(&page).unwrap();
        let parent = cycle.nodes[div].parent.unwrap();
        cycle.nodes[div].children.push(parent);
        assert!(unpack_page(&postcard::to_allocvec(&cycle).unwrap()).is_err());
        let mut cycle = unpack_page(&page).unwrap();
        let (a, b) = (cycle.nodes.len(), cycle.nodes.len() + 1);
        for other in [b, a] {
            let mut node = cycle.nodes[div].clone();
            node.parent = Some(other);
            node.children = vec![other];
            cycle.nodes.push(node);
        }
        assert!(unpack_page(&postcard::to_allocvec(&cycle).unwrap()).is_err());
    }

    #[test]
    fn scope_roots_inside_functions_survive_the_bundle() {
        let html = "<html><head><style>@scope (.card) { :is(:scope) > p { color: red } :not(:scope) > b { color: blue } }</style></head><body><div class=card><p>x</p><span><b>y</b></span></div><p>z</p></body></html>";
        let dir = tempfile::tempdir().unwrap();
        let files = BBTreeMap::from([
            ("app.js".into(), "$appsDefine({id:0});".into()),
            ("page-0.html".into(), html.into()),
        ]);
        write(&dir.path().join("app.bundle"), &files, &[]).unwrap();
        let doc = unpack_page(&read(dir.path()).unwrap().pages["page-0.html"]).unwrap();
        let colors = |doc: &Document| {
            let styles = style::styles(doc, 800., 600.);
            (0..doc.nodes.len())
                .filter(|&id| matches!(&*doc.nodes[id].tag, "p" | "b"))
                .map(|id| styles[id].get("color").cloned().unwrap_or_default())
                .collect::<Vec<_>>()
        };
        let raw = colors(&Document::parse(html));
        assert_eq!(raw[0], "red");
        assert_eq!(raw[1], "blue");
        assert_eq!(colors(&doc), raw);
    }
}
