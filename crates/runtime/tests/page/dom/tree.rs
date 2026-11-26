use super::*;

#[test]
fn svg_presentation_attributes_links_and_attribute_nodes() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow().store.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><head><style>.c{fill:blue}</style></head><body><svg><g style='fill:red;stroke:blue'><path id=a fill=none /><path id=b stroke-width=5 /><path id=c class=c fill=green /></g><g fill=green><path id=d /></g></svg><p id=p fill=red></p></body></html>"),
        dirty: false, ..Default::default()
    });
    let engine = JsContext::new(JavaScriptCore::load().unwrap(), host, Role::Renderer(1)).unwrap();
    engine.eval(r#"
        const style = (id, name) => getComputedStyle(document.getElementById(id)).getPropertyValue(name);
        const got = [style('a', 'fill'), style('b', 'stroke-width'), style('c', 'fill'), style('d', 'fill'), style('p', 'fill')].join('|');
        if (got !== 'none|5px|rgb(0, 0, 255)|rgb(0, 128, 0)|rgb(0, 0, 0)') throw Error('presentation ' + got);
        const link = document.createElement('link');
        link.rel = 'stylesheet'; link.href = '/x.css';
        if (link.rel !== 'stylesheet' || link.getAttribute('rel') !== 'stylesheet') throw Error('rel');
        globalThis.failed = 0; link.onerror = () => failed++;
        document.head.append(link);
        const attr = link.getAttributeNode('rel');
        if (attr.value !== 'stylesheet' || link.removeAttributeNode(attr) !== attr || link.hasAttribute('rel') || attr.ownerElement !== null) throw Error('attribute node');
        let thrown = false; try { link.removeAttributeNode(attr); } catch (error) { thrown = error.name === 'NotFoundError'; }
        if (!thrown || document.currentScript !== null) throw Error('attribute node removed twice');
    "#, "test").unwrap();
    for _ in 0..10 {
        engine.tick().unwrap();
    }
    assert_eq!(engine.eval("failed", "test").unwrap(), "1");
}

#[test]
fn core_dom_interfaces_follow_the_standard() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow().store.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><body><div id='a' z='1' b='2' class='x y'>t<!--c-->u</div><template id='t'><p>hi</p></template></body></html>"),
        dirty: false, ..Default::default()
    });
    let engine = JsContext::new(JavaScriptCore::load().unwrap(), host, Role::Renderer(1)).unwrap();
    engine.eval(r#"
      const fails = [];
      const check = (name, fn) => { try { if (!fn()) fails.push(name); } catch (e) { fails.push(name + ': ' + e.message); } };
      const a = document.getElementById('a');
      check('text instanceof Text', () => a.firstChild instanceof Text && !(a instanceof Text));
      check('comment instanceof Comment', () => a.childNodes[1] instanceof Comment && !(a.firstChild instanceof Comment));
      check('CharacterData', () => a.firstChild instanceof CharacterData);
      check('new Text', () => new Text('hi').data === 'hi');
      check('new Comment', () => new Comment('hi').data === 'hi');
      check('new DocumentFragment', () => new DocumentFragment().nodeType === 11);
      check('EventTarget', () => a instanceof EventTarget);
      check('element.length', () => a.length === undefined);
      check('NodeList', () => a.childNodes instanceof NodeList && document.querySelectorAll('div') instanceof NodeList);
      check('HTMLCollection', () => a.children instanceof HTMLCollection);
      check('qsa item', () => document.querySelectorAll('div').item(0) === a);
      check('document.children', () => document.children.length === 1 && document.firstElementChild === document.documentElement);
      check('fragment.firstElementChild', () => { const f = document.createDocumentFragment(); f.append(document.createElement('i')); return f.firstElementChild?.localName === 'i' && f.childElementCount === 1; });
      check('attr order', () => a.getAttributeNames?.().join() === 'id,z,b,class');
      check('attributes order', () => [...a.attributes].map(x => x.name).join() === 'id,z,b,class');
      check('hasAttributes', () => a.hasAttributes());
      check('attributes named', () => a.attributes.id?.value === 'a' && a.attributes.getNamedItem('z').ownerElement === a);
      check('classList.length', () => a.classList.length === 2 && a.classList.item(1) === 'y' && a.classList.value === 'x y' && String(a.classList) === 'x y');
      check('classList.forEach', () => { let n = 0; a.classList.forEach(() => n++); return n === 2; });
      check('classList identity', () => a.classList === a.classList);
      check('DOMTokenList', () => a.classList instanceof DOMTokenList);
      check('classList index', () => a.classList[0] === 'x' && a.classList[1] === 'y' && a.classList[2] === undefined && 1 in a.classList && !(2 in a.classList));
      check('textContent skips comments', () => a.textContent === 'tu');
      check('template content', () => { const t = document.getElementById('t'); return t.content instanceof DocumentFragment && t.content.firstChild?.localName === 'p' && t.childNodes.length === 0; });
      check('template innerHTML', () => { const t = document.createElement('template'); t.innerHTML = '<b>x</b>'; return t.content.firstChild.localName === 'b' && t.innerHTML === '<b>x</b>' && t.cloneNode(true).content.firstChild?.localName === 'b'; });
      check('HTMLTemplateElement', () => document.getElementById('t') instanceof HTMLTemplateElement);
      check('HTMLDivElement', () => a instanceof HTMLDivElement && document.createElement('button') instanceof HTMLButtonElement);
      check('setAttribute lowercase', () => { const d = document.createElement('div'); d.setAttribute('dataFoo', '1'); return d.getAttribute('datafoo') === '1' && d.hasAttribute('DATAFOO'); });
      check('insertAdjacentElement', () => { const d = document.createElement('div'), s = document.createElement('span'); d.insertAdjacentElement('afterbegin', s); d.insertAdjacentText('beforeend', 'x'); return d.firstChild === s && d.lastChild.data === 'x'; });
      check('fragment textContent', () => { const f = document.createDocumentFragment(); f.textContent = 'x'; return f.textContent === 'x'; });
      check('document textContent', () => document.textContent === null);
      check('replaceChild same', () => { const d = document.createElement('div'), s = document.createElement('span'); d.append(s); d.replaceChild(s, s); return s.parentNode === d; });
      check('dataset in', () => 'foo' in Object.assign(document.createElement('div'), {}).dataset === false && (() => { const d = document.createElement('div'); d.dataset.fooBar = '1'; return 'fooBar' in d.dataset; })());
      check('style.cssFloat', () => { const d = document.createElement('div'); d.style.cssFloat = 'left'; return d.style.getPropertyValue('float') === 'left'; });
      check('style data url', () => { const d = document.createElement('div'); d.style.backgroundImage = 'url(data:image/png;base64,AA)'; d.style.color = 'red'; d.style.removeProperty('background-image'); return d.style.cssText.trim() === 'color:red' || d.style.cssText.trim() === 'color: red;'; });
      check('Node consts', () => Node.ATTRIBUTE_NODE === 2 && Node.DOCUMENT_TYPE_NODE === 10 && Node.CDATA_SECTION_NODE === 4);
      check('NodeFilter consts', () => NodeFilter.SHOW_DOCUMENT_FRAGMENT === 1024 && NodeFilter.SHOW_ATTRIBUTE === 2);
      check('treewalker methods', () => { const w = document.createTreeWalker(a, NodeFilter.SHOW_ALL); return w.firstChild() === a.firstChild && w.nextSibling() === a.childNodes[1] && w.previousSibling() === a.firstChild && w.parentNode() === a && w.lastChild() === a.lastChild && w.previousNode() === a.childNodes[1] && w.previousNode() === a.firstChild && w.previousNode() === a && w.previousNode() === null; });
      check('nodeiterator', () => { const it = document.createNodeIterator(a, NodeFilter.SHOW_TEXT); return it.nextNode() === a.firstChild && it.nextNode() === a.lastChild && it.nextNode() === null && it.previousNode() === a.lastChild; });
      check('XMLSerializer', () => new XMLSerializer().serializeToString(a).startsWith('<div'));
      check('document.URL', () => typeof document.URL === 'string' && document.location === location && document.characterSet === 'UTF-8' && document.contentType === 'text/html');
      check('scrollingElement', () => document.scrollingElement === document.documentElement);
      check('createEvent CustomEvent', () => document.createEvent('CustomEvent') instanceof CustomEvent);
      check('input type default', () => document.createElement('input').type === 'text');
      check('Attr', () => typeof Attr === 'function' && typeof NamedNodeMap === 'function');
      check('window capture first', () => { const order = []; const d = document.createElement('div'); document.body.append(d); const w = () => order.push('window'), b = () => order.push('body'); addEventListener('x', w, true); document.body.addEventListener('x', b, true); d.dispatchEvent(new Event('x', { bubbles: true })); removeEventListener('x', w, true); d.remove(); return order.join() === 'window,body'; });
      check('passive', () => { const d = document.createElement('div'); d.addEventListener('x', e => e.preventDefault(), { passive: true }); return d.dispatchEvent(new Event('x', { cancelable: true })) === true; });
      check('click toggles checkbox', () => { const c = document.createElement('input'); c.type = 'checkbox'; document.body.append(c); let changed = 0; c.addEventListener('change', () => changed++); c.click(); const ok = c.checked && changed === 1; c.remove(); return ok; });
      check('getElementsByName', () => typeof document.getElementsByName === 'function');
      check('parsed comments', () => { const d = document.createElement('div'); d.innerHTML = '<p>a<!--x-->b<?lit$1$></p><!DOCTYPE html>'; return d.innerHTML === '<p>a<!--x-->b<!--?lit$1$--></p>' && d.firstChild.childNodes[1] instanceof Comment && d.firstChild.textContent === 'ab'; });
      check('getElementById quotes', () => { const ids = ['a"b', "a'b", 'a(b)', 'a\\b', 'a b']; const d = document.createElement('div'); document.body.append(d); for (const id of ids) { const e = document.createElement('i'); e.id = id; d.append(e); } const ok = ids.every(id => document.getElementById(id)?.id === id); d.remove(); return ok; });
      check('getElementsByClassName tailwind', () => { const d = document.createElement('div'); d.innerHTML = '<i class="md:flex w-1/2 x"></i><i class="x"></i>'; return d.getElementsByClassName('md:flex w-1/2').length === 1 && d.getElementsByClassName('x').length === 2 && d.getElementsByClassName(' ').length === 0; });
      check('shadow retarget', () => { const host = document.createElement('div'); document.body.append(host); const root = host.attachShadow({ mode: 'open' }); root.innerHTML = '<b>x</b>'; const seen = []; const fn = e => seen.push(e.target === host, e.composedPath()[0] === root.firstChild); document.addEventListener('ping', fn); root.addEventListener('ping', e => seen.push(e.target === root.firstChild)); root.firstChild.dispatchEvent(new Event('ping', { bubbles: true, composed: true })); document.removeEventListener('ping', fn); host.remove(); return seen.join() === 'true,true,true'; });
      check('window currentTarget', () => { let ok = false; const fn = e => { ok = e.currentTarget === window && e.eventPhase === 3; }; addEventListener('pong', fn); document.body.dispatchEvent(new Event('pong', { bubbles: true })); removeEventListener('pong', fn); return ok; });
      check('redispatch throws', () => { const d = document.createElement('div'), e = new Event('x'); let threw = false; d.addEventListener('x', () => { try { d.dispatchEvent(e); } catch { threw = true; } }); d.dispatchEvent(e); return threw; });
      check('lit-like template', () => { const t = document.createElement('template'); t.innerHTML = '<div a$lit$="x" class="c"><?lit$1$></div>'; const f = document.importNode(t.content, true), w = document.createTreeWalker(f, 133); w.currentNode = f; const seen = []; let n; while ((n = w.nextNode())) seen.push(n.nodeType === 1 ? n.getAttributeNames().join('+') : n.nodeType + ':' + n.data); return seen.join() === 'a$lit$+class,8:?lit$1$'; });
      check('custom element via innerHTML', () => { class XProbe extends HTMLElement { connectedCallback() { this.dataset.on = '1'; } } customElements.define('x-probe', XProbe); const d = document.createElement('div'); document.body.append(d); d.innerHTML = '<x-probe></x-probe>'; const ok = d.firstChild instanceof XProbe && d.firstChild.dataset.on === '1'; d.remove(); return ok; });
      check('template inert', () => document.querySelectorAll('p').length === 0 && document.getElementById('t').content.querySelectorAll('p').length === 1);
      check('svg serialize', () => { const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg'); svg.setAttribute('viewBox', '0 0 1 1'); return new XMLSerializer().serializeToString(svg) === '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1 1"/>'; });
      check('style quotes', () => { const d = document.createElement('div'); d.style.cssText = 'content: "a;b"; color: red'; return d.style.length === 2 && d.style.content === '"a;b"' && d.style.color === 'red'; });
      check('isConnected', () => a.isConnected && !document.createElement('div').isConnected);
      check('ownerDocument', () => document.createElement('div').ownerDocument === document);
      check('getRootNode', () => a.getRootNode() === document);
      check('normalize', () => { const d = document.createElement('div'); d.append('a', 'b'); d.normalize(); return d.childNodes.length === 1 && d.textContent === 'ab'; });
      check('outerHTML', () => { const d = document.createElement('div'); d.innerHTML = '<span a="1" b="2">x</span>'; return d.innerHTML === '<span a="1" b="2">x</span>'; });
      check('splitText', () => { const d = document.createElement('div'); d.append('hello'); const t = d.firstChild.splitText(2); return d.childNodes.length === 2 && t.data === 'llo'; });
      check('wholeText', () => { const d = document.createElement('div'); d.append('a', 'b'); return d.firstChild.wholeText === 'ab'; });
      check('closest', () => a.firstChild.parentElement.closest('body') === document.body);
      check('compareDocumentPosition', () => (a.compareDocumentPosition(a.firstChild) & Node.DOCUMENT_POSITION_CONTAINED_BY) !== 0);
      check('implementation', () => typeof document.implementation.createHTMLDocument === 'function' && document.implementation.hasFeature());
      if (fails.length) throw Error(fails.join('\n'));
    "#, "test").unwrap();
}

#[test]
fn dom_matrix_math_keeps_nan_infinity_negative_zero_and_2d() {
    let (.., result) = offscreen_page(
        r#"
        const bits = m => (m._m ?? m).map(v => Object.is(v, -0) ? '-0' : String(v)).join();
        const mul = (a, m) => a.map((_, i) => a[i & 3] * m[i & 12] + a[4 + (i & 3)] * m[(i & 12) + 1] + a[8 + (i & 3)] * m[(i & 12) + 2] + a[12 + (i & 3)] * m[(i & 12) + 3]);
        const a = [1, -0, 0, 0, Infinity, 2, 0, 0, 0, 0, 1, 0, NaN, 3, 0, 1], b = [-0, 1, 0, 0, 2, Infinity, 0, 0, 0, 0, 1, 0, 0, -1, 0, 1];
        same(bits(new DOMMatrix(a).multiply(new DOMMatrix(b))), bits(mul(a, b)), 'multiply');
        same(bits(new DOMMatrix(a).multiply({ a: -0, b: 1, c: 2, d: Infinity, e: 0, f: -1 })), bits(mul(a, b)), 'multiply dictionary');
        same(bits(new DOMMatrix(a).translate(Infinity, -0, 0)), bits(mul(a, [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, Infinity, -0, 0, 1])), 'translate');
        same(bits(new DOMMatrix(a).scale(-0, NaN, 1, 1, 2)), bits(mul(mul(mul(a, [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 1, 2, 0, 1]), [-0, 0, 0, 0, 0, NaN, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]), [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, -1, -2, -0, 1])), 'scale');
        const m = new DOMMatrix(a); m.multiplySelf(m); same(bits(m), bits(mul(a, a)), 'self');
        same([new DOMMatrix().translate(1, 2, -0).is2D, new DOMMatrix().translate(1, 2, '0').is2D, String(new DOMMatrix().translate('3', '4')), new DOMMatrix().scale(2, 3, 1).is2D, new DOMMatrix().scale(2, 3, '1').is2D, new DOMMatrix().scale(2, 3, 2).is2D],
          [true, false, 'matrix(1, 0, 0, 1, 3, 4)', true, false, false], 'is2D');
        same([new DOMMatrix().rotate(30).is2D, new DOMMatrix().rotateAxisAngle(1, 0, 0, 90).is2D, new DOMMatrix().skewX(10).is2D, new DOMMatrix([1, 0, 0, 1, 0, 0]).multiply({ m33: 2 }).is2D, new DOMMatrix([1, 0, 0, 1, 0, 0]).multiply({ m33: 2, is2D: true }).is2D, new DOMMatrix().translate(0, 0, 1).multiply(new DOMMatrix()).is2D],
          [true, false, true, false, true, false], 'is2D products');
    "#,
    );
    assert_eq!(result, "ok");
}

#[test]
fn sibling_walks_follow_mutations() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow().store.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<body><ul></ul><ol><li>a</li><li>b</li></ol></body>"),
            dirty: false,
            ..Default::default()
        },
    );
    let engine = JsContext::new(JavaScriptCore::load().unwrap(), host, Role::Renderer(1)).unwrap();
    engine.eval(r#"
        const list = document.querySelector('ul'), other = document.querySelector('ol');
        for (let i = 0; i < 6; i++) { const li = document.createElement('li'); li.id = 'i' + i; li.append('x'); list.append(li); }
        const seen = [];
        for (let n = list.firstChild; n; n = n.nextSibling) {
            seen.push(n.id + (n.firstChild.nextSibling ? '!' : '') + other.firstChild.nextSibling.textContent);
            if (n.id === 'i1') list.removeChild(n.previousSibling);
            if (n.id === 'i3') list.insertBefore(document.createElement('b'), n);
            if (n.id === 'i4') list.append(n.previousSibling);
        }
        const back = [];
        for (let n = list.lastChild; n; n = n.previousSibling) back.push(n.id || n.localName);
        if (seen.join() !== 'i0b,i1b,i2b,i3b,i4b,i5b,i3b' || back.join() !== 'i3,b,i5,i4,b,i2,i1') throw Error(seen + ' ' + back);
    "#, "test").unwrap();
}

