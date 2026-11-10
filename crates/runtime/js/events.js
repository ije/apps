(() => {
  const capture = Symbol('kCapture'), errorMonitor = Symbol('events.errorMonitor'), captureRejectionSymbol = Symbol.for('nodejs.rejection'), targets = new WeakMap();
  const abortError = signal => Object.assign(new Error('The operation was aborted', { cause: signal.reason }), { name: 'AbortError', code: 'ABORT_ERR' });
  const received = value => value == null ? String(value) : typeof value === 'function' ? `function ${value.name}` : typeof value === 'object' ? `an instance of ${value.constructor?.name ?? 'Object'}` : `type ${typeof value} (${typeof value === 'string' ? `'${value.length > 28 ? value.slice(0, 25) + '...' : value}'` : typeof value === 'bigint' ? value + 'n' : Object.is(value, -0) ? '-0' : String(value)})`;
  const invalid = (name, types, value) => Object.assign(new TypeError(`The "${name}" ${name.includes('.') ? 'property' : 'argument'} must be ${types}. Received ${received(value)}`), { code: 'ERR_INVALID_ARG_TYPE' });
  const checkListener = listener => { if (typeof listener !== 'function') throw invalid('listener', 'of type function', listener); };
  const limit = (value, name) => { if (typeof value !== 'number') throw invalid(name, 'of type number', value); if (!(value >= 0)) throw Object.assign(new RangeError(`The value of "${name}" is out of range. It must be >= 0. Received ${value}`), { code: 'ERR_OUT_OF_RANGE' }); return value; };
  const rejected = (emitter, error, name, args) => {
    if (typeof emitter[captureRejectionSymbol] === 'function') return emitter[captureRejectionSymbol](error, name, ...args);
    const previous = emitter[capture];
    try { emitter[capture] = false; emitter.emit('error', error); } finally { emitter[capture] = previous; }
  };
  const caught = (emitter, result, name, args) => { if (result != null && typeof result.then === 'function') result.then(undefined, error => { const report = () => rejected(emitter, error, name, args); if (globalThis.process) process.nextTick(report); else queueMicrotask(report); }); };
  // Preserve lazy, string-keyed _events for subclasses that skip the constructor and mixins that copy
  // prototype properties.
  function EventEmitter(options) {
    if (this._events === undefined || this._events === Object.getPrototypeOf(this)._events) { this._events = { __proto__: null }; this._eventsCount = 0; }
    this._maxListeners ||= undefined;
    if (options?.captureRejections !== undefined) { if (typeof options.captureRejections !== 'boolean') throw invalid('options.captureRejections', 'of type boolean', options.captureRejections); this[capture] = options.captureRejections; }
  }
  EventEmitter.defaultMaxListeners = 10;
  const add = (target, name, listener, prepend) => {
    checkListener(listener);
    let events = target._events;
    if (events === undefined) { events = target._events = { __proto__: null }; target._eventsCount = 0; }
    else if (events.newListener !== undefined) { target.emit('newListener', name, listener.listener ?? listener); events = target._events; }
    let existing = events[name];
    if (existing === undefined) { events[name] = listener; ++target._eventsCount; return target; }
    if (typeof existing === 'function') existing = events[name] = prepend ? [listener, existing] : [existing, listener];
    else if (prepend) existing.unshift(listener); else existing.push(listener);
    const max = target._maxListeners ?? EventEmitter.defaultMaxListeners;
    if (max > 0 && existing.length > max && !existing.warned) {
      existing.warned = true;
      globalThis.process?.emitWarning?.(Object.assign(new Error(`Possible EventEmitter memory leak detected. ${existing.length} ${String(name)} listeners added to [${target.constructor?.name ?? 'EventEmitter'}]. MaxListeners is ${max}. Use emitter.setMaxListeners() to increase limit`), { name: 'MaxListenersExceededWarning', emitter: target, type: name, count: existing.length }));
    }
    return target;
  };
  const wrap = (target, name, listener) => {
    checkListener(listener);
    let fired = false;
    const wrapper = (...args) => { if (fired) return; fired = true; target.removeListener(name, wrapper); return listener.apply(target, args); };
    wrapper.listener = listener;
    return wrapper;
  };
  const list = (target, name, unwrap) => { const listener = target._events?.[name]; return listener === undefined ? [] : typeof listener === 'function' ? [unwrap ? listener.listener ?? listener : listener] : unwrap ? listener.map(item => item.listener ?? item) : [...listener]; };
  Object.assign(EventEmitter.prototype, {
    _events: undefined, _eventsCount: 0, _maxListeners: undefined, [capture]: false,
    setMaxListeners(value) { this._maxListeners = limit(value, 'setMaxListeners'); return this; },
    getMaxListeners() { return this._maxListeners ?? EventEmitter.defaultMaxListeners; },
    on(name, listener) { return add(this, name, listener, false); },
    addListener(name, listener) { return add(this, name, listener, false); },
    prependListener(name, listener) { return add(this, name, listener, true); },
    once(name, listener) { return add(this, name, wrap(this, name, listener), false); },
    prependOnceListener(name, listener) { return add(this, name, wrap(this, name, listener), true); },
    removeListener(name, listener) {
      checkListener(listener);
      const events = this._events, current = events?.[name];
      if (current === undefined) return this;
      if (current === listener || current.listener === listener) {
        if (--this._eventsCount === 0) this._events = { __proto__: null };
        else { delete events[name]; if (events.removeListener) this.emit('removeListener', name, current.listener || listener); }
      } else if (typeof current !== 'function') {
        const index = current.findLastIndex(item => item === listener || item.listener === listener);
        if (index < 0) return this;
        current.splice(index, 1);
        if (current.length === 1) events[name] = current[0];
        if (events.removeListener !== undefined) this.emit('removeListener', name, listener);
      }
      return this;
    },
    off(name, listener) { return this.removeListener(name, listener); },
    removeAllListeners(name) {
      const events = this._events;
      if (events === undefined) return this;
      if (events.removeListener === undefined) {
        if (!arguments.length) { this._events = { __proto__: null }; this._eventsCount = 0; }
        else if (events[name] !== undefined) { if (--this._eventsCount === 0) this._events = { __proto__: null }; else delete events[name]; }
        return this;
      }
      if (!arguments.length) {
        for (const key of Reflect.ownKeys(events)) if (key !== 'removeListener') this.removeAllListeners(key);
        this.removeAllListeners('removeListener');
        this._events = { __proto__: null }; this._eventsCount = 0;
        return this;
      }
      for (const listener of list(this, name, false).reverse()) this.removeListener(name, listener);
      return this;
    },
    listeners(name) { return list(this, name, true); },
    rawListeners(name) { return list(this, name, false); },
    listenerCount(name, listener) { const all = list(this, name, false); return listener == null ? all.length : all.filter(item => item === listener || item.listener === listener).length; },
    eventNames() { return this._eventsCount > 0 ? Reflect.ownKeys(this._events) : []; },
    emit(name, ...args) {
      const events = this._events;
      let error = name === 'error';
      if (events !== undefined) { if (error && events[errorMonitor] !== undefined) this.emit(errorMonitor, ...args); error &&= events.error === undefined; }
      else if (!error) return false;
      if (error) {
        if (args[0] instanceof Error) throw args[0];
        let text;
        try { text = (globalThis.__apps?.util.inspect ?? String)(args[0]); } catch { text = args[0]; }
        throw Object.assign(new Error(`Unhandled error. (${text})`), { code: 'ERR_UNHANDLED_ERROR', context: args[0] });
      }
      const handler = events[name];
      if (handler === undefined) return false;
      if (typeof handler === 'function') { const result = handler.apply(this, args); if (this[capture]) caught(this, result, name, args); return true; }
      for (const listener of [...handler]) { const result = listener.apply(this, args); if (this[capture]) caught(this, result, name, args); }
      return true;
    },
  });
  const once = (emitter, name, options = {}) => new Promise((resolve, reject) => {
    const signal = options.signal;
    const events = typeof emitter.once === 'function';
    const remove = (event, listener) => events ? emitter.removeListener(event, listener) : emitter.removeEventListener(event, listener);
    const cleanup = () => {
      remove(name, success);
      if (events && name !== 'error') remove('error', failure);
      signal?.removeEventListener('abort', aborted);
    };
    const success = (...args) => { cleanup(); resolve(args); };
    const failure = error => { cleanup(); reject(error); };
    const aborted = () => failure(abortError(signal));
    if (signal?.aborted) { aborted(); return; }
    if (events) {
      emitter.once(name, success);
      if (name !== 'error') emitter.once('error', failure);
    } else emitter.addEventListener(name, success, { once: true });
    signal?.addEventListener('abort', aborted, { once: true });
  });
  const on = (emitter, name, { signal, close = [] } = {}) => {
    if (signal?.aborted) throw abortError(signal);
    const events = typeof emitter.on === 'function', queue = [], waiters = [];
    let failed = null, done = false;
    const listen = (event, listener) => events ? emitter.on(event, listener) : emitter.addEventListener(event, listener);
    const unlisten = (event, listener) => events ? emitter.removeListener(event, listener) : emitter.removeEventListener(event, listener);
    const push = (...args) => { if (waiters.length) waiters.shift().resolve({ value: args, done: false }); else queue.push(args); };
    const finish = () => { if (done) return; done = true; unlisten(name, push); if (events && name !== 'error') unlisten('error', fail); for (const event of close) unlisten(event, finish); signal?.removeEventListener('abort', aborted); for (const waiter of waiters.splice(0)) waiter.resolve({ value: undefined, done: true }); };
    const fail = error => { if (waiters.length) waiters.shift().reject(error); else failed = error; finish(); };
    const aborted = () => fail(abortError(signal));
    listen(name, push);
    if (events && name !== 'error') listen('error', fail);
    for (const event of close) listen(event, finish);
    signal?.addEventListener('abort', aborted, { once: true });
    return {
      next() {
        if (queue.length) return Promise.resolve({ value: queue.shift(), done: false });
        if (failed) { const error = failed; failed = null; return Promise.reject(error); }
        return done ? Promise.resolve({ value: undefined, done: true }) : new Promise((resolve, reject) => waiters.push({ resolve, reject }));
      },
      return() { finish(); return Promise.resolve({ value: undefined, done: true }); },
      throw(error) { fail(error); return Promise.reject(error); },
      [Symbol.asyncIterator]() { return this; },
    };
  };
  let targetListeners = () => [];
  const getEventListeners = (emitter, name) => typeof EventTarget !== 'undefined' && emitter instanceof EventTarget ? targetListeners(emitter, name) : emitter.listeners?.(name) ?? [];
  globalThis.__appsEventTargets = listeners => { targetListeners = listeners; };
  const getMaxListeners = target => { if (typeof target?.getMaxListeners === 'function') return target.getMaxListeners(); if (target instanceof EventTarget) return targets.get(target) ?? EventEmitter.defaultMaxListeners; throw invalid('emitter', 'an instance of EventEmitter or EventTarget', target); };
  const setMaxListeners = (value = EventEmitter.defaultMaxListeners, ...list) => {
    limit(value, 'setMaxListeners');
    if (!list.length) EventEmitter.defaultMaxListeners = value;
    for (const target of list) { if (target instanceof EventTarget) targets.set(target, value); else if (typeof target?.setMaxListeners === 'function') target.setMaxListeners(value); else throw invalid('eventTargets', 'an instance of EventEmitter or EventTarget', target); }
  };
  const addAbortListener = (signal, listener) => {
    if (!(signal instanceof AbortSignal)) throw invalid('signal', 'an instance of AbortSignal', signal);
    checkListener(listener);
    if (signal.aborted) queueMicrotask(() => listener());
    else signal.addEventListener('abort', listener, { once: true });
    return { [Symbol.dispose]() { signal.removeEventListener('abort', listener); } };
  };
  Object.defineProperty(EventEmitter, 'captureRejections', { enumerable: true, get: () => EventEmitter.prototype[capture], set: value => { if (typeof value !== 'boolean') throw invalid('EventEmitter.captureRejections', 'of type boolean', value); EventEmitter.prototype[capture] = value; } });
  globalThis.__appsEvents = Object.assign(EventEmitter, { EventEmitter, once, on, errorMonitor, captureRejectionSymbol, getEventListeners, getMaxListeners, setMaxListeners, addAbortListener });
})();
