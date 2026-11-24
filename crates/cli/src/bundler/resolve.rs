use super::html::remote;
use super::*;

pub(super) type Paths = (
    Option<(PathBuf, serde_json::Map<String, Value>)>,
    Option<PathBuf>,
);

// Returns `paths` with the folder they are relative to without `baseUrl`, and `baseUrl`. Each comes from the nearest
// config in the `extends` chain that sets it.
pub(super) fn tsconfig_paths(file: &Path, depth: usize, read: &mut Vec<PathBuf>) -> Result<Paths> {
    ensure!(
        depth < 16,
        "tsconfig.json extends itself: {}",
        file.display()
    );
    read.push(file.to_owned());
    let source =
        fs::read_to_string(file).with_context(|| format!("Cannot read {}", file.display()))?;
    let mut json = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                json.push(c);
                while let Some(c) = chars.next() {
                    json.push(c);
                    if c == '\\' {
                        json.extend(chars.next());
                    } else if c == '"' {
                        break;
                    }
                }
            }
            '/' if chars.next_if_eq(&'/').is_some() => {
                while chars.next_if(|&c| c != '\n').is_some() {}
            }
            '/' if chars.next_if_eq(&'*').is_some() => {
                while let Some(c) = chars.next() {
                    if c == '*' && chars.next_if_eq(&'/').is_some() {
                        break;
                    }
                }
            }
            '}' | ']' => {
                let end = json.trim_end().len();
                if json[..end].ends_with(',') {
                    json.truncate(end - 1);
                }
                json.push(c);
            }
            _ => json.push(c),
        }
    }
    let config: Value =
        serde_json::from_str(&json).with_context(|| format!("Invalid {}", file.display()))?;
    let directory = file.parent().context("Missing tsconfig.json folder")?;
    let options = &config["compilerOptions"];
    let mut paths = options["paths"]
        .as_object()
        .map(|paths| (directory.to_owned(), paths.clone()));
    let mut base = options["baseUrl"].as_str().map(|url| directory.join(url));
    if let Some(parent) = config["extends"]
        .as_str()
        .filter(|_| paths.is_none() || base.is_none())
        && parent.starts_with('.')
    {
        let mut parent = directory.join(parent);
        if parent
            .extension()
            .is_none_or(|extension| extension != "json")
        {
            parent.as_mut_os_string().push(".json");
        }
        let (inherited, inherited_base) = tsconfig_paths(&parent, depth + 1, read)?;
        paths = paths.or(inherited);
        base = base.or(inherited_base);
    }
    Ok((paths, base))
}

pub(super) const ELECTRON: &[&str] = &[
    "app",
    "BrowserWindow",
    "View",
    "WebContentsView",
    "webContents",
    "serve",
    "contextBridge",
    "ipcMain",
    "ipcRenderer",
    "clipboard",
    "shell",
    "dialog",
    "nativeTheme",
    "safeStorage",
    "Notification",
    "Menu",
    "MenuItem",
    "Tray",
    "globalShortcut",
    "session",
    "powerMonitor",
    "screen",
    "powerSaveBlocker",
    "systemPreferences",
    "webUtils",
    "net",
    "nativeImage",
    "protocol",
    "webFrame",
    "autoUpdater",
    "desktopCapturer",
    "ClipboardItem",
    "utilityProcess",
    "terminal",
];

pub(super) const BUILTINS: &str = include_str!("builtins.txt");
pub(super) const PACKAGES: &[&str] = &["@pierre/apps", "electron", "node-pty", "ws", "undici"];

pub(super) fn in_packages(path: &Path) -> bool {
    path.components()
        .any(|part| part.as_os_str() == "node_modules")
}

