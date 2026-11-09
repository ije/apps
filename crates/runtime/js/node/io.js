globalThis.__appsNodeIO = ({ call, sync, asyncCall, Buffer, stream, EventEmitter, StringDecoder, webcrypto, inspect, codec }) => {
  const input = (value, encoding) => typeof value === 'string' ? Buffer.from(value, encoding) : ArrayBuffer.isView(value) ? new Uint8Array(value.buffer, value.byteOffset, value.byteLength) : value instanceof ArrayBuffer ? new Uint8Array(value) : (() => { throw Object.assign(new TypeError('The "data" argument must be of type string or an instance of Buffer, TypedArray, or DataView'), { code: 'ERR_INVALID_ARG_TYPE' }); })();
  const output = data => Buffer.from(data.buffer, data.byteOffset, data.byteLength);
  const encoded = (data, encoding) => encoding && encoding !== 'buffer' ? data.toString(encoding) : data;

  const zlibError = error => Object.assign(error, /^Cannot create a Buffer/.test(error.message) ? { code: 'ERR_BUFFER_TOO_LARGE' } : /^unexpected end of file/.test(error.message) ? { code: 'Z_BUF_ERROR', errno: -5 } : { code: 'Z_DATA_ERROR', errno: -3 });
  const zlib = { constants: { Z_NO_FLUSH: 0, Z_PARTIAL_FLUSH: 1, Z_SYNC_FLUSH: 2, Z_FULL_FLUSH: 3, Z_FINISH: 4, Z_OK: 0, Z_STREAM_END: 1, Z_NEED_DICT: 2, Z_ERRNO: -1, Z_STREAM_ERROR: -2, Z_DATA_ERROR: -3, Z_BUF_ERROR: -5, Z_NO_COMPRESSION: 0, Z_BEST_SPEED: 1, Z_BEST_COMPRESSION: 9, Z_DEFAULT_COMPRESSION: -1, Z_FILTERED: 1, Z_HUFFMAN_ONLY: 2, Z_RLE: 3, Z_FIXED: 4, Z_DEFAULT_STRATEGY: 0 } };
  const codecs = new __appsFinalizers(id => sync('zlib.free', id)), flushes = new WeakMap();
  class Zlib extends stream.Transform {
    #format; #inflate; #options; #id = null; #ended = false;
    constructor(format, inflate, o = {}) {
      super({ ...o, objectMode: false, readableObjectMode: false, writableObjectMode: false });
      this.#format = format; this.#inflate = inflate; this.#options = o; this.bytesWritten = 0;
      this._defaultFlushFlag = o.flush ?? zlib.constants.Z_NO_FLUSH; this._finishFlushFlag = o.finishFlush ?? zlib.constants.Z_FINISH;
    }
    #write(chunk, flush) {
      if (this.#id === null) {
        if (!chunk.length && this.#format === 'unzip' && flush !== zlib.constants.Z_FINISH) return null;
        const kind = this.#format === 'unzip' ? (chunk[0] === 0x1f && chunk[1] === 0x8b ? 'gzip' : 'deflate') : this.#format, opts = this.#options;
        this.#id = sync('zlib.new', kind, this.#inflate, opts.level ?? -1, opts.windowBits ?? 15, opts.dictionary ? input(opts.dictionary) : null);
        codecs.register(this, this.#id, this);
      }
      try {
        const result = sync('zlib.write', this.#id, chunk, flush);
        this.#ended = result.ended;
        return result.bytes.length ? output(result.bytes) : null;
      } catch (error) { throw zlibError(error); }
    }
    _transform(chunk, _encoding, callback) {
      let data;
      try { this.bytesWritten += chunk.length; data = this.#write(chunk, flushes.get(chunk) ?? this._defaultFlushFlag); } catch (error) { return callback(error); }
      callback(null, data);
    }
    _flush(callback) {
      let data;
      try {
        data = this.#write(Buffer.alloc(0), this._finishFlushFlag);
        if (this.#inflate && !this.#ended) throw Object.assign(new Error('unexpected end of file'), { code: 'Z_BUF_ERROR', errno: -5 });
      } catch (error) { return callback(error); }
      callback(null, data);
    }
    _destroy(error, callback) { if (this.#id !== null) { codecs.unregister(this); sync('zlib.free', this.#id); this.#id = null; } callback(error); }
    flush(kind = zlib.constants.Z_FULL_FLUSH, callback) {
      if (typeof kind === 'function') [kind, callback] = [zlib.constants.Z_FULL_FLUSH, kind];
      if (this.writableEnded) { if (callback) queueMicrotask(callback); return; }
      const marker = Buffer.alloc(0);
      flushes.set(marker, kind);
      this.write(marker, () => callback?.());
    }
    params(level, _strategy, callback) { this.flush(zlib.constants.Z_SYNC_FLUSH, () => { if (this.#id !== null) sync('zlib.params', this.#id, level); else this.#options = { ...this.#options, level }; callback?.(); }); }
    reset() { if (this.#id !== null) { codecs.unregister(this); sync('zlib.free', this.#id); this.#id = null; } this.#ended = false; }
    close(callback) { if (callback) { if (this.closed) queueMicrotask(callback); else this.once('close', callback); } this.destroy(); }
  }
  let table;
  zlib.crc32 = (data, value = 0) => {
    table ??= Int32Array.from({ length: 256 }, (_, n) => { for (let k = 0; k < 8; k++) n = n & 1 ? 0xEDB88320 ^ (n >>> 1) : n >>> 1; return n; });
    const bytes = input(data, 'utf8');
    let crc = ~value;
    for (let i = 0; i < bytes.length; i++) crc = table[(crc ^ bytes[i]) & 255] ^ (crc >>> 8);
    return ~crc >>> 0;
  };
  for (const [name, format, inflate] of [['Gzip', 'gzip', false], ['Gunzip', 'gzip', true], ['Deflate', 'deflate', false], ['Inflate', 'deflate', true], ['DeflateRaw', 'deflate-raw', false], ['InflateRaw', 'deflate-raw', true], ['Unzip', 'unzip', true]]) {
    const method = name[0].toLowerCase() + name.slice(1), args = (data, o) => [format, inflate, input(data), o?.level ?? -1, o?.maxOutputLength];
    zlib[method + 'Sync'] = (data, o) => { try { return output(sync('zlib', ...args(data, o))); } catch (error) { throw zlibError(error); } };
    zlib[method] = (data, o, callback) => { if (typeof o === 'function') { callback = o; o = {}; } asyncCall('zlib', ...args(data, o)).then(result => callback(null, output(result)), error => callback(zlibError(error))); };
    zlib[name] = { [name]: class extends Zlib { constructor(o) { super(format, inflate, o); } } }[name];
    zlib['create' + name] = o => new zlib[name](o);
  }

  class Interface extends EventEmitter {
    #rest = []; #cr = false; #decoder = new StringDecoder('utf8'); #questions = []; #prompt;
    #data = chunk => {
      const lines = ((this.#cr ? '\r' : '') + (typeof chunk === 'string' ? chunk : this.#decoder.write(chunk))).split(/\r\n|\n|\r(?!$)/);
      let last = lines.pop();
      this.#cr = last.endsWith('\r');
      if (this.#cr) last = last.slice(0, -1);
      if (lines.length) { lines[0] = this.#rest.join('') + lines[0]; this.#rest = []; }
      if (last) this.#rest.push(last);
      for (const line of lines) this.#line(line);
    };
    #end = () => { const rest = this.#rest.join('') + (this.#cr ? '\r' : '') + this.#decoder.end(); this.#rest = []; this.#cr = false; if (rest) this.#line(rest.replace(/\r$/, '')); this.close(); };
    constructor(input, output, _completer, terminal) {
      super();
      const options = input?.input ? input : { input, output, terminal };
      Object.assign(this, { input: options.input, output: options.output, terminal: !!options.terminal, line: '', closed: false });
      this.#prompt = options.prompt ?? '> ';
      this.input.on('data', this.#data);
      this.input.on('end', this.#end);
      options.signal?.addEventListener('abort', () => this.close(), { once: true });
    }
    #line(line) { const answer = this.#questions.shift(); if (answer) answer(line); else this.emit('line', line); }
    close() { if (this.closed) return; this.closed = true; this.input.off('data', this.#data); this.input.off('end', this.#end); this.input.pause?.(); this.emit('close'); }
    pause() { this.input.pause?.(); this.emit('pause'); return this; }
    resume() { this.input.resume?.(); this.emit('resume'); return this; }
    setPrompt(prompt) { this.#prompt = prompt; }
    getPrompt() { return this.#prompt; }
    prompt() { this.output?.write(this.#prompt); }
    question(query, options, callback) { callback = typeof options === 'function' ? options : callback; if (this.closed) throw Object.assign(new Error('readline was closed'), { code: 'ERR_USE_AFTER_CLOSE' }); this.output?.write(query); this.#questions.push(callback); }
    write(data) { if (!this.closed) this.#data(String(data)); }
    async *[Symbol.asyncIterator]() {
      const forward = error => this.emit('error', error);
      this.input.on('error', forward);
      try { for await (const [line] of EventEmitter.on(this, 'line', { close: ['close'] })) yield line; }
      finally { this.input.off('error', forward); this.close(); }
    }
  }
  class PromiseInterface extends Interface {
    question(query, options = {}) { return new Promise((resolve, reject) => { if (options.signal?.aborted) return reject(Object.assign(new Error('The operation was aborted'), { name: 'AbortError', code: 'ABORT_ERR' })); super.question(query, resolve); }); }
  }
  const terminal = (stream, data, callback) => { if (!stream) { callback?.(); return true; } return stream.write(data, callback); };
  const codes = {
    clearLine: direction => direction < 0 ? '\x1b[1K' : direction > 0 ? '\x1b[0K' : '\x1b[2K',
    cursorTo: (x, y) => typeof y === 'number' ? `\x1b[${y + 1};${x + 1}H` : `\x1b[${x + 1}G`,
    moveCursor: (dx, dy) => (dx < 0 ? `\x1b[${-dx}D` : dx > 0 ? `\x1b[${dx}C` : '') + (dy < 0 ? `\x1b[${-dy}A` : dy > 0 ? `\x1b[${dy}B` : ''),
  };
  class Readline {
    #stream; #auto; #todo = [];
    constructor(stream, { autoCommit = false } = {}) { this.#stream = stream; this.#auto = autoCommit; }
    #add(data) { if (this.#auto) this.#stream.write(data); else this.#todo.push(data); return this; }
    clearLine(direction) { return this.#add(codes.clearLine(direction)); }
    clearScreenDown() { return this.#add('\x1b[0J'); }
    cursorTo(x, y) { return this.#add(codes.cursorTo(x, y)); }
    moveCursor(dx, dy) { return dx || dy ? this.#add(codes.moveCursor(dx, dy)) : this; }
    commit() { const data = this.#todo.splice(0).join(''); return new Promise(resolve => this.#stream.write(data, () => resolve())); }
    rollback() { this.#todo = []; return this; }
  }
  const readline = {
    Interface, createInterface: (...args) => new Interface(...args),
    clearLine: (stream, direction, callback) => terminal(stream, codes.clearLine(direction), callback),
    clearScreenDown: (stream, callback) => terminal(stream, '\x1b[0J', callback),
    cursorTo: (stream, x, y, callback) => { if (typeof y === 'function') { callback = y; y = undefined; } return terminal(stream, codes.cursorTo(x, y), callback); },
    moveCursor: (stream, dx, dy, callback) => terminal(stream, codes.moveCursor(dx, dy), callback),
    emitKeypressEvents() {},
    promises: { Interface: PromiseInterface, Readline, createInterface: (...args) => new PromiseInterface(...args) },
  };

  const hashes = ['md5', 'sha1', 'sha224', 'sha256', 'sha384', 'sha512'];
  const digest = algorithm => { const name = String(algorithm).toLowerCase().replace(/^rsa-/, '').replace('-', ''); if (!hashes.includes(name)) throw new Error('Digest method not supported'); return name; };
  const handles = new __appsFinalizers(id => sync('hash.free', id));
  class Hash extends stream.Transform {
    #algorithm; #secret; #chunks = []; #size = 0; #id = null; #done = false;
    constructor(algorithm, secret, state) {
      super();
      this.#algorithm = digest(algorithm); this.#secret = secret;
      if (state) { this.#chunks = [...state.chunks]; this.#size = state.size; if (state.id !== null) { this.#id = sync('hash.copy', state.id); handles.register(this, this.#id, this); } }
    }
    _transform(chunk, encoding, callback) { this.update(chunk, encoding); callback(); }
    _flush(callback) { callback(null, this.digest()); }
    update(data, encoding) {
      if (this.#done) throw Object.assign(new Error('Digest already called'), { code: 'ERR_CRYPTO_HASH_FINALIZED' });
      // WARNING: chunks wait for digest(), so views are copied: their owner can change them. A string's bytes are new already.
      data = typeof data === 'string' ? input(data, encoding) : Buffer.from(input(data, encoding));
      this.#chunks.push(data); this.#size += data.length;
      if (this.#size >= 65536) {
        if (this.#id === null) { this.#id = sync('hash.new', this.#algorithm, this.#secret); handles.register(this, this.#id, this); }
        sync('hash.update', this.#id, Buffer.concat(this.#chunks)); this.#chunks = []; this.#size = 0;
      }
      return this;
    }
    digest(encoding) {
      if (this.#done) throw Object.assign(new Error('Digest already called'), { code: 'ERR_CRYPTO_HASH_FINALIZED' });
      this.#done = true;
      const data = this.#chunks.length === 1 ? this.#chunks[0] : Buffer.concat(this.#chunks);
      this.#chunks = [];
      let result;
      if (this.#id === null) result = codec.digest(this.#algorithm, data, this.#secret);
      else { if (data.length) sync('hash.update', this.#id, data); result = sync('hash.digest', this.#id); handles.unregister(this); }
      return encoded(output(result), encoding);
    }
    copy() { if (this.#done || this.#secret) throw Object.assign(new Error('Digest already called'), { code: 'ERR_CRYPTO_HASH_FINALIZED' }); return new Hash(this.#algorithm, null, { chunks: this.#chunks, size: this.#size, id: this.#id }); }
  }
  const invalidTag = length => Object.assign(new TypeError(`Invalid authentication tag length: ${length}`), { code: 'ERR_CRYPTO_INVALID_AUTH_TAG' });
  const invalidState = name => Object.assign(new Error(`Invalid state for operation ${name}`), { code: 'ERR_CRYPTO_INVALID_STATE' });
  class Cipher extends stream.Transform {
    #decrypt; #key; #iv; #chunks = []; #aad = new Uint8Array(0); #tag = null; #tagLength = 16; #done = false; #pad = new Uint8Array(0); #size = 0; #decoder = null;
    constructor(decrypt, algorithm, key, iv, options) {
      super();
      const bits = /^aes-(128|256)-gcm$/i.exec(algorithm)?.[1];
      if (!bits) throw Object.assign(new Error('Unknown cipher'), { code: 'ERR_CRYPTO_UNKNOWN_CIPHER' });
      this.#decrypt = decrypt; this.#key = secret(key); this.#iv = input(iv);
      if (this.#key.length * 8 !== Number(bits)) throw Object.assign(new RangeError('Invalid key length'), { code: 'ERR_CRYPTO_INVALID_KEYLEN' });
      if (this.#iv.length !== 12) throw Object.assign(new TypeError('Invalid initialization vector'), { code: 'ERR_CRYPTO_INVALID_IV' });
      // WARNING: ring verifies only 16-byte GCM tags, so a decipher rejects the shorter tags that Node also takes.
      const length = options?.authTagLength;
      if (length !== undefined && !(Number.isInteger(length) && (decrypt ? length === 16 : length === 4 || length === 8 || length >= 12 && length <= 16))) throw invalidTag(length);
      this.#tagLength = length ?? 16;
    }
    _transform(chunk, encoding, callback) { callback(null, this.update(chunk, encoding)); }
    _flush(callback) { try { callback(null, this.final()); } catch (error) { callback(error); } }
    #text(data, encoding) { if (!encoding || encoding === 'buffer') return data; this.#decoder ??= new StringDecoder(encoding); return this.#decoder.write(data); }
    update(data, inputEncoding, outputEncoding) {
      if (this.#done) throw Object.assign(new Error('Unsupported state'), { code: 'ERR_CRYPTO_INVALID_STATE' });
      const bytes = input(data, inputEncoding), end = this.#size + bytes.length;
      // WARNING: GCM encrypts with an AES-CTR keystream that does not depend on the data, so sealing zeros yields it. It grows by doubling, so the total work stays linear.
      if (end > this.#pad.length) { const size = Math.max(end, this.#pad.length * 2, 4096); this.#pad = output(call('web.crypto.encrypt', this.#key, this.#iv, new Uint8Array(0), new Uint8Array(size))).subarray(0, size); }
      const out = Buffer.allocUnsafe(bytes.length);
      for (let i = 0; i < bytes.length; i++) out[i] = bytes[i] ^ this.#pad[this.#size + i];
      this.#chunks.push(Buffer.from(bytes));
      this.#size = end;
      return this.#text(out, outputEncoding);
    }
    final(outputEncoding) {
      if (this.#done) throw Object.assign(new Error('Unsupported state'), { code: 'ERR_CRYPTO_INVALID_STATE' });
      this.#done = true;
      const data = Buffer.concat(this.#chunks), tail = outputEncoding && outputEncoding !== 'buffer' ? this.#decoder?.end() ?? '' : Buffer.alloc(0);
      this.#chunks = []; this.#pad = new Uint8Array(0);
      if (this.#decrypt) {
        if (!this.#tag) throw new Error('Unsupported state or unable to authenticate data');
        try { call('web.crypto.decrypt', this.#key, this.#iv, this.#aad, data, this.#tag); }
        catch { throw new Error('Unsupported state or unable to authenticate data'); }
        return tail;
      }
      const sealed = output(call('web.crypto.encrypt', this.#key, this.#iv, this.#aad, data));
      this.#tag = sealed.subarray(sealed.length - 16);
      return tail;
    }
    setAAD(data) { this.#aad = input(data); return this; }
    getAuthTag() { if (this.#decrypt || !this.#tag) throw invalidState('getAuthTag'); return Buffer.from(this.#tag.subarray(0, this.#tagLength)); }
    setAuthTag(tag) {
      if (!this.#decrypt || this.#tag || this.#done) throw invalidState('setAuthTag');
      tag = input(tag);
      if (tag.length !== this.#tagLength) throw invalidTag(tag.length);
      this.#tag = Buffer.from(tag);
      return this;
    }
    setAutoPadding() { return this; }
  }
  const fail = (code, message, Type = Error) => { throw Object.assign(new Type(message), { code }); };
  const unsupported = () => fail('ERR_OSSL_UNSUPPORTED', 'error:1E08010C:DECODER routines::unsupported');
  // WARNING: DER readers return [tag, value, next]. Key fields are views of a private copy of the input.
  const der = (bytes, at = 0) => {
    let length = bytes[at + 1], start = at + 2;
    if (length > 127) { const count = length & 127; length = 0; for (let i = 0; i < count; i++) length = length * 256 + bytes[start++]; }
    if (!(start + length <= bytes.length)) unsupported();
    return [bytes[at], bytes.subarray(start, start + length), start + length];
  };
  const items = bytes => { const list = []; for (let at = 0; at < bytes.length;) { const [tag, value, next] = der(bytes, at); list.push([tag, value]); at = next; } return list; };
  const tlv = (tag, ...parts) => { const n = parts.reduce((sum, part) => sum + part.length, 0); return Buffer.concat([Buffer.from([tag, ...(n < 128 ? [n] : n < 256 ? [129, n] : n < 65536 ? [130, n >> 8, n & 255] : [131, n >> 16, n >> 8 & 255, n & 255])]), ...parts]); };
  const trim = value => { let i = 0; while (i < value.length - 1 && !value[i]) i++; return value.subarray(i); };
  const pad = (value, size) => value.length < size ? Buffer.concat([Buffer.alloc(size - value.length), value]) : value;
  const int = value => { value = trim(value); return tlv(2, ...(value[0] > 127 ? [Buffer.of(0)] : []), value); };
  const oid = hex => tlv(6, Buffer.from(hex, 'hex'));
  const kinds = { rsa: '2a864886f70d010101', ec: '2a8648ce3d0201', ed25519: '2b6570' }, curves = { 'P-256': ['2a8648ce3d030107', 'prime256v1', 32], 'P-384': ['2b81040022', 'secp384r1', 48] };
  const curveOf = name => Object.keys(curves).find(curve => curve === name || curves[curve][1] === name || curves[curve][0] === name || (curve === 'P-256' && name === 'secp256r1'));
  const point = data => { const size = curves[data.curve]?.[2]; if (!size || data.x.length !== 1 + 2 * size || data.x[0] !== 4 || (data.d && data.d.length !== size)) unsupported(); return data; };
  const rsaKey = (bytes, type) => { const [n, e, d, p, q, dp, dq, qi] = items(der(bytes)[1]).slice(type === 'private' ? 1 : 0).map(([, value]) => trim(value)); if (!e || (type === 'private' && !qi)) unsupported(); return type === 'private' ? { type, kind: 'rsa', n, e, d, p, q, dp, dq, qi } : { type, kind: 'rsa', n, e }; };
  // WARNING: ring cannot derive an EC public key, so private EC keys must carry it, as OpenSSL writes them.
  const ecKey = (bytes, curve) => {
    const list = items(der(bytes)[1]), params = list.find(([tag]) => tag === 0xa0), x = list.find(([tag]) => tag === 0xa1);
    if (params) curve = curveOf(der(params[1])[1].toString('hex'));
    if (!x) fail('ERR_OSSL_UNSUPPORTED', 'EC private keys without a public key are not supported');
    return point({ type: 'private', kind: 'ec', curve, x: der(x[1])[1].subarray(1), d: pad(list[1][1], curves[curve]?.[2]) });
  };
  const spki = bytes => {
    const [[, algorithm], [, bits]] = items(der(bytes)[1]), [[, id], parameter] = items(algorithm), x = bits.subarray(1), kind = id.toString('hex');
    if (kind === kinds.rsa) return rsaKey(x, 'public');
    if (kind === kinds.ec) return point({ type: 'public', kind: 'ec', curve: curveOf(parameter?.[1].toString('hex')), x });
    return kind === kinds.ed25519 && x.length === 32 ? { type: 'public', kind: 'ed25519', x } : unsupported();
  };
  const pkcs8 = bytes => {
    const [, [, algorithm], [, key], ...rest] = items(der(bytes)[1]), [[, id], parameter] = items(algorithm), kind = id.toString('hex');
    if (kind === kinds.rsa) return rsaKey(key, 'private');
    if (kind === kinds.ec) return ecKey(key, curveOf(parameter?.[1].toString('hex')));
    if (kind !== kinds.ed25519) unsupported();
    const d = der(key)[1], x = rest.find(([tag]) => tag === 0x81)?.[1].subarray(1);
    return { type: 'private', kind: 'ed25519', d, x: x ?? output(call('web.crypto.generate', 'ed25519', d).public) };
  };
  const algorithmId = data => tlv(0x30, oid(kinds[data.kind]), ...(data.kind === 'rsa' ? [Buffer.of(5, 0)] : data.kind === 'ec' ? [oid(curves[data.curve][0])] : []));
  const rsaDer = (data, type) => tlv(0x30, ...(type === 'private' ? [Buffer.of(0), data.n, data.e, data.d, data.p, data.q, data.dp, data.dq, data.qi] : [data.n, data.e]).map(int));
  const sec1 = (data, named) => tlv(0x30, int(Buffer.of(1)), tlv(4, data.d), ...(named ? [tlv(0xa0, oid(curves[data.curve][0]))] : []), tlv(0xa1, tlv(3, Buffer.of(0), data.x)));
  const encode = (data, type) => type === 'pkcs1' ? rsaDer(data, data.type) : type === 'sec1' ? sec1(data, true)
    : type === 'spki' ? tlv(0x30, algorithmId(data), tlv(3, Buffer.of(0), data.kind === 'rsa' ? rsaDer(data, 'public') : data.x))
    : tlv(0x30, int(Buffer.of(0)), algorithmId(data), tlv(4, data.kind === 'rsa' ? rsaDer(data, 'private') : data.kind === 'ec' ? sec1(data, false) : tlv(4, data.d)));
  const jwk = data => {
    const field = value => Buffer.from(value).toString('base64url'), d = data.d ? { d: field(data.d) } : {};
    if (data.type === 'secret') return { kty: 'oct', k: field(data.key) };
    if (data.kind === 'ec') { const size = curves[data.curve][2]; return { kty: 'EC', x: field(data.x.subarray(1, 1 + size)), y: field(data.x.subarray(1 + size)), crv: data.curve, ...d }; }
    if (data.kind === 'ed25519') return { crv: 'Ed25519', ...d, x: field(data.x), kty: 'OKP' };
    return Object.fromEntries([['kty', 'RSA'], ...['n', 'e', 'd', 'p', 'q', 'dp', 'dq', 'qi'].filter(name => data[name]).map(name => [name, field(data[name])])]);
  };
  const fromJwk = value => {
    const kty = value?.kty, field = name => typeof value[name] === 'string' ? Buffer.from(value[name], 'base64url') : fail('ERR_CRYPTO_INVALID_JWK', `Invalid JWK ${kty} key`, TypeError);
    const type = value?.d === undefined ? 'public' : 'private', size = curves[value?.crv]?.[2];
    if (kty === 'oct') return { type: 'secret', key: field('k') };
    if (kty === 'RSA') return type === 'private' ? { type, kind: 'rsa', ...Object.fromEntries(['n', 'e', 'd', 'p', 'q', 'dp', 'dq', 'qi'].map(name => [name, trim(field(name))])) } : { type, kind: 'rsa', n: trim(field('n')), e: trim(field('e')) };
    if (kty === 'EC' && size) return point({ type, kind: 'ec', curve: value.crv, x: Buffer.concat([Buffer.of(4), pad(field('x'), size), pad(field('y'), size)]), ...(type === 'private' ? { d: pad(field('d'), size) } : {}) });
    if (kty === 'OKP' && value.crv === 'Ed25519') { const x = field('x'); if (x.length !== 32 || (type === 'private' && field('d').length !== 32)) fail('ERR_CRYPTO_INVALID_JWK', 'Invalid JWK OKP key', TypeError); return type === 'private' ? { type, kind: 'ed25519', x, d: field('d') } : { type, kind: 'ed25519', x }; }
    fail('ERR_CRYPTO_INVALID_JWK', kty === 'EC' || kty === 'OKP' ? 'Unsupported JWK curve' : 'Invalid JWK key type', TypeError);
  };
  const publicOf = ({ d, p, q, dp, dq, qi, ...data }) => ({ ...data, type: 'public' });
  const keys = new WeakMap(), token = Symbol();
  class KeyObject {
    constructor(type, data) { if (type !== token) fail('ERR_INVALID_ARG_VALUE', `The argument 'type' is invalid. Received ${typeof type === 'string' ? `'${type}'` : type}`, TypeError); keys.set(this, data); }
    get type() { return keys.get(this).type; }
    get asymmetricKeyType() { return keys.get(this).kind; }
    get symmetricKeySize() { return keys.get(this).key?.length; }
    get asymmetricKeyDetails() {
      const data = keys.get(this);
      if (data.kind === 'ec') return { namedCurve: curves[data.curve][1] };
      if (data.kind === 'rsa') return { modulusLength: data.n.length * 8 - Math.clz32(data.n[0]) + 24, publicExponent: BigInt('0x' + data.e.toString('hex')) };
      return data.kind && {};
    }
    equals(other) {
      const a = keys.get(this), b = keys.get(other) ?? fail('ERR_INVALID_ARG_TYPE', 'The "otherKeyObject" argument must be an instance of KeyObject', TypeError);
      return a.type === b.type && (a.type === 'secret' ? a.key.length === b.key.length && codec.equal(a.key, b.key) : encode(a, a.type === 'public' ? 'spki' : 'pkcs8').equals(encode(b, b.type === 'public' ? 'spki' : 'pkcs8')));
    }
    export(o) {
      const data = keys.get(this);
      if (data.type === 'secret') return o?.format === 'jwk' ? jwk(data) : Buffer.from(data.key);
      if (o === null || typeof o !== 'object') fail('ERR_INVALID_ARG_TYPE', `The "options" argument must be of type object. Received ${o === null ? 'null' : typeof o === 'undefined' ? 'undefined' : `type ${typeof o}`}`, TypeError);
      if (o.format === 'jwk') return jwk(data);
      if (o.cipher !== undefined || o.passphrase !== undefined) fail('ERR_FEATURE_UNAVAILABLE_ON_PLATFORM', 'Encrypted key export is not supported');
      if ((o.type === 'pkcs1' && data.kind !== 'rsa') || (o.type === 'sec1' && data.kind !== 'ec')) fail('ERR_CRYPTO_INCOMPATIBLE_KEY_OPTIONS', `The selected key encoding ${o.type} can only be used for ${o.type === 'sec1' ? 'EC' : 'RSA'} keys.`);
      if (!(data.type === 'public' ? ['spki', 'pkcs1'] : ['pkcs8', 'pkcs1', 'sec1']).includes(o.type)) fail('ERR_INVALID_ARG_VALUE', `The property 'options.type' is invalid. Received ${inspect(o.type)}`, TypeError);
      if (o.format !== 'pem' && o.format !== 'der') fail('ERR_INVALID_ARG_VALUE', `The property 'options.format' is invalid. Received ${inspect(o.format)}`, TypeError);
      const bytes = encode(data, o.type), label = `${o.type === 'pkcs1' ? 'RSA ' : o.type === 'sec1' ? 'EC ' : ''}${data.type.toUpperCase()} KEY`;
      return o.format === 'der' ? bytes : `-----BEGIN ${label}-----\n${bytes.toString('base64').match(/.{1,64}/g).join('\n')}\n-----END ${label}-----\n`;
    }
  }
  const wrap = data => new (data.type === 'secret' ? SecretKeyObject : data.type === 'public' ? PublicKeyObject : PrivateKeyObject)(token, data);
  class SecretKeyObject extends KeyObject {}
  class PublicKeyObject extends KeyObject {}
  class PrivateKeyObject extends KeyObject {}
  const load = (value, want) => {
    if (value instanceof KeyObject || value?.key instanceof KeyObject) {
      const data = keys.get(value.key ?? value);
      if (data.type === want || (want === 'public' && data.type === 'private')) return want === 'public' ? publicOf(data) : data;
      fail('ERR_CRYPTO_INVALID_KEY_OBJECT_TYPE', `Invalid key object type ${data.type}, expected ${want}.`, TypeError);
    }
    const opts = typeof value === 'string' || ArrayBuffer.isView(value) || value instanceof ArrayBuffer ? { key: value } : value ?? {}, format = opts.format ?? 'pem';
    let data;
    if (format === 'jwk') data = fromJwk(opts.key);
    else {
      if (typeof opts.key !== 'string' && !ArrayBuffer.isView(opts.key) && !(opts.key instanceof ArrayBuffer)) fail('ERR_INVALID_ARG_TYPE', 'The "key.key" property must be of type string or an instance of ArrayBuffer, Buffer, TypedArray, or DataView.', TypeError);
      let bytes = Buffer.from(input(opts.key, opts.encoding)), type = opts.type;
      if (format === 'pem') {
        const text = bytes.toString('latin1'), match = /-----BEGIN ([A-Z0-9 ]+)-----([^-]*)-----END \1-----/.exec(text);
        if (/-----BEGIN ENCRYPTED|Proc-Type: 4,ENCRYPTED/.test(text)) fail('ERR_FEATURE_UNAVAILABLE_ON_PLATFORM', 'Encrypted private keys are not supported');
        type = { 'PUBLIC KEY': 'spki', 'RSA PUBLIC KEY': 'pkcs1-public', 'PRIVATE KEY': 'pkcs8', 'RSA PRIVATE KEY': 'pkcs1', 'EC PRIVATE KEY': 'sec1', CERTIFICATE: 'x509' }[match?.[1]] ?? unsupported();
        bytes = Buffer.from(match[2].replace(/\s+/g, ''), 'base64');
      } else if (format !== 'der') fail('ERR_INVALID_ARG_VALUE', `The property 'key.format' is invalid. Received ${inspect(format)}`, TypeError);
      else if (type === 'pkcs1' && want === 'public') type = 'pkcs1-public';
      try {
        if (type === 'x509') { const fields = items(der(der(bytes)[1])[1]); bytes = tlv(0x30, fields[fields[0][0] === 0xa0 ? 6 : 5][1]); type = 'spki'; }
        data = type === 'spki' ? spki(bytes) : type === 'pkcs8' ? pkcs8(bytes) : type === 'sec1' ? ecKey(bytes) : type === 'pkcs1' ? rsaKey(bytes, 'private') : type === 'pkcs1-public' ? rsaKey(bytes, 'public') : fail('ERR_INVALID_ARG_VALUE', `The property 'key.type' is invalid. Received ${inspect(type)}`, TypeError);
      } catch (error) { if (error.code) throw error; unsupported(); }
    }
    if (data.type === 'secret' || (want === 'private' && data.type === 'public')) fail('ERR_CRYPTO_INVALID_KEY_OBJECT_TYPE', `Invalid key object type ${data.type}, expected ${want}.`, TypeError);
    return want === 'public' ? publicOf(data) : data;
  };
  const secret = (value, encoding) => value instanceof KeyObject ? keys.get(value).key ?? fail('ERR_CRYPTO_INVALID_KEY_OBJECT_TYPE', `Invalid key object type ${value.type}, expected secret.`, TypeError) : Buffer.from(input(value, encoding));
  const signing = { sha1: ['SHA-1', 20], sha256: ['SHA-256', 32], sha384: ['SHA-384', 48], sha512: ['SHA-512', 64] };
  // WARNING: ring pads RSA-PSS with a salt as long as the digest, so other salt lengths are rejected.
  const signature = (algorithm, data, key, signed) => {
    const verify = signed !== undefined, opts = key !== null && typeof key === 'object' && !ArrayBuffer.isView(key) ? key : {}, value = load(key, verify ? 'public' : 'private');
    const name = algorithm == null ? 'sha256' : String(algorithm).toLowerCase().replace(/^(?:rsa-|ecdsa-with-)/, '').replace('-', ''), [hash, size] = signing[name] ?? fail('ERR_CRYPTO_INVALID_DIGEST', `Invalid digest: ${algorithm}`, TypeError);
    let kind = value.kind;
    if (kind === 'ec') kind = opts.dsaEncoding === 'ieee-p1363' ? 'ECDSA' : 'ec';
    else if (kind === 'rsa' && opts.padding === 6) { kind = 'rsa-pss'; if (opts.saltLength !== undefined && opts.saltLength !== -1 && opts.saltLength !== size && !(verify && opts.saltLength === -2)) fail('ERR_FEATURE_UNAVAILABLE_ON_PLATFORM', 'RSA-PSS salts must be as long as the digest'); }
    else if (kind === 'rsa' && opts.padding !== undefined && opts.padding !== 1) fail('ERR_INVALID_ARG_VALUE', `The property 'options.padding' is invalid. Received ${inspect(opts.padding)}`, TypeError);
    const empty = new Uint8Array(0), bytes = input(data, 'utf8');
    const args = [kind, verify ? empty : kind.startsWith('rsa') ? rsaDer(value, 'private') : value.d, kind.startsWith('rsa') ? (verify ? rsaDer(value, 'public') : empty) : value.x, bytes, verify ? input(signed) : empty, hash];
    return verify ? call('web.crypto.verify', ...args) : output(call('web.crypto.sign', ...args));
  };
  let signed;
  class Signer extends stream.Writable {
    #algorithm; #chunks = [];
    static { signed = (self, key, value) => signature(self.#algorithm, Buffer.concat(self.#chunks), key, value); }
    constructor(algorithm, o) { super(o); this.#algorithm = algorithm; }
    _write(chunk, encoding, callback) { this.update(chunk, encoding); callback(); }
    update(data, encoding) { this.#chunks.push(Buffer.from(input(data, encoding))); return this; }
  }
  class Sign extends Signer { sign(key, encoding) { return encoded(signed(this, key), encoding); } }
  class Verify extends Signer { verify(key, value, encoding) { return signed(this, key, typeof value === 'string' ? Buffer.from(value, encoding) : value); } }
  const later = (callback, run) => { let result, failure = null; try { result = run(); } catch (error) { failure = error; } queueMicrotask(() => failure ? callback(failure) : callback(null, ...(Array.isArray(result) ? result : [result]))); };
  const generateKeyPairSync = (type, o = {}) => {
    let data;
    if (type === 'ec' || type === 'ed25519') {
      const curve = type === 'ec' ? curveOf(o.namedCurve) ?? fail('ERR_CRYPTO_INVALID_CURVE', 'Invalid EC curve name', TypeError) : 'ed25519', { private: d, public: x } = call('web.crypto.generate', curve);
      data = { type: 'private', kind: type, ...(type === 'ec' ? { curve } : {}), d: output(d), x: output(x) };
    } else if (type === 'rsa' || type === 'rsa-pss') fail('ERR_FEATURE_UNAVAILABLE_ON_PLATFORM', 'RSA key generation is not supported. Generate RSA keys outside the app and import them with createPrivateKey()');
    else fail('ERR_INVALID_ARG_VALUE', `The argument 'type' must be a supported key type. Received ${inspect(type)}`, TypeError);
    const privateKey = wrap(data), publicKey = wrap(publicOf(data));
    return { publicKey: o.publicKeyEncoding ? publicKey.export(o.publicKeyEncoding) : publicKey, privateKey: o.privateKeyEncoding ? privateKey.export(o.privateKeyEncoding) : privateKey };
  };
  const generateKeySync = (type, o) => {
    const length = o?.length;
    if (type !== 'hmac' && type !== 'aes') fail('ERR_INVALID_ARG_VALUE', `The argument 'type' must be a supported key type. Received ${inspect(type)}`, TypeError);
    if (type === 'aes' ? ![128, 192, 256].includes(length) : !(Number.isInteger(length) && length >= 8 && length <= 2 ** 31 - 1)) fail('ERR_INVALID_ARG_VALUE', `The property 'options.length' is invalid. Received ${inspect(length)}`, TypeError);
    return wrap({ type: 'secret', key: fill(Buffer.alloc(length >> 3)) });
  };
  const hkdf = (name, ikm, salt, info, length) => {
    name = String(name).toLowerCase();
    const size = { md5: 16, sha1: 20, sha224: 28, sha256: 32, sha384: 48, sha512: 64 }[name] ?? fail('ERR_CRYPTO_INVALID_DIGEST', `Invalid digest: ${name}`, TypeError);
    info = secret(info);
    if (info.length > 1024) fail('ERR_OUT_OF_RANGE', `The value of "info" is out of range. It must be <= 1024. Received ${info.length}`, RangeError);
    if (!Number.isInteger(length) || length < 0 || length > 255 * size) fail('ERR_CRYPTO_INVALID_KEYLEN', 'Invalid key length', RangeError);
    return [name, secret(ikm), secret(salt), info, length];
  };
  const scrypt = (password, salt, length, o = {}) => {
    const option = (names, fallback) => names.map(name => o[name]).find(value => value !== undefined) ?? fallback;
    if (!Number.isInteger(length) || length < 0) fail('ERR_OUT_OF_RANGE', `The value of "keylen" is out of range. It must be >= 0 && <= 2147483647. Received ${length}`, RangeError);
    return [secret(password), secret(salt), option(['N', 'cost'], 16384), option(['r', 'blockSize'], 8), option(['p', 'parallelization'], 1), length, option(['maxmem'], 32 << 20)];
  };
  const scryptError = error => Object.assign(error, { code: 'ERR_CRYPTO_INVALID_SCRYPT_PARAMS' });
  const arrayBuffer = data => data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength);
  const key = value => typeof value === 'string' ? Buffer.from(value) : secret(value);
  const pbkdf2 = (password, salt, iterations, length, name) => [key(password), key(salt), iterations, length, digest(name)];
  const fill = (target, offset = 0, size) => { const bytes = input(target); size ??= bytes.length - offset; for (let at = offset; at < offset + size; at += 65536) webcrypto.getRandomValues(bytes.subarray(at, Math.min(at + 65536, offset + size))); return target; };
  const crypto = {
    webcrypto, subtle: webcrypto.subtle, getRandomValues: array => webcrypto.getRandomValues(array),
    createHash: algorithm => new Hash(algorithm),
    createHmac: (algorithm, secret) => new Hash(digest(algorithm), key(secret)),
    hash: (algorithm, data, encoding = 'hex') => new Hash(algorithm).update(data).digest(encoding),
    getHashes: () => [...hashes],
    getCiphers: () => ['aes-128-gcm', 'aes-256-gcm'],
    getCurves: () => ['prime256v1', 'secp384r1'],
    KeyObject, Sign, Verify,
    createSecretKey: (value, encoding) => wrap({ type: 'secret', key: secret(value, encoding) }),
    createPublicKey: value => wrap(load(value, 'public')),
    createPrivateKey: value => wrap(load(value, 'private')),
    createSign: (algorithm, o) => new Sign(algorithm, o),
    createVerify: (algorithm, o) => new Verify(algorithm, o),
    sign: (algorithm, data, key, callback) => callback ? later(callback, () => signature(algorithm, data, key)) : signature(algorithm, data, key),
    verify: (algorithm, data, key, value, callback) => callback ? later(callback, () => signature(algorithm, data, key, value)) : signature(algorithm, data, key, value),
    generateKeyPairSync,
    generateKeyPair: (type, o, callback) => { if (typeof o === 'function') [o, callback] = [{}, o]; later(callback, () => { const pair = generateKeyPairSync(type, o); return [pair.publicKey, pair.privateKey]; }); },
    generateKeySync,
    generateKey: (type, o, callback) => later(callback, () => generateKeySync(type, o)),
    hkdfSync: (...args) => arrayBuffer(sync('hkdf', ...hkdf(...args))),
    hkdf: (name, ikm, salt, info, length, callback) => { const args = hkdf(name, ikm, salt, info, length); asyncCall('hkdf', ...args).then(result => callback(null, arrayBuffer(result)), callback); },
    scryptSync: (...args) => { try { return output(sync('scrypt', ...scrypt(...args))); } catch (error) { throw scryptError(error); } },
    scrypt: (password, salt, length, o, callback) => { if (typeof o === 'function') [o, callback] = [{}, o]; asyncCall('scrypt', ...scrypt(password, salt, length, o)).then(result => callback(null, output(result)), error => callback(scryptError(error))); },
    timingSafeEqual: (a, b) => { a = input(a); b = input(b); if (a.length !== b.length) throw Object.assign(new RangeError('Input buffers must have the same byte length'), { code: 'ERR_CRYPTO_TIMING_SAFE_EQUAL_LENGTH' }); return codec.equal(a, b); },
    constants: { RSA_PKCS1_PADDING: 1, RSA_NO_PADDING: 3, RSA_PKCS1_OAEP_PADDING: 4, RSA_X931_PADDING: 5, RSA_PKCS1_PSS_PADDING: 6, RSA_PSS_SALTLEN_DIGEST: -1, RSA_PSS_SALTLEN_MAX_SIGN: -2, RSA_PSS_SALTLEN_AUTO: -2, POINT_CONVERSION_COMPRESSED: 2, POINT_CONVERSION_UNCOMPRESSED: 4, POINT_CONVERSION_HYBRID: 6 },
    createCipheriv: (algorithm, secret, iv, options) => new Cipher(false, algorithm, secret, iv, options),
    createDecipheriv: (algorithm, secret, iv, options) => new Cipher(true, algorithm, secret, iv, options),
    pbkdf2Sync: (...args) => output(sync('pbkdf2', ...pbkdf2(...args))),
    pbkdf2: (password, salt, iterations, length, name, callback) => { asyncCall('pbkdf2', ...pbkdf2(password, salt, iterations, length, name)).then(result => callback(null, output(result)), callback); },
    randomFillSync: fill,
    randomFill(target, offset, size, callback) { callback = [offset, size, callback].find(value => typeof value === 'function'); fill(target, typeof offset === 'number' ? offset : 0, typeof size === 'number' ? size : undefined); queueMicrotask(() => callback(null, target)); },
    randomInt(min, max, callback) {
      if (typeof max !== 'number') { callback = max; max = min; min = 0; }
      if (!Number.isSafeInteger(min) || !Number.isSafeInteger(max) || max <= min || max - min > 2 ** 48) throw Object.assign(new RangeError('The value of "max" is out of range. It must be greater than the value of "min" and the range must be at most 2 ** 48'), { code: 'ERR_OUT_OF_RANGE' });
      const range = max - min, limit = 2 ** 48 - 2 ** 48 % range, words = new Uint16Array(3);
      let value;
      do { webcrypto.getRandomValues(words); value = words[0] * 2 ** 32 + words[1] * 2 ** 16 + words[2]; } while (value >= limit);
      if (!callback) return min + value % range;
      queueMicrotask(() => callback(null, min + value % range));
    },
  };
  crypto.generateKeyPair[Symbol.for('nodejs.util.promisify.custom')] = async (type, o) => generateKeyPairSync(type, o);
  return { zlib, readline, crypto };
};
