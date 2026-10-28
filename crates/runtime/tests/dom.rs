use apps_runtime::{
    host::{Host, Page, Role},
    html::Document,
    jsc::{Api, Engine},
};
use std::{cell::RefCell, rc::Rc};

#[test]
fn scroll_events_coalesce_and_synchronize_native_and_script_scrolling() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><body><main><div id='left'></div><div id='right'></div></main></body></html>"),
        dirty: false,
    });
    let engine = Engine::new(Api::load().unwrap(), host.clone(), Role::Renderer(1)).unwrap();
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
    host.borrow().dom[&1].scroll[&id].set_offset(gpui::point(gpui::px(-60.), gpui::px(-25.)));
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
fn utility_classes_follow_dom_changes_without_css_generation() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><head><style>@import 'tailwindcss';</style></head><body><div id='dot' class='h-4 w-4 flex'></div></body></html>"),
        dirty: false,
    });
    let engine = Engine::new(Api::load().unwrap(), host.clone(), Role::Renderer(1)).unwrap();
    engine.eval("const dot = document.getElementById('dot'); dot.setAttribute('class', 'h-' + 8 + ' w-' + 8 + ' bg-[#123456]');", "test").unwrap();
    {
        let host = host.borrow();
        let doc = &host.pages[&1].doc;
        let id = doc
            .nodes
            .iter()
            .position(|n| n.attrs.get("id").is_some_and(|v| v == "dot"))
            .unwrap();
        let styles = apps_runtime::render::styles(doc, 800., 600.);
        assert_eq!(styles[id]["height"], "2rem");
        assert_eq!(styles[id]["width"], "2rem");
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
    assert!(!apps_runtime::render::styles(doc, 800., 600.)[id].contains_key("height"));
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
    let engine = Engine::new(Api::load().unwrap(), host.clone(), Role::Renderer(1)).unwrap();
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
    let styles = apps_runtime::render::styles(doc, 800., 600.);
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
    let engine = Engine::new(Api::load().unwrap(), host.clone(), Role::Renderer(1)).unwrap();
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
        const resize = new ResizeObserver(entries => observed.push(entries[0].contentRect.width)); resize.observe(input);
        globalThis.editorId = input._id;
    "#, "test").unwrap();
    let id: usize = engine.eval("editorId", "test").unwrap().parse().unwrap();
    host.borrow_mut()
        .dom
        .entry(1)
        .or_default()
        .bounds
        .insert(id, [20., 30., 200., 80.]);
    engine.eval("__appsLayout(); __appsLayout(); if (observed.join() !== '200' || input.getBoundingClientRect().x !== 20 || input.clientHeight !== 80) throw Error('Layout observation');", "test").unwrap();
}

#[test]
fn media_queries_follow_viewport_changes() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><head><style>@media (min-width:40em) { #target { color:red; } }</style></head><body><div id='target'></div></body></html>"),
        dirty: false,
    });
    let engine = Engine::new(Api::load().unwrap(), host.clone(), Role::Renderer(1)).unwrap();
    engine.eval(r#"
        const query = matchMedia('(min-width:40em)');
        if (!(query instanceof MediaQueryList) || !query.matches || query.media !== '(min-width:40em)') throw Error('Media query result');
        if (!matchMedia('screen and (max-height:600px)').matches || !matchMedia('print, (pointer:fine)').matches) throw Error('Media query list');
        if (matchMedia('print').matches || matchMedia('(unknown:yes)').matches) throw Error('Unsupported media query');
        if (getComputedStyle(document.getElementById('target')).color !== 'red') throw Error('CSS media query disagrees');
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
        if (getComputedStyle(document.getElementById('target')).color === 'red') throw Error('CSS media query did not resize');
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
    let engine = Engine::new(Api::load().unwrap(), host, Role::Renderer(1)).unwrap();
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
    let engine = Engine::new(Api::load().unwrap(), host, Role::Renderer(1)).unwrap();
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
      if(getComputedStyle(inside).color!=='#123456'||getComputedStyle(document.getElementById('outside')).color==='#123456')throw Error('Shadow styles leaked');
      existing.style.whiteSpace='pre';inside.innerHTML='<span>  const value = 1</span>';
      if(getComputedStyle(inside.firstChild).whiteSpace!=='pre')throw Error('Shadow whitespace inheritance failed');
      inside.firstChild.style.whiteSpace='normal';
      if(getComputedStyle(inside.firstChild.firstChild).whiteSpace!=='normal')throw Error('Text whitespace override failed');
      const sheet=new CSSStyleSheet();sheet.replaceSync('#inside {color:#654321}');existing.shadowRoot.adoptedStyleSheets=[sheet];
      if(getComputedStyle(inside).color!=='#654321')throw Error('Adopted sheet did not apply');
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
    let engine = Engine::new(Api::load().unwrap(), host.clone(), Role::Renderer(1)).unwrap();
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
        if (getComputedStyle(edit).color !== '#123456' || getComputedStyle(slot).display !== 'contents') throw Error('Slot styles');
        if (getComputedStyle(host).backgroundColor !== 'white' || getComputedStyle(host).color !== '#445566') throw Error('Host styles');
        if (getComputedStyle(host.querySelector('em')).color !== '#987654') throw Error('Slotted inheritance');
        host.style.color = 'orange';
        if (getComputedStyle(host).color !== 'orange' || getComputedStyle(root.querySelector('header')).color !== 'orange') throw Error('Outer host cascade');
        root.querySelector('style').textContent += ':host {color:purple!important}';
        if (getComputedStyle(host).color !== 'purple') throw Error('Important host cascade');
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
    let engine = Engine::new(Api::load().unwrap(), host, Role::Renderer(1)).unwrap();
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
        __appsDispatch('click', [text._id]);
        if (events.join() !== 'pointerdown,pointerup,keydown,click') throw Error('Native event stopped at shadow root');
    "#, "test").unwrap();
}

