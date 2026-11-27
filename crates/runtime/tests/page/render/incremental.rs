use super::*;

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
fn fields_that_a_scroll_moves_take_clicks_and_draw_their_selection() {
  // The page scrolls the field without a prepaint of it. Debug builds prepaint all boxes in every third pass, so
  // each check runs three times with one frame before it.
  let text = run_hidden(
    &format!(
      r#"{INPUT}
        const capture = () => win.webContents.capturePage({{ x: 0, y: 0, width: 400, height: 300 }});
        const pixel = async (x, y) => [...(await win.webContents.capturePage({{ x, y, width: 1, height: 1 }})).toBitmap()].slice(0, 3).join();
        const place = async () => JSON.parse(await run("(r => JSON.stringify([Math.round(r.x), Math.round(r.y + r.height / 2)]))(document.getElementById('f').getBoundingClientRect())"));
        await load('field.html');
        await frames(50);
        const seen = [];
        for (let n = 0; n < 3; n++) {{
            await run("document.getElementById('f').blur(); scrollBy(0, 20)");
            await capture();
            const [x, y] = await place();
            await fetch(MCP + '/input', {{ method: 'POST', body: JSON.stringify({{ window: win.id, action: 'click', x: x + 48, y }}) }});
            seen.push(await run("[document.activeElement.id, document.getElementById('f').selectionStart].join()"));
            for (const [end, color] of [[11, 'blue'], [0, 'white']]) {{
                await run(`document.getElementById('f').setSelectionRange(0, ${{end}})`);
                seen.push(color + ':' + await pixel(x + 4, y));
            }}
        }}
        console.log('SEEN ' + seen.join(' '));
        "#
    ),
    &[(
      "field.html",
      "<body style='margin:0;background:#fff'><div style='height:150px'></div><div style='padding:10px'>\
             <input id=f value='hello world' style='font:16px monospace;width:200px;border:0;padding:0;outline:0;background:#fff;color:#fff;caret-color:#fff'>\
             </div><div style='height:3000px'></div><style>#f::selection{background:#00f}</style></body>",
    )],
  );
  let seen = (text.lines()).find_map(|line| line.strip_prefix("SEEN "));
  assert_eq!(
    seen,
    Some(["f,5 blue:255,0,0 white:255,255,255"; 3].join(" ").as_str()),
    "{text}"
  );
}

#[test]
#[ignore = "requires the macOS window server"]
fn far_tiles_place_the_items_of_a_flexed_grid_at_its_new_size() {
  // The grid of a tile takes the rest of its column, so a taller caption moves its item up.
  let mut page = String::from(
    "<style>body{margin:0;font:14px sans-serif}#g{display:grid;grid-template-columns:1fr 1fr;\
         grid-auto-rows:200px;gap:10px}.t{display:flex;flex-direction:column}\
         .s{flex:1;display:grid;place-items:center}</style><body><main id=g>",
  );
  for index in 0..30 {
    page.push_str(&format!(
      "<article class=t id=t{index}><div class=s><span id=r{index}>R</span></div>\
             <p id=c{index} style='margin:0'>tile</p></article>"
    ));
  }
  page.push_str("</main></body>");
  let text = run_hidden(
    r#"
        const top = () => run("Math.round(document.getElementById('r25').getBoundingClientRect().y - document.getElementById('t25').getBoundingClientRect().y)");
        await load('tiles.html');
        await win.webContents.capturePage();
        await run("document.getElementById('c25').style.height = '40px'");
        for (let n = 0; n < 3; n++) await win.webContents.capturePage();
        console.log('FAR ' + await top());
        await run("document.getElementById('t25').scrollIntoView()");
        for (let n = 0; n < 3; n++) await win.webContents.capturePage();
        console.log('NEAR ' + await top());
        "#,
    &[("tiles.html", &page)],
  );
  let line = |prefix: &str| (text.lines()).find_map(|line| line.strip_prefix(prefix));
  assert!(
    line("FAR ").is_some() && line("FAR ") == line("NEAR "),
    "the item of a far tile takes the new size of its grid: {text}"
  );
}

