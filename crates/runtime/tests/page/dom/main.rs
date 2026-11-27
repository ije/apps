use apps_runtime::{
    dom::Document,
    host::Host,
    host::context::JsContext,
    jsc::{JavaScriptCore, Role},
    render::Page,
};
use std::{cell::RefCell, rc::Rc};

fn offscreen_page(source: &str) -> (Rc<RefCell<Host>>, JsContext, String) {
    let (wake, woken) = std::sync::mpsc::channel();
    apps_runtime::jsc::Waker::install(move || {
        let _ = wake.send(());
    });
    let (host, engine) = css_page("<body><canvas id=shown width=8 height=8></canvas></body>");
    engine
        .eval(
            &format!(
                r#"globalThis.testResult = 'pending';
                const worker = code => new Worker('data:text/javascript,' + encodeURIComponent(code));
                const reply = target => new Promise((resolve, reject) => {{ target.onmessage = event => resolve(event.data); target.onerror = event => reject(event.error ?? Error(event.message)); target.addEventListener('error', event => reject(event.error ?? Error('worker failed'))); }});
                const rgba = (bitmap, x = 0, y = 0) => {{ const canvas = new OffscreenCanvas(bitmap.width, bitmap.height), context = canvas.getContext('2d'); context.drawImage(bitmap, 0, 0); return [...context.getImageData(x, y, 1, 1).data].join(); }};
                const same = (actual, expected, name) => {{ actual = JSON.stringify(actual); expected = JSON.stringify(expected); if (actual !== expected) throw Error(name + ': ' + actual + ' is not ' + expected); }};
                const rejected = async (run, name) => {{ try {{ await run(); }} catch (error) {{ if (error.name !== name) throw Error('Expected ' + name + ', got ' + error); return; }} throw Error('Expected ' + name); }};
                (async () => {{ {source} }})().then(() => {{ testResult = 'ok'; }}, error => {{ testResult = error + '\n' + error?.stack; }});"#
            ),
            "test",
        )
        .unwrap();
    let started = std::time::Instant::now();
    let result = loop {
        let result = engine.eval("testResult", "test").unwrap();
        if result != "pending" {
            break result;
        }
        let _ = woken.recv_timeout(std::time::Duration::from_millis(2));
        engine.tick().unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "Test timed out"
        );
    };
    (host, engine, result)
}

fn checked_page(html: &str, script: &str) -> (Rc<RefCell<Host>>, JsContext) {
    let (host, engine) = css_page(html);
    let result = engine
        .eval(
            &format!(
                "(() => {{ const fails = []; const check = (name, fn) => {{ try {{ if (!fn()) fails.push(name); }} catch (e) {{ fails.push(name + ': ' + e.message); }} }}; {script}; return fails.join('\\n'); }})()"
            ),
            "test",
        )
        .unwrap();
    assert_eq!(result, "");
    (host, engine)
}

fn css_page(html: &str) -> (Rc<RefCell<Host>>, JsContext) {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow().store.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse(html),
            ..Default::default()
        },
    );
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    (host, engine)
}

mod canvas;
mod css;
mod events;
mod layout;
mod top_layer;
mod tree;
