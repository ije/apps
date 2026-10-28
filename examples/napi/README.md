# Node-API example

This example imports a C addon from a local `.node` file.
It needs macOS or Linux, a C compiler, and Node.js headers.

From the repository root, build the addon and the runtime:

```sh
npm run build --prefix examples/napi
cargo build --workspace
```

The addon compiler uses the `include/node` directory beside the Node.js executable directory.
Set `NODE_INCLUDE_DIR` if your Node.js headers use a different directory.

Run the example:

```sh
target/debug/apps run examples/napi/main.js
```

Expected output:

```text
Native add: 42
Default export: 3
```

Build the app:

```sh
target/debug/apps build examples/napi/main.js
examples/napi/dist/apps_napi_example --check
```

The build copies the addon to `node_modules/apps_napi_example/build/Release/addon.node` in the app resources.
On macOS, the resources are in `dist/apps_napi_example.app/Contents/Resources`.
On Linux, the resources are in `dist`.
The compiled app needs neither Node.js nor a C compiler.
