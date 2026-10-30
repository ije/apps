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

- Disable `new Funcion()` and `eval()`
- Reject remote JS and CSS
- Disable `loadURL`

> The limtation is only applied to `BrowserWindow` class, `WebContentsView` respects the webview sandboxing.

## Node Integration

- Can import nodejs internal modules from `node:child_process`, `node:fs`, etc
- Global `process` object
- Support NAPI (`.node` module)
- Support FFI via `node:ffi`

> Unlike eletron bundles nodejs, `@pierre/apps` provides the node integration via rust code.

## Renderer

`crates/render` (`apps-render`) is h2o's window system and GPU renderer. It started from [Zed's GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui), but keeps only what h2o needs: an element tree built each frame, taffy layout, text shaping, an atlas, and one renderer per platform.

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

## Compatibility

@COMPATIBILITY.md
