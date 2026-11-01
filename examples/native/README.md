# Native

A window that calls C code in two ways:

| Way | File | Loaded by |
|---|---|---|
| Node-API addon | `addon.c` | `import addon from './build/Release/addon.node'` |
| FFI library | `math.c` | `dlopen()` from `node:ffi`, with a JavaScript callback |

`package.json` sets `preload` to `./preload.js`. This module loads both libraries and exports `napiAdd`, `ffiAdd`, and `ffiDouble`.
`page.js` imports these functions from `./preload.js`. Calls run in the isolated preload context.
Page scripts cannot load native code themselves.

It needs macOS or Linux, a C compiler, and Node.js headers.
The build script uses the `include/node` directory beside the Node.js executable directory.
Set `NODE_INCLUDE_DIR` if your Node.js headers use a different directory.

From the repository root:

```sh
npm run build:native --prefix examples/native
cargo build --workspace --release
target/release/apps run examples/native/main.js
```

Build the app:

```sh
target/release/apps build examples/native/main.js
```

`apps build` packs the addon into `app.h2o`, and copies the FFI library with `files` in `package.json`.
