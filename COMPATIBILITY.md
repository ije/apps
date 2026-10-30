# Compatibility

## DOM

Pages get the standard DOM API (nodes, elements, events, ranges, selection, observers, shadow DOM and custom elements). These parts differ from browsers:

| Feature | Support |
|---|---|
| `NodeList`, `HTMLCollection` | Static arrays with `item()`. They don't update when the tree changes |
| `DocumentType`, `CDATASection`, `ProcessingInstruction` | Not supported. `Attr` is not a `Node` |
| `DOMParser`, XPath, XSLT | `DOMParser` parses `text/html` only. XPath and XSLT are not supported |
| `*AttributeNS()` | The namespace is ignored |
| `<template>` | Parsed children move into `content` when `content` is first read |
| `eval()`, `new Function()` | Throw an `EvalError` in pages and page workers, as do the constructors of async and generator functions |
| Inline event attributes (`onclick="..."`) | Not run, because `eval` is disabled. Use `addEventListener` or `el.onclick = fn` |
| Navigation | Pages can go to bundled `apps:///` pages and local `file:///` files only. Other URLs throw. Use `WebContentsView` or `shell.openExternal()` for websites |
| `Worker` | Loads bundled, `data:`, `blob:` and local scripts. Remote scripts are rejected |
| Long scripts | A page script that runs for more than 10 seconds without returning is stopped, so one page cannot freeze the app |
| Forms | No `submit` event, `<select>` popup or `<button>` default action. Clicks on checkbox and radio inputs work |

## CSS

Pages use standard CSS, drawn by h2o on the GPU. Transforms, filters, blend modes, masks and clip paths paint through offscreen GPU layers.
These parts differ from browsers. The ones that don't work show an error in the Developer Tools console (`apps run -D`).

| Feature | Support |
|---|---|
| `transform-style: preserve-3d` | 3D children are flattened into the parent's plane |
| `filter: url()` | Only `<filter>` elements in the page, with `feGaussianBlur`, `feColorMatrix`, `feOffset` and `feDropShadow` |
| `clip-path: shape()` | Not supported |
| `line-height`, `vertical-align` in `::first-letter` and `::first-line` | Ignored. Sizes, colors and font styles work |
| `float` | Text next to a float doesn't move under it when the float ends |
| Multi-column layout | Columns break between child blocks. One paragraph doesn't split across columns |
| `writing-mode: vertical-lr` | Lines are ordered right to left, like `vertical-rl` |
| Tailwind CSS v4 | `@import "tailwindcss"` compiles the classes in the page when they appear, with `@theme`, `@custom-variant`, `@utility` and `@apply`. `@plugin`, `@source`, `@config`, container query variants (`@md:`) and the `prefix()`, `source()` and `important` import options are not supported |
| Scrollbars | Drawn as macOS overlay bars on all platforms. They show while the box scrolls or the pointer is on them, then fade out, and they take no space. The knob is light on a dark `color-scheme` or background. The macOS "Always" scroll bar setting is ignored. `scrollbar-width` and `scrollbar-color` work |
| `::-webkit-scrollbar` | Gives bars that always show and take space, as in Chromium. `-track`, `-thumb`, `-thumb:hover` and `-corner` use the background color, border, `border-radius` and `background-clip`. Buttons, other scrollbar states and background images are not drawn. `scrollbar-gutter: stable` works only with these bars |
| `app-region`, `-webkit-app-region` | `drag` and `no-drag` work as in Electron. The value is inherited, so `getComputedStyle()` returns it for children, and a box that only overlaps a drag area doesn't move the window |
| `@font-face` | TTF and OTF sources only. WOFF and WOFF2 sources are skipped. Faces load when the page first draws |

## Electron API

These parts differ from Electron:

