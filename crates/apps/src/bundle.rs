use anyhow::{Context, Result, bail, ensure};
use apps_runtime::{bundle_source, html::Document};
use bbtree::unsync::BBTreeMap;
use oxc_allocator::{Allocator, ArenaVec};
use oxc_ast::{ast::*, builder::AstBuilder};
use oxc_ast_visit::{VisitMut, walk_mut};
use oxc_codegen::{Codegen, CodegenOptions, CommentOptions};
use oxc_minifier::{Minifier, MinifierOptions};
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::{GetSpan, SourceType};
use oxc_transformer::{TransformOptions, Transformer};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::{BTreeSet, VecDeque},
    fmt::Write,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Realm {
    Main,
    Preload,
    Renderer,
}

struct Builder {
    realm: Realm,
    production: bool,
    modules: Vec<Value>,
    ids: BBTreeMap<PathBuf, usize>,
    dependencies: BTreeSet<usize>,
    package_roots: BTreeSet<PathBuf>,
    dynamic: BTreeSet<usize>,
    assets: BBTreeMap<PathBuf, (String, String)>,
    files: Rc<RefCell<BTreeSet<PathBuf>>>,
}

struct Linker<'a, 'b> {
    builder: &'b mut Builder,
    allocator: &'a Allocator,
    ast: AstBuilder<'a>,
    scoping: &'b oxc_semantic::Scoping,
    path: &'b Path,
    commonjs: bool,
    error: Option<anyhow::Error>,
}

impl<'a> VisitMut<'a> for Linker<'a, '_> {
    fn visit_expression(&mut self, expression: &mut Expression<'a>) {
        if self.error.is_some() {
            return;
        }
        let result = (|| -> Result<()> {
            let span = expression.span();
            if let Expression::NewExpression(worker) = expression
                && worker.callee.is_specific_id("Worker")
                && let Some(Argument::NewExpression(url)) = worker.arguments.first_mut()
                && url.callee.is_specific_id("URL")
                && let [
                    Argument::StringLiteral(specifier),
                    Argument::StaticMemberExpression(base),
                ] = url.arguments.as_slice()
                && base.property.name == "url"
                && matches!(
                    base.object.get_inner_expression(),
                    Expression::ImportMeta(_)
                )
            {
                let specifier = specifier.value.as_str().context("Invalid worker path")?;
                ensure!(
                    specifier.starts_with('.'),
                    "Worker URL must be a relative path"
                );
                let target =
                    resolve_file(&self.path.parent().unwrap_or(Path::new(".")).join(specifier))?;
                ensure!(
                    target != self.path,
                    "A worker cannot bundle its own entry recursively"
                );
                let worker_files = compile(
                    &target,
                    if self.builder.realm == Realm::Renderer {
                        Realm::Renderer
                    } else {
                        Realm::Main
                    },
                    None,
                    self.builder.files.clone(),
                    self.builder.production,
                )?;
                let code = script(&worker_files, self.builder.production)?;
                let mut value = String::with_capacity(code.len() + 32);
                value.push_str("data:application/javascript,");
                for character in code.chars() {
                    if matches!(character, '%' | '#' | '\x7f') || character < ' ' {
                        let _ = write!(value, "%{:02X}", character as u8);
                    } else {
                        value.push(character);
                    }
                }
                let value = self.allocator.alloc_str(&value);
                worker.arguments[0] =
                    Expression::new_string_literal(url.span, value, None, &self.ast).into();
            }
            if let Expression::StaticMemberExpression(member) = expression {
                if member.property.name == "NODE_ENV"
                    && let Expression::StaticMemberExpression(env) =
                        member.object.get_inner_expression()
                    && env.property.name == "env"
                    && let Expression::Identifier(process) = env.object.get_inner_expression()
                    && process.name == "process"
                    && self
                        .scoping
                        .get_reference(process.reference_id())
                        .symbol_id()
                        .is_none()
                {
                    *expression = Expression::new_string_literal(
                        span,
                        if self.builder.production {
                            "production"
                        } else {
                            "development"
                        },
                        None,
                        &self.ast,
                    );
                    return Ok(());
                }
                if matches!(
                    member.object.get_inner_expression(),
                    Expression::ImportMeta(_)
                ) && member.property.name == "url"
                {
                    let url = url::Url::from_file_path(std::path::absolute(self.path)?).map_err(
                        |_| anyhow::anyhow!("Cannot create a file URL for {}", self.path.display()),
                    )?;
                    let value = self.allocator.alloc_str(url.as_str());
                    *expression = Expression::new_string_literal(span, value, None, &self.ast);
                    return Ok(());
                }
                ensure!(
                    self.commonjs
                        || !(member.object.is_specific_id("module")
                            && member.property.name == "exports"),
                    "ESM only: module is not supported in {}",
                    self.path.display()
                );
            }
            if let Expression::CallExpression(call) = expression
                && let Expression::StaticMemberExpression(member) =
                    call.callee.get_inner_expression()
                && matches!(
                    member.object.get_inner_expression(),
                    Expression::ImportMeta(_)
                )
                && member.property.name == "resolve"
            {
                let [Argument::StringLiteral(specifier)] = call.arguments.as_slice() else {
                    bail!(
                        "Use import.meta.resolve with one string literal in {}",
                        self.path.display()
                    );
                };
                ensure!(
                    self.builder.realm == Realm::Main,
                    "HTML files can only be resolved in main"
                );
                let specifier = specifier.value.as_str().context("Invalid HTML path")?;
                ensure!(
                    (specifier.starts_with('.') || Path::new(specifier).is_absolute())
                        && !specifier.contains("://")
                        && !specifier.starts_with("//"),
                    "import.meta.resolve needs a local HTML path"
                );
                let target = self.path.parent().unwrap_or(Path::new(".")).join(specifier);
                ensure!(
                    target.extension().is_some_and(|e| e == "html"),
                    "import.meta.resolve only supports HTML files"
                );
                let target = target
                    .canonicalize()
                    .with_context(|| format!("Cannot resolve {}", target.display()))?;
                if !self.builder.assets.contains_key(&target) {
                    self.builder.files.borrow_mut().insert(target.clone());
                    let name = format!("page-{}.html", self.builder.assets.len());
                    let source =
                        html(&target, self.builder.files.clone(), self.builder.production)?;
                    self.builder.assets.insert(target.clone(), (name, source));
                }
                let value = self
                    .allocator
                    .alloc_str(&format!("apps:///{}", self.builder.assets[&target].0));
                *expression = Expression::new_string_literal(span, value, None, &self.ast);
                return Ok(());
            }
            let import = match expression {
                Expression::ImportExpression(import) => {
                    let Expression::StringLiteral(specifier) = import.source.get_inner_expression()
                    else {
                        bail!(
                            "Dynamic import needs one literal specifier in {}",
                            self.path.display()
                        );
                    };
                    Some((
                        "$appsImport",
                        specifier.value.as_str().context("Invalid module path")?,
                    ))
                }
                Expression::CallExpression(call) if call.callee.is_specific_id("require") => {
                    let Expression::Identifier(identifier) = call.callee.get_inner_expression()
                    else {
                        unreachable!()
                    };
                    if self
                        .scoping
                        .get_reference(identifier.reference_id())
                        .symbol_id()
                        .is_some()
                    {
                        None
                    } else {
                        ensure!(
                            self.commonjs,
                            "ESM only: require is not supported in {}",
                            self.path.display()
                        );
                        let [Argument::StringLiteral(specifier)] = call.arguments.as_slice() else {
                            bail!(
                                "Package require needs one string literal in {}",
                                self.path.display()
                            );
                        };
                        Some((
                            "$appsRequire",
                            specifier.value.as_str().context("Invalid module path")?,
                        ))
                    }
                }
                Expression::ImportMeta(_) => bail!(
                    "Use import.meta.resolve with one string literal or import.meta.url in {}",
                    self.path.display()
                ),
                _ => None,
            };
            if let Some((name, specifier)) = import {
                let dep = self.builder.resolve(
                    self.path.parent().unwrap_or(Path::new(".")),
                    specifier,
                    name == "$appsRequire",
                )?;
                self.builder.dynamic.insert(dep);
                *expression = Expression::new_call_expression(
                    span,
                    Expression::new_identifier(span, name, &self.ast),
                    None,
                    ArenaVec::from_array_in(
                        [Expression::new_numeric_literal(
                            span,
                            dep as f64,
                            None,
                            NumberBase::Decimal,
                            &self.ast,
                        )
                        .into()],
                        &self.ast,
                    ),
                    false,
                    &self.ast,
                );
                return Ok(());
            }
            walk_mut::walk_expression(self, expression);
            Ok(())
        })();
        if let Err(error) = result {
            self.error = Some(error);
        }
    }
}

