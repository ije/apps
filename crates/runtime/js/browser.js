globalThis.__appsBrowser = (call, EventEmitter, isReady = () => true, created = () => {}) => {
  const contents = new Map(), pending = new Map(), partitions = new Map(), layouts = new WeakMap(), owners = new WeakMap();
  const safe = { nodeIntegration: false, nodeIntegrationInWorker: false, nodeIntegrationInSubFrames: false, contextIsolation: true, sandbox: true, webSecurity: true, webviewTag: false, allowRunningInsecureContent: false, experimentalFeatures: false };
  const eventOf = (fields = {}) => ({ ...fields, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; } });
  const failure = text => { const match = /^(\w*Error): ([\s\S]*)$/.exec(text ?? ''); return match ? Object.assign(new Error(match[2]), { name: match[1] }) : new Error(text); };
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
    #open = null;
    constructor(options = {}) {
      super();
      Object.defineProperty(this, 'id', { value: call('browser.create', { partition: options.partition || 'persist:default', javascript: options.javascript ?? true, inspectable: options.devTools ?? true, navigation: true, ...(options.show ? { show: true } : {}) }) });
      this.#devTools = options.devTools !== false;
      this.#partition = options.partition;
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
      if (/^apps:/.test(file)) return Promise.reject(new Error('Bundled pages load in BrowserWindow. Use loadURL() with a file URL in WebContentsView'));
      const url = __apps.url.pathToFileURL(__apps.path.resolve(__apps.app.getAppPath(), file));
      if (options.query) url.search = new URLSearchParams(options.query).toString();
      if (options.search !== undefined) url.search = options.search;
      if (options.hash !== undefined) url.hash = options.hash;
      return this.loadURL(url.href);
    }
    #partition;
    setWindowOpenHandler(handler) { if (handler !== null && typeof handler !== 'function') throw new TypeError('Expected a window open handler'); this.#open = handler; }
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
      if (event.url !== undefined && /^did-/.test(event.type)) this.#url = event.url;
      if (event.title !== undefined) this.#title = event.title;
      if (event.canGoBack !== undefined) this.#back = event.canGoBack;
      if (event.canGoForward !== undefined) this.#forward = event.canGoForward;
      if (event.type === 'did-start-loading') this.#loading = true;
      if (event.type === 'did-finish-load' || event.type === 'did-fail-load') this.#loading = false;
      if (event.type === 'closed') { this.close(); return; }
      if (event.type === 'will-navigate') {
        const navigation = eventOf({ url: event.url, isSameDocument: false, isMainFrame: true });
        try { this.emit('will-navigate', navigation, event.url, false, true, -1, -1); }
        finally { call('browser.command', this.id, 'decide', { id: event.id, allow: !navigation.defaultPrevented && !this.#destroyed }); }
      }
      else if (event.type === 'window-open') {
        const details = { url: event.url ?? '', frameName: event.frameName ?? '', features: event.features ?? '', disposition: 'new-window', referrer: { url: this.#url, policy: 'strict-origin-when-cross-origin' } };
        const result = this.#open ? this.#open(details) : { action: 'allow' };
        if (result?.action === 'allow' && details.url) {
          const child = new WebContents({ partition: this.#partition, show: true });
          created(child);
          child.loadURL(details.url).catch(() => {});
          this.emit('did-create-window', child, details);
        }
      }
      else if (event.type === 'did-fail-load') this.emit(event.type, eventOf(), event.code ?? -2, event.error ?? 'Navigation failed', event.url ?? this.#url, true, -1, -1);
      else if (event.type === 'page-title-updated') this.emit(event.type, eventOf(), this.#title, true);
      else if (event.type === 'did-navigate') this.emit(event.type, eventOf(), this.#url, -1, '');
      else if (event.type === 'did-start-loading' || event.type === 'did-finish-load' || event.type === 'dom-ready') this.emit(event.type, eventOf());
      else if (event.type === 'render-process-gone') this.emit(event.type, eventOf(), { reason: 'crashed', exitCode: 0 });
      else if (event.type === 'message' && (event.data?.kind === 'console' || event.data?.kind === 'error')) {
        const level = event.data.kind === 'error' ? 'error' : { log: 'info', info: 'info', warn: 'warning', error: 'error', debug: 'debug' }[event.data.level] ?? 'info';
        const message = event.data.kind === 'error' ? String(event.data.message ?? '') : (event.data.args ?? []).join(' '), line = event.data.line ?? 0, source = event.data.source ?? event.url ?? '';
        this.emit('console-message', eventOf({ level, message, lineNumber: line, sourceId: source }), ['debug', 'info', 'warning', 'error'].indexOf(level), message, line, source);
      }
      else if (event.type === 'message') this.emit('ipc-message', eventOf(), 'message', event.data?.data);
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
      const preferences = options.webPreferences ?? {};
      for (const [key, value] of Object.entries(preferences))
        if (value !== undefined && (key === 'partition' ? typeof value !== 'string' : key === 'javascript' || key === 'devTools' ? typeof value !== 'boolean' : !Object.hasOwn(safe, key) || safe[key] !== value)) throw new Error(`Unsupported webPreferences option: ${key}`);
      const webContents = options.webContents ?? new WebContents(preferences);
      if (!(webContents instanceof WebContents) || webContents.isDestroyed()) throw new TypeError('Expected a live WebContents');
      if (owners.has(webContents)) {
        call('browser.detach', webContents.id);
        call('browser.command', webContents.id, 'visible', true);
      }
      Object.defineProperty(this, 'webContents', { value: webContents, enumerable: true });
      owners.set(webContents, this);
      sync(this);
      if (!options.webContents) created(webContents);
    }
  }
  const poll = () => {
    if (!contents.size) return;
    for (const { view, event } of call('browser.poll')) {
      if (event.type === 'evaluation') {
        const request = pending.get(event.id);
        if (!request || request.view !== view) continue;
        pending.delete(event.id);
        if (event.error) request.reject(failure(event.error)); else request.resolve(event.value);
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
  // WARNING: Linux webviews need GTK pumping from poll; other platforms wake the loop on events.
  return { View, WebContentsView, session, poll, pending: () => contents.size > 0 && process.platform === 'linux', reserve: () => call('browser.reserve'), all: () => [...contents.values()].filter(item => ![...partitions.values()].includes(item)),
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
  const shortcuts = new Map(), notifications = new Map(), trays = new Map();
  const eventOf = () => ({ defaultPrevented: false, preventDefault() { this.defaultPrevented = true; } });
  let nextNativeID = 0, menuActive = false, menuReady = false, applicationMenu, observeTheme = false, observePower = false, observeApp = false, nativeApp, focused = () => null;
  const bindApp = (app, identifier, focusedWindow = focused) => {
    nativeApp = app;
    focused = focusedWindow;
    let ownsInstance = false, badge = '', badgeCount = 0;
    app.requestSingleInstanceLock = (additionalData = null) => {
      ownsInstance = native('app.singleInstance', { id: String(identifier), argv: process.argv, cwd: process.cwd(), data: additionalData });
      observeApp = true; return ownsInstance;
    };
    app.hasSingleInstanceLock = () => ownsInstance;
    app.releaseSingleInstanceLock = () => { native('app.releaseInstance'); ownsInstance = false; };
    app.showAboutPanel = () => native('app.about');
    app.setAboutPanelOptions = options => native('app.about', { ...options });
    app.getLoginItemSettings = () => native('app.loginItem');
    app.setLoginItemSettings = settings => { native('app.loginItem', !!settings?.openAtLogin); };
    Object.defineProperty(app, 'applicationMenu', { enumerable: true, get: () => Menu.getApplicationMenu(), set: menu => Menu.setApplicationMenu(menu) });
    if (process.platform !== 'darwin') return;
    const setBadge = text => { native('app.badge', badge = String(text)); };
    Object.assign(app, {
      hide: () => native('app.hidden', true),
      show: () => native('app.hidden', false),
      isHidden: () => native('app.state').hidden,
      isActive: () => native('app.state').active,
      setActivationPolicy: policy => native('app.activationPolicy', policy),
      setBadgeCount(count) {
        if (count !== undefined && (!Number.isInteger(count) || count < 0)) throw new TypeError('Expected a nonnegative badge count');
        setBadge(count === undefined ? '•' : count || '');
        badgeCount = count ?? 0;
        return true;
      },
      getBadgeCount: () => badgeCount,
      dock: Object.freeze({
        bounce: (type = 'informational') => native('app.attention', type),
        cancelBounce: id => native('app.cancelAttention', id),
        downloadFinished: path => native('app.downloadFinished', String(path)),
        setBadge, getBadge: () => badge,
        hide: () => { native('app.activationPolicy', 'accessory'); },
        show: async () => { native('app.activationPolicy', 'regular'); },
        isVisible: () => native('app.activationPolicy') === 'regular',
        setIcon: image => native('app.icon', image == null ? '' : String(image)),
      }),
    });
    Object.defineProperty(app, 'badgeCount', { enumerable: true, get: app.getBadgeCount, set: app.setBadgeCount });
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
  const replies = new Map();
  let nextReply = 0;
  // WARNING: native code sends the reply through the main queue, so it can't arrive before `replies` holds it.
  const later = (op, value) => new Promise(resolve => { const id = ++nextReply; native(op, { ...value, async: id }); replies.set(id, resolve); });
  const dialog = Object.freeze({
    showOpenDialog: async (...args) => later('dialog.open', options(args)),
    showOpenDialogSync: (...args) => { const result = native('dialog.open', options(args)); return result.canceled ? undefined : result.filePaths; },
    showSaveDialog: async (...args) => later('dialog.save', options(args)),
    showSaveDialogSync: (...args) => { const result = native('dialog.save', options(args)); return result.canceled ? undefined : result.filePath; },
    showMessageBox: async (...args) => later('dialog.message', options(args)),
    showMessageBoxSync: (...args) => native('dialog.message', options(args)).response,
    showErrorBox: (title, content) => native('dialog.message', { message: String(title), detail: String(content), type: 'error' }),
  });
  const nativeTheme = new EventEmitter();
  let themeSource = 'system';
  // WARNING: only macOS reports the system theme, so other platforms read light defaults instead of throwing.
  const theme = () => { try { return native('theme.get'); } catch { return { themeSource, shouldUseDarkColors: themeSource === 'dark', shouldUseHighContrastColors: false, shouldUseDarkColorsForSystemIntegratedUI: themeSource === 'dark', shouldUseInvertedColorScheme: false, inForcedColorsMode: false, prefersReducedTransparency: false, shouldDifferentiateWithoutColor: false }; } };
  for (const method of ['on', 'addListener', 'prependListener', 'prependOnceListener']) nativeTheme[method] = function (...args) { theme(); observeTheme = true; return EventEmitter.prototype[method].apply(this, args); };
  for (const name of ['shouldUseDarkColors', 'shouldUseHighContrastColors', 'shouldUseDarkColorsForSystemIntegratedUI', 'shouldUseInvertedColorScheme', 'inForcedColorsMode', 'prefersReducedTransparency', 'shouldDifferentiateWithoutColor']) {
    Object.defineProperty(nativeTheme, name, { enumerable: true, get: () => theme()[name] });
  }
  Object.defineProperty(nativeTheme, 'themeSource', {
    enumerable: true, get: () => theme().themeSource, set: value => {
      if (!['system', 'light', 'dark'].includes(value)) throw new TypeError('Invalid theme source');
      try { native('theme.set', value); } catch { themeSource = value; }
    },
  });
  const safeStorage = Object.freeze({
    isEncryptionAvailable: () => native('safeStorage.available'),
    encryptString: value => Buffer.from(native('safeStorage.encrypt', String(value)), 'base64'),
    decryptString: bytes => {
      if (!ArrayBuffer.isView(bytes)) throw new TypeError('Expected encrypted bytes');
      return native('safeStorage.decrypt', Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength).toString('base64'));
    },
  });
  class Notification extends EventEmitter {
    #id;
    static isSupported() { return process.platform === 'darwin'; }
    constructor(options = {}) { super(); this.options = { ...options }; }
    show() {
      try { this.#id = native('notification.show', this.options); notifications.set(this.#id, this); }
      catch (error) { setTimeout(() => this.emit('failed', eventOf(), error.message), 0); }
    }
    close() { if (this.#id) { native('notification.close', this.#id); notifications.delete(this.#id); this.emit('close', eventOf()); } }
  }
  const roles = { undo: ['Undo', 'CommandOrControl+Z'], redo: ['Redo', 'Shift+CommandOrControl+Z'], cut: ['Cut', 'CommandOrControl+X'], copy: ['Copy', 'CommandOrControl+C'], paste: ['Paste', 'CommandOrControl+V'], pasteandmatchstyle: ['Paste and Match Style', 'Shift+Option+CommandOrControl+V'], delete: ['Delete'], selectall: ['Select All', 'CommandOrControl+A'],
    reload: ['Reload', 'CommandOrControl+R'], forcereload: ['Force Reload', 'Shift+CommandOrControl+R'], toggledevtools: ['Toggle Developer Tools', 'Alt+CommandOrControl+I'], resetzoom: ['Actual Size', 'CommandOrControl+0'], zoomin: ['Zoom In', 'CommandOrControl+Plus'], zoomout: ['Zoom Out', 'CommandOrControl+-'], togglespellchecker: ['Check Spelling While Typing'], togglefullscreen: ['Toggle Full Screen', 'Control+Command+F'],
    window: ['Window'], minimize: ['Minimize', 'CommandOrControl+M'], close: ['Close Window', 'CommandOrControl+W'], help: ['Help'], about: [name => 'About ' + name], services: ['Services'], hide: [name => 'Hide ' + name, 'Command+H'], hideothers: ['Hide Others', 'Command+Alt+H'], unhide: ['Show All'], quit: [name => 'Quit ' + name, 'Command+Q'],
    startspeaking: ['Start Speaking'], stopspeaking: ['Stop Speaking'], zoom: ['Zoom'], front: ['Bring All to Front'], appmenu: [name => name], filemenu: ['File'], editmenu: ['Edit'], viewmenu: ['View'], windowmenu: ['Window'], sharemenu: ['Share'], recentdocuments: ['Open Recent'], clearrecentdocuments: ['Clear Menu'],
    toggletabbar: ['Show Tab Bar'], selectnexttab: ['Select Next Tab'], selectprevioustab: ['Select Previous Tab'], showalltabs: ['Show All Tabs'], mergeallwindows: ['Merge All Windows'], movetabtonewwindow: ['Move Tab to New Window'], showsubstitutions: ['Show Substitutions'], togglesmartquotes: ['Smart Quotes'], togglesmartdashes: ['Smart Dashes'], toggletextreplacement: ['Text Replacement'] };
  const separator = { type: 'separator' };
  const submenus = {
    appmenu: () => [{ role: 'about' }, separator, { role: 'services' }, separator, { role: 'hide' }, { role: 'hideOthers' }, { role: 'unhide' }, separator, { role: 'quit' }],
    filemenu: () => [{ role: 'close' }],
    editmenu: () => [{ role: 'undo' }, { role: 'redo' }, separator, { role: 'cut' }, { role: 'copy' }, { role: 'paste' }, { role: 'pasteAndMatchStyle' }, { role: 'delete' }, { role: 'selectAll' }, separator, { label: 'Speech', submenu: [{ role: 'startSpeaking' }, { role: 'stopSpeaking' }] }],
    viewmenu: () => [{ role: 'reload' }, { role: 'forceReload' }, ...(nativeApp?.isPackaged ? [] : [{ role: 'toggleDevTools' }]), separator, { role: 'resetZoom' }, { role: 'zoomIn' }, { role: 'zoomOut' }, separator, { role: 'togglefullscreen' }],
    windowmenu: () => [{ role: 'minimize' }, { role: 'zoom' }, separator, { role: 'front' }],
    recentdocuments: () => [{ role: 'clearRecentDocuments' }],
  };
  class MenuItem {
    constructor(options = {}) {
      const role = typeof options.role === 'string' ? options.role : undefined, key = role?.toLowerCase(), defaults = roles[key] ?? [], label = defaults[0];
      let submenu = options.submenu instanceof Menu ? options.submenu : Array.isArray(options.submenu) ? Menu.buildFromTemplate(options.submenu) : submenus[key] ? Menu.buildFromTemplate(submenus[key]()) : undefined;
      Object.assign(this, { id: options.id, label: options.label ?? (typeof label === 'function' ? label(nativeApp?.getName() ?? '') : label ?? ''), sublabel: options.sublabel ?? '', toolTip: options.toolTip ?? '', role, type: options.type ?? (submenu ? 'submenu' : 'normal'), accelerator: options.accelerator ?? defaults[1], icon: options.icon, enabled: options.enabled ?? true, visible: options.visible ?? true, checked: options.checked ?? false, submenu, registerAccelerator: options.registerAccelerator ?? true, menu: null });
      const action = options.click;
      this.click = (event = {}, window = focused(), contents = window?.webContents) => {
        if (this.type === 'checkbox') this.checked = !this.checked;
        if (this.type === 'radio') {
          const items = this.menu?.items ?? [this];
          let start = items.indexOf(this), end = start;
          while (start > 0 && items[start - 1].type === 'radio') start--;
          while (end + 1 < items.length && items[end + 1].type === 'radio') end++;
          for (let i = start; i <= end; i++) items[i].checked = items[i] === this;
        }
        if (key === 'reload' || key === 'forcereload') return contents?.reload();
        // Like Electron, the zoom roles step the zoom level by 0.5.
        if (key === 'resetzoom' || key === 'zoomin' || key === 'zoomout') { if (contents) contents.zoomLevel = key === 'resetzoom' ? 0 : contents.zoomLevel + (key === 'zoomin' ? 0.5 : -0.5); return; }
        if (!key && typeof action === 'function') action(this, window, event);
      };
    }
  }
  let interactive = false;
  const serialize = (menu, items) => menu.items.map(item => {
    item.menu = menu;
    items.set(++nextNativeID, item);
    if (!item.submenu && item.type !== 'separator' && (!item.role || /^((force)?reload|resetzoom|zoomin|zoomout)$/i.test(item.role))) interactive = true;
    return { label: String(item.label), type: String(item.type), ...(item.role ? { role: item.role.toLowerCase() } : {}), ...(item.accelerator && item.registerAccelerator !== false ? { accelerator: String(item.accelerator) } : {}), ...(item.toolTip ? { toolTip: String(item.toolTip) } : {}), ...(item.sublabel ? { sublabel: String(item.sublabel) } : {}), ...(typeof item.icon === 'string' ? { icon: item.icon } : {}), enabled: !!item.enabled, visible: !!item.visible, checked: !!item.checked, click: nextNativeID, ...(item.submenu ? { submenu: serialize(item.submenu, items) } : {}) };
  });
  class Menu extends EventEmitter {
    constructor() { super(); this.items = []; }
    static buildFromTemplate(template) {
      if (!Array.isArray(template)) throw new TypeError('Expected menu template');
      const menu = new Menu();
      for (const item of template) menu.append(item instanceof MenuItem ? item : new MenuItem(item));
      return menu;
    }
    static setApplicationMenu(menu) {
      if (menu != null && !(menu instanceof Menu)) throw new TypeError('Expected a Menu');
      interactive = false;
      if (menuReady || nativeApp?.isReady()) { applicationItems = new Map(); native('menu.set', menu ? serialize(menu, applicationItems) : null); menuReady = true; }
      applicationMenu = menu ?? null;
      menuActive = menuReady && interactive;
    }
    static getApplicationMenu() { return applicationMenu ?? null; }
    append(item) { this.insert(this.items.length, item); }
    insert(position, item) { if (!(item instanceof MenuItem)) throw new TypeError('Expected a MenuItem'); item.menu = this; this.items.splice(position, 0, item); }
    getMenuItemById(id) { for (const item of this.items) { const found = item.id === id ? item : item.submenu?.getMenuItemById(id); if (found) return found; } return null; }
    popup(options = {}) {
      popupItems = new Map();
      const template = serialize(this, popupItems);
      this.emit('menu-will-show', eventOf());
      native('menu.popup', { template, ...(options.x === undefined ? {} : { x: options.x }), ...(options.y === undefined ? {} : { y: options.y }) });
      this.emit('menu-will-close', eventOf());
      options.callback?.();
    }
    closePopup() {}
  }
  let applicationItems = new Map(), popupItems = new Map();
  class Tray extends EventEmitter {
    #id;
    #call(op, options = {}) {
      if (this.isDestroyed()) throw Error('Tray is destroyed');
      return native('tray.' + op, { id: this.#id, ...options });
    }
    constructor(image, guid) {
      super();
      if (!nativeApp?.isReady()) throw Error('Call app.whenReady() before creating a tray');
      this.#id = ++nextNativeID;
      native('tray.create', { id: this.#id, image, guid });
      trays.set(this.#id, { tray: this, menus: new Map() });
    }
    destroy() {
      if (this.isDestroyed()) return;
      this.#call('destroy');
      trays.delete(this.#id);
    }
    isDestroyed() { return !trays.has(this.#id); }
    setImage(image) { this.#call('image', { image }); }
    setPressedImage(image) { this.#call('pressedImage', { image }); }
    setToolTip(text) { this.#call('toolTip', { text }); }
    setTitle(text, options) { this.#call('title', { text, fontType: options?.fontType }); }
    getTitle() { return this.#call('getTitle'); }
    getGUID() { return this.#call('guid'); }
    setIgnoreDoubleClickEvents(ignore) { this.#call('ignoreDoubleClick', { ignore }); }
    getIgnoreDoubleClickEvents() { return this.#call('getIgnoreDoubleClick'); }
    getBounds() { return this.#call('bounds'); }
    #menu(op, menu, request) {
      if (menu != null && !(menu instanceof Menu)) throw new TypeError('Expected a Menu');
      const items = new Map();
      this.#call(op, { template: menu ? serialize(menu, items) : null, request });
      const menus = trays.get(this.#id).menus;
      if (menu) menus.set(request, items); else menus.delete(request);
    }
    setContextMenu(menu) { this.#menu('menu', menu, 0); }
    popUpContextMenu(menu) { this.#menu('popup', menu, ++nextNativeID); }
    closeContextMenu() { this.#call('close'); }
  }
  globalThis.__appsInitMenu = () => {
    if (menuReady) return;
    if (applicationMenu === undefined) {
      applicationMenu = Menu.buildFromTemplate([
        { role: 'appMenu' },
        { role: 'fileMenu' },
        { role: 'editMenu' },
        { label: 'View', submenu: [...(nativeApp.isPackaged ? [] : [{ role: 'toggleDevTools' }, separator]), { role: 'togglefullscreen' }] },
        { role: 'windowMenu' },
        { role: 'help', submenu: [{ role: 'about' }] },
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
    registerAll(accelerators, callback) { for (const accelerator of accelerators) this.register(accelerator, callback); },
    unregister(accelerator) { const item = shortcuts.get(accelerator); if (item) { native('shortcut.unregister', item.id); shortcuts.delete(accelerator); } },
    unregisterAll() { for (const accelerator of [...shortcuts.keys()]) this.unregister(accelerator); },
    isRegistered: accelerator => shortcuts.has(accelerator),
  });
  const blockers = new Map();
  let nextBlocker = 0;
  const powerSaveBlocker = Object.freeze({
    start(type) {
      if (type !== 'prevent-app-suspension' && type !== 'prevent-display-sleep') throw new TypeError(`Invalid power save blocker type: ${type}`);
      blockers.set(nextBlocker, native('power.block', type === 'prevent-display-sleep'));
      return nextBlocker++;
    },
    stop: id => { if (!blockers.has(id)) return false; native('power.unblock', blockers.get(id)); return blockers.delete(id); },
    isStarted: id => blockers.has(id),
  });
  const defaults = { string: value => typeof value === 'string', boolean: value => typeof value === 'boolean', integer: Number.isInteger, float: Number.isFinite, double: Number.isFinite, url: value => typeof value === 'string', array: Array.isArray, dictionary: value => value !== null && typeof value === 'object' && !Array.isArray(value) };
  const defaultType = type => { if (!Object.hasOwn(defaults, type)) throw new TypeError(`Invalid type: ${type}`); return type; };
  const camel = name => name.replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
  const color = (name, kind, selector) => { name = String(name); const value = /^[a-z]+(-[a-z]+)*$/.test(name) ? native('prefs.color', selector(camel(name))) : null; if (value === null) throw new Error(`Unknown ${kind}: ${name}`); return value; };
  // WARNING: Electron emits systemPreferences events on Windows and Linux only, so on macOS nothing emits them either.
  const systemPreferences = Object.assign(new EventEmitter(), {
    getAccentColor: () => native('prefs.color', 'controlAccentColor').slice(1),
    getColor: name => color(name, 'color', name => name + 'Color'),
    getSystemColor: name => color(name, 'system color', name => 'system' + name[0].toUpperCase() + name.slice(1) + 'Color'),
    getEffectiveAppearance: () => native('prefs.appearance'),
    getMediaAccessStatus: type => native('prefs.media', String(type)),
    askForMediaAccess: async type => later('prefs.askMedia', { type: String(type) }),
    isTrustedAccessibilityClient: prompt => native('prefs.trusted', !!prompt),
    getUserDefault: (key, type) => native('prefs.getDefault', { key: String(key), type: defaultType(type) }),
    setUserDefault(key, type, value) { if (!defaults[defaultType(type)](value)) throw new TypeError(`Unable to convert value to: ${type}`); native('prefs.setDefault', { key: String(key), type, value }); },
    removeUserDefault: key => native('prefs.removeDefault', String(key)),
    registerDefaults: values => native('prefs.registerDefaults', values),
    isSwipeTrackingFromScrollEventsEnabled: () => native('prefs.swipe'),
    getAnimationSettings: () => native('prefs.animation'),
    canPromptTouchID: () => native('prefs.touchID'),
    promptTouchID: async reason => { const value = await later('prefs.promptTouchID', { reason: String(reason) }); if (value !== true) throw new Error(value); },
  });
  Object.defineProperty(systemPreferences, 'effectiveAppearance', { enumerable: true, get: () => native('prefs.appearance') });
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
  let displays = null;
  const screen = new EventEmitter();
  const distance = (display, x, y) => { const { x: left, y: top, width, height } = display.bounds; return Math.hypot(Math.max(left - x, 0, x - left - width), Math.max(top - y, 0, y - top - height)); };
  const nearest = (x, y) => native('screen.displays').reduce((best, display) => distance(display, x, y) < distance(best, x, y) ? display : best);
  Object.assign(screen, {
    getAllDisplays: () => native('screen.displays'),
    getPrimaryDisplay: () => native('screen.displays')[0],
    getCursorScreenPoint: () => native('screen.cursor'),
    getDisplayNearestPoint: point => nearest(point.x, point.y),
    getDisplayMatching(rect) {
      const area = display => { const b = display.bounds; return Math.max(0, Math.min(b.x + b.width, rect.x + rect.width) - Math.max(b.x, rect.x)) * Math.max(0, Math.min(b.y + b.height, rect.y + rect.height) - Math.max(b.y, rect.y)); };
      const best = native('screen.displays').reduce((best, display) => area(display) > area(best) ? display : best);
      return area(best) > 0 ? best : nearest(rect.x + rect.width / 2, rect.y + rect.height / 2);
    },
    on(name, listener) { displays ??= native('screen.displays'); return EventEmitter.prototype.on.call(this, name, listener); },
  });
  const poll = () => {
    if (!menuActive && !popupItems.size && !trays.size && !shortcuts.size && !notifications.size && !replies.size && !observeTheme && !observePower && !observeApp && !displays) return;
    for (const event of native('poll')) {
      if (event.type === 'reply') { replies.get(event.id)?.(event.value); replies.delete(event.id); }
      else if (event.type === 'shortcut') { for (const item of shortcuts.values()) if (item.id === event.id) item.callback(); }
      else if (event.type === 'menu') {
        let item = applicationItems.get(event.id) ?? popupItems.get(event.id);
        if (!item) for (const { menus } of trays.values()) {
          for (const items of menus.values()) if ((item = items.get(event.id))) break;
          if (item) break;
        }
        item?.click({ shiftKey: !!event.shift, ctrlKey: !!event.control, altKey: !!event.alt, metaKey: !!event.meta, triggeredByAccelerator: !!event.accelerator });
      }
      else if (event.type === 'tray') {
        const state = trays.get(event.id);
        if (!state) continue;
        if (event.request) state.menus.delete(event.request);
        else {
          const input = { ...eventOf(), shiftKey: !!event.shift, ctrlKey: !!event.control, altKey: !!event.alt, metaKey: !!event.meta };
          state.tray.emit(event.event, input, ...(event.bounds ? [event.bounds] : []), ...(event.position ? [event.position] : []), ...(event.data === undefined ? [] : [event.data]));
        }
      }
      else if (event.type === 'theme') nativeTheme.emit('updated');
      else if (event.type === 'power') powerMonitor.emit(event.event, event.event === 'thermal-state-change' ? { state: native('power.thermal') } : undefined);
      else if (event.type === 'screen' && displays) {
        const next = native('screen.displays'), old = new Map(displays.map(display => [display.id, display]));
        displays = next;
        for (const display of next) {
          const before = old.get(display.id);
          old.delete(display.id);
          const changed = before ? ['bounds', 'workArea', 'scaleFactor', 'rotation'].filter(key => JSON.stringify(before[key]) !== JSON.stringify(display[key])) : [];
          if (!before) screen.emit('display-added', eventOf(), display);
          else if (changed.length) screen.emit('display-metrics-changed', eventOf(), display, changed);
        }
        for (const display of old.values()) screen.emit('display-removed', eventOf(), display);
      }
      else if (event.type === 'app' && nativeApp) {
        if (event.event === 'open-url') nativeApp.emit('open-url', eventOf(), event.url);
        else if (event.event === 'second-instance') nativeApp.emit('second-instance', eventOf(), event.args, event.cwd, event.data);
      }
      else if (event.type === 'notification') {
        const notification = notifications.get(event.id);
        if (event.event === 'failed' || event.event === 'close') notifications.delete(event.id);
        notification?.emit(event.event, eventOf(), ...(event.error ? [event.error] : []));
      }
    }
  };
  return { clipboard, shell, dialog, nativeTheme, safeStorage, Notification, Menu, MenuItem, Tray, globalShortcut, powerMonitor, screen, powerSaveBlocker, systemPreferences, bindApp, poll, pending: () => menuActive || shortcuts.size > 0 || notifications.size > 0 || observePower || observeApp };
};
