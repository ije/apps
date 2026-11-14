use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let ghostty = root.join(format!("../../target/ghostty/{os}-{arch}"));
    println!("cargo:rerun-if-changed=native/ghostty.c");
    println!("cargo:rerun-if-changed={}", ghostty.display());
    let name = if os == "windows" {
        "ghostty-vt-static"
    } else {
        "ghostty-vt"
    };
    let lib = ghostty.join("lib");
    if !["a", "lib"].iter().any(|ext| {
        lib.join(format!("lib{name}.{ext}")).is_file()
            || lib.join(format!("{name}.{ext}")).is_file()
    }) {
        println!(
            "cargo:warning=libghostty-vt is missing, so the terminal element will not work. Run bash scripts/build-ghostty.sh"
        );
        return;
    }
    println!("cargo:rustc-cfg=ghostty");
    cc::Build::new()
        .file(root.join("native/ghostty.c"))
        .include(ghostty.join("include"))
        .define("GHOSTTY_STATIC", None)
        .warnings(false)
        .compile("apps_ghostty");
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=static={name}");
    // Cargo 1.100 moved the output folder one level down, so find the profile folder by name.
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let profile = out
        .ancestors()
        .find(|folder| folder.ends_with("build"))
        .and_then(Path::parent)
        .unwrap();
    fs::copy(
        ghostty.join("Ghostty.LICENSE"),
        profile.join("Ghostty.LICENSE"),
    )
    .unwrap();
}
