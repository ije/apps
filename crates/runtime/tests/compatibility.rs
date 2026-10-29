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

fn run(source: &str) {
    let engine = Engine::new(Api::load().unwrap(), Rc::default(), Role::Main).unwrap();
    engine.eval(&format!("globalThis.result='pending';(async()=>{{{source}}})().then(()=>result='ok',e=>result=String(e)+'\\n'+e.stack);"),"compatibility").unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        engine.tick().unwrap();
        let result = engine.eval("result", "test").unwrap();
        if result != "pending" {
            assert_eq!(result, "ok");
            break;
        }
        assert!(Instant::now() < deadline, "Test timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn buffer_streams_and_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = serde_json::to_string(&dir.path().join("file.txt")).unwrap();
    run(&format!(
        r#"
        const assert=(c,m)=>{{if(!c)throw Error(m)}};
        const b=Buffer.from('Hello 世界');assert(Buffer.from(b.toString('base64'),'base64').equals(b),'base64');
        const n=Buffer.alloc(8);n.writeBigUInt64LE(12345678901234n);assert(n.readBigUInt64LE()===12345678901234n,'bigint');
        const fs=__apps.fs;await fs.promises.writeFile({path},b);assert((await fs.promises.readFile({path})).equals(b),'file');
        const handle=await fs.promises.open({path},'r');const small=Buffer.alloc(5);await handle.read(small,0,5,0);assert(small.toString()==='Hello','handle');const first=Buffer.alloc(1);await handle.read(first,0,1,null);assert(first.toString()==='H','positioned read preserves offset');await handle.close();
        const chunks=[];for await(const chunk of fs.createReadStream({path},{{highWaterMark:4}}))chunks.push(chunk);assert(Buffer.concat(chunks).equals(b),'read stream');
        const upper=new __apps.stream.Transform({{transform(chunk,encoding,callback){{callback(null,Buffer.from(chunk.toString().toUpperCase()));}}}});
        const out=[];const sink=new __apps.stream.Writable({{write(chunk,encoding,callback){{out.push(chunk.toString());callback();}}}});
        await __apps.stream.promises.pipeline(__apps.stream.Readable.from(['a','b']),upper,sink);assert(out.join('')==='AB','pipeline');
        await fs.promises.rename({path},{path}+'.moved');assert(!fs.existsSync({path}),'rename');await fs.promises.rm({path}+'.moved');
    "#
    ));
}

#[test]
fn bridge_preload_calls_main_and_publishes_events() {
    let api = Api::load().unwrap();
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<body></body>"),
            dirty: false,
        },
    );
    let main = Engine::new(api.clone(), host.clone(), Role::Main).unwrap();
    let preload = Engine::new(api.clone(), host.clone(), Role::Preload(1)).unwrap();
    main.eval("__apps.ipcMain.handle('sum',(_e,a,b)=>a+b);__apps.ipcMain.handle('double',(_e,v)=>v.map(x=>x*2));__apps.ipcMain.on('version',e=>e.returnValue='v1')","test").unwrap();
    preload.eval("__apps.contextBridge.exposeInMainWorld('desktop',{sum:(a,b)=>__apps.ipcRenderer.invoke('sum',a,b),double:v=>__apps.ipcRenderer.invoke('double',v),version:__apps.ipcRenderer.sendSync('version'),subscribe:callback=>{const listener=(_event,data)=>callback(data);__apps.ipcRenderer.on('change',listener);return()=>__apps.ipcRenderer.off('change',listener);}});","test").unwrap();
    let page = Engine::new(api, host, Role::Renderer(1)).unwrap();
    assert_eq!(page.eval("desktop.version", "test").unwrap(), "v1");
    page.eval(
        "globalThis.result='pending';desktop.sum(3,4).then(v=>result=v,e=>result=String(e));globalThis.bytes='pending';desktop.double(new Float32Array([1.5,-2,0.25])).then(v=>bytes=v.constructor.name+':'+[...v],e=>bytes=String(e));",
        "test",
    )
    .unwrap();
    for _ in 0..10 {
        main.tick().unwrap();
        preload.tick().unwrap();
        page.tick().unwrap();
    }
    assert_eq!(page.eval("result", "test").unwrap(), "7");
    assert_eq!(page.eval("bytes", "test").unwrap(), "Float32Array:3,-4,0.5");
    page.eval("globalThis.observed=[];globalThis.unsubscribe=desktop.subscribe(value=>observed.push(value));if(typeof unsubscribe!=='function')throw Error('unsubscribe must be synchronous');", "test").unwrap();
    main.eval("__apps.send(1,'change',{value:42});", "test")
        .unwrap();
    assert_eq!(page.eval("observed[0].value", "test").unwrap(), "42");
    page.eval("unsubscribe();", "test").unwrap();
    main.eval("__apps.send(1,'change',{value:43});", "test")
        .unwrap();
    assert_eq!(page.eval("observed.length", "test").unwrap(), "1");
    assert_eq!(
        page.eval("typeof process+':'+typeof __apps", "test")
            .unwrap(),
        "undefined:undefined"
    );
}

