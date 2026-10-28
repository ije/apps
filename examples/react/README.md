# React

A native counter with React state, click handlers, and conditional elements.
apps compiles `page.tsx` and its npm dependencies directly.

From the repository root:

```sh
cargo build --workspace --release
export PATH="$PWD/target/release:$PATH"
npm ci --prefix examples/react
apps run --watch examples/react/main.ts
```

Save an imported `.jsx` or `.tsx` file to restart the app.
The compiler imports React's `createElement` and `Fragment` APIs for JSX.
The native app needs no Node.js or npm installation.

To build the app or run the counter interaction test:

```sh
apps build examples/react/main.ts
npm test --prefix examples/react
```

The example demonstrates text, buttons, state updates, and conditional elements.
Other React DOM features depend on apps's DOM support.
