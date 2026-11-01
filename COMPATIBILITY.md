# Compatibility

## DOM

Pages get the standard DOM API (nodes, elements, events, ranges, selection, observers, shadow DOM and custom elements). These parts differ from browsers:

| Feature | Support |
|---|---|
| `NodeList`, `HTMLCollection` | Static arrays with `item()`. They don't update when the tree changes |
| `DocumentType`, `CDATASection`, `ProcessingInstruction` | Not supported. `Attr` is not a `Node` |
| Named access | Elements are not globals by their `id`, so `window.foo` for `<div id="foo">` is `undefined`. Use `document.getElementById()` |
| `DOMParser`, XPath, XSLT | `DOMParser` parses `text/html` only. XPath and XSLT are not supported |
| `*AttributeNS()` | The namespace is ignored |
| `<template>` | Parsed children move into `content` when `content` is first read |
| `eval()`, `new Function()` | Throw an `EvalError` in pages and page workers, as do the constructors of async and generator functions. The `jsExecute` feature allows them |
| `import()` | A string literal imports a bundled module. Other specifiers stay in the code and load when they run: `data:` and `blob:` URLs with the `jsExecute` feature, `http` and `https` URLs with `httpImport`. Remote modules can import other modules only by URL |
| Remote scripts and stylesheets | `<script src>`, `<link rel="stylesheet">`, JavaScript imports and CSS `@import` with `http` or `https` URLs need the `httpImport` feature. `apps build` downloads them into the app |
| Classic scripts | `<script>` without `type="module"` runs as a classic script, with global `var` and `function`, sloppy mode and `this` as `window`. Scripts without `defer` or `async` run before module scripts. Classic scripts are bundled as written: no TypeScript or JSX, and their `import()` is not bundled. `nomodule` scripts don't run |
| Root-relative paths | In `src`, `href` and CSS `@import`, a path that starts with `/` is relative to the app folder and cannot leave it |
| Inline event attributes (`onclick="..."`) | Not run. `el.onclick = fn` and `addEventListener` work |
| HTML parsing | Follows the HTML5 tree builder, except misnested formatting tags (`<b><p></b>` keeps `p` in `b`), MathML and framesets |
| Navigation | Pages can go to bundled `apps:///` pages and local `file:///` files only. Other URLs throw. Use `WebContentsView` or `shell.openExternal()` for websites |
| `Worker` | Loads bundled, `data:`, `blob:` and local scripts. Remote scripts are rejected |
| Private networks | `fetch()` and `WebSocket` in pages can't reach loopback, LAN and other private addresses, except loopback ports that the app's own servers listen on |
| Long scripts | A page script that runs for more than 10 seconds without returning is stopped, so one page cannot freeze the app |
| Origin private file system | `navigator.storage.getDirectory()` keeps files in memory, so they are lost when the page reloads or the app quits |
| Forms | A submit that is not canceled sends nothing and does not navigate, so handle `submit` and call `preventDefault()`. `method="dialog"` closes its dialog |
| Date and time inputs | Show and edit the value as plain text, like `2026-10-31`, with no picker |
| `<input type=file>` | Doesn't open a file picker. Use `dialog.showOpenDialog()` in main, or drag and drop files onto the page |
| `<select>` | No popup list. A closed select shows its selected option, and arrow keys and typing change it |
| `<dialog>` | Modal dialogs stay on top with `z-index` instead of a top layer, and `::backdrop` is not drawn |
| `<video>`, `<audio>` | Need the `video` and `audio` features. They play MP4, MOV, WebM, Matroska, Ogg, MP3, WAV, FLAC and AAC files with H.264, HEVC, VP8, VP9, AAC, MP3, Opus, Vorbis, FLAC, ALAC and PCM. AV1 plays only with hardware decoding. `controls` draws no controls, rotation metadata is ignored and `playbackRate` changes the pitch. Remote and `blob:` sources download fully before they play. `srcObject`, Media Source Extensions, text tracks, `requestVideoFrameCallback()` and Web Audio are not supported. Frames are scaled to fit 8192 pixels per side after the pixel aspect ratio is applied, and late frames are skipped |
| Deep trees | Boxes nested past the native stack budget (thousands of levels in release builds) are not drawn |

