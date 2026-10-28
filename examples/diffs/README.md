# Diffs

A native diff viewer and editor with [`@pierre/diffs`](https://diffs.com) and [`@pierre/highlights`](https://diffs.com/highlights).
Highlights supplies TypeScript syntax colors through WebAssembly.
The example supports split and unified views, light and dark themes, and line selection.

Select **Edit** in the custom diff header to change the new file.
Select **Review** to return to the diff view with your changes.
Select **Reset** to restore the sample and clear the undo history.

The editor loads on the first edit.
Changes stay in memory until the window closes.

From the repository root:

```sh
cargo build --workspace --release
export PATH="$PWD/target/release:$PATH"
npm ci --prefix examples/diffs
apps run --watch examples/diffs/main.ts
```

To build the app:

```sh
apps build examples/diffs/main.ts
```

Edit `oldFile` and `newFile` in `page.ts` to compare different source text.
The example preloads both themes and sets `preferredHighlighter: 'highlights'` on `FileDiff`.
The native renderer uses the library’s original CSS.
