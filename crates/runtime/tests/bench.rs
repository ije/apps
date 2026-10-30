use apps_runtime::{
    dom::Document,
    host::{Host, Page, Role},
    jsc::{Api, Engine},
};
use std::time::{Duration, Instant};

#[test]
#[ignore = "release performance measurement"]
fn binary_io() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("data.bin");
    let data: Vec<u8> = (0..8 * 1024 * 1024)
        .map(|index| (index % 251) as u8)
        .collect();
    std::fs::write(&path, data).unwrap();
    let engine = Engine::new(Api::load().unwrap(), Default::default(), Role::Main).unwrap();
    engine
        .eval(
            &format!("globalThis.path={}", serde_json::json!(path)),
            "bench",
        )
        .unwrap();
    engine
        .eval("__apps.fs.readFileSync(path)", "bench")
        .unwrap();
    let mut reads = Vec::new();
    for _ in 0..9 {
        let start = Instant::now();
        assert_eq!(
            engine
                .eval("__apps.fs.readFileSync(path).length", "bench")
                .unwrap(),
            "8388608"
        );
        reads.push(start.elapsed().as_secs_f64() * 1000.);
    }
    reads.sort_by(f64::total_cmp);
    println!("readFileSync_8MiB_median_ms={:.3}", reads[4]);
    engine
        .eval("globalThis.data=__apps.fs.readFileSync(path)", "bench")
        .unwrap();
    let mut rounds = Vec::new();
    for _ in 0..3 {
        let start = Instant::now();
        engine.eval(r#"globalThis.done=false;globalThis.failure='';(async()=>{const compressed=await new Response(new Blob([data]).stream().pipeThrough(new CompressionStream('gzip'))).arrayBuffer();const decoded=new Uint8Array(await new Response(new Blob([compressed]).stream().pipeThrough(new DecompressionStream('gzip'))).arrayBuffer());if(decoded.length!==data.length||decoded[1000]!==data[1000])throw Error('bad round trip');done=true})().catch(e=>failure=String(e))"#, "bench").unwrap();
        while engine.eval("done||failure", "bench").unwrap() == "" {
            assert!(start.elapsed() < Duration::from_secs(30));
            engine.tick().unwrap();
        }
        assert_eq!(engine.eval("failure", "bench").unwrap(), "");
        rounds.push(start.elapsed().as_secs_f64() * 1000.);
    }
    rounds.sort_by(f64::total_cmp);
    println!("gzip_round_trip_8MiB_median_ms={:.3}", rounds[1]);
}

#[test]
#[ignore = "release performance measurement; the baseline may time out"]
fn descendant_selector() {
    let doc = Document::parse(&format!(
        "{}<span></span>{}",
        "<div class='a'>".repeat(1000),
        "</div>".repeat(1000)
    ));
    let selector = format!(".missing {}span", ".a ".repeat(29));
    let leaf = doc
        .nodes
        .iter()
        .position(|node| node.tag == "span")
        .unwrap();
    let started = Instant::now();
    assert!(!doc.matches(leaf, &selector));
    println!(
        "descendant_30_parts_1000_nodes_ms={:.3}",
        started.elapsed().as_secs_f64() * 1000.
    );
}

fn page(html: &str, script: &str) -> String {
    let host = std::rc::Rc::new(std::cell::RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse(html),
            dirty: false,
        },
    );
    let engine = Engine::new(Api::load().unwrap(), host, Role::Renderer(1)).unwrap();
    engine.eval(script, "bench").unwrap()
}

