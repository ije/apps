use super::{
    Host, Page, WORKER_PAGE,
    context::{JsContext, NEXT},
};
use crate::{
    dom::Document,
    jsc::{JavaScriptCore, Role},
    web::worker::Job,
};
use anyhow::Result;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{atomic::Ordering, mpsc},
    time::Duration,
};

// Runs the script of a worker in a new JavaScript context, on the worker thread.
pub(crate) fn run(mut job: Job) -> Result<()> {
    let host = Rc::new(RefCell::new(Host {
        app: std::mem::take(&mut job.app),
        ..Host::default()
    }));
    let role = if job.native {
        Role::Main
    } else {
        host.borrow_mut().pages.insert(
            WORKER_PAGE,
            Page {
                doc: Document::parse("<html><body></body></html>"),
                dirty: false,
                ..Default::default()
            },
        );
        Role::Renderer(WORKER_PAGE)
    };
    let api = JavaScriptCore::load()?;
    let engine = JsContext::new_interruptible(api.clone(), host, role, Some(job.stop.clone()))?;
    // Declared after the engine, so it is cleared before the VM goes away.
    struct Clear(crate::web::worker::Running);
    impl Drop for Clear {
        fn drop(&mut self) {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
        }
    }
    *job.running
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = api.terminator();
    let _clear = Clear(job.running.clone());
    engine.eval("for (const name of ['document', 'window', 'screen', 'screenX', 'screenY', 'screenLeft', 'screenTop', 'outerWidth', 'outerHeight', 'localStorage', 'sessionStorage']) delete globalThis[name]; globalThis.self=globalThis;", "apps:worker-scope")?;
    if job.native {
        engine.eval("globalThis.require = globalThis.__apps.module.createRequire(); globalThis.onmessage = null;", "apps:worker-modules")?;
    }
    job.online();
    engine.eval_page(std::str::from_utf8(&job.source)?, "apps:worker")?;
    while engine.eval_page(NEXT, "apps:worker")? == "true" {}
    while !job.stop.load(Ordering::Acquire) {
        let delay = engine.tick()?;
        if job.ends && delay != 0 && engine.call("__appsAlive", &[])? != "true" {
            break;
        }
        crate::jsc::idle(true);
        let wait = if api.shrink_pending() && !(0..1000).contains(&delay) {
            1000
        } else {
            delay
        };
        let message = if wait < 0 {
            job.inbox
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        } else {
            job.inbox.recv_timeout(Duration::from_millis(wait as u64))
        };
        if api.shrink_due() {
            api.shrink();
            #[cfg(target_os = "macos")]
            crate::malloc::flush();
        }
        crate::jsc::idle(false);
        match message {
            Ok(message) => {
                for message in std::iter::once(message).chain(job.inbox.try_iter().take(63)) {
                    if let Some(text) = message.value[0].as_str() {
                        job.receive(message.buffers);
                        engine.call("__appsWorkerReceive", &[text])?;
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
    Ok(())
}
