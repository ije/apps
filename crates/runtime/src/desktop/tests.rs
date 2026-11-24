use super::*;
use crate::host::link::Target;

impl Desktop {
    fn js(&self) -> &JsContext {
        self.main.as_ref().unwrap().engine().unwrap()
    }

    fn command(&self, command: serde_json::Value) {
        let args = &command["args"];
        match crate::host::window::route(args).unwrap() {
            Target::Main => self.link.main(ToMain::Native(command)),
            Target::Window(id) => self.link.page(ToPage::Window(id, command)),
            Target::All => unreachable!(),
        }
    }

    fn commands(&self) -> Vec<serde_json::Value> {
        let mut commands = self.link.native_commands();
        for message in self.queue.take() {
            if let ToPage::Window(_, command) = &message {
                commands.push(command["args"].clone());
            }
            self.link.page(message);
        }
        commands
    }
}

fn app(code: &str, extra: &[(&str, &str)], root: std::path::PathBuf) -> Desktop {
    let mut files = BBTreeMap::from([(
        "app.js".into(),
        format!(
            "$appsDefine({});",
            serde_json::json!({"id":0,"imports":[],"exports":[],"stars":[],"code":code})
        ),
    )]);
    for (name, source) in extra {
        files.insert((*name).into(), (*source).into());
    }
    let mut desktop = Desktop::create(
        crate::bundle::Bundle {
            files,
            root,
            ..Default::default()
        },
        false,
    )
    .unwrap();
    desktop.js().eval("__appsEvent('ready')", "test").unwrap();
    desktop.pump(None, None).unwrap();
    desktop
}

#[test]
fn navigation_trust_and_optional_preloads() {
    let dir = tempfile::tempdir().unwrap();
    let (root, user) = (dir.path().join("app"), dir.path().join("user"));
    let page = "<script type='preload'>__apps.ipcRenderer.send('pwned')</script><body>safe</body>";
    for directory in [&root, &user] {
        std::fs::create_dir(directory).unwrap();
        std::fs::write(directory.join("page.html"), page).unwrap();
    }
    let code = format!(
        "globalThis.pwned=0;__apps.ipcMain.on('pwned',()=>pwned++);__apps.app.whenReady().then(()=>{{globalThis.win=new __apps.BrowserWindow();win.load({})}})",
        serde_json::json!(page)
    );
    let mut desktop = app(&code, &[("bundled.html", page)], root.clone());
    let step = |desktop: &mut Desktop, script: &str, preload: bool, pwned: &str| {
        if !script.is_empty() {
            desktop.js().eval(script, "test").unwrap();
            desktop.pump(None, None).unwrap();
        }
        desktop.js().tick().unwrap();
        assert_eq!(desktop.views[&1].preload.is_some(), preload, "{script}");
        assert_eq!(
            desktop.js().eval("pwned", "test").unwrap(),
            pwned,
            "{script}"
        );
    };
    step(&mut desktop, "", false, "0");
    desktop
        .js()
        .eval("win.webContents.send('unused',42)", "test")
        .unwrap();
    let url = url::Url::from_file_path(root.join("page.html")).unwrap();
    desktop.views[&1]
        .renderer
        .as_ref()
        .unwrap()
        .eval(
            &format!("location.href={}", serde_json::json!(url.as_str())),
            "test",
        )
        .unwrap();
    desktop.pump(None, None).unwrap();
    step(&mut desktop, "", false, "0");
    let load = |path: &std::path::Path| format!("win.loadFile({})", serde_json::json!(path));
    step(&mut desktop, &load(&user.join("page.html")), false, "0");
    step(
        &mut desktop,
        &load(&root.join("../user/page.html")),
        false,
        "0",
    );
    step(&mut desktop, &load(&root.join("page.html")), true, "1");
    step(&mut desktop, "win.loadFile('page.html')", true, "2");
    assert_eq!(
        desktop.views[&1]
            .renderer
            .as_ref()
            .unwrap()
            .eval("location.href", "test")
            .unwrap(),
        url.as_str()
    );
    step(
        &mut desktop,
        "win.loadFile('apps:///bundled.html')",
        true,
        "3",
    );
    step(&mut desktop, "win.reload()", true, "4");
}