#[test]
#[ignore = "release performance measurement"]
fn structural_selectors() {
    let html = format!(
        "<style>li:nth-child(odd){{color:red}} li:last-child{{color:blue}} li+li{{margin-top:1px}} li~li{{padding-top:1px}} li:nth-of-type(3n){{font-weight:700}}</style><ul>{}</ul>",
        "<li>x</li>".repeat(2000)
    );
    println!(
        "structural_2000_items_first_append_ms={}",
        page(
            &html,
            r#"
        const ul = document.querySelector('ul');
        let t = performance.now(); getComputedStyle(ul.lastChild).color; const first = performance.now() - t;
        t = performance.now();
        for (let i = 0; i < 20; i++) { const li = document.createElement('li'); ul.append(li); getComputedStyle(li).color; }
        [first.toFixed(1), ((performance.now() - t) / 20).toFixed(1)].join()
    "#
        )
    );
}

#[test]
#[ignore = "release performance measurement"]
fn table_columns() {
    let html = format!(
        "<table>{}</table>",
        format!("<tr>{}</tr>", "<td>x</td>".repeat(6)).repeat(1000)
    );
    println!(
        "table_1000x6_first_append_ms={}",
        page(
            &html,
            r#"
        const table = document.querySelector('table');
        let t = performance.now(); getComputedStyle(table).gridTemplateColumns; const first = performance.now() - t;
        t = performance.now();
        for (let i = 0; i < 20; i++) { const row = document.createElement('tr'); for (let j = 0; j < 6; j++) row.append(document.createElement('td')); table.append(row); getComputedStyle(row.lastChild).color; }
        if (getComputedStyle(table).gridTemplateColumns !== 'repeat(6,auto)') throw Error(getComputedStyle(table).gridTemplateColumns);
        [first.toFixed(1), ((performance.now() - t) / 20).toFixed(1)].join()
    "#
        )
    );
}

#[test]
#[ignore = "release performance measurement"]
fn child_access() {
    let html = format!("<div id='box'>{}</div>", "<p>x</p>".repeat(5000));
    println!(
        "children_5000_access_remove_ms={}",
        page(
            &html,
            r#"
        const box = document.getElementById('box'), last = box.lastChild;
        let t = performance.now();
        for (let i = 0; i < 5000; i++) if (!box.firstChild || !box.hasChildNodes() || last.parentNode !== box) throw Error('access');
        const access = performance.now() - t;
        t = performance.now();
        while (box.firstChild) box.removeChild(box.firstChild);
        const remove = performance.now() - t;
        [access.toFixed(1), remove.toFixed(1)].join()
    "#
        )
    );
}

#[test]
#[ignore = "release performance measurement"]
fn release_sweep() {
    let host = std::rc::Rc::new(std::cell::RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse(&format!(
                "<div id='list'>{}</div>{}",
                "<p>x</p>".repeat(20000),
                "<x-h></x-h>".repeat(1000)
            )),
            dirty: false,
        },
    );
    let engine = Engine::new(Api::load().unwrap(), host, Role::Renderer(1)).unwrap();
    engine.eval("for (const h of document.querySelectorAll('x-h')) h.attachShadow({mode:'open'}).innerHTML = '<b>y</b>'; globalThis.kept = Array.from({length: 2000}, () => document.createElement('i')); __appsDomSweep(true);", "bench").unwrap();
    let started = Instant::now();
    engine
        .eval(
            "for (let i = 0; i < 100; i++) __appsDomSweep(true)",
            "bench",
        )
        .unwrap();
    let idle = started.elapsed().as_secs_f64() * 10.;
    engine
        .eval("document.getElementById('list').textContent = ''", "bench")
        .unwrap();
    engine.collect_garbage();
    let started = Instant::now();
    engine.eval("__appsDomSweep(true)", "bench").unwrap();
    println!(
        "release_idle_sweep_ms={idle:.3} release_free_40k_ms={:.3}",
        started.elapsed().as_secs_f64() * 1000.
    );
}

