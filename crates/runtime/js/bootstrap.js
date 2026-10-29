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
  const inspector = globalThis.__appsInspect, { inspect, formatWithOptions } = inspector;
  delete globalThis.__appsInspect;
  const nodeRole = !(typeof role === 'object' && 'renderer' in role);
  let group = '';
  const write = (level, text) => call('log', group ? text.replace(/^/gm, group) : text, level);
  const print = level => (...args) => write(level, formatWithOptions({}, ...args));
  const counts = new Map(), clocks = new Map();
  const elapsed = label => { const ms = performance.now() - clocks.get(label); return `${label}: ${ms >= 1000 ? (ms / 1000).toFixed(3) + 's' : ms.toFixed(3) + 'ms'}`; };
  const missing = (label, method) => { if (clocks.has(label)) return false; write('warn', `No such label '${label}' for console.${method}()`); return true; };
  const cell = value => typeof value === 'string' ? inspect(value) : inspect(value, { depth: 0, breakLength: Infinity });
  globalThis.console = Object.freeze({
    log: print('log'), info: print('info'), warn: print('warn'), error: print('error'), debug: print('debug'),
    trace: (...args) => write('error', `Trace${args.length ? ': ' + formatWithOptions({}, ...args) : ''}\n${(new Error().stack ?? '').split('\n').slice(1).map(line => '    ' + line).join('\n')}`),
    dir: (value, options) => write('log', inspect(value, options)),
    assert: (condition, ...data) => { if (condition) return; if (typeof data[0] === 'string') data[0] = 'Assertion failed: ' + data[0]; else data.unshift('Assertion failed'); write('error', formatWithOptions({}, ...data)); },
    count: (label = 'default') => { const count = (counts.get(label) ?? 0) + 1; counts.set(label, count); write('info', `${label}: ${count}`); },
    countReset: (label = 'default') => { counts.delete(label); },
    time: (label = 'default') => { if (clocks.has(label)) write('warn', `Label '${label}' already exists for console.time()`); else clocks.set(label, performance.now()); },
    timeLog: (label = 'default', ...data) => { if (!missing(label, 'timeLog')) write('info', elapsed(label) + (data.length ? ' ' + formatWithOptions({}, ...data) : '')); },
    timeEnd: (label = 'default') => { if (!missing(label, 'timeEnd')) { write('info', elapsed(label)); clocks.delete(label); } },
    group: (...label) => { if (label.length) print('log')(...label); group += '  '; },
    groupCollapsed: (...label) => { if (label.length) print('log')(...label); group += '  '; },
    groupEnd: () => { group = group.slice(2); },
    table(data, columns) {
      if (data === null || typeof data !== 'object') return print('log')(data);
      const keys = [], rows = [];
      let values = false;
      for (const [index, value] of data instanceof Map ? data : Object.entries(data)) {
        const row = { '(index)': typeof index === 'string' ? index : cell(index) };
        if (value !== null && typeof value === 'object') for (const key of columns ?? Object.keys(value)) { if (!keys.includes(key)) keys.push(key); if (key in value) row[key] = cell(value[key]); }
        else { row.Values = cell(value); values = true; }
        rows.push(row);
      }
      const header = ['(index)', ...keys, ...(values ? ['Values'] : [])];
      const widths = header.map(name => Math.max(name.length, ...rows.map(row => (row[name] ?? '').length)) + 2);
      const line = (left, middle, right) => left + widths.map(width => '─'.repeat(width)).join(middle) + right;
      const cells = list => '│' + list.map((text, i) => ' ' + text.padEnd(widths[i] - 1)).join('│') + '│';
      write('log', [line('┌', '┬', '┐'), cells(header), line('├', '┼', '┤'), ...rows.map(row => cells(header.map(name => row[name] ?? ''))), line('└', '┴', '┘')].join('\n'));
    },
  });
  const timers = new Map();
  let timerID = 0;
  class Timeout {
    #id;
    constructor(id) { this.#id = id; }
    ref() { const timer = timers.get(this.#id); if (timer) timer.ref = true; return this; }
    unref() { const timer = timers.get(this.#id); if (timer) timer.ref = false; return this; }
    hasRef() { return timers.get(this.#id)?.ref ?? false; }
    refresh() { const timer = timers.get(this.#id); if (timer) timer.at = Date.now() + timer.delay; return this; }
    close() { timers.delete(this.#id); return this; }
    [Symbol.toPrimitive]() { return this.#id; }
  }
  globalThis.setTimeout = (fn, delay = 0, ...args) => {
    if (typeof fn !== 'function') throw new TypeError('Expected a timer function');
    const id = ++timerID;
    delay = Math.max(0, Number(delay) || 0);
    timers.set(id, { fn, args, delay, at: Date.now() + delay, ref: true });
    return nodeRole ? new Timeout(id) : id;
  };
  globalThis.clearTimeout = id => { timers.delete(+id); };
  globalThis.setInterval = (fn, delay = 0, ...args) => {
    const id = setTimeout(fn, delay, ...args);
    timers.get(+id).interval = Math.max(1, Number(delay) || 1);
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
  let children, servers, node, pty, browsers, net, nativeShell, uncaught = () => false;
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
    } catch (error) { if (!uncaught(error)) { if (role === 'main') throw error; console.error(error); } }
    const now = Date.now();
    for (const [id, timer] of [...timers]) {
      if (timer.at > now || !timers.has(id)) continue;
      if (timer.interval) timer.at = now + timer.interval;
      else timers.delete(id);
      try { timer.fn(...timer.args); } catch (error) { if (!uncaught(error)) { if (role === 'main') throw error; console.error(error); } }
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
  globalThis.global = globalThis;
  globalThis.setImmediate = (fn, ...args) => setTimeout(fn, 0, ...args);
  globalThis.clearImmediate = clearTimeout;
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
  node = globalThis.__appsNode(call, events.EventEmitter, inspector, info);
  delete globalThis.__appsNode;
  const stream = globalThis.__appsNodeStreams(events.EventEmitter, node.Buffer, node.string_decoder.StringDecoder);
  delete globalThis.__appsNodeStreams;
  const io = globalThis.__appsNodeIO({ call, sync: node.sync, asyncCall: node.asyncCall, Buffer: node.Buffer, stream, EventEmitter: events.EventEmitter, StringDecoder: node.string_decoder.StringDecoder, webcrypto: globalThis.__appsNodeCrypto.webcrypto });
  delete globalThis.__appsNodeIO;
  net = globalThis.__appsNet(call, events.EventEmitter, stream, node.Buffer);
  delete globalThis.__appsNet;
  const http = globalThis.__appsHTTP(servers.serve, stream, events.EventEmitter);
  delete globalThis.__appsHTTP;
  pty = globalThis.__appsPty(call);
  delete globalThis.__appsPty;
  browsers = globalThis.__appsBrowser(call, events.EventEmitter, () => ready, contents => app.emit('web-contents-created', event(), contents));
  delete globalThis.__appsBrowser;
  nativeShell = globalThis.__appsNativeShell(call, events.EventEmitter);
  delete globalThis.__appsNativeShell;
  // WARNING: process.version reports the Node.js API level that the node: modules follow.
  const nodeVersion = '24.0.0';
  let exiting = false;
  const exit = code => {
    if (code !== undefined) process.exitCode = code;
    if (!exiting) { exiting = true; process.emit('exit', Number(process.exitCode ?? 0) | 0); }
    call('quit', Number(process.exitCode ?? 0) | 0);
  };
  const memoryUsage = () => ({ rss: memoryUsage.rss(), heapTotal: 0, heapUsed: 0, external: 0, arrayBuffers: 0 });
  memoryUsage.rss = () => call('node.process.memory');
  const process = globalThis.process = Object.assign(new events.EventEmitter(), {
    platform: info.platform, arch: info.arch, pid: info.pid, title: info.appName,
    version: 'v' + nodeVersion, versions: Object.freeze({ node: nodeVersion, apps: info.version, javascriptcore: info.javascriptcore, napi: '8' }), release: Object.freeze({ name: 'node' }),
    argv: info.argv, argv0: info.argv[0], execArgv: [], env: info.env, execPath: info.execPath, exitCode: undefined,
    cwd: () => call('cwd'), chdir: path => call('chdir', path),
    nextTick: (fn, ...args) => queueMicrotask(() => fn(...args)),
    exit, kill: (pid, signal = 'SIGTERM') => { call('process.kill', pid, signal); return true; },
    hrtime: previous => { const now = performance.now() * 1e6, value = [Math.floor(now / 1e9), Math.floor(now % 1e9)]; if (previous) { value[0] -= previous[0]; value[1] -= previous[1]; if (value[1] < 0) { value[0]--; value[1] += 1e9; } } return value; },
    uptime: () => performance.now() / 1000,
    memoryUsage,
    cpuUsage: previous => { const usage = call('node.process.cpu'); return previous ? { user: usage.user - previous.user, system: usage.system - previous.system } : usage; },
    umask: mask => call('node.umask', mask === undefined ? null : typeof mask === 'string' ? parseInt(mask, 8) : mask),
    emitWarning(warning, options, code) {
      const type = typeof options === 'object' && options !== null ? options.type ?? 'Warning' : options ?? 'Warning';
      if (typeof options === 'object' && options !== null) code = options.code;
      if (typeof warning === 'string') warning = Object.assign(new Error(warning), { name: type }, code ? { code } : {});
      if (warning.name === 'DeprecationWarning' && process.noDeprecation) return;
      queueMicrotask(() => { process.emit('warning', warning); if (!process.noProcessWarnings) console.error(`(node:${info.pid}) ${warning.code ? `[${warning.code}] ` : ''}${warning.name}: ${warning.message}`); });
    },
  });
  process.hrtime.bigint = () => BigInt(Math.floor(performance.now() * 1e6));
  Object.defineProperty(process, 'ppid', { get: () => node.details().ppid ?? 0, enumerable: true });
  if (info.platform !== 'win32') Object.assign(process, { getuid: () => node.details().ids[0], geteuid: () => node.details().ids[1], getgid: () => node.details().ids[2], getegid: () => node.details().ids[3] });
  uncaught = error => {
    process.emit('uncaughtExceptionMonitor', error, 'uncaughtException');
    if (!process.listenerCount('uncaughtException')) return false;
    process.emit('uncaughtException', error, 'uncaughtException');
    return true;
  };
  addEventListener('unhandledrejection', event => { if (!process.listenerCount('unhandledRejection')) return; event.preventDefault(); process.emit('unhandledRejection', event.reason, event.promise); });
  const output = fd => {
    let tty;
    const writable = new stream.Writable({ write(data, _encoding, done) { call('process.write', fd, data); done(); } });
    // WARNING: Writes stay synchronous so they keep their order with console output.
    writable.write = (data, encoding, callback) => { if (typeof encoding === 'function') { callback = encoding; encoding = undefined; } call('process.write', fd, typeof data === 'string' && !encoding ? data : Buffer.from(data, encoding)); if (callback) queueMicrotask(callback); return true; };
    return Object.defineProperties(writable, { fd: { value: fd, enumerable: true }, isTTY: { get: () => tty ??= node.tty.isatty(fd), enumerable: true } });
  };
  process.stdout = output(1);
  process.stderr = output(2);
  const fs = node.fs;
  const handles = () => { let closed = false; return handle => { if (closed) return; closed = true; return handle.close(); }; };
  fs.createReadStream = (file, options = {}) => {
    if (typeof options === 'string') options = { encoding: options };
    const opening = fs.promises.open(file, options.flags ?? 'r'), close = handles();
    let offset = options.start ?? (options.end == null ? null : 0);
    const readable = new stream.Readable({ highWaterMark: options.highWaterMark ?? 65536, encoding: options.encoding,
      read(size) {
        opening.then(async handle => {
          const length = options.end == null ? size : Math.min(size, options.end - offset + 1);
          const { bytesRead, buffer } = length > 0 ? await handle.read(Buffer.allocUnsafe(length), 0, length, offset) : { bytesRead: 0 };
          if (offset !== null) offset += bytesRead;
          readable.bytesRead += bytesRead;
          readable.push(bytesRead ? buffer.subarray(0, bytesRead) : null);
        }).catch(error => readable.destroy(error));
      },
      destroy(error, done) { opening.then(close, () => {}).then(() => done(error), done); },
    });
    Object.assign(readable, { path: file, bytesRead: 0, pending: true, close(callback) { if (callback) this.once('close', callback); this.destroy(); } });
    opening.then(handle => { readable.pending = false; readable.fd = handle.fd; readable.emit('open', handle.fd); readable.emit('ready'); }, error => readable.destroy(error));
    return readable;
  };
  fs.createWriteStream = (file, options = {}) => {
    if (typeof options === 'string') options = { encoding: options };
    const opening = fs.promises.open(file, options.flags ?? 'w', options.mode), close = handles();
    const writable = new stream.Writable({ highWaterMark: options.highWaterMark, defaultEncoding: options.encoding,
      write(data, _encoding, done) { opening.then(handle => handle.writeFile(data)).then(() => { writable.bytesWritten += data.length; done(); }, done); },
      final(done) { opening.then(close).then(() => done(), done); },
      destroy(error, done) { opening.then(close, () => {}).then(() => done(error), done); },
    });
    Object.assign(writable, { path: file, bytesWritten: 0, pending: true, close(callback) { if (callback) this.once('close', callback); this.end(); } });
    opening.then(handle => { writable.pending = false; writable.fd = handle.fd; writable.emit('open', handle.fd); writable.emit('ready'); }, error => writable.destroy(error));
    return writable;
  };
  const { path, os } = node;
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
      app.emit('web-contents-created', event(), this.webContents);
      app.emit('browser-window-created', event(), this);
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
  const event = () => ({ defaultPrevented: false, preventDefault() { this.defaultPrevented = true; } });
  const switches = new Map(info.argv.slice(1).flatMap(arg => { const match = /^--([^=]+)(?:=(.*))?$/s.exec(arg); return match ? [[match[1], match[2] ?? '']] : []; }));
  let quitting = false, name = info.appName, locale;
  const locales = () => locale ??= info.platform === 'darwin' ? call('native.app.locale', null) : info.locale;
  const willQuit = () => {
    const will = event();
    app.emit('will-quit', will);
    if (will.defaultPrevented) { quitting = false; return; }
    app.emit('quit', event(), Number(process.exitCode ?? 0) | 0);
    exit();
  };
  const quit = () => {
    if (quitting) return;
    const before = event();
    app.emit('before-quit', before);
    if (before.defaultPrevented) return;
    quitting = true;
    for (const win of windows.values()) win.close();
    if (!windows.size) willQuit();
  };
  Object.assign(app, {
    whenReady: () => readyPromise, isReady: () => ready, quit,
    exit: (code = 0) => exit(code),
    relaunch: ({ args = info.argv.slice(1), execPath = info.execPath } = {}) => call('app.relaunch', [String(execPath), ...args.map(String)]),
    focus: ({ steal = false } = {}) => call('focus', !!steal),
    getAppPath: () => info.appPath,
    getPath: name => call('app.path', name),
    setPath: (name, path) => call('app.setPath', name, path),
    getVersion: () => info.appVersion,
    getName: () => name,
    setName: value => { name = String(value); },
    getLocale: () => locales().languages[0] ?? locales().system,
    getSystemLocale: () => locales().system,
    getPreferredSystemLanguages: () => [...locales().languages],
    getLocaleCountryCode: () => locales().system.split('-').slice(1).find(part => /^[A-Z]{2}$/.test(part)) ?? '',
    commandLine: Object.freeze({
      appendSwitch: (key, value = '') => { switches.set(String(key), String(value)); },
      appendArgument: () => {},
      hasSwitch: key => switches.has(key),
      getSwitchValue: key => switches.get(key) ?? '',
      removeSwitch: key => { switches.delete(key); },
    }),
    disableHardwareAcceleration: () => {},
    isHardwareAccelerationEnabled: () => true,
    isPackaged: info.isPackaged, platform: info.platform,
  });
  Object.defineProperty(app, 'name', { get: () => name, set: app.setName, enumerable: true });
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
  globalThis.__appsEvent = (name, id, data) => {
    const win = windows.get(id);
    if (name === 'ready') { app.emit('will-finish-launching'); ready = true; app.emit('ready'); resolveReady(); }
    else if (name === 'closed') {
      if (!win) return;
      const closing = quitting;
      windows.delete(id);
      for (const child of win.contentView.children) win.contentView.removeChildView(child);
      win.emit('closed');
      app.emit('window-close');
      if (windows.size) return;
      if (closing) willQuit();
      else if (quitting) return;
      else if (app.listenerCount('window-all-closed') || app.listenerCount('window-close')) app.emit('window-all-closed');
      else quit();
    }
    else if (name === 'resize') { if (win) { win.contentView.setBounds({ x: 0, y: 0, ...data }); win.emit('resize'); } }
    else if (name === 'loaded') win?.emit('ready-to-show');
    else if (name === 'focus' || name === 'blur') { if (win) { win.emit(name); app.emit('browser-window-' + name, event(), win); } }
    else if (name === 'activate') app.emit('activate', event(), !!data);
    else if (name === 'open-url' || name === 'open-file') app.emit(name, event(), data);
    else if (name === 'before-quit') quit();
  };
  const timerModule = { setTimeout, clearTimeout, setInterval, clearInterval, setImmediate, clearImmediate, promises: {
    setTimeout: (delay, value, options) => scheduler.wait(delay, options).then(() => value),
    setImmediate: (value, options) => scheduler.wait(0, options).then(() => value),
    async *setInterval(delay = 1, value, { signal } = {}) {
      signal?.throwIfAborted();
      let ticks = 0, wake = null;
      const abort = () => wake?.(), id = setInterval(() => { ticks++; wake?.(); }, delay);
      signal?.addEventListener('abort', abort, { once: true });
      try { for (;;) { if (!ticks) await new Promise(resolve => { wake = resolve; }); wake = null; signal?.throwIfAborted(); ticks--; yield value; } }
      finally { clearInterval(id); signal?.removeEventListener('abort', abort); }
    },
    scheduler,
  } };
  setTimeout[node.util.promisify.custom] = timerModule.promises.setTimeout;
  setImmediate[node.util.promisify.custom] = timerModule.promises.setImmediate;
  const modules = { fs, path, os, process, events, stream, util: node.util, url: node.url, buffer: node.buffer, string_decoder: node.string_decoder, assert: node.assert, querystring: node.querystring, dns: node.dns, tty: node.tty, zlib: io.zlib, readline: io.readline, crypto: { ...globalThis.__appsNodeCrypto, ...io.crypto }, child_process: children.api, net: net.api, http: http.http, https: http.https, worker_threads: workers.api, perf_hooks: { performance }, timers: timerModule, sqlite, ffi };
  const builtins = { ...modules, module: null, 'fs/promises': fs.promises, 'stream/promises': stream.promises, 'stream/consumers': stream.consumers, 'stream/web': { ReadableStream, WritableStream, TransformStream, ByteLengthQueuingStrategy, CountQueuingStrategy, TextEncoderStream, TextDecoderStream, CompressionStream, DecompressionStream }, 'timers/promises': timerModule.promises, 'util/types': node.util.types, 'path/posix': path.posix, 'path/win32': path.win32, 'assert/strict': node.assert.strict, 'readline/promises': io.readline.promises, 'dns/promises': node.dns.promises };
  builtins.module = { builtinModules: Object.keys(builtins), isBuiltin: name => Object.hasOwn(builtins, String(name).replace(/^node:/, '')), createRequire: () => name => {
    const key = String(name).replace(/^node:/, '');
    const value = key === 'electron' || key === '@pierre/apps' ? globalThis.__apps : key === 'node-pty' ? pty.api : key === 'ws' ? http.ws : Object.hasOwn(builtins, key) ? builtins[key] : undefined;
    if (value === undefined) throw Object.assign(new Error(`Cannot find module '${name}'`), { code: 'MODULE_NOT_FOUND' });
    return value;
  } };
  globalThis.__apps = Object.freeze({ app, BrowserWindow, View: browsers.View, WebContentsView: browsers.WebContentsView, session: browsers.session, ...nativeShell, ...bridge, serve: servers.serve, ...modules, module: builtins.module, pty: pty.api, ws: http.ws });
})();
