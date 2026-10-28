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
            ("app.js".into(), format!("define({definition});")),
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
