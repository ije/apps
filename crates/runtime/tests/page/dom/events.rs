use super::*;

#[test]
fn scroll_events_coalesce_and_synchronize_native_and_script_scrolling() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><body><main><div id='left' style='overflow:auto'></div><div id='right' style='overflow:auto'></div></main></body></html>"),
        dirty: false, ..Default::default()
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
    host.borrow().pages[&1].state.geometry.scroll[&id]
        .set_offset(renderer::point(renderer::px(-60.), renderer::px(-25.)));
    engine.eval(r#"
        __appsLayout(); __appsLayout();
        if(JSON.stringify(received)!=='[["capture","left"],["left",60,25],["capture","right"],["right",60,25]]') throw Error(JSON.stringify(received));
        received.length=0;
        const root=document.body.attachShadow({mode:'open'}), inner=document.createElement('div');
        inner.style.overflow='auto';
        root.append(inner);
        document.addEventListener('scroll',()=>received.push('outside'),true);
        root.addEventListener('scroll',()=>received.push('shadow'),true);
        inner.scrollLeft=8; __appsLayout();
        if(JSON.stringify(received)!=='["shadow"]') throw Error('Scroll escaped shadow root');
    "#, "test").unwrap();
}

#[test]
fn mutations_preserve_tree_and_event_order() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<html><body><main id='root'><p>old</p></main></body></html>"),
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
        .state(1)
        .geometry
        .mirror
        .insert(id, [20., 30., 200., 80.]);
    engine.eval("__appsLayout(); __appsLayout(); if (observed.join() !== '194,200' || input.getBoundingClientRect().x !== 20 || input.clientHeight !== 78 || input.offsetHeight !== 80 || input.clientLeft !== 1) throw Error('Layout observation ' + observed.join());", "test").unwrap();
}

#[test]
fn inner_html_of_a_form_drops_nested_forms() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<form id=f><div id=d></div></form>"),
            dirty: false,
            ..Default::default()
        },
    );
    let engine = JsContext::new(JavaScriptCore::load().unwrap(), host, Role::Renderer(1)).unwrap();
    engine
        .eval(
            r#"
      const d=document.getElementById('d');d.innerHTML='<form id=n><input name=a></form>b';
      if(d.innerHTML!=='<input name="a">b')throw Error(d.innerHTML);
      const out=document.createElement('div');out.innerHTML='<form id=n><input></form>';
      if(out.querySelectorAll('form').length!==1)throw Error(out.innerHTML);
    "#,
            "test",
        )
        .unwrap();
}

