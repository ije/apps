use super::*;

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
        let px = |value: &str| apps_runtime::css::value::length(value, 0., 16., 800., 600.);
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

// Debug builds also compare each patched tree walk, list and mark with a full pass. Shadow trees, which take
// the full pass, come late.
#[test]
fn computed_styles_follow_random_tree_changes() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><head><style>body{color:rgb(1, 1, 1)} p + p{color:rgb(6, 6, 6)} .a{color:rgb(2, 2, 2)} li:first-child{color:rgb(4, 4, 4)} li:nth-child(3n){color:rgb(5, 5, 5)} div:empty{color:rgb(7, 7, 7)} [inert] span{color:rgb(8, 8, 8)} td:last-child{color:rgb(9, 9, 9)} .a > .b{color:rgb(3, 3, 3)} .flat{display:contents} span ~ p{background-color:rgb(1, 0, 0)} div:has(+ div){background-color:rgb(2, 0, 0)} li:last-child{background-color:rgb(3, 0, 0)} li:only-child{background-color:rgb(4, 0, 0)} li:nth-last-child(2){background-color:rgb(5, 0, 0)} li:has(> .a){background-color:rgb(6, 0, 0)} td:has(~ .a){background-color:rgb(7, 0, 0)} ul:has(.b) p{background-color:rgb(8, 0, 0)} body:has(.flat) td{background-color:rgb(9, 0, 0)}</style></head><body><ul id='list'></ul><table><tbody id='rows'></tbody></table><div id='box' class='a'></div></body></html>"),
        dirty: false,
    });
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    engine.eval(r#"
        const rules = [['body', 'rgb(1, 1, 1)'], ['p + p', 'rgb(6, 6, 6)'], ['.a', 'rgb(2, 2, 2)'], ['li:first-child', 'rgb(4, 4, 4)'], ['li:nth-child(3n)', 'rgb(5, 5, 5)'], ['div:empty', 'rgb(7, 7, 7)'], ['[inert] span', 'rgb(8, 8, 8)'], ['td:last-child', 'rgb(9, 9, 9)'], ['.a > .b', 'rgb(3, 3, 3)']];
        const expected = el => { for (let i = rules.length; i--;) if (el.matches(rules[i][0])) return rules[i][1]; return el.parentElement ? expected(el.parentElement) : 'rgb(0, 0, 0)'; };
        const places = [['span ~ p', 'rgb(1, 0, 0)'], ['div:has(+ div)', 'rgb(2, 0, 0)'], ['li:last-child', 'rgb(3, 0, 0)'], ['li:only-child', 'rgb(4, 0, 0)'], ['li:nth-last-child(2)', 'rgb(5, 0, 0)'], ['li:has(> .a)', 'rgb(6, 0, 0)'], ['td:has(~ .a)', 'rgb(7, 0, 0)'], ['ul:has(.b) p', 'rgb(8, 0, 0)'], ['body:has(.flat) td', 'rgb(9, 0, 0)']];
        let seed = 12345, step = 0;
        const random = below => (seed = Math.imul(seed, 1103515245) + 12345 >>> 0, (seed >>> 8) % below);
        const list = document.getElementById('list'), rows = document.getElementById('rows'), box = document.getElementById('box');
        const make = (tag, text) => { const el = document.createElement(tag); if (random(3) === 0) el.className = ['a', 'b', 'flat'][random(3)]; if (text) el.textContent = text; return el; };
        const item = () => { const li = make('li'); li.append(make('span', 'item'), make('p', 'one'), make('p', 'two')); return li; };
        const any = () => { const all = document.body.querySelectorAll('*'); return all[random(all.length)]; };
        const holder = () => { for (;;) { const el = any(); if (!['TABLE', 'TBODY', 'TR', 'STYLE'].includes(el.tagName)) return el; } };
        for (let i = 0; i < 12; i++) list.append(item());
        const steps = [
            () => list.insertBefore(item(), list.children[random(list.children.length + 1)] ?? null),
            () => list.children[random(list.children.length)]?.remove(),
            () => list.children.length && list.insertBefore(list.children[random(list.children.length)], list.children[random(list.children.length)]),
            () => { const el = list.querySelectorAll('span, p')[random(40)]; if (el) el.textContent = 'text ' + random(9); },
            () => any().classList.toggle(['a', 'b', 'flat'][random(3)]),
            () => { const row = document.createElement('tr'); for (let i = 1 + random(3); i--;) row.append(make('td', 'cell')); rows.insertBefore(row, rows.children[random(rows.children.length + 1)] ?? null); },
            () => rows.children[random(rows.children.length)]?.remove(),
            () => any().toggleAttribute(['inert', 'contenteditable'][random(2)]),
            () => holder().append(make(['div', 'p', 'span'][random(3)], random(2) ? 'more' : '')),
            () => box.replaceChildren(make('div'), make('p', 'x'), make('p', 'y')),
            () => { const el = holder(); if (el !== box && !el.contains(box)) box.append(el); },
            () => { const style = document.head.querySelector('style + style'); style ? style.remove() : document.head.append(make('style', '.none{color:rgb(10, 10, 10)}')); },
            () => { const el = holder(); if (el.id || el.children.length > 20) return; const wrap = make('div'); el.replaceWith(wrap); wrap.append(el); },
            () => { if (step < 120) return; const el = make('div'); el.attachShadow({ mode: 'open' }).append(make('span', 'shadow'), document.createElement('slot')); el.append(make('p', 'light')); box.append(el); },
            () => { const el = box.children[random(box.children.length)]; if (el) el.remove(); },
            () => { const el = holder(); if (!el.id) el.remove(); },
        ];
        for (; step < 200; step++) {
            for (let count = 1 + random(3); count--;) steps[random(steps.length)]();
            if (list.children.length > 30) list.replaceChildren(...Array.from(list.children).slice(0, 8));
            if (document.body.querySelectorAll('*').length > 400) box.replaceChildren();
            for (const el of document.body.querySelectorAll('*')) {
                const color = getComputedStyle(el).color;
                if (color !== expected(el)) throw Error(`step ${step}: <${el.tagName} class="${el.className}"> is ${color}, not ${expected(el)}`);
                const back = getComputedStyle(el).backgroundColor, place = places.findLast(rule => el.matches(rule[0]))?.[1] ?? 'rgba(0, 0, 0, 0)';
                if (back !== place) throw Error(`step ${step}: <${el.tagName} class="${el.className}"> has ${back}, not ${place}`);
            }
        }
    "#, "test").unwrap();
}

// Release builds hash only the changed nodes, so this is a check of them.
#[test]
fn rules_added_later_follow_names_that_nodes_already_had() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html class='dark'><head><style>p{color:rgb(1, 1, 1)}</style></head><body><div data-on='1'><p class='x'>a</p></div></body></html>"),
        dirty: false,
    });
    let engine = JsContext::new(JavaScriptCore::load().unwrap(), host, Role::Renderer(1)).unwrap();
    let styles = engine.eval(r#"
        const p = document.querySelector('p'), sheet = document.styleSheets[0], seen = [getComputedStyle(p).color];
        sheet.insertRule('.dark .x{color:rgb(2, 2, 2)}', 1);
        sheet.insertRule('[data-on] > .x{background-color:rgb(3, 3, 3)}', 2);
        seen.push(getComputedStyle(p).color, getComputedStyle(p).backgroundColor);
        document.documentElement.classList.remove('dark');
        p.parentNode.removeAttribute('data-on');
        seen.push(getComputedStyle(p).color, getComputedStyle(p).backgroundColor);
        seen.join('|')
    "#, "test").unwrap();
    assert_eq!(
        styles,
        "rgb(1, 1, 1)|rgb(2, 2, 2)|rgb(3, 3, 3)|rgb(1, 1, 1)|rgba(0, 0, 0, 0)"
    );
}

