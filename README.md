# @pierre/apps
 
A tiny(small bundle), high performance, low memory usage and memory-safe electron alternative with compatible API design.

The Cargo workspace keeps each component in `crates/`:

- `cli`: The `apps` command, JavaScript bundler, and app packages. `src/commands` contains the command handlers. `src/packaging.rs` creates app directories and archives.
- `runtime`: JavaScriptCore contexts, desktop APIs, DOM, CSS, and page views.
- `renderer`: Native windows, text layout, and GPU rendering.
- `webview`: System webviews.
- `webgpu`, `media`, and `perfhud`: WebGPU, media playback, and the FPS HUD.

The workspace root selects `cli` by default. Use `--workspace` to build or check all crates.
The Rust APIs use `jsc::JavaScriptCore` for the shared JavaScriptCore library and `jsc::JsContext` for each JavaScript context.
`JsContext::load_bundle()` executes a bundle, and `JsContext::validate_bundle()` checks it without execution.

- `apps run [entry]`: Compile and run the app in memory. Defaults to `main` in `package.json`, then `index.ts`, `index.mts`, `index.mjs` or `index.js`.
- `apps run --watch` / `-W`: Restart when the entry or its imports change. Watching is off by default.
- `apps run --devtools` / `-D`: Open Developer Tools in a standalone window.
- `apps run --enable-devtools-mcp [--devtools-mcp-port <port>]`: Serve devtools to agents on `127.0.0.1`. The default port is `19876`, and `0` picks a free port. It is off by default.
- `apps run main.js a b` or `apps run main.js -- --flag`: Pass arguments to the app. `process.argv` is `[execPath, entry, ...arguments]`.
- `apps skill`: Print the agent skill that explains the devtools server.
- `apps build [entry] [--out-dir <directory>] [--platform <platform>] [--arch <arch>]`: Create a production app with Bun's JavaScriptCore. Defaults to `main` in `package.json`, then `index.ts`, `index.mts`, `index.mjs` or `index.js`.

The devtools server gives agents screenshots, DOM, JavaScript evaluation, console, mouse and keyboard input, memory, CPU and frame profiles, and a breakpoint debugger that uses source maps.
Agents call the tools with plain HTTP (`curl -s http://127.0.0.1:<port>/` lists them), or connect an MCP client to `http://127.0.0.1:<port>/mcp`.
To teach an agent the HTTP API, install the skill, for example `mkdir -p ~/.claude/skills/apps-devtools && apps skill > ~/.claude/skills/apps-devtools/SKILL.md`.

`apps run` keeps source maps in memory and reads assets and native addons from their original paths. It does not create `.apps/`.
`apps build` writes to `dist/` by default and does not generate source maps.
On macOS, `identifier` in `package.json` sets the bundle ID (`CFBundleIdentifier`). It defaults to `com.apps.<name>`, with `_` changed to `-`.
It packs imported `.node` addons into the app bundle and writes each one to the user's cache folder the first time it loads.
It copies the contents of `public/` into the app folder, and the files and folders in `files` of `package.json` at their path in the app, so `join(import.meta.dirname, './lib/math.dylib')` finds the same file in `apps run` and built apps:

```json
{
  "files": ["./lib/math.dylib"]
}
```

`imports` in the app's `package.json` works like Node's [subpath imports](https://nodejs.org/api/packages.html#subpath-imports). Keys without `#` also map bare specifiers in every module, dependencies included, like an import map. Use it to replace a package that a dependency imports but the app doesn't need:

```json
{
  "imports": {
    "shiki": "./shiki.ts",
    "shiki/*": "./shiki.ts"
  }
}
```

Set `preload` in `package.json` to expose module exports to page scripts:

```json
{ "name": "my_app", "preload": "./preload.ts" }
```

```ts
// preload.ts
export function foo() {
  return process.versions.node
}
```

```ts
// page.ts
import { foo } from './preload'

console.log(foo())
```

The preload path is relative to `package.json`. Each bundled page runs this module once in its isolated preload context with Node APIs.
Page imports of this file receive its exports through the context bridge. Functions run in the preload context. Other values are copies.

Named exports, default exports, callbacks, promises, and literal dynamic imports work. Imports wait for asynchronous preload initialization.
The page has no direct access to Node APIs or private preload variables. Export only the API that the page needs.
`<script type="preload">` scripts and `contextBridge.exposeInMainWorld()` also work.

Set `loaders` in `package.json` to import other file types. Each key is a file extension, and each value is a module path relative to `package.json` or a package name:

```json
{
  "name": "my-app",
  "loaders": {
    ".vue": "./vue-loader.ts"
  }
}
```

```ts
// vue-loader.ts
export default {
  load({ filename, content }) {
    return { type: 'ts', code: '...', map: { /* source map */ } }
  }
}
```

`apps run` and `apps build` run the loaders with Node APIs. `load()` gets the absolute path and the text of each imported file with that extension, in main, preload and page scripts, `node_modules` included. It returns `type` (`js`, `jsx`, `ts` or `tsx`), `code` and an optional source map, as an object or a JSON string, or a Promise of them. Errors and stack traces map to the original file. See [examples/vue](examples/vue).

WebGPU, `<video>`, `<audio>`, code from strings and remote imports are off by default. Turn them on with `features` in `package.json`:

```json
{
  "name": "my_app",
  "features": ["webgpu", "video", "audio", "jsExecute", "httpImport"]
}
```

`"name"` enables a feature, `"-name"` disables it, and `"*"` means all features. Entries apply in order.
A disabled feature is not loaded, and its API throws an error when called. On macOS and Windows, WebGPU is a separate library (3.2 MB) that `apps build` copies only for apps that enable it. Linux always includes it, because the renderer uses wgpu.

`jsExecute` lets pages use `eval()`, `new Function()` and `import()` of `data:` and `blob:` URLs. `httpImport` lets apps import `http` and `https` URLs: `apps build` and `apps run` download the imports, `<script src>`, `<link rel="stylesheet">` and CSS `@import` with a string literal URL, and bundle them. Remote files can import other files only by relative or absolute URLs. An `import()` without a string literal stays in the code and loads its URL when it runs. It needs `jsExecute` or `httpImport`.

`video` and `audio` play media with the FFmpeg shared libraries, under the LGPL 2.1. Build them once with `bash scripts/build-ffmpeg.sh` before `cargo build --workspace`. `apps build` copies the media library and FFmpeg (about 7 MB) with `FFmpeg.LICENSE` only for apps that enable one of them, so users can replace FFmpeg. Hardware decoding uses VideoToolbox on macOS and Direct3D 11 on Windows.

The main module can export an object with lifecycle handlers and an HTTP server:

```ts
import { BrowserWindow } from '@pierre/apps'

function createWindow() {
  const win = new BrowserWindow({ width: 800, height: 600 })
  win.loadFile(import.meta.resolve('./index.html'))
}

export default {
  ready(app) {
    createWindow()
  },
  activate(app) {
    if (!app.hasWindows) createWindow()
  },
  windowClose(app) {
    if (!app.hasWindows && app.platform !== 'darwin') app.quit()
  },
  fetch(req) {
    return new Response('hello world')
  },
  port: 8080,
}
```

All handlers are optional. Lifecycle handlers receive `app` and can be async.
The `ready` handler runs once after the main module and the app are ready.
The `windowClose` handler runs after each window closes. Calls to `app.on()` and `app.whenReady()` also work.
The `fetch` handler starts a http server with the same options as `serve()`.
The default port is `3000`.
