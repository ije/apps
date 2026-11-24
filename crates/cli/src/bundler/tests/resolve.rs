use super::*;

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
                apps_runtime::bundle::write(&output.path().join("app.bundle"), &files, &[])
                    .unwrap();
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
            crate::build_target::BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH)
                .unwrap(),
            None,
            false,
            None,
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
            assert!(!path.exists(), "Copied packed addon: {name}");
            assert!(bundle.scripts.contains_key(&format!("node_modules/{name}")));
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
fn tsconfig_paths_and_css_package_exports() {
    let dir = tempfile::tempdir().unwrap();
    for (path, source) in [
        (
            "tsconfig.base.json",
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["./missing/*","./src/*"],"exact":["./src/exact.ts"]}}}"#,
        ),
        (
            "tsconfig.json",
            "{\n  // \"extends\": \"./other\",\n  \"extends\": \"./tsconfig.base\",\n  /* , */ \"compilerOptions\": { \"strict\": true, \"jsx\": \"a//b\", },\n}",
        ),
        (
            "app.ts",
            "import { a } from '@/lib/a'; import { exact } from 'exact'; import { dep } from 'dep'; globalThis.result = [a, exact, dep].join(':'); console.log(import.meta.resolve('./index.html'));",
        ),
        ("src/lib/a.ts", "export const a = 'a';"),
        ("src/exact.ts", "export const exact = 'exact';"),
        (
            "node_modules/dep/package.json",
            r#"{"type":"module","main":"index.js"}"#,
        ),
        ("node_modules/dep/index.js", "export const dep = 'dep';"),
        (
            "node_modules/kit/package.json",
            r#"{"exports":{".":{"types":"./index.d.ts","default":"./index.js"},"./theme.css":"./dist/theme.css"}}"#,
        ),
        ("node_modules/kit/dist/theme.css", ".kit{color:red}"),
        ("style.css", "@import 'kit/theme.css';"),
        ("index.html", "<link rel='stylesheet' href='style.css'>"),
    ] {
        fs::create_dir_all(dir.path().join(path).parent().unwrap()).unwrap();
        fs::write(dir.path().join(path), source).unwrap();
    }
    let entry = dir.path().join("app.ts");
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
    assert!(files["index.html"].contains(".kit{color:red}"));
    let engine = apps_runtime::jsc::JsContext::new(
        apps_runtime::jsc::JavaScriptCore::load().unwrap(),
        Rc::default(),
        apps_runtime::host::Role::Main,
    )
    .unwrap();
    engine.load_bundle(&files, "app.js").unwrap();
    assert_eq!(engine.eval("result", "test").unwrap(), "a:exact:dep");
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
    assert!(files["index.html"].contains(".spin{color:red}"));
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
            let css = linked_css(&files["index.html"]);
            assert!(
                css.contains(".b{color:red}") && css.contains(&format!("\"{origin}/img/a.png\"")),
                "{css}"
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
fn require_of_a_folder_uses_main() {
    let dir = tempfile::tempdir().unwrap();
    let package = dir.path().join("node_modules/pkg");
    fs::create_dir_all(package.join("lib")).unwrap();
    fs::write(package.join("package.json"), r#"{"main":"./index.js"}"#).unwrap();
    fs::write(
        package.join("index.js"),
        "module.exports = require('./lib');",
    )
    .unwrap();
    fs::write(
        package.join("lib/package.json"),
        r#"{"main":"./cjs.js","module":"./esm.js"}"#,
    )
    .unwrap();
    fs::write(package.join("lib/cjs.js"), "module.exports = 'CJS_MARKER';").unwrap();
    fs::write(package.join("lib/esm.js"), "export default 'ESM_MARKER';").unwrap();
    let entry = dir.path().join("app.ts");
    fs::write(&entry, "import value from 'pkg'; globalThis.value = value;").unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
    assert!(files["app.js"].contains("CJS_MARKER"));
    assert!(!files["app.js"].contains("ESM_MARKER"));
}
