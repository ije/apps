use super::*;

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
fn borders_that_the_interest_area_cut_draw_after_a_scroll_jump() {
    // Debug builds check each copied paint against a fresh one. Rows below a short window are not painted.
    run_hidden(
        r#"
        win.setContentSize(800, 600);
        await load('rows.html');
        await win.webContents.capturePage();
        for (const top of [510, 37, 1500, 1499]) {
            await run(`document.getElementById('s').scrollTop = ${top}`);
            await win.webContents.capturePage();
            await win.webContents.capturePage();
        }
        "#,
        &[(
            "rows.html",
            "<body style='margin:8px;font:12px/16px Helvetica'><style>\
             #s{width:150px;height:200px;overflow:auto;border:1px solid}\
             #s div{height:40px;border-bottom:1px solid #ccc;box-sizing:border-box}\
             #s div:nth-child(3n){border-top:2px solid #c66}</style><div id=s>\
             <div>0</div><div>1</div><div>2</div><div>3</div><div>4</div><div>5</div><div>6</div><div>7</div>\
             <div>8</div><div>9</div><div>10</div><div>11</div><div>12</div><div>13</div><div>14</div><div>15</div>\
             <div>16</div><div>17</div><div>18</div><div>19</div><div>20</div><div>21</div><div>22</div><div>23</div>\
             <div>24</div><div>25</div><div>26</div><div>27</div><div>28</div><div>29</div><div>30</div><div>31</div>\
             <div>32</div><div>33</div><div>34</div><div>35</div><div>36</div><div>37</div><div>38</div><div>39</div>\
             <div>40</div><div>41</div><div>42</div><div>43</div><div>44</div><div>45</div><div>46</div><div>47</div>\
             </div></body>",
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
fn z_indexed_boxes_keep_the_cuts_of_scrolls_between_them_and_their_context() {
    // The red boxes paint in the root context, cut by the viewports of the scrolls around them. A page scroll moves
    // the viewport of #a with them, and a scroll of #b moves #c but not the viewport of #b. Debug builds check each
    // shifted prepaint and copied paint against a fresh one.
    run_hidden(
        r#"
        const pixel = async (x, y) => [...(await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap()].slice(0, 3).join();
        const edge = async id => JSON.parse(await run(`(r => JSON.stringify([r.top, r.bottom]))(document.getElementById('${id}').getBoundingClientRect())`));
        await load('cuts.html');
        const seen = [];
        for (let step = 0; step < 12; step++) {
            await run('scrollBy(0, 7)');
            await win.webContents.capturePage();
            const [, bottom] = await edge('a');
            seen.push(await pixel(50, bottom - 3) + '/' + await pixel(50, bottom + 3));
        }
        await run('scrollTo(0, 0)');
        await win.webContents.capturePage();
        for (let step = 0; step < 12; step++) {
            await run("document.getElementById('b').scrollTop += 5");
            await win.webContents.capturePage();
            const [top] = await edge('b');
            seen.push(await pixel(250, top + 3) + '/' + await pixel(250, top - 3));
        }
        if (seen.some(pair => pair !== '0,0,255/255,255,255')) fail('cuts: ' + seen.join(' '));
        "#,
        &[(
            "cuts.html",
            "<body style='margin:0;height:3000px;background:#fff'><style>\
             .s{overflow:auto;scrollbar-width:none;width:150px;height:100px;background:#fff}\
             .r{position:absolute;left:0;top:-200px;width:100px;height:600px;background:#f00;z-index:1}</style>\
             <div id=a class=s style='position:absolute;top:110px;left:0'>\
             <div style='position:relative;height:300px'><div class=r></div></div></div>\
             <div id=b class=s style='position:absolute;top:60px;left:200px;height:150px'><div style='height:600px'>\
             <div id=c class=s><div style='position:relative;height:300px'><div class=r></div></div></div></div></div></body>",
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
fn emphasis_marks_draw_over_letters_and_grow_the_line_like_chrome() {
    run_hidden(
        r#"
        await load('marks.html');
        await win.webContents.capturePage();
        const bytes = (await win.webContents.capturePage({ x: 0, y: 0, width: 200, height: 200 })).toBitmap();
        // The capture has device pixels.
        const scale = Math.round(Math.sqrt(bytes.length / 160000));
        const pixel = (x, y) => [2, 1, 0].map(at => bytes[(y * scale * 200 * scale + x * scale) * 4 + at]);
        // Runs of marks in the row with the most of them, in a band of rows.
        const marks = (from, to, ink) => {
            let best = '';
            for (let y = from; y < to; y++) {
                const row = Array.from({ length: 200 }, (_, x) => ink(...pixel(x, y)) ? 'm' : 'w').join('').replace(/(.)\1*/g, '$1');
                if (row.length > best.length) best = row;
            }
            return best;
        };
        const red = (r, g, b) => r > 200 && g < 80 && b < 80, blue = (r, g, b) => b > 200 && r < 80 && g < 80;
        // Two letters with a space between them get two marks, over the text in a line that grew for them.
        if (marks(0, 20, red) !== 'wmwmw') fail('marks over: ' + marks(0, 20, red));
        if (marks(30, 60, red) !== 'w') fail('marks inside the line: ' + marks(30, 60, red));
        if (marks(100, 150, blue) !== 'wmwmw') fail('marks under: ' + marks(100, 150, blue));
        const height = await win.webContents.executeJavaScript('document.getElementById("a").getBoundingClientRect().height');
        if (height !== 59) fail('line height: ' + height);
        "#,
        &[(
            "marks.html",
            "<style>body{margin:0;background:#fff}div{font:40px/1 Helvetica;display:flow-root;padding-left:10px}</style>\
             <div id=a style='text-emphasis:circle #f00'>I I</div>\
             <div style='text-emphasis:circle #00f;text-emphasis-position:under'>I, I</div>",
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

// The colors are those of Chrome.
#[test]
#[ignore = "requires the macOS window server"]
fn shapes_mask_boxes_offset_paths_and_clip_margins_draw_like_chrome() {
    let page = "<body style='margin:0;width:400px;background:white'><style>div{width:100px;height:100px;box-sizing:border-box;\
        background:red;float:left}</style>\
        <div style='clip-path:shape(from 0 0,hline to 100%,vline by 50%,hline to 0,close)'></div>\
        <div style='clip-path:content-box;padding:20px'></div>\
        <div style='clip-path:xywh(0 0 50% 100%);filter:drop-shadow(40px 0 red)'></div>\
        <div id=m style='padding:20px;mask:linear-gradient(#000 0 0) content-box exclude,linear-gradient(#000 0 0)'></div>\
        <div style='background:none'><i style='display:block;width:20px;height:20px;background:red;\
        offset-path:ray(90deg);offset-distance:30px'></i></div>\
        <div style='background:none;overflow:clip;overflow-clip-margin:10px;width:50px;margin:0 50px 0 20px'>\
        <b style='display:block;width:200px;height:50px;background:red'></b></div></body>";
    let text = run_hidden(
        &format!(
            r#"{STEPS}
            await load('page.html');
            const row = async points => {{ const out = []; for (const [x, y] of points) out.push(await pixel(x, y) === '255,255,255' ? 'w' : 'r'); return out.join(''); }};
            console.log('PIXELS ' + await row([[50, 25], [50, 75], [150, 10], [150, 50], [240, 50], [260, 50], [350, 5], [350, 50], [80, 150], [50, 150], [115, 110], [125, 110], [175, 110], [185, 110]]));
            await step('padding', () => run("document.getElementById('m').style.padding = '2px'"));
            console.log('PIXELS ' + await row([[350, 5], [350, 1]]));
            "#
        ),
        &[("page.html", page)],
    );
    let pixels: Vec<_> = text
        .lines()
        .filter(|line| line.starts_with("PIXELS"))
        .collect();
    assert_eq!(pixels, ["PIXELS rwwrrwrwrwwrrw", "PIXELS wr"], "{text}");
}

#[test]
#[ignore = "requires the macOS window server"]
fn faded_inline_text_stays_in_its_lines() {
    let (text, rects) = picture_rects(
        "faded.html",
        "<body style='margin:0;font:20px/30px Helvetica'><style>.w{width:300px}</style>\
         <div class=w><p id=p style='margin:0'>C <span id=c style='opacity:.5;padding:0 20px'>wraps across many \
         lines of text here to show line pieces</span> end</p></div>\
         <div class=w><p style='margin:0'>E <span style='opacity:.4'>box <b id=b style='display:inline-block;\
         width:40px;height:20px'></b> after</span></p></div></body>",
    );
    for (id, expected) in [
        ("p", "0,0,300,90"),
        ("c", "0,3.5,281.2,53"),
        ("b", "56.7,2,40,20"),
    ] {
        let at: Vec<f32> = rects[id].split(',').map(|v| v.parse().unwrap()).collect();
        let want: Vec<f32> = expected.split(',').map(|v| v.parse().unwrap()).collect();
        assert!(
            at.iter().zip(&want).all(|(a, b)| (a - b).abs() < 0.6),
            "{id} at {expected}, as in Chrome: {text}"
        );
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn faded_inline_boxes_with_backgrounds_draw_as_groups_in_their_lines() {
    run_hidden(
        r#"
        await load('group.html');
        await win.webContents.capturePage();
        const rect = await run("(r => [r.x, r.y].map(Math.round).join())(document.getElementById('g').getBoundingClientRect())");
        if (rect !== '18,4') fail('the box left its line: ' + rect);
        const pixel = async (x, y) => [...(await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap()].slice(0, 3).reverse().join();
        const back = await pixel(21, 26);
        if (!/^12[78],12[78],255$/.test(back)) fail('the background is not half faded: ' + back);
        "#,
        &[(
            "group.html",
            "<body style='margin:0;font:20px/30px Helvetica'><p style='margin:0'>A <span id=g \
             style='opacity:.5;background:rgb(0,0,255);color:#000'>faded</span> end</p></body>",
        )],
    );
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
fn scroll_containers_snap_like_chrome() {
    let text = run_hidden(
        r#"
        await load('snap.html');
        console.log('SNAP ' + await run(`(() => {
            const box = document.getElementById('box'), out = [];
            for (const value of [41, 130, 999]) { box.scrollTop = value; out.push(box.scrollTop); }
            box.scrollTop = 80; box.scrollBy(0, 10); out.push(box.scrollTop);
            box.children[0].style.height = '120px'; out.push(box.scrollTop);
            box.style.scrollSnapType = 'none'; box.scrollTop = 50; out.push(box.scrollTop);
            box.style.scrollSnapType = ''; out.push(box.scrollTop);
            return out.join();
        })()`));
        "#,
        &[(
            "snap.html",
            "<body style='margin:0'><style>#box{width:200px;height:100px;overflow:auto;scroll-snap-type:y mandatory}#box>div{height:80px;scroll-snap-align:start}</style>\
             <div id=box><div></div><div></div><div></div><div></div><div></div><div></div><div></div><div></div></div></body>",
        )],
    );
    assert!(text.contains("SNAP 80,160,540,160,200,50,20"), "{text}");
}

#[test]
#[ignore = "requires the macOS window server"]
fn scroll_snaps_fire_events_and_reach_into_shadow_trees() {
    let text = run_hidden(
        r#"
        await load('snap.html');
        const step = async (name, code) => {
            await run(code + ';0');
            for (let i = 0; i < 4; i++) { await win.webContents.capturePage(); await run('new Promise(done => setTimeout(done, 20))'); }
            console.log('SNAP ' + name + ' ' + await run("log.splice(0).join(' ')"));
        };
        await step('load', '');
        await step('scroll', 'box.scrollTop = 50');
        await step('same', 'box.scrollTop = 90');
        await step('layout', "first.style.height = '100px'");
        await step('hide', "box.style.display = 'none'");
        await step('show', "box.style.display = ''");
        await step('off', "box.style.scrollSnapType = 'none'");
        await step('bars', "box.style.scrollSnapType = ''; bars.scrollTop = 110; log.splice(0); log.push(bars.scrollTop)");
        "#,
        &[(
            "snap.html",
            "<body style='margin:0'><style>.s{width:200px;height:100px;overflow:auto;scroll-snap-type:y mandatory}#bars{overflow:scroll;scroll-snap-type:y proximity}#bars::-webkit-scrollbar{width:20px;height:20px}#bars>div{height:60px;margin-bottom:80px;scroll-snap-align:end}</style>\
             <div class=s id=box><div id=host></div></div><div class=s id=bars><div id=b0></div><div id=b1></div><div id=b2></div></div><script>\
             globalThis.log = []; globalThis.box = document.getElementById('box'); globalThis.bars = document.getElementById('bars');\
             const root = document.getElementById('host').attachShadow({ mode: 'open' });\
             root.innerHTML = '<style>p{margin:0;height:80px;scroll-snap-align:start}</style>' + [0, 1, 2, 3].map(i => '<p id=p' + i + '></p>').join('');\
             globalThis.first = root.getElementById('p0');\
             for (const type of ['scrollsnapchanging', 'scrollsnapchange']) box['on' + type] = event => log.push(type.slice(10) + ':' + event.snapTargetBlock?.id + ',' + event.snapTargetInline + '@' + box.scrollTop);\
             </script></body>",
        )],
    );
    for line in [
        "load change:p0,null@0",
        "scroll changing:p1,null@80 change:p1,null@80",
        "same ",
        "layout ",
        "hide ",
        "show change:p1,null@100",
        "off change:undefined,null@100",
        "bars 120 change:p1,null@100",
    ] {
        assert!(
            text.lines()
                .any(|out| out.trim_end() == format!("SNAP {line}").trim_end()),
            "{line}: {text}"
        );
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn scroll_snaps_follow_chrome_in_events_focus_and_bars() {
    let text = run_hidden(
        r#"
        await load('snap.html');
        const step = async (name, code) => {
            await run(code + ';0');
            for (let i = 0; i < 4; i++) { await win.webContents.capturePage(); await run('new Promise(done => setTimeout(done, 20))'); }
            console.log('SNAP ' + name + ' ' + await run("log.splice(0).join(' ')"));
        };
        await step('load', 'log.length = 0; log.push(gap.scrollLeft, gutter.scrollLeft, gutter.clientWidth)');
        await step('order', 'b.scrollTop = 85; a.scrollTop = 85; b.scrollTop = 170');
        await step('anchor', "a.children[0].style.height = '120px'");
        await step('focus', "b.children[1].firstChild.focus({ preventScroll: true }); b.scrollTop = 0; b.scrollTop = 85; log.push(b.scrollTop)");
        await step('page', 'const d = document.documentElement; scrollTo(9999, 0); log.push(d.clientWidth - innerWidth, d.clientHeight - innerHeight, d.scrollWidth, scrollX + d.clientWidth)');
        "#,
        &[(
            "snap.html",
            "<body style='margin:0'><style>.s{width:200px;height:100px;overflow:auto;scroll-snap-type:y mandatory}.s::-webkit-scrollbar{display:none}.s>div{height:80px;scroll-snap-align:start}.s p{margin:0;height:80px;scroll-snap-align:start}\
             .x{width:120px;height:40px;overflow:hidden;scroll-snap-type:x mandatory}.x>div{width:500px;height:40px;padding-left:40px;box-sizing:border-box}.x p{margin:0;width:90px;height:40px;scroll-snap-align:start}\
             #gutter{overflow:auto;scrollbar-gutter:stable both-edges}#gutter>div{padding-left:20px}::-webkit-scrollbar{width:20px;height:14px}</style>\
             <div class=s id=a><div></div><div></div><div></div><div></div></div><div class=s id=b><div><p tabindex=0></p></div><div><p tabindex=0></p></div><div></div><div></div></div>\
             <div class=x id=gap><div><p></p></div></div><div class=x id=gutter><div><p></p></div></div><div style='width:1500px;height:10px'></div><script>\
             globalThis.log = [];for (const id of ['a', 'b', 'gap', 'gutter']) globalThis[id] = document.getElementById(id);\
             for (const box of [a, b]) for (const type of ['scrollsnapchanging', 'scrollsnapchange', 'scrollend']) box.addEventListener(type, event => log.push(box.id + ':' + type.replace('scrollsnap', '') + (event.snapTargetBlock ? [...box.children].indexOf(event.snapTargetBlock) : '')));\
             </script></body>",
        )],
    );
    for line in [
        "load 40 40 80",
        "order b:changing1 b:change1 b:scrollend a:changing1 a:change1 a:scrollend",
        "anchor ",
        "focus 80 b:changing0 b:change0 b:scrollend",
        "page -20 -14 1500 1500",
    ] {
        assert!(
            text.lines()
                .any(|out| out.trim_end() == format!("SNAP {line}").trim_end()),
            "{line}: {text}"
        );
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn page_bars_follow_chrome() {
    let text = run_hidden(
        r#"
        const step = async (page, name, code) => {
            await load(page);
            for (let i = 0; i < 2; i++) await win.webContents.capturePage();
            console.log('BARS ' + name + ' ' + await run(`(() => { const d = document.documentElement, at = id => { const r = document.getElementById(id).getBoundingClientRect(); return [r.x, r.y, r.right, r.bottom].map(Math.round); }; const size = () => [d.clientWidth - innerWidth, d.clientHeight - innerHeight, d.scrollWidth - innerWidth, d.scrollHeight - innerHeight]; return [${code}].flat().join(); })()`));
        };
        // The fixed box stops at the bars, also one that its right and bottom insets place.
        await step('auto.html', 'fixed', "at('all').map((v, i) => i < 2 ? v : v - [innerWidth, innerHeight][i - 2]), at('end')[2] - innerWidth, at('end')[3] - innerHeight");
        await step('scroll.html', 'scroll', "(scrollTo(0, 70), scrollY), (scrollTo(9999, 9999), [scrollX, scrollY].map((v, i) => v - (d[['scrollWidth', 'scrollHeight'][i]] - d[['clientWidth', 'clientHeight'][i]])))");
        await step('hidden.html', 'hidden', "size(), (scrollTo(50, 60), [scrollX, scrollY])");
        await step('both.html', 'both', "size().slice(0, 2), (m.style.height = '10px', d.scrollHeight, size()), (m.style.height = '3000px', d.scrollHeight, size().slice(0, 2))");
        await step('rtl.html', 'rtl', "(scrollTo(-9999, 0), [scrollX + d.scrollWidth - d.clientWidth]), (scrollTo(9999, 0), scrollX)");
        await step('ltr.html', 'ltr', "(scrollTo(50, 0), scrollX), (scrollTo(-50, 0), scrollX)");
        "#,
        &[
            (
                "auto.html",
                "<style>body{margin:0}::-webkit-scrollbar{width:20px;height:14px}</style><body><div id=all style='position:fixed;inset:0'></div><div id=end style='position:fixed;right:0;bottom:0;width:10px;height:10px'></div><div style='width:3000px;height:3000px'></div></body>",
            ),
            (
                "scroll.html",
                "<style>html{overflow-y:scroll}body{margin:0}::-webkit-scrollbar{width:20px;height:14px}</style><body><div style='width:3000px;height:3000px'></div></body>",
            ),
            (
                "hidden.html",
                "<style>html{overflow:hidden}body{margin:0}</style><body><div style='width:3000px;height:3000px'></div><script>document.querySelector('div').style.cssText = `width:${innerWidth + 100}px;height:${innerHeight + 200}px`</script></body>",
            ),
            (
                "both.html",
                "<style>body{margin:0}::-webkit-scrollbar{width:20px;height:14px}</style><body><div id=m></div><script>globalThis.m = document.getElementById('m'); m.style.cssText = `width:${innerWidth - 10}px;height:3000px`</script></body>",
            ),
            (
                "rtl.html",
                "<style>body{margin:0;direction:rtl}::-webkit-scrollbar{width:20px;height:14px}</style><body><div style='width:3000px;height:3000px'></div></body>",
            ),
            (
                "ltr.html",
                "<style>html{direction:rtl}body{margin:0;direction:ltr}</style><body><div style='width:3000px;height:3000px'></div></body>",
            ),
        ],
    );
    for line in [
        "fixed 0,0,-20,-14,-20,-14",
        "scroll 70,0,0",
        "hidden 0,0,100,200,50,60",
        "both -20,-14,0,0,0,0,-20,-14",
        "rtl 0,0",
        "ltr 50,0",
    ] {
        assert!(
            text.lines()
                .any(|out| out.trim_end() == format!("BARS {line}")),
            "{line}: {text}"
        );
    }
}

#[test]
#[ignore = "requires the macOS window server"]
fn boxes_in_3d_rendering_contexts_draw_and_hit_by_depth_like_chrome() {
    run_hidden(
        r#"
        await load('page.html');
        // BGRA to RGB.
        const pixel = async (x, y) => [...(await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap()].slice(0, 3).reverse().join();
        const look = async (name, x, y, color, id) => {
            const seen = await pixel(x, y), hit = await run(`document.elementFromPoint(${x}, ${y}).id`);
            if (seen !== color || id && hit !== id) fail(name + ': ' + seen + ' on ' + hit);
        };
        const [red, green] = ['255,0,0', '0,128,0'];
        await look('front', 50, 50, red, 'front');
        await run("card.className = 'turned'");
        await look('back', 50, 50, green, 'back');
        // The first box in the tree is nearest to the viewer.
        await look('near', 170, 50, red, 'near');
        await run("near.style.transform = 'translateZ(-30px)'");
        await look('far', 170, 50, green, 'far');
        // Two planes that go through each other.
        await look('left', 270, 50, red);
        await look('right', 330, 50, green);
        // `opacity` flattens the children, which then paint in tree order.
        await run("near.style.transform = 'translateZ(30px)', near.parentNode.style.opacity = 0.99");
        for (let n = 0; n < 2; n++) await win.webContents.capturePage();
        const hit = await run('document.elementFromPoint(170, 50).id');
        if (hit !== 'far') fail('flat: ' + hit);
        const rect = JSON.parse(await run("card.className = 'edge', JSON.stringify(back.getBoundingClientRect())"));
        if (Math.abs(rect.x - 50) > 0.1 || rect.width > 0.1) fail('edge: ' + JSON.stringify(rect));
        "#,
        &[(
            "page.html",
            "<style>body{margin:0;background:#fff}.s{position:absolute;top:0;width:100px;height:100px;transform-style:preserve-3d}\
             .s>div{position:absolute;inset:0}.turned{transform:rotateY(180deg)}.edge{transform:rotateY(90deg)}</style>\
             <div class=s style='left:0'><div id=card style='transform-style:preserve-3d'><div id=front style='position:absolute;inset:0;background:#f00;backface-visibility:hidden'></div>\
             <div id=back style='position:absolute;inset:0;background:#008000;backface-visibility:hidden;transform:rotateY(180deg)'></div></div></div>\
             <div class=s style='left:120px'><div id=near style='background:#f00;transform:translateZ(30px)'></div><div id=far style='background:#008000'></div></div>\
             <div class=s style='left:250px'><div id=one style='background:#f00;transform:rotateY(45deg)'></div><div id=two style='background:#008000;transform:rotateY(-45deg)'></div></div>\
             <script>const card = document.getElementById('card'), near = document.getElementById('near'), back = document.getElementById('back')</script></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn boxes_take_the_pointer_where_3d_transforms_put_them_like_chrome() {
    run_hidden(
        r#"
        await load('page.html');
        const hit = async (name, x, y, id) => {
            const found = await run(`(node => node && (node.id || node.localName))(document.elementFromPoint(${x}, ${y}))`);
            if (found !== id) fail(name + ': ' + found + ' instead of ' + id);
        };
        // A plane behind the viewer takes no pointer, and one that crosses the viewer only the part before it.
        await hit('behind', 50, 50, 'a');
        await hit('behind flat', 150, 50, 'b');
        await hit('crossing', 270, 95, 'near');
        await hit('crossing', 270, 120, 'near');
        await hit('crossing', 270, 30, 'html');
        // A transform without an inverse draws nothing and takes no pointer.
        const pixel = async (x, y) => [...(await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap()].slice(0, 3).join();
        for (const x of [320, 350, 380]) {
            await hit('singular', x, 30, 'html');
            if (await pixel(x, 30) !== '255,255,255') fail('singular at ' + x);
        }
        // A box that becomes inline, and a new node with the number of a removed box, lose the place in 3D.
        await run("turned.parentNode.id = 'line'");
        await hit('turned', 60, 220, 'line');
        await run("turned.style.cssText = ''");
        for (let n = 0; n < 2; n++) await win.webContents.capturePage();
        const [x, y] = JSON.parse(await run("(r => JSON.stringify([r.x + r.width / 2, r.y + r.height / 2]))(turned.getBoundingClientRect())"));
        await hit('inline', x, y, 'turned');
        await run("document.querySelectorAll('.s, #boxes').forEach(node => node.remove())");
        for (let n = 0; n < 10; n++) await win.webContents.capturePage();
        await run("words.innerHTML = Array.from({ length: 60 }, (_, at) => '<span id=w' + at + '>word</span> ').join('')");
        for (let n = 0; n < 2; n++) await win.webContents.capturePage();
        const missed = await run("[...words.children].filter(word => { const r = word.getBoundingClientRect(); return document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2) !== word }).length");
        if (missed) fail('new words missed: ' + missed);
        "#,
        &[(
            "page.html",
            "<style>body{margin:0;background:#fff}.s{position:absolute;top:0;width:100px;height:100px;perspective:100px;pointer-events:none}\
             .s *{position:absolute;inset:0;pointer-events:auto}.k{background:#f00}#boxes i{position:absolute;display:block;width:30px;height:20px;transform:rotateX(40deg);left:0;top:300px}</style>\
             <div id=a style='position:absolute;inset:0 auto auto 0;width:100px;height:100px'></div><div id=b style='position:absolute;left:100px;top:0;width:100px;height:100px'></div>\
             <div class=s style='left:0'><div id=p style='transform-style:preserve-3d;pointer-events:none'><div id=c style='transform:translateZ(200px)'></div></div></div>\
             <div class=s style='left:100px'><div id=d style='transform:translateZ(200px)'></div></div>\
             <div class=s style='left:220px;height:200px;overflow:hidden'><div id=near class=k style='transform:rotateX(80deg) scaleY(4)'></div></div>\
             <div class=s style='width:30px;left:310px'><div class=k style='transform:scale3d(1,1,0)'></div></div>\
             <div class=s style='width:30px;left:340px;perspective:none;transform-style:preserve-3d'><div class=k style='transform:scaleZ(0)'></div></div>\
             <div class=s style='width:30px;left:370px;perspective:none'><div class=k style='transform:scaleZ(0)'></div></div>\
             <p style='position:absolute;top:200px;margin:0'><a id=turned style='display:inline-block;padding:20px 60px;pointer-events:auto;transform:rotateY(180deg);backface-visibility:hidden'>turned</a></p>\
             <div id=boxes style='transform-style:preserve-3d;pointer-events:none'><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i></div>\
             <div id=words style='position:absolute;top:0;width:400px'></div>\
             <script>const turned = document.getElementById('turned'), words = document.getElementById('words')</script></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn positioned_boxes_paint_in_their_new_order() {
    run_hidden(
        r#"
        await load('page.html');
        for (let n = 0; n < 3; n++) await win.webContents.capturePage();
        await run("const box = document.getElementById('box'); box.append(box.firstChild)");
        await win.webContents.capturePage();
        const seen = [...(await win.webContents.capturePage({ x: 10, y: 10, width: 1, height: 1 })).toBitmap()].slice(0, 3).reverse().join();
        if (seen !== '255,0,0') fail(seen);
        "#,
        &[(
            "page.html",
            "<style>body{margin:0}#box{position:relative;height:90px}#box>div{position:absolute;inset:0;background:#f00}#box>.b{background:#00f}</style>\
             <div id=box><div>a</div><div class=b>b</div></div></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn turning_3d_contexts_draw_their_last_frame() {
    run_hidden(
        r#"
        await load('page.html');
        // BGRA to RGB.
        const pixel = async (x, y) => [...(await win.webContents.capturePage({ x, y, width: 1, height: 1 })).toBitmap()].slice(0, 3).reverse().join();
        await run("document.body.className = 'on'");
        for (let n = 0; n < 30; n++) await win.webContents.capturePage();
        await new Promise(resolve => setTimeout(resolve, 300));
        const seen = await pixel(50, 50);
        if (seen !== '0,128,0') fail(seen);
        "#,
        &[(
            "page.html",
            "<style>body{margin:0;background:#fff}#s{perspective:400px;width:100px}#card{height:100px;transform-style:preserve-3d;transition:transform .2s}\
             .on #card{transform:rotateY(180deg)}#card>div{position:absolute;inset:0;background:#f00;backface-visibility:hidden}\
             #card>.b{background:#008000;transform:rotateY(180deg)}i{display:block;width:9px;height:9px;animation:m .2s infinite}@keyframes m{to{translate:9px}}</style>\
             <div id=s><div id=card><div><i></i></div><div class=b></div></div></div></body>",
        )],
    );
}
