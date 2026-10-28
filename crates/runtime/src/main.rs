use anyhow::Context;

fn main() {
    let result = (|| -> anyhow::Result<i32> {
        let mut check = false;
        let mut devtools = false;
        let mut directory = None;
        let mut bundle_stdin = false;
        for arg in std::env::args().skip(1) {
            if arg == "--check" {
                check = true;
            } else if arg == "--devtools" {
                devtools = true;
            } else if arg == "--bundle-stdin" {
                bundle_stdin = true;
            } else {
                anyhow::ensure!(
                    directory.is_none() && !arg.starts_with('-'),
                    "Usage: apps-runtime [directory | --bundle-stdin] [--check] [--devtools]"
                );
                directory = Some(std::path::PathBuf::from(arg));
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
            let directory = directory.unwrap_or(
                std::env::current_exe()?
                    .canonicalize()?
                    .parent()
                    .unwrap()
                    .to_owned(),
            );
            let directory = if directory.ends_with("Contents/MacOS") {
                directory.parent().unwrap().join("Resources")
            } else {
                directory
            };
            apps_runtime::bundle::read(&directory)?
        };
        let app = apps_runtime::desktop::Desktop::from_bundle(bundle)?;
        if check {
            app.check()
        } else {
            app.run(devtools)
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
