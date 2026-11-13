globalThis.__appsWeb = (call, codec) => {
  const disturbed = globalThis.__appsStreamDisturbed;
  delete globalThis.__appsStreamDisturbed;
  const bytes = value => {
    if (value instanceof ArrayBuffer) return new Uint8Array(value);
    if (ArrayBuffer.isView(value)) return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
    throw new TypeError('Expected an ArrayBuffer or a view');
  };
  const decodeBytes = value => typeof value === 'string' ? Uint8Array.fromBase64(value) : value instanceof Uint8Array ? value : new Uint8Array(value);
  globalThis.__appsBytes = { decode: decodeBytes, memory: body => memoryBodies.get(body) };
  const retries = globalThis.__appsRetries = new Set();
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
    preventDefault() { const state = eventStates.get(this); if (state.cancelable && !state.passive) state.defaultPrevented = true; }
    stopPropagation() { eventStates.get(this).stopped = true; }
    stopImmediatePropagation() { const state = eventStates.get(this); state.stopped = state.immediate = true; }
    composedPath() { const state = eventStates.get(this), path = state.path ?? [state.target]; return !state.dispatching ? [] : state.hidden ? path.filter(node => !state.hidden(node, state.currentTarget)) : [...path]; }
    get cancelBubble() { return eventStates.get(this).stopped; }
    set cancelBubble(value) { if (value) this.stopPropagation(); }
    get srcElement() { return eventStates.get(this).target; }
    get returnValue() { return !this.defaultPrevented; }
    set returnValue(value) { if (!value) this.preventDefault(); }
    get isTrusted() { return !!eventStates.get(this).trusted; }
    initEvent(type, bubbles = false, cancelable = false) {
      const state = eventStates.get(this); if (state.dispatching) return;
      Object.assign(state, { type: String(type), bubbles: !!bubbles, cancelable: !!cancelable, defaultPrevented: false, target: null, stopped: false, immediate: false });
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
  class ErrorEvent extends Event {
    constructor(type, options = {}) { super(type, options); this.message = String(options.message ?? ''); this.filename = String(options.filename ?? ''); this.lineno = options.lineno >>> 0; this.colno = options.colno >>> 0; this.error = options.error ?? null; }
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
      const state = eventStates.get(event);
      if (state.dispatching) throw new DOMException('Event is already in dispatch', 'InvalidStateError');
      Object.assign(state, { target: this, currentTarget: this, eventPhase: 2, dispatching: true, trusted: false, path: null, hidden: null });
      // Removal replaces the list and adding appends to it, so the first `length` items are the snapshot.
      const target = eventTargets.get(this), list = target.get(event.type) ?? [];
      try {
        for (let i = 0, length = list.length; i < length && !state.immediate; i++) {
          const item = list[i], current = target.get(event.type) ?? [];
          if (current !== list && !current.includes(item)) continue;
          if (item.once) this.removeEventListener(event.type, item.callback, item.capture);
          state.passive = item.passive;
          try { if (typeof item.callback === 'function') item.callback.call(this, event); else item.callback.handleEvent(event); }
          catch (error) { (globalThis.reportError ?? console.error)(error); }
          state.passive = false;
        }
        // Run only own on* properties. Accessor handlers already have listeners, so a second call duplicates
        // their events.
        try { const handler = Object.hasOwn(this, 'on' + event.type) ? this['on' + event.type] : null; if (!state.immediate && typeof handler === 'function') handler.call(this, event); }
        catch (error) { (globalThis.reportError ?? console.error)(error); }
      } finally { Object.assign(state, { currentTarget: null, eventPhase: 0, dispatching: false, stopped: false, immediate: false, passive: false }); }
      return !event.defaultPrevented;
    }
  }
  eventTargets.set(globalThis, new Map());
  for (const name of ['addEventListener', 'removeEventListener', 'dispatchEvent']) globalThis[name] = EventTarget.prototype[name].bind(globalThis);
  const signals = new WeakMap();
  const abort = (signal, reason) => {
    const state = signals.get(signal);
    if (state.aborted) return;
    state.aborted = true; state.reason = reason === undefined ? new DOMException('signal is aborted without reason', 'AbortError') : reason;
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
      setTimeout(() => abort(signal, new DOMException('The operation timed out', 'TimeoutError')), delay).unref?.();
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
    encode(input = '') { return codec.encode(String(input), 0); }
    encodeInto(input, output) {
      if (!(output instanceof Uint8Array)) throw new TypeError('Expected Uint8Array');
      const [read, written] = codec.encodeInto(String(input), output);
      return { read, written };
    }
  }
  const decoderLabels = { 'utf-8': 'utf-8', utf8: 'utf-8', 'unicode-1-1-utf-8': 'utf-8', 'utf-16le': 'utf-16le', 'utf-16': 'utf-16le', 'ucs-2': 'utf-16le', 'unicode': 'utf-16le', 'utf-16be': 'utf-16be', 'unicodefffe': 'utf-16be', latin1: 'windows-1252', 'iso-8859-1': 'windows-1252', 'iso8859-1': 'windows-1252', ascii: 'windows-1252', 'us-ascii': 'windows-1252', 'windows-1252': 'windows-1252', cp1252: 'windows-1252', 'x-cp1252': 'windows-1252' };
  const decoderKinds = { 'utf-8': 0, 'utf-16le': 4, 'utf-16be': 5, 'windows-1252': 6 };
  class TextDecoder {
    #pending = null; #started = false; #fatal; #ignoreBOM; #encoding;
    constructor(label = 'utf-8', options = {}) {
      this.#encoding = Object.hasOwn(decoderLabels, String(label).trim().toLowerCase()) ? decoderLabels[String(label).trim().toLowerCase()] : null;
      if (!this.#encoding) throw new RangeError(`The encoding label provided ('${label}') is invalid.`);
      this.#fatal = !!options.fatal; this.#ignoreBOM = !!options.ignoreBOM;
    }
    get encoding() { return this.#encoding; }
    get fatal() { return this.#fatal; }
    get ignoreBOM() { return this.#ignoreBOM; }
    decode(input = new Uint8Array(), options = {}) {
      let data = input;
      if (this.#pending) { const head = bytes(input), combined = new Uint8Array(this.#pending.length + head.length); combined.set(this.#pending); combined.set(head, this.#pending.length); data = combined; }
      else if (!ArrayBuffer.isView(data) && !(data instanceof ArrayBuffer) && !(typeof SharedArrayBuffer !== 'undefined' && data instanceof SharedArrayBuffer)) throw new TypeError('Expected an ArrayBuffer or a view');
      const stream = !!options.stream;
      let text;
      try { text = codec.decode(data, decoderKinds[this.#encoding], this.#fatal, stream); }
      catch (error) { this.#pending = null; this.#started = false; throw new TypeError(error.message); }
      this.#pending = null;
      if (stream) { const kept = text[1]; text = text[0]; if (kept) this.#pending = bytes(data).slice(-kept); }
      if (text && !this.#started) { this.#started = true; if (!this.#ignoreBOM && text.charCodeAt(0) === 0xFEFF) text = text.slice(1); }
      if (!stream) this.#started = false;
      return text;
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
  const query = text => {
    const list = [];
    try {
      for (const part of text.split('&')) {
        if (!part) continue;
        const at = part.indexOf('='), name = at < 0 ? part : part.slice(0, at), value = at < 0 ? '' : part.slice(at + 1);
        list.push([decodeURIComponent(name.replaceAll('+', ' ')), decodeURIComponent(value.replaceAll('+', ' '))]);
      }
    } catch { return call('web.params.parse', text); }
    return list;
  };
  const form = text => encodeURIComponent(text).replace(/[!'()~]|%20/g, c => c === '%20' ? '+' : '%' + c.charCodeAt(0).toString(16).toUpperCase());
  class URLSearchParams {
    constructor(input = '') {
      let list;
      if (typeof input === 'object' && input !== null) {
        if (input[Symbol.iterator]) list = Array.from(input, pair => { const values = Array.from(pair); if (values.length !== 2) throw new TypeError('Expected a pair'); return values.map(usv); });
        else list = Object.entries(input).map(pair => pair.map(usv));
      } else list = query(usv(input).replace(/^\?/, ''));
      params.set(this, { list });
    }
    get size() { return params.get(this).list.length; }
    append(name, value) { params.get(this).list.push([usv(name), usv(value)]); updateParams(this); }
    delete(name, value) { name = usv(name); if (value !== undefined) value = usv(value); const state = params.get(this); state.list = state.list.filter(pair => pair[0] !== name || (value !== undefined && pair[1] !== value)); updateParams(this); }
    get(name) { name = usv(name); return params.get(this).list.find(pair => pair[0] === name)?.[1] ?? null; }
    getAll(name) { name = usv(name); return params.get(this).list.filter(pair => pair[0] === name).map(pair => pair[1]); }
    has(name, value) { name = usv(name); if (value !== undefined) value = usv(value); return params.get(this).list.some(pair => pair[0] === name && (value === undefined || pair[1] === value)); }
    set(name, value) {
      name = usv(name); value = usv(value); const state = params.get(this), index = state.list.findIndex(pair => pair[0] === name);
      if (index < 0) state.list.push([name, value]);
      else { state.list[index][1] = value; state.list = state.list.filter((pair, i) => pair[0] !== name || i === index); }
      updateParams(this);
    }
    sort() { params.get(this).list.sort((a, b) => a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0); updateParams(this); }
    *entries() { for (let i = 0; i < params.get(this).list.length; i++) yield params.get(this).list[i].slice(); }
    *keys() { for (const [key] of this) yield key; }
    *values() { for (const [, value] of this) yield value; }
    forEach(fn, thisArg) { for (const [key, value] of this) fn.call(thisArg, value, key, this); }
    [Symbol.iterator]() { return this.entries(); }
    toString() { return params.get(this).list.map(([name, value]) => `${form(name)}=${form(value)}`).join('&'); }
  }
  const parseURL = (...args) => { try { return codec.url(...args.map(value => typeof value === 'string' ? usv(value) : value)); } catch (error) { throw new TypeError(error.message); } };
  const parts = {
    href: at => at[0],
    protocol: at => at[0].slice(0, at[1] + 1),
    username: at => at[0].slice(at[2], at[3]),
    password: at => at[0].slice(at[4], at[5]),
    host: at => at[0].slice(at[6], at[8]),
    hostname: at => at[0].slice(at[6], at[7]),
    port: at => at[0].slice(at[7] + 1, at[8]),
    pathname: at => at[0].slice(at[8], at[9]),
    search: at => at[10] - at[9] > 1 ? at[0].slice(at[9], at[10]) : '',
    hash: at => at[0].length - at[10] > 1 ? at[0].slice(at[10]) : '',
  };
  const special = new Set(['http:', 'https:', 'ws:', 'wss:', 'ftp:']);
  class URL {
    constructor(input, base) { urls.set(this, { data: parseURL(String(input), base === undefined ? null : String(base)), searchParams: null }); }
    get origin() {
      const at = urls.get(this).data, protocol = parts.protocol(at);
      if (special.has(protocol)) return `${protocol}//${parts.host(at)}`;
      if (protocol !== 'blob:') return 'null';
      try { const inner = new URL(parts.pathname(at)); return inner.protocol === 'http:' || inner.protocol === 'https:' ? inner.origin : 'null'; } catch { return 'null'; }
    }
    get searchParams() {
      const state = urls.get(this);
      if (!state.searchParams) {
        state.searchParams = new URLSearchParams(parts.search(state.data));
        params.get(state.searchParams).update = () => { state.data = parseURL(state.data[0], null, 'search', state.searchParams.toString()); };
      }
      return state.searchParams;
    }
    toString() { return this.href; }
    toJSON() { return this.href; }
    static canParse(input, base) { try { new URL(input, base); return true; } catch { return false; } }
    static parse(input, base) { try { return new URL(input, base); } catch { return null; } }
  }
  for (const key of ['href', 'protocol', 'username', 'password', 'host', 'hostname', 'port', 'pathname', 'search', 'hash']) Object.defineProperty(URL.prototype, key, {
    enumerable: true, configurable: true,
    get() { return parts[key](urls.get(this).data); },
    set(value) { const state = urls.get(this); state.data = parseURL(state.data[0], null, key, String(value)); if ((key === 'search' || key === 'href') && state.searchParams) params.get(state.searchParams).list = query(parts.search(state.data).slice(1)); },
  });
  const uuid = new Uint8Array(16);
  const crypto = globalThis.__appsCrypto(call, decodeBytes, {
    getRandomValues(array) {
      if (!ArrayBuffer.isView(array) || array instanceof DataView || array instanceof Float32Array || array instanceof Float64Array || (typeof Float16Array !== 'undefined' && array instanceof Float16Array)) throw new DOMException('Expected an integer typed array', 'TypeMismatchError');
      if (array.byteLength > 65536) throw new DOMException('Random byte quota exceeded', 'QuotaExceededError');
      codec.random(array); return array;
    },
    randomUUID() {
      codec.random(uuid); uuid[6] = (uuid[6] & 15) | 64; uuid[8] = (uuid[8] & 63) | 128;
      const hex = uuid.toHex();
      return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
    },
    subtle: Object.freeze({
      async digest(algorithm, input) {
        const name = String(typeof algorithm === 'string' ? algorithm : algorithm?.name).toUpperCase();
        const data = bytes(input);
        if (!['SHA-1', 'SHA-256', 'SHA-384', 'SHA-512'].includes(name)) throw new DOMException('Unsupported digest algorithm', 'NotSupportedError');
        return decodeBytes(call('web.digest', name, data)).buffer;
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
  for (const Type of [Blob, File]) Object.defineProperty(Type.prototype, Symbol.toStringTag, { configurable: true, value: Type.name });
  const forms = new WeakMap();
  const formValue = (value, filename) => {
    if (filename !== undefined && !(value instanceof Blob)) throw new TypeError('Filename requires a Blob');
    if (!(value instanceof Blob)) return usv(value);
    return value instanceof File && filename === undefined ? value : new File([value], filename ?? 'blob', { type: value.type });
  };
  class FormData {
    constructor(form, submitter = null) { forms.set(this, []); if (form !== undefined) { if (typeof HTMLFormElement !== 'function' || !(form instanceof HTMLFormElement)) throw new TypeError('Expected a form element'); form._entries(submitter, this); } }
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
      this.bufferedAmount += call('web.socket.send', this._socket, data instanceof Blob ? blobs.get(data) : binary ? bytes(data) : String(data), binary);
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
      const url = new URL(request.url), protocol = url.protocol;
      if (protocol === 'apps:') {
        if (!['GET', 'HEAD'].includes(request.method)) throw new TypeError('Bundled assets require GET');
        const data = call('web.asset', request.url);
        if (data === null) throw new TypeError('Failed to fetch');
        const type = { wasm: 'application/wasm', svg: 'image/svg+xml', png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', webp: 'image/webp', woff2: 'font/woff2', woff: 'font/woff', ttf: 'font/ttf', otf: 'font/otf', mp3: 'audio/mpeg', wav: 'audio/wav', mp4: 'video/mp4', glb: 'model/gltf-binary' }[request.url.split('.').pop()] ?? 'application/octet-stream';
        resolve(new Response(request.method === 'HEAD' ? null : decodeBytes(data), { headers: { 'content-type': type } })); return;
      }
      if (protocol === 'data:') {
        if (request.method !== 'GET') throw new TypeError('Data URLs require GET');
        const source=url.href.slice(5,url.hash?-url.hash.length:undefined), comma=source.indexOf(',');
        if(comma<0)throw new TypeError('Invalid data URL');
        let type=source.slice(0,comma), bytes=[];
        const raw=source.slice(comma+1);
        const encoder=new TextEncoder();
        for(let i=0;i<raw.length;){const code=raw.charCodeAt(i);if(code===37&&/^[0-9a-f]{2}$/i.test(raw.slice(i+1,i+3))){bytes.push(parseInt(raw.slice(i+1,i+3),16));i+=3;}else if(code<128){bytes.push(code);i++;}else{let end=i+1;while(end<raw.length&&raw.charCodeAt(end)>=128)end++;for(const byte of encoder.encode(raw.slice(i,end)))bytes.push(byte);i=end;}}
        if(/;base64$/i.test(type)){type=type.slice(0,-7);bytes=Uint8Array.fromBase64(codec.decode(new Uint8Array(bytes),1));}
        if(!type)type='text/plain;charset=US-ASCII';else if(type.startsWith(';'))type='text/plain'+type;
        resolve(new Response(new Uint8Array(bytes),{headers:{'content-type':type}}));return;
      }
      if (protocol === 'blob:') {
        const blob = objectURLs.get(request.url); if (!blob || request.method !== 'GET') throw new TypeError('Invalid Blob URL request');
        resolve(new Response(blob, { headers: { 'content-type': blob.type, 'content-length': String(blob.size) } })); return;
      }
      if (!['http:', 'https:'].includes(protocol)) throw new TypeError('Fetch supports HTTP, HTTPS, Blob, and data URLs');
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
        id = call('web.fetch.start', { url: request.url, method: request.method, headers: [...request.headers], redirect: request.redirect, body: data, stream: !!state.uploadReader });
        pending.set(id, state);
      } catch (error) { if (state.uploadReader) { const reader = state.uploadReader; reader.cancel(error).catch(() => {}).finally(() => reader.releaseLock()); } signal.removeEventListener('abort', state.onAbort); reject(signal.aborted ? signal.reason : new TypeError(error.message)); queued.shift()?.(); }
    };
    if (request.body && !memoryBodies.has(request.body)) { state.uploadReader = request.body.getReader(); start(new Uint8Array()); }
    else if (request.body) consume(request, signal).then(start, error => { signal.removeEventListener('abort', state.onAbort); reject(error); });
    else start(new Uint8Array());
  });
  let beaconBytes = 0;
  globalThis.navigator ??= {};
  for (const [name, get] of Object.entries({ language: () => new Intl.DateTimeFormat().resolvedOptions().locale, languages: () => [navigator.language], onLine: () => true, cookieEnabled: () => true, maxTouchPoints: () => 0, vendor: () => '', appName: () => 'Netscape', appCodeName: () => 'Mozilla', appVersion: () => navigator.userAgent, product: () => 'Gecko', webdriver: () => false, pdfViewerEnabled: () => false, hardwareConcurrency: () => call('web.cpus') })) if (!(name in navigator)) Object.defineProperty(navigator, name, { get, configurable: true, enumerable: true });
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
    for (const retry of [...retries]) { retries.delete(retry); retry(); }
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
          call('web.fetch.upload', event.id, done ? null : value);
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
  poll.pending = () => sockets.size > 0 || retries.size > 0 || waiting();
  poll.eventState = event => eventStates.get(event);
  poll.blob = blob => blobs.get(blob);
  poll.listeners = (target, type) => (eventTargets.get(target)?.get(String(type)) ?? []).map(item => item.callback);
  class ProgressEvent extends Event {
    constructor(type, init = {}) { super(type, init); this.lengthComputable = !!init.lengthComputable; this.loaded = Number(init.loaded) || 0; this.total = Number(init.total) || 0; }
  }
  const fireXhr = (target, type, init = {}) => { const event = type === 'readystatechange' ? new Event(type) : new ProgressEvent(type, init); target.dispatchEvent(event); target['on' + type]?.call(target, event); };
  class XMLHttpRequestEventTarget extends EventTarget {
    onloadstart = null; onprogress = null; onabort = null; onerror = null; onload = null; ontimeout = null; onloadend = null;
  }
  class XMLHttpRequestUpload extends XMLHttpRequestEventTarget {}
  class XMLHttpRequest extends XMLHttpRequestEventTarget {
    static UNSENT = 0; static OPENED = 1; static HEADERS_RECEIVED = 2; static LOADING = 3; static DONE = 4;
    UNSENT = 0; OPENED = 1; HEADERS_RECEIVED = 2; LOADING = 3; DONE = 4;
    onreadystatechange = null; upload = new XMLHttpRequestUpload(); readyState = 0; status = 0; statusText = ''; responseURL = ''; responseType = ''; timeout = 0; withCredentials = false;
    #method; #url; #headers; #sent = false; #run = 0; #controller = null; #timer; #response = null; #chunks = []; #size = 0; #mime = null; #uploading = false;
    #state(value) { this.readyState = value; fireXhr(this, 'readystatechange'); }
    #reset() { this.#run++; clearTimeout(this.#timer); this.#controller = null; this.#sent = false; this.#uploading = false; this.#response = null; this.#chunks = []; this.#size = 0; this.status = 0; this.statusText = ''; this.responseURL = ''; }
    #fail(type) {
      this.#run++; clearTimeout(this.#timer); this.#sent = false; this.#response = null; this.#chunks = []; this.status = 0; this.statusText = ''; this.responseURL = '';
      this.#state(4);
      if (this.#uploading) { this.#uploading = false; fireXhr(this.upload, type); fireXhr(this.upload, 'loadend'); }
      fireXhr(this, type); fireXhr(this, 'loadend');
    }
    open(method, url, async = true) {
      if (async === false) throw new DOMException('Synchronous requests are not supported', 'InvalidAccessError');
      this.#controller?.abort(); this.#reset();
      this.#method = String(method).toUpperCase(); this.#url = String(url); this.#headers = new Headers();
      this.#state(1);
    }
    setRequestHeader(name, value) { if (this.readyState !== 1 || this.#sent) throw new DOMException('The request is not open', 'InvalidStateError'); this.#headers.append(name, value); }
    overrideMimeType(mime) { this.#mime = String(mime); }
    getResponseHeader(name) { return this.#response?.headers.get(name) ?? null; }
    getAllResponseHeaders() { return this.#response ? [...this.#response.headers].map(([name, value]) => `${name}: ${value}\r\n`).join('') : ''; }
    send(body = null) {
      if (this.readyState !== 1 || this.#sent) throw new DOMException('The request is not open', 'InvalidStateError');
      const bodied = body !== null && !['GET', 'HEAD'].includes(this.#method);
      const total = !bodied ? 0 : body.size ?? body.byteLength ?? (typeof body === 'string' ? new TextEncoder().encode(body).length : 0);
      const run = ++this.#run, controller = this.#controller = new AbortController();
      this.#sent = true; this.#uploading = bodied;
      fireXhr(this, 'loadstart');
      if (bodied) fireXhr(this.upload, 'loadstart', { lengthComputable: true, loaded: 0, total });
      let timedOut = false;
      if (this.timeout > 0) this.#timer = setTimeout(() => { timedOut = true; controller.abort(); }, this.timeout);
      (async () => {
        const response = await fetch(this.#url, { method: this.#method, headers: this.#headers, body: bodied ? body : undefined, signal: controller.signal, credentials: this.withCredentials ? 'include' : 'same-origin' });
        if (run !== this.#run) return;
        if (bodied) { this.#uploading = false; for (const type of ['progress', 'load', 'loadend']) fireXhr(this.upload, type, { lengthComputable: true, loaded: total, total }); }
        this.#response = response; this.status = response.status; this.statusText = response.statusText; this.responseURL = response.url;
        this.#state(2);
        const length = Number(response.headers.get('content-length')) || 0, reader = response.body?.getReader();
        while (reader) {
          const { value, done } = await reader.read();
          if (run !== this.#run) { reader.cancel().catch(() => {}); return; }
          if (done) break;
          this.#chunks.push(value); this.#size += value.length;
          this.#state(3);
          fireXhr(this, 'progress', { lengthComputable: length > 0, loaded: this.#size, total: length });
        }
        if (run !== this.#run) return;
        clearTimeout(this.#timer); this.#sent = false;
        this.#state(4);
        fireXhr(this, 'load', { lengthComputable: length > 0, loaded: this.#size, total: length });
        fireXhr(this, 'loadend', { lengthComputable: length > 0, loaded: this.#size, total: length });
      })().catch(() => { if (run === this.#run) this.#fail(timedOut ? 'timeout' : 'error'); });
    }
    abort() {
      const controller = this.#controller, active = this.#sent;
      this.#controller = null;
      if (active) { this.#fail('abort'); controller?.abort(); }
      else this.#reset();
      this.readyState = 0;
    }
    get #bytes() { const bytes = new Uint8Array(this.#size); let at = 0; for (const chunk of this.#chunks) { bytes.set(chunk, at); at += chunk.length; } return bytes; }
    get responseText() { if (this.responseType !== '' && this.responseType !== 'text') throw new DOMException('The response is not text', 'InvalidStateError'); return this.readyState < 3 ? '' : new TextDecoder().decode(this.#bytes); }
    get responseXML() { return this.responseType === 'document' && this.readyState === 4 && globalThis.DOMParser && this.#size ? new DOMParser().parseFromString(new TextDecoder().decode(this.#bytes), 'text/html') : null; }
    get response() {
      if (this.responseType === '' || this.responseType === 'text') return this.responseText;
      if (this.readyState !== 4 || !this.#response) return null;
      if (this.responseType === 'json') { try { return JSON.parse(new TextDecoder().decode(this.#bytes)); } catch { return null; } }
      if (this.responseType === 'arraybuffer') return this.#bytes.buffer;
      if (this.responseType === 'blob') return new Blob([this.#bytes], { type: this.#mime ?? this.#response.headers.get('content-type') ?? '' });
      return this.responseXML;
    }
  }
  class FileReader extends EventTarget {
    static EMPTY = 0; static LOADING = 1; static DONE = 2;
    EMPTY = 0; LOADING = 1; DONE = 2;
    readyState = 0; result = null; error = null;
    onloadstart = null; onprogress = null; onload = null; onabort = null; onerror = null; onloadend = null;
    #run = 0;
    #fire(type, blob) { const event = new ProgressEvent(type, { lengthComputable: true, loaded: this.result === null ? 0 : blob.size, total: blob.size }); this.dispatchEvent(event); }
    #read(blob, convert) {
      if (!(blob instanceof Blob)) throw new TypeError('Expected a Blob');
      if (this.readyState === 1) throw new DOMException('The reader is already busy', 'InvalidStateError');
      this.readyState = 1; this.result = this.error = null;
      const run = ++this.#run;
      setTimeout(() => {
        if (run !== this.#run) return;
        this.#fire('loadstart', blob);
        blob.arrayBuffer().then(buffer => {
          if (run !== this.#run) return;
          this.result = convert(buffer); this.readyState = 2;
          this.#fire('progress', blob); this.#fire('load', blob); if (run === this.#run && this.readyState === 2) this.#fire('loadend', blob);
        }, error => {
          if (run !== this.#run) return;
          this.error = error; this.readyState = 2;
          this.#fire('error', blob); this.#fire('loadend', blob);
        });
      }, 0);
    }
    readAsArrayBuffer(blob) { this.#read(blob, buffer => buffer); }
    readAsBinaryString(blob) { this.#read(blob, buffer => codec.decode(buffer, 1)); }
    readAsText(blob, encoding = 'utf-8') { this.#read(blob, buffer => new TextDecoder(encoding).decode(buffer)); }
    readAsDataURL(blob) { this.#read(blob, buffer => `data:${blob.type || 'application/octet-stream'};base64,${new Uint8Array(buffer).toBase64()}`); }
    abort() { if (this.readyState !== 1) { return; } this.#run++; this.readyState = 2; this.result = null; const blob = new Blob(); this.#fire('abort', blob); this.#fire('loadend', blob); }
  }
  Object.assign(globalThis, { FileReader, ProgressEvent, XMLHttpRequest, XMLHttpRequestEventTarget, XMLHttpRequestUpload, DOMException, Event, CustomEvent, ErrorEvent, EventTarget, AbortSignal, AbortController, TextEncoder, TextDecoder, TextEncoderStream,
    TextDecoderStream, URL, URLSearchParams, crypto, Headers, Request, Response, Blob, File, FormData, fetch, WebSocket, MessageEvent, CloseEvent });
  return poll;
};
