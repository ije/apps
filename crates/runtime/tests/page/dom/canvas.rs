use super::*;

#[test]
fn visibility_observers_option_lists_and_canvas_current_color_follow_chrome() {
    let (_, engine) = css_page(
        "<html><body><div id=v>v</div><canvas id=k style='color: rgb(255, 0, 0)'></canvas><select id=s><option>A</option><option>B</option></select></body></html>",
    );
    engine.eval(r#"
        const $ = id => document.getElementById(id), check = (got, want, what) => { if (JSON.stringify(got) !== JSON.stringify(want)) throw Error(what + ': ' + JSON.stringify(got)); };
        check([typeof $('v').checkVisibility(null), typeof $('v').checkVisibility(undefined)], ['boolean', 'boolean'], 'null options');
        for (const Type of [ResizeObserver, IntersectionObserver, MutationObserver]) { let threw = null; try { new Type(5); } catch (error) { threw = error.constructor.name; } check(threw, 'TypeError', Type.name); }
        const ctx = $('k').getContext('2d');
        ctx.fillStyle = '#00ff00'; ctx.fillStyle = 'currentcolor'; ctx.strokeStyle = 'CurrentColor';
        check([ctx.fillStyle, ctx.strokeStyle], ['#ff0000', '#ff0000'], 'currentcolor');
        ctx.fillRect(0, 0, 1, 1); check([...ctx.getImageData(0, 0, 1, 1).data], [255, 0, 0, 255], 'currentcolor pixel');
        $('k').style.color = 'blue'; check(ctx.fillStyle, '#ff0000', 'currentcolor resolves when set');
        ctx.fillStyle = 'currentcolor junk'; check(ctx.fillStyle, '#ff0000', 'invalid currentcolor');
        check(ctx.shadowColor, 'rgba(0, 0, 0, 0)', 'default shadow');
        ctx.shadowColor = 'red'; ctx.shadowColor = 'bogus'; check(ctx.shadowColor, '#ff0000', 'invalid shadow');
        ctx.shadowColor = 'rgba(0, 0, 255, 0.5)'; check(ctx.shadowColor, 'rgba(0, 0, 255, 0.5)', 'shadow alpha');
        ctx.shadowColor = 'currentcolor'; check(ctx.shadowColor, '#0000ff', 'shadow currentcolor');
        $('k').style.color = 'rgb(255, 0, 0)'; ctx.fillStyle = 'currentcolor'; check(ctx.fillStyle, '#ff0000', 'currentcolor after the color changes back');
        $('k').removeAttribute('style'); ctx.fillStyle = 'currentcolor'; check(ctx.fillStyle, '#000000', 'currentcolor without an inline style');
        $('k').style.color = 'blue'; ctx.fillStyle = 'red';
        const hidden = document.createElement('div'), inner = document.createElement('div'); hidden.style.display = 'none'; inner.style.transform = 'translateX(5px)'; hidden.append(inner); document.body.append(hidden);
        check(getComputedStyle(inner).transform, 'none', 'transform inside display none'); let threw = ''; const fade = inner.animate([{ opacity: 0 }, { opacity: 1 }], 1000); fade.pause(); try { fade.commitStyles(); } catch (error) { threw = error.name; } fade.cancel(); check(threw, 'InvalidStateError', 'commit without a box'); hidden.style.display = 'block'; check(getComputedStyle(inner).transform, 'matrix(1, 0, 0, 1, 5, 0)', 'transform when shown'); hidden.remove();
        ctx.fillStyle = 'color-mix(in srgb, currentcolor, blue)'; check(ctx.fillStyle, '#000080', 'currentcolor in a function is black');
        const classed = document.createElement('canvas'); classed.className = 'c'; document.body.append(classed); document.head.append(Object.assign(document.createElement('style'), { textContent: '.c { color: red }' }));
        const fromSheet = classed.getContext('2d'); fromSheet.fillStyle = 'currentcolor'; check(fromSheet.fillStyle, '#000000', 'only the inline color counts');
        const loose = document.createElement('canvas').getContext('2d'), offscreen = new OffscreenCanvas(1, 1).getContext('2d');
        for (const c of [loose, offscreen]) { c.fillStyle = '#00ff00'; c.fillStyle = 'currentcolor'; check(c.fillStyle, '#000000', 'currentcolor outside the document'); }
        offscreen.fillStyle = 'color-mix(in srgb, currentcolor, blue)'; check(offscreen.fillStyle, '#000000', 'offscreen rejects currentcolor in a function');
        const s = $('s'), first = s.options;
        document.body.append(document.createElement('i'));
        check(first === s.options, true, 'same options');
        s.append(new Option('C'));
        check([first === s.options, first.length, s.options.length, s.options === s.options], [false, 2, 3, true], 'options after a change');
    "#, "test").unwrap();
}

#[test]
fn canvas_pixels_survive_partial_updates_and_clipping() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<html><body></body></html>"),
            dirty: false,
        },
    );
    let engine = JsContext::new(JavaScriptCore::load().unwrap(), host, Role::Renderer(1)).unwrap();
    engine.eval(r#"
      const canvas=document.createElement('canvas');canvas.width=8;canvas.height=8;
      const ctx=canvas.getContext('2d');ctx.fillStyle='#ff0000';ctx.fillRect(0,0,8,8);
      ctx.clearRect(2,2,2,2);let pixels=ctx.getImageData(0,0,8,8).data;
      if(pixels[0]!==255||pixels[3]!==255||pixels[(2*8+2)*4+3]!==0||pixels[(4*8+4)*4]!==255)throw Error('Partial clear failed');
      ctx.save();ctx.beginPath();ctx.moveTo(0,0);ctx.lineTo(4,0);ctx.lineTo(0,4);ctx.closePath();ctx.clip();ctx.translate(1,0);ctx.fillStyle='#0000ff';ctx.fillRect(0,0,8,8);ctx.restore();
      pixels=ctx.getImageData(0,0,8,8).data;
      if(pixels[(1*8+1)*4+2]!==255||pixels[(5*8+5)*4+2]!==0)throw Error('Path clip failed');
      const patch=new ImageData(new Uint8ClampedArray([0,255,0,255]),1,1);ctx.putImageData(patch,7,7);
      if(ctx.getImageData(7,7,1,1).data[1]!==255)throw Error('putImageData failed');
      const copy=document.createElement('canvas');copy.width=8;copy.height=8;copy.getContext('2d').drawImage(canvas,0,0);
      if(copy.getContext('2d').getImageData(7,7,1,1).data[1]!==255)throw Error('drawImage failed');
      const target=copy.getContext('2d');target.globalCompositeOperation='copy';
      target.drawImage(canvas,NaN,0);
      if(target.getImageData(7,7,1,1).data[1]!==255)throw Error('drawImage at no place cleared the canvas');
      target.globalCompositeOperation='source-over';
      if(!canvas.toDataURL().startsWith('data:image/png;base64,iVBOR'))throw Error('PNG encoding failed');
      canvas.width=8;if(ctx.getImageData(0,0,1,1).data[3]!==0)throw Error('Canvas resize did not clear pixels');
      ctx.translate(-1e8,0);ctx.fillRect(1e8+3,0,1,1);ctx.resetTransform();
      const row=ctx.getImageData(0,0,8,1).data;if(row[2*4+3]||row[3*4+3]!==255||row[4*4+3])throw Error('Far coordinates lost precision');
      ctx.fillStyle='#ff0000';ctx.fillRect(2,4,1,1);
      if(ctx.getImageData(2.7,4.2,1.9,1.5).data[0]!==255||ctx.getImageData(3,5,-1,-1).data[0]!==255)throw Error('getImageData arguments are longs');
      for(const size of [[0,1],[0.5,1],[1,-0.2]]){let threw=null;try{ctx.getImageData(0,0,...size);}catch(error){threw=error.name;}if(threw!=='IndexSizeError')throw Error('Empty getImageData: '+threw);}
      ctx.beginPath();ctx.moveTo(0,6);ctx.lineTo(NaN,7);ctx.lineTo(1e300,7);ctx.lineTo(8,6);ctx.lineTo(8,8);ctx.lineTo(0,8);ctx.fill();
      if(ctx.getImageData(5,7,1,1).data[0]!==255)throw Error('A non-finite point dropped the path');
    "#, "test").unwrap();
}

