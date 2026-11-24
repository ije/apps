use anyhow::{Context, Result, bail, ensure};
use apps_runtime::{
    bundle::script_source,
    dom::Document,
    host::{Role, Waker},
    jsc::{JavaScriptCore, JsContext},
};
use bbtree::unsync::BBTreeMap;
use emit::{Export, Import, Module, Slots, Uses, factory, in_cycles};
use html::html;
use oxc_allocator::{Allocator, ArenaVec, TakeIn};
use oxc_ast::{ast::*, builder::AstBuilder};
use oxc_ast_visit::{Visit, VisitMut, walk, walk_mut};
use oxc_codegen::{Codegen, CodegenOptions, CommentOptions};
use oxc_minifier::{
    CompressOptions, Minifier, MinifierOptions, MinifierReturn, PropertyReadSideEffects,
    TreeShakeOptions,
};
use oxc_parser::Parser;
use oxc_react_compiler::{CompileResult, PluginOptions};
use oxc_semantic::SemanticBuilder;
use oxc_span::{GetSpan, SourceType, Span};
use oxc_transformer::{TransformOptions, Transformer};
use resolve::remote_base;
use resolve::{entry_fields, native_specifier, resolve_file, tsconfig_paths};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeSet, HashMap, VecDeque},
    fmt::Write,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Realm {
    Main,
    Preload,
    Renderer,
}

struct Builder {
    realm: Realm,
    modules: Vec<Module>,
    lines: Vec<Option<Rc<str>>>,
    ids: BBTreeMap<PathBuf, usize>,
    graph: Graph,
    requests: Vec<(PathBuf, String, bool, usize)>,
    inlined: usize,
    package_roots: BTreeSet<PathBuf>,
    dynamic: BTreeSet<usize>,
    shared: Rc<Shared>,
}

#[derive(Default)]
struct Shared {
    root: PathBuf,
    files: Rc<RefCell<BTreeSet<PathBuf>>>,
    production: bool,
    meta_env: Value,
    pages: RefCell<BBTreeMap<PathBuf, (String, String)>>,
    manifests: RefCell<BBTreeMap<PathBuf, Rc<Value>>>,
    workers: RefCell<BBTreeMap<(PathBuf, Realm), usize>>,
    scripts: RefCell<Vec<String>>,
    cache: Option<Rc<Cache>>,
    disabled: u32,
    aliases: Option<(PathBuf, Vec<String>)>,
    preload: Option<PathBuf>,
    loaders: Option<Rc<Loaders>>,
    process_entries: RefCell<BTreeSet<PathBuf>>,
    names: RefCell<HashMap<String, PathBuf>>,
    tsconfigs: RefCell<HashMap<PathBuf, Option<Rc<TsPaths>>>>,
}

struct TsPaths {
    base: PathBuf,
    paths: serde_json::Map<String, Value>,
}

struct Loaders {
    modules: Vec<(String, String)>,
    files: BBTreeMap<String, String>,
    engine: JsContext,
    woken: std::sync::mpsc::Receiver<()>,
}

impl Shared {
    fn watch(&self, path: PathBuf) {
        self.files.borrow_mut().insert(path);
    }

    fn watch_all(&self, paths: impl IntoIterator<Item = PathBuf>) {
        self.files.borrow_mut().extend(paths);
    }

    fn app_file(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.root.join(relative)
    }

    fn in_app(&self, path: &Path) -> bool {
        path.starts_with(&self.root)
    }

    fn is_preload(&self, path: &Path) -> bool {
        self.preload.as_deref() == Some(path)
    }

    fn has_preload(&self) -> bool {
        self.preload.is_some()
    }

    // The code of a page that gives the exports of the package preload to the page.
    fn preload_import(&self) -> Option<String> {
        let preload = self.preload.as_ref()?;
        Some(format!(
            "import * as api from {};globalThis.__appsPublishPreload(api);",
            json!(preload)
        ))
    }

    // `import.meta.env`, or one of its values.
    fn meta_env(&self, key: Option<&str>) -> String {
        match key {
            Some(key) => self.meta_env[key].to_string(),
            None => self.meta_env.to_string(),
        }
    }

    fn is_process_entry(&self, path: &Path) -> bool {
        self.process_entries.borrow().contains(path)
    }

    fn add_process_entry(&self, path: PathBuf) {
        self.process_entries.borrow_mut().insert(path);
    }

    // A watch build keeps modules and resolutions in its cache.
    fn watching(&self) -> bool {
        self.cache.is_some()
    }

    fn resolution(&self, key: &ResolutionKey) -> Option<Resolved> {
        self.cache
            .as_ref()
            .and_then(|cache| cache.resolutions.borrow().get(key).cloned())
    }

    fn remember(&self, key: ResolutionKey, resolved: Resolved) {
        if let Some(cache) = &self.cache {
            cache.resolutions.borrow_mut().insert(key, resolved);
        }
    }

    fn take_graph(&self, key: &(Realm, PathBuf)) -> Graph {
        self.cache
            .as_ref()
            .and_then(|cache| cache.graphs.borrow_mut().remove(key))
            .unwrap_or_default()
    }

    fn store_graph(&self, key: (Realm, PathBuf), graph: Graph) {
        if let Some(cache) = &self.cache {
            cache.graphs.borrow_mut().insert(key, graph);
        }
    }

    // The metadata of a package.json, read once per build.
    fn manifest(&self, file: &Path) -> Result<Rc<Value>> {
        if let Some(metadata) = self.manifests.borrow().get(file) {
            return Ok(metadata.clone());
        }
        self.watch(file.to_owned());
        let metadata = Rc::new(
            serde_json::from_str::<Value>(
                &fs::read_to_string(file)
                    .with_context(|| format!("Cannot read {}", file.display()))?,
            )
            .with_context(|| format!("Invalid package metadata in {}", file.display()))?,
        );
        self.manifests
            .borrow_mut()
            .insert(file.to_owned(), metadata.clone());
        Ok(metadata)
    }

