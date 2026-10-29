(() => {
  const events = Symbol('events'), maximum = Symbol('maxListeners'), errorMonitor = Symbol('events.errorMonitor');
  const abortError = signal => Object.assign(new Error('The operation was aborted', { cause: signal.reason }), { name: 'AbortError', code: 'ABORT_ERR' });
  function EventEmitter() { Object.defineProperty(this, events, { value: new Map(), configurable: true }); }
  EventEmitter.defaultMaxListeners = 10;
  const methods = Object.getOwnPropertyDescriptors(class {
    setMaxListeners(value) { if (!Number.isFinite(value) || value < 0) throw new RangeError('Invalid listener limit'); this[maximum] = value; return this; }
    getMaxListeners() { return this[maximum] ?? EventEmitter.defaultMaxListeners; }
    prependListener(name, listener) { this.on(name, listener); const list = this[events].get(name); list.unshift(list.pop()); return this; }
    prependOnceListener(name, listener) { this.once(name, listener); const list = this[events].get(name); list.unshift(list.pop()); return this; }
    on(name, listener) {
      if (typeof listener !== 'function') throw new TypeError('Listener must be a function');
      if (this[events].has('newListener')) this.emit('newListener', name, listener.listener ?? listener);
      const listeners = this[events].get(name) ?? [];
      listeners.push(listener);
      this[events].set(name, listeners);
      const max = this.getMaxListeners();
      if (max > 0 && listeners.length > max && !listeners.warned) {
        listeners.warned = true;
        globalThis.process?.emitWarning?.(Object.assign(new Error(`Possible EventEmitter memory leak detected. ${listeners.length} ${String(name)} listeners added to ${this.constructor?.name ?? 'EventEmitter'}. MaxListeners is ${max}. Use emitter.setMaxListeners() to increase limit`), { name: 'MaxListenersExceededWarning', emitter: this, type: name, count: listeners.length }));
      }
      return this;
    }
    addListener(name, listener) { return this.on(name, listener); }
    once(name, listener) {
      if (typeof listener !== 'function') throw new TypeError('Listener must be a function');
      let fired = false;
      const wrapper = (...args) => {
        if (fired) return;
        fired = true;
        this.off(name, wrapper);
        listener.apply(this, args);
      };
      wrapper.listener = listener;
      return this.on(name, wrapper);
    }
    off(name, listener) {
      const listeners = this[events].get(name);
      const index = listeners?.findLastIndex(item => item === listener || item.listener === listener) ?? -1;
      if (index < 0) return this;
      const [removed] = listeners.splice(index, 1);
      if (!listeners.length) this[events].delete(name);
      if (this[events].has('removeListener')) this.emit('removeListener', name, removed.listener ?? removed);
      return this;
    }
    removeListener(name, listener) { return this.off(name, listener); }
    removeAllListeners(name) {
      if (!this[events].has('removeListener')) { if (arguments.length) this[events].delete(name); else this[events].clear(); return this; }
      for (const key of arguments.length ? [name] : [...[...this[events].keys()].filter(key => key !== 'removeListener'), 'removeListener']) for (const listener of this.rawListeners(key).reverse()) this.off(key, listener);
      return this;
    }
    listeners(name) { return (this[events].get(name) ?? []).map(listener => listener.listener ?? listener); }
    rawListeners(name) { return [...(this[events].get(name) ?? [])]; }
    listenerCount(name, listener) { const listeners = this[events].get(name) ?? []; return listener ? listeners.filter(item => item === listener || item.listener === listener).length : listeners.length; }
    eventNames() { return [...this[events].keys()]; }
    emit(name, ...args) {
      if (name === 'error' && this[events].has(errorMonitor)) this.emit(errorMonitor, ...args);
      const listeners = this[events].get(name);
      if (!listeners?.length) {
        if (name === 'error') throw args[0] instanceof Error ? args[0] : new Error(String(args[0]));
        return false;
      }
      for (const listener of [...listeners]) listener.apply(this, args);
      return true;
    }
  }.prototype);
  delete methods.constructor;
  Object.defineProperties(EventEmitter.prototype, methods);
  // WARNING: Legacy subclasses often skip the constructor, so the listener map is created on first use.
  Object.defineProperty(EventEmitter.prototype, events, { configurable: true, get() { const map = new Map(); Object.defineProperty(this, events, { value: map, configurable: true }); return map; } });
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
  const getEventListeners = (emitter, name) => emitter.listeners?.(name) ?? [];
  const setMaxListeners = (value = EventEmitter.defaultMaxListeners, ...targets) => { if (!targets.length) EventEmitter.defaultMaxListeners = value; for (const target of targets) target.setMaxListeners?.(value); };
  globalThis.__appsEvents = Object.assign(EventEmitter, { EventEmitter, once, on, errorMonitor, getEventListeners, setMaxListeners });
})();
