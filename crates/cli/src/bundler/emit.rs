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
    pub(super) fn code(&self) -> String {
        match self {
            Export::Local(name) => name.clone(),
            Export::Reexport(dep, name) => namespace_access(*dep, name),
        }
    }
}

impl Import {
    pub(super) fn json(&self) -> Value {
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
    pub(super) ast: AstBuilder<'a>,
    pub(super) allocator: &'a Allocator,
    pub(super) origin: &'b dyn Fn(usize, &str) -> (usize, String),
    pub(super) flat: &'b [bool],
    pub(super) slots: &'b HashMap<(usize, String), (usize, bool)>,
    pub(super) namespaces: BTreeSet<usize>,
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

pub(super) struct Uses(pub(super) Vec<(usize, Option<String>)>);

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