#[test]
fn script_animations_leave_important_declarations() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><style>#d{opacity:0.5 !important}</style><body><div id=d></div><div id=e style='color:red !important'></div></body></html>"),
        dirty: false,
    });
    let engine = JsContext::new(JavaScriptCore::load().unwrap(), host, Role::Renderer(1)).unwrap();
    engine.eval(r#"
        const d=document.getElementById('d'),e=document.getElementById('e');
        const a=d.animate([{opacity:0},{opacity:1}],{duration:1000,fill:'both'});a.pause();a.currentTime=250;
        e.animate([{color:'blue'},{color:'blue'}],{duration:1000,fill:'both'}).pause();
        if(getComputedStyle(d).opacity!=='0.5')throw Error('sheet '+getComputedStyle(d).opacity);
        if(getComputedStyle(e).color!=='rgb(255, 0, 0)')throw Error('inline '+getComputedStyle(e).color);
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
fn form_pseudo_classes_follow_script_state() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><body><form><input id=em type=email value=nope><input id=req required><input id=num type=number min=5 value=2><input id=plain><fieldset id=fs disabled><input id=in1 required></fieldset><select id=sel required><option value=''>-</option><option>a</option></select><input type=radio name=r id=r1 required><input type=radio name=r id=r2><input type=checkbox id=cb><textarea id=ta required></textarea></form><x-el id=x></x-el></body></html>"),
        dirty: false,
    });
    let engine = JsContext::new(JavaScriptCore::load().unwrap(), host, Role::Renderer(1)).unwrap();
    engine.eval(r#"
        const q=s=>[...document.querySelectorAll(s)].map(e=>e.id).join(),$=id=>document.getElementById(id);
        const expect=(s,v,step)=>{if(q(s)!==v)throw Error(step+': '+s+' = '+q(s));};
        expect(':invalid','em,req,num,sel,r1,r2,ta','load');
        $('em').value='a@b.co';$('req').value='x';$('num').setAttribute('min','1');$('ta').value='t';
        expect(':invalid','sel,r1,r2','values');
        $('req').value='';$('plain').setCustomValidity('bad');
        expect(':invalid','req,plain,sel,r1,r2','custom');
        $('plain').setCustomValidity('');$('sel').value='a';$('r2').checked=true;$('fs').disabled=false;
        expect(':invalid','req,in1','groups');
        const made=document.createElement('input');made.id='made';made.required=true;$('fs').append(made);
        expect('fieldset :invalid','in1,made','created');
        $('fs').disabled=true;
        expect(':invalid','req','disabled');
        $('cb').indeterminate=true;
        expect(':indeterminate','cb','indeterminate');
        $('cb').click();
        if($('cb').indeterminate||!$('cb').checked)throw Error('click clears indeterminate');
        expect(':indeterminate','','click');
        expect(':not(:defined)','x','undefined');
        customElements.define('x-el',class extends HTMLElement{});
        const y=document.createElement('y-el');y.id='y';document.body.append(y);
        expect(':not(:defined)','y','defined');
        expect('[type=EMAIL], [TYPE=radio i]','em,r1,r2','case');
    "#, "test").unwrap();
}

#[test]
fn labels_walkers_names_documents_selectors_and_validity_follow_browsers() {
    checked_page(
        "<html><body><label><input type=checkbox id=c><input id=t><textarea id=ta></textarea><span id=s>s</span></label>\
         <select id=sel required><option value=''>pick</option></select><input type=radio name=a id=r1 required><input type=radio name=a id=r2>\
         <select id=list><option>One</option><option selected>Two</option><option hidden>Hidden</option></select></body></html>",
        r#"
        const $ = id => document.getElementById(id), c = $('c');
        $('t').click(); $('ta').click(); check('a click on other controls in a label does not activate its control', () => !c.checked);
        $('s').click(); check('a click on plain label content activates its control', () => c.checked);
        const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_ELEMENT, () => true), iterator = document.createNodeIterator(document.body, NodeFilter.SHOW_ELEMENT, { acceptNode: () => '1' });
        check('filter results convert to numbers', () => walker.nextNode() === document.querySelector('label') && iterator.nextNode() === document.body);
        const error = fn => { try { fn(); } catch (e) { return e.name; } };
        check('invalid attribute names throw', () => ['', 'a b', 'a/b', 'a=b', 'a>b'].every(name => error(() => document.body.setAttribute(name, '')) === 'InvalidCharacterError') && error(() => document.body.toggleAttribute(' ', false)) === 'InvalidCharacterError' && error(() => document.createAttribute('a b')) === 'InvalidCharacterError');
        document.body.setAttribute('data-x:y', 1); document.body.toggleAttribute('data-z');
        check('valid attribute names work', () => document.body.getAttribute('data-x:y') === '1' && document.body.hasAttribute('data-z') && document.createAttribute('A').name === 'a' && document.createAttribute('a').value === '');
        check('invalid element names throw', () => ['1bad', '', 'a b', '#text'].every(name => error(() => document.createElement(name)) === 'InvalidCharacterError') && error(() => document.createElementNS(null, '#text')) === 'InvalidCharacterError');
        check('documents and doctypes have no text content', () => { document.textContent = 'x'; return document.textContent === null && document.body.childElementCount === 5 && document.implementation.createDocumentType('html', '', '').textContent === null; });
        const parsed = new DOMParser().parseFromString('<p>a</p>', 'text/html'), made = document.implementation.createHTMLDocument('t');
        parsed.documentElement.remove(); parsed.append(parsed.createElement('html')); made.insertBefore(made.createComment('c'), made.documentElement);
        check('other documents take children', () => parsed.documentElement.localName === 'html' && made.documentElement.previousSibling.nodeType === 8 && made.doctype.name === 'html' && error(() => made.append('text')) === 'HierarchyRequestError');
        const style = document.createElement('style'); style.textContent = 'div>.c{} a + b{} a~b{} a  b{} .p{& > .q{}}'; document.head.append(style);
        check('selectors serialize with spaced combinators', () => [...style.sheet.cssRules].map(rule => rule.selectorText).join() === 'div > .c,a + b,a ~ b,a b,.p' && style.sheet.cssRules[4].cssRules[0].selectorText === '& > .q');
        check('innerText of a select lists its options', () => $('list').innerText === 'One\nTwo');
        const sel = $('sel'), option = new Option('x', 'x', true, true);
        sel.append(option); check('adding an option revalidates its select', () => !sel.matches(':invalid'));
        option.remove(); check('removing an option revalidates its select', () => sel.matches(':invalid'));
        $('r2').checked = true; $('r2').name = 'b';
        check('renaming a radio revalidates its old group', () => $('r1').matches(':invalid') && $('r1').validity.valueMissing);
        const box = document.createElement('div'); box.style.marginLeft = '3px'; document.body.append(box);
        const animation = box.animate([{ marginLeft: '5px' }, { marginLeft: '5px' }], { duration: 1000, fill: 'both' }); animation.pause();
        const before = getComputedStyle(box).marginLeft; animation.effect.composite = 'add';
        check('a new composite reaches the engine', () => before === '5px' && getComputedStyle(box).marginLeft === '8px');
        "#,
    );
}

#[test]
fn freed_shadow_roots_drop_their_adopted_sheets() {
    let (_, engine) = checked_page(
        "<html><body></body></html>",
        r#"
        const sheet = new CSSStyleSheet(); sheet.replaceSync('span { color: rgb(255, 0, 0) }');
        (() => { const host = document.createElement('div'); document.body.append(host); const root = host.attachShadow({ mode: 'open' }); root.adoptedStyleSheets = [sheet]; root.innerHTML = '<span>x</span>'; globalThis.old = root._id; getComputedStyle(root.firstChild).color; host.remove(); })();
    "#,
    );
    let mut reused = String::new();
    for _ in 0..20 {
        engine.collect_garbage();
        engine.tick().unwrap();
        reused = engine.eval(r#"__appsDomSweep(true); (() => {
            const hosts = []; let root;
            for (let i = 0; i < 8 && root?._id !== old; i++) { const host = document.createElement('div'); document.body.append(host); hosts.push(host); root = host.attachShadow({ mode: 'open' }); }
            if (root._id !== old) { hosts.forEach(host => host.remove()); return ''; }
            root.innerHTML = '<span>y</span>';
            return root.adoptedStyleSheets.length + ' ' + getComputedStyle(root.firstChild).color;
        })()"#, "test").unwrap();
        if !reused.is_empty() {
            break;
        }
    }
    assert_eq!(reused, "0 rgb(0, 0, 0)");
}