#[test]
fn ranges_and_iterators_follow_moved_merged_and_removed_nodes() {
    checked_page(
        "<html><body><p id=p></p><div id=d><b>x</b><u>m</u><i>yz</i>w</div><div id=s><b>x</b><i>y</i></div><div id=r><a></a><b></b><i></i></div></body></html>",
        r#"
        const $ = id => document.getElementById(id), p = $('p'), d = $('d'), s = $('s'), r = $('r');
        const a = document.createTextNode('abc'), b = document.createTextNode('def'); p.append(a, b);
        const inner = document.createRange(), between = document.createRange(), wide = document.createRange();
        inner.setStart(b, 1); inner.setEnd(b, 2); between.setStart(p, 1); wide.setStart(a, 1); wide.setEnd(p, 2);
        getSelection().collapse(b, 2);
        p.normalize();
        check('normalize', () => p.childNodes.length === 1 && inner.startContainer === a && inner.startOffset === 4 && inner.endOffset === 5 && between.startContainer === a && between.startOffset === 3 && wide.endContainer === p && wide.endOffset === 1 && getSelection().anchorNode === a && getSelection().anchorOffset === 5);
        const [bold, under, italic] = d.children, watcher = document.createRange(), range = document.createRange();
        watcher.setStart(under.firstChild, 1); range.setStart(bold.firstChild, 0); range.setEnd(italic.firstChild, 1);
        const fragment = range.extractContents();
        check('extract moves', () => fragment.children[1] === under && fragment.textContent === 'xmy' && d.innerHTML === '<b></b><i>z</i>w' && range.collapsed && range.startContainer === d && range.startOffset === 1 && watcher.startContainer === d && watcher.startOffset === 1);
        const kept = s.firstChild, around = document.createRange(), span = document.createElement('span');
        span.textContent = 'old'; around.selectNodeContents(s); around.surroundContents(span);
        check('surround moves', () => s.firstChild === span && span.firstChild === kept && span.textContent === 'xy' && around.startContainer === s && around.startOffset === 0 && around.endOffset === 1);
        check('surround partial', () => { const part = document.createRange(); part.setStart(kept.firstChild, 0); part.setEnd(span.lastChild.firstChild, 1); try { part.surroundContents(document.createElement('q')); } catch (e) { return e.name === 'InvalidStateError'; } });
        check('insertNode extends', () => { const host = document.createElement('div'); host.textContent = 'ab'; const at = document.createRange(); at.setStart(host.firstChild, 1); at.collapse(true); at.insertNode(document.createElement('em')); return host.innerHTML === 'a<em></em>b' && at.startContainer === host.firstChild && at.startOffset === 1 && at.endContainer === host && at.endOffset === 2; });
        const it = document.createNodeIterator(r, NodeFilter.SHOW_ELEMENT);
        it.nextNode(); it.nextNode();
        r.firstChild.remove();
        check('iterator after', () => it.referenceNode === r && !it.pointerBeforeReferenceNode && it.nextNode()?.localName === 'b');
        it.nextNode(); it.previousNode();
        r.children[0].remove();
        check('iterator before', () => it.referenceNode.localName === 'i' && it.pointerBeforeReferenceNode && it.nextNode()?.localName === 'i');
        r.textContent = '';
        check('iterator clear', () => it.referenceNode === r && !it.pointerBeforeReferenceNode && it.nextNode() === null);
        "#,
    );
}

