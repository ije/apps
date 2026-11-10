/// <reference types="node" />
/// <reference path="./client.d.ts" />

import { EventEmitter } from 'node:events'

export interface Point { x: number; y: number }
export interface Size { width: number; height: number }
export interface Rectangle extends Point, Size {}

export type Event<Params extends object = {}> = Params & {
  preventDefault(): void
  readonly defaultPrevented: boolean
}

// App

export type PathName = 'home' | 'appData' | 'assets' | 'userData' | 'sessionData' | 'temp' | 'exe' | 'module' | 'desktop' | 'documents' | 'downloads' | 'music' | 'pictures' | 'videos' | 'recent' | 'logs' | 'crashDumps'

export interface AppEvents {
  'will-finish-launching': []
  ready: [event: Event, launchInfo: Record<string, any>]
  'window-all-closed': []
  /** Fires after each window closes. */
  'window-close': []
  'before-quit': [event: Event]
  'will-quit': [event: Event]
  quit: [event: Event, exitCode: number]
  activate: [event: Event, hasVisibleWindows: boolean]
  'open-file': [event: Event, path: string]
  'open-url': [event: Event, url: string]
  'browser-window-created': [event: Event, window: BrowserWindow]
  'browser-window-focus': [event: Event, window: BrowserWindow]
  'browser-window-blur': [event: Event, window: BrowserWindow]
  'web-contents-created': [event: Event, webContents: WebContents]
  'second-instance': [event: Event, argv: string[], workingDirectory: string, additionalData: unknown]
}

export interface CommandLine {
  appendSwitch(name: string, value?: string): void
  appendArgument(value: string): void
  hasSwitch(name: string): boolean
  getSwitchValue(name: string): string
  removeSwitch(name: string): void
}

export interface Dock {
  bounce(type?: 'critical' | 'informational'): number
  cancelBounce(id: number): void
  downloadFinished(filePath: string): void
  setBadge(text: string): void
  getBadge(): string
  hide(): void
  show(): Promise<void>
  isVisible(): boolean
  /** Takes a file path. */
  setIcon(image: string | null): void
}

export interface AboutPanelOptions {
  applicationName?: string
  applicationVersion?: string
  copyright?: string
  version?: string
  credits?: string
  authors?: string[]
  website?: string
  iconPath?: string
}

export interface LoginItemSettings {
  openAtLogin: boolean
  status: 'not-registered' | 'enabled' | 'requires-approval' | 'not-found'
}

export interface App extends EventEmitter<AppEvents> {
  whenReady(): Promise<void>
  isReady(): boolean
  quit(): void
  exit(exitCode?: number): void
  /** Works in built apps. `apps run` ignores it. */
  relaunch(options?: { args?: string[]; execPath?: string }): void
  focus(options?: { steal: boolean }): void
  hide(): void
  show(): void
  isHidden(): boolean
  isActive(): boolean
  getAppPath(): string
  getPath(name: PathName): string
  setPath(name: PathName, path: string): void
  getVersion(): string
  getName(): string
  setName(name: string): void
  name: string
  getLocale(): string
  getSystemLocale(): string
  getPreferredSystemLanguages(): string[]
  getLocaleCountryCode(): string
  readonly commandLine: CommandLine
  disableHardwareAcceleration(): void
  isHardwareAccelerationEnabled(): boolean
  getFileIcon(path: string, options?: { size?: 'small' | 'normal' | 'large' }): Promise<NativeImage>
  setAppUserModelId(id: string): void
  setDesktopName(desktopName: string): void
  readonly runningUnderARM64Translation: boolean
  readonly runningUnderRosettaTranslation: boolean
  readonly isPackaged: boolean
  readonly platform: NodeJS.Platform
  readonly hasWindows: boolean
  requestSingleInstanceLock(additionalData?: Record<any, any>): boolean
  hasSingleInstanceLock(): boolean
  releaseSingleInstanceLock(): void
  setAsDefaultProtocolClient(protocol: string, path?: string, args?: string[]): boolean
  isDefaultProtocolClient(protocol: string, path?: string, args?: string[]): boolean
  removeAsDefaultProtocolClient(protocol: string, path?: string, args?: string[]): boolean
  userAgentFallback: string
  showAboutPanel(): void
  setAboutPanelOptions(options: AboutPanelOptions): void
  getLoginItemSettings(options?: { type?: string; serviceName?: string }): LoginItemSettings
  setLoginItemSettings(settings: { openAtLogin?: boolean; type?: string; serviceName?: string }): void
  applicationMenu: Menu | null
  setActivationPolicy(policy: 'regular' | 'accessory' | 'prohibited'): void
  setBadgeCount(count?: number): boolean
  getBadgeCount(): number
  badgeCount: number
  /** macOS only. */
  readonly dock?: Dock
}

export declare const app: App

// Main module

/**
 * The default export of the main module.
 *
 * ```ts
 * export default { ready(app) { new BrowserWindow().loadFile('index.html') } } satisfies AppModule
 * ```
 */
export interface AppModule<WebSocketData = undefined> extends Partial<ServeOptions<WebSocketData>> {
  /** Runs once after the main module and the app are ready. */
  ready?(app: App): unknown
  activate?(app: App): unknown
  /** Runs after each window closes. */
  windowClose?(app: App): unknown
}

// Server

export interface ServerWebSocket<Data = undefined> extends WebSocket {
  data: Data
}

export interface ServeOptions<WebSocketData = undefined> {
  fetch(request: Request, server: Server<WebSocketData>): Response | Promise<Response | undefined> | undefined
  /** Defaults to 3000. `0` picks a free port. */
  port?: number
  /** Defaults to `0.0.0.0`. */
  hostname?: string
  websocket?: {
    open?(socket: ServerWebSocket<WebSocketData>): void
    message?(socket: ServerWebSocket<WebSocketData>, message: string | ArrayBuffer): void
    close?(socket: ServerWebSocket<WebSocketData>, code: number, reason: string): void
    error?(socket: ServerWebSocket<WebSocketData>, event: globalThis.Event): void
  }
}