#[test]
fn bridge_capabilities_failures_and_closed_contexts() {
    let api = Api::load().unwrap();
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<body></body>"),
            dirty: false,
        },
    );
    let main = Engine::new(api.clone(), host.clone(), Role::Main).unwrap();
    let preload = Engine::new(api.clone(), host.clone(), Role::Preload(1)).unwrap();
    main.eval("__apps.ipcMain.handle('wait',()=>new Promise(()=>{}));const receive=__appsBridgeReceive;__appsBridgeReceive=message=>{globalThis.lastMessage=message;return receive(message)};", "test").unwrap();
    preload.eval("__apps.contextBridge.exposeInMainWorld('desktop',{value:()=>7,callback:fn=>fn(),fail:()=>Promise.reject(0),wait:()=>__apps.ipcRenderer.invoke('wait')});", "test").unwrap();
    let token = host.borrow().bridge[&1]["desktop"].value["items"]["value"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        token.len() > 30,
        "Callback capabilities must not use guessable counters"
    );
    let denied=preload.eval(&format!("__appsBridgeReceive({{kind:'call',function:{},sender:{{renderer:2}},args:{{type:'array',id:0,items:[]}}}})",serde_json::to_string(&token).unwrap()),"test").unwrap_err();
    assert!(denied.to_string().contains("unavailable to this context"));
    let page = Engine::new(api.clone(), host.clone(), Role::Renderer(1)).unwrap();
    assert_eq!(page.eval("desktop.value()", "test").unwrap(), "7");
    assert!(page.eval("(()=>{try{desktop.callback(()=>{throw Error('callback failed')})}catch(e){return e.message}})()","test").unwrap().ends_with("callback failed"));
    page.eval("globalThis.rejected='pending';desktop.fail().then(()=>rejected='wrong',e=>rejected=e.message)","test").unwrap();
    for _ in 0..3 {
        preload.tick().unwrap();
        page.tick().unwrap();
    }
    assert_eq!(page.eval("rejected", "test").unwrap(), "0");
    page.eval(
        "globalThis.result='pending';desktop.wait().then(v=>result=v,e=>result=e.message)",
        "test",
    )
    .unwrap();
    let request = main.eval("lastMessage.id", "test").unwrap();
    preload.eval(&format!("__appsBridgeReceive({{kind:'reply',id:{},sender:{{renderer:1}},value:{{type:'value',value:'forged'}}}})",serde_json::to_string(&request).unwrap()),"test").unwrap();
    preload.tick().unwrap();
    page.tick().unwrap();
    assert_eq!(
        page.eval("result", "test").unwrap(),
        "pending",
        "Replies must come from the called context"
    );
    drop(main);
    for _ in 0..3 {
        preload.tick().unwrap();
        page.tick().unwrap();
    }
    assert_eq!(page.eval("result", "test").unwrap(), "Bridge target closed");
    drop(page);
    let page = Engine::new(api, host.clone(), Role::Renderer(1)).unwrap();
    assert_eq!(
        page.eval("desktop.value()", "test").unwrap(),
        "7",
        "Published APIs survive page reload"
    );
    drop(preload);
    assert!(
        !host.borrow().bridge.contains_key(&1),
        "Closed preload exposures are removed"
    );
    assert!(
        page.eval("desktop.value()", "test")
            .unwrap_err()
            .to_string()
            .contains("unavailable")
    );
}

