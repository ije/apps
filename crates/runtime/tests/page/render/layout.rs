use super::*;

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
fn absolute_pseudo_elements_of_static_boxes_take_the_containing_block_of_an_ancestor() {
    run_hidden(
        r#"
        await load('page.html');
        const read = (id, pseudo) => run(`(s => [s.top, s.right, s.bottom, s.left, s.width, s.height].join())(getComputedStyle(document.getElementById('${id}'), '${pseudo}'))`);
        const corner = await read('e', '::before');
        if (corner !== '110px,0px,0px,200px,20px,10px') fail('a corner of the ancestor block: ' + corner);
        const stretched = await read('e', '::after');
        if (stretched !== '114px,0px,0px,0px,220px,6px') fail('stretched across the ancestor block: ' + stretched);
        const percent = await read('a', '::before');
        if (percent !== '12px,55px,84px,110px,55px,24px') fail('percentages of the ancestor block: ' + percent);
        "#,
        &[(
            "page.html",
            "<head><style>#p{position:relative;width:200px;height:100px;padding:10px} #p>div{margin:20px;height:30px;overflow:hidden}\
             #e::before{content:'';position:absolute;right:0;bottom:0;width:20px;height:10px;background:red}\
             #e::after{content:'';position:absolute;left:0;right:0;bottom:0;height:6px;background:blue}\
             #a::before{content:'';position:absolute;left:50%;top:10%;width:25%;height:20%;background:green}</style></head>\
             <body style='margin:0'><div id=p><div id=e></div><div id=a></div></div></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn absolute_boxes_in_contents_boxes_keep_their_static_place_in_the_line() {
    run_hidden(
        r#"
        await load('page.html');
        const box = await run(`(r => [r.left, r.top].map(Math.round).join())(document.getElementById('i').getBoundingClientRect())`);
        const text = await run(`Math.round((g => (g.selectNodeContents(document.getElementById('t')), g.getClientRects()[0].right))(document.createRange()))`);
        if (box !== text + ',0') fail('the box after the text: ' + box + ' text ' + text);
        "#,
        &[(
            "page.html",
            "<head><style>body{margin:0;font:14px/18px Helvetica} b{display:contents}\
             i{position:absolute;width:8px;height:8px}</style></head>\
             <body><div><span id=t>aaa</span><b><i id=i></i></b>bbb</div></body>",
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
fn fixed_floats_lay_out_their_content_alone() {
    run_hidden(
        r#"
        const expect = async (step, code, value) => {
            const got = await run(code);
            if (got !== value) fail(step + ': ' + got + ' instead of ' + value);
        };
        const rect = id => `(r => [r.x, r.y, r.width, r.height].map(Math.round).join())(document.getElementById('${id}').getBoundingClientRect())`;
        await load('floats.html');
        await win.webContents.capturePage();
        await expect('first', rect('i') + " + ' ' + " + rect('x'), '0,0,20,10 135,0,20,20');
        await run("document.getElementById('i').style.width = '60px'");
        await win.webContents.capturePage();
        await expect('inside', rect('i') + " + ' ' + " + rect('x'), '0,0,60,10 135,0,20,20');
        await run("document.getElementById('t').textContent = 'a much longer text'");
        await win.webContents.capturePage();
        await expect('text', rect('x'), '135,0,20,20');
        await run("document.getElementById('f').style.width = '200px'");
        await win.webContents.capturePage();
        await expect('outside', rect('x'), '235,0,20,20');
        "#,
        &[(
            "floats.html",
            "<body style='margin:0;font:16px/20px Helvetica;width:300px'>\
             <div id=f style='float:left;width:100px;height:40px;overflow:hidden'><div id=i style='width:20px;height:10px'></div>\
             <span id=t>text</span></div><div id=n>next <span id=x style='display:inline-block;width:20px;height:20px'></span></div></body>",
        )],
    );
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
fn negative_empty_and_container_margins_collapse_like_chrome() {
    let (text, rects) = picture_rects(
        "collapse.html",
        "<body style='margin:0;font:16px/20px Helvetica'><style>.w{width:600px}</style>\
         <div class=w><div id=a style='overflow:hidden'><div style='height:20px;margin-bottom:20px'></div>\
         <div style='margin:-30px 0 5px'></div></div></div>\
         <div class=w><div id=b style='overflow:hidden'><div style='height:20px'></div>\
         <div style='margin:10px 0;height:0'></div><div id=b2 style='margin:10px 0;min-height:1px'></div></div></div>\
         <div class=w><div style='height:20px'></div><div id=c style='container-type:inline-size;width:400px'>\
         <div id=c2 style='height:10px;margin-top:2cqi'></div></div></div></body>",
    );
    for (id, [y, height]) in [
        ("a", [0., 10.]),
        ("b", [0., 41.]),
        ("b2", [30., 1.]),
        ("c", [20., 18.]),
        ("c2", [28., 10.]),
    ] {
        let at: Vec<f32> = rects[id].split(',').map(|v| v.parse().unwrap()).collect();
        assert!(
            at[1] == y && at[3] == height,
            "{id} at y {y} with height {height}, as in Chrome: {text}"
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
fn offsets_leave_out_animated_transforms_like_chrome() {
    let text = run_hidden(
        r#"
        await load('t.html');
        await run("window.runs = [document.getElementById('a').animate([{ translate: '0 50px' }, { translate: '0 0' }], { duration: 4000 }), document.getElementById('b').animate([{ transform: 'translate(30px, 40px) rotate(10deg)' }, { transform: 'none' }], { duration: 4000 })]; 1");
        const out = [];
        for (let i = 0; i < 4; i++) {
            await win.webContents.capturePage({ x: 0, y: 0, width: 8, height: 8 });
            await new Promise(done => setTimeout(done, 40));
            out.push(await run("[...'ab'].map(id => { const e = document.getElementById(id); return [e.offsetLeft, e.offsetTop, Math.round(e.getBoundingClientRect().top) > e.offsetTop] }).join(' ')"));
        }
        console.log('OFFSETS ' + [...new Set(out)].join(' | '));
        "#,
        &[(
            "t.html",
            "<body style='margin:0;font:10px/12px Helvetica'><div style='position:relative;padding:3px'>\
             <b id=a style='display:block;height:20px'>x</b><b id=b style='display:block;height:20px'>x</b></div></body>",
        )],
    );
    let offsets = text
        .lines()
        .find_map(|line| line.strip_prefix("OFFSETS "))
        .unwrap_or_else(|| panic!("{text}"));
    // Chrome's values: the boxes are drawn lower while they animate, and their offsets stay their layout places.
    assert_eq!(offsets, "3,3,true 3,23,true");
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
fn static_places_of_out_of_flow_boxes_follow_chrome() {
    let text = run_hidden(
        r#"
        await load('static.html');
        const step = async (name, code) => console.log('STATIC ' + name + ' ' + await run(code));
        await step('end', "const d = document.documentElement; [d.scrollHeight, at(line), at(end)].join()");
        await step('scrolled', "scrollTo(0, 100); list.scrollTop = 30; [at(fixed), at(inline), at(menu), at(after), list.scrollHeight].join()");
        await win.webContents.capturePage();
        await step('frame', "[at(fixed), at(inline), at(menu)].join()");
        await step('moved', "first.style.height = '70px'; [at(fixed), at(inline), at(menu), d.scrollHeight].join()");
        await step('placed', "fixed.style.top = '5px'; const top = at(fixed); fixed.style.top = ''; [top, at(fixed)].join()");
        await load('lines.html');
        await step('line', "x() >= 0 && x() < 10");
        await win.webContents.capturePage();
        await step('row', "document.body.append(document.createElement('pre')); x() >= 0 && x() < 10");
        "#,
        &[
            (
                "static.html",
                "<body style='margin:0'><style>body{line-height:20px}.f{position:fixed}.a{position:absolute}.f,.a{width:10px;height:50px}</style>\
             <div id=first style='height:50px'></div><div class=f id=fixed></div><div>text <span class=f id=inline></span></div>\
             <div id=list style='overflow:auto;height:60px'><div style='height:40px'></div><div class=a id=menu></div><div style='height:80px'></div></div>\
             <div id=after style='height:800px'></div><div>a <span class=a id=line></span></div><div class=a id=end style='margin-top:30px'></div><script>\
             for (const id of ['first', 'fixed', 'inline', 'list', 'menu', 'after', 'line', 'end']) globalThis[id] = document.getElementById(id);\
             globalThis.at = box => Math.round(box.getBoundingClientRect().top);</script></body>",
            ),
            (
                "lines.html",
                "<body style='margin:0'><a id=a>text</a> <span id=box style='position:absolute;top:50px'>box</span><script>\
             globalThis.box = document.getElementById('box');\
             globalThis.x = () => box.getBoundingClientRect().left - document.getElementById('a').getBoundingClientRect().right;</script></body>",
            ),
        ],
    );
    for line in [
        "end 1030,930,980",
        "scrolled 50,50,10,30,120",
        "frame 50,50,10",
        "moved 70,70,10,1050",
        "placed 5,70",
        "line true",
        "row true",
    ] {
        assert!(text.contains(&format!("STATIC {line}\n")), "{line}: {text}");
    }
}