    // The nearest package above a file that this build read, with its folder.
    fn package_of<'p>(&self, path: &'p Path) -> Option<(&'p Path, Rc<Value>)> {
        let manifests = self.manifests.borrow();
        path.ancestors()
            .skip(1)
            .find_map(|root| Some((root, manifests.get(&root.join("package.json"))?.clone())))
    }

    // Adds a script chunk. Returns its index.
    fn script(&self, code: String) -> usize {
        let mut scripts = self.scripts.borrow_mut();
        scripts.push(code);
        scripts.len() - 1
    }

    // The script chunk of a worker or a script file, built once per realm.
    fn chunk(self: &Rc<Self>, path: PathBuf, realm: Realm) -> Result<usize> {
        let key = (path, realm);
        if let Some(index) = self.workers.borrow().get(&key).copied() {
            return Ok(index);
        }
        // Reserve the index first: the chunk can reach itself again.
        let index = self.script(String::new());
        self.workers.borrow_mut().insert(key.clone(), index);
        let app = bundle(vec![(key.0, None)], key.1, self.clone())?;
        self.scripts.borrow_mut()[index] =
            script_source(&BBTreeMap::from([("app.js".into(), app)]), true);
        Ok(index)
    }

    // The name of a bundled page.
    fn page(self: &Rc<Self>, target: PathBuf) -> Result<String> {
        let known = self
            .pages
            .borrow()
            .get(&target)
            .map(|(name, _)| name.clone());
        Ok(match known {
            Some(name) => name,
            None => {
                self.watch(target.clone());
                // Reserve the name first: the page's own workers can resolve the page again.
                // Keep the app path, so `location.pathname` is what routers expect.
                let name = target
                    .strip_prefix(&self.root)
                    .ok()
                    .and_then(|path| path.to_str())
                    .map(|path| path.replace('\\', "/"))
                    .filter(|path| {
                        path.bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"._-/".contains(&byte))
                            && !path.split('/').any(|part| part.is_empty() || part == "..")
                    })
                    // `~` can't be in an app path above, so this name is unique.
                    .unwrap_or_else(|| format!("page~{}.html", self.pages.borrow().len()));
                self.pages
                    .borrow_mut()
                    .insert(target.clone(), (name.clone(), String::new()));
                let source = html(&target, self)?;
                self.pages.borrow_mut().get_mut(&target).unwrap().1 = source;
                name
            }
        })
    }

    // File names must be unique: they key `require.cache`, `import.meta.url` and `__filename`.
    fn app_name(&self, path: &Path) -> Result<PathBuf> {
        let relative = app_relative(&self.root, path);
        let name = relative.to_string_lossy().replace('\\', "/");
        let mut names = self.names.borrow_mut();
        let previous = names.entry(name.clone()).or_insert_with(|| path.to_owned());
        ensure!(
            previous == path,
            "Output name collision at {name}: {} and {}",
            previous.display(),
            path.display()
        );
        Ok(relative)
    }

    fn asset_name(&self, path: &Path) -> Option<String> {
        let extension = path
            .extension()
            .filter(|e| is_asset(e))?
            .to_str()?
            .to_ascii_lowercase();
        let hash = app_relative(&self.root, path)
            .as_os_str()
            .as_encoded_bytes()
            .iter()
            .map(|&byte| {
                if byte == std::path::MAIN_SEPARATOR as u8 {
                    b'/'
                } else {
                    byte
                }
            })
            .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3)
            });
        Some(format!("assets/{hash:016x}.{extension}"))
    }

    fn asset(&self, directory: &Path, reference: &str) -> Option<String> {
        let root = &self.root;
        let reference = reference.trim().trim_matches(['"', '\'']);
        if reference.contains(':') || reference.starts_with('#') {
            return None;
        }
        if let Some(base) = remote_base(directory) {
            return base.join(reference).ok().map(String::from);
        }
        let reference = reference.replace('\\', "/");
        if reference.starts_with("//") {
            return None;
        }
        let file = reference.split(['?', '#']).next()?;
        let path = match file.strip_prefix('/') {
            Some(rest) => root.join(rest),
            None => directory.join(file),
        }
        .canonicalize()
        .ok()
        .filter(|path| !reference.starts_with('/') || path.starts_with(root))?;
        let name = self.asset_name(&path)?;
        self.watch(path);
        Some(format!("apps:///{name}"))
    }

    fn local(&self, directory: &Path, reference: &str) -> Result<PathBuf> {
        let root = &self.root;
        // Treat `\` as `/`. Root-relative and absolute paths must stay inside the app folder.
        let reference = reference.replace('\\', "/");
        let rooted = !matches!(
            Path::new(&reference).components().next(),
            Some(
                std::path::Component::Normal(_)
                    | std::path::Component::CurDir
                    | std::path::Component::ParentDir
            )
        );
        let path = match reference.strip_prefix('/') {
            Some(rest) => root.join(rest),
            None => directory.join(&reference),
        };
        let path = path
            .canonicalize()
            .with_context(|| format!("Cannot find {reference} from {}", directory.display()))?;
        ensure!(
            !rooted || path.starts_with(root),
            "{reference} is outside the app folder"
        );
        Ok(path)
    }

    // Adds the pages, script chunks, native addons, assets and package metadata to the output.
    fn finish(&self, files: &mut BBTreeMap<String, String>, main: Option<&Path>) -> Result<()> {
        for (_, (name, source)) in self.pages.take() {
            files.insert(name, source);
        }
        for (index, code) in self.scripts.take().into_iter().enumerate() {
            files.insert(format!("script-{index}.js"), code);
        }
        let sources = &mut *self.files.borrow_mut();
        let mut native = std::collections::BTreeMap::new();
        for path in sources.iter().filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "node")
        }) {
            let name = native_specifier(path)?;
            if let Some(previous) = native.insert(name.clone(), path.clone()) {
                ensure!(
                    previous == *path,
                    "Native addon output collision at {name}: {} and {}",
                    previous.display(),
                    path.display()
                );
            }
        }
        if !native.is_empty() {
            files.insert("native.json".into(), serde_json::to_string(&native)?);
        }
        let mut assets = std::collections::BTreeMap::new();
        for path in sources.iter() {
            if let Some(name) = self.asset_name(path)
                && let Some(previous) = assets.insert(name.clone(), path.clone())
            {
                bail!(
                    "Asset output collision at {name}: {} and {}",
                    previous.display(),
                    path.display()
                );
            }
        }
        if !assets.is_empty() {
            files.insert("assets.json".into(), serde_json::to_string(&assets)?);
        }
        if let Some(root) = main {
            let mut package = json!({});
            for directory in root.ancestors() {
                let path = directory.join("package.json");
                if !path.try_exists()? {
                    continue;
                }
                let source = fs::read_to_string(&path)
                    .with_context(|| format!("Cannot read {}", path.display()))?;
                package = serde_json::from_str(&source)
                    .with_context(|| format!("Invalid package metadata in {}", path.display()))?;
                ensure!(
                    package.is_object(),
                    "Expected an object in {}",
                    path.display()
                );
                sources.insert(path);
                break;
            }
            if package.get("name").is_none() {
                let path = root.join("index.html");
                if path.try_exists()? {
                    let source = fs::read_to_string(&path)
                        .with_context(|| format!("Cannot read {}", path.display()))?;
                    if let Some(title) = Document::parse(&source)
                        .title()
                        .filter(|title| !title.is_empty())
                    {
                        for key in ["name", "productName"] {
                            if package.get(key).is_none() {
                                let mut candidate = package.clone();
                                candidate[key] = title.as_str().into();
                                if apps_runtime::bundle::package_metadata(Some(
                                    &candidate.to_string(),
                                ))
                                .is_ok()
                                {
                                    package = candidate;
                                    break;
                                }
                            }
                        }
                    }
                    sources.insert(path);
                }
            }
            let source = package.to_string();
            let (name, product_name, version, identifier) =
                apps_runtime::bundle::package_metadata(Some(&source))?;
            apps_runtime::bundle::disabled_features(Some(&source))?;
            let mut metadata = json!({"name":name,"productName":product_name,"version":version,"identifier":identifier});
            if package["type"] == "rust" {
                metadata["type"] = "rust".into();
            }
            if let Some(features) = package.get("features") {
                metadata["features"] = features.clone();
            }
            files.insert("package.json".into(), metadata.to_string());
        }
        Ok(())
    }

    fn enabled(&self, feature: &str) -> bool {
        self.disabled & apps_runtime::bundle::feature_bit(feature) == 0
    }

    // Unlike Node, bare `imports` keys apply to all modules, including dependencies.
    fn aliased(&self, specifier: &str) -> bool {
        !specifier.starts_with(['.', '/', '#'])
            && self.aliases.as_ref().is_some_and(|(_, keys)| {
                keys.iter().any(|key| match key.split_once('*') {
                    Some((prefix, suffix)) => {
                        specifier.len() >= prefix.len() + suffix.len()
                            && specifier.starts_with(prefix)
                            && specifier.ends_with(suffix)
                    }
                    None => key == specifier,
                })
            })
    }

    // Like TypeScript, only the nearest tsconfig.json counts, even when it has no paths.
    fn tsconfig(&self, directory: &Path) -> Result<Option<Rc<TsPaths>>> {
        if let Some(found) = self.tsconfigs.borrow().get(directory) {
            return Ok(found.clone());
        }
        let file = directory.join("tsconfig.json");
        let found = if file.is_file() {
            let mut read = Vec::new();
            let result = tsconfig_paths(&file, 0, &mut read);
            // Watch the whole `extends` chain, also when it has no `paths` or is invalid.
            for file in &mut read {
                if let Ok(path) = file.canonicalize() {
                    *file = path;
                }
            }
            if let Some(cache) = &self.cache {
                cache.tsconfigs.borrow_mut().extend(read.iter().cloned());
            }
            self.watch_all(read);
            let (paths, base) = result?;
            paths.map(|(directory, paths)| {
                Rc::new(TsPaths {
                    base: base.unwrap_or(directory),
                    paths,
                })
            })
        } else {
            match directory.parent() {
                Some(parent) => self.tsconfig(parent)?,
                None => None,
            }
        };
        self.tsconfigs
            .borrow_mut()
            .insert(directory.to_owned(), found.clone());
        Ok(found)
    }
}

