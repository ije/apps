globalThis.__appsNodeIO = ({ call, sync, asyncCall, Buffer, stream, EventEmitter, StringDecoder, webcrypto }) => {
  const input = (value, encoding) => typeof value === 'string' ? Buffer.from(value, encoding) : ArrayBuffer.isView(value) ? new Uint8Array(value.buffer, value.byteOffset, value.byteLength) : value instanceof ArrayBuffer ? new Uint8Array(value) : (() => { throw Object.assign(new TypeError('The "data" argument must be of type string or an instance of Buffer, TypedArray, or DataView'), { code: 'ERR_INVALID_ARG_TYPE' }); })();
  const output = data => Buffer.from(data.buffer, data.byteOffset, data.byteLength);
  const encoded = (data, encoding) => encoding && encoding !== 'buffer' ? data.toString(encoding) : data;

  const zlibError = error => Object.assign(error, /maxOutputLength/.test(error.message) ? { code: 'ERR_BUFFER_TOO_LARGE' } : { code: 'Z_DATA_ERROR', errno: -3 });
  const zlib = { constants: { Z_NO_FLUSH: 0, Z_PARTIAL_FLUSH: 1, Z_SYNC_FLUSH: 2, Z_FULL_FLUSH: 3, Z_FINISH: 4, Z_OK: 0, Z_STREAM_END: 1, Z_NEED_DICT: 2, Z_ERRNO: -1, Z_STREAM_ERROR: -2, Z_DATA_ERROR: -3, Z_BUF_ERROR: -5, Z_NO_COMPRESSION: 0, Z_BEST_SPEED: 1, Z_BEST_COMPRESSION: 9, Z_DEFAULT_COMPRESSION: -1, Z_FILTERED: 1, Z_HUFFMAN_ONLY: 2, Z_RLE: 3, Z_FIXED: 4, Z_DEFAULT_STRATEGY: 0 } };
  const compressor = (format, inflate) => {
    let writer, reading, failed = null;
    const start = chunk => {
      const kind = format === 'unzip' ? (chunk?.[0] === 0x1f && chunk?.[1] === 0x8b ? 'gzip' : 'deflate') : format;
      const web = inflate ? new DecompressionStream(kind) : new CompressionStream(kind);
      writer = web.writable.getWriter();
      const reader = web.readable.getReader();
      reading = (async () => { for (;;) { const { value, done } = await reader.read(); if (done) return; transform.push(output(value)); } })().catch(error => { failed = zlibError(error); });
      writer.closed.catch(() => {});
    };
    const transform = new stream.Transform({
      transform(chunk, _encoding, callback) { if (!writer) start(chunk); writer.write(chunk).then(() => callback(failed), error => callback(failed ?? zlibError(error))); },
      flush(callback) { if (!writer) start(); writer.close().then(() => reading, () => reading).then(() => callback(failed)); },
    });
    return transform;
  };
  for (const [name, format, inflate] of [['Gzip', 'gzip', false], ['Gunzip', 'gzip', true], ['Deflate', 'deflate', false], ['Inflate', 'deflate', true], ['DeflateRaw', 'deflate-raw', false], ['InflateRaw', 'deflate-raw', true], ['Unzip', 'unzip', true]]) {
    const method = name[0].toLowerCase() + name.slice(1), args = (data, o) => [format, inflate, input(data), o?.level ?? -1, o?.maxOutputLength];
    zlib[method + 'Sync'] = (data, o) => { try { return output(sync('zlib', ...args(data, o))); } catch (error) { throw zlibError(error); } };
    zlib[method] = (data, o, callback) => { if (typeof o === 'function') { callback = o; o = {}; } asyncCall('zlib', ...args(data, o)).then(result => callback(null, output(result)), error => callback(zlibError(error))); };
    zlib['create' + name] = () => compressor(format, inflate);
    zlib[name] = function() { return compressor(format, inflate); };
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
  const readline = {
    Interface, createInterface: (...args) => new Interface(...args),
    clearLine: (stream, direction, callback) => terminal(stream, direction < 0 ? '\x1b[1K' : direction > 0 ? '\x1b[0K' : '\x1b[2K', callback),
    clearScreenDown: (stream, callback) => terminal(stream, '\x1b[0J', callback),
    cursorTo: (stream, x, y, callback) => { if (typeof y === 'function') { callback = y; y = undefined; } return terminal(stream, typeof y === 'number' ? `\x1b[${y + 1};${x + 1}H` : `\x1b[${x + 1}G`, callback); },
    moveCursor: (stream, dx, dy, callback) => terminal(stream, (dx < 0 ? `\x1b[${-dx}D` : dx > 0 ? `\x1b[${dx}C` : '') + (dy < 0 ? `\x1b[${-dy}A` : dy > 0 ? `\x1b[${dy}B` : ''), callback),
    emitKeypressEvents() {},
    promises: { Interface: PromiseInterface, createInterface: (...args) => new PromiseInterface(...args) },
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
      data = Buffer.from(input(data, encoding));
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
      const data = Buffer.concat(this.#chunks);
      this.#chunks = [];
      let result;
      if (this.#id === null) result = sync('hash', this.#algorithm, data, this.#secret);
      else { if (data.length) sync('hash.update', this.#id, data); result = sync('hash.digest', this.#id); handles.unregister(this); }
      return encoded(output(result), encoding);
    }
    copy() { if (this.#done || this.#secret) throw Object.assign(new Error('Digest already called'), { code: 'ERR_CRYPTO_HASH_FINALIZED' }); return new Hash(this.#algorithm, null, { chunks: this.#chunks, size: this.#size, id: this.#id }); }
  }
  class Cipher extends stream.Transform {
    #decrypt; #key; #iv; #chunks = []; #aad = new Uint8Array(0); #tag = null; #done = false;
    constructor(decrypt, algorithm, key, iv) {
      super();
      const bits = /^aes-(128|256)-gcm$/i.exec(algorithm)?.[1];
      if (!bits) throw Object.assign(new Error('Unknown cipher'), { code: 'ERR_CRYPTO_UNKNOWN_CIPHER' });
      this.#decrypt = decrypt; this.#key = input(key); this.#iv = input(iv);
      if (this.#key.length * 8 !== Number(bits)) throw Object.assign(new RangeError('Invalid key length'), { code: 'ERR_CRYPTO_INVALID_KEYLEN' });
      if (this.#iv.length !== 12) throw Object.assign(new TypeError('Invalid initialization vector'), { code: 'ERR_CRYPTO_INVALID_IV' });
    }
    _transform(chunk, encoding, callback) { callback(null, this.update(chunk, encoding)); }
    _flush(callback) { try { callback(null, this.final()); } catch (error) { callback(error); } }
    update(data, inputEncoding, outputEncoding) { this.#chunks.push(Buffer.from(input(data, inputEncoding))); return encoded(Buffer.alloc(0), outputEncoding); }
    final(outputEncoding) {
      if (this.#done) throw new Error('Unsupported state');
      this.#done = true;
      const data = Buffer.concat(this.#chunks);
      if (this.#decrypt) {
        if (!this.#tag) throw new Error('Unsupported state or unable to authenticate data');
        try { return encoded(output(call('web.crypto.decrypt', this.#key, this.#iv, this.#aad, Buffer.concat([data, this.#tag]))), outputEncoding); }
        catch { throw new Error('Unsupported state or unable to authenticate data'); }
      }
      const sealed = output(call('web.crypto.encrypt', this.#key, this.#iv, this.#aad, data));
      this.#tag = sealed.subarray(sealed.length - 16);
      return encoded(sealed.subarray(0, sealed.length - 16), outputEncoding);
    }
    setAAD(data) { this.#aad = input(data); return this; }
    getAuthTag() { if (this.#decrypt || !this.#tag) throw new Error('Invalid state for operation getAuthTag'); return Buffer.from(this.#tag); }
    setAuthTag(tag) { if (!this.#decrypt) throw new Error('Invalid state for operation setAuthTag'); this.#tag = input(tag); return this; }
    setAutoPadding() { return this; }
  }
  const key = value => typeof value === 'string' ? Buffer.from(value) : input(value);
  const pbkdf2 = (password, salt, iterations, length, name) => [key(password), key(salt), iterations, length, digest(name)];
  const fill = (target, offset = 0, size) => { const bytes = input(target); size ??= bytes.length - offset; for (let at = offset; at < offset + size; at += 65536) webcrypto.getRandomValues(bytes.subarray(at, Math.min(at + 65536, offset + size))); return target; };
  const crypto = {
    webcrypto, subtle: webcrypto.subtle, getRandomValues: array => webcrypto.getRandomValues(array),
    createHash: algorithm => new Hash(algorithm),
    createHmac: (algorithm, secret) => { const name = digest(algorithm); if (name === 'md5' || name === 'sha224') throw new Error('Digest method not supported'); return new Hash(name, key(secret)); },
    hash: (algorithm, data, encoding = 'hex') => new Hash(algorithm).update(data).digest(encoding),
    getHashes: () => [...hashes],
    getCiphers: () => ['aes-128-gcm', 'aes-256-gcm'],
    createCipheriv: (algorithm, secret, iv) => new Cipher(false, algorithm, secret, iv),
    createDecipheriv: (algorithm, secret, iv) => new Cipher(true, algorithm, secret, iv),
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
  return { zlib, readline, crypto };
};
