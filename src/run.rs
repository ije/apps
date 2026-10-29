use crate::bundler;
use anyhow::{Context, Result};
use apps_runtime::{
    host::Role,
    jsc::{Api, Engine},
};
use notify::Watcher;
use std::{
    cell::RefCell,
    collections::BTreeSet,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub fn run(entry: &Path, runtime: &Path, watch: bool, devtools: bool) -> Result<i32> {
    let root = entry.parent().context("Missing project directory")?;
    let public = root.join("public");
    let runtime = runtime.canonicalize().context(
        "Cannot find the runtime. Build the workspace or set APPS_RUNTIME to an executable",
    )?;
    let library = apps_runtime::jsc::library_path(&runtime, std::env::consts::OS)?;
    let mut files = Rc::new(RefCell::new(BTreeSet::new()));
    let started = Instant::now();
    let bundle = bundler::compile(entry, bundler::Realm::Main, None, files.clone(), false)?;
    let validator = Engine::new(Api::load()?, Rc::default(), Role::Main)?;
    validator.validate(&bundle)?;
    let validator = watch.then_some(validator);
    println!(
        "apps: compiled {} bytes in {} ms",
        bundle.values().map(String::len).sum::<usize>(),
        started.elapsed().as_millis()
    );
    let mut bundle = Some(bundle);
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
    let result = (|| -> Result<i32> {
        let mut fresh = true;
        let mut launch = true;
        let mut failed = false;
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
                        .filter(|path| !path.starts_with(&public))
                        .map(Path::to_owned)
                        .collect();
                    if public.is_dir() {
                        directories.insert(public.clone());
                    }
                    if directories != watched {
                        let mut paths = watcher.paths_mut();
                        for path in watched.difference(&directories) {
                            let _ = paths.remove(path);
                        }
                        for path in directories.difference(&watched) {
                            paths.add(
                                path,
                                if path == &public {
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
                child = Some(
                    Command::new(&runtime)
                        .arg("--bundle-stdin")
                        .args(devtools.then_some("--devtools"))
                        .env("APPS_JSC_LIBRARY", &library)
                        .current_dir(root)
                        .stdin(Stdio::piped())
                        .spawn()?,
                );
                let input = child.as_mut().unwrap().stdin.take().unwrap();
                let files = bundle.take().unwrap();
                writer = Some(
                    std::thread::Builder::new()
                        .name("apps-bundle".into())
                        .spawn(move || -> Result<()> {
                            let bundle = apps_runtime::bundle::Bundle {
                                files,
                                ..Default::default()
                            };
                            let mut input = BufWriter::new(input);
                            serde_json::to_writer(&mut input, &bundle)?;
                            input.flush()?;
                            Ok(())
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
                writer
                    .take()
                    .unwrap()
                    .join()
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("Bundle writer panicked")))
                    .context("Cannot send the app bundle to the runtime")?;
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
                    for path in &event.paths {
                        if !path.exists() {
                            watched.retain(|directory| !directory.starts_with(path));
                        }
                    }
                    let files = files.borrow();
                    if event.paths.iter().any(|path| {
                        path.starts_with(&public)
                            || files
                                .range(path.clone()..)
                                .next()
                                .is_some_and(|file| file.starts_with(path))
                            || failed
                    }) {
                        changed = Some(Instant::now());
                        fresh = true;
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
                let rebuilt = bundler::compile(
                    entry,
                    bundler::Realm::Main,
                    None,
                    Rc::clone(&next_files),
                    false,
                )
                .and_then(|bundle| {
                    validator.as_ref().unwrap().validate(&bundle)?;
                    Ok(bundle)
                });
                match rebuilt {
                    Ok(next_bundle) => {
                        files = next_files;
                        if let Some(mut process) = child.take() {
                            let _ = process.kill();
                            let _ = process.wait();
                        }
                        if let Some(writer) = writer.take() {
                            let _ = writer.join();
                        }
                        bundle = Some(next_bundle);
                        failed = false;
                        launch = true;
                        println!("apps: restarted");
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
    if let Some(mut process) = child {
        let _ = process.kill();
        let _ = process.wait();
    }
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    result
}
