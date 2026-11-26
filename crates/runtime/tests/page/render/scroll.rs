use super::*;

#[test]
#[ignore = "requires the macOS window server"]
fn absolute_boxes_keep_their_static_place_in_the_line_and_out_of_the_scroll_range() {
    run_hidden(
        r#"
        await load('page.html');
        const box = await run(`(r => [r.left, r.top].map(Math.round).join())(document.getElementById('i').getBoundingClientRect())`);
        const end = await run(`Math.round((g => (g.selectNodeContents(document.getElementById('p').firstChild), g.getClientRects()[1].right))(document.createRange()))`);
        if (box !== end + ',20') fail('the box at the top of the second line, after its text: ' + box + ' text end ' + end);
        const height = await run(`document.getElementById('p').getBoundingClientRect().height`);
        if (height !== 40) fail('the paragraph stays two lines: ' + height);
        const left = await run(`parseFloat(getComputedStyle(document.getElementById('q'), '::after').left)`);
        if (!(left > 15 && left < 40)) fail('an ::after after its text: ' + left);
        const width = await run(`document.documentElement.scrollWidth`);
        if (width !== 400) fail('a box placed in another block scrolls from where it is: ' + width);
        "#,
        &[(
            "page.html",
            "<head><style>body{margin:0;font:16px Helvetica} #p{width:120px;line-height:20px} #p i{position:absolute;width:10px;height:10px}\
             #q{position:relative;width:200px} #q::after{content:'';position:absolute;top:0;width:10px;height:10px}\
             #w{margin-left:100px} #w>u{position:absolute;left:300px;width:30px;height:30px}</style></head>\
             <body><div id=p>one two three four<i id=i></i> five</div><div id=q>abc</div><div id=w><u></u>x</div></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn scroll_anchors_keep_the_content_in_view() {
    run_hidden(
        r#"
        await load('page.html');
        const step = async code => { const out = await run(code); await win.webContents.capturePage(); return out; };
        const read = () => run(`[...'abcde'].map(id => document.getElementById(id).scrollTop).join()`);
        await step(`for (const id of 'abcd') document.getElementById(id).scrollTop = 60`);
        const sync = await step(`for (const id of 'abcde') document.getElementById(id + '1').style.height = '30px';
            document.getElementById('c3').style.position = 'relative'; document.getElementById('a').offsetTop;
            document.getElementById('a').scrollTop`);
        if (sync !== 70) fail('a layout for a script: ' + sync);
        const grown = await read();
        if (grown !== '70,60,60,70,0') fail('grown: ' + grown);
        await step(`for (const id of 'abcd') document.getElementById(id + '2').style.height = '0'`);
        const shrunk = await read();
        if (shrunk !== '50,60,60,50,0') fail('shrunk: ' + shrunk);
        await step(`document.getElementById('a').scrollTop = 0; document.getElementById('a3').style.height = '50px'`);
        const top = await read();
        if (top !== '0,60,60,50,0') fail('at the top: ' + top);
        await step('scrollTo(0, 300)');
        const page = await step(`document.getElementById('p1').style.height = '30px'; scrollY`);
        if (page !== 310) fail('the page: ' + page);
        // As in Chrome, a move for an anchor fires a scroll event.
        const events = await run(`events.join()`);
        if (!events.startsWith('a60,a70,') || !events.endsWith(',p300,p310')) fail('scroll events: ' + events);
        "#,
        &[(
            "page.html",
            "<head><style>.s{display:inline-block;width:60px;height:100px;overflow:auto}\
             .s>div,#p>div{height:20px}</style></head><body style='margin:0'>\
             <div id=a class=s></div><div id=b class=s style='overflow-anchor:none'></div><div id=c class=s></div>\
             <div id=d class=s></div><div id=e class=s></div><div id=p></div>\
             <script>for (const id of 'abcdep') { const box = document.getElementById(id);\
             for (let i = 0; i < 40; i++) box.append(Object.assign(document.createElement('div'), { id: id + i, textContent: i })); }\
             document.getElementById('d3').style.overflowAnchor = 'none';\
             globalThis.events = []; document.getElementById('a').addEventListener('scroll', e => events.push('a' + e.target.scrollTop));\
             addEventListener('scroll', () => events.push('p' + scrollY));</script></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn pages_scroll_sideways_from_their_start() {
    run_hidden(
        r#"
        await load('wide.html');
        const read = () => run(`[scrollX, document.scrollingElement.scrollWidth - innerWidth, document.body.scrollWidth].join()`);
        const step = async code => { await run(code); await win.webContents.capturePage(); return read(); };
        const ltr = [await read(), await step('scrollTo(9999, 0)'), await step('scrollTo(-9999, 0)')].join(' ');
        const m = 1500 - (await run('innerWidth'));
        if (ltr !== `0,${m},1500 ${m},${m},1500 0,${m},1500`) fail('ltr: ' + ltr);
        const rtl = [
            await step("document.documentElement.dir = 'rtl'"),
            await step('scrollTo(-9999, 0)'),
            await step('scrollTo(9999, 0)'),
            await step("document.getElementById('w').scrollIntoView({ inline: 'end' })"),
        ].join(' ');
        if (rtl !== `0,${m},1500 -${m},${m},1500 0,${m},1500 -${m},${m},1500`) fail('rtl: ' + rtl);
        "#,
        &[(
            "wide.html",
            "<body style='margin:0'><div id=w style='width:1500px;height:40px'></div></body>",
        )],
    );
}

#[test]
#[ignore = "requires the macOS window server"]
fn blocks_far_from_the_view_move_with_a_sideways_scroll() {
    run_hidden(
        r#"
        await load('page.html');
        const read = () => run(`[0, 15, 20, 39].map(i => Math.round(document.getElementById('r' + i).getBoundingClientRect().left)).join()`);
        const got = [await read()];
        for (const [key, value] of [['scrollLeft', 100], ['scrollLeft', 30], ['scrollTop', 300]]) {
            await run(`document.getElementById('b')['${key}'] = ${value}`);
            got.push(await read());
            await win.webContents.capturePage();
            got.push(await read());
        }
        const want = '9,9,9,9 -91,-91,-91,-91 -91,-91,-91,-91 -21,-21,-21,-21 -21,-21,-21,-21 -21,-21,-21,-21 -21,-21,-21,-21';
        if (got.join(' ') !== want) fail(got.join(' '));
        "#,
        &[(
            "page.html",
            "<body style='margin:0'><div id=b style='width:150px;height:200px;overflow:auto;border:1px solid;margin:8px'>\
             <div style='width:600px;height:20px'></div></div>\
             <script type=module>const b = document.getElementById('b');\
             for (let i = 0; i < 40; i++) b.append(Object.assign(document.createElement('div'), { id: 'r' + i, style: 'height:40px' }));</script></body>",
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
        await expect('outer scroll hit', hit(170, 25), 'abs');
        await expect('inner scroll', "B.scrollTop = 20;" + rect('abs'), '152,2,50');
        await win.webContents.capturePage();
        await expect('inner scroll hit', hit(170, 25), 'abs');
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

// The elements are those of Chrome.
#[test]
#[ignore = "requires the macOS window server"]
fn hits_follow_clips_in_transforms_and_containment_like_chrome() {
    run_hidden(
        r#"
        const expect = async (step, code, value) => {
            const got = await run(code);
            if (got !== value) fail(step + ': ' + got + ' instead of ' + value);
        };
        const hits = points => `${JSON.stringify(points)}.map(([x, y]) => (e => e.id || e.localName)(document.elementFromPoint(x, y))).join()`;
        await load('hits.html');
        await expect('contained', hits([[105, 40], [140, 40], [280, 40], [340, 40], [130, 160]]), 'a,sib,div,html,html');
        await expect('scaled', hits([[205, 135], [212, 142], [255, 160], [261, 155]]), 'sc,btn,btn,sc');
        await expect('layer', hits([[15, 240], [40, 265], [65, 240], [75, 265]]), 'in,in,in,html');
        await expect('margin', hits([[155, 265], [195, 265], [211, 265]]), 'out,out,html');
        "#,
        &[(
            "hits.html",
            "<body style='margin:0'><style>.c{position:absolute;contain:paint;width:100px;height:100px}.c>a{position:absolute;\
             left:80px;top:20px;width:100px;height:40px}</style>\
             <div class=c style='left:10px;top:10px'><a id=a></a></div><div class=c id=sib style='left:130px;top:10px;contain:none'></div>\
             <div class=c style='left:230px;top:10px;contain:strict'><a style='position:fixed'></a></div>\
             <div class=c style='left:10px;top:130px;contain:content;overflow:auto'><a></a></div>\
             <div style='position:absolute;left:200px;top:130px;transform:scale(.5);transform-origin:0 0'>\
             <div style='width:100px;height:100px;border:20px solid;overflow:auto' id=sc>\
             <button id=btn style='width:100px;height:60px;display:block'>x</button></div></div>\
             <div style='position:absolute;left:10px;top:235px;width:40px;height:40px;border:10px solid;overflow:hidden'>\
             <b id=in style='display:block;width:100px;height:100px;margin:-20px'></b></div>\
             <div style='position:absolute;left:160px;top:235px'><div style='width:40px;height:40px;overflow:clip;overflow-clip-margin:10px'>\
             <b id=out style='display:block;width:100px;height:100px;margin:-20px'></b></div></div></body>",
        )],
    );
}
