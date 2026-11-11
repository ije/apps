use anyhow::Context;

fn main() {
    let result = (|| -> anyhow::Result<i32> {
        let mut check = false;
        let mut devtools = false;
        let mut mcp = None;
        let mut directory = None;
        let mut bundle_stdin = false;
        let mut home = std::env::current_exe()?
            .canonicalize()?
            .parent()
            .unwrap()
            .to_owned();
        if home.ends_with("Contents/MacOS") {
            home = home.parent().unwrap().join("Resources");
        }
        let packaged = home.join("app.bundle").is_file() || home.join("app.js").is_file();
        if std::env::var_os("ELECTRON_RUN_AS_NODE")
            .is_some_and(|value| !value.is_empty() && value != "0")
        {
            let mut args = std::env::args().peekable();
            let mut argv = vec![args.next().unwrap_or_default()];
            while let Some(arg) = args.next_if(|arg| arg.starts_with('-')) {
                if arg == "--require" || arg == "-r" {
                    args.next();
                }
            }
            argv.extend(args);
            // A built app runs only its own scripts. Otherwise anyone could run code as the app with a variable.
            let bundle = match std::env::var_os("APPS_DEV_BUNDLE").filter(|_| !packaged) {
                Some(path) => apps_runtime::bundle::from_json(
                    std::fs::File::open(path).context("Cannot read the app bundle")?,
                )
                .context("Cannot read the app bundle")?,
                None => apps_runtime::bundle::read(&home)?,
            };
            return apps_runtime::node::script::run(bundle, argv);
        }
        for os in std::env::args_os().skip(1).filter(|_| !packaged) {
            let arg = os.to_string_lossy();
            if arg == "--" {
                break;
            } else if arg == "--check" {
                check = true;
            } else if arg == "--devtools" {
                devtools = true;
            } else if let Some(port) = arg.strip_prefix("--devtools-mcp=") {
                mcp = Some(port.parse().context("Invalid devtools MCP port")?);
            } else if arg == "--bundle-stdin" {
                bundle_stdin = true;
            } else {
                anyhow::ensure!(
                    directory.is_none() && !arg.starts_with('-'),
                    "Usage: apps-runtime [directory | --bundle-stdin] [--check] [--devtools] [--devtools-mcp=<port>] [-- <arguments>]"
                );
                directory = Some(std::path::PathBuf::from(&os));
            }
        }
        let (bundle, input) = if bundle_stdin {
            anyhow::ensure!(
                directory.is_none(),
                "Cannot use a directory with --bundle-stdin"
            );
            let (bundle, input) = apps_runtime::bundle::from_stdin()
                .context("Cannot read the app bundle from stdin")?;
            anyhow::ensure!(bundle.files.contains_key("app.js"), "Missing app.js");
            (bundle, Some(input))
        } else {
            let directory = directory.unwrap_or(home);
            let mut bundle = apps_runtime::bundle::read(&directory)?;
            bundle.packaged |= packaged;
            (bundle, None)
        };
        let app = apps_runtime::desktop::Desktop::from_bundle(bundle)?;
        if let Some(input) = input {
            app.reload_from(input);
        }
        if check {
            app.check()
        } else {
            app.run(devtools, mcp)
        }
    })();
    match result {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("apps: {error:#}");
            std::process::exit(1);
        }
    }
}
