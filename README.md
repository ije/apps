# @pierre/apps

A tiny, fast, memory-safe Electron alternative with a compatible API.

- `apps run [entry]`: Compile and run the app in memory. Defaults to `main` in `package.json`, then `index.ts`, `index.mts`, `index.mjs` or `index.js`.
- `apps run --watch` / `-W`: Restart when the main code changes, and reload the pages when only page code changes. Watching is off by default.
- `apps run --devtools` / `-D`: Open Developer Tools in a standalone window.
- `apps run --enable-devtools-mcp [--devtools-mcp-port <port>]`: Serve devtools to agents on `127.0.0.1`. The default port is `19876`, and `0` picks a free port. It is off by default.
- `apps run main.js a b` or `apps run main.js -- --flag`: Pass arguments to the app. `process.argv` is `[execPath, entry, ...arguments]`.
- `apps skill`: Print the agent skill that explains the devtools server.
- `apps build [entry] [--out-dir <directory>] [--platform <platform>] [--arch <arch>] [--bytecode] [--aot]`: Create a production app with Bun's JavaScriptCore. Defaults to `main` in `package.json`, then `index.ts`, `index.mts`, `index.mjs` or `index.js`.
- `apps build --bytecode`: Also pack the bytecode of the app's scripts and of the runtime, so the app starts faster and uses less memory. The bundle grows by about 2.6 MB plus about 1.5 times the size of the app's JavaScript, and the build must be for this machine.
- `apps build --aot`: Compile the app's modules and the runtime scripts to native code, on macOS arm64. See [AOT build](#aot-build-experimental).

The devtools server gives agents screenshots, DOM, JavaScript evaluation, console, mouse and keyboard input, memory, CPU and frame profiles, and a breakpoint debugger that uses source maps.
Agents call the tools with plain HTTP (`curl -s http://127.0.0.1:<port>/` lists them), or connect an MCP client to `http://127.0.0.1:<port>/mcp`.
To teach an agent the HTTP API, install the skill, for example `mkdir -p ~/.claude/skills/apps-devtools && apps skill > ~/.claude/skills/apps-devtools/SKILL.md`.

`apps run` keeps source maps in memory and reads assets and native addons from their original paths. For JavaScript apps, it does not create `.apps/`.
`apps build` writes to `dist/` by default and does not generate source maps.
On macOS, `identifier` in `package.json` sets the bundle ID (`CFBundleIdentifier`). It defaults to `com.apps.<name>`, with `_` changed to `-`.
It packs imported `.node` addons into the app bundle and writes each one to the user's cache folder the first time it loads.
It copies only the files and folders in `files` of `package.json`, at their path in the app, so `join(import.meta.dirname, './lib/math.dylib')` finds the same file in `apps run` and built apps:

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

