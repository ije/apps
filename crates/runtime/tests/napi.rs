#[cfg(unix)]
#[test]
fn native_addons_values_callbacks_and_contexts() {
    use apps_runtime::{
        host::{Host, Role},
        jsc::{JavaScriptCore, JsContext},
    };
    use std::{cell::RefCell, process::Command, rc::Rc};
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join("addon.node");
    let mut command = Command::new("cc");
    if cfg!(target_os = "macos") {
        command.args(["-dynamiclib", "-undefined", "dynamic_lookup"]);
    } else {
        command.args(["-shared", "-fPIC"]);
    }
    let output = command
        .args(["-Wall", "-Wextra", "-Werror", "-o"])
        .arg(&library)
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/napi.c"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let second = dir.path().join("second.node");
    std::fs::copy(&library, &second).unwrap();
    for role in [Role::Main, Role::Preload(1)] {
        let mut host = Host::default();
        host.native_modules
            .insert("fixture/addon.node".into(), library.clone());
        host.native_modules
            .insert("fixture/second.node".into(), second.clone());
        let engine = JsContext::new(
            JavaScriptCore::load().unwrap(),
            Rc::new(RefCell::new(host)),
            role,
        )
        .unwrap();
        engine.eval(include_str!("napi.js"), "napi-test").unwrap();
        assert_eq!(engine.eval("napiPromise", "test").unwrap(), "true");
        assert_eq!(
            engine
                .eval(
                    "addon.dateStatus({})+':'+addon.dateStatus(new Date())",
                    "test"
                )
                .unwrap(),
            "18:0"
        );
        engine.eval("globalThis.initialFinalizers=addon.finalizerCount();for(let i=0;i<100000;i++)addon.garbage()", "test").unwrap();
        let collect = |done: &str| {
            for _ in 0..200 {
                engine.tick().unwrap();
                if engine.eval(done, "test").unwrap() == "true" {
                    return true;
                }
                engine
                    .eval(
                        "(()=>{const values=Array.from({length:10000},()=>({}))})()",
                        "test",
                    )
                    .unwrap();
            }
            false
        };
        assert!(
            collect("addon.finalizerCount()-initialFinalizers>=299990"),
            "Finalizers waited for a lazy sweep"
        );
        engine.eval("globalThis.externals=addon.externalCount();globalThis.moved=addon.externalBuffer().buffer.transfer();addon.setLoader(()=>{try{globalThis.loaded=__appsLoadAddon('fixture/second.node').add(1,1)}catch(error){globalThis.loaded=String(error)}});addon.loading();0", "test").unwrap();
        assert!(collect("globalThis.loaded!==undefined"));
        assert_eq!(engine.eval("loaded", "test").unwrap(), "3");
        assert_eq!(
            engine
                .eval(
                    "addon.externalCount()-externals+':'+new TextDecoder().decode(moved)",
                    "test"
                )
                .unwrap(),
            "0:napi",
            "A transferred external buffer was freed"
        );
        engine.eval("moved=null", "test").unwrap();
        assert!(collect("addon.externalCount()-externals===1"));
        let wait = |done: &str| {
            (0..1000).any(|_| {
                engine.tick().unwrap();
                std::thread::sleep(std::time::Duration::from_millis(5));
                engine.eval(done, "test").unwrap() == "true"
            })
        };
        assert!(
            wait("addon.threadsafeCount()-threadsafeStart===2&&addon.workState().completed===8"),
            "Thread-safe functions or async work did not finish"
        );
        assert_eq!(
            engine
                .eval("threadsafeValues.filter(value=>value>=0).join()", "test")
                .unwrap(),
            std::iter::once(0)
                .chain(0..=50)
                .map(|value| value.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );
        assert_eq!(
            engine
                .eval(
                    "(({cancelled,most})=>cancelled+':'+(most>=2&&most<=4))(addon.workState())",
                    "test"
                )
                .unwrap(),
            "1:true",
            "Async work must run on at most 4 threads"
        );
        engine.eval("globalThis.process?.on('uncaughtException',error=>globalThis.finalizerError=error.message);for(let i=0;i<1000;i++){addon.throwing();addon.garbage()}", "test").unwrap();
        engine.collect_garbage();
        assert_eq!(
            engine.eval("addon.stable()", "test").unwrap(),
            "true",
            "A Node-API call ran finalizers"
        );
        assert!(collect(
            "typeof process==='undefined'||globalThis.finalizerError==='finalizer failed'"
        ));
        assert_eq!(
            engine.eval("addon.add(1,1)", "test").unwrap(),
            "3",
            "A finalizer exception leaked into a later call"
        );
        assert!(
            engine
                .eval("__appsLoadAddon('missing.node')", "test")
                .unwrap_err()
                .to_string()
                .contains("not in the bundle")
        );
    }
    // An environment that ends must not free addon state under work that still runs on the pool. Work that
    // has not started must never start.
    let load = || {
        let mut host = Host::default();
        host.native_modules
            .insert("fixture/addon.node".into(), library.clone());
        JsContext::new(
            JavaScriptCore::load().unwrap(),
            Rc::new(RefCell::new(host)),
            Role::Main,
        )
        .unwrap()
    };
    let engine = load();
    engine
        .eval(
            "globalThis.kept=__appsLoadAddon('fixture/addon.node').slow()",
            "test",
        )
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    drop(engine);
    std::thread::sleep(std::time::Duration::from_millis(900));
    assert_eq!(
        load()
            .eval(
                "JSON.stringify(__appsLoadAddon('fixture/addon.node').slowState())",
                "test"
            )
            .unwrap(),
        r#"{"ran":4,"late":0}"#
    );
    let mut host = Host::default();
    host.pages.insert(
        1,
        apps_runtime::host::Page {
            doc: apps_runtime::dom::Document::parse("<body></body>"),
            dirty: false,
        },
    );
    let renderer = JsContext::new(
        JavaScriptCore::load().unwrap(),
        Rc::new(RefCell::new(host)),
        Role::Renderer(1),
    )
    .unwrap();
    assert_eq!(
        renderer.eval("typeof __appsLoadAddon", "test").unwrap(),
        "undefined"
    );
}