#[test]
fn hostile_selectors_canvas_data_and_traversal() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><body><ul id='list'><li id='a'>1</li><li id=\"it's\">2</li><li id='c'>3</li></ul><svg><g><circle></circle></g><foreignObject><div id='html'></div></foreignObject></svg></body></html>"),
        dirty: false,
    });
    let engine = JsContext::new(JavaScriptCore::load().unwrap(), host, Role::Renderer(1)).unwrap();
    engine.eval(r#"
        const deep=':not('.repeat(100000)+'li'+')'.repeat(100000);
        if(document.querySelector(deep)!==null||document.querySelector(':is(:not(#a))#c')?.id!=='c'||document.querySelectorAll('li:not(:first-child)').length!==2)throw Error('Selector nesting');
        if(document.getElementById('')!==null||document.getElementById("it's")?.textContent!=='2'||document.getElementById('c')?.textContent!=='3')throw Error('getElementById');
        const ids=[];for(let n=document.getElementById('list').firstChild;n;n=n.nextSibling)ids.push(n.id);
        if(ids.join()!=="a,it's,c"||document.getElementById('c').previousSibling.id!=="it's"||document.getElementById('a').previousSibling!==null)throw Error('Sibling traversal');
        const walker=document.createTreeWalker(document.body,NodeFilter.SHOW_ELEMENT,node=>node.localName==='svg'?NodeFilter.FILTER_REJECT:node.localName==='ul'?NodeFilter.FILTER_SKIP:NodeFilter.FILTER_ACCEPT);
        const seen=[];for(let n;(n=walker.nextNode());)seen.push(n.id);
        if(seen.join()!=="a,it's,c")throw Error('TreeWalker filters');
        const [a,c]=['a','c'].map(id=>document.getElementById(id));
        if(a.compareDocumentPosition(c)!==4||c.compareDocumentPosition(a)!==2||document.body.compareDocumentPosition(a)!==20||(a.compareDocumentPosition(document.createElement('p'))|6)!==39)throw Error('Document position');
        if(!(document.querySelector('circle') instanceof SVGElement)||document.getElementById('html') instanceof SVGElement)throw Error('SVG wrapper');
        const calls=[];a.addEventListener('ping',()=>{throw Error('listener failure')});a.addEventListener('ping',()=>calls.push('second'));document.body.addEventListener('ping',()=>calls.push('bubble'));
        a.dispatchEvent(new Event('ping',{bubbles:true}));if(calls.join()!=='second,bubble')throw Error('Listener exception stopped dispatch');
        const canvas=document.createElement('canvas');canvas.width=canvas.height=4;const ctx=canvas.getContext('2d'),forged=new ImageData(1,1);
        Object.assign(forged,{width:2**31,height:2**31,data:new Uint8ClampedArray(0)});ctx.putImageData(forged,0,0);
        let rejected=false;try{ctx.getImageData(0,0,1,1);}catch{rejected=true;}if(!rejected)throw Error('Forged image data accepted');
    "#, "test").unwrap();
}