// Never reuse module IDs: cached code embeds them in `$appsImport(N)` and `$appsN<id>`.
#[derive(Default)]
pub struct Cache {
    graphs: RefCell<HashMap<(Realm, PathBuf), Graph>>,
    loaders: RefCell<Option<Rc<Loaders>>>,
    disabled: Cell<u32>,
    env: RefCell<Value>,
    pub resolutions: RefCell<HashMap<ResolutionKey, Resolved>>,
    // Cached resolutions skip the tsconfig lookup, so each build watches the configs found before.
    tsconfigs: RefCell<BTreeSet<PathBuf>>,
}

type Resolved = (PathBuf, Option<PathBuf>, Option<PathBuf>);
type ResolutionKey = (Realm, PathBuf, String, bool);
type Entry = (PathBuf, Option<String>);

#[derive(Default)]
struct Graph {
    ids: HashMap<PathBuf, usize>,
    modules: HashMap<usize, Cached>,
}

struct Cached {
    stamp: (std::time::SystemTime, u64),
    line: Rc<str>,
    requests: Vec<(PathBuf, String, bool, usize)>,
}

fn identifier(name: &str) -> bool {
    name.starts_with(|c: char| c.is_alphabetic() || c == '_' || c == '$')
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

// `with { type: 'text' }` loads the file like `?raw`. An HTML import is the text of its bundled page already.
fn attributed(specifier: &str, text: bool) -> String {
    if text && !specifier.ends_with(".html") {
        format!("{specifier}?raw")
    } else {
        specifier.to_owned()
    }
}

fn property<'e, 'a>(object: Option<&'e Expression<'a>>, name: &str) -> Option<&'e Expression<'a>> {
    let Expression::ObjectExpression(object) = object? else {
        return None;
    };
    object
        .properties
        .iter()
        .find_map(|property| match property {
            ObjectPropertyKind::ObjectProperty(property)
                if property.key.is_specific_static_name(name) =>
            {
                Some(&property.value)
            }
            _ => None,
        })
}

impl Builder {
    fn script_chunk(&mut self, path: PathBuf) -> Result<usize> {
        let realm = if self.realm == Realm::Renderer {
            Realm::Renderer
        } else {
            Realm::Main
        };
        self.shared.chunk(path, realm)
    }

    fn url_of(&mut self, path: &Path) -> Result<Option<String>> {
        if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("js" | "mjs" | "cjs" | "ts")
        ) {
            let index = self.script_chunk(path.to_owned())?;
            return Ok(Some(format!("apps:///script-{index}.js")));
        }
        let Some(name) = self.shared.asset_name(path) else {
            return Ok(None);
        };
        self.shared.watch(path.to_owned());
        Ok(Some(format!("apps:///{name}")))
    }

    fn id(&mut self, key: PathBuf) -> usize {
        let next = self.graph.ids.len();
        let id = *self.graph.ids.entry(key.clone()).or_insert(next);
        self.ids.insert(key, id);
        if self.lines.len() <= id {
            self.lines.resize(id + 1, None);
            self.modules.resize_with(id + 1, Module::default);
        }
        id
    }

    fn define(&mut self, id: usize, module: Module) {
        if self.shared.production {
            self.modules[id] = module;
        } else {
            self.lines[id] = Some(module.line(id).into());
        }
    }

    fn resolve(&mut self, base: &Path, specifier: &str, require: bool) -> Result<usize> {
        let (path, inline) = self.locate(base, specifier, require)?;
        self.visit(path, inline)
    }

    // A require() that can't be resolved throws MODULE_NOT_FOUND at run time, so it gives no module here.
    fn require(&mut self, base: &Path, specifier: &str) -> Result<Option<usize>> {
        let Ok((path, inline)) = self.locate(base, specifier, true) else {
            return Ok(None);
        };
        self.visit(path, inline).map(Some)
    }

    fn runtime_path(&self, path: &Path) -> Result<String> {
        if !self.shared.production {
            return Ok(std::path::absolute(path)
                .unwrap_or_else(|_| path.to_owned())
                .to_string_lossy()
                .into_owned());
        }
        Ok(self
            .shared
            .app_name(path)?
            .to_string_lossy()
            .replace('\\', "/"))
    }
}