#[test]
#[ignore = "release performance measurement"]
fn shadow_restyle_and_hover() {
    let html = format!(
        "<style>.row{{padding:1px}}</style><main>{}</main>{}",
        "<div class='row'><span>x</span></div>".repeat(2000),
        "<x-card>light</x-card>".repeat(300)
    );
    println!(
        "shadow_300_hosts_restyle_hover_ms={}",
        page(
            &html,
            r#"
        for (const card of document.querySelectorAll('x-card')) card.attachShadow({mode:'open'}).innerHTML = '<style>.title{color:red}</style><div class="title"><slot></slot></div>';
        const card = document.querySelector('x-card'), row = document.querySelector('.row span');
        let t = performance.now(); getComputedStyle(card).color; const first = performance.now() - t;
        t = performance.now();
        for (let i = 0; i < 20; i++) { card.classList.toggle('on'); getComputedStyle(card).color; }
        const restyle = (performance.now() - t) / 20;
        t = performance.now();
        for (let i = 0; i < 20; i++) { __appsDispatch('pointermove', [i % 2 ? row._id : card._id], {pointerId: 1, pointerType: 'mouse', clientX: 0, clientY: 0}); getComputedStyle(row).color; }
        [first.toFixed(1), restyle.toFixed(1), ((performance.now() - t) / 20).toFixed(1)].join()
    "#
        )
    );
}

#[test]
#[ignore = "release performance measurement"]
fn host_calls() {
    let host: std::rc::Rc<std::cell::RefCell<apps_runtime::host::Host>> = Default::default();
    host.borrow_mut().pages.insert(
        1,
        apps_runtime::host::Page {
            doc: Document::parse("<body><p id=a class='x y'>hi</p></body>"),
            dirty: false,
        },
    );
    let engine = Engine::new(Api::load().unwrap(), host, Role::Renderer(1)).unwrap();
    let source =
        "for(let i=0;i<100000;i++){p.getAttribute('class');p.setAttribute('data-i',String(i&7))}";
    engine
        .eval("globalThis.p=document.getElementById('a')", "bench")
        .unwrap();
    engine.eval(source, "bench").unwrap();
    let started = Instant::now();
    engine.eval(source, "bench").unwrap();
    println!(
        "dom_attribute_200k_host_calls_ms={:.1}",
        started.elapsed().as_secs_f64() * 1000.
    );
}

#[test]
#[ignore = "release performance measurement"]
fn dom_updates() {
    let host: std::rc::Rc<std::cell::RefCell<Host>> = Default::default();
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<body><main></main><section></section></body>"),
            dirty: false,
        },
    );
    let engine = Engine::new(Api::load().unwrap(), host, Role::Renderer(1)).unwrap();
    engine.eval("globalThis.main = document.querySelector('main'); globalThis.box = document.querySelector('section'); globalThis.items = []; globalThis.k = 0; for (let i = 0; i < 1000; i++) { const d = document.createElement('div'); d.className = 'item'; d.id = 'i' + i; d.append('x'); main.append(d); items.push(d); }", "bench").unwrap();
    let time = |source: &str| {
        let mut rounds: Vec<f64> = (0..9)
            .map(|_| {
                let started = Instant::now();
                engine.eval(&format!("{{{source}}}"), "bench").unwrap();
                started.elapsed().as_secs_f64() * 1000.
            })
            .collect();
        rounds.sort_by(f64::total_cmp);
        rounds[4]
    };
    println!(
        "dom_reads_80k_ms={:.2} dom_toggle_1000_ms={:.2} dom_table_1000_rows_ms={:.2} dom_update_1000_ms={:.2}",
        time(
            "let n = 0; for (let r = 0; r < 20; r++) for (const d of items) if (d.getAttribute('class') && d.id && d.parentNode === main && d.firstChild) n++; if (n !== 20000) throw Error(n)"
        ),
        time("for (const d of items) d.classList.toggle('on')"),
        time(
            "const t = document.createElement('table'); for (let i = 0; i < 1000; i++) { const tr = document.createElement('tr'); for (let j = 0; j < 3; j++) { const td = document.createElement('td'); td.textContent = 'c' + i + ':' + j; tr.appendChild(td); } t.appendChild(tr); } box.replaceChildren(t)"
        ),
        time(
            "k++; for (let i = 0; i < 1000; i++) { const d = items[i]; d.firstChild.data = String(i + k); d.setAttribute('data-v', k); d.className = (i + k) % 2 ? 'item odd' : 'item'; }"
        ),
    );
}