## CSS

Pages use standard CSS, drawn by h2o on the GPU. Transforms, filters, blend modes, masks and clip paths paint through offscreen GPU layers.
These parts differ from browsers. The ones that don't work show an error in the Developer Tools console (`apps run -D`): properties and at-rules that h2o reads but does not apply, like `hyphens`, `tab-size` and `@counter-style`, and values it rejects. `@supports` and `CSS.supports()` return false for them.

| Feature | Support |
|---|---|
| `transform-style: preserve-3d` | 3D children are flattened into the parent's plane |
| `filter: url()` | Only `<filter>` elements in the page, with `feGaussianBlur`, `feColorMatrix`, `feOffset` and `feDropShadow` |
| `clip-path: shape()` | Not supported |
| `line-height`, `vertical-align` in `::first-letter` and `::first-line` | Ignored. Sizes, colors and font styles work |
| `float` | Text next to a float doesn't move under it when the float ends |
| Multi-column layout | Columns break between child blocks. One paragraph doesn't split across columns |
| `writing-mode: vertical-lr` | Lines are ordered right to left, like `vertical-rl` |
| Tailwind CSS v4 | `@import "tailwindcss"` compiles the classes in the page when they appear, with `@theme`, `@custom-variant`, `@utility` and `@apply`. `@plugin`, `@source`, `@config` and the `prefix()`, `source()` and `important` import options are not supported. An `@utility` that applies itself, directly or through other utilities, skips that `@apply`, where Tailwind reports an error |
| Scrollbars | Drawn as macOS overlay bars on all platforms. They show while the box scrolls or the pointer is on them, then fade out, and they take no space. The knob is light on a dark `color-scheme` or background. The macOS "Always" scroll bar setting is ignored. `scrollbar-width` and `scrollbar-color` work |
| `::-webkit-scrollbar` | Gives bars that always show and take space, as in Chromium. `-track`, `-thumb`, `-thumb:hover` and `-corner` use the background color, border, `border-radius` and `background-clip`. Buttons, other scrollbar states and background images are not drawn. `scrollbar-gutter: stable` works only with these bars |
| `app-region`, `-webkit-app-region` | `drag` and `no-drag` work as in Electron. The value is inherited, so `getComputedStyle()` returns it for children, and a box that only overlaps a drag area doesn't move the window |
| `@font-face` | TTF and OTF sources only. WOFF and WOFF2 sources are skipped. Faces load when the page first draws |
| `border-radius` | Percentages in the corners of outlines and of `<img>` content use the box's `width` and `height` properties, so they are off on boxes that are sized by their content |
| `direction: rtl` | Logical properties, text alignment, floats, flex rows, grid columns and block boxes follow the direction. Text that mixes directions is not reordered |
| `@scope` | `@scope` without a selector applies to the whole page, not to the parent of its `<style>`. Scope proximity doesn't affect the cascade |
| `:host-context()` | Not supported, like in Safari |
| Counters | `@counter-style` is not supported |
| `<legend>` | In a `<fieldset>` with rounded corners, the legend is drawn inside the border instead of on it |
| Custom property animations | Only `@property` values with a `<length>`, `<number>`, `<percentage>`, `<length-percentage>`, `<color>`, `<angle>` or `<time>` syntax animate and transition. Other custom properties keep their value during keyframes, where browsers switch it halfway |
| Animations out of view | Animations and transitions that only paint and can't move the box (rotate, scale, opacity, colors, shadows) pause while the box is outside the window or clipped away. They go on at the current time when the box comes into view, and fire no `animationiteration` events meanwhile |
| `var()` fallbacks | Fallbacks nested more than 32 levels deep make the value invalid |
| `getComputedStyle()` | `grid-template-columns`, `grid-template-rows` and `auto` margins return the computed values, not the used sizes in pixels. `text-decoration-line` includes the lines a box gets from its ancestors. An explicit `overflow: visible` on a `<textarea>` stays `visible`, though the textarea scrolls like `auto` |
| CSSOM | `<link>` elements have no `sheet`, because `apps build` turns them into `<style>`. `@import` works only in bundled CSS, so `CSSImportRule.styleSheet` is `null` |

## Electron API

These parts differ from Electron:

| Feature | Support |
|---|---|
| Platforms | `activate`, `open-url`, `open-file`, `second-instance`, the about panel, login items, the single instance lock, `clipboard`, `dialog`, `shell`, `Menu`, `Notification`, `globalShortcut`, `safeStorage`, `powerMonitor` and `screen` work on macOS only. On Windows and Linux they throw an error. `nativeTheme` returns light defaults there |
| Exports | `Tray`, `protocol`, `systemPreferences`, `powerSaveBlocker`, `autoUpdater`, `crashReporter`, `desktopCapturer`, `net`, `utilityProcess`, `BaseWindow`, `MessageChannelMain`, `webFrame` and `webUtils` are not supported. Importing them fails at build time |
| `process` | `process.type` is `browser` in main and `renderer` in preload scripts. `process.versions.electron` is `40.0.0`, the first Electron release on Node.js 24 |
| Preload scripts | `preload` in `package.json` runs a module in each bundled page's isolated preload context. Page imports of that file receive its exports through the context bridge. A page can also declare `<script type="preload">`, inline or with `src`. There is no `webPreferences.preload`. Preloads run only in pages that ship with the app: bundled `apps:///` pages and files in the app folder. HTML strings from `load()` and other files get no preload. In the preload, `document` is a proxy for the page DOM: values are copied, page objects are handles, and functions can't be passed to the page this way |
| `loadFile()`, `load()` | A relative path is relative to `app.getAppPath()`. In main, `loadFile('<name>.html')` with a string literal is bundled like `import.meta.resolve()`, so its scripts, stylesheets and preload work in dev and built apps. Other paths load the file as it is, without its `src` scripts and linked stylesheets. Both return a Promise. A failed load rejects with `ERR_FILE_NOT_FOUND` or `ERR_FAILED` and emits `did-fail-load` |
| IPC | Values follow structured clone. Functions, symbols and promises throw `An object could not be cloned.`, but `contextBridge` values can hold them. Listeners run in a later task of the receiver, in send order. Exceptions in `ipcMain` listeners, and in `closed`, `destroyed` and `window-close` listeners, go to `uncaughtException`, or to the console |
| `BrowserWindow` `webContents` | `session`, zoom, edit commands, `print()`, `insertCSS()` and `setWindowOpenHandler()` are not supported. `executeJavaScript()` works, while page code still can't run strings |
| `WebContentsView` | Uses the system WebKit (WKWebView, WebView2 or WebKitGTK), so `did-fail-load` codes are the platform's, for example `NSURLError` codes on macOS. `will-navigate` can be cancelled on macOS only, and `will-redirect` is not emitted. With `setWindowOpenHandler()`, `allow` opens the page in a separate browser window and ignores `overrideBrowserWindowOptions`. `webPreferences` takes `partition`, `javascript`, `devTools` and the safe default values of `nodeIntegration`, `contextIsolation`, `sandbox` and `webSecurity`. Other keys throw an error. On macOS, `executeJavaScript()` on a page whose CSP blocks `eval` runs in an isolated world, which can't see page globals. On Windows and Linux it fails on such pages. `loadFile()` can't load bundled pages. A page's `window.apps.postMessage(data)` arrives as `ipc-message` with the channel `message` |
| `Menu` | Every role has Electron's label and accelerator. `reload` and `forceReload` reload the focused window. `resetZoom`, `zoomIn`, `zoomOut` and `shareMenu` are normal items that do nothing. Changes to a `MenuItem` after `setApplicationMenu()` need another `setApplicationMenu()`. `icon`, `sublabel` and `before`/`after` positions are ignored. Media key accelerators are not shown in menus, and `globalShortcut.register()` returns `false` for them |
| `shell`, `clipboard` | `openExternal()` opens only `http`, `https`, `mailto` and `tel` URLs. The clipboard reads and writes text and HTML only |
| `Notification` | Needs an app bundle identifier, so in `apps run` `show()` emits `failed` |
| `screen` | Point and rectangle conversion helpers are not supported |
| `open-url` | The URL scheme must be in the app's `Info.plist`. `apps build` doesn't add it, and `setAsDefaultProtocolClient()` is not supported |
| `relaunch()` | Works in built apps. `apps run` ignores it |
| `setPath()`, `setName()` | Change only the values that `app` returns. Web storage keeps its own location |
| `commandLine` | Switches can be added and read, but they don't change apps. `disableHardwareAcceleration()` does nothing, because apps always draws on the GPU |
| Login items | Need macOS 13 and a built app. Only `openAtLogin` and `status` are returned |
| Images | `dock.setIcon()` and `iconPath` take a file path. There is no `nativeImage` |
| `BrowserWindow` options | `width`, `height`, `x`, `y`, `useContentSize`, `center`, `minWidth`, `minHeight`, `maxWidth`, `maxHeight`, `resizable`, `movable`, `minimizable`, `maximizable`, `closable`, `alwaysOnTop`, `fullscreen`, `show`, `title`, `backgroundColor`, `frame`, `transparent`, `titleBarStyle` and `trafficLightPosition`. Other options, like `webPreferences`, throw an error. `titleBarStyle` is `default`, `hidden` or `hiddenInset` |
| `BrowserWindow` state | Getters return cached values. A setter changes them at once, and the real values arrive with the window events. For example, `getPosition()` is correct after `center()` once `move` fires. `resized` also fires after `setSize()` and `maximize()` |
| Frameless windows | On Linux, `frame: false` and a hidden title bar remove the window border, so the window can't be resized from its edges |
| `setAlwaysOnTop()` | `level` and `relativeLevel` are ignored. The window always uses the floating level |
| `BrowserWindow` on Windows and Linux | Sizes and bounds are the content area. `setPosition()`, `center()`, `hide()`, `showInactive()`, `blur()`, `unmaximize()`, the size limit setters, `setResizable()`, `setMovable()`, `setMinimizable()`, `setMaximizable()`, `setClosable()` and `setAlwaysOnTop()` throw an error. The size limit, `maximizable`, `closable` and `alwaysOnTop` options are ignored. `isMinimized()` is always `false`, and `restore()` focuses the window |
| Not supported | Recent documents, jump lists, user activities, `setAppLogsPath()`, `getFileIcon()`, `getAppMetrics()`, GPU info, proxy, certificates and accessibility APIs. In `BrowserWindow`: `getNormalBounds()`, `setOpacity()`, `setAspectRatio()`, `setProgressBar()`, `flashFrame()`, `setMenu()`, `removeMenu()`, `setIcon()`, `setSkipTaskbar()`, `setIgnoreMouseEvents()`, `setHasShadow()`, `setVisibleOnAllWorkspaces()`, `setKiosk()`, `setContentProtection()`, `setFocusable()`, `getParentWindow()`, parent and modal windows, `will-move`, `will-resize` and `vibrancy` |