#[test]
fn cssom_edits_keep_the_engine_and_detached_rules_apart() {
    let (_, engine) = css_page(
        "<html><head><style id=s>#a{color:red} @media all{#b{color:green}} @keyframes k{from{opacity:0}to{opacity:1}} @font-face{font-family:A;src:local(Arial)} @media not all{#c{color:blue}}</style></head><body><p id=a>a</p><p id=b>b</p><p id=c>c</p></body></html>",
    );
    engine.eval(r#"
        const style = document.getElementById('s'), sheet = style.sheet, color = id => getComputedStyle(document.getElementById(id)).color;
        // Reads the engine's rules back, which also detaches every rule object.
        const same = step => { const js = [...sheet.cssRules].map(rule => rule.cssText).join(' | '); sheet._text = null; const engine = [...sheet.cssRules].map(rule => rule.cssText).join(' | '); if (js !== engine) throw Error(step + ': ' + js + ' != ' + engine); };
        const media = sheet.cssRules[1]; sheet.deleteRule(1);
        media.deleteRule(0); media.insertRule('#a{color:pink}', 0); media.cssRules[0].style.color = 'lime'; media.cssRules[0].selectorText = '#b'; media.media.mediaText = 'not all';
        if (color('a') !== 'rgb(255, 0, 0)' || color('b') !== 'rgb(0, 0, 0)' || media.cssRules[0].cssText !== '#b { color: lime; }') throw Error('stale rule ' + color('a') + ' ' + media.cssRules[0].cssText);
        same('stale');
        if (sheet.insertRule('#b{color:navy}', 1) !== 1 || color('b') !== 'rgb(0, 0, 128)') throw Error('insert after stale');
        same('insert');
        const [, , keys, face] = sheet.cssRules;
        keys.cssRules[0].style.opacity = '0.5'; keys.cssRules[1].keyText = '50.0%'; keys.name = 'k2'; keys.appendRule('to{opacity:.75}');
        let bad; try { keys.cssRules[0].keyText = 'middle'; } catch (e) { bad = e.name; } if (bad !== 'SyntaxError') throw Error('keyText ' + bad);
        face.style.setProperty('font-family', 'B');
        if (keys.cssText !== '@keyframes k2 { \n  0% { opacity: 0.5; }\n  50% { opacity: 1; }\n  100% { opacity: 0.75; }\n}' || !face.cssText.includes('font-family: B')) throw Error('keyframes ' + keys.cssText + ' ' + face.cssText);
        same('keyframes');
        const hidden = sheet.cssRules[4];
        if (color('c') !== 'rgb(0, 0, 0)') throw Error('not all');
        hidden.media.appendMedium('all'); if (color('c') !== 'rgb(0, 0, 255)' || hidden.media.length !== 2) throw Error('appendMedium');
        hidden.media.deleteMedium('all'); if (color('c') !== 'rgb(0, 0, 0)') throw Error('deleteMedium');
        hidden.media.mediaText = 'all'; if (color('c') !== 'rgb(0, 0, 255)' || hidden.conditionText !== 'all') throw Error('mediaText');
        same('media');
        const first = sheet.cssRules[0]; first.selectorText = '###';
        if (first.selectorText !== '#a' || color('a') !== 'rgb(255, 0, 0)') throw Error('invalid selectorText');
        first.selectorText = 'p#a'; if (first.selectorText !== 'p#a') throw Error('selectorText');
        same('selector');
        const old = sheet.cssRules[0]; style.textContent = '#b{color:olive}'; old.selectorText = '#b'; old.style.color = 'red';
        if (color('b') !== 'rgb(128, 128, 0)' || sheet.cssRules.length !== 1) throw Error('text change');
        same('text');
        const page = new CSSStyleSheet({ media: 'not all' }); page.replaceSync('#a{color:teal}'); document.adoptedStyleSheets.push(page);
        if (color('a') === 'rgb(0, 128, 128)' || document.adoptedStyleSheets.length !== 1) throw Error('constructed media');
        page.media.mediaText = 'all'; if (color('a') !== 'rgb(0, 128, 128)') throw Error('constructed media change');
        let typed; try { document.adoptedStyleSheets.push({}); } catch (e) { typed = e.name; }
        if (typed !== 'TypeError' || document.adoptedStyleSheets.length !== 1) throw Error('adopted push ' + typed);
    "#, "test").unwrap();
}

#[test]
fn computed_appearance_follows_chrome() {
    checked_page(
        "<p>x</p>",
        r#"
        check('appearance follows chrome', () => {
          const kinds = ['text', 'hidden', 'image', 'file', 'checkbox', 'radio', 'range', 'button'].map(type => { const input = document.createElement('input'); input.type = type; return input; });
          const elements = [...kinds, ...['button', 'select', 'textarea', 'meter', 'progress', 'div', 'a', 'option', 'img'].map(tag => document.createElement(tag))];
          document.body.append(...elements);
          const values = elements.map(element => getComputedStyle(element).appearance).join();
          elements.forEach(element => element.remove());
          return values === 'auto,none,none,none,auto,auto,auto,auto,auto,auto,auto,auto,auto,none,none,none,none';
        });
        "#,
    );
}

#[test]
fn system_appearance_inputs_keep_their_types() {
    checked_page(
        "<style>input{appearance:system;padding:4px;border:2px solid red}</style><form id='form'><input id='switch' name='wifi' type='checkbox' switch><input id='range' type='range' value='250'><input id='text' value='a'><input id='date' type='date'><input id='button' type='button' value='Go'><input id='radio' type='radio'></form>",
        r#"
        const $ = id => document.getElementById(id);
        check('types', () => [...document.querySelectorAll('input')].map(input => input.type).join() === 'checkbox,range,text,date,button,radio');
        check('appearance', () => getComputedStyle($('text')).appearance === 'system');
        let events = '';
        for (const id of ['switch', 'range', 'text', 'button']) for (const type of ['input', 'change', 'click']) $(id).addEventListener(type, () => events += `${id}:${type} `);
        $('switch').click();
        check('switch toggles like a checkbox', () => $('switch').checked && $('switch').matches(':checked') && new FormData($('form')).get('wifi') === 'on');
        check('range sanitizes', () => $('range').value === '100');
        const mac = navigator.platform.startsWith('Mac');
        check('native boxes draw nothing on macOS', () => getComputedStyle($('text')).borderTopWidth === (mac ? '0px' : '2px') && getComputedStyle($('text')).paddingLeft === (mac ? '0px' : '4px'));
        check('other types keep their box', () => getComputedStyle($('radio')).borderTopWidth === '2px');
        events = '';
        __appsNativeControl($('text')._id, 'hello', false);
        __appsNativeControl($('text')._id, 'hello', true);
        __appsNativeControl($('range')._id, '33.3', false);
        __appsNativeControl($('range')._id, '33.3', true);
        __appsNativeControl($('switch')._id, null, true);
        __appsNativeControl($('button')._id, null, true);
        check('native edits run as user input', () => $('text').value === 'hello' && $('text').getAttribute('value') === 'a' && $('range').value === '33' && !$('switch').checked);
        check('native events', () => events === 'text:input text:change range:input range:change switch:click switch:input switch:change button:click ');
        "#,
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
            state.geometry.mirror.insert(id, rect);
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
        if (!(media instanceof CSSMediaRule) || media.conditionText !== '(min-width: 1px)' || media.cssRules[0].selectorText !== '.b' || media.cssRules[0].parentRule !== media) throw Error('media ' + media.conditionText);
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
            .geometry
            .mirror
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
        expect(before, 'content', '"x"'); expect(before, 'fontWeight', '700'); expect(before, 'color', 'rgba(255, 0, 0, 0.5)');
        if (getComputedStyle(a, '::after').content !== 'none') throw Error('missing ::after');
        a.style.transform = 'translate(10px, 5px)';
        expect(getComputedStyle(a), 'transform', 'matrix(1, 0, 0, 1, 10, 5)');
        expect(getComputedStyle(document.getElementById('h')), 'display', 'none');
        if (!s.length || !s.item(0)) throw Error('length');
    "#, "test").unwrap();
}

#[test]
fn hosts_take_pseudo_elements_from_their_shadow_tree() {
    checked_page(
        "<html><head><style>x-card::before{color:blue}</style></head><body><x-card id=card class=on></x-card></body></html>",
        r#"
        const card = document.getElementById('card');
        card.attachShadow({ mode: 'open' }).innerHTML = `<style>:host::before{content:"A";color:red;font-weight:700!important} :host(.on)::after{content:"B";color:green}</style>`;
        const before = getComputedStyle(card, '::before');
        check('content', () => before.content === '"A"');
        check('outer normal wins', () => before.color === 'rgb(0, 0, 255)');
        check('inner important wins', () => before.fontWeight === '700');
        check(':host(.on)::after', () => getComputedStyle(card, '::after').content === '"B"');
        card.classList.remove('on');
        check('class change', () => getComputedStyle(card, '::after').content === 'none');
        "#,
    );
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
        check('svg case', () => document.querySelector('linearGradient')?.id === 'g' && document.querySelector('lineargradient')?.id === 'g' && document.getElementsByTagName('linearGradient').length === 1);
        check('namespaces', () => document.querySelector('*|rect')?.id === 'r' && document.querySelector('|rect') === null);
        "#,
    );
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
        // As in Chrome, a set name keeps its place, unless a later name of its logical group maps the other way.
        const order = (text, change) => { a.setAttribute('style', text); change(style); return style.cssText + ' | ' + a.getAttribute('style'); };
        for (const [text, change, want] of [
          ['color: red; width: 1px; height: 2px', s => { s.width = '5px'; }, 'color: red; width: 5px; height: 2px;'],
          ['color: red; width: 1px; height: 2px', s => { s.width = '5px'; s.color = 'green'; s.top = '1px'; s.height = '7px'; }, 'color: green; width: 5px; height: 7px; top: 1px;'],
          ['color: red; width: 1px; height: 2px', s => s.setProperty('width', '9px', 'important'), 'color: red; width: 9px !important; height: 2px;'],
          // As in Chrome, important declarations serialize after the others.
          ['color: red; width: 1px !important; height: 2px', s => s.setProperty('width', '9px'), 'color: red; height: 2px; width: 9px;'],
          ['--a: 1; color: red; --b: 2', s => s.setProperty('--a', '5'), '--a: 5; color: red; --b: 2;'],
          ['color: red; margin: 1px; height: 2px', s => s.setProperty('margin', '4px'), 'color: red; margin: 4px; height: 2px;'],
          ['margin-top: 1px; color: red; height: 2px', s => s.setProperty('margin', '4px'), 'margin: 4px; color: red; height: 2px;'],
          ['color: red; margin: 1px; height: 2px', s => s.setProperty('margin-left', '4px', 'important'), 'color: red; margin-top: 1px; margin-right: 1px; margin-bottom: 1px; margin-left: 4px !important; height: 2px;'],
          ['margin-inline-start: 2px; margin-left: 1px; color: red', s => s.setProperty('margin-left', '3px'), 'margin-inline-start: 2px; margin-left: 3px; color: red;'],
          ['margin-left: 1px; margin-inline-start: 2px; color: red', s => s.setProperty('margin-left', '3px'), 'margin-inline-start: 2px; color: red; margin-left: 3px;'],
          ['corner-top-left-shape: round; corner-start-start-shape: bevel', s => s.setProperty('corner-top-left-shape', 'scoop'), 'corner-start-start-shape: bevel; corner-top-left-shape: scoop;'],
        ]) { const got = order(text, change); if (got !== want + ' | ' + want) throw Error(text + ' -> ' + got); }
        a.setAttribute('style', 'border: 1px solid black; border-image: linear-gradient(red, red) 1');
        style.borderImageSource = 'none';
        if (getComputedStyle(a).borderImageSource !== 'none') throw Error('a name twice in the list ' + a.getAttribute('style'));
    "#, "test").unwrap();
}

#[test]
fn web_animations_release_finished_effects_and_commit_their_own_stack() {
    let (_, engine) = css_page("<html><body><div id=a></div><div id=b></div></body></html>");
    engine.eval(r#"
        const [a, b] = ['a', 'b'].map(id => document.getElementById(id)), counts = { finish: 0, settled: 0 };
        const check = (got, want) => { if (JSON.stringify(got) !== JSON.stringify(want)) throw Error(JSON.stringify(got) + ' != ' + JSON.stringify(want)); };
        const opacity = () => getComputedStyle(a).opacity;
        const done = a.animate({ opacity: [0, 1] }, 100);
        done.onfinish = () => counts.finish++; done.finished.then(() => counts.settled++);
        done.finish();
        check([done.playState, a.getAnimations().length, opacity()], ['finished', 0, '1']);
        done.pause(); done.currentTime = 50;
        check([a.getAnimations().length, opacity()], [1, '0.5']);
        done.effect.updateTiming({ duration: 200 });
        check(opacity(), '0.25');
        done.effect.setKeyframes({ opacity: [1, 0] });
        check(opacity(), '0.75');
        done.finish(); check(a.getAnimations().length, 0);
        done.reverse(); check(a.getAnimations().length, 1);
        done.cancel(); done.play(); check(a.getAnimations().length, 1);
        done.cancel();
        const split = a.animate([{ offset: 0, opacity: 0 }, { offset: 0.5, opacity: 0.2 }, { offset: 0.5, opacity: 0.8 }, { offset: 1, opacity: 1 }], { duration: 1000, fill: 'both' });
        split.pause(); split.currentTime = 250; const low = opacity(); split.currentTime = 500;
        check([low, opacity()], ['0.1', '0.8']); split.cancel();
        const one = a.animate({ opacity: [0.25, 0.25] }, { duration: 1000, fill: 'forwards' }), two = a.animate({ opacity: [0.9, 0.9], width: ['30px', '30px'] }, { duration: 1000, fill: 'forwards' });
        const three = a.animate({ width: ['10px', '10px'] }, { duration: 1000, fill: 'forwards' });
        for (const animation of [one, two, three]) animation.pause();
        one.commitStyles(); three.commitStyles();
        check([a.style.opacity, a.style.width], ['0.25', '10px']);
        for (const animation of [one, two, three]) animation.cancel();
        const older = b.animate({ opacity: [0, 0.3] }, { duration: 2000, fill: 'forwards' }), newer = b.animate({ opacity: [0, 0.6] }, { duration: 1000, fill: 'forwards' });
        const kept = b.animate({ opacity: [0, 0.1] }, { duration: 3000, fill: 'forwards' }); kept.persist();
        newer.finish(); kept.finish(); older.finish();
        check([older.replaceState, newer.replaceState, kept.replaceState, b.getAnimations().length], ['removed', 'active', 'persisted', 2]);
        newer.cancel(); older.commitStyles();
        check(b.style.opacity, '0.3');
        const steps = new KeyframeEffect(null, { opacity: [0, 1] }, { duration: 100, delay: 100, fill: 'both', easing: 'steps(2, jump-start)' });
        new Animation(steps).currentTime = 50;
        check(steps.getComputedTiming().progress, 0);
        check(['steps(0)', 'cubic-bezier(2, 0, 0, 1)', 'linear(0)', 'steps(2)'].map(value => CSS.supports('transition-timing-function', value)), [false, false, false, true]);
        try { a.animate({ opacity: [0, 1] }, { easing: 'cubic-bezier(2,0,0,1)' }); throw Error('accepted'); } catch (error) { if (!(error instanceof TypeError)) throw error; }
    "#, "test").unwrap();
    let started = std::time::Instant::now();
    while engine.eval("counts.finish", "test").unwrap() == "0" {
        let delay = engine.tick().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(delay.clamp(1, 10) as u64));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
    for _ in 0..3 {
        engine.tick().unwrap();
    }
    assert_eq!(
        engine.eval("JSON.stringify(counts)", "test").unwrap(),
        r#"{"finish":2,"settled":1}"#
    );
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

#[test]
fn bundled_link_sheets_are_not_text() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: apps_runtime::bundle::page(
                "<html><head><link rel='stylesheet' href='a.css' apps-sheet='p { color: red }'></head><body><p>x</p></body></html>",
            ),
            dirty: false,
        },
    );
    let engine = JsContext::new(JavaScriptCore::load().unwrap(), host, Role::Renderer(1)).unwrap();
    assert_eq!(
        engine
            .eval("[document.head.innerHTML, document.head.textContent, document.documentElement.textContent, document.querySelector('link').sheet?.cssRules.length].join('|')", "test")
            .unwrap(),
        "<link rel=\"stylesheet\" href=\"a.css\">||x|1"
    );
}

