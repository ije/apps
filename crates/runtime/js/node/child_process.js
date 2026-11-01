globalThis.__appsChildProcess = (call, EventEmitter, stream) => {
  const children = new Map();
  const key = {};
  let failureID = 0;
  const view = data => Buffer.from(data.buffer, data.byteOffset, data.byteLength);
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
    if (stdio === 'pipe' || stdio === 'ignore' || stdio === 'inherit') return index < 3 || stdio === 'ignore' ? stdio : null;
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
    result.stdio = [0, 1, 2].map(index => stdioOf(list[index], index));
    for (let index = 0; index < Math.max(3, list.length); index++) if (stdioOf(list[index], index) === null) throw Object.assign(new TypeError(list[index] === 'ipc' ? 'IPC channels are not supported' : `Unsupported stdio option at index ${index}: ${String(list[index])}`), { code: 'ERR_INVALID_ARG_VALUE' });
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
      else if (event.event === 'written') child.stdin?._childWritten();
      else if (event.event === 'finish') child.stdin?._childEnded();
      else if (event.event === 'end') { if (!child[event.stream].destroyed) child[event.stream].push(null); }
      else if (event.event === 'error') child[event.stream]?.destroy(Object.assign(new Error(`${event.stream === 'stdin' ? 'write' : 'read'} ${event.error.code}`), event.error, { message: `${event.stream === 'stdin' ? 'write' : 'read'} ${event.error.code}`, syscall: event.stream === 'stdin' ? 'write' : 'read' }));
      else if (event.event === 'exit' || event.event === 'close') {
        child.exitCode = event.code;
        child.signalCode = event.signal;
        if (event.event === 'close') { children.delete(event.id); closed(child, event.code, event.signal); }
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
  return { api: { spawn, ChildProcess, spawnSync, execFileSync, execFile, exec, execSync }, poll, pending: () => [...children.values()].some(({child})=>!child._unreferenced) };
};
