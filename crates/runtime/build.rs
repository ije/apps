#[path = "../../scripts/napi_exports.rs"]
mod napi_exports;

use std::{env, fs, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    napi_exports::export_symbols(&root.join("src/node/napi.rs"));
    let target = env::var("CARGO_CFG_TARGET_OS").unwrap();
    // Debug layout uses about 500 KB of stack per DOM level. The default 8 MB main stack is too small.
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
    let mut scripts = 0u64;
    let mut hash = |path: &std::path::Path, text: &str| {
        let bytes = path.to_string_lossy().into_owned() + text;
        scripts = scripts.wrapping_add(bytes.bytes().fold(0xcbf29ce484222325, |hash, byte| {
            (hash ^ byte as u64).wrapping_mul(0x100000001b3)
        }));
    };
    for dir in ["js", "js/node", "js/web"] {
        fs::create_dir_all(out.join(dir)).unwrap();
        for entry in fs::read_dir(root.join(dir)).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|extension| extension == "js") {
                // Strip only whitespace and comments: compression loses script completion values. Preserve
                // napi.js line numbers for napi.rs.
                let source = fs::read_to_string(&path).unwrap();
                if path.ends_with("node/napi.js") {
                    hash(path.strip_prefix(&root).unwrap(), &source);
                    let data = miniz_oxide::deflate::compress_to_vec(source.as_bytes(), 10);
                    fs::write(out.join(dir).join("napi.js"), data).unwrap();
                    continue;
                }
                let allocator = oxc_allocator::Allocator::default();
                let parsed =
                    oxc_parser::Parser::new(&allocator, &source, oxc_span::SourceType::script())
                        .with_options(oxc_parser::ParseOptions {
                            allow_return_outside_function: true,
                            ..Default::default()
                        })
                        .parse();
                assert!(
                    !parsed.diagnostics.has_errors(),
                    "Cannot parse {}",
                    path.display()
                );
                let text = oxc_codegen::Codegen::new()
                    .with_options(oxc_codegen::CodegenOptions {
                        ascii_only: true,
                        comments: oxc_codegen::CommentOptions::disabled(),
                        ..oxc_codegen::CodegenOptions::minify()
                    })
                    .build(&parsed.program)
                    .code;
                assert!(text.is_ascii(), "{} must stay ASCII", path.display());
                hash(path.strip_prefix(&root).unwrap(), &text);
                let data = miniz_oxide::deflate::compress_to_vec(text.as_bytes(), 10);
                fs::write(out.join(dir).join(path.file_name().unwrap()), data).unwrap();
            }
        }
    }
    println!("cargo:rustc-env=APPS_RUNTIME_SCRIPTS={scripts:016x}");
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
            "CoreText",
            "ImageIO",
            "ServiceManagement",
            "UniformTypeIdentifiers",
        ] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    }
}
