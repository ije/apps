use super::*;
use super::{
    html::is_remote,
    resolve::{entry_fields, in_packages, resolve_file},
};

pub(super) struct Linker<'a, 'b> {
    pub(super) builder: &'b mut Builder,
    pub(super) allocator: &'a Allocator,
    pub(super) ast: AstBuilder<'a>,
    pub(super) scoping: &'b oxc_semantic::Scoping,
    pub(super) path: &'b Path,
    pub(super) folder: &'b Path,
    pub(super) commonjs: bool,
    pub(super) cacheable: bool,
    pub(super) imported: Vec<usize>,
    pub(super) required: Vec<usize>,
    pub(super) resolved: serde_json::Map<String, Value>,
    pub(super) requires: Vec<oxc_semantic::SymbolId>,
    pub(super) aliases: Vec<(oxc_semantic::SymbolId, &'static str)>,
    pub(super) error: Option<anyhow::Error>,
}

impl<'a> Linker<'a, '_> {
    fn page(&mut self, target: PathBuf) -> Result<&'a str> {
        self.cacheable = false;
        let name = self.builder.shared.page(target)?;
        Ok(self.allocator.alloc_str(&format!("apps:///{name}")))
    }

    fn global(&self, ident: &IdentifierReference<'a>) -> bool {
        self.scoping
            .get_reference(ident.reference_id())
            .symbol_id()
            .is_none()
    }

    fn parse(&self, code: &str) -> Result<Expression<'a>> {
        Parser::new(
            self.allocator,
            self.allocator.alloc_str(code),
            SourceType::mjs(),
        )
        .parse_expression()
        .map_err(|_| anyhow::anyhow!("Cannot rewrite {code} in {}", self.path.display()))
    }

    fn local_path(&mut self, expression: &Expression<'a>) -> Option<PathBuf> {
        let folder = self.folder.to_owned();
        // A `?worker` stub module has the path of its worker file with the query.
        let file = || {
            self.path
                .to_str()
                .and_then(|path| {
                    path.strip_suffix("?worker")
                        .or_else(|| path.strip_suffix("?sharedworker"))
                })
                .map_or_else(|| self.path.to_owned(), PathBuf::from)
        };
        let meta = |object: &Expression<'a>| {
            matches!(object.get_inner_expression(), Expression::ImportMeta(_))
        };
        let relative = |text: Option<&'a str>| {
            text.filter(|text| !text.contains(':') && !text.starts_with(['/', '\\']))
        };
        let literal = |argument: &Argument<'a>| match argument {
            Argument::StringLiteral(text) => relative(text.value.as_str()),
            _ => None,
        };
        match expression.get_inner_expression() {
            Expression::Identifier(name) if self.global(name) => match name.name.as_str() {
                "__filename" => Some(file()),
                "__dirname" => Some(folder),
                _ => None,
            },
            Expression::StaticMemberExpression(member) => match member.property.name.as_str() {
                "url" | "filename" if meta(&member.object) => Some(file()),
                "dirname" if meta(&member.object) => Some(folder),
                "href" | "pathname" => self.local_path(&member.object),
                _ => None,
            },
            Expression::NewExpression(url) if url.callee.is_specific_id("URL") => {
                match url.arguments.as_slice() {
                    [file] => self.local_path(file.as_expression()?),
                    [specifier, base] => Some(
                        self.local_path(base.as_expression()?)
                            .filter(|base| base.is_file())?
                            .parent()?
                            .join(literal(specifier)?),
                    ),
                    _ => None,
                }
            }
            Expression::BinaryExpression(sum) if sum.operator == BinaryOperator::Addition => {
                let Expression::StringLiteral(tail) = &sum.right else {
                    return None;
                };
                Some(
                    self.local_path(&sum.left)?
                        .join(relative(tail.value.as_str()?.strip_prefix('/'))?),
                )
            }
            Expression::TemplateLiteral(template) => {
                let ([folder], [head, tail]) =
                    (template.expressions.as_slice(), template.quasis.as_slice())
                else {
                    return None;
                };
                let tail = tail.value.cooked.as_ref()?.as_str()?.strip_prefix('/');
                Some(
                    self.local_path(folder)
                        .filter(|_| head.value.raw.is_empty())?
                        .join(relative(tail)?),
                )
            }
            Expression::CallExpression(call) => {
                let (name, object) = match call.callee.get_inner_expression() {
                    Expression::Identifier(name) => (name.name.as_str(), None),
                    Expression::StaticMemberExpression(member) => {
                        (member.property.name.as_str(), Some(&member.object))
                    }
                    _ => return None,
                };
                let first = call.arguments.first();
                match (name, call.arguments.len()) {
                    ("resolve", 1)
                        if object.is_some_and(|object| {
                            meta(object) || object.is_specific_id("require")
                        }) =>
                    {
                        let specifier = literal(first?)?;
                        if specifier.starts_with('.') {
                            return Some(folder.join(specifier));
                        }
                        // other names are packages, not files next to this one.
                        match self.builder.locate(&folder, specifier, true) {
                            Ok((path, None)) if path.is_file() => Some(path),
                            _ => None,
                        }
                    }
                    ("toString", 0) => self.local_path(object?),
                    ("String" | "fileURLToPath", 1) => self.local_path(first?.as_expression()?),
                    ("dirname", 1) => self
                        .local_path(first?.as_expression()?)?
                        .parent()
                        .map(Path::to_owned),
                    ("join" | "resolve", _) => {
                        let mut path = self.local_path(first?.as_expression()?)?;
                        for argument in &call.arguments[1..] {
                            path.push(literal(argument)?);
                        }
                        Some(path)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
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

    fn visit_import_specifier(&mut self, specifier: &mut ImportSpecifier<'a>) {
        let imported = specifier.imported.name();
        if specifier.local.name != imported
            && let Some(name) = ["Worker", "fork"]
                .into_iter()
                .find(|name| imported == *name)
        {
            self.aliases.push((specifier.local.symbol_id(), name));
        }
    }

    fn visit_property_key(&mut self, key: &mut PropertyKey<'a>) {
        if !matches!(
            key,
            PropertyKey::StringLiteral(_) | PropertyKey::TemplateLiteral(_)
        ) {
            walk_mut::walk_property_key(self, key);
        }
    }

    fn visit_expression(&mut self, expression: &mut Expression<'a>) {
        if self.error.is_some() {
            return;
        }
        let result = (|| -> Result<()> {
            let span = expression.span();
            if self.builder.realm == Realm::Renderer
                && let Some(value) = match &*expression {
                    Expression::StringLiteral(literal) => literal.value.as_str(),
                    Expression::TemplateLiteral(template) if template.expressions.is_empty() => {
                        template.quasis[0]
                            .value
                            .cooked
                            .as_ref()
                            .and_then(|v| v.as_str())
                    }
                    _ => None,
                }
                && let Some(file) = value.strip_prefix('/')
                && Path::new(file).extension().is_some_and(is_asset)
                && !is_remote(self.path)
                && let Ok(path) = self.builder.shared.app_file(file).canonicalize()
                && self.builder.shared.in_app(&path)
                && let Some(url) = self.builder.url_of(&path)?
            {
                self.cacheable = false;
                let value = self.allocator.alloc_str(&url);
                *expression = Expression::new_string_literal(span, value, None, &self.ast);
                return Ok(());
            }
            let (scoping, aliases) = (self.scoping, &self.aliases);
            let name = |callee: &Expression<'a>| match callee.get_inner_expression() {
                Expression::Identifier(name) => Some(
                    scoping
                        .get_reference(name.reference_id())
                        .symbol_id()
                        .and_then(|symbol| aliases.iter().find(|alias| alias.0 == symbol))
                        .map_or(name.name.as_str(), |alias| alias.1),
                ),
                Expression::StaticMemberExpression(member) => Some(member.property.name.as_str()),
                _ => None,
            };
            let script = match expression {
                Expression::NewExpression(worker) if name(&worker.callee) == Some("Worker") => {
                    Some(&mut worker.arguments)
                }
                Expression::CallExpression(fork)
                    if self.builder.realm != Realm::Renderer
                        && name(&fork.callee) == Some("fork") =>
                {
                    Some(&mut fork.arguments)
                }
                _ => None,
            };
            if let Some(arguments) = script
                && let Some(source) = arguments.first().and_then(Argument::as_expression)
                && let Some(path) = self.local_path(source)
            {
                let path = resolve_file(
                    &path,
                    entry_fields(self.builder.realm == Realm::Renderer, false),
                )
                .with_context(|| format!("Cannot find the worker {}", path.display()))?;
                self.cacheable = false;
                let index = self.builder.script_chunk(path)?;
                let value = self
                    .allocator
                    .alloc_str(&format!("apps:///script-{index}.js"));
                arguments[0] =
                    Expression::new_string_literal(source.span(), value, None, &self.ast).into();
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
                && let Some(asset) = self.builder.shared.asset(self.folder, path)
            {
                self.cacheable = false;
                let value = self.allocator.alloc_str(&asset);
                url.arguments[0] =
                    Expression::new_string_literal(specifier.span, value, None, &self.ast).into();
            }
            if !self.commonjs
                && let Expression::Identifier(name) = expression
                && matches!(name.name.as_str(), "__dirname" | "__filename")
                && self.builder.realm == Realm::Main
                && self.global(name)
            {
                *expression = self.parse(if name.name == "__dirname" {
                    "import.meta.dirname"
                } else {
                    "import.meta.filename"
                })?;
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
                let main = self.builder.shared.is_process_entry(self.path);
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
                    && self.global(process)
                {
                    *expression = Expression::new_string_literal(
                        span,
                        if self.builder.shared.production {
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
                    let code = self
                        .builder
                        .shared
                        .meta_env(Some(member.property.name.as_str()));
                    *expression = self.parse(if code == "null" { "void 0" } else { &code })?;
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
                    } else if self.builder.shared.production {
                        let relative = self.builder.shared.app_name(self.path)?;
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
                    *expression = self.parse(&code)?;
                    return Ok(());
                }
                ensure!(
                    self.commonjs
                        || member.property.name != "exports"
                        || !matches!(&member.object, Expression::Identifier(module) if module.name == "module"
                            && self.global(module)),
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
                let target = self.folder.join(specifier);
                if !target.extension().is_some_and(|e| e == "html") {
                    let target = target
                        .canonicalize()
                        .with_context(|| format!("Cannot resolve {}", target.display()))?;
                    self.cacheable = false;
                    self.builder.shared.add_process_entry(target.clone());
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
                let base = self.folder;
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
                *expression = self.parse(&format!("({})", Value::Object(urls)))?;
                return Ok(());
            }
            if self.commonjs
                && let Expression::CallExpression(call) = expression
                && let Expression::StaticMemberExpression(member) =
                    call.callee.get_inner_expression()
                && member.property.name == "resolve"
                && let Expression::Identifier(require) = member.object.get_inner_expression()
                && require.name == "require"
                && self.global(require)
                && let Some(Argument::StringLiteral(specifier)) = call.arguments.first()
                && let Some(specifier) = specifier.value.as_str()
            {
                self.cacheable = false;
                if let Ok((path, None)) = self.builder.locate(self.folder, specifier, true)
                    && !path.as_os_str().as_encoded_bytes().starts_with(b"apps:")
                {
                    let path = self.builder.runtime_path(&path)?;
                    self.resolved.insert(specifier.to_owned(), path.into());
                }
            }
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
                && let Ok(target) = self.builder.shared.app_file(path).canonicalize()
            {
                let literal = specifier.span;
                let value = self.page(target)?;
                call.arguments[0] =
                    Expression::new_string_literal(literal, value, None, &self.ast).into();
                walk_mut::walk_expression(self, expression);
                return Ok(());
            }
            let defer = matches!(expression, Expression::ImportExpression(import) if import.phase == Some(ImportPhase::Defer));
            let import = match expression {
                Expression::ImportExpression(import) => {
                    match import.source.get_inner_expression() {
                        Expression::StringLiteral(specifier)
                            if !specifier.value.starts_with("data:") =>
                        {
                            let specifier =
                                specifier.value.as_str().context("Invalid module path")?;
                            let text = matches!(
                                property(property(import.options.as_ref(), "with"), "type"),
                                Some(Expression::StringLiteral(kind)) if kind.value.as_str() == Some("text")
                            );
                            Some((
                                "$appsImport",
                                self.allocator.alloc_str(&attributed(specifier, text)),
                            ))
                        }
                        source => {
                            let (data, shared) = (source.is_string_literal(), &self.builder.shared);
                            let feature = if data {
                                "jsExecute"
                            } else {
                                "jsExecute or httpImport"
                            };
                            let enabled = shared.enabled("jsExecute")
                                || (!data && shared.enabled("httpImport"));
                            // Unresolved imports in node_modules fail at run time, so unused paths do not
                            // break the build.
                            if !enabled && in_packages(self.path) {
                                *expression = self.parse(&format!(
                                    "Promise.reject(new Error({}))",
                                    json!(format!(
                                        "import() without a string literal needs the {feature} feature"
                                    ))
                                ))?;
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
                    if !self.global(identifier) {
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
                Expression::ImportMeta(_) => {
                    *expression =
                        self.parse(&format!("({{env:{}}})", self.builder.shared.meta_env(None)))?;
                    return Ok(());
                }
                _ => None,
            };
            if let Some((name, specifier)) = import {
                let base = self.folder;
                // Unresolved require() calls throw MODULE_NOT_FOUND at run time, like esbuild.
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
                let number = |value| {
                    Expression::new_numeric_literal(
                        span,
                        value,
                        None,
                        NumberBase::Decimal,
                        &self.ast,
                    )
                    .into()
                };
                let mut arguments = ArenaVec::from_array_in([number(dep as f64)], &self.ast);
                if defer {
                    arguments.push(number(1.));
                }
                *expression = Expression::new_call_expression(
                    span,
                    Expression::new_identifier(span, name, &self.ast),
                    None,
                    arguments,
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
