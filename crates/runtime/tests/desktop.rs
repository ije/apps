#[test]
#[cfg(target_os = "macos")]
#[ignore = "requires the macOS window server"]
fn native_desktop_dispatches_application_events() {
    use std::{path::Path, process::Command};
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = tempfile::tempdir().unwrap();
    let engine = apps_runtime::jsc::Engine::new(
        apps_runtime::jsc::Api::load().unwrap(),
        std::rc::Rc::default(),
        apps_runtime::host::Role::Main,
    )
    .unwrap();
    engine
        .eval(include_str!("../js/browser.js"), "browser")
        .unwrap();
    engine.eval(include_str!("menu.js"), "menu-tests").unwrap();
    let template = output.path().join("menu.json");
    std::fs::write(
        &template,
        engine.eval("JSON.stringify(menuTemplate)", "menu").unwrap(),
    )
    .unwrap();
    let binary = output.path().join("desktop-smoke");
    let result = Command::new("xcrun")
        .args(["clang", "-fobjc-arc", "-Wno-incompatible-pointer-types"])
        .arg(root.join("native/desktop.m"))
        .arg(root.join("tests/desktop_native.m"))
        .args([
            "-framework",
            "AppKit",
            "-framework",
            "Foundation",
            "-framework",
            "Security",
            "-framework",
            "UserNotifications",
            "-framework",
            "Carbon",
            "-framework",
            "IOKit",
            "-framework",
            "CoreGraphics",
            "-o",
        ])
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}: {}",
        result.status,
        String::from_utf8_lossy(&result.stderr)
    );
    let result = Command::new(binary).arg(template).output().unwrap();
    assert!(
        result.status.success(),
        "{}: {}",
        result.status,
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn application_menu_defaults_and_overrides() {
    let engine = apps_runtime::jsc::Engine::new(
        apps_runtime::jsc::Api::load().unwrap(),
        std::rc::Rc::default(),
        apps_runtime::host::Role::Main,
    )
    .unwrap();
    engine
        .eval(include_str!("../js/browser.js"), "browser")
        .unwrap();
    engine.eval(include_str!("menu.js"), "menu-tests").unwrap();
}

#[test]
fn native_theme_properties_and_events() {
    let engine = apps_runtime::jsc::Engine::new(
        apps_runtime::jsc::Api::load().unwrap(),
        std::rc::Rc::default(),
        apps_runtime::host::Role::Main,
    )
    .unwrap();
    engine
        .eval(include_str!("../js/browser.js"), "browser")
        .unwrap();
    engine
        .eval(include_str!("native_theme.js"), "native-theme-tests")
        .unwrap();
}

#[test]
#[cfg(target_os = "macos")]
#[ignore = "requires the macOS window server"]
fn native_theme_updates_hidden_renderers_and_cached_styles() {
    use std::{
        io::Write,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let main = r#"
        const { app, BrowserWindow, ipcMain, nativeTheme } = __apps;
        nativeTheme.themeSource = 'dark';
        let reports = 0;
        const updates = [];
        const finish = () => {
            if (reports !== 3 || !updates.includes(false) || updates.at(-1) !== true) return;
            nativeTheme.themeSource = 'system';
            console.log('NATIVE_THEME_OK'); app.quit();
        };
        nativeTheme.on('updated', () => { updates.push(nativeTheme.shouldUseDarkColors); finish(); });
        ipcMain.on('theme', (_, dark, light, color, changes) => {
            const expected = reports !== 1;
            if (dark !== expected || light === expected || color !== (expected ? 'white' : 'black') || changes !== reports)
                throw Error('Theme mismatch: ' + JSON.stringify({ dark, light, color, changes, reports }));
            reports++;
            if (reports < 3) setTimeout(() => { nativeTheme.themeSource = reports === 1 ? 'light' : 'dark'; }, 40);
            else finish();
        });
        app.whenReady().then(() => {
            new BrowserWindow({ show: false }).loadFile('apps:///index.html');
            setTimeout(() => { throw Error('Theme notification did not wake the idle runtime: ' + JSON.stringify({ reports, updates })); }, 5000);
        });
    "#;
    let page = r#"
        <style>body { color: black } @media (prefers-color-scheme: dark) { body { color: white } }</style>
        <body>
        <script type='preload'>__apps.contextBridge.exposeInMainWorld('report', (...args) => __apps.ipcRenderer.send('theme', ...args));</script>
        <script>
            const dark = matchMedia('(prefers-color-scheme: dark)'), light = matchMedia('(prefers-color-scheme: light)');
            let changes = 0;
            dark.addEventListener('change', event => {
                if (event.matches !== dark.matches || event.media !== dark.media) throw Error('MediaQueryListEvent');
                report(dark.matches, light.matches, getComputedStyle(document.body).color, ++changes);
            });
            report(dark.matches, light.matches, getComputedStyle(document.body).color, changes);
        </script>
    "#;
    let bundle = serde_json::json!({"files": [
        ["app.js", format!("define({});", serde_json::json!({"id":0,"imports":[],"exports":[],"stars":[],"code":main}))],
        ["index.html", page],
    ], "pages": []});
    let mut child = Command::new(env!("CARGO_BIN_EXE_apps-runtime"))
        .arg("--bundle-stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(bundle.to_string().as_bytes())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(12);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("NATIVE_THEME_OK"),
        "{stdout}\n{stderr}"
    );
}
