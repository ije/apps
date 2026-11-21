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
fn observers_rects_hits_and_reloads_like_chrome() {
    run_hidden(
        r#"
        const input = body => fetch(MCP + '/input', { method: 'POST', body: JSON.stringify({ window: win.id, ...body }) }).then(response => response.text());
        const errors = [], check = (ok, what) => ok || errors.push(what);
        const frames = async () => { for (let n = 0; n < 3; n++) await win.webContents.capturePage(); };
        await load('page.html');
        await run("document.getElementById('o').style.setProperty('--o', '1')");
        await frames();
        const seen = await run('JSON.stringify(seen)');
        check(seen === '[[56,30]]', 'resize entries for a transform: ' + seen);
        const boxes = await run('JSON.stringify(boxes)');
        check(boxes === '{"in":[0,0,0,0],"sr":[20,10,20,10]}', 'resize entries for an inline box and an SVG shape: ' + boxes);
        await input({ action: 'scroll', x: 50, y: 150, delta_x: 0, delta_y: 60 });
        await frames();
        const offsets = await run('JSON.stringify(offsets)');
        check(/^\[100(,100)*\]$/.test(offsets), 'row offsets in scroll listeners: ' + offsets);
        // A gesture spreads its delta over frames, so the offsets between them have fractions.
        for (const [phase, delta_y] of [['began', 0], ['changed', -40], ['ended', 0]]) await input({ action: 'scroll', x: 50, y: 150, delta_x: 0, delta_y, phase });
        await new Promise(resolve => setTimeout(resolve, 300));
        await frames();
        const gesture = await run('JSON.stringify([offsets, texts])');
        check(/^\[\[100(,100)*\],\[0(,0)*\]\]$/.test(gesture), 'row and text offsets in a scroll gesture: ' + gesture);
        await input({ action: 'scroll', x: 150, y: 140, delta_x: 0, delta_y: 30 });
        await frames();
        const scaled = await run('JSON.stringify(scaled)');
        check(/^\[40(,40)*\]$/.test(scaled) && scaled !== '[40]', 'row offsets in a scaled scroll: ' + scaled);
        await input({ action: 'click', x: 203, y: 103 });
        await input({ action: 'click', x: 250, y: 150 });
        await input({ action: 'click', x: 302, y: 202 });
        await frames();
        const downs = await run('downs.join()');
        const hits = await run("[[203, 103], [302, 202], [320, 220]].map(([x, y]) => (e => e.id || e.localName)(document.elementFromPoint(x, y))).join()");
        check(downs === 'html,r,html' && hits === 'html,html,k', 'presses in rounded corners: ' + downs + ' ' + hits);
        await run("document.getElementById('m').style.top = '200px'");
        for (let n = 0; n < 3; n++) {
            await frames();
            const [top, y] = (await run("(m => [parseFloat(getComputedStyle(m).top), m.getBoundingClientRect().y].join())(document.getElementById('m'))")).split(',').map(Number);
            check(top > 0 && top < 200 && Math.abs(top - y) < 0.01, 'a transition between frames: ' + [top, y]);
        }
        await run("document.getElementById('f').style.opacity = '0'");
        await frames();
        const fades = [await run("getComputedStyle(document.getElementById('f')).opacity")];
        await new Promise(resolve => setTimeout(resolve, 300));
        fades.push(await run("getComputedStyle(document.getElementById('f')).opacity"));
        check(fades[1] < fades[0] && fades[0] < 1, 'an opacity transition without frames: ' + fades);
        await run('globalThis.mark = 1; location.search = location.search');
        for (let n = 0; n < 40 && await run('typeof mark') !== 'undefined'; n++) await new Promise(resolve => setTimeout(resolve, 50));
        check(await run('typeof mark') === 'undefined', 'the same URL did not load the page again');
        await run("globalThis.mark = 2; globalThis.hashes = []; addEventListener('hashchange', () => hashes.push('hash')); addEventListener('popstate', () => hashes.push('pop')); location.hash = 'x'");
        await run("location.hash = ''");
        await new Promise(resolve => setTimeout(resolve, 100));
        check(await run("[typeof mark, location.href.endsWith('.html#'), hashes.sort().join()].join()") === 'number,true,hash,hash,pop,pop', 'an empty hash: ' + await run("[location.href, hashes.join()].join()"));
        if (errors.length) fail(errors.join('; '));
        "#,
        &[(
            "page.html",
            "<body style='margin:0'><div id=o style='width:56px;perspective:300px'>\
             <div id=t style='height:30px;transform:rotateX(calc(var(--o, 0) * 40deg))'>t</div></div>\
             <div id=s style='position:absolute;left:0;top:100px;width:100px;height:100px;overflow:auto'>\
             <div style='height:20px'></div><div style='height:20px'></div><div style='height:20px'></div>\
             <div style='height:20px'></div><div style='height:20px'></div><div style='height:20px'><span>row</span></div>\
             <div style='height:400px'></div></div>\
             <div id=r style='position:absolute;left:200px;top:100px;width:100px;height:100px;border-radius:20px;background:red'></div>\
             <div style='position:absolute;left:120px;top:100px;transform:scale(2);transform-origin:0 0'>\
             <div id=z style='width:40px;height:40px;overflow:auto'><div style='height:10px'></div><div style='height:10px'></div>\
             <div style='height:10px'></div><div style='height:10px'></div><div style='height:200px'></div></div></div>\
             <div id=m style='position:fixed;left:350px;top:0;width:10px;height:10px;transition:top 10s linear'></div>\
             <div id=f style='position:fixed;left:350px;top:50px;width:10px;height:10px;background:blue;transition:opacity 10s linear'></div>\
             <div id=c style='position:absolute;left:300px;top:200px;width:60px;height:60px;border-radius:30px;overflow:hidden'>\
             <div id=k style='height:60px;background:green'></div></div>\
             <div style='position:absolute;left:0;top:250px'><span id=in style='padding:3px'>i</span><svg width=30 height=20><rect id=sr x=2 y=3 width=20 height=10 /></svg></div>\
             <script>globalThis.seen = []; globalThis.offsets = []; globalThis.downs = []; globalThis.boxes = {};\
             const sizes = new ResizeObserver(entries => { for (const e of entries) boxes[e.target.id] = [e.contentRect.width, e.contentRect.height, e.borderBoxSize[0].inlineSize, e.borderBoxSize[0].blockSize]; });\
             sizes.observe(document.getElementById('in')); sizes.observe(document.getElementById('sr'));\
             new ResizeObserver(entries => { for (const e of entries) seen.push([e.borderBoxSize[0].inlineSize, e.borderBoxSize[0].blockSize]); }).observe(document.getElementById('t'));\
             const s = document.getElementById('s'); s.scrollTop = 30;\
             globalThis.texts = []; const row = s.children[5].firstChild, range = new Range(); range.selectNodeContents(row.firstChild);\
             s.addEventListener('scroll', () => { offsets.push(s.children[5].getBoundingClientRect().top - s.getBoundingClientRect().top + s.scrollTop); texts.push(range.getBoundingClientRect().top - row.getBoundingClientRect().top); });\
             globalThis.scaled = []; const z = document.getElementById('z'); z.scrollTop = 10;\
             z.addEventListener('scroll', () => scaled.push(Math.round((z.children[4].getBoundingClientRect().top - z.getBoundingClientRect().top) / 2 + z.scrollTop)));\
             addEventListener('pointerdown', e => downs.push(e.target.id || e.target.localName));</script></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn pseudo_elements_report_the_sizes_of_their_boxes() {
    run_hidden(
        r#"
        await load('page.html');
        const sizes = await run(`JSON.stringify(['f', 'b', 'n'].map(id => (s => [s.width, s.height])(getComputedStyle(document.getElementById(id), '::before'))))`);
        if (sizes !== '[["200px","20px"],["400px","18px"],["50%","auto"]]') fail('pseudo sizes: ' + sizes);
        const box = await run(`(s => [s.top, s.right, s.bottom, s.left, s.paddingLeft, s.width].join())(getComputedStyle(document.getElementById('x'), '::before'))`);
        if (box !== '0px,270px,90px,0px,10px,20px') fail('an absolute pseudo-element box: ' + box);
        const scrolled = await run(`(s => (s.scrollTop = 40, (b => [s.scrollTop, b.top, b.left].join())(getComputedStyle(s, '::before'))))(document.getElementById('s'))`);
        if (scrolled !== '40,0px,0px') fail('the pseudo-element of a scroll box: ' + scrolled);
        "#,
        &[(
            "page.html",
            "<head><style>#f{display:flex;width:300px;height:20px} #f::before{content:'';flex:1}\
             #b{width:400px} #b::before{content:'x';display:block;line-height:18px} #n::before{content:'x';display:none;width:50%}\
             #x{position:relative;width:300px;height:100px} #x::before{content:'';position:absolute;width:20px;height:10px;padding-left:10px;rotate:30deg}\
             #s{position:relative;overflow:auto;width:200px;height:100px} #s::before{content:'';position:absolute;width:10px;height:10px}</style></head>\
             <body style='margin:0'><div id=f><i style='display:block;width:100px'></i></div><div id=b></div><div id=n></div><div id=x></div><div id=s><div style='height:1000px'></div></div></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn right_to_left_and_reversed_boxes_scroll_from_their_end() {
    run_hidden(
        r#"
        await load('page.html');
        const read = id => run(`(s => (r => [s.scrollWidth, s.scrollLeft, s.scrollTop, Math.round(r.x), Math.round(r.y)].join())(s.firstElementChild.getBoundingClientRect()))(document.getElementById('${id}'))`);
        const set = (id, key, value) => run(`document.getElementById('${id}').${key} = ${value}`);
        for (const [id, key, ...want] of [
            ['r', 'scrollLeft', '500,0,0,-300,0', '500,-300,0,0,0', '500,-50,0,-250,0', '500,0,0,-300,0'],
            ['f', 'scrollLeft', '500,0,0,-300,40', '500,-300,0,0,40', '500,-50,0,-250,40', '500,0,0,-300,40'],
            ['c', 'scrollTop', '200,0,0,0,0', '200,0,-80,0,80', '200,0,-50,0,50', '200,0,0,0,0'],
            ['l', 'scrollLeft', '400,0,0,-100,200', '400,0,0,-100,200', '400,0,0,-100,200', '400,200,0,-300,200'],
            ['k', 'scrollLeft', '500,0,0,-300,280', '500,-300,0,0,280', '500,-50,0,-250,280', '500,0,0,-300,280'],
        ]) {
            const got = [await read(id)];
            for (const value of [-9999, -50, 9999]) {
                await set(id, key, value);
                await win.webContents.capturePage();
                got.push(await read(id));
            }
            if (got.join(' ') !== want.join(' ')) fail(id + ': ' + got.join(' '));
        }
        const text = (await read('t')).split(',').map(Number);
        await set('t', 'scrollLeft', -9999);
        await win.webContents.capturePage();
        const end = (await read('t')).split(',').map(Number);
        if (!(text[0] > 300 && Math.abs(end[1] + text[0] - 200) <= 0.5 && Math.abs(end[3] + end[1]) <= 0.5)) fail('text that overflows to the left: ' + text + ' ' + end);
        "#,
        &[(
            "page.html",
            "<head><style>body{margin:0} div>div{flex:none;width:500px;height:20px} .s{width:200px;height:40px;overflow:auto}</style></head>\
             <body><div id=r class=s dir=rtl><div></div></div>\
             <div id=f class=s style='display:flex;flex-direction:row-reverse'><div></div></div>\
             <div id=c class=s style='display:flex;flex-direction:column-reverse;height:120px'><div style='width:100px;height:200px'></div></div>\
             <div id=l class=s><div style='margin-left:-100px'></div></div>\
             <div id=t class=s dir=rtl><div style='width:auto;white-space:nowrap'>A line of text that is much wider than its box</div></div>\
             <div id=k class=s dir=rtl style='display:flex;flex-direction:column'><div></div></div></body>",
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
        await load('nested-escape.html');
        await expect('fixed in absolute', hit(305, 5), 'f');
        await load('outer-block.html');
        await expect('outer scroll', "A.scrollTop = 30;" + rect('abs'), '152,2,50');
        await win.webContents.capturePage();
        await expect('outer scroll hit', hit(170, 20), 'abs');
        await expect('inner scroll', "B.scrollTop = 20;" + rect('abs'), '152,2,50');
        await win.webContents.capturePage();
        await expect('inner scroll hit', hit(170, 20), 'abs');
        await load('chain.html');
        const chain = "['x3', 'x4', 'x6', 'st6'].map(id => (r => [r.x, r.y].map(Math.round).join())(document.getElementById(id).getBoundingClientRect())).join(' ')";
        await expect('chain', chain, '112,22 266,12 112,166 112,166');
        await run("for (const id of ['A3', 'A4', 'A6']) document.getElementById(id).scrollTop = 30");
        await win.webContents.capturePage();
        await expect('chain outer scroll', chain, '112,-8 266,-18 112,136 112,156');
        await run("for (const id of ['B3', 'B4', 'B6', 'C3']) document.getElementById(id).scrollTop = 15");
        await win.webContents.capturePage();
        await expect('chain inner scroll', chain, '112,-8 266,-18 112,136 112,156');
        await expect('sticky hit', hit(120, 160), 'st6');
        const sticky = (await win.webContents.capturePage({ x: 120, y: 160, width: 1, height: 1 })).toBitmap();
        if (sticky[2] < 200 || sticky[1] > 50) fail('sticky not drawn: ' + [...sticky].join());
        // A kept sticky box finds its scroller again when a box of its containing block chain changes.
        await load('restick.html');
        const stuck = "['x6', 'st6'].map(id => Math.round(document.getElementById(id).getBoundingClientRect().y)).join()";
        for (const [step, code, value] of [
            ['rest', '', '122,122'],
            ['scroll A', "A6.scrollTop = 30", '92,112'],
            ['A visible', "A6.style.overflow = 'visible'", '122,122'],
            ['scroll root', "document.scrollingElement.scrollTop = 200", '-78,0'],
        ]) {
            await run(code);
            await win.webContents.capturePage();
            await expect(step, stuck, value);
        }
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
            (
                "outer-block.html",
                "<body style='margin:0'><style>.s{overflow:auto;border:2px solid}.r{height:40px}</style>\
                 <div class=s id=A style='position:relative;width:200px;height:150px;margin:20px'><div class=r></div>\
                 <div class=s id=B style='width:120px;height:80px'><div class=r></div>\
                 <div id=abs style='position:absolute;left:130px;top:10px;width:50px;height:50px'></div>\
                 <div class=r></div><div class=r></div></div><div class=r></div><div class=r></div><div class=r></div></div>\
                 <script>globalThis.A = document.getElementById('A'); globalThis.B = document.getElementById('B')</script></body>",
            ),
            (
                "chain.html",
                "<body style='margin:0'><style>.s{overflow:auto;border:2px solid}.r{height:40px}.x{position:absolute;width:20px;height:20px}</style><div style='display:flex'><div class=s id=A3 style='position:relative;width:150px;height:150px'><div class=r></div><div class=s id=B3 style='width:120px;height:100px'><div class=r></div><div class=s id=C3 style='width:90px;height:60px'><div class=r></div><i class=x id=x3 style='left:110px;top:20px'></i><div class=r></div><div class=r></div></div><div class=r></div><div class=r></div><div class=r></div></div><div class=r></div><div class=r></div><div class=r></div><div class=r></div></div>\
                 <div class=s id=A4 style='transform:translateZ(0);width:150px;height:150px'><div class=r></div><div class=s id=B4 style='width:100px;height:80px'><div class=r></div><i id=x4 style='position:fixed;left:110px;top:10px;width:20px;height:20px'></i><div class=r></div><div class=r></div><div class=r></div></div><div class=r></div><div class=r></div><div class=r></div><div class=r></div></div></div>\
                 <div class=s id=A6 style='position:relative;width:150px;height:120px'><div class=r></div><div class=s id=B6 style='width:100px;height:60px'><div class=r></div><div id=x6 style='position:absolute;left:110px;top:10px;width:30px;height:200px'><b id=st6 style='position:sticky;top:0;display:block;height:20px;background:#f00'></b></div><div class=r></div><div class=r></div><div class=r></div></div><div class=r></div><div class=r></div><div class=r></div><div class=r></div></div></body>",
            ),
            (
                "restick.html",
                "<body style='margin:0;height:2000px'><style>.s{overflow:auto;border:2px solid}.r{height:40px}</style>\
                 <div style='height:100px'></div><div class=s id=A6 style='position:relative;width:150px;height:120px;margin:10px'>\
                 <div class=r></div><div class=s id=B6 style='width:100px;height:60px'><div class=r></div>\
                 <div id=x6 style='position:absolute;left:110px;top:10px;width:30px;height:400px'>\
                 <b id=st6 style='position:sticky;top:0;display:block;height:20px'></b></div>\
                 <div class=r></div><div class=r></div><div class=r></div></div>\
                 <div class=r></div><div class=r></div><div class=r></div><div class=r></div></div>\
                 <script>globalThis.A6 = document.getElementById('A6')</script></body>",
            ),
            (
                "nested-escape.html",
                "<body style='margin:0'><p>Text</p>\
                 <div style='position:absolute;left:10px;top:40px;width:40px;height:40px;background:#ccc'>\
                 <div id=f style='position:fixed;left:300px;top:0;width:10px;height:10px;background:red'></div></div></body>",
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
fn canvases_draw_their_drawable_elements() {
    run_hidden(
        r#"
        await load('page.html');
        const frames = async (done, what) => {
            for (let n = 0; n < 60; n++) { await win.webContents.capturePage(); if (await run(done)) return; }
            fail(what + ': ' + JSON.stringify(await run('state()')));
        };
        const shot = async (x, y) => [...(await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap()].slice(0, 3).reverse().join();
        await frames('paints > 0 && pixel(110, 50) === "255,0,0,255"', 'the element did not draw');
        const first = await run('state()');
        if (first.changed !== 'box,field' || first.empty !== '0,0,0,0') fail('first paint: ' + JSON.stringify(first));
        // The window shows the drawing of the canvas, not the boxes of its children.
        await frames('true', '');
        if (await shot(110, 50) !== '255,0,0' || await shot(10, 10) !== '255,255,255' || await shot(10, 40) !== '255,255,255') fail('window pixels: ' + [await shot(110, 50), await shot(10, 10), await shot(10, 40)].join(' '));
        await frames('at(110, 50) === "box" && at(10, 10) === "c"', 'the pointer misses the drawn element');
        const drawn = await run('state()');
        if (drawn.rect !== '100,40,50,30' || drawn.matrix !== '100,40' || drawn.plain !== 'InvalidStateError' || drawn.size !== '50,30') fail('geometry: ' + JSON.stringify(drawn));
        // A paint follows a change of the element, and no paint follows a frame without one.
        const paints = drawn.paints;
        await run('box.style.background = "rgb(0,128,0)"');
        await frames('pixel(110, 50) === "0,128,0,255"', 'the change did not draw');
        for (let n = 0; n < 5; n++) await win.webContents.capturePage();
        const settled = await run('state()');
        if (settled.paints !== paints + 1 || settled.changed !== 'box') fail('paints after a change: ' + (settled.paints - paints) + ' ' + settled.changed);
        await run('c.requestPaint()');
        await frames('paints === ' + (paints + 2), 'requestPaint did not paint');
        if ((await run('state()')).changed !== '') fail('requestPaint listed a changed element');
        // The drawn field takes the pointer and the keyboard, and what the user types paints.
        const input = body => fetch(MCP + '/input', { method: 'POST', body: JSON.stringify({ window: win.id, ...body }) }).then(response => response.text());
        await input({ action: 'click', x: 110, y: 50 });
        await input({ action: 'click', x: 60, y: 85 });
        await frames('clicks === 1 && document.activeElement.id === "field"', 'the click missed the drawn field');
        await input({ action: 'type', text: 'ab' });
        await frames('field.value === "ab" && changed === "field"', 'typed text did not paint');
        // A clip leaves only a part of the element to the pointer.
        await run('c.updateElementGeometry(box, { canvasClip: { x: 100, y: 40, width: 20, height: 30 } })');
        await frames('at(110, 50) === "box" && at(130, 50) === "c"', 'the clip of the geometry');
        await run('c.clearElementGeometry(box)');
        await frames('at(110, 50) === "c"', 'cleared geometry still takes the pointer');
        "#,
        &[(
            "page.html",
            "<body style='margin:0;background:white'>\
             <canvas id=c width=200 height=100 content=drawable style='display:block;width:200px;height:100px'>\
             <div id=box drawable style='width:50px;height:30px;background:rgb(255,0,0)'></div>\
             <div id=plain style='width:50px;height:30px;background:rgb(0,0,255)'></div>\
             <input id=field drawable style='display:block;box-sizing:border-box;width:80px;height:20px;margin:0'></canvas>\
             <script>\
             var c = document.getElementById('c'), box = document.getElementById('box'), field = document.getElementById('field'), ctx = c.getContext('2d'), paints = 0, changed = null, clicks = 0;\
             box.onclick = () => clicks++;\
             var pixel = (x, y) => ctx.getImageData(x, y, 1, 1).data.join();\
             var at = (x, y) => document.elementFromPoint(x, y).id;\
             var state = () => { const r = box.getBoundingClientRect(), m = c.getElementTransform(box), size = c.getElementImageDefaultSize(box); let plain = ''; try { ctx.drawElementImage(document.getElementById('plain'), 0, 0); } catch (error) { plain = error.name; }\
               return { paints, changed, empty: pixel(10, 10), rect: [r.x, r.y, r.width, r.height].join(), matrix: m.e + ',' + m.f, plain, size: size.width + ',' + size.height }; };\
             c.onpaint = event => { paints++; changed = event.changedElements.map(element => element.id).join(); ctx.reset(); ctx.drawElementImage(box, 100, 40); ctx.drawElementImage(field, 20, 75); };\
             </script></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn drawable_elements_nest_scroll_and_go_to_workers_and_textures() {
    run_hidden(
        r#"
        await load('page.html');
        const frames = async (done, what) => {
            for (let n = 0; n < 60; n++) { await win.webContents.capturePage(); if (await run(done)) return; }
            fail(what + ': ' + JSON.stringify(await run('state()')));
        };
        // A drawable element inside another one draws by itself, and the outer one draws without it.
        await frames('pixel(20, 58) === "255,0,0,255" && pixel(105, 55) === "0,0,255,255"', 'nested elements');
        await frames('at(20, 60) === "outer" && at(110, 55) === "inner"', 'the pointer on nested elements');
        // The elements move with the canvas, which needs no new drawing.
        const paints = (await run('state()')).paints;
        await run('scrollTo(0, 40)');
        await frames('at(20, 20) === "outer" && at(110, 15) === "inner"', 'the pointer after a scroll');
        for (let n = 0; n < 3; n++) await win.webContents.capturePage();
        if ((await run('state()')).paints !== paints) fail('a scroll painted the canvas');
        await run('scrollTo(0, 0)');
        // An OffscreenCanvas of the page places its elements in a microtask and tells the canvas.
        await frames('updated === "para" && at(50, 120) === "para"', 'the geometry of an OffscreenCanvas');
        if (await run('d.getElementTransform(para).e') !== 40) fail('the transform of an OffscreenCanvas');
        // A worker draws a captured element, and the page has no use of the image after the transfer.
        const worked = await run(`new Promise(resolve => {
            const code = 'onmessage = event => { const context = new OffscreenCanvas(8, 8).getContext("2d"); context.drawElementImage(event.data, 0, 0); postMessage(context.getImageData(1, 1, 1, 1).data.join()); }';
            const worker = new Worker(URL.createObjectURL(new Blob([code]))), image = c.captureElementImage(outer);
            worker.onmessage = event => { let closed = ''; try { ctx.drawElementImage(image, 0, 0); } catch (error) { closed = error.name; } resolve(event.data + ' ' + closed); };
            worker.postMessage(image, [image]);
        })`);
        if (worked !== '255,0,0,255 InvalidStateError') fail('the worker drew ' + worked);
        const texel = await run(`(async () => {
            const device = await (await navigator.gpu.requestAdapter()).requestDevice();
            const texture = device.createTexture({ size: [4, 4], format: 'rgba8unorm', usage: GPUTextureUsage.COPY_SRC | GPUTextureUsage.COPY_DST | GPUTextureUsage.RENDER_ATTACHMENT });
            device.queue.drawElementImageToTexture({ source: para }, { texture, size: [4, 4] });
            const buffer = device.createBuffer({ size: 1024, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ }), encoder = device.createCommandEncoder();
            encoder.copyTextureToBuffer({ texture }, { buffer, bytesPerRow: 256 }, [4, 4]);
            device.queue.submit([encoder.finish()]);
            await buffer.mapAsync(GPUMapMode.READ);
            return new Uint8Array(buffer.getMappedRange()).slice(0, 4).join();
        })()`);
        if (texel !== '0,128,0,255') fail('the texture holds ' + texel);
        // An animation inside a drawable element paints the canvas frame by frame.
        await frames('alphas.size > 3', 'an animation did not paint');
        "#,
        &[
            (
                "page.html",
                "<body style='margin:0;background:white;height:2000px'>\
                 <canvas id=c width=200 height=100 content=drawable style='display:block;width:200px;height:100px'>\
                 <div id=outer drawable style='width:60px;height:40px;background:rgb(255,0,0)'>\
                 <span id=inner drawable style='width:20px;height:10px;margin:5px;background:rgb(0,0,255)'></span></div></canvas>\
                 <canvas id=d width=100 height=50 content=drawable style='display:block;width:100px;height:50px'>\
                 <p id=para drawable style='margin:0;width:30px;height:20px;background:rgb(0,128,0)'></p></canvas>\
                 <style>@keyframes fade { to { opacity: 0 } }</style>\
                 <canvas id=e width=20 height=20 content=drawable style='display:block;width:20px;height:20px'><div id=pulse drawable>\
                 <div style='width:20px;height:20px;background:black;animation:fade 1s linear infinite alternate'></div></div></canvas>\
                 <script>\
                 var c = document.getElementById('c'), d = document.getElementById('d'), ctx = c.getContext('2d'), paints = 0, updated = '';\
                 var [outer, inner, para] = ['outer', 'inner', 'para'].map(id => document.getElementById(id));\
                 var pixel = (x, y) => ctx.getImageData(x, y, 1, 1).data.join();\
                 var at = (x, y) => document.elementFromPoint(x, y).id;\
                 var state = () => ({ paints, updated, alphas: [...alphas].join(), rect: JSON.stringify(inner.getBoundingClientRect()) });\
                 c.onpaint = () => { paints++; ctx.reset(); ctx.drawElementImage(outer, 10, 50); ctx.drawElementImage(inner, 100, 50); };\
                 var offscreen = d.transferControlToOffscreen().getContext('2d');\
                 d.onpaint = () => { offscreen.reset(); offscreen.drawElementImage(para, 40, 10); };\
                 d.onelementgeometryupdate = event => { updated = event.elements.map(element => element.id).join(); };\
                 var e = document.getElementById('e'), faded = e.getContext('2d'), alphas = new Set();\
                 e.onpaint = () => { faded.reset(); faded.drawElementImage(document.getElementById('pulse'), 0, 0); alphas.add(faded.getImageData(5, 5, 1, 1).data[3]); };\
                 </script></body>",
            ),
            ("package.json", r#"{"features":["webgpu","jsExecute"]}"#),
        ],
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
        if (!before.startsWith('0,0,48,48 | 48,0,') || hover !== before.replace(/ 1$/, ' 0.8')) fail('hover: ' + before + ' then ' + hover);
        "#,
        &[(
            "page.html",
            "<body style='margin:0'><style>img:hover{opacity:.8}</style><div style='display:flex;height:48px'>\
             <img id=i src='data:image/svg+xml,%3Csvg xmlns=%22http://www.w3.org/2000/svg%22 width=%2296%22 height=%2296%22%3E%3Crect width=%2296%22 height=%2296%22 fill=%22red%22/%3E%3C/svg%3E'>\
             <span id=j>after</span></div></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn shadow_tree_tops_do_not_share_styles_with_light_boxes() {
    run_hidden(
        r#"
        await load('page.html');
        for (let n = 0; n < 3; n++) await win.webContents.capturePage();
        const width = await run("document.getElementById('outer').shadowRoot.getElementById('mid').getBoundingClientRect().width === innerWidth");
        if (!width) fail('nested host is not as wide as the page');
        "#,
        &[(
            "page.html",
            "<body style='margin:0'><div id=outer></div><div id=other></div><script>\
             const outer = document.getElementById('outer'), s1 = outer.attachShadow({ mode: 'open' });\
             s1.innerHTML = '<div id=mid></div><button>b1</button>';\
             s1.getElementById('mid').attachShadow({ mode: 'open' }).innerHTML = '<button>b2</button>';\
             document.getElementById('other').attachShadow({ mode: 'open' }).innerHTML = '<button>b3</button>';\
             </script></body>",
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
fn truncated_text_draws_after_a_layout_from_the_cache() {
    run_hidden(
        r#"
        const input = body => fetch(MCP + '/input', { method: 'POST', body: JSON.stringify({ window: win.id, ...body }) }).then(response => response.text());
        await load('page.html');
        const ink = async () => (await win.webContents.capturePage({ x: 30, y: 0, width: 100, height: 24 })).toBitmap().filter((value, index) => index % 4 === 0 && value < 128).length;
        const before = await ink();
        await input({ action: 'click', selector: '#one' });
        await win.webContents.capturePage();
        await input({ action: 'key', key: 'tab' });
        await win.webContents.capturePage();
        const after = await ink();
        // In a release build the label lost its place here: a measure shaped it again and prepaint skipped the box.
        if (!(before > 500 && after >= before)) fail('label ink: ' + before + ' ' + after);
        "#,
        &[(
            "page.html",
            "<body style='margin:0;background:#fff'><style>button{display:flex;align-items:center;gap:8px;height:24px;border:0;background:none;padding:0 4px;font:16px sans-serif;color:#000}\
             button:focus-visible{outline:2px solid green;outline-offset:-1px}.b{display:grid;width:18px;height:18px;border:1px solid #888;place-items:center}\
             .n{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}</style><div style='width:300px'>\
             <button id=one><span class=b></span><span class=n>Road trip</span></button>\
             <button id=two><span class=b></span><span class=n>Coastline</span></button></div>\
             <script>document.getElementById('one').onclick = () => document.querySelector('#one .b').textContent = 'x'</script></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn text_of_mixed_fonts_moves_with_a_scroll() {
    run_hidden(
        &format!(
            r#"{INPUT}
        await load('page.html');
        const ink = async () => (await win.webContents.capturePage({{ x: 0, y: 0, width: 200, height: 30 }})).toBitmap().filter((value, index) => index % 4 === 0 && value < 128).length;
        const before = await ink();
        await input({{ action: 'scroll', x: 50, y: 15, delta_x: 0, delta_y: 60 }});
        await frames(300);
        const after = await ink();
        // Each row has the same text, so the top row has as much ink after the scroll. A release build drew text of
        // mixed fonts, which prepaint shapes, at its place before the scroll.
        if (await run("document.getElementById('list').scrollTop") !== 60 || !(after > before * 0.8)) fail('ink: ' + before + ' ' + after);
        "#
        ),
        &[(
            "page.html",
            "<body style='margin:0;background:#fff'><style>#list{height:100px;overflow:auto;font:16px sans-serif;color:#000}\
             #list>div{display:grid;grid-template-columns:16px 1fr auto;align-items:center;height:30px}\
             .e{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-weight:600}</style><div id=list></div>\
             <script>for (let i = 0; i < 20; i++) document.getElementById('list').insertAdjacentHTML('beforeend', '<div><i></i><span class=e><span style=\"font-weight:400\">Part</span> two</span><b>81</b></div>');</script></body>",
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
fn inline_flex_boxes_sit_on_the_baseline_of_their_first_item() {
    run_hidden(
        r#"
        await load('page.html');
        // Chrome: 26 and 29. The baseline of the box is the one of its text, so the box fits in the line.
        const heights = await run("['e', 'f'].map(id => document.getElementById(id).getBoundingClientRect().height).join()");
        if (heights !== '26,29') fail('line heights: ' + heights);
        "#,
        &[(
            "page.html",
            "<body style='margin:0;font-family:sans-serif'><style>div{font-size:13px;line-height:26px;width:400px}\
             span{display:inline-flex;height:22px;font-size:12.5px;line-height:1}</style>\
             <div id=e>a <span style='align-items:center'>Ada</span> b</div><div id=f>a <span>Ada</span> b</div></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn text_runs_make_one_flex_or_grid_item() {
    run_hidden(
        r#"
        await load('page.html');
        const rects = id => run(`[...document.getElementById('${id}').childNodes].filter(node => node.nodeType === 3).map(node => { const range = document.createRange(); range.selectNodeContents(node); const r = range.getBoundingClientRect(); return [r.x, r.y, r.width].map(Math.round).join() }).join(' ')`);
        const [split, whole] = [await rects('a'), await rects('b')];
        const [[x1, , w1], [x2, , w2]] = split.split(' ').map(rect => rect.split(',').map(Number));
        // Like Chrome, the space between the nodes stays and the gap goes only between items.
        if (x1 !== 0 || x2 !== w1 || Math.abs(w1 + w2 - whole.split(',')[2]) > 1) fail('flex: ' + split + ' and ' + whole);
        const [plus, four] = (await rects('g')).split(' ').map(rect => rect.split(','));
        if (plus[1] !== four[1] || +four[0] <= +plus[0]) fail('grid: ' + plus + ' and ' + four);
        // Runs go across `display: contents`: the items are "pre", "bold" and "c" with "post".
        const text = selector => run(`(range => (range.selectNodeContents(document.querySelector('${selector}')), (r => [r.x, r.width].map(Math.round))(range.getBoundingClientRect())))(document.createRange())`);
        const [c, post] = [await text('#c > span'), await text('#c')];
        const tail = await run("(node => { const range = document.createRange(); range.selectNodeContents(node); return (r => [r.x, r.width].map(Math.round).join())(range.getBoundingClientRect()) })(document.getElementById('c').lastChild)");
        const [px, pw] = tail.split(',').map(Number);
        if (!(pw > 0) || Math.abs(c[0] + c[1] - px) > 1 || post[0] !== 0) fail('contents: ' + c + ' ' + tail + ' ' + post);
        // A run that starts with a space is an item of its own: it does not hang on the box before it.
        const star = await run(`(p => { const range = document.createRange(); range.selectNodeContents(p.lastChild); const [r, t] = [p.getBoundingClientRect(), range.getBoundingClientRect()]; return [r.height, t.x - r.x].map(Math.round).join() })(document.getElementById('s'))`);
        if (star !== '16,20') fail('space run: ' + star);
        "#,
        &[(
            "page.html",
            "<body style='margin:0;font:16px sans-serif'><style>p{display:flex;gap:6px;margin:0;height:20px}</style>\
             <p id=a></p><p id=b>2 selected</p><span id=g style='display:inline-grid;place-items:center;width:60px'></span>\
             <p id=c>pre<span style='display:contents'><b>bold</b> c</span>post</p>\
             <p id=s style='align-items:center;gap:4px;height:auto;line-height:16px'><svg width=16 height=16></svg></p>\
             <script>\
             document.getElementById('s').append(' ', '4.8');\
             document.getElementById('a').append('2', document.createComment(''), ' selected');\
             document.getElementById('g').append('+', '4');\
             </script></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn inline_boxes_draw_their_outlines_on_each_row() {
    run_hidden(
        r#"
        await load('page.html');
        // Counts the blue device pixels (BGRA) of each row of the paragraph.
        const pixels = async top => { const bytes = (await win.webContents.capturePage({ x: 0, y: top, width: 200, height: 20 })).toBitmap(); let count = 0; for (let at = 0; at < bytes.length; at += 4) if (bytes[at] > 200 && bytes[at + 1] < 80 && bytes[at + 2] < 80) count++; return count; };
        const rows = [await pixels(0), await pixels(20)];
        if (!(rows[0] > 100 && rows[1] > 100)) fail('outline pixels: ' + rows);
        "#,
        &[(
            "page.html",
            "<body style='margin:0;background:#fff'><p style='margin:0;width:150px;font:16px/20px sans-serif'>\
             x <a href='#x' style='outline:2px solid #0000ff;color:#000;text-decoration:none'>an outlined link that wraps</a> y</p></body>",
        )],
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

// Debug builds check each copied paint against a fresh paint, so these frames fail when a copy is wrong.
#[test]
#[ignore = "requires the macOS window server"]
fn nested_scrolls_copy_their_content_when_an_outer_scroll_moves_them() {
    run_hidden(
        r#"
        await load('scroll.html');
        for (const code of ["o.scrollTop = 10", "i.scrollTop = 30", "o.scrollBy(0, 10)", "scrollTo(0, 50)", "o.scrollTop = 0", "scrollTo(0, 120)", "i.scrollTop = 0", "p.scrollTop = 20"]) {
            await run("(() => { const [o, i, p] = ['o', 'i', 'p'].map(id => document.getElementById(id)); " + code + " })()");
            await win.webContents.capturePage();
            await win.webContents.capturePage();
        }
        "#,
        &[(
            "scroll.html",
            "<body style='margin:8px;height:2000px'><style>\
             .s{width:200px;height:100px;overflow:auto;background:#eee}\
             .g{height:400px;background:repeating-linear-gradient(#9cf 0 20px,#fc9 20px 40px)}\
             </style><div class=s id=o><div style='height:150px'></div>\
             <div class=s id=i style='width:150px;height:60px'><div class=g style='height:120px'>text</div></div>\
             <div class=g></div></div>\
             <div style='opacity:.8'><div class=s id=p style='position:relative'><div class=g></div></div><div class=g></div></div></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn scrolled_copies_follow_offsets_outlines_and_moving_interest_areas() {
    // Debug builds check each copied paint and kept prepaint against a fresh one.
    run_hidden(
        r#"
        await load('copies.html');
        for (let index = 0; index < 120; index++) {
            await run('scrollBy(0, 7)');
            await win.webContents.capturePage();
        }
        "#,
        &[(
            "copies.html",
            "<body style='margin:0;height:4000px'><style>\
             .b{width:180px;height:40px;margin:8px;background:#9cf;box-shadow:0 2px 6px #0006}\
             </style><div style='filter:drop-shadow(0 0 1px red);position:relative;z-index:1'>\
             <div class=b style='outline:2px solid #333'></div>\
             <div class=b style='position:absolute;left:200px;top:0;transform:translate(30px,5px)'></div>\
             <div class=b style='position:relative;z-index:2'></div></div>\
             <div style='height:900px'></div><input style='width:180px;height:30px;border:2px solid #333'>\
             <div style='filter:opacity(.9)'><div class=b style='outline:2px solid #333'></div></div></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn text_draws_its_selection_in_the_frame_a_popover_leaves() {
    run_hidden(
        r#"
        await load('popover.html');
        await run("document.getElementById('p').showPopover()");
        await win.webContents.capturePage();
        await run("document.getElementById('p').hidePopover(); getSelection().selectAllChildren(document.getElementById('t'))");
        await win.webContents.capturePage();
        await win.webContents.capturePage();
        const pixel = [...(await win.webContents.capturePage({ x: 10, y: 110, width: 1, height: 1 })).toBitmap().subarray(0, 3)];
        if (pixel.every(value => value > 240)) fail('no selection: ' + pixel);
        "#,
        &[(
            "popover.html",
            "<body style='margin:0;font:40px/1 Helvetica'><div id=p popover>pop</div>\
             <p id=t style='margin:100px 0 0'>MMMMMM</p></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn fixed_root_backgrounds_fill_the_viewport_like_chrome() {
    run_hidden(
        r#"
        await load('fixed.html');
        for (const top of [0, 500]) {
            await run(`scrollTo(0, ${top})`);
            const image = await win.webContents.capturePage();
            const { width, height } = image.getSize(), bitmap = image.toBitmap();
            const pixel = y => [...bitmap.subarray(4 * (y * width + 200), 4 * (y * width + 200) + 3)].reverse();
            const [first, last] = [pixel(1), pixel(height - 2)];
            if (!(first[0] > 245 && first[2] < 10 && last[2] > 245 && last[0] < 10)) fail(top + ': ' + first + ' ' + last);
        }
        "#,
        &[(
            "fixed.html",
            "<body style='margin:0;height:2000px'><style>html{background:linear-gradient(#f00,#00f) fixed}</style></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn borders_outlines_and_spaced_tiles_paint_like_chrome() {
    run_hidden(
        r#"
        const pixel = async (x, y) => [...(await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap().subarray(0, 3)].reverse();
        await load('paint.html');
        const near = (got, want) => got.every((value, at) => Math.abs(value - want[at]) < 12);
        const checks = [
            ['border above an image layer', 15, 5, [128, 0, 0]],
            ['groove outline, top', 150, 13, [116, 87, 58]],
            ['groove outline, bottom', 150, 76, [255, 191, 128]],
            ['gap between spaced tiles', 270, 40, [255, 255, 255]],
            ['spaced tile', 300, 40, [0, 0, 255]],
            ['corner of a wider dashed side', 285, 101, [170, 0, 0]],
            ['first dot beside a side without a border', 21, 124, [0, 0, 170]],
            ['rounded dashes start after the top left corner', 49, 161, [170, 0, 0]],
            ['gap of a rounded border', 53, 161, [255, 255, 255]],
            ['next rounded dash', 55, 161, [170, 0, 0]],
            ['square corner of a partly rounded dashed border', 308, 161, [170, 0, 0]],
            ['other square corner', 201, 218, [170, 0, 0]],
            ['dot on a square corner of a partly rounded border', 348, 162, [170, 0, 0]],
        ];
        for (const [name, x, y, want] of checks) {
            const got = await pixel(x, y);
            if (!near(got, want)) fail(name + ': ' + got);
        }
        "#,
        &[(
            "paint.html",
            "<body style='margin:0;background:#fff'>\
             <div style='position:absolute;left:0;top:0;width:60px;height:60px;border:10px solid rgba(0,0,0,.5);background:linear-gradient(#f00,#f00)'></div>\
             <div style='position:absolute;left:120px;top:20px;width:60px;height:50px;outline:8px groove rgb(200,150,100)'></div>\
             <div style='position:absolute;left:220px;top:20px;width:100px;height:40px;background:linear-gradient(#00f,#00f) 0 0/40px 40px space'></div>\
             <div style='position:absolute;left:220px;top:100px;width:60px;height:40px;border-style:dashed;border-color:#a00;border-width:2px 8px 4px 1px'></div>\
             <div style='position:absolute;left:20px;top:120px;width:100px;height:20px;border-style:dotted;border-color:#00a;border-width:6px 0 2px 0'></div>\
             <div style='position:absolute;left:40px;top:160px;width:106px;height:52px;border:2px dashed #a00;border-radius:8px'></div>\
             <div style='position:absolute;left:200px;top:160px;width:102px;height:52px;border:4px dashed #a00;border-radius:20px 0 0 0'></div>\
             <div style='position:absolute;left:346px;top:160px;width:102px;height:52px;border:4px dotted #a00;border-radius:0 20px 0 20px'></div>\
             </body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn text_strokes_draw_hollow_glyphs_like_chrome() {
    run_hidden(
        r#"
        await load('stroke.html');
        await win.webContents.capturePage();
        // One row through the middle of the `I`, as [red, green, blue] per pixel.
        const row = async y => {
            const bytes = (await win.webContents.capturePage({ x: 0, y, width: 100, height: 1 })).toBitmap();
            return Array.from({ length: 100 }, (_, x) => [bytes[x * 4 + 2], bytes[x * 4 + 1], bytes[x * 4]]);
        };
        // Runs of pixels: b for the stroke, w for white or the fill.
        const runs = (pixels, stroke) => pixels.map(([r, g, b]) => stroke(r, g, b) ? 'b' : 'w').join('').replace(/(.)\1*/g, '$1');
        const blue = (r, g, b) => b > 200 && r < 100 && g < 100;
        if (runs(await row(60), blue) !== 'wbwbw') fail('hollow stroke: ' + runs(await row(60), blue));
        const green = (r, g, b) => g > 100 && r < 80 && b < 80;
        if (runs(await row(160), green) !== 'wbwbw') fail('strokeText: ' + runs(await row(160), green));
        const selected = await row(260);
        if (selected.some(pixel => blue(...pixel))) fail('a ::selection style keeps the stroke');
        "#,
        &[(
            "stroke.html",
            "<style>body{margin:0;background:#fff}div{height:100px;font:700 100px/100px Helvetica;-webkit-text-stroke:4px #00f;-webkit-text-fill-color:transparent}\
             #s::selection{background:#fff}</style>\
             <div>I</div><canvas id=c width=100 height=100 style=display:block></canvas><div id=s>I</div>\
             <script>const x=document.getElementById('c').getContext('2d');x.font='700 100px Helvetica';x.lineWidth=4;x.strokeStyle='#0a0';x.strokeText('I',0,95);\
             const r=document.createRange();r.selectNodeContents(document.getElementById('s'));getSelection().addRange(r)</script>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn focus_rings_paint_like_chrome() {
    run_hidden(
        r#"
        const pixel = async (x, y) => [...(await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap().subarray(0, 3)].reverse();
        await load('ring.html');
        await win.webContents.capturePage();
        const near = (got, want) => got.every((value, at) => Math.abs(value - want[at]) < 12);
        const checks = [
            ['ring on the border edge', 20, 40, [0, 0, 255]],
            ['white ring outside', 18, 40, [255, 255, 255]],
            ['box inside the ring', 22, 40, [128, 128, 128]],
            ['round corner of 3px', 18, 18, [51, 51, 51]],
            ['ring inside a border', 121, 40, [0, 0, 255]],
            ['border under the ring', 123, 40, [0, 255, 0]],
            ['outline color keyword', 220, 40, [0, 95, 204]],
        ];
        for (const [name, x, y, want] of checks) {
            const got = await pixel(x, y);
            if (!near(got, want)) fail(name + ': ' + got);
        }
        "#,
        &[(
            "ring.html",
            "<body style='margin:0;background:#333'>\
             <div style='position:absolute;left:20px;top:20px;width:60px;height:40px;background:#808080;outline:auto;outline-color:#00f'></div>\
             <div style='position:absolute;left:120px;top:20px;width:60px;height:40px;background:#808080;border:4px solid #0f0;outline:auto #00f'></div>\
             <div style='position:absolute;left:220px;top:20px;width:60px;height:40px;background:#808080;outline:auto 1px -webkit-focus-ring-color'></div>\
             </body>",
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
fn computed_grid_templates_give_used_tracks_like_chrome() {
    run_hidden(
        r#"
        await load('grid.html');
        const read = () => run("['a', 'b', 'c', 'd', 'e', 'h'].map(id => getComputedStyle(document.getElementById(id)).gridTemplateColumns + ' / ' + getComputedStyle(document.getElementById(id)).gridTemplateRows).join('|')");
        const first = await read();
        const expected = '264px 32px / 10px|100px 100px 100px / 20px 10px|50px 50px 50px 50px 50px 50px / 10px|[a] 100px [b c] 200px [d] / 10px|300px / 10px 10px|1fr 2fr / none';
        if (first !== expected) fail('tracks: ' + first);
        await run("document.getElementById('a').style.width = '200px'");
        await win.webContents.capturePage();
        const second = await read();
        if (!second.startsWith('164px 32px / 10px|')) fail('resized: ' + second);
        "#,
        &[(
            "grid.html",
            "<body style='margin:0'><style>.g{display:grid;width:300px}.g>div{height:10px}</style>\
             <div class=g id=a style='grid-template-columns:1fr 32px;gap:4px'><div></div><div></div></div>\
             <div class=g id=b style='grid-template-columns:repeat(3,1fr);grid-template-rows:20px auto'><div></div><div></div><div></div><div></div></div>\
             <div class=g id=c style='grid-template-columns:repeat(auto-fill,minmax(50px,1fr))'><div></div></div>\
             <div class=g id=d style='grid-template-columns:[a] 100px [b c] auto [d]'><div></div></div>\
             <div class=g id=e><div></div><div></div></div>\
             <div id=h style='grid-template-columns:1fr 2fr'></div></body>",
        )],
    );
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
fn grids_size_like_chrome_in_flex_boxes_and_collapsed_rows() {
    let text = run_hidden(
        &format!(
            r#"{RECTS}
            await load('grid.html');
            console.log('RECTS ' + await rects());
            "#
        ),
        &[(
            "grid.html",
            "<body style='margin:0;font:16px sans-serif'><style>.w{width:300px}i{display:block;height:31px}li{height:30px}ul{margin:0}</style>\
             <div class=w style='display:flex;flex-direction:column;height:60px;gap:2px;overflow:auto'>\
             <div id=a style='display:grid'><i></i></div><div id=b style='display:grid;height:31px'><i></i></div><div style='display:grid'><i></i></div></div>\
             <div class=w><div id=c style='display:grid;grid-template-rows:0fr'><ul style='min-height:0;overflow:hidden'><li></li><li></li></ul></div></div>\
             <div class=w><div id=d style='display:grid;grid-template-rows:0fr'><ul style='min-height:0'><li></li><li></li></ul></div></div>\
             <div class=w><div id=e style='display:grid;grid-template-rows:0fr'><ul><li></li><li></li></ul></div></div>\
             <div class=w><button id=f style='display:grid;padding:0 6px;border:0;font:inherit'>212</button></div>\
             <div class=w><button id=g style='display:block;padding:0 6px;border:0;font:inherit'>212</button></div>\
             <div class=w><ul style='display:flex;padding:0;list-style:none'><li><div id=h style='display:grid;padding:0 6px'>212</div></li></ul></div>\
             <div class=w style='display:grid;place-items:center'><div id=k style='display:flex;flex-wrap:wrap;gap:20px;width:100%'>\
             <div id=m style='flex:1 1 100px;min-width:0'><div style='aspect-ratio:4'></div></div>\
             <div style='flex:1 1 100px;min-width:0;padding:0 20px;border:5px solid;box-sizing:border-box;height:10px'></div></div></div>\
             <div class=w style='display:grid;place-items:center'><div id=n style='display:flex;flex-wrap:wrap;gap:20px;width:100%'>\
             <div style='flex:0 0 0px'></div><div style='flex:1 1 350px;min-width:0'><div style='aspect-ratio:4'></div></div></div></div>\
             <div class=w><div id=o style='display:grid;grid-template-rows:0.5fr'><i style='overflow:hidden'></i></div></div>\
             <div class=w><div id=p style='display:grid;grid-template-rows:0fr 1fr'><i style='overflow:hidden'></i><i style='overflow:hidden'></i></div></div>\
             <div class=w><div id=q style='display:grid;grid-template-rows:minmax(auto,10px)'><i style='min-height:0'></i></div></div>\
             <div class=w style='display:flex;align-items:flex-start'><div id=r style='display:grid;grid-template-rows:0.3fr'><i style='overflow:hidden'></i></div></div>\
             <div class=w style='display:grid;width:150px;grid-template-columns:1fr'><div id=s style='width:100%;overflow:hidden;white-space:nowrap'>a very long single line label that overflows</div></div>\
             <div class=w style='display:grid;width:60px'><b id=t style='overflow:hidden;width:50%'>longwordhere</b></div>\
             <div class=w><div id=u style='display:grid;grid-template-rows:0fr'><b style='display:block;min-height:min-content;overflow:hidden'><i></i></b></div></div>\
             <div class=w><div id=w style='display:grid;grid-template-rows:0fr'><b id=v style='display:block;aspect-ratio:6;overflow:hidden'></b></div></div>\
             <div class=w><div id=x style='display:grid;grid-template-rows:0.5fr'><ul id=y style='min-height:0;overflow:hidden'><li></li><li></li></ul></div></div>\
             <div class=w><div id=z style='display:grid;grid-template-rows:0fr'><ul id=zz style='min-height:min-content'><li></li><li></li></ul></div></div>\
             <div class=w><div id=mx style='display:grid;grid-template-rows:0.5fr;max-height:30px'><ul id=my style='min-height:0;overflow:hidden'><li></li><li></li><li></li><li></li></ul></div></div>\
             </body>",
        )],
    );
    let rects = rects(&text);
    let field = |id: &str, index: usize| -> f32 {
        rects[id].split(',').nth(index).unwrap().parse().unwrap()
    };
    assert_eq!(
        [field("a", 3), field("b", 3)],
        [31., 31.],
        "grid flex items keep their rows: {rects:?}"
    );
    assert_eq!(
        [field("c", 3), field("d", 3), field("e", 3)],
        [0., 0., 60.],
        "0fr rows collapse to the minimum contributions: {rects:?}"
    );
    assert!(field("f", 2) > 20., "{rects:?}");
    assert_eq!(field("f", 2), field("g", 2), "{rects:?}");
    assert_eq!(field("h", 2), field("g", 2), "{rects:?}");
    assert_eq!(
        [field("m", 2), field("k", 3)],
        [140., 35.],
        "wrapped lines grow their items before they size: {rects:?}"
    );
    assert_eq!(
        field("n", 3),
        95.,
        "an empty first item takes a line: {rects:?}"
    );
    assert_eq!(
        [field("o", 3), field("p", 3), field("q", 3), field("r", 3)],
        [31., 62., 31., 31.],
        "items with a height keep it as their minimum: {rects:?}"
    );
    assert_eq!(
        [
            field("s", 2),
            field("t", 2),
            field("u", 3),
            field("v", 3),
            field("w", 3)
        ],
        [150., 30., 0., 50., 0.],
        "percent sizes and content keywords count as auto, a ratio keeps its height: {rects:?}"
    );
    assert_eq!(
        [
            field("x", 3),
            field("y", 3),
            field("z", 3),
            field("zz", 3),
            field("mx", 3),
            field("my", 3)
        ],
        [30., 15., 0., 60., 30., 15.],
        "rows below 1fr size again in the grid height, a content keyword min-height gives no row size: {rects:?}"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn grid_items_take_percentages_from_their_grid_area() {
    let text = run_hidden(
        &format!(
            r#"{RECTS}
            await load('grid.html');
            await win.webContents.capturePage();
            await win.webContents.capturePage();
            console.log('PADDING ' + await run("['a', 'd', 'e'].map(id => getComputedStyle(document.getElementById(id)).paddingLeft).join()"));
            console.log('RECTS ' + await rects());
            "#
        ),
        &[(
            "grid.html",
            "<body style='margin:0'><style>.w{width:400px;display:grid}</style>\
             <div class=w style='grid-template-columns:200px 100px'><div id=a style='padding:10% 5%;margin:5%;width:50px;height:20px'></div></div>\
             <div class=w style='height:200px;grid-template-rows:1fr 1fr'><div id=b style='min-height:75%;height:0'></div><div></div></div>\
             <div class=w style='grid-template-columns:200px;grid-template-rows:100px'><div id=c style='width:50%;max-width:40%;max-height:30%'></div></div>\
             <div class=w style='grid-template-columns:300px'><div id=d style='display:table-cell;padding-left:10%;height:10px'></div></div>\
             <div class=w style='grid-template-columns:200px 100px'><div style='display:contents'><div id=e style='padding-left:10%;height:5px'></div></div></div>\
             </body>",
        )],
    );
    let rects = rects(&text);
    let padding = text
        .lines()
        .find_map(|line| line.strip_prefix("PADDING "))
        .unwrap();
    assert_eq!(
        [&*rects["a"], &*rects["b"], &*rects["c"], padding],
        ["10,10,70,60", "0,0,400,75", "0,0,80,30", "10px,30px,20px"],
        "{text}"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn absolute_boxes_of_right_to_left_blocks_align_from_the_right() {
    let text = run_hidden(
        &format!(
            r#"{RECTS}
            await load('rtl.html');
            console.log('RECTS ' + await rects());
            "#
        ),
        &[(
            "rtl.html",
            "<body style='margin:0'><style>.w{position:relative;width:400px;height:20px;direction:rtl}\
             i{position:absolute;left:10px;right:10px;width:100px;height:10px}</style>\
             <div class=w><i id=a></i></div>\
             <div class=w><i id=b style='direction:ltr'></i></div>\
             <div class=w><i id=c style='margin:0 20px'></i></div>\
             <div class=w><i id=d style='width:500px;margin:0 auto'></i></div>\
             <div class=w><i id=e style='margin:0 auto'></i></div>\
             <div class=w><i id=f style='margin-right:auto'></i></div>\
             <div class=w style='direction:ltr'><i id=g></i></div>\
             <div class=w><div><p style='margin:0;direction:ltr'>x <i id=h></i></p></div></div>\
             <div class=w style='overflow:hidden'><i id=k></i></div>\
             <div class=w style='direction:ltr'><div style='direction:rtl'><i id=l></i></div></div>\
             <div class=w><i id=m style='left:0;right:0;justify-self:end'></i></div>\
             <div class=w><i id=n style='left:0;right:0;justify-self:center'></i></div>\
             <div class=w><i id=o style='left:auto;right:auto;margin-right:30px'></i></div>\
             <div class=w style='display:flex;justify-content:flex-end'><i id=p style='left:auto;right:auto'></i></div>\
             <div class=w style='display:grid;grid-template-columns:100px 200px'><i id=q style='grid-column:2'></i></div>\
             </body>",
        )],
    );
    let rects = rects(&text);
    let x = |id: &str| rects[id].split(',').next().unwrap().to_string();
    assert_eq!(
        [
            "a", "b", "c", "d", "e", "f", "g", "h", "k", "l", "m", "n", "o", "p", "q"
        ]
        .map(x),
        [
            "290", "290", "270", "-110", "150", "10", "10", "290", "290", "10", "0", "150", "270",
            "0", "190"
        ],
        "{rects:?}"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn grid_items_measure_their_content_like_layout() {
    let text = run_hidden(
        &format!(
            r#"{RECTS}
            await load('grid.html');
            console.log('RECTS ' + await rects());
            "#
        ),
        &[(
            "grid.html",
            "<body style='margin:0;font:13px/1.45 sans-serif'><style>.w{width:300px}.g{display:grid;place-items:center;height:240px}\
             .c{display:flex;flex-direction:column;gap:12px;width:272px}.r{display:flex;align-items:baseline;margin:0}\
             .big{font:30px/1 serif}strong{font:26px/1 serif;font-variant-numeric:tabular-nums}\
             .t{display:inline-flex;align-items:center;gap:4px;font-size:13px;line-height:22px}\
             .h{display:inline-flex;align-items:center;gap:6px}.h>i{width:14px;height:14px}.h strong{font:inherit;font-variant-numeric:tabular-nums}</style>\
             <div class='w g'><div class=c id=a><i style='height:20px'></i><p class=r><span>Extras</span><span class=big>$35</span></p></div></div>\
             <div class=w><div class=c id=b><i style='height:20px'></i><p class=r><span>Extras</span><span class=big>$35</span></p></div></div>\
             <div class='w g'><div class=c id=c><p><strong>371</strong> stays</p></div></div>\
             <div class=w><div class=c id=d><p><strong>371</strong> stays</p></div></div>\
             <div class='w g'><div id=e style='font:14px/1.75 sans-serif;width:240px'>Compare <span class=t><svg width=14 height=14></svg>Long Fog</span> with \
             <span class=t><svg width=14 height=14></svg>Thaw</span> Why <span class=t><svg width=14 height=14></svg>Lightning Rods</span></div></div>\
             <div class=w><div id=f style='font:14px/1.75 sans-serif;width:240px'>Compare <span class=t><svg width=14 height=14></svg>Long Fog</span> with \
             <span class=t><svg width=14 height=14></svg>Thaw</span> Why <span class=t><svg width=14 height=14></svg>Lightning Rods</span></div></div>\
             <div class='w g' style='font:12.5px sans-serif'><div class=h><i id=i></i><span id=s>Wake up at<!-- --> <strong>12<!-- -->:<!-- -->30<!-- --> <!-- -->AM</strong></span></div></div>\
             </body>",
        )],
    );
    let rects = rects(&text);
    let field = |id: &str, index: usize| -> f32 {
        rects[id].split(',').nth(index).unwrap().parse().unwrap()
    };
    // The content height of a grid item is the height that layout gives its content, as in a block.
    assert_eq!(
        field("a", 3),
        field("b", 3),
        "baseline-aligned row: {rects:?}"
    );
    assert_eq!(
        field("c", 3),
        field("d", 3),
        "text with font features: {rects:?}"
    );
    // Layout snaps each baseline to a device pixel, which the measure can miss by a fraction.
    assert!(
        (field("e", 3) - field("f", 3)).abs() < 1. && field("f", 3) > 40.,
        "inline-flex boxes on the lines: {rects:?}"
    );
    // An inline-flex box sized by its content fits its text on one line.
    assert_eq!(field("i", 2), 14., "{rects:?}");
    assert!(field("s", 3) < 20., "{rects:?}");
}

#[test]
#[ignore = "requires the macOS window server"]
fn inline_flex_and_grid_boxes_measure_on_their_first_row_baseline() {
    let text = run_hidden(
        &format!(
            r#"{RECTS}
            await load('grid.html');
            // Inline boxes take their size from the last frame.
            await win.webContents.capturePage();
            console.log('RECTS ' + await rects());
            "#
        ),
        &[(
            "grid.html",
            "<body style='margin:0;font:13px/1.45 sans-serif'><style>.w{width:300px}.g{display:grid;align-items:start}\
             .x{display:inline-grid;grid-template-columns:30px auto}.y{display:inline-flex}b{display:block;height:30px;width:20px}</style>\
             <div class='w g'><div id=a>A <span class=x><b></b><span>grid</span></span> Z</div></div>\
             <div class=w><div id=b>A <span class=x><b></b><span>grid</span></span> Z</div></div>\
             <div class='w g'><div id=c>A <span class=y><b></b><span>flex</span></span> Z</div></div>\
             <div class=w><div id=d>A <span class=y><b></b><span>flex</span></span> Z</div></div>\
             </body>",
        )],
    );
    let rects = rects(&text);
    let height = |id: &str| -> f32 { rects[id].split(',').nth(3).unwrap().parse().unwrap() };
    // The first item has no baseline, so the box sits on its bottom edge and the line grows below it.
    assert!(height("b") > 33., "{rects:?}");
    assert_eq!(height("a"), height("b"), "inline-grid: {rects:?}");
    assert_eq!(height("c"), height("d"), "inline-flex: {rects:?}");
}

#[test]
#[ignore = "requires the macOS window server"]
fn positioned_boxes_that_leave_a_paragraph_give_back_its_height() {
    let text = run_hidden(
        r#"
        await load('grid.html');
        const height = () => run("['a', 'b', 'm', 'n'].map(id => (r => id < 'm' ? r.height : r.x)(document.getElementById(id).getBoundingClientRect())).join()");
        const before = await height();
        // Like Chrome, a placed out-of-flow child leaves the lines of the paragraph as they are.
        await run("for (const id of ['m', 'n']) { const f = document.createElement('i'); f.className = 'f'; f.style.cssText = 'position:fixed;width:10px;height:10px'; document.getElementById(id).after(f) }");
        await win.webContents.capturePage();
        await run("for (const f of document.querySelectorAll('.f')) Object.assign(f.style, { left: '0px', top: '0px' })");
        await win.webContents.capturePage();
        const during = await height();
        const box = await run("JSON.stringify(document.querySelector('.f').getBoundingClientRect())");
        if (box !== JSON.stringify({ x: 0, y: 0, width: 10, height: 10, top: 0, right: 10, bottom: 10, left: 0 })) fail('placed box: ' + box);
        if (during !== before) fail('during: ' + before + ' ' + during);
        await run("const p = document.createElement('div'); p.popover = 'manual'; p.id = 'pop'; p.style.cssText = 'position:fixed;inset:10px auto auto 10px;margin:0;padding:0;border:0'; p.innerHTML = '<div style=\\'height:40px\\'></div>'; document.getElementById('m').after(p); p.showPopover()");
        await win.webContents.capturePage();
        const pop = await run("document.getElementById('pop').getBoundingClientRect().height");
        if (pop !== 40 || await height() !== before) fail('popover: ' + pop);
        await run("document.getElementById('pop').remove()");
        // A positioned box after a placed child paints above it, in DOM order.
        const top = await run("(p => { const r = p.getBoundingClientRect(); return document.elementFromPoint(r.x + 5, r.y + 5).id })(document.getElementById('z'))");
        if (top !== 'zr') fail('paint order: ' + top);
        await run("for (const f of document.querySelectorAll('.f')) f.remove()");
        await win.webContents.capturePage();
        console.log('HEIGHTS ' + before + ' ' + await height());
        "#,
        &[(
            "grid.html",
            "<body style='margin:0;font:14px/1.5 sans-serif'>\
             <div style='display:grid;place-items:center;height:280px;width:300px'><article id=a style='width:250px'>\
             <div>Retry budgets are per tenant now. <a id=m>@lena</a> can you sanity-check the alert thresholds?</div>\
             </article></div>\
             <div style='display:grid;place-items:center;height:280px;width:300px'><article id=b style='width:250px'>\
             <div><span>Retry budgets are per tenant now.</span> <a id=n>@lena</a> <span>can you sanity-check the alert thresholds?</span></div>\
             </article></div>\
             <p id=z style='position:absolute;left:300px;top:0;width:90px;margin:0'><i style='position:absolute;left:0;top:0;width:60px;height:40px'></i><span id=zr style='position:relative'>relative span</span> text</p></body>",
        )],
    );
    let heights = text
        .lines()
        .find_map(|line| line.strip_prefix("HEIGHTS "))
        .unwrap_or_else(|| panic!("{text}"));
    let [before, after] = [0, 1].map(|i| heights.split(' ').nth(i).unwrap());
    assert_eq!(before, after, "{text}");
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
fn grid_items_build_in_place_when_only_their_paint_changes() {
    let page = r#"<style>
        body { margin: 0; }
        #list { display: grid; grid-template-columns: 200px; width: 200px; }
        .row { position: relative; height: 20px; background: rgb(255, 255, 255); --p: 0; }
        .row::after { content: ""; position: absolute; left: 0; top: 0; height: 20px; width: calc(200px * var(--p)); background: rgb(0, 0, 255); }
        </style>
        <div id="list"><div class="row" id="r0"></div><div class="row" id="r1"></div><div class="row" id="r2"></div></div>"#;
    let text = run_hidden(
        &format!(
            r#"{STEPS}
            await load('rows.html');
            for (let index = 1; index <= 4; index++) {{
                await step('p' + index, () => run(`document.getElementById('r1').style.setProperty('--p', '${{index / 4}}')`));
                // The progress of the middle row reaches index / 4 of its width.
                console.log('ROW ' + index + ' ' + await pixel(index * 50 - 5, 30) + ' ' + await pixel(index * 50 + 5, 30));
            }}
            await step('paint', () => run("document.getElementById('r2').style.background = 'rgb(0, 255, 0)'"));
            console.log('PAINT ' + await pixel(100, 50));
            "#
        ),
        &[("rows.html", page)],
    );
    let rows: Vec<&str> = text
        .lines()
        .filter(|line| line.starts_with("ROW "))
        .collect();
    assert_eq!(rows.len(), 4, "{text}");
    for (index, line) in rows.iter().enumerate() {
        // Bitmaps are BGRA. Past the end of the row the next pixel is outside the window at 200px.
        let tail = if index == 3 { "" } else { " 255,255,255" };
        assert!(
            line.contains(" 255,0,0") && line.ends_with(tail),
            "the progress draws to its width: {line}"
        );
    }
    assert!(text.contains("PAINT 0,255,0"), "{text}");
    // Only debug builds name the boxes they build. The rows build in place, and the grid stays.
    for step in ["p2", "p3", "p4", "paint"]
        .into_iter()
        .filter(|_| cfg!(debug_assertions))
    {
        let built = built(&text, step);
        assert!(
            !built.contains(&"list") && !built.is_empty(),
            "{step} builds {built:?}"
        );
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn animated_boxes_in_positioned_boxes_draw_each_frame() {
    let page = r#"<style>
        body { margin: 0; }
        #card { position: relative; width: 200px; height: 100px; background: rgb(255, 255, 255); overflow: hidden; border-radius: 8px; box-shadow: inset 0 0 4px rgb(0 0 0 / 0.2); }
        #card::before { content: ""; position: absolute; inset: 0; z-index: 1; background: rgb(0 0 0 / 0.01); mix-blend-mode: multiply; }
        #art { position: absolute; inset: 0; z-index: 0; }
        #bars { position: absolute; left: 10px; bottom: 10px; height: 60px; display: flex; align-items: flex-end; z-index: 1; }
        #bars i { width: 20px; height: 10px; background: rgb(0, 0, 255); box-shadow: 0 0 3px rgb(0 0 0 / 0.5); animation: grow 500ms linear infinite alternate; }
        @keyframes grow { from { height: 10px; } to { height: 50px; } }
        </style>
        <div id="card"><div id="art"><div id="bars"><i id="a"></i></div></div></div>"#;
    let text = run_hidden(
        &format!(
            r#"{STEPS}
            await load('anim.html');
            for (let index = 0; index < 8; index++) {{
                await wait(130);
                await win.webContents.capturePage();
                // The drawn height of the bar: its blue pixels in a column through it.
                const column = (await win.webContents.capturePage({{ x: 20, y: 30, width: 1, height: 60 }})).toBitmap();
                let blue = 0;
                for (let at = 0; at < column.length; at += 4) blue += column[at] > 200 && column[at + 2] < 60 ? 1 : 0;
                console.log('BAR ' + blue);
            }}
            "#
        ),
        &[("anim.html", page)],
    );
    let bars: Vec<&str> = text
        .lines()
        .filter(|line| line.starts_with("BAR "))
        .collect();
    assert_eq!(bars.len(), 8, "{text}");
    // Bitmaps are BGRA.
    let mut heights = bars.clone();
    heights.sort_unstable();
    heights.dedup();
    assert!(heights.len() >= 4, "the bar draws at new heights: {bars:?}");
}

#[test]
#[ignore = "requires the macOS window server"]
fn positioned_boxes_build_in_place_on_each_frame() {
    let page = r#"<style>
        body { margin: 0; }
        #card { position: relative; width: 200px; height: 100px; background: rgb(255, 255, 255); }
        #bars { position: absolute; left: 10px; bottom: 10px; height: 60px; display: flex; align-items: flex-end; gap: 2px; }
        #bars i { width: 20px; height: 10px; background: rgb(0, 0, 255); }
        </style>
        <div id="card"><div id="bars"><i id="a"></i><i id="b"></i></div></div>"#;
    let text = run_hidden(
        &format!(
            r#"{STEPS}
            await load('bars.html');
            for (let index = 0; index < 12; index++) {{
                const height = 10 + (index * 7) % 50;
                await step('bar' + index, () => run(`document.getElementById('a').style.height = '${{height}}px'`));
                // A pixel 5px above the bottom of the bar is blue, and 5px above its top is white.
                const top = 90 - height;
                console.log('BAR ' + index + ' ' + await pixel(20, top + 5) + ' ' + await pixel(20, top - 5));
            }}
            "#
        ),
        &[("bars.html", page)],
    );
    let bars: Vec<&str> = text
        .lines()
        .filter(|line| line.starts_with("BAR "))
        .collect();
    assert_eq!(bars.len(), 12, "{text}");
    // After the first build, the bars build again in place and the card around them stays. Only debug builds
    // name the boxes they build.
    for index in (1..12).filter(|_| cfg!(debug_assertions)) {
        let built = built(&text, &format!("bar{index}"));
        assert!(
            built.contains(&"bars") && !built.contains(&"card"),
            "step {index} builds {built:?}"
        );
    }
    for line in bars {
        // Bitmaps are BGRA.
        assert!(
            line.ends_with(" 255,0,0 255,255,255"),
            "the bar draws at its height: {line}"
        );
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn positioned_boxes_change_without_moving_the_grid_around_them() {
    let page = r#"<style>
        body { margin: 0; font: 16px monospace; }
        #g { display: grid; grid-template-columns: auto 1fr; width: 300px; }
        #a { position: relative; }
        #abs { position: absolute; left: 0; top: 0; width: 10px; height: 10px; background: red; }
        #row { position: relative; height: 20px; --p: 0; }
        #row::after { content: ""; position: absolute; left: 0; top: 0; height: 20px; width: calc(200px * var(--p)); background: rgb(0, 0, 255); }
        </style>
        <div id="g"><div id="a"><span id="abs"></span>XX</div><div id="b">Y</div></div>
        <div id="row"></div>"#;
    let text = run_hidden(
        &format!(
            r#"{STEPS}
            await load('grid.html');
            const left = () => run("document.getElementById('b').getBoundingClientRect().left");
            const start = await left();
            await step('positioned', () => run("document.getElementById('abs').style.width = '120px'"));
            console.log('POSITIONED ' + (await left() === start));
            await step('both', () => run("document.getElementById('abs').style.width = '60px'; document.getElementById('a').lastChild.data = 'XXXXXX'"));
            console.log('GROWN ' + (await left() > start));
            const y = await run("Math.round(document.getElementById('row').getBoundingClientRect().top) + 10");
            await step('half', () => run("document.getElementById('row').style.setProperty('--p', '0.5')"));
            console.log('HALF ' + await pixel(50, y) + ' ' + await pixel(150, y));
            await step('full', () => run("document.getElementById('row').style.setProperty('--p', '1')"));
            console.log('FULL ' + await pixel(150, y));
            "#
        ),
        &[("grid.html", page)],
    );
    let line = |name: &str| {
        let prefix = format!("{name} ");
        text.lines()
            .find_map(|line| line.strip_prefix(&prefix))
            .unwrap_or_else(|| panic!("{text}"))
            .to_owned()
    };
    assert_eq!(
        line("POSITIONED"),
        "true",
        "an absolutely positioned box leaves the grid"
    );
    assert_eq!(
        line("GROWN"),
        "true",
        "a grid item that grows moves the next column"
    );
    // Bitmaps are BGRA.
    assert_eq!(
        line("HALF"),
        "255,0,0 255,255,255",
        "the pseudo-element is half as wide"
    );
    assert_eq!(
        line("FULL"),
        "255,0,0",
        "the pseudo-element grows with its property"
    );
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
fn out_of_flow_boxes_follow_the_clip_of_their_containing_block() {
    // The box with `overflow: hidden` stays where it was while the clip of the scroller around it changes.
    let page = "<body style='margin:0'><div id=s style='height:200px;overflow:auto'>\
        <div style='position:relative;height:400px'><div style='overflow:hidden;width:100px;height:50px'>\
        <div id=a style='position:absolute;left:0;top:0;width:300px;height:300px;background:red;cursor:pointer'>\
        </div></div></div></div></body>";
    let text = run_hidden(
        &format!(
            r#"{STEPS}
            await load('page.html');
            const s = 'document.getElementById("s")';
            for (const [name, code, y] of [['short', `${{s}}.style.height = '120px'`, 110], ['tall', `${{s}}.style.height = '250px'`, 240], ['scroll', `${{s}}.scrollTop = 30`, 240]]) {{
                await step(name, () => run(code));
                console.log('PIXELS ' + name + ' ' + await pixel(150, y) + ' ' + await pixel(150, y + 15));
            }}
            "#
        ),
        &[("page.html", page)],
    );
    let pixels: Vec<_> = text
        .lines()
        .filter(|line| line.starts_with("PIXELS"))
        .collect();
    assert_eq!(
        pixels,
        [
            "PIXELS short 0,0,255 255,255,255",
            "PIXELS tall 0,0,255 255,255,255",
            "PIXELS scroll 0,0,255 255,255,255",
        ],
        "{text}"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn clip_paths_clip_out_of_flow_descendants_like_chrome() {
    let page = "<body style='margin:0;background:white'><div style='position:relative;width:200px;height:50px'>\
        <div id=c style='height:50px;clip-path:inset(0 50% 0 0)'>\
        <div style='position:absolute;inset:0;background:red'></div></div></div></body>";
    let text = run_hidden(
        &format!(
            r#"{STEPS}
            await load('page.html');
            console.log('PIXELS half ' + await pixel(50, 25) + ' ' + await pixel(150, 25));
            await step('narrow', () => run("document.getElementById('c').style.clipPath = 'inset(0 80% 0 0)'"));
            console.log('PIXELS narrow ' + await pixel(20, 25) + ' ' + await pixel(50, 25));
            "#
        ),
        &[("page.html", page)],
    );
    let pixels: Vec<_> = text
        .lines()
        .filter(|line| line.starts_with("PIXELS"))
        .collect();
    assert_eq!(
        pixels,
        [
            "PIXELS half 0,0,255 255,255,255",
            "PIXELS narrow 0,0,255 255,255,255",
        ],
        "{text}"
    );
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
fn text_fields_and_ranges_size_and_draw_like_chrome() {
    run_hidden(
        r#"
        const pixel = async (x, y) => [...(await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap().subarray(0, 3)].reverse();
        const near = (got, want) => got.every((value, index) => Math.abs(value - want[index]) <= 8);
        const height = id => run(`document.getElementById('${id}').getBoundingClientRect().height`);
        await load('fields.html');
        const [normal, short, tall, wrapped] = [await height('n'), await height('l1'), await height('l2'), await height('t')];
        if (short !== normal || tall !== 41) fail(`input line heights: ${short} ${tall}, normal ${normal}`);
        if (wrapped !== 30) fail('a textarea sized by its content grows for wrapped lines: ' + wrapped);
        const fine = await height('l3');
        if (await run('devicePixelRatio') === 2 && Math.abs(fine - 16.6) > 0.05) fail('line heights compare in device pixels: ' + fine);
        const markers = await run("['m1', 'm2', 'm3'].map(id => document.getElementById(id).getBoundingClientRect().height).join()");
        if (markers !== '22.5,40,30') fail('disclosure markers keep their line height: ' + markers);
        const shot = async id => [...(await win.webContents.capturePage(JSON.parse(await run(`(r => JSON.stringify({ x: r.x, y: r.y, width: r.width, height: r.height }))(document.getElementById('${id}').getBoundingClientRect())`)))).toBitmap()].join();
        if (await shot('u1') !== await shot('u2') || await run("document.getElementById('u1').value") !== 'abc') fail('text-transform draws the field text only');
        const track = await pixel(100, 8), edge = await pixel(50, 20);
        if (!near(track, [59, 59, 59])) fail('a light accent makes the range dark: ' + track);
        if (!near(edge, [59, 59, 186])) fail('the value edge follows the accent: ' + edge);
        "#,
        &[(
            "fields.html",
            "<body style='margin:0;background:#fff'><style>input,textarea{display:block;margin:0;padding:0;border:0}</style>\
             <input type=range id=r1 value=0 style='width:129px;height:16px;accent-color:#ffcc00'>\
             <input type=range id=r2 value=100 style='width:129px;height:16px;accent-color:#0000ff'>\
             <input id=l1 style='font:40px Arial;line-height:1'><input id=l2 style='font:40px Arial;line-height:41px'>\
             <input id=l3 style='font:16px Arial;line-height:16.6px'>\
             <div style='position:absolute;left:200px;top:0;width:150px'>\
             <details><summary id=m1 style='font:20px Arial'></summary></details>\
             <details><summary id=m2 style='font:20px Arial;line-height:40px'></summary></details>\
             <ul style='margin:0;list-style:disclosure-closed inside;font:20px Arial;line-height:30px'><li id=m3></li></ul></div>\
             <span id=n style='font:40px Arial;display:inline-block'>x</span>\
             <textarea id=t style='box-sizing:border-box;width:100px;field-sizing:content;font:10px/10px Arial'>word word word word word word word word word word</textarea>\
             <input id=u1 value=abc style='font:20px Arial;width:80px;height:24px;text-transform:uppercase'>\
             <input id=u2 value=ABC style='font:20px Arial;width:80px;height:24px'></body>",
        )],
    );
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
fn percent_indents_and_huge_line_heights_lay_out_like_chrome() {
    let (text, rects) = picture_rects(
        "indent.html",
        "<body style='margin:0;font:16px sans-serif'><div class=w style='width:300px;padding:0 10px;text-indent:10%'><span id=a>Hello</span></div>\
         <div class=w style='width:300px;box-sizing:border-box;padding:0 50px;text-indent:calc(10% + 1em)'><span id=b>Hello</span></div>\
         <div class=w style='line-height:1e40;height:20px;overflow:hidden'><span id=c>Hello</span></div>\
         <div class=w style='width:400px'><div style='width:600px;max-width:100%;text-indent:25%'><span id=d>Hello</span></div></div>\
         <div class=w style='width:400px;display:flex'><div style='width:600px;text-indent:25%'><span id=e>Hello</span></div></div>\
         <div class=w style='width:400px;height:20px;position:relative'><div style='position:absolute;left:0;right:100px;text-indent:25%'><span id=f>Hello</span></div></div>\
         <div class=w style='width:400px'><div style='display:table'><div style='display:table-cell;width:200px;text-indent:25%'><span id=g>Hello</span></div></div></div>\
         <div class=w style='width:400px;display:flex;text-indent:25%'><span id=h>Hello</span></div></body>",
    );
    let at = |id: &str, index: usize| {
        rects[id]
            .split(',')
            .nth(index)
            .unwrap()
            .parse::<f32>()
            .unwrap()
    };
    assert_eq!(at("a", 0), 40., "10% of the content width: {text}");
    assert_eq!(
        at("b", 0),
        86.,
        "of the content box of a border box: {text}"
    );
    assert!(
        at("c", 1).is_finite() && at("c", 1) < 33_554_432.,
        "a line is at most 2^25 px: {text}"
    );
    assert_eq!(
        at("d", 0),
        100.,
        "of the used width under max-width: {text}"
    );
    assert_eq!(
        at("e", 0),
        100.,
        "of the used width of a flex item that shrinks: {text}"
    );
    assert_eq!(
        at("f", 0),
        75.,
        "of an absolute box between its offsets: {text}"
    );
    assert_eq!(at("g", 0), 50., "of a table cell: {text}");
    assert!(
        at("h", 0) < 10.,
        "of the anonymous item of a flex container: {text}"
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
fn top_aligned_spans_keep_fixed_line_heights_like_chrome() {
    let (text, rects) = picture_rects(
        "top.html",
        "<body style='margin:0;font:14px/20px Helvetica'><style>.w{width:300px}</style>\
         <div class=w><p id=p0 style='margin:0'>Ab <span style='font-size:17px;vertical-align:top'>Hxg</span> z</p></div>\
         <div class=w><p id=p1 style='margin:0'>Ab <span style='font-size:22px;vertical-align:top'>Hxg</span> z</p></div>\
         <div class=w><p id=p2 style='margin:0'>Ab <span style='font-size:22px;font-family:Menlo;vertical-align:top'>Hxg</span> z</p></div></body>",
    );
    for id in ["p0", "p1", "p2"] {
        assert_eq!(rects[id], "0,0,300,20", "{id}, as in Chrome: {text}");
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
fn inline_boxes_keep_lines_without_hanging_spaces_and_fill_columns_like_chrome() {
    let (text, rects) = picture_rects(
        "fit.html",
        "<body style='margin:0;font:12px/16px Helvetica'><style>.w{width:300px}\
         .b{display:inline-block;height:12px;vertical-align:top}.z{font-size:0}.z .b{font-size:12px}</style>\
         <div class=w><span class=b style='width:148px'></span> <span id=e0 class=b style='width:148px'></span> <span id=e1 class=b style='width:40px'></span></div>\
         <div class=w><span class=b style='width:149px'></span> <span id=e2 class=b style='width:148px'></span></div>\
         <div class=w style='text-align:right'><span id=r0 class=b style='width:148px'></span> <span id=r1 class=b style='width:148px'></span> <span class=b style='width:40px'></span></div>\
         <div class=w style='text-align:center'><span id=c0 class=b style='width:148px'></span> <span id=c1 class=b style='width:148px'></span> <span class=b style='width:40px'></span></div>\
         <div class='w z'><span class=b style='width:50%'></span> <span id=z0 class=b style='width:50%'></span> <span id=z1 class=b style='width:40px'></span></div>\
         <div class=w><span class=b style='width:138px;margin-right:10px'></span> <span id=m0 class=b style='width:138px;margin-right:10px'></span> <span id=m1 class=b style='width:40px'></span></div>\
         <div class=w><span class=b style='width:148px'></span> <span class=b style='width:148px'></span> <span class=b style='width:296px'></span><span id=s0 class=b style='width:10px'></span></div>\
         <div class=w><span class=b style='width:100%'></span> <span id=p0 class=b style='width:100%'></span></div>\
         <div class=w style='columns:2;column-gap:10px;width:410px'><span class=b style='width:100%'></span> <span id=k0 class=b style='width:100%'></span> <span id=k1 class=b style='width:100%'></span></div></body>",
    );
    for (id, [x, y]) in [
        ("e0", [151.34, 0.]),
        ("e1", [0., 16.]),
        ("e2", [0., 16.]),
        ("r0", [0.66, 0.]),
        ("r1", [152., 0.]),
        ("c0", [0.33, 0.]),
        ("c1", [151.66, 0.]),
        ("z0", [150., 0.]),
        ("z1", [0., 16.]),
        ("m0", [151.34, 0.]),
        ("m1", [0., 16.]),
        ("s0", [0., 32.]),
        ("p0", [0., 16.]),
        ("k0", [0., 16.]),
        ("k1", [210., 0.]),
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
fn inline_content_between_blocks_fills_its_lines_like_chrome() {
    let (text, rects) = picture_rects(
        "runs.html",
        "<body style='margin:0;font:14px/20px Helvetica'><style>.w{width:192px}.ib{display:inline-block;padding:0 3px}</style>\
         <div class=w>Some text then<div>a block child</div>and text after <span id=i0 class=ib>ib</span> trailing \
         <b id=b0>words</b> to wrap a bit more.<div id=d0>end</div></div></body>",
    );
    for (id, rect) in [
        ("i0", "85.61,40,16.9,20"),
        ("b0", "0,62,41.23,16"),
        ("d0", "0,80,192,20"),
    ] {
        assert_eq!(rects[id], rect, "{id}, as in Chrome: {text}");
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn inline_blocks_shorter_than_their_text_keep_its_baseline_like_chrome() {
    let (text, rects) = picture_rects(
        "short.html",
        "<body style='margin:0;font:16px/24px Helvetica'><style>.w{width:300px}\
         .b{display:inline-block;width:140px}</style><div class=w><span id=a0 class=b style='height:0'>t</span>\
         <span class=b style='height:0'>t</span><span id=a2 class=b style='height:5px'>t</span>\
         <span class=b style='height:5px'>t</span></div></body>",
    );
    for (id, rect) in [("a0", "0,0,140,0"), ("a2", "0,24,140,5")] {
        assert_eq!(rects[id], rect, "{id}, as in Chrome: {text}");
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
fn percent_heights_tables_and_measured_lines_like_chrome() {
    let (text, rects) = picture_rects(
        "sizes.html",
        "<!doctype html><body style='margin:0;font:12px/20px Helvetica'><div class=w style='width:400px'>\
         <style>.c{width:120px}.ib{display:inline-block;width:60px;border:1px solid}td{padding:4px}</style>\
         <div class=c style='height:100px'><div><p id=block style='margin:0;height:50%'>a</p></div></div>\
         <div class=c style='display:flex;flex-direction:column;height:100px'><div><p id=flex style='margin:0;height:50%'>a</p></div></div>\
         <div class=c style='display:flex;flex-direction:column;min-height:100px'><div id=grow style='flex:1'><p id=minh style='margin:0;height:50%'>a</p></div></div>\
         <div class=c style='display:grid;min-height:60px'><div id=row>a</div></div>\
         <div class=c><table><tr><td id=cell style='width:2000px'>wide cell text</td><td id=x>x</td></tr></table></div>\
         <div style='display:grid;grid-template-columns:120px'><div id=line><span class=ib>a</span> <span class=ib>b</span></div></div>\
         <div class=c><svg id=svg viewBox='0 0 20 10' style='border:2px solid'></svg></div>\
         <div style='columns:2;column-gap:20px'><div id=col style='width:50%'>a</div></div>\
         <div style='height:100px'><div style='display:flex'><div id=full style='height:100%'>b</div><div style='height:60px;width:5px'></div></div></div>\
         <div style='display:grid;grid-template-columns:max-content'><div id=lead>\n<span class=ib>a</span><span class=ib>b</span></div></div>\
         <div style='display:grid;grid-template-columns:max-content'><svg id=icon viewBox='0 0 20 10'></svg></div></div></body>",
    );
    for (id, rect) in [
        ("block", "0,0,120,20"),
        ("flex", "0,100,120,10"),
        ("grow", "0,200,120,100"),
        ("minh", "0,200,120,20"),
        ("row", "0,300,120,60"),
        ("cell", "2,362,100,28"),
        ("x", "104,362,14,28"),
        ("line", "0,392,120,44"),
        ("svg", "0,436,120,62"),
        ("col", "0,504,95,20"),
        ("full", "0,524,6.68,20"),
        ("lead", "0,624,124,22"),
        ("icon", "0,646,0,0"),
    ] {
        assert_eq!(rects[id], rect, "{id}, as in Chrome: {text}");
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn relative_percent_offsets_need_a_definite_height_like_chrome() {
    let (text, rects) = picture_rects(
        "relative.html",
        "<!doctype html><body style='margin:0;font:12px/20px Helvetica'><div class=w style='width:400px'>\
         <div id=auto style='padding:1px;position:relative;top:10%'>a</div>\
         <div style='height:200px'><div id=fixed style='position:relative;top:10%;height:10px'></div></div>\
         <div><div id=bottom style='position:relative;bottom:10%;height:10px'></div></div>\
         <div style='display:flex;height:100px'><div id=item style='position:relative;top:10%'>e</div></div>\
         <div style='display:flex'><div id=row style='position:relative;top:calc(10% + 3px)'>f</div><div style='height:100px;width:5px'></div></div></div></body>",
    );
    for (id, rect) in [
        ("auto", "0,0,400,22"),
        ("fixed", "0,42,400,10"),
        ("bottom", "0,222,400,10"),
        ("item", "0,242,6.68,100"),
        ("row", "0,332,3.34,100"),
    ] {
        assert_eq!(rects[id], rect, "{id}, as in Chrome: {text}");
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn inline_pseudos_empty_boxes_grid_offsets_and_scroll_transforms_lay_out_like_chrome() {
    let text = run_hidden(
        &format!(
            "{RECTS} await load('page.html');
            await win.webContents.capturePage();
            console.log('RECTS ' + await rects());
            console.log('SCROLL ' + await run(\"document.getElementById('scroll').scrollWidth\"));"
        ),
        &[(
            "page.html",
            "<!doctype html><body style='margin:0;font:16px/20px Helvetica'><div class=w style='width:400px'>\
             <style>.b::before{content:'*'}.g{display:grid;grid-template-columns:100px 100px;grid-template-rows:100px;position:relative}\
             .f{float:left;width:40px;height:30px}.s{width:150px;height:60px;overflow:auto}</style>\
             <ul style='margin:0'><li class=b id=li>text<ul><li>y</li></ul></li></ul>\
             <p style='margin:16px 0'><span id=pad style='padding:3px;border:1px solid'></span></p>\
             <p style='margin:16px 0'><span></span></p><div id=after>a</div>\
             <div class=g><div id=rel style='position:relative;top:20%;height:10px'></div>\
             <div id=abs style='position:absolute;width:20px;height:20px;justify-self:center;align-self:end'></div></div>\
             <div style='height:100px'><i id=ib style='display:inline-block;width:20px;height:50%'></i></div>\
             <details open style='height:60px'><summary>s</summary><div id=det style='height:50%'>t</div></details>\
             <div style='display:grid'><div id=floats><div class=f></div><div class=f style='clear:left'></div></div></div>\
             <div class=s id=scroll><div style='width:150px;height:20px;transform:translateX(50px)'></div></div>\
             <video id=video width=100 height=50 style='width:auto;height:auto'></video>\
             <div><p>y</p><p id=trail><span></span></p></div>\
             <div style='margin:10px 0' id=vpad><span style='padding-top:5px'></span></div>\
             <div><div style='margin-bottom:10px'>a</div><div id=etop style='margin:20px 0 35px'></div></div>\
             <div style='display:grid'><div id=clr><div class=f></div><div class=f style='clear:right'></div></div></div>\
             <div style='display:flex;height:100px'><div><img id=fimg style='height:30%;width:10px'></div></div></div></body>",
        )],
    );
    let rects = rects(&text);
    for (id, rect) in [
        ("li", "40,0,360,40"),
        ("pad", "0,52.5,8,26.5"),
        ("after", "0,92,400,20"),
        ("rel", "0,132,100,10"),
        ("abs", "190,192,20,20"),
        ("ib", "0,212,20,50"),
        ("det", "0,332,400,20"),
        ("floats", "0,372,400,60"),
        ("scroll", "0,432,150,60"),
        ("video", "0,492,400,200"),
        ("trail", "0,748.5,400,0"),
        ("vpad", "0,748.5,400,0"),
        ("etop", "0,788.5,400,0"),
        ("clr", "0,803.5,400,30"),
        ("fimg", "0,833.5,10,30"),
    ] {
        assert_eq!(rects[id], rect, "{id}, as in Chrome: {text}");
    }
    assert!(text.contains("SCROLL 200"), "as in Chrome: {text}");
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
fn blurred_shadows_of_mixed_radii_have_no_seams_like_chrome() {
    run_hidden(
        r#"
        const pixel = async (x, y) => [...(await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap().subarray(0, 3)].reverse();
        await load('shadow.html');
        // Each side of the box takes its own corners: the shadow beside a straight edge under a big corner is smooth.
        const [above, below] = [await pixel(158, 139), await pixel(158, 141)];
        if (Math.abs(above[0] - below[0]) > 4) fail('seam: ' + above + ' / ' + below);
        "#,
        &[(
            "shadow.html",
            "<body style='margin:0;background:white'><div style='position:absolute;left:50px;top:50px;width:100px;height:100px;\
             background:white;border-top-right-radius:90px;box-shadow:0 0 20px black'></div>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn videos_without_frames_take_the_ratio_of_their_attributes_like_chrome() {
    run_hidden(
        r#"
        await load('video.html');
        // The attributes are an `aspect-ratio: auto 20 / 10` hint, which author styles replace.
        const heights = JSON.parse(await run(`JSON.stringify([...document.querySelectorAll('video')].map(v => v.getBoundingClientRect().height))`));
        if (heights[0] !== 10 || heights[1] !== 100) fail('heights: ' + heights);
        "#,
        &[(
            "video.html",
            "<body style='margin:0'><video width=20 height=10 style='height:auto'></video>\
             <video width=20 height=10 style='width:100px;height:auto;aspect-ratio:auto 1/1'></video>",
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
        // none, right 5px bottom 0px: the 2x2 image sits 5px from the right.
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
             <img src='data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAFElEQVR4nGP4z8DAwPCfCUQyMAAAFQcCATPcN/oAAAAASUVORK5CYII=' style='width:100px;height:40px;object-fit:none;object-position:right 5px bottom 0px'></div>\
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
fn selection_modify_moves_by_words_sentences_and_layout_lines_like_chrome() {
    run_hidden(
        &format!(
            r#"{INPUT}
        await load('modify.html');
        for (const [name, value] of [
            ['word', 'pa#0:5,pa#0:11,pa#0:12,pa#0:17,pa#0:20,pa#0:22'],
            ['sentence', 'pa#0:29,pa#0:51,pa#0:29,pa#0:13,pa#0:13 > pa#0:29'],
            ['inline', 'pb#0:3,pb#0:4,b#0:1,b#0:2,b#0:3,b#0:4,i#0:1,i#0:2,i#0:3'],
            ['inline words', 'pb#4:1,pb#2:1,b#0:4,pb#0:4,pb#0:0,pa#0:51,pa#0:50'],
            ['lines', 'pc#0:12,pc#0:22,pc#0:32,pe#0:6,pc#0:32,pc#0:22'],
            ['line edges', 'pc#0:19,pc#0:19,pc#0:10,pc#0:10,pc#0:19'],
            ['line extend', 'pc#0:7 > pc#0:17,pc#0:7 > pc#0:27,pc#0:7 > pc#0:17,pc#0:7,pc#0:7 > i#0:2'],
            ['editable blocks', 'div#0:9,div#0:10,div#0:0,div#0:1,div#0:2'],
            ['editable lines', 'div#0:2,div:0,div#0:2,div#0:6,div:0,div#0:2'],
            ['editable edges', 'div#2:5,div#0:0,div#2:5,div#0:0,div#0:6'],
            ['editable wraps', 'pf#2:5,pf#2:15,pf#2:18,pf#2:5,pf#2:5 > pf#2:14'],
            ['spaces', 'pe#0:3,pe#0:4,pe#0:5,pe#0:6,pe#0:7,pe#0:10'],
        ]) await expect(name, `results[${{JSON.stringify(name)}}]`, value);
        "#
        ),
        &[(
            "modify.html",
            "<style>body { margin: 8px; font: 14px/1.3 Helvetica; } p, div { margin: 0 0 4px; } .w { width: 116px; font: 16px/20px Menlo; }</style>\
             <p id=pa>Hello world. This is a test! Another sentence here.</p><p id=pb>one <b>two <i>three</i></b> four<span>five</span> six</p>\
             <div id=pc class=w>aaaa bbbb cccc dddd eeee ffff gggg</div>\
             <div id=pd contenteditable><div>first line</div><div>second <b>bold</b> line</div><div><br></div><div>fourth</div></div>\
             <p id=pe>  lots   of   spaces  </p><div id=pf class=w contenteditable>xx <b>yy zz</b> ww vvvv uuuu tttt</div>\
             <script>globalThis.results = {}; const s = getSelection(), $ = id => document.getElementById(id);\
             const at = (node, offset) => (node.nodeType === 3 ? (node.parentNode.id || node.parentNode.localName) + '#' + [...node.parentNode.childNodes].indexOf(node) : node.id || node.localName) + ':' + offset;\
             const t = (el, ...path) => path.reduce((node, i) => node.childNodes[i], el);\
             const run = (name, node, offset, calls, focus) => { focus?.focus(); s.collapse(node, offset); results[name] = calls.map(call => { s.modify(...call.split(' ')); return at(s.anchorNode, s.anchorOffset) + (s.isCollapsed ? '' : ' > ' + at(s.focusNode, s.focusOffset)); }).join(); };\
             const many = (call, count) => Array(count).fill(call);\
             requestAnimationFrame(() => {\
             run('word', t($('pa'), 0), 0, many('move forward word', 6));\
             run('sentence', t($('pa'), 0), 15, ['move forward sentence', 'move forward sentence', 'move backward sentence', 'move backward sentence', 'extend forward sentence']);\
             run('inline', t($('pb'), 0), 2, many('move forward character', 9));\
             run('inline words', t($('pb'), 4), 4, many('move backward word', 7));\
             run('lines', t($('pc'), 0), 2, ['move forward line', 'move forward line', 'move forward line', 'move forward line', 'move backward line', 'move backward line']);\
             run('line edges', t($('pc'), 0), 12, ['move forward lineboundary', 'move forward lineboundary', 'move backward lineboundary', 'move backward lineboundary', 'move right lineboundary']);\
             run('line extend', t($('pc'), 0), 7, ['extend forward line', 'extend forward line', 'extend backward line', 'extend backward line', 'extend backward line']);\
             run('editable blocks', t($('pd'), 0, 0), 8, many('move forward character', 5), $('pd'));\
             run('editable lines', t($('pd'), 0, 0), 3, ['move forward line', 'move forward line', 'move forward line', 'move forward line', 'move backward line', 'move backward line'], $('pd'));\
             run('editable edges', t($('pd'), 1, 1, 0), 2, ['move forward lineboundary', 'move backward lineboundary', 'move forward paragraphboundary', 'move backward documentboundary', 'move forward documentboundary'], $('pd'));\
             run('editable wraps', t($('pf'), 0), 1, ['move forward line', 'move forward line', 'move forward line', 'move backward line', 'extend forward lineboundary'], $('pf'));\
             run('spaces', t($('pe'), 0), 0, many('move forward character', 6)); });</script>",
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
fn pointer_capture_moves_the_hover_like_chrome() {
    run_hidden(
        &format!(
            r#"{INPUT}
        await load('a.html');
        await input({{ action: 'press', x: 50, y: 20 }});
        await input({{ action: 'pressed_move', x: 50, y: 80 }});
        await frames(200);
        await expect('capture pending', "[hovered(), log.splice(0).join()].join()", 'b,pointerover:a,mouseover:a,pointerover:b,mouseover:b,pointermove:b');
        await input({{ action: 'pressed_move', x: 50, y: 85 }});
        await expect('capture target hovered', "[hovered(), log.join()].join()", 'a,pointerover:a,gotpointercapture:a,mouseover:a,pointermove:a');
        "#
        ),
        &[(
            "a.html",
            "<body style='margin:0'><div id=a style='height:60px'>a</div><div id=b style='height:60px'>b</div>\
             <script>globalThis.$ = id => document.getElementById(id); globalThis.log = [];\
             globalThis.hovered = () => [...document.querySelectorAll('body > :hover')].map(e => e.id).join();\
             $('a').addEventListener('pointerdown', () => addEventListener('pointermove', e => $('a').setPointerCapture(e.pointerId), { once: true }));\
             for (const type of ['pointerover', 'gotpointercapture', 'mouseover', 'pointermove']) for (const id of ['a', 'b']) $(id).addEventListener(type, e => log.push(type + ':' + id));</script></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn hover_follows_a_transition_under_the_pointer_like_chrome() {
    run_hidden(
        &format!(
            r#"{INPUT}
        await load('a.html');
        await input({{ action: 'move', x: 300, y: 20 }});
        await expect('hover', "hovered()", 'm');
        await run("$('m').classList.add('away')");
        await frames(700);
        await expect('hover after a transition', "hovered()", 'a');
        "#
        ),
        &[(
            "a.html",
            "<body style='margin:0'><div id=a style='height:60px'>a</div><div id=m></div>\
             <style>#m{position:absolute;top:0;left:250px;width:100px;height:40px;transition:left 300ms}#m.away{left:500px}</style>\
             <script>globalThis.$ = id => document.getElementById(id);\
             globalThis.hovered = () => [...document.querySelectorAll('body > :hover')].map(e => e.id).join();</script></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn caret_past_a_line_end_takes_editable_text_of_that_line_like_chrome() {
    run_hidden(
        r#"
        await load('a.html');
        const got = await run("(r => { const p = document.caretPositionFromPoint(r.left + 190, r.top + 36); return p.offsetNode.textContent + ':' + p.offset; })(document.getElementById('ed').getBoundingClientRect())");
        if (got !== ' x:2') fail('caret after a line end: ' + got);
        const at = (id, x, y) => run(`(r => { const p = document.caretPositionFromPoint(r.left + ${x}, r.top + ${y}); return (p.offsetNode.parentNode.id || p.offsetNode.id) + ':' + p.offset; })(document.getElementById('${id}').getBoundingClientRect())`);
        const gap = await at('g', 10, 30);
        if (gap !== 'g2:0') fail('caret in the margin between blocks: ' + gap);
        const hang = await at('w', 59, 10);
        if (hang !== 'w:4') fail('caret after a wrapped line: ' + hang);
        "#,
        &[(
            "a.html",
            "<body style='margin:0;font:14px/24px Helvetica'><div id=ed contenteditable style='width:200px;height:80px'>Compare <span contenteditable=false>The Long Fog</span> with <span contenteditable=false>Thaw</span> x</div>\
             <div id=g><p id=g1 style='margin:0 0 30px'>one</p><p id=g2 style='margin:0'>two</p></div><div id=w style='width:60px'>aaaa bbbb</div></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn scrollers_take_script_and_tab_focus_like_chrome() {
    run_hidden(
        r#"
        await load('a.html');
        const focus = await run("[$('s').focus(), $('n').focus(), document.activeElement.id].join()");
        if (focus !== ',,s') fail('scroller focus: ' + focus);
        const tabs = await run("$('z').focus(), [1, 2, 3, 4].map(() => (__appsDispatch('keydown', [], { key: 'Tab', code: 'Tab' }), document.activeElement.id)).join()");
        if (tabs !== 's,i,y,z') fail('scroller Tab stops: ' + tabs);
        "#,
        &[(
            "a.html",
            "<body style='margin:0'><button id=z>z</button><div id=s style='overflow:auto;height:40px;width:100px'><div style='height:100px'>s</div></div>\
             <div id=n style='overflow:hidden;height:40px'><div style='height:100px'>n</div></div>\
             <div id=w style='overflow:auto;height:40px'><button id=i>i</button><div style='height:100px'></div></div><button id=y>y</button>\
             <script>globalThis.$ = id => document.getElementById(id);</script></body>",
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
fn style_queries_follow_custom_properties_like_chrome() {
    run_hidden(
        r#"
        await load('a.html');
        const read = () => run(`JSON.stringify([getComputedStyle(document.getElementById('l')).backgroundColor, getComputedStyle(document.getElementById('p'), '::before').color])`);
        const first = await read();
        if (first !== '["rgba(0, 0, 0, 0)","rgb(0, 128, 0)"]') fail('first: ' + first);
        await run(`document.getElementById('pl').style.setProperty('--late', 'on'); document.getElementById('p').style.setProperty('--p', '2')`);
        await win.webContents.capturePage(); await win.webContents.capturePage();
        const after = await read();
        if (after !== '["rgb(255, 0, 0)","rgb(0, 0, 0)"]') fail('after: ' + after);
        "#,
        &[(
            "a.html",
            "<style>@container style(--late: on) { #l { background: rgb(255, 0, 0) } } #p { --p: 1 } #p::before { content: 'x' } @container style(--p: 1) { #p::before { color: rgb(0, 128, 0) } }</style>\
             <body style='margin:0'><div id=pl><div id=l>l</div></div><div id=p>p</div></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn container_units_follow_the_container_axes_like_chrome() {
    run_hidden(
        r#"
        await load('a.html');
        // Block-axis units of an inline-size container use the viewport, which is 10vh here.
        const out = await run(`JSON.stringify(['u1', 'u2'].map(id => { const u = document.getElementById(id), s = getComputedStyle(u), r = u.getBoundingClientRect(); return [Math.round(r.width), Math.round(r.height), s.marginLeft, s.marginTop, s.paddingLeft, s.paddingRight].join(' '); }).concat(String(innerHeight / 10)))`);
        const [u1, u2, h] = JSON.parse(out), min = Math.min(30, h), max = Math.max(30, h);
        if (u1 !== [Math.round(30 + min + max), Math.round(h), '30px', h + 'px', min + 'px', max + 'px'].join(' ') || u2 !== '50 10 20px 10px 10px 20px') fail('container units: ' + out);
        "#,
        &[(
            "a.html",
            "<style>.u{width:10cqw;height:10cqh;margin-left:10cqi;margin-top:10cqb;padding-left:10cqmin;padding-right:10cqmax}</style>\
             <body style='margin:0'><div style='container-type:inline-size;width:300px;height:50px'><div class=u id=u1></div></div>\
             <div style='container-type:size;width:200px;height:100px'><div class=u id=u2></div></div></body>",
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
fn gradients_that_repeat_on_one_axis_stay_sharp_on_wide_boxes_like_chrome() {
    // One raster of these boxes would pass the raster limit and blur.
    let page = "<style>body{margin:0;background:white}div{background:white linear-gradient(black 0 8px,white 8px) 4px 4px / 6px 16px}</style>\
        <body><div style='width:20000px;height:20px;background-image:linear-gradient(to right,black 0 8px,white 8px);background-size:16px 6px;background-repeat:repeat-x'></div>\
        <div style='height:100px;overflow:hidden'><div style='width:20px;height:20000px;background-repeat:repeat-y'></div></div>\
        <div style='height:40px;overflow:hidden;background:white'><div style='width:20px;height:40000px;background-image:linear-gradient(black 0 1px,white 1px);background-size:16px 10px;background-position:2px 5px;background-repeat:repeat-x'></div></div></body>";
    // The root box is empty, and its background still repeats over the canvas.
    let root = "<html style='background:white linear-gradient(to right,red 0 3px,white 3px) 0 200px / 10px 20px repeat-x'><body style='margin:0'></body></html>";
    let text = run_hidden(
        &format!(
            r#"{STEPS}
            await load('page.html');
            const pixels = [];
            for (const [x, y] of [[4, 6], [11, 6], [12, 6], [20, 6], [11, 2], [11, 12], [6, 24], [6, 31], [6, 32], [6, 40], [2, 31], [12, 31]]) pixels.push(await pixel(x, y));
            console.log('PIXELS ' + pixels.join(' '));
            console.log('TALL ' + [await pixel(4, 124), await pixel(4, 125), await pixel(4, 126), await pixel(4, 135)].join(' '));
            await load('root.html');
            console.log('ROOT ' + [await pixel(1, 210), await pixel(5, 210), await pixel(11, 210), await pixel(1, 190)].join(' '));
            "#
        ),
        &[("page.html", page), ("root.html", root)],
    );
    // Tiles repeat only on their axis, and their edges are sharp.
    assert!(
        text.contains(
            "PIXELS 0,0,0 0,0,0 255,255,255 0,0,0 255,255,255 255,255,255 0,0,0 0,0,0 255,255,255 0,0,0 255,255,255 255,255,255"
        ),
        "{text}"
    );
    // A box taller than a raster keeps a sharp tile on the axis that doesn't repeat.
    assert!(
        text.contains("TALL 255,255,255 0,0,0 255,255,255 255,255,255"),
        "{text}"
    );
    assert!(
        text.contains("ROOT 0,0,255 255,255,255 0,0,255 255,255,255"),
        "{text}"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn inline_svgs_with_visible_overflow_paint_outside_their_box_like_chrome() {
    let page = "<body style='margin:0;background:white'><style>svg{position:absolute;top:50px;width:10px;height:10px}</style>\
        <svg style='left:50px;overflow:visible'><rect x=-20 y=-20 width=40 height=40 fill=red /><rect x=100 y=60 width=20 height=20 fill=blue /></svg>\
        <svg style='left:300px'><rect x=-20 y=-20 width=40 height=40 fill=red /></svg></body>";
    // A long spill from a box far below the view keeps its device scale.
    let far = "<body style='margin:0;background:white;height:7000px'><div style='position:absolute;top:6000px;left:10px'>\
        <svg width=10 height=10 style='overflow:visible;display:block'><rect y=-5990 width=21 height=90 fill=red /></svg></div>\
        <div style='position:absolute;top:1500px;left:100px'><div><svg width=10 height=10 style='overflow:visible;display:block'><rect y=-1490 width=20 height=1490 fill=red /></svg></div></div>\
        <div style='position:absolute;top:1500px;left:150px'><div><svg width=10 height=10 style='overflow:visible'><rect y=-1490 width=20 height=1490 fill=red /></svg></div></div>\
        <div style='position:absolute;top:0;left:200px;width:50px'><div style='height:1500px'></div><div><div>\
        <svg width=10 height=10 style='overflow:visible;display:block'><rect y=-1490 width=20 height=1490 fill=red /></svg></div></div></div></body>";
    let text = run_hidden(
        &format!(
            r#"{STEPS}
            await load('page.html');
            console.log('PIXELS ' + [await pixel(35, 35), await pixel(55, 55), await pixel(160, 120), await pixel(285, 55), await pixel(305, 55)].join(' '));
            await load('far.html');
            console.log('FAR ' + [await pixel(15, 50), await pixel(30, 99), await pixel(31, 50), await pixel(15, 100)].join(' '));
            console.log('AROUND ' + [await pixel(105, 50), await pixel(155, 50), await pixel(205, 50)].join(' '));
            "#
        ),
        &[("page.html", page), ("far.html", far)],
    );
    // An `<svg>` clips to its box by default, as in Chrome.
    assert!(
        text.contains("PIXELS 0,0,255 0,0,255 255,0,0 255,255,255 0,0,255"),
        "{text}"
    );
    assert!(
        text.contains("FAR 0,0,255 0,0,255 255,255,255 255,255,255"),
        "{text}"
    );
    // The blocks around such an `<svg>` don't become spacers either.
    assert!(text.contains("AROUND 0,0,255 0,0,255 0,0,255"), "{text}");
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
fn buttons_center_their_content_in_a_set_height() {
    let text = run_hidden(
        r#"
        await load('t.html');
        console.log('TOPS ' + await run("JSON.stringify([...document.querySelectorAll('button')].map(button => button.firstElementChild.getBoundingClientRect().y - button.getBoundingClientRect().y))"));
        "#,
        &[(
            "t.html",
            "<body style='margin:0'><style>button{height:44px;padding:0;border:0;font:10px/12px Helvetica}\
             i,b{display:block;height:12px}u{display:inline-block;width:10px;height:12px;vertical-align:top}</style>\
             <button><i></i></button><button style='display:block'><u></u></button>\
             <button style='display:block'><i></i><b></b></button><button style='display:flex'><i></i></button>\
             <button style='display:grid'><i></i></button><button style='display:block;height:auto'><i></i></button></body>",
        )],
    );
    let tops = text
        .lines()
        .find_map(|line| line.strip_prefix("TOPS "))
        .unwrap_or_else(|| panic!("{text}"));
    // Chrome's values: a flex or grid button aligns its content like other flex and grid boxes.
    assert_eq!(tops, "[16,16,10,0,0,0]");
}

#[test]
#[ignore = "requires the macOS window server"]
fn flex_items_behind_contents_boxes_size_like_chrome() {
    let text = run_hidden(
        r#"
        await load('t.html');
        const boxes = "[...document.querySelectorAll('[id]')].map(e => { const r = e.getBoundingClientRect(); return e.id + ':' + r.x + '+' + r.width }).join(' ')";
        const out = [await run(boxes)];
        for (const change of ["w.insertAdjacentHTML('beforeend', '<div id=e class=g><i></i></div>')", "w.style.display = 'block'", "w.style.display = ''"]) {
            await run("{ const w = document.getElementById('w'); " + change + " }");
            await win.webContents.capturePage();
            out.push(await run(boxes));
        }
        console.log('BOXES ' + out.join(' | '));
        "#,
        &[(
            "t.html",
            "<body style='margin:0'><style>.c{display:contents}.g{display:grid}.g>i{display:block;width:72px;height:10px}.f{display:flex}</style>\
             <div class=f><div class=c><div class=c><div id=a><div class=g><i></i></div></div></div></div><div id=b style='width:50px;height:10px'></div></div>\
             <div class=f><div class=c id=w><div id=c class=g><i></i></div></div><div id=d style='width:50px;height:10px'></div></div></body>",
        )],
    );
    let boxes = text
        .lines()
        .find_map(|line| line.strip_prefix("BOXES "))
        .unwrap_or_else(|| panic!("{text}"));
    // Chrome's values: a flex item sizes by its content whether or not a contents box wraps it.
    assert_eq!(
        boxes,
        "a:0+72 b:72+50 w:0+0 c:0+72 d:72+50 | a:0+72 b:72+50 w:0+0 c:0+72 e:72+72 d:144+50 | \
         a:0+72 b:72+50 w:0+72 c:0+72 e:0+72 d:72+50 | a:0+72 b:72+50 w:0+0 c:0+72 e:72+72 d:144+50"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn grids_with_content_sized_heights_fit_their_rows_like_chrome() {
    let text = run_hidden(
        r#"
        await load('t.html');
        const boxes = "[...document.querySelectorAll('[id]')].map(e => { const r = e.getBoundingClientRect(); return e.id + ':' + [r.y, r.width, r.height].join(',') }).join(' ')";
        const out = [await run(boxes)];
        await run("document.getElementById('a').append(document.createElement('i'))");
        await win.webContents.capturePage();
        out.push(await run(boxes));
        console.log('BOXES ' + out.join(' | '));
        "#,
        &[(
            "t.html",
            "<body style='margin:0'><style>.g{display:grid}i{display:block;width:40px;height:30px}</style>\
             <div id=a class=g style='height:fit-content'><i></i></div>\
             <div id=b class=g style='height:max-content;grid-template-columns:auto auto;gap:5px 7px;padding:3px 4px'><i></i><i></i><i></i></div>\
             <div id=c class=g style='height:min-content;width:max-content;gap:4px'><i></i><i></i></div>\
             <div id=d class=g style='height:fit-content;width:fit-content;padding:2px'><i></i></div>\
             <div id=e class=g style='height:max-content;width:min-content;grid-template-columns:auto auto;gap:3px'><i></i><i></i></div></body>",
        )],
    );
    let boxes = text
        .lines()
        .find_map(|line| line.strip_prefix("BOXES "))
        .unwrap_or_else(|| panic!("{text}"));
    // Chrome's values: these heights size a grid like `auto`.
    assert_eq!(
        boxes,
        "a:0,400,30 b:30,400,71 c:101,40,64 d:165,44,34 e:199,83,30 | \
         a:0,400,60 b:60,400,71 c:131,40,64 d:195,44,34 e:229,83,30"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn positioned_grids_and_popovers_size_like_chrome() {
    let text = run_hidden(
        r#"
        await load('t.html');
        const boxes = "[...document.querySelectorAll('[id]')].map(e => { const r = e.getBoundingClientRect(); return e.id + ':' + [r.x, r.y, r.width, r.height].join(',') }).join(' ')";
        console.log('BOXES ' + await run(boxes));
        await run("document.getElementById('p').remove()");
        const bytes = (await win.webContents.capturePage({ x: 20, y: 180, width: 1, height: 1 })).toBitmap();
        console.log('REMOVED ' + [bytes[2], bytes[1], bytes[0]].join(','));
        "#,
        &[(
            "t.html",
            "<body style='margin:0'><style>.g{display:grid;position:absolute}i{display:block;width:40px;height:30px}\
             [popover]{margin:0;padding:0;border:0;inset:10px auto auto 8px;background:red}</style>\
             <div style='position:relative;width:300px;height:200px'>\
             <div id=a class=g style='left:10px;top:5px'><i></i><i></i></div>\
             <div id=b class=g style='left:20px;right:30px;top:5px'><i></i></div>\
             <div id=c class=g style='right:10px;bottom:10px'><i></i></div>\
             <div id=d class=g style='left:20px;right:30px;top:5px;bottom:15px;width:fit-content;height:fit-content'><i></i></div>\
             <div id=e class=g style='left:50px;top:20px;bottom:30px'><i></i></div>\
             <div id=f class=g style='left:60px;top:20px;bottom:30px;height:max-content'><i></i></div></div>\
             <div id=p popover style='display:grid;height:fit-content;top:150px'><i></i><i></i></div>\
             <div id=q popover style='display:flex'><i></i><i></i></div>\
             <div id=r popover style='display:block'><i></i><i></i></div>\
             <script>for (const id of 'pqr') document.getElementById(id).showPopover()</script></body>",
        )],
    );
    let line = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name))
            .unwrap_or_else(|| panic!("{text}"))
    };
    // Chrome's values: a fit-content grid shrinks to its items, and insets take room from a stretched grid.
    assert_eq!(
        line("BOXES "),
        "a:10,5,40,60 b:20,5,250,30 c:250,160,40,30 d:20,5,40,30 e:50,20,40,150 f:60,20,40,30 \
         p:8,150,40,60 q:8,10,80,30 r:8,10,40,60"
    );
    assert_eq!(line("REMOVED "), "255,255,255");
}

#[test]
#[ignore = "requires the macOS window server"]
fn replaced_content_paints_above_gradient_backgrounds_like_chrome() {
    let text = run_hidden(
        r#"
        await load('t.html');
        await run('redraw()');
        await win.webContents.capturePage();
        const out = [];
        for (const [x, y] of [[20, 20], [50, 5], [20, 60], [50, 75], [28, 108], [5, 92], [1, 100], [20, 156], [50, 140], [20, 196], [50, 180]]) {
            const bytes = (await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap();
            out.push([bytes[2], bytes[1], bytes[0]].map(value => value > 200 ? 'f' : value < 60 ? '0' : '?').join(''));
        }
        console.log('COLORS ' + out.join(' '));
        "#,
        &[(
            "t.html",
            "<body style='margin:0;background:#fff'><style>canvas,svg{display:block;width:60px;height:40px}</style>\
             <canvas id=a width=60 height=40 style='background:linear-gradient(#00f,#00f),radial-gradient(#0f0,#0f0)'></canvas>\
             <canvas id=b width=60 height=40 style='background:linear-gradient(90deg,#00f,#00f) 0 0/20px 20px,#0f0'></canvas>\
             <canvas id=c width=60 height=40 style='padding:5px;border:3px solid #000;background:linear-gradient(#00f,#00f)'></canvas>\
             <svg style='background:linear-gradient(#00f,#00f)'><rect x=10 y=10 width=20 height=20 fill=red /></svg>\
             <canvas id=d width=60 height=40 style='background:conic-gradient(#00f 0 100%)'></canvas>\
             <script>const draw = (id, color) => { const ctx = document.getElementById(id).getContext('2d'); ctx.fillStyle = color; ctx.fillRect(10, 10, 20, 20) };\
             for (const id of 'abcd') draw(id, '#f00'); globalThis.redraw = () => { draw('a', '#ff0'); draw('c', '#ff0') }</script></body>",
        )],
    );
    let colors = text
        .lines()
        .find_map(|line| line.strip_prefix("COLORS "))
        .unwrap_or_else(|| panic!("{text}"));
    // Chrome's colors: canvas and SVG pixels cover the background layers, also after a redraw.
    assert_eq!(colors, "ff0 00f f00 00f ff0 00f 000 f00 00f f00 00f");
}

#[test]
#[ignore = "requires the macOS window server"]
fn svg_grid_items_size_by_their_view_box_like_chrome() {
    let text = run_hidden(
        r#"
        await load('t.html');
        console.log('SIZES ' + await run("[...'abcde'].map(id => { const r = document.getElementById(id).getBoundingClientRect(); return r.width + 'x' + r.height }).join(' ')"));
        "#,
        &[(
            "t.html",
            "<body style='margin:0'><style>.g{display:grid;grid-template-columns:auto 1fr}</style>\
             <div class=g><svg id=a viewBox='-50 -50 100 100' style='width:128px;height:auto'><circle r='40'/></svg><div>x</div></div>\
             <div class=g><svg id=b viewBox='0 0 100 50' style='width:60px'></svg></div>\
             <div class=g><svg id=c viewBox='0 0 100 50' width=40></svg></div>\
             <div class=g><svg id=d viewBox='0 0 100 50' style='height:10px'></svg></div>\
             <div class=g><svg id=e width=30 height=20></svg></div></body>",
        )],
    );
    let sizes = text
        .lines()
        .find_map(|line| line.strip_prefix("SIZES "))
        .unwrap_or_else(|| panic!("{text}"));
    // Chrome's values: the CSS size and the view box ratio give the other side.
    assert_eq!(sizes, "128x128 60x30 40x20 20x10 30x20");
}

#[test]
#[ignore = "requires the macOS window server"]
fn offsets_leave_out_transforms_like_chrome() {
    let text = run_hidden(
        r#"
        await load('t.html');
        console.log('OFFSETS ' + await run("[...'abcde'].map(id => { const e = document.getElementById(id); return [e.offsetLeft, e.offsetTop] }).join(' ') + ' ' + document.getElementById('f').scrollWidth"));
        "#,
        &[(
            "t.html",
            "<body style='margin:0;font:10px/12px Helvetica'><div style='position:relative;padding:3px'>\
             <b id=a style='display:block;translate:24px 0'>x</b><b id=b style='display:block;transform:translate(10px,7px)'>x</b>\
             <b id=c style='display:block;width:20px;scale:2;translate:0 5px'>x</b>\
             <b id=d style='display:block;width:20px;rotate:180deg;translate:4px 4px'>x</b>\
             <div style='display:flex;translate:5px 6px'><b id=e style='margin-left:7px'>x</b></div>\
             <div id=f style='width:100px;translate:3px 0'><i style='display:block;width:500px;height:1px'></i></div></div></body>",
        )],
    );
    let offsets = text
        .lines()
        .find_map(|line| line.strip_prefix("OFFSETS "))
        .unwrap_or_else(|| panic!("{text}"));
    // Chrome's values: offsets are layout positions, and a translated box still reaches its children.
    assert_eq!(offsets, "3,3 3,15 3,27 3,39 7,0 500");
}

#[test]
#[ignore = "requires the macOS window server"]
fn positioned_boxes_fill_aspect_ratio_and_padding_boxes_like_chrome() {
    let text = run_hidden(
        r#"
        await load('t.html');
        console.log('SIZES ' + await run("[...'abcdefg'].map(id => { const r = document.getElementById(id).getBoundingClientRect(); return r.width + 'x' + r.height }).join(' ')"));
        "#,
        &[(
            "t.html",
            "<body style='margin:0'><style>.r{position:relative}.a{position:absolute;inset:0;display:grid}.a>i{display:block;height:22px}</style>\
             <div class=r style='width:100px;aspect-ratio:1/1'><div id=a class=a><i></i></div></div>\
             <div class=r style='width:200px;aspect-ratio:2;padding:5px;border:3px solid;box-sizing:border-box'><div id=b class=a style='bottom:10px'><i></i></div></div>\
             <div class=r style='width:100px;aspect-ratio:1/2;max-height:150px'><div id=c class=a></div></div>\
             <div class=r style='width:100px;aspect-ratio:1/1'><div id=d style='height:50%'></div></div>\
             <div class=r style='width:200px;height:60px;padding:10px 20px'><div id=e style='position:absolute;width:50%;height:50%'></div><div id=f class=a></div></div>\
             <div style='width:152px'><div class=r style='aspect-ratio:1/1'><div id=g class=a style='box-sizing:border-box;padding:16px'><b style='display:none'></b></div></div></div></body>",
        )],
    );
    let sizes = text
        .lines()
        .find_map(|line| line.strip_prefix("SIZES "))
        .unwrap_or_else(|| panic!("{text}"));
    // Chrome's values: an aspect ratio gives a height, positioned children fill the padding box, and a grid
    // without rows keeps the height of its insets.
    assert_eq!(sizes, "100x100 194x84 100x150 100x50 120x40 240x80 152x152");
}

#[test]
#[ignore = "requires the macOS window server"]
fn conic_gradients_take_math_angles_like_chrome() {
    let text = run_hidden(
        r#"
        await load('t.html');
        const out = [];
        for (const box of [0, 100, 200]) for (const [x, y] of [[75, 25], [25, 25], [25, 75], [75, 75]]) {
            const bytes = (await win.webContents.capturePage({ x: box + x, y, width: 1, height: 1 })).toBitmap();
            out.push([bytes[2], bytes[1], bytes[0]].map(value => value > 200 ? 'f' : value < 60 ? '0' : '?').join(''));
        }
        console.log('COLORS ' + out.join(' '));
        "#,
        &[(
            "t.html",
            "<body style='margin:0;background:#fff'><style>div>i{display:block;width:100px;height:100px}</style><div style='display:flex'>\
             <i style='background:conic-gradient(red calc(0.25 * 360deg), transparent 0)'></i>\
             <i style='background:conic-gradient(red calc(0.25 * 360deg - 0.4deg), transparent calc(0.25 * 360deg + 0.4deg))'></i>\
             <i style='--share:0.25;background:conic-gradient(red calc(var(--share) * 360deg), transparent 0)'></i></div></body>",
        )],
    );
    let colors = text
        .lines()
        .find_map(|line| line.strip_prefix("COLORS "))
        .unwrap_or_else(|| panic!("{text}"));
    // Chrome's colors: red fills the first quarter of the turn only.
    assert_eq!(colors, ["f00 fff fff fff"; 3].join(" "));
}

#[test]
#[ignore = "requires the macOS window server"]
fn conic_gradients_keep_thin_stops_and_masks_composite_like_chrome() {
    let text = run_hidden(
        r#"
        await load('t.html');
        const out = [];
        for (const box of [0, 100, 200, 300]) for (const [x, y] of [[80, 20], [20, 20], [58, 42], [42, 58]]) {
            const bytes = (await win.webContents.capturePage({ x: box + x, y, width: 1, height: 1 })).toBitmap();
            out.push([bytes[2], bytes[1], bytes[0]].map(value => value > 200 ? 'f' : value < 60 ? '0' : '?').join(''));
        }
        const ticks = (await win.webContents.capturePage({ x: 0, y: 100, width: 100, height: 100 })).toBitmap();
        let red = 0;
        for (let at = 0; at < ticks.length; at += 4) if (ticks[at + 2] > 200 && ticks[at + 1] < 100) red++;
        console.log('COLORS ' + out.join(' ') + ' TICKS ' + red / (ticks.length / 4));
        "#,
        &[(
            "t.html",
            "<body style='margin:0;background:#fff'><style>div>i{display:block;width:100px;height:100px;background:red;\
             mask:radial-gradient(closest-side, transparent 60%, #000 60%), conic-gradient(#000 90deg, transparent 0)}</style>\
             <div style='display:flex'><i style='mask-composite:intersect'></i><i style='mask-composite:subtract'></i>\
             <i style='mask-composite:exclude'></i><i></i></div>\
             <b style='display:block;width:100px;height:100px;background:repeating-conic-gradient(from -0.15deg, red 0deg 0.3deg, transparent 0.3deg 6deg)'></b></body>",
        )],
    );
    let line = text
        .lines()
        .find_map(|line| line.strip_prefix("COLORS "))
        .unwrap_or_else(|| panic!("{text}"));
    let (colors, ticks) = line.split_once(" TICKS ").unwrap();
    // Chrome's colors: a point in the ring and the quarter, in the ring only, in the quarter only, and in neither.
    assert_eq!(
        colors,
        "f00 fff fff fff fff f00 fff fff fff f00 f00 fff f00 f00 f00 fff"
    );
    // Lines of 0.3deg every 6deg cover 5% of the turn.
    let ticks: f32 = ticks.parse().unwrap();
    assert!((0.025..0.08).contains(&ticks), "{ticks}");
}

#[test]
#[ignore = "requires the macOS window server"]
fn animated_custom_properties_restyle_slotted_children() {
    let text = run_hidden(
        r#"
        await load('t.html');
        for (let n = 0; n < 4; n++) await win.webContents.capturePage();
        console.log('WIDTH ' + await run("document.getElementById('a').getBoundingClientRect().width"));
        "#,
        &[(
            "t.html",
            "<body style='margin:0'><style>@property --w{syntax:'<length>';inherits:false;initial-value:0px}\
             @keyframes grow{from{--w:10px}to{--w:90px}}#a{display:block;height:20px;width:var(--w);animation:grow 1s linear infinite}</style>\
             <div id=h><span id=a></span></div>\
             <script>document.getElementById('h').attachShadow({mode:'open'}).innerHTML='<p><slot></slot></p>'</script></body>",
        )],
    );
    let width: f32 = text
        .lines()
        .find_map(|line| line.strip_prefix("WIDTH "))
        .and_then(|width| width.trim().parse().ok())
        .unwrap_or_else(|| panic!("{text}"));
    assert!((10. ..=90.).contains(&width), "{text}");
}

#[test]
#[ignore = "requires the macOS window server"]
fn pseudo_elements_are_grid_and_flex_items_like_chrome() {
    let text = run_hidden(
        r#"
        await load('t.html');
        const colors = async points => {
            const out = [];
            for (const [x, y] of points) {
                const bytes = (await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap();
                out.push([bytes[2], bytes[1], bytes[0]].map(value => value > 200 ? 'f' : value < 60 ? '0' : '?').join(''));
            }
            return out.join(' ');
        };
        const before = await colors([[20, 20], [70, 10], [70, 30], [120, 10], [120, 35], [155, 20], [185, 20], [205, 5], [235, 5], [235, 35], [205, 35], [15, 55], [15, 75], [105, 70], [115, 70], [125, 70], [135, 70], [155, 70]]);
        const toggled = [];
        for (const change of ["remove('n')", "add('n')"]) {
            await run("document.getElementById('t').classList." + change);
            await win.webContents.capturePage();
            toggled.push(await colors([[70, 55], [70, 85]]));
        }
        console.log('COLORS ' + before + ' | ' + toggled.join(' | '));
        "#,
        &[(
            "t.html",
            "<body style='margin:0;background:#fff;font:10px/12px Helvetica'><style>.g{display:grid;width:40px;height:40px;float:left;margin:0 10px 10px 0;background:#eee}\
             .g::before{content:'';background:#f00}.g>i,.f>i{display:block;background:#00f}.h{grid-template-rows:1fr 1fr}\
             .d::before{grid-row:1/3;grid-column:1}.d>i{grid-row:1;grid-column:1;margin:5px;height:10px;z-index:1;background:#ff0}.d>b{grid-row:2;grid-column:1;display:block;height:10px}\
             .c{grid-template-columns:1fr 1fr}.o::before{order:1}.e::before{grid-column:2;grid-row:2}.e::after{content:'';background:#0c0;grid-area:1/1}.n::before{content:none}\
             .ig{display:inline-grid;float:left;margin:0 20px 10px 0}.ig::before{content:'';width:30px;height:20px;background:#f00}.ig>i{display:block;width:30px;height:10px;background:#00f}\
             .f{display:flex;width:40px;height:40px;float:left;margin:0 10px 10px 0;background:#eee}.f::before{content:'';background:#f00;width:10px;order:1}\
             .f::after{content:'';background:#0c0;width:10px;order:-1}.f>i{width:10px}</style>\
             <div class=g></div><div class='g h'><i></i></div><div class='g d'><i></i><b></b></div><div class='g c o'><i></i></div><div class='g c h e'><i></i></div>\
             <div style='clear:both'></div><span class=ig><i></i></span><div id=t class='g h n'><i></i></div><div class=f><i></i></div><div class='g c'>x</div></body>",
        )],
    );
    let colors = text
        .lines()
        .find_map(|line| line.strip_prefix("COLORS "))
        .unwrap_or_else(|| panic!("{text}"));
    // Chrome's colors: `::before` and `::after` are the first and last items, and take the grid and flex
    // properties. A grid with only a pseudo-element fills its cell with it.
    assert_eq!(
        colors,
        "f00 f00 00f ff0 f00 00f f00 0f0 00f f00 fff f00 00f 0f0 00f f00 fff f00 | f00 00f | 00f fff"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn grids_stretch_in_flex_rows_like_chrome() {
    let text = run_hidden(
        r#"
        await load('t.html');
        console.log('BOXES ' + await run("[...'abcde'].map(id => { const r = document.getElementById(id).getBoundingClientRect(); return r.y + '+' + r.height }).join(' ')"));
        "#,
        &[(
            "t.html",
            "<body style='margin:0'><style>i{display:block;height:10px}</style>\
             <div style='display:flex;height:100px'><div id=a style='display:grid'><i></i></div>\
             <div id=b style='display:inline-grid;grid-template-rows:1fr 1fr'><i></i><i id=c></i></div>\
             <div id=d style='display:grid;align-self:center'><i></i></div></div>\
             <div style='display:flex'><div id=e style='display:grid'><i></i></div><div style='height:40px;width:5px'></div></div></body>",
        )],
    );
    let boxes = text
        .lines()
        .find_map(|line| line.strip_prefix("BOXES "))
        .unwrap_or_else(|| panic!("{text}"));
    // Chrome's values: `align-items: normal` stretches a grid to its line, and its rows fill it.
    assert_eq!(boxes, "0+100 0+100 50+10 45+10 100+40");
}

#[test]
#[ignore = "requires the macOS window server"]
fn grids_take_their_height_from_aspect_ratio_like_chrome() {
    let text = run_hidden(
        r#"
        await load('t.html');
        console.log('BOXES ' + await run("[...'abcde'].map(id => { const r = document.getElementById(id).getBoundingClientRect(); return r.y + ':' + r.width + 'x' + r.height }).join(' ')"));
        "#,
        &[(
            "t.html",
            "<body style='margin:0'><style>i{display:block;height:10px}</style><div style='width:300px'>\
             <div id=a style='display:grid;aspect-ratio:2.5/1'><i></i></div><div id=b style='display:grid;aspect-ratio:2/1;padding:10px'><i></i></div>\
             <div id=c style='display:grid;aspect-ratio:3/1;grid-template-rows:1fr 1fr'><i></i><i id=d></i></div>\
             <div id=e style='display:grid;aspect-ratio:4/1;width:100px'><i style='height:40px'></i></div></div></body>",
        )],
    );
    let boxes = text
        .lines()
        .find_map(|line| line.strip_prefix("BOXES "))
        .unwrap_or_else(|| panic!("{text}"));
    // Chrome's values: the width gives the height of the content box, and taller rows make the box taller.
    assert_eq!(
        boxes,
        "0:300x120 120:300x160 280:300x100 330:300x10 380:100x40"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn pseudo_element_text_transforms_and_fits_its_content_like_chrome() {
    let text = run_hidden(
        r#"
        await load('t.html');
        const widths = await run("[...'abcdef'].map(id => Math.round(document.getElementById(id).getBoundingClientRect().width)).join(' ')");
        const out = [];
        for (const x of [75, 80, 88, 93]) {
            const bytes = (await win.webContents.capturePage({ x, y: 145, width: 1, height: 1 })).toBitmap();
            out.push([bytes[2], bytes[1], bytes[0]].map(value => value > 200 ? 'f' : value < 60 ? '0' : '?').join(''));
        }
        console.log('RESULT ' + widths + ' | ' + out.join(' '));
        "#,
        &[(
            "t.html",
            "<body style='margin:0;background:#fff'><style>.f{float:left;clear:left;font:20px Helvetica}.u::before{content:'off';text-transform:uppercase}\
             .l::before{content:'OFF';text-transform:lowercase}.c::before{content:'ab cd';text-transform:capitalize}\
             .s{position:relative;clear:left;margin-left:100px;width:50px;height:20px;background:#ccc}\
             .s::before{content:'XX';position:absolute;top:0;right:calc(100% + 10px);font:10px monospace;color:transparent;background:#f00}</style>\
             <div id=a class='f u'></div><div id=b class='f l'></div><div id=c class='f c'></div>\
             <div id=d class='f'>OFF</div><div id=e class='f'>off</div><div id=f class='f'>Ab Cd</div><div class=s></div></body>",
        )],
    );
    let result = text
        .lines()
        .find_map(|line| line.strip_prefix("RESULT "))
        .unwrap_or_else(|| panic!("{text}"));
    // Chrome's values: the content takes `text-transform`, and a box outside the left edge keeps its text.
    assert_eq!(result, "40 22 56 40 22 56 | fff f00 f00 fff");
}

#[test]
#[ignore = "requires the macOS window server"]
fn masks_place_and_repeat_image_layers_like_chrome() {
    let text = run_hidden(
        r#"
        await load('t.html');
        const out = [];
        for (const [x, y] of [[15, 5], [100, 5], [185, 5], [15, 25], [100, 25], [185, 25], [15, 45], [28, 45], [40, 45], [50, 45], [100, 45]]) {
            const bytes = (await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap();
            out.push([bytes[2], bytes[1], bytes[0]].map(value => value > 200 ? 'f' : value < 60 ? '0' : '?').join(''));
        }
        console.log('COLORS ' + out.join(' '));
        "#,
        &[(
            "t.html",
            "<body style='margin:0;background:#fff'><style>.m{position:relative;width:200px;height:10px;margin-bottom:10px}\
             .b,.p::before{position:absolute;inset:2px 8px;background:#f00;\
             mask:url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 22 6'%3E%3Cpath d='M0 0H22V6H0Z'/%3E%3C/svg%3E\") left center / 22px 6px no-repeat,\
             url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 22 6'%3E%3Cpath d='M0 0H22V6H0Z'/%3E%3C/svg%3E\") right center / 22px 6px no-repeat}\
             .p::before{content:''}.r{position:absolute;inset:2px 8px;background:#00f;\
             mask:url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 22 6'%3E%3Cpath d='M0 0H11V6H0Z'/%3E%3C/svg%3E\") 4px 0 / 22px 6px repeat-x}</style>\
             <div class=m><div class=b></div></div><div class='m p'></div><div class=m><div class=r></div></div></body>",
        )],
    );
    let colors = text
        .lines()
        .find_map(|line| line.strip_prefix("COLORS "))
        .unwrap_or_else(|| panic!("{text}"));
    // Chrome's colors: each image layer has its size, place and repeat, also on a pseudo-element.
    assert_eq!(colors, "f00 fff f00 f00 fff f00 00f fff 00f fff 00f");
}

#[test]
#[ignore = "requires the macOS window server"]
fn bitmap_masks_draw_again_at_new_sizes() {
    run_hidden(
        r#"
        await load('page.html');
        const look = async (...xs) => {
            for (let n = 0; n < 3; n++) await win.webContents.capturePage();
            const out = [];
            for (const x of xs) {
                const bytes = (await win.webContents.capturePage({ x, y: 5, width: 1, height: 1 })).toBitmap();
                out.push([bytes[2], bytes[1], bytes[0]].map(value => value > 200 ? 'f' : value < 60 ? '0' : '?').join(''));
            }
            return out.join(' ');
        };
        let first = '';
        for (let n = 0; n < 40 && first !== 'f00 fff'; n++) first = await look(3, 37);
        if (first !== 'f00 fff') fail('first frame: ' + first);
        await run("document.getElementById('m').style.width = '80px'");
        const wide = await look(5, 15, 75);
        if (wide !== 'f00 f00 fff') fail('after resize: ' + wide);
        await run("document.getElementById('m').style.maskImage = 'none'");
        const plain = await look(5, 75);
        if (plain !== 'f00 f00') fail('without mask: ' + plain);
        "#,
        &[(
            "page.html",
            "<body style='margin:0;background:#fff'><div id=m style='width:40px;height:10px;background:#f00;\
             mask:url(data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAYAAAD0In+KAAAAD0lEQVR4nGNgYGD4D8QMAAUEAQCwBUiSAAAAAElFTkSuQmCC) 0 0 / 100% 100% no-repeat'></div></body>",
        )],
    );
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

#[test]
#[ignore = "requires the macOS window server"]
fn terminals_draw_their_grid_and_take_input() {
    run_hidden(
        r#"
        await load('term.html');
        const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
        const input = body => fetch(MCP + '/input', { method: 'POST', body: JSON.stringify({ window: win.id, ...body }) }).then(response => response.text());
        // Counts the pixels of a row of cells by color: [red, green, blue, dark].
        const colors = async row => {
            const bytes = (await win.webContents.capturePage({ x: 10, y: 10 + row * 20, width: 200, height: 20 })).toBitmap();
            const count = [0, 0, 0, 0];
            for (let i = 0; i < bytes.length; i += 4) {
                const [b, g, r] = [bytes[i], bytes[i + 1], bytes[i + 2]];
                if (r > 200 && g < 60 && b < 60) count[0]++;
                else if (g > 200 && r < 60 && b < 60) count[1]++;
                else if (b > 200 && r < 60 && g < 60) count[2]++;
                else if (r < 60 && g < 60 && b < 60) count[3]++;
            }
            return count;
        };
        const size = await run('[t.cols, t.rows].join()');
        if (size !== '20,5') fail('size ' + size);
        await run(String.raw`t.output('\x1b[41m    \x1b[0m\r\n\x1b[31mXXXX\x1b[0m plain\r\n\x1b[7m  \x1b[0m\r\n')`);
        const [fill, text, inverse] = [await colors(0), await colors(1), await colors(2)];
        const cell = fill[0] / 4;
        if (cell < 100 || fill[1] || fill[3]) fail('red cells: ' + fill);
        if (text[0] < 40 || text[0] > cell * 2 || text[3] < 40) fail('red and plain text: ' + text);
        if (Math.abs(inverse[3] - cell * 2) > cell / 4) fail('inverse cells: ' + inverse + ' cell ' + cell);
        // Without a session, the element is an empty box.
        await run('el.session = null');
        const empty = await colors(0);
        if (empty[0] || empty[3]) fail('element without a session: ' + empty);
        await run('el.session = t');
        if ((await colors(0))[0] !== fill[0]) fail('session again: ' + await colors(0));
        // The theme follows the palette properties and the colors of the element.
        const computed = await run("[getComputedStyle(el).getPropertyValue('-terminal-red'), getComputedStyle(el).getPropertyValue('-terminal-blue'), CSS.supports('-terminal-red', 'red'), CSS.supports('-terminal-red', '1px')].join()");
        if (computed !== 'rgb(255, 0, 0),auto,true,false') fail('computed ' + computed);
        await run("el.style.setProperty('-terminal-red', '#00ff00'); el.style.color = '#0000ff'");
        const themed = [await colors(0), await colors(1), await colors(2)];
        if (themed[0][1] !== fill[0] || themed[0][0]) fail('themed cells: ' + themed[0]);
        if (themed[1][1] < 40 || themed[1][2] < 40 || themed[1][0] || themed[1][3]) fail('themed text: ' + themed[1]);
        if (Math.abs(themed[2][2] - cell * 2) > cell / 4) fail('themed inverse cells: ' + themed[2]);
        // A session with a program gets the size of its element.
        for (let i = 0; i < 100 && !(await run('p.text')).includes('in /'); i++) await wait(20);
        const program = await run("[p.text, p.pid > 0, p.cols, p.rows].join()");
        if (program !== 'in /,true,20,2') fail('program ' + program);
        // The grid follows the box of the element.
        // The first frame gave the terminal its size.
        if (await run('events.splice(0).join()') !== 'resize') fail('first size');
        await run("el.style.width = '30ch'; el.style.height = '6lh'");
        await win.webContents.capturePage();
        await wait(50);
        const resized = await run('[t.cols, t.rows, events.join()].join()');
        if (resized !== '30,6,resize') fail('resized ' + resized);
        // A click moves the focus to the terminal, and keys go to the program.
        await run("events.length = 0; t.addEventListener('data', event => events.push(JSON.stringify(event.data)))");
        await input({ action: 'click', x: 150, y: 40 });
        await wait(50);
        if (await run('document.activeElement === el') !== true) fail('focus');
        await input({ action: 'key', key: 'x shift-y enter up ctrl-c tab' });
        await input({ action: 'type', text: 'é' });
        await wait(50);
        const typed = await run('events.join()');
        if (typed !== String.raw`"x","Y","\r","\u001b[A","\u0003","\t","é"`) fail('typed ' + typed);
        // A drag selects text, and the wheel scrolls the scrollback.
        await input({ action: 'drag', x: 11, y: 40, to_x: 49, to_y: 40 });
        await wait(50);
        const selected = await run('t.selectedText');
        if (selected !== 'XXXX') fail('selected ' + JSON.stringify(selected));
        const marked = await colors(1);
        await run('t.clearSelection()');
        const cleared = await colors(1);
        if (marked[1] <= cleared[1]) fail('selection colors: ' + marked + ' then ' + cleared);
        await run(String.raw`t.output('\r\n1\r\n2\r\n3\r\n4\r\n5\r\n\x1b[42m \x1b[0m')`);
        const bottom = await colors(5);
        await input({ action: 'scroll', x: 50, y: 40, delta_x: 0, delta_y: -60 });
        await wait(50);
        const scrolled = await colors(5);
        if (!bottom[1] || scrolled[1]) fail('scrolled: ' + bottom + ' then ' + scrolled);
        // The bar of the scrollback shows after a scroll, and its knob scrolls the rows.
        const knob = async () => {
            const image = await win.webContents.capturePage({ x: 300, y: 0, width: 20, height: 140 });
            const bytes = image.toBitmap(), { width, height } = image.getSize();
            let [first, last] = [-1, -1];
            for (let y = 0; y < height; y++) for (let x = 0; x < width; x++) {
                const i = (y * width + x) * 4;
                if (bytes[i] > 90 && bytes[i] < 170 && Math.abs(bytes[i] - bytes[i + 1]) < 8 && Math.abs(bytes[i] - bytes[i + 2]) < 8) { if (first < 0) first = y; last = y; }
            }
            return [first, last].map(y => y * 140 / height);
        };
        const [start, end] = await knob();
        if (start < 0 || end - start < 20 || end > 120) fail('bar after a scroll: ' + [start, end]);
        await input({ action: 'drag', x: 305, y: (start + end) / 2, to_x: 305, to_y: 139 });
        await wait(50);
        const dragged = [await colors(5), await knob()];
        if (!dragged[0][1] || dragged[1][1] < 130) fail('dragged knob: ' + dragged.join(' / '));
        await input({ action: 'scroll', x: 50, y: 40, delta_x: 0, delta_y: -60 });
        await wait(50);
        await run('t.scrollToBottom()');
        if (!(await colors(5))[1]) fail('scrolled back');
        // Images of the Kitty graphics protocol: RGBA pixels, and a PNG file.
        await run(String.raw`{
            t.reset();
            const canvas = document.createElement('canvas');
            canvas.width = canvas.height = 2;
            const context = canvas.getContext('2d');
            context.fillStyle = '#00ff00';
            context.fillRect(0, 0, 2, 2);
            t.output('\x1b_Ga=T,f=32,s=1,v=1,c=4,r=1;AAD//w==\x1b\\\r\n\x1b_Ga=T,f=100,c=2,r=1;' + canvas.toDataURL().split(',')[1] + '\x1b\\');
        }`);
        const [pixels, png] = [await colors(0), await colors(1)];
        if (Math.abs(pixels[2] - cell * 4) > cell / 4 || pixels[1]) fail('image of pixels: ' + pixels + ' cell ' + cell);
        if (Math.abs(png[1] - cell * 2) > cell / 4) fail('PNG image: ' + png + ' cell ' + cell);
        await run(String.raw`t.output('\x1b_Ga=d\x1b\\')`);
        if ((await colors(0))[2] > cell / 4) fail('deleted image');
        "#,
        &[
            (
                "term.html",
                "<style>:root{-terminal-red:#ff0000;-terminal-green:#00ff00}body{margin:0;background:white}\
                 terminal{display:block;font:16px/20px Menlo;width:20ch;height:5lh;padding:10px;color:#000}\
                 terminal::selection{background:#00ff00}</style>\
                 <terminal id=t></terminal><terminal id=p style='height:2lh'></terminal>\
                 <script type=preload>__apps.contextBridge.exposeInMainWorld('create', options => new __apps.terminal.Session(options));</script>\
                 <script>globalThis.el = document.getElementById('t'); globalThis.t = create({ manual: true }); el.session = t;\
                 globalThis.events = []; t.addEventListener('resize', () => events.push('resize'));\
                 globalThis.p = create({ command: ['/bin/sh', '-c', 'printf \"in $PWD\"; sleep 9'], cwd: '/' });\
                 document.getElementById('p').session = p;</script>",
            ),
            ("package.json", r#"{"features":["terminal"]}"#),
        ],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn inset_shadows_stay_out_of_the_scroll_range_like_chrome() {
    let text = run_hidden(
        r#"
        await load('t.html');
        await run("document.getElementById('p').showPopover(); document.getElementById('s').scrollTop = 100");
        for (let n = 0; n < 3; n++) await win.webContents.capturePage();
        const sizes = await run("[...document.querySelectorAll('.c')].map(c => [c.scrollWidth, c.clientWidth, c.scrollHeight, c.clientHeight].join(',')).join(' ')");
        const bytes = (await win.webContents.capturePage({ x: 205, y: 205, width: 1, height: 1 })).toBitmap();
        console.log('SIZES ' + sizes + ' EDGE ' + [bytes[2], bytes[1], bytes[0]].map(value => value > 200 ? 'f' : value < 60 ? '0' : '?').join(''));
        "#,
        &[(
            "t.html",
            "<body style='margin:0'><style>.c{width:190px;padding:8px;box-sizing:border-box;overflow:auto;box-shadow:inset 0 1px 3px #000,0 0 0 5px #eee}.c i{display:block;height:30px}</style>\
             <div class=c style='display:grid;gap:1px'>\n <i></i><i></i>\n</div>\
             <div class=c id=p popover=manual style='display:grid;gap:1px;position:fixed;inset:20px auto auto 200px;margin:0;border:0'><i></i><i></i></div>\
             <div class=c id=s style='position:absolute;left:200px;top:200px;width:80px;height:60px;padding:0;background:#fff;box-shadow:inset 0 0 0 8px #f00'><i style='height:300px'></i></div></body>",
        )],
    );
    let line = text
        .lines()
        .find_map(|line| line.strip_prefix("SIZES "))
        .unwrap_or_else(|| panic!("{text}"));
    // Chrome's values: the children fit, so the boxes don't scroll, and the shadow of a scrolled box stays at its edge.
    assert_eq!(line, "190,190,77,77 190,190,77,77 80,80,300,60 EDGE f00");
}

#[test]
#[ignore = "requires the macOS window server"]
fn backgrounds_stay_out_of_the_scroll_range_like_chrome() {
    let text = run_hidden(
        r#"
        await load('t.html');
        await run("document.getElementById('s').scrollTop = 100");
        for (let n = 0; n < 3; n++) await win.webContents.capturePage();
        const sizes = await run("[...document.querySelectorAll('.c')].map(c => [c.scrollWidth, c.clientWidth, c.scrollHeight, c.clientHeight].join(',')).join(' ')");
        const bytes = (await win.webContents.capturePage({ x: 205, y: 205, width: 1, height: 1 })).toBitmap();
        console.log('SIZES ' + sizes + ' EDGE ' + [bytes[2], bytes[1], bytes[0]].map(value => value > 200 ? 'f' : value < 60 ? '0' : '?').join(''));
        "#,
        &[(
            "t.html",
            "<body style='margin:0'><style>.c{width:120px;height:60px;padding:14px 16px;box-sizing:border-box;overflow-x:auto}.c i{display:block;height:20px}</style>\
             <div class=c style='background:radial-gradient(#111,#000)'><i></i></div>\
             <div class=c style='background:#000;background-clip:padding-box'><i></i></div>\
             <div class=c id=s style='position:absolute;left:200px;top:200px;padding:0;background:linear-gradient(#f00 0 20px,#fff 0)'><i style='height:300px'></i></div></body>",
        )],
    );
    let line = text
        .lines()
        .find_map(|line| line.strip_prefix("SIZES "))
        .unwrap_or_else(|| panic!("{text}"));
    // Chrome's values: the child fits, so the boxes don't scroll, and the background of a scrolled box stays in place.
    assert_eq!(line, "120,120,60,60 120,120,60,60 120,120,300,60 EDGE f00");
}

#[test]
#[ignore = "requires the macOS window server"]
fn pseudo_element_layers_follow_the_inner_border_corner_like_chrome() {
    let text = run_hidden(
        r#"
        await load('t.html');
        const out = [];
        for (const [x, y] of [[16, 16], [3, 30], [50, 30]]) {
            const bytes = (await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap();
            out.push([bytes[2], bytes[1], bytes[0]].map(value => value > 200 ? 'f' : value < 60 ? '0' : '?').join(''));
        }
        console.log('COLORS ' + out.join(' '));
        "#,
        &[(
            "t.html",
            "<body style='margin:0;background:#fff'><style>p{margin:0}p::before{content:'';display:block;width:84px;height:44px;\
             border:8px solid red;border-radius:30px;background:linear-gradient(#000,#000)}</style><p></p></body>",
        )],
    );
    let line = text
        .lines()
        .find_map(|line| line.strip_prefix("COLORS "))
        .unwrap_or_else(|| panic!("{text}"));
    // Chrome's colors: inside the inner corner curve, in the left border, and in the middle.
    assert_eq!(line, "000 f00 000");
}

#[test]
#[ignore = "requires the macOS window server"]
fn observing_a_target_again_reports_nothing_new_like_chrome() {
    let text = run_hidden(
        r#"
        await load('t.html');
        await run(`globalThis.counts = [0, 0];
            const el = document.getElementById('a');
            const resize = new ResizeObserver(() => { counts[0]++; resize.observe(el); });
            const seen = new IntersectionObserver(() => { counts[1]++; seen.observe(el); });
            resize.observe(el); seen.observe(el); 1`);
        for (let n = 0; n < 8; n++) await win.webContents.capturePage();
        console.log('COUNTS ' + await run("counts.join(',')"));
        "#,
        &[(
            "t.html",
            "<body style='margin:0'><div id=a style='width:50px;height:50px'></div></body>",
        )],
    );
    let counts = text
        .lines()
        .find_map(|line| line.strip_prefix("COUNTS "))
        .unwrap_or_else(|| panic!("{text}"));
    // Chrome's counts: one first report each, as the targets are already observed when the callbacks observe them.
    assert_eq!(counts, "1,1");
}

#[test]
#[ignore = "requires the macOS window server"]
fn fading_pseudo_elements_draw_at_their_animated_opacity_like_chrome() {
    let text = run_hidden(
        r#"
        await load('t.html');
        const color = async () => {
            for (let n = 0; n < 3; n++) await win.webContents.capturePage();
            const bytes = (await win.webContents.capturePage({ x: 20, y: 20, width: 1, height: 1 })).toBitmap();
            return [bytes[2], bytes[1], bytes[0]].map(value => Math.round(value / 32)).join('.');
        };
        const first = await color();
        await run("document.getElementById('a').classList.add('on')");
        const changed = await color();
        console.log('LOOK ' + first + ' ' + changed + ' ' + await run("getComputedStyle(document.getElementById('a'), '::before').opacity"));
        "#,
        &[(
            "t.html",
            "<body style='margin:0;background:#fff'><style>@keyframes f{from,to{opacity:.5}}@keyframes g{from,to{opacity:.25}}\
             #a::before{content:'';position:absolute;inset:0;background:#f00;animation:f 1s infinite}#a.on::before{animation-name:g}</style>\
             <div id=a style='position:relative;width:60px;height:60px'></div></body>",
        )],
    );
    let look = text
        .lines()
        .find_map(|line| line.strip_prefix("LOOK "))
        .unwrap_or_else(|| panic!("{text}"));
    // Chrome's look: red at half opacity, then at a quarter, and the computed opacity follows the animation.
    assert_eq!(look, "8.4.4 8.6.6 0.25");
}

#[test]
#[ignore = "requires the macOS window server"]
fn faded_leaves_with_overlapping_paints_fade_as_a_group() {
    let text = run_hidden(
        r#"
        await load('t.html');
        for (let n = 0; n < 3; n++) await win.webContents.capturePage();
        const red = async (x, y) => (await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap()[2];
        console.log('LOOK ' + [await red(17, 40), await red(122, 40), await red(140, 140)].map(value => Math.round(value / 32)).join());
        "#,
        &[(
            "t.html",
            "<body style='margin:0;background:#fff'><style>i{position:absolute;width:60px;height:60px;opacity:.5}</style>\
             <i style='left:20px;top:20px;box-shadow:0 0 0 6px #fff,0 0 0 12px #000'></i>\
             <i style='left:120px;top:20px;background:#000;border:6px solid #fff'></i>\
             <i style='left:120px;top:120px;background:#000'></i></body>",
        )],
    );
    let look = text
        .lines()
        .find_map(|line| line.strip_prefix("LOOK "))
        .unwrap_or_else(|| panic!("{text}"));
    // Chrome's look: the white ring and the border hide what is under them, and a lone background fades alone.
    assert_eq!(look, "8,8,4");
}

#[test]
#[ignore = "requires the macOS window server"]
fn blur_animations_draw_their_animated_blur_like_chrome() {
    let text = run_hidden(
        r#"
        await load('t.html');
        for (let n = 0; n < 3; n++) await win.webContents.capturePage();
        const shades = [];
        for (const [x, y] of [[10, 40], [16, 40], [20, 40], [24, 40], [30, 40], [140, 50], [146, 50], [150, 50], [154, 50], [160, 50], [146, 90]]) {
            const bytes = (await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap();
            shades.push([bytes[2], bytes[1], bytes[0]]);
        }
        console.log('SHADES ' + JSON.stringify(shades));
        "#,
        &[(
            "t.html",
            "<body style='margin:0;background:#fff'><style>@keyframes f{from{filter:blur(0)}to{filter:blur(8px)}}\
             @keyframes g{from{backdrop-filter:blur(0)}to{backdrop-filter:blur(12px)}}\
             #s{position:absolute;left:20px;top:20px;width:40px;height:40px;background:#000;filter:blur(0);animation:f 1000s linear -500s}\
             #p{position:absolute;left:100px;top:0;width:100px;height:100px;background:linear-gradient(90deg,red 50%,blue 50%)}\
             #b{position:absolute;left:100px;top:20px;width:100px;height:60px;backdrop-filter:blur(0);animation:g 1000s linear -500s}</style>\
             <div id=s></div><div id=p></div><div id=b></div></body>",
        )],
    );
    let shades: Vec<[i32; 3]> = text
        .lines()
        .find_map(|line| line.strip_prefix("SHADES "))
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_else(|| panic!("{text}"));
    // Chrome halfway through: the square blurred by 4px, and the edge of the stripes under the backdrop by 6px.
    let chrome = [
        [253, 253, 253],
        [206, 206, 206],
        [115, 115, 115],
        [33, 33, 33],
        [1, 1, 1],
        [240, 0, 15],
        [184, 0, 71],
        [119, 0, 136],
        [58, 0, 197],
        [10, 0, 245],
        [255, 0, 0],
    ];
    for (shade, chrome) in shades.iter().zip(chrome) {
        assert!(
            shade.iter().zip(chrome).all(|(a, b)| (a - b).abs() <= 16),
            "{shades:?}"
        );
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn transform_animations_draw_content_that_moves_far_into_view() {
    // The animated belt is 1000px left of its place, so the box at 1050px shows at 50px.
    run_hidden(
        r#"
        await load('t.html');
        for (let n = 0; n < 3; n++) await win.webContents.capturePage();
        for (const [id, y] of [['run', 30], ['stop', 130]]) {
            const shade = async x => (await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap()[1];
            const [box, beside] = [await shade(100), await shade(250)];
            if (box > 64 || beside < 192) fail(id + ' ' + box + ' ' + beside);
        }
        "#,
        &[(
            "t.html",
            "<body style='margin:0;background:#fff'><style>@keyframes f{to{transform:translateX(-2000px)}}\
             .w{overflow:hidden;height:60px;margin-bottom:40px}.b{width:3000px;height:60px;position:relative;animation:f 1000s linear -500s}\
             .b>i{position:absolute;left:1050px;top:0;width:100px;height:60px;background:#000}</style>\
             <div class=w><div class=b><i></i></div></div>\
             <div class=w><div class=b style='animation-play-state:paused'><i></i></div></div></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn corner_shapes_draw_like_chrome() {
    // Points that Chromium draws black (inside) or white (outside) for these shapes.
    run_hidden(
        r#"
        await load('page.html');
        const shade = async (x, y) => (await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap()[1];
        const points = [
            ['squircle in', 10, 10, true], ['squircle out', 4, 4, false],
            ['bevel out', 125, 15, false], ['bevel in', 135, 25, true],
            ['scoop out', 245, 25, false], ['scoop in', 255, 35, true], ['scoop side', 222, 50, true],
            ['notch out', 30, 140, false], ['notch in', 45, 120, true], ['notch side', 10, 155, true],
            ['round out', 120, 120, false], ['round in', 125, 125, true],
        ];
        for (const [name, x, y, inside] of points) {
            const value = await shade(x, y);
            if ((value < 128) !== inside) fail(name + ' ' + value);
        }
        const style = await run(`(() => {
            const cs = getComputedStyle(document.querySelector('div'));
            return [cs.cornerShape, cs.cornerTopLeftShape, CSS.supports('corner-shape', 'superellipse(-2.5)'), CSS.supports('corner-shape', 'blob')].join();
        })()`);
        if (style !== 'superellipse(2),superellipse(2),true,false') fail(style);
        "#,
        &[(
            "page.html",
            "<body style='margin:0;background:white'><style>div{position:absolute;top:0;width:100px;height:100px;\
             background:black;border-radius:40px}</style><div style='left:0;corner-shape:squircle'></div>\
             <div style='left:110px;corner-shape:bevel'></div><div style='left:220px;corner-shape:scoop'></div>\
             <div style='left:0;top:110px;corner-shape:notch'></div><div style='left:110px;top:110px'></div></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn wide_gamut_colors_draw_in_display_p3() {
    // Values of Chromium with `--force-color-profile` srgb and display-p3-d65.
    let page = "<body style='margin:0;background:white'><style>body>*{position:absolute;width:100px;height:100px}</style>\
        <div style='left:0;top:0;background:#f00'></div>\
        <div style='left:100px;top:0;background:rgb(200,100,50)'></div>\
        <div style='left:200px;top:0;background:oklch(0.64 0.31 48)'></div>\
        <div style='left:300px;top:0;background:color(display-p3 0 1 0)'></div>\
        <div style='left:0;top:100px;background:blue'><div style='height:100%;background:rgba(255,0,0,.5)'></div></div>\
        <div style='left:100px;top:100px;background:linear-gradient(90deg,#f00,#00f)'></div>\
        <svg style='left:200px;top:100px'><rect width='100' height='100' fill='#f00'/></svg>\
        <div style='left:300px;top:100px;box-sizing:border-box;border:50px solid oklch(0.64 0.31 48)'></div>\
        <div style='left:0;top:200px;background:blue'><div style='height:100%;background:oklch(0.64 0.31 48 / .5)'></div></div>\
        <div style='left:100px;top:200px;background:linear-gradient(90deg,oklch(0.64 0.31 48),#00f)'></div></body>";
    let main = |expected: &str| {
        format!(
            r#"
        await load('page.html');
        const points = [[50, 50], [150, 50], [250, 50], [350, 50], [50, 150], [150, 150], [250, 150], [350, 150], [50, 250], [150, 250]];
        const expected = {expected};
        const got = [];
        for (const [x, y] of points) got.push([...(await win.webContents.capturePage({{ x, y, width: 1, height: 1 }})).toBitmap()].slice(0, 3).reverse());
        if (got.some((pixel, at) => pixel.some((value, channel) => Math.abs(value - expected[at][channel]) > 2))) fail(JSON.stringify(got));
        "#
        )
    };
    let page = [("page.html", page)];
    run_hidden(
        &main(
            "[[255,0,0],[200,100,50],[255,0,0],[0,255,0],[128,0,127],[126,0,128],[255,0,0],[255,0,0],[128,0,127],[145,90,136]]",
        ),
        &page,
    );
    run_hidden(
        &(main(
            "[[234,51,35],[187,105,62],[249,36,0],[0,255,0],[117,26,140],[115,20,124],[234,51,35],[249,36,0],[124,18,123],[137,92,133]]",
        ) + "// FORCE_P3"),
        &page,
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn content_visibility_skips_contents_like_chrome() {
    let page = "<body style='margin:8px;font:14px/18px Helvetica'><style>\
        .box{background:#cde;border:3px solid;padding:5px;width:120px}.cv{content-visibility:hidden}\
        .inner{height:50px}.ib{display:inline-block;width:auto}\
        .item{content-visibility:auto;contain-intrinsic-size:auto 200px;height:auto}.item div{height:120px}</style>\
        <div id=a class='box cv'><div id=a1 class=inner>hidden text</div><button id=btn>b</button></div>\
        <div id=b class='box cv' style='contain-intrinsic-size:80px 40px'><div class=inner></div></div>\
        <div id=c class='box ib cv' style='contain-intrinsic-width:60px'><div class=inner></div></div>\
        <div id=d class='box cv' style='contain-intrinsic-size:auto 20px'><div class=inner></div></div>\
        <div id=h class=box hidden=until-found><p id=t>found</p></div>\
        <div id=list></div></body>";
    // Captures draw frames in the hidden window.
    let text = run_hidden(
        "await load('cv.html');
        const frames = async () => { for (let n = 0; n < 3; n++) await win.webContents.capturePage(); };
        await run(`globalThis.out = {}; globalThis.states = [];
            globalThis.height = id => document.getElementById(id).getBoundingClientRect().height;
            document.addEventListener('contentvisibilityautostatechange', e => states.push(e.target.id + ':' + e.skipped));
            for (let n = 0; n < 8; n++) document.getElementById('list').insertAdjacentHTML('beforeend', '<div class=item id=s' + n + '><div></div></div>');`);
        await frames();
        await run(`out.sizes = ['a', 'b', 'c', 'h'].map(height).join();
            out.width = document.getElementById('c').getBoundingClientRect().width;
            out.inner = Object.values(document.getElementById('a1').getBoundingClientRect().toJSON()).slice(0, 4).join();
            out.hit = document.elementFromPoint(20, 12)?.id;
            document.getElementById('btn').focus();
            out.focus = document.activeElement.localName;
            out.text = document.getElementById('a').innerText;
            out.visible = document.getElementById('a1').checkVisibility();
            out.items = [0, 1, 2, 3, 4, 5, 6, 7].map(n => height('s' + n)).join();
            document.getElementById('d').classList.remove('cv');`);
        await frames();
        await run(`document.getElementById('d').classList.add('cv'); location.hash = '#t';`);
        await frames();
        await new Promise(resolve => setTimeout(resolve, 50));
        const result = await run(`out.states = states.join(' ');
            out.remembered = height('d');
            out.found = [document.getElementById('h').hidden, height('h')].join();
            JSON.stringify(out)`);
        console.log('CV ' + result);",
        &[("cv.html", page)],
    );
    let line = text
        .lines()
        .find_map(|line| line.strip_prefix("CV "))
        .expect(&text);
    let out: serde_json::Value = serde_json::from_str(line).unwrap();
    // The values of Chrome for the same page.
    assert_eq!(out["sizes"], "16,56,16,16", "{text}");
    assert_eq!(out["width"], 76, "{text}");
    assert_eq!(out["inner"], "16,16,120,50", "{text}");
    assert_eq!(out["hit"], "a", "{text}");
    assert_eq!(out["focus"], "body", "{text}");
    assert_eq!(out["text"], "", "{text}");
    assert_eq!(out["visible"], false, "{text}");
    assert_eq!(out["remembered"], 66, "{text}");
    assert_eq!(out["found"], "false,62", "{text}");
    assert_eq!(out["items"], "120,120,120,120,120,200,200,200", "{text}");
    let states = "s0:true s1:true s2:true s3:true s4:true s5:true s6:true s7:true s0:false s1:false s2:false s3:false s4:false";
    assert!(
        out["states"].as_str().unwrap().starts_with(states),
        "{text}"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn auto_margins_of_a_box_wider_than_its_container_start_at_zero_like_chrome() {
    let (text, rects) = picture_rects(
        "margins.html",
        "<body style='margin:0'><style>.w{width:300px}.o{width:100%;padding:0 28px;height:10px}\
         .f{display:flex;flex-direction:column}.r{display:flex;height:40px}</style>\
         <div class=w><div id=a0 class=o style='margin:0 auto'></div></div>\
         <div class=w><div id=a1 class=o style='margin-left:auto'></div></div>\
         <div class='w f'><div id=a2 class=o style='margin:0 auto'></div></div>\
         <div class=w dir=rtl><div id=a3 class=o style='margin:0 auto'></div></div>\
         <div class=w dir=rtl><div id=a4 class=o style='margin-right:auto'></div></div>\
         <div class=w dir=rtl><div id=a5 class=o></div></div>\
         <div class='w f' dir=rtl><div id=a6 class=o style='margin:0 auto'></div></div>\
         <div class='w r'><div id=a7 style='width:20px;height:60px;margin:auto 0'></div></div></body>",
    );
    for (id, x) in [
        ("a0", 0),
        ("a1", 0),
        ("a2", 0),
        ("a3", -56),
        ("a4", -56),
        ("a5", -56),
        ("a6", -56),
    ] {
        assert!(
            rects[id].starts_with(&format!("{x},0,")),
            "{id} at {x}, as in Chrome: {text}"
        );
    }
    assert_eq!(rects["a7"], "0,0,20,60", "{text}");
}

#[test]
#[ignore = "requires the macOS window server"]
fn align_content_centers_nowrap_buttons_and_blocks_like_chrome() {
    let (text, rects) = picture_rects(
        "align.html",
        "<body style='margin:0;font:14px/20px Menlo'><style>.w{width:300px}\
         button{height:40px;padding:0;border:0;font:inherit;white-space:nowrap}</style>\
         <div class=w><button><span id=b0>ab</span> cd</button></div>\
         <div class=w><div style='height:40px;align-content:center'><span id=c0>ab</span></div></div>\
         <div class=w><div style='height:40px;align-content:end;white-space:nowrap'><span id=c1>ab</span></div></div>\
         <div class=w><div style='height:40px;align-content:center;padding-top:6px'><div id=c2 style='height:10px'></div></div></div>\
         <div class=w style='position:relative;height:40px'><div style='position:absolute;inset-block:0;align-content:center'><span id=c3>ab</span></div></div>\
         <div class=w><div style='height:40px;align-content:space-between'><span id=c4>ab</span></div></div>\
         <div class=w><div style='height:40px;align-content:center'><div id=o1 style='height:80px'></div></div></div>\
         <div class=w><div style='height:40px;align-content:unsafe center'><div id=o2 style='height:80px'></div></div></div>\
         <div class=w><button style='display:inline;height:50px'><span id=i1>ab</span></button></div>\
         <div class=w><div style='height:40px;align-content:end;display:flow-root'><div id=k5 style='height:10px;float:left;width:10px'></div></div></div>\
         <div class=w><div style='height:40px;align-content:center'><div id=m2 style='height:10px;margin-top:10px'></div></div></div></body>",
    );
    let y = |id: &str| rects[id].split(',').nth(1).unwrap().parse::<f32>().unwrap();
    for (id, top) in [
        ("b0", 12.),
        ("c0", 12.),
        ("c1", 22.),
        ("c2", 21.),
        ("c3", 12.),
        ("c4", 2.),
        // Overflowing content starts at the top unless `unsafe`, an inline button still centers, and the block
        // is a formatting context of its own that holds its floats and margins.
        ("o1", 0.),
        ("o2", -20.),
        ("i1", 17.),
        ("k5", 30.),
        ("m2", 20.),
    ] {
        assert!(
            (y(id) - top).abs() < 0.6,
            "{id} at {top}, as in Chrome: {text}"
        );
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn min_max_and_clamp_with_percentages_resolve_at_layout_like_chrome() {
    let (text, rects) = picture_rects(
        "bounds.html",
        "<body style='margin:0'><style>.w{width:600px;display:flex}.w>i{width:100px;flex:none}.w>div{flex:1}\
         div>div{height:2px}#m4::before{content:'';flex:none;width:max(10%,3px)}</style>\
         <div class=w><i></i><div><div id=m0 style='width:max(10%,3px)'></div>\
         <div id=m1 style='width:min(80%,450px)'></div><div id=m2 style='width:clamp(10px,50%,300px)'></div>\
         <div id=m3 style='width:max(1%,30px)'></div><div id=m4 style='display:flex'><span id=m6></span></div>\
         <div id=m5 style='width:min(calc(100% - 20px),max(10%,700px));padding:0 5px'></div>\
         <div id=m7 style='width:calc(min(10%,80px) + 5px)'></div><div id=m8 style='width:min(10% * 2,300px)'></div>\
         <div style='text-align:center'><s id=m9 style='display:inline-block;height:2px;width:min(50%,300px)'></s></div>\
         </div></div></body>",
    );
    let width = |id: &str| rects[id].split(',').nth(2).unwrap().parse::<f32>().unwrap();
    for (id, expected) in [
        ("m0", 50.),
        ("m1", 400.),
        ("m2", 250.),
        ("m3", 30.),
        ("m5", 490.),
        ("m7", 55.),
        ("m8", 100.),
        ("m9", 250.),
    ] {
        assert_eq!(width(id), expected, "{id}, as in Chrome: {text}");
    }
    assert!(
        rects["m6"].starts_with("150,"),
        "a pseudo-element too: {text}"
    );
    assert!(
        rects["m9"].starts_with("225,"),
        "an inline-block alone in its line: {text}"
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn scroll_ranges_snap_to_device_pixels_like_chrome() {
    run_hidden(
        r#"
        await load('scroll.html');
        const ends = await run("[...document.querySelectorAll('.s')].map(s => (s.scrollTop = 1e5, s.scrollTop)).join()");
        // Chrome snaps both sizes to device pixels.
        const dpr = await run('devicePixelRatio');
        const want = { 1: '48,101,100', 2: '48.5,100.5,100' }[dpr];
        if (ends !== want) fail('scroll ends at ' + dpr + 'x: ' + ends);
        "#,
        &[(
            "scroll.html",
            "<body style='margin:0'><style>.s{overflow:auto;width:100px}</style>\
             <div class=s style='height:100.5px;margin-top:10.3px'><div style='height:148.75px'></div></div>\
             <div class=s style='height:100px'><div style='height:200.5px'></div></div>\
             <div class=s style='height:100.25px;padding:3.3px'><div style='height:200.6px'></div></div></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn bounds_keep_their_width_after_a_zoom_change() {
    run_hidden(
        r#"
        await load('zoom.html');
        const width = () => run("document.getElementById('t').getBoundingClientRect().width");
        for (const zoom of [1, 1.25, 1]) {
            win.webContents.setZoomFactor(zoom);
            await win.webContents.capturePage();
            await run("{ const t = document.getElementById('t'); for (let i = 0; i < 200; i++) { t.style.width = `min(50%, ${300 + i}px)`; t.getBoundingClientRect(); } }");
            const got = await width();
            if (Math.abs(got - 250) > 0.5) fail('width at zoom ' + zoom + ': ' + got);
        }
        "#,
        &[(
            "zoom.html",
            "<body style='margin:0'><div style='width:600px;display:flex'><i style='width:100px;flex:none'></i>\
             <div style='flex:1'><div id=t style='height:4px;width:min(50%,10px)'></div></div></div></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn rtl_pseudo_elements_keep_their_right_edge_when_wider_than_the_box() {
    run_hidden(
        r#"
        await load('rtl.html');
        const at = x => run(`document.elementFromPoint(${x}, 2)?.id ?? ''`);
        const [outside, inside] = [await at(330), await at(250)];
        if (outside === 'p' || inside !== 'p') fail('the pseudo-element is at ' + [outside, inside]);
        "#,
        &[(
            "rtl.html",
            "<body style='margin:0'><style>#p::before{content:'';display:block;width:360px;height:5px;margin:0 auto}</style>\
             <div dir=rtl style='width:300px'><div id=p></div></div></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn pretty_rows_rects_and_carets_of_case_mappings_like_chrome() {
    run_hidden(
        &r#"
        await load('page.html');
        // Chrome's values. Pretty moves `buff` down, but not with `break-spaces`, `::first-line` or a tab. Text far
        // below the window keeps its indent and justified spaces. A click in the `SS` of `ß` goes to its nearer side.
        const got = await run(`JS`);
        const want = '1,0,0,0 ; 30,1,26 0,21,40 13,41,35 106,41,38 ; 0,1,26 76,21,8 0,41,35 93,41,38 ; 4,4,5,5,5,6 ; 140';
        if (got !== want) fail(got);
        "#
        .replace("JS", r#"(()=>{const top=id=>{const p=document.getElementById(id);return p.querySelector('b').getBoundingClientRect().top-p.getBoundingClientRect().top>5?1:0};const word=(id,w)=>{const p=document.getElementById(id),t=p.firstChild,i=t.data.indexOf(w),r=document.createRange();r.setStart(t,i);r.setEnd(t,i+w.length);const o=p.getBoundingClientRect();return [...r.getClientRects()].map(q=>[q.x-o.x,q.y-o.y,q.width].map(Math.round).join(',')).join('|')};const s=document.getElementById('s').getBoundingClientRect(),t=document.getElementById('s').firstChild;const caret=x=>{const r=document.caretRangeFromPoint(s.left+x,s.top+s.height/2);return r.startOffset};const w=s.width;return [['a','b','c','d'].map(top).join(),['The','jumps','small','otters'].map(w=>word('g',w)).join(' '),['The','a','small','otters'].map(w=>word('h',w)).join(' '),[0.70,0.76,0.80,0.84,0.88,0.95].map(f=>caret(w*f)).join(),Math.round(w)].join(' ; ')})()"#),
        &[("page.html", "<body style='margin:0;font:13.5px Helvetica'><style>p{margin:0 0 4px;width:210px;text-wrap:pretty}b{font-weight:400}.f::first-line{color:red}i{font-style:normal}</style>\
             <p id=a>Can you grab 20 kg of <b>buff</b> stoneware?</p>\
             <p id=b style='white-space:break-spaces'>Can you grab 20 kg of <b>buff</b> stoneware?</p>\
             <p id=c class=f>Can you grab 20 kg of <b>buff</b> stoneware?</p>\
             <p id=d style='white-space:pre-wrap'>Can\tyou grab 20 kg of <b>buff</b> stoneware?</p>\
             <p id=e style='font-size:30px;width:auto;text-transform:uppercase'><i id=s>straße</i></p>\
             <div style='height:3000px'></div>\
             <p id=g style='font:15px/20px Helvetica;width:200px;text-indent:30px;text-wrap:wrap'>The quick brown fox jumps over the lazy dog while a small band of otters</p>\
             <p id=h style='font:15px/20px Helvetica;width:200px;text-align:justify;text-wrap:wrap'>The quick brown fox jumps over the lazy dog while a small band of otters</p>\
             </body>")],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn inline_boxes_go_back_into_their_lines_without_opacity() {
    let text = run_hidden(
        &format!(
            r#"{RECTS}
            await load('back.html');
            for (const value of ['0.5', '']) {{
              await run(`for (const id of ['a', 'c']) document.getElementById(id).style.opacity = '${{value}}'`);
              await win.webContents.capturePage();
            }}
            console.log('RECTS ' + await rects());
            "#
        ),
        &[(
            "back.html",
            "<body style='margin:0;font:14px/20px Helvetica'><style>.w{width:220px}</style>\
             <div class=w>aa lead <span id=a>plain child text with <i>italic</i> part in text</span> zz</div>\
             <div class=w>aa lead <span id=b>plain child text with <i>italic</i> part in text</span> zz</div>\
             <div class=w>aa <span style='border:1px solid'>lead <span id=c>plain child text with \
             <i>italic</i> part</span><div>blk</div> tail</span> zz</div>\
             <div class=w>aa <span style='border:1px solid'>lead <span id=d>plain child text with \
             <i>italic</i> part</span><div>blk</div> tail</span> zz</div></body>",
        )],
    );
    let rects = rects(&text);
    for (toggled, fresh) in [("a", "b"), ("c", "d")] {
        assert_eq!(rects[toggled], rects[fresh], "{toggled}: {text}");
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn inline_boxes_split_around_their_blocks_like_chrome() {
    let text = run_hidden(
        &format!(
            r#"{RECTS}
            const parts = () => run(`['a', 'c', 'd'].map(id => [...document.getElementById(id).getClientRects()]
                .map(rect => Math.round(rect.width)).join()).join('|')`);
            await load('split.html');
            console.log('PARTS ' + await parts());
            await run("document.getElementById('a').style.opacity = '0.5'");
            await win.webContents.capturePage();
            await run("document.getElementById('a').style.opacity = ''");
            await win.webContents.capturePage();
            console.log('RECTS ' + await rects());
            "#
        ),
        &[(
            "split.html",
            "<body style='margin:0;font:14px/20px Helvetica'><style>.w{width:300px}\
             .p::before{content:'B';display:block}.d{border:2px solid;padding:0 6px}</style>\
             <div class=w>One <span id=a>inline <div id=b>block</div> tail</span> two</div>\
             <div class=w>One <span id=c class=p>inline with block before</span> two</div>\
             <div class=w><span id=d class=d><div id=e>first</div>after</span></div></body>",
        )],
    );
    let rects = rects(&text);
    for (id, rect) in [
        ("a", "0,2,300,56"),
        ("b", "0,20,300,20"),
        ("c", "0,20,300,38"),
        ("d", "0,0,300,60"),
        ("e", "0,20,300,20"),
    ] {
        assert_eq!(rects[id], rect, "{id}, as in Chrome: {text}");
    }
    assert!(
        text.contains("PARTS 33,300,18|0,300,142|8,300,36"),
        "{text}"
    );
}
