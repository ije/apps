use std::{fs, path::Path, process::Command};

fn binary(platform: &str, arch: &str) -> Vec<u8> {
    match platform {
        "macos" => {
            let mut bytes = vec![0; 32];
            bytes[..4].copy_from_slice(&0xfeed_facfu32.to_le_bytes());
            bytes[4..8].copy_from_slice(
                &(if arch == "arm64" {
                    0x0100_000cu32
                } else {
                    0x0100_0007u32
                })
                .to_le_bytes(),
            );
            bytes[12..16].copy_from_slice(&2u32.to_le_bytes());
            bytes
        }
        "linux" => {
            let mut bytes = vec![0; 64];
            bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
            bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
            bytes[18..20]
                .copy_from_slice(&(if arch == "arm64" { 183u16 } else { 62u16 }).to_le_bytes());
            bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
            bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
            bytes
        }
        "windows" => {
            let mut bytes = vec![0; 512];
            bytes[..2].copy_from_slice(b"MZ");
            bytes[60..64].copy_from_slice(&128u32.to_le_bytes());
            bytes[128..132].copy_from_slice(b"PE\0\0");
            bytes[132..134].copy_from_slice(
                &(if arch == "arm64" {
                    0xaa64u16
                } else {
                    0x8664u16
                })
                .to_le_bytes(),
            );
            bytes[148..150].copy_from_slice(&240u16.to_le_bytes());
            bytes[150..152].copy_from_slice(&2u16.to_le_bytes());
            bytes[152..154].copy_from_slice(&0x20bu16.to_le_bytes());
            bytes
        }
        _ => unreachable!(),
    }
}

fn apps(entry: &Path, runtime: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_apps"));
    command
        .arg("build")
        .arg(entry)
        .env("APPS_RUNTIME", runtime)
        .env("APPS_JSC_LIBRARY", runtime);
    command
}

#[test]
fn resolves_default_entries_with_build_options() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = dir.path().join("runtime");
    let out = dir.path().join("output");
    fs::write(&runtime, binary("windows", "x64")).unwrap();
    fs::write(dir.path().join("main.js"), "invalid javascript !!!").unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_apps"));
    command
        .args([
            "build",
            "--platform",
            "windows",
            "--arch",
            "x64",
            "--out-dir",
        ])
        .arg(&out)
        .current_dir(dir.path())
        .env("APPS_RUNTIME", &runtime)
        .env("APPS_JSC_LIBRARY", &runtime);
    for (entry, source) in [
        ("main.ts", "const answer: number = 42; console.log(answer);"),
        ("main.js", "console.log(42);"),
    ] {
        fs::write(dir.path().join(entry), source).unwrap();
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(apps_runtime::bundle::read(&out).unwrap().files["app.js"].contains("42"));
        fs::remove_file(dir.path().join(entry)).unwrap();
    }
}

