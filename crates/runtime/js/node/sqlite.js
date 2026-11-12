globalThis.__appsSQLite = native => {
  const token = {};
  const databases = new WeakMap(), authorizers = new Map();
  let authorizing = false;
  // SQLite state is borrowed while the authorizer runs, so native calls must not re-enter it.
  const call = (...args) => {
    if (authorizing) throw new Error('SQLite cannot be used inside an authorizer');
    try { return native(...args); } catch (error) {
      const [message, errcode, errstr] = String(error?.message).split('\x01');
      if (errstr !== undefined) Object.assign(error, { message, code: 'ERR_SQLITE_ERROR', errcode: Number(errcode), errstr });
      throw error;
    }
  };
  const fail = (Type, code, message) => Object.assign(new Type(message), { code });
  const open = database => { if (!database.isOpen) throw fail(Error, 'ERR_INVALID_STATE', 'database is not open'); };
  globalThis.__appsSqliteAuthorize = (id, action, args) => {
    const callback = authorizers.get(id);
    if (!callback) return 0;
    authorizing = true;
    try { return callback(action, ...args); } finally { authorizing = false; }
  };
  const resources = typeof __appsFinalizers === 'function' ? new __appsFinalizers(({ op, id, cursor }) => {
    if (cursor && !cursor.active) return;
    if (cursor) cursor.active = false;
    authorizers.delete(id);
    try { call(op, id); } catch {}
  }) : null;
  const boolean = (name, value) => {
    if (typeof value !== 'boolean') throw new TypeError(name + ' must be a boolean');
    return value;
  };
  const encode = (value, index) => {
    if (value === null) return [0, null];
    if (typeof value === 'bigint') {
      if (value < -9223372036854775808n || value > 9223372036854775807n) throw fail(TypeError, 'ERR_INVALID_ARG_VALUE', 'BigInt value is too large to bind.');
      return [1, String(value)];
    }
    if (typeof value === 'number') return [2, Number.isFinite(value) ? value : String(value)];
    if (typeof value === 'string') return [3, value];
    if (ArrayBuffer.isView(value) || value instanceof ArrayBuffer) return [4, value];
    if (typeof SharedArrayBuffer !== 'undefined' && value instanceof SharedArrayBuffer) return [4, new Uint8Array(value)];
    throw fail(TypeError, 'ERR_INVALID_ARG_TYPE', `Provided value cannot be bound to SQLite parameter ${index}.`);
  };
  const integer = (value, bigints) => {
    if (bigints) return BigInt(value);
    const number = Number(value);
    if (!Number.isSafeInteger(number)) throw fail(RangeError, 'ERR_OUT_OF_RANGE', `Value is too large to be represented as a JavaScript number: ${value}`);
    return number;
  };
  class DatabaseSync {
    #id;
    #path;
    #options;
    constructor(path, options = {}) {
      if (path instanceof URL && path.protocol === 'file:') path = decodeURIComponent(path.pathname);
      else if (ArrayBuffer.isView(path)) path = new TextDecoder().decode(path);
      if (typeof path !== 'string' || path.includes('\0')) throw fail(TypeError, 'ERR_INVALID_ARG_TYPE', 'The "path" argument must be a string, Uint8Array, or URL without null bytes.');
      if (!options || typeof options !== 'object' || Array.isArray(options)) throw new TypeError('Expected database options');
      this.#path = path;
      this.#options = {};
      for (const name of ['open', 'readOnly', 'enableForeignKeyConstraints', 'enableDoubleQuotedStringLiterals', 'allowExtension', 'defensive', 'readBigInts', 'returnArrays', 'allowBareNamedParameters', 'allowUnknownNamedParameters']) {
        if (options[name] !== undefined) this.#options[name] = boolean(name, options[name]);
      }
      if (options.allowExtension) throw new Error('SQLite extensions are not supported');
      if (options.limits !== undefined) throw new Error('Custom SQLite limits are not supported');
      if (options.timeout !== undefined && (!Number.isInteger(options.timeout) || options.timeout < 0 || options.timeout > 2147483647)) throw new RangeError('Invalid SQLite timeout');
      this.#options.timeout = options.timeout ?? 0;
      if (options.open !== false) this.open();
    }
    get isOpen() { return this.#id !== undefined; }
    get isTransaction() { return this.isOpen && call('sqlite.isTransaction', this.#id); }
    open() {
      if (this.isOpen) throw fail(Error, 'ERR_INVALID_STATE', 'database is already open');
      this.#id = call('sqlite.open', this.#path, this.#options);
      databases.set(this, this.#id);
      resources?.register(this, { op: 'sqlite.releaseDatabase', id: this.#id }, this);
    }
    close() {
      open(this);
      call('sqlite.close', this.#id);
      authorizers.delete(this.#id); databases.delete(this);
      this.#id = undefined;
      resources?.unregister(this);
    }
    exec(sql) {
      open(this);
      if (typeof sql !== 'string') throw fail(TypeError, 'ERR_INVALID_ARG_TYPE', 'The "sql" argument must be a string.');
      call('sqlite.exec', this.#id, sql);
    }
    prepare(sql) {
      open(this);
      if (typeof sql !== 'string') throw fail(TypeError, 'ERR_INVALID_ARG_TYPE', 'The "sql" argument must be a string.');
      return new StatementSync(token, this, call('sqlite.prepare', this.#id, sql), this.#options);
    }
    setAuthorizer(callback) {
      open(this);
      if (callback !== null && typeof callback !== 'function') throw new TypeError('Authorizer must be a function or null');
      call('sqlite.authorizer', this.#id, callback !== null);
      if (callback === null) authorizers.delete(this.#id); else authorizers.set(this.#id, callback);
    }
  }
  class StatementSync {
    #id;
    #database;
    #names = new Map();
    #bareNames = new Map();
    #anonymous = [];
    #ambiguous;
    #source;
    #cursor;
    #bigints;
    #arrays;
    #bare;
    #unknown;
    constructor(key, database, statement, options) {
      if (key !== token) throw new TypeError('Use database.prepare() to create a statement');
      this.#id = statement.id;
      this.#database = database;
      for (let index = 0; index < statement.parameters.length; index++) {
        const name = statement.parameters[index];
        if (name === null || name.startsWith('?')) this.#anonymous.push(index + 1);
        if (name === null) continue;
        this.#names.set(name, index + 1);
        const bare = name.slice(1);
        if (this.#bareNames.has(bare)) this.#ambiguous = bare;
        this.#bareNames.set(bare, index + 1);
      }
      this.#source = statement.sourceSQL;
      this.#bigints = options.readBigInts ?? false;
      this.#arrays = options.returnArrays ?? false;
      this.#bare = options.allowBareNamedParameters ?? true;
      this.#unknown = options.allowUnknownNamedParameters ?? false;
      resources?.register(this, { op: 'sqlite.releaseStatement', id: this.#id }, this);
    }
    #bind(args) {
      if (this.#id === undefined || !this.#database.isOpen) throw fail(Error, 'ERR_INVALID_STATE', 'statement has been finalized');
      if (this.#cursor?.active) {
        this.#cursor.active = false;
        this.#cursor.invalidated = true;
        call('sqlite.reset', this.#id);
      }
      const bindings = [];
      const first = args[0];
      if (first !== null && typeof first === 'object' && !ArrayBuffer.isView(first) && !(first instanceof ArrayBuffer) && !(typeof SharedArrayBuffer !== 'undefined' && first instanceof SharedArrayBuffer)) {
        if (Array.isArray(first)) throw new TypeError('Named parameters must be an object');
        if (this.#bare && this.#ambiguous !== undefined) throw new Error('Ambiguous named parameter: ' + this.#ambiguous);
        for (const name of Object.keys(args.shift())) {
          const index = this.#names.get(name) ?? (this.#bare ? this.#bareNames.get(name) : undefined);
          if (index === undefined) {
            if (this.#unknown) continue;
            throw fail(Error, 'ERR_INVALID_STATE', `Unknown named parameter '${name}'`);
          }
          bindings.push(index, ...encode(first[name], index));
        }
      }
      if (args.length > this.#anonymous.length) throw Object.assign(fail(Error, 'ERR_SQLITE_ERROR', 'column index out of range'), { errcode: 25, errstr: 'column index out of range' });
      for (let index = 0; index < args.length; index++) {
        bindings.push(this.#anonymous[index], ...encode(args[index], this.#anonymous[index]));
      }
      return bindings;
    }
    #rows(mode, bindings = null) {
      const result = call('sqlite.rows', this.#id, mode, this.#bigints, bindings !== null, ...(bindings ?? []));
      return result.rows.map(values => {
        const row = this.#arrays ? [] : Object.create(null);
        for (let index = 0; index < values.length; index++) {
          let value = values[index];
          if (value && typeof value === 'object') {
            if ('integer' in value) value = integer(value.integer, this.#bigints);
            else if ('blob' in value) value = value.blob;
            else value = value.real === 'inf' ? Infinity : value.real === '-inf' ? -Infinity : Number(value.real);
          }
          row[this.#arrays ? index : result.columns[index]] = value;
        }
        return row;
      });
    }
    all(...args) { return this.#rows('all', this.#bind(args)); }
    get(...args) { return this.#rows('get', this.#bind(args))[0]; }
    run(...args) {
      const [changes, lastInsertRowid] = call('sqlite.run', this.#id, ...this.#bind(args));
      return { changes: integer(changes, this.#bigints), lastInsertRowid: integer(lastInsertRowid, this.#bigints) };
    }
    iterate(...args) {
      let bindings = this.#bind(args);
      const cursor = this.#cursor = { active: true };
      const finish = () => {
        if (cursor.active) {
          cursor.active = false;
          call('sqlite.reset', this.#id);
        }
      };
      const iterator = {
        next: () => {
          if (cursor.invalidated) throw new Error('Iterator was invalidated');
          if (!cursor.active) return { done: true, value: undefined };
          try {
            const rows = this.#rows('next', bindings);
            bindings = null;
            if (rows.length) return { done: false, value: rows[0] };
            cursor.active = false;
            return { done: true, value: undefined };
          } catch (error) {
            try { finish(); } catch {}
            throw error;
          }
        },
        return: () => { finish(); return { done: true, value: undefined }; },
        [Symbol.iterator]() { return this; },
      };
      resources?.register(iterator, { op: 'sqlite.reset', id: this.#id, cursor });
      return iterator;
    }
    get sourceSQL() { return this.#source; }
    columns() { return call('sqlite.columns', this.#id); }
    get expandedSQL() { return call('sqlite.expandedSQL', this.#id); }
    setReadBigInts(enabled) { this.#bigints = boolean('readBigInts', enabled); }
    setReturnArrays(enabled) { this.#arrays = boolean('returnArrays', enabled); }
    setAllowBareNamedParameters(enabled) { this.#bare = boolean('allowBareNamedParameters', enabled); }
    setAllowUnknownNamedParameters(enabled) { this.#unknown = boolean('allowUnknownNamedParameters', enabled); }
    close() {
      if (this.#cursor) this.#cursor.active = false;
      if (this.#id === undefined) return;
      call('sqlite.releaseStatement', this.#id);
      this.#id = undefined;
      resources?.unregister(this);
    }
  }
  if (Symbol.dispose) {
    DatabaseSync.prototype[Symbol.dispose] = function () { if (this.isOpen) this.close(); };
    StatementSync.prototype[Symbol.dispose] = StatementSync.prototype.close;
  }
  const constants = { SQLITE_OK: 0, SQLITE_DENY: 1, SQLITE_IGNORE: 2 };
  'CREATE_INDEX CREATE_TABLE CREATE_TEMP_INDEX CREATE_TEMP_TABLE CREATE_TEMP_TRIGGER CREATE_TEMP_VIEW CREATE_TRIGGER CREATE_VIEW DELETE DROP_INDEX DROP_TABLE DROP_TEMP_INDEX DROP_TEMP_TABLE DROP_TEMP_TRIGGER DROP_TEMP_VIEW DROP_TRIGGER DROP_VIEW INSERT PRAGMA READ SELECT TRANSACTION UPDATE ATTACH DETACH ALTER_TABLE REINDEX ANALYZE CREATE_VTABLE DROP_VTABLE FUNCTION SAVEPOINT RECURSIVE'.split(' ').forEach((name, index) => { constants['SQLITE_' + name] = index + 1; });
  const backup = async (database, path, options = {}) => {
    const source = databases.get(database);
    if (source === undefined) throw new TypeError('Expected an open database');
    const pages = options.rate ?? 100;
    if (!Number.isInteger(pages) || pages <= 0) throw new RangeError('Backup rate must be positive');
    const id = call('sqlite.backup.start', source, String(path), options.source ?? 'main', options.target ?? 'main');
    const deadline = performance.now() + 30000;
    try {
      while (true) {
        const state = call('sqlite.backup.step', id, pages);
        options.progress?.({ totalPages: state.totalPages, remainingPages: state.remainingPages });
        if (state.done) return state.totalPages;
        if (performance.now() > deadline) throw new Error('SQLite backup timed out');
        await new Promise(resolve => setTimeout(resolve, 4));
      }
    } finally { call('sqlite.backup.finish', id); }
  };
  return Object.freeze({ DatabaseSync, StatementSync, Database: DatabaseSync, Statement: StatementSync, backup, constants: Object.freeze(constants) });
};
