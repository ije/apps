use apps_runtime::{
    host::{Host, Page, Role},
    html::Document,
    jsc::{Api, Engine},
};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

fn page() -> (Engine, Rc<RefCell<Host>>) {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<body></body>"),
            dirty: false,
        },
    );
    (
        Engine::new(Api::load().unwrap(), host.clone(), Role::Renderer(1)).unwrap(),
        host,
    )
}

#[test]
fn page_networks_are_public_and_main_can_use_localhost() {
    let (renderer, host) = page();
    let main = Engine::new(Api::load().unwrap(), host, Role::Main).unwrap();
    let port = main
        .eval(
            "globalThis.server=__apps.serve({port:0,fetch:()=>new Response('ok')});server.port",
            "test",
        )
        .unwrap();
    main.eval(&format!("globalThis.result='';fetch('http://127.0.0.1:{port}').then(r=>r.text()).then(v=>result=v)"), "test").unwrap();
    renderer.eval(&format!(r#"globalThis.blocked=0;for(const host of ['127.0.0.1','localhost','[::1]','[::ffff:127.0.0.1]'])fetch(`http://${{host}}:{port}`).then(()=>{{throw Error('private fetch allowed')}},e=>{{if(!String(e).includes('private networks'))throw e;blocked++}});const socket=new WebSocket('ws://127.0.0.1:{port}');socket.onerror=()=>blocked++"#), "test").unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        main.tick().unwrap();
        renderer.tick().unwrap();
        if main.eval("result", "test").unwrap() == "ok"
            && renderer.eval("blocked", "test").unwrap() == "5"
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(main.eval("result", "test").unwrap(), "ok");
    assert_eq!(renderer.eval("blocked", "test").unwrap(), "5");
    main.eval("server.stop()", "test").unwrap();
}

#[test]
fn weak_sweeps_free_detached_nodes_and_preserve_live_subtrees() {
    let (engine, host) = page();
    engine.eval(r#"globalThis.kept=document.createElement('section');kept.innerHTML='<b>saved</b>';kept.firstChild.addEventListener('test',()=>kept.value=42);document.body.append(kept);globalThis.live=kept.firstChild;kept.remove();"#, "test").unwrap();
    for _ in 0..15 {
        engine.eval("(()=>{for(let i=0;i<2000;i++){const item=document.createElement('div');item.innerHTML='<span>gone</span>';item.addEventListener('x',()=>item.textContent);document.body.append(item);item.remove()}})()", "test").unwrap();
        engine.collect_garbage();
        engine.eval("__appsDomSweep(true)", "test").unwrap();
    }
    assert!(
        host.borrow().pages[&1].doc.nodes.len() < 20000,
        "{} nodes",
        host.borrow().pages[&1].doc.nodes.len()
    );
    assert_eq!(engine.eval("live.dispatchEvent(new Event('test'));[live.parentNode===kept,live.textContent,kept.value].join(':')", "test").unwrap(), "true:saved:42");
}

#[test]
fn weak_finalizers_run_without_finalization_registry() {
    let engine = Engine::new(Api::load().unwrap(), Rc::default(), Role::Main).unwrap();
    engine.eval("globalThis.released=0;globalThis.registry=new __appsFinalizers(()=>released++);(()=>{for(let i=0;i<10000;i++)registry.register({},i)})()", "test").unwrap();
    for _ in 0..20 {
        engine.collect_garbage();
        engine.tick().unwrap();
        if engine
            .eval("released", "test")
            .unwrap()
            .parse::<usize>()
            .unwrap()
            >= 9000
        {
            break;
        }
        engine
            .eval(
                "(()=>{const values=Array.from({length:10000},()=>({}));})()",
                "test",
            )
            .unwrap();
    }
    assert!(
        engine
            .eval("released", "test")
            .unwrap()
            .parse::<usize>()
            .unwrap()
            >= 9000
    );
}

#[test]
fn binary_files_and_compression_preserve_views_and_large_data() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(Api::load().unwrap(), Rc::default(), Role::Main).unwrap();
    engine
        .eval(
            &format!(
                "globalThis.path={}",
                serde_json::json!(dir.path().join("data.bin"))
            ),
            "test",
        )
        .unwrap();
    engine.eval(r#"globalThis.done=false;globalThis.failure='';(async()=>{const fs=__apps.fs;const data=Uint8Array.from({length:8*1024*1024+4},(_,i)=>i%251);const part=data.subarray(2,-2);fs.writeFileSync(path,part);if(Buffer.compare(fs.readFileSync(path),part))throw Error('sync bytes');await fs.promises.writeFile(path,part);if(Buffer.compare(await fs.promises.readFile(path),part))throw Error('async bytes');const compressed=await new Response(new Blob([part]).stream().pipeThrough(new CompressionStream('gzip'))).arrayBuffer();const decoded=new Uint8Array(await new Response(new Blob([compressed]).stream().pipeThrough(new DecompressionStream('gzip'))).arrayBuffer());if(Buffer.compare(decoded,part))throw Error('compression bytes');done=true})().catch(e=>failure=String(e))"#, "test").unwrap();
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until && engine.eval("done||failure", "test").unwrap() == "" {
        engine.tick().unwrap();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(engine.eval("failure", "test").unwrap(), "");
    assert_eq!(engine.eval("done", "test").unwrap(), "true");
}

#[test]
fn event_emitters_support_legacy_constructors_and_prepend() {
    let engine = Engine::new(Api::load().unwrap(), Rc::default(), Role::Main).unwrap();
    assert_eq!(engine.eval(r#"(()=>{const E=__apps.events.EventEmitter;function Old(){E.call(this)}Object.setPrototypeOf(Old.prototype,E.prototype);const e=new Old(),out=[];e.setMaxListeners(20).on('x',()=>out.push('last')).prependListener('x',()=>out.push('first')).prependOnceListener('x',()=>out.push('once'));e.emit('x');e.emit('x');return e.getMaxListeners()+':'+out.join(',')})()"#, "test").unwrap(), "20:once,first,last,first,last");
}

#[test]
fn published_binary_bridge_values_survive_reload_without_aliasing() {
    let (_, host) = page();
    let api = Api::load().unwrap();
    let preload = Engine::new(api.clone(), host.clone(), Role::Preload(1)).unwrap();
    preload.eval("__apps.contextBridge.exposeInMainWorld('first',new Uint8Array([1,2,3]).subarray(1));__apps.contextBridge.exposeInMainWorld('second',new Uint8Array([4,5]));", "test").unwrap();
    for _ in 0..2 {
        let page = Engine::new(api.clone(), host.clone(), Role::Renderer(1)).unwrap();
        assert_eq!(
            page.eval("[...first,...second].join(',')", "test").unwrap(),
            "2,3,4,5"
        );
        page.eval("first[0]=99;second[0]=99", "test").unwrap();
    }
}
