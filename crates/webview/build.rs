fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/platform/macos.m")
            .flag("-fobjc-arc")
            .compile("apps_webview");
        println!("cargo:rerun-if-changed=src/platform/macos.m");
        for framework in ["AppKit", "WebKit", "Foundation"] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    }
}
