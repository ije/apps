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
| Blocking | Async `fs`, `dns.lookup()`, `zlib` and `crypto` work and Node-API async work of every context, on one pool of up to 4 threads, like libuv's |
| Workers | Each JS worker, and any thread that Rust code starts. Rust threads reach pages with `AppHandle` |

The main side and the pages share no JavaScript state. Each has its own `Host`, and they talk only through the queues in `crates/runtime/src/desktop/link.rs`. These are threads, not OS processes, so a crash in native code ends the whole app.

A page or preload can block on main: `window.open()`, `ipcRenderer.sendSync()`, the clipboard and opening a window. Main never blocks on a page without a limit. Keys come to main, which runs a window's `before-input-event` before it gives the key to the page, so a page never waits for main on its keys. AppKit needs some answers at once, like whether a page handled a key and the IME ranges, so main asks the window's render thread and waits for at most 100 ms, running its own work meanwhile. After that it answers as if the page ignored the question. A key that the input method takes goes to the page with the first change the input method makes, so main doesn't wait for it. When focus, a press or the window's focus ends a composition, the page keeps the text and asks main, without waiting, to make the input method drop its own. A frame for a live resize gets 8 ms, because the window server moves the old frame until AppKit's transaction commits. A later frame shows after the commit, anchored to the top left, so a slow page lags only at the window's edge.

Window commands like `setBounds()`, `minimize()` and `close()` run on main and don't wait for the page. `close()` and quitting also stop a running page script, so a hung page can't hold them up.

Main's work runs from a run loop source and timers in the common modes, so it goes on while a popup menu or a modal panel runs a nested loop. While a main script waits in such a loop, like `dialog.showMessageBoxSync()`, IPC and events that would run main scripts wait until it returns, keys skip `before-input-event`, and window commands and the native work of pages still run. `Menu.popup()` returns at once, as in Electron.

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

That mode makes malloc slower, so on macOS the runtime has a global allocator that keeps freed blocks of up to 512 bytes per thread, at most 1 MB per size, and gives them back when the thread is quiet. A Rust app can't define its own `#[global_allocator]`.

JSC's GC timers don't fire on the generic event loop, so each VM collects itself. A thread that ran JS asks about once a second whether to collect: yes after a second that ran at least 100 ms of JS followed by 5 quiet seconds, or every 5 minutes at a second that ran less than 100 ms, because some apps' timers never leave a quiet second. Apps have timers that never stop, so collecting more often uses more CPU at idle than the app itself. A collection also drops all compiled code, about 70% of a typical live heap, and functions compile again when they next run. A window thread waits while it draws more than a few frames a second, because the collection blocks drawing. On machines with 16 GB of RAM or more, JSC lets a heap grow to about 4x its live size before a full collection, so the runtime sets `heapGrowthMaxIncrease` to make that about 2x. JSC's sweeper, which frees the memory of dead cells such as string buffers, is a timer too, so every collection sweeps the heap at once, in a few ms.

A removed DOM node is freed at the next event loop turn, unless page code made a JS wrapper for it or a node below it. Then it waits for a GC that collects the wrapper, so each new wrapper counts as 64 KB of memory to JSC and GCs keep up with pages that rebuild their rows.

While a process draws with Metal, the driver on Apple GPUs holds about 126 MB for render encoders and 84 MB more for blit encoders, until about 1 second after the last frame, at any window size. So frames copy textures with draws and only `capturePage()` uses a blit. After the first 500 ms, Core Animation blinks a text caret, so a focused field draws no frames and the driver frees this memory. A caret in a box with an effect, under round clip corners or below a later box blinks by drawing.

A window that draws no frames for 1 second, or that can't be seen, frees what only drawing needs: its spare drawables, layer textures, instance buffers and atlas images that the last frame didn't show. The window keeps showing its last frame, and images keep their pixels on the CPU, so the next frame only allocates and uploads again. An atlas image bigger than 1024 px on a side gets its own texture, so a small tile can't keep that texture alive after the image goes.

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

With `apps build --bytecode`, bytecode follows: an index of names and sizes, then each one at an 8-byte aligned offset, because JSC reads it in place. The names are runtime script URLs like `apps:dom`, `app.js` for the main script with its module loader, and the names of script chunks.

The runtime maps the file into memory. Script chunks and addons stay in the mapping. JavaScriptCore reads an ASCII chunk directly from it, so the source doesn't count in the footprint and windows and workers share it. So the file must not change while the app runs, and a new build replaces it instead of rewriting it. `crates/runtime/src/bundle.rs` reads and writes the file.

