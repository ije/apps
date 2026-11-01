use apps_runtime::{
    host::Role,
    jsc::{Api, Engine},
};
use std::{
    rc::Rc,
    time::{Duration, Instant},
};

fn run(source: &str) {
    let engine = Engine::new(Api::load().unwrap(), Rc::default(), Role::Main).unwrap();
    engine.eval(&format!("globalThis.result='pending'; (async()=>{{{source}}})().then(()=>result='ok',error=>result=String(error)+'\\n'+String(error.stack??''));"), "backend-tests").unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        engine.tick().unwrap();
        let result = engine.eval("result", "backend-result").unwrap();
        if result != "pending" {
            assert_eq!(result, "ok");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Backend test timed out: {}",
            engine.eval("globalThis.phase", "phase").unwrap()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn websocket_upgrade_protocols_binary_and_streaming_http() {
    run(r#"
        const assert=(v,m)=>{if(!v)throw Error(m)};
        let produced=false;
        const server=__apps.serve({hostname:'127.0.0.1',port:0,
            fetch(request,server){
                if(new URL(request.url).pathname==='/ws'){server.upgrade(request,{protocol:'chat'});return;}
                if(new URL(request.url).pathname==='/echo')return new Response(request.body);
                return new Response(new ReadableStream({start(c){c.enqueue(new TextEncoder().encode('first'));setTimeout(()=>{produced=true;c.enqueue(new TextEncoder().encode('second'));c.close()},200)}}));
            },websocket:{message(socket,data){socket.send(data)}}});
        const response=await fetch('http://127.0.0.1:'+server.port);
        const reader=response.body.getReader();
        const first=await reader.read();
        assert(new TextDecoder().decode(first.value)==='first'&&!produced,'Response is streamed before completion');
        assert(new TextDecoder().decode((await reader.read()).value)==='second','Second streamed chunk');
        assert((await reader.read()).done,'Stream ends');
        let uploaded=false;
        const echo=await fetch('http://127.0.0.1:'+server.port+'/echo',{method:'POST',body:new ReadableStream({start(c){c.enqueue(new TextEncoder().encode('upload-first'));setTimeout(()=>{uploaded=true;c.enqueue(new TextEncoder().encode('upload-second'));c.close()},200)}}),duplex:'half'});
        const echoReader=echo.body.getReader();
        assert(new TextDecoder().decode((await echoReader.read()).value)==='upload-first'&&!uploaded,'Request body streams before upload completes');
        assert(new TextDecoder().decode((await echoReader.read()).value)==='upload-second','Second request chunk');
        assert((await echoReader.read()).done,'Request echo stream ends');
        const socket=new WebSocket('ws://127.0.0.1:'+server.port+'/ws','chat');socket.binaryType='arraybuffer';
        await new Promise((resolve,reject)=>{socket.onopen=resolve;socket.onerror=()=>reject(Error('Socket failed'))});
        assert(socket.protocol==='chat','Negotiated protocol');
        let received=new Promise(resolve=>socket.onmessage=event=>resolve(event.data));socket.send('hello');
        assert(await received==='hello','Text roundtrip');
        received=new Promise(resolve=>socket.onmessage=event=>resolve(event.data));socket.send(new Uint8Array([0,255,3]));
        assert(new Uint8Array(await received).join(',')==='0,255,3','Binary roundtrip');
        const large=new Uint8Array(300000).map((_,i)=>i*7);received=new Promise(resolve=>socket.onmessage=event=>resolve(event.data));socket.send(large);
        assert(new Uint8Array(await received).every((v,i)=>v===large[i]),'Large binary roundtrip');
        const burst=[];received=new Promise(resolve=>socket.onmessage=event=>{burst.push(event.data);if(burst.length===300)resolve()});for(let i=0;i<300;i++)socket.send('m'+i);
        await received;assert(burst.every((v,i)=>v==='m'+i),'Burst sends are queued in order');
        const closed=new Promise(resolve=>socket.onclose=resolve);socket.close(3001,'complete');
        const close=await closed;assert(close.code===3001&&close.wasClean,'Close handshake');server.stop();
    "#);
}

#[test]
fn node_http_response_backpressure() {
    run(r#"
        let writes=0,full=0;
        const server=__apps.http.createServer((request,response)=>{const chunk=new Uint8Array(65536);const pump=()=>{while(writes<400){writes++;if(!response.write(chunk)){full++;response.once('drain',pump);return;}}response.end();};pump();}).listen(0,'127.0.0.1');
        await new Promise(resolve=>server.once('listening',resolve));
        const reader=(await fetch('http://127.0.0.1:'+server.address().port+'/')).body.getReader();let size=(await reader.read()).value.length;
        await new Promise(resolve=>setTimeout(resolve,100));const buffered=writes;
        for(;;){const {value,done}=await reader.read();if(done)break;size+=value.length;}server.close();
        if(!(full>0&&buffered<400&&size===400*65536))throw Error(`Unbounded response buffering: ${buffered} ${full} ${size}`);
    "#);
}

#[test]
fn signing_jwk_hmac_and_authenticated_encryption() {
    run(r#"
        const assert=(v,m)=>{if(!v)throw Error(m)}, bytes=new TextEncoder().encode('t3code');
        const pair=await crypto.subtle.generateKey({name:'ECDSA',namedCurve:'P-256'},true,['sign','verify']);
        const jwk=await crypto.subtle.exportKey('jwk',pair.privateKey);
        const secret=await crypto.subtle.importKey('jwk',jwk,{name:'ECDSA',namedCurve:'P-256'},false,['sign']);
        const signature=await crypto.subtle.sign({name:'ECDSA',hash:'SHA-256'},secret,bytes);
        assert(await crypto.subtle.verify({name:'ECDSA',hash:'SHA-256'},pair.publicKey,signature,bytes),'ECDSA verification');
        assert(!await crypto.subtle.verify({name:'ECDSA',hash:'SHA-256'},pair.publicKey,signature,new Uint8Array([1])),'Tampered message');
        let denied=false;try{await crypto.subtle.exportKey('jwk',secret)}catch{denied=true}assert(denied,'Nonextractable private key');
        const key=await crypto.subtle.importKey('raw',new Uint8Array(32).fill(7),'AES-GCM',false,['encrypt','decrypt']);
        const algorithm={name:'AES-GCM',iv:new Uint8Array(12),additionalData:bytes};
        const sealed=await crypto.subtle.encrypt(algorithm,key,bytes);
        assert(new TextDecoder().decode(await crypto.subtle.decrypt(algorithm,key,sealed))==='t3code','AES-GCM roundtrip');
        new Uint8Array(sealed)[0]^=1;denied=false;try{await crypto.subtle.decrypt(algorithm,key,sealed)}catch{denied=true}assert(denied,'Authenticated ciphertext');
        const hmac=await crypto.subtle.importKey('raw',bytes,{name:'HMAC',hash:'SHA-256'},false,['sign','verify']);
        const mac=await crypto.subtle.sign('HMAC',hmac,bytes);assert(await crypto.subtle.verify('HMAC',hmac,mac,bytes),'HMAC verification');
        assert(__apps.crypto.createHash('sha256').update('abc').digest('hex')==='ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad','Node hash');
    "#);
}

#[test]
fn workers_clone_sqlite_authorization_and_termination() {
    run(r#"
        const assert=(v,m)=>{if(!v)throw Error(m)};
        globalThis.phase="start";const {Worker}=__apps.worker_threads;
        const worker=new Worker(`const {parentPort,workerData}=require('node:worker_threads');const {DatabaseSync,constants}=require('node:sqlite');const db=new DatabaseSync(':memory:');db.exec('CREATE TABLE test(n INTEGER);INSERT INTO test VALUES (7)');db.setAuthorizer(action=>action===constants.SQLITE_DELETE?constants.SQLITE_DENY:constants.SQLITE_OK);parentPort.on('message',data=>{let denied=false;try{db.exec('DELETE FROM test')}catch{denied=true}parentPort.postMessage({n:db.prepare('SELECT * FROM test').get().n,columns:db.prepare('SELECT n FROM test').columns(),denied,data,workerData});});`,{eval:true,workerData:new Map([['key',4n]])});
        const message=new Promise((resolve,reject)=>{worker.on('message',resolve);worker.on('error',reject)});
        const cycle={bytes:new Uint8Array([1,2,3])};cycle.self=cycle;worker.postMessage(cycle);
        const reply=await message;phase="reply";
        assert(reply.n===7&&reply.denied,'Worker SQLite authorizer');
        assert(reply.columns[0].name==='n'&&reply.columns[0].table==='test','Column metadata');
        assert(reply.data.self===reply.data&&reply.data.bytes[2]===3&&reply.workerData.get('key')===4n,'Structured clone');
        await worker.terminate();phase="first-terminated";
        const echo=new Worker(`const {parentPort}=require('node:worker_threads');parentPort.on('message',data=>parentPort.postMessage(data))`,{eval:true}),replies=[];
        const all=new Promise(resolve=>echo.on('message',data=>{replies.push(data);if(replies.length===300)resolve()}));
        const buffer=new Uint8Array(256).map((_,i)=>i).buffer;for(let i=0;i<300;i++)echo.postMessage({i,buffer});
        await all;assert(replies.every((v,i)=>v.i===i&&new Uint8Array(v.buffer).every((b,j)=>b===j)),'Worker message bursts and buffers');await echo.terminate();phase="echo-terminated";
        const source=URL.createObjectURL(new Blob([`onmessage=event=>{postMessage(event.data+1);close()}`],{type:'text/javascript'}));
        const browser=new globalThis.Worker(source), echoed=new Promise((resolve,reject)=>{browser.onmessage=event=>resolve(event.data);browser.onerror=event=>reject(event.error)});browser.postMessage(4);
        assert(await echoed===5,'Blob worker execution');URL.revokeObjectURL(source);await browser.terminate();
        const finished=new Worker(`require('node:worker_threads').parentPort.postMessage('complete')`,{eval:true});
        assert(await new Promise((resolve,reject)=>{finished.on('exit',resolve);finished.on('error',reject)})===0,'Native worker exits when idle');await finished.terminate();
        const loop=new Worker('while(true){}',{eval:true});loop.on('error',()=>{});await new Promise(resolve=>setTimeout(resolve,30));phase='terminating-loop';await loop.terminate();phase='done';
    "#);
}

#[test]
fn sqlite_online_backup() {
    run(r#"
        const assert=(v,m)=>{if(!v)throw Error(m)},sqlite=__apps.sqlite,fs=__apps.fs;
        const folder=fs.mkdtempSync(__apps.os.tmpdir()+'/apps-backup-'),path=folder+'/copy.sqlite',db=new sqlite.DatabaseSync(':memory:');
        try{
            db.exec('CREATE TABLE records(value TEXT);INSERT INTO records VALUES (\'saved\')');let progress=0;
            await sqlite.backup(db,path,{rate:1,progress:()=>progress++});
            const copy=new sqlite.DatabaseSync(path);assert(copy.prepare('SELECT value FROM records').get().value==='saved','Backup contains committed data');copy.close();assert(progress>0,'Backup progress');
        }finally{db.close();fs.rmSync(folder,{recursive:true,force:true})}
    "#);
}

#[test]
fn tcp_half_close_and_peer_addresses() {
    run(r#"
        const assert=(v,m)=>{if(!v)throw Error(m)},net=__apps.net;
        assert(net.isIP('::1')===6&&net.isIP('127.0.0.1')===4&&net.isIP('invalid')===0,'IP parsing');
        const server=net.createServer({allowHalfOpen:true},socket=>{
            let data='';socket.setEncoding('utf8');socket.on('data',chunk=>data+=chunk);
            socket.on('end',()=>socket.end(data.toUpperCase()));
        });
        await new Promise((resolve,reject)=>{server.on('error',reject);server.listen(0,'127.0.0.1',resolve)});
        const client=net.connect({host:'127.0.0.1',port:server.address().port});
        let value='';client.setEncoding('utf8');client.on('data',chunk=>value+=chunk);
        const closed=new Promise((resolve,reject)=>{client.on('error',reject);client.on('close',resolve)});
        await new Promise(resolve=>client.on('connect',resolve));
        assert(client.remoteAddress==='127.0.0.1'&&client.remotePort===server.address().port,'TCP peer address');
        client.setNoDelay();client.end('hello');await closed;assert(value==='HELLO','TCP half-close roundtrip');
        await new Promise(resolve=>server.close(resolve));
        const path=__apps.os.tmpdir()+'/apps-tcp-'+Date.now()+'.sock',ipc=net.createServer(socket=>socket.pipe(socket));
        await new Promise((resolve,reject)=>{ipc.on('error',reject);ipc.listen(path,resolve)});
        const local=net.connect(path),payload=new Uint8Array(200000).map((_,i)=>i*13);let echoed=[];local.on('data',chunk=>echoed.push(chunk));
        await new Promise((resolve,reject)=>{local.on('error',reject);local.on('connect',resolve)});local.setNoDelay(true);local.setKeepAlive(true,1000);local.write(payload);
        for(let i=0;i<500&&echoed.reduce((n,c)=>n+c.length,0)<payload.length;i++)await new Promise(resolve=>setTimeout(resolve,2));
        const joined=__apps.buffer.Buffer.concat(echoed);assert(joined.length===payload.length&&joined.every((v,i)=>v===payload[i]),'Unix socket options and binary echo');
        local.destroy();await new Promise(resolve=>ipc.close(resolve));assert(!__apps.fs.existsSync(path),'Closing removes the socket file');
    "#);
}

#[test]
fn durable_browser_storage_crypto_keys_and_transactions() {
    if std::env::var_os("APPS_STORAGE_TEST_CHILD").is_none() {
        let root = std::env::temp_dir().join(format!(
            "apps-browser-storage-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "durable_browser_storage_crypto_keys_and_transactions",
                "--nocapture",
            ])
            .env("APPS_STORAGE_TEST_CHILD", "1")
            .env("APPS_DATA_DIR", &root)
            .output()
            .unwrap();
        let _ = std::fs::remove_dir_all(root);
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    run(r#"
        const assert=(v,m)=>{if(!v)throw Error(m)};
        const req=request=>new Promise((resolve,reject)=>{request.onsuccess=()=>resolve(request.result);request.onerror=()=>reject(request.error)});
        const done=tx=>new Promise((resolve,reject)=>{tx.oncomplete=resolve;tx.onabort=()=>reject(tx.error)});
        localStorage.setItem('language','en');localStorage.property='value';
        assert(localStorage.length===2&&Object.keys(localStorage).includes('property'),'Storage properties');
        sessionStorage.setItem('tab','saved');
        const opening=indexedDB.open('t3',1);opening.onupgradeneeded=()=>opening.result.createObjectStore('keys');
        const db=await req(opening), pair=await crypto.subtle.generateKey({name:'ECDSA',namedCurve:'P-256'},false,['sign','verify']);
        let tx=db.transaction('keys','readwrite'), completed=done(tx);tx.objectStore('keys').put(pair,'pair');tx.objectStore('keys').put({n:1},'env:a');tx.objectStore('keys').put({n:2},'env:b');await completed;
        tx=db.transaction('keys','readwrite');completed=new Promise(resolve=>tx.onabort=resolve);tx.objectStore('keys').put('should rollback','abort').onsuccess=()=>tx.abort();await completed;
        tx=db.transaction('keys','readwrite');completed=done(tx);const cursor=tx.objectStore('keys').openCursor(IDBKeyRange.bound('env:','env:\uffff'));cursor.onsuccess=()=>{if(cursor.result){cursor.result.delete();cursor.result.continue()}};await completed;
        tx=db.transaction('keys');assert(await req(tx.objectStore('keys').count())===1,'Cursor deletes and transaction rollback');
        const ordered=indexedDB.open('order',1);ordered.onupgradeneeded=()=>ordered.result.createObjectStore('items');const odb=await req(ordered);
        tx=odb.transaction('items','readwrite');completed=done(tx);
        for(const key of [[1],new Uint8Array([2]),'b','a','',new Date(5),10,-Infinity,2,Infinity,-0.5,'é','😀',[1,'x'],[]])tx.objectStore('items').put(key instanceof Uint8Array?'buf':String(key),key);await completed;
        const label=k=>k instanceof ArrayBuffer?'buf'+new Uint8Array(k):k instanceof Date?'date'+(+k):Array.isArray(k)?'arr'+JSON.stringify(k):String(k);
        const walk=(source,step)=>new Promise((resolve,reject)=>{const out=[];source.onsuccess=()=>{const cursor=source.result;if(!cursor)return resolve(out);out.push(label(cursor.key));const next=step(cursor,out);if(next===false)resolve(out);else cursor.continue(next);};source.onerror=()=>reject(source.error);});
        tx=odb.transaction('items');const items=tx.objectStore('items');
        assert((await req(items.getAllKeys())).map(label).join()==='-Infinity,-0.5,2,10,Infinity,date5,,a,b,é,😀,buf2,arr[],arr[1],arr[1,"x"]','IndexedDB key order');
        assert(await req(items.count())===15&&await req(items.count(IDBKeyRange.bound(2,10)))===2&&await req(items.count(-0.5))===1,'Counts in SQL');
        assert((await req(items.getAll(IDBKeyRange.lowerBound('a',true),2))).join()==='b,é'&&(await req(items.getAllKeys(null,3))).map(label).join()==='-Infinity,-0.5,2','Ranges and limits');
        assert(label(await req(items.getKey(IDBKeyRange.lowerBound(new Date(0)))))==='date5'&&await req(items.get(IDBKeyRange.bound('a','b',true,false)))==='b'&&await req(items.get('zzz'))===undefined,'First key in a range');
        assert((await walk(items.openKeyCursor(IDBKeyRange.bound('','￿'),'prev'),()=>{})).join()==='😀,é,b,a,','Reverse key cursor');
        assert((await walk(items.openCursor(IDBKeyRange.lowerBound(0)),(cursor,seen)=>seen.length===1?Infinity:seen.length<3?undefined:false)).join()==='2,Infinity,date5','Cursor continue to a key');
        tx=odb.transaction('items','readwrite');completed=done(tx);tx.objectStore('items').delete(IDBKeyRange.bound('a','é'));const left=req(tx.objectStore('items').count());await completed;
        assert(await left===12,'Range delete');
        const later=indexedDB.open('order',2);await new Promise(resolve=>later.onblocked=resolve);
        assert(__appsDelay()===-1,'A blocked upgrade waits without polling');odb.close();assert((await req(later)).version===2,'Upgrade after close');later.result.close();
        db.close();
        __appsSetLocation('https://app.test/start');history.pushState({n:1},'', '/next?q=1');assert(location.pathname==='/next'&&history.state.n===1,'History push');
        const popped=new Promise(resolve=>addEventListener('popstate',resolve,{once:true}));history.back();await popped;assert(location.pathname==='/start','History back');
        const changed=new Promise(resolve=>addEventListener('hashchange',resolve,{once:true}));location.hash='chat';await changed;assert(location.hash==='#chat','Hash change');
        assert(globalThis.__appsStorageClone===undefined&&globalThis.__appsCryptoCodec===undefined&&globalThis.__appsBytes===undefined&&globalThis.__appsRetries===undefined,'Private key codecs hidden');
    "#);
    run(r#"
        const assert=(v,m)=>{if(!v)throw Error(m)};
        const req=request=>new Promise((resolve,reject)=>{request.onsuccess=()=>resolve(request.result);request.onerror=()=>reject(request.error)});
        assert(localStorage.getItem('language')==='en'&&sessionStorage.getItem('tab')==='saved','Storage survives context recreation');
        const db=await req(indexedDB.open('t3',1)), pair=await req(db.transaction('keys').objectStore('keys').get('pair'));
        assert(pair.privateKey instanceof CryptoKey&&!pair.privateKey.extractable,'Nonextractable CryptoKey persistence');
        const bytes=new TextEncoder().encode('persisted proof'), algorithm={name:'ECDSA',hash:'SHA-256'},signature=await crypto.subtle.sign(algorithm,pair.privateKey,bytes);
        assert(await crypto.subtle.verify(algorithm,pair.publicKey,signature,bytes),'Persisted key signs');
        let denied=false;try{await crypto.subtle.exportKey('jwk',pair.privateKey)}catch{denied=true}assert(denied,'Persisted private key still nonextractable');db.close();
    "#);
}