impl Builder {
    pub(super) fn locate(
        &mut self,
        base: &Path,
        specifier: &str,
        require: bool,
    ) -> Result<(PathBuf, Option<String>)> {
        let fields = entry_fields(self.realm == Realm::Renderer, require);
        let builtin = |name: &str| {
            BUILTINS
                .lines()
                .find(|line| line.split(' ').next() == Some(name))
                .filter(|_| !PACKAGES.contains(&name))
        };
        let normalized;
        let specifier = if !specifier.starts_with("node:")
            && builtin(specifier).is_some()
            && (self.realm != Realm::Renderer
                || !base.ancestors().any(|directory| {
                    directory
                        .join("node_modules")
                        .join(specifier.split('/').next().unwrap())
                        .is_dir()
                })) {
            normalized = format!("node:{specifier}");
            normalized.as_str()
        } else {
            specifier
        };
        if PACKAGES.contains(&specifier) || specifier.starts_with("node:") {
            ensure!(
                self.realm != Realm::Renderer,
                "{specifier} is unavailable in page scripts. Use a preload script."
            );
            ensure!(
                self.realm == Realm::Main
                    || specifier.starts_with("node:")
                    || matches!(specifier, "electron" | "@pierre/apps"),
                "Window APIs are only available in the main context"
            );
            let path = PathBuf::from(format!("apps:{specifier}"));
            if self.ids.contains_key(&path) {
                return Ok((path, None));
            }
            let (object, names) = if matches!(specifier, "electron" | "@pierre/apps") {
                ("globalThis.__apps", ELECTRON.to_vec())
            } else {
                let line = match specifier.strip_prefix("node:") {
                    Some(name) => builtin(name).or_else(|| builtin(specifier)),
                    None => BUILTINS
                        .lines()
                        .find(|line| line.split(' ').next() == Some(specifier)),
                }
                .with_context(|| format!("Unsupported built-in module: {specifier}"))?;
                let mut parts = line.split(' ').skip(1);
                (parts.next().unwrap_or_default(), parts.collect())
            };
            // Node's require(esm) returns the 'module.exports' export, so CommonJS gets the module object itself.
            let source = format!(
                "const api={object};export default api;export{{api as 'module.exports'}};{}",
                names
                    .iter()
                    .map(|name| format!("export const {name}=api.{name};"))
                    .collect::<String>()
            );
            return Ok((path, Some(source)));
        }
        if let Some((path, source)) = remote(base, specifier, self.shared.enabled("httpImport"))? {
            return Ok((path, Some(source)));
        }
        if let Some((file, query)) = specifier.split_once('?') {
            let kind = query.split('&').next().unwrap_or_default();
            if matches!(kind, "url" | "raw" | "worker" | "sharedworker") {
                let (path, _) = self.locate(base, file, require)?;
                let source = match kind {
                    "url" => {
                        let url = self.url_of(&path)?.with_context(|| {
                            format!("Cannot bundle {} as a URL", path.display())
                        })?;
                        format!("export default {};", json!(url))
                    }
                    "raw" => {
                        self.shared.watch(path.clone());
                        format!("export default {};", json!(fs::read_to_string(&path)?))
                    }
                    _ => format!(
                        "export default function(options){{return new {}(new URL({},import.meta.url),options);}}",
                        if kind == "worker" {
                            "Worker"
                        } else {
                            "SharedWorker"
                        },
                        json!(format!(
                            "./{}",
                            path.file_name().unwrap_or_default().to_string_lossy()
                        ))
                    ),
                };
                let mut name = path.into_os_string();
                name.push(format!("?{kind}"));
                return Ok((PathBuf::from(name), Some(source)));
            }
        }
        let resolution = (self.realm, base.to_owned(), specifier.to_owned(), require);
        let cached = self.shared.resolution(&resolution);
        if let Some((target, root, manifest)) = cached {
            self.package_roots.extend(root);
            self.shared.watch_all(manifest);
            return Ok((target, None));
        }
        if specifier.starts_with('.') || Path::new(specifier).is_absolute() {
            let path = base.join(specifier);
            let package = in_packages(&path);
            let path = resolve_file(&path, fields)?;
            let root = path.parent().filter(|_| package).map(Path::to_owned);
            self.package_roots.extend(root.clone());
            self.shared.remember(resolution, (path.clone(), root, None));
            return Ok((path, None));
        }
        if !specifier.starts_with('#')
            && !in_packages(base)
            && let Some(config) = self.shared.tsconfig(base)?
            && let Some((star, targets)) = config
                .paths
                .iter()
                .filter_map(|(pattern, targets)| match pattern.split_once('*') {
                    Some((prefix, suffix)) => (specifier.len() >= prefix.len() + suffix.len()
                        && specifier.starts_with(prefix)
                        && specifier.ends_with(suffix))
                    .then(|| {
                        let star = &specifier[prefix.len()..specifier.len() - suffix.len()];
                        (Some(star), prefix.len(), targets)
                    }),
                    None => (pattern == specifier).then_some((None, usize::MAX, targets)),
                })
                .max_by_key(|(_, length, _)| *length)
                .map(|(star, _, targets)| (star, targets))
            && let Some(path) = targets.as_array().into_iter().flatten().find_map(|target| {
                let target = target.as_str()?;
                let target =
                    star.map_or_else(|| target.to_owned(), |star| target.replace('*', star));
                resolve_file(&config.base.join(target), fields).ok()
            })
        {
            self.shared.remember(resolution, (path.clone(), None, None));
            return Ok((path, None));
        }
        let alias = self.shared.aliased(specifier);
        let imports = alias || specifier.starts_with('#');
        let (directory, key) = if let Some((directory, _)) =
            self.shared.aliases.as_ref().filter(|_| alias)
        {
            (directory.canonicalize()?, specifier.to_owned())
        } else if imports {
            (
                base.ancestors()
                    .find(|directory| directory.join("package.json").is_file())
                    .with_context(|| format!("Cannot resolve {specifier} from {}", base.display()))?
                    .canonicalize()?,
                specifier.to_owned(),
            )
        } else {
            let components: Vec<_> = specifier.split('/').collect();
            let count = if specifier.starts_with('@') { 2 } else { 1 };
            ensure!(components.len() >= count, "Invalid package name");
            let package = components[..count].join("/");
            let subpath = components[count..].join("/");
            (
                base.ancestors()
                    .map(|p| p.join("node_modules").join(&package))
                    .find(|p| p.is_dir())
                    .or_else(|| {
                        let scope = base
                            .ancestors()
                            .find(|p| p.join("package.json").is_file())?;
                        let manifest: Value = serde_json::from_str(
                            &fs::read_to_string(scope.join("package.json")).ok()?,
                        )
                        .ok()?;
                        (manifest["name"] == package.as_str()).then(|| scope.to_owned())
                    })
                    .with_context(|| format!("Cannot resolve {specifier} from {}", base.display()))?
                    .canonicalize()?,
                if subpath.is_empty() {
                    ".".to_owned()
                } else {
                    format!("./{subpath}")
                },
            )
        };
        if !imports {
            self.package_roots.insert(directory.clone());
        }
        let package_file = directory.join("package.json");
        let metadata = self.shared.manifest(&package_file)?;
        let entry = if let Some(exports) = metadata.get(if imports { "imports" } else { "exports" })
        {
            let conditions: &[&str] = match (self.realm == Realm::Renderer, require) {
                (true, true) => &["browser", "require", "default"],
                (true, false) => &["browser", "import", "default"],
                (false, true) => &["node", "require", "module-sync", "node-addons", "default"],
                (false, false) => &["node", "import", "module-sync", "node-addons", "default"],
            };
            let value = exported(exports, &key, conditions, imports)?;
            if imports && !value.starts_with(['.', '/', '#']) {
                ensure!(
                    !self.shared.aliased(&value),
                    "{specifier} maps to {value}, which package.json imports maps again"
                );
                return self.locate(&directory, &value, require);
            }
            ensure!(value.starts_with("./"), "Package export must start with ./");
            value
        } else if imports {
            bail!(
                "Cannot resolve {specifier}: {} has no imports",
                package_file.display()
            );
        } else if key != "." {
            key
        } else {
            fields
                .iter()
                .find_map(|field| {
                    metadata
                        .get(*field)
                        .and_then(Value::as_str)
                        .filter(|entry| resolve_file(&directory.join(entry), fields).is_ok())
                })
                .unwrap_or("index.js")
                .to_owned()
        };
        let target = resolve_file(&directory.join(entry), fields)?;
        // Only package exports must stay inside their folder. App imports can reach symlinked packages.
        ensure!(
            imports || target.starts_with(&directory),
            "Package export escapes its directory"
        );
        self.shared.remember(
            resolution,
            (
                target.clone(),
                (!imports).then_some(directory),
                Some(package_file),
            ),
        );
        Ok((target, None))
    }
}

