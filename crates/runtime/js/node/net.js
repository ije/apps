globalThis.__appsNet = (call, EventEmitter, streams, Buffer, { encode, decode }) => {
  const sockets = new Map(), servers = new Map();
  const fail = (code, message, Type = TypeError) => Object.assign(new Type(message), { code });
  const port = (value, name = 'Port') => {
    const number = typeof value === 'string' && value.trim() ? Number(value) : value;
    if ((typeof value !== 'number' && typeof value !== 'string') || !Number.isInteger(number) || number < 0 || number > 65535) throw fail('ERR_SOCKET_BAD_PORT', `${name} should be >= 0 and < 65536. Received ${typeof value === 'string' ? `type string ('${value}')` : `type ${typeof value} (${String(value)})`}.`, RangeError);
    return number;
  };
  const pipe = value => typeof value === 'string' && !(Number(value) >= 0);
  const normalize = args => {
    const callback = typeof args.at(-1) === 'function' ? args.at(-1) : undefined, [first, second] = args;
    const options = first !== null && typeof first === 'object' ? { ...first } : pipe(first) ? { path: first } : { port: first, ...(typeof second === 'string' ? { host: second } : {}) };
    return [options, callback];
  };
  const errno = code => { const value = globalThis.__apps?.os.constants.errno[code]; return value === undefined ? undefined : -value; };
  const failure = event => {
    if (event.alert) return Object.assign(new Error(`SSL alert number ${event.alert}`), { code: 'ERR_SSL_' + ({ 40: 'SSLV3_ALERT_HANDSHAKE_FAILURE', 42: 'SSLV3_ALERT_BAD_CERTIFICATE', 46: 'SSLV3_ALERT_CERTIFICATE_UNKNOWN', 48: 'TLSV1_ALERT_UNKNOWN_CA', 70: 'TLSV1_ALERT_PROTOCOL_VERSION', 80: 'TLSV1_ALERT_INTERNAL_ERROR', 112: 'TLSV1_UNRECOGNIZED_NAME', 116: 'TLSV13_ALERT_CERTIFICATE_REQUIRED', 120: 'TLSV1_ALERT_NO_APPLICATION_PROTOCOL' }[event.alert] ?? 'TLS_ALERT_' + event.alert) });
    if (event.message) return Object.assign(new Error(event.message), { code: event.code });
    const target = event.syscall === 'getaddrinfo' ? ` ${event.hostname}` : event.port !== undefined ? ` ${event.address}:${event.port}` : event.address !== undefined ? ` ${event.address}` : '';
    const message = event.syscall === 'listen' ? `listen ${event.code}: ${event.description}${target}` : `${event.syscall} ${event.code}${target}`;
    const error = Object.assign(new Error(message), { errno: event.errno ?? errno(event.code), code: event.code, syscall: event.syscall });
    for (const name of ['address', 'port', 'hostname']) if (event[name] !== undefined) error[name] = event[name];
    return error;
  };
  class Socket extends streams.Duplex {
    constructor(options = {}) {
      super({ ...options, emitClose: false }); this.connecting = false; this.pending = true; this.bytesRead = 0; this.bytesWritten = 0; this._referenced = true; this.allowHalfOpen = !!options.allowHalfOpen;
      this._read = () => { if (this._paused && !this.destroyed) { this._paused = false; call('web.tcp.pause', this._socket, false); } };
      this._write = (chunk, _encoding, callback) => {
        if (this._socket === undefined) { callback(fail('ERR_SOCKET_CLOSED', 'Socket is closed', Error)); return; }
        this._written = callback;
        try { call('web.tcp.write', this._socket, encode(chunk)); } catch (error) { this._written = null; callback(error); }
      };
      this._final = callback => { this._finished = callback; if (this._socket === undefined) callback(); else call('web.tcp.end', this._socket); };
      if (options.fd !== undefined) { this._socket = call('web.tcp.open', options.fd); sockets.set(this._socket, this); this.pending = false; }
    }
    connect(...args) {
      if (this._socket !== undefined) throw fail('ERR_SOCKET_CONNECTION', 'Socket is already connected', Error);
      const [options, listener] = normalize(args);
      if (options.path === undefined) options.port = port(options.port, 'Port');
      if (listener) this.once('connect', listener);
      this.connecting = true; this.pending = true; this._lookup = options.path === undefined && !call('web.tcp.ip', options.host ??= 'localhost') ? options.host : undefined;
      this._socket = call('web.tcp.connect', options); sockets.set(this._socket, this);
      if (options.timeout) this.setTimeout(options.timeout);
      if (options.signal) {
        const abort = () => this.destroy(Object.assign(new Error('The operation was aborted', { cause: options.signal.reason }), { name: 'AbortError', code: 'ABORT_ERR' }));
        if (options.signal.aborted) queueMicrotask(abort); else { options.signal.addEventListener('abort', abort, { once: true }); this.once('close', () => options.signal.removeEventListener('abort', abort)); }
      }
      return this;
    }
    _destroy(error, callback) {
      if (this._socket !== undefined) { call('web.tcp.destroy', this._socket); sockets.delete(this._socket); }
      clearTimeout(this._timer); this.connecting = false; this.pending = false;
      this._server?._connections.delete(this);
      this._server?._finishClose();
      callback(error);
      queueMicrotask(() => this.emit('close', !!error));
    }
    setNoDelay(value = true) { if (this._socket !== undefined) call('web.tcp.noDelay', this._socket, !!value); else this.once('connect', () => this.setNoDelay(value)); return this; }
    setKeepAlive(value = false, delay = 0) { if (this._socket !== undefined) call('web.tcp.keepAlive', this._socket, !!value, delay); else this.once('connect', () => this.setKeepAlive(value, delay)); return this; }
    setTimeout(delay, listener) { this._timeout = delay; if (listener) this.once('timeout', listener); this._touch(); return this; }
    _touch() { clearTimeout(this._timer); if (this._timeout > 0) this._timer = setTimeout(() => this.emit('timeout'), this._timeout); }
    address() { return this.localAddress ? { address: this.localAddress, family: this.localAddress.includes(':') ? 'IPv6' : 'IPv4', port: this.localPort } : {}; }
    ref() { this._referenced = true; return this; }
    unref() { this._referenced = false; return this; }
    get readyState() { return this.destroyed ? 'closed' : this.connecting ? 'opening' : this.readable && this.writable ? 'open' : this.readable ? 'readOnly' : this.writable ? 'writeOnly' : 'closed'; }
  }
  class Server extends EventEmitter {
    constructor(options, listener) { super(); if (typeof options === 'function') { listener = options; options = {}; } this._options = options ?? {}; this._referenced = true; this._connections = new Set(); this.listening = false; if (listener) this.on('connection', listener); }
    listen(...args) {
      if (this.listening) throw fail('ERR_SERVER_ALREADY_LISTEN', 'Listen method has been called more than once without closing.', Error);
      const [options, callback] = normalize(args);
      if (options.path === undefined) options.port = options.port == null ? 0 : port(options.port, 'options.port');
      if (callback) this.once('listening', callback);
      const result = call('web.tcp.listen', options);
      if (result.error) { queueMicrotask(() => this.emit('error', failure(result.error))); return this; }
      this._server = result.id; this._address = result.address.path ?? result.address;
      servers.set(result.id, this); this.listening = true; queueMicrotask(() => this.emit('listening'));
      return this;
    }
    address() { return this.listening ? this._address : null; }
    close(callback) {
      if (callback) this.once('close', callback);
      if (this.listening) { call('web.tcp.stop', this._server); servers.delete(this._server); this.listening = false; }
      this._closing = true; this._finishClose(); return this;
    }
    _finishClose() { if (this._closing && !this._connections.size) { this._closing = false; queueMicrotask(() => this.emit('close')); } }
    getConnections(callback) { queueMicrotask(() => callback(null, this._connections.size)); return this; }
    ref() { this._referenced = true; return this; }
    unref() { this._referenced = false; return this; }
  }
  const connect = (...args) => { const [options] = normalize(args); return new Socket(options).connect(...args); };
  const parseIP = (text, family) => {
    text = String(text);
    if (family === 'ipv4') { const parts = text.split('.'); return parts.length === 4 && parts.every(part => /^(?:0|[1-9]\d{0,2})$/.test(part) && part < 256) ? parts.reduce((n, part) => n << 8n | BigInt(part), 0xffffn) : undefined; }
    if (!/^[\da-f:.]+$/i.test(text)) return;
    let host;
    try { host = new URL(`http://[${text}]`).hostname.slice(1, -1); } catch { return; }
    const [head, tail] = host.split('::'), left = head ? head.split(':') : [], right = tail ? tail.split(':') : [];
    return [...left, ...Array(8 - left.length - right.length).fill('0'), ...right].reduce((n, group) => n << 16n | BigInt('0x' + group), 0n);
  };
  const familyOf = value => { const family = String(value).toLowerCase(); if (family !== 'ipv4' && family !== 'ipv6') throw fail('ERR_INVALID_ARG_VALUE', `The property 'options.family' must be one of: 'ipv4', 'ipv6'. Received '${value}'`); return family; };
  class SocketAddress {
    #address; #port; #family; #flowlabel;
    constructor(options = {}) {
      const family = familyOf(options.family ?? 'ipv4'), address = options.address ?? (family === 'ipv4' ? '127.0.0.1' : '::'), flowlabel = options.flowlabel ?? 0;
      if (typeof address !== 'string' || parseIP(address, family) === undefined) throw fail('ERR_INVALID_ADDRESS', 'Invalid socket address', Error);
      if (!Number.isInteger(flowlabel) || flowlabel < 0 || flowlabel > 0xffffffff) throw fail('ERR_OUT_OF_RANGE', `The value of "options.flowlabel" is out of range. It must be >= 0 && <= 4294967295. Received ${flowlabel}`, RangeError);
      this.#address = family === 'ipv6' ? new URL(`http://[${address}]`).hostname.slice(1, -1) : address; this.#port = port(options.port ?? 0); this.#family = family; this.#flowlabel = flowlabel;
    }
    get address() { return this.#address; }
    get port() { return this.#port; }
    get family() { return this.#family; }
    get flowlabel() { return this.#flowlabel; }
    toJSON() { return { address: this.#address, port: this.#port, family: this.#family, flowlabel: this.#flowlabel }; }
    [Symbol.for('nodejs.util.inspect.custom')](depth, options, inspect) { return `SocketAddress ${inspect(this.toJSON(), options)}`; }
    static isSocketAddress(value) { return typeof value === 'object' && value !== null && #address in value; }
    static parse(input) {
      if (typeof input !== 'string') throw fail('ERR_INVALID_ARG_TYPE', `The "input" argument must be of type string. Received ${typeof input}`);
      try { const { hostname, port } = URL.parse(`http://${input}`); return hostname.startsWith('[') ? new SocketAddress({ address: hostname.slice(1, -1), port: port | 0, family: 'ipv6' }) : new SocketAddress({ address: hostname, port: port | 0 }); } catch {}
    }
  }
  class BlockList {
    #rules = [];
    #read(value, family = 'ipv4') {
      const text = SocketAddress.isSocketAddress(value) ? value.address : value, type = SocketAddress.isSocketAddress(value) ? value.family : familyOf(family), ip = parseIP(text, type);
      if (ip === undefined) throw fail('ERR_INVALID_ADDRESS', 'Invalid socket address', Error);
      return [ip, text, type === 'ipv4' ? 'IPv4' : 'IPv6'];
    }
    addAddress(address, family) { const [ip, text, type] = this.#read(address, family); this.#rules.unshift([`Address: ${type} ${text}`, ip, ip]); }
    addRange(start, end, family) {
      const [low, from, type] = this.#read(start, family), [high, to] = this.#read(end, family);
      if (high < low) throw fail('ERR_INVALID_ARG_VALUE', `The argument 'start' must come before end. Received '${from}'`);
      this.#rules.unshift([`Range: ${type} ${from}-${to}`, low, high]);
    }
    addSubnet(network, prefix, family) {
      const [ip, text, type] = this.#read(network, family), bits = type === 'IPv4' ? 32 : 128;
      if (!Number.isInteger(prefix) || prefix < 0 || prefix > bits) throw fail('ERR_OUT_OF_RANGE', `The value of "prefix" is out of range. It must be >= 0 && <= ${bits}. Received ${prefix}`, RangeError);
      const mask = (1n << BigInt(bits - prefix)) - 1n;
      this.#rules.unshift([`Subnet: ${type} ${text}/${prefix}`, ip & ~mask, ip | mask]);
    }
    check(address, family) { try { const [ip] = this.#read(address, family); return this.#rules.some(([, low, high]) => ip >= low && ip <= high); } catch { return false; } }
    get rules() { return this.#rules.map(([text]) => text); }
    toJSON() { return this.rules; }
    fromJSON(data) {
      for (const rule of typeof data === 'string' ? JSON.parse(data) : data) {
        const [, kind, type, value] = /^(Address|Range|Subnet): IPv([46]) (.+)$/.exec(rule) ?? [], family = 'ipv' + type;
        if (kind === 'Address') this.addAddress(value, family);
        else if (kind === 'Range') this.addRange(...value.split('-'), family);
        else if (kind === 'Subnet') this.addSubnet(value.slice(0, value.lastIndexOf('/')), +value.slice(value.lastIndexOf('/') + 1), family);
      }
    }
    [Symbol.for('nodejs.util.inspect.custom')](depth, options, inspect) { return `BlockList ${inspect({ rules: this.rules }, options)}`; }
    static isBlockList(value) { return typeof value === 'object' && value !== null && #rules in value; }
  }
  const unsupported = what => fail('ERR_FEATURE_UNAVAILABLE_ON_PLATFORM', `TLS ${what} are not supported`, Error);
  class TLSSocket extends Socket {
    constructor(socket, options = {}) {
      const settings = options.secureContext?.context ?? options;
      if (options.isServer) throw unsupported('servers');
      if (settings.cert || settings.key || settings.pfx) throw unsupported('client certificates');
      super(options); this.encrypted = true; this.authorized = false; this.authorizationError = null; this.alpnProtocol = false; this.servername = options.servername || options.host || 'localhost';
      this._settings = { servername: this.servername, ca: settings.ca && [].concat(settings.ca).join('\n'), alpn: [...options.ALPNProtocols ?? []].map(item => Buffer.from(item).toString('latin1')), reject: !!(options.rejectUnauthorized ?? globalThis.process?.env.NODE_TLS_REJECT_UNAUTHORIZED !== '0') };
      if (!socket) return;
      if (socket._socket === undefined) throw fail('ERR_SOCKET_CLOSED', 'Socket is closed', Error);
      this._socket = socket._socket; socket._upgrade = this; this.once('close', () => socket.destroy());
      call('web.tcp.tls', this._socket, this._settings);
    }
    connect(...args) { super.connect(...args); call('web.tcp.tls', this._socket, this._settings); return this; }
    getProtocol() { return this._secure?.protocol ?? null; }
    getCipher() {
      const { cipher, protocol } = this._secure ?? {};
      if (!cipher) return;
      const standardName = 'TLS_' + (cipher >> 8 === 0x13 ? '' : `ECDHE_${[0xc02f, 0xc030, 0xcca8].includes(cipher) ? 'RSA' : 'ECDSA'}_WITH_`) + ([0x1301, 0xc02b, 0xc02f].includes(cipher) ? 'AES_128_GCM_SHA256' : [0x1302, 0xc02c, 0xc030].includes(cipher) ? 'AES_256_GCM_SHA384' : 'CHACHA20_POLY1305_SHA256');
      return { name: protocol === 'TLSv1.3' ? standardName : standardName.slice(4).replace('_WITH_', '_').replace(/AES_(\d+)/, 'AES$1').replace('POLY1305_SHA256', 'POLY1305').replaceAll('_', '-'), standardName, version: protocol };
    }
    getPeerCertificate() { return this._secure ? { raw: this._secure.raw } : {}; }
  }
  class TLSServer extends Server { constructor() { super(); throw unsupported('servers'); } }
  const tls = {
    TLSSocket, Server: TLSServer,
    connect(...args) {
      const callback = typeof args.at(-1) === 'function' ? args.pop() : undefined, options = { ...args.slice(1).find(value => value !== null && typeof value === 'object'), ...normalize(args)[0] };
      const socket = new TLSSocket(options.socket, options);
      if (callback) socket.once('secureConnect', callback);
      if (!options.socket) socket.connect(options); else if (options.timeout) socket.setTimeout(options.timeout);
      return socket;
    },
    createServer() { throw unsupported('servers'); }, createSecureContext: (options = {}) => ({ context: options }), rootCertificates: Object.freeze([]), getCACertificates: () => [], setDefaultCACertificates() {}, getCiphers: () => [], checkServerIdentity() {},
    DEFAULT_MIN_VERSION: 'TLSv1.2', DEFAULT_MAX_VERSION: 'TLSv1.3', DEFAULT_ECDH_CURVE: 'auto', CLIENT_RENEG_LIMIT: 3, CLIENT_RENEG_WINDOW: 600,
  };
  let autoSelect = false, autoSelectTimeout = 250;
  const api = { Socket, Server, BlockList, SocketAddress, createServer: (...args) => new Server(...args), connect, createConnection: connect,
    getDefaultAutoSelectFamily: () => autoSelect, setDefaultAutoSelectFamily: value => { autoSelect = !!value; },
    getDefaultAutoSelectFamilyAttemptTimeout: () => autoSelectTimeout, setDefaultAutoSelectFamilyAttemptTimeout: value => { autoSelectTimeout = Math.max(10, Number(value)); },
    isIP: value => call('web.tcp.ip', String(value)), isIPv4: value => call('web.tcp.ip', String(value)) === 4, isIPv6: value => call('web.tcp.ip', String(value)) === 6 };
  return { api, tls, normalize, port, failure,
    poll() {
      if (!sockets.size && !servers.size) return;
      for (const event of call('web.tcp.poll')) {
        if (event.server !== undefined) {
          const server = servers.get(event.server); if (!server) continue;
          const socket = new Socket(server._options); Object.assign(socket, event.address); socket._socket = event.id; socket._server = server; socket.pending = false;
          sockets.set(event.id, socket); server._connections.add(socket); server.emit('connection', socket); continue;
        }
        const socket = sockets.get(event.id); if (!socket) continue;
        if (event.kind === 'connect') { if (socket._server) continue; Object.assign(socket, event.address); socket.connecting = socket.pending = false; if (socket._lookup) socket.emit('lookup', null, socket.remoteAddress, socket.remoteFamily === 'IPv6' ? 6 : 4, socket._lookup); socket.emit('connect'); socket.emit('ready'); }
        else if (event.kind === 'data') { const data = Buffer.from(decode(event.bytes).buffer); socket.bytesRead += data.length; socket._touch(); if (!socket.push(data) && !socket._paused) { socket._paused = true; call('web.tcp.pause', event.id, true); } }
        else if (event.kind === 'written') { socket.bytesWritten += event.bytes; socket._touch(); const callback = socket._written; socket._written = null; callback?.(); }
        else if (event.kind === 'tls') { const next = socket._upgrade; if (next) { sockets.set(event.id, next); Object.assign(next, event.address, { connecting: false, pending: false, _paused: socket._paused }); socket._socket = undefined; socket._paused = false; } }
        else if (event.kind === 'secure') { socket.authorized = !event.error; socket.authorizationError = event.error ?? null; socket.alpnProtocol = event.alpn ?? false; socket._secure = { ...event, raw: Buffer.from(decode(event.bytes).buffer) }; socket.emit('secureConnect'); }
        else if (event.kind === 'finish') { const callback = socket._finished; socket._finished = null; callback?.(); }
        else if (event.kind === 'end') { socket.push(null); if (!socket.allowHalfOpen && socket.writable) socket.end(); }
        else if (event.kind === 'error') socket.destroy(failure(event));
        else if (event.kind === 'close') { if (socket._readableState.ended && !socket.readableEnded && socket.readableFlowing) socket.once('end', () => socket.destroy()); else socket.destroy(); }
      }
    },
    pending: () => [...sockets.values(), ...servers.values()].some(value => value._referenced),
  };
};
