use super::*;

#[test]
fn import_meta_resolve_makes_a_script_chunk() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("main.js");
    fs::write(dir.path().join("server.ts"), "console.log(1 as number);").unwrap();
    let files = compile(
        &entry,
        Realm::Main,
        Some("globalThis.url = import.meta.resolve('./server.ts');".into()),
        Rc::default(),
        false,
        None,
    )
    .unwrap();
    assert!(files["app.js"].contains("apps:///script-0.js"));
    assert!(files.contains_key("script-0.js"));
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
            "import.meta.resolve('./missing.js')",
            Realm::Main,
            "Cannot resolve",
        ),
        (
            "import.meta.resolve('https://example.com/index.html')",
            Realm::Main,
            "local path",
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
            (Realm::Main, apps_runtime::jsc::Role::Main),
            (Realm::Preload, apps_runtime::jsc::Role::Preload(1)),
            (Realm::Renderer, apps_runtime::jsc::Role::Renderer(1)),
        ] {
            let files = compile(&entry, realm, None, Rc::default(), production, None).unwrap();
            let host = Rc::new(RefCell::new(apps_runtime::host::Host::default()));
            host.borrow().store.borrow_mut().pages.insert(
                1,
                apps_runtime::render::Page {
                    doc: Document::parse("<body></body>"),
                    dirty: false,
                    ..Default::default()
                },
            );
            let engine = apps_runtime::host::context::JsContext::new(
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
        let engine = apps_runtime::host::context::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::jsc::Role::Main,
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
        let engine = apps_runtime::host::context::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::jsc::Role::Main,
        )
        .unwrap();
        if let Err(error) = engine.load_bundle(&files, "app.js") {
            panic!("production={production}: {error:#}");
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
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
fn main_workers_share_pages_and_skip_package_metadata() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("index.html"), "<p>main-page</p>").unwrap();
    fs::write(dir.path().join("other.html"), "<p>other-page</p>").unwrap();
    fs::write(
        dir.path().join("worker.js"),
        "postMessage(import.meta.resolve('./other.html'));",
    )
    .unwrap();
    let package = dir.path().join("node_modules/some-pkg");
    fs::create_dir_all(&package).unwrap();
    fs::write(
        package.join("package.json"),
        r#"{"name":"some-pkg","version":"1.0.0"}"#,
    )
    .unwrap();
    fs::write(package.join("worker.js"), "postMessage(1);").unwrap();
    fs::write(
        dir.path().join("a.html"),
        "<p>preload-page</p><script type='preload'>new Worker(new URL('./page-worker.js', import.meta.url));</script>",
    )
    .unwrap();
    fs::write(
        dir.path().join("page-worker.js"),
        "postMessage(import.meta.resolve('./a.html'));",
    )
    .unwrap();
    let entry = dir.path().join("app.js");
    fs::write(
        &entry,
        "globalThis.page = import.meta.resolve('./index.html'); new Worker(new URL('./worker.js', import.meta.url)); new Worker(import.meta.resolve('some-pkg/worker.js')); if (globalThis.window) window.loadFile('a.html');",
    )
    .unwrap();
    for production in [false, true] {
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        let pages: Vec<_> = files
            .iter()
            .filter(|(name, _)| name.ends_with(".html"))
            .collect();
        assert_eq!(pages.len(), 3, "production={production}");
        let (other, _) = pages
            .iter()
            .find(|(_, source)| source.contains("other-page"))
            .unwrap();
        assert!(
            files
                .iter()
                .any(|(name, source)| name.starts_with("script-")
                    && source.contains(&format!("apps:///{other}"))),
            "production={production}"
        );
        let (preload, _) = pages
            .iter()
            .find(|(_, source)| source.contains("preload-page"))
            .unwrap();
        assert!(
            files
                .iter()
                .any(|(name, source)| name.starts_with("script-")
                    && source.contains(&format!("apps:///{preload}"))),
            "production={production}"
        );
        for (_, source) in &files {
            for (at, _) in source.match_indices("apps:///page~") {
                let name = &source[at + 8..];
                let name = &name[..name.find(".html").unwrap() + 5];
                assert!(files.contains_key(name), "{name} production={production}");
            }
        }
    }
}

