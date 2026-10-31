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
        // A built app passes all of its arguments to process.argv. Otherwise the arguments after `--` go there.
        let packaged = home.join("app.h2o").is_file() || home.join("app.js").is_file();
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
        let bundle = if bundle_stdin {
            anyhow::ensure!(
                directory.is_none(),
                "Cannot use a directory with --bundle-stdin"
            );
            let bundle: apps_runtime::bundle::Bundle =
                serde_json::from_reader(std::io::BufReader::new(std::io::stdin().lock()))
                    .context("Cannot read the app bundle from stdin")?;
            anyhow::ensure!(bundle.files.contains_key("app.js"), "Missing app.js");
            bundle
        } else {
            let directory = directory.unwrap_or(home);
            let mut bundle = apps_runtime::bundle::read(&directory)?;
            bundle.packaged |= packaged;
            bundle
        };
        let app = apps_runtime::desktop::Desktop::from_bundle(bundle)?;
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
