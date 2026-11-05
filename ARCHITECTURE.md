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

A page or preload can block on main: `window.open()`, `ipcRenderer.sendSync()`, the clipboard and opening a window. Main never blocks on a page without a limit. AppKit needs some answers at once, like whether a page handled a key, the IME ranges and a frame for a live resize, so main asks the window's render thread and waits for at most 100 ms, running its own work meanwhile. After that it answers as if the page ignored the question.

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

## Node Integration

- Can import nodejs internal modules from `node:child_process`, `node:fs`, etc
- Global `process` object
- Support NAPI (`.node` module)
- Support FFI via `node:ffi`

> Unlike Electron, which bundles Node.js, `@pierre/apps` provides limited node integration via rust code.

## Renderer

`crates/renderer` (`apps-renderer`) is apps' window system and GPU renderer for DOM and CSS: a box tree kept per window, text shaping, an atlas, and one renderer per platform. The platform code started from [Zed's GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui).

Boxes are kept across frames. A box keeps its identity (element id, or position in the nearest box with an id). Each frame the view describes only what changed and links unchanged subtrees with `Child::Kept`, so they are not built, measured or laid out again. A changed box drops the layout caches of itself and its ancestors only, and taffy skips the rest. Measured leaves (text) keep their state while it compares equal.

Boxes paint in CSS order: a stacking context lists the boxes with a `z-index` below it and paints them around its children. Code that draws more than a box (text, effects, sticky and fixed offsets) runs as `Hooks` around the box each frame.

| Platform | Window | Renderer | Text |
|---|---|---|---|
| macOS | AppKit | Metal. Shaders are compiled at build time when the Metal toolchain is installed | CoreText |
| Windows | Win32 | Direct3D 11 with DirectComposition | DirectWrite |
| Linux | X11 and Wayland | wgpu on Vulkan or OpenGL | cosmic-text |


## Media

`crates/media` (`apps-media`) plays `<video>` and `<audio>` for the `video` and `audio` features. The runtime loads it like the WebGPU library, and it links the FFmpeg shared libraries that `scripts/build-ffmpeg.sh` builds into `target/ffmpeg`. FFmpeg must stay LGPL: no GPL or nonfree options, dynamic linking only, and `FFmpeg.LICENSE` ships next to it.

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
