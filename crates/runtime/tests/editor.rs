use apps_runtime::{
    dom::Document,
    host::{Host, Page, Role},
    jsc::{JavaScriptCore, JsContext},
};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

#[test]
fn editor_selection_keyboard_and_input_events() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse(
                "<html><body><test-editor></test-editor><textarea></textarea></body></html>",
            ),
            dirty: false,
        },
    );
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    engine
        .eval(include_str!("editor.js"), "editor-tests")
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        engine.tick().unwrap();
        let result = engine.eval("editorResult", "test").unwrap();
        if result != "pending" {
            assert_eq!(result, "ok");
            break;
        }
        assert!(Instant::now() < deadline, "Editor tests timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
    let composition = serde_json::json!({"op":"window","args":["composition",1]});
    let ended = host
        .borrow()
        .commands
        .iter()
        .filter(|command| **command == composition)
        .count();
    assert_eq!(
        ended, 7,
        "Each finished composition asks the window to drop its own"
    );
}

#[test]
fn page_text_selection_respects_user_select() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<html><body><p id='first'>alpha beta</p><p id='blocked' style='user-select:none'>hidden<span id='override' style='user-select:text'>visible</span></p><p id='last'>gamma delta</p><p id='all' style='user-select:all'>whole <span>group</span></p><p id='alias' style='-webkit-user-select:none'>alias</p><button>button</button></body></html>"),
        dirty: false,
    });
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    engine.eval(r#"
        const assert=(value,message)=>{if(!value)throw Error(message)};
        const first=document.getElementById('first').firstChild, last=document.getElementById('last').firstChild;
        const blocked=document.getElementById('blocked').firstChild, override=document.getElementById('override').firstChild;
        const all=document.getElementById('all'), alias=document.getElementById('alias').firstChild;
        const selected=getSelection();
        let hit=first;
        document.caretRangeFromPoint=x=>{const range=document.createRange();range.setStart(hit,x);return range};
        const pointer=(type,node,x,extra={})=>{hit=node;__appsDispatch(type,[node._id],{pointerId:1,clientX:x,clientY:0,buttons:type==='pointerup'?0:1,...extra})};
        pointer('pointerdown',first,1);pointer('pointerup',first,5);
        assert(selected.toString()==='lpha','Plain text drag');
        assert(__appsClipboard('copy')==='lpha','Plain text copy');
        assert(__appsEditing()===null,'Plain text does not become editable');
        pointer('pointerdown',first,7,{detail:2});pointer('pointerup',first,7);
        assert(selected.toString()==='beta','Plain text double click');
        pointer('pointerdown',first,7,{detail:3});pointer('pointerup',first,7);
        assert(selected.toString()==='alpha beta','Plain text triple click');
        pointer('pointerdown',first,0);pointer('pointerup',last,5);
        assert(selected.toString()==='alpha beta\n\nvisible\n\ngamma','Drag skips none but includes text override');
        assert(selected.containsNode(blocked,true),'DOM range still spans excluded text');
        const previous=selected.toString();
        pointer('pointerdown',blocked,2);pointer('pointerup',last,8);
        assert(selected.toString()===previous,'Starting in none preserves selection');
        pointer('pointerdown',alias,1);pointer('pointerup',last,8);
        assert(selected.toString()===previous,'WebKit alias prevents selection');
        pointer('pointerdown',first,0);pointer('pointerup',blocked,2);
        assert(selected.toString()==='alpha beta','Forward drag ends at none boundary');
        pointer('pointerdown',last,5);pointer('pointerup',blocked,2);
        assert(selected.toString()==='gamma','Backward drag ends at none boundary');
        pointer('pointerdown',override,1);pointer('pointerup',override,5);
        assert(selected.toString()==='isib','Text override permits selection');
        override.parentElement.style.userSelect='none';
        assert(selected.toString()==='','Style changes update selection exclusions');
        override.parentElement.style.userSelect='text';
        assert(selected.toString()==='isib','Style changes restore selectable text');
        pointer('pointerdown',all.firstChild,2);pointer('pointerup',all.firstChild,2);
        assert(selected.toString()==='whole group','All selects atomically');
        pointer('pointerdown',first,6);pointer('pointerup',all.lastChild.firstChild,2);
        assert(selected.toString()==='beta\n\nvisible\n\ngamma delta\n\nwhole group','Drag expands all: '+selected.toString());
        pointer('pointerdown',first,0);pointer('pointerup',first,2);
        pointer('pointerdown',first,5,{shiftKey:true});pointer('pointerup',first,5);
        assert(selected.toString()==='alpha','Shift click extends selection');
        first.parentElement.addEventListener('selectstart',e=>e.preventDefault(),{once:true});
        pointer('pointerdown',first,7);pointer('pointerup',last,5);
        assert(selected.toString()==='alpha','Canceled selectstart preserves selection');
        __appsDispatch('keydown',[],{key:'a',...(navigator.platform.includes('Mac')?{metaKey:true}:{ctrlKey:true})});
        assert(selected.toString()==='alpha beta\n\nvisible\n\ngamma delta\n\nwhole group','Select all excludes none and controls');
        const host=document.createElement('div');host.style.userSelect='none';document.body.append(host);
        const shadow=host.attachShadow({mode:'open'});shadow.innerHTML='<p>shadow hidden</p><p style="user-select:text">shadow visible</p>';
        selected.selectAllChildren(shadow);
        assert(selected.toString()==='shadow visible','User select crosses shadow host boundary');
        const editor=document.createElement('div');editor.contentEditable='true';editor.textContent='editable';blocked.parentElement.append(editor);
        editor.focus();selected.selectAllChildren(editor);
        assert(selected.toString()==='editable','Editable content remains selectable under none');
    "#, "page-selection").unwrap();
    assert_eq!(host.borrow().dom[&1].selection_caret, None);
}

