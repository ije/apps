use crate::webview::Browser;
use crate::{host::Host, host::context::JsContext, jsc::Role};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
  cell::{Cell, RefCell},
  rc::Rc,
  sync::{
    OnceLock,
    atomic::{AtomicUsize, Ordering},
  },
  time::{Duration, Instant},
};

const POLL: Duration = Duration::from_millis(250);

// The open tools of pages. Main sends its logs to the pages only while some are open.
static PAGE_TOOLS: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn pages_open() -> bool {
  PAGE_TOOLS.load(Ordering::Relaxed) > 0
}

// The page of the panel. Its style is Tailwind, compiled for the classes that the page has.
fn page() -> Result<&'static str> {
  static PAGE: OnceLock<String> = OnceLock::new();
  if let Some(page) = PAGE.get() {
    return Ok(page);
  }
  let html = crate::native::devtools::text(false)?;
  let (head, rest) = html.split_once("<style>").unwrap_or((html, ""));
  let (css, body) = rest.split_once("</style>").unwrap_or_default();
  let css = crate::style::tailwind::page_css(css, body);
  Ok(PAGE.get_or_init(|| format!("{head}<style>{css}</style>{body}")))
}

// The panel of one window, at its left side. Where it cannot dock, and for the main process, it has its own
// window.
pub(crate) struct DevTools {
  browser: Browser,
  mode: &'static str,
  // The context that the tools look at. Only its console messages have values to expand.
  role: Role,
  titlebar: f64,
  ready: Cell<bool>,
  pub(super) closed: Cell<bool>,
  next: Cell<u64>,
  events: RefCell<Vec<Value>>,
  polled: Cell<Instant>,
  fresh: Cell<bool>,
}

impl DevTools {
  pub(crate) fn new(title: &str, role: Role, window: Option<&renderer::Window>) -> Result<Self> {
    let page = page()?;
    let docked = window.filter(|_| cfg!(target_os = "macos")).map(|window| {
      let browser = Browser::new(
        window,
        window.scale_factor() as f64,
        &json!({"inspectable":true}),
      )?;
      let titlebar = browser.dock(300.)?;
      anyhow::Ok((browser, titlebar))
    });
    let (browser, mode, titlebar) = match docked {
      Some(Ok((browser, titlebar))) => (browser, "docked", titlebar),
      _ => {
        let options = json!({"title":format!("Developer Tools — {title}"),"inspectable":true});
        let mode = if window.is_some() { "window" } else { "main" };
        (Browser::detached(&options)?, mode, 0.)
      }
    };
    browser.load_html(page, "about:blank")?;
    if role != Role::Main {
      PAGE_TOOLS.fetch_add(1, Ordering::Relaxed);
    }
    Ok(Self {
      browser,
      mode,
      role,
      titlebar,
      ready: Cell::new(false),
      closed: Cell::new(false),
      next: Cell::new(0),
      events: RefCell::default(),
      polled: Cell::new(Instant::now()),
      fresh: Cell::new(true),
    })
  }

  fn evaluate(&self, source: &str) -> Result<()> {
    let id = self.next.get() + 1;
    self.next.set(id);
    self.browser.evaluate(id, source)
  }

  pub(crate) fn commands(&self) -> Vec<Value> {
    self
      .browser
      .poll()
      .into_iter()
      .filter_map(|event| {
        if event["type"] == "closed" {
          self.closed.set(true);
        }
        if event["type"] == "message" && event["data"]["kind"] == "error" {
          eprintln!("apps: developer tools: {}", event["data"]);
        }
        if event["type"] != "message" || event["data"]["kind"] != "message" {
          return None;
        }
        let command = event["data"]["data"].clone();
        if command["method"] != "ready" {
          return Some(command);
        }
        self.ready.set(true);
        let mut events = vec![json!({"type":"init","mode":self.mode,"titlebar":self.titlebar})];
        events.append(&mut self.events.borrow_mut());
        let _ = self.evaluate(&format!(
          "for(const event of {})window.appsDevTools.receive(event)",
          json!(events)
        ));
        None
      })
      .collect()
  }

