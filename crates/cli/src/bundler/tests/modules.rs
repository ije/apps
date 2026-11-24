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
        let engine = apps_runtime::host::context::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::jsc::Role::Main,
        )
        .unwrap();
        engine.load_bundle(&files, "app.js").unwrap();
    }
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
        let engine = apps_runtime::host::context::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::jsc::Role::Main,
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
        let engine = apps_runtime::host::context::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::jsc::Role::Main,
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
        let engine = apps_runtime::host::context::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::jsc::Role::Main,
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
    let engine = apps_runtime::host::context::JsContext::new(
        apps_runtime::jsc::JavaScriptCore::load().unwrap(),
        Rc::default(),
        apps_runtime::jsc::Role::Main,
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
        let engine = apps_runtime::host::context::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::jsc::Role::Main,
        )
        .unwrap();
        engine.load_bundle(&files, "test").unwrap();
        assert_eq!(
            engine.eval("result", "test").unwrap(),
            "2:1:1:first,second:2"
        );
    }
    fs::write(&entry, "import {missing} from './counter';").unwrap();
    let error = compile(&entry, Realm::Main, None, Rc::default(), true, None).unwrap_err();
    assert!(
        error.chain().any(|error| {
            let text = error.to_string();
            text.contains("Missing export missing from") && text.contains("counter")
        }),
        "{error:#}"
    );
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
        let engine = apps_runtime::host::context::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::jsc::Role::Main,
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
            let engine = apps_runtime::host::context::JsContext::new(
                apps_runtime::jsc::JavaScriptCore::load().unwrap(),
                Rc::default(),
                apps_runtime::jsc::Role::Main,
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
    let engine = apps_runtime::host::context::JsContext::new(
        apps_runtime::jsc::JavaScriptCore::load().unwrap(),
        Rc::default(),
        apps_runtime::jsc::Role::Main,
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
        let engine = apps_runtime::host::context::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::jsc::Role::Main,
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
        let engine = apps_runtime::host::context::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::jsc::Role::Main,
        )
        .unwrap();
        engine.load_bundle(&files, "test").unwrap();
        assert_eq!(engine.eval("result", "test").unwrap(), "2:1:2:1");
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
    let engine = apps_runtime::host::context::JsContext::new(
        apps_runtime::jsc::JavaScriptCore::load().unwrap(),
        std::rc::Rc::new(std::cell::RefCell::new(apps_runtime::host::Host::default())),
        apps_runtime::jsc::Role::Main,
    )
    .unwrap();
    engine.load_bundle(&files, "app.js").unwrap();
    assert_eq!(engine.eval("result", "test").unwrap(), "2");
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
    let engine = apps_runtime::host::context::JsContext::new(
        apps_runtime::jsc::JavaScriptCore::load().unwrap(),
        std::rc::Rc::new(std::cell::RefCell::new(apps_runtime::host::Host::default())),
        apps_runtime::jsc::Role::Main,
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

#[cfg(unix)]
#[test]
fn child_process_with_top_level_await() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("app.js");
    fs::write(
        &entry,
        format!(
            "{}\nif (code !== 0) throw Error('child process failed');",
            include_str!("../../../tests/fixtures/child_process.js")
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
            include_str!("../../../tests/fixtures/wasm.js")
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

#[test]
fn text_imports_deferred_modules_and_top_level_await_using() {
    for production in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("note.txt"), "plain `text`\n").unwrap();
        fs::write(
            dir.path().join("slow.js"),
            "await Promise.resolve(); (globalThis.order ??= []).push('slow');",
        )
        .unwrap();
        fs::write(
            dir.path().join("lazy.js"),
            "import './slow.js'; globalThis.order.push('lazy'); export const value = 42;",
        )
        .unwrap();
        fs::write(
            dir.path().join("later.js"),
            "globalThis.order.push('later'); export const value = 7;",
        )
        .unwrap();
        let entry = dir.path().join("app.ts");
        fs::write(&entry, r#"import note from './note.txt' with { type: 'text' };
import defer * as lazy from './lazy.js';
const order: string[] = globalThis.order;
{ await using resource = { async [Symbol.asyncDispose]() { order.push('disposed'); } }; }
const dynamic = await import('./note.txt', { with: { type: 'text' } });
const later = await import.defer('./later.js');
const before = order.join();
const result = [note === 'plain `text`\n', dynamic.default === note, before, lazy.value, later.value, order.join(), Object.keys(lazy).join()].join(':');
if (result !== 'true:true:slow,disposed:42:7:slow,disposed,lazy,later:value') throw Error(result);"#).unwrap();
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
fn production_keeps_imports_that_shadow_globals() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("shadow.js"), "export const String = v => 'S' + v, Boolean = v => 'B' + v, Array = v => 'A' + v, Math = {PI: 3}, parseInt = v => 'P' + v;").unwrap();
    let entry = dir.path().join("app.js");
    fs::write(&entry, "import {String, Boolean, Array, Math, parseInt} from './shadow.js'; export {String}; globalThis.result = [String(1), Boolean(0), Array(2), Math.PI, parseInt('7'), ({String}).String === String].join(':');").unwrap();
    for production in [false, true] {
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        let engine = apps_runtime::host::context::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::jsc::Role::Main,
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
        let engine = apps_runtime::host::context::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::jsc::Role::Main,
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
        let engine = apps_runtime::host::context::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::jsc::Role::Main,
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
fn production_slots_keep_module_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, source: &str| fs::write(dir.path().join(name), source).unwrap();
    write(
        "live.js",
        "export let count = 0; export function bump() { count++; } export let set = 1; set = 2; export var state; (function (s) { state = s; })('ready');",
    );
    write(
        "origin.js",
        "export const value = 'VALUE'; export default 'DEFAULT'; export function self() { return this === undefined; }",
    );
    write(
        "barrel.js",
        "globalThis.order.push('barrel'); export { value, default as renamed } from './origin.js'; export * from './stars.js';",
    );
    write(
        "stars.js",
        "globalThis.order.push('stars'); export const starred = 'STARRED';",
    );
    write(
        "spread.js",
        "export const a = 1; export let b = 2; export function setB() { b = 3; }",
    );
    write(
        "cycle-a.js",
        "import { fromB } from './cycle-b.js'; export function fromA() { return 'A'; } export const seen = fromB();",
    );
    write(
        "cycle-b.js",
        "import { fromA } from './cycle-a.js'; export function fromB() { return fromA() + 'B'; }",
    );
    fs::create_dir_all(dir.path().join("node_modules/dep")).unwrap();
    write(
        "node_modules/dep/package.json",
        r#"{"name":"dep","main":"index.js"}"#,
    );
    write(
        "node_modules/dep/index.js",
        "exports.named = 'NAMED'; exports.viaRequire = require('./required.mjs').viaEsm;",
    );
    write(
        "node_modules/dep/required.mjs",
        "export const viaEsm = 'VIA_ESM';",
    );
    write(
        "later.js",
        "export const later = 'LATER'; export default 'LATER_DEFAULT';",
    );
    write(
        "app.js",
        r#"import './setup.js';
import { count, bump, set, state } from './live.js';
import { value, renamed, starred } from './barrel.js';
import * as origin from './origin.js';
import * as spread from './spread.js';
import { seen } from './cycle-a.js';
import { named, viaRequire } from 'dep';
const before = count; bump(); bump();
const keys = Object.keys(spread).sort().join();
const beforeB = spread.b; spread.setB();
globalThis.result = [before, count, set, state, value, renamed, starred, origin.self(), keys, beforeB, spread.b, spread[Symbol.toStringTag], seen, named, viaRequire, globalThis.order.join('>')].join('|');
globalThis.later = import('./later.js').then(ns => { globalThis.result += '|' + ns.later + '|' + ns.default + '|' + Object.isExtensible(ns); });
"#,
    );
    write("setup.js", "globalThis.order = [];");
    let entry = dir.path().join("app.js");
    for production in [false, true] {
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        if production {
            assert!(files["app.js"].contains(",flat:1"));
            assert!(!files["app.js"].contains("get:()=>\"VALUE\""));
        }
        let engine = apps_runtime::host::context::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::jsc::Role::Main,
        )
        .unwrap();
        engine.load_bundle(&files, "app.js").unwrap();
        for _ in 0..4 {
            engine.tick().unwrap();
        }
        assert_eq!(
            engine.eval("result", "test").unwrap(),
            format!(
                "0|2|2|ready|VALUE|DEFAULT|STARRED|{production}|a,b,setB|2|3|Module|AB|NAMED|VIA_ESM|stars>barrel|LATER|LATER_DEFAULT|false"
            ),
            "production={production}"
        );
    }
}

#[test]
fn production_names_of_nested_packages_stay_unique() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let write = |name: &str, source: &[u8]| {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, source).unwrap();
    };
    for (folder, label) in [
        ("node_modules/x", "outer"),
        ("node_modules/a/node_modules/x", "inner"),
        ("node_modules/.pnpm/x@2/node_modules/x", "pnpm"),
    ] {
        write(&format!("{folder}/package.json"), br#"{"name":"x"}"#);
        write(
            &format!("{folder}/index.js"),
            format!("globalThis.runs.push('{label}');exports.file=__filename;").as_bytes(),
        );
        write(
            &format!("{folder}/url.mjs"),
            b"export default import.meta.url;",
        );
        write(&format!("{folder}/logo.png"), label.as_bytes());
    }
    for name in ["a", "b", "c"] {
        write(
            &format!("node_modules/{name}/package.json"),
            format!(r#"{{"name":"{name}"}}"#).as_bytes(),
        );
        write(
            &format!("node_modules/{name}/index.js"),
            b"exports.get=()=>require('x');exports.url=require('x/url.mjs');exports.logo=require('x/logo.png');",
        );
    }
    write(
        "node_modules/c/index.js",
        b"exports.get=()=>require('../.pnpm/x@2/node_modules/x/index.js');",
    );
    write(
        "src/app.js",
        b"globalThis.runs = []; import a from 'a'; import b from 'b'; import c from 'c';
const files = [a.get(), b.get(), c.get(), a.get(), b.get(), c.get()].map(x => x.file);
globalThis.result = [globalThis.runs.join(), new Set(files).size, a.url !== b.url, a.logo !== b.logo].join('|');",
    );
    let entry = root.join("src/app.js");
    let files = compile(&entry, Realm::Main, None, Rc::default(), true, None).unwrap();
    assert!(files["app.js"].contains("../node_modules/a/node_modules/x/index.js"));
    let engine = apps_runtime::host::context::JsContext::new(
        apps_runtime::jsc::JavaScriptCore::load().unwrap(),
        Rc::default(),
        apps_runtime::jsc::Role::Main,
    )
    .unwrap();
    engine.load_bundle(&files, "app.js").unwrap();
    assert_eq!(
        engine.eval("result", "test").unwrap(),
        "inner,outer,pnpm|3|true|true"
    );
    assert_eq!(
        app_relative(
            Path::new("/repo/app/src"),
            Path::new("/repo/app/node_modules/.pnpm/x@1/node_modules/x/index.js")
        ),
        Path::new("../node_modules/.pnpm/x@1/node_modules/x/index.js")
    );
}

#[test]
fn production_slots_stay_live_for_writes_after_the_body() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("ids.js"),
        "export let nextId = 0; export class Node { id = nextId++; }
export let early = 0; class Setup { static { early++; } static field = early++; [early++]() {} }
for (let i = 0; i < 3; i++) early++;
export let viaGetter = 0; export const object = { get x() { return viaGetter++; } };
export let viaMethod = 0; export class Counter { bump() { viaMethod++; } }",
    )
    .unwrap();
    let entry = dir.path().join("app.js");
    fs::write(
        &entry,
        "import { nextId, Node, early, viaGetter, object, viaMethod, Counter } from './ids.js';
new Node(); new Node(); object.x; new Counter().bump();
globalThis.result = [nextId, early, viaGetter, viaMethod].join();",
    )
    .unwrap();
    for production in [false, true] {
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        let engine = apps_runtime::host::context::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::jsc::Role::Main,
        )
        .unwrap();
        engine.load_bundle(&files, "app.js").unwrap();
        assert_eq!(
            engine.eval("result", "test").unwrap(),
            "2,6,1,1",
            "production={production}"
        );
    }
}