#[test]
fn packages_every_platform_and_arch_with_matching_jsc() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("app.ts");
    let runtime = dir.path().join("runtime");
    fs::write(&entry, "const answer: number = 42; console.log(answer);").unwrap();
    fs::write(
        dir.path().join("package.json"),
        r#"{"name":"Tea","productName":"Tea App"}"#,
    )
    .unwrap();
    fs::create_dir(dir.path().join("public")).unwrap();
    fs::write(dir.path().join("public/asset.txt"), "asset").unwrap();
    fs::write(dir.path().join("JavaScriptCore.LICENSE"), "engine license").unwrap();
    for platform in ["macos", "linux", "windows"] {
        for arch in ["x64", "arm64"] {
            let native = binary(platform, arch);
            fs::write(&runtime, &native).unwrap();
            let result = apps(&entry, &runtime)
                .args(["--platform", platform, "--arch", arch])
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            let out = dir.path().join(format!("dist/{platform}-{arch}"));
            let executable = match platform {
                "macos" => "Tea App.app/Contents/MacOS/Tea",
                "windows" => "Tea.exe",
                _ => "Tea",
            };
            let resources = if platform == "macos" {
                "Tea App.app/Contents/Resources/"
            } else {
                ""
            };
            assert_eq!(fs::read(out.join(executable)).unwrap(), native);
            let library = match platform {
                "macos" => "Tea App.app/Contents/Frameworks/libJavaScriptCore.dylib",
                "windows" => "JavaScriptCore.dll",
                _ => "libJavaScriptCore.so",
            };
            assert_eq!(fs::read(out.join(library)).unwrap(), native);
            assert_eq!(
                fs::read(out.join(library).with_file_name("JavaScriptCore.LICENSE")).unwrap(),
                b"engine license"
            );
            assert_eq!(
                fs::read(out.join(format!("{resources}asset.txt"))).unwrap(),
                b"asset"
            );
            assert!(out.join(format!("{resources}app.h2o")).is_file());
            assert!(
                apps_runtime::bundle::read(&out)
                    .unwrap()
                    .files
                    .contains_key("app.js")
            );
            assert_eq!(
                out.join("Tea App.app/Contents/Info.plist").is_file(),
                platform == "macos"
            );
            let mut zip =
                zip::ZipArchive::new(fs::File::open(out.join("Tea App.zip")).unwrap()).unwrap();
            assert!(zip.by_name(library).is_ok());
            assert!(zip.by_name(&format!("{resources}app.h2o")).is_ok());
            assert!(zip.by_name(&format!("{resources}asset.txt")).is_ok());
            let mut file = zip.by_name(executable).unwrap();
            if platform != "windows" {
                assert_eq!(file.unix_mode().unwrap() & 0o777, 0o755);
            }
            let mut archived = Vec::new();
            std::io::Read::read_to_end(&mut file, &mut archived).unwrap();
            assert_eq!(archived, native);
        }
    }
}

#[test]
fn rejects_wrong_runtime_addon_and_windows_names_without_replacing_output() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("app.js");
    let runtime = dir.path().join("runtime");
    let out = dir.path().join("output");
    fs::write(&entry, "console.log('hello');").unwrap();
    fs::write(&runtime, binary("windows", "x64")).unwrap();
    let mut command = apps(&entry, &runtime);
    command
        .args(["--platform", "windows", "--arch", "x64", "--out-dir"])
        .arg(&out);
    assert!(command.output().unwrap().status.success());
    fs::write(out.join("keep.txt"), "keep").unwrap();
    for (platform, arch) in [("macos", "x64"), ("windows", "arm64"), ("linux", "x64")] {
        fs::write(&runtime, binary(platform, arch)).unwrap();
        let result = command.output().unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("does not match windows-x64"));
        assert_eq!(fs::read(out.join("keep.txt")).unwrap(), b"keep");
    }
    fs::write(&runtime, binary("windows", "x64")).unwrap();
    fs::write(
        &entry,
        "import addon from './addon.node'; console.log(addon);",
    )
    .unwrap();
    fs::write(dir.path().join("addon.node"), binary("macos", "x64")).unwrap();
    let result = command.output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("Native addon does not match"));
    fs::write(&entry, "console.log('hello');").unwrap();
    for package in [
        r#"{"name":"CON"}"#,
        r#"{"name":"App","productName":"LPT1.zip"}"#,
    ] {
        fs::write(dir.path().join("package.json"), package).unwrap();
        let result = command.output().unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("Invalid Windows filename"));
    }
    assert_eq!(fs::read(out.join("keep.txt")).unwrap(), b"keep");
}

