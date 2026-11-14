use apps_runtime::{
    dom::Document,
    host::{Host, Page, Role},
    jsc::{JavaScriptCore, JsContext},
};
use std::{cell::RefCell, rc::Rc, time::Duration};

/// A preload script that makes sessions for its page, and the page.
struct Contexts {
    preload: JsContext,
    page: JsContext,
}

fn contexts(features: &str) -> Contexts {
    let host = Rc::new(RefCell::new(Host::default()));
    {
        let mut host = host.borrow_mut();
        host.disabled =
            apps_runtime::bundle::disabled_features(Some(&format!(r#"{{"features":{features}}}"#)))
                .unwrap();
        host.pages.insert(
            1,
            Page {
                doc: Document::parse("<html><body><terminal id=parsed></terminal></body></html>"),
                dirty: false,
            },
        );
    }
    let api = JavaScriptCore::load().unwrap();
    let preload = JsContext::new(api.clone(), host.clone(), Role::Preload(1)).unwrap();
    preload
        .eval(
            "globalThis.made = []; globalThis.seen = [];
             __apps.contextBridge.exposeInMainWorld('create', options => {
                 const session = new __apps.terminal.Session(options);
                 for (const type of ['data', 'exit', 'close']) session.addEventListener(type, event => seen.push([type, event.data ?? event.exitCode ?? null]));
                 made.push(session);
                 return session;
             });
             __apps.contextBridge.exposeInMainWorld('same', (session, index) => session === made[index]);",
            "terminal-test",
        )
        .unwrap();
    let page = JsContext::new(api, host, Role::Renderer(1)).unwrap();
    Contexts { preload, page }
}

impl Contexts {
    fn tick(&self) {
        self.preload.tick().unwrap();
        self.page.tick().unwrap();
    }

    fn wait(&self, ready: &str) {
        for _ in 0..5000 {
            self.tick();
            if self.page.eval(ready, "terminal-test").unwrap() == "true" {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("The terminal did not finish: {ready}");
    }
}

const SETUP: &str = "globalThis.el = document.getElementById('parsed'); globalThis.events = [];
    globalThis.listen = session => { for (const type of ['data', 'titlechange', 'cwdchange', 'bell', 'exit', 'statuschange', 'resize', 'close'])
        session.addEventListener(type, event => events.push([type, event.data ?? event.exitCode ?? null])); return session; };";

#[test]
fn pages_show_the_sessions_of_their_preload_script() {
    let contexts = contexts(r#"["terminal"]"#);
    let value = |code: &str| contexts.page.eval(code, "terminal-test").unwrap();
    let preload = |code: &str| contexts.preload.eval(code, "terminal-test").unwrap();
    value(SETUP);
    assert_eq!(
        value("[el instanceof HTMLTerminalElement, el.session, el.tabIndex].join()"),
        "true,,0"
    );
    value("globalThis.term = listen(create({ manual: true })); el.session = term");
    assert_eq!(
        value(
            "[el.session === term, same(term, 0), term.closed, term.cols, term.rows, term.pid, term.process].join()"
        ),
        "true,true,false,80,24,,"
    );
    // Only a preload script makes sessions and starts programs.
    for code in ["new term.constructor()", "term.spawn()"] {
        let error = contexts
            .page
            .eval(code, "terminal-test")
            .unwrap_err()
            .to_string();
        assert!(error.contains("is unavailable in this context"), "{error}");
    }
    let error = contexts
        .page
        .eval("el.session = {}", "terminal-test")
        .unwrap_err()
        .to_string();
    assert!(error.contains("Expected a terminal session"), "{error}");
    value(
        "term.output('hello \\x1b[1mworld\\x1b[0m\\r\\n\\x1b]2;my title\\x07\\x1b]7;file:///tmp/a%20b\\x07\\x07');
         term.output(new TextEncoder().encode('bytes'));",
    );
    assert_eq!(value("term.text"), "hello world\nbytes");
    assert_eq!(preload("made[0].text"), "hello world\nbytes");
    assert_eq!(
        value("JSON.stringify(term.cursor)"),
        r#"{"x":5,"y":1,"visible":true}"#
    );
    // Events arrive in the next task.
    assert_eq!(value("events.length"), "0");
    contexts.tick();
    assert_eq!(
        value("JSON.stringify([events.splice(0), term.title, term.cwd])"),
        r#"[[["titlechange",null],["cwdchange",null],["bell",null]],"my title","/tmp/a b"]"#
    );
    assert_eq!(preload("made[0].title"), "my title");
    // Without a program, the scripts get the input, with the answers of the terminal.
    value("term.input('ls\\r'); term.paste('a\\nb'); term.output('\\x1b[6n')");
    contexts.tick();
    let input = r#"[["data","ls\r"],["data","a\rb"],["data","\u001b[2;6R"]]"#;
    assert_eq!(value("JSON.stringify(events.splice(0))"), input);
    assert_eq!(preload("JSON.stringify(seen.splice(0))"), input);
    value("term.selectAll()");
    assert_eq!(value("term.selectedText"), "hello world\nbytes");
    value("term.clearSelection(); term.reset()");
    assert_eq!(
        value("term.selectedText + term.text + term.alternateScreen"),
        "false"
    );
    // The program status protocol (OSC 7501).
    value(
        "term.output('\\x1b]7501;state=blocked:kind=permission:app=terraform:progress=40:msg=QXBwbHk/\\x1b\\\\');
         term.output('\\x1b]7501;state=working:id=plan:title=UGxhbg==\\x07')",
    );
    contexts.tick();
    assert_eq!(
        value("JSON.stringify([events.splice(0), term.programStatus])"),
        concat!(
            r#"[[["statuschange",null]],[{"id":"","state":"blocked","kind":"permission","progress":40,"app":"terraform","title":"","message":"Apply?"},"#,
            r#"{"id":"plan","state":"working","kind":null,"progress":null,"app":"terraform","title":"Plan","message":""}]]"#
        )
    );
    // An input method composes text at the cursor, and the result goes to the program.
    value(
        "el.focus(); for (const type of ['compositionstart', 'compositionupdate', 'compositionend']) el.addEventListener(type, event => events.push([type, event.data]));
         __appsInput('ni', undefined, undefined, true, [2, 2]); globalThis.editing = JSON.stringify(__appsEditing());
         __appsInput('你', undefined, undefined, false); __appsInput('x', undefined, undefined, false)",
    );
    contexts.tick();
    assert_eq!(
        value("JSON.stringify([events.splice(0), JSON.parse(editing).text, __appsEditing().text])"),
        r#"[[["compositionstart",""],["compositionupdate","ni"],["compositionend","你"],["data","你"],["data","x"]],"ni",""]"#
    );
    // The clipboard commands of the page copy the selection and paste to the program.
    value("term.output('copy me'); term.selectAll()");
    assert_eq!(value("__appsClipboard('copy')"), "copy me");
    value("__appsClipboard('paste', 'pasted')");
    contexts.tick();
    assert_eq!(
        value("JSON.stringify(events.splice(0))"),
        r#"[["data","pasted"]]"#
    );
    // A session shows in one element, and it stays open without one.
    value(
        "globalThis.other = document.createElement('terminal'); document.body.append(other); other.session = create({ manual: true }); other.session.output('second')",
    );
    assert_eq!(
        value("[other.session.text, term.text].join()"),
        "second,copy me"
    );
    value("globalThis.second = other.session; other.session = term");
    assert_eq!(
        value("[el.session, other.session === term, second.closed].join()"),
        ",true,false"
    );
    value("other.session = null; el.session = term");
    assert_eq!(
        value("[other.session, term.text, __appsEditing() !== null].join()"),
        ",copy me,true"
    );
    // The preload script reaches the element through its `document`.
    preload(
        "document.getElementById('parsed').session = made[1]; globalThis.read = document.getElementById('parsed').session === made[1]",
    );
    assert_eq!(
        value("[el.session === second, term.closed].join()") + &preload("read"),
        "true,falsetrue"
    );
    // A session stays in its window.
    let error = contexts
        .preload
        .eval(
            "__apps.ipcRenderer.send('session', made[0])",
            "terminal-test",
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("could not be cloned"), "{error}");
    // An element that moves in the document keeps its session. One that leaves the document ends it.
    contexts.tick();
    value("listen(second); document.body.prepend(el)");
    contexts.tick();
    assert_eq!(value("[second.closed, events.length].join()"), "false,0");
    preload("seen.length = 0");
    value("el.remove()");
    contexts.tick();
    assert_eq!(
        value("JSON.stringify([second.closed, el.session, events.splice(0)])"),
        r#"[true,null,[["close",null]]]"#
    );
    assert_eq!(
        preload("JSON.stringify([made[1].closed, seen.splice(0)])"),
        r#"[true,[["close",null]]]"#
    );
    let error = contexts
        .page
        .eval("second.text", "terminal-test")
        .unwrap_err()
        .to_string();
    assert!(error.contains("The terminal session is closed"), "{error}");
    // `close()` ends a session in both contexts.
    value("other.session = term");
    preload("made[0].close()");
    contexts.tick();
    assert_eq!(
        value(
            "JSON.stringify([term.closed, other.session, events.splice(0), create({ manual: true }).closed])"
        ),
        r#"[true,null,[["close",null]],false]"#
    );
    assert_eq!(
        preload("JSON.stringify(seen.splice(0))"),
        r#"[["close",null]]"#
    );
}

#[cfg(unix)]
#[test]
fn sessions_run_programs() {
    let contexts = contexts(r#"["terminal"]"#);
    let value = |code: &str| contexts.page.eval(code, "terminal-test").unwrap();
    let preload = |code: &str| contexts.preload.eval(code, "terminal-test").unwrap();
    value(SETUP);
    // The program starts with the session.
    let pid = value(
        "globalThis.term = listen(create({ command: ['/bin/sh', '-c', 'printf \"in %s\" \"$PWD\"; read line; echo \"got $line $NAME\"; exit 3'], cwd: '/', env: { NAME: 'env', PATH: '/usr/bin:/bin' } })); term.pid",
    );
    assert!(pid.parse::<u32>().unwrap() > 0, "{pid}");
    contexts.wait("term.text.includes('in /')");
    assert_eq!(value("term.process.pid"), pid);
    assert_eq!(value("term.cwd"), "/");
    value("term.input('hi\\r')");
    contexts.wait("events.some(event => event[0] === 'exit') && term.text.includes('got hi env')");
    assert_eq!(
        value(
            "JSON.stringify([events.filter(event => event[0] === 'exit'), term.pid, term.process])"
        ),
        r#"[[["exit",3]],null,null]"#
    );
    assert_eq!(
        preload("JSON.stringify(seen.filter(event => event[0] === 'exit'))"),
        r#"[["exit",3]]"#
    );
    // A command line runs in the shell of the user, and a signal ends it.
    value("events.length = 0; term.reset()");
    preload("made[0].spawn('printf started; sleep 30')");
    contexts.wait("term.text.includes('started')");
    value("term.kill('SIGTERM')");
    contexts.wait("events.some(event => event[0] === 'exit')");
    let error = contexts
        .page
        .eval("create({ command: ['/no/such/program'] })", "terminal-test")
        .unwrap_err()
        .to_string();
    assert!(error.contains("Cannot start the program"), "{error}");
}

#[test]
fn sessions_need_the_terminal_feature() {
    let contexts = contexts("[]");
    let error = contexts
        .page
        .eval("create()", "terminal-test")
        .unwrap_err()
        .to_string();
    assert!(
        error.contains(
            r#"The terminal feature is disabled. Add "terminal" to "features" in package.json"#
        ),
        "{error}"
    );
    contexts.tick();
}
