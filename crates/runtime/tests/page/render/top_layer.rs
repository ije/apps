use super::*;

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
fn view_transitions_cross_fade_captures_and_end() {
    run_hidden(
        r#"
        const pixel = async (x, y) => [...(await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap().subarray(0, 3)].reverse();
        await load('vt.html');
        await run(`globalThis.t = document.startViewTransition(() => document.body.className = 'on'); t.ready.then(() => globalThis.ready = 1); 0`);
        for (let n = 0; n < 20 && !await run('globalThis.ready'); n++) await win.webContents.capturePage();
        if (!await run('globalThis.ready')) fail('not ready: ' + await run('document.activeViewTransition?._phase'));
        const state = await run(`[document.activeViewTransition === t, document.documentElement.matches(':active-view-transition'), document.elementFromPoint(30, 30).localName, document.getAnimations().length, getComputedStyle(document.documentElement, '::view-transition-group(box)').width].join()`);
        if (state !== 'true,true,html,10,40px') fail('during: ' + state);
        // Halfway, the old red box and the new blue one blend on the way.
        await run(`for (const a of document.getAnimations()) { a.pause(); a.currentTime = 125; }`);
        for (let n = 0; n < 3; n++) await win.webContents.capturePage();
        const [r, g, b] = await pixel(190, 30);
        if (!(r > 40 && b > 40 && g < 60)) fail('halfway: ' + [r, g, b]);
        await run(`for (const a of document.getAnimations()) a.finish(); t.finished.then(() => globalThis.done = 1); 0`);
        for (let n = 0; n < 60 && !await run('globalThis.done'); n++) await win.webContents.capturePage();
        const after = await run(`[document.activeViewTransition, document.documentElement.matches(':active-view-transition'), document.getAnimations().length].join()`);
        if (after !== ',false,0') fail('after: ' + after);
        const blue = await pixel(230, 30);
        if (!(blue[2] > 200 && blue[0] < 60)) fail('the page after: ' + blue);
        "#,
        &[(
            "vt.html",
            "<body style='margin:0;background:#fff'><style>#box{position:absolute;left:10px;top:10px;width:40px;height:40px;\
             background:red;view-transition-name:box}.on #box{left:210px;background:blue}</style><div id=box></div></body>",
        )],
    );
}
