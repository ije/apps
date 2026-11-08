use anyhow::{Context, Result, bail, ensure};
use apps_runtime::{
    bundle::script_source,
    dom::Document,
    host::{Role, Waker},
    jsc::{JavaScriptCore, JsContext},
};
use bbtree::unsync::BBTreeMap;
use oxc_allocator::{Allocator, ArenaVec, TakeIn};
use oxc_ast::{ast::*, builder::AstBuilder};
use oxc_ast_visit::{Visit, VisitMut, walk, walk_mut};
use oxc_codegen::{Codegen, CodegenOptions, CommentOptions};
use oxc_minifier::{Minifier, MinifierOptions};
use oxc_parser::Parser;
use oxc_react_compiler::{CompileResult, PluginOptions};
use oxc_semantic::SemanticBuilder;
use oxc_span::{GetSpan, SourceType, Span};
use oxc_transformer::{TransformOptions, Transformer};
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
    production: bool,
    modules: Vec<Module>,
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
    disabled: u32,
    aliases: Option<(PathBuf, Vec<String>)>,
    preload: Option<PathBuf>,
    loaders: Option<Rc<Loaders>>,
    env: Value,
    process_entries: RefCell<BTreeSet<PathBuf>>,
    names: RefCell<HashMap<String, PathBuf>>,
}

struct Loaders {
    modules: Vec<(String, String)>,
    files: BBTreeMap<String, String>,
    engine: JsContext,
    woken: std::sync::mpsc::Receiver<()>,
}

impl Shared {
    fn enabled(&self, feature: &str) -> bool {
        self.disabled & apps_runtime::bundle::feature_bit(feature) == 0
    }

    // WARNING: unlike Node, `imports` keys without `#` in the app's package.json map bare specifiers in every module,
    // dependencies included, like an import map.
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
}

// WARNING: cached modules contain `$appsImport(N)` and `$appsN<id>`, so a graph never reuses or changes module ids.
#[derive(Default)]
pub struct Cache {
    graphs: RefCell<HashMap<(Realm, PathBuf), Graph>>,
    loaders: RefCell<Option<Rc<Loaders>>>,
    disabled: Cell<u32>,
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
    requires: Vec<oxc_semantic::SymbolId>,
    error: Option<anyhow::Error>,
}

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
];

const BUILTINS: &str = include_str!("builtins.txt");
// WARNING: these are built in by their package name, never with `node:`, and only in main.
const PACKAGES: &[&str] = &["@pierre/apps", "electron", "node-pty", "ws", "undici"];

#[derive(Default)]
struct Module {
    code: String,
    deps: Vec<usize>,
    imports: Vec<Import>,
    exports: Vec<(String, Export)>,
    stars: Vec<usize>,
    live: BTreeSet<String>,
    imported: Vec<usize>,
    required: Vec<usize>,
    file: Option<String>,
    resolved: serde_json::Map<String, Value>,
    map: Option<Value>,
    url: Option<String>,
    commonjs: bool,
    linked: bool,
    preload: bool,
    pure: bool,
    asynchronous: u8,
}

struct Import {
    dep: usize,
    local: Option<String>,
    name: String,
}

enum Export {
    Local(String),
    Reexport(usize, String),
}

impl Export {
    fn code(&self) -> String {
        match self {
            Export::Local(name) => name.clone(),
            Export::Reexport(dep, name) => namespace_access(*dep, name),
        }
    }
}

impl Import {
    fn json(&self) -> Value {
        json!({"id": self.dep, "local": self.local, "name": self.name})
    }
}

impl Module {
    fn line(&self, id: usize) -> Result<String> {
        let mut module = json!({
            "id": id,
            "deps": self.deps,
            "code": self.code,
            "imports": self.imports.iter().map(Import::json).collect::<Vec<_>>(),
            "exports": self.exports.iter().map(|(name, export)| json!({"name": name, "value": export.code()})).collect::<Vec<_>>(),
            "stars": self.stars,
            "commonjs": self.commonjs,
            "linked": self.linked,
            "async": self.asynchronous,
        });
        if self.preload {
            module["preload"] = true.into();
        }
        if let Some(file) = &self.file {
            module["file"] = file.as_str().into();
        }
        if !self.resolved.is_empty() {
            module["resolved"] = self.resolved.clone().into();
        }
        if let Some(map) = &self.map {
            module["map"] = map.clone();
        }
        if let Some(url) = &self.url {
            module["url"] = url.as_str().into();
        }
        Ok(format!("$appsDefine({});\n", ascii_json(&module)?))
    }
}

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

fn namespace_read<'a>(
    ast: &AstBuilder<'a>,
    allocator: &'a Allocator,
    span: Span,
    dep: usize,
    name: &str,
) -> Expression<'a> {
    let namespace =
        Expression::new_identifier(span, allocator.alloc_str(&format!("$appsN{dep}")), ast);
    if name == "*" {
        return namespace;
    }
    let property = allocator.alloc_str(name);
    if namespace_access(dep, name).contains('[') {
        Expression::new_computed_member_expression(
            span,
            namespace,
            Expression::new_string_literal(span, property, None, ast),
            false,
            ast,
        )
    } else {
        Expression::new_static_member_expression(
            span,
            namespace,
            IdentifierName::new(span, property, ast),
            false,
            ast,
        )
    }
}

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

    // WARNING: `(0, ns.f)` keeps `this` undefined when an imported function is called.
    fn callee(&self, expression: &mut Expression<'a>) {
        if let Expression::Identifier(ident) = expression
            && let Some((dep, name)) = self.binding(ident).filter(|(_, name)| name != "*").cloned()
        {
            let span = ident.span;
            let zero =
                Expression::new_numeric_literal(span, 0., None, NumberBase::Decimal, &self.ast);
            let read = namespace_read(&self.ast, self.allocator, span, dep, &name);
            *expression = Expression::new_sequence_expression(
                span,
                ArenaVec::from_array_in([zero, read], &self.ast),
                &self.ast,
            );
        }
    }
}

