use anyhow::{Context, Result, bail, ensure};
use apps_runtime::{bundle_source, dom::Document};
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
                | "path/posix"
                | "path/win32"
                | "os"
                | "events"
                | "child_process"
                | "buffer"
                | "stream"
                | "stream/promises"
                | "stream/web"
                | "stream/consumers"
                | "crypto"
                | "url"
                | "util"
                | "util/types"
                | "string_decoder"
                | "worker_threads"
                | "module"
                | "net"
                | "http"
                | "https"
                | "timers"
                | "timers/promises"
                | "process"
                | "assert"
                | "assert/strict"
                | "zlib"
                | "readline"
                | "readline/promises"
                | "tty"
                | "dns"
                | "dns/promises"
                | "querystring"
                | "perf_hooks"
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
                "node:buffer" => (
                    "globalThis.__apps.buffer",
                    vec![
                        "Buffer",
                        "Blob",
                        "File",
                        "atob",
                        "btoa",
                        "constants",
                        "kMaxLength",
                        "kStringMaxLength",
                        "isUtf8",
                        "isAscii",
                        "INSPECT_MAX_BYTES",
                    ],
                ),
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
                        "addAbortSignal",
                        "promises",
                    ],
                ),
                "node:stream/promises" => (
                    "globalThis.__apps.stream.promises",
                    vec!["pipeline", "finished"],
                ),
                "node:stream/consumers" => (
                    "globalThis.__apps.stream.consumers",
                    vec!["arrayBuffer", "blob", "buffer", "json", "text"],
                ),
                "node:stream/web" => (
                    "globalThis",
                    vec![
                        "ReadableStream",
                        "WritableStream",
                        "TransformStream",
                        "ByteLengthQueuingStrategy",
                        "CountQueuingStrategy",
                        "TextEncoderStream",
                        "TextDecoderStream",
                        "CompressionStream",
                        "DecompressionStream",
                    ],
                ),
                "node:util" => (
                    "globalThis.__apps.util",
                    vec![
                        "promisify",
                        "callbackify",
                        "inspect",
                        "format",
                        "formatWithOptions",
                        "inherits",
                        "isDeepStrictEqual",
                        "deprecate",
                        "debuglog",
                        "debug",
                        "parseArgs",
                        "stripVTControlCharacters",
                        "styleText",
                        "aborted",
                        "getSystemErrorName",
                        "types",
                        "TextEncoder",
                        "TextDecoder",
                    ],
                ),
                "node:util/types" => (
                    "globalThis.__apps.util.types",
                    vec![
                        "isAnyArrayBuffer",
                        "isArrayBuffer",
                        "isArrayBufferView",
                        "isAsyncFunction",
                        "isBoxedPrimitive",
                        "isDataView",
                        "isDate",
                        "isGeneratorFunction",
                        "isGeneratorObject",
                        "isMap",
                        "isNativeError",
                        "isPromise",
                        "isRegExp",
                        "isSet",
                        "isSharedArrayBuffer",
                        "isTypedArray",
                        "isUint8Array",
                        "isWeakMap",
                        "isWeakSet",
                    ],
                ),
                "node:url" => (
                    "globalThis.__apps.url",
                    vec![
                        "URL",
                        "URLSearchParams",
                        "fileURLToPath",
                        "pathToFileURL",
                        "format",
                        "urlToHttpOptions",
                    ],
                ),
                "node:string_decoder" => {
                    ("globalThis.__apps.string_decoder", vec!["StringDecoder"])
                }
                "node:crypto" => (
                    "globalThis.__apps.crypto",
                    vec![
                        "createHash",
                        "createHmac",
                        "hash",
                        "getHashes",
                        "getCiphers",
                        "createCipheriv",
                        "createDecipheriv",
                        "pbkdf2",
                        "pbkdf2Sync",
                        "randomBytes",
                        "randomFill",
                        "randomFillSync",
                        "randomInt",
                        "randomUUID",
                        "getRandomValues",
                        "timingSafeEqual",
                        "webcrypto",
                        "subtle",
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
                    "globalThis.__apps.timers",
                    vec![
                        "setTimeout",
                        "clearTimeout",
                        "setInterval",
                        "clearInterval",
                        "setImmediate",
                        "clearImmediate",
                        "promises",
                    ],
                ),
                "node:timers/promises" => (
                    "globalThis.__apps.timers.promises",
                    vec!["setTimeout", "setImmediate", "setInterval", "scheduler"],
                ),
                "node:process" => (
                    "globalThis.__apps.process",
                    vec![
                        "platform",
                        "arch",
                        "pid",
                        "ppid",
                        "title",
                        "version",
                        "versions",
                        "release",
                        "argv",
                        "argv0",
                        "execArgv",
                        "execPath",
                        "env",
                        "cwd",
                        "chdir",
                        "nextTick",
                        "exit",
                        "on",
                        "once",
                        "off",
                        "emit",
                        "kill",
                        "hrtime",
                        "uptime",
                        "memoryUsage",
                        "cpuUsage",
                        "umask",
                        "emitWarning",
                        "stdout",
                        "stderr",
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
                "node:events" => (
                    "globalThis.__apps.events",
                    vec![
                        "EventEmitter",
                        "once",
                        "on",
                        "errorMonitor",
                        "getEventListeners",
                        "setMaxListeners",
                        "defaultMaxListeners",
                    ],
                ),
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
                        "Stats",
                        "Dirent",
                        "readFile",
                        "writeFile",
                        "appendFile",
                        "mkdir",
                        "readdir",
                        "stat",
                        "lstat",
                        "fstat",
                        "rename",
                        "copyFile",
                        "cp",
                        "realpath",
                        "readlink",
                        "symlink",
                        "link",
                        "chmod",
                        "chown",
                        "lchown",
                        "utimes",
                        "lutimes",
                        "truncate",
                        "access",
                        "unlink",
                        "rm",
                        "rmdir",
                        "mkdtemp",
                        "open",
                        "opendir",
                        "close",
                        "fsync",
                        "ftruncate",
                        "readFileSync",
                        "writeFileSync",
                        "appendFileSync",
                        "mkdirSync",
                        "readdirSync",
                        "statSync",
                        "lstatSync",
                        "fstatSync",
                        "renameSync",
                        "copyFileSync",
                        "cpSync",
                        "realpathSync",
                        "readlinkSync",
                        "symlinkSync",
                        "linkSync",
                        "chmodSync",
                        "chownSync",
                        "lchownSync",
                        "utimesSync",
                        "lutimesSync",
                        "truncateSync",
                        "accessSync",
                        "unlinkSync",
                        "rmSync",
                        "rmdirSync",
                        "mkdtempSync",
                        "openSync",
                        "opendirSync",
                        "closeSync",
                        "fsyncSync",
                        "ftruncateSync",
                        "existsSync",
                        "read",
                        "readSync",
                        "write",
                        "writeSync",
                        "watch",
                        "watchFile",
                        "unwatchFile",
                        "createReadStream",
                        "createWriteStream",
                    ],
                ),
                "node:fs/promises" => (
                    "globalThis.__apps.fs.promises",
                    vec![
                        "constants",
                        "readFile",
                        "writeFile",
                        "appendFile",
                        "mkdir",
                        "readdir",
                        "stat",
                        "lstat",
                        "rename",
                        "copyFile",
                        "cp",
                        "realpath",
                        "readlink",
                        "symlink",
                        "link",
                        "chmod",
                        "chown",
                        "lchown",
                        "utimes",
                        "lutimes",
                        "truncate",
                        "access",
                        "unlink",
                        "rm",
                        "rmdir",
                        "mkdtemp",
                        "open",
                        "opendir",
                        "watch",
                    ],
                ),
                "node:os" => (
                    "globalThis.__apps.os",
                    vec![
                        "EOL",
                        "devNull",
                        "platform",
                        "arch",
                        "machine",
                        "release",
                        "version",
                        "type",
                        "homedir",
                        "tmpdir",
                        "hostname",
                        "endianness",
                        "availableParallelism",
                        "cpus",
                        "totalmem",
                        "freemem",
                        "uptime",
                        "loadavg",
                        "networkInterfaces",
                        "userInfo",
                        "constants",
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
                        "parse",
                        "format",
                        "toNamespacedPath",
                        "posix",
                        "win32",
                    ],
                ),
                "node:path/posix" => (
                    "globalThis.__apps.path.posix",
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
                        "parse",
                        "format",
                        "toNamespacedPath",
                        "posix",
                        "win32",
                    ],
                ),
                "node:path/win32" => (
                    "globalThis.__apps.path.win32",
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
                        "parse",
                        "format",
                        "toNamespacedPath",
                        "posix",
                        "win32",
                    ],
                ),
                "node:assert" => (
                    "globalThis.__apps.assert",
                    vec![
                        "AssertionError",
                        "ok",
                        "equal",
                        "notEqual",
                        "strictEqual",
                        "notStrictEqual",
                        "deepEqual",
                        "notDeepEqual",
                        "deepStrictEqual",
                        "notDeepStrictEqual",
                        "throws",
                        "doesNotThrow",
                        "rejects",
                        "doesNotReject",
                        "match",
                        "doesNotMatch",
                        "fail",
                        "ifError",
                        "strict",
                    ],
                ),
                "node:assert/strict" => (
                    "globalThis.__apps.assert.strict",
                    vec![
                        "AssertionError",
                        "ok",
                        "equal",
                        "notEqual",
                        "strictEqual",
                        "notStrictEqual",
                        "deepEqual",
                        "notDeepEqual",
                        "deepStrictEqual",
                        "notDeepStrictEqual",
                        "throws",
                        "doesNotThrow",
                        "rejects",
                        "doesNotReject",
                        "match",
                        "doesNotMatch",
                        "fail",
                        "ifError",
                        "strict",
                    ],
                ),
                "node:zlib" => (
                    "globalThis.__apps.zlib",
                    vec![
                        "constants",
                        "gzip",
                        "gzipSync",
                        "createGzip",
                        "Gzip",
                        "gunzip",
                        "gunzipSync",
                        "createGunzip",
                        "Gunzip",
                        "deflate",
                        "deflateSync",
                        "createDeflate",
                        "Deflate",
                        "inflate",
                        "inflateSync",
                        "createInflate",
                        "Inflate",
                        "deflateRaw",
                        "deflateRawSync",
                        "createDeflateRaw",
                        "DeflateRaw",
                        "inflateRaw",
                        "inflateRawSync",
                        "createInflateRaw",
                        "InflateRaw",
                        "unzip",
                        "unzipSync",
                        "createUnzip",
                        "Unzip",
                    ],
                ),
                "node:readline" => (
                    "globalThis.__apps.readline",
                    vec![
                        "Interface",
                        "createInterface",
                        "clearLine",
                        "clearScreenDown",
                        "cursorTo",
                        "moveCursor",
                        "emitKeypressEvents",
                        "promises",
                    ],
                ),
                "node:readline/promises" => (
                    "globalThis.__apps.readline.promises",
                    vec!["Interface", "createInterface"],
                ),
                "node:tty" => ("globalThis.__apps.tty", vec!["isatty"]),
                "node:dns" => ("globalThis.__apps.dns", vec!["lookup", "promises"]),
                "node:dns/promises" => ("globalThis.__apps.dns.promises", vec!["lookup"]),
                "node:querystring" => (
                    "globalThis.__apps.querystring",
                    vec![
                        "parse",
                        "stringify",
                        "escape",
                        "unescape",
                        "decode",
                        "encode",
                    ],
                ),
                "node:perf_hooks" => ("globalThis.__apps.perf_hooks", vec!["performance"]),
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
mod tests;
