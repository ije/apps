use ::wry::{WebContext, WebView, WebViewBuilder};
use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use raw_window_handle::RawWindowHandle;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    rc::Rc,
    sync::{Arc, Mutex},
};

#[cfg(target_os = "linux")]
#[path = "platform/linux.rs"]
mod linux;
#[cfg(target_os = "linux")]
use linux::Window;
#[cfg(target_os = "windows")]
#[path = "platform/windows.rs"]
mod windows;
#[cfg(target_os = "windows")]
use windows::Window;

#[derive(Default)]
struct Events(Mutex<VecDeque<Value>>);

impl Events {
    fn push(&self, mut event: Value) {
        if event.to_string().len() > 4 * 1024 * 1024 {
            if event["type"] != "evaluation" {
                return;
            }
            event = json!({"type":"evaluation", "id":event["id"], "error":"Browser result exceeds 4 MiB"});
        }
        let mut queue = self.0.lock().unwrap();
        if queue.len() == 4096 {
            queue.pop_front();
        }
        queue.push_back(event);
    }

    fn evaluation(&self, token: &str, waiting: &RefCell<HashSet<u64>>, mut data: Value) {
        if data["token"] == token
            && data["type"] == "evaluation"
            && data["id"]
                .as_u64()
                .is_some_and(|id| waiting.borrow_mut().remove(&id))
        {
            if let Some(object) = data.as_object_mut() {
                object.remove("token");
            }
            self.push(data);
        }
    }

    fn complete(&self, id: &Value, result: Result<Value>) {
        self.push(match result {
            Ok(value) => json!({"type":"evaluation", "id":id, "value":value}),
            Err(error) => json!({"type":"evaluation", "id":id, "error":error.to_string()}),
        });
    }

    fn capture(&self, arguments: &Value, png: Result<Vec<u8>>, scale: f64) {
        self.complete(&arguments["id"], (|| {
            let mut image = image::load_from_memory(&png?)?;
            if let Some(rect) = arguments.get("rect") {
                let x = rect["x"].as_f64().unwrap() * scale;
                let y = rect["y"].as_f64().unwrap() * scale;
                let width = rect["width"].as_f64().unwrap() * scale;
                let height = rect["height"].as_f64().unwrap() * scale;
                ensure!(x >= 0. && y >= 0. && x + width <= image.width() as f64 && y + height <= image.height() as f64, "Capture bounds exceed the browser viewport");
                ensure!(width >= 1. && height >= 1., "Capture bounds are smaller than a pixel");
                image = image.crop_imm(x as u32, y as u32, width as u32, height as u32);
            }
            let mut output = std::io::Cursor::new(Vec::new());
            image.write_to(&mut output, image::ImageFormat::Png)?;
            Ok(json!({"png":base64::engine::general_purpose::STANDARD.encode(output.into_inner()), "width":image.width(), "height":image.height()}))
        })());
    }
}

struct Session {
    context: RefCell<WebContext>,
    _temporary: Option<tempfile::TempDir>,
}

thread_local! {
    static SESSIONS: RefCell<HashMap<String, Rc<Session>>> = RefCell::default();
}

pub(super) struct Browser {
    view: WebView,
    window: Window,
    _session: Rc<Session>,
    events: Arc<Events>,
    token: String,
    waiting: Rc<RefCell<HashSet<u64>>>,
}

