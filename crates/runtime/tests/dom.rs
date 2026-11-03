use apps_runtime::{
    dom::Document,
    host::{Host, Page, Role},
    jsc::{JavaScriptCore, JsContext},
};
use std::{cell::RefCell, rc::Rc};

#[test]
fn scroll_events_coalesce_and_synchronize_native_and_script_scrolling() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><body><main><div id='left'></div><div id='right'></div></main></body></html>"),
        dirty: false,
    });
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    engine.eval(r#"
        const left = document.getElementById('left'), right = document.getElementById('right');
        const received = [];
        document.querySelector('main').addEventListener('scroll', () => received.push('bubble'));
        document.querySelector('main').addEventListener('scroll', e => received.push(['capture',e.target.id]), true);
        left.addEventListener('scroll', e => {
            if (e.bubbles || e.cancelable || e.composed || e.target !== left) throw Error('Wrong scroll event');
            received.push(['left',left.scrollLeft,left.scrollTop]);
            right.scrollTo({left:left.scrollLeft,top:left.scrollTop});
        });
        right.addEventListener('scroll', () => received.push(['right',right.scrollLeft,right.scrollTop]));
        left.scrollTo(10,5); left.scrollTo(30,15);
        if(received.length) throw Error('Synchronous scroll event');
        __appsLayout();
        if(JSON.stringify(received)!=='[["capture","left"],["left",30,15]]') throw Error(JSON.stringify(received));
        __appsLayout();
        if(JSON.stringify(received.slice(2))!=='[["capture","right"],["right",30,15]]') throw Error(JSON.stringify(received));
        received.length=0;
        left.scrollTo(30,15); __appsLayout(); __appsLayout();
        if(received.length) throw Error('Unchanged scroll dispatched an event');
    "#, "test").unwrap();
    let id: usize = engine.eval("left._id", "test").unwrap().parse().unwrap();
    host.borrow().dom[&1].scroll[&id]
        .set_offset(renderer::point(renderer::px(-60.), renderer::px(-25.)));
    engine.eval(r#"
        __appsLayout(); __appsLayout();
        if(JSON.stringify(received)!=='[["capture","left"],["left",60,25],["capture","right"],["right",60,25]]') throw Error(JSON.stringify(received));
        received.length=0;
        const root=document.body.attachShadow({mode:'open'}), inner=document.createElement('div');
        root.append(inner);
        document.addEventListener('scroll',()=>received.push('outside'),true);
        root.addEventListener('scroll',()=>received.push('shadow'),true);
        inner.scrollLeft=8; __appsLayout();
        if(JSON.stringify(received)!=='["shadow"]') throw Error('Scroll escaped shadow root');
    "#, "test").unwrap();
}

#[test]
fn svg_presentation_attributes_links_and_attribute_nodes() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><head><style>.c{fill:blue}</style></head><body><svg><g style='fill:red;stroke:blue'><path id=a fill=none /><path id=b stroke-width=5 /><path id=c class=c fill=green /></g><g fill=green><path id=d /></g></svg><p id=p fill=red></p></body></html>"),
        dirty: false,
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
fn window_scroll_moves_the_page() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<html><body><main></main></body></html>"),
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
    host.borrow().dom[&1].scroll[&0]
        .set_offset(renderer::point(renderer::px(0.), renderer::px(-120.)));
    engine
        .eval(
            "__appsLayout(); if (scrollY !== 120) throw Error(String(scrollY));",
            "test",
        )
        .unwrap();
}

#[test]
fn utility_classes_follow_dom_changes() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><head><style>@import 'tailwindcss';</style></head><body><div id='dot' class='h-4 w-4 flex'></div></body></html>"),
        dirty: false,
    });
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    engine.eval("const dot = document.getElementById('dot'); dot.setAttribute('class', 'h-' + 8 + ' w-' + 8 + ' bg-[#123456]');", "test").unwrap();
    {
        let host = host.borrow();
        let doc = &host.pages[&1].doc;
        let id = doc
            .nodes
            .iter()
            .position(|n| n.attrs.get("id").is_some_and(|v| v == "dot"))
            .unwrap();
        let styles = apps_runtime::style::styles(doc, 800., 600.);
        let px = |value: &str| apps_runtime::style::length(value, 0., 16., 800., 600.);
        assert_eq!(px(&styles[id]["height"]), 32.);
        assert_eq!(px(&styles[id]["width"]), 32.);
        assert_eq!(styles[id]["background-color"], "#123456");
        assert_eq!(styles[id]["display"], "block");
    }
    engine
        .eval("dot.removeAttribute('class');", "test")
        .unwrap();
    let host = host.borrow();
    let doc = &host.pages[&1].doc;
    let id = doc
        .nodes
        .iter()
        .position(|n| n.attrs.get("id").is_some_and(|v| v == "dot"))
        .unwrap();
    assert!(!apps_runtime::style::styles(doc, 800., 600.)[id].contains_key("height"));
}

#[test]
fn mutations_preserve_tree_and_event_order() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<html><body><main id='root'><p>old</p></main></body></html>"),
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
        const root = document.getElementById('root');
        const old = root.firstChild;
        root.textContent = '';
        if (old.parentNode !== null) throw Error('Removed child still has a parent');
        const button = document.createElement('button');
        button.id = 'counter';
        const text = document.createTextNode('one');
        button.appendChild(text);
        root.appendChild(button);
        const label = document.createElement('p');
        label.textContent = 'label';
        root.insertBefore(label, button);
        root.insertBefore(button, label);
        root.insertBefore(button, button);
        if (root.firstChild !== button || button.nextSibling !== label || root.lastChild !== label) throw Error('Wrong order');
        if (document.getElementById('counter') !== button || button.ownerDocument !== document) throw Error('Wrong identity');
        if (!root.contains(text) || button.nodeType !== 1 || text.nodeType !== 3) throw Error('Wrong node metadata');
        let rejected = 0;
        try { button.appendChild(root); } catch { rejected++; }
        try { text.appendChild(label); } catch { rejected++; }
        try { root.removeChild(text); } catch { rejected++; }
        try { root.insertBefore(text, old); } catch { rejected++; }
        if (rejected !== 4) throw Error('Invalid mutation accepted');
        button.setAttribute('title', 'count');
        button.setAttribute('onclick', 'throw Error("Inline handler ran")');
        button.removeAttribute('title');
        if (button.getAttribute('title') !== null) throw Error('Attribute not removed');
        const calls = [];
        root.addEventListener('click', () => calls.push('capture'), true);
        button.addEventListener('click', () => { calls.push('target'); text.nodeValue = 'two'; });
        root.addEventListener('click', () => calls.push('bubble'));
        const removed = () => { throw Error('Removed listener called'); };
        root.addEventListener('click', removed);
        root.removeEventListener('click', removed);
        const ids = [];
        for (let node = button; node; node = node.parentNode) ids.push(node._id);
        __appsDispatch('click', ids);
        if (calls.join(',') !== 'capture,target,bubble' || button.textContent !== 'two') throw Error('Wrong event result');
        root.removeChild(button);
        if (button.parentNode !== null || document.getElementById('counter') !== null) throw Error('Node not detached');
        root.appendChild(button);
        if (root.lastChild !== button) throw Error('Cannot reattach node');
        const parent = document.createElement('section');
        parent.setAttribute('style', 'color: #123456; font-size: 24px');
        button.setAttribute('style', 'color: inherit; font-size: inherit');
        parent.appendChild(button);
        root.appendChild(parent);
    "#, "test").unwrap();
    assert!(host.borrow().pages[&1].dirty);
    let host = host.borrow();
    let doc = &host.pages[&1].doc;
    let styles = apps_runtime::style::styles(doc, 800., 600.);
    let button = doc
        .nodes
        .iter()
        .position(|node| node.tag == "button")
        .unwrap();
    assert_eq!(styles[button]["color"], "#123456");
    assert_eq!(styles[button]["font-size"], "24px");
}

#[test]
fn editing_fragments_observers_and_layout() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<html><body><main id='root'></main></body></html>"),
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
        const root = document.getElementById('root');
        const fragment = document.createDocumentFragment();
        const input = document.createElement('textarea');
        input.id = 'editor';
        input.dataset.state = 'ready';
        input.classList.add('editor', 'large');
        fragment.append(input, document.createTextNode('tail'));
        root.append(fragment);
        if (fragment.childNodes.length || root.children.length !== 1 || root.lastChild.data !== 'tail') throw Error('Fragment insertion');
        if (root.querySelector('[data-state="ready"]') !== input || !input.matches('.editor.large')) throw Error('Attribute query');
        input.style.cssText = 'width:100px;color:red';
        input.style.width = '200px';
        input.style.removeProperty('color');
        if (input.style.width !== '200px' || input.style.color !== '') throw Error('Style mutation');
        let mutationCount = 0;
        const observer = new MutationObserver(records => mutationCount += records.length);
        observer.observe(root, { subtree: true, attributes: true, childList: true, characterDataOldValue: true });
        input.title = 'edit';
        root.lastChild.data = 'new';
        const records = observer.takeRecords();
        if (records.length !== 2 || records[0].attributeName !== 'title' || records[1].oldValue !== 'tail') throw Error('Mutation records');
        input.value = 'A😀B';
        input.focus();
        input.setSelectionRange(1, 3);
        if (document.activeElement !== input || __appsEditing().start !== 1) throw Error('Focus selection');
        const inputEvents = [];
        input.addEventListener('beforeinput', event => inputEvents.push(event.type));
        input.addEventListener('input', event => inputEvents.push(event.type));
        __appsInput('中', 1, 3, true, [1,1]);
        if (input.value !== 'A中B' || __appsEditing().marked[1] !== 2) throw Error('Composition update');
        __appsInput('文', 1, 2);
        if (input.value !== 'A文B' || __appsEditing().marked !== null || inputEvents.join() !== 'beforeinput,input,beforeinput,input') throw Error('Composition commit');
        input.addEventListener('beforeinput', event => event.preventDefault(), { once: true });
        __appsInput('x',0,1);
        if (input.value !== 'A文B') throw Error('Cancelled edit applied');
        let stopped = 0;
        const button = document.createElement('button'); root.append(button);
        button.addEventListener('click', event => { stopped++; event.stopImmediatePropagation(); }, { once: true });
        button.addEventListener('click', () => stopped += 10);
        button.click(); button.click();
        if (stopped !== 11) throw Error('Event listener semantics');
        root.insertAdjacentHTML('beforeend', '<p class="message">hello <b>world</b></p>');
        const message = root.querySelector('p');
        if (message.textContent !== 'hello world' || message.querySelector('b').textContent !== 'world') throw Error('HTML tree');
        const clone = message.cloneNode(true);
        if (clone.outerHTML !== message.outerHTML || clone === message) throw Error('Deep clone');
        const range = document.createRange(); range.selectNodeContents(message.firstChild); range.setStart(message.firstChild,1); range.setEnd(message.firstChild,4);
        if (range.toString() !== 'ell') throw Error('Range text');
        range.deleteContents();
        if (message.firstChild.data !== 'ho ') throw Error('Range deletion');
        globalThis.observed = [];
        const resize = new ResizeObserver(entries => observed.push(entries[0].contentRect.width, entries[0].borderBoxSize[0].inlineSize)); resize.observe(input);
        globalThis.editorId = input._id;
    "#, "test").unwrap();
    let id: usize = engine.eval("editorId", "test").unwrap().parse().unwrap();
    host.borrow_mut()
        .dom
        .entry(1)
        .or_default()
        .bounds
        .insert(id, [20., 30., 200., 80.]);
    engine.eval("__appsLayout(); __appsLayout(); if (observed.join() !== '194,200' || input.getBoundingClientRect().x !== 20 || input.clientHeight !== 78 || input.offsetHeight !== 80 || input.clientLeft !== 1) throw Error('Layout observation ' + observed.join());", "test").unwrap();
}

