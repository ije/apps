use crate::bundler;
use anyhow::{Context, Result};
use apps_runtime::{
    bundle::Bundle,
    host::Role,
    jsc::{JavaScriptCore, JsContext},
};
use bbtree::unsync::BBTreeMap;
use notify::Watcher;
use std::{
    cell::RefCell,
    collections::BTreeSet,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    process::{ChildStdin, Command, Stdio},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub fn run(
    entry: &Path,
    runtime: &Path,
    rust: Option<&Path>,
    watch: bool,
    devtools: bool,
    mcp: Option<u16>,
    args: &[String],
) -> Result<i32> {
    let root = rust
        .or_else(|| entry.parent())
        .context("Missing project directory")?;
    let runtime = runtime.canonicalize().context(
        "Cannot find the runtime. Build the workspace or set APPS_RUNTIME to an executable",
    )?;
    let library = apps_runtime::jsc::library_path(&runtime, std::env::consts::OS)?;
    let mut files = Rc::new(RefCell::new(BTreeSet::new()));
    let cache = watch.then(Rc::<bundler::Cache>::default);
    let started = Instant::now();
    let target =
        crate::build_target::BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH)?;
    let compile = |files: Rc<RefCell<BTreeSet<PathBuf>>>| {
        if rust.is_some() {
            files.borrow_mut().extend([
                entry.to_owned(),
                root.join("Cargo.toml"),
                root.join("Cargo.lock"),
            ]);
        }
        bundler::compile(
            entry,
            bundler::Realm::Main,
            None,
            files,
            false,
            cache.clone(),
        )
    };
    let build = || match rust {
        Some(root) => crate::rust::build(root, entry, false, target),
        None => Ok(runtime.clone()),
    };
    let main_chunks = |files: &BBTreeMap<String, String>| {
        let mut chunks = BTreeSet::new();
        let mut pending = vec!["app.js".to_owned()];
        while let Some(name) = pending.pop() {
            let source = files.get(&name).map_or("", String::as_str);
            for (at, _) in source.match_indices("script-") {
                let rest = &source[at + 7..];
                let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
                if digits > 0 && rest[digits..].starts_with(".js") {
                    let chunk = format!("script-{}.js", &rest[..digits]);
                    if chunks.insert(chunk.clone()) {
                        pending.push(chunk);
                    }
                }
            }
        }
        chunks
    };
    let mut binary = None;
    let mut bundle = match compile(files.clone()).and_then(|bundle| Ok((build()?, bundle))) {
        Ok((path, bundle)) => {
            println!(
                "apps: compiled {} bytes in {} ms",
                bundle.values().map(String::len).sum::<usize>(),
                started.elapsed().as_millis()
            );
            binary = Some(path);
            Some(bundle)
        }
        Err(error) if watch => {
            eprintln!("apps: {error:#}");
            println!("apps: waiting for changes");
            None
        }
        Err(error) => return Err(error),
    };
    let mut validator = None;
    let stop = Arc::new(AtomicBool::new(false));
    let signal = stop.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed))?;
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = if watch {
        Some(notify::recommended_watcher(move |event| {
            let _ = tx.send(event);
        })?)
    } else {
        None
    };
    let mut watched = BTreeSet::<PathBuf>::new();
    let mut child = None;
    let mut writer = None;
    let mut input: Option<ChildStdin> = None;
    let mut running: Option<BBTreeMap<String, String>> = None;
    let dev_bundle = rust
        .is_none()
        .then(|| std::env::temp_dir().join(format!("apps-bundle-{}.json", std::process::id())));
    let result = (|| -> Result<i32> {
        let mut fresh = true;
        let mut launch = bundle.is_some();
        let mut failed = bundle.is_none();
        let mut changed = None;
        loop {
            if stop.load(Ordering::Relaxed) {
                return Ok(130);
            }
            if fresh {
                fresh = false;
                if let Some(watcher) = &mut watcher {
                    let mut directories: BTreeSet<_> = files
                        .borrow()
                        .iter()
                        .filter_map(|path| path.parent())
                        .filter_map(|path| path.ancestors().find(|path| path.is_dir()))
                        .map(Path::to_owned)
                        .collect();
                    let src = root.join("src");
                    if rust.is_some() && src.is_dir() {
                        directories.insert(src.clone());
                    }
                    if directories != watched {
                        let mut paths = watcher.paths_mut();
                        for path in watched.difference(&directories) {
                            let _ = paths.remove(path);
                        }
                        for path in directories.difference(&watched) {
                            paths.add(
                                path,
                                if rust.is_some() && path == &src {
                                    notify::RecursiveMode::Recursive
                                } else {
                                    notify::RecursiveMode::NonRecursive
                                },
                            )?;
                        }
                        paths.commit()?;
                        watched = directories;
                    }
                }
            }
            if launch {
                launch = false;
                let files = bundle.take().unwrap();
                let mut cmd = Command::new(binary.as_ref().unwrap());
                if rust.is_some() {
                    cmd.env("APPS_RUST_DEV", "1")
                        .env("APPS_RUST_LIBRARIES", &runtime)
                        .env_remove("APPS_RUST_DEVTOOLS")
                        .env_remove("APPS_RUST_MCP_PORT");
                    if devtools {
                        cmd.env("APPS_RUST_DEVTOOLS", "1");
                    }
                    if let Some(port) = mcp {
                        cmd.env("APPS_RUST_MCP_PORT", port.to_string());
                    }
                } else {
                    cmd.arg("--bundle-stdin")
                        .args(devtools.then_some("--devtools"))
                        .args(mcp.map(|port| format!("--devtools-mcp={port}")))
                        .arg("--")
                        .arg(entry);
                }
                // Set MallocSpaceEfficient before launch to avoid the runtime's 10 ms restart.
                #[cfg(target_os = "macos")]
                cmd.env("MallocSpaceEfficient", "1");
                if let Some(path) = &dev_bundle {
                    cmd.env("APPS_DEV_BUNDLE", path);
                }
                child = Some(
                    cmd.args(args)
                        .env("APPS_JSC_LIBRARY", &library)
                        .current_dir(root)
                        .stdin(Stdio::piped())
                        .spawn()?,
                );
                let stdin = child.as_mut().unwrap().stdin.take().unwrap();
                let dev_bundle = dev_bundle.clone();
                running = watch.then(|| files.clone());
                writer = Some(
                    std::thread::Builder::new()
                        .name("apps-bundle".into())
                        .spawn(move || -> Result<ChildStdin> {
                            let bundle = Bundle {
                                files,
                                ..Default::default()
                            };
                            // Write the bundle before launch: ELECTRON_RUN_AS_NODE reads script chunks from
                            // it.
                            if let Some(path) = &dev_bundle
                                && bundle.files.keys().any(|name| name.starts_with("script-"))
                            {
                                serde_json::to_writer(
                                    BufWriter::new(std::fs::File::create(path)?),
                                    &bundle,
                                )?;
                            }
                            let mut stdin = BufWriter::new(stdin);
                            serde_json::to_writer(&mut stdin, &bundle)?;
                            stdin.write_all(b"\n")?;
                            Ok(stdin.into_inner()?)
                        })?,
                );
                if watch {
                    println!("apps: watching {} and its imports", entry.display());
                }
            }
            if writer
                .as_ref()
                .is_some_and(std::thread::JoinHandle::is_finished)
            {
                input = watch.then_some(
                    writer
                        .take()
                        .unwrap()
                        .join()
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("Bundle writer panicked")))
                        .context("Cannot send the app bundle to the runtime")?,
                );
            }
            if let Some(process) = &mut child
                && let Some(status) = process.try_wait()?
            {
                child = None;
                if !watch {
                    return Ok(status.code().unwrap_or(1));
                }
                println!("apps: App exited with {status}; waiting for changes");
            }
            if !watch {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Ok(event)) if !matches!(event.kind, notify::EventKind::Access(_)) => {
                    if let Some(cache) = &cache
                        && (!matches!(
                            event.kind,
                            notify::EventKind::Modify(
                                notify::event::ModifyKind::Data(_)
                                    | notify::event::ModifyKind::Metadata(_)
                            )
                        ) || event.paths.iter().any(|path| {
                            path.ends_with("package.json") || path.ends_with("tsconfig.json")
                        }))
                    {
                        cache.resolutions.borrow_mut().clear();
                    }
                    for path in &event.paths {
                        if !path.exists() {
                            watched.retain(|directory| !directory.starts_with(path));
                        }
                    }
                    let files = files.borrow();
                    let cargo = |path: &PathBuf| {
                        rust.is_some()
                            && (path.extension().is_some_and(|e| e == "rs")
                                || path.ends_with("Cargo.toml")
                                || path.ends_with("Cargo.lock"))
                    };
                    if event.paths.iter().any(|path| {
                        cargo(path)
                            || files
                                .range(path.clone()..)
                                .next()
                                .is_some_and(|file| file.starts_with(path))
                            || failed
                    }) {
                        changed = Some(Instant::now());
                        fresh = true;
                    }
                    if event.paths.iter().any(cargo) {
                        binary = None;
                    }
                }
                Ok(Err(error)) => eprintln!("apps: watch error: {error}"),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(0),
                _ => {}
            }
            if changed.is_some_and(|time| time.elapsed() >= Duration::from_millis(150)) {
                changed = None;
                fresh = true;
                let next_files = Rc::default();
                let rebuild = Instant::now();
                let rebuilt = compile(Rc::clone(&next_files)).and_then(|bundle| {
                    if rust.is_none() {
                        if validator.is_none() {
                            validator = Some(JsContext::new(
                                JavaScriptCore::load()?,
                                Rc::default(),
                                Role::Main,
                            )?);
                        }
                        validator.as_ref().unwrap().validate_bundle(&bundle)?;
                    }
                    Ok(bundle)
                });
                match rebuilt {
                    Ok(next) => {
                        files = next_files;
                        failed = false;
                        // Main keeps its code. Only page-exclusive code can change without a restart.
                        let pages = child.is_some()
                            && binary.is_some()
                            && running.as_ref().is_some_and(|running| {
                                let chunks: BTreeSet<_> = main_chunks(running)
                                    .into_iter()
                                    .chain(main_chunks(&next))
                                    .collect();
                                let main = |(name, _): &(&String, &String)| {
                                    !name.ends_with(".html")
                                        && (!name.starts_with("script-") || chunks.contains(*name))
                                };
                                running.iter().filter(main).eq(next.iter().filter(main))
                            });
                        if pages
                            && running
                                .as_ref()
                                .is_some_and(|running| running.iter().eq(next.iter()))
                        {
                            continue;
                        }
                        let next = Bundle {
                            files: next,
                            ..Default::default()
                        };
                        if pages {
                            if let Some(writer) = writer.take() {
                                input = writer.join().ok().and_then(Result::ok);
                            }
                            let sent = input.as_mut().is_some_and(|input| {
                                let mut input = BufWriter::new(input);
                                serde_json::to_writer(&mut input, &next).is_ok()
                                    && input.write_all(b"\n").and_then(|()| input.flush()).is_ok()
                            });
                            if sent {
                                running = Some(next.files);
                                println!(
                                    "apps: reloaded pages in {} ms",
                                    rebuild.elapsed().as_millis()
                                );
                                continue;
                            }
                        }
                        match binary.take().map_or_else(build, Ok) {
                            Ok(path) => {
                                binary = Some(path);
                                if let Some(mut process) = child.take() {
                                    let _ = process.kill();
                                    let _ = process.wait();
                                }
                                if let Some(writer) = writer.take() {
                                    let _ = writer.join();
                                }
                                bundle = Some(next.files);
                                launch = true;
                                println!("apps: restarted in {} ms", rebuild.elapsed().as_millis());
                            }
                            Err(error) => {
                                failed = true;
                                eprintln!("apps: {error:#}");
                            }
                        }
                    }
                    Err(error) => {
                        files
                            .borrow_mut()
                            .extend(std::mem::take(&mut *next_files.borrow_mut()));
                        failed = true;
                        eprintln!("apps: {error:#}");
                    }
                }
            }
        }
    })();
    if let Some(path) = &dev_bundle {
        let _ = std::fs::remove_file(path);
    }
    if let Some(mut process) = child {
        let _ = process.kill();
        let _ = process.wait();
    }
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    result
}