#[test]
fn web_animation_times_must_be_finite_and_may_be_huge() {
    checked_page(
        "<html><body><div id='a' style='opacity:0.5'></div></body></html>",
        r#"
        const a = document.getElementById('a');
        const animation = a.animate({ opacity: [0, 1] }, { duration: 1000, delay: 1e300 });
        const rejects = (set) => { try { set(); return false; } catch (error) { return error instanceof TypeError; } };
        check('NaN time', () => rejects(() => { animation.currentTime = NaN; }));
        check('infinite time', () => rejects(() => { animation.currentTime = Infinity; }));
        check('infinite start', () => rejects(() => { animation.startTime = -Infinity; }));
        check('NaN rate', () => rejects(() => { animation.playbackRate = NaN; }));
        check('infinite pending rate', () => rejects(() => animation.updatePlaybackRate(Infinity)));
        check('huge time', () => { animation.currentTime = 1e300; animation.pause(); animation.currentTime = -1e300; return getComputedStyle(a).opacity === '0.5'; });
        check('huge start', () => { animation.startTime = 1e300; animation.play(); return getComputedStyle(a).opacity === '0.5'; });
        "#,
    );
}

#[test]
fn declared_values_serialize_like_chrome() {
    checked_page(
        "<html><head><style id=s>@media (min-width: 100px) { .m { color: blue } } .n { color: red; &:hover { color: blue } } @font-face { font-family: \"X\"; src: url(x.woff2) }</style></head><body><div id=d></div></body></html>",
        r#"
        const d = document.getElementById('d'), rules = document.getElementById('s').sheet.cssRules;
        for (const [text, expected] of [
            ['color:RED', 'color: red;'], ['color:#FFF', 'color: rgb(255, 255, 255);'], ['color:rgba(1,2,3,.5)', 'color: rgba(1, 2, 3, 0.5);'],
            ['width:calc(1px + 2px)', 'width: calc(3px);'], ['width:0', 'width: 0px;'], ['width:+.5E1px', 'width: 5px;'], ['line-height:1.50', 'line-height: 1.5;'],
            ['transform:translateX( 1PX ) ROTATE(1turn)', 'transform: translateX(1px) rotate(1turn);'], ['transform:translate3d(1px, 2px, 0)', 'transform: translate3d(1px, 2px, 0px);'],
            ['font-family:"Inter",  Some  Font, serif', 'font-family: Inter, "Some Font", serif;'], ['box-shadow:0 0 0 1px RED inset', 'box-shadow: red 0px 0px 0px 1px inset;'],
            ['transition:all 0s ease 0s', 'transition: all;'], ['animation:x 1s infinite', 'animation: 1s ease 0s infinite normal none running x;'],
            ['border-radius:1px 1px 1px 1px / 2px', 'border-radius: 1px / 2px;'], ['outline:2px solid transparent', 'outline: transparent solid 2px;'],
            ['background-position:left', 'background-position: left center;'], ['opacity:50%', 'opacity: 0.5;'], ['aspect-ratio:16/9', 'aspect-ratio: 16 / 9;'],
            ['mask-image:linear-gradient(to bottom, black, transparent)', 'mask-image: linear-gradient(black, transparent);'], ['width:var( --a , 1px )', 'width: var( --a , 1px );'],
        ]) check(text, () => { d.style.cssText = text; return d.style.cssText === expected; });
        d.style.cssText = ''; d.style.transform = 'translateX(10PX)';
        check('a longhand reads back normalized', () => d.style.transform === 'translateX(10px)');
        check('media rule text', () => rules[0].cssText === '@media (min-width: 100px) {\n  .m { color: blue; }\n}' && rules[0].conditionText === '(min-width: 100px)');
        check('nested rule text', () => rules[1].cssText === '.n {\n  color: red;\n  &:hover { color: blue; }\n}');
        check('font face text', () => rules[2].cssText === '@font-face { font-family: X; src: url("x.woff2"); }');
        "#,
    );
}

