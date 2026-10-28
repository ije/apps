mod build;
mod bundle;
mod run;
mod target;

use anyhow::{Context, Result, bail, ensure};
use std::path::PathBuf;
use target::Target;

fn main() {
    // WARNING: Bundling recurses once per import depth. The main thread stack is too small for deep graphs.
    let result = std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(run)
        .map_err(anyhow::Error::from)
        .and_then(|thread| {
            thread
                .join()
                .unwrap_or_else(|_| Err(anyhow::anyhow!("Internal error")))
        });
    std::process::exit(match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("apps: {error:#}");
            1
        }
    });
}

fn run() -> Result<i32> {
    let mut args = std::env::args().skip(1).peekable();
    let command = args.next().unwrap_or_else(|| "--help".into());
    if matches!(command.as_str(), "--help" | "-h")
        || matches!(command.as_str(), "run" | "build")
            && args
                .peek()
                .is_some_and(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        println!(
            "apps {}\n\napps run [entry] [--watch|-W] [--devtools|-D]\napps build [entry] [--out-dir <directory>] [--platform <macos|linux|windows>] [--arch <x64|arm64>]\n\nRun and build default to main.ts, then main.js. Watching is off by default.\n--watch, -W       Restart when the entry or its imports change.\n--devtools, -D    Open Developer Tools in a standalone window.\n\nPlatform and architecture default to this machine. Explicit targets use dist/<platform>-<arch>.\nSet APPS_RUNTIME to a matching runtime executable.\nBun JavaScriptCore is bundled with every app. Set APPS_JSC_LIBRARY to a matching shared library.",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(0);
    }
    if command == "--version" {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(0);
    }
    ensure!(
        matches!(command.as_str(), "run" | "build"),
        "Unknown command: {command}"
    );
    let mut entry = None;
    let mut out = None;
    let host = Target::new(std::env::consts::OS, std::env::consts::ARCH)?;
    let mut target = host;
    let mut explicit_target = false;
    let mut watch = false;
    let mut devtools = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--watch" | "-W" if command == "run" => watch = true,
            "--devtools" | "-D" if command == "run" => devtools = true,
            "--out-dir" if command == "build" => {
                out = Some(std::path::absolute(
                    args.next().context("Expected output directory")?,
                )?);
            }
            "--platform" if command == "build" => {
                target = Target::new(&args.next().context("Expected platform")?, target.arch)?;
                explicit_target = true;
            }
            "--arch" if command == "build" => {
                target = Target::new(
                    target.platform,
                    &args.next().context("Expected architecture")?,
                )?;
                explicit_target = true;
            }
            _ if !arg.starts_with('-') => {
                ensure!(entry.is_none(), "Unexpected argument: {arg}");
                entry = Some(PathBuf::from(arg));
            }
            _ => bail!("Unknown option: {arg}"),
        }
    }
    if entry.is_none() {
        entry = ["main.ts", "main.js"]
            .into_iter()
            .map(PathBuf::from)
            .find(|path| path.is_file());
    }
    let entry = entry
        .context("No entry found. Provide an entry file or create main.ts or main.js")?
        .canonicalize()?;
    let root = entry.parent().context("Missing project directory")?;
    let runtime = if let Some(path) = std::env::var_os("APPS_RUNTIME") {
        PathBuf::from(path)
    } else {
        ensure!(
            target == host,
            "Set APPS_RUNTIME to an apps-runtime executable built for {}-{}",
            target.platform,
            target.arch
        );
        std::env::current_exe()?.with_file_name(if target.platform == "windows" {
            "apps-runtime.exe"
        } else {
            "apps-runtime"
        })
    };
    match command.as_str() {
        "run" => run::run(&entry, &runtime, watch, devtools),
        "build" => {
            let out = out.unwrap_or_else(|| {
                let out = root.join("dist");
                if explicit_target {
                    out.join(format!("{}-{}", target.platform, target.arch))
                } else {
                    out
                }
            });
            ensure!(
                out != root && !entry.starts_with(&out),
                "Output must be separate from source files"
            );
            build::run(&entry, &out, &runtime, target).map(|_| 0)
        }
        _ => unreachable!(),
    }
}