#[test]
fn loads_return_promises_and_failures_never_quit() {
    let code = r#"
        globalThis.log = [];
        __apps.app.whenReady().then(() => {
            globalThis.win = new __apps.BrowserWindow();
            for (const name of ['did-start-loading', 'did-navigate', 'dom-ready', 'did-finish-load', 'did-stop-loading', 'did-fail-load', 'destroyed'])
                win.webContents.on(name, (event, ...args) => log.push(name + (name === 'did-fail-load' || name === 'did-navigate' ? ':' + args.slice(0, 3).join(' ') : '')));
            win.loadFile('missing.html').then(() => log.push('resolved'), error => log.push('rejected:' + [error.message, error.code, error.errno].join(' ')));
        });
    "#;
    let mut desktop = app(code, &[], std::path::PathBuf::from("/app"));
    let eval = |desktop: &Desktop, script: &str| desktop.js().eval(script, "test").unwrap();
    assert!(!desktop.quit && desktop.views.contains_key(&1));
    assert_eq!(
        eval(&desktop, "log.join('|')"),
        "did-fail-load:-6 ERR_FILE_NOT_FOUND file:///app/missing.html|did-stop-loading|rejected:ERR_FILE_NOT_FOUND (-6) loading 'file:///app/missing.html' ERR_FILE_NOT_FOUND -6"
    );
    eval(
        &desktop,
        "log.length = 0; win.load('<p>first</p>').catch(error => log.push(error.code)); win.load('<title>T</title><p>second</p>').then(() => log.push('resolved:' + win.webContents.getURL() + ':' + win.webContents.isLoading()))",
    );
    assert_eq!(
        eval(&desktop, "log.join('|')+':'+win.webContents.isLoading()"),
        "ERR_ABORTED:true"
    );
    desktop.pump(None, None).unwrap();
    assert_eq!(
        eval(&desktop, "log.slice(1).join('|')"),
        "did-start-loading|did-navigate:apps://app/ -1 |dom-ready|did-finish-load|did-stop-loading|did-start-loading|did-navigate:apps://app/ -1 |dom-ready|did-finish-load|did-stop-loading|resolved:apps://app/:false"
    );
    eval(
        &desktop,
        "globalThis.results = []; for (const code of ['document.title', 'new Promise(r => setTimeout(() => r({list: [1, 2]}), 0))', 'throw new TypeError(\"inner\")', 'undefined']) win.webContents.executeJavaScript(code).then(v => results.push(JSON.stringify(v) ?? 'undefined'), e => results.push('error:' + e.message))",
    );
    desktop.pump(None, None).unwrap();
    for engine in [desktop.js(), desktop.views[&1].renderer.as_deref().unwrap()] {
        engine.tick().unwrap();
    }
    desktop.pump(None, None).unwrap();
    assert_eq!(
        eval(&desktop, "results.join('|')"),
        r#""T"|error:inner|undefined|{"list":[1,2]}"#
    );
    assert!(
        desktop.views[&1]
            .renderer
            .as_ref()
            .unwrap()
            .eval("eval('1')", "test")
            .is_err(),
        "executeJavaScript must not unlock eval for the page"
    );
    assert!(
        eval(
            &desktop,
            "try { win.webContents.executeJavaScript('1'); win.load(1); 'ok' } catch (e) { e.message }"
        ) == "ok"
    );
    assert!(!desktop.quit);
}

#[test]
fn fatal_errors_quit_with_a_failure_code() {
    let mut desktop = app("", &[], std::path::PathBuf::new());
    desktop.fail();
    assert!(desktop.quit && desktop.link.exit_code() == 1);
    desktop.link.quit(3);
    desktop.fail();
    assert_eq!(desktop.link.exit_code(), 3);
}

#[test]
fn destroy_is_synchronous_and_web_contents_are_real() {
    let code = r#"
        const {app, BrowserWindow, webContents} = __apps;
        globalThis.log = [];
        app.on('window-all-closed', () => log.push('all-closed'));
        app.whenReady().then(() => {
            globalThis.win = new BrowserWindow();
            globalThis.other = new BrowserWindow();
            for (const name of ['close', 'closed']) win.on(name, () => log.push(name));
            win.webContents.on('destroyed', () => log.push('destroyed'));
            globalThis.ids = [win.id, win.webContents.id, other.webContents.id, webContents.fromId(other.webContents.id) === other.webContents, webContents.getAllWebContents().length, BrowserWindow.fromWebContents(win.webContents) === win].join();
            win.destroy();
            log.push(win.isDestroyed(), win.webContents.isDestroyed(), BrowserWindow.getAllWindows().length);
            for (const call of [() => win.setTitle('x'), () => win.close(), () => win.webContents.send('x'), () => win.loadFile('x.html')]) try { call(); log.push('no throw'); } catch (error) { log.push(error.message); }
            win.destroy();
        });
    "#;
    let desktop = app(code, &[], std::path::PathBuf::new());
    assert_eq!(
        desktop.js().eval("ids", "test").unwrap(),
        "1,1,2,true,2,true"
    );
    assert_eq!(
        desktop.js().eval("log.join('|')", "test").unwrap(),
        "destroyed|closed|true|true|1|Object has been destroyed|Object has been destroyed|Object has been destroyed|Object has been destroyed"
    );
    assert!(!desktop.views.contains_key(&1) && desktop.views.contains_key(&2));
    desktop.js().eval("other.destroy()", "test").unwrap();
    assert_eq!(
        desktop.js().eval("log.at(-1)", "test").unwrap(),
        "all-closed"
    );
}

#[test]
fn page_navigation_stays_local_and_never_quits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("page.html");
    std::fs::write(&path, "<script type='application/json'>{}</script><script type='text/javascript'>globalThis.ran=1</script><body>ok</body>").unwrap();
    let mut desktop = app(
        "__apps.app.whenReady().then(()=>{globalThis.win=new __apps.BrowserWindow();win.load('<body>first</body>')})",
        &[],
        std::path::PathBuf::new(),
    );
    let navigate = |desktop: &Desktop, url: &str| {
        desktop
            .host
            .borrow_mut()
            .call(Role::Renderer(1), "dom.navigate", serde_json::json!([url]))
    };
    for url in [
        "https://example.com/",
        "apps:/../../x.html",
        "file://host/share/x.html",
    ] {
        assert!(navigate(&desktop, url).is_err(), "{url}");
    }
    navigate(&desktop, "file:///missing/page.html").unwrap();
    desktop.pump(None, None).unwrap();
    assert!(!desktop.quit);
    let url = url::Url::from_file_path(&path).unwrap();
    navigate(&desktop, url.as_str()).unwrap();
    desktop.pump(None, None).unwrap();
    let renderer = desktop.views[&1].renderer.clone().unwrap();
    assert_eq!(renderer.eval("globalThis.ran", "test").unwrap(), "1");
    assert!(desktop.views[&1].preload.is_none());
}

