(() => {
  const call = globalThis.__appsGPU;
  delete globalThis.__appsGPU;
  const objects = new WeakMap(), rawBytes = Symbol();
  const garbage = new __appsFinalizers(({ device, id }) => { try { call('gpu.release', device, [id]); } catch {} });
  const adapters = new __appsFinalizers(id => call('gpu.releaseAdapter', id));
  const devices = new __appsFinalizers(id => { try { call('gpu.destroyDevice', id, null); } catch {} });
  const waiting = new Map();
  const settle = (device, token) => new Promise((resolve, reject) => waiting.set(token, { device, resolve, reject }));
  globalThis.__appsGPUPoll = () => {
    if (waiting.size) for (const [token, value] of call('gpu.poll')) { const entry = waiting.get(token); waiting.delete(token); entry?.resolve(value); }
  };
  const wire = (value, refs) => {
    if (objects.has(value)) { refs?.push(value); return objects.get(value); }
    if (value?.[rawBytes]) return value[rawBytes];
    if (Array.isArray(value) || ArrayBuffer.isView(value)) return Array.from(value, v => wire(v, refs));
    if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, wire(v, refs)]));
    return value;
  };
  const bytes = (data, offset = 0, size) => {
    const view = ArrayBuffer.isView(data);
    const unit = view ? (data.BYTES_PER_ELEMENT ?? 1) : 1;
    const length = data.byteLength;
    offset *= unit;
    size = size === undefined ? length - offset : size * unit;
    if (!Number.isSafeInteger(offset) || !Number.isSafeInteger(size) || offset < 0 || size < 0 || offset + size > length) throw new RangeError('Invalid GPU data range');
    return new Uint8Array(view ? data.buffer : data, (view ? data.byteOffset : 0) + offset, size);
  };
  const binary = data => ({ [rawBytes]: data });
  class GPUValidationError extends Error {}
  class GPUOutOfMemoryError extends Error {}
  class GPUObject {
    constructor(device, id, label = '') {
      this.label = label;
      this._device = device;
      objects.set(this, id);
      garbage.register(this, { device: objects.get(device), id }, this);
    }
  }
  class GPUBuffer extends GPUObject {
    constructor(device, id, desc) {
      super(device, id, desc.label);
      this.size = desc.size;
      this.usage = desc.usage;
      this.mapState = desc.mappedAtCreation ? 'mapped' : 'unmapped';
      this._mode = desc.mappedAtCreation ? 2 : 0;
      this._offset = 0;
      this._data = desc.mappedAtCreation ? new ArrayBuffer(desc.size) : null;
      this._ranges = [];
    }
    async mapAsync(mode, offset = 0, size = this.size - offset) {
      if (this.mapState !== 'unmapped' || ![1, 2].includes(mode)) throw new Error('Buffer cannot be mapped');
      if (offset % 8 || size % 4 || offset < 0 || size < 0 || offset + size > this.size) throw new RangeError('Invalid mapped range');
      this.mapState = 'pending';
      try { this._token = this._device._call('mapBuffer', [this, mode, offset, size]); } catch (error) { this.mapState = 'unmapped'; throw error; }
      const data = await settle(this._device, this._token);
      if (!(data instanceof Uint8Array)) { this.mapState = 'unmapped'; throw new Error('Buffer mapping failed'); }
      this._data = data.buffer;
      this._offset = offset; this._mode = mode; this.mapState = 'mapped';
    }
    getMappedRange(offset = 0, size = this.size - offset) {
      if (this.mapState !== 'mapped') throw new Error('Buffer is not mapped');
      if (offset % 8 || size % 4 || offset < this._offset || size < 0 || offset + size > this._offset + this._data.byteLength) throw new RangeError('Invalid mapped range');
      if (this._ranges.some(([start, data]) => offset < start + data.byteLength && start < offset + size)) throw new Error('Mapped ranges overlap');
      const data = this._data.slice(offset - this._offset, offset - this._offset + size);
      this._ranges.push([offset, data]);
      return data;
    }
    unmap() {
      if (this.mapState === 'unmapped') return;
      this._device._call('unmapBuffer', [this, this._mode === 2 ? this._ranges.map(([offset, data]) => [offset, binary(bytes(data))]) : []]);
      if (this.mapState === 'pending') { waiting.get(this._token)?.reject(new DOMException('Buffer mapping was aborted', 'AbortError')); waiting.delete(this._token); }
      for (const [, data] of this._ranges) data.transfer?.(0);
      this._ranges = []; this._data = null; this.mapState = 'unmapped';
    }
    destroy() {
      if (this._destroyed) return;
      this.unmap(); this._device._call('destroyBuffer', this); this._destroyed = true; garbage.unregister(this);
    }
  }
  class GPUTexture extends GPUObject {
    constructor(device, id, desc) {
      super(device, id, desc.label);
      const size = desc.size;
      this.width = Array.isArray(size) ? size[0] : size.width;
      this.height = (Array.isArray(size) ? size[1] : size.height) ?? 1;
      this.depthOrArrayLayers = (Array.isArray(size) ? size[2] : size.depthOrArrayLayers) ?? 1;
      this.mipLevelCount = desc.mipLevelCount ?? 1;
      this.sampleCount = desc.sampleCount ?? 1;
      this.dimension = desc.dimension ?? '2d'; this.format = desc.format; this.usage = desc.usage;
    }
    createView(desc = {}) { return new GPUTextureView(this._device, this._device._call('createView', [this, desc]), desc.label); }
    destroy() {
      if (this._destroyed) return;
      this._device._call('destroyTexture', this); this._destroyed = true; garbage.unregister(this);
    }
  }
  class GPUTextureView extends GPUObject {}
  class GPUSampler extends GPUObject {}
  class GPUBindGroupLayout extends GPUObject {}
  class GPUBindGroup extends GPUObject {}
  class GPUPipelineLayout extends GPUObject {}
  class GPUShaderModule extends GPUObject {
    async getCompilationInfo() { return this._device._call('getCompilationInfo', this); }
  }
  class GPURenderPipeline extends GPUObject {
    getBindGroupLayout(index) { return new GPUBindGroupLayout(this._device, this._device._call('getBindGroupLayout', [this, index])); }
  }
  class GPUComputePipeline extends GPURenderPipeline {}
  class GPUCommands {
    constructor(device, desc = {}) { this._device = device; this.label = desc.label ?? ''; this._commands = []; this._refs = []; this._ended = false; }
    _record(name, args) {
      if (this._ended || this._pass) throw new Error('GPU encoder is not open');
      this._commands.push([name, ...wire(args, this._refs)]);
    }
    setBindGroup(index, group, offsets = [], start = 0, length = offsets.length - start) {
      this._record('setBindGroup', [index, group, Array.from(offsets).slice(start, start + length)]);
    }
  }
  class GPURenderPassEncoder extends GPUCommands {
    end() { if (this._ended) throw new Error('Render pass has ended'); this._ended = true; this._parent._pass = null; }
    executeBundles(bundles) {
      for (const bundle of bundles) {
        if (bundle._device !== this._device) throw new Error('Render bundle belongs to another device');
        this._refs.push(bundle);
      }
      this._record('executeBundles', [bundles.map(b => b._commands)]);
    }
  }
  class GPUComputePassEncoder extends GPURenderPassEncoder {}
  class GPURenderBundleEncoder extends GPUCommands {
    finish(desc = {}) {
      if (this._ended) throw new Error('Render bundle encoder has ended');
      this._ended = true;
      return { label: desc.label ?? '', _device: this._device, _commands: this._commands, _refs: this._refs };
    }
  }
  class GPUCommandEncoder extends GPUCommands {
    beginRenderPass(desc) {
      if (this._ended || this._pass) throw new Error('GPU encoder is not open');
      const pass = new GPURenderPassEncoder(this._device, desc); pass._parent = this; this._pass = pass;
      this._refs.push(pass); this._commands.push(['render', wire(desc, this._refs), pass._commands]);
      return pass;
    }
    beginComputePass(desc = {}) {
      if (this._ended || this._pass) throw new Error('GPU encoder is not open');
      const pass = new GPUComputePassEncoder(this._device, desc); pass._parent = this; this._pass = pass;
      this._refs.push(pass); this._commands.push(['compute', wire(desc, this._refs), pass._commands]);
      return pass;
    }
    finish(desc = {}) {
      if (this._ended || this._pass) throw new Error('GPU encoder is not open');
      this._ended = true;
      return { label: desc.label ?? '', _device: this._device, _commands: this._commands, _refs: this._refs, _submitted: false };
    }
  }
  for (const name of ['pushDebugGroup', 'popDebugGroup', 'insertDebugMarker']) GPUCommands.prototype[name] = function (...args) { this._record(name, args); };
  for (const name of ['setPipeline', 'setVertexBuffer', 'setIndexBuffer', 'draw', 'drawIndexed', 'drawIndirect', 'drawIndexedIndirect']) {
    GPURenderPassEncoder.prototype[name] = GPURenderBundleEncoder.prototype[name] = function (...args) { this._record(name, args); };
  }
  for (const name of ['setViewport', 'setScissorRect', 'setStencilReference', 'setBlendConstant']) GPURenderPassEncoder.prototype[name] = function (...args) { this._record(name, args); };
  for (const name of ['dispatchWorkgroups', 'dispatchWorkgroupsIndirect']) GPUComputePassEncoder.prototype[name] = function (...args) { this._record(name, args); };
  for (const name of ['copyBufferToBuffer', 'copyTextureToTexture', 'copyTextureToBuffer', 'copyBufferToTexture', 'clearBuffer']) GPUCommandEncoder.prototype[name] = function (...args) { this._record(name, args); };
  class GPUQueue {
    constructor(device) { this._device = device; this.label = ''; }
    submit(buffers) {
      const seen = new Set();
      for (const b of buffers) {
        if (b._device !== this._device || b._submitted || seen.has(b)) throw new Error('Invalid or already submitted command buffer');
        seen.add(b);
      }
      this._device._call('submit', buffers.map(b => b._commands), true);
      for (const b of buffers) { b._submitted = true; b._refs = []; }
    }
    writeBuffer(buffer, offset, data, dataOffset = 0, size) {
      const value = bytes(data, dataOffset, size);
      if (offset % 4 || value.length % 4) throw new RangeError('Buffer writes must be aligned to 4 bytes');
      this._device._call('writeBuffer', [buffer, offset, binary(value)]);
    }
    writeTexture(destination, data, dataLayout, size) { this._device._call('writeTexture', [destination, binary(bytes(data)), dataLayout, size]); }
    async onSubmittedWorkDone() { await settle(this._device, this._device._call('workDone', null)); }
    copyExternalImageToTexture() { throw new Error('External image uploads are not supported; use writeTexture with pixel data'); }
  }
  class GPUDevice {
    constructor(info, adapterInfo) {
      objects.set(this, info.id); this.label = ''; this.features = new Set(info.features); this.limits = Object.freeze(info.limits); this.adapterInfo = adapterInfo;
      devices.register(this, info.id, this);
      this.queue = new GPUQueue(this); this._scopes = []; this._events = new Map();
      this.lost = new Promise(resolve => { this._resolveLost = resolve; });
    }
    _call(op, params, wired) {
      if (this._destroyed) throw new Error('GPU device is destroyed');
      const result = call('gpu.' + op, objects.get(this), wired ? params : wire(params));
      if (result?.gpuError) {
        const error = new (result.kind === 'out-of-memory' ? GPUOutOfMemoryError : GPUValidationError)(result.gpuError);
        const scope = this._scopes.findLast(s => s.filter === result.kind);
        if (scope) { scope.error ??= error; return result.value; }
        const event = { type: 'uncapturederror', error };
        this.onuncapturederror?.(event);
        for (const fn of this._events.get(event.type) ?? []) fn(event);
        throw error;
      }
      return result;
    }
    createBuffer(desc) { return new GPUBuffer(this, this._call('createBuffer', desc), desc); }
    createTexture(desc) { return new GPUTexture(this, this._call('createTexture', desc), desc); }
    createCommandEncoder(desc = {}) { return new GPUCommandEncoder(this, desc); }
    createRenderBundleEncoder(desc) { return new GPURenderBundleEncoder(this, desc); }
    async createRenderPipelineAsync(desc) { return this.createRenderPipeline(desc); }
    async createComputePipelineAsync(desc) { return this.createComputePipeline(desc); }
    pushErrorScope(filter) { if (!['validation', 'out-of-memory', 'internal'].includes(filter)) throw new TypeError('Invalid GPU error filter'); this._scopes.push({ filter, error: null }); }
    async popErrorScope() { if (!this._scopes.length) throw new Error('No GPU error scope'); return this._scopes.pop().error; }
    addEventListener(type, fn) { const list = this._events.get(type) ?? []; list.push(fn); this._events.set(type, list); }
    removeEventListener(type, fn) { this._events.set(type, (this._events.get(type) ?? []).filter(f => f !== fn)); }
    destroy() {
      if (this._destroyed) return;
      this._call('destroyDevice', null); devices.unregister(this); this._destroyed = true;
      for (const [token, entry] of waiting) if (entry.device === this) { waiting.delete(token); entry.reject(new Error('GPU device is destroyed')); }
      this._resolveLost({ reason: 'destroyed', message: 'Device destroyed' });
    }
    importExternalTexture() { throw new Error('External textures are not supported'); }
    createQuerySet() { throw new Error('GPU query sets are not supported'); }
  }
  for (const [name, Type] of Object.entries({ Sampler: GPUSampler, BindGroupLayout: GPUBindGroupLayout, BindGroup: GPUBindGroup, PipelineLayout: GPUPipelineLayout, ShaderModule: GPUShaderModule, RenderPipeline: GPURenderPipeline, ComputePipeline: GPUComputePipeline })) {
    GPUDevice.prototype['create' + name] = function (desc = {}) { return new Type(this, this._call('create' + name, desc), desc.label); };
  }
  class GPUAdapter {
    constructor(info) { this._id = info.id; adapters.register(this, info.id, this); this.features = new Set(info.features); this.limits = Object.freeze(info.limits); this.info = Object.freeze(info.info); this.isFallbackAdapter = info.info.isFallbackAdapter; }
    async requestDevice(desc = {}) { return new GPUDevice(call('gpu.device', this._id, desc), this.info); }
    async requestAdapterInfo() { return this.info; }
  }
  class GPUCanvasContext {
    constructor(canvas, node) { this.canvas = canvas; this._node = node; this._configuration = null; this._texture = null; }
    configure(config) {
      if (!(config.device instanceof GPUDevice)) throw new TypeError('Expected a GPUDevice');
      if (!['bgra8unorm', 'rgba8unorm'].includes(config.format)) throw new TypeError('Canvas format must be bgra8unorm or rgba8unorm');
      if (config.alphaMode && !['opaque', 'premultiplied'].includes(config.alphaMode)) throw new TypeError('Invalid canvas alphaMode');
      if (config.colorSpace && config.colorSpace !== 'srgb') throw new Error('Only the srgb canvas color space is supported');
      this.unconfigure(); this._configuration = { ...config, usage: config.usage ?? 16, alphaMode: config.alphaMode ?? 'opaque', colorSpace: 'srgb' };
    }
    unconfigure() { if (this._texture) garbage.unregister(this._texture); call('gpu.resetCanvas', this._node); this._texture = null; this._configuration = null; }
    getConfiguration() { return this._configuration ? { ...this._configuration } : null; }
    getCurrentTexture() {
      const config = this._configuration;
      if (!config) throw new Error('GPU canvas context is not configured');
      const width = this.canvas.width, height = this.canvas.height;
      if (!this._texture || this._texture._destroyed || this._texture.width !== width || this._texture.height !== height) {
        if (this._texture) garbage.unregister(this._texture);
        const id = config.device._call('canvasTexture', { canvas: this._node, width, height, format: config.format, usage: config.usage, alphaMode: config.alphaMode });
        this._texture = new GPUTexture(config.device, id, { size: [width, height], format: config.format, usage: config.usage });
      }
      return this._texture;
    }
    _resize() { if (this._texture) garbage.unregister(this._texture); this._texture = null; call('gpu.resetCanvas', this._node); }
  }
  globalThis.navigator ??= {};
  Object.assign(globalThis.navigator, { gpu: {
    async requestAdapter(options = {}) { const info = call('gpu.adapter', options); return info ? new GPUAdapter(info) : null; },
    getPreferredCanvasFormat() { return 'bgra8unorm'; },
    wgslLanguageFeatures: new Set(),
  } });
  Object.assign(globalThis, { GPUAdapter, GPUDevice, GPUQueue, GPUBuffer, GPUTexture, GPUTextureView, GPUSampler, GPUShaderModule, GPUBindGroup, GPUBindGroupLayout, GPUPipelineLayout, GPURenderPipeline, GPUComputePipeline, GPUCommandEncoder, GPURenderPassEncoder, GPUComputePassEncoder, GPURenderBundleEncoder, GPUCanvasContext, GPUValidationError, GPUOutOfMemoryError,
    GPUBufferUsage: Object.freeze({ MAP_READ: 1, MAP_WRITE: 2, COPY_SRC: 4, COPY_DST: 8, INDEX: 16, VERTEX: 32, UNIFORM: 64, STORAGE: 128, INDIRECT: 256, QUERY_RESOLVE: 512 }),
    GPUTextureUsage: Object.freeze({ COPY_SRC: 1, COPY_DST: 2, TEXTURE_BINDING: 4, STORAGE_BINDING: 8, RENDER_ATTACHMENT: 16 }),
    GPUShaderStage: Object.freeze({ VERTEX: 1, FRAGMENT: 2, COMPUTE: 4 }),
    GPUMapMode: Object.freeze({ READ: 1, WRITE: 2 }), GPUColorWrite: Object.freeze({ RED: 1, GREEN: 2, BLUE: 4, ALPHA: 8, ALL: 15 }),
  });
})();
