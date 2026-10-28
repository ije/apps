#[cfg(unix)]
#[test]
fn native_calls_callbacks_memory_and_lifecycle() {
    use apps_runtime::{
        host::{Host, Role},
        jsc::{Api, Engine},
    };
    use std::{cell::RefCell, process::Command, rc::Rc};

    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join(if cfg!(target_os = "macos") {
        "fixture.dylib"
    } else {
        "fixture.so"
    });
    let output = Command::new("cc")
        .arg(if cfg!(target_os = "macos") {
            "-dynamiclib"
        } else {
            "-shared"
        })
        .args(["-fPIC", "-pthread", "-Wall", "-Wextra", "-Werror", "-o"])
        .arg(&library)
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/ffi.c"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for role in [Role::Main, Role::Preload(1)] {
        let engine = Engine::new(
            Api::load().unwrap(),
            Rc::new(RefCell::new(Host::default())),
            role,
        )
        .unwrap();
        engine
            .eval(
                &format!(
                    "globalThis.libraryPath = {};",
                    serde_json::to_string(&library).unwrap()
                ),
                "ffi-path",
            )
            .unwrap();
        engine.eval(include_str!("ffi.js"), "ffi-test").unwrap();
    }
}