A folder with `app.bundle` or `app.js` is a built app. It passes all of its arguments to `process.argv`, and it loads JavaScriptCore only from its own folder.

With `apps build --aot`, `libapps_aot.dylib` compiles the app's modules and the runtime scripts to IR, and `libJavaScriptCoreCompiler.dylib` compiles the IR to native code. The first is Bun's AOT front end: `build-jsc.sh --aot` builds Bun's Rust crates at the pinned revision with Bun's toolchain and codegen into one archive, and links a dylib that exports only the two functions of `scripts/aot-front-end`, so the dead code that needs Bun's C++ goes. Its IR is the bytes that `bun build --compile --aot` writes, so the CLI runs no Bun. It links the native code with `libJavaScriptCoreAOT.a`, a JSC without JIT compilers or Inspector, into `app.dylib`, which the app loads as its JavaScriptCore. The IR names built-ins by ID and compiled code reads JSC fields by offset, so both JSC builds use the same source and Bun's built-in table. The bundle keeps a reference with a build ID in place of each compiled script, and the runtime finds the code of a runtime script by its URL. A hash of the runtime scripts, from `build.rs`, ties `app.dylib` to one runtime. Each context locks the built-ins that compiled code relies on once, before its runtime scripts run. The images use wide references, so the runtime turns off JSC's cell cage, which keeps every block the heap holds resident: about 110 MB more on DiffsHub's stress test.

## Modules

`apps build` links ES modules through one array of slots per bundle. A module is a plain function that writes its exports to its slots when its body ends, and importers read `slots[k]` directly. Re-exports and `export *` resolve at build time to the module that holds the binding, so barrels cost nothing at run time. A binding that code can write after the body ends, in a function, a method or an instance field, keeps a closure in its slot, which importers call.

A module has a namespace object only where code needs one: `import *` used as a value, `import()`, `require()` and entries. Modules in import cycles, CommonJS, top-level await and modules that use `eval` keep the slower form, with a generator, a namespace and a getter per export. `apps run` uses that form for all modules, so it follows the standard.

## Renderer

`crates/renderer` (`apps-renderer`) is apps' window system and GPU renderer for DOM and CSS: a box tree kept per window, text shaping, an atlas, and one renderer per platform. The platform code started from [Zed's GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui).

Boxes are kept across frames. A box keeps its identity (element id, or position in the nearest box with an id). Each frame the view describes only what changed and links unchanged subtrees with `Child::Kept`, so they are not built, measured or laid out again. A changed box drops the layout caches of itself and its ancestors only, and taffy skips the rest. Like Blink, a box whose width can't depend on its height, and whose height can't depend on the height it may take (no grid, column flex or aspect ratio below it), keeps one cache entry for any available height, so parents that try several heights don't lay it out again. Measured leaves (text) keep their state while it compares equal.

A subtree is kept while the revision of its nodes and styles and the sizes it is built in are the same, and each read its build made beyond them gives the same value again: styles of ancestors and siblings, the last layout, scroll handles, images and the frame. The build logs these reads, and each box keeps a record of its own reads and of the reads below it that its revision doesn't cover. Page reads are checked again only after the page changed, the others in every frame. Images, video frames and canvas drawings paint from their node when the box paints, so the build reads only their sizes, and a new frame or drawing keeps every box. Fields paint their selection, caret and scroll offset, and native views take their bounds, in the same way. Controls record their hover and pressed state as reads. A text field's value is in no revision: when it changes, the frame builds only the field again and patches it into the kept tree. Tables, lists and quotes read their siblings, so they are kept only in frames where nothing changed. Debug builds build each kept box again and check that it reads and builds the same.

Boxes far from the view are culled: a parent replaces runs of plain blocks more than a viewport above or below the visible range with spacers of their size. Culling reads the last layout, moved by the scroll since. A subtree that culled is kept with its spacers, so a long list builds nothing while it scrolls, until a spacer comes within half a viewport of the view. Then the boxes down to the list build again, and of its rows only those that came into range. A scroll of more than half a viewport in one frame culls again at once. A subtree that culled nothing is kept only while no box below it that could be culled is out of range, also inside scroll boxes.

