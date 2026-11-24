use super::*;

#[test]
fn window_scroll_moves_the_page() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow().store.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<html><body><main></main></body></html>"),
            dirty: false,
            ..Default::default()
        },
    );
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    engine.eval(r#"
        const received = [];
        addEventListener('scroll', e => received.push([e.target === document, e.bubbles, scrollX, scrollY]));
        scrollTo(0, 40); scrollBy({top: 10});
        if (scrollY !== 50 || pageYOffset !== 50 || document.scrollingElement.scrollTop !== 50) throw Error('Page scroll was not set');
        if (document.querySelector('main').scrollTop !== 0) throw Error('Page scroll moved an element');
        __appsLayout();
        if (JSON.stringify(received) !== '[[true,true,0,50]]') throw Error(JSON.stringify(received));
        if (document.documentElement.clientHeight !== innerHeight) throw Error('Root client height is not the viewport');
        if (!CSS.supports('scroll-behavior', 'smooth')) throw Error('scroll-behavior is not supported');
        document.documentElement.style.scrollBehavior = 'smooth';
        if (getComputedStyle(document.documentElement).scrollBehavior !== 'smooth') throw Error('scroll-behavior is not computed');
        scrollTo({top: 70, behavior: 'smooth'}); scrollBy(0, 5);
        if (scrollY !== 75) throw Error('A page without a scroll range does not scroll at once: ' + scrollY);
        document.documentElement.style.scrollBehavior = '';
        scrollTo(0, 50);
    "#, "test").unwrap();
    host.borrow().store.borrow().pages[&1].state.geometry.scroll[&0]
        .set_offset(renderer::point(renderer::px(0.), renderer::px(-120.)));
    engine
        .eval(
            "__appsLayout(); if (scrollY !== 120) throw Error(String(scrollY));",
            "test",
        )
        .unwrap();
}

#[test]
fn hit_testing_follows_paint_order_clipping_and_shadow_trees() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow().store.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><body><div id='back'></div><div id='front'></div><div id='clip' style='overflow:hidden'><span id='child'></span></div><test-hit></test-hit></body></html>"),
        dirty: false, ..Default::default()
    });
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    engine.eval("const shadowHost=document.querySelector('test-hit');shadowHost.attachShadow({mode:'open'}).innerHTML='<span id=\"inside\"></span>';", "test").unwrap();
    {
        let host = host.borrow().store.clone();
        let mut host = host.borrow_mut();
        let bounds: Vec<_> = host.pages[&1]
            .doc
            .nodes
            .iter()
            .enumerate()
            .map(|(id, node)| {
                let bounds = match node.attrs.get("id").map(String::as_str) {
                    Some("clip") => [0., 0., 20., 20.],
                    Some("child") => [10., 10., 50., 50.],
                    Some("inside") => [70., 70., 20., 20.],
                    _ if node.tag == "test-hit" => [70., 70., 20., 20.],
                    _ => [0., 0., 100., 100.],
                };
                (id, bounds)
            })
            .collect();
        host.state(1).geometry.mirror.extend(bounds);
    }
    engine.eval(r#"
        const front=document.getElementById('front'),back=document.getElementById('back');
        if(document.elementFromPoint(15,15).id!=='child')throw Error('Child hit order');
        if(document.elementFromPoint(40,40)!==front)throw Error('Overflow clipping');
        if(document.elementsFromPoint(40,40).slice(0,2).map(node=>node.id).join()!=='front,back')throw Error('Stack order');
        front.style.pointerEvents='none';
        if(document.elementFromPoint(40,40)!==back)throw Error('Pointer events');
        front.style.pointerEvents='auto';front.style.display='none';
        if(document.elementFromPoint(40,40)!==back)throw Error('Hidden element');
        front.style.display='block';front.remove();
        if(document.elementFromPoint(40,40)!==back)throw Error('Detached bounds');
        document.body.append(front);
        if(document.elementFromPoint(40,40)!==front)throw Error('Reordered element');
        front.remove();
        if(shadowHost.shadowRoot.elementFromPoint(75,75)!==shadowHost.shadowRoot.firstChild||document.elementFromPoint(75,75)!==shadowHost)throw Error('Shadow hit');
        shadowHost.remove();
        if(document.elementFromPoint(75,75)!==back)throw Error('Detached shadow');
        if(document.elementFromPoint(-1,10)!==null||document.elementsFromPoint(900,10).length)throw Error('Viewport clipping');
    "#, "test").unwrap();
}

