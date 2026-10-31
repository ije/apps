# @pierre/apps
 
A tiny(small bundle), high performance, low memory usage and memory-safe electron alternative with compatible API design.

- `apps run [entry]`: Compile and run the app in memory. Defaults to `main.ts`, then `main.js`.
- `apps run --watch` / `-W`: Restart when the entry or its imports change. Watching is off by default.
- `apps run --devtools` / `-D`: Open Developer Tools in a standalone window.
- `apps run --enable-devtools-mcp [--devtools-mcp-port <port>]`: Serve devtools to agents on `127.0.0.1`. The default port is `19876`, and `0` picks a free port. It is off by default.
- `apps run main.js a b` or `apps run main.js -- --flag`: Pass arguments to the app. `process.argv` is `[execPath, entry, ...arguments]`.
- `apps skill`: Print the agent skill that explains the devtools server.
- `apps build [entry] [--out-dir <directory>] [--platform <platform>] [--arch <arch>]`: Create a production app with Bun's JavaScriptCore. Defaults to `main.ts`, then `main.js`.

The devtools server gives agents screenshots, DOM, JavaScript evaluation, console, mouse and keyboard input, memory, CPU and frame profiles, and a breakpoint debugger that uses source maps.
Agents call the tools with plain HTTP (`curl -s http://127.0.0.1:<port>/` lists them), or connect an MCP client to `http://127.0.0.1:<port>/mcp`.
To teach an agent the HTTP API, install the skill, for example `mkdir -p ~/.claude/skills/apps-devtools && apps skill > ~/.claude/skills/apps-devtools/SKILL.md`.

`apps run` keeps source maps in memory and reads assets and native addons from their original paths. It does not create `.apps/`.
`apps build` writes to `dist/` by default and does not generate source maps.

WebGPU, `<video>` and `<audio>` are off by default. Turn them on with `features` in `package.json`:

```json
{
  "name": "my_app",
  "features": ["webgpu", "video", "audio"]
}
```

`"name"` enables a feature, `"-name"` disables it, and `"*"` means all features. Entries apply in order.
A disabled feature is not loaded, and its API throws an error when called. On macOS and Windows, WebGPU is a separate library (3.2 MB) that `apps build` copies only for apps that enable it. Linux always includes it, because the renderer uses wgpu.

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