#[test]
fn parser_raw_text_tables_and_document_structure() {
    checked_page(
        "<!doctype html><title>x &amp; y</title><style>a>b{}</style><p>one<p>two<table><tr><td>cell</table>a &lt; b &notin; &#x80;",
        r#"
        check('implied structure', () => document.head && document.body && document.documentElement && document.head.firstChild.localName === 'title' && document.title === 'x & y');
        const style = document.querySelector('style');
        check('style text child', () => style.firstChild?.nodeType === 3 && style.firstChild.data === 'a>b{}' && style.childNodes.length === 1);
        check('raw text serialization', () => style.outerHTML === '<style>a>b{}</style>');
        check('implied p end', () => document.querySelectorAll('p').length === 2 && document.querySelectorAll('p')[1].textContent === 'two');
        check('implied tbody', () => document.querySelector('table > tbody > tr > td')?.textContent === 'cell');
        check('entities', () => document.body.lastChild.data === 'a < b ∉ €');
        const tbody = document.querySelector('tbody'); tbody.innerHTML = '<tr><td>1</td></tr>';
        check('tbody fragment context', () => tbody.firstChild.localName === 'tr');
        const tr = document.querySelector('tr'); tr.insertAdjacentHTML('afterend', '<tr><td>2</td></tr>');
        check('insertAdjacentHTML context', () => tbody.children.length === 2 && tbody.lastChild.localName === 'tr');
        const parsed = new DOMParser().parseFromString('<title>t</title><p>x', 'text/html');
        check('DOMParser', () => parsed.head.firstChild.localName === 'title' && parsed.body.firstChild.localName === 'p' && parsed.title === 't');
        const drawing = new DOMParser().parseFromString('<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 2 2"><clipPath id="c"/><circle r="1"/></svg>', 'image/svg+xml');
        check('DOMParser SVG', () => drawing.documentElement.localName === 'svg' && drawing.documentElement.namespaceURI === 'http://www.w3.org/2000/svg' && drawing.querySelector('clipPath').nextSibling.getAttribute('r') === '1' && drawing.documentElement.getAttribute('viewBox') === '0 0 2 2');
        check('navigator', () => navigator.vendor === '' && navigator.appName === 'Netscape' && navigator.webdriver === false && navigator.appVersion === navigator.userAgent);
        const svg = document.createElement('div'); svg.innerHTML = '<svg viewbox="0 0 1 1"><lineargradient id="g"/><foreignObject><p>x</p></foreignObject></svg>';
        check('svg case', () => svg.querySelector('linearGradient')?.id === 'g' && svg.firstChild.getAttribute('viewBox') === '0 0 1 1' && svg.querySelector('foreignObject p') !== null);
        "#,
    );
}

#[test]
fn created_svg_keeps_qualified_name_case() {
    let (host, _) = checked_page(
        "<html><body><svg id='s'></svg></body></html>",
        r#"
        const NS = 'http://www.w3.org/2000/svg', s = document.getElementById('s');
        const g = document.createElementNS(NS, 'linearGradient'); g.id = 'grad'; s.append(g);
        s.append(document.createElementNS(NS, 'feGaussianBlur'));
        check('tag names', () => g.tagName === 'linearGradient' && g.localName === 'linearGradient' && g instanceof SVGElement);
        check('serialization', () => s.innerHTML === '<linearGradient id="grad"></linearGradient><feGaussianBlur></feGaussianBlur>');
        "#,
    );
    let host = host.borrow().store.clone();
    let host = host.borrow();
    let tags: Vec<_> = host.pages[&1]
        .doc
        .nodes
        .iter()
        .map(|node| node.tag.as_str())
        .collect();
    assert!(tags.contains(&"linearGradient") && tags.contains(&"feGaussianBlur"));
}

#[test]
fn every_dom_interface_has_its_string_tag() {
    checked_page(
        "<html><body><div id='d'>t</div></body></html>",
        r#"
        const tag = value => Object.prototype.toString.call(value);
        const d = document.getElementById('d');
        check('element', () => tag(d) === '[object HTMLDivElement]');
        check('text', () => tag(d.firstChild) === '[object Text]');
        check('document', () => tag(document) === '[object Document]');
        check('window', () => tag(window) === '[object Window]');
        check('collections', () => tag(d.childNodes) === '[object NodeList]' && tag(document.body.children) === '[object HTMLCollection]');
        check('others', () => tag(document.createRange()) === '[object Range]' && tag(document.createDocumentFragment()) === '[object DocumentFragment]' && tag(new MouseEvent('x')) === '[object MouseEvent]' && tag(document.createElementNS('http://www.w3.org/2000/svg', 'svg')) === '[object SVGSVGElement]');
        "#,
    );
}

#[test]
fn custom_element_reactions_and_slot_changes() {
    let (_, engine) = checked_page(
        "<html><body><div id='box'></div><div id='host'><span>a</span></div></body></html>",
        r#"
        globalThis.log = [];
        customElements.define('x-item', class extends HTMLElement { connectedCallback() { log.push('in:' + this.id); } disconnectedCallback() { log.push('out:' + this.id); } });
        customElements.define('x-mover', class extends HTMLElement { connectedCallback() { log.push('in:' + this.id); } connectedMoveCallback() { log.push('move:' + this.id); } });
        const box = document.getElementById('box');
        box.innerHTML = '<x-item id="a"></x-item>';
        box.innerHTML = '';
        check('innerHTML disconnects', () => log.join() === 'in:a,out:a');
        log.length = 0; box.innerHTML = '<x-item id="b"></x-item>'; box.textContent = 'x';
        check('textContent disconnects', () => log.join() === 'in:b,out:b');
        log.length = 0; box.innerHTML = '<x-item id="c"></x-item>'; box.replaceChildren();
        check('replaceChildren disconnects', () => log.join() === 'in:c,out:c');
        log.length = 0; box.innerHTML = '<x-item id="d"></x-item><p></p>'; box.lastChild.append(box.firstChild);
        check('move disconnects then connects', () => log.join() === 'in:d,out:d,in:d');
        log.length = 0; const mover = document.createElement('x-mover'); mover.id = 'm'; box.append(mover); box.moveBefore(mover, box.firstChild);
        check('moveBefore', () => log.join() === 'in:m,move:m' && box.firstChild === mover);
        const host = document.getElementById('host'), root = host.attachShadow({ mode: 'open' });
        root.innerHTML = '<slot id="main"></slot><slot name="side" id="side"></slot>';
        globalThis.slots = [];
        for (const slot of root.querySelectorAll('slot')) slot.addEventListener('slotchange', () => slots.push(slot.id));
        host.append(document.createElement('i'), document.createElement('i'));
        const side = document.createElement('b'); side.slot = 'side'; host.append(side);
        check('slotchange is async', () => slots.length === 0);
        "#,
    );
    engine.tick().unwrap();
    assert_eq!(engine.eval("slots.join()", "test").unwrap(), "main,side");
    engine
        .eval(
            "slots.length = 0; document.querySelector('#host b').slot = ''",
            "test",
        )
        .unwrap();
    engine.tick().unwrap();
    assert_eq!(
        engine.eval("slots.sort().join()", "test").unwrap(),
        "main,side"
    );
}

