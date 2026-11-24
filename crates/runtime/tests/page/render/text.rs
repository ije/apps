use super::*;

#[test]
#[ignore = "requires the macOS window server"]
fn positioned_inline_boxes_contain_the_boxes_in_them_from_their_first_to_their_last_line() {
    run_hidden(
        r#"
        await load('page.html');
        const start = await run(`Math.round((g => (g.selectNodeContents(document.getElementById('s').firstChild), g.getClientRects()[0].left))(document.createRange()))`);
        const span = await run(`Math.round(document.getElementById('s').getBoundingClientRect().bottom)`);
        const box = await run(`(r => [r.left, r.bottom].map(Math.round).join())(document.getElementById('i').getBoundingClientRect())`);
        if (box !== start + ',' + span) fail('the box at the start of the first line and the bottom of the last: ' + box + ' start ' + start + ' bottom ' + span);
        const height = await run(`document.getElementById('p').getBoundingClientRect().height`);
        if (height !== 36) fail('the text stays two lines: ' + height);
        const before = await run(`parseFloat(getComputedStyle(document.getElementById('s'), '::before').top)`);
        if (before !== -4) fail('the ::before of the inline box: ' + before);
        "#,
        &[(
            "page.html",
            "<head><style>body{margin:0;font:14px/18px Helvetica} #p{width:180px}\
             #s{position:relative} #s::before{content:'';position:absolute;top:-4px;left:0;width:4px;height:4px}\
             i{position:absolute;left:0;bottom:0;width:8px;height:8px}</style></head>\
             <body><div id=p>aaa bbb ccc <span id=s>ddd eee fff ggg hhh<i id=i></i></span> iii</div></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn boxes_with_break_spaces_text_share_styles() {
    let (text, rects) = picture_rects(
        "code.html",
        "<body style='margin:0;font:16px/20px Helvetica'><style>code{padding:.2em .4em;font-size:85%;\
         white-space:break-spaces;background:#eee;border-radius:6px}</style>\
         <p>One <code>inline code</code> and <code>width</code>.</p><ul class=w><li><code>show</code></li>\
         <li><code>true</code></li><li><code id=c0>false</code></li></ul></body>",
    );
    assert!(rects.contains_key("c0"), "{text}");
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
fn middle_boxes_and_preserved_inline_text_keep_lines_like_chrome() {
    let (text, rects) = picture_rects(
        "middle.html",
        "<body style='margin:0;font:16px/24px Helvetica'><style>.w{width:300px}\
         .k{display:inline-block;padding:4px;font:11px/10px Menlo;vertical-align:middle;border:1px solid}\
         td{padding:0}table{border-collapse:collapse}code{font:13px Menlo;padding:3px}</style>\
         <div class=w><p id=p0 style='margin:0'>A <kbd id=k0 class=k>K</kbd> z</p></div>\
         <div class=w><p id=p1 style='margin:0'>A <kbd id=k1 class=k style='margin:2px 0 6px'>K</kbd> z</p></div>\
         <div class=w><table><tr><td id=c0><code style='white-space:pre'>code</code></td></tr></table></div>\
         <div class=w><table><tr><td id=c1><code style='white-space:break-spaces'>code</code></td></tr></table></div>\
         <div class=w style='display:grid'><div id=g0><code style='white-space:pre-wrap'>code</code></div></div></body>",
    );
    for (id, [y, height]) in [
        ("p0", [0., 24.]),
        ("p1", [0., 28.]),
        ("k0", [3.31, 20.]),
        ("k1", [2., 20.]),
        ("c0", [0., 24.]),
        ("c1", [0., 24.]),
        ("g0", [0., 24.]),
    ] {
        let at: Vec<f32> = rects[id].split(',').map(|v| v.parse().unwrap()).collect();
        assert!(
            (at[1] - y).abs() < 0.05 && (at[3] - height).abs() < 0.05,
            "{id} at y {y} with height {height}, as in Chrome: {text}"
        );
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn baseline_items_of_columns_align_as_a_group_like_chrome() {
    let (text, rects) = picture_rects(
        "columns.html",
        "<body style='margin:0;font:16px/20px Helvetica'><style>.w{display:flex;flex-direction:column;width:300px}</style>\
         <div class=w dir=rtl style='align-items:baseline'><div id=a>first</div>\
         <div id=b style='font-size:30px;line-height:36px'>second</div></div>\
         <div class=w dir=rtl style='align-items:last baseline'><div id=f>first</div>\
         <div id=g style='font-size:30px'>second</div></div>\
         <div class=w style='align-items:baseline'><div id=d>first</div></div></body>",
    );
    for (id, x) in [
        ("a", 203.26),
        ("b", 203.26),
        ("f", 0.),
        ("g", 0.),
        ("d", 0.),
    ] {
        let at: f32 = rects[id].split(',').next().unwrap().parse().unwrap();
        assert!((at - x).abs() < 0.05, "{id} at x {x}, as in Chrome: {text}");
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