fn app_relative(root: &Path, path: &Path) -> PathBuf {
    let common = root
        .ancestors()
        .find(|directory| path.starts_with(directory));
    let mut relative: PathBuf = root
        .components()
        .skip(common.map_or(0, |common| common.components().count()))
        .filter(|component| matches!(component, std::path::Component::Normal(_)))
        .map(|_| std::path::Component::ParentDir)
        .collect();
    for component in path
        .strip_prefix(common.unwrap_or(Path::new("")))
        .unwrap_or(path)
        .components()
    {
        match component {
            std::path::Component::Prefix(prefix) => relative.push(
                prefix
                    .as_os_str()
                    .to_string_lossy()
                    .replace([':', '\\', '/', '?'], ""),
            ),
            std::path::Component::Normal(name) => relative.push(name),
            _ => {}
        }
    }
    relative
}

fn glob(pattern: &[u8], text: &[u8]) -> bool {
    match pattern {
        [] => text.is_empty(),
        [b'*', b'*', b'/', rest @ ..] => {
            (0..=text.len()).any(|i| (i == 0 || text[i - 1] == b'/') && glob(rest, &text[i..]))
        }
        [b'*', b'*', rest @ ..] => (0..=text.len()).any(|i| glob(rest, &text[i..])),
        [b'*', rest @ ..] => (0..=text.len())
            .take_while(|&i| i == 0 || text[i - 1] != b'/')
            .any(|i| glob(rest, &text[i..])),
        [b'?', rest @ ..] => text.first().is_some_and(|&c| c != b'/') && glob(rest, &text[1..]),
        [b'[', rest @ ..] => {
            let negate = matches!(rest.first(), Some(b'!' | b'^'));
            let class = &rest[usize::from(negate)..];
            // A `]` right after `[` or `[!` is a literal, and a `[` without `]` matches itself.
            let Some(end) = class.iter().skip(1).position(|&c| c == b']').map(|i| i + 1) else {
                return text.first() == Some(&b'[') && glob(rest, &text[1..]);
            };
            let Some(&c) = text.first().filter(|&&c| c != b'/') else {
                return false;
            };
            let set = &class[..end];
            let mut found = false;
            let mut i = 0;
            while i < set.len() && !found {
                if i + 2 < set.len() && set[i + 1] == b'-' {
                    found = (set[i]..=set[i + 2]).contains(&c);
                    i += 3;
                } else {
                    found = set[i] == c;
                    i += 1;
                }
            }
            found != negate && glob(&class[end + 1..], &text[1..])
        }
        [c, rest @ ..] => text.first() == Some(c) && glob(rest, &text[1..]),
    }
}

fn expand_braces(pattern: &str) -> Vec<String> {
    match (pattern.find('{'), pattern.find('}')) {
        (Some(open), Some(close)) if open < close => pattern[open + 1..close]
            .split(',')
            .flat_map(|part| {
                expand_braces(&format!(
                    "{}{part}{}",
                    &pattern[..open],
                    &pattern[close + 1..]
                ))
            })
            .collect(),
        _ => vec![pattern.to_owned()],
    }
}

// JSC keeps a whole script as UTF-16 when it has one non-ASCII character.
fn ascii_json(value: &Value) -> String {
    let json = value.to_string();
    let mut result = String::with_capacity(json.len());
    let mut start = 0;
    for (index, matched) in json.match_indices(|c: char| !c.is_ascii()) {
        result.push_str(&json[start..index]);
        start = index + matched.len();
        for unit in matched.encode_utf16() {
            let _ = write!(result, "\\u{unit:04x}");
        }
    }
    result.push_str(&json[start..]);
    result
}

