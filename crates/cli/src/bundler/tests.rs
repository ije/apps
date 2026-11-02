use super::*;

#[test]
#[ignore = "release bundle size measurement; set APPS_BENCH_ENTRY"]
fn development_bundle_size() {
    let entry = PathBuf::from(std::env::var_os("APPS_BENCH_ENTRY").unwrap());
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
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
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        let engine = apps_runtime::jsc::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        engine.load_bundle(&files, "app.js").unwrap();
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
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
    let engine = apps_runtime::jsc::JsContext::new(
        apps_runtime::jsc::JavaScriptCore::load().unwrap(),
        Rc::default(),
        apps_runtime::host::Role::Main,
    )
    .unwrap();
    engine.load_bundle(&files, "app.js").unwrap();
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
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        let engine = apps_runtime::jsc::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        engine.load_bundle(&files, "app.js").unwrap();
        assert_eq!(engine.eval("result", "test").unwrap(), "42:42:6:true");
        let output = files.values().cloned().collect::<String>();
        assert_eq!(output.contains("UNUSED_TYPESCRIPT_EXPORT"), !production);
        assert_eq!(output.contains("\"map\":"), !production);
        assert!(!output.contains("\"debug\":"));
    }
}

#[test]
fn linked_imports_stay_live_without_a_with_scope() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("state.js"),
        r#"
            export let count = 0;
            export const bump = () => { count++; };
            export function self() { return this; }
            export const tag = (parts, value) => parts[0] + value;
            export const String = value => 'imported:' + value;
            export { count as alias };
        "#,
    )
    .unwrap();
    fs::write(
        dir.path().join("barrel.js"),
        "export { bump as increment } from './state.js'; export * as all from './state.js';",
    )
    .unwrap();
    let entry = dir.path().join("app.js");
    fs::write(&entry, r#"
            import { count, bump, self, tag, String, alias } from './state.js';
            import * as state from './state.js';
            import { increment, all } from './barrel.js';
            const before = count;
            bump(); increment(); all.bump();
            const wait = async value => value;
            const awaited = await wait(count);
            const shorthand = { count };
            globalThis.result = [before, count, alias, state.count, shorthand.count, awaited, self() === undefined, tag`n${count}`, String(1), typeof Math.sqrt].join(':');
        "#).unwrap();
    for production in [false, true] {
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        let engine = apps_runtime::jsc::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        engine.load_bundle(&files, "app.js").unwrap();
        engine.tick().unwrap();
        assert_eq!(
            engine.eval("result", "test").unwrap(),
            "0:3:3:3:3:3:true:n3:imported:1:function",
            "production: {production}"
        );
    }
}

