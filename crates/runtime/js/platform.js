globalThis.__appsPlatform = call => {
  const codecBytes = globalThis.__appsBytes;
  globalThis.self = globalThis;
  const native = globalThis.__appsNow;
  delete globalThis.__appsNow;
  const clock = call('web.platform.time'), base = native() - clock.now;
  globalThis.performance = Object.freeze({ timeOrigin: clock.origin, now: () => native() - base, toJSON() { return { timeOrigin: this.timeOrigin }; } });
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
    let text = String(value).replace(/[\t\n\f\r ]/g, '');
    if (text.length % 4 === 0) text = text.replace(/={1,2}$/, '');
    if (text.length % 4 === 1 || /[^A-Za-z0-9+/]/.test(text)) throw new DOMException('Invalid Base64 data', 'InvalidCharacterError');
    try { return call('web.platform.base64.decode', text); } catch { throw new DOMException('Invalid Base64 data', 'InvalidCharacterError'); }
  };
  globalThis.btoa = function btoa(value) {
    if (!arguments.length) throw new TypeError('Binary data is required');
    const text = String(value);
    if (/[^\x00-\xff]/.test(text)) throw new DOMException('Expected a binary string', 'InvalidCharacterError');
    return call('web.platform.base64.encode', text);
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
          const result = call('web.platform.compression.write', state.id, codecBytes.encode(input.subarray(offset, end)), finish && end === input.length);
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
  Object.defineProperty(Promise.prototype, 'then', { configurable: true, writable: true, value: function then(onFulfilled, onRejected) {
    const record = rejections.get(this);
    const result = Reflect.apply(nativeThen, this, [onFulfilled, onRejected]);
    if (record && !record.handled) {
      record.handled = true;
      if (record.reported) setTimeout(() => dispatchEvent(new PromiseRejectionEvent('rejectionhandled', { promise: this, reason: record.reason })), 0);
    }
    return result;
  } });
  globalThis.__appsRejection = (promise, reason) => {
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
