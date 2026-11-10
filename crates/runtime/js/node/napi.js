({
  object: () => ({}),
  holder: (() => { const held = new WeakMap(); return (object, holder) => { const values = held.get(object); if (values) values.push(holder); else held.set(object, [holder]); }; })(),
  wrap: (() => { const ids = new WeakMap(); return (object, id) => { const previous = ids.get(object) || 0; if (id !== undefined) { if (id) ids.set(object, id); else ids.delete(object); } return previous; }; })(),
  external: (() => { const ids = new WeakMap(); return (object, id) => { if (id !== undefined) ids.set(object, id); return ids.get(object) || 0; }; })(),
  array: length => new Array(length),
  function: (native, name) => {
    const fn = function(...args) { return native(this, new.target, ...args); };
    Object.defineProperty(fn, 'name', { value: name, configurable: true });
    return fn;
  },
  get: (object, key) => object[key],
  set: (object, key, value) => { object[key] = value; },
  has: (object, key) => key in object,
  own: (object, key) => Object.hasOwn(object, key),
  delete: (object, key) => delete object[key],
  keys: object => { const keys = []; for (const key in object) keys.push(key); return keys; },
  // Like V8's key collector, a filtered own key still hides the same key on prototypes.
  all_keys: (object, own, filter, strings) => {
    const keys = [], seen = new Set();
    for (let target = object; target !== null; target = own ? null : Object.getPrototypeOf(target)) {
      for (const key of Reflect.ownKeys(target)) {
        if (seen.has(key)) continue;
        seen.add(key);
        const descriptor = Object.getOwnPropertyDescriptor(target, key);
        if (filter & (typeof key === 'symbol' ? 16 : 8) || filter & 1 && descriptor.writable === false || filter & 2 && !descriptor.enumerable || filter & 4 && !descriptor.configurable) continue;
        keys.push(!strings && typeof key === 'string' && /^(?:0|[1-9]\d*)$/.test(key) && +key < 4294967295 ? +key : key);
      }
    }
    return keys;
  },
  prototype: object => Object.getPrototypeOf(object),
  define: (object, key, value, get, set, attributes) => Object.defineProperty(object, key,
    get || set ? {get, set, enumerable: !!(attributes & 2), configurable: !!(attributes & 4)} :
      {value, writable: !!(attributes & 1), enumerable: !!(attributes & 2), configurable: !!(attributes & 4)}),
  call: (fn, receiver, ...args) => Reflect.apply(fn, receiver, args),
  construct: (constructor, ...args) => Reflect.construct(constructor, args),
  instanceof: (object, constructor) => object instanceof constructor,
  equal: (a, b) => a === b,
  is_array: Array.isArray,
  is_error: value => value instanceof Error,
  is_promise: value => value instanceof Promise,
  is_dataview: value => value instanceof DataView,
  boolean: value => !!value,
  number: value => +value,
  string: value => String(value),
  to_object: value => { if (value == null) throw TypeError('Cannot convert null or undefined to object'); return Object(value); },
  error: (kind, code, message) => { const error = new ({Error, TypeError, RangeError})[kind](message); if (code !== undefined) error.code = code; return error; },
  symbol: description => Symbol(description),
  bigint: value => BigInt(value),
  bigint_hex: (hex, negative) => negative ? -BigInt(hex) : BigInt(hex),
  bigint_i64: value => BigInt.asIntN(64, value).toString(),
  bigint_u64: value => BigInt.asUintN(64, value).toString(),
  bigint_words: value => { const negative = value < 0n, words = [negative ? '1' : '0']; for (let rest = negative ? -value : value; rest > 0n; rest >>= 64n) words.push((rest & 0xffffffffffffffffn).toString()); return words.join(','); },
  type_tag: (() => { const tags = new WeakMap(); return (object, tag) => { if (tag !== undefined) tags.set(object, tag); return tags.get(object); }; })(),
  report: error => { if (globalThis.process?.listenerCount?.('uncaughtException')) process.emit('uncaughtException', error, 'uncaughtException'); else console.error(error); },
  bigint_lossless: (value, signed) => value === (signed ? BigInt.asIntN(64, value) : BigInt.asUintN(64, value)),
  detach: buffer => { try { buffer.transfer(); return true; } catch { return false; } },
  weak: value => new WeakRef(value),
  deref: ref => ref.deref(),
  freeze: Object.freeze,
  seal: Object.seal,
  buffer: buffer => globalThis.Buffer ? Buffer.from(buffer) : new Uint8Array(buffer),
  typed: (type, buffer, offset, length) => new ([Int8Array, Uint8Array, Uint8ClampedArray, Int16Array, Uint16Array, Int32Array, Uint32Array, Float32Array, Float64Array, BigInt64Array, BigUint64Array][type])(buffer, offset, length),
  dataview: (buffer, offset, length) => new DataView(buffer, offset, length),
  date: value => new Date(value),
  is_date: value => value instanceof Date,
  date_value: value => Date.prototype.getTime.call(value),
  promise: () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return {promise, resolve, reject}; },
})