#[test]
fn check_reports_page_script_errors() {
    for (script, fails) in [
        ("throw Error('page failure')", true),
        ("reportError(Error('page failure'))", true),
        (
            "queueMicrotask(() => { throw Error('page failure'); })",
            true,
        ),
        (
            "addEventListener('error', event => event.preventDefault()); reportError(Error('page failure'));",
            false,
        ),
        ("console.error('page failure')", false),
    ] {
        let files = BBTreeMap::from([(
            "app.js".into(),
            format!(
                "$appsDefine({});",
                serde_json::json!({"id":0,"imports":[],"exports":[],"stars":[],"code":format!("__apps.app.whenReady().then(()=>new __apps.BrowserWindow().load({}))", serde_json::json!(format!("<script>{script}</script>")))})
            ),
        )]);
        let result = Desktop::new(&files).unwrap().check();
        if fails {
            assert!(
                result.unwrap_err().to_string().contains("page failure"),
                "{script}"
            );
        } else {
            assert_eq!(result.unwrap(), 0, "{script}");
        }
    }
}

#[test]
fn window_errors_are_synchronous_and_closed_windows_are_ignored() {
    let code = "__apps.app.whenReady().then(()=>{try{new __apps.BrowserWindow({unknownOption:1})}catch(e){globalThis.rejected=e.message}globalThis.win=new __apps.BrowserWindow();win.close()})";
    let mut desktop = app(code, &[], std::path::PathBuf::new());
    assert_eq!(
        desktop
            .js()
            .eval(
                "rejected+':'+__apps.BrowserWindow.getAllWindows().length+':'+win.isDestroyed()",
                "test"
            )
            .unwrap(),
        "Unsupported BrowserWindow option: unknownOption:0:true"
    );
    assert!(desktop.js().eval("win.show()", "test").is_err());
    desktop.command(serde_json::json!({"op":"window","args":["title",2,"stale"]}));
    desktop.pump(None, None).unwrap();
}

#[test]
fn pages_cannot_settle_other_windows_and_closed_errors_never_quit() {
    let code = r#"
        const {app, BrowserWindow} = __apps;
        globalThis.log = [];
        process.on('uncaughtException', error => log.push(error.message));
        app.on('window-all-closed', () => log.push('all-closed'));
        app.whenReady().then(() => {
            globalThis.a = new BrowserWindow({title: 'a\ud83d'});
            globalThis.b = new BrowserWindow();
            a.load('<p>a</p>');
            b.load('<p>b</p>');
            a.on('closed', () => { throw new Error('closed failed'); });
        });
    "#;
    let mut desktop = app(code, &[], std::path::PathBuf::new());
    let eval = |desktop: &Desktop, script: &str| desktop.js().eval(script, "test").unwrap();
    eval(
        &desktop,
        "globalThis.results = []; for (const contents of [a.webContents, a.webContents]) contents.executeJavaScript('new Promise(() => {})').then(value => results.push(value)); a.webContents.capturePage().then(value => results.push(typeof value), error => results.push(error.message))",
    );
    desktop.pump(None, None).unwrap();
    let page = |id: u64| desktop.views[&id].renderer.clone().unwrap();
    page(2)
        .eval("__appsExecuted(1, 'forged', false)", "test")
        .unwrap();
    page(1)
        .eval("__appsExecuted(3, 'capture', false)", "test")
        .unwrap();
    page(1)
        .eval("__appsExecuted(2, 'own', false)", "test")
        .unwrap();
    desktop.pump(None, None).unwrap();
    desktop.js().tick().unwrap();
    assert_eq!(eval(&desktop, "results.join()"), "own");
    assert_eq!(eval(&desktop, "a.getTitle()"), "a\u{fffd}");
    eval(&desktop, "a.close(); b.close()");
    desktop.pump(None, None).unwrap();
    assert_eq!(eval(&desktop, "log.join('|')"), "closed failed|all-closed");
    assert!(!desktop.quit);
}

