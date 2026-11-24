use crate::{bundle::Bundle, host::Host, host::context::JsContext, jsc::JavaScriptCore, jsc::Role};
use anyhow::{Context, Result};
use std::{cell::RefCell, rc::Rc, sync::mpsc, time::Duration};

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
    drop(bundle.files.remove("app.js"));
    let (wake, wakes) = mpsc::channel();
    crate::jsc::Waker::install(move || {
        let _ = wake.send(());
    });
    let package =
        crate::bundle::Package::parse(bundle.files.get("package.json").map(String::as_str))?;
    let host = Rc::new(RefCell::new(Host {
        argv: Some(argv),
        ..Host::new(crate::bundle::AppInfo::new(&mut bundle, &package, false)?)
    }));
    let api = JavaScriptCore::load()?;
    let engine = JsContext::new(api.clone(), host, Role::Main)?;
    engine.eval(
        "globalThis.require = globalThis.__apps.module.createRequire();",
        "apps:node",
    )?;
    engine.eval_page(std::str::from_utf8(&source)?, "apps:node")?;
    while engine.eval_page(crate::host::context::NEXT, "apps:node")? == "true" {}
    let code = loop {
        let delay = engine.tick()?;
        if delay != 0 && !engine.addons_pending() && engine.call("__appsAlive", &[])? != "true" {
            let code = engine.call("__appsFinish", &[])?;
            if code != "-1" {
                break code.parse().unwrap_or(0);
            }
            continue;
        }
        crate::jsc::idle(true);
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
        crate::jsc::idle(false);
    };
    std::mem::forget(engine);
    Ok(code)
}
