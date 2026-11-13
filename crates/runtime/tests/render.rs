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
fn sf_symbol_elements_paint_and_follow_css() {
    run_hidden(
        r#"
        await load('symbols.html');
        const bounds = JSON.parse(await run(`JSON.stringify(['icon', 'next', 'inline', 'block'].map(id => {
            const r = document.getElementById(id).getBoundingClientRect(); return [r.x, r.y, r.width, r.height];
        }))`));
        if (bounds[0][2] !== 32 || bounds[0][3] !== 32 || bounds[1][0] < 40 || bounds[2][2] !== 40 || bounds[2][3] !== 40 || Math.abs(bounds[2][1] - bounds[3][1]) > 1)
            fail('symbol bounds: ' + JSON.stringify(bounds));
        const colors = async (inset = 0) => {
            const bytes = (await win.webContents.capturePage({ x: bounds[0][0] + inset, y: bounds[0][1] + inset, width: 32, height: 32 })).toBitmap();
            let red = 0, green = 0;
            for (let i = 0; i < bytes.length; i += 4) {
                if (bytes[i + 2] > 180 && bytes[i + 1] < 80 && bytes[i] < 80) red++;
                if (bytes[i + 1] > 180 && bytes[i + 2] < 80 && bytes[i] < 80) green++;
            }
            return [red, green];
        };
        const before = await colors();
        if (before[0] < 100 || before[1] !== 0) fail('missing red symbol: ' + before);
        await run("Object.assign(document.getElementById('icon').style, {boxSizing:'content-box', padding:'4px', border:'2px solid blue'})");
        const padded = await colors(6);
        if (padded[0] !== before[0]) fail('symbol content changed with padding: ' + padded + ' before ' + before);
        await run("document.body.style.color = '#00e600'");
        const after = await colors(6);
        if (after[1] < 100 || after[0] !== 0) fail('symbol did not inherit color: ' + after);
        await run("document.getElementById('icon').style.visibility = 'hidden'");
        const hidden = await colors(6);
        if (hidden[0] !== 0 || hidden[1] !== 0) fail('hidden symbol still paints: ' + hidden);
        "#,
        &[(
            "symbols.html",
            "<style>@import 'tailwindcss';body{margin:0;color:#e60000;background:white}</style>\
             <div style='display:flex;align-items:center;gap:8px'><sf-symbol-square-fill id=icon class=size-8 />\
             <button id=next>Next</button></div>\
             <p style='font-size:20px;line-height:20px'>X<sf-symbol-info-circle id=inline style='display:inline;width:40px;height:40px' />\
             <sf-symbol-info-circle id=block style='width:40px;height:40px' />Y</p>",
        )],
    );
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
        await load('escape.html');
        const hit = (x, y) => `(e => e.id || e.localName)(document.elementFromPoint(${x}, ${y}))`;
        await expect('absolute menu', hit(170, 20), 'menu');
        await expect('fixed overlay', hit(250, 220), 'overlay');
        await expect('clipped by its block', hit(160, 130), 'html');
        await expect('fixed in a transform', "s.style.transform = 'translateX(0px)';" + hit(250, 220), 'html');
        await run("s.style.transform = ''; document.getElementById('host').attachShadow({mode: 'open'}).innerHTML = `<i style='position:absolute;left:250px;top:0;width:20px;height:20px'></i>`");
        await win.webContents.capturePage();
        await expect('shadow host', hit(260, 10), 'host');
        await expect('shadow hits', "const root = document.getElementById('host').shadowRoot; [root.elementFromPoint(260, 10).localName, document.elementsFromPoint(260, 10).filter(e => e.id === 'host').length, root.elementsFromPoint(260, 10)[0].localName].join()", 'i,1,i');
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
            (
                "escape.html",
                "<body style='margin:0'><div id=s style='width:100px;height:100px;overflow:hidden'>\
                 <div id=menu style='position:absolute;left:150px;top:0;width:50px;height:50px'></div>\
                 <div id=overlay style='position:fixed;left:0;top:200px;width:300px;height:50px'></div>\
                 <div style='position:relative;height:100px'>\
                 <i id=clipped style='position:absolute;left:150px;top:120px;width:20px;height:20px'></i></div>\
                 <div><div id=host></div></div></div>\
                 <script>globalThis.s = document.getElementById('s')</script></body>",
            ),
        ],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn scrollbars_fade_after_a_drag_that_ends_outside_the_window() {
    run_hidden(
        r#"
        await load('page.html');
        const input = body => fetch(MCP + '/input', { method: 'POST', body: JSON.stringify({ window: win.id, ...body }) }).then(response => response.text());
        // Counts the device pixels of the bar's column that are not the white page.
        const knob = async () => (await win.webContents.capturePage({ x: 393, y: 0, width: 1, height: 200 })).toBitmap().filter((value, index) => index % 4 === 0 && value < 250).length;
        await input({ action: 'drag', x: 393, y: 15, to_x: 600, to_y: 60 });
        const [held, top] = [await knob(), await run("document.getElementById('s').scrollTop")];
        await new Promise(resolve => setTimeout(resolve, 1600));
        const idle = await knob();
        if (!held || !top || idle) fail('bar after the drag: ' + held + ' knob pixels at ' + top + ', then ' + idle);
        "#,
        &[(
            "page.html",
            "<body style='margin:0;background:#fff;overflow:hidden'>\
             <div id=s style='width:400px;height:200px;overflow:auto'><div style='height:3000px'></div></div></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn background_images_decode_and_draw() {
    run_hidden(
        r#"
        await load('page.html');
        const red = async () => {
            const bytes = (await win.webContents.capturePage({ x: 10, y: 10, width: 40, height: 40 })).toBitmap();
            let count = 0;
            for (let i = 0; i < bytes.length; i += 4) if (bytes[i + 2] > 200 && bytes[i + 1] < 60 && bytes[i] < 60) count++;
            return count;
        };
        let count = 0;
        for (let n = 0; n < 40 && count < 1600; n++) count = await red();
        if (count < 1600) fail('background image did not draw: ' + count);
        "#,
        &[(
            "page.html",
            "<body style='margin:0;background:white'><div style='width:60px;height:60px;background-size:100% 100%;\
             background-image:url(data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR4nGP4z8AARAwQCgAf7gP9i18U1AAAAABJRU5ErkJggg==)'></div></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn stretched_images_keep_their_size_when_they_restyle_in_place() {
    run_hidden(
        r#"
        await load('page.html');
        const input = body => fetch(MCP + '/input', { method: 'POST', body: JSON.stringify({ window: win.id, ...body }) }).then(response => response.text());
        const look = async () => {
            for (let n = 0; n < 3; n++) await win.webContents.capturePage();
            return run("['i','j'].map(id => (r => [r.x, r.y, r.width, r.height].join())(document.getElementById(id).getBoundingClientRect())).join(' | ') + ' ' + getComputedStyle(document.getElementById('i')).opacity");
        };
        const before = await look();
        await input({ action: 'move', x: 10, y: 10 });
        const hover = await look();
        if (!before.startsWith('0,0,48,48 | 48,0,') || hover !== before.replace(/ 1$/, ' .8')) fail('hover: ' + before + ' then ' + hover);
        "#,
        &[(
            "page.html",
            "<body style='margin:0'><style>img:hover{opacity:.8}</style><div style='display:flex;height:48px'>\
             <img id=i src='data:image/svg+xml,%3Csvg xmlns=%22http://www.w3.org/2000/svg%22 width=%2296%22 height=%2296%22%3E%3Crect width=%2296%22 height=%2296%22 fill=%22red%22/%3E%3C/svg%3E'>\
             <span id=j>after</span></div></body>",
        )],
    );
}

// Debug builds compare each patched frame with a full one: tree walks, visibility, boxes and layout.
#[test]
#[ignore = "requires the macOS window server"]
fn random_tree_changes_draw_like_full_frames() {
    run_hidden(
        r#"
        await load('page.html');
        for (let frame = 0; frame < 80; frame++) {
            await run('change()');
            await win.webContents.capturePage();
        }
        const state = JSON.parse(await run(`JSON.stringify([...document.querySelectorAll('li')].map(li => {
            const boxed = !li.closest('.hide') && !li.classList.contains('flat');
            return boxed === li.getBoundingClientRect().height > 0;
        }))`));
        if (!state.length || state.includes(false)) fail('rows and boxes disagree: ' + state.join());
        "#,
        &[(
            "page.html",
            "<body style='margin:0'><style>.hide{display:none} .flat{display:contents} li{height:12px} #pane{height:80px;overflow:auto}</style>\
             <div id=pane><ul id=list></ul></div><table><tbody id=rows></tbody></table><div id=box></div>\
             <script>\
             let seed = 4321;\
             const random = below => (seed = Math.imul(seed, 1103515245) + 12345 >>> 0, (seed >>> 8) % below);\
             const list = document.getElementById('list'), rows = document.getElementById('rows'), box = document.getElementById('box');\
             const make = (tag, text) => { const el = document.createElement(tag); if (text) el.textContent = text; return el; };\
             const item = () => { const li = make('li'); li.append(make('span', 'item'), make('b', 'bold')); return li; };\
             const any = () => { const all = document.body.querySelectorAll('li, li *, #box *, td'); return all[random(all.length)]; };\
             for (let i = 0; i < 40; i++) list.append(item());\
             const steps = [\
               () => list.insertBefore(item(), list.children[random(list.children.length + 1)] ?? null),\
               () => list.children.length > 5 && list.children[random(list.children.length)].remove(),\
               () => list.insertBefore(list.children[random(list.children.length)], list.children[random(list.children.length)]),\
               () => { const el = list.querySelectorAll('span, b')[random(60)]; if (el) el.textContent = 'text ' + random(9); },\
               () => any()?.classList.toggle(['hide', 'flat'][random(2)]),\
               () => { const row = make('tr'); for (let i = 1 + random(3); i--;) row.append(make('td', 'cell')); rows.insertBefore(row, rows.children[random(rows.children.length + 1)] ?? null); },\
               () => rows.children[random(rows.children.length)]?.remove(),\
               () => any()?.toggleAttribute('inert'),\
               () => box.append(make(['div', 'p', 'canvas'][random(3)], random(2) ? 'more' : '')),\
               () => box.children[random(box.children.length)]?.remove(),\
               () => { const el = box.children[random(box.children.length)]; if (el) { const wrap = make('div'); el.replaceWith(wrap); wrap.append(el); } },\
               () => { const el = list.children[random(list.children.length)]; if (el.parentNode === list) box.append(el); },\
               () => { const el = box.querySelector('li'); if (el) list.append(el); },\
             ];\
             globalThis.change = () => { for (let count = 1 + random(3); count--;) steps[random(steps.length)](); };\
             </script></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn pages_reach_no_local_files_through_webviews_and_no_devtools() {
    run_hidden(
        r#"
        await load('page.html');
        // A guest attaches when its box is laid out, so frames must go on while the page waits.
        const attach = src => run(`new Promise(resolve => {
            const view = document.createElement('webview');
            view.addEventListener('did-fail-load', event => resolve('failed ' + event.errorDescription));
            view.addEventListener('did-finish-load', async () => resolve('loaded ' + await view.executeJavaScript('document.body.innerText')));
            view.src = ${JSON.stringify(src)};
            document.body.append(view);
            setTimeout(() => resolve('nothing'), 4000);
        })`);
        const settle = async pending => { let result; pending.then(value => result = value); while (result === undefined) await win.webContents.capturePage(); return result; };
        const control = await settle(attach('data:text/html,<p>guest</p>'));
        if (control !== 'loaded guest') fail('webview with a data URL: ' + control);
        const file = await run("'file://' + document.body.dataset.file");
        const guest = await settle(attach(file));
        if (guest !== 'failed ERR_ACCESS_DENIED') fail('webview with a local file: ' + guest);
        const later = await settle(run(`new Promise(resolve => {
            const view = document.querySelector('webview');
            view.loadURL('file://' + document.body.dataset.file).then(() => resolve('loaded'), error => resolve('refused'));
            setTimeout(() => resolve('nothing'), 4000);
        })`));
        if (later !== 'refused') fail('loadURL with a local file: ' + later);
        // A page in a browser always names its origin when it posts. Tool clients don't.
        const post = headers => fetch(MCP + '/input', { method: 'POST', headers, body: JSON.stringify({ window: win.id, action: 'move', x: 1, y: 1 }) }).then(response => response.status);
        const statuses = [await post({}), await post({ Origin: 'http://localhost:3000' }), await post({ Origin: 'null' })].join();
        if (statuses !== '200,403,403') fail('devtools answered ' + statuses);
        "#,
        &[(
            "page.html",
            concat!(
                "<body style='margin:0' data-file='",
                env!("CARGO_MANIFEST_DIR"),
                "/Cargo.toml'><p>page</p></body>"
            ),
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn webviews_report_one_event_for_a_navigation_inside_a_page() {
    run_hidden(
        r#"
        await load('page.html');
        const pending = run(`new Promise(resolve => {
            const view = document.createElement('webview');
            let count = 0;
            view.addEventListener('did-navigate-in-page', () => count++);
            view.addEventListener('did-finish-load', async () => {
                await view.executeJavaScript("location.hash = 'a'");
                setTimeout(() => resolve(count), 500);
            });
            view.src = 'data:text/html,<p>guest</p>';
            document.body.append(view);
        })`);
        let count;
        pending.then(value => count = value);
        while (count === undefined) await win.webContents.capturePage();
        if (count !== 1) fail('did-navigate-in-page fired ' + count + ' times');
        "#,
        &[("page.html", "<body style='margin:0'><p>page</p></body>")],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn svg_text_takes_its_decoration_from_css() {
    run_hidden(
        r#"
        await load('page.html');
        await win.webContents.capturePage();
        // Counts the dark device pixels of each 60 px row: plain text, an attribute, CSS, and text in a link,
        // whose underline stops at the edge of the image as in browsers.
        const ink = async top => (await win.webContents.capturePage({ x: 0, y: top, width: 300, height: 60 })).toBitmap().filter((value, index) => index % 4 === 0 && value < 128).length;
        const [plain, attribute, css, linked] = [await ink(0), await ink(60), await ink(120), await ink(180)];
        if (!(attribute > plain) || css !== attribute || linked !== plain) fail('underline ink: ' + [plain, attribute, css, linked].join());
        "#,
        &[(
            "page.html",
            "<body style='margin:0;background:#fff'><style>svg{display:block}.u{text-decoration:underline}</style>\
             <svg width=300 height=60><text x=10 y=40 font-size=30>mmmm</text></svg>\
             <svg width=300 height=60><text x=10 y=40 font-size=30 text-decoration=underline>mmmm</text></svg>\
             <svg width=300 height=60><text x=10 y=40 font-size=30 class=u>mmmm</text></svg>\
             <a href='#x' style='text-decoration:underline;color:#000'><svg width=300 height=60><text x=10 y=40 font-size=30>mmmm</text></svg></a></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn boxes_that_stop_scrolling_report_their_content_extent() {
    run_hidden(
        r#"
        await load('page.html');
        const heights = () => run("['a','b'].map(id => document.getElementById(id).scrollHeight).join()");
        const first = await heights();
        await run("document.getElementById('a').scrollTop = 0; document.getElementById('b').style.overflow = 'visible'");
        await win.webContents.capturePage();
        await win.webContents.capturePage();
        const second = await heights();
        if (first !== '500,500' || second !== '500,500') fail('scroll extents: ' + first + ' then ' + second);
        "#,
        &[(
            "page.html",
            "<body style='margin:0'><div id=a style='height:50px'><div style='height:500px'></div></div>\
             <div id=b style='height:50px;overflow:hidden'><div style='height:500px'></div></div></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn closed_selects_draw_only_their_selected_option() {
    run_hidden(
        r#"
        await load('page.html');
        const boxes = await run("[...document.querySelectorAll('select')].map(s => { const r = s.getBoundingClientRect(); return [r.top, r.height].join(); }).join(' ')");
        if (new Set(boxes.split(' ')).size !== 1 || !boxes.endsWith(',19')) fail('select boxes: ' + boxes);
        "#,
        &[(
            "page.html",
            "<body style='margin:0'><select><option>A<option selected>B</select>\n<select>\n  <option>A</option>\n  <option selected>B</option>\n</select>\n\
             <select>\n text <optgroup label=G>\n<option>A</option>\n</optgroup>\n<hr>\n<option selected>B</option>\n</select></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn kept_boxes_follow_what_their_build_read_outside_them() {
    run_hidden(
        r#"
        const at = id => `(r => [r.x, r.y].join())(document.getElementById('${id}').getBoundingClientRect())`;
        await load('changed.html');
        const top = await run(at('t'));
        const scrolled = await run("document.getElementById('h').scrollTop = 40;" + at('t'));
        if (+scrolled.split(',')[1] !== +top.split(',')[1] - 40) fail('script scroll: ' + top + ' then ' + scrolled);
        const ltr = await run("document.getElementById('d').style.direction = 'ltr';" + at('c'));
        const font = await run("document.getElementById('p').style.fontSize = '40px';" + at('s'));
        await load('fresh.html');
        const fresh = [await run(at('c')), await run(at('s'))];
        if (ltr !== fresh[0]) fail('parent direction: ' + ltr + ' instead of ' + fresh[0]);
        if (font !== fresh[1]) fail('parent font: ' + font + ' instead of ' + fresh[1]);
        "#,
        &[
            (
                "changed.html",
                "<body style='margin:0'><div style='padding:10px'><div id=h style='overflow:hidden;height:50px'>\
                 <div style='height:200px'><i id=t style='display:block;height:10px;margin-top:60px'></i></div></div></div>\
                 <div id=d style='direction:rtl;width:300px'><div id=c style='direction:ltr;width:100px;height:10px'></div></div>\
                 <p id=p style='font-size:20px;line-height:30px;margin:0'>x<span id=s \
                 style='display:inline-block;vertical-align:super;font-size:10px;line-height:10px'>y</span></p></body>",
            ),
            (
                "fresh.html",
                "<body style='margin:0'><div style='padding:10px'><div id=h style='overflow:hidden;height:50px'>\
                 <div style='height:200px'><i id=t style='display:block;height:10px;margin-top:60px'></i></div></div></div>\
                 <div id=d style='direction:ltr;width:300px'><div id=c style='direction:ltr;width:100px;height:10px'></div></div>\
                 <p id=p style='font-size:40px;line-height:30px;margin:0'>x<span id=s \
                 style='display:inline-block;vertical-align:super;font-size:10px;line-height:10px'>y</span></p></body>",
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

#[test]
#[ignore = "requires the macOS window server"]
fn native_overlays_stack_in_one_order() {
    let text = run_hidden(
        r#"
        const { WebContentsView, ffi } = __apps;
        const path = '/usr/lib/libobjc.A.dylib', lib = new ffi.DynamicLibrary(path);
        const send = (args, result) => new ffi.DynamicLibrary(path).getFunction('objc_msgSend', { arguments: ['ptr', 'ptr', ...args], return: result });
        const [object, count, at, yes] = [send([], 'ptr'), send([], 'u64'), send(['u64'], 'ptr'), send([], 'u8')];
        const sel = lib.getFunction('sel_registerName', { arguments: ['string'], return: 'ptr' });
        const named = lib.getFunction('objc_getClass', { arguments: ['string'], return: 'ptr' });
        const className = lib.getFunction('object_getClassName', { arguments: ['ptr'], return: 'ptr' });
        const kind = target => ffi.toString(className(target)).replace('NSKVONotifying_', '');
        const get = (target, ...names) => names.reduce((value, name) => object(value, sel(name)), target);
        const list = array => Array.from({ length: Number(count(array, sel('count'))) }, (_, k) => at(array, sel('objectAtIndex:'), k));
        const web = new WebContentsView();
        win.contentView.addChildView(web);
        web.setBounds({ x: 200, y: 150, width: 100, height: 100 });
        await win.loadFile('apps:///glass.html');
        await win.webContents.capturePage().catch(() => {});
        const window = list(get(named('NSApplication'), 'sharedApplication', 'windows')).find(target => kind(target) === 'AppsWindow');
        const view = list(get(window, 'contentView', 'subviews')).find(target => kind(target) === 'AppsView');
        const glass = named('NSGlassEffectView') !== 0n;
        let views = [], layers = [];
        for (let tries = 0; tries < 100; tries++) {
            views = list(get(view, 'subviews'));
            layers = list(get(view, 'layer', 'sublayers'));
            if (views.some(target => kind(target) === 'NSGlassEffectView') === glass && layers.length === views.length) break;
            await new Promise(resolve => setTimeout(resolve, 50));
        }
        console.log('ORDER ' + views.map(kind).join(' '));
        if (!views.every((target, k) => get(target, 'layer') === layers[k])) fail('the frame layer has sublayers of its own: ' + layers.map(kind));
        const overlay = views.find(target => kind(target) === 'AppsOverlayView');
        if (!overlay || !yes(get(overlay, 'layer'), sel('isGeometryFlipped'))) fail('the overlay layer must be flipped');
        if (glass) console.log('GLASS');
        "#,
        &[(
            "glass.html",
            "<body style='margin:0;background:white'>\
             <div class=bg-glass style='position:absolute;left:20px;top:20px;width:120px;height:60px;border-radius:30px'></div></body>",
        )],
    );
    let order: Vec<_> = text
        .lines()
        .find_map(|line| line.strip_prefix("ORDER "))
        .unwrap_or_else(|| panic!("{text}"))
        .split(' ')
        .collect();
    let expected: &[&str] = if text.contains("GLASS") {
        &["NSGlassEffectView", "AppsOverlayView", "WKWebView"]
    } else {
        &["AppsOverlayView", "WKWebView"]
    };
    assert_eq!(
        order, expected,
        "glass, then promoted layers and the caret, then web views"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn main_filters_synthetic_keys_before_the_page() {
    run_hidden(
        r#"
        await load('keys.html');
        const seen = [];
        win.webContents.on('before-input-event', (event, input) => {
            seen.push(input.type + ' ' + input.code + ' ' + input.shift);
            if (input.code === 'x') event.preventDefault();
        });
        await new Promise(resolve => setTimeout(resolve, 50));
        const body = JSON.stringify({ window: win.id, action: 'key', key: 'x shift-y' });
        const sent = await fetch(MCP + '/input', { method: 'POST', body }).then(response => response.text());
        if (!sent.startsWith('Sent')) fail('input: ' + sent);
        const page = await run('keys.join()');
        if (seen.join() !== 'keyDown x false,keyUp x false,keyDown y true,keyUp y true') fail('main saw ' + seen.join());
        if (page !== 'keydown Y,keyup Y') fail('page saw ' + page);
        "#,
        &[(
            "keys.html",
            "<body><script>globalThis.keys = [];\
             for (const type of ['keydown', 'keyup']) addEventListener(type, event => keys.push(type + ' ' + event.key));\
             </script></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn window_focus_and_blur_reach_the_page_like_chrome() {
    run_hidden(
        r#"
        const expect = async (step, code, value) => { const got = await run(code); if (got !== value) fail(step + ': ' + got + ' instead of ' + value); };
        await load('focus.html');
        await expect('hidden window', "[document.hasFocus(), document.activeElement.localName].join()", 'false,body');
        await expect('focus without events', "t.focus(), [document.activeElement.id, log.splice(0).join(), t.matches(':focus')].join()", 't,,false');
        await expect('window focus', "__appsDispatch('window:focus', []), [document.hasFocus(), log.splice(0).join(), t.matches(':focus'), document.body.matches(':focus-within')].join()", 'true,focus:win,focus:t,focusin:t,true,true');
        await run("__appsInput('か', null, null, true, [1, 1])");
        await expect('window blur', "__appsDispatch('window:blur', []), [document.hasFocus(), document.activeElement.id, log.splice(0).join(), t.value, __appsEditing().marked, t.matches(':focus')].join()", 'false,t,compositionend:か:t,blur:t,focusout:t,blur:win,か,,false');
        await expect('blur without events', "t.blur(), [document.activeElement.localName, log.splice(0).join()].join()", 'body,');
        "#,
        &[(
            "focus.html",
            "<body><textarea id=t></textarea><script>globalThis.log = []; globalThis.t = document.getElementById('t');\
             for (const type of ['focus', 'blur', 'focusin', 'focusout', 'compositionend']) addEventListener(type, event => log.push(type + (event.data ? ':' + event.data : '') + ':' + (event.target === window ? 'win' : event.target.id)), true);</script></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn exit_transitions_keep_their_boxes_until_they_end() {
    run_hidden(
        r#"
        const state = "(r => [r.width, r.height, document.getElementById('d').dataset.ended].join())(document.getElementById('i').getBoundingClientRect())";
        const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
        await load('exit.html');
        let got = await run(state);
        if (got !== '200,200,') fail('before: ' + got);
        await run("document.getElementById('d').className = 'off'");
        for (let step = 0; step < 3; step++) {
            await win.webContents.capturePage();
            got = await run(state);
            if (got !== '200,200,') fail('exiting ' + step + ': ' + got);
            await wait(100);
        }
        await wait(1000);
        await win.webContents.capturePage();
        await wait(50);
        await win.webContents.capturePage();
        got = await run(state);
        if (got !== '0,0,1') fail('after: ' + got);
        "#,
        &[(
            "exit.html",
            "<body style='margin:0'><style>#d{transition:opacity 1s,display 1s allow-discrete}#d.off{display:none;opacity:0}</style>\
             <div id=d><svg id=i width=200 height=200><rect width=200 height=200 fill=red /></svg></div>\
             <script>document.getElementById('d').addEventListener('transitionend', event => {\
             if (event.propertyName === 'display') event.target.dataset.ended = 1 })</script></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn shadow_hosts_take_pseudo_elements_and_container_rules() {
    run_hidden(
        r#"
        const state = "(r => [r.width, r.height, getComputedStyle(card).color, empty.getBoundingClientRect().height].join())(card.getBoundingClientRect())";
        const expect = async (step, code, value) => {
            await run(code);
            for (let frame = 0; frame < 3; frame++) await win.webContents.capturePage();
            const got = await run(state);
            if (got !== value) fail(step + ': ' + got + ' instead of ' + value);
        };
        await load('host.html');
        await expect('first', '0', '200,58,rgb(0, 128, 0),9');
        await expect('class', "card.classList.remove('on')", '200,48,rgb(0, 128, 0),9');
        await expect('wide', "outer.style.width = '600px'", '300,56,rgb(0, 128, 0),9');
        await expect('narrow', "outer.style.width = '200px'", '100,33,rgb(0, 0, 0),9');
        "#,
        &[(
            "host.html",
            "<body style='margin:0'><div id=outer style='width:400px;container-type:inline-size'>\
             <x-card id=card class=on><span>s</span></x-card><div id=empty></div></div>\
             <style>#empty{width:100px;container-type:inline-size}\
             @container (width < 150px){#empty::before{content:'';display:block;height:9px}}</style><script>\
             globalThis.outer = document.getElementById('outer'); globalThis.card = document.getElementById('card');\
             globalThis.empty = document.getElementById('empty');\
             card.attachShadow({mode:'open'}).innerHTML = `<style>\
             :host{display:block;container-type:inline-size;width:50cqw}\
             @container (width > 300px){:host{color:rgb(0, 128, 0)}}\
             :host::before{content:'';display:block;height:15cqw}\
             @container (width < 250px){:host::after{content:'';display:block;height:7px}}\
             :host(.on)::before{height:20cqw}\
             slot{display:block;width:100px;container-type:inline-size}\
             @container (width < 150px){::slotted(span){display:block;height:11px}}\
             </style><slot></slot>`;</script></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn slotted_and_part_pseudo_elements_paint_and_query_inner_containers() {
    run_hidden(
        r#"
        const pixel = async (x, y) => [...(await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap().subarray(0, 3)].reverse().join();
        const state = "['s', 'p', 'q'].map(id => (id == 's' ? slotted : shadow.getElementById(id)).getBoundingClientRect().height).join()";
        const expect = async (step, code, value) => {
            await run(code);
            for (let frame = 0; frame < 3; frame++) await win.webContents.capturePage();
            const got = await run(state);
            if (got !== value) fail(step + ': ' + got + ' instead of ' + value);
        };
        await load('parts.html');
        await expect('first', '0', '9,7,5');
        const green = await pixel(5, 5);
        if (green !== '0,128,0') fail('slotted ::before pixel: ' + green);
        await expect('wide', "shadow.getElementById('in').style.width = '200px'", '9,0,5');
        await expect('narrow', "shadow.getElementById('in').style.width = '100px'", '9,7,5');
        await expect('class', "slotted.className = ''", '0,7,5');
        "#,
        &[(
            "parts.html",
            "<body style='margin:0;background:white'><div id=outer style='width:400px;container-type:inline-size'>\
             <x-card id=card><span id=s class=a></span></x-card></div>\
             <style>@container (width < 150px){#card::part(p)::after{content:'';display:block;height:7px}}</style><script>\
             globalThis.slotted = document.getElementById('s');\
             globalThis.shadow = document.getElementById('card').attachShadow({mode:'open'});\
             shadow.innerHTML = `<style>:host{display:block}::slotted(span){display:block}\
             ::slotted(.a)::before{content:'';display:block;height:9px;background:rgb(0, 128, 0)}\
             #in{width:100px;container-type:inline-size}:host::part(q)::before{content:'';display:block;height:5px}\
             </style><slot></slot><div id=in><div id=p part=p></div></div><div id=q part=q></div>`;</script></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn closing_dialogs_stay_in_the_top_layer_until_their_overlay_ends() {
    run_hidden(
        r#"
        const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
        const pixel = async (x, y) => [...(await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap().subarray(0, 3)].reverse();
        const frames = async ms => { for (const end = Date.now() + ms; Date.now() < end; await wait(16)) await win.webContents.capturePage(); };
        await load('dialog.html');
        await run("document.getElementById('d').showModal()");
        await frames(1300);
        let [dialog, backdrop] = [await pixel(200, 150), await pixel(20, 20)];
        if (!(dialog[2] > 200 && dialog[1] < 60 && backdrop[0] > 200 && backdrop[1] < 60)) fail('open: ' + dialog + ' ' + backdrop);
        await run("document.getElementById('d').close()");
        await frames(300);
        [dialog, backdrop] = [await pixel(200, 150), await pixel(20, 20)];
        const state = await run("(d => [d.open, d.matches(':modal'), getComputedStyle(d).overlay, document.elementFromPoint(200, 150).id].join())(document.getElementById('d'))");
        if (!(dialog[2] > 100 && backdrop[0] > 200 && backdrop[1] < 230)) fail('closing: ' + dialog + ' ' + backdrop);
        if (state !== 'false,false,auto,later') fail('closing state: ' + state);
        await frames(1000);
        [dialog, backdrop] = [await pixel(200, 150), await pixel(20, 20)];
        if (!(dialog[1] > 200 && dialog[2] < 60 && backdrop.every(value => value > 240))) fail('closed: ' + dialog + ' ' + backdrop);
        "#,
        &[(
            "dialog.html",
            "<body style='margin:0;background:white'><style>\
             dialog{opacity:0;transition:opacity 1s linear,overlay 1s allow-discrete,display 1s allow-discrete;width:100px;height:100px;padding:0;border:0;background:blue}\
             dialog[open]{opacity:1}@starting-style{dialog[open]{opacity:0}}\
             dialog::backdrop{background:transparent;transition:background-color 1s linear,overlay 1s allow-discrete,display 1s allow-discrete}\
             dialog[open]::backdrop{background:red}@starting-style{dialog[open]::backdrop{background:transparent}}\
             #later{position:absolute;left:100px;top:50px;width:200px;height:200px;background:lime;z-index:10}\
             </style><dialog id=d></dialog><div id=later></div></body>",
        )],
    );
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

#[test]
#[ignore = "requires the macOS window server"]
fn modal_dialogs_make_the_rest_of_the_page_inert() {
    run_hidden(
        &format!(
            r#"{INPUT}
        await load('modal.html');
        await run("$('d').showModal()");
        await frames(50);
        await expect('opened', "document.activeElement.id", 'in1');
        await expect('page focus', "$('out').focus(), document.activeElement.id", 'in1');
        await expect('backdrop hit', "document.elementFromPoint(20, 20).id", 'd');
        await input({{ action: 'click', x: 20, y: 20 }});
        await expect('backdrop press', "log.splice(0).join()", 'pointerdown:d,click:d');
        await run("document.body.className = 'through'");
        await frames(50);
        await expect('see-through hit', "document.elementFromPoint(20, 20).localName", 'html');
        await run("$('in1').focus()");
        await input({{ action: 'click', x: 20, y: 20 }});
        await expect('page press', "[log.splice(0).join(), document.activeElement.localName].join()", 'pointerdown:html,click:html,body');
        await run("$('in2').focus()");
        await input({{ action: 'key', key: 'tab' }});
        await expect('tab', "document.activeElement.id", 'in1');
        await input({{ action: 'key', key: 'tab' }});
        await expect('tab again', "document.activeElement.id", 'in2');
        await run("$('pop').showPopover()");
        await frames(50);
        await expect('popover above', "[document.elementFromPoint(310, 20).localName, ($('pb').focus(), document.activeElement.id)].join()", 'html,in2');
        await run("$('pop').hidePopover(); $('d').close()");
        await frames(50);
        await expect('closed', "[document.elementFromPoint(20, 20).id, ($('out').focus(), document.activeElement.id)].join()", 'out,out');
        "#
        ),
        &[(
            "modal.html",
            "<body style='margin:0'><style>#out{position:absolute;left:10px;top:10px;width:80px;height:30px}\
             #pop{position:fixed;inset:auto;left:300px;top:10px;margin:0;padding:0;border:0;width:80px;height:40px}\
             dialog{position:fixed;inset:auto;left:150px;top:150px;margin:0;padding:0;border:0;width:200px;height:100px}\
             body.through dialog::backdrop{pointer-events:none}</style>\
             <button id=out>out</button><div id=pop popover=manual><button id=pb>pb</button></div>\
             <dialog id=d><button id=in1>in1</button><button id=in2>in2</button></dialog><script>\
             globalThis.$ = id => document.getElementById(id); globalThis.log = [];\
             for (const type of ['pointerdown', 'click']) addEventListener(type, event => log.push(type + ':' + (event.target.id || event.target.localName)), true);\
             </script></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn dialogs_take_the_focus_when_nothing_in_them_can() {
    run_hidden(
        &format!(
            r#"{INPUT}
        await load('dialog.html');
        await expect('tab index', "$('d').tabIndex", -1);
        await expect('closed', "$('d').focus(), document.activeElement.localName", 'body');
        await run("$('out').focus(); log.length = 0; $('d').showModal()");
        await frames(50);
        await expect('empty modal', "[document.activeElement.id, log.splice(0).join()].join()", 'd,blur:out,focus:d');
        await run("$('d').blur()");
        await input({{ action: 'click', x: 20, y: 20 }});
        await expect('backdrop press', "document.activeElement.id", 'd');
        await input({{ action: 'key', key: 'tab' }});
        await expect('not in the tab order', "document.activeElement.id", 'd');
        await run("$('d').close(); $('e').showModal()");
        await frames(50);
        await expect('focusable child', "document.activeElement.id", 'b');
        await input({{ action: 'click', x: 160, y: 110 }});
        await expect('press on text', "document.activeElement.id", 'e');
        await run("$('e').close(); $('d').show()");
        await frames(50);
        await expect('empty dialog', "document.activeElement.id", 'd');
        "#
        ),
        &[(
            "dialog.html",
            "<body style='margin:0'><style>dialog{{position:fixed;inset:auto;left:150px;top:100px;margin:0;width:200px;height:100px}}</style>\
             <button id=out>out</button><dialog id=d><p>text</p></dialog><dialog id=e><p>text</p><button id=b>b</button></dialog><script>\
             globalThis.$ = id => document.getElementById(id); globalThis.log = [];\
             for (const type of ['focus', 'blur']) addEventListener(type, event => log.push(type + ':' + event.target.id), true);\
             </script></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn inert_subtrees_lose_focus_and_take_no_presses_or_typing() {
    run_hidden(
        &format!(
            r#"{INPUT}
        await load('inert.html');
        await expect('focus', "$('f').focus(), $('wrap').inert = true, document.activeElement.id", 'f');
        await frames(150);
        await expect('fixup', "[document.activeElement.localName, log.join()].join()", 'body,blur:f');
        await input({{ action: 'click', x: 20, y: 15 }});
        await input({{ action: 'type', text: 'x' }});
        await expect('press and typing', "[document.activeElement.localName, $('f').value].join()", 'body,');
        await run("$('wrap').inert = false");
        await frames(50);
        await input({{ action: 'click', x: 20, y: 15 }});
        await input({{ action: 'type', text: 'y' }});
        await expect('live again', "[document.activeElement.id, $('f').value].join()", 'f,y');
        "#
        ),
        &[(
            "inert.html",
            "<body style='margin:0'><div id=wrap><input id=f style='position:absolute;left:10px;top:10px;width:100px;height:20px'></div>\
             <script>globalThis.$ = id => document.getElementById(id); globalThis.log = [];\
             addEventListener('blur', event => log.push('blur:' + event.target.id), true);</script></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn exit_transitions_take_no_focus_but_leaving_shown_boxes_do() {
    run_hidden(
        &format!(
            r#"{INPUT}
        await load('exit.html');
        await run("$('out').focus(); $('d').showModal()");
        await frames(50);
        await expect('opened', "document.activeElement.id", 'in1');
        await run("$('d').close()");
        await frames(100);
        await expect('closing', "[document.activeElement.id, ($('in1').focus(), document.activeElement.id), document.elementFromPoint(160, 110).localName].join()", 'out,out,html');
        await input({{ action: 'key', key: 'tab' }});
        await expect('tab skips it', "document.activeElement.id", 'k1');
        await input({{ action: 'click', x: 160, y: 110 }});
        await expect('press', "[log.splice(0).join(), document.activeElement.localName].join()", 'pointerdown:html,click:html,body');
        await run("$('box').className = 'off'");
        await frames(100);
        await expect('display exit', "[($('bb').focus(), document.activeElement.localName), document.elementFromPoint(110, 210).localName].join()", 'body,html');
        await run("$('kept').showModal()");
        await frames(1100);
        await run("$('kept').close(); $('out').focus()");
        await frames(100);
        await expect('leaving but shown', "[($('k1').focus(), document.activeElement.id), document.elementFromPoint(25, 155).id, document.elementFromPoint(300, 240).id].join()", 'k1,k1,kept');
        await frames(1200);
        await expect('left', "[document.elementFromPoint(25, 155).id, document.elementFromPoint(300, 240).localName].join()", 'cover,html');
        await run("$('box').animate({{ display: ['block', 'block'] }}, 3000)");
        await frames(100);
        await expect('shown by an animation', "[getComputedStyle($('box')).display, ($('bb').focus(), document.activeElement.id)].join()", 'block,k1');
        "#
        ),
        &[(
            "exit.html",
            "<body style='margin:0'><style>#out{position:absolute;left:300px;top:10px;width:60px;height:30px}\
             dialog{position:fixed;inset:auto;left:150px;top:100px;margin:0;padding:0;border:0;width:200px;height:100px;transition:overlay 1s allow-discrete,display 1s allow-discrete}\
             #kept{display:block;left:20px;top:150px;width:100px;height:60px}\
             #box{position:absolute;left:100px;top:200px;width:100px;height:40px;transition:display 1s allow-discrete}#box.off{display:none}\
             #cover{position:absolute;left:10px;top:140px;width:40px;height:40px;z-index:5}</style>\
             <button id=out>out</button><dialog id=d><button id=in1>in1</button></dialog><dialog id=kept><button id=k1>k1</button></dialog>\
             <div id=box><button id=bb>bb</button></div><div id=cover></div><script>\
             globalThis.$ = id => document.getElementById(id); globalThis.log = [];\
             for (const type of ['pointerdown', 'click']) addEventListener(type, event => log.push(type + ':' + (event.target.id || event.target.localName)), true);\
             </script></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn script_animations_show_boxes_that_the_cascade_hides() {
    run_hidden(
        r#"
        const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
        const pixel = async (x, y) => [...(await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap().subarray(0, 3)].reverse();
        const frames = async ms => { for (const end = Date.now() + ms; Date.now() < end; await wait(16)) await win.webContents.capturePage(); };
        await load('hidden.html');
        const width = () => run("document.getElementById('h').getBoundingClientRect().width");
        await run("globalThis.shown = document.getElementById('h').animate([{ display: 'block', opacity: 1 }, { display: 'block', opacity: 1 }], 100000)");
        let got = await width();
        if (got !== 100) fail('shown width: ' + got);
        let color = await pixel(50, 50);
        if (!(color[2] > 200 && color[1] < 60)) fail('shown pixel: ' + color);
        await run("shown.cancel()");
        got = await width();
        if (got !== 0) fail('canceled width: ' + got);
        await run("globalThis.events = []; const timed = document.getElementById('t').animate([{ opacity: 0 }, { opacity: 1 }], 100); timed.onfinish = () => events.push('finish'); timed.finished.then(() => events.push('finished'));\
            globalThis.filled = document.getElementById('h').animate([{ display: 'block' }], { duration: 50, fill: 'forwards' })");
        await frames(400);
        got = await run("[events.join(), filled.playState, document.getElementById('h').getBoundingClientRect().width].join()");
        if (got !== 'finished,finish,finished,100') fail('timing without a box: ' + got);
        "#,
        &[(
            "hidden.html",
            "<body style='margin:0;background:white'>\
             <div id=h style='display:none;width:100px;height:100px;background:blue'></div><div id=t style='display:none'></div></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn new_nodes_read_styles_like_a_full_pass_before_the_first_frame() {
    run_hidden(
        r#"
        await load('new.html');
        const got = await run('window.got');
        if (got !== ',rgb(255, 0, 0)') fail('styles of new nodes: ' + got);
        "#,
        &[(
            "new.html",
            "<body><script>getComputedStyle(document.body).color;\
             const got = [getComputedStyle(document.createElement('i')).color];\
             const x = document.createElement('div'); x.style.color = 'red'; document.body.append(x);\
             got.push(getComputedStyle(x).color); window.got = got.join();</script></body>",
        )],
    );
}

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

#[test]
#[ignore = "requires the macOS window server"]
fn native_content_keeps_the_boxes_around_it() {
    let text = run_hidden(
        &format!(
            r#"{STEPS}
            await load('native.html');
            await run("f.focus(); c.getContext('2d').fillRect(0, 0, 40, 20)");
            await win.webContents.capturePage();
            await step('canvas', () => run("c.getContext('2d').fillStyle = 'rgb(0, 255, 0)'; c.getContext('2d').fillRect(0, 0, 40, 20)"));
            console.log('CANVAS ' + await inside('c'));
            const plain = await inside('f');
            await step('selection', () => run('f.setSelectionRange(0, 5)'));
            console.log('SELECTION ' + plain + ' ' + await inside('f'));
            const idle = await inside('b');
            const [x, y] = JSON.parse(await run("(r => JSON.stringify([r.x + 5, r.y + 5]))(document.getElementById('b').getBoundingClientRect())"));
            await step('hover', () => fetch(MCP + '/input', {{ method: 'POST', body: JSON.stringify({{ window: win.id, action: 'move', x, y }}) }}));
            console.log('HOVER ' + idle + ' ' + await inside('b'));
            await step('value', () => run("f.value = 'W'.repeat(40)"));
            console.log('SCROLL ' + await run('f.scrollWidth'));
            await run('f.setSelectionRange(40, 40)');
            await win.webContents.capturePage();
            await step('typing', async () => {{
                await fetch(MCP + '/input', {{ method: 'POST', body: JSON.stringify({{ window: win.id, action: 'type', text: 'xy' }}) }});
                for (let tries = 0; tries < 100 && await run('f.value.length') < 42; tries++) await wait(10);
            }});
            console.log('TYPED ' + await run('f.value.slice(-3)'));
            await step('size', () => run('c.width = 60'));
            console.log('WIDTH ' + await run('c.getBoundingClientRect().width'));
            await step('cell', () => run("document.getElementById('g').value = 'abc'"));
            const short = await run("document.getElementById('a').getBoundingClientRect().height");
            await step('content', () => run("document.getElementById('a').value = 'a\\nb\\nc\\nd'"));
            console.log('GROWN ' + short + ' ' + await run("document.getElementById('a').getBoundingClientRect().height"));
            "#
        ),
        &[(
            "native.html",
            "<body id=body style='margin:0'><div id=outer><div id=inner>\
             <canvas id=c width=40 height=20 style='display:block'></canvas>\
             <input id=f value='     ' style='display:block;margin:10px 0 0;padding:0;border:0;width:200px;height:40px;font:20px/40px sans-serif;outline:none'>\
             <button id=b style='display:block;margin-top:10px;width:60px;height:30px'>Go</button>\
             <div id=grid style='display:grid;grid-template-columns:100px 100px'><input id=g><i>x</i></div>\
             <textarea id=a rows=1 style='field-sizing:content;font:10px/10px sans-serif'>a</textarea>\
             </div></div><p id=other>x</p>\
             <script>globalThis.c = document.getElementById('c'); globalThis.f = document.getElementById('f')</script></body>",
        )],
    );
    let line = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name))
            .unwrap_or_else(|| panic!("{text}"))
            .trim()
            .to_owned()
    };
    for step in ["canvas", "selection"] {
        assert_eq!(built(&text, step), [""; 0], "{step} builds no box: {text}");
    }
    assert_eq!(line("CANVAS"), "0,255,0", "the new drawing shows: {text}");
    let selection: Vec<_> = line("SELECTION").split(' ').map(str::to_owned).collect();
    assert_ne!(selection[0], selection[1], "the selection shows");
    let hover: Vec<_> = line("HOVER").split(' ').map(str::to_owned).collect();
    assert_ne!(hover[0], hover[1], "the hovered button looks hovered");
    let hovered = built(&text, "hover");
    assert!(
        hovered.contains(&"b") && !hovered.contains(&"other"),
        "{hovered:?}"
    );
    for step in ["value", "typing"] {
        let built = built(&text, step);
        assert!(
            !built.is_empty() && built.iter().all(|id| *id == "f"),
            "a new value builds only the field: {step} {built:?}"
        );
    }
    assert!(line("SCROLL").parse::<f32>().unwrap() > 300., "{text}");
    assert_eq!(line("TYPED"), "Wxy");
    assert!(built(&text, "size").contains(&"c"), "{text}");
    assert_eq!(
        built(&text, "cell"),
        ["g"],
        "a field in a grid patches in place"
    );
    let content = built(&text, "content");
    assert!(
        content.contains(&"a") && content.contains(&"inner"),
        "a field sized by its content lays out again: {content:?}"
    );
    let grown: Vec<f32> = line("GROWN")
        .split(' ')
        .map(|height| height.parse().unwrap())
        .collect();
    assert!(grown[1] > grown[0] + 20., "{grown:?}");
    assert_eq!(line("WIDTH"), "60");
}

#[test]
#[ignore = "requires the macOS window server"]
fn table_cells_keep_their_boxes_and_follow_their_rows() {
    let text = run_hidden(
        &format!(
            r#"{STEPS}
            await load('table.html');
            console.log('PLAIN ' + await inside('c2'));
            await step('row', () => run("document.getElementById('r2').className = 'on'"));
            console.log('ROW ' + await inside('c2') + ' ' + await inside('c5'));
            await step('collapse', () => run("document.getElementById('t').style.borderCollapse = 'separate'"));
            await step('block', () => run("document.getElementById('b').style.borderCollapse = 'separate'"));
            const boxes = () => run("JSON.stringify(['c2', 'c3', 'c4'].map(id => document.getElementById(id).getBoundingClientRect()).map(r => [r.x, r.width]))");
            await step('fixed', () => run("document.getElementById('t').style.width = '300px'; document.getElementById('c4').style.width = 'auto'"));
            const narrow = await boxes();
            await step('wide', () => run("const c1 = document.getElementById('c1'); c1.style.width = 'auto'; c1.textContent = 'wide'.repeat(4)"));
            console.log('WIDE ' + narrow + ' ' + await boxes());
            "#
        ),
        &[(
            "table.html",
            "<body style='margin:0'><table id=t style='border-collapse:collapse'><style>td{border:4px solid #000;width:40px;height:20px} \
             tr.on{background-color:rgb(0,255,0)} .own{border-collapse:separate}</style>\
             <tr id=r1><td id=c1></td><td id=c4></td></tr><tr id=r2><td id=c2 class=own></td><td id=c3></td></tr>\
             <tr id=r3><td id=c5></td><td id=c6></td></tr></table>\
             <table id=b style='display:block;border-collapse:collapse'><tr><td id=c7 class=own></td></tr></table></body>",
        )],
    );
    let line = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name))
            .unwrap_or_else(|| panic!("{text}"))
            .trim()
            .to_owned()
    };
    assert_eq!(line("PLAIN"), "255,255,255", "{text}");
    assert_eq!(
        line("ROW"),
        "0,255,0 255,255,255",
        "a cell shows its row's new background: {text}"
    );
    let row = built(&text, "row");
    assert!(
        row.contains(&"c2") && !row.contains(&"c5") && !row.contains(&"c1"),
        "only the cells of the row build again: {row:?}"
    );
    assert!(built(&text, "collapse").contains(&"c2"), "{text}");
    assert!(
        built(&text, "block").contains(&"c7"),
        "a cell follows its table's border-collapse: {text}"
    );
    let wide = built(&text, "wide");
    assert!(
        wide.contains(&"c1")
            && !["c2", "c3", "c4", "c5", "c6"]
                .iter()
                .any(|id| wide.contains(id)),
        "in a table of a set width, cells that only take a new place keep their boxes: {wide:?}"
    );
    let boxes: Vec<Vec<[f32; 2]>> = line("WIDE")
        .split(' ')
        .map(|boxes| serde_json::from_str(boxes).unwrap())
        .collect();
    let [[first, next, fill], [first_after, next_after, fill_after]] =
        [0, 1].map(|at| [0, 1, 2].map(|cell| boxes[at][cell]));
    assert!(
        first[0] == first_after[0]
            && first_after[1] > first[1] + 20.
            && next_after[0] > next[0] + 20.
            && fill_after[1] < fill[1] - 20.,
        "like Chrome, a cell of a set width fills its widened column, and the next column moves and narrows: {boxes:?}"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn video_frames_keep_the_boxes_around_them() {
    let clip = url::Url::from_file_path(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/media/clip.mp4"),
    )
    .unwrap();
    let page = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("video.html");
    std::fs::write(
        &page,
        format!(
            "<body id=body style='margin:0'><div id=outer>\
             <video id=v src='{clip}' muted loop style='display:block;width:64px;height:48px'></video></div>\
             <p id=other>x</p></body>"
        ),
    )
    .unwrap();
    let text = run_hidden(
        &format!(
            r#"{STEPS}
            await win.loadFile({page:?});
            await run("globalThis.v = document.getElementById('v'); v.play().catch(error => console.error(String(error))); 1");
            for (let tries = 0; tries < 200 && !(await run('v.currentTime') > 0.2); tries++) {{
                await win.webContents.capturePage();
                await wait(10);
            }}
            const start = await run('v.currentTime');
            await step('video', async () => {{
                for (let frame = 0; frame < 10; frame++) {{
                    await win.webContents.capturePage();
                    await wait(30);
                }}
            }});
            console.log('PLAYED ' + start + ' ' + await run('v.currentTime'));
            console.log('VIDEO ' + await pixel(30, 20));
            "#
        ),
        &[("package.json", r#"{"features":["media"]}"#)],
    );
    let line = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name))
            .unwrap_or_else(|| panic!("{text}"))
            .trim()
            .to_owned()
    };
    let played: Vec<f64> = line("PLAYED")
        .split(' ')
        .map(|time| time.parse().unwrap())
        .collect();
    assert!(played[0] > 0.2 && played[1] != played[0], "{text}");
    assert_eq!(built(&text, "video"), [""; 0], "{text}");
    let video: Vec<u8> = line("VIDEO")
        .split(',')
        .map(|value| value.parse().unwrap())
        .collect();
    assert!(
        video[2] > 180 && (90..170).contains(&video[1]) && video[0] < 80,
        "the clip's orange frame shows: {video:?}"
    );
}

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

#[test]
#[ignore = "requires the macOS window server"]
fn grid_tracks_take_content_only_where_their_sizing_reads_it() {
    let text = run_hidden(
        &format!(
            r#"{RECTS}
            await load('grid.html');
            console.log('RECTS ' + await rects());
            "#
        ),
        &[(
            "grid.html",
            "<body style='margin:0;font:16px sans-serif'><style>.w{display:grid;width:200px}</style>\
             <div class=w style='grid-template-columns:repeat(2,minmax(0,1fr))'><span id=a>Wwwwwwwwwwwwwwwwwwwwwww</span><span id=b>x</span></div>\
             <div class=w style='grid-template-columns:repeat(2,1fr)'><span id=c>Wwwwwwwwwwwwwwwwwwwwwww</span><span id=d>x</span></div>\
             <div class=w style='grid-template-columns:30px 70px;column-gap:10px'>\
             <div style='grid-column:1/3;display:grid;grid-template-columns:subgrid'><i id=e>e</i><i id=f>f</i></div></div>\
             <div class=w style='grid-template-columns:100px 100px'><i id=g style='margin:5px'>g</i><b id=h style='margin:0 10px'>h</b></div>\
             </body>",
        )],
    );
    let rects = rects(&text);
    let field = |id: &str, index: usize| -> f32 {
        rects[id].split(',').nth(index).unwrap().parse().unwrap()
    };
    assert_eq!([field("a", 2), field("b", 0)], [100., 100.], "{rects:?}");
    assert!(field("c", 2) > 150. && field("d", 0) > 150., "{rects:?}");
    assert_eq!(
        [field("e", 2), field("f", 0), field("f", 2)],
        [30., 40., 70.],
        "{rects:?}"
    );
    assert_eq!(
        [field("g", 0), field("g", 2), field("h", 0), field("h", 2)],
        [5., 90., 110., 80.],
        "each item takes its own margins: {rects:?}"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn canvases_keep_the_ratio_of_their_attributes() {
    let text = run_hidden(
        &format!(
            r#"{RECTS}
            await load('canvas.html');
            await run("const x = document.getElementById('p').getContext('2d'); x.fillStyle = 'red'; x.fillRect(0, 0, 40, 20);\
                createImageBitmap(new OffscreenCanvas(50, 40)).then(bitmap => document.getElementById('b').getContext('bitmaprenderer').transferFromImageBitmap(bitmap))");
            await new Promise(resolve => setTimeout(resolve, 100));
            await win.webContents.capturePage();
            console.log('RECTS ' + await rects());
            const pixel = async (x, y) => [...(await win.webContents.capturePage({{ x, y, width: 1, height: 1 }})).toBitmap()].slice(0, 3).join();
            const [x, y] = JSON.parse(await run("(r => JSON.stringify([r.x, r.y]))(document.getElementById('p').getBoundingClientRect())"));
            console.log('PADDING ' + await pixel(x + 8, y + 8) + ' ' + await pixel(x + 16, y + 16) + ' ' + await pixel(x + 16, y + 52));
            "#
        ),
        &[(
            "canvas.html",
            "<body style='margin:0'><style>.w{display:flow-root;width:400px}</style>\
             <div class=w><canvas id=p width=40 height=20 style='display:block;padding:10px;border:4px solid black;background:blue;width:80px;height:auto'></canvas></div>\
             <div class=w><canvas id=a width=200 height=100 style='display:block;width:100%;height:auto'></canvas></div>\
             <div class=w style='width:100px'><canvas id=m width=200 height=100 style='display:block;max-width:100%;height:auto'></canvas></div>\
             <div class=w><canvas id=r width=200 height=100 style='display:block;width:100px;height:auto;aspect-ratio:1/1'></canvas></div>\
             <div class=w><canvas id=n width=200 height=100 style='display:block;width:100px;height:auto;aspect-ratio:auto 1/1'></canvas></div>\
             <div class=w><canvas id=d style='display:block;width:100%;height:auto'></canvas></div>\
             <div class=w style='display:flex'><canvas id=f width=200 height=100 style='width:100%;height:auto'></canvas></div>\
             <div class=w style='display:grid;grid-template-columns:300px'><canvas id=g width=200 height=100 style='width:100%;height:auto'></canvas></div>\
             <div class=w><canvas id=h width=200 height=100 style='display:block;height:50px'></canvas></div>\
             <div class=w><canvas id=b width=200 height=100 style='display:block;width:100px;height:auto'></canvas></div>\
             </body>",
        )],
    );
    let rects = rects(&text);
    for (id, rect) in [
        ("a", "0,0,400,200"),
        ("m", "0,0,100,50"),
        ("r", "0,0,100,100"),
        ("n", "0,0,100,50"),
        ("d", "0,0,400,200"),
        ("f", "0,0,400,200"),
        ("g", "0,0,300,150"),
        ("h", "0,0,100,50"),
        ("b", "0,0,100,50"),
        ("p", "0,0,108,68"),
    ] {
        assert_eq!(rects[id], rect, "canvas {id}, as in Chrome: {text}");
    }
    let padding: Vec<_> = text
        .lines()
        .find_map(|line| line.strip_prefix("PADDING "))
        .unwrap_or_else(|| panic!("{text}"))
        .split(' ')
        .collect();
    assert_eq!(
        padding,
        ["255,0,0", "0,0,255", "0,0,255"],
        "the drawing fills the content box, not the padding: {text}"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn margins_never_collapse_through_replaced_elements() {
    let cases = [
        ("canvas", "width=40 height=20", "</canvas>", "40,20"),
        ("canvas", "width=40 height=0", "</canvas>", "40,0"),
        ("video", "", "</video>", "300,150"),
        ("iframe", "style='border:0'", "</iframe>", "300,150"),
        (
            "svg",
            "width=40 height=20 style='overflow:visible'",
            "</svg>",
            "40,20",
        ),
        ("input", "style='padding:0;border:0'", "", ""),
        ("select", "style='padding:0;border:0'", "</select>", ""),
        (
            "canvas",
            "width=40 height=20",
            "<p style='margin:30px 0'>fallback</p></canvas>",
            "40,20",
        ),
    ];
    let mut page = String::from(
        "<body style='margin:0'><style>.w{display:flow-root;width:400px}.w *{display:block}\
         .n{margin-top:10px;height:5px}</style>",
    );
    for (index, (tag, attrs, close, _)) in cases.iter().enumerate() {
        page.push_str(&format!(
            "<div class=w><div><div><{tag} id=e{index} {attrs}>{close}<div class=n id=n{index}></div></div></div></div>"
        ));
    }
    page.push_str("</body>");
    let text = run_hidden(
        &format!("{RECTS} await load('margins.html'); console.log('RECTS ' + await rects());"),
        &[("margins.html", &page)],
    );
    let rects = rects(&text);
    for (index, (tag, _, _, size)) in cases.iter().enumerate() {
        let rect: Vec<f32> = rects[&format!("e{index}")]
            .split(',')
            .map(|value| value.parse().unwrap())
            .collect();
        let next: f32 = rects[&format!("n{index}")]
            .split(',')
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(rect[1], 0., "{tag} {index} stays at the top: {text}");
        assert_eq!(
            next,
            rect[3] + 10.,
            "{tag} {index} keeps the next margin below it: {text}"
        );
        if !size.is_empty() {
            assert_eq!(
                format!("{},{}", rect[2], rect[3]),
                *size,
                "{tag} {index}: {text}"
            );
        }
    }
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

#[test]
#[ignore = "requires the macOS window server"]
fn long_lists_keep_their_rows_while_they_scroll() {
    let text = run_hidden(
        &format!(
            r#"{STEPS}{LIST}
            await load('list.html');
            await run("globalThis.s = document.getElementById('s'); globalThis.o = document.getElementById('o'); 1");
            await win.webContents.capturePage();
            await step('idle', () => run("o.textContent = 'y'"));
            await look('idle');
            for (let index = 1; index <= 30; index++) {{
                await step('slow' + index, () => run(`s.scrollTop = ${{index * 40}}`));
                await look('slow' + index);
            }}
            await step('fast', () => run("s.scrollTop = 30000"));
            await look('fast');
            await step('still', () => win.webContents.capturePage());
            await step('back', () => run("s.scrollTop = 0"));
            await look('back');
            await step('focus', () => run("document.getElementById('r1500').focus({{ preventScroll: true }})"));
            await step('shown', () => run("s.scrollTop = 29900"));
            await look('shown');
            "#
        ),
        &[("list.html", &long_list(true))],
    );
    check_looks(&text, 0.);
    let rows = |built: &[&str]| built.iter().filter(|id| id.starts_with('r')).count();
    let idle = built(&text, "idle");
    assert!(
        idle.iter().all(|id| ["o", "body"].contains(id)),
        "an unrelated change keeps the list: {idle:?}"
    );
    let slow: Vec<Vec<&str>> = (1..=30)
        .map(|index| built(&text, &format!("slow{index}")))
        .collect();
    let planned = slow.iter().filter(|built| built.contains(&"list")).count();
    assert!(
        (1..10).contains(&planned),
        "a slow scroll culls again only now and then: {planned} {slow:?}"
    );
    assert!(
        slow.iter().map(|built| rows(built)).sum::<usize>() < 100,
        "rows build once as they come near the view: {slow:?}"
    );
    for step in ["fast", "back"] {
        let built = built(&text, step);
        assert!(
            built.contains(&"list") && rows(&built) < 60,
            "a fast scroll builds only the rows that come into view: {step} {built:?}"
        );
    }
    assert_eq!(built(&text, "still"), [""; 0]);
    assert!(
        built(&text, "focus").contains(&"r1500"),
        "a focused box is never culled: {text}"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn long_pages_keep_their_rows_while_they_scroll() {
    let text = run_hidden(
        &format!(
            r#"{STEPS}{LIST}
            await load('page.html');
            await run("globalThis.o = document.getElementById('o'); 1");
            await win.webContents.capturePage();
            await step('idle', () => run("o.textContent = 'y'"));
            await look('idle');
            for (let index = 1; index <= 12; index++) {{
                await step('slow' + index, () => run(`scrollTo(0, ${{index * 60}})`));
                await look('slow' + index);
            }}
            await step('fast', () => run("scrollTo(0, 20000)"));
            await look('fast');
            await step('still', () => win.webContents.capturePage());
            "#
        ),
        &[("page.html", &long_list(false))],
    );
    check_looks(&text, 20.);
    let idle = built(&text, "idle");
    assert!(
        idle.iter().all(|id| ["o", "body"].contains(id)),
        "an unrelated change keeps the list: {idle:?}"
    );
    let fast = built(&text, "fast");
    assert!(
        fast.iter().filter(|id| id.starts_with('r')).count() < 60,
        "{fast:?}"
    );
    assert_eq!(built(&text, "still"), [""; 0]);
}

#[test]
#[ignore = "requires the macOS window server"]
fn boxes_that_never_cull_keep_the_boxes_around_them() {
    let mut page = String::from(
        "<body id=body style='margin:0'><p id=o style='margin:0'>x</p>\
         <div id=s style='height:200px;overflow:auto'><div id=t style='transform:translateZ(0)'>",
    );
    for index in 0..400 {
        page.push_str(&format!(
            "<div id=r{index} style='height:20px'>row {index}</div>"
        ));
    }
    page.push_str("</div></div></body>");
    let text = run_hidden(
        &format!(
            r#"{STEPS}
            await load('moved.html');
            await win.webContents.capturePage();
            await step('idle', () => run("document.getElementById('o').textContent = 'y'"));
            await step('scroll', () => run("document.getElementById('s').scrollTop = 4000"));
            "#
        ),
        &[("moved.html", &page)],
    );
    for step in ["idle", "scroll"] {
        let built = built(&text, step);
        assert!(
            built.iter().all(|id| ["o", "body"].contains(id)),
            "a transformed box culls nothing, so it is kept: {step} {built:?}"
        );
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn first_builds_log_reads_that_replay_the_same() {
    // Debug builds build the second wrapper again when it takes the style of the first.
    let wrapper = "<div class=w><div style='height:50px;overflow:auto'>short</div>\
                   <div class=b></div><span>inline</span><p>block</p></div>";
    let text = run_hidden(
        &format!(
            r#"{STEPS}
            await load('reads.html');
            await win.webContents.capturePage();
            await step('text', () => run("document.getElementById('o').textContent = 'y'"));
            await step('scroller', () => run("document.getElementById('a').innerHTML = '<div id=s style=\"height:50px;overflow:auto\">short</div>'"));
            await step('grid', () => run("for (const box of document.querySelectorAll('.b')) box.innerHTML = '<div style=\"display:grid\"><span>cell</span></div>'"));
            "#
        ),
        &[(
            "reads.html",
            &format!(
                "<body id=body style='margin:0'><p id=o style='margin:0'>x</p>\
                 <svg style='display:none'><symbol id=dot viewBox='0 0 10 10'><circle cx=5 cy=5 r=4 /></symbol></svg>\
                 <div id=i><svg width=10 height=10><use href='#dot' /></svg></div><div id=a></div>{wrapper}{wrapper}</body>"
            ),
        )],
    );
    let texted = built(&text, "text");
    assert!(
        !texted.contains(&"i"),
        "a box with a <use> icon is kept: {texted:?}"
    );
    let scroller = built(&text, "scroller");
    assert_eq!(
        scroller.iter().filter(|id| **id == "s").count(),
        1,
        "a new scroll box builds once: {scroller:?}"
    );
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

#[test]
#[ignore = "requires the macOS window server"]
fn replaced_elements_and_controls_keep_their_natural_size_as_blocks() {
    let (text, rects) = picture_rects(
        "blocks.html",
        "<body style='margin:0'><style>.w{display:flow-root;width:300px}.w>*{display:block}</style>\
         <div class=w><img id=i0></div><div class=w><img id=i1 height=40></div>\
         <div class=w><img id=i2 src='PICTURE' style='height:20px;padding:5px'></div>\
         <div class=w><button id=b0></button></div><div class=w><object id=o0></object></div>\
         <div class=w><object id=o1 style='padding:5px'></object></div><div class=w><iframe id=f0></iframe></div>\
         <div class=w><video id=v0 style='height:50px'></video></div>\
         <div class=w><canvas id=c0 width=200 height=100 style='height:50px;padding:10px'></canvas></div>\
         <div class=w><canvas id=c1 width=200 height=100 style='height:50px;padding:10px;box-sizing:border-box'></canvas></div>\
         <div class=w><canvas id=c2 width=200 height=100 style='height:50px;padding:5px;border:5px solid'></canvas></div>\
         <div class=w><input id=k0 type=checkbox></div><div class=w><audio id=a0 controls></audio></div>\
         <div class=w><audio id=a1></audio></div><div class=w><embed id=e0></div>\
         <div class=w dir=rtl><canvas id=r0 width=40 height=20></canvas></div>\
         <div class=w><canvas id=m0 width=40 height=20 style='margin:0 auto'></canvas></div>\
         <div class=w><img id=t0 src='missing.png' width=800 height=600 style='max-width:100%;height:auto'></div></body>",
    );
    for (id, rect) in [
        ("i0", "0,0,0,0"),
        ("i1", "0,0,0,40"),
        ("i2", "0,0,35,30"),
        ("b0", "0,0,16,6"),
        ("o0", "0,0,300,150"),
        ("o1", "0,0,310,160"),
        ("f0", "0,0,304,154"),
        ("v0", "0,0,300,50"),
        ("c0", "0,0,120,70"),
        ("c1", "0,0,80,50"),
        ("c2", "0,0,120,70"),
        ("k0", "4,3,13,13"),
        ("a0", "0,0,300,54"),
        ("r0", "260,0,40,20"),
        ("m0", "130,0,40,20"),
        ("t0", "0,0,300,225"),
    ] {
        assert_eq!(rects[id], rect, "{id}, as in Chrome: {text}");
    }
    for id in ["a1", "e0"] {
        assert!(rects[id].ends_with(",0,0"), "{id} has no box: {text}");
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn failed_images_show_their_alt_text_like_chrome() {
    let (text, rects) = picture_rects(
        "alt.html",
        "<body style='margin:0;font:16px/18px sans-serif'><style>.w{width:300px}.g{display:grid;grid-template-columns:min-content}</style>\
         <div class=w><img id=a0 src='missing.png' alt='Hello'></div>\
         <div class=w><img id=a1 src='missing.png' alt='Hello' width=120 height=40 style='width:90px'></div>\
         <div class=w><img id=a2 src='missing.png' alt='Hello' width=150 height=40 style='display:block'></div>\
         <div class=w><img id=a3 src='missing.png' alt=''></div><div class=w><img id=a4 src='missing.png'></div>\
         <div class=w><img id=a5 src='missing.png' width=60></div><div class=w><img id=a6 src='missing.png' width=60 height=30></div>\
         <div class=w><img id=a7 src='missing.png' alt='' width=60 height=30></div><div class=w><img id=a8 alt='Hello'></div>\
         <div class=w><img id=w0 src='missing.png' alt='Hello'><img id=w1 src='missing.png' alt='Hello'></div>\
         <div class=w><img id=a9 src='missing.png' alt='Hello' width=120 height=40 style='display:inline-block'></div>\
         <div class=w style='width:60px'>x <img id=a10 src='missing.png' alt='Hello world again'></div>\
         <div class='w g' style='grid-template-columns:min-content 1fr'><img id=g0 src='missing.png' alt='Hello world'><i>z</i></div>\
         <div class='w g' style='grid-template-columns:min-content 1fr'><span id=g1>Hello world</span><i>z</i></div>\
         <div class='w g'><img id=g2 src='missing.png' alt='a verylongword'></div><div class='w g'><span id=g3>a verylongword</span></div>\
         <div class='w g' style='grid-template-columns:max-content'><img id=g4 src='missing.png' alt='Hello world'></div>\
         <div class='w g' style='grid-template-columns:max-content'><span id=g5>Hello world</span></div>\
         <div class='w g'><img id=g6 src='missing.png' alt='Hello world' style='white-space:nowrap'></div>\
         <div class=w style='width:40px'><img id=a11 src='missing.png' alt='Hello world'></div></body>",
    );
    let width = |id: &str| rects[id].split(',').nth(2).unwrap().parse::<f32>().unwrap();
    assert!(width("a0") > 40., "alt text: {text}");
    assert!(rects["a0"].ends_with(",18"), "one line high: {text}");
    for id in ["a1", "a8"] {
        assert_eq!(width(id), width("a0"), "{id} ignores its size: {text}");
    }
    for (id, rect) in [
        ("a2", "0,0,150,40"),
        ("a4", "0,0,16,16"),
        ("a5", "0,0,16,16"),
        ("a6", "0,0,60,30"),
        ("a7", "0,0,60,30"),
        ("a9", "0,0,120,40"),
    ] {
        assert_eq!(rects[id], rect, "{id}, as in Chrome: {text}");
    }
    assert!(rects["a3"].ends_with(",0,0"), "empty alt text: {text}");
    assert!(
        rects["w1"].starts_with(&format!("{},0,", width("w0"))),
        "inline after inline: {text}"
    );
    let height = |id: &str| rects[id].split(',').nth(3).unwrap().parse::<f32>().unwrap();
    assert!(
        rects["a10"].starts_with("0,18,60,") && height("a10") >= 36.,
        "long alt text wraps in its line: {text}"
    );
    for (id, word, high) in [("g0", "g1", 52.), ("g2", "g3", 36.)] {
        assert_eq!(
            width(id),
            width(word),
            "{id} is as wide as its longest word: {text}"
        );
        assert_eq!(
            height(id),
            high,
            "{id} text goes below the icon only when its first word doesn't fit beside it: {text}"
        );
    }
    assert!(
        (width("g4") - width("g5") - 16.).abs() < 0.02 && height("g4") == 18.,
        "the max-content width holds the icon: {text}"
    );
    assert!(
        (width("g6") - width("g5")).abs() < 0.02 && height("g6") == 18.,
        "the icon doesn't widen the min-content width of text that can't wrap: {text}"
    );
    assert_eq!(
        rects["a11"], "0,0,40,52",
        "an image wider than its line stays on it: {text}"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn images_that_fail_after_a_frame_show_their_alt_text() {
    let page = "<body style='margin:0;font:16px/18px sans-serif'><style>.w{width:300px}.g{display:grid;grid-template-columns:min-content}</style>\
         <div class=w><img id=a0 src='PICTURE' alt='Hello'></div><div class=w><img id=a1 src='missing.png' alt='Hello'></div>\
         <div class=w><img id=a2 alt='Hello'></div><div class='w g'><img id=g0 src='PICTURE' alt='a verylongword'></div><div class='w g'><span id=g1>a verylongword</span></div>\
         <div class='w g'><input id=s0 type=Submit></div><div class='w g'><input id=s1 type=submit></div></body>";
    let text = run_hidden(
        &format!(
            "{RECTS}{STEPS} const server = __apps.http
                .createServer((request, response) => setTimeout(() => {{ response.statusCode = 404; response.end(); }}, 300))
                .listen(0, '127.0.0.1');
            const slow = 'http://127.0.0.1:' + server.address().port + '/x.png';
            await load('late.html');
            await run(`Promise.all([...document.querySelectorAll('img')].map(image => image.decode().catch(() => {{}})))`);
            await new Promise(resolve => setTimeout(resolve, 100));
            await win.webContents.capturePage();
            await run(`document.getElementById('a2').setAttribute('src', '${{slow}}')`);
            await win.webContents.capturePage();
            await run(`Promise.all(['a0', 'a2', 'g0'].map(id => {{
                const image = document.getElementById(id);
                if (id !== 'a2') image.setAttribute('src', 'missing.png');
                return image.decode().catch(() => {{}});
            }}))`);
            await new Promise(resolve => setTimeout(resolve, 100));
            await win.webContents.capturePage();
            console.log('RECTS ' + await rects());
            await step('other', () => run(`document.getElementById('s1').value = 'Go'`));"
        ),
        &[("late.html", &page.replace("PICTURE", PICTURE))],
    );
    let other = built(&text, "other");
    assert!(
        !other.contains(&"a0") && !other.contains(&"g0"),
        "{other:?}"
    );
    let rects = rects(&text);
    assert_eq!(
        rects["a0"], rects["a1"],
        "alt text after the picture: {text}"
    );
    assert_eq!(
        rects["a2"], rects["a1"],
        "alt text after a first frame without the picture: {text}"
    );
    let width = |id: &str| rects[id].split(',').nth(2).unwrap().parse::<f32>().unwrap();
    assert_eq!(
        width("g0"),
        width("g1"),
        "no size from the old picture: {text}"
    );
    assert!(
        width("s0") > 20. && width("s0") == width("s1"),
        "Submit label: {text}"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn inline_blocks_shrink_to_their_line_like_chrome() {
    let long = "A long inline block text that wraps";
    let (text, rects) = picture_rects(
        "shrink.html",
        &format!(
            "<body style='margin:0;font:16px/18px sans-serif'><style>.w{{width:150px}}.ib{{display:inline-block}}</style>\
             <div class=w>x <span id=s0 class=ib>{long}</span> y</div>\
             <div class=w>x <span id=s1 class=ib style='margin:0 10px;padding:0 5px;border:1px solid'>{long}</span></div>\
             <div class=w style='padding:0 20px'>x <span id=s2 class=ib>{long}</span></div>\
             <div class=w>x <span id=s3 class=ib style='width:300px'>wide</span></div>\
             <div class=w>x <button id=b0>{long}</button></div>\
             <div class=w style='width:auto'><span id=t0 class=ib style='text-indent:16px'>Indented</span> <span id=t1 class=ib>Indented</span></div>\
             <div class=w style='width:40px'><span id=l0 class=ib>Hello world</span></div>\
             <div class=w style='width:40px'><span id=l1 class=ib>Averyverylongword</span></div></body>"
        ),
    );
    let at = |id: &str, index: usize| {
        rects[id]
            .split(',')
            .nth(index)
            .unwrap()
            .parse::<f32>()
            .unwrap()
    };
    for (id, [x, width]) in [
        ("s0", [0., 150.]),
        ("s1", [10., 130.]),
        ("s2", [20., 150.]),
        ("s3", [0., 300.]),
        ("b0", [0., 150.]),
    ] {
        assert_eq!(
            [at(id, 0), at(id, 2)],
            [x, width],
            "{id}, as in Chrome: {text}"
        );
        assert!(at(id, 1) >= 18., "{id} starts a new line: {text}");
    }
    assert!(at("s0", 3) >= 36., "the text wraps: {text}");
    assert_eq!(
        at("t0", 2),
        at("t1", 2) + 16.,
        "the indent widens the box: {text}"
    );
    assert_eq!(at("t0", 3), 18., "an indented line doesn't wrap: {text}");
    assert_eq!(
        rects["l0"], "0,0,40,36",
        "a box wider than its line shrinks on it: {text}"
    );
    assert!(
        rects["l1"].starts_with("0,0,") && at("l1", 2) > 40.,
        "a word wider than its line stays on it: {text}"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn paragraphs_with_inline_boxes_shrink_to_fit_like_chrome() {
    let (text, rects) = picture_rects(
        "fit.html",
        "<body style='margin:0;font:16px/18px sans-serif'><style>.w{display:flex;flex-direction:column;align-items:center;width:200px}\
         p{margin:0}.x{display:inline-block;width:50px;height:10px}</style>\
         <div class=w><p id=p0>Some text before the <span class=x></span> box and some more text after it</p></div>\
         <div class=w><p id=p1>Some text with <span style='font-size:20px'>bigger words</span> that wraps in the column</p></div>\
         <div class=w style='width:30px'><p id=p2>a <span class=x></span> b</p></div>\
         <div class=w style='width:30px'><p id=p3>a <span style='font-size:20px'>big</span> b</p></div></body>",
    );
    for (id, rect) in [
        ("p0", [0., 0., 200., 54.]),
        ("p1", [0., 0., 200., 57.]),
        ("p2", [-10., 0., 50., 54.]),
        ("p3", [0., 0., 30., 55.5]),
    ] {
        let at = rects[id].split(',').map(|v| v.parse::<f32>().unwrap());
        assert!(
            at.zip(rect).all(|(at, side)| (at - side).abs() < 0.05),
            "{id} at {rect:?}, as in Chrome: {text}"
        );
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn wide_first_items_and_spaces_break_lines_like_chrome() {
    let (text, rects) = picture_rects(
        "breaks.html",
        "<body style='margin:0;font:16px/18px sans-serif'><style>.w{width:40px}.ib{display:inline-block}\
         .v{display:inline-block;width:100px;height:10px}</style>\
         <div class=w><span class=ib>Averyverylongword</span> <span id=s0 class=ib>b</span></div>\
         <div class=w style='width:150px'><span class=ib>A long inline block text that wraps</span> <span id=s1 class=ib>b</span></div>\
         <div class=w dir=rtl><span class=ib>Averyverylongword</span> <span id=s2 class=ib>b</span></div>\
         <div class=w><span id=t0 class=v style='vertical-align:top'></span></div>\
         <div class=w><span id=t1 class=v style='vertical-align:bottom'></span></div>\
         <div class=w><span id=t2 class=v style='vertical-align:top'></span> <span id=t3 class=ib>b</span></div></body>",
    );
    for (id, [x, y]) in [
        ("s0", [0., 18.]),
        ("s1", [0., 36.]),
        ("s2", [31.09, 18.]),
        ("t0", [0., 0.]),
        ("t1", [0., 8.]),
        ("t2", [0., 0.]),
        ("t3", [0., 18.]),
    ] {
        let at: Vec<f32> = rects[id].split(',').map(|v| v.parse().unwrap()).collect();
        assert!(
            (at[0] - x).abs() < 0.05 && at[1] == y,
            "{id} at {x},{y}, as in Chrome: {text}"
        );
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn top_and_bottom_boxes_and_hanging_spaces_align_like_chrome() {
    let (text, rects) = picture_rects(
        "edges.html",
        "<body style='margin:0;font:16px/18px sans-serif'><style>.w{width:300px}.ib{display:inline-block}\
         .x{display:inline-block;width:20px;height:5px}.t{vertical-align:top}.b{vertical-align:bottom}</style>\
         <div class=w><span id=m0 class='x t' style='height:6px'></span><small>small</small><span class=x style='height:30px'></span></div>\
         <div class=w>text <span id=m1 class='x b' style='height:40px'></span> more <span id=m2 class=x></span></div>\
         <div class=w><span class='x b' style='height:40px'></span><span id=f0 class=x></span></div>\
         <div class=w><span class='x t' style='height:30px'></span><span class='x b' style='height:40px'></span><span id=f1 class=x></span></div>\
         <div class=w><span class='x b' style='height:40px'></span><span class='x t' style='height:50px'></span><span id=f2 class=x></span></div>\
         <div class=w style='width:60px'><span class=x style='width:50px'></span> <span class='x b' style='height:40px'></span> <span id=f3 class=x></span></div>\
         <div class=w style='width:110px;text-align:center'><span id=h0 class=ib style='width:100px'>x</span> <span class=ib>b</span></div>\
         <div class=w style='width:110px;text-align:right'><span id=h1 class=ib style='width:100px'>x</span> <span class=ib>b</span></div></body>",
    );
    for (id, [x, y]) in [
        ("m0", [0., 0.]),
        ("m1", [30.23, 0.]),
        ("m2", [95.58, 31.5]),
        ("f0", [20., 31.5]),
        ("f1", [40., 31.5]),
        ("f2", [40., 9.5]),
        ("f3", [24.45, 49.5]),
        ("h0", [5., 0.]),
        ("h1", [10., 0.]),
    ] {
        let at: Vec<f32> = rects[id].split(',').map(|v| v.parse().unwrap()).collect();
        assert!(
            (at[0] - x).abs() < 0.05 && at[1] == y,
            "{id} at {x},{y}, as in Chrome: {text}"
        );
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn flex_and_grid_lines_stretch_replaced_elements_and_controls() {
    let (text, rects) = picture_rects(
        "lines.html",
        "<body style='margin:0'><style>.w{width:300px}.c{display:flex;flex-direction:column}\
         .r{display:flex;height:100px}.g{display:grid;height:100px}.n{display:flex;width:100px}</style>\
         <div class='w c'><input id=cI></div><div class='w c'><input id=cK type=checkbox></div>\
         <div class='w c'><img id=cP src='PICTURE'></div><div class='w c'><video id=cV></video></div>\
         <div class='w r'><input id=rK type=checkbox></div><div class='w r'><canvas id=rC></canvas></div>\
         <div class='w r'><img id=rP src='PICTURE'></div><div class='w r'><video id=rV></video></div>\
         <div class='w r'><canvas id=rD width=200 height=100 style='padding:5px'></canvas></div>\
         <div class='w g'><img id=gP src='PICTURE'></div><div class='w g'><iframe id=gF></iframe></div>\
         <div class='w g'><input id=gK type=checkbox></div>\
         <div class='w g'><canvas id=gC width=200 height=100 style='height:50px;padding:10px'></canvas></div>\
         <div class='w n'><video id=nV></video></div><div class='w n'><input id=nR type=range></div></body>",
    );
    assert!(
        rects["cI"].starts_with("0,0,300,"),
        "a field stretches: {text}"
    );
    for (id, rect) in [
        ("cK", "4,3,293,13"),
        ("cP", "0,0,300,240"),
        ("cV", "0,0,300,150"),
        ("rK", "4,3,13,94"),
        ("rC", "0,0,200,100"),
        ("rP", "0,0,125,100"),
        ("rV", "0,0,300,100"),
        ("rD", "0,0,190,100"),
        ("gP", "0,0,50,40"),
        ("gF", "0,0,304,154"),
        ("gK", "4,3,293,94"),
        ("gC", "0,0,120,70"),
        ("nV", "0,0,300,150"),
        ("nR", "2,2,129,16"),
    ] {
        assert_eq!(rects[id], rect, "{id}, as in Chrome: {text}");
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn scroll_sizes_count_overflowing_content_like_chrome() {
    let text = run_hidden(
        r#"await load('page.html');
        console.log('SIZES ' + await run(`(() => {
            const box = style => '<div style="' + style + '"><div style="height:24px;padding-bottom:10px"></div></div>';
            const cases = ['overflow:hidden;height:0', 'overflow:hidden;height:0;padding:4px', 'height:0', 'display:inline'];
            document.body.innerHTML = cases.map(box).join('') + '<div id=chat style="overflow:auto;height:50px"><p style="height:40px;margin:0"></p></div>';
            const sizes = [...document.body.children].slice(0, cases.length).map(node => node.scrollHeight);
            const chat = document.getElementById('chat');
            sizes.push(chat.scrollHeight);
            chat.insertAdjacentHTML('beforeend', '<p style="height:40px;margin:0"></p>');
            sizes.push(chat.scrollHeight);
            return sizes.join(',');
        })()`));"#,
        &[("page.html", "<!doctype html><body style='margin:0'></body>")],
    );
    assert!(text.contains("SIZES 34,42,34,0,50,80"), "{text}");
}

#[test]
#[ignore = "requires the macOS window server"]
fn lines_pushed_down_by_tall_rows_paint_below_the_clip_top() {
    run_hidden(
        r#"
        await load('page.html');
        await win.webContents.capturePage();
        // Counts the dark device pixels of the text. Each hard line holds a 100 px inline-block, so the
        // lines in the window are far below where their line height alone would put them. The last line is
        // below the window, so only lines in the middle can paint.
        const ink = (await win.webContents.capturePage({ x: 0, y: 0, width: 400, height: 300 })).toBitmap().filter((value, index) => index % 4 === 0 && value < 128).length;
        if (ink < 50) fail('no text in view: ' + ink);
        "#,
        &[(
            "page.html",
            "<body style='margin:0;background:#fff'>\
             <p style='margin:0;position:relative;top:-250px;font-size:20px;line-height:20px;color:#000'>\
             XX<span style='display:inline-block;width:10px;height:100px'></span><br>\
             XX<span style='display:inline-block;width:10px;height:100px'></span><br>\
             XX<span style='display:inline-block;width:10px;height:100px'></span><br>\
             XX<span style='display:inline-block;width:10px;height:100px'></span><br>\
             XX<span style='display:inline-block;width:10px;height:100px'></span><br>\
             XX<span style='display:inline-block;width:10px;height:100px'></span><br>\
             XX<span style='display:inline-block;width:10px;height:100px'></span><br>\
             XX<span style='display:inline-block;width:10px;height:100px'></span><br>\
             XX<span style='display:inline-block;width:10px;height:100px'></span><br>\
             XX<span style='display:inline-block;width:10px;height:100px'></span></p></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn root_percent_heights_take_the_viewport_like_chrome() {
    let text = run_hidden(
        r#"await load('page.html');
        const sizes = () => run("(([m, f], page) => [m.height, f.y, page.scrollHeight].map(value => value - innerHeight).join())(['m', 'f'].map(id => document.getElementById(id).getBoundingClientRect()), document.documentElement)");
        console.log('SHELL ' + await sizes());
        const before = await run('innerHeight');
        win.setContentSize(400, 200);
        while (await run('innerHeight') === before) await win.webContents.capturePage();
        await win.webContents.capturePage();
        console.log('SMALL ' + await sizes());
        await run("document.documentElement.style.height = 'calc(50% + 20px)'");
        await win.webContents.capturePage();
        console.log('CALC ' + await run("document.body.getBoundingClientRect().height - innerHeight / 2"));"#,
        &[(
            "page.html",
            "<!doctype html><style>html,body{height:100%}body{margin:0;display:flex;flex-direction:column}\
             #m{flex:1;overflow:auto}#f{height:40px}</style><body><div id=m><div style='height:900px'></div></div><div id=f></div></body>",
        )],
    );
    for line in ["SHELL -40,-40,0", "SMALL -40,-40,0", "CALC 20"] {
        assert!(text.contains(line), "{line}, as in Chrome: {text}");
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn percent_widths_of_inline_boxes_take_the_line_like_chrome() {
    let (text, rects) = picture_rects(
        "percent.html",
        "<!doctype html><body style='margin:0;font:14px/20px Helvetica'><div class=w style='width:400px'>\
         <style>i,input{display:inline-block;box-sizing:border-box;height:20px}.w div{height:30px}</style>\
         <div><i id=alone style='width:100%'></i></div>\
         <div><i id=half style='width:50%'></i> <i id=next style='width:25%'></i></div>\
         <div style='text-align:center'><i id=center style='width:50%'></i></div>\
         <div style='text-align:right'><i id=right style='width:25%;margin-right:10px'></i></div>\
         <div><i id=limit style='width:100%;max-width:50%'></i></div>\
         <div><input id=input style='width:100%;max-width:300px'></div>\
         <div><label><input id=label style='width:100%'></label></div>\
         <div>text <i id=text style='width:50%'></i></div>\
         <div><img id=image width=600 height=20 style='max-width:100%'></div>\
         <div><i id=wide style='width:120%'></i></div>\
         <div style='display:flex'><p style='flex-grow:1;margin:0'><input id=grow style='width:100%'></p><p style='width:100px;margin:0'></p></div></div></body>",
    );
    for (id, rect) in [
        ("alone", "0,0,400,20"),
        ("half", "0,30,200,20"),
        ("next", "203.89,30,100,20"),
        ("center", "100,60,200,20"),
        ("right", "290,90,100,20"),
        ("limit", "0,120,200,20"),
        ("input", "0,150.75,300,20"),
        ("label", "0,180.75,400,20"),
        ("text", "26.46,210,200,20"),
        ("image", "0,240,400,20"),
        ("wide", "0,270,480,20"),
        ("grow", "0,300.75,300,20"),
    ] {
        assert_eq!(rects[id], rect, "{id}, as in Chrome: {text}");
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn tables_lists_and_hyphens_size_like_chrome() {
    let text = run_hidden(
        r#"
        await load('t.html');
        console.log('BOXES ' + await run("JSON.stringify(['a','b','b1','b2','c','d','e','foot','body','cap','h','s'].map(id => (r => [r.x, r.y, r.width, r.height])(document.getElementById(id).getBoundingClientRect())))"));
        "#,
        &[(
            "t.html",
            "<body style='margin:0;font:12px Helvetica'><style>td{padding:2px;border:1px solid}\
             i{display:block;width:30px;height:10px}b{display:block;width:60px;height:10px}</style>\
             <table id=a style='width:200px'><tr><td><i></i></td></tr></table>\
             <table id=b style='width:200px;table-layout:fixed'><tr><td id=b1 style='width:50px'><i></i></td>\
             <td id=b2><b></b><b></b><b></b><b></b></td></tr></table>\
             <div style='display:grid;grid-template-columns:300px;justify-items:start'>\
             <table id=c><tr><td><i></i></td><td><b></b></td></tr></table></div>\
             <div style='display:flex;width:100px'><div style='flex:1'><table id=d style='width:100%'>\
             <tr><td><b></b> <b></b></td><td><b></b></td></tr></table></div><div style='flex:1'></div></div>\
             <table id=e><colgroup><col style='width:80px'></colgroup><tfoot><tr><td id=foot><i></i></td></tr></tfoot>\
             <tbody><tr style='height:40px'><td id=body><i></i></td></tr></tbody>\
             <caption id=cap style='caption-side:bottom'><i></i></caption></table>\
             <div id=h style='width:min-content;font:20px Helvetica'>abc-def</div>\
             <ol style='list-style-position:inside;padding:0;font:20px Helvetica'><li><span id=s>x</span></li></ol></body>",
        )],
    );
    let boxes: Vec<[f32; 4]> = serde_json::from_str(
        text.lines()
            .find_map(|line| line.strip_prefix("BOXES "))
            .unwrap_or_else(|| panic!("{text}")),
    )
    .unwrap();
    // Chrome's boxes: a set width includes the border spacing, a fixed layout takes the first row, a table in a grid
    // or flex item fits its content, columns take `<col>` widths, footers go last, rows take their height, captions
    // follow `caption-side`, words break after hyphens, and inside markers indent the first line.
    let chrome: [[f32; 4]; 12] = [
        [0., 0., 200., 20.],
        [0., 20., 200., 50.],
        [2., 22., 56., 46.],
        [60., 22., 138., 46.],
        [0., 70., 108., 20.],
        [0., 90., 138., 30.],
        [0., 120., 84., 72.],
        [2., 164., 80., 16.],
        [2., 122., 80., 40.],
        [0., 182., 84., 10.],
        [0., 192., 38.91, 46.],
        [22.24, 258., 10., 23.],
    ];
    for (found, chrome) in boxes.iter().zip(chrome) {
        assert!(
            found.iter().zip(chrome).all(|(a, b)| (a - b).abs() < 0.6),
            "{found:?} is not {chrome:?}: {boxes:?}"
        );
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn grid_items_measure_subgrids_flex_lines_and_mixed_fonts_like_chrome() {
    let (text, rects) = picture_rects(
        "measure.html",
        "<body style='margin:0;font:12px/1.3 Helvetica'><style>.w{display:grid;grid-template-columns:250px;align-items:start}\
         .f{display:flex}.f>*{border:1px solid;padding:2px 4px}.g{display:grid;gap:4px}.g>*{border:1px solid;padding:2px 4px}</style>\
         <div class=w style='grid-template-columns:80px 1fr 40px;gap:4px;width:250px'><div class=g style='grid-column:1/-1;grid-template-columns:subgrid;padding:2px 4px;border:1px solid'>\
         <div id=s0>sub a</div><div id=s1>sub b</div><div id=s2>c</div></div></div>\
         <div class=w><div id=f0 class=f><div style='flex:1'>text that wraps when the container is too narrow for it</div><div style='flex:1'>short</div></div></div>\
         <div class=w><div id=f1 class=f><div>min-content longwordwithoutbreaks more</div><div style='flex-shrink:0'>noshrink noshrink noshrink noshrink</div></div></div>\
         <div class=w style='font:13px/1.5 Helvetica'><div id=m0 class=f style='align-items:center'><span>a</span><span><b style='font-size:15px'>$49</b></span></div></div>\
         <div class=w style='grid-template-columns:60px 190px;font:14px/20px Helvetica'><span>Role</span>\
         <select id=n0 style='font:inherit;appearance:none;padding:6px 10px;border:1px solid'><option>Maintainer</option></select></div>\
         <div class=w style='font:13px/1.4 Helvetica'><div id=c0><div style='width:min-content;border:1px solid;padding:2px'>min content box</div></div></div>\
         <div class=w style='font:13px/1.4 Helvetica'><div id=c1 class=f style='flex-wrap:wrap;gap:4px'><div style='flex:1 1 100px'>a</div><div style='flex:1 1 100px'>b</div><div style='flex:1 1 100px'>c</div></div></div>\
         <div class=w><div id=c2 style='display:flex'><img style='width:50%;height:auto' width=200 height=100><div style='flex:1'>x</div></div></div>\
         <div class=w style='font:13px/1.4 Helvetica'><div id=c3>a <button style='display:inline-flex;padding:4px 8px'>Icon</button> text</div></div></body>",
    );
    for (id, rect) in [
        ("s0", [5., 3., 75.]),
        ("s1", [84., 3., 122.]),
        ("s2", [210., 3., 35.]),
        ("f0", [0., 0., 250.]),
        ("f1", [0., 0., 250.]),
    ] {
        let at: Vec<f32> = rects[id].split(',').map(|v| v.parse().unwrap()).collect();
        assert!(
            at.iter()
                .zip(rect)
                .all(|(at, side)| (at - side).abs() < 0.05),
            "{id} at {rect:?}, as in Chrome: {text}"
        );
    }
    let height = |id: &str| rects[id].split(',').nth(3).unwrap().parse::<f32>().unwrap();
    for (id, value) in [
        ("s0", 21.59),
        ("f0", 52.78),
        ("f1", 52.78),
        ("m0", 28.5),
        ("n0", 34.),
        ("c0", 60.59),
        ("c1", 52.39),
        ("c2", 62.5),
        ("c3", 27.5),
    ] {
        assert!(
            (height(id) - value).abs() < 0.05,
            "{id} is {value} tall, as in Chrome: {text}"
        );
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn controls_take_percent_widths_and_list_rows_like_chrome() {
    let (text, rects) = picture_rects(
        "controls.html",
        "<body style='margin:0;font:14px/1.45 Helvetica'><style>.w{width:200px}</style>\
         <div class=w><input id=p0 style='width:100%;box-sizing:border-box'></div>\
         <div class=w><input id=p1 style='width:calc(100% - 20px);box-sizing:border-box'></div>\
         <div class=w><select id=p2 style='width:100%'><option>x</option></select></div>\
         <div class=w><button id=p3 style='width:50%'>b</button></div>\
         <fieldset class=w><input id=p4 style='width:100%;box-sizing:border-box'></fieldset>\
         <div class=w><select id=l0 size=2 style='font:inherit;padding:0;border:1px solid'><option id=o0>a</option><option>b</option></select></div></body>",
    );
    let at = |id: &str, index: usize| {
        rects[id]
            .split(',')
            .nth(index)
            .unwrap()
            .parse::<f32>()
            .unwrap()
    };
    for (id, width) in [
        ("p0", 200.),
        ("p1", 180.),
        ("p2", 200.),
        ("p3", 100.),
        ("p4", 200.),
    ] {
        assert_eq!(at(id, 2), width, "{id}, as in Chrome: {text}");
    }
    assert!(
        (at("o0", 3) - 17.8).abs() < 0.05,
        "options keep line-height: normal: {text}"
    );
    assert!(
        (at("l0", 3) - 37.59).abs() < 0.05,
        "two rows of options: {text}"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn grids_align_a_lone_pseudo_element_item_like_chrome() {
    let text = run_hidden(
        r#"await load('page.html');
        await win.webContents.capturePage();
        const image = await win.webContents.capturePage({ x: 0, y: 0, width: 32, height: 32 });
        const size = image.getSize(), bitmap = image.toBitmap(), scale = size.width / 32;
        const at = (x, y) => (Math.round(y * scale) * size.width + Math.round(x * scale)) * 4;
        const red = (x, y) => bitmap[at(x, y) + 2] > 200 && bitmap[at(x, y) + 1] < 50;
        console.log('DOTS ' + [red(15, 15), red(2, 2)].join());"#,
        &[(
            "page.html",
            "<body style='margin:0'><style>span{display:grid;place-items:center;width:30px;height:30px}\
             span::after{content:'';width:10px;height:10px;background:#f00}</style><span></span></body>",
        )],
    );
    assert!(text.contains("DOTS true,false"), "{text}");
}

#[test]
#[ignore = "requires the macOS window server"]
fn scroll_boxes_reach_content_below_fixed_height_boxes_like_chrome() {
    let text = run_hidden(
        r#"await load('page.html');
        const sizes = () => run("(box => [document.documentElement.scrollHeight, scrollY, box.scrollHeight, box.scrollTop].join())(document.getElementById('box'))");
        console.log('START ' + await sizes());
        await run("scrollTo(0, 700); document.getElementById('box').scrollTop = 200");
        await win.webContents.capturePage();
        console.log('MOVED ' + await sizes());
        await run("document.getElementById('deep').style.height = '80px'");
        await win.webContents.capturePage();
        await win.webContents.capturePage();
        console.log('SHORT ' + await sizes());"#,
        &[(
            "page.html",
            "<!doctype html><style>html,body{height:100%}body{margin:0}</style><body>\
             <div id=box style='height:100px;overflow:auto'><div style='height:50px'><div id=deep style='height:500px'></div></div></div>\
             <div style='height:1500px'></div></body>",
        )],
    );
    for line in [
        "START 1600,0,500,0",
        "MOVED 1600,700,500,200",
        "SHORT 1600,700,100,0",
    ] {
        assert!(text.contains(line), "{line}, as in Chrome: {text}");
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn backgrounds_borders_and_outlines_paint_like_chrome() {
    let dots = "url(data:image/svg+xml,%3Csvg%20xmlns=%22http://www.w3.org/2000/svg%22%20width=%2220%22%20height=%2220%22%3E%3Ccircle%20cx=%2210%22%20cy=%2210%22%20r=%228%22%20fill=%22teal%22/%3E%3C/svg%3E)";
    let page = format!(
        "<body style='margin:0;background:white'><style>div{{position:absolute;box-sizing:border-box}}</style>\
         <div style='left:0;top:0;width:100px;height:120px;border:10px solid transparent;background:linear-gradient(red,blue)'></div>\
         <div style='left:110px;top:0;width:100px;height:120px;border:10px solid black;background:linear-gradient(red,blue)'></div>\
         <div style='left:220px;top:0;width:100px;height:120px;border:10px solid transparent;background:yellow padding-box'></div>\
         <div style='left:0;top:130px;width:100px;height:40px;background:linear-gradient(to right,red,transparent)'></div>\
         <div style='left:110px;top:130px;width:100px;height:40px;background:{dots}'></div>\
         <div style='left:220px;top:130px;width:100px;height:40px;background:{dots} no-repeat right 10px bottom 0'></div>\
         <div id=dashes style='left:0;top:200px;width:100px;height:20px;border:1px dashed black'></div>\
         <div style='left:150px;top:200px;width:50px;height:50px;outline:4px solid black;outline-offset:2px'></div></body>"
    );
    let text = run_hidden(
        r#"
        await load('paint.html');
        await new Promise(done => setTimeout(done, 300));
        await win.webContents.capturePage();
        for (const [x, y] of [[50, 5], [115, 5], [225, 60], [50, 150], [120, 140], [300, 160], [230, 160], [145, 195]]) {
            const image = await win.webContents.capturePage({ x, y, width: 1, height: 1 });
            const [b, g, r] = image.toBitmap();
            console.log(`PIXEL ${x} ${y} ${r} ${g} ${b}`);
        }
        const row = await win.webContents.capturePage({ x: 0, y: 200, width: 100, height: 1 });
        const bytes = row.toBitmap(), width = row.getSize().width;
        let runs = 0;
        for (let x = 0, dark = false; x < width; x++) {
            const now = bytes[x * 4] < 128;
            runs += now && !dark;
            dark = now;
        }
        console.log('DASHES ' + runs);
        "#,
        &[("paint.html", &page)],
    );
    let pixel = |x: u32, y: u32| -> [u8; 3] {
        let line = text
            .lines()
            .find_map(|line| line.strip_prefix(&format!("PIXEL {x} {y} ")))
            .unwrap_or_else(|| panic!("{text}"));
        let parts: Vec<u8> = line.split(' ').map(|v| v.parse().unwrap()).collect();
        [parts[0], parts[1], parts[2]]
    };
    let near = |x: u32, y: u32, expected: [u8; 3]| {
        let actual = pixel(x, y);
        assert!(
            actual
                .iter()
                .zip(expected)
                .all(|(a, e)| a.abs_diff(e) <= 12),
            "pixel at {x},{y} is {actual:?}, Chrome paints {expected:?}"
        );
    };
    // The gradient repeats under a transparent border, so the top border shows its bottom end.
    near(50, 5, [13, 0, 242]);
    // A border paints over the layers under it.
    near(115, 5, [0, 0, 0]);
    // `padding-box` keeps the color out of the border.
    near(225, 60, [255, 255, 255]);
    // Gradients mix premultiplied colors, so red fades to transparent without gray.
    near(50, 150, [255, 128, 128]);
    // A small SVG image repeats at its own size.
    near(120, 140, [0, 128, 128]);
    // Four-value positions measure from the named edges.
    near(300, 160, [0, 128, 128]);
    near(230, 160, [255, 255, 255]);
    // Outlines keep the square corners of the box.
    near(145, 195, [0, 0, 0]);
    assert!(
        text.contains("DASHES 20"),
        "1px dashes are 3px long with 2px gaps: {text}"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn backdrop_blurs_and_drop_shadows_spread_like_chrome() {
    run_hidden(
        r#"
        const pixel = async (x, y) => [...(await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap().subarray(0, 3)].reverse();
        await load('effects.html');
        // Chrome reads only the backdrop under the box, so the red page around it doesn't bleed in.
        for (const [x, y] of [[51, 100], [100, 51], [148, 100], [100, 148]]) {
            const color = await pixel(x, y);
            if (color[0] > 20 || color[2] < 200) fail('backdrop edge ' + [x, y] + ': ' + color);
        }
        // Chrome blurs drop-shadow(0 0 6px) with a standard deviation of 6px, twice the box-shadow one.
        const shadow = await pixel(256, 240);
        if (shadow[0] > 230 || shadow[0] < 190) fail('drop shadow: ' + shadow);
        "#,
        &[(
            "effects.html",
            "<body style='margin:0;background:red'>\
             <div style='position:absolute;left:50px;top:50px;width:100px;height:100px;background:blue'>\
             <div style='height:100%;backdrop-filter:blur(8px)'></div></div>\
             <div style='position:absolute;left:200px;top:160px;width:150px;height:100px;background:white'>\
             <div style='margin:30px 0 0 30px;width:50px;height:44px;background:black;filter:drop-shadow(0 0 6px black)'></div></div>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn font_size_animations_resize_em_boxes_like_chrome() {
    run_hidden(
        r#"
        await load('grow.html');
        await win.webContents.capturePage();
        const sizes = JSON.parse(await run(`JSON.stringify(['a', 'b'].map(id => document.getElementById(id).getBoundingClientRect().height))`));
        if (sizes[0] !== 30 || sizes[1] !== 17.5) fail('sizes: ' + sizes);
        "#,
        &[(
            "grow.html",
            "<style>body{margin:0;font:12px Helvetica}@keyframes grow{from{font-size:10px}to{font-size:20px}}\
             div{animation:grow 10s linear -5s paused}</style>\
             <div id=a style='height:2em'></div><div id=b>Text</div>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn images_take_object_position_and_image_rendering_like_chrome() {
    run_hidden(
        r#"
        const pixel = async (x, y) => [...(await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap().subarray(0, 3)].reverse();
        await load('images.html');
        const red = c => c[0] > 230 && c[1] < 20 && c[2] < 20, blue = c => c[2] > 230 && c[0] < 20 && c[1] < 20;
        // contain, left: the whole image sits at the left of the box.
        let [left, right, rest] = [await pixel(5, 20), await pixel(35, 20), await pixel(60, 20)];
        if (!red(left) || !blue(right) || rest[0] !== 221) fail('contain left: ' + left + ' ' + right + ' ' + rest);
        // none, right 5px bottom: the 2x2 image sits 5px from the right.
        [rest, left] = [await pixel(192, 38), await pixel(193, 38)];
        if (rest[0] !== 221 || !red(left)) fail('none right: ' + rest + ' ' + left);
        // pixelated: texels keep hard edges.
        [left, right] = [await pixel(31, 70), await pixel(32, 70)];
        if (!red(left) || !blue(right)) fail('pixelated: ' + left + ' ' + right);
        // smooth: edges repeat the edge texel, as in Chrome, and don't fade out.
        [left, right] = [await pixel(100, 70), await pixel(163, 70)];
        if (!red(left) || !blue(right)) fail('smooth edges: ' + left + ' ' + right);
        "#,
        &[(
            "images.html",
            "<style>body{margin:0}img{display:block;background:#ddd}</style>\
             <div style='display:flex'><img src='data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAFElEQVR4nGP4z8DAwPCfCUQyMAAAFQcCATPcN/oAAAAASUVORK5CYII=' style='width:100px;height:40px;object-fit:contain;object-position:left'>\
             <img src='data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAFElEQVR4nGP4z8DAwPCfCUQyMAAAFQcCATPcN/oAAAAASUVORK5CYII=' style='width:100px;height:40px;object-fit:none;object-position:right 5px bottom'></div>\
             <div style='display:flex'><img src='data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAFElEQVR4nGP4z8DAwPCfCUQyMAAAFQcCATPcN/oAAAAASUVORK5CYII=' style='width:64px;height:32px;image-rendering:pixelated'>\
             <img src='data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAFElEQVR4nGP4z8DAwPCfCUQyMAAAFQcCATPcN/oAAAAASUVORK5CYII=' style='margin-left:36px;width:64px;height:32px'></div>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn text_decorations_take_their_place_and_thickness_like_chrome() {
    let text = run_hidden(
        r#"
        await load('page.html');
        // Dark device rows of a 1 px column right of the 'x', from the baseline of the plain row.
        const rows = async (top, x, width) => {
            const bitmap = (await win.webContents.capturePage({ x, y: top, width, height: 60 })).toBitmap();
            const scale = Math.round(Math.sqrt(bitmap.length / 4 / (width * 60)));
            const dark = [];
            for (let row = 0; row < 60 * scale; row++)
                for (let column = 0; column < width * scale; column++)
                    if (bitmap[(row * width * scale + column) * 4] < 128) { dark.push(row); break; }
            return [scale, dark];
        };
        const [scale, plain] = await rows(0, 0, 20);
        const base = plain[plain.length - 1] + 1;
        const lines = [];
        for (const top of [60, 120, 180]) {
            const [, dark] = await rows(top, 25, 1);
            lines.push(dark.length ? [dark[0] - base, dark[dark.length - 1] - base] : []);
        }
        console.log('LINES ' + JSON.stringify([scale, lines]));
        "#,
        &[(
            "page.html",
            "<body style='margin:0;background:#fff'><style>div{font:24px/60px Helvetica;height:60px;color:#000}</style>\
             <div>x</div><div style='text-decoration:underline'>x&nbsp;&nbsp;&nbsp;&nbsp;</div>\
             <div style='text-decoration:line-through'>x&nbsp;&nbsp;&nbsp;&nbsp;</div>\
             <div style='text-decoration:overline'>x&nbsp;&nbsp;&nbsp;&nbsp;</div></body>",
        )],
    );
    let line = text
        .lines()
        .find_map(|line| line.strip_prefix("LINES "))
        .unwrap_or_else(|| panic!("{text}"));
    let (scale, lines): (u32, Vec<Vec<i32>>) = serde_json::from_str(line).unwrap();
    // Chrome's rows for 24 px Helvetica at 2x, and at 1x the rows of 12 px at 2x.
    let chrome = if scale == 2 {
        [[3, 6], [-17, -14], [-48, -45]].map(Vec::from)
    } else {
        [[2, 3], [-9, -8], [-24, -23]].map(Vec::from)
    };
    assert_eq!(
        lines, chrome,
        "underline, line-through and overline: {text}"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn lines_grow_around_inline_fonts_and_tabs_reach_their_stops_like_chrome() {
    let (text, rects) = picture_rects(
        "lines.html",
        "<body style='margin:0;font:14px/1.4 system-ui'><style>.w{width:300px}p{margin:0}code{font:12px Menlo}</style>\
         <div class=w style='font:16px/20px Helvetica'>text<br id=br0>x</div>\
         <div class=w><p id=g0>text <span style='font-family:Times'>times</span> text</p></div>\
         <div class=w><p id=h0>text <span style='line-height:30px'>lh30</span> text</p></div>\
         <div class=w style='display:grid'><p id=e0>text <span style='font-size:20px'>big</span> text</p></div>\
         <div class=w><div id=m0>I think it is <code>line-height</code> again. Can you check \
         <code>getComputedStyle(el).lineHeight</code> on that element and send me a screenshot?<small style='display:block'>x</small></div></div>\
         <div class=w><pre id=p0 style='margin:0'>a\t<span id=t0>b</span>\n12345678\t<span id=t1>c</span></pre></div>\
         <div class=w><div id=n0 style='width:min-content'>min-content box</div></div>\
         <div class=w style='display:grid'><div id=q0><button style='font:14px/20px system-ui;padding:8px 16px;border:0'>Go</button> text</div></div>\
         <div class=w><details id=d0><summary>Summary</summary>hidden</details></div>\
         <div class=w style='display:grid'><details id=d1><summary>Summary</summary>hidden</details></div>\
         <div class=w style='display:grid'><p id=s0>text <sub>sub</sub> <sup>sup</sup> text</p></div>\
         <div class=w><p>text <span id=v1 style='vertical-align:text-top;font-size:10px'>tt</span> text</p></div></body>",
    );
    for (id, rect) in [
        ("br0", [25.79, 0.5, 0., 18.5]),
        ("g0", [0., 0., 300., 20.59]),
        ("h0", [0., 0., 300., 30.]),
        ("e0", [0., 0., 300., 28.]),
        ("m0", [0., 0., 300., 75.11]),
        ("p0", [0., 0., 300., 39.19]),
        ("n0", [0., 0., 49.46, 58.78]),
        ("q0", [0., 0., 300., 36.]),
        ("d0", [0., 0., 300., 19.59]),
        ("d1", [0., 0., 300., 19.59]),
        ("s0", [0., 0., 300., 24.79]),
    ] {
        let at = rects[id].split(',').map(|v| v.parse::<f32>().unwrap());
        assert!(
            at.zip(rect).all(|(at, side)| (at - side).abs() < 0.05),
            "{id} at {rect:?}, as in Chrome: {text}"
        );
    }
    let top: f32 = rects["v1"].split(',').nth(1).unwrap().parse().unwrap();
    assert!(
        (top - 2.5).abs() < 0.6,
        "text-top puts the line box of a span at the text top: {text}"
    );
    // The rows of text inside a line snap to the device pixels of the test window.
    for (id, rect) in [("t0", [67.43, 8.43]), ("t1", [134.86, 8.43])] {
        let at: Vec<f32> = rects[id].split(',').map(|v| v.parse().unwrap()).collect();
        assert!(
            (at[0] - rect[0]).abs() < 0.05 && (at[2] - rect[1]).abs() < 0.05,
            "{id} at {rect:?}, as in Chrome: {text}"
        );
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn keys_focus_and_fragments_scroll_like_chrome() {
    run_hidden(
        &format!(
            r#"{INPUT}
        await load('scroll.html');
        await expect('autofocus', "document.activeElement.id", 'auto');
        await input({{ action: 'click', x: 200, y: 120 }});
        await input({{ action: 'key', key: 'space' }});
        await frames(700);
        await expect('space', "String(scrollY)", '248');
        await input({{ action: 'key', key: 'down' }});
        await frames(400);
        await expect('arrow', "[scrollY, log.splice(0).at(-1)].join()", '288,scrollend');
        await run("$('far').focus()");
        await frames(100);
        await expect('focus centers', "String((r => Math.round(r.top + r.height / 2))($('far').getBoundingClientRect()))", '134');
        await run("$('far').focus(); scrollTo(0, 0); $('near').focus()");
        await frames(100);
        await expect('visible focus', "String(scrollY)", '0');
        await input({{ action: 'click', x: 10, y: 160 }});
        await frames(100);
        await expect('fragment link', "[location.hash, scrollY].join()", '#far,2000');
        "#
        ),
        &[(
            "scroll.html",
            "<body style='margin:0;height:4000px'><input id=auto autofocus style='position:absolute;top:20px'><a id=link href='#far' style='position:absolute;top:150px;font:20px Helvetica'>far</a>\
             <button id=near style='position:absolute;top:100px'>near</button><button id=far style='position:absolute;top:2000px;height:20px'>far</button>\
             <script>globalThis.$ = id => document.getElementById(id); globalThis.log = []; addEventListener('scrollend', () => log.push('scrollend'));</script></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn hover_and_capture_follow_the_pointer_like_chrome() {
    run_hidden(
        &format!(
            r#"{INPUT}
        await load('hover.html');
        await input({{ action: 'move', x: 50, y: 15 }});
        await expect('hover', "document.querySelector('#list :hover').id", 'i0');
        await input({{ action: 'scroll', x: 50, y: 15, delta_x: 0, delta_y: 60 }});
        await frames(300);
        await expect('hover after scroll', "[$('list').scrollTop, document.querySelector('#list :hover').id].join()", '60,i2');
        await input({{ action: 'press', x: 50, y: 250 }});
        await input({{ action: 'pressed_move', x: 300, y: 250 }});
        await input({{ action: 'release', x: 300, y: 250 }});
        await expect('capture', "log.join()", 'pointerdown,gotpointercapture,pointermove,pointerup,lostpointercapture,click');
        await expect('caret at the nearest edge', "String((r => document.caretRangeFromPoint(r.left + r.width * 0.6, r.top + r.height / 2).startOffset)($('w').getBoundingClientRect()))", '2');
        "#
        ),
        &[(
            "hover.html",
            "<body style='margin:0;font:40px Helvetica'><div id=list style='height:100px;overflow:auto'></div><div id=cap style='width:100px;height:40px'></div><span id=w>WMW</span>\
             <style>#list div{height:30px}</style><script>globalThis.$ = id => document.getElementById(id); globalThis.log = [];\
             for (let i = 0; i < 20; i++) $('list').append(Object.assign(document.createElement('div'), { id: 'i' + i }));\
             $('cap').style.position = 'absolute'; $('cap').style.top = '230px';\
             $('cap').addEventListener('pointerdown', e => $('cap').setPointerCapture(e.pointerId));\
             for (const type of ['pointerdown', 'gotpointercapture', 'pointermove', 'pointerup', 'lostpointercapture', 'click']) $('cap').addEventListener(type, e => { if (log.at(-1) !== type) log.push(type); });</script></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn first_scripts_and_offsets_read_layout_like_chrome() {
    run_hidden(
        r#"
        await load('a.html');
        const early = await run('JSON.stringify(early)');
        if (early !== '[100,"100px","95px","rgb(1, 0, 0)"]') fail('first script: ' + early);
        const late = await run(`JSON.stringify((() => {
            const t = document.getElementById('t'), k = document.getElementById('k');
            return [t.offsetWidth, t.offsetHeight, t.clientWidth, t.offsetLeft, t.offsetTop, k.offsetLeft, k.offsetTop, document.elementsFromPoint(390, 200).at(-1)?.localName, visualViewport.width === innerWidth];
        })())`);
        if (late !== '[60,30,60,10,60,0,30,"html",true]') fail('offsets: ' + late);
        "#,
        &[(
            "a.html",
            "<style>.q{container-type:inline-size;width:300px} @container (min-width: 200px) { #c { color: rgb(1, 0, 0) } }</style>\
             <body style='margin:0'><div id=o style='overflow:auto;width:100px;height:50px;position:relative'><div style='height:200px;padding-top:30px'><div id=k style='height:10px'></div></div></div>\
             <div id=t style='width:60px;height:30px;margin:10px;transform:rotate(30deg) scale(2)'></div>\
             <div style='width:200px;position:relative'><div id=a style='position:absolute;width:50%;right:5px'></div></div><div class=q><i id=c>c</i></div>\
             <script>const a = document.getElementById('a'); globalThis.early = [a.getBoundingClientRect().width, getComputedStyle(a).width, getComputedStyle(a).left, getComputedStyle(document.getElementById('c')).color]; document.getElementById('o').scrollTop = 20;</script></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn pseudo_elements_size_and_place_like_chrome() {
    let (text, rects) = picture_rects(
        "pseudo.html",
        "<body style='margin:0;font:16px Helvetica'><style>.w{width:200px}.f{display:flex;height:20px}\
         #a::before{content:'12';flex:none;width:36px;padding-right:14px}\
         #b::before{content:'12';flex:none;width:5ch;padding-right:2ch}\
         #c::before{content:'12';display:inline-block;width:36px;padding-right:14px}\
         #d::before{content:'';flex:1}#e::before{content:'x';margin-left:auto}#f::before{content:'x';order:2}\
         #g{display:grid;place-items:center;width:40px;height:40px}\
         #g::after{content:'';width:10px;height:10px;background:rgb(0,128,0)}\
         #h::before{content:'x';display:block;width:50px}\
         #i::before{content:'';display:inline-block;width:10px;height:10px;vertical-align:-4px}</style>\
         <div class='w f' id=a><span id=a1>a</span></div>\
         <div class='w f' style='font:12px Menlo' id=b><span id=b1>b</span></div>\
         <div class=w id=c><span id=c1>c</span></div>\
         <div class='w f' id=d><span id=d1>d</span></div><div class='w f' id=e><span id=e1>e</span></div>\
         <div class='w f' id=f><span id=f1>f</span></div><div class=w><div id=g></div></div>\
         <div class=w id=h><span id=h1>h</span></div>\
         <div class=w id=i style='line-height:20px'><span id=i1>i</span></div></body>",
    );
    let at = |id: &str, index: usize| {
        rects[id]
            .split(',')
            .nth(index)
            .unwrap()
            .parse::<f32>()
            .unwrap()
    };
    for (id, x, why) in [
        ("a1", 50., "padding adds to a content-box width"),
        (
            "b1",
            50.56,
            "`ch` is the advance of `0` in the pseudo-element's font",
        ),
        ("c1", 50., "an inline-block takes its padding too"),
        ("d1", 191.1, "`flex: 1` grows"),
        ("e1", 191.1, "an auto margin takes the free space"),
        ("f1", 0., "`order` moves the pseudo-element after the span"),
    ] {
        assert!(
            (at(id, 0) - x).abs() < 0.3,
            "{id}: {why}, as in Chrome: {text}"
        );
    }
    assert_eq!(at("h1", 1), 18.5, "a block takes its own line: {text}");
    assert_eq!(
        [at("i1", 1), at("i1", 3)],
        [0.5, 18.5],
        "a lowered box keeps the line of 20px: {text}"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn lone_pseudo_elements_take_the_grid_alignment_like_chrome() {
    run_hidden(
        r#"
        const pixel = async (x, y) => [...(await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap().subarray(0, 3)].reverse().join();
        await load('dot.html');
        const [center, corner] = [await pixel(20, 20), await pixel(2, 2)];
        if (center !== '0,128,0' || corner !== '255,255,255') fail('dot: ' + center + ' ' + corner);
        "#,
        &[(
            "dot.html",
            "<body style='margin:0;background:white'><style>\
             #g{display:grid;place-items:center;width:40px;height:40px}\
             #g::after{content:'';width:10px;height:10px;background:rgb(0,128,0)}</style><div id=g></div></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn text_wraps_inside_padding_and_borders_like_chrome() {
    let (text, rects) = picture_rects(
        "wrap.html",
        "<body style='margin:0;font:10px/12px Menlo'><style>.w{width:200px}</style>\
         <div class=w><div id=b0 style='width:50px;border:5px solid'>aaaa aaaa</div></div>\
         <div class=w><div id=b1 style='width:50px;padding:0 5px'>aaaa aaaa</div></div></body>",
    );
    assert_eq!(rects["b0"], "0,0,60,34", "{text}");
    assert_eq!(rects["b1"], "0,0,60,24", "{text}");
}

#[test]
#[ignore = "requires the macOS window server"]
fn positioned_boxes_paint_after_in_flow_boxes_in_tree_order_like_chrome() {
    let rows = [
        "<div class=a></div><div class=g style='position:relative'></div>",
        "<div class=g style='position:absolute'></div><div class=r></div>",
        "<div class=a></div><div class=g style='opacity:.99'></div>",
        "<div style='position:relative'><div class=g style='position:relative;z-index:2'></div></div><div class=a style='z-index:1'></div>",
        "<div class=a><div class=a></div></div><div class=g style='position:relative'></div>",
        "<div style='display:flex'><div class=a></div><div class=g style='position:relative'></div></div>",
    ];
    let page = format!(
        "<body style='margin:0;background:white'><style>.p{{position:relative;height:40px}}\
         .a{{position:absolute;top:0;left:0;width:40px;height:40px;background:red}}\
         .r{{width:40px;height:40px;background:red}}.g{{width:40px;height:40px;background:lime}}</style>{}</body>",
        rows.map(|row| format!("<div class=p>{row}</div>")).join("")
    );
    let text = run_hidden(
        &format!(
            "await load('order.html');
            for (let row = 0; row < {}; row++) {{
                const image = await win.webContents.capturePage({{ x: 20, y: row * 40 + 20, width: 1, height: 1 }});
                console.log('PIXEL ' + row + ' ' + image.toBitmap()[1]);
            }}",
            rows.len()
        ),
        &[("order.html", &page)],
    );
    let green: Vec<u8> = text
        .lines()
        .filter_map(|line| line.strip_prefix("PIXEL "))
        .map(|line| line.rsplit(' ').next().unwrap().parse().unwrap())
        .collect();
    assert_eq!(green.len(), rows.len(), "{text}");
    for (row, value) in green.iter().enumerate() {
        assert!(*value > 200, "the green box covers in row {row}: {text}");
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn grid_rows_collapse_the_margins_of_block_content_like_chrome() {
    let (text, rects) = picture_rects(
        "margins.html",
        "<body style='margin:0'><style>.w{display:grid;grid-template-columns:100px 100px 100px;align-items:start}\
         .h{height:10px}</style><div class=w>\
         <div id=m0><div class=h style='margin:20px 0'></div><div class=h style='margin:30px 0'></div><div class=h style='margin-top:-10px'></div></div>\
         <div id=m1><div style='margin-top:20px'><div class=h style='margin-top:30px'></div></div><div style='margin:10px 0'></div><div class=h style='margin:15px 0'></div></div>\
         <div id=m2><div style='padding:1px'><div class=h style='margin:20px 0'></div></div></div></div></body>",
    );
    assert_eq!(rects["m0"], "0,0,100,100", "{text}");
    assert_eq!(rects["m1"], "100,0,100,80", "{text}");
    assert_eq!(rects["m2"], "200,0,100,52", "{text}");
}

#[test]
#[ignore = "requires the macOS window server"]
fn children_paint_over_borders_and_clip_at_the_padding_box_like_chrome() {
    let text = run_hidden(
        r#"await load('borders.html');
        for (const [x, y] of [[5, 25], [5, 85]]) {
            const image = await win.webContents.capturePage({ x, y, width: 1, height: 1 });
            console.log('PIXEL ' + [...image.toBitmap()].slice(0, 3).join(','));
        }"#,
        &[(
            "borders.html",
            "<body style='margin:0;background:white'><style>.b{height:40px;border:10px solid red;margin-bottom:10px}\
             .g{height:40px;margin:-10px -10px 0;background:lime}</style>\
             <div class=b><div class=g></div></div>\
             <div class=b style='overflow:hidden;border-color:transparent'><div class=g></div></div></body>",
        )],
    );
    let pixels: Vec<_> = text
        .lines()
        .filter_map(|line| line.strip_prefix("PIXEL "))
        .collect();
    assert_eq!(pixels, ["0,255,0", "255,255,255"], "{text}");
}

#[test]
#[ignore = "requires the macOS window server"]
fn scroll_ranges_reach_overflowing_descendants_like_chrome() {
    let text = run_hidden(
        r#"await load('page.html');
        console.log('SIZES ' + await run(`[...document.querySelectorAll('.s')].map(s => s.scrollWidth + 'x' + s.scrollHeight).join()`));"#,
        &[(
            "page.html",
            "<body style='margin:0;font:10px/12px Menlo'><style>.s{width:100px;height:40px;overflow:auto;border:2px solid;padding:5px}\
             .w{width:150px;height:10px;flex:none}</style>\
             <div class=s><div class=w style='margin:3px 20px 30px 7px'></div></div>\
             <div class=s><div style='display:flex;gap:10px'><div class=w></div><div class=w></div></div></div>\
             <div class=s style='white-space:nowrap'>aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</div>\
             <div class=s><div style='width:50px;height:10px'><div class=w style='height:80px'></div></div></div>\
             <div class=s style='overflow:hidden'><div style='overflow:hidden;width:50px'><div class=w></div></div></div></body>",
        )],
    );
    // Chrome: end padding follows children and lines, not the overflow of boxes inside them.
    assert!(
        text.contains("SIZES 187x53,315x50,191x50,155x85,110x50"),
        "{text}"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn outlines_stay_outside_the_overflow_clip_of_their_box_like_chrome() {
    let text = run_hidden(
        r#"await load('outline.html');
        await run(`document.getElementById('s').scrollTop = 40`);
        await win.webContents.capturePage();
        for (const [x, y] of [[8, 30], [40, 8], [40, 69]]) {
            const image = await win.webContents.capturePage({ x, y, width: 1, height: 1 });
            console.log('PIXEL ' + [...image.toBitmap()].slice(0, 3).join(','));
        }"#,
        &[(
            "outline.html",
            "<body style='margin:0;background:white'>\
             <div id=s style='position:absolute;left:10px;top:10px;width:100px;height:58px;overflow:auto;outline:2px solid red'>\
             <div style='height:300px;background:lime'></div></div></body>",
        )],
    );
    let pixels: Vec<_> = text
        .lines()
        .filter_map(|line| line.strip_prefix("PIXEL "))
        .collect();
    assert_eq!(pixels, ["0,0,255"; 3], "{text}");
}

#[test]
#[ignore = "requires the macOS window server"]
fn floats_wrap_to_new_rows_and_static_positions_skip_empty_lines_like_chrome() {
    let (text, rects) = picture_rects(
        "floats.html",
        "<body style='margin:0;font:10px/12px Menlo'><style>.w{width:100px}.f{float:left;width:40px;height:10px}</style>\
         <div class=w><div id=f0 class=f></div><div id=f1 class=f></div><div id=f2 class=f></div><div style='clear:both'></div></div>\
         <div class=w style='position:relative;padding:10px'><div id=a0 style='position:absolute;width:5px;height:5px'></div>text</div></body>",
    );
    assert_eq!(rects["f0"], "0,0,40,10", "{text}");
    assert_eq!(rects["f1"], "40,0,40,10", "{text}");
    assert_eq!(rects["f2"], "0,10,40,10", "{text}");
    assert_eq!(rects["a0"], "10,10,5,5", "{text}");
}

#[test]
#[ignore = "requires the macOS window server"]
fn root_backgrounds_paint_the_whole_canvas_like_chrome() {
    let page = "<body style='margin:20px;height:50px;background:linear-gradient(red,blue)'></body>";
    let text = run_hidden(
        r#"
        await load('root.html');
        const shot = async step => {
            await new Promise(done => setTimeout(done, 200));
            await win.webContents.capturePage();
            for (const [x, y] of [[5, 5], [5, 95], [5, 250], [30, 30]]) {
                const [b, g, r] = (await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap();
                console.log(`PIXEL ${step} ${x} ${y} ${r} ${g} ${b}`);
            }
        };
        await shot(0);
        await run("document.documentElement.style.background = 'lime'");
        await shot(1);
        await run("document.documentElement.style.cssText = 'height: 100%'; document.body.style.height = '100%'; document.body.innerHTML = '<div style=\"height: 2000px\"></div>'");
        await win.webContents.capturePage();
        await run('scrollTo(0, 700)');
        await shot(2);
        "#,
        &[("root.html", page)],
    );
    let near = |step: u32, x: u32, y: u32, expected: [i32; 3]| {
        let line = text
            .lines()
            .find_map(|line| line.strip_prefix(&format!("PIXEL {step} {x} {y} ")))
            .unwrap_or_else(|| panic!("{text}"));
        let actual: Vec<i32> = line.split(' ').map(|v| v.parse().unwrap()).collect();
        assert!(
            actual
                .iter()
                .zip(expected)
                .all(|(a, e)| (a - e).abs() <= 12),
            "step {step}: pixel at {x},{y} is {actual:?}, Chrome paints {expected:?}"
        );
    };
    // The body gradient covers the canvas in tiles of the root box, which is 90 px tall.
    near(0, 5, 5, [240, 0, 15]);
    near(0, 5, 95, [240, 0, 15]);
    near(0, 5, 250, [56, 0, 199]);
    // With a root background of its own, the body gradient stays in the body box.
    near(1, 5, 5, [0, 255, 0]);
    near(1, 5, 250, [0, 255, 0]);
    near(1, 30, 30, [203, 0, 52]);
    // Far below a root box of the viewport height (268 px), the tiles still cover the canvas.
    near(2, 5, 5, [94, 0, 161]);
}

#[test]
#[ignore = "requires the macOS window server"]
fn svg_backgrounds_keep_the_ratio_of_their_view_box_like_chrome() {
    let square = "url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 10 10'%3E%3Crect width='10' height='10' fill='red'/%3E%3C/svg%3E\")";
    let page = format!(
        "<style>body{{margin:0;background:white}}div{{width:40px;height:20px;background:{square} no-repeat 0 0 / 40px 20px}}</style><body><div></div></body>"
    );
    let text = run_hidden(
        r#"
        await load('svg.html');
        await new Promise(done => setTimeout(done, 300));
        await win.webContents.capturePage();
        for (const x of [5, 20, 35]) {
            const [b, g, r] = (await win.webContents.capturePage({ x, y: 10, width: 1, height: 1 })).toBitmap();
            console.log(`PIXEL ${x} ${r} ${g} ${b}`);
        }
        "#,
        &[("svg.html", &page)],
    );
    // The square fits the 40x20 tile as a 20 px square in its middle.
    for (x, color) in [(5, "255 255 255"), (20, "255 0 0"), (35, "255 255 255")] {
        assert!(
            text.contains(&format!("PIXEL {x} {color}")),
            "{x}, as in Chrome: {text}"
        );
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn form_controls_keep_their_rows_floats_and_lines_like_chrome() {
    let (text, rects) = picture_rects(
        "forms.html",
        "<body style='margin:0;font:16px/24px Helvetica'><style>.fc{padding-left:24px}\
         .fc input{float:left;margin-left:-24px;width:16px;height:16px;margin-top:4px;appearance:none;border:1px solid}\
         .col{display:flex}.col>div{width:200px}</style><div class=w>\
         <div class=fc><input type=checkbox><label id=l0>Label</label></div>\
         <textarea id=t0 rows=2 style='display:block;width:200px;padding:6px;border:1px solid;font:inherit;min-height:38px'></textarea>\
         <div class=col><div><label style='display:inline-block' id=l1>A</label><input id=r0 type=range style='width:100%;margin:0'></div><div style='height:100px'></div></div>\
         <div style='display:grid;grid-template-columns:200px'><div id=g0><span>Text</span><input id=i0 style='width:100%;box-sizing:border-box'></div></div>\
         <fieldset id=f0 style='margin:0;padding:0;border:0'><p style='margin:10px 0'>p</p></fieldset>\
         <progress id=p0 style='appearance:none;display:block;width:100px;height:10px'>Fallback text</progress>\
         <style>.ra::before{display:block;padding-top:50%;content:''}.fb::file-selector-button{padding:6px 12px;border:0;margin-right:12px}</style>\
         <div id=ra class=ra style='width:120px'></div><input id=fb class=fb type=file>\
         <select><optgroup id=og label=G><option>One</option></optgroup></select>\
         <div style='display:grid;grid-template-columns:100px;align-items:start'><div id=gi><img src='PICTURE' style='vertical-align:top;width:60px;height:40px'></div></div></div></body>",
    );
    assert!(
        rects["fb"].ends_with(",27.5"),
        "button padding sets the height, as in Chrome: {text}"
    );
    assert!(
        rects["gi"].ends_with(",100,40"),
        "a top-aligned image keeps no strut below it, as in Chrome: {text}"
    );
    for (id, rect) in [
        ("l0", "24,2.5,39.15,18.5"),
        ("t0", "0,24,214,62"),
        ("r0", "0,111.5,200,16"),
        ("g0", "0,186,200,48"),
        ("i0", "0,212.5,200,21.5"),
        ("f0", "0,234,400,44"),
        ("p0", "0,278,100,10"),
        ("ra", "0,288,120,60"),
        ("og", "0,0,0,0"),
    ] {
        assert_eq!(rects[id], rect, "{id}, as in Chrome: {text}");
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn plain_ranges_progress_bars_and_clipped_fields_paint_like_chrome() {
    let text = run_hidden(
        r#"await load('parts.html');
        await win.webContents.capturePage();
        for (const [x, y] of [[10, 10], [60, 10], [25, 45], [75, 45], [12, 80]]) {
            const image = await win.webContents.capturePage({ x, y, width: 1, height: 1 });
            console.log('PIXEL ' + [...image.toBitmap()].slice(0, 3).join(','));
        }"#,
        &[(
            "parts.html",
            "<body style='margin:0;background:white'><style>*{position:absolute;left:0;margin:0}\
             .r{appearance:none;top:0;width:100px;height:20px;padding:0;background:none}\
             .r::-webkit-slider-runnable-track{height:4px;background:#00f}\
             .r::-webkit-slider-thumb{appearance:none;width:20px;height:20px;margin-top:-8px;background:#f00}\
             progress{appearance:none;top:40px;width:100px;height:10px}\
             input:not(.r){top:60px;width:100px;height:40px;padding:0;border:0;font:40px/40px Menlo;color:#f00;background:#fff;background-clip:padding-box}</style>\
             <input class=r type=range value=0><progress value=50 max=100></progress><input value='\u{2588}\u{2588}'></body>",
        )],
    );
    let pixels: Vec<_> = text
        .lines()
        .filter_map(|line| line.strip_prefix("PIXEL "))
        .collect();
    assert_eq!(
        pixels,
        ["0,0,255", "255,0,0", "0,128,0", "128,128,128", "0,0,255"],
        "{text}"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn css_tables_block_tables_and_table_attributes_lay_out_like_chrome() {
    let text = run_hidden(
        r#"
        await load('t.html');
        console.log('BOXES ' + await run("JSON.stringify(['a','a1','a2','f','f1','f2','s1','s2','s3','k','h1','h3','h4','y','n','m'].map(id => (r => [r.x, r.y, r.width, r.height])(document.getElementById(id).getBoundingClientRect())))"));
        console.log('MORE ' + await run("document.getElementById('k').scrollWidth + ' ' + ['a','a1','k','s'].map(id => getComputedStyle(document.getElementById(id)).display).join()"));
        "#,
        &[(
            "t.html",
            "<body style='margin:0;font:12px Helvetica'><style>i{display:block;width:30px;height:10px}\
             b{display:block;width:60px;height:10px}.c{display:table-cell;padding:2px}</style>\
             <div id=a style='display:table;border-spacing:4px'><div style='display:table-row'><div class=c id=a1><i></i></div>\
             <div class=c id=a2 style='vertical-align:bottom'><b></b><b></b></div></div></div>\
             <div id=f style='display:table;width:200px;table-layout:fixed'><div class=c id=f1><i></i></div><div class=c id=f2><i></i></div></div>\
             <table id=s><tr><td id=s1><i></i></td></tr><tr><td id=s2><i></i></td><td id=s3><b></b></td></tr></table>\
             <div style='width:50px'><table id=k style='display:block;max-width:100%;overflow:auto'><tr><td><b></b></td><td><b></b></td></tr></table></div>\
             <table id=h cellpadding=5 cellspacing=0 width=150><tr><td id=h1 width=40 valign=bottom height=30><i id=h3></i></td>\
             <td id=h2 align=right><i id=h4 style='display:inline-block'></i></td></tr></table>\
             <style>.p td{padding:2px 6px;border:1px solid}</style><table class=p style='width:300px;table-layout:fixed'>\
             <tr><td style='width:50px'>50</td><td>a</td><td>b long long long</td></tr><tr><td>x</td><td id=y style='width:200px'>y</td>\
             <td>z</td></tr></table><div style='width:300px'><table class=p style='width:100%;table-layout:fixed;border-collapse:collapse'>\
             <tr><td id=n style='white-space:nowrap;overflow:hidden'>A long file name that gets truncated.txt</td><td id=m>12 KB</td></tr>\
             </table></div></body>",
        )],
    );
    let line = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name))
            .unwrap_or_else(|| panic!("{text}"))
            .trim()
            .to_owned()
    };
    let boxes: Vec<[f32; 4]> = serde_json::from_str(&line("BOXES")).unwrap();
    // Chrome's boxes: `display: table` boxes size like tables, rows with fewer cells start a new row, a `<table>`
    // box clips its wider table, `cellpadding`, `width`, `height`, `valign` and `align` are hints, and with
    // `table-layout: fixed` a cell takes its column's width, whatever its own width or content.
    let chrome: [[f32; 4]; 16] = [
        [0., 0., 110., 32.],
        [4., 4., 34., 24.],
        [42., 4., 64., 24.],
        [0., 32., 200., 14.],
        [0., 32., 100., 14.],
        [100., 32., 100., 14.],
        [2., 48., 32., 12.],
        [2., 62., 32., 12.],
        [36., 62., 62., 12.],
        [0., 76., 50., 16.],
        [0., 92., 50., 40.],
        [5., 117., 30., 10.],
        [115., 106., 30., 10.],
        [68., 156., 114., 20.],
        [0.5, 178.5, 149.5, 19.],
        [150., 178.5, 149.5, 19.],
    ];
    for (found, chrome) in boxes.iter().zip(chrome) {
        assert!(
            found.iter().zip(chrome).all(|(a, b)| (a - b).abs() < 0.6),
            "{found:?} is not {chrome:?}: {boxes:?}"
        );
    }
    assert_eq!(line("MORE"), "130 table,table-cell,block,table");
}

#[test]
#[ignore = "requires the macOS window server"]
fn empty_items_clearfix_tables_and_row_boxes_like_chrome() {
    let text = run_hidden(
        &format!(
            r#"{STEPS}
            await load('t.html');
            console.log('BOXES ' + await run("JSON.stringify(['u','e','t','r1','r2','c'].map(id => (r => [r.x, r.y, r.width, r.height])(document.getElementById(id).getBoundingClientRect())))"));
            console.log('EMPTY ' + await inside('h1'));
            "#
        ),
        &[(
            "t.html",
            "<body style='margin:0;font:12px/20px Helvetica;background:#fff'><style>i{display:block;width:30px;height:10px}\
             .cf::before,.cf::after{content:' ';display:table}</style><ul id=u style='margin:0'><li>a</li><li id=e></li><li>b</li></ul>\
             <div id=g style='display:grid;grid-template-columns:200px'><table id=t><tr id=r1><td rowspan=2><i></i></td><td><i></i></td></tr>\
             <tr id=r2><td><i></i></td></tr></table></div><div id=c class=cf><i></i></div>\
             <table id=h style='empty-cells:hide;border-spacing:0'><tr><td id=h1 style='background:rgb(255,0,0);width:20px;height:20px'></td>\
             <td><i></i></td></tr></table></body>",
        )],
    );
    let line = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name))
            .unwrap_or_else(|| panic!("{text}"))
            .trim()
            .to_owned()
    };
    let boxes: Vec<[f32; 4]> = serde_json::from_str(&line("BOXES")).unwrap();
    // Chrome's boxes in a 400px window: an empty item keeps the line of its marker, a table in a grid stretches, rows span all
    // columns and end with their own cells, and a clearfix of blank `display: table` pseudo-elements adds no line.
    let chrome: [[f32; 4]; 6] = [
        [0., 0., 400., 60.],
        [40., 20., 360., 20.],
        [0., 60., 200., 30.],
        [2., 62., 196., 12.],
        [2., 76., 196., 12.],
        [0., 90., 400., 10.],
    ];
    for (found, chrome) in boxes.iter().zip(chrome) {
        assert!(
            found.iter().zip(chrome).all(|(a, b)| (a - b).abs() < 0.6),
            "{found:?} is not {chrome:?}: {boxes:?}"
        );
    }
    assert_eq!(
        line("EMPTY"),
        "255,255,255",
        "an empty cell hides its background: {text}"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn background_clip_text_shows_the_background_through_the_text_like_chrome() {
    let row = |id: &str, background: &str| {
        format!(
            "<div id={id} style='width:200px;font:40px Helvetica;color:transparent;{background};-webkit-background-clip:text;text-decoration:line-through 20px'>MMMM</div>"
        )
    };
    let page = format!(
        "<body style='margin:0'>{}{}</body>",
        row("a", "background:linear-gradient(90deg,red,blue)"),
        row("b", "background-color:#c33")
    );
    let text = run_hidden(
        r#"
        await load('clip.html');
        const shot = async step => {
            await new Promise(done => setTimeout(done, 200));
            await win.webContents.capturePage();
            for (const [x, y] of [[5, 22], [120, 22], [160, 22], [190, 22], [5, 5], [5, 68], [190, 68]]) {
                const [b, g, r] = (await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap();
                console.log(`PIXEL ${step} ${x} ${y} ${r} ${g} ${b}`);
            }
        };
        await shot(0);
        await run("document.getElementById('a').textContent = 'MMMMM'");
        await shot(1);
        await run("document.getElementById('a').style.backgroundClip = 'border-box'");
        await shot(2);
        "#,
        &[("clip.html", &page)],
    );
    let near = |step: u32, x: u32, y: u32, expected: [i32; 3]| {
        let line = text
            .lines()
            .find_map(|line| line.strip_prefix(&format!("PIXEL {step} {x} {y} ")))
            .unwrap_or_else(|| panic!("{text}"));
        let actual: Vec<i32> = line.split(' ').map(|v| v.parse().unwrap()).collect();
        assert!(
            actual
                .iter()
                .zip(expected)
                .all(|(a, e)| (a - e).abs() <= 12),
            "step {step}: pixel at {x},{y} is {actual:?}, Chrome paints {expected:?}"
        );
    };
    // The gradient shows only in the text and its line, with the colors of its place in the box.
    near(0, 5, 22, [248, 0, 6]);
    near(0, 120, 22, [101, 0, 153]);
    near(0, 190, 22, [255, 255, 255]);
    near(0, 5, 5, [255, 255, 255]);
    // A background color fills only the text too.
    near(0, 5, 68, [204, 51, 51]);
    near(0, 190, 68, [255, 255, 255]);
    // New text after the first frame shows the background.
    near(1, 160, 22, [51, 0, 204]);
    // Without the text clip, the gradient fills the box.
    near(2, 5, 5, [248, 0, 6]);
    near(2, 190, 22, [13, 0, 242]);
}

#[test]
#[ignore = "requires the macOS window server"]
fn table_parts_without_a_table_get_an_anonymous_table_like_chrome() {
    let text = run_hidden(
        r#"
        await load('t.html');
        console.log('BOXES ' + await run("JSON.stringify(['p','a','b','q','r1','c1','c2','g','h','f','x','e'].map(id => (r => [r.x, r.y, r.width, r.height])(document.getElementById(id).getBoundingClientRect())))"));
        console.log('DISPLAY ' + await run("['p','d','r1'].map(id => getComputedStyle(document.getElementById(id)).display).join()"));
        "#,
        &[(
            "t.html",
            "<body style='margin:0;font:12px Helvetica'><style>i{display:block;width:30px;height:10px}\
             b{display:block;width:60px;height:20px}.c{display:table-cell;padding:2px}.r{display:table-row}</style>\
             <div id=p><div class=c id=a><i></i></div><div class=c id=b style='vertical-align:middle'><b></b></div></div>\
             <div id=q dir=rtl><div class=r id=r1><div class=c id=c1><i></i></div><div class=c><i></i></div></div>\
             <div class=r><div class=c id=c2><b></b></div></div></div><div><div style='display:table-row-group'>\
             <div class=c id=g><i></i></div></div><div style='display:table-header-group'><div class=c id=h><i></i></div></div></div>\
             <div style='display:table;width:200px'><div class=c id=f style='width:100%'><i></i></div>\
             <div class=c id=x style='width:1%'><i></i></div></div><div id=d style='display:inline-block;width:100px'>\
             <div class=c id=e style='width:50%'><i></i></div><div class=c><b></b></div></div></body>",
        )],
    );
    let line = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name))
            .unwrap_or_else(|| panic!("{text}"))
            .trim()
            .to_owned()
    };
    let boxes: Vec<[f32; 4]> = serde_json::from_str(&line("BOXES")).unwrap();
    // Chrome's boxes in a 400px window: sibling cells sit side by side in an anonymous table as wide as its content,
    // rows share their columns and in RTL the table is at the right, a header group goes first, a 100% column takes
    // the space that the other columns don't need, and a 50% column makes room for its share.
    let chrome: [[f32; 4]; 12] = [
        [0., 0., 400., 24.],
        [0., 0., 34., 24.],
        [34., 0., 64., 24.],
        [0., 24., 400., 38.],
        [302., 24., 98., 14.],
        [336., 24., 64., 14.],
        [336., 38., 64., 24.],
        [0., 76., 34., 14.],
        [0., 62., 34., 14.],
        [0., 90., 166., 14.],
        [166., 90., 34., 14.],
        [0., 104., 36., 24.],
    ];
    for (found, chrome) in boxes.iter().zip(chrome) {
        assert!(
            found.iter().zip(chrome).all(|(a, b)| (a - b).abs() < 0.6),
            "{found:?} is not {chrome:?}: {boxes:?}"
        );
    }
    assert_eq!(line("DISPLAY"), "block,inline-block,table-row");
}

#[test]
#[ignore = "requires the macOS window server"]
fn border_images_replace_the_border_and_follow_changes_like_chrome() {
    run_hidden(
        r#"
        await load('page.html');
        const color = async (x, y) => {
            const bytes = (await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap();
            return [bytes[2], bytes[1], bytes[0]].map(value => value > 200 ? 'f' : value < 60 ? '0' : '?').join('');
        };
        const look = async () => {
            for (let n = 0; n < 3; n++) await win.webContents.capturePage();
            return [await color(4, 30), await color(30, 30), await color(84, 4), await color(110, 4), await color(84, 30)].join(' ');
        };
        let first = '';
        for (let n = 0; n < 40 && first !== 'f00 fff 00f 0f0 f00'; n++) first = await look();
        if (first !== 'f00 fff 00f 0f0 f00') fail('first frame: ' + first);
        await run("document.getElementById('g').style.borderImageSource = 'linear-gradient(#00f, #00f)'; document.getElementById('i').style.borderWidth = '0'");
        const changed = await look();
        if (changed !== '00f fff fff fff fff') fail('after changes: ' + changed);
        await run("document.getElementById('g').style.borderImage = 'none'");
        const plain = await look();
        if (!plain.startsWith('000 ')) fail('normal border: ' + plain);
        "#,
        &[(
            "page.html",
            "<body style='margin:0;background:white'><div style='display:flex'>\
             <div id=g style='width:40px;height:40px;border:10px solid black;border-image:linear-gradient(red, red) 1'></div>\
             <div id=i style='margin-left:20px;width:40px;height:40px;border:10px solid black;border-image:url(data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAMAAAADCAIAAADZSiLoAAAAFklEQVR4nGNgYPgPhiCa4T8QQFlgBACpdAr2Fc+7cAAAAABJRU5ErkJggg==) 1'></div></div></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn columns_break_text_rows_and_move_kept_boxes_like_chrome() {
    let text = run_hidden(
        &format!(
            r#"{RECTS}
            await load('columns.html');
            console.log('RECTS ' + await rects());
            await run(`document.getElementById('a').style.height = '40px'`);
            await win.webContents.capturePage();
            console.log('MOVED ' + await rects());
            console.log('HITS ' + await run(`(() => {{
                const p = document.getElementById('p').getBoundingClientRect();
                return [50, 150, 250].map(x => document.elementFromPoint(x, p.y + 10)?.id).join();
            }})()`));
            "#
        ),
        &[(
            "columns.html",
            "<body style='margin:0;font:10px/20px sans-serif'><style>.w{columns:3;column-gap:0;width:300px}             .w div{height:20px;break-inside:avoid}p{margin:0}</style>             <div class=w><p id=p>One paragraph with enough words to run over all three columns of this box,              line after line, so each column holds two of its rows.</p></div>             <div class=w><div id=a></div><div id=b></div><div id=c></div><div id=d></div><div id=e></div><div id=f></div></div>             </body>",
        )],
    );
    let before = rects(&text);
    let line = |prefix: &str| {
        let line = text.lines().find_map(|line| line.strip_prefix(prefix));
        line.unwrap_or_else(|| panic!("{text}")).to_owned()
    };
    let moved: std::collections::HashMap<String, String> =
        serde_json::from_str(&line("MOVED ")).unwrap();
    let at = |rects: &std::collections::HashMap<String, String>, ids: &str| {
        ids.chars()
            .map(|id| rects[&id.to_string()].clone())
            .collect::<Vec<_>>()
    };
    assert!(
        before["p"].starts_with("0,0,300,"),
        "the text spans three columns: {before:?}"
    );
    assert_eq!(
        at(&before, "abcdef"),
        [
            "0,0,100,20",
            "0,20,100,20",
            "100,0,100,20",
            "100,20,100,20",
            "200,0,100,20",
            "200,20,100,20"
        ]
    );
    assert_eq!(
        at(&moved, "abcdef"),
        [
            "0,0,100,40",
            "0,40,100,20",
            "100,0,100,20",
            "100,20,100,20",
            "100,40,100,20",
            "200,0,100,20"
        ],
        "kept boxes move to the next columns like in Chrome"
    );
    assert_eq!(
        line("HITS "),
        "p,p,p",
        "each column of the text takes pointer events"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn lines_beside_a_float_follow_its_new_height_like_chrome() {
    // Words of six 10px letters make the rows: two fit beside the 50px float, three below it. The float grows
    // after the first frame, so the paragraph two blocks down must lay out again, not keep a cached result.
    let atoms = (1..=7)
        .map(|index| {
            format!(
                "<span id=s{index}>{}</span>",
                "abcdefg".repeat(6)[index - 1..]
                    .chars()
                    .take(6)
                    .collect::<String>()
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    let text = run_hidden(
        &format!(
            r#"{RECTS}
            await load('grow.html');
            console.log('RECTS ' + await rects());
            await run(`document.getElementById('f').style.height = '60px'`);
            await win.webContents.capturePage();
            await win.webContents.capturePage();
            console.log('GROWN ' + await rects());"#
        ),
        &[(
            "grow.html",
            &format!(
                "<body style='margin:0;font:16.6px/20px Menlo'><style>.w{{width:200px}}p{{margin:0}}</style>\
                 <div class=w><div id=f style='float:left;width:50px;height:20px'></div>\
                 <div><div><p>{atoms}</p></div></div></div></body>"
            ),
        )],
    );
    let grown = text
        .lines()
        .find_map(|line| line.strip_prefix("GROWN "))
        .unwrap_or_else(|| panic!("{text}"));
    let grown: std::collections::HashMap<String, String> = serde_json::from_str(grown).unwrap();
    let at = |rects: &std::collections::HashMap<String, String>, id: &str| -> [f32; 2] {
        let mut parts = rects[id].split(',').map(|v| v.parse::<f32>().unwrap());
        [parts.next().unwrap(), parts.next().unwrap()]
    };
    let first = rects(&text);
    for (rects, expected) in [
        (&first, [("s1", 50.), ("s3", 0.), ("s6", 0.)]),
        (&grown, [("s1", 50.), ("s3", 50.), ("s5", 50.)]),
    ] {
        for (id, x) in expected {
            assert!(
                (at(rects, id)[0] - x).abs() < 0.6,
                "{id} starts its row at {x}, as in Chrome: {rects:?}"
            );
        }
    }
    assert!(
        (at(&grown, "s7")[0]).abs() < 0.6 && at(&grown, "s7")[1] >= 60.,
        "below the float the row takes the whole width: {grown:?}"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn blocks_with_margins_beside_a_float_wrap_their_lines_like_chrome() {
    // Chrome's rows: left and top of each text row from its box, and the height of the box.
    let text = run_hidden(
        r#"await load('margins.html');
        console.log('ROWS ' + await run(`JSON.stringify([1, 2, 3, 4].map(i => {
            const box = document.getElementById('c' + i), at = box.getBoundingClientRect(), range = document.createRange();
            range.selectNodeContents(document.getElementById('t' + i));
            return [...range.getClientRects()].map(rect => [rect.x - at.x, rect.y - at.y].map(Math.round).join(',')).join(' ') + ' ' + box.offsetHeight;
        }))`));"#,
        &[(
            "margins.html",
            "<body style='margin:0;font:12px/1.3 Helvetica'><style>.c{width:200px;display:flow-root}.l{float:left;width:50px;height:60px}</style>\
             <div class=c id=c1><div class=l></div><div id=t1 style='margin-left:20px'>block with margin left beside the float text wraps along</div></div>\
             <div class=c id=c2><div class=l></div><div id=t2 style='margin:0 30px'>block with side margins beside the float text wraps along</div></div>\
             <div class=c id=c3><div class=l></div><p id=t3 style='margin:20px 0'>paragraph with margins beside the float text wraps along here</p></div>\
             <div class=c id=c4 style='width:180px'><div class=l style='height:40px'></div><div id=t4 style='margin-top:30px'>margin top 30 text text text</div></div></body>",
        )],
    );
    let rows = text
        .lines()
        .find_map(|line| line.strip_prefix("ROWS "))
        .unwrap_or_else(|| panic!("{text}"));
    let rows: Vec<String> = serde_json::from_str(rows).unwrap();
    assert_eq!(
        rows,
        [
            "50,1 50,16 50,32 60",
            "50,1 50,16 50,32 60",
            "50,21 50,36 50,52 87",
            "50,31 0,46 61",
        ],
        "as in Chrome: {text}"
    );
}
