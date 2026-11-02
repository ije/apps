use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn runs_a_streamed_bundle_with_project_assets_and_metadata() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("public")).unwrap();
    fs::write(directory.path().join("public/asset.txt"), "project asset").unwrap();
    let code = r#"
        const {app, BrowserWindow, fs} = globalThis.__apps;
        if (app.isPackaged || app.getVersion() !== '1.2.3') throw Error('metadata');
        if (fs.readFileSync('public/asset.txt', 'utf8') !== 'project asset') throw Error('asset');
        app.whenReady().then(() => new BrowserWindow().loadFile('apps:///page.html'));
    "#;
    let definition = serde_json::json!({
        "id":0,"code":code,"imports":[],"exports":[],"stars":[],"commonjs":false
    });
    let bundle = apps_runtime::bundle::Bundle {
        files: bbtree::unsync::BBTreeMap::from([
            ("app.js".into(), format!("$appsDefine({definition});")),
            ("package.json".into(), r#"{"name":"MemoryApp","version":"1.2.3"}"#.into()),
            ("page.html".into(), "<h1>In memory</h1><script type='preload'>if(__apps.fs.readFileSync('public/asset.txt','utf8')!=='project asset')throw Error('preload');</script><script type='module'>if(document.querySelector('h1').textContent!=='In memory')throw Error('renderer');</script>".into()),
        ]),
        ..Default::default()
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_apps-runtime"))
        .args(["--bundle-stdin", "--check"])
        .current_dir(directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    serde_json::to_writer(child.stdin.take().unwrap(), &bundle).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("checked 1 window(s)"));
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn rejects_invalid_streamed_bundles_and_conflicting_arguments() {
    for (input, error) in [
        ("{", "Cannot read the app bundle from stdin"),
        (r#"{"files":[],"pages":[]}"#, "Missing app.js"),
        (
            r#"{"files":[["app.js",""],["app.js",""]],"pages":[]}"#,
            "Duplicate bundle key",
        ),
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_apps-runtime"))
            .args(["--bundle-stdin", "--check"])
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(error),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = Command::new(env!("CARGO_BIN_EXE_apps-runtime"))
        .args([".", "--bundle-stdin", "--check"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Cannot use a directory with --bundle-stdin")
    );
}

#[test]
#[ignore = "release performance measurement"]
fn engine_launch() {
    use apps_runtime::{
        host::Role,
        jsc::{JavaScriptCore, JsContext},
    };
    let api = JavaScriptCore::load().unwrap();
    let host: std::rc::Rc<std::cell::RefCell<apps_runtime::host::Host>> = std::rc::Rc::default();
    let mut times = Vec::new();
    for id in 1..=9 {
        host.borrow_mut().pages.insert(
            id,
            apps_runtime::host::Page {
                doc: apps_runtime::dom::Document::parse("<body></body>"),
                dirty: false,
            },
        );
        let start = std::time::Instant::now();
        let preload =
            JsContext::new(api.clone(), std::rc::Rc::clone(&host), Role::Preload(id)).unwrap();
        let page =
            JsContext::new(api.clone(), std::rc::Rc::clone(&host), Role::Renderer(id)).unwrap();
        times.push(start.elapsed().as_secs_f64() * 1000.);
        drop((preload, page));
    }
    println!("window_engines_first_ms={:.3}", times[0]);
    times.sort_by(f64::total_cmp);
    println!("window_engines_median_ms={:.3}", times[4]);
}

#[cfg(unix)]
#[test]
fn process_exit_argv_and_non_utf8_environment() {
    let code = r#"
        process.on('exit', code => process.stdout.write(`exit ${code} ${JSON.stringify(process.argv.slice(1))} ${process.env.APPS_BAD}\n`));
        process.env.APPS_NUMBER = 42; process.env.APPS_UNDEFINED = undefined;
        process.stdout.write([process.env.APPS_NUMBER, process.env.APPS_UNDEFINED, 'APPS_UNDEFINED' in process.env].join() + '\n');
        setTimeout(() => process.stdout.write('timer\n'), 0);
        process.exitCode = 2;
        process.exit(3);
        process.stdout.write('after\n');
    "#;
    let definition = serde_json::json!({
        "id":0,"code":code,"imports":[],"exports":[],"stars":[],"commonjs":false
    });
    let bundle = apps_runtime::bundle::Bundle {
        files: bbtree::unsync::BBTreeMap::from([(
            "app.js".into(),
            format!("$appsDefine({definition});"),
        )]),
        ..Default::default()
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_apps-runtime"))
        .args(["--bundle-stdin", "--", "/app/main.js", "a", "--b"])
        .env(
            "APPS_BAD",
            <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(b"\xff"),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    serde_json::to_writer(child.stdin.take().unwrap(), &bundle).unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "42,undefined,true\nexit 3 [\"/app/main.js\",\"a\",\"--b\"] \u{fffd}\n"
    );
}