`paths` in the nearest `tsconfig.json`, with `baseUrl` and relative `extends`, map imports in app modules like TypeScript does, for example `"@/*": ["./src/*"]`. A CSS `@import` of a package uses its `exports` with the `style` condition, like Tailwind. So the [shadcn/ui manual installation](https://ui.shadcn.com/docs/installation/manual) works as written. See [examples/shadcn](examples/shadcn).

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

Bundled code also takes these:

```ts
import './theme.css';                             // adds a <style> to the page
import icon from './icon.png';                    // the URL of the file
import text from './notes.md?raw';                // the text of the file
import url from './data.bin?url';                 // the URL of the file
import MyWorker from './work.ts?worker';          // a Worker class
const pages = import.meta.glob('./pages/*.html', { eager: true, query: '?url', import: 'default' });
const version = import.meta.env.APP_VERSION;      // from `env` in package.json
const server = import.meta.resolve('./server.ts'); // a script that `ELECTRON_RUN_AS_NODE` can run
```

`productName` names the app, `executableName` names its executable, and `env` gives the values of `import.meta.env`.

On macOS, the built-in `bg-glass`, `bg-glass-clear` and `bg-glass-interactive` classes give a box native Liquid Glass (macOS 26 and later), and `<sf-symbol-*>` elements show SF Symbols, with no import:

```html
<button class="bg-glass-blue-500 bg-glass-interactive rounded-full flex items-center gap-2 px-4 py-2">
  <sf-symbol-folder-fill class="size-5" aria-hidden="true" />
  Open
</button>
```

Glass sets no corner radius. With `@import "tailwindcss"`, `bg-glass-*` takes the colors of `bg-*`, like `bg-glass-blue-500/50` or `bg-glass-brand` for a `@theme` color, and `supports-bg-glass:` and `not-supports-bg-glass:` style the fallback. The CSS properties are `-macos-glass`, `-macos-glass-interactive` and `-macos-glass-tint-color`. `visualEffectState: 'active'` in `BrowserWindow` options keeps glass active while the window has no focus. A symbol tag maps hyphens to dots, so `<sf-symbol-folder-fill>` is `folder.fill`. Symbols take the text color and are `1em` square. See [examples/glass](examples/glass) and [Compatibility](COMPATIBILITY.md).

Inputs with an `ns-*` type show AppKit controls on macOS, and the fallback type elsewhere:

| Type | macOS | Fallback |
|---|---|---|
| `ns-button` | `NSButton` | `button` |
| `ns-switch` | `NSSwitch` | `checkbox` |
| `ns-range` | `NSSlider`, or `NSStepper` with a `step` attribute | `range` |
| `ns-text` | `NSTextField` | `text` |
| `ns-search` | `NSSearchField` | `search` |
| `ns-color` | `NSColorWell` | `color` |
| `ns-date` | `NSDatePicker` | `date` |

```html
<label><input type="ns-switch" name="wifi" checked> Wi-Fi</label>
<input type="ns-range" min="1" max="10" step="1" value="2">
<input type="ns-button" value="Save">
```

The element's `type` property returns the fallback type, and the controls fire the same events as the fallback inputs. See [examples/nscontrol](examples/nscontrol).

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

`apps run` and `apps build` run the loaders with Node APIs. `load()` gets the absolute path and the text of each imported file with that extension, in main, preload and page scripts, `node_modules` included. It returns `type` (`js`, `jsx`, `ts` or `tsx`), `code` and an optional source map, as an object or a JSON string, or a Promise of them. Errors and stack traces map to the original file. See [examples/vue](examples/vue) and [examples/svelte](examples/svelte).

WebGPU, `<video>`, `<audio>`, code from strings and remote imports are off by default. Turn them on with `features` in `package.json`:

```json
{
  "name": "my_app",
  "features": ["webgpu", "video", "audio", "jsExecute", "httpImport"]
}
```

`"name"` enables a feature, `"-name"` disables it, and `"*"` means all features. Entries apply in order.
The `localNetwork` feature lets pages reach loopback and LAN addresses, for apps with a server in a child process.
The `reactCompiler` feature runs the [React Compiler](https://react.dev/learn/react-compiler) on page scripts outside `node_modules`, so components and hooks are memoized without `useMemo`. It needs React 19 or later, which has `react/compiler-runtime`.
A disabled feature is not loaded, and its API throws an error when called. On macOS and Windows, WebGPU is a separate library (3.2 MB) that `apps build` copies only for apps that enable it. Linux always includes it, because the renderer uses wgpu.

`jsExecute` lets pages use `eval()`, `new Function()` and `import()` of `data:` and `blob:` URLs. `httpImport` lets apps import `http` and `https` URLs: `apps build` and `apps run` download the imports, `<script src>`, `<link rel="stylesheet">` and CSS `@import` with a string literal URL, and bundle them. Remote files can import other files only by relative or absolute URLs. An `import()` without a string literal stays in the code and loads its URL when it runs. It needs `jsExecute` or `httpImport`.

`video` and `audio` play media with an FFmpeg shared library, under the LGPL 2.1. Build it once with `bash scripts/build-ffmpeg.sh` before `cargo build --workspace`. `apps build` copies the media library and FFmpeg (about 4 MB) with `FFmpeg.LICENSE` only for apps that enable one of them, so users can replace FFmpeg. Hardware decoding uses VideoToolbox on macOS and Direct3D 11 on Windows.

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

## macOS tray

`Tray` takes a file path. Ship the icon with `"files": ["icons"]` in `package.json` and load `join(app.getAppPath(), 'icons', 'TrayTemplate.png')`. A name that ends in `Template.png` lets macOS adapt the icon to the menu bar.

## AOT build (experimental)

`apps build --aot` compiles the app's modules and the runtime scripts to native code with Bun's AOT compiler.
It works for JavaScript and TypeScript apps on macOS arm64, and can't be combined with `--bytecode`.
Bun is needed only for the build.

Set `APPS_BUN` to a Bun binary from revision `2596ac5d391d47804c93a42bdbe615804aa62387` ([Bun PR #47942](https://github.com/oven-sh/bun/pull/47942)), and build JSC with AOT support.
Other revisions are rejected, because Bun's IR must match the format and the built-in table of the JSC compiler.

```sh
export APPS_BUN=/path/to/bun
bash scripts/build-jsc.sh --aot
cargo build -p apps -p apps-runtime
./target/debug/apps build index.ts --aot
```

`--aot` fetches Bun's source into `target/bun-aot`, and also builds `libJavaScriptCoreCompiler.dylib`, which the CLI loads to compile, and `libJavaScriptCoreAOT.a`, a JSC without JIT compilers and Inspector.
The CLI links the archive and the native code into `app.dylib` in `Contents/Resources`, in place of the app's JavaScriptCore library.

The main module, module chunks, page modules, preloads, workers and the runtime scripts compile.
Classic page scripts and code from strings run in JSC's interpreter.
Packaged `.wasm` files and valid Wasm in base64 string literals passed to `atob()` compile too. Other Wasm runs in the interpreter.

`app.bundle` keeps a reference with a build ID in place of each compiled script, so code that reads a script file gets the reference.
The app doesn't start when `app.dylib` comes from another build or another runtime.

Bun's AOT limits apply, for example to some changes of built-in objects.
On DiffsHub, AOT starts with about half the CPU and holds about 50 MB less memory, but the app is 16 MB bigger, and `%` on numbers that are not known to be integers is about 2 times slower than with the JIT.

To run the integration test:

```sh
APPS_JSC_LIBRARY="$PWD/target/bun-jsc/macos-aarch64/libJavaScriptCore.dylib" \
  cargo test -p apps --test aot -- --include-ignored
```

## Rust main

Set `type` to `rust` and point `main` at a Rust source file.

```json
{
  "name": "my-app",
  "main": "src/main.rs",
  "type": "rust",
  "preload": "preload.ts"
}
```

```rust
use apps::{app, BrowserWindow, WindowOptions};

fn main() -> apps::Result<()> {
    app::run(|app| {
        app.handle("greet", |_window, args| {
            let name = args.first().and_then(|v| v.as_str()).unwrap_or("world");
            Ok(format!("Hello, {name}!").into())
        })?;
        BrowserWindow::new(app, WindowOptions::default())?.load_file("index.html")
    })
}
```

An extension can be a Rust crate with a function such as `pub fn register(app: &apps::App) -> apps::Result<()>`. Call it from the startup callback to register its handlers.

The callback runs on the app's main thread. Handlers run on a small thread pool, so a slow handler doesn't stop the window. A handler receives the window ID and a list of JSON arguments. Its result resolves the preload's promise, and an error rejects it.

A handler that runs for a long time should start its own thread, so it doesn't hold a place in the pool. Use `app.app_handle()` to reach a window from any thread:

```rust
let handle = app.app_handle();
app.handle("start", move |window, _args| {
    let handle = handle.clone();
    std::thread::spawn(move || handle.send(window, "progress", 42.into()));
    Ok(apps::Value::Null)
})?;
```

```ts
// preload.ts
import { ipcRenderer } from '@pierre/apps'

export const greet = (name: string) => ipcRenderer.invoke('greet', name)
ipcRenderer.on('progress', (_event, value) => console.log(value))
```

Page scripts run in JavaScriptCore. They can import `greet` from the preload. The CLI bundles `index.html` if it exists. Set `"pages": ["index.html", "settings.html"]` to bundle more pages. Paths are relative to `package.json`.

Run `apps run` or `apps build` from the app folder. The CLI uses Cargo to compile the executable. It creates a manifest in `.apps/cargo/` if the app has no `Cargo.toml`. Set `APPS_RUST_SDK` to the `crates/runtime` directory if the CLI has moved away from its source checkout.

To add a crate, put a `Cargo.toml` next to `package.json`. Add `apps = { package = "apps-runtime", path = "/path/to/apps/crates/runtime" }` under `[dependencies]`. The path in `main` must match a Cargo binary target. Cargo detects `src/main.rs` by default. See [examples/rust](examples/rust) for a complete app.
