(() => {
  const readable = new WeakMap(), readers = new WeakMap(), writable = new WeakMap(), writers = new WeakMap();
  const controllers = new WeakMap(), requests = new WeakMap();
  const deferred = () => {
    let resolve, reject;
    const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
    promise.catch(() => {});
    return { promise, resolve, reject };
  };
  class Queue {
    items = [];
    head = 0;
    get length() { return this.items.length - this.head; }
    get first() { return this.items[this.head]; }
    push(value) { this.items.push(value); }
    unshift(value) { if (this.head) this.items[--this.head] = value; else this.items.unshift(value); }
    shift() {
      const value = this.items[this.head];
      this.items[this.head++] = undefined;
      if (this.head === this.items.length) { this.items.length = 0; this.head = 0; }
      else if (this.head > 1024 && this.head * 2 > this.items.length) { this.items = this.items.slice(this.head); this.head = 0; }
      return value;
    }
    clear() { this.items.length = 0; this.head = 0; }
  }
  const highWaterMark = (value, fallback) => {
    const number = value === undefined ? fallback : Number(value);
    if (Number.isNaN(number) || number < 0) throw new RangeError('Invalid highWaterMark');
    return number;
  };
  const invoke = (object, name, ...args) => {
    try { return Promise.resolve(object[name]?.(...args)); } catch (error) { return Promise.reject(error); }
  };
  const transfer = bytes => {
    const offset = bytes.byteOffset, length = bytes.byteLength;
    const buffer = typeof bytes.buffer.transfer === 'function' ? bytes.buffer.transfer() : bytes.buffer.slice(0);
    return new Uint8Array(buffer, offset, length);
  };
  const finishReadable = s => {
    s.state = 'closed';
    s.reader?.closed.resolve();
    while (s.reads.length) {
      const read = s.reads.shift();
      read.resolve({ value: read.view ? new read.ctor(read.view.buffer, read.offset, 0) : undefined, done: true });
    }
  };
  const errorReadable = (s, error) => {
    if (s.state !== 'readable') return;
    s.state = 'errored'; s.error = error; s.queue.clear(); s.total = 0;
    s.reader?.closed.reject(error);
    while (s.reads.length) s.reads.shift().reject(error);
  };
  const pull = s => {
    if (!s.started || s.state !== 'readable' || s.closing || (!s.reads.length && s.hwm <= s.total)) return;
    if (s.pulling) { s.pullAgain = true; return; }
    s.pulling = true;
    invoke(s.source, 'pull', s.controller).then(() => {
      s.pulling = false;
      if (s.pullAgain) { s.pullAgain = false; pull(s); }
    }, error => errorReadable(s, error));
  };
  const drain = s => {
    while (s.reads.length && s.queue.length) {
      const read = s.reads.first;
      if (!read.view) {
        const chunk = s.queue.shift(); s.total -= chunk.size;
        s.reads.shift().resolve({ value: chunk.value, done: false });
        continue;
      }
      const chunk = s.queue.first;
      const count = Math.min(read.view.byteLength - read.filled, chunk.value.byteLength);
      read.view.set(chunk.value.subarray(0, count), read.filled);
      read.filled += count; s.total -= count;
      if (count === chunk.value.byteLength) s.queue.shift();
      else chunk.value = chunk.value.subarray(count);
      const aligned = read.filled - read.filled % read.width;
      if (aligned >= read.min) {
        if (aligned < read.filled) {
          const rest = read.view.slice(aligned, read.filled);
          s.queue.unshift({ value: rest, size: rest.byteLength }); s.total += rest.byteLength;
        }
        s.reads.shift();
        read.resolve({ value: new read.ctor(read.view.buffer, read.offset, aligned / read.width), done: false });
      }
    }
    if (s.closing && !s.queue.length) {
      if (s.reads.length && s.reads.first.filled) {
        const read = s.reads.shift();
        if (read.filled % read.width) {
          const error = new TypeError('Incomplete byte stream element');
          read.reject(error); errorReadable(s, error); return;
        }
        read.resolve({ value: new read.ctor(read.view.buffer, read.offset, read.filled / read.width), done: false });
      }
      finishReadable(s);
    }
  };
  const cancel = (s, reason) => {
    s.disturbed = true;
    if (s.state === 'closed') return Promise.resolve();
    if (s.state === 'errored') return Promise.reject(s.error);
    s.queue.clear(); s.total = 0; finishReadable(s);
    return invoke(s.source, 'cancel', reason).then(() => undefined);
  };
  class ReadableStreamDefaultController {
    constructor(token) { if (token !== controllers) throw new TypeError('Illegal constructor'); }
    get desiredSize() { const s = controllers.get(this); return s.state === 'errored' ? null : s.state === 'closed' ? 0 : s.hwm - s.total; }
    close() {
      const s = controllers.get(this);
      if (s.closing || s.state !== 'readable') throw new TypeError('Stream is not readable');
      s.closing = true; drain(s);
    }
    enqueue(value) {
      const s = controllers.get(this);
      if (s.closing || s.state !== 'readable') throw new TypeError('Stream is not readable');
      if (s.reads.length) s.reads.shift().resolve({ value, done: false });
      else {
        try {
          const size = Number(s.size(value));
          if (!Number.isFinite(size) || size < 0) throw new RangeError('Invalid chunk size');
          s.queue.push({ value, size }); s.total += size;
        } catch (error) { errorReadable(s, error); throw error; }
      }
      pull(s);
    }
    error(error) { errorReadable(controllers.get(this), error); }
  }
  class ReadableByteStreamController extends ReadableStreamDefaultController {
    get byobRequest() {
      const s = controllers.get(this), read = s.reads.first;
      if (!read?.view || s.state !== 'readable') return null;
      if (!s.byob || requests.get(s.byob).read !== read || requests.get(s.byob).filled !== read.filled) {
        s.byob = new ReadableStreamBYOBRequest(requests);
        requests.set(s.byob, { s, read, filled: read.filled });
      }
      return s.byob;
    }
    enqueue(value) {
      const s = controllers.get(this);
      if (s.closing || s.state !== 'readable') throw new TypeError('Stream is not readable');
      if (!ArrayBuffer.isView(value) || !value.byteLength) throw new TypeError('Expected a nonempty byte view');
      const chunk = transfer(value);
      s.queue.push({ value: chunk, size: chunk.byteLength }); s.total += chunk.byteLength;
      drain(s); pull(s);
    }
  }
  class ReadableStreamBYOBRequest {
    constructor(token) { if (token !== requests) throw new TypeError('Illegal constructor'); }
    get view() {
      const r = requests.get(this);
      return r && (r.s.reads.first === r.read || r.s.state === 'closed') && r.filled === r.read.filled ? r.read.view.subarray(r.filled) : null;
    }
    respond(count) {
      const r = requests.get(this), view = this.view;
      count = Number(count);
      if (!view) throw new TypeError('BYOB request is no longer active');
      if (!Number.isInteger(count) || count < 0 || count > view.byteLength || (!count && r.s.state === 'readable')) throw new RangeError('Invalid byte count');
      requests.delete(this); r.s.byob = null;
      if (r.s.state === 'closed') { if (count) throw new TypeError('Closed stream requires zero bytes'); return; }
      r.read.filled += count;
      const read = r.read;
      if (read.filled >= read.min) {
        const aligned = read.filled - read.filled % read.width;
        if (aligned < read.filled) {
          const rest = read.view.slice(aligned, read.filled);
          r.s.queue.push({ value: rest, size: rest.byteLength }); r.s.total += rest.byteLength;
        }
        r.s.reads.shift();
        read.resolve({ value: new read.ctor(read.view.buffer, read.offset, aligned / read.width), done: false });
      }
      drain(r.s); pull(r.s);
    }
    respondWithNewView(view) {
      const current = this.view;
      if (!current || !ArrayBuffer.isView(view) || view.buffer !== current.buffer || view.byteOffset !== current.byteOffset) throw new RangeError('Invalid BYOB view');
      this.respond(view.byteLength);
    }
  }
  class ReadableStreamDefaultReader {
    constructor(stream) {
      const s = readable.get(stream);
      if (!s || s.reader) throw new TypeError('Stream is locked or invalid');
      const r = { stream: s, closed: deferred(), byob: false };
      readers.set(this, r); s.reader = r;
      if (s.state === 'closed') r.closed.resolve();
      if (s.state === 'errored') r.closed.reject(s.error);
    }
    get closed() { return readers.get(this).closed.promise; }
    read() {
      const r = readers.get(this), s = r.stream;
      if (!s) return Promise.reject(new TypeError('Reader was released'));
      s.disturbed = true;
      if (s.state === 'closed') return Promise.resolve({ value: undefined, done: true });
      if (s.state === 'errored') return Promise.reject(s.error);
      const read = deferred();
      if (s.autoAllocate) Object.assign(read, { view: new Uint8Array(s.autoAllocate), ctor: Uint8Array, offset: 0, width: 1, min: 1, filled: 0 });
      s.reads.push(read); drain(s); pull(s);
      return read.promise;
    }
    cancel(reason) { const s = readers.get(this).stream; return s ? cancel(s, reason) : Promise.reject(new TypeError('Reader was released')); }
    releaseLock() {
      const r = readers.get(this), s = r.stream;
      if (!s) return;
      const error = new TypeError('Reader was released');
      r.closed.reject(error); r.closed = deferred(); r.closed.reject(error);
      while (s.reads.length) {
        const read = s.reads.shift();
        if (read.filled) { const value = read.view.slice(0, read.filled); s.queue.unshift({ value, size: value.byteLength }); s.total += value.byteLength; }
        read.reject(error);
      }
      s.byob = null; s.reader = null; r.stream = null;
    }
  }
  class ReadableStreamBYOBReader extends ReadableStreamDefaultReader {
    constructor(stream) {
      if (!readable.get(stream)?.bytes) throw new TypeError('Expected a byte stream');
      super(stream); readers.get(this).byob = true;
    }
    read(view, options = {}) {
      const s = readers.get(this).stream;
      if (!s) return Promise.reject(new TypeError('Reader was released'));
      if (!ArrayBuffer.isView(view) || !view.byteLength) return Promise.reject(new TypeError('Expected a nonempty byte view'));
      const width = view.BYTES_PER_ELEMENT || 1, min = options.min === undefined ? 1 : Number(options.min);
      if (!Number.isInteger(min) || min < 1 || min * width > view.byteLength) return Promise.reject(new RangeError('Invalid minimum'));
      s.disturbed = true;
      if (s.state === 'errored') return Promise.reject(s.error);
      const read = deferred(), ctor = view.constructor, offset = view.byteOffset;
      Object.assign(read, { view: transfer(view), ctor, offset, width, min: min * width, filled: 0 });
      s.reads.push(read);
      if (s.state === 'closed') finishReadable(s);
      else { drain(s); pull(s); }
      return read.promise;
    }
  }
  class ReadableStream {
    constructor(source = {}, strategy = {}) {
      if (source.type !== undefined && source.type !== 'bytes') throw new RangeError('Invalid stream type');
      const bytes = source.type === 'bytes';
      if (bytes && strategy.size !== undefined) throw new RangeError('Byte streams do not accept size functions');
      if (strategy.size !== undefined && typeof strategy.size !== 'function') throw new TypeError('Invalid size function');
      const autoAllocate = source.autoAllocateChunkSize;
      if (bytes && autoAllocate !== undefined && (!Number.isInteger(autoAllocate) || autoAllocate <= 0)) throw new TypeError('Invalid allocation size');
      const controller = bytes ? new ReadableByteStreamController(controllers) : new ReadableStreamDefaultController(controllers);
      const s = { source, controller, bytes, autoAllocate, hwm: highWaterMark(strategy.highWaterMark, bytes ? 0 : 1), size: strategy.size ?? (() => 1),
        state: 'readable', reader: null, queue: new Queue(), reads: new Queue(), total: 0, started: false, closing: false, disturbed: false, pulling: false, pullAgain: false };
      readable.set(this, s); controllers.set(controller, s);
      const start = source.start?.(controller);
      Promise.resolve(start).then(() => { s.started = true; pull(s); }, error => errorReadable(s, error));
    }
    get locked() { return !!readable.get(this).reader; }
    cancel(reason) { const s = readable.get(this); return s.reader ? Promise.reject(new TypeError('Stream is locked')) : cancel(s, reason); }
    getReader(options = {}) {
      if (options.mode !== undefined && options.mode !== 'byob') throw new TypeError('Invalid reader mode');
      return options.mode === 'byob' ? new ReadableStreamBYOBReader(this) : new ReadableStreamDefaultReader(this);
    }
    async pipeTo(destination, options = {}) {
      if (!(destination instanceof WritableStream) || this.locked || destination.locked) throw new TypeError('Streams must be unlocked');
      const signal = options.signal;
      if (signal !== undefined && (typeof signal.addEventListener !== 'function' || typeof signal.aborted !== 'boolean')) throw new TypeError('Invalid abort signal');
      const reader = this.getReader(), writer = destination.getWriter();
      const stopped = deferred();
      let failure, stopping = false, finished = false, halted = false, interrupt = null;
      stopped.promise.catch(error => { halted = true; interrupt?.(error); });
      const race = promise => new Promise((resolve, reject) => { if (halted) stopped.promise.catch(reject); else interrupt = reject; promise.then(resolve, reject); });
      const stop = (reason, origin) => {
        if (stopping || finished) return;
        stopping = true; failure = reason;
        const actions = [];
        if (origin !== 'source' && !options.preventCancel) actions.push(reader.cancel(reason));
        if (origin !== 'destination' && !options.preventAbort) actions.push(writer.abort(reason));
        Promise.all(actions).then(() => stopped.reject(reason), stopped.reject);
      };
      const onAbort = () => stop(signal.reason, 'signal');
      if (signal?.aborted) onAbort();
      else signal?.addEventListener('abort', onAbort, { once: true });
      reader.closed.catch(error => stop(error, 'source'));
      writer.closed.then(() => { if (!finished) stop(new TypeError('Destination closed early'), 'destination'); }, error => stop(error, 'destination'));
      try {
        while (!stopping) {
          await race(writer.ready);
          const chunk = await race(reader.read());
          if (stopping) break;
          if (chunk.done) {
            finished = true;
            if (!options.preventClose) await writer.close();
            return;
          }
          try { await race(writer.write(chunk.value)); }
          catch (error) { stop(error, 'destination'); }
        }
        if (stopping) await stopped.promise;
      } catch (error) {
        if (!stopping) { stop(error, 'source'); await stopped.promise; }
        throw failure ?? error;
      } finally {
        finished = true; signal?.removeEventListener('abort', onAbort);
        reader.releaseLock(); writer.releaseLock();
      }
    }
    pipeThrough(transform, options) {
      if (!(transform?.readable instanceof ReadableStream) || !(transform.writable instanceof WritableStream) || this.locked || transform.writable.locked) throw new TypeError('Invalid transform');
      this.pipeTo(transform.writable, options).catch(() => {});
      return transform.readable;
    }
    tee() {
      const s = readable.get(this), reader = this.getReader(), canceled = [false, false], reasons = [], branches = [], outputs = [], complete = deferred();
      let reading = null;
      for (let i = 0; i < 2; i++) outputs.push(new ReadableStream({
        ...(s.bytes ? { type: 'bytes' } : {}),
        start(controller) { branches[i] = controller; },
        pull() {
          if (reading) return reading;
          reading = reader.read().then(({ value, done }) => {
            for (let j = 0; j < 2; j++) if (!canceled[j]) {
              if (done) branches[j].close();
              else branches[j].enqueue(s.bytes ? value.slice() : value);
            }
            if (done) { complete.resolve(); reader.releaseLock(); }
          }, error => {
            for (let j = 0; j < 2; j++) if (!canceled[j]) branches[j].error(error);
            complete.resolve(); reader.releaseLock();
          }).finally(() => { reading = null; });
          return reading;
        },
        cancel(reason) {
          canceled[i] = true; reasons[i] = reason;
          if (canceled[0] && canceled[1]) reader.cancel(reasons).then(complete.resolve, complete.reject).finally(() => reader.releaseLock());
          return complete.promise;
        },
      }));
      return outputs;
    }
    values({ preventCancel = false } = {}) {
      const reader = this.getReader();
      let finished = false, chain = Promise.resolve();
      return {
        next() {
          const result = chain.then(async () => {
            if (finished) return { value: undefined, done: true };
            try { const item = await reader.read(); if (item.done) { finished = true; reader.releaseLock(); } return item; }
            catch (error) { finished = true; reader.releaseLock(); throw error; }
          });
          chain = result.catch(() => {}); return result;
        },
        return(value) {
          const result = chain.then(async () => { if (!finished) { finished = true; try { if (!preventCancel) await reader.cancel(value); } finally { reader.releaseLock(); } } return { value, done: true }; });
          chain = result.catch(() => {}); return result;
        },
        [Symbol.asyncIterator]() { return this; },
      };
    }
    [Symbol.asyncIterator](options) { return this.values(options); }
    static from(iterable) {
      const iterator = iterable[Symbol.asyncIterator]?.() ?? iterable[Symbol.iterator]?.();
      if (!iterator) throw new TypeError('Expected an iterable');
      return new ReadableStream({
        async pull(controller) { const item = await iterator.next(); if (item.done) controller.close(); else controller.enqueue(await item.value); },
        async cancel(reason) { await iterator.return?.(reason); },
      }, { highWaterMark: 0 });
    }
  }
  const updatePressure = s => {
    const pressure = s.total >= s.hwm;
    if (pressure && !s.pressure) { s.ready = deferred(); if (s.writer) s.writer.ready = s.ready; }
    if (!pressure && s.pressure) s.ready.resolve();
    s.pressure = pressure;
  };
  const errorWritable = (s, error) => {
    if (s.state !== 'writable') return;
    s.state = 'errored'; s.error = error;
    if (!s.pressure) { s.ready = deferred(); if (s.writer) s.writer.ready = s.ready; }
    s.ready.reject(error); s.closed.reject(error);
    while (s.queue.length) s.queue.shift().reject(error);
    s.total = 0;
  };
  const advance = s => {
    if (!s.started || s.busy || s.state !== 'writable' || !s.queue.length) return;
    const item = s.queue.first; s.busy = true;
    s.inFlight = invoke(s.sink, item.close ? 'close' : 'write', ...(item.close ? [] : [item.value, s.controller]));
    s.inFlight.then(() => {
      s.busy = false;
      if (s.state !== 'writable') return;
      s.queue.shift(); s.total -= item.size; item.resolve(); updatePressure(s);
      if (item.close) { s.state = 'closed'; s.ready.resolve(); s.closed.resolve(); }
      else advance(s);
    }, error => { s.busy = false; errorWritable(s, error); });
  };
  const abortWritable = (s, reason) => {
    if (s.state !== 'writable') return Promise.resolve();
    s.abortController?.abort(reason);
    const pending = s.busy ? s.inFlight : null;
    errorWritable(s, reason);
    return Promise.resolve(pending).catch(() => {}).then(() => invoke(s.sink, 'abort', reason)).then(() => undefined);
  };
  const closeWritable = s => {
    if (s.state !== 'writable' || s.closing) return Promise.reject(s.error ?? new TypeError('Stream cannot close'));
    s.closing = true; s.ready.resolve();
    const item = { ...deferred(), close: true, size: 0 }; s.queue.push(item); advance(s); return item.promise;
  };
  class WritableStreamDefaultController {
    constructor(token) { if (token !== controllers) throw new TypeError('Illegal constructor'); }
    get signal() { const s = controllers.get(this); return (s.abortController ??= new AbortController()).signal; }
    error(error) { errorWritable(controllers.get(this), error); }
  }
  class WritableStreamDefaultWriter {
    constructor(stream) {
      const s = writable.get(stream);
      if (!s || s.writer) throw new TypeError('Stream is locked or invalid');
      const w = { stream: s, ready: s.ready, closed: deferred() }; s.writer = w; writers.set(this, w);
      s.closed.promise.then(w.closed.resolve, w.closed.reject);
    }
    get closed() { return writers.get(this).closed.promise; }
    get ready() { return writers.get(this).ready.promise; }
    get desiredSize() { const s = writers.get(this).stream; if (!s) throw new TypeError('Writer was released'); return s.state === 'errored' ? null : s.state === 'closed' ? 0 : s.hwm - s.total; }
    write(value) {
      const s = writers.get(this).stream;
      if (!s || s.state !== 'writable' || s.closing) return Promise.reject(s?.error ?? new TypeError('Stream is not writable'));
      try {
        const size = Number(s.size(value));
        if (!Number.isFinite(size) || size < 0) throw new RangeError('Invalid chunk size');
        const item = { ...deferred(), value, size }; s.queue.push(item); s.total += size; updatePressure(s); advance(s); return item.promise;
      } catch (error) { errorWritable(s, error); return Promise.reject(error); }
    }
    close() { const s = writers.get(this).stream; return s ? closeWritable(s) : Promise.reject(new TypeError('Writer was released')); }
    abort(reason) { const s = writers.get(this).stream; return s ? abortWritable(s, reason) : Promise.reject(new TypeError('Writer was released')); }
    releaseLock() {
      const w = writers.get(this), s = w.stream;
      if (!s) return;
      const error = new TypeError('Writer was released');
      w.ready.reject(error); w.closed.reject(error);
      w.ready = deferred(); w.closed = deferred(); w.ready.reject(error); w.closed.reject(error);
      s.ready = deferred(); if (s.state === 'errored') s.ready.reject(s.error); else if (!s.pressure) s.ready.resolve();
      w.stream = null; s.writer = null;
    }
  }
  class WritableStream {
    constructor(sink = {}, strategy = {}) {
      if (sink.type !== undefined) throw new RangeError('Invalid sink type');
      if (strategy.size !== undefined && typeof strategy.size !== 'function') throw new TypeError('Invalid size function');
      const controller = new WritableStreamDefaultController(controllers);
      const s = { sink, controller, hwm: highWaterMark(strategy.highWaterMark, 1), size: strategy.size ?? (() => 1), state: 'writable',
        queue: new Queue(), total: 0, busy: false, started: false, closing: false, pressure: false, ready: deferred(), closed: deferred(), writer: null };
      writable.set(this, s); controllers.set(controller, s);
      if (s.hwm > 0) s.ready.resolve(); else s.pressure = true;
      const start = sink.start?.(controller);
      Promise.resolve(start).then(() => { s.started = true; advance(s); }, error => errorWritable(s, error));
    }
    get locked() { return !!writable.get(this).writer; }
    getWriter() { return new WritableStreamDefaultWriter(this); }
    close() { const s = writable.get(this); return s.writer ? Promise.reject(new TypeError('Stream is locked')) : closeWritable(s); }
    abort(reason) { const s = writable.get(this); return s.writer ? Promise.reject(new TypeError('Stream is locked')) : abortWritable(s, reason); }
  }
  class TransformStreamDefaultController {
    constructor(token) { if (token !== controllers) throw new TypeError('Illegal constructor'); }
    get desiredSize() { return controllers.get(this).readable.desiredSize; }
    enqueue(value) { const s = controllers.get(this); s.readable.enqueue(value); if (s.readable.desiredSize <= 0 && !s.blocked) { s.blocked = true; s.ready = deferred(); } }
    error(error) { const s = controllers.get(this); s.readable.error(error); errorWritable(s.writable, error); s.ready.resolve(); }
    terminate() { const s = controllers.get(this); s.readable.close(); errorWritable(s.writable, new TypeError('Transform terminated')); s.ready.resolve(); }
  }
  class TransformStream {
    constructor(transformer = {}, writableStrategy = {}, readableStrategy = {}) {
      if (transformer.readableType !== undefined || transformer.writableType !== undefined) throw new RangeError('Transform types are not supported');
      const controller = new TransformStreamDefaultController(controllers);
      const s = { ready: deferred(), blocked: true };
      controllers.set(controller, s);
      const start = deferred();
      let canceled = false;
      const cancel = reason => { if (canceled) return; canceled = true; return transformer.cancel?.(reason); };
      this.readable = new ReadableStream({
        start(c) { s.readable = c; return start.promise; },
        pull() { s.blocked = false; s.ready.resolve(); },
        cancel(reason) { errorWritable(s.writable, reason); s.ready.resolve(); return cancel(reason); },
      }, { ...readableStrategy, highWaterMark: readableStrategy.highWaterMark ?? 0 });
      this.writable = new WritableStream({
        start() { return start.promise; },
        async write(value) {
          if (s.blocked) await s.ready.promise;
          if (s.writable.state !== 'writable') throw s.writable.error;
          try { if (transformer.transform) await transformer.transform(value, controller); else controller.enqueue(value); }
          catch (error) { controller.error(error); throw error; }
        },
        async close() { try { await transformer.flush?.(controller); s.readable.close(); } catch (error) { controller.error(error); throw error; } },
        abort(reason) { s.readable.error(reason); s.ready.resolve(); return cancel(reason); },
      }, writableStrategy);
      s.writable = writable.get(this.writable);
      try { start.resolve(transformer.start?.(controller)); } catch (error) { start.reject(error); throw error; }
    }
  }
  class ByteLengthQueuingStrategy {
    constructor(options) { this.highWaterMark = Number(options.highWaterMark); }
    size(chunk) { return chunk.byteLength; }
  }
  class CountQueuingStrategy {
    constructor(options) { this.highWaterMark = Number(options.highWaterMark); }
    size() { return 1; }
  }
  globalThis.__appsStreamDisturbed = stream => readable.get(stream)?.disturbed ?? false;
  Object.assign(globalThis, { ReadableStream, ReadableStreamDefaultReader, ReadableStreamBYOBReader, ReadableStreamDefaultController,
    ReadableByteStreamController, ReadableStreamBYOBRequest, WritableStream, WritableStreamDefaultWriter, WritableStreamDefaultController,
    TransformStream, TransformStreamDefaultController, ByteLengthQueuingStrategy, CountQueuingStrategy });
})();