#[test]
fn named_style_queries_and_values_like_chrome() {
    checked_page(
        "<html><head><style>.box { background-color: rgb(200, 200, 200); } @container card style(--k: 1) { .named { background-color: rgb(0, 0, 255); } } @container card style(--k: 2) { .named { color: rgb(0, 128, 0); } } @container side style(--k: 1) { .named { outline-color: rgb(1, 2, 3); } } @container card (min-width: 10px) { .named { border-top-color: rgb(9, 9, 9); } } @container card style(--k) { .named::before { content: 'p'; color: rgb(0, 0, 255); } } .card { container-name: card; --k: 1; } .side { container: side / inline-size; --k: 1; } .mid { --k: 2; } .p { color: rgb(255, 0, 0); border: 2px solid; text-decoration: underline; } .c { color: rgb(0, 0, 255); border-color: inherit; text-decoration-color: inherit; background-color: inherit; } .p2 { color: rgb(255, 0, 0); border-color: rgb(255, 0, 0); } body { counter-reset: n; } .n::before { counter-increment: n; content: counter(n) '.'; } @media (200px <= width < 3000px) { .m { order: 1; } } @media (1/2 < aspect-ratio) { .m { order: 2; } }</style></head><body><div class=card id=c1><div><div class='box named' id=a></div></div></div><div class=card id=c2><div class=mid><div class='box named' id=b></div></div></div><div class=side id=s><div class=card id=c3 style='--k: 2'><div class='box named' id=c></div></div></div><div id=nc><div class='box named' id=d></div></div><div class=p><div class=c id=e></div></div><div class=p2><div class=c id=f></div></div><p class=n id=n>x</p><div id=x></div></body></html>",
        r#"
        const value = (id, name, pseudo) => getComputedStyle(document.getElementById(id), pseudo).getPropertyValue(name);
        const node = id => document.getElementById(id);
        const snap = () => ['a', 'b', 'c', 'd'].map(id => [value(id, 'background-color'), value(id, 'color'), value(id, 'outline-color'), value(id, 'border-top-color')].join(' ')).join('|');
        const gray = 'rgb(200, 200, 200)', blue = 'rgb(0, 0, 255)', green = 'rgb(0, 128, 0)', black = 'rgb(0, 0, 0)';
        check('named style queries read the nearest container of the name', () => snap() === [`${blue} ${black} ${black} ${black}`, `${blue} ${black} ${black} ${black}`, `${gray} ${green} rgb(1, 2, 3) ${green}`, `${gray} ${black} ${black} ${black}`].join('|') && value('b', 'content', '::before') === '"p"');
        node('c1').style.setProperty('--k', '2'); node('c3').style.setProperty('--k', '1'); node('nc').className = 'card';
        check('named style queries follow changes of the container', () => snap() === [`${gray} ${green} ${green} ${green}`, `${blue} ${black} ${black} ${black}`, `${blue} ${black} rgb(1, 2, 3) ${black}`, `${blue} ${black} ${black} ${black}`].join('|'));
        node('c2').style.setProperty('container-name', 'other'); node('c3').style.setProperty('container-name', 'x'); node('s').style.setProperty('--k', '7');
        check('named style queries follow container names', () => value('b', 'background-color') === gray && value('b', 'content', '::before') === 'none' && value('c', 'background-color') === gray && value('c', 'outline-color') === black);
        check('inherit of a currentcolor value uses the own color', () => ['border-top-color', 'text-decoration-color', 'background-color'].map(name => value('e', name)).join() === `${blue},${blue},rgba(0, 0, 0, 0)` && value('f', 'border-top-color') === 'rgb(255, 0, 0)');
        node('e').style.color = 'rgb(9, 9, 9)';
        check('inherited currentcolor follows a new color', () => value('e', 'border-top-color') === 'rgb(9, 9, 9)');
        node('f').parentNode.style.borderColor = 'currentcolor';
        check('a parent that turns to currentcolor', () => value('f', 'border-top-color') === blue);
        node('f').parentNode.style.borderColor = '';
        check('a parent that turns back to its color', () => value('f', 'border-top-color') === 'rgb(255, 0, 0)');
        check('counters keep their declared form', () => value('n', 'content', '::before') === 'counter(n) "."');
        check('range media text', () => [...document.styleSheets[0].cssRules].filter(rule => rule.media).map(rule => rule.conditionText).join() === '(200px <= width < 3000px),(1 / 2 < aspect-ratio)');
        const x = node('x');
        const specified = (name, text) => { x.style.cssText = ''; x.style.setProperty(name, text); return x.style.getPropertyValue(name); };
        const computed = (name, text) => { x.style.cssText = ''; x.style.setProperty(name, text); return getComputedStyle(x).getPropertyValue(name); };
        check('calc sums', () => ['calc(10px + 2em)', 'calc(50% / 2)', 'calc(10px + 5% + 1em + 2px)', 'calc(2 * (3em + 1vw))', 'calc(-1 * 10px + 1em)', 'calc(1q + 1em)', 'calc(100% - (2em + 10px))', 'calc(1e-3px + 1em)'].map(text => specified('width', text)).join() === 'calc(2em + 10px),calc(25%),calc(5% + 1em + 12px),calc(6em + 2vw),calc(1em - 10px),calc(1em + 0.944882px),calc(100% - 2em - 10px),calc(1em + 0.001px)');
        check('flex-flow', () => ['row wrap', 'column nowrap', 'row nowrap'].map(text => specified('flex-flow', text) + '/' + computed('flex-flow', text)).join() === 'wrap/row wrap,column/column nowrap,row/row nowrap');
        check('list-style', () => ['square inside', 'url(a.png)', 'inside none'].map(text => specified('list-style', text)).join() === 'inside square,url("a.png"),inside none' && computed('list-style', 'inside none') === 'inside none none');
        check('grid', () => specified('grid', 'auto-flow / 1fr 1fr') === 'none / 1fr 1fr' && specified('grid-auto-flow', 'row dense') === 'dense' && computed('grid-auto-flow', 'dense column') === 'column dense');
        check('font stretch keywords', () => computed('font', 'condensed 12px Helvetica') === 'condensed 12px Helvetica' && (x.style.cssText = 'font: 10px serif; font-stretch: 80%', getComputedStyle(x).font === ''));
        check('text-emphasis', () => computed('text-emphasis', 'filled red') === 'dot rgb(255, 0, 0)' && computed('text-emphasis', 'open') === 'open dot rgb(0, 0, 0)');
        check('position values', () => computed('object-position', 'right 10px bottom') === '50% 50%' && specified('perspective-origin', 'left 10px top') === '');
        check('background layers', () => (x.style.cssText = 'background-image: url(a.png); background-position: 1px 2px, 3px 4px; background-size: 1px, 2px', [getComputedStyle(x).backgroundPosition, getComputedStyle(x).backgroundSize].join() === '1px 2px,1px auto'));
        check('offsets of static boxes', () => (x.style.cssText = 'left: calc(10% + 2px); top: 10%', [getComputedStyle(x).left, getComputedStyle(x).top].join() === 'calc(10% + 2px),10%'));
        "#,
    );
}

