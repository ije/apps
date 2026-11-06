use crate::{
    bundle::Bundle,
    host::{Host, Role},
    jsc::{JavaScriptCore, JsContext},
};
use anyhow::{Context, Result};
use std::{cell::RefCell, rc::Rc, sync::mpsc, time::Duration};

/// Runs a script chunk as a plain process, without windows, for `ELECTRON_RUN_AS_NODE`.
/// `argv` is `[execPath, script, ...arguments]`.
pub fn run(mut bundle: Bundle, argv: Vec<String>) -> Result<i32> {
    let script = argv
        .get(1)
        .context("Usage: ELECTRON_RUN_AS_NODE=1 <app> <script> [arguments]")?;
    let name = script.strip_prefix("apps:///").unwrap_or(script);
    let source = match bundle.scripts.remove(name) {
        Some(source) => source,
        None => bundle
            .files
            .remove(name)
            .with_context(|| format!("Cannot find the script {script}"))?
            .into(),
    };
    let (wake, wakes) = mpsc::channel();
    crate::host::Waker::install(move || {
        let _ = wake.send(());
    });
    let package = bundle.files.get("package.json").map(String::as_str);
    let (_, name, version, _) = crate::bundle::package_metadata(package)?;
    let assets = bundle
        .files
        .get("assets.json")
        .map(|source| serde_json::from_str(source))
        .transpose()
        .context("Invalid assets.json")?
        .unwrap_or_default();
    let assets: std::collections::HashMap<String, std::path::PathBuf> = assets;
    let _ = crate::web::ASSETS.set(assets.clone());
    let host = Rc::new(RefCell::new(Host {
        app_version: Some(version),
        disabled: crate::bundle::disabled_features(package)?,
        app_name: Some(name),
        app_path: if bundle.root.as_os_str().is_empty() {
            std::env::current_dir().unwrap_or_default()
        } else {
            bundle.root
        },
        packaged: bundle.packaged,
        scripts: bundle.scripts.into(),
        assets,
        argv: Some(argv),
        ..Host::default()
    }));
    host.borrow_mut().install(&bundle.files)?;
    let api = JavaScriptCore::load()?;
    let engine = JsContext::new(api.clone(), host, Role::Main)?;
    engine.eval(
        "globalThis.require = globalThis.__apps.module.createRequire();",
        "apps:node",
    )?;
    engine.eval_page(std::str::from_utf8(&source)?, "apps:node")?;
    while engine.eval_page("this.__appsNext?.()", "apps:node")? == "true" {}
    let code = loop {
        let delay = engine.tick()?;
        // WARNING: like in Node, the process ends when only unreferenced handles are left.
        if delay != 0 && !engine.addons_pending() && engine.call("__appsAlive", &[])? != "true" {
            let code = engine.call("__appsFinish", &[])?;
            if code != "-1" {
                break code.parse().unwrap_or(0);
            }
            continue;
        }
        crate::host::idle(true);
        // WARNING: JSC's GC timers never fire on the generic event loop, so the VM is collected when it has been quiet for a second.
        let wait = if api.shrink_pending() && !(0..1000).contains(&delay) {
            1000
        } else {
            delay
        };
        let _ = if wait < 0 {
            wakes
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        } else {
            wakes.recv_timeout(Duration::from_millis(wait as u64))
        };
        if api.shrink_due() {
            api.shrink();
            #[cfg(target_os = "macos")]
            crate::malloc::flush();
        }
        while wakes.try_recv().is_ok() {}
        crate::host::idle(false);
    };
    std::mem::forget(engine);
    Ok(code)
}
