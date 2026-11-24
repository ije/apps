use super::*;
use super::{
    css::css,
    emit::{Awaits, Export, Import, Imports, Module, TopLevelAwait, Writes},
    html::html,
    link::Linker,
    resolve::{ELECTRON, PACKAGES, in_packages, is_remote, native_specifier},
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
        if self.realm == Realm::Renderer && self.shared.is_preload(&path) {
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
            self.shared.watch(path.clone());
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
        let stamp = (self.shared.watching() && inline.is_none())
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
            self.shared.watch(path.clone());
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
            self.shared.watch(path.clone());
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
            self.shared.watch(path.clone());
            match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
                "html" => {
                    ensure!(
                        self.realm == Realm::Main,
                        "HTML imports are only supported in main"
                    );
                    format!(
                        "export default {};",
                        serde_json::to_string(&html(&path, &self.shared)?)?
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
                _ if self.shared.asset_name(&path).is_some() => format!(
                    "export default {};",
                    json!(format!(
                        "apps:///{}",
                        self.shared.asset_name(&path).unwrap()
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
