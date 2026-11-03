mod build_target;
mod bundler;
mod commands;
#[path = "../../../scripts/napi_exports.rs"]
#[allow(dead_code)]
mod napi_exports;
mod packaging;
mod rust;

use anyhow::{Context, Result, bail, ensure};
use build_target::BuildTarget;
use std::path::PathBuf;

fn main() {
    // WARNING: Bundling recurses once per import depth. The main thread stack is too small for deep graphs.
    let result = std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(run_cli)
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

fn run_cli() -> Result<i32> {
    let mut args = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .peekable();
    let command = args.next().unwrap_or_else(|| "--help".into());
    if matches!(command.as_str(), "--help" | "-h")
        || matches!(command.as_str(), "run" | "build")
            && args
                .peek()
                .is_some_and(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        println!(
            "apps {}\n\napps run [entry] [--watch|-W] [--devtools|-D] [--enable-devtools-mcp] [--devtools-mcp-port <port>] [--] [app arguments]\napps build [entry] [--out-dir <directory>] [--platform <macos|linux|windows>] [--arch <x64|arm64>]\napps skill\n\nRun and build default to main in package.json, then index.ts, index.mts, index.mjs or index.js. Watching is off by default. Apps with type rust use Cargo for their main executable.\nOther arguments after the entry, and all arguments after --, go to the app in process.argv.\n--watch, -W       Restart when the entry or its imports change.\n--devtools, -D    Open Developer Tools in a standalone window.\n--enable-devtools-mcp       Serve devtools to agents at http://127.0.0.1:<port>, over plain HTTP and MCP (/mcp).\n--devtools-mcp-port <port>  Port of the devtools server. Default 19876, 0 picks a free port.\nCmd+Alt+F         Show or hide the FPS HUD of a window. Ctrl+Alt+F on Windows and Linux.\napps skill        Print the agent skill that explains the devtools server.\n\nPlatform and architecture default to this machine. Explicit targets use dist/<platform>-<arch>.\nSet APPS_RUNTIME to a matching runtime executable.\nBun JavaScriptCore is bundled with every app. Set APPS_JSC_LIBRARY to a matching shared library.",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(0);
    }
    if command == "skill" {
        print!("{}", include_str!("../../../skills/apps-devtools/SKILL.md"));
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
    let host = BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH)?;
    let mut target = host;
    let mut explicit_target = false;
    let mut watch = false;
    let mut devtools = false;
    let mut mcp = false;
    let mut mcp_port = 19876;
    let mut app_args = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--" if command == "run" => {
                app_args.extend(args.by_ref());
                break;
            }
            "--watch" | "-W" if command == "run" => watch = true,
            "--devtools" | "-D" if command == "run" => devtools = true,
            "--enable-devtools-mcp" if command == "run" => mcp = true,
            "--devtools-mcp-port" if command == "run" => {
                mcp_port = args
                    .next()
                    .context("Expected devtools MCP port")?
                    .parse()
                    .context("Invalid devtools MCP port")?;
            }
            "--out-dir" if command == "build" => {
                out = Some(std::path::absolute(
                    args.next().context("Expected output directory")?,
                )?);
            }
            "--platform" if command == "build" => {
                target = BuildTarget::new(&args.next().context("Expected platform")?, target.arch)?;
                explicit_target = true;
            }
            "--arch" if command == "build" => {
                target = BuildTarget::new(
                    target.platform,
                    &args.next().context("Expected architecture")?,
                )?;
                explicit_target = true;
            }
            _ if command == "run" && entry.is_some() => app_args.push(arg),
            _ if !arg.starts_with('-') => {
                ensure!(entry.is_none(), "Unexpected argument: {arg}");
                entry = Some(PathBuf::from(arg));
            }
            _ => bail!("Unknown option: {arg}"),
        }
    }
    let entry = match entry {
        Some(entry) => entry,
        None => {
            let main = match std::fs::read_to_string("package.json") {
                Ok(source) => serde_json::from_str::<serde_json::Value>(&source)
                    .context("Invalid package.json")?["main"]
                    .as_str()
                    .map(PathBuf::from),
                Err(_) => None,
            };
            main.or_else(|| {
                ["index.ts", "index.mts", "index.mjs", "index.js"]
                    .into_iter()
                    .map(PathBuf::from)
                    .find(|path| path.is_file())
            })
            .context("No entry found. Provide an entry file, set main in package.json, or create index.ts, index.mts, index.mjs or index.js")?
        }
    };
    let entry = entry
        .canonicalize()
        .with_context(|| format!("Cannot find the entry {}", entry.display()))?;
    let rust = rust::project(&entry)?;
    let root = rust
        .as_deref()
        .or_else(|| entry.parent())
        .context("Missing project directory")?;
    let runtime = if let Some(path) = std::env::var_os("APPS_RUNTIME") {
        PathBuf::from(path)
    } else if rust.is_some() && target == host {
        std::env::current_exe()?
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
        "run" => {
            // Port 0 picks a free port once, so `--watch` restarts keep it.
            let mcp = match (mcp, mcp_port) {
                (false, _) => None,
                (true, 0) => Some(
                    std::net::TcpListener::bind(("127.0.0.1", 0))?
                        .local_addr()?
                        .port(),
                ),
                (true, port) => Some(port),
            };
            commands::run::run(
                &entry,
                &runtime,
                rust.as_deref(),
                watch,
                devtools,
                mcp,
                &app_args,
            )
        }
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
            commands::build::run(&entry, &out, &runtime, rust.as_deref(), target).map(|_| 0)
        }
        _ => unreachable!(),
    }
}
