globalThis.__appsNode = (call, EventEmitter, { inspect, format, formatWithOptions, deepEqual }, info) => {
  const windows = info.platform === 'win32';
  const bytes = text => { if(typeof text !== 'string')return new Buffer(text.buffer, text.byteOffset, text.byteLength); const length = text.length, out = new Buffer(length); for (let i = 0; i < length; i++) out[i] = text.charCodeAt(i); return out; };
  const binary = data => { let state = ''; for (let i = 0; i < data.length; i += 8192) state += String.fromCharCode.apply(null, data.subarray(i, i + 8192)); return state; };
  const view = (value, encoding) => typeof value === 'string' ? Buffer.from(value, encoding) : ArrayBuffer.isView(value) ? new Uint8Array(value.buffer, value.byteOffset, value.byteLength) : Buffer.from(value);
  const utf8 = /^utf-?8$/i, utf8Encoder = new TextEncoder(), utf8Decoder = new TextDecoder('utf-8', { ignoreBOM: true });
  const error = (Type, code, message) => Object.assign(new Type(message), { code });
  const received = value => { if (value == null) return String(value); if (typeof value === 'function') return `function ${value.name}`; if (typeof value === 'object') return value.constructor?.name ? `an instance of ${value.constructor.name}` : inspect(value, { depth: -1 }); if (typeof value !== 'string') return `type ${typeof value} (${inspect(value)})`; if (value.length > 28) value = value.slice(0, 25) + '...'; return `type string (${value.includes("'") ? JSON.stringify(value) : `'${value}'`})`; };
  const typeError = (name, types, value) => error(TypeError, 'ERR_INVALID_ARG_TYPE', `The ${name.endsWith(' argument') ? name : `"${name}" ${name.includes('.') ? 'property' : 'argument'}`} must be ${types}. Received ${received(value)}`);
  const separated = value => String(value).replace(/\B(?=(\d{3})+$)/g, '_');
  const rangeError = (name, range, value) => error(RangeError, 'ERR_OUT_OF_RANGE', `The value of "${name}" is out of range. It must be ${range}. Received ${typeof value === 'bigint' ? (value > 2n ** 32n || value < -(2n ** 32n) ? separated(value) : value) + 'n' : Number.isInteger(value) && Math.abs(value) > 2 ** 32 ? separated(value) : inspect(value)}`);
  const integer = (value, name, min = 0, max = kMaxLength) => { if (typeof value !== 'number') throw typeError(name, 'of type number', value); if (!Number.isInteger(value)) throw rangeError(name, 'an integer', value); if (value < min || value > max) throw rangeError(name, `>= ${min} && <= ${max}`, value); return value; };
  const kMaxLength = 2 ** 32 - 1, bufferTypes = 'an instance of Buffer or Uint8Array';
  const encodings = { __proto__: null, utf8: 'utf8', 'utf-8': 'utf8', ucs2: 'utf16le', 'ucs-2': 'utf16le', utf16le: 'utf16le', 'utf-16le': 'utf16le', latin1: 'latin1', binary: 'latin1', base64: 'base64', base64url: 'base64url', hex: 'hex', ascii: 'ascii' };
  const normalize = encoding => encoding == null ? 'utf8' : encodings[encoding] ?? encodings[String(encoding).toLowerCase()];
  const encodingOf = encoding => { const value = encoding === null ? undefined : normalize(encoding); if (!value) throw error(TypeError, 'ERR_UNKNOWN_ENCODING', `Unknown encoding: ${encoding}`); return value; };
  const encode = (value, encoding = 'utf8') => {
    if (encoding === 'utf8') return utf8Encoder.encode(value);
    if (encoding === 'hex') return Uint8Array.fromHex(value.match(/^(?:[\da-f]{2})*/i)[0]);
    if (encoding === 'base64' || encoding === 'base64url') { const pad = value.indexOf('='); let text = (pad < 0 ? value : value.slice(0, pad)).replace(/[^\w+/-]|_/g, c => c === '_' ? '/' : '').replace(/-/g, '+'); if (text.length % 4 === 1) text = text.slice(0, -1); return Uint8Array.fromBase64(text); }
    if (encoding === 'utf16le') { const out = new Uint8Array(value.length * 2); for (let i = 0; i < value.length; i++) { const c = value.charCodeAt(i); out[i * 2] = c; out[i * 2 + 1] = c >>> 8; } return out; }
    return bytes(value);
  };
  const decode = (data, encoding) => {
    if (encoding === 'utf8') return utf8Decoder.decode(data);
    if (encoding === 'hex') return data.toHex();
    if (encoding === 'base64' || encoding === 'base64url') return data.toBase64({ alphabet: encoding, omitPadding: encoding === 'base64url' });
    if (encoding === 'latin1') return binary(data);
    if (encoding === 'ascii') return binary(data.map(byte => byte & 127));
    const length = data.length >> 1;
    return binary(data.byteOffset & 1 ? new Uint16Array(new Uint8Array(data.subarray(0, length * 2)).buffer) : new Uint16Array(data.buffer, data.byteOffset, length));
  };
  const bounds = (value, length, type) => { if (Math.floor(value) !== value) { if (typeof value !== 'number') throw typeError(type || 'offset', 'of type number', value); throw rangeError(type || 'offset', 'an integer', value); } if (length < 0) throw error(RangeError, 'ERR_BUFFER_OUT_OF_BOUNDS', 'Attempt to access memory outside buffer bounds'); throw rangeError(type || 'offset', `>= ${type ? 1 : 0} and <= ${length}`, value); };
  const at = (buffer, offset, size) => { if (typeof offset !== 'number') throw typeError('offset', 'of type number', offset); if (buffer[offset] === undefined || buffer[offset + size - 1] === undefined) bounds(offset, buffer.length - size); return offset; };
  const width = size => size >= 1 && size <= 6 && Number.isInteger(size) ? size : bounds(size, 6, 'byteLength');
  const limit = (value, min, max, size) => { if (value > max || value < min) { const n = typeof min === 'bigint' ? 'n' : ''; throw rangeError('value', size <= 4 ? `>= ${min}${n} and <= ${max}${n}` : min == 0 ? `>= 0${n} and < 2${n} ** ${size * 8}${n}` : `>= -(2${n} ** ${size * 8 - 1}${n}) and < 2${n} ** ${size * 8 - 1}${n}`, value); } };
  const offsetOf = (value, name, max) => { if (value === undefined) return; value = Number.isInteger(value) ? value : +value >= Number.MIN_SAFE_INTEGER && +value <= Number.MAX_SAFE_INTEGER ? Math.floor(+value) : 0; if (value < 0 || value > max) throw rangeError(name, max === Infinity ? '>= 0' : `>= 0 && <= ${max}`, value); return value; };
  const search = (buffer, value, offset, encoding, last) => {
    if (typeof offset === 'string') { encoding = offset; offset = undefined; }
    offset = +Math.min(Math.max(offset, -0x80000000), 0x7fffffff);
    if (Number.isNaN(offset)) offset = last ? buffer.length : 0;
    if (offset < 0) offset += buffer.length;
    if (typeof value === 'number') return last ? (offset < 0 ? -1 : Uint8Array.prototype.lastIndexOf.call(buffer, value >>> 0 & 255, offset)) : Uint8Array.prototype.indexOf.call(buffer, value >>> 0 & 255, Math.max(offset, 0));
    let needle;
    if (typeof value === 'string') needle = encode(value, encodingOf(encoding));
    else if (value instanceof Uint8Array) needle = value;
    else throw typeError('value', 'one of type number or string or an instance of Buffer or Uint8Array', value);
    if (!needle.length) return Math.min(Math.max(offset, 0), buffer.length);
    if (last) { for (let i = Math.min(offset, buffer.length - needle.length); i >= 0; i--) { let j = 0; while (j < needle.length && buffer[i + j] === needle[j]) j++; if (j === needle.length) return i; } return -1; }
    for (let i = Uint8Array.prototype.indexOf.call(buffer, needle[0], Math.max(offset, 0)); i >= 0 && i <= buffer.length - needle.length; i = Uint8Array.prototype.indexOf.call(buffer, needle[0], i + 1)) { let j = 1; while (j < needle.length && buffer[i + j] === needle[j]) j++; if (j === needle.length) return i; }
    return -1;
  };
  const size = value => { if (typeof value !== 'number') throw typeError('size', 'of type number', value); if (!(value >= 0 && value <= kMaxLength)) throw rangeError('size', `>= 0 && <= ${kMaxLength}`, value); return value; };
  const compare = (a, b) => { const length = Math.min(a.length, b.length); for (let i = 0; i < length; i++) if (a[i] !== b[i]) return a[i] < b[i] ? -1 : 1; return Math.sign(a.length - b.length); };
  class Buffer extends Uint8Array {
    static from(value, encoding, length) {
      if (typeof value === 'string') { const data = encode(value, typeof encoding === 'string' && encoding ? encodingOf(encoding) : 'utf8'); return new Buffer(data.buffer, data.byteOffset, data.length); }
      if (typeof value === 'object' && value !== null) {
        if (value instanceof ArrayBuffer || (typeof SharedArrayBuffer !== 'undefined' && value instanceof SharedArrayBuffer)) {
          const offset = +encoding || 0, max = value.byteLength - offset;
          if (max < 0) throw error(RangeError, 'ERR_BUFFER_OUT_OF_BOUNDS', '"offset" is outside of buffer bounds');
          if (length === undefined) length = max; else if ((length = +length) > 0) { if (length > max) throw error(RangeError, 'ERR_BUFFER_OUT_OF_BOUNDS', '"length" is outside of buffer bounds'); } else length = 0;
          return new Buffer(value, offset, length);
        }
        const primitive = value.valueOf?.();
        if (primitive != null && primitive !== value && (typeof primitive === 'string' || typeof primitive === 'object')) return Buffer.from(primitive, encoding, length);
        if (value.length !== undefined || value.buffer instanceof ArrayBuffer) return typeof value.length === 'number' ? new Buffer(value) : new Buffer(0);
        if (value.type === 'Buffer' && Array.isArray(value.data)) return new Buffer(value.data);
        if (typeof value[Symbol.toPrimitive] === 'function') { const text = value[Symbol.toPrimitive]('string'); if (typeof text === 'string') return Buffer.from(text, encoding); }
      }
      throw typeError('first argument', 'of type string or an instance of Buffer, ArrayBuffer, or Array or an Array-like Object', value);
    }
    static alloc(length, fill, encoding) { const out = new Buffer(size(length)); return fill === undefined || fill === 0 || !out.length ? out : out.fill(fill, 0, out.length, encoding); }
    static allocUnsafe(length) { return new Buffer(size(length)); }
    static of(...items) { return new Buffer(items); }
    static copyBytesFrom(view, offset, length) {
      if (!ArrayBuffer.isView(view) || view instanceof DataView) throw typeError('view', 'an instance of TypedArray', view);
      if (offset !== undefined) integer(offset, 'offset', 0, Number.MAX_SAFE_INTEGER);
      if (length !== undefined) integer(length, 'length', 0, Number.MAX_SAFE_INTEGER);
      const part = offset === undefined && length === undefined ? view : Object.getPrototypeOf(Uint8Array.prototype).slice.call(view, offset, length === undefined ? undefined : (offset ?? 0) + length);
      return Buffer.from(new Uint8Array(part.buffer, part.byteOffset, part.byteLength));
    }
    static allocUnsafeSlow(length) { return new Buffer(size(length)); }
    static isBuffer(value) { return value instanceof Buffer; }
    static isEncoding(value) { return typeof value === 'string' && value.length !== 0 && normalize(value) !== undefined; }
    static byteLength(value, encoding) {
      if (typeof value !== 'string') { if (ArrayBuffer.isView(value) || value instanceof ArrayBuffer || (typeof SharedArrayBuffer !== 'undefined' && value instanceof SharedArrayBuffer)) return value.byteLength; throw typeError('string', 'of type string or an instance of Buffer or ArrayBuffer', value); }
      let length = value.length;
      encoding = encoding ? normalize(encoding) ?? 'utf8' : 'utf8';
      if (encoding === 'utf16le') return length * 2;
      if (encoding === 'hex') return length >>> 1;
      if (encoding === 'base64' || encoding === 'base64url') { if (value.charCodeAt(length - 1) === 61) length--; if (length > 1 && value.charCodeAt(length - 1) === 61) length--; return (length * 3) >>> 2; }
      if (encoding !== 'utf8') return length;
      for (let i = 0; i < value.length; i++) { const c = value.charCodeAt(i); if (c < 128) continue; length += c < 2048 ? 1 : 2; if (c >= 0xD800 && c < 0xDC00 && (value.charCodeAt(i + 1) & 0xFC00) === 0xDC00) i++; }
      return length;
    }
    static concat(list, length) {
      if (!Array.isArray(list)) throw typeError('list', 'an instance of Array', list);
      if (!list.length) return new Buffer(0);
      for (let i = 0; i < list.length; i++) if (!(list[i] instanceof Uint8Array)) throw typeError(`list[${i}]`, bufferTypes, list[i]);
      length = length === undefined ? list.reduce((n, v) => n + v.length, 0) : integer(length, 'length');
      const out = new Buffer(length);
      let offset = 0;
      for (const value of list) { if (offset >= length) break; const n = Math.min(value.length, length - offset); out.set(n === value.length ? value : value.subarray(0, n), offset); offset += n; }
      return out;
    }
    static compare(a, b) { if (!(a instanceof Uint8Array)) throw typeError('buf1', bufferTypes, a); if (!(b instanceof Uint8Array)) throw typeError('buf2', bufferTypes, b); return compare(a, b); }
    toString(encoding, start, end) {
      if (!arguments.length) return utf8Decoder.decode(this);
      const length = this.length;
      if (start <= 0) start = 0; else if (start >= length) return ''; else start = Math.trunc(start) || 0;
      if (end === undefined || end > length) end = length; else end = Math.trunc(end) || 0;
      return end <= start ? '' : decode(start || end < length ? this.subarray(start, end) : this, encodingOf(encoding));
    }
    toLocaleString(...args) { return this.toString(...args); }
    equals(other) { if (!(other instanceof Uint8Array)) throw typeError('otherBuffer', bufferTypes, other); return this === other || compare(this, other) === 0; }
    compare(target, targetStart, targetEnd, start, end) {
      if (!(target instanceof Uint8Array)) throw typeError('target', bufferTypes, target);
      targetStart = targetStart === undefined ? 0 : integer(targetStart, 'targetStart'); targetEnd = targetEnd === undefined ? target.length : integer(targetEnd, 'targetEnd', 0, target.length);
      start = start === undefined ? 0 : integer(start, 'sourceStart'); end = end === undefined ? this.length : integer(end, 'sourceEnd', 0, this.length);
      return start >= end ? (targetStart >= targetEnd ? 0 : -1) : targetStart >= targetEnd ? 1 : compare(this.subarray(start, end), target.subarray(targetStart, targetEnd));
    }
    slice(start, end) { return this.subarray(start, end); }
    copy(target, targetStart, start, end) {
      if (!(target instanceof Uint8Array)) throw typeError('target', bufferTypes, target);
      targetStart = offsetOf(targetStart, 'targetStart', Infinity) ?? 0; start = offsetOf(start, 'sourceStart', this.length) ?? 0; end = offsetOf(end, 'sourceEnd', Infinity) ?? this.length;
      if (targetStart >= target.length || start >= end) return 0;
      const data = this.subarray(start, Math.min(end, start + target.length - targetStart));
      target.set(data, targetStart);
      return data.length;
    }
    write(string, offset, length, encoding) {
      if (typeof string !== 'string') throw error(TypeError, 'ERR_INVALID_ARG_TYPE', 'argument must be a string');
      if (offset === undefined) { offset = 0; length = this.length; }
      else if (length === undefined && typeof offset === 'string') { encoding = offset; offset = 0; length = this.length; }
      else { integer(offset, 'offset', 0, this.length); const remaining = this.length - offset; if (length === undefined) length = remaining; else if (typeof length === 'string') { encoding = length; length = remaining; } else length = Math.min(integer(length, 'length', 0, this.length), remaining); }
      const target = this.subarray(offset, offset + length);
      encoding = encoding ? encodingOf(encoding) : 'utf8';
      if (encoding === 'utf8') return utf8Encoder.encodeInto(string, target).written;
      const data = encode(encoding === 'utf16le' ? string.slice(0, length >> 1) : string, encoding).subarray(0, length);
      target.set(data);
      return data.length;
    }
    fill(value, offset, end, encoding) {
      if (typeof value === 'string') {
        if (offset === undefined || typeof offset === 'string') { encoding = offset; offset = 0; end = this.length; } else if (typeof end === 'string') { encoding = end; end = this.length; }
        encoding = normalize(encoding) ?? (() => { throw typeof encoding === 'string' ? error(TypeError, 'ERR_UNKNOWN_ENCODING', `Unknown encoding: ${encoding}`) : typeError('encoding', 'of type string', encoding); })();
        if (!value.length) value = 0;
        else if (value.length === 1 && (encoding === 'latin1' || ((encoding === 'utf8' || encoding === 'ascii') && value.charCodeAt(0) < 128))) value = value.charCodeAt(0);
      }
      if (offset === undefined) { offset = 0; end = this.length; }
      else { integer(offset, 'offset'); end = end === undefined ? this.length : integer(end, 'end', 0, this.length); if (offset >= end) return this; }
      if (typeof value === 'number') return super.fill(value, offset, end);
      if (typeof value !== 'string' && !ArrayBuffer.isView(value)) return super.fill(Number(value) & 255, offset, end);
      const pattern = typeof value === 'string' ? encode(value, encoding) : view(value);
      if (!pattern.length) throw error(TypeError, 'ERR_INVALID_ARG_VALUE', `The argument 'value' is invalid. Received ${inspect(value)}`);
      if (pattern.length === 1) return super.fill(pattern[0], offset, end);
      this.set(pattern.subarray(0, Math.min(pattern.length, end - offset)), offset);
      for (let size = pattern.length; size < end - offset; size *= 2) this.copyWithin(offset + size, offset, offset + Math.min(size, end - offset - size));
      return this;
    }
    toJSON() { return { type: 'Buffer', data: Array.from(this) }; }
    indexOf(value, offset, encoding) { return search(this, value, offset, encoding, false); }
    lastIndexOf(value, offset, encoding) { return search(this, value, offset, encoding, true); }
    includes(value, offset, encoding) { return search(this, value, offset, encoding, false) !== -1; }
    [inspect.custom]() { const max = buffer.INSPECT_MAX_BYTES, more = this.length - max; return `<${this.constructor.name} ${this.subarray(0, max).toHex().replace(/(..)(?!$)/g, '$1 ')}${more > 0 ? ` ... ${more} more byte${more > 1 ? 's' : ''}` : ''}>`; }
  }
  for (const [suffix, type, size, min, max] of [['UInt8', 'Uint8', 1, 0, 255], ['Int8', 'Int8', 1, -128, 127], ['UInt16', 'Uint16', 2, 0, 65535], ['Int16', 'Int16', 2, -32768, 32767], ['UInt32', 'Uint32', 4, 0, 4294967295], ['Int32', 'Int32', 4, -2147483648, 2147483647], ['Float', 'Float32', 4], ['Double', 'Float64', 8], ['BigInt64', 'BigInt64', 8, -(2n ** 63n), 2n ** 63n - 1n], ['BigUInt64', 'BigUint64', 8, 0n, 2n ** 64n - 1n]]) for (const endian of size === 1 ? [''] : ['LE', 'BE']) {
    Buffer.prototype['read' + suffix + endian] = function(offset = 0) { return new DataView(this.buffer, this.byteOffset, this.byteLength)['get' + type](at(this, offset, size), endian === 'LE'); };
    Buffer.prototype['write' + suffix + endian] = function(value, offset = 0) { if (typeof min === 'number') value = +value; if (min !== undefined) limit(value, min, max, size); new DataView(this.buffer, this.byteOffset, this.byteLength)['set' + type](at(this, offset, size), value, endian === 'LE'); return offset + size; };
  }
  for (const endian of ['LE', 'BE']) {
    const index = (offset, length, i) => offset + (endian === 'LE' ? i : length - 1 - i);
    Buffer.prototype['readUInt' + endian] = function(offset, length) { if (offset === undefined) throw typeError('offset', 'of type number', offset); width(length); at(this, offset, length); let value = 0; for (let i = length - 1; i >= 0; i--) value = value * 256 + this[index(offset, length, i)]; return value; };
    Buffer.prototype['readInt' + endian] = function(offset, length) { const value = this['readUInt' + endian](offset, length); return value >= 2 ** (length * 8 - 1) ? value - 2 ** (length * 8) : value; };
    for (const signed of [false, true]) Buffer.prototype[(signed ? 'writeInt' : 'writeUInt') + endian] = function(value, offset, length) { const bits = width(length) * 8; value = +value; limit(value, signed ? -(2 ** (bits - 1)) : 0, signed ? 2 ** (bits - 1) - 1 : 2 ** bits - 1, length); at(this, offset, length); if (value < 0) value += 2 ** bits; for (let i = 0; i < length; i++) { this[index(offset, length, i)] = value % 256; value = Math.floor(value / 256); } return offset + length; };
  }
  for (const name of Object.getOwnPropertyNames(Buffer.prototype)) if (name.includes('UInt')) Buffer.prototype[name.replace('UInt', 'Uint')] = Buffer.prototype[name];
  for (const size of [2, 4, 8]) Buffer.prototype['swap' + size * 8] = function() { if (this.length % size) throw Object.assign(new RangeError(`Buffer size must be a multiple of ${size * 8}-bits`), { code: 'ERR_INVALID_BUFFER_SIZE' }); for (let i = 0; i < this.length; i += size) for (let a = i, b = i + size - 1; a < b; a++, b--) { const t = this[a]; this[a] = this[b]; this[b] = t; } return this; };
  Buffer.poolSize = 8192;
  for (const name of Object.getOwnPropertyNames(Buffer)) if (typeof Buffer[name] === 'function') Object.defineProperty(Buffer, name, { enumerable: true });
  globalThis.Buffer = Buffer;
  const transcodeError = code => Object.assign(new Error(`Unable to transcode Buffer [${code}]`), { code, errno: code === 'U_ILLEGAL_ARGUMENT_ERROR' ? 1 : 10 });
  const buffer = { Buffer, Blob, File, atob, btoa, kMaxLength, kStringMaxLength: 2 ** 30 - 25, constants: { MAX_LENGTH: kMaxLength, MAX_STRING_LENGTH: 2 ** 30 - 25 }, INSPECT_MAX_BYTES: 50,
    isUtf8: value => { try { new TextDecoder('utf-8', { fatal: true }).decode(value); return true; } catch { return false; } },
    isAscii: value => view(value).every(byte => byte < 128),
    transcode(source, from, to) {
      if (!(source instanceof Uint8Array)) throw typeError('source', bufferTypes, source);
      if (!source.length) return new Buffer(0);
      [from, to] = [normalize(from) ?? from, normalize(to) ?? to];
      if (![from, to].every(name => ['ascii', 'latin1', 'utf16le', 'utf8'].includes(name))) throw transcodeError('U_ILLEGAL_ARGUMENT_ERROR');
      let text;
      if (from === 'utf8') { try { text = new TextDecoder('utf-8', { fatal: to === 'utf16le', ignoreBOM: true }).decode(source); } catch { throw transcodeError('U_INVALID_CHAR_FOUND'); } }
      else text = from === 'ascii' ? binary(source).replace(/[\x80-\xff]/g, '�') : decode(source, from);
      if (to === 'utf8' && from === 'utf16le' && /[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/.test(text)) throw transcodeError('U_INVALID_CHAR_FOUND');
      if (to === 'ascii' || to === 'latin1') text = text.replace(to === 'ascii' ? /[\ud800-\udbff][\udc00-\udfff]|[^\x00-\x7f]/g : /[\ud800-\udbff][\udc00-\udfff]|[^\x00-\xff]/g, '?');
      return Buffer.from(text, to);
    } };

  const pending = new Map(), watchers = new Map();
  const sync = (op, ...args) => call('node.' + op, ...args);
  const asyncCall = (op, ...args) => new Promise((resolve, reject) => { const id = sync('async', op, args); pending.set(id, { resolve, reject }); });
  const constants = sync('constants');
  let system;
  const details = () => system ??= sync('os.info');

  const makePath = windows => {
    const sep = windows ? '\\' : '/', isSep = windows ? c => c === 47 || c === 92 : c => c === 47;
    const check = (value, name) => { if (typeof value !== 'string') throw typeError(name, 'of type string', value); };
    const drive = p => (p.charCodeAt(0) | 32) >= 97 && (p.charCodeAt(0) | 32) <= 122 && p.charCodeAt(1) === 58;
    const collapse = (path, above) => {
      let res = '', segment = 0, slash = -1, dots = 0, code = 0;
      for (let i = 0; i <= path.length; ++i) {
        if (i < path.length) code = path.charCodeAt(i);
        else if (isSep(code)) break;
        else code = 47;
        if (isSep(code)) {
          if (slash === i - 1 || dots === 1) {}
          else if (dots === 2) {
            if (res.length < 2 || segment !== 2 || !res.endsWith('..')) {
              if (res.length > 2) { const index = res.lastIndexOf(sep); res = index < 0 ? '' : res.slice(0, index); segment = index < 0 ? 0 : res.length - 1 - res.lastIndexOf(sep); slash = i; dots = 0; continue; }
              if (res.length) { res = ''; segment = 0; slash = i; dots = 0; continue; }
            }
            if (above) { res += res.length ? sep + '..' : '..'; segment = 2; }
          } else { res += (res.length ? sep : '') + path.slice(slash + 1, i); segment = i - slash - 1; }
          slash = i; dots = 0;
        } else if (code === 46 && dots !== -1) ++dots;
        else dots = -1;
      }
      return res;
    };
    const unc = p => {
      let j = 2;
      while (j < p.length && !isSep(p.charCodeAt(j))) j++;
      if (j >= p.length || j === 2) return;
      const first = p.slice(2, j);
      while (j < p.length && isSep(p.charCodeAt(j))) j++;
      if (j >= p.length) return;
      const start = j;
      while (j < p.length && !isSep(p.charCodeAt(j))) j++;
      return [first, start, j];
    };
    const reserved = (p, colon) => /^(?:CON|PRN|AUX|NUL|COM[1-9\xb9\xb2\xb3]|LPT[1-9\xb9\xb2\xb3])$/.test(p.slice(0, colon).toUpperCase());
    const head = (p, normal) => {
      if (isSep(p.charCodeAt(0))) {
        if (!windows || p.length < 2 || !isSep(p.charCodeAt(1))) return ['', 1, true];
        const [first, start, j] = unc(p) ?? [];
        if (first === undefined) return ['', 0, true];
        if (first !== '.' && first !== '?') return [`\\\\${first}\\${p.slice(start, j)}`, j, true];
        const device = p.slice(4, p.indexOf(':') + 1);
        return normal && reserved(device, device.length - 1) ? ['\\\\?\\' + device, 4 + device.length, true] : ['\\\\' + first, 4, true];
      }
      if (windows && drive(p)) return p.length > 2 && isSep(p.charCodeAt(2)) ? [p.slice(0, 2), 3, true] : [p.slice(0, 2), 2, false];
      const colon = windows && normal ? p.indexOf(':') : -1;
      return colon > 0 && reserved(p, colon) ? [p.slice(0, colon + 1), colon + 1, false] : ['', 0, false];
    };
    const rootLength = p => { if (isSep(p.charCodeAt(0))) { const u = windows && p.length > 1 && isSep(p.charCodeAt(1)) && unc(p); return u ? Math.min(u[2] + 1, p.length) : 1; } return windows && drive(p) ? (p.length > 2 && isSep(p.charCodeAt(2)) ? 3 : 2) : 0; };
    const scan = (p, start, part) => {
      let dot = -1, end = -1, matched = true, state = 0;
      for (let i = p.length - 1; i >= start; --i) {
        const code = p.charCodeAt(i);
        if (isSep(code)) { if (!matched) { part = i + 1; break; } continue; }
        if (end === -1) { matched = false; end = i + 1; }
        if (code === 46) { if (dot === -1) dot = i; else if (state !== 1) state = 1; } else if (dot !== -1) state = -1;
      }
      return [part, end, dot, dot === -1 || end === -1 || state === 0 || (state === 1 && dot === end - 1 && dot === part + 1)];
    };
    const path = {
      sep, delimiter: windows ? ';' : ':',
      normalize(p) {
        check(p, 'path');
        if (!windows) {
          if (!p.length) return '.';
          const absolute = p.charCodeAt(0) === 47, trailing = p.charCodeAt(p.length - 1) === 47;
          p = collapse(p, !absolute);
          if (!p.length) return absolute ? '/' : trailing ? './' : '.';
          return (absolute ? '/' : '') + p + (trailing ? '/' : '');
        }
        const len = p.length;
        if (len < 2) return len ? (p === '/' ? '\\' : p) : '.';
        const [device, rootEnd, absolute] = head(p, true);
        let rest = rootEnd < len ? collapse(p.slice(rootEnd), !absolute) : '';
        if (!rest.length && !absolute) rest = '.';
        if (rest.length && isSep(p.charCodeAt(len - 1))) rest += '\\';
        if (!absolute && !device && p.includes(':')) {
          if (rest.length >= 2 && drive(rest)) return '.\\' + rest;
          for (let index = p.indexOf(':'); index !== -1; index = p.indexOf(':', index + 1)) if (index === len - 1 || isSep(p.charCodeAt(index + 1))) return '.\\' + rest;
        }
        return (reserved(p, p.indexOf(':')) ? '.\\' : '') + device + (absolute ? '\\' : '') + rest;
      },
      isAbsolute(p) { check(p, 'path'); return p.length > 0 && (isSep(p.charCodeAt(0)) || (windows && p.length > 2 && drive(p) && isSep(p.charCodeAt(2)))); },
      join(...parts) {
        let joined, first;
        for (const part of parts) { check(part, 'path'); if (part.length) { if (joined === undefined) joined = first = part; else joined += sep + part; } }
        if (joined === undefined) return '.';
        if (windows) {
          let replace = true, slashes = 0;
          if (isSep(first.charCodeAt(0))) { ++slashes; if (first.length > 1 && isSep(first.charCodeAt(1))) { ++slashes; if (first.length > 2) { if (isSep(first.charCodeAt(2))) ++slashes; else replace = false; } } }
          if (replace) { while (slashes < joined.length && isSep(joined.charCodeAt(slashes))) slashes++; if (slashes >= 2) joined = '\\' + joined.slice(slashes); }
          if (joined.split('\\').some(part => { const colon = part.indexOf(':'); return colon !== -1 && reserved(part, colon); })) return joined.replaceAll('/', '\\');
        }
        return path.normalize(joined);
      },
      resolve(...parts) {
        if (!windows) {
          let resolved = '', absolute = false;
          for (let i = parts.length - 1; i >= -1 && !absolute; i--) { const p = i >= 0 ? parts[i] : call('cwd'); check(p, `paths[${i}]`); if (!p.length) continue; resolved = `${p}/${resolved}`; absolute = p.charCodeAt(0) === 47; }
          resolved = collapse(resolved, !absolute);
          return absolute ? '/' + resolved : resolved || '.';
        }
        let resolvedDevice = '', resolvedTail = '', resolvedAbsolute = false;
        for (let i = parts.length - 1; i >= -1; i--) {
          let p;
          if (i >= 0) { p = parts[i]; check(p, `paths[${i}]`); if (!p.length) continue; }
          else if (!resolvedDevice) p = call('cwd');
          else { p = globalThis.process?.env[`=${resolvedDevice}`] || call('cwd'); if (p.slice(0, 2).toLowerCase() !== resolvedDevice.toLowerCase() && p.charCodeAt(2) === 92) p = resolvedDevice + '\\'; }
          const [device, rootEnd, absolute] = head(p);
          if (device) { if (!resolvedDevice) resolvedDevice = device; else if (device.toLowerCase() !== resolvedDevice.toLowerCase()) continue; }
          if (resolvedAbsolute) { if (resolvedDevice) break; }
          else { resolvedTail = `${p.slice(rootEnd)}\\${resolvedTail}`; resolvedAbsolute = absolute; if (absolute && resolvedDevice) break; }
        }
        resolvedTail = collapse(resolvedTail, !resolvedAbsolute);
        return resolvedAbsolute ? `${resolvedDevice}\\${resolvedTail}` : `${resolvedDevice}${resolvedTail}` || '.';
      },
      relative(from, to) {
        check(from, 'from'); check(to, 'to');
        if (from === to) return '';
        const fromOrig = path.resolve(from), toOrig = path.resolve(to);
        if (fromOrig === toOrig) return '';
        from = windows ? fromOrig.toLowerCase() : fromOrig; to = windows ? toOrig.toLowerCase() : toOrig;
        if (from === to) return '';
        let fromStart = 0, fromEnd = from.length, toStart = 0, toEnd = to.length;
        if (windows) { while (fromStart < fromEnd && from.charCodeAt(fromStart) === 92) fromStart++; while (fromEnd - 1 > fromStart && from.charCodeAt(fromEnd - 1) === 92) fromEnd--; while (toStart < toEnd && to.charCodeAt(toStart) === 92) toStart++; while (toEnd - 1 > toStart && to.charCodeAt(toEnd - 1) === 92) toEnd--; }
        else { fromStart = toStart = 1; }
        const fromLen = fromEnd - fromStart, toLen = toEnd - toStart, length = Math.min(fromLen, toLen), code = sep.charCodeAt(0);
        let last = -1, i = 0;
        for (; i < length; i++) { const c = from.charCodeAt(fromStart + i); if (c !== to.charCodeAt(toStart + i)) break; if (c === code) last = i; }
        if (i !== length) { if (windows && last === -1) return toOrig; }
        else {
          if (toLen > length) { if (to.charCodeAt(toStart + i) === code) return toOrig.slice(toStart + i + 1); if (i === (windows ? 2 : 0)) return toOrig.slice(toStart + i); }
          if (fromLen > length) { if (from.charCodeAt(fromStart + i) === code) last = i; else if (i === (windows ? 2 : 0)) last = windows ? 3 : 0; }
          if (windows && last === -1) last = 0;
        }
        let out = '';
        for (i = fromStart + last + 1; i <= fromEnd; ++i) if (i === fromEnd || from.charCodeAt(i) === code) out += out.length ? sep + '..' : '..';
        toStart += last;
        if (!windows) return out + toOrig.slice(toStart);
        if (out.length) return out + toOrig.slice(toStart, toEnd);
        if (toOrig.charCodeAt(toStart) === 92) ++toStart;
        return toOrig.slice(toStart, toEnd);
      },
      toNamespacedPath(p) {
        if (!windows || typeof p !== 'string' || !p.length) return p;
        const resolved = path.resolve(p);
        if (resolved.length <= 2) return p;
        if (resolved.charCodeAt(0) === 92) { if (resolved.charCodeAt(1) === 92 && resolved.charCodeAt(2) !== 63 && resolved.charCodeAt(2) !== 46) return '\\\\?\\UNC\\' + resolved.slice(2); }
        else if (drive(resolved) && resolved.charCodeAt(2) === 92) return '\\\\?\\' + resolved;
        return resolved;
      },
      dirname(p) {
        check(p, 'path');
        if (!p.length) return '.';
        const root = rootLength(p);
        let end = -1, matched = true;
        for (let i = p.length - 1; i >= (windows ? root : 1); --i) { if (isSep(p.charCodeAt(i))) { if (!matched) { end = i; break; } } else matched = false; }
        return end === -1 ? (root ? p.slice(0, root) : '.') : !windows && root && end === 1 ? '//' : p.slice(0, end);
      },
      basename(p, suffix) {
        if (suffix !== undefined) check(suffix, 'suffix');
        check(p, 'path');
        let start = windows && drive(p) ? 2 : 0, end = -1, matched = true;
        if (suffix !== undefined && suffix.length > 0 && suffix.length <= p.length) {
          if (suffix === p) return '';
          let ext = suffix.length - 1, first = -1;
          for (let i = p.length - 1; i >= start; --i) {
            const code = p.charCodeAt(i);
            if (isSep(code)) { if (!matched) { start = i + 1; break; } }
            else { if (first === -1) { matched = false; first = i + 1; } if (ext >= 0) { if (code === suffix.charCodeAt(ext)) { if (--ext === -1) end = i; } else { ext = -1; end = first; } } }
          }
          return p.slice(start, start === end ? first : end === -1 ? p.length : end);
        }
        for (let i = p.length - 1; i >= start; --i) { if (isSep(p.charCodeAt(i))) { if (!matched) { start = i + 1; break; } } else if (end === -1) { matched = false; end = i + 1; } }
        return end === -1 ? '' : p.slice(start, end);
      },
      extname(p) { check(p, 'path'); const start = windows && drive(p) ? 2 : 0, [, end, dot, none] = scan(p, start, start); return none ? '' : p.slice(dot, end); },
      format(object) {
        if (object === null || typeof object !== 'object') throw typeError('pathObject', 'of type object', object);
        const dir = object.dir || object.root, base = object.base || `${object.name || ''}${object.ext ? (object.ext[0] === '.' ? '' : '.') + object.ext : ''}`;
        return !dir ? base : dir === object.root ? dir + base : dir + sep + base;
      },
      parse(p) {
        check(p, 'path');
        const result = { root: '', dir: '', base: '', ext: '', name: '' };
        if (!p.length) return result;
        const root = rootLength(p), [part, end, dot, none] = scan(p, root, windows ? root : 0), from = part || root;
        result.root = p.slice(0, root);
        if (end !== -1) { result.base = p.slice(from, end); if (none) result.name = result.base; else { result.name = p.slice(from, dot); result.ext = p.slice(dot, end); } }
        result.dir = part > 0 && part !== root ? p.slice(0, part - 1) : result.root;
        return result;
      },
    };
    return path;
  };
  const posix = makePath(false), win32 = makePath(true);
  const path = windows ? win32 : posix;
  posix.posix = win32.posix = posix;
  posix.win32 = win32.win32 = win32;

  const pathArg = value => { const text = value instanceof URL ? url.fileURLToPath(value) : ArrayBuffer.isView(value) ? Buffer.from(value.buffer, value.byteOffset, value.byteLength).toString() : String(value); if (text.includes('\0')) throw Object.assign(new TypeError(`The argument 'path' must be a string, Uint8Array, or URL without null bytes. Received ${inspect(text)}`), { code: 'ERR_INVALID_ARG_VALUE' }); return text; };
  const aborted = signal => { if (signal?.aborted) throw Object.assign(new Error('The operation was aborted', { cause: signal.reason }), { name: 'AbortError', code: 'ABORT_ERR' }); };
  const options = value => typeof value === 'string' ? { encoding: value } : value ?? {};
  const kind = Symbol('type'), S = constants.fs;
  const typed = proto => Object.assign(proto, { isFile() { return this[kind]() === S.S_IFREG; }, isDirectory() { return this[kind]() === S.S_IFDIR; }, isSymbolicLink() { return this[kind]() === S.S_IFLNK; }, isBlockDevice() { return this[kind]() === S.S_IFBLK; }, isCharacterDevice() { return this[kind]() === S.S_IFCHR; }, isFIFO() { return this[kind]() === S.S_IFIFO; }, isSocket() { return this[kind]() === S.S_IFSOCK; } });
  class Stats {
    constructor(value) { Object.assign(this, value); this.birthtimeMs ??= this.ctimeMs; }
    [kind]() { return this.mode & S.S_IFMT; }
  }
  for (const name of ['atime', 'mtime', 'ctime', 'birthtime']) Object.defineProperty(Stats.prototype, name, { configurable: true, get() { return this[name] = new Date(this[name + 'Ms']); }, set(value) { Object.defineProperty(this, name, { value, writable: true, enumerable: true, configurable: true }); } });
  class BigIntStats extends Stats {
    constructor(value) {
      super(value);
      for (const name of ['atime', 'mtime', 'ctime', 'birthtime']) { const ms = this[name + 'Ms']; this[name] = new Date(ms); this[name + 'Ms'] = BigInt(Math.floor(ms)); this[name + 'Ns'] = BigInt(Math.floor(ms)) * 1000000n; }
      for (const name of ['dev', 'ino', 'mode', 'nlink', 'uid', 'gid', 'rdev', 'size', 'blksize', 'blocks']) this[name] = BigInt(this[name]);
    }
    [kind]() { return Number(this.mode) & S.S_IFMT; }
  }
  class Dirent {
    #type;
    constructor(name, type, parentPath) { this.name = name; this.parentPath = parentPath; this.#type = type; }
    [kind]() { return this.#type; }
  }
  typed(Stats.prototype); typed(Dirent.prototype);
  class StatFs { constructor(values, big) { ['type', 'bsize', 'frsize', 'blocks', 'bfree', 'bavail', 'files', 'ffree'].forEach((key, i) => { this[key] = big ? BigInt(values[i]) : values[i]; }); } }
  const descriptions = { ENOENT: 'no such file or directory', EACCES: 'permission denied', EEXIST: 'file already exists', ENOTDIR: 'not a directory', EISDIR: 'illegal operation on a directory', ENOTEMPTY: 'directory not empty', EPERM: 'operation not permitted', EBUSY: 'resource busy or locked', EXDEV: 'cross-device link not permitted', EINVAL: 'invalid argument', EROFS: 'read-only file system', ENOSPC: 'no space left on device', EMFILE: 'too many open files', ELOOP: 'too many symbolic links encountered', ENAMETOOLONG: 'name too long', EBADF: 'bad file descriptor', EIO: 'i/o error' };
  const syscalls = { readFile: 'open', writeFile: 'open', appendFile: 'open', readdir: 'scandir', copyFile: 'copyfile', truncate: 'open' };
  const failure = (error, name, args) => {
    if (name === 'rm' && error.code === 'EISDIR') return Object.assign(new Error(`Path is a directory: rm returned EISDIR (is a directory) ${args[0]}`), { name: 'SystemError', code: 'ERR_FS_EISDIR', errno: 21, syscall: 'rm', path: args[0], info: { code: 'EISDIR', message: 'is a directory', path: args[0], syscall: 'rm', errno: 21 } });
    if (!descriptions[error.code]) return error;
    const read = name === 'readFile' && (typeof args[0] === 'number' || error.code === 'EISDIR');
    const syscall = name === 'rm' && (error.code === 'ENOENT' || error.code === 'ENOTDIR') || name === 'rmdir' && args[1].recursive && error.code === 'ENOENT' ? 'lstat' : read ? 'read' : typeof args[0] === 'number' && name.endsWith('File') ? 'write' : syscalls[name] ?? name.toLowerCase(), file = !read && typeof args[0] === 'string' ? args[0] : undefined, dest = ['rename', 'copyFile', 'symlink', 'link'].includes(name) ? args[1] : undefined;
    error.message = `${error.code}: ${descriptions[error.code]}, ${syscall}${file === undefined ? '' : ` '${file}'`}${dest === undefined ? '' : ` -> '${dest}'`}`;
    return Object.assign(error, { errno: constants.errno[error.code] ? -constants.errno[error.code] : undefined, syscall }, file === undefined ? {} : { path: file }, dest === undefined ? {} : { dest });
  };
  const files = { constants: S, promises: { constants: S }, Stats, Dirent, openAsBlob: async (file, o = {}) => new Blob([await files.promises.readFile(file)], { type: o.type ?? '' }) };
  // WARNING: reads and writes check their range before the call, because a failure after it would still move the file position.
  const span = (size, offset, length, write) => {
    if (!write) integer(offset, 'offset', 0, Number.MAX_SAFE_INTEGER);
    else if (offset > size) throw rangeError('offset', `<= ${size}`, offset);
    if (length < 0 || offset + length > size) throw rangeError('length', length < 0 ? '>= 0' : `<= ${size - offset}`, length);
  };
  const chunk = (buffer, offset, length, position) => { if (typeof buffer === 'string') return [new Uint8Array(Buffer.from(buffer, length)), offset ?? null]; const bytes = view(buffer); if (offset !== null && typeof offset === 'object') ({ offset, length, position } = offset); offset = typeof offset === 'number' ? offset : 0; length = typeof length === 'number' ? length : bytes.length - offset; span(bytes.length, offset, length, true); return [bytes.subarray(offset, offset + length), position ?? null]; };
  const vectors = buffers => { if (!Array.isArray(buffers) || !buffers.every(buffer => ArrayBuffer.isView(buffer))) throw typeError('buffers', 'an instance of ArrayBufferView[]', buffers); return buffers.map(buffer => view(buffer)); };
  const scatter = (list, data) => { let at = 0; for (const buffer of list) { buffer.set(data.subarray(at, at + buffer.length)); at += buffer.length; } return data.length; };
  const offsetOrNull = position => typeof position === 'number' ? position : null;
  const seconds = value => value instanceof Date ? value.getTime() / 1000 : Number(value);
  const finish = (name, result, args) => {
    if (name === 'readFile') { const encoding = options(args[1]).encoding; return utf8.test(encoding) ? result : encoding && encoding !== 'buffer' ? bytes(result).toString(encoding) : bytes(result); }
    if (name === 'stat' || name === 'lstat' || name === 'fstat') return args[1]?.bigint ? new BigIntStats(result) : new Stats(result);
    if (name === 'statfs') return new StatFs(result, args[1]?.bigint);
    if (name === 'readdir') {
      const opts = options(args[1]), raw = opts.encoding === 'buffer';
      return result.map(([name, type]) => {
        if (!opts.withFileTypes) return raw ? Buffer.from(name) : name;
        const index = Math.max(name.lastIndexOf('/'), windows ? name.lastIndexOf('\\') : -1), base = name.slice(index + 1);
        return new Dirent(raw ? Buffer.from(base) : base, type, index < 0 ? args[0] : path.join(args[0], name.slice(0, index)));
      });
    }
    return result ?? undefined;
  };
  const handles = new __appsFinalizers(fd => { try { sync('close', fd); } catch {} });
  class FileHandle {
    #fd;
    constructor(fd) { this.#fd = fd; handles.register(this, fd, this); }
    get fd() { return this.#fd; }
    async close() { const fd = this.#fd; if (fd < 0) return; this.#fd = -1; handles.unregister(this); await asyncCall('close', fd); }
    async read(buffer = Buffer.alloc(16384), offset = 0, length, position = null) { if (!ArrayBuffer.isView(buffer)) ({ buffer = Buffer.alloc(16384), offset = 0, length, position = null } = buffer ?? {}); else if (offset !== null && typeof offset === 'object') ({ offset = 0, length, position = null } = offset); offset ??= 0; length ??= buffer.byteLength - offset; span(buffer.byteLength, offset, length); const data = bytes(await asyncCall('read', this.#fd, length, position)); view(buffer).set(data, offset); return { bytesRead: data.length, buffer }; }
    async write(buffer, ...args) { return { bytesWritten: await asyncCall('write', this.#fd, ...chunk(buffer, ...args)), buffer }; }
    async readv(buffers, position) { const list = vectors(buffers); return { bytesRead: scatter(list, bytes(await asyncCall('read', this.#fd, list.reduce((n, b) => n + b.length, 0), offsetOrNull(position)))), buffers }; }
    async writev(buffers, position) { return { bytesWritten: await asyncCall('write', this.#fd, Buffer.concat(vectors(buffers)), offsetOrNull(position)), buffers }; }
    async readFile(o) { const { encoding, signal } = options(o), chunks = []; for (;;) { aborted(signal); const chunk = await asyncCall('read', this.#fd, 1048576, null); if (!chunk.length) break; chunks.push(bytes(chunk)); } const b = Buffer.concat(chunks); return encoding && encoding !== 'buffer' ? b.toString(encoding) : b; }
    async writeFile(data, o) { const { encoding, signal } = options(o), bytes = view(data, encoding); let at = 0; do { aborted(signal); at += await asyncCall('write', this.#fd, bytes.subarray(at, at + 1048576), null); } while (at < bytes.length); }
    async appendFile(data, o) { await this.writeFile(data, o); }
    async stat(options) { const result = await asyncCall('fstat', this.#fd); return options?.bigint ? new BigIntStats(result) : new Stats(result); }
    async sync() { await asyncCall('fsync', this.#fd); }
    async datasync() { await asyncCall('fdatasync', this.#fd); }
    async truncate(size = 0) { await asyncCall('ftruncate', this.#fd, size); }
    async chmod(mode) { await asyncCall('fchmod', ...prepare('fchmod', [this.#fd, mode])); }
    async chown(uid, gid) { await asyncCall('fchown', this.#fd, uid, gid); }
    async utimes(atime, mtime) { await asyncCall('futimes', this.#fd, seconds(atime), seconds(mtime)); }
    createReadStream(o) { return files.createReadStream(null, { ...o, fd: this }); }
    createWriteStream(o) { return files.createWriteStream(null, { ...o, fd: this }); }
    [Symbol.asyncDispose]() { return this.close(); }
  }
  const flags = new Set(['r', 'rs', 'sr', 'r+', 'rs+', 'sr+', 'w', 'wx', 'xw', 'w+', 'wx+', 'xw+', 'a', 'ax', 'xa', 'as', 'sa', 'a+', 'ax+', 'xa+', 'as+', 'sa+']);
  const flag = value => { if (value == null || Number.isInteger(value) || flags.has(value)) return value; throw Object.assign(new TypeError(`The argument 'flags' is invalid. Received ${inspect(value)}`), { code: 'ERR_INVALID_ARG_VALUE' }); };
  const prepare = (name, args) => {
    args = [...args];
    if (name === 'open') args[1] = flag(args[1]);
    if (typeof args[0] !== 'number') args[0] = pathArg(args[0]);
    if (['rename', 'copyFile', 'symlink', 'link'].includes(name)) args[1] = pathArg(args[1]);
    if (['readFile', 'readdir', 'mkdir', 'rm', 'rmdir'].includes(name)) args[1] = options(args[1]);
    if (name === 'writeFile' || name === 'appendFile') { args[2] = options(args[2]); flag(args[2].flag); if (typeof args[1] !== 'string' && !ArrayBuffer.isView(args[1])) throw Object.assign(new TypeError('The "data" argument must be of type string or an instance of Buffer, TypedArray, or DataView.' + ' Received ' + received(args[1])), { code: 'ERR_INVALID_ARG_TYPE' }); args[1] = typeof args[1] === 'string' && (!args[2].encoding || utf8.test(args[2].encoding)) ? args[1] : view(args[1], args[2].encoding); }
    if (name === 'utimes' || name === 'lutimes' || name === 'futimes') { args[1] = seconds(args[1]); args[2] = seconds(args[2]); }
    if (name === 'truncate' || name === 'ftruncate') args[1] ??= 0;
    if (name === 'chmod' || name === 'fchmod') { const mode = typeof args[1] === 'string' && /^[0-7]+$/.test(args[1]) ? parseInt(args[1], 8) : args[1]; if (!Number.isInteger(mode) || mode < 0 || mode > 0xFFFFFFFF) throw Object.assign(new TypeError(`The argument 'mode' must be a 32-bit unsigned integer or an octal string. Received ${inspect(args[1])}`), { code: 'ERR_INVALID_ARG_VALUE' }); args[1] = mode; }
    if (name === 'mkdir' && typeof args[1].mode === 'string') args[1].mode = parseInt(args[1].mode, 8);
    return args;
  };
  for (const name of ['readFile', 'writeFile', 'appendFile', 'mkdir', 'readdir', 'stat', 'lstat', 'fstat', 'statfs', 'rename', 'copyFile', 'realpath', 'readlink', 'symlink', 'link', 'chmod', 'chown', 'lchown', 'utimes', 'lutimes', 'truncate', 'access', 'unlink', 'rm', 'rmdir', 'mkdtemp', 'open', 'close', 'fsync', 'fdatasync', 'ftruncate', 'fchmod', 'fchown', 'futimes']) {
    files[name + 'Sync'] = (...args) => {
      args = prepare(name, args);
      let result;
      try { result = sync(name, ...args); }
      catch (error) { if ((name === 'stat' || name === 'lstat') && args[1]?.throwIfNoEntry === false && error.code === 'ENOENT') return undefined; throw failure(error, name, args); }
      return finish(name, result, args);
    };
    const task = async (...args) => {
      args = prepare(name, args);
      const signal = name.endsWith('File') ? args[name === 'readFile' ? 1 : 2]?.signal : undefined;
      aborted(signal);
      try { const result = finish(name, await asyncCall(name, ...args), args); aborted(signal); return result; } catch (error) { throw failure(error, name, args); }
    };
    if (name === 'open') files.promises.open = async (...args) => new FileHandle(await task(...args));
    else if (name.endsWith('File') && name !== 'copyFile') files.promises[name] = (file, ...args) => file instanceof FileHandle ? file[name](...args) : task(file, ...args);
    else if (name[0] !== 'f' && name !== 'close') files.promises[name] = task;
    files[name] = (...args) => { const callback = args.pop(); if (typeof callback !== 'function') throw Object.assign(new TypeError('The "cb" argument must be of type function'), { code: 'ERR_INVALID_ARG_TYPE' }); task(...args).then(result => callback(null, result), callback); };
  }
  files.realpathSync.native = files.realpathSync; files.realpath.native = files.realpath;
  files.existsSync = value => { try { sync('access', pathArg(value)); return true; } catch { return false; } };
  files.readSync = (fd, buffer, offset = 0, length, position = null) => { if (offset !== null && typeof offset === 'object') ({ offset = 0, length, position = null } = offset); offset ??= 0; length ??= buffer.byteLength - offset; span(buffer.byteLength, offset, length); let data; try { data = bytes(sync('read', fd, length, position)); } catch (error) { throw failure(error, 'read', [fd]); } view(buffer).set(data, offset); return data.length; };
  files.writeSync = (fd, ...args) => { try { return sync('write', fd, ...chunk(...args)); } catch (error) { throw failure(error, 'write', [fd]); } };
  files.read = (fd, buffer, offset, length, position, callback) => {
    callback = [buffer, offset, length, position, callback].findLast(value => typeof value === 'function');
    if (!ArrayBuffer.isView(buffer)) ({ buffer = Buffer.alloc(16384), offset = 0, length = buffer.byteLength - offset, position = null } = typeof buffer === 'object' ? buffer : {});
    else if (typeof offset === 'object' && offset !== null) ({ offset = 0, length = buffer.byteLength - offset, position = null } = offset);
    offset = typeof offset === 'number' ? offset : 0; length = typeof length === 'number' ? length : buffer.byteLength - offset; position = typeof position === 'number' || typeof position === 'bigint' ? Number(position) : null;
    span(buffer.byteLength, offset, length);
    asyncCall('read', fd, length, position).then(data => { data = bytes(data); view(buffer).set(data, offset); callback(null, data.length, buffer); }, error => callback(failure(error, 'read', [fd])));
  };
  files.write = (fd, buffer, ...args) => { const callback = args.pop(); asyncCall('write', fd, ...chunk(buffer, ...args)).then(written => callback(null, written, buffer), error => callback(failure(error, 'write', [fd]))); };
  files.readvSync = (fd, buffers, position) => { const list = vectors(buffers); try { return scatter(list, bytes(sync('read', fd, list.reduce((n, b) => n + b.length, 0), offsetOrNull(position)))); } catch (error) { throw failure(error, 'read', [fd]); } };
  files.writevSync = (fd, buffers, position) => { const data = Buffer.concat(vectors(buffers)); try { return sync('write', fd, data, offsetOrNull(position)); } catch (error) { throw failure(error, 'write', [fd]); } };
  files.readv = (fd, buffers, position, callback) => { if (typeof position === 'function') [position, callback] = [null, position]; const list = vectors(buffers); asyncCall('read', fd, list.reduce((n, b) => n + b.length, 0), offsetOrNull(position)).then(data => callback(null, scatter(list, bytes(data)), buffers), error => callback(failure(error, 'read', [fd]))); };
  files.writev = (fd, buffers, position, callback) => { if (typeof position === 'function') [position, callback] = [null, position]; asyncCall('write', fd, Buffer.concat(vectors(buffers)), offsetOrNull(position)).then(written => callback(null, written, buffers), error => callback(failure(error, 'write', [fd]))); };
  // WARNING: like Node, globs match names case-insensitively on macOS and Windows, and wildcards and `**` skip names that start with a dot.
  const fold = windows || info.platform === 'darwin', split = text => text.split(windows ? /[\\/]+/ : /\/+/).filter(Boolean);
  const expand = text => { const brace = /\{([^{}]*,[^{}]*)\}/.exec(text); return brace ? brace[1].split(',').flatMap(part => expand(text.slice(0, brace.index) + part + text.slice(brace.index + brace[0].length))) : [text]; };
  const segment = text => {
    if (text === '**' || !/[*?[]/.test(text)) return text;
    let source = '';
    for (let i = 0; i < text.length; i++) {
      const c = text[i], end = c === '[' ? text.indexOf(']', i + 2) : -1;
      if (c === '*') source += '.*';
      else if (c === '?') source += '.';
      else if (end > 0) { source += '[' + (text[i + 1] === '!' ? '^' + text.slice(i + 2, end) : text.slice(i + 1, end)) + ']'; i = end; }
      else source += c.replace(/[.+^$|{}()[\]\\]/, '\\$&');
    }
    return new RegExp(`^${text[0] === '.' ? '' : '(?!\\.)'}${source}$`, fold ? 'i' : '');
  };
  const named = (part, name) => typeof part === 'string' ? (fold ? part.toLowerCase() === name.toLowerCase() : part === name) : part.test(name);
  const globbed = (parts, names, i = 0, j = 0) => i === parts.length ? j === names.length : parts[i] === '**' ? globbed(parts, names, i + 1, j) || (j < names.length && names[j][0] !== '.' && globbed(parts, names, i, j + 1)) : j < names.length && named(parts[i], names[j]) && globbed(parts, names, i + 1, j + 1);
  const glob = function* (patterns, o = {}) {
    const cwd = o.cwd === undefined ? call('cwd') : pathArg(o.cwd), results = new Map(), excludes = Array.isArray(o.exclude) && o.exclude.flatMap(expand).map(text => split(text).map(segment));
    const excluded = (rel, entry) => excludes ? excludes.some(parts => globbed(parts, split(rel))) : !!o.exclude?.(o.withFileTypes ? entry : rel);
    const walk = function* (dir, rel, parts, index) {
      if (index === parts.length) return void results.set(rel, new Dirent(path.basename(dir), S.S_IFDIR, path.dirname(dir)));
      const part = parts[index];
      if (part === '**') {
        yield* walk(dir, rel, parts, index + 1);
        for (const entry of yield ['readdir', dir]) {
          const next = path.join(rel, entry.name);
          if (entry.name[0] === '.' || excluded(next, entry)) continue;
          if (entry.isDirectory()) yield* walk(path.join(dir, entry.name), next, parts, index);
          else if (index === parts.length - 1) results.set(next, entry);
        }
        return;
      }
      const list = typeof part === 'string' ? [yield ['lstat', path.join(dir, part)]].filter(Boolean).map(stats => new Dirent(part, stats.mode & S.S_IFMT, dir)) : (yield ['readdir', dir]).filter(entry => part.test(entry.name));
      for (const entry of list) {
        const next = path.join(rel, entry.name);
        if (excluded(next, entry)) continue;
        if (index === parts.length - 1) results.set(next, entry);
        else if (entry.isDirectory() || entry.isSymbolicLink()) yield* walk(path.join(dir, entry.name), next, parts, index + 1);
      }
    };
    for (const pattern of new Set([patterns].flat().flatMap(expand))) {
      const root = path.isAbsolute(pattern) ? path.parse(pattern).root : '';
      yield* walk(root || cwd, root || '.', split(pattern.slice(root.length)).map(segment), 0);
    }
    return o.withFileTypes ? [...results.values()] : [...results.keys()];
  };
  files.globSync = (pattern, o) => {
    const steps = glob(pattern, o);
    let step = steps.next();
    while (!step.done) { const [op, file] = step.value; let result; try { result = op === 'readdir' ? files.readdirSync(file, { withFileTypes: true }) : files.lstatSync(file, { throwIfNoEntry: false }); } catch { result = op === 'readdir' ? [] : undefined; } step = steps.next(result); }
    return step.value;
  };
  files.promises.glob = async function* (pattern, o) {
    const steps = glob(pattern, o);
    let step = steps.next();
    while (!step.done) { const [op, file] = step.value; let result; try { result = op === 'readdir' ? await files.promises.readdir(file, { withFileTypes: true }) : await files.promises.lstat(file); } catch { result = op === 'readdir' ? [] : undefined; } step = steps.next(result); }
    yield* step.value;
  };
  files.glob = (pattern, o, callback) => { if (typeof o === 'function') [o, callback] = [{}, o]; Array.fromAsync(files.promises.glob(pattern, o)).then(list => callback(null, list), callback); };
  class Dir {
    #entries;
    constructor(path, entries) { this.path = path; this.#entries = entries; }
    readSync() { return this.#entries.shift() ?? null; }
    read(callback) { const entry = this.readSync(); if (callback) queueMicrotask(() => callback(null, entry)); else return Promise.resolve(entry); }
    closeSync() { this.#entries = []; }
    close(callback) { this.closeSync(); if (callback) queueMicrotask(() => callback(null)); else return Promise.resolve(); }
    async *[Symbol.asyncIterator]() { for (let entry; (entry = this.readSync());) yield entry; }
    [Symbol.dispose]() { this.closeSync(); }
    [Symbol.asyncDispose]() { return this.close(); }
  }
  files.Dir = Dir;
  files.opendirSync = (value, o) => new Dir(pathArg(value), files.readdirSync(value, { ...options(o), withFileTypes: true }));
  files.promises.opendir = async (value, o) => new Dir(pathArg(value), await files.promises.readdir(value, { ...options(o), withFileTypes: true }));
  files.opendir = (value, o, callback) => { if (typeof o === 'function') { callback = o; o = {}; } files.promises.opendir(value, o).then(dir => callback(null, dir), callback); };
  const invalidCopy = (message, to) => Object.assign(new Error(`Invalid src or dest: cp returned EINVAL (${message}) ${to}`), { name: 'SystemError', code: 'ERR_FS_CP_EINVAL', errno: 22, syscall: 'cp', path: to, info: { code: 'EINVAL', message, path: to, syscall: 'cp', errno: 22 } });
  const copy = function* (from, to, o, nested) {
    if (o.filter && !(yield ['filter', from, to])) return;
    const source = yield [o.dereference ? 'stat' : 'lstat', from];
    if (!nested) {
      // WARNING: like Node, copying onto the source or into itself fails before anything is written.
      const target = (yield ['exists', to]) ? yield [o.dereference ? 'stat' : 'lstat', to] : null;
      if (target?.ino && target.dev && target.ino === source.ino && target.dev === source.dev) throw invalidCopy('src and dest cannot be the same', to);
      const parts = path.resolve(from).split(path.sep), inside = path.resolve(to).split(path.sep);
      if (source.isDirectory() && parts.every((part, i) => inside[i] === part)) throw invalidCopy(`cannot copy ${from} to a subdirectory of self ${to}`, to);
    }
    if (source.isDirectory()) {
      if (!o.recursive) throw Object.assign(new Error(`Recursive option not enabled, cannot copy a directory: ${from}/`), { code: 'ERR_FS_EISDIR' });
      yield ['mkdir', to, { recursive: true }];
      for (const name of yield ['readdir', from]) yield* copy(path.join(from, name), path.join(to, name), o, true);
      return;
    }
    if (yield ['exists', to]) {
      if (o.force === false) { if (o.errorOnExist) throw failure(Object.assign(new Error(), { code: 'EEXIST' }), 'cp', [to]); return; }
      if (source.isSymbolicLink()) yield ['unlink', to];
    }
    if (source.isSymbolicLink()) yield ['symlink', yield ['readlink', from], to];
    else yield ['copyFile', from, to, o.mode ?? 0];
    if (o.preserveTimestamps) yield ['utimes', to, source.atime, source.mtime];
  };
  files.cpSync = (from, to, o = {}) => { const steps = copy(pathArg(from), pathArg(to), o); for (let step = steps.next(); !step.done;) { const [op, ...args] = step.value; step = steps.next(op === 'filter' ? o.filter(...args) : op === 'exists' ? files.existsSync(args[0]) : files[op + 'Sync'](...args)); } };
  files.promises.cp = async (from, to, o = {}) => { const steps = copy(pathArg(from), pathArg(to), o); for (let step = steps.next(); !step.done;) { const [op, ...args] = step.value; step = steps.next(op === 'filter' ? await o.filter(...args) : op === 'exists' ? files.existsSync(args[0]) : await files.promises[op](...args)); } };
  files.cp = (from, to, o, callback) => { if (typeof o === 'function') { callback = o; o = {}; } files.promises.cp(from, to, o).then(() => callback(null), callback); };
  files.watch = (value, o = {}, listener) => { if (typeof o === 'function') { listener = o; o = {}; } o = options(o); const id = sync('watch', pathArg(value), o), watcher = new EventEmitter(); watcher.close = () => { if (!watchers.delete(id)) return; sync('unwatch', id); watcher.emit('close'); }; watcher.ref = () => { watcher._ref = true; return watcher; }; watcher.unref = () => { watcher._ref = false; return watcher; }; watcher._ref = o.persistent !== false; watchers.set(id, watcher); if (listener) watcher.on('change', listener); o.signal?.addEventListener('abort', () => watcher.close(), { once: true }); return watcher; };
  files.promises.watch = async function* (value, o = {}) { const watcher = files.watch(value, { ...options(o), signal: undefined }); try { for await (const [eventType, filename] of EventEmitter.on(watcher, 'change', { signal: options(o).signal })) yield { eventType, filename }; } finally { watcher.close(); } };
  const polls = new Map();
  files.watchFile = (value, o, listener) => {
    if (typeof o === 'function') { listener = o; o = {}; }
    const file = pathArg(value), current = () => files.statSync(file, { throwIfNoEntry: false }) ?? new Stats({ dev: 0, ino: 0, mode: 0, nlink: 0, uid: 0, gid: 0, rdev: 0, size: 0, blksize: 0, blocks: 0, atimeMs: 0, mtimeMs: 0, ctimeMs: 0, birthtimeMs: 0 });
    let entry = polls.get(file);
    if (!entry) {
      entry = Object.assign(new EventEmitter(), { previous: current(), ref() { return this; }, unref() { return this; } });
      entry.timer = setInterval(() => { const next = current(), previous = entry.previous; entry.previous = next; if (['mtimeMs', 'ctimeMs', 'size', 'ino', 'mode', 'nlink'].some(name => next[name] !== previous[name])) entry.emit('change', next, previous); }, o.interval ?? 5007);
      polls.set(file, entry);
    }
    entry.on('change', listener);
    return entry;
  };
  files.unwatchFile = (value, listener) => { const file = pathArg(value), entry = polls.get(file); if (!entry) return; if (listener) entry.off('change', listener); else entry.removeAllListeners('change'); if (!entry.listenerCount('change')) { clearInterval(entry.timer); polls.delete(file); } };
  const poll = () => { if (!pending.size && !watchers.size && !resizing.size) return; for (const event of sync('poll')) { if (event.resize) { for (const stream of resizing) stream.emit('resize'); } else if (event.watch) { const watcher = watchers.get(event.id); if (!watcher) continue; if (event.error) watcher.emit('error', new Error(event.error)); else for (const p of event.paths) watcher.emit('change', event.event, p); } else { const item = pending.get(event.id); pending.delete(event.id); if (item) { if (event.error) item.reject(Object.assign(new Error(event.error), { code: event.code ?? undefined })); else item.resolve(event.value); } } } };
  poll.pending = () => pending.size > 0 || [...watchers.values()].some(w => w._ref);

  const os = {
    EOL: windows ? '\r\n' : '\n', devNull: windows ? '\\\\.\\nul' : '/dev/null',
    platform: () => info.platform, arch: () => info.arch, machine: () => details().machine, release: () => details().release, version: () => details().version,
    type: () => ({ darwin: 'Darwin', win32: 'Windows_NT', linux: 'Linux' })[info.platform],
    homedir: () => info.homedir, tmpdir: () => String(info.tmpdir).replace(/(.)[\\/]+$/, '$1'), hostname: () => info.hostname, endianness: () => 'LE',
    availableParallelism: () => info.parallelism,
    cpus: () => Array.from({ length: info.parallelism }, () => ({ model: details().model, speed: details().speed, times: { user: 0, nice: 0, sys: 0, idle: 0, irq: 0 } })),
    totalmem: () => details().totalmem, freemem: () => sync('os.freemem'), uptime: () => sync('os.uptime'), loadavg: () => sync('os.loadavg'),
    networkInterfaces: () => sync('os.interfaces'),
    userInfo: () => ({ ...details().user }),
    getPriority: (pid = 0) => priority('uv_os_getpriority', pid),
    setPriority(pid, value) { if (value === undefined) [pid, value] = [0, pid]; if (!Number.isInteger(value) || value < -20 || value > 19) throw Object.assign(new RangeError(`The value of "priority" is out of range. It must be >= -20 && <= 19. Received ${inspect(value)}`), { code: 'ERR_OUT_OF_RANGE' }); priority('uv_os_setpriority', pid, value); },
    constants: { UV_UDP_REUSEADDR: 4, dlopen: constants.dlopen, signals: constants.signals, errno: constants.errno, priority: { PRIORITY_LOW: 19, PRIORITY_BELOW_NORMAL: 10, PRIORITY_NORMAL: 0, PRIORITY_ABOVE_NORMAL: -7, PRIORITY_HIGH: -14, PRIORITY_HIGHEST: -20 } },
  };
  const priority = (syscall, pid, value) => {
    if (typeof pid !== 'number') throw Object.assign(new TypeError('The "pid" argument must be of type number.' + ' Received ' + received(pid)), { code: 'ERR_INVALID_ARG_TYPE' });
    try { return sync('os.priority', pid, value ?? null); } catch (error) {
      const info = { errno: -(constants.errno[error.code] ?? 0), code: error.code, message: { ESRCH: 'no such process', EACCES: 'permission denied', EPERM: 'operation not permitted' }[error.code] ?? error.message, syscall };
      throw Object.assign(new Error(`A system error occurred: ${syscall} returned ${info.code} (${info.message})`), { name: 'SystemError', code: 'ERR_SYSTEM_ERROR', errno: info.errno, syscall, info });
    }
  };

  const promisify = fn => {
    if (typeof fn !== 'function') throw Object.assign(new TypeError('The "original" argument must be of type function'), { code: 'ERR_INVALID_ARG_TYPE' });
    if (fn[promisify.custom]) return fn[promisify.custom];
    const promised = function(...args) { return new Promise((resolve, reject) => fn.call(this, ...args, (error, ...values) => error ? reject(error) : resolve(values[0]))); };
    return Object.defineProperty(promised, promisify.custom, { value: promised });
  };
  promisify.custom = Symbol.for('nodejs.util.promisify.custom');
  const tag = value => Object.prototype.toString.call(value).slice(8, -1);
  const brand = (method, value) => { if (value === null || typeof value !== 'object') return false; try { method.call(value); return true; } catch { return false; } };
  const getter = (proto, name) => Object.getOwnPropertyDescriptor(proto, name).get, byteLength = getter(ArrayBuffer.prototype, 'byteLength'), sharedLength = typeof SharedArrayBuffer !== 'undefined' ? getter(SharedArrayBuffer.prototype, 'byteLength') : () => { throw 0; };
  const dataLength = getter(DataView.prototype, 'byteLength'), source = getter(RegExp.prototype, 'source'), mapSize = getter(Map.prototype, 'size'), setSize = getter(Set.prototype, 'size');
  const types = {
    isAnyArrayBuffer: v => brand(byteLength, v) || brand(sharedLength, v), isArrayBuffer: v => brand(byteLength, v), isSharedArrayBuffer: v => brand(sharedLength, v),
    isArrayBufferView: v => ArrayBuffer.isView(v), isTypedArray: v => ArrayBuffer.isView(v) && !(v instanceof DataView), isDataView: v => brand(dataLength, v),
    isDate: v => brand(Date.prototype.getTime, v), isRegExp: v => v !== RegExp.prototype && brand(source, v), isMap: v => brand(mapSize, v), isSet: v => brand(setSize, v), isWeakMap: v => brand(WeakMap.prototype.has, v), isWeakSet: v => brand(WeakSet.prototype.has, v),
    isPromise: v => v instanceof Promise, isNativeError: v => Error.isError(v), isCryptoKey: v => typeof CryptoKey !== 'undefined' && v instanceof CryptoKey, isKeyObject: () => false, isExternal: () => false, isProxy: () => false,
    isAsyncFunction: v => typeof v === 'function' && tag(v).startsWith('Async'), isGeneratorFunction: v => typeof v === 'function' && tag(v).endsWith('GeneratorFunction'), isGeneratorObject: v => tag(v) === 'Generator' || tag(v) === 'AsyncGenerator',
    isMapIterator: v => tag(v) === 'Map Iterator', isSetIterator: v => tag(v) === 'Set Iterator', isModuleNamespaceObject: v => tag(v) === 'Module', isArgumentsObject: v => tag(v) === 'Arguments',
    isBoxedPrimitive: v => ['Number', 'String', 'Boolean', 'BigInt', 'Symbol'].some(name => types[`is${name}Object`](v)),
  };
  for (const name of ['Number', 'String', 'Boolean', 'BigInt', 'Symbol']) types[`is${name}Object`] = v => brand(globalThis[name].prototype.valueOf, v);
  for (const name of ['Uint8Array', 'Uint8ClampedArray', 'Uint16Array', 'Uint32Array', 'Int8Array', 'Int16Array', 'Int32Array', 'Float16Array', 'Float32Array', 'Float64Array', 'BigInt64Array', 'BigUint64Array']) types['is' + name] = v => ArrayBuffer.isView(v) && tag(v) === name;
  const colors = inspect.colors;
  const invalid = (name, value) => Object.assign(new TypeError(`The argument '${name}' is invalid. Received ${inspect(value)}`), { code: 'ERR_INVALID_ARG_VALUE' });
  const parseArgs = ({ args = info.argv.slice(info.isPackaged ? 1 : 2), options = {}, strict = true, allowPositionals = !strict, allowNegative = false, tokens: withTokens = false } = {}) => {
    const values = { __proto__: null }, positionals = [], tokens = [];
    const short = Object.fromEntries(Object.entries(options).filter(([, o]) => o.short).map(([name, o]) => [o.short, name]));
    const fail = (code, message) => { throw Object.assign(new TypeError(message), { code }); };
    const store = (name, rawName, index, value, inline) => {
      const option = options[name];
      if (strict && !option) fail('ERR_PARSE_ARGS_UNKNOWN_OPTION', `Unknown option '${rawName}'`);
      if (strict && option?.type === 'string' && value === undefined) fail('ERR_PARSE_ARGS_INVALID_OPTION_VALUE', `Option '${rawName} <value>' argument missing`);
      if (strict && option?.type === 'boolean' && inline) fail('ERR_PARSE_ARGS_INVALID_OPTION_VALUE', `Option '${rawName}' does not take an argument`);
      tokens.push({ kind: 'option', name, rawName, index, value, inlineValue: value === undefined ? undefined : inline });
      const result = option?.type === 'boolean' ? value !== false : value ?? true;
      if (option?.multiple) (values[name] ??= []).push(result); else values[name] = result;
    };
    for (let i = 0; i < args.length; i++) {
      const arg = args[i];
      if (arg === '--') { tokens.push({ kind: 'option-terminator', index: i }); for (const rest of args.slice(i + 1)) { if (!allowPositionals) fail('ERR_PARSE_ARGS_UNEXPECTED_POSITIONAL', `Unexpected argument '${rest}'. This command does not take positional arguments`); positionals.push(rest); tokens.push({ kind: 'positional', index: ++i, value: rest }); } break; }
      if (arg.startsWith('--')) {
        const eq = arg.indexOf('='), name = eq > 0 ? arg.slice(2, eq) : arg.slice(2);
        if (eq > 0) store(name, '--' + name, i, arg.slice(eq + 1), true);
        else if (allowNegative && name.startsWith('no-') && options[name.slice(3)]?.type === 'boolean') store(name.slice(3), arg, i, false, false);
        else if (options[name]?.type === 'string') store(name, arg, i, i + 1 < args.length && !args[i + 1].startsWith('-') ? args[++i] : undefined, false);
        else store(name, arg, i, undefined, false);
      } else if (arg.startsWith('-') && arg !== '-') {
        for (let j = 1; j < arg.length; j++) {
          const letter = arg[j], name = short[letter] ?? letter;
          if (options[name]?.type !== 'string') { store(name, '-' + letter, i, undefined, false); continue; }
          if (j + 1 < arg.length) store(name, '-' + letter, i, arg.slice(j + 1), true);
          else store(name, '-' + letter, i, i + 1 < args.length && !args[i + 1].startsWith('-') ? args[++i] : undefined, false);
          break;
        }
      } else {
        if (!allowPositionals) fail('ERR_PARSE_ARGS_UNEXPECTED_POSITIONAL', `Unexpected argument '${arg}'. This command does not take positional arguments`);
        positionals.push(arg); tokens.push({ kind: 'positional', index: i, value: arg });
      }
    }
    for (const [name, option] of Object.entries(options)) if (option.default !== undefined && values[name] === undefined) values[name] = option.default;
    return withTokens ? { values, positionals, tokens } : { values, positionals };
  };
  const messages = Object.fromEntries('E2BIG:argument list too long|EACCES:permission denied|EADDRINUSE:address already in use|EADDRNOTAVAIL:address not available|EAGAIN:resource temporarily unavailable|EBADF:bad file descriptor|EBUSY:resource busy or locked|ECONNABORTED:software caused connection abort|ECONNREFUSED:connection refused|ECONNRESET:connection reset by peer|EEXIST:file already exists|EFBIG:file too large|EHOSTUNREACH:host is unreachable|EINTR:interrupted system call|EINVAL:invalid argument|EIO:i/o error|EISDIR:illegal operation on a directory|ELOOP:too many symbolic links encountered|EMFILE:too many open files|EMLINK:too many links|ENAMETOOLONG:name too long|ENETUNREACH:network is unreachable|ENFILE:file table overflow|ENOENT:no such file or directory|ENOMEM:not enough memory|ENOSPC:no space left on device|ENOSYS:function not implemented|ENOTCONN:socket is not connected|ENOTDIR:not a directory|ENOTEMPTY:directory not empty|ENOTSUP:operation not supported on socket|EPERM:operation not permitted|EPIPE:broken pipe|EROFS:read-only file system|ESRCH:no such process|ETIMEDOUT:connection timed out|ETXTBSY:text file is busy|EXDEV:cross-device link not permitted'.split('|').map(entry => entry.split(':')));
  const systemError = errno => {
    if (typeof errno !== 'number') throw typeError('err', 'of type number', errno);
    if (errno >= 0 || !Number.isSafeInteger(errno)) throw rangeError('err', 'a negative integer', errno);
    const name = Object.keys(constants.errno).find(key => constants.errno[key] === -errno);
    return messages[name] ? [name, messages[name]] : [`Unknown system error ${errno}`, `Unknown system error ${errno}`];
  };
  const token = /[^!#$%&'*+\-.^_`|~A-Za-z0-9]/, quoted = /[^\t\u0020-~\u0080-\u00FF]/, lower = text => text.replace(/[A-Z]+/g, c => c.toLowerCase());
  const mimeError = (production, text, index) => error(TypeError, 'ERR_INVALID_MIME_SYNTAX', `The MIME syntax for a ${production} in "${text}" is invalid${index !== -1 ? ` at ${index}` : ''}`);
  const mimeToken = (value, production) => { value = `${value}`; const index = value.search(token); if (!value || index !== -1) throw mimeError(production, value, index); return lower(value); };
  let parseParams;
  class MIMEParams {
    #data = new Map();
    static {
      parseParams = (params, text) => {
        const end = text.search(/[\r\n\t ]*$/), data = params.#data;
        for (let position = 0; position < end;) {
          position += text.slice(position).search(/[^\r\n\t ]|$/);
          const after = text.slice(position).search(/[;=]|$/) + position, name = lower(text.slice(position, after));
          position = after;
          if (position < end && text[position++] === ';') continue;
          if (position >= end) break;
          let value;
          if (text[position] === '"') {
            const match = /^(?:([\\]$)|[\\][\s\S]|[^"])*(?:(")|$)/u.exec(text.slice(++position));
            position += match[0].length;
            value = (match[1] || match[2] ? match[0].slice(0, -1) : match[0]).replace(/\\([\s\S])/g, '$1') + (match[1] ? '\\' : '');
          } else {
            const valueEnd = text.indexOf(';', position), raw = valueEnd === -1 ? text.slice(position) : text.slice(position, valueEnd);
            position += raw.length;
            value = raw.slice(0, raw.search(/[\r\n\t ]*$/));
            if (!value) continue;
          }
          if (name && name.search(token) === -1 && value.search(quoted) === -1 && !data.has(name)) data.set(name, value);
          position++;
        }
      };
    }
    delete(name) { this.#data.delete(name); }
    get(name) { return this.#data.get(name) ?? null; }
    has(name) { return this.#data.has(name); }
    set(name, value) { name = `${name}`; value = `${value}`; const index = name.search(token); if (!name || index !== -1) throw mimeError('parameter name', name, index); const bad = value.search(quoted); if (bad !== -1) throw mimeError('parameter value', value, bad); this.#data.set(name, value); }
    entries() { return this.#data.entries(); }
    keys() { return this.#data.keys(); }
    values() { return this.#data.values(); }
    toString() { let out = ''; for (const [name, value] of this.#data) out += `${out ? ';' : ''}${name}=${!value ? '""' : value.search(token) === -1 ? value : `"${value.replace(/["\\]/g, '\\$&')}"`}`; return out; }
    toJSON() { return this.toString(); }
    [Symbol.iterator]() { return this.#data.entries(); }
  }
  class MIMEType {
    #type; #subtype; #params = new MIMEParams();
    constructor(text) {
      text = `${text}`;
      let position = text.search(/[^\r\n\t ]|$/);
      const typeEnd = text.indexOf('/', position), type = typeEnd === -1 ? text.slice(position) : text.slice(position, typeEnd), typeIndex = type.search(token);
      if (!type || typeIndex !== -1 || typeEnd === -1) throw mimeError('type', text, typeIndex);
      position = typeEnd + 1;
      const subtypeEnd = text.indexOf(';', position), raw = subtypeEnd === -1 ? text.slice(position) : text.slice(position, subtypeEnd), subtype = raw.slice(0, raw.search(/[\r\n\t ]*$/)), subtypeIndex = subtype.search(token);
      if (!subtype || subtypeIndex !== -1) throw mimeError('subtype', text, subtypeIndex);
      this.#type = lower(type); this.#subtype = lower(subtype);
      parseParams(this.#params, text.slice(position + raw.length + (subtypeEnd !== -1)));
    }
    get type() { return this.#type; }
    set type(value) { this.#type = mimeToken(value, 'type'); }
    get subtype() { return this.#subtype; }
    set subtype(value) { this.#subtype = mimeToken(value, 'subtype'); }
    get essence() { return `${this.#type}/${this.#subtype}`; }
    get params() { return this.#params; }
    toString() { const params = this.#params.toString(); return this.essence + (params ? ';' + params : ''); }
    toJSON() { return this.toString(); }
  }
  const debugs = {};
  const util = {
    promisify, inspect, format, formatWithOptions, types, TextEncoder, TextDecoder, parseArgs,
    callbackify: fn => function(...args) { const callback = args.pop(); Promise.resolve().then(() => fn.apply(this, args)).then(value => queueMicrotask(() => callback(null, value)), reason => queueMicrotask(() => callback(reason || Object.assign(new Error('Promise was rejected with a falsy value'), { code: 'ERR_FALSY_VALUE_REJECTION', reason })))); },
    inherits: (constructor, parent) => { Object.defineProperty(constructor, 'super_', { value: parent, writable: true, configurable: true }); Object.setPrototypeOf(constructor.prototype, parent.prototype); },
    isDeepStrictEqual: (a, b) => deepEqual(a, b, true),
    deprecate: (fn, message, code) => { let warned = false; return function(...args) { if (!warned) { warned = true; globalThis.process?.emitWarning(message, 'DeprecationWarning', code); } return new.target ? Reflect.construct(fn, args, new.target) : fn.apply(this, args); }; },
    debuglog: (section, callback) => {
      const enabled = debugs[section] ??= new RegExp(`^(?:${(globalThis.process?.env.NODE_DEBUG ?? '').replace(/[|\\{}()[\]^$+?.]/g, '\\$&').replace(/\*/g, '.*').replace(/,/g, '|')})$`, 'i').test(section) && !!globalThis.process?.env.NODE_DEBUG;
      const log = (...args) => { if (enabled) console.error(`${section.toUpperCase()} ${info.pid}: ${format(...args)}`); };
      callback?.(log);
      return Object.defineProperty(log, 'enabled', { get: () => enabled });
    },
    stripVTControlCharacters: text => String(text).replace(/[\u001B\u009B][[\]()#;?]*(?:(?:(?:(?:;[-a-zA-Z\d/#&.:=?%@~_]+)*|[a-zA-Z\d]+(?:;[-a-zA-Z\d/#&.:=?%@~_]*)*)?\u0007)|(?:(?:\d{1,4}(?:;\d{0,4})*)?[\dA-PR-TZcf-ntqry=><~]))/g, ''),
    styleText(format, text, { validateStream = true, stream = globalThis.process?.stdout } = {}) {
      const formats = Array.isArray(format) ? format : [format];
      for (const name of formats) if (name !== 'none' && !colors[name]) throw invalid('format', name);
      const env = globalThis.process?.env ?? {};
      if (validateStream && !(env.FORCE_COLOR ? env.FORCE_COLOR !== '0' : stream?.isTTY && !('NO_COLOR' in env) && !('NODE_DISABLE_COLORS' in env) && env.TERM !== 'dumb')) return text;
      let left = '', right = '';
      for (const name of formats) if (name !== 'none') { left += `\u001b[${colors[name][0]}m`; right = `\u001b[${colors[name][1]}m` + right; }
      return left + text + right;
    },
    aborted: (signal, _resource) => new Promise(resolve => signal.aborted ? resolve() : signal.addEventListener('abort', () => resolve(), { once: true })),
    getSystemErrorName: errno => systemError(errno)[0],
    getSystemErrorMessage: errno => systemError(errno)[1],
    getSystemErrorMap: () => new Map(Object.entries(constants.errno).filter(([name]) => messages[name]).map(([name, errno]) => [-errno, [name, messages[name]]])),
    toUSVString: value => `${value}`.toWellFormed(),
    transferableAbortSignal: signal => { if (!(signal instanceof AbortSignal)) throw typeError('signal', 'an instance of AbortSignal', signal); return signal; },
    transferableAbortController: () => new AbortController(),
    MIMEType, MIMEParams,
  };
  util.debug = util.debuglog;

  const domain = (value, unicode) => {
    value = `${value}`;
    if (!value) return '';
    const a = new URL('ws://x'), b = new URL('ws://y');
    a.hostname = b.hostname = value;
    if (a.hostname !== b.hostname) return '';
    return !unicode ? a.hostname : a.hostname.split('.').map(label => {
      if (!label.startsWith('xn--')) return label;
      try {
        const input = label.slice(4), basic = input.lastIndexOf('-'), output = basic > 0 ? [...input.slice(0, basic)].map(c => c.charCodeAt(0)) : [];
        let n = 128, i = 0, bias = 72;
        for (let at = basic > 0 ? basic + 1 : 0; at < input.length;) {
          const old = i;
          for (let w = 1, k = 36; ; k += 36) {
            const c = input.charCodeAt(at++), digit = c >= 48 && c < 58 ? c - 22 : c >= 65 && c < 91 ? c - 65 : c >= 97 && c < 123 ? c - 97 : 36;
            if (digit === 36 || at > input.length) throw 0;
            i += digit * w;
            const t = k <= bias ? 1 : k >= bias + 26 ? 26 : k - bias;
            if (digit < t) break;
            w *= 36 - t;
          }
          const length = output.length + 1;
          let delta = old === 0 ? Math.floor((i - old) / 700) : (i - old) >> 1, k = 0;
          for (delta += Math.floor(delta / length); delta > 455; k += 36) delta = Math.floor(delta / 35);
          bias = Math.floor(k + 36 * delta / (delta + 38));
          n += Math.floor(i / length); i %= length;
          output.splice(i++, 0, n);
        }
        return String.fromCodePoint(...output);
      } catch { return label; }
    }).join('.');
  };
  const url = {
    URL, URLSearchParams,
    fileURLToPath(value) {
      const url = typeof value === 'string' ? new URL(value) : value;
      if (url.protocol === 'apps:') return path.join(info.appPath, decodeURIComponent(url.pathname));
      if (url.protocol !== 'file:') throw Object.assign(new TypeError('The URL must be of scheme file'), { code: 'ERR_INVALID_URL_SCHEME' });
      if (/%2f/i.test(url.pathname) || (windows && /%5c/i.test(url.pathname))) throw Object.assign(new TypeError(`File URL path must not include encoded ${windows ? '\\ or /' : '/'} characters`), { code: 'ERR_INVALID_FILE_URL_PATH' });
      const pathname = decodeURIComponent(url.pathname);
      if (!windows) { if (url.hostname && url.hostname !== 'localhost') throw Object.assign(new TypeError(`File URL host must be "localhost" or empty on ${info.platform}`), { code: 'ERR_INVALID_FILE_URL_HOST' }); return pathname; }
      if (url.hostname) return `\\\\${url.hostname}${pathname.replaceAll('/', '\\')}`;
      if (!/^\/[A-Za-z]:/.test(pathname)) throw Object.assign(new TypeError('File URL path must be absolute'), { code: 'ERR_INVALID_FILE_URL_PATH' });
      return pathname.slice(1).replaceAll('/', '\\');
    },
    pathToFileURL(value) {
      let resolved = path.resolve(value);
      if (/[\\/]$/.test(value) && !resolved.endsWith(path.sep)) resolved += '/';
      const url = new URL('file://');
      if (windows && resolved.startsWith('\\\\')) { const [host, ...rest] = resolved.slice(2).split('\\'); url.hostname = host; resolved = '/' + rest.join('/'); }
      else if (windows) resolved = '/' + resolved.replaceAll('\\', '/');
      url.pathname = resolved.replace(/[%\n\r\t#?\\]/g, c => '%' + c.charCodeAt(0).toString(16).toUpperCase().padStart(2, '0'));
      return url;
    },
    format(value, options) {
      if (typeof value === 'string') return value;
      if (value === null || typeof value !== 'object') throw typeError('urlObject', 'one of type object or string', value);
      if (value instanceof URL) {
        const flag = (name, fallback) => options?.[name] == null ? fallback : !!options[name], url = new URL(value.href);
        if (!flag('auth', true)) { url.username = ''; url.password = ''; }
        if (!flag('fragment', true)) url.hash = '';
        if (!flag('search', true)) url.search = '';
        if (!flag('unicode', false) || !url.hostname) return url.href;
        const start = url.href.indexOf(url.hostname, url.protocol.length);
        return url.href.slice(0, start) + domain(url.hostname, true) + url.href.slice(start + url.hostname.length);
      }
      const auth = value.auth ? encodeURIComponent(value.auth).replace(/%3A/gi, ':') + '@' : '', query = value.query !== null && typeof value.query === 'object' ? querystring.stringify(value.query) : '';
      let protocol = value.protocol || '', pathname = (value.pathname || '').replace(/[#?]/g, c => c === '#' ? '%23' : '%3F'), hash = value.hash || '', search = (value.search || (query && '?' + query) || '').replaceAll('#', '%23'), host = '';
      if (protocol && !protocol.endsWith(':')) protocol += ':';
      if (value.host) host = auth + value.host;
      else if (value.hostname) host = auth + (value.hostname.includes(':') && !/^\[.*\]$/.test(value.hostname) ? `[${value.hostname}]` : value.hostname) + (value.port ? ':' + value.port : '');
      if (value.slashes || /^(?:https?|ftp|gopher|file|wss?):$/.test(protocol)) {
        if (value.slashes || host) { if (pathname && pathname[0] !== '/') pathname = '/' + pathname; host = '//' + host; }
        else if (protocol.startsWith('file')) host = '//';
      }
      return protocol + host + pathname + (search && search[0] !== '?' ? '?' : '') + search + (hash && hash[0] !== '#' ? '#' : '') + hash;
    },
    domainToASCII: value => domain(value, false),
    domainToUnicode: value => domain(value, true),
    urlToHttpOptions: u => ({ protocol: u.protocol, hostname: u.hostname.replace(/^\[|\]$/g, ''), hash: u.hash, search: u.search, pathname: u.pathname, path: u.pathname + u.search, href: u.href, ...(u.port ? { port: Number(u.port) } : {}), ...(u.username || u.password ? { auth: `${decodeURIComponent(u.username)}:${decodeURIComponent(u.password)}` } : {}) }),
  };

  class StringDecoder {
    #need = 0; #have = 0; #char = Buffer.alloc(4);
    constructor(encoding) { this.encoding = normalize(encoding); if (!this.encoding) throw error(TypeError, 'ERR_UNKNOWN_ENCODING', `Unknown encoding: ${encoding}`); }
    get lastChar() { return this.#char; }
    get lastNeed() { return this.#need; }
    get lastTotal() { return this.#have + this.#need; }
    write(data) {
      if (typeof data === 'string') return data;
      if (!ArrayBuffer.isView(data)) throw typeError('buf', 'an instance of Buffer, TypedArray, or DataView', data);
      const encoding = this.encoding, char = this.#char, n = data.byteLength;
      data = new Uint8Array(data.buffer, data.byteOffset, n);
      if (encoding === 'ascii' || encoding === 'hex' || encoding === 'latin1') return decode(data, encoding);
      let prefix = '', at = 0;
      if (this.#need) {
        if (encoding === 'utf8') for (let i = 0; i < n && i < this.#need; i++) if ((data[i] & 0xC0) !== 0x80) { this.#need = 0; char.set(data.subarray(0, i), this.#have); this.#have += i; at = i; break; }
        const found = Math.min(n - at, this.#need);
        char.set(data.subarray(at, at + found), this.#have); at += found; this.#need -= found; this.#have += found;
        if (!this.#need) { prefix = decode(char.subarray(0, this.#have), encoding); this.#have = 0; }
      }
      if (at === n) return prefix;
      if (encoding === 'utf8') {
        if (data[n - 1] & 0x80) for (let i = n - 1; ; --i) {
          this.#have++;
          if ((data[i] & 0xC0) === 0x80) { if (this.#have >= 4 || i === at) { this.#have = 0; break; } continue; }
          const need = (data[i] & 0xE0) === 0xC0 ? 2 : (data[i] & 0xF0) === 0xE0 ? 3 : (data[i] & 0xF8) === 0xF0 ? 4 : 0;
          this.#need = need && this.#have < need ? need - this.#have : 0;
          if (!this.#need) this.#have = 0;
          break;
        }
      } else if (encoding === 'utf16le') { if ((n - at) % 2) this.#have = this.#need = 1; else if ((data[n - 1] & 0xFC) === 0xD8) this.#have = this.#need = 2; }
      else if ((this.#have = (n - at) % 3)) this.#need = 3 - this.#have;
      if (this.#have) char.set(data.subarray(n - this.#have), 0);
      return prefix + (n - this.#have > at ? decode(data.subarray(at, n - this.#have), encoding) : '');
    }
    end(data) {
      let text = data === undefined ? '' : this.write(data);
      if (this.#have && this.encoding === 'utf16le' && this.#have % 2) { this.#need--; this.#have--; }
      if (this.#have) { text += decode(this.#char.subarray(0, this.#have), this.encoding); this.#need = this.#have = 0; }
      return text;
    }
  }

  class AssertionError extends Error {
    constructor({ message, actual, expected, operator } = {}) { super(message ?? `${inspect(actual)} ${operator} ${inspect(expected)}`); Object.assign(this, { name: 'AssertionError', code: 'ERR_ASSERTION', actual, expected, operator, generatedMessage: message === undefined }); }
  }
  const fail = (actual, expected, message, operator, generated, flag = true) => { if (message instanceof Error) throw message; const failure = new AssertionError({ message: message ?? generated, actual, expected, operator }); failure.generatedMessage = message === undefined && flag; throw failure; };
  const differ = (title, actual, expected) => `${title}\n\n${inspect(actual)}\n\nshould ${title.includes('not') ? 'not ' : ''}equal\n\n${inspect(expected)}\n`;
  const matches = (error, expected, message, operator) => {
    if (expected === undefined) return;
    if (typeof expected === 'function') {
      if (expected.prototype !== undefined && error instanceof expected) return;
      if (expected === Error || Error.isPrototypeOf(expected)) fail(error, expected, message, operator, `The error is expected to be an instance of "${expected.name}". Received "${error?.constructor?.name}"`);
      if (expected.call({}, error) !== true) fail(error, expected, message, operator, 'The validation function is expected to return "true". Received false');
      return;
    }
    if (expected instanceof RegExp) { if (!expected.test(String(error))) fail(error, expected, message, operator, `The input did not match the regular expression ${expected}. Input:\n\n${inspect(String(error))}\n`); return; }
    for (const name of Object.keys(expected)) { const want = expected[name], got = error?.[name]; if (want instanceof RegExp && typeof got === 'string' ? !want.test(got) : !deepEqual(got, want, true)) fail(error, expected, message, operator, differ(`Expected values to be strictly deep-equal (property ${name}):`, got, want)); }
  };
  const assert = (value, message) => assert.ok(value, message);
  Object.assign(assert, {
    AssertionError,
    ok(...args) { if (!args[0]) fail(args[0], true, args[1], '==', args.length ? `The expression evaluated to a falsy value:\n\n  assert.ok(${inspect(args[0])})\n` : 'No value argument passed to `assert.ok()`'); },
    fail(message) { fail(undefined, undefined, message, 'fail', 'Failed'); },
    equal(actual, expected, message) { if (!(actual == expected || (actual !== actual && expected !== expected))) fail(actual, expected, message, '==', `${inspect(actual)} == ${inspect(expected)}`); },
    notEqual(actual, expected, message) { if (actual == expected || (actual !== actual && expected !== expected)) fail(actual, expected, message, '!=', `${inspect(actual)} != ${inspect(expected)}`); },
    strictEqual(actual, expected, message) { if (!Object.is(actual, expected)) fail(actual, expected, message, 'strictEqual', `Expected values to be strictly equal:\n\n${inspect(actual)} !== ${inspect(expected)}\n`); },
    notStrictEqual(actual, expected, message) { if (Object.is(actual, expected)) fail(actual, expected, message, 'notStrictEqual', `Expected "actual" to be strictly unequal to: ${inspect(expected)}`); },
    deepEqual(actual, expected, message) { if (!deepEqual(actual, expected, false)) fail(actual, expected, message, 'deepEqual', differ('Expected values to be loosely deep-equal:', actual, expected)); },
    notDeepEqual(actual, expected, message) { if (deepEqual(actual, expected, false)) fail(actual, expected, message, 'notDeepEqual', differ('Expected "actual" not to be loosely deep-equal to:', actual, expected)); },
    deepStrictEqual(actual, expected, message) { if (!deepEqual(actual, expected, true)) fail(actual, expected, message, 'deepStrictEqual', differ('Expected values to be strictly deep-equal:', actual, expected)); },
    notDeepStrictEqual(actual, expected, message) { if (deepEqual(actual, expected, true)) fail(actual, expected, message, 'notDeepStrictEqual', differ('Expected "actual" not to be strictly deep-equal to:', actual, expected)); },
    match(value, regexp, message) { if (typeof value !== 'string' || !regexp.test(value)) fail(value, regexp, message, 'match', `The input did not match the regular expression ${regexp}. Input:\n\n${inspect(value)}\n`); },
    doesNotMatch(value, regexp, message) { if (typeof value !== 'string' || regexp.test(value)) fail(value, regexp, message, 'doesNotMatch', `The input was expected to not match the regular expression ${regexp}. Input:\n\n${inspect(value)}\n`); },
    throws(fn, expected, message) { if (typeof expected === 'string') { message = expected; expected = undefined; } try { fn(); } catch (error) { matches(error, expected, message, 'throws'); return; } fail(undefined, expected, message, 'throws', `Missing expected exception${expected?.name ? ` (${expected.name})` : ''}.`, false); },
    async rejects(input, expected, message) { if (typeof expected === 'string') { message = expected; expected = undefined; } try { await (typeof input === 'function' ? input() : input); } catch (error) { matches(error, expected, message, 'rejects'); return; } fail(undefined, expected, message, 'rejects', `Missing expected rejection${expected?.name ? ` (${expected.name})` : ''}.`, false); },
    doesNotThrow(fn, expected, message) { if (typeof expected === 'string') message = expected; try { fn(); } catch (error) { fail(error, expected, message, 'doesNotThrow', `Got unwanted exception.\nActual message: "${error?.message}"`, false); } },
    async doesNotReject(input, expected, message) { if (typeof expected === 'string') message = expected; try { await (typeof input === 'function' ? input() : input); } catch (error) { fail(error, expected, message, 'doesNotReject', `Got unwanted rejection.\nActual message: "${error?.message}"`, false); } },
    ifError(value) { if (value !== null && value !== undefined) throw value instanceof Error ? value : new AssertionError({ message: `ifError got unwanted exception: ${inspect(value)}`, actual: value, expected: null, operator: 'ifError' }); },
  });
  const strict = (value, message) => assert.ok(value, message);
  Object.assign(strict, assert, { equal: assert.strictEqual, notEqual: assert.notStrictEqual, deepEqual: assert.deepStrictEqual, notDeepEqual: assert.notDeepStrictEqual });
  assert.strict = strict.strict = strict;

  const querystring = {
    escape: value => { try { return encodeURIComponent(value); } catch { throw error(URIError, 'ERR_INVALID_URI', 'URI malformed'); } },
    unescape: (value, spaces) => { try { return decodeURIComponent(value); } catch { return querystring.unescapeBuffer(value, spaces).toString(); } },
    unescapeBuffer(text, spaces) {
      const out = Buffer.allocUnsafe(text.length), hex = c => c >= 48 && c <= 57 ? c - 48 : (c | 32) >= 97 && (c | 32) <= 102 ? (c | 32) - 87 : -1;
      let at = 0;
      for (let index = 0; index < text.length; index++) {
        let c = text.charCodeAt(index);
        if (c === 43 && spaces) c = 32;
        else if (c === 37 && index < text.length - 2) { const high = hex(text.charCodeAt(index + 1)), low = hex(text.charCodeAt(index + 2)); if (high >= 0 && low >= 0) { c = high * 16 + low; index += 2; } }
        out[at++] = c;
      }
      return out.subarray(0, at);
    },
    stringify(value, separator, equals, options) {
      if (value === null || typeof value !== 'object') return '';
      separator ||= '&'; equals ||= '=';
      const escape = typeof options?.encodeURIComponent === 'function' ? options.encodeURIComponent : querystring.escape;
      const text = item => typeof item === 'string' || typeof item === 'bigint' || typeof item === 'boolean' || (typeof item === 'number' && Number.isFinite(item)) ? escape(String(item)) : '';
      return Object.keys(value).map(name => { const item = value[name], prefix = escape(name) + equals; return Array.isArray(item) ? item.map(entry => prefix + text(entry)).join(separator) : prefix + text(item); }).filter(Boolean).join(separator);
    },
    parse(text, separator, equals, options) {
      const result = { __proto__: null };
      if (typeof text !== 'string' || !text) return result;
      separator = separator ? String(separator) : '&'; equals = equals ? String(equals) : '=';
      const maxKeys = typeof options?.maxKeys === 'number' ? options.maxKeys : 1000, decode = typeof options?.decodeURIComponent === 'function' ? options.decodeURIComponent : querystring.unescape;
      const unescape = value => { try { return decode(value); } catch { return querystring.unescape(value, true); } };
      let parts = text.split(separator);
      if (maxKeys > 0) parts = parts.slice(0, maxKeys);
      for (const part of parts) {
        if (!part) continue;
        const index = part.indexOf(equals), name = unescape((index >= 0 ? part.slice(0, index) : part).replace(/\+/g, ' ')), value = index >= 0 ? unescape(part.slice(index + equals.length).replace(/\+/g, ' ')) : '';
        if (!(name in result)) result[name] = value; else if (Array.isArray(result[name])) result[name].push(value); else result[name] = [result[name], value];
      }
      return result;
    },
  };
  querystring.decode = querystring.parse; querystring.encode = querystring.stringify;

  let resultOrder = 'verbatim';
  const family = value => ({ IPv4: 4, IPv6: 6 })[value] ?? Number(value ?? 0);
  const resolve = async (hostname, o) => {
    let list;
    try { list = await asyncCall('lookup', String(hostname), family(o.family)); } catch (error) { throw Object.assign(error, { message: `getaddrinfo ENOTFOUND ${hostname}`, code: 'ENOTFOUND', errno: undefined, syscall: 'getaddrinfo', hostname }); }
    const order = o.order ?? (o.verbatim === false ? 'ipv4first' : resultOrder);
    return order === 'verbatim' ? list : list.sort((a, b) => order === 'ipv4first' ? a.family - b.family : b.family - a.family);
  };
  const rrtypes = { A: 1, AAAA: 28, ANY: 255, CAA: 257, CNAME: 5, MX: 15, NAPTR: 35, NS: 2, PTR: 12, SOA: 6, SRV: 33, TLSA: 52, TXT: 16 };
  const fields = { A: ['address', 'ttl'], AAAA: ['address', 'ttl'], CNAME: ['value'], NS: ['value'], PTR: ['value'], MX: ['exchange', 'priority'], TXT: ['entries'], SRV: ['name', 'port', 'priority', 'weight'], SOA: ['nsname', 'hostmaster', 'serial', 'refresh', 'retry', 'expire', 'minttl'], NAPTR: ['flags', 'service', 'regexp', 'replacement', 'order', 'preference'], TLSA: ['certUsage', 'selector', 'match', 'data'] };
  const shape = (type, records, o) => {
    const list = records.map(([kind, ...values]) => {
      const record = kind === 'CAA' ? { critical: values[0], [values[1]]: values[2] } : Object.fromEntries(fields[kind].map((key, i) => [key, values[i]]));
      if (kind === 'TLSA') record.data = record.data.buffer.slice(record.data.byteOffset, record.data.byteOffset + record.data.byteLength);
      return type === 'ANY' ? Object.assign(record, { type: kind }) : record;
    });
    if (type === 'A' || type === 'AAAA') return o?.ttl ? list : list.map(record => record.address);
    if (type === 'CNAME' || type === 'NS' || type === 'PTR' || type === 'TXT') return list.map(record => record.value ?? record.entries);
    return type === 'SOA' ? list[0] : list;
  };
  const systemServers = () => { try { return [...files.readFileSync('/etc/resolv.conf', 'utf8').matchAll(/^\s*nameserver\s+([^\s%]+)/gm)].map(match => match[1]); } catch { return []; } };
  // WARNING: queries go through the system resolver library, which takes only IPv4 servers from setServers().
  const PromiseResolver = class Resolver {
    #servers = null; #seconds; #tries; #generation = 0;
    constructor(o = {}) { this.#seconds = o.timeout > 0 ? Math.ceil(o.timeout / 1000) : null; this.#tries = o.tries ?? null; }
    getServers() { return this.#servers ? [...this.#servers] : systemServers(); }
    setServers(list) {
      if (!Array.isArray(list)) throw typeError('servers', 'an instance of Array', list);
      this.#servers = list.map(server => {
        const text = String(server), [, ip, port] = /^\[(.+)\](?::(\d+))?$/.exec(text) ?? /^([^:]+)(?::(\d+))?$/.exec(text) ?? [, text], kind = call('web.tcp.ip', ip);
        if (!kind) throw error(TypeError, 'ERR_INVALID_IP_ADDRESS', `Invalid IP address: ${text}`);
        if (kind === 6) throw error(Error, 'ERR_FEATURE_UNAVAILABLE_ON_PLATFORM', 'IPv6 DNS servers are not supported');
        return port && port !== '53' ? `${ip}:${port}` : ip;
      });
    }
    cancel() { this.#generation++; }
    #query(hostname, type, reverse) {
      if (typeof hostname !== 'string') throw typeError(reverse ? 'ip' : 'name', 'of type string', hostname);
      const syscall = reverse ? 'getHostByAddr' : 'query' + type[0] + type.slice(1).toLowerCase(), generation = this.#generation;
      const fail = code => Object.assign(new Error(`${syscall} ${code} ${hostname}`), { errno: undefined, code, syscall, hostname });
      return asyncCall('resolve', hostname, rrtypes[type], this.#servers?.map(server => server.includes(':') ? server : server + ':53') ?? null, this.#seconds, this.#tries, !!reverse)
        .then(list => { if (generation !== this.#generation) throw fail('ECANCELLED'); return list; }, failure => { throw fail(generation !== this.#generation ? 'ECANCELLED' : /^E[A-Z]+$/.test(failure.message) ? failure.message : 'EBADRESP'); });
    }
    resolve(hostname, rrtype = 'A') { if (!Object.hasOwn(rrtypes, rrtype)) throw error(TypeError, 'ERR_INVALID_ARG_VALUE', `The argument 'rrtype' is invalid. Received '${rrtype}'`); return this.#query(hostname, rrtype).then(list => shape(rrtype, list)); }
    async reverse(ip) { return shape('PTR', await this.#query(ip, 'PTR', true)); }
    static { for (const type of Object.keys(rrtypes)) this.prototype['resolve' + (type === 'A' ? '4' : type === 'AAAA' ? '6' : type[0] + type.slice(1).toLowerCase())] = async function(hostname, o) { return shape(type, await this.#query(hostname, type), o); }; }
  };
  class Resolver extends PromiseResolver {}
  const methods = ['resolve', 'reverse', ...Object.keys(rrtypes).map(type => 'resolve' + (type === 'A' ? '4' : type === 'AAAA' ? '6' : type[0] + type.slice(1).toLowerCase()))];
  for (const method of methods) Resolver.prototype[method] = function(...args) { const callback = args.pop(); if (typeof callback !== 'function') throw typeError('callback', 'of type function', callback); PromiseResolver.prototype[method].apply(this, args).then(value => callback(null, value), callback); };
  const resolver = new Resolver(), order = {
    getDefaultResultOrder: () => resultOrder,
    setDefaultResultOrder(value) { if (!['ipv4first', 'ipv6first', 'verbatim'].includes(value)) throw error(TypeError, 'ERR_INVALID_ARG_VALUE', `The argument 'order' must be one of: 'verbatim', 'ipv4first', 'ipv6first'. Received '${value}'`); resultOrder = value; },
    getServers: () => resolver.getServers(), setServers: list => resolver.setServers(list),
    ...Object.fromEntries('NODATA FORMERR SERVFAIL NOTFOUND NOTIMP REFUSED BADQUERY BADNAME BADFAMILY BADRESP CONNREFUSED TIMEOUT EOF FILE NOMEM DESTRUCTION BADSTR BADFLAGS NONAME BADHINTS NOTINITIALIZED LOADIPHLPAPI ADDRGETNETWORKPARAMS CANCELLED'.split(' ').map(code => [code, 'E' + code])),
  };
  const dns = {
    ...order, Resolver, ADDRCONFIG: 1024, V4MAPPED: 2048, ALL: 256,
    ...Object.fromEntries(methods.map(method => [method, Resolver.prototype[method].bind(resolver)])),
    lookup(hostname, o, callback) { if (typeof o === 'function') { callback = o; o = {}; } o = typeof o === 'object' && o !== null ? o : { family: o }; resolve(hostname, o).then(list => o.all ? callback(null, list) : callback(null, list[0].address, list[0].family), callback); },
    promises: { ...order, Resolver: PromiseResolver, ...Object.fromEntries(methods.map(method => [method, PromiseResolver.prototype[method].bind(resolver)])), lookup: async (hostname, o = {}) => { o = typeof o === 'object' && o !== null ? o : { family: o }; const list = await resolve(hostname, o); return o.all ? list : list[0]; } },
  };
  dns.lookup[promisify.custom] = dns.promises.lookup;

  const colorDepth = (env = globalThis.process.env) => {
    if (env.FORCE_COLOR !== undefined) return ['', '1', 'true'].includes(env.FORCE_COLOR) ? 4 : env.FORCE_COLOR === '2' ? 8 : env.FORCE_COLOR === '3' ? 24 : 1;
    if (env.NODE_DISABLE_COLORS !== undefined || env.NO_COLOR !== undefined || env.TERM === 'dumb') return 1;
    if (windows || env.TMUX) return 24;
    if (env.CI) return ['APPVEYOR', 'BUILDKITE', 'CIRCLECI', 'DRONE', 'GITHUB_ACTIONS', 'GITLAB_CI', 'TRAVIS'].some(name => name in env) || env.CI_NAME === 'codeship' ? 8 : 1;
    if (env.TERM_PROGRAM === 'iTerm.app') return !env.TERM_PROGRAM_VERSION || /^[0-2]\./.test(env.TERM_PROGRAM_VERSION) ? 8 : 24;
    if (env.TERM_PROGRAM === 'HyperTerm' || env.TERM_PROGRAM === 'MacTerm') return 24;
    if (env.TERM_PROGRAM === 'Apple_Terminal') return 8;
    if (env.COLORTERM === 'truecolor' || env.COLORTERM === '24bit' || /truecolor|^(?:mosh|rxvt-unicode-24bit|terminator|xterm-kitty)$/i.test(env.TERM)) return 24;
    if (/^xterm-256/.test(env.TERM)) return 8;
    return env.COLORTERM || /ansi|color|linux|direct|^con\d*x\d|^rxvt|^screen|^xterm|^vt[12][02]0|^(?:eterm|cons25|console|cygwin|dtterm|gnome|hurd|jfbterm|konsole|kterm|mlterm|putty|st)$/i.test(env.TERM ?? '') ? 4 : 1;
  };
  let ttyStreams;
  const ttyStream = name => (ttyStreams ??= (() => {
    const { Readable, Writable } = streams ?? globalThis.__apps.stream, unsupported = message => error(Error, 'ERR_TTY_INIT_FAILED', `TTY initialization failed: ${message}`);
    function ReadStream(fd) { throw unsupported(tty.isatty(fd) ? 'reading a terminal is not supported' : 'uv_tty_init returned EINVAL (invalid argument)'); }
    function WriteStream(fd) {
      if (!(this instanceof WriteStream)) return new WriteStream(fd);
      if ((fd !== 1 && fd !== 2) || !tty.isatty(fd)) throw unsupported('uv_tty_init returned EINVAL (invalid argument)');
      Writable.call(this, { write(data, _encoding, done) { call('process.write', fd, data); done(); } });
      Object.assign(this, { fd, isTTY: true });
      resizable(this);
    }
    for (const [type, base] of [[ReadStream, Readable], [WriteStream, Writable]]) { Object.setPrototypeOf(type.prototype, base.prototype); Object.setPrototypeOf(type, base); }
    Object.assign(WriteStream.prototype, {
      getColorDepth: colorDepth,
      hasColors(count, env) { if (env === undefined && (count === undefined || (typeof count === 'object' && count !== null))) { env = count; count = 16; } else integer(count, 'count', 2); return count <= 2 ** colorDepth(env); },
      getWindowSize() { return sync('tty.size', this.fd) ?? [80, 24]; },
      clearLine(direction, callback) { return this.write(direction < 0 ? '\x1b[1K' : direction > 0 ? '\x1b[0K' : '\x1b[2K', callback); },
      clearScreenDown(callback) { return this.write('\x1b[0J', callback); },
      cursorTo(x, y, callback) { if (typeof y === 'function') { callback = y; y = undefined; } return this.write(typeof y === 'number' ? `\x1b[${y + 1};${x + 1}H` : `\x1b[${x + 1}G`, callback); },
      moveCursor(dx, dy, callback) { return this.write((dx < 0 ? `\x1b[${-dx}D` : dx > 0 ? `\x1b[${dx}C` : '') + (dy < 0 ? `\x1b[${-dy}A` : dy > 0 ? `\x1b[${dy}B` : ''), callback); },
    });
    for (const [index, name] of ['columns', 'rows'].entries()) Object.defineProperty(WriteStream.prototype, name, { configurable: true, enumerable: true, get() { return this.getWindowSize()[index]; } });
    return { ReadStream, WriteStream };
  })())[name];
  const tty = { isatty: fd => Number.isInteger(fd) && sync('isatty', fd), get ReadStream() { return ttyStream('ReadStream'); }, get WriteStream() { return ttyStream('WriteStream'); } };
  // WARNING: SIGWINCH is hooked only when a terminal stream gets its first `resize` listener.
  const resizing = new Set(), resizable = stream => stream.on('newListener', name => { if (name !== 'resize' || resizing.has(stream)) return; if (!resizing.size) sync('tty.watch'); resizing.add(stream); });
  let streams;
  // WARNING: makes process.stdout and stderr terminal streams. They are created before `__apps.stream` exists.
  const terminal = (stream, modules) => { streams = modules; return resizable(Object.setPrototypeOf(stream, tty.WriteStream.prototype)); };
  return { Buffer, buffer, fs: files, path, os, util, url, string_decoder: { StringDecoder }, assert, querystring, dns, tty, details, sync, asyncCall, poll, terminal };
};
