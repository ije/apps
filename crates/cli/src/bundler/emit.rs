use super::*;

#[derive(Default)]
pub(super) struct Module {
    pub(super) code: String,
    pub(super) deps: Vec<usize>,
    pub(super) imports: Vec<Import>,
    pub(super) exports: Vec<(String, Export)>,
    pub(super) stars: Vec<usize>,
    pub(super) live: BTreeSet<String>,
    pub(super) imported: Vec<usize>,
    pub(super) required: Vec<usize>,
    pub(super) defers: Vec<usize>,
    pub(super) file: Option<String>,
    pub(super) resolved: serde_json::Map<String, Value>,
    pub(super) map: Option<Value>,
    pub(super) url: Option<String>,
    pub(super) commonjs: bool,
    pub(super) linked: bool,
    pub(super) preload: bool,
    pub(super) pure: bool,
    pub(super) builtin: bool,
    pub(super) asynchronous: u8,
}

pub(super) struct Import {
    pub(super) dep: usize,
    pub(super) local: Option<String>,
    pub(super) name: String,
}

pub(super) enum Export {
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
    pub(super) fn line(&self, id: usize) -> String {
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
        if !self.defers.is_empty() {
            module["defers"] = json!(self.defers);
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
        format!("$appsDefine({});\n", ascii_json(&module))
    }
}

pub(super) fn namespace_id(name: &str) -> Option<usize> {
    name.strip_prefix("$appsN")?.parse().ok()
}

// `(0, read)` keeps `this` undefined when an imported function is called.
pub(super) fn unbound<'a>(
    ast: &AstBuilder<'a>,
    span: Span,
    read: Expression<'a>,
) -> Expression<'a> {
    let zero = Expression::new_numeric_literal(span, 0., None, NumberBase::Decimal, ast);
    Expression::new_sequence_expression(span, ArenaVec::from_array_in([zero, read], ast), ast)
}

pub(super) fn namespace_access(dep: usize, name: &str) -> String {
    match name {
        "*" => format!("$appsN{dep}"),
        _ if identifier(name) => format!("$appsN{dep}.{name}"),
        _ => format!("$appsN{dep}[{}]", json!(name)),
    }
}

pub(super) fn namespace_read<'a>(
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
    if !identifier(name) {
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

pub(super) struct Imports<'a, 'b> {
    pub(super) ast: AstBuilder<'a>,
    pub(super) allocator: &'a Allocator,
    pub(super) scoping: &'b oxc_semantic::Scoping,
    pub(super) bindings: &'b HashMap<oxc_semantic::SymbolId, (usize, String)>,
}

impl<'a> Imports<'a, '_> {
    fn binding(&self, ident: &IdentifierReference<'a>) -> Option<&(usize, String)> {
        let reference = ident.reference_id.get()?;
        self.bindings
            .get(&self.scoping.get_reference(reference).symbol_id()?)
    }

    fn callee(&self, expression: &mut Expression<'a>) {
        if let Expression::Identifier(ident) = expression
            && let Some((dep, name)) = self.binding(ident).filter(|(_, name)| name != "*").cloned()
        {
            let span = ident.span;
            let read = namespace_read(&self.ast, self.allocator, span, dep, &name);
            *expression = unbound(&self.ast, span, read);
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

pub(super) struct TopLevelAwait(pub(super) u8);

impl<'a> Visit<'a> for TopLevelAwait {
    fn visit_function(&mut self, _: &Function<'a>, _: oxc_semantic::ScopeFlags) {}
    fn visit_arrow_function_expression(&mut self, _: &ArrowFunctionExpression<'a>) {}
    fn visit_class(&mut self, _: &Class<'a>) {}
    fn visit_await_expression(&mut self, expression: &AwaitExpression<'a>) {
        self.0 = self.0.max(1);
        walk::walk_await_expression(self, expression);
    }
    fn visit_for_of_statement(&mut self, statement: &ForOfStatement<'a>) {
        if statement.r#await {
            self.0 = 2;
        }
        walk::walk_for_of_statement(self, statement);
    }
    fn visit_variable_declaration(&mut self, declaration: &VariableDeclaration<'a>) {
        if declaration.kind == VariableDeclarationKind::AwaitUsing {
            self.0 = 2;
        }
        walk::walk_variable_declaration(self, declaration);
    }
}

pub(super) struct Awaits<'a>(pub(super) AstBuilder<'a>);

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

pub(super) struct Slots<'a, 'b> {
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
            Expression::Identifier(object) => namespace_id(&object.name),
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

    fn callee(&mut self, expression: &mut Expression<'a>) {
        if let Some((dep, name)) = Self::import(expression) {
            let span = expression.span();
            let read = self.read(span, dep, &name);
            *expression = unbound(&self.ast, span, read);
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
        if let Some(dep) = namespace_id(&ident.name) {
            self.namespaces.insert(dep);
        }
    }
}

pub(super) fn in_cycles(modules: &[Module]) -> Vec<bool> {
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

// Imports need live reads for bindings that functions, methods, accessors or instance fields can write after
// module evaluation.
pub(super) struct Writes<'b> {
    pub(super) scoping: &'b oxc_semantic::Scoping,
    pub(super) late: bool,
    pub(super) live: BTreeSet<String>,
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
        // oxc gives field initializers no scope, so only the AST tells an instance field from a static one.
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

pub(super) struct Uses(Vec<(usize, Option<String>)>);

impl<'a> Visit<'a> for Uses {
    fn visit_static_member_expression(&mut self, member: &StaticMemberExpression<'a>) {
        if let Expression::Identifier(object) = &member.object
            && let Some(dep) = namespace_id(&object.name)
        {
            self.0.push((dep, Some(member.property.name.to_string())));
        } else {
            walk::walk_static_member_expression(self, member);
        }
    }

    fn visit_computed_member_expression(&mut self, member: &ComputedMemberExpression<'a>) {
        if let (Expression::Identifier(object), Expression::StringLiteral(key)) =
            (&member.object, &member.expression)
            && let Some(dep) = namespace_id(&object.name)
        {
            self.0.push((dep, key.value.as_str().map(str::to_owned)));
        } else {
            walk::walk_computed_member_expression(self, member);
        }
    }

    fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
        if let Some(dep) = namespace_id(&ident.name) {
            self.0.push((dep, None));
        }
    }
}

pub(super) fn bundle(entries: Vec<Entry>, realm: Realm, shared: Rc<Shared>) -> Result<String> {
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

pub(super) fn factory(program: &Program, result: MinifierReturn, id: usize) -> Result<String> {
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
    Ok(codegen.into_source_text())
}
