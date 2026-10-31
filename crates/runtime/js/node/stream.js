globalThis.__appsNodeStreams = (EventEmitter, Buffer, StringDecoder) => {
  const error = (Type, code, message) => Object.assign(new Type(message), { code });
  const copy = (target, source, skip) => { for (const name of Reflect.ownKeys(source)) if (!skip.includes(name)) Object.defineProperty(target, name, { ...Object.getOwnPropertyDescriptor(source, name), enumerable: false }); };
  const define = (target, source) => { copy(target.prototype, source.prototype, ['constructor']); copy(target, source, ['length', 'name', 'prototype']); };
  const inherit = (child, parent) => { Object.setPrototypeOf(child.prototype, parent.prototype); Object.setPrototypeOf(child, parent); };
  const size = (state, chunk) => state.objectMode ? 1 : chunk.length, marks = [65536, 16];
  function Stream(options) { EventEmitter.call(this, options); }
  inherit(Stream, EventEmitter);
  Stream.prototype.pipe = function(destination, options) {
    const ondata = chunk => { if (destination.writable && destination.write(chunk) === false) this.pause?.(); };
    this.on('data', ondata);
    destination.on('drain', () => this.resume?.());
    if (options?.end !== false) this.on('end', () => destination.end());
    destination.emit('pipe', this);
    return destination;
  };
  const destroy = function(reason, callback) {
    if (this.destroyed) { if (callback) queueMicrotask(() => callback(this.errored)); return this; }
    this.destroyed = true;
    if (reason) this.errored = reason;
    const finishers = this._writableState?.finishers;
    if (finishers?.length) queueMicrotask(() => { for (const finisher of finishers.splice(0)) finisher(this.errored ?? error(Error, 'ERR_STREAM_DESTROYED', 'Cannot call end after a stream was destroyed')); });
    const emitClose = (this._readableState ?? this._writableState).emitClose;
    this._destroy(reason ?? null, failure => queueMicrotask(() => {
      if (failure) { this.errored = failure; this.emit('error', failure); }
      this.closed = true;
      callback?.(failure ?? null);
      if (emitClose) this.emit('close');
    }));
    return this;
  };
  const autoDestroy = stream => {
    const read = stream._readableState, write = stream._writableState;
    if ((read ? read.autoDestroy : write.autoDestroy) && (!read || read.endEmitted || read.readable === false) && (!write || write.finished || write.writable === false)) stream.destroy();
  };
  const initReadable = (stream, options = {}) => {
    const objectMode = !!(options.objectMode || options.readableObjectMode);
    stream._readableState = { objectMode, highWaterMark: options.readableHighWaterMark ?? options.highWaterMark ?? marks[+objectMode], buffer: [], length: 0, flowing: null, ended: false, endEmitted: false, reading: false, consumed: false, listening: false, pending: false, decoder: null, encoding: null, pipes: [], autoDestroy: options.autoDestroy !== false, emitClose: options.emitClose !== false };
    stream.destroyed = false; stream.closed = false; stream.errored = null;
    if (options.read) stream._read = options.read;
    if (options.destroy) stream._destroy = options.destroy;
    if (options.encoding) stream.setEncoding(options.encoding);
    if (options.signal) addAbortSignal(options.signal, stream);
  };
  function Readable(options) { if (!(this instanceof Readable)) return new Readable(options); Stream.call(this, options); initReadable(this, options); }
  inherit(Readable, Stream);
  define(Readable, class {
    get readable() { const s = this._readableState; return !!s && s.readable !== false && !this.destroyed && !this.errored && !s.endEmitted; }
    set readable(value) { if (this._readableState) this._readableState.readable = !!value; }
    get readableEnded() { return this._readableState.endEmitted; }
    get readableFlowing() { return this._readableState.flowing; }
    get readableLength() { return this._readableState.length; }
    get readableHighWaterMark() { return this._readableState.highWaterMark; }
    get readableObjectMode() { return this._readableState.objectMode; }
    get readableEncoding() { return this._readableState.encoding; }
    get readableAborted() { return this.destroyed && !this._readableState.endEmitted; }
    _read() { this.destroy(error(Error, 'ERR_METHOD_NOT_IMPLEMENTED', 'The _read() method is not implemented')); }
    _destroy(reason, callback) { callback(reason); }
    push(chunk, encoding) {
      const s = this._readableState;
      s.reading = false;
      if (chunk === null) {
        if (s.ended) return false;
        s.ended = true;
        if (s.decoder) { const tail = s.decoder.end(); if (tail) { s.buffer.push(tail); s.length += size(s, tail); } }
        this._signal();
        return false;
      }
      if (this.destroyed) return false;
      if (s.ended) { this.destroy(error(Error, 'ERR_STREAM_PUSH_AFTER_EOF', 'stream.push() after EOF')); return false; }
      if (!s.objectMode) {
        if (typeof chunk === 'string') chunk = Buffer.from(chunk, encoding);
        else if (ArrayBuffer.isView(chunk)) chunk = chunk instanceof Buffer ? chunk : Buffer.from(chunk.buffer, chunk.byteOffset, chunk.byteLength);
        else throw error(TypeError, 'ERR_INVALID_ARG_TYPE', 'The "chunk" argument must be of type string or an instance of Buffer or Uint8Array');
        if (s.decoder) chunk = s.decoder.write(chunk);
        if (!chunk.length) return s.length < s.highWaterMark;
      }
      s.buffer.push(chunk); s.length += size(s, chunk);
      this._signal();
      return s.length < s.highWaterMark;
    }
    unshift(chunk, encoding) {
      const s = this._readableState;
      if (chunk === null) return this.push(null);
      if (!s.objectMode && typeof chunk === 'string' && !s.decoder) chunk = Buffer.from(chunk, encoding);
      s.buffer.unshift(chunk); s.length += size(s, chunk);
      this._signal();
    }
    _signal() {
      const s = this._readableState;
      if (s.flowing) { this._flow(); return; }
      if (s.listening && !s.pending) { s.pending = true; queueMicrotask(() => { s.pending = false; if (!this.destroyed) this.emit('readable'); this._end(); }); }
    }
    _pull() {
      const s = this._readableState;
      if (s.reading || s.ended || this.destroyed || s.length >= s.highWaterMark) return;
      s.reading = true;
      this._read(s.highWaterMark);
    }
    _flow() {
      const s = this._readableState;
      if (s.draining) return;
      s.draining = true;
      try { while (s.flowing && s.buffer.length && !this.destroyed) { const chunk = s.buffer.shift(); s.length -= size(s, chunk); this.emit('data', chunk); } }
      finally { s.draining = false; }
      this._end();
      if (s.flowing && !s.ended) queueMicrotask(() => this._pull());
    }
    _end() {
      const s = this._readableState;
      if (!s.ended || s.length || s.endEmitted || s.ending || !(s.flowing || s.consumed)) return;
      s.ending = true;
      queueMicrotask(() => { if (s.endEmitted || s.length || this.destroyed) { s.ending = false; return; } s.endEmitted = true; this.emit('end'); autoDestroy(this); });
    }
    read(count) {
      const s = this._readableState;
      s.consumed = true;
      let chunk = null;
      if (s.buffer.length && count !== 0 && !(count > s.length && !s.ended)) {
        if (s.objectMode) chunk = s.buffer.shift();
        else {
          const all = s.buffer.length === 1 ? s.buffer[0] : s.decoder ? s.buffer.join('') : Buffer.concat(s.buffer);
          chunk = count === undefined || count >= all.length ? all : all.slice(0, count);
          s.buffer = chunk.length < all.length ? [all.slice(chunk.length)] : [];
        }
        s.length -= size(s, chunk);
      }
      if (chunk !== null) this.emit('data', chunk);
      if (s.ended) this._end(); else queueMicrotask(() => this._pull());
      return chunk;
    }
    on(name, listener) {
      Stream.prototype.on.call(this, name, listener);
      const s = this._readableState;
      if (name === 'data' && s.flowing !== false) this.resume();
      if (name === 'readable' && !s.listening) { s.listening = true; s.flowing = false; if (s.length || s.ended) this._signal(); else queueMicrotask(() => this._pull()); }
      return this;
    }
    addListener(name, listener) { return this.on(name, listener); }
    pause() { const s = this._readableState; if (s.flowing !== false) { s.flowing = false; this.emit('pause'); } return this; }
    resume() {
      const s = this._readableState;
      if (!s.flowing) { s.flowing = true; s.listening = false; queueMicrotask(() => { if (!s.flowing) return; this.emit('resume'); this._flow(); }); }
      return this;
    }
    isPaused() { return this._readableState.flowing === false; }
    setEncoding(encoding) {
      const s = this._readableState;
      s.decoder = new StringDecoder(encoding); s.encoding = s.decoder.encoding;
      if (!s.objectMode && s.buffer.length) { const text = s.decoder.write(Buffer.concat(s.buffer)); s.buffer = text ? [text] : []; s.length = text.length; }
      return this;
    }
    pipe(destination, { end = true } = {}) {
      const s = this._readableState;
      const ondata = chunk => { if (destination.write(chunk) === false) this.pause(); };
      const ondrain = () => { if (s.flowing === false) this.resume(); };
      const onend = () => destination.end();
      const onerror = failure => { cleanup(); if (!destination.listenerCount('error')) destination.emit('error', failure); };
      const cleanup = () => { this.off('data', ondata); this.off('end', onend); destination.off('drain', ondrain); destination.off('error', onerror); destination.off('close', cleanup); destination.off('finish', cleanup); s.pipes = s.pipes.filter(pipe => pipe.destination !== destination); };
      s.pipes.push({ destination, cleanup });
      this.on('data', ondata);
      if (end && destination !== globalThis.process?.stdout && destination !== globalThis.process?.stderr) this.once('end', onend);
      destination.on('drain', ondrain); destination.prependListener('error', onerror); destination.once('close', cleanup); destination.once('finish', cleanup);
      destination.emit('pipe', this);
      return destination;
    }
    unpipe(destination) {
      for (const pipe of [...this._readableState.pipes]) if (!destination || pipe.destination === destination) { pipe.cleanup(); pipe.destination.emit('unpipe', this); }
      if (!this._readableState.pipes.length && !this.listenerCount('data')) this.pause();
      return this;
    }
    wrap(stream) {
      let paused = false;
      stream.on('data', chunk => { if (!this.push(chunk) && typeof stream.pause === 'function') { paused = true; stream.pause(); } });
      stream.on('end', () => this.push(null));
      stream.on('error', error => this.destroy(error));
      stream.on('close', () => this.destroy());
      this._read = () => { if (paused && typeof stream.resume === 'function') { paused = false; stream.resume(); } };
      return this;
    }
    async *[Symbol.asyncIterator]() {
      const s = this._readableState;
      let failed = null, wake = null;
      const signal = () => { const resolve = wake; wake = null; resolve?.(); };
      const onerror = reason => { failed = reason; signal(); };
      this.on('readable', signal); this.on('end', signal); this.on('close', signal); this.on('error', onerror);
      try {
        for (;;) {
          if (failed) throw failed;
          const chunk = this.destroyed ? null : this.read();
          if (chunk !== null) { yield chunk; continue; }
          if (s.endEmitted || (s.ended && !s.length)) return;
          if (this.destroyed) { if (this.errored) throw this.errored; throw error(Error, 'ERR_STREAM_PREMATURE_CLOSE', 'Premature close'); }
          await new Promise(resolve => { wake = resolve; });
        }
      } finally {
        this.off('readable', signal); this.off('end', signal); this.off('close', signal); this.off('error', onerror);
        if (!(s.ended && !s.length) && !this.destroyed) this.destroy();
      }
    }
    static from(iterable, options) {
      if (typeof iterable === 'string' || iterable instanceof Uint8Array) return new Readable({ objectMode: true, ...options, read() { this.push(iterable); this.push(null); } });
      const iterator = iterable?.[Symbol.asyncIterator]?.() ?? iterable?.[Symbol.iterator]?.();
      if (!iterator) throw error(TypeError, 'ERR_INVALID_ARG_TYPE', 'The "iterable" argument must be an instance of Iterable');
      let reading = false;
      return new Readable({ objectMode: true, highWaterMark: 1, ...options,
        read() {
          if (reading) return;
          reading = true;
          (async () => {
            try {
              for (;;) {
                const next = await iterator.next();
                if (next.done) { this.push(null); break; }
                const value = await next.value;
                if (value === null) throw error(TypeError, 'ERR_STREAM_NULL_VALUES', 'May not write null values to stream');
                if (!this.push(value)) break;
              }
            } catch (reason) { this.destroy(reason); } finally { reading = false; }
          })();
        },
        destroy(reason, callback) { Promise.resolve(this._readableState.endEmitted ? undefined : iterator.return?.()).then(() => callback(reason), callback); },
      });
    }
    static fromWeb(stream, options) { return Readable.from(stream, { objectMode: false, ...options }); }
    static toWeb(stream) { const iterator = stream[Symbol.asyncIterator](); return new ReadableStream({ async pull(controller) { const next = await iterator.next(); if (next.done) controller.close(); else controller.enqueue(next.value); }, cancel: reason => { stream.destroy(reason instanceof Error ? reason : undefined); return iterator.return?.(); } }); }
  });
  Readable.prototype.destroy = destroy;

  const initWritable = (stream, options = {}) => {
    const objectMode = !!(options.objectMode || options.writableObjectMode);
    stream._writableState = { objectMode, highWaterMark: options.writableHighWaterMark ?? options.highWaterMark ?? marks[+objectMode], buffer: [], length: 0, writing: false, corked: 0, needDrain: false, finishers: [], ending: false, finalCalled: false, finished: false, decodeStrings: options.decodeStrings !== false, defaultEncoding: options.defaultEncoding ?? 'utf8', autoDestroy: options.autoDestroy !== false, emitClose: options.emitClose !== false };
    stream.destroyed = false; stream.closed = false; stream.errored = null;
    if (options.write) stream._write = options.write;
    if (options.writev) stream._writev = options.writev;
    if (options.final) stream._final = options.final;
    if (options.destroy) stream._destroy = options.destroy;
    if (options.signal) addAbortSignal(options.signal, stream);
  };
  function Writable(options) { if (!(this instanceof Writable)) return new Writable(options); Stream.call(this, options); initWritable(this, options); }
  inherit(Writable, Stream);
  class WritableMethods {
    get writable() { const s = this._writableState; return !!s && s.writable !== false && !this.destroyed && !this.errored && !s.ending; }
    set writable(value) { if (this._writableState) this._writableState.writable = !!value; }
    get writableEnded() { return this._writableState.ending; }
    get writableFinished() { return this._writableState.finished; }
    get writableLength() { return this._writableState.length; }
    get writableHighWaterMark() { return this._writableState.highWaterMark; }
    get writableObjectMode() { return this._writableState.objectMode; }
    get writableNeedDrain() { return this._writableState.needDrain; }
    get writableCorked() { return this._writableState.corked; }
    _write(chunk, encoding, callback) { if (this._writev) this._writev([{ chunk, encoding }], callback); else callback(error(Error, 'ERR_METHOD_NOT_IMPLEMENTED', 'The _write() method is not implemented')); }
    write(chunk, encoding, callback) {
      const s = this._writableState;
      if (typeof encoding === 'function') { callback = encoding; encoding = null; }
      encoding ||= s.defaultEncoding;
      if (chunk === null) throw error(TypeError, 'ERR_STREAM_NULL_VALUES', 'May not write null values to stream');
      if (!s.objectMode) {
        if (typeof chunk === 'string') { if (s.decodeStrings) { chunk = Buffer.from(chunk, encoding); encoding = 'buffer'; } }
        else if (ArrayBuffer.isView(chunk)) { chunk = chunk instanceof Buffer ? chunk : Buffer.from(chunk.buffer, chunk.byteOffset, chunk.byteLength); encoding = 'buffer'; }
        else throw error(TypeError, 'ERR_INVALID_ARG_TYPE', 'The "chunk" argument must be of type string or an instance of Buffer or Uint8Array');
      }
      const failure = s.ending ? error(Error, 'ERR_STREAM_WRITE_AFTER_END', 'write after end') : this.destroyed ? error(Error, 'ERR_STREAM_DESTROYED', 'Cannot call write after a stream was destroyed') : null;
      if (failure) { queueMicrotask(() => callback?.(failure)); if (s.ending) this.destroy(failure); return false; }
      s.length += size(s, chunk);
      const ok = s.length < s.highWaterMark;
      if (!ok) s.needDrain = true;
      s.buffer.push({ chunk, encoding, callback });
      if (!s.writing && !s.corked) this._next();
      return ok;
    }
    _next() {
      const s = this._writableState;
      if (s.writing || s.corked || this.destroyed) return;
      const entries = this._writev && s.buffer.length > 1 ? s.buffer.splice(0) : [s.buffer.shift()];
      if (!entries[0]) { this._finish(); return; }
      s.writing = true;
      let sync = true, called = false;
      const done = failure => {
        if (called) return;
        called = true;
        const after = () => {
          s.writing = false;
          for (const entry of entries) { s.length -= size(s, entry.chunk); entry.callback?.(failure ?? null); }
          if (failure) { this.destroy(failure); return; }
          if (s.buffer.length) { this._next(); return; }
          if (s.needDrain) { s.needDrain = false; if (!s.ending) this.emit('drain'); }
          this._finish();
        };
        if (sync) queueMicrotask(after); else after();
      };
      if (entries.length > 1) this._writev(entries.map(({ chunk, encoding }) => ({ chunk, encoding })), done); else this._write(entries[0].chunk, entries[0].encoding, done);
      sync = false;
    }
    _finish() {
      const s = this._writableState;
      if (!s.ending || s.finalCalled || s.writing || s.buffer.length || this.destroyed) return;
      s.finalCalled = true;
      queueMicrotask(() => {
        const after = failure => {
          if (failure) { this.destroy(failure); return; }
          s.finished = true;
          for (const finisher of s.finishers.splice(0)) finisher(null);
          this.emit('finish');
          autoDestroy(this);
        };
        if (this._final) this._final(after); else after();
      });
    }
    end(chunk, encoding, callback) {
      const s = this._writableState;
      if (typeof chunk === 'function') { callback = chunk; chunk = null; } else if (typeof encoding === 'function') { callback = encoding; encoding = null; }
      if (chunk !== null && chunk !== undefined) this.write(chunk, encoding);
      if (s.corked) { s.corked = 1; this.uncork(); }
      const failure = s.finished ? error(Error, 'ERR_STREAM_ALREADY_FINISHED', 'Cannot call end after a stream was finished') : this.destroyed ? error(Error, 'ERR_STREAM_DESTROYED', 'Cannot call end after a stream was destroyed') : null;
      if (callback) { if (failure || s.finished) queueMicrotask(() => callback(failure)); else s.finishers.push(callback); }
      if (!s.ending) { s.ending = true; this._finish(); }
      return this;
    }
    cork() { this._writableState.corked++; }
    uncork() { const s = this._writableState; if (s.corked && !--s.corked) this._next(); }
    setDefaultEncoding(encoding) { this._writableState.defaultEncoding = encoding; return this; }
    _destroy(reason, callback) { callback(reason); }
    static fromWeb(stream) { const writer = stream.getWriter(); return new Writable({ write(chunk, _encoding, callback) { writer.write(chunk).then(() => callback(), callback); }, final(callback) { writer.close().then(() => callback(), callback); }, destroy(reason, callback) { (reason ? writer.abort(reason) : Promise.resolve()).then(() => callback(reason), callback); } }); }
    static toWeb(stream) { return new WritableStream({ write: chunk => new Promise((resolve, reject) => stream.write(chunk, failure => failure ? reject(failure) : resolve())), close: () => new Promise((resolve, reject) => stream.end(failure => failure ? reject(failure) : resolve())), abort: reason => { stream.destroy(reason); } }); }
  }
  define(Writable, WritableMethods);
  Writable.prototype.destroy = destroy;
  Object.defineProperty(Writable, Symbol.hasInstance, { value(object) { return Function.prototype[Symbol.hasInstance].call(this, object) || (this === Writable && object?._writableState !== undefined); } });

  function Duplex(options = {}) {
    if (!(this instanceof Duplex)) return new Duplex(options);
    Readable.call(this, options);
    initWritable(this, options);
    if (options.allowHalfOpen === false) this.once('end', () => this.end());
  }
  inherit(Duplex, Readable);
  copy(Duplex.prototype, WritableMethods.prototype, ['constructor', '_destroy']);

  function Transform(options = {}) {
    if (!(this instanceof Transform)) return new Transform(options);
    Duplex.call(this, options);
    if (options.transform) this._transform = options.transform;
    if (options.flush) this._flush = options.flush;
  }
  inherit(Transform, Duplex);
  define(Transform, class {
    _transform(_chunk, _encoding, callback) { callback(error(Error, 'ERR_METHOD_NOT_IMPLEMENTED', 'The _transform() method is not implemented')); }
    _write(chunk, encoding, callback) {
      this._transform(chunk, encoding, (failure, data) => {
        if (failure) { callback(failure); return; }
        if (data != null) this.push(data);
        const s = this._readableState;
        if (s.ended || s.length < s.highWaterMark) callback(); else this._pending = callback;
      });
    }
    _read() { const callback = this._pending; this._pending = null; callback?.(); }
    _final(callback) {
      const done = (failure, data) => { if (failure) { callback(failure); return; } if (data != null) this.push(data); this.push(null); callback(); };
      if (this._flush) this._flush(done); else done();
    }
  });
  function PassThrough(options) { if (!(this instanceof PassThrough)) return new PassThrough(options); Transform.call(this, options); }
  inherit(PassThrough, Transform);
  PassThrough.prototype._transform = function(chunk, _encoding, callback) { callback(null, chunk); };

  const finished = (stream, options, callback) => {
    if (typeof options === 'function') { callback = options; options = {}; }
    const readable = options.readable ?? (stream._readableState ? stream._readableState.readable !== false : !stream._writableState && 'readable' in stream);
    const writable = options.writable ?? (stream._writableState ? stream._writableState.writable !== false : !stream._readableState && typeof stream.write === 'function');
    let readDone = !readable || !!stream._readableState?.endEmitted, writeDone = !writable || !!stream._writableState?.finished, settled = false;
    const done = failure => { if (settled) return; settled = true; cleanup(); callback.call(stream, failure); };
    const onend = () => { readDone = true; if (writeDone) done(); };
    const onfinish = () => { writeDone = true; if (readDone) done(); };
    const onerror = failure => done(failure);
    const onclose = () => { if (!readDone || !writeDone) done(stream.errored ?? error(Error, 'ERR_STREAM_PREMATURE_CLOSE', 'Premature close')); };
    const cleanup = () => { stream.off('end', onend); stream.off('finish', onfinish); stream.off('error', onerror); stream.off('close', onclose); };
    stream.on('end', onend); stream.on('finish', onfinish); stream.on('error', onerror); stream.on('close', onclose);
    if (readDone && writeDone) queueMicrotask(() => done());
    else if (stream.destroyed || stream.closed) queueMicrotask(onclose);
    return cleanup;
  };
  const abortError = signal => Object.assign(new Error('The operation was aborted', { cause: signal.reason }), { name: 'AbortError', code: 'ABORT_ERR' });
  const addAbortSignal = (signal, stream) => {
    const abort = () => stream.destroy(abortError(signal));
    if (signal.aborted) queueMicrotask(abort);
    else { signal.addEventListener('abort', abort, { once: true }); finished(stream, () => signal.removeEventListener('abort', abort)); }
    return stream;
  };
  const connect = (stages, { signal, end = true } = {}, callback) => {
    const streams = [];
    let settled = false, result, previous = null, consumer = null;
    const finish = failure => { if (settled) return; settled = true; if (failure) for (const stream of streams) if (!stream.destroyed) stream.destroy(); callback(failure, result); };
    stages.forEach((stage, index) => {
      const last = index === stages.length - 1, generated = typeof stage === 'function';
      if (generated) stage = index ? stage(previous, { signal }) : stage({ signal });
      if (last && typeof stage?.then === 'function') { consumer = stage; return; }
      if (typeof stage?.on !== 'function') stage = Readable.from(stage);
      if (previous && !generated) previous.pipe(stage, { end: last ? end : true });
      streams.push(stage);
      previous = stage;
    });
    streams.forEach((stream, index) => {
      const writable = typeof stream.write === 'function', readable = typeof stream.read === 'function';
      finished(stream, { readable: readable && (index < streams.length - 1 || !!consumer || !writable), writable: writable && index > 0 }, failure => { if (failure) finish(failure); else if (index === streams.length - 1 && !consumer) finish(); });
    });
    consumer?.then(value => { result = value; finish(); }, finish);
    if (signal) { if (signal.aborted) queueMicrotask(() => finish(abortError(signal))); else signal.addEventListener('abort', () => finish(abortError(signal)), { once: true }); }
    return previous;
  };
  const pipeline = (...streams) => {
    const callback = streams.pop();
    if (typeof callback !== 'function') throw error(TypeError, 'ERR_INVALID_ARG_TYPE', 'The "callback" argument must be of type function');
    return connect(streams.length === 1 && Array.isArray(streams[0]) ? streams[0] : streams, {}, failure => callback(failure));
  };
  const promises = {
    finished: (stream, options) => new Promise((resolve, reject) => finished(stream, options ?? {}, failure => failure ? reject(failure) : resolve())),
    pipeline: (...stages) => {
      const options = stages.length > 1 && stages.at(-1) && typeof stages.at(-1) === 'object' && !stages.at(-1).on && !stages.at(-1)[Symbol.asyncIterator] && !stages.at(-1)[Symbol.iterator] ? stages.pop() : {};
      return new Promise((resolve, reject) => connect(stages.length === 1 && Array.isArray(stages[0]) ? stages[0] : stages, options, (failure, value) => failure ? reject(failure) : resolve(value)));
    },
  };
  const getDefaultHighWaterMark = objectMode => marks[+!!objectMode];
  const setDefaultHighWaterMark = (objectMode, value) => { if (typeof value !== 'number') throw error(TypeError, 'ERR_INVALID_ARG_TYPE', `The "value" argument must be of type number. Received type ${typeof value} (${String(value)})`); if (!Number.isInteger(value) || value < 0) throw error(RangeError, 'ERR_OUT_OF_RANGE', `The value of "value" is out of range. It must be ${Number.isInteger(value) ? '>= 0 && <= 9007199254740991' : 'an integer'}. Received ${value}`); marks[+!!objectMode] = value; };
  const isErrored = stream => !!(stream && (stream.errored ?? stream._readableState?.errored ?? stream._writableState?.errored));
  const isReadable = stream => typeof stream?.readable !== 'boolean' ? null : !stream.destroyed && typeof stream.pipe === 'function' && stream.readable && !stream._readableState?.endEmitted;
  const waiting = Symbol('waiting');
  const duplexPair = options => {
    const sides = [0, 1].map(() => new Duplex({ ...options, read() { const callback = this[waiting]; this[waiting] = null; callback?.(); } }));
    sides.forEach((side, i) => {
      const other = sides[1 - i];
      side._write = (chunk, _encoding, callback) => { if (!chunk.length) queueMicrotask(callback); else { other.push(chunk); other[waiting] = callback; } };
      side._final = callback => { other.on('end', callback); other.push(null); };
    });
    return sides;
  };
  const combine = (writable, readable, options = {}) => {
    const duplex = new Duplex({ readableObjectMode: !!readable?.readableObjectMode, writableObjectMode: !!writable?.writableObjectMode,
      write(chunk, encoding, callback) { if (writable.write(chunk, encoding)) callback(); else writable.once('drain', () => callback()); },
      final(callback) { writable.end(); finished(writable, { readable: false }, failure => callback(failure)); },
      read() { readable?.resume(); },
      destroy(failure, callback) { writable?.destroy(failure); readable?.destroy(failure); callback(failure); }, ...options });
    if (!writable && !options.write) duplex.writable = false;
    if (!readable) duplex.readable = false;
    else {
      readable.on('data', chunk => { if (!duplex.push(chunk)) readable.pause(); });
      readable.on('end', () => duplex.push(null));
      readable.on('error', failure => duplex.destroy(failure));
      readable.pause();
    }
    writable?.on('error', failure => duplex.destroy(failure));
    return duplex;
  };
  Duplex.from = body => {
    if (body instanceof Duplex) return body;
    if (typeof body?.pipe === 'function' && typeof body.on === 'function') return combine(typeof body.write === 'function' && body.writable !== false ? body : null, body._readableState && body.readable !== false ? body : null);
    if (typeof ReadableStream === 'function' && body instanceof ReadableStream) return combine(null, Readable.fromWeb(body));
    if (typeof WritableStream === 'function' && body instanceof WritableStream) return combine(Writable.fromWeb(body), null);
    if (typeof body === 'function') {
      let resolve, next = new Promise(done => { resolve = done; });
      const controller = new AbortController();
      const output = body((async function* () { for (;;) { const { chunk, done, callback } = await next; queueMicrotask(callback); if (done) return; controller.signal.throwIfAborted(); next = new Promise(done => { resolve = done; }); yield chunk; } })(), { signal: controller.signal });
      return combine(null, Readable.from(output), { objectMode: true, write(chunk, _encoding, callback) { resolve({ chunk, callback }); }, final(callback) { resolve({ done: true, callback }); }, destroy(failure, callback) { controller.abort(); callback(failure); } });
    }
    if (typeof Blob === 'function' && body instanceof Blob) return Duplex.from(body.arrayBuffer());
    if (typeof body?.[Symbol.asyncIterator] === 'function' || typeof body?.[Symbol.iterator] === 'function') return combine(null, Readable.from(body));
    if (typeof body?.writable === 'object' || typeof body?.readable === 'object') return combine(body.writable ? Duplex.from(body.writable) : null, body.readable ? Duplex.from(body.readable) : null);
    if (typeof body?.then === 'function') return combine(null, Readable.from((async function* () { const value = await body; if (value != null) yield value; })()));
    throw error(TypeError, 'ERR_INVALID_ARG_TYPE', `The "body" argument must be of type function or an instance of Blob, ReadableStream, WritableStream, Stream, Iterable, AsyncIterable, or Promise or { readable, writable } pair. Received ${body == null ? String(body) : typeof body === 'object' ? `an instance of ${body.constructor?.name ?? 'Object'}` : `type ${typeof body} (${String(body)})`}`);
  };
  const compose = (...streams) => {
    if (streams.length === 1 && Array.isArray(streams[0])) streams = streams[0];
    if (!streams.length) throw error(TypeError, 'ERR_MISSING_ARGS', 'The "streams" argument must be specified');
    if (streams.length === 1) return Duplex.from(streams[0]);
    streams = streams.map(stream => typeof stream?.on === 'function' ? stream : Duplex.from(stream));
    let duplex;
    pipeline(streams, failure => { if (failure) duplex?.destroy(failure); });
    const head = streams[0], tail = streams.at(-1);
    return duplex = combine(typeof head.write === 'function' && head.writable !== false ? head : null, tail._readableState && tail.readable !== false ? tail : null);
  };
  const collect = async stream => { const chunks = []; for await (const chunk of stream) chunks.push(typeof chunk === 'string' ? Buffer.from(chunk) : chunk); return Buffer.concat(chunks); };
  const consumers = {
    buffer: collect,
    arrayBuffer: async stream => { const data = await collect(stream); return data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength); },
    text: async stream => (await collect(stream)).toString(),
    json: async stream => JSON.parse((await collect(stream)).toString()),
    blob: async stream => new Blob([await collect(stream)]),
  };
  return Object.assign(Stream, { Stream, Readable, Writable, Duplex, Transform, PassThrough, finished, pipeline, addAbortSignal, promises, consumers, compose, duplexPair, isErrored, isReadable, getDefaultHighWaterMark, setDefaultHighWaterMark });
};