#[test]
fn app_lifecycle_follows_electron() {
    let code = r#"
        const {app, BrowserWindow} = __apps;
        globalThis.log = [];
        for (const name of ['will-finish-launching', 'ready', 'web-contents-created', 'browser-window-created', 'browser-window-focus', 'activate', 'open-url', 'open-file', 'window-all-closed', 'will-quit', 'quit'])
            app.on(name, (_event, value) => log.push(value === undefined || typeof value === 'object' ? name : name + ':' + value));
        let blocked = true;
        app.on('before-quit', event => { log.push('before-quit'); if (blocked) event.preventDefault(); blocked = false; });
        app.commandLine.appendSwitch('flag', 'on');
        app.setPath('userData', app.getPath('temp'));
        globalThis.state = [app.getPath('sessionData') === app.getPath('temp'), !!app.getAppPath(), app.commandLine.getSwitchValue('flag'), app.name].join();
        app.whenReady().then(() => new BrowserWindow());
    "#;
    let files = BBTreeMap::from([(
        "app.js".into(),
        format!(
            "$appsDefine({});",
            serde_json::json!({"id":0,"imports":[],"exports":[],"stars":[],"code":code})
        ),
    )]);
    let mut desktop = Desktop::new(&files).unwrap();
    desktop.js().eval("__appsEvent('ready')", "test").unwrap();
    desktop.js().eval("__appsEvent('state',1,{frame:[0,0,800,600],content:[0,28,800,572],minimized:false,maximized:false,fullscreen:false,visible:true,focused:true,resizing:false});__appsEvent('activate',0,true);__appsEvent('open-url',0,'myapp://x');__appsEvent('open-file',0,'/tmp/a');__appsEvent('before-quit')", "test").unwrap();
    desktop.pump(None, None).unwrap();
    assert!(!desktop.quit);
    desktop
        .js()
        .eval("__appsEvent('before-quit')", "test")
        .unwrap();
    desktop.pump(None, None).unwrap();
    assert!(desktop.quit);
    assert_eq!(
        desktop.js().eval("state", "test").unwrap(),
        "true,true,on,App"
    );
    assert_eq!(
        desktop.js().eval("log.join()", "test").unwrap(),
        "will-finish-launching,ready,web-contents-created,browser-window-created,browser-window-focus,activate:true,open-url:myapp://x,open-file:/tmp/a,before-quit,before-quit,will-quit,quit:0"
    );
    for (listener, quits) in [
        ("", true),
        ("__apps.app.on('window-all-closed',()=>{});", false),
    ] {
        let code = format!(
            "{listener}__apps.app.whenReady().then(()=>new __apps.BrowserWindow().close())"
        );
        let desktop = app(&code, &[], std::path::PathBuf::new());
        assert_eq!(desktop.quit, quits);
    }
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "only macOS moves windows")]
fn window_state_events_and_close_follow_electron() {
    let code = r#"
        const {app, BrowserWindow} = __apps;
        globalThis.log = [];
        app.on('window-all-closed', () => {});
        app.whenReady().then(() => {
            globalThis.win = new BrowserWindow({width: 400, height: 300, minWidth: 200, minHeight: 100, title: 'Start', backgroundColor: '#80112233'});
            for (const name of ['resize', 'resized', 'move', 'moved', 'focus', 'blur', 'maximize', 'unmaximize', 'minimize', 'restore', 'enter-full-screen', 'leave-full-screen', 'show', 'hide', 'close', 'closed', 'page-title-updated'])
                win.on(name, () => log.push(name));
            win.on('page-title-updated', event => event.preventDefault());
            win.on('close', event => { if (!globalThis.allow) event.preventDefault(); });
            win.load('<title>Page</title>');
        });
    "#;
    let mut desktop = app(code, &[], std::path::PathBuf::new());
    let eval = |desktop: &Desktop, script: &str| desktop.js().eval(script, "test").unwrap();
    assert_eq!(
        eval(
            &desktop,
            "JSON.stringify([win.getBounds(), win.getTitle(), log])"
        ),
        r#"[{"x":0,"y":0,"width":400,"height":300},"Start",["page-title-updated"]]"#
    );
    assert_eq!(desktop.views[&1].title.as_deref(), Some("Page"));
    assert_eq!(desktop.host.borrow().backgrounds[&1], (0x112233, 0x80));
    let state = |frame: [i32; 4], extra: &str| {
        let [x, y, width, height] = frame;
        format!(
            "__appsEvent('state',1,{{frame:[{x},{y},{width},{height}],content:[{x},{},{width},{}],minimized:false,maximized:false,fullscreen:false,visible:true,focused:true,resizing:false,{extra}}})",
            y + 28,
            height - 28
        )
    };
    for (script, events) in [
        (state([10, 20, 400, 300], ""), "focus"),
        (state([10, 20, 500, 300], "resizing:true"), "resize"),
        (state([10, 20, 500, 300], ""), "resized"),
        (state([30, 40, 500, 300], ""), "move,moved"),
        (
            state([0, 0, 900, 700], "maximized:true"),
            "maximize,resize,resized,move,moved",
        ),
        (
            state(
                [0, 0, 900, 700],
                "minimized:true,visible:false,focused:false",
            ),
            "minimize,unmaximize,hide,blur",
        ),
        (
            state([0, 0, 900, 700], "fullscreen:true"),
            "restore,enter-full-screen,show,focus",
        ),
    ] {
        eval(&desktop, &format!("log.length = 0; {script}"));
        assert_eq!(eval(&desktop, "log.join()"), events, "{script}");
    }
    assert_eq!(
        eval(
            &desktop,
            "JSON.stringify([win.getBounds(), win.getContentSize(), win.isFullScreen(), win.isNormal(), win.isFocused(), __apps.BrowserWindow.getFocusedWindow() === win, win.contentView.getBounds()])"
        ),
        r#"[{"x":0,"y":0,"width":900,"height":700},[900,672],true,false,true,true,{"x":0,"y":0,"width":900,"height":672}]"#
    );
    desktop.queue.take();
    assert_eq!(
        eval(
            &desktop,
            "win.setSize(50.4, 50); win.setContentBounds({y: 100}); JSON.stringify([win.getBounds(), win.getContentBounds()])"
        ),
        r#"[{"x":0,"y":72,"width":200,"height":100},{"x":0,"y":100,"width":200,"height":72}]"#
    );
    assert_eq!(
        desktop.commands()[1],
        serde_json::json!(["bounds", 1, 0, 72, 200, 100])
    );
    for script in [
        "win.setSize('x', 1)",
        "win.setMinimumSize(-1, 0)",
        "win.setBackgroundColor('nope')",
        "new __apps.BrowserWindow({frame: 'no'})",
        "new __apps.BrowserWindow({x: Infinity})",
    ] {
        assert_eq!(
            eval(
                &desktop,
                &format!("try {{ {script}; 'ok' }} catch {{ 'threw' }}")
            ),
            "threw",
            "{script}"
        );
    }
    eval(&desktop, "__apps.app.quit()");
    eval(&desktop, "__appsEvent('close',1)");
    desktop.pump(None, None).unwrap();
    assert!(!desktop.quit && desktop.views.contains_key(&1));
    assert_eq!(
        eval(&desktop, "log.filter(name => name === 'close').length"),
        "2"
    );
    eval(&desktop, "allow = true; __apps.app.quit()");
    desktop.pump(None, None).unwrap();
    assert!(desktop.quit && !desktop.views.contains_key(&1));
    assert_eq!(eval(&desktop, "log.slice(-2).join()"), "close,closed");
}

