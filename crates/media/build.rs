use std::{env, fs, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let ffmpeg = root.join(format!("../../target/ffmpeg/{os}-{arch}"));
    println!("cargo:rustc-check-cfg=cfg(ffmpeg)");
    println!("cargo:rerun-if-changed=native/ffmpeg.c");
    println!("cargo:rerun-if-changed={}", ffmpeg.display());
    let lib = ffmpeg.join("lib");
    let name = match os.as_str() {
        "macos" => "libffmpeg.dylib",
        "windows" => "ffmpeg.dll",
        _ => "libffmpeg.so",
    };
    if !lib.join(name).is_file() {
        println!(
            "cargo:warning=FFmpeg is missing, so video and audio will not play. Run bash scripts/build-ffmpeg.sh"
        );
        return;
    }
    println!("cargo:rustc-cfg=ffmpeg");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    // WARNING: only the cdylib links FFmpeg, so test executables that link the rlib don't need it.
    cc::Build::new()
        .file(root.join("native/ffmpeg.c"))
        .include(ffmpeg.join("include"))
        .cargo_metadata(false)
        .compile("apps_ffmpeg");
    let msvc = env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let link = |arg: String| println!("cargo:rustc-cdylib-link-arg={arg}");
    if msvc {
        link(out.join("apps_ffmpeg.lib").display().to_string());
        link(format!("/LIBPATH:{}", lib.display()));
        link("ffmpeg.lib".to_owned());
    } else {
        link(out.join("libapps_ffmpeg.a").display().to_string());
        link(format!("-L{}", lib.display()));
        link("-lffmpeg".to_owned());
    }
    if os == "linux" {
        link("-Wl,-rpath,$ORIGIN".to_owned());
    }
    // The library goes next to the media library in target/<profile> and in deps for tests.
    let profile = out.ancestors().nth(3).unwrap();
    let path = lib.join(name);
    for folder in [profile.to_owned(), profile.join("deps")] {
        let target = folder.join(name);
        // WARNING: macOS kills processes that load a library rewritten in place, so copy to a new file.
        if fs::read(&target).ok() != Some(fs::read(&path).unwrap()) {
            let temporary = folder.join(format!(".{name}"));
            fs::copy(&path, &temporary).unwrap();
            fs::rename(&temporary, &target).unwrap();
        }
        fs::copy(ffmpeg.join("FFmpeg.LICENSE"), folder.join("FFmpeg.LICENSE")).unwrap();
    }
}
