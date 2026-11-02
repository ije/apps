use apps_runtime::{
    dom::Document,
    host::{Host, Page, Role},
    jsc::{JavaScriptCore, JsContext},
};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

fn page() -> (JsContext, Rc<RefCell<Host>>) {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<body></body>"),
            dirty: false,
        },
    );
    (
        JsContext::new(
            JavaScriptCore::load().unwrap(),
            host.clone(),
            Role::Renderer(1),
        )
        .unwrap(),
        host,
    )
}

#[test]
fn page_networks_are_public_except_own_servers() {
    let (renderer, host) = page();
    let main = JsContext::new(JavaScriptCore::load().unwrap(), host, Role::Main).unwrap();
    let port = main
        .eval(
            "globalThis.server=__apps.serve({port:0,fetch:()=>new Response('ok')});server.port",
            "test",
        )
        .unwrap();
    main.eval(&format!("globalThis.result='';fetch('http://127.0.0.1:{port}').then(r=>r.text()).then(v=>result=v)"), "test").unwrap();
    renderer.eval(&format!(r#"globalThis.allowed=0;globalThis.blocked=0;for(const host of ['127.0.0.1','localhost'])fetch(`http://${{host}}:{port}`).then(r=>r.text()).then(v=>{{if(v!=='ok')throw Error(v);allowed++}});for(const url of ['http://[::ffff:127.0.0.1]:{port}','http://127.0.0.1:1','http://localhost:1'])fetch(url).then(()=>{{throw Error('private fetch allowed')}},e=>{{if(!String(e).includes('private networks'))throw e;blocked++}});const socket=new WebSocket('ws://127.0.0.1:1');socket.onerror=()=>blocked++"#), "test").unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        main.tick().unwrap();
        renderer.tick().unwrap();
        if main.eval("result", "test").unwrap() == "ok"
            && renderer.eval("allowed", "test").unwrap() == "2"
            && renderer.eval("blocked", "test").unwrap() == "4"
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(main.eval("result", "test").unwrap(), "ok");
    assert_eq!(renderer.eval("allowed", "test").unwrap(), "2");
    assert_eq!(renderer.eval("blocked", "test").unwrap(), "4");
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
    let engine =
        JsContext::new(JavaScriptCore::load().unwrap(), Rc::default(), Role::Main).unwrap();
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
    let engine =
        JsContext::new(JavaScriptCore::load().unwrap(), Rc::default(), Role::Main).unwrap();
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
fn node_bytes_lines_and_handles() {
    let dir = tempfile::tempdir().unwrap();
    let engine =
        JsContext::new(JavaScriptCore::load().unwrap(), Rc::default(), Role::Main).unwrap();
    engine.eval(&format!("globalThis.file={};globalThis.failure='';globalThis.handles=null;(async()=>{{{}}})().catch(e=>failure=String(e)+e.stack)", serde_json::json!(dir.path().join("file")), r#"
        const {assert,fs,stream,readline,crypto,sqlite}=__apps;
        const input=new stream.PassThrough(),lines=[];readline.createInterface({input}).on('line',line=>lines.push(line));
        for(const chunk of ['a\r','\nb\r','c','\r','','d\n','\re\n','f\r'])input.write(chunk);input.end();await new Promise(resolve=>setTimeout(resolve,5));
        assert.deepStrictEqual(lines,['a','b','c','d','','e','f']);
        assert.strictEqual(Buffer.alloc(10).fill('abc').toString(),'abcabcabca');
        assert.strictEqual(Buffer.alloc(7).fill(Buffer.from([1,2]),1,6).join(),'0,1,2,1,2,1,0');
        assert.strictEqual(Buffer.alloc(4).fill('z',1).toString('latin1'),'\0zzz');
        assert.strictEqual(crypto.createHmac('sha256','key').update('The quick brown fox jumps over the lazy dog').digest('hex'),'f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8');
        const parts=Array.from({length:40},(_,i)=>Buffer.alloc(4096,i)),hmac=crypto.createHmac('sha512','k');for(const part of parts)hmac.update(part);
        assert.strictEqual(hmac.digest('base64'),crypto.createHmac('sha512','k').update(Buffer.concat(parts)).digest('base64'));
        const all=Uint8Array.from({length:256},(_,i)=>i),row=new sqlite.DatabaseSync(':memory:').prepare('select ? as b').get(all.subarray(1));
        assert.ok(row.b instanceof Uint8Array&&row.b.length===255&&row.b[0]===1&&row.b[254]===255,'SQLite blob');
        fs.writeFileSync(file,'x');
        try{fs.utimesSync(file,1e300,-1e300)}catch{}
        const raw=await new Promise((resolve,reject)=>fs.open(file,'r',(error,fd)=>error?reject(error):resolve(fd)));
        handles={raw,lost:(await fs.promises.open(file)).fd};
    "#), "test").unwrap();
    let open = |name: &str| {
        engine
            .eval(
                &format!("(()=>{{try{{__apps.fs.fstatSync(handles.{name});return true}}catch{{return false}}}})()"),
                "test",
            )
            .unwrap()
            == "true"
    };
    let until = Instant::now() + Duration::from_secs(10);
    while engine.eval("!!(failure||handles)", "test").unwrap() == "false" {
        assert!(Instant::now() < until, "Test timed out");
        engine.tick().unwrap();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(engine.eval("failure", "test").unwrap(), "");
    assert!(open("lost"));
    for _ in 0..20 {
        if !open("lost") {
            break;
        }
        engine.collect_garbage();
        engine.tick().unwrap();
    }
    assert!(!open("lost"), "A forgotten FileHandle kept its descriptor");
    assert!(open("raw"), "A callback descriptor was closed");
}

#[test]
fn event_emitters_support_legacy_constructors_and_prepend() {
    let engine =
        JsContext::new(JavaScriptCore::load().unwrap(), Rc::default(), Role::Main).unwrap();
    assert_eq!(engine.eval(r#"(()=>{const E=__apps.events.EventEmitter;function Old(){E.call(this)}Object.setPrototypeOf(Old.prototype,E.prototype);const e=new Old(),out=[];e.setMaxListeners(20).on('x',()=>out.push('last')).prependListener('x',()=>out.push('first')).prependOnceListener('x',()=>out.push('once'));e.emit('x');e.emit('x');return e.getMaxListeners()+':'+out.join(',')})()"#, "test").unwrap(), "20:once,first,last,first,last");
}

#[test]
fn published_binary_bridge_values_survive_reload_without_aliasing() {
    let (_, host) = page();
    let api = JavaScriptCore::load().unwrap();
    let preload = JsContext::new(api.clone(), host.clone(), Role::Preload(1)).unwrap();
    preload.eval("__apps.contextBridge.exposeInMainWorld('first',new Uint8Array([1,2,3]).subarray(1));__apps.contextBridge.exposeInMainWorld('second',new Uint8Array([4,5]));", "test").unwrap();
    for _ in 0..2 {
        let page = JsContext::new(api.clone(), host.clone(), Role::Renderer(1)).unwrap();
        assert_eq!(
            page.eval("[...first,...second].join(',')", "test").unwrap(),
            "2,3,4,5"
        );
        page.eval("first[0]=99;second[0]=99", "test").unwrap();
    }
}