#[test]
fn shadow_slots_render_light_dom_and_receive_pointer_events() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><head><style>button { color:#123456; }</style></head><body><test-slots><button id='edit'>Edit</button><span slot='missing'>hidden</span><em>text</em></test-slots></body></html>"),
        dirty: false, ..Default::default()
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
            .state(1)
            .geometry
            .mirror
            .insert(id, [0., 0., 100., 40.]);
    }
    engine.eval(r#"
        if (document.elementFromPoint(10,10) !== edit) throw Error('Slotted button is not hit-testable');
        edit.slot = 'missing';
        if (slot.assignedNodes().length || slot.assignedNodes({flatten:true})[0].id !== 'fallback') throw Error('Fallback assignment');
        if (root.elementFromPoint(10,10)?.id !== 'fallback' || document.elementFromPoint(10,10) !== host) throw Error('Fallback is not hit-testable');
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
            ..Default::default()
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
fn aria_properties_reflect_and_select_focuses_fields() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><body><button id='toggle'></button><input id='field' value='text'></body></html>"),
        dirty: false, ..Default::default()
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
fn pointer_hover_tracks_boundary_events_and_interaction_states() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><head><style>#outer:hover{color:red} #inner:hover{color:blue} #outer:focus-within{background-color:green} #inner:active{font-weight:700} #inner:focus{border-top:3px solid}</style></head><body><div id='outer'><div id='inner' tabindex='0'></div></div><div id='other'></div><test-card></test-card></body></html>"),
        dirty: false, ..Default::default()
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
            .state(1)
            .geometry
            .mirror
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
fn uncaught_page_errors_fire_error_events() {
    let (_, engine) = checked_page(
        "<html><body></body></html>",
        r#"
        globalThis.seen = [];
        addEventListener('error', e => { seen.push(e.constructor.name + ':' + e.message + ':' + (e.lineno > 0) + ':' + (e.error?.message === e.message.slice(16)));});
        addEventListener('unhandledrejection', e => { seen.push('rejection:' + e.reason.message); e.preventDefault(); });
        window.onerror = (message, source, line, column, error) => { seen.push('onerror:' + [typeof message, typeof source, typeof line, typeof column, error.message]); return error.message !== 'timer'; };
        setTimeout(() => { throw Error('timer'); });
        queueMicrotask(() => { throw Error('micro'); });
        requestAnimationFrame(() => { throw Error('frame'); }); __appsAnimationFrame();
        Promise.reject(Error('promise'));
        const event = new ErrorEvent('error', { message: 'm', lineno: 3, error: 1 });
        check('ErrorEvent fields', () => event.message === 'm' && event.filename === '' && event.lineno === 3 && event.colno === 0 && event.error === 1);
        let canceled; document.body.onerror = e => { canceled = e; return true; }; document.body.dispatchEvent(new ErrorEvent('error'));
        check('element onerror gets the event', () => canceled instanceof ErrorEvent);
        "#,
    );
    for _ in 0..5 {
        engine.tick().unwrap();
    }
    assert_eq!(
        engine.eval("seen.join('\\n')", "test").unwrap(),
        "ErrorEvent:Uncaught Error: frame:true:true\nonerror:string,string,number,number,frame\nErrorEvent:Uncaught Error: micro:true:true\nonerror:string,string,number,number,micro\nErrorEvent:Uncaught Error: timer:true:true\nonerror:string,string,number,number,timer\nrejection:promise"
    );
}

#[test]
fn inner_text_selections_and_table_displays_follow_rendering() {
    checked_page(
        "<html><body><div id='n'>\n  <div>\n    <p>one   two</p>\n    <p>three</p>\n  </div>\n  <div> four <b>five</b></div>\n</div>\
         <div id='br'><div>a<br></div><div>b<br><br></div>c<br></div>\
         <div id='pre'><pre>  a  b\n  c </pre><span style='white-space:pre-wrap'> x  y </span><span style='white-space:pre-line'> p  q\n r </span>z</div>\
         <table id='t'><thead><tr><th>h1</th><th>h2</th></tr></thead><tbody><tr><td>a</td><td>b</td></tr><tr><td>c</td><td></td></tr></tbody></table>\
         <div id='tt'>x<table><tr><td>a <div>b</div></td><td>c</td></tr></table>y</div>\
         <div id='l'><ul><li>one</li><li>two <ol><li>three</li></ol></li></ul>after</div>\
         <div id='pt'><p>para</p>text<h1>head</h1>tail</div>\
         <div id='h'>a<span style='display:none'>b</span><span style='visibility:hidden'>c<span style='visibility:visible'>d</span></span>f</div>\
         <div id='f' style='display:flex'><span>a</span> <span>b</span></div>\
         <div id='s'><p>one   two</p>\n<div>three <b>four</b> five</div>\n<table><tr><td>a</td><td>b</td></tr></table></div>\
         <table id='g' style='display:grid'><tbody style='display:contents'><tr style='display:contents'><td style='display:block'>g</td></tr></tbody></table></body></html>",
        r#"
        const text = id => document.getElementById(id).innerText;
        check('blocks and white space', () => text('n') === 'one two\n\nthree\n\nfour five');
        check('br at block ends', () => text('br') === 'a\n\nb\n\n\nc\n');
        check('white-space', () => text('pre') === '  a  b\n  c \n x  y  p q\nr z');
        check('table', () => text('t') === 'h1\th2\na\tb\nc\t');
        check('blocks in cells', () => text('tt') === 'x\na\nb\n\tc\ny');
        check('lists', () => text('l') === 'one\ntwo\nthree\nafter');
        check('paragraph then text', () => text('pt') === 'para\n\ntext\nhead\ntail');
        check('hidden', () => text('h') === 'adf');
        check('flex items', () => text('f') === 'a\nb');
        const detached = document.createElement('div'); detached.innerHTML = '<p>a  b</p><p>c</p>';
        check('detached', () => detached.innerText === 'a  bc');
        const s = document.getElementById('s'), range = document.createRange(), selection = getSelection();
        range.setStart(s.querySelector('p').firstChild, 4); range.setEnd(s.querySelector('b').nextSibling, 3);
        selection.removeAllRanges(); selection.addRange(range);
        check('selection inside text nodes', () => selection.toString() === 'two\n\nthree four fi');
        check('range stays raw', () => range.toString() === '  two\nthree four fi');
        range.selectNodeContents(s);
        check('whole selection', () => selection.toString() === 'one two\n\nthree four five\na\tb');
        const display = selector => getComputedStyle(document.querySelector(selector)).display;
        check('built-in table displays', () => [display('#t'), display('#t tbody'), display('#t tr'), display('#t td')].join() === 'table,table-row-group,table-row,table-cell');
        check('author table displays', () => [display('#g'), display('#g tbody'), display('#g tr'), display('#g td')].join() === 'grid,contents,contents,block');
        check('author displays in text', () => text('g') === 'g');
        const css = document.createElement('div'); css.id = 'css';
        css.innerHTML = "<div style='display:table'><div style='display:table-row'><div style='display:table-cell'>a</div><div style='display:table-cell'>b</div></div></div><table style='display:block'><tr><td>c</td></tr></table><div style='display:table-row'>d</div>";
        document.body.append(css);
        check('css table displays', () => [display('#css > div'), display('#css > div > div'), display('#css > div > div > div'), display('#css > table'), display('#css > table td'), display('#css > div:last-child')].join() === 'table,table-row,table-cell,block,table-cell,table-row');
        check('css tables in text', () => text('css') === 'a\tb\nc\nd');
        const host = document.createElement('div'); host.innerHTML = '<span>light</span>'; document.body.append(host);
        const root = host.attachShadow({ mode: 'open' }); root.innerHTML = '<template><slot></slot></template><slot id=slot></slot>';
        check('slots skip templates', () => root.querySelector('#slot').assignedNodes().length === 1 && host.firstChild.assignedSlot === root.querySelector('#slot') && host.innerText === 'light');
        "#,
    );
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
        check('primary sequence', () => log.join() === 'pointerdown:0:1:0:true,mousedown:0:1:1:true,pointerup:0:0:0:true,mouseup:0:0:1:true,click:0:0:1:true');
        check('primary press focuses', () => document.activeElement === a);
        log.length = 0; press(a, 2);
        check('secondary sequence', () => log.join() === 'pointerdown:2:2:0:true,mousedown:2:2:1:true,contextmenu:2:2:0:true,pointerup:2:0:0:true,mouseup:2:0:1:true,auxclick:2:0:1:true');
        log.length = 0; press(a, 1);
        check('middle sequence', () => log.join().endsWith('mouseup:1:0:1:true,auxclick:1:0:1:true') && !log.join().includes('click:1:0:1:true,') && !log.join().includes('contextmenu'));
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
        r.step = 'any'; r.min = '1'; r.max = '1e30';
        const any = ['1.6494845360824741', '3.999999999999999', '1.2e2', '0.0000001234', '1152921504606846976', '12345678901234567.5', ' 5', '-0'].map(v => (r.value = v, r.value));
        check('step any keeps 15 digits like Chrome ' + any, () => any.join() === '1.64948453608247,4,1.2e+2,1,1.15292150460684697e+18,1.23456789012346e+16,5e+29,1');
        "#,
    );
    let id: usize = engine
        .eval("document.getElementById('r')._id", "test")
        .unwrap()
        .parse()
        .unwrap();
    host.borrow_mut()
        .state(1)
        .geometry
        .mirror
        .insert(id, [0., 0., 116., 16.]);
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
fn file_lists_and_input_type_changes_like_chrome() {
    checked_page(
        "<html><body><form id=fm><input id=f name=f type=file></form><input id=t value=12345></body></html>",
        r#"
        const f = document.getElementById('f'), t = document.getElementById('t'), dt = new DataTransfer();
        dt.items.add(new File(['abc'], 'a.txt')); dt.items.add(new File(['d'], 'b.txt'));
        const list = dt.files;
        check('FileList', () => list instanceof FileList && list === dt.files && !Array.isArray(list) && String(list) === '[object FileList]' && list.length === 2 && list.item(1).name === 'b.txt' && list.item(2) === null && [...list].length === 2);
        let illegal; try { new FileList(); } catch (e) { illegal = e.name; }
        check('no FileList constructor', () => illegal === 'TypeError');
        check('empty files', () => f.files instanceof FileList && f.files.length === 0 && f.files === f.files);
        f.files = list;
        check('files setter', () => f.files === list && f.value === 'C:\\fakepath\\a.txt');
        dt.items.add(new File([''], 'c.txt'));
        check('live list', () => f.files.length === 3);
        let typed; try { f.files = []; } catch (e) { typed = e.name; }
        f.files = null;
        check('setter types', () => typed === 'TypeError' && f.files === list);
        const clone = f.cloneNode();
        check('a clone has a new list of the files', () => clone.files !== list && clone.files.length === 3 && clone.value === 'C:\\fakepath\\a.txt');
        document.getElementById('fm').reset();
        check('reset empties the list', () => list.length === 0 && f.value === '' && !new FormData(document.getElementById('fm')).get('f').name);
        f.files = clone.files; f.type = 'text';
        check('a text type has no files and no fake path', () => f.files === null && f.value === '');
        f.type = 'file';
        check('files go with the file type', () => f.files.length === 0);
        f.files = clone.files; f.value = '';
        check('clearing the value empties the list', () => clone.files.length === 0 && f.value === '');
        t.setSelectionRange(1, 3, 'backward'); t.type = 'password';
        check('selection stays between text types', () => t.selectionStart === 1 && t.selectionEnd === 3);
        t.type = 'number'; t.type = 'text';
        check('a type that gains the selection API starts at 0', () => t.selectionStart === 0 && t.selectionEnd === 0 && t.selectionDirection === 'none');
        "#,
    );
}

#[test]
fn focus_editing_fonts_and_xml_follow_the_standard() {
    checked_page(
        "<html><body><button id=b1>1</button><x-host id=h><button id=light>light</button></x-host><button id=b3>3</button><textarea id=t></textarea><svg viewBox='0 0 10 10'><text>a&nbsp;b</text><foreignObject><div>x<br>y</div></foreignObject></svg></body></html>",
        r#"
        const $ = id => document.getElementById(id), root = $('h').attachShadow({ mode: 'open' }), order = [];
        root.innerHTML = '<button id=inner>inner</button><slot></slot>';
        $('b1').focus();
        for (let i = 0; i < 4; i++) { __appsDispatch('keydown', [], { key: 'Tab', code: 'Tab' }); order.push(root.activeElement?.id ?? document.activeElement.id); }
        check('tab enters slots', () => order.join() === 'inner,light,b3,t');
        const t = $('t'), key = (name, options = {}) => __appsDispatch('keydown', [], { key: name, code: name, ...options }), edits = [];
        t.focus();
        for (const value of ['ab👨‍👩‍👧', 'xé', 'a🇫🇷🇩🇪', '🇫🇷'.repeat(40), 'q🏳️‍🌈']) { t.value = value; t.setSelectionRange(value.length, value.length); key('Backspace'); edits.push(t.value); }
        t.value = '🇫🇷🇩🇪x'; t.setSelectionRange(0, 0); key('Delete'); edits.push(t.value);
        const mac = navigator.platform.includes('Mac');
        t.value = 'foo  bar  '; t.setSelectionRange(10, 10); key('Backspace', { altKey: mac, ctrlKey: !mac }); edits.push(t.value);
        t.value = '  foo bar'; t.setSelectionRange(0, 0); key('Delete', { altKey: mac, ctrlKey: !mac }); edits.push(t.value);
        check('graphemes and words', () => JSON.stringify(edits) === JSON.stringify(['ab', 'x', 'a🇫🇷', '🇫🇷'.repeat(39), 'q', '🇩🇪x', 'foo  ', ' bar']));
        const data = new FontFace('X', new Uint8Array([0, 1, 2, 3]).buffer), url = new FontFace('Y', 'url(y.woff2)');
        check('font data loads', () => data.status === 'error' && url.status === 'unloaded');
        const xml = new XMLSerializer();
        check('xml', () => xml.serializeToString(document.querySelector('svg')) === '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><text>a b</text><foreignObject><div xmlns="http://www.w3.org/1999/xhtml">x<br />y</div></foreignObject></svg>' && xml.serializeToString(document.createTextNode('a <&>')) === 'a &lt;&amp;&gt;');
        check('xml attributes', () => { const div = document.createElement('div'); div.setAttribute('title', '"\n'); return xml.serializeToString(div) === '<div xmlns="http://www.w3.org/1999/xhtml" title="&quot;&#10;"></div>'; });
        "#,
    );
}

#[test]
fn focus_follows_focusability_removal_and_shadow_roots() {
    checked_page(
        "<html><body><div id='plain'>x</div><a id='link' href='#x'>l</a><a id='nolink'>n</a><button id='b'>b</button><div id='tab' tabindex='2'></div><div id='host'></div><input id='first' tabindex='1'></body></html>",
        r#"
        const $ = id => document.getElementById(id);
        $('plain').focus();
        check('non-focusable ignored', () => document.activeElement === document.body);
        check('link tabIndex', () => $('link').tabIndex === 0 && $('nolink').tabIndex === 0 && $('plain').tabIndex === -1 && $('b').tabIndex === 0);
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
        document.insertBefore(document.createComment('first'), document.documentElement);
        const order = [], top = document.childNodes, kinds = [...top].map(node => node.nodeType).join();
        for (let n = 0; n < 4; n++) { __appsDispatch('keydown', [], { key: 'Tab' }); order.push(document.activeElement.id); }
        check('tab order', () => order.join() === 'first,tab,link,first');
        check('tab keeps node lists in order', () => kinds === '8,1' && [...top].map(node => node.nodeType).join() === kinds);
        const link = document.head.appendChild(document.createElement('link')); link.rel = 'stylesheet';
        link.getBoundingClientRect();
        check('a link without a stylesheet has no sheet', () => link.sheet === null);
        __appsDispatch('keydown', [], { key: 'Tab', shiftKey: true });
        check('shift tab', () => document.activeElement.id === 'link');
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
fn events_trust_errors_listener_signals_and_shadow_paths() {
    checked_page(
        "<html><body><form id='f'><input id='cb' type='checkbox'></form></body></html>",
        r#"
        const log = [], cb = document.getElementById('cb'), f = document.getElementById('f');
        for (const type of ['click', 'input', 'change', 'submit', 'reset', 'invalid']) f.addEventListener(type, e => log.push(type + ':' + e.isTrusted), true);
        cb.click(); f.requestSubmit(); f.reset(); cb.required = true; cb.checkValidity();
        check('engine events are trusted and script clicks are not', () => log.join() === 'click:false,input:true,change:true,submit:true,reset:true,invalid:true');
        let trusted; cb.addEventListener('change', e => { if (e.isTrusted) { trusted = e; } });
        cb.click(); log.length = 0; cb.dispatchEvent(trusted);
        check('dispatchEvent makes events untrusted', () => log.join() === 'change:false' && !trusted.isTrusted);
        cb.indeterminate = true; cb.addEventListener('click', e => e.preventDefault(), { once: true }); cb.click();
        check('a canceled click restores indeterminate', () => cb.indeterminate);
        const errors = [];
        addEventListener('error', e => { errors.push(e.error.message); e.preventDefault(); if (e.error.message === 'first') throw Error('nested'); });
        cb.addEventListener('boom', () => { throw Error('first'); }); cb.dispatchEvent(new Event('boom'));
        reportError(Error('second'));
        check('listener errors reach window error listeners once', () => errors.join() === 'first,second');
        const ctrl = new AbortController(); let hits = 0; const fn = () => hits++;
        cb.addEventListener('w', fn, { signal: ctrl.signal }); cb.removeEventListener('w', fn); cb.addEventListener('w', fn); ctrl.abort(); cb.dispatchEvent(new Event('w'));
        check('abort removes only its own listener', () => hits === 1);
        const host = document.createElement('div'), open = document.createElement('div'); document.body.append(host, open);
        const inner = document.createElement('span'); host.attachShadow({ mode: 'closed' }).append(inner);
        let path; document.addEventListener('y', e => { path = e.composedPath(); });
        inner.dispatchEvent(new Event('y', { bubbles: true, composed: true }));
        check('composedPath hides closed trees', () => path[0] === host && path.length === 5);
        const root = open.attachShadow({ mode: 'open' }), a = document.createElement('i'), b = document.createElement('b'); root.append(a, b);
        let seen = 'none', inside; root.addEventListener('mouseover', e => { inside = e.relatedTarget; }); document.addEventListener('mouseover', e => { seen = e.relatedTarget; });
        const over = new MouseEvent('mouseover', { bubbles: true, composed: true, relatedTarget: b }); a.dispatchEvent(over);
        check('relatedTarget in the same tree stops at the shadow root', () => inside === b && seen === 'none' && over.relatedTarget === null);
        b.dispatchEvent(new MouseEvent('mouseover', { bubbles: true, composed: true, relatedTarget: cb }));
        check('relatedTarget outside the tree passes', () => seen === cb);
        const d = document.createElement('div'); d.setAttribute('hidden', 'until-found');
        const mo = new MutationObserver(() => {}); mo.observe(d, { attributes: true });
        check('toggleAttribute keeps a present attribute', () => d.toggleAttribute('hidden', true) && d.getAttribute('hidden') === 'until-found' && mo.takeRecords().length === 0);
        "#,
    );
}

#[test]
fn form_controls_keep_radio_groups_values_and_clones() {
    checked_page(
        "<html><body><form id='f'><input type=radio name=r value=1 checked><input type=radio name=r value=2 checked><select id=s name=s><option>a</option><option>b</option></select></form></body></html>",
        r#"
        const f = document.getElementById('f'), [r1, r2] = f.querySelectorAll('[type=radio]');
        check('the last parsed checked radio wins', () => !r1.checked && r2.checked && !r1.matches(':checked') && [...new FormData(f)].map(e => e.join('=')).join('&') === 'r=2&s=a');
        r1.checked = true; r2.removeAttribute('checked'); r2.setAttribute('checked', '');
        check('the checked attribute checks a clean radio', () => r2.checked && !r1.checked);
        r1.checked = true; f.reset(); check('reset restores the group', () => r2.checked && !r1.checked);
        const c = document.createElement('input'); c.type = 'checkbox'; c.checked = false; c.defaultChecked = true;
        check('the checked attribute leaves a dirty checkbox', () => !c.checked);
        const i = document.createElement('input'); i.value = 'a\nb'; check('text values drop newlines', () => i.value === 'ab' && i.selectionStart === 2 && i.selectionEnd === 2);
        i.type = 'number'; i.value = 'abc'; check('number values sanitize', () => i.value === '');
        i.type = 'email'; i.value = ' a@b.c '; check('email values trim', () => i.value === 'a@b.c');
        i.type = 'text'; i.value = 'typed'; const t = document.createElement('textarea'); t.value = 'note';
        c.checked = true; const o = document.createElement('option'); o.selected = true;
        check('clones keep form state', () => i.cloneNode().value === 'typed' && t.cloneNode().value === 'note' && c.cloneNode().checked && o.cloneNode().selected);
        const s = document.getElementById('s'); s.value = 'zzz';
        check('an unknown value selects nothing', () => s.selectedIndex === -1);
        s.append(new Option('c')); check('new options select the first again', () => s.selectedIndex === 0);
        "#,
    );
}

#[test]
fn text_field_selection_follows_chrome_on_value_writes_and_tab_focus() {
    checked_page(
        "<html><body><button id=s>s</button><input id=a value=hello><textarea id=t>abc def</textarea><input id=n type=number value=12><div contenteditable id=ce>x</div><input id=d value=xyz></body></html>",
        r#"
        const $ = id => document.getElementById(id), sel = el => [el.selectionStart, el.selectionEnd].join();
        const a = $('a'), t = $('t');
        check('a fresh field has its caret at the start', () => sel(a) === '0,0' && sel(t) === '0,0');
        a.value = a.value; t.value = t.value;
        check('the same value keeps the caret', () => sel(a) === '0,0' && sel(t) === '0,0');
        $('d').defaultValue = 'longer'; check('a new default value keeps the caret', () => $('d').value === 'longer' && sel($('d')) === '0,0');
        a.value = 'other'; check('a new value puts the caret at the end', () => sel(a) === '5,5');
        a.setSelectionRange(1, 2); t.setSelectionRange(1, 2); $('s').focus(); a.focus();
        check('script focus keeps the selection', () => sel(a) === '1,2');
        $('s').focus();
        const seen = []; a.addEventListener('focus', () => seen.push(sel(a)));
        __appsDispatch('keydown', [], { key: 'Tab' });
        check('tab into an input selects all', () => document.activeElement === a && sel(a) === '0,5' && seen.join() === '0,5');
        __appsDispatch('keydown', [], { key: 'Tab' });
        check('tab into a textarea keeps its selection', () => document.activeElement === t && sel(t) === '1,2');
        __appsDispatch('keydown', [], { key: 'Tab' });
        check('tab into a number input selects all', () => document.activeElement === $('n') && $('n')._start === 0 && $('n')._end === 2);
        __appsDispatch('keydown', [], { key: 'Tab', shiftKey: true }); __appsDispatch('keydown', [], { key: 'Tab', shiftKey: true });
        check('shift tab back selects all again', () => document.activeElement === a && sel(a) === '0,5');
        const form = document.createElement('form'); form.innerHTML = '<input value=start>'; document.body.append(form);
        const reset = form.firstChild; reset.value = 'typed text'; reset.setSelectionRange(1, 2); form.reset();
        check('reset puts the caret at the end', () => reset.value === 'start' && sel(reset) === '5,5');
        form.innerHTML = '<input value=abc><input value=abc><textarea>abc</textarea><input>';
        const [same, dirty, area, fresh] = form.children; dirty.value = 'abc';
        for (const field of [same, dirty, area]) field.setSelectionRange(1, 2);
        fresh.defaultValue = 'abc'; form.reset();
        check('reset keeps the selection when the value stays', () => [same, dirty, area].every(field => sel(field) === '1,2') && fresh.value === 'abc' && sel(fresh) === '0,0');
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
fn script_focus_in_a_press_keeps_the_last_focus_modality() {
    let (_, engine) = css_page(
        "<html><body><div id=d tabindex=-1>d</div><button id=c>c</button><p id=p>p</p><input id=r type=range></body></html>",
    );
    engine.eval(r#"
        const $ = id => document.getElementById(id);
        const press = node => { __appsDispatch('pointerdown', [node._id], { pointerId: 1, pointerType: 'mouse', clientX: 1, clientY: 1, button: 0 }); __appsDispatch('pointerup', [node._id], { pointerId: 1, pointerType: 'mouse', clientX: 1, clientY: 1, button: 0 }); };
        $('d').addEventListener('pointerdown', () => $('d').focus());
        $('p').addEventListener('pointerdown', event => { event.preventDefault(); $('r').focus(); });
        press($('d'));
        if (document.activeElement !== $('d') || !$('d').matches(':focus-visible')) throw Error('Focus in a press before any mouse focus');
        press($('c'));
        press($('d'));
        if (document.activeElement !== $('d') || $('d').matches(':focus-visible')) throw Error('Focus in a press after a mouse focus');
        __appsDispatch('keydown', [], { key: 'a', code: 'KeyA' });
        press($('p'));
        if (document.activeElement !== $('r') || !$('r').matches(':focus-visible')) throw Error('Focus in a press after a key');
    "#, "test").unwrap();
}

#[test]
fn presses_and_labels_set_the_focus_modality_like_chrome() {
    let (_, engine) = css_page(
        "<html><body><button id=a>a</button><button id=b>b</button><p id=p>p</p><p id=x>x</p><label id=l>label <input id=c type=checkbox></label></body></html>",
    );
    engine.eval(r#"
        const $ = id => document.getElementById(id), shown = id => document.activeElement === $(id) && $(id).matches(':focus-visible');
        const send = (name, node) => __appsDispatch(name, [node._id], { pointerId: 1, pointerType: 'mouse', clientX: 1, clientY: 1, button: 0 });
        const click = node => { send('pointerdown', node); send('pointerup', node); __appsDispatch('click', [node._id], { clientX: 1, clientY: 1, button: 0, detail: 1 }); };
        const key = key => __appsDispatch('keydown', [], { key, code: key });
        $('x').addEventListener('pointerdown', event => event.preventDefault());
        click($('a')); key('ArrowDown'); click($('l'));
        if (document.activeElement !== $('c') || $('c').matches(':focus-visible')) throw Error('A label click after a key');
        click($('a')); key('x'); click($('p')); $('b').focus();
        if ($('b').matches(':focus-visible')) throw Error('Script focus after a press on text');
        click($('a')); key('x'); click($('x')); $('b').focus();
        if (!shown('b')) throw Error('Script focus after a canceled press keeps the key');
        $('a').focus(); key('Tab'); click($('b'));
        if (!shown('b')) throw Error('A press on the element of a Tab keeps its ring');
        $('a').focus(); $('a').blur(); click($('x')); key('Tab');
        if (document.activeElement !== $('c')) throw Error('Tab after a canceled press starts after it');
        click($('a')); key('a'); click($('a'));
        if (!shown('a')) throw Error('A press on the focused element keeps the key');
        $('b').focus();
        if (!shown('b')) throw Error('Script focus after a press on the focused element');
        $('l').focus();
        if (document.activeElement !== $('c')) throw Error('A label gives the focus to its control');
    "#, "test").unwrap();
}

#[test]
fn shift_arrows_do_not_scroll_like_chrome() {
    let (_, engine) = css_page("<html><body><p>p</p></body></html>");
    engine.eval(r#"
        const calls = [];
        document.scrollingElement.scrollBy = options => calls.push(Object.keys(options)[0]);
        for (const [key, shiftKey] of [['ArrowRight', true], ['ArrowDown', true], ['PageDown', true], ['ArrowRight', false], [' ', true]]) __appsDispatch('keydown', [], { key, code: key, shiftKey });
        if (calls.join() !== 'left,top') throw Error('scrolls: ' + calls.join());
    "#, "test").unwrap();
}

#[test]
fn tab_goes_on_from_the_last_press_or_removed_focus() {
    let (_, engine) = css_page(
        "<html><body><button id=a>a</button><div><button id=b>b</button><button id=c>c</button></div><p id=p>p</p><button id=d>d</button><div id=e><button id=f>f</button></div><button id=g>g</button><div id=host></div><p id=q>q</p><button id=h>h</button></body></html>",
    );
    engine.eval(r#"
        const $ = id => document.getElementById(id);
        const tab = (shiftKey = false) => { __appsDispatch('keydown', [], { key: 'Tab', code: 'Tab', shiftKey }); return document.activeElement.id; };
        const press = node => { __appsDispatch('pointerdown', [node._id], { pointerId: 1, pointerType: 'mouse', clientX: 1, clientY: 1, button: 0 }); __appsDispatch('pointerup', [node._id], { pointerId: 1, pointerType: 'mouse', clientX: 1, clientY: 1, button: 0 }); };
        $('b').focus(); $('b').remove();
        if (tab() !== 'c') throw Error('Tab after removing the focused element');
        $('c').focus(); $('c').remove();
        if (tab(true) !== 'a') throw Error('Shift+Tab after removing the last child');
        press($('p'));
        if (document.activeElement !== document.body || tab() !== 'd') throw Error('Tab after a press');
        press($('p'));
        if (tab(true) !== 'a') throw Error('Shift+Tab after a press');
        $('f').focus(); $('e').textContent = '';
        if (tab() !== 'g') throw Error('Tab after emptying the parent of the focused element');
        const root = $('host').attachShadow({ mode: 'open' }); root.innerHTML = '<button id=s1>s1</button><span id=st>t</span><button id=s2>s2</button>';
        const deep = () => root.activeElement?.id ?? document.activeElement.id;
        press($('q'));
        if (tab() !== 'h') throw Error('Tab after a press past a shadow tree');
        press($('q'));
        if ((tab(true), deep()) !== 's2') throw Error('Shift+Tab after a press past a shadow tree');
        press(root.getElementById('st'));
        if ((tab(), deep()) !== 's2') throw Error('Tab after a press in a shadow tree');
        press(root.getElementById('st'));
        if ((tab(true), deep()) !== 's1') throw Error('Shift+Tab after a press in a shadow tree');
        press($('q')); $('q').remove();
        if (tab() !== 'h') throw Error('Tab after removing the pressed element');
        $('g').focus(); $('g').blur();
        if ((tab(), deep()) !== 's1') throw Error('Tab after a blur');
    "#, "test").unwrap();
}

#[test]
fn indexes_follow_ids_labels_forms_and_bases() {
    checked_page(
        "<html><head><title> a  b </title></head><body><form id='f'><input id='in' name='r' type='radio'><label for='in'>one</label></form><label>two <input id='nested'></label><input form='f' id='out' name='r' type='radio'><template><i id='hidden'></i></template><a id='link' href='x/y'>l</a></body></html>",
        r#"
        const input = document.getElementById('in'), out = document.getElementById('out'), form = document.getElementById('f');
        check('ids', () => input?.localName === 'input' && document.getElementById('hidden') === null && document.getElementById('none') === null);
        const twin = document.createElement('b'); twin.id = 'in';
        check('detached ids are not found', () => document.getElementById('in') === input);
        document.body.prepend(twin);
        check('the first element in tree order wins', () => document.getElementById('in') === twin);
        twin.remove(); input.id = 'renamed';
        check('id changes update the index', () => document.getElementById('in') === null && document.getElementById('renamed') === input);
        input.id = 'in';
        check('labels', () => input.labels.length === 1 && input.labels[0].textContent === 'one' && document.getElementById('nested').labels[0].textContent.startsWith('two'));
        check('form elements include controls outside the form', () => form.elements.length === 2 && form.elements[1] === out && form.elements.r instanceof RadioNodeList && form.elements.r[0] === input && form.elements.r[1] === out);
        out.checked = true; input.checked = true;
        check('radio groups follow the form owner', () => !out.checked);
        check('closest', () => input.closest('form') === form && input.closest(':scope') === input && input.closest('p') === null && out.closest('body') === document.body);
        check('closest throws on bad selectors', () => { try { input.closest('[['); return false; } catch (e) { return e.name === 'SyntaxError'; } });
        check('title', () => document.title === 'a b');
        const before = document.getElementById('link').href;
        const base = document.createElement('base'); base.href = 'https://example.com/dir/'; document.head.append(base);
        check('base changes resolve again', () => document.baseURI === 'https://example.com/dir/' && document.getElementById('link').href === 'https://example.com/dir/x/y');
        base.remove();
        check('removed bases stop applying', () => document.getElementById('link').href === before);
        const div = document.createElement('div');
        div.classList.remove('x'); div.classList.toggle('y', false); div.classList.add();
        check('empty token sets add no class attribute', () => !div.hasAttribute('class'));
        div.className = 'x'; div.classList.remove('x');
        check('an existing class attribute stays', () => div.getAttribute('class') === '');
        "#,
    );
}

#[cfg(target_os = "macos")]
#[test]
fn select_popup_picks_options() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<select id='s'><option>One<optgroup label='G'><option disabled>Two<option>Three</optgroup><hr><option hidden>Four<option>Five</select>"),
        dirty: false, ..Default::default()
    });
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    engine.eval(r#"
        const s = document.getElementById('s'), seen = [];
        for (const type of ['input', 'change']) s.addEventListener(type, () => seen.push(type + ':' + s.value));
        const press = () => __appsDispatch('pointerdown', [s.selectedOptions[0]._id, s._id], {pointerId:1, pointerType:'mouse', clientX:1, clientY:1, button:0, buttons:1});
        try { s.showPicker(); throw Error('showPicker() ran without a gesture'); } catch (error) { if (error.name !== 'NotAllowedError') throw error; }
        press();
        if (document.activeElement !== s || !s.matches(':active')) throw Error('The press did not reach the select');
        __appsSelectPicked(1, 3);
        if (s.value !== 'Three' || seen.join() !== 'input:Three,change:Three' || s.matches(':active')) throw Error('Picked ' + s.value + ' ' + seen + ' ' + s.matches(':active'));
        press(); __appsSelectPicked(2, -1); press(); __appsSelectPicked(3, 4); press(); __appsSelectPicked(4, 3);
        if (seen.length !== 2) throw Error('A closed menu changed the select: ' + seen);
        press(); __appsSelectPicked(5, 5);
        if (s.value !== 'Five' || s.selectedIndex !== 4) throw Error('Hidden options shift the menu: ' + s.value);
        s.multiple = true; press(); __appsSelectPicked(6, 0);
        if (s.value !== 'Five') throw Error('A list box opened a menu');
    "#, "test").unwrap();
}

#[test]
fn tab_order_includes_custom_elements_with_their_own_tab_index() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><body><button id='a'></button><x-pad id='pad' contenteditable='true'></x-pad><button id='b'></button></body></html>"),
        dirty: false, ..Default::default()
    });
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    engine.eval(r#"
        customElements.define('x-pad', class extends HTMLElement { get tabIndex() { return 0; } });
        const order = [];
        for (let n = 0; n < 3; n++) { __appsDispatch('keydown', [], { key: 'Tab' }); order.push(document.activeElement.id); }
        if (order.join() !== 'a,pad,b') throw Error(order.join());
    "#, "test").unwrap();
}

#[test]
fn events_like_chrome() {
    checked_page(
        "<html><body><form id='f'><input name='q' value='1'></form><span id='s'></span></body></html>",
        r#"
        const s = document.getElementById('s'), f = document.getElementById('f');
        let seen;
        s.addEventListener('q', e => { seen = [e.srcElement, window.event === e]; });
        s.dispatchEvent(new Event('q'));
        check('srcElement and window.event', () => seen[0] === s && seen[1] && window.event === undefined);
        check('constructors', () => [['TransitionEvent', 'propertyName'], ['AnimationEvent', 'animationName'], ['HashChangeEvent', 'newURL'], ['PopStateEvent', 'state'], ['PageTransitionEvent', 'persisted'], ['StorageEvent', 'key']].every(([name, key]) => new globalThis[name]('x')[key] !== undefined) && new TransitionEvent('t', { propertyName: 'opacity', elapsedTime: 1.5 }).elapsedTime === 1.5 && new StorageEvent('s').newValue === null);
        check('keyboard fields', () => (e => e.location === 1 && e.which === 0 && e.charCode === 13 && KeyboardEvent.DOM_KEY_LOCATION_LEFT === 1)(new KeyboardEvent('keypress', { key: 'Enter', location: 1, charCode: 13 })) && new KeyboardEvent('keydown', { keyCode: 65 }).which === 65 && new UIEvent('x').which === 0);
        check('createEvent', () => ['MessageEvent', 'StorageEvent', 'HashChangeEvent', 'CompositionEvent', 'DragEvent'].every(name => document.createEvent(name).constructor.name === name));
        check('initMouseEvent', () => (e => (e.initMouseEvent('click', true, true, window, 1, 2, 3, 4, 5, true, false, false, false, 0, null), e.clientX === 4 && e.ctrlKey && e.bubbles))(document.createEvent('MouseEvents')));
        check('toString', () => String(new Event('x')) === '[object Event]' && Object.prototype.toString.call(new File([], 'a')) === '[object File]');
        const data = [];
        f.addEventListener('formdata', e => data.push(e.constructor.name, e.bubbles, e.cancelable, e.formData.get('q')));
        new FormData(f);
        check('formdata event', () => data.join() === 'FormDataEvent,true,false,1');
        check('abort reason', () => new AbortController().signal.reason === undefined && (c => (c.abort(), c.signal.reason.message === 'signal is aborted without reason'))(new AbortController()));
        check('window.postMessage', () => typeof postMessage === 'function' && typeof requestIdleCallback === 'function');
    "#,
    );
}

#[test]
fn form_controls_like_chrome() {
    checked_page(
        "<html><body><form id='f'><input type='radio' name='r' value='x'><input type='radio' name='r' value='y' checked><input name='a'><input name='a'></form><input id='lv' type='hidden'></body></html>",
        r#"
        const f = document.getElementById('f'), input = type => Object.assign(document.createElement('input'), { type });
        check('radio node lists', () => f.elements.r instanceof RadioNodeList && f.elements.r.value === 'y' && f.elements.a.length === 2 && (f.elements.r.value = 'x', f.elements.r[0].checked));
        check('form fields', () => f.autocomplete === 'on' && f.encoding === 'application/x-www-form-urlencoded' && f.acceptCharset === '');
        const n = input('number'); n.value = '5'; n.step = '3'; n.stepUp(); const up = n.value; n.stepDown(2);
        check('number steps', () => up === '6' && n.value === '0' && (n.valueAsNumber = 2.5, n.value === '2.5') && (n.value = '', Number.isNaN(n.valueAsNumber)));
        check('any step', () => { n.step = 'any'; try { n.stepUp(); } catch (e) { return e.name === 'InvalidStateError'; } });
        const d = input('date'); d.value = '2024-02-29';
        check('dates', () => d.valueAsNumber === 1709164800000 && d.valueAsDate.toISOString() === '2024-02-29T00:00:00.000Z' && (d.valueAsDate = new Date(Date.UTC(2020, 0, 5)), d.stepUp(3), d.value === '2020-01-08'));
        const t = input('time'), m = input('month'), w = input('week'), l = input('datetime-local'); t.value = '13:45'; m.value = '2024-03'; w.value = '2024-W01'; l.value = '2024-01-02T03:04';
        check('times', () => t.valueAsNumber === 49500000 && m.valueAsNumber === 650 && w.valueAsDate.toISOString() === '2024-01-01T00:00:00.000Z' && l.valueAsNumber === 1704164640000 && l.valueAsDate === null && (w.stepUp(), w.value === '2024-W02'));
        const s = document.createElement('select'); s.add(new Option('a', 'A')); s.add(new Option('b', 'B'), 0);
        check('select keeps its option', () => s.selectedIndex === 1 && s.value === 'A');
        s.options.length = 0;
        check('options length clears', () => s.length === 0 && (s.options[0] = new Option('z'), s.length === 1 && s.options[0].text === 'z'));
        check('option selectedness', () => !new Option('t', 'v', true, false).selected);
        check('hidden inputs have no labels', () => document.getElementById('lv').labels === null);
    "#,
    );
}

#[test]
fn rendered_text_and_removed_focus_like_chrome() {
    checked_page(
        "<html><body><div id='t'><span style='text-transform:uppercase'>up</span> <span style='text-transform:lowercase'>LOW</span><textarea>ta</textarea><canvas>fallback</canvas></div><div id='box'><input id='i'></div></body></html>",
        r#"
        const $ = id => document.getElementById(id), seen = [];
        check('innerText', () => $('t').innerText === 'UP low');
        const p = document.createElement('p'); p.innerHTML = '<b>a</b><b>b</b>'; document.body.append(p);
        p.firstChild.outerText = 'x\ny';
        check('outerText lines', () => p.innerHTML === 'x<br>y<b>b</b>');
        for (const target of [document, $('box')]) for (const type of ['blur', 'focusout']) target.addEventListener(type, event => seen.push(type + ':' + (target === document ? 'document' : 'box') + ':' + event.target.id), true);
        $('i').focus(); $('box').removeChild($('i'));
        check('removed focus blurs through the old ancestors', () => seen.join() === 'blur:document:i,blur:box:i,focusout:document:i,focusout:box:i' && document.activeElement === document.body);
        const again = document.createElement('input'); again.id = 'j'; $('box').append(again); again.focus(); seen.length = 0; $('box').textContent = '';
        check('cleared parents blur too', () => seen.join() === 'blur:document:j,blur:box:j,focusout:document:j,focusout:box:j');
    "#,
    );
}

#[test]
fn native_mouse_and_key_events_carry_chrome_fields() {
    checked_page(
        "<body><div id=a></div><input id=t><label id=l for=c>l</label><input type=checkbox id=c></body>",
        r#"
        const [a, t, l, c] = ['a', 't', 'l', 'c'].map(id => document.getElementById(id)), log = [];
        for (const type of ['pointerover', 'mouseover', 'pointermove', 'mousemove', 'pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click'])
            a.addEventListener(type, e => log.push([type, e.button, e.which, e.detail, e.clientX, e.offsetX].join(' ')));
        const at = { clientX: 10.5, clientY: 10.25 };
        __appsDispatch('pointermove', [a._id], at);
        __appsDispatch('pointerdown', [a._id], { ...at, button: 0, buttons: 1, detail: 1 });
        __appsDispatch('pointerup', [a._id], { ...at, button: 0, buttons: 0 });
        check('mouse fields: ' + log.join(), () => log.join() === 'pointerover -1 0 0 10.5 10.5,mouseover 0 0 0 10 11,pointermove -1 0 0 10.5 10.5,mousemove 0 0 0 10 11,pointerdown 0 1 0 10.5 10.5,mousedown 0 1 1 10 11,pointerup 0 1 0 10.5 10.5,mouseup 0 1 1 10 11,click 0 1 1 10 11');
        log.length = 0;
        c.addEventListener('click', e => log.push('click ' + e.detail));
        __appsDispatch('pointerdown', [l._id], { ...at, button: 0, buttons: 1, detail: 1 });
        __appsDispatch('pointerup', [l._id], { ...at, button: 0, buttons: 0 });
        check('label click keeps its detail: ' + log.join(), () => log.join() === 'click 1' && c.checked);
        log.length = 0;
        for (const type of ['keydown', 'keypress', 'keyup']) t.addEventListener(type, e => log.push([type, e.key, e.keyCode, e.charCode, e.which, e.location].join(' ')));
        t.addEventListener('keypress', e => { if (e.key === 'x') e.preventDefault(); });
        t.focus();
        const typed = __appsDispatch('keydown', [], { key: 'a', code: 'KeyA', keyCode: 65 });
        const blocked = __appsDispatch('keydown', [], { key: 'x', code: 'KeyX', keyCode: 88 });
        __appsDispatch('keydown', [], { key: 'ArrowLeft', code: 'ArrowLeft', keyCode: 37 });
        __appsDispatch('modifiers', [], { shiftKey: true });
        check('which in the init', () => new KeyboardEvent('keydown', { keyCode: 13, which: 13 }).which === 13);
        check('key fields: ' + log.join(), () => typed && !blocked && log.join() === 'keydown a 65 0 65 0,keypress a 97 97 97 0,keydown x 88 0 88 0,keypress x 120 120 120 0,keydown ArrowLeft 37 0 37 0,keydown Shift 16 0 16 1');
        "#,
    );
}

#[test]
fn focus_tab_order_and_radio_keys_follow_chrome() {
    checked_page(
        "<body><input id=i1><div id=e contenteditable>ed</div><button id=h style='visibility:hidden'>h</button><a id=a0>a</a><dialog id=d open>d</dialog><input id=i2>\
         <input type=radio name=g id=r1><input type=radio name=g id=r2 disabled><input type=radio name=g id=r3></body>",
        r#"
        const $ = id => document.getElementById(id), log = [];
        for (const type of ['focus', 'blur', 'focusout', 'change']) document.addEventListener(type, e => log.push(type + ':' + e.target.id + ':' + (document.activeElement.id || document.activeElement.localName)), true);
        $('i1').focus();
        $('i1')._edited = true;
        const tab = shift => __appsDispatch('keydown', [], { key: 'Tab', keyCode: 9, shiftKey: shift });
        tab();
        check('blur sees no focused element: ' + log.join(), () => log.join() === 'focus:i1:i1,change:i1:body,blur:i1:body,focusout:i1:body,focus:e:e');
        tab();
        check('hidden boxes and dialogs are skipped: ' + document.activeElement.id, () => document.activeElement.id === 'i2');
        check('anchors have tab index 0', () => $('a0').tabIndex === 0 && $('e').tabIndex === -1);
        $('e').addEventListener('blur', () => $('r1').focus(), { once: true });
        $('e').focus(); $('i1').focus();
        check('a blur listener moves the focus: ' + document.activeElement.id, () => document.activeElement.id === 'r1');
        const clicks = [];
        for (const type of ['click', 'change']) document.addEventListener(type, e => clicks.push(type + ':' + e.target.id));
        __appsDispatch('keydown', [], { key: 'ArrowDown', keyCode: 40 });
        __appsDispatch('keydown', [], { key: 'ArrowDown', keyCode: 40 });
        __appsDispatch('keyup', [], { key: ' ', keyCode: 32 });
        check('arrows move in the group: ' + clicks.join(), () => clicks.join() === 'click:r3,change:r3,click:r1,change:r1' && $('r1').checked && document.activeElement.id === 'r1');
        "#,
    );
}

#[test]
fn editing_keys_fire_input_events_like_chrome() {
    checked_page(
        "<body><input id=i value=ab><div id=e contenteditable>one <b id=b>two</b> three</div><label id=l for=i>l</label></body>",
        r#"
        const $ = id => document.getElementById(id), log = [];
        for (const type of ['beforeinput', 'input']) document.addEventListener(type, e => log.push(type + ':' + e.inputType));
        $('i').focus(); $('i').setSelectionRange(0, 0);
        __appsDispatch('keydown', [], { key: 'Backspace', keyCode: 8 });
        __appsDispatch('keydown', [], { key: 'Enter', keyCode: 13 });
        check('nothing to delete, and Enter in a field: ' + log.join(), () => log.join() === 'beforeinput:deleteContentBackward,beforeinput:insertLineBreak' && $('i').value === 'ab');
        $('e').focus();
        check('an editing host gets a caret at its start', () => getSelection().anchorNode === $('e').firstChild && getSelection().anchorOffset === 0);
        getSelection().collapse($('b').firstChild, 3);
        __appsDispatch('keydown', [], { key: 'Enter', keyCode: 13 });
        check('a split drops empty copies and ids: ' + $('e').innerHTML, () => $('e').innerHTML === 'one <b id="b">two</b><div> three</div>');
        check('a label shares its hover', () => (__appsDispatch('pointermove', [$('l')._id], { clientX: 1, clientY: 1 }), $('i').matches(':hover')));
        const event = new MouseEvent('x', { clientX: 10.7, screenY: 3.5 });
        check('mouse events keep whole coordinates', () => event.clientX === 10 && event.screenY === 3 && new PointerEvent('x', { clientX: 10.7 }).clientX === 10.7);
        "#,
    );
}

#[test]
fn data_and_blob_requests_fire_each_event_once_with_their_url() {
    let (.., result) = offscreen_page(
        r#"
        const blob = URL.createObjectURL(new Blob(['b']));
        for (const [url, text, final] of [['data:,abc#x', 'abc', 'data:,abc'], [blob + '#f', 'b', blob]]) {
          const response = await fetch(url);
          same([response.url, response.type, await response.text()], [final, 'basic', text], 'fetch ' + url);
          const xhr = new XMLHttpRequest(), events = [];
          for (const type of ['load', 'loadend', 'readystatechange']) xhr.addEventListener(type, () => events.push(type));
          xhr.onload = () => events.push('onload');
          await new Promise(resolve => { xhr.onloadend = resolve; xhr.open('GET', url); xhr.send(); });
          same([xhr.responseURL, xhr.responseText, events.filter(type => type !== 'readystatechange')], [final, text, ['load', 'onload', 'loadend']], 'xhr ' + url);
        }
        "#,
    );
    assert_eq!(result, "ok");
}
