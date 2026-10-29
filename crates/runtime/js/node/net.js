globalThis.__appsNet = (call, EventEmitter, streams, Buffer) => {
  const sockets = new Map(), servers = new Map(), { encode, decode } = globalThis.__appsBytes;
  delete globalThis.__appsBytes;
  class Socket extends streams.Duplex {
    constructor(options = {}) {
      super(options); this.connecting = false; this.pending = true; this.bytesRead = 0; this.bytesWritten = 0; this._referenced = true; this.allowHalfOpen = !!options.allowHalfOpen;
      this._read = () => { if (this._paused && !this.destroyed) { this._paused = false; call('web.tcp.pause', this._socket, false); } };
      this._write = (chunk, _encoding, callback) => {
        if (this._socket === undefined) { callback(new Error('Socket is not connected')); return; }
        this._written = callback;
        try { call('web.tcp.write', this._socket, encode(chunk)); } catch (error) { this._written = null; callback(error); }
      };
      this._final = callback => { this._finished = callback; if (this._socket === undefined) callback(); else call('web.tcp.end', this._socket); };
    }
    connect(options, host, listener) {
      if (this._socket !== undefined) throw new Error('Socket is already connected');
      if (typeof options === 'number') options = { port: options, ...(typeof host === 'string' ? { host } : {}) };
      else if (typeof options === 'string') options = { path: options };
      if (typeof host === 'function') listener = host;
      if (listener) this.once('connect', listener);
      this.connecting = true; this.pending = true;
      this._socket = call('web.tcp.connect', options); sockets.set(this._socket, this);
      if (options.signal) {
        const abort = () => this.destroy(options.signal.reason instanceof Error ? options.signal.reason : new Error('Socket connection aborted'));
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
    }
    setNoDelay(value = true) { if (this._socket !== undefined) call('web.tcp.noDelay', this._socket, !!value); else this.once('connect', () => this.setNoDelay(value)); return this; }
    setKeepAlive(value = false, delay = 0) { if (this._socket !== undefined) call('web.tcp.keepAlive', this._socket, !!value, delay); else this.once('connect', () => this.setKeepAlive(value, delay)); return this; }
    setTimeout(delay, listener) { this._timeout = delay; if (listener) this.once('timeout', listener); this._touch(); return this; }
    _touch() { clearTimeout(this._timer); if (this._timeout > 0) this._timer = setTimeout(() => this.emit('timeout'), this._timeout); }
    address() { return this.localAddress ? { address: this.localAddress, port: this.localPort, family: this.localAddress.includes(':') ? 'IPv6' : 'IPv4' } : {}; }
    ref() { this._referenced = true; return this; }
    unref() { this._referenced = false; return this; }
    get readyState() { return this.destroyed ? 'closed' : this.connecting ? 'opening' : this.readable && this.writable ? 'open' : this.readable ? 'readOnly' : this.writable ? 'writeOnly' : 'closed'; }
  }
  class Server extends EventEmitter {
    constructor(options, listener) { super(); if (typeof options === 'function') { listener = options; options = {}; } this._options = options ?? {}; this._referenced = true; this._connections = new Set(); this.listening = false; if (listener) this.on('connection', listener); }
    listen(options, ...args) {
      if (this.listening) throw new Error('Server is already listening');
      const callback = typeof args.at(-1) === 'function' ? args.pop() : undefined;
      if (callback) this.once('listening', callback);
      if (typeof options === 'number') options = { port: options, ...(typeof args[0] === 'string' ? { host: args[0] } : {}) };
      else if (typeof options === 'string') options = { path: options };
      try {
        const result = call('web.tcp.listen', options); this._server = result.id; this._address = result.address.path ?? result.address;
        servers.set(result.id, this); this.listening = true; queueMicrotask(() => this.emit('listening'));
      } catch (error) { queueMicrotask(() => this.emit('error', error)); }
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
  const connect = (...args) => new Socket().connect(...args);
  const api = { Socket, Server, createServer: (...args) => new Server(...args), connect, createConnection: connect,
    isIP: value => call('web.tcp.ip', String(value)), isIPv4: value => call('web.tcp.ip', String(value)) === 4, isIPv6: value => call('web.tcp.ip', String(value)) === 6 };
  return { api,
    poll() {
      if (!sockets.size && !servers.size) return;
      for (const event of call('web.tcp.poll')) {
        if (event.server !== undefined) {
          const server = servers.get(event.server); if (!server) continue;
          const socket = new Socket(server._options); socket._socket = event.id; socket._server = server; socket.pending = false;
          sockets.set(event.id, socket); server._connections.add(socket); server.emit('connection', socket); continue;
        }
        const socket = sockets.get(event.id); if (!socket) continue;
        if (event.kind === 'connect') { Object.assign(socket, event.address); socket.connecting = socket.pending = false; socket.emit('connect'); socket.emit('ready'); }
        else if (event.kind === 'data') { const data = Buffer.from(decode(event.bytes).buffer); socket.bytesRead += data.length; socket._touch(); if (!socket.push(data) && !socket._paused) { socket._paused = true; call('web.tcp.pause', event.id, true); } }
        else if (event.kind === 'written') { socket.bytesWritten += event.bytes; socket._touch(); const callback = socket._written; socket._written = null; callback?.(); }
        else if (event.kind === 'finish') { const callback = socket._finished; socket._finished = null; callback?.(); }
        else if (event.kind === 'end') { socket.push(null); if (!socket.allowHalfOpen && socket.writable) socket.end(); }
        else if (event.kind === 'error') socket.destroy(new Error(event.message));
        else if (event.kind === 'close') socket.destroy();
      }
    },
    pending: () => [...sockets.values(), ...servers.values()].some(value => value._referenced),
  };
};
