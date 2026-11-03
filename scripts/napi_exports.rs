use std::{env, fs, path::Path};

pub fn link_args(source: &str, target: &str, release: bool) -> Vec<String> {
    let mut args = Vec::new();
    if target == "macos" && release {
        args.push("-Wl,-x".into());
    }
    for part in source.split("api!(").skip(1) {
        let symbol = part.split('(').next().unwrap();
        args.push(match target {
            "macos" => format!("-Wl,-exported_symbol,_{symbol}"),
            "windows" => format!("/EXPORT:{symbol}"),
            _ => format!("-Wl,--export-dynamic-symbol={symbol}"),
        });
    }
    args
}

pub fn export_symbols(source: &Path) {
    println!("cargo:rerun-if-changed={}", source.display());
    for arg in link_args(
        &fs::read_to_string(source).unwrap(),
        &env::var("CARGO_CFG_TARGET_OS").unwrap(),
        env::var("PROFILE").as_deref() == Ok("release"),
    ) {
        println!("cargo:rustc-link-arg={arg}");
    }
}
