globalThis.__appsPlatform = (call, node, codec) => {
  const codecBytes = globalThis.__appsBytes;
  globalThis.self = globalThis;
  const native = globalThis.__appsNow;
  delete globalThis.__appsNow;
  const clock = call('web.platform.time'), base = native() - clock.now;
  const timeline = [], observers = new Map(), illegal = Symbol(), types = node ? ['function', 'mark', 'measure'] : ['mark', 'measure'];
  class PerformanceEntry {
    #name; #type; #start; #duration;
    constructor(name, type, start, duration) { if (new.target === PerformanceEntry) throw new TypeError('Illegal constructor'); this.#name = String(name); this.#type = type; this.#start = start; this.#duration = duration; }
    get name() { return this.#name; }
    get entryType() { return this.#type; }
    get startTime() { return this.#start; }
    get duration() { return this.#duration; }
    toJSON() { return { name: this.name, entryType: this.entryType, startTime: this.startTime, duration: this.duration, detail: this.detail }; }
  }
  class PerformanceMark extends PerformanceEntry {
    #detail;
    constructor(name, { startTime = performance.now(), detail = null } = {}) { if (!(startTime >= 0)) throw new TypeError(`${startTime} is not a valid startTime`); super(name, 'mark', +startTime, 0); this.#detail = detail; }
    get detail() { return this.#detail; }
  }
  class PerformanceMeasure extends PerformanceEntry {
    #detail;
    constructor(token, name, start, duration, detail) { if (token !== illegal) throw new TypeError('Illegal constructor'); super(name, 'measure', start, duration); this.#detail = detail; }
    get detail() { return this.#detail; }
  }
  const select = (list, name, type) => list.filter(entry => (name === undefined || entry.name === String(name)) && (type === undefined || entry.entryType === type));
  class PerformanceObserverEntryList {
    #entries;
    constructor(token, entries) { if (token !== illegal) throw new TypeError('Illegal constructor'); this.#entries = entries; }
    getEntries() { return [...this.#entries]; }
    getEntriesByName(name, type) { return select(this.#entries, name, type); }
    getEntriesByType(type) { return select(this.#entries, undefined, type); }
  }
  class PerformanceObserver {
    #callback; #types = new Set(); #queue = []; #timer = null;
    static get supportedEntryTypes() { return [...types]; }
    constructor(callback) { if (typeof callback !== 'function') throw Object.assign(new TypeError('The "callback" argument must be of type function'), { code: 'ERR_INVALID_ARG_TYPE' }); this.#callback = callback; }
    observe({ entryTypes, type, buffered = false } = {}) {
      for (const name of entryTypes ?? [type]) if (types.includes(name)) this.#types.add(name);
      observers.set(this, entry => { if (!this.#types.has(entry.entryType)) return; this.#queue.push(entry); this.#timer ??= setTimeout(() => { this.#timer = null; const list = this.takeRecords(); if (list.length) this.#callback(new PerformanceObserverEntryList(illegal, list), this); }, 0); });
      if (buffered && type) for (const entry of select(timeline, undefined, type)) observers.get(this)(entry);
    }
    disconnect() { observers.delete(this); this.#types.clear(); this.#queue = []; }
    takeRecords() { const list = this.#queue; this.#queue = []; return list; }
  }
  const record = entry => { timeline.push(entry); for (const notify of observers.values()) notify(entry); return entry; };
  const at = value => {
    if (typeof value === 'number') return value;
    const mark = timeline.findLast(entry => entry.entryType === 'mark' && entry.name === String(value));
    if (!mark) throw new DOMException(`The "${value}" performance mark has not been set`, 'SyntaxError');
    return mark.startTime;
  };
  const clear = (type, name) => { for (let i = timeline.length - 1; i >= 0; i--) if (timeline[i].entryType === type && (name === undefined || timeline[i].name === String(name))) timeline.splice(i, 1); };
  class PerformanceNodeEntry extends PerformanceEntry {
    #detail;
    constructor(name, start, duration, detail) { super(name, 'function', start, duration); this.#detail = detail; }
    get detail() { return this.#detail; }
  }
  const timerify = (fn, { histogram } = {}) => {
    if (typeof fn !== 'function') throw Object.assign(new TypeError(`The "fn" argument must be of type function. Received ${fn === null ? 'null' : `type ${typeof fn} (${String(fn)})`}`), { code: 'ERR_INVALID_ARG_TYPE' });
    const done = (start, args) => {
      const duration = performance.now() - start, entry = new PerformanceNodeEntry(fn.name, start, duration, args);
      histogram?.record(Math.max(1, Math.round(duration * 1e6)));
      for (const notify of observers.values()) notify(entry);
    };
    const timed = function(...args) {
      const start = performance.now(), result = new.target ? Reflect.construct(fn, args) : fn.apply(this, args);
      if (typeof result?.finally !== 'function') { done(start, args); return result; }
      return result.finally(() => done(start, args));
    };
    return Object.defineProperties(timed, { name: { value: `timerified ${fn.name}` }, length: { value: fn.length } });
  };
  const eventLoopUtilization = (first, second) => {
    let idle, active;
    if (second) { idle = first.idle - second.idle; active = first.active - second.active; }
    else {
      const time = call('node.loop');
      if (!time) return { idle: 0, active: 0, utilization: 0 };
      idle = time[0] - (first?.idle ?? 0); active = time[1] - time[0] - (first?.active ?? 0);
    }
    return { idle, active, utilization: active / (idle + active) };
  };
  class Performance { constructor() { throw new TypeError('Illegal constructor'); } }
  globalThis.performance = Object.freeze(Object.setPrototypeOf({ timeOrigin: clock.origin, now: () => native() - base, toJSON() { return { timeOrigin: this.timeOrigin }; },
    mark: (name, options) => record(new PerformanceMark(name, options)),
    measure(name, start, end) {
      let duration, detail = null;
      if (start !== null && typeof start === 'object') ({ start, end, duration, detail = null } = start);
      const to = end !== undefined ? at(end) : start !== undefined && duration !== undefined ? at(start) + duration : performance.now();
      const from = start !== undefined ? at(start) : duration !== undefined ? to - duration : 0;
      return record(new PerformanceMeasure(illegal, name, from, to - from, detail));
    },
    clearMarks: name => clear('mark', name), clearMeasures: name => clear('measure', name),
    getEntries: () => [...timeline].sort((a, b) => a.startTime - b.startTime),
    getEntriesByName: (name, type) => select(timeline, name, type).sort((a, b) => a.startTime - b.startTime),
    getEntriesByType: type => select(timeline, undefined, type).sort((a, b) => a.startTime - b.startTime),
    ...(node ? { timerify, eventLoopUtilization } : {}),
  }, Performance.prototype));
  Object.assign(globalThis, { Performance, PerformanceEntry, PerformanceMark, PerformanceMeasure, PerformanceObserver, PerformanceObserverEntryList });
  globalThis.scheduler = Object.freeze({
    wait(delay, options = {}) {
      return new Promise((resolve, reject) => {
        const signal = options.signal;
        if (signal !== undefined && !(signal instanceof AbortSignal)) throw new TypeError('Invalid abort signal');
        signal?.throwIfAborted();
        const aborted = () => { clearTimeout(timer); reject(signal.reason); };
        const timer = setTimeout(() => { signal?.removeEventListener('abort', aborted); resolve(); }, delay);
        signal?.addEventListener('abort', aborted, { once: true });
      });
    },
  });
  globalThis.atob = function atob(value) {
    if (!arguments.length) throw new TypeError('Base64 data is required');
    try { return codec.decode(Uint8Array.fromBase64(String(value)), 1); } catch { throw new DOMException('Invalid Base64 data', 'InvalidCharacterError'); }
  };
  globalThis.btoa = function btoa(value) {
    if (!arguments.length) throw new TypeError('Binary data is required');
    const text = String(value);
    if (/[^\x00-\xff]/.test(text)) throw new DOMException('Expected a binary string', 'InvalidCharacterError');
    return codec.encode(text, 1).toBase64();
  };
  const codecs = typeof __appsFinalizers === 'function' ? new __appsFinalizers(id => call('web.platform.compression.close', id)) : null;
  const transform = (format, decompress) => {
    format = String(format);
    if (!['gzip', 'deflate', 'deflate-raw'].includes(format)) throw new TypeError('Invalid compression format');
    const state = { id: call('web.platform.compression.new', format, decompress) };
    codecs?.register(state, state.id, state);
    const close = () => { if (state.id !== null) { call('web.platform.compression.close', state.id); state.id = null; codecs?.unregister(state); } };
    const write = (data, controller, finish) => {
      const input = data instanceof ArrayBuffer ? new Uint8Array(data) : ArrayBuffer.isView(data) ? new Uint8Array(data.buffer, data.byteOffset, data.byteLength) : null;
      if (!input) { close(); throw new TypeError('Expected an ArrayBuffer or view'); }
      let offset = 0;
      try {
        while (true) {
          const end = Math.min(offset + 65536, input.length);
          const result = call('web.platform.compression.write', state.id, input.subarray(offset, end), finish && end === input.length);
          offset += result.read;
          const output = codecBytes.decode(result.bytes);
          if (output.length) controller.enqueue(output);
          if (result.done) { if (offset < input.length) throw new TypeError('Trailing compressed data'); break; }
          if (offset < input.length || output.length === 65536 || finish) continue;
          break;
        }
      } catch (error) { close(); throw new TypeError(error.message); }
    };
    return new TransformStream({
      transform(data, controller) { write(data, controller, false); },
      flush(controller) { try { write(new Uint8Array(), controller, true); } finally { close(); } },
      cancel: close,
    });
  };
  class CompressionStream {
    #stream;
    constructor(format) { this.#stream = transform(format, false); }
    get readable() { return this.#stream.readable; }
    get writable() { return this.#stream.writable; }
  }
  class DecompressionStream {
    #stream;
    constructor(format) { this.#stream = transform(format, true); }
    get readable() { return this.#stream.readable; }
    get writable() { return this.#stream.writable; }
  }
  class PromiseRejectionEvent extends Event {
    constructor(type, options) {
      if (!(options?.promise instanceof Promise)) throw new TypeError('Expected a Promise');
      super(type, options);
      Object.defineProperties(this, { promise: { value: options.promise, enumerable: true }, reason: { value: options.reason, enumerable: true } });
    }
  }
  const rejections = new WeakMap(), nativeThen = Promise.prototype.then;
  let tracking = false;
  globalThis.__appsRejection = (promise, reason) => {
    // A custom `then` slows every promise in JSC, so it is installed only after the first rejection.
    if (!tracking) {
      tracking = true;
      Object.defineProperty(Promise.prototype, 'then', { configurable: true, writable: true, value: function then(onFulfilled, onRejected) {
        const record = rejections.get(this);
        const result = Reflect.apply(nativeThen, this, [onFulfilled, onRejected]);
        if (record && !record.handled) {
          record.handled = true;
          if (record.reported) setTimeout(() => dispatchEvent(new PromiseRejectionEvent('rejectionhandled', { promise: this, reason: record.reason })), 0);
        }
        return result;
      } });
    }
    if (rejections.has(promise)) return;
    const record = { reason, handled: false, reported: false }; rejections.set(promise, record);
    setTimeout(() => {
      if (record.handled) return;
      record.reported = true;
      const event = new PromiseRejectionEvent('unhandledrejection', { promise, reason, cancelable: true });
      if (dispatchEvent(event)) console.error('Unhandled promise rejection:', String(reason));
    }, 0);
  };
  Object.assign(globalThis, { CompressionStream, DecompressionStream, PromiseRejectionEvent });
  globalThis.navigator ??= {};
  Object.defineProperty(navigator, 'userAgent', { value: 'apps/' + clock.version, enumerable: true });
  Object.defineProperty(navigator, 'platform', { value: clock.platform, enumerable: true });
};