| Feature | Support |
|---|---|
| Platforms | `activate`, `open-url`, `open-file`, `second-instance` and the native calls (about panel, login items, single instance lock) work on macOS only. On Windows and Linux the native calls throw an error |
| `open-url` | The URL scheme must be in the app's `Info.plist`. `apps build` doesn't add it, and `setAsDefaultProtocolClient()` is not supported |
| `relaunch()` | Works in built apps. `apps run` ignores it |
| `setPath()`, `setName()` | Change only the values that `app` returns. Web storage keeps its own location |
| `commandLine` | Switches can be added and read, but they don't change h2o. `disableHardwareAcceleration()` does nothing, because h2o always draws on the GPU |
| Login items | Need macOS 13 and a built app. Only `openAtLogin` and `status` are returned |
| Images | `dock.setIcon()` and `iconPath` take a file path. There is no `nativeImage` |
| `BrowserWindow` options | `width`, `height`, `x`, `y`, `useContentSize`, `center`, `minWidth`, `minHeight`, `maxWidth`, `maxHeight`, `resizable`, `movable`, `minimizable`, `maximizable`, `closable`, `alwaysOnTop`, `fullscreen`, `show`, `title`, `backgroundColor`, `frame`, `transparent`, `titleBarStyle` and `trafficLightPosition`. Other options, like `webPreferences`, throw an error. `titleBarStyle` is `default`, `hidden` or `hiddenInset` |
| `BrowserWindow` state | Getters return cached values. A setter changes them at once, and the real values arrive with the window events. For example, `getPosition()` is correct after `center()` once `move` fires. `resized` also fires after `setSize()` and `maximize()` |
| Frameless windows | On Linux, `frame: false` and a hidden title bar remove the window border, so the window can't be resized from its edges |
| `setAlwaysOnTop()` | `level` and `relativeLevel` are ignored. The window always uses the floating level |
| `BrowserWindow` on Windows and Linux | Sizes and bounds are the content area. `setPosition()`, `center()`, `hide()`, `showInactive()`, `blur()`, `unmaximize()`, the size limit setters, `setResizable()`, `setMovable()`, `setMinimizable()`, `setMaximizable()`, `setClosable()` and `setAlwaysOnTop()` throw an error. The size limit, `maximizable`, `closable` and `alwaysOnTop` options are ignored. `isMinimized()` is always `false`, and `restore()` focuses the window |
| Not supported | Recent documents, jump lists, user activities, `setAppLogsPath()`, `getFileIcon()`, `getAppMetrics()`, GPU info, proxy, certificates and accessibility APIs. In `BrowserWindow`: `getNormalBounds()`, `setOpacity()`, `setAspectRatio()`, `setProgressBar()`, `flashFrame()`, parent and modal windows, `will-move`, `will-resize` and `vibrancy` |

## Node Integration

The main process, preload scripts and workers can import these built-in modules, with or without the `node:` prefix: `assert`, `buffer`, `child_process`, `crypto`, `dns`, `events`, `fs`, `http`, `https`, `module`, `net`, `os`, `path`, `perf_hooks`, `process`, `querystring`, `readline`, `stream`, `string_decoder`, `timers`, `tty`, `url`, `util`, `worker_threads` and `zlib`. Their `/promises`, `/strict`, `/posix`, `/win32`, `/types`, `/consumers` and `/web` subpaths also work. `node:sqlite` and `node:ffi` are available too.

h2o follows the Node.js 24 API, so `process.version` is `v24.0.0`. Deprecated and experimental APIs are not included. These parts differ from Node.js:

| Feature | Support |
|---|---|
| Other modules | `async_hooks`, `cluster`, `dgram`, `diagnostics_channel`, `http2`, `inspector`, `repl`, `test`, `tls`, `v8`, `vm` and `wasi` are not supported. Importing them fails at build time |
| `require()` | App code is ESM. CommonJS works in `node_modules`. `module.createRequire()` loads built-in modules only |
| `import.meta.url` | In built apps it is `apps:///` plus the path in the app. `url.fileURLToPath()` turns it into a path in the app folder |
| `process` | No `stdin`. `memoryUsage()` returns `rss` only, the heap values are 0. An unhandled promise rejection is logged and does not stop the app. `process.kill()` doesn't work on Windows |
| Timers | The app runs until it quits, so `ref()` and `unref()` have no effect |
| `fs` | File descriptors are h2o handles, not OS descriptors. There are no `bigint` stats, `readv()`, `writev()`, `statfs()` or `glob()` |
| `child_process` | No `fork()` and no IPC channel. The `uid` and `gid` options are not supported |
| `crypto` | Hashes are `md5`, `sha1`, `sha224`, `sha256`, `sha384` and `sha512`. HMAC supports the SHA-1 and SHA-2 hashes. Ciphers are `aes-128-gcm` and `aes-256-gcm` with a 12-byte IV. `pbkdf2()` works. There is no `KeyObject`, `sign()`, `verify()`, key pair generation, `scrypt()`, `hkdf()` or Diffie-Hellman. Use `crypto.subtle` for ECDSA P-256 |
| `zlib` | Gzip, deflate and raw deflate only, there is no Brotli. Streams from `createGzip()` and the other `create*()` functions ignore `level` |
| `os` | `cpus()` times are 0. On Windows, `networkInterfaces()` returns `{}`, `loadavg()` returns zeros and `version()` returns `Windows` |
| `dns` | Only `lookup()` is supported |
| `http`, `https` | `request()` uses `fetch`. It sends the body when `end()` is called, and there is no `Agent` |
| `readline` | Reads lines from streams. There is no terminal line editing, history or completion |
| `util.inspect()` | No colors and no Proxy details. The state of a Promise is not shown |
