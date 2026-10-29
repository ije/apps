# Agents Guide
 
A tiny, high performance, memory-safe electron alternative with compatible API.

## Goals

- 120FPS, 0 FPS when idle, low CPU load
- Small runtime size (less than 15MB)
- Low memory usage
- Fast launch (less than 300ms)

## Architecture

- Electron compatible API
- Stanard Web API
- DOM and CSS renderer using GPUI (no chrome bundled)
- JSC as runtime

## Security

- Disable `new Funcion()` and `eval()`
- Reject remote JS and CSS
- Disable `loadURL`

> The limtation is only applied to `BrowserWindow` class, `WebContentsView` respects the webview sandboxing.

## Node Integration

- Can import nodejs internal modules like `node:child_process`, `node:fs`, etc
- Global `process` object
- Support NAPI (`.node` module)
- Support FFI via `node:ffi`

> Unlike eletron bundles nodejs, `@pierre/apps` provides the node integration via rust code.

## GPUI

The `crates/gpui*` crates are a fork of [Zed's GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) (main, October 2026), split into crates like upstream.

| Platform | Renderer |
|---|---|
| macOS | Metal. Shaders are compiled at build time when the Metal toolchain is installed |
| Windows | Direct3D 11 with DirectComposition |
| Linux | wgpu on Vulkan or OpenGL |

h2o adds offscreen effect layers, rounded clips, dotted and double borders, per-run text sizes, letter and word spacing, and text indent.
Parts h2o doesn't use are removed: HTTP images, screen capture, list elements, the web and iOS targets, and test support.

## JSC

The runtime uses [Bun's WebKit fork](https://github.com/oven-sh/WebKit).
The build enables Bun's JSC additions with the generic event loop.
`scripts/jsc.patch` restores C API locks and adds support for shared libraries.

```sh
bash scripts/build-jsc.sh
```

## Compatibility

@COMPATIBILITY.md
