globalThis.__appsWorkers = (call, nativeAccess) => {
  const cryptoCodec = globalThis.__appsCryptoCodec, { decode: decodeBytes } = globalThis.__appsBytes;
  const encodeBytes = value => { if(value.toBase64)return value.toBase64();let text='';for(let i=0;i<value.length;i+=8192)text+=String.fromCharCode(...value.subarray(i,i+8192));return btoa(text); };
  delete globalThis.__appsCryptoCodec;
  if (!nativeAccess) delete globalThis.__appsBytes;
  const encode = value => {
    const seen = new Map(), nodes = [];
    const visit = value => {
      if (value === undefined) return { u: true };
      if (typeof value === 'bigint') return { b: String(value) };
      if (typeof value === 'number' && (!Number.isFinite(value) || Object.is(value, -0))) return { n: Object.is(value, -0) ? '-0' : String(value) };
      if (typeof value === 'function' || typeof value === 'symbol') throw new DOMException('Value cannot be cloned', 'DataCloneError');
      if (value === null || typeof value !== 'object') return value;
      if (seen.has(value)) return { r: seen.get(value) };
      const id = nodes.length; seen.set(value, id); nodes.push(null);
      let node;
      if (globalThis.CryptoKey && value instanceof CryptoKey) node = ['crypto', cryptoCodec.encode(value)];
      else if (value instanceof ArrayBuffer) node = ['buffer', encodeBytes(new Uint8Array(value))];
      else if (ArrayBuffer.isView(value)) node = ['view', value.constructor.name, visit(value.buffer), value.byteOffset, value.byteLength];
      else if (value instanceof Date) node = ['date', value.getTime()];
      else if (value instanceof RegExp) node = ['regexp', value.source, value.flags];
      else if (value instanceof Map) node = ['map', [...value].map(([k, v]) => [visit(k), visit(v)])];
      else if (value instanceof Set) node = ['set', [...value].map(visit)];
      else if (value instanceof Error) node = ['error', value.name, value.message, value.stack, visit(value.cause)];
      else if (Array.isArray(value)) node = ['array', value.map(visit)];
      else if (Object.prototype.toString.call(value) === '[object Object]') node = ['object', Object.keys(value).map(k => [k, visit(value[k])])];
      else throw new DOMException('Value cannot be cloned', 'DataCloneError');
      nodes[id] = node; return { r: id };
    };
    return { value: visit(value), nodes };
  };
  const decode = wire => {
    if (!wire) return undefined;
    const created = new Map();
    const visit = value => {
      if (value === null || typeof value !== 'object') return value;
      if (value.u) return undefined;
      if (value.b !== undefined) return BigInt(value.b);
      if (value.n !== undefined) return value.n === '-0' ? -0 : Number(value.n);
      const id = value.r;
      if (created.has(id)) return created.get(id);
      const node = wire.nodes[id]; let result;
      switch (node[0]) {
        case 'crypto': result = cryptoCodec.decode(node[1]); break;
        case 'buffer': result = (typeof node[1] === 'string' ? decodeBytes(node[1]) : new Uint8Array(node[1])).buffer; break;
        case 'view': { const buffer = visit(node[2]), Type = globalThis[node[1]]; result = node[1] === 'DataView' ? new DataView(buffer, node[3], node[4]) : new Type(buffer, node[3], node[4] / Type.BYTES_PER_ELEMENT); break; }
        case 'date': result = new Date(node[1]); break;
        case 'regexp': result = new RegExp(node[1], node[2]); break;
        case 'map': result = new Map(); break;
        case 'set': result = new Set(); break;
        case 'array': result = []; break;
        case 'object': result = {}; break;
        case 'error': result = new Error(node[2]); result.name = node[1]; result.stack = node[3]; break;
        default: throw new DOMException('Invalid clone data', 'DataCloneError');
      }
      created.set(id, result);
      if (node[0] === 'map') for (const [k, v] of node[1]) result.set(visit(k), visit(v));
      if (node[0] === 'set') for (const v of node[1]) result.add(visit(v));
      if (node[0] === 'array') for (const v of node[1]) result.push(visit(v));
      if (node[0] === 'object') for (const [k, v] of node[1]) Object.defineProperty(result, k, { value: visit(v), enumerable: true, writable: true, configurable: true });
      if (node[0] === 'error') result.cause = visit(node[4]);
      return result;
    };
    return visit(wire.value);
  };
  const transfers = options => {
    const values = Array.from(Array.isArray(options) ? options : options?.transfer ?? []);
    if (new Set(values).size !== values.length || values.some(v => !(v instanceof ArrayBuffer) || typeof v.transfer !== 'function')) throw new DOMException('Only transferable ArrayBuffers are supported', 'DataCloneError');
    return values;
  };
  if (!globalThis.structuredClone) globalThis.structuredClone = (value, options) => {
    const transfer = transfers(options), result = decode(encode(value));
    for (const buffer of transfer) buffer.transfer(0);
    return result;
  };
  globalThis.__appsStorageClone = { encode, decode };
  class Endpoint extends EventTarget {
    _listeners = new Map();
    _messageListeners = new Set();
    addEventListener(type, listener, options) { super.addEventListener(type, listener, options); if (type === 'message' && listener && !options?.signal?.aborted) this._messageListeners.add(listener); }
    removeEventListener(type, listener, options) { super.removeEventListener(type, listener, options); if (type === 'message') this._messageListeners.delete(listener); }
    on(type, listener) { const list = this._listeners.get(type) ?? []; list.push(listener); this._listeners.set(type, list); return this; }
    once(type, listener) { const once = (...args) => { this.off(type, once); listener(...args); }; return this.on(type, once); }
    off(type, listener) { this._listeners.set(type, (this._listeners.get(type) ?? []).filter(fn => fn !== listener)); return this; }
    removeListener(type, listener) { return this.off(type, listener); }
    removeAllListeners(type) { if (type === undefined) this._listeners.clear(); else this._listeners.delete(type); return this; }
    _emit(type, value) {
      this.dispatchEvent(type === 'message' ? new MessageEvent(type, { data: value }) : Object.assign(new Event(type), type === 'error' ? { error: value, message: value.message } : {}));
      for (const listener of [...this._listeners.get(type) ?? []]) listener(value);
    }
  }
  const info = call('web.worker.info'), parentPort = info.main ? null : new Endpoint();
  if (parentPort) {
    parentPort.postMessage = (value, options) => {
      const transfer = transfers(options), message = encode(value);
      call('web.worker.post', message); for (const buffer of transfer) buffer.transfer(0);
    };
    parentPort.close = () => call('web.worker.close');
    globalThis.postMessage = parentPort.postMessage;
    globalThis.close = parentPort.close;
    globalThis.__appsWorkerReceive = wire => { const data = decode(wire); parentPort._emit('message', data); dispatchEvent(new MessageEvent('message', { data })); };
  }
  const workers = new Map(), nativeToken = Symbol();
  class Worker extends Endpoint {
    _queue = [];
    _referenced = true;
    _closed = false;
    constructor(source, options = {}, token) {
      super();
      const native = token === nativeToken && nativeAccess;
      const start = text => {
        if (this._closed) return;
        this.threadId = call('web.worker.start', text, native, encode(options.workerData));
        workers.set(this.threadId, this);
        for (const message of this._queue) call('web.worker.send', this.threadId, message);
        this._queue.length = 0;
      };
      if (options.eval && native) start(String(source));
      else if (native && !/^(?:https?|data|blob):/.test(String(source))) {
        const path = String(source).startsWith('file:') ? decodeURIComponent(new URL(source).pathname) : String(source);
        start(__apps.fs.readFileSync(path, 'utf8'));
      } else {
        Promise.resolve().then(async () => {
          const url = String(source);
          if (url.startsWith('data:')) { const comma = url.indexOf(','); start(url.slice(0, comma).endsWith(';base64') ? atob(url.slice(comma + 1)) : decodeURIComponent(url.slice(comma + 1))); }
          else { const response = await fetch(source); if (!response.ok) throw new Error('Cannot load worker: ' + response.status); start(await response.text()); }
        }).catch(error => { this._closed = true; this._emit('error', error); });
      }
    }
    postMessage(value, options) {
      if (this._closed) return;
      const transfer = transfers(options), message = encode(value);
      if (this.threadId === undefined) this._queue.push(message); else call('web.worker.send', this.threadId, message);
      for (const buffer of transfer) buffer.transfer(0);
    }
    terminate() {
      this._closed = true;
      if (this._exitCode !== undefined) return Promise.resolve(this._exitCode);
      if (this.threadId === undefined) return Promise.resolve(0);
      call('web.worker.terminate', this.threadId);
      return new Promise(resolve => this.once('exit', resolve));
    }
    ref() { this._referenced = true; return this; }
    unref() { this._referenced = false; return this; }
  }
  class NodeWorker extends Worker { constructor(source, options) { super(source, options, nativeToken); } }
  class MessagePort extends Endpoint {
    _closed = false;
    _started = false;
    _pending = [];
    postMessage(value, options) {
      if (this._closed || this._peer._closed) return;
      const data = structuredClone(value, Array.isArray(options) ? { transfer: options } : options), target = this._peer;
      queueMicrotask(() => { if (target._closed) return; if (target._started || target.onmessage || target._listeners.has('message')) target._emit('message', data); else target._pending.push(data); });
    }
    start() { this._started = true; for (const value of this._pending.splice(0)) queueMicrotask(() => { if (!this._closed) this._emit('message', value); }); }
    close() { this._closed = true; this._pending.length = 0; }
    on(type, listener) { super.on(type, listener); if (type === 'message') this.start(); return this; }
  }
  class MessageChannel { constructor() { this.port1 = new MessagePort(); this.port2 = new MessagePort(); this.port1._peer = this.port2; this.port2._peer = this.port1; } }
  Object.assign(globalThis, { Worker, MessageChannel, MessagePort });
  return {
    poll() {
      if (!workers.size) return;
      for (const event of call('web.worker.poll')) {
        const worker = workers.get(event.id); if (!worker) continue;
        if (event.kind === 'message') worker._emit('message', decode(event.data));
        else if (event.kind === 'error') worker._emit('error', new Error(event.message));
        else if (event.kind === 'online') worker._emit('online');
        else if (event.kind === 'exit') { workers.delete(event.id); worker._closed = true; worker._exitCode = event.code; worker._emit('exit', event.code); }
      }
    },
    pending: () => !!parentPort && ((parentPort._listeners.get('message')?.length ?? 0) > 0 || parentPort._messageListeners.size > 0 || typeof parentPort.onmessage === 'function') || [...workers.values()].some(worker => worker._referenced),
    api: { Worker: NodeWorker, MessageChannel, MessagePort, isMainThread: info.main, parentPort, workerData: decode(info.data) ?? null, threadId: info.id },
  };
};
