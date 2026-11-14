use pierre_apps::{BrowserWindow, WindowOptions, app};

fn main() -> pierre_apps::Result<()> {
    app::run(|app| {
        app.handle("greet", |_window, args| {
            let name = args.first().and_then(|arg| arg.as_str()).unwrap_or("world");
            Ok(format!("Hello, {name}!").into())
        })?;
        BrowserWindow::new(app, WindowOptions::default())?.load_file("index.html")
    })
}