#[test]
#[ignore = "requires the macOS window server"]
fn clipping_tiles_far_from_the_view_are_spacers_with_exact_boxes_inside() {
  // Each tile clips and positions what is in it, like the tiles of ui-sink. Tile 28 has a canvas, which is
  // uncullable inside any box.
  let mut page = String::from(
    "<style>body{margin:0;font:14px sans-serif}#g{display:grid;grid-template-columns:1fr 1fr;\
         grid-auto-rows:200px;gap:10px}.t{position:relative;display:flex;flex-direction:column;overflow:hidden}\
         .s{position:relative;flex:1;display:grid;place-items:center;overflow:hidden}\
         .r{display:inline-block;transform:rotate(10deg);padding:4px}.a{position:absolute;right:4px;top:4px}</style><body id=body><p id=o style='margin:0'>x</p><main id=g>",
  );
  for index in 0..30 {
    let canvas = ["", "<canvas width=8 height=8></canvas>"][usize::from(index == 28)];
    page.push_str(&format!(
            "<article class=t id=t{index}><div class=s id=s{index}><span class=r id=r{index}>R</span>\
             <b class=a id=a{index}>A</b>{canvas}</div><p id=c{index} style='margin:0'>tile</p></article>"
        ));
  }
  page.push_str("</main></body>");
  let text = run_hidden(
    &format!(
      r#"{STEPS}
            const rects = id => run(`JSON.stringify(['r', 'a', 's'].map(part => {{
                const box = document.getElementById(part + '${{id}}').getBoundingClientRect(), tile = document.getElementById('t${{id}}').getBoundingClientRect();
                return [box.x - tile.x, box.y - tile.y, box.width, box.height].map(v => Math.round(v * 10) / 10).join();
            }}))`);
            await load('tiles.html');
            await win.webContents.capturePage();
            await step('edit', () => run("document.getElementById('c0').style.height = '30px'"));
            await step('pinned', () => run("document.getElementById('t28').scrollIntoView()"));
            await step('back', () => run("scrollTo(0, 0)"));
            await step('far', () => run("document.getElementById('c25').style.height = '40px'; document.getElementById('r25').style.transform = 'rotate(45deg)'; document.getElementById('a25').textContent = 'longer'"));
            console.log('FAR ' + await rects(25));
            await step('near', () => run("document.getElementById('t25').scrollIntoView()"));
            console.log('NEAR ' + await rects(25));
            "#
    ),
    &[("tiles.html", &page)],
  );
  let built = built(&text, "pinned");
  assert!(
    built.contains(&"s29") && !built.contains(&"s28"),
    "far tiles are spacers that build again near the view, but not one with a canvas: {built:?}"
  );
  let line = |prefix: &str| {
    (text.lines())
      .find_map(|line| line.strip_prefix(prefix))
      .unwrap_or_default()
      .to_string()
  };
  assert_eq!(
    line("FAR "),
    line("NEAR "),
    "boxes in a far tile take its changes: {text}"
  );
}

#[test]
#[ignore = "requires the macOS window server"]
fn far_blocks_around_clipping_tiles_take_the_transforms_in_them() {
  // The row is a plain block far below the view, so it becomes a spacer when its tiles confine their cards.
  let tiles = "<div class=t><div class=f id=f><div class=c></div></div></div>".repeat(4);
  let page = format!(
    "<style>body{{margin:0}}.t{{position:relative;display:inline-block;width:200px;height:200px;overflow:hidden}}\
         .f{{position:absolute;left:50%;top:50%;translate:-50% -50%;scale:var(--s,1)}}.c{{width:300px;height:300px}}</style>\
         <body><div style='height:3000px'></div><div id=row>{tiles}</div><div style='height:3000px'></div></body>"
  );
  let text = run_hidden(
    r#"
        const width = () => run("String(Math.round(document.getElementById('f').getBoundingClientRect().width))");
        await load('row.html');
        for (let n = 0; n < 3; n++) await win.webContents.capturePage();
        await run("document.getElementById('f').style.setProperty('--s', '0.5')");
        for (let n = 0; n < 3; n++) await win.webContents.capturePage();
        console.log('FAR ' + await width());
        "#,
    &[("row.html", &page)],
  );
  assert!(
    text.contains("FAR 150"),
    "a far card takes its new scale: {text}"
  );
}

