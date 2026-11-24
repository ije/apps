use super::*;

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
