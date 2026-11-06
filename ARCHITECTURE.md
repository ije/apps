# Architecture

- Electron compatible API
- Standard Web API
- DOM and CSS drawn by a GPU renderer, with built-in Tailwind CSS
- JavaScriptCore runs page scripts and preloads. A Rust app uses its Cargo binary as the main entry and registers native handlers, which run on a thread pool. JavaScript apps also use JavaScriptCore for the main entry.

## Threads

| Thread | Runs |
|---|---|
| Main | AppKit, native windows, menus and web views, and the main side: the main script or the Rust startup callback |
| Render | One for each `BrowserWindow`: its page scripts and preloads in their own JavaScriptCore VM and heap, its event loop, DOM, CSS, layout, paint and Metal. It takes display link ticks directly, so a busy main thread or another window stops no frames |
| Handlers | The Rust handlers of `ipcRenderer.invoke()`, on a pool of up to 4 threads |
| Workers | Each JS worker, and any thread that Rust code starts. Rust threads reach pages with `AppHandle` |

The main side and the pages share no JavaScript state. Each has its own `Host`, and they talk only through the queues in `crates/runtime/src/desktop/link.rs`. These are threads, not OS processes, so a crash in native code ends the whole app.

A page or preload can block on main: `window.open()`, `ipcRenderer.sendSync()`, the clipboard and opening a window. Main never blocks on a page without a limit. AppKit needs some answers at once, like whether a page handled a key and the IME ranges, so main asks the window's render thread and waits for at most 100 ms, running its own work meanwhile. After that it answers as if the page ignored the question. A frame for a live resize gets 8 ms, because the window server moves the old frame until AppKit's transaction commits. A later frame shows after the commit, anchored to the top left, so a slow page lags only at the window's edge.

Window commands like `setBounds()`, `minimize()` and `close()` run on main and don't wait for the page. `close()` and quitting also stop a running page script, so a hung page can't hold them up.

Render threads exist on macOS. On Windows and Linux the main side and all pages run on the main thread, so a busy script delays page frames there.

## Security

By default script execute and dynamic importing are disabled:

- `new Function()`
- `eval()`
- `import("data:...")`
- Remote JS and HTTP imports
- Remote CSS

To enable the two features, add `jsExecute` and `httpImport` in `features` option in `package.json`

> The limitation applies only to the `BrowserWindow` class, `WebContentsView` respects the webview sandboxing.

## Memory

By default macOS malloc keeps freed memory in the process footprint: about 40% of small blocks and all blocks of 48 KB or more. Only `MallocSpaceEfficient=1` changes that, and malloc reads it before any app code runs. So `apps run` sets the variable, `apps build` puts it in `LSEnvironment`, and a process started without it starts itself again once, which takes about 10 ms. The runtime then removes it, so `process.env` and child processes don't see it.

That mode makes malloc slower, so on macOS the runtime has a global allocator that keeps freed blocks of up to 512 bytes per thread, at most 64 KB per size, and gives them back when the thread is quiet. A Rust app can't define its own `#[global_allocator]`.

JSC's GC timers don't fire on the generic event loop, so each VM collects itself. A thread that ran JS asks about once a second whether to collect: yes after a second that ran at least 100 ms of JS followed by 5 quiet seconds, or every 5 minutes when only a little ran. Apps have timers that never stop, so collecting more often uses more CPU at idle than the app itself. A collection also drops all compiled code, about 70% of a typical live heap, and functions compile again when they next run. A window thread waits while it draws more than a few frames a second, because the collection blocks drawing. On machines with 16 GB of RAM or more, JSC lets a heap grow to about 4x its live size before a full collection, so the runtime sets `heapGrowthMaxIncrease` to make that about 2x.

## Node Integration

- Can import nodejs internal modules from `node:child_process`, `node:fs`, etc
- Global `process` object
- Support NAPI (`.node` module)
- Support FFI via `node:ffi`

> Unlike Electron, which bundles Node.js, `@pierre/apps` provides limited node integration via rust code.

With `ELECTRON_RUN_AS_NODE=1` the executable runs one script without AppKit or windows, so an app can keep a server in a child process. `import.meta.resolve('./server.ts')` bundles the script as a chunk and returns an `apps:///script-*.js` URL for it, and `child_process` can pass it extra `stdio` pipes.

## Web Views

