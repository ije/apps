use std::{env, fs, path::Path};

pub fn export_symbols(source: &Path) {
    println!("cargo:rerun-if-changed={}", source.display());
    let target = env::var("CARGO_CFG_TARGET_OS").unwrap();
    if target == "macos" && env::var("PROFILE").as_deref() == Ok("release") {
        println!("cargo:rustc-link-arg=-Wl,-x");
    }
    for part in fs::read_to_string(source).unwrap().split("api!(").skip(1) {
        let symbol = part.split('(').next().unwrap();
        match target.as_str() {
            "macos" => println!("cargo:rustc-link-arg=-Wl,-exported_symbol,_{symbol}"),
            "windows" => println!("cargo:rustc-link-arg=/EXPORT:{symbol}"),
            _ => println!("cargo:rustc-link-arg=-Wl,--export-dynamic-symbol={symbol}"),
        }
    }
}
