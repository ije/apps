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
            "-framework",
            "ServiceManagement",
            "-framework",
            "UniformTypeIdentifiers",
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
fn oversized_windows_open_and_report_their_size() {
    use std::{
        io::Write,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let main = r#"
        const { app, BrowserWindow, ipcMain } = __apps;
        ipcMain.on('size', (_, width, height) => {
            if (!(width > 0 && height > 0 && width < 6000)) throw Error('Wrong size: ' + width + 'x' + height);
            console.log('OVERSIZED_OK'); app.quit();
        });
        app.whenReady().then(() => {
            new BrowserWindow({ width: 6000, height: 5000 }).loadFile('apps:///index.html');
            setTimeout(() => process.exit(1), 5000);
        });
    "#;
    let page = r#"
        <body>
        <script type='preload'>__apps.contextBridge.exposeInMainWorld('report', (...args) => __apps.ipcRenderer.send('size', ...args));</script>
        <script>addEventListener('resize', () => report(innerWidth, innerHeight));</script>
    "#;
    let bundle = serde_json::json!({"files": [
        ["app.js", format!("$appsDefine({});", serde_json::json!({"id":0,"imports":[],"exports":[],"stars":[],"code":main}))],
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
        output.status.success() && stdout.contains("OVERSIZED_OK"),
        "{stdout}\n{stderr}"
    );
}

#[test]
#[cfg(target_os = "macos")]
#[ignore = "requires the macOS window server"]
fn native_captures_repeat_and_fatal_errors_exit_with_failure() {
    use std::{
        io::Write,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let capture = r#"
        const { app, BrowserWindow } = __apps;
        app.whenReady().then(async () => {
            setTimeout(() => process.exit(3), 8000);
            const win = new BrowserWindow({ width: 300, height: 200 });
            await win.loadFile('apps:///index.html');
            for (let i = 0; i < 4; i++) if ((await win.webContents.capturePage()).isEmpty()) throw Error('empty capture');
            console.log('CAPTURE_OK'); app.quit();
        });
    "#;
    let fatal = "__apps.app.whenReady().then(() => { new __apps.BrowserWindow(); setTimeout(() => { throw Error('FATAL_IN_TIMER'); }, 50); });";
    for (main, success, marker) in [
        (capture, true, "CAPTURE_OK"),
        (fatal, false, "FATAL_IN_TIMER"),
    ] {
        let bundle = serde_json::json!({"files": [
            ["app.js", format!("$appsDefine({});", serde_json::json!({"id":0,"imports":[],"exports":[],"stars":[],"code":main}))],
            ["index.html", "<body style='background:red'>x</body>"],
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
        let text =
            String::from_utf8_lossy(&output.stdout) + String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success() == success
                && output.status.code().is_some()
                && text.contains(marker),
            "{:?} {text}",
            output.status
        );
    }
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
        ["app.js", format!("$appsDefine({});", serde_json::json!({"id":0,"imports":[],"exports":[],"stars":[],"code":main}))],
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

#[test]
#[cfg(target_os = "macos")]
#[ignore = "requires the macOS window server"]
fn native_browser_window_geometry_state_and_close() {
    use std::{
        io::Write,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let main = r#"
        const { app, BrowserWindow } = __apps;
        const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
        const once = (win, name) => new Promise((resolve, reject) => { const timer = setTimeout(() => reject(Error('No ' + name + ' event')), 5000); win.once(name, () => { clearTimeout(timer); resolve(); }); });
        const expect = (ok, what) => { if (!ok) throw Error(what + ': ' + JSON.stringify([win.getBounds(), win.getContentBounds()])); };
        let win;
        app.on('window-all-closed', () => {});
        app.whenReady().then(async () => {
            setTimeout(() => { throw Error('Window test timed out'); }, 20000);
            win = new BrowserWindow({ x: 80, y: 90, width: 400, height: 300, minWidth: 200, minHeight: 150, show: false });
            win.load('<title>Native</title><body></body>');
            await once(win, 'ready-to-show');
            const bar = win.getSize()[1] - win.getContentSize()[1];
            expect(same(win.getBounds(), { x: 80, y: 90, width: 400, height: 300 }) && bar > 0 && !win.isVisible(), 'initial bounds');
            const focus = once(win, 'focus');
            win.show();
            await focus;
            expect(win.isVisible() && win.isFocused() && BrowserWindow.getFocusedWindow() === win, 'show');
            win.setBounds({ x: 120, y: 130, width: 500, height: 350 });
            await once(win, 'resized');
            expect(same(win.getBounds(), { x: 120, y: 130, width: 500, height: 350 }), 'setBounds');
            win.setContentSize(100, 100);
            await once(win, 'resized');
            expect(same(win.getSize(), [200, 150]), 'minimum size');
            const maximized = once(win, 'resized');
            win.maximize();
            await once(win, 'maximize');
            await maximized;
            expect(win.isMaximized() && win.getSize()[0] > 500, 'maximize');
            const restored = once(win, 'resized');
            win.unmaximize();
            await once(win, 'unmaximize');
            await restored;
            expect(!win.isMaximized() && same(win.getSize(), [200, 150]), 'unmaximize');
            win.minimize();
            await once(win, 'minimize');
            expect(win.isMinimized() && !win.isVisible(), 'minimize');
            win.restore();
            await once(win, 'restore');
            expect(!win.isMinimized() && win.isVisible(), 'restore');
            win.hide();
            await once(win, 'hide');
            win.show();
            await once(win, 'show');
            let closes = 0;
            win.on('close', event => { if (++closes === 1) event.preventDefault(); });
            win.close();
            await new Promise(resolve => setTimeout(resolve, 100));
            expect(!win.isDestroyed(), 'close prevented');
            win.close();
            await once(win, 'closed');
            console.log('BROWSER_WINDOW_OK');
            app.quit();
        });
    "#;
    let bundle = serde_json::json!({"files": [
        ["app.js", format!("$appsDefine({});", serde_json::json!({"id":0,"imports":[],"exports":[],"stars":[],"code":main}))],
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
    let deadline = Instant::now() + Duration::from_secs(25);
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
        output.status.success() && stdout.contains("BROWSER_WINDOW_OK"),
        "{stdout}\n{stderr}"
    );
}
