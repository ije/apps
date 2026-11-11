use apps_runtime::{
    host::{Host, Role},
    jsc::{JavaScriptCore, JsContext},
};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

fn run(source: &str) {
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        Rc::new(RefCell::new(Host::default())),
        Role::Preload(1),
    )
    .unwrap();
    engine.eval(&format!("globalThis.testResult = 'pending'; (async () => {{ {source} }})().then(() => testResult = 'ok', error => testResult = error.stack || String(error));"), "child-test").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        engine.tick().unwrap();
        let result = engine.eval("testResult", "child-result").unwrap();
        if result != "pending" {
            assert_eq!(result, "ok");
            break;
        }
        assert!(Instant::now() < deadline, "Child process test timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn events_once_resolves_rejects_and_cleans_up() {
    run(r#"
        const { EventEmitter, once } = __apps.events;
        const assert = (condition, message) => { if (!condition) throw Error(message); };
        const emitter = new EventEmitter();
        let calls = 0;
        function listener(value) { assert(this === emitter, 'Listener this'); calls += value; }
        emitter.on('data', listener).on('data', listener).off('data', listener);
        assert(emitter.emit('data', 2) && calls === 2, 'Remove one duplicate');
        emitter.once('single', listener).off('single', listener);
        assert(!emitter.emit('single', 5), 'Remove once by original listener');
        emitter.once('recursive', () => { calls++; emitter.emit('recursive'); });
        emitter.emit('recursive');
        assert(calls === 3, 'Once reentrancy');
        const pending = once(emitter, 'close');
        emitter.emit('close', 7, null);
        const args = await pending;
        assert(args[0] === 7 && args[1] === null, 'Once arguments');
        assert(emitter.listenerCount('close') === 0 && emitter.listenerCount('error') === 0, 'Once cleanup');
        const failure = once(emitter, 'close');
        const error = Error('failure');
        emitter.emit('error', error);
        try { await failure; throw Error('Missing rejection'); } catch (caught) { assert(caught === error, 'Error identity'); }
        assert(emitter.listenerCount('close') === 0, 'Failure cleanup');
        const errorEvent = once(emitter, 'error');
        emitter.emit('error', error);
        assert((await errorEvent)[0] === error, 'Waiting for error');
        const controller = new AbortController();
        const aborted = once(emitter, 'close', { signal: controller.signal });
        controller.abort('cancelled');
        try { await aborted; throw Error('Missing abort'); } catch (caught) { assert(caught.name === 'AbortError', 'Abort error'); }
        assert(emitter.listenerCount('close') === 0 && emitter.listenerCount('error') === 0, 'Abort cleanup');
        const target = new EventTarget();
        const event = new Event('ready');
        const ready = once(target, 'ready');
        target.dispatchEvent(event);
        assert((await ready)[0] === event, 'EventTarget support');
    "#);
}

#[cfg(unix)]
#[test]
fn child_process_output_stdin_errors_and_signals() {
    run(r#"
        const { spawn } = __apps.child_process;
        const { once } = __apps.events;
        const assert = (condition, message) => { if (!condition) throw Error(message); };
        const child = spawn('/bin/sh', ['-c', 'printf "output\\n"; printf "error\\n" >&2; exit 7']);
        let out = '', err = '';
        const order = [];
        child.on('spawn', () => order.push('spawn'));
        child.stdout.on('data', data => { assert(data instanceof Uint8Array, 'Byte chunk'); out += `${data}`; });
        child.stderr.on('data', data => { err += data.toString(); });
        child.stdout.on('end', () => order.push('stdout:end'));
        child.stderr.on('end', () => order.push('stderr:end'));
        child.on('exit', (code, signal) => { assert(code === 7 && signal === null, 'Exit arguments'); order.push('exit'); });
        child.on('close', () => order.push('close'));
        const [code, signal] = await once(child, 'close');
        assert(code === 7 && signal === null && child.exitCode === 7, 'Close arguments');
        assert(out === 'output\n' && err === 'error\n', 'Decoded output');
        assert(order[0] === 'spawn' && order.at(-1) === 'close' && order.includes('stdout:end') && order.includes('stderr:end'), 'Close follows stream ends');
        assert(!child.kill(), 'Cannot kill closed child');
        const cat = spawn('/bin/cat');
        let echoed = '';
        cat.stdout.on('data', data => { echoed += data; });
        cat.stdin.write('hello 🌊');
        cat.stdin.end(new Uint8Array([99, 33, 99]).subarray(1, 2));
        assert((await once(cat, 'close'))[0] === 0 && echoed === 'hello 🌊!', 'Async stdin');
        const all = Buffer.from(Array.from({ length: 256 }, (_, i) => i)), binary = spawn('/bin/cat'), chunks = [];
        binary.stdout.on('data', data => chunks.push(data));
        binary.stdin.end(all);
        await once(binary, 'close');
        assert(Buffer.concat(chunks).equals(all) && __apps.child_process.spawnSync('/bin/cat', [], { input: all }).stdout.equals(all), 'Binary stdio');
        const late = spawn('/bin/sh', ['-c', '(sleep 0.02; printf x) & exit 0']), lateOrder = [];
        late.stdout.on('data', () => {});
        late.stdout.on('end', () => lateOrder.push('end'));
        late.on('close', () => lateOrder.push('close'));
        await once(late, 'close');
        assert(lateOrder.join() === 'end,close', 'Close follows a late stream end');
        const missing = spawn('/apps-does-not-exist');
        const failureOrder = [];
        missing.on('error', () => failureOrder.push('error'));
        missing.on('close', () => failureOrder.push('close'));
        try { await once(missing, 'close'); throw Error('Missing spawn rejection'); }
        catch (error) { assert(error.code === 'ENOENT' && error.path === '/apps-does-not-exist', 'Spawn error details'); }
        assert(failureOrder.join(',') === 'error,close', 'Failure event order');
        const sleeper = spawn('/bin/sleep', ['30']);
        await once(sleeper, 'spawn');
        assert(sleeper.kill() && sleeper.killed, 'Kill');
        const killed = await once(sleeper, 'close');
        assert(killed[0] === null && killed[1] === 'SIGTERM', 'Signal exit');
        const environment = spawn('/bin/sh', ['-c', 'printf "%s:%s" "$APPS_CHILD_TEST" "$PWD"'], { env: { APPS_CHILD_TEST: 'value' }, cwd: '/usr', stdio: ['ignore', 'pipe', 'ignore'] });
        let value = '';
        environment.stdout.on('data', data => { value += data; });
        assert(environment.stdin === null && environment.stderr === null, 'Stdio options');
        await once(environment, 'close');
        assert(value === 'value:/usr', 'Environment and cwd');
    "#);
}

#[cfg(unix)]
#[test]
fn file_flags_and_copies_never_lose_data() {
    run(r#"
        const { fs, path, os } = __apps;
        const assert = (condition, message) => { if (!condition) throw Error(message); };
        const code = action => { try { action(); } catch (error) { return error.code; } };
        const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'apps-flags-')), file = path.join(dir, 'data'), text = () => fs.readFileSync(file, 'utf8');
        fs.writeFileSync(file, 'hello world');
        fs.writeFileSync(file, 'HE', { flag: 'r+' });
        assert(text() === 'HEllo world', 'r+ overwrites without truncating: ' + text());
        assert(code(() => fs.writeFileSync(file, 'x', { flag: 'r' })) === 'EBADF' && text() === 'HEllo world', 'r never writes');
        assert(code(() => fs.writeFileSync(path.join(dir, 'missing'), 'x', { flag: 'r+' })) === 'ENOENT', 'r+ needs a file');
        fs.appendFileSync(file, '!', { flag: 'as' });
        const fd = fs.openSync(file, 'rs+');
        fs.writeSync(fd, 'h', 0);
        fs.closeSync(fd);
        assert(text() === 'hEllo world!', 'as and rs+: ' + text());
        assert(code(() => fs.openSync(file, 'rw')) === 'ERR_INVALID_ARG_VALUE', 'invalid flags');
        fs.copyFileSync(file, file);
        fs.linkSync(file, path.join(dir, 'link'));
        fs.copyFileSync(path.join(dir, 'link'), file);
        await fs.promises.copyFile(file, file);
        assert(text() === 'hEllo world!', 'copying a file onto itself emptied it');
        assert(code(() => fs.copyFileSync(file, file, fs.constants.COPYFILE_EXCL)) === 'EEXIST', 'COPYFILE_EXCL');
        assert(code(() => fs.cpSync(file, file)) === 'ERR_FS_CP_EINVAL', 'cp onto itself');
        fs.mkdirSync(path.join(dir, 'tree'));
        fs.writeFileSync(path.join(dir, 'tree', 'leaf'), 'leaf');
        let failure;
        await fs.promises.cp(path.join(dir, 'tree'), path.join(dir, 'tree', 'inner'), { recursive: true }).catch(error => { failure = error; });
        assert(failure?.code === 'ERR_FS_CP_EINVAL' && !fs.existsSync(path.join(dir, 'tree', 'inner')), 'cp into itself');
        assert(text() === 'hEllo world!', 'cp changed the source');
        fs.rmSync(dir, { recursive: true });
    "#);
}

#[test]
fn gcm_tags_must_be_complete() {
    run(r#"
        const { crypto } = __apps;
        const assert = (condition, message) => { if (!condition) throw Error(message); };
        const code = action => { try { action(); } catch (error) { return error.code; } };
        const key = crypto.randomBytes(32), iv = crypto.randomBytes(12), cipher = crypto.createCipheriv('aes-256-gcm', key, iv);
        const sealed = Buffer.concat([cipher.update('attack at dawn'), cipher.final()]), tag = cipher.getAuthTag();
        const decipher = () => crypto.createDecipheriv('aes-256-gcm', key, iv);
        const copy = Buffer.from(tag), opened = decipher().setAuthTag(copy);
        copy.fill(0);
        assert(Buffer.concat([opened.update(sealed), opened.final()]).toString() === 'attack at dawn', 'GCM round trip with a copied tag');
        assert(code(() => decipher().setAuthTag(tag.subarray(0, 12))) === 'ERR_CRYPTO_INVALID_AUTH_TAG', 'short tag');
        const truncated = decipher();
        truncated.update(sealed.subarray(0, -4));
        assert(code(() => truncated.setAuthTag(Buffer.concat([sealed.subarray(-4), tag])).final()) === 'ERR_CRYPTO_INVALID_AUTH_TAG', 'a long tag authenticated a truncated message');
        assert(code(() => crypto.createDecipheriv('aes-256-gcm', key, iv, { authTagLength: 8 })) === 'ERR_CRYPTO_INVALID_AUTH_TAG', 'short authTagLength');
        const short = crypto.createCipheriv('aes-256-gcm', key, iv, { authTagLength: 12 });
        short.update('x'); short.final();
        assert(short.getAuthTag().length === 12, 'cipher authTagLength');
    "#);
}

#[test]
fn piped_sources_wait_for_every_destination() {
    run(r#"
        const { Readable, Writable } = __apps.stream;
        const assert = (condition, message) => { if (!condition) throw Error(message); };
        let produced = 0, release = null;
        const writes = { fast: 0, slow: 0 };
        const source = new Readable({ highWaterMark: 1, read() { this.push(produced < 20 ? Buffer.from([produced++]) : null); } });
        const fast = new Writable({ highWaterMark: 1, write(chunk, encoding, callback) { writes.fast++; setTimeout(callback, 1); } });
        const slow = new Writable({ highWaterMark: 1, write(chunk, encoding, callback) { writes.slow++; release = callback; } });
        source.pipe(fast);
        source.pipe(slow);
        await new Promise(resolve => setTimeout(resolve, 50));
        assert(produced <= 4 && writes.fast <= 3, `A fast destination resumed the source while a slow one waited: ${produced} ${writes.fast}`);
        for (let i = 0; i < 200 && writes.slow < 20; i++) { const next = release; release = null; next?.(); await new Promise(resolve => setTimeout(resolve, 2)); }
        assert(writes.slow === 20 && writes.fast === 20, `Every chunk reached both destinations: ${writes.slow} ${writes.fast}`);
    "#);
}

#[cfg(unix)]
#[test]
fn child_process_streams_large_output_without_blocking_timers() {
    run(r#"
        const { spawn } = __apps.child_process;
        const child = spawn('/bin/sh', ['-c', 'head -c 262144 /dev/zero; head -c 131072 /dev/zero >&2; sleep 0.05']);
        let out = 0, err = 0, ticks = 0;
        const timer = setInterval(() => ticks++, 1);
        child.stdout.on('data', data => out += data.length);
        child.stderr.on('data', data => err += data.length);
        const [code] = await __apps.events.once(child, 'close');
        clearInterval(timer);
        if (code !== 0 || out !== 262144 || err !== 131072 || ticks === 0) throw Error('Output lost or runtime blocked');
    "#);
}

#[cfg(unix)]
#[test]
fn child_process_context_cleanup_reaps_running_children() {
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        Rc::new(RefCell::new(Host::default())),
        Role::Main,
    )
    .unwrap();
    let pid: libc::pid_t = engine
        .eval(
            "__apps.child_process.spawn('/bin/sleep', ['30'], { stdio: 'ignore' }).pid",
            "child-cleanup",
        )
        .unwrap()
        .parse()
        .unwrap();
    drop(engine);
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}

#[cfg(unix)]
#[test]
fn files_transfer_bytes_and_text() {
    run(r#"
        const assert = (condition, message) => { if (!condition) throw Error(message); };
        const fs = __apps.fs, all = Buffer.from(Array.from({ length: 256 }, (_, i) => i));
        const dir = fs.mkdtempSync('/tmp/apps-files-'), file = dir + '/data';
        fs.writeFileSync(file, all);
        assert(fs.readFileSync(file).equals(all) && fs.readFileSync(file, 'hex') === all.toString('hex'), 'Binary file');
        await fs.promises.writeFile(file, new DataView(all.buffer, 1, 3));
        assert((await fs.promises.readFile(file)).join() === '1,2,3', 'DataView file');
        fs.writeFileSync(file, '﻿hé');
        assert(fs.readFileSync(file, 'utf8') === '﻿hé' && fs.readFileSync(file).toString() === '﻿hé', 'UTF-8 keeps BOM');
        const fd = fs.openSync(file, 'w+'), out = Buffer.alloc(9);
        assert(fs.writeSync(fd, 'héllo') === 6 && fs.writeSync(fd, all, 253, 3, 6) === 3, 'Descriptor writes');
        assert(fs.readSync(fd, out, 0, 9, 0) === 9 && out.subarray(0, 6).toString() === 'héllo' && out.subarray(6).join() === '253,254,255', 'Descriptor reads');
        fs.closeSync(fd);
        assert((await Promise.all(Array.from({ length: 300 }, () => fs.promises.stat(file)))).length === 300, 'Many async operations');
        try { await fs.promises.rmdir(dir); throw Error('Removed a full directory'); } catch (error) { assert(error.code === 'ENOTEMPTY', 'Error code ' + error.code); }
        fs.rmSync(dir, { recursive: true });
        const terminal = __apps.pty.spawn('/bin/sh', ['-c', 'printf "🌊é🌊é"; printf "\\377"']);
        let text = ''; terminal.onData(data => text += data);
        await new Promise(resolve => terminal.onExit(resolve));
        for (let i = 0; i < 200 && !text.endsWith('�'); i++) await new Promise(resolve => setTimeout(resolve, 5));
        assert(text.includes('🌊é🌊é') && text.endsWith('�'), 'Terminal text');
    "#);
}

#[cfg(unix)]
#[test]
fn native_events_wake_the_event_loop() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let wakes = Arc::new(AtomicUsize::new(0));
    let counter = wakes.clone();
    apps_runtime::host::Waker::install(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        Rc::new(RefCell::new(Host::default())),
        Role::Main,
    )
    .unwrap();
    for start in [
        "__apps.child_process.spawn('/bin/sh',['-c','sleep 0.05'],{stdio:'ignore'}).on('exit',()=>done=true)",
        "globalThis.out='';__apps.child_process.spawn('/bin/sh',['-c','sleep 0.05; printf hi']).stdout.on('data',data=>done=(out+=data)==='hi')",
        "__apps.fs.promises.stat('/').then(()=>done=true)",
        "__apps.zlib.gzip('wake',()=>done=true)",
        "globalThis.out='';__apps.pty.spawn('/bin/sh',['-c','sleep 0.05; printf hi']).onData(data=>done=(out+=data).includes('hi'))",
        "__apps.pty.spawn('/bin/sh',['-c','sleep 0.05']).onExit(()=>done=true)",
    ] {
        engine
            .eval(&format!("globalThis.done=false;{start}"), "wake")
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let seen = wakes.load(Ordering::SeqCst);
            engine.tick().unwrap();
            if engine.eval("done", "wake").unwrap() == "true" {
                break;
            }
            while wakes.load(Ordering::SeqCst) == seen {
                assert!(Instant::now() < deadline, "No wake for: {start}");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn closing_terminals_does_not_block_on_kill() {
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        Rc::new(RefCell::new(Host::default())),
        Role::Main,
    )
    .unwrap();
    let pid: libc::pid_t = engine
        .eval(
            "__apps.pty.spawn('/bin/sh',['-c','trap \"\" HUP; exec sleep 30']).pid",
            "pty-close",
        )
        .unwrap()
        .parse()
        .unwrap();
    std::thread::sleep(Duration::from_millis(50));
    let start = Instant::now();
    drop(engine);
    assert!(
        start.elapsed() < Duration::from_millis(150),
        "Closing a terminal blocked for {:?}",
        start.elapsed()
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while unsafe { libc::kill(pid, 0) } == 0 {
        assert!(Instant::now() < deadline, "Terminal child was not killed");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
#[test]
fn child_output_applies_backpressure() {
    run(r#"
        const { spawn, spawnSync } = __apps.child_process;
        const child = spawn('/bin/sh', ['-c', 'head -c 4194304 /dev/zero']);
        await new Promise(resolve => setTimeout(resolve, 200));
        const buffered = child.stdout.readableLength;
        let total = 0;
        child.stdout.on('data', data => total += data.length);
        await __apps.events.once(child, 'close');
        if (buffered > 524288 || total !== 4194304) throw Error(`Buffered ${buffered} bytes, received ${total}`);
        const result = spawnSync('/bin/sh', ['-c', 'head -c 2097152 /dev/zero'], { maxBuffer: Infinity });
        if (result.error || result.stdout.length !== 2097152) throw Error('maxBuffer: Infinity');
    "#);
}

#[cfg(unix)]
#[test]
fn destroyed_child_output_closes_the_pipe() {
    run(r#"
        const { spawn } = __apps.child_process;
        const child = spawn('yes'), closed = __apps.events.once(child, 'close');
        for await (const chunk of child.stdout) break;
        const [code, signal] = await closed;
        if (code !== null || signal !== 'SIGPIPE' || !child.stdout.destroyed) throw Error(`Closed with ${code} ${signal}`);
        const paused = spawn('/bin/sh', ['-c', 'exec head -c 1048576 /dev/zero']);
        await new Promise(resolve => setTimeout(resolve, 100));
        paused.stdout.destroy();
        const [status, killed] = await __apps.events.once(paused, 'close');
        if (status !== null || killed !== 'SIGPIPE') throw Error(`Paused output closed with ${status} ${killed}`);
    "#);
}

#[test]
fn synchronous_children_honor_stdio_modes() {
    run(
        r#"for(const stdio of ['ignore','inherit',['ignore','ignore','ignore']]){const result=__apps.child_process.spawnSync('/bin/sh',['-c','exit 0'],{stdio});if(result.status!==0||result.stdout!==null||result.stderr!==null)throw Error('sync stdio')}"#,
    );
}

#[cfg(unix)]
#[test]
fn extra_pipes_and_failed_spawns_keep_node_behavior() {
    run(r#"
        const { spawn, spawnSync } = __apps.child_process;
        const result = spawnSync('/bin/sh', ['-c', 'echo out; echo err >&2; echo extra >&3'], { stdio: ['pipe', 'pipe', 'pipe', 'pipe'] });
        if (String(result.stdout) !== 'out\n' || String(result.stderr) !== 'err\n') throw Error(`Extra pipe output leaked: ${result.stdout} ${result.stderr}`);
        const missing = spawn('/apps-missing-command', [], { detached: true });
        const failed = __apps.events.once(missing, 'error');
        if (missing.unref() !== missing || missing.ref() !== missing) throw Error('ref and unref return the child');
        const [error] = await failed;
        if (error.code !== 'ENOENT') throw Error(`Spawn failed with ${error.code}`);
    "#);
}

#[cfg(unix)]
#[test]
fn child_process_options_stdio_and_sync_errors() {
    run(r#"
        const { spawn, spawnSync, execFile } = __apps.child_process, { Writable } = __apps.stream, { once } = __apps.events;
        const assert = (condition, message) => { if (!condition) throw Error(message); };
        const missing = spawnSync('apps-missing-command', ['a']);
        assert(missing.status === null && missing.pid === 0 && missing.output === null && missing.error.code === 'ENOENT' && missing.error.syscall === 'spawnSync apps-missing-command' && missing.error.message === 'spawnSync apps-missing-command ENOENT' && missing.error.path === 'apps-missing-command', 'spawnSync ENOENT');
        const slow = spawnSync('/bin/sleep', ['5'], { timeout: 30 });
        assert(slow.status === null && slow.signal === 'SIGTERM' && slow.error.code === 'ETIMEDOUT' && slow.error.errno < 0, 'spawnSync timeout uses SIGTERM');
        assert(spawnSync('/bin/sleep', ['5'], { timeout: 30, killSignal: 'SIGKILL' }).signal === 'SIGKILL', 'spawnSync killSignal');
        assert(spawnSync('/bin/sh', ['-c', 'printf "$0"'], { argv0: 'custom0', encoding: 'utf8', maxBuffer: 100, buffer: true, foo: 1 }).stdout === 'custom0', 'argv0 and unknown options');
        for (const [stdio, kinds] of [[[0, 'pipe', 2], 'null,R,null'], [['ignore', 'pipe', process.stderr], 'null,R,null'], ['overlapped', 'W,R,R'], [[null, undefined, 'pipe'], 'W,R,R'], [['pipe', process.stdout, 'ignore'], 'W,null,null']]) {
            const child = spawn('/bin/sh', ['-c', 'true'], { stdio, maxBuffer: 1, buffer: true });
            assert(child.stdio.map(item => item === null ? 'null' : item instanceof Writable ? 'W' : 'R').join() === kinds, 'stdio ' + JSON.stringify(stdio));
            await once(child, 'close');
        }
        try { spawn('/bin/sh', [], { stdio: ['pipe', 'pipe', 'pipe', 'ipc'] }); assert(false, 'IPC accepted'); } catch (error) { assert(error.code === 'ERR_INVALID_ARG_VALUE', 'IPC error'); }
        const cat = spawn('/bin/cat'), order = [];
        let echoed = '';
        cat.stdout.on('data', data => { echoed += data; });
        cat.stdout.on('close', () => order.push('stdout-close'));
        cat.stdin.on('finish', () => order.push('finish'));
        cat.stdin.on('close', () => order.push('stdin-close'));
        assert(cat.stdin instanceof Writable, 'stdin is Writable');
        cat.stdin.write('a', () => order.push('write'));
        cat.stdin.end('b', () => order.push('end'));
        await once(cat, 'close');
        assert(echoed === 'ab' && order[0] === 'write' && order.includes('end') && order.includes('finish') && order.filter(item => item === 'stdout-close').length === 1 && order.at(-1) === 'stdout-close', 'stdin callbacks and one stdout close: ' + order);
        const failed = spawn('apps-missing-command', ['x']);
        const error = await once(failed, 'close').then(() => null, error => error);
        assert(error.message === 'spawn apps-missing-command ENOENT' && error.syscall === 'spawn apps-missing-command', 'spawn error message');
        const sleeper = spawn('/bin/sleep', ['5']);
        sleeper.kill(9);
        assert((await once(sleeper, 'exit'))[1] === 'SIGKILL', 'numeric signal');
        const [limit, output] = await new Promise(resolve => execFile('/bin/sh', ['-c', 'printf 12345678'], { maxBuffer: 2 }, (error, stdout) => resolve([error, stdout])));
        assert(limit.code === 'ERR_CHILD_PROCESS_STDIO_MAXBUFFER' && limit.message === 'stdout maxBuffer length exceeded' && output === '12', 'maxBuffer truncates');
    "#);
}
