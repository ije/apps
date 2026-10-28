use apps_runtime::{
    host::{Host, Page, Role},
    html::Document,
    jsc::{Api, Engine},
};
use std::{cell::RefCell, rc::Rc};

#[test]
fn canvas_identity_and_webgl_errors() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<html><body><canvas id='parsed'></canvas></body></html>"),
            dirty: false,
        },
    );
    let engine = Engine::new(Api::load().unwrap(), host, Role::Renderer(1)).unwrap();
    engine.eval(r#"
        const parsed = document.getElementById('parsed');
        const canvas = document.createElementNS('http://www.w3.org/1999/xhtml', 'canvas');
        document.body.appendChild(canvas);
        if (!(parsed instanceof HTMLCanvasElement) || !(canvas instanceof HTMLCanvasElement)) throw Error('Canvas wrapper missing');
        if (canvas.width !== 300 || canvas.height !== 150) throw Error('Invalid canvas defaults');
        canvas.width = 64; canvas.height = 32; canvas.style.width = '128px';
        if (canvas.width !== 64 || canvas.height !== 32 || canvas.clientWidth !== 128) throw Error('Canvas sizing failed');
        if (canvas.getContext('webgpu') !== canvas.getContext('webgpu')) throw Error('Context identity changed');
        for (const type of ['webgl', 'webgl2', 'experimental-webgl', 'experimental-webgl2']) {
            let rejected = false;
            try { canvas.getContext(type); } catch (e) { rejected = e.message.includes('WebGL is not supported'); }
            if (!rejected) throw Error(type + ' was accepted');
        }
        let unconfigured = false;
        try { canvas.getContext('webgpu').getCurrentTexture(); } catch { unconfigured = true; }
        if (!unconfigured || canvas.getContext('unknown') !== null) throw Error('Invalid context handling');
        if (typeof __appsGPU !== 'undefined' || typeof __appsNative !== 'undefined' || typeof process !== 'undefined') throw Error('Host bridge leaked');
        let delta = 0, released = false;
        canvas.addEventListener('pointerdown', e => canvas.setPointerCapture(e.pointerId));
        canvas.addEventListener('pointermove', e => { if (e.target !== canvas) throw Error('Capture target changed'); delta = e.clientX; });
        canvas.addEventListener('lostpointercapture', () => { released = true; });
        __appsDispatch('pointerdown', [canvas._id], { pointerId: 1, clientX: 10, buttons: 1 });
        __appsDispatch('pointermove', [], { pointerId: 1, clientX: 200, buttons: 1 });
        if (delta !== 200 || !canvas.hasPointerCapture(1)) throw Error('Pointer capture failed');
        __appsDispatch('pointerup', [], { pointerId: 1, buttons: 0 });
        if (!released || canvas.hasPointerCapture(1)) throw Error('Pointer capture was not released');
        if (__appsDispatch('pointermove', [], { pointerId: 1, clientX: 300, buttons: 0 }) !== false) throw Error('Released pointer was still captured');
    "#, "canvas-test").unwrap();
}

#[test]
#[ignore = "Requires a native GPU adapter"]
fn renders_pixels_and_runs_compute() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse(
                "<html><body><canvas id='canvas' width='64' height='64'></canvas></body></html>",
            ),
            dirty: false,
        },
    );
    let engine = Engine::new(Api::load().unwrap(), host.clone(), Role::Renderer(1)).unwrap();
    engine.eval(include_str!("webgpu.js"), "gpu-test").unwrap();
    for _ in 0..1000 {
        if engine
            .eval("globalThis.gpuTestResult ?? 'pending'", "gpu-test")
            .unwrap()
            != "pending"
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(
        engine
            .eval("globalThis.gpuTestResult ?? 'pending'", "gpu-test")
            .unwrap(),
        "ok"
    );
    let broken: usize = engine
        .eval("brokenCanvas", "gpu-test")
        .unwrap()
        .parse()
        .unwrap();
    let mut state = host.borrow_mut();
    assert!(
        state
            .graphics
            .get_mut(&1)
            .unwrap()
            .image(broken)
            .unwrap()
            .is_none()
    );
    let node = state.pages[&1]
        .doc
        .nodes
        .iter()
        .position(|n| n.tag == "canvas")
        .unwrap();
    let image = state
        .graphics
        .get_mut(&1)
        .unwrap()
        .image(node)
        .unwrap()
        .unwrap();
    let pixels = image.as_bytes(0).unwrap();
    assert_eq!(
        &pixels[(32 * 64 + 32) * 4..(32 * 64 + 32) * 4 + 4],
        &[0, 0, 255, 255]
    );
    assert_eq!(&pixels[0..4], &[0, 0, 0, 255]);
    drop(state);
    engine
        .eval(
            "canvas.width = 16; canvas.height = 8; context.getCurrentTexture();",
            "resize",
        )
        .unwrap();
    let mut state = host.borrow_mut();
    let image = state
        .graphics
        .get_mut(&1)
        .unwrap()
        .image(node)
        .unwrap()
        .unwrap();
    assert_eq!(image.as_bytes(0).unwrap().len(), 16 * 8 * 4);
}