#[test]
fn production_reads_only_the_builtin_exports_that_imports_use() {
    let dir = tempfile::tempdir().unwrap();
    let named = dir.path().join("named.js");
    fs::write(&named, "import { existsSync } from 'node:fs'; import { join } from 'node:path'; globalThis.result = [existsSync(join('/', 'missing-file')), typeof join].join(':');").unwrap();
    let whole = dir.path().join("whole.js");
    fs::write(&whole, "import * as all from 'node:fs'; globalThis.result = [typeof all.ReadStream, Object.keys(all).includes('createReadStream')].join(':');").unwrap();
    for production in [false, true] {
        for (entry, reads, expected) in [
            (&named, !production, "false:function"),
            (&whole, true, "function:true"),
        ] {
            let files = compile(entry, Realm::Main, None, Rc::default(), production, None).unwrap();
            assert_eq!(
                files["app.js"].contains("ReadStream"),
                reads,
                "{entry:?} production={production}"
            );
            let engine = apps_runtime::host::context::JsContext::new(
                apps_runtime::jsc::JavaScriptCore::load().unwrap(),
                Rc::default(),
                apps_runtime::jsc::Role::Main,
            )
            .unwrap();
            engine.load_bundle(&files, "app.js").unwrap();
            assert_eq!(engine.eval("result", "test").unwrap(), expected);
        }
    }
}

#[test]
fn production_drops_unused_modules_of_side_effect_free_packages() {
    let dir = tempfile::tempdir().unwrap();
    let icons = dir.path().join("node_modules/icons");
    fs::create_dir_all(icons.join("dist/icons")).unwrap();
    fs::write(icons.join("package.json"), r#"{"name":"icons","type":"module","main":"dist/index.js","sideEffects":["./dist/register.js","*.{fx,effect}.js"]}"#).unwrap();
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
        let engine = apps_runtime::host::context::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::jsc::Role::Main,
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
            && glob(b"[a-c]?.js", b"b1.js")
            && !glob(b"[!a-c].js", b"b.js")
            && glob(b"[^a-c].js", b"d.js")
            && glob(b"[]x].js", b"].js")
            && glob(b"a[.js", b"a[.js")
            && !glob(b"a[/]b", b"a/b")
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
        let engine = apps_runtime::host::context::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::jsc::Role::Main,
        )
        .unwrap();
        engine.load_bundle(&files, "app.js").unwrap();
        assert_eq!(engine.eval("result", "test").unwrap(), "a:b:2");
    }
}
