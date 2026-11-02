#[path = "../../scripts/napi_exports.rs"]
mod napi_exports;

use std::{env, fs, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    napi_exports::export_symbols(&root.join("src/node/napi.rs"));
    let target = env::var("CARGO_CFG_TARGET_OS").unwrap();
    // WARNING: elements build recursively on the main thread, one frame per DOM level, and debug
    // frames are about 500 KB. The default 8 MB stack overflows on ordinary pages.
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
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed={}", root.join("js").display());
    for dir in ["js", "js/node", "js/web"] {
        fs::create_dir_all(out.join(dir)).unwrap();
        for entry in fs::read_dir(root.join(dir)).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|extension| extension == "js") {
                let data = miniz_oxide::deflate::compress_to_vec(&fs::read(&path).unwrap(), 10);
                fs::write(out.join(dir).join(path.file_name().unwrap()), data).unwrap();
            }
        }
    }
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let directory = root.join(format!("../../target/bun-jsc/{target}-{arch}"));
    println!("cargo:rustc-env=APPS_JSC_BUILD_DIR={}", directory.display());
    if target == "macos" {
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
            "ImageIO",
            "ServiceManagement",
            "UniformTypeIdentifiers",
        ] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    }
}
