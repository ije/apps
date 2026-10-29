#[cfg(unix)]
#[test]
fn native_addons_values_callbacks_and_contexts() {
    use apps_runtime::{
        host::{Host, Role},
        jsc::{Api, Engine},
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
    for role in [Role::Main, Role::Preload(1)] {
        let mut host = Host::default();
        host.native_modules
            .insert("fixture/addon.node".into(), library.clone());
        let engine = Engine::new(Api::load().unwrap(), Rc::new(RefCell::new(host)), role).unwrap();
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
        for _ in 0..20 {
            engine.collect_garbage();
            engine.tick().unwrap();
            if engine
                .eval("addon.finalizerCount()-initialFinalizers", "test")
                .unwrap()
                .parse::<usize>()
                .unwrap()
                >= 270000
            {
                break;
            }
            engine
                .eval(
                    "(()=>{const values=Array.from({length:10000},()=>({}))})()",
                    "test",
                )
                .unwrap();
        }
        assert!(
            engine
                .eval("addon.finalizerCount()-initialFinalizers", "test")
                .unwrap()
                .parse::<usize>()
                .unwrap()
                >= 270000
        );
        assert!(
            engine
                .eval("__appsLoadAddon('missing.node')", "test")
                .unwrap_err()
                .to_string()
                .contains("not in the bundle")
        );
    }
    let mut host = Host::default();
    host.pages.insert(
        1,
        apps_runtime::host::Page {
            doc: apps_runtime::dom::Document::parse("<body></body>"),
            dirty: false,
        },
    );
    let renderer = Engine::new(
        Api::load().unwrap(),
        Rc::new(RefCell::new(host)),
        Role::Renderer(1),
    )
    .unwrap();
    assert_eq!(
        renderer.eval("typeof __appsLoadAddon", "test").unwrap(),
        "undefined"
    );
}
