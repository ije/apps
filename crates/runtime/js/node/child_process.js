globalThis.__appsChildProcess = (call, EventEmitter, stream) => {
  const children = new Map();
  const key = {};
  let failureID = 0;
  const view = data => Buffer.from(data.buffer, data.byteOffset, data.byteLength);
  const channel = (target, id, name) => {
    let parts = [];
    const disconnect = now => {
      if (!target.connected) return;
      target.connected = false; target.channel = null;
      try { call('child.end', id, name); call('child.destroy', id, name); } catch {}
      if (now) target.emit('disconnect'); else process.nextTick(() => target.emit('disconnect'));
    };
    const ipc = target.channel = {
      sent: [], destroyed: false, unreferenced: false,
      ref() { ipc.unreferenced = false; }, unref() { ipc.unreferenced = true; },
      push(data) {
        if (data === null) { ipc.destroyed = true; disconnect(true); return true; }
        let start = 0;
        for (let end; (end = data.indexOf(10, start)) !== -1; start = end + 1) {
          parts.push(data.subarray(start, end));
          const message = JSON.parse(Buffer.concat(parts).toString());
          parts = [];
          if (!target.connected || typeof message?.cmd === 'string' && message.cmd.startsWith('NODE_')) continue;
          target.emit('message', message);
          target.parentPort?.emit('message', { data: message, ports: [] });
        }
        if (start < data.length) parts.push(data.subarray(start));
        return true;
      },
      _childWritten() { ipc.sent.shift()?.(null); },
      _childEnded() {},
      destroy(error) { ipc.destroyed = true; for (const callback of ipc.sent.splice(0)) callback ? callback(error) : target.emit('error', error); disconnect(true); },
    };
    target.connected = true;
    target.send = (message, handle, options, callback) => {
      if (typeof handle === 'function') [handle, callback] = [undefined, handle]; else if (typeof options === 'function') callback = options;
      if (message === undefined) throw Object.assign(new TypeError('The "message" argument must be specified'), { code: 'ERR_MISSING_ARGS' });
      if (handle != null) throw Object.assign(new TypeError('Sending handles is not supported'), { code: 'ERR_INVALID_HANDLE_TYPE' });
      const data = Buffer.from(JSON.stringify(message) + '\n');
      if (target.connected) try { call('child.write', id, data, name); ipc.sent.push(callback); return true; } catch {}
      const error = Object.assign(new Error('Channel closed'), { code: 'ERR_IPC_CHANNEL_CLOSED' });
      process.nextTick(() => callback ? callback(error) : target.emit('error', error));
      return false;
    };
    target.disconnect = () => { if (target.connected) disconnect(false); else process.nextTick(() => target.emit('error', Object.assign(new Error('IPC channel is already disconnected'), { code: 'ERR_IPC_DISCONNECTED' }))); };
    return ipc;
  };
  const failed = (failure, syscall, file, args) => Object.assign(new Error(`${syscall} ${failure.code}`), { errno: failure.errno, code: failure.code, syscall, path: file, spawnargs: args });
  const input = id => {
    let done = null, finished = null;
    const writable = new stream.Writable({
      write(data, _encoding, callback) { try { call('child.write', id, data); done = callback; } catch (error) { callback(error); } },
      final(callback) { try { call('child.end', id); finished = callback; } catch (error) { callback(error); } },
      destroy(error, callback) { if (!finished) try { call('child.end', id); } catch {} callback(error); },
    });
    return Object.assign(writable, { _childWritten() { const callback = done; done = null; callback?.(); }, _childEnded() { const callback = finished; finished = null; if (callback) callback(); else writable.destroy(); } });
  };
  class ChildProcess extends EventEmitter {
    #id;
    constructor(token, command, args, options, result) {
      super();
      if (token !== key) throw new TypeError('Use spawn() to create a child process');
      this.#id = result.id;
      this.pid = result.pid;
      this.spawnfile = command;
      this.spawnargs = [command, ...args];
      this.exitCode = null;
      this.signalCode = null;
      this.killed = false;
      this.stdin = options.stdio[0] === 'pipe' ? input(result.id) : null;
      const output = name => options.stdio[name === 'stdout' ? 1 : 2] === 'pipe' ? new stream.Readable({ read() { if (this._paused) { this._paused = false; call('child.pause', result.id, name, false); } }, destroy(error, callback) { call('child.destroy', result.id, name); callback(error); } }) : null;
      this.stdout = output('stdout');
      this.stderr = output('stderr');
      this.stdio = [this.stdin, this.stdout, this.stderr];
      this.connected = false;
      for (let index = 3; index < options.stdio.length; index++) {
        const name = 'fd' + index, id = result.id;
        if (options.stdio[index] !== 'pipe' || id === undefined) { this.stdio[index] = null; continue; }
        if (index === options.ipc) { this.stdio[index] = null; this[name] = channel(this, id, name); continue; }
        const pipe = this.stdio[index] = this[name] = new stream.Duplex({
          read() { if (this._paused) { this._paused = false; call('child.pause', id, name, false); } },
          write(data, _encoding, callback) { try { call('child.write', id, data, name); this._done = callback; } catch (error) { callback(error); } },
          writev(chunks, callback) { this._write(Buffer.concat(chunks.map(({ chunk }) => chunk)), null, callback); },
          final(callback) { try { call('child.end', id, name); this._finished = callback; } catch (error) { callback(error); } },
          destroy(error, callback) { if (!this._finished) try { call('child.end', id, name); } catch {} try { call('child.destroy', id, name); } catch {} callback(error); },
        });
        pipe._childWritten = () => { const callback = pipe._done; pipe._done = null; callback?.(); };
        pipe._childEnded = () => { const callback = pipe._finished; pipe._finished = null; callback?.(); };
      }
    }
    ref() { call('child.unref',this.#id,false);this._unreferenced=false;return this; }
    unref() { call('child.unref',this.#id,true);this._unreferenced=true;return this; }
    kill(signal = 'SIGTERM') {
      if (typeof signal === 'number') signal = Object.keys(globalThis.__apps.os.constants.signals).find(name => globalThis.__apps.os.constants.signals[name] === signal) ?? signal;
      if (typeof signal !== 'string') throw Object.assign(new TypeError(`Unknown signal: ${signal}`), { code: 'ERR_UNKNOWN_SIGNAL' });
      if (this.#id === undefined) return false;
      const killed = call('child.kill', this.#id, signal);
      if (killed) this.killed = true;
      return killed;
    }
  }
  const stdioOf = (stdio, index) => {
    if (stdio == null || stdio === 'overlapped') return index < 3 ? 'pipe' : 'ignore';
    if (stdio === 'ipc') return index < 3 ? null : 'pipe';
    if (stdio === 'pipe' || stdio === 'ignore' || stdio === 'inherit') return index < 3 || stdio !== 'inherit' ? stdio : null;
    const fd = typeof stdio === 'number' ? stdio : typeof stdio?.fd === 'number' && typeof stdio.on === 'function' ? stdio.fd : -1;
    return index < 3 && fd >= 0 && fd <= 2 ? fd : null;
  };
  const config = (options, sync) => {
    const result = { detached: !!options.detached, windowsHide: !!options.windowsHide };
    if (options.cwd != null) {
      const cwd = options.cwd instanceof URL ? globalThis.__apps.url.fileURLToPath(options.cwd) : options.cwd;
      if (typeof cwd !== 'string' || cwd.includes('\0')) throw Object.assign(new TypeError('The "options.cwd" property must be of type string or an instance of URL'), { code: 'ERR_INVALID_ARG_TYPE' });
      result.cwd = cwd;
    }
    for (const name of ['uid', 'gid']) if (options[name] != null) { if (!Number.isInteger(options[name])) throw Object.assign(new TypeError(`The "options.${name}" property must be of type number`), { code: 'ERR_INVALID_ARG_TYPE' }); result[name] = options[name]; }
    if (typeof options.argv0 === 'string') result.argv0 = options.argv0;
    const env = options.env ?? globalThis.process?.env;
    if (env !== undefined) {
      if (!env || typeof env !== 'object') throw Object.assign(new TypeError('The "options.env" property must be of type object'), { code: 'ERR_INVALID_ARG_TYPE' });
      result.env = Object.create(null);
      for (const name in env) {
        if (env[name] === undefined) continue;
        const value = String(env[name]);
        if (name.includes('=') || name.includes('\0') || value.includes('\0')) throw Object.assign(new TypeError('The "options.env" property must not contain null bytes or "=" in names'), { code: 'ERR_INVALID_ARG_VALUE' });
        result.env[name] = value;
      }
    }
    const stdio = options.stdio ?? 'pipe', list = typeof stdio === 'string' ? [stdio, stdio, stdio] : Array.isArray(stdio) ? stdio : null;
    if (!list) throw Object.assign(new TypeError(`The argument 'stdio' is invalid. Received ${stdio}`), { code: 'ERR_INVALID_ARG_VALUE' });
    result.stdio = [];
    for (let index = 0; index < Math.max(3, list.length); index++) { const value = stdioOf(list[index], index); if (value === null) throw Object.assign(new TypeError(`Unsupported stdio option at index ${index}: ${String(list[index])}`), { code: 'ERR_INVALID_ARG_VALUE' }); result.stdio.push(value); }
    const ipc = list.indexOf('ipc');
    if (ipc !== -1) {
      if (list.includes('ipc', ipc + 1)) throw Object.assign(new Error('Child process can have only one IPC pipe'), { code: 'ERR_IPC_ONE_PIPE' });
      if ((options.serialization ?? 'json') !== 'json') throw Object.assign(new TypeError(`The property 'options.serialization' must be 'json'. Received ${String(options.serialization)}`), { code: 'ERR_INVALID_ARG_VALUE' });
      if (process.platform === 'win32') throw Object.assign(new Error('IPC channels are not supported on Windows'), { code: 'ERR_FEATURE_UNAVAILABLE_ON_PLATFORM' });
      Object.assign(result.env ??= Object.create(null), { NODE_CHANNEL_FD: String(ipc), NODE_CHANNEL_SERIALIZATION_MODE: 'json' });
      result.ipc = ipc;
    }
    if (sync) Object.assign(result, { input: options.input, timeout: options.timeout, killSignal: options.killSignal ?? 'SIGTERM', maxBuffer: options.maxBuffer === Infinity ? Number.MAX_SAFE_INTEGER : options.maxBuffer });
    return result;
  };
  const command = (file, args, options) => {
    if (typeof file !== 'string' || !file || file.includes('\0')) throw Object.assign(new TypeError('The "file" argument must be of type string and not empty'), { code: 'ERR_INVALID_ARG_TYPE' });
    if (!Array.isArray(args)) { options = args; args = []; }
    options ??= {};
    if (typeof options !== 'object' || Array.isArray(options)) throw Object.assign(new TypeError('The "options" argument must be of type object'), { code: 'ERR_INVALID_ARG_TYPE' });
    args = args.map(value => String(value));
    if (args.some(value => value.includes('\0'))) throw Object.assign(new TypeError('The "args" argument must not contain null bytes'), { code: 'ERR_INVALID_ARG_VALUE' });
    if (options.shell) {
      const text=[file,...args].join(' ');
      file=typeof options.shell==='string'?options.shell:shell();args=shellArgs(file,text);
    }
    return [file, args, options];
  };
  const spawn = (...input) => {
    const [file, args, options] = command(...input), settings = config(options, false);
    const result = call('child.spawn', file, args, settings);
    const child = new ChildProcess(key, file, args, settings, result);
    children.set(result.id ?? --failureID, { child, failure: result.error });
    if(options.timeout>0){const timer=setTimeout(()=>child.kill(options.killSignal??'SIGTERM'),options.timeout);child.once('close',()=>clearTimeout(timer));}
    if(options.signal){const abort=()=>{child.kill(options.killSignal??'SIGTERM');child.emit('error',Object.assign(new Error('The operation was aborted',{cause:options.signal.reason}),{code:'ABORT_ERR',name:'AbortError'}));};if(options.signal.aborted)queueMicrotask(abort);else{options.signal.addEventListener('abort',abort,{once:true});child.once('close',()=>options.signal.removeEventListener('abort',abort));}}
    return child;
  };
  const closed = (child, code, signal, now) => {
    const close = () => child.emit('close', code, signal), open = [child.stdout, child.stderr].filter(output => output && !output.closed && output.readableFlowing);
    let left = open.length;
    if (left) for (const output of open) output.once('close', () => { if (!--left) close(); });
    else if (now) close(); else queueMicrotask(close);
  };
  const poll = () => {
    if (!children.size) return false;
    for (const [id, { child, failure }] of children) {
      if (!failure) continue;
      children.delete(id);
      child.exitCode = failure.errno ?? -1;
      child.emit('error', failed(failure, 'spawn ' + child.spawnfile, child.spawnfile, child.spawnargs.slice(1)));
      for (const output of child.stdio) output?.destroy();
      closed(child, child.exitCode, null, true);
    }
    for (const event of call('child.poll')) {
      const child = children.get(event.id)?.child;
      if (!child) continue;
      if (event.event === 'data') { const output=child[event.stream]; if(!output.destroyed&&!output.push(view(event.data))&&!output._paused){output._paused=true;call('child.pause',event.id,event.stream,true);} }
      else if (event.event === 'written') (child[event.stream] ?? child.stdin)?._childWritten();
      else if (event.event === 'finish') (child[event.stream] ?? child.stdin)?._childEnded();
      else if (event.event === 'end') { if (!child[event.stream].destroyed) child[event.stream].push(null); }
      else if (event.event === 'error') child[event.stream]?.destroy(Object.assign(new Error(`${event.stream === 'stdin' ? 'write' : 'read'} ${event.error.code}`), event.error, { message: `${event.stream === 'stdin' ? 'write' : 'read'} ${event.error.code}`, syscall: event.stream === 'stdin' ? 'write' : 'read' }));
      else if (event.event === 'exit' || event.event === 'close') {
        child.exitCode = event.code;
        child.signalCode = event.signal;
        if (event.event === 'close') { children.delete(event.id); closed(child, event.code, event.signal); }
        else if (child.connected) child.once('disconnect', () => child.emit('exit', event.code, event.signal));
        else child.emit('exit', event.code, event.signal);
      } else child.emit(event.event);
    }
    return children.size > 0;
  };
  const spawnSync=(...input)=>{
    const [file,args,options]=command(...input),settings=config(options,true);
    if(settings.stdio[0]!=='pipe')delete settings.input;else if(typeof settings.input==='string')settings.input=Buffer.from(settings.input,options.encoding&&options.encoding!=='buffer'?options.encoding:'utf8');
    const result=call('child.sync',file,args,settings);
    if(result.error){const error=failed(result.error,'spawnSync '+file,file,args);if(result.pid===undefined)return{error,status:null,signal:null,output:null,pid:0};result.error=error;}
    for(const name of ['stdout','stderr']){if(result[name]==null)continue;result[name]=view(result[name]);if(options.encoding&&options.encoding!=='buffer')result[name]=result[name].toString(options.encoding);}
    result.output=[null,result.stdout,result.stderr];return result;
  };
  const execFileSync=(file,args,options={},cmd=[file,...(Array.isArray(args)?args:[])].join(' '))=>{
    if(!Array.isArray(args)){options=args??{};args=[];}
    const result=spawnSync(file,args,options);
    if(!options.stdio&&result.stderr?.length)process.stderr.write(result.stderr);
    if(result.error||result.status!==0)throw Object.assign(result.error??new Error(`Command failed: ${cmd}${result.stderr?.length?'\n'+result.stderr:''}`),result);
    return result.stdout;
  };
  const execFile=(file,args=[],options={},callback,cmd)=>{
    if(typeof args==='function'){callback=args;args=[];options={};}else if(!Array.isArray(args)){callback=typeof options==='function'?options:callback;options=args??{};args=[];}
    if(typeof options==='function'){callback=options;options={};}
    const {encoding='utf8',maxBuffer=1024*1024,...spawnOptions}=options??{};
    cmd??=[file,...args].join(' ');
    const child=spawn(file,args,spawnOptions),out=[],err=[];let failure;
    const collect=(list,name)=>{let size=0;return data=>{if(size>maxBuffer)return;size+=data.length;if(size<=maxBuffer)return list.push(data);list.push(data.subarray(0,data.length-size+maxBuffer));failure??=Object.assign(new RangeError(`${name} maxBuffer length exceeded`),{code:'ERR_CHILD_PROCESS_STDIO_MAXBUFFER'});child.kill(options?.killSignal??'SIGTERM');};};
    child.stdout?.on('data',collect(out,'stdout'));
    child.stderr?.on('data',collect(err,'stderr'));
    child.on('error',error=>{failure??=Object.assign(error,{cmd});});
    child.once('close',(code,signal)=>{
      const stdout=Buffer.concat(out),stderr=Buffer.concat(err),text=value=>encoding==='buffer'||!encoding?value:value.toString(encoding);
      if((code!==0||signal)&&!failure)failure=Object.assign(new Error(`Command failed: ${cmd}${stderr.length?'\n'+stderr:''}`),{code:signal?null:code,killed:child.killed,signal,cmd});
      callback?.(failure??null,text(stdout),text(stderr));
    });
    return child;
  };
  const shellArgs=(shell,command)=>/(?:^|[\\/])cmd(?:\.exe)?$/i.test(shell)?['/d','/s','/c',command]:['-c',command];
  const shell=()=>process.platform==='win32'?process.env.ComSpec??'cmd.exe':'/bin/sh';
  const exec=(command,options,callback)=>{if(typeof options==='function'){callback=options;options={};}const{shell:custom,...rest}=options??{},program=typeof custom==='string'?custom:shell();return execFile(program,shellArgs(program,command),rest,callback,command);};
  const execSync=(command,options={})=>{const{shell:custom,...rest}=options,program=typeof custom==='string'?custom:shell();return execFileSync(program,shellArgs(program,command),rest,command);};
  const custom=Symbol.for('nodejs.util.promisify.custom');
  for(const run of [exec,execFile])run[custom]=(...args)=>{let child;const promise=new Promise((resolve,reject)=>{child=run(...args,(error,stdout,stderr)=>error?reject(Object.assign(error,{stdout,stderr})):resolve({stdout,stderr}));});promise.child=child;return promise;};
  const fork = (modulePath, args, options) => {
    if (modulePath instanceof URL || /^file:/.test(modulePath)) modulePath = globalThis.__apps.url.fileURLToPath(modulePath);
    if (typeof modulePath !== 'string') throw Object.assign(new TypeError('The "modulePath" argument must be of type string or an instance of URL'), { code: 'ERR_INVALID_ARG_TYPE' });
    if (!Array.isArray(args)) { options = args; args = []; }
    options ??= {};
    let stdio = options.stdio ?? (options.silent ? 'pipe' : [0, 1, 2]);
    if (typeof stdio === 'string') stdio = [stdio, stdio, stdio, 'ipc'];
    else if (!options.stdio) stdio = [...stdio, 'ipc'];
    else if (!stdio.includes('ipc')) throw Object.assign(new Error("Forked processes must have an IPC channel, missing value 'ipc' in options.stdio"), { code: 'ERR_CHILD_PROCESS_IPC_REQUIRED' });
    const env = { ...options.env ?? process.env, ELECTRON_RUN_AS_NODE: '1' };
    for (const name of ['APPS_DEV_BUNDLE', 'APPS_JSC_LIBRARY']) env[name] ??= process.env[name];
    return spawn(options.execPath ?? process.execPath, [...options.execArgv ?? process.execArgv, modulePath, ...args], { ...options, stdio, env, shell: false });
  };
  class UtilityProcess extends EventEmitter {
    #child;
    constructor(child) {
      super();
      this.#child = child;
      Object.assign(this, { pid: child.pid, stdout: child.stdout, stderr: child.stderr });
      child.on('spawn', () => this.emit('spawn')).on('message', message => this.emit('message', message));
      child.on('exit', (code, signal) => { this.pid = undefined; this.emit('exit', code ?? globalThis.__apps.os.constants.signals[signal] ?? 1); });
    }
    postMessage(message, transfer) {
      if (transfer?.length) throw Object.assign(new TypeError('Transferring MessagePortMain objects is not supported'), { code: 'ERR_INVALID_ARG_VALUE' });
      this.#child.send(message, () => {});
    }
    kill() { return this.#child.kill(); }
  }
  const utilityProcess = Object.freeze({ fork(modulePath, args, options) {
    if (!Array.isArray(args)) { options = args; args = []; }
    const { stdio = 'inherit', env = process.env, ...rest } = options ?? {}, [, stdout = 'inherit', stderr = 'inherit'] = typeof stdio === 'string' ? [, stdio, stdio] : stdio;
    return new UtilityProcess(fork(modulePath, args, { ...rest, env: { ...env, APPS_PARENT_PORT: '1' }, stdio: ['ignore', stdout, stderr, 'ipc'] }));
  } });
  let flush;
  const connect = (fd, port) => {
    const id = call('child.channel', fd);
    if (id == null) return;
    if (port) Object.assign(process, { type: 'utility', parentPort: Object.assign(new EventEmitter(), { postMessage: message => process.send(message) }) });
    const ipc = channel(process, id, 'ipc'), live = name => process.listenerCount(name) || process.parentPort?.listenerCount(name);
    children.set(id, { child: { ipc, stdio: [], emit() {}, get _unreferenced() { return !ipc.sent.length && !(process.connected && !ipc.unreferenced && (live('message') || live('disconnect'))); } } });
    flush = () => { if (ipc.sent.length) call('child.flush', id, 'ipc'); };
  };
  return { api: { spawn, fork, ChildProcess, spawnSync, execFileSync, execFile, exec, execSync }, utilityProcess, connect, flush: () => flush?.(), poll, pending: () => [...children.values()].some(({child})=>!child._unreferenced) };
};
