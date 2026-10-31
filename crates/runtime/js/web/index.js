globalThis.__appsWeb = call => {
  const disturbed = globalThis.__appsStreamDisturbed;
  delete globalThis.__appsStreamDisturbed;
  const bytes = value => {
    if (value instanceof ArrayBuffer) return new Uint8Array(value);
    if (ArrayBuffer.isView(value)) return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
    throw new TypeError('Expected an ArrayBuffer or a view');
  };
  const encodeBytes = value => value;
  const decodeBytes = value => typeof value === 'string' ? (Uint8Array.fromBase64 ? Uint8Array.fromBase64(value) : Uint8Array.from(atob(value), c => c.charCodeAt(0))) : value instanceof Uint8Array ? value : new Uint8Array(value);
  globalThis.__appsBytes = { encode: encodeBytes, decode: decodeBytes, memory: body => memoryBodies.get(body) };
  class DOMException extends Error {
    constructor(message = '', name = 'Error') { super(String(message)); this.name = String(name); }
    get code() { return ({ InvalidCharacterError: 5, NotFoundError: 8, TypeMismatchError: 17, NoModificationAllowedError: 7, InvalidModificationError: 13, AbortError: 20, QuotaExceededError: 22, NotSupportedError: 9, InvalidStateError: 11, SyntaxError: 12, TimeoutError: 23, DataCloneError: 25 })[this.name] ?? 0; }
  }
  const eventStates = new WeakMap(), eventTargets = new WeakMap();
  class Event {
    constructor(type, options = {}) {
      if (!arguments.length) throw new TypeError('Event type is required');
      eventStates.set(this, { type: String(type), bubbles: !!options.bubbles, cancelable: !!options.cancelable, composed: !!options.composed,
        defaultPrevented: false, target: null, currentTarget: null, eventPhase: 0, timeStamp: Date.now() - (globalThis.performance?.timeOrigin ?? Date.now()), dispatching: false, stopped: false, immediate: false, passive: false });
    }
    preventDefault() { const s = eventStates.get(this); if (s.cancelable && !s.passive) s.defaultPrevented = true; }
    stopPropagation() { eventStates.get(this).stopped = true; }
    stopImmediatePropagation() { const s = eventStates.get(this); s.stopped = s.immediate = true; }
    composedPath() { const s = eventStates.get(this); return s.dispatching ? [...(s.path ?? [s.target])] : []; }
    get cancelBubble() { return eventStates.get(this).stopped; }
    set cancelBubble(value) { if (value) this.stopPropagation(); }
    get returnValue() { return !this.defaultPrevented; }
    set returnValue(value) { if (!value) this.preventDefault(); }
    get isTrusted() { return !!eventStates.get(this).trusted; }
    initEvent(type, bubbles = false, cancelable = false) {
      const s = eventStates.get(this); if (s.dispatching) return;
      Object.assign(s, { type: String(type), bubbles: !!bubbles, cancelable: !!cancelable, defaultPrevented: false, target: null, stopped: false, immediate: false });
    }
  }
  for (const name of ['type', 'bubbles', 'cancelable', 'composed', 'defaultPrevented', 'target', 'currentTarget', 'eventPhase', 'timeStamp']) Object.defineProperty(Event.prototype, name, { enumerable: true, get() { return eventStates.get(this)[name]; } });
  for (const [name, value] of Object.entries({ NONE: 0, CAPTURING_PHASE: 1, AT_TARGET: 2, BUBBLING_PHASE: 3 })) {
    Object.defineProperty(Event, name, { value, enumerable: true }); Object.defineProperty(Event.prototype, name, { value, enumerable: true });
  }
  class CustomEvent extends Event {
    constructor(type, options = {}) { super(type, options); this.detail = options.detail ?? null; }
    initCustomEvent(type, bubbles, cancelable, detail = null) { this.initEvent(type, bubbles, cancelable); this.detail = detail; }
  }
  class EventTarget {
    constructor() { eventTargets.set(this, new Map()); }
    addEventListener(type, callback, options = {}) {
      if (callback == null) return;
      if (typeof callback !== 'function' && typeof callback !== 'object') throw new TypeError('Invalid event listener');
      type = String(type); options ??= {};
      const capture = typeof options === 'boolean' ? options : !!options.capture;
      const signal = options.signal;
      if (signal !== undefined && !(signal instanceof AbortSignal)) throw new TypeError('Invalid abort signal');
      if (signal?.aborted) return;
      const target = eventTargets.get(this), list = target.get(type) ?? [];
      if (list.some(item => item.callback === callback && item.capture === capture)) return;
      const item = { callback, capture, once: !!options.once, passive: !!options.passive, signal };
      if (signal) { item.abort = () => this.removeEventListener(type, callback, capture); signal.addEventListener('abort', item.abort, { once: true }); }
      list.push(item); target.set(type, list);
    }
    removeEventListener(type, callback, options = {}) {
      const capture = typeof options === 'boolean' ? options : !!options?.capture;
      const target = eventTargets.get(this), list = target.get(String(type)) ?? [];
      for (const item of list) if (item.callback === callback && item.capture === capture) item.signal?.removeEventListener('abort', item.abort);
      target.set(String(type), list.filter(item => item.callback !== callback || item.capture !== capture));
    }
    dispatchEvent(event) {
      if (!(event instanceof Event)) throw new TypeError('Expected an Event');
      const s = eventStates.get(event);
      if (s.dispatching) throw new DOMException('Event is already in dispatch', 'InvalidStateError');
      Object.assign(s, { target: this, currentTarget: this, eventPhase: 2, dispatching: true });
      const target = eventTargets.get(this), list = [...(target.get(event.type) ?? [])];
      try {
        for (const item of list) {
          if (s.immediate) break;
          if (!(target.get(event.type) ?? []).includes(item)) continue;
          if (item.once) this.removeEventListener(event.type, item.callback, item.capture);
          s.passive = item.passive;
          try { if (typeof item.callback === 'function') item.callback.call(this, event); else item.callback.handleEvent(event); }
          catch (error) { console.error(error); }
          s.passive = false;
        }
        try { if (!s.immediate && typeof this['on' + event.type] === 'function') this['on' + event.type](event); }
        catch (error) { console.error(error); }
      } finally { Object.assign(s, { currentTarget: null, eventPhase: 0, dispatching: false, stopped: false, immediate: false, passive: false }); }
      return !event.defaultPrevented;
    }
  }
  eventTargets.set(globalThis, new Map());
  for (const name of ['addEventListener', 'removeEventListener', 'dispatchEvent']) globalThis[name] = EventTarget.prototype[name].bind(globalThis);
  const signals = new WeakMap();
  const abort = (signal, reason) => {
    const state = signals.get(signal);
    if (state.aborted) return;
    state.aborted = true; state.reason = reason === undefined ? new DOMException('The operation was aborted', 'AbortError') : reason;
    signal.dispatchEvent(new Event('abort'));
  };
  class AbortSignal extends EventTarget {
    constructor(token) { super(); if (token !== signals) throw new TypeError('Illegal constructor'); signals.set(this, { aborted: false, reason: undefined }); this.onabort = null; }
    get aborted() { return signals.get(this).aborted; }
    get reason() { return signals.get(this).reason; }
    throwIfAborted() { if (this.aborted) throw this.reason; }
    static abort(reason) { const signal = new AbortSignal(signals); abort(signal, reason); return signal; }
    static timeout(delay) {
      delay = Number(delay);
      if (!Number.isSafeInteger(delay) || delay < 0) throw new RangeError('Invalid timeout');
      const signal = new AbortSignal(signals);
      setTimeout(() => abort(signal, new DOMException('The operation timed out', 'TimeoutError')), delay);
      return signal;
    }
    static any(inputs) {
      const list = [...inputs], signal = new AbortSignal(signals), listeners = [];
      for (const input of list) if (!(input instanceof AbortSignal)) throw new TypeError('Expected AbortSignal');
      for (const input of list) {
        if (input.aborted) { abort(signal, input.reason); break; }
        const listener = () => abort(signal, input.reason);
        input.addEventListener('abort', listener, { once: true }); listeners.push([input, listener]);
      }
      const cleanup = () => { for (const [input, listener] of listeners) input.removeEventListener('abort', listener); };
      if (signal.aborted) cleanup(); else signal.addEventListener('abort', cleanup, { once: true });
      return signal;
    }
  }
  class AbortController {
    #signal = new AbortSignal(signals);
    get signal() { return this.#signal; }
    abort(reason) { abort(this.#signal, reason); }
  }
  class TextEncoder {
    get encoding() { return 'utf-8'; }
    encode(input = '') {
      const text = String(input), output = new Uint8Array(text.length * 3);
      const { written } = this.encodeInto(text, output);
      return output.slice(0, written);
    }
    encodeInto(input, output) {
      if (!(output instanceof Uint8Array)) throw new TypeError('Expected Uint8Array');
      const text = String(input);
      let read = 0, written = 0;
      while (read < text.length) {
        let point = text.codePointAt(read);
        const units = point > 65535 ? 2 : 1;
        if (point >= 0xD800 && point <= 0xDFFF) point = 0xFFFD;
        const length = point < 128 ? 1 : point < 2048 ? 2 : point < 65536 ? 3 : 4;
        if (written + length > output.length) break;
        if (length === 1) output[written++] = point;
        else {
          output[written++] = (length === 2 ? 192 : length === 3 ? 224 : 240) | (point >> (6 * (length - 1)));
          for (let shift = 6 * (length - 2); shift >= 0; shift -= 6) output[written++] = 128 | ((point >> shift) & 63);
        }
        read += units;
      }
      return { read, written };
    }
  }
  class TextDecoder {
    #pending = []; #started = false; #fatal; #ignoreBOM;
    constructor(label = 'utf-8', options = {}) {
      if (!['utf-8', 'utf8', 'unicode-1-1-utf-8'].includes(String(label).trim().toLowerCase())) throw new RangeError('Only UTF-8 decoding is supported');
      this.#fatal = !!options.fatal; this.#ignoreBOM = !!options.ignoreBOM;
    }
    get encoding() { return 'utf-8'; }
    get fatal() { return this.#fatal; }
    get ignoreBOM() { return this.#ignoreBOM; }
    decode(input = new Uint8Array(), options = {}) {
      let data = bytes(input);
      if (this.#pending.length) { const combined = new Uint8Array(this.#pending.length + data.length); combined.set(this.#pending); combined.set(data, this.#pending.length); data = combined; }
      try {
        let [text, pending] = call('web.decode', encodeBytes(data), !!options.stream, this.#fatal);
        this.#pending = pending ? data.slice(data.length - pending) : [];
        if (text && !this.#started) { this.#started = true; if (!this.#ignoreBOM && text.charCodeAt(0) === 0xFEFF) text = text.slice(1); }
        if (!options.stream) { this.#pending = []; this.#started = false; }
        return text;
      } catch (error) { this.#pending = []; this.#started = false; throw new TypeError(error.message); }
    }
  }
  class TextEncoderStream {
    constructor() {
      const encoder = new TextEncoder(); let pending = '';
      const stream = new TransformStream({
        transform(value, controller) {
          let text = pending + String(value); pending = '';
          const last = text.charCodeAt(text.length - 1);
          if (last >= 0xD800 && last <= 0xDBFF) { pending = text.slice(-1); text = text.slice(0, -1); }
          if (text) controller.enqueue(encoder.encode(text));
        },
        flush(controller) { if (pending) controller.enqueue(encoder.encode(pending)); },
      });
      this.readable = stream.readable; this.writable = stream.writable;
    }
    get encoding() { return 'utf-8'; }
  }
  class TextDecoderStream {
    constructor(label, options) {
      const decoder = this._decoder = new TextDecoder(label, options);
      const stream = new TransformStream({
        transform(value, controller) { const text = decoder.decode(value, { stream: true }); if (text) controller.enqueue(text); },
        flush(controller) { const text = decoder.decode(); if (text) controller.enqueue(text); },
      });
      this.readable = stream.readable; this.writable = stream.writable;
    }
    get encoding() { return this._decoder.encoding; }
    get fatal() { return this._decoder.fatal; }
    get ignoreBOM() { return this._decoder.ignoreBOM; }
  }
  const params = new WeakMap(), urls = new WeakMap();
  const usv = value => { const text = String(value); return text.toWellFormed ? text.toWellFormed() : text.replace(/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/g, '\uFFFD'); };
  const updateParams = value => { const state = params.get(value); state.update?.(); };
  class URLSearchParams {
    constructor(input = '') {
      let list;
      if (typeof input === 'object' && input !== null) {
        if (input[Symbol.iterator]) list = Array.from(input, pair => { const values = Array.from(pair); if (values.length !== 2) throw new TypeError('Expected a pair'); return values.map(usv); });
        else list = Object.entries(input).map(pair => pair.map(usv));
      } else list = call('web.params.parse', usv(input).replace(/^\?/, ''));
      params.set(this, { list });
    }
    get size() { return params.get(this).list.length; }
    append(name, value) { params.get(this).list.push([usv(name), usv(value)]); updateParams(this); }
    delete(name, value) { name = usv(name); if (value !== undefined) value = usv(value); const s = params.get(this); s.list = s.list.filter(pair => pair[0] !== name || (value !== undefined && pair[1] !== value)); updateParams(this); }
    get(name) { name = usv(name); return params.get(this).list.find(pair => pair[0] === name)?.[1] ?? null; }
    getAll(name) { name = usv(name); return params.get(this).list.filter(pair => pair[0] === name).map(pair => pair[1]); }
    has(name, value) { name = usv(name); if (value !== undefined) value = usv(value); return params.get(this).list.some(pair => pair[0] === name && (value === undefined || pair[1] === value)); }
    set(name, value) {
      name = usv(name); value = usv(value); const s = params.get(this), index = s.list.findIndex(pair => pair[0] === name);
      if (index < 0) s.list.push([name, value]);
      else { s.list[index][1] = value; s.list = s.list.filter((pair, i) => pair[0] !== name || i === index); }
      updateParams(this);
    }
    sort() { params.get(this).list.sort((a, b) => a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0); updateParams(this); }
    *entries() { for (let i = 0; i < params.get(this).list.length; i++) yield params.get(this).list[i].slice(); }
    *keys() { for (const [key] of this) yield key; }
    *values() { for (const [, value] of this) yield value; }
    forEach(fn, thisArg) { for (const [key, value] of this) fn.call(thisArg, value, key, this); }
    [Symbol.iterator]() { return this.entries(); }
    toString() { return call('web.params.stringify', params.get(this).list); }
  }
  const parseURL = (...args) => { try { return call('web.url', ...args.map(value => typeof value === 'string' ? usv(value) : value)); } catch (error) { throw new TypeError(error.message); } };
  class URL {
    constructor(input, base) {
      const data = parseURL(String(input), base === undefined ? null : String(base)), searchParams = new URLSearchParams(data.search);
      urls.set(this, { data, searchParams });
      params.get(searchParams).update = () => { const state = urls.get(this); state.data = parseURL(state.data.href, null, 'search', searchParams.toString()); };
    }
    get origin() { return urls.get(this).data.origin; }
    get searchParams() { return urls.get(this).searchParams; }
    toString() { return this.href; }
    toJSON() { return this.href; }
    static canParse(input, base) { try { new URL(input, base); return true; } catch { return false; } }
    static parse(input, base) { try { return new URL(input, base); } catch { return null; } }
  }
  for (const key of ['href', 'protocol', 'username', 'password', 'host', 'hostname', 'port', 'pathname', 'search', 'hash']) Object.defineProperty(URL.prototype, key, {
    enumerable: true, configurable: true,
    get() { return urls.get(this).data[key]; },
    set(value) { const state = urls.get(this); state.data = parseURL(state.data.href, null, key, String(value)); if (key === 'search' || key === 'href') params.get(state.searchParams).list = call('web.params.parse', state.data.search.slice(1)); },
  });
  const crypto = globalThis.__appsCrypto(call, encodeBytes, decodeBytes, {
    getRandomValues(array) {
      if (!ArrayBuffer.isView(array) || array instanceof DataView || array instanceof Float32Array || array instanceof Float64Array || (typeof Float16Array !== 'undefined' && array instanceof Float16Array)) throw new DOMException('Expected an integer typed array', 'TypeMismatchError');
      if (array.byteLength > 65536) throw new DOMException('Random byte quota exceeded', 'QuotaExceededError');
      bytes(array).set(decodeBytes(call('web.random', array.byteLength))); return array;
    },
    randomUUID() {
      const values = decodeBytes(call('web.random', 16)); values[6] = (values[6] & 15) | 64; values[8] = (values[8] & 63) | 128;
      const hex = Array.from(values, value => value.toString(16).padStart(2, '0')).join('');
      return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
    },
    subtle: Object.freeze({
      async digest(algorithm, input) {
        const name = String(typeof algorithm === 'string' ? algorithm : algorithm?.name).toUpperCase();
        const data = bytes(input);
        if (!['SHA-1', 'SHA-256', 'SHA-384', 'SHA-512'].includes(name)) throw new DOMException('Unsupported digest algorithm', 'NotSupportedError');
        return decodeBytes(call('web.digest', name, encodeBytes(data))).buffer;
      },
    }),
  });
  delete globalThis.__appsCrypto;
  const headers = new WeakMap(), immutable = new WeakSet();
  const headerName = value => { value = String(value); if (!/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(value)) throw new TypeError('Invalid header name'); return value.toLowerCase(); };
  const headerValue = value => { value = String(value); if (/[^\x00-\xFF]|[\0\r\n]/.test(value)) throw new TypeError('Invalid header value'); return value.replace(/^[\t ]+|[\t ]+$/g, ''); };
  class Headers {
    constructor(input = {}) {
      headers.set(this, []);
      if (input == null) throw new TypeError('Invalid headers');
      if (input[Symbol.iterator]) for (const pair of input) { const values = Array.from(pair); if (values.length !== 2) throw new TypeError('Expected a header pair'); this.append(...values); }
      else for (const [key, value] of Object.entries(input)) this.append(key, value);
    }
    append(name, value) { name = headerName(name); value = headerValue(value); if (immutable.has(this)) throw new TypeError('Headers are immutable'); headers.get(this).push([name, value]); }
    set(name, value) { name = headerName(name); value = headerValue(value); if (immutable.has(this)) throw new TypeError('Headers are immutable'); headers.set(this, headers.get(this).filter(pair => pair[0] !== name)); headers.get(this).push([name, value]); }
    delete(name) { name = headerName(name); if (immutable.has(this)) throw new TypeError('Headers are immutable'); headers.set(this, headers.get(this).filter(pair => pair[0] !== name)); }
    get(name) { name = headerName(name); const values = headers.get(this).filter(pair => pair[0] === name).map(pair => pair[1]); return values.length ? values.join(', ') : null; }
    has(name) { name = headerName(name); return headers.get(this).some(pair => pair[0] === name); }
    getSetCookie() { return headers.get(this).filter(pair => pair[0] === 'set-cookie').map(pair => pair[1]); }
    *entries() {
      const list = new Map();
      for (const [key, value] of headers.get(this)) { const values = list.get(key) ?? []; values.push(value); list.set(key, values); }
      for (const key of [...list.keys()].sort()) {
        if (key === 'set-cookie') for (const value of list.get(key)) yield [key, value];
        else yield [key, list.get(key).join(', ')];
      }
    }
    *keys() { for (const [key] of this) yield key; }
    *values() { for (const [, value] of this) yield value; }
    forEach(fn, thisArg) { for (const [key, value] of this) fn.call(thisArg, value, key, this); }
    [Symbol.iterator]() { return this.entries(); }
  }
  const blobs = new WeakMap(), objectURLs = new Map();
  URL.createObjectURL = value => { if (!(value instanceof Blob)) throw new TypeError('Expected a Blob'); const url = 'blob:' + (globalThis.location?.origin ?? 'null') + '/' + crypto.randomUUID(); objectURLs.set(url, value); return url; };
  URL.revokeObjectURL = url => { objectURLs.delete(String(url)); };
  class Blob {
    constructor(parts = [], options = {}) {
      const chunks = Array.from(parts, part => part instanceof Blob ? blobs.get(part) : ArrayBuffer.isView(part) || part instanceof ArrayBuffer ? bytes(part) : new TextEncoder().encode(String(part)));
      const output = new Uint8Array(chunks.reduce((size, chunk) => size + chunk.length, 0));
      let offset = 0; for (const chunk of chunks) { output.set(chunk, offset); offset += chunk.length; }
      blobs.set(this, output);
      const type = String(options.type ?? '');
      this._type = /[^\x20-\x7E]/.test(type) ? '' : type.toLowerCase();
    }
    get size() { return blobs.get(this).length; }
    get type() { return this._type; }
    async arrayBuffer() { return blobs.get(this).slice().buffer; }
    async bytes() { return blobs.get(this).slice(); }
    async text() { return new TextDecoder().decode(blobs.get(this)); }
    slice(start = 0, end = this.size, type = '') { return new Blob([blobs.get(this).slice(start, end)], { type }); }
    stream() {
      const data = blobs.get(this); let offset = 0;
      return new ReadableStream({ type: 'bytes', pull(controller) { if (offset < data.length) { const next = Math.min(offset + 65536, data.length); controller.enqueue(data.slice(offset, next)); offset = next; } if (offset === data.length) controller.close(); } });
    }
  }
  class File extends Blob {
    constructor(parts, name, options = {}) { super(parts, options); this.name = usv(name); this.lastModified = Number(options.lastModified ?? Date.now()); }
  }
  const forms = new WeakMap();
  const formValue = (value, filename) => {
    if (filename !== undefined && !(value instanceof Blob)) throw new TypeError('Filename requires a Blob');
    if (!(value instanceof Blob)) return usv(value);
    return value instanceof File && filename === undefined ? value : new File([value], filename ?? 'blob', { type: value.type });
  };
  class FormData {
    constructor(form, submitter = null) { forms.set(this, []); if (form !== undefined) { if (typeof HTMLFormElement !== 'function' || !(form instanceof HTMLFormElement)) throw new TypeError('Expected a form element'); for (const [name, value] of form._entries(submitter)) this.append(name, value); } }
    append(name, value, filename) { forms.get(this).push([usv(name), formValue(value, filename)]); }
    set(name, value, filename) { name = usv(name); value = formValue(value, filename); const list = forms.get(this), first = list.findIndex(pair => pair[0] === name); if (first < 0) list.push([name, value]); else { list[first] = [name, value]; forms.set(this, list.filter((pair, i) => pair[0] !== name || i === first)); } }
    delete(name) { name = usv(name); forms.set(this, forms.get(this).filter(pair => pair[0] !== name)); }
    get(name) { name = usv(name); return forms.get(this).find(pair => pair[0] === name)?.[1] ?? null; }
    getAll(name) { name = usv(name); return forms.get(this).filter(pair => pair[0] === name).map(pair => pair[1]); }
    has(name) { name = usv(name); return forms.get(this).some(pair => pair[0] === name); }
    *entries() { for (let i = 0; i < forms.get(this).length; i++) yield forms.get(this)[i].slice(); }
    *keys() { for (const [key] of this) yield key; }
    *values() { for (const [, value] of this) yield value; }
    forEach(fn, thisArg) { for (const [key, value] of this) fn.call(thisArg, value, key, this); }
    [Symbol.iterator]() { return this.entries(); }
  }
  const memoryBodies = new WeakMap(), bodies = new WeakMap(), responses = new WeakMap(), requests = new WeakMap();
  const extractBody = (input, headers) => {
    if (input == null) return null;
    if (input instanceof ReadableStream) { if (input.locked || disturbed(input)) throw new TypeError('Body is already used'); return input; }
    let blob, type = '';
    if (input instanceof Blob) { blob = input; type = input.type; }
    else if (input instanceof FormData) {
      const boundary = '----apps-' + crypto.randomUUID(), parts = [];
      const escape = value => value.replace(/\r?\n|\r/g, '\r\n').replace(/[\r\n"]/g, c => '%' + c.charCodeAt(0).toString(16).toUpperCase());
      for (const [name, value] of input) {
        parts.push(`--${boundary}\r\nContent-Disposition: form-data; name="${escape(name)}"`);
        if (value instanceof File) parts.push(`; filename="${escape(value.name)}"\r\nContent-Type: ${value.type || 'application/octet-stream'}\r\n\r\n`, value, '\r\n');
        else parts.push('\r\n\r\n', value.replace(/\r?\n|\r/g, '\r\n'), '\r\n');
      }
      parts.push(`--${boundary}--\r\n`); blob = new Blob(parts); type = 'multipart/form-data; boundary=' + boundary;
    } else if (ArrayBuffer.isView(input) || input instanceof ArrayBuffer) blob = new Blob([input]);
    else { blob = new Blob([String(input)]); type = input instanceof URLSearchParams ? 'application/x-www-form-urlencoded;charset=UTF-8' : 'text/plain;charset=UTF-8'; }
    if (type && !headers.has('content-type')) headers.set('content-type', type);
    const stream = blob.stream(); memoryBodies.set(stream, blobs.get(blob)); return stream;
  };
  const consume = async (object, signal) => {
    const body = object.body;
    if (!body) return new Uint8Array();
    if (body.locked || disturbed(body)) throw new TypeError('Body is already used');
    const reader = body.getReader(), chunks = []; let size = 0;
    const onAbort = () => reader.cancel(signal.reason).catch(() => {});
    signal?.addEventListener('abort', onAbort, { once: true });
    try {
      while (true) {
        signal?.throwIfAborted();
        const { value, done } = await reader.read();
        signal?.throwIfAborted();
        if (done) break;
        if (!(value instanceof Uint8Array)) { await reader.cancel(new TypeError('Body chunks must be Uint8Array')); throw new TypeError('Body chunks must be Uint8Array'); }
        chunks.push(value); size += value.byteLength;
      }
      const result = new Uint8Array(size); let offset = 0;
      for (const chunk of chunks) { result.set(chunk, offset); offset += chunk.byteLength; }
      return result;
    } finally { signal?.removeEventListener('abort', onAbort); reader.releaseLock(); }
  };
  const cloneBody = object => {
    const body = object.body;
    if (body && (body.locked || disturbed(body))) throw new TypeError('Body is already used');
    if (!body) return null;
    const branches = body.tee();
    if (memoryBodies.has(body)) { memoryBodies.set(branches[0], memoryBodies.get(body)); memoryBodies.set(branches[1], memoryBodies.get(body)); }
    bodies.set(object, branches[0]); return branches[1];
  };
  class Body {
    get body() { return bodies.get(this); }
    get bodyUsed() { return !!this.body && disturbed(this.body); }
    async arrayBuffer() { return (await consume(this)).buffer; }
    bytes() { return consume(this); }
    async text() { return new TextDecoder().decode(await consume(this)); }
    async json() { return JSON.parse(await this.text()); }
    async blob() { return new Blob([await consume(this)], { type: this.headers.get('content-type') ?? '' }); }
    async formData() {
      const type = this.headers.get('content-type') ?? '';
      if (!/^application\/x-www-form-urlencoded(?:;|$)/i.test(type)) throw new DOMException('Only URL-encoded form responses are supported', 'NotSupportedError');
      const result = new FormData(); for (const [name, value] of new URLSearchParams(await this.text())) result.append(name, value); return result;
    }
  }
  class Request extends Body {
    constructor(input, init = {}) {
      super(); const source = input instanceof Request ? input : null;
      const url = new URL(source ? source.url : String(input));
      if (url.username || url.password) throw new TypeError('Request URLs cannot contain credentials');
      let method = String(init.method ?? source?.method ?? 'GET');
      if (!/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(method) || ['CONNECT', 'TRACE', 'TRACK'].includes(method.toUpperCase())) throw new TypeError('Invalid request method');
      if (['DELETE', 'GET', 'HEAD', 'OPTIONS', 'POST', 'PUT'].includes(method.toUpperCase())) method = method.toUpperCase();
      const headers = new Headers(init.headers ?? source?.headers);
      const redirect = init.redirect ?? source?.redirect ?? 'follow';
      if (!['follow', 'error', 'manual'].includes(redirect)) throw new TypeError('Invalid redirect mode');
      const signal = init.signal === undefined ? source?.signal : init.signal;
      if (signal != null && !(signal instanceof AbortSignal)) throw new TypeError('Invalid abort signal');
      const controller = new AbortController();
      if (signal?.aborted) controller.abort(signal.reason);
      else signal?.addEventListener('abort', () => controller.abort(signal.reason), { once: true });
      let body = init.body ?? null;
      if (body === null && source?.body) {
        if (source.body.locked || source.bodyUsed) throw new TypeError('Body is already used');
        body = source.body.pipeThrough(new TransformStream());
        if (memoryBodies.has(source.body)) memoryBodies.set(body, memoryBodies.get(source.body));
      }
      if (body !== null && ['GET', 'HEAD'].includes(method)) throw new TypeError('GET and HEAD requests cannot have a body');
      const mode = init.mode ?? source?.mode ?? 'cors', credentials = init.credentials ?? source?.credentials ?? 'same-origin';
      if (!['cors', 'same-origin', 'no-cors', 'navigate'].includes(mode)) throw new TypeError('Invalid request mode');
      if (!['omit', 'same-origin', 'include'].includes(credentials)) throw new TypeError('Invalid credentials mode');
      if (init.duplex !== undefined && init.duplex !== 'half') throw new TypeError('Invalid duplex mode');
      requests.set(this, { url: url.href, method, headers, redirect, signal: controller.signal, mode, credentials, duplex: 'half' });
      bodies.set(this, extractBody(body, headers));
    }
    clone() {
      const body = cloneBody(this), state = requests.get(this);
      return new Request(state.url, { ...state, body });
    }
  }
  for (const key of ['url', 'method', 'headers', 'redirect', 'signal', 'mode', 'credentials', 'duplex']) Object.defineProperty(Request.prototype, key, { get() { return requests.get(this)[key]; }, enumerable: true });
  class Response extends Body {
    constructor(body = null, init = {}) {
      super(); const status = Number(init.status ?? 200), statusText = String(init.statusText ?? '');
      if (!Number.isInteger(status) || status < 200 || status > 599) throw new RangeError('Invalid response status');
      if (/[^\t\x20-\x7E\x80-\xFF]/.test(statusText)) throw new TypeError('Invalid status text');
      if (body !== null && [204, 205, 304].includes(status)) throw new TypeError('This status cannot have a body');
      const headers = new Headers(init.headers);
      bodies.set(this, extractBody(body, headers));
      responses.set(this, { status, statusText, headers, url: '', redirected: false, type: 'default' });
    }
    get ok() { return this.status >= 200 && this.status <= 299; }
    clone() {
      const body = cloneBody(this), result = new Response();
      bodies.set(result, body); responses.set(result, { ...responses.get(this), headers: new Headers(this.headers) });
      if (immutable.has(this.headers)) immutable.add(result.headers); return result;
    }
    static error() { const result = new Response(); Object.assign(responses.get(result), { status: 0, type: 'error' }); immutable.add(result.headers); return result; }
    static redirect(url, status = 302) {
      if (![301, 302, 303, 307, 308].includes(Number(status))) throw new RangeError('Invalid redirect status');
      const result = new Response(null, { status, headers: { location: new URL(url).href } }); immutable.add(result.headers); return result;
    }
    static json(data, init = {}) {
      const text = JSON.stringify(data); if (text === undefined) throw new TypeError('Value is not JSON serializable');
      const headers = new Headers(init.headers); if (!headers.has('content-type')) headers.set('content-type', 'application/json');
      return new Response(text, { ...init, headers });
    }
  }
  for (const key of ['status', 'statusText', 'headers', 'url', 'redirected', 'type']) Object.defineProperty(Response.prototype, key, { get() { return responses.get(this)[key]; }, enumerable: true });
  class MessageEvent extends Event {
    constructor(type, options = {}) { super(type, options); this.data = options.data ?? null; this.origin = options.origin ?? ''; this.ports = options.ports ?? []; this.source = options.source ?? null; this.lastEventId = options.lastEventId ?? ''; }
  }
  class CloseEvent extends Event {
    constructor(type, options = {}) { super(type, options); this.code = options.code ?? 0; this.reason = options.reason ?? ''; this.wasClean = !!options.wasClean; }
  }
  const sockets = new Map(), acceptSocket = Symbol();
  class WebSocket extends EventTarget {
    constructor(url, protocols = []) {
      super();
      this.binaryType = 'blob'; this.bufferedAmount = 0; this.readyState = 0; this.protocol = ''; this.extensions = '';
      if (url === acceptSocket) { this._socket = protocols; this.url = ''; }
      else {
        const address = new URL(url, globalThis.location?.href);
        if (address.protocol === 'http:') address.protocol = 'ws:';
        if (address.protocol === 'https:') address.protocol = 'wss:';
        if (!['ws:', 'wss:'].includes(address.protocol) || address.hash) throw new DOMException('Invalid WebSocket URL', 'SyntaxError');
        protocols = typeof protocols === 'string' ? [protocols] : Array.from(protocols, String);
        if (new Set(protocols).size !== protocols.length || protocols.some(p => !/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(p))) throw new DOMException('Invalid WebSocket protocols', 'SyntaxError');
        this.url = address.href;
        this._socket = call('web.socket.connect', this.url, protocols);
      }
      sockets.set(this._socket, this);
    }
    send(data) {
      if (this.readyState === 0) throw new DOMException('WebSocket is connecting', 'InvalidStateError');
      if (this.readyState !== 1) return;
      const binary = data instanceof Blob || data instanceof ArrayBuffer || ArrayBuffer.isView(data);
      this.bufferedAmount += call('web.socket.send', this._socket, data instanceof Blob ? encodeBytes(blobs.get(data)) : binary ? encodeBytes(bytes(data)) : String(data), binary);
    }
    close(code = 1000, reason = '') {
      if (code !== 1000 && !(code >= 3000 && code <= 4999)) throw new DOMException('Invalid close code', 'InvalidAccessError');
      reason = String(reason);
      if (new TextEncoder().encode(reason).length > 123) throw new DOMException('Close reason is too long', 'SyntaxError');
      if (this.readyState >= 2) return;
      if (this.readyState === 0) {
        call('web.socket.abort', this._socket); sockets.delete(this._socket); this.readyState = 3;
        queueMicrotask(() => this.dispatchEvent(new CloseEvent('close', { code: 1006 })));
      } else { call('web.socket.close', this._socket, code, reason); this.readyState = 2; }
    }
  }
  for (const [name, value] of Object.entries({ CONNECTING: 0, OPEN: 1, CLOSING: 2, CLOSED: 3 })) {
    Object.defineProperty(WebSocket, name, { value }); Object.defineProperty(WebSocket.prototype, name, { value });
  }
  globalThis.__appsAcceptSocket = id => new WebSocket(acceptSocket, id);
  const pending = new Map(), queued = [];
  const waiting = () => { for (const state of pending.values()) if (state.waiting) return true; return false; };
  const cleanup = (id, state) => {
    call('web.fetch.cancel', id); state.signal.removeEventListener('abort', state.onAbort);
    if (state.uploadReader) { const reader = state.uploadReader; state.uploadReader = null; reader.cancel(state.signal.reason).catch(() => {}).finally(() => reader.releaseLock()); }
    if (pending.delete(id)) queued.shift()?.();
  };
  const fetch = (input, init) => new Promise((resolve, reject) => {
    let request;
    try {
      request = new Request(input, init); request.signal.throwIfAborted();
      if (new URL(request.url).protocol === 'data:') {
        if (request.method !== 'GET') throw new TypeError('Data URLs require GET');
        const url=new URL(request.url), source=url.href.slice(5,url.hash?-url.hash.length:undefined), comma=source.indexOf(',');
        if(comma<0)throw new TypeError('Invalid data URL');
        let type=source.slice(0,comma), bytes=[];
        const raw=source.slice(comma+1);
        const encoder=new TextEncoder();
        for(let i=0;i<raw.length;){const code=raw.charCodeAt(i);if(code===37&&/^[0-9a-f]{2}$/i.test(raw.slice(i+1,i+3))){bytes.push(parseInt(raw.slice(i+1,i+3),16));i+=3;}else if(code<128){bytes.push(code);i++;}else{let end=i+1;while(end<raw.length&&raw.charCodeAt(end)>=128)end++;for(const byte of encoder.encode(raw.slice(i,end)))bytes.push(byte);i=end;}}
        if(/;base64$/i.test(type)){type=type.slice(0,-7);let text='';for(let i=0;i<bytes.length;i+=8192)text+=String.fromCharCode(...bytes.slice(i,i+8192));bytes=Uint8Array.from(atob(text),character=>character.charCodeAt(0));}
        if(!type)type='text/plain;charset=US-ASCII';else if(type.startsWith(';'))type='text/plain'+type;
        resolve(new Response(new Uint8Array(bytes),{headers:{'content-type':type}}));return;
      }
      if (new URL(request.url).protocol === 'blob:') {
        const blob = objectURLs.get(request.url); if (!blob || request.method !== 'GET') throw new TypeError('Invalid Blob URL request');
        resolve(new Response(blob, { headers: { 'content-type': blob.type, 'content-length': String(blob.size) } })); return;
      }
      if (!['http:', 'https:'].includes(new URL(request.url).protocol)) throw new TypeError('Fetch supports HTTP, HTTPS, Blob, and data URLs');
    }
    catch (error) { reject(error); return; }
    const signal = request.signal;
    let id;
    const state = { signal, resolve, reject, waiting: true, request, controller: null, pullDone: null };
    state.onAbort = () => {
      reject(signal.reason); state.controller?.deref()?.error(signal.reason); state.pullDone?.();
      if (id !== undefined) cleanup(id, state);
    };
    signal.addEventListener('abort', state.onAbort, { once: true });
    const start = data => {
      let connecting = 0; for (const other of pending.values()) if (!other.controller) connecting++;
      if (connecting >= 64 || pending.size >= 256) { queued.push(() => start(data)); return; }
      try {
        signal.throwIfAborted();
        const fields = [...request.headers].filter(([name]) => !['host', 'content-length', 'connection', 'transfer-encoding', 'upgrade'].includes(name));
        id = call('web.fetch.start', { url: request.url, method: request.method, headers: fields, redirect: request.redirect, body: encodeBytes(data), stream: !!state.uploadReader });
        pending.set(id, state);
      } catch (error) { if (state.uploadReader) { const reader = state.uploadReader; reader.cancel(error).catch(() => {}).finally(() => reader.releaseLock()); } signal.removeEventListener('abort', state.onAbort); reject(signal.aborted ? signal.reason : new TypeError(error.message)); queued.shift()?.(); }
    };
    if (request.body && !memoryBodies.has(request.body)) { state.uploadReader = request.body.getReader(); start(new Uint8Array()); }
    else if (request.body) consume(request, signal).then(start, error => { signal.removeEventListener('abort', state.onAbort); reject(error); });
    else start(new Uint8Array());
  });
  let beaconBytes = 0;
  globalThis.navigator ??= {};
  navigator.sendBeacon = (url, data = null) => {
    const request = new Request(url, { method: 'POST', body: data, credentials: 'include' });
    if (!['http:', 'https:'].includes(new URL(request.url).protocol)) throw new TypeError('Beacon requires HTTP or HTTPS');
    const body = memoryBodies.get(request.body);
    if (request.body && !body) throw new TypeError('Beacon does not accept stream bodies');
    const length = body?.byteLength ?? 0;
    if (beaconBytes + length > 65536) return false;
    beaconBytes += length;
    fetch(request).then(response => response.body?.cancel()).catch(() => {}).finally(() => { beaconBytes -= length; });
    return true;
  };
  const poll = () => {
    if (sockets.size) for (const event of call('web.socket.poll')) {
      const socket = sockets.get(event.id); if (!socket) continue;
      if (event.kind === 'open') { socket.readyState = 1; socket.protocol = event.protocol; socket.dispatchEvent(new Event('open')); }
      else if (event.kind === 'written') socket.bufferedAmount = Math.max(0, socket.bufferedAmount - event.bytes);
      else if (event.kind === 'message') socket.dispatchEvent(new MessageEvent('message', { data: event.text ?? (socket.binaryType === 'arraybuffer' ? decodeBytes(event.bytes).buffer : new Blob([decodeBytes(event.bytes)])) }));
      else {
        socket.readyState = 3; sockets.delete(event.id);
        if (event.kind === 'error') socket.dispatchEvent(new Event('error'));
        socket.dispatchEvent(new CloseEvent('close', { code: event.code ?? 1006, reason: event.reason ?? '', wasClean: event.clean }));
      }
    }
    for (const [id, state] of pending) if (state.controller && !state.controller.deref()) cleanup(id, state);
    if (waiting()) for (const event of call('web.fetch.poll')) {
      const state = pending.get(event.id); if (!state) continue;
      state.waiting = false;
      if (event.kind === 'upload') {
        state.waiting = true;
        const reader = state.uploadReader;
        if (!reader) continue;
        reader.read().then(({ value, done }) => {
          if (!pending.has(event.id) || state.uploadReader !== reader) return;
          if (!done && !(value instanceof Uint8Array)) throw new TypeError('Body chunks must be Uint8Array');
          call('web.fetch.upload', event.id, done ? null : encodeBytes(value));
          if (done) { reader.releaseLock(); state.uploadReader = null; }
        }).catch(error => { state.reject(error); cleanup(event.id, state); });
      } else if (event.kind === 'headers') {
        const manual = state.request.redirect === 'manual' && [301, 302, 303, 307, 308].includes(event.status);
        const empty = manual || state.request.method === 'HEAD' || [204, 205, 304].includes(event.status);
        const body = empty ? null : new ReadableStream({ type: 'bytes',
          start(controller) { state.controller = new WeakRef(controller); },
          pull() { state.waiting = true; call('web.fetch.pull', event.id); return new Promise(resolve => { state.pullDone = resolve; }); },
          cancel() { cleanup(event.id, state); state.pullDone?.(); },
        });
        const response = new Response(); bodies.set(response, body);
        Object.assign(responses.get(response), { status: manual ? 0 : event.status, statusText: manual ? '' : event.statusText,
          headers: new Headers(manual ? [] : event.headers), url: manual ? '' : event.url, redirected: event.redirected, type: manual ? 'opaqueredirect' : 'basic' });
        immutable.add(response.headers); state.resolve(response);
        if (empty) cleanup(event.id, state);
      } else if (event.kind === 'chunk') {
        if (event.bytes) state.controller.deref()?.enqueue(decodeBytes(event.bytes));
        state.pullDone?.(); state.pullDone = null;
      } else {
        if (event.kind === 'done') state.controller?.deref()?.close();
        else { const error = new TypeError(event.message); state.reject(error); state.controller?.deref()?.error(error); }
        state.pullDone?.(); cleanup(event.id, state);
      }
    }
    return poll.pending();
  };
  poll.pending = () => sockets.size > 0 || waiting();
  poll.eventState = event => eventStates.get(event);
  Object.assign(globalThis, { DOMException, Event, CustomEvent, EventTarget, AbortSignal, AbortController, TextEncoder, TextDecoder, TextEncoderStream,
    TextDecoderStream, URL, URLSearchParams, crypto, Headers, Request, Response, Blob, File, FormData, fetch, WebSocket, MessageEvent, CloseEvent });
  return poll;
};
