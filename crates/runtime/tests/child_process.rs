use apps_runtime::{
    host::{Host, Role},
    jsc::{Api, Engine},
};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

fn run(source: &str) {
    let engine = Engine::new(
        Api::load().unwrap(),
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
    let engine = Engine::new(
        Api::load().unwrap(),
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
        assert(text.includes('🌊é🌊é') && text.endsWith('�'), 'Terminal text');
    "#);
}

#[test]
fn synchronous_children_honor_stdio_modes() {
    run(
        r#"for(const stdio of ['ignore','inherit',['ignore','ignore','ignore']]){const result=__apps.child_process.spawnSync('/bin/sh',['-c','exit 0'],{stdio});if(result.status!==0||result.stdout!==null||result.stderr!==null)throw Error('sync stdio')}"#,
    );
}