impl Builder {
    fn resolve(&mut self, base: &Path, specifier: &str, require: bool) -> Result<usize> {
        let normalized;
        let specifier = if matches!(
            specifier,
            "fs" | "fs/promises"
                | "path"
                | "os"
                | "events"
                | "child_process"
                | "buffer"
                | "stream"
                | "stream/promises"
                | "stream/web"
                | "crypto"
                | "url"
                | "util"
                | "string_decoder"
                | "worker_threads"
                | "module"
                | "net"
                | "http"
                | "https"
                | "timers"
                | "timers/promises"
                | "process"
        ) && (self.realm != Realm::Renderer
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
        if matches!(specifier, "@pierre/apps" | "electron" | "node-pty" | "ws")
            || specifier.starts_with("node:")
        {
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
            if let Some(&id) = self.ids.get(&path) {
                return Ok(id);
            }
            let (object, names) = match specifier {
                "@pierre/apps" | "electron" => (
                    "globalThis.__apps",
                    vec![
                        "app",
                        "BrowserWindow",
                        "View",
                        "WebContentsView",
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
                        "globalShortcut",
                        "session",
                        "powerMonitor",
                    ],
                ),
                "ws" => (
                    "globalThis.__apps.ws",
                    vec![
                        "WebSocket",
                        "WebSocketServer",
                        "Server",
                        "OPEN",
                        "CLOSED",
                        "CONNECTING",
                        "CLOSING",
                    ],
                ),
                "node:net" => (
                    "globalThis.__apps.net",
                    vec![
                        "Socket",
                        "Server",
                        "connect",
                        "createConnection",
                        "createServer",
                        "isIP",
                        "isIPv4",
                        "isIPv6",
                    ],
                ),
                "node:https" => ("globalThis.__apps.https", vec!["request", "get"]),
                "node:http" => (
                    "globalThis.__apps.http",
                    vec![
                        "Server",
                        "createServer",
                        "request",
                        "get",
                        "STATUS_CODES",
                        "METHODS",
                    ],
                ),
                "node-pty" => ("globalThis.__apps.pty", vec!["spawn"]),
                "node:buffer" => ("globalThis.__apps.buffer", vec!["Buffer"]),
                "node:stream" => (
                    "globalThis.__apps.stream",
                    vec![
                        "Readable",
                        "Writable",
                        "Duplex",
                        "Transform",
                        "PassThrough",
                        "Stream",
                        "pipeline",
                        "finished",
                    ],
                ),
                "node:stream/promises" => (
                    "globalThis.__apps.stream.promises",
                    vec!["pipeline", "finished"],
                ),
                "node:stream/web" => (
                    "globalThis",
                    vec![
                        "ReadableStream",
                        "WritableStream",
                        "TransformStream",
                        "ByteLengthQueuingStrategy",
                        "CountQueuingStrategy",
                    ],
                ),
                "node:util" => (
                    "globalThis.__apps.util",
                    vec![
                        "promisify",
                        "callbackify",
                        "inspect",
                        "format",
                        "types",
                        "TextEncoder",
                        "TextDecoder",
                    ],
                ),
                "node:url" => (
                    "globalThis.__apps.url",
                    vec!["URL", "URLSearchParams", "fileURLToPath", "pathToFileURL"],
                ),
                "node:string_decoder" => {
                    ("globalThis.__apps.string_decoder", vec!["StringDecoder"])
                }
                "node:crypto" => (
                    "globalThis.__apps.crypto",
                    vec![
                        "createHash",
                        "createHmac",
                        "randomBytes",
                        "randomUUID",
                        "timingSafeEqual",
                        "webcrypto",
                    ],
                ),
                "node:worker_threads" => (
                    "globalThis.__apps.worker_threads",
                    vec![
                        "Worker",
                        "MessageChannel",
                        "MessagePort",
                        "isMainThread",
                        "parentPort",
                        "workerData",
                        "threadId",
                    ],
                ),
                "node:module" => (
                    "globalThis.__apps.module",
                    vec!["createRequire", "builtinModules", "isBuiltin"],
                ),
                "node:timers" => (
                    "globalThis",
                    vec![
                        "setTimeout",
                        "clearTimeout",
                        "setInterval",
                        "clearInterval",
                        "setImmediate",
                        "clearImmediate",
                    ],
                ),
                "node:timers/promises" => (
                    "globalThis.__apps.timers",
                    vec!["setTimeout", "setImmediate", "scheduler"],
                ),
                "node:process" => (
                    "globalThis.__apps.process",
                    vec![
                        "platform", "arch", "pid", "argv", "env", "versions", "cwd", "chdir",
                        "nextTick", "exit", "on", "once", "off", "kill", "hrtime", "execPath",
                        "stdout", "stderr",
                    ],
                ),
                "node:sqlite" => (
                    "globalThis.__apps.sqlite",
                    vec![
                        "DatabaseSync",
                        "StatementSync",
                        "Database",
                        "Statement",
                        "backup",
                        "constants",
                    ],
                ),
                "node:events" => ("globalThis.__apps.events", vec!["EventEmitter", "once"]),
                "node:ffi" => (
                    "globalThis.__apps.ffi",
                    vec![
                        "DynamicLibrary",
                        "suffix",
                        "types",
                        "dlopen",
                        "dlclose",
                        "dlsym",
                        "getRawPointer",
                        "toString",
                        "toBuffer",
                        "toArrayBuffer",
                        "exportString",
                        "exportBuffer",
                        "exportArrayBuffer",
                        "exportArrayBufferView",
                        "getCurrentEventLoop",
                        "getInt8",
                        "getUint8",
                        "getInt16",
                        "getUint16",
                        "getInt32",
                        "getUint32",
                        "getInt64",
                        "getUint64",
                        "getFloat32",
                        "getFloat64",
                        "setInt8",
                        "setUint8",
                        "setInt16",
                        "setUint16",
                        "setInt32",
                        "setUint32",
                        "setInt64",
                        "setUint64",
                        "setFloat32",
                        "setFloat64",
                    ],
                ),
                "node:child_process" => (
                    "globalThis.__apps.child_process",
                    vec![
                        "spawn",
                        "ChildProcess",
                        "execFile",
                        "exec",
                        "spawnSync",
                        "execFileSync",
                        "execSync",
                    ],
                ),
                "node:fs" => (
                    "globalThis.__apps.fs",
                    vec![
                        "constants",
                        "promises",
                        "readFile",
                        "writeFile",
                        "appendFile",
                        "mkdir",
                        "readdir",
                        "stat",
                        "lstat",
                        "rename",
                        "copyFile",
                        "realpath",
                        "readlink",
                        "symlink",
                        "chmod",
                        "access",
                        "unlink",
                        "rm",
                        "rmdir",
                        "mkdtemp",
                        "open",
                        "close",
                        "readFileSync",
                        "writeFileSync",
                        "appendFileSync",
                        "existsSync",
                        "mkdirSync",
                        "readdirSync",
                        "statSync",
                        "lstatSync",
                        "renameSync",
                        "copyFileSync",
                        "realpathSync",
                        "readlinkSync",
                        "symlinkSync",
                        "chmodSync",
                        "accessSync",
                        "unlinkSync",
                        "rmSync",
                        "rmdirSync",
                        "mkdtempSync",
                        "openSync",
                        "closeSync",
                        "readSync",
                        "writeSync",
                        "fsyncSync",
                        "ftruncateSync",
                        "watch",
                        "createReadStream",
                        "createWriteStream",
                    ],
                ),
                "node:fs/promises" => (
                    "globalThis.__apps.fs.promises",
                    vec![
                        "readFile",
                        "writeFile",
                        "appendFile",
                        "mkdir",
                        "readdir",
                        "stat",
                        "lstat",
                        "rename",
                        "copyFile",
                        "realpath",
                        "readlink",
                        "symlink",
                        "chmod",
                        "access",
                        "unlink",
                        "rm",
                        "rmdir",
                        "mkdtemp",
                        "open",
                    ],
                ),
                "node:os" => (
                    "globalThis.__apps.os",
                    vec![
                        "platform",
                        "arch",
                        "homedir",
                        "tmpdir",
                        "hostname",
                        "availableParallelism",
                        "type",
                        "EOL",
                    ],
                ),
                "node:path" => (
                    "globalThis.__apps.path",
                    vec![
                        "sep",
                        "delimiter",
                        "normalize",
                        "isAbsolute",
                        "join",
                        "resolve",
                        "basename",
                        "dirname",
                        "extname",
                        "relative",
                        "posix",
                        "win32",
                    ],
                ),
                _ => bail!("Unsupported built-in module: {specifier}"),
            };
            let source = format!(
                "const api={object};export default api;{}",
                names
                    .iter()
                    .map(|name| format!("export const {name}=api.{name};"))
                    .collect::<String>()
            );
            return self.visit(path, Some(source));
        }
        ensure!(
            !specifier.contains("://") && !specifier.starts_with("//"),
            "Only local ESM imports are supported: {specifier}"
        );
        if specifier.starts_with('.') || Path::new(specifier).is_absolute() {
            let path = base.join(specifier);
            let package = path
                .components()
                .any(|part| part.as_os_str() == "node_modules");
            let path = resolve_file(&path)?;
            if package && let Some(parent) = path.parent() {
                self.package_roots.insert(parent.to_owned());
            }
            return self.visit(path, None);
        }
        let imports = specifier.starts_with('#');
        let (directory, key) = if imports {
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
        self.files.borrow_mut().insert(package_file.clone());
        let metadata: Value = serde_json::from_str(
            &fs::read_to_string(&package_file)
                .with_context(|| format!("Cannot read {}", package_file.display()))?,
        )
        .with_context(|| format!("Invalid package metadata in {}", package_file.display()))?;
        let entry = if let Some(exports) = metadata.get(if imports { "imports" } else { "exports" })
        {
            let mut replacement = None;
            let selected = if let Some(map) = exports
                .as_object()
                .filter(|map| map.keys().any(|k| k.starts_with(['.', '#'])))
            {
                if let Some(value) = map.get(&key) {
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
            let conditions = [
                if self.realm == Realm::Renderer {
                    "browser"
                } else {
                    "node"
                },
                if require { "require" } else { "import" },
                "default",
            ];
            let mut candidates = vec![selected];
            let mut path = None;
            while let Some(value) = candidates.pop() {
                match value {
                    Value::String(value) => {
                        path = Some(value.as_str());
                        break;
                    }
                    Value::Array(values) => candidates.extend(values.iter().rev()),
                    Value::Object(values) => {
                        if let Some((_, value)) = values
                            .iter()
                            .find(|(key, _)| conditions.contains(&key.as_str()))
                        {
                            candidates.push(value);
                        }
                    }
                    _ => {}
                }
            }
            let value = path.context("Package has no supported export condition")?;
            let value = if let Some(replacement) = replacement {
                value.replace('*', replacement)
            } else {
                value.to_owned()
            };
            if imports && !value.starts_with(['.', '/', '#']) {
                return self.resolve(&directory, &value, require);
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
            metadata
                .get("browser")
                .filter(|_| self.realm == Realm::Renderer)
                .and_then(Value::as_str)
                .or_else(|| metadata.get("module").and_then(Value::as_str))
                .or_else(|| metadata.get("main").and_then(Value::as_str))
                .unwrap_or("index.js")
                .to_owned()
        };
        let target = resolve_file(&directory.join(entry))?;
        ensure!(
            target.starts_with(&directory),
            "Package export escapes its directory"
        );
        self.visit(target, None)
    }

    fn visit(&mut self, path: PathBuf, inline: Option<String>) -> Result<usize> {
        if let Some(&id) = self.ids.get(&path) {
            return Ok(id);
        }
        let id = self.modules.len();
        self.ids.insert(path.clone(), id);
        self.modules.push(Value::Null);
        let dependency = path.components().any(|p| p.as_os_str() == "node_modules")
            || self.package_roots.iter().any(|root| path.starts_with(root));
        if dependency {
            self.dependencies.insert(id);
        }
        if inline.is_none()
            && path
                .extension()
                .is_some_and(|extension| extension == "node")
        {
            ensure!(
                self.realm != Realm::Renderer,
                "Native addons are unavailable in page scripts. Use a preload script."
            );
            self.files.borrow_mut().insert(path.clone());
            let name = native_specifier(&path)?;
            self.modules[id] = json!({"id": id, "code": format!("module.exports=globalThis.__appsLoadAddon({});", serde_json::to_string(&name)?), "imports": [], "exports": [], "stars": [], "commonjs": true});
            return Ok(id);
        }
        let inline_html = inline.is_some() && path.extension().is_some_and(|e| e == "html");
        let source = if let Some(source) = inline {
            source
        } else {
            self.files.borrow_mut().insert(path.clone());
            match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
                "html" => {
                    ensure!(
                        self.realm == Realm::Main,
                        "HTML imports are only supported in main"
                    );
                    format!(
                        "export default {};",
                        serde_json::to_string(&html(&path, self.files.clone(), self.production)?)?
                    )
                }
                "json" => {
                    let value: Value = serde_json::from_str(&fs::read_to_string(&path)?)?;
                    format!("export default {value};")
                }
                "cjs" | "cts" if !dependency => bail!(
                    "CommonJS is only supported in npm packages: {}",
                    path.display()
                ),
                _ => fs::read_to_string(&path)
                    .with_context(|| format!("Cannot read {}", path.display()))?,
            }
        };
        let allocator = Allocator::default();
        let source_type = if inline_html {
            SourceType::tsx()
        } else {
            SourceType::from_path(&path).unwrap_or(SourceType::mjs())
        };
        let parsed = Parser::new(&allocator, &source, source_type).parse();
        ensure!(
            parsed.diagnostics.is_empty(),
            "Invalid source in {}: {:?}",
            path.display(),
            parsed.diagnostics
        );
        let mut program = parsed.program;
        let semantic = SemanticBuilder::new()
            .with_check_syntax_error(true)
            .with_enum_eval(true)
            .build(&program);
        ensure!(
            semantic.diagnostics.is_empty(),
            "Invalid source in {}: {:?}",
            path.display(),
            semantic.diagnostics
        );
        let scoping = semantic.semantic.into_scoping();
        for name in scoping.symbol_names().chain(
            scoping
                .root_unresolved_references()
                .keys()
                .map(|name| name.as_str()),
        ) {
            ensure!(
                !name.starts_with("$apps") && !matches!(name, "$scope" | "$out"),
                "Reserved apps identifier: {name}"
            );
        }
        let types: Vec<_> = program
            .body
            .iter()
            .filter_map(|statement| match statement {
                Statement::ImportDeclaration(import) => import.specifiers.as_ref(),
                _ => None,
            })
            .flatten()
            .map(|specifier| specifier.local().symbol_id())
            .filter(|&symbol| {
                let mut references = scoping.get_resolved_references(symbol).peekable();
                references.peek().is_some() && references.all(|r| r.is_type())
            })
            .collect();
        let mut options = TransformOptions::default();
        options.jsx.development = !self.production;
        options.typescript.only_remove_type_imports = true;
        let transformed =
            Transformer::new(&allocator, &path, &options).build_with_scoping(scoping, &mut program);
        ensure!(
            transformed.diagnostics.is_empty(),
            "Cannot transform {}: {:?}",
            path.display(),
            transformed.diagnostics
        );
        let esm = program.body.iter().any(Statement::is_module_declaration);
        let references = transformed.scoping.root_unresolved_references();
        let commonjs = !esm
            && dependency
            && (path.extension().is_some_and(|e| e == "cjs" || e == "cts")
                || ["require", "module", "exports"]
                    .iter()
                    .any(|name| references.contains_key(*name)));
        let mut linker = Linker {
            builder: self,
            allocator: &allocator,
            ast: AstBuilder::new(&allocator),
            scoping: &transformed.scoping,
            path: &path,
            commonjs,
            error: None,
        };
        linker.visit_program(&mut program);
        if let Some(error) = linker.error {
            return Err(error);
        }
        let ast = AstBuilder::new(&allocator);
        let mut imports = Vec::new();
        let mut exports = Vec::new();
        let mut stars = Vec::new();
        let mut body = ArenaVec::new_in(&ast);
        for statement in program.body {
            match statement {
                Statement::ImportDeclaration(import) => {
                    let dep = self.resolve(
                        path.parent().unwrap_or(Path::new(".")),
                        import
                            .source
                            .value
                            .as_str()
                            .context("Invalid module path")?,
                        false,
                    )?;
                    imports.push(json!({"id":dep,"local":null,"name":"*"}));
                    for specifier in import.unbox().specifiers.into_iter().flatten() {
                        if types.contains(&specifier.local().symbol_id()) {
                            continue;
                        }
                        let (name, local) = match specifier {
                            ImportDeclarationSpecifier::ImportSpecifier(specifier) => (
                                specifier.imported.name().to_string(),
                                specifier.local.name.to_string(),
                            ),
                            ImportDeclarationSpecifier::ImportDefaultSpecifier(specifier) => {
                                ("default".into(), specifier.local.name.to_string())
                            }
                            ImportDeclarationSpecifier::ImportNamespaceSpecifier(specifier) => {
                                ("*".into(), specifier.local.name.to_string())
                            }
                        };
                        imports.push(json!({"id":dep,"local":local,"name":name}));
                    }
                }
                Statement::ExportDeclaration(export) => {
                    let declaration = export.unbox().declaration;
                    let names = match &declaration {
                        Declaration::VariableDeclaration(variable) => variable
                            .declarations
                            .iter()
                            .flat_map(|declaration| declaration.id.get_binding_identifiers())
                            .map(|id| id.name.to_string())
                            .collect::<Vec<_>>(),
                        Declaration::FunctionDeclaration(function) => vec![
                            function
                                .id
                                .as_ref()
                                .context("Missing export name")?
                                .name
                                .to_string(),
                        ],
                        Declaration::ClassDeclaration(class) => vec![
                            class
                                .id
                                .as_ref()
                                .context("Missing export name")?
                                .name
                                .to_string(),
                        ],
                        _ => bail!("Unsupported export in {}", path.display()),
                    };
                    for name in names {
                        exports.push(json!({"name":name,"value":name}));
                    }
                    body.push(Statement::from(declaration));
                }
                Statement::ExportNamedDeclaration(export) => {
                    for specifier in &export.specifiers {
                        exports.push(json!({"name":specifier.exported.name().as_str(),"value":specifier.local.name().as_str()}));
                    }
                }
                Statement::ExportFromDeclaration(export) => {
                    let dep = self.resolve(
                        path.parent().unwrap_or(Path::new(".")),
                        export
                            .source
                            .value
                            .as_str()
                            .context("Invalid module path")?,
                        false,
                    )?;
                    imports.push(json!({"id":dep,"local":null,"name":"*"}));
                    for specifier in &export.specifiers {
                        let local = format!("$appsRe{}", imports.len());
                        imports.push(
                            json!({"id":dep,"local":local,"name":specifier.local.name().as_str()}),
                        );
                        exports
                            .push(json!({"name":specifier.exported.name().as_str(),"value":local}));
                    }
                }
                Statement::ExportAllDeclaration(export) => {
                    let dep = self.resolve(
                        path.parent().unwrap_or(Path::new(".")),
                        export
                            .source
                            .value
                            .as_str()
                            .context("Invalid module path")?,
                        false,
                    )?;
                    if let Some(name) = &export.exported {
                        let local = format!("$appsRe{}", imports.len());
                        imports.push(json!({"id":dep,"local":local,"name":"*"}));
                        exports.push(json!({"name":name.name().as_str(),"value":local}));
                    } else {
                        stars.push(dep);
                    }
                }
                Statement::ExportDefaultDeclaration(export) => {
                    let span = export.span;
                    let expression = match export.unbox().declaration {
                        ExportDefaultDeclarationKind::FunctionDeclaration(mut function) => {
                            if function.id.is_none() {
                                function.id =
                                    Some(BindingIdentifier::new(span, "$appsDefault", &ast));
                            }
                            exports.push(json!({"name":"default","value":function.id.as_ref().unwrap().name.as_str()}));
                            body.push(Statement::FunctionDeclaration(function));
                            continue;
                        }
                        ExportDefaultDeclarationKind::ClassDeclaration(mut class) => {
                            if class.id.is_none() {
                                class.id = Some(BindingIdentifier::new(span, "$appsDefault", &ast));
                            }
                            exports.push(json!({"name":"default","value":class.id.as_ref().unwrap().name.as_str()}));
                            body.push(Statement::ClassDeclaration(class));
                            continue;
                        }
                        declaration => declaration.into_expression(),
                    };
                    exports.push(json!({"name":"default","value":"$appsDefault"}));
                    body.push(Statement::new_variable_declaration(
                        span,
                        VariableDeclarationKind::Const,
                        ArenaVec::from_array_in(
                            [VariableDeclarator::new(
                                span,
                                BindingPattern::BindingIdentifier(BindingIdentifier::boxed(
                                    span,
                                    "$appsDefault",
                                    &ast,
                                )),
                                None,
                                Some(expression),
                                false,
                                &ast,
                            )],
                            &ast,
                        ),
                        false,
                        &ast,
                    ));
                }
                statement => body.push(statement),
            }
        }
        program.body = body;
        program.hashbang = None;
        let output = Codegen::new()
            .with_options(CodegenOptions {
                source_map_path: (!self.production).then(|| path.clone()),
                ..CodegenOptions::default()
            })
            .build(&program);
        let mut module = json!({"id":id,"code":output.code,"imports":imports,"exports":exports,"stars":stars,"commonjs":commonjs});
        if let Some(map) = output.map {
            let mut map: Value = serde_json::from_str(&map.to_json_string())?;
            // WARNING: The runtime adds one line before the module body.
            map["mappings"] = format!(";{}", map["mappings"].as_str().unwrap()).into();
            module["map"] = map;
            module["url"] = url::Url::from_file_path(std::path::absolute(&path)?)
                .map(|url| Value::String(url.into()))
                .unwrap_or_else(|_| Value::String(path.to_string_lossy().into()));
        }
        self.modules[id] = module;
        Ok(id)
    }
}

pub fn compile(
    entry: &Path,
    realm: Realm,
    inline: Option<String>,
    files: Rc<RefCell<BTreeSet<PathBuf>>>,
    production: bool,
) -> Result<BBTreeMap<String, String>> {
    let mut builder = Builder {
        realm,
        production,
        modules: Vec::new(),
        ids: BBTreeMap::new(),
        dependencies: BTreeSet::new(),
        package_roots: BTreeSet::new(),
        dynamic: BTreeSet::new(),
        assets: BBTreeMap::new(),
        files,
    };
    builder.visit(entry.to_owned(), inline)?;
    if production {
        let modules = &mut builder.modules;
        let mut available: Vec<BTreeSet<String>> = modules
            .iter()
            .map(|module| {
                module["exports"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|export| export["name"].as_str().unwrap().to_owned())
                    .collect()
            })
            .collect();
        let mut dynamic_stars: Vec<_> = modules
            .iter()
            .map(|module| module["commonjs"] == true)
            .collect();
        loop {
            let mut changed = false;
            for (id, module) in modules.iter().enumerate() {
                for star in module["stars"].as_array().unwrap() {
                    let target = star.as_u64().unwrap() as usize;
                    if dynamic_stars[target] && !dynamic_stars[id] {
                        dynamic_stars[id] = true;
                        changed = true;
                    }
                    let names = available[target].clone();
                    for name in names {
                        if name != "default" {
                            changed |= available[id].insert(name);
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        for module in modules.iter() {
            for import in module["imports"].as_array().unwrap() {
                let target = import["id"].as_u64().unwrap() as usize;
                let name = import["name"].as_str().unwrap();
                ensure!(
                    import["local"].is_null()
                        || name == "*"
                        || dynamic_stars[target]
                        || available[target].contains(name),
                    "Missing export {name} from {target}"
                );
            }
        }

        let mut needed = vec![BTreeSet::new(); modules.len()];
        needed[0] = available[0].clone();
        for &id in &builder.dynamic {
            needed[id] = available[id].clone();
        }
        let mut used_imports = vec![BTreeSet::new(); modules.len()];
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
                let imports = module["imports"].as_array().unwrap();
                let target = imports
                    .get(*next)
                    .map(|import| &import["id"])
                    .or_else(|| module["stars"].get(*next - imports.len()));
                if let Some(target) = target {
                    *next += 1;
                    let target = target.as_u64().unwrap() as usize;
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
            let exports = module["exports"].as_array().unwrap();
            let mut source = if module["commonjs"] == true {
                format!(
                    "$appsFactory=function(module,exports,$appsRequire,$appsImport){{\n{}\n}};",
                    module["code"].as_str().unwrap()
                )
            } else {
                let getters = exports
                    .iter()
                    .filter(|export| needed[id].contains(export["name"].as_str().unwrap()))
                    .map(|export| {
                        format!(
                            "[{}]:{{enumerable:true,get:()=>({})}}",
                            export["name"],
                            export["value"].as_str().unwrap()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                format!(
                    "$appsFactory=function*($out,$appsBind){{'use strict';$appsBind($out,{{{getters}}});yield;\n{}\n}};",
                    module["code"].as_str().unwrap()
                )
            };
            let mut allocator = Allocator::default();
            let parsed = Parser::new(&allocator, &source, SourceType::cjs()).parse();
            let asynchronous = !parsed.diagnostics.is_empty() && module["commonjs"] != true;
            let mut parsed = if asynchronous {
                drop(parsed);
                allocator.reset();
                source = source.replacen("=function*(", "=async function*(", 1);
                Parser::new(&allocator, &source, SourceType::cjs()).parse()
            } else {
                parsed
            };
            ensure!(
                parsed.diagnostics.is_empty(),
                "Invalid JavaScript in module {id} ({}): {:?}",
                module["url"],
                parsed.diagnostics
            );
            let result =
                Minifier::new(MinifierOptions::default()).minify(&allocator, &mut parsed.program);
            let scoping = result.scoping.as_ref().unwrap();
            let references = scoping.root_unresolved_references();
            let eval = references.contains_key("eval");
            used_imports[id].clear();
            let mut requests = Vec::new();
            for import in module["imports"].as_array().unwrap() {
                let Some(local) = import["local"].as_str() else {
                    continue;
                };
                if eval || references.contains_key(local) {
                    used_imports[id].insert(local.to_owned());
                    let target = import["id"].as_u64().unwrap() as usize;
                    let name = import["name"].as_str().unwrap();
                    if name == "*" {
                        requests
                            .extend(available[target].iter().map(|name| (target, name.clone())));
                    } else {
                        requests.push((target, name.to_owned()));
                    }
                }
            }
            for star in module["stars"].as_array().unwrap() {
                let target = star.as_u64().unwrap() as usize;
                for name in &needed[id] {
                    if name != "default"
                        && available[target].contains(name)
                        && !exports.iter().any(|export| export["name"] == *name)
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
            let Some(Statement::ExpressionStatement(statement)) = parsed.program.body.first()
            else {
                bail!("Missing module factory: {id}");
            };
            let Expression::AssignmentExpression(assignment) = &statement.expression else {
                bail!("Invalid module factory: {id}");
            };
            let mut codegen = Codegen::new()
                .with_options(CodegenOptions {
                    minify: true,
                    comments: CommentOptions::disabled(),
                    ..CodegenOptions::default()
                })
                .with_scoping(result.scoping)
                .with_private_member_mappings(result.class_private_mappings);
            codegen.print_expression(&assignment.right);
            modules[id]["factory"] = Value::String(codegen.into_source_text());
            modules[id]["async"] = Value::Bool(asynchronous);
        }
        for (id, module) in modules.iter_mut().enumerate() {
            for import in module["imports"].as_array_mut().unwrap() {
                if import["local"]
                    .as_str()
                    .is_some_and(|local| !used_imports[id].contains(local))
                {
                    import["local"] = Value::Null;
                }
            }
            let module = module.as_object_mut().unwrap();
            module.remove("code");
            module.remove("exports");
        }
    }
    let mut app = String::new();
    let mut deps = String::new();
    for (id, module) in builder.modules.iter().enumerate() {
        let target = if builder.dependencies.contains(&id) {
            &mut deps
        } else {
            &mut app
        };
        target.push_str("define(");
        target.push_str(&serde_json::to_string(module)?.replace('<', "\\u003c"));
        target.push_str(");\n");
    }
    let mut files = BBTreeMap::from([("app.js".into(), app)]);
    if !deps.is_empty() {
        files.insert("deps.js".into(), deps);
    }
    for (_, (name, source)) in builder.assets {
        files.insert(name, source);
    }
    let mut native = std::collections::BTreeMap::new();
    for path in builder.files.borrow().iter().filter(|path| {
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
    let assets: std::collections::BTreeMap<_, _> = builder
        .files
        .borrow()
        .iter()
        .filter_map(|path| Some((asset_name(path)?, path.clone())))
        .collect();
    if !assets.is_empty() {
        files.insert("assets.json".into(), serde_json::to_string(&assets)?);
    }
    if realm == Realm::Main {
        let root = entry.parent().context("Missing entry directory")?;
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
            builder.files.borrow_mut().insert(path);
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
                            if apps_runtime::bundle::package_metadata(Some(&candidate.to_string()))
                                .is_ok()
                            {
                                package = candidate;
                                break;
                            }
                        }
                    }
                }
                builder.files.borrow_mut().insert(path);
            }
        }
        let (name, product_name, version) =
            apps_runtime::bundle::package_metadata(Some(&package.to_string()))?;
        files.insert(
            "package.json".into(),
            json!({"name":name,"productName":product_name,"version":version}).to_string(),
        );
    }
    Ok(files)
}

fn script(files: &BBTreeMap<String, String>, production: bool) -> Result<String> {
    let source = bundle_source(files, true)?;
    if !production {
        return Ok(source);
    }
    let allocator = Allocator::default();
    let mut parsed = Parser::new(&allocator, &source, SourceType::cjs()).parse();
    ensure!(
        parsed.diagnostics.is_empty(),
        "Invalid script bundle: {:?}",
        parsed.diagnostics
    );
    let result = Minifier::new(MinifierOptions::default()).minify(&allocator, &mut parsed.program);
    Ok(Codegen::new()
        .with_options(CodegenOptions {
            minify: true,
            comments: CommentOptions::disabled(),
            ..CodegenOptions::default()
        })
        .with_scoping(result.scoping)
        .with_private_member_mappings(result.class_private_mappings)
        .build(&parsed.program)
        .code)
}

fn native_specifier(path: &Path) -> Result<String> {
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

fn resolve_file(path: &Path) -> Result<PathBuf> {
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

fn asset_name(path: &Path) -> Option<String> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "bmp" | "ico" | "svg"
    ) {
        return None;
    }
    let mut hasher = std::hash::DefaultHasher::new();
    std::hash::Hash::hash(path, &mut hasher);
    Some(format!(
        "assets/{:016x}.{extension}",
        std::hash::Hasher::finish(&hasher)
    ))
}

fn asset(
    directory: &Path,
    reference: &str,
    files: &Rc<RefCell<BTreeSet<PathBuf>>>,
) -> Option<String> {
    let reference = reference.trim().trim_matches(['"', '\'']);
    if reference.contains(':') || reference.starts_with(['/', '#']) {
        return None;
    }
    let path = directory
        .join(reference.split(['?', '#']).next()?)
        .canonicalize()
        .ok()?;
    let name = asset_name(&path)?;
    files.borrow_mut().insert(path);
    Some(format!("apps:///{name}"))
}

fn urls(source: &str, directory: &Path, files: &Rc<RefCell<BTreeSet<PathBuf>>>) -> String {
    let mut result = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(start) = rest.find("url(") {
        let (head, tail) = rest.split_at(start + 4);
        result.push_str(head);
        let end = tail.find(')').unwrap_or(tail.len());
        match asset(directory, &tail[..end], files) {
            Some(url) => result.push_str(&format!("\"{url}\"")),
            None => result.push_str(&tail[..end]),
        }
        rest = &tail[end..];
    }
    result.push_str(rest);
    result
}

fn html(path: &Path, files: Rc<RefCell<BTreeSet<PathBuf>>>, production: bool) -> Result<String> {
    let mut doc = Document::parse(&fs::read_to_string(path)?);
    let parent = path.parent().context("Missing HTML directory")?;
    for node in &mut doc.nodes {
        if let Some(style) = node.attrs.get_mut("style") {
            *style = urls(style, parent, &files);
        }
        if node.tag == "img"
            && let Some(source) = node.attrs.get_mut("src")
            && let Some(url) = asset(parent, source, &files)
        {
            *source = url;
        }
        if node.tag == "style" {
            node.text = urls(&node.text, parent, &files);
        } else if node.tag == "link" && node.attrs.get("rel").is_some_and(|v| v == "stylesheet") {
            let href = node.attrs.get("href").context("Stylesheet has no href")?;
            ensure!(
                !href.contains("://") && !href.starts_with("//"),
                "Only local CSS is supported"
            );
            let file = parent.join(href).canonicalize()?;
            files.borrow_mut().insert(file.clone());
            node.tag = "style".into();
            node.text = css(&file, &mut Vec::new(), &files)?;
            node.attrs.clear();
        } else if node.tag == "script" {
            let kind = node
                .attrs
                .get("type")
                .map(String::as_str)
                .unwrap_or("module");
            ensure!(
                matches!(kind, "module" | "preload"),
                "Use type=module or type=preload for HTML scripts"
            );
            let realm = if kind == "preload" {
                Realm::Preload
            } else {
                Realm::Renderer
            };
            let scripts = if let Some(src) = node.attrs.get("src") {
                ensure!(
                    !src.contains("://") && !src.starts_with("//"),
                    "Only local scripts are supported"
                );
                compile(
                    &parent.join(src).canonicalize()?,
                    realm,
                    None,
                    files.clone(),
                    production,
                )?
            } else {
                compile(
                    path,
                    realm,
                    Some(node.text.clone()),
                    files.clone(),
                    production,
                )?
            };
            node.text = script(&scripts, production)?;
            node.attrs.remove("src");
            node.attrs.insert(
                "type".into(),
                if realm == Realm::Preload {
                    "preload"
                } else {
                    "module"
                }
                .into(),
            );
        }
    }
    Ok(doc.serialize(0))
}

fn css(
    path: &Path,
    stack: &mut Vec<PathBuf>,
    files: &Rc<RefCell<BTreeSet<PathBuf>>>,
) -> Result<String> {
    ensure!(
        !stack.contains(&path.to_owned()),
        "CSS import cycle: {}",
        path.display()
    );
    stack.push(path.to_owned());
    files.borrow_mut().insert(path.to_owned());
    let source = urls(
        &fs::read_to_string(path)?,
        path.parent().context("Missing CSS directory")?,
        files,
    );
    let mut result = String::new();
    let mut rest = source.as_str();
    loop {
        let bytes = rest.as_bytes();
        let mut pos = 0;
        let mut quote = 0;
        let mut depth = 0;
        let mut found = None;
        while pos < bytes.len() {
            let byte = bytes[pos];
            if quote != 0 {
                if byte == b'\\' {
                    pos += 2;
                    continue;
                }
                if byte == quote {
                    quote = 0;
                }
            } else if byte == b'\'' || byte == b'"' {
                quote = byte;
            } else if bytes[pos..].starts_with(b"/*") {
                pos += 2;
                pos += bytes[pos..]
                    .windows(2)
                    .position(|w| w == b"*/")
                    .map_or(bytes.len() - pos, |end| end + 2);
                continue;
            } else if byte == b'{' {
                depth += 1;
            } else if byte == b'}' {
                depth -= 1;
            } else if depth == 0 && bytes[pos..].starts_with(b"@import") {
                found = Some(pos);
                break;
            }
            pos += 1;
        }
        let Some(start) = found else {
            break;
        };
        result.push_str(&rest[..start]);
        rest = &rest[start + 7..];
        let end = rest.find(';').context("CSS @import needs a semicolon")?;
        let spec = rest[..end].trim();
        let spec = spec
            .strip_prefix("url(")
            .and_then(|s| s.strip_suffix(')'))
            .unwrap_or(spec)
            .trim();
        ensure!(
            spec.len() >= 2 && spec.starts_with(['\'', '"']) && spec.ends_with(&spec[..1]),
            "Expected a string literal"
        );
        let name = &spec[1..spec.len() - 1];
        ensure!(
            !name.contains('\\'),
            "Escaped CSS imports are not supported"
        );
        if name == "tailwindcss" {
            result.push_str("@import \"tailwindcss\";");
            rest = &rest[end + 1..];
            continue;
        }
        ensure!(
            !name.contains("://") && !name.starts_with("//"),
            "Remote CSS is not supported"
        );
        result.push_str(&css(
            &path.parent().unwrap().join(name).canonicalize()?,
            stack,
            files,
        )?);
        rest = &rest[end + 1..];
    }
    result.push_str(rest);
    stack.pop();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "release bundle size measurement; set APPS_BENCH_ENTRY"]
    fn development_bundle_size() {
        let entry = PathBuf::from(std::env::var_os("APPS_BENCH_ENTRY").unwrap());
        let files = compile(&entry, Realm::Main, None, Rc::default(), false).unwrap();
        println!(
            "development_bundle_bytes={}",
            files.iter().map(|(_, source)| source.len()).sum::<usize>()
        );
    }

    #[test]
    fn commonjs_star_exports_follow_linked_packages() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("packages/shared");
        fs::create_dir_all(&package).unwrap();
        fs::create_dir_all(dir.path().join("node_modules")).unwrap();
        fs::write(
            package.join("package.json"),
            r#"{"name":"shared","main":"index.cjs"}"#,
        )
        .unwrap();
        fs::write(
            package.join("index.cjs"),
            "module.exports=require('./nested.cjs')",
        )
        .unwrap();
        fs::write(
            package.join("nested.cjs"),
            "exports.named=42;exports.change=()=>exports.named++",
        )
        .unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&package, dir.path().join("node_modules/shared")).unwrap();
        #[cfg(windows)]
        {
            fs::create_dir_all(dir.path().join("node_modules/shared")).unwrap();
            for name in ["package.json", "index.cjs", "nested.cjs"] {
                fs::copy(
                    package.join(name),
                    dir.path().join("node_modules/shared").join(name),
                )
                .unwrap();
            }
        }
        fs::write(dir.path().join("barrel.js"), "export * from 'shared'").unwrap();
        fs::write(dir.path().join("outer.js"), "export * from './barrel.js'").unwrap();
        let entry = dir.path().join("app.js");
        fs::write(&entry, "import {named,change} from './outer.js';if(named!==42)throw Error('star');change();if(named!==43)throw Error('live star')").unwrap();
        for production in [false, true] {
            let files = compile(&entry, Realm::Main, None, Rc::default(), production).unwrap();
            let engine = apps_runtime::jsc::Engine::new(
                apps_runtime::jsc::Api::load().unwrap(),
                Rc::default(),
                apps_runtime::host::Role::Main,
            )
            .unwrap();
            engine.bundle(&files, "app.js").unwrap();
        }
    }

    #[test]
    fn export_conditions_keep_order_and_accept_arrays() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("node_modules/ordered");
        fs::create_dir_all(&package).unwrap();
        fs::write(
            package.join("package.json"),
            r#"{"exports":{"default":[{"unsupported":"./bad.js"},"./good.js"],"node":"./bad.js"}}"#,
        )
        .unwrap();
        fs::write(package.join("good.js"), "export default 42").unwrap();
        let entry = dir.path().join("app.js");
        fs::write(
            &entry,
            "import answer from 'ordered';if(answer!==42)throw Error('conditions')",
        )
        .unwrap();
        let files = compile(&entry, Realm::Main, None, Rc::default(), false).unwrap();
        let engine = apps_runtime::jsc::Engine::new(
            apps_runtime::jsc::Api::load().unwrap(),
            Rc::default(),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        engine.bundle(&files, "app.js").unwrap();
    }

    #[test]
    fn oxc_transforms_typescript_and_export_declarations() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("values.ts"),
            r#"
            export enum Mode { Ready = 40 }
            export namespace Values { export const offset: number = 2; }
            export class Counter { constructor(public value: number) {} }
            export const { first, second } = { first: 1, second: 2 }, third = 3;
            export default async function () { return Mode.Ready + Values.offset; }
            export function unused() { return 'UNUSED_TYPESCRIPT_EXPORT'; }
        "#,
        )
        .unwrap();
        fs::write(
            dir.path().join("barrel.js"),
            "export * as values from './values.ts'; export {default as answer} from './values.ts';",
        )
        .unwrap();
        let entry = dir.path().join("app.ts");
        fs::write(&entry, r#"
            import type {Missing} from './missing.ts';
            import {Mode, Values, Counter, first, second, third} from './values.ts';
            import {answer} from './barrel.js';
            const identity = <T,>(value: T): T => value;
            const regex = /import.*export/g;
            globalThis.result = [await answer(), new Counter(identity(Mode.Ready)).value + Values.offset, first + second + third, `${regex.test('import export')}`].join(':');
        "#).unwrap();
        for production in [false, true] {
            let files = compile(&entry, Realm::Main, None, Rc::default(), production).unwrap();
            let engine = apps_runtime::jsc::Engine::new(
                apps_runtime::jsc::Api::load().unwrap(),
                Rc::default(),
                apps_runtime::host::Role::Main,
            )
            .unwrap();
            engine.bundle(&files, "app.js").unwrap();
            assert_eq!(engine.eval("result", "test").unwrap(), "42:42:6:true");
            let output = files.values().cloned().collect::<String>();
            assert_eq!(output.contains("UNUSED_TYPESCRIPT_EXPORT"), !production);
            assert_eq!(output.contains("\"map\":"), !production);
            assert!(!output.contains("\"debug\":"));
        }
    }

    #[test]
    fn development_maps_runtime_locations_to_original_sources() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("node_modules/mapped");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("package.json"), r#"{"main":"index.cjs"}"#).unwrap();
        fs::write(
            package.join("index.cjs"),
            "exports.probe = function cjsProbe() {\n  throw new Error('cjs mapped');\n};\n",
        )
        .unwrap();
        let entry = dir.path().join("app.ts");
        let source = "import mapped from 'mapped';\ninterface Value { count: number }\nfunction esmProbe(value: Value) {\n  const text: string = '😀'; throw new Error(text);\n}\nglobalThis.esmProbe = esmProbe;\nglobalThis.cjsProbe = mapped.probe;\n";
        fs::write(&entry, source).unwrap();
        let files = compile(&entry, Realm::Main, None, Rc::default(), false).unwrap();
        let host = Rc::default();
        let engine = apps_runtime::jsc::Engine::new(
            apps_runtime::jsc::Api::load().unwrap(),
            Rc::clone(&host),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        engine.bundle(&files, "app.js").unwrap();
        let definitions: Vec<Value> = files
            .iter()
            .filter(|(name, _)| name.ends_with(".js"))
            .flat_map(|(_, code)| code.lines())
            .map(|line| {
                serde_json::from_str(
                    line.strip_prefix("define(")
                        .unwrap()
                        .strip_suffix(");")
                        .unwrap(),
                )
                .unwrap()
            })
            .collect();
        for (name, original_line) in [("esmProbe", 3), ("cjsProbe", 1)] {
            let stack = engine
                .eval(
                    &format!("(()=>{{try{{{name}()}}catch(error){{return error.stack}}}})()"),
                    "apps:test",
                )
                .unwrap();
            let frame = stack.lines().next().unwrap();
            let mut location = frame.rsplitn(3, ':');
            let column: u32 = location
                .next()
                .unwrap()
                .parse()
                .unwrap_or_else(|_| panic!("Invalid stack: {stack}"));
            let line: u32 = location.next().unwrap().parse().unwrap();
            let url = location.next().unwrap().split_once('@').unwrap().1;
            let definition = definitions
                .iter()
                .find(|definition| definition["url"] == url)
                .unwrap_or_else(|| panic!("Missing source for {frame}"));
            let json = definition["map"].to_string();
            let map = oxc_sourcemap::SourceMap::from_json_string(&json).unwrap();
            let lookup = map.generate_lookup_table();
            let token = map.lookup_token(&lookup, line - 1, column - 1).unwrap();
            assert_eq!(token.get_src_line(), original_line, "{frame}");
            let content = map
                .get_source_content(token.get_source_id().unwrap())
                .unwrap();
            let original = content.lines().nth(original_line as usize).unwrap();
            let prefix = original.split("Error").next().unwrap();
            assert_eq!(
                token.get_src_col(),
                prefix.encode_utf16().count() as u32,
                "{frame}"
            );
        }
        let sources =
            apps_runtime::devtools::dispatch(&json!({"method":"Sources.list"}), &engine, &host, 0)
                .unwrap();
        assert!(
            sources
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["source"] == source)
        );
    }

    #[test]
    fn html_and_css_images_become_bundled_assets() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("main.ts");
        fs::write(
            &entry,
            "console.log(import.meta.resolve('./web/index.html'));",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("web/css")).unwrap();
        fs::create_dir_all(dir.path().join("img")).unwrap();
        fs::write(dir.path().join("img/logo.png"), b"png").unwrap();
        fs::write(dir.path().join("img/icon.svg"), b"<svg/>").unwrap();
        fs::write(
            dir.path().join("web/css/style.css"),
            ".a { background: url(\"../../img/icon.svg\") }",
        )
        .unwrap();
        fs::write(
            dir.path().join("web/index.html"),
            r#"<link rel="stylesheet" href="css/style.css"><style>.b{background:url(../img/logo.png)}.c{background:url(data:image/png;base64,AA==)}</style><img src="../img/logo.png"><img src="https://example.com/x.png"><div style="background-image:url('../img/missing.png')"></div>"#,
        )
        .unwrap();
        let files = compile(&entry, Realm::Main, None, Rc::default(), true).unwrap();
        let assets: std::collections::BTreeMap<String, PathBuf> =
            serde_json::from_str(&files["assets.json"]).unwrap();
        assert_eq!(assets.len(), 2);
        let logo = asset_name(&dir.path().join("img/logo.png").canonicalize().unwrap()).unwrap();
        let icon = asset_name(&dir.path().join("img/icon.svg").canonicalize().unwrap()).unwrap();
        assert!(logo.starts_with("assets/") && logo.ends_with(".png"));
        assert_eq!(
            assets[&icon],
            dir.path().join("img/icon.svg").canonicalize().unwrap()
        );
        let page = &files["page-0.html"];
        assert_eq!(page.matches(&format!("apps:///{logo}")).count(), 2);
        assert!(page.contains(&format!("url(\"apps:///{icon}\")")));
        assert!(
            page.contains("https://example.com/x.png")
                && page.contains("data:image/png;base64,AA==")
        );
        assert!(page.contains("../img/missing.png"));

        let out = dir.path().join("out");
        fs::create_dir_all(&out).unwrap();
        apps_runtime::bundle::write(&out.join("app.h2o"), &files).unwrap();
        assert_eq!(fs::read(out.join(&logo)).unwrap(), b"png");
        let bundle = apps_runtime::bundle::read(&out).unwrap();
        let resolved: std::collections::BTreeMap<String, PathBuf> =
            serde_json::from_str(&bundle.files["assets.json"]).unwrap();
        assert_eq!(resolved[&logo], out.canonicalize().unwrap().join(&logo));
    }

    #[test]
    fn declarative_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("app.ts");
        fs::write(&entry, "export { default } from './config';").unwrap();
        fs::write(
            dir.path().join("dependency.ts"),
            "globalThis.calls = []; export default { ready: false, fetch: false };",
        )
        .unwrap();
        for production in [false, true] {
            for startup in [
                "",
                "await app.whenReady(); await new Promise(resolve => setTimeout(resolve, 5));",
            ] {
                fs::write(dir.path().join("config.ts"), format!(r#"
                    import {{ app, BrowserWindow }} from '@pierre/apps';
                    import './dependency';
                    if (app.hasWindows !== false || app.platform !== process.platform) throw Error('initial app');
                    app.on('ready', () => calls.push('event'));
                    {startup}
                    export default {{
                        async ready(instance) {{
                            if (instance !== app || !app.isReady()) throw Error('ready app');
                            new BrowserWindow({{ width: 800, height: 600 }}).loadFile('index.html');
                            if (app.hasWindows !== true) throw Error('created window');
                            await new Promise(resolve => setTimeout(resolve, 5));
                            calls.push('ready');
                        }},
                        activate(instance) {{
                            if (instance !== app) throw Error('activate app');
                            if (!app.hasWindows) new BrowserWindow();
                            calls.push('activate');
                        }},
                        windowClose(instance) {{
                            if (instance !== app) throw Error('closed app');
                            calls.push('closed:' + app.hasWindows);
                            if (!app.hasWindows) instance.quit();
                        }},
                    }};
                "#)).unwrap();
                let files = compile(&entry, Realm::Main, None, Rc::default(), production).unwrap();
                let host = Rc::new(RefCell::new(apps_runtime::host::Host::default()));
                let engine = apps_runtime::jsc::Engine::new(
                    apps_runtime::jsc::Api::load().unwrap(),
                    host.clone(),
                    apps_runtime::host::Role::Main,
                )
                .unwrap();
                engine.bundle(&files, "app.js").unwrap();
                engine.eval("__appsEvent('ready')", "test").unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while engine.eval("__appsPending()", "test").unwrap() == "true" {
                    engine.tick().unwrap();
                    assert!(
                        std::time::Instant::now() < deadline,
                        "Lifecycle startup timed out"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                assert_eq!(engine.eval("calls.join()", "test").unwrap(), "event,ready");
                engine.eval("__appsEvent('activate');new __apps.BrowserWindow();__appsEvent('closed',1);__appsEvent('closed',2);__appsEvent('closed',2);__appsEvent('activate');__appsEvent('closed',3);__appsEvent('ready')", "test").unwrap();
                assert_eq!(
                    engine.eval("calls.join()", "test").unwrap(),
                    "event,ready,activate,closed:true,closed:false,activate,closed:false,event"
                );
                let host = host.borrow();
                assert_eq!(
                    host.commands
                        .iter()
                        .filter(|command| command["op"] == "quit")
                        .count(),
                    2
                );
                assert!(
                    host.commands
                        .iter()
                        .any(|command| command["args"] == json!(["loadFile", 1, "index.html"]))
                );
            }
        }
    }

    #[test]
    fn declarative_fetch() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("app.ts");
        for production in [false, true] {
            let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = socket.local_addr().unwrap().port();
            fs::write(&entry, format!(r#"
                export default {{
                    port: {port},
                    hostname: '127.0.0.1',
                    async ready(app) {{
                        const response = await fetch('http://127.0.0.1:{port}/hello', {{ method: 'POST', body: 'world' }});
                        if (response.status !== 201 || await response.text() !== '/hello:world') throw Error('declared fetch');
                        app.quit();
                    }},
                    async fetch(req) {{
                        return new Response(new URL(req.url).pathname + ':' + await req.text(), {{ status: 201 }});
                    }},
                }};
            "#)).unwrap();
            let files = compile(&entry, Realm::Main, None, Rc::default(), production).unwrap();
            drop(socket);
            assert_eq!(
                apps_runtime::desktop::Desktop::new(&files)
                    .unwrap()
                    .check()
                    .unwrap(),
                0
            );
        }
    }

    #[test]
    fn declarative_errors() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("app.ts");
        for production in [false, true] {
            for (source, message) in [
                (
                    "export default { ready: true }",
                    "Expected a function for ready",
                ),
                (
                    "export default { activate: 1 }",
                    "Expected a function for activate",
                ),
                (
                    "export default { windowClose: null }",
                    "Expected a function for windowClose",
                ),
                (
                    "export default { fetch: false }",
                    "Expected a function for fetch",
                ),
                ("export default { fetch() {}, port: -1 }", "Invalid port"),
                (
                    "export default { async ready() { await new Promise(resolve => setTimeout(resolve, 5)); throw Error('ready failed'); } }",
                    "ready failed",
                ),
            ] {
                fs::write(&entry, source).unwrap();
                let files = compile(&entry, Realm::Main, None, Rc::default(), production).unwrap();
                let error = match apps_runtime::desktop::Desktop::new(&files) {
                    Ok(desktop) => desktop.check().unwrap_err(),
                    Err(error) => error,
                };
                assert!(format!("{error:#}").contains(message), "{error:#}");
            }
        }
    }

    #[test]
    fn declarative_handlers_are_main_only() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("app.ts");
        fs::write(&entry, "export default { ready: false, fetch: false };").unwrap();
        for production in [false, true] {
            for (realm, role) in [
                (Realm::Preload, apps_runtime::host::Role::Preload(1)),
                (Realm::Renderer, apps_runtime::host::Role::Renderer(1)),
            ] {
                let files = compile(&entry, realm, None, Rc::default(), production).unwrap();
                let host = Rc::new(RefCell::new(apps_runtime::host::Host::default()));
                host.borrow_mut().pages.insert(
                    1,
                    apps_runtime::host::Page {
                        doc: Document::parse("<body></body>"),
                        dirty: false,
                    },
                );
                let engine = apps_runtime::jsc::Engine::new(
                    apps_runtime::jsc::Api::load().unwrap(),
                    host,
                    role,
                )
                .unwrap();
                engine.bundle(&files, "app.js").unwrap();
                assert_eq!(
                    engine.eval("typeof __appsStart", "test").unwrap(),
                    "undefined"
                );
            }
        }
    }

    #[test]
    fn builtin_web_contents_view_exports() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("app.ts");
        for module in ["@pierre/apps", "electron"] {
            fs::write(&entry, format!("import apps, {{ View, WebContentsView, BrowserWindow }} from '{module}'; if (apps.WebContentsView !== WebContentsView || !(WebContentsView.prototype instanceof View) || typeof WebContentsView.prototype.loadURL !== 'undefined' || typeof View.prototype.addChildView !== 'function') throw Error('WebContentsView API'); apps.app.whenReady().then(() => {{ const win = new BrowserWindow(); if (!(win.contentView instanceof View)) throw Error('contentView'); globalThis.checked = true; }});")).unwrap();
            for production in [false, true] {
                let files = compile(&entry, Realm::Main, None, Rc::default(), production).unwrap();
                let engine = apps_runtime::jsc::Engine::new(
                    apps_runtime::jsc::Api::load().unwrap(),
                    Rc::default(),
                    apps_runtime::host::Role::Main,
                )
                .unwrap();
                engine.bundle(&files, "view-test").unwrap();
                engine.eval("__appsEvent('ready')", "test").unwrap();
                engine.tick().unwrap();
                assert_eq!(engine.eval("checked", "test").unwrap(), "true");
            }
        }
    }

    #[test]
    fn builtin_serve_export() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("app.ts");
        fs::write(&entry, "import apps, { serve } from '@pierre/apps'; if (apps.serve !== serve) throw Error('serve export'); const server = serve({ fetch: (req: Request) => new Response('hello world', {}), port: 0 }); globalThis.port = server.port; server.stop();").unwrap();
        for production in [false, true] {
            let files = compile(&entry, Realm::Main, None, Rc::default(), production).unwrap();
            let engine = apps_runtime::jsc::Engine::new(
                apps_runtime::jsc::Api::load().unwrap(),
                Rc::default(),
                apps_runtime::host::Role::Main,
            )
            .unwrap();
            engine.bundle(&files, "serve-test").unwrap();
            assert!(
                engine
                    .eval("port", "serve-test")
                    .unwrap()
                    .parse::<u16>()
                    .unwrap()
                    > 0
            );
            assert_eq!(engine.tick().unwrap(), -1);
        }
    }

    #[test]
    fn production_minifies_and_shakes_modules() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("counter.ts"),
            r#"
            export let count: number = 1;
            export function next() { count++; }
            export function unused() { return 'UNUSED_EXPORT_MARKER'; }
            export const effect = globalThis.effects = (globalThis.effects ?? 0) + 1;
        "#,
        )
        .unwrap();
        fs::write(
            dir.path().join("barrel.js"),
            "export {count, next, unused} from './counter'; export * from './counter';",
        )
        .unwrap();
        fs::write(
            dir.path().join("namespace.js"),
            "export const first = 1; export const second = 2;",
        )
        .unwrap();
        fs::write(
            dir.path().join("a.js"),
            "import {b} from './b'; export function a() { return b() + 'UNUSED_CYCLE_MARKER'; }",
        )
        .unwrap();
        fs::write(
            dir.path().join("b.js"),
            "import {a} from './a'; export function b() { return a(); }",
        )
        .unwrap();
        fs::write(
            dir.path().join("eval.js"),
            "import {count} from './counter'; eval('globalThis.before = count');",
        )
        .unwrap();
        let entry = dir.path().join("app.ts");
        fs::write(&entry, r#"
            import {count, next} from './barrel';
            import * as values from './namespace';
            import './a';
            import './eval';
            // REMOVE_COMMENT_MARKER
            const unused: string = 'UNUSED_LOCAL_MARKER';
            if (false) throw Error('DEAD_BRANCH_MARKER');
            next();
            globalThis.result = [count, effects, before, Object.keys(values).sort().join(','), values.second].join(':');
        "#).unwrap();
        for production in [false, true] {
            let files = compile(&entry, Realm::Main, None, Rc::default(), production).unwrap();
            let output = files.values().cloned().collect::<String>();
            for marker in [
                "UNUSED_EXPORT_MARKER",
                "UNUSED_CYCLE_MARKER",
                "REMOVE_COMMENT_MARKER",
                "UNUSED_LOCAL_MARKER",
                "DEAD_BRANCH_MARKER",
            ] {
                assert_eq!(output.contains(marker), !production, "{marker}");
            }
            let engine = apps_runtime::jsc::Engine::new(
                apps_runtime::jsc::Api::load().unwrap(),
                Rc::default(),
                apps_runtime::host::Role::Main,
            )
            .unwrap();
            engine.bundle(&files, "test").unwrap();
            assert_eq!(
                engine.eval("result", "test").unwrap(),
                "2:1:1:first,second:2"
            );
        }
        fs::write(&entry, "import {missing} from './counter';").unwrap();
        assert!(
            compile(&entry, Realm::Main, None, Rc::default(), true)
                .unwrap_err()
                .chain()
                .any(|error| error.to_string().contains("Missing export"))
        );
    }

    #[test]
    fn production_optimizes_html_scripts() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("page.ts"), "const unused: string = 'UNUSED_PAGE_MARKER'; await Promise.resolve(); document.querySelector('h1').textContent = '</script>';").unwrap();
        fs::write(dir.path().join("index.html"), "<html><head><script type='preload'>const unused = 'UNUSED_PRELOAD_MARKER'; if (!process.versions.apps) throw Error('preload');</script></head><body><h1>Hello</h1><script type='module' src='./page.ts'></script><script type='module'>const unused = 'UNUSED_INLINE_MARKER'; if (!document.querySelector('h1')) throw Error('page');</script></body></html>").unwrap();
        let entry = dir.path().join("app.ts");
        fs::write(&entry, "import {app, BrowserWindow} from '@pierre/apps'; app.whenReady().then(() => new BrowserWindow().loadFile(import.meta.resolve('./index.html')));").unwrap();
        let files = compile(&entry, Realm::Main, None, Rc::default(), true).unwrap();
        let output = files.values().cloned().collect::<String>();
        for marker in [
            "UNUSED_PAGE_MARKER",
            "UNUSED_PRELOAD_MARKER",
            "UNUSED_INLINE_MARKER",
        ] {
            assert!(!output.contains(marker), "{marker}");
        }
        assert_eq!(
            apps_runtime::desktop::Desktop::new(&files)
                .unwrap()
                .check()
                .unwrap(),
            0
        );
    }

    #[test]
    fn preload_bridge_survives_minification() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("app.ts");
        fs::write(&entry,"import {app,BrowserWindow,ipcMain} from 'electron';ipcMain.handle('sum',(_event,a,b)=>a+b);ipcMain.handle('echo',(event,value)=>event.sender.send('echo',value));app.whenReady().then(()=>new BrowserWindow().loadFile(import.meta.resolve('./index.html')));").unwrap();
        fs::write(dir.path().join("index.html"),r#"<html><head><script type="preload" src="./preload.ts"></script></head><body><p id="result">waiting</p><script type="module" src="./page.ts"></script></body></html>"#).unwrap();
        fs::write(dir.path().join("preload.ts"),"import {contextBridge,ipcRenderer} from 'electron';contextBridge.exposeInMainWorld('bridge',{sum:(a,b)=>ipcRenderer.invoke('sum',a,b),echo:value=>ipcRenderer.invoke('echo',value),subscribe:callback=>{const listener=(_event,value)=>callback(value);ipcRenderer.on('echo',listener);return()=>{ipcRenderer.off('echo',listener)}}});").unwrap();
        fs::write(dir.path().join("page.ts"),"const values=[];const off=bridge.subscribe(value=>values.push(value));if(typeof off!=='function')throw Error('cleanup');if(await bridge.sum(20,22)!==42)throw Error('sum');await bridge.echo('first');off();await bridge.echo('second');if(values.join()!=='first')throw Error('events');document.getElementById('result').textContent='ok';").unwrap();
        for production in [false, true] {
            let files = compile(&entry, Realm::Main, None, Rc::default(), production).unwrap();
            assert_eq!(
                apps_runtime::desktop::Desktop::new(&files)
                    .unwrap()
                    .check()
                    .unwrap(),
                0
            );
        }
    }

    #[test]
    fn jsx_tsx_and_commonjs_packages() {
        for production in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let package = dir.path().join("node_modules/react");
            fs::create_dir_all(&package).unwrap();
            fs::write(package.join("package.json"), r#"{"main":"index.js"}"#).unwrap();
            fs::write(
                package.join("index.js"),
                "module.exports = require('./runtime.cjs');",
            )
            .unwrap();
            fs::write(package.join("runtime.cjs"), "exports.Fragment='fragment';exports.createElement=(type,props,...children)=>({type,props,children});exports.mode=process.env.NODE_ENV;").unwrap();
            for name in ["jsx-runtime.js", "jsx-dev-runtime.js"] {
                fs::write(package.join(name), "const react=require('./runtime.cjs');exports.Fragment=react.Fragment;exports.jsx=exports.jsxs=exports.jsxDEV=(type,props)=>react.createElement(type,props,...(props.children===undefined?[]:Array.isArray(props.children)?props.children:[props.children]));").unwrap();
            }
            fs::write(
                dir.path().join("label.jsx"),
                "export default function Label(props) { return <span>Hi {props.name}</span>; }",
            )
            .unwrap();
            let entry = dir.path().join("page.tsx");
            fs::write(&entry, "import Label from './label'; import {mode} from 'react'; interface Props {name: string} const props: Props = {name: 'Ada'}; globalThis.tree = <><Label {...props}/><input disabled data-id=\"1\" /></>; globalThis.result = tree.children[0].type(tree.children[0].props).children.join('') + ':' + mode;").unwrap();
            let watched = Rc::default();
            let files = compile(&entry, Realm::Renderer, None, watched, production).unwrap();
            let engine = apps_runtime::jsc::Engine::new(
                apps_runtime::jsc::Api::load().unwrap(),
                Rc::default(),
                apps_runtime::host::Role::Main,
            )
            .unwrap();
            engine.bundle(&files, "test").unwrap();
            assert_eq!(
                engine.eval("result", "test").unwrap(),
                if production {
                    "Hi Ada:production"
                } else {
                    "Hi Ada:development"
                }
            );
            assert_eq!(
                engine
                    .eval("tree.type + ':' + tree.children[1].props.disabled", "test")
                    .unwrap(),
                "fragment:true"
            );
            fs::write(package.join("index.js"), "module.exports = require(name);").unwrap();
            assert!(
                compile(&entry, Realm::Renderer, None, Rc::default(), production)
                    .unwrap_err()
                    .to_string()
                    .contains("string literal")
            );
            fs::write(
                package.join("index.js"),
                "module.exports = require('node:fs');",
            )
            .unwrap();
            assert!(compile(&entry, Realm::Renderer, None, Rc::default(), production).is_err());
        }
    }

    #[test]
    fn commonjs_cycles_and_module_exports() {
        for production in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let package = dir.path().join("node_modules/counter");
            fs::create_dir_all(&package).unwrap();
            fs::write(package.join("package.json"), r#"{"main":"index.js"}"#).unwrap();
            fs::write(package.join("index.js"), "exports.count=1;const peer=require('./peer');exports.seen=peer.seen;exports.next=()=>exports.count++;exports.load=()=>import('./peer');if(false) require('./unused');").unwrap();
            fs::write(
                package.join("peer.js"),
                "exports.seen=require('./index').count;",
            )
            .unwrap();
            fs::write(
                package.join("unused.js"),
                "module.exports=0;throw Error('Unused require ran');",
            )
            .unwrap();
            let entry = dir.path().join("app.js");
            fs::write(&entry, "import counter,{count,seen,next} from 'counter';next();globalThis.result = [count, seen, counter.count, (await counter.load()).seen].join(':');").unwrap();
            let files = compile(&entry, Realm::Main, None, Rc::default(), production).unwrap();
            let engine = apps_runtime::jsc::Engine::new(
                apps_runtime::jsc::Api::load().unwrap(),
                Rc::default(),
                apps_runtime::host::Role::Main,
            )
            .unwrap();
            engine.bundle(&files, "test").unwrap();
            assert_eq!(engine.eval("result", "test").unwrap(), "2:1:2:1");
        }
    }

    #[test]
    #[ignore = "Run npm ci in examples/react first"]
    fn react_counter() {
        for production in [false, true] {
            use apps_runtime::{
                host::{Host, Page, Role},
                jsc::{Api, Engine},
            };
            let host = Rc::new(RefCell::new(Host::default()));
            host.borrow_mut().pages.insert(
                1,
                Page {
                    doc: Document::parse("<html><body><div id='root'></div></body></html>"),
                    dirty: false,
                },
            );
            let engine =
                Engine::new(Api::load().unwrap(), host.clone(), Role::Renderer(1)).unwrap();
            let entry = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/react/page.tsx")
                .canonicalize()
                .unwrap();
            let files = compile(&entry, Realm::Renderer, None, Rc::default(), production).unwrap();
            engine.bundle(&files, "react-example").unwrap();
            assert_eq!(
                engine
                    .eval("document.getElementById('count').textContent", "test")
                    .unwrap(),
                "Count: 0"
            );
            for (button, count) in [
                ("increment", 1),
                ("increment", 2),
                ("reset", 0),
                ("increment", 1),
            ] {
                engine.eval(
                &format!("{{ const ids = []; for (let node = document.getElementById('{button}'); node; node = node.parentNode) ids.push(node._id); __appsDispatch('click', ids); }}"),
                "test",
            ).unwrap();
                engine.eval("__appsTick()", "test").unwrap();
                assert_eq!(
                    engine
                        .eval("document.getElementById('count').textContent", "test")
                        .unwrap(),
                    format!("Count: {count}")
                );
                assert_eq!(
                    engine
                        .eval("document.getElementById('message') !== null", "test")
                        .unwrap(),
                    (count > 0).to_string()
                );
            }
            assert_eq!(engine.eval("typeof process", "test").unwrap(), "undefined");
            assert!(host.borrow().pages[&1].dirty);
        }
    }

    #[test]
    #[ignore = "Requires a native GPU and npm install --prefix examples/three"]
    fn three_webgpu_renders_a_cube() {
        for production in [false, true] {
            use apps_runtime::{
                host::{Host, Page, Role},
                jsc::{Api, Engine},
            };
            let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/three")
                .canonicalize()
                .unwrap();
            let files = compile(
                &root.join("page.js"),
                Realm::Renderer,
                None,
                Rc::default(),
                production,
            )
            .unwrap();
            let host = Rc::new(RefCell::new(Host::default()));
            host.borrow_mut().pages.insert(
                1,
                Page {
                    doc: Document::parse("<html><body></body></html>"),
                    dirty: false,
                },
            );
            host.borrow_mut().viewports.insert(1, [96., 96.]);
            let engine =
                Engine::new(Api::load().unwrap(), host.clone(), Role::Renderer(1)).unwrap();
            engine.bundle(&files, "three-test").unwrap();
            for _ in 0..1000 {
                if engine
                    .eval("globalThis.threeTestResult ?? 'pending'", "three-test")
                    .unwrap()
                    != "pending"
                {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert_eq!(
                engine
                    .eval("globalThis.threeTestResult ?? 'pending'", "three-test")
                    .unwrap(),
                "ok"
            );
            assert_eq!(
                engine
                    .eval("threeRenderer.backend.isWebGPUBackend", "three-test")
                    .unwrap(),
                "true"
            );
            let mut state = host.borrow_mut();
            let node = state.pages[&1]
                .doc
                .nodes
                .iter()
                .position(|n| n.tag == "canvas")
                .unwrap();
            let image = state
                .graphics
                .get_mut(&1)
                .unwrap()
                .image(node)
                .unwrap()
                .unwrap();
            let pixels = image.as_bytes(0).unwrap();
            assert_eq!(pixels.len(), 96 * 96 * 4);
            let center = &pixels[(48 * 96 + 48) * 4..(48 * 96 + 48) * 4 + 4];
            assert_ne!(center, &pixels[..4], "Cube was not drawn");
            assert!(
                center[..3].iter().any(|&channel| channel > 100),
                "Cube is too dark"
            );
            drop(state);
            std::thread::sleep(std::time::Duration::from_millis(20));
            engine.eval("__appsTick()", "three-animation").unwrap();
            let frame = host
                .borrow_mut()
                .graphics
                .get_mut(&1)
                .unwrap()
                .image(node)
                .unwrap()
                .unwrap();
            assert_ne!(
                pixels,
                frame.as_bytes(0).unwrap(),
                "Animation did not update the canvas"
            );
            engine
                .eval("threeRenderer.dispose()", "three-test")
                .unwrap();
        }
    }

    #[test]
    fn live_imports_typescript_and_dependencies() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("node_modules/counter")).unwrap();
        fs::write(
            dir.path().join("node_modules/counter/package.json"),
            r#"{"type":"module","exports":"./index.js"}"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("node_modules/counter/index.js"),
            "export let count=1;export function next(){count++}",
        )
        .unwrap();
        let entry = dir.path().join("app.ts");
        fs::write(&entry,"import {count,next} from 'counter'; const x: number = 2; next(); if(count !== x) throw Error('stale import'); globalThis.result=count;").unwrap();
        let files = compile(&entry, Realm::Main, None, Rc::default(), false).unwrap();
        assert!(files.contains_key("deps.js"));
        let engine = apps_runtime::jsc::Engine::new(
            apps_runtime::jsc::Api::load().unwrap(),
            std::rc::Rc::new(std::cell::RefCell::new(apps_runtime::host::Host::default())),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        engine.bundle(&files, "app.js").unwrap();
        assert_eq!(engine.eval("result", "test").unwrap(), "2");
    }
    #[test]
    fn rejects_commonjs_and_privileged_page_imports() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("app.ts");
        for source in [
            "const fs=require('fs')",
            "import fs from 'node:fs'",
            "import { DatabaseSync } from 'node:sqlite'",
            "import { spawn } from 'node:child_process'",
            "import { once } from 'node:events'",
            "import { DynamicLibrary } from 'node:ffi'",
        ] {
            fs::write(&entry, source).unwrap();
            assert!(compile(&entry, Realm::Renderer, None, Rc::default(), false).is_err());
        }
    }
    #[test]
    fn cycles_reexports_and_missing_exports() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.js"),"import { b } from './b.js'; export function a(){return 'a'} export const result = b();").unwrap();
        fs::write(
            dir.path().join("b.js"),
            "import { a } from './a.js'; export function b(){return a()+'b'}",
        )
        .unwrap();
        fs::write(dir.path().join("barrel.js"), "export * from './a.js'").unwrap();
        let entry = dir.path().join("app.ts");
        fs::write(
            &entry,
            "import {result as value} from './barrel.js'; globalThis.answer=value;",
        )
        .unwrap();
        let engine = apps_runtime::jsc::Engine::new(
            apps_runtime::jsc::Api::load().unwrap(),
            std::rc::Rc::new(std::cell::RefCell::new(apps_runtime::host::Host::default())),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        let files = compile(&entry, Realm::Main, None, Rc::default(), false).unwrap();
        engine.validate(&files).unwrap();
        engine.bundle(&files, "app.js").unwrap();
        assert_eq!(engine.eval("answer", "test").unwrap(), "ab");
        fs::write(&entry, "import {missing} from './a.js'").unwrap();
        let files = compile(&entry, Realm::Main, None, Rc::default(), false).unwrap();
        assert!(
            engine
                .validate(&files)
                .unwrap_err()
                .to_string()
                .contains("Missing export")
        );
    }

    #[test]
    fn tailwind_import_survives_nested_css_without_a_package() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("base.css"),
            "@import 'tailwindcss'; @theme { --color-mint-500: oklch(0.72 0.11 178); }",
        )
        .unwrap();
        fs::write(
            dir.path().join("style.css"),
            "@import './base.css'; div { color: #123456; @apply rounded-b-lg shadow-md; }",
        )
        .unwrap();
        let path = dir.path().join("index.html");
        fs::write(
            &path,
            "<link rel='stylesheet' href='./style.css'><div class='h-4 w-4 flex bg-mint-500'></div>",
        )
        .unwrap();
        let files = Rc::default();
        let source = html(&path, Rc::clone(&files), false).unwrap();
        assert!(source.contains("@import \"tailwindcss\";"));
        assert!(!source.contains(".h-4"));
        assert_eq!(files.borrow().len(), 2);
        let doc = Document::parse(&source);
        let style = apps_runtime::render::styles(&doc, 800., 600.)
            .pop()
            .unwrap();
        assert_eq!(style["height"], "1rem");
        assert_eq!(style["width"], "1rem");
        assert_eq!(style["display"], "flex");
        assert_eq!(style["color"], "#123456");
        assert_eq!(style["background-color"], "oklch(0.72 0.11 178)");
        assert_eq!(style["border-bottom-left-radius"], "8px");
        assert!(style.contains_key("box-shadow"));
    }

