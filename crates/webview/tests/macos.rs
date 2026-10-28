#[test]
#[cfg(target_os = "macos")]
#[ignore = "requires the macOS window server and WebKit renderer services"]
fn native_webkit_loads_html_and_evaluates_javascript() {
    use std::{path::Path, process::Command};
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = tempfile::tempdir().unwrap();
    let binary = output.path().join("browser-smoke");
    let result = Command::new("xcrun")
        .args(["clang", "-fobjc-arc", "-Wno-incompatible-pointer-types"])
        .arg(root.join("src/platform/macos.m"))
        .arg(root.join("tests/macos.m"))
        .args([
            "-framework",
            "AppKit",
            "-framework",
            "WebKit",
            "-framework",
            "Foundation",
            "-framework",
            "CoreGraphics",
            "-o",
        ])
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let result = Command::new(binary).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("WebKit:42"));
}