#[test]
fn media_queries_follow_viewport_changes() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><head><style>@media (min-width:40em) { #target { color:red; } }</style></head><body><div id='target'></div></body></html>"),
        dirty: false,
    });
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    engine.eval(r#"
        const query = matchMedia('(min-width:40em)');
        if (!(query instanceof MediaQueryList) || !query.matches || query.media !== '(min-width:40em)') throw Error('Media query result');
        if (!matchMedia('screen and (max-height:600px)').matches || !matchMedia('print, (pointer:fine)').matches) throw Error('Media query list');
        if (matchMedia('print').matches || matchMedia('(unknown:yes)').matches) throw Error('Unsupported media query');
        if (getComputedStyle(document.getElementById('target')).color !== 'rgb(255, 0, 0)') throw Error('CSS media query disagrees');
        const changes = [];
        const listener = event => changes.push([event instanceof MediaQueryListEvent, event.media, event.matches]);
        query.addListener(listener);
        query.onchange = event => changes.push(event.matches);
        __appsLayout();
        if (changes.length) throw Error('Initial media event');
    "#, "test").unwrap();
    host.borrow_mut().viewports.insert(1, [400., 600.]);
    engine.eval(r#"
        if (query.matches) throw Error('Media query did not resize');
        __appsLayout(); __appsLayout();
        if (JSON.stringify(changes) !== '[[true,"(min-width:40em)",false],false]') throw Error('Media change event');
        if (getComputedStyle(document.getElementById('target')).color === 'rgb(255, 0, 0)') throw Error('CSS media query did not resize');
        query.removeListener(listener);
        query.onchange = null;
    "#, "test").unwrap();
    host.borrow_mut().viewports.insert(1, [1000., 600.]);
    engine.eval("__appsLayout(); if (!query.matches || changes.length !== 2) throw Error('Media listener removal');", "test").unwrap();
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
      if(!canvas.toDataURL().startsWith('data:image/png;base64,iVBOR'))throw Error('PNG encoding failed');
      canvas.width=8;if(ctx.getImageData(0,0,1,1).data[3]!==0)throw Error('Canvas resize did not clear pixels');
    "#, "test").unwrap();
}

#[test]
fn custom_elements_shadow_styles_and_lifecycle() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page { doc: Document::parse("<html><body><div id='outside'>outside</div><test-existing></test-existing></body></html>"), dirty: false });
    let engine = JsContext::new(JavaScriptCore::load().unwrap(), host, Role::Renderer(1)).unwrap();
    engine.eval(r#"
      const svg=document.createElementNS('http://www.w3.org/2000/svg','svg');
      const path=document.createElementNS('http://www.w3.org/2000/svg','path');svg.append(path);
      if(!(svg instanceof SVGSVGElement)||!(path instanceof SVGElement)||path instanceof HTMLElement||path.ownerSVGElement!==svg)throw Error('SVG namespace wrapper failed');
      const fragment=new DOMParser().parseFromString('<body><p>Hello <strong>bold</strong> end</p></body>','text/html');
      const paragraph=fragment.body.firstChild,range=document.createRange();range.setStart(paragraph.firstChild,2);range.setEnd(paragraph.lastChild,2);
      if(range.cloneContents().textContent!=='llo bold e')throw Error('Cross-container range clone failed');range.deleteContents();
      if(paragraph.textContent!=='Hend')throw Error('Cross-container range delete failed: '+paragraph.textContent);
      let connected=0,disconnected=0,attributes=0;
      class Example extends HTMLElement {static observedAttributes=['data-value'];constructor(){super();const root=this.attachShadow({mode:'open'});root.innerHTML='<style>div {color:#123456}</style><div id="inside">inside</div>'; } connectedCallback(){connected++;}disconnectedCallback(){disconnected++;}attributeChangedCallback(){attributes++;}}
      customElements.define('test-existing',Example);
      const existing=document.querySelector('test-existing');if(!(existing instanceof Example)||connected!==1)throw Error('Upgrade failed');
      const inside=existing.shadowRoot.getElementById('inside');if(document.getElementById('inside')||existing.querySelector('#inside'))throw Error('Shadow tree leaked');
      if(!inside.isConnected||inside.getRootNode()!==existing.shadowRoot||inside.getRootNode({composed:true})!==document)throw Error('Wrong shadow roots');
      if(getComputedStyle(inside).color!=='rgb(18, 52, 86)'||getComputedStyle(document.getElementById('outside')).color==='rgb(18, 52, 86)')throw Error('Shadow styles leaked');
      existing.style.whiteSpace='pre';inside.innerHTML='<span>  const value = 1</span>';
      if(getComputedStyle(inside.firstChild).whiteSpace!=='pre')throw Error('Shadow whitespace inheritance failed');
      inside.firstChild.style.whiteSpace='normal';
      if(getComputedStyle(inside.firstChild).whiteSpace!=='normal')throw Error('Text whitespace override failed');let threw=false;try{getComputedStyle(inside.firstChild.firstChild);}catch{threw=true;}if(!threw)throw Error('Text nodes have no computed style');
      const sheet=new CSSStyleSheet();sheet.replaceSync('#inside {color:#654321}');existing.shadowRoot.adoptedStyleSheets=[sheet];
      if(getComputedStyle(inside).color!=='rgb(101, 67, 33)')throw Error('Adopted sheet did not apply');
      existing.setAttribute('data-value','a');existing.remove();if(disconnected!==1||attributes!==1||inside.isConnected)throw Error('Lifecycle failed');
      let delivered=0;existing.addEventListener('private',()=>delivered++);inside.dispatchEvent(new Event('private',{bubbles:true}));if(delivered)throw Error('Non-composed event crossed shadow');
      inside.dispatchEvent(new Event('private',{bubbles:true,composed:true}));if(delivered!==1)throw Error('Composed event did not cross shadow');
    "#, "test").unwrap();
}

#[test]
fn shadow_slots_render_light_dom_and_receive_pointer_events() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><head><style>button { color:#123456; }</style></head><body><test-slots><button id='edit'>Edit</button><span slot='missing'>hidden</span><em>text</em></test-slots></body></html>"),
        dirty: false,
    });
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    engine.eval(r#"
        const host = document.querySelector('test-slots');
        const root = host.attachShadow({mode:'open'});
        root.innerHTML = '<style>:host {background-color:white;color:#445566;--tone:#987654} slot {color:var(--tone)} button {color:red}</style><header><slot name="actions"><button id="fallback">Fallback</button></slot></header><slot name="actions" id="duplicate">duplicate</slot><slot></slot>';
        const edit = document.getElementById('edit');
        const slot = root.querySelector('slot');
        edit.slot = 'actions';
        if (edit.getAttribute('slot') !== 'actions' || edit.assignedSlot !== slot) throw Error('Slot reflection or assignment');
        if (!(slot instanceof HTMLSlotElement) || slot.assignedElements()[0] !== edit || slot.assignedNodes().length !== 1) throw Error('Assigned nodes');
        if (root.getElementById('duplicate').assignedNodes().length) throw Error('Duplicate slot received nodes');
        if (root.querySelector('slot:not([name])').assignedNodes()[0].textContent !== 'text') throw Error('Default slot');
        if (edit.parentNode !== host || edit.getRootNode() !== document || root.querySelector('#edit')) throw Error('Slot changed the DOM tree');
        if (getComputedStyle(edit).color !== 'rgb(18, 52, 86)' || getComputedStyle(slot).display !== 'contents') throw Error('Slot styles');
        if (getComputedStyle(host).backgroundColor !== 'rgb(255, 255, 255)' || getComputedStyle(host).color !== 'rgb(68, 85, 102)') throw Error('Host styles');
        if (getComputedStyle(host.querySelector('em')).color !== 'rgb(152, 118, 84)') throw Error('Slotted inheritance');
        host.style.color = 'orange';
        if (getComputedStyle(host).color !== 'rgb(255, 165, 0)' || getComputedStyle(root.querySelector('header')).color !== 'rgb(255, 165, 0)') throw Error('Outer host cascade');
        root.querySelector('style').textContent += ':host {color:purple!important}';
        if (getComputedStyle(host).color !== 'rgb(128, 0, 128)') throw Error('Important host cascade');
        const events = [];
        edit.addEventListener('click', () => events.push('edit'));
        slot.addEventListener('click', () => events.push('slot'));
        host.addEventListener('click', () => events.push('host'));
        edit.click();
        if (events.join() !== 'edit,slot,host') throw Error('Slotted event path');
        globalThis.ids = [host._id, edit._id, root.getElementById('fallback')._id, host.querySelector('span')._id];
    "#, "test").unwrap();
    let ids: Vec<usize> =
        serde_json::from_str(&engine.eval("JSON.stringify(ids)", "test").unwrap()).unwrap();
    for id in ids {
        host.borrow_mut()
            .dom
            .entry(1)
            .or_default()
            .bounds
            .insert(id, [0., 0., 100., 40.]);
    }
    engine.eval(r#"
        if (document.elementFromPoint(10,10) !== edit) throw Error('Slotted button is not hit-testable');
        edit.slot = 'missing';
        if (slot.assignedNodes().length || slot.assignedNodes({flatten:true})[0].id !== 'fallback') throw Error('Fallback assignment');
        if (document.elementFromPoint(10,10)?.id !== 'fallback') throw Error('Fallback is not hit-testable');
        edit.slot = 'actions';
        if (document.elementFromPoint(10,10) !== edit) throw Error('Slot reassignment did not redraw');
        const closed = document.createElement('test-closed');
        document.body.append(closed);
        closed.attachShadow({mode:'closed'}).innerHTML = '<slot></slot>';
        closed.append(edit);
        if (edit.assignedSlot !== null) throw Error('Closed shadow slot leaked');
    "#, "test").unwrap();
}

#[test]
fn native_events_focus_the_editing_host_and_cross_shadow_roots() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<html><body><test-editor></test-editor></body></html>"),
            dirty: false,
        },
    );
    let engine = JsContext::new(JavaScriptCore::load().unwrap(), host, Role::Renderer(1)).unwrap();
    engine.eval(r#"
        const host = document.querySelector('test-editor');
        const root = host.attachShadow({mode:'open'});
        root.innerHTML = '<div contenteditable="true"><span><b>text</b></span></div>';
        const editor = root.firstChild, text = editor.firstChild.firstChild.firstChild;
        const selection = getSelection();
        if (selection.getComposedRanges().length) throw Error('Empty composed selection');
        selection.collapse(text, 2);
        const range = selection.getComposedRanges({shadowRoots:[root]})[0];
        if (!(range instanceof StaticRange) || range.startContainer !== text || range.startOffset !== 2 || !range.collapsed) throw Error('Composed selection lost its caret');
        const hidden = selection.getComposedRanges()[0];
        if (hidden.startContainer !== document.body || hidden.startOffset !== 0 || hidden.endOffset !== 1) throw Error('Unlisted shadow root was exposed');
        selection.collapse(text, 3);
        if (range.startOffset !== 2) throw Error('Static range followed selection');
        const events = [];
        for (const type of ['pointerdown','pointerup','keydown','click']) document.addEventListener(type, event => {
            if (!event.composed || event.composedPath()[0].getRootNode() !== root) throw Error('Native event path');
            events.push(type);
        });
        __appsDispatch('pointerdown', [text._id], {pointerId:1,pointerType:'mouse',clientX:0,clientY:0});
        if (__appsEditing()?.id !== editor._id) throw Error('Click focused an inline child');
        __appsDispatch('pointerup', [text._id], {pointerId:1,pointerType:'mouse'});
        __appsDispatch('keydown', [], {key:'ArrowRight'});
        if (events.join() !== 'pointerdown,pointerup,click,keydown') throw Error('Native event stopped at shadow root');
    "#, "test").unwrap();
}

#[test]
fn hit_testing_follows_paint_order_clipping_and_shadow_trees() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><body><div id='back'></div><div id='front'></div><div id='clip' style='overflow:hidden'><span id='child'></span></div><test-hit></test-hit></body></html>"),
        dirty: false,
    });
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    engine.eval("const shadowHost=document.querySelector('test-hit');shadowHost.attachShadow({mode:'open'}).innerHTML='<span id=\"inside\"></span>';", "test").unwrap();
    {
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
        host.dom.entry(1).or_default().bounds.extend(bounds);
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
        if(document.elementFromPoint(75,75)!==shadowHost.shadowRoot.firstChild)throw Error('Shadow hit');
        shadowHost.remove();
        if(document.elementFromPoint(75,75)!==back)throw Error('Detached shadow');
        if(document.elementFromPoint(-1,10)!==null||document.elementsFromPoint(900,10).length)throw Error('Viewport clipping');
    "#, "test").unwrap();
}

#[test]
fn hit_testing_takes_the_box_painted_last() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><body><div id='panel' style='height:0'><button id='button'></button></div><pre id='code'><span id='line'>x</span></pre></body></html>"),
        dirty: false,
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
        let mut host = host.borrow_mut();
        let dom = host.dom.entry(1).or_default();
        for (&id, bounds) in ids.iter().zip([
            [0., 0., 100., 100.],
            [0., 0., 100., 0.],
            [10., 10., 40., 20.],
            [0., 0., 100., 100.],
            [0., 0., 100., 20.],
        ]) {
            dom.bounds.insert(id, bounds);
        }
        // The panel comes first in the tree but paints last, like a z-index layer.
        dom.painted = vec![0; ids.iter().max().unwrap() + 1];
        for (order, id) in [ids[0], ids[3], ids[4], ids[1], ids[2]]
            .into_iter()
            .enumerate()
        {
            dom.painted[id] = order as u64 + 1;
        }
    }
    engine.eval(r#"
        if (document.elementFromPoint(20, 15).id !== 'button') throw Error('Later sibling hid a box painted above it');
        if (document.elementFromPoint(80, 15).id !== 'line') throw Error('Hit outside the button');
        if (document.elementsFromPoint(20, 15).map(node => node.id).slice(0, 3).join() !== 'button,line,code') throw Error('Stack order');
    "#, "test").unwrap();
}

