# Agents Guide
 
A tiny, high performance, memory-safe electron alternative with compatible API.

## Goals

- 120FPS, 0 FPS when idle, low CPU load
- Small runtime size (less than 10MB)
- Fast launch (less than 300ms)
- Low memory usage

## Architecture

- Electron compatible API
- Stanard Web API
- DOM and CSS renderer on GPU renderer with builtin tailwindCSS support
- JSC as runtime

## Security

By default script execute and dynamic importing are disabled:

- `new Function()`
- `eval()`
- `import("data:...")`
- Remote JS and HTTP imports
- Remote CSS

To enable the two features, add `jsExecute` and `httpImport` in `features` option in `package.json`

> The limtation is only applied to `BrowserWindow` class, `WebContentsView` respects the webview sandboxing.

## Node Integration

- Can import nodejs internal modules from `node:child_process`, `node:fs`, etc
- Global `process` object
- Support NAPI (`.node` module)
- Support FFI via `node:ffi`

> Unlike eletron bundles nodejs, `@pierre/apps` provides the node integration via rust code.

## Renderer

`crates/renderer` (`apps-renderer`) is apps' window system and GPU renderer. It started from [Zed's GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui), but keeps only what apps needs: an element tree built each frame over a retained taffy layout tree, text shaping, an atlas, and one renderer per platform.

Layout nodes are kept across frames. A node keeps its identity (element id, or position in the nearest element with an id) and is reused while its style, children and measure inputs stay equal, so taffy's caches skip unchanged subtrees. Measured leaves (text) pass their inputs as a `PartialEq` state to `request_measured_layout` and paint from the state it returns.

| Platform | Window | Renderer | Text |
|---|---|---|---|
| macOS | AppKit | Metal. Shaders are compiled at build time when the Metal toolchain is installed | CoreText |
| Windows | Win32 | Direct3D 11 with DirectComposition | DirectWrite |
| Linux | X11 and Wayland | wgpu on Vulkan or OpenGL | cosmic-text |

## JSC

The runtime uses [Bun's WebKit fork](https://github.com/oven-sh/WebKit).
The build enables Bun's JSC additions with the generic event loop.
`scripts/jsc.patch` restores C API locks and adds support for shared libraries.

```sh
bash scripts/build-jsc.sh
```

## Media

`crates/media` (`apps-media`) plays `<video>` and `<audio>` for the `video` and `audio` features. The runtime loads it like the WebGPU library, and it links the FFmpeg shared libraries that `scripts/build-ffmpeg.sh` builds into `target/ffmpeg`. FFmpeg must stay LGPL: no GPL or nonfree options, dynamic linking only, and `FFmpeg.LICENSE` ships next to it.

```sh
bash scripts/build-ffmpeg.sh
```

## FPS HUD

`crates/perfhud` (`apps-perfhud`) is the FPS HUD that `apps run` shows on Cmd+Alt+F. The runtime loads the library next to its executable, and `apps build` never ships it, so built apps keep only the small hook in `crates/runtime/src/render/hud.rs`.

## Compatibility

@COMPATIBILITY.md