#[test]
fn child_lists_inline_shorthands_and_serialization_stay_current() {
    checked_page(
        "<html><body><div id='box'></div></body></html>",
        r#"
        const p = document.getElementById('box');
        for (let i = 0; i < 4; i++) p.append(document.createElement('span'), 't');
        const kids = p.children, nodes = p.childNodes;
        kids[0].className = 'x'; p.setAttribute('data-x', '1');
        check('lists survive attribute writes', () => kids === p.children && nodes === p.childNodes);
        for (const kid of p.children) kid.remove();
        check('lists are live', () => kids.length === 2 && nodes.length === 6 && kids === p.children && nodes === p.childNodes && kids[1] === nodes[4]);
        const q = document.createElement('div'); q.innerHTML = '<i></i>'; q.childNodes; p.append(q.firstChild);
        check('moves renew both lists', () => q.childNodes.length === 0 && p.lastChild.localName === 'i');
        p.innerHTML = '<a></a><b></b>'; check('innerHTML renews lists', () => p.children.length === 2 && p.firstChild.localName === 'a');
        p.textContent = 'x'; check('textContent renews lists', () => p.childNodes.length === 1 && p.children.length === 0);
        const d = document.createElement('div'), css = () => d.getAttribute('style');
        d.style.background = 'red';
        check('background reads back', () => d.style.background === 'red' && css() === 'background: red;' && d.style.backgroundColor === 'red');
        d.style.background = ''; check('an empty shorthand removes its longhands', () => !d.hasAttribute('style') || css() === '');
        d.style.font = 'bold 12px / 1.5 serif'; check('font reads back', () => d.style.font === 'bold 12px / 1.5 serif' && d.style.removeProperty('font') === 'bold 12px / 1.5 serif' && d.style.length === 0);
        d.style.border = 'solid'; check('border omits default parts', () => d.style.border === 'solid');
        d.removeAttribute('style'); d.style.marginBlock = '1px';
        check('logical shorthands read back without folding physical sides', () => d.style.marginBlock === '1px' && css() === 'margin-block: 1px;');
        d.removeAttribute('style'); d.style.borderInline = '1px solid red'; d.style.inlineSize = '5px';
        check('logical border and alias', () => d.style.borderInline === '1px solid red' && d.style.inlineSize === '5px' && css() === 'border-inline: 1px solid red; inline-size: 5px;');
        d.removeAttribute('style'); d.style.background = 'url(a.png) no-repeat 1px 2px / 3px, blue';
        check('background layers', () => d.style.background === 'url("a.png") 1px 2px / 3px no-repeat, blue');
        const svg = document.createElement('div'); svg.innerHTML = '<svg><style>a&lt;b</style></svg><style>a<b</style>';
        check('only HTML raw text stays raw', () => svg.innerHTML === '<svg><style>a&lt;b</style></svg><style>a<b</style>');
        const deep = document.createElement('div'); deep.innerHTML = '<div>'.repeat(20000);
        check('deep trees serialize and clone', () => deep.innerHTML.length === 220000 && deep.cloneNode(true).innerHTML.length === 220000);
        const source = document.createElement('div'); source.innerHTML = '<p id="c1" class="a">x<b>y</b><!--n--></p><style>.c1{color:red}</style><svg><g><rect/></g></svg><input value="v"><details open>d</details>';
        source.querySelector('input').value = 'typed'; source.querySelector('g').setAttribute('k', '1');
        const copy = source.cloneNode(true), holder = document.createDocumentFragment(); holder.append(copy);
        check('deep clones copy plain nodes, keep namespaces and wrapped controls', () => copy.innerHTML === source.innerHTML && holder.getElementById('c1')?.parentNode === copy && copy.querySelector('rect') instanceof SVGElement && copy.querySelector('input').value === 'typed' && copy.querySelector('details').open && copy.querySelector('style').textContent === '.c1{color:red}' && copy.matches('div') && copy.querySelector('p.a > b') === copy.querySelector('b'));
        const styled = document.createElement('div'); styled.style.cssText = 'margin: 1px; color: red'; styled.style.opacity = '0.5'; styled.style.color = 'blue'; styled.style.marginTop = '2px'; styled.style.opacity = '1';
        check('inline style writes keep the text of a full rebuild', () => styled.getAttribute('style').startsWith('margin: 2px 1px 1px;') && styled.getAttribute('style').endsWith(' opacity: 1;') && styled.style.cssText === styled.getAttribute('style'));
        const x = document.createElement('a'), y = document.createElement('a'); x.setAttribute('p', '1'); x.setAttribute('q', '2'); y.setAttribute('q', '2'); y.setAttribute('p', '1');
        check('isEqualNode ignores attribute order', () => x.isEqualNode(y) && !x.isEqualNode(document.createElement('a')));
        const [m, n] = [document.createElement('i'), document.createElement('i')], mn = m.compareDocumentPosition(n), nm = n.compareDocumentPosition(m);
        check('disconnected nodes have opposite positions', () => (mn & 33) === 33 && (nm & 33) === 33 && (mn & 6) + (nm & 6) === 6);
        const t = document.createElement('p'); t.append('a', '', 'b', document.createElement('i'), ''); const r = new Range(); r.setStart(t.childNodes[2], 1); r.setEnd(t, 3); t.normalize();
        check('normalize keeps ranges in the merged text', () => t.childNodes.length === 2 && r.startContainer === t.firstChild && r.startOffset === 2 && r.endContainer === t && r.endOffset === 1);
        check('a document takes one element', () => { try { document.appendChild(document.createElement('div')); return false; } catch (e) { return e.name === 'HierarchyRequestError' && document.children.length === 1; } });
        const html = document.documentElement, other = document.createElement('html'); document.replaceChild(other, html); other.replaceWith(html);
        check('the document element can be replaced', () => document.documentElement === html && document.children.length === 1);
        check('insertAdjacentElement takes elements', () => { try { x.insertAdjacentElement('beforeend', 'text'); return false; } catch (e) { return e instanceof TypeError && !x.hasChildNodes(); } });
        x.insertAdjacentText('beforeend', 'text'); check('insertAdjacentText', () => x.textContent === 'text');
        const node = x.getAttributeNode('p'); x.setAttribute('p', '3');
        check('attribute nodes are kept and live', () => node === x.getAttributeNode('p') && node === x.attributes.p && node.value === '3');
        "#,
    );
}

#[test]
fn mutation_records_and_live_ranges_follow_the_dom_standard() {
    let (_, engine) = checked_page(
        "<html><body><div id='p'><i id='a'>a</i><b id='b'>b</b><span id='t'>hello</span></div></body></html>",
        r#"
        const p = document.getElementById('p'), a = document.getElementById('a'), b = document.getElementById('b');
        check('observe needs a kind', () => { try { new MutationObserver(() => {}).observe(p, {}); return false; } catch (e) { return e instanceof TypeError; } });
        {
            const both = new MutationObserver(() => {}), other = new MutationObserver(() => {}), own = new MutationObserver(() => {}), sized = new ResizeObserver(() => {});
            both.observe(document.body, { attributes: true, subtree: true }); both.observe(p, { attributes: true, subtree: true, attributeOldValue: true, attributeFilter: ['title'] });
            other.observe(b, { attributes: true, childList: true, subtree: true }); own.observe(document.body, { attributes: true, childList: true }); sized.observe(a);
            a.setAttribute('title', 'x'); a.setAttribute('title', 'y'); a.setAttribute('lang', 'en'); a.textContent = 'a';
            const records = both.takeRecords();
            check('an observer of two ancestors gets one record for each change', () => records.map(record => record.attributeName + ':' + record.oldValue).join() === 'title:null,title:x,lang:null' && records.every(record => record.target === a));
            check('observers of other nodes and of a parent without subtree get nothing', () => !other.takeRecords().length && !own.takeRecords().length);
            a.removeAttribute('title'); a.removeAttribute('lang'); both.disconnect(); other.disconnect(); own.disconnect(); sized.disconnect();
        }
        const observer = new MutationObserver(() => {});
        observer.observe(p, { childList: true });
        p.insertBefore(b, a);
        const moved = observer.takeRecords();
        check('move gives removal then insertion', () => moved.length === 2 && moved[0].removedNodes[0] === b && moved[0].previousSibling === a && moved[1].addedNodes[0] === b && moved[1].previousSibling === null && moved[1].nextSibling === a);
        const fragment = document.createDocumentFragment(); fragment.append(document.createElement('u'), document.createElement('s'));
        p.append(fragment);
        const inserted = observer.takeRecords();
        check('fragment insert is one record', () => inserted.length === 1 && inserted[0].addedNodes.length === 2);
        p.append(document.createElement('q'), 'text', document.createElement('em'));
        const appended = observer.takeRecords();
        check('append of several nodes is one record', () => appended.length === 1 && appended[0].addedNodes.length === 3 && appended[0].nextSibling === null);
        const first = p.firstChild, next = first.nextSibling;
        first.replaceWith(document.createElement('dd'), document.createElement('dt'));
        const replaced = observer.takeRecords();
        check('replaceWith is one record', () => replaced.length === 1 && replaced[0].addedNodes.length === 2 && replaced[0].removedNodes[0] === first && replaced[0].previousSibling === null && replaced[0].nextSibling === next);
        const head = p.firstChild;
        p.lastChild.before(head, document.createElement('kbd'));
        const moved2 = observer.takeRecords();
        check('before moves out, then inserts as one record', () => moved2.length === 2 && moved2[0].removedNodes[0] === head && moved2[1].addedNodes.length === 2 && moved2[1].addedNodes[0] === head);
        observer.disconnect();
        const text = document.getElementById('t').firstChild, range = document.createRange();
        range.setStart(text, 4); range.setEnd(text, 5);
        text.data = 'hi';
        check('data change clamps', () => range.startOffset === 0 && range.endOffset === 0 && range.startContainer === text);
        text.data = 'hello'; range.setStart(text, 1); range.setEnd(text, 4);
        text.insertData(0, 'xx');
        check('insertData shifts', () => range.startOffset === 3 && range.endOffset === 6);
        const tail = text.splitText(4);
        check('splitText moves the end', () => range.startContainer === text && range.startOffset === 3 && range.endContainer === tail && range.endOffset === 2);
        const parentRange = document.createRange(); parentRange.setStart(p, 2); parentRange.setEnd(p, 3);
        p.insertBefore(document.createElement('hr'), p.firstChild);
        check('insert shifts parent offsets', () => parentRange.startOffset === 3 && parentRange.endOffset === 4);
        p.firstChild.remove();
        check('remove shifts back', () => parentRange.startOffset === 2 && parentRange.endOffset === 3);
        const inner = document.createRange(); inner.selectNodeContents(a); a.remove();
        check('removed container moves to the parent', () => inner.startContainer === p && inner.endContainer === p);
        "#,
    );
    engine.tick().unwrap();
}