#[test]
fn selection_paint_ranges_and_native_text_offsets() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<div contenteditable><span>A😀</span><b>BC</b><i>文D</i></div>"),
            dirty: false,
        },
    );
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    engine.eval("const editor=document.querySelector('div');editor.focus();const a=editor.firstChild.firstChild,b=editor.children[1].firstChild,c=editor.lastChild.firstChild;getSelection().setBaseAndExtent(c,1,a,1)", "test").unwrap();
    let ranges = host.borrow().dom[&1].selection.clone();
    for (name, expected) in [("a", [1, 3]), ("b", [0, 2]), ("c", [0, 1])] {
        let id: usize = engine
            .eval(&format!("{name}._id"), "test")
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(ranges[&id], expected);
    }
    assert_eq!(
        engine
            .eval(
                "JSON.stringify([__appsEditing().start,__appsEditing().end])",
                "test"
            )
            .unwrap(),
        "[6,1]"
    );
    engine
        .eval("getSelection().removeAllRanges()", "test")
        .unwrap();
    assert!(host.borrow().dom[&1].selection.is_empty());
    engine
        .eval("getSelection().collapse(c,1);__appsInput('x')", "test")
        .unwrap();
    assert_eq!(
        engine.eval("editor.textContent", "test").unwrap(),
        "A😀BC文xD"
    );
}