impl Browser {
    pub fn new(parent: Option<RawWindowHandle>, options: &Value) -> Result<Self> {
        let events = Arc::new(Events::default());
        let window = Window::new(parent, options, events.clone())?;
        let partition = options["partition"]
            .as_str()
            .unwrap_or(if options["persistent"] == true {
                "persist:default"
            } else {
                ""
            });
        let application = options["application"]
            .as_str()
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .unwrap_or(std::env::current_exe()?.to_string_lossy().into_owned());
        let session_key = json!([application, partition]).to_string();
        let session = SESSIONS.with(|sessions| -> Result<Rc<Session>> {
            let mut sessions = sessions.borrow_mut();
            if !partition.is_empty()
                && let Some(session) = sessions.get(&session_key)
            {
                return Ok(session.clone());
            }
            let temporary = if partition.starts_with("persist:") {
                None
            } else {
                Some(tempfile::Builder::new().prefix("apps-webview-").tempdir()?)
            };
            let directory = if let Some(temporary) = &temporary {
                temporary.path().to_owned()
            } else {
                #[cfg(target_os = "windows")]
                let root = std::env::var_os("LOCALAPPDATA")
                    .map(std::path::PathBuf::from)
                    .context("LOCALAPPDATA is unavailable")?;
                #[cfg(target_os = "linux")]
                let root = std::env::var_os("XDG_DATA_HOME")
                    .map(std::path::PathBuf::from)
                    .or_else(|| {
                        std::env::var_os("HOME")
                            .map(|home| std::path::PathBuf::from(home).join(".local/share"))
                    })
                    .context("User data directory is unavailable")?;
                root.join("apps/webview")
                    .join(format!("{:x}", Sha256::digest(session_key.as_bytes())))
            };
            std::fs::create_dir_all(&directory)?;
            let session = Rc::new(Session {
                context: RefCell::new(WebContext::new(Some(directory))),
                _temporary: temporary,
            });
            if !partition.is_empty() {
                sessions.insert(session_key, session.clone());
            }
            Ok(session)
        })?;
        let mut random = [0; 16];
        getrandom::fill(&mut random)
            .map_err(|error| anyhow::anyhow!("Browser randomness failed: {error}"))?;
        let token = base64::engine::general_purpose::STANDARD.encode(random);
        let waiting = Rc::new(RefCell::new(HashSet::new()));
        let ipc_waiting = waiting.clone();
        let ipc_events = events.clone();
        let ipc_token = token.clone();
        let popup_events = events.clone();
        let mut context = session.context.borrow_mut();
        let mut builder = WebViewBuilder::new_with_web_context(&mut context)
            .with_devtools(options["inspectable"] == true)
            .with_initialization_script_for_main_only(include_str!("../js/bridge.js"), false)
            .with_ipc_handler(move |request| {
                if request.body().len() > 4 * 1024 * 1024 { return; }
                let Ok(data) = serde_json::from_str::<Value>(request.body()) else { return; };
                if data["type"] == "evaluation" { ipc_events.evaluation(&ipc_token, &ipc_waiting, data); }
                else if data["message"].is_object() {
                    ipc_events.push(json!({"type":"message", "data":data["message"], "url":data["url"], "mainFrame":data["mainFrame"]}));
                }
            })
            .with_new_window_req_handler(move |url, _| {
                popup_events.push(json!({"type":"new-window", "url":url}));
                ::wry::NewWindowResponse::Deny
            });
        if options["javascript"] == false {
            builder = builder.with_javascript_disabled();
        }
        let view = window.build(builder, events.clone())?;
        drop(context);
        Ok(Self {
            view,
            window,
            _session: session,
            events,
            token,
            waiting,
        })
    }

    pub fn attach(&self, parent: RawWindowHandle) -> Result<()> {
        self.window.attach(parent)
    }
    pub fn set_bounds(&self, bounds: [f64; 4]) -> Result<()> {
        self.window.set_bounds(bounds)
    }
    pub fn detach(&self) -> Result<()> {
        self.window.detach()
    }