#[test]
fn document_types_follow_the_parser_and_the_document() {
    checked_page(
        "<!DOCTYPE html><html><body><p>x</p></body></html>",
        r#"
        const doctype = document.doctype;
        check('parsed', () => doctype instanceof DocumentType && doctype.nodeType === 10 && doctype.name === 'html' && doctype.publicId === '' && doctype.systemId === '' && doctype.nodeName === 'html' && doctype.textContent === null && document.firstChild === doctype && document.childNodes.length === 2);
        check('standards mode', () => document.compatMode === 'CSS1Compat');
        check('serialized', () => new XMLSerializer().serializeToString(document).startsWith('<!DOCTYPE html><html xmlns="http://www.w3.org/1999/xhtml">'));
        check('string tag', () => Object.prototype.toString.call(doctype) === '[object DocumentType]');
        check('created', () => { const made = document.implementation.createDocumentType('svg:svg', '-//W3C//DTD SVG 1.1//EN', 'x.dtd'); return made.name === 'svg:svg' && made.publicId === '-//W3C//DTD SVG 1.1//EN' && made.systemId === 'x.dtd' && made.parentNode === null; });
        check('invalid name', () => { try { document.implementation.createDocumentType('1x', '', ''); return false; } catch (e) { return e.name === 'InvalidCharacterError'; } });
        check('only documents hold doctypes', () => { try { document.body.append(document.implementation.createDocumentType('html', '', '')); return false; } catch (e) { return e.name === 'HierarchyRequestError'; } });
        check('moves', () => { doctype.remove(); const gone = document.doctype === null; document.insertBefore(doctype, document.documentElement); return gone && document.doctype === doctype && document.firstChild === doctype; });
        const parse = source => new DOMParser().parseFromString(source, 'text/html');
        check('quirks', () => parse('<p>x').compatMode === 'BackCompat' && parse('<!DOCTYPE HTML PUBLIC "-//W3C//DTD HTML 4.01 Transitional//EN"><p>').compatMode === 'BackCompat' && parse('<!DOCTYPE HTML PUBLIC "-//W3C//DTD HTML 4.01 Transitional//EN" "http://www.w3.org/TR/html4/loose.dtd"><p>').compatMode === 'CSS1Compat' && parse('<!doctype html><p>').doctype.name === 'html');
        check('text skips doctypes', () => document.body.textContent === 'x');
        "#,
    );
}

#[test]
fn custom_element_reactions_wait_for_the_call_and_report_errors() {
    let (_, engine) = checked_page(
        "<html><body><div id='box'></div><x-bad id='bad'></x-bad></body></html>",
        r#"
        globalThis.log = []; globalThis.errors = [];
        addEventListener('error', event => { errors.push(event.error.message); event.preventDefault(); });
        customElements.define('x-throw', class extends HTMLElement { connectedCallback() { log.push('in:' + this.id); throw new Error('boom ' + this.id); } });
        const box = document.getElementById('box'), a = document.createElement('x-throw'), b = document.createElement('x-throw');
        a.id = 'a'; b.id = 'b';
        box.append(a, b);
        check('a throwing callback does not stop the insertion', () => b.parentNode === box && log.join() === 'in:a,in:b');
        check('callback errors are reported', () => errors.join() === 'boom a,boom b');
        globalThis.defined = false;
        customElements.whenDefined('x-bad').then(() => defined = true);
        customElements.define('x-bad', class extends HTMLElement { constructor() { super(); throw new Error('constructor'); } connectedCallback() { log.push('bad'); } });
        check('a throwing upgrade is reported and fails for good', () => errors.at(-1) === 'constructor' && !log.includes('bad'));
        const bad = document.getElementById('bad'); box.append(bad);
        check('a failed element does not upgrade again', () => errors.length === 3 && !log.includes('bad'));
        const observer = new MutationObserver(() => {}); observer.observe(box, { childList: true, attributes: true, subtree: true });
        customElements.define('x-writer', class extends HTMLElement { static observedAttributes = ['data-on']; connectedCallback() { log.push('connected'); this.setAttribute('data-on', '1'); } attributeChangedCallback(name, old, value) { log.push(name + '=' + value); } });
        log.length = 0; box.append(document.createElement('x-writer'));
        check('records keep the browser order', () => observer.takeRecords().map(record => record.type).join() === 'childList,attributes');
        check('callbacks run in queue order', () => log.join() === 'connected,data-on=1');
        log.length = 0; box.innerHTML = '<x-writer data-on="2"></x-writer>';
        check('parsed elements upgrade with their attributes', () => log.join() === 'data-on=2,connected,data-on=1');
        const clone = box.firstChild.cloneNode();
        check('clones upgrade after their attributes are copied', () => clone instanceof customElements.get('x-writer') && clone.getAttribute('data-on') === '1');
        "#,
    );
    engine.tick().unwrap();
    assert_eq!(engine.eval("String(defined)", "test").unwrap(), "true");
}

#[test]
fn svg_fragments_parse_in_their_context() {
    checked_page(
        "<html><body><svg id='s'><g id='g'></g></svg></body></html>",
        r#"
        const NS = 'http://www.w3.org/2000/svg', g = document.getElementById('g');
        g.insertAdjacentHTML('beforeend', '<rect width="5" height="5"/><circle r="2"/><linearGradient id=lg></linearGradient><div>html</div>');
        check('self-closing tags close', () => g.children.length === 4 && g.children[0].localName === 'rect' && !g.children[0].firstChild && g.children[1].localName === 'circle');
        check('SVG tag names keep their case', () => g.children[2].localName === 'linearGradient' && g.querySelector('linearGradient')?.id === 'lg');
        check('fragment children are SVG elements', () => g.children[0] instanceof SVGElement && g.children[0].namespaceURI === NS && typeof g.children[0].getBBox === 'function');
        check('breakout tags stay HTML', () => g.children[3] instanceof HTMLElement && !(g.children[3] instanceof SVGElement));
        const detached = document.createElementNS(NS, 'g'); detached.innerHTML = '<rect viewbox="0 0 1 1"/><circle/>';
        check('innerHTML of a detached SVG element', () => detached.children.length === 2 && detached.firstChild instanceof SVGElement && detached.firstChild.getAttribute('viewBox') === '0 0 1 1');
        const range = document.createRange(); range.selectNodeContents(g);
        const fragment = range.createContextualFragment('<path d="M0 0"/><text>t</text>');
        check('contextual fragments in SVG', () => fragment.childNodes.length === 2 && fragment.firstChild instanceof SVGElement && fragment.lastChild.textContent === 't');
        const object = document.createElementNS(NS, 'foreignObject'); object.innerHTML = '<p>x</p>';
        check('HTML integration points parse HTML', () => object.firstChild instanceof HTMLElement && object.firstChild.localName === 'p');
        "#,
    );
}

#[test]
fn range_contents_walk_only_the_boundary_paths() {
    checked_page(
        "<html><body><div id='root'><p id='one'>abc<b>def</b></p><p id='two'>ghi</p><p id='three'><i>jkl</i>mno</p></div></body></html>",
        r#"
        const root = document.getElementById('root'), one = document.getElementById('one'), three = document.getElementById('three');
        const range = document.createRange(); range.setStart(one.firstChild, 1); range.setEnd(three.lastChild, 2);
        check('cloneContents', () => { const div = document.createElement('div'); div.append(range.cloneContents()); return div.innerHTML === '<p id="one">bc<b>def</b></p><p id="two">ghi</p><p id="three"><i>jkl</i>mn</p>'; });
        check('toString', () => range.toString() === 'bcdefghijklmn');
        range.deleteContents();
        check('deleteContents', () => root.innerHTML === '<p id="one">a</p><p id="three">o</p>' && range.collapsed && range.startContainer === root && range.startOffset === 1);
        const list = document.createElement('ul'); for (let i = 0; i < 3000; i++) list.append(document.createElement('li')); document.body.append(list);
        const many = document.createRange(); many.setStart(list, 1000); many.setEnd(list, 2000);
        check('cloneContents of siblings', () => many.cloneContents().childNodes.length === 1000);
        many.deleteContents();
        check('deleteContents of siblings', () => list.childNodes.length === 2000 && many.startOffset === 1000 && many.collapsed);
        const garbage = []; for (let i = 0; i < 100; i++) { const r = document.createRange(); r.setStart(list.firstChild, 0); garbage.push(r); }
        const far = document.createRange(); far.setStart(list, 5); far.setEnd(list, 6);
        list.firstChild.remove();
        check('ranges inside a removed node move to its parent', () => garbage.every(r => r.startContainer === list && r.startOffset === 0) && far.startOffset === 4 && far.endOffset === 5);
        const copy = far.cloneRange(); list.prepend(document.createElement('li'));
        check('clones are live too', () => copy.startOffset === 5 && copy.endOffset === 6);
        "#,
    );
}

#[test]
fn busy_hosts_keep_page_writes() {
    let (host, engine) = checked_page("<html><body></body></html>", "");
    {
        let _busy = host.borrow_mut();
        engine
            .eval("document.body.setAttribute('data-kept', '1')", "test")
            .unwrap();
    }
    engine.tick().unwrap();
    let host = host.borrow().store.clone();
    let host = host.borrow();
    let doc = &host.pages[&1].doc;
    let body = doc
        .nodes
        .iter()
        .position(|node| node.tag == "body")
        .unwrap();
    assert_eq!(
        doc.nodes[body].attrs.get("data-kept").map(String::as_str),
        Some("1")
    );
}