pub(super) fn native_specifier(path: &Path) -> Result<String> {
    let components: Vec<_> = path.components().collect();
    if let Some(index) = components
        .iter()
        .rposition(|component| component.as_os_str() == "node_modules")
    {
        return Ok(components[index + 1..]
            .iter()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"));
    }
    for directory in path.parent().unwrap().ancestors() {
        let manifest = directory.join("package.json");
        if !manifest.is_file() {
            continue;
        }
        let metadata: Value = serde_json::from_str(&fs::read_to_string(manifest)?)?;
        if let Some(name) = metadata["name"].as_str() {
            let parts: Vec<_> = name.split('/').collect();
            ensure!(
                !name.contains('\\')
                    && parts
                        .iter()
                        .all(|part| !part.is_empty() && *part != "." && *part != "..")
                    && (parts.len() == 1 || (parts.len() == 2 && name.starts_with('@'))),
                "Invalid package name for native addon: {name}"
            );
            return Ok(format!(
                "{name}/{}",
                path.strip_prefix(directory)?
                    .to_string_lossy()
                    .replace('\\', "/")
            ));
        }
    }
    let directory = path.parent().context("Native addon has no parent")?;
    let package = directory
        .file_name()
        .context("Native addon needs a package directory")?
        .to_string_lossy();
    Ok(format!(
        "{package}/{}",
        path.file_name().unwrap().to_string_lossy()
    ))
}

// Node ignores `module`. Like esbuild, node realms prefer `main` and require() never uses `module`.
pub(super) fn entry_fields(renderer: bool, require: bool) -> &'static [&'static str] {
    match (renderer, require) {
        (true, true) => &["browser", "main"],
        (true, false) => &["browser", "module", "main"],
        (false, true) => &["main"],
        (false, false) => &["main", "module"],
    }
}

