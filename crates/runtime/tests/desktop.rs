#[test]
#[cfg(target_os = "macos")]
#[ignore = "requires the macOS window server"]
fn native_desktop_dispatches_application_events() {
    use std::{path::Path, process::Command};
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = tempfile::tempdir().unwrap();
    let engine = apps_runtime::jsc::JsContext::new(
        apps_runtime::jsc::JavaScriptCore::load().unwrap(),
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
            "ImageIO",
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
    for argument in [Path::new("--tray"), &template] {
        let result = Command::new(&binary).arg(argument).output().unwrap();
        assert!(
            result.status.success(),
            "{argument:?} {}: {}",
            result.status,
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn application_menu_defaults_and_overrides() {
    let engine = apps_runtime::jsc::JsContext::new(
        apps_runtime::jsc::JavaScriptCore::load().unwrap(),
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
    let engine = apps_runtime::jsc::JsContext::new(
        apps_runtime::jsc::JavaScriptCore::load().unwrap(),
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
fn power_save_blocker_and_system_preferences() {
    let engine = apps_runtime::jsc::JsContext::new(
        apps_runtime::jsc::JavaScriptCore::load().unwrap(),
        std::rc::Rc::default(),
        apps_runtime::host::Role::Main,
    )
    .unwrap();
    engine
        .eval(include_str!("../js/browser.js"), "browser")
        .unwrap();
    engine
        .eval(include_str!("native_shell.js"), "native-shell-tests")
        .unwrap();
    engine.tick().unwrap();
    assert_eq!(
        engine
            .eval("nativeShellAsync.sort().join()", "test")
            .unwrap(),
        "Canceled,true"
    );
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
fn page_zoom_shrinks_the_viewport_and_survives_reloads() {
    use std::{
        io::Write,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let main = r#"
        const { app, BrowserWindow, ipcMain } = __apps;
        let win, stage = 0, base;
        const expect = (ok, what) => { if (!ok) { console.error('ZOOM_FAIL ' + what); process.exit(2); } };
        ipcMain.on('size', async (_, width, height, ratio) => {
            if (stage === 0 && width === 400) { base = ratio; stage = 1; win.webContents.zoomFactor = 2; }
            else if (stage === 1 && width !== 400) { expect(width === 200 && height === 150 && ratio === base * 2, 'zoomed ' + [width, height, ratio]); stage = 2; win.webContents.reload(); }
            else if (stage === 2) {
                expect(width === 200 && ratio === base * 2 && win.webContents.getZoomFactor() === 2, 'reloaded ' + [width, height, ratio]);
                stage = 3;
                const size = (await win.webContents.capturePage({ x: 0, y: 0, width: 100, height: 100 })).getSize();
                expect(size.width === 100 * base && size.height === 100 * base, 'capture ' + JSON.stringify(size));
                console.log('ZOOM_OK'); app.quit();
            }
        });
        app.whenReady().then(() => {
            setTimeout(() => process.exit(3), 8000);
            win = new BrowserWindow({ width: 400, height: 300, useContentSize: true });
            win.loadFile('apps:///index.html');
        });
    "#;
    let page = r#"
        <body>
        <script type='preload'>__apps.contextBridge.exposeInMainWorld('report', (...args) => __apps.ipcRenderer.send('size', ...args));</script>
        <script>const send = () => report(innerWidth, innerHeight, devicePixelRatio); send(); addEventListener('resize', send);</script>
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
        output.status.success() && stdout.contains("ZOOM_OK"),
        "{stdout}\n{stderr}"
    );
}

#[cfg(target_os = "macos")]
fn run_window_test(main: &str, page: &str) -> std::process::Output {
    use std::{
        io::Write,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
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
    child.wait_with_output().unwrap()
}

#[test]
#[cfg(target_os = "macos")]
#[ignore = "requires the macOS window server"]
fn native_glass_appearance_preserves_focus() {
    let main = r#"
        const { app, BrowserWindow } = __apps;
        const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
        app.whenReady().then(async () => {
            setTimeout(() => process.exit(3), 8000);
            const owner = new BrowserWindow({ width: 300, height: 200 });
            await owner.loadFile('apps:///index.html');
            await sleep(100);
            if (!owner.isFocused()) throw Error('Owner did not get focus');
            for (const visualEffectState of ['followWindow', 'active', 'inactive']) {
                const float = new BrowserWindow({ width: 260, height: 76, frame: false, transparent: true, focusable: false, show: false, visualEffectState });
                let focused = false;
                float.on('focus', () => { focused = true; });
                await float.loadFile('apps:///index.html');
                for (let i = 0; i < 2; i++) {
                    float.showInactive();
                    await sleep(100);
                    if (!float.isVisible() || float.isFocused() || focused || BrowserWindow.getFocusedWindow() !== owner)
                        throw Error(visualEffectState + ' glass changed keyboard focus');
                    float.hide();
                    await sleep(50);
                }
                const closed = new Promise(resolve => float.once('closed', resolve));
                float.close();
                await closed;
            }
            console.log('GLASS_FOCUS_OK'); app.quit();
        });
    "#;
    let output = run_window_test(
        main,
        r#"<body style="margin:0;background:transparent"><div style="-macos-glass:clear;width:236px;height:52px;border-radius:26px">Glass</div></body>"#,
    );
    let text = String::from_utf8_lossy(&output.stdout) + String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && text.contains("GLASS_FOCUS_OK"),
        "{text}"
    );
}

#[test]
#[cfg(target_os = "macos")]
#[ignore = "requires the macOS window server"]
fn pages_see_the_window_frame_and_levels_apply() {
    let main = r#"
        const { app, BrowserWindow } = __apps;
        app.whenReady().then(async () => {
            setTimeout(() => process.exit(3), 8000);
            const win = new BrowserWindow({ x: 80, y: 90, width: 400, height: 300, show: false });
            await win.loadFile('apps:///index.html');
            const frame = async () => JSON.stringify(await win.webContents.executeJavaScript('[screenX, screenY, screenLeft, screenTop, outerWidth, outerHeight]'));
            const first = await frame();
            const moved = new Promise(resolve => win.once('move', resolve));
            win.setBounds({ x: 120, y: 130, width: 420 });
            await moved;
            const second = await frame();
            if (first !== '[80,90,80,90,400,300]' || second !== '[120,130,120,130,420,300]') throw Error('Frame ' + first + ' ' + second);
            win.setAlwaysOnTop(true, 'status');
            win.setDocumentEdited(true);
            console.log('WINDOW_FRAME_OK'); app.quit();
        });
    "#;
    let output = run_window_test(main, "<body></body>");
    let text = String::from_utf8_lossy(&output.stdout) + String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && text.contains("WINDOW_FRAME_OK"),
        "{text}"
    );
}

#[test]
#[cfg(target_os = "macos")]
#[ignore = "requires the macOS window server"]
fn native_captures_repeat_and_fatal_errors_exit_with_failure() {
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
        let output = run_window_test(main, "<body style='background:red'>x</body>");
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
fn animation_end_handlers_run_before_the_final_frame() {
    use base64::Engine;

    let main = r#"
        const { app, BrowserWindow } = __apps;
        app.whenReady().then(async () => {
            const win = new BrowserWindow({ width: 300, height: 200 });
            await win.loadFile('apps:///index.html');
            await win.webContents.capturePage();
            await win.webContents.executeJavaScript("document.querySelector('div').style.animation = 'exit .3s linear'");
            for (let i = 0; i < 40; i++) {
                const image = await win.webContents.capturePage({ x: 40, y: 40, width: 1, height: 1 });
                console.log('FRAME ' + Buffer.from(image.toPNG()).toString('base64'));
            }
            console.log('ANIMATION_PAINT_OK'); app.quit();
        });
    "#;
    let page = r#"
        <style>
            body { margin:0; background:lime; }
            div { width:100px; height:100px; background:red; }
            @keyframes exit { to { opacity:0; transform:scale(.95); } }
        </style>
        <div></div>
        <script>
            const box = document.querySelector('div');
            box.addEventListener('animationend', () => queueMicrotask(() => box.remove()));
        </script>
    "#;
    let output = run_window_test(main, page);
    let text = String::from_utf8_lossy(&output.stdout) + String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && text.contains("ANIMATION_PAINT_OK"),
        "{text}"
    );
    let red: Vec<_> = text
        .lines()
        .filter_map(|line| line.strip_prefix("FRAME "))
        .map(|encoded| {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .unwrap();
            image::load_from_memory(&bytes)
                .unwrap()
                .to_rgba8()
                .get_pixel(0, 0)[0]
        })
        .collect();
    assert_eq!(red.len(), 40, "{text}");
    assert!(red.iter().any(|&value| value > 0 && value < 250), "{red:?}");
    assert_eq!(red.last(), Some(&0), "{red:?}");
    assert!(
        red.windows(2)
            .all(|pair| pair[1] <= pair[0].saturating_add(2)),
        "The closing popup flashed: {red:?}"
    );
}

#[test]
#[cfg(target_os = "macos")]
#[ignore = "requires the macOS window server"]
fn textarea_content_sizing_follows_text_and_constraints() {
    let main = r#"
        const { app, BrowserWindow, ipcMain } = __apps;
        let finished = false;
        ipcMain.on('done', () => { finished = true; console.log('FIELD_SIZING_OK'); app.quit(); });
        app.whenReady().then(async () => {
            const win = new BrowserWindow({ width: 600, height: 500 });
            await win.loadFile('apps:///index.html');
            while (!finished) await win.webContents.capturePage();
        });
    "#;
    let page = r#"
        <style>
            textarea { display:block; font:16px/20px monospace; padding:6px; border:1px solid; width:200px; box-sizing:border-box; }
            #content { field-sizing:content; }
            section { display:flex; width:240px; }
            section textarea { field-sizing:content; width:100%; }
            aside { width:32px; flex-shrink:0; }
            main { display:grid; grid-template-columns:200px; }
            main textarea { field-sizing:content; width:100%; }
        </style>
        <textarea id=content rows=8 placeholder='Add a comment'></textarea>
        <textarea id=fixed rows=2></textarea>
        <section><aside></aside><textarea id=flex rows=2 placeholder='Add a comment'></textarea></section>
        <main><textarea rows=8>one
two
three</textarea></main>
        <script type='preload'>__apps.contextBridge.exposeInMainWorld('done', () => __apps.ipcRenderer.send('done'));</script>
        <script>
            const frame = () => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
            const field = document.getElementById('content');
            const height = element => element.getBoundingClientRect().height;
            const check = (actual, expected, label) => { if (Math.abs(actual - expected) > 1) throw Error(`${label}: ${actual}, expected ${expected}`); };
            setTimeout(async () => {
                check(height(field), 34, 'placeholder ignores rows');
                check(height(document.getElementById('fixed')), 54, 'fixed keeps rows');
                check(height(document.getElementById('flex')), 34, 'flex percentage width');
                check(height(document.querySelector('main')), 74, 'grid content height');
                field.value = 'one\ntwo\nthree'; await frame();
                check(height(field), 74, 'multiline value');
                field.value = 'word '.repeat(20); await frame();
                if (height(field) <= 74) throw Error('text does not wrap');
                field.style.maxHeight = '64px'; await frame();
                check(height(field), 64, 'maximum height');
                field.style.height = '44px'; await frame();
                check(height(field), 44, 'explicit height');
                field.style.height = ''; field.style.maxHeight = ''; field.value = ''; await frame();
                check(height(field), 34, 'shrinks to placeholder');
                field.removeAttribute('placeholder'); await frame();
                check(height(field), 34, 'empty line');
                field.style.minHeight = '80px'; await frame();
                check(height(field), 80, 'minimum height');
                done();
            }, 200);
        </script>
    "#;
    let output = run_window_test(main, page);
    let text = String::from_utf8_lossy(&output.stdout) + String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && text.contains("FIELD_SIZING_OK"),
        "{text}"
    );
}

#[test]
#[cfg(target_os = "macos")]
#[ignore = "requires the macOS window server"]
fn layout_callbacks_can_request_the_next_frame() {
    let main = r#"
        const { app, BrowserWindow, ipcMain } = __apps;
        ipcMain.on('done', () => { console.log('FRAME_OK'); app.quit(); });
        app.whenReady().then(() => {
            new BrowserWindow({ width: 280, height: 120 }).loadFile('apps:///index.html');
        });
    "#;
    let page = r#"
        <body><div style='height:10px'></div>
        <script type='preload'>__apps.contextBridge.exposeInMainWorld('done', () => __apps.ipcRenderer.send('done'));</script>
        <script>
            const box = document.querySelector('div');
            let frames = 0;
            const observer = new ResizeObserver(() => requestAnimationFrame(() => {
                if (++frames === 4) done();
                else box.style.height = (20 + frames) + 'px';
            }));
            setTimeout(() => { observer.observe(box); box.style.height = '20px'; }, 300);
        </script>
    "#;
    let output = run_window_test(main, page);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("FRAME_OK"));
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

#[test]
#[cfg(target_os = "macos")]
#[ignore = "requires the macOS window server"]
fn native_browser_window_extras_and_child_windows() {
    use std::{
        io::Write,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let lister = r#"
        #import <Foundation/Foundation.h>
        #import <CoreGraphics/CoreGraphics.h>
        int main(int argc, char **argv) {
            NSArray *windows = CFBridgingRelease(CGWindowListCopyWindowInfo(kCGWindowListOptionAll, kCGNullWindowID));
            NSMutableArray *found = [NSMutableArray new];
            for (NSDictionary *window in windows) if ([window[(id)kCGWindowOwnerPID] intValue] == atoi(argv[1]) && [window[(id)kCGWindowLayer] intValue] == 0)
                [found addObject:@{ @"alpha": window[(id)kCGWindowAlpha], @"sharing": window[(id)kCGWindowSharingState], @"width": window[(id)kCGWindowBounds][@"Width"], @"height": window[(id)kCGWindowBounds][@"Height"] }];
            puts([[NSString alloc] initWithData:[NSJSONSerialization dataWithJSONObject:found options:0 error:nil] encoding:NSUTF8StringEncoding].UTF8String);
        }
    "#;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("windows.m"), lister).unwrap();
    let binary = dir.path().join("windows");
    let result = Command::new("xcrun")
        .args([
            "clang",
            "-fobjc-arc",
            "-framework",
            "Foundation",
            "-framework",
            "CoreGraphics",
            "-o",
        ])
        .arg(&binary)
        .arg(dir.path().join("windows.m"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let main = r#"
        const { app, BrowserWindow, child_process } = __apps;
        const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
        const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
        const once = (win, name) => new Promise((resolve, reject) => { const timer = setTimeout(() => reject(Error('No ' + name + ' event')), 5000); win.once(name, () => { clearTimeout(timer); resolve(); }); });
        const expect = (ok, what) => { if (!ok) throw Error(what); };
        const native = (width, height) => new Promise((resolve, reject) => child_process.execFile(process.env.APPS_WINDOW_LIST, [String(process.pid)], (error, stdout) => error ? reject(error) : resolve(JSON.parse(stdout).find(win => win.width === width && win.height === height))));
        const threw = call => { try { call(); return false; } catch { return true; } };
        app.on('window-all-closed', () => {});
        app.whenReady().then(async () => {
            setTimeout(() => { throw Error('Window test timed out'); }, 25000);
            const parent = new BrowserWindow({ x: 100, y: 100, width: 420, height: 320, opacity: 0.5, hasShadow: false, show: false });
            parent.load('<body></body>');
            await once(parent, 'ready-to-show');
            const focus = once(parent, 'focus');
            parent.show();
            await focus;
            await sleep(200);
            let info = await native(420, 320);
            expect(info && Math.abs(info.alpha - 0.5) < 0.01 && info.sharing !== 0 && !parent.hasShadow(), 'opacity option: ' + JSON.stringify(info));
            parent.setOpacity(2);
            parent.setContentProtection(true);
            parent.setVisibleOnAllWorkspaces(true, { skipTransformProcessType: true });
            await sleep(200);
            info = await native(420, 320);
            expect(info.alpha === 1 && info.sharing === 0 && parent.getOpacity() === 1 && parent.isContentProtected() && parent.isVisibleOnAllWorkspaces(), 'setters: ' + JSON.stringify(info));
            parent.setContentProtection(false);
            parent.setVisibleOnAllWorkspaces(false, { skipTransformProcessType: true });
            const normal = parent.getBounds(), resized = once(parent, 'resized');
            parent.maximize();
            await once(parent, 'maximize');
            await resized;
            expect(parent.isMaximized() && same(parent.getNormalBounds(), normal) && !same(parent.getBounds(), normal), 'normal bounds ' + JSON.stringify([normal, parent.getNormalBounds(), parent.getBounds()]));
            const restored = once(parent, 'resized');
            parent.unmaximize();
            await once(parent, 'unmaximize');
            await restored;
            expect(same(parent.getNormalBounds(), parent.getBounds()), 'normal bounds after restore');

            const child = new BrowserWindow({ parent, x: 150, y: 160, width: 200, height: 120, show: false });
            expect(child.getParentWindow() === parent && same(parent.getChildWindows(), [child]), 'child links');
            child.show();
            await once(child, 'show');
            const [x, y] = child.getPosition(), [left, top] = parent.getPosition();
            const moved = once(child, 'move');
            parent.setPosition(left + 40, top + 30);
            await moved;
            expect(same(child.getPosition(), [x + 40, y + 30]), 'child follows its parent: ' + JSON.stringify(child.getPosition()));
            // The window server moves children a moment after the parent, so a child detached at once can still follow.
            await sleep(300);
            child.setParentWindow(null);
            parent.setPosition(left, top);
            await once(parent, 'move');
            await sleep(100);
            expect(same(child.getPosition(), [x + 40, y + 30]) && !parent.getChildWindows().length, 'detached child stays: ' + JSON.stringify([x, y, child.getPosition(), parent.getPosition(), parent.getChildWindows().length]));

            const passive = new BrowserWindow({ x: 400, y: 300, width: 180, height: 110, focusable: false });
            await sleep(300);
            expect(passive.isVisible() && !passive.isFocused() && !passive.isFocusable(), 'focusable option');
            passive.setFocusable(true);
            const focused = once(passive, 'focus');
            passive.focus();
            await focused;

            for (const round of [1, 2]) {
                const sheet = new BrowserWindow({ parent, modal: true, width: 240, height: 140 - round, show: false });
                expect(sheet.isModal() && threw(() => sheet.setParentWindow(null)), 'modal');
                const shown = once(sheet, 'show');
                sheet.show();
                await shown;
                await sleep(400);
                const outer = parent.getBounds(), inner = sheet.getBounds();
                expect(Math.abs(inner.x + inner.width / 2 - (outer.x + outer.width / 2)) <= 1 && inner.y > outer.y, 'sheet ' + round + ' on its parent: ' + JSON.stringify([outer, inner]));
                sheet.close();
                await once(sheet, 'closed');
            }

            child.setParentWindow(parent);
            child.setIgnoreMouseEvents(true, { forward: true });
            child.setIgnoreMouseEvents(false);
            child.setAspectRatio(16 / 9, { width: 10, height: 0 });
            child.setAspectRatio(0);
            child.setProgressBar(0.5);
            child.setProgressBar(2);
            child.setProgressBar(-1);
            child.flashFrame(true);
            child.flashFrame(false);
            child.setSkipTaskbar(true);
            child.setMenu(null);
            child.removeMenu();
            expect(child.setIcon === undefined && threw(() => child.setMenu({})) && threw(() => child.setParentWindow(child)), 'misc');
            const glass = new BrowserWindow({ width: 260, height: 160, vibrancy: 'sidebar', visualEffectState: 'active' });
            glass.load('<body>glass</body>');
            await once(glass, 'ready-to-show');
            glass.setVibrancy('under-window');
            glass.setVibrancy(null);
            glass.close();
            await once(glass, 'closed');
            parent.close();
            await once(parent, 'closed');
            expect(!child.isDestroyed() && child.getParentWindow() === null, 'closing a parent keeps its children');
            console.log('BROWSER_WINDOW_EXTRAS_OK');
            app.quit();
        });
    "#;
    let bundle = serde_json::json!({"files": [
        ["app.js", format!("$appsDefine({});", serde_json::json!({"id":0,"imports":[],"exports":[],"stars":[],"code":main}))],
    ], "pages": []});
    let mut child = Command::new(env!("CARGO_BIN_EXE_apps-runtime"))
        .arg("--bundle-stdin")
        .env("APPS_WINDOW_LIST", &binary)
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
    let deadline = Instant::now() + Duration::from_secs(30);
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
        output.status.success() && stdout.contains("BROWSER_WINDOW_EXTRAS_OK"),
        "{stdout}\n{stderr}"
    );
}