#[test]
fn empty_selection_fragments_are_not_carets() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(1, Page {
        doc: Document::parse("<div contenteditable><span>A</span><span></span><span>\n</span><span>B</span></div><textarea>A\n\nB</textarea><pre>read only</pre>"),
        dirty: false,
    });
    let engine = JsContext::new(
        JavaScriptCore::load().unwrap(),
        host.clone(),
        Role::Renderer(1),
    )
    .unwrap();
    engine.eval("const editor=document.querySelector('div');editor.children[1].append(document.createTextNode(''));editor.focus();getSelection().selectAllChildren(editor)", "test").unwrap();
    let empty: usize = engine
        .eval("editor.children[1].firstChild._id", "test")
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(host.borrow().dom[&1].selection[&empty], [0, 0]);
    assert_eq!(host.borrow().dom[&1].selection_caret, None);
    engine
        .eval(
            "getSelection().collapse(editor.children[1].firstChild,0)",
            "test",
        )
        .unwrap();
    assert_eq!(host.borrow().dom[&1].selection_caret, Some(empty));
    engine
        .eval(
            "getSelection().collapse(document.querySelector('pre').firstChild,4)",
            "test",
        )
        .unwrap();
    assert_eq!(host.borrow().dom[&1].selection_caret, None);
    engine
        .eval("getSelection().removeAllRanges()", "test")
        .unwrap();
    assert_eq!(host.borrow().dom[&1].selection_caret, None);
    engine
        .eval(
            "const input=document.querySelector('textarea');input.focus();input.select()",
            "test",
        )
        .unwrap();
    assert_eq!(host.borrow().dom[&1].selection_caret, None);
    engine.eval("input.setSelectionRange(2,2)", "test").unwrap();
    let input: usize = engine.eval("input._id", "test").unwrap().parse().unwrap();
    assert_eq!(host.borrow().dom[&1].selection_caret, Some(input));
}

#[test]
fn native_ranges_respect_element_boundaries_and_utf16_offsets() {
    let mut doc = Document::parse("<div><span>A😀</span><b>BC</b><i>文D</i><em>outside</em></div>");
    let mut state = apps_runtime::dom::State::default();
    let parent = doc.nodes.iter().position(|node| node.tag == "div").unwrap();
    let children = doc.nodes[parent].children.clone();
    let text: Vec<_> = children
        .iter()
        .map(|&id| doc.nodes[id].children[0])
        .collect();
    for (bounds, expected) in [
        (
            [text[0], 1, text[2], 1],
            serde_json::json!([[text[0], 1, 3], [text[1], 0, 2], [text[2], 0, 1]]),
        ),
        (
            [parent, 1, parent, 3],
            serde_json::json!([[text[1], 0, 2], [text[2], 0, 2]]),
        ),
        ([parent, 1, parent, 1], serde_json::json!([])),
        (
            [text[1], 1, text[1], 1],
            serde_json::json!([[text[1], 1, 1]]),
        ),
        (
            [text[1], 1, parent, 2],
            serde_json::json!([[text[1], 1, 2]]),
        ),
    ] {
        let actual = apps_runtime::dom::call(
            "dom.rangeNodes",
            &serde_json::json!(bounds),
            &mut doc,
            &mut state,
            [800., 600.],
        )
        .unwrap();
        assert_eq!(actual, expected);
    }
}

#[test]
fn caret_hit_testing_uses_the_nearest_visible_text() {
    let mut doc = Document::parse(
        "<html><body><div contenteditable><span>first</span><span>second</span><span style='visibility:hidden'>hidden</span></div></body></html>",
    );
    let mut state = apps_runtime::dom::State::default();
    for (id, node) in doc.nodes.iter().enumerate() {
        let text = if node.tag == "span" {
            &doc.nodes[node.children[0]].text
        } else {
            &node.text
        };
        let top = match text.as_str() {
            "first" => 0.,
            "second" => 20.,
            _ => 40.,
        };
        let rect = match node.tag.as_str() {
            "#text" => [0., top, 40., 20.],
            "span" => [0., top, 100., 20.],
            _ => [0., 0., 100., 100.],
        };
        state.bounds.insert(id, rect);
    }
    for (point, expected) in [
        ([10, 10], Some("first")),
        ([90, 30], Some("second")),
        ([90, 80], Some("second")),
        ([-1, 10], None),
    ] {
        let actual = apps_runtime::dom::call(
            "dom.hitTest",
            &serde_json::json!([point[0], point[1], true, true]),
            &mut doc,
            &mut state,
            [800., 600.],
        )
        .unwrap();
        assert_eq!(
            actual
                .as_u64()
                .map(|id| doc.nodes[id as usize].text.as_str()),
            expected
        );
    }
}