export interface Server<WebSocketData = undefined> {
  readonly port: number
  readonly hostname: string
  requestIP(request: Request): { address: string; family: string; port: number } | null
  upgrade(request: Request, options?: { protocol?: string; data?: WebSocketData }): ServerWebSocket<WebSocketData>
  /** Resolves when the server has closed. */
  stop(force?: boolean): Promise<void>
}

export declare function serve<WebSocketData = undefined>(options: ServeOptions<WebSocketData>): Server<WebSocketData>

// Windows

export type VibrancyType = 'appearance-based' | 'titlebar' | 'selection' | 'menu' | 'popover' | 'sidebar' | 'header' | 'sheet' | 'window' | 'hud' | 'fullscreen-ui' | 'tooltip' | 'content' | 'under-window' | 'under-page'

export interface BrowserWindowConstructorOptions {
  width?: number
  height?: number
  x?: number
  y?: number
  useContentSize?: boolean
  center?: boolean
  minWidth?: number
  minHeight?: number
  maxWidth?: number
  maxHeight?: number
  resizable?: boolean
  movable?: boolean
  minimizable?: boolean
  maximizable?: boolean
  closable?: boolean
  focusable?: boolean
  alwaysOnTop?: boolean
  fullscreen?: boolean
  skipTaskbar?: boolean
  show?: boolean
  parent?: BrowserWindow
  modal?: boolean
  title?: string
  /** A file path. */
  icon?: string
  backgroundColor?: string
  hasShadow?: boolean
  opacity?: number
  frame?: boolean
  transparent?: boolean
  titleBarStyle?: 'default' | 'hidden' | 'hiddenInset'
  trafficLightPosition?: Point
  vibrancy?: VibrancyType
  /** `active` keeps vibrancy and glass active while the window has no focus. */
  visualEffectState?: 'followWindow' | 'active' | 'inactive'
  // Ignored, as they only tune Chromium.
  webPreferences?: WebPreferences
  autoHideMenuBar?: boolean
  disableAutoHideCursor?: boolean
  titleBarOverlay?: boolean | object
  enableLargerThanScreen?: boolean
  tabbingIdentifier?: string
  acceptFirstMouse?: boolean
  thickFrame?: boolean
  paintWhenInitiallyHidden?: boolean
  roundedCorners?: boolean
}

export interface BrowserWindowEvents {
  close: [event: Event]
  closed: []
  show: []
  hide: []
  focus: []
  blur: []
  minimize: []
  restore: []
  maximize: []
  unmaximize: []
  'enter-full-screen': []
  'leave-full-screen': []
  'enter-html-full-screen': []
  'leave-html-full-screen': []
  resize: []
  resized: []
  move: []
  moved: []
  'page-title-updated': [event: Event, title: string, explicitSet: boolean]
  'ready-to-show': []
}

export interface LoadFileOptions {
  query?: Record<string, string>
  search?: string
  hash?: string
}

export interface LoadURLOptions {
  httpReferrer?: string
  userAgent?: string
  extraHeaders?: string
}

/** Getters return cached values. The real values arrive with the window events. */
export declare class BrowserWindow extends EventEmitter<BrowserWindowEvents> {
  constructor(options?: BrowserWindowConstructorOptions)
  static getAllWindows(): BrowserWindow[]
  static getFocusedWindow(): BrowserWindow | null
  static fromId(id: number): BrowserWindow | null
  static fromWebContents(webContents: WebContents): BrowserWindow | null
  readonly id: number
  readonly webContents: WebContents
  readonly contentView: View
  /** Loads an HTML string. It gets no preload. */
  load(html: string): Promise<void>
  /** A string literal `<name>.html` is bundled with its scripts, styles and preload. */
  loadFile(filePath: string, options?: LoadFileOptions): Promise<void>
  loadURL(url: string, options?: LoadURLOptions): Promise<void>
  reload(): void
  close(): void
  destroy(): void
  isDestroyed(): boolean
  getTitle(): string
  setTitle(title: string): void
  setBackgroundColor(color: string): void
  getBounds(): Rectangle
  setBounds(bounds: Partial<Rectangle>, animate?: boolean): void
  getContentBounds(): Rectangle
  setContentBounds(bounds: Rectangle, animate?: boolean): void
  getSize(): [number, number]
  setSize(width: number, height: number, animate?: boolean): void
  getContentSize(): [number, number]
  setContentSize(width: number, height: number, animate?: boolean): void
  getPosition(): [number, number]
  setPosition(x: number, y: number, animate?: boolean): void
  center(): void
  getMinimumSize(): [number, number]
  setMinimumSize(width: number, height: number): void
  getMaximumSize(): [number, number]
  setMaximumSize(width: number, height: number): void
  isResizable(): boolean
  setResizable(resizable: boolean): void
  isMovable(): boolean
  setMovable(movable: boolean): void
  isMinimizable(): boolean
  setMinimizable(minimizable: boolean): void
  isMaximizable(): boolean
  setMaximizable(maximizable: boolean): void
  isClosable(): boolean
  setClosable(closable: boolean): void
  isAlwaysOnTop(): boolean
  /** `level` and `relativeLevel` are ignored. */
  setAlwaysOnTop(flag: boolean, level?: string, relativeLevel?: number): void
  minimize(): void
  isMinimized(): boolean
  maximize(): void
  unmaximize(): void
  isMaximized(): boolean
  restore(): void
  isNormal(): boolean
  getNormalBounds(): Rectangle
  setFullScreen(flag: boolean): void
  isFullScreen(): boolean
  show(): void
  showInactive(): void
  hide(): void
  isVisible(): boolean
  focus(): void
  blur(): void
  isFocused(): boolean
  setOpacity(opacity: number): void
  getOpacity(): number
  setHasShadow(hasShadow: boolean): void
  hasShadow(): boolean
  invalidateShadow(): void
  setIgnoreMouseEvents(ignore: boolean, options?: { forward?: boolean }): void
  setVisibleOnAllWorkspaces(visible: boolean, options?: { visibleOnFullScreen?: boolean; skipTransformProcessType?: boolean }): void
  isVisibleOnAllWorkspaces(): boolean
  setContentProtection(enable: boolean): void
  isContentProtected(): boolean
  setFocusable(focusable: boolean): void
  isFocusable(): boolean
  setAspectRatio(aspectRatio: number, extraSize?: Size): void
  setProgressBar(progress: number, options?: { mode?: 'none' | 'normal' | 'indeterminate' | 'error' | 'paused' }): void
  flashFrame(flag: boolean): void
  setSkipTaskbar(skip: boolean): void
  setMenu(menu: Menu | null): void
  removeMenu(): void
  setAutoHideMenuBar(hide: boolean): void
  isMenuBarAutoHide(): boolean
  setMenuBarVisibility(visible: boolean): void
  isMenuBarVisible(): boolean
  /** `animationDuration` is ignored. */
  setVibrancy(type: VibrancyType | null, options?: { animationDuration?: number }): void
  setParentWindow(parent: BrowserWindow | null): void
  getParentWindow(): BrowserWindow | null
  getChildWindows(): BrowserWindow[]
  isModal(): boolean
  setWindowButtonVisibility(visible: boolean): void
  setWindowButtonPosition(position: Point | null): void
  getWindowButtonPosition(): Point | null
  setAutoHideCursor(autoHide: boolean): void
  setTitleBarOverlay(options: object): void
  setOverlayIcon(overlay: NativeImage | null, description: string): void
  setIcon(icon: string | NativeImage): void
  setDocumentEdited(edited: boolean): void
  isDocumentEdited(): boolean
  setRepresentedFilename(filename: string): void
  getRepresentedFilename(): string
  setSheetOffset(offsetY: number, offsetX?: number): void
  getNativeWindowHandle(): Buffer
  moveTop(): void
  moveAbove(mediaSourceId: string): void
}

