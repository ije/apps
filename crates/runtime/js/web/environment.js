globalThis.__appsBrowserEnvironment = (call, task) => {
  const codec = globalThis.__appsStorageClone, retries = globalThis.__appsRetries;
  delete globalThis.__appsStorageClone; delete globalThis.__appsRetries;
  const clone = value => codec.decode(codec.encode(value));
  const fail = (name, message = name) => new DOMException(message, name);
  const native = (op, ...args) => {
    try { return call('web.storage.' + op, ...args); }
    catch (error) { throw fail(error.message.includes('UNIQUE constraint') ? 'ConstraintError' : error.message.startsWith('QuotaExceededError') ? 'QuotaExceededError' : 'UnknownError', error.message); }
  };
  const storageKinds = new WeakMap();
  class Storage {
    constructor(kind) { storageKinds.set(this, kind); }
    get length() { return native(storageKinds.get(this) + '.length'); }
    key(index) { return native(storageKinds.get(this) + '.key', Number(index) >>> 0); }
    getItem(key) { return native(storageKinds.get(this) + '.get', String(key)); }
    setItem(key, value) { native(storageKinds.get(this) + '.set', String(key), String(value)); }
    removeItem(key) { native(storageKinds.get(this) + '.remove', String(key)); }
    clear() { native(storageKinds.get(this) + '.clear'); }
  }
  for (const [name, kind] of [['localStorage', 'local'], ['sessionStorage', 'session']]) globalThis[name] = new Proxy(new Storage(kind), {
    get(target, key) { if (typeof key === 'symbol' || key in target) { const value = Reflect.get(target, key, target); return typeof value === 'function' ? value.bind(target) : value; } return target.getItem(key) ?? undefined; },
    set(target, key, value) { target.setItem(key, value); return true; },
    deleteProperty(target, key) { target.removeItem(key); return true; },
    ownKeys(target) { return native(storageKinds.get(target) + '.keys'); },
    getOwnPropertyDescriptor(target, key) { const value = target.getItem(key); return value === null ? undefined : { value, enumerable: true, configurable: true, writable: true }; },
  });
  const keyWire = (key, seen = new Set()) => {
    if (typeof key === 'number' && !Number.isNaN(key)) return [0, key === Infinity ? '+' : key === -Infinity ? '-' : key];
    if (key instanceof Date && !Number.isNaN(+key)) return [1, +key];
    if (typeof key === 'string') return [2, key];
    if (key instanceof ArrayBuffer || ArrayBuffer.isView(key)) return [3, Array.from(new Uint8Array(key.buffer ?? key, key.byteOffset ?? 0, key.byteLength))];
    if (Array.isArray(key) && !seen.has(key) && Array.from({ length: key.length }, (_, i) => i in key).every(Boolean)) { seen.add(key); const wire = [4, key.map(value => keyWire(value, seen))]; seen.delete(key); return wire; }
    throw fail('DataError', 'Invalid IndexedDB key');
  };
  const keyValue = key => key[0] === 0 ? key[1] === '+' ? Infinity : key[1] === '-' ? -Infinity : key[1] : key[0] === 1 ? new Date(key[1]) : key[0] === 3 ? new Uint8Array(key[1]).buffer : key[0] === 4 ? key[1].map(keyValue) : key[1];
  const compare = (a, b) => {
    if (a[0] !== b[0]) return Math.sign(a[0] - b[0]);
    if (a[0] >= 3) { for (let i = 0; i < Math.min(a[1].length, b[1].length); i++) { const c = a[0] === 4 ? compare(a[1][i], b[1][i]) : Math.sign(a[1][i] - b[1][i]); if (c) return c; } return Math.sign(a[1].length - b[1].length); }
    const x = a[0] === 0 ? keyValue(a) : a[1], y = b[0] === 0 ? keyValue(b) : b[1]; return x < y ? -1 : x > y ? 1 : 0;
  };
  class IDBKeyRange {
    constructor(lower, upper, lowerOpen, upperOpen) { Object.assign(this, { lower, upper, lowerOpen, upperOpen }); if (lower !== undefined && upper !== undefined) { const c = compare(keyWire(lower), keyWire(upper)); if (c > 0 || c === 0 && (lowerOpen || upperOpen)) throw fail('DataError'); } }
    includes(key) { const k = keyWire(key); return (this.lower === undefined || compare(k, keyWire(this.lower)) >= Number(this.lowerOpen)) && (this.upper === undefined || compare(k, keyWire(this.upper)) <= -Number(this.upperOpen)); }
    static only(key) { keyWire(key); return new this(key, key, false, false); }
    static lowerBound(key, open = false) { keyWire(key); return new this(key, undefined, !!open, false); }
    static upperBound(key, open = false) { keyWire(key); return new this(undefined, key, false, !!open); }
    static bound(lower, upper, lowerOpen = false, upperOpen = false) { keyWire(lower); keyWire(upper); return new this(lower, upper, !!lowerOpen, !!upperOpen); }
  }
  const names = values => Object.assign([...values].sort(), { contains(value) { return this.includes(String(value)); }, item(index) { return this[index] ?? null; } });
  const connections = new Map(), transactions = [], blocked = new Set();
  const unblock = () => { if (blocked.size) queueMicrotask(() => { for (const retry of [...blocked]) { blocked.delete(retry); retry(); } }); };
  let running = false;
  const schedule = () => {
    if (running || !transactions.length) return;
    running = true;
    const tx = transactions[0];
    const begin = () => {
      if (tx._done) { running = false; transactions.shift(); schedule(); return; }
      try { const result = native('begin', tx.db.name, tx.mode); if (result.busy) { retries.add(begin); setTimeout(() => { if (retries.delete(begin)) begin(); }, 50); return; } tx._id = result.id; tx._start?.(); tx._drain(); }
      catch (error) { tx._abort(error); }
    };
    task(begin);
  };
  class IDBRequest extends EventTarget {
    constructor(source = null, transaction = null) { super(); Object.assign(this, { source, transaction, readyState: 'pending', result: undefined, error: null }); }
    _success(result) { this.result = result; this.readyState = 'done'; this.dispatchEvent(new Event('success')); }
    _error(error) { this.error = error; this.readyState = 'done'; const event = new Event('error', { cancelable: true }); this.dispatchEvent(event); return event.defaultPrevented; }
  }
  class IDBOpenDBRequest extends IDBRequest {}
  class IDBTransaction extends EventTarget {
    constructor(db, stores, mode, start) {
      super(); if (!['readonly', 'readwrite', 'versionchange'].includes(mode)) throw new TypeError('Invalid transaction mode');
      Object.assign(this, { db, mode, durability: 'default', objectStoreNames: names(stores), error: null, _queue: [], _done: false, _id: null, _start: start });
      transactions.push(this); schedule();
    }
    objectStore(name) { name = String(name); if (this._done) throw fail('InvalidStateError'); if (!this.objectStoreNames.contains(name) || !Object.hasOwn(this.db._stores, name)) throw fail('NotFoundError'); return new IDBObjectStore(this, name); }
    abort() { if (this._done) throw fail('InvalidStateError'); this._abort(fail('AbortError')); }
    commit() { if (this._done) throw fail('InvalidStateError'); this._drain(); }
    _request(source, operation, request = new IDBRequest(source, this)) { if (this._done) throw fail('TransactionInactiveError'); this._queue.push([request, operation]); if (this._id !== null) this._drain(); return request; }
    _drain() {
      if (this._scheduled || this._done || this._id === null) return;
      this._scheduled = true;
      task(() => {
        this._scheduled = false; if (this._done) return;
        const next = this._queue.shift();
        if (next) {
          try { const result = next[1](); next[0]._success(result); }
          catch (error) { if (!next[0]._error(error)) { this._abort(error); return; } }
          this._drain();
        } else {
          try { native('commit', this._id); this._finish(); this.dispatchEvent(new Event('complete')); }
          catch (error) { this._abort(error); }
        }
      });
    }
    _finish() { this._done = true; this.db._transactions.delete(this); unblock(); if (this.db._closed && !this.db._transactions.size) connections.get(this.db.name)?.delete(this.db); if (transactions[0] === this) { transactions.shift(); running = false; schedule(); } }
    _abort(error) {
      if (this._done) return;
      if (this._id !== null) { try { native('abort', this._id); } catch {} }
      this.error = error;
      for (const [request] of this._queue.splice(0)) request._error(fail('AbortError'));
      if (this.mode === 'versionchange') this.db._stores = this._previousStores;
      this._finish(); this.dispatchEvent(new Event('error')); this.dispatchEvent(new Event('abort'));
    }
  }
  class IDBObjectStore {
    constructor(transaction, name) { Object.assign(this, { transaction, name }); }
    get keyPath() { return this.transaction.db._stores[this.name].keyPath; }
    get autoIncrement() { return !!this.transaction.db._stores[this.name].autoIncrement; }
    get indexNames() { return names([]); }
    _range(query) {
      if (query === undefined || query === null) return [null, null, false, false];
      const range = query instanceof IDBKeyRange ? query : IDBKeyRange.only(query), wire = key => key === undefined ? null : JSON.stringify(keyWire(key));
      return [wire(range.lower), wire(range.upper), !!range.lowerOpen, !!range.upperOpen];
    }
    _scan(range, kind, count = 0, reverse = false) { return native('scan', this.transaction._id, this.name, ...range, reverse, count, kind); }
    get(query) { if (query === undefined || query === null) throw fail('DataError'); const range = this._range(query); return this.transaction._request(this, () => { const value = this._scan(range, 'values', 1)[0]; return value === undefined ? undefined : codec.decode(JSON.parse(value)); }); }
    getKey(query) { if (query === undefined || query === null) throw fail('DataError'); const range = this._range(query); return this.transaction._request(this, () => { const key = this._scan(range, 'keys', 1)[0]; return key === undefined ? undefined : keyValue(key); }); }
    getAll(query, count) { const range = this._range(query); return this.transaction._request(this, () => this._scan(range, 'values', count).map(value => codec.decode(JSON.parse(value)))); }
    getAllKeys(query, count) { const range = this._range(query); return this.transaction._request(this, () => this._scan(range, 'keys', count).map(key => keyValue(key))); }
    count(query) { const range = this._range(query); return this.transaction._request(this, () => this._scan(range, 'count')); }
    _write(op, value, key) {
      if (this.transaction.mode === 'readonly') throw fail('ReadOnlyError');
      if (this.keyPath !== null) { if (key !== undefined) throw fail('DataError'); const read = path => path === '' ? value : String(path).split('.').reduce((v, part) => v?.[part], value); key = Array.isArray(this.keyPath) ? this.keyPath.map(read) : read(this.keyPath); }
      const encoded = JSON.stringify(keyWire(key)), data = JSON.stringify(codec.encode(value));
      return this.transaction._request(this, () => { native(op, this.transaction._id, this.name, encoded, data); return keyValue(JSON.parse(encoded)); });
    }
    put(value, key) { return this._write('put', value, key); }
    add(value, key) { return this._write('add', value, key); }
    delete(query) { if (this.transaction.mode === 'readonly') throw fail('ReadOnlyError'); if (query === undefined || query === null) throw fail('DataError'); const range = this._range(query); return this.transaction._request(this, () => { native('delete', this.transaction._id, this.name, ...range); }); }
    clear() { if (this.transaction.mode === 'readonly') throw fail('ReadOnlyError'); return this.transaction._request(this, () => { native('clear', this.transaction._id, this.name); }); }
    openCursor(query, direction = 'next') { return this._cursor(query, direction, false); }
    openKeyCursor(query, direction = 'next') { return this._cursor(query, direction, true); }
    _cursor(query, direction, keyOnly) {
      if (!['next', 'nextunique', 'prev', 'prevunique'].includes(direction)) throw new TypeError('Invalid cursor direction');
      const request = new IDBRequest(this, this.transaction), range = this._range(query), reverse = direction.startsWith('prev');
      const from = (wire, open) => { range[reverse ? 1 : 0] = JSON.stringify(wire); range[reverse ? 3 : 2] = open; };
      const step = () => {
        const row = this._scan(range, keyOnly ? 'keys' : 'rows', 1, reverse)[0]; if (!row) return null;
        const key = keyOnly ? row : row[0], cursor = new IDBCursor(this, request, direction, keyValue(key), keyOnly ? undefined : codec.decode(JSON.parse(row[1])));
        from(key, true);
        cursor._continue = target => { if (target !== undefined) { const wire = keyWire(target), order = compare(wire, key); if (reverse ? order >= 0 : order <= 0) throw fail('DataError'); from(wire, false); } this.transaction._request(this, step, request); };
        return cursor;
      };
      return this.transaction._request(this, step, request);
    }
  }
  class IDBCursor {
    constructor(source, request, direction, key, value) { Object.assign(this, { source, request, direction, key, primaryKey: key, value, _advanced: false }); }
    continue(key) { if (this._advanced) throw fail('InvalidStateError'); this._continue(key); this._advanced = true; this.request.readyState = 'pending'; }
    delete() { return this.source.delete(this.primaryKey); }
    update(value) { return this.source.put(value, this.source.keyPath === null ? this.primaryKey : undefined); }
  }
  class IDBDatabase extends EventTarget {
    constructor(name, version, stores) { super(); Object.assign(this, { name, version, _stores: Object.assign(Object.create(null), stores), _transactions: new Set(), _closed: false }); const list = connections.get(name) ?? new Set(); list.add(this); connections.set(name, list); }
    get objectStoreNames() { return names(Object.keys(this._stores)); }
    createObjectStore(name, options = {}) {
      name = String(name); const tx = this._upgrade; if (!tx || tx._done) throw fail('InvalidStateError'); if (Object.hasOwn(this._stores, name)) throw fail('ConstraintError');
      if (options.autoIncrement) throw fail('NotSupportedError', 'Automatic key generators are not supported');
      const config = { keyPath: options.keyPath ?? null, autoIncrement: false }; native('createStore', tx._id, name, config); this._stores[name] = config; tx.objectStoreNames = this.objectStoreNames; return tx.objectStore(name);
    }
    deleteObjectStore(name) { const tx = this._upgrade; if (!tx || tx._done) throw fail('InvalidStateError'); if (!Object.hasOwn(this._stores, name)) throw fail('NotFoundError'); native('deleteStore', tx._id, name); delete this._stores[name]; tx.objectStoreNames = this.objectStoreNames; }
    transaction(stores, mode = 'readonly') { if (this._closed || this._upgrade && !this._upgrade._done) throw fail('InvalidStateError'); stores = typeof stores === 'string' ? [stores] : Array.from(stores); if (!stores.length) throw fail('InvalidAccessError'); for (const name of stores) if (!Object.hasOwn(this._stores, name)) throw fail('NotFoundError'); const tx = new IDBTransaction(this, stores, mode); this._transactions.add(tx); return tx; }
    close() { this._closed = true; if (!this._transactions.size) connections.get(this.name)?.delete(this); unblock(); }
  }
  class IDBFactory {
    cmp(a, b) { return compare(keyWire(a), keyWire(b)); }
    databases() { return Promise.resolve(native('databases')); }
    open(name, version) {
      name = String(name); if (version !== undefined && (!Number.isSafeInteger(version) || version <= 0)) throw new TypeError('Invalid database version');
      const request = new IDBOpenDBRequest();
      task(() => {
        try {
          const info = native('open', name), target = version ?? (info.version || 1);
          if (target < info.version) throw fail('VersionError');
          if (target === info.version) { request._success(new IDBDatabase(name, target, info.stores)); return; }
          const prior = [...connections.get(name) ?? []];
          for (const db of prior) if (!db._closed) db.dispatchEvent(Object.assign(new Event('versionchange'), { oldVersion: info.version, newVersion: target }));
          let waited = false;
          const upgrade = () => {
            if (prior.some(db => !db._closed || db._transactions.size)) { if (!waited) { request.dispatchEvent(Object.assign(new Event('blocked'), { oldVersion: info.version, newVersion: target })); waited = true; } blocked.add(upgrade); return; }
            const db = new IDBDatabase(name, target, info.stores), tx = new IDBTransaction(db, Object.keys(info.stores), 'versionchange', () => {
              native('version', tx._id, target); request.result = db; request.transaction = tx; request.readyState = 'done'; request.dispatchEvent(Object.assign(new Event('upgradeneeded'), { oldVersion: info.version, newVersion: target }));
            });
            tx._previousStores = clone(info.stores); db._upgrade = tx; db._transactions.add(tx);
            tx.addEventListener('complete', () => { request.transaction = null; db._upgrade = null; request._success(db); });
            tx.addEventListener('abort', () => { db.close(); request._error(tx.error); });
          }; upgrade();
        } catch (error) { request._error(error); }
      }); return request;
    }
    deleteDatabase(name) {
      name = String(name); const request = new IDBOpenDBRequest();
      task(() => {
        try {
          const info = native('open', name), prior = [...connections.get(name) ?? []];
          for (const db of prior) if (!db._closed) db.dispatchEvent(Object.assign(new Event('versionchange'), { oldVersion: info.version, newVersion: null }));
          let waited = false;
          const remove = () => {
            if (prior.some(db => !db._closed || db._transactions.size)) { if (!waited) { request.dispatchEvent(new Event('blocked')); waited = true; } blocked.add(remove); return; }
            const db = new IDBDatabase(name, info.version, info.stores), tx = new IDBTransaction(db, [], 'versionchange', () => native('deleteDatabase', tx._id));
            tx.addEventListener('complete', () => { db.close(); request._success(undefined); }); tx.addEventListener('abort', () => { db.close(); request._error(tx.error); });
          }; remove();
        } catch (error) { request._error(error); }
      }); return request;
    }
  }
  Object.assign(globalThis, { Storage, IDBFactory, IDBDatabase, IDBTransaction, IDBRequest, IDBOpenDBRequest, IDBObjectStore, IDBCursor, IDBCursorWithValue: IDBCursor, IDBKeyRange, indexedDB: new IDBFactory() });
  let current = new URL('apps://app/'), entries = [{ url: current.href, state: null }], position = 0;
  const changed = oldURL => { if (new URL(oldURL).hash !== current.hash) setTimeout(() => dispatchEvent(Object.assign(new Event('hashchange'), { oldURL, newURL: current.href })), 0); };
  const navigate = (url, replace = false, reload = false) => {
    const target = new URL(String(url), current), oldURL = current.href;
    if (!reload && target.href.split('#')[0] === current.href.split('#')[0]) {
      current = target; if (replace) entries[position] = { url: current.href, state: null }; else { entries.splice(++position); entries.push({ url: current.href, state: null }); } dispatchEvent(Object.assign(new Event('popstate'), { state: null })); changed(oldURL); entryChanged();
    } else call('dom.navigate', target.href, replace, reload);
  };
  // WARNING: URL origins of non-special schemes are opaque, but code uses `location.origin` as a base URL, as with a privileged Electron scheme.
  const originOf = url => url.protocol === 'apps:' ? 'apps://app' : url.protocol === 'file:' ? 'file://' : url.origin;
  class Location {
    assign(url) { navigate(url); }
    replace(url) { navigate(url, true); }
    reload() { navigate(current.href, true, true); }
    toString() { return current.href; }
    valueOf() { return this.toString(); }
  }
  for (const name of ['href', 'origin', 'protocol', 'host', 'hostname', 'port', 'pathname', 'search', 'hash']) Object.defineProperty(Location.prototype, name, { enumerable: true, get: () => name === 'origin' ? originOf(current) : current[name], ...(name === 'origin' ? {} : { set: value => { const url = new URL(current); url[name] = value; if (name !== 'hash' || url.hash !== current.hash) navigate(url); } }) });
  class History {
    get length() { return entries.length; }
    get state() { return clone(entries[position].state); }
    scrollRestoration = 'auto';
    pushState(state, _, url) { this._set(state, url, false); }
    replaceState(state, _, url) { this._set(state, url, true); }
    _set(state, url, replace) { const target = url === undefined || url === null ? new URL(current) : new URL(String(url), current); if (target.origin !== current.origin || target.protocol !== current.protocol || target.host !== current.host) throw fail('SecurityError'); const entry = { url: target.href, state: clone(state) }; current = target; if (replace) entries[position] = entry; else { entries.splice(++position); entries.push(entry); } entryChanged(); }
    go(delta = 0) { delta = Math.trunc(Number(delta) || 0); if (!delta) { navigate(current.href, true, true); return; } const next = position + delta; if (next < 0 || next >= entries.length) return; setTimeout(() => { const oldURL = current.href; position = next; current = new URL(entries[position].url); dispatchEvent(Object.assign(new Event('popstate'), { state: this.state })); changed(oldURL); entryChanged(); }, 0); }
    back() { this.go(-1); }
    forward() { this.go(1); }
  }
  class NavigationHistoryEntry extends EventTarget {
    #index; #entry;
    constructor(index, entry) { super(); this.#index = index; this.#entry = entry; entry.key ??= crypto.randomUUID(); }
    get key() { return this.#entry.key; }
    get id() { return this.#entry.key; }
    get url() { return this.#entry.url; }
    get index() { return this.#index; }
    get sameDocument() { return true; }
    getState() { return clone(this.#entry.state); }
  }
  class Navigation extends EventTarget {
    get canGoBack() { return position > 0; }
    get canGoForward() { return position < entries.length - 1; }
    get currentEntry() { return new NavigationHistoryEntry(position, entries[position]); }
    get transition() { return null; }
    entries() { return entries.map((entry, index) => new NavigationHistoryEntry(index, entry)); }
    #result() { const done = Promise.resolve(this.currentEntry); return { committed: done, finished: done }; }
    navigate(url, options = {}) { navigate(url, options.history === 'replace'); return this.#result(); }
    reload() { navigate(current.href, true, true); return this.#result(); }
    back() { history.back(); return this.#result(); }
    forward() { history.forward(); return this.#result(); }
    traverseTo(key) { const index = entries.findIndex(entry => entry.key === key); if (index >= 0) history.go(index - position); return this.#result(); }
    updateCurrentEntry({ state }) { entries[position].state = clone(state); entryChanged(); }
  }
  const navigation = new Navigation();
  function entryChanged() { navigation.dispatchEvent(new Event('currententrychange')); }
  Object.assign(globalThis, { Location, History, Navigation, NavigationHistoryEntry, location: new Location(), history: new History(), navigation });
  globalThis.__appsSetLocation = url => { current = new URL(String(url)); entries = [{ url: current.href, state: null }]; position = 0; };
};
