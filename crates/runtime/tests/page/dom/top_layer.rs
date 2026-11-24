use super::*;

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
        host.state(1)
            .geometry
            .mirror
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
    assert_eq!(host.borrow().pages[&1].state.interaction.top_layer, [id]);
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
        let dom = host.state(1);
        assert_eq!(dom.interaction.top_layer, [ids[0]]);
        dom.geometry.mirror.insert(ids[0], [40., 40., 20., 20.]);
        dom.geometry.mirror.insert(ids[1], [0., 0., 100., 100.]);
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
    assert!(
        host.borrow().pages[&1]
            .state
            .interaction
            .top_layer
            .is_empty()
    );
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
        assert_eq!(
            host.pages[&1].state.interaction.top_layer.first(),
            Some(&id)
        );
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
fn inert_nodes_and_modal_dialogs_take_no_hits_focus_or_selection() {
    let (host, engine) = checked_page(
        "<html><body><style>body.through dialog::backdrop{pointer-events:none}</style>\
         <div id='wrap' inert><button id='b'>b</button><span id='t'>inert text</span><dialog id='d'><button id='in'>in</button></dialog></div>\
         <div id='under'><button id='u'>u</button></div><div id='over' inert></div><button id='out'>out</button><p id='free'>free text</p>\
         <div id='pop' popover='manual'><button id='pb'>pb</button></div><div id='fs'><button id='fb'>fb</button></div></body></html>",
        r#"
        const $ = id => document.getElementById(id);
        check('inert takes no focus', () => { $('b').focus(); return document.activeElement === document.body; });
        check('other nodes take focus', () => { $('out').focus(); return document.activeElement === $('out'); });
        check('tab skips inert and hidden nodes', () => { const order = []; for (let n = 0; n < 3; n++) { __appsDispatch('keydown', [], { key: 'Tab' }); order.push(document.activeElement.id); } return order.join() === 'fb,u,out'; });
        check('inert text is not selected', () => { getSelection().selectAllChildren($('t')); return String(getSelection()) === ''; });
        check('other text is selected', () => { getSelection().selectAllChildren($('free')); return String(getSelection()) === 'free text'; });
        check('tailwind variant and attribute', () => $('t').matches('[inert] *') && $('wrap').inert);
        "#,
    );
    let id = |name: &str| -> usize {
        engine
            .eval(&format!("document.getElementById('{name}')._id"), "test")
            .unwrap()
            .parse()
            .unwrap()
    };
    let rects = [
        ("wrap", [0., 0., 50., 40.]),
        ("b", [0., 0., 50., 20.]),
        ("t", [0., 20., 50., 20.]),
        ("under", [100., 0., 50., 50.]),
        ("u", [100., 0., 50., 20.]),
        ("over", [100., 0., 50., 50.]),
        ("out", [0., 60., 50., 20.]),
        ("free", [0., 90., 50., 20.]),
        ("d", [200., 200., 100., 50.]),
        ("in", [200., 200., 50., 20.]),
        ("pop", [300., 0., 50., 50.]),
        ("pb", [300., 0., 50., 20.]),
        ("fs", [0., 120., 50., 50.]),
        ("fb", [0., 120., 50., 20.]),
    ]
    .map(|(name, rect)| (id(name), rect));
    host.borrow_mut().state(1).geometry.mirror.extend(rects);
    let fails = engine
        .eval(
            r#"(() => {
        const $ = id => document.getElementById(id), at = (x, y) => { const node = document.elementFromPoint(x, y); return node?.id || node?.localName; };
        const fails = [], check = (name, fn) => { try { if (!fn()) fails.push(name); } catch (e) { fails.push(name + ': ' + e.message); } };
        check('hits pass through inert boxes', () => at(5, 5) === 'html' && at(105, 5) === 'u');
        $('out').focus();
        $('d').showModal();
        check('a modal dialog in an inert subtree escapes it', () => document.activeElement === $('in') && at(205, 205) === 'in');
        check('its backdrop takes presses', () => at(5, 65) === 'd');
        document.body.className = 'through';
        check('a see-through backdrop shows an inert page', () => at(5, 65) === 'html' && at(105, 5) === 'html');
        check('the page takes no focus', () => { $('out').focus(); $('u').focus(); return document.activeElement === $('in'); });
        check('the page text is not selected', () => { getSelection().selectAllChildren($('free')); return String(getSelection()) === ''; });
        $('pop').showPopover();
        check('a popover above the modal dialog is inert', () => { $('pb').focus(); return document.activeElement === $('in') && at(305, 5) === 'html'; });
        $('pop').hidePopover();
        $('fs').requestFullscreen();
        check('the modal dialog stays above a fullscreen element', () => { $('fb').focus(); return document.activeElement === $('in') && at(5, 125) === 'html'; });
        $('d').close();
        check('a fullscreen element makes the rest inert', () => at(5, 125) === 'fb' && at(5, 65) === 'fs' && ($('fb').focus(), $('out').focus(), document.activeElement === $('fb')));
        document.exitFullscreen();
        check('the page is live again', () => at(5, 65) === 'out' && ($('out').focus(), document.activeElement === $('out')));
        $('wrap').inert = false;
        check('removing inert', () => at(5, 5) === 'b' && ($('b').focus(), document.activeElement === $('b')));
        return fails.join('\n');
    })()"#,
            "test",
        )
        .unwrap();
    assert_eq!(fails, "");
}