## Node Integration

The main process, preload scripts and workers can import these built-in modules, with or without the `node:` prefix: `assert`, `buffer`, `child_process`, `crypto`, `dns`, `events`, `fs`, `http`, `https`, `module`, `net`, `os`, `path`, `perf_hooks`, `process`, `querystring`, `readline`, `stream`, `string_decoder`, `timers`, `tty`, `url`, `util`, `worker_threads` and `zlib`. Their `/promises`, `/strict`, `/posix`, `/win32`, `/types`, `/consumers` and `/web` subpaths also work. `node:sqlite` and `node:ffi` are available too.

h2o follows the Node.js 24 API, so `process.version` is `v24.0.0`. Deprecated and experimental APIs are not included. These parts differ from Node.js:

| Feature | Support |
|---|---|
| Other modules | `async_hooks`, `cluster`, `dgram`, `diagnostics_channel`, `domain`, `http2`, `inspector`, `punycode`, `repl`, `test`, `tls`, `trace_events`, `v8`, `vm` and `wasi` are not supported. Importing them fails at build time. A `require()` of them, or of any module that can't be found, throws `MODULE_NOT_FOUND` when it runs. `constants` and `sys` work, but `constants` has no named imports |
| `require()` | App code is ESM. CommonJS works in `node_modules`, with `module`, `__filename` and `__dirname`. `require()` with a computed name and `module.createRequire()` load built-in modules only. `require.resolve()` finds string literals only. `require.main` is `undefined`, because the entry is ESM. `require()` uses the package `main`; `import` uses `main`, then `module` |
| `import.meta.url`, `__filename` | In built apps they point into the app: `import.meta.url` is `apps:///` plus the path in the app, and `__filename` is that path in the app folder. `url.fileURLToPath()` turns the URL into the same path. `import.meta.filename` and `import.meta.dirname` work in main, preload scripts and workers; pages get `undefined`. Files outside the app folder keep their path relative to it, like `apps:///../shared/x.js` |
| `process` | No `stdin`. `memoryUsage()` returns `rss` only, the heap values are 0. An unhandled promise rejection is logged and does not stop the app. `process.kill()` doesn't work on Windows. A built app gets all of its arguments in `process.argv` after the executable, like Electron, so there is no script path. `process.exit()` throws in preload scripts |
| Timers | The main process runs until it quits, so `ref()` and `unref()` have no effect there. In `worker_threads` workers, `unref()` lets the worker end while the timer is pending |
| `perf_hooks` | `performance` records marks and measures only. There is no `timerify()`, `eventLoopUtilization()` or resource timing |
| `worker_threads` | Workers get the environment of the app process, not the parent's `process.env`, so `SHARE_ENV` and the `env` option have no effect. A `BroadcastChannel` in a page only reaches channels of the same page |
| `fs` | File descriptors are h2o handles, not OS descriptors. There are no `bigint` stats, `readv()`, `writev()`, `statfs()` or `glob()`. `openAsBlob()` reads the whole file into memory. A FileHandle has no `readLines()` or `readableWebStream()` |
| `child_process` | No `fork()` and no IPC channel, so `'ipc'` in `stdio` throws. `stdio` takes the descriptors 0, 1 and 2 only. The `uid` and `gid` options throw on Windows. The child's `stdout` and `stderr` are readable streams, not sockets |
| `crypto` | Hashes are `md5`, `sha1`, `sha224`, `sha256`, `sha384` and `sha512`. HMAC and `pbkdf2()` support all of them. Ciphers are `aes-128-gcm` and `aes-256-gcm` with a 12-byte IV, and a cipher keeps the message in memory until `final()`. There is no `KeyObject`, `sign()`, `verify()`, key pair generation, `scrypt()`, `hkdf()` or Diffie-Hellman. Use `crypto.subtle` for ECDSA P-256 |
| `zlib` | Gzip, deflate and raw deflate only, there is no Brotli or Zstd. `params()` keeps the old level when the new one needs a different strategy. Streams compress on the JavaScript thread |
| `os` | `cpus()` times are 0. On Windows, `networkInterfaces()` returns `{}`, `loadavg()` returns zeros, `version()` returns `Windows`, and `getPriority()` and `setPriority()` throw |
| `dns` | Only `lookup()` is supported |
| `http`, `https` | `request()` uses `fetch`. It sends the body when `end()` is called, follows redirects, decodes gzip responses (without their `content-encoding` header) and reports the standard status text. `Agent` exists, but requests don't use its sockets. Servers listen on TCP ports only, by default on `0.0.0.0`. `https.createServer()` throws, because h2o has no TLS server. `https.request()` checks certificates against the system roots. `rejectUnauthorized`, `ca`, `cert` and `key` are ignored, so servers with self-signed certificates fail |
| `net` | The default host is IPv4. There is no `'lookup'` event, `BlockList`, `SocketAddress` or automatic family selection |
| `node:sqlite` | No extensions, limits or `Session` |
| `readline` | Reads lines from streams. There is no terminal line editing, history or completion |
| `util.inspect()` | The state of a Promise, the items of a Map or Set iterator and Proxy details are not shown |
| `util.types` | `isProxy()`, `isExternal()` and `isKeyObject()` always return `false` |
| `buffer` | `kMaxLength` is 2^32 - 1, the JavaScriptCore limit. `transcode()` converts between `ascii`, `latin1`, `utf16le` and `utf8` |
| `events` | `getEventListeners()` returns `[]` for an `EventTarget`. There is no `EventEmitterAsyncResource` |
| `tty` | There is no stdin, so `tty.ReadStream` throws. `tty.WriteStream` works for descriptors 1 and 2 when they are terminals, and has no `columns` or `rows` |