pub fn compile(
    entry: &Path,
    realm: Realm,
    inline: Option<String>,
    files: Rc<RefCell<BTreeSet<PathBuf>>>,
    production: bool,
    cache: Option<Rc<Cache>>,
) -> Result<BBTreeMap<String, String>> {
    let root = entry.parent().context("Missing entry directory")?;
    let root = root.canonicalize().unwrap_or_else(|_| root.to_owned());
    let manifest = root
        .ancestors()
        .map(|directory| directory.join("package.json"))
        .find(|path| path.is_file());
    // Watch these even when the build fails below.
    files
        .borrow_mut()
        .extend(manifest.iter().cloned().chain([entry.to_owned()]));
    if let Some(cache) = &cache {
        files
            .borrow_mut()
            .extend(cache.tsconfigs.borrow().iter().cloned());
    }
    let package = manifest.as_ref().map(fs::read_to_string).transpose()?;
    let metadata: Value =
        serde_json::from_str(package.as_deref().unwrap_or("{}")).context("Invalid package.json")?;
    let rust = entry.extension().is_some_and(|ext| ext == "rs") && realm == Realm::Main;
    let root = if rust {
        ensure!(
            metadata["type"] == "rust",
            "A Rust main needs type: rust in package.json"
        );
        manifest.as_ref().unwrap().parent().unwrap().to_owned()
    } else {
        root
    };
    let preload = metadata
        .get("preload")
        .map(|value| {
            let value = value
                .as_str()
                .filter(|value| !value.is_empty())
                .context("package.json preload must be a non-empty string")?;
            resolve_file(
                &manifest.as_ref().unwrap().parent().unwrap().join(value),
                entry_fields(false, false),
            )
            .context("Cannot resolve package.json preload")
        })
        .transpose()?;
    let aliases = metadata
        .get("imports")
        .and_then(Value::as_object)
        .and_then(|imports| {
            let keys: Vec<_> = imports
                .keys()
                .filter(|key| !key.starts_with('#'))
                .cloned()
                .collect();
            Some((manifest.as_ref()?.parent()?.to_owned(), keys))
        })
        .filter(|(_, keys)| !keys.is_empty());
    let disabled = apps_runtime::bundle::disabled_features(package.as_deref())?;
    if disabled & apps_runtime::bundle::feature_bit("reactCompiler") == 0
        && let Some(react) = root.ancestors().find_map(|directory| {
            fs::read_to_string(directory.join("node_modules/react/package.json")).ok()
        })
    {
        let react: Value = serde_json::from_str(&react).context("Invalid React package.json")?;
        let version = react["version"].as_str().unwrap_or_default();
        ensure!(
            version
                .split('.')
                .next()
                .and_then(|major| major.parse::<u32>().ok())
                .is_some_and(|major| major >= 19)
                || version.starts_with("0.0.0-"),
            "The reactCompiler feature needs React 19 or later, but node_modules has React {version}"
        );
    }
    let mut loaders = None;
    if let Some(value) = metadata.get("loaders") {
        let mut modules = Vec::new();
        let mut code = String::new();
        for (index, (suffix, module)) in value
            .as_object()
            .context("package.json loaders must map file extensions to modules")?
            .iter()
            .enumerate()
        {
            let module = module
                .as_str()
                .filter(|module| !module.is_empty() && suffix.len() > 1 && suffix.starts_with('.'))
                .with_context(|| {
                    format!("package.json loaders must map file extensions like .vue to modules: {suffix}")
                })?;
            write!(code, "import l{index} from {};", json!(module))?;
            modules.push((suffix.clone(), module.to_owned()));
        }
        write!(
            code,
            "const loaders=[{}];let result;globalThis.__appsLoad=(index,args)=>{{result=undefined;Promise.resolve(loaders[index]).then(loader=>{{if(typeof loader?.load!=='function')throw new TypeError('A loader must export default an object with a load() function');return loader.load(args);}}).then(value=>{{result=JSON.stringify({{value}});}},error=>{{result=JSON.stringify({{error:String(error)+(error?.stack?'\\n'+error.stack:'')}});}});}};globalThis.__appsLoaded=()=>result;",
            (0..modules.len())
                .map(|index| format!("l{index}"))
                .collect::<Vec<_>>()
                .join(",")
        )?;
        // The inline entry needs a synthetic path that no loader can import.
        let entry = manifest
            .as_ref()
            .unwrap()
            .with_file_name("package.json#loaders");
        let shared = Rc::new(Shared {
            disabled,
            aliases: aliases.clone(),
            root: root.clone(),
            files: files.clone(),
            meta_env: meta_env(false, None),
            cache: cache.clone(),
            ..Shared::default()
        });
        let mut output = BBTreeMap::from([(
            "app.js".into(),
            bundle(
                vec![(entry.clone(), Some(code))],
                Realm::Main,
                shared.clone(),
            )?,
        )]);
        shared.finish(&mut output, entry.parent())?;
        let cached = cache
            .as_ref()
            .and_then(|cache| cache.loaders.borrow().clone())
            .filter(|loaders| loaders.modules == modules && loaders.files == output);
        loaders = Some(match cached {
            Some(loaders) => loaders,
            None => {
                let (wake, woken) = std::sync::mpsc::channel();
                Waker::install(move || {
                    let _ = wake.send(());
                });
                let engine = JsContext::new(JavaScriptCore::load()?, Rc::default(), Role::Main)?;
                engine
                    .load_bundle(&output, "apps:loaders")
                    .context("Cannot start the package.json loaders")?;
                Rc::new(Loaders {
                    modules,
                    files: output,
                    engine,
                    woken,
                })
            }
        });
    }
    let meta_env = meta_env(production, metadata.get("env"));
    if let Some(cache) = &cache {
        let previous = cache.loaders.replace(loaders.clone());
        // Loader or feature changes must invalidate cached modules.
        if previous.as_ref().map(Rc::as_ptr) != loaders.as_ref().map(Rc::as_ptr) {
            cache
                .graphs
                .borrow_mut()
                .retain(|(_, entry), _| entry.ends_with("package.json#loaders"));
        }
        // Cached modules inline `import.meta.env`.
        if cache.disabled.replace(disabled) != disabled || *cache.env.borrow() != meta_env {
            cache.env.replace(meta_env.clone());
            cache.graphs.borrow_mut().clear();
        }
    }
    let shared = Rc::new(Shared {
        disabled,
        aliases,
        preload,
        loaders,
        meta_env,
        root: root.clone(),
        files,
        production,
        cache,
        ..Shared::default()
    });
    if let Ok(entry) = entry.canonicalize() {
        shared.add_process_entry(entry);
    }
    let mut output = BBTreeMap::new();
    let main = if rust {
        let defaults = [json!("index.html")];
        let pages = match metadata.get("pages") {
            Some(value) => value
                .as_array()
                .context("package.json pages must be an array")?
                .as_slice(),
            None => {
                let index = shared.app_file("index.html");
                shared.watch(index.clone());
                if index.try_exists()? {
                    defaults.as_slice()
                } else {
                    &[]
                }
            }
        };
        for page in pages {
            let name = page
                .as_str()
                .context("Expected a page path")?
                .trim_start_matches("./");
            let relative = Path::new(name);
            ensure!(
                name.ends_with(".html")
                    && !name.contains(['\\', '?', '#'])
                    && relative
                        .components()
                        .all(|c| matches!(c, std::path::Component::Normal(_))),
                "Invalid page path {name:?}"
            );
            ensure!(!output.contains_key(name), "Duplicate page {name:?}");
            let path = shared.app_file(relative);
            shared.watch(path.clone());
            ensure!(
                path.canonicalize()
                    .with_context(|| format!("Cannot find page {name:?}"))?
                    .starts_with(&root),
                "Page outside app: {name}"
            );
            output.insert(name.into(), html(&path, &shared)?);
        }
        Some(root.as_path())
    } else {
        output.insert(
            "app.js".into(),
            bundle(vec![(entry.to_owned(), inline)], realm, shared.clone())?,
        );
        entry.parent().filter(|_| realm == Realm::Main)
    };
    shared.finish(&mut output, main)?;
    Ok(output)
}

