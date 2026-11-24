use super::*;
use crate::css;
use crate::css::properties::Prop;
use crate::dom::Node;
use crate::{host::Host, host::context::JsContext, jsc::JavaScriptCore, jsc::Role, render::Page};
use std::{cell::RefCell, rc::Rc};

fn engine(html: &str) -> (Rc<RefCell<Host>>, JsContext) {
    let host = Rc::new(RefCell::new(Host::default()));
    let page = Page {
        doc: Document::parse(html),
        dirty: false,
        ..Default::default()
    };
    host.borrow().store.borrow_mut().pages.insert(1, page);
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    (host, engine)
}

#[test]
fn kept_containers_keep_their_sizes_between_frames() {
    let mut state = crate::render::PageState::default();
    state.layout.set_container(1, [150., 20.]);
    state.update_containers(&Document::default());
    assert_eq!(state.layout.container_size(1), Some(&[150., 20.]));
}

#[test]
fn font_faces_load_woff_and_list_css_faces() {
    let fixtures = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fonts"));
    let (host, engine) = engine(
        r#"<style>@font-face{font-family:Css;src:url(test.woff) format("woff")}
                @font-face{font-family:Later;src:url(later.woff);unicode-range:U+0400-04FF}</style><p>AB</p>"#,
    );
    host.borrow_mut().update_app(|app| {
        std::sync::Arc::make_mut(&mut app.assets)
            .insert("test.woff".into(), fixtures.join("test.woff"))
    });
    {
        let host = host.borrow().store.clone();
        let mut host = host.borrow_mut();
        let state = host.state(1);
        state.fonts.system = Some(Rc::new(renderer::TextSystem::new()));
        state.base = url::Url::parse("apps:///index.html").ok();
    }
    let bytes = std::fs::read(fixtures.join("test-cff.woff2")).unwrap();
    let bytes: Vec<_> = bytes.iter().map(u8::to_string).collect();
    engine
        .eval(
            &format!(
                r#"
        globalThis.log = [];
        const face = new FontFace('Bytes', new Uint8Array([{}]));
        document.fonts.add(face);
        face.load().then(() => log.push('bytes ' + face.status));
        new FontFace('Bad', 'url(a.eot) format("embedded-opentype")').load().catch(error => log.push('bad ' + error.name));
        new FontFace('Local', 'local("No Such Font 123")').load().catch(error => log.push('local ' + error.name));
        const junk = new FontFace('Junk', new Uint8Array([1, 2, 3, 4])); junk.loaded.catch(error => log.push('junk ' + (error instanceof DOMException) + ' ' + error.name + ' ' + junk.status));
        document.fonts.ready.then(fonts => {{
            log.push([...fonts].map(face => face.family + ' ' + face.status).join());
            const context = document.createElement('canvas').getContext('2d');
            for (const family of ['Bytes', 'Css']) {{ context.font = '10px ' + family; log.push(context.measureText('AB').width); }}
            log.push(document.fonts.check('10px Css'), document.fonts.check('10px Later'), document.fonts.check('10px Later', 'Ж'), document.fonts.has(face));
        }});
    "#,
                bytes.join(",")
            ),
            "test",
        )
        .unwrap();
    for _ in 0..5 {
        engine.tick().unwrap();
    }
    assert_eq!(
        engine.eval("JSON.stringify(log)", "test").unwrap(),
        r#"["bytes loaded","bad SyntaxError","local NetworkError","junk true SyntaxError error","Css loaded,Later unloaded,Bytes loaded",15,15,true,true,false,true]"#
    );
}