#[test]
fn shadow_cycles_child_access_release_and_retired_images() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<html><body><div id='box'></div></body></html>"),
            dirty: false,
        },
    );
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    engine.eval(r#"
        const el = document.createElement('div'); document.body.append(el);
        const root = el.attachShadow({mode:'open'}), inner = document.createElement('span'); root.append(inner);
        let rejected = 0;
        for (const [parent, child] of [[root, el], [inner, el], [inner, document.body]]) try { parent.appendChild(child); } catch { rejected++; }
        if (rejected !== 3 || el.parentNode !== document.body || !inner.isConnected) throw Error('Shadow cycle accepted');
        const list = document.createElement('ul'); for (let i = 0; i < 3; i++) list.append(document.createElement('li'));
        if (list.firstChild !== list.children[0] || list.lastChild !== list.children[2] || !list.hasChildNodes() || list.lastChild.parentNode !== list) throw Error('Child access');
        while (list.firstChild) list.removeChild(list.firstChild);
        if (list.hasChildNodes() || list.firstChild || list.lastChild) throw Error('Empty list');
        const a = document.createElement('i'), b = document.createElement('b'); list.append(a); list.replaceChildren(b, 'c');
        if (a.parentNode || list.firstChild !== b || list.childNodes.length !== 2 || list.lastChild.data !== 'c') throw Error('replaceChildren');
        el.style.cssText = 'content: "a;b"; color: red';
        const style = getComputedStyle(el);
        if (style.content !== '"a;b"' || style.color !== 'rgb(255, 0, 0)' || style.getPropertyValue('constructor') !== '' || style[0] !== style.item(0) || !style.length || el.style.cssText !== 'content: "a;b"; color: red;') throw Error('Computed style');
        let threw = false; try { style.color = 'blue'; } catch { threw = true; }
        if (!threw) throw Error('Computed style is writable');
        const canvas = document.createElement('canvas'); canvas.width = 4; canvas.height = 4; document.body.append(canvas);
        const ctx = canvas.getContext('2d'); ctx.fillRect(0, 0, 4, 4); ctx._flush(); ctx.fillRect(0, 0, 2, 2); ctx._flush(); canvas.width = 8;
        globalThis.image = document.createElement('img'); document.body.append(image);
        document.getElementById('box').innerHTML = '<p>a</p><canvas></canvas>';
        __appsDomSweep(true);
        document.getElementById('box').textContent = '';
    "#, "test").unwrap();
    let [p, canvas] = ["p", "canvas"].map(|tag| {
        host.borrow().pages[&1]
            .doc
            .nodes
            .iter()
            .position(|node| node.tag == tag && node.parent.is_none())
            .unwrap()
    });
    host.borrow_mut().pages.get_mut(&1).unwrap().dirty = false;
    let freed = host
        .borrow_mut()
        .call(
            Role::Renderer(1),
            "dom.release",
            serde_json::json!([[], []]),
        )
        .unwrap();
    assert_eq!(freed[0], serde_json::json!([canvas]));
    assert!(host.borrow().pages[&1].doc.nodes[p].tag.is_empty());
    assert!(!host.borrow().pages[&1].dirty);
    let image: usize = engine.eval("image._id", "test").unwrap().parse().unwrap();
    {
        use base64::Engine as _;
        let png = base64::engine::general_purpose::STANDARD
            .encode(std::fs::read("tests/images/sample.png").unwrap());
        for _ in 0..2 {
            host.borrow_mut()
                .call(
                    Role::Renderer(1),
                    "dom.image",
                    serde_json::json!([image, png]),
                )
                .unwrap();
            let started = std::time::Instant::now();
            while host
                .borrow_mut()
                .call(Role::Renderer(1), "dom.decoded", serde_json::json!([]))
                .unwrap()
                .as_array()
                .unwrap()
                .is_empty()
            {
                assert!(started.elapsed() < std::time::Duration::from_secs(10));
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }
    assert_eq!(host.borrow().dom[&1].media.retired.len(), 3);
}

#[test]
fn canvas_paths_matrices_patterns_and_shadows() {
    let (.., result) = offscreen_page(
        r#"
        const canvas = new OffscreenCanvas(20, 20), ctx = canvas.getContext('2d'), at = (x, y) => [...ctx.getImageData(x, y, 1, 1).data];
        ctx.font = 'bold 12pt/2 Arial'; const font = ctx.font; ctx.font = 'bogus'; same([font, ctx.font], ['bold 16px Arial', 'bold 16px Arial'], 'font');
        const square = new Path2D('M2 2h6v6H2z'), copy = new Path2D(square); copy.rect(12, 2, 6, 6);
        ctx.beginPath(); ctx.rect(0, 12, 4, 4); ctx.fillStyle = '#ff0000'; ctx.fill(copy); ctx.translate(0, 10); ctx.fill(square, 'evenodd'); ctx.resetTransform();
        same([at(5, 5), at(15, 5), at(5, 15), at(1, 13)], [[255, 0, 0, 255], [255, 0, 0, 255], [255, 0, 0, 255], [0, 0, 0, 0]], 'Path2D fill');
        same([ctx.isPointInPath(square, 5, 5), ctx.isPointInPath(square, 9, 5), ctx.isPointInStroke(square, 2, 5), ctx.isPointInPath(1, 13), ctx.isPointInPath(NaN, 1)], [true, false, true, true, false], 'hits');
        ctx.fill(); same(at(1, 13), [255, 0, 0, 255], 'current path');
        const moved = new Path2D(); moved.addPath(square, new DOMMatrix().translate(10, 10)); ctx.fillStyle = '#0000ff'; ctx.fill(moved);
        moved.arc(15, 15, 1, 0, 7); ctx.fill(moved); same(at(15, 15), [0, 0, 255, 255], 'addPath');
        const half = new Path2D('M0 0H10V20H0Z'); ctx.save(); ctx.clip(half); ctx.clearRect(0, 0, 20, 20); ctx.restore(); same([at(5, 5)[3], at(15, 15)[3]], [0, 255], 'clip');
        try { ctx.fill(square, 'odd'); throw Error('rule'); } catch (error) { if (!(error instanceof TypeError)) throw error; }
        const m = new DOMMatrix([1, 2, 3, 4, 5, 6]);
        same([String(m.inverse()), String(m.multiply(m.inverse())), m.is2D, m.isIdentity, new DOMMatrix().isIdentity], ['matrix(-2, 1, 1.5, -0.5, 1, -2)', 'matrix(1, 0, 0, 1, 0, 0)', true, false, true], 'matrix');
        const p = new DOMMatrix().translate(10, 20).scale(2, 3).transformPoint(new DOMPoint(1, 1));
        same([p.x, p.y, p.w, new DOMMatrix().translate(0, 0, 5).is2D, new DOMMatrix().rotate(0, 0, 30).toString() === new DOMMatrix().rotate(30).toString(), Math.round(new DOMMatrix().rotate(30).b * 1000)], [12, 23, 1, false, true, 500], 'transforms');
        same([String(DOMMatrix.fromMatrix({ a: 2, f: 3 })), new DOMMatrix('matrix(1, 2, 3, 4, 5, 6)').f, new DOMMatrix(new DOMMatrix([1, 0, 0, 1, 7, 0])).e, [...new DOMMatrix().scale(2).toFloat64Array()].join()], ['matrix(2, 0, 0, 1, 0, 3)', 6, 7, '2,0,0,0,0,2,0,0,0,0,1,0,0,0,0,1'], 'conversions');
        ctx.setTransform(new DOMMatrix([2, 0, 0, 2, 1, 1])); const t = ctx.getTransform(); same([t instanceof DOMMatrix, t.a, t.e], [true, 2, 1], 'getTransform'); ctx.resetTransform();
        const near = (got, want, name) => { if (got.some((v, i) => Math.abs(v - want[i]) > 2)) throw Error(`${name}: ${got} is not ${want}`); };
        const tile = new OffscreenCanvas(2, 1), tiles = tile.getContext('2d'); tiles.fillStyle = '#ff0000'; tiles.fillRect(0, 0, 1, 1); tiles.fillStyle = '#0000ff'; tiles.fillRect(1, 0, 1, 1);
        ctx.clearRect(0, 0, 20, 20); ctx.fillStyle = ctx.createPattern(tile, 'repeat'); ctx.fillRect(0, 0, 20, 20);
        same([at(4, 3), at(5, 3)], [[255, 0, 0, 255], [0, 0, 255, 255]], 'repeat');
        const strip = ctx.createPattern(tile, 'repeat-x'); strip.setTransform(new DOMMatrix().translate(0, 10)); ctx.clearRect(0, 0, 20, 20); ctx.fillStyle = strip; ctx.fillRect(0, 0, 20, 20);
        same([at(16, 10), at(17, 10), at(4, 9)[3], at(4, 11)[3]], [[255, 0, 0, 255], [0, 0, 255, 255], 0, 0], 'repeat-x');
        ctx.clearRect(0, 0, 20, 20); ctx.fillStyle = ctx.createPattern(tile, 'no-repeat'); ctx.fillRect(0, 0, 20, 20); same([at(1, 0)[2], at(2, 0)[3], at(0, 1)[3]], [255, 0, 0], 'no-repeat');
        same([ctx.createPattern(new Image(), null), (() => { try { ctx.createPattern(tile, 'x'); } catch (error) { return error.name; } })()], [null, 'SyntaxError'], 'pattern errors');
        const cone = ctx.createConicGradient(0, 10.5, 10.5); cone.addColorStop(0, '#ff0000'); cone.addColorStop(1, '#0000ff'); ctx.fillStyle = cone; ctx.fillRect(0, 0, 20, 20);
        near(at(10, 15), [191, 0, 64, 255], 'conic quarter'); near(at(5, 10), [128, 0, 128, 255], 'conic half');
        ctx.clearRect(0, 0, 20, 20); ctx.fillStyle = '#ff0000'; ctx.shadowColor = '#0000ff'; ctx.shadowOffsetX = ctx.shadowOffsetY = 5; ctx.fillRect(2, 2, 4, 4);
        same([at(3, 3), at(8, 8), at(12, 12)[3], at(3, 8)[3]], [[255, 0, 0, 255], [0, 0, 255, 255], 0, 0], 'shadow offset');
        ctx.clearRect(0, 0, 20, 20); ctx.shadowOffsetX = ctx.shadowOffsetY = 0; ctx.shadowBlur = 4; ctx.fillRect(7, 7, 4, 4);
        same([at(9, 9), Math.abs(at(9, 6)[3] - 66) < 10, at(9, 3)[3] > 0, at(9, 0)[3]], [[255, 0, 0, 255], true, true, 0], 'shadow blur');
        ctx.shadowColor = 'transparent';
        same(await reply(worker(`postMessage([typeof Path2D, typeof DOMMatrix, typeof DOMPoint])`)), ['function', 'function', 'function'], 'worker globals');
    "#,
    );
    assert_eq!(result, "ok");
}

#[test]
fn canvas_sources_flush_their_readers_before_their_pixels_change() {
    let (.., result) = offscreen_page(
        r#"
        const source = new OffscreenCanvas(2, 2), from = source.getContext('2d'), target = new OffscreenCanvas(10, 2), to = target.getContext('2d');
        const at = x => [...to.getImageData(x, 0, 1, 1).data];
        from.fillStyle = '#ff0000'; from.fillRect(0, 0, 2, 2); to.drawImage(source, 0, 0);
        from.fillStyle = '#0000ff'; from.fillRect(0, 0, 2, 2); to.drawImage(source, 2, 0);
        source.width = 2; to.drawImage(source, 4, 0);
        from.fillStyle = '#00ff00'; from.fillRect(0, 0, 2, 2); to.fillStyle = to.createPattern(source, 'no-repeat'); to.fillRect(0, 0, 0, 0); to.drawImage(source, 6, 0);
        const bitmap = source.transferToImageBitmap(); to.drawImage(source, 8, 0);
        same([at(0), at(2), at(4)[3], at(6), at(8)[3]], [[255, 0, 0, 255], [0, 0, 255, 255], 0, [0, 255, 0, 255], 0], 'draws');
        const check = new OffscreenCanvas(2, 2).getContext('2d'); check.drawImage(bitmap, 0, 0); same([...check.getImageData(0, 0, 1, 1).data], [0, 255, 0, 255], 'bitmap');
        const element = document.createElement('canvas'); element.width = element.height = 2; const shown = element.getContext('2d');
        shown.fillStyle = '#ff0000'; shown.fillRect(0, 0, 2, 2); to.clearRect(0, 0, 10, 2); to.drawImage(element, 0, 0);
        element.width = 2; shown.fillStyle = '#0000ff'; shown.fillRect(0, 0, 1, 2); to.drawImage(element, 2, 0);
        same([at(0), at(2), at(3)[3]], [[255, 0, 0, 255], [0, 0, 255, 255], 0], 'element source');
        const a = new OffscreenCanvas(1, 1), b = new OffscreenCanvas(1, 1), ac = a.getContext('2d'), bc = b.getContext('2d');
        ac.fillStyle = '#ff0000'; ac.fillRect(0, 0, 1, 1); bc.drawImage(a, 0, 0); ac.drawImage(b, 0, 0); ac.clearRect(0, 0, 1, 1); bc.fillStyle = '#0000ff'; bc.fillRect(0, 0, 1, 1); ac.drawImage(b, 0, 0);
        same([[...ac.getImageData(0, 0, 1, 1).data], [...bc.getImageData(0, 0, 1, 1).data]], [[0, 0, 255, 255], [0, 0, 255, 255]], 'cycle');
    "#,
    );
    assert_eq!(result, "ok");
}

#[test]
fn offscreen_canvas_draws_reads_and_resizes() {
    let (.., result) = offscreen_page(
        r#"
        same(typeof OffscreenCanvas, 'function', 'global');
        const canvas = new OffscreenCanvas(4.7, 2), context = canvas.getContext('2d');
        same([canvas.width, canvas.height, String(canvas), context.canvas === canvas, context instanceof OffscreenCanvasRenderingContext2D, canvas.getContext('2d') === context, canvas.getContext('webgpu'), new OffscreenCanvas(1, 1).getContext('webgl2')], [4, 2, '[object OffscreenCanvas]', true, true, true, null, null], 'object');
        for (const size of [-1, NaN, Infinity]) { try { new OffscreenCanvas(size, 1); } catch (error) { if (error instanceof TypeError) continue; } throw Error('size ' + size); }
        try { new OffscreenCanvas(1); throw Error('one size'); } catch (error) { if (!(error instanceof TypeError)) throw error; }
        context.fillStyle = '#ff0000'; context.fillRect(0, 0, 2, 2);
        same([...context.getImageData(1, 1, 1, 1).data], [255, 0, 0, 255], 'pixels');
        const element = document.createElement('canvas'); element.width = element.height = 4; const other = element.getContext('2d'); other.drawImage(canvas, 2, 2);
        same([...other.getImageData(3, 3, 1, 1).data], [255, 0, 0, 255], 'drawn on a canvas');
        const blob = await canvas.convertToBlob({ type: 'image/webp' }), jpeg = await canvas.convertToBlob({ type: 'image/jpeg', quality: 0.5 });
        // Only macOS makes JPEG, and other platforms give PNG for it.
        const mac = navigator.platform.includes('Mac');
        same([blob.type, jpeg.type, element.toDataURL('image/JPEG').slice(0, 26), element.toDataURL('text/plain').slice(0, 22)], mac ? ['image/png', 'image/jpeg', 'data:image/jpeg;base64,/9j', 'data:image/png;base64,'] : ['image/png', 'image/png', 'data:image/png;base64,iVBO', 'data:image/png;base64,'], 'blob type');
        same([...new Uint8Array(await blob.arrayBuffer()).subarray(0, 4), ...new Uint8Array(await jpeg.arrayBuffer()).subarray(0, 2)], [137, 80, 78, 71, ...(mac ? [255, 216] : [137, 80])], 'png');
        context.fillStyle = '#00ff00'; canvas.width = 6;
        same([canvas.width, [...context.getImageData(0, 0, 1, 1).data]], [6, [0, 0, 0, 0]], 'resize clears');
        same(context.fillStyle, '#000000', 'resize resets the state');

        const empty = new OffscreenCanvas(0, 0), blank = empty.getContext('2d'); blank.fillRect(0, 0, 1, 1); blank._flush();
        await rejected(() => empty.convertToBlob(), 'IndexSizeError');
        await rejected(async () => empty.transferToImageBitmap(), 'InvalidStateError');
        await rejected(async () => new OffscreenCanvas(1, 1).transferToImageBitmap(), 'InvalidStateError');

        const moved = new OffscreenCanvas(2, 2), drawing = moved.getContext('2d');
        drawing.fillStyle = '#0000ff'; drawing.translate(1, 0); drawing.fillRect(0, 0, 1, 1);
        const taken = moved.transferToImageBitmap();
        same([taken.width, taken.height, rgba(taken, 1, 0), rgba(taken, 0, 0)], [2, 2, '0,0,255,255', '0,0,0,0'], 'bitmap');
        same([...drawing.getImageData(1, 0, 1, 1).data], [0, 0, 0, 0], 'the canvas is blank after the transfer');
        drawing.fillRect(0, 1, 1, 1);
        same([...drawing.getImageData(1, 1, 1, 1).data], [0, 0, 255, 255], 'the drawing state survives the transfer');
    "#,
    );
    assert_eq!(result, "ok");
}

#[test]
fn create_image_bitmap_takes_all_sources() {
    let (.., result) = offscreen_page(
        r#"
        const sample = new OffscreenCanvas(2, 1); sample.getContext('2d').fillRect(0, 0, 1, 1);
        const png = new Uint8Array(await (await sample.convertToBlob()).arrayBuffer());
        const fromBlob = await createImageBitmap(new Blob([png], { type: 'image/png' }));
        same([fromBlob.width, fromBlob.height, fromBlob instanceof ImageBitmap, String(fromBlob)], [2, 1, true, '[object ImageBitmap]'], 'blob');
        await rejected(() => createImageBitmap(new Blob([new Uint8Array(4)])), 'InvalidStateError');
        const svg = await createImageBitmap(new Blob(['<svg xmlns="http://www.w3.org/2000/svg" width="3" height="2"><rect width="3" height="2" fill="blue"/></svg>'], { type: 'image/svg+xml' }));
        same([svg.width, svg.height, rgba(svg, 1, 1)], [3, 2, '0,0,255,255'], 'svg');

        const data = new ImageData(new Uint8ClampedArray([255,0,0,255, 0,255,0,255, 0,0,255,255, 255,255,255,255]), 2, 2);
        const bitmap = await createImageBitmap(data);
        same([bitmap.width, bitmap.height, rgba(bitmap, 0, 0), rgba(bitmap, 1, 0), rgba(bitmap, 0, 1), rgba(bitmap, 1, 1)], [2, 2, '255,0,0,255', '0,255,0,255', '0,0,255,255', '255,255,255,255'], 'ImageData is exact');
        const crop = await createImageBitmap(data, 1, 0, 1, 2);
        same([crop.width, crop.height, rgba(crop, 0, 0), rgba(crop, 0, 1)], [1, 2, '0,255,0,255', '255,255,255,255'], 'crop');
        const negative = await createImageBitmap(data, 2, 2, -1, -1);
        same([negative.width, negative.height, rgba(negative)], [1, 1, '255,255,255,255'], 'negative crop');
        const outside = await createImageBitmap(data, -1, 0, 2, 1);
        same([outside.width, rgba(outside, 0, 0), rgba(outside, 1, 0)], [2, '0,0,0,0', '255,0,0,255'], 'a crop outside the image is transparent');
        const flipped = await createImageBitmap(data, { imageOrientation: 'flipY' });
        same(rgba(flipped, 0, 0), '0,0,255,255', 'flipY');
        const large = await createImageBitmap(data, { resizeWidth: 4, resizeQuality: 'pixelated' });
        same([large.width, large.height, rgba(large, 3, 3)], [4, 4, '255,255,255,255'], 'resize');
        const small = await createImageBitmap(data, { resizeHeight: 1 });
        same([small.width, small.height], [1, 1], 'resize keeps the ratio');
        const copy = await createImageBitmap(bitmap);
        const canvas = new OffscreenCanvas(1, 1), context = canvas.getContext('2d'); context.fillStyle = '#123456'; context.fillRect(0, 0, 1, 1);
        const fromCanvas = await createImageBitmap(canvas); context.fillStyle = '#ffffff'; context.fillRect(0, 0, 1, 1);
        same([rgba(copy), rgba(fromCanvas)], ['255,0,0,255', '18,52,86,255'], 'bitmaps copy their source');
        const element = document.createElement('canvas'); element.width = element.height = 1; element.getContext('2d').fillRect(0, 0, 1, 1);
        same(rgba(await createImageBitmap(element)), '0,0,0,255', 'canvas element');
        const image = new Image(); image.src = 'data:image/png;base64,' + btoa(String.fromCharCode(...png));
        await rejected(() => createImageBitmap(new Image()), 'InvalidStateError');
        await image.decode();
        same((await createImageBitmap(image)).width, 2, 'image element');
        await rejected(() => createImageBitmap(document.createElement('video')), 'InvalidStateError');

        await rejected(() => createImageBitmap({}), 'TypeError');
        await rejected(() => createImageBitmap(data, 0, 0), 'TypeError');
        await rejected(() => createImageBitmap(data, 0, 0, 0, 1), 'RangeError');
        await rejected(() => createImageBitmap(data, { resizeWidth: 0 }), 'InvalidStateError');
        await rejected(() => createImageBitmap(new OffscreenCanvas(0, 1)), 'InvalidStateError');
        try { new ImageBitmap(); throw Error('constructor'); } catch (error) { if (!(error instanceof TypeError)) throw error; }

        const target = new OffscreenCanvas(2, 2), drawing = target.getContext('2d');
        drawing.drawImage(bitmap, 0, 0); bitmap.close();
        same([...drawing.getImageData(1, 1, 1, 1).data], [255, 255, 255, 255], 'a draw before close() keeps its pixels');
        same([bitmap.width, bitmap.height], [0, 0], 'close');
        bitmap.close();
        try { drawing.drawImage(bitmap, 0, 0); throw Error('drew a closed bitmap'); } catch (error) { if (error.name !== 'InvalidStateError') throw error; }
        await rejected(() => createImageBitmap(bitmap), 'InvalidStateError');

        const shown = document.createElement('canvas'); shown.width = shown.height = 2;
        const renderer = shown.getContext('bitmaprenderer');
        const source = await createImageBitmap(data);
        renderer.transferFromImageBitmap(source);
        same(source.width, 0, 'the bitmap moves');
        same(shown.toDataURL().startsWith('data:image/png'), true, 'bitmaprenderer shows');
        const read = shown.ownerDocument.createElement('canvas'); read.width = read.height = 2; const reader = read.getContext('2d'); reader.drawImage(shown, 0, 0);
        same([...reader.getImageData(1, 0, 1, 1).data], [0, 255, 0, 255], 'bitmaprenderer pixels');
        renderer.transferFromImageBitmap(null);
        reader.clearRect(0, 0, 2, 2); reader.drawImage(shown, 0, 0);
        same([...reader.getImageData(1, 0, 1, 1).data], [0, 0, 0, 0], 'bitmaprenderer clears');
        try { renderer.transferFromImageBitmap({}); throw Error('object'); } catch (error) { if (!(error instanceof TypeError)) throw error; }
    "#,
    );
    assert_eq!(result, "ok");
}

#[test]
fn huge_svg_images_draw_at_their_canvas_size() {
    let (.., result) = offscreen_page(
        r#"
        const huge = new Image(); huge.src = 'data:image/svg+xml,' + encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" width="30000" height="30000"><rect width="15000" height="30000" fill="red"/><rect x="15000" width="15000" height="30000" fill="blue"/></svg>');
        await huge.decode();
        same([huge.naturalWidth, huge.naturalHeight], [30000, 30000], 'natural size');
        const target = new OffscreenCanvas(10, 10), drawing = target.getContext('2d');
        drawing.drawImage(huge, 0, 0, 10, 10);
        same([...drawing.getImageData(2, 5, 1, 1).data, ...drawing.getImageData(7, 5, 1, 1).data], [255, 0, 0, 255, 0, 0, 255, 255], 'whole');
        drawing.drawImage(huge, 15000, 0, 15000, 30000, 0, 0, 10, 10);
        same([...drawing.getImageData(2, 5, 1, 1).data], [0, 0, 255, 255], 'part');
        drawing.drawImage(huge, 0, 0, 10, 10); drawing.drawImage(huge, 0, 0, 10, 10);
        same([...drawing.getImageData(2, 5, 1, 1).data], [255, 0, 0, 255], 'again');
    "#,
    );
    assert_eq!(result, "ok");
}

#[test]
fn page_workers_report_errors_and_get_files() {
    let (.., result) = offscreen_page(
        r#"
        const inner = worker(`onmessage = ({ data }) => {
            if (data === 'throw') throw new TypeError('bad');
            if (data === 'later') return setTimeout(() => { throw new Error('late'); });
            if (data === 'handled') { self.onerror = () => true; throw new Error('kept'); }
            if (data.shared) return postMessage(Atomics.add(new Int32Array(data.shared), 0, 2));
            if (data.move) return postMessage(data, [data.move]);
            if (data.file) return data.file.text().then(text => postMessage([data.file instanceof File, data.file.name, data.file.type, text, data.blob instanceof Blob && !(data.blob instanceof File), data.blob.size]));
            postMessage(data);
        }`);
        const failed = () => new Promise(resolve => inner.addEventListener('error', event => resolve([event instanceof ErrorEvent, event.message, event.lineno > 0]), { once: true }));
        const answer = (data, transfer) => new Promise(resolve => { inner.onmessage = event => resolve(event.data); inner.postMessage(data, transfer); });
        let failure = failed(); inner.postMessage('throw');
        same(await failure, [true, 'Uncaught TypeError: bad', true], 'error in a handler');
        failure = failed(); inner.postMessage('later');
        same(await failure, [true, 'Uncaught Error: late', true], 'error in a timer');
        let seen = 0; inner.addEventListener('error', () => seen++); inner.postMessage('handled');
        same([await answer('ping'), seen], ['ping', 0], 'the worker lives on and keeps the errors that it handles');
        same(await answer({ file: new File(['hello'], 'a.txt', { type: 'text/plain' }), blob: new Blob(['xy']) }), [true, 'a.txt', 'text/plain', 'hello', true, 2], 'files');
        const pixels = await answer(new ImageData(new Uint8ClampedArray([7, 0, 0, 255]), 1, 1));
        same([pixels instanceof ImageData, pixels.width, pixels.height, pixels.data[0]], [true, 1, 1, 7], 'image data');
        const shared = new SharedArrayBuffer(8);
        same([await answer({ shared }), new Int32Array(shared)[0]], [0, 2], 'shared memory');
        await rejected(async () => Atomics.wait(new Int32Array(shared), 0, 0, 1), 'TypeError');
        const moved = new ArrayBuffer(65536); new Uint8Array(moved)[1] = 5;
        const back = (await answer({ move: moved }, [moved])).move;
        same([moved.byteLength, back.byteLength, new Uint8Array(back)[1]], [0, 65536, 5], 'moved memory');
        const missing = new Worker('apps:///missing.js');
        same(await new Promise(resolve => { missing.onerror = event => resolve(event.message); }), 'Missing bundled worker: missing.js', 'a missing worker fails with an event');
    "#,
    );
    assert_eq!(result, "ok");
}

#[test]
fn bitmaps_and_canvases_move_between_threads() {
    let (host, engine, result) = offscreen_page(
        r#"
        const data = new ImageData(new Uint8ClampedArray([255,0,0,128, 0,255,0,255]), 2, 1);
        const original = await createImageBitmap(data);
        const before = rgba(original);
        const cloned = structuredClone(original);
        same([cloned.width, rgba(cloned)], [2, before], 'structured clone');
        const moved = structuredClone(original, { transfer: [original] });
        same([original.width, moved.width, rgba(moved)], [0, 2, before], 'structured clone transfer');
        try { structuredClone(moved, { transfer: [moved, moved] }); throw Error('twice'); } catch (error) { if (error.name !== 'DataCloneError') throw error; }

        const echo = worker(`onmessage = async ({ data }) => {
          const canvas = new OffscreenCanvas(data.width, data.height), context = canvas.getContext('2d');
          context.drawImage(data, 0, 0);
          const pixel = [...context.getImageData(0, 0, 1, 1).data];
          context.fillStyle = '#0000ff'; context.fillRect(1, 0, 1, 1);
          const bitmap = canvas.transferToImageBitmap(), blob = await createImageBitmap(await new OffscreenCanvas(2, 1).convertToBlob());
          const text = new OffscreenCanvas(32, 16), writer = text.getContext('2d'); writer.fillText('Hi', 2, 12);
          const ink = writer.getImageData(0, 0, 32, 16).data.some(value => value), width = writer.measureText('Hi').width;
          await new Promise(requestAnimationFrame);
          postMessage({ pixel, bitmap, sizes: [blob.width, blob.height], ink, width }, [bitmap]);
        };`);
        const answer = reply(echo);
        echo.postMessage(moved, [moved]);
        same(moved.width, 0, 'transfer detaches');
        const { pixel, bitmap, sizes, ink, width } = await answer;
        same([pixel, rgba(bitmap, 1, 0), sizes], [[...before.split(',').map(Number)].map((v, i) => i < 3 ? pixel[i] : v), '0,0,255,255', [2, 1]], 'worker bitmap');
        if (!ink || !(width > 0)) throw Error('worker text: ' + ink + ' ' + width);
        await echo.terminate();

        const shown = document.getElementById('shown');
        const offscreen = shown.transferControlToOffscreen();
        same([offscreen.width, offscreen.height], [8, 8], 'placeholder size');
        for (const run of [() => shown.getContext('2d'), () => { shown.width = 1; }, () => shown.transferControlToOffscreen()]) { try { run(); throw Error('placeholder was usable'); } catch (error) { if (error.name !== 'InvalidStateError') throw error; } }
        try { worker('').postMessage(offscreen); throw Error('copied'); } catch (error) { if (error.name !== 'DataCloneError') throw error; }
        const drawer = worker(`onmessage = ({ data }) => {
          const context = data.getContext('2d'); context.fillStyle = '#00ff00'; context.fillRect(0, 0, 16, 4);
          data.width = 16; data.height = 4;
          context.fillStyle = '#00ff00'; context.fillRect(0, 0, 16, 4);
          postMessage([data.width, data.height]);
        };`);
        const drawn = reply(drawer);
        drawer.postMessage(offscreen, [offscreen]);
        same([offscreen.width, await drawn], [0, [16, 4]], 'transfer to a worker');
        for (let i = 0; i < 2000 && shown.width !== 16; i++) await new Promise(resolve => setTimeout(resolve, 1));
        same([shown.width, shown.height], [16, 4], 'the placeholder follows the frame');
        const read = new OffscreenCanvas(16, 4), reader = read.getContext('2d'); reader.drawImage(shown, 0, 0);
        same([...reader.getImageData(15, 3, 1, 1).data], [0, 255, 0, 255], 'the placeholder shows the frame');
        same(shown.toDataURL().startsWith('data:image/png'), true, 'placeholder png');
        globalThis.placeholder = shown._id;
        await drawer.terminate();

        const local = document.createElement('canvas'); local.width = local.height = 2; document.body.append(local);
        const same_thread = local.transferControlToOffscreen(), ownContext = same_thread.getContext('2d');
        ownContext.fillStyle = '#ff00ff'; ownContext.fillRect(0, 0, 2, 2); ownContext._flush();
        globalThis.local = local._id;
    "#,
    );
    assert_eq!(result, "ok");
    let node = |name: &str| -> usize { engine.eval(name, "test").unwrap().parse().unwrap() };
    let host = host.borrow();
    let dom = &host.dom[&1];
    assert_eq!(
        dom.media.images[&node("placeholder")].size().width as u32,
        16
    );
    assert!(host.pages[&1].dirty);
    let local = &dom.media.canvases[&node("local")];
    assert_eq!(local.image.bytes()[..4], [255, 0, 255, 255]);
}

#[test]
fn bitmaps_free_their_pixels() {
    let (host, engine, result) = offscreen_page(
        r#"
        globalThis.closed = await createImageBitmap(new ImageData(1, 1));
        globalThis.closedId = closed._id;
        closed.close();
    "#,
    );
    assert_eq!(result, "ok");
    let id: usize = engine.eval("closedId", "test").unwrap().parse().unwrap();
    assert!(!host.borrow().dom[&1].media.canvases.contains_key(&id));
    host.borrow_mut().pages.get_mut(&1).unwrap().dirty = false;
    engine.eval("globalThis.kept = new OffscreenCanvas(1, 1); kept.getContext('2d').fillRect(0, 0, 1, 1); kept._context._flush();", "test").unwrap();
    let kept: usize = engine.eval("kept._id", "test").unwrap().parse().unwrap();
    {
        let host = host.borrow();
        assert!(!host.pages[&1].dirty);
        assert!(!host.dom[&1].media.canvases[&kept].present);
        assert_eq!(
            host.dom[&1].media.canvases[&kept].pixels.pixels()[0].alpha(),
            255
        );
    }
    engine
        .eval(
            r#"globalThis.source = new OffscreenCanvas(1024, 1024); source.getContext('2d').fillRect(0, 0, 1024, 1024);
            globalThis.drop = () => { createImageBitmap(source).catch(error => console.error(error)); };"#,
            "test",
        )
        .unwrap();
    let (mut most, mut made) = (0, 0);
    let started = std::time::Instant::now();
    // A loaded machine makes few bitmaps in a debug build, so it makes at least 40.
    while started.elapsed() < std::time::Duration::from_secs(3) || made < 40 {
        engine.eval("drop()", "test").unwrap();
        made += 1;
        engine.tick().unwrap();
        most = most.max(host.borrow().dom[&1].media.canvases.len());
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        most * 2 < made,
        "{most} of {made} unclosed bitmaps stayed alive"
    );
}

#[test]
fn images_decode_on_workers_and_load_after_a_wake() {
    let (wake, woken) = std::sync::mpsc::channel();
    apps_runtime::jsc::Waker::install(move || {
        let _ = wake.send(());
    });
    let directory = tempfile::tempdir().unwrap();
    std::fs::copy(
        "tests/images/sample.png",
        directory.path().join("sample.png"),
    )
    .unwrap();
    image::GrayImage::new(16385, 1)
        .save(directory.path().join("wide.png"))
        .unwrap();
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<body></body>"),
            dirty: false,
        },
    );
    let base = url::Url::from_directory_path(directory.path()).unwrap();
    host.borrow_mut().dom.entry(1).or_default().base = Some(base.clone());
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    let settle = |count: usize| {
        let started = std::time::Instant::now();
        while engine
            .eval("events.length", "test")
            .unwrap()
            .parse::<usize>()
            .unwrap()
            < count
        {
            woken
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            engine.tick().unwrap();
            assert!(started.elapsed() < std::time::Duration::from_secs(10));
        }
        engine
            .eval("JSON.stringify(events.splice(0).sort())", "test")
            .unwrap()
    };
    engine.eval(&format!(r#"
        globalThis.base = {}; globalThis.events = [];
        globalThis.image = new Image(); globalThis.wide = new Image(); document.body.append(image, wide);
        image.addEventListener('load', () => events.push('load:' + image.naturalWidth + ':' + image.complete));
        image.addEventListener('error', () => events.push('error'));
        wide.addEventListener('load', () => events.push('wide:load'));
        wide.addEventListener('error', () => events.push('wide:error:' + wide.complete + ':' + wide.naturalWidth));
        image.src = base + 'sample.png'; wide.src = base + 'wide.png';
        image.decode().then(() => events.push('decoded:' + image.naturalHeight));
        if (image.complete || wide.complete || image.naturalWidth || events.length) throw Error('Image loaded synchronously');
    "#, serde_json::json!(base.as_str())), "test").unwrap();
    host.borrow_mut().pages.get_mut(&1).unwrap().dirty = false;
    assert_eq!(
        settle(3),
        r#"["decoded:3","load:4:true","wide:error:true:0"]"#
    );
    let id: usize = engine.eval("image._id", "test").unwrap().parse().unwrap();
    {
        let host = host.borrow();
        assert!(host.pages[&1].dirty && host.dom[&1].media.images.contains_key(&id));
        assert!(host.dom[&1].media.retired.is_empty());
    }
    engine.eval(r#"
        const svg = 'data:image/svg+xml,' + encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" width="30" height="12"></svg>');
        image.src = base + 'wide.png'; image.src = base + 'sample.png'; image.src = svg;
    "#, "test").unwrap();
    assert_eq!(settle(1), r#"["load:30:true"]"#);
    engine
        .eval("image.src = base + 'sample.png'", "test")
        .unwrap();
    assert_eq!(settle(1), r#"["load:4:true"]"#);
    engine
        .eval("image.src = base + 'missing.png'", "test")
        .unwrap();
    assert_eq!(settle(1), r#"["error"]"#);
    let host = host.borrow();
    assert_eq!(host.dom[&1].media.retired.len(), 1);
    assert!(host.dom[&1].media.images.contains_key(&id));
}
