# @pierre/apps

WIP

Rust apps use the `pierre-apps` crate:

```rust
use pierre_apps::{BrowserWindow, WindowOptions, app};

fn main() -> pierre_apps::Result<()> {
    app::run(|app| {
        BrowserWindow::new(app, WindowOptions::default())?.load_file("index.html")
    })
}
```