#[test]
#[ignore = "release performance measurement"]
fn sibling_walk() {
    let html = format!("<div id='box'>{}</div>", "<p>x</p>".repeat(5000));
    println!(
        "siblings_5000_forward_backward_nested_ms={}",
        page(
            &html,
            r#"
        const box = document.getElementById('box');
        for (let n = box.firstChild; n; n = n.nextSibling) n.firstChild;
        let t = performance.now(), count = 0;
        for (let n = box.firstChild; n; n = n.nextSibling) count++;
        const forward = performance.now() - t;
        t = performance.now();
        for (let n = box.lastChild; n; n = n.previousSibling) count++;
        const backward = performance.now() - t;
        t = performance.now();
        for (let n = box.firstChild; n; n = n.nextSibling) if (!n.firstChild.nextSibling) count++;
        if (count !== 15000) throw Error(count);
        [forward.toFixed(1), backward.toFixed(1), (performance.now() - t).toFixed(1)].join()
    "#
        )
    );
}

#[test]
#[ignore = "release performance measurement"]
fn distinct_styles() {
    let html = format!(
        "<main>{}</main>",
        (0..5000)
            .map(|i| format!("<p style='width:{i}px'>x</p>"))
            .collect::<String>()
    );
    println!(
        "distinct_5000_styles_first_restyle_ms={}",
        page(
            &html,
            r#"
        const p = document.querySelector('p');
        let t = performance.now(); getComputedStyle(p).width; const first = performance.now() - t;
        t = performance.now();
        for (let i = 0; i < 20; i++) { p.classList.toggle('on'); getComputedStyle(p).width; }
        [first.toFixed(1), ((performance.now() - t) / 20).toFixed(1)].join()
    "#
        )
    );
}

#[test]
#[ignore = "release performance measurement"]
fn image_decode() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("large.jpg");
    image::RgbImage::from_fn(6000, 4000, |x, y| {
        image::Rgb([x as u8, y as u8, (x ^ y) as u8])
    })
    .save(&path)
    .unwrap();
    let host = std::rc::Rc::new(std::cell::RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<body></body>"),
            dirty: false,
        },
    );
    host.borrow_mut().dom.entry(1).or_default().base =
        Some(url::Url::from_directory_path(directory.path()).unwrap());
    let engine = Engine::new(Api::load().unwrap(), host, Role::Renderer(1)).unwrap();
    let source = url::Url::from_file_path(&path).unwrap();
    let mut rounds = Vec::new();
    for _ in 0..3 {
        let started = Instant::now();
        engine.eval(&format!("globalThis.done = ''; globalThis.image = new Image(); image.addEventListener('load', () => done = 'load'); image.addEventListener('error', () => done = 'error'); image.src = {}; done", serde_json::json!(source.as_str())), "bench").unwrap();
        let blocked = started.elapsed().as_secs_f64() * 1000.;
        while engine.eval("done", "bench").unwrap().is_empty() {
            assert!(started.elapsed() < Duration::from_secs(30));
            engine.tick().unwrap();
        }
        assert_eq!(engine.eval("done", "bench").unwrap(), "load");
        rounds.push([blocked, started.elapsed().as_secs_f64() * 1000.]);
    }
    rounds.sort_by(|a, b| a[1].total_cmp(&b[1]));
    println!(
        "image_24mp_jpeg_blocked_ms={:.1} loaded_ms={:.1}",
        rounds[1][0], rounds[1][1]
    );
}