export interface ViewEvents {
  'bounds-changed': []
}

export declare class View extends EventEmitter<ViewEvents> {
  constructor()
  readonly children: View[]
  addChildView(view: View, index?: number): void
  removeChildView(view: View): void
  setBounds(bounds: Rectangle): void
  getBounds(): Rectangle
  setVisible(visible: boolean): void
  getVisible(): boolean
}

export interface WebPreferences {
  partition?: string
  javascript?: boolean
  devTools?: boolean
  nodeIntegration?: false
  contextIsolation?: true
  sandbox?: true
  webSecurity?: true
}

/** Shows a page in the system web view (WKWebView, WebView2 or WebKitGTK) above the window. */
export declare class WebContentsView extends View {
  constructor(options?: { webPreferences?: WebPreferences; webContents?: WebContents })
  readonly webContents: WebContents
}

// Web contents

export interface Input {
  type: 'keyDown' | 'keyUp'
  key: string
  code: string
  isAutoRepeat: boolean
  isComposing: boolean
  shift: boolean
  control: boolean
  alt: boolean
  meta: boolean
  location: number
  modifiers: string[]
}

export interface ContextMenuParams {
  x: number
  y: number
  frame: WebFrameMain | null
  linkURL: string
  linkText: string
  pageURL: string
  frameURL: string
  srcURL: string
  mediaType: 'none' | 'image' | 'audio' | 'video' | 'canvas' | 'file' | 'plugin'
  hasImageContents: boolean
  isEditable: boolean
  selectionText: string
  titleText: string
  altText: string
  suggestedFilename: string
  misspelledWord: string
  dictionarySuggestions: string[]
  inputFieldType: string
  editFlags: {
    canUndo: boolean
    canRedo: boolean
    canCut: boolean
    canCopy: boolean
    canPaste: boolean
    canDelete: boolean
    canSelectAll: boolean
    canEditRichly: boolean
  }
  [key: string]: unknown
}

export interface HandlerDetails {
  url: string
  frameName: string
  features: string
  disposition: 'default' | 'foreground-tab' | 'background-tab' | 'new-window' | 'other'
  referrer: { url: string; policy: string }
  postBody?: unknown
}

export type WindowOpenHandlerResponse =
  | { action: 'deny' }
  | { action: 'allow'; overrideBrowserWindowOptions?: BrowserWindowConstructorOptions; outlivesOpener?: boolean }

export interface WebContentsEvents {
  'did-start-loading': []
  'did-stop-loading': []
  'did-start-navigation': [event: Event<{ url: string; isMainFrame: boolean; isSameDocument: boolean }>, url: string, isInPlace: boolean, isMainFrame: boolean, frameProcessId: number, frameRoutingId: number]
  'will-navigate': [event: Event<{ url: string; isMainFrame: boolean; isSameDocument: boolean }>, url: string, isInPlace: boolean, isMainFrame: boolean, frameProcessId: number, frameRoutingId: number]
  'did-navigate': [event: Event, url: string, httpResponseCode: number, httpStatusText: string]
  'did-navigate-in-page': [event: Event, url: string, isMainFrame: boolean, frameProcessId: number, frameRoutingId: number]
  'dom-ready': [event: Event]
  'did-finish-load': [event: Event]
  'did-fail-load': [event: Event, errorCode: number, errorDescription: string, validatedURL: string, isMainFrame: boolean, frameProcessId: number, frameRoutingId: number]
  'page-title-updated': [event: Event, title: string, explicitSet: boolean]
  'page-favicon-updated': [event: Event, favicons: string[]]
  'did-create-window': [window: WebContents, details: HandlerDetails]
  'console-message': [event: Event<{ level: 'info' | 'warning' | 'error' | 'debug'; message: string; lineNumber: number; sourceId: string }>, level: number, message: string, line: number, sourceId: string]
  'ipc-message': [event: Event, channel: string, ...args: any[]]
  'render-process-gone': [event: Event, details: { reason: string; exitCode: number }]
  'before-input-event': [event: Event, input: Input]
  'input-event': [event: Event, inputEvent: object]
  'context-menu': [event: Event, params: ContextMenuParams]
  'will-attach-webview': [event: Event, webPreferences: WebPreferences, params: Record<string, string>]
  'did-attach-webview': [event: Event, webContents: WebContents]
  destroyed: []
}