#[test]
fn css_values_and_cascade_like_chrome() {
    checked_page(
        "<html><head><style>@scope (.card) { :scope { color: rgb(1, 0, 0); } } div { color: rgb(2, 0, 0); } @scope (.r5) { :where(:scope) { color: rgb(255, 0, 0); } & { color: rgb(0, 128, 0); } } @scope (.r3) { :not(:scope) { background-color: rgb(1, 1, 1); } :is(:scope) i { color: rgb(3, 3, 3); } } @container card style(--theme: dark) { .q { outline-color: rgb(9, 9, 9); } } .dark { --theme: dark; } @container style(--theme: dark) { .q { color: rgb(3, 0, 0); } } @container not style(--theme) { .q { background-color: rgb(4, 0, 0); } } .b { margin: 1px !important; margin-top: 5px; } .ctr { counter-reset: n m 3; counter-increment: n; } .nest { counter-reset: s; } p::before { content: normal; } p::first-letter { color: rgb(5, 6, 7); } input::placeholder { font-size: 12px; } li::marker { color: rgb(8, 9, 10); }</style></head><body><div class=card id=c></div><p class=r5 id=r5></p><div class=r3 id=r3><i id=r3i></i></div><div class=dark><div class=q id=q1></div></div><div><div class=q id=q2></div></div><div class=b id=b></div><div class=ctr id=ctr></div><p id=p>p</p><input id=in placeholder=x><ol class=nest><li id=l1><ol class=nest><li id=l2></li></ol></li></ol><div id=d></div></body></html>",
        r#"
        const value = (id, name, pseudo) => getComputedStyle(document.getElementById(id), pseudo).getPropertyValue(name);
        const attempt = fn => { try { return fn(); } catch (error) { return error.name; } };
        const d = document.getElementById('d'), set = text => { d.style.cssText = text; return getComputedStyle(d); };
        check(':scope in @scope has the specificity of a class', () => value('c', 'color') === 'rgb(1, 0, 0)');
        check(':not(:scope) stays in the scope', () => value('c', 'background-color') === 'rgba(0, 0, 0, 0)' && value('r3i', 'background-color') === 'rgb(1, 1, 1)' && value('r3i', 'color') === 'rgb(3, 3, 3)');
        check(':scope in :where() adds nothing', () => value('r5', 'color') === 'rgb(0, 128, 0)');
        check('named style queries never match', () => value('q1', 'outline-color') !== 'rgb(9, 9, 9)');
        check('style() container queries', () => value('q1', 'color') === 'rgb(3, 0, 0)' && value('q2', 'color') === 'rgb(2, 0, 0)' && value('q2', 'background-color') === 'rgb(4, 0, 0)' && value('q1', 'background-color') === 'rgba(0, 0, 0, 0)');
        check('important longhand of a rule wins', () => value('b', 'margin-top') === '1px' && document.styleSheets[0].cssRules[8].style.cssText === 'margin: 1px !important;');
        check('important inline longhand wins', () => { d.style.cssText = 'margin: 1px !important; margin-top: 5px; color: red !important; color: blue'; return d.style.cssText === 'margin: 1px !important; color: red !important;' && d.style.getPropertyPriority('margin-top') === 'important'; });
        check('important declarations go last', () => { d.style.cssText = 'color:red !important;width:1px'; return d.style.cssText === 'width: 1px; color: red !important;'; });
        check('internal pseudo-classes are not for pages', () => !CSS.supports('selector(:-internal-scope)') && attempt(() => document.querySelector('div:-internal-scope')) === 'SyntaxError');
        check('rotation axes', () => set('rotate: z 45deg').rotate === '45deg' && set('rotate: 0 0 -1 45deg').rotate === '-45deg');
        check('translate and inset sides', () => { d.style.cssText = 'translate: 0 251px'; const one = d.style.translate; d.style.translate = '0px 0px'; return one === '0px 251px' && d.style.translate === '0px' && set('clip-path: inset(0 calc(100% - 30%) 0 0)').clipPath === 'inset(0px 70% 0px 0px)' && set('clip-path: inset(0 calc(100% - 60px) 0 0)').clipPath === 'inset(0px calc(100% - 60px) 0px 0px)' && set('clip-path: inset(0 round 10% 2em)').clipPath === 'inset(0px round 10% 32px)' && set('clip-path: inset(1px 2px round 1em / 5px)').clipPath === 'inset(1px 2px round 16px / 5px)'; });
        check('a repeated declaration moves to its last place', () => { d.style.cssText = 'color:red;width:1px;color:blue'; return d.style.cssText === 'width: 1px; color: blue;'; });
        check('declarations iterate', () => { d.style.cssText = 'color: red; width: 1px'; return [...d.style].join() === 'color,width' && [...getComputedStyle(d)].includes('color'); });
        check('webkit aliases', () => { d.style.cssText = '-webkit-transform: scale(2); -webkit-box-shadow: 1px 1px red'; return d.style.transform === 'scale(2)' && d.style.webkitTransform === 'scale(2)' && getComputedStyle(d).boxShadow === 'rgb(255, 0, 0) 1px 1px 0px 0px'; });
        check('opacity clamps', () => set('opacity: 2').opacity === '1' && set('opacity: -1').opacity === '0' && set('opacity: calc(50% + 10%)').opacity === '0.6');
        check('aspect ratios', () => set('aspect-ratio: 16/9').aspectRatio === '16 / 9' && set('aspect-ratio: 2').aspectRatio === '2 / 1' && set('aspect-ratio: auto 1/2').aspectRatio === 'auto 1 / 2');
        check('spacing', () => set('letter-spacing: 0').letterSpacing === 'normal' && set('word-spacing: normal').wordSpacing === '0px');
        check('counters', () => value('ctr', 'counter-reset') === 'n 0 m 3' && value('ctr', 'counter-increment') === 'n 1');
        check('scroll margins and column rules', () => { const s = set('scroll-margin: 5px 1em; scroll-padding: 2px; column-rule: 1px solid red; container: a / size'); return s.scrollMargin === '5px 16px' && s.scrollPaddingTop === '2px' && s.columnRule === '1px solid rgb(255, 0, 0)' && s.container === 'a / size'; });
        check('logical border sides', () => set('border-inline-start: 2px dashed red').borderInlineStart === '2px dashed rgb(255, 0, 0)');
        check('background positions with offsets', () => set('background-position: right 10px bottom').backgroundPosition === 'calc(100% - 10px) 100%' && set('background-position: left 5px top 20%').backgroundPosition === '5px 20%' && set('background-position: right 25% center').backgroundPosition === '75% 50%');
        check('text fill color', () => set('color: red').webkitTextFillColor === 'rgb(255, 0, 0)');
        check('content normal on ::before', () => value('p', 'content', '::before') === 'none');
        check('individual transforms and zoom', () => { const s = set('rotate: calc(12deg * -1); translate: 0.25rem 0; scale: 150% -1; zoom: 50%'); return [s.rotate, s.translate, s.scale, s.zoom].join() === '-12deg,4px,1.5 -1,0.5'; });
        check('grid tracks in pixels', () => { const s = set('grid-auto-columns: calc(0.25rem * 3); grid-template-columns: repeat(2, minmax(0, 1em)) 1fr 10%'); return s.gridAutoColumns === '12px' && s.gridTemplateColumns === 'repeat(2, minmax(0px, 16px)) 1fr 10%'; });
        check('empty references leave no spaces', () => set('--x: ; filter: var(--x,) blur(2px) var(--x,)').filter === 'blur(2px)');
        check('more computed values', () => { const s = set('letter-spacing: 1%; rotate: 1 0 0 90deg; scale: 1 1 1; translate: calc(10% + 1em); zoom: normal; background-position: right calc(10% + 2px) bottom; min-width: calc(10% + 1em); grid-area: -a / 1'); return [s.letterSpacing, s.rotate, s.scale, s.translate, s.zoom, s.backgroundPosition, s.minWidth, s.gridArea].join() === '1%,x 90deg,1,calc(10% + 16px),1,calc(90% - 2px) 100%,calc(10% + 16px),-a / 1'; });
        check('webkit aliases in supports and masks', () => CSS.supports('-webkit-transform', 'none') && CSS.supports('(-webkit-columns: 2)') && (d.style.cssText = '-webkit-mask-image: none; -webkit-filter: blur(1px)', d.style.cssText === 'mask-image: none; filter: blur(1px);') && d.style.getPropertyValue('-webkit-mask-image') === 'none');
        check('text pseudo-elements', () => value('p', 'color', '::first-letter') === 'rgb(5, 6, 7)' && value('in', 'font-size', '::placeholder') === '12px' && value('l1', 'color', '::marker') === 'rgb(8, 9, 10)');
        for (const [text, expected] of [
            ['width: round(23px, 10px)', 'width: calc(20px);'], ['transition-duration: calc(1s + 500ms)', 'transition-duration: calc(1.5s);'],
            ['transform: rotate(calc(45deg + 0.25turn))', 'transform: rotate(calc(135deg));'], ['z-index: clamp(0, 2, 1)', 'z-index: calc(1);'],
            ['width: calc(min(10px, 20px) + max(1em, 2px))', 'width: calc(10px + max(1em, 2px));'], ['width: calc(1rem + 2px)', 'width: calc(2px + 1rem);'], ['border-radius: calc(1px + 2%) / calc(3px + 1em)', 'border-radius: calc(2% + 1px) / calc(1em + 3px);'], ['--x: calc( 2px+1em )', '--x: calc( 2px+1em );'], ['height: -webkit-calc(1px + 2px)', 'height: calc(3px);'], ['width: calc(min(10px, 20px) + 5px)', 'width: calc(15px);'], ['width: calc(min(10px, 1em) * 1 + 2px)', 'width: calc(2px + min(10px, 1em));'],
        ]) check(text, () => { d.style.cssText = text; return d.style.cssText === expected; });
        "#,
    );
}