#[test]
#[cfg_attr(
    not(target_os = "macos"),
    ignore = "uses window methods that only macOS has"
)]
fn window_extras_follow_electron() {
    let code = r#"
        const {app, BrowserWindow} = __apps;
        app.on('window-all-closed', () => {});
        app.whenReady().then(() => {
            globalThis.parent = new BrowserWindow({width: 400, height: 300, opacity: 3, vibrancy: 'sidebar', hasShadow: false, focusable: false, skipTaskbar: true, icon: 'icon.png'});
            globalThis.child = new BrowserWindow({parent, show: false});
            globalThis.sheet = new BrowserWindow({parent, modal: true});
        });
    "#;
    let mut desktop = app(code, &[], std::path::PathBuf::new());
    let eval = |desktop: &Desktop, script: &str| desktop.js().eval(script, "test").unwrap();
    let thrown = |desktop: &Desktop, script: &str| {
        eval(
            desktop,
            &format!(
                "(() => {{ try {{ {script}; return 'ok' }} catch (error) {{ return error.message }} }})()"
            ),
        )
    };
    assert_eq!(
        eval(
            &desktop,
            "JSON.stringify([parent.getOpacity(), parent.hasShadow(), parent.isFocusable(), parent.getChildWindows().map(win => win.id), child.getParentWindow() === parent, sheet.isModal(), child.isModal(), parent.getParentWindow(), parent.setIcon])"
        ),
        "[1,false,false,[2,3],true,true,false,null,null]"
    );
    if cfg!(target_os = "macos") {
        assert_eq!(desktop.host.borrow().backgrounds[&1], (0, 0));
    }
    for (script, error) in [
        (
            "sheet.setParentWindow(null)",
            "Can not be called for modal window",
        ),
        (
            "parent.setParentWindow(child)",
            "A window cannot be its own ancestor",
        ),
        (
            "child.setParentWindow({})",
            "Expected a BrowserWindow or null",
        ),
        ("child.setOpacity(NaN)", "Expected a number for opacity"),
        ("child.setMenu({})", "Invalid Menu"),
        (
            "new __apps.BrowserWindow({vibrancy: 'nope'})",
            "Invalid vibrancy type: nope",
        ),
        (
            "new __apps.BrowserWindow({visualEffectState: 'x'})",
            "Invalid visualEffectState: x",
        ),
        (
            "new __apps.BrowserWindow({opacity: '1'})",
            "Invalid BrowserWindow option opacity: \"1\"",
        ),
        (
            "new __apps.BrowserWindow({parent: {}})",
            "Expected a BrowserWindow for parent",
        ),
    ] {
        assert_eq!(thrown(&desktop, script), error, "{script}");
    }
    let state = |frame: [i32; 4], extra: &str| {
        let [x, y, width, height] = frame;
        format!(
            "__appsEvent('state',1,{{frame:[{x},{y},{width},{height}],content:[{x},{y},{width},{height}],minimized:false,maximized:false,fullscreen:false,visible:true,focused:true,resizing:false,{extra}}})"
        )
    };
    eval(&desktop, &state([10, 20, 400, 300], ""));
    eval(&desktop, &state([0, 0, 900, 700], "maximized:true"));
    assert_eq!(
        eval(
            &desktop,
            "JSON.stringify([parent.getNormalBounds(), parent.getBounds()])"
        ),
        r#"[{"x":10,"y":20,"width":400,"height":300},{"x":0,"y":0,"width":900,"height":700}]"#
    );
    eval(&desktop, &state([0, 0, 900, 700], ""));
    desktop.queue.take();
    eval(
        &desktop,
        "parent.setOpacity(0.5); parent.setAspectRatio(-1, {width: 5}); parent.setVibrancy(''); parent.setSkipTaskbar(true); parent.removeMenu(); parent.close()",
    );
    let commands = desktop.commands();
    assert_eq!(
        serde_json::Value::from(commands),
        serde_json::json!([
            ["opacity", 1, 0.5],
            ["aspectRatio", 1, 0, 5, 0],
            ["parent", 2, null],
            ["parent", 3, null],
            ["close", 1],
            ["vibrancy", 1, null, 0],
            ["background", 1, "#fff"]
        ])
    );
    desktop.pump(None, None).unwrap();
    assert_eq!(
        eval(
            &desktop,
            "JSON.stringify([parent.isDestroyed(), child.isDestroyed(), child.getParentWindow()])"
        ),
        "[true,false,null]"
    );
    assert_eq!(
        eval(
            &desktop,
            "globalThis.seen = []; child.on('always-on-top-changed', (event, on) => seen.push(on)).on('will-resize', (event, bounds, details) => { seen.push(bounds.width, details.edge); event.preventDefault(); }); for (const args of [[true, 'screen-saver', 1], [true], [true, 'normal'], [false], [true, 'toString']]) child.setAlwaysOnTop(...args); child.setDocumentEdited(true); seen.push(__appsWillChange(2, ['right', 2, 2, 30, 40]), child.isDocumentEdited(), child.isAlwaysOnTop()); child.removeAllListeners('will-resize'); JSON.stringify(seen)"
        ),
        r#"[true,false,30,"right",true,true,false]"#
    );
    assert_eq!(
        serde_json::Value::from(desktop.commands()),
        serde_json::json!([
            ["willChange", 2, true],
            ["alwaysOnTop", 2, 1001],
            ["alwaysOnTop", 2, 3],
            ["alwaysOnTop", 2, 0],
            ["alwaysOnTop", 2, 0],
            ["alwaysOnTop", 2, 0],
            ["edited", 2, true],
            ["willChange", 2, false]
        ])
    );
}