#[test]
fn detached_nodes_keep_styles_that_a_frame_holds() {
    let (host, engine) = engine("<html><body><div id='editor'></div></body></html>");
    engine
        .eval(
            "const editor=document.getElementById('editor');document.createElement('i');getComputedStyle(editor).color;",
            "test",
        )
        .unwrap();
    let held = host.borrow().store.borrow().pages[&1]
        .state
        .styles
        .computed
        .clone()
        .unwrap();
    engine
        .eval(
            "for(let i=0;i<20;i++)document.createElement('b').textContent='x';getComputedStyle(editor).color",
            "test",
        )
        .unwrap();
    assert!(Rc::ptr_eq(
        &held,
        host.borrow().store.borrow().pages[&1]
            .state
            .styles
            .computed
            .as_ref()
            .unwrap()
    ));
}
#[test]
fn detached_editor_overlays_preserve_connected_styles() {
    let (host, engine) =
        engine("<html><body><div id='editor' style='color:red'></div></body></html>");
    engine
        .eval(
            "const editor=document.getElementById('editor');getComputedStyle(editor).color;",
            "test",
        )
        .unwrap();
    let cached = Rc::as_ptr(
        host.borrow().store.borrow().pages[&1]
            .state
            .styles
            .computed
            .as_ref()
            .unwrap(),
    );
    engine
        .eval(
            "editor.setAttribute('style','color:red');editor.removeAttribute('missing')",
            "test",
        )
        .unwrap();
    assert_eq!(
        cached,
        Rc::as_ptr(
            host.borrow().store.borrow().pages[&1]
                .state
                .styles
                .computed
                .as_ref()
                .unwrap()
        )
    );
    engine
        .eval(
            r#"
        const fragment=document.createDocumentFragment();
        for(let i=0;i<100;i++) {
            const overlay=document.createElement('div');
            overlay.style.cssText='position:absolute;color:blue';
            overlay.dataset.selectionRange='';
            overlay.textContent='selection';
            fragment.append(overlay);
            if(getComputedStyle(editor).color!=='rgb(255, 0, 0)')throw Error('Connected style changed');
        }
    "#,
            "test",
        )
        .unwrap();
    assert_eq!(
        cached,
        Rc::as_ptr(
            host.borrow().store.borrow().pages[&1]
                .state
                .styles
                .computed
                .as_ref()
                .unwrap()
        )
    );
    engine.eval(r#"
        editor.append(fragment);
        if(getComputedStyle(editor.firstChild).color!=='rgb(0, 0, 255)')throw Error('Inserted style was stale');
        fragment.append(editor.firstChild);
    "#, "test").unwrap();
    assert!(
        host.borrow().store.borrow().pages[&1]
            .state
            .styles
            .computed
            .is_none()
    );
    engine.eval(r#"
        const root=editor.attachShadow({mode:'open'});
        root.innerHTML='<span style="color:green">text</span>';
        if(getComputedStyle(root.firstChild).color!=='rgb(0, 128, 0)')throw Error('Shadow style');
        root.firstChild.style.color='purple';
        if(getComputedStyle(root.firstChild).color!=='rgb(128, 0, 128)')throw Error('Shadow mutation was stale');
    "#, "test").unwrap();
}

#[test]
fn invalidation_skips_text_edits_and_unused_states() {
    let (host, engine) = engine(
        "<html><head><style id=s>p:empty{color:red}</style></head><body><p id=p>text</p><svg><text>a</text></svg></body></html>",
    );
    let computed = || {
        host.borrow().store.borrow().pages[&1]
            .state
            .styles
            .computed
            .as_ref()
            .map(Rc::as_ptr)
    };
    let p = host.borrow().store.borrow().pages[&1]
        .doc
        .nodes
        .iter()
        .position(|node| node.tag == "p")
        .unwrap();
    let revision = || host.borrow().store.borrow().pages[&1].doc.nodes.latest();
    let hover = |ids: &[usize]| {
        host.borrow_mut()
            .call(Role::Renderer(1), "dom.hover", json!([ids]))
            .unwrap();
    };
    engine
        .eval(
            "const p=document.getElementById('p');getComputedStyle(p).color;",
            "test",
        )
        .unwrap();
    let cached = computed();
    assert!(cached.is_some());
    let built = revision();
    engine
        .eval(
            "p.firstChild.data='other';document.querySelector('svg text').firstChild.data='b';",
            "test",
        )
        .unwrap();
    hover(&[p]);
    hover(&[]);
    assert_eq!(computed(), cached);
    assert_ne!(revision(), built);
    engine
        .eval(
            r#"
        p.firstChild.data='';
        if(getComputedStyle(p).color!=='rgb(255, 0, 0)')throw Error('Empty text was stale');
        const sheet=document.getElementById('s');
        sheet.textContent='p:hover{color:blue}';
        sheet.firstChild.data='p:hover{color:green}';
        if(getComputedStyle(p).color==='rgb(0, 128, 0)')throw Error('Hover without state');
    "#,
            "test",
        )
        .unwrap();
    hover(&[p]);
    assert!(computed().is_none());
    engine
        .eval(
            "if(getComputedStyle(p).color!=='rgb(0, 128, 0)')throw Error('Hover state was stale')",
            "test",
        )
        .unwrap();
    host.borrow()
        .store
        .borrow_mut()
        .page_state_mut(1)
        .unwrap()
        .fonts
        .aliases
        .insert("Brand".into(), "Real Font".into());
    engine
        .eval("p.style.fontFamily='Brand';if(getComputedStyle(p).fontFamily!=='Real Font')throw Error('Alias')", "test")
        .unwrap();
    let first = host.borrow().store.borrow().pages[&1]
        .state
        .styles
        .computed
        .as_ref()
        .unwrap()
        .values[p]
        .clone();
    host.borrow()
        .store
        .borrow_mut()
        .page_state_mut(1)
        .unwrap()
        .styles
        .computed = None;
    engine.eval("getComputedStyle(p).color", "test").unwrap();
    assert!(Rc::ptr_eq(
        &first,
        &host.borrow().store.borrow().pages[&1]
            .state
            .styles
            .computed
            .as_ref()
            .unwrap()
            .values[p]
    ));
}

#[test]
fn incremental_styles_match_full_passes() {
    let (host, engine) = engine("<html><head><style>
                html{font-size:16px} .on{color:red} .dark .leaf{color:blue} .a+.b{color:green}
                .box:has(.on){background-color:yellow} li:nth-child(2n){color:purple} p:empty{color:teal}
                .rem{padding-left:2rem} .em{padding-top:2em} .vars{color:var(--c,gray)} .x::before{content:'a'}
                .s::selection{color:white} .h:hover{color:orange} .none{display:none} [data-k=v]{color:navy}
                </style></head><body><div id=root class=box><span id=leaf class=leaf>x</span>
                <i class=a id=a></i><i class=b></i><ul id=list><li>1</li><li>2</li><li>3</li></ul>
                <p id=p>t</p><div id=sized class=em><b class=rem>r</b><b class=em>e</b></div>
                <div id=vars class=vars><span class=vars>v</span></div><div id=hover class='h s'>h</div>
                <div id=k>k</div></div></body></html>");
    let hover = host.borrow().store.borrow().pages[&1]
        .doc
        .nodes
        .iter()
        .position(|node| node.attrs.get("id").is_some_and(|id| id == "hover"))
        .unwrap();
    let read = "for(const e of document.querySelectorAll('*'))getComputedStyle(e).color;";
    engine
        .eval(&format!("for(const id of ['root','leaf','a','list','p','sized','vars','hover','k'])globalThis[id]=document.getElementById(id);{read}"), "test")
        .unwrap();
    for step in [
        "leaf.classList.add('on')",
        "root.classList.add('dark')",
        "a.classList.remove('a')",
        "a.classList.add('a')",
        "leaf.classList.remove('on')",
        "list.prepend(document.createElement('li'))",
        "list.firstChild.remove()",
        "p.firstChild.data=''",
        "document.documentElement.style.fontSize='20px'",
        "sized.style.fontSize='30px'",
        "vars.style.setProperty('--c','red')",
        "vars.firstChild.style.setProperty('--c','blue')",
        "leaf.classList.add('x')",
        "hover.classList.remove('s')",
        "hover.classList.add('none')",
        "hover.classList.remove('none')",
        "k.dataset.k='v'",
        "list.append(sized)",
        "root.prepend(list)",
        "root.removeChild(p)",
        "root.append(p)",
        "document.documentElement.style.fontSize=''",
    ] {
        engine.eval(&format!("{step};{read}"), "test").unwrap();
    }
    for ids in [vec![hover], vec![]] {
        host.borrow_mut()
            .call(Role::Renderer(1), "dom.hover", json!([ids]))
            .unwrap();
        engine.eval(read, "test").unwrap();
    }
    #[cfg(debug_assertions)]
    assert!(
        host.borrow().store.borrow().pages[&1]
            .state
            .styles
            .cache
            .incremental
            >= 20
    );
}

#[test]
fn incremental_styles_follow_class_changes() {
    let (host, engine) = engine("<html><head><style>
                .on{color:red} .dark .leaf{color:blue} .a+.b{color:green} .box:has(.hit){background-color:yellow}
                li:nth-child(1 of .pick){color:purple} .wrap:not(.off) .inner{color:teal} .nest{& .deep{color:navy}}
                .row.sel{color:olive} .row b{padding-left:1px}
                </style></head><body><div id=root class=box><div id=wrap class=wrap><span id=inner class=inner>i</span></div>
                <span id=leaf class=leaf>x</span><i id=a></i><i class=b></i><ul><li id=l1>1</li><li id=l2>2</li></ul>
                <div id=nest><b class=deep>d</b></div>
                <div id=row class=row><b>r</b></div><em id=hit>h</em></div></body></html>");
    let read = "for(const e of document.querySelectorAll('*'))getComputedStyle(e).color;";
    engine
        .eval(&format!("for(const id of ['root','wrap','leaf','a','l1','l2','nest','row','hit'])globalThis[id]=document.getElementById(id);{read}"), "test")
        .unwrap();
    for step in [
        "row.classList.add('sel')",
        "row.classList.add('zz')",
        "row.classList.remove('sel')",
        "root.classList.add('dark')",
        "a.classList.add('a')",
        "hit.classList.add('hit')",
        "l2.classList.add('pick')",
        "l1.classList.add('pick')",
        "wrap.classList.add('off')",
        "nest.classList.add('nest')",
        "row.className='other'",
        "root.classList.remove('dark')",
        "wrap.classList.remove('off')",
        "l1.classList.remove('pick')",
        "hit.classList.remove('hit')",
        "leaf.classList.add('on')",
    ] {
        engine.eval(&format!("{step};{read}"), "test").unwrap();
    }
    #[cfg(debug_assertions)]
    assert!(
        host.borrow().store.borrow().pages[&1]
            .state
            .styles
            .cache
            .incremental
            >= 15
    );
}

#[test]
fn incremental_styles_follow_added_rules_and_places() {
    let (host, engine) = engine("<html><head><style>li:last-child{padding-left:1px} li:nth-child(odd){padding-right:2px}
                li+li{padding-top:3px} ul:has(.hit) b{color:red}</style><style>@import 'tailwindcss';</style></head>
                <body><ul id=list><li id=one class=late><b>1</b></li><li><b>2</b></li><li><b>3</b></li></ul>
                <p id=leaf class='flex late'>x</p></body></html>");
    let read = "for(const e of document.querySelectorAll('*'))getComputedStyle(e).color;";
    engine
        .eval(&format!("for(const id of ['list','one','leaf'])globalThis[id]=document.getElementById(id);globalThis.sheet=document.styleSheets[0];globalThis.css=(e,p)=>getComputedStyle(e)[p];{read}"), "test")
        .unwrap();
    let mut passes = Vec::new();
    for (step, check) in [
        (
            "sheet.insertRule('.late{color:rgb(1, 1, 1)}', sheet.cssRules.length)",
            "css(leaf,'color')==='rgb(1, 1, 1)'&&css(one.firstChild,'color')==='rgb(1, 1, 1)'",
        ),
        (
            "sheet.insertRule('#one b{color:rgb(2, 2, 2)}', 0)",
            "css(one.firstChild,'color')==='rgb(2, 2, 2)'",
        ),
        (
            "const s=document.createElement('style');s.textContent='li:first-child{margin-top:4px} .late::before{content:\"x\";color:rgb(3, 3, 3)}';document.head.append(s)",
            "css(one,'marginTop')==='4px'&&getComputedStyle(leaf,'::before').color==='rgb(3, 3, 3)'",
        ),
        (
            "leaf.classList.add('pt-[3px]')",
            "css(leaf,'paddingTop')==='3px'",
        ),
        (
            "list.append(document.createElement('li'))",
            "css(list.lastChild,'paddingLeft')==='1px'&&css(list.children[2],'paddingLeft')==='0px'&&css(list.lastChild,'paddingRight')==='0px'",
        ),
        (
            "list.prepend(document.createElement('li'))",
            "css(one,'marginTop')==='0px'&&css(one,'paddingRight')==='0px'&&css(one,'paddingTop')==='3px'&&css(list.firstChild,'paddingTop')==='0px'",
        ),
        (
            "list.lastChild.className='hit'",
            "css(one.firstChild,'color')==='rgb(2, 2, 2)'&&css(list.children[2].firstChild,'color')==='rgb(255, 0, 0)'",
        ),
        (
            "list.lastChild.remove()",
            "css(list.children[2].firstChild,'color')==='rgb(1, 1, 1)'||css(list.children[2].firstChild,'color')==='rgb(0, 0, 0)'",
        ),
        (
            "sheet.insertRule('*{margin-left:5px}', 0)",
            "css(leaf,'marginLeft')==='5px'",
        ),
        ("sheet.deleteRule(0)", "css(leaf,'marginLeft')==='0px'"),
    ] {
        let before = host.borrow().store.borrow().pages[&1]
            .state
            .styles
            .cache
            .incremental;
        let result = engine
            .eval(&format!("{step};{read}{check}"), "test")
            .unwrap();
        assert_eq!(result, "true", "{step}");
        passes.push(
            host.borrow().store.borrow().pages[&1]
                .state
                .styles
                .cache
                .incremental
                > before,
        );
    }
    // A rule that any node can match needs a full pass.
    assert_eq!(
        passes,
        [true, true, true, true, true, true, true, true, false, true]
    );
}

#[test]
fn incremental_styles_renumber_counters() {
    let (host, engine) = engine("<html><head><style>main{counter-reset:sec} h2{counter-increment:sec}
                h2::before{content:counter(sec) \". \"} h2::after{content:\"counter \" counters(sec, \"-\")}
                .big{font-size:20px}</style></head><body><main id=main><h2 id=a>a</h2><p id=p>p</p><h2 id=b>b</h2>
                <section id=s><h2 id=c>c</h2></section></main></body></html>");
    // `getComputedStyle()` gives the declared counters, so the text comes from the styles.
    let read =
        "[...document.querySelectorAll('h2')].map(h=>getComputedStyle(h).color&&h._id).join()";
    let text = |ids: String| {
        let host = host.borrow().store.clone();
        let host = host.borrow();
        let computed = host.pages[&1].state.styles.computed.as_ref().unwrap();
        ids.split(',')
            .map(|id| {
                let pseudos = &computed.generated(id.parse::<usize>().unwrap()).unwrap();
                pseudos
                    .iter()
                    .map(|style| style.get(Prop::CONTENT).unwrap_or_default())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("|")
    };
    let numbers = |list: &[&str]| -> String {
        let text = |n: &&str| {
            let last = n.rsplit('-').next().unwrap();
            format!("\"{last}\" \". \"\"counter \" \"{n}\"")
        };
        list.iter().map(text).collect::<Vec<_>>().join("|")
    };
    engine
        .eval(&format!("for(const id of ['main','a','p','b','s','c'])globalThis[id]=document.getElementById(id);{read}"), "test")
        .unwrap();
    let mut passes = Vec::new();
    for (step, expected) in [
        ("p.className='big'", &["1", "2", "3"][..]),
        (
            "main.prepend(document.createElement('h2'))",
            &["1", "2", "3", "4"],
        ),
        ("b.remove()", &["1", "2", "3"]),
        ("s.style.counterReset='sec 7'", &["1", "2", "2-8"]),
        ("s.style.counterReset=''", &["1", "2", "3"]),
        ("a.className='big'", &["1", "2", "3"]),
    ] {
        let before = host.borrow().store.borrow().pages[&1]
            .state
            .styles
            .cache
            .incremental;
        let result = text(engine.eval(&format!("{step};{read}"), "test").unwrap());
        assert_eq!(result, numbers(expected), "{step}");
        passes.push(
            host.borrow().store.borrow().pages[&1]
                .state
                .styles
                .cache
                .incremental
                > before,
        );
    }
    assert_eq!(passes, [true; 6]);
}

#[test]
fn cousins_that_share_matches_restyle_with_their_own_ancestors() {
    let rows =
        "<div class=row><span class=x>1</span></div><div class=row><span class=x>2</span></div>"
            .repeat(2);
    let (host, engine) = engine(&format!(
                "<html><head><style>.a .x{{color:red}} .g[data-k=v] .x{{padding-left:1px}} .row.on .x{{color:blue}}
                .g:hover .x{{color:green}}</style></head><body><div id=list><div id=g1 class=g>{rows}</div>
                <div id=g2 class=g>{rows}</div><div id=g3 class=g>{rows}</div></div></body></html>"
            ));
    let read = "for(const e of document.querySelectorAll('*'))getComputedStyle(e).color;";
    engine
        .eval(
            &format!(
                "for(const id of ['g1','g2','g3'])globalThis[id]=document.getElementById(id);{read}"
            ),
            "test",
        )
        .unwrap();
    for step in [
        "g2.classList.add('a')",
        "g3.children[3].classList.add('on')",
        "g1.dataset.k='v'",
        "g2.children[1].classList.add('on')",
        "g2.classList.remove('a')",
        "g3.append(g1.children[0])",
        "g1.dataset.k='w'",
    ] {
        engine.eval(&format!("{step};{read}"), "test").unwrap();
    }
    let g3 = host.borrow().store.borrow().pages[&1]
        .doc
        .nodes
        .iter()
        .position(|node| node.attrs.get("id").is_some_and(|id| id == "g3"))
        .unwrap();
    host.borrow_mut()
        .call(Role::Renderer(1), "dom.hover", json!([[g3]]))
        .unwrap();
    engine.eval(read, "test").unwrap();
    #[cfg(debug_assertions)]
    assert!(
        host.borrow().store.borrow().pages[&1]
            .state
            .styles
            .cache
            .incremental
            >= 8
    );
}

#[test]
fn incremental_styles_adjust_collapsed_tables_again() {
    let rows = "<tr><td>a</td><td>b</td></tr>".repeat(4);
    let (host, engine) = engine(&format!(
                "<html><head><style>table{{border-collapse:collapse}} td{{border:1px solid #ccc}}
                tr.danger td{{background-color:#fdd}} tr.thick td{{border-bottom:3px solid red}} tr.wide{{border-top:4px solid blue}}
                .x{{border-left:5px double green}} .blocky{{display:block}}</style></head><body><table id=t><thead><tr><th>h</th><th>i</th></tr></thead>
                <tbody id=b>{rows}</tbody></table><table id=u style='border:2px solid'><tr><td>c</td></tr></table></body></html>"
            ));
    let read = "for(const e of document.querySelectorAll('*'))getComputedStyle(e).color;";
    engine
        .eval(
            &format!(
                "for(const id of ['t','b','u'])globalThis[id]=document.getElementById(id);{read}"
            ),
            "test",
        )
        .unwrap();
    for step in [
        "b.rows[1].className='danger'",
        "b.rows[1].className='thick'",
        "b.rows[2].className='wide'",
        "b.rows[0].cells[1].className='x'",
        "b.rows[3].cells[0].colSpan=2",
        "b.rows[3].deleteCell(1)",
        "b.insertRow().insertCell().textContent='n'",
        "b.rows[0].remove()",
        "t.style.border='6px solid'",
        "u.rows[0].insertCell().style.border='7px dashed'",
        "b.rows[1].className=''",
        "t.style.borderCollapse='separate'",
        "t.style.borderCollapse=''",
        "t.classList.add('blocky')",
        "t.classList.remove('blocky')",
        "b.rows[0].className='danger'",
    ] {
        engine.eval(&format!("{step};{read}"), "test").unwrap();
    }
    #[cfg(debug_assertions)]
    assert!(
        host.borrow().store.borrow().pages[&1]
            .state
            .styles
            .cache
            .incremental
            >= 12
    );
}

#[test]
fn incremental_styles_match_full_passes_in_shadow_trees() {
    let (host, engine) = engine("<html><head><style>
                :root{--ink:navy} x-card.big{font-size:20px} x-card::part(title){color:green}
                x-card.alt::part(title){color:olive} x-card::part(deep){padding-left:3px}
                #b{margin-top:1px} .late{color:maroon}
                </style></head><body><x-card id=card class=c><span id=a class=on>a<s id=s></s></span>
                <b id=b slot=side>b</b><i id=i>i</i></x-card><x-card id=other></x-card>
                <x-card id=third></x-card><div id=late class=late><p id=lp>late</p></div></body></html>");
    let card = host.borrow().store.borrow().pages[&1]
        .doc
        .nodes
        .iter()
        .position(|node| node.attrs.get("id").is_some_and(|id| id == "card"))
        .unwrap();
    let read = "for(const e of document.querySelectorAll('*'))getComputedStyle(e).color;";
    engine
        .eval(&format!(r#"
            for (const id of ['card','a','b','i','other','third','late']) globalThis[id] = document.getElementById(id);
            globalThis.root = card.attachShadow({{mode:'open'}});
            root.innerHTML = '<style>:host{{display:block;color:var(--ink)}} :host(.on){{border-top:1px solid red}} :host(:hover){{color:orange}} ::slotted(.on){{color:red}} slot[name=side]::slotted(*){{padding-top:2px}} .t{{font-weight:700;font-size:2em}} slot{{color:var(--tone,inherit)}} slot.wide::slotted(*){{padding-left:9px}} :host(:has(> .flag)){{padding-right:2px}} ::slotted(:first-child){{padding-bottom:4px}} ::slotted(:has(> .mark)){{padding-right:5px}}</style><h1 part=title class=t id=t>t</h1><slot id=main></slot><slot name=side id=side>fallback</slot><x-inner id=inner exportparts="core: deep"></x-inner>';
            globalThis.inner = root.getElementById('inner').attachShadow({{mode:'open'}});
            inner.innerHTML = '<style>:host{{display:block}} b{{color:var(--ink)}}</style><b part=core id=core>c</b>';
            globalThis.sheet = new CSSStyleSheet();
            sheet.replaceSync(':host{{margin-left:4px}} p{{color:teal}}');
            for (const host of [other, third]) {{ const shadow = host.attachShadow({{mode:'open'}}); shadow.adoptedStyleSheets = [sheet]; shadow.innerHTML = '<p>o</p>'; }}
            {read}"#), "test")
        .unwrap();
    // Only debug builds count the incremental passes.
    #[cfg(debug_assertions)]
    let mut full = Vec::new();
    for step in [
        "card.classList.add('on')",
        "card.classList.add('alt')",
        "card.classList.add('big')",
        "document.documentElement.style.setProperty('--ink','purple')",
        "card.style.setProperty('--ink','teal')",
        "card.style.setProperty('--tone','gray')",
        "a.classList.remove('on')",
        "a.classList.add('on')",
        "root.getElementById('main').classList.add('wide')",
        "root.getElementById('main').classList.remove('wide')",
        "a.classList.add('flag')",
        "a.classList.remove('flag')",
        "card.prepend(document.createElement('u'))",
        "document.getElementById('s').classList.add('mark')",
        "document.getElementById('s').classList.remove('mark')",
        "b.slot=''",
        "b.slot='side'",
        "root.getElementById('side').setAttribute('name','none')",
        "root.getElementById('side').setAttribute('name','side')",
        "root.getElementById('t').setAttribute('part','other')",
        "root.getElementById('t').setAttribute('part','title')",
        "root.getElementById('inner').setAttribute('exportparts','core')",
        "root.getElementById('inner').setAttribute('exportparts','core: deep')",
        "inner.getElementById('core').classList.add('x')",
        "card.append(document.createElement('em'))",
        "root.append(document.createElement('div'))",
        "i.remove()",
        "card.prepend(i)",
        "other.remove()",
        "document.body.append(other)",
        "late.attachShadow({mode:'open'}).adoptedStyleSheets=[sheet]",
        "late.shadowRoot.innerHTML='<slot></slot><p>s</p>'",
        "third.remove()",
        "card.classList.remove('big')",
        "card.classList.remove('on')",
        "sheet.replaceSync(':host{margin-left:5px} p{color:red}')",
        "card.classList.add('on')",
    ] {
        #[cfg(debug_assertions)]
        let before = host.borrow().store.borrow().pages[&1]
            .state
            .styles
            .cache
            .incremental;
        engine.eval(&format!("{step};{read}"), "test").unwrap();
        #[cfg(debug_assertions)]
        if host.borrow().store.borrow().pages[&1]
            .state
            .styles
            .cache
            .incremental
            == before
        {
            full.push(step);
        }
    }
    for ids in [vec![card], vec![]] {
        host.borrow_mut()
            .call(Role::Renderer(1), "dom.hover", json!([ids]))
            .unwrap();
        engine.eval(read, "test").unwrap();
    }
    #[cfg(debug_assertions)]
    assert_eq!(
        full,
        ["sheet.replaceSync(':host{margin-left:5px} p{color:red}')"]
    );
    let colors = engine
        .eval(
            "JSON.stringify([getComputedStyle(card).color, getComputedStyle(a).color, getComputedStyle(root.getElementById('t')).color, getComputedStyle(inner.getElementById('core')).paddingLeft, getComputedStyle(late.shadowRoot.querySelector('p')).color])",
            "test",
        )
        .unwrap();
    assert_eq!(
        colors,
        r#"["rgb(0, 128, 128)","rgb(255, 0, 0)","rgb(128, 128, 0)","3px","rgb(255, 0, 0)"]"#
    );
}

#[test]
fn runtime_stylesheets_reuse_parses_and_evict_old_text() {
    let mut doc = Document::parse(
        &(0..200)
            .map(|index| format!("<style>.item{index}{{color:red}}</style>"))
            .collect::<String>(),
    );
    let mut state = PageState::default();
    state.restyle(&mut doc, [800., 600.]);
    let sheets = state.styles.sheets.clone();
    assert_eq!(sheets.len(), 200);
    state.styles.computed = None;
    state.restyle(&mut doc, [800., 600.]);
    for (source, sheet) in sheets {
        assert!(Rc::ptr_eq(&sheet, &state.styles.sheets[&source]));
    }
    let sheet = doc
        .nodes
        .iter()
        .position(|node| node.tag == "style")
        .unwrap();
    let text = doc.nodes[sheet].children[0];
    doc.nodes[text].text = ".replacement{color:blue}".into();
    state.styles.computed = None;
    state.restyle(&mut doc, [800., 600.]);
    assert_eq!(state.styles.sheets.len(), 200);
    assert!(!state.styles.sheets.contains_key(".item0{color:red}"));
    let head = doc.nodes[sheet].parent.unwrap();
    doc.nodes[head].children.retain(|id| *id != sheet);
    doc.nodes[sheet].parent = None;
    state.styles.computed = None;
    state.restyle(&mut doc, [800., 600.]);
    assert_eq!(state.styles.sheets.len(), 199);
    assert!(!state.styles.sheets.contains_key(".replacement{color:blue}"));
}

#[test]
fn registrations_restyle_the_nodes_that_read_them() {
    let mut doc = Document::parse(
        "<style>p{width:var(--w,5px)}b{--x:1}b span{height:var(--x,2px)}i{color:red}</style><p>x</p><b><span>y</span></b><i>z</i>",
    );
    let mut state = PageState::default();
    state.restyle(&mut doc, [800., 600.]);
    for (name, initial) in [("--w", "initial-value:7px"), ("--x", "")] {
        let rule = format!("@property {name}{{syntax:\"*\";inherits:false;{initial}}}");
        let value = css::sheet::Stylesheet::parse(&rule).property().unwrap();
        state
            .styles
            .cache
            .scripted
            .insert(css::properties::Prop::new(name), value);
    }
    state.styles.invalidate();
    let computed = state.restyle(&mut doc, [800., 600.]);
    let find = |tag: &str| doc.nodes.iter().position(|node| node.tag == tag).unwrap();
    assert_eq!(&computed.values[find("p")]["width"], "7px");
    assert_eq!(&computed.values[find("span")]["height"], "2px");
    assert_eq!(&computed.values[find("i")]["color"], "red");
}

#[test]
fn replaced_style_text_keeps_the_other_sheets() {
    let mut doc = Document::parse(
        "<style>p{color:red}</style><style>.a{color:green}.a+p{width:1px}</style><style>b{color:black}</style><p class=a>x</p><p>y</p><b>z</b><i>w</i>",
    );
    let find = |doc: &Document, tag: &str, nth: usize| {
        let mut found = doc
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.tag == tag);
        found.nth(nth).unwrap().0
    };
    let mut state = PageState::default();
    state.restyle(&mut doc, [800., 600.]);
    let sheet = find(&doc, "style", 1);
    let text = doc.nodes[sheet].children[0];
    doc.nodes[text].text = ".a{color:blue}".into();
    doc.nodes[sheet].stylesheet = None;
    state.styles.invalidate();
    let computed = state.restyle(&mut doc, [800., 600.]);
    let [a, p, b] = [find(&doc, "p", 0), find(&doc, "p", 1), find(&doc, "b", 0)];
    assert_eq!(&computed.values[a]["color"], "blue");
    assert_eq!(&computed.values[p]["color"], "red");
    assert_eq!(computed.values[p].get(Prop::WIDTH), None);
    assert_eq!(&computed.values[b]["color"], "black");
}

#[test]
fn removed_nodes_drop_caches_and_style_again_when_they_return() {
    let mut doc = Document::parse(
        "<style>.on p{color:red}p:first-child{width:1px}</style><div class=on><p>x</p></div><div id=b></div>",
    );
    let find = |doc: &Document, tag: &str| doc.nodes.iter().position(|node| node.tag == tag);
    let p = find(&doc, "p").unwrap();
    let on = doc.nodes[p].parent.unwrap();
    let mut state = PageState::default();
    let viewport = [800., 600.];
    state.restyle(&mut doc, viewport);
    doc.nodes[on].children.retain(|&id| id != p);
    doc.nodes[p].parent = None;
    state.tree.removed.push(p);
    state.styles.invalidate();
    state.restyle(&mut doc, viewport);
    assert!(state.tree.removed.is_empty());
    doc.nodes[on].children.push(p);
    doc.nodes[p].parent = Some(on);
    state.styles.invalidate();
    let returned = state.restyle(&mut doc, viewport);
    let fresh = PageState::default().restyle(&mut doc, viewport);
    assert_eq!(returned.values[p], fresh.values[p]);
    assert_eq!(returned.values[p].get(Prop::COLOR), Some("red"));
}

#[test]
fn new_nodes_grow_cached_styles_like_a_full_pass() {
    let mut doc = Document::parse("<p style='font-size:30px;--x:1'>a</p>");
    let mut state = PageState::default();
    let viewport = [800., 600.];
    state.restyle(&mut doc, viewport);
    let id = state.allocate(
        &mut doc,
        Node {
            tag: "div".into(),
            ..Default::default()
        },
    );
    let read = |doc: &mut Document, state: &mut PageState| {
        call(
            "dom.computed",
            &json!([id, "color", ""]),
            doc,
            state,
            viewport,
        )
        .unwrap()
    };
    assert_eq!(read(&mut doc, &mut state), json!(""));
    let cached = state.restyle(&mut doc, viewport);
    let fresh = PageState::default().restyle(&mut doc, viewport);
    assert_eq!(cached.vars.len(), doc.nodes.len());
    for node in 0..doc.nodes.len() {
        assert!(cached.lengths[node] == fresh.lengths[node], "{node}");
    }
    let p = (0..doc.nodes.len())
        .find(|&node| doc.nodes[node].tag == "p")
        .unwrap();
    doc.nodes[p].attrs.insert("class".into(), "x".into());
    state.styles.invalidate();
    state.restyle(&mut doc, viewport);
}

#[test]
fn trim_cuts_the_free_end_of_the_arena_and_ids_come_back_lowest_first() {
    let mut doc = Document::parse("<div></div>");
    let mut state = PageState::default();
    let viewport = [800., 600.];
    let node = || Node {
        tag: "p".into(),
        ..Default::default()
    };
    let ids: Vec<usize> = (0..4).map(|_| state.allocate(&mut doc, node())).collect();
    state.restyle(&mut doc, viewport);
    for id in [ids[3], ids[1], ids[2]] {
        doc.nodes[id] = Node::default();
        state.tree.free.push(std::cmp::Reverse(id));
    }
    assert!(state.trim(&mut doc));
    assert_eq!(doc.nodes.len(), ids[1]);
    assert_eq!(state.tree.free.len(), 0);
    assert!(!state.trim(&mut doc));
    state.restyle(&mut doc, viewport);
    assert_eq!(state.allocate(&mut doc, node()), ids[1]);
    assert_eq!(state.allocate(&mut doc, node()), ids[1] + 1);
    doc.nodes[ids[1]] = Node::default();
    state.tree.free.push(std::cmp::Reverse(ids[1]));
    doc.nodes[ids[0]] = Node::default();
    state.tree.free.push(std::cmp::Reverse(ids[0]));
    assert_eq!(state.allocate(&mut doc, node()), ids[0]);
}

#[test]
fn reused_ids_drop_the_old_style_while_the_styles_are_shared() {
    let mut doc = Document::parse("<body><div><p style='color: rgb(255, 0, 0)'></p></div></body>");
    let mut state = PageState::default();
    let viewport = [800., 600.];
    let p = doc.nodes.iter().position(|node| node.tag == "p").unwrap();
    let div = doc.nodes[p].parent.unwrap();
    state.restyle(&mut doc, viewport);
    let body = doc.nodes[div].parent.unwrap();
    doc.nodes[body].children.clear();
    doc.nodes[div].parent = None;
    state.styles.invalidate();
    let shared = state.restyle(&mut doc, viewport);
    assert_eq!(shared.values[p].get(Prop::COLOR), None);
    for id in [p, div] {
        doc.nodes[id] = Node::default();
        state.tree.free.push(std::cmp::Reverse(id));
    }
    let node = Node {
        tag: "p".into(),
        ..Default::default()
    };
    assert_eq!(state.allocate(&mut doc, node.clone()), div);
    assert_eq!(state.allocate(&mut doc, node), p);
    assert_eq!(
        state.restyle(&mut doc, viewport).values[p].get(Prop::COLOR),
        None
    );
}

#[test]
fn trim_shrinks_the_layout_mirror_under_a_page_that_holds_it() {
    let (host, engine) = engine("<html><body><p id='a'></p></body></html>");
    let a = host.borrow().store.borrow().pages[&1]
        .doc
        .nodes
        .iter()
        .position(|node| node.tag == "p")
        .unwrap();
    let grow = |rect| {
        let host = host.borrow().store.clone();
        let mut host = host.borrow_mut();
        let host = &mut *host;
        let page = host.pages.get_mut(&1).unwrap();
        let (doc, state) = (&mut page.doc, &mut page.state);
        let ids: Vec<usize> = (0..20_000)
            .map(|_| state.allocate(doc, Node::default()))
            .collect();
        state.geometry.mirror.insert(a, rect);
        state.geometry.mirror.insert(ids[ids.len() - 1], rect);
        ids
    };
    let ids = grow([0., 0., 10., 10.]);
    assert_eq!(
        engine
            .eval(
                "globalThis.a = document.getElementById('a'); a.offsetWidth",
                "test"
            )
            .unwrap(),
        "10"
    );
    {
        let host = host.borrow().store.clone();
        let mut host = host.borrow_mut();
        let host = &mut *host;
        let page = host.pages.get_mut(&1).unwrap();
        let (doc, state) = (&mut page.doc, &mut page.state);
        for &id in &ids {
            state.tree.free.push(std::cmp::Reverse(id));
        }
        assert!(state.trim(doc));
        assert_eq!(state.geometry.mirror.capacity(), 1024);
        assert_eq!(state.geometry.mirror.get(&a), Some(&[0., 0., 10., 10.]));
        state.geometry.mirror.insert(a, [0., 0., 30., 10.]);
        state.geometry.mirror.touch();
    }
    assert_eq!(engine.eval("a.offsetWidth", "test").unwrap(), "30");
    grow([0., 0., 40., 10.]);
    assert_eq!(engine.eval("a.offsetWidth", "test").unwrap(), "40");
}
