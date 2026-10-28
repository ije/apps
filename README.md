# @pierre/apps
 
A tiny(small bundle), high performance, low memory usage and memory-safe electron alternative with compatible API design.

- `apps run [entry]`: Compile and run the app in memory. Defaults to `main.ts`, then `main.js`.
- `apps run --watch` / `-W`: Restart when the entry or its imports change. Watching is off by default.
- `apps run --devtools` / `-D`: Open Developer Tools in a standalone window.
- `apps build [entry] [--out-dir <directory>] [--platform <platform>] [--arch <arch>]`: Create a production app with Bun's JavaScriptCore. Defaults to `main.ts`, then `main.js`.

`apps run` keeps source maps in memory and reads assets and native addons from their original paths. It does not create `.apps/`.
`apps build` writes to `dist/` by default and does not generate source maps.

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
The `windowClose` handler runs after each window closes. The `app.hasWindows` getter reports whether any windows remain.
The `app.platform` value matches `process.platform` (`darwin`, `linux`, or `win32`).
The `fetch` handler starts a server with the same options as `serve()`.
The default port is `3000`. Calls to `app.on()` and `app.whenReady()` also work.

## JSC

The runtime uses [Bun's WebKit fork](https://github.com/oven-sh/WebKit).
The build enables Bun's JSC additions with the generic event loop.
`scripts/jsc.patch` restores C API locks and adds support for shared libraries.

```sh
bash scripts/build-jsc.sh
```