Styles are incremental too. The selector cache records which nodes each node's matches read, so a DOM change marks only the nodes that could match differently. Classes count apart from other attributes: a class that no selector tests outside its last compound, like a row's `selected`, restyles only its own element, and its children keep their matches. A style pass starts from the last output and styles those nodes, the children of nodes whose output changed, and every node after a new root font size. It walks the composed tree, so a shadow root inherits from its host and a slotted node from its slot, and each tree matches the rules of its own sheets. Pages with CSS counters or tables, and passes after a new viewport or a changed stylesheet, style every node. A node takes the matched rules of a sibling with the same tag, attributes and state when that sibling's matches read only itself and its ancestors, not positions or neighbors, so the rows of a long list match once. The per-node outputs are lists of 256-entry chunks that a pass copies on write, so a pass costs what it styles, not the length of the node arena, and the frame finds changed styles by comparing chunks. Removed nodes that JS still holds stay in the arena until a GC, so per-frame work must not visit every node. Debug builds check each incremental pass against a full one with an empty cache and no shared matches. The per-frame cull info works the same way: only nodes with a new revision, style or focus, and their ancestors, are visited again.

Boxes paint in CSS order: a stacking context lists the boxes with a `z-index` below it and paints them around its children. Code that draws more than a box (text, effects, sticky and fixed offsets) runs as `Hooks` around the box each frame. A box at opacity 0 draws nothing and takes no layer, but still lays out and takes events. Lines of ASCII text in a monospace font shape by runs of spaces and non-spaces, each once per font and size, since code repeats its words and indents.

| Platform | Window | Renderer | Text |
|---|---|---|---|
| macOS | AppKit | Metal, with layers tagged sRGB so wide-gamut displays show CSS colors like browsers. Shaders are compiled at build time when the Metal toolchain is installed | CoreText |
| Windows | Win32 | Direct3D 11 with DirectComposition | DirectWrite |
| Linux | X11 and Wayland | wgpu on Vulkan or OpenGL | cosmic-text |

On macOS, a box with Liquid Glass draws its content into its own Metal surface, which AppKit shows in an `NSGlassEffectView` above the page. The native material draws the highlights, refraction and tint. These frames need the main thread to update the views and present their surfaces together, so only the newest one waits for main and a newer one drops it: a busy main thread skips frames but never stops the page. Native glass is not in Metal readback, so `capturePage()` rejects these frames.

From the bottom, the native layers of a window are the frame, glass views, a view with the Core Animation layers of composited animations and the blinking caret, then web views. AppKit reorders sublayers added to the frame layer by hand, so none are. The renderer doesn't know where web views paint, so they stay on top.

`<sf-symbol-*>` elements rasterize with AppKit on the render thread on a cache miss and draw through the monochrome atlas, so a new CSS color needs no new bitmap. The cache keeps every symbol that the current frame uses, even past its bounds.

Inputs with an `ns-*` type show AppKit controls on macOS. Their box paints nothing and takes the control's natural size, measured once on main. When it paints, it gives the window the control's state, its bounds and the clip of its ancestors. After the frame, a changed list goes to main, which keeps each control in a clipping view above the frame. Main takes only the newest list, so a busy main thread skips lists. A control's action goes back to the page's thread as user input. Each event has a number, and the page's lists carry the last number it took, so main doesn't set a value from the page while the page has not yet taken the control's own edits.

The atlas keeps a tile that fails or is bigger than a GPU texture as an empty entry, so it is not built again each frame. A glyph bigger than 2048 device pixels on a side rasters at a smaller scale and stretches to its size.


## Offscreen Canvases

An `OffscreenCanvas` and an `ImageBitmap` keep their pixels in a detached `apps-canvas` node, the same way a `<canvas>` keeps them, so a dead wrapper frees them. Unlike `<canvas>`, the node keeps no second copy to show, and drawing on it costs the page no frames. A page worker has its own hidden page for these nodes, and its own text system for canvas text.

`transferControlToOffscreen()` links the worker's node to the `<canvas>` of the page. Each commit puts the newest frame in a slot of that page and wakes its thread, so a page that is behind skips frames and a fast worker can't queue them. WebGPU frames reach the CPU by readback, so a worker that draws with WebGPU polls for them until its frames stop.

## Media

`crates/media` (`apps-media`) plays `<video>` and `<audio>` for the `video` and `audio` features. The runtime loads it like the WebGPU library, and it links `libffmpeg`, one shared library that `scripts/build-ffmpeg.sh` builds into `target/ffmpeg`. The library exports only the functions that `crates/media/native/ffmpeg.c` calls, so run the script again when that file calls a new one. FFmpeg must stay LGPL: no GPL or nonfree options, dynamic linking only, and `FFmpeg.LICENSE` ships next to it.

