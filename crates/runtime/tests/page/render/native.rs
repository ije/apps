use super::*;

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
        // A reloaded page draws its new sessions with their own theme, not with the drawings of the old ones.
        await run("el.removeAttribute('style')");
        await win.webContents.capturePage();
        await load('term.html');
        await run(String.raw`t.output('\x1b[41m    \x1b[0m')`);
        const reloaded = await colors(0);
        if (Math.abs(reloaded[0] - fill[0]) > cell / 4 || reloaded[1]) fail('reloaded page: ' + reloaded);
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