    pub fn command(&self, operation: &str, arguments: &Value) -> Result<Value> {
        match operation {
            "visible" => self.window.set_visible(arguments.as_bool().unwrap())?,
            "loadURL" => {
                let url = arguments.as_str().unwrap();
                ensure!(
                    matches!(
                        url::Url::parse(url)?.scheme(),
                        "http" | "https" | "file" | "about" | "data"
                    ),
                    "Unsupported browser URL scheme"
                );
                self.view.load_url(url)?;
            }
            "evaluate" => {
                let id = arguments["id"].as_u64().context("Expected evaluation ID")?;
                ensure!(
                    self.waiting.borrow_mut().insert(id),
                    "Evaluation ID is already pending"
                );
                let result = self.view.evaluate_script(&format!(r#"(()=>{{const send=result=>{{let text;try{{text=JSON.stringify({{type:'evaluation',id:{id},token:{token},...result}});if(new TextEncoder().encode(text).length>4*1024*1024)throw Error('Browser result exceeds 4 MiB')}}catch(error){{text=JSON.stringify({{type:'evaluation',id:{id},token:{token},error:String(error.message||error)}})}}window.ipc.postMessage(text)}};Promise.resolve().then(()=>(0,eval)({script})).then(value=>send({{value:value===undefined?null:value}}),error=>send({{error:String(error?.message||error)}}))}})()"#, id=arguments["id"], token=json!(self.token), script=arguments["script"]));
                if result.is_err() {
                    self.waiting.borrow_mut().remove(&id);
                }
                result?;
            }
            "cookies.get" | "cookies.remove" => {
                let filter = &arguments["filter"];
                let cookies = if let Some(url) = filter["url"].as_str() {
                    ensure!(
                        matches!(url::Url::parse(url)?.scheme(), "http" | "https"),
                        "Cookies require an HTTP URL"
                    );
                    self.view.cookies_for_url(url)?
                } else {
                    self.view.cookies()?
                };
                let mut matches = Vec::new();
                for cookie in cookies {
                    if filter["name"]
                        .as_str()
                        .is_some_and(|name| name != cookie.name())
                        || filter["domain"].as_str().is_some_and(|domain| {
                            Some(domain.trim_start_matches('.'))
                                != cookie.domain().map(|d| d.trim_start_matches('.'))
                        })
                        || filter["path"]
                            .as_str()
                            .is_some_and(|path| Some(path) != cookie.path())
                    {
                        continue;
                    }
                    if operation == "cookies.remove" {
                        self.view.delete_cookie(&cookie)?;
                    } else {
                        matches.push(json!({"name":cookie.name(),"value":cookie.value(),"domain":cookie.domain(),"path":cookie.path(),"secure":cookie.secure().unwrap_or(false),"httpOnly":cookie.http_only().unwrap_or(false),"session":cookie.expires_datetime().is_none(),"expirationDate":cookie.expires_datetime().map(|date|date.unix_timestamp())}));
                    }
                }
                self.events.complete(
                    &arguments["id"],
                    Ok(if operation == "cookies.get" {
                        json!(matches)
                    } else {
                        Value::Null
                    }),
                );
            }
            "cookies.set" => {
                let details = &arguments["details"];
                let url = url::Url::parse(details["url"].as_str().unwrap())?;
                let mut cookie = wry::cookie::Cookie::new(
                    details["name"].as_str().unwrap().to_owned(),
                    details["value"].as_str().unwrap().to_owned(),
                );
                cookie.set_domain(
                    details["domain"]
                        .as_str()
                        .unwrap_or(url.host_str().unwrap())
                        .to_owned(),
                );
                cookie.set_path(details["path"].as_str().unwrap_or("/").to_owned());
                cookie.set_secure(details["secure"].as_bool().unwrap_or(false));
                cookie.set_http_only(details["httpOnly"].as_bool().unwrap_or(false));
                if let Some(time) = details["expirationDate"].as_f64() {
                    cookie.set_expires(wry::cookie::time::OffsetDateTime::from_unix_timestamp(
                        time as i64,
                    )?);
                }
                self.view.set_cookie(&cookie)?;
                self.events.complete(&arguments["id"], Ok(Value::Null));
            }
            "focus" => self.view.focus()?,
            "zoom" => self.view.zoom(arguments.as_f64().unwrap())?,
            _ => {
                return self
                    .window
                    .command(&self.view, operation, arguments, &self.events);
            }
        }
        Ok(Value::Null)
    }

    pub fn poll(&self) -> Vec<Value> {
        #[cfg(target_os = "linux")]
        for _ in 0..64 {
            if !gtk::events_pending() {
                break;
            }
            gtk::main_iteration_do(false);
        }
        self.events.0.lock().unwrap().drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use raw_window_handle::HasWindowHandle;
    use std::time::{Duration, Instant};

    fn wait(browser: &Browser, matches: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            #[cfg(target_os = "windows")]
            unsafe {
                use ::windows::Win32::UI::WindowsAndMessaging::*;
                let mut message = MSG::default();
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            for event in browser.poll() {
                if matches(&event) {
                    return event;
                }
            }
            assert!(
                Instant::now() < deadline,
                "Timed out waiting for a browser event"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn evaluation_results_require_a_pending_id_and_are_delivered_once() {
        let events = Events::default();
        let waiting = RefCell::new(HashSet::from([42]));
        events.evaluation(
            "secret",
            &waiting,
            json!({"type":"evaluation","token":"secret","id":999,"value":"forged"}),
        );
        events.evaluation(
            "secret",
            &waiting,
            json!({"type":"evaluation","token":"wrong","id":42,"value":"forged"}),
        );
        assert!(events.0.lock().unwrap().is_empty());
        events.evaluation(
            "secret",
            &waiting,
            json!({"type":"evaluation","token":"secret","id":42,"value":"ok"}),
        );
        events.evaluation(
            "secret",
            &waiting,
            json!({"type":"evaluation","token":"secret","id":42,"value":"duplicate"}),
        );
        let events = events.0.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["value"], "ok");
        assert!(waiting.borrow().is_empty());
    }

    #[test]
    #[ignore = "requires a desktop display and the installed WebKitGTK or WebView2 runtime"]
    fn system_webview() {
        let parent = Window::new(None, &json!({"show":true}), Arc::default()).unwrap();
        let browser =
            Browser::new(None, &json!({"show":false,"partition":"browser-smoke"})).unwrap();
        browser.set_bounds([10., 20., 320., 240.]).unwrap();
        browser
            .attach(parent.window_handle().unwrap().as_raw())
            .unwrap();
        browser.command("loadHTML", &json!({"html":"<!doctype html><title>System browser</title><h1 id='value'>你好</h1>","baseURL":"https://example.test/base/"})).unwrap();
        let loaded = wait(&browser, |event| event["type"] == "did-finish-load");
        assert_eq!(loaded["title"], "System browser");
        browser.command("evaluate", &json!({"id":1,"script":"Promise.resolve(document.getElementById('value').textContent + ':' + (6*7) + ':' + typeof process + ':' + document.baseURI)"})).unwrap();
        assert_eq!(
            wait(&browser, |event| event["id"] == 1)["value"],
            "你好:42:undefined:https://example.test/base/"
        );
        browser
            .command(
                "evaluate",
                &json!({"id":2,"script":"Promise.reject(new Error('expected failure'))"}),
            )
            .unwrap();
        assert_eq!(
            wait(&browser, |event| event["id"] == 2)["error"],
            "expected failure"
        );
        browser
            .command(
                "evaluate",
                &json!({"id":3,"script":"'界'.repeat(2*1024*1024)"}),
            )
            .unwrap();
        assert_eq!(
            wait(&browser, |event| event["id"] == 3)["error"],
            "Browser result exceeds 4 MiB"
        );
        browser
            .command(
                "evaluate",
                &json!({"id":4,"script":"apps.postMessage({text:'hello'})"}),
            )
            .unwrap();
        assert_eq!(
            wait(&browser, |event| event["type"] == "message"
                && event["data"]["kind"] == "message")["data"]["data"]["text"],
            "hello"
        );
        browser.command("cookies.set", &json!({"id":5,"details":{"url":"https://example.test/","name":"test","value":"shared","httpOnly":true,"secure":true}})).unwrap();
        assert!(wait(&browser, |event| event["id"] == 5)["error"].is_null());
        let shared =
            Browser::new(None, &json!({"show":false,"partition":"browser-smoke"})).unwrap();
        shared
            .command(
                "cookies.get",
                &json!({"id":6,"filter":{"url":"https://example.test/"}}),
            )
            .unwrap();
        let cookies = wait(&shared, |event| event["id"] == 6);
        assert_eq!(cookies["value"][0]["value"], "shared");
        assert_eq!(cookies["value"][0]["httpOnly"], true);
        let separate = Browser::new(None, &json!({"show":false})).unwrap();
        separate
            .command("cookies.get", &json!({"id":7,"filter":{}}))
            .unwrap();
        assert_eq!(
            wait(&separate, |event| event["id"] == 7)["value"],
            json!([])
        );
        browser
            .command(
                "cookies.remove",
                &json!({"id":8,"filter":{"url":"https://example.test/","name":"test"}}),
            )
            .unwrap();
        assert!(wait(&browser, |event| event["id"] == 8)["error"].is_null());
        shared
            .command("cookies.get", &json!({"id":9,"filter":{}}))
            .unwrap();
        assert_eq!(wait(&shared, |event| event["id"] == 9)["value"], json!([]));
        browser
            .command(
                "capture",
                &json!({"id":10,"rect":{"x":0,"y":0,"width":80,"height":60}}),
            )
            .unwrap();
        let capture = wait(&browser, |event| event["id"] == 10);
        assert!(capture["error"].is_null(), "{capture}");
        assert!(capture["value"]["width"].as_u64().unwrap() >= 80);
        assert!(capture["value"]["height"].as_u64().unwrap() >= 60);
        assert!(
            capture["value"]["png"]
                .as_str()
                .unwrap()
                .starts_with("iVBOR")
        );
        browser.command("clearStorage", &json!({"id":11})).unwrap();
        assert!(wait(&browser, |event| event["id"] == 11)["error"].is_null());
        browser.set_bounds([0., 0., 0., 0.]).unwrap();
        browser.set_bounds([10., 20., 320., 240.]).unwrap();
        browser
            .attach(parent.window_handle().unwrap().as_raw())
            .unwrap();
        browser
            .command(
                "evaluate",
                &json!({"id":12,"script":"document.getElementById('value').textContent"}),
            )
            .unwrap();
        assert_eq!(wait(&browser, |event| event["id"] == 12)["value"], "你好");
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", port.local_addr().unwrap());
        drop(port);
        browser.command("loadURL", &json!(url)).unwrap();
        assert!(wait(&browser, |event| event["type"] == "did-fail-load")["error"].is_string());
    }
}
