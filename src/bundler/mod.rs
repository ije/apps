use anyhow::{Context, Result, bail, ensure};
use apps_runtime::{bundle_source, dom::Document};
use bbtree::unsync::BBTreeMap;
use oxc_allocator::{Allocator, ArenaVec, TakeIn};
use oxc_ast::{ast::*, builder::AstBuilder};
use oxc_ast_visit::{Visit, VisitMut, walk, walk_mut};
use oxc_codegen::{Codegen, CodegenOptions, CommentOptions};
use oxc_minifier::{Minifier, MinifierOptions};
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::{GetSpan, SourceType, Span};
use oxc_transformer::{TransformOptions, Transformer};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
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
    production: bool,
    modules: Vec<Value>,
    lines: Vec<Option<Rc<str>>>,
    ids: BBTreeMap<PathBuf, usize>,
    graph: Graph,
    requests: Vec<(PathBuf, String, bool, usize)>,
    inlined: usize,
    package_roots: BTreeSet<PathBuf>,
    dynamic: BTreeSet<usize>,
    assets: BBTreeMap<PathBuf, (String, String)>,
    files: Rc<RefCell<BTreeSet<PathBuf>>>,
    shared: Rc<Shared>,
}

#[derive(Default)]
struct Shared {
    root: PathBuf,
    manifests: RefCell<BBTreeMap<PathBuf, Rc<Value>>>,
    workers: RefCell<BBTreeMap<(PathBuf, Realm), usize>>,
    scripts: RefCell<Vec<String>>,
    cache: Option<Rc<Cache>>,
}

// WARNING: cached modules contain `$appsImport(N)` and `$appsN<id>`, so a graph never reuses or changes module ids.
#[derive(Default)]
pub struct Cache {
    graphs: RefCell<HashMap<(Realm, PathBuf), Graph>>,
    pub resolutions: RefCell<HashMap<(Realm, PathBuf, String, bool), Resolved>>,
}

type Resolved = (PathBuf, Option<PathBuf>, Option<PathBuf>);
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

struct Linker<'a, 'b> {
    builder: &'b mut Builder,
    allocator: &'a Allocator,
    ast: AstBuilder<'a>,
    scoping: &'b oxc_semantic::Scoping,
    path: &'b Path,
    commonjs: bool,
    cacheable: bool,
    imported: Vec<usize>,
    required: Vec<usize>,
    resolved: serde_json::Map<String, Value>,
    error: Option<anyhow::Error>,
}

/// The named exports of `electron` and `@pierre/apps`.
const ELECTRON: &[&str] = &[
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
    "globalShortcut",
    "session",
    "powerMonitor",
    "screen",
];

/// `$appsN<module>.<name>`: how linked module code reads an import, so it needs no `with` scope.
fn namespace_access(dep: usize, name: &str) -> String {
    let identifier = name.starts_with(|c: char| c.is_alphabetic() || c == '_' || c == '$')
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$');
    match name {
        "*" => format!("$appsN{dep}"),
        _ if identifier => format!("$appsN{dep}.{name}"),
        _ => format!("$appsN{dep}[{}]", json!(name)),
    }
}

/// Rewrites references to import bindings into namespace reads. Namespace getters keep them live.
struct Imports<'a, 'b> {
    ast: AstBuilder<'a>,
    allocator: &'a Allocator,
    scoping: &'b oxc_semantic::Scoping,
    bindings: &'b HashMap<oxc_semantic::SymbolId, (usize, String)>,
}

impl<'a> Imports<'a, '_> {
    fn binding(&self, ident: &IdentifierReference<'a>) -> Option<&(usize, String)> {
        let reference = ident.reference_id.get()?;
        self.bindings
            .get(&self.scoping.get_reference(reference).symbol_id()?)
    }

    fn access(&self, span: Span, (dep, name): &(usize, String)) -> Expression<'a> {
        let namespace = Expression::new_identifier(
            span,
            self.allocator.alloc_str(&format!("$appsN{dep}")),
            &self.ast,
        );
        if name == "*" {
            return namespace;
        }
        let property = self.allocator.alloc_str(name);
        if namespace_access(*dep, name).contains('[') {
            Expression::new_computed_member_expression(
                span,
                namespace,
                Expression::new_string_literal(span, property, None, &self.ast),
                false,
                &self.ast,
            )
        } else {
            Expression::new_static_member_expression(
                span,
                namespace,
                IdentifierName::new(span, property, &self.ast),
                false,
                &self.ast,
            )
        }
    }

    /// `(0, ns.f)` keeps `this` undefined when an imported function is called.
    fn callee(&self, expression: &mut Expression<'a>) {
        if let Expression::Identifier(ident) = expression
            && let Some(binding) = self.binding(ident).filter(|(_, name)| name != "*").cloned()
        {
            let span = ident.span;
            let zero =
                Expression::new_numeric_literal(span, 0., None, NumberBase::Decimal, &self.ast);
            *expression = Expression::new_sequence_expression(
                span,
                ArenaVec::from_array_in([zero, self.access(span, &binding)], &self.ast),
                &self.ast,
            );
        }
    }
}

impl<'a> VisitMut<'a> for Imports<'a, '_> {
    fn visit_expression(&mut self, expression: &mut Expression<'a>) {
        if let Expression::Identifier(ident) = expression
            && let Some(binding) = self.binding(ident).cloned()
        {
            *expression = self.access(ident.span, &binding);
            return;
        }
        walk_mut::walk_expression(self, expression);
    }

    fn visit_call_expression(&mut self, call: &mut CallExpression<'a>) {
        self.callee(&mut call.callee);
        walk_mut::walk_call_expression(self, call);
    }

    fn visit_tagged_template_expression(&mut self, tagged: &mut TaggedTemplateExpression<'a>) {
        self.callee(&mut tagged.tag);
        walk_mut::walk_tagged_template_expression(self, tagged);
    }

    fn visit_object_property(&mut self, property: &mut ObjectProperty<'a>) {
        if let Expression::Identifier(ident) = &property.value
            && self.binding(ident).is_some()
        {
            property.shorthand = false;
        }
        walk_mut::walk_object_property(self, property);
    }
}

/// Finds top-level `await` and `for await`.
struct TopLevelAwait(bool, bool);

