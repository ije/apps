use crate::{build_target::BuildTarget, bundler, packaging};
use anyhow::{Context, Result, ensure};
use std::{
    path::{Path, PathBuf},
    rc::Rc,
    time::Instant,
};

pub fn run(entry: &Path, out: &Path, runtime: &Path, target: BuildTarget) -> Result<()> {
    let started = Instant::now();
    target.validate_binary(runtime).context(
        "Invalid runtime. Build the workspace or set APPS_RUNTIME to a matching executable",
    )?;
    let files = bundler::compile(entry, bundler::Realm::Main, None, Rc::default(), true, None)?;
    if let Some(source) = files.get("native.json") {
        let addons: std::collections::BTreeMap<String, PathBuf> = serde_json::from_str(source)?;
        for path in addons.values() {
            target
                .validate_binary(path)
                .context("Native addon does not match the build target")?;
        }
    }
    let app = (
        "app.js".to_owned(),
        apps_runtime::bundle::script_source(&files, false),
    );
    for (name, source) in files
        .iter()
        .filter(|(name, _)| name.starts_with("script-"))
        .chain([(&app.0, &app.1)])
    {
        let allocator = oxc_allocator::Allocator::default();
        let parsed =
            oxc_parser::Parser::new(&allocator, source, oxc_span::SourceType::cjs()).parse();
        ensure!(
            parsed.diagnostics.is_empty(),
            "Invalid app bundle {name}: {:?}",
            parsed.diagnostics
        );
    }
    let root = entry.parent().context("Missing project directory")?;
    let (bytes, _) = packaging::package_app(out, &files, runtime, root, true, target)?;
    println!(
        "apps: built {} bytes in {} ms → {}",
        bytes,
        started.elapsed().as_millis(),
        out.display()
    );
    Ok(())
}