#[test]
fn aria_properties_reflect_and_select_focuses_fields() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><body><button id='toggle'></button><input id='field' value='text'></body></html>"),
        dirty: false,
    });
    let engine = JsContext::new(JavaScriptCore::load().unwrap(), host, Role::Renderer(1)).unwrap();
    engine.eval(r#"
        const toggle = document.getElementById('toggle'), field = document.getElementById('field');
        if (toggle.ariaExpanded !== null || toggle.role !== null) throw Error('Missing attributes reflect as null');
        toggle.ariaExpanded = 'false'; toggle.ariaPressed = String(true); toggle.ariaLabel = 'Toggle'; toggle.role = 'switch';
        if (toggle.getAttribute('aria-expanded') !== 'false' || toggle.getAttribute('aria-pressed') !== 'true' || toggle.getAttribute('aria-label') !== 'Toggle' || toggle.getAttribute('role') !== 'switch') throw Error('ARIA properties set attributes');
        if (!toggle.matches('[aria-expanded=false][aria-pressed=true]')) throw Error('Selectors see reflected attributes');
        toggle.ariaPressed = null;
        if (toggle.hasAttribute('aria-pressed') || toggle.ariaPressed !== null) throw Error('null removes the attribute');
        toggle.focus(); field.select();
        if (document.activeElement !== field || field.selectionStart !== 0 || field.selectionEnd !== 4) throw Error('select() focuses the field');
    "#, "test").unwrap();
}

#[test]
fn scrolling_snaps_to_device_pixels_and_respects_scroll_margins() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><body><div id='box' style='overflow:scroll;scroll-padding:0 10px 0 20%'><i id='caret' style='scroll-margin:0 8px 0 50px'></i><i id='plain'></i></div></body></html>"),
        dirty: false,
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
        let mut host = host.borrow_mut();
        let dom = host.dom.entry(1).or_default();
        dom.pixel_ratio = 2.;
        for (&id, bounds) in
            ids.iter()
                .zip([[0., 0., 200., 50.], [300., 0., 2., 20.], [40., 0., 2., 20.]])
        {
            dom.bounds.insert(id, bounds);
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
fn computed_styles_follow_mutations_and_resize() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><style>.item {color:red} @media (min-width:900px) {.item {color:blue}}</style><body><div class='item'></div></body></html>"),
        dirty: false,
    });
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    engine.eval("const item=document.querySelector('.item');for(let i=0;i<3;i++)if(getComputedStyle(item).color!=='rgb(255, 0, 0)')throw Error('Initial style');", "test").unwrap();
    host.borrow_mut().viewports.insert(1, [1000., 600.]);
    engine.eval(r#"
        if(getComputedStyle(item).color!=='rgb(0, 0, 255)')throw Error('Viewport change');
        document.querySelector('style').textContent='.item {color:green}';
        if(getComputedStyle(item).color!=='rgb(0, 128, 0)')throw Error('Stylesheet mutation');
        item.style.color='purple';
        if(getComputedStyle(item).color!=='rgb(128, 0, 128)')throw Error('Inline mutation');
        item.removeAttribute('style');
        if(getComputedStyle(item).color!=='rgb(0, 128, 0)')throw Error('Attribute removal');
        item.className='';
        if(getComputedStyle(item).color==='rgb(0, 128, 0)')throw Error('Class removal');
        const root=item.attachShadow({mode:'open'});root.innerHTML='<span style="color:var(--ink)">text</span>';
        item.style.setProperty('--ink','red');
        if(getComputedStyle(root.firstChild).color!=='rgb(255, 0, 0)')throw Error('Shadow inheritance');
        item.style.setProperty('--ink','blue');
        if(getComputedStyle(root.firstChild).color!=='rgb(0, 0, 255)')throw Error('Shadow invalidation');
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
        if(a.compareDocumentPosition(c)!==4||c.compareDocumentPosition(a)!==2||document.body.compareDocumentPosition(a)!==20||a.compareDocumentPosition(document.createElement('p'))!==33)throw Error('Document position');
        if(!(document.querySelector('circle') instanceof SVGElement)||document.getElementById('html') instanceof SVGElement)throw Error('SVG wrapper');
        const calls=[];a.addEventListener('ping',()=>{throw Error('listener failure')});a.addEventListener('ping',()=>calls.push('second'));document.body.addEventListener('ping',()=>calls.push('bubble'));
        a.dispatchEvent(new Event('ping',{bubbles:true}));if(calls.join()!=='second,bubble')throw Error('Listener exception stopped dispatch');
        const canvas=document.createElement('canvas');canvas.width=canvas.height=4;const ctx=canvas.getContext('2d'),forged=new ImageData(1,1);
        Object.assign(forged,{width:2**31,height:2**31,data:new Uint8ClampedArray(0)});ctx.putImageData(forged,0,0);
        let rejected=false;try{ctx.getImageData(0,0,1,1);}catch{rejected=true;}if(!rejected)throw Error('Forged image data accepted');
    "#, "test").unwrap();
}