#[test]
fn main_workers_from_paths_known_at_build_time_get_node_apis() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("lib")).unwrap();
    fs::write(
        dir.path().join("lib/worker.ts"),
        "import {basename} from 'node:path'; onmessage = event => postMessage(basename(event.data) + typeof require);",
    )
    .unwrap();
    let entry = dir.path().join("app.ts");
    fs::write(&entry, r#"import path from 'node:path';
import {fileURLToPath} from 'node:url';
import * as threads from 'node:worker_threads';
import {Worker as Thread} from 'node:worker_threads';
if (threads.isMainThread) {
    const workers = [
        new Thread(new URL(import.meta.url)),
        new Worker(new URL('./lib/worker.ts', import.meta.url).href),
        new Worker(String(new URL('./worker.ts', new URL('./lib/worker.ts', import.meta.url)))),
        new Worker(fileURLToPath(new URL('./lib/worker.ts', import.meta.url))),
        new Worker(import.meta.resolve('./lib/worker.ts')),
        new Worker(path.join(__dirname, 'lib', 'worker.ts')),
        new Worker(path.resolve(path.dirname(__filename), './lib/worker')),
        new Worker(import.meta.dirname + '/lib/worker.ts'),
        new Worker(`${__dirname}/lib/worker.ts`),
        new threads.Worker(new URL(import.meta.url)),
        new threads.Worker(__filename),
    ];
    const replies = await Promise.all(workers.map((worker, index) => new Promise((resolve, reject) => {
        worker.onmessage = event => resolve(event.data);
        worker.onerror = event => reject(event.error);
        worker.postMessage('/folder/file-' + index);
    })));
    await Promise.all(workers.map(worker => worker.terminate()));
    globalThis.result = replies.join();
} else threads.parentPort.on('message', value => threads.parentPort.postMessage(path.basename(value) + 'function'));
"#).unwrap();
    for production in [false, true] {
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        assert_eq!(
            files
                .iter()
                .filter(|(name, _)| name.starts_with("script-"))
                .count(),
            2,
            "production={production}"
        );
        let engine = apps_runtime::host::context::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::jsc::Role::Main,
        )
        .unwrap();
        engine.load_bundle(&files, "app.js").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
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
            (0..11)
                .map(|index| format!("file-{index}function"))
                .collect::<Vec<_>>()
                .join(","),
            "production={production}"
        );
    }
    fs::write(&entry, "new Worker(__dirname + '/missing.ts');").unwrap();
    let error = compile(&entry, Realm::Main, None, Rc::default(), true, None).unwrap_err();
    assert!(format!("{error:#}").contains("Cannot find the worker"));
    fs::create_dir_all(dir.path().join("node_modules/package")).unwrap();
    fs::write(dir.path().join("node_modules/package/package.json"), "{}").unwrap();
    fs::write(dir.path().join("node_modules/package/worker.js"), "").unwrap();
    for source in [
        "require.resolve('package/worker.js')",
        "require.resolve('missing/worker.js')",
    ] {
        fs::write(&entry, format!("new Worker({source});")).unwrap();
        let files = compile(&entry, Realm::Main, None, Rc::default(), true, None).unwrap();
        assert_eq!(
            files.keys().any(|name| name.starts_with("script-")),
            source.contains("package"),
            "{source}"
        );
    }
    fs::write(&entry, "import {fork as start} from 'node:child_process'; import {utilityProcess} from 'electron'; import path from 'node:path'; start(path.join(__dirname, 'lib', 'worker.ts')); utilityProcess.fork(import.meta.dirname + '/lib/worker.ts', []); saga.fork(() => 1);").unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), true, None).unwrap();
    assert_eq!(files["app.js"].matches("apps:///script-0.js").count(), 2);
    assert!(files.contains_key("script-0.js") && !files.contains_key("script-1.js"));
}

#[test]
fn page_workers_and_chunks_load_from_bundled_scripts() {
    use apps_runtime::{host::Host, jsc::Role, render::Page};
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("worker.ts"),
        "const {value} = await import('./value.ts'); postMessage(typeof require === 'undefined' && typeof process === 'undefined' && !globalThis.__apps ? value : 0);",
    )
    .unwrap();
    fs::write(dir.path().join("node.ts"), "import 'node:fs';").unwrap();
    let node = dir.path().join("node-page.ts");
    fs::write(&node, "new Worker(new URL('./node.ts', import.meta.url));").unwrap();
    let error = compile(&node, Realm::Renderer, None, Rc::default(), true, None).unwrap_err();
    assert!(format!("{error:#}").contains("node:fs is unavailable in page scripts"));
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
        host.borrow().store.borrow_mut().pages.insert(
            1,
            Page {
                doc: Document::parse("<html><body></body></html>"),
                dirty: false,
                ..Default::default()
            },
        );
        let engine = apps_runtime::host::context::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            host,
            Role::Renderer(1),
        )
        .unwrap();
        engine.load_bundle(&files, "page").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
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
fn import_meta_filename_and_dirname_follow_node() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let entry = root.join("app.js");
    fs::write(&entry, "globalThis.result = typeof import.meta.filename === 'string' ? JSON.stringify([import.meta.filename === globalThis.__apps.url.fileURLToPath(import.meta.url), import.meta.dirname === globalThis.__apps.path.dirname(import.meta.filename), import.meta.filename]) : typeof import.meta.dirname;").unwrap();
    for production in [false, true] {
        for (realm, role) in [
            (Realm::Main, apps_runtime::jsc::Role::Main),
            (Realm::Preload, apps_runtime::jsc::Role::Preload(1)),
            (Realm::Renderer, apps_runtime::jsc::Role::Renderer(1)),
        ] {
            let files = compile(&entry, realm, None, Rc::default(), production, None).unwrap();
            let host = Rc::new(RefCell::new(apps_runtime::host::Host::default()));
            host.borrow().store.borrow_mut().pages.insert(
                1,
                apps_runtime::render::Page {
                    doc: Document::parse("<body></body>"),
                    dirty: false,
                    ..Default::default()
                },
            );
            let engine = apps_runtime::host::context::JsContext::new(
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
                _ => json!([
                    true,
                    true,
                    entry.to_str().unwrap().trim_start_matches(r"\\?\")
                ])
                .to_string(),
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
fn worker_query_imports_bundle_the_worker() {
    for production in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("app.ts");
        fs::write(&entry, "import W from './w.ts?worker'; globalThis.w = W;").unwrap();
        fs::write(dir.path().join("w.ts"), "globalThis.WORKER_MARKER = 1;").unwrap();
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        assert!(
            files
                .iter()
                .any(|(name, source)| name.starts_with("script-")
                    && source.contains("WORKER_MARKER")),
            "production={production}"
        );
    }
}
