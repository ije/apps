#![cfg(target_os = "macos")]

fn run_hidden(main: &str, pages: &[(&str, &str)]) -> String {
    use std::{
        io::Write,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let port = main.contains("MCP").then(|| {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    });
    let main = format!(
        "const {{ app, BrowserWindow }} = __apps; app.setActivationPolicy('prohibited');
        const MCP = 'http://127.0.0.1:{}';
        const fail = what => {{ console.error('RENDER_FAIL ' + what); process.exit(2); }};
        app.whenReady().then(async () => {{
            setTimeout(() => fail('timeout'), 10000);
            const win = new BrowserWindow({{ width: 400, height: 300, show: false, focusable: false }});
            const run = code => win.webContents.executeJavaScript(code);
            const load = async name => {{ await win.loadFile('apps:///' + name); await win.webContents.capturePage(); }};
            {main}
            console.log('RENDER_OK'); app.quit();
        }});",
        port.unwrap_or(0)
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_apps-runtime"));
    if main.contains("BUILT") {
        command.env("APPS_TRACE_BUILDS", "1");
    }
    if main.contains("FORCE_P3") {
        command.env("APPS_FORCE_COLOR_PROFILE", "display-p3");
    }
    let mut child = command
        .arg("--bundle-stdin")
        .args(port.map(|port| format!("--devtools-mcp={port}")))
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
    let deadline = Instant::now() + Duration::from_secs(60);
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

const INPUT: &str = r#"
    const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
    const frames = async ms => { for (const end = Date.now() + ms; Date.now() < end; await wait(16)) await win.webContents.capturePage(); };
    const input = async body => {
        const sent = await fetch(MCP + '/input', { method: 'POST', body: JSON.stringify({ window: win.id, ...body }) }).then(response => response.text());
        if (!sent.startsWith('Sent')) fail('input: ' + sent);
        await frames(50);
    };
    const expect = async (step, code, value) => { const got = await run(code); if (got !== value) fail(step + ': ' + got + ' instead of ' + value); };
"#;

fn built<'a>(text: &'a str, step: &str) -> Vec<&'a str> {
    let (start, end) = (format!("STEP {step}"), format!("DONE {step}"));
    let lines: Vec<_> = text.lines().skip_while(|line| *line != start).collect();
    assert!(lines.contains(&end.as_str()), "{text}");
    lines
        .into_iter()
        .take_while(|line| *line != end)
        .filter_map(|line| line.strip_prefix("BUILT "))
        .collect()
}

const STEPS: &str = r#"
    // BUILT lines name the boxes that each step builds.
    const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
    const pixel = async (x, y) => [...(await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap()].slice(0, 3).join();
    const inside = async id => pixel(...JSON.parse(await run(`(r => JSON.stringify([r.x + 5, r.y + 5]))(document.getElementById('${id}').getBoundingClientRect())`)));
    const step = async (name, code) => {
        console.error('STEP ' + name);
        const value = await code();
        await win.webContents.capturePage();
        await win.webContents.capturePage();
        console.error('DONE ' + name);
        return value;
    };
"#;

const RECTS: &str = r#"
    const rects = () => run(`JSON.stringify(Object.fromEntries([...document.querySelectorAll('.w [id]')].map(el => {
        const box = el.getBoundingClientRect(), wrap = el.closest('.w').getBoundingClientRect();
        return [el.id, [box.x - wrap.x, box.y - wrap.y, box.width, box.height].map(v => Math.round(v * 100) / 100).join()];
    })))`);
"#;

fn rects(text: &str) -> std::collections::HashMap<String, String> {
    let line = text
        .lines()
        .find_map(|line| line.strip_prefix("RECTS "))
        .unwrap_or_else(|| panic!("{text}"));
    serde_json::from_str(line).unwrap()
}

fn long_list(scroller: bool) -> String {
    let mut page = String::from(
        "<body id=body style='margin:0'><style>.r{height:20px;font:12px/20px sans-serif}</style>\
         <p id=o style='margin:0;height:20px'>x</p>",
    );
    page.push_str(if scroller {
        "<div id=s style='height:200px;overflow:auto'><div id=list>"
    } else {
        "<div id=s><div id=list>"
    });
    for index in 0..2000 {
        page.push_str(&format!(
            "<div id=r{index} class=r tabindex=-1 style='background:rgb({},{},200)'>row {index}</div>",
            index % 250,
            index / 250 * 30
        ));
    }
    page.push_str("</div></div></body>");
    page
}

const LIST: &str = r#"
    // `look` prints the scroll offset, the row that the page hits and paints 110 px into the list's view, and the
    // last row's place in the page.
    const look = async name => {
        const [y, top, hit, last] = JSON.parse(await run(`(() => {
            const s = document.getElementById('s'), inner = s.scrollHeight > s.clientHeight;
            const top = inner ? s.scrollTop : scrollY, y = inner ? s.getBoundingClientRect().y + 110 : 110;
            const hit = document.elementFromPoint(300, y);
            return JSON.stringify([y, top, hit && hit.id, document.getElementById('r1999').getBoundingClientRect().y + top]);
        })()`));
        console.log('LOOK ' + name + ' ' + JSON.stringify([top, hit, last, await pixel(300, y)]));
    };
"#;

fn looks(text: &str) -> Vec<(String, f32, String, f32, Vec<u8>)> {
    text.lines()
        .filter_map(|line| line.strip_prefix("LOOK "))
        .map(|line| {
            let (name, json) = line.split_once(' ').unwrap();
            let value: serde_json::Value = serde_json::from_str(json).unwrap();
            (
                name.to_owned(),
                value[0].as_f64().unwrap() as f32,
                value[1].as_str().unwrap_or("").to_owned(),
                value[2].as_f64().unwrap() as f32,
                value[3]
                    .as_str()
                    .unwrap()
                    .split(',')
                    .map(|part| part.parse().unwrap())
                    .collect(),
            )
        })
        .collect()
}

fn check_looks(text: &str, offset: f32) {
    let looks = looks(text);
    assert!(looks.len() > 3, "{text}");
    for (name, top, hit, last, pixel) in looks {
        let row = ((top + 110. - offset) / 20.).floor() as usize;
        assert_eq!(hit, format!("r{row}"), "{name}: the row under the point");
        assert!(
            (last - 40000.).abs() < 0.1,
            "{name}: culled rows keep their place: {last}"
        );
        let [blue, green, red] = [200, row / 250 * 30, row % 250].map(|value| value as i32);
        assert!(
            (pixel[0] as i32 - blue).abs() <= 2
                && (pixel[1] as i32 - green).abs() <= 2
                && (pixel[2] as i32 - red).abs() <= 2,
            "{name}: row {row} paints {pixel:?}"
        );
    }
}

const PICTURE: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAADIAAAAoCAYAAAC8cqlMAAAASUlEQVR4nO3PsQ0AMAjAMP5/mp7RCHnIHs/O7IXm9wAISDyQWiC1QGqB1AKpBVILpBZILZBaILVAaoHUAqkFUgukFkgtkFpnIA+Vw5FCgTkXpwAAAABJRU5ErkJggg==";

fn picture_rects(name: &str, page: &str) -> (String, std::collections::HashMap<String, String>) {
    let text = run_hidden(
        &format!(
            "{RECTS} await load('{name}');
            await run(`Promise.all([...document.querySelectorAll('img')].map(image => image.decode().catch(() => {{}})))`);
            await new Promise(resolve => setTimeout(resolve, 100));
            await win.webContents.capturePage();
            console.log('RECTS ' + await rects());"
        ),
        &[(name, &page.replace("PICTURE", PICTURE))],
    );
    let rects = rects(&text);
    (text, rects)
}

mod anchor;
mod incremental;
mod layout;
mod media;
mod native;
mod paint;
mod scroll;
mod text;
mod top_layer;
