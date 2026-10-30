use std::{env, fs, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let source = if root.ends_with("runtime") {
        root.join("src/node/napi.rs")
    } else {
        root.join("crates/runtime/src/node/napi.rs")
    };
    println!("cargo:rerun-if-changed={}", source.display());
    let target = env::var("CARGO_CFG_TARGET_OS").unwrap();
    if target == "macos" && env::var("PROFILE").as_deref() == Ok("release") {
        println!("cargo:rustc-link-arg=-Wl,-x");
    }
    // WARNING: elements build recursively on the main thread, one frame per DOM level, and debug
    // frames are about 500 KB. The default 8 MB stack overflows on ordinary pages.
    if root.ends_with("runtime") {
        match (target.as_str(), env::var("CARGO_CFG_TARGET_ENV").as_deref()) {
            ("macos", _) => {
                println!("cargo:rustc-link-arg-bin=apps-runtime=-Wl,-stack_size,0x4000000")
            }
            ("windows", Ok("msvc")) => {
                println!("cargo:rustc-link-arg-bin=apps-runtime=/STACK:67108864")
            }
            ("windows", _) => {
                println!("cargo:rustc-link-arg-bin=apps-runtime=-Wl,--stack,67108864")
            }
            _ => {}
        }
    }
    if root.ends_with("runtime") {
        let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
        let directory = root.join(format!("../../target/bun-jsc/{target}-{arch}"));
        println!("cargo:rustc-env=APPS_JSC_BUILD_DIR={}", directory.display());
    }
    if target == "macos" && root.ends_with("runtime") {
        cc::Build::new()
            .file(root.join("native/desktop.m"))
            .flag("-fobjc-arc")
            .flag("-Wno-incompatible-pointer-types")
            .compile("apps_native");
        println!("cargo:rerun-if-changed=native/desktop.m");
        for framework in [
            "AppKit",
            "Foundation",
            "Security",
            "UserNotifications",
            "Carbon",
            "IOKit",
            "CoreGraphics",
            "ServiceManagement",
        ] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
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