fn meta_env(production: bool, values: Option<&Value>) -> Value {
    let mut env = json!({
        "MODE": if production { "production" } else { "development" },
        "DEV": !production,
        "PROD": production,
        "SSR": false,
        "BASE_URL": "/",
    });
    if let (Some(env), Some(values)) = (env.as_object_mut(), values.and_then(Value::as_object)) {
        env.extend(
            values
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
    env
}

pub(crate) fn is_asset(extension: &std::ffi::OsStr) -> bool {
    extension.to_str().is_some_and(|extension| {
        matches!(
            extension.to_ascii_lowercase().as_str(),
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "webp"
                | "avif"
                | "bmp"
                | "ico"
                | "svg"
                | "ttf"
                | "otf"
                | "woff"
                | "woff2"
                | "mp4"
                | "m4v"
                | "m4a"
                | "mov"
                | "webm"
                | "mkv"
                | "ogg"
                | "oga"
                | "ogv"
                | "opus"
                | "mp3"
                | "wav"
                | "flac"
                | "aac"
                | "wasm"
                | "glb"
                | "bcmap"
                | "pfb"
        )
    })
}

fn bundle(entries: Vec<Entry>, realm: Realm, shared: Rc<Shared>) -> Result<String> {
    let production = shared.production;
    let key = (realm, entries[0].0.clone());
    let graph = shared.take_graph(&key);
    let mut builder = Builder {
        realm,
        modules: Vec::new(),
        lines: Vec::new(),
        ids: BBTreeMap::new(),
        graph,
        requests: Vec::new(),
        inlined: 0,
        package_roots: BTreeSet::new(),
        dynamic: BTreeSet::new(),
        shared,
    };
    let mut classic = false;
    let result = entries
        .into_iter()
        .map(|(path, inline)| {
            let Some(name) = path.to_str().and_then(|path| path.strip_prefix("apps:///")) else {
                return builder.visit(path, inline);
            };
            let (name, element) = name.split_once('#').context("Missing script element")?;
            let code = format!("$appsClassic({},{element})", json!(name));
            classic = true;
            let id = builder.id(path);
            if production {
                builder.modules[id] = Module {
                    code,
                    linked: true,
                    ..Module::default()
                };
            } else {
                builder.lines[id] = Some(
                    format!("$appsDefine({{id:{id},factory:function*(){{yield;{code}}}}});\n")
                        .into(),
                );
            }
            Ok(id)
        })
        .collect::<Result<Vec<_>>>();
    builder
        .shared
        .store_graph(key.clone(), std::mem::take(&mut builder.graph));
    let entries = result?;
    let mut chunked = serde_json::Map::new();
    if production {
        let modules = &builder.modules;
        let mut available: Vec<BTreeSet<String>> = modules
            .iter()
            .map(|module| {
                module
                    .exports
                    .iter()
                    .map(|(name, _)| name.clone())
                    .collect()
            })
            .collect();
        let mut dynamic_stars: Vec<_> = modules
            .iter()
            .map(|module| module.commonjs || module.preload)
            .collect();
        loop {
            let mut changed = false;
            for (id, module) in modules.iter().enumerate() {
                for &target in &module.stars {
                    if dynamic_stars[target] && !dynamic_stars[id] {
                        dynamic_stars[id] = true;
                        changed = true;
                    }
                    let missing: Vec<_> = available[target]
                        .iter()
                        .filter(|name| *name != "default" && !available[id].contains(*name))
                        .cloned()
                        .collect();
                    changed |= !missing.is_empty();
                    available[id].extend(missing);
                }
            }
            if !changed {
                break;
            }
        }
        for module in modules.iter() {
            for import in &module.imports {
                ensure!(
                    import.local.is_none()
                        || import.name == "*"
                        || dynamic_stars[import.dep]
                        || available[import.dep].contains(&import.name),
                    "Missing export {} from {}",
                    import.name,
                    builder
                        .ids
                        .iter()
                        .find(|(_, id)| **id == import.dep)
                        .map_or_else(
                            || import.dep.to_string(),
                            |(path, _)| path.display().to_string()
                        )
                );
            }
        }

        let cyclic = in_cycles(modules);
        let flat: Vec<bool> = (0..modules.len())
            .map(|id| {
                let module = &modules[id];
                module.linked
                    && !module.commonjs
                    && !module.preload
                    && module.asynchronous == 0
                    && !dynamic_stars[id]
                    && !cyclic[id]
            })
            .collect();
        let export_values: Vec<HashMap<&str, &Export>> = modules
            .iter()
            .map(|module| {
                module
                    .exports
                    .iter()
                    .map(|(name, export)| (name.as_str(), export))
                    .collect()
            })
            .collect();
        let walk = |dep: usize, name: &str, through: &mut Vec<usize>| -> (usize, String) {
            let (mut dep, mut name) = (dep, name.to_owned());
            for _ in 0..256 {
                let module = &modules[dep];
                through.push(dep);
                if !module.linked || module.commonjs || module.preload {
                    break;
                }
                if let Some(export) = export_values[dep].get(name.as_str()) {
                    match export {
                        Export::Reexport(target, field) if field != "*" => {
                            (dep, name) = (*target, field.clone())
                        }
                        _ => break,
                    }
                } else if name != "default"
                    && !dynamic_stars[dep]
                    && let Some(&star) = module
                        .stars
                        .iter()
                        .find(|&&star| available[star].contains(&name))
                {
                    dep = star;
                } else {
                    break;
                }
            }
            (dep, name)
        };
        let origin = |dep: usize, name: &str| walk(dep, name, &mut Vec::new());
        let mut uses_of: Vec<Vec<(usize, Option<String>)>> = vec![Vec::new(); modules.len()];
        // Drop unused built-in exports: their getters initialize runtime modules.
        let minifier = |module: &Module| MinifierOptions {
            compress: Some(CompressOptions {
                treeshake: TreeShakeOptions {
                    property_read_side_effects: if module.builtin {
                        PropertyReadSideEffects::None
                    } else {
                        PropertyReadSideEffects::All
                    },
                    ..TreeShakeOptions::default()
                },
                ..CompressOptions::default()
            }),
            ..MinifierOptions::default()
        };
        let mut needed = vec![BTreeSet::new(); modules.len()];
        for &id in entries.iter().chain(&builder.dynamic) {
            needed[id] = available[id].clone();
        }
        let mut used_imports = vec![BTreeSet::new(); modules.len()];
        let mut factories = vec![String::new(); modules.len()];
        let mut whole = vec![false; modules.len()];
        let mut pending = vec![true; modules.len()];
        let mut queue = VecDeque::with_capacity(modules.len());
        let mut seen = vec![false; modules.len()];
        for root in 0..modules.len() {
            if seen[root] {
                continue;
            }
            seen[root] = true;
            let mut stack = vec![(root, 0)];
            while let Some((id, next)) = stack.last_mut() {
                let module = &modules[*id];
                let target = module
                    .imports
                    .get(*next)
                    .map(|import| import.dep)
                    .or_else(|| module.stars.get(*next - module.imports.len()).copied());
                if let Some(target) = target {
                    *next += 1;
                    if !seen[target] {
                        seen[target] = true;
                        stack.push((target, 0));
                    }
                } else {
                    queue.push_front(*id);
                    stack.pop();
                }
            }
        }
        while let Some(id) = queue.pop_front() {
            pending[id] = false;
            let module = &modules[id];
            let mut source = if module.commonjs {
                format!(
                    "$appsFactory=function(exports,require,module,__filename,__dirname,$appsRequire,$appsImport){{\n{}\n}};",
                    module.code
                )
            } else {
                let getters = module
                    .exports
                    .iter()
                    .filter(|(name, _)| needed[id].contains(name))
                    .map(|(name, export)| {
                        format!(
                            "[{}]:{{enumerable:true,get:()=>({})}}",
                            json!(name),
                            export.code()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                let params = if module.linked {
                    module
                        .deps
                        .iter()
                        .fold(String::from(",$appsImport,$appsRequire"), |params, dep| {
                            params + ",$appsN" + &dep.to_string()
                        })
                } else {
                    String::new()
                };
                format!(
                    "$appsFactory=function*($out,$appsBind{params}){{'use strict';$appsBind($out,{{{getters}}});yield;\n{}\n}};",
                    module.code
                )
            };
            if module.asynchronous == 2 {
                source = source.replacen("=function*(", "=async function*(", 1);
            }
            let allocator = Allocator::default();
            let mut parsed = Parser::new(&allocator, &source, SourceType::cjs()).parse();
            ensure!(
                parsed.diagnostics.is_empty(),
                "Invalid JavaScript in module {id}: {:?}",
                parsed.diagnostics
            );
            let result = Minifier::new(minifier(module)).minify(&allocator, &mut parsed.program);
            let scoping = result.scoping.as_ref().unwrap();
            let references = scoping.root_unresolved_references();
            let eval = references.contains_key("eval");
            used_imports[id].clear();
            let mut requests = Vec::new();
            let mut uses = Uses(Vec::new());
            if module.linked {
                uses.visit_program(&parsed.program);
                for (target, name) in &uses.0 {
                    match name {
                        Some(name) if flat[id] => requests.push(origin(*target, name)),
                        Some(name) => requests.push((*target, name.clone())),
                        None => {
                            whole[*target] = true;
                            requests.extend(
                                available[*target]
                                    .iter()
                                    .map(|name| (*target, name.clone())),
                            );
                        }
                    }
                }
            }
            for import in &module.imports {
                let Some(local) = &import.local else {
                    continue;
                };
                if module.linked {
                    if uses.0.iter().any(|(dep, used)| {
                        *dep == import.dep
                            && (import.name == "*"
                                || used.as_deref().is_none_or(|used| used == import.name))
                    }) {
                        used_imports[id].insert(local.clone());
                    }
                } else if eval || references.contains_key(local.as_str()) {
                    used_imports[id].insert(local.clone());
                    if import.name == "*" {
                        whole[import.dep] = true;
                        requests.extend(
                            available[import.dep]
                                .iter()
                                .map(|name| (import.dep, name.clone())),
                        );
                    } else {
                        requests.push((import.dep, import.name.clone()));
                    }
                }
            }
            for &target in &module.stars {
                for name in &needed[id] {
                    if name != "default"
                        && available[target].contains(name)
                        && !export_values[id].contains_key(name.as_str())
                    {
                        requests.push((target, name.clone()));
                    }
                }
            }
            for (target, name) in requests {
                if needed[target].insert(name) && !pending[target] {
                    pending[target] = true;
                    queue.push_back(target);
                }
            }
            uses_of[id] = uses.0;
            if flat[id] {
                continue;
            }
            factories[id] = factory(&parsed.program, result, id)?;
        }
        // Barrel imports still need to run for their side effects.
        let mut through = BTreeSet::new();
        for id in (0..modules.len()).filter(|&id| flat[id]) {
            for (target, name) in &uses_of[id] {
                let mut path = Vec::new();
                if let Some(name) = name {
                    walk(*target, name, &mut path);
                }
                path.pop();
                through.extend(path);
            }
        }
        let runs_imports = |id: usize| {
            through.contains(&id) && modules[id].deps.iter().any(|&dep| !modules[dep].pure)
        };
        let pruned: Vec<_> = (0..modules.len())
            .map(|id| {
                modules[id].pure
                    && needed[id].is_empty()
                    && !runs_imports(id)
                    && !whole[id]
                    && !dynamic_stars[id]
                    && !entries.contains(&id)
                    && !builder.dynamic.contains(&id)
            })
            .collect();
        let flat: Vec<bool> = (0..modules.len())
            .map(|id| flat[id] && !pruned[id])
            .collect();
        let mut namespaced = whole.clone();
        for &id in entries.iter().chain(&builder.dynamic) {
            namespaced[id] = true;
        }
        for (id, module) in modules.iter().enumerate() {
            if !pruned[id] && !flat[id] {
                for &dep in module.deps.iter().chain(&module.stars) {
                    namespaced[dep] = true;
                }
            }
        }
        let mut slots: HashMap<(usize, String), (usize, bool)> = HashMap::new();
        let mut tails = vec![String::new(); modules.len()];
        for id in (0..modules.len()).filter(|&id| flat[id]) {
            for name in &needed[id] {
                if origin(id, name) != (id, name.clone()) {
                    continue;
                }
                let Some(export) = export_values[id].get(name.as_str()) else {
                    continue;
                };
                let slot = slots.len();
                let live =
                    matches!(export, Export::Local(local) if modules[id].live.contains(local));
                slots.insert((id, name.clone()), (slot, live));
                let _ = write!(
                    tails[id],
                    "$appsS[{slot}]={}{};",
                    if live { "()=>" } else { "" },
                    export.code()
                );
            }
        }
        let mut arguments: Vec<Vec<Option<usize>>> = vec![Vec::new(); modules.len()];
        for id in (0..modules.len()).filter(|&id| flat[id]) {
            let module = &modules[id];
            let source = format!("{}\n;{}", module.code, tails[id]);
            let allocator = Allocator::default();
            let mut parsed = Parser::new(&allocator, &source, SourceType::cjs()).parse();
            ensure!(
                parsed.diagnostics.is_empty(),
                "Invalid JavaScript in module {id}: {:?}",
                parsed.diagnostics
            );
            let mut linker = Slots {
                ast: AstBuilder::new(&allocator),
                allocator: &allocator,
                origin: &origin,
                flat: &flat,
                slots: &slots,
                namespaces: BTreeSet::new(),
            };
            linker.visit_program(&mut parsed.program);
            let params = linker.namespaces.iter().fold(String::new(), |params, dep| {
                params + ",$appsN" + &dep.to_string()
            });
            let declared = linker.namespaces.into_iter().collect::<Vec<_>>();
            let wrapper = allocator.alloc_str(&format!(
                "$appsFactory=function($appsS,$appsImport,$appsRequire{params}){{'use strict';}};"
            ));
            let mut program = Parser::new(&allocator, wrapper, SourceType::cjs())
                .parse()
                .program;
            let Some(Statement::ExpressionStatement(statement)) = program.body.first_mut() else {
                bail!("Missing module factory: {id}");
            };
            let Expression::AssignmentExpression(assignment) = &mut statement.expression else {
                bail!("Invalid module factory: {id}");
            };
            let Expression::FunctionExpression(function) = &mut assignment.right else {
                bail!("Invalid module factory: {id}");
            };
            function
                .body
                .as_mut()
                .context("Missing module factory body")?
                .statements = parsed.program.body;
            let result = Minifier::new(minifier(module)).minify(&allocator, &mut program);
            // Pruned imports need no argument. Preserve parameter positions for imports that survive
            // minification.
            let mut kept = Uses(Vec::new());
            kept.visit_program(&program);
            let kept: BTreeSet<usize> = kept.0.into_iter().map(|(dep, _)| dep).collect();
            for &dep in &kept {
                namespaced[dep] = true;
            }
            arguments[id] = declared
                .iter()
                .map(|dep| kept.contains(dep).then_some(*dep))
                .collect::<Vec<_>>();
            while arguments[id].last() == Some(&None) {
                arguments[id].pop();
            }
            factories[id] = factory(&program, result, id)?;
        }
        // An import that skips a pruned barrel still needs its origin to run, in the barrel's place.
        let linked_deps: Vec<Vec<usize>> = (0..modules.len())
            .map(|id| {
                if !flat[id] {
                    return Vec::new();
                }
                let mut deps = Vec::new();
                for &dep in &modules[id].deps {
                    if pruned[dep] {
                        deps.extend(uses_of[id].iter().filter_map(|(target, name)| {
                            if *target != dep {
                                return None;
                            }
                            Some(origin(dep, name.as_deref()?).0)
                        }));
                    } else {
                        deps.push(dep);
                    }
                }
                deps.extend(uses_of[id].iter().filter_map(|(target, name)| {
                    Some(origin(*target, name.as_deref()?).0).filter(|&dep| !pruned[dep])
                }));
                let mut seen = BTreeSet::new();
                deps.retain(|&dep| dep != id && !pruned[dep] && seen.insert(dep));
                deps
            })
            .collect();
        let mut compiled = vec![false; modules.len()];
        for (id, module) in modules.iter().enumerate().filter(|&(id, _)| !pruned[id]) {
            let mut line = format!("$appsDefine({{id:{id}");
            let deps: Vec<_> = if flat[id] {
                linked_deps[id].iter().copied().map(Some).collect()
            } else {
                module
                    .deps
                    .iter()
                    .map(|&dep| (!pruned[dep]).then_some(dep))
                    .collect()
            };
            let stars: Vec<_> = module
                .stars
                .iter()
                .copied()
                .filter(|&star| !pruned[star] && !flat[id])
                .collect();
            if !deps.is_empty() {
                let _ = write!(line, ",deps:{}", json!(deps));
            }
            if flat[id] {
                line.push_str(",flat:1");
                if !arguments[id].is_empty() {
                    let _ = write!(line, ",args:{}", json!(arguments[id]));
                }
                if namespaced[id] {
                    let table: serde_json::Map<String, Value> = needed[id]
                        .iter()
                        .map(|name| {
                            let (target, field) = origin(id, name);
                            let value = match slots.get(&(target, field.clone())) {
                                Some(&(slot, false)) => json!(slot),
                                Some(&(slot, true)) => json!(-(slot as i64) - 1),
                                None => json!([target, field]),
                            };
                            (name.clone(), value)
                        })
                        .collect();
                    let _ = write!(line, ",slots:{}", ascii_json(&Value::Object(table)));
                }
            }
            if !stars.is_empty() {
                let _ = write!(line, ",stars:{}", json!(stars));
            }
            if module.preload {
                line.push_str(",preload:1");
            }
            if !module.defers.is_empty() {
                let _ = write!(line, ",defers:{}", json!(module.defers));
            }
            if module.commonjs {
                line.push_str(",commonjs:1");
                if let Some(file) = &module.file {
                    let _ = write!(line, ",file:{}", ascii_json(&json!(file)));
                }
                if !module.resolved.is_empty() {
                    let _ = write!(
                        line,
                        ",resolved:{}",
                        ascii_json(&Value::Object(module.resolved.clone()))
                    );
                }
            }
            if module.asynchronous != 0 {
                let _ = write!(line, ",async:{}", module.asynchronous);
            }
            let mut factory = std::mem::take(&mut factories[id]);
            if !module.linked && !module.commonjs {
                let imports: Vec<_> = module
                    .imports
                    .iter()
                    .filter(|import| {
                        import
                            .local
                            .as_ref()
                            .is_some_and(|local| used_imports[id].contains(local))
                            && !pruned[import.dep]
                    })
                    .map(Import::json)
                    .collect();
                let _ = write!(
                    line,
                    ",linked:false,imports:{}",
                    ascii_json(&json!(imports))
                );
                factory = format!("function($scope){{with($scope)return {factory}}}");
            } else if realm == Realm::Renderer && builder.shared.has_preload() && module.linked {
                let imports: Vec<_> = module
                    .imports
                    .iter()
                    .filter(|import| {
                        dynamic_stars[import.dep]
                            && !pruned[import.dep]
                            && import
                                .local
                                .as_ref()
                                .is_some_and(|local| used_imports[id].contains(local))
                    })
                    .map(Import::json)
                    .collect();
                if !imports.is_empty() {
                    let _ = write!(line, ",imports:{}", ascii_json(&json!(imports)));
                }
            }
            // Compile raw non-ASCII templates from strings to keep the bundle ASCII.
            if factory.is_ascii() {
                let _ = writeln!(line, ",factory:{factory}}});");
            } else {
                compiled[id] = true;
                let _ = writeln!(line, ",source:{}}});", ascii_json(&Value::String(factory)));
            }
            builder.lines[id] = Some(line.into());
        }
        // Keep string-compiled modules in the bundle. Pages cannot compile them from later import() chunks.
        let mut owner = vec![usize::MAX; modules.len()];
        let mut groups = VecDeque::from([entries
            .iter()
            .copied()
            .chain((0..modules.len()).filter(|&id| compiled[id]))
            .collect::<Vec<_>>()]);
        let mut chunks: Vec<Vec<usize>> = Vec::new();
        while let Some(mut stack) = groups.pop_front() {
            let mut members = Vec::new();
            while let Some(id) = stack.pop() {
                if pruned[id] || owner[id] != usize::MAX {
                    continue;
                }
                owner[id] = chunks.len();
                members.push(id);
                stack.extend(&linked_deps[id]);
                stack.extend(modules[id].deps.iter().chain(&modules[id].required));
                groups.extend(modules[id].imported.iter().map(|&target| vec![target]));
            }
            if chunks.is_empty() || !members.is_empty() {
                members.sort_unstable();
                chunks.push(members);
            }
        }
        for members in chunks.iter().skip(1) {
            let mut source = String::from("(($appsDefine)=>{\n");
            for &id in members {
                source.push_str(builder.lines[id].as_deref().unwrap_or(""));
            }
            source.push_str("})");
            let index = builder.shared.script(source);
            chunked.insert(format!("script-{index}.js"), json!(members));
        }
        for (id, line) in builder.lines.iter_mut().enumerate() {
            if owner[id] != 0 {
                *line = None;
            }
        }
    }
    let mut app = String::with_capacity(
        builder
            .lines
            .iter()
            .flatten()
            .map(|line| line.len())
            .sum::<usize>()
            + 64,
    );
    if classic {
        app.push_str("const $appsChunk=this.__appsChunk,$appsScripts=document.getElementsByTagName('script'),$appsClassic=(name,element)=>{__appsScript($appsScripts[element]);$appsChunk(name)};\n");
    }
    for line in builder.lines.iter().flatten() {
        app.push_str(line);
    }
    if !chunked.is_empty() {
        let _ = write!(
            app,
            "return[{},{}];",
            json!(entries),
            Value::Object(chunked)
        );
    } else if entries != [0] {
        let _ = write!(app, "return[{}];", json!(entries));
    }
    Ok(app)
}

mod css;
mod emit;
mod html;
mod link;
mod resolve;
mod visit;

#[cfg(test)]
mod tests;
