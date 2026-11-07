#![cfg(target_os = "macos")]

fn run_hidden(main: &str, pages: &[(&str, &str)]) -> String {
    use std::{
        io::Write,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let main = format!(
        "const {{ app, BrowserWindow }} = __apps; app.setActivationPolicy('prohibited');
        const fail = what => {{ console.error('RENDER_FAIL ' + what); process.exit(2); }};
        app.whenReady().then(async () => {{
            setTimeout(() => fail('timeout'), 10000);
            const win = new BrowserWindow({{ width: 400, height: 300, show: false, focusable: false }});
            const run = code => win.webContents.executeJavaScript(code);
            const load = async name => {{ await win.loadFile('apps:///' + name); await win.webContents.capturePage(); }};
            {main}
            console.log('RENDER_OK'); app.quit();
        }});"
    );
    let mut files = vec![serde_json::json!([
        "app.js",
        format!(
            "$appsDefine({});",
            serde_json::json!({"id":0,"imports":[],"exports":[],"stars":[],"code":main})
        )
    ])];
    for (name, html) in pages {
        let doc = apps_runtime::dom::Document::parse(html);
        assert!(doc.nodes.iter().any(|node| node.tag == "body"), "{name}");
        files.push(serde_json::json!([name, html]));
    }
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
        .write_all(
            serde_json::json!({"files": files, "pages": []})
                .to_string()
                .as_bytes(),
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let output = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout) + String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && text.contains("RENDER_OK"),
        "{text}"
    );
    text.into_owned()
}

#[test]
#[ignore = "requires the macOS window server"]
fn reloads_never_show_boxes_of_the_old_page() {
    run_hidden(
        r#"
        const width = () => run("document.getElementById('t').getBoundingClientRect().width");
        await load('a.html');
        const narrow = await width();
        await load('b.html');
        const wide = await width();
        if (!(wide > narrow * 4)) fail('old boxes after a navigation: ' + narrow + ' ' + wide);
        "#,
        &[
            (
                "a.html",
                "<body style='margin:0'><span id=t>i</span></body>",
            ),
            (
                "b.html",
                "<body style='margin:0'><span id=t>WWWWWWWW</span></body>",
            ),
        ],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn positioned_boxes_follow_their_containing_block_and_scroller() {
    run_hidden(
        r#"
        const rect = id => `(r => [r.x, r.y, r.height].join())(document.getElementById('${id}').getBoundingClientRect())`;
        const expect = async (step, code, value) => {
            const got = await run(code);
            if (got !== value) fail(step + ': ' + got + ' instead of ' + value);
        };
        await load('position.html');
        await expect('static', rect('a'), '0,0,10');
        await expect('grid', rect('p'), '0,150,150');
        await expect('relative', "g.style.position = 'relative';" + rect('a'), '100,50,10');
        await expect('static again', "g.style.position = '';" + rect('a'), '0,0,10');
        await expect('translate', "g.style.translate = '5px 0';" + rect('a'), '105,50,10');
        await load('sticky.html');
        await expect('stuck', "s.scrollTop = 40;" + rect('t'), '0,200,10');
        await expect('scrolled', "s.scrollTop", 40);
        await expect('unscrolled', "s.style.overflow = 'visible'; s.scrollTop", 0);
        await expect('page', "scrollTo(0, 250);" + rect('t'), '0,0,10');
        "#,
        &[
            (
                "position.html",
                "<body style='margin:0'>\
                 <div id=g style='margin:50px 0 0 100px;width:200px;height:100px'><div>\
                 <i id=a style='position:absolute;left:0;top:0;width:10px;height:10px'></i></div></div>\
                 <div style='display:grid;position:relative;min-height:300px;width:300px'><div>\
                 <i id=p style='position:absolute;top:0;width:10px;height:50%'></i></div></div>\
                 <script>globalThis.g = document.getElementById('g')</script></body>",
            ),
            (
                "sticky.html",
                "<body style='margin:0'><div style='height:200px'></div>\
                 <div id=s style='height:100px;overflow:auto'><div style='height:300px'>\
                 <b id=t style='position:sticky;top:0;display:block;height:10px'></b></div></div>\
                 <div style='height:2000px'></div>\
                 <script>globalThis.s = document.getElementById('s')</script></body>",
            ),
        ],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn individual_transforms_hold_their_z_indexed_children() {
    let text = run_hidden(
        r#"
        await load('stack.html');
        for (const value of ['scale: 1.05', 'translate: 1px', 'rotate: 1deg', 'transform: scale(1.05)']) {
            await run(`document.getElementById('outer').style.cssText = '${value}'`);
            const image = await win.webContents.capturePage({ x: 20, y: 80, width: 1, height: 1 });
            console.log('PIXEL ' + value + ' ' + image.toBitmap()[1]);
        }
        "#,
        &[(
            "stack.html",
            "<body style='margin:0;background:white'>\
             <div id=outer><div style='position:relative;z-index:10;background:red;width:100px;height:100px'></div></div>\
             <div style='position:relative;z-index:5;background:lime;width:100px;height:100px;margin-top:-50px'></div></body>",
        )],
    );
    let green: Vec<_> = text
        .lines()
        .filter_map(|line| line.strip_prefix("PIXEL "))
        .collect();
    assert_eq!(green.len(), 4, "{text}");
    for line in green {
        let value: u8 = line.rsplit(' ').next().unwrap().parse().unwrap();
        assert!(value > 200, "the later z-index:5 box must cover: {line}");
    }
}