#[test]
#[ignore = "requires the macOS window server"]
fn cleared_blocks_far_below_floats_stay_spacers() {
  let mut page = String::from("<body id=body style='margin:0'><p id=o style='margin:0'>x</p><div>");
  for _ in 0..300 {
    page.push_str("<div style='float:left;width:90px;height:40px'></div>");
  }
  page.push_str("</div><div id=g style='clear:both'>");
  for index in 0..50 {
    page.push_str(&format!("<div id=c{index}>row {index}</div>"));
  }
  page.push_str("</div></body>");
  let text = run_hidden(
    &format!(
      r#"{STEPS}
            await load('cleared.html');
            await win.webContents.capturePage();
            await win.webContents.capturePage();
            await step('edit', () => run("document.getElementById('o').textContent = 'y'"));
            console.log('TOP ' + await run("Math.round(document.getElementById('g').getBoundingClientRect().top)"));
            "#
    ),
    &[("cleared.html", &page)],
  );
  let built = built(&text, "edit");
  assert!(
    !built.iter().any(|id| *id == "g" || id.starts_with('c')),
    "a cleared block far below floats is a spacer, which stays: {built:?}"
  );
  assert!(
    text.contains("TOP 3019"),
    "the spacer keeps the clearance: {text}"
  );
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

#[test]
#[ignore = "requires the macOS window server"]
fn shrunk_grids_with_new_content_replay_their_width_in_the_same_build() {
  // Debug builds build the second flex item again when it takes the style of the first, and visit the grid
  // that just took a narrower content width.
  run_hidden(
    r#"
        await load('page.html');
        await run("document.getElementById('g').textContent = 'ab'");
        await win.webContents.capturePage();
        const width = await run("document.getElementById('g').getBoundingClientRect().width");
        const want = await run("(r => (r.selectNodeContents(document.getElementById('g')), r.getBoundingClientRect().width))(document.createRange())");
        if (Math.abs(width - want) > 1) fail('the grid fits its new text: ' + width + ' ' + want);
        "#,
    &[(
      "page.html",
      "<body style='margin:0'><div style='display:flex'><div><div style='display:grid'>first</div></div></div>\
             <div style='display:flex'><div><div id=g style='display:grid'>second row</div></div></div></body>",
    )],
  );
}

#[test]
#[ignore = "requires the macOS window server"]
fn the_only_box_of_a_run_paints_its_new_color() {
  run_hidden(
    r#"
        await load('run.html');
        await win.webContents.capturePage();
        const pixel = async () => [...(await win.webContents.capturePage({ x: 10, y: 20, width: 1, height: 1 })).toBitmap()].slice(0, 3).reverse().join();
        if (await pixel() !== '0,128,0') fail('first color: ' + await pixel());
        await run("document.getElementById('l').style.color = 'rgb(255, 0, 0)'");
        await win.webContents.capturePage();
        if (await pixel() !== '255,0,0') fail('the new color did not draw: ' + await pixel());
        "#,
    &[(
      "run.html",
      "<body style='margin:0;font:40px/40px Menlo'><div><div></div><span id=l style='color:rgb(0,128,0)'>\u{2588}</span></div>\
             <script>getComputedStyle(document.getElementById('l')).color</script></body>",
    )],
  );
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
    ids
      .chars()
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