  pub(crate) fn is_closed(&self) -> bool {
    self.closed.get()
  }

  pub(crate) fn events(&self, values: &[Value]) -> Result<()> {
    let window = |context: &Value| context["renderer"].as_u64().or(context["preload"].as_u64());
    if values.is_empty() {
      return Ok(());
    }
    let own = json!(self.role);
    let batch: Vec<_> = values
      .iter()
      .filter_map(|event| {
        if event["method"] == "Runtime.consoleAPICalled" {
          let params = &event["params"];
          // Windows that share a thread share their logs: keep the ones of this window.
          let other = window(&params["context"]).is_some_and(|id| {
            id != crate::host::WORKER_PAGE && window(&own).is_some_and(|own| own != id)
          });
          let values = (params["context"] == own).then(|| &params["ref"]);
          (!other).then(
            || json!({"type":"console","level":params["type"],"text":params["text"],"ref":values}),
          )
        } else {
          Some(event.clone())
        }
      })
      .collect();
    if !self.ready.get() {
      // Keep the newest events: the errors just before the panel opened matter most.
      let mut events = self.events.borrow_mut();
      events.extend(batch);
      let extra = events.len().saturating_sub(500);
      events.drain(..extra);
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

  pub(crate) fn answer(
    &self,
    command: &Value,
    engine: &JsContext,
    host: &Rc<RefCell<Host>>,
    page: Option<&crate::render::ViewState>,
  ) -> Result<()> {
    let params = &command["params"];
    let result = match command["method"].as_str() {
      Some("Panel.close") => {
        self.closed.set(true);
        Ok(Value::Null)
      }
      Some("Panel.resize") if self.mode == "docked" => params["width"]
        .as_f64()
        .context("Expected a width")
        .and_then(|width| self.browser.dock(width.clamp(200., 800.)))
        .map(Value::from),
      Some("Hud.toggle") => {
        if let Some(page) = page {
          host.borrow_mut().command(
            crate::host::link::Target::Window(page.id),
            json!({"op":"window","args":["hud",page.id]}),
          );
        }
        Ok(Value::Null)
      }
      Some("Inspect.set") => {
        if let Some(page) = page {
          let mut inspect = page.inspect.get();
          let node = |value: &Value| value.as_u64().map(|id| id as usize);
          if let Some(pick) = params["pick"].as_bool() {
            inspect.pick = pick;
          }
          if let Some(hover) = params.get("hover") {
            inspect.hover = node(hover);
          }
          if let Some(selected) = params.get("selected") {
            inspect.selected = node(selected);
          }
          page.inspect.set(inspect);
          if params.get("hover").is_none() {
            self.inspected(page)?;
          }
        }
        Ok(Value::Null)
      }
      _ => dispatch(command, engine, host),
    };
    let (value, error) = match result {
      Ok(value) => (value, None),
      Err(error) => (Value::Null, Some(format!("{error:#}"))),
    };
    self.evaluate(&format!(
      "window.appsDevTools?.receive({})",
      json!({"id":command["id"],"value":value,"error":error})
    ))
  }

  // Tells the panel which node is selected and if the picker and the HUD are on.
  pub(crate) fn inspected(&self, page: &crate::render::ViewState) -> Result<()> {
    let (inspect, hud) = (page.inspect.get(), page.hud.borrow().is_some());
    self.events(&[
      json!({"type":"inspect","pick":inspect.pick,"selected":inspect.selected,"hud":hud}),
    ])
  }

  // Tells the panel when the document changed or a new page loaded, at most four times a second. Returns the
  // milliseconds until the next check.
  pub(crate) fn poll(&self, engine: &JsContext) -> Result<i64> {
    let wait = POLL.saturating_sub(self.polled.get().elapsed());
    if !wait.is_zero() {
      return Ok(wait.as_micros().div_ceil(1000) as i64);
    }
    self.polled.set(Instant::now());
    let state = agent(engine, "poll", &Value::Null)?.as_u64();
    // A new panel reads the page by itself.
    if !self.fresh.replace(false) {
      match state {
        Some(2) => self.events(&[json!({"type":"reset"})])?,
        Some(1) => self.events(&[json!({"type":"dom"})])?,
        _ => {}
      }
    }
    Ok(POLL.as_millis() as i64)
  }
}

impl Drop for DevTools {
  fn drop(&mut self) {
    if self.role != Role::Main {
      PAGE_TOOLS.fetch_sub(1, Ordering::Relaxed);
    }
  }
}

impl super::Desktop {
  // The developer tools of window `id`, its FPS HUD, and the picker.
  pub(super) fn tools(
    &mut self,
    action: &str,
    id: u64,
    args: &Value,
    mut cx: Option<&mut renderer::App>,
  ) -> Result<()> {
    match action {
      "devtools" => {
        if self.host.borrow().app.packaged {
          return Ok(());
        }
        let cx = cx
          .as_deref_mut()
          .context("Developer Tools require a native window")?;
        let view = self.views.get(&id);
        // The console or the picker opens the tools, and keeps open tools.
        let show = args[2].as_str();
        let fresh = !self.devtools.contains_key(&id);
        let open = fresh || show.is_some();
        if fresh {
          let title = format!("Window {id}");
          let tools = view
            .and_then(|view| view.handle)
            .and_then(|handle| {
              cx.update_window(handle, |window, _| {
                DevTools::new(&title, Role::Renderer(id), Some(window))
              })
            })
            .unwrap_or_else(|| DevTools::new(&title, Role::Renderer(id), None))?;
          self.devtools.insert(id, tools);
        } else if !open {
          self.devtools.remove(&id);
          if let Some(engine) = view.and_then(|view| view.renderer.as_deref())
            && let Err(error) = release(engine)
          {
            eprintln!("apps: developer tools: {error:#}");
          }
        }
        // Refresh first: a failed message to the panel below must not keep the old picker on screen.
        if let Some(handle) = view.and_then(|view| view.handle) {
          cx.refresh_window(handle);
        }
        if let Some(data) = view.and_then(|view| view.data.upgrade()) {
          let mut inspect = data.inspect.get();
          if fresh || !open {
            inspect = crate::render::Inspect {
              open,
              ..Default::default()
            };
          }
          if show == Some("pick") {
            inspect.pick = true;
          }
          data.inspect.set(inspect);
          if let Some(tools) = self.devtools.get(&id) {
            tools.inspected(&data)?;
            if show == Some("console") {
              tools.events(&[serde_json::json!({"type":"show"})])?;
            }
          }
        }
      }
      // The FPS HUD of the window. Open developer tools show if it is on.
      "hud" => {
        if self.host.borrow().app.packaged {
          return Ok(());
        }
        let (Some(view), Some(cx)) = (self.views.get(&id), cx.as_deref_mut()) else {
          return Ok(());
        };
        let (Some(data), Some(handle)) = (view.data.upgrade(), view.handle) else {
          return Ok(());
        };
        cx.update_window(handle, |window, _| {
          crate::render::Hud::toggle(&data, window)
        });
        if let Some(tools) = self.devtools.get(&id) {
          tools.inspected(&data)?;
        }
      }
      // The picker of the developer tools: the pointer moved to a place on the page, or
      // pressed there. Without a place it turns the picker on or off.
      "inspect" => {
        let (Some(tools), Some(view)) = (self.devtools.get(&id), self.views.get(&id)) else {
          return Ok(());
        };
        let (Some(data), Some(engine), Some(cx)) = (view.data.upgrade(), &view.renderer, cx) else {
          return Ok(());
        };
        let mut inspect = data.inspect.get();
        if args[2].is_number() {
          let at = serde_json::json!({"x":args[2],"y":args[3]});
          let node = super::view::scripted(cx, None, || agent(engine, "pick", &at))?
            .as_u64()
            .map(|node| node as usize);
          if args[4] == true {
            inspect.selected = node;
          } else if inspect.hover == node {
            return Ok(());
          }
          inspect.hover = node;
        } else {
          inspect.pick = args[2].as_bool().unwrap_or(!inspect.pick);
          inspect.hover = None;
        }
        data.inspect.set(inspect);
        if let Some(handle) = view.handle {
          cx.refresh_window(handle);
        }
        if !args[2].is_number() || args[4] == true {
          tools.inspected(&data)?;
        }
      }
      _ => {}
    }
    Ok(())
  }
}

// Calls the script that inspects a context from inside it. A new context gets the script on its first call.
pub(crate) fn agent(engine: &JsContext, method: &str, params: &Value) -> Result<Value> {
  if engine.eval_page("typeof this.__appsAgent", "apps:devtools")? != "function" {
    let source = format!("this.__appsAgent={}", crate::native::devtools::text(true)?);
    engine.eval_page(&source, "apps:devtools")?;
  }
  let call = format!(
    "JSON.stringify(this.__appsAgent({},{})??null)",
    json!(method),
    params
  );
  Ok(serde_json::from_str(
    &engine.eval_page(&call, "apps:devtools")?,
  )?)
}

// Takes the agent out of the page when its tools close, with its observer and the logged values.
pub(crate) fn release(engine: &JsContext) -> Result<()> {
  let script = "this.__appsAgent?.('close');delete this.__appsAgent";
  engine.eval_page(script, "apps:devtools").map(drop)
}

fn dispatch(command: &Value, engine: &JsContext, host: &Rc<RefCell<Host>>) -> Result<Value> {
  ensure!(
    !host.borrow().app.packaged,
    "Developer tools are unavailable in packaged apps"
  );
  let params = &command["params"];
  match command["method"]
    .as_str()
    .context("Expected developer tools method")?
  {
    // Answers the value of an expression, or the error that it threw, as the console shows them.
    "Runtime.evaluate" => {
      let expression = params["expression"]
        .as_str()
        .context("Expected JavaScript expression")?;
      agent(engine, "inspect", &Value::Null)?;
      let output = engine.eval_page(&format!("(()=>{{const agent=this.__appsAgent;try{{return JSON.stringify({{value:agent('inspect',{{value:this.__appsCompile({},'apps:console')}})}})}}catch(error){{return JSON.stringify({{thrown:agent('inspect',{{value:error}})}})}}}})()", json!(expression)), "apps:devtools")?;
      Ok(serde_json::from_str(&output)?)
    }
    method => match method
      .strip_prefix("DOM.")
      .or_else(|| method.strip_prefix("Console."))
    {
      Some(name) => agent(engine, name, params),
      None => bail!("Unsupported developer tools method: {method}"),
    },
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::{dom::Document, jsc::JavaScriptCore, render::Page};

  #[test]
  fn packaged_apps_disable_developer_tools() {
    let mut host = Host::default();
    host.update_app(|app| app.packaged = true);
    let host = Rc::new(RefCell::new(host));
    let engine = JsContext::new(JavaScriptCore::load().unwrap(), host.clone(), Role::Main).unwrap();
    engine.eval("globalThis.answer = 42", "app.js").unwrap();
    assert!(dispatch(&json!({"method":"Console.clear"}), &engine, &host).is_err());
    assert!(
      dispatch(
        &json!({"method":"Runtime.evaluate","params":{"expression":"answer = 0"}}),
        &engine,
        &host
      )
      .is_err()
    );
    assert_eq!(engine.eval("answer", "test").unwrap(), "42");
    host
      .borrow_mut()
      .call(Role::Main, "window", json!(["devtools", 1]))
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
    host.borrow().store.borrow_mut().pages.insert(
      1,
      Page {
        doc: Document::parse("<body><p id='value'>before</p></body>"),
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
    let command = json!({"method":"Runtime.evaluate","params":{"expression":"document.querySelector('#value').textContent = 'changed'"}});
    assert_eq!(
      dispatch(&command, &engine, &host).unwrap(),
      json!({"value":{"type":"string","text":"changed"}})
    );
    assert!(host.borrow().store.borrow().pages[&1].dirty);
    let call = |method: &str, params: Value| {
      dispatch(&json!({"method":method,"params":params}), &engine, &host).unwrap()
    };
    let thrown = call(
      "Runtime.evaluate",
      json!({"expression":"throw new TypeError('expected')"}),
    );
    assert_eq!(thrown["thrown"]["text"], "TypeError: expected");
    assert!(thrown["thrown"]["stack"].is_string());
    let object = call(
      "Runtime.evaluate",
      json!({"expression":"({a: 1, list: [1, 'x'], get b() { return 2 }})"}),
    );
    assert_eq!(object["value"]["text"], "{a: 1, list: Array(2), b: (…)}");
    let props = call("Console.props", json!({"id":object["value"]["id"]}));
    assert_eq!(props[0], json!({"name":"a","type":"number","text":"1"}));
    assert_eq!(props[1]["text"], "(2) [1, \"x\"]");
    assert_eq!(props[3]["name"], "[[Prototype]]");
    // Big values read only what the console shows.
    let typed = call(
      "Runtime.evaluate",
      json!({"expression":"new Float32Array(1e7)"}),
    );
    assert_eq!(
      typed["value"]["text"],
      "Float32Array(10000000) [0, 0, 0, 0, 0, …]"
    );
    let props = call("Console.props", json!({"id":typed["value"]["id"]}));
    assert_eq!(props.as_array().unwrap().len(), 501);
    let map = call(
      "Runtime.evaluate",
      json!({"expression":"new Map(Array.from({length: 1e5}, (_, at) => [at, at]))"}),
    );
    let props = call("Console.props", json!({"id":map["value"]["id"]}));
    assert_eq!(props.as_array().unwrap().len(), 201);
    // A logged object stays for the console to expand.
    engine
      .eval_page(
        "console.log('plain'); console.log('with', {deep: {er: 1}})",
        "page",
      )
      .unwrap();
    let logs: Vec<_> = host.borrow_mut().logs.drain(..).collect();
    assert_eq!(logs[0]["params"]["ref"], Value::Null);
    let args = call("Console.args", json!({"ref":logs[1]["params"]["ref"]}));
    assert_eq!(args[1]["text"], "{deep: {…}}");
    assert!(dispatch(&json!({"method":"Debugger.open"}), &engine, &host).is_err());
  }

  #[test]
  fn page_tools_read_the_tree_and_the_panel_has_its_styles() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow().store.borrow_mut().pages.insert(
            1,
            Page {
                doc: Document::parse("<head><title>t</title></head><body><p id='value' class='a b'>before</p> <script></script></body>"),
                dirty: false, ..Default::default()
            },
        );
    let engine = JsContext::new(
      JavaScriptCore::load().unwrap(),
      host.clone(),
      Role::Renderer(1),
    )
    .unwrap();
    let call = |method: &str, params: Value| {
      dispatch(&json!({"method":method,"params":params}), &engine, &host).unwrap()
    };
    assert_eq!(agent(&engine, "poll", &Value::Null).unwrap(), 2);
    let tree = call("DOM.tree", json!({"ids":[null]}));
    assert_eq!(
      (&tree["root"][0]["tag"], &tree["root"][0]["kids"]),
      (&json!("html"), &json!(1))
    );
    let html = tree["root"][0]["id"].clone();
    let body = call("DOM.tree", json!({"ids":[html]}))[html.to_string()][0]["id"].clone();
    let kids = call("DOM.tree", json!({"ids":[body]}))[body.to_string()].clone();
    assert_eq!(kids[0]["text"], "before");
    assert_eq!(kids.as_array().unwrap().len(), 1);
    assert_eq!(
      call("DOM.path", json!({"id":kids[0]["id"]})),
      json!([html, body, kids[0]["id"]])
    );
    engine
      .eval_page("document.querySelector('p').className = 'c'", "page")
      .unwrap();
    assert_eq!(agent(&engine, "poll", &Value::Null).unwrap(), 1);
    assert_eq!(agent(&engine, "poll", &Value::Null).unwrap(), 0);
    release(&engine).unwrap();
    let left = engine.eval_page("typeof this.__appsAgent + typeof __appsLogged", "page");
    assert_eq!(left.unwrap(), "undefinedundefined");
    release(&engine).unwrap();

    let page = page().unwrap();
    assert!(page.contains(".bg-panel{") && page.contains("box-sizing:border-box"));
    assert!(!page.contains("@import"));
  }
}
