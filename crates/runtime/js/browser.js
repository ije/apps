globalThis.__appsBrowser = (call, EventEmitter, isReady = () => true) => {
  const contents = new Map(), pending = new Map(), partitions = new Map(), layouts = new WeakMap(), owners = new WeakMap();
  let nextRequest = 0;
  class WebContents extends EventEmitter {
    #destroyed = false;
    #url = '';
    #title = '';
    #loading = false;
    #zoom = 1;
    #back = false;
    #forward = false;
    #devTools;
    constructor(options = {}) {
      super();
      Object.defineProperty(this, 'id', { value: call('browser.create', { partition: options.partition || 'persist:default', javascript: options.javascript ?? true, inspectable: options.devTools ?? true }) });
      this.#devTools = options.devTools !== false;
      this.session = Object.freeze({
        cookies: Object.freeze({
          get: (filter = {}) => this.#request('cookies.get', { filter }),
          set: details => this.#request('cookies.set', { details }),
          remove: (url, name) => this.#request('cookies.remove', { filter: { url: String(url), name: String(name) } }),
        }),
        clearStorageData: () => this.#request('clearStorage', {}),
      });
      contents.set(this.id, this);
    }
    loadURL(url) {
      if (this.#destroyed) return Promise.reject(new Error('Browser is destroyed'));
      return new Promise((resolve, reject) => {
        const success = () => { clean(); resolve(); };
        const failure = (_event, _code, description) => { clean(); reject(new Error(description ?? 'WebContents was destroyed')); };
        const clean = () => { this.off('did-finish-load', success); this.off('did-fail-load', failure); this.off('destroyed', failure); };
        this.once('did-finish-load', success); this.once('did-fail-load', failure); this.once('destroyed', failure);
        try { call('browser.command', this.id, 'loadURL', String(url)); }
        catch (error) { clean(); reject(error); }
      });
    }
    loadFile(file, options = {}) {
      const url = __apps.url.pathToFileURL(__apps.path.resolve(file));
      if (options.query) url.search = new URLSearchParams(options.query).toString();
      if (options.search !== undefined) url.search = options.search;
      if (options.hash !== undefined) url.hash = options.hash;
      return this.loadURL(url.href);
    }
    #request(operation, arguments_) {
      if (this.#destroyed) return Promise.reject(new Error('Browser is destroyed'));
      return new Promise((resolve, reject) => {
        const id = ++nextRequest;
        pending.set(id, { view: this.id, resolve, reject });
        try { call('browser.command', this.id, operation, { ...arguments_, id }); }
        catch (error) { pending.delete(id); reject(error); }
      });
    }
    executeJavaScript(script) { return this.#request('evaluate', { script: String(script) }); }
    capturePage(rect) {
      return this.#request('capture', rect ? { rect } : {}).then(image => {
        const bytes = Uint8Array.from(atob(image.png), char => char.charCodeAt(0));
        return Object.freeze({ toPNG: () => bytes.slice(), toDataURL: () => 'data:image/png;base64,' + image.png, getSize: () => ({ width: image.width, height: image.height }), isEmpty: () => !bytes.length });
      });
    }
    getURL() { return this.#url; }
    getTitle() { return this.#title; }
    isLoading() { return this.#loading; }
    canGoBack() { return this.#back; }
    canGoForward() { return this.#forward; }
    goBack() { call('browser.command', this.id, 'back', null); }
    goForward() { call('browser.command', this.id, 'forward', null); }
    reload() { call('browser.command', this.id, 'reload', null); }
    stop() { call('browser.command', this.id, 'stop', null); }
    focus() { call('browser.command', this.id, 'focus', null); }
    getZoomFactor() { return this.#zoom; }
    setZoomFactor(value) {
      if (!Number.isFinite(value) || value < 0.25 || value > 5) throw new RangeError('Zoom must be between 0.25 and 5');
      call('browser.command', this.id, 'zoom', value); this.#zoom = value;
    }
    openDevTools() { if (this.#devTools) call('browser.command', this.id, 'inspect', null); }
    isDestroyed() { return this.#destroyed; }
    close() {
      if (this.#destroyed) return;
      this.#destroyed = true;
      call('browser.destroy', this.id);
      contents.delete(this.id);
      for (const [id, request] of pending) if (request.view === this.id) { pending.delete(id); request.reject(new Error('Browser is destroyed')); }
      this.emit('destroyed');
    }
    _event(event) {
      if (event.url !== undefined) this.#url = event.url;
      if (event.title !== undefined) this.#title = event.title;
      if (event.canGoBack !== undefined) this.#back = event.canGoBack;
      if (event.canGoForward !== undefined) this.#forward = event.canGoForward;
      if (event.type === 'did-start-loading') this.#loading = true;
      if (event.type === 'did-finish-load' || event.type === 'did-fail-load') this.#loading = false;
      if (event.type === 'closed') { this.close(); return; }
      if (event.type === 'did-fail-load') this.emit(event.type, {}, event.code ?? -2, event.error ?? 'Navigation failed', event.url ?? this.#url, true, -1, -1);
      else if (event.type === 'page-title-updated') this.emit(event.type, {}, this.#title, true);
      else if (event.type === 'did-navigate') this.emit(event.type, {}, this.#url, -1, '');
      else if (event.type === 'did-start-loading' || event.type === 'did-finish-load') this.emit(event.type);
      else this.emit(event.type, event);
      if (event.type === 'did-finish-load' || event.type === 'did-fail-load') this.emit('did-stop-loading');
    }
  }
  const sync = (view, reorder = false) => {
    const state = layouts.get(view), parent = layouts.get(state.parent);
    const window = parent?.window ?? state.root;
    const x = (parent?.x ?? 0) + state.bounds.x, y = (parent?.y ?? 0) + state.bounds.y;
    const visible = state.visible && (parent?.shown ?? true);
    const webContents = view.webContents;
    if (webContents && owners.get(webContents) === view && !webContents.isDestroyed()) {
      if (window && (window !== state.window || reorder)) call('browser.attach', webContents.id, window.id);
      else if (!window && state.window) call('browser.detach', webContents.id);
      call('browser.bounds', webContents.id, [x, y, state.bounds.width, state.bounds.height]);
      if (visible !== state.shown) call('browser.command', webContents.id, 'visible', visible);
    }
    Object.assign(state, { window, x, y, shown: visible });
    for (const child of state.children) sync(child, reorder);
  };
  class View extends EventEmitter {
    constructor() {
      super();
      if (!isReady()) throw Error('Call app.whenReady() before creating a view');
      layouts.set(this, { parent: null, root: null, window: null, children: [], bounds: { x: 0, y: 0, width: 0, height: 0 }, x: 0, y: 0, visible: true, shown: true });
    }
    get children() { return layouts.get(this).children.slice(); }
    addChildView(view, index) {
      const state = layouts.get(this), child = layouts.get(view);
      if (!child || child.root) throw new TypeError('Expected a child View');
      if (state.window?.isDestroyed()) throw Error('Window is closed');
      for (let parent = this; parent; parent = layouts.get(parent).parent) if (parent === view) throw Error('Cannot create a view cycle');
      if (index !== undefined && (!Number.isInteger(index) || index < 0)) throw new TypeError('Invalid child index');
      if (child.parent) {
        const siblings = layouts.get(child.parent).children;
        siblings.splice(siblings.indexOf(view), 1);
      }
      child.parent = this;
      state.children.splice(Math.min(index ?? state.children.length, state.children.length), 0, view);
      sync(this, true);
    }
    removeChildView(view) {
      const state = layouts.get(this), index = state.children.indexOf(view);
      if (index < 0) return;
      state.children.splice(index, 1);
      layouts.get(view).parent = null;
      sync(view);
    }
    setBounds(bounds) {
      const values = ['x', 'y', 'width', 'height'].map(key => bounds[key]);
      if (!values.every(Number.isInteger) || values[2] < 0 || values[3] < 0) throw new TypeError('Invalid view bounds');
      const state = layouts.get(this);
      if (values[0] === state.bounds.x && values[1] === state.bounds.y && values[2] === state.bounds.width && values[3] === state.bounds.height) return;
      state.bounds = { x: values[0], y: values[1], width: values[2], height: values[3] };
      sync(this);
      this.emit('bounds-changed');
    }
    getBounds() { return { ...layouts.get(this).bounds }; }
    setVisible(visible) {
      if (typeof visible !== 'boolean') throw new TypeError('Expected visibility boolean');
      const state = layouts.get(this);
      if (state.visible === visible) return;
      state.visible = visible;
      sync(this);
    }
    getVisible() { return layouts.get(this).visible; }
  }
  class WebContentsView extends View {
    constructor(options = {}) {
      super();
      const webContents = options.webContents ?? new WebContents(options.webPreferences);
      if (!(webContents instanceof WebContents) || webContents.isDestroyed()) throw new TypeError('Expected a live WebContents');
      if (owners.has(webContents)) {
        call('browser.detach', webContents.id);
        call('browser.command', webContents.id, 'visible', true);
      }
      Object.defineProperty(this, 'webContents', { value: webContents, enumerable: true });
      owners.set(webContents, this);
      sync(this);
    }
  }
  const poll = () => {
    if (!contents.size) return;
    for (const { view, event } of call('browser.poll')) {
      if (event.type === 'evaluation') {
        const request = pending.get(event.id);
        if (!request || request.view !== view) continue;
        pending.delete(event.id);
        if (event.error) request.reject(new Error(event.error)); else request.resolve(event.value);
      } else contents.get(view)?._event(event);
    }
  };
  const session = Object.freeze({
    fromPartition(partition) {
      if (typeof partition !== 'string') throw new TypeError('Expected partition name');
      if (!partitions.has(partition)) partitions.set(partition, new WebContents({ partition }));
      return partitions.get(partition).session;
    },
    get defaultSession() { return this.fromPartition('persist:default'); },
  });
  return { View, WebContentsView, session, poll, pending: () => contents.size > 0,
    contentView(window, options) {
      const view = new View(), state = layouts.get(view);
      state.root = state.window = window;
      state.bounds.width = options.width ?? 800;
      state.bounds.height = options.height ?? 600;
      return view;
    },
  };
};

globalThis.__appsNativeShell = (call, EventEmitter) => {
  const native = (op, args = null) => call('native.' + op, args);
  const shortcuts = new Map(), menuCallbacks = new Map(), notifications = new Map();
  let nextNativeID = 0, menuActive = false, menuReady = false, applicationMenu, observeTheme = false, observePower = false, observeApp = false, nativeApp;
  const bindApp = (app, identifier) => {
    nativeApp = app;
    let ownsInstance = false;
    const enable = () => { native('app.events'); observeApp = true; };
    app.on = function (event, listener) {
      if (this.isReady() && (event === 'open-url' || event === 'second-instance')) enable();
      return EventEmitter.prototype.on.call(this, event, listener);
    };
    app.once('ready', () => { if (app.listenerCount('open-url') || app.listenerCount('second-instance')) enable(); });
    app.requestSingleInstanceLock = (additionalData = null) => {
      ownsInstance = native('app.singleInstance', { id: String(identifier), argv: process.argv, cwd: process.cwd(), data: additionalData });
      observeApp = true; return ownsInstance;
    };
    app.hasSingleInstanceLock = () => ownsInstance;
    app.releaseSingleInstanceLock = () => { native('app.releaseInstance'); ownsInstance = false; };
  };
  const clipboard = Object.freeze({
    readText: () => native('clipboard.readText'), writeText: text => native('clipboard.writeText', String(text)),
    readHTML: () => native('clipboard.readHTML'), writeHTML: text => native('clipboard.writeHTML', String(text)),
    clear: () => native('clipboard.clear'),
  });
  const shell = Object.freeze({
    async openExternal(url) { if (!native('shell.openExternal', String(url))) throw new Error('Could not open URL'); },
    async openPath(path) { return native('shell.openPath', String(path)) ? '' : 'Could not open path'; },
    showItemInFolder: path => native('shell.showItemInFolder', String(path)),
  });
  const options = args => args.length > 1 ? args[1] : args[0] ?? {};
  const dialog = Object.freeze({
    showOpenDialog: async (...args) => native('dialog.open', options(args)),
    showOpenDialogSync: (...args) => { const result = native('dialog.open', options(args)); return result.canceled ? undefined : result.filePaths; },
    showSaveDialog: async (...args) => native('dialog.save', options(args)),
    showSaveDialogSync: (...args) => { const result = native('dialog.save', options(args)); return result.canceled ? undefined : result.filePath; },
    showMessageBox: async (...args) => native('dialog.message', options(args)),
    showMessageBoxSync: (...args) => native('dialog.message', options(args)).response,
    showErrorBox: (title, content) => native('dialog.message', { message: String(title), detail: String(content) }),
  });
  const nativeTheme = new EventEmitter();
  nativeTheme.on = function (name, listener) { native('theme.get'); observeTheme = true; return EventEmitter.prototype.on.call(this, name, listener); };
  for (const name of ['shouldUseDarkColors', 'shouldUseHighContrastColors', 'shouldUseDarkColorsForSystemIntegratedUI', 'shouldUseInvertedColorScheme', 'inForcedColorsMode', 'prefersReducedTransparency', 'shouldDifferentiateWithoutColor']) {
    Object.defineProperty(nativeTheme, name, { enumerable: true, get: () => native('theme.get')[name] });
  }
  Object.defineProperty(nativeTheme, 'themeSource', {
    enumerable: true, get: () => native('theme.get').themeSource, set: value => {
      if (!['system', 'light', 'dark'].includes(value)) throw new TypeError('Invalid theme source');
      native('theme.set', value);
    },
  });
  const safeStorage = Object.freeze({
    isEncryptionAvailable: () => native('safeStorage.available'),
    encryptString: value => Uint8Array.from(atob(native('safeStorage.encrypt', String(value))), char => char.charCodeAt(0)),
    decryptString: bytes => {
      if (!ArrayBuffer.isView(bytes)) throw new TypeError('Expected encrypted bytes');
      let value = ''; for (const byte of new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength)) value += String.fromCharCode(byte);
      return native('safeStorage.decrypt', btoa(value));
    },
  });
  class Notification extends EventEmitter {
    #id;
    constructor(options = {}) { super(); this.options = { ...options }; }
    show() { this.#id = native('notification.show', this.options); notifications.set(this.#id, this); }
    close() { if (this.#id) { native('notification.close', this.#id); notifications.delete(this.#id); this.emit('close'); } }
  }
  class Menu {
    constructor() { this.items = []; }
    static buildFromTemplate(template) {
      if (!Array.isArray(template)) throw new TypeError('Expected menu template');
      const menu = new Menu();
      menu.items = template.map(item => {
        const value = { ...item };
        if (value.submenu) value.submenu = Menu.buildFromTemplate(value.submenu).items;
        if (typeof value.click === 'function') { const id = ++nextNativeID; menuCallbacks.set(id, { callback: value.click, item }); value.click = id; }
        return value;
      });
      return menu;
    }
    static setApplicationMenu(menu) {
      if (menuReady || nativeApp?.isReady()) { native('menu.set', menu?.items ?? null); menuReady = true; }
      applicationMenu = menu ?? null;
      menuActive = menuReady && !!menu && menuCallbacks.size > 0;
    }
    static getApplicationMenu() { return applicationMenu ?? null; }
    popup(options = {}) { menuActive = true; native('menu.popup', { template: this.items, ...(options.x === undefined ? {} : { x: options.x }), ...(options.y === undefined ? {} : { y: options.y }) }); options.callback?.(); }
  }
  globalThis.__appsInitMenu = () => {
    if (menuReady) return;
    if (applicationMenu === undefined) {
      const name = nativeApp.getName();
      applicationMenu = Menu.buildFromTemplate([
        { label: name, submenu: [
          { label: 'About ' + name, role: 'about' },
          { type: 'separator' },
          { label: 'Services', role: 'services' },
          { type: 'separator' },
          { label: 'Hide ' + name, role: 'hide', accelerator: 'Command+H' },
          { label: 'Hide Others', role: 'hideOthers', accelerator: 'Command+Alt+H' },
          { label: 'Show All', role: 'unhide' },
          { type: 'separator' },
          { label: 'Quit ' + name, role: 'quit', accelerator: 'Command+Q' },
        ] },
        { label: 'File', submenu: [{ label: 'Close Window', role: 'close', accelerator: 'Command+W' }] },
        { label: 'Edit', submenu: [
          { label: 'Undo', role: 'undo', accelerator: 'Command+Z' },
          { label: 'Redo', role: 'redo', accelerator: 'Command+Shift+Z' },
          { type: 'separator' },
          { label: 'Cut', role: 'cut', accelerator: 'Command+X' },
          { label: 'Copy', role: 'copy', accelerator: 'Command+C' },
          { label: 'Paste', role: 'paste', accelerator: 'Command+V' },
          { label: 'Paste and Match Style', role: 'pasteAndMatchStyle', accelerator: 'Command+Alt+Shift+V' },
          { label: 'Delete', role: 'delete' },
          { label: 'Select All', role: 'selectAll', accelerator: 'Command+A' },
          { type: 'separator' },
          { label: 'Speech', submenu: [{ label: 'Start Speaking', role: 'startSpeaking' }, { label: 'Stop Speaking', role: 'stopSpeaking' }] },
        ] },
        { label: 'View', submenu: [
          ...(!nativeApp.isPackaged ? [{ label: 'Toggle Developer Tools', role: 'toggleDevTools', accelerator: 'Command+Alt+I' }, { type: 'separator' }] : []),
          { label: 'Toggle Full Screen', role: 'togglefullscreen', accelerator: 'Control+Command+F' },
        ] },
        { label: 'Window', role: 'windowMenu', submenu: [
          { label: 'Minimize', role: 'minimize', accelerator: 'Command+M' },
          { label: 'Zoom', role: 'zoom' },
          { type: 'separator' },
          { label: 'Bring All to Front', role: 'front' },
        ] },
        { label: 'Help', role: 'help', submenu: [{ label: 'About ' + name, role: 'about' }] },
      ]);
    }
    menuReady = true;
    Menu.setApplicationMenu(applicationMenu);
  };
  const globalShortcut = Object.freeze({
    register(accelerator, callback) {
      if (typeof accelerator !== 'string' || typeof callback !== 'function') throw new TypeError('Expected accelerator and callback');
      if (shortcuts.has(accelerator)) return false;
      const id = ++nextNativeID;
      if (!native('shortcut.register', { id, accelerator })) return false;
      shortcuts.set(accelerator, { id, callback }); return true;
    },
    unregister(accelerator) { const item = shortcuts.get(accelerator); if (item) { native('shortcut.unregister', item.id); shortcuts.delete(accelerator); } },
    unregisterAll() { for (const accelerator of [...shortcuts.keys()]) this.unregister(accelerator); },
    isRegistered: accelerator => shortcuts.has(accelerator),
  });
  const powerMonitor = new EventEmitter();
  Object.assign(powerMonitor, {
    isOnBatteryPower: () => native('power.battery'),
    getSystemIdleTime: () => Math.floor(native('power.idle')),
    getSystemIdleState: threshold => {
      if (!Number.isInteger(threshold) || threshold <= 0) throw new TypeError('Expected positive idle threshold');
      return native('power.idle') >= threshold ? 'idle' : 'active';
    },
    getCurrentThermalState: () => native('power.thermal'),
    on(name, listener) { native('power.init'); observePower = true; return EventEmitter.prototype.on.call(this, name, listener); },
  });
  const poll = () => {
    if (!menuActive && !shortcuts.size && !notifications.size && !observeTheme && !observePower && !observeApp) return;
    for (const event of native('poll')) {
      if (event.type === 'shortcut') { for (const item of shortcuts.values()) if (item.id === event.id) item.callback(); }
      else if (event.type === 'menu') { const item = menuCallbacks.get(event.id); if (item) { if (item.item.type === 'checkbox') item.item.checked = event.checked; item.callback(item.item); } }
      else if (event.type === 'theme') nativeTheme.emit('updated');
      else if (event.type === 'power') powerMonitor.emit(event.event, event.event === 'thermal-state-change' ? { state: native('power.thermal') } : undefined);
      else if (event.type === 'app' && nativeApp) {
        const message = { defaultPrevented: false, preventDefault() { this.defaultPrevented = true; } };
        if (event.event === 'open-url') nativeApp.emit('open-url', message, event.url);
        else if (event.event === 'second-instance') nativeApp.emit('second-instance', message, event.args, event.cwd, event.data);
      }
      else if (event.type === 'notification') {
        const notification = notifications.get(event.id);
        if (notification) notification.emit(event.event, event.error ? new Error(event.error) : undefined);
        if (event.event === 'error' || event.event === 'close') notifications.delete(event.id);
      }
    }
  };
  return { clipboard, shell, dialog, nativeTheme, safeStorage, Notification, Menu, globalShortcut, powerMonitor, bindApp, poll, pending: () => menuActive || shortcuts.size > 0 || notifications.size > 0 || observePower || observeApp };


};
