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
        first == first_after && next_after[0] > next[0] + 20. && fill_after[1] < fill[1] - 20.,
        "the cells of the next column move, and its cell of no set width narrows: {boxes:?}"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn video_frames_keep_the_boxes_around_them() {
    let runtime = std::path::Path::new(env!("CARGO_BIN_EXE_apps-runtime"))
        .parent()
        .unwrap();
    // Copy the media library from deps to the executable's folder. Replace it atomically to preserve macOS
    // code signatures.
    let library = runtime.join("libapps_media.dylib");
    let copy = runtime.join(format!("libapps_media.{}.tmp", std::process::id()));
    std::fs::copy(runtime.join("deps/libapps_media.dylib"), &copy).unwrap();
    std::fs::rename(&copy, &library).unwrap();
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
        &[("package.json", r#"{"features":["video"]}"#)],
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
