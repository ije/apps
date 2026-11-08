globalThis.__appsFFI = (native, codec) => {
  const send = (request, storage = null, keep = null) => {
    const result = native(request, storage, keep);
    if (typeof result !== 'string') return result;
    const response = JSON.parse(result);
    if (response.error) throw new Error(response.error);
    return response.value;
  };
  const bridge = (op, args = [], storage, keep) => send(JSON.stringify({ op, args }), storage, keep);
  const info = bridge('info');
  const little = info.littleEndian;
  const maxPointer = (1n << BigInt(info.pointerSize * 8)) - 1n;
  const encoder = new TextEncoder();
  const names = ['void', 'int8', 'uint8', 'int16', 'uint16', 'int32', 'uint32', 'int64', 'uint64', 'float32', 'float64', 'pointer'];
  const methods = ['', 'Int8', 'Uint8', 'Int16', 'Uint16', 'Int32', 'Uint32', 'BigInt64', 'BigUint64', 'Float32', 'Float64', info.pointerSize === 8 ? 'BigUint64' : 'Uint32'];
  const getters = methods.map(name => 'get' + name), setters = methods.map(name => 'set' + name);
  const sizes = [0, 1, 1, 2, 2, 4, 4, 8, 8, 4, 8, info.pointerSize];
  const aliases = { i8: 'int8', u8: 'uint8', bool: 'uint8', i16: 'int16', u16: 'uint16', i32: 'int32', u32: 'uint32', i64: 'int64', u64: 'uint64', f32: 'float32', float: 'float32', f64: 'float64', double: 'float64', ptr: 'pointer', str: 'string', char: info.signedChar ? 'int8' : 'uint8' };
  const pointerNames = new Set(['pointer', 'string', 'buffer', 'arraybuffer', 'function']);
  const type = name => {
    if (typeof name !== 'string') throw new TypeError('FFI type must be a string');
    name = aliases[name] ?? name;
    const kind = pointerNames.has(name) ? 11 : names.indexOf(name);
    if (kind < 0) throw new TypeError('Unsupported FFI type: ' + name);
    return { name, kind };
  };
  const signature = (value = {}) => {
    if (!value || typeof value !== 'object' || Array.isArray(value)) throw new TypeError('Expected an FFI signature');
    const args = value.arguments ?? [];
    if (!Array.isArray(args)) throw new TypeError('FFI arguments must be an array');
    const parameters = args.map(type);
    if (parameters.some(item => item.kind === 0)) throw new TypeError('FFI arguments cannot be void');
    const result = type(value.return ?? 'void');
    return { parameters, result, key: JSON.stringify([result.name, parameters.map(item => item.name)]), native: { arguments: parameters.map(item => item.kind), return: result.kind } };
  };
  const pointer = value => {
    if (typeof value !== 'bigint') throw new TypeError('Pointer must be a bigint');
    if (value < 0n || value > maxPointer) throw new RangeError('Pointer is out of range');
    return value;
  };
  const length = value => {
    if (!Number.isSafeInteger(value) || value < 0) throw new RangeError('Expected a nonnegative byte length or offset');
    return value;
  };
  const offsetPointer = (address, offset) => {
    address = pointer(address);
    if (address === 0n) throw new RangeError('Cannot access a null pointer');
    return pointer(address + BigInt(length(offset)));
  };
  const bytes = source => {
    if (ArrayBuffer.isView(source)) return new Uint8Array(source.buffer, source.byteOffset, source.byteLength);
    if (source instanceof ArrayBuffer || (typeof SharedArrayBuffer !== 'undefined' && source instanceof SharedArrayBuffer)) return new Uint8Array(source);
    throw new TypeError('Expected an ArrayBuffer or typed array');
  };
  const getRawPointer = source => BigInt(bridge('pointer', [], bytes(source)));
  const write = (view, offset, kind, value, keep) => {
    if (kind === 0) return;
    if (kind === 11) {
      if (value == null) value = 0n;
      else if (typeof value !== 'bigint') {
        const storage = typeof value === 'string' ? encoder.encode(value + '\0') : bytes(value);
        keep.push(storage);
        value = getRawPointer(storage);
      }
      value = pointer(value);
      if (info.pointerSize === 4) value = Number(value);
    } else if (kind === 7 || kind === 8) {
      if (typeof value === 'number') {
        if (!Number.isSafeInteger(value)) throw new RangeError('FFI integer must be a safe integer or bigint');
        value = BigInt(value);
      }
      if (typeof value !== 'bigint') throw new TypeError('Expected an integer or bigint');
      const min = kind === 7 ? -(1n << 63n) : 0n;
      const max = kind === 7 ? (1n << 63n) - 1n : (1n << 64n) - 1n;
      if (value < min || value > max) throw new RangeError('FFI integer is out of range');
    } else {
      if (typeof value !== 'number') throw new TypeError('FFI numeric arguments must be numbers');
      if (kind < 7) {
        const bits = sizes[kind] * 8;
        const signed = kind % 2 === 1;
        const min = signed ? -(2 ** (bits - 1)) : 0;
        const max = 2 ** (signed ? bits - 1 : bits) - 1;
        if (!Number.isInteger(value) || value < min || value > max) throw new RangeError('FFI integer is out of range');
      }
    }
    view[setters[kind]](offset, value, little);
  };
  const read = (view, offset, kind) => {
    if (kind === 0) return undefined;
    const value = view[getters[kind]](offset, little);
    return kind === 11 ? BigInt(value) : value;
  };
  const resources = typeof __appsFinalizers === 'function' ? new __appsFinalizers(id => { try { bridge('close', [id]); } catch {} }) : null;
  const callbackOwners = new Set();
  class DynamicLibrary {
    #id;
    #functions = Object.create(null);
    #symbols = Object.create(null);
    #signatures = new Map();
    #callbacks = new Map();
    constructor(path) {
      if (path !== null && (typeof path !== 'string' || !path || path.includes('\0'))) throw new TypeError('Library path must be a string or null');
      this.#id = bridge('open', [path]);
      Object.defineProperty(this, 'path', { value: path, enumerable: true });
      resources?.register(this, this.#id, this);
    }
    #open() { if (this.#id === undefined) throw new Error('Library is closed'); return this.#id; }
    get functions() { return this.#functions; }
    get symbols() { return this.#symbols; }
    close() {
      if (this.#id === undefined) return;
      bridge('close', [this.#id]);
      this.#id = undefined;
      this.#callbacks.clear();
      callbackOwners.delete(this);
      resources?.unregister(this);
    }
    getFunction(name, definition = {}) {
      this.#open();
      if (typeof name !== 'string' || !name || name.includes('\0')) throw new TypeError('Expected a symbol name');
      const sig = signature(definition);
      const cached = this.#signatures.get(name);
      if (cached) {
        if (cached.key !== sig.key) throw new Error('Symbol already has a different signature: ' + name);
        return cached.function;
      }
      const resolved = bridge('function', [this.#id, name, sig.native]);
      const request = JSON.stringify({ op: 'invoke', args: [resolved.id] });
      const storage = new Uint8Array((sig.parameters.length + 1) * 8);
      const view = new DataView(storage.buffer);
      const fn = (...args) => {
        this.#open();
        if (args.length !== sig.parameters.length) throw new TypeError('Incorrect number of FFI arguments');
        const keep = [storage, this];
        for (let index = 0; index < args.length; index++) write(view, (index + 1) * 8, sig.parameters[index].kind, args[index], keep);
        // WARNING: keep must stay a native call argument so string and buffer arguments outlive the call.
        send(request, storage, keep);
        return read(view, 0, sig.result.kind);
      };
      Object.defineProperty(fn, 'pointer', { value: BigInt(resolved.pointer), enumerable: true });
      this.#functions[name] = fn;
      this.#symbols[name] = fn.pointer;
      this.#signatures.set(name, { key: sig.key, function: fn });
      return fn;
    }
    getFunctions(definitions) {
      this.#open();
      if (definitions !== undefined) {
        if (!definitions || typeof definitions !== 'object' || Array.isArray(definitions)) throw new TypeError('Expected function definitions');
        for (const [name, definition] of Object.entries(definitions)) this.getFunction(name, definition);
      }
      return this.#functions;
    }
    getSymbol(name) {
      this.#open();
      if (typeof name !== 'string' || !name || name.includes('\0')) throw new TypeError('Expected a symbol name');
      return this.#symbols[name] ??= BigInt(bridge('symbol', [this.#id, name]));
    }
    getSymbols() { this.#open(); return this.#symbols; }
    registerCallback(definition, fn) {
      this.#open();
      if (typeof definition === 'function' && fn === undefined) { fn = definition; definition = {}; }
      if (typeof fn !== 'function') throw new TypeError('Expected a callback function');
      const sig = signature(definition);
      callbackOwners.add(this);
      const record = { strong: fn, weak: new WeakRef(fn), retained: [] };
      const dispatch = buffer => {
        const fn = record.strong ?? record.weak.deref();
        if (!fn) return;
        const view = new DataView(buffer);
        const result = fn(...sig.parameters.map((item, index) => read(view, (index + 1) * 8, item.kind)));
        if (result && typeof result.then === 'function') throw new TypeError('FFI callbacks cannot return promises');
        const keep = [];
        write(view, 0, sig.result.kind, result, keep);
        record.retained = keep;
      };
      const address = BigInt(bridge('callback', [this.#id, sig.native], dispatch));
      this.#callbacks.set(address, record);
      return address;
    }
    unregisterCallback(address) {
      this.#open();
      pointer(address);
      if (!this.#callbacks.has(address)) throw new Error('Unknown callback');
      bridge('unregister', [this.#id, String(address)]);
      this.#callbacks.delete(address);
    }
    refCallback(address) {
      this.#open();
      const record = this.#callbacks.get(pointer(address));
      if (!record) throw new Error('Unknown callback');
      const fn = record.strong ?? record.weak.deref();
      if (!fn) throw new Error('Callback has been garbage collected');
      record.strong = fn;
    }
    unrefCallback(address) {
      this.#open();
      const record = this.#callbacks.get(pointer(address));
      if (!record || !record.weak.deref()) throw new Error('Unknown or collected callback');
      record.strong = undefined;
    }
  }
  if (Symbol.dispose) DynamicLibrary.prototype[Symbol.dispose] = DynamicLibrary.prototype.close;
  const toArrayBuffer = (address, size, copy = true) => {
    if (typeof copy !== 'boolean') throw new TypeError('copy must be a boolean');
    return bridge('view', [String(pointer(address)), length(size), copy]);
  };
  class NativeBuffer extends Uint8Array {
    toString(encoding = 'utf8') { return new TextDecoder(encoding).decode(this); }
  }
  const exportBytes = (source, address, size) => { bridge('copy', [String(pointer(address)), length(size)], bytes(source)); };
  const api = {
    DynamicLibrary, suffix: info.suffix,
    types: Object.freeze({ VOID: 'void', POINTER: 'pointer', BUFFER: 'buffer', ARRAY_BUFFER: 'arraybuffer', FUNCTION: 'function', BOOL: 'bool', CHAR: 'char', STRING: 'string', FLOAT: 'float', DOUBLE: 'double', INT_8: 'int8', UINT_8: 'uint8', INT_16: 'int16', UINT_16: 'uint16', INT_32: 'int32', UINT_32: 'uint32', INT_64: 'int64', UINT_64: 'uint64', FLOAT_32: 'float32', FLOAT_64: 'float64' }),
    dlopen(path, definitions) {
      const lib = new DynamicLibrary(path);
      try {
        const result = { lib, functions: lib.getFunctions(definitions) };
        if (Symbol.dispose) result[Symbol.dispose] = () => lib.close();
        return result;
      } catch (error) { lib.close(); throw error; }
    },
    dlclose: lib => lib.close(),
    dlsym: (lib, name) => lib.getSymbol(name),
    getRawPointer, toArrayBuffer,
    toBuffer: (address, size, copy = true) => new NativeBuffer(toArrayBuffer(address, size, copy)),
    toString: address => bridge('string', [String(pointer(address))]),
    exportString(value, address, size, encoding = 'utf8') {
      if (typeof value !== 'string' || typeof encoding !== 'string') throw new TypeError('Expected a string and encoding');
      let storage;
      switch (encoding.toLowerCase()) {
        case 'utf8': case 'utf-8': storage = encoder.encode(value + '\0'); break;
        case 'utf16le': case 'utf-16le': case 'ucs2': case 'ucs-2': storage = codec.encode(value + '\0', 2); break;
        case 'latin1': case 'binary': case 'ascii': storage = codec.encode(value + '\0', 1); break;
        default: throw new TypeError('Unsupported string encoding: ' + encoding);
      }
      exportBytes(storage, address, size);
    },
    exportBuffer(source, address, size) {
      if (!(source instanceof Uint8Array)) throw new TypeError('Expected a Uint8Array');
      exportBytes(source, address, size);
    },
    exportArrayBuffer(source, address, size) {
      if (!(source instanceof ArrayBuffer)) throw new TypeError('Expected an ArrayBuffer');
      exportBytes(source, address, size);
    },
    exportArrayBufferView(source, address, size) {
      if (!ArrayBuffer.isView(source)) throw new TypeError('Expected an ArrayBuffer view');
      exportBytes(source, address, size);
    },
    getCurrentEventLoop() { throw new Error('apps does not use a libuv event loop'); },
  };
  for (let kind = 1; kind <= 10; kind++) {
    const name = methods[kind].replace('Big', '');
    api['get' + name] = (address, offset = 0) => read(new DataView(toArrayBuffer(offsetPointer(address, offset), sizes[kind], false)), 0, kind);
    api['set' + name] = (address, offset, value) => {
      const storage = new Uint8Array(sizes[kind]);
      write(new DataView(storage.buffer), 0, kind, value, []);
      exportBytes(storage, offsetPointer(address, offset), storage.length);
    };
  }
  return Object.freeze(api);
};
