use crate::{browser::Browser, host::Host, jsc::Engine};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

pub struct DevTools {
    browser: Browser,
    ready: Cell<bool>,
    closed: Cell<bool>,
    next: Cell<u64>,
    events: RefCell<Vec<Value>>,
}

impl DevTools {
    pub fn new(title: &str) -> Result<Self> {
        let browser = Browser::detached(
            &json!({"title":format!("Developer Tools — {title}"),"inspectable":true}),
        )?;
        browser.load_html(include_str!("../assets/devtools.html"), "about:blank")?;
        Ok(Self {
            browser,
            ready: Cell::new(false),
            closed: Cell::new(false),
            next: Cell::new(0),
            events: RefCell::default(),
        })
    }

    fn evaluate(&self, source: &str) -> Result<()> {
        let id = self.next.get() + 1;
        self.next.set(id);
        self.browser.evaluate(id, source)
    }

    pub fn commands(&self) -> Vec<Value> {
        self.browser
            .poll()
            .into_iter()
            .filter_map(|event| {
                if event["type"] == "closed" {
                    self.closed.set(true);
                    return None;
                }
                if event["type"] != "message" || event["data"]["kind"] != "message" {
                    return None;
                }
                let command = event["data"]["data"].clone();
                if command["method"] == "ready" {
                    self.ready.set(true);
                    let events = std::mem::take(&mut *self.events.borrow_mut());
                    if !events.is_empty() {
                        let _ = self.evaluate(&format!(
                            "for(const event of {})window.appsDevTools.receive(event)",
                            json!(events)
                        ));
                    }
                    return None;
                }
                Some(command)
            })
            .collect()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.get()
    }

    pub fn respond(&self, id: u64, value: Value, error: Option<&str>) -> Result<()> {
        self.evaluate(&format!(
            "window.appsDevTools?.receive({})",
            json!({"id":id,"value":value,"error":error})
        ))
    }

    pub fn events(&self, values: &[Value]) -> Result<()> {
        let batch: Vec<_> = values.iter().map(|event| if event["method"] == "Runtime.consoleAPICalled" {
            json!({"type":"console","level":event["params"]["type"],"args":[event["params"]["text"]]})
        } else { event.clone() }).collect();
        if !self.ready.get() {
            let mut events = self.events.borrow_mut();
            let count = 500usize.saturating_sub(events.len());
            events.extend(batch.into_iter().take(count));
            return Ok(());
        }
        if !batch.is_empty() {
            self.evaluate(&format!(
                "for(const event of {})window.appsDevTools?.receive(event)",
                json!(batch)
            ))?;
        }
        Ok(())
    }
}

pub fn dispatch(
    command: &Value,
    engine: &Engine,
    host: &Rc<RefCell<Host>>,
    window: u64,
) -> Result<Value> {
    anyhow::ensure!(
        !host.borrow().packaged,
        "Developer tools are unavailable in packaged apps"
    );
    let params = &command["params"];
    match command["method"]
        .as_str()
        .context("Expected developer tools method")?
    {
        "Runtime.evaluate" => {
            let expression = params["expression"]
                .as_str()
                .context("Expected JavaScript expression")?;
            let output = engine.eval(&format!("(()=>{{const result=(0,eval)({});try{{return JSON.stringify(result,(_,v)=>typeof v==='bigint'?String(v)+'n':v)}}catch{{return JSON.stringify(String(result))}}}})()", json!(expression)), "apps:devtools")?;
            Ok(serde_json::from_str(&output).unwrap_or(Value::String(output)))
        }
        "DOM.getDocument" => {
            let host = host.borrow();
            let page = host
                .pages
                .get(&window)
                .context("Window has no GPUI document")?;
            Ok(page.doc.serialize(0).into())
        }
        "DOM.querySelector" => {
            let selector = params["selector"]
                .as_str()
                .context("Expected CSS selector")?;
            let host = host.borrow();
            let page = host
                .pages
                .get(&window)
                .context("Window has no GPUI document")?;
            let mut pending = vec![0];
            while let Some(node) = pending.pop() {
                if page.doc.matches(node, selector) {
                    return Ok(page.doc.serialize(node).into());
                }
                pending.extend(page.doc.nodes[node].children.iter().rev().copied());
            }
            Ok(Value::Null)
        }
        "Sources.list" => {
            let mut sources: Vec<_> = engine.sources().into_iter().collect();
            sources.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            Ok(Value::Array(
                sources
                    .into_iter()
                    .map(|(name, source)| json!({"name":name,"source":source}))
                    .collect(),
            ))
        }
        "Debugger.pause" => {
            engine.eval("debugger;", "apps:devtools")?;
            Ok("Debugger statement requested. Execution pauses only when a Web Inspector debugger is attached.".into())
        }
        "Debugger.open" => bail!(
            "Connect to this apps JavaScript context from Safari → Develop → this application. WebKit browser views can open their inspector directly."
        ),
        method => bail!("Unsupported developer tools method: {method}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        host::{Page, Role},
        html::Document,
        jsc::Api,
    };

    #[test]
    fn packaged_apps_disable_developer_tools() {
        let host = Rc::new(RefCell::new(Host {
            packaged: true,
            ..Host::default()
        }));
        let engine = Engine::new(Api::load().unwrap(), host.clone(), Role::Main).unwrap();
        engine.eval("globalThis.answer = 42", "app.js").unwrap();
        assert!(engine.sources().is_empty());
        assert!(dispatch(&json!({"method":"Sources.list"}), &engine, &host, 0).is_err());
        assert!(
            dispatch(
                &json!({"method":"Runtime.evaluate","params":{"expression":"answer = 0"}}),
                &engine,
                &host,
                0
            )
            .is_err()
        );
        assert_eq!(engine.eval("answer", "test").unwrap(), "42");
        host.borrow_mut()
            .call(Role::Main, json!({"op":"window","args":["devtools",1]}))
            .unwrap();
        assert!(host.borrow().commands.is_empty());
        assert_eq!(
            engine.eval("typeof __appsCompile", "test").unwrap(),
            "undefined"
        );
        assert_eq!(
            engine
                .eval(
                    "__appsEvent('ready'); void new __apps.BrowserWindow().webContents.openDevTools()",
                    "test"
                )
                .unwrap(),
            "undefined"
        );
    }

    #[test]
    fn evaluation_updates_live_document_and_reports_errors() {
        let host = Rc::new(RefCell::new(Host::default()));
        host.borrow_mut().pages.insert(
            1,
            Page {
                doc: Document::parse("<body><p id='value'>before</p></body>"),
                dirty: false,
            },
        );
        let engine = Engine::new(Api::load().unwrap(), host.clone(), Role::Renderer(1)).unwrap();
        let command = json!({"method":"Runtime.evaluate","params":{"expression":"document.querySelector('#value').textContent = 'changed'"}});
        assert_eq!(dispatch(&command, &engine, &host, 1).unwrap(), "changed");
        let document = dispatch(
            &json!({"method":"DOM.querySelector","params":{"selector":"#value"}}),
            &engine,
            &host,
            1,
        )
        .unwrap();
        assert!(document.as_str().unwrap().contains("changed"));
        assert!(host.borrow().pages[&1].dirty);
        assert!(dispatch(&json!({"method":"Runtime.evaluate","params":{"expression":"throw new Error('expected')"}}), &engine, &host, 1).is_err());
        assert!(dispatch(&json!({"method":"Debugger.open"}), &engine, &host, 1).is_err());
    }
}