export interface WebFrameMain {
  readonly url: string
  readonly top: WebFrameMain | null
  readonly parent: WebFrameMain | null
  readonly frames: WebFrameMain[]
  readonly framesInSubtree: WebFrameMain[]
  readonly processId: number
  readonly routingId: number
  readonly frameTreeNodeId: number
  readonly visibilityState: string
  readonly detached: boolean
  executeJavaScript(code: string, userGesture?: boolean): Promise<unknown>
  send(channel: string, ...args: any[]): void
  isDestroyed(): boolean
}

export interface NavigationEntry { url: string; title: string }

export interface NavigationHistory {
  canGoBack(): boolean
  canGoForward(): boolean
  goBack(): void
  goForward(): void
  canGoToOffset(offset: number): boolean
  goToOffset(offset: number): void
  goToIndex(index: number): void
  clear(): void
  getActiveIndex(): number
  length(): number
  getEntryAtIndex(index: number): NavigationEntry
}

export interface Debugger extends EventEmitter<{ detach: [event: Event, reason: string]; message: [event: Event, method: string, params: any, sessionId: string] }> {
  attach(protocolVersion?: string): void
  detach(): void
  isAttached(): boolean
  /** Only for `WebContentsView`. Answers `Runtime.evaluate`, `Page.captureScreenshot`, `Page.reload`, `Target.getTargetInfo`, `Emulation.setEmulatedMedia` and the `enable` commands. */
  sendCommand(method: string, commandParams?: any, sessionId?: string): Promise<any>
}

export interface WebContents extends EventEmitter<WebContentsEvents> {
  readonly id: number
  readonly session: Session
  readonly ipc: IpcMain
  readonly mainFrame: WebFrameMain
  readonly focusedFrame: WebFrameMain | null
  readonly navigationHistory: NavigationHistory
  readonly hostWebContents: WebContents | null
  readonly devToolsWebContents: WebContents | null
  readonly debugger: Debugger
  zoomFactor: number
  zoomLevel: number
  loadFile(filePath: string, options?: LoadFileOptions): Promise<void>
  loadURL(url: string, options?: LoadURLOptions): Promise<void>
  getType(): 'backgroundPage' | 'window' | 'browserView' | 'remote' | 'webview' | 'offscreen'
  getURL(): string
  getTitle(): string
  isLoading(): boolean
  isLoadingMainFrame(): boolean
  isDestroyed(): boolean
  isFocused(): boolean
  isCrashed(): boolean
  focus(): void
  stop(): void
  reload(): void
  reloadIgnoringCache(): void
  canGoBack(): boolean
  canGoForward(): boolean
  goBack(): void
  goForward(): void
  /** Closes a `WebContentsView` page. */
  close(): void
  getUserAgent(): string
  setUserAgent(userAgent: string): void
  setBackgroundThrottling(allowed: boolean): void
  getBackgroundThrottling(): boolean
  setIgnoreMenuShortcuts(ignore: boolean): void
  setAudioMuted(muted: boolean): void
  isAudioMuted(): boolean
  isCurrentlyAudible(): boolean
  openDevTools(options?: { mode?: 'left' | 'right' | 'bottom' | 'undocked' | 'detach'; activate?: boolean }): void
  closeDevTools(): void
  toggleDevTools(): void
  isDevToolsOpened(): boolean
  send(channel: string, ...args: any[]): void
  /** Runs in the page, though page code can't run strings itself. */
  executeJavaScript(code: string, userGesture?: boolean): Promise<any>
  executeJavaScriptInIsolatedWorld(worldId: number, scripts: { code: string; url?: string }[], userGesture?: boolean): Promise<any>
  insertCSS(css: string, options?: { cssOrigin?: 'user' | 'author' }): Promise<string>
  removeInsertedCSS(key: string): Promise<void>
  /** Opens only bundled pages and local files. Open other URLs with `shell.openExternal()`. */
  setWindowOpenHandler(handler: ((details: HandlerDetails) => WindowOpenHandlerResponse) | null): void
  undo(): void
  redo(): void
  cut(): void
  copy(): void
  paste(): void
  pasteAndMatchStyle(): void
  delete(): void
  selectAll(): void
  unselect(): void
  replace(text: string): void
  replaceMisspelling(text: string): void
  insertText(text: string): Promise<void>
  /** Rejects frames with Liquid Glass. */
  capturePage(rect?: Rectangle): Promise<NativeImage>
  copyImageAt(x: number, y: number): void
  startDrag(item: { file: string; files?: string[]; icon: string | NativeImage }): void
  sendInputEvent(inputEvent: object): void
  setZoomFactor(factor: number): void
  getZoomFactor(): number
  setZoomLevel(level: number): void
  getZoomLevel(): number
  setVisualZoomLevelLimits(minimumLevel: number, maximumLevel: number): Promise<void>
  getFrameRate(): number
  setFrameRate(fps: number): void
  getProcessMemoryInfo(): Promise<{ residentSet: number; private: number; shared: number }>
  getOSProcessId(): number
  getProcessId(): number
}