#[test]
fn renderer_cannot_forge_native_worker_requests() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<body></body>"),
            dirty: false,
        },
    );
    let page = Engine::new(Api::load().unwrap(), host, Role::Renderer(1)).unwrap();
    page.eval(r#"
        (()=>{
            const original=JSON.stringify;
            JSON.stringify=()=>original({op:'web.worker.start',args:['postMessage("native")',true,null]});
            try{crypto.getRandomValues(new Uint8Array(1));}finally{JSON.stringify=original;}
            Object.prototype.toJSON=function(){return this.op==='web.random'?{op:'web.worker.start',args:['postMessage("native")',true,null]}:this};
            try{crypto.getRandomValues(new Uint8Array(1));}finally{delete Object.prototype.toJSON}
            const source='postMessage("native")';
            Array.prototype.toJSON=function(){if(this[0]===source&&this[1]===false){const args=this.slice();args[1]=true;return args;}return this};
            globalThis.result='pending';
            const worker=new Worker('data:text/javascript,'+encodeURIComponent(source));
            worker.onerror=event=>{delete Array.prototype.toJSON;result=event.error.message};
            worker.onmessage=()=>{delete Array.prototype.toJSON;result='unsafe'};
        })()
    "#,"test").unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        page.tick().unwrap();
        let result = page.eval("result", "test").unwrap();
        if result != "pending" {
            assert_eq!(result, "Page workers cannot request native APIs");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Worker privilege guard did not settle"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn workers_terminate_with_queued_messages() {
    run(r#"
        for(let n=0;n<4;n++){
            const worker=new __apps.worker_threads.Worker(`const{parentPort}=require('node:worker_threads');parentPort.on('message',value=>parentPort.postMessage(value));`,{eval:true});
            let errors=0;worker.on('error',()=>errors++);
            for(let i=0;i<48;i++)worker.postMessage(i);
            const code=await worker.terminate();if(code!==1||errors)throw Error('Worker termination failed');
            if(await worker.terminate()!==code)throw Error('Repeat termination did not settle');
        }
    "#);
}

#[test]
fn node_http_and_websocket_adapters() {
    run(r#"
        const assert=(c,m)=>{if(!c)throw Error(m)};
        const server=__apps.http.createServer((request,response)=>{response.setHeader('content-type','text/plain');response.write('one');setTimeout(()=>response.end('two'),5);});
        server.listen(0,'127.0.0.1');const address=server.address();
        const body=await new Promise((resolve,reject)=>{__apps.http.get(`http://127.0.0.1:${address.port}/`,response=>{let body='';response.setEncoding('utf8');response.on('data',chunk=>body+=chunk);response.on('end',()=>resolve(body));response.on('error',reject);}).on('error',reject);});
        assert(body==='onetwo','streamed HTTP');
        const wss=new __apps.ws.WebSocketServer({server});wss.on('connection',ws=>ws.on('message',data=>ws.send(data)));
        const ws=new __apps.ws.WebSocket(`ws://127.0.0.1:${address.port}/`);
        const result=await new Promise((resolve,reject)=>{ws.on('open',()=>ws.send('echo'));ws.on('message',data=>resolve(data.toString()));ws.on('error',reject);});
        assert(result==='echo','WebSocket adapter');ws.close();wss.close();server.close();
    "#);
}

#[cfg(unix)]
#[test]
fn terminal_has_real_tty_and_can_resize() {
    run(r#"
        const terminal=__apps.pty.spawn('/bin/sh',['-c','stty size; test -t 0 && printf tty'],{cols:91,rows:32});
        let output='';terminal.onData(data=>output+=data);
        await new Promise(resolve=>terminal.onExit(resolve));
        if(!output.includes('32 91')||!output.includes('tty'))throw Error('Not a PTY: '+output);
    "#);
}

#[cfg(unix)]
#[test]
fn synchronous_child_processes_and_errors() {
    run(r#"
        const child=__apps.child_process;
        const result=child.spawnSync('/bin/sh',['-c','cat; printf failure >&2; exit 7'],{input:'hello',encoding:'utf8'});
        if(result.status!==7||result.stdout!=='hello'||result.stderr!=='failure')throw Error(JSON.stringify(result));
        const shell=child.spawnSync('printf shell',{shell:true,encoding:'utf8'});if(shell.stdout!=='shell')throw Error('shell option');
        const output=child.execFileSync('/bin/echo',['ok'],{encoding:'utf8'});
        if(output!=='ok\n')throw Error('execFileSync output');
        const timeout=child.spawnSync('/bin/sleep',['10'],{timeout:20});
        if(timeout.error?.code!=='ETIMEDOUT')throw Error('missing timeout error');
        let missing=false;try{__apps.fs.readFileSync('/does/not/exist/apps-test')}catch(e){missing=e.code==='ENOENT'}
        if(!missing)throw Error('synchronous file error code');
    "#);
}

#[test]
fn node_core_modules() {
    run(r#"
        const {assert,util,path,os,url,querystring,events,crypto,zlib,string_decoder:{StringDecoder}}=__apps;
        assert.deepStrictEqual(Buffer.concat([Buffer.alloc(0),Buffer.from('ab')]),Buffer.from('ab'));
        assert.strictEqual(Buffer.alloc(5).fill('ab').toString(),'ababa');
        assert.strictEqual(Buffer.from('abcabc').lastIndexOf('b'),4);
        const word=Buffer.alloc(6);word.writeUIntBE(0x123456789a,0,5);assert.strictEqual(word.readUIntBE(0,5),0x123456789a);
        assert.strictEqual(util.format('%s=%d %j','a',5,{x:1}),'a=5 {"x":1}');
        assert.strictEqual(util.inspect({a:[1,{b:new Map([[1,2]])}],c:'x'}),"{ a: [ 1, { b: [Map] } ], c: 'x' }");
        assert.strictEqual(util.inspect(Buffer.from('hi')),'<Buffer 68 69>');
        assert.throws(()=>assert.deepStrictEqual({a:1},{a:'1'}),{code:'ERR_ASSERTION'});
        await assert.rejects(Promise.reject(new TypeError('x')),TypeError);
        assert.deepStrictEqual(util.parseArgs({args:['-v','--name=x','pos'],options:{verbose:{type:'boolean',short:'v'},name:{type:'string'}},allowPositionals:true}).positionals,['pos']);
        assert.deepStrictEqual(path.parse('/home/u/f.txt'),{root:'/',dir:'/home/u',base:'f.txt',ext:'.txt',name:'f'});
        assert.strictEqual(path.win32.format({dir:'C:\\a',name:'b',ext:'.c'}),'C:\\a\\b.c');
        assert.strictEqual(url.fileURLToPath(url.pathToFileURL('/tmp/a b#c')),'/tmp/a b#c');
        assert.ok(os.totalmem()>0&&os.cpus().length>0&&!os.tmpdir().endsWith('/')&&typeof os.userInfo().username==='string');
        assert.deepStrictEqual(querystring.parse('a=1&a=2&b=x+y'),Object.assign(Object.create(null),{a:['1','2'],b:'x y'}));
        const decoder=new StringDecoder('utf8'),euro=Buffer.from('€');assert.strictEqual(decoder.write(euro.subarray(0,1))+decoder.end(euro.subarray(1)),'€');
        const emitter=new events.EventEmitter();let added;emitter.on('newListener',name=>added=name);emitter.on('x',()=>{});assert.strictEqual(added,'x');
        setTimeout(()=>{emitter.emit('tick',1);emitter.emit('tick',2);},1);const ticks=[];for await(const [value] of events.on(emitter,'tick')){ticks.push(value);if(ticks.length===2)break;}assert.deepStrictEqual(ticks,[1,2]);
        assert.strictEqual(crypto.createHash('md5').update('a').digest('hex'),'0cc175b9c0f1b6a831c399e269772661');
        const big=crypto.createHash('sha256');for(let i=0;i<40;i++)big.update(Buffer.alloc(4096,i));const copy=big.copy();assert.strictEqual(big.digest('hex'),copy.digest('hex'));
        const key=crypto.randomBytes(32),iv=crypto.randomBytes(12),cipher=crypto.createCipheriv('aes-256-gcm',key,iv);
        const sealed=Buffer.concat([cipher.update('secret','utf8'),cipher.final()]),decipher=crypto.createDecipheriv('aes-256-gcm',key,iv);decipher.setAuthTag(cipher.getAuthTag());
        assert.strictEqual(Buffer.concat([decipher.update(sealed),decipher.final()]).toString(),'secret');
        assert.strictEqual((await util.promisify(crypto.pbkdf2)('pw','salt',1000,16,'sha256')).toString('hex'),crypto.pbkdf2Sync('pw','salt',1000,16,'sha256').toString('hex'));
        assert.strictEqual(zlib.gunzipSync(zlib.gzipSync('hello')).toString(),'hello');
        assert.strictEqual((await util.promisify(zlib.inflate)(zlib.deflateSync('async'))).toString(),'async');
        assert.throws(()=>zlib.gunzipSync(Buffer.from('nope')),{code:'Z_DATA_ERROR'});
        assert.strictEqual(typeof setTimeout(()=>{},0).unref().hasRef,'function');
        assert.strictEqual(await util.promisify(setTimeout)(1,'done'),'done');
        assert.ok(process.memoryUsage().rss>0&&process.cpuUsage().user>=0&&process.versions.node);
        assert.ok(__apps.module.isBuiltin('node:readline/promises'));
    "#);
}

#[cfg(unix)]
#[test]
fn node_files_streams_and_processes() {
    let dir = tempfile::tempdir().unwrap();
    let root = serde_json::to_string(dir.path()).unwrap();
    run(&format!(
        r#"
        const {{assert,fs,path,stream,readline,zlib,crypto,util,child_process}}=__apps,root={root};
        const file=path.join(root,'a','lines.txt');
        fs.mkdirSync(path.dirname(file),{{recursive:true}});fs.writeFileSync(file,'one\r\ntwo\nthree');
        try{{fs.readFileSync(path.join(root,'missing'))}}catch(error){{assert.strictEqual(error.message,`ENOENT: no such file or directory, open '${{path.join(root,'missing')}}'`);assert.strictEqual(error.syscall,'open');}}
        assert.strictEqual(fs.statSync(path.join(root,'missing'),{{throwIfNoEntry:false}}),undefined);
        const stats=fs.statSync(file);assert.ok(stats.isFile()&&stats.atime instanceof Date&&stats.ctimeMs>0&&stats.nlink===1);
        assert.deepStrictEqual(fs.readdirSync(root,{{recursive:true}}).sort(),['a',path.join('a','lines.txt')]);
        const entry=fs.readdirSync(root,{{recursive:true,withFileTypes:true}}).find(item=>item.isFile());assert.strictEqual(entry.name,'lines.txt');assert.strictEqual(entry.parentPath,path.join(root,'a'));
        fs.utimesSync(file,1000,2000);assert.strictEqual(fs.statSync(file).mtimeMs,2000000);
        fs.cpSync(path.join(root,'a'),path.join(root,'b'),{{recursive:true}});assert.strictEqual(fs.readFileSync(path.join(root,'b','lines.txt'),'utf8'),'one\r\ntwo\nthree');
        fs.copyFileSync(file,path.join(root,'b','copy.txt'));assert.throws(()=>fs.copyFileSync(file,path.join(root,'b','copy.txt'),fs.constants.COPYFILE_EXCL),{{code:'EEXIST'}});
        fs.truncateSync(path.join(root,'b','copy.txt'),3);assert.strictEqual(fs.readFileSync(path.join(root,'b','copy.txt'),'utf8'),'one');
        const lines=[];for await(const line of readline.createInterface({{input:fs.createReadStream(file,{{highWaterMark:2}})}}))lines.push(line);assert.deepStrictEqual(lines,['one','two','three']);
        class Upper extends stream.Transform{{_transform(chunk,encoding,callback){{callback(null,chunk.toString().toUpperCase());}}}}
        const output=path.join(root,'out.gz');
        await stream.promises.pipeline(fs.createReadStream(file),new Upper(),async function*(source){{for await(const chunk of source)yield chunk;}},zlib.createGzip(),fs.createWriteStream(output));
        assert.strictEqual(zlib.gunzipSync(fs.readFileSync(output)).toString(),'ONE\r\nTWO\nTHREE');
        const hash=await new Promise((resolve,reject)=>fs.createReadStream(file).pipe(crypto.createHash('sha1')).setEncoding('hex').on('data',resolve).on('error',reject));
        assert.strictEqual(hash,crypto.createHash('sha1').update(fs.readFileSync(file)).digest('hex'));
        assert.strictEqual(await stream.consumers.text(stream.Readable.from(['a','b'])),'ab');
        const exec=util.promisify(child_process.exec);
        const {{stdout}}=await exec('echo "$0"',{{shell:'/bin/sh'}});assert.strictEqual(stdout.trim(),'/bin/sh');
        await assert.rejects(exec('echo bad >&2; exit 3'),error=>error.code===3&&error.stderr==='bad\n');
        process.env.APPS_ENV_TEST='inherited';assert.strictEqual((await exec('printf "$APPS_ENV_TEST"')).stdout,'inherited');assert.strictEqual(child_process.execSync('printf "$APPS_ENV_TEST"',{{encoding:'utf8'}}),'inherited');
    "#
    ));
}

#[test]
fn data_urls_load_binary_wasm_without_a_server() {
    run(r#"
        const raw=await fetch('data:application/octet-stream,%00%FF%23#ignored');
        if(new Uint8Array(await raw.arrayBuffer()).join(',')!=='0,255,35')throw Error('data bytes');
        const result=await WebAssembly.instantiateStreaming(fetch('data:application/wasm;base64,AGFzbQEAAAA='));
        if(!(result.instance instanceof WebAssembly.Instance))throw Error('Wasm data URL');
    "#);
}