impl<'a> Visit<'a> for TopLevelAwait {
    fn visit_function(&mut self, _: &Function<'a>, _: oxc_semantic::ScopeFlags) {}
    fn visit_arrow_function_expression(&mut self, _: &ArrowFunctionExpression<'a>) {}
    fn visit_class(&mut self, _: &Class<'a>) {}
    fn visit_await_expression(&mut self, expression: &AwaitExpression<'a>) {
        self.0 = true;
        walk::walk_await_expression(self, expression);
    }
    fn visit_for_of_statement(&mut self, statement: &ForOfStatement<'a>) {
        self.1 |= statement.r#await;
        walk::walk_for_of_statement(self, statement);
    }
}

// WARNING: top-level `await` becomes `yield`, and the loader resumes the generator with the awaited value.
// The body then runs synchronously until its first `await`, like an ES module, so later page scripts run after it.
struct Awaits<'a>(AstBuilder<'a>);

impl<'a> VisitMut<'a> for Awaits<'a> {
    fn visit_function(&mut self, _: &mut Function<'a>, _: oxc_semantic::ScopeFlags) {}
    fn visit_arrow_function_expression(&mut self, _: &mut ArrowFunctionExpression<'a>) {}
    fn visit_class(&mut self, _: &mut Class<'a>) {}
    fn visit_expression(&mut self, expression: &mut Expression<'a>) {
        if let Expression::AwaitExpression(node) = expression {
            let argument = node.argument.take_in(&self.0);
            *expression =
                Expression::new_yield_expression(node.span, false, Some(argument), &self.0);
        }
        walk_mut::walk_expression(self, expression);
    }
}

/// Collects `$appsN<module>.<name>` reads; `None` means the whole namespace is used.
struct Uses(Vec<(usize, Option<String>)>);

impl<'a> Visit<'a> for Uses {
    fn visit_static_member_expression(&mut self, member: &StaticMemberExpression<'a>) {
        if let Expression::Identifier(object) = &member.object
            && let Some(dep) = object
                .name
                .strip_prefix("$appsN")
                .and_then(|id| id.parse().ok())
        {
            self.0.push((dep, Some(member.property.name.to_string())));
        } else {
            walk::walk_static_member_expression(self, member);
        }
    }

    fn visit_computed_member_expression(&mut self, member: &ComputedMemberExpression<'a>) {
        if let (Expression::Identifier(object), Expression::StringLiteral(key)) =
            (&member.object, &member.expression)
            && let Some(dep) = object
                .name
                .strip_prefix("$appsN")
                .and_then(|id| id.parse().ok())
        {
            self.0.push((dep, key.value.as_str().map(str::to_owned)));
        } else {
            walk::walk_computed_member_expression(self, member);
        }
    }

    fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
        if let Some(dep) = ident
            .name
            .strip_prefix("$appsN")
            .and_then(|id| id.parse().ok())
        {
            self.0.push((dep, None));
        }
    }
}

impl<'a> Linker<'a, '_> {
    /// Bundles an HTML page of the app and returns its `apps:///` URL.
    fn page(&mut self, target: PathBuf) -> Result<&'a str> {
        self.cacheable = false;
        if !self.builder.assets.contains_key(&target) {
            self.builder.files.borrow_mut().insert(target.clone());
            let name = format!("page-{}.html", self.builder.assets.len());
            let source = html(
                &target,
                self.builder.files.clone(),
                self.builder.shared.clone(),
                self.builder.production,
            )?;
            self.builder.assets.insert(target.clone(), (name, source));
        }
        Ok(self
            .allocator
            .alloc_str(&format!("apps:///{}", self.builder.assets[&target].0)))
    }
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
                let key = (
                    resolve_file(&self.path.parent().unwrap_or(Path::new(".")).join(specifier))?,
                    if self.builder.realm == Realm::Renderer {
                        Realm::Renderer
                    } else {
                        Realm::Main
                    },
                );
                self.cacheable = false;
                let shared = self.builder.shared.clone();
                let existing = shared.workers.borrow().get(&key).copied();
                let index = match existing {
                    Some(index) => index,
                    None => {
                        let index = shared.scripts.borrow().len();
                        shared.scripts.borrow_mut().push(String::new());
                        shared.workers.borrow_mut().insert(key.clone(), index);
                        let files = bundle(
                            vec![(key.0, None)],
                            key.1,
                            self.builder.files.clone(),
                            shared.clone(),
                            self.builder.production,
                        )?;
                        shared.scripts.borrow_mut()[index] = bundle_source(&files, true);
                        index
                    }
                };
                let value = self
                    .allocator
                    .alloc_str(&format!("apps:///script-{index}.js"));
                worker.arguments[0] =
                    Expression::new_string_literal(url.span, value, None, &self.ast).into();
            }
            if let Expression::NewExpression(url) = expression
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
                && let Some(path) = specifier.value.as_str()
                && path.starts_with('.')
                && let Some(asset) = asset(
                    self.path.parent().unwrap_or(Path::new(".")),
                    path,
                    &self.builder.files,
                )
            {
                self.cacheable = false;
                let value = self.allocator.alloc_str(&asset);
                url.arguments[0] =
                    Expression::new_string_literal(specifier.span, value, None, &self.ast).into();
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
                    let url = if self.builder.production {
                        let relative = app_relative(&self.builder.shared.root, self.path);
                        let mut url = String::from("apps:///");
                        for &byte in relative.as_os_str().as_encoded_bytes() {
                            match byte {
                                b'a'..=b'z'
                                | b'A'..=b'Z'
                                | b'0'..=b'9'
                                | b'-'
                                | b'.'
                                | b'_'
                                | b'~' => url.push(byte as char),
                                _ if byte == std::path::MAIN_SEPARATOR as u8 => url.push('/'),
                                _ => {
                                    let _ = write!(url, "%{byte:02X}");
                                }
                            }
                        }
                        url
                    } else {
                        url::Url::from_file_path(std::path::absolute(self.path)?)
                            .map_err(|_| {
                                anyhow::anyhow!(
                                    "Cannot create a file URL for {}",
                                    self.path.display()
                                )
                            })?
                            .into()
                    };
                    let value = self.allocator.alloc_str(&url);
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
                let value = self.page(target)?;
                *expression = Expression::new_string_literal(span, value, None, &self.ast);
                return Ok(());
            }
            if self.commonjs
                && let Expression::CallExpression(call) = expression
                && let Expression::StaticMemberExpression(member) =
                    call.callee.get_inner_expression()
                && member.property.name == "resolve"
                && let Expression::Identifier(require) = member.object.get_inner_expression()
                && require.name == "require"
                && self
                    .scoping
                    .get_reference(require.reference_id())
                    .symbol_id()
                    .is_none()
                && let Some(Argument::StringLiteral(specifier)) = call.arguments.first()
                && let Some(specifier) = specifier.value.as_str()
            {
                let base = self.path.parent().unwrap_or(Path::new("."));
                self.cacheable = false;
                if let Ok((path, None)) = self.builder.locate(base, specifier, true)
                    && !path.as_os_str().as_encoded_bytes().starts_with(b"apps:")
                {
                    let path = self.builder.runtime_path(&path);
                    self.resolved.insert(specifier.to_owned(), path.into());
                }
            }
            // WARNING: like Electron, a relative `loadFile` path is relative to the app folder, not to the module.
            if let Expression::CallExpression(call) = expression
                && self.builder.realm == Realm::Main
                && let Expression::StaticMemberExpression(member) =
                    call.callee.get_inner_expression()
                && member.property.name == "loadFile"
                && let Some(Argument::StringLiteral(specifier)) = call.arguments.first()
                && let Some(path) = specifier.value.as_str()
                && (path.ends_with(".html") || path.ends_with(".htm"))
                && !path.contains(':')
                && !Path::new(path).is_absolute()
                && let Ok(target) = self.builder.shared.root.join(path).canonicalize()
            {
                let literal = specifier.span;
                let value = self.page(target)?;
                call.arguments[0] =
                    Expression::new_string_literal(literal, value, None, &self.ast).into();
                walk_mut::walk_expression(self, expression);
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
                        match call.arguments.as_slice() {
                            [Argument::StringLiteral(specifier)] => specifier
                                .value
                                .as_str()
                                .map(|specifier| ("$appsRequire", specifier)),
                            _ => None,
                        }
                    }
                }
                Expression::ImportMeta(_) => bail!(
                    "Use import.meta.resolve with one string literal or import.meta.url in {}",
                    self.path.display()
                ),
                _ => None,
            };
            if let Some((name, specifier)) = import {
                let base = self.path.parent().unwrap_or(Path::new("."));
                // WARNING: like esbuild, a require() that cannot be resolved throws MODULE_NOT_FOUND when it runs.
                let dep = if name == "$appsImport" {
                    self.builder.resolve(base, specifier, false)?
                } else if let Ok((path, inline)) = self.builder.locate(base, specifier, true) {
                    self.builder.visit(path, inline)?
                } else {
                    self.cacheable = false;
                    walk_mut::walk_expression(self, expression);
                    return Ok(());
                };
                self.builder.requests.push((
                    base.to_owned(),
                    specifier.to_owned(),
                    name == "$appsRequire",
                    dep,
                ));
                self.builder.dynamic.insert(dep);
                if name == "$appsRequire" {
                    &mut self.required
                } else {
                    &mut self.imported
                }
                .push(dep);
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
        let (path, inline) = self.locate(base, specifier, require)?;
        self.visit(path, inline)
    }

    fn runtime_path(&self, path: &Path) -> String {
        if !self.production {
            return std::path::absolute(path)
                .unwrap_or_else(|_| path.to_owned())
                .to_string_lossy()
                .into_owned();
        }
        app_relative(&self.shared.root, path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn locate(
        &mut self,
        base: &Path,
        specifier: &str,
        require: bool,
    ) -> Result<(PathBuf, Option<String>)> {
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
                | "constants"
                | "sys"
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
            if self.ids.contains_key(&path) {
                return Ok((path, None));
            }
            let (object, names) = match specifier {
                "@pierre/apps" | "electron" => ("globalThis.__apps", ELECTRON.to_vec()),
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
                "node:https" => (
                    "globalThis.__apps.https",
                    vec![
                        "Agent",
                        "Server",
                        "createServer",
                        "get",
                        "globalAgent",
                        "request",
                    ],
                ),
                "node:http" => (
                    "globalThis.__apps.http",
                    vec![
                        "Server",
                        "createServer",
                        "request",
                        "get",
                        "STATUS_CODES",
                        "METHODS",
                        "Agent",
                        "globalAgent",
                        "ClientRequest",
                        "IncomingMessage",
                        "OutgoingMessage",
                        "ServerResponse",
                        "validateHeaderName",
                        "validateHeaderValue",
                        "maxHeaderSize",
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
                        "transcode",
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
                        "consumers",
                        "compose",
                        "duplexPair",
                        "isErrored",
                        "isReadable",
                        "getDefaultHighWaterMark",
                        "setDefaultHighWaterMark",
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
                    "globalThis.__apps.module.createRequire()('stream/web')",
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
                        "ReadableStreamDefaultReader",
                        "ReadableStreamBYOBReader",
                        "ReadableStreamBYOBRequest",
                        "ReadableStreamDefaultController",
                        "ReadableByteStreamController",
                        "WritableStreamDefaultWriter",
                        "WritableStreamDefaultController",
                        "TransformStreamDefaultController",
                    ],
                ),
                "node:util" | "node:sys" => (
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
                        "getSystemErrorMessage",
                        "getSystemErrorMap",
                        "toUSVString",
                        "transferableAbortSignal",
                        "transferableAbortController",
                        "MIMEType",
                        "MIMEParams",
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
                        "isCryptoKey",
                        "isMapIterator",
                        "isSetIterator",
                        "isModuleNamespaceObject",
                        "isNumberObject",
                        "isStringObject",
                        "isBooleanObject",
                        "isBigIntObject",
                        "isSymbolObject",
                        "isUint8ClampedArray",
                        "isUint16Array",
                        "isUint32Array",
                        "isInt8Array",
                        "isInt16Array",
                        "isInt32Array",
                        "isFloat32Array",
                        "isFloat64Array",
                        "isBigInt64Array",
                        "isBigUint64Array",
                        "isKeyObject",
                        "isExternal",
                        "isProxy",
                        "isArgumentsObject",
                        "isFloat16Array",
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
                        "domainToASCII",
                        "domainToUnicode",
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
                        "constants",
                        "getCurves",
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
                        "BroadcastChannel",
                        "SHARE_ENV",
                        "receiveMessageOnPort",
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
                        "type",
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
                        "exitCode",
                        "getuid",
                        "geteuid",
                        "getgid",
                        "getegid",
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
                        "captureRejections",
                        "captureRejectionSymbol",
                        "getMaxListeners",
                        "addAbortListener",
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
                        "ReadStream",
                        "WriteStream",
                        "Dir",
                        "openAsBlob",
                        "fchmod",
                        "fchmodSync",
                        "fchown",
                        "fchownSync",
                        "futimes",
                        "futimesSync",
                        "fdatasync",
                        "fdatasyncSync",
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
                        "getPriority",
                        "setPriority",
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
                        "crc32",
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
                    vec!["Interface", "Readline", "createInterface"],
                ),
                "node:tty" => (
                    "globalThis.__apps.tty",
                    vec!["isatty", "ReadStream", "WriteStream"],
                ),
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
                        "unescapeBuffer",
                    ],
                ),
                "node:perf_hooks" => (
                    "globalThis.__apps.perf_hooks",
                    vec![
                        "performance",
                        "PerformanceEntry",
                        "PerformanceMark",
                        "PerformanceMeasure",
                        "PerformanceObserver",
                        "PerformanceObserverEntryList",
                        "createHistogram",
                        "monitorEventLoopDelay",
                    ],
                ),
                "node:constants" => ("globalThis.__apps.constants", vec![]),
                _ => bail!("Unsupported built-in module: {specifier}"),
            };
            // WARNING: Node's require(esm) returns the 'module.exports' export, so CommonJS gets the module object itself.
            let source = format!(
                "const api={object};export default api;export{{api as 'module.exports'}};{}",
                names
                    .iter()
                    .map(|name| format!("export const {name}=api.{name};"))
                    .collect::<String>()
            );
            return Ok((path, Some(source)));
        }
        ensure!(
            !specifier.contains("://") && !specifier.starts_with("//"),
            "Only local ESM imports are supported: {specifier}"
        );
        let cache = self.shared.cache.clone();
        let resolution = (self.realm, base.to_owned(), specifier.to_owned(), require);
        let cached = cache
            .as_ref()
            .and_then(|cache| cache.resolutions.borrow().get(&resolution).cloned());
        if let Some((target, root, manifest)) = cached {
            self.package_roots.extend(root);
            self.files.borrow_mut().extend(manifest);
            return Ok((target, None));
        }
        if specifier.starts_with('.') || Path::new(specifier).is_absolute() {
            let path = base.join(specifier);
            let package = path
                .components()
                .any(|part| part.as_os_str() == "node_modules");
            let path = resolve_file(&path)?;
            let root = path.parent().filter(|_| package).map(Path::to_owned);
            self.package_roots.extend(root.clone());
            if let Some(cache) = cache {
                cache
                    .resolutions
                    .borrow_mut()
                    .insert(resolution, (path.clone(), root, None));
            }
            return Ok((path, None));
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
        let cached = self.shared.manifests.borrow().get(&package_file).cloned();
        let metadata = match cached {
            Some(metadata) => metadata,
            None => {
                self.files.borrow_mut().insert(package_file.clone());
                let metadata = Rc::new(
                    serde_json::from_str::<Value>(
                        &fs::read_to_string(&package_file)
                            .with_context(|| format!("Cannot read {}", package_file.display()))?,
                    )
                    .with_context(|| {
                        format!("Invalid package metadata in {}", package_file.display())
                    })?,
                );
                self.shared
                    .manifests
                    .borrow_mut()
                    .insert(package_file.clone(), metadata.clone());
                metadata
            }
        };
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
            let conditions: &[&str] = match (self.realm == Realm::Renderer, require) {
                (true, true) => &["browser", "require", "default"],
                (true, false) => &["browser", "import", "default"],
                (false, true) => &["node", "require", "module-sync", "node-addons", "default"],
                (false, false) => &["node", "import", "module-sync", "node-addons", "default"],
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
            // WARNING: Node ignores `module`. Like esbuild, node realms prefer `main` and require() never uses `module`.
            let fields: &[&str] = match (self.realm == Realm::Renderer, require) {
                (true, true) => &["browser", "main"],
                (true, false) => &["browser", "module", "main"],
                (false, true) => &["main"],
                (false, false) => &["main", "module"],
            };
            fields
                .iter()
                .find_map(|field| metadata.get(*field).and_then(Value::as_str))
                .unwrap_or("index.js")
                .to_owned()
        };
        let target = resolve_file(&directory.join(entry))?;
        ensure!(
            target.starts_with(&directory),
            "Package export escapes its directory"
        );
        if let Some(cache) = cache {
            cache.resolutions.borrow_mut().insert(
                resolution,
                (
                    target.clone(),
                    (!imports).then_some(directory),
                    Some(package_file),
                ),
            );
        }
        Ok((target, None))
    }

    fn visit(&mut self, path: PathBuf, inline: Option<String>) -> Result<usize> {
        let inline_html = inline.is_some() && path.extension().is_some_and(|e| e == "html");
        let key = if inline_html {
            self.inlined += 1;
            let mut key = path.clone().into_os_string();
            key.push(format!("#{}", self.inlined));
            PathBuf::from(key)
        } else {
            path.clone()
        };
        if let Some(&id) = self.ids.get(&key) {
            return Ok(id);
        }
        let next = self.graph.ids.len();
        let id = *self.graph.ids.entry(key.clone()).or_insert(next);
        self.ids.insert(key, id);
        if self.lines.len() <= id {
            self.lines.resize(id + 1, None);
            self.modules.resize(id + 1, Value::Null);
        }
        let dependency = path.components().any(|p| p.as_os_str() == "node_modules")
            || self.package_roots.iter().any(|root| path.starts_with(root));
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
            let module = json!({"id": id, "code": format!("module.exports=globalThis.__appsLoadAddon({});", serde_json::to_string(&name)?), "deps": [], "imports": [], "exports": [], "stars": [], "commonjs": true});
            if self.production {
                self.modules[id] = module;
            } else {
                self.lines[id] = Some(format!("$appsDefine({});\n", ascii_json(&module)?).into());
            }
            return Ok(id);
        }
        let stamp = (self.shared.cache.is_some() && inline.is_none())
            .then(|| fs::metadata(&path).ok())
            .flatten()
            .and_then(|metadata| Some((metadata.modified().ok()?, metadata.len())));
        let cached = self
            .graph
            .modules
            .get(&id)
            .filter(|cached| Some(cached.stamp) == stamp)
            .map(|cached| (cached.line.clone(), cached.requests.clone()));
        if let Some((line, requests)) = cached {
            self.files.borrow_mut().insert(path.clone());
            let mut valid = true;
            for (base, specifier, require, dep) in &requests {
                if self.resolve(base, specifier, *require)? != *dep {
                    valid = false;
                    break;
                }
            }
            if valid {
                self.lines[id] = Some(line);
                return Ok(id);
            }
        }
        let outer = std::mem::take(&mut self.requests);
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
                        serde_json::to_string(&html(
                            &path,
                            self.files.clone(),
                            self.shared.clone(),
                            self.production
                        )?)?
                    )
                }
                "json" => {
                    let value: Value = serde_json::from_str(&fs::read_to_string(&path)?)
                        .with_context(|| format!("Invalid JSON in {}", path.display()))?;
                    format!(
                        "export default JSON.parse({});",
                        serde_json::to_string(&value.to_string())?
                    )
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
                || ["require", "module", "exports", "__dirname", "__filename"]
                    .iter()
                    .any(|name| references.contains_key(*name)));
        // WARNING: direct `eval` needs import names in scope, so only those modules keep the `with` scope.
        let linked = !commonjs && !references.contains_key("eval");
        let mut asynchronous = TopLevelAwait(false, false);
        asynchronous.visit_program(&program);
        let mut linker = Linker {
            builder: self,
            allocator: &allocator,
            ast: AstBuilder::new(&allocator),
            scoping: &transformed.scoping,
            path: &path,
            commonjs,
            cacheable: stamp.is_some() && !path.extension().is_some_and(|e| e == "html"),
            imported: Vec::new(),
            required: Vec::new(),
            resolved: serde_json::Map::new(),
            error: None,
        };
        linker.visit_program(&mut program);
        if let Some(error) = linker.error {
            return Err(error);
        }
        let (cacheable, imported, required, resolved) = (
            linker.cacheable,
            linker.imported,
            linker.required,
            linker.resolved,
        );
        let ast = AstBuilder::new(&allocator);
        let mut imports = Vec::new();
        let mut exports = Vec::new();
        let mut stars = Vec::new();
        let mut bindings = HashMap::new();
        let mut aliases = Vec::new();
        let mut body = ArenaVec::new_in(&ast);
        for statement in program.body {
            match statement {
                Statement::ImportDeclaration(import) => {
                    let base = path.parent().unwrap_or(Path::new("."));
                    let specifier = import
                        .source
                        .value
                        .as_str()
                        .context("Invalid module path")?;
                    let dep = self.resolve(base, specifier, false)?;
                    self.requests
                        .push((base.to_owned(), specifier.to_owned(), false, dep));
                    imports.push(json!({"id":dep,"local":null,"name":"*"}));
                    let source = specifier.to_owned();
                    for specifier in import.unbox().specifiers.into_iter().flatten() {
                        let symbol = specifier.local().symbol_id();
                        if types.contains(&symbol) {
                            continue;
                        }
                        let local = specifier.local().name.to_string();
                        let name = match specifier {
                            ImportDeclarationSpecifier::ImportSpecifier(specifier) => {
                                let name = specifier.imported.name().to_string();
                                ensure!(
                                    !matches!(source.as_str(), "electron" | "@pierre/apps")
                                        || ELECTRON.contains(&name.as_str()),
                                    "{source}.{name} is not supported (imported in {})",
                                    path.display()
                                );
                                name
                            }
                            ImportDeclarationSpecifier::ImportDefaultSpecifier(_) => {
                                "default".into()
                            }
                            ImportDeclarationSpecifier::ImportNamespaceSpecifier(_) => "*".into(),
                        };
                        bindings.insert(symbol, (dep, name.clone()));
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
                        if let ModuleExportName::IdentifierReference(ident) = &specifier.local
                            && let Some(symbol) = ident.reference_id.get().and_then(|reference| {
                                transformed.scoping.get_reference(reference).symbol_id()
                            })
                        {
                            aliases.push((exports.len(), symbol));
                        }
                        exports.push(json!({"name":specifier.exported.name().as_str(),"value":specifier.local.name().as_str()}));
                    }
                }
                Statement::ExportFromDeclaration(export) => {
                    let base = path.parent().unwrap_or(Path::new("."));
                    let specifier = export
                        .source
                        .value
                        .as_str()
                        .context("Invalid module path")?;
                    let dep = self.resolve(base, specifier, false)?;
                    self.requests
                        .push((base.to_owned(), specifier.to_owned(), false, dep));
                    imports.push(json!({"id":dep,"local":null,"name":"*"}));
                    for specifier in &export.specifiers {
                        let local = format!("$appsRe{}", imports.len());
                        let name = specifier.local.name();
                        imports.push(json!({"id":dep,"local":local,"name":name.as_str()}));
                        let value = if linked {
                            namespace_access(dep, &name)
                        } else {
                            local
                        };
                        exports
                            .push(json!({"name":specifier.exported.name().as_str(),"value":value}));
                    }
                }
                Statement::ExportAllDeclaration(export) => {
                    let base = path.parent().unwrap_or(Path::new("."));
                    let specifier = export
                        .source
                        .value
                        .as_str()
                        .context("Invalid module path")?;
                    let dep = self.resolve(base, specifier, false)?;
                    self.requests
                        .push((base.to_owned(), specifier.to_owned(), false, dep));
                    if let Some(name) = &export.exported {
                        let local = format!("$appsRe{}", imports.len());
                        imports.push(json!({"id":dep,"local":local,"name":"*"}));
                        let value = if linked {
                            namespace_access(dep, "*")
                        } else {
                            local
                        };
                        exports.push(json!({"name":name.name().as_str(),"value":value}));
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
        // WARNING: `for await` keeps an async generator, whose body starts one microtask after `load`.
        let asynchronous = if !asynchronous.0 && !asynchronous.1 {
            0
        } else if asynchronous.1 {
            2
        } else {
            1
        };
        if asynchronous == 1 {
            let mut awaits = Awaits(AstBuilder::new(&allocator));
            for statement in body.iter_mut() {
                awaits.visit_statement(statement);
            }
        }
        if linked {
            let mut imports = Imports {
                ast: AstBuilder::new(&allocator),
                allocator: &allocator,
                scoping: &transformed.scoping,
                bindings: &bindings,
            };
            for statement in body.iter_mut() {
                imports.visit_statement(statement);
            }
            for (index, symbol) in aliases {
                if let Some((dep, name)) = bindings.get(&symbol) {
                    exports[index]["value"] = namespace_access(*dep, name).into();
                }
            }
        }
        program.body = body;
        program.hashbang = None;
        let output = Codegen::new()
            .with_options(CodegenOptions {
                ascii_only: true,
                source_map_path: (!self.production).then(|| path.clone()),
                ..CodegenOptions::default()
            })
            .build(&program);
        let mut deps = Vec::new();
        for dep in imports
            .iter()
            .filter_map(|item| item["id"].as_u64())
            .chain(stars.iter().map(|&dep| dep as u64))
        {
            if !deps.contains(&dep) {
                deps.push(dep);
            }
        }
        let mut module = json!({"id":id,"deps":deps,"code":output.code,"imports":imports,"exports":exports,"stars":stars,"commonjs":commonjs,"linked":linked,"async":asynchronous});
        if commonjs {
            module["file"] = self.runtime_path(&path).into();
            if !resolved.is_empty() {
                module["resolved"] = resolved.into();
            }
        }
        if let Some(map) = output.map {
            let mut map: Value = serde_json::from_str(&map.to_json_string())?;
            // WARNING: The runtime adds one line before the module body.
            map["mappings"] = format!(";{}", map["mappings"].as_str().unwrap()).into();
            module["map"] = map;
            module["url"] = url::Url::from_file_path(std::path::absolute(&path)?)
                .map(|url| Value::String(url.into()))
                .unwrap_or_else(|_| Value::String(path.to_string_lossy().into()));
        }
        let requests = std::mem::replace(&mut self.requests, outer);
        if self.production {
            let manifests = self.shared.manifests.borrow();
            let package = path
                .ancestors()
                .skip(1)
                .find_map(|root| Some((root, manifests.get(&root.join("package.json"))?)));
            module["pure"] = match package.map(|(root, manifest)| (root, &manifest["sideEffects"]))
            {
                Some((_, Value::Bool(false))) => true,
                Some((root, Value::Array(patterns))) => {
                    let relative = path
                        .strip_prefix(root)?
                        .to_string_lossy()
                        .replace('\\', "/");
                    !patterns.iter().filter_map(Value::as_str).any(|pattern| {
                        let pattern = pattern.trim_start_matches("./");
                        if pattern.contains('/') {
                            glob(pattern.as_bytes(), relative.as_bytes())
                        } else {
                            glob(format!("**/{pattern}").as_bytes(), relative.as_bytes())
                        }
                    })
                }
                _ => false,
            }
            .into();
            drop(manifests);
            module["imported"] = json!(imported);
            module["required"] = json!(required);
            self.modules[id] = module;
        } else {
            let line: Rc<str> = format!("$appsDefine({});\n", ascii_json(&module)?).into();
            if cacheable && let Some(stamp) = stamp {
                self.graph.modules.insert(
                    id,
                    Cached {
                        stamp,
                        line: line.clone(),
                        requests,
                    },
                );
            }
            self.lines[id] = Some(line);
        }
        Ok(id)
    }
}

fn app_relative<'a>(root: &Path, path: &'a Path) -> &'a Path {
    path.strip_prefix(root)
        .ok()
        .or_else(|| {
            path.ancestors()
                .find(|directory| directory.ends_with("node_modules"))
                .and_then(Path::parent)
                .and_then(|base| path.strip_prefix(base).ok())
        })
        .or_else(|| path.file_name().map(Path::new))
        .unwrap_or(Path::new(""))
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
        [c, rest @ ..] => text.first() == Some(c) && glob(rest, &text[1..]),
    }
}