#[test]
#[ignore = "release performance measurement"]
fn linked_module_hot_loop() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("vec.js"),
        "export const scale = { x: 1.5 };",
    )
    .unwrap();
    for (name, extra) in [("linked", ""), ("with", "if (globalThis.never) eval('');")] {
        let entry = dir.path().join(format!("{name}.js"));
        fs::write(&entry, format!("import {{ scale }} from './vec.js'; {extra} const run = () => {{ let s = 0; for (let i = 0; i < 2e7; i++) s += Math.sqrt(i) * scale.x; return s; }}; run(); const t = performance.now(); run(); globalThis.ms = Math.round(performance.now() - t);")).unwrap();
        let files = compile(&entry, Realm::Main, None, Rc::default(), true, None).unwrap();
        let engine = apps_runtime::jsc::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        engine.load_bundle(&files, "app.js").unwrap();
        println!("{name}_hot_loop_ms={}", engine.eval("ms", "bench").unwrap());
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
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
    let host = Rc::default();
    let engine = apps_runtime::jsc::JsContext::new(
        apps_runtime::jsc::JavaScriptCore::load().unwrap(),
        Rc::clone(&host),
        apps_runtime::host::Role::Main,
    )
    .unwrap();
    engine.load_bundle(&files, "app.js").unwrap();
    let definitions: Vec<Value> = files
        .iter()
        .filter(|(name, _)| name.ends_with(".js"))
        .flat_map(|(_, code)| code.lines())
        .map(|line| {
            serde_json::from_str(
                line.strip_prefix("$appsDefine(")
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
    let sources = apps_runtime::desktop::devtools::dispatch(
        &json!({"method":"Sources.list"}),
        &engine,
        &host,
        0,
    )
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
fn package_loaders_transform_files_with_source_maps() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("package.json"),
        r#"{"loaders":{".txt":"./loader.ts"}}"#,
    )
    .unwrap();
    fs::write(dir.path().join("note.txt"), "hello\nsecond\nthird").unwrap();
    let entry = dir.path().join("app.ts");
    fs::write(
        &entry,
        "import text, {fail} from './note.txt';globalThis.text=text;globalThis.fail=fail;",
    )
    .unwrap();
    let cache = Rc::new(Cache::default());
    for version in ["first", "second"] {
        fs::write(dir.path().join("loader.ts"), format!("import {{basename}} from 'node:path';\nexport default {{\n  async load({{filename, content}}: {{filename: string; content: string}}) {{\n    await new Promise(resolve => setTimeout(resolve, 1));\n    const text = basename(filename) + ':' + content + ':{version}';\n    return {{type: 'ts', code: `const text: string = ${{JSON.stringify(text)}};\\nexport default text;\\nexport function fail() {{ throw new Error(text) }}`, map: {{version: 3, sources: ['note.txt'], sourcesContent: [content], names: [], mappings: 'AAAA;AACA;AACA'}}}};\n  }},\n}};\n")).unwrap();
        for production in [false, true] {
            let files = compile(
                &entry,
                Realm::Main,
                None,
                Rc::default(),
                production,
                (!production).then(|| cache.clone()),
            )
            .unwrap();
            let engine = apps_runtime::jsc::JsContext::new(
                apps_runtime::jsc::JavaScriptCore::load().unwrap(),
                Rc::default(),
                apps_runtime::host::Role::Main,
            )
            .unwrap();
            engine.load_bundle(&files, "app.js").unwrap();
            assert_eq!(
                engine.eval("globalThis.text", "apps:test").unwrap(),
                format!("note.txt:hello\nsecond\nthird:{version}")
            );
            if production {
                continue;
            }
            let stack = engine
                .eval(
                    "(()=>{try{fail()}catch(error){return error.stack}})()",
                    "apps:test",
                )
                .unwrap();
            let mut location = stack.lines().next().unwrap().rsplitn(3, ':');
            let column: u32 = location.next().unwrap().parse().unwrap();
            let line: u32 = location.next().unwrap().parse().unwrap();
            let definition: Value = files["app.js"]
                .lines()
                .map(|line| {
                    serde_json::from_str::<Value>(
                        line.strip_prefix("$appsDefine(")
                            .unwrap()
                            .strip_suffix(");")
                            .unwrap(),
                    )
                    .unwrap()
                })
                .find(|definition| {
                    definition["url"]
                        .as_str()
                        .is_some_and(|url| url.ends_with("note.txt"))
                })
                .unwrap();
            let json = definition["map"].to_string();
            let map = oxc_sourcemap::SourceMap::from_json_string(&json).unwrap();
            let token = map
                .lookup_token(&map.generate_lookup_table(), line - 1, column - 1)
                .unwrap();
            assert_eq!(token.get_src_line(), 2, "{stack}");
            assert_eq!(
                map.get_source_content(token.get_source_id().unwrap()),
                Some("hello\nsecond\nthird")
            );
        }
    }
    fs::write(
        dir.path().join("loader.ts"),
        "export default {load() { throw new TypeError('bad note') }}",
    )
    .unwrap();
    let error = compile(&entry, Realm::Main, None, Rc::default(), false, Some(cache)).unwrap_err();
    let message = format!("{error:#}");
    assert!(
        message.contains("Loader ./loader.ts failed on") && message.contains("TypeError: bad note"),
        "{message}"
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
    for name in ["clip.mp4", "clip.webm", "tone.mp3"] {
        fs::write(dir.path().join("img").join(name), b"media").unwrap();
    }
    fs::write(
        dir.path().join("web/css/style.css"),
        ".a { background: url(\"../../img/icon.svg\") }",
    )
    .unwrap();
    fs::write(
            dir.path().join("web/index.html"),
            r#"<link rel="stylesheet" href="css/style.css"><style>.b{background:url(../img/logo.png)}.c{background:url(data:image/png;base64,AA==)}</style><img src="../img/logo.png"><img src="https://example.com/x.png"><div style="background-image:url('../img/missing.png')"></div><video src="../img/clip.mp4" poster="../img/logo.png"><source src="../img/clip.webm"></video><audio src="../img/tone.mp3"></audio>"#,
        )
        .unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), true, None).unwrap();
    let assets: std::collections::BTreeMap<String, PathBuf> =
        serde_json::from_str(&files["assets.json"]).unwrap();
    assert_eq!(assets.len(), 5);
    let root = dir.path().canonicalize().unwrap();
    let logo = asset_name(&root, &root.join("img/logo.png")).unwrap();
    let icon = asset_name(&root, &root.join("img/icon.svg")).unwrap();
    assert!(logo.starts_with("assets/") && logo.ends_with(".png"));
    assert_eq!(
        assets[&icon],
        dir.path().join("img/icon.svg").canonicalize().unwrap()
    );
    let page = &files["page-0.html"];
    assert_eq!(page.matches(&format!("apps:///{logo}")).count(), 3);
    for name in ["clip.mp4", "clip.webm", "tone.mp3"] {
        let media = asset_name(&root, &root.join("img").join(name)).unwrap();
        assert!(page.contains(&format!("src=\"apps:///{media}\"")), "{page}");
    }
    assert!(page.contains(&format!("url(\"apps:///{icon}\")")));
    assert!(
        page.contains("https://example.com/x.png") && page.contains("data:image/png;base64,AA==")
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
            let files =
                compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
            let host = Rc::new(RefCell::new(apps_runtime::host::Host::default()));
            let engine = apps_runtime::jsc::JsContext::new(
                apps_runtime::jsc::JavaScriptCore::load().unwrap(),
                host.clone(),
                apps_runtime::host::Role::Main,
            )
            .unwrap();
            engine.load_bundle(&files, "app.js").unwrap();
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
                    .any(|command| command["args"] == json!(["loadFile", 1, "index.html", 1]))
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
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
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
            let files =
                compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
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
            let files = compile(&entry, realm, None, Rc::default(), production, None).unwrap();
            let host = Rc::new(RefCell::new(apps_runtime::host::Host::default()));
            host.borrow_mut().pages.insert(
                1,
                apps_runtime::host::Page {
                    doc: Document::parse("<body></body>"),
                    dirty: false,
                },
            );
            let engine = apps_runtime::jsc::JsContext::new(
                apps_runtime::jsc::JavaScriptCore::load().unwrap(),
                host,
                role,
            )
            .unwrap();
            engine.load_bundle(&files, "app.js").unwrap();
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
            let files =
                compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
            let engine = apps_runtime::jsc::JsContext::new(
                apps_runtime::jsc::JavaScriptCore::load().unwrap(),
                Rc::default(),
                apps_runtime::host::Role::Main,
            )
            .unwrap();
            engine.load_bundle(&files, "view-test").unwrap();
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
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        let engine = apps_runtime::jsc::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        engine.load_bundle(&files, "serve-test").unwrap();
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
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
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
        let engine = apps_runtime::jsc::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        engine.load_bundle(&files, "test").unwrap();
        assert_eq!(
            engine.eval("result", "test").unwrap(),
            "2:1:1:first,second:2"
        );
    }
    fs::write(&entry, "import {missing} from './counter';").unwrap();
    assert!(
        compile(&entry, Realm::Main, None, Rc::default(), true, None)
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
    let files = compile(&entry, Realm::Main, None, Rc::default(), true, None).unwrap();
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
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
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
fn package_preload_exports_cross_isolated_contexts() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("src");
    fs::create_dir(&source).unwrap();
    fs::write(
        dir.path().join("package.json"),
        r##"{"name":"my_app","preload":"./preload.ts","imports":{"#preload":"./preload.ts"}}"##,
    )
    .unwrap();
    fs::write(
        dir.path().join("native.ts"),
        "import {basename} from 'node:path'; export const native = () => basename('/a/b');",
    )
    .unwrap();
    let entry = source.join("main.ts");
    fs::write(&entry, "import {app,BrowserWindow} from 'electron';app.whenReady().then(()=>{for(let i=0;i<2;i++)new BrowserWindow().loadFile('./index.html')});").unwrap();
    fs::write(source.join("index.html"), r#"<script type="preload">import {foo} from '../preload';if(foo()!=='24.0.0')throw Error('manual preload');</script><script type="module" src="./page.ts"></script><script type="module">import {foo} from '../preload';if(foo()!=='24.0.0')throw Error('second script');</script>"#).unwrap();
    fs::write(source.join("exports.ts"), "export * from '../preload.ts';").unwrap();
    fs::write(source.join("page.ts"), r##"
        import defaults, {foo, next, callback, config, fail, later} from '../preload';
        import * as api from '#preload';
        import {native} from './exports';
        if (foo() !== '24.0.0' || native() !== 'b' || defaults.answer() !== 42) throw Error('exports');
        if (typeof process !== 'undefined' || typeof __apps !== 'undefined' || typeof privatePreload !== 'undefined') throw Error('isolation');
        if (next() !== 1 || api.next() !== 2) throw Error('per-page state');
        if (callback(value => value + 1) !== 42 || await later() !== 43) throw Error('callbacks');
        config.nested.value = 0;
        if (defaults.value() !== 7) throw Error('copy');
        let caught = false;
        try { fail(); } catch (error) { caught = error instanceof TypeError && error.message === 'preload error'; }
        if (!caught) throw Error('errors');
        const dynamic = await import('../preload.ts');
        if (dynamic !== api || dynamic.foo !== foo || Object.keys(api).includes('module.exports')) throw Error('namespace');
    "##).unwrap();
    for asynchronous in [false, true] {
        fs::write(
            dir.path().join("preload.ts"),
            format!(
                r#"
            export * from './native';
            globalThis.privatePreload = (globalThis.privatePreload ?? 0) + 1;
            if (globalThis.privatePreload !== 1) throw Error('duplicate preload');
            {}
            let count = 0;
            export function foo() {{ return process.versions.node; }}
            export const next = () => ++count;
            export const callback = fn => fn(41);
            export const config = {{ nested: {{ value: 7 }} }};
            export const fail = () => {{ throw new TypeError('preload error'); }};
            export async function later() {{ return 43; }}
            export default {{ answer: () => 42, value: () => config.nested.value }};
        "#,
                if asynchronous {
                    "await new Promise(resolve => setTimeout(resolve, 1));"
                } else {
                    ""
                }
            ),
        )
        .unwrap();
        for production in [false, true] {
            let watched = Rc::default();
            let files = compile(&entry, Realm::Main, None, watched, production, None).unwrap();
            assert_eq!(
                apps_runtime::desktop::Desktop::new(&files)
                    .unwrap()
                    .check()
                    .unwrap(),
                0,
                "production={production}, asynchronous={asynchronous}"
            );
        }
    }
}

#[test]
fn package_preload_validates_metadata_and_runs_without_imports() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("main.ts");
    fs::write(&entry, "import {app,BrowserWindow} from 'electron';app.whenReady().then(()=>new BrowserWindow().loadFile('./index.html'));").unwrap();
    fs::write(
        dir.path().join("index.html"),
        "<script>if(document.body.getAttribute('data-preload')!=='yes')throw Error('missing preload');</script>",
    )
    .unwrap();
    fs::write(
        dir.path().join("preload.ts"),
        "document.body.setAttribute('data-preload','yes');export function foo(){return process.versions.node}",
    )
    .unwrap();
    for value in [
        json!(null),
        json!(1),
        json!([]),
        json!(""),
        json!("./missing.ts"),
    ] {
        fs::write(
            dir.path().join("package.json"),
            json!({"preload":value}).to_string(),
        )
        .unwrap();
        let error = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap_err();
        assert!(
            error.to_string().contains("package.json preload"),
            "{error:#}"
        );
    }
    fs::write(
        dir.path().join("package.json"),
        r#"{"preload":"./preload.ts"}"#,
    )
    .unwrap();
    for production in [false, true] {
        let watched = Rc::new(RefCell::new(BTreeSet::new()));
        let files = compile(&entry, Realm::Main, None, watched.clone(), production, None).unwrap();
        assert!(
            watched
                .borrow()
                .contains(&dir.path().join("preload.ts").canonicalize().unwrap())
        );
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
fn package_preload_dynamic_imports_follow_watch_changes() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("main.ts");
    fs::write(&entry, "import {app,BrowserWindow} from 'electron';app.whenReady().then(()=>new BrowserWindow().loadFile('./index.html'));").unwrap();
    fs::write(
        dir.path().join("package.json"),
        r#"{"preload":"./preload.ts"}"#,
    )
    .unwrap();
    fs::write(dir.path().join("index.html"), r#"<script type="module">const api=await import('./preload');if(api.foo()!==document.body.getAttribute('data-version'))throw Error('stale preload');if(typeof process!=='undefined')throw Error('isolation');</script>"#).unwrap();
    let cache = Rc::new(Cache::default());
    for version in ["first", "second"] {
        fs::write(dir.path().join("preload.ts"), format!("const version={};document.body.setAttribute('data-version',version);export const foo=()=>version;", json!(version))).unwrap();
        for production in [false, true] {
            let files = compile(
                &entry,
                Realm::Main,
                None,
                Rc::default(),
                production,
                (!production).then(|| cache.clone()),
            )
            .unwrap();
            assert_eq!(
                apps_runtime::desktop::Desktop::new(&files)
                    .unwrap()
                    .check()
                    .unwrap(),
                0
            );
        }
    }
    fs::write(dir.path().join("index.html"), r#"<script type="module">import {missing} from './preload';globalThis.missing=missing;</script>"#).unwrap();
    for production in [false, true] {
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        let error = apps_runtime::desktop::Desktop::new(&files)
            .unwrap()
            .check()
            .unwrap_err();
        assert!(
            format!("{error:#}").contains("Missing export missing"),
            "{error:#}"
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
        let files = compile(&entry, Realm::Renderer, None, watched, production, None).unwrap();
        let engine = apps_runtime::jsc::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        engine.load_bundle(&files, "test").unwrap();
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
        for source in [
            "module.exports = require(name);",
            "module.exports = require('node:fs');",
        ] {
            fs::write(package.join("index.js"), source).unwrap();
            let files = compile(
                &entry,
                Realm::Renderer,
                None,
                Rc::default(),
                production,
                None,
            )
            .unwrap();
            let engine = apps_runtime::jsc::JsContext::new(
                apps_runtime::jsc::JavaScriptCore::load().unwrap(),
                Rc::default(),
                apps_runtime::host::Role::Main,
            )
            .unwrap();
            engine.eval("delete globalThis.__apps", "test").unwrap();
            assert!(engine.load_bundle(&files, "test").is_err());
        }
    }
}

#[test]
fn commonjs_interop_patterns_of_express_dependencies() {
    let dir = tempfile::tempdir().unwrap();
    let package = dir.path().join("node_modules/legacy");
    fs::create_dir_all(&package).unwrap();
    fs::write(package.join("package.json"), "{}").unwrap();
    fs::write(package.join("index.js"), r#"
        function stack() { var prepare = Error.prepareStackTrace, holder = {}; Error.prepareStackTrace = function (_, sites) { return sites; }; Error.captureStackTrace(holder); var sites = holder.stack.slice(1); Error.prepareStackTrace = prepare; return sites; }
        var EventEmitter = require('events').EventEmitter, app = function () {};
        Object.getOwnPropertyNames(EventEmitter.prototype).forEach(function (name) { if (name !== 'constructor') Object.defineProperty(app, name, Object.getOwnPropertyDescriptor(EventEmitter.prototype, name)); });
        app.on('ping', function (value) { app.seen = value; }); app.emit('ping', 1);
        var safe = {}; for (var key in require('buffer').Buffer) safe[key] = require('buffer').Buffer[key];
        var http = require('http'), request = Object.create(http.IncomingMessage.prototype), response = Object.create(http.ServerResponse.prototype);
        module.exports = [typeof (function caller() { return stack(); })()[0].getFileName(), app.seen, typeof safe.isBuffer, typeof request.on, typeof response.setHeader, require('util').inherits.length].join();
    "#).unwrap();
    let entry = dir.path().join("main.js");
    fs::write(
        &entry,
        "import result from 'legacy'; globalThis.result = result;",
    )
    .unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
    let engine = apps_runtime::jsc::JsContext::new(
        apps_runtime::jsc::JavaScriptCore::load().unwrap(),
        Rc::default(),
        apps_runtime::host::Role::Main,
    )
    .unwrap();
    engine.load_bundle(&files, "app.js").unwrap();
    assert_eq!(
        engine.eval("result", "test").unwrap(),
        "string,1,function,function,function,2"
    );
}

#[test]
fn commonjs_require_runtime_semantics() {
    for production in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("node_modules/pkg");
        fs::create_dir_all(package.join("lib")).unwrap();
        fs::write(
            package.join("package.json"),
            r#"{"name":"pkg","main":"lib/main.js","module":"lib/esm.js"}"#,
        )
        .unwrap();
        fs::write(package.join("lib/esm.js"), "export default 'esm';").unwrap();
        fs::write(package.join("lib/bad.js"), "exports.partial = 1; globalThis.runs = (globalThis.runs || 0) + 1; throw new Error('bad');").unwrap();
        fs::write(package.join("lib/main.js"), r#"
            const result = [];
            try { require('does-not-exist'); } catch (error) { result.push(error.code); }
            try { require('vm'); } catch (error) { result.push(error.code); }
            const name = 'pa' + 'th';
            result.push(typeof require(name).join, typeof require('events'), require('events') === require('events').EventEmitter, typeof require('assert'), require('fs') === require('node:fs'));
            Object.defineProperty(require('fs'), Symbol.for('patched'), { value: 1 });
            result.push(require('node:fs')[Symbol.for('patched')], typeof require('constants').O_RDONLY, require('sys') === require('util'));
            for (let i = 0; i < 2; i++) try { require('./bad.js'); } catch (error) { result.push(error.message + globalThis.runs); }
            result.push(__filename.endsWith('main.js'), __dirname === require('path').dirname(__filename), module.filename === __filename, module.id === __filename, require.main, typeof module.require);
            result.push(require.resolve('./bad.js') === require('path').join(__dirname, 'bad.js'), require.resolve('fs'), require.cache[__filename] === module);
            try { require.resolve('nope'); } catch (error) { result.push(error.code); }
            module.exports = result.join();
        "#).unwrap();
        let entry = dir.path().join("main.js");
        fs::write(
            &entry,
            "import result from 'pkg'; globalThis.result = result;",
        )
        .unwrap();
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        let engine = apps_runtime::jsc::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        engine.load_bundle(&files, "app.js").unwrap();
        assert_eq!(
            engine.eval("result", "test").unwrap(),
            "MODULE_NOT_FOUND,MODULE_NOT_FOUND,function,function,true,function,true,1,number,true,bad1,bad2,true,true,true,true,,function,true,fs,true,MODULE_NOT_FOUND",
            "production={production}"
        );
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
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        let engine = apps_runtime::jsc::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        engine.load_bundle(&files, "test").unwrap();
        assert_eq!(engine.eval("result", "test").unwrap(), "2:1:2:1");
    }
}

#[test]
#[ignore = "Run npm ci in examples/react first"]
fn react_counter() {
    for production in [false, true] {
        use apps_runtime::{
            host::{Host, Page, Role},
            jsc::{JavaScriptCore, JsContext},
        };
        let host = Rc::new(RefCell::new(Host::default()));
        host.borrow_mut().pages.insert(
            1,
            Page {
                doc: Document::parse("<html><body><div id='root'></div></body></html>"),
                dirty: false,
            },
        );
        let engine = JsContext::new(
            JavaScriptCore::load().unwrap(),
            host.clone(),
            Role::Renderer(1),
        )
        .unwrap();
        let entry = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/react/page.tsx")
            .canonicalize()
            .unwrap();
        let files = compile(
            &entry,
            Realm::Renderer,
            None,
            Rc::default(),
            production,
            None,
        )
        .unwrap();
        engine.load_bundle(&files, "react-example").unwrap();
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
            engine.tick().unwrap();
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
            jsc::{JavaScriptCore, JsContext},
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
            None,
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
        let engine = JsContext::new(
            JavaScriptCore::load().unwrap(),
            host.clone(),
            Role::Renderer(1),
        )
        .unwrap();
        engine.load_bundle(&files, "three-test").unwrap();
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
        let node = host.borrow().pages[&1]
            .doc
            .nodes
            .iter()
            .position(|n| n.tag == "canvas")
            .unwrap();
        // Each `image` call starts a readback when the canvas changed, so a new frame shows up a call later.
        let present = |old: Option<*const ()>| {
            for _ in 0..5000 {
                engine.tick().unwrap();
                let image = host
                    .borrow_mut()
                    .graphics
                    .get_mut(&1)
                    .unwrap()
                    .image(node)
                    .unwrap();
                if let Some(image) =
                    image.filter(|image| Some(std::sync::Arc::as_ptr(image).cast()) != old)
                {
                    return image;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            panic!("Canvas was not presented");
        };
        let image = present(None);
        let pixels = image.bytes();
        assert_eq!(pixels.len(), 96 * 96 * 4);
        let center = &pixels[(48 * 96 + 48) * 4..(48 * 96 + 48) * 4 + 4];
        assert_ne!(center, &pixels[..4], "Cube was not drawn");
        assert!(
            center[..3].iter().any(|&channel| channel > 100),
            "Cube is too dark"
        );
        let frame = present(Some(std::sync::Arc::as_ptr(&image).cast()));
        assert_ne!(pixels, frame.bytes(), "Animation did not update the canvas");
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
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
    let engine = apps_runtime::jsc::JsContext::new(
        apps_runtime::jsc::JavaScriptCore::load().unwrap(),
        std::rc::Rc::new(std::cell::RefCell::new(apps_runtime::host::Host::default())),
        apps_runtime::host::Role::Main,
    )
    .unwrap();
    engine.load_bundle(&files, "app.js").unwrap();
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
        assert!(compile(&entry, Realm::Renderer, None, Rc::default(), false, None).is_err());
    }
}
#[test]
fn cycles_reexports_and_missing_exports() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("a.js"),
        "import { b } from './b.js'; export function a(){return 'a'} export const result = b();",
    )
    .unwrap();
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
    let engine = apps_runtime::jsc::JsContext::new(
        apps_runtime::jsc::JavaScriptCore::load().unwrap(),
        std::rc::Rc::new(std::cell::RefCell::new(apps_runtime::host::Host::default())),
        apps_runtime::host::Role::Main,
    )
    .unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
    engine.validate_bundle(&files).unwrap();
    engine.load_bundle(&files, "app.js").unwrap();
    assert_eq!(engine.eval("answer", "test").unwrap(), "ab");
    fs::write(&entry, "import {missing} from './a.js'").unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
    assert!(
        engine
            .validate_bundle(&files)
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
    let source = html(&path, Rc::clone(&files), Rc::default(), false).unwrap();
    assert!(source.contains("@import \"tailwindcss\";"));
    assert!(!source.contains(".h-4"));
    assert_eq!(files.borrow().len(), 2);
    let doc = Document::parse(&source);
    let style = apps_runtime::style::styles(&doc, 800., 600.).pop().unwrap();
    let px = |key: &str| apps_runtime::style::length(&style[key], 0., 16., 800., 600.);
    assert_eq!(px("height"), 16.);
    assert_eq!(px("width"), 16.);
    assert_eq!(style["display"], "flex");
    assert_eq!(style["color"], "#123456");
    assert_eq!(style["background-color"], "oklch(0.72 0.11 178)");
    assert_eq!(px("border-bottom-left-radius"), 8.);
    assert!(style.contains_key("box-shadow"));
}

#[test]
fn tailwind_parts_keep_their_imports() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("style.css"),
        "@layer theme, base, components, utilities;\n@import 'tailwindcss/theme.css' layer(theme);\n@import \"tailwindcss/utilities\" layer(utilities);",
    )
    .unwrap();
    let path = dir.path().join("index.html");
    fs::write(
        &path,
        "<link rel='stylesheet' href='./style.css'><ul class='flex'></ul>",
    )
    .unwrap();
    let source = html(&path, Rc::default(), Rc::default(), false).unwrap();
    assert!(source.contains("@import \"tailwindcss/theme.css\";"));
    assert!(source.contains("@import \"tailwindcss/utilities\";"));
    let style = apps_runtime::style::styles(&Document::parse(&source), 800., 600.)
        .pop()
        .unwrap();
    assert_eq!(style["display"], "flex");
    assert_eq!(style["list-style-type"], "disc");
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
    fs::write(dir.path().join("page.ts"),r#"const end: string = "</script>"; if (end !== '<' + '/script>') throw Error('escape'); document.querySelector('h1').textContent = end; if(typeof process !== 'undefined') throw Error('privilege leak'); if (!(globalThis as any).preloaded) throw Error('preload');"#).unwrap();
    fs::write(dir.path().join("index.html"),"<html><head><link rel='stylesheet' href='./style.css'><script type='preload'>__apps.contextBridge.exposeInMainWorld('preloaded', process.versions.apps)</script></head><body><h1>Hello</h1><script type='module' src='./page.ts'></script></body></html>").unwrap();
    let entry = dir.path().join("app.ts");
    fs::write(&entry,"import {app,BrowserWindow} from '@pierre/apps';import html from './index.html' with {type:'text'};if(!html.includes('#123456')) throw Error('css');app.whenReady().then(()=>new BrowserWindow().loadFile('index.html'));").unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
    assert_eq!(
        apps_runtime::desktop::Desktop::new(&files)
            .unwrap()
            .check()
            .unwrap(),
        0
    );
}

#[test]
fn load_file_literals_bundle_plain_electron_pages() {
    for production in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("styles.css"),
            "#color { color: rgb(1, 2, 3) }",
        )
        .unwrap();
        fs::write(dir.path().join("preload.js"), "import {contextBridge} from 'electron'; contextBridge.exposeInMainWorld('versions', {electron: process.versions.electron});").unwrap();
        fs::write(dir.path().join("renderer.js"), "if (versions.electron !== '40.0.0' || getComputedStyle(document.getElementById('color')).color !== 'rgb(1, 2, 3)' || document.querySelector('script[type=\"application/json\"]').textContent !== '{}') throw Error('renderer');").unwrap();
        fs::write(dir.path().join("index.html"), "<link rel='stylesheet' href='styles.css'><script type='preload' src='./preload.js'></script><p id='color'>x</p><script type='application/json'>{}</script><script type='text/javascript' src='renderer.js'></script>").unwrap();
        let entry = dir.path().join("main.js");
        fs::write(&entry, "import {app, BrowserWindow} from 'electron'; const page = 'missing.html'; app.whenReady().then(() => { new BrowserWindow().loadFile('index.html', {hash: 'top'}); new BrowserWindow().webContents.loadFile('./index.html'); globalThis.missing = new BrowserWindow().loadFile(page).catch(() => {}); });").unwrap();
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        assert!(files.contains_key("page-0.html") && !files.contains_key("page-1.html"));
        let app = apps_runtime::bundle::script_source(&files, false);
        assert!(app.matches("apps:///page-0.html").count() == 2 && app.contains("missing.html"));
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
fn unsupported_electron_exports_fail_at_build_time() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("main.js");
    for (source, error) in [
        (
            "import {app, Tray} from 'electron'; app.quit();",
            "electron.Tray is not supported",
        ),
        (
            "import {protocol} from '@pierre/apps';",
            "@pierre/apps.protocol is not supported",
        ),
    ] {
        fs::write(&entry, source).unwrap();
        for production in [false, true] {
            let message = compile(&entry, Realm::Main, None, Rc::default(), production, None)
                .unwrap_err()
                .to_string();
            assert!(message.contains(error), "{message}");
        }
    }
    let entry = dir.path().join("main.ts");
    fs::write(&entry, "import electron, {screen, MenuItem, webContents} from 'electron'; import type {Tray} from 'electron'; const tray: Tray | null = null; if (tray || electron.screen !== screen || typeof MenuItem !== 'function' || typeof webContents.fromId !== 'function') throw Error('exports');").unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), true, None).unwrap();
    apps_runtime::desktop::Desktop::new(&files).unwrap();
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
        let files = compile(
            &entry,
            Realm::Main,
            None,
            Rc::clone(&watched),
            production,
            None,
        )
        .unwrap();
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
        let engine = apps_runtime::jsc::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        engine.validate_bundle(&bundle.files).unwrap();
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
            let files = compile(
                &entry,
                Realm::Main,
                None,
                Rc::clone(&watched),
                production,
                None,
            )
            .unwrap();
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
            assert_eq!(
                metadata["identifier"],
                if local_package.is_none() {
                    "com.apps.package-name"
                } else {
                    "com.apps.App"
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
        let error = compile(
            &entry,
            realm,
            Some(source.into()),
            Rc::default(),
            false,
            None,
        )
        .unwrap_err();
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
            let files = compile(&entry, realm, None, Rc::default(), production, None).unwrap();
            let host = Rc::new(RefCell::new(apps_runtime::host::Host::default()));
            host.borrow_mut().pages.insert(
                1,
                apps_runtime::host::Page {
                    doc: Document::parse("<body></body>"),
                    dirty: false,
                },
            );
            let engine = apps_runtime::jsc::JsContext::new(
                apps_runtime::jsc::JavaScriptCore::load().unwrap(),
                host,
                role,
            )
            .unwrap();
            engine.load_bundle(&files, "app.js").unwrap();
            let result: Value =
                serde_json::from_str(&engine.eval("result", "test").unwrap()).unwrap();
            let base = if production {
                "apps:///".into()
            } else {
                url::Url::from_directory_path(&root).unwrap().to_string()
            };
            let url = format!("{base}app.ts");
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
    let base = dir.path().canonicalize().unwrap();
    let entry = base.join(std::ffi::OsStr::from_bytes(b"module-\xff.js"));
    for production in [false, true] {
        let files = compile(
            &entry,
            Realm::Main,
            Some("globalThis.result = import.meta.url;".into()),
            Rc::default(),
            production,
            None,
        )
        .unwrap();
        let engine = apps_runtime::jsc::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        engine.load_bundle(&files, "app.js").unwrap();
        assert_eq!(
            engine.eval("result", "test").unwrap(),
            if production {
                "apps:///module-%FF.js".into()
            } else {
                format!(
                    "{}module-%FF.js",
                    url::Url::from_directory_path(&base).unwrap()
                )
            },
        );
    }
}

#[test]
fn import_meta_url_in_inline_html_scripts() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let page = root.join("index.html");
    let entry = root.join("app.ts");
    fs::write(&entry, "import {app, BrowserWindow} from '@pierre/apps'; app.whenReady().then(() => new BrowserWindow().loadFile(import.meta.resolve('./index.html')));").unwrap();
    for production in [false, true] {
        let url = if production {
            "\"apps:///index.html\"".into()
        } else {
            serde_json::to_string(url::Url::from_file_path(&page).unwrap().as_str()).unwrap()
        };
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
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
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
        fs::write(&entry,"import {Worker} from 'node:worker_threads'; const feature=await import('fixture/features/example'); let matched;matched??=/Mac/.test('Mac');if(feature.answer!==42||!matched)throw Error('package or regexp'); const dynamic=await import('./value'); if(dynamic.value!==42||!import.meta.url.endsWith('/app.ts'))throw Error('module '+JSON.stringify(dynamic)+' url '+import.meta.url); const worker=new Worker(new URL('./worker.ts',import.meta.url)); const value=await new Promise((resolve,reject)=>{worker.on('message',resolve);worker.on('error',reject)}); if(value!==42)throw Error('worker'); await worker.terminate(); globalThis.result='ok';").unwrap();
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        let engine = apps_runtime::jsc::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        if let Err(error) = engine.load_bundle(&files, "app.js") {
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
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
    let engine = apps_runtime::jsc::JsContext::new(
        apps_runtime::jsc::JavaScriptCore::load().unwrap(),
        std::rc::Rc::new(std::cell::RefCell::new(apps_runtime::host::Host::default())),
        apps_runtime::host::Role::Main,
    )
    .unwrap();
    engine.load_bundle(&files, "app.js").unwrap();
    assert!(!dir.path().join("data.txt").exists());
}

#[test]
fn node_builtin_exports_match_runtime_objects() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("app.ts");
    let modules = [
        "assert",
        "assert/strict",
        "buffer",
        "child_process",
        "constants",
        "crypto",
        "dns",
        "dns/promises",
        "events",
        "fs",
        "fs/promises",
        "http",
        "https",
        "module",
        "net",
        "os",
        "path",
        "path/posix",
        "path/win32",
        "perf_hooks",
        "process",
        "querystring",
        "readline",
        "readline/promises",
        "stream",
        "stream/consumers",
        "stream/promises",
        "stream/web",
        "sqlite",
        "string_decoder",
        "sys",
        "timers",
        "timers/promises",
        "tty",
        "url",
        "util",
        "util/types",
        "worker_threads",
        "zlib",
    ];
    let mut source = String::new();
    for (index, name) in modules.iter().enumerate() {
        source += &format!("import * as m{index} from 'node:{name}';");
    }
    // WARNING: both directions: every listed export exists at runtime, and every runtime name can be imported by name.
    source += "const require=__apps.module.createRequire(),problems=[];";
    for (index, name) in modules.iter().enumerate() {
        source += &format!(
            "{{const object=require('node:{name}'),names=Object.keys(m{index}).filter(key=>key!=='default'&&key!=='module.exports');if(object!==m{index}.default)problems.push('{name}: default');for(const key of names)if(!(key in object))problems.push('{name}: missing '+key);if('{name}'!=='constants')for(const key of Object.keys(object))if(!key.startsWith('_')&&!names.includes(key))problems.push('{name}: not importable '+key);}}"
        );
    }
    source += "globalThis.result=problems.join(', ')||'ok';";
    fs::write(&entry, source).unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
    let engine = apps_runtime::jsc::JsContext::new(
        apps_runtime::jsc::JavaScriptCore::load().unwrap(),
        std::rc::Rc::new(std::cell::RefCell::new(apps_runtime::host::Host::default())),
        apps_runtime::host::Role::Main,
    )
    .unwrap();
    engine.load_bundle(&files, "app.js").unwrap();
    assert_eq!(engine.eval("result", "test").unwrap(), "ok");
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
        let files = compile(&entry, realm, None, Rc::default(), false, None).unwrap();
        let engine = apps_runtime::jsc::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::new(RefCell::new(apps_runtime::host::Host::default())),
            role,
        )
        .unwrap();
        engine.load_bundle(&files, "app.js").unwrap();
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
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
    let api = apps_runtime::jsc::JavaScriptCore::load().unwrap();
    let host = Rc::new(RefCell::new(apps_runtime::host::Host::default()));
    {
        let engine = apps_runtime::jsc::JsContext::new(
            api.clone(),
            host.clone(),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        engine.load_bundle(&files, "app.js").unwrap();
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
    let files = compile(&entry, Realm::Preload, None, Rc::default(), false, None).unwrap();
    let engine =
        apps_runtime::jsc::JsContext::new(api, host, apps_runtime::host::Role::Preload(1)).unwrap();
    engine.load_bundle(&files, "preload.js").unwrap();
}

#[test]
fn package_features_reach_the_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("app.js");
    fs::write(&entry, "").unwrap();
    for features in [r#"["webgpu"]"#, r#"["*","-webgpu"]"#] {
        fs::write(
            dir.path().join("package.json"),
            format!(r#"{{"features":{features}}}"#),
        )
        .unwrap();
        for production in [false, true] {
            let files =
                compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
            let package: Value = serde_json::from_str(&files["package.json"]).unwrap();
            assert_eq!(
                package["features"],
                serde_json::from_str::<Value>(features).unwrap()
            );
        }
    }
    let gpu = |disabled| {
        let host = Rc::new(RefCell::new(apps_runtime::host::Host {
            disabled,
            ..Default::default()
        }));
        host.borrow_mut().pages.insert(
            1,
            apps_runtime::host::Page {
                doc: Document::parse("<html><body><canvas></canvas></body></html>"),
                dirty: false,
            },
        );
        let engine = apps_runtime::jsc::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            host,
            apps_runtime::host::Role::Renderer(1),
        )
        .unwrap();
        engine.eval("const message = run => { try { run(); } catch (error) { return error.message; } }; [typeof __appsGPU, typeof GPUDevice, message(() => navigator.gpu.requestAdapter()), message(() => document.querySelector('canvas').getContext('webgpu'))].join()", "gpu-test").unwrap()
    };
    let error = r#"The webgpu feature is disabled. Add "webgpu" to "features" in package.json"#;
    assert_eq!(
        gpu(apps_runtime::bundle::disabled_features(None).unwrap()),
        format!("undefined,undefined,{error},{error}")
    );
    assert!(gpu(0).starts_with("undefined,function,,"));
    fs::write(dir.path().join("package.json"), r#"{"features":["webgl"]}"#).unwrap();
    let error = compile(&entry, Realm::Main, None, Rc::default(), true, None).unwrap_err();
    assert!(
        format!("{error:#}").contains(r#"Unknown feature "webgl""#),
        "{error:#}"
    );
}

#[cfg(unix)]
#[test]
fn child_process_with_top_level_await() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("app.js");
    fs::write(
        &entry,
        format!(
            "{}\nif (code !== 0) throw Error('child process failed');",
            include_str!("../../tests/fixtures/child_process.js")
        ),
    )
    .unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
    assert_eq!(
        apps_runtime::desktop::Desktop::new(&files)
            .unwrap()
            .check()
            .unwrap(),
        0
    );
}

#[test]
fn wasm_with_top_level_await() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("app.js");
    fs::write(
        &entry,
        format!(
            "{}\nif (instance.exports.add(20, 22) !== 42) throw Error('Wasm result');",
            include_str!("../../tests/fixtures/wasm.js")
        ),
    )
    .unwrap();
    for production in [false, true] {
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
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
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
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
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
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
            let files =
                compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
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
        let files = compile(
            &entry,
            Realm::Main,
            None,
            Rc::clone(&watched),
            production,
            None,
        )
        .unwrap();
        assert!(watched.borrow().contains(&library.canonicalize().unwrap()));
        assert_eq!(
            apps_runtime::desktop::Desktop::new(&files)
                .unwrap()
                .check()
                .unwrap(),
            0
        );
        let output = dir.path().join("dist");
        crate::packaging::package_app(
            &output,
            &files,
            &entry,
            &input,
            production,
            crate::build_target::BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH)
                .unwrap(),
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
            if production {
                assert!(!path.exists(), "Copied packed addon: {name}");
                assert!(bundle.scripts.contains_key(&format!("node_modules/{name}")));
            } else {
                assert!(path.is_file(), "Missing copied addon: {name}");
                assert_eq!(native[name], path.canonicalize().unwrap());
            }
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
        compile(&entry, Realm::Renderer, None, Rc::default(), false, None)
            .unwrap_err()
            .to_string()
            .contains("unavailable in page scripts")
    );
    fs::write(&entry, "import a from './node_modules/native/addon.node';import b from './node_modules/parent/node_modules/native/addon.node';").unwrap();
    assert!(
        compile(&entry, Realm::Main, None, Rc::default(), false, None)
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
            None,
        )
        .unwrap();
        let engine = apps_runtime::jsc::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        engine.load_bundle(&files, "app.js").unwrap();
        assert_eq!(engine.eval("result", "test").unwrap(), expected);
    }
    assert!(
        css(
            &dir.path().join("style.css"),
            None,
            &mut Vec::new(),
            &Rc::default(),
            &Shared::default()
        )
        .is_err()
    );
}

#[test]
fn app_imports_map_bare_specifiers_in_dependencies() {
    let dir = tempfile::tempdir().unwrap();
    for (path, source) in [
        (
            "package.json",
            r##"{"imports":{"heavy":"./fake.js","heavy/*":{"node":"./fake.js","default":"./missing.js"},"swap":"other","#local":"./local.js"}}"##,
        ),
        (
            "app.ts",
            "import { dep } from 'dep'; import { other } from 'swap'; import { local } from '#local'; globalThis.result = [dep, other, local].join(':');",
        ),
        ("fake.js", "export const name = 'fake', core = '!';"),
        ("local.js", "export const local = 'local';"),
        (
            "node_modules/dep/package.json",
            r#"{"type":"module","main":"index.js"}"#,
        ),
        (
            "node_modules/dep/index.js",
            "import { name } from 'heavy'; import { core } from 'heavy/core'; export const dep = name + core;",
        ),
        (
            "node_modules/heavy/package.json",
            r#"{"type":"module","main":"index.js"}"#,
        ),
        ("node_modules/heavy/index.js", "export const name = 'real';"),
        ("node_modules/heavy/core.js", "export const core = '?';"),
        (
            "node_modules/other/package.json",
            r#"{"type":"module","main":"index.js"}"#,
        ),
        (
            "node_modules/other/index.js",
            "export const other = 'other';",
        ),
    ] {
        fs::create_dir_all(dir.path().join(path).parent().unwrap()).unwrap();
        fs::write(dir.path().join(path), source).unwrap();
    }
    let entry = dir.path().join("app.ts");
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
    let engine = apps_runtime::jsc::JsContext::new(
        apps_runtime::jsc::JavaScriptCore::load().unwrap(),
        Rc::default(),
        apps_runtime::host::Role::Main,
    )
    .unwrap();
    engine.load_bundle(&files, "app.js").unwrap();
    assert_eq!(engine.eval("result", "test").unwrap(), "fake!:other:local");
    fs::write(
        dir.path().join("package.json"),
        r#"{"imports":{"swap":"heavy","heavy":"swap"}}"#,
    )
    .unwrap();
    let error = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap_err();
    assert!(format!("{error:#}").contains("maps again"), "{error:#}");
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
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
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

#[test]
fn stylesheets_cannot_close_style_or_read_other_files() {
    let dir = tempfile::tempdir().unwrap();
    let theme = dir.path().join("node_modules/theme");
    fs::create_dir_all(&theme).unwrap();
    fs::write(dir.path().join("secret.txt"), ".secret { color: red }").unwrap();
    fs::write(theme.join("base.css"), "h1 { color: #123456 }").unwrap();
    fs::write(
        theme.join("screen.css"),
        "@IMPORT './base.css'; p { color: #654321 }",
    )
    .unwrap();
    fs::write(
        theme.join("evil.css"),
        "a{}</STYLE><script type=\"preload\">__apps.child_process.exec('touch pwned')</script><style>",
    )
    .unwrap();
    fs::write(theme.join("split.css"), "a{}<@import './tail.css';").unwrap();
    fs::write(
        theme.join("tail.css"),
        "/style><script type=\"preload\">__apps.child_process.exec('touch pwned')</script><style>",
    )
    .unwrap();
    let page = dir.path().join("index.html");
    let entry = dir.path().join("app.ts");
    fs::write(&entry, "console.log(import.meta.resolve('./index.html'));").unwrap();
    fs::write(&page, "<style>@import './node_modules/theme/screen.css'; body { margin: 0 }</style><link rel='stylesheet' media='print' href='./node_modules/theme/base.css'>").unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
    let doc = Document::parse(&files["page-0.html"]);
    let styles: Vec<_> = (0..doc.nodes.len())
        .filter(|&id| doc.nodes[id].tag == "style")
        .map(|id| doc.text(id))
        .collect();
    assert_eq!(styles.len(), 2, "{}", files["page-0.html"]);
    assert!(styles[0].contains("#123456") && styles[0].contains("#654321"));
    assert!(!styles[0].contains("@IMPORT"));
    assert!(styles[1].starts_with("@media print{") && styles[1].contains("#123456"));
    for (html, message) in [
        (
            "<link rel='stylesheet' href='./node_modules/theme/evil.css'>",
            "</style>",
        ),
        (
            "<style>@import './node_modules/theme/evil.css';</style>",
            "</style>",
        ),
        (
            "<link rel='stylesheet' href='./node_modules/theme/split.css'>",
            "</style>",
        ),
        ("<style>@import './secret.txt';</style>", ".css files"),
        ("<link rel='stylesheet' href='./secret.txt'>", ".css files"),
    ] {
        fs::write(&page, html).unwrap();
        let error = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap_err();
        assert!(format!("{error:#}").contains(message), "{html}: {error:#}");
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.path().join("secret.txt"), theme.join("link.css")).unwrap();
        fs::write(
            &page,
            "<link rel='stylesheet' href='./node_modules/theme/link.css'>",
        )
        .unwrap();
        let error = compile(&entry, Realm::Main, None, Rc::default(), true, None).unwrap_err();
        assert!(format!("{error:#}").contains(".css files"), "{error:#}");
    }
}

#[test]
fn worker_cycles_and_repeated_workers_share_script_files() {
    let dir = tempfile::tempdir().unwrap();
    let spawn = |name: &str| format!("new Worker(new URL('./{name}', import.meta.url));");
    fs::write(dir.path().join("a.js"), spawn("b.js")).unwrap();
    fs::write(dir.path().join("b.js"), spawn("a.js")).unwrap();
    fs::write(dir.path().join("self.js"), spawn("self.js")).unwrap();
    fs::write(
        dir.path().join("leaf.js"),
        "postMessage('LEAF_WORKER_MARKER');",
    )
    .unwrap();
    let entry = dir.path().join("app.js");
    for (source, scripts) in [
        (spawn("a.js"), 2),
        (spawn("self.js"), 1),
        (spawn("app.js"), 1),
        (format!("{}{}", spawn("leaf.js"), spawn("leaf.js")), 1),
    ] {
        fs::write(&entry, &source).unwrap();
        for production in [false, true] {
            let files =
                compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
            assert_eq!(
                files
                    .keys()
                    .filter(|name| name.starts_with("script-"))
                    .count(),
                scripts,
                "{source}"
            );
            assert_eq!(
                files["app.js"].matches("apps:///script-0.js").count(),
                source.matches("Worker").count()
            );
            assert!(!files["app.js"].contains("LEAF_WORKER_MARKER"));
        }
    }
    assert_eq!(
        compile(&entry, Realm::Main, None, Rc::default(), true, None).unwrap()["script-0.js"]
            .matches("LEAF_WORKER_MARKER")
            .count(),
        1
    );
}

#[test]
fn production_keeps_imports_that_shadow_globals() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("shadow.js"), "export const String = v => 'S' + v, Boolean = v => 'B' + v, Array = v => 'A' + v, Math = {PI: 3}, parseInt = v => 'P' + v;").unwrap();
    let entry = dir.path().join("app.js");
    fs::write(&entry, "import {String, Boolean, Array, Math, parseInt} from './shadow.js'; export {String}; globalThis.result = [String(1), Boolean(0), Array(2), Math.PI, parseInt('7'), ({String}).String === String].join(':');").unwrap();
    for production in [false, true] {
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        let engine = apps_runtime::jsc::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        engine.load_bundle(&files, "app.js").unwrap();
        assert_eq!(
            engine.eval("result", "test").unwrap(),
            "S1:B0:A2:3:P7:true",
            "production={production}"
        );
    }
}

#[test]
fn json_imports_keep_proto_keys_and_bundles_stay_ascii() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("data.json"),
        r#"{"__proto__": {"polluted": true}, "text": "café 😀 </script>"}"#,
    )
    .unwrap();
    let entry = dir.path().join("app.js");
    fs::write(&entry, "import data from './data.json'; const tag = String.raw`ü`; globalThis.result = [Object.keys(data).join(), data.polluted, Object.getPrototypeOf(data) === Object.prototype, data.text, tag, 'é😀'].join(':');").unwrap();
    for production in [false, true] {
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        assert!(files["app.js"].is_ascii(), "production={production}");
        let engine = apps_runtime::jsc::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        engine.load_bundle(&files, "app.js").unwrap();
        assert_eq!(
            engine.eval("result", "test").unwrap(),
            "__proto__,text::true:café 😀 </script>:ü:é😀",
            "production={production}"
        );
    }
}

#[test]
fn package_folders_css_packages_and_asset_urls() {
    let dir = tempfile::tempdir().unwrap();
    let package = dir.path().join("node_modules/pkg");
    fs::create_dir_all(package.join("constants")).unwrap();
    fs::create_dir_all(package.join("dist")).unwrap();
    fs::write(
        package.join("constants/package.json"),
        r#"{"main":"../dist/constants.js"}"#,
    )
    .unwrap();
    fs::write(package.join("package.json"), r#"{"name":"pkg"}"#).unwrap();
    fs::write(
        package.join("dist/constants.js"),
        "export const answer = 42",
    )
    .unwrap();
    let style = dir.path().join("node_modules/anim");
    fs::create_dir_all(&style).unwrap();
    fs::write(
        style.join("package.json"),
        r#"{"exports":{".":{"style":"./anim.css"}}}"#,
    )
    .unwrap();
    fs::write(style.join("anim.css"), ".spin{color:red}").unwrap();
    fs::write(dir.path().join("logo.png"), b"png").unwrap();
    fs::write(dir.path().join("style.css"), "@import 'anim';").unwrap();
    fs::write(
        dir.path().join("index.html"),
        "<link rel='stylesheet' href='style.css'>",
    )
    .unwrap();
    let entry = dir.path().join("app.js");
    fs::write(
        &entry,
        "import { answer } from 'pkg/constants';
        const logo = new URL('./logo.png', import.meta.url).href;
        if (answer !== 42 || !logo.startsWith('apps:///assets/')) throw Error(logo);
        console.log(import.meta.resolve('./index.html'));",
    )
    .unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
    assert!(files["page-0.html"].contains(".spin{color:red}"));
    let assets: std::collections::BTreeMap<String, PathBuf> =
        serde_json::from_str(&files["assets.json"]).unwrap();
    assert_eq!(assets.len(), 1);
    let engine = apps_runtime::jsc::JsContext::new(
        apps_runtime::jsc::JavaScriptCore::load().unwrap(),
        Rc::default(),
        apps_runtime::host::Role::Main,
    )
    .unwrap();
    engine.load_bundle(&files, "app.js").unwrap();
}

#[test]
fn dynamic_imports_set_up_on_first_use_and_share_static_modules() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("eager.js"),
        "export let count = 0; export const bump = () => count++;",
    )
    .unwrap();
    fs::write(dir.path().join("lazy.js"), "import {bump} from './eager.js'; import {shared} from './shared.js'; export * from './eager.js'; bump(); export const extra = shared;").unwrap();
    fs::write(
        dir.path().join("other.js"),
        "import {shared} from './shared.js'; export const other = shared;",
    )
    .unwrap();
    fs::write(
        dir.path().join("shared.js"),
        "export const shared = 'LAZY_MARKER'.length - 10;",
    )
    .unwrap();
    fs::write(
        dir.path().join("broken.js"),
        "import {missing} from './eager.js'; export default missing;",
    )
    .unwrap();
    let package = dir.path().join("node_modules/pick");
    fs::create_dir_all(&package).unwrap();
    fs::write(package.join("package.json"), r#"{"main":"index.js"}"#).unwrap();
    fs::write(
        package.join("index.js"),
        "module.exports = globalThis.never ? require('./unused.js') : require('./used.js');",
    )
    .unwrap();
    fs::write(package.join("used.js"), "exports.value = 'used';").unwrap();
    fs::write(
        package.join("unused.js"),
        "globalThis.unused = true; exports.value = 'unused';",
    )
    .unwrap();
    let entry = dir.path().join("app.js");
    fs::write(&entry, "import {count} from './eager.js'; import pick from 'pick'; if (globalThis.never) await import('./lazy.js'); const {other} = await import('./other.js'); if (other !== 1) throw Error('other'); const lazy = await import('./lazy.js'); const again = await import('./lazy.js'); const broken = globalThis.dev ? await import('./broken.js').then(() => 'loaded', error => error.message) : 'skipped'; globalThis.result = [count, lazy.count, lazy.extra, lazy === again, pick.value, globalThis.unused ?? false, broken].join(':');").unwrap();
    for production in [false, true] {
        if production {
            fs::write(dir.path().join("broken.js"), "export default 1;").unwrap();
        }
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        assert_eq!(files["app.js"].contains("LAZY_MARKER"), !production);
        assert_eq!(
            files
                .keys()
                .filter(|name| name.starts_with("script-"))
                .count(),
            if production { 3 } else { 0 }
        );
        let engine = apps_runtime::jsc::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        engine
            .eval(&format!("globalThis.dev = {}", !production), "test")
            .unwrap();
        engine.load_bundle(&files, "app.js").unwrap();
        engine.tick().unwrap();
        let broken = if production {
            "skipped"
        } else {
            "Missing export missing from 2"
        };
        assert_eq!(
            engine.eval("result", "test").unwrap(),
            format!("1:1:1:true:used:false:{broken}"),
            "production={production}"
        );
    }
}

#[test]
fn page_scripts_share_one_module_graph_per_realm() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("shared.ts"),
        "globalThis.loads = (globalThis.loads ?? 0) + 1; export const order: string[] = [];",
    )
    .unwrap();
    fs::write(dir.path().join("first.ts"), "import {order} from './shared.ts'; order.push('first'); await new Promise(resolve => setTimeout(resolve, 5)); order.push('late');").unwrap();
    fs::write(dir.path().join("index.html"), r#"<html><head><script type="preload">import {order} from './shared.ts'; order.push('preload');</script><script type="preload">import {order} from './shared.ts'; if (globalThis.loads !== 1 || order.join() !== 'preload') throw Error('preload graph ' + order);</script></head><body><script type="module" src="./first.ts"></script><script type="module">import {order} from './shared.ts'; if ([typeof compile, typeof define, typeof load, typeof setup, typeof modules, typeof chunk, typeof __appsChunk, typeof __appsCompile].some(type => type !== 'undefined')) throw Error('loader leak'); if (globalThis.loads !== 1 || order.join() !== 'first') throw Error('page graph ' + globalThis.loads + order); setTimeout(() => { if (order.join() !== 'first,late') throw Error('late ' + order); document.body.dataset.done = 'yes'; }, 20);</script></body></html>"#).unwrap();
    let entry = dir.path().join("app.ts");
    fs::write(&entry, "import {app, BrowserWindow} from '@pierre/apps'; app.whenReady().then(() => new BrowserWindow().loadFile(import.meta.resolve('./index.html')));").unwrap();
    for production in [false, true] {
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        assert_eq!(
            files
                .keys()
                .filter(|name| name.starts_with("script-"))
                .count(),
            2
        );
        let page = Document::parse(&files["page-0.html"]);
        let sources: Vec<_> = (0..page.nodes.len())
            .filter(|&id| page.nodes[id].tag == "script")
            .map(|id| {
                (
                    page.nodes[id].attrs.get("src").cloned(),
                    page.text(id).is_empty(),
                )
            })
            .collect();
        assert_eq!(
            sources,
            [
                (Some("apps:///script-1.js".into()), true),
                (None, true),
                (Some("apps:///script-0.js".into()), true),
                (None, true)
            ]
        );
        assert_eq!(
            apps_runtime::desktop::Desktop::new(&files)
                .unwrap()
                .check()
                .unwrap(),
            0,
            "production={production}"
        );
    }
}

#[test]
fn watch_cache_reuses_modules_with_stable_ids() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("value.ts"),
        "export const value: number = 1;",
    )
    .unwrap();
    fs::write(
        dir.path().join("other.ts"),
        "export const OTHER_MARKER = 2;",
    )
    .unwrap();
    fs::write(dir.path().join("dep.js"), "export default 'js';").unwrap();
    let entry = dir.path().join("app.ts");
    fs::write(&entry, "import {value} from './value.ts'; import {OTHER_MARKER} from './other.ts'; import dep from './dep'; globalThis.result = [value, OTHER_MARKER, dep].join(':');").unwrap();
    let cache = Rc::new(Cache::default());
    let run = |files: &BBTreeMap<String, String>| {
        let engine = apps_runtime::jsc::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        engine.load_bundle(files, "app.js").unwrap();
        engine.eval("result", "test").unwrap()
    };
    let line = |files: &BBTreeMap<String, String>| {
        files["app.js"]
            .lines()
            .find(|line| line.contains("OTHER_MARKER = 2"))
            .unwrap()
            .to_owned()
    };
    let first = compile(
        &entry,
        Realm::Main,
        None,
        Rc::default(),
        false,
        Some(cache.clone()),
    )
    .unwrap();
    assert_eq!(run(&first), "1:2:js");
    fs::write(
        dir.path().join("value.ts"),
        "export const value: number = 30;",
    )
    .unwrap();
    let watched = Rc::default();
    let second = compile(
        &entry,
        Realm::Main,
        None,
        Rc::clone(&watched),
        false,
        Some(cache.clone()),
    )
    .unwrap();
    assert_eq!(run(&second), "30:2:js");
    assert_eq!(line(&first), line(&second));
    assert!(
        watched
            .borrow()
            .contains(&dir.path().join("other.ts").canonicalize().unwrap())
    );
    fs::write(&entry, "import {OTHER_MARKER} from './other.ts'; import dep from './dep'; import {value} from './value.ts'; globalThis.result = [value, OTHER_MARKER, dep].join(':');").unwrap();
    fs::write(dir.path().join("dep.ts"), "export default 'ts';").unwrap();
    cache.resolutions.borrow_mut().clear();
    let third = compile(
        &entry,
        Realm::Main,
        None,
        Rc::default(),
        false,
        Some(cache.clone()),
    )
    .unwrap();
    assert_eq!(run(&third), "30:2:ts");
    assert_eq!(line(&first), line(&third));
}