```sh
bash scripts/build-ffmpeg.sh
```

## Watch

`apps run` sends the bundle to the app on stdin as one line of JSON. With `--watch`, it compiles the app again after a change and compares the new bundle with the running one. Pages and the script chunks that only pages load can change while the app runs: chunks that app.js names, and the chunks they name, belong to main. When only those changed, the new bundle goes on stdin as one more line, and each window loads its `apps:///` page again with the new files. Any other change restarts the app. A Rust app runs Cargo again only after a change to a `.rs` file, `Cargo.toml` or `Cargo.lock`. The runtime moves the pipe off fd 0 and puts `/dev/null` there, so child processes that inherit stdin get no bundles.

## npm Packages

`packages/apps` is `@pierre/apps`: the `apps` command, the types and no runtime code, because the bundler gives `@pierre/apps` imports the runtime's API. Each `packages/apps-<platform>-<arch>` holds the CLI with everything it loads from its own folder: the runtime, JavaScriptCore, and the WebGPU, media, FFmpeg, HUD and MCP libraries. The macOS arm64 one also has the AOT compilers of `apps build --aot`. npm installs only the one for the machine. `bin/apps.mjs` starts it from Node, and on macOS and Linux the install script replaces that file with a link to the native CLI, so no Node process waits while an app runs. The CLI resolves the link to find its folder. `scripts/build-npm.sh <version>` fills the package of the machine it runs on and sets the version of every package and of the Cargo workspace. `.github/workflows/release.yml` runs it on each platform for a `v*` tag, then publishes.

## FPS HUD

`crates/perfhud` (`apps-perfhud`) is the FPS HUD that `apps run` shows on Cmd+Alt+F. The runtime loads the library next to its executable, and `apps build` never ships it, so built apps keep only the small hook in `crates/runtime/src/render/hud.rs`.

## Devtools MCP Server

`crates/mcp` (`apps-mcp`) is the devtools server of `apps run --enable-devtools-mcp`: HTTP, MCP, the debugger and the profiler. Like the FPS HUD, it is a library next to the runtime that `apps build` never ships. The runtime keeps only `crates/runtime/src/desktop/mcp.rs`: the JSC inspector bridge and the work that needs the app, like window lists, screenshots, input and memory.

## JSC

The runtime uses [Bun's WebKit fork](https://github.com/oven-sh/WebKit).
The build enables Bun's JSC additions with the generic event loop.
`scripts/jsc.patch` restores C API locks and adds support for shared libraries. Each VM runs on one thread, so a native function keeps the lock while it runs, and the API calls it makes find the lock held and skip it. A call into a native function costs about 0.02 µs. Like Bun's event loop, one call into a context's JavaScript runs a whole loop turn and drains the microtasks after each task with `JSGlobalContextDrainMicrotasks`, so a task costs no call from Rust.

Like Bun, native code reads strings and bytes where JSC keeps them, without copies. It makes strings from Latin-1, UTF-16 or UTF-8 with one copy, and Uint8Arrays it fills without zeroing them. Conversions use the simdutf that JSC already has. `crates/runtime/src/jsc/encoding.rs` has `TextEncoder`, `TextDecoder`, the `Buffer` encodings, `atob()`, `btoa()`, `indexOf()` and `compare()` of `Buffer`.

Each VM runs the runtime scripts in `crates/runtime/js` before page or app code. The build removes their whitespace and comments and makes them ASCII, so the process keeps one copy, about 0.8 MB, that JSC reads in place. JSC's code cache is per VM, so when a second VM runs a runtime script, a thread compiles the script and all of its functions into bytecode, and later VMs decode that bytecode instead of compiling the script. A context in a new VM then starts in about 3 ms instead of 12 ms, and holds about 1 MB less. Decoded code points into the bytecode, so the process keeps it: about 2.6 MB for all runtime scripts. JSC compiles the source when the bytecode doesn't match it, for example while a debugger is attached.

`apps build --bytecode` builds this bytecode with the JavaScriptCore that ships with the app, also for the main script and the ASCII script chunks, and packs it in `app.bundle`. The runtime finds the bytecode of a chunk by where its source is in the mapped file, so page, preload, worker and imported chunks all use it, and JSC reads both in place, so they don't count in the footprint.

```sh
bash scripts/build-jsc.sh
```