// WARNING: JSC keeps a whole script as UTF-16 when it has one non-ASCII character.
fn ascii_json(value: &Value) -> Result<String> {
    let json = serde_json::to_string(value)?;
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
    Ok(result)
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
    let shared = Rc::new(Shared {
        root: root.canonicalize().unwrap_or_else(|_| root.to_owned()),
        cache,
        ..Shared::default()
    });
    let mut output = bundle(
        vec![(entry.to_owned(), inline)],
        realm,
        files,
        shared.clone(),
        production,
    )?;
    for (index, code) in shared.scripts.take().into_iter().enumerate() {
        output.insert(format!("script-{index}.js"), code);
    }
    Ok(output)
}

fn bundle(
    entries: Vec<Entry>,
    realm: Realm,
    files: Rc<RefCell<BTreeSet<PathBuf>>>,
    shared: Rc<Shared>,
    production: bool,
) -> Result<BBTreeMap<String, String>> {
    let key = (realm, entries[0].0.clone());
    let graph = shared
        .cache
        .as_ref()
        .and_then(|cache| cache.graphs.borrow_mut().remove(&key));
    let mut builder = Builder {
        realm,
        production,
        modules: Vec::new(),
        lines: Vec::new(),
        ids: BBTreeMap::new(),
        graph: graph.unwrap_or_default(),
        requests: Vec::new(),
        inlined: 0,
        package_roots: BTreeSet::new(),
        dynamic: BTreeSet::new(),
        assets: BBTreeMap::new(),
        files,
        shared,
    };
    let result = entries
        .into_iter()
        .map(|(path, inline)| builder.visit(path, inline))
        .collect::<Result<Vec<_>>>();
    if let Some(cache) = &builder.shared.cache {
        cache
            .graphs
            .borrow_mut()
            .insert(key.clone(), std::mem::take(&mut builder.graph));
    }
    let entries = result?;
    let mut chunked = serde_json::Map::new();
    if production {
        let modules = &builder.modules;
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
                    "$appsFactory=function(exports,require,module,__filename,__dirname,$appsRequire,$appsImport){{\n{}\n}};",
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
                let params = if module["linked"] == true {
                    let deps = module["deps"].as_array().into_iter().flatten();
                    deps.fold(String::from(",$appsImport"), |params, dep| {
                        params + ",$appsN" + &dep.to_string()
                    })
                } else {
                    String::new()
                };
                format!(
                    "$appsFactory=function*($out,$appsBind{params}){{'use strict';$appsBind($out,{{{getters}}});yield;\n{}\n}};",
                    module["code"].as_str().unwrap()
                )
            };
            if module["async"] == 2 {
                source = source.replacen("=function*(", "=async function*(", 1);
            }
            let allocator = Allocator::default();
            let mut parsed = Parser::new(&allocator, &source, SourceType::cjs()).parse();
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
            let linked = module["linked"] == true;
            let mut uses = Uses(Vec::new());
            if linked {
                uses.visit_program(&parsed.program);
                for (target, name) in &uses.0 {
                    match name {
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
            for import in module["imports"].as_array().unwrap() {
                let Some(local) = import["local"].as_str() else {
                    continue;
                };
                let target = import["id"].as_u64().unwrap() as usize;
                let name = import["name"].as_str().unwrap();
                if linked {
                    if uses.0.iter().any(|(dep, used)| {
                        *dep == target
                            && (name == "*" || used.as_deref().is_none_or(|used| used == name))
                    }) {
                        used_imports[id].insert(local.to_owned());
                    }
                } else if eval || references.contains_key(local) {
                    used_imports[id].insert(local.to_owned());
                    if name == "*" {
                        whole[target] = true;
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
                    ascii_only: true,
                    comments: CommentOptions::disabled(),
                    ..CodegenOptions::default()
                })
                .with_scoping(result.scoping)
                .with_private_member_mappings(result.class_private_mappings);
            codegen.print_expression(&assignment.right);
            factories[id] = codegen.into_source_text();
        }
        // WARNING: a module that package.json marks free of side effects is dropped when nothing reads it, and so is
        // everything only it imports, like other bundlers. Its importers get `null` for its namespace.
        let pruned: Vec<_> = (0..modules.len())
            .map(|id| {
                modules[id]["pure"] == true
                    && needed[id].is_empty()
                    && !whole[id]
                    && !dynamic_stars[id]
                    && !entries.contains(&id)
                    && !builder.dynamic.contains(&id)
            })
            .collect();
        let ids = |module: &Value, field: &str| {
            module[field]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_u64)
                .map(|id| id as usize)
                .collect::<Vec<_>>()
        };
        let mut compiled = vec![false; modules.len()];
        for (id, module) in modules.iter().enumerate().filter(|&(id, _)| !pruned[id]) {
            let mut line = format!("$appsDefine({{id:{id}");
            let deps: Vec<_> = ids(module, "deps")
                .into_iter()
                .map(|dep| (!pruned[dep]).then_some(dep))
                .collect();
            let stars: Vec<_> = ids(module, "stars")
                .into_iter()
                .filter(|&star| !pruned[star])
                .collect();
            if !deps.is_empty() {
                let _ = write!(line, ",deps:{}", json!(deps));
            }
            if !stars.is_empty() {
                let _ = write!(line, ",stars:{}", json!(stars));
            }
            if module["commonjs"] == true {
                line.push_str(",commonjs:1");
                for field in ["file", "resolved"] {
                    if !module[field].is_null() {
                        let _ = write!(line, ",{field}:{}", ascii_json(&module[field])?);
                    }
                }
            }
            if module["async"] != 0 {
                let _ = write!(line, ",async:{}", module["async"]);
            }
            let mut factory = std::mem::take(&mut factories[id]);
            if module["linked"] == false && module["commonjs"] == false {
                let imports: Vec<_> = module["imports"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|import| {
                        import["local"]
                            .as_str()
                            .is_some_and(|local| used_imports[id].contains(local))
                            && !pruned[import["id"].as_u64().unwrap() as usize]
                    })
                    .collect();
                let _ = write!(
                    line,
                    ",linked:false,imports:{}",
                    ascii_json(&json!(imports))?
                );
                factory = format!("function($scope){{with($scope)return {factory}}}");
            }
            // WARNING: codegen cannot escape some text, like raw template strings. Those modules compile from a string, so the bundle stays ASCII.
            if factory.is_ascii() {
                let _ = writeln!(line, ",factory:{factory}}});");
            } else {
                compiled[id] = true;
                let _ = writeln!(line, ",source:{}}});", ascii_json(&Value::String(factory))?);
            }
            builder.lines[id] = Some(line.into());
        }
        // WARNING: modules only reachable through `import()` go to chunk scripts that the loader runs on first use. Pages
        // compile strings only while their own script runs, so modules that compile from a string stay in the bundle.
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
                stack.extend(
                    ids(&modules[id], "deps")
                        .into_iter()
                        .chain(ids(&modules[id], "required")),
                );
                groups.extend(
                    ids(&modules[id], "imported")
                        .into_iter()
                        .map(|target| vec![target]),
                );
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
            let mut scripts = builder.shared.scripts.borrow_mut();
            scripts.push(source);
            chunked.insert(format!("script-{}.js", scripts.len() - 1), json!(members));
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
            + 32,
    );
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
    let mut files = BBTreeMap::from([("app.js".into(), app)]);
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
        let root = key.1.parent().context("Missing entry directory")?;
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
        let source = package.to_string();
        let (name, product_name, version) = apps_runtime::bundle::package_metadata(Some(&source))?;
        apps_runtime::bundle::disabled_features(Some(&source))?;
        let mut metadata = json!({"name":name,"productName":product_name,"version":version});
        if let Some(features) = package.get("features") {
            metadata["features"] = features.clone();
        }
        files.insert("package.json".into(), metadata.to_string());
    }
    Ok(files)
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
        if let Ok(source) = fs::read_to_string(path.join("package.json"))
            && let Ok(metadata) = serde_json::from_str::<Value>(&source)
            && let Some(entry) = ["module", "main"]
                .iter()
                .find_map(|key| metadata.get(key).and_then(Value::as_str))
            && !matches!(entry.trim_end_matches('/'), "" | ".")
            && let Ok(target) = resolve_file(&path.join(entry))
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

fn asset_name(path: &Path) -> Option<String> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
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

fn html(
    path: &Path,
    files: Rc<RefCell<BTreeSet<PathBuf>>>,
    shared: Rc<Shared>,
    production: bool,
) -> Result<String> {
    let mut doc = Document::parse(&fs::read_to_string(path)?);
    let parent = path.parent().context("Missing HTML directory")?;
    let mut scripts: [Vec<(usize, Entry)>; 2] = Default::default();
    for index in 0..doc.nodes.len() {
        let text = matches!(doc.nodes[index].tag.as_str(), "style" | "script")
            .then(|| doc.text(index))
            .unwrap_or_default();
        let child = doc.nodes.len();
        let node = &mut doc.nodes[index];
        if let Some(style) = node.attrs.get_mut("style") {
            *style = urls(style, parent, &files);
        }
        let references: &[&str] = match node.tag.as_str() {
            "img" | "audio" | "source" => &["src"],
            "video" => &["src", "poster"],
            _ => &[],
        };
        for name in references {
            if let Some(source) = node.attrs.get_mut(*name)
                && let Some(url) = asset(parent, source, &files)
            {
                *source = url;
            }
        }
        let mut sheet = None;
        if node.tag == "style" {
            sheet = Some(css(path, Some(text), &mut Vec::new(), &files)?);
        } else if node.tag == "link" && node.attrs.get("rel").is_some_and(|v| v == "stylesheet") {
            let href = node.attrs.get("href").context("Stylesheet has no href")?;
            ensure!(
                !href.contains("://") && !href.starts_with("//"),
                "Only local CSS is supported"
            );
            let text = css(
                &parent.join(href).canonicalize()?,
                None,
                &mut Vec::new(),
                &files,
            )?;
            sheet = Some(match node.attrs.get("media") {
                Some(media) => format!("@media {media}{{{text}}}"),
                None => text,
            });
            node.tag = "style".into();
            node.attrs.clear();
        } else if node.tag == "script" {
            let preload = match node
                .attrs
                .get("type")
                .map(|kind| kind.trim().to_ascii_lowercase())
                .as_deref()
            {
                None | Some("" | "module" | "text/javascript" | "application/javascript") => false,
                Some("preload") => true,
                _ => continue,
            };
            let entry = match node.attrs.remove("src") {
                Some(src) => {
                    ensure!(
                        !src.contains("://") && !src.starts_with("//"),
                        "Only local scripts are supported"
                    );
                    (parent.join(src).canonicalize()?, None)
                }
                None => (path.to_owned(), Some(text)),
            };
            node.children.clear();
            node.attrs.insert(
                "type".into(),
                if preload { "preload" } else { "module" }.into(),
            );
            scripts[usize::from(preload)].push((index, entry));
        }
        if let Some(sheet) = sheet {
            node.children = vec![child];
            let mut text = apps_runtime::dom::Node::default();
            text.tag = "#text".into();
            text.text = sheet;
            text.parent = Some(index);
            doc.nodes.push(text);
        }
    }
    for (realm, list) in [Realm::Renderer, Realm::Preload].into_iter().zip(scripts) {
        let Some(&(first, _)) = list.first() else {
            continue;
        };
        let entries = list.into_iter().map(|(_, entry)| entry).collect();
        let code = bundle_source(
            &bundle(entries, realm, files.clone(), shared.clone(), production)?,
            true,
        );
        let mut scripts = shared.scripts.borrow_mut();
        scripts.push(code);
        doc.nodes[first].attrs.insert(
            "src".into(),
            format!("apps:///script-{}.js", scripts.len() - 1),
        );
    }
    Ok(doc.serialize(0))
}

/// Resolves `@import "package"` to the stylesheet of an npm package, like postcss-import.
fn css_package(directory: &Path, name: &str) -> Result<PathBuf> {
    let count = if name.starts_with('@') { 2 } else { 1 };
    let package = name.split('/').take(count).collect::<Vec<_>>().join("/");
    let subpath = name[package.len()..].trim_start_matches('/');
    let root = directory
        .ancestors()
        .map(|directory| directory.join("node_modules").join(&package))
        .find(|path| path.is_dir())
        .with_context(|| format!("Cannot resolve CSS import {name}"))?;
    if !subpath.is_empty() {
        return Ok(root.join(subpath));
    }
    let metadata: Value = serde_json::from_str(&fs::read_to_string(root.join("package.json"))?)?;
    let entry = metadata["exports"]["."]["style"]
        .as_str()
        .or_else(|| metadata["style"].as_str())
        .or_else(|| {
            metadata["main"]
                .as_str()
                .filter(|main| main.ends_with(".css"))
        })
        .unwrap_or("index.css");
    Ok(root.join(entry))
}

fn css(
    path: &Path,
    inline: Option<String>,
    stack: &mut Vec<PathBuf>,
    files: &Rc<RefCell<BTreeSet<PathBuf>>>,
) -> Result<String> {
    ensure!(
        !stack.contains(&path.to_owned()),
        "CSS import cycle: {}",
        path.display()
    );
    stack.push(path.to_owned());
    let source = match inline {
        Some(source) => source,
        None => {
            files.borrow_mut().insert(path.to_owned());
            ensure!(
                path.extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("css")),
                "Stylesheets must be .css files: {}",
                path.display()
            );
            fs::read_to_string(path)?
        }
    };
    let source = urls(
        &source,
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
            } else if depth == 0
                && bytes[pos..]
                    .get(..7)
                    .is_some_and(|name| name.eq_ignore_ascii_case(b"@import"))
            {
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
        let (spec, options) = match spec.strip_prefix("url(") {
            Some(inner) => {
                let close = inner.find(')').context("Expected a closing parenthesis")?;
                let target = inner[..close].trim();
                let target = if target.starts_with(['\'', '"']) {
                    target
                } else {
                    &spec[4..4 + close]
                };
                (target, inner[close + 1..].trim())
            }
            None => match spec.as_bytes().first() {
                Some(&quote @ (b'\'' | b'"')) => {
                    let close = spec[1..]
                        .find(quote as char)
                        .context("Expected a string literal")?;
                    (&spec[..close + 2], spec[close + 2..].trim())
                }
                _ => (spec, ""),
            },
        };
        let spec = if spec.starts_with(['\'', '"']) {
            spec
        } else {
            &format!("'{spec}'")
        };
        ensure!(
            spec.len() >= 2 && spec.starts_with(['\'', '"']) && spec.ends_with(&spec[..1]),
            "Expected a string literal"
        );
        let name = &spec[1..spec.len() - 1];
        ensure!(
            !name.contains('\\'),
            "Escaped CSS imports are not supported"
        );
        if name == "tailwindcss"
            || name.strip_prefix("tailwindcss/").is_some_and(|part| {
                matches!(
                    part.strip_suffix(".css").unwrap_or(part),
                    "index" | "theme" | "preflight" | "utilities"
                )
            })
        {
            let _ = write!(result, "@import \"{name}\";");
            rest = &rest[end + 1..];
            continue;
        }
        ensure!(
            !name.contains("://") && !name.starts_with("//"),
            "Remote CSS is not supported"
        );
        let directory = path.parent().unwrap();
        let target = if name.starts_with('.') || Path::new(name).is_absolute() {
            directory.join(name)
        } else {
            css_package(directory, name)?
        };
        let mut imported = css(&target.canonicalize()?, None, stack, files)?;
        let mut options = options;
        let starts = |text: &str, word: &str| {
            text.get(..word.len())
                .is_some_and(|start| start.eq_ignore_ascii_case(word))
        };
        let group = |text: &str| -> Option<usize> {
            let mut depth = 0;
            for (index, c) in text.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' if depth == 1 => return Some(index),
                    ')' => depth -= 1,
                    _ => {}
                }
            }
            None
        };
        if starts(options, "layer(") {
            let close = group(options).context("Expected a closing parenthesis")?;
            imported = format!("@layer {}{{{imported}}}", options[6..close].trim());
            options = options[close + 1..].trim();
        } else if starts(options, "layer")
            && !options[5..].starts_with(|c: char| c.is_alphanumeric() || c == '-')
        {
            imported = format!("@layer{{{imported}}}");
            options = options[5..].trim();
        }
        if starts(options, "supports(") {
            let close = group(options).context("Expected a closing parenthesis")?;
            imported = format!("@supports ({}){{{imported}}}", options[9..close].trim());
            options = options[close + 1..].trim();
        }
        if !options.is_empty() {
            imported = format!("@media {options}{{{imported}}}");
        }
        result.push_str(&imported);
        rest = &rest[end + 1..];
    }
    result.push_str(rest);
    ensure!(
        !result.to_ascii_lowercase().contains("</style"),
        "CSS cannot contain </style>: {}",
        path.display()
    );
    stack.pop();
    Ok(result)
}

#[cfg(test)]
mod tests;
