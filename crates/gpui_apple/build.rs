//! Compiles the Metal shaders ahead of time when the Metal toolchain is installed, so apps skip
//! shader compilation at launch. Without it, the shaders are compiled at runtime.
#![allow(clippy::disallowed_methods, reason = "build scripts are exempt")]

use std::{env, process::Command};

fn main() {
    println!("cargo::rustc-check-cfg=cfg(gpui_shaders)");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let source = "src/shaders.metal";
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
    println!("cargo::rustc-cfg=gpui_shaders");
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
