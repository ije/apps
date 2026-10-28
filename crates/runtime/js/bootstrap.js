(() => {
  const native = globalThis.__appsNative;
  const role = globalThis.__appsRole;
  delete globalThis.__appsNative;
  delete globalThis.__appsRole;
  const stringify = JSON.stringify.bind(JSON), parse = JSON.parse.bind(JSON);
  const pack = value => {
    const buffers = [];
    const json = stringify(value, function(key, value) {
      const original = this[key];
      if (original instanceof ArrayBuffer || ArrayBuffer.isView(original)) {
        const index = buffers.length;
        buffers.push(original instanceof ArrayBuffer ? new Uint8Array(original) : new Uint8Array(original.buffer, original.byteOffset, original.byteLength));
        return { $appsBytes: index };
      }
      return value;
    });
    return { json, buffers };
  };
  const unpack = result => typeof result === 'string' ? parse(result) : parse(result.json, (_, value) => value && typeof value === 'object' && Object.hasOwn(value, '$appsBytes') ? new Uint8Array(result[value.$appsBytes]) : value);
  const call = (op, ...args) => {
    const request = pack({ __proto__: null, op, args });
    const result = unpack(native(request.json, request.buffers));
    if (result.error) throw Object.assign(new Error(result.error), result.code ? {code: result.code} : {});
    return result.value;
  };
  globalThis.__appsBridgeReceiveBytes = value => pack(__appsBridgeReceive(unpack(value)));
  const finalizers = new Set();
  globalThis.__appsFinalizers = class {
    constructor(release) { this.release = release; this.live = new Map(); this.tokens = new WeakMap(); finalizers.add(this); }
    register(object, value, token) { const key = {}; this.live.set(key, { ref: new WeakRef(object), value }); if (token) { let keys = this.tokens.get(token); if (!keys) this.tokens.set(token, keys = new Set()); keys.add(key); } }
    unregister(token) { const keys = this.tokens.get(token); if (!keys) return false; for (const key of keys) this.live.delete(key); this.tokens.delete(token); return true; }
  };
  const print = (level, ...args) => call('log', args.map(value => { if (typeof value === 'string') return value; if (value instanceof Error) return value.stack ? String(value)+'\n'+value.stack : String(value); try { return JSON.stringify(value); } catch { return String(value); } }).join(' '), level);
  globalThis.console = Object.freeze(Object.fromEntries(['log', 'info', 'warn', 'error', 'debug'].map(level => [level, (...args) => print(level, ...args)])));
  const timers = new Map();
  let timerID = 0;
  globalThis.setTimeout = (fn, delay = 0, ...args) => {
    if (typeof fn !== 'function') throw new TypeError('Expected a timer function');
    const id = ++timerID;
    timers.set(id, { fn, args, at: Date.now() + Math.max(0, Number(delay) || 0) });
    return id;
  };
  globalThis.clearTimeout = id => timers.delete(id);
  globalThis.setInterval = (fn, delay = 0, ...args) => {
    const id = setTimeout(fn, delay, ...args);
    timers.get(id).interval = Math.max(1, Number(delay) || 1);
    return id;
  };
  globalThis.clearInterval = clearTimeout;
  globalThis.queueMicrotask = fn => Promise.resolve().then(fn);
  const pollWeb = globalThis.__appsWeb(call);
  delete globalThis.__appsWeb;
  globalThis.__appsPlatform(call);
  delete globalThis.__appsPlatform;
  let modulesPending = 0;
  const moduleErrors = [];
  globalThis.__appsTrackModule = promise => {
    modulesPending++;
    promise.then(() => { modulesPending--; }, error => { modulesPending--; moduleErrors.push(error); });
  };
  let children, servers, node, pty, browsers, net, nativeShell;
  const workers = globalThis.__appsWorkers(call, !(typeof role === "object" && "renderer" in role));
  delete globalThis.__appsWorkers;
  globalThis.__appsBrowserEnvironment(call);
  delete globalThis.__appsBrowserEnvironment;
  const bridge = globalThis.__appsBridge(call, role, globalThis.__appsEvents?.EventEmitter);
  delete globalThis.__appsBridge;
  __appsSyncBridge();
  globalThis.__appsPending = () => modulesPending > 0 || moduleErrors.length > 0 || (children?.pending() ?? false) || (node?.poll.pending() ?? false) || (pty?.poll.pending() ?? false) || bridge.poll.pending() || workers.pending() || (net?.pending() ?? false);
  globalThis.__appsTick = () => {
    if (moduleErrors.length) throw moduleErrors.shift();
    try {
      for (const registry of finalizers) for (const [key, entry] of registry.live) if (!entry.ref.deref()) { registry.live.delete(key); registry.release(entry.value); }
      globalThis.__appsDomSweep?.();
      pollWeb();
      children?.poll();
      servers?.poll();
      node?.poll();
      pty?.poll();
      browsers?.poll();
      workers.poll();
      net?.poll();
      nativeShell?.poll?.();
    } catch (error) { if (role === 'main') throw error; console.error(error); }
    const now = Date.now();
    for (const [id, timer] of [...timers]) {
      if (timer.at > now || !timers.has(id)) continue;
      if (timer.interval) timer.at = now + timer.interval;
      else timers.delete(id);
      try { timer.fn(...timer.args); } catch (error) { if (role === 'main') throw error; console.error(error); }
    }
    return __appsDelay();
  };
  globalThis.__appsDelay = () => {
    let at = Infinity;
    for (const timer of timers.values()) if (timer.at < at) at = timer.at;
    const delay = at === Infinity ? -1 : Math.max(0, at - Date.now());
    return pollWeb.pending() || servers?.pending() || __appsPending() || moduleErrors.length ? (delay < 0 ? 4 : Math.min(4, delay)) : browsers?.pending() || nativeShell?.pending?.() ? (delay < 0 ? 16 : Math.min(16, delay)) : delay;
  };
  if (typeof role === 'object' && 'renderer' in role) {
    globalThis.__appsDOM = call;
    globalThis.__appsEventState = pollWeb.eventState;
    globalThis.__appsGPU = call;
    return;
  }
  const info = call('info');
  const ffi = globalThis.__appsFFI(globalThis.__appsFFINative);
  delete globalThis.__appsFFI;
  delete globalThis.__appsFFINative;
  const sqlite = globalThis.__appsSQLite(call);
  delete globalThis.__appsSQLite;
  const events = globalThis.__appsEvents;
  delete globalThis.__appsEvents;
  children = globalThis.__appsChildProcess(call, events.EventEmitter);
  delete globalThis.__appsChildProcess;
  servers = globalThis.__appsServe(call);
  delete globalThis.__appsServe;
  node = globalThis.__appsNode(call, events.EventEmitter);
  delete globalThis.__appsNode;
  const stream = globalThis.__appsNodeStreams(events.EventEmitter, node.Buffer);
  delete globalThis.__appsNodeStreams;
  net = globalThis.__appsNet(call, events.EventEmitter, stream, node.Buffer);
  delete globalThis.__appsNet;
  const http = globalThis.__appsHTTP(servers.serve, stream, events.EventEmitter);
  delete globalThis.__appsHTTP;
  pty = globalThis.__appsPty(call);
  delete globalThis.__appsPty;
  browsers = globalThis.__appsBrowser(call, events.EventEmitter, () => ready);
  delete globalThis.__appsBrowser;
  nativeShell = globalThis.__appsNativeShell(call, events.EventEmitter);
  delete globalThis.__appsNativeShell;
  const process = globalThis.process = Object.assign(new events.EventEmitter(), {
    platform: info.platform, arch: info.arch, pid: info.pid,
    versions: Object.freeze({ apps: info.version, javascriptcore: info.javascriptcore, napi: '8' }),
    argv: info.argv, env: info.env,
    cwd: () => call('cwd'), chdir: path => call('chdir', path),
    nextTick: (fn, ...args) => queueMicrotask(() => fn(...args)),
    exit: (code = 0) => call('quit', code),
  });
  const fs = node.fs;
  fs.createReadStream = (file, options = {}) => {
    let handle, reading = false, offset = options.start ?? null;
    const readable = new stream.Readable({ highWaterMark: options.highWaterMark, read() {
      if (reading) return; reading = true;
      (async () => {
        try {
          handle ??= await fs.promises.open(file, options.flags ?? 'r');
          const bytes = Buffer.alloc(Math.min(options.highWaterMark ?? 65536, options.end == null ? 65536 : Math.max(0, options.end - (offset ?? 0) + 1)));
          const { bytesRead } = await handle.read(bytes, 0, bytes.length, offset);
          if (offset !== null) offset += bytesRead;
          reading = false;
          if (!bytesRead) { await handle.close(); handle = null; readable.push(null); }
          else readable.push(bytes.subarray(0, bytesRead));
        } catch (error) { reading = false; readable.destroy(error); }
      })();
    }, destroy(error, done) { Promise.resolve(handle?.close()).then(() => done(error), done); } });
    if (options.encoding) readable.setEncoding(options.encoding);
    return readable;
  };
  fs.createWriteStream = (file, options = {}) => {
    let handle;
    return new stream.Writable({ write(data, _encoding, done) {
      (async () => { handle ??= await fs.promises.open(file, options.flags ?? 'w', options.mode); await handle.writeFile(data); })().then(() => done(), done);
    }, final(done) { Promise.resolve(handle?.close()).then(() => done(), done); }, destroy(error, done) { Promise.resolve(handle?.close()).then(() => done(error), done); } });
  };
  process.stdout = new stream.Writable({ write(data, _encoding, done) { call('process.write', 1, data.toString()); done(); } });
  process.stderr = new stream.Writable({ write(data, _encoding, done) { call('process.write', 2, data.toString()); done(); } });
  process.execPath = info.execPath;
  process.kill = (pid, signal = 'SIGTERM') => call('process.kill', pid, signal);
  process.hrtime = previous => { const now = performance.now() * 1e6, value = [Math.floor(now / 1e9), Math.floor(now % 1e9)]; if (previous) { value[0] -= previous[0]; value[1] -= previous[1]; if (value[1] < 0) { value[0]--; value[1] += 1e9; } } return value; };
  process.hrtime.bigint = () => BigInt(Math.floor(performance.now() * 1e6));
  const makePath = windows => {
    const sep = windows ? '\\' : '/';
    const slash = path => {
      if (typeof path !== 'string') throw new TypeError('Path must be a string');
      return windows ? path.replace(/\\/g, '/') : path;
    };
    const root = path => windows ? (path.match(/^(?:[A-Za-z]:\/|\/\/[^/]+\/[^/]+\/?|\/)/)?.[0] ?? '') : path.startsWith('/') ? '/' : '';
    const normalize = input => {
      const path = slash(input);
      if (windows && /^[A-Za-z]:[^/]/.test(path)) throw Error('Drive-relative paths are not supported');
      const prefix = root(path);
      const parts = [];
      for (const part of path.slice(prefix.length).split('/')) {
        if (!part || part === '.') continue;
        if (part === '..' && parts.length && parts.at(-1) !== '..') parts.pop();
        else if (part !== '..' || !prefix) parts.push(part);
      }
      const value = (prefix.endsWith('/') || !prefix ? prefix : prefix + '/') + parts.join('/');
      return ((value || '.') + (path.endsWith('/') && parts.length ? '/' : '')).replaceAll('/', sep);
    };
    const path = {
      sep, delimiter: windows ? ';' : ':', normalize,
      isAbsolute: value => windows ? /^(?:[A-Za-z]:\/|\/)/.test(slash(value)) : slash(value).startsWith('/'),
      join: (...parts) => normalize(parts.filter(value => slash(value)).join('/')),
      resolve(...parts) {
        let value = '';
        for (let i = parts.length - 1; i >= -1; i--) {
          const part = i >= 0 ? parts[i] : process.cwd();
          if (!part) continue;
          value = slash(part) + (value ? '/' + value : '');
          if (path.isAbsolute(part)) break;
        }
        const normalized = normalize(value);
        if (slash(normalized) === root(slash(normalized))) return normalized;
        return normalized.replace(new RegExp('\\' + sep + '$'), '') || sep;
      },
      basename(value, suffix = '') {
        const name = slash(value).replace(/\/+$/, '').split('/').at(-1) || '';
        return suffix && name.endsWith(suffix) ? name.slice(0, -suffix.length) : name;
      },
      dirname(value) {
        const raw = slash(value);
        const prefix = root(raw);
        if (raw === prefix) return raw.replaceAll('/', sep);
        const s = raw.replace(/\/+$/, '');
        const index = s.lastIndexOf('/');
        if (index < 0) return '.';
        return (index < prefix.length ? prefix : s.slice(0, index) || '/').replaceAll('/', sep);
      },
      extname(value) {
        const name = path.basename(value);
        const index = name.lastIndexOf('.');
        return index <= 0 || name === '..' ? '' : name.slice(index);
      },
      relative(from, to) {
        const a = slash(path.resolve(from)).split('/');
        const b = slash(path.resolve(to)).split('/');
        if (windows && a[0].toLowerCase() !== b[0].toLowerCase()) return path.resolve(to);
        let i = 0;
        while (i < a.length && i < b.length && (windows ? a[i].toLowerCase() === b[i].toLowerCase() : a[i] === b[i])) i++;
        return [...a.slice(i).filter(Boolean).map(() => '..'), ...b.slice(i)].join(sep);
      },
    };
    return path;
  };
  const posix = makePath(false), win32 = makePath(true);
  const path = info.platform === 'win32' ? win32 : posix;
  posix.posix = win32.posix = posix;
  posix.win32 = win32.win32 = win32;
  const os = Object.freeze({
    platform: () => info.platform, arch: () => info.arch,
    homedir: () => info.homedir, tmpdir: () => info.tmpdir,
    hostname: () => info.hostname, availableParallelism: () => info.parallelism,
    type: () => ({ darwin: 'Darwin', win32: 'Windows_NT', linux: 'Linux' })[info.platform],
    EOL: info.platform === 'win32' ? '\r\n' : '\n',
  });
  let ready = false, nextWindow = 0, resolveReady;
  const readyPromise = new Promise(resolve => { resolveReady = resolve; });
  const windows = new Map();
  class BrowserWindow extends events.EventEmitter {
    constructor(options = {}) {
      super();
      if (!ready) throw Error('Call app.whenReady() before creating a window');
      this.id = ++nextWindow;
      this.webContents = new events.EventEmitter();
      Object.assign(this.webContents, { id: this.id, send: (channel, ...args) => bridge.send(this.id, channel, ...args), openDevTools: () => call('window', 'devtools', this.id), toggleDevTools: () => call('window', 'devtools', this.id) });
      call('window', 'create', this.id, options);
      windows.set(this.id, this);
      Object.defineProperty(this, 'contentView', { value: browsers.contentView(this, options), enumerable: true });
    }
    load(html) { if (this.isDestroyed()) throw Error('Window is closed'); call('window', 'load', this.id, String(html)); }
    loadFile(file) { if (this.isDestroyed()) throw Error('Window is closed'); call('window', 'loadFile', this.id, String(file)); }
    loadURL() { throw Error('loadURL is not allowed, use WebContentsView to load webpage'); }
    close() { if (!this.isDestroyed()) call('window', 'close', this.id); }
    destroy() { this.close(); }
    show() { if (this.isDestroyed()) throw Error('Window is closed'); call('window', 'show', this.id); }
    setTitle(title) { if (this.isDestroyed()) throw Error('Window is closed'); call('window', 'title', this.id, String(title)); }
    isDestroyed() { return !windows.has(this.id); }
    static getAllWindows() { return [...windows.values()]; }
    static fromId(id) { return windows.get(id) ?? null; }
  }
  const app = new events.EventEmitter();
  Object.assign(app, { whenReady: () => readyPromise, isReady: () => ready, quit: () => call('quit', 0), getVersion: () => info.appVersion, getPath: name => call('app.path', name), getName: () => info.appName, getLocale: () => info.locale, isPackaged: info.isPackaged, platform: info.platform });
  Object.defineProperty(app, 'hasWindows', { get: () => windows.size > 0, enumerable: true });
  nativeShell.bindApp(app,info.appName);
  if (role === 'main' && workers.api.isMainThread) globalThis.__appsStart = options => {
    if (!options || typeof options !== 'object') return;
    const handlers = [['ready', 'ready'], ['activate', 'activate'], ['windowClose', 'window-close']];
    for (const name of [...handlers.map(([name]) => name), 'fetch']) {
      if (options[name] !== undefined && typeof options[name] !== 'function') throw new TypeError(`Expected a function for ${name}`);
    }
    if (options.fetch !== undefined) servers.serve(options);
    let onReady;
    for (const [name, event] of handlers) {
      const handler = options[name];
      if (handler === undefined) continue;
      const listener = () => {
        const result = handler.call(options, app);
        if (result && typeof result.then === 'function') __appsTrackModule(Promise.resolve(result));
      };
      if (name === 'ready') {
        if (ready) onReady = listener;
        else app.once(event, listener);
      } else app.on(event, listener);
    }
    onReady?.();
  };
  globalThis.__appsEvent = (name, id, bounds) => {
    if (name === 'ready') { ready = true; app.emit('ready'); resolveReady(); }
    else if (name === 'closed') { const win = windows.get(id); if (!win) return; windows.delete(id); for (const child of win.contentView.children) win.contentView.removeChildView(child); win.emit('closed'); app.emit('window-close'); if (!windows.size) app.emit('window-all-closed'); }
    else if (name === 'resize') { const win = windows.get(id); if (win) { win.contentView.setBounds({ x: 0, y: 0, ...bounds }); win.emit('resize'); } }
    else if (name === 'loaded') windows.get(id)?.emit('ready-to-show');
    else app.emit(name);
  };
  globalThis.__apps = Object.freeze({ app, BrowserWindow, View: browsers.View, WebContentsView: browsers.WebContentsView, session: browsers.session, ...nativeShell, ...bridge, serve: servers.serve, fs, path, os, process, sqlite, events, child_process: children.api, pty: pty.api, ffi, buffer: { Buffer: node.Buffer }, stream, net: net.api, http: http.http, https: http.https, ws: http.ws, util: node.util, url: node.url, string_decoder: node.string_decoder, crypto: globalThis.__appsNodeCrypto, worker_threads: workers.api, timers: { setTimeout: (delay, value, options) => scheduler.wait(delay, options).then(() => value), setImmediate: value => scheduler.wait(0).then(() => value), scheduler }, module: { createRequire: () => name => { const key=name.replace(/^node:/, ""); const value = key==='electron'?globalThis.__apps:key==='node-pty'?globalThis.__apps.pty:key==='fs/promises'?globalThis.__apps.fs.promises:key==='stream/promises'?globalThis.__apps.stream.promises:key==='stream/web'?{ReadableStream,WritableStream,TransformStream}:globalThis.__apps[key]; if (value === undefined) throw new Error("Unsupported required module: " + name); return value; }, isBuiltin: name => Object.hasOwn(globalThis.__apps, name.replace(/^node:/, "")), get builtinModules() { return Object.keys(globalThis.__apps); } } });
})();