#[test]
fn hit_testing_follows_paint_order_clipping_and_shadow_trees() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><body><div id='back'></div><div id='front'></div><div id='clip' style='overflow:hidden'><span id='child'></span></div><test-hit></test-hit></body></html>"),
        dirty: false,
    });
    let engine = Engine::new(Api::load().unwrap(), host.clone(), Role::Renderer(1)).unwrap();
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
fn computed_styles_follow_mutations_and_resize() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><style>.item {color:red} @media (min-width:900px) {.item {color:blue}}</style><body><div class='item'></div></body></html>"),
        dirty: false,
    });
    let engine = Engine::new(Api::load().unwrap(), host.clone(), Role::Renderer(1)).unwrap();
    engine.eval("const item=document.querySelector('.item');for(let i=0;i<3;i++)if(getComputedStyle(item).color!=='red')throw Error('Initial style');", "test").unwrap();
    host.borrow_mut().viewports.insert(1, [1000., 600.]);
    engine.eval(r#"
        if(getComputedStyle(item).color!=='blue')throw Error('Viewport change');
        document.querySelector('style').textContent='.item {color:green}';
        if(getComputedStyle(item).color!=='green')throw Error('Stylesheet mutation');
        item.style.color='purple';
        if(getComputedStyle(item).color!=='purple')throw Error('Inline mutation');
        item.removeAttribute('style');
        if(getComputedStyle(item).color!=='green')throw Error('Attribute removal');
        item.className='';
        if(getComputedStyle(item).color==='green')throw Error('Class removal');
        const root=item.attachShadow({mode:'open'});root.innerHTML='<span style="color:var(--ink)">text</span>';
        item.style.setProperty('--ink','red');
        if(getComputedStyle(root.firstChild).color!=='red')throw Error('Shadow inheritance');
        item.style.setProperty('--ink','blue');
        if(getComputedStyle(root.firstChild).color!=='blue')throw Error('Shadow invalidation');
    "#, "test").unwrap();
}

#[test]
fn hostile_selectors_canvas_data_and_traversal() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><body><ul id='list'><li id='a'>1</li><li id=\"it's\">2</li><li id='c'>3</li></ul><svg><g><circle></circle></g><foreignObject><div id='html'></div></foreignObject></svg></body></html>"),
        dirty: false,
    });
    let engine = Engine::new(Api::load().unwrap(), host, Role::Renderer(1)).unwrap();
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
        doc: Document::parse("<html><head><style>#outer:hover{color:red} #inner:hover{color:blue} #outer:focus-within{background-color:green} #inner:active{font-weight:700} #inner:focus{border-top-width:3px}</style></head><body><div id='outer'><div id='inner' tabindex='0'></div></div><div id='other'></div><test-card></test-card></body></html>"),
        dirty: false,
    });
    let engine = Engine::new(Api::load().unwrap(), host.clone(), Role::Renderer(1)).unwrap();
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
        if (getComputedStyle(outer).color !== 'red' || getComputedStyle(inner).color !== 'blue') throw Error('Hover styles');
        move(12, 12);
        check('', 'same target');
        move(80, 80);
        check('pointerout:inner>outer,pointerleave:inner>outer,pointerover:outer>inner,mouseout:inner>outer,mouseleave:inner>outer,mouseover:outer>inner', 'leave child');
        if (getComputedStyle(inner).color !== 'red') throw Error('Inner hover was not cleared');
        move(10, 150);
        check('pointerout:outer>other,pointerleave:outer>other,pointerover:other>outer,pointerenter:other>outer,mouseout:outer>other,mouseleave:outer>other,mouseover:other>outer,mouseenter:other>outer', 'sibling');
        if (getComputedStyle(outer).color === 'red') throw Error('Outer hover was not cleared');
        move(10, 210);
        if (getComputedStyle(card).color !== 'purple') throw Error('Shadow host is not hovered');
        move(400, 400);
        if (getComputedStyle(card).color === 'purple') throw Error('Shadow host hover was not cleared');
        log.length = 0;
        __appsDispatch('pointerdown', [inner._id], {pointerId:1, pointerType:'mouse', clientX:10, clientY:10, button:0});
        if (getComputedStyle(inner).fontWeight !== '700' || getComputedStyle(outer).backgroundColor !== 'green' || getComputedStyle(inner).borderTopWidth !== '3px') throw Error('Active or focus styles');
        if (inner.attributes.length !== 2 || inner.outerHTML.includes('apps') || inner.hasAttribute('data-apps-focus')) throw Error('Interaction state leaked into attributes');
        __appsDispatch('pointerup', [], {pointerId:1, pointerType:'mouse', clientX:600, clientY:10});
        if (getComputedStyle(inner).fontWeight === '700') throw Error('Active was not cleared');
        inner.blur();
        if (getComputedStyle(outer).backgroundColor === 'green') throw Error('Focus within was not cleared');
        move(10, 10);
        log.length = 0;
        __appsDispatch('pointerexit', [], {pointerId:1, pointerType:'mouse', clientX:10, clientY:10});
        check('pointerout:inner,pointerleave:inner,pointerleave:outer,mouseout:inner,mouseleave:inner,mouseleave:outer', 'exit');
        if (getComputedStyle(outer).color === 'red' || document.querySelector(':hover')) throw Error('Exit kept hover');
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
    let engine = Engine::new(Api::load().unwrap(), host, Role::Renderer(1)).unwrap();
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