#[test]
#[ignore = "release performance measurement; requires a native GPU adapter"]
fn webgpu_present() {
    let host: std::rc::Rc<std::cell::RefCell<Host>> = Default::default();
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<canvas id=c width=1920 height=1080></canvas>"),
            dirty: false,
        },
    );
    let engine = Engine::new(Api::load().unwrap(), host.clone(), Role::Renderer(1)).unwrap();
    engine.eval(r#"(async()=>{const device=await (await navigator.gpu.requestAdapter()).requestDevice();const context=document.getElementById('c').getContext('webgpu');context.configure({device,format:'bgra8unorm'});
      const module=device.createShaderModule({code:`@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f { return vec4f(f32(i % 2u) * 4 - 1, f32(i / 2u) * 4 - 1, 0, 1); }
        @fragment fn fs(@builtin(position) p: vec4f) -> @location(0) vec4f { var v = p.x * 0.001; for (var i = 0; i < 1500; i++) { v = fract(sin(v * 12.9898 + p.y) * 43758.5453); } return vec4f(v, 0, 1, 1); }`});
      const pipeline=device.createRenderPipeline({layout:'auto',vertex:{module,entryPoint:'vs'},fragment:{module,entryPoint:'fs',targets:[{format:'bgra8unorm'}]}});
      globalThis.frame=(i,heavy)=>{const e=device.createCommandEncoder();const pass=e.beginRenderPass({colorAttachments:[{view:context.getCurrentTexture().createView(),clearValue:[i%2,0,1,1],loadOp:'clear',storeOp:'store'}]});if(heavy){pass.setPipeline(pipeline);pass.draw(3)}pass.end();device.queue.submit([e.finish()])}})()"#, "bench").unwrap();
    while engine.eval("typeof frame", "bench").unwrap() != "function" {
        engine.tick().unwrap();
    }
    let node = host.borrow().pages[&1]
        .doc
        .nodes
        .iter()
        .position(|n| n.tag == "canvas")
        .unwrap();
    let mut last = None;
    for heavy in [false, true] {
        let (mut busy, mut stalls, mut frames) = (Vec::new(), Vec::new(), Vec::new());
        for i in 0..70 {
            let started = Instant::now();
            engine
                .eval(&format!("frame({i},{heavy})"), "bench")
                .unwrap();
            let (mut spent, mut stall) = (Duration::ZERO, Duration::ZERO);
            loop {
                let call = Instant::now();
                let image = host
                    .borrow_mut()
                    .graphics
                    .get_mut(&1)
                    .unwrap()
                    .image(node)
                    .unwrap();
                spent += call.elapsed();
                stall = stall.max(call.elapsed());
                if let Some(image) = image.filter(|image| Some(image.id) != last) {
                    last = Some(image.id);
                    break;
                }
                std::thread::sleep(Duration::from_micros(100));
            }
            if i >= 10 {
                busy.push(spent.as_secs_f64() * 1000.);
                stalls.push(stall.as_secs_f64() * 1000.);
                frames.push(started.elapsed().as_secs_f64() * 1000.);
            }
        }
        for list in [&mut busy, &mut stalls, &mut frames] {
            list.sort_by(f64::total_cmp);
        }
        println!(
            "webgpu_present_1080p_{} ui_busy_median_ms={:.3} longest_call_median_ms={:.3} longest_call_max_ms={:.3} frame_latency_median_ms={:.3}",
            if heavy { "heavy" } else { "clear" },
            busy[30],
            stalls[30],
            stalls[59],
            frames[30]
        );
    }
}

fn median(engine: &Engine, source: &str) -> f64 {
    engine.eval(&format!("{{{source}}}"), "bench").unwrap();
    let mut rounds: Vec<f64> = (0..7)
        .map(|_| {
            let started = Instant::now();
            engine.eval(&format!("{{{source}}}"), "bench").unwrap();
            started.elapsed().as_secs_f64() * 1000.
        })
        .collect();
    rounds.sort_by(f64::total_cmp);
    rounds[3]
}

