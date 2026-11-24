use super::*;

#[test]
fn react_compiler_memoizes_page_components() {
    let dir = tempfile::tempdir().unwrap();
    let react = dir.path().join("node_modules/react");
    fs::create_dir_all(&react).unwrap();
    fs::write(
        react.join("package.json"),
        r#"{"main":"index.js","version":"18.3.1"}"#,
    )
    .unwrap();
    fs::write(react.join("index.js"), "").unwrap();
    fs::write(react.join("compiler-runtime.js"), "let cache; exports.c = size => cache ??= new Array(size).fill(Symbol.for('react.memo_cache_sentinel'));").unwrap();
    for name in ["jsx-runtime.js", "jsx-dev-runtime.js"] {
        fs::write(
            react.join(name),
            "exports.jsx = exports.jsxs = exports.jsxDEV = (type, props) => ({type, props});",
        )
        .unwrap();
    }
    fs::write(
        react.join("dependency.jsx"),
        "export function Dependency() { return <i />; }",
    )
    .unwrap();
    let entry = dir.path().join("page.tsx");
    fs::write(&entry, "import {Dependency} from 'react/dependency.jsx'; function Greeting({name}: {name: string}) { return <b>{name.toUpperCase()}</b>; } const a = Greeting({name: 'x'}); globalThis.result = [a === Greeting({name: 'x'}), a === Greeting({name: 'y'}), Dependency() === Dependency()].join(':');").unwrap();
    for (features, expected) in [
        ("[]", "false:false:false"),
        (r#"["reactCompiler"]"#, "true:false:false"),
    ] {
        fs::write(
            dir.path().join("package.json"),
            format!(r#"{{"name":"react_compiler","features":{features}}}"#),
        )
        .unwrap();
        if features != "[]" {
            let error = compile(&entry, Realm::Renderer, None, Rc::default(), false, None)
                .unwrap_err()
                .to_string();
            assert!(error.contains("React 18.3.1"), "{error}");
            fs::write(
                react.join("package.json"),
                r#"{"main":"index.js","version":"19.0.0"}"#,
            )
            .unwrap();
        }
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
            let engine = apps_runtime::jsc::JsContext::new(
                apps_runtime::jsc::JavaScriptCore::load().unwrap(),
                Rc::default(),
                apps_runtime::host::Role::Main,
            )
            .unwrap();
            engine.load_bundle(&files, "test").unwrap();
            assert_eq!(
                engine.eval("result", "test").unwrap(),
                expected,
                "{features}"
            );
        }
    }
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
fn unsupported_electron_exports_fail_at_build_time() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("main.js");
    for (source, error) in [
        (
            "import {app, crashReporter} from 'electron'; app.quit();",
            "electron.crashReporter is not supported",
        ),
        (
            "import {MessageChannelMain} from '@pierre/apps';",
            "@pierre/apps.MessageChannelMain is not supported",
        ),
        (
            "export {app, crashReporter} from 'electron';",
            "electron.crashReporter is not supported",
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
    fs::write(&entry, "import electron, {screen, MenuItem, Tray, webContents, powerSaveBlocker, systemPreferences, webUtils, net} from 'electron'; import {Tray as AppsTray} from '@pierre/apps'; import {Socket} from 'node:net'; if (electron.Tray !== Tray || AppsTray !== Tray || typeof Tray !== 'function' || electron.screen !== screen || typeof MenuItem !== 'function' || typeof webContents.fromId !== 'function' || typeof powerSaveBlocker.start !== 'function' || typeof systemPreferences.getColor !== 'function' || typeof webUtils.getPathForFile !== 'function' || typeof net.request !== 'function' || electron.net !== net || typeof Socket !== 'function') throw Error('exports');").unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), true, None).unwrap();
    apps_runtime::desktop::Desktop::new(&files).unwrap();
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
    let modules: Vec<_> = BUILTINS
        .lines()
        .filter_map(|line| line.split(' ').next())
        .map(|name| {
            if name.starts_with("node:") || PACKAGES.contains(&name) {
                name.to_owned()
            } else {
                format!("node:{name}")
            }
        })
        .collect();
    assert!(modules.len() > 40 && modules.contains(&"node:fs".to_owned()));
    let mut source = String::new();
    for (index, name) in modules.iter().enumerate() {
        source += &format!("import * as m{index} from '{name}';");
    }
    source += "const require=__apps.module.createRequire(),problems=[];";
    for (index, name) in modules.iter().enumerate() {
        source += &format!(
            "{{const object=require('{name}'),names=Object.keys(m{index}).filter(key=>key!=='default'&&key!=='module.exports');if(object!==m{index}.default)problems.push('{name}: default');for(const key of names)if(!(key in object))problems.push('{name}: missing '+key);if('{name}'!=='node:constants')for(const key of Object.keys(object))if(!key.startsWith('_')&&!names.includes(key))problems.push('{name}: not importable '+key);}}"
        );
    }
    source += "import {on, emit} from 'process';import {readFileSync} from 'fs';if(typeof on!=='function'||typeof emit!=='function'||typeof readFileSync!=='function')problems.push('named imports');";
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
        let mut host = apps_runtime::host::Host::default();
        host.disabled = disabled;
        let host = Rc::new(RefCell::new(host));
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

// After runtime exports change, regenerate builtins.txt with `APPS_UPDATE_BUILTINS=1 cargo test -p apps
// builtin_table`.
#[test]
fn builtin_table_matches_the_runtime() {
    let engine = apps_runtime::jsc::JsContext::new(
        apps_runtime::jsc::JavaScriptCore::load().unwrap(),
        Rc::default(),
        apps_runtime::host::Role::Main,
    )
    .unwrap();
    let table = engine
        .eval(
            r#"(() => {
  const require = __apps.module.createRequire(), lines = [], packages = [['node-pty', 'pty'], ['undici', 'undici'], ['ws', 'ws']];
  const reserved = new Set('default break case catch class const continue debugger delete do else enum export extends false finally for function if import in instanceof new null return super switch this throw true try typeof var void while with yield let static implements interface package private protected public await arguments eval'.split(' '));
  const names = value => {
    const all = Object.keys(value);
    if (typeof value === 'object') for (let proto = Object.getPrototypeOf(value); proto && proto !== Object.prototype; proto = Object.getPrototypeOf(proto))
      for (const name of Object.getOwnPropertyNames(proto)) if (name !== 'constructor' && typeof Object.getOwnPropertyDescriptor(proto, name).value === 'function') all.push(name);
    return [...new Set(all)].filter(name => /^[A-Za-z$][\w$]*$/.test(name) && !name.startsWith('_') && !reserved.has(name));
  };
  const object = (key, value) => {
    if (__apps[key] === value) return 'globalThis.__apps.' + key;
    const slash = key.lastIndexOf('/');
    if (slash > 0) { const base = key.slice(0, slash), sub = key.slice(slash + 1); if (require(base)[sub] === value) return object(base, require(base)) + '.' + sub; }
    const other = Object.keys(__apps).find(name => __apps[name] === value);
    return other ? 'globalThis.__apps.' + other : `globalThis.__apps.module.createRequire()(${JSON.stringify(key)})`;
  };
  for (const name of [...__apps.module.builtinModules].sort()) {
    const key = name.replace(/^node:/, ''), value = require(name);
    if (!packages.some(([name]) => name === key)) lines.push([name, object(key, value), ...(key === 'constants' ? [] : names(value))].join(' '));
  }
  for (const [name, key] of packages) lines.push([name, 'globalThis.__apps.' + key, ...names(__apps[key])].join(' '));
  return lines.join('\n') + '\n';
})()"#,
            "builtins",
        )
        .unwrap();
    if std::env::var_os("APPS_UPDATE_BUILTINS").is_some() {
        fs::write(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bundler/builtins.txt"),
            &table,
        )
        .unwrap();
    } else {
        assert!(
            table == BUILTINS,
            "The runtime's built-in modules changed. Run APPS_UPDATE_BUILTINS=1 cargo test -p apps builtin_table"
        );
    }
}