    #[test]
    fn html_bundles_css_scripts_and_preload() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("base.css"), "h1 { color: #123456 }").unwrap();
        fs::write(
            dir.path().join("style.css"),
            "@import './base.css'; body { margin: 0 }",
        )
        .unwrap();
        fs::write(dir.path().join("page.ts"),r#"const end: string = "</script>"; if (end !== '<' + '/script>') throw Error('escape'); document.querySelector('h1').textContent = end; if(typeof process !== 'undefined') throw Error('privilege leak');"#).unwrap();
        fs::write(dir.path().join("index.html"),"<html><head><link rel='stylesheet' href='./style.css'><script type='preload'>if(!process.versions.apps) throw Error('preload')</script></head><body><h1>Hello</h1><script type='module' src='./page.ts'></script></body></html>").unwrap();
        let entry = dir.path().join("app.ts");
        fs::write(&entry,"import {app,BrowserWindow} from '@pierre/apps';import html from './index.html' with {type:'text'};if(!html.includes('#123456')) throw Error('css');app.whenReady().then(()=>new BrowserWindow().load(html));").unwrap();
        let files = compile(&entry, Realm::Main, None, Rc::default(), false).unwrap();
        assert_eq!(
            apps_runtime::desktop::Desktop::new(&files)
                .unwrap()
                .check()
                .unwrap(),
            0
        );
    }

    #[test]
    fn resolved_html_survives_relocation() {
        for production in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let nested = dir.path().join("nested");
            fs::create_dir(&nested).unwrap();
            fs::write(nested.join("style.css"), "h1 { color: #123456 }").unwrap();
            fs::write(nested.join("page.ts"), "if (document.querySelector('h1').textContent !== 'Hello') throw Error('page'); document.querySelector('h1').textContent = 'Loaded';").unwrap();
            fs::write(nested.join("index.html"), "<link rel='stylesheet' href='./style.css'><script type='preload'>if (!process.versions.apps) throw Error('preload');</script><h1>Hello</h1><script type='module' src='./page.ts'></script>").unwrap();
            fs::write(
                nested.join("window.ts"),
                "export const page = import.meta.resolve('./index.html');",
            )
            .unwrap();
            let entry = dir.path().join("app.ts");
            fs::write(&entry, "import {app, BrowserWindow} from '@pierre/apps'; import {page} from './nested/window'; if (page !== import.meta.resolve('./nested/index.html')) throw Error('resolve'); app.whenReady().then(() => new BrowserWindow().loadFile(page));").unwrap();
            let watched = Rc::default();
            let files =
                compile(&entry, Realm::Main, None, Rc::clone(&watched), production).unwrap();
            assert_eq!(watched.borrow().len(), 5);
            assert_eq!(
                files.keys().filter(|name| name.ends_with(".html")).count(),
                1
            );
            assert!(files["page-0.html"].contains("#123456"));
            let output = tempfile::tempdir().unwrap();
            if production {
                apps_runtime::bundle::write(&output.path().join("app.h2o"), &files).unwrap();
            } else {
                for (name, source) in &files {
                    fs::write(output.path().join(name), source).unwrap();
                }
            }
            dir.close().unwrap();
            let bundle = apps_runtime::bundle::read(output.path()).unwrap();
            let engine = apps_runtime::jsc::Engine::new(
                apps_runtime::jsc::Api::load().unwrap(),
                Rc::default(),
                apps_runtime::host::Role::Main,
            )
            .unwrap();
            engine.validate(&bundle.files).unwrap();
            assert_eq!(
                apps_runtime::desktop::Desktop::from_bundle(bundle)
                    .unwrap()
                    .check()
                    .unwrap(),
                0
            );
        }
    }

    #[test]
    fn nearest_package_version_reaches_dev_and_production_apps() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("src");
        fs::create_dir(&source).unwrap();
        let entry = source.join("app.ts");
        let package = dir.path().join("package.json");
        fs::write(&package, r#"{"name":"package_name","productName":"My_App","version":"1.2.3","scripts":{"dev":"private command"}}"#).unwrap();
        let watched = Rc::default();
        for (local_package, version) in [
            (None, "1.2.3"),
            (Some("{}"), "0.0.0"),
            (Some(r#"{"version":"2.0.0"}"#), "2.0.0"),
            (Some(r#"{"version":"2.0.0-beta_1"}"#), "2.0.0-beta_1"),
        ] {
            if let Some(package) = local_package {
                fs::write(source.join("package.json"), package).unwrap();
            }
            for production in [false, true] {
                fs::write(&entry, format!("import {{app}} from '@pierre/apps'; if(app.getVersion() !== '{version}') throw Error('App version'); if(process.versions.apps !== '{}') throw Error('Runtime version'); if(app.isPackaged !== {production}) throw Error('Packaged status');", apps_runtime::VERSION)).unwrap();
                let files =
                    compile(&entry, Realm::Main, None, Rc::clone(&watched), production).unwrap();
                let metadata: Value = serde_json::from_str(&files["package.json"]).unwrap();
                assert!(metadata.get("scripts").is_none());
                assert_eq!(
                    metadata["name"],
                    if local_package.is_none() {
                        "package_name"
                    } else {
                        "App"
                    }
                );
                if !production {
                    assert_eq!(
                        apps_runtime::desktop::Desktop::new(&files)
                            .unwrap()
                            .check()
                            .unwrap(),
                        0
                    );
                } else {
                    let output = tempfile::tempdir().unwrap();
                    apps_runtime::bundle::write(&output.path().join("app.h2o"), &files).unwrap();
                    let bundle = apps_runtime::bundle::read(output.path()).unwrap();
                    assert_eq!(
                        apps_runtime::desktop::Desktop::from_bundle(bundle)
                            .unwrap()
                            .check()
                            .unwrap(),
                        0
                    );
                }
            }
        }
        assert!(watched.borrow().contains(&package));
        assert!(watched.borrow().contains(&source.join("package.json")));
    }

    #[test]
    fn rejects_unsupported_import_meta_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("app.js");
        fs::write(dir.path().join("index.html"), "<h1>Hello</h1>").unwrap();
        for (source, realm, message) in [
            (
                "import.meta.resolve(path)",
                Realm::Main,
                "one string literal",
            ),
            (
                "import.meta.resolve('./app.js')",
                Realm::Main,
                "only supports HTML",
            ),
            (
                "import.meta.resolve('https://example.com/index.html')",
                Realm::Main,
                "local HTML path",
            ),
            (
                "import.meta.resolve('./missing.html')",
                Realm::Main,
                "Cannot resolve",
            ),
            (
                "import.meta.resolve('./index.html')",
                Realm::Renderer,
                "only be resolved in main",
            ),
            (
                "import.meta.resolve('./index.html')",
                Realm::Preload,
                "only be resolved in main",
            ),
        ] {
            let error =
                compile(&entry, realm, Some(source.into()), Rc::default(), false).unwrap_err();
            assert!(error.to_string().contains(message), "{error}");
        }
    }

    #[test]
    fn import_meta_url_in_modules_and_templates() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let nested = root.join("nested # % café");
        fs::create_dir(&nested).unwrap();
        let dependency = nested.join("module.js");
        fs::write(&dependency, "export const url = import.meta.url;").unwrap();
        let entry = root.join("app.ts");
        fs::write(&entry, r#"
            import { url } from './nested # % café/module.js';
            const own: string = import /* comment */ . meta . url;
            const text = `URL: ${import.meta.url}; nested: ${`${import.meta.url}`}`;
            const literal = `import.meta.url`;
            globalThis.result = JSON.stringify([own, url, text, literal, (import.meta).url, new URL('./asset.txt', import.meta.url).href]);
        "#).unwrap();
        for production in [false, true] {
            for (realm, role) in [
                (Realm::Main, apps_runtime::host::Role::Main),
                (Realm::Preload, apps_runtime::host::Role::Preload(1)),
                (Realm::Renderer, apps_runtime::host::Role::Renderer(1)),
            ] {
                let files = compile(&entry, realm, None, Rc::default(), production).unwrap();
                let host = Rc::new(RefCell::new(apps_runtime::host::Host::default()));
                host.borrow_mut().pages.insert(
                    1,
                    apps_runtime::host::Page {
                        doc: Document::parse("<body></body>"),
                        dirty: false,
                    },
                );
                let engine = apps_runtime::jsc::Engine::new(
                    apps_runtime::jsc::Api::load().unwrap(),
                    host,
                    role,
                )
                .unwrap();
                engine.bundle(&files, "app.js").unwrap();
                let result: Value =
                    serde_json::from_str(&engine.eval("result", "test").unwrap()).unwrap();
                let base = url::Url::from_directory_path(&root).unwrap();
                let url = base.join("app.ts").unwrap().to_string();
                assert_eq!(
                    result,
                    json!([
                        url,
                        format!("{base}nested%20%23%20%25%20caf%C3%A9/module.js"),
                        format!("URL: {url}; nested: {url}"),
                        "import.meta.url",
                        url,
                        format!("{base}asset.txt"),
                    ]),
                    "production={production}, realm={realm:?}"
                );
            }
        }
    }

    #[test]
    #[cfg(unix)]
    fn import_meta_url_preserves_non_utf8_paths() {
        use std::os::unix::ffi::OsStrExt;

        let dir = tempfile::tempdir().unwrap();
        let entry = dir
            .path()
            .join(std::ffi::OsStr::from_bytes(b"module-\xff.js"));
        for production in [false, true] {
            let files = compile(
                &entry,
                Realm::Main,
                Some("globalThis.result = import.meta.url;".into()),
                Rc::default(),
                production,
            )
            .unwrap();
            let engine = apps_runtime::jsc::Engine::new(
                apps_runtime::jsc::Api::load().unwrap(),
                Rc::default(),
                apps_runtime::host::Role::Main,
            )
            .unwrap();
            engine.bundle(&files, "app.js").unwrap();
            assert_eq!(
                engine.eval("result", "test").unwrap(),
                format!(
                    "{}module-%FF.js",
                    url::Url::from_directory_path(dir.path()).unwrap()
                ),
            );
        }
    }

    #[test]
    fn import_meta_url_in_inline_html_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let page = root.join("index.html");
        let url = serde_json::to_string(url::Url::from_file_path(&page).unwrap().as_str()).unwrap();
        fs::write(
            &page,
            format!(
                r#"
            <script type="preload">
                const url: string = import.meta.url;
                if (url !== {url}) throw Error('preload URL');
            </script>
            <script type="module">
                if (`${{import.meta.url}}` !== {url}) throw Error('page URL');
            </script>
        "#
            ),
        )
        .unwrap();
        let entry = root.join("app.ts");
        fs::write(&entry, "import {app, BrowserWindow} from '@pierre/apps'; app.whenReady().then(() => new BrowserWindow().loadFile(import.meta.resolve('./index.html')));").unwrap();
        for production in [false, true] {
            let files = compile(&entry, Realm::Main, None, Rc::default(), production).unwrap();
            assert_eq!(
                apps_runtime::desktop::Desktop::new(&files)
                    .unwrap()
                    .check()
                    .unwrap(),
                0
            );
        }
    }

    #[test]
    fn bundled_workers_and_dynamic_imports_in_production() {
        for production in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let entry = dir.path().join("app.ts");
            let package = dir.path().join("node_modules/fixture");
            fs::create_dir_all(package.join("src")).unwrap();
            fs::write(
                package.join("package.json"),
                r#"{"type":"module","exports":{"./features/*":{"import":"./src/*.js"}}}"#,
            )
            .unwrap();
            fs::write(
                package.join("src/example.js"),
                "const instance={exports:{answer:42}};export const answer=instance.exports.answer;",
            )
            .unwrap();
            fs::write(
                dir.path().join("value.ts"),
                "export const value: number = 42;",
            )
            .unwrap();
            fs::write(dir.path().join("worker.ts"), "import {parentPort} from 'node:worker_threads'; import {value} from './value'; if (typeof __appsStart !== 'undefined') throw Error('worker app startup'); parentPort.postMessage(value); export default { ready: false, fetch: false };").unwrap();
            fs::write(&entry,"import {Worker} from 'node:worker_threads'; const feature=await import('fixture/features/example'); let matched;matched??=/Mac/.test('Mac');if(feature.answer!==42||!matched)throw Error('package or regexp'); const dynamic=await import('./value'); if(dynamic.value!==42||!import.meta.url.startsWith('file:'))throw Error('module '+JSON.stringify(dynamic)+' url '+import.meta.url); const worker=new Worker(new URL('./worker.ts',import.meta.url)); const value=await new Promise((resolve,reject)=>{worker.on('message',resolve);worker.on('error',reject)}); if(value!==42)throw Error('worker'); await worker.terminate(); globalThis.result='ok';").unwrap();
            let files = compile(&entry, Realm::Main, None, Rc::default(), production).unwrap();
            let engine = apps_runtime::jsc::Engine::new(
                apps_runtime::jsc::Api::load().unwrap(),
                Rc::default(),
                apps_runtime::host::Role::Main,
            )
            .unwrap();
            if let Err(error) = engine.bundle(&files, "app.js") {
                panic!("production={production}: {error:#}");
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while engine.eval("globalThis.result", "test").unwrap() != "ok" {
                engine.tick().unwrap();
                assert!(
                    std::time::Instant::now() < deadline,
                    "Worker bundle timed out (production={production})"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }

    #[test]
    fn node_file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("app.ts");
        let file = serde_json::to_string(&dir.path().join("data.txt")).unwrap();
        fs::write(&entry,format!("import fs from 'node:fs'; import path from 'node:path'; const file={file}; fs.writeFileSync(file,'hello'); if(fs.readFileSync(file,'utf8')!=='hello') throw Error('read'); if(!fs.statSync(file).isFile()) throw Error('stat'); if(path.basename(file)!=='data.txt') throw Error('path'); fs.unlinkSync(file); if(fs.existsSync(file)) throw Error('unlink');")).unwrap();
        let files = compile(&entry, Realm::Main, None, Rc::default(), false).unwrap();
        let engine = apps_runtime::jsc::Engine::new(
            apps_runtime::jsc::Api::load().unwrap(),
            std::rc::Rc::new(std::cell::RefCell::new(apps_runtime::host::Host::default())),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        engine.bundle(&files, "app.js").unwrap();
        assert!(!dir.path().join("data.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn node_ffi_imports() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("app.js");
        fs::write(&entry, r#"
            import ffi, { DynamicLibrary, getRawPointer, getInt32, setInt32, types } from 'node:ffi';
            const lib = new DynamicLibrary(null);
            const abs = lib.getFunction('abs', { arguments: [types.INT_32], return: types.INT_32 });
            if (abs(-42) !== 42 || ffi.dlsym(lib, 'abs') !== abs.pointer) throw Error('FFI import');
            const storage = new Uint8Array(4);
            const pointer = getRawPointer(storage);
            setInt32(pointer, 0, 42);
            if (getInt32(pointer) !== 42) throw Error('Memory import');
            lib.close();
        "#).unwrap();
        for (realm, role) in [
            (Realm::Main, apps_runtime::host::Role::Main),
            (Realm::Preload, apps_runtime::host::Role::Preload(1)),
        ] {
            let files = compile(&entry, realm, None, Rc::default(), false).unwrap();
            let engine = apps_runtime::jsc::Engine::new(
                apps_runtime::jsc::Api::load().unwrap(),
                Rc::new(RefCell::new(apps_runtime::host::Host::default())),
                role,
            )
            .unwrap();
            engine.bundle(&files, "app.js").unwrap();
        }
    }

    #[test]
    fn node_sqlite_imports_persistence_and_context_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("app.ts");
        let file = serde_json::to_string(&dir.path().join("data.sqlite")).unwrap();
        fs::write(&entry, format!(r#"
            import sqlite, {{ DatabaseSync, StatementSync, Database, Statement }} from 'node:sqlite';
            if (sqlite.DatabaseSync !== DatabaseSync || Database !== DatabaseSync || Statement !== StatementSync) throw Error('exports');
            const db = new DatabaseSync({file});
            db.exec('CREATE TABLE data (id INTEGER PRIMARY KEY, value TEXT)');
            const insert = db.prepare('INSERT INTO data (value) VALUES (?)');
            if (!(insert instanceof StatementSync)) throw Error('statement');
            insert.run('saved');
            db.exec('BEGIN');
            insert.run('rolled back on context close');
        "#)).unwrap();
        let files = compile(&entry, Realm::Main, None, Rc::default(), false).unwrap();
        let api = apps_runtime::jsc::Api::load().unwrap();
        let host = Rc::new(RefCell::new(apps_runtime::host::Host::default()));
        {
            let engine = apps_runtime::jsc::Engine::new(
                api.clone(),
                host.clone(),
                apps_runtime::host::Role::Main,
            )
            .unwrap();
            engine.bundle(&files, "app.js").unwrap();
        }
        fs::write(&entry, format!(r#"
            import {{ DatabaseSync }} from 'node:sqlite';
            const db = new DatabaseSync({file}, {{ readOnly: true }});
            const rows = db.prepare('SELECT value FROM data').all();
            if (rows.length !== 1 || rows[0].value !== 'saved') throw Error('persistence');
            let rejected = false;
            try {{ db.exec("INSERT INTO data (value) VALUES ('forbidden')"); }} catch {{ rejected = true; }}
            if (!rejected) throw Error('read-only');
            db.close();
        "#)).unwrap();
        let files = compile(&entry, Realm::Preload, None, Rc::default(), false).unwrap();
        let engine =
            apps_runtime::jsc::Engine::new(api, host, apps_runtime::host::Role::Preload(1))
                .unwrap();
        engine.bundle(&files, "preload.js").unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn child_process_example_with_top_level_await() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("app.js");
        fs::write(
            &entry,
            format!(
                "{}\nif (code !== 0) throw Error('ls failed');",
                include_str!("../../../examples/child-process/main.js")
            ),
        )
        .unwrap();
        let files = compile(&entry, Realm::Main, None, Rc::default(), false).unwrap();
        assert_eq!(
            apps_runtime::desktop::Desktop::new(&files)
                .unwrap()
                .check()
                .unwrap(),
            0
        );
    }

    #[test]
    fn wasm_example_with_top_level_await() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("app.js");
        fs::write(
            &entry,
            format!(
                "{}\nif (instance.exports.add(20, 22) !== 42) throw Error('Wasm result');",
                include_str!("../../../examples/wasm/main.js")
            ),
        )
        .unwrap();
        for production in [false, true] {
            let files = compile(&entry, Realm::Main, None, Rc::default(), production).unwrap();
            assert_eq!(
                apps_runtime::desktop::Desktop::new(&files)
                    .unwrap()
                    .check()
                    .unwrap(),
                0
            );
        }
    }

    #[test]
    fn top_level_await_waits_for_shared_dependencies_and_cycles() {
        for production in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join("shared.js"), "globalThis.order = []; export let value = 0; await new Promise(resolve => setTimeout(resolve, 5)); value = 42; order.push('shared');").unwrap();
            fs::write(
            dir.path().join("left.js"),
            "import { value } from './shared.js'; export const left = value; order.push('left');",
        )
        .unwrap();
            fs::write(
            dir.path().join("right.js"),
            "import { value } from './shared.js'; export const right = value; order.push('right');",
        )
        .unwrap();
            let entry = dir.path().join("app.ts");
            fs::write(&entry, "import { left } from './left.js'; import { right } from './right.js'; if (left !== 42 || right !== 42 || order[0] !== 'shared' || order.length !== 3) throw Error('Async dependency order');").unwrap();
            let files = compile(&entry, Realm::Main, None, Rc::default(), production).unwrap();
            assert_eq!(
                apps_runtime::desktop::Desktop::new(&files)
                    .unwrap()
                    .check()
                    .unwrap(),
                0
            );
            fs::write(dir.path().join("a.js"), "import { b } from './b.js'; export function a() { return 'a'; } await Promise.resolve(); export const result = b();").unwrap();
            fs::write(dir.path().join("b.js"), "import { a } from './a.js'; export function b() { return a() + 'b'; } await Promise.resolve();").unwrap();
            fs::write(
                &entry,
                "import { result } from './a.js'; if (result !== 'ab') throw Error('Async cycle');",
            )
            .unwrap();
            let files = compile(&entry, Realm::Main, None, Rc::default(), production).unwrap();
            assert_eq!(
                apps_runtime::desktop::Desktop::new(&files)
                    .unwrap()
                    .check()
                    .unwrap(),
                0
            );
        }
    }

    #[test]
    fn top_level_await_reports_rejections() {
        for production in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let entry = dir.path().join("app.js");
            for source in [
                "await Promise.reject(Error('startup failure'));",
                "await new Promise((resolve, reject) => setTimeout(() => reject(Error('startup failure')), 5));",
                "await Promise.resolve(); throw Error('startup failure');",
            ] {
                fs::write(&entry, source).unwrap();
                let files = compile(&entry, Realm::Main, None, Rc::default(), production).unwrap();
                let error = apps_runtime::desktop::Desktop::new(&files)
                    .unwrap()
                    .check()
                    .unwrap_err();
                assert!(error.to_string().contains("startup failure"), "{error}");
            }
        }
    }
    #[cfg(unix)]
    #[test]
    fn native_addons_copy_package_paths_and_survive_relocation() {
        for production in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let input = dir.path().join("source");
            let package = input.join("node_modules/@fixture/native");
            fs::create_dir_all(package.join("build")).unwrap();
            fs::create_dir_all(input.join("local/build")).unwrap();
            fs::write(
                input.join("package.json"),
                r#"{"name":"local_app","productName":"Local App"}"#,
            )
            .unwrap();
            fs::write(
                package.join("package.json"),
                r#"{"name":"@fixture/native","main":"index.cjs"}"#,
            )
            .unwrap();
            fs::write(
                package.join("index.cjs"),
                "module.exports=require('./build/addon');",
            )
            .unwrap();
            let library = package.join("build/addon.node");
            let mut command = std::process::Command::new("cc");
            if cfg!(target_os = "macos") {
                command.args(["-dynamiclib", "-undefined", "dynamic_lookup"]);
            } else {
                command.args(["-shared", "-fPIC"]);
            }
            let output = command
                .arg("-o")
                .arg(&library)
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/tests/napi.c"))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            fs::copy(&library, input.join("local/build/own.node")).unwrap();
            let entry = input.join("app.js");
            fs::write(&entry, "import addon, {add} from '@fixture/native/build/addon.node'; import wrapper from '@fixture/native'; import local from './local/build/own.node'; import {app, BrowserWindow} from '@pierre/apps'; if(add(20,21)!==42 || addon!==wrapper || addon.state()!==1 || local.state()!==1)throw Error('native imports'); app.whenReady().then(()=>new BrowserWindow().loadFile(import.meta.resolve('./index.html')));").unwrap();
            fs::write(input.join("index.html"), "<script type='preload'>import addon from '@fixture/native/build/addon.node'; if(addon.add(1,2)!==4 || addon.state()!==1)throw Error('preload addon');</script><body>Native addon</body>").unwrap();
            let watched = Rc::default();
            let files =
                compile(&entry, Realm::Main, None, Rc::clone(&watched), production).unwrap();
            assert!(watched.borrow().contains(&library.canonicalize().unwrap()));
            assert_eq!(
                apps_runtime::desktop::Desktop::new(&files)
                    .unwrap()
                    .check()
                    .unwrap(),
                0
            );
            let output = dir.path().join("dist");
            crate::build::write_bundle(
                &output,
                &files,
                &entry,
                &input.join("public"),
                production,
                crate::target::Target::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
            )
            .unwrap();
            let moved = dir.path().join("relocated");
            fs::rename(output, &moved).unwrap();
            fs::rename(&input, dir.path().join("hidden-source")).unwrap();
            let assets = if cfg!(target_os = "macos") {
                moved.join("Local App.app/Contents/Resources")
            } else {
                moved.clone()
            };
            let bundle = apps_runtime::bundle::read(&moved).unwrap();
            assert!(!bundle.files["native.json"].contains(input.to_str().unwrap()));
            let native: std::collections::BTreeMap<String, PathBuf> =
                serde_json::from_str(&bundle.files["native.json"]).unwrap();
            assert_eq!(native.len(), 2);
            for name in [
                "@fixture/native/build/addon.node",
                "local_app/local/build/own.node",
            ] {
                let path = assets.join("node_modules").join(name);
                assert!(path.is_file(), "Missing copied addon: {name}");
                assert_eq!(native[name], path.canonicalize().unwrap());
            }
            assert_eq!(
                apps_runtime::desktop::Desktop::from_bundle(bundle)
                    .unwrap()
                    .check()
                    .unwrap(),
                0
            );
        }
    }

    #[test]
    fn native_addons_reject_renderer_imports_and_output_collisions() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("node_modules/native/addon.node");
        let second = dir
            .path()
            .join("node_modules/parent/node_modules/native/addon.node");
        fs::create_dir_all(first.parent().unwrap()).unwrap();
        fs::create_dir_all(second.parent().unwrap()).unwrap();
        fs::write(&first, [0, 255, 128]).unwrap();
        fs::write(&second, [0, 255, 129]).unwrap();
        let entry = dir.path().join("app.js");
        fs::write(
            &entry,
            "import addon from './node_modules/native/addon.node';",
        )
        .unwrap();
        assert!(
            compile(&entry, Realm::Renderer, None, Rc::default(), false)
                .unwrap_err()
                .to_string()
                .contains("unavailable in page scripts")
        );
        fs::write(&entry, "import a from './node_modules/native/addon.node';import b from './node_modules/parent/node_modules/native/addon.node';").unwrap();
        assert!(
            compile(&entry, Realm::Main, None, Rc::default(), false)
                .unwrap_err()
                .to_string()
                .contains("output collision")
        );
    }

    #[test]
    fn resolves_dotted_files_package_imports_browser_fields_and_type_only_imports() {
        let dir = tempfile::tempdir().unwrap();
        let modules = dir.path().join("node_modules");
        for (path, source) in [
            (
                "app.config.ts",
                "export interface Shape { value: string } export const value = 'dotted';",
            ),
            (
                "app.ts",
                "import { Shape, value } from './app.config'; import { emit } from 'emitter'; import { path } from 'scoped'; const shape: Shape = { value }; globalThis.result = [shape.value, emit(), path].join(':');",
            ),
            ("style.css", "@import './base.css' é;"),
            (
                "node_modules/emitter/package.json",
                r#"{"main":"node.js","browser":"browser.js"}"#,
            ),
            (
                "node_modules/emitter/node.js",
                "const {EventEmitter}=require('events');exports.emit=()=>'node:'+typeof EventEmitter;",
            ),
            (
                "node_modules/emitter/browser.js",
                "const {EventEmitter}=require('events');exports.emit=()=>new EventEmitter().kind;",
            ),
            (
                "node_modules/events/package.json",
                r#"{"main":"events.js"}"#,
            ),
            (
                "node_modules/events/events.js",
                "class EventEmitter{kind='polyfill'};module.exports={EventEmitter};",
            ),
            (
                "node_modules/scoped/package.json",
                r##"{"type":"module","exports":"./index.js","imports":{"#path":{"node":"./node.js","default":"./browser.js"},"#*":"./lib/*.js"}}"##,
            ),
            (
                "node_modules/scoped/index.js",
                "import {name} from '#path'; import {suffix} from '#suffix'; export const path = name + suffix;",
            ),
            ("node_modules/scoped/node.js", "export const name = 'node';"),
            (
                "node_modules/scoped/browser.js",
                "export const name = 'browser';",
            ),
            (
                "node_modules/scoped/lib/suffix.js",
                "export const suffix = '!';",
            ),
        ] {
            fs::create_dir_all(dir.path().join(path).parent().unwrap()).unwrap();
            fs::write(dir.path().join(path), source).unwrap();
        }
        assert!(modules.is_dir());
        for (realm, expected) in [
            (Realm::Main, "dotted:node:function:node!"),
            (Realm::Renderer, "dotted:polyfill:browser!"),
        ] {
            let files = compile(
                &dir.path().join("app.ts"),
                realm,
                None,
                Rc::default(),
                false,
            )
            .unwrap();
            let engine = apps_runtime::jsc::Engine::new(
                apps_runtime::jsc::Api::load().unwrap(),
                Rc::default(),
                apps_runtime::host::Role::Main,
            )
            .unwrap();
            engine.bundle(&files, "app.js").unwrap();
            assert_eq!(engine.eval("result", "test").unwrap(), expected);
        }
        assert!(
            css(
                &dir.path().join("style.css"),
                &mut Vec::new(),
                &Rc::default()
            )
            .is_err()
        );
    }

    #[test]
    fn workers_keep_native_addons_from_earlier_modules() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("node_modules/native")).unwrap();
        fs::write(dir.path().join("node_modules/native/addon.node"), [0]).unwrap();
        fs::write(
            dir.path().join("native.js"),
            "import addon from './node_modules/native/addon.node'; export default addon;",
        )
        .unwrap();
        fs::write(dir.path().join("worker.js"), "postMessage(1);").unwrap();
        fs::write(
            dir.path().join("spawn.js"),
            "export const worker = () => new Worker(new URL('./worker.js', import.meta.url));",
        )
        .unwrap();
        let entry = dir.path().join("app.js");
        fs::write(&entry, "import addon from './native.js'; import {worker} from './spawn.js'; globalThis.x = [addon, worker];").unwrap();
        let files = compile(&entry, Realm::Main, None, Rc::default(), false).unwrap();
        let native: std::collections::BTreeMap<String, PathBuf> =
            serde_json::from_str(&files["native.json"]).unwrap();
        assert_eq!(
            native["native/addon.node"],
            dir.path()
                .join("node_modules/native/addon.node")
                .canonicalize()
                .unwrap()
        );
    }
}