export declare const webContents: {
  getAllWebContents(): WebContents[]
  getFocusedWebContents(): WebContents | null
  fromId(id: number): WebContents | undefined
}

// Sessions

export interface Cookie {
  name: string
  value: string
  domain?: string
  hostOnly?: boolean
  path?: string
  secure?: boolean
  httpOnly?: boolean
  session?: boolean
  expirationDate?: number
  sameSite?: 'unspecified' | 'no_restriction' | 'lax' | 'strict'
}

export interface Cookies extends EventEmitter {
  get(filter: { url?: string; name?: string; domain?: string; path?: string; secure?: boolean; session?: boolean; httpOnly?: boolean }): Promise<Cookie[]>
  set(details: Partial<Cookie> & { url: string }): Promise<void>
  remove(url: string, name: string): Promise<void>
  flushStore(): Promise<void>
}

export interface Session extends EventEmitter {
  readonly cookies: Cookies
  /** Listeners are accepted and never called. */
  readonly webRequest: Record<'onBeforeRequest' | 'onBeforeSendHeaders' | 'onSendHeaders' | 'onHeadersReceived' | 'onResponseStarted' | 'onBeforeRedirect' | 'onCompleted' | 'onErrorOccurred', (...args: any[]) => void>
  readonly serviceWorkers: EventEmitter
  readonly protocol: Protocol
  readonly storagePath: string | null
  readonly availableSpellCheckerLanguages: string[]
  fetch(input: string | Request, init?: RequestInit): Promise<Response>
  clearStorageData(options?: object): Promise<void>
  clearCache(): Promise<void>
  clearAuthCache(): Promise<void>
  clearHostResolverCache(): Promise<void>
  flushStorageData(): void
  setPermissionRequestHandler(handler: ((webContents: WebContents, permission: string, callback: (granted: boolean) => void, details: object) => void) | null): void
  setPermissionCheckHandler(handler: ((webContents: WebContents | null, permission: string, requestingOrigin: string, details: object) => boolean) | null): void
  setDisplayMediaRequestHandler(handler: ((...args: any[]) => void) | null): void
  setDevicePermissionHandler(handler: ((...args: any[]) => boolean) | null): void
  setCertificateVerifyProc(proc: ((...args: any[]) => void) | null): void
  setUserAgent(userAgent: string, acceptLanguages?: string): void
  getUserAgent(): string
  setDownloadPath(path: string): void
  setSpellCheckerEnabled(enable: boolean): void
  isSpellCheckerEnabled(): boolean
  setSpellCheckerLanguages(languages: string[]): void
  getSpellCheckerLanguages(): string[]
  setProxy(config: object): Promise<void>
  resolveProxy(url: string): Promise<string>
  closeAllConnections(): Promise<void>
  getStoragePath(): string | null
  isPersistent(): boolean
}

export declare const session: {
  readonly defaultSession: Session
  fromPartition(partition: string, options?: { cache?: boolean }): Session
}

export interface Protocol {
  registerSchemesAsPrivileged(customSchemes: { scheme: string; privileges?: Record<string, boolean> }[]): void
  /** Answers `loadURL()` and `net.fetch()` in main. */
  handle(scheme: string, handler: (request: Request) => Response | Promise<Response>): void
  unhandle(scheme: string): void
  isProtocolHandled(scheme: string): boolean
}

export declare const protocol: Protocol

export declare const net: {
  fetch(input: string | Request, init?: RequestInit): Promise<Response>
  request(options: string | ClientRequestConstructorOptions): import('node:http').ClientRequest
  isOnline(): boolean
  readonly online: boolean
}

export interface ClientRequestConstructorOptions {
  url?: string
  method?: string
  headers?: Record<string, string | string[]>
  protocol?: string
  host?: string
  hostname?: string
  port?: number
  path?: string
  redirect?: 'follow' | 'error'
}

// IPC

export interface IpcMainEvent extends Event {
  sender: WebContents
  senderFrame: { url: string; processId: number; routingId: number; send(channel: string, ...args: any[]): void }
  processId: number
  frameId: number
  ports: unknown[]
  returnValue: any
  reply(channel: string, ...args: any[]): void
}

export type IpcMainInvokeEvent = Omit<IpcMainEvent, 'returnValue' | 'reply'>

export interface IpcRendererEvent extends Event {
  sender: IpcRenderer
  senderId: number
  ports: unknown[]
}

export interface IpcMain extends EventEmitter<Record<string, [event: IpcMainEvent, ...args: any[]]>> {
  handle(channel: string, listener: (event: IpcMainInvokeEvent, ...args: any[]) => any): void
  handleOnce(channel: string, listener: (event: IpcMainInvokeEvent, ...args: any[]) => any): void
  removeHandler(channel: string): void
}

export interface IpcRenderer extends EventEmitter<Record<string, [event: IpcRendererEvent, ...args: any[]]>> {
  invoke(channel: string, ...args: any[]): Promise<any>
  send(channel: string, ...args: any[]): void
  sendSync(channel: string, ...args: any[]): any
}

/** Main only. */
export declare const ipcMain: IpcMain
/** Preload only. */
export declare const ipcRenderer: IpcRenderer

/** Preload only. */
export declare const contextBridge: {
  exposeInMainWorld(apiKey: string, api: any): void
}

export declare const webUtils: {
  /** Knows only files dropped on a page. */
  getPathForFile(file: File): string
}

/** Always reports a zoom of 1, and its setters do nothing. */
export declare const webFrame: {
  getZoomFactor(): number
  getZoomLevel(): number
  setZoomFactor(factor: number): void
  setZoomLevel(level: number): void
  setVisualZoomLevelLimits(minimumLevel: number, maximumLevel: number): void
  insertCSS(css: string): string
  removeInsertedCSS(key: string): void
}