#[test]
fn window_titles_follow_the_document() {
    for (html, title) in [
        ("<h1>Page</h1>", None),
        (
            "<html><head><title> Initial   title </title></head><body></body></html>",
            Some("Initial title"),
        ),
        (
            "<title>Initial</title><script>document.title = 'Startup'</script>",
            Some("Startup"),
        ),
    ] {
        let code = format!(
            "__apps.app.whenReady().then(()=>{{new __apps.BrowserWindow().load({});new __apps.BrowserWindow().load('<title>Other</title>')}})",
            serde_json::json!(html)
        );
        let mut desktop = app(&code, &[], std::path::PathBuf::new());
        assert_eq!(desktop.views[&1].title.as_deref(), title);
        let renderer = desktop.views[&1].renderer.clone().unwrap();
        for (script, title) in [
            ("document.title = 'New title'", Some("New title")),
            (
                "document.querySelector('title').textContent = 'DOM title'",
                Some("DOM title"),
            ),
            (
                "document.querySelector('title').firstChild.data = 'Text title'",
                Some("Text title"),
            ),
            (
                "document.title = '  Spaced \\n title  '",
                Some("Spaced title"),
            ),
            (
                "document.implementation.createHTMLDocument('Detached')",
                Some("Spaced title"),
            ),
            ("document.querySelector('title').remove()", None),
            ("document.title = 'Restored'", Some("Restored")),
            ("document.title = ''", Some("")),
        ] {
            renderer.eval(script, "test").unwrap();
            desktop.pump(None, None).unwrap();
            assert_eq!(desktop.views[&1].title.as_deref(), title, "{script}");
            assert_eq!(desktop.views[&2].title.as_deref(), Some("Other"));
            assert_eq!(
                renderer.eval("document.title", "test").unwrap(),
                title.unwrap_or("")
            );
        }
        renderer
            .eval(
                "getComputedStyle(document.querySelector('title')).color",
                "test",
            )
            .unwrap();
        assert!(
            desktop.host.borrow().pages[&1]
                .state
                .styles
                .computed
                .is_some()
        );
        desktop.command(serde_json::json!({
            "op":"window", "args":["load",1,"<html><body style='color:purple'>New page</body></html>"]
        }));
        desktop.pump(None, None).unwrap();
        let renderer = desktop.views[&1].renderer.as_ref().unwrap();
        assert_eq!(
            renderer
                .eval("getComputedStyle(document.body).color", "test")
                .unwrap(),
            "rgb(128, 0, 128)"
        );
    }
}