impl<'a> VisitMut<'a> for Imports<'a, '_> {
    fn visit_expression(&mut self, expression: &mut Expression<'a>) {
        if let Expression::Identifier(ident) = expression
            && let Some((dep, name)) = self.binding(ident).cloned()
        {
            *expression = namespace_read(&self.ast, self.allocator, ident.span, dep, &name);
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

// A module's exports are slots in one array of the bundle: it writes them when its body ends, and importers read
// `slots[k]`. So there is no generator, namespace object or getter per export, which took a quarter of the JS heap.
struct Slots<'a, 'b> {
    ast: AstBuilder<'a>,
    allocator: &'a Allocator,
    origin: &'b dyn Fn(usize, &str) -> (usize, String),
    flat: &'b [bool],
    slots: &'b HashMap<(usize, String), (usize, bool)>,
    namespaces: BTreeSet<usize>,
}

impl<'a> Slots<'a, '_> {
    fn import(expression: &Expression<'a>) -> Option<(usize, String)> {
        let dep = |object: &Expression<'a>| match object {
            Expression::Identifier(object) => object.name.strip_prefix("$appsN")?.parse().ok(),
            _ => None,
        };
        match expression {
            Expression::StaticMemberExpression(member) => {
                Some((dep(&member.object)?, member.property.name.to_string()))
            }
            Expression::ComputedMemberExpression(member) => match &member.expression {
                Expression::StringLiteral(key) => {
                    Some((dep(&member.object)?, key.value.as_str()?.to_owned()))
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn read(&mut self, span: Span, dep: usize, name: &str) -> Expression<'a> {
        let (origin, name) = (self.origin)(dep, name);
        if let Some(&(slot, late)) = self.slots.get(&(origin, name.clone())) {
            let read = Expression::new_computed_member_expression(
                span,
                Expression::new_identifier(span, "$appsS", &self.ast),
                Expression::new_numeric_literal(
                    span,
                    slot as f64,
                    None,
                    NumberBase::Decimal,
                    &self.ast,
                ),
                false,
                &self.ast,
            );
            return if late {
                Expression::new_call_expression(
                    span,
                    read,
                    None,
                    ArenaVec::new_in(&self.ast),
                    false,
                    &self.ast,
                )
            } else {
                read
            };
        }
        if self.flat[origin] {
            // WARNING: `ns.missing` on a namespace reads undefined. A named import of it fails at build time.
            return Expression::new_unary_expression(
                span,
                UnaryOperator::Void,
                Expression::new_numeric_literal(span, 0., None, NumberBase::Decimal, &self.ast),
                &self.ast,
            );
        }
        self.namespaces.insert(origin);
        namespace_read(&self.ast, self.allocator, span, origin, &name)
    }

    // WARNING: like other bundlers, a called import gets `this` undefined, also through a namespace. It must not see the slots.
    fn callee(&mut self, expression: &mut Expression<'a>) {
        if let Some((dep, name)) = Self::import(expression) {
            let span = expression.span();
            let read = self.read(span, dep, &name);
            let zero =
                Expression::new_numeric_literal(span, 0., None, NumberBase::Decimal, &self.ast);
            *expression = Expression::new_sequence_expression(
                span,
                ArenaVec::from_array_in([zero, read], &self.ast),
                &self.ast,
            );
        }
    }
}

impl<'a> VisitMut<'a> for Slots<'a, '_> {
    fn visit_expression(&mut self, expression: &mut Expression<'a>) {
        if let Some((dep, name)) = Self::import(expression) {
            *expression = self.read(expression.span(), dep, &name);
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

    fn visit_identifier_reference(&mut self, ident: &mut IdentifierReference<'a>) {
        if let Some(dep) = ident
            .name
            .strip_prefix("$appsN")
            .and_then(|id| id.parse().ok())
        {
            self.namespaces.insert(dep);
        }
    }
}

// Modules in import cycles can read each other's exports before their bodies end, so they keep namespaces.
fn in_cycles(modules: &[Module]) -> Vec<bool> {
    let edges = |id: usize| -> Vec<usize> {
        let module = &modules[id];
        [&module.deps, &module.stars, &module.required]
            .into_iter()
            .flatten()
            .copied()
            .collect()
    };
    let mut cyclic = vec![false; modules.len()];
    let (mut index, mut low, mut on) = (
        vec![usize::MAX; modules.len()],
        vec![0; modules.len()],
        vec![false; modules.len()],
    );
    let (mut stack, mut next) = (Vec::new(), 0);
    for root in 0..modules.len() {
        if index[root] != usize::MAX {
            continue;
        }
        let mut work = vec![(root, edges(root), 0)];
        index[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on[root] = true;
        while let Some((id, deps, at)) = work.last_mut() {
            let id = *id;
            if let Some(&dep) = deps.get(*at) {
                *at += 1;
                cyclic[id] |= dep == id;
                if index[dep] == usize::MAX {
                    index[dep] = next;
                    low[dep] = next;
                    next += 1;
                    stack.push(dep);
                    on[dep] = true;
                    work.push((dep, edges(dep), 0));
                } else if on[dep] {
                    low[id] = low[id].min(index[dep]);
                }
                continue;
            }
            work.pop();
            if let Some((parent, ..)) = work.last() {
                low[*parent] = low[*parent].min(low[id]);
            }
            if low[id] == index[id] {
                let top = stack.iter().rposition(|&item| item == id).unwrap();
                let members = stack.split_off(top);
                let many = members.len() > 1;
                for member in members {
                    on[member] = false;
                    cyclic[member] |= many;
                }
            }
        }
    }
    cyclic
}

// WARNING: root bindings that code can write after the module's body ended must be read live by importers. A write is
// early only while the body runs: in the root statements, their blocks and loops, class static blocks, static fields
// and computed keys. Functions, methods, accessors and instance fields run later.
struct Writes<'b> {
    scoping: &'b oxc_semantic::Scoping,
    late: bool,
    live: BTreeSet<String>,
}

impl Writes<'_> {
    fn later(&mut self, late: bool, visit: impl FnOnce(&mut Self)) {
        let outer = self.late;
        self.late |= late;
        visit(self);
        self.late = outer;
    }
}

impl<'a> Visit<'a> for Writes<'_> {
    fn visit_function(&mut self, function: &Function<'a>, flags: oxc_semantic::ScopeFlags) {
        self.later(true, |this| walk::walk_function(this, function, flags));
    }

    fn visit_arrow_function_expression(&mut self, arrow: &ArrowFunctionExpression<'a>) {
        self.later(true, |this| {
            walk::walk_arrow_function_expression(this, arrow)
        });
    }

    fn visit_class_element(&mut self, element: &ClassElement<'a>) {
        // WARNING: oxc gives field initializers no scope, so only the AST tells an instance field from a static one.
        let late = match element {
            ClassElement::StaticBlock(_) => false,
            ClassElement::PropertyDefinition(property) => !property.r#static,
            ClassElement::AccessorProperty(property) => !property.r#static,
            _ => true,
        };
        match element {
            ClassElement::PropertyDefinition(property) => {
                self.visit_decorators(&property.decorators);
                self.visit_property_key(&property.key);
                if let Some(value) = &property.value {
                    self.later(late, |this| this.visit_expression(value));
                }
            }
            ClassElement::AccessorProperty(property) => {
                self.visit_decorators(&property.decorators);
                self.visit_property_key(&property.key);
                if let Some(value) = &property.value {
                    self.later(late, |this| this.visit_expression(value));
                }
            }
            ClassElement::MethodDefinition(method) => {
                self.visit_decorators(&method.decorators);
                self.visit_property_key(&method.key);
                self.later(true, |this| {
                    this.visit_function(&method.value, oxc_semantic::ScopeFlags::Function)
                });
            }
            element => self.later(late, |this| walk::walk_class_element(this, element)),
        }
    }

    fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
        if let Some(reference) = ident.reference_id.get()
            && let reference = self.scoping.get_reference(reference)
            && reference.is_write()
            && let Some(symbol) = reference.symbol_id()
            && self.scoping.symbol_scope_id(symbol) == self.scoping.root_scope_id()
            && (self.late
                || self
                    .scoping
                    .scope_ancestors(reference.scope_id())
                    .any(|scope| {
                        let flags = self.scoping.scope_flags(scope);
                        flags.is_function() || flags.is_arrow()
                    }))
        {
            self.live
                .insert(self.scoping.symbol_name(symbol).to_owned());
        }
    }
}

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
    fn visit_variable_declarator(&mut self, declarator: &mut VariableDeclarator<'a>) {
        if let (BindingPattern::BindingIdentifier(name), Some(Expression::CallExpression(call))) =
            (&declarator.id, declarator.init.as_ref())
            && self.builder.realm != Realm::Renderer
            && match call.callee.get_inner_expression() {
                Expression::Identifier(callee) => callee.name == "createRequire",
                Expression::StaticMemberExpression(callee) => {
                    callee.property.name == "createRequire"
                }
                _ => false,
            }
        {
            self.requires.push(name.symbol_id());
        }
        walk_mut::walk_variable_declarator(self, declarator);
    }

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
                let path =
                    resolve_file(&self.path.parent().unwrap_or(Path::new(".")).join(specifier))?;
                self.cacheable = false;
                let index = self.builder.script_chunk(path)?;
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
                    &self.builder.shared.root,
                )
            {
                self.cacheable = false;
                let value = self.allocator.alloc_str(&asset);
                url.arguments[0] =
                    Expression::new_string_literal(specifier.span, value, None, &self.ast).into();
            }
            // WARNING: apps often compile ESM to CommonJS, so ESM here also has these two as `import.meta` values.
            if !self.commonjs
                && let Expression::Identifier(name) = expression
                && matches!(name.name.as_str(), "__dirname" | "__filename")
                && self.builder.realm == Realm::Main
                && self
                    .scoping
                    .get_reference(name.reference_id())
                    .symbol_id()
                    .is_none()
            {
                let code = if name.name == "__dirname" {
                    "import.meta.dirname"
                } else {
                    "import.meta.filename"
                };
                *expression = Parser::new(self.allocator, code, SourceType::mjs())
                    .parse_expression()
                    .map_err(|_| anyhow::anyhow!("Cannot rewrite {code}"))?;
                self.visit_expression(expression);
                return Ok(());
            }
            if let Expression::StaticMemberExpression(member) = expression
                && member.property.name == "main"
                && matches!(
                    member.object.get_inner_expression(),
                    Expression::ImportMeta(_)
                )
            {
                self.cacheable = false;
                let main = self
                    .builder
                    .shared
                    .process_entries
                    .borrow()
                    .contains(self.path);
                *expression = Expression::new_boolean_literal(span, main, &self.ast);
                return Ok(());
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
                if let Expression::StaticMemberExpression(env) =
                    member.object.get_inner_expression()
                    && env.property.name == "env"
                    && matches!(env.object.get_inner_expression(), Expression::ImportMeta(_))
                {
                    let code = self.builder.meta_env()[member.property.name.as_str()].to_string();
                    let code = if code == "null" {
                        "void 0".to_owned()
                    } else {
                        code
                    };
                    *expression = Parser::new(
                        self.allocator,
                        self.allocator.alloc_str(&code),
                        SourceType::mjs(),
                    )
                    .parse_expression()
                    .map_err(|_| {
                        anyhow::anyhow!("Cannot rewrite import.meta in {}", self.path.display())
                    })?;
                    return Ok(());
                }
                if matches!(
                    member.object.get_inner_expression(),
                    Expression::ImportMeta(_)
                ) && matches!(
                    member.property.name.as_str(),
                    "url" | "filename" | "dirname"
                ) {
                    let url = if is_remote(self.path) {
                        self.path.to_string_lossy().trim_end_matches('#').to_owned()
                    } else if self.builder.production {
                        let relative = self.builder.app_name(self.path)?;
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
                    // WARNING: like Node, `filename` and `dirname` are paths of file modules, so pages and remote files get undefined.
                    let code = match member.property.name.as_str() {
                        "url" => {
                            let value = self.allocator.alloc_str(&url);
                            *expression =
                                Expression::new_string_literal(span, value, None, &self.ast);
                            return Ok(());
                        }
                        _ if self.builder.realm == Realm::Renderer || is_remote(self.path) => {
                            "void 0".to_owned()
                        }
                        "filename" => {
                            format!("globalThis.__apps.url.fileURLToPath({})", json!(url))
                        }
                        _ => format!(
                            "globalThis.__apps.path.dirname(globalThis.__apps.url.fileURLToPath({}))",
                            json!(url)
                        ),
                    };
                    *expression = Parser::new(
                        self.allocator,
                        self.allocator.alloc_str(&code),
                        SourceType::mjs(),
                    )
                    .parse_expression()
                    .map_err(|_| {
                        anyhow::anyhow!("Cannot rewrite import.meta in {}", self.path.display())
                    })?;
                    return Ok(());
                }
                ensure!(
                    self.commonjs
                        || member.property.name != "exports"
                        || !matches!(&member.object, Expression::Identifier(module) if module.name == "module"
                            && self.scoping.get_reference(module.reference_id()).symbol_id().is_none()),
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
                    "import.meta.resolve needs a local path"
                );
                let target = self.path.parent().unwrap_or(Path::new(".")).join(specifier);
                // WARNING: a script becomes a chunk that `ELECTRON_RUN_AS_NODE` can run in a child process.
                if !target.extension().is_some_and(|e| e == "html") {
                    let target = target
                        .canonicalize()
                        .with_context(|| format!("Cannot resolve {}", target.display()))?;
                    self.cacheable = false;
                    self.builder
                        .shared
                        .process_entries
                        .borrow_mut()
                        .insert(target.clone());
                    let index = self.builder.script_chunk(target)?;
                    let value = self
                        .allocator
                        .alloc_str(&format!("apps:///script-{index}.js"));
                    *expression = Expression::new_string_literal(span, value, None, &self.ast);
                    return Ok(());
                }
                let target = target
                    .canonicalize()
                    .with_context(|| format!("Cannot resolve {}", target.display()))?;
                let value = self.page(target)?;
                *expression = Expression::new_string_literal(span, value, None, &self.ast);
                return Ok(());
            }
            // WARNING: only the eager `?url` form that Vite apps use for assets. It reads the folder of each pattern at build time.
            if let Expression::CallExpression(call) = expression
                && let Expression::StaticMemberExpression(member) =
                    call.callee.get_inner_expression()
                && matches!(
                    member.object.get_inner_expression(),
                    Expression::ImportMeta(_)
                )
                && member.property.name == "glob"
            {
                let Some(Argument::StringLiteral(pattern)) = call.arguments.first() else {
                    bail!(
                        "import.meta.glob needs one string literal in {}",
                        self.path.display()
                    );
                };
                let base = self.path.parent().unwrap_or(Path::new(".")).to_owned();
                let mut urls = serde_json::Map::new();
                let pattern = pattern
                    .value
                    .as_str()
                    .context("Invalid import.meta.glob pattern")?;
                for pattern in expand_braces(pattern) {
                    let (folder, name) = pattern.rsplit_once('/').unwrap_or((".", &pattern));
                    let Ok(entries) = fs::read_dir(base.join(folder)) else {
                        continue;
                    };
                    let mut names: Vec<_> = entries
                        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
                        .filter(|file| glob(name.as_bytes(), file.as_bytes()))
                        .collect();
                    names.sort();
                    for file in names {
                        let key = format!("{folder}/{file}");
                        let path = base.join(&key).canonicalize()?;
                        if let Some(url) = self.builder.url_of(&path)? {
                            urls.insert(key, url.into());
                        }
                    }
                }
                self.cacheable = false;
                let code = format!("({})", Value::Object(urls));
                *expression = Parser::new(
                    self.allocator,
                    self.allocator.alloc_str(&code),
                    SourceType::mjs(),
                )
                .parse_expression()
                .map_err(|_| {
                    anyhow::anyhow!("Cannot rewrite import.meta.glob in {}", self.path.display())
                })?;
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
                    let path = self.builder.runtime_path(&path)?;
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
                    match import.source.get_inner_expression() {
                        Expression::StringLiteral(specifier)
                            if !specifier.value.starts_with("data:") =>
                        {
                            Some((
                                "$appsImport",
                                specifier.value.as_str().context("Invalid module path")?,
                            ))
                        }
                        // WARNING: other specifiers stay native import() calls, which the runtime loads only with these features.
                        source => {
                            let (data, shared) = (source.is_string_literal(), &self.builder.shared);
                            let feature = if data {
                                "jsExecute"
                            } else {
                                "jsExecute or httpImport"
                            };
                            let enabled = shared.enabled("jsExecute")
                                || (!data && shared.enabled("httpImport"));
                            // WARNING: packages often keep such an import for a path that the app never takes, so there it fails when it runs.
                            if !enabled
                                && self
                                    .path
                                    .components()
                                    .any(|part| part.as_os_str() == "node_modules")
                            {
                                let code = format!(
                                    "Promise.reject(new Error({}))",
                                    json!(format!(
                                        "import() without a string literal needs the {feature} feature"
                                    ))
                                );
                                *expression = Parser::new(
                                    self.allocator,
                                    self.allocator.alloc_str(&code),
                                    SourceType::mjs(),
                                )
                                .parse_expression()
                                .map_err(|_| {
                                    anyhow::anyhow!(
                                        "Cannot rewrite import() in {}",
                                        self.path.display()
                                    )
                                })?;
                                return Ok(());
                            }
                            ensure!(
                                enabled,
                                "import() of a data: URL or without a string literal needs the {feature} feature in {}",
                                self.path.display()
                            );
                            None
                        }
                    }
                }
                Expression::CallExpression(call)
                    if matches!(call.callee.get_inner_expression(), Expression::Identifier(callee)
                        if self.scoping.get_reference(callee.reference_id()).symbol_id().is_some_and(|symbol| self.requires.contains(&symbol))) =>
                {
                    match call.arguments.as_slice() {
                        [Argument::StringLiteral(specifier)] => specifier
                            .value
                            .as_str()
                            .map(|specifier| ("$appsRequire", specifier)),
                        _ => None,
                    }
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
                // WARNING: `url`, `filename`, `dirname` and `resolve` are static above, so other reads see only `env`.
                Expression::ImportMeta(_) => {
                    let code = format!("({{env:{}}})", self.builder.meta_env());
                    *expression = Parser::new(
                        self.allocator,
                        self.allocator.alloc_str(&code),
                        SourceType::mjs(),
                    )
                    .parse_expression()
                    .map_err(|_| {
                        anyhow::anyhow!("Cannot rewrite import.meta in {}", self.path.display())
                    })?;
                    return Ok(());
                }
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
    fn script_chunk(&mut self, path: PathBuf) -> Result<usize> {
        let realm = if self.realm == Realm::Renderer {
            Realm::Renderer
        } else {
            Realm::Main
        };
        let key = (path, realm);
        let shared = self.shared.clone();
        if let Some(index) = shared.workers.borrow().get(&key).copied() {
            return Ok(index);
        }
        let index = shared.scripts.borrow().len();
        shared.scripts.borrow_mut().push(String::new());
        shared.workers.borrow_mut().insert(key.clone(), index);
        let files = bundle(
            vec![(key.0, None)],
            key.1,
            self.files.clone(),
            shared.clone(),
            self.production,
        )?;
        shared.scripts.borrow_mut()[index] = script_source(&files, true);
        Ok(index)
    }

    fn url_of(&mut self, path: &Path) -> Result<Option<String>> {
        if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("js" | "mjs" | "cjs" | "ts")
        ) {
            let index = self.script_chunk(path.to_owned())?;
            return Ok(Some(format!("apps:///script-{index}.js")));
        }
        let Some(name) = asset_name(&self.shared.root, path) else {
            return Ok(None);
        };
        self.files.borrow_mut().insert(path.to_owned());
        Ok(Some(format!("apps:///{name}")))
    }

    fn meta_env(&self) -> Value {
        let mut env = json!({
            "MODE": if self.production { "production" } else { "development" },
            "DEV": !self.production,
            "PROD": self.production,
            "SSR": false,
            "BASE_URL": "/",
        });
        if let (Some(env), Some(values)) = (env.as_object_mut(), self.shared.env.as_object()) {
            env.extend(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        }
        env
    }

    fn resolve(&mut self, base: &Path, specifier: &str, require: bool) -> Result<usize> {
        let (path, inline) = self.locate(base, specifier, require)?;
        self.visit(path, inline)
    }

    fn runtime_path(&self, path: &Path) -> Result<String> {
        if !self.production {
            return Ok(std::path::absolute(path)
                .unwrap_or_else(|_| path.to_owned())
                .to_string_lossy()
                .into_owned());
        }
        Ok(self.app_name(path)?.to_string_lossy().replace('\\', "/"))
    }

    // WARNING: names key `require.cache`, `import.meta.url` and `__filename`, so two files must never share one.
    fn app_name(&self, path: &Path) -> Result<PathBuf> {
        let relative = app_relative(&self.shared.root, path);
        let name = relative.to_string_lossy().replace('\\', "/");
        let mut names = self.shared.names.borrow_mut();
        let previous = names.entry(name.clone()).or_insert_with(|| path.to_owned());
        ensure!(
            previous == path,
            "Output name collision at {name}: {} and {}",
            previous.display(),
            path.display()
        );
        Ok(relative)
    }

    fn locate(
        &mut self,
        base: &Path,
        specifier: &str,
        require: bool,
    ) -> Result<(PathBuf, Option<String>)> {
        // WARNING: like Node, a built-in module without `node:` is built in unless it needs the prefix, and pages take a
        // package of the same name first.
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
        if let Some((path, source)) = remote(base, specifier, self.shared.enabled("httpImport"))? {
            return Ok((path, Some(source)));
        }
        // WARNING: `?url`, `?raw` and `?worker` work as in Vite. The query stays in the path, so the module is not the plain file.
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
                        self.files.borrow_mut().insert(path.clone());
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
            // WARNING: like Node, a package can import itself by name.
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
                .filter(|map| imports || map.keys().any(|k| k.starts_with(['.', '#'])))
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
                    // WARNING: like Node, a matching condition without a target falls back to the next matching key.
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
            let value = if let Some(replacement) = replacement {
                value.replace('*', replacement)
            } else {
                value.to_owned()
            };
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
            // WARNING: Node ignores `module`. Like esbuild, node realms prefer `main` and require() never uses `module`.
            let fields: &[&str] = match (self.realm == Realm::Renderer, require) {
                (true, true) => &["browser", "main"],
                (true, false) => &["browser", "module", "main"],
                (false, true) => &["main"],
                (false, false) => &["main", "module"],
            };
            // WARNING: like Vite, a field that names a missing file falls through to the next one.
            fields
                .iter()
                .find_map(|field| {
                    metadata
                        .get(*field)
                        .and_then(Value::as_str)
                        .filter(|entry| resolve_file(&directory.join(entry)).is_ok())
                })
                .unwrap_or("index.js")
                .to_owned()
        };
        let target = resolve_file(&directory.join(entry))?;
        // WARNING: only package exports stay in their folder. An app's `imports` can name a symlinked package in node_modules.
        ensure!(
            imports || target.starts_with(&directory),
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
            self.modules.resize_with(id + 1, Module::default);
        }
        if self.realm == Realm::Renderer && self.shared.preload.as_ref() == Some(&path) {
            let module = Module {
                preload: true,
                linked: true,
                ..Module::default()
            };
            if self.production {
                self.modules[id] = module;
            } else {
                self.lines[id] = Some(module.line(id)?.into());
            }
            return Ok(id);
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
            let module = Module {
                code: format!(
                    "module.exports=globalThis.__appsLoadAddon({});",
                    json!(name)
                ),
                commonjs: true,
                ..Module::default()
            };
            if self.production {
                self.modules[id] = module;
            } else {
                self.lines[id] = Some(module.line(id)?.into());
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
        let mut loaded = None;
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let loader = self
            .shared
            .loaders
            .clone()
            .filter(|_| inline.is_none())
            .and_then(|loaders| {
                let (index, (_, module)) = loaders
                    .modules
                    .iter()
                    .enumerate()
                    .filter(|(_, (suffix, _))| name.ends_with(suffix.as_str()))
                    .max_by_key(|(_, (suffix, _))| suffix.len())?;
                Some((index, module.clone(), loaders))
            });
        let source = if let Some(source) = inline {
            source
        } else if let Some((index, module, loaders)) = loader {
            self.files.borrow_mut().insert(path.clone());
            let (code, source_type, map) = (|| -> Result<_> {
                let content = fs::read_to_string(&path)?;
                let args = json!({"filename": path.to_string_lossy(), "content": content});
                loaders
                    .engine
                    .call("__appsLoad", &[&index.to_string(), &args.to_string()])?;
                let mut delay = 0;
                let result = loop {
                    let result = loaders.engine.call("__appsLoaded", &[])?;
                    if result != "undefined" {
                        break result;
                    }
                    if delay < 0 {
                        ensure!(
                            loaders.engine.call("__appsAlive", &[])? == "true",
                            "load() did not settle"
                        );
                        let _ = loaders.woken.recv();
                    } else if delay > 0 {
                        let _ = loaders
                            .woken
                            .recv_timeout(std::time::Duration::from_millis(delay as u64));
                    }
                    delay = loaders.engine.tick()?;
                };
                let mut result: Value = serde_json::from_str(&result)?;
                if let Some(error) = result["error"].as_str() {
                    bail!("{error}");
                }
                let value = result["value"].take();
                let source_type = match value["type"].as_str() {
                    Some("js") => SourceType::mjs(),
                    Some("jsx") => SourceType::jsx(),
                    Some("ts") => SourceType::ts(),
                    Some("tsx") => SourceType::tsx(),
                    _ => bail!("load() must return a type of js, jsx, ts or tsx"),
                };
                let code = value["code"]
                    .as_str()
                    .context("load() must return code as a string")?;
                let map = match &value["map"] {
                    Value::Null => None,
                    Value::String(map) => Some(map.clone()),
                    map => Some(map.to_string()),
                };
                Ok((code.to_owned(), source_type, map))
            })()
            .with_context(|| format!("Loader {module} failed on {}", path.display()))?;
            loaded = Some((source_type, map));
            code
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
                // WARNING: like Vite, importing CSS applies it. Tailwind's `@import` stays for the runtime compiler.
                "css" if self.realm == Realm::Renderer => {
                    let text = json!(css(
                        &path,
                        None,
                        &mut Vec::new(),
                        &self.files,
                        &self.shared
                    )?);
                    format!(
                        "const sheet=document.createElement('style');sheet.textContent={text};document.head.append(sheet);export default {text};"
                    )
                }
                "css" => "export default '';".to_owned(),
                "json" => {
                    let value: Value = serde_json::from_str(&fs::read_to_string(&path)?)
                        .with_context(|| format!("Invalid JSON in {}", path.display()))?;
                    format!(
                        "export default JSON.parse({});",
                        serde_json::to_string(&value.to_string())?
                    )
                }
                // WARNING: like Vite, importing an asset gives its URL.
                _ if asset_name(&self.shared.root, &path).is_some() => format!(
                    "export default {};",
                    json!(format!(
                        "apps:///{}",
                        asset_name(&self.shared.root, &path).unwrap()
                    ))
                ),
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
        } else if let Some((source_type, _)) = loaded {
            source_type
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
        let react =
            self.realm == Realm::Renderer && !dependency && self.shared.enabled("reactCompiler");
        let semantic = SemanticBuilder::new()
            .with_check_syntax_error(true)
            .with_enum_eval(true)
            .with_build_nodes(react)
            .build(&program);
        ensure!(
            semantic.diagnostics.is_empty(),
            "Invalid source in {}: {:?}",
            path.display(),
            semantic.diagnostics
        );
        // WARNING: the React Compiler must run on the program as parsed, before any other transform.
        let compiled = react.then(|| {
            oxc_react_compiler::compile(
                &program,
                &semantic.semantic,
                &allocator,
                PluginOptions::default(),
            )
        });
        let mut scoping = semantic.semantic.into_scoping();
        match compiled {
            Some(CompileResult::Success {
                output: Some(output),
                ..
            }) => {
                output.transform(&mut program);
                scoping = SemanticBuilder::new()
                    .with_enum_eval(true)
                    .build(&program)
                    .semantic
                    .into_scoping();
            }
            Some(CompileResult::Fatal { diagnostics }) => bail!(
                "Cannot compile React in {}: {diagnostics:?}",
                path.display()
            ),
            _ => {}
        }
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
            requires: Vec::new(),
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
                    imports.push(Import {
                        dep,
                        local: None,
                        name: "*".into(),
                    });
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
                        imports.push(Import {
                            dep,
                            local: Some(local),
                            name,
                        });
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
                        exports.push((name.clone(), Export::Local(name)));
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
                        exports.push((
                            specifier.exported.name().to_string(),
                            Export::Local(specifier.local.name().to_string()),
                        ));
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
                    imports.push(Import {
                        dep,
                        local: None,
                        name: "*".into(),
                    });
                    for specifier in &export.specifiers {
                        let local = format!("$appsRe{}", imports.len());
                        let name = specifier.local.name().to_string();
                        let value = if linked {
                            Export::Reexport(dep, name.clone())
                        } else {
                            Export::Local(local.clone())
                        };
                        imports.push(Import {
                            dep,
                            local: Some(local),
                            name,
                        });
                        exports.push((specifier.exported.name().to_string(), value));
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
                        let value = if linked {
                            Export::Reexport(dep, "*".into())
                        } else {
                            Export::Local(local.clone())
                        };
                        imports.push(Import {
                            dep,
                            local: Some(local),
                            name: "*".into(),
                        });
                        exports.push((name.name().to_string(), value));
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
                            exports.push((
                                "default".into(),
                                Export::Local(function.id.as_ref().unwrap().name.to_string()),
                            ));
                            body.push(Statement::FunctionDeclaration(function));
                            continue;
                        }
                        ExportDefaultDeclarationKind::ClassDeclaration(mut class) => {
                            if class.id.is_none() {
                                class.id = Some(BindingIdentifier::new(span, "$appsDefault", &ast));
                            }
                            exports.push((
                                "default".into(),
                                Export::Local(class.id.as_ref().unwrap().name.to_string()),
                            ));
                            body.push(Statement::ClassDeclaration(class));
                            continue;
                        }
                        declaration => declaration.into_expression(),
                    };
                    exports.push(("default".into(), Export::Local("$appsDefault".into())));
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
                    exports[index].1 = Export::Reexport(*dep, name.clone());
                }
            }
        }
        program.body = body;
        program.hashbang = None;
        let mut live = BTreeSet::new();
        if self.production && linked {
            let mut writes = Writes {
                scoping: &transformed.scoping,
                late: false,
                live: BTreeSet::new(),
            };
            writes.visit_program(&program);
            live = writes.live;
        }
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
            .map(|import| import.dep)
            .chain(stars.iter().copied())
        {
            if !deps.contains(&dep) {
                deps.push(dep);
            }
        }
        let mut module = Module {
            code: output.code,
            deps,
            imports,
            exports,
            stars,
            live,
            imported,
            required,
            commonjs,
            linked,
            asynchronous,
            ..Module::default()
        };
        if commonjs {
            module.file = Some(self.runtime_path(&path)?);
            module.resolved = resolved;
        }
        if let Some(map) = output.map {
            let map = match loaded.as_ref().and_then(|(_, map)| map.as_deref()) {
                Some(input) => map
                    .compose(
                        oxc_sourcemap::SourceMap::from_json_string(input)
                            .map_err(|error| anyhow::anyhow!("{error:?}"))
                            .with_context(|| {
                                format!("Invalid source map from the loader of {}", path.display())
                            })?,
                    )
                    .to_json_string(),
                None => map.to_json_string(),
            };
            let mut map: Value = serde_json::from_str(&map)?;
            // WARNING: The runtime adds one line before the module body.
            map["mappings"] = format!(";{}", map["mappings"].as_str().unwrap()).into();
            module.map = Some(map);
            module.url = Some(if is_remote(&path) {
                path.to_string_lossy().trim_end_matches('#').into()
            } else {
                url::Url::from_file_path(std::path::absolute(&path)?)
                    .map(String::from)
                    .unwrap_or_else(|_| path.to_string_lossy().into())
            });
        }
        let requests = std::mem::replace(&mut self.requests, outer);
        if self.production {
            let manifests = self.shared.manifests.borrow();
            let package = path
                .ancestors()
                .skip(1)
                .find_map(|root| Some((root, manifests.get(&root.join("package.json"))?)));
            module.pure = match package.map(|(root, manifest)| (root, &manifest["sideEffects"])) {
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
            };
            drop(manifests);
            self.modules[id] = module;
        } else {
            let line: Rc<str> = module.line(id)?.into();
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

// WARNING: files outside the app, like packages in a parent node_modules, keep their path from the app folder, so two
// files never share a name. Only a file on another Windows drive gets its drive as the first folder.
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
        [c, rest @ ..] => text.first() == Some(c) && glob(rest, &text[1..]),
    }
}

// WARNING: JSC keeps a whole script as UTF-16 when it has one non-ASCII character.
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
    let root = root.canonicalize().unwrap_or_else(|_| root.to_owned());
    let manifest = root
        .ancestors()
        .map(|directory| directory.join("package.json"))
        .find(|path| path.is_file());
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
            resolve_file(&manifest.as_ref().unwrap().parent().unwrap().join(value))
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
    // WARNING: compiled components import react/compiler-runtime, which React has since 19. Experimental builds are 0.0.0.
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
        // WARNING: the entry is inline, so its path only names it and must not be a file that a loader imports.
        let entry = manifest
            .as_ref()
            .unwrap()
            .with_file_name("package.json#loaders");
        let shared = Rc::new(Shared {
            disabled,
            aliases: aliases.clone(),
            root: root.clone(),
            cache: cache.clone(),
            ..Shared::default()
        });
        let mut output = bundle(
            vec![(entry, Some(code))],
            Realm::Main,
            files.clone(),
            shared.clone(),
            false,
        )?;
        for (index, code) in shared.scripts.take().into_iter().enumerate() {
            output.insert(format!("script-{index}.js"), code);
        }
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
    if let Some(cache) = &cache {
        let previous = cache.loaders.replace(loaders.clone());
        // WARNING: cached modules keep the output of the loaders that compiled them.
        if previous.as_ref().map(Rc::as_ptr) != loaders.as_ref().map(Rc::as_ptr) {
            cache
                .graphs
                .borrow_mut()
                .retain(|(_, entry), _| entry.ends_with("package.json#loaders"));
        }
        // WARNING: cached modules also keep the output of the features that compiled them, like reactCompiler.
        if cache.disabled.replace(disabled) != disabled {
            cache.graphs.borrow_mut().clear();
        }
    }
    let shared = Rc::new(Shared {
        disabled,
        aliases,
        preload,
        loaders,
        env: metadata.get("env").cloned().unwrap_or_default(),
        root,
        cache,
        ..Shared::default()
    });
    shared
        .process_entries
        .borrow_mut()
        .extend(entry.canonicalize());
    let mut output = if rust {
        let mut output = BBTreeMap::new();
        let defaults = [json!("index.html")];
        let pages = match metadata.get("pages") {
            Some(value) => value
                .as_array()
                .context("package.json pages must be an array")?
                .as_slice(),
            None => {
                let index = shared.root.join("index.html");
                files.borrow_mut().insert(index.clone());
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
            let path = shared.root.join(relative);
            files.borrow_mut().insert(path.clone());
            ensure!(
                path.canonicalize()
                    .with_context(|| format!("Cannot find page {name:?}"))?
                    .starts_with(&shared.root),
                "Page outside app: {name}"
            );
            output.insert(
                name.into(),
                html(&path, files.clone(), shared.clone(), production)?,
            );
        }
        collect_files(
            &mut output,
            &mut files.borrow_mut(),
            &shared.root,
            Some(&shared.root),
        )?;
        output
    } else {
        bundle(
            vec![(entry.to_owned(), inline)],
            realm,
            files,
            shared.clone(),
            production,
        )?
    };
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
    let mut classic = false;
    let result = entries
        .into_iter()
        .map(|(path, inline)| {
            let Some(name) = path.to_str().and_then(|path| path.strip_prefix("apps:///")) else {
                return builder.visit(path, inline);
            };
            // WARNING: a classic script runs as its own bundled script through `__appsChunk`, so it gets the global
            // scope, sloppy mode and the window as `this`. Its entry module only runs it in order. The name ends
            // with `#` and the position of its `<script>` element.
            let (name, element) = name.split_once('#').context("Missing script element")?;
            let code = format!("$appsClassic({},{element})", json!(name));
            classic = true;
            let next = builder.graph.ids.len();
            let id = *builder.graph.ids.entry(path.clone()).or_insert(next);
            builder.ids.insert(path, id);
            if builder.lines.len() <= id {
                builder.lines.resize(id + 1, None);
                builder.modules.resize_with(id + 1, Module::default);
            }
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
            for import in &module.imports {
                ensure!(
                    import.local.is_none()
                        || import.name == "*"
                        || dynamic_stars[import.dep]
                        || available[import.dep].contains(&import.name),
                    "Missing export {} from {}",
                    import.name,
                    import.dep
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
        // Follows re-exports and `export *` to the module that holds a binding, so barrels cost nothing at run time.
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
                "Invalid JavaScript in module {id} ({:?}): {:?}",
                module.url,
                parsed.diagnostics
            );
            let result =
                Minifier::new(MinifierOptions::default()).minify(&allocator, &mut parsed.program);
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
            // WARNING: a flat module is printed after linking, so here its minified code only tells what it reads.
            if flat[id] {
                continue;
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
        // WARNING: reads skip a barrel, but a barrel that was read through still runs its imports with side effects.
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
                "Invalid JavaScript in module {id} ({:?}): {:?}",
                module.url,
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
            // WARNING: the linked body moves into the factory as AST, so it is not printed and parsed again.
            let factory = allocator.alloc_str(&format!(
                "$appsFactory=function($appsS,$appsImport,$appsRequire{params}){{'use strict';}};"
            ));
            let mut program = Parser::new(&allocator, factory, SourceType::cjs())
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
            let result = Minifier::new(MinifierOptions::default()).minify(&allocator, &mut program);
            // WARNING: the linker also saw reads in dead code, of modules that may be pruned, so only what the
            // minifier kept gets an argument. Parameters stay in place, so the others get nothing.
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
            let Some(Statement::ExpressionStatement(statement)) = program.body.first() else {
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
        // WARNING: an import that skips a pruned barrel still needs its origin to run, in the barrel's place.
        let linked_deps = |id: usize| -> Vec<usize> {
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
        };
        let mut compiled = vec![false; modules.len()];
        for (id, module) in modules.iter().enumerate().filter(|&(id, _)| !pruned[id]) {
            let mut line = format!("$appsDefine({{id:{id}");
            let deps: Vec<_> = if flat[id] {
                linked_deps(id).into_iter().map(Some).collect()
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
                    let _ = write!(line, ",slots:{}", ascii_json(&Value::Object(table))?);
                }
            }
            if !stars.is_empty() {
                let _ = write!(line, ",stars:{}", json!(stars));
            }
            if module.preload {
                line.push_str(",preload:1");
            }
            if module.commonjs {
                line.push_str(",commonjs:1");
                if let Some(file) = &module.file {
                    let _ = write!(line, ",file:{}", ascii_json(&json!(file))?);
                }
                if !module.resolved.is_empty() {
                    let _ = write!(
                        line,
                        ",resolved:{}",
                        ascii_json(&Value::Object(module.resolved.clone()))?
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
                    ascii_json(&json!(imports))?
                );
                factory = format!("function($scope){{with($scope)return {factory}}}");
            } else if realm == Realm::Renderer && builder.shared.preload.is_some() && module.linked
            {
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
                    let _ = write!(line, ",imports:{}", ascii_json(&json!(imports))?);
                }
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
                if flat[id] {
                    stack.extend(linked_deps(id));
                }
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
    let mut files = BBTreeMap::from([("app.js".into(), app)]);
    for (_, (name, source)) in builder.assets {
        files.insert(name, source);
    }
    let main = (realm == Realm::Main).then_some(key.1.parent()).flatten();
    collect_files(
        &mut files,
        &mut builder.files.borrow_mut(),
        &builder.shared.root,
        main,
    )?;
    Ok(files)
}

fn collect_files(
    files: &mut BBTreeMap<String, String>,
    sources: &mut BTreeSet<PathBuf>,
    root: &Path,
    main: Option<&Path>,
) -> Result<()> {
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
        if let Some(name) = asset_name(root, path)
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
                            if apps_runtime::bundle::package_metadata(Some(&candidate.to_string()))
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

fn asset_name(root: &Path, path: &Path) -> Option<String> {
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
            | "wasm"
            | "glb"
            | "bcmap"
            | "pfb"
    ) {
        return None;
    }
    // WARNING: names hash the path in the app with FNV-1a, so builds of the same app match on every machine.
    let hash = app_relative(root, path)
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

thread_local! {
    // WARNING: downloads stay for the process, so `apps run` rebuilds don't fetch remote files again.
    static DOWNLOADS: RefCell<HashMap<String, String>> = RefCell::default();
}

fn is_remote(path: &Path) -> bool {
    path.to_str()
        .is_some_and(|path| path.starts_with("https://") || path.starts_with("http://"))
}

fn remote_base(directory: &Path) -> Option<url::Url> {
    is_remote(directory)
        .then(|| url::Url::parse(&format!("{}/", directory.to_str()?)).ok())
        .flatten()
}

fn remote(directory: &Path, reference: &str, http: bool) -> Result<Option<(PathBuf, String)>> {
    let mut url = match remote_base(directory) {
        Some(base) => {
            ensure!(
                reference.contains(':') || reference.starts_with(['.', '/']),
                "Bare import {reference:?} is not supported in remote files: {base}"
            );
            base.join(reference)?
        }
        None if reference.contains("://") || reference.starts_with("//") => {
            url::Url::parse(reference).with_context(|| format!("Invalid URL: {reference}"))?
        }
        None => return Ok(None),
    };
    ensure!(
        matches!(url.scheme(), "http" | "https"),
        "Only local files and HTTP URLs can be imported: {url}"
    );
    ensure!(
        http,
        "Importing {url} needs the httpImport feature. Add \"httpImport\" to \"features\" in package.json"
    );
    url.set_fragment(None);
    let source = match DOWNLOADS.with_borrow(|downloads| downloads.get(url.as_str()).cloned()) {
        Some(source) => source,
        None => {
            let source = String::from_utf8(apps_runtime::download(url.as_str())?)
                .with_context(|| format!("{url} is not UTF-8"))?;
            DOWNLOADS
                .with_borrow_mut(|downloads| downloads.insert(url.to_string(), source.clone()));
            source
        }
    };
    let mut path = url.to_string();
    // WARNING: `parent()` drops a trailing slash, so a directory URL gets an empty fragment.
    if url.path().ends_with('/') && url.query().is_none() {
        path.push('#');
    }
    Ok(Some((PathBuf::from(path), source)))
}

fn asset(
    directory: &Path,
    reference: &str,
    files: &Rc<RefCell<BTreeSet<PathBuf>>>,
    root: &Path,
) -> Option<String> {
    let reference = reference.trim().trim_matches(['"', '\'']);
    if reference.contains(':') || reference.starts_with('#') {
        return None;
    }
    if let Some(base) = remote_base(directory) {
        return base.join(reference).ok().map(String::from);
    }
    // WARNING: like browsers, `\` is `/`, so `\x.png` is in the app folder too.
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
    let name = asset_name(root, &path)?;
    files.borrow_mut().insert(path);
    Some(format!("apps:///{name}"))
}

fn urls(
    source: &str,
    directory: &Path,
    files: &Rc<RefCell<BTreeSet<PathBuf>>>,
    root: &Path,
) -> String {
    let bytes = source.as_bytes();
    let string = |start: usize| {
        let mut end = start + 1;
        while end < bytes.len() && bytes[end] != bytes[start] && bytes[end] != b'\n' {
            end += if bytes[end] == b'\\' { 2 } else { 1 };
        }
        (end + 1).min(bytes.len())
    };
    let (mut result, mut copied, mut pos) = (String::with_capacity(source.len()), 0, 0);
    // WARNING: comments and strings are skipped, so a `url(` in them never takes the next `url()`.
    while pos < bytes.len() {
        if bytes[pos..].starts_with(b"/*") {
            pos = source[pos + 2..]
                .find("*/")
                .map_or(bytes.len(), |end| pos + end + 4);
        } else if matches!(bytes[pos], b'"' | b'\'') {
            pos = string(pos);
        } else if bytes[pos..].len() >= 4 && bytes[pos..pos + 4].eq_ignore_ascii_case(b"url(") {
            let start = pos + 4;
            let mut end = start;
            while end < bytes.len() && bytes[end].is_ascii_whitespace() {
                end += 1;
            }
            if end < bytes.len() && matches!(bytes[end], b'"' | b'\'') {
                end = string(end);
            }
            pos = source[end..]
                .find(')')
                .map_or(bytes.len(), |close| end + close);
            if let Some(url) = asset(directory, &source[start..pos], files, root) {
                result.push_str(&source[copied..start]);
                result.push('"');
                result.push_str(&url);
                result.push('"');
                copied = pos;
            }
        } else {
            pos += 1;
        }
    }
    result.push_str(&source[copied..]);
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
    let http = shared.enabled("httpImport");
    // WARNING: browsers run classic scripts without defer or async while they parse, before any module, so they go first.
    let mut blocking = Vec::new();
    let mut scripts: [Vec<(usize, Entry)>; 2] = Default::default();
    for index in 0..doc.nodes.len() {
        let text = matches!(doc.nodes[index].tag.as_str(), "style" | "script")
            .then(|| doc.text(index))
            .unwrap_or_default();
        let child = doc.nodes.len();
        let node = &mut doc.nodes[index];
        if let Some(style) = node.attrs.get_mut("style") {
            *style = urls(style, parent, &files, &shared.root);
        }
        let references: &[&str] = match node.tag.as_str() {
            "img" | "audio" | "source" => &["src"],
            "video" => &["src", "poster"],
            _ => &[],
        };
        for name in references {
            if let Some(source) = node.attrs.get_mut(name)
                && let Some(url) = asset(parent, source, &files, &shared.root)
            {
                *source = url;
            }
        }
        if matches!(node.tag.as_str(), "img" | "source")
            && let Some(srcset) = node.attrs.get_mut("srcset")
        {
            let mut result = String::with_capacity(srcset.len());
            let mut rest = srcset.as_str();
            while let Some(start) = rest.find(|c: char| !c.is_ascii_whitespace() && c != ',') {
                let candidate = &rest[start..];
                let end = candidate
                    .find(|c: char| c.is_ascii_whitespace())
                    .unwrap_or(candidate.len());
                let url = candidate[..end].trim_end_matches(',');
                result.push_str(&rest[..start]);
                result.push_str(
                    &asset(parent, url, &files, &shared.root).unwrap_or_else(|| url.into()),
                );
                let descriptors = &candidate[url.len()..];
                let next = if url.len() < end {
                    0
                } else {
                    descriptors
                        .find(',')
                        .map_or(descriptors.len(), |comma| comma + 1)
                };
                result.push_str(&descriptors[..next]);
                rest = &descriptors[next..];
            }
            result.push_str(rest);
            *srcset = result;
        }
        let mut sheet = None;
        if node.tag == "style" {
            sheet = Some(css(path, Some(text), &mut Vec::new(), &files, &shared)?);
        } else if node.tag == "link"
            && node.attrs.get("rel").is_some_and(|rel| {
                let mut tokens = rel.split_ascii_whitespace();
                tokens
                    .clone()
                    .any(|token| token.eq_ignore_ascii_case("stylesheet"))
                    && !tokens.any(|token| token.eq_ignore_ascii_case("alternate"))
            })
        {
            let href = node.attrs.get("href").context("Stylesheet has no href")?;
            let text = match remote(parent, href, http)? {
                Some((path, text)) => css(&path, Some(text), &mut Vec::new(), &files, &shared)?,
                None => css(
                    &local(&shared.root, parent, href)?,
                    None,
                    &mut Vec::new(),
                    &files,
                    &shared,
                )?,
            };
            // WARNING: the link stays like in browsers, and the page loader moves its CSS out of this attribute.
            node.attrs
                .insert(apps_runtime::bundle::LINK_SHEET.into(), text);
        } else if node.tag == "script" {
            let (classic, preload) = match node
                .attrs
                .get("type")
                .map(|kind| kind.trim().to_ascii_lowercase())
                .as_deref()
            {
                None | Some("" | "text/javascript" | "application/javascript") => (true, false),
                Some("module") => (false, false),
                Some("preload") => (false, true),
                _ => continue,
            };
            // WARNING: page scripts keep `src` and `type` like in browsers. Without text, the runtime skips them.
            let src = if preload {
                node.attrs.remove("src")
            } else {
                node.attrs.get("src").cloned()
            };
            node.children.clear();
            // WARNING: like browsers with modules, `nomodule` scripts never run.
            if classic && node.attrs.contains_key("nomodule") {
                continue;
            }
            let deferred = src.is_some()
                && (node.attrs.contains_key("defer") || node.attrs.contains_key("async"));
            let entry = match src {
                Some(src) => match remote(parent, &src, http)? {
                    Some((path, source)) => (path, Some(source)),
                    None => (local(&shared.root, parent, &src)?, None),
                },
                None => (path.to_owned(), Some(text)),
            };
            if !classic {
                scripts[usize::from(preload)].push((index, entry));
                continue;
            }
            let source = match entry {
                (_, Some(source)) => source,
                (file, None) => {
                    let source = fs::read_to_string(&file)
                        .with_context(|| format!("Cannot read {}", file.display()))?;
                    files.borrow_mut().insert(file);
                    source
                }
            };
            let mut list = shared.scripts.borrow_mut();
            list.push(source);
            let entry = (
                PathBuf::from(format!("apps:///script-{}.js", list.len() - 1)),
                None,
            );
            if deferred {
                scripts[0].push((index, entry));
            } else {
                blocking.push((index, entry));
            }
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
    scripts[0].splice(0..0, blocking);
    if let Some(preload) = &shared.preload {
        let parent = doc
            .nodes
            .iter()
            .position(|node| node.tag == "head")
            .unwrap_or(0);
        let index = doc.nodes.len();
        let mut node = apps_runtime::dom::Node::default();
        node.tag = "script".into();
        node.attrs.insert("type".into(), "preload".into());
        node.parent = Some(parent);
        doc.nodes.push(node);
        doc.nodes[parent].children.insert(0, index);
        scripts[1].insert(
            0,
            (
                index,
                (
                    PathBuf::from("apps:preload"),
                    Some(format!(
                        "import * as api from {};globalThis.__appsPublishPreload(api);",
                        json!(preload)
                    )),
                ),
            ),
        );
    }
    for (realm, list) in [Realm::Renderer, Realm::Preload].into_iter().zip(scripts) {
        let Some(&(mut first, _)) = list.first() else {
            continue;
        };
        let mut order = HashMap::new();
        if realm == Realm::Renderer {
            // WARNING: the bundle runs from a new element before the page scripts, which keep their own attributes.
            // A classic script gets the position of its element among all scripts, for `document.currentScript`.
            let parent = doc.nodes[first].parent.unwrap_or(0);
            let mut node = apps_runtime::dom::Node::default();
            node.tag = "script".into();
            node.attrs.insert("type".into(), "module".into());
            node.parent = Some(parent);
            doc.nodes.push(node);
            let at = doc.nodes[parent]
                .children
                .iter()
                .position(|&child| child == first)
                .unwrap_or(0);
            first = doc.nodes.len() - 1;
            doc.nodes[parent].children.insert(at, first);
            let mut pending = vec![0];
            while let Some(id) = pending.pop() {
                if doc.nodes[id].tag == "script" {
                    order.insert(id, order.len());
                }
                pending.extend(doc.nodes[id].children.iter().rev());
            }
        }
        let entries = list
            .into_iter()
            .map(|(node, (path, inline))| match path.to_str() {
                Some(name) if name.starts_with("apps:///") => {
                    (PathBuf::from(format!("{name}#{}", order[&node])), inline)
                }
                _ => (path, inline),
            })
            .collect();
        let code = script_source(
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

fn local(root: &Path, directory: &Path, reference: &str) -> Result<PathBuf> {
    // WARNING: like browsers, `\` is `/`. Like a web server root, `/x` is the app folder, and a path from it or an
    // absolute path, like `C:/x` on Windows, cannot leave it.
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
    shared: &Shared,
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
        &shared.root,
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
        let directory = path.parent().unwrap();
        let mut imported = match remote(directory, name, shared.enabled("httpImport"))? {
            Some((target, text)) => css(&target, Some(text), stack, files, shared)?,
            None => {
                let target = if name.starts_with(['.', '/']) || Path::new(name).is_absolute() {
                    local(&shared.root, directory, name)?
                } else {
                    css_package(directory, name)?.canonicalize()?
                };
                css(&target, None, stack, files, shared)?
            }
        };
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
