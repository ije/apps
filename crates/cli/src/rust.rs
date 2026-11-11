use crate::build_target::BuildTarget;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub fn project(entry: &Path) -> Result<Option<PathBuf>> {
    let Some(path) = entry
        .ancestors()
        .skip(1)
        .map(|dir| dir.join("package.json"))
        .find(|p| p.is_file())
    else {
        return Ok(None);
    };
    let package: Value =
        serde_json::from_slice(&fs::read(&path)?).context("Invalid package.json")?;
    if package["type"] != "rust" {
        return Ok(None);
    }
    ensure!(
        entry.extension().is_some_and(|ext| ext == "rs"),
        "A Rust app needs a .rs main"
    );
    Ok(Some(path.parent().unwrap().to_owned()))
}

pub fn build(root: &Path, entry: &Path, release: bool, target: BuildTarget) -> Result<PathBuf> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut manifest = root.join("Cargo.toml");
    let name;
    if manifest.is_file() {
        let out = Command::new(&cargo)
            .current_dir(root)
            .args([
                "metadata",
                "--format-version=1",
                "--no-deps",
                "--manifest-path",
            ])
            .arg(&manifest)
            .output()
            .context("Cannot start Cargo")?;
        ensure!(
            out.status.success(),
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let meta: Value = serde_json::from_slice(&out.stdout)?;
        name = meta["packages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|pkg| {
                pkg["manifest_path"]
                    .as_str()
                    .is_some_and(|p| Path::new(p) == manifest)
            })
            .flat_map(|pkg| pkg["targets"].as_array().into_iter().flatten())
            .find(|t| {
                t["kind"] == json!(["bin"])
                    && t["src_path"]
                        .as_str()
                        .and_then(|s| Path::new(s).canonicalize().ok())
                        .as_deref()
                        == Some(entry)
            })
            .and_then(|t| t["name"].as_str())
            .context("package.json main must be a binary target in Cargo.toml")?
            .to_owned();
    } else {
        let package = fs::read_to_string(root.join("package.json"))?;
        name = apps_runtime::bundle::package_metadata(Some(&package))?.0;
        let sdk = std::env::var_os("APPS_RUST_SDK")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime"));
        let sdk = sdk
            .canonicalize()
            .context("Set APPS_RUST_SDK to the apps-runtime crate directory")?;
        let directory = root.join(".apps/cargo");
        fs::create_dir_all(&directory)?;
        manifest = directory.join("Cargo.toml");
        let (_, profiles) = include_str!("../../../Cargo.toml")
            .split_once("[profile.release]")
            .context("The workspace Cargo.toml has no release profile")?;
        let source = format!(
            "[workspace]\n[package]\nname = {}\nversion = \"0.0.0\"\nedition = \"2024\"\n[[bin]]\nname = {}\npath = {}\n[dependencies]\napps = {{ package = \"apps-runtime\", path = {} }}\n[profile.release]{profiles}",
            json!(name),
            json!(name),
            json!(entry),
            json!(sdk)
        );
        if fs::read_to_string(&manifest).ok().as_deref() != Some(&source) {
            let mut file = tempfile::NamedTempFile::new_in(&directory)?;
            file.write_all(source.as_bytes())?;
            file.persist(&manifest)?;
        }
    }
    let host = BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH)?;
    let mut cmd = Command::new(cargo);
    cmd.current_dir(root)
        .args([
            "rustc",
            "--message-format=json-render-diagnostics",
            "--manifest-path",
        ])
        .arg(&manifest)
        .args(["--bin", &name]);
    if release {
        cmd.arg("--release");
    }
    if target != host {
        let arch = if target.arch == "arm64" {
            "aarch64"
        } else {
            "x86_64"
        };
        let os = match target.platform {
            "macos" => "apple-darwin",
            "windows" => "pc-windows-msvc",
            _ => "unknown-linux-gnu",
        };
        cmd.args(["--target", &format!("{arch}-{os}")]);
    }
    let mut flags = crate::napi_exports::link_args(
        include_str!("../../runtime/src/node/napi.rs"),
        target.platform,
        release,
    );
    match target.platform {
        "macos" => flags.push("-Wl,-stack_size,0x4000000".into()),
        "windows" if cfg!(target_env = "gnu") && target == host => {
            flags.push("-Wl,--stack,67108864".into())
        }
        "windows" => flags.push("/STACK:67108864".into()),
        _ => {}
    }
    let output = cmd
        .arg("--")
        .args(flags.into_iter().map(|flag| format!("-Clink-arg={flag}")))
        .stderr(Stdio::inherit())
        .output()
        .context("Cannot start Cargo")?;
    ensure!(output.status.success(), "Cargo could not build {name}");
    for line in std::str::from_utf8(&output.stdout)?.lines() {
        let message: Value = serde_json::from_str(line)?;
        if message["reason"] == "compiler-artifact"
            && message["target"]["name"] == name
            && let Some(path) = message["executable"].as_str()
        {
            return Ok(PathBuf::from(path));
        }
    }
    anyhow::bail!("Cargo did not produce an executable for the Rust main")
}
