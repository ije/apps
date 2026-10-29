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
            let engine =
                apps_runtime::jsc::Engine::new(apps_runtime::jsc::Api::load().unwrap(), host, role)
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
        let engine = Engine::new(Api::load().unwrap(), host.clone(), Role::Renderer(1)).unwrap();
        let entry = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples/react/page.tsx")
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
            .join("examples/three")
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
        let engine = Engine::new(Api::load().unwrap(), host.clone(), Role::Renderer(1)).unwrap();
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
    let style = apps_runtime::style::styles(&doc, 800., 600.).pop().unwrap();
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
        let files = compile(&entry, Realm::Main, None, Rc::clone(&watched), production).unwrap();
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
        let error = compile(&entry, realm, Some(source.into()), Rc::default(), false).unwrap_err();
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
            let engine =
                apps_runtime::jsc::Engine::new(apps_runtime::jsc::Api::load().unwrap(), host, role)
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

#[test]
fn node_builtin_exports_exist() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("app.ts");
    let modules = [
        "assert",
        "assert/strict",
        "buffer",
        "child_process",
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
        "string_decoder",
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
    source += "const missing=[";
    for (index, name) in modules.iter().enumerate() {
        source += &format!(
            "...Object.keys(m{index}).filter(key=>m{index}[key]===undefined).map(key=>'{name}.'+key),"
        );
    }
    source += "];globalThis.result=missing.join(', ')||'ok';";
    fs::write(&entry, source).unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), false).unwrap();
    let engine = apps_runtime::jsc::Engine::new(
        apps_runtime::jsc::Api::load().unwrap(),
        std::rc::Rc::new(std::cell::RefCell::new(apps_runtime::host::Host::default())),
        apps_runtime::host::Role::Main,
    )
    .unwrap();
    engine.bundle(&files, "app.js").unwrap();
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
        apps_runtime::jsc::Engine::new(api, host, apps_runtime::host::Role::Preload(1)).unwrap();
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
            include_str!("../../examples/child-process/main.js")
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
            include_str!("../../examples/wasm/main.js")
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
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/runtime/tests/napi.c"))
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
        let files = compile(&entry, Realm::Main, None, Rc::clone(&watched), production).unwrap();
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
