use crate::{build_target::BuildTarget, bundler, packaging};
use anyhow::{Context, Result, ensure};
use std::{
    path::{Path, PathBuf},
    rc::Rc,
    time::Instant,
};

pub fn run(
    entry: &Path,
    out: &Path,
    runtime: &Path,
    rust: Option<&Path>,
    target: BuildTarget,
    bytecode: bool,
    aot: bool,
) -> Result<()> {
    let started = Instant::now();
    if rust.is_none() {
        target.validate_binary(runtime).context(
            "Invalid runtime. Build the workspace or set APPS_RUNTIME to a matching executable",
        )?;
    }
    let mut files = bundler::compile(entry, bundler::Realm::Main, None, Rc::default(), true, None)?;
    if let Some(source) = files.get("native.json") {
        let addons: std::collections::BTreeMap<String, PathBuf> = serde_json::from_str(source)?;
        for path in addons.values() {
            target
                .validate_binary(path)
                .context("Native addon does not match the build target")?;
        }
    }
    let app = rust.is_none().then(|| {
        (
            "app.js".to_owned(),
            apps_runtime::bundle::script_source(&files, false),
        )
    });
    for (name, source) in files
        .iter()
        .filter(|(name, _)| name.starts_with("script-"))
        .chain(app.as_ref().map(|(name, source)| (name, source)))
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
    let root = rust
        .or_else(|| entry.parent())
        .context("Missing project directory")?;
    let binary = rust
        .map(|root| crate::rust::build(root, entry, true, target))
        .transpose()?;
    if let Some(path) = &binary {
        target.validate_binary(path)?;
    }
    let disabled =
        apps_runtime::bundle::disabled_features(files.get("package.json").map(String::as_str))?;
    let dynamic = ["jsExecute", "httpImport"]
        .into_iter()
        .find(|feature| disabled & apps_runtime::bundle::feature_bit(feature) == 0);
    if aot && let Some(feature) = dynamic {
        eprintln!(
            "apps: warning: --aot is ignored, because the {feature} feature runs code that is not compiled"
        );
    }
    let compiled = (aot && dynamic.is_none())
        .then(|| crate::aot::compile(&mut files, runtime))
        .transpose()?;
    let bytes = packaging::package_app(
        out,
        &files,
        runtime,
        root,
        target,
        binary.as_deref(),
        bytecode,
        compiled
            .as_ref()
            .map(|directory| directory.path().join("app.dylib"))
            .as_deref(),
    )?;
    println!(
        "apps: built {} bytes in {} ms → {}",
        bytes,
        started.elapsed().as_millis(),
        out.display()
    );
    Ok(())
}