#[test]
fn reflected_attributes_like_chrome() {
    checked_page(
        "<html><body><a id='a'>x</a><img id='img'><div id='d' spellcheck='false'><p id='p'>t</p></div><textarea id='t'></textarea><progress id='pr'></progress></body></html>",
        r#"
        const $ = id => document.getElementById(id), el = tag => document.createElement(tag), d = el('div');
        check('draggable', () => !$('a').draggable && $('img').draggable && (d.draggable = true, d.getAttribute('draggable') === 'true') && (d.draggable = false, d.getAttribute('draggable') === 'false'));
        check('spellcheck inherits', () => el('div').spellcheck && !$('p').spellcheck && ($('p').spellcheck = true, $('p').getAttribute('spellcheck') === 'true'));
        check('translate', () => el('div').translate && (d.translate = false, d.getAttribute('translate') === 'no'));
        d.accessKey = 'k'; d.enterKeyHint = 'go'; d.inputMode = 'numeric'; d.nonce = 'n';
        check('strings', () => d.getAttribute('accesskey') === 'k' && d.getAttribute('enterkeyhint') === 'go' && d.inputMode === 'numeric' && d.nonce === 'n' && !d.hasAttribute('nonce') && el('div').accessKey === '');
        check('keywords', () => (d.setAttribute('enterkeyhint', 'BOGUS'), d.enterKeyHint === '') && (d.setAttribute('inputmode', 'Numeric'), d.inputMode === 'numeric') && (d.dir = 'RTL', d.dir === 'rtl') && (d.dir = 'x', d.dir === '') && document.dir === '');
        check('hidden', () => (d.hidden = 'until-found', d.hidden === 'until-found') && (d.hidden = true, d.getAttribute('hidden') === '') && (d.hidden = 0, !d.hasAttribute('hidden')));
        check('tabIndex', () => [el('a'), el('area'), el('object'), el('video'), el('select')].every(node => node.tabIndex === 0) && el('summary').tabIndex === -1 && el('div').tabIndex === -1);
        check('contentEditable', () => d.contentEditable === 'inherit' && (d.setAttribute('contenteditable', ''), d.contentEditable === 'true' && d.isContentEditable) && (d.setAttribute('contenteditable', 'BOGUS'), d.contentEditable === 'inherit' && !d.isContentEditable) && (d.contentEditable = 'inherit', !d.hasAttribute('contenteditable')));
        check('contentEditable rejects values', () => { try { d.contentEditable = 'nope'; } catch (e) { return e.name === 'SyntaxError'; } });
        check('numbers', () => el('input').maxLength === -1 && el('input').size === 20 && $('t').rows === 2 && $('t').cols === 20 && el('td').colSpan === 1 && el('ol').start === 1 && $('pr').max === 1 && $('pr').position === -1 && (() => { try { el('input').maxLength = -2; } catch (e) { return e.name === 'IndexSizeError'; } })());
        check('anchor parts', () => el('a').protocol === ':' && el('a').origin === '' && $('a').text === 'x');
        check('img', () => (i => (i.alt = 'a', i.loading = 'lazy', i.getAttribute('alt') === 'a' && i.loading === 'lazy' && el('img').loading === 'auto' && el('img').decoding === 'auto' && el('img').crossOrigin === null))(el('img')));
        const host = el('div'); host.innerHTML = '<b>x</b>'; document.body.append(host);
        check('newer methods', () => host.webkitMatchesSelector('div') && host.getHTML() === '<b>x</b>' && (host.setHTMLUnsafe('<i>y</i>'), host.innerHTML === '<i>y</i>') && typeof host.checkVisibility() === 'boolean' && (host.part.add('p'), host.getAttribute('part') === 'p'));
        check('outerText', () => (host.firstChild.outerText = 'plain', host.innerHTML === 'plain'));
    "#,
    );
}

#[test]
fn dom_api_edges_like_chrome() {
    let (_, engine) = checked_page(
        "<html><body><div id='h'></div><p id='p'>abc</p></body></html>",
        r#"
        const $ = id => document.getElementById(id), el = tag => document.createElement(tag);
        check('classList.supports throws', () => { try { el('div').classList.supports('x'); } catch (e) { return e instanceof TypeError; } });
        check('UIEvent view', () => new UIEvent('x').view === null && new MouseEvent('m', { view: window }).view === window);
        const st = el('div'); st.style.color = 'red'; st.style.width = '1px'; st.style.color = 'blue';
        check('style keeps order', () => st.style.cssText === 'color: blue; width: 1px;');
        const dup = el('div'); dup.style.cssText = 'color:red;width:1px;color:blue'; dup.style.color = 'green';
        check('style drops earlier duplicates', () => dup.style.cssText === 'width: 1px; color: green;');
        const views = []; const field = el('input'); document.body.append(field); for (const type of ['focus', 'focusin', 'blur', 'focusout']) field.addEventListener(type, e => views.push(e.view === window)); field.focus(); field.blur();
        check('focus events have a view', () => views.join() === 'true,true,true,true');
        check('DOMException codes', () => new DOMException('', 'InUseAttributeError').code === 10 && new DOMException('', 'HierarchyRequestError').code === 3);
        let records = 0; new MutationObserver(r => records += r.length).observe(st, { attributes: true }); st.style.color = 'blue';
        globalThis.__records = () => records;
        const r = document.createRange(), t = $('p').firstChild; r.setStart(t, 1); r.setEnd(t, 3); const c = r.cloneRange(); c.collapse(true);
        check('Range constants', () => Range.START_TO_END === 1 && r.compareBoundaryPoints(Range.START_TO_END, c) === 1 && r.compareBoundaryPoints(Range.END_TO_START, c) === 0);
        check('Range rejects doctype', () => { try { r.setStart(document.doctype ?? document.implementation.createDocumentType('html', '', ''), 0); } catch (e) { return e.name === 'InvalidNodeTypeError' && e.code === 24; } });
        const box = el('input'); box.type = 'checkbox'; const num = el('input'); num.type = 'number';
        check('no selection on checkbox', () => box.selectionStart === null && num.selectionStart === null && (() => { try { box.setSelectionRange(0, 1); } catch (e) { return e.name === 'InvalidStateError'; } })());
        check('date sanitizing', () => (d => (d.type = 'date', d.value = '2020-13-01', d.value === '' && (d.value = '2020-02-03', d.value === '2020-02-03')))(el('input')));
        check('tabIndex setter', () => (d => (d.tabIndex = '5x', d.getAttribute('tabindex') === '0'))(el('div')));
        check('enumerated', () => el('img').referrerPolicy === '' && el('img').fetchPriority === 'auto' && el('video').preload === 'metadata' && el('track').kind === 'subtitles' && el('button').formMethod === '' && (a => (a.referrerPolicy = 'BOGUS', a.referrerPolicy === ''))(el('a')) && (l => (l.as = 'Script', l.as === 'script'))(el('link')));
        check('text setters', () => (s => (s.text = 'x()', s.outerHTML === '<script>x()</script>'))(el('script')) && el('img').currentSrc === '');
        check('svg className', () => (s => (s.setAttribute('class', 'a'), typeof s.className === 'object' && s.className.baseVal === 'a' && s.className === s.className))(document.createElementNS('http://www.w3.org/2000/svg', 'svg')));
        const shadow = $('h').attachShadow({ mode: 'open' }); shadow.innerHTML = '<button>b</button>'; shadow.firstChild.focus();
        check('host matches :focus', () => $('h').matches(':focus') && $('h').matches(':focus-within') && !$('p').matches(':focus'));
        shadow.firstChild.blur();
        check('host loses :focus', () => !$('h').matches(':focus'));
        const order = []; globalThis.__order = order;
        requestAnimationFrame(() => { order.push('raf1'); Promise.resolve().then(() => order.push('micro')); });
        requestAnimationFrame(() => order.push('raf2'));
    "#,
    );
    engine.eval("__appsAnimationFrame()", "test").unwrap();
    assert_eq!(
        engine
            .eval("__order.join() + ' ' + __records()", "test")
            .unwrap(),
        "raf1,micro,raf2 0"
    );
}

#[test]
fn documents_like_chrome() {
    checked_page(
        "<!doctype html><html><body><form name='f1' id='f1'></form><img name='im'><a href='#x'>x</a><a name='n'>y</a><div id='dsd'><template shadowrootmode='open'><slot></slot></template><b>b</b></div></body></html>",
        r#"
        check('collections', () => document.forms.length === 1 && document.forms.f1.id === 'f1' && document.images.length === 1 && document.links.length === 1 && document.anchors.length === 1 && document.scripts.length === 0);
        check('fields', () => document.charset === 'UTF-8' && document.inputEncoding === 'UTF-8' && document.designMode === 'off' && document.referrer === '' && typeof document.lastModified === 'string' && document.contentType === 'text/html');
        document.cookie = 'a=1'; document.cookie = 'b=2; path=/'; document.cookie = 'a=; max-age=0';
        check('cookies', () => document.cookie === 'b=2');
        const made = document.implementation.createHTMLDocument('X'), p = made.createElement('p');
        check('created documents', () => made.doctype.name === 'html' && made.compatMode === 'CSS1Compat' && made.URL === 'about:blank' && made.defaultView === null && p.ownerDocument === made && made.createTextNode('t').ownerDocument === made);
        document.adoptNode(p);
        check('adopted nodes', () => p.ownerDocument === document && document.importNode(made.createElement('q')).ownerDocument === document);
        check('declarative shadow roots', () => { const host = document.getElementById('dsd'); return host.shadowRoot?.mode === 'open' && host.querySelector('template') === null && host.shadowRoot.querySelector('slot').assignedNodes()[0].localName === 'b'; });
        check('shadow hosts', () => { try { document.createElement('input').attachShadow({ mode: 'open' }); } catch (e) { return e.name === 'NotSupportedError' && document.createElement('x-y').attachShadow({ mode: 'open', delegatesFocus: true }).delegatesFocus; } });
        check('custom element names', () => { try { customElements.define('nodash', class extends HTMLElement {}); } catch (e) { return e.name === 'SyntaxError'; } });
        check('namespaces', () => document.body.lookupNamespaceURI(null) === 'http://www.w3.org/1999/xhtml' && document.body.isDefaultNamespace('http://www.w3.org/1999/xhtml') && document.getElementsByTagNameNS('*', 'a').length === 2);
        check('dataset names', () => { try { document.body.dataset['a-b'] = 1; } catch (e) { return e.name === 'SyntaxError'; } });
        const range = document.createRange(); range.selectNodeContents(document.body);
        check('ranges in other trees', () => { try { range.comparePoint(document.createElement('x'), 0); } catch (e) { return e.name === 'WrongDocumentError' && !range.isPointInRange(document.createElement('x'), 0); } });
        check('storage has', () => (localStorage.setItem('k', 'v'), 'k' in localStorage && !('nope' in localStorage)));
    "#,
    );
}

#[test]
fn link_activation_and_details_toggles_like_chrome() {
    let (_, engine) = checked_page(
        "<html><body><a id='same' href='#frag'>a</a><a id='blocked' href='#other'>b</a><a id='web' href='https://example.com/'>c</a><details id='d'><summary>s</summary>x</details></body></html>",
        r#"
        const $ = id => document.getElementById(id);
        $('blocked').addEventListener('click', event => event.preventDefault());
        $('same').click();
        check('same-document links navigate', () => location.hash === '#frag');
        $('blocked').click(); $('web').click();
        check('canceled and web links stay', () => location.hash === '#frag');
        globalThis.toggles = [];
        $('d').addEventListener('toggle', event => toggles.push(event.oldState + '>' + event.newState));
        $('d').open = true; $('d').open = false;
    "#,
    );
    let started = std::time::Instant::now();
    while engine.eval("toggles.length", "test").unwrap() == "0"
        && started.elapsed() < std::time::Duration::from_secs(5)
    {
        engine.tick().unwrap();
    }
    assert_eq!(
        engine.eval("toggles.join()", "test").unwrap(),
        "closed>closed"
    );
}

