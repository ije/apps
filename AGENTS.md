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
