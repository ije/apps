use super::{
    Builder, Cached, Realm, attributed,
    css::css,
    expand_braces, glob, is_asset,
    module::{Awaits, Export, Import, Imports, Module, TopLevelAwait, Writes},
    property,
    resolve::{
        ELECTRON, PACKAGES, entry_fields, in_packages, is_remote, native_specifier, resolve_file,
    },
};
use anyhow::{Context, Result, bail, ensure};
use oxc_allocator::{Allocator, ArenaVec};
use oxc_ast::{ast::*, builder::AstBuilder};
use oxc_ast_visit::{Visit, VisitMut, walk_mut};
use oxc_codegen::{Codegen, CodegenOptions};
use oxc_parser::Parser;
use oxc_react_compiler::{CompileResult, PluginOptions};
use oxc_semantic::SemanticBuilder;
use oxc_span::{GetSpan, SourceType};
use oxc_transformer::{TransformOptions, Transformer};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap},
    fmt::Write,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

impl Builder {
    pub(super) fn visit(&mut self, path: PathBuf, inline: Option<String>) -> Result<usize> {
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
        let id = self.id(key);
        if self.realm == Realm::Renderer && self.shared.preload.as_ref() == Some(&path) {
            self.define(
                id,
                Module {
                    preload: true,
                    linked: true,
                    ..Module::default()
                },
            );
            return Ok(id);
        }
        let dependency =
            in_packages(&path) || self.package_roots.iter().any(|root| path.starts_with(root));
        if inline.is_none()
            && path
                .extension()
                .is_some_and(|extension| extension == "node")
        {
            ensure!(
                self.realm != Realm::Renderer,
                "Native addons are unavailable in page scripts. Use a preload script."
            );
            self.shared.files.borrow_mut().insert(path.clone());
            let name = native_specifier(&path)?;
            self.define(
                id,
                Module {
                    code: format!(
                        "module.exports=globalThis.__appsLoadAddon({});",
                        json!(name)
                    ),
                    commonjs: true,
                    ..Module::default()
                },
            );
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
            self.shared.files.borrow_mut().insert(path.clone());
            let mut valid = true;
            for (base, specifier, require, dep) in &requests {
                if self.resolve(base, specifier, *require).ok() != Some(*dep) {
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
            self.shared.files.borrow_mut().insert(path.clone());
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
            self.shared.files.borrow_mut().insert(path.clone());
            match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
                "html" => {
                    ensure!(
                        self.realm == Realm::Main,
                        "HTML imports are only supported in main"
                    );
                    format!(
                        "export default {};",
                        serde_json::to_string(&self.shared.html(&path)?)?
                    )
                }
                "css" if self.realm == Realm::Renderer => {
                    let text = json!(css(&path, None, &mut Vec::new(), &self.shared)?);
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
                _ if let Some(name) = self.shared.asset_name(&path) => {
                    format!("export default {};", json!(format!("apps:///{name}")))
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
        let react = self.realm == Realm::Renderer
            && !dependency
            && self
                .shared
                .enabled(apps_runtime::bundle::Feature::ReactCompiler);
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
        // The React Compiler must run on the program as parsed, before any other transform.
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
        options.jsx.development = !self.shared.production;
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
        // Direct `eval` needs import names in scope, so only those modules keep the `with` scope.
        let linked = !commonjs && !references.contains_key("eval");
        let mut asynchronous = TopLevelAwait(0);
        asynchronous.visit_program(&program);
        let asynchronous = asynchronous.0;
        let folder = path.parent().unwrap_or(Path::new("."));
        let mut linker = Linker {
            builder: self,
            allocator: &allocator,
            ast: AstBuilder::new(&allocator),
            scoping: &transformed.scoping,
            path: &path,
            folder,
            commonjs,
            // CSS keeps no record of its `@import` and `url()` files, so it can't be cached.
            cacheable: stamp.is_some()
                && !path.extension().is_some_and(|e| e == "html" || e == "css"),
            imported: Vec::new(),
            required: Vec::new(),
            requires: Vec::new(),
            aliases: Vec::new(),
            resolved: serde_json::Map::new(),
            error: None,
        };
        linker.visit_program(&mut program);
        if let Some(error) = linker.error {
            return Err(error);
        }
        let (cacheable, imported, mut required, resolved) = (
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
        let mut defers = Vec::new();
        let mut body = ArenaVec::new_in(&ast);
        for statement in program.body {
            let source = match &statement {
                Statement::ImportDeclaration(s) => Some((&s.source, s.with_clause.as_deref())),
                Statement::ExportFromDeclaration(s) => Some((&s.source, s.with_clause.as_deref())),
                Statement::ExportAllDeclaration(s) => Some((&s.source, s.with_clause.as_deref())),
                _ => None,
            };
            let resolved = match source {
                Some((source, clause)) => {
                    let text = clause.is_some_and(|clause| {
                        clause.with_entries.iter().any(|entry| {
                            entry.key.as_js_str().as_str() == Some("type")
                                && entry.value.value.as_str() == Some("text")
                        })
                    });
                    let specifier =
                        attributed(source.value.as_str().context("Invalid module path")?, text);
                    let dep = self.resolve(folder, &specifier, false)?;
                    self.requests
                        .push((folder.to_owned(), specifier.clone(), false, dep));
                    Some((dep, specifier))
                }
                None => None,
            };
            match (statement, resolved) {
                (Statement::ImportDeclaration(import), Some((dep, source))) => {
                    // A deferred module runs when its namespace is first read, so it is no dependency.
                    if import.phase == Some(ImportPhase::Defer) {
                        self.dynamic.insert(dep);
                        required.push(dep);
                        defers.push(dep);
                        for specifier in import.unbox().specifiers.into_iter().flatten() {
                            let code = allocator.alloc_str(&format!(
                                "const {}=$appsImport.defer({dep});",
                                specifier.local().name
                            ));
                            let parsed = Parser::new(&allocator, code, SourceType::mjs()).parse();
                            body.splice(0..0, parsed.program.body);
                        }
                        continue;
                    }
                    imports.push(Import {
                        dep,
                        local: None,
                        name: "*".into(),
                    });
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
                (Statement::ExportDeclaration(export), _) => {
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
                (Statement::ExportNamedDeclaration(export), _) => {
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
                (Statement::ExportFromDeclaration(export), Some((dep, source))) => {
                    imports.push(Import {
                        dep,
                        local: None,
                        name: "*".into(),
                    });
                    for specifier in &export.specifiers {
                        let local = format!("$appsRe{}", imports.len());
                        let name = specifier.local.name().to_string();
                        ensure!(
                            !matches!(source.as_str(), "electron" | "@pierre/apps")
                                || name == "default"
                                || ELECTRON.contains(&name.as_str()),
                            "{source}.{name} is not supported (exported in {})",
                            path.display()
                        );
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
                (Statement::ExportAllDeclaration(export), Some((dep, _))) => {
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
                (Statement::ExportDefaultDeclaration(export), _) => {
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
                (statement, _) => body.push(statement),
            }
        }
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
        if self.shared.production && linked {
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
                source_map_path: (!self.shared.production).then(|| path.clone()),
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
            defers,
            commonjs,
            linked,
            asynchronous,
            builtin: path
                .to_str()
                .and_then(|path| path.strip_prefix("apps:"))
                .is_some_and(|specifier| {
                    specifier.starts_with("node:") || PACKAGES.contains(&specifier)
                }),
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
            // The runtime adds one line before the module body, so the source map starts with an empty line.
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
        if self.shared.production {
            let package = self.shared.package_of(&path);
            module.pure = match package
                .as_ref()
                .map(|(root, manifest)| (*root, &manifest["sideEffects"]))
            {
                Some((_, Value::Bool(false))) => true,
                Some((root, Value::Array(patterns))) => {
                    let relative = path
                        .strip_prefix(root)?
                        .to_string_lossy()
                        .replace('\\', "/");
                    !patterns
                        .iter()
                        .filter_map(Value::as_str)
                        .flat_map(expand_braces)
                        .any(|pattern| {
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
            self.modules[id] = module;
        } else {
            let line: Rc<str> = module.line(id).into();
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

struct Linker<'a, 'b> {
    builder: &'b mut Builder,
    allocator: &'a Allocator,
    ast: AstBuilder<'a>,
    scoping: &'b oxc_semantic::Scoping,
    path: &'b Path,
    folder: &'b Path,
    commonjs: bool,
    cacheable: bool,
    imported: Vec<usize>,
    required: Vec<usize>,
    resolved: serde_json::Map<String, Value>,
    requires: Vec<oxc_semantic::SymbolId>,
    aliases: Vec<(oxc_semantic::SymbolId, &'static str)>,
    error: Option<anyhow::Error>,
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
                && let Ok(path) = self.builder.shared.root.join(file).canonicalize()
                && path.starts_with(&self.builder.shared.root)
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
                Expression::NewExpression(worker)
                    if matches!(name(&worker.callee), Some("Worker" | "SharedWorker")) =>
                {
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
                    let code =
                        self.builder.shared.meta_env[member.property.name.as_str()].to_string();
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
                && let Ok(target) = self.builder.shared.root.join(path).canonicalize()
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
                            let enabled = shared.enabled(apps_runtime::bundle::Feature::JsExecute)
                                || (!data
                                    && shared.enabled(apps_runtime::bundle::Feature::HttpImport));
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
                        self.parse(&format!("({{env:{}}})", self.builder.shared.meta_env))?;
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