#[test]
fn element_internals_like_chrome() {
    checked_page(
        "<html><body><form id='f'><input name='n' value='1'><object></object></form></body></html>",
        r#"
        class XField extends HTMLElement { static formAssociated = true; constructor() { super(); this.internals = this.attachInternals(); } }
        customElements.define('x-field', XField);
        const f = document.getElementById('f'), x = document.createElement('x-field'), i = x.internals, error = fn => { try { fn(); } catch (e) { return e.name; } };
        x.setAttribute('name', 'xf'); f.prepend(x); i.setFormValue('val');
        check('form participation', () => i.form === f && f.elements[0] === x && [...new FormData(f)].map(pair => pair.join('=')).join('&') === 'xf=val&n=1');
        check('one per custom element', () => error(() => x.attachInternals()) === 'NotSupportedError' && error(() => document.createElement('div').attachInternals()) === 'NotSupportedError');
        i.setValidity({ valueMissing: true }, 'need');
        let submitted = false; f.addEventListener('submit', event => { submitted = true; event.preventDefault(); });
        f.requestSubmit();
        check('validity', () => !i.validity.valid && i.validity.valueMissing && i.validationMessage === 'need' && !f.checkValidity() && !submitted);
        i.setValidity({});
        f.requestSubmit();
        check('valid again', () => i.validity.valid && i.validationMessage === '' && f.checkValidity() && submitted);
    "#,
    );
}

#[test]
fn table_parts_without_a_table_keep_their_displays_like_chrome() {
    checked_page(
        "<html><body><div id='p'><div class='c'>a</div> <div class='c'>b</div></div>\
         <div id='q' style='display:inline-block'><div class='r'><div class='c'>c</div><div class='c'>d</div></div></div>\
         <style>.c{display:table-cell}.r{display:table-row}</style></body></html>",
        r#"
        const p = document.getElementById('p'), q = document.getElementById('q');
        const display = node => getComputedStyle(node).display;
        const style = node => [display(node), getComputedStyle(node).justifyContent, getComputedStyle(node).rowGap].join();
        check('hosts keep their own styles', () => style(p) === 'block,normal,normal' && style(q) === 'inline-block,normal,normal');
        check('parts keep their displays', () => [display(p.firstChild), display(q.firstChild), display(q.firstChild.firstChild)].join() === 'table-cell,table-row,table-cell');
        check('cells of an anonymous table in text', () => p.innerText === 'a\tb' && q.innerText === 'c\td');
        p.children[1].style.display = 'block';
        check('a block among cells', () => display(p) === 'block' && display(p.firstChild) === 'table-cell' && display(p.children[1]) === 'block');
        p.children[1].style.display = '';
        p.append('text');
        check('text among cells', () => display(p) === 'block' && display(p.firstChild) === 'table-cell');
        p.lastChild.remove();
        q.firstChild.className = '';
        check('a row turns block', () => display(q) === 'inline-block' && display(q.firstChild) === 'block' && display(q.firstChild.firstChild) === 'table-cell');
        q.firstChild.className = 'r';
        check('the row comes back', () => display(q.firstChild) === 'table-row' && q.innerText === 'c\td');
        "#,
    );
}

#[test]
fn same_text_writes_large_fragments_and_id_queries() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow().store.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<body><p id=label>a</p><div id=empty></div></body>"),
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
    let dirty = |source: &str| {
        engine.tick().unwrap();
        host.borrow()
            .store
            .borrow_mut()
            .pages
            .get_mut(&1)
            .unwrap()
            .dirty = false;
        engine.eval(source, "test").unwrap();
        engine.tick().unwrap();
        host.borrow().store.borrow().pages[&1].dirty
    };
    assert!(!dirty(
        "const label = document.getElementById('label'), text = label.firstChild; label.textContent = 'a'; text.data = 'a'; text.replaceData(0, 1, 'a'); document.getElementById('empty').textContent = ''; document.getElementById('empty').replaceChildren(); if (label.firstChild !== text) throw Error('kept text')"
    ));
    assert!(dirty("label.textContent = 'b'"));
    assert!(dirty("label.firstChild.data = 'c'"));
    let result = engine.eval(r#"
        const fails = [], check = (name, fn) => { try { if (!fn()) fails.push(name); } catch (error) { fails.push(name + ': ' + error); } };
        check('observed same text', () => { const observer = new MutationObserver(() => {}); observer.observe(label, { childList: true, characterData: true, subtree: true }); const old = label.firstChild; old.data = 'c'; label.textContent = 'c'; const records = observer.takeRecords(); observer.disconnect(); return records.map(r => r.type).join() === 'characterData,childList' && records[1].removedNodes[0] === old && label.firstChild !== old && label.textContent === 'c'; });
        const n = 5000;
        const list = document.createElement('ul'); document.body.append(list);
        let fragment = document.createDocumentFragment(); for (let i = 0; i < n; i++) { const li = document.createElement('li'); li.id = 'i' + i; fragment.append(li); }
        list.append(fragment);
        check('wrapped fragment', () => !fragment.firstChild && list.childNodes.length === n && list.children[0].id === 'i0' && list.lastChild.id === 'i' + (n - 1) && list.children[777].previousSibling.id === 'i776');
        const template = document.createElement('template'); template.innerHTML = '<li>x</li>'.repeat(n);
        const marker = list.children[1]; list.insertBefore(template.content, marker);
        check('parsed fragment', () => list.childNodes.length === 2 * n && list.children[1].textContent === 'x' && list.children[n].textContent === 'x' && list.children[n + 1] === marker && marker.previousSibling.textContent === 'x');
        while (list.firstChild) list.removeChild(list.firstChild);
        check('mixed fragment', () => { const parsed = document.createElement('template'); parsed.innerHTML = '<b>1</b><b id=m>2</b><link><b>4</b>'; const content = parsed.content, kept = content.getElementById('m'), into = document.createElement('p'); into.innerHTML = '<i>0</i><i>5</i>'; into.insertBefore(content, into.lastChild); return into.innerHTML === '<i>0</i><b>1</b><b id="m">2</b><link><b>4</b><i>5</i>' && kept.parentNode === into && kept.nextSibling.nextSibling.textContent === '4' && into.lastChild.previousSibling.textContent === '4' && !content.firstChild; });
        check('removed', () => !list.firstChild && !list.childNodes.length);
        check('document fragment', () => { const parsed = document.createElement('template'); parsed.innerHTML = '<a></a>'; const two = parsed.content; try { document.appendChild(two); } catch (error) { return error.name === 'HierarchyRequestError' && two.childNodes.length === 1; } });
        document.body.insertAdjacentHTML('beforeend', '<section id=scope><b id=dup></b><i id=dup></i></section><b id=dup></b><template><b id=inner></b></template>');
        const scope = document.getElementById('scope');
        check('id query', () => document.querySelector('#scope') === scope && scope.querySelector('#scope') === null && scope.querySelector('#dup') === scope.firstChild && scope.querySelectorAll('#dup').length === 2 && document.querySelectorAll('#dup').length === 3 && document.querySelector('#dup') === scope.firstChild && document.querySelector('#inner') === null && document.querySelector('#missing') === null && document.querySelectorAll('#missing').length === 0);
        check('id query order', () => { const late = document.createElement('i'); late.id = 'order'; const early = document.createElement('i'); early.id = 'order'; scope.append(late); scope.prepend(early); return scope.querySelector('#order') === early && scope.querySelectorAll('#order')[0] === early; });
        check('id query shadow', () => { const shadow = scope.attachShadow({ mode: 'open' }); shadow.innerHTML = '<p id=deep></p>'; return document.querySelector('#deep') === null && shadow.querySelector('#deep') === shadow.firstChild; });
        check('bad id selector', () => { try { document.querySelector('#1a'); } catch (error) { return error.name === 'SyntaxError'; } });
        fails.join('\n') || 'ok'
    "#, "test").unwrap();
    assert_eq!(result, "ok");
}

#[test]
fn live_collections_form_controls_and_custom_states() {
    checked_page(
        "<html><body><form id=f><input name=user id=u><input name=r type=radio value=1><input name=r type=radio value=2 checked><select name=s required><option value=''>a</select><x-f name=x></x-f></form><input form=f name=out><div id=d><p class='a b'></p>text</div></body></html>",
        r#"
        const $ = id => document.getElementById(id), [f, d, u] = [$('f'), $('d'), $('u')], make = tag => document.createElement(tag);
        const tags = d.getElementsByTagName('i'), classes = d.getElementsByClassName('a'), kids = d.children, nodes = d.childNodes;
        d.append(make('i')); d.insertBefore(make('b'), d.firstChild);
        check('insert', () => tags.length === 1 && tags === d.getElementsByTagName('i') && [...kids].map(kid => kid.localName).join() === 'b,p,i' && nodes[2].data === 'text');
        d.lastChild.className = 'a'; check('class', () => classes.length === 2 && classes[1] === tags[0]);
        d.replaceChildren('x', make('u')); check('replace', () => !tags.length && !classes.length && kids.length === 1 && nodes.length === 2);
        d.innerHTML = '<i></i><i></i>y'; check('html', () => tags.length === 2 && kids[1] === tags[1] && nodes[2].data === 'y' && Array.from(tags).length === 2 && '1' in tags && !('2' in tags));
        const fragment = document.createDocumentFragment(), held = fragment.childNodes; fragment.append(make('s'), 'z'); d.append(fragment);
        check('fragment', () => !held.length && kids[2].localName === 's' && nodes.length === 5);
        const elements = f.elements;
        check('elements', () => elements === f.elements && elements.user === u && elements.u === u && elements.r.value === '2' && elements.out.form === f && f.length === elements.length);
        const late = make('input'); late.name = 'title'; f.append(late);
        check('late', () => elements.title === late && Object.hasOwn(elements, 'title') && !Object.hasOwn(elements, 'nope')); late.remove(); check('gone', () => elements.title === undefined);
        const named = make('form'); named.name = 'find'; named.id = 'ff'; document.body.append(named);
        check('form names', () => document.forms.namedItem('ff') === named && document.forms.find === named && document.forms.namedItem('find') === named); named.remove();
        check('radio list', () => { const radios = elements.r; radios.value = '1'; return radios.value === '1' && radios.length === 2; });
        const items = d.getElementsByTagName('i'), marked = d.getElementsByClassName('m');
        check('loops', () => { for (let i = 0; i < items.length; i++) { items[i].id = 'i' + i; items[i].classList.add('m'); items[i].append(make('b')); } return items.length === 2 && marked.length === 2; });
        check('nested', () => { const box = make('div'); box.append(make('i')); const deep = box.getElementsByTagName('i'); d.append(box); const before = items.length; box.firstChild.remove(); return before === 3 && items.length === 2 && !deep.length; });
        check('required select', () => f.matches(':invalid') && elements.s.matches(':invalid'));
        elements.s.innerHTML = '<option>b'; check('new options', () => !f.matches(':invalid'));
        elements.s.firstChild.firstChild.data = ''; check('option text', () => elements.s.matches(':invalid'));
        elements.s.append(new Option('c', 'c')); elements.s.value = 'c';
        const log = [];
        customElements.define('x-f', class extends HTMLElement {
            static formAssociated = true;
            constructor() { super(); this.i = this.attachInternals(); }
            formAssociatedCallback(form) { log.push(form ? 'in' : 'out'); }
            formDisabledCallback(off) { log.push(off); }
            formResetCallback() { log.push('reset'); }
        });
        const x = f.querySelector('x-f');
        x.i.setValidity({ customError: true }, 'm'); check('custom invalid', () => f.matches(':invalid') && x.matches(':invalid') && elements.x === x);
        x.i.setValidity({}); x.i.states.add('on'); check('states', () => x.matches(':state(on)') && !f.matches(':invalid'));
        x.setAttribute('disabled', ''); check('disabled', () => x.matches(':disabled'));
        f.reset(); x.remove(); check('callbacks', () => log.join() === 'in,true,reset,out');
        f.addEventListener('submit', event => event.preventDefault()); u.required = true; check('user', () => !u.matches(':user-invalid'));
        f.requestSubmit(); check('submitted', () => u.matches(':user-invalid')); f.reset(); check('reset', () => !u.matches(':user-invalid'));
        "#,
    );
}