#[test]
fn pointer_hover_tracks_boundary_events_and_interaction_states() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><head><style>#outer:hover{color:red} #inner:hover{color:blue} #outer:focus-within{background-color:green} #inner:active{font-weight:700} #inner:focus{border-top:3px solid}</style></head><body><div id='outer'><div id='inner' tabindex='0'></div></div><div id='other'></div><test-card></test-card></body></html>"),
        dirty: false,
    });
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    engine.eval(r#"
        const [outer, inner, other, card] = ['#outer', '#inner', '#other', 'test-card'].map(s => document.querySelector(s));
        card.attachShadow({mode:'open'}).innerHTML = '<style>:host(:hover){color:purple}</style><span id="part"></span>';
        const log = [];
        for (const kind of ['pointer', 'mouse']) for (const type of ['over', 'out', 'enter', 'leave']) document.addEventListener(kind + type, event => {
            if (event.target.id) log.push(kind + type + ':' + event.target.id + (event.relatedTarget?.id ? '>' + event.relatedTarget.id : ''));
        }, true);
        globalThis.ids = [outer._id, inner._id, other._id, card._id, card.shadowRoot.querySelector('#part')._id];
        globalThis.check = (expected, label) => { const actual = log.splice(0).join(); if (actual !== expected) throw Error(label + ': ' + actual); };
    "#, "test").unwrap();
    let ids: Vec<usize> =
        serde_json::from_str(&engine.eval("JSON.stringify(ids)", "test").unwrap()).unwrap();
    for (id, bounds) in ids.into_iter().zip([
        [0., 0., 100., 100.],
        [0., 0., 50., 50.],
        [0., 100., 100., 100.],
        [0., 200., 100., 100.],
        [0., 200., 50., 50.],
    ]) {
        host.borrow_mut()
            .dom
            .entry(1)
            .or_default()
            .bounds
            .insert(id, bounds);
    }
    engine.eval(r#"
        const move = (x, y) => __appsDispatch('pointermove', [], {pointerId:1, pointerType:'mouse', clientX:x, clientY:y});
        move(10, 10);
        check('pointerover:inner,pointerenter:outer,pointerenter:inner,mouseover:inner,mouseenter:outer,mouseenter:inner', 'enter');
        if (getComputedStyle(outer).color !== 'rgb(255, 0, 0)' || getComputedStyle(inner).color !== 'rgb(0, 0, 255)') throw Error('Hover styles');
        move(12, 12);
        check('', 'same target');
        move(80, 80);
        check('pointerout:inner>outer,pointerleave:inner>outer,pointerover:outer>inner,mouseout:inner>outer,mouseleave:inner>outer,mouseover:outer>inner', 'leave child');
        if (getComputedStyle(inner).color !== 'rgb(255, 0, 0)') throw Error('Inner hover was not cleared');
        move(10, 150);
        check('pointerout:outer>other,pointerleave:outer>other,pointerover:other>outer,pointerenter:other>outer,mouseout:outer>other,mouseleave:outer>other,mouseover:other>outer,mouseenter:other>outer', 'sibling');
        if (getComputedStyle(outer).color === 'rgb(255, 0, 0)') throw Error('Outer hover was not cleared');
        move(10, 210);
        if (getComputedStyle(card).color !== 'rgb(128, 0, 128)') throw Error('Shadow host is not hovered');
        move(400, 400);
        if (getComputedStyle(card).color === 'rgb(128, 0, 128)') throw Error('Shadow host hover was not cleared');
        log.length = 0;
        __appsDispatch('pointerdown', [inner._id], {pointerId:1, pointerType:'mouse', clientX:10, clientY:10, button:0});
        if (getComputedStyle(inner).fontWeight !== '700' || getComputedStyle(outer).backgroundColor !== 'rgb(0, 128, 0)' || getComputedStyle(inner).borderTopWidth !== '3px') throw Error('Active or focus styles');
        if (inner.attributes.length !== 2 || inner.outerHTML.includes('apps') || inner.hasAttribute('data-apps-focus')) throw Error('Interaction state leaked into attributes');
        __appsDispatch('pointerup', [], {pointerId:1, pointerType:'mouse', clientX:600, clientY:10});
        if (getComputedStyle(inner).fontWeight === '700') throw Error('Active was not cleared');
        inner.blur();
        if (getComputedStyle(outer).backgroundColor === 'rgb(0, 128, 0)') throw Error('Focus within was not cleared');
        move(10, 10);
        log.length = 0;
        __appsDispatch('pointerexit', [], {pointerId:1, pointerType:'mouse', clientX:10, clientY:10});
        check('pointerout:inner,pointerleave:inner,pointerleave:outer,mouseout:inner,mouseleave:inner,mouseleave:outer', 'exit');
        if (getComputedStyle(outer).color === 'rgb(255, 0, 0)' || document.querySelector(':hover')) throw Error('Exit kept hover');
        if (document.querySelector('#outer:scope') || outer.querySelector(':scope > div') !== inner || !outer.matches(':scope')) throw Error('Scope');
    "#, "test").unwrap();
}

#[test]
fn core_dom_interfaces_follow_the_standard() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><body><div id='a' z='1' b='2' class='x y'>t<!--c-->u</div><template id='t'><p>hi</p></template></body></html>"),
        dirty: false,
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
      check('svg serialize', () => { const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg'); svg.setAttribute('viewBox', '0 0 1 1'); return new XMLSerializer().serializeToString(svg) === '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1 1"></svg>'; });
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
    assert_eq!(freed, serde_json::json!([canvas]));
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
    assert_eq!(host.borrow().dom[&1].retired.len(), 3);
}

#[test]
fn images_decode_on_workers_and_load_after_a_wake() {
    let (wake, woken) = std::sync::mpsc::channel();
    apps_runtime::host::Waker::install(move || {
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
        assert!(host.pages[&1].dirty && host.dom[&1].images.contains_key(&id));
        assert!(host.dom[&1].retired.is_empty());
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
    assert_eq!(host.dom[&1].retired.len(), 1);
    assert!(host.dom[&1].images.contains_key(&id));
}

#[test]
fn sibling_walks_follow_mutations() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<body><ul></ul><ol><li>a</li><li>b</li></ol></body>"),
            dirty: false,
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
fn mirror_follows_random_mutations_and_releases() {
    for seed in 1..17u32 {
        let host = Rc::new(RefCell::new(Host::default()));
        host.borrow_mut().pages.insert(1, Page { doc: Document::parse("<html><body><div id=a class='x y'>t<!--c-->u<p>p</p></div><ul><li>1</li><li>2</li></ul></body></html>"), dirty: false });
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
                let host = host.borrow();
                let (doc, shadows) = (&host.pages[&1].doc, &host.dom[&1].shadows);
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
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><body><div id='clip' style='overflow:hidden'><p id='a'>a</p><p id='b'>b</p></div><div id='rel' style='position:relative'><span id='c'>c</span></div></body></html>"),
        dirty: false,
    });
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    let place = |rects: &[(&str, [f32; 4])]| {
        let mut host = host.borrow_mut();
        let host = &mut *host;
        let doc = &host.pages[&1].doc;
        let state = host.dom.entry(1).or_default();
        for (name, rect) in rects {
            let id = doc
                .nodes
                .iter()
                .position(|node| node.attrs.get("id").map(String::as_str) == Some(name))
                .unwrap();
            state.bounds.insert(id, *rect);
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
        const io = new IntersectionObserver(entries => seen.push(...entries.map(e => [e.target.id, e.isIntersecting, e.intersectionRatio, e.intersectionRect.y, e.intersectionRect.height].join())), { threshold: [0, 0.5, 1] });
        for (const id of ['a', 'b', 'c']) io.observe(document.getElementById(id));
        __appsLayout();
        if (seen.join(';') !== 'a,true,0.5,50,50;b,false,0,200,0;c,true,1,320,10') throw Error(seen.join(';'));
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

fn checked_page(html: &str, script: &str) -> (Rc<RefCell<Host>>, JsContext) {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse(html),
            dirty: false,
        },
    );
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    let result = engine
        .eval(
            &format!(
                "(() => {{ const fails = []; const check = (name, fn) => {{ try {{ if (!fn()) fails.push(name); }} catch (e) {{ fails.push(name + ': ' + e.message); }} }}; {script}; return fails.join('\\n'); }})()"
            ),
            "test",
        )
        .unwrap();
    assert_eq!(result, "");
    (host, engine)
}

#[test]
fn event_handler_properties_are_listeners_in_order() {
    checked_page(
        "<html><body><div id='d' onclick='throw Error(1)'></div><svg id='s'></svg></body></html>",
        r#"
        const d = document.getElementById('d');
        check('feature detection', () => 'oninput' in document && 'onclick' in d && 'onmousedown' in window && 'onclick' in document.getElementById('s') && !('onclick' in new Text('')));
        check('default null', () => d.onclick === null && document.oninput === null && window.onload === null);
        const order = [];
        d.addEventListener('click', () => order.push(1));
        d.onclick = () => order.push(2);
        d.addEventListener('click', () => order.push(3));
        d.onclick = () => order.push(4);
        d.dispatchEvent(new Event('click'));
        check('handler keeps its first place', () => order.join() === '1,4,3');
        d.onclick = null; order.length = 0; d.dispatchEvent(new Event('click'));
        check('null removes the handler', () => order.join() === '1,3' && d.onclick === null);
        d.onclick = () => false;
        const event = new Event('click', { cancelable: true });
        check('false return cancels', () => !d.dispatchEvent(event) && event.defaultPrevented);
        let windowCalls = 0; window.onresize = () => windowCalls++;
        __appsDispatch('window:resize', []);
        check('window handler', () => windowCalls === 1);
        check('preact style names', () => 'onclick' in d && !('onClick' in d));
        "#,
    );
}

#[test]
fn select_options_follow_selection_and_keyboard() {
    let (host, _) = checked_page(
        "<html><body><form id='f'><select id='s' name='fruit'><option value='a'>Apple</option><option>  Banana  split </option><optgroup label='g'><option value='c' disabled>Cherry</option><option value='d'>Date</option></optgroup></select><select id='m' multiple><option selected>x</option><option selected>y</option></select></form></body></html>",
        r#"
        const s = document.getElementById('s'), m = document.getElementById('m');
        check('classes', () => s instanceof HTMLSelectElement && s.options[0] instanceof HTMLOptionElement && s.querySelector('optgroup') instanceof HTMLOptGroupElement && Object.prototype.toString.call(s) === '[object HTMLSelectElement]');
        check('default first option', () => s.selectedIndex === 0 && s.value === 'a' && s.type === 'select-one' && s.length === 4 && s.options.length === 4);
        check('option fields', () => s.options[1].value === 'Banana split' && s.options[1].text === 'Banana split' && s.options[3].index === 3 && s.options[2].disabled && s.options[0].form === document.getElementById('f') && s.options[0].label === 'Apple');
        check(':checked default', () => document.querySelector('#s :checked') === s.options[0]);
        s.value = 'd';
        check('value setter', () => s.selectedIndex === 3 && s.options[3].selected && !s.options[0].selected && document.querySelector('#s :checked') === s.options[3]);
        s.selectedIndex = -1;
        check('selectedIndex -1', () => s.value === '' && s.selectedOptions.length === 0);
        s.options[1].selected = true;
        check('option.selected', () => s.value === 'Banana split' && s.selectedOptions[0] === s.options[1]);
        check('multiple', () => m.type === 'select-multiple' && m.selectedOptions.length === 2 && m.options.selectedIndex === 0);
        const extra = document.createElement('option'); extra.value = 'e'; extra.text = 'Egg';
        s.add(extra, 0);
        check('add before index', () => s.options[0] === extra && s.length === 5);
        s.remove(0);
        check('remove index', () => s.length === 4 && !extra.parentNode);
        check('Option constructor', () => { const o = new Option('Fig', 'f', true, true); return o.value === 'f' && o.text === 'Fig' && o.defaultSelected && o.selected; });
        s.value = 'a'; s.focus();
        const events = [];
        s.addEventListener('input', e => events.push('input:' + e.isTrusted + ':' + s.value));
        s.addEventListener('change', e => events.push('change:' + e.isTrusted + ':' + s.value));
        __appsDispatch('keydown', [], { key: 'ArrowDown' });
        __appsDispatch('keydown', [], { key: 'ArrowDown' });
        check('keyboard skips disabled options', () => events.join() === 'input:true:Banana split,change:true:Banana split,input:true:d,change:true:d');
        globalThis.selectId = s.options[3]._id;
        "#,
    );
    let host = host.borrow();
    let doc = &host.pages[&1].doc;
    let option = doc
        .nodes
        .iter()
        .position(|node| {
            node.tag == "option" && node.attrs.get("value").map(String::as_str) == Some("d")
        })
        .unwrap();
    assert!(apps_runtime::dom::Document::matches(
        doc,
        option,
        "select option:checked"
    ));
}

#[test]
fn native_presses_follow_the_browser_event_sequence() {
    checked_page(
        "<html><body><div id='a' tabindex='0'>a</div><input id='i'><div id='b'>b</div></body></html>",
        r#"
        const a = document.getElementById('a'), i = document.getElementById('i'), b = document.getElementById('b');
        const log = [];
        for (const type of ['pointerdown','mousedown','pointerup','mouseup','click','auxclick','contextmenu','dblclick','wheel']) a.addEventListener(type, e => log.push(type + ':' + e.button + ':' + e.buttons + ':' + e.detail + ':' + e.isTrusted + (e instanceof MouseEvent ? '' : '!')));
        const press = (target, button, detail = 1) => { __appsDispatch('pointerdown', [target._id], { clientX: 5, clientY: 6, button, buttons: 1 << [0, 2, 1][button], detail }); __appsDispatch('pointerup', [target._id], { clientX: 7, clientY: 9, button, buttons: 0 }); };
        press(a, 0);
        check('primary sequence', () => log.join() === 'pointerdown:0:1:1:true,mousedown:0:1:1:true,pointerup:0:0:0:true,mouseup:0:0:0:true,click:0:0:1:true');
        check('primary press focuses', () => document.activeElement === a);
        log.length = 0; press(a, 2);
        check('secondary sequence', () => log.join() === 'pointerdown:2:2:1:true,mousedown:2:2:1:true,contextmenu:2:2:1:true,pointerup:2:0:0:true,mouseup:2:0:0:true,auxclick:2:0:1:true');
        log.length = 0; press(a, 1);
        check('middle sequence', () => log.join().endsWith('mouseup:1:0:0:true,auxclick:1:0:1:true') && !log.join().includes('click:1:0:1:true,') && !log.join().includes('contextmenu'));
        log.length = 0; press(a, 0, 2);
        check('dblclick', () => log.at(-1) === 'dblclick:0:0:2:true');
        let fields;
        a.addEventListener('mousemove', e => fields = [e.pageX, e.offsetX, e.movementX, e.x, e.screenX, e instanceof MouseEvent]);
        __appsDispatch('pointermove', [a._id], { clientX: 10, clientY: 10, buttons: 0 });
        __appsDispatch('pointermove', [a._id], { clientX: 14, clientY: 10, buttons: 0 });
        check('mouse fields', () => fields.join() === '14,14,4,14,14,true');
        let cancel = false; a.addEventListener('wheel', e => { e.preventDefault(); cancel = e.defaultPrevented; });
        check('wheel', () => __appsDispatch('wheel', [a._id], { clientX: 1, clientY: 1, deltaY: 30 }) === false && cancel && log.at(-1).startsWith('wheel:') && log.at(-1).endsWith('!') === false);
        let passive = null; document.addEventListener('wheel', e => { e.preventDefault(); passive = e.defaultPrevented; });
        __appsDispatch('wheel', [b._id], { clientX: 1, clientY: 1, deltaY: 30 });
        check('document wheel listeners are passive', () => passive === false);
        i.focus();
        b.addEventListener('mousedown', e => e.preventDefault(), { once: true });
        press(b, 0);
        check('canceled mousedown keeps focus', () => document.activeElement === i);
        press(b, 0);
        check('press on plain content blurs', () => document.activeElement === document.body);
        check('script events are untrusted', () => !new MouseEvent('click').isTrusted);
        let late = null; const e = new Event('ping'); a.addEventListener('ping', ev => late = ev.currentTarget); a.dispatchEvent(e);
        check('currentTarget cleared after dispatch', () => late === a && e.currentTarget === null && e.target === a && e.eventPhase === 0);
        const host = document.createElement('div'); document.body.append(host); const inner = host.attachShadow({ mode: 'open' }).appendChild(document.createElement('span'));
        const shadowEvent = new Event('x', { bubbles: true, composed: true }); inner.dispatchEvent(shadowEvent);
        check('shadow target cleared after dispatch', () => shadowEvent.target === null);
        "#,
    );
}

#[test]
fn checkbox_and_radio_activation_runs_before_listeners() {
    checked_page(
        "<html><body><input id='c' type='checkbox'><input id='r1' type='radio' name='g' checked><input id='r2' type='radio' name='g'><label id='l'>Label <input id='lc' type='checkbox'></label><label for='t' id='lt'>Text</label><input id='t'></body></html>",
        r#"
        const c = document.getElementById('c'), r1 = document.getElementById('r1'), r2 = document.getElementById('r2');
        const seen = [];
        c.addEventListener('click', () => seen.push('click:' + c.checked));
        c.addEventListener('input', e => seen.push('input:' + c.checked + ':' + e.bubbles));
        c.addEventListener('change', () => seen.push('change:' + c.checked));
        c.click();
        check('checked before listeners, then input and change', () => seen.join() === 'click:true,input:true:true,change:true' && c.checked);
        seen.length = 0;
        c.addEventListener('click', e => e.preventDefault(), { once: true });
        c.click();
        check('canceled click restores', () => seen.join() === 'click:false' && c.checked);
        check('checked does not write the attribute', () => !c.hasAttribute('checked') && !c.defaultChecked);
        r2.click();
        check('radio group', () => r2.checked && !r1.checked && r1.defaultChecked);
        r1.addEventListener('click', e => e.preventDefault(), { once: true });
        r1.click();
        check('canceled radio restores the group', () => r2.checked && !r1.checked);
        document.getElementById('l').click();
        check('label clicks its control', () => document.getElementById('lc').checked);
        document.getElementById('lt').click();
        check('label focuses a text control', () => document.activeElement === document.getElementById('t'));
        let tracked = null, writes = 0;
        const descriptor = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'checked');
        Object.defineProperty(c, 'checked', { configurable: true, get() { return descriptor.get.call(this); }, set(value) { writes++; descriptor.set.call(this, value); } });
        c.addEventListener('click', () => tracked = c.checked, { once: true });
        c.click();
        check('user toggles skip page setters, like React tracks', () => writes === 0 && tracked === false);
        "#,
    );
}

#[test]
fn range_inputs_sanitize_slide_and_step() {
    let (host, engine) = checked_page(
        "<html><body><input id='r' type='range'></body></html>",
        r#"
        const r = document.getElementById('r'), values = [];
        check('default is the middle', () => r.value === '50' && r.valueAsNumber === 50);
        r.max = '1';
        values.push(r.getAttribute('max'), r.value);
        r.step = 'any'; values.push(r.value);
        r.value = 7; values.push(r.value);
        r.min = '-1'; r.step = '0.3'; r.value = '0.95'; values.push(r.value);
        r.value = 'abc'; values.push(r.value);
        check('min, max and step reflect and sanitize ' + values, () => values.join() === '1,1,0.5,1,0.8,-0.1');
        "#,
    );
    let id: usize = engine.eval("document.getElementById('r')._id", "test").unwrap().parse().unwrap();
    host.borrow_mut().dom.entry(1).or_default().bounds.insert(id, [0., 0., 116., 16.]);
    engine.eval(r#"
        const r = document.getElementById('r'), seen = [];
        r.removeAttribute('min'); r.removeAttribute('max'); r.removeAttribute('step'); r.value = '50';
        r.addEventListener('input', () => seen.push('input:' + r.value));
        r.addEventListener('change', () => seen.push('change:' + r.value));
        __appsLayout();
        __appsDispatch('pointerdown', [r._id], { clientX: 38, clientY: 8, button: 0, buttons: 1 });
        __appsDispatch('pointermove', [r._id], { clientX: 83, clientY: 8, buttons: 1 });
        __appsDispatch('pointerup', [r._id], { clientX: 83, clientY: 8, button: 0, buttons: 0 });
        if (seen.join() !== 'input:30,input:75,change:75' || document.activeElement !== r) throw Error('Drag ' + seen);
        seen.length = 0;
        for (const key of ['ArrowRight', 'PageDown', 'Home', 'Home', 'End']) __appsDispatch('keydown', [], { key });
        if (seen.join() !== 'input:76,change:76,input:66,change:66,input:0,change:0,input:100,change:100') throw Error('Keys ' + seen);
    "#, "test").unwrap();
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
fn focus_follows_focusability_removal_and_shadow_roots() {
    checked_page(
        "<html><body><div id='plain'>x</div><a id='link' href='#x'>l</a><a id='nolink'>n</a><button id='b'>b</button><div id='tab' tabindex='2'></div><div id='host'></div><input id='first' tabindex='1'></body></html>",
        r#"
        const $ = id => document.getElementById(id);
        $('plain').focus();
        check('non-focusable ignored', () => document.activeElement === document.body);
        check('link tabIndex', () => $('link').tabIndex === 0 && $('nolink').tabIndex === -1 && $('b').tabIndex === 0);
        $('link').focus();
        check('link focus', () => document.activeElement === $('link'));
        const detached = document.createElement('button'); detached.focus();
        check('detached ignored', () => document.activeElement === $('link'));
        const events = [];
        $('b').focus();
        $('b').addEventListener('blur', () => events.push('blur'));
        $('b').addEventListener('focusout', () => events.push('focusout'));
        $('b').remove();
        check('removal blurs to body', () => document.activeElement === document.body && events.join() === 'blur,focusout');
        let key = false; document.addEventListener('keydown', () => key = true);
        __appsDispatch('keydown', [], { key: 'x' });
        check('document keydown after removal', () => key);
        const root = $('host').attachShadow({ mode: 'open' }); root.innerHTML = '<input id="inner">';
        root.firstChild.focus();
        check('activeElement retargets to the host', () => document.activeElement === $('host') && root.activeElement === root.firstChild);
        $('host').remove();
        check('removing a host blurs', () => document.activeElement === document.body);
        const order = [];
        for (let n = 0; n < 4; n++) { __appsDispatch('keydown', [], { key: 'Tab' }); order.push(document.activeElement.id); }
        check('tab order', () => order.join() === 'first,tab,link,first');
        __appsDispatch('keydown', [], { key: 'Tab', shiftKey: true });
        check('shift tab', () => document.activeElement.id === 'link');
        "#,
    );
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
fn text_controls_commit_change_on_blur_and_enter_and_forms_submit() {
    checked_page(
        "<html><body><form id='f'><input id='i' name='q' value='start' required pattern='[a-z]+'><textarea id='t' name='t'>hello</textarea><input type='email' id='e' name='e'><button id='go'>Go</button><button id='clear' type='reset'>Reset</button></form><input id='other'></body></html>",
        r#"
        const $ = id => document.getElementById(id), i = $('i'), t = $('t'), f = $('f');
        check('textarea class', () => t instanceof HTMLTextAreaElement && !(t instanceof HTMLInputElement) && !(i instanceof HTMLTextAreaElement) && t.type === 'textarea');
        i.value = 'abc';
        check('value is not the attribute', () => i.getAttribute('value') === 'start' && i.defaultValue === 'start' && i.value === 'abc');
        check('textarea default value', () => t.value === 'hello' && t.defaultValue === 'hello');
        const changes = [];
        i.addEventListener('change', e => changes.push('i:' + i.value + ':' + e.isTrusted));
        t.addEventListener('change', () => changes.push('t:' + t.value));
        i.focus(); __appsInput('x');
        check('no change while editing', () => changes.length === 0);
        $('other').focus();
        check('change on blur', () => changes.join() === 'i:abcx:true');
        $('other').focus(); t.focus(); t.setSelectionRange(5, 5); __appsInput('!'); t.blur();
        check('textarea change on blur ' + changes, () => changes.at(-1) === 't:hello!');
        const submits = [];
        f.addEventListener('submit', e => { submits.push(e.submitter?.id ?? null); e.preventDefault(); });
        i.focus(); i.value = 'abcy'; i._edited = true;
        __appsDispatch('keydown', [], { key: 'Enter' });
        check('enter commits and submits with the default button', () => changes.at(-1) === 'i:abcy:true' && submits.join() === 'go');
        check('validity', () => i.validity.valid && i.willValidate && i.checkValidity());
        i.value = 'ABC';
        check('pattern', () => i.validity.patternMismatch && !i.checkValidity() && i.validationMessage !== '');
        i.value = '';
        check('required', () => i.validity.valueMissing && !f.checkValidity());
        $('e').value = 'nope';
        check('email', () => $('e').validity.typeMismatch);
        i.setCustomValidity('custom');
        check('custom validity', () => i.validity.customError && i.validationMessage === 'custom');
        i.setCustomValidity(''); i.value = 'ok'; $('e').value = 'a@b.co';
        let invalid = 0; i.addEventListener('invalid', () => invalid++);
        submits.length = 0; f.requestSubmit();
        check('requestSubmit fires submit', () => submits.join() === '' && submits.length === 1);
        i.value = ''; submits.length = 0; f.requestSubmit();
        check('invalid forms do not submit', () => submits.length === 0 && invalid === 1);
        i.value = 'ok'; submits.length = 0; $('go').click();
        check('button submits', () => submits.join() === 'go');
        submits.length = 0; f.submit();
        check('submit() skips the event', () => submits.length === 0);
        $('clear').click();
        check('reset button restores defaults', () => i.value === 'start' && t.value === 'hello' && $('e').value === '');
        check('form data', () => { i.value = 'q1'; const data = new FormData(f); return data.get('q') === 'q1' && data.get('t') === 'hello' && !data.has('go'); });
        "#,
    );
}

#[test]
fn load_events_and_ready_state_follow_the_page_lifecycle() {
    let (_, engine) = checked_page(
        "<html><body></body></html>",
        r#"
        globalThis.lifecycle = [document.readyState];
        document.addEventListener('readystatechange', () => lifecycle.push(document.readyState));
        document.addEventListener('DOMContentLoaded', e => lifecycle.push('DOMContentLoaded:' + e.isTrusted));
        window.onload = () => lifecycle.push('load');
        __appsLoaded();
        check('interactive first', () => lifecycle.join() === 'loading,interactive,DOMContentLoaded:true');
        "#,
    );
    for _ in 0..20 {
        engine.tick().unwrap();
    }
    assert_eq!(
        engine.eval("lifecycle.join()", "test").unwrap(),
        "loading,interactive,DOMContentLoaded:true,complete,load"
    );
}

#[test]
fn offset_parent_follows_cssom_view() {
    let (host, engine) = checked_page(
        "<html><body><div id='rel' style='position:relative'><span id='in'>x</span><b id='fixed' style='position:fixed'>f</b><i id='gone' style='display:none'>g</i></div><table><tr><td><em id='cell'>c</em></td></tr></table></body></html>",
        "",
    );
    {
        let mut host = host.borrow_mut();
        let host = &mut *host;
        let doc = &host.pages[&1].doc;
        let state = host.dom.entry(1).or_default();
        for (name, rect) in [
            ("rel", [10., 20., 100., 50.]),
            ("in", [15., 30., 10., 10.]),
            ("fixed", [0., 0., 5., 5.]),
            ("cell", [3., 70., 5., 5.]),
        ] {
            let id = doc
                .nodes
                .iter()
                .position(|node| node.attrs.get("id").map(String::as_str) == Some(name))
                .unwrap();
            state.bounds.insert(id, rect);
        }
    }
    engine.eval(r#"
        const fails = [];
        const $ = id => document.getElementById(id);
        if (document.body.offsetParent !== null || document.documentElement.offsetParent !== null) fails.push('root');
        if ($('in').offsetParent !== $('rel') || $('in').offsetTop !== 10 || $('in').offsetLeft !== 5) fails.push('positioned');
        if ($('fixed').offsetParent !== null) fails.push('fixed');
        if (document.createElement('div').offsetParent !== null) fails.push('detached');
        if ($('cell').offsetParent?.localName !== 'td') fails.push('cell');
        let steps = 0; for (let node = $('in'); node; node = node.offsetParent) steps++;
        if (steps !== 3) fails.push('loop ' + steps);
        const removed = $('in'); removed.remove();
        if (removed.getBoundingClientRect().width !== 0 || removed.getClientRects().length !== 0 || removed.offsetWidth !== 0) fails.push('removed');
        if (fails.length) throw Error(fails.join());
    "#, "test").unwrap();
}

fn css_page(html: &str) -> (Rc<RefCell<Host>>, JsContext) {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse(html),
            dirty: false,
        },
    );
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    (host, engine)
}

#[test]
fn cssom_rule_lists_insert_delete_and_nest() {
    let (_, engine) = css_page(
        "<html><head><style id=s>.a{color:red} @media (min-width:1px){.b{color:blue}}</style><template><style>.t{}</style></template></head><body><p id=p class='a'>x</p><i id=i class=c>y</i></body></html>",
    );
    engine.eval(r#"
        const style = document.getElementById('s'), sheet = style.sheet, p = document.getElementById('p'), i = document.getElementById('i');
        if (sheet !== style.sheet || document.styleSheets.length !== 1 || document.styleSheets[0] !== sheet || sheet.ownerNode !== style) throw Error('styleSheets');
        if (sheet.cssRules.length !== 2 || !(sheet.cssRules[0] instanceof CSSStyleRule) || sheet.cssRules[0].selectorText !== '.a' || sheet.cssRules[0].style.color !== 'red') throw Error('rules ' + sheet.cssRules.length);
        const media = sheet.cssRules[1];
        if (!(media instanceof CSSMediaRule) || media.conditionText !== '(width>=1px)' || media.cssRules[0].selectorText !== '.b' || media.cssRules[0].parentRule !== media) throw Error('media ' + media.conditionText);
        if (sheet.insertRule('.c{color:green}', 2) !== 2 || getComputedStyle(i).color !== 'rgb(0, 128, 0)') throw Error('insertRule');
        for (const [rule, name] of [['garbage', 'SyntaxError'], ['.e{} .f{}', 'SyntaxError']]) { let error; try { sheet.insertRule(rule); } catch (e) { error = e.name; } if (error !== name) throw Error(rule + ' ' + error); }
        let index; try { sheet.insertRule('.g{}', 9); } catch (e) { index = e.name; } if (index !== 'IndexSizeError') throw Error('index');
        sheet.cssRules[0].style.setProperty('color', 'orange');
        if (getComputedStyle(p).color !== 'rgb(255, 165, 0)' || !sheet.cssRules[0].cssText.includes('color: orange')) throw Error('rule style ' + sheet.cssRules[0].cssText);
        sheet.cssRules[0].selectorText = '.missing';
        if (getComputedStyle(p).color === 'rgb(255, 165, 0)') throw Error('selectorText');
        media.insertRule('.a{color:purple}', 1);
        if (getComputedStyle(p).color !== 'rgb(128, 0, 128)' || media.cssRules.length !== 2) throw Error('nested insert');
        sheet.deleteRule(0);
        if (sheet.cssRules.length !== 2 || sheet.cssRules[0] !== media) throw Error('deleteRule');
        if (style.textContent.includes('purple')) throw Error('CSSOM changed the text');
        style.textContent = '.a{color:teal}';
        if (sheet.cssRules.length !== 1 || getComputedStyle(p).color !== 'rgb(0, 128, 128)') throw Error('text reset');
        const constructed = new CSSStyleSheet();
        constructed.replaceSync('@media(x){.a{}} .b{}');
        if (constructed.cssRules.length !== 2 || constructed.cssRules[0].cssRules.length !== 1) throw Error('replaceSync');
        constructed.deleteRule(0);
        if (constructed.cssRules.length !== 1 || constructed.cssRules[0].selectorText !== '.b') throw Error('constructed delete');
        let allowed; try { sheet.replaceSync(''); } catch (e) { allowed = e.name; } if (allowed !== 'NotAllowedError') throw Error('replaceSync on owned sheet');
        constructed.replaceSync('@keyframes k{from{opacity:0}to{opacity:1}} @font-face{font-family:x;src:url(a.ttf)} @layer a,b; @layer a{.z{}} @supports (display:grid){.y{}} @property --p{syntax:"<length>";inherits:false;initial-value:1px}');
        const [keys, face, layers, layer, supports, property] = constructed.cssRules;
        if (!(keys instanceof CSSKeyframesRule) || keys.name !== 'k' || keys.cssRules.length !== 2 || keys.findRule('to').style.opacity !== '1') throw Error('keyframes');
        keys.appendRule('50%{opacity:.5}');
        if (keys.cssRules.length !== 3) throw Error('appendRule');
        if (!(face instanceof CSSFontFaceRule) || face.style.getPropertyValue('font-family') !== 'x') throw Error('font-face');
        if (!(layers instanceof CSSLayerStatementRule) || layers.nameList.join() !== 'a,b' || !(layer instanceof CSSLayerBlockRule) || layer.name !== 'a') throw Error('layers');
        if (!(supports instanceof CSSSupportsRule) || supports.conditionText !== '(display:grid)') throw Error('supports');
        if (!(property instanceof CSSPropertyRule) || property.syntax !== '<length>' || property.inherits || property.initialValue !== '1px') throw Error('property');
    "#, "test").unwrap();
}

#[test]
fn css_in_js_libraries_insert_rules_through_style_tags() {
    let (_, engine) =
        css_page("<html><head></head><body><div id=a>a</div><div id=b>b</div></body></html>");
    engine.eval(r#"
        const a = document.getElementById('a'), b = document.getElementById('b');
        const emotion = { tags: [], ctr: 0, insert(rule) {
            if (this.ctr % 65000 === 0) { const tag = document.createElement('style'); tag.setAttribute('data-emotion', 'css'); tag.appendChild(document.createTextNode('')); tag.setAttribute('data-s', ''); document.head.insertBefore(tag, this.tags.length ? this.tags.at(-1).nextSibling : null); this.tags.push(tag); }
            const tag = this.tags.at(-1);
            let sheet = tag.sheet; if (!sheet) for (let i = 0; i < document.styleSheets.length; i++) if (document.styleSheets[i].ownerNode === tag) sheet = document.styleSheets[i];
            sheet.insertRule(rule, sheet.cssRules.length); this.ctr++;
        } };
        emotion.insert('.css-1{color:red}'); emotion.insert('.css-2{color:blue;padding:2px}');
        a.className = 'css-1'; b.className = 'css-2';
        if (getComputedStyle(a).color !== 'rgb(255, 0, 0)' || getComputedStyle(b).paddingTop !== '2px') throw Error('emotion ' + getComputedStyle(a).color);
        class CSSOMTag {
            constructor() { const style = document.createElement('style'); style.setAttribute('data-styled', 'active'); document.head.appendChild(style); this.element = style; style.appendChild(document.createTextNode('')); this.sheet = style.sheet; this.length = 0; }
            insertRule(index, rule) { try { this.sheet.insertRule(rule, index); this.length++; return true; } catch { return false; } }
            deleteRule(index) { this.sheet.deleteRule(index); this.length--; }
            getRule(index) { const rule = this.sheet.cssRules[index]; return rule && rule.cssText ? rule.cssText : ''; }
        }
        const tag = new CSSOMTag();
        if (!tag.insertRule(0, '.sc-b{color:green}') || !tag.insertRule(0, '.sc-a{color:purple}') || tag.insertRule(0, 'garbage')) throw Error('styled insert');
        if (tag.getRule(1) !== '.sc-b { color: green; }' || tag.length !== 2) throw Error('getRule ' + tag.getRule(1));
        b.className = 'sc-b';
        if (getComputedStyle(b).color !== 'rgb(0, 128, 0)') throw Error('styled apply');
        tag.deleteRule(1);
        if (getComputedStyle(b).color === 'rgb(0, 128, 0)') throw Error('styled delete');
        for (const tag of emotion.tags) tag.remove();
        if (getComputedStyle(a).color === 'rgb(255, 0, 0)' || document.styleSheets.length !== 1) throw Error('flush');
    "#, "test").unwrap();
}

#[test]
fn adopted_style_sheets_keep_order_and_stay_out_of_the_tree() {
    let (_, engine) = css_page("<html><body><p id=p>p</p><x-card id=card></x-card></body></html>");
    engine.eval(r#"
        const base = new CSSStyleSheet(), override = new CSSStyleSheet();
        base.replaceSync('span{color:red;padding:1px}'); override.replaceSync('span{color:blue}');
        const root = document.getElementById('card').attachShadow({ mode: 'open' });
        root.innerHTML = '<span>x</span>';
        root.adoptedStyleSheets = [base, override];
        const span = root.firstChild;
        if (getComputedStyle(span).color !== 'rgb(0, 0, 255)' || getComputedStyle(span).paddingTop !== '1px') throw Error('Lit overrides ' + getComputedStyle(span).color);
        if (root.childNodes.length !== 1 || root.innerHTML !== '<span>x</span>') throw Error('Adopted sheets in the tree');
        root.innerHTML = '<span>y</span>';
        if (getComputedStyle(root.firstChild).color !== 'rgb(0, 0, 255)') throw Error('innerHTML removed adopted sheets');
        override.replaceSync('span{color:green}');
        if (getComputedStyle(root.firstChild).color !== 'rgb(0, 128, 0)') throw Error('Live sheet update');
        override.insertRule('span{color:orange}', 1);
        if (getComputedStyle(root.firstChild).color !== 'rgb(255, 165, 0)') throw Error('Live insertRule');
        const page = new CSSStyleSheet(); page.replaceSync('p{color:purple}');
        document.adoptedStyleSheets.push(page);
        if (getComputedStyle(document.getElementById('p')).color !== 'rgb(128, 0, 128)' || document.adoptedStyleSheets.length !== 1) throw Error('document push');
        page.disabled = true;
        if (getComputedStyle(document.getElementById('p')).color === 'rgb(128, 0, 128)') throw Error('disabled');
        document.adoptedStyleSheets = [];
        let error; try { document.adoptedStyleSheets = [document.createElement('style').sheet ?? {}]; } catch (e) { error = e.name; }
        if (!error) throw Error('Adopted a non-constructed sheet');
    "#, "test").unwrap();
}

#[test]
fn css_global_supports_escape_and_register_property() {
    let (_, engine) = css_page(
        "<html><head><style>.x{width:var(--w)}</style></head><body><div class=x id=x><p id=c></p></div></body></html>",
    );
    engine.eval(r#"
        if (!CSS.supports('display', 'grid') || CSS.supports('width', 'bogus') || !CSS.supports('(filter: blur(1px)) and (not (display: nonsense))') || !CSS.supports('display: flex') || CSS.supports('hyphens', 'auto') || !CSS.supports('word-break', 'break-all')) throw Error('supports');
        if (!CSS.supports('selector(:has(a))') || CSS.supports('selector(::nonsense(')) throw Error('selector()');
        if (CSS.escape('1a b') !== '\\31 a\\ b' || CSS.escape('-') !== '\\-' || CSS.escape('a#b') !== 'a\\#b') throw Error('escape ' + CSS.escape('1a b'));
        CSS.registerProperty({ name: '--w', syntax: '<length>', inherits: false, initialValue: '12px' });
        const x = document.getElementById('x'), c = document.getElementById('c');
        if (getComputedStyle(x).width !== '12px' && getComputedStyle(x).getPropertyValue('--w') !== '12px') throw Error('registered initial ' + getComputedStyle(x).getPropertyValue('--w'));
        x.style.setProperty('--w', '20px');
        if (getComputedStyle(c).getPropertyValue('--w') !== '12px' || getComputedStyle(x).getPropertyValue('--w') !== '20px') throw Error('inherits false');
        let error; try { CSS.registerProperty({ name: '--w', syntax: '<length>', inherits: false, initialValue: '1px' }); } catch (e) { error = e.name; }
        if (error !== 'InvalidModificationError') throw Error('re-register ' + error);
        try { CSS.registerProperty({ name: '--bad', syntax: '<length>', inherits: false, initialValue: 'red' }); error = ''; } catch (e) { error = e.name; }
        if (error !== 'SyntaxError') throw Error('bad initial');
    "#, "test").unwrap();
}

#[test]
fn computed_styles_resolve_lengths_colors_initial_values_and_pseudo_elements() {
    let (host, engine) = css_page(
        "<html><head><style>:root{font-size:16px} #a{padding:calc(0.25rem * 2) 1em;margin:5%;color:#ff000080;border:2px solid;animation:fade 2s ease-in 1s infinite;line-height:1.5;opacity:50%;box-sizing:border-box;width:100px;height:40px} #a::before{content:'x';font-weight:700} #h{display:none}</style></head><body><div id=a>a</div><span id=s>s</span><div id=h></div></body></html>",
    );
    {
        let mut host = host.borrow_mut();
        let state = host.dom.entry(1).or_default();
        let ids: Vec<usize> = (0..64).collect();
        state
            .bounds
            .extend(ids.into_iter().map(|id| (id, [0., 0., 200., 100.])));
    }
    engine.eval(r#"
        const a = document.getElementById('a'), s = getComputedStyle(a), span = getComputedStyle(document.getElementById('s'));
        const expect = (style, name, value) => { if (style[name] !== value) throw Error(name + ': ' + style[name] + ' != ' + value); };
        expect(s, 'paddingTop', '8px'); expect(s, 'paddingLeft', '16px'); expect(s, 'marginTop', '10px');
        expect(s, 'color', 'rgba(255, 0, 0, 0.5)'); expect(s, 'borderTopColor', 'rgba(255, 0, 0, 0.5)'); expect(s, 'borderTopWidth', '2px');
        expect(s, 'animationName', 'fade'); expect(s, 'animationDuration', '2s'); expect(s, 'animationDelay', '1s'); expect(s, 'animationIterationCount', 'infinite'); expect(s, 'animationTimingFunction', 'ease-in');
        expect(s, 'lineHeight', '24px'); expect(s, 'opacity', '0.5'); expect(s, 'boxSizing', 'border-box'); expect(s, 'width', '200px');
        expect(s, 'padding', '8px 16px'); expect(s, 'position', 'static'); expect(s, 'zIndex', 'auto'); expect(s, 'transform', 'none'); expect(s, 'fontWeight', '400');
        expect(span, 'display', 'inline'); expect(span, 'boxSizing', 'content-box'); expect(span, 'borderTopWidth', '0px'); expect(span, 'backgroundColor', 'rgba(0, 0, 0, 0)'); expect(span, 'color', 'rgb(0, 0, 0)');
        if (parseFloat(s.marginTop) !== 10) throw Error('parseFloat');
        const before = getComputedStyle(a, '::before');
        expect(before, 'content', "'x'"); expect(before, 'fontWeight', '700'); expect(before, 'color', 'rgba(255, 0, 0, 0.5)');
        if (getComputedStyle(a, '::after').content !== '') throw Error('missing ::after');
        a.style.transform = 'translate(10px, 5px)';
        expect(getComputedStyle(a), 'transform', 'matrix(1, 0, 0, 1, 10, 5)');
        expect(getComputedStyle(document.getElementById('h')), 'display', 'none');
        if (!s.length || !s.item(0)) throw Error('length');
    "#, "test").unwrap();
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
        early.remove();
        check('removal releases the pointer capture', () => !early.hasPointerCapture(1) && lost === 1);
        const target = document.createElement('i'), detached = document.createElement('div');
        box.append(target); target.setPointerCapture(1);
        let bubbled = false; box.addEventListener('lostpointercapture', () => bubbled = true);
        target.releasePointerCapture(1);
        check('release fires a bubbling lostpointercapture', () => bubbled && lost === 2);
        target.setPointerCapture(1); detached.append(target);
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
    let nodes = host.borrow().pages[&1].doc.nodes.len();
    assert!(nodes < 20000, "{nodes} nodes");
}

#[test]
fn text_offsets_skip_comments_like_text_points() {
    let (host, _engine) = checked_page(
        "<html><body><div id='e' contenteditable>ab<!--?lit$123$-->cd<p>e<!--x-->f</p></div></body></html>",
        "",
    );
    let editor = host.borrow().pages[&1]
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
fn invalid_selectors_throw_and_html_names_ignore_case() {
    checked_page(
        "<html><body><ul><li data-v='1'>x</li></ul><svg><linearGradient id='g'></linearGradient><rect id='r'></rect></svg></body></html>",
        r#"
        for (const bad of ['div[', 'li,', 'li >', ':foo', '::foo', ':has(:has(a))']) check('throws ' + bad, () => { try { document.querySelector(bad); return false; } catch (e) { return e instanceof DOMException && e.name === 'SyntaxError'; } });
        check('matches throws', () => { try { document.body.matches('['); return false; } catch (e) { return e.name === 'SyntaxError'; } });
        check('closest throws', () => { try { document.querySelector('li').closest(':foo'); return false; } catch (e) { return e.name === 'SyntaxError'; } });
        check('type case', () => document.querySelectorAll('LI').length === 1 && document.getElementsByTagName('LI').length === 1 && document.getElementsByTagName('*').length > 5);
        check('attribute case', () => document.querySelector('[DATA-V]') !== null);
        check('svg case', () => document.querySelector('linearGradient')?.id === 'g' && document.querySelector('lineargradient') === null && document.getElementsByTagName('linearGradient').length === 1);
        check('namespaces', () => document.querySelector('*|rect')?.id === 'r' && document.querySelector('|rect') === null);
        "#,
    );
}

#[test]
fn contenteditable_typing_edits_text_nodes_and_splits_paragraphs() {
    checked_page(
        "<html><body><div id='e' contenteditable='true'>ab</div></body></html>",
        r#"
        const e = document.getElementById('e'), text = e.firstChild;
        e.focus(); getSelection().collapse(text, 2);
        const types = [];
        e.addEventListener('beforeinput', ev => types.push('before:' + ev.inputType));
        e.addEventListener('input', ev => types.push(ev.inputType));
        __appsInput('c');
        check('typing extends the text node', () => e.firstChild === text && text.data === 'abc' && e.childNodes.length === 1 && getSelection().focusNode === text && getSelection().focusOffset === 3);
        __appsDispatch('keydown', [], { key: 'Enter' });
        check('enter adds a div line', () => e.innerHTML === 'abc<div><br></div>' && types.includes('insertParagraph'));
        __appsInput('d');
        check('typing fills the new line', () => e.innerHTML === 'abc<div>d</div>');
        getSelection().collapse(text, 1);
        __appsDispatch('keydown', [], { key: 'Enter' });
        check('enter splits text ' + e.innerHTML, () => e.innerHTML === 'a<div>bc</div><div>d</div>');
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
fn inner_text_client_box_urls_dialog_and_details() {
    let (host, engine) = checked_page(
        "<html><head><base href='apps://app/sub/'></head><body><div id='t'>a  b<br>c<p>para</p><span style='display:none'>hidden</span>end</div><div id='box' style='border:3px solid;padding:5px'></div><a id='a' href='x.html#h'>x</a><img id='i' src='img.png'><dialog id='d'><form method='dialog'><button id='ok' value='yes'>ok</button></form></dialog><details id='det'><summary id='sum'>s</summary>body</details></body></html>",
        r#"
        check('innerText', () => document.getElementById('t').innerText === 'a b\nc\n\npara\n\nend');
        const t = document.getElementById('t'); t.innerText = 'x\ny';
        check('innerText setter', () => t.innerHTML === 'x<br>y');
        check('absolute urls', () => document.getElementById('a').href === 'apps://app/sub/x.html#h' && document.getElementById('a').hash === '#h' && document.getElementById('i').src === 'apps://app/sub/img.png' && document.getElementById('a').getAttribute('href') === 'x.html#h');
        const d = document.getElementById('d'), events = [];
        d.addEventListener('close', () => events.push('close:' + d.returnValue));
        d.addEventListener('cancel', () => events.push('cancel'));
        d.showModal();
        check('modal open', () => d.open && document.querySelector('dialog:modal') === d && document.activeElement === document.getElementById('ok'));
        __appsDispatch('keydown', [], { key: 'Escape' });
        check('escape cancels', () => !d.open && events.join() === 'cancel');
        d.show(); document.getElementById('ok').click();
        check('form method=dialog closes', () => !d.open && d.returnValue === 'yes');
        globalThis.dialogEvents = events;
        const det = document.getElementById('det'), toggles = [];
        det.addEventListener('toggle', e => toggles.push(e.newState));
        globalThis.toggles = toggles;
        document.getElementById('sum').click();
        check('summary opens details', () => det.open && det.hasAttribute('open'));
        "#,
    );
    let id = {
        let mut host = host.borrow_mut();
        let host = &mut *host;
        let doc = &host.pages[&1].doc;
        let id = doc
            .nodes
            .iter()
            .position(|node| node.attrs.get("id").map(String::as_str) == Some("box"))
            .unwrap();
        host.dom
            .entry(1)
            .or_default()
            .bounds
            .insert(id, [0., 0., 116., 40.]);
        id
    };
    let _ = id;
    for _ in 0..10 {
        engine.tick().unwrap();
    }
    engine.eval(r#"
        const box = document.getElementById('box');
        if (box.clientWidth !== 110 || box.clientHeight !== 34 || box.clientLeft !== 3 || box.offsetWidth !== 116) throw Error('client box ' + [box.clientWidth, box.clientHeight, box.clientLeft]);
        if (dialogEvents.join() !== 'cancel,close:yes,close:yes') throw Error('dialog events ' + dialogEvents.join());
        if (toggles.join() !== 'open') throw Error('toggle ' + toggles.join());
    "#, "test").unwrap();
}

#[test]
fn inline_style_declarations_validate_expand_and_serialize() {
    let (_, engine) = css_page("<html><body><div id=a></div></body></html>");
    engine.eval(r#"
        const a = document.getElementById('a'), style = a.style;
        style.width = '10px'; style.width = 'bogus';
        if (style.width !== '10px') throw Error('invalid value replaced ' + style.width);
        style.margin = '1px 2px';
        if (style.marginTop !== '1px' || style.marginLeft !== '2px' || style.margin !== '1px 2px' || style.getPropertyValue('margin') !== '1px 2px') throw Error('margin ' + style.margin);
        if (style.cssText !== 'width: 10px; margin: 1px 2px;') throw Error('cssText ' + style.cssText);
        style.marginTop = '3px';
        if (style.margin !== '3px 2px 1px') throw Error('margin longhand ' + style.margin);
        if (!('transform' in style) || !('webkitLineClamp' in style) || 'nonsense' in style) throw Error('has');
        if (!Object.keys(style).includes('marginTop') || !Object.keys(style).includes('margin-top')) throw Error('keys');
        style.setProperty('color', 'red', 'important');
        if (style.getPropertyPriority('color') !== 'important' || !a.getAttribute('style').includes('color: red !important')) throw Error('priority ' + a.getAttribute('style'));
        style.webkitLineClamp = '2';
        if (style.getPropertyValue('-webkit-line-clamp') !== '2' || style.WebkitLineClamp !== '2') throw Error('vendor ' + a.getAttribute('style'));
        style.removeProperty('margin');
        if (style.marginTop || style.margin) throw Error('removeProperty');
        style.cssText = 'border: 1px solid red; --x: 1';
        if (style.borderTopColor !== 'red' || style.border !== '1px solid red' || style.getPropertyValue('--x') !== '1') throw Error('border ' + style.cssText);
        a.style = 'padding: 4px';
        if (style.paddingRight !== '4px' || style.length !== 4) throw Error('assign');
    "#, "test").unwrap();
}

#[test]
fn focus_visible_follows_the_input_modality() {
    let (_, engine) = css_page(
        "<html><head><style>:focus-visible{color:red}</style></head><body><button id=b>b</button><input id=i><button id=c>c</button></body></html>",
    );
    engine.eval(r#"
        const b = document.getElementById('b'), i = document.getElementById('i'), c = document.getElementById('c');
        const down = node => __appsDispatch('pointerdown', [node._id], { pointerId: 1, pointerType: 'mouse', clientX: 1, clientY: 1, button: 0 });
        const up = node => __appsDispatch('pointerup', [node._id], { pointerId: 1, pointerType: 'mouse', clientX: 1, clientY: 1, button: 0 });
        b.focus();
        if (!b.matches(':focus-visible') || getComputedStyle(b).color !== 'rgb(255, 0, 0)') throw Error('Script focus before any input');
        down(c); up(c);
        if (document.activeElement !== c || c.matches(':focus-visible') || !c.matches(':focus') || getComputedStyle(c).color === 'rgb(255, 0, 0)') throw Error('Mouse focus on a button');
        b.focus();
        if (b.matches(':focus-visible')) throw Error('Script focus after a click');
        down(i); up(i);
        if (!i.matches(':focus-visible')) throw Error('Text inputs always show focus');
        down(c); up(c);
        __appsDispatch('keydown', [], { key: 'a', code: 'KeyA' });
        if (!c.matches(':focus-visible')) throw Error('A key shows the focus');
        b.focus();
        if (!b.matches(':focus-visible') || c.matches(':focus-visible')) throw Error('Script focus after a key');
    "#, "test").unwrap();
}

#[test]
fn slotted_and_part_rules_cross_the_shadow_boundary() {
    let (_, engine) = css_page(
        "<html><head><style>#host::part(label){color:blue}#host::part(other){color:purple}#host::part(deep){color:orange}#b{font-weight:300}</style></head><body><div id=host></div></body></html>",
    );
    engine.eval(r#"
        const host = document.getElementById('host'), root = host.attachShadow({ mode: 'open' });
        root.innerHTML = '<style>::slotted(span){color:red}::slotted(.b){font-weight:700}slot[name=x]::slotted(*){padding-top:3px}.lbl{color:green}</style><slot></slot><slot name=x></slot><p part="label" class=lbl id=p>p</p><i part=other id=i>i</i><div id=inner exportparts="core: deep"></div>';
        const inner = root.getElementById('inner').attachShadow({ mode: 'open' });
        inner.innerHTML = '<b part=core id=core>c</b><u part=label id=u>u</u>';
        host.innerHTML = '<span id=s>s</span><b class=b id=b>b</b><em slot=x id=e>e</em>';
        const style = (node, name) => getComputedStyle(node)[name];
        if (style(document.getElementById('s'), 'color') !== 'rgb(255, 0, 0)') throw Error('::slotted ' + style(document.getElementById('s'), 'color'));
        if (style(document.getElementById('b'), 'fontWeight') !== '300') throw Error('outer rules beat ::slotted');
        if (style(document.getElementById('e'), 'paddingTop') !== '3px') throw Error('slot selector');
        if (style(root.getElementById('p'), 'color') !== 'rgb(0, 0, 255)') throw Error('::part ' + style(root.getElementById('p'), 'color'));
        if (style(root.getElementById('i'), 'color') !== 'rgb(128, 0, 128)') throw Error('::part other');
        if (style(inner.getElementById('core'), 'color') !== 'rgb(255, 165, 0)') throw Error('exportparts ' + style(inner.getElementById('core'), 'color'));
        if (style(inner.getElementById('u'), 'color') === 'rgb(0, 0, 255)') throw Error('unexported part');
    "#, "test").unwrap();
}

#[test]
fn popovers_follow_the_auto_stack_light_dismiss_and_invokers() {
    let (host, engine) = checked_page(
        "<html><body><div id='a' popover><button id='open-b' popovertarget='b'>b</button><div id='b' popover='auto'>b</div></div><div id='c' popover=''>c</div><div id='m' popover='manual'>m</div><div id='h' popover='hint'>h</div><div id='plain'>x</div><button id='toggle' popovertarget='a'>a</button><button id='show' popovertarget='m' popovertargetaction='show'>m</button><form><button id='submit' popovertarget='a'>no</button></form><button id='cmd' commandfor='c' command='toggle-popover'>c</button><button id='custom' commandfor='plain' command='--go'>go</button></body></html>",
        r#"
        const $ = id => document.getElementById(id), [a, b, c, m, h, plain] = ['a', 'b', 'c', 'm', 'h', 'plain'].map($);
        const log = [];
        for (const node of [a, b, c, m, h]) for (const type of ['beforetoggle', 'toggle']) node.addEventListener(type, e => log.push(node.id + ':' + type + ':' + e.oldState + '>' + e.newState));
        const press = target => { __appsDispatch('pointerdown', [target._id], { clientX: 1, clientY: 1, button: 0, buttons: 1 }); __appsDispatch('pointerup', [target._id], { clientX: 1, clientY: 1, button: 0, buttons: 0 }); };
        check('attribute states', () => a.popover === 'auto' && c.popover === 'auto' && m.popover === 'manual' && h.popover === 'hint' && plain.popover === null && Object.assign(document.createElement('div'), { popover: 'bogus' }).popover === 'manual');
        check('not a popover', () => { try { plain.showPopover(); return false; } catch (e) { return e.name === 'NotSupportedError'; } });
        check('disconnected', () => { const d = document.createElement('div'); d.popover = 'auto'; try { d.showPopover(); return false; } catch (e) { return e.name === 'InvalidStateError'; } });
        a.showPopover();
        check('open', () => a.matches(':popover-open') && !b.matches(':popover-open') && log.join() === 'a:beforetoggle:closed>open');
        a.showPopover();
        check('showing twice is a no-op', () => log.length === 1);
        $('open-b').click();
        check('nested popover keeps its ancestor', () => a.matches(':popover-open') && b.matches(':popover-open'));
        c.showPopover();
        check('unrelated auto popover closes the stack', () => !a.matches(':popover-open') && !b.matches(':popover-open') && c.matches(':popover-open'));
        m.showPopover(); h.showPopover();
        check('manual and hint stay', () => m.matches(':popover-open') && h.matches(':popover-open') && c.matches(':popover-open'));
        press(plain);
        check('light dismiss closes auto and hint, not manual', () => !c.matches(':popover-open') && !h.matches(':popover-open') && m.matches(':popover-open'));
        a.showPopover(); b.showPopover();
        press(a);
        check('a press inside keeps the popover and closes nested ones', () => a.matches(':popover-open') && !b.matches(':popover-open'));
        __appsDispatch('keydown', [], { key: 'Escape' });
        check('escape hides the top popover', () => !a.matches(':popover-open'));
        check('toggle returns the state', () => c.togglePopover() === true && c.togglePopover() === false && c.togglePopover({ force: true }) === true && c.togglePopover(true) === true && c.togglePopover(false) === false);
        a.addEventListener('beforetoggle', e => e.preventDefault(), { once: true });
        a.showPopover();
        check('canceled beforetoggle keeps it closed', () => !a.matches(':popover-open'));
        $('toggle').click();
        check('popovertarget opens', () => a.matches(':popover-open'));
        $('toggle').click();
        check('popovertarget toggles', () => !a.matches(':popover-open'));
        m.hidePopover(); $('show').click(); $('show').click();
        check('popovertargetaction show', () => m.matches(':popover-open') && $('show').popoverTargetAction === 'show' && $('toggle').popoverTargetAction === 'toggle');
        check('popoverTargetElement', () => $('toggle').popoverTargetElement === a && ($('toggle').popoverTargetElement = c, $('toggle').popoverTargetElement === c && $('toggle').getAttribute('popovertarget') === ''));
        $('submit').click();
        check('submit buttons in forms do not invoke', () => !a.matches(':popover-open'));
        let commanded = null; c.addEventListener('command', e => commanded = [e.command, e.source?.id, e instanceof CommandEvent]);
        $('cmd').click();
        check('commandfor toggles a popover', () => c.matches(':popover-open') && commanded.join() === 'toggle-popover,cmd,true' && $('cmd').commandForElement === c && $('cmd').command === 'toggle-popover' && $('cmd').type === 'button');
        let custom = null; plain.addEventListener('command', e => custom = e.command);
        $('custom').click();
        check('custom commands only fire the event', () => custom === '--go');
        m.popover = 'auto';
        check('changing the type hides', () => !m.matches(':popover-open'));
        c.remove();
        check('removal hides', () => !c.matches(':popover-open'));
        globalThis.toggleLog = log;
        "#,
    );
    let id: usize = engine
        .eval(
            "document.getElementById('m').showPopover(); document.getElementById('m')._id",
            "test",
        )
        .unwrap()
        .parse()
        .unwrap();
    engine.eval("document.body.offsetWidth", "test").unwrap();
    assert_eq!(host.borrow().dom[&1].top_layer, [id]);
    for _ in 0..10 {
        engine.tick().unwrap();
    }
    engine
        .eval(
            r#"
        const toggles = toggleLog.filter(entry => entry.includes(':toggle:'));
        if (toggles.join() !== 'a:toggle:closed>closed,b:toggle:closed>closed,c:toggle:closed>open,m:toggle:closed>open,h:toggle:closed>closed') throw Error('coalesced toggles ' + toggles.join());
    "#,
            "test",
        )
        .unwrap();
}

#[test]
fn dialogs_close_on_requests_and_light_dismiss_by_closedby() {
    let (host, engine) = checked_page(
        "<html><body><div style='overflow:hidden;height:10px'><dialog id='d'><button id='in'>in</button></dialog></div><dialog id='n' closedby='none'>n</dialog><dialog id='any' closedby='any'>any</dialog><button id='open' commandfor='d' command='show-modal'>open</button><button id='close' commandfor='d' command='close' value='done'>close</button><div id='out'>out</div></body></html>",
        r#"
        const $ = id => document.getElementById(id), d = $('d');
        const log = [];
        for (const type of ['beforetoggle', 'cancel', 'close']) d.addEventListener(type, e => log.push(type + (e.newState ? ':' + e.newState : '')));
        check('closedBy', () => d.closedBy === 'none' && $('n').closedBy === 'none' && $('any').closedBy === 'any');
        $('open').click();
        check('show-modal command', () => d.open && d.matches(':modal') && d.closedBy === 'closerequest' && log.join() === 'beforetoggle:open');
        check('backdrop style', () => getComputedStyle(d, '::backdrop').backgroundColor === 'rgba(0, 0, 0, 0.1)' && getComputedStyle($('n'), '::backdrop').backgroundColor === '');
        check('modal tab order', () => { __appsDispatch('keydown', [], { key: 'Tab' }); return document.activeElement === $('in'); });
        d.requestClose('x');
        check('requestClose', () => !d.open && d.returnValue === 'x' && log.slice(1).join() === 'cancel,beforetoggle:closed');
        $('n').showModal();
        __appsDispatch('keydown', [], { key: 'Escape' });
        check('closedby none ignores escape', () => $('n').open);
        $('n').close();
        $('any').showModal();
        __appsDispatch('pointerdown', [$('any')._id], { clientX: 1, clientY: 1, button: 0, buttons: 1 });
        __appsDispatch('pointerup', [$('any')._id], { clientX: 1, clientY: 1, button: 0, buttons: 0 });
        check('closedby any closes on a backdrop press', () => !$('any').open);
        $('open').click();
        $('close').click();
        check('close command', () => !d.open && d.returnValue === 'done');
        d.addEventListener('beforetoggle', e => { if (e.newState === 'open') e.preventDefault(); }, { once: true });
        d.showModal();
        check('canceled beforetoggle keeps the dialog closed', () => !d.open);
        d.showModal();
        "#,
    );
    let ids: Vec<usize> = ["d", "out"]
        .iter()
        .map(|id| {
            engine
                .eval(&format!("document.getElementById('{id}')._id"), "test")
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect();
    {
        let mut host = host.borrow_mut();
        let dom = host.dom.entry(1).or_default();
        assert_eq!(dom.top_layer, [ids[0]]);
        dom.bounds.insert(ids[0], [40., 40., 20., 20.]);
        dom.bounds.insert(ids[1], [0., 0., 100., 100.]);
    }
    engine
        .eval(
            r#"
        const d = document.getElementById('d');
        if (document.elementFromPoint(45, 45) !== d) throw Error('modal outside the clip of its parent ' + document.elementFromPoint(45, 45)?.id);
        if (document.elementFromPoint(5, 5) !== d) throw Error('backdrop hits as its dialog');
        d.remove();
        if (document.elementFromPoint(5, 5)?.id !== 'out') throw Error('removed dialog left the top layer');
    "#,
            "test",
        )
        .unwrap();
    assert!(host.borrow().dom[&1].top_layer.is_empty());
}

#[test]
fn fullscreen_elements_enter_the_top_layer_and_exit_on_escape() {
    let (host, engine) = checked_page(
        "<html><body><div id='v'>video</div><dialog id='d'>d</dialog></body></html>",
        r#"
        const v = document.getElementById('v'), log = [];
        document.addEventListener('fullscreenchange', e => log.push('change:' + e.target.id + ':' + !!document.fullscreenElement));
        document.addEventListener('fullscreenerror', e => log.push('error:' + e.target.id));
        globalThis.fullscreenLog = log;
        check('enabled', () => document.fullscreenEnabled && document.fullscreenElement === null && typeof v.webkitRequestFullscreen === 'function');
        v.requestFullscreen();
        check('element', () => document.fullscreenElement === v && v.matches(':fullscreen') && document.webkitFullscreenElement === v && document.fullscreen);
        const d = document.getElementById('d'); d.showModal();
        globalThis.failed = d.requestFullscreen().then(() => 'resolved', e => e instanceof TypeError);
        "#,
    );
    let id: usize = engine
        .eval("document.getElementById('v')._id", "test")
        .unwrap()
        .parse()
        .unwrap();
    {
        let host = host.borrow();
        assert_eq!(host.dom[&1].top_layer.first(), Some(&id));
        assert!(host.commands.iter().any(|command| {
            command["args"][0] == "htmlFullscreen" && command["args"][2] == true
        }));
    }
    for _ in 0..10 {
        engine.tick().unwrap();
    }
    engine
        .eval(
            r#"
        globalThis.done = null; failed.then(value => globalThis.done = value);
        __appsDispatch('keydown', [], { key: 'Escape' });
        if (document.fullscreenElement !== null || document.getElementById('v').matches(':fullscreen')) throw Error('escape exits');
        if (!document.getElementById('d').open) throw Error('escape closed the dialog while it left fullscreen');
    "#,
            "test",
        )
        .unwrap();
    for _ in 0..10 {
        engine.tick().unwrap();
    }
    engine
        .eval(
            r#"
        if (done !== true) throw Error('modal dialogs cannot go fullscreen ' + done);
        if (fullscreenLog.join() !== 'change:v:true,error:d,change:v:false') throw Error(fullscreenLog.join());
    "#,
            "test",
        )
        .unwrap();
    assert!(
        host.borrow().commands.iter().any(|command| {
            command["args"][0] == "htmlFullscreen" && command["args"][2] == false
        })
    );
}

#[test]
fn document_types_follow_the_parser_and_the_document() {
    checked_page(
        "<!DOCTYPE html><html><body><p>x</p></body></html>",
        r#"
        const doctype = document.doctype;
        check('parsed', () => doctype instanceof DocumentType && doctype.nodeType === 10 && doctype.name === 'html' && doctype.publicId === '' && doctype.systemId === '' && doctype.nodeName === 'html' && doctype.textContent === null && document.firstChild === doctype && document.childNodes.length === 2);
        check('standards mode', () => document.compatMode === 'CSS1Compat');
        check('serialized', () => new XMLSerializer().serializeToString(document).startsWith('<!DOCTYPE html><html>'));
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
fn web_animations_sample_commit_finish_and_replace() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><body><div id='a'></div><div id='b' style='opacity:0.5'></div></body></html>"),
        dirty: false,
    });
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    engine.eval(r#"
        const a = document.getElementById('a'), b = document.getElementById('b'), events = [];
        const check = (got, want) => { if (JSON.stringify(got) !== JSON.stringify(want)) throw Error(JSON.stringify(got) + ' != ' + JSON.stringify(want)); };
        if (!Object.hasOwn(Element.prototype, 'animate')) throw Error('Element.animate is missing');
        const paused = a.animate({ opacity: [0, 1], transform: ['translateX(0px)', 'translateX(100px)'] }, { duration: 1000, easing: 'linear(0, 0.25 50%, 1)' });
        paused.pause(); paused.currentTime = 250;
        check([paused.playState, getComputedStyle(a).opacity, getComputedStyle(a).transform], ['paused', '0.125', 'matrix(1, 0, 0, 1, 12.5, 0)']);
        check(paused.effect.getKeyframes().map(frame => [frame.offset, frame.computedOffset]), [[null, 0], [null, 1]]);
        paused.commitStyles(); paused.cancel();
        check([a.style.opacity, a.style.transform, getComputedStyle(a).opacity, paused.playState], ['0.125', 'translateX(12.5px)', '0.125', 'idle']);
        for (const bad of [{ easing: 'bogus' }, { duration: -1 }, { fill: 'up' }, { easing: 'ease, linear' }]) {
            try { a.animate({ opacity: [0, 1] }, bad); } catch (error) { if (error instanceof TypeError) continue; throw error; }
            throw Error('Accepted ' + JSON.stringify(bad));
        }
        const one = b.animate({ opacity: [0, 0.2] }, { duration: 20, fill: 'forwards' }), two = b.animate({ opacity: [0, 0.4] }, { duration: 20, fill: 'forwards' });
        one.onremove = () => events.push('remove');
        two.onfinish = event => events.push('finish ' + event.currentTime);
        two.finished.then(animation => events.push('finished ' + (animation === two)));
        const gone = b.animate({ opacity: [1, 0] }, 1000);
        gone.finished.catch(error => events.push(error.name));
        gone.cancel();
        // Play and pause wait for the next frame, like in browsers.
        const fresh = document.body.appendChild(document.createElement('i')).animate({ opacity: [0, 1] }, 1000);
        events.push('pending ' + fresh.pending + ' ' + fresh.startTime);
        fresh.ready.then(() => events.push('ready ' + fresh.pending + ' ' + (typeof fresh.startTime)));
    "#, "test").unwrap();
    let started = std::time::Instant::now();
    while engine
        .eval("events.length", "test")
        .unwrap()
        .parse::<usize>()
        .unwrap()
        < 6
    {
        let delay = engine.tick().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(delay.clamp(1, 10) as u64));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
    assert_eq!(
        engine.eval("JSON.stringify([events.sort(), one.replaceState, two.playState, b.getAnimations().length, getComputedStyle(b).opacity])", "test").unwrap(),
        r#"[["AbortError","finish 20","finished true","pending true null","ready false number","remove"],"removed","finished",1,"0.4"]"#
    );
}
