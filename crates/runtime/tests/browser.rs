#[test]
fn browser_window_rejects_url_loads() {
    use apps_runtime::{
        host::Role,
        jsc::{JavaScriptCore, JsContext},
    };
    let host = std::rc::Rc::new(std::cell::RefCell::new(apps_runtime::host::Host::default()));
    let engine = JsContext::new(JavaScriptCore::load().unwrap(), host.clone(), Role::Main).unwrap();
    assert!(engine.eval("new __apps.WebContentsView()", "test").is_err());
    engine
        .eval(
            "__appsEvent('ready'); globalThis.win = new __apps.BrowserWindow()",
            "test",
        )
        .unwrap();
    host.borrow_mut().commands.clear();
    engine.eval(r#"
        if (__apps.WebContentsView.name !== 'WebContentsView' || 'BrowserView' in __apps) throw Error('View exports');
        if (!(win.contentView instanceof __apps.View) || win.contentView.children.length || 'addBrowserView' in win) throw Error('Window content view');
        __appsEvent('resize', win.id, { width: 1000, height: 700 });
        if (win.contentView.getBounds().width !== 1000 || win.contentView.getBounds().height !== 700) throw Error('Content view follows window size');
        for (const url of ['https://example.com', 'http://localhost', 'file:///page.html', 'apps:///page.html', undefined, { toString() { throw Error('URL conversion'); } }]) {
            let error;
            try { win.loadURL(url); } catch (caught) { error = caught; }
            if (!(error instanceof Error) || error.message !== 'loadURL is not allowed, use WebContentsView to load webpage') throw Error('Window navigation must throw');
        }
    "#, "test").unwrap();
    assert!(host.borrow().commands.is_empty());
    engine
        .eval("win.loadFile('apps:///page.html')", "test")
        .unwrap();
    assert_eq!(
        host.borrow().commands.as_slice(),
        &[serde_json::json!({"op":"window","args":["loadFile",1,"apps:///page.html",1]})]
    );
}

#[test]
fn web_contents_view_matches_electron_api() {
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
        .eval(include_str!("browser.js"), "browser-tests")
        .unwrap();
    for _ in 0..20 {
        engine.tick().unwrap();
        if engine.eval("browserTestsDone", "test").unwrap() == "true" {
            break;
        }
    }
    assert_eq!(engine.eval("browserTestsDone", "test").unwrap(), "true");
}

#[test]
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
#[ignore = "requires a desktop display, JavaScriptCore, and the system webview"]
fn iframe_delivers_load_after_native_layout() {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let output = tempfile::tempdir().unwrap();
    let app = serde_json::json!({"id":0,"imports":[],"exports":[],"stars":[],"commonjs":false,"code":"__apps.app.whenReady().then(()=>{const w=new __apps.BrowserWindow({width:800,height:600});w.loadFile('apps:///index.html');setTimeout(()=>__apps.app.quit(),5000)});"});
    let files = bbtree::unsync::BBTreeMap::from_iter([
        ("app.js".to_owned(), format!("$appsDefine({app});")),
        ("index.html".to_owned(), "<html><body><script type='module'>const frame=document.createElement('iframe');frame.setAttribute('srcdoc','<h1>Native iframe</h1>');frame.style.width='600px';frame.style.height='150px';frame.addEventListener('load',()=>console.log('IFRAME_NATIVE_LOAD',frame.getBoundingClientRect().width,frame.getBoundingClientRect().height));document.body.append(frame);</script></body></html>".to_owned()),
    ]);
    apps_runtime::bundle::write(&output.path().join("app.bundle"), &files).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_apps-runtime"))
        .arg(output.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(12);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            child.kill().unwrap();
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        String::from_utf8_lossy(&result.stdout).contains("IFRAME_NATIVE_LOAD 600 150"),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
}

#[test]
#[ignore = "Requires a desktop display and JavaScriptCore"]
fn page_error_survives_and_native_window_runs_finalizers() {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let output = tempfile::tempdir().unwrap();
    let main = "__apps.app.whenReady().then(()=>{const w=new __apps.BrowserWindow();__apps.ipcMain.on('probe',(_,native,swept)=>{if(w.isDestroyed()||swept<1000)throw Error('GUI cleanup failed');console.log('GUI_PROBE',native,swept);__apps.app.quit()});w.loadFile('apps:///index.html');setTimeout(()=>__apps.app.quit(),5000)})";
    let page = r#"<script type='preload'>__apps.contextBridge.exposeInMainWorld('probe',{done:(a,b)=>__apps.ipcRenderer.send('probe',a,b)})</script><script>throw Error('page failure')</script><script>let native=0,swept=0,turns=0;const registry=new FinalizationRegistry(()=>native++),fallback=new __appsFinalizers(()=>swept++);const timer=setInterval(()=>{for(let i=0;i<10000;i++){const object={data:'x'.repeat(100)};registry.register(object,i);fallback.register(object,i)}if(++turns===40){clearInterval(timer);setTimeout(()=>probe.done(native,swept),100)}},10)</script>"#;
    let files = bbtree::unsync::BBTreeMap::from_iter([
        (
            "app.js".to_owned(),
            format!(
                "$appsDefine({});",
                serde_json::json!({"id":0,"imports":[],"exports":[],"stars":[],"code":main})
            ),
        ),
        ("index.html".to_owned(), page.to_owned()),
    ]);
    apps_runtime::bundle::write(&output.path().join("app.bundle"), &files).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_apps-runtime"))
        .arg(output.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(12);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            child.kill().unwrap();
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let result = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "{stderr}");
    assert!(stdout.contains("GUI_PROBE"), "{stdout} {stderr}");
    assert!(stderr.contains("page failure"), "{stderr}");
}