#[test]
#[ignore = "release performance measurement"]
fn bridge_calls() {
    let directory = tempfile::tempdir().unwrap();
    let engine = Engine::new(Api::load().unwrap(), Default::default(), Role::Main).unwrap();
    engine.eval(&format!("globalThis.file={};globalThis.db=new __apps.sqlite.DatabaseSync(':memory:');db.exec('CREATE TABLE t(a INTEGER, b TEXT)');globalThis.insert=db.prepare('INSERT INTO t VALUES (?, ?)');", serde_json::json!(directory.path().join("x.txt"))), "bench").unwrap();
    engine
        .eval("__apps.fs.writeFileSync(file, 'x')", "bench")
        .unwrap();
    println!(
        "native_now_100k_ms={:.1}",
        median(
            &engine,
            "for (let i = 0; i < 100000; i++) performance.now()"
        )
    );
    println!(
        "main_primitive_100k_ms={:.1} sqlite_insert_200k_ms={:.1} fs_stat_20k_ms={:.1} fs_exists_20k_ms={:.1} path_join_100k_ms={:.1}",
        median(&engine, "for (let i = 0; i < 100000; i++) db.isTransaction"),
        median(
            &engine,
            "db.exec('BEGIN'); for (let i = 0; i < 200000; i++) insert.run(i, 'name' + i); db.exec('COMMIT')"
        ),
        median(
            &engine,
            "for (let i = 0; i < 20000; i++) __apps.fs.statSync(file)"
        ),
        median(
            &engine,
            "for (let i = 0; i < 20000; i++) __apps.fs.existsSync(file)"
        ),
        median(
            &engine,
            "for (let i = 0; i < 100000; i++) __apps.path.join('/a/b', 'c' + (i & 7), '../d')"
        ),
    );
}

#[test]
#[ignore = "release performance measurement"]
fn layout_reads() {
    let sections = 20;
    let html = format!(
        "<html><body><main style='overflow:auto'>{}</main></body></html>",
        format!(
            "<section style='overflow:hidden'><div>{}</div></section>",
            "<div><p class='t'>x</p></div>".repeat(100)
        )
        .repeat(sections)
    );
    let host: std::rc::Rc<std::cell::RefCell<Host>> = Default::default();
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse(&html),
            dirty: false,
        },
    );
    {
        let mut host = host.borrow_mut();
        let bounds: Vec<_> = (0..host.pages[&1].doc.nodes.len())
            .map(|id| (id, [0., (id * 7 % 3000) as f32, 300., 20.]))
            .collect();
        host.dom.entry(1).or_default().bounds.extend(bounds);
    }
    let engine = Engine::new(Api::load().unwrap(), host, Role::Renderer(1)).unwrap();
    engine.eval("globalThis.targets = [...document.querySelectorAll('.t')]; globalThis.p = targets[0]; if (targets.length !== 2000) throw Error('targets');", "bench").unwrap();
    let rect = median(
        &engine,
        "let s = 0; for (let i = 0; i < 100000; i++) s += p.getBoundingClientRect().top",
    );
    let scroll = median(
        &engine,
        "let s = 0; for (let i = 0; i < 100000; i++) s += p.scrollTop + p.clientWidth",
    );
    let offset = median(
        &engine,
        "let s = 0; for (let i = 0; i < 10000; i++) s += p.offsetTop",
    );
    engine.eval("globalThis.seen = 0; globalThis.io = new IntersectionObserver(entries => seen += entries.length, { threshold: [0, 0.5, 1] }); for (const t of targets) io.observe(t); __appsLayout();", "bench").unwrap();
    let intersection = median(&engine, "for (let i = 0; i < 10; i++) __appsLayout()") / 10.;
    engine.eval("io.disconnect(); globalThis.ro = new ResizeObserver(() => {}); for (const t of targets) ro.observe(t); __appsLayout();", "bench").unwrap();
    let resize = median(&engine, "for (let i = 0; i < 10; i++) __appsLayout()") / 10.;
    println!(
        "rect_100k_ms={rect:.1} scroll_client_100k_ms={scroll:.1} offset_top_10k_ms={offset:.1} intersection_2000_layout_ms={intersection:.2} resize_2000_layout_ms={resize:.2}"
    );
}