#[test]
fn production_drops_unused_modules_of_side_effect_free_packages() {
    let dir = tempfile::tempdir().unwrap();
    let icons = dir.path().join("node_modules/icons");
    fs::create_dir_all(icons.join("dist/icons")).unwrap();
    fs::write(icons.join("package.json"), r#"{"name":"icons","type":"module","main":"dist/index.js","sideEffects":["./dist/register.js","*.effect.js"]}"#).unwrap();
    fs::write(icons.join("dist/index.js"), "export {default as Used} from './icons/used.js'; export {default as Unused} from './icons/unused.js'; export * from './icons/starred.js'; import './register.js'; import './tag.effect.js'; import './plain.js'; export {empty} from './empty.js';").unwrap();
    fs::write(
        icons.join("dist/icons/used.js"),
        "export default 'USED_ICON';",
    )
    .unwrap();
    fs::write(
        icons.join("dist/icons/unused.js"),
        "import '../deep.js'; export default 'UNUSED_ICON';",
    )
    .unwrap();
    fs::write(
        icons.join("dist/deep.js"),
        "globalThis.deep = 'DEEP_MARKER';",
    )
    .unwrap();
    fs::write(
        icons.join("dist/icons/starred.js"),
        "export const starred = 'STARRED_ICON'; export const other = 'OTHER_STAR';",
    )
    .unwrap();
    fs::write(
        icons.join("dist/register.js"),
        "globalThis.registered = true;",
    )
    .unwrap();
    fs::write(
        icons.join("dist/tag.effect.js"),
        "globalThis.tagged = true;",
    )
    .unwrap();
    fs::write(
        icons.join("dist/plain.js"),
        "globalThis.plain = 'PLAIN_MARKER';",
    )
    .unwrap();
    fs::write(icons.join("dist/empty.js"), "export const empty = {};").unwrap();
    let entry = dir.path().join("app.js");
    fs::write(&entry, "import {Used, starred} from 'icons'; import * as all from './spread.js'; globalThis.result = [Used, starred, globalThis.registered, globalThis.tagged, globalThis.plain, globalThis.deep, Object.keys(all).length].join(':');").unwrap();
    fs::write(
        dir.path().join("spread.js"),
        "export * from 'icons/dist/empty.js';",
    )
    .unwrap();
    for production in [false, true] {
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        for marker in ["UNUSED_ICON", "DEEP_MARKER", "PLAIN_MARKER", "OTHER_STAR"] {
            assert_eq!(files["app.js"].contains(marker), !production, "{marker}");
        }
        let engine = apps_runtime::jsc::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        engine.load_bundle(&files, "app.js").unwrap();
        let expected = if production {
            "USED_ICON:STARRED_ICON:true:true:::1"
        } else {
            "USED_ICON:STARRED_ICON:true:true:PLAIN_MARKER:DEEP_MARKER:1"
        };
        assert_eq!(
            engine.eval("result", "test").unwrap(),
            expected,
            "production={production}"
        );
    }
    assert!(
        glob(b"**/dist/*.js", b"a/dist/x.js")
            && glob(b"**/x.js", b"x.js")
            && !glob(b"dist/*.js", b"dist/a/x.js")
            && glob(b"dist/**", b"dist/a/x.js")
    );
}

#[test]
fn page_workers_and_chunks_load_from_bundled_scripts() {
    use apps_runtime::host::{Host, Page, Role};
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("worker.ts"),
        "const {value} = await import('./value.ts'); postMessage(value);",
    )
    .unwrap();
    fs::write(
        dir.path().join("value.ts"),
        "export const value: number = 42;",
    )
    .unwrap();
    let entry = dir.path().join("page.ts");
    fs::write(&entry, "const worker = new Worker(new URL('./worker.ts', import.meta.url)); worker.addEventListener('message', async event => { const {value} = await import('./value.ts'); globalThis.result = event.data + value; worker.terminate(); });").unwrap();
    for production in [false, true] {
        let files = compile(
            &entry,
            Realm::Renderer,
            None,
            Rc::default(),
            production,
            None,
        )
        .unwrap();
        assert!(!files["app.js"].contains("data:"));
        let host = Rc::new(RefCell::new(Host::default()));
        host.borrow_mut().pages.insert(
            1,
            Page {
                doc: Document::parse("<html><body></body></html>"),
                dirty: false,
            },
        );
        let engine = apps_runtime::jsc::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            host,
            Role::Renderer(1),
        )
        .unwrap();
        engine.load_bundle(&files, "page").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while engine
            .eval("globalThis.result ?? 'pending'", "test")
            .unwrap()
            == "pending"
        {
            engine.tick().unwrap();
            assert!(
                std::time::Instant::now() < deadline,
                "production={production}"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(
            engine.eval("result", "test").unwrap(),
            "84",
            "production={production}"
        );
    }
}

#[test]
fn css_imports_keep_layer_supports_and_media_conditions() {
    let dir = tempfile::tempdir().unwrap();
    for (name, rule) in [
        ("a.css", ".a{}"),
        ("b.css", ".b{}"),
        ("c.css", ".c{}"),
        ("d.css", ".d{}"),
        ("e.css", ".e{}"),
    ] {
        fs::write(dir.path().join(name), rule).unwrap();
    }
    fs::write(
        dir.path().join("style.css"),
        "@import \"./a.css\" layer(base);@import url(./b.css) layer;@import './c.css' supports(display: grid) screen and (min-width: 1px);@import url('./d.css') print;@import './e.css' layer(x.y) supports(not (display: nonsense));",
    )
    .unwrap();
    fs::write(
        dir.path().join("index.html"),
        "<link rel='stylesheet' href='./style.css'>",
    )
    .unwrap();
    let entry = dir.path().join("app.ts");
    fs::write(&entry, "console.log(import.meta.resolve('./index.html'));").unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
    let page = &files["page-0.html"];
    for expected in [
        "@layer base{.a{}}",
        "@layer{.b{}}",
        "@media screen and (min-width: 1px){@supports (display: grid){.c{}}}",
        "@media print{.d{}}",
        "@supports (not (display: nonsense)){@layer x.y{.e{}}}",
    ] {
        assert!(page.contains(expected), "{expected} in {page}");
    }
}

#[test]
fn http_imports_download_with_the_feature() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for mut socket in listener.incoming().flatten() {
            let (mut request, mut byte) = (Vec::new(), [0]);
            while !request.ends_with(b"\r\n\r\n") && socket.read_exact(&mut byte).is_ok() {
                request.push(byte[0]);
            }
            let body = match String::from_utf8_lossy(&request)
                .split(' ')
                .nth(1)
                .unwrap_or("")
            {
                "/lib/a.js" => {
                    "import b from './b.js'; import c from '/c.js'; export default b + c; export const url = import.meta.url;"
                }
                "/lib/b.js" => "export default 40",
                "/c.js" => "export default 2",
                "/style.css" => "@import './more.css'; .a{background:url(img/a.png)}",
                "/more.css" => ".b{color:red}",
                _ => "",
            };
            let _ = write!(
                socket,
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("main.js");
    fs::write(
        &entry,
        format!("import a, {{url}} from '{origin}/lib/a.js'; globalThis.result = a + ' ' + url; globalThis.page = import.meta.resolve('./index.html'); globalThis.later = () => import(globalThis.name);"),
    )
    .unwrap();
    fs::write(
        dir.path().join("index.html"),
        format!(r#"<link rel="stylesheet" href="{origin}/style.css"><script type="module" src="{origin}/c.js"></script>"#),
    )
    .unwrap();
    let package = dir.path().join("package.json");
    fs::write(&package, r#"{"name":"remote"}"#).unwrap();
    let error = compile(&entry, Realm::Main, None, Rc::default(), true, None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("httpImport"), "{error}");
    let dynamic = dir.path().join("dynamic.js");
    fs::write(&dynamic, "import(globalThis.name)").unwrap();
    let error = compile(&dynamic, Realm::Main, None, Rc::default(), true, None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("jsExecute or httpImport"), "{error}");
    fs::write(&package, r#"{"name":"remote","features":["httpImport"]}"#).unwrap();
    fs::write(&dynamic, "import('data:text/javascript,export default 1')").unwrap();
    let error = compile(&dynamic, Realm::Main, None, Rc::default(), true, None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("needs the jsExecute feature"), "{error}");
    for production in [false, true] {
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        if production {
            let page = &files["page-0.html"];
            assert!(
                page.contains(".b{color:red}") && page.contains(&format!("\"{origin}/img/a.png\"")),
                "{page}"
            );
        }
        let engine = apps_runtime::jsc::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        engine.load_bundle(&files, "app.js").unwrap();
        assert_eq!(
            engine.eval("result", "test").unwrap(),
            format!("42 {origin}/lib/a.js")
        );
    }
}

#[test]
fn classic_scripts_share_the_global_scope_and_run_before_modules() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("app");
    for folder in ["web/vendor", "src"] {
        fs::create_dir_all(root.join(folder)).unwrap();
    }
    for (name, source) in [
        (
            "web/vendor/lib.umd.js",
            "(function (root, factory) { if (typeof module === 'object' && module.exports) module.exports = factory(); else root.Lib = factory(); })(this, function () { return { answer: 42 }; });",
        ),
        (
            "web/a.js",
            "function helper() { return 'helper'; } var shared = 1; implicit = 2;",
        ),
        (
            "web/b.js",
            "if (helper() !== 'helper' || shared !== 1 || implicit !== 2 || this !== globalThis || Lib.answer !== 42 || (function () { return this; })() !== globalThis) throw Error('classic'); try { eval('1'); throw Error('eval'); } catch (error) { if (!(error instanceof EvalError)) throw error; } order.push('classic');",
        ),
        (
            "src/main.ts",
            "const order: string[] = (globalThis as any).order; if (order.join() !== 'inline,classic') throw Error('module ' + order); order.push('module');",
        ),
        ("web/deferred.js", "order.push('deferred');"),
        ("web/style.css", "@import '/web/base.css';"),
        ("web/base.css", "#color { color: rgb(1, 2, 3) }"),
        ("web/alt.css", "#color { color: rgb(9, 9, 9) }"),
    ] {
        fs::write(root.join(name), source).unwrap();
    }
    fs::write(dir.path().join("outside.js"), "").unwrap();
    fs::write(root.join("web/index.html"), r#"<link rel="Preload  Stylesheet" href="/web/style.css"><link rel="alternate stylesheet" href="/web/alt.css"><p id="color">x</p><script type="module" src="/src/main.ts"></script><script>globalThis.order = ['inline'];</script><script src="vendor/lib.umd.js"></script><script src="a.js"></script><script src="b.js"></script><script nomodule>throw Error('nomodule');</script><script defer src="deferred.js"></script><script type="module">if (order.join() !== 'inline,classic,module,deferred' || getComputedStyle(document.getElementById('color')).color !== 'rgb(1, 2, 3)') throw Error('order ' + order);</script>"#).unwrap();
    let entry = root.join("main.ts");
    fs::write(&entry, "import {app, BrowserWindow} from '@pierre/apps'; app.whenReady().then(() => new BrowserWindow().loadFile(import.meta.resolve('./web/index.html')));").unwrap();
    for production in [false, true] {
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        assert!(!files.values().any(|file| file.contains("nomodule')")));
        assert_eq!(
            apps_runtime::desktop::Desktop::new(&files)
                .unwrap()
                .check()
                .unwrap(),
            0,
            "production={production}"
        );
    }
    fs::write(
        root.join("web/index.html"),
        r#"<script src="/../outside.js"></script>"#,
    )
    .unwrap();
    let error = compile(&entry, Realm::Main, None, Rc::default(), true, None).unwrap_err();
    assert!(
        error.to_string().contains("outside the app folder"),
        "{error}"
    );
}

#[test]
fn exports_fall_back_to_the_next_condition_and_module_bindings_are_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let modules = dir.path().join("node_modules");
    fs::create_dir_all(modules.join("fallback")).unwrap();
    fs::create_dir_all(modules.join("consumer")).unwrap();
    for (path, source) in [
        (
            "fallback/package.json",
            r#"{"exports":{".":{"node":{"import":"./a.mjs"},"default":"./b.cjs"}}}"#,
        ),
        ("fallback/a.mjs", "export default 'a';"),
        ("fallback/b.cjs", "module.exports = 'b';"),
        ("consumer/package.json", r#"{"main":"index.js"}"#),
        ("consumer/index.js", "module.exports = require('fallback');"),
    ] {
        fs::write(modules.join(path), source).unwrap();
    }
    let entry = dir.path().join("app.js");
    fs::write(&entry, "import a from 'fallback'; import b from 'consumer'; const plugins = [{exports: new Set([1, 2])}]; let size = 0; for (const module of plugins) size += module.exports.size; globalThis.result = [a, b, size].join(':');").unwrap();
    for production in [false, true] {
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        let engine = apps_runtime::jsc::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::host::Role::Main,
        )
        .unwrap();
        engine.load_bundle(&files, "app.js").unwrap();
        assert_eq!(engine.eval("result", "test").unwrap(), "a:b:2");
    }
}

#[test]
fn import_meta_filename_and_dirname_follow_node() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let entry = root.join("app.js");
    fs::write(&entry, "globalThis.result = typeof import.meta.filename === 'string' ? JSON.stringify([import.meta.filename === globalThis.__apps.url.fileURLToPath(import.meta.url), import.meta.dirname === globalThis.__apps.path.dirname(import.meta.filename), import.meta.filename]) : typeof import.meta.dirname;").unwrap();
    for production in [false, true] {
        for (realm, role) in [
            (Realm::Main, apps_runtime::host::Role::Main),
            (Realm::Preload, apps_runtime::host::Role::Preload(1)),
            (Realm::Renderer, apps_runtime::host::Role::Renderer(1)),
        ] {
            let files = compile(&entry, realm, None, Rc::default(), production, None).unwrap();
            let host = Rc::new(RefCell::new(apps_runtime::host::Host::default()));
            host.borrow_mut().pages.insert(
                1,
                apps_runtime::host::Page {
                    doc: Document::parse("<body></body>"),
                    dirty: false,
                },
            );
            let engine = apps_runtime::jsc::JsContext::new(
                apps_runtime::jsc::JavaScriptCore::load().unwrap(),
                host,
                role,
            )
            .unwrap();
            engine.load_bundle(&files, "app.js").unwrap();
            let result = engine.eval("result", "test").unwrap();
            let expected = match realm {
                Realm::Renderer => "undefined".to_owned(),
                _ if production => result.clone(),
                _ => json!([true, true, entry]).to_string(),
            };
            assert_eq!(result, expected, "production={production}, realm={realm:?}");
            assert!(
                realm == Realm::Renderer || result.starts_with("[true,true,"),
                "{result}"
            );
        }
    }
}

#[test]
fn css_urls_skip_comments_and_strings_and_asset_names_are_stable() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("img")).unwrap();
    fs::create_dir_all(root.join("web")).unwrap();
    fs::write(root.join("img/a.png"), b"a").unwrap();
    fs::write(root.join("img/b.png"), b"b").unwrap();
    let a = asset_name(&root, &root.join("img/a.png")).unwrap();
    let b = asset_name(&root, &root.join("img/b.png")).unwrap();
    assert_eq!(a, "assets/5e9e943b2565fff5.png");
    assert_eq!(
        asset_name(Path::new("/elsewhere"), Path::new("/elsewhere/img/a.png")).unwrap(),
        a
    );
    assert_eq!(
        app_relative(
            Path::new("/repo/app"),
            Path::new("/repo/packages/a/index.js")
        ),
        Path::new("../packages/a/index.js")
    );
    let css = urls(
        "/* url( */ .a{background:URL( '../img/a.png' )} .b::after{content:'url('} .c{background:url(../img/b.png)}",
        &root.join("web"),
        &Rc::default(),
        &root,
    );
    assert_eq!(
        css,
        format!(
            "/* url( */ .a{{background:URL(\"apps:///{a}\")}} .b::after{{content:'url('}} .c{{background:url(\"apps:///{b}\")}}"
        )
    );
    fs::write(root.join("web/index.html"), r#"<img srcset="../img/a.png 1x,../img/b.png, data:image/png;base64,AA== 3x"><picture><source srcset=" ../img/b.png 2x"></picture>"#).unwrap();
    let entry = root.join("app.js");
    fs::write(
        &entry,
        "console.log(import.meta.resolve('./web/index.html'));",
    )
    .unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), true, None).unwrap();
    let page = &files["page-0.html"];
    assert!(
        page.contains(&format!(
            "srcset=\"apps:///{a} 1x,apps:///{b}, data:image/png;base64,AA== 3x\""
        )) && page.contains(&format!("srcset=\" apps:///{b} 2x\"")),
        "{page}"
    );
}