`WebContentsView`, `<webview>` and `<iframe>` show pages in a system web view (WKWebView, WebView2 or WebKitGTK) above the GPU frame, so CSS can't clip or blend them. A `<webview>` is a `WebContentsView` that main keeps over the element: its page reads the box each frame and sends main the bounds when they change, and main sends the view's events back to the element. These views have no Chrome DevTools Protocol, so `webContents.debugger` answers only the commands that have a web view equivalent.

## App File

`apps build` packs the app into one file, `app.bundle`. It sits next to the executable, or in `Contents/Resources` of a macOS app.

The file starts with an 8-byte header, `APP\0` and the format version, followed by [postcard](https://postcard.jamesmunns.com/) data:

- `files`: `app.js`, script chunks (`script-*.js`), `package.json`, `native.json`, `assets.json` and packed `.node` addons
- `pages`: the bundled HTML pages, already parsed, with their `<style>` and `style` attributes parsed too

The runtime maps the file into memory. Script chunks and addons stay in the mapping until they are used, so the file must not change while the app runs, and a new build replaces it instead of rewriting it. `crates/runtime/src/bundle.rs` reads and writes the file.

A folder with `app.bundle` or `app.js` is a built app. It passes all of its arguments to `process.argv`, and it loads JavaScriptCore only from its own folder.

## Renderer

`crates/renderer` (`apps-renderer`) is apps' window system and GPU renderer for DOM and CSS: a box tree kept per window, text shaping, an atlas, and one renderer per platform. The platform code started from [Zed's GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui).

Boxes are kept across frames. A box keeps its identity (element id, or position in the nearest box with an id). Each frame the view describes only what changed and links unchanged subtrees with `Child::Kept`, so they are not built, measured or laid out again. A changed box drops the layout caches of itself and its ancestors only, and taffy skips the rest. Measured leaves (text) keep their state while it compares equal.

Boxes paint in CSS order: a stacking context lists the boxes with a `z-index` below it and paints them around its children. Code that draws more than a box (text, effects, sticky and fixed offsets) runs as `Hooks` around the box each frame.

| Platform | Window | Renderer | Text |
|---|---|---|---|
| macOS | AppKit | Metal, with layers tagged sRGB so wide-gamut displays show CSS colors like browsers. Shaders are compiled at build time when the Metal toolchain is installed | CoreText |
| Windows | Win32 | Direct3D 11 with DirectComposition | DirectWrite |
| Linux | X11 and Wayland | wgpu on Vulkan or OpenGL | cosmic-text |


## Offscreen Canvases

An `OffscreenCanvas` and an `ImageBitmap` keep their pixels in a detached `apps-canvas` node, the same way a `<canvas>` keeps them, so a dead wrapper frees them. Unlike `<canvas>`, the node keeps no second copy to show, and drawing on it costs the page no frames. A page worker has its own hidden page for these nodes, and its own text system for canvas text.

`transferControlToOffscreen()` links the worker's node to the `<canvas>` of the page. Each commit puts the newest frame in a slot of that page and wakes its thread, so a page that is behind skips frames and a fast worker can't queue them. WebGPU frames reach the CPU by readback, so a worker that draws with WebGPU polls for them until its frames stop.

## Media

`crates/media` (`apps-media`) plays `<video>` and `<audio>` for the `video` and `audio` features. The runtime loads it like the WebGPU library, and it links `libffmpeg`, one shared library that `scripts/build-ffmpeg.sh` builds into `target/ffmpeg`. The library exports only the functions that `crates/media/native/ffmpeg.c` calls, so run the script again when that file calls a new one. FFmpeg must stay LGPL: no GPL or nonfree options, dynamic linking only, and `FFmpeg.LICENSE` ships next to it.

```sh
bash scripts/build-ffmpeg.sh
```

## FPS HUD

`crates/perfhud` (`apps-perfhud`) is the FPS HUD that `apps run` shows on Cmd+Alt+F. The runtime loads the library next to its executable, and `apps build` never ships it, so built apps keep only the small hook in `crates/runtime/src/render/hud.rs`.

## JSC

The runtime uses [Bun's WebKit fork](https://github.com/oven-sh/WebKit).
The build enables Bun's JSC additions with the generic event loop.
`scripts/jsc.patch` restores C API locks and adds support for shared libraries.

```sh
bash scripts/build-jsc.sh
```