pub(super) fn resolve_file(path: &Path, fields: &[&str]) -> Result<PathBuf> {
    if path.is_file() {
        return Ok(path.canonicalize()?);
    }
    let typescript = path
        .extension()
        .is_some_and(|extension| matches!(extension.to_str(), Some("js" | "jsx" | "mjs" | "cjs")));
    for candidate in [
        "ts", "tsx", "mts", "js", "jsx", "mjs", "cjs", "json", "node",
    ]
    .iter()
    .map(|extension| {
        let mut candidate = path.as_os_str().to_owned();
        candidate.push(".");
        candidate.push(extension);
        PathBuf::from(candidate)
    })
    .chain(
        ["ts", "tsx", "mts", "cts"]
            .iter()
            .filter(|_| typescript)
            .map(|extension| path.with_extension(extension)),
    ) {
        if candidate.is_file() {
            return Ok(candidate.canonicalize()?);
        }
    }
    if path.is_dir() {
        if let Ok(source) = fs::read_to_string(path.join("package.json"))
            && let Ok(metadata) = serde_json::from_str::<Value>(&source)
            && let Some(entry) = fields
                .iter()
                .find_map(|key| metadata.get(*key).and_then(Value::as_str))
            && !matches!(entry.trim_end_matches('/'), "" | ".")
            && let Ok(target) = resolve_file(&path.join(entry), fields)
        {
            return Ok(target);
        }
        for name in [
            "index.ts",
            "index.tsx",
            "index.mts",
            "index.js",
            "index.jsx",
            "index.mjs",
            "index.cjs",
            "index.node",
        ] {
            let candidate = path.join(name);
            if candidate.is_file() {
                return Ok(candidate.canonicalize()?);
            }
        }
    }
    bail!("Cannot resolve {}", path.display())
}

pub(super) fn exported(
    exports: &Value,
    key: &str,
    conditions: &[&str],
    imports: bool,
) -> Result<String> {
    let mut replacement = None;
    let selected = if let Some(map) = exports
        .as_object()
        .filter(|map| imports || map.keys().any(|k| k.starts_with(['.', '#'])))
    {
        if let Some(value) = map.get(key) {
            value
        } else {
            let (prefix, suffix, value) = map
                .iter()
                .filter_map(|(pattern, value)| {
                    let (prefix, suffix) = pattern.split_once('*')?;
                    (key.starts_with(prefix)
                        && key.ends_with(suffix)
                        && key.len() >= prefix.len() + suffix.len())
                    .then_some((prefix, suffix, value))
                })
                .max_by_key(|(prefix, suffix, _)| (prefix.len(), suffix.len()))
                .with_context(|| format!("Package does not export {key}"))?;
            replacement = Some(&key[prefix.len()..key.len() - suffix.len()]);
            value
        }
    } else {
        ensure!(key == ".", "Package does not export {key}");
        exports
    };
    let mut candidates = vec![selected];
    let mut path = None;
    while let Some(value) = candidates.pop() {
        match value {
            Value::String(value) => {
                path = Some(value.as_str());
                break;
            }
            Value::Array(values) => candidates.extend(values.iter().rev()),
            Value::Object(values) => candidates.extend(
                values
                    .iter()
                    .filter(|(key, _)| conditions.contains(&key.as_str()))
                    .map(|(_, value)| value)
                    .rev(),
            ),
            Value::Null => break,
            _ => {}
        }
    }
    let value = path.context("Package has no supported export condition")?;
    Ok(match replacement {
        Some(replacement) => value.replace('*', replacement),
        None => value.to_owned(),
    })
}