// Images

export declare class NativeImage {
  private constructor()
  isEmpty(): boolean
  getSize(): Size
  getAspectRatio(): number
  getScaleFactors(): number[]
  toPNG(): Buffer
  toJPEG(quality: number): Buffer
  toBitmap(): Buffer
  getBitmap(): Buffer
  toDataURL(): string
  resize(options: { width?: number; height?: number; quality?: 'good' | 'better' | 'best' }): NativeImage
  crop(rect: Rectangle): NativeImage
  setTemplateImage(option: boolean): void
  isTemplateImage(): boolean
  addRepresentation(options: object): void
}

export declare const nativeImage: {
  createEmpty(): NativeImage
  createFromBuffer(buffer: Buffer | Uint8Array): NativeImage
  createFromPath(path: string): NativeImage
  createFromDataURL(dataURL: string): NativeImage
  createFromBitmap(buffer: Buffer | Uint8Array, options: { width: number; height: number }): NativeImage
  /** Always empty. */
  createFromNamedImage(imageName: string): NativeImage
}

// Menus

export type MenuItemRole =
  | 'undo' | 'redo' | 'cut' | 'copy' | 'paste' | 'pasteAndMatchStyle' | 'delete' | 'selectAll'
  | 'reload' | 'forceReload' | 'toggleDevTools' | 'resetZoom' | 'zoomIn' | 'zoomOut' | 'toggleSpellChecker' | 'togglefullscreen'
  | 'window' | 'minimize' | 'close' | 'help' | 'about' | 'services' | 'hide' | 'hideOthers' | 'unhide' | 'quit'
  | 'startSpeaking' | 'stopSpeaking' | 'zoom' | 'front' | 'appMenu' | 'fileMenu' | 'editMenu' | 'viewMenu' | 'windowMenu' | 'shareMenu'
  | 'recentDocuments' | 'clearRecentDocuments' | 'toggleTabBar' | 'selectNextTab' | 'selectPreviousTab' | 'showAllTabs' | 'mergeAllWindows'
  | 'moveTabToNewWindow' | 'showSubstitutions' | 'toggleSmartQuotes' | 'toggleSmartDashes' | 'toggleTextReplacement'

export interface MenuItemConstructorOptions {
  click?: (menuItem: MenuItem, window: BrowserWindow | undefined, event: KeyboardEvent) => void
  role?: MenuItemRole
  type?: 'normal' | 'separator' | 'submenu' | 'checkbox' | 'radio'
  label?: string
  /** macOS 14.4 or later. */
  sublabel?: string
  toolTip?: string
  accelerator?: string
  /** A file path. */
  icon?: string
  enabled?: boolean
  visible?: boolean
  checked?: boolean
  registerAccelerator?: boolean
  submenu?: (MenuItemConstructorOptions | MenuItem)[] | Menu
  id?: string
  /** Ignored. */
  before?: string[]
  /** Ignored. */
  after?: string[]
}

export interface KeyboardEvent {
  altKey?: boolean
  ctrlKey?: boolean
  metaKey?: boolean
  shiftKey?: boolean
  triggeredByAccelerator?: boolean
}

export declare class MenuItem {
  constructor(options: MenuItemConstructorOptions)
  id: string | undefined
  label: string
  sublabel: string
  toolTip: string
  role: MenuItemRole | undefined
  type: 'normal' | 'separator' | 'submenu' | 'checkbox' | 'radio'
  accelerator: string | undefined
  icon: string | undefined
  enabled: boolean
  visible: boolean
  checked: boolean
  registerAccelerator: boolean
  submenu: Menu | undefined
  menu: Menu | null
  click: (event?: KeyboardEvent, window?: BrowserWindow, webContents?: WebContents) => void
}

export interface PopupOptions {
  window?: BrowserWindow
  x?: number
  y?: number
  positioningItem?: number
  callback?: () => void
}

/** Changes to a `MenuItem` after `setApplicationMenu()` need another `setApplicationMenu()`. */
export declare class Menu extends EventEmitter<{ 'menu-will-show': [event: Event]; 'menu-will-close': [event: Event] }> {
  constructor()
  static buildFromTemplate(template: (MenuItemConstructorOptions | MenuItem)[]): Menu
  static setApplicationMenu(menu: Menu | null): void
  static getApplicationMenu(): Menu | null
  readonly items: MenuItem[]
  append(menuItem: MenuItem): void
  insert(pos: number, menuItem: MenuItem): void
  getMenuItemById(id: string): MenuItem | null
  /** Returns at once, as in Electron. */
  popup(options?: PopupOptions): void
  closePopup(window?: BrowserWindow): void
}

// Tray

export interface TrayClickEvent extends Event {
  shiftKey: boolean
  ctrlKey: boolean
  altKey: boolean
  metaKey: boolean
}

export interface TrayEvents {
  click: [event: TrayClickEvent, bounds: Rectangle, position: Point]
  'right-click': [event: TrayClickEvent, bounds: Rectangle]
  'double-click': [event: TrayClickEvent, bounds: Rectangle]
  'mouse-up': [event: TrayClickEvent, position: Point]
  'mouse-down': [event: TrayClickEvent, position: Point]
  'mouse-enter': [event: TrayClickEvent, position: Point]
  'mouse-leave': [event: TrayClickEvent, position: Point]
  'mouse-move': [event: TrayClickEvent, position: Point]
  'drag-enter': [event: TrayClickEvent]
  'drag-leave': [event: TrayClickEvent]
  'drag-end': [event: TrayClickEvent]
}