#[test]
fn web_contents_zoom_css_and_edit_commands() {
    let page = "<style>p{color:red;margin:1px}</style><p id=p>x</p><input id=i value=hello><div id=e contenteditable>ab</div>";
    let code = format!(
        "__apps.app.whenReady().then(()=>{{globalThis.win=new __apps.BrowserWindow();win.load({})}})",
        serde_json::json!(page)
    );
    let mut desktop = app(&code, &[], std::path::PathBuf::new());
    let main = |desktop: &mut Desktop, script: &str| {
        let value = desktop.js().eval(script, "test").unwrap();
        desktop.js().tick().unwrap();
        desktop.pump(None, None).unwrap();
        value
    };
    let page = |desktop: &Desktop, script: &str| {
        let renderer = desktop.views[&1].renderer.as_ref().unwrap();
        renderer.eval(script, "test").unwrap()
    };
    assert_eq!(
        main(
            &mut desktop,
            "win.webContents.zoomLevel=1;[win.webContents.getZoomFactor(),win.webContents.zoomLevel,(win.webContents.setZoomFactor(1.5),win.webContents.zoomFactor),win.webContents.getZoomLevel().toFixed(3)].join()"
        ),
        "1.2,1,1.5,2.224"
    );
    assert!(
        main(
            &mut desktop,
            "try{win.webContents.setZoomFactor(0)}catch(e){e.message}"
        )
        .contains("greater than 0")
    );
    let style = "(s=>[s.color,s.marginTop,s.paddingTop].join())(getComputedStyle(document.getElementById('p')))";
    main(
        &mut desktop,
        "win.webContents.insertCSS('p{color:blue!important;margin:2px}',{cssOrigin:'user'}).then(key=>globalThis.user=key);win.webContents.insertCSS('p{padding:3px}')",
    );
    assert_eq!(page(&desktop, style), "rgb(0, 0, 255),1px,3px");
    main(&mut desktop, "win.webContents.removeInsertedCSS(user)");
    assert_eq!(page(&desktop, style), "rgb(255, 0, 0),1px,3px");
    main(&mut desktop, "win.reload()");
    assert_eq!(page(&desktop, style), "rgb(255, 0, 0),1px,0px");
    let field =
        "(i=>[i.value,i.selectionStart,i.selectionEnd].join())(document.getElementById('i'))";
    page(
        &desktop,
        "document.getElementById('i').focus();document.getElementById('i').setSelectionRange(5,5)",
    );
    main(
        &mut desktop,
        "const contents=win.webContents;contents.insertText(' world');contents.selectAll();contents.replace('hi');contents.undo();contents.undo();contents.redo();contents.delete()",
    );
    assert_eq!(page(&desktop, field), "hello worl,10,10");
    main(&mut desktop, "contents.selectAll();contents.unselect()");
    assert_eq!(page(&desktop, field), "hello worl,10,10");
    assert_eq!(
        page(
            &desktop,
            "(i=>[document.queryCommandSupported('insertText'),document.queryCommandSupported('bold'),document.execCommand('insertText',false,'!'),i.value,document.execCommand('undo'),i.value,document.execCommand('redo'),i.value,document.queryCommandEnabled('copy'),document.execCommand('copy'),document.execCommand('delete'),i.value].join())(document.getElementById('i'))"
        ),
        "true,false,true,hello worl!,true,hello worl,true,hello worl!,false,false,true,hello worl"
    );
    assert_eq!(
        page(
            &desktop,
            "(e=>{e.focus();getSelection().collapse(e.firstChild,2);document.execCommand('insertText',false,'c');const typed=e.textContent;document.execCommand('undo');return typed+','+e.textContent})(document.getElementById('e'))"
        ),
        "abc,ab"
    );
}

#[test]
fn window_open_asks_the_open_handler() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("child.html"), "<title>child</title>").unwrap();
    std::fs::write(
        dir.path().join("index.html"),
        "<a id=a href='child.html' target='_blank'>x</a>",
    )
    .unwrap();
    let code = r#"
        globalThis.log = [];
        __apps.app.whenReady().then(() => {
            globalThis.win = new __apps.BrowserWindow();
            win.webContents.on('did-create-window', (child, details) => log.push('created:' + child.id + ':' + details.disposition + ':' + details.options.width));
            win.loadFile('index.html');
        });
    "#;
    let mut desktop = app(code, &[], dir.path().to_path_buf());
    let page = |desktop: &mut Desktop, script: &str| {
        let renderer = desktop.views[&1].renderer.clone().unwrap();
        let value = renderer.eval(script, "test").unwrap();
        desktop.pump(None, None).unwrap();
        value
    };
    assert_eq!(
        page(
            &mut desktop,
            "globalThis.child=open('child.html','','width=300,height=200');[open('https://example.com/'),child===null,child.closed].join()"
        ),
        ",false,false"
    );
    assert!(desktop.views[&2].renderer.is_some());
    desktop
        .js()
        .eval(
            "win.webContents.setWindowOpenHandler(details => { log.push([details.url.endsWith('/child.html'), details.frameName, details.features, details.disposition, details.referrer.url.endsWith('/index.html')].join()); return details.frameName === 'deny' ? { action: 'deny' } : { action: 'allow', overrideBrowserWindowOptions: { width: 320 } }; })",
            "test",
        )
        .unwrap();
    assert_eq!(
        page(
            &mut desktop,
            "[open('child.html','deny'),document.getElementById('a').click(),child.close()].join()"
        ),
        ",,"
    );
    assert!(desktop.views.contains_key(&3) && !desktop.views.contains_key(&2));
    assert_eq!(page(&mut desktop, "child.closed"), "true");
    assert_eq!(
        desktop.js().eval("log.join('|')", "test").unwrap(),
        "created:2:new-window:300|true,deny,,foreground-tab,true|true,_blank,,foreground-tab,true|created:3:foreground-tab:320"
    );
}

