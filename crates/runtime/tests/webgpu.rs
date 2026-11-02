use apps_runtime::{
    dom::Document,
    host::{Host, Page, Role},
    jsc::{JavaScriptCore, JsContext},
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

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
    let engine = JsContext::new(JavaScriptCore::load().unwrap(), host, Role::Renderer(1)).unwrap();
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

fn page(html: &str) -> (Rc<RefCell<Host>>, JsContext) {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse(html),
            dirty: false,
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

fn wait(engine: &JsContext, ready: &str) -> String {
    for _ in 0..5000 {
        let value = engine.eval(ready, "gpu-test").unwrap();
        if value != "pending" {
            return value;
        }
        engine.tick().unwrap();
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("GPU work did not finish: {ready}");
}

fn present(host: &RefCell<Host>, node: usize) -> Arc<renderer::RenderImage> {
    for _ in 0..5000 {
        if let Some(image) = host
            .borrow_mut()
            .graphics
            .get_mut(&1)
            .unwrap()
            .image(node)
            .unwrap()
        {
            return image;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("Canvas was not presented");
}

#[test]
#[ignore = "Requires a native GPU adapter"]
fn renders_pixels_and_runs_compute() {
    let (host, engine) =
        page("<html><body><canvas id='canvas' width='64' height='64'></canvas></body></html>");
    engine.eval(include_str!("webgpu.js"), "gpu-test").unwrap();
    assert_eq!(wait(&engine, "globalThis.gpuTestResult ?? 'pending'"), "ok");
    let broken: usize = engine
        .eval("brokenCanvas", "gpu-test")
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        host.borrow_mut()
            .graphics
            .get_mut(&1)
            .unwrap()
            .image(broken)
            .unwrap()
            .is_none()
    );
    let node = host.borrow().pages[&1]
        .doc
        .nodes
        .iter()
        .position(|n| n.tag == "canvas")
        .unwrap();
    let image = present(&host, node);
    let pixels = image.bytes();
    assert_eq!(
        &pixels[(32 * 64 + 32) * 4..(32 * 64 + 32) * 4 + 4],
        &[0, 0, 255, 255]
    );
    assert_eq!(&pixels[0..4], &[0, 0, 0, 255]);
    engine
        .eval(
            "canvas.width = 16; canvas.height = 8; context.getCurrentTexture();",
            "resize",
        )
        .unwrap();
    assert_eq!(present(&host, node).bytes().len(), 16 * 8 * 4);
}

#[test]
#[ignore = "Requires a native GPU adapter"]
fn map_async_and_work_done_resolve_without_blocking_the_tick() {
    let (_host, engine) = page("<body></body>");
    engine.eval(r#"(async () => {
        const device = await (await navigator.gpu.requestAdapter()).requestDevice();
        const output = device.createBuffer({ size: 4, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC });
        const readback = device.createBuffer({ size: 4, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ });
        const pipeline = device.createComputePipeline({ layout: 'auto', compute: { module: device.createShaderModule({ code: `
            @group(0) @binding(0) var<storage, read_write> out: array<u32>;
            @compute @workgroup_size(1) fn main() { var v = 1u; for (var i = 0u; i < 4000000u; i++) { v = v * 1664525u + 1013904223u; } out[0] = v; }
        ` }), entryPoint: 'main' } });
        const encoder = device.createCommandEncoder();
        const pass = encoder.beginComputePass();
        pass.setPipeline(pipeline);
        pass.setBindGroup(0, device.createBindGroup({ layout: pipeline.getBindGroupLayout(0), entries: [{ binding: 0, resource: { buffer: output } }] }));
        pass.dispatchWorkgroups(1); pass.end();
        encoder.copyBufferToBuffer(output, 0, readback, 0, 4);
        device.queue.submit([encoder.finish()]);
        const started = performance.now();
        const mapped = readback.mapAsync(GPUMapMode.READ).then(() => {
            globalThis.mapped = performance.now() - started;
            let v = 1; for (let i = 0; i < 4000000; i++) v = (Math.imul(v, 1664525) + 1013904223) >>> 0;
            if (new Uint32Array(readback.getMappedRange())[0] !== v) throw Error('Wrong compute result');
        });
        const done = device.queue.onSubmittedWorkDone().then(() => { globalThis.done = true; });
        globalThis.issued = performance.now() - started;
        globalThis.state = readback.mapState;
        await Promise.all([mapped, done]);
        const aborted = device.createBuffer({ size: 4, usage: GPUBufferUsage.MAP_READ });
        const abort = aborted.mapAsync(GPUMapMode.READ).then(() => 'mapped', error => error.name);
        aborted.unmap();
        if (await abort !== 'AbortError' || aborted.mapState !== 'unmapped') throw Error('Unmap did not abort the mapping');
        await aborted.mapAsync(GPUMapMode.READ);
        aborted.unmap();
        globalThis.gpuTestResult = 'ok';
    })().catch(error => { globalThis.gpuTestResult = String(error); });"#, "gpu-test").unwrap();
    assert_eq!(engine.eval("state", "gpu-test").unwrap(), "pending");
    let tick = Instant::now();
    engine.tick().unwrap();
    let tick = tick.elapsed().as_secs_f64() * 1000.;
    assert_eq!(wait(&engine, "globalThis.gpuTestResult ?? 'pending'"), "ok");
    let issued: f64 = engine.eval("issued", "gpu-test").unwrap().parse().unwrap();
    let mapped: f64 = engine.eval("mapped", "gpu-test").unwrap().parse().unwrap();
    assert_eq!(engine.eval("done", "gpu-test").unwrap(), "true");
    assert!(
        issued + tick < mapped / 2.,
        "mapAsync blocked: issued in {issued:.1} ms, tick {tick:.1} ms, mapped after {mapped:.1} ms"
    );
}

#[test]
#[ignore = "Requires a native GPU adapter"]
fn static_canvas_stops_presenting_when_idle() {
    let (host, engine) = page("<body><canvas id='canvas' width='32' height='32'></canvas></body>");
    engine.eval(r#"(async () => {
        const device = await (await navigator.gpu.requestAdapter()).requestDevice();
        const context = document.getElementById('canvas').getContext('webgpu');
        context.configure({ device, format: 'rgba8unorm' });
        const encoder = device.createCommandEncoder();
        encoder.beginRenderPass({ colorAttachments: [{ view: context.getCurrentTexture().createView(), clearValue: [0, 1, 0, 1], loadOp: 'clear', storeOp: 'store' }] }).end();
        device.queue.submit([encoder.finish()]);
        globalThis.gpuTestResult = 'ok';
    })().catch(error => { globalThis.gpuTestResult = String(error); });"#, "gpu-test").unwrap();
    assert_eq!(wait(&engine, "globalThis.gpuTestResult ?? 'pending'"), "ok");
    let node = host.borrow().pages[&1]
        .doc
        .nodes
        .iter()
        .position(|n| n.tag == "canvas")
        .unwrap();
    let image = present(&host, node);
    assert_eq!(&image.bytes()[..4], &[0, 255, 0, 255]);
    host.borrow_mut().pages.get_mut(&1).unwrap().dirty = false;
    std::thread::sleep(Duration::from_millis(20));
    for _ in 0..3 {
        assert_eq!(engine.tick().unwrap(), -1);
        let mut state = host.borrow_mut();
        let gpu = state.graphics.get_mut(&1).unwrap();
        assert!(!gpu.presented());
        assert_eq!(gpu.image(node).unwrap().unwrap().id, image.id);
        assert!(!state.pages[&1].dirty);
        assert!(!state.dom[&1].frame_requested);
    }
}