/** Images are file paths. Use an empty string for a title-only tray. */
export declare class Tray extends EventEmitter<TrayEvents> {
  constructor(image: string, guid?: string)
  destroy(): void
  isDestroyed(): boolean
  setImage(image: string): void
  setPressedImage(image: string): void
  setToolTip(toolTip: string): void
  setTitle(title: string, options?: { fontType?: 'monospaced' | 'monospacedDigit' }): void
  getTitle(): string
  getGUID(): string | null
  setIgnoreDoubleClickEvents(ignore: boolean): void
  getIgnoreDoubleClickEvents(): boolean
  getBounds(): Rectangle
  /** Call it again after editing menu items. */
  setContextMenu(menu: Menu | null): void
  popUpContextMenu(menu?: Menu, position?: Point): void
  closeContextMenu(): void
}

// Notifications

export interface NotificationConstructorOptions {
  title?: string
  subtitle?: string
  body?: string
  silent?: boolean
  icon?: string
  hasReply?: boolean
  replyPlaceholder?: string
  sound?: string
  urgency?: 'normal' | 'critical' | 'low'
  actions?: { type: 'button'; text?: string }[]
  closeButtonText?: string
}

/** Needs an app bundle identifier, so in `apps run` `show()` emits `failed`. */
export declare class Notification extends EventEmitter<{
  show: [event: Event]
  click: [event: Event]
  close: [event: Event]
  failed: [event: Event, error: string]
}> {
  constructor(options?: NotificationConstructorOptions)
  static isSupported(): boolean
  show(): void
  close(): void
}

// Dialogs

export interface FileFilter { name: string; extensions: string[] }

export interface OpenDialogOptions {
  title?: string
  defaultPath?: string
  buttonLabel?: string
  filters?: FileFilter[]
  properties?: ('openFile' | 'openDirectory' | 'multiSelections' | 'showHiddenFiles' | 'createDirectory' | 'promptToCreate' | 'noResolveAliases' | 'treatPackageAsDirectory' | 'dontAddToRecent')[]
  message?: string
  securityScopedBookmarks?: boolean
}

export interface SaveDialogOptions {
  title?: string
  defaultPath?: string
  buttonLabel?: string
  filters?: FileFilter[]
  message?: string
  nameFieldLabel?: string
  showsTagField?: boolean
  properties?: ('showHiddenFiles' | 'createDirectory' | 'treatPackageAsDirectory' | 'showOverwriteConfirmation' | 'dontAddToRecent')[]
  securityScopedBookmarks?: boolean
}

export interface MessageBoxOptions {
  message: string
  type?: 'none' | 'info' | 'error' | 'question' | 'warning'
  buttons?: string[]
  defaultId?: number
  signal?: AbortSignal
  title?: string
  detail?: string
  checkboxLabel?: string
  checkboxChecked?: boolean
  icon?: string | NativeImage
  textWidth?: number
  cancelId?: number
  noLink?: boolean
  normalizeAccessKeys?: boolean
}

export declare const dialog: {
  showOpenDialog(window: BrowserWindow, options: OpenDialogOptions): Promise<{ canceled: boolean; filePaths: string[] }>
  showOpenDialog(options: OpenDialogOptions): Promise<{ canceled: boolean; filePaths: string[] }>
  showOpenDialogSync(window: BrowserWindow, options: OpenDialogOptions): string[] | undefined
  showOpenDialogSync(options: OpenDialogOptions): string[] | undefined
  showSaveDialog(window: BrowserWindow, options: SaveDialogOptions): Promise<{ canceled: boolean; filePath: string }>
  showSaveDialog(options: SaveDialogOptions): Promise<{ canceled: boolean; filePath: string }>
  showSaveDialogSync(window: BrowserWindow, options: SaveDialogOptions): string
  showSaveDialogSync(options: SaveDialogOptions): string
  showMessageBox(window: BrowserWindow, options: MessageBoxOptions): Promise<{ response: number; checkboxChecked: boolean }>
  showMessageBox(options: MessageBoxOptions): Promise<{ response: number; checkboxChecked: boolean }>
  showMessageBoxSync(window: BrowserWindow, options: MessageBoxOptions): number
  showMessageBoxSync(options: MessageBoxOptions): number
  showErrorBox(title: string, content: string): void
}

// System

/** Reads and writes text and HTML only. */
export declare const clipboard: {
  readText(type?: 'selection' | 'clipboard'): string
  writeText(text: string, type?: 'selection' | 'clipboard'): void
  readHTML(type?: 'selection' | 'clipboard'): string
  writeHTML(markup: string, type?: 'selection' | 'clipboard'): void
  write(data: { text?: string; html?: string }, type?: 'selection' | 'clipboard'): void
  clear(type?: 'selection' | 'clipboard'): void
}

export declare const shell: {
  /** Opens only `http`, `https`, `mailto` and `tel` URLs. */
  openExternal(url: string, options?: { activate?: boolean }): Promise<void>
  openPath(path: string): Promise<string>
  showItemInFolder(fullPath: string): void
}

export declare const nativeTheme: EventEmitter<{ updated: [] }> & {
  readonly shouldUseDarkColors: boolean
  readonly shouldUseHighContrastColors: boolean
  readonly shouldUseDarkColorsForSystemIntegratedUI: boolean
  readonly shouldUseInvertedColorScheme: boolean
  readonly inForcedColorsMode: boolean
  readonly prefersReducedTransparency: boolean
  readonly shouldDifferentiateWithoutColor: boolean
  themeSource: 'system' | 'light' | 'dark'
}

export declare const safeStorage: {
  isEncryptionAvailable(): boolean
  getSelectedStorageBackend(): string
  encryptString(plainText: string): Buffer
  decryptString(encrypted: Buffer): string
}

export declare const globalShortcut: {
  /** Returns `false` for media keys. */
  register(accelerator: string, callback: () => void): boolean
  registerAll(accelerators: string[], callback: () => void): void
  unregister(accelerator: string): void
  unregisterAll(): void
  isRegistered(accelerator: string): boolean
}

