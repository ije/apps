globalThis.__appsNodeCompat = (EventEmitter, stream, tls) => {
  // JSC cannot hook await. Stores cover synchronous run() calls and functions from bind() or snapshot().
  let current = new Map();
  const within = (context, fn, self, args) => { const saved = current; current = context; try { return Reflect.apply(fn, self, args); } finally { current = saved; } };
  class AsyncLocalStorage {
    static bind(fn) { return AsyncResource.bind(fn); }
    static snapshot() { const context = current; return (fn, ...args) => within(context, fn, undefined, args); }
    disable() { current = new Map(current); current.delete(this); }
    enterWith(store) { current = new Map(current).set(this, store); }
    exit(fn, ...args) { const context = new Map(current); context.delete(this); return within(context, fn, undefined, args); }
    getStore() { return current.get(this); }
    run(store, fn, ...args) { return within(new Map(current).set(this, store), fn, undefined, args); }
  }
  let nextAsyncId = 1;
  class AsyncResource {
    #context = current; #id = ++nextAsyncId;
    constructor(type) { if (typeof type !== 'string') throw Object.assign(new TypeError('The "type" argument must be of type string.'), { code: 'ERR_INVALID_ARG_TYPE' }); this.type = type; }
    static bind(fn, type, thisArg) { return new AsyncResource(type ?? (fn.name || 'bound-anonymous-fn')).bind(fn, thisArg); }
    bind(fn, thisArg) { const resource = this; return function (...args) { return resource.runInAsyncScope(fn, thisArg ?? this, ...args); }; }
    runInAsyncScope(fn, thisArg, ...args) { return within(this.#context, fn, thisArg, args); }
    emitDestroy() { return this; }
    asyncId() { return this.#id; }
    triggerAsyncId() { return 0; }
  }
  class EventEmitterAsyncResource extends EventEmitter {
    #resource;
    constructor(options = {}) { super(options); this.#resource = Object.assign(new AsyncResource(options.name ?? new.target.name), { eventEmitter: this }); }
    emit(...args) { return this.#resource.runInAsyncScope(super.emit, this, ...args); }
    emitDestroy() { this.#resource.emitDestroy(); }
    get asyncResource() { return this.#resource; }
    get asyncId() { return this.#resource.asyncId(); }
    get triggerAsyncId() { return this.#resource.triggerAsyncId(); }
  }
  const async_hooks = { AsyncLocalStorage, AsyncResource, createHook: () => ({ enable() { return this; }, disable() { return this; } }), executionAsyncId: () => 0, triggerAsyncId: () => 0, executionAsyncResource: () => ({}) };

  const channels = new Map();
  class Channel {
    #subscribers = [];
    constructor(name) { this.name = name; }
    get hasSubscribers() { return this.#subscribers.length > 0; }
    subscribe(fn) { this.#subscribers = [...this.#subscribers, fn]; }
    unsubscribe(fn) { const list = this.#subscribers, index = list.indexOf(fn); if (index < 0) return false; this.#subscribers = list.toSpliced(index, 1); return true; }
    publish(message) { for (const fn of this.#subscribers) try { fn(message, this.name); } catch (error) { queueMicrotask(() => { throw error; }); } }
    bindStore() {} unbindStore() { return false; }
    runStores(data, fn, thisArg, ...args) { this.publish(data); return Reflect.apply(fn, thisArg, args); }
  }
  const channel = name => { let item = channels.get(name); if (!item) channels.set(name, item = new Channel(name)); return item; };
  const events = ['start', 'end', 'asyncStart', 'asyncEnd', 'error'];
  class TracingChannel {
    constructor(name) { for (const event of events) this[event] = typeof name === 'string' ? channel(`tracing:${name}:${event}`) : name[event]; }
    get hasSubscribers() { return events.some(event => this[event].hasSubscribers); }
    subscribe(handlers) { for (const event of events) if (handlers[event]) this[event].subscribe(handlers[event]); }
    unsubscribe(handlers) { let done = true; for (const event of events) if (handlers[event] && !this[event].unsubscribe(handlers[event])) done = false; return done; }
    traceSync(fn, context = {}, thisArg, ...args) {
      this.start.publish(context);
      try { return context.result = Reflect.apply(fn, thisArg, args); }
      catch (error) { context.error = error; this.error.publish(context); throw error; }
      finally { this.end.publish(context); }
    }
    tracePromise(fn, context = {}, thisArg, ...args) {
      const settle = () => { this.asyncStart.publish(context); this.asyncEnd.publish(context); };
      return Promise.resolve(this.traceSync(fn, context, thisArg, ...args)).then(result => { context.result = result; settle(); return result; }, error => { context.error = error; this.error.publish(context); settle(); throw error; });
    }
    traceCallback(fn, position = -1, context = {}, thisArg, ...args) {
      const callback = args.at(position), tracer = this;
      args[position < 0 ? args.length + position : position] = function (error, result) {
        if (error) { context.error = error; tracer.error.publish(context); } else context.result = result;
        tracer.asyncStart.publish(context);
        try { return Reflect.apply(callback, this, arguments); } finally { tracer.asyncEnd.publish(context); }
      };
      return this.traceSync(fn, context, thisArg, ...args);
    }
  }
  const diagnostics_channel = { Channel, channel, hasSubscribers: name => channel(name).hasSubscribers, subscribe: (name, fn) => channel(name).subscribe(fn), unsubscribe: (name, fn) => channel(name).unsubscribe(fn), tracingChannel: name => new TracingChannel(name) };

  const absent = name => Object.assign(new Error(`Cannot get SEA asset ${name}: this is not a single executable application`), { code: 'ERR_NOT_IN_SINGLE_EXECUTABLE_APPLICATION' });
  const sea = { isSea: () => false, getAssetKeys: () => [], getAsset(name) { throw absent(name); }, getAssetAsBlob(name) { throw absent(name); }, getRawAsset(name) { throw absent(name); } };

  const body = response => {
    const output = response.body ? stream.Readable.fromWeb(response.body) : stream.Readable.from([]);
    const read = () => new Promise((resolve, reject) => { const chunks = []; output.on('data', chunk => chunks.push(chunk)).once('end', () => resolve(Buffer.concat(chunks))).once('error', reject); });
    return Object.assign(output, {
      text: async () => (await read()).toString(), json: async () => JSON.parse((await read()).toString()), bytes: async () => new Uint8Array(await read()),
      arrayBuffer: async () => { const bytes = await read(); return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength); },
      blob: async () => new Blob([await read()], { type: response.headers.get('content-type') ?? '' }),
      dump: async () => { output.destroy(); },
    });
  };
  class Dispatcher extends EventEmitter {
    #closed = false;
    get closed() { return this.#closed; }
    get destroyed() { return this.#closed; }
    async request(options, callback) {
      if (this.#closed) throw Object.assign(new Error('The client is destroyed'), { code: 'UND_ERR_DESTROYED' });
      const { origin, path = '/', method = 'GET', headers, signal, opaque, query } = options;
      const url = new URL(path, origin);
      for (const [name, value] of Object.entries(query ?? {})) url.searchParams.append(name, value);
      const list = new Headers();
      if (Array.isArray(headers)) for (let index = 0; index < headers.length; index += 2) list.append(headers[index], headers[index + 1]);
      else for (const [name, value] of Object.entries(headers ?? {})) if (value !== undefined) for (const item of Array.isArray(value) ? value : [value]) list.append(name, item);
      const init = { method, headers: list, signal, redirect: 'manual' };
      if (options.body != null && method !== 'GET' && method !== 'HEAD') {
        const source = options.body;
        init.body = typeof source.pipe === 'function' || typeof source[Symbol.asyncIterator] === 'function' ? stream.Readable.toWeb(stream.Readable.from(source)) : source;
        if (init.body instanceof ReadableStream) init.duplex = 'half';
      }
      const response = await fetch(url, init), result = {};
      for (const [name, value] of response.headers) result[name] = name === 'set-cookie' ? response.headers.getSetCookie() : value;
      const reply = { statusCode: response.status, statusText: response.statusText, headers: result, trailers: {}, opaque, context: {}, body: body(response) };
      if (callback) { callback(null, reply); return; }
      return reply;
    }
    dispatch() { throw Object.assign(new Error('undici dispatch() is not supported'), { code: 'UND_ERR_NOT_SUPPORTED' }); }
    close(callback) { this.#closed = true; if (callback) queueMicrotask(callback); else return Promise.resolve(); }
    destroy(error, callback) { this.#closed = true; if (typeof (callback ?? error) === 'function') queueMicrotask(callback ?? error); else return Promise.resolve(); }
  }
  class Agent extends Dispatcher {}
  class Client extends Dispatcher { #origin; constructor(origin) { super(); this.#origin = origin; } request(options, callback) { return super.request({ origin: this.#origin, ...options }, callback); } }
  class Pool extends Client {}
  let global = new Agent();
  const undici = {
    Dispatcher, Agent, Client, Pool, Headers, Request, Response, FormData, WebSocket: globalThis.WebSocket, fetch: (...args) => fetch(...args), getGlobalDispatcher: () => global, setGlobalDispatcher: value => { global = value; },
    request: (url, options = {}) => { const target = new URL(url); return (options.dispatcher ?? global).request({ ...options, origin: target.origin, path: options.path ?? target.pathname + target.search }); },
  };
  const v8 = {
    getHeapStatistics() { const rss = process.memoryUsage.rss(); return { total_heap_size: rss, total_heap_size_executable: 0, total_physical_size: rss, total_available_size: 0, used_heap_size: rss, heap_size_limit: 0, malloced_memory: 0, peak_malloced_memory: 0, does_zap_garbage: 0, number_of_native_contexts: 1, number_of_detached_contexts: 0, total_global_handles_size: 0, used_global_handles_size: 0, external_memory: 0 }; },
    getHeapSnapshot() { throw Object.assign(new Error('Heap snapshots are not supported'), { code: 'ERR_FEATURE_UNAVAILABLE_ON_PLATFORM' }); },
    writeHeapSnapshot() { throw Object.assign(new Error('Heap snapshots are not supported'), { code: 'ERR_FEATURE_UNAVAILABLE_ON_PLATFORM' }); },
    setFlagsFromString() {},
    serialize: value => Buffer.from(JSON.stringify(value)),
    deserialize: bytes => JSON.parse(Buffer.from(bytes).toString()),
  };
  return { async_hooks, diagnostics_channel, sea, tls, undici, v8, EventEmitterAsyncResource };
};
