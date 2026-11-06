globalThis.__appsElectron = (call, EventEmitter, ready) => {
  const native = (op, args = null) => call('native.' + op, args);

  class NativeImage {
    #png; #width; #height; #template = false;
    constructor(png, width, height) { this.#png = png; this.#width = width; this.#height = height; }
    isEmpty() { return !this.#width || !this.#height; }
    getSize() { return { width: this.#width, height: this.#height }; }
    getAspectRatio() { return this.#height ? this.#width / this.#height : 1; }
    getScaleFactors() { return [1]; }
    toPNG() { return Buffer.from(this.#png); }
    toJPEG(quality = 90) { return this.isEmpty() ? Buffer.alloc(0) : Buffer.from(image({ op: 'jpeg', data: this.#png, quality }).png, 'base64'); }
    toBitmap() { return this.isEmpty() ? Buffer.alloc(0) : Buffer.from(image({ op: 'bitmap', data: this.#png }).bytes, 'base64'); }
    getBitmap() { return this.toBitmap(); }
    toDataURL() { return 'data:image/png;base64,' + this.toPNG().toString('base64'); }
    resize({ width, height }) {
      if (this.isEmpty()) return this;
      const ratio = this.#width / this.#height;
      width ??= height * ratio; height ??= width / ratio;
      return result(image({ op: 'resize', data: this.#png, width: Math.round(width), height: Math.round(height) }));
    }
    crop({ x, y, width, height }) { return this.isEmpty() ? this : result(image({ op: 'crop', data: this.#png, x: Math.round(x), y: Math.round(y), width: Math.round(width), height: Math.round(height) })); }
    setTemplateImage(value) { this.#template = !!value; }
    isTemplateImage() { return this.#template; }
    addRepresentation() {}
  }
  const image = options => native('image', { ...options, data: Buffer.from(options.data).toString('base64') });
  const empty = () => new NativeImage(Buffer.alloc(0), 0, 0);
  const result = value => new NativeImage(Buffer.from(value.png, 'base64'), value.width, value.height);
  const decode = bytes => { try { return result(image({ op: 'decode', data: bytes })); } catch { return empty(); } };
  const nativeImage = Object.freeze({
    createEmpty: empty,
    createFromBuffer: buffer => buffer?.length ? decode(buffer) : empty(),
    createFromPath(path) { try { return decode(__apps.fs.readFileSync(String(path))); } catch { return empty(); } },
    createFromDataURL(url) { const match = /^data:[^,]*?(;base64)?,(.*)$/s.exec(String(url)); return match ? decode(match[1] ? Buffer.from(match[2], 'base64') : Buffer.from(decodeURIComponent(match[2]))) : empty(); },
    createFromBitmap: (buffer, { width, height }) => result(image({ op: 'fromBitmap', data: buffer, width, height })),
    createFromNamedImage: () => empty(),
  });

  const handlers = new Map();
  const protocol = Object.freeze({
    registerSchemesAsPrivileged(schemes) { if (ready()) throw new Error('protocol.registerSchemesAsPrivileged should be called before app is ready'); for (const { scheme } of schemes) if (typeof scheme !== 'string') throw new TypeError('Expected a scheme'); },
    handle(scheme, handler) { if (typeof handler !== 'function') throw new TypeError('Expected a handler'); if (handlers.has(scheme)) throw new Error(`Failed to register protocol: ${scheme}`); handlers.set(scheme, handler); },
    unhandle(scheme) { if (!handlers.delete(scheme)) throw new Error(`Failed to unregister protocol: ${scheme}`); },
    isProtocolHandled: scheme => handlers.has(scheme),
  });
  const handled = input => { const url = typeof input === 'string' ? input : input?.url ?? String(input), scheme = /^([a-z][a-z\d+.-]*):/i.exec(url)?.[1]?.toLowerCase(); return scheme ? handlers.get(scheme) : undefined; };

  // Electron's built-in updater needs a signed app with a feed. There is none here, so it only reports that.
  const autoUpdater = Object.assign(new EventEmitter(), {
    getFeedURL: () => '', setFeedURL() {}, quitAndInstall() {},
    checkForUpdates() { queueMicrotask(() => autoUpdater.emit('error', new Error('Could not get code signature for running application'))); },
  });

  // WARNING: capturing the screen needs ScreenCaptureKit, which `@pierre/apps` doesn't link, so there are no sources.
  const desktopCapturer = Object.freeze({ getSources: async () => [] });
  class ClipboardItem {
    #items;
    constructor(items) { this.#items = new Map(Object.entries(items)); }
    get types() { return [...this.#items.keys()]; }
    async getType(type) { const item = await this.#items.get(type); if (item === undefined) throw new DOMException('The type was not found', 'NotFoundError'); return item instanceof Blob ? item : new Blob([item], { type }); }
  }
  const fileIcon = async path => { try { return result(native('image', { op: 'fileIcon', path: String(path), data: '' })); } catch { return empty(); } };
  return { nativeImage, protocol, handled, autoUpdater, desktopCapturer, ClipboardItem, fileIcon };
};