#[test]
fn requires_matching_jsc_without_replacing_output() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("app.js");
    let runtime = dir.path().join("runtime");
    let jsc = dir.path().join("JavaScriptCore");
    fs::write(&entry, "console.log('hello');").unwrap();
    fs::write(&runtime, binary("macos", "arm64")).unwrap();
    let native = binary("macos", "arm64");
    fs::write(&jsc, &native).unwrap();
    let mut command = apps(&entry, &runtime);
    command
        .args(["--platform", "macos", "--arch", "arm64"])
        .env("APPS_JSC_LIBRARY", &jsc);
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let out = dir.path().join("dist/macos-arm64");
    let library = "App.app/Contents/Frameworks/libJavaScriptCore.dylib";
    assert_eq!(fs::read(out.join(library)).unwrap(), native);
    let mut zip = zip::ZipArchive::new(fs::File::open(out.join("App.zip")).unwrap()).unwrap();
    assert!(zip.by_name(library).is_ok());
    drop(zip);
    fs::write(&jsc, binary("macos", "x64")).unwrap();
    let result = command.output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("JavaScriptCore does not match"));
    assert_eq!(fs::read(out.join(library)).unwrap(), native);
    fs::remove_file(&jsc).unwrap();
    let result = command.output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("standalone library file"));
    let result = command.arg("--bundle-jsc").output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("Unknown option: --bundle-jsc"));
}

#[test]
fn rejects_invalid_options_and_missing_target_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("app.js");
    let runtime = dir.path().join("runtime");
    fs::write(&entry, "").unwrap();
    for (args, error) in [
        (vec!["--arch", "ia32"], "Unsupported architecture"),
        (vec!["--platform", "android"], "Unsupported platform"),
        (vec!["--arch"], "Expected architecture"),
        (vec!["--platform"], "Expected platform"),
    ] {
        let result = apps(&entry, &runtime).args(args).output().unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains(error));
    }
    let platform = if cfg!(target_os = "macos") {
        "windows"
    } else {
        "macos"
    };
    let result = apps(&entry, &runtime)
        .args(["--platform", platform])
        .env_remove("APPS_RUNTIME")
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("Set APPS_RUNTIME"));
}

#[test]
fn invalid_jsc_override_does_not_use_system_engine() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("app.js");
    fs::write(&entry, "console.log('unexpected engine');").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_apps"))
        .arg("run")
        .arg(entry)
        .env("APPS_JSC_LIBRARY", dir.path().join("missing-jsc"))
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("APPS_JSC_LIBRARY"));
    assert!(!String::from_utf8_lossy(&result.stdout).contains("unexpected engine"));
}

#[test]
fn accepts_universal_macos_runtimes_and_target_aliases() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("app.js");
    let runtime = dir.path().join("runtime");
    fs::write(&entry, "").unwrap();
    let mut fat = vec![0; 64];
    fat[..4].copy_from_slice(&0xcafe_babeu32.to_be_bytes());
    fat[4..8].copy_from_slice(&2u32.to_be_bytes());
    for (index, (arch, cpu)) in [("x64", 0x0100_0007u32), ("arm64", 0x0100_000cu32)]
        .into_iter()
        .enumerate()
    {
        let start = 8 + index * 20;
        fat[start..start + 4].copy_from_slice(&cpu.to_be_bytes());
        fat[start + 8..start + 12].copy_from_slice(&(64 + index as u32 * 32).to_be_bytes());
        fat[start + 12..start + 16].copy_from_slice(&32u32.to_be_bytes());
        fat.extend(binary("macos", arch));
    }
    fs::write(&runtime, &fat).unwrap();
    for (alias, arch) in [("x86_64", "x64"), ("aarch64", "arm64")] {
        let result = apps(&entry, &runtime)
            .args(["--platform", "darwin", "--arch", alias])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            fs::read(
                dir.path()
                    .join(format!("dist/macos-{arch}/App.app/Contents/MacOS/App"))
            )
            .unwrap(),
            fat
        );
    }
    fat[4..8].copy_from_slice(&1u32.to_be_bytes());
    fs::write(&runtime, &fat).unwrap();
    let result = apps(&entry, &runtime)
        .args(["--platform", "macos", "--arch", "arm64"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("has no arm64 binary"));
}
