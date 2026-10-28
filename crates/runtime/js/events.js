(() => {
  const events = Symbol('events'), maximum = Symbol('maxListeners');
  function EventEmitter() { Object.defineProperty(this, events, { value: new Map(), configurable: true }); }
  EventEmitter.defaultMaxListeners = 10;
  const methods = Object.getOwnPropertyDescriptors(class {
    setMaxListeners(value) { if (!Number.isFinite(value) || value < 0) throw new RangeError('Invalid listener limit'); this[maximum] = value; return this; }
    getMaxListeners() { return this[maximum] ?? EventEmitter.defaultMaxListeners; }
    prependListener(name, listener) { this.on(name, listener); const list = this[events].get(name); list.unshift(list.pop()); return this; }
    prependOnceListener(name, listener) { this.once(name, listener); const list = this[events].get(name); list.unshift(list.pop()); return this; }
    on(name, listener) {
      if (typeof listener !== 'function') throw new TypeError('Listener must be a function');
      const listeners = this[events].get(name) ?? [];
      listeners.push(listener);
      this[events].set(name, listeners);
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
      if (index >= 0) listeners.splice(index, 1);
      if (listeners?.length === 0) this[events].delete(name);
      return this;
    }
    removeListener(name, listener) { return this.off(name, listener); }
    removeAllListeners(name) { if (arguments.length) this[events].delete(name); else this[events].clear(); return this; }
    listeners(name) { return (this[events].get(name) ?? []).map(listener => listener.listener ?? listener); }
    rawListeners(name) { return [...(this[events].get(name) ?? [])]; }
    listenerCount(name) { return this[events].get(name)?.length ?? 0; }
    eventNames() { return [...this[events].keys()]; }
    emit(name, ...args) {
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
    const aborted = () => {
      const error = new Error('The operation was aborted', { cause: signal.reason });
      error.name = 'AbortError';
      error.code = 'ABORT_ERR';
      failure(error);
    };
    if (signal?.aborted) { aborted(); return; }
    if (events) {
      emitter.once(name, success);
      if (name !== 'error') emitter.once('error', failure);
    } else emitter.addEventListener(name, success, { once: true });
    signal?.addEventListener('abort', aborted, { once: true });
  });
  globalThis.__appsEvents = Object.assign(EventEmitter, { EventEmitter, once });
})();
