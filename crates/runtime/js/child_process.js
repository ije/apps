globalThis.__appsChildProcess = (call, EventEmitter) => {
  const children = new Map();
  const key = {};
  let failureID = 0;
  const latin1 = data => new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  class Input extends EventEmitter {
    #id;
    #ended = false;
    #buffered = 0;
    constructor(id) { super(); this.#id = id; }
    get writable() { return !this.#ended; }
    write(data) {
      if (this.#ended) throw new Error('Child stdin is closed');
      if (typeof data !== 'string') {
        if (!ArrayBuffer.isView(data)) throw new TypeError('Expected a string or typed array');
        data = latin1(data);
      }
      this.#buffered += call('child.write', this.#id, data);
      return this.#buffered < 65536;
    }
    end(data) {
      if (data !== undefined) this.write(data);
      if (!this.#ended) { call('child.end', this.#id); this.#ended = true; }
      return this;
    }
    _written(size) {
      const blocked = this.#buffered >= 65536;
      this.#buffered -= size;
      if (blocked && this.#buffered < 65536) this.emit('drain');
    }
    _finish() { this.#ended = true; this.#buffered = 0; this.emit('finish'); this.emit('close'); }
  }
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
      this.stdin = options.stdio[0] === 'pipe' ? new Input(result.id) : null;
      this.stdout = options.stdio[1] === 'pipe' ? new (globalThis.__apps?.stream.Readable ?? EventEmitter)({ read() {} }) : null;
      this.stderr = options.stdio[2] === 'pipe' ? new (globalThis.__apps?.stream.Readable ?? EventEmitter)({ read() {} }) : null;
      this.stdio = [this.stdin, this.stdout, this.stderr];
    }
    ref() { call('child.unref',this.#id,false);this._unreferenced=false;return this; }
    unref() { call('child.unref',this.#id,true);this._unreferenced=true;return this; }
    kill(signal = 'SIGTERM') {
      if (typeof signal !== 'string') throw new TypeError('Signal must be a string');
      if (this.#id === undefined) return false;
      const killed = call('child.kill', this.#id, signal);
      if (killed) this.killed = true;
      return killed;
    }
  }
  const spawn = (command, args = [], options = {}) => {
    if (typeof command !== 'string' || !command || command.includes('\0')) throw new TypeError('Expected a nonempty command without null bytes');
    if (!Array.isArray(args)) { options = args; args = []; }
    if (!options || typeof options !== 'object' || Array.isArray(options)) throw new TypeError('Expected spawn options');
    for (const name of Object.keys(options)) if (!['cwd', 'env', 'stdio', 'detached', 'windowsHide', 'shell', 'signal', 'timeout', 'killSignal', 'argv0', 'encoding', 'serialization', 'windowsVerbatimArguments'].includes(name)) throw new Error('Unsupported spawn option: ' + name);
    args = args.map(value => String(value));
    if (args.some(value => value.includes('\0'))) throw new TypeError('Arguments cannot contain null bytes');
    if (options.shell) {
      const text=[command,...args].join(' ');
      command=typeof options.shell==='string'?options.shell:shell();args=shellArgs(command,text);
    }
    const config = { detached: !!options.detached, windowsHide: !!options.windowsHide };
    if (options.cwd !== undefined) {
      if (typeof options.cwd !== 'string' || options.cwd.includes('\0')) throw new TypeError('cwd must be a path string');
      config.cwd = options.cwd;
    }
    const env = options.env ?? globalThis.process?.env;
    if (env !== undefined) {
      if (!env || typeof env !== 'object') throw new TypeError('env must be an object');
      config.env = Object.create(null);
      for (const name of Object.keys(env)) {
        if (env[name] === undefined) continue;
        const value = String(env[name]);
        if (name.includes('=') || name.includes('\0') || value.includes('\0')) throw new TypeError('Invalid environment entry');
        config.env[name] = value;
      }
    }
    const stdio = options.stdio ?? 'pipe';
    if (typeof stdio !== 'string' && (!Array.isArray(stdio) || stdio.length > 3)) throw new TypeError('Unsupported stdio option');
    config.stdio = [0, 1, 2].map(index => typeof stdio === 'string' ? stdio : stdio[index] ?? 'pipe');
    if (config.stdio.some(value => !['pipe', 'ignore', 'inherit'].includes(value))) throw new TypeError('Unsupported stdio option');
    const result = call('child.spawn', command, args, config);
    const child = new ChildProcess(key, command, args, config, result);
    children.set(result.id ?? --failureID, { child, failure: result.error });
    if(options.timeout>0){const timer=setTimeout(()=>child.kill(options.killSignal??'SIGTERM'),options.timeout);child.once('close',()=>clearTimeout(timer));}
    if(options.signal){const abort=()=>{child.kill(options.killSignal??'SIGTERM');child.emit('error',Object.assign(new Error('The operation was aborted'),{code:'ABORT_ERR',name:'AbortError'}));};if(options.signal.aborted)queueMicrotask(abort);else options.signal.addEventListener('abort',abort,{once:true});child.once('close',()=>options.signal.removeEventListener('abort',abort));}
    return child;
  };
  const poll = () => {
    if (!children.size) return false;
    for (const [id, { child, failure }] of children) {
      if (!failure) continue;
      children.delete(id);
      const error = Object.assign(new Error(), failure, { message: 'spawn ' + child.spawnfile + ': ' + failure.message, path: child.spawnfile, spawnargs: child.spawnargs.slice(1), syscall: 'spawn ' + child.spawnfile });
      child.exitCode = failure.errno ?? -1;
      child.emit('error', error);
      child.stdin?._finish();
      child.stdout?.emit('close');
      child.stderr?.emit('close');
      child.emit('close', child.exitCode, null);
    }
    for (const event of call('child.poll')) {
      const child = children.get(event.id)?.child;
      if (!child) continue;
      if (event.event === 'data') { const output=child[event.stream], data=Buffer.from(event.data,'latin1'); if(output.push)output.push(data);else output.emit('data',data); }
      else if (event.event === 'written') child.stdin._written(event.bytes);
      else if (event.event === 'finish') child.stdin._finish();
      else if (event.event === 'end') { const output=child[event.stream]; if(output.push)output.push(null);else output.emit('end');queueMicrotask(()=>output.emit('close')); }
      else if (event.event === 'error') child[event.stream].emit('error', Object.assign(new Error(event.error.message), event.error));
      else if (event.event === 'exit' || event.event === 'close') {
        child.exitCode = event.code;
        child.signalCode = event.signal;
        if (event.event === 'close') { children.delete(event.id); queueMicrotask(() => child.emit('close', event.code, event.signal)); }
        else child.emit('exit', event.code, event.signal);
      } else child.emit(event.event);
    }
    return children.size > 0;
  };
  const spawnSync=(command,args=[],options={})=>{
    if(!Array.isArray(args)){options=args;args=[];}
    if(options.shell){const text=[command,...args].join(' ');command=typeof options.shell==='string'?options.shell:shell();args=shellArgs(command,text);}
    const result=call('child.sync',command,args,{...options,env:options.env??globalThis.process?.env,input:ArrayBuffer.isView(options.input)?latin1(options.input):options.input});
    for(const name of ['stdout','stderr']){if(result[name]==null)continue;result[name]=Buffer.from(result[name],'latin1');if(options.encoding&&options.encoding!=='buffer')result[name]=result[name].toString(options.encoding);}
    result.output=[null,result.stdout,result.stderr];if(result.error)result.error=Object.assign(new Error(result.error.message),result.error);return result;
  };
  const execFileSync=(command,args,options={},cmd=[command,...(Array.isArray(args)?args:[])].join(' '))=>{
    if(!Array.isArray(args)){options=args??{};args=[];}
    const result=spawnSync(command,args,options);
    if(!options.stdio&&result.stderr?.length)process.stderr.write(result.stderr);
    if(result.error||result.status!==0)throw Object.assign(result.error??new Error(`Command failed: ${cmd}${result.stderr?.length?'\n'+result.stderr:''}`),result);
    return result.stdout;
  };
  const execFile=(command,args=[],options={},callback,cmd)=>{
    if(typeof args==='function'){callback=args;args=[];options={};}else if(!Array.isArray(args)){callback=typeof options==='function'?options:callback;options=args;args=[];}
    if(typeof options==='function'){callback=options;options={};}
    const {encoding='utf8',maxBuffer=1024*1024,...spawnOptions}=options;
    const child=spawn(command,args,spawnOptions),out=[],err=[];let size=0,failure;
    child.stdout?.on('data',data=>{out.push(data);size+=data.length;if(size>maxBuffer){failure=Object.assign(new Error('maxBuffer exceeded'),{code:'ERR_CHILD_PROCESS_STDIO_MAXBUFFER'});child.kill();}});
    child.stderr?.on('data',data=>{err.push(data);size+=data.length;if(size>maxBuffer){failure=Object.assign(new Error('maxBuffer exceeded'),{code:'ERR_CHILD_PROCESS_STDIO_MAXBUFFER'});child.kill();}});
    child.on('error',error=>{failure=error;});
    child.once('close',(code,signal)=>{
      const stdout=Buffer.concat(out),stderr=Buffer.concat(err),text=value=>encoding==='buffer'?value:value.toString(encoding);
      cmd??=[command,...args].join(' ');
      if(code!==0&&!failure)failure=Object.assign(new Error(`Command failed: ${cmd}${stderr.length?'\n'+stderr:''}`),{code,killed:child.killed,signal,cmd});
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