#[test]
fn hit_testing_takes_the_box_painted_last() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow().store.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><body><div id='panel' style='height:0'><button id='button'></button></div><pre id='code'><span id='line'>x</span></pre></body></html>"),
        dirty: false, ..Default::default()
    });
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    let ids: Vec<usize> = ["body", "#panel", "#button", "#code", "#line"]
        .iter()
        .map(|selector| {
            engine
                .eval(&format!("document.querySelector('{selector}')._id"), "test")
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect();
    {
        let host = host.borrow().store.clone();
        let mut host = host.borrow_mut();
        let dom = host.state(1);
        for (&id, bounds) in ids.iter().zip([
            [0., 0., 100., 100.],
            [0., 0., 100., 0.],
            [10., 10., 40., 20.],
            [0., 0., 100., 100.],
            [0., 0., 100., 20.],
        ]) {
            dom.geometry.mirror.insert(id, bounds);
        }
        dom.layout.painted = vec![0; ids.iter().max().unwrap() + 1];
        for (order, id) in [ids[0], ids[3], ids[4], ids[1], ids[2]]
            .into_iter()
            .enumerate()
        {
            dom.layout.painted[id] = order as u64 + 1;
        }
    }
    engine.eval(r#"
        if (document.elementFromPoint(20, 15).id !== 'button') throw Error('Later sibling hid a box painted above it');
        if (document.elementFromPoint(80, 15).id !== 'line') throw Error('Hit outside the button');
        if (document.elementsFromPoint(20, 15).map(node => node.id).slice(0, 3).join() !== 'button,line,code') throw Error('Stack order');
    "#, "test").unwrap();
}

#[test]
fn scrolling_snaps_to_device_pixels_and_respects_scroll_margins() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow().store.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><body><div id='box' style='overflow:scroll;scroll-padding:0 10px 0 20%'><i id='caret' style='scroll-margin:0 8px 0 50px'></i><i id='plain'></i></div></body></html>"),
        dirty: false, ..Default::default()
    });
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    let ids: Vec<usize> = ["#box", "#caret", "#plain"]
        .iter()
        .map(|selector| {
            engine
                .eval(&format!("document.querySelector('{selector}')._id"), "test")
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect();
    {
        let host = host.borrow().store.clone();
        let mut host = host.borrow_mut();
        let dom = host.state(1);
        dom.pixel_ratio = 2.;
        for (&id, bounds) in
            ids.iter()
                .zip([[0., 0., 200., 50.], [300., 0., 2., 20.], [40., 0., 2., 20.]])
        {
            dom.geometry.mirror.insert(id, bounds);
        }
    }
    engine.eval(r#"
        const box = document.getElementById('box');
        for (const [value, snapped] of [[12.367, 12.5], [12.25, 12.5], [12.75, 13]]) {
            box.scrollLeft = value;
            if (box.scrollLeft !== snapped) throw Error(`scrollLeft ${value} read ${box.scrollLeft}`);
        }
        box.scrollLeft = 0;
        document.getElementById('caret').scrollIntoView({ block: 'nearest', inline: 'nearest' });
        if (box.scrollLeft !== 120) throw Error(`Scroll margin and padding: ${box.scrollLeft}`);
    "#, "test").unwrap();
}

#[test]
fn screen_describes_the_viewport_without_a_window() {
    let (.., result) = offscreen_page(
        r#"
        same([screen.width, screen.height, screen.availWidth, screen.availHeight, screen.availLeft, screen.availTop], [800, 600, 800, 600, 0, 0], 'size');
        same([screenX, screenY, screenLeft, screenTop, outerWidth, outerHeight], [0, 0, 0, 0, 800, 600], 'window frame');
        same([screen.colorDepth, screen.pixelDepth, screen.orientation.type, screen.orientation.angle], [24, 24, 'landscape-primary', 0], 'depth');
        if (!(screen instanceof Screen) || !(screen.orientation instanceof ScreenOrientation) || window.screen !== screen) throw Error('types');
        if (String(screen) !== '[object Screen]' || screen.onchange !== null || !(screen instanceof EventTarget)) throw Error('events');
        await rejected(() => screen.orientation.lock('portrait'), 'NotSupportedError');
        const inner = await reply(worker(`postMessage([typeof screen, typeof screenX, typeof OffscreenCanvas, typeof createImageBitmap, typeof ImageBitmap])`));
        same(inner, ['undefined', 'undefined', 'function', 'function', 'function'], 'worker globals');
    "#,
    );
    assert_eq!(result, "ok");
}

#[test]
fn mirror_follows_random_mutations_and_releases() {
    for seed in 1..17u32 {
        let host = Rc::new(RefCell::new(Host::default()));
        host.borrow().store.borrow_mut().pages.insert(1, Page { doc: Document::parse("<html><body><div id=a class='x y'>t<!--c-->u<p>p</p></div><ul><li>1</li><li>2</li></ul></body></html>"), dirty: false, ..Default::default() });
        let engine = JsContext::new(
            JavaScriptCore::load().unwrap(),
            host.clone(),
            Role::Renderer(1),
        )
        .unwrap();
        engine.eval(&format!(r#"
          let seed = {seed};
          const rand = n => {{ seed = (seed * 1103515245 + 12345) & 0x7fffffff; return seed % n; }};
          const root = document.body, pool = [], all = () => {{ const list = []; const walk = n => {{ list.push(n); for (const c of n.childNodes) walk(c); }}; walk(root); return list; }};
          const pick = () => {{ const list = all().slice(1).concat(pool); return list.length ? list[rand(list.length)] : document.createElement('s'); }};
          const el = () => {{ const list = all().concat(pool).filter(n => n.nodeType === 1); return list[rand(list.length)]; }};
          globalThis.step = () => {{ for (let i = 0; i < 400; i++) try {{
            switch (rand(19)) {{
              case 0: pool.push(document.createElement(['div','span','b','i'][rand(4)])); break;
              case 1: pool.push(document.createTextNode('t' + rand(100))); break;
              case 2: el().appendChild(pick()); break;
              case 3: {{ const p = el(), kids = p.childNodes; p.insertBefore(pick(), kids.length ? kids[rand(kids.length)] : null); break; }}
              case 4: {{ const n = pick(); n.parentNode?.removeChild(n); break; }}
              case 5: el().setAttribute(['a','b','class','data-x'][rand(4)], 'v' + rand(5)); break;
              case 6: el().removeAttribute(['a','b','class','data-x'][rand(4)]); break;
              case 7: el().textContent = rand(2) ? 'txt' + rand(9) : ''; break;
              case 8: el().innerHTML = '<i a=1>x<b>y</b></i>z<!--q-->'; break;
              case 9: el().classList.toggle('k' + rand(3)); break;
              case 10: {{ const n = pick(); if (n.nodeType === 3) n.data = 'd' + rand(9); break; }}
              case 11: el().replaceChildren(document.createElement('em'), 'w'); break;
              case 12: pool.push(el().cloneNode(true)); break;
              case 13: {{ const n = el(); n.firstChild?.nextSibling?.previousSibling; n.lastChild?.remove(); if (!rand(4)) pool.length = 0; break; }}
              case 14: {{ const h = el(); if (h !== root && !h.shadowRoot && h.localName !== 'em') {{ const r = h.attachShadow({{ mode: 'open' }}); r.innerHTML = '<b>s<i>t</i></b>'; pool.push(r.firstChild, r); }} break; }}
              case 15: {{ const p = el(), f = document.createDocumentFragment(); for (let k = rand(4); k--;) f.append(pick()); p.insertBefore(f, p.childNodes[rand(p.childNodes.length + 1)] ?? null); break; }}
              case 16: {{ const p = el(); while (p.firstChild && rand(4)) p.removeChild(rand(2) ? p.firstChild : p.lastChild); if (rand(2)) p.append(pick(), pick()); break; }}
              case 17: {{ const n = el(), k = [pick(), pick(), 'w' + rand(9)]; if (n === root) break; [() => n.after(...k), () => n.before(...k), () => n.prepend(...k), () => n.replaceWith(...k)][rand(4)](); break; }}
              case 18: {{ const t = pick(), u = pick(); if (t.nodeType === 3 && u.nodeType === 3) {{ const s = 'a\u{{1F600}}b'; t.data = s.slice(0, 2); u.data = s.slice(2); }} break; }}
            }}
          }} catch (e) {{ if (!(e instanceof DOMException)) throw e; }} }};
          globalThis.dumpMirror = () => {{ let out = ''; const walk = n => {{ out += '(' + (n.nodeType === 1 ? n.localName : n.nodeType === 11 ? '#fragment' : n.nodeName); if (n.nodeType === 3 || n.nodeType === 8) out += '|' + n.data; if (n.nodeType === 1) for (const name of n.getAttributeNames()) out += ' ' + name + '=' + n.getAttribute(name); if (n.shadowRoot) {{ out += '['; walk(n.shadowRoot); out += ']'; }} for (const c of n.childNodes) walk(c); out += ')'; }}; walk(document); return out; }};
          step();
        "#), "test").unwrap();
        for round in 0..4 {
            let rust = {
                let host = host.borrow().store.clone();
                let host = host.borrow();
                let (doc, shadows) = (&host.pages[&1].doc, &host.pages[&1].state.tree.shadows);
                let (mut out, mut pending) = (String::new(), vec![Ok(0)]);
                while let Some(item) = pending.pop() {
                    let id = match item {
                        Ok(id) => id,
                        Err(text) => {
                            out.push_str(text);
                            continue;
                        }
                    };
                    let node = &doc.nodes[id];
                    out.push('(');
                    out.push_str(&node.tag);
                    if matches!(node.tag.as_str(), "#text" | "#comment") {
                        out.push('|');
                        out.push_str(&node.text);
                    }
                    for (key, value) in &node.attrs {
                        out.push_str(&format!(" {key}={value}"));
                    }
                    assert!(
                        node.children
                            .iter()
                            .all(|&child| doc.nodes[child].parent == Some(id))
                    );
                    pending.push(Err(")"));
                    pending.extend(node.children.iter().rev().map(|&child| Ok(child)));
                    if let Some(&root) = shadows.get(&id) {
                        pending.extend([Err("]"), Ok(root), Err("[")]);
                    }
                }
                out
            };
            assert_eq!(
                engine.eval("dumpMirror()", "test").unwrap(),
                rust,
                "seed {seed} round {round}"
            );
            engine.collect_garbage();
            engine
                .eval("__appsDomSweep(true); step(); __appsDomSweep(true)", "test")
                .unwrap();
        }
    }
}

#[test]
fn layout_mirror_feeds_observers_offsets_and_scrolling() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow().store.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><body><div id='clip' style='overflow:hidden'><p id='a'>a</p><p id='b'>b</p></div><div id='rel' style='position:relative'><span id='c'>c</span></div><div id='gone' style='display:none'></div></body></html>"),
        dirty: false, ..Default::default()
    });
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    let place = |rects: &[(&str, [f32; 4])]| {
        let host = host.borrow().store.clone();
        let mut host = host.borrow_mut();
        let host = &mut *host;
        let page = host.pages.get_mut(&1).unwrap();
        let (doc, state) = (&page.doc, &mut page.state);
        for (name, rect) in rects {
            let id = doc
                .nodes
                .iter()
                .position(|node| node.attrs.get("id").map(String::as_str) == Some(name))
                .unwrap();
            state.geometry.mirror.insert(id, *rect);
        }
    };
    place(&[
        ("clip", [0., 0., 100., 100.]),
        ("a", [0., 50., 100., 100.]),
        ("b", [0., 200., 100., 50.]),
        ("rel", [10., 300., 200., 100.]),
        ("c", [30., 320., 20., 10.]),
    ]);
    engine.eval(r#"
        globalThis.seen = [];
        let margin; try { new IntersectionObserver(() => {}, { rootMargin: 'foo' }); } catch (e) { margin = e.name; }
        if (margin !== 'SyntaxError' || new IntersectionObserver(() => {}, { rootMargin: '10px 5%' }).rootMargin !== '10px 5% 10px 5%') throw Error('rootMargin ' + margin);
        let reported; addEventListener('error', event => { reported = event.error.message; event.preventDefault(); }, { once: true });
        new IntersectionObserver(() => { throw Error('failing observer'); }).observe(document.getElementById('a'));
        const io = new IntersectionObserver(entries => seen.push(...entries.map(e => [e.target.id, e.isIntersecting, e.intersectionRatio, e.intersectionRect.y, e.intersectionRect.height].join())), { threshold: [0, 0.5, 1] });
        for (const id of ['a', 'b', 'c', 'gone']) io.observe(document.getElementById(id));
        __appsLayout();
        if (seen.join(';') !== 'a,true,0.5,50,50;b,false,0,200,0;c,true,1,320,10;gone,false,0,0,0' || reported !== 'failing observer') throw Error(seen.join(';'));
        seen.length = 0; __appsLayout();
        if (seen.length) throw Error('Unchanged targets reported');
        let entry;
        const pageObserver = new IntersectionObserver(([e]) => entry = e, { root: document });
        pageObserver.observe(document.getElementById('a')); __appsLayout();
        if (entry?.intersectionRatio !== 0.5 || entry.rootBounds.width !== innerWidth || entry.rootBounds.height !== innerHeight) throw Error('Document root bounds');
        pageObserver.disconnect();
        const c = document.getElementById('c'), clip = document.getElementById('clip');
        if (c.offsetParent.id !== 'rel' || c.offsetTop !== 20 || c.offsetLeft !== 20) throw Error('Offsets');
        if (document.getElementById('a').offsetParent !== document.body) throw Error('Static offset parent');
        document.getElementById('rel').style.position = 'static';
        if (c.offsetParent !== document.body) throw Error('Offset parent ignores a style change');
        if (c.getBoundingClientRect().right !== 50 || c.offsetWidth !== 20 || c.clientWidth !== 0 || c.offsetHeight !== 10) throw Error('Rect');
        clip.scrollTop = 30;
        if (clip.scrollTop !== 30 || clip.scrollLeft !== 0 || clip.scrollHeight !== 100 || document.documentElement.scrollTop !== 0) throw Error('Scroll');
        globalThis.late = []; for (let i = 0; i < 3000; i++) late.push(document.body.appendChild(document.createElement('i')));
        late.at(-1).id = 'late';
    "#, "test").unwrap();
    place(&[("a", [0., 0., 100., 100.]), ("late", [1., 2., 3., 4.])]);
    engine.eval(r#"
        __appsLayout();
        if (seen.join(';') !== 'a,true,1,0,100') throw Error(seen.join(';'));
        if (late.at(-1).getBoundingClientRect().height !== 4 || document.getElementById('c').getBoundingClientRect().x !== 30) throw Error('Grown mirror');
    "#, "test").unwrap();
}

#[test]
fn child_node_methods_reactions_and_observers_follow_the_standard() {
    let (_, engine) = checked_page(
        "<html><body><div id='box'></div></body></html>",
        r#"
        const p = document.createElement('p'), [a, b, c] = ['a', 'b', 'c'].map(tag => document.createElement(tag));
        p.append(a, b);
        a.after(b, c);
        check('after skips its own nodes', () => p.innerHTML === '<a></a><b></b><c></c>');
        p.prepend(p.firstChild, 'x');
        check('prepend skips its own nodes', () => p.innerHTML === '<a></a>x<b></b><c></c>');
        b.replaceWith(b, c);
        check('replaceWith keeps itself', () => p.innerHTML === '<a></a>x<b></b><c></c>');
        a.replaceWith(a, b);
        check('replaceWith moves the rest', () => p.innerHTML === '<a></a><b></b>x<c></c>');
        c.before(c, a);
        check('before skips its own nodes', () => p.innerHTML === '<b></b>x<c></c><a></a>');
        globalThis.log = [];
        const box = document.getElementById('box');
        box.innerHTML = '<x-order id="1"><x-order id="2"></x-order></x-order><x-order id="3"></x-order>';
        const early = document.createElement('x-late');
        customElements.define('x-order', class extends HTMLElement { connectedCallback() { log.push('in' + this.id); } disconnectedCallback() { log.push('out' + this.id); } });
        check('upgrades run in tree order', () => log.join() === 'in1,in2,in3');
        log.length = 0;
        box.replaceChildren(...[...box.children].reverse());
        check('replaceChildren disconnects before it connects again', () => log.length === 6 && ['1', '2', '3'].every(id => log.lastIndexOf('out' + id) < log.lastIndexOf('in' + id)) && box.firstChild.id === '3');
        customElements.define('x-late', class extends HTMLElement { connectedCallback() { log.push('late'); } });
        check('detached element waits for its upgrade', () => !(early instanceof customElements.get('x-late')));
        box.append(early);
        check('insertion upgrades', () => early instanceof customElements.get('x-late') && log.at(-1) === 'late');
        check(':scope in a document is the root element', () => document.querySelector(':scope > body') === document.body && document.querySelector(':scope') === document.documentElement);
        globalThis.seen = [];
        new MutationObserver(() => { seen.push(1); throw Error('observer failure'); }).observe(box, { attributes: true });
        new MutationObserver(() => seen.push(2)).observe(box, { attributes: true });
        box.setAttribute('data-x', '1');
        __appsDispatch('pointerdown', [early._id], { pointerId: 1, pointerType: 'mouse', clientX: 0, clientY: 0, buttons: 1 });
        early.setPointerCapture(1);
        globalThis.lost = 0;
        document.addEventListener('lostpointercapture', () => lost++);
        // As in Chrome, a capture takes effect at the next pointer event.
        const move = () => __appsDispatch('pointermove', [], { pointerId: 1, pointerType: 'mouse', clientX: 1, clientY: 0, buttons: 1 });
        move();
        early.remove();
        check('removal releases the pointer capture', () => !early.hasPointerCapture(1) && lost === 1);
        const target = document.createElement('i'), detached = document.createElement('div');
        box.append(target); target.setPointerCapture(1); move();
        let bubbled = false; box.addEventListener('lostpointercapture', () => bubbled = true);
        target.releasePointerCapture(1); move();
        check('release fires a bubbling lostpointercapture', () => bubbled && lost === 2);
        target.setPointerCapture(1); move(); detached.append(target);
        check('a move into a detached tree releases the pointer capture', () => !target.hasPointerCapture(1) && lost === 3);
        const field = document.createElement('input'); box.append(field); field.focus();
        let blurred = false; field.addEventListener('blur', () => blurred = true);
        detached.insertBefore(field, null);
        check('a move into a detached tree blurs', () => blurred && document.activeElement !== field);
        const sized = document.createElement('canvas'); sized.setAttribute('width', ' +100px'); sized.setAttribute('height', '1e9');
        check('canvas sizes parse as HTML integers', () => sized.width === 100 && sized.height === 1);
        sized.setAttribute('width', 'x'); sized.setAttribute('height', '2147483648');
        check('invalid canvas sizes use the defaults', () => sized.width === 300 && sized.height === 150);
        "#,
    );
    engine.tick().unwrap();
    assert_eq!(engine.eval("seen.join()", "test").unwrap(), "1,2");
}

#[test]
fn observers_hold_targets_weakly_and_batches_keep_split_surrogates() {
    let (host, engine) = checked_page(
        "<html><body><p id='one'>a</p><p id='two'>b</p><ul id='list'></ul></body></html>",
        r#"
        const [one, two] = ['one', 'two'].map(id => document.getElementById(id).firstChild), text = 'a\u{1F600}b';
        one.data = text.slice(0, 2); two.data = text.slice(2); one.parentNode.setAttribute('data-after', 'kept');
        const list = document.getElementById('list'), rows = document.createDocumentFragment();
        for (let i = 0; i < 3000; i++) { const row = document.createElement('li'); row.textContent = i; rows.append(row); }
        list.append(rows); list.prepend(...['x', 'y'].map(id => Object.assign(document.createElement('li'), { id })));
        check('mirror order', () => list.firstChild.id === 'x' && list.children[2].textContent === '0' && list.childElementCount === 3002);
        "#,
    );
    {
        let host = host.borrow().store.clone();
        let host = host.borrow();
        let doc = &host.pages[&1].doc;
        let text = |id: &str| {
            let p = doc
                .nodes
                .iter()
                .position(|node| node.attrs.get("id").is_some_and(|value| value == id))
                .unwrap();
            doc.nodes[doc.nodes[p].children[0]].text.clone()
        };
        assert_eq!(
            (text("one"), text("two")),
            ("a\u{fffd}".to_owned(), "\u{fffd}b".to_owned())
        );
        assert!(doc.nodes.iter().any(|node| {
            node.attrs
                .get("data-after")
                .is_some_and(|value| value == "kept")
        }));
        let list = doc
            .nodes
            .iter()
            .position(|node| node.attrs.get("id").is_some_and(|value| value == "list"))
            .unwrap();
        let children = &doc.nodes[list].children;
        assert_eq!(children.len(), 3002);
        assert!(
            children
                .iter()
                .all(|&child| doc.nodes[child].parent == Some(list))
        );
        assert_eq!(
            doc.nodes[children[0]].attrs.get("id").map(String::as_str),
            Some("x")
        );
        assert_eq!(doc.nodes[doc.nodes[children[2]].children[0]].text, "0");
    }
    engine.eval("const list = document.getElementById('list'); while (list.firstChild) list.removeChild(list.firstChild); list.append(document.createElement('hr'))", "test").unwrap();
    {
        let host = host.borrow().store.clone();
        let host = host.borrow();
        let doc = &host.pages[&1].doc;
        let list = doc
            .nodes
            .iter()
            .position(|node| node.attrs.get("id").is_some_and(|value| value == "list"))
            .unwrap();
        assert_eq!(doc.nodes[list].children.len(), 1);
        assert_eq!(doc.nodes[doc.nodes[list].children[0]].tag, "hr");
    }
    for _ in 0..5 {
        engine.eval("(() => { for (let i = 0; i < 2000; i++) { const el = document.createElement('div'); el.innerHTML = '<span>x</span>'; new MutationObserver(() => el.id).observe(el, { attributes: true }); new ResizeObserver(() => el.id).observe(el); new IntersectionObserver(() => el.id).observe(el); } })()", "test").unwrap();
        engine.collect_garbage();
        engine
            .eval(
                "__appsDomSweep(true); __appsLayout(); document.body.setAttribute('data-x', '1')",
                "test",
            )
            .unwrap();
    }
    let nodes = host.borrow().store.borrow().pages[&1].doc.nodes.len();
    assert!(nodes < 20000, "{nodes} nodes");
}

#[test]
fn text_offsets_skip_comments_like_text_points() {
    let (host, _engine) = checked_page(
        "<html><body><div id='e' contenteditable>ab<!--?lit$123$-->cd<p>e<!--x-->f</p></div></body></html>",
        "",
    );
    let editor = host.borrow().store.borrow().pages[&1]
        .doc
        .nodes
        .iter()
        .position(|node| node.tag == "div")
        .unwrap();
    let offset = |index: usize| {
        host.borrow_mut()
            .call(
                Role::Renderer(1),
                "dom.textOffset",
                serde_json::json!([editor, editor, index]),
            )
            .unwrap()
    };
    assert_eq!(
        (offset(3), offset(4)),
        (serde_json::json!(4), serde_json::json!(6))
    );
    let point = host
        .borrow_mut()
        .call(
            Role::Renderer(1),
            "dom.textPoint",
            serde_json::json!([editor, 6]),
        )
        .unwrap();
    assert_eq!(point[1], serde_json::json!(1));
}

#[test]
fn throwing_resize_observer_does_not_stop_layout_callbacks() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow().store.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse(
                "<html><body><main><div id='box' style='overflow:auto'></div></main></body></html>",
            ),
            dirty: false,
            ..Default::default()
        },
    );
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    engine
        .eval(
            r#"
        const box = document.getElementById('box'), seen = [];
        addEventListener('error', event => { seen.push('error'); event.preventDefault(); });
        new ResizeObserver(() => { throw Error('boom'); }).observe(box);
        new ResizeObserver(() => seen.push('resize')).observe(box);
        box.addEventListener('scroll', () => seen.push('scroll'));
        box.scrollTo(0, 5);
        __appsLayout();
        if (seen.join() !== 'error,resize,scroll') throw Error(seen.join());
    "#,
            "test",
        )
        .unwrap();
}