#[test]
fn dialogs_of_main_and_pages_show_a_message_box() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), "<p>x</p>").unwrap();
    let code =
        "__apps.app.whenReady().then(() => new __apps.BrowserWindow().loadFile('index.html'));";
    let desktop = app(code, &[], dir.path().to_path_buf());
    crate::host::context::NATIVE.set(Some(|op, args| {
        assert_eq!(op, "dialog.message");
        let buttons = args["buttons"].as_array().map_or(0, Vec::len);
        Ok(serde_json::json!({
            "response": u8::from(args["message"] == "no"),
            "input": format!("{}:{buttons}", args["input"].as_str().unwrap_or("-")),
        }))
    }));
    let script = "[alert('hi'),confirm('yes'),confirm('no'),prompt('name','x'),prompt('no','x'),prompt()].join()";
    let page = desktop.views[&1].renderer.clone().unwrap();
    for engine in [&*page, desktop.js()] {
        assert_eq!(engine.eval(script, "test").unwrap(), ",true,false,x:2,,:2");
    }
}

#[test]
fn main_runs_before_input_event_only_while_it_listens() {
    let code = "__apps.app.whenReady().then(()=>{globalThis.win=new __apps.BrowserWindow();globalThis.contents=win.webContents;globalThis.f=e=>e.preventDefault();globalThis.calls=0;const real=__appsBeforeInput;globalThis.__appsBeforeInput=(...args)=>(calls++,real(...args));win.load('<body></body>')})";
    let mut desktop = app(code, &[], std::path::PathBuf::new());
    let main = desktop.main.clone().unwrap();
    let down = renderer::Event::KeyDown {
        key: renderer::Key::default(),
        modifiers: renderer::Modifiers::NONE,
        repeat: false,
        ime: false,
    };
    let mut key = |script: &str| {
        desktop.js().eval(script, "test").unwrap();
        desktop.pump(None, None).unwrap();
        let prevented = main.before_input(1, &down, &|| false);
        match desktop
            .js()
            .eval("[calls,calls=0][0]", "test")
            .unwrap()
            .as_str()
        {
            "0" => "skipped",
            _ if prevented => "prevented",
            _ => "passed",
        }
    };
    assert_eq!(key("contents.on('other',f)"), "skipped");
    for method in [
        "on",
        "once",
        "addListener",
        "prependListener",
        "prependOnceListener",
    ] {
        assert_eq!(
            key(&format!("contents.{method}('before-input-event',f)")),
            "prevented",
            "{method}"
        );
        assert_eq!(
            key("contents.off('before-input-event',f)"),
            "skipped",
            "{method}"
        );
    }
    assert_eq!(key("contents.on('before-input-event',()=>{})"), "passed");
    assert_eq!(key("contents.on('before-input-event',f)"), "prevented");
    assert_eq!(
        key("contents.removeAllListeners('before-input-event')"),
        "skipped"
    );
    assert_eq!(key("contents.once('before-input-event',f)"), "prevented");
    assert_eq!(key(""), "skipped");
}

#[test]
fn closed_windows_never_reach_main() {
    let page =
        "<script type='preload'>globalThis.send=()=>__apps.ipcRenderer.send('late')</script>";
    let code = "globalThis.late=[];__apps.ipcMain.on('late',e=>late.push(e.sender===null));__apps.app.whenReady().then(()=>{globalThis.win=new __apps.BrowserWindow();win.loadFile('apps:///page.html')})";
    let mut desktop = app(code, &[("page.html", page)], std::path::PathBuf::new());
    let preload = desktop.views[&1].preload.clone().unwrap();
    preload.eval("send()", "test").unwrap();
    desktop.pump(None, None).unwrap();
    desktop.js().tick().unwrap();
    desktop.js().eval("win.close()", "test").unwrap();
    desktop.main.as_ref().unwrap().receive();
    preload.eval("send()", "test").unwrap();
    desktop.pump(None, None).unwrap();
    desktop.js().tick().unwrap();
    assert_eq!(desktop.js().eval("late.join()", "test").unwrap(), "false");
    assert!(!desktop.views.contains_key(&1));
}

#[test]
fn main_answers_pages_with_data_for_fixed_functions() {
    let code = "__apps.app.whenReady().then(()=>{globalThis.win=new __apps.BrowserWindow();win.load('<body></body>')})";
    let mut desktop = app(code, &[], std::path::PathBuf::new());
    let renderer = desktop.views[&1].renderer.clone().unwrap();
    renderer
        .eval(
            "globalThis.__appsFilesPicked=(...args)=>{globalThis.picked=args}",
            "test",
        )
        .unwrap();
    let files = serde_json::json!([7, [{"name":"a\");evil(\"","data":""}]]);
    desktop.command(serde_json::json!({"op":"window","args":["deliver",1,"filesPicked",files]}));
    desktop.pump(None, None).unwrap();
    assert_eq!(
        renderer.eval("JSON.stringify(picked)", "test").unwrap(),
        files.to_string()
    );
    for args in [
        serde_json::json!(["deliver", 1, "eval", []]),
        serde_json::json!(["deliver", 1, "filesPicked", "[7]"]),
        serde_json::json!(["input", 1, 1]),
    ] {
        assert!(crate::host::window::route(&args).is_err(), "{args}");
    }
}