export declare const powerMonitor: EventEmitter<{
  suspend: []
  resume: []
  shutdown: []
  'thermal-state-change': [details: { state: 'unknown' | 'nominal' | 'fair' | 'serious' | 'critical' }]
}> & {
  isOnBatteryPower(): boolean
  getSystemIdleTime(): number
  getSystemIdleState(idleThreshold: number): 'active' | 'idle' | 'locked' | 'unknown'
  getCurrentThermalState(): 'unknown' | 'nominal' | 'fair' | 'serious' | 'critical'
}

export declare const powerSaveBlocker: {
  start(type: 'prevent-app-suspension' | 'prevent-display-sleep'): number
  stop(id: number): boolean
  isStarted(id: number): boolean
}

export interface Display {
  id: number
  label: string
  bounds: Rectangle
  workArea: Rectangle
  size: Size
  workAreaSize: Size
  scaleFactor: number
  rotation: number
  internal: boolean
  monochrome: boolean
  colorDepth: number
  colorSpace: string
  depthPerComponent: number
  displayFrequency: number
  accelerometerSupport: 'available' | 'unavailable' | 'unknown'
  touchSupport: 'available' | 'unavailable' | 'unknown'
  detected: boolean
}

export declare const screen: EventEmitter<{
  'display-added': [event: Event, newDisplay: Display]
  'display-removed': [event: Event, oldDisplay: Display]
  'display-metrics-changed': [event: Event, display: Display, changedMetrics: string[]]
}> & {
  getAllDisplays(): Display[]
  getPrimaryDisplay(): Display
  getCursorScreenPoint(): Point
  getDisplayNearestPoint(point: Point): Display
  getDisplayMatching(rect: Rectangle): Display
  screenToDipPoint(point: Point): Point
  dipToScreenPoint(point: Point): Point
  screenToDipRect(window: BrowserWindow | null, rect: Rectangle): Rectangle
  dipToScreenRect(window: BrowserWindow | null, rect: Rectangle): Rectangle
}

export declare const systemPreferences: {
  readonly effectiveAppearance: 'dark' | 'light' | 'unknown'
  getAccentColor(): string
  getColor(color: string): string
  getSystemColor(color: 'blue' | 'brown' | 'gray' | 'green' | 'orange' | 'pink' | 'purple' | 'red' | 'yellow'): string
  getEffectiveAppearance(): 'dark' | 'light' | 'unknown'
  getMediaAccessStatus(mediaType: 'microphone' | 'camera' | 'screen'): 'not-determined' | 'granted' | 'denied' | 'restricted' | 'unknown'
  /** Needs `NSCameraUsageDescription` or `NSMicrophoneUsageDescription` in the app's Info.plist. */
  askForMediaAccess(mediaType: 'microphone' | 'camera'): Promise<boolean>
  isTrustedAccessibilityClient(prompt: boolean): boolean
  getUserDefault(key: string, type: 'string' | 'boolean' | 'integer' | 'float' | 'double' | 'url' | 'array' | 'dictionary'): any
  setUserDefault(key: string, type: 'string' | 'boolean' | 'integer' | 'float' | 'double' | 'url' | 'array' | 'dictionary', value: any): void
  removeUserDefault(key: string): void
  registerDefaults(defaults: Record<string, string | boolean | number>): void
  isSwipeTrackingFromScrollEventsEnabled(): boolean
  getAnimationSettings(): { shouldRenderRichAnimation: boolean; scrollAnimationsEnabledBySystem: boolean; prefersReducedMotion: boolean }
  canPromptTouchID(): boolean
  promptTouchID(reason: string): Promise<void>
}

/** `checkForUpdates()` emits `error`, as Electron does in an unsigned app. */
export declare const autoUpdater: EventEmitter<{
  error: [error: Error]
  'checking-for-update': []
  'update-available': []
  'update-not-available': []
  'update-downloaded': [event: Event, releaseNotes: string, releaseName: string, releaseDate: Date, updateURL: string]
}> & {
  getFeedURL(): string
  setFeedURL(options: { url: string; headers?: Record<string, string>; serverType?: 'json' | 'default' }): void
  checkForUpdates(): void
  quitAndInstall(): void
}

/** Returns no sources. */
export declare const desktopCapturer: {
  getSources(options: { types: ('screen' | 'window')[]; thumbnailSize?: Size; fetchWindowIcons?: boolean }): Promise<never[]>
}

export declare class ClipboardItem {
  constructor(items: Record<string, string | Blob | PromiseLike<string | Blob>>)
  readonly types: string[]
  getType(type: string): Promise<Blob>
}

declare const apps: {
  app: typeof app
  BrowserWindow: typeof BrowserWindow
  View: typeof View
  WebContentsView: typeof WebContentsView
  webContents: typeof webContents
  serve: typeof serve
  contextBridge: typeof contextBridge
  ipcMain: typeof ipcMain
  ipcRenderer: typeof ipcRenderer
  clipboard: typeof clipboard
  shell: typeof shell
  dialog: typeof dialog
  nativeTheme: typeof nativeTheme
  safeStorage: typeof safeStorage
  Notification: typeof Notification
  Menu: typeof Menu
  MenuItem: typeof MenuItem
  Tray: typeof Tray
  globalShortcut: typeof globalShortcut
  session: typeof session
  powerMonitor: typeof powerMonitor
  screen: typeof screen
  powerSaveBlocker: typeof powerSaveBlocker
  systemPreferences: typeof systemPreferences
  webUtils: typeof webUtils
  net: typeof net
  nativeImage: typeof nativeImage
  protocol: typeof protocol
  webFrame: typeof webFrame
  autoUpdater: typeof autoUpdater
  desktopCapturer: typeof desktopCapturer
  ClipboardItem: typeof ClipboardItem
}
export default apps
