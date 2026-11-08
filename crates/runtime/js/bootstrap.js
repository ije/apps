(() => {
  const native = globalThis.__appsNative, clock = globalThis.__appsNow;
  const role = globalThis.__appsRole, disabled = new Set(globalThis.__appsDisabled);
  delete globalThis.__appsNative;
  delete globalThis.__appsRole;
  delete globalThis.__appsDisabled;
  const feature = name => {
    const fail = () => { throw new Error(`The ${name} feature is disabled. Add "${name}" to "features" in package.json`); };
    const stub = new Proxy(function() {}, { get: (_, key) => typeof key === 'symbol' || key === 'then' ? undefined : stub, apply: fail, construct: fail });
    return stub;
  };
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
  const nested = (value, depth) => {
    if (value === null || typeof value !== 'object') return false;
    if (depth > 32 || value instanceof ArrayBuffer || ArrayBuffer.isView(value)) return true;
    for (const key in value) if (nested(value[key], depth + 1)) return true;
    return false;
  };
  // WARNING: primitives and top-level bytes go to Rust as JS values. Other arguments go as JSON, and a JSON.stringify replacer is slow in JSC, so only arguments that hold bytes use it.
  const call = (op, ...args) => {
    for (let i = 0; i < args.length; i++) {
      const arg = args[i];
      if (arg === null || typeof arg !== 'object' || arg instanceof ArrayBuffer || ArrayBuffer.isView(arg) && !(arg instanceof DataView)) continue;
      if (!nested(args, 0)) return native(0, op, stringify(args));
      const request = pack(args);
      return native(0, op, request.json, request.buffers);
    }
    return native(op, ...args);
  };
  Object.defineProperty(globalThis, '__appsBridgeReceiveBytes', { value: value => pack(__appsBridgeReceive(unpack(value))) });
  // WARNING: a message from the other side of the app arrives in its own task, so its listeners run now.
  Object.defineProperty(globalThis, '__appsBridgeDeliver', { value: value => { __appsBridgeReceive(unpack(value)); __appsBridgeDrain(); } });
  const finalizers = new Set();
  let collected = new WeakRef({});
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
  // WARNING: `heap` orders timers by (at, id). A timer object never changes its `at`: a refresh or an interval run schedules a
  // new object, and a heap entry is live only while `timers` maps its id to it. Timers use the monotonic `clock`, because the wall clock can jump.
  const timers = new Map(), heap = [];
  let timerID = 0, refs = 0, nesting = 0, limit = 0;
  const before = (a, b) => a.at < b.at || (a.at === b.at && a.id < b.id);
  const pop = () => {
    const top = heap[0], last = heap.pop();
    if (heap.length) {
      let i = 0;
      for (let child = 1; child < heap.length; child = 2 * i + 1) {
        if (child + 1 < heap.length && before(heap[child + 1], heap[child])) child++;
        if (!before(heap[child], last)) break;
        heap[i] = heap[child]; i = child;
      }
      heap[i] = last;
    }
    return top;
  };
  // WARNING: cleared timers stay in the heap until they reach the top, so it is rebuilt once they are most of it.
  const compact = () => { if (heap.length > 64 && heap.length > 2 * timers.size) { heap.length = 0; for (const timer of [...timers.values()].sort((a, b) => a.at - b.at || a.id - b.id)) heap.push(timer); } };
  const schedule = timer => {
    timers.set(timer.id, timer);
    let i = heap.push(timer) - 1;
    while (i > 0) { const parent = (i - 1) >> 1; if (!before(timer, heap[parent])) break; heap[i] = heap[parent]; i = parent; }
    heap[i] = timer;
    compact();
  };
  const earliest = () => { while (heap.length && timers.get(heap[0].id) !== heap[0]) pop(); return heap.length ? heap[0].at : Infinity; };
  const remove = id => { const timer = timers.get(id); if (!timer) return; timers.delete(id); if (timer.ref) refs--; compact(); };
  class Timeout {
    #id;
    constructor(id) { this.#id = id; }
    ref() { const timer = timers.get(this.#id); if (timer && !timer.ref) { timer.ref = true; refs++; } return this; }
    unref() { const timer = timers.get(this.#id); if (timer?.ref) { timer.ref = false; refs--; } return this; }
    hasRef() { return timers.get(this.#id)?.ref ?? false; }
    refresh() { const timer = timers.get(this.#id); if (timer) schedule({ ...timer, at: clock() + timer.delay }); return this; }
    close() { clearTimeout(this.#id); return this; }
    [Symbol.toPrimitive]() { return this.#id; }
    [Symbol.dispose]() { clearTimeout(this.#id); }
  }
  const start = (fn, delay, args, interval) => {
    if (typeof fn !== 'function') throw new TypeError('Expected a timer function');
    const id = ++timerID;
    delay = Number(delay);
    delay = delay >= 0 && delay <= 2147483647 ? delay : delay > 0 ? 1 : 0;
    // WARNING: like browsers, pages clamp timers nested more than 5 levels deep to 4 ms.
    if (nodeRole && delay < 1) delay = 1;
    else if (!nodeRole && nesting > 5 && delay < 4) delay = 4;
    refs++;
    schedule({ id, fn, args, delay, at: clock() + delay, ref: true, level: nesting + 1, interval: interval ? Math.max(1, delay) : 0 });
    return nodeRole ? new Timeout(id) : id;
  };
  globalThis.setTimeout = (fn, delay = 0, ...args) => start(fn, delay, args, false);
  globalThis.clearTimeout = id => remove(+id);
  globalThis.setInterval = (fn, delay = 0, ...args) => start(fn, delay, args, true);
  globalThis.clearInterval = clearTimeout;
  // WARNING: internal tasks (IndexedDB events) are not timers, so they skip the timer clamps and reset the nesting level.
  const task = fn => { refs++; schedule({ id: ++timerID, fn, args: [], delay: 0, at: clock(), ref: true, level: 0, interval: 0 }); };
  const resolved = Promise.resolve();
  // WARNING: errors that `fail` throws in main are thrown by the next tick, like a failed timer.
  globalThis.queueMicrotask = fn => {
    if (typeof fn !== 'function') throw new TypeError('The "callback" argument must be of type function');
    resolved.then(() => { try { fn(); } catch (error) { try { fail(error); } catch (error) { moduleErrors.push(error); } } });
  };
  // WARNING: Node's nextTick queue runs after each macrotask and before promise jobs; __appsTick returns -2 after each one, so the engine drains microtasks and calls it again.
  const ticks = [], immediates = new Map();
  let phase = 0, now = 0, checks = [];
  const fail = error => { if (!uncaught(error)) { if (role === 'main') throw error; console.error(error); } };
  const runTicks = () => {
    for (let i = 0; i < ticks.length; i++) { const [fn, args] = ticks[i]; try { fn(...args); } catch (error) { ticks.splice(0, i + 1); i = -1; fail(error); } }
    ticks.length = 0;
  };
  const pollWeb = globalThis.__appsWeb(call);
  delete globalThis.__appsWeb;
  // WARNING: runtime import() loads data: and blob: modules only with jsExecute, and http(s) modules only with httpImport.
  const download = globalThis.fetch;
  globalThis.__appsModuleLoader = (url, resolve, reject) => {
    const name = /^(?:data|blob):/.test(url) ? 'jsExecute' : /^https?:/.test(url) ? 'httpImport' : '';
    if (!name) return reject(new TypeError(`Cannot import ${url}. Only bundled modules, data:, blob: and http(s) URLs can be imported`));
    if (disabled.has(name)) return reject(new TypeError(`Importing ${url.slice(0, url.indexOf(':'))} URLs needs the ${name} feature. Add "${name}" to "features" in package.json`));
    download(url).then(response => response.ok ? response.text() : Promise.reject(new TypeError(`Cannot import ${url}: HTTP ${response.status}`))).then(resolve, reject);
  };
  globalThis.__appsPlatform(call, nodeRole);
  delete globalThis.__appsPlatform;
  let modulesPending = 0;
  const moduleErrors = [];
  globalThis.__appsTrackModule = promise => {
    modulesPending++;
    promise.then(() => { modulesPending--; }, error => { modulesPending--; moduleErrors.push(error); });
  };
  let children, servers, node, pty, browsers, net, nativeShell, uncaught = () => false;
  const workers = globalThis.__appsWorkers(call, !(typeof role === "object" && "renderer" in role), task);
  delete globalThis.__appsWorkers;
  globalThis.__appsBrowserEnvironment(call, task);
  delete globalThis.__appsBrowserEnvironment;
  const hooks = { blob: pollWeb.blob }, bridge = globalThis.__appsBridge(call, role, globalThis.__appsEvents?.EventEmitter, hooks);
  delete globalThis.__appsBridge;
  __appsSyncBridge();
  globalThis.__appsPending = () => modulesPending > 0 || moduleErrors.length > 0 || (children?.pending() ?? false) || (node?.poll.pending() ?? false) || (pty?.poll.pending() ?? false) || bridge.poll.pending() || workers.pending() || (net?.pending() ?? false);
  // WARNING: one call runs one macrotask: pending ticks, one due timer, the I/O polls or one immediate (timers, poll and check phases, like Node).
  globalThis.__appsTick = () => {
    if (moduleErrors.length) throw moduleErrors.shift();
    if (ticks.length) { runTicks(); return -2; }
    if (phase === 0) {
      phase = 1; now = clock(); limit = timerID;
    }
    // WARNING: timers that start in this phase have larger ids and are never earlier than `now`, so they wait for the next phase.
    if (phase === 1 && earliest() <= now && heap[0].id <= limit) {
      const timer = pop();
      nesting = timer.level;
      if (timer.interval) { const interval = !nodeRole && timer.level > 5 && timer.interval < 4 ? 4 : timer.interval; schedule({ ...timer, interval, level: timer.level + 1, at: clock() + interval }); }
      else remove(timer.id);
      try { timer.fn(...timer.args); } catch (error) { fail(error); } finally { nesting = 0; }
      runTicks();
      return -2;
    }
    if (phase === 1) {
      phase = 2;
      try {
        // WARNING: WeakRef targets only die in a GC, so sweep only after the canary is collected.
        if (!collected.deref()) { collected = new WeakRef({}); for (const registry of finalizers) for (const [key, entry] of registry.live) if (!entry.ref.deref()) { registry.live.delete(key); registry.release(entry.value); } }
        globalThis.__appsDomSweep?.();
        globalThis.__appsDecoded?.();
        globalThis.__appsGPUPoll?.();
        globalThis.__appsMediaPoll?.();
        bridge.poll();
        pollWeb();
        children?.poll();
        servers?.poll();
        node?.poll();
        pty?.poll();
        browsers?.poll();
        workers.poll();
        net?.poll();
        nativeShell?.poll?.();
      } catch (error) { fail(error); }
      runTicks();
      checks = [...immediates];
      return -2;
    }
    while (checks.length) {
      const [immediate, [fn, args]] = checks.shift();
      if (!immediates.delete(immediate)) continue;
      try { fn(...args); } catch (error) { fail(error); }
      runTicks();
      return -2;
    }
    phase = 0;
    return __appsDelay();
  };
  globalThis.__appsDelay = () => {
    if (ticks.length || immediates.size || bridge.poll.queued()) return 0;
    const at = earliest(), delay = at === Infinity ? -1 : Math.max(0, at - clock());
    // WARNING: native producers wake the loop when they queue events and bridge replies arrive in a call, so only module errors poll here.
    return moduleErrors.length ? (delay < 0 ? 4 : Math.min(4, delay)) : browsers?.pending() ? (delay < 0 ? 16 : Math.min(16, delay)) : delay;
  };
  globalThis.__appsAlive = () => {
    return refs > 0 || ticks.length > 0 || immediates.size > 0 || pollWeb.pending() || !!servers?.pending() || __appsPending();
  };
  if (typeof role === 'object' && 'renderer' in role) {
    globalThis.__appsDOM = call;
    globalThis.__appsEventState = pollWeb.eventState;
    globalThis.__appsFiles = hooks.files;
    const executed = (id, data) => call('dom.executed', id, stringify(data));
    Object.defineProperty(globalThis, '__appsExecuted', { value: (id, value, failed) => (failed ? Promise.reject(value) : Promise.resolve(value)).then(value => executed(id, { value })).catch(error => executed(id, { error: String(error?.message ?? error) })) });
    if (!disabled.has('webgpu')) globalThis.__appsGPU = call;
    else { globalThis.navigator.gpu = feature('webgpu'); globalThis.GPUCanvasContext = feature('webgpu'); }
    globalThis.__appsMedia = { video: !disabled.has('video'), audio: !disabled.has('audio') };
    return;
  }
  const info = call('info');
  if (info.rustMain && typeof role === 'object' && 'preload' in role) {
    bridge.ipcRenderer.invoke = (channel, ...args) => new Promise(resolve => resolve(bridge.rpc('main', { kind: 'rust', channel, args: stringify(args) })));
  }
  globalThis.global = globalThis;
  // WARNING: V8 call sites are parsed from JSC's stack text, so getThis(), getFunction() and getTypeName() know nothing.
  const capture = Error.captureStackTrace;
  if (capture) Error.captureStackTrace = function captureStackTrace(object, constructor) {
    capture(object, constructor ?? captureStackTrace);
    const prepare = Error.prepareStackTrace;
    if (typeof prepare !== 'function') return;
    const sites = String(object.stack).split('\n').filter(Boolean).map(line => {
      const [, name = '', file, row, column] = /^(?:(.*?)@)?(.*?)(?::(\d+):(\d+))?$/.exec(line), native = file === '[native code]', top = !name || / code$/.test(name);
      return { getThis: () => undefined, getTypeName: () => null, getFunction: () => undefined, getFunctionName: () => top ? null : name, getMethodName: () => null, getFileName: () => native ? undefined : file, getScriptNameOrSourceURL: () => native ? undefined : file,
        getLineNumber: () => row ? +row : null, getColumnNumber: () => column ? +column : null, getEvalOrigin: () => undefined, isToplevel: () => top, isEval: () => name === 'eval code', isNative: () => native, isConstructor: () => false, isAsync: () => false, isPromiseAll: () => false, getPromiseIndex: () => null, toString: () => line };
    });
    Object.defineProperty(object, 'stack', { value: prepare(object, sites), writable: true, configurable: true });
  };
  const callable = (fn, name) => { if (typeof fn !== 'function') throw Object.assign(new TypeError(`The "${name}" argument must be of type function. Received ${fn === null ? 'null' : typeof fn}`), { code: 'ERR_INVALID_ARG_TYPE' }); };
  class Immediate {
    #ref = true;
    ref() { this.#ref = true; return this; }
    unref() { this.#ref = false; return this; }
    hasRef() { return this.#ref; }
    [Symbol.dispose]() { clearImmediate(this); }
  }
  globalThis.setImmediate = (fn, ...args) => { callable(fn, 'callback'); const immediate = new Immediate(); immediates.set(immediate, [fn, args]); return immediate; };
  globalThis.clearImmediate = immediate => { immediates.delete(immediate); };
  const ffi = globalThis.__appsFFI(globalThis.__appsFFINative);
  delete globalThis.__appsFFI;
  delete globalThis.__appsFFINative;
  const sqlite = globalThis.__appsSQLite(call);
  delete globalThis.__appsSQLite;
  const events = globalThis.__appsEvents;
  delete globalThis.__appsEvents;
  globalThis.__appsEventTargets(pollWeb.listeners);
  delete globalThis.__appsEventTargets;
  servers = globalThis.__appsServe(call);
  delete globalThis.__appsServe;
  node = globalThis.__appsNode(call, events.EventEmitter, inspector, info);
  delete globalThis.__appsNode;
  const stream = globalThis.__appsNodeStreams(events.EventEmitter, node.Buffer, node.string_decoder.StringDecoder);
  delete globalThis.__appsNodeStreams;
  children = globalThis.__appsChildProcess(call, events.EventEmitter, stream);
  delete globalThis.__appsChildProcess;
  const io = globalThis.__appsNodeIO({ call, sync: node.sync, asyncCall: node.asyncCall, Buffer: node.Buffer, stream, EventEmitter: events.EventEmitter, StringDecoder: node.string_decoder.StringDecoder, webcrypto: globalThis.__appsNodeCrypto.webcrypto, inspect });
  delete globalThis.__appsNodeIO;
  node.util.types.isKeyObject = value => value instanceof io.crypto.KeyObject;
  net = globalThis.__appsNet(call, events.EventEmitter, stream, node.Buffer);
  delete globalThis.__appsNet;
  const http = globalThis.__appsHTTP(servers.serve, stream, events.EventEmitter, net);
  delete globalThis.__appsHTTP;
  class ClientRequest extends http.http.ClientRequest {
    constructor(options) {
      options = typeof options === 'string' ? { url: options } : { ...options };
      if (!['follow', 'error'].includes(options.redirect ??= 'follow')) throw new TypeError(`Unsupported redirect mode: ${options.redirect}`);
      super(options.url ?? `${options.protocol ?? 'http:'}//${options.host ?? (options.hostname ?? 'localhost') + (options.port ? ':' + options.port : '')}${options.path ?? '/'}`, { method: options.method, headers: options.headers });
      Object.assign(this, { _redirect: options.redirect, chunkedEncoding: false });
    }
    followRedirect() {}
    getUploadProgress() { const total = this._chunks.reduce((size, chunk) => size + chunk.length, 0); return { active: this.headersSent && !this.res, started: this.headersSent, current: this.res ? total : 0, total }; }
  }
  const isOnline = () => info.platform === 'win32' || Object.values(node.os.networkInterfaces()).flat().some(item => !item.internal && !/^(169\.254\.|fe80:)/i.test(item.address));
  const electron = globalThis.__appsElectron(call, events.EventEmitter, () => ready);
  delete globalThis.__appsElectron;
  const electronNet = Object.freeze({ fetch: async (input, init) => { const handler = electron.handled(input); return handler ? handler(new Request(input, init)) : fetch(input, init); }, request: options => new ClientRequest(options), isOnline, get online() { return isOnline(); } });
  pty = globalThis.__appsPty(call);
  delete globalThis.__appsPty;
  // WARNING: like Node.js, main has `fetch` but no `XMLHttpRequest`, and libraries look for it to detect a browser.
  for (const name of ['FileReader', 'ProgressEvent', 'XMLHttpRequest', 'XMLHttpRequestEventTarget', 'XMLHttpRequestUpload']) delete globalThis[name];
  browsers = globalThis.__appsBrowser(call, events.EventEmitter, () => ready, contents => app.emit('web-contents-created', event(), contents), electron.nativeImage);
  delete globalThis.__appsBrowser;
  nativeShell = globalThis.__appsNativeShell(call, events.EventEmitter, role !== 'main');
  delete globalThis.__appsNativeShell;
  // WARNING: process.version reports the Node.js API level that the node: modules follow, and versions.electron the Electron release that uses it.
  const nodeVersion = '24.0.0', electronVersion = '40.0.0';
  let exiting = false;
  const finish = code => {
    if (code !== undefined) process.exitCode = code;
    if (!exiting) { exiting = true; process.emit('exit', Number(process.exitCode ?? 0) | 0); }
    return Number(process.exitCode ?? 0) | 0;
  };
  const quitApp = code => call('quit', finish(code));
  // WARNING: a worker keeps running until the watchdog sees its stop flag, so the loop keeps code after exit() from running.
  const exit = code => { call('process.exit', finish(code)); for (;;); };
  const env = new Proxy(info.env, {
    set: (target, name, value) => { target[name] = String(value); return true; },
    defineProperty: (target, name, descriptor) => {
      if (descriptor.configurable !== true || descriptor.writable !== true || descriptor.enumerable !== true || !('value' in descriptor)) throw Object.assign(new TypeError("'process.env' only accepts a configurable, writable, and enumerable data descriptor"), { code: 'ERR_INVALID_OBJECT_DEFINE_PROPERTY' });
      target[name] = String(descriptor.value);
      return true;
    },
  });
  const memoryUsage = () => ({ rss: memoryUsage.rss(), heapTotal: 0, heapUsed: 0, external: 0, arrayBuffers: 0 });
  memoryUsage.rss = () => call('node.process.memory');
  const process = globalThis.process = Object.assign(new events.EventEmitter(), {
    platform: info.platform, arch: info.arch, pid: info.pid, title: info.appName, type: info.node ? undefined : role === 'main' ? 'browser' : 'renderer',
    version: 'v' + nodeVersion, versions: Object.freeze({ node: nodeVersion, electron: electronVersion, apps: info.version, javascriptcore: info.javascriptcore, napi: '8' }), release: Object.freeze({ name: 'node' }),
    argv: [info.execPath ?? info.argv[0], ...info.argv.slice(1)], argv0: info.argv[0], execArgv: [], env, execPath: info.execPath, exitCode: undefined,
    cwd: () => call('cwd'), chdir: path => call('chdir', path),
    nextTick: (fn, ...args) => { callable(fn, 'callback'); ticks.push([fn, args]); },
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
  Object.assign(process, {
    resourceUsage: () => call('node.process.resourceUsage'),
    abort() { call('node.process.abort'); },
    constrainedMemory: () => 0,
    availableMemory: () => node.os.freemem(),
    getActiveResourcesInfo: () => [],
    features: Object.freeze({ inspector: false, debug: false, uv: true, ipv6: true, tls_alpn: true, tls_sni: true, tls_ocsp: true, tls: true, cached_builtins: true, require_module: true, typescript: false }),
    config: Object.freeze({ target_defaults: Object.freeze({}), variables: Object.freeze({}) }),
    allowedNodeEnvironmentFlags: new Set(),
    debugPort: 9229,
    report: Object.freeze({ compact: false, directory: '', filename: '', getReport: () => ({}), reportOnFatalError: false, reportOnSignal: false, reportOnUncaughtException: false, signal: 'SIGUSR2', writeReport: () => '' }),
    sourceMapsEnabled: false,
    setSourceMapsEnabled() {},
    hasUncaughtExceptionCaptureCallback: () => false,
    setUncaughtExceptionCaptureCallback() {},
    loadEnvFile() {},
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
  hooks.report = error => { if (!uncaught(error)) console.error(error); };
  addEventListener('unhandledrejection', event => { if (!process.listenerCount('unhandledRejection')) return; event.preventDefault(); process.emit('unhandledRejection', event.reason, event.promise); });
  const output = fd => {
    const tty = node.tty.isatty(fd), writable = new stream.Writable({ write(data, _encoding, done) { call('process.write', fd, data); done(); } });
    // WARNING: Writes stay synchronous so they keep their order with console output.
    writable.write = (data, encoding, callback) => { if (typeof encoding === 'function') { callback = encoding; encoding = undefined; } call('process.write', fd, typeof data === 'string' && !encoding ? data : Buffer.from(data, encoding)); if (callback) queueMicrotask(callback); return true; };
    Object.defineProperties(writable, { fd: { value: fd, enumerable: true }, isTTY: { value: tty || undefined, enumerable: true } });
    return tty ? node.terminal(writable, stream) : writable;
  };
  process.stdout = output(1);
  process.stderr = output(2);
  const fs = node.fs;
  // WARNING: reading it holds a thread of the file pool until input comes, so it opens on first use.
  let stdin;
  Object.defineProperty(process, 'stdin', { enumerable: true, configurable: true, get() {
    if (stdin) return stdin;
    stdin = fs.createReadStream('', { fd: 0, autoClose: false });
    return Object.assign(stdin, { isTTY: node.tty.isatty(0) || undefined, setRawMode() { return this; }, ref() { return this; }, unref() { return this; } });
  } });
  const fileStream = (self, file, o, flags) => {
    const handle = typeof o.fd === 'object' && o.fd !== null ? o.fd : null;
    if (o.start !== undefined && o.end !== undefined && o.start > o.end) throw Object.assign(new RangeError(`The value of "start" is out of range. It must be <= "end" (here: ${o.end}). Received ${o.start}`), { code: 'ERR_OUT_OF_RANGE' });
    Object.assign(self, { path: file instanceof URL ? node.url.fileURLToPath(file) : file ?? undefined, fd: handle ? handle.fd : o.fd ?? null, flags: o.flags ?? flags, mode: o.mode ?? 0o666, start: o.start, autoClose: o.autoClose !== false, pos: o.start });
    self.opening = self.fd !== null ? Promise.resolve(self.fd) : new Promise((resolve, reject) => fs.open(self.path, self.flags, self.mode, (error, fd) => error ? reject(error) : resolve(fd)));
    if (self.fd === null) self.opening.then(fd => { self.fd = fd; self.emit('open', fd); self.emit('ready'); }, error => self.destroy(error));
    self._destroy = (error, done) => self.opening.then(fd => self.autoClose && new Promise(resolve => { self.fd = null; if (handle) handle.close().then(resolve, resolve); else fs.close(fd, resolve); }), () => {}).then(() => done(error));
  };
  class ReadStream extends stream.Readable {
    get pending() { return this.fd === null; }
    constructor(file, o) {
      o = typeof o === 'string' ? { encoding: o } : o ?? {};
      super({ highWaterMark: o.highWaterMark ?? 65536, encoding: o.encoding, emitClose: o.emitClose, signal: o.signal });
      this.bytesRead = 0; this.end = o.end ?? Infinity;
      fileStream(this, file, { ...o, start: o.start ?? (o.end === undefined ? undefined : 0) }, 'r');
    }
    _read(size) {
      this.opening.then(fd => {
        const length = Math.min(size, this.end - (this.pos ?? 0) + 1);
        if (length <= 0) return this.push(null);
        fs.read(fd, Buffer.allocUnsafe(length), 0, length, this.pos ?? null, (error, bytesRead, buffer) => {
          if (error) return this.destroy(error);
          if (this.pos !== undefined) this.pos += bytesRead;
          this.bytesRead += bytesRead;
          this.push(bytesRead ? buffer.subarray(0, bytesRead) : null);
        });
      }, () => {});
    }
    close(callback) { if (callback) { if (this.closed) queueMicrotask(callback); else this.once('close', callback); } this.destroy(); }
  }
  class WriteStream extends stream.Writable {
    get pending() { return this.fd === null; }
    constructor(file, o) {
      o = typeof o === 'string' ? { encoding: o } : o ?? {};
      super({ highWaterMark: o.highWaterMark, defaultEncoding: o.encoding, emitClose: o.emitClose, signal: o.signal });
      this.bytesWritten = 0; this.flush = !!o.flush;
      fileStream(this, file, o, 'w');
    }
    _write(data, _encoding, done) {
      this.opening.then(fd => {
        const step = at => fs.write(fd, data, at, data.length - at, this.pos ?? null, (error, written) => {
          if (error) return done(error);
          this.bytesWritten += written;
          if (this.pos !== undefined) this.pos += written;
          if (at + written < data.length) step(at + written); else done();
        });
        step(0);
      }, done);
    }
    _final(done) { this.opening.then(fd => this.flush ? fs.fsync(fd, done) : done(), done); }
    close(callback) { if (callback) { if (this.closed) queueMicrotask(callback); else this.once('close', callback); } if (this.writable) this.end(); else this.destroy(); }
    destroySoon() { this.end(); }
  }
  Object.assign(fs, { ReadStream, WriteStream, createReadStream: (file, o) => new ReadStream(file, o), createWriteStream: (file, o) => new WriteStream(file, o) });
  const { path, os } = node;
  let ready = false, nextWindow = 0, resolveReady;
  const readyPromise = new Promise(resolve => { resolveReady = resolve; });
  const windows = new Map(), captures = new Map(), states = new WeakMap(), pages = new WeakMap(), opened = new Map();
  let nextCapture = 0, nextLoad = 0, nextCSS = 0;
  const destroyed = () => Error('Object has been destroyed');
  const state = win => { if (win.isDestroyed()) throw destroyed(); return states.get(win); };
  const loadError = (code, description, url) => Object.assign(new Error(`${description} (${code}) loading '${url}'`), { code: description, errno: code, url });
  const pending = (win, op, ...args) => new Promise((resolve, reject) => { const id = ++nextCapture; captures.set(id, { resolve, reject, win, op }); call('window', op, win, id, ...args); });
  class WebContents extends events.EventEmitter {
    constructor(win) { super(); Object.defineProperty(this, 'id', { value: browsers.reserve(), enumerable: true }); pages.set(this, { win, url: '', title: '', loading: false, load: null, html: null, zoom: [1, 0] }); }
    #page() { const page = pages.get(this); if (!windows.has(page.win)) throw destroyed(); return page; }
    _load(op, target, url) {
      const page = this.#page(), request = ++nextLoad;
      call('window', op, page.win, target, request);
      page.load?.reject(loadError(-3, 'ERR_ABORTED', page.load.url));
      Object.assign(page, { loading: true, html: op === 'load' ? target : page.html });
      return new Promise((resolve, reject) => { page.load = { request, url, resolve, reject }; });
    }
    _event(name, data) {
      const page = pages.get(this), load = page.load?.request === data?.request ? page.load : null;
      if (name === 'title') { page.title = data; return; }
      if (load) page.load = null;
      page.loading = false;
      if (name === 'failed') {
        this.emit('did-fail-load', event(), data.code, data.description, data.url, true, info.pid, 1);
        this.emit('did-stop-loading');
        return load?.reject(loadError(data.code, data.description, data.url));
      }
      page.url = data.url;
      this.emit('did-start-loading');
      this.emit('did-navigate', event(), data.url, -1, '');
      this.emit('dom-ready', event());
      this.emit('did-finish-load', event());
      this.emit('did-stop-loading');
      load?.resolve();
    }
    loadFile(file, options = {}) {
      let target = String(file);
      const local = /^(apps|file):/.test(target);
      const url = local ? new URL(target) : node.url.pathToFileURL(path.resolve(info.appPath, target));
      if (options.query || options.search !== undefined || options.hash !== undefined) {
        if (options.query) url.search = new URLSearchParams(options.query).toString();
        if (options.search !== undefined) url.search = options.search;
        if (options.hash !== undefined) url.hash = options.hash;
        target = url.href;
      }
      return this._load('loadFile', target, url.href);
    }
    // WARNING: pages that ship with the app are bundled, so only these URLs load here. Use WebContentsView or `shell.openExternal()` for websites.
    loadURL(url) {
      url = String(url);
      if (/^(apps|file):/i.test(url)) return this.loadFile(url);
      const data = /^data:text\/html(;[^,]*)?,(.*)$/is.exec(url);
      if (data) return this._load('load', /;base64/i.test(data[1] ?? '') ? node.Buffer.from(data[2], 'base64').toString() : decodeURIComponent(data[2]), url);
      const handler = electron.handled(url);
      if (handler) return Promise.resolve(handler(new Request(url))).then(response => response.text()).then(html => this._load('load', html, url));
      const failure = loadError(-301, 'ERR_DISALLOWED_URL_SCHEME', url);
      this.emit('did-fail-load', event(), failure.errno, failure.code, url, true, info.pid, 1);
      return Promise.reject(failure);
    }
    // WARNING: main runs a script for the keys of a window only while it listens to its `before-input-event`.
    #input = false;
    #listen() { const on = this.listenerCount('before-input-event') > 0; if (on !== this.#input && !this.isDestroyed()) { this.#input = on; call('window', 'input', pages.get(this).win, on); } return this; }
    static { for (const method of ['on', 'addListener', 'prependListener', 'once', 'prependOnceListener', 'removeListener', 'removeAllListeners']) this.prototype[method] = function(...args) { events.EventEmitter.prototype[method].apply(this, args); return this.#listen(); }; }
    getType() { return 'window'; }
    get session() { return browsers.session.defaultSession; }
    get mainFrame() { return frameOf(this); }
    get focusedFrame() { return frameOf(this); }
    get navigationHistory() { return navigationHistory(this); }
    get hostWebContents() { return null; }
    get devToolsWebContents() { return null; }
    get debugger() { return debuggerOf(this); }
    ipc = Object.assign(new events.EventEmitter(), { handle() {}, handleOnce() {}, removeHandler() {} });
    isFocused() { return !this.isDestroyed() && states.get(windows.get(pages.get(this).win)).focused; }
    isLoadingMainFrame() { return this.isLoading(); }
    getUserAgent() { return `Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) ${info.appName}/${info.appVersion}`; }
    setUserAgent() {}
    setBackgroundThrottling() {}
    getBackgroundThrottling() { return false; }
    setIgnoreMenuShortcuts() {}
    setVisualZoomLevelLimits() { return Promise.resolve(); }
    setAudioMuted() {}
    isAudioMuted() { return false; }
    isCurrentlyAudible() { return false; }
    isDevToolsOpened() { return false; }
    closeDevTools() {}
    isCrashed() { return false; }
    stop() {}
    startDrag() {}
    sendInputEvent() {}
    replaceMisspelling(text) { this.replace(text); }
    copyImageAt() {}
    executeJavaScriptInIsolatedWorld(_world, scripts) { return Promise.all(scripts.map(({ code }) => this.executeJavaScript(code))).then(results => results.at(-1)); }
    getFrameRate() { return 120; }
    setFrameRate() {}
    getProcessMemoryInfo() { return Promise.resolve({ residentSet: process.memoryUsage.rss() >> 10, private: 0, shared: 0 }); }
    reload() { const page = this.#page(), html = page.url === 'apps://app/'; if (page.url) this._load(html ? 'load' : 'loadFile', html ? page.html : page.url, page.url).catch(() => {}); }
    reloadIgnoringCache() { this.reload(); }
    send(channel, ...args) { bridge.send(this.#page().win, channel, ...args); }
    executeJavaScript(code) { return pending(this.#page().win, 'execute', String(code)); }
    insertCSS(css, { cssOrigin = 'author' } = {}) { const key = `apps-css-${++nextCSS}`; call('window', 'insertCSS', this.#page().win, key, String(css), cssOrigin === 'user'); return Promise.resolve(key); }
    removeInsertedCSS(key) { call('window', 'removeInsertedCSS', this.#page().win, String(key)); return Promise.resolve(); }
    setWindowOpenHandler(handler) { if (handler !== null && typeof handler !== 'function') throw new TypeError('Expected a window open handler'); this.#page().open = handler; }
    #edit(command, text = null) { call('window', 'edit', this.#page().win, command, text); }
    undo() { this.#edit('undo'); } redo() { this.#edit('redo'); } cut() { this.#edit('cut'); } copy() { this.#edit('copy'); } paste() { this.#edit('paste'); } pasteAndMatchStyle() { this.#edit('pasteAndMatchStyle'); }
    delete() { this.#edit('delete'); } selectAll() { this.#edit('selectAll'); } unselect() { this.#edit('unselect'); } replace(text) { this.#edit('replace', String(text)); }
    insertText(text) { this.#edit('insertText', String(text)); return Promise.resolve(); }
    capturePage(rect) { return pending(this.#page().win, 'capture', rect ?? null); }
    // WARNING: like Chromium, the page draws at most 5x and at least 0.25x, while the getters keep the value that was set.
    #zoom(factor, level) { const page = this.#page(); page.zoom = [factor, level]; call('window', 'zoom', page.win, Math.min(Math.max(factor, 0.25), 5)); }
    setZoomFactor(factor) { if (typeof factor !== 'number' || !(factor > 0)) throw new Error("'zoomFactor' must be a double greater than 0.0"); this.#zoom(factor, Math.log(factor) / Math.log(1.2)); }
    getZoomFactor() { return pages.get(this).zoom[0]; }
    setZoomLevel(level) { if (typeof level !== 'number' || !Number.isFinite(level)) throw new TypeError('Expected a number for zoomLevel'); this.#zoom(1.2 ** level, level); }
    getZoomLevel() { return pages.get(this).zoom[1]; }
    get zoomFactor() { return this.getZoomFactor(); }
    set zoomFactor(factor) { this.setZoomFactor(factor); }
    get zoomLevel() { return this.getZoomLevel(); }
    set zoomLevel(level) { this.setZoomLevel(level); }
    openDevTools() { call('window', 'devtools', this.#page().win); }
    toggleDevTools() { this.openDevTools(); }
    focus() { BrowserWindow.fromId(this.#page().win).focus(); }
    getURL() { return pages.get(this).url; }
    getTitle() { return pages.get(this).title; }
    isLoading() { return pages.get(this).loading; }
    isDestroyed() { return !windows.has(pages.get(this).win); }
    getOSProcessId() { return info.pid; }
    getProcessId() { return info.pid; }
  }
  // WARNING: a page has one frame here, and `<iframe>` contents are system web views that main can't reach.
  const frames = new WeakMap();
  const frameOf = contents => {
    let frame = frames.get(contents);
    if (!frame) frames.set(contents, frame = { get url() { return contents.getURL(); }, get name() { return ''; }, get top() { return frame; }, get parent() { return null; }, frames: [], framesInSubtree: [], get origin() { try { return new URL(contents.getURL()).origin; } catch { return ''; } }, processId: info.pid, routingId: 1, frameTreeNodeId: 1, visibilityState: 'visible', detached: false, ipc: contents.ipc,
      executeJavaScript: code => contents.executeJavaScript(code), send: (channel, ...args) => contents.send(channel, ...args), postMessage: (channel, message) => contents.send(channel, message), isDestroyed: () => contents.isDestroyed(), reload: () => { contents.reload(); return true; }, collectJavaScriptCallStack: async () => '' });
    return frame;
  };
  const histories = new WeakMap();
  const navigationHistory = contents => {
    let history = histories.get(contents);
    if (!history) histories.set(contents, history = { canGoBack: () => false, canGoForward: () => false, canGoToOffset: () => false, goBack() { contents.executeJavaScript('history.back()'); }, goForward() { contents.executeJavaScript('history.forward()'); }, goToOffset() {}, goToIndex() {}, clear() {}, getActiveIndex: () => 0, length: () => 1, getEntryAtIndex: () => ({ url: contents.getURL(), title: contents.getTitle() }), getAllEntries: () => [{ url: contents.getURL(), title: contents.getTitle() }] });
    return history;
  };
  // WARNING: the renderer is not Chromium, so there is no Chrome DevTools Protocol to attach to.
  const debuggers = new WeakMap();
  const debuggerOf = contents => {
    let item = debuggers.get(contents);
    if (!item) debuggers.set(contents, item = Object.assign(new events.EventEmitter(), { attach() { throw new Error('Chrome DevTools Protocol is not supported'); }, detach() {}, isAttached: () => false, sendCommand: () => Promise.reject(new Error('Chrome DevTools Protocol is not supported')) }));
    return item;
  };
  const webContents = Object.freeze({
    getAllWebContents: () => [...[...windows.values()].map(win => win.webContents), ...browsers.all()],
    fromId: id => webContents.getAllWebContents().find(contents => contents.id === id),
    getFocusedWebContents: () => BrowserWindow.getFocusedWindow()?.webContents ?? null,
  });
  const closed = win => {
    const closing = quitting;
    windows.delete(win.id);
    opened.delete(win.id);
    for (const child of win.contentView.children) win.contentView.removeChildView(child);
    const page = pages.get(win.webContents);
    page.load?.reject(loadError(-3, 'ERR_ABORTED', page.load.url));
    for (const [id, request] of captures) if (request.win === win.id) { captures.delete(id); request.reject(destroyed()); }
    // WARNING: a throwing listener must not stop the app from quitting after its last window.
    for (const [target, name] of [[win.webContents, 'destroyed'], [win, 'closed'], [app, 'window-close']]) try { target.emit(name); } catch (error) { hooks.report(error); }
    if (windows.size) return;
    if (closing) willQuit();
    else if (quitting) return;
    else if (app.listenerCount('window-all-closed') || app.listenerCount('window-close')) app.emit('window-all-closed');
    else quit();
  };
  hooks.contents = id => windows.get(id)?.webContents ?? null;
  // WARNING: results go to the page as data for a fixed page function, never as code.
  const deliver = (win, name, ...args) => { if (!win.isDestroyed()) call('window', 'deliver', win.id, name, args); };
  // WARNING: pages call this while their `window.open()` waits, so it returns JSON: the new window's id, or whether a window it opened is closed.
  // A `<webview>` is a WebContentsView that main keeps over the element, and its events go to the element.
  const webviews = new Map();
  const webview = (win, op, guestId, data) => {
    const entry = webviews.get(guestId), contents = win.webContents;
    if (op === 'attach') {
      const preferences = { partition: data.partition || undefined }, attach = event();
      contents.emit('will-attach-webview', attach, preferences, { ...data });
      if (attach.defaultPrevented) return null;
      const view = new browsers.WebContentsView({ webPreferences: { partition: data.partition || undefined } }), guest = view.webContents;
      guest.hostWebContents = contents;
      win.contentView.addChildView(view);
      const emit = guest.emit;
      guest.emit = (name, ...args) => {
        if (webviewEvents[name]) deliver(win, 'webviewEvent', guest.id, name, webviewEvents[name](...args));
        return emit.call(guest, name, ...args);
      };
      const drop = () => { if (webviews.delete(guest.id) && !guest.isDestroyed()) guest.close(); };
      webviews.set(guest.id, { view, guest, drop, win });
      contents.once('did-start-loading', drop);
      win.once('closed', drop);
      contents.emit('did-attach-webview', event(), guest);
      if (data.src) guest.loadURL(data.src).catch(() => {});
      return guest.id;
    }
    if (entry?.win !== win) return null;
    const { view, guest, drop } = entry;
    if (op === 'bounds') { view.setBounds(data.bounds); view.setVisible(data.visible); return null; }
    if (op === 'detach') { contents.off('did-start-loading', drop); win.off('closed', drop); win.contentView.removeChildView(view); drop(); return null; }
    if (op === 'call') {
      const result = guest[data.method](...data.args);
      if (typeof result?.then !== 'function') return result ?? null;
      const reply = (value, error) => deliver(win, 'webviewReply', data.reply ?? null, value ?? null, error ?? null);
      result.then(value => reply(value), error => reply(null, String(error?.message ?? error)));
    }
    return null;
  };
  const webviewEvents = {
    'did-start-loading': () => ({}), 'did-stop-loading': () => ({}), 'dom-ready': () => ({}), 'did-finish-load': () => ({}),
    'did-navigate': (_event, url) => ({ url }), 'did-navigate-in-page': (_event, url, isMainFrame) => ({ url, isMainFrame }), 'page-title-updated': (_event, title) => ({ title }), 'page-favicon-updated': (_event, favicons) => ({ favicons }),
    'did-fail-load': (_event, errorCode, errorDescription, validatedURL) => ({ errorCode, errorDescription, validatedURL, isMainFrame: true }),
    'console-message': (_event, level, message, line, sourceId) => ({ level, message, line, sourceId }), 'render-process-gone': (_event, details) => ({ details }), 'destroyed': () => ({}),
  };
  globalThis.__appsWindowOpen = (id, [kind, url, frameName, features, disposition]) => {
    const opener = windows.get(id);
    if (!opener) return 'null';
    if (kind === 'webview') { try { return JSON.stringify(webview(opener, url, frameName, features) ?? null); } catch (error) { hooks.report(error); return 'null'; } }
    if (kind !== 'open') { const child = opened.get(url) === id ? windows.get(url) : null; if (kind === 'close') child?.close(); return JSON.stringify(!child); }
    const page = pages.get(opener.webContents), details = { url, frameName, features, disposition, referrer: { url: page.url, policy: 'strict-origin-when-cross-origin' }, postBody: undefined };
    try {
      const result = page.open ? page.open(details) : { action: 'allow' };
      // WARNING: BrowserWindow loads only bundled pages and local files, so other URLs are denied.
      if (result?.action !== 'allow' || !/^(apps:\/\/\/|file:)/i.test(url)) return 'null';
      const options = {};
      for (const [, key, value] of features.matchAll(/(\w+)\s*=\s*([\d.]+)/g)) { const name = { width: 'width', height: 'height', left: 'x', top: 'y' }[key.toLowerCase()]; if (name) options[name] = Number(value); }
      Object.assign(options, result.overrideBrowserWindowOptions);
      const win = new BrowserWindow(options);
      opened.set(win.id, id);
      win.loadFile(url).catch(() => {});
      opener.webContents.emit('did-create-window', win, { ...details, options });
      return String(win.id);
    } catch (error) { hooks.report(error); return 'null'; }
  };
  const pickFiles = async (win, { id, multiple, directory, accept }) => {
    let files = null;
    try {
      const images = ['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'heic', 'bmp', 'tiff'];
      const extensions = accept.split(',').map(item => item.trim().toLowerCase()).flatMap(item => item === 'image/*' ? images : item.startsWith('.') ? [item.slice(1)] : [null]);
      const { canceled, filePaths } = await globalThis.__apps.dialog.showOpenDialog(win, {
        properties: [directory ? 'openDirectory' : 'openFile', ...(multiple ? ['multiSelections'] : [])],
        ...(accept && !extensions.includes(null) ? { filters: [{ name: 'Files', extensions }] } : {}),
      });
      // WARNING: as with dropped files, more than 64 MB is left out, because the bytes cross to the page as a string.
      if (!canceled) files = filePaths.flatMap(path => {
        const stat = __apps.fs.statSync(path);
        return stat.isFile() && stat.size <= 64 * 1024 * 1024 ? [{ name: path.split('/').at(-1), lastModified: stat.mtimeMs, data: __apps.fs.readFileSync(path).toString('base64') }] : [];
      });
    } catch (error) { hooks.report(error); }
    deliver(win, 'filesPicked', id, files);
  };
  const num = (value, name) => { if (typeof value !== 'number' || !Number.isFinite(value)) throw new TypeError(`Expected a number for ${name}`); return value; };
  const int = (value, name) => Math.round(num(value, name));
  const materials = { 'appearance-based': 0, titlebar: 3, selection: 4, menu: 5, popover: 6, sidebar: 7, header: 10, sheet: 11, window: 12, hud: 13, 'fullscreen-ui': 15, tooltip: 17, content: 18, 'under-window': 21, 'under-page': 22 };
  const material = type => { if (!type) return null; if (!Object.hasOwn(materials, type)) throw new TypeError(`Invalid vibrancy type: ${type}`); return materials[type]; };
  const effectStates = ['followWindow', 'active', 'inactive'];
  // WARNING: a parent destroys or keeps its children natively, so they leave it before it closes.
  const orphan = win => { for (const child of win.getChildWindows()) { states.get(child).parent = 0; call('window', 'parent', child.id, null); } };
  // WARNING: Electron reads 4 and 8 digit hex colors as ARGB, CSS reads them as RGBA.
  const argb = color => /^#([\da-f]{4}|[\da-f]{8})$/i.test(color) ? '#' + color.slice(color.length === 5 ? 2 : 3) + color.slice(1, color.length === 5 ? 2 : 3) : color;
  const resize = (win, next, content) => {
    const windowState = state(win), { bounds, content: inner } = windowState, base = content ? inner : bounds, target = { ...base };
    for (const key of ['x', 'y', 'width', 'height']) if (next?.[key] !== undefined) target[key] = int(next[key], key);
    if (info.platform !== 'darwin' && (target.x !== base.x || target.y !== base.y)) throw Error('Moving a window is only available on macOS');
    const [dx, dy, dw, dh] = [inner.x - bounds.x, inner.y - bounds.y, bounds.width - inner.width, bounds.height - inner.height];
    if (content) Object.assign(target, { x: target.x - dx, y: target.y - dy, width: target.width + dw, height: target.height + dh });
    target.width = Math.min(Math.max(target.width, windowState.min[0], 1), windowState.max[0] || 16384);
    target.height = Math.min(Math.max(target.height, windowState.min[1], 1), windowState.max[1] || 16384);
    call('window', 'bounds', win.id, target.x, target.y, target.width, target.height);
    windowState.bounds = target;
    if (win.isNormal()) windowState.normal = { ...target };
    windowState.content = { x: target.x + dx, y: target.y + dy, width: target.width - dw, height: target.height - dh };
  };
  class BrowserWindow extends events.EventEmitter {
    constructor(options = {}) {
      super();
      if (!ready) throw Error('Call app.whenReady() before creating a window');
      options = { ...options, title: String(options.title ?? name) };
      // WARNING: pages use the `preload` of package.json, and these have no effect here.
      for (const key of ['webPreferences', 'autoHideMenuBar', 'disableAutoHideCursor', 'titleBarOverlay', 'enableLargerThanScreen', 'tabbingIdentifier', 'acceptFirstMouse', 'thickFrame', 'paintWhenInitiallyHidden', 'roundedCorners']) delete options[key];
      if (typeof options.backgroundColor === 'string') options.backgroundColor = argb(options.backgroundColor);
      if (options.parent != null && !(options.parent instanceof BrowserWindow)) throw new TypeError('Expected a BrowserWindow for parent');
      if (options.parent) { state(options.parent); options.parent = options.parent.id; } else delete options.parent;
      if (typeof options.opacity === 'number') options.opacity = Math.min(Math.max(options.opacity, 0), 1);
      if (options.vibrancy !== undefined) options.vibrancy = material(options.vibrancy) ?? undefined;
      const effect = effectStates.indexOf(options.visualEffectState ?? 'followWindow');
      if (effect < 0) throw new TypeError(`Invalid visualEffectState: ${options.visualEffectState}`);
      options.visualEffectState = effect;
      this.id = ++nextWindow;
      call('window', 'create', this.id, options);
      Object.defineProperty(this, 'webContents', { value: new WebContents(this.id), enumerable: true });
      const { x = 0, y = 0, width = 800, height = 600 } = options;
      states.set(this, { bounds: { x, y, width, height }, content: { x, y, width, height }, normal: { x, y, width, height }, minimized: false, maximized: false, fullscreen: false, visible: options.show !== false, focused: false,
        title: options.title, min: [options.minWidth ?? 0, options.minHeight ?? 0], max: [options.maxWidth ?? 0, options.maxHeight ?? 0], alwaysOnTop: options.alwaysOnTop === true,
        resizable: options.resizable !== false, movable: options.movable !== false, minimizable: options.minimizable !== false, maximizable: options.maximizable !== false, closable: options.closable !== false, native: null, closing: false,
        opacity: options.opacity ?? 1, shadow: options.hasShadow !== false, focusable: options.focusable !== false, contentProtection: false, workspaces: false, attention: -1,
        parent: options.parent ?? 0, modal: !!options.parent && options.modal === true, background: options.backgroundColor, transparent: options.transparent === true, effect });
      windows.set(this.id, this);
      Object.defineProperty(this, 'contentView', { value: browsers.contentView(this, options), enumerable: true });
      app.emit('web-contents-created', event(), this.webContents);
      app.emit('browser-window-created', event(), this);
    }
    load(html) { return this.webContents._load('load', String(html), 'apps://app/'); }
    loadFile(file, options) { return this.webContents.loadFile(file, options); }
    loadURL(url) { return this.webContents.loadURL(url); }
    setWindowButtonPosition(position) { const windowState = state(this); windowState.buttonPosition = position ?? null; call('window', 'buttonPosition', this.id, position ? int(position.x, 'x') : null, position ? int(position.y, 'y') : null); }
    getWindowButtonPosition() { return state(this).buttonPosition ?? null; }
    setAutoHideCursor() { state(this); }
    setTitleBarOverlay() { state(this); }
    setOverlayIcon() { state(this); }
    setIcon() { state(this); }
    setAutoHideMenuBar() { state(this); }
    isMenuBarAutoHide() { state(this); return false; }
    setMenuBarVisibility() { state(this); }
    isMenuBarVisible() { state(this); return false; }
    setDocumentEdited(edited) { state(this).edited = !!edited; }
    isDocumentEdited() { return !!state(this).edited; }
    setRepresentedFilename(file) { state(this).represented = String(file); }
    getRepresentedFilename() { return state(this).represented ?? ''; }
    setSheetOffset() { state(this); }
    invalidateShadow() { state(this); }
    getNativeWindowHandle() { state(this); const handle = node.Buffer.alloc(8); handle.writeBigUInt64LE(BigInt(this.id)); return handle; }
    moveTop() { state(this); if (this.isVisible()) this.showInactive(); }
    moveAbove() { state(this); }
    reload() { this.webContents.reload(); }
    close() {
      if (state(this).closing) return;
      const close = event();
      this.emit('close', close);
      if (close.defaultPrevented) { quitting = false; return; }
      states.get(this).closing = true;
      orphan(this);
      call('window', 'close', this.id);
    }
    destroy() { if (this.isDestroyed()) return; orphan(this); call('window', 'close', this.id); closed(this); }
    setWindowButtonVisibility(visible) { state(this); call('window', 'buttons', this.id, !!visible); }
    isDestroyed() { return !windows.has(this.id); }
    getTitle() { return state(this).title; }
    setTitle(title) { const windowState = state(this); windowState.title = String(title); call('window', 'title', this.id, windowState.title); }
    setBackgroundColor(color) { const windowState = state(this), background = argb(String(color)); call('window', 'background', this.id, background); windowState.background = background; }
    getBounds() { return { ...state(this).bounds }; }
    setBounds(bounds) { resize(this, bounds, false); }
    getContentBounds() { return { ...state(this).content }; }
    setContentBounds(bounds) { resize(this, bounds, true); }
    getSize() { const { width, height } = state(this).bounds; return [width, height]; }
    setSize(width, height) { resize(this, { width, height }, false); }
    getContentSize() { const { width, height } = state(this).content; return [width, height]; }
    setContentSize(width, height) { resize(this, { width, height }, true); }
    getPosition() { const { x, y } = state(this).bounds; return [x, y]; }
    setPosition(x, y) { resize(this, { x, y }, false); }
    center() { state(this); call('window', 'center', this.id); }
    getMinimumSize() { return [...state(this).min]; }
    setMinimumSize(width, height) { const windowState = state(this), min = [int(width, 'width'), int(height, 'height')]; call('window', 'limits', this.id, ...min, ...windowState.max); windowState.min = min; }
    getMaximumSize() { return [...state(this).max]; }
    setMaximumSize(width, height) { const windowState = state(this), max = [int(width, 'width'), int(height, 'height')]; call('window', 'limits', this.id, ...windowState.min, ...max); windowState.max = max; }
    #flag(key, value) { const windowState = state(this); call('window', key, this.id, !!value); windowState[key] = !!value; }
    isResizable() { return state(this).resizable; }
    setResizable(resizable) { this.#flag('resizable', resizable); }
    isMovable() { return state(this).movable; }
    setMovable(movable) { this.#flag('movable', movable); }
    isMinimizable() { return state(this).minimizable; }
    setMinimizable(minimizable) { this.#flag('minimizable', minimizable); }
    isMaximizable() { return state(this).maximizable; }
    setMaximizable(maximizable) { this.#flag('maximizable', maximizable); }
    isClosable() { return state(this).closable; }
    setClosable(closable) { this.#flag('closable', closable); }
    isAlwaysOnTop() { return state(this).alwaysOnTop; }
    setAlwaysOnTop(flag) { this.#flag('alwaysOnTop', flag); }
    minimize() { state(this).minimized = true; call('window', 'minimize', this.id); }
    isMinimized() { return state(this).minimized; }
    maximize() { state(this).maximized = true; call('window', 'maximize', this.id); }
    unmaximize() { state(this).maximized = false; call('window', 'unmaximize', this.id); }
    isMaximized() { return state(this).maximized; }
    restore() { state(this).minimized = false; call('window', 'restore', this.id); }
    isNormal() { const windowState = state(this); return !windowState.minimized && !windowState.maximized && !windowState.fullscreen; }
    getNormalBounds() { const windowState = state(this); return { ...(this.isNormal() ? windowState.bounds : windowState.normal) }; }
    setFullScreen(flag) { state(this).fullscreen = !!flag; call('window', 'fullscreen', this.id, !!flag); }
    isFullScreen() { return state(this).fullscreen; }
    show() { Object.assign(state(this), { visible: true, focused: true }); call('window', 'show', this.id); }
    showInactive() { state(this).visible = true; call('window', 'showInactive', this.id); }
    hide() { Object.assign(state(this), { visible: false, focused: false }); call('window', 'hide', this.id); }
    isVisible() { return state(this).visible; }
    focus() { Object.assign(state(this), { visible: true, focused: true }); call('window', 'focus', this.id); }
    blur() { state(this).focused = false; call('window', 'blur', this.id); }
    isFocused() { return state(this).focused; }
    setOpacity(opacity) { const windowState = state(this), value = Math.min(Math.max(num(opacity, 'opacity'), 0), 1); call('window', 'opacity', this.id, value); windowState.opacity = value; }
    getOpacity() { return state(this).opacity; }
    setHasShadow(hasShadow) { this.#flag('shadow', hasShadow); }
    hasShadow() { return state(this).shadow; }
    setIgnoreMouseEvents(ignore, { forward = false } = {}) { state(this); call('window', 'ignoreMouse', this.id, !!ignore, !!forward); }
    setVisibleOnAllWorkspaces(visible, { visibleOnFullScreen = false, skipTransformProcessType = false } = {}) {
      const windowState = state(this);
      if (!skipTransformProcessType) call('native.app.activationPolicy', visibleOnFullScreen ? 'accessory' : 'regular');
      call('window', 'workspaces', this.id, !!visible, !!visibleOnFullScreen);
      windowState.workspaces = !!visible;
    }
    isVisibleOnAllWorkspaces() { return state(this).workspaces; }
    setContentProtection(enable) { this.#flag('contentProtection', enable); }
    isContentProtected() { return state(this).contentProtection; }
    setFocusable(focusable) { this.#flag('focusable', focusable); }
    isFocusable() { return state(this).focusable; }
    setAspectRatio(aspectRatio, extraSize = {}) { state(this); call('window', 'aspectRatio', this.id, Math.max(num(aspectRatio, 'aspectRatio'), 0), int(extraSize.width ?? 0, 'width'), int(extraSize.height ?? 0, 'height')); }
    setProgressBar(progress) { state(this); call('native.app.progress', num(progress, 'progress')); }
    flashFrame(flag) {
      const windowState = state(this);
      // WARNING: request ids start at 0, and an active app gets -1, no request.
      if (flag) windowState.attention = call('native.app.attention', 'critical');
      else if (windowState.attention >= 0) { call('native.app.cancelAttention', windowState.attention); windowState.attention = -1; }
    }
    setSkipTaskbar() { state(this); }
    setMenu(menu) { state(this); if (menu !== null && !(menu instanceof nativeShell.Menu)) throw new TypeError('Invalid Menu'); }
    removeMenu() { state(this); }
    setVibrancy(type) {
      const windowState = state(this), value = material(type);
      call('window', 'vibrancy', this.id, value, windowState.effect);
      call('window', 'background', this.id, windowState.background ?? (value === null && !windowState.transparent ? '#fff' : '#0000'));
    }
    setParentWindow(parent) {
      const windowState = state(this);
      if (windowState.modal) throw Error('Can not be called for modal window');
      if (parent !== null && !(parent instanceof BrowserWindow)) throw new TypeError('Expected a BrowserWindow or null');
      for (let win = parent; win; win = win.getParentWindow()) if (win === this) throw Error('A window cannot be its own ancestor');
      call('window', 'parent', this.id, parent?.id ?? null);
      windowState.parent = parent?.id ?? 0;
    }
    getParentWindow() { return windows.get(state(this).parent) ?? null; }
    getChildWindows() { state(this); return [...windows.values()].filter(win => states.get(win).parent === this.id); }
    isModal() { return state(this).modal; }
    static getAllWindows() { return [...windows.values()]; }
    static fromId(id) { return windows.get(id) ?? null; }
    static fromWebContents(webContents) { for (const win of windows.values()) if (win.webContents === webContents) return win; return null; }
    static getFocusedWindow() { for (const win of windows.values()) if (states.get(win).focused) return win; return null; }
  }
  if (info.platform !== 'darwin') {
    const win32 = info.platform === 'win32';
    for (const method of ['center', 'unmaximize', 'showInactive', 'hide', 'blur', 'setMinimumSize', 'setMaximumSize', 'setResizable', 'setMovable', 'setMinimizable', 'setMaximizable', 'setClosable', 'setAlwaysOnTop',
      'setHasShadow', 'setIgnoreMouseEvents', 'setFocusable', 'setAspectRatio', 'setProgressBar', 'flashFrame', 'setSkipTaskbar', 'setIcon', 'setMenu', 'removeMenu', win32 ? 'setOpacity' : 'setVisibleOnAllWorkspaces'])
      BrowserWindow.prototype[method] = () => { throw Error(`BrowserWindow.${method}() is not supported on ${info.platform}`); };
    Object.assign(BrowserWindow.prototype, { setVibrancy() { state(this); } }, win32
      ? { setVisibleOnAllWorkspaces() { state(this); }, isVisibleOnAllWorkspaces() { state(this); return false; } }
      : { setOpacity() { state(this); }, getOpacity() { state(this); return 1; }, setContentProtection() { state(this); }, isContentProtected() { state(this); return false; } });
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
    quitApp();
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
    exit: (code = 0) => quitApp(code),
    relaunch: ({ args = info.argv.slice(1), execPath = info.execPath } = {}) => call('app.relaunch', [String(execPath), ...args.map(String)]),
    focus: ({ steal = false } = {}) => call('focus', !!steal),
    getAppPath: () => info.appPath,
    getPath: name => call('app.path', name),
    setPath: (name, path) => { call('app.setPath', name, path); if (name === 'userData') fs.mkdirSync(path, { recursive: true }); },
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
    getFileIcon: electron.fileIcon,
    setAppUserModelId() {},
    setDesktopName() {},
    getAppMetrics: () => [{ pid: info.pid, type: 'Browser', creationTime: 0, cpu: { percentCPUUsage: 0, cumulativeCPUUsage: 0, idleWakeupsPerSecond: 0 }, memory: { workingSetSize: process.memoryUsage.rss() >> 10, peakWorkingSetSize: 0 } }],
    runningUnderARM64Translation: false,
    runningUnderRosettaTranslation: false,
    isPackaged: info.isPackaged, platform: info.platform,
  });
  Object.defineProperty(app, 'name', { get: () => name, set: app.setName, enumerable: true });
  Object.defineProperty(app, 'hasWindows', { get: () => windows.size > 0, enumerable: true });
  nativeShell.bindApp?.(app, info.appName, () => BrowserWindow.getFocusedWindow());
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
  globalThis.__appsBeforeInput = (id, input) => {
    const prevented = event();
    try { windows.get(id)?.webContents.emit('before-input-event', prevented, input); } catch (error) { hooks.report(error); }
    return prevented.defaultPrevented;
  };
  globalThis.__appsEvent = (name, id, data) => {
    const win = windows.get(id);
    if (name === 'ready') { app.emit('will-finish-launching'); ready = true; app.emit('ready', event(), {}); resolveReady(); }
    else if (name === 'closed') { if (win) closed(win); }
    else if (name === 'resize') win?.contentView.setBounds({ x: 0, y: 0, ...data });
    else if (name === 'state') {
      const windowState = win && states.get(win);
      if (!windowState) return;
      // WARNING: the first state is the window's initial state, so it only reports focus.
      const old = windowState.native ?? { ...data, focused: false }, [x, y, width, height] = data.frame, [left, top, innerWidth, innerHeight] = data.content;
      windowState.native = data;
      Object.assign(windowState, { bounds: { x, y, width, height }, content: { x: left, y: top, width: innerWidth, height: innerHeight }, minimized: data.minimized, maximized: data.maximized, fullscreen: data.fullscreen, visible: data.visible, focused: data.focused });
      const resized = old.frame[2] !== width || old.frame[3] !== height;
      // WARNING: zoom and fullscreen animations report the frames in between as normal, so only moves and finished live resizes are normal bounds.
      if (win.isNormal() && !data.resizing && (old.resizing || !resized)) windowState.normal = { x, y, width, height };
      win.contentView.setBounds({ x: 0, y: 0, width: innerWidth, height: innerHeight });
      for (const [key, on, off] of [['minimized', 'minimize', 'restore'], ['maximized', 'maximize', 'unmaximize'], ['fullscreen', 'enter-full-screen', 'leave-full-screen'], ['visible', 'show', 'hide']])
        if (old[key] !== data[key]) win.emit(data[key] ? on : off);
      if (resized) win.emit('resize');
      if (!data.resizing && (resized || old.resizing)) win.emit('resized');
      if (old.frame[0] !== x || old.frame[1] !== y) { win.emit('move'); win.emit('moved'); }
      if (old.focused !== data.focused) { const kind = data.focused ? 'focus' : 'blur'; win.emit(kind); app.emit('browser-window-' + kind, event(), win); }
    }
    else if (name === 'close') win?.close();
    else if (name === 'html-full-screen') win?.emit(data ? 'enter-html-full-screen' : 'leave-html-full-screen');
    else if (name === 'title') {
      if (!win) return;
      const updated = event();
      win.webContents._event('title', data);
      win.emit('page-title-updated', updated, data, true);
      if (!updated.defaultPrevented) { states.get(win).title = data; call('window', 'title', id, data); }
    }
    else if (name === 'loaded' || name === 'failed') { win?.webContents._event(name, data); if (name === 'loaded') win?.emit('ready-to-show'); }
    else if (name === 'context-menu') win?.webContents.emit('context-menu', event(), { ...data, frame: frameOf(win.webContents) });
    else if (name === 'input-event') win?.webContents.emit('input-event', event(), data);
    else if (name === 'file-picker') { if (win) pickFiles(win, data); }
    else if (name === 'activate') app.emit('activate', event(), !!data);
    else if (name === 'open-url' || name === 'open-file') app.emit(name, event(), data);
    else if (name === 'before-quit') quit();
    else if (name === 'executed') {
      // WARNING: any page can send `executed`, so it settles only a pending executeJavaScript() of its own window.
      const request = captures.get(data.request);
      if (request?.win !== id || request.op !== 'execute') return;
      captures.delete(data.request);
      const result = parse(data.data);
      if (result.error !== undefined) request.reject(new Error(result.error)); else request.resolve(result.value);
    }
    else if (name === 'captured') {
      const request = captures.get(data.id);
      captures.delete(data.id);
      if (data.error) return request?.reject(new Error(data.error));
      const bytes = Uint8Array.from(atob(data.png), char => char.charCodeAt(0));
      request?.resolve(electron.nativeImage.createFromBuffer(bytes));
    }
  };
  const abortError = signal => Object.assign(new Error('The operation was aborted', { cause: signal.reason }), { name: 'AbortError', code: 'ABORT_ERR' });
  const later = (schedule, cancel, value, { signal } = {}) => new Promise((resolve, reject) => {
    if (signal?.aborted) return reject(abortError(signal));
    const abort = () => { cancel(handle); reject(abortError(signal)); };
    const handle = schedule(() => { signal?.removeEventListener('abort', abort); resolve(value); });
    signal?.addEventListener('abort', abort, { once: true });
  });
  const timerModule = { setTimeout, clearTimeout, setInterval, clearInterval, setImmediate, clearImmediate, promises: {
    setTimeout: (delay, value, options) => later(fn => setTimeout(fn, delay), clearTimeout, value, options),
    setImmediate: (value, options) => later(setImmediate, clearImmediate, value, options),
    async *setInterval(delay = 1, value, { signal } = {}) {
      if (signal?.aborted) throw abortError(signal);
      let ticks = 0, wake = null;
      const abort = () => wake?.(), id = setInterval(() => { ticks++; wake?.(); }, delay);
      signal?.addEventListener('abort', abort, { once: true });
      try { for (;;) { if (!ticks) await new Promise(resolve => { wake = resolve; }); wake = null; if (signal?.aborted) throw abortError(signal); ticks--; yield value; } }
      finally { clearInterval(id); signal?.removeEventListener('abort', abort); }
    },
    scheduler,
  } };
  setTimeout[node.util.promisify.custom] = timerModule.promises.setTimeout;
  setImmediate[node.util.promisify.custom] = timerModule.promises.setImmediate;
  const outOfRange = (name, value) => Object.assign(new RangeError(`The value of "${name}" is out of range. It must be >= 1 && <= 9007199254740991. Received ${value}`), { code: 'ERR_OUT_OF_RANGE' });
  // WARNING: values are counted in buckets of about 3 significant digits, so percentiles are approximate like HdrHistogram's.
  class RecordableHistogram {
    #counts = new Map(); #count = 0; #min = 9223372036854776000; #max = 0; #sum = 0; #squares = 0; #last;
    get count() { return this.#count; }
    get min() { return this.#min; }
    get max() { return this.#max; }
    get mean() { return this.#count ? this.#sum / this.#count : NaN; }
    get stddev() { return this.#count ? Math.sqrt(Math.max(0, this.#squares / this.#count - (this.#sum / this.#count) ** 2)) : NaN; }
    get exceeds() { return 0; }
    get countBigInt() { return BigInt(this.#count); }
    get minBigInt() { return BigInt(this.#min); }
    get maxBigInt() { return BigInt(this.#max); }
    get exceedsBigInt() { return 0n; }
    percentile(percentile) {
      if (!(percentile > 0 && percentile <= 100)) throw Object.assign(new RangeError(`The value of "percentile" is out of range. It must be > 0 && <= 100. Received ${percentile}`), { code: 'ERR_OUT_OF_RANGE' });
      let seen = 0;
      for (const value of [...this.#counts.keys()].sort((a, b) => a - b)) if ((seen += this.#counts.get(value)) >= percentile / 100 * this.#count) return Math.max(this.#min, Math.min(value, this.#max));
      return 0;
    }
    percentileBigInt(percentile) { return BigInt(this.percentile(percentile)); }
    get percentiles() {
      if (!this.#count) return new Map([[100, 0]]);
      const map = new Map([[0, this.#min]]);
      for (let i = 1; i < 64; i++) { const percentile = 100 - 100 / 2 ** i, value = this.percentile(percentile); map.set(percentile, value); if (value >= this.#max) break; }
      return map.set(100, this.#max);
    }
    get percentilesBigInt() { return new Map([...this.percentiles].map(([percentile, value]) => [percentile, BigInt(value)])); }
    record(value) {
      if (typeof value !== 'number' && typeof value !== 'bigint') throw Object.assign(new TypeError(`The "val" argument must be of type number or bigint. Received ${value === null ? 'null' : typeof value}`), { code: 'ERR_INVALID_ARG_TYPE' });
      value = Number(value);
      if (!Number.isSafeInteger(value) || value < 1) throw outOfRange('val', value);
      const step = value < 2048 ? 1 : 2 ** (Math.floor(Math.log2(value)) - 10), key = Math.round(value / step) * step;
      this.#counts.set(key, (this.#counts.get(key) ?? 0) + 1);
      this.#count++; this.#sum += value; this.#squares += value * value;
      if (value < this.#min) this.#min = value;
      if (value > this.#max) this.#max = value;
    }
    recordDelta() { const now = process.hrtime.bigint(); if (this.#last !== undefined) this.record(Number(now - this.#last) || 1); this.#last = now; }
    add(other) {
      for (const [value, count] of other.#counts) this.#counts.set(value, (this.#counts.get(value) ?? 0) + count);
      this.#count += other.#count; this.#sum += other.#sum; this.#squares += other.#squares; this.#min = Math.min(this.#min, other.#min); this.#max = Math.max(this.#max, other.#max);
    }
    reset() { this.#counts.clear(); this.#count = this.#sum = this.#squares = this.#max = 0; this.#min = 9223372036854776000; this.#last = undefined; }
  }
  const monitorEventLoopDelay = ({ resolution = 10 } = {}) => {
    if (!Number.isSafeInteger(resolution) || resolution < 1) throw outOfRange('options.resolution', resolution);
    const histogram = new RecordableHistogram();
    let timer = null, last = 0;
    return Object.assign(histogram, {
      enable() { if (timer) return false; last = performance.now(); timer = setInterval(() => { const now = performance.now(); histogram.record(Math.max(1, Math.round((now - last - resolution) * 1e6))); last = now; }, resolution); return true; },
      disable() { if (!timer) return false; clearInterval(timer); timer = null; return true; },
    });
  };
  const perf_hooks = { performance, PerformanceEntry, PerformanceMark, PerformanceMeasure, PerformanceObserver, PerformanceObserverEntryList, createHistogram: () => new RecordableHistogram(), monitorEventLoopDelay };
  const compat = globalThis.__appsNodeCompat(events.EventEmitter, stream, net.api);
  delete globalThis.__appsNodeCompat;
  const modules = { ...compat, fs, path, os, process, events, stream, util: node.util, url: node.url, buffer: node.buffer, string_decoder: node.string_decoder, assert: node.assert, querystring: node.querystring, dns: node.dns, tty: node.tty, zlib: io.zlib, readline: io.readline, crypto: { ...globalThis.__appsNodeCrypto, ...io.crypto }, child_process: children.api, net: net.api, http: http.http, https: http.https, worker_threads: workers.api, perf_hooks, timers: timerModule, sqlite, ffi };
  modules.constants = Object.assign({}, os.constants.errno, os.constants.priority, os.constants.signals, fs.constants, modules.crypto.constants);
  const builtins = { ...modules, module: null, sys: node.util, 'fs/promises': fs.promises, 'stream/promises': stream.promises, 'stream/consumers': stream.consumers, 'stream/web': { ReadableStream, WritableStream, TransformStream, ByteLengthQueuingStrategy, CountQueuingStrategy, TextEncoderStream, TextDecoderStream, CompressionStream, DecompressionStream, ReadableStreamDefaultReader, ReadableStreamBYOBReader, ReadableStreamBYOBRequest, ReadableStreamDefaultController, ReadableByteStreamController, WritableStreamDefaultWriter, WritableStreamDefaultController, TransformStreamDefaultController }, 'timers/promises': timerModule.promises, 'util/types': node.util.types, 'path/posix': path.posix, 'path/win32': path.win32, 'assert/strict': node.assert.strict, 'readline/promises': io.readline.promises, 'dns/promises': node.dns.promises };
  // WARNING: like Node, prefixed modules are built in only with `node:`.
  const prefixed = new Set(['sqlite', 'ffi', 'sea']), isBuiltin = name => { name = String(name); const key = name.replace(/^node:/, ''); return Object.hasOwn(builtins, key) && (key !== name || !prefixed.has(key)); };
  const missingModule = name => Object.assign(new Error(`Cannot find module '${name}'`), { code: 'MODULE_NOT_FOUND' });
  builtins.module = { builtinModules: Object.keys(builtins).map(name => prefixed.has(name) ? 'node:' + name : name), isBuiltin, createRequire: () => Object.assign(name => {
    const key = String(name).replace(/^node:/, '');
    const value = key === 'electron' || key === '@pierre/apps' ? globalThis.__apps : key === 'node-pty' ? pty.api : key === 'ws' ? http.ws : isBuiltin(name) ? builtins[key] : undefined;
    if (value === undefined) throw missingModule(name);
    return value;
  }, { resolve: name => { if (isBuiltin(name) || /^(node-pty|ws|undici|electron)(\/|$)/.test(String(name))) return String(name); throw missingModule(name); } }) };
  // WARNING: zoom is per window and lives on main, so a preload reads what main last told it.
  const webFrame = Object.freeze({ getZoomFactor: () => 1, getZoomLevel: () => 0, setZoomFactor() {}, setZoomLevel() {}, setVisualZoomLevelLimits() {}, insertCSS: () => '', removeInsertedCSS() {} });
  process.getBuiltinModule = name => isBuiltin(name) ? builtins[String(name).replace(/^node:/, '')] : undefined;
  if (info.node) globalThis.__appsFinish = () => { process.emit('beforeExit', Number(process.exitCode ?? 0) | 0); return __appsAlive() ? -1 : finish(); };
  // WARNING: like Electron, preloads don't get the main process modules: windows, app and native state belong to main.
  const main = role === 'main' ? { app, BrowserWindow, View: browsers.View, WebContentsView: browsers.WebContentsView, session: browsers.session, webContents, protocol: electron.protocol, autoUpdater: electron.autoUpdater, desktopCapturer: electron.desktopCapturer } : {};
  globalThis.__apps = Object.freeze({ ...main, nativeImage: electron.nativeImage, ClipboardItem: electron.ClipboardItem, webFrame, ...nativeShell, ...bridge, ipcMain: role === 'main' ? bridge.ipcMain : undefined, ipcRenderer: role === 'main' ? undefined : bridge.ipcRenderer, serve: servers.serve, ...modules, net: electronNet, module: builtins.module, pty: pty.api, ws: http.ws });
})();
