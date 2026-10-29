# Compatibility

## DOM

Pages get the standard DOM API (nodes, elements, events, ranges, selection, observers, shadow DOM and custom elements). These parts differ from browsers:

| Feature | Support |
|---|---|
| `NodeList`, `HTMLCollection` | Static arrays with `item()`. They don't update when the tree changes |
| `DocumentType`, `CDATASection`, `ProcessingInstruction` | Not supported. `Attr` is not a `Node` |
| `DOMParser`, XPath, XSLT | `DOMParser` parses `text/html` only. XPath and XSLT are not supported |
| `*AttributeNS()` | The namespace is ignored |
| `<template>` | Parsed children move into `content` when `content` is first read |
| Inline event attributes (`onclick="..."`) | Not run, because `eval` is disabled. Use `addEventListener` or `el.onclick = fn` |
| Forms | No `submit` event, `<select>` popup or `<button>` default action. Clicks on checkbox and radio inputs work |

## CSS

Pages use standard CSS, drawn with GPUI. Transforms, filters, blend modes, masks and clip paths paint through offscreen GPU layers.
These parts differ from browsers. The ones that don't work show an error in the Developer Tools console (`apps run -D`).

| Feature | Support |
|---|---|
| `transform-style: preserve-3d` | 3D children are flattened into the parent's plane |
| `filter: url()` | Only `<filter>` elements in the page, with `feGaussianBlur`, `feColorMatrix`, `feOffset` and `feDropShadow` |
| `clip-path: shape()` | Not supported |
| `line-height`, `vertical-align` in `::first-letter` and `::first-line` | Ignored. Sizes, colors and font styles work |
| `float` | Text next to a float doesn't move under it when the float ends |
| Multi-column layout | Columns break between child blocks. One paragraph doesn't split across columns |
| `writing-mode: vertical-lr` | Lines are ordered right to left, like `vertical-rl` |

## Electron API

These parts differ from Electron:

| Feature | Support |
|---|---|
| Platforms | `activate`, `open-url`, `open-file`, `second-instance` and the native calls (about panel, login items, single instance lock) work on macOS only. On Windows and Linux the native calls throw an error |
| `open-url` | The URL scheme must be in the app's `Info.plist`. `apps build` doesn't add it, and `setAsDefaultProtocolClient()` is not supported |
| `relaunch()` | Works in built apps. `apps run` ignores it |
| `setPath()`, `setName()` | Change only the values that `app` returns. Web storage keeps its own location |
| `commandLine` | Switches can be added and read, but they don't change h2o. `disableHardwareAcceleration()` does nothing, because GPUI always draws on the GPU |
| Login items | Need macOS 13 and a built app. Only `openAtLogin` and `status` are returned |
| Images | `dock.setIcon()` and `iconPath` take a file path. There is no `nativeImage` |
| Not supported | Recent documents, jump lists, user activities, `setAppLogsPath()`, `getFileIcon()`, `getAppMetrics()`, GPU info, proxy, certificates and accessibility APIs |

## Node Integration