#[test]
fn xml_documents_attribute_nodes_and_highlights() {
    checked_page(
        "<html><body><div id=d a=1>some text</div></body></html>",
        r#"
        const parse = (source, type = 'text/xml') => new DOMParser().parseFromString(source, type), write = node => new XMLSerializer().serializeToString(node), d = document.getElementById('d');
        const source = '<?xml version="1.0"?><!DOCTYPE N><?p q?><N xmlns:x="u" A="&lt;"><x:I x:k="1"/>a&amp;b<![CDATA[<c>]]><b_d/></N>';
        const doc = parse(source), root = doc.documentElement, item = root.firstChild;
        check('names', () => doc instanceof XMLDocument && root.tagName === 'N' && item.prefix === 'x' && item.localName === 'I' && item.namespaceURI === 'u' && item.getAttributeNS('u', 'k') === '1' && root.getAttribute('A') === '<');
        check('nodes', () => [...root.childNodes].map(node => node.nodeType).join() === '1,3,4,1' && doc.childNodes[1].nodeType === 7);
        check('write', () => write(doc) === source && write(parse('<a xmlns="u"><b xmlns=""/></a>')) === '<a xmlns="u"><b xmlns=""/></a>');
        for (const bad of ['', 'x', '<!DOCTYPE a><!DOCTYPE a><a/>', '<a xmlns:p="u" xmlns:q="u" p:b="1" q:b="2"/>', '<a xmlns:p=""/>', '<a:b:c/>', '<constructor:a/>', '<a>&constructor;</a>', '<a>\u0001</a>', '<a>'.repeat(6000), '<!DOCTYPE a [<!ENTITY e "' + 'x'.repeat(1000) + '">]><a>' + '&e;'.repeat(11000) + '</a>', '<a>', '<a></b>', '<a b=c/>', '<a b="1" b="2"/>', '<a>&nope;</a>', '<x:a/>', '<a/><b/>']) check('bad ' + bad, () => parse(bad).querySelector('parsererror'));
        check('svg', () => parse('<svg xmlns="http://www.w3.org/2000/svg"><linearGradient/></svg>', 'image/svg+xml').documentElement.firstChild instanceof SVGElement);
        check('xhtml', () => parse('<html xmlns="http://www.w3.org/1999/xhtml"><body><p/></body></html>', 'application/xhtml+xml').body.firstChild instanceof HTMLParagraphElement);
        check('namespaces', () => { const node = document.createElementNS('u', 'a'); node.setAttributeNS('v', 'q:z', '1'); return node.getAttributeNS('v', 'z') === '1' && node.getAttributeNode('q:z').namespaceURI === 'v' && write(node) === '<a xmlns="u" xmlns:q="v" q:z="1"/>' && item.lookupPrefix('u') === 'x' && doc.cloneNode(true).documentElement.tagName === 'N'; });
        const attr = d.getAttributeNode('a');
        check('attr', () => attr instanceof Node && attr.ownerElement === d && attr.parentNode === null && !attr.childNodes.length && attr.textContent === '1' && attr.getRootNode() === attr);
        check('attr child', () => { try { d.append(attr); } catch (error) { return error.name === 'HierarchyRequestError'; } });
        check('attr gone', () => { d.setAttribute('b', '2'); const gone = d.getAttributeNode('b'); d.removeAttribute('b'); return gone.ownerElement === null && gone.value === '2' && d.getAttributeNode('b') === null; });
        const range = new Range(); range.setStart(d.firstChild, 0); range.setEnd(d.firstChild, 4);
        const highlight = new Highlight(range); CSS.highlights.set('x', highlight);
        check('highlight', () => highlight.size === 1 && highlight.add(range) === highlight && CSS.highlights.get('x') === highlight);
        document.head.innerHTML = '<style>:root{--c:red}::highlight(x){color:var(--c)}div::highlight(x){background-color:blue}@media print{div::highlight(x){color:green}}</style>';
        check('style', () => { const style = getComputedStyle(d, '::highlight(x)'); return style.color === 'rgb(255, 0, 0)' && style.backgroundColor === 'rgb(0, 0, 255)'; });
        check('supports', () => CSS.supports('selector(::highlight(a))') && !CSS.supports('selector(::highlight())') && CSS.supports('at-rule(@scope)') && !CSS.supports('at-rule(@nope)'));
        "#,
    );
}

#[test]
fn form_states_restyle_with_the_order_of_the_tree() {
    checked_page(
        "<html><head><style>:default{left:1px}form:invalid{left:2px}:indeterminate{left:3px}</style></head><body><form id=f><button id=a type=button></button><input id=b type=submit></form><button id=x form=f></button><form id=p></form><form id=q></form><input id=o required form=w><form id=g><input type=radio name=r id=r1><input type=radio name=r id=r2></form><input type=radio name=r id=r3 form=g></body></html>",
        r#"
        const $ = id => document.getElementById(id), [f, a, b, x, p, q, g, r1, r2, r3] = ['f', 'a', 'b', 'x', 'p', 'q', 'g', 'r1', 'r2', 'r3'].map($);
        const all = [f, a, b, x, p, q, g, r1, r2, r3], is = (node, value) => getComputedStyle(node).left === value;
        const right = () => all.every(node => !node.isConnected || is(node, node.matches(':default') ? '1px' : node.matches('form:invalid') ? '2px' : node.matches(':indeterminate') ? '3px' : 'auto'));
        const step = (name, change, expected) => check(name, () => { change(); return right() && expected(); });
        step('start', () => {}, () => is(b, '1px') && is(x, 'auto') && is(r3, '3px'));
        step('before', () => f.before(x), () => is(x, '1px') && is(b, 'auto'));
        step('after', () => f.after(x), () => is(b, '1px') && is(x, 'auto'));
        step('into', () => { const box = document.createElement('div'); f.before(box); box.append(x); }, () => is(x, '1px'));
        step('removed', () => x.parentNode.remove(), () => is(b, '1px'));
        step('type', () => { a.type = 'submit'; }, () => is(a, '1px') && is(b, 'auto'));
        step('same id', () => { p.id = q.id = 'w'; }, () => is(p, '2px') && is(q, 'auto'));
        step('swap', () => q.after(p), () => is(q, '2px') && is(p, 'auto'));
        step('swap back', () => p.after(q), () => is(p, '2px') && is(q, 'auto'));
        step('other id', () => { p.id = 'v'; }, () => is(q, '2px') && is(p, 'auto'));
        step('first gone', () => { p.id = 'w'; p.remove(); }, () => is(q, '2px'));
        step('checked', () => { r3.checked = true; }, () => is(r1, 'auto') && is(r2, 'auto'));
        step('out of group', () => r3.removeAttribute('form'), () => is(r1, '3px') && is(r3, 'auto'));
        step('in group', () => r3.setAttribute('form', 'g'), () => is(r1, 'auto'));
        step('new form id', () => { g.id = 'h'; }, () => is(r1, '3px'));
        step('wrapped', () => { const inner = document.createElement('div'); g.append(inner); inner.append(r3); r3.removeAttribute('form'); }, () => is(r1, 'auto'));
        step('moved out', () => document.body.append(r3.parentNode), () => is(r1, '3px'));
        step('left', () => { r1.checked = true; r1.remove(); }, () => is(r2, '3px'));
        step('back', () => g.append(r1), () => is(r2, 'auto'));
        const box = document.createElement('div'); document.body.append(box);
        step('template', () => { r1.checked = false; p.id = q.id = g.id = 't'; box.innerHTML = '<template><input form=t required><form id=t></form><input type=radio name=r checked form=t></template>'; q.id = 'other'; g.id = 'h'; g.before(p); r2.name = r1.name = 'r'; }, () => is(p, 'auto') && is(r2, '3px'));
        step('shared id', () => { box.innerHTML = '<template><form id=u></form></template>'; p.id = 'u'; box.append(Object.assign(document.createElement('input'), { required: true })); box.lastChild.setAttribute('form', 'u'); }, () => is(p, '2px'));
        step('read', () => box.firstChild.content, () => is(p, '2px'));
        step('into template', () => box.firstChild.append(box.lastChild), () => is(p, 'auto'));
        "#,
    );
}
