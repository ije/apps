//! Compiles the Metal shaders, and the HLSL shaders of release builds, ahead of time when the
//! compiler is installed, so apps skip shader compilation at launch. Without it, the shaders are
//! compiled at runtime.

use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo::rustc-check-cfg=cfg(render_shaders)");
    match env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("macos") => metal(),
        Ok("windows") if env::var("PROFILE").as_deref() == Ok("release") => hlsl(),
        _ => {}
    }
}

fn hlsl() {
    let source = "src/platform/windows/shaders.hlsl";
    println!("cargo::rerun-if-changed={source}");
    println!("cargo::rerun-if-env-changed=RENDER_FXC");
    // `fxc` ships with the Windows SDK.
    let sdk = PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut versions: Vec<_> = std::fs::read_dir(&sdk)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with("10."))
        .collect();
    versions.sort_by_key(|version| {
        version
            .split('.')
            .filter_map(|part| part.parse().ok())
            .collect::<Vec<u32>>()
    });
    let fxc = env::var_os("RENDER_FXC")
        .map(PathBuf::from)
        .or_else(|| {
            versions
                .last()
                .map(|version| sdk.join(version).join("x64").join("fxc.exe"))
        })
        .filter(|fxc| fxc.exists())
        .unwrap_or_else(|| "fxc.exe".into());
    if Command::new(&fxc).arg("/?").output().is_err() {
        println!("cargo::warning=fxc is missing, shaders are compiled at runtime");
        return;
    }
    let out = env::var("OUT_DIR").unwrap();
    for module in [
        "quad",
        "shadow",
        "underline",
        "monochrome_sprite",
        "polychrome_sprite",
        "composite",
        "blur",
    ] {
        for (stage, target) in [("vertex", "vs_4_1"), ("fragment", "ps_4_1")] {
            let entry = format!("{module}_{stage}");
            let output = format!("{out}/{entry}.cso");
            run(Command::new(&fxc).args([
                "/nologo", "/O3", "/T", target, "/E", &entry, "/Fo", &output, source,
            ]));
        }
    }
    println!("cargo::rustc-cfg=render_shaders");
}

fn metal() {
    let source = "src/platform/mac/shaders.metal";
    println!("cargo::rerun-if-changed={source}");
    if !Command::new("xcrun")
        .args(["-sdk", "macosx", "-f", "metal"])
        .output()
        .is_ok_and(|output| output.status.success())
    {
        println!("cargo::warning=the Metal toolchain is missing, shaders are compiled at runtime");
        return;
    }
    let out = env::var("OUT_DIR").unwrap();
    let air = format!("{out}/shaders.air");
    run(Command::new("xcrun").args([
        "-sdk",
        "macosx",
        "metal",
        "-mmacosx-version-min=10.15.7",
        "-c",
        source,
        "-o",
        &air,
    ]));
    run(Command::new("xcrun").args([
        "-sdk",
        "macosx",
        "metallib",
        &air,
        "-o",
        &format!("{out}/shaders.metallib"),
    ]));
    println!("cargo::rustc-cfg=render_shaders");
}

fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "shader compilation failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
