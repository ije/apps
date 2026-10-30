globalThis.__appsNode = (call, EventEmitter, { inspect, format, formatWithOptions, deepEqual }, info) => {
  const windows = info.platform === 'win32';
  const bytes = text => { if(typeof text !== 'string')return new Buffer(text.buffer, text.byteOffset, text.byteLength); const length = text.length, out = new Buffer(length); for (let i = 0; i < length; i++) out[i] = text.charCodeAt(i); return out; };
  const binary = data => { let s = ''; for (let i = 0; i < data.length; i += 8192) s += String.fromCharCode.apply(null, data.subarray(i, i + 8192)); return s; };
  const view = (value, encoding) => typeof value === 'string' ? Buffer.from(value, encoding) : ArrayBuffer.isView(value) ? new Uint8Array(value.buffer, value.byteOffset, value.byteLength) : Buffer.from(value);
  const utf8 = /^utf-?8$/i;
  const normalize = encoding => { const value = String(encoding).toLowerCase().replace('-', ''); return value === 'binary' ? 'latin1' : value === 'ucs2' ? 'utf16le' : value; };
  const encode = (value, encoding = 'utf8') => {
    encoding = normalize(encoding);
    if (encoding === 'hex') { const pairs = String(value).match(/^(?:[\da-f]{2})*/i)[0]; return Uint8Array.fromHex ? Uint8Array.fromHex(pairs) : Uint8Array.from(pairs.match(/../g) ?? [], s => parseInt(s, 16)); }
    if (encoding === 'base64' || encoding === 'base64url') { let text = String(value).replace(/-/g, '+').replace(/_/g, '/').replace(/[^A-Za-z0-9+/]/g, ''); if (text.length % 4 === 1) text = text.slice(0, -1); return Uint8Array.fromBase64 ? Uint8Array.fromBase64(text) : bytes(atob(text)); }
    if (encoding === 'ascii' || encoding === 'latin1') return bytes(String(value));
    if (encoding === 'utf16le') { const s=String(value), bytes=new Uint8Array(s.length*2); for(let i=0;i<s.length;i++){bytes[i*2]=s.charCodeAt(i)&255;bytes[i*2+1]=s.charCodeAt(i)>>>8;} return bytes; }
    if (encoding !== 'utf8') throw new TypeError('Unknown encoding: ' + encoding);
    return new TextEncoder().encode(String(value));
  };
  const range = (buffer, offset, size) => { if (!Number.isInteger(offset) || offset < 0 || offset + size > buffer.length) throw Object.assign(new RangeError(`The value of "offset" is out of range. It must be >= 0 and <= ${buffer.length - size}. Received ${offset}`), { code: 'ERR_OUT_OF_RANGE' }); };
  class Buffer extends Uint8Array {
    static from(value, encoding, length) {
      if (typeof value === 'string') { const bytes = encode(value, encoding); return new Buffer(bytes.buffer, bytes.byteOffset, bytes.length); }
      if (value instanceof ArrayBuffer || (typeof SharedArrayBuffer !== 'undefined' && value instanceof SharedArrayBuffer)) return new Buffer(value, encoding, length);
      if (value?.type === 'Buffer' && Array.isArray(value.data)) value = value.data;
      if (typeof value === 'number' || value == null) throw new TypeError('The first argument must be of type string or an instance of Buffer, ArrayBuffer, or Array or an Array-like Object');
      return new Buffer(value);
    }
    static alloc(size, fill, encoding) { const out = new Buffer(size); return fill === undefined || fill === 0 ? out : out.fill(fill, 0, size, encoding); }
    static allocUnsafe(size) { return new Buffer(size); }
    static allocUnsafeSlow(size) { return new Buffer(size); }
    static isBuffer(value) { return value instanceof Buffer; }
    static isEncoding(value) { return typeof value === 'string' && /^(utf-?8|hex|base64(?:url)?|ascii|latin1|binary|utf-?16le|ucs-?2)$/i.test(value); }
    static byteLength(value, encoding) { return typeof value === 'string' ? encode(value, encoding).length : value.byteLength; }
    static concat(values, length = values.reduce((n, v) => n + v.length, 0)) { const out = new Buffer(length); let offset = 0; for (const value of values) { if (offset >= length) break; const n = Math.min(value.length, length - offset); out.set(value.subarray(0, n), offset); offset += n; } return out; }
    static compare(a, b) { for (let i = 0; i < Math.min(a.length, b.length); i++) if (a[i] !== b[i]) return a[i] < b[i] ? -1 : 1; return Math.sign(a.length - b.length); }
    toString(encoding = 'utf8', start = 0, end = this.length) {
      const bytes = this.subarray(start, end); encoding = normalize(encoding);
      if (encoding === 'hex') return bytes.toHex ? bytes.toHex() : Array.from(bytes, v => v.toString(16).padStart(2, '0')).join('');
      if (encoding === 'base64' || encoding === 'base64url') return bytes.toBase64 ? bytes.toBase64({ alphabet: encoding, omitPadding: encoding === 'base64url' }) : encoding === 'base64' ? btoa(binary(bytes)) : btoa(binary(bytes)).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
      if (encoding === 'ascii' || encoding === 'latin1') return binary(encoding === 'ascii' ? bytes.map(byte => byte & 127) : bytes);
      if (encoding === 'utf16le') { let s = ''; for (let i = 0; i + 1 < bytes.length; i += 2) s += String.fromCharCode(bytes[i] | bytes[i + 1] << 8); return s; }
      if (encoding !== 'utf8') throw new TypeError('Unknown encoding: ' + encoding); return new TextDecoder('utf-8', { ignoreBOM: true }).decode(bytes);
    }
    toLocaleString(...args) { return this.toString(...args); }
    equals(other) { return Buffer.compare(this, other) === 0; }
    compare(other, targetStart = 0, targetEnd = other.length, start = 0, end = this.length) { return Buffer.compare(this.subarray(start, end), other.subarray(targetStart, targetEnd)); }
    slice(start, end) { return this.subarray(start, end); }
    copy(target, targetStart = 0, start = 0, end = this.length) { const v = this.subarray(start, Math.min(end, start + target.length - targetStart)); target.set(v, targetStart); return v.length; }
    write(value, offset = 0, length = this.length - offset, encoding = 'utf8') { if (typeof offset === 'string') { encoding = offset; offset = 0; length = this.length; } if (typeof length === 'string') { encoding = length; length = this.length - offset; } const b = encode(value, encoding).subarray(0, Math.min(length, this.length - offset)); this.set(b, offset); return b.length; }
    fill(value, offset = 0, end = this.length, encoding) {
      if (typeof offset === 'string') { encoding = offset; offset = 0; end = this.length; } else if (typeof end === 'string') { encoding = end; end = this.length; }
      if (typeof value !== 'string' && !ArrayBuffer.isView(value)) return super.fill(Number(value) & 255, offset, end);
      const pattern = typeof value === 'string' ? encode(value, encoding) : view(value);
      if (!pattern.length) { if (typeof value === 'string') return super.fill(0, offset, end); throw Object.assign(new TypeError('The argument \'value\' is invalid'), { code: 'ERR_INVALID_ARG_VALUE' }); }
      if (pattern.length === 1) return super.fill(pattern[0], offset, end);
      if (end <= offset) return this;
      this.set(pattern.subarray(0, Math.min(pattern.length, end - offset)), offset);
      for (let size = pattern.length; size < end - offset; size *= 2) this.copyWithin(offset + size, offset, offset + Math.min(size, end - offset - size));
      return this;
    }
    toJSON() { return { type: 'Buffer', data: Array.from(this) }; }
    indexOf(value, offset = 0, encoding) {
      if (typeof offset === 'string') { encoding = offset; offset = 0; }
      if (offset < 0) offset = Math.max(0, this.length + offset);
      if (typeof value === 'number') return super.indexOf(value & 255, offset);
      const needle = typeof value === 'string' ? encode(value, encoding) : view(value);
      if (!needle.length) return Math.min(offset, this.length);
      for (let i = super.indexOf(needle[0], offset); i >= 0 && i <= this.length - needle.length; i = super.indexOf(needle[0], i + 1)) { let j = 1; while (j < needle.length && this[i + j] === needle[j]) j++; if (j === needle.length) return i; }
      return -1;
    }
    lastIndexOf(value, offset = this.length, encoding) {
      if (typeof offset === 'string') { encoding = offset; offset = this.length; }
      if (offset < 0) offset += this.length;
      if (typeof value === 'number') return super.lastIndexOf(value & 255, offset);
      const needle = typeof value === 'string' ? encode(value, encoding) : view(value);
      for (let i = Math.min(offset, this.length - needle.length); i >= 0; i--) { let j = 0; while (j < needle.length && this[i + j] === needle[j]) j++; if (j === needle.length) return i; }
      return -1;
    }
    includes(value, offset, encoding) { return this.indexOf(value, offset, encoding) >= 0; }
    [inspect.custom]() { const hex = this.subarray(0, 50).toString('hex').replace(/../g, ' $&'); return `<Buffer${hex}${this.length > 50 ? ` ... ${this.length - 50} more bytes` : ''}>`; }
  }
  for (const [suffix, type, size] of [['UInt8','Uint8',1],['Int8','Int8',1],['UInt16','Uint16',2],['Int16','Int16',2],['UInt32','Uint32',4],['Int32','Int32',4],['Float','Float32',4],['Double','Float64',8],['BigInt64','BigInt64',8],['BigUInt64','BigUint64',8]]) for (const endian of size === 1 ? [''] : ['LE','BE']) {
    Buffer.prototype['read' + suffix + endian] = function(offset = 0) { range(this, offset, size); return new DataView(this.buffer, this.byteOffset, this.byteLength)['get' + type](offset, endian === 'LE'); };
    Buffer.prototype['write' + suffix + endian] = function(value, offset = 0) { range(this, offset, size); new DataView(this.buffer, this.byteOffset, this.byteLength)['set' + type](offset, value, endian === 'LE'); return offset + size; };
  }
  for (const endian of ['LE', 'BE']) {
    const at = (offset, length, i) => offset + (endian === 'LE' ? i : length - 1 - i);
    Buffer.prototype['readUInt' + endian] = function(offset, length) { range(this, offset, length); let value = 0; for (let i = length - 1; i >= 0; i--) value = value * 256 + this[at(offset, length, i)]; return value; };
    Buffer.prototype['readInt' + endian] = function(offset, length) { const value = this['readUInt' + endian](offset, length); return value >= 2 ** (length * 8 - 1) ? value - 2 ** (length * 8) : value; };
    Buffer.prototype['writeUInt' + endian] = Buffer.prototype['writeInt' + endian] = function(value, offset, length) { range(this, offset, length); value = +value; if (value < 0) value += 2 ** (length * 8); for (let i = 0; i < length; i++) { this[at(offset, length, i)] = value % 256; value = Math.floor(value / 256); } return offset + length; };
  }
  for (const name of Object.getOwnPropertyNames(Buffer.prototype)) if (name.includes('UInt')) Buffer.prototype[name.replace('UInt', 'Uint')] = Buffer.prototype[name];
  for (const size of [2, 4, 8]) Buffer.prototype['swap' + size * 8] = function() { if (this.length % size) throw Object.assign(new RangeError(`Buffer size must be a multiple of ${size * 8}-bits`), { code: 'ERR_INVALID_BUFFER_SIZE' }); for (let i = 0; i < this.length; i += size) for (let a = i, b = i + size - 1; a < b; a++, b--) { const t = this[a]; this[a] = this[b]; this[b] = t; } return this; };
  Buffer.poolSize = 8192;
  globalThis.Buffer = Buffer;
  const buffer = { Buffer, Blob, File, atob, btoa, kMaxLength: 2 ** 32 - 1, kStringMaxLength: 2 ** 30 - 25, constants: { MAX_LENGTH: 2 ** 32 - 1, MAX_STRING_LENGTH: 2 ** 30 - 25 }, INSPECT_MAX_BYTES: 50,
    isUtf8: value => { try { new TextDecoder('utf-8', { fatal: true }).decode(value); return true; } catch { return false; } },
    isAscii: value => view(value).every(byte => byte < 128) };

  const pending = new Map(), watchers = new Map();
  const sync = (op, ...args) => call('node.' + op, ...args);
  const asyncCall = (op, ...args) => new Promise((resolve, reject) => { const id = sync('async', op, args); pending.set(id, { resolve, reject }); });
  const constants = sync('constants');
  let system;
  const details = () => system ??= sync('os.info');

  const makePath = windows => {
    const sep = windows ? '\\' : '/';
    const slash = path => {
      if (typeof path !== 'string') throw Object.assign(new TypeError(`The "path" argument must be of type string. Received ${inspect(path)}`), { code: 'ERR_INVALID_ARG_TYPE' });
      return windows ? path.replace(/\\/g, '/') : path;
    };
    const root = path => windows ? (path.match(/^(?:[A-Za-z]:\/|\/\/[^/]+\/[^/]+\/?|\/)/)?.[0] ?? '') : path.startsWith('/') ? '/' : '';
    const normalize = input => {
      const path = slash(input);
      if (windows && /^[A-Za-z]:[^/]/.test(path)) throw Error('Drive-relative paths are not supported');
      const prefix = root(path);
      const parts = [];
      for (const part of path.slice(prefix.length).split('/')) {
        if (!part || part === '.') continue;
        if (part === '..' && parts.length && parts.at(-1) !== '..') parts.pop();
        else if (part !== '..' || !prefix) parts.push(part);
      }
      const value = (prefix.endsWith('/') || !prefix ? prefix : prefix + '/') + parts.join('/');
      return ((value || '.') + (path.endsWith('/') && parts.length ? '/' : '')).replaceAll('/', sep);
    };
    const path = {
      sep, delimiter: windows ? ';' : ':', normalize,
      isAbsolute: value => windows ? /^(?:[A-Za-z]:\/|\/)/.test(slash(value)) : slash(value).startsWith('/'),
      join: (...parts) => normalize(parts.filter(value => slash(value)).join('/')),
      resolve(...parts) {
        let value = '';
        for (let i = parts.length - 1; i >= -1; i--) {
          const part = i >= 0 ? parts[i] : call('cwd');
          if (!slash(part)) continue;
          value = slash(part) + (value ? '/' + value : '');
          if (path.isAbsolute(part)) break;
        }
        const normalized = normalize(value);
        if (slash(normalized) === root(slash(normalized))) return normalized;
        return normalized.replace(new RegExp('\\' + sep + '$'), '') || sep;
      },
      basename(value, suffix = '') {
        const name = slash(value).replace(/\/+$/, '').split('/').at(-1) || '';
        return suffix && name !== suffix && name.endsWith(suffix) ? name.slice(0, -suffix.length) : name;
      },
      dirname(value) {
        const raw = slash(value);
        const prefix = root(raw);
        if (raw === prefix) return raw.replaceAll('/', sep);
        const s = raw.replace(/\/+$/, '');
        const index = s.lastIndexOf('/');
        if (index < 0) return '.';
        return (index < prefix.length ? prefix : s.slice(0, index) || '/').replaceAll('/', sep);
      },
      extname(value) {
        const name = path.basename(value);
        const index = name.lastIndexOf('.');
        return index <= 0 || name === '..' ? '' : name.slice(index);
      },
      relative(from, to) {
        const a = slash(path.resolve(from)).split('/');
        const b = slash(path.resolve(to)).split('/');
        if (windows && a[0].toLowerCase() !== b[0].toLowerCase()) return path.resolve(to);
        let i = 0;
        while (i < a.length && i < b.length && (windows ? a[i].toLowerCase() === b[i].toLowerCase() : a[i] === b[i])) i++;
        return [...a.slice(i).filter(Boolean).map(() => '..'), ...b.slice(i)].join(sep);
      },
      parse(value) {
        const raw = slash(value), base = path.basename(value), ext = path.extname(value);
        return { root: root(raw).replaceAll('/', sep), dir: raw.replace(/(.)\/+$/, '$1').includes('/') ? path.dirname(value) : '', base, ext, name: ext ? base.slice(0, -ext.length) : base };
      },
      format({ root = '', dir, base, name = '', ext = '' }) {
        base ||= name + (ext && !ext.startsWith('.') ? '.' : '') + ext;
        dir ||= root;
        return !dir ? base : dir === root ? dir + base : dir + sep + base;
      },
      toNamespacedPath(value) {
        if (!windows || typeof value !== 'string' || !value) return value;
        const resolved = path.resolve(value);
        return /^[A-Za-z]:\\/.test(resolved) ? '\\\\?\\' + resolved : /^\\\\[^?.]/.test(resolved) ? '\\\\?\\UNC\\' + resolved.slice(2) : value;
      },
    };
    return path;
  };
  const posix = makePath(false), win32 = makePath(true);
  const path = windows ? win32 : posix;
  posix.posix = win32.posix = posix;
  posix.win32 = win32.win32 = win32;

  const pathArg = value => value instanceof URL ? url.fileURLToPath(value) : ArrayBuffer.isView(value) ? Buffer.from(value.buffer, value.byteOffset, value.byteLength).toString() : String(value);
  const options = value => typeof value === 'string' ? { encoding: value } : value ?? {};
  const kind = Symbol('type'), S = constants.fs;
  const typed = proto => Object.assign(proto, { isFile() { return this[kind]() === S.S_IFREG; }, isDirectory() { return this[kind]() === S.S_IFDIR; }, isSymbolicLink() { return this[kind]() === S.S_IFLNK; }, isBlockDevice() { return this[kind]() === S.S_IFBLK; }, isCharacterDevice() { return this[kind]() === S.S_IFCHR; }, isFIFO() { return this[kind]() === S.S_IFIFO; }, isSocket() { return this[kind]() === S.S_IFSOCK; } });
  class Stats {
    constructor(value) { Object.assign(this, value); this.birthtimeMs ??= this.ctimeMs; for (const name of ['atime', 'mtime', 'ctime', 'birthtime']) this[name] = new Date(this[name + 'Ms']); }
    [kind]() { return this.mode & S.S_IFMT; }
  }
  class Dirent {
    #type;
    constructor(name, type, parentPath) { this.name = name; this.parentPath = parentPath; this.#type = type; }
    [kind]() { return this.#type; }
  }
  typed(Stats.prototype); typed(Dirent.prototype);
  const descriptions = { ENOENT: 'no such file or directory', EACCES: 'permission denied', EEXIST: 'file already exists', ENOTDIR: 'not a directory', EISDIR: 'illegal operation on a directory', ENOTEMPTY: 'directory not empty', EPERM: 'operation not permitted', EBUSY: 'resource busy or locked', EXDEV: 'cross-device link not permitted', EINVAL: 'invalid argument', EROFS: 'read-only file system', ENOSPC: 'no space left on device', EMFILE: 'too many open files', ELOOP: 'too many symbolic links encountered', ENAMETOOLONG: 'name too long', EBADF: 'bad file descriptor', EIO: 'i/o error' };
  const syscalls = { readFile: 'open', writeFile: 'open', appendFile: 'open', readdir: 'scandir', copyFile: 'copyfile', fstat: 'fstat', ftruncate: 'ftruncate', truncate: 'open' };
  const failure = (error, name, args) => {
    if (!descriptions[error.code]) return error;
    const syscall = syscalls[name] ?? name.toLowerCase(), file = typeof args[0] === 'string' ? args[0] : undefined, dest = ['rename', 'copyFile', 'symlink', 'link'].includes(name) ? args[1] : undefined;
    error.message = `${error.code}: ${descriptions[error.code]}, ${syscall}${file === undefined ? '' : ` '${file}'`}${dest === undefined ? '' : ` -> '${dest}'`}`;
    return Object.assign(error, { errno: constants.errno[error.code] ? -constants.errno[error.code] : undefined, syscall }, file === undefined ? {} : { path: file }, dest === undefined ? {} : { dest });
  };
  const files = { constants: S, promises: { constants: S }, Stats, Dirent };
  const chunk = (buffer, offset, length, position) => { if (typeof buffer === 'string') return [new Uint8Array(Buffer.from(buffer, length)), offset ?? null]; const bytes = view(buffer); if (offset !== null && typeof offset === 'object') ({ offset, length, position } = offset); offset ??= 0; return [bytes.subarray(offset, offset + (length ?? bytes.length - offset)), position ?? null]; };
  const seconds = value => value instanceof Date ? value.getTime() / 1000 : Number(value);
  const finish = (name, result, args) => {
    if (name === 'readFile') { const encoding = options(args[1]).encoding; return utf8.test(encoding) ? result : encoding && encoding !== 'buffer' ? bytes(result).toString(encoding) : bytes(result); }
    if (name === 'stat' || name === 'lstat' || name === 'fstat') return new Stats(result);
    if (name === 'readdir') {
      const o = options(args[1]);
      return result.map(([name, type]) => {
        if (!o.withFileTypes) return name;
        const index = Math.max(name.lastIndexOf('/'), windows ? name.lastIndexOf('\\') : -1);
        return new Dirent(name.slice(index + 1), type, index < 0 ? args[0] : path.join(args[0], name.slice(0, index)));
      });
    }
    return result ?? undefined;
  };
  const handles = new __appsFinalizers(fd => { try { sync('close', fd); } catch {} });
  const handle = fd => { const file = { fd,
    async close() { handles.unregister(file); await asyncCall('close', fd); },
    async read(buffer = Buffer.alloc(16384), offset = 0, length = buffer.length - offset, position = null) { if (!ArrayBuffer.isView(buffer)) ({ buffer = Buffer.alloc(16384), offset = 0, length = buffer.length - offset, position = null } = buffer ?? {}); const data = bytes(await asyncCall('read', fd, length, position)); buffer.set(data, offset); return { bytesRead: data.length, buffer }; },
    async write(buffer, ...args) { return { bytesWritten: await asyncCall('write', fd, ...chunk(buffer, ...args)), buffer }; },
    async readFile(o) { const chunks = []; for (;;) { const b = await asyncCall('read', fd, 1048576, null); if (!b.length) break; chunks.push(bytes(b)); } const b = Buffer.concat(chunks); const encoding = options(o).encoding; return encoding && encoding !== 'buffer' ? b.toString(encoding) : b; },
    async writeFile(data, o) { const bytes = view(data, options(o).encoding); let at = 0; while (at < bytes.length) at += await asyncCall('write', fd, bytes.subarray(at, at + 1048576), null); },
    async appendFile(data, o) { await this.writeFile(data, o); },
    async stat() { return new Stats(await asyncCall('fstat', fd)); },
    async sync() { await asyncCall('fsync', fd); }, async datasync() { await asyncCall('fsync', fd); },
    async truncate(size = 0) { await asyncCall('ftruncate', fd, size); },
    [Symbol.asyncDispose]() { return this.close(); },
  }; handles.register(file, fd, file); return file; };
  const prepare = (name, args) => {
    args = [...args];
    if (typeof args[0] !== 'number') args[0] = pathArg(args[0]);
    if (['rename', 'copyFile', 'symlink', 'link'].includes(name)) args[1] = pathArg(args[1]);
    if (['readFile', 'readdir', 'mkdir', 'rm', 'rmdir'].includes(name)) args[1] = options(args[1]);
    if (name === 'writeFile' || name === 'appendFile') { args[2] = options(args[2]); args[1] = typeof args[1] === 'string' && (!args[2].encoding || utf8.test(args[2].encoding)) ? args[1] : view(args[1], args[2].encoding); }
    if (name === 'utimes' || name === 'lutimes') { args[1] = seconds(args[1]); args[2] = seconds(args[2]); }
    if (name === 'truncate' || name === 'ftruncate') args[1] ??= 0;
    if (name === 'chmod' && typeof args[1] === 'string') args[1] = parseInt(args[1], 8);
    if (name === 'mkdir' && typeof args[1].mode === 'string') args[1].mode = parseInt(args[1].mode, 8);
    return args;
  };
  for (const name of ['readFile', 'writeFile', 'appendFile', 'mkdir', 'readdir', 'stat', 'lstat', 'fstat', 'rename', 'copyFile', 'realpath', 'readlink', 'symlink', 'link', 'chmod', 'chown', 'lchown', 'utimes', 'lutimes', 'truncate', 'access', 'unlink', 'rm', 'rmdir', 'mkdtemp', 'open', 'close', 'fsync', 'ftruncate']) {
    files[name + 'Sync'] = (...args) => {
      args = prepare(name, args);
      let result;
      try { result = sync(name, ...args); }
      catch (error) { if ((name === 'stat' || name === 'lstat') && args[1]?.throwIfNoEntry === false && error.code === 'ENOENT') return undefined; throw failure(error, name, args); }
      return finish(name, result, args);
    };
    const task = async (...args) => { args = prepare(name, args); try { return finish(name, await asyncCall(name, ...args), args); } catch (error) { throw failure(error, name, args); } };
    if (!['close', 'fsync', 'ftruncate', 'fstat'].includes(name)) files.promises[name] = name === 'open' ? async (...args) => handle(await task(...args)) : task;
    files[name] = (...args) => { const callback = args.pop(); if (typeof callback !== 'function') throw Object.assign(new TypeError('The "cb" argument must be of type function'), { code: 'ERR_INVALID_ARG_TYPE' }); task(...args).then(result => callback(null, result), callback); };
  }
  files.realpathSync.native = files.realpathSync; files.realpath.native = files.realpath;
  files.existsSync = value => { try { sync('access', pathArg(value)); return true; } catch { return false; } };
  files.readSync = (fd, buffer, offset = 0, length = buffer.byteLength - offset, position = null) => { if (offset !== null && typeof offset === 'object') ({ offset = 0, length = buffer.byteLength - offset, position = null } = offset); const data = bytes(sync('read', fd, length, position)); view(buffer).set(data, offset); return data.length; };
  files.writeSync = (fd, ...args) => sync('write', fd, ...chunk(...args));
  files.read = (fd, buffer, offset, length, position, callback) => {
    callback = [buffer, offset, length, position, callback].findLast(value => typeof value === 'function');
    if (!ArrayBuffer.isView(buffer)) ({ buffer = Buffer.alloc(16384), offset = 0, length = buffer.byteLength - offset, position = null } = typeof buffer === 'object' ? buffer : {});
    else if (typeof offset === 'object' && offset !== null) ({ offset = 0, length = buffer.byteLength - offset, position = null } = offset);
    offset = typeof offset === 'number' ? offset : 0; length = typeof length === 'number' ? length : buffer.byteLength - offset; position = typeof position === 'number' || typeof position === 'bigint' ? Number(position) : null;
    asyncCall('read', fd, length, position).then(data => { data = bytes(data); view(buffer).set(data, offset); callback(null, data.length, buffer); }, callback);
  };
  files.write = (fd, buffer, ...args) => { const callback = args.pop(); asyncCall('write', fd, ...chunk(buffer, ...args)).then(written => callback(null, written, buffer), callback); };
  class Dir {
    #entries;
    constructor(path, entries) { this.path = path; this.#entries = entries; }
    readSync() { return this.#entries.shift() ?? null; }
    read(callback) { const entry = this.readSync(); if (callback) queueMicrotask(() => callback(null, entry)); else return Promise.resolve(entry); }
    closeSync() { this.#entries = []; }
    close(callback) { this.closeSync(); if (callback) queueMicrotask(() => callback(null)); else return Promise.resolve(); }
    async *[Symbol.asyncIterator]() { for (let entry; (entry = this.readSync());) yield entry; }
  }
  files.opendirSync = (value, o) => new Dir(pathArg(value), files.readdirSync(value, { ...options(o), withFileTypes: true }));
  files.promises.opendir = async (value, o) => new Dir(pathArg(value), await files.promises.readdir(value, { ...options(o), withFileTypes: true }));
  files.opendir = (value, o, callback) => { if (typeof o === 'function') { callback = o; o = {}; } files.promises.opendir(value, o).then(dir => callback(null, dir), callback); };
  const copy = function* (from, to, o) {
    if (o.filter && !(yield ['filter', from, to])) return;
    const source = yield [o.dereference ? 'stat' : 'lstat', from];
    if (source.isDirectory()) {
      if (!o.recursive) throw Object.assign(new Error(`Recursive option is required to copy a directory: ${from}`), { code: 'ERR_FS_EISDIR', path: from });
      yield ['mkdir', to, { recursive: true }];
      for (const name of yield ['readdir', from]) yield* copy(path.join(from, name), path.join(to, name), o);
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
  const poll = () => { if (!pending.size && !watchers.size) return; for (const event of sync('poll')) { if (event.watch) { const watcher = watchers.get(event.id); if (!watcher) continue; if (event.error) watcher.emit('error', new Error(event.error)); else for (const p of event.paths) watcher.emit('change', event.event, p); } else { const item = pending.get(event.id); pending.delete(event.id); if (item) { if (event.error) item.reject(Object.assign(new Error(event.error), { code: event.code ?? undefined })); else item.resolve(event.value); } } } };
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
    constants: { signals: constants.signals, errno: constants.errno, priority: { PRIORITY_LOW: 19, PRIORITY_BELOW_NORMAL: 10, PRIORITY_NORMAL: 0, PRIORITY_ABOVE_NORMAL: -7, PRIORITY_HIGH: -14, PRIORITY_HIGHEST: -20 } },
  };

  const promisify = fn => {
    if (typeof fn !== 'function') throw Object.assign(new TypeError('The "original" argument must be of type function'), { code: 'ERR_INVALID_ARG_TYPE' });
    if (fn[promisify.custom]) return fn[promisify.custom];
    const promised = function(...args) { return new Promise((resolve, reject) => fn.call(this, ...args, (error, ...values) => error ? reject(error) : resolve(values[0]))); };
    return Object.defineProperty(promised, promisify.custom, { value: promised });
  };
  promisify.custom = Symbol.for('nodejs.util.promisify.custom');
  const tag = value => Object.prototype.toString.call(value).slice(8, -1);
  const types = {
    isAnyArrayBuffer: v => v instanceof ArrayBuffer || (typeof SharedArrayBuffer !== 'undefined' && v instanceof SharedArrayBuffer), isArrayBuffer: v => v instanceof ArrayBuffer, isSharedArrayBuffer: v => typeof SharedArrayBuffer !== 'undefined' && v instanceof SharedArrayBuffer,
    isArrayBufferView: v => ArrayBuffer.isView(v), isTypedArray: v => ArrayBuffer.isView(v) && !(v instanceof DataView), isDataView: v => v instanceof DataView,
    isDate: v => v instanceof Date, isRegExp: v => v instanceof RegExp, isMap: v => v instanceof Map, isSet: v => v instanceof Set, isWeakMap: v => v instanceof WeakMap, isWeakSet: v => v instanceof WeakSet,
    isPromise: v => v instanceof Promise, isNativeError: v => v instanceof Error, isCryptoKey: v => typeof CryptoKey !== 'undefined' && v instanceof CryptoKey,
    isAsyncFunction: v => typeof v === 'function' && tag(v).startsWith('Async'), isGeneratorFunction: v => typeof v === 'function' && tag(v).endsWith('GeneratorFunction'), isGeneratorObject: v => tag(v) === 'Generator' || tag(v) === 'AsyncGenerator',
    isMapIterator: v => tag(v) === 'Map Iterator', isSetIterator: v => tag(v) === 'Set Iterator', isModuleNamespaceObject: v => tag(v) === 'Module',
    isBoxedPrimitive: v => typeof v === 'object' && ['Number', 'String', 'Boolean', 'BigInt', 'Symbol'].includes(tag(v)),
  };
  for (const name of ['Number', 'String', 'Boolean', 'BigInt', 'Symbol']) types[`is${name}Object`] = v => typeof v === 'object' && tag(v) === name;
  for (const name of ['Uint8Array', 'Uint8ClampedArray', 'Uint16Array', 'Uint32Array', 'Int8Array', 'Int16Array', 'Int32Array', 'Float32Array', 'Float64Array', 'BigInt64Array', 'BigUint64Array']) types['is' + name] = v => ArrayBuffer.isView(v) && tag(v) === name;
  const colors = { reset: [0, 0], bold: [1, 22], dim: [2, 22], italic: [3, 23], underline: [4, 24], blink: [5, 25], inverse: [7, 27], hidden: [8, 28], strikethrough: [9, 29], doubleunderline: [21, 24], framed: [51, 54], overlined: [53, 55], black: [30, 39], red: [31, 39], green: [32, 39], yellow: [33, 39], blue: [34, 39], magenta: [35, 39], cyan: [36, 39], white: [37, 39], gray: [90, 39], grey: [90, 39], blackBright: [90, 39], redBright: [91, 39], greenBright: [92, 39], yellowBright: [93, 39], blueBright: [94, 39], magentaBright: [95, 39], cyanBright: [96, 39], whiteBright: [97, 39], bgBlack: [40, 49], bgRed: [41, 49], bgGreen: [42, 49], bgYellow: [43, 49], bgBlue: [44, 49], bgMagenta: [45, 49], bgCyan: [46, 49], bgWhite: [47, 49], bgGray: [100, 49], bgGrey: [100, 49], bgBlackBright: [100, 49], bgRedBright: [101, 49], bgGreenBright: [102, 49], bgYellowBright: [103, 49], bgBlueBright: [104, 49], bgMagentaBright: [105, 49], bgCyanBright: [106, 49], bgWhiteBright: [107, 49] };
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
    getSystemErrorName: errno => Object.keys(constants.errno).find(name => constants.errno[name] === -errno),
  };
  util.debug = util.debuglog;

  const url = {
    URL, URLSearchParams,
    fileURLToPath(value) {
      const u = typeof value === 'string' ? new URL(value) : value;
      if (u.protocol === 'apps:') return path.join(info.appPath, decodeURIComponent(u.pathname));
      if (u.protocol !== 'file:') throw Object.assign(new TypeError('The URL must be of scheme file'), { code: 'ERR_INVALID_URL_SCHEME' });
      if (/%2f/i.test(u.pathname) || (windows && /%5c/i.test(u.pathname))) throw Object.assign(new TypeError(`File URL path must not include encoded ${windows ? '\\ or /' : '/'} characters`), { code: 'ERR_INVALID_FILE_URL_PATH' });
      const pathname = decodeURIComponent(u.pathname);
      if (!windows) { if (u.hostname && u.hostname !== 'localhost') throw Object.assign(new TypeError(`File URL host must be "localhost" or empty on ${info.platform}`), { code: 'ERR_INVALID_FILE_URL_HOST' }); return pathname; }
      if (u.hostname) return `\\\\${u.hostname}${pathname.replaceAll('/', '\\')}`;
      if (!/^\/[A-Za-z]:/.test(pathname)) throw Object.assign(new TypeError('File URL path must be absolute'), { code: 'ERR_INVALID_FILE_URL_PATH' });
      return pathname.slice(1).replaceAll('/', '\\');
    },
    pathToFileURL(value) {
      let resolved = path.resolve(value);
      if (/[\\/]$/.test(value) && !resolved.endsWith(path.sep)) resolved += '/';
      const u = new URL('file://');
      if (windows && resolved.startsWith('\\\\')) { const [host, ...rest] = resolved.slice(2).split('\\'); u.hostname = host; resolved = '/' + rest.join('/'); }
      else if (windows) resolved = '/' + resolved.replaceAll('\\', '/');
      u.pathname = resolved.replace(/[%\n\r\t#?\\]/g, c => '%' + c.charCodeAt(0).toString(16).toUpperCase().padStart(2, '0'));
      return u;
    },
    format(value, { auth = true, fragment = true, search = true } = {}) {
      if (typeof value === 'string') return value;
      if (!(value instanceof URL)) throw Object.assign(new TypeError('The "urlObject" argument must be a URL'), { code: 'ERR_INVALID_ARG_TYPE' });
      const u = new URL(value.href);
      if (!auth) { u.username = ''; u.password = ''; }
      if (!fragment) u.hash = '';
      if (!search) u.search = '';
      return u.href;
    },
    urlToHttpOptions: u => ({ protocol: u.protocol, hostname: u.hostname.replace(/^\[|\]$/g, ''), hash: u.hash, search: u.search, pathname: u.pathname, path: u.pathname + u.search, href: u.href, ...(u.port ? { port: Number(u.port) } : {}), ...(u.username || u.password ? { auth: `${decodeURIComponent(u.username)}:${decodeURIComponent(u.password)}` } : {}) }),
  };

  class StringDecoder {
    #decoder; #rest = new Uint8Array(0);
    constructor(encoding = 'utf8') { if (!Buffer.isEncoding(encoding)) throw Object.assign(new TypeError('Unknown encoding: ' + encoding), { code: 'ERR_UNKNOWN_ENCODING' }); this.encoding = normalize(encoding); if (this.encoding === 'utf8' || this.encoding === 'utf16le') this.#decoder = new TextDecoder(this.encoding === 'utf8' ? 'utf-8' : 'utf-16le', { ignoreBOM: true }); }
    write(value) {
      if (typeof value === 'string') return value;
      const data = view(value);
      if (this.#decoder) return this.#decoder.decode(data, { stream: true });
      if (!this.encoding.startsWith('base64')) return Buffer.from(data.buffer, data.byteOffset, data.byteLength).toString(this.encoding);
      const all = Buffer.concat([this.#rest, data]), size = all.length - all.length % 3;
      this.#rest = all.subarray(size);
      return all.subarray(0, size).toString(this.encoding);
    }
    end(value) { const text = value === undefined ? '' : this.write(value); if (this.#decoder) return text + this.#decoder.decode(); const rest = this.#rest; this.#rest = new Uint8Array(0); return text + Buffer.from(rest).toString(this.encoding); }
  }

  class AssertionError extends Error {
    constructor({ message, actual, expected, operator } = {}) { super(message ?? `${inspect(actual)} ${operator} ${inspect(expected)}`); Object.assign(this, { name: 'AssertionError', code: 'ERR_ASSERTION', actual, expected, operator, generatedMessage: message === undefined }); }
  }
  const fail = (actual, expected, message, operator, generated) => { if (message instanceof Error) throw message; throw new AssertionError({ message: message ?? generated, actual, expected, operator }); };
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
    fail(message = 'Failed') { if (message instanceof Error) throw message; throw new AssertionError({ message, operator: 'fail' }); },
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
    throws(fn, expected, message) { if (typeof expected === 'string') { message = expected; expected = undefined; } try { fn(); } catch (error) { matches(error, expected, message, 'throws'); return; } fail(undefined, expected, message, 'throws', `Missing expected exception${expected?.name ? ` (${expected.name})` : ''}.`); },
    async rejects(input, expected, message) { if (typeof expected === 'string') { message = expected; expected = undefined; } try { await (typeof input === 'function' ? input() : input); } catch (error) { matches(error, expected, message, 'rejects'); return; } fail(undefined, expected, message, 'rejects', `Missing expected rejection${expected?.name ? ` (${expected.name})` : ''}.`); },
    doesNotThrow(fn, expected, message) { if (typeof expected === 'string') message = expected; try { fn(); } catch (error) { fail(error, expected, message, 'doesNotThrow', `Got unwanted exception.\nActual message: "${error?.message}"`); } },
    async doesNotReject(input, expected, message) { if (typeof expected === 'string') message = expected; try { await (typeof input === 'function' ? input() : input); } catch (error) { fail(error, expected, message, 'doesNotReject', `Got unwanted rejection.\nActual message: "${error?.message}"`); } },
    ifError(value) { if (value !== null && value !== undefined) throw value instanceof Error ? value : new AssertionError({ message: `ifError got unwanted exception: ${inspect(value)}`, actual: value, expected: null, operator: 'ifError' }); },
  });
  const strict = (value, message) => assert.ok(value, message);
  Object.assign(strict, assert, { equal: assert.strictEqual, notEqual: assert.notStrictEqual, deepEqual: assert.deepStrictEqual, notDeepEqual: assert.notDeepStrictEqual });
  assert.strict = strict.strict = strict;

  const querystring = {
    escape: value => encodeURIComponent(value),
    unescape: value => { try { return decodeURIComponent(value); } catch { return value; } },
    stringify(value, separator = '&', equals = '=', { encodeURIComponent: escape = querystring.escape } = {}) {
      if (value === null || typeof value !== 'object') return '';
      const text = item => typeof item === 'string' || typeof item === 'bigint' || typeof item === 'boolean' || (typeof item === 'number' && Number.isFinite(item)) ? escape(String(item)) : '';
      return Object.keys(value).map(name => { const item = value[name], prefix = escape(name) + equals; return Array.isArray(item) ? item.map(entry => prefix + text(entry)).join(separator) : prefix + text(item); }).filter(Boolean).join(separator);
    },
    parse(text, separator = '&', equals = '=', { maxKeys = 1000, decodeURIComponent: unescape = querystring.unescape } = {}) {
      const result = { __proto__: null };
      if (typeof text !== 'string' || !text) return result;
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

  const family = value => ({ IPv4: 4, IPv6: 6 })[value] ?? Number(value ?? 0);
  const resolve = async (hostname, o) => { try { return await asyncCall('lookup', String(hostname), family(o.family)); } catch (error) { throw Object.assign(error, { message: `getaddrinfo ENOTFOUND ${hostname}`, code: 'ENOTFOUND', errno: undefined, syscall: 'getaddrinfo', hostname }); } };
  const dns = {
    lookup(hostname, o, callback) { if (typeof o === 'function') { callback = o; o = {}; } o = typeof o === 'object' && o !== null ? o : { family: o }; resolve(hostname, o).then(list => o.all ? callback(null, list) : callback(null, list[0].address, list[0].family), callback); },
    promises: { lookup: async (hostname, o = {}) => { o = typeof o === 'object' && o !== null ? o : { family: o }; const list = await resolve(hostname, o); return o.all ? list : list[0]; } },
  };
  dns.lookup[promisify.custom] = dns.promises.lookup;

  const tty = { isatty: fd => Number.isInteger(fd) && sync('isatty', fd) };
  return { Buffer, buffer, fs: files, path, os, util, url, string_decoder: { StringDecoder }, assert, querystring, dns, tty, details, sync, asyncCall, poll };
};
