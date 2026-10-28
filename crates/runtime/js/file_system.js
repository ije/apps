(() => {
  const handles = new WeakMap(), accesses = new WeakMap(), writers = new WeakMap();
  const locks = typeof __appsFinalizers === 'function' ? new __appsFinalizers(state => state.node.locks.delete(state)) : null;
  const quota = 1024 ** 3;
  const root = { name: '', kind: 'directory', children: new Map(), parent: null, removed: false };
  const valid = handle => {
    const node = handles.get(handle);
    if (!node) throw new TypeError('Invalid file-system handle');
    for (let current = node; current; current = current.parent) if (current.removed) throw new DOMException('Entry no longer exists', 'NotFoundError');
    return node;
  };
  const nameOf = value => {
    const name = String(value);
    if (!name || name === '.' || name === '..' || /[\0/\\]/.test(name)) throw new TypeError('Invalid entry name');
    return name;
  };
  const position = value => {
    const number = Number(value);
    if (!Number.isSafeInteger(number) || number < 0) throw new TypeError('Expected a nonnegative safe integer');
    return number;
  };
  let used = 0;
  const resize = (state, size) => {
    if (size + used - (state.node?.size ?? state.size) > quota) throw new DOMException('Storage quota exceeded', 'QuotaExceededError');
    if (size > state.data.length) { const buffer = new Uint8Array(Math.min(quota, Math.max(size, state.data.length * 2, 4096))); buffer.set(state.data.subarray(0, state.size)); state.data = buffer; }
    if (size < state.size) state.data.fill(0, size, state.size);
    else if (size > state.size) state.data.fill(0, state.size, size);
    state.size = size;
  };
  const wrap = node => {
    const result = node.kind === 'directory' ? new FileSystemDirectoryHandle(handles) : new FileSystemFileHandle(handles);
    handles.set(result, node); return result;
  };
  const entry = (handle, name, kind, create) => {
    const parent = valid(handle); name = nameOf(name);
    let node = parent.children.get(name);
    if (node && node.kind !== kind) throw new DOMException('Entry has a different kind', 'TypeMismatchError');
    if (!node) {
      if (!create) throw new DOMException('Entry does not exist', 'NotFoundError');
      node = { name, kind, parent, removed: false, ...(kind === 'directory' ? { children: new Map() } : { data: new Uint8Array(), size: 0, modified: Date.now(), locks: new Set() }) };
      parent.children.set(name, node);
    }
    return wrap(node);
  };
  class FileSystemHandle {
    constructor(token) { if (token !== handles) throw new TypeError('Illegal constructor'); }
    get name() { return handles.get(this).name; }
    get kind() { return handles.get(this).kind; }
    async isSameEntry(other) { if (!handles.has(other)) throw new TypeError('Expected a file-system handle'); return handles.get(this) === handles.get(other); }
    async queryPermission(options = {}) { valid(this); if (!['read', 'readwrite'].includes(options.mode ?? 'read')) throw new TypeError('Invalid permission mode'); return 'granted'; }
    requestPermission(options) { return this.queryPermission(options); }
    async remove(options = {}) { const node = valid(this); if (!node.parent) throw new DOMException('Cannot remove the storage root', 'InvalidModificationError'); await wrap(node.parent).removeEntry(node.name, options); }
  }
  class FileSystemDirectoryHandle extends FileSystemHandle {
    async getDirectoryHandle(name, options = {}) { return entry(this, name, 'directory', !!options.create); }
    async getFileHandle(name, options = {}) { return entry(this, name, 'file', !!options.create); }
    async removeEntry(name, options = {}) {
      const parent = valid(this), node = parent.children.get(nameOf(name));
      if (!node) throw new DOMException('Entry does not exist', 'NotFoundError');
      if (node.kind === 'directory' && node.children.size && !options.recursive) throw new DOMException('Directory is not empty', 'InvalidModificationError');
      const nodes = [node]; let removed = 0;
      while (nodes.length) { const current = nodes.pop(); if (current.locks?.size) throw new DOMException('File is locked', 'NoModificationAllowedError'); if (current.children) { for (const child of current.children.values()) nodes.push(child); } else removed += current.size; }
      parent.children.delete(node.name); node.removed = true; used -= removed;
    }
    async resolve(descendant) {
      const ancestor = valid(this), path = []; let node = valid(descendant);
      while (node !== ancestor) { if (!node.parent) return null; path.unshift(node.name); node = node.parent; }
      return path;
    }
    async *entries() { for (const [name, node] of valid(this).children) if (!node.removed) yield [name, wrap(node)]; }
    async *keys() { for await (const [name] of this) yield name; }
    async *values() { for await (const [, handle] of this) yield handle; }
    [Symbol.asyncIterator]() { return this.entries(); }
  }
  class FileSystemFileHandle extends FileSystemHandle {
    async getFile() { const node = valid(this); return new File([node.data.subarray(0, node.size)], node.name, { lastModified: node.modified }); }
    async createWritable(options = {}) {
      const node = valid(this), mode = options.mode ?? 'siloed';
      if (!['siloed', 'exclusive'].includes(mode)) throw new TypeError('Invalid writable mode');
      if ([...node.locks].some(lock => lock.mode !== 'siloed') || (mode === 'exclusive' && node.locks.size)) throw new DOMException('File is locked', 'NoModificationAllowedError');
      const state = { node, data: options.keepExistingData ? node.data.slice(0, node.size) : new Uint8Array(), size: options.keepExistingData ? node.size : 0, cursor: 0, mode, closed: false };
      node.locks.add(state);
      const stream = new FileSystemWritableFileStream(writers, state);
      return stream;
    }
    async createSyncAccessHandle(options = {}) {
      const node = valid(this), mode = options.mode ?? 'readwrite';
      if (!['readwrite', 'read-only', 'readwrite-unsafe'].includes(mode)) throw new TypeError('Invalid access mode');
      if (node.locks.size && (mode === 'readwrite' || [...node.locks].some(lock => lock.mode !== mode))) throw new DOMException('File is locked', 'NoModificationAllowedError');
      const result = new FileSystemSyncAccessHandle(accesses), state = { node, mode, cursor: 0, closed: false }; node.locks.add(state); accesses.set(result, state); locks?.register(result, state); return result;
    }
  }
  class FileSystemWritableFileStream extends WritableStream {
    constructor(token, state) {
      if (token !== writers) throw new TypeError('Illegal constructor');
      super({
        async write(chunk) {
          try {
            let type = 'write', data = chunk, at = state.cursor;
            if (chunk && typeof chunk === 'object' && !(chunk instanceof Blob) && !(chunk instanceof ArrayBuffer) && !ArrayBuffer.isView(chunk)) {
              type = String(chunk.type);
              if (type === 'seek') { if (chunk.position === undefined) throw new TypeError('Missing position'); state.cursor = position(chunk.position); return; }
              if (type === 'truncate') { if (chunk.size === undefined) throw new TypeError('Missing size'); resize(state, position(chunk.size)); state.cursor = Math.min(state.cursor, state.size); return; }
              if (type !== 'write' || chunk.data === undefined) throw new TypeError('Invalid write command');
              data = chunk.data; if (chunk.position !== undefined) at = position(chunk.position);
            }
            const input = data instanceof Blob ? new Uint8Array(await data.arrayBuffer()) : data instanceof ArrayBuffer ? new Uint8Array(data) : ArrayBuffer.isView(data) ? new Uint8Array(data.buffer, data.byteOffset, data.byteLength) : new TextEncoder().encode(String(data));
            resize(state, Math.max(state.size, at + input.byteLength)); state.data.set(input, at); state.cursor = at + input.byteLength;
          } catch (error) { state.closed = true; state.node.locks.delete(state); throw error; }
        },
        close() {
          try {
            if (used - state.node.size + state.size > quota) throw new DOMException('Storage quota exceeded', 'QuotaExceededError');
            used += state.size - state.node.size; state.node.data = state.data; state.node.size = state.size; state.node.modified = Date.now();
          } finally { state.closed = true; state.node.locks.delete(state); }
        },
        abort() { state.closed = true; state.node.locks.delete(state); },
      });
      writers.set(this, state); locks?.register(this, state);
      Object.defineProperty(this, 'mode', { value: state.mode, enumerable: true });
    }
    write(data) {
      if (this.locked) return Promise.reject(new TypeError('Stream is locked'));
      const writer = this.getWriter();
      return writer.write(data).finally(() => writer.releaseLock());
    }
    seek(position) { return this.write({ type: 'seek', position }); }
    truncate(size) { return this.write({ type: 'truncate', size }); }
  }
  const access = (handle, write = false) => {
    const state = accesses.get(handle);
    if (!state || state.closed) throw new DOMException('Access handle is closed', 'InvalidStateError');
    if (write && state.mode === 'read-only') throw new DOMException('Access handle is read-only', 'NoModificationAllowedError');
    return state;
  };
  const view = value => { if (value instanceof ArrayBuffer) return new Uint8Array(value); if (!ArrayBuffer.isView(value)) throw new TypeError('Expected an ArrayBuffer or view'); return new Uint8Array(value.buffer, value.byteOffset, value.byteLength); };
  class FileSystemSyncAccessHandle {
    constructor(token) { if (token !== accesses) throw new TypeError('Illegal constructor'); }
    get mode() { return accesses.get(this).mode; }
    read(buffer, options = {}) { const state = access(this), output = view(buffer), at = position(options.at ?? state.cursor), count = Math.min(output.byteLength, Math.max(0, state.node.size - at)); output.set(state.node.data.subarray(at, at + count)); state.cursor = at + count; return count; }
    write(buffer, options = {}) {
      const state = access(this, true), input = view(buffer), at = position(options.at ?? state.cursor);
      const size = Math.max(state.node.size, at + input.byteLength), previous = state.node.size;
      resize(state.node, size); used += size - previous;
      state.node.data.set(input, at); state.node.modified = Date.now(); state.cursor = at + input.byteLength; return input.byteLength;
    }
    truncate(size) {
      const state = access(this, true); size = position(size);
      const previous = state.node.size; resize(state.node, size); used += size - previous;
      state.node.modified = Date.now(); state.cursor = Math.min(state.cursor, size);
    }
    getSize() { return access(this).node.size; }
    flush() { access(this); }
    close() { const state = accesses.get(this); if (!state || state.closed) return; state.closed = true; state.node.locks.delete(state); }
  }
  Object.assign(globalThis, { FileSystemHandle, FileSystemDirectoryHandle, FileSystemFileHandle, FileSystemWritableFileStream, FileSystemSyncAccessHandle });
  navigator.storage = Object.freeze({ getDirectory: async () => wrap(root), estimate: async () => ({ usage: used, quota }), persisted: async () => false, persist: async () => false });
})();