#[test]
fn computed_styles_report_used_values_like_chrome() {
    let (host, engine) = css_page(
        "<html><head><style>#w{position:relative;width:200px;height:100px;padding:5px;border:2px solid} #a{position:absolute;width:50%;left:10%;top:4px;padding:2px 3%} #r{position:relative;top:10px;left:5px} #m{width:100px;margin:0 auto} #h{display:none;width:50%;transform-origin:center} #p::before{content:'x' attr(data-v) \"y\"} .o{width:100%;padding:28px;margin:0 auto} #v{direction:rtl;width:200px} #w::before{content:'';display:block;width:max(10%,3px);height:4px;margin-left:5%} #w::after{content:'';display:block;width:calc(10% + 1px);min-width:20%} #m::before{content:'';display:block;padding-left:5%} #r::before{content:'x';display:none;width:50%} #r::after{content:'';position:absolute;width:10px;height:10px}</style></head><body><div id=w><div id=a></div><div id=r></div><div id=m></div><div id=o class=o></div></div><div id=h></div><p id=p data-v=t></p><div id=v><div id=q class=o></div></div></body></html>",
    );
    let id = |name: &str| -> usize {
        engine
            .eval(&format!("document.getElementById('{name}')._id"), "id")
            .unwrap()
            .parse()
            .unwrap()
    };
    let ids = ["w", "a", "r", "m", "p", "o", "v", "q"].map(id);
    {
        let mut host = host.borrow_mut();
        let state = host.dom.entry(1).or_default();
        state.geometry.mirror.extend(ids.into_iter().zip([
            [8., 8., 214., 114.],
            [31., 14., 117.6, 15.5],
            [20., 25., 200., 0.],
            [65., 15., 100., 0.],
            [8., 130., 784., 16.],
            [15., 15., 256., 56.],
            [8., 150., 200., 56.],
            [8., 150., 256., 56.],
        ]));
        // The boxes that layout gave the pseudo-elements, by node | pseudo << 30: the place from the border box of
        // the element, the size, then the padding plus border of each side.
        state.layout.pseudo_boxes.extend([
            (ids[0] | 1 << 30, [17., 7., 20., 4., 0., 0., 0., 0.]),
            (ids[0] | 2 << 30, [7., 11., 40., 0., 0., 0., 0., 0.]),
            (ids[3] | 1 << 30, [0., 0., 100., 0., 0., 0., 0., 10.]),
            (ids[2] | 1 << 30, [0., 0., 200., 18., 0., 0., 0., 0.]),
            (ids[2] | 2 << 30, [0., 0., 10., 10., 0., 0., 0., 0.]),
        ]);
    }
    engine.eval(r#"
        const value = (id, name, pseudo) => getComputedStyle(document.getElementById(id), pseudo).getPropertyValue(name);
        for (const [id, name, expected, pseudo] of [
            ['a', 'width', '105px'], ['a', 'inline-size', '105px'], ['a', 'padding-left', '6.29688px'], ['a', 'left', '21px'], ['a', 'inset', '4px 71.4062px 90.5px 21px'],
            ['r', 'right', '-5px'], ['r', 'bottom', '-10px'], ['m', 'margin', '0px 50px'], ['m', 'margin-left', '50px'],
            ['h', 'width', '50%'], ['h', 'transform-origin', '50% 50%'],
            ['p', 'content', '"xty"', '::before'], ['p', 'content', 'none', '::after'],
            ['o', 'margin', '0px -56px 0px 0px'], ['q', 'margin', '0px 0px 0px -56px'],
            ['w', 'width', '20px', '::before'], ['w', 'margin-left', '10px', '::before'], ['w', 'height', '4px', '::before'],
            ['w', 'width', '40px', '::after'], ['w', 'height', '0px', '::after'], ['m', 'width', '90px', '::before'], ['m', 'padding-left', '10px', '::before'],
            ['r', 'inset', '0px 190px -10px 0px', '::after'],
            ['r', 'width', '50%', '::before'],
        ]) if (value(id, name, pseudo) !== expected) throw Error(`${id} ${name}: ${value(id, name, pseudo)} != ${expected}`);
    "#, "test").unwrap();
}

#[test]
fn target_and_range_pseudo_classes_like_chrome() {
    checked_page(
        "<html><head><style>:target{color:red}</style></head><body><p id='a'>a</p><p id='b'>b</p><input id='n' type='number' min='1' max='5' value='3'><input id='d' type='date' min='2020-01-01' value='2019-12-31'><input id='r' type='range'><input id='x' type='number' value='9'></body></html>",
        r#"
        const target = () => document.querySelector(':target')?.id ?? null, color = id => getComputedStyle(document.getElementById(id)).color;
        location.hash = 'a';
        check('hash sets the target', () => target() === 'a' && color('a') === 'rgb(255, 0, 0)');
        history.pushState(null, '', '#x');
        check('pushState keeps it', () => target() === 'a');
        location.hash = 'b';
        check('a new hash moves it', () => target() === 'b' && color('a') === 'rgb(0, 0, 0)' && color('b') === 'rgb(255, 0, 0)');
        const ids = s => [...document.querySelectorAll(s)].map(e => e.id).join(' ');
        check('ranges', () => ids(':in-range') === 'n r' && ids(':out-of-range') === 'd');
        const n = document.getElementById('n'); n.value = '7';
        check('typed values', () => ids(':out-of-range') === 'n d' && n.matches(':out-of-range'));
    "#,
    );
}

#[test]
fn border_images_parse_and_serialize_like_chrome() {
    checked_page(
        "<html><head><style>#r{border:10px solid red;border-image:linear-gradient(red, blue) 30% fill / 2 / 4px round space} @supports (border-image: linear-gradient(red, blue) 1) { #r { color: green } }</style></head><body><div id=r></div><div id=d></div></body></html>",
        r#"
        const d = document.getElementById('d'), r = document.getElementById('r');
        for (const [text, expected] of [
            ['border-image:linear-gradient(red, blue) 1', 'border-image: linear-gradient(red, blue) 1 / 1 / 0 stretch;'],
            ['border-image:url(a.png) fill 10', 'border-image: url("a.png") 10 fill / 1 / 0 stretch;'],
            ['border-image:url(a.png) 30 / / 2', 'border-image: url("a.png") 30 / 1 / 2 stretch;'],
            ['border-image:url(a.png) 30 / auto 2 1px 10%', 'border-image: url("a.png") 30 / auto 2 1px 10% / 0 stretch;'],
            ['border-image:#f00 1', ''], ['border-image:none', 'border-image: none;'], ['border-image:30', 'border-image: none 30 / 1 / 0 stretch;'],
            ['border-image:linear-gradient(#f00, #00f) 1 1 1 1 / 1 / 0 stretch stretch', 'border-image: linear-gradient(rgb(255, 0, 0), rgb(0, 0, 255)) 1 / 1 / 0 stretch;'],
            ['border-image-repeat:round round', 'border-image-repeat: round;'], ['border:1px solid red', 'border: 1px solid red;'],
            ['border-image:linear-gradient(red, blue) 1; border:2px solid', 'border: 2px solid;'],
            ['border:2px solid; border-image:linear-gradient(red, blue) 1', 'border-width: 2px; border-style: solid; border-color: currentcolor; border-image: linear-gradient(red, blue) 1 / 1 / 0 stretch;'],
        ]) check(text, () => { d.style.cssText = text; return d.style.cssText === expected; });
        const s = getComputedStyle(r);
        check('computed', () => [s.borderImage, s.borderImageSlice, s.borderImageWidth, s.borderImageOutset, s.borderImageRepeat].join(' | ') === 'linear-gradient(red, blue) 30% fill / 2 / 4px round space | 30% fill | 2 | 4px | round space');
        check('initial', () => { const s = getComputedStyle(d); d.style.cssText = ''; return [s.borderImage, s.borderImageSlice, s.borderImageWidth, s.borderImageOutset, s.borderImageRepeat].join() === 'none,100%,1,0,stretch'; });
        check('@supports', () => s.color === 'rgb(0, 128, 0)');
        check('CSS.supports', () => ['border-image', 'linear-gradient(red, blue) 1', 'border-image', 'url(a.png) 30 / 10px / 5px stretch repeat', 'border-image', 'red', 'border-image', '1 2 3 4 5', 'border-image-slice', '30 fill', 'border-image-slice', 'fill', 'border-image-slice', '-1', 'border-image-slice', '1px', 'border-image-repeat', 'round space', 'border-image-repeat', 'round space stretch', 'border-image-width', 'auto 1px 10% 2', 'border-image-width', '-1px', 'border-image-outset', '1 2px', 'border-image-outset', '10%', 'border-image-source', 'none', 'border-image-source', 'linear-gradient(red, blue)', 'border-image-source', 'red'].reduce((out, v, i, a) => i % 2 ? out : out + (CSS.supports(v, a[i + 1]) ? '1' : '0'), '') === '11001000101010110');
        "#,
    );
}

#[test]
fn text_stroke_values_like_chrome() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><head><style>body{font-size:16px}#a{-webkit-text-stroke:2px red}#b{-webkit-text-stroke:thin;color:blue}#c{-webkit-text-stroke:green 3px}#d{-webkit-text-stroke:-1px red}#e{-webkit-text-stroke-width:0.1em;font-size:20px}</style></head><body><div id=a><span id=a2>x</span></div><div id=b></div><div id=c></div><div id=d></div><div id=e></div></body></html>"),
        dirty: false,
    });
    let engine = JsContext::new(JavaScriptCore::load().unwrap(), host, Role::Renderer(1)).unwrap();
    engine.eval(r#"
        const got = ['a', 'a2', 'b', 'c', 'd', 'e'].map(id => getComputedStyle(document.getElementById(id)).getPropertyValue('-webkit-text-stroke')).join('|');
        if (got !== '2px rgb(255, 0, 0)|2px rgb(255, 0, 0)|0.666667px rgb(0, 0, 255)|3px rgb(0, 128, 0)|0px rgb(0, 0, 0)|2px rgb(0, 0, 0)') throw Error(got);
        if (!CSS.supports('-webkit-text-stroke', '1px red') || CSS.supports('-webkit-text-stroke-width', '-1px')) throw Error('supports');
        if (CSS.supports('color', '-webkit-focus-ring-color') || !CSS.supports('outline', 'auto -webkit-focus-ring-color')) throw Error('focus ring color');
        document.body.style.outlineColor = '-webkit-focus-ring-color';
        if (getComputedStyle(document.body).outlineColor !== 'rgb(0, 95, 204)') throw Error('focus ring color value');
    "#, "test").unwrap();
}
