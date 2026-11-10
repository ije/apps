use anyhow::{Context as _, Result, anyhow, bail, ensure};
use serde_json::{Value, json};
use std::{
    cell::{OnceCell, RefCell},
    collections::{BTreeMap, HashMap, VecDeque},
    ffi::{CString, c_char, c_void},
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    rc::Rc,
    sync::{
        Mutex, OnceLock, PoisonError,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const ABI: u32 = 1;

type Done = extern "C" fn(*mut c_void, bool, *const u8, usize);

#[repr(C)]
pub struct Host {
    connect: extern "C" fn(usize) -> i32,
    inspect: extern "C" fn(usize, *const u8, usize) -> bool,
    messages: extern "C" fn(*mut c_void, extern "C" fn(*mut c_void, usize, *const u8, usize)),
    contexts: extern "C" fn(*mut c_void, extern "C" fn(*mut c_void, usize, u32, u64)),
    wake: extern "C" fn(u64),
    call: extern "C" fn(*mut c_void, *const u8, usize, *const u8, usize, *mut c_void, Done),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Role {
    Main,
    Preload(u64),
    Renderer(u64),
}

type Reply = mpsc::Sender<Result<Value>>;
type Waiter = Box<dyn FnOnce(Result<Value>)>;

struct Call {
    tool: String,
    args: Value,
    reply: Reply,
}

struct Shared {
    paused: Mutex<Option<String>>,
    pauses: AtomicU64,
    page_paused: AtomicU64,
}

struct Script {
    url: String,
    map_url: String,
    map: OnceCell<Option<Rc<SourceMap>>>,
}

#[derive(Default)]
struct Session {
    scripts: HashMap<String, Script>,
    breakpoints: Vec<(String, String)>,
}

struct Profile {
    ctx: usize,
    started: Instant,
    cpu: f64,
}

#[derive(Default)]
struct State {
    next: u64,
    responses: HashMap<u64, Result<Value, String>>,
    waiting: HashMap<u64, Waiter>,
    sessions: HashMap<usize, Session>,
    paused: Option<(usize, Value)>,
    stepping: Option<Reply>,
    profile: Option<Profile>,
    samples: Option<Value>,
}

static HOST: OnceLock<&'static Host> = OnceLock::new();
static MAIN: OnceLock<mpsc::Sender<Call>> = OnceLock::new();
static PAGES: Mutex<BTreeMap<u64, mpsc::Sender<Call>>> = Mutex::new(BTreeMap::new());
static SHARED: Shared = Shared {
    paused: Mutex::new(None),
    pauses: AtomicU64::new(0),
    page_paused: AtomicU64::new(0),
};
static LOGS: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

thread_local! {
    static CHANNEL: OnceCell<(mpsc::Receiver<Call>, u64)> = const { OnceCell::new() };
    static MAIN_CHANNEL: OnceCell<mpsc::Receiver<Call>> = const { OnceCell::new() };
    static STATE: RefCell<State> = RefCell::default();
    static FRAMES: RefCell<Option<Vec<(u64, Duration)>>> = const { RefCell::new(None) };
}

const PAUSED: &str = "The app is paused in the debugger. Resume it with the debugger tool first";

// Pages can redefine globals, so this formatter uses only syntax and builtins it checks itself.
const SHOW: &str = r#"function(){const seen=new WeakSet(),plain=(v,d)=>{if(typeof v==='bigint')return v+'n';if(typeof v==='symbol'||v===undefined)return String(v);if(typeof v==='function')return`[Function ${v.name||'(anonymous)'}]`;if(v===null||typeof v!=='object')return v;if(typeof Node==='function'&&v instanceof Node)return String(v.outerHTML??v.textContent??'').slice(0,4000);if(v instanceof Error)return`${v.name}: ${v.message}`+(v.stack?'\n'+v.stack:'');if(v instanceof Date)return v.toISOString();if(seen.has(v))return'[Circular]';if(d>4)return Array.isArray(v)?`[Array(${v.length})]`:`[${v.constructor?.name??'Object'}]`;seen.add(v);if(v instanceof Map||v instanceof Set||Array.isArray(v)||ArrayBuffer.isView(v))return Array.from(v).slice(0,100).map(e=>plain(e,d+1));const o={};for(const k of Object.keys(v).slice(0,100)){try{o[k]=plain(v[k],d+1)}catch{o[k]='[Thrown]'}}return o},show=v=>{const p=plain(v,0),s=typeof p==='string'?p:JSON.stringify(p)??String(p);return s.length>120&&typeof p==='object'?JSON.stringify(p,null,1):s};return this instanceof Promise?this.then(show,e=>{throw show(e)}):show(this)}"#;

#[unsafe(no_mangle)]
pub extern "C" fn apps_mcp_abi() -> u32 {
    ABI
}

#[unsafe(no_mangle)]
pub extern "C" fn apps_mcp_start(port: u16, host: &'static Host) -> *mut c_char {
    let _ = HOST.set(host);
    let start = || -> Result<()> {
        let listener = TcpListener::bind(("127.0.0.1", port))
            .with_context(|| format!("Cannot start the devtools MCP server on port {port}"))?;
        let (main, calls) = mpsc::channel();
        MAIN_CHANNEL.with(|channel| channel.set(calls)).ok();
        let _ = MAIN.set(main);
        std::thread::Builder::new()
            .name("apps-mcp".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let _ = std::thread::Builder::new()
                        .name("apps-mcp-client".into())
                        .spawn(move || serve(stream));
                }
            })?;
        println!("apps: devtools MCP server at http://127.0.0.1:{port}/mcp");
        Ok(())
    };
    match start() {
        Ok(()) => std::ptr::null_mut(),
        Err(error) => CString::new(format!("{error:#}"))
            .unwrap_or_default()
            .into_raw(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn apps_mcp_install(key: u64) {
    let (sender, calls) = mpsc::channel();
    PAGES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key, sender);
    CHANNEL.with(|channel| channel.set((calls, key))).ok();
}

#[unsafe(no_mangle)]
pub extern "C" fn apps_mcp_uninstall() {
    if let Some(key) = CHANNEL.with(|channel| channel.get().map(|(_, key)| *key)) {
        PAGES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&key);
    }
}

fn serve(mut stream: TcpStream) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let local = |host: &str| {
        let host = host
            .rsplit_once(':')
            .filter(|(_, port)| port.bytes().all(|byte| byte.is_ascii_digit()));
        matches!(
            host.map_or("", |(host, _)| host),
            "127.0.0.1" | "localhost" | "[::1]"
        )
    };
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let mut parts = line.split_whitespace();
        let (method, path) = (
            parts.next().unwrap_or("").to_owned(),
            parts.next().unwrap_or("").to_owned(),
        );
        let (mut length, mut allowed, mut close, mut expect) = (0usize, true, false, false);
        loop {
            let mut header = String::new();
            reader.read_line(&mut header)?;
            let Some((name, value)) = header.trim_end().split_once(':') else {
                break;
            };
            let value = value.trim();
            match name.to_ascii_lowercase().as_str() {
                "content-length" => length = value.parse()?,
                // Web pages can reach localhost, so only local hosts and origins may call tools.
                "host" => allowed &= local(value),
                "origin" => {
                    allowed &= local(value.split_once("://").map_or(value, |(_, host)| host))
                }
                "connection" => close = value.eq_ignore_ascii_case("close"),
                "expect" => expect = value.eq_ignore_ascii_case("100-continue"),
                _ => {}
            }
        }
        ensure!(length <= 64 << 20, "Request is too large");
        if expect {
            stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body)?;
        let path = path.split('?').next().unwrap_or("");
        let tool = path.strip_prefix('/').filter(|tool| {
            tools()
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["name"] == *tool)
        });
        let body = if body.iter().all(u8::is_ascii_whitespace) {
            b"{}".to_vec()
        } else {
            body
        };
        let (status, kind, response) = match (allowed, method.as_str(), path, tool) {
            (false, ..) => ("403 Forbidden", "text/plain", b"Only local clients may connect".to_vec()),
            (_, "GET", "/", _) => ("200 OK", "text/plain; charset=utf-8", help().into_bytes()),
            (_, "POST", "/mcp", _) => match serde_json::from_slice::<Value>(&body) {
                Ok(request) => match rpc(&request) {
                    Some(response) => ("200 OK", "application/json", response.to_string().into_bytes()),
                    None => ("202 Accepted", "application/json", Vec::new()),
                },
                Err(error) => ("400 Bad Request", "application/json", json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":error.to_string()}}).to_string().into_bytes()),
            },
            (_, "POST", _, Some(tool)) => match serde_json::from_slice::<Value>(&body).map_err(anyhow::Error::from).and_then(|args| call(tool, args)) {
                Ok(Value::Object(image)) if image.contains_key("image") => (
                    "200 OK",
                    "image/png",
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, image["image"].as_str().unwrap_or(""))?,
                ),
                Ok(Value::String(text)) => ("200 OK", "text/plain; charset=utf-8", text.into_bytes()),
                Ok(value) => ("200 OK", "application/json", value.to_string().into_bytes()),
                Err(error) => ("400 Bad Request", "text/plain; charset=utf-8", format!("{error:#}").into_bytes()),
            },
            (_, "GET" | "POST", ..) => ("404 Not Found", "text/plain", b"Not found. GET / lists the tools".to_vec()),
            _ => ("405 Method Not Allowed", "text/plain", Vec::new()),
        };
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nAllow: GET, POST\r\n\r\n",
            response.len()
        )?;
        stream.write_all(&response)?;
        if close {
            return Ok(());
        }
    }
}

fn rpc(request: &Value) -> Option<Value> {
    let id = request.get("id")?.clone();
    let params = &request["params"];
    let result = match request["method"].as_str().unwrap_or("") {
        "initialize" => json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "apps-devtools", "version": env!("CARGO_PKG_VERSION")},
            "instructions": "Debug the running app. Tools without `window` target the main process; pass a window id from `windows` for a page. Lines and columns are 1-based and refer to source files.",
        }),
        "ping" => json!({}),
        "tools/list" => json!({"tools": tools()}),
        "tools/call" => match call(
            params["name"].as_str().unwrap_or(""),
            params["arguments"].clone(),
        ) {
            Ok(Value::String(text)) => json!({"content": [{"type": "text", "text": text}]}),
            Ok(Value::Object(image)) if image.contains_key("image") => json!({"content": [
                {"type": "image", "data": image["image"], "mimeType": "image/png"},
                {"type": "text", "text": image["text"]},
            ]}),
            Ok(value) => json!({"content": [{"type": "text", "text": value.to_string()}]}),
            Err(error) => {
                json!({"content": [{"type": "text", "text": format!("{error:#}")}], "isError": true})
            }
        },
        method => {
            return Some(
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":format!("Unknown method: {method}")}}),
            );
        }
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn call(tool: &str, args: Value) -> Result<Value> {
    let pages = PAGES.lock().unwrap_or_else(PoisonError::into_inner);
    if tool == "windows" {
        let keys: Vec<_> = pages.keys().copied().collect();
        drop(pages);
        let mut list = Vec::new();
        for key in keys {
            if let Ok(Value::Array(windows)) = dispatch(tool, args.clone(), Some(key)) {
                list.extend(windows);
            }
        }
        list.sort_by_key(|window| window["id"].as_u64());
        return Ok(list.into());
    }
    let paused = SHARED.page_paused.load(Ordering::Acquire);
    let key = if args["window"].is_null()
        && (tool == "evaluate" || tool == "debugger" || tool == "console" && pages.is_empty())
        && paused == 0
    {
        None
    } else {
        let wanted = args["window"]
            .as_u64()
            .filter(|window| pages.contains_key(window))
            .or((paused > 0).then(|| paused - 1));
        Some(
            wanted
                .or_else(|| pages.keys().next().copied())
                .context("The app has no windows")?,
        )
    };
    drop(pages);
    if tool == "profile"
        && args["action"].is_null()
        && let Some(duration) = args["duration"].as_u64()
    {
        dispatch(tool, json!({"action": "start"}), key)?;
        std::thread::sleep(Duration::from_millis(duration.min(60_000)));
        return dispatch(tool, json!({"action": "stop"}), key);
    }
    dispatch(tool, args, key)
}

fn dispatch(tool: &str, args: Value, key: Option<u64>) -> Result<Value> {
    let timeout =
        Duration::from_millis(30_000 + args["duration"].as_u64().unwrap_or(0).min(60_000));
    let (reply, result) = mpsc::channel();
    let pauses = SHARED.pauses.load(Ordering::Acquire);
    let sender = match key {
        None => MAIN.get().context("The app has quit")?.clone(),
        Some(key) => PAGES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned()
            .context("The window is closed")?,
    };
    sender
        .send(Call {
            tool: tool.into(),
            args,
            reply,
        })
        .map_err(|_| anyhow!("The app has quit"))?;
    (host().wake)(key.unwrap_or(u64::MAX));
    let started = Instant::now();
    let value = loop {
        match result.recv_timeout(Duration::from_millis(100)) {
            Ok(value) => break value?,
            Err(mpsc::RecvTimeoutError::Disconnected) => bail!("The app has quit"),
            Err(_) if started.elapsed() > timeout => {
                bail!(
                    "No answer after {} seconds. The app may be busy, or its window hidden",
                    timeout.as_secs()
                )
            }
            Err(_) => {
                let paused = SHARED.paused.lock().unwrap().clone();
                // This call itself may have hit a breakpoint, and then only the debugger tool can answer.
                if let Some(at) = paused.filter(|_| SHARED.pauses.load(Ordering::Acquire) != pauses)
                {
                    if let Ok(value) = result.try_recv() {
                        break value?;
                    }
                    bail!(
                        "The app paused in the debugger at {at}. Use the debugger tool to inspect or resume it"
                    );
                }
            }
        }
    };
    Ok(value)
}

const TOOLS: &str = r#"[
  {"name": "windows", "description": "Lists the open windows with their ids, titles, URLs and bounds.", "inputSchema": {"type": "object", "properties": {}}},
  {"name": "evaluate", "description": "Runs JavaScript in the main process or a page and returns the result. Promises are awaited. While the debugger is paused in that context, it runs in the selected call frame.", "inputSchema": {"type": "object", "properties": {
    "window": {"type": "integer", "description": "Window id from `windows`. Omit for the main process."},
    "preload": {"type": "boolean", "description": "Use the preload context of the window."},
    "expression": {"type": "string"},
    "frame": {"type": "integer", "description": "Call frame index while paused. Default 0."}
  }, "required": ["expression"]}},
  {"name": "screenshot", "description": "Captures a window, or one element of it, as PNG.", "inputSchema": {"type": "object", "properties": {
    "window": {"type": "integer", "description": "Window id from `windows`. Defaults to the first window."},
    "selector": {"type": "string", "description": "CSS selector of the element to crop to."},
    "device_pixels": {"type": "boolean", "description": "Keep device pixels instead of CSS pixels."}
  }}},
  {"name": "dom", "description": "Returns the HTML of the page or of the first element that matches a selector, with its bounding box and computed styles.", "inputSchema": {"type": "object", "properties": {
    "window": {"type": "integer", "description": "Window id from `windows`. Defaults to the first window."},
    "selector": {"type": "string"},
    "styles": {"description": "true for all computed styles, or a list of property names.", "anyOf": [{"type": "boolean"}, {"type": "array", "items": {"type": "string"}}]},
    "max_length": {"type": "integer", "description": "Maximum HTML length. Default 20000."}
  }}},
  {"name": "input", "description": "Sends real mouse and keyboard input to a window: click, move, press, pressed_move and release for a paced drag, drag (from x, y to to_x, to_y), drop (paths dragged from another app, from x, y to to_x, to_y, canceled with leave), scroll, key (for example `enter`, `cmd-a`, `shift-tab`) or type (text into the focused element).", "inputSchema": {"type": "object", "properties": {
    "window": {"type": "integer", "description": "Window id from `windows`. Defaults to the first window."},
    "action": {"type": "string", "enum": ["click", "move", "press", "pressed_move", "release", "drag", "drop", "scroll", "key", "type"]},
    "selector": {"type": "string", "description": "Element to point at. Its center is used."},
    "x": {"type": "number"},
    "y": {"type": "number"},
    "button": {"type": "string", "enum": ["left", "right", "middle"]},
    "count": {"type": "integer", "description": "Click count. 2 is a double click."},
    "key": {"type": "string"},
    "text": {"type": "string"},
    "to_x": {"type": "number"},
    "to_y": {"type": "number"},
    "paths": {"type": "array", "items": {"type": "string"}, "description": "Files that a drop action carries."},
    "leave": {"type": "boolean", "description": "A drop action leaves the window instead of dropping."},
    "hold": {"type": "boolean", "description": "A drop action stays over the window, to look at the page during the drag."},
    "delta_x": {"type": "number"},
    "delta_y": {"type": "number"},
    "lines": {"type": "boolean", "description": "Scroll deltas are mouse wheel lines instead of trackpad pixels."},
    "phase": {"type": "string", "enum": ["began", "changed", "ended", "momentum", "momentum_ended"], "description": "Where a trackpad scroll is in its gesture. Without it, the box bounces back 100 ms after the last scroll."}
  }, "required": ["action"]}},
  {"name": "console", "description": "Returns console messages and uncaught errors from all contexts since the last call.", "inputSchema": {"type": "object", "properties": {
    "keep": {"type": "boolean", "description": "Keep the messages for the next call."}
  }}},
  {"name": "memory", "description": "Reports process memory, the JavaScript heap and DOM sizes. Can collect garbage first or write a heap snapshot.", "inputSchema": {"type": "object", "properties": {
    "gc": {"type": "boolean", "description": "Run a full garbage collection first."},
    "snapshot": {"type": "boolean", "description": "Write a Web Inspector heap snapshot file and summarize it by class."}
  }}},
  {"name": "profile", "description": "Profiles JavaScript (sampling, all contexts), frame times of each window and process CPU. Pass `duration`, or `action` start and later stop.", "inputSchema": {"type": "object", "properties": {
    "action": {"type": "string", "enum": ["start", "stop"]},
    "duration": {"type": "integer", "description": "Milliseconds to profile, at most 60000."}
  }}},
  {"name": "debugger", "description": "JavaScript debugger. Actions: break (set a breakpoint at url and line), clear (remove breakpoint id, or all), pause, resume, step_over, step_into, step_out, state (call stack and variables when paused), exceptions (pause on `none`, `uncaught` or `all`). Once used, `debugger` statements in that context pause too. Use evaluate to read values in a paused frame.", "inputSchema": {"type": "object", "properties": {
    "window": {"type": "integer", "description": "Window id from `windows`. Omit for the main process."},
    "preload": {"type": "boolean", "description": "Use the preload context of the window."},
    "action": {"type": "string", "enum": ["break", "clear", "pause", "resume", "step_over", "step_into", "step_out", "state", "exceptions"]},
    "url": {"type": "string", "description": "Source file path or script URL."},
    "line": {"type": "integer"},
    "column": {"type": "integer"},
    "condition": {"type": "string", "description": "JavaScript condition for the breakpoint."},
    "id": {"type": "string", "description": "Breakpoint id for clear."},
    "frame": {"type": "integer", "description": "Call frame whose variables `state` lists. Default 0."},
    "state": {"type": "string", "enum": ["none", "uncaught", "all"]}
  }, "required": ["action"]}}
]"#;

fn tools() -> Value {
    serde_json::from_str(TOOLS).unwrap()
}

fn help() -> String {
    let mut text = String::from(
        "apps devtools. POST /<tool> with JSON arguments; the answer is text, JSON or PNG.\nMCP clients use POST /mcp.\n",
    );
    for tool in tools().as_array().unwrap() {
        text += &format!(
            "\n{}: {}\n",
            tool["name"].as_str().unwrap(),
            tool["description"].as_str().unwrap()
        );
        for (name, schema) in tool["inputSchema"]["properties"].as_object().unwrap() {
            let kind = schema["enum"].as_array().map(|values| {
                values
                    .iter()
                    .map(|value| value.as_str().unwrap())
                    .collect::<Vec<_>>()
                    .join("|")
            });
            let kind = kind
                .unwrap_or_else(|| schema["type"].as_str().unwrap_or("boolean|string[]").into());
            text += &format!(
                "  {name} ({kind}){}\n",
                schema["description"]
                    .as_str()
                    .map(|text| format!(": {text}"))
                    .unwrap_or_default()
            );
        }
    }
    text
}

fn next(wait: bool) -> Option<Call> {
    let take = |calls: &mpsc::Receiver<Call>| {
        if wait {
            calls.recv_timeout(Duration::from_millis(50)).ok()
        } else {
            calls.try_recv().ok()
        }
    };
    CHANNEL
        .with(|channel| channel.get().and_then(|(calls, _)| take(calls)))
        .or_else(|| MAIN_CHANNEL.with(|channel| channel.get().and_then(take)))
}

fn settle_step() {
    if STATE.with_borrow(|state| state.paused.is_none())
        && let Some(reply) = STATE.with_borrow_mut(|state| state.stepping.take())
    {
        let _ = reply.send(Ok(
            "Resumed. The code finished without pausing again.".into()
        ));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn apps_mcp_poll(app: *mut c_void) -> bool {
    if app.is_null() && !MAIN_CHANNEL.with(|channel| channel.get().is_some()) {
        return false;
    }
    pump();
    settle_step();
    let mut ran = false;
    while let Some(call) = if app.is_null() {
        MAIN_CHANNEL.with(|channel| channel.get()?.try_recv().ok())
    } else {
        next(false)
    } {
        ran = true;
        run(call, app);
    }
    ran
}

/// # Safety
/// `logs` is a JSON array of `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_mcp_logs(logs: *const u8, len: usize) {
    let Ok(logs) =
        serde_json::from_slice::<Vec<Value>>(unsafe { std::slice::from_raw_parts(logs, len) })
    else {
        return;
    };
    let mut state = LOGS.lock().unwrap_or_else(PoisonError::into_inner);
    for log in logs {
        let text = if log["type"] == "error" {
            format!("[error] {}", log["message"].as_str().unwrap_or(""))
        } else {
            let context = match &log["params"]["context"] {
                Value::String(role) => role.clone(),
                Value::Object(role) => role
                    .iter()
                    .map(|(role, id)| format!("{role} {id}"))
                    .collect(),
                _ => String::new(),
            };
            let level = log["params"]["type"].as_str().unwrap_or("log");
            format!(
                "[{level}] {context}: {}",
                log["params"]["text"].as_str().unwrap_or("")
            )
        };
        if state.len() == 1000 {
            state.pop_front();
        }
        state.push_back(text);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn apps_mcp_profiling() -> bool {
    FRAMES.with_borrow(Option::is_some)
}

#[unsafe(no_mangle)]
pub extern "C" fn apps_mcp_frame(window: u64, nanos: u64) {
    FRAMES.with_borrow_mut(|frames| {
        if let Some(frames) = frames {
            frames.push((window, Duration::from_nanos(nanos)));
        }
    });
}

/// # Safety
/// Runs while JSC is paused in `ctx`, and dispatches protocol messages until JSC sets `done`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_mcp_paused(ctx: usize, done: *mut bool) {
    pump();
    let page = CHANNEL.with(|channel| channel.get().map(|(_, key)| *key));
    if page.is_none() && !MAIN_CHANNEL.with(|channel| channel.get().is_some()) {
        let _ = send(ctx, "Debugger.resume", json!({}));
        return;
    }
    if let Some(reply) = STATE.with_borrow_mut(|state| state.stepping.take()) {
        let _ = reply.send(state(&json!({})));
    }
    let at = STATE.with_borrow(|state| {
        let (_, paused) = state.paused.as_ref()?;
        let frame = &paused["callFrames"][0];
        Some(format!(
            "{} in {} ({})",
            location(state, &frame["location"]),
            name(frame),
            paused["reason"].as_str().unwrap_or("pause")
        ))
    });
    *SHARED.paused.lock().unwrap() = Some(at.unwrap_or_else(|| "a statement".into()));
    SHARED.pauses.fetch_add(1, Ordering::AcqRel);
    SHARED
        .page_paused
        .store(page.map_or(0, |key| key + 1), Ordering::Release);
    // JSC sets `done` while a dispatched message runs, so read it again each time.
    while !unsafe { std::ptr::read_volatile(done) } {
        if let Some(call) = next(true) {
            run(call, std::ptr::null_mut());
        }
        pump();
    }
    *SHARED.paused.lock().unwrap() = None;
    SHARED.page_paused.store(0, Ordering::Release);
    STATE.with_borrow_mut(|state| state.paused = None);
}

fn run(Call { tool, args, reply }: Call, app: *mut c_void) {
    let result = match tool.as_str() {
        "windows" | "screenshot" | "input" | "memory" if app.is_null() => Err(anyhow!(PAUSED)),
        "windows" => {
            desktop(app, "windows", &args, reply.clone());
            Ok(None)
        }
        "evaluate" => evaluate(&args, &reply),
        "screenshot" => screenshot(&args, app, &reply),
        "dom" => dom(&args).map(Some),
        "input" => input(&args, app, &reply),
        "console" => Ok(Some(console(&args))),
        "memory" => memory(&args, app).map(Some),
        "profile" => profile(&args).map(Some),
        "debugger" => debugger(&args, &reply),
        _ => Err(anyhow!("Unknown tool: {tool}")),
    };
    match result {
        Ok(Some(value)) => drop(reply.send(Ok(value))),
        Ok(None) => {}
        Err(error) => drop(reply.send(Err(error))),
    }
}

fn host() -> &'static Host {
    HOST.get().expect("The devtools MCP server is not started")
}

fn desktop(app: *mut c_void, op: &str, args: &Value, reply: Reply) {
    extern "C" fn done(reply: *mut c_void, ok: bool, data: *const u8, len: usize) {
        let reply = unsafe { Box::from_raw(reply.cast::<Reply>()) };
        let data = unsafe { std::slice::from_raw_parts(data, len) };
        let _ = reply.send(if ok {
            serde_json::from_slice(data).map_err(Into::into)
        } else {
            Err(anyhow!("{}", String::from_utf8_lossy(data)))
        });
    }
    let args = args.to_string();
    let reply = Box::into_raw(Box::new(reply)).cast();
    (host().call)(
        app,
        op.as_ptr(),
        op.len(),
        args.as_ptr(),
        args.len(),
        reply,
        done,
    );
}

fn desktop_now(app: *mut c_void, op: &str, args: &Value) -> Result<Value> {
    let (reply, result) = mpsc::channel();
    desktop(app, op, args, reply);
    result
        .try_recv()
        .with_context(|| format!("{op} did not answer"))?
}

fn contexts() -> Vec<(usize, Role)> {
    extern "C" fn push(list: *mut c_void, ctx: usize, role: u32, id: u64) {
        let list = unsafe { &mut *list.cast::<Vec<(usize, Role)>>() };
        list.push((
            ctx,
            match role {
                0 => Role::Main,
                1 => Role::Preload(id),
                _ => Role::Renderer(id),
            },
        ));
    }
    let mut list = Vec::new();
    (host().contexts)((&raw mut list).cast(), push);
    list
}

fn context(role: Role) -> Option<usize> {
    contexts()
        .into_iter()
        .find_map(|(ctx, found)| (found == role).then_some(ctx))
}

fn inspector_messages() -> Vec<(usize, String)> {
    extern "C" fn push(list: *mut c_void, ctx: usize, message: *const u8, len: usize) {
        let list = unsafe { &mut *list.cast::<Vec<(usize, String)>>() };
        let message = unsafe { std::slice::from_raw_parts(message, len) };
        list.push((ctx, String::from_utf8_lossy(message).into_owned()));
    }
    let mut list = Vec::new();
    (host().messages)((&raw mut list).cast(), push);
    list
}

fn connect_inspector(ctx: usize) -> Result<bool> {
    match (host().connect)(ctx) {
        0 => Ok(false),
        1 => Ok(true),
        _ => bail!(
            "Cannot inspect the JavaScript context. It closed, or JavaScriptCore has no inspector"
        ),
    }
}

fn inspect(ctx: usize, message: &str) -> Result<()> {
    ensure!(
        (host().inspect)(ctx, message.as_ptr(), message.len()),
        "The JavaScript context is closed"
    );
    Ok(())
}

fn pump() {
    for (ctx, message) in inspector_messages() {
        let Ok(message) = serde_json::from_str::<Value>(&message) else {
            continue;
        };
        if let Some(id) = message["id"].as_u64() {
            let result = match message.get("error") {
                Some(error) => Err(error["message"]
                    .as_str()
                    .unwrap_or("Inspector error")
                    .to_owned()),
                None => Ok(message["result"].clone()),
            };
            match STATE.with_borrow_mut(|state| state.waiting.remove(&id)) {
                Some(done) => done(result.map_err(anyhow::Error::msg)),
                None => drop(STATE.with_borrow_mut(|state| state.responses.insert(id, result))),
            }
            continue;
        }
        let params = &message["params"];
        STATE.with_borrow_mut(|state| match message["method"].as_str().unwrap_or("") {
            "Debugger.scriptParsed" => {
                if let (Some(session), Some(id)) =
                    (state.sessions.get_mut(&ctx), params["scriptId"].as_str())
                {
                    session.scripts.insert(
                        id.into(),
                        Script {
                            url: params["url"].as_str().unwrap_or("").into(),
                            map_url: params["sourceMapURL"].as_str().unwrap_or("").into(),
                            map: OnceCell::new(),
                        },
                    );
                }
            }
            "Debugger.paused" => state.paused = Some((ctx, params.clone())),
            "Debugger.resumed" => state.paused = None,
            "ScriptProfiler.trackingComplete" => state.samples = Some(params["samples"].clone()),
            _ => {}
        });
    }
}

fn send(ctx: usize, method: &str, params: Value) -> Result<u64> {
    if connect_inspector(ctx)? {
        STATE.with_borrow_mut(|state| state.sessions.remove(&ctx));
    }
    let id = STATE.with_borrow_mut(|state| {
        state.next += 1;
        state.next
    });
    inspect(
        ctx,
        &json!({"id": id, "method": method, "params": params}).to_string(),
    )?;
    Ok(id)
}

fn request(ctx: usize, method: &str, params: Value) -> Result<Value> {
    let id = send(ctx, method, params)?;
    pump();
    STATE
        .with_borrow_mut(|state| state.responses.remove(&id))
        .with_context(|| format!("{method} did not answer"))?
        .map_err(anyhow::Error::msg)
}

fn request_later(ctx: usize, method: &str, params: Value, done: Waiter) -> Result<()> {
    let id = send(ctx, method, params)?;
    STATE.with_borrow_mut(|state| state.waiting.insert(id, done));
    pump();
    Ok(())
}

fn attach(ctx: usize) -> Result<()> {
    if !connect_inspector(ctx)? && STATE.with_borrow(|state| state.sessions.contains_key(&ctx)) {
        return Ok(());
    }
    STATE.with_borrow_mut(|state| state.sessions.insert(ctx, Session::default()));
    request(ctx, "Debugger.enable", json!({}))?;
    request(
        ctx,
        "Debugger.setBreakpointsActive",
        json!({"active": true}),
    )?;
    Ok(())
}

fn debug(ctx: usize) -> Result<()> {
    attach(ctx)?;
    request(
        ctx,
        "Debugger.setPauseOnDebuggerStatements",
        json!({"enabled": true}),
    )?;
    Ok(())
}

fn target(args: &Value) -> Result<usize> {
    let role = match args["window"].as_u64() {
        None => Role::Main,
        Some(id) if args["preload"] == true => Role::Preload(id),
        Some(id) => Role::Renderer(id),
    };
    context(role).with_context(|| format!("No JavaScript context for {role:?}"))
}

fn window(args: &Value) -> Result<u64> {
    match args["window"].as_u64() {
        Some(id) => Ok(id),
        None => contexts()
            .into_iter()
            .filter_map(|(_, role)| match role {
                Role::Renderer(id) => Some(id),
                _ => None,
            })
            .min()
            .context("The app has no pages"),
    }
}

fn page_value(window: u64, expression: &str) -> Result<Value> {
    let ctx =
        context(Role::Renderer(window)).with_context(|| format!("Window {window} has no page"))?;
    let response = request(
        ctx,
        "Runtime.evaluate",
        json!({"expression": expression, "returnByValue": true}),
    )?;
    let value = response["result"]["value"].clone();
    ensure!(
        response["wasThrown"] != true,
        "{}",
        response["result"]["description"]
            .as_str()
            .unwrap_or("Page script failed")
    );
    Ok(value)
}

fn evaluate(args: &Value, reply: &Reply) -> Result<Option<Value>> {
    let expression = args["expression"]
        .as_str()
        .context("Expected an expression")?;
    let ctx = target(args)?;
    let frame = STATE.with_borrow(|state| {
        let (_, data) = state.paused.as_ref().filter(|(paused, _)| *paused == ctx)?;
        data["callFrames"][args["frame"].as_u64().unwrap_or(0) as usize]["callFrameId"]
            .as_str()
            .map(str::to_owned)
    });
    let response = match frame {
        Some(frame) => request(
            ctx,
            "Debugger.evaluateOnCallFrame",
            json!({"callFrameId": frame, "expression": expression, "objectGroup": "mcp", "includeCommandLineAPI": true}),
        ),
        None => request(
            ctx,
            "Runtime.evaluate",
            json!({"expression": expression, "objectGroup": "mcp", "includeCommandLineAPI": true}),
        ),
    }?;
    let thrown = response["wasThrown"] == true;
    let object = &response["result"];
    let Some(id) = object["objectId"].as_str() else {
        let _ = request(
            ctx,
            "Runtime.releaseObjectGroup",
            json!({"objectGroup": "mcp"}),
        );
        let text = match object.get("value") {
            Some(Value::String(text)) => text.clone(),
            Some(value) => value.to_string(),
            None => object["description"].as_str().unwrap_or("undefined").into(),
        };
        ensure!(!thrown, "Uncaught {text}");
        return Ok(Some(text.into()));
    };
    let reply = reply.clone();
    request_later(
        ctx,
        "Runtime.callFunctionOn",
        json!({"objectId": id, "functionDeclaration": SHOW, "returnByValue": true, "awaitPromise": true}),
        Box::new(move |result| {
            let _ = request(
                ctx,
                "Runtime.releaseObjectGroup",
                json!({"objectGroup": "mcp"}),
            );
            let _ = reply.send(result.and_then(|response| {
                let text = response["result"]["value"]
                    .as_str()
                    .unwrap_or("undefined")
                    .to_owned();
                ensure!(!thrown && response["wasThrown"] != true, "Uncaught {text}");
                Ok(text.into())
            }));
        }),
    )?;
    Ok(None)
}

fn rect(window: u64, selector: &str) -> Result<[f64; 4]> {
    let value = page_value(
        window,
        &format!(
            "(()=>{{const r=document.querySelector({})?.getBoundingClientRect();return r&&[r.x,r.y,r.width,r.height]}})()",
            json!(selector)
        ),
    )?;
    serde_json::from_value(value)
        .ok()
        .with_context(|| format!("No element matches {selector}"))
}

fn screenshot(args: &Value, app: *mut c_void, reply: &Reply) -> Result<Option<Value>> {
    let mut args = args.clone();
    if let Some(selector) = args["selector"].as_str() {
        let window = window(&args)?;
        let [x, y, width, height] = rect(window, selector)?;
        args["window"] = window.into();
        args["crop"] = json!({"x": x, "y": y, "width": width, "height": height});
    }
    desktop(app, "screenshot", &args, reply.clone());
    Ok(None)
}

fn dom(args: &Value) -> Result<Value> {
    let window = window(args)?;
    let styles = match &args["styles"] {
        Value::Bool(true) => json!(true),
        Value::Array(names) => json!(names),
        _ => json!([]),
    };
    let mut value = page_value(
        window,
        &format!(
            "((s,p)=>{{const e=s?document.querySelector(s):document.documentElement;if(!e)return null;const r=e.getBoundingClientRect(),c=getComputedStyle(e);return{{html:e.outerHTML,rect:[r.x,r.y,r.width,r.height],style:p===true?Object.fromEntries(Array.from(c,k=>[k,c.getPropertyValue(k)])):Object.fromEntries(p.map(k=>[k,c.getPropertyValue(k)]))}}}})({},{styles})",
            json!(args["selector"].as_str()),
        ),
    )?;
    ensure!(
        !value.is_null(),
        "No element matches {}",
        args["selector"].as_str().unwrap_or("the document")
    );
    let limit = args["max_length"].as_u64().unwrap_or(20_000) as usize;
    if let Some(html) = value["html"].as_str()
        && html.len() > limit
    {
        let cut = html.floor_char_boundary(limit);
        value["html"] = format!("{}… ({} more bytes)", &html[..cut], html.len() - cut).into();
    }
    if value["style"]
        .as_object()
        .is_some_and(serde_json::Map::is_empty)
    {
        value.as_object_mut().unwrap().remove("style");
    }
    Ok(value)
}

fn input(args: &Value, app: *mut c_void, reply: &Reply) -> Result<Option<Value>> {
    let mut args = args.clone();
    if let Some(selector) = args["selector"].as_str() {
        let window = window(&args)?;
        let [x, y, width, height] = rect(window, selector)?;
        ensure!(width > 0. && height > 0., "{selector} has no size");
        args["window"] = window.into();
        args["x"] = (x + width / 2.).into();
        args["y"] = (y + height / 2.).into();
    } else {
        ensure!(
            args["x"].is_number() && args["y"].is_number()
                || matches!(args["action"].as_str(), Some("key" | "type")),
            "Pass a selector, or x and y"
        );
    }
    desktop(app, "input", &args, reply.clone());
    Ok(None)
}

fn console(args: &Value) -> Value {
    let mut logs = LOGS.lock().unwrap_or_else(PoisonError::into_inner);
    let text = logs.iter().cloned().collect::<Vec<_>>().join("\n");
    if args["keep"] != true {
        logs.clear();
    }
    if text.is_empty() {
        "No console messages.".into()
    } else {
        text.into()
    }
}

fn memory(args: &Value, app: *mut c_void) -> Result<Value> {
    let mut report = desktop_now(app, "memory", args)?;
    if let Some(types) = report["heap"]
        .as_object_mut()
        .and_then(|heap| heap.remove("objectTypeCounts"))
        && let Some(types) = types.as_object()
    {
        let mut types: Vec<_> = types
            .iter()
            .map(|(name, count)| (count.as_u64().unwrap_or(0), name))
            .collect();
        types.sort_unstable_by(|a, b| b.cmp(a));
        report["heap"]["topObjectTypes"] = types
            .into_iter()
            .take(20)
            .map(|(count, name)| json!([name, count]))
            .collect();
    }
    if args["snapshot"] == true {
        let ctx = context(Role::Renderer(window(args)?)).context("No JavaScript context")?;
        let snapshot = request(ctx, "Heap.snapshot", json!({}))?;
        let data = snapshot["snapshotData"]
            .as_str()
            .context("Empty heap snapshot")?;
        let path = std::env::temp_dir().join(format!(
            "apps-heap-{}-{}.json",
            std::process::id(),
            snapshot["timestamp"].as_f64().unwrap_or(0.) as u64
        ));
        std::fs::write(&path, data)?;
        let parsed: Value = serde_json::from_str(data)?;
        let stride = if parsed["type"] == "GCDebugging" {
            7
        } else {
            4
        };
        let names = parsed["nodeClassNames"]
            .as_array()
            .context("Invalid heap snapshot")?;
        let mut classes: HashMap<usize, (u64, u64)> = HashMap::new();
        for node in parsed["nodes"]
            .as_array()
            .context("Invalid heap snapshot")?
            .chunks(stride)
        {
            let entry = classes
                .entry(node[2].as_u64().unwrap_or(0) as usize)
                .or_default();
            entry.0 += 1;
            entry.1 += node[1].as_u64().unwrap_or(0);
        }
        let mut classes: Vec<_> = classes.into_iter().collect();
        classes.sort_unstable_by_key(|(_, (_, size))| std::cmp::Reverse(*size));
        report["snapshot"] = json!({
            "path": path,
            "largestClasses": classes.into_iter().take(25).map(|(name, (count, size))| json!({"class": names.get(name), "count": count, "bytes": size})).collect::<Vec<_>>(),
        });
    }
    Ok(report)
}

fn cpu_time() -> f64 {
    let usage = desktop_now(std::ptr::null_mut(), "cpu", &Value::Null).unwrap_or_default();
    (usage["user"].as_f64().unwrap_or(0.) + usage["system"].as_f64().unwrap_or(0.)) / 1000.
}

fn profile(args: &Value) -> Result<Value> {
    match args["action"].as_str() {
        Some("start") => {
            profile_start()?;
            Ok("Profiling. Call profile with action stop to get the report.".into())
        }
        Some("stop") => profile_stop(),
        _ => bail!("Pass action start or stop, or a duration"),
    }
}

fn profile_start() -> Result<()> {
    ensure!(
        STATE.with_borrow(|state| state.profile.is_none()),
        "A profile is already running"
    );
    let ctx = context(Role::Main)
        .or_else(|| {
            contexts()
                .into_iter()
                .find_map(|(ctx, role)| matches!(role, Role::Renderer(_)).then_some(ctx))
        })
        .context("No JavaScript context")?;
    for (ctx, _) in contexts() {
        attach(ctx)?;
    }
    request(
        ctx,
        "ScriptProfiler.startTracking",
        json!({"includeSamples": true}),
    )?;
    STATE.with_borrow_mut(|state| {
        state.samples = None;
        state.profile = Some(Profile {
            ctx,
            started: Instant::now(),
            cpu: cpu_time(),
        });
    });
    FRAMES.set(Some(Vec::new()));
    Ok(())
}

fn profile_stop() -> Result<Value> {
    let profile = STATE
        .with_borrow_mut(|state| state.profile.take())
        .context("No profile is running")?;
    let frames = FRAMES.take().unwrap_or_default();
    let elapsed = profile.started.elapsed().as_secs_f64() * 1000.;
    request(profile.ctx, "ScriptProfiler.stopTracking", json!({}))?;
    let cpu = cpu_time() - profile.cpu;
    let mut text = format!(
        "Profiled {elapsed:.0} ms. Process CPU {cpu:.0} ms ({:.1}% of one core).\n",
        cpu / elapsed * 100.
    );
    if frames.is_empty() {
        text += "No window drew a frame.\n";
    }
    let mut windows: Vec<u64> = frames.iter().map(|(window, _)| *window).collect();
    windows.sort_unstable();
    windows.dedup();
    for window in windows {
        let mut times: Vec<f64> = frames
            .iter()
            .filter(|(id, _)| *id == window)
            .map(|(_, time)| time.as_secs_f64() * 1000.)
            .collect();
        times.sort_unstable_by(f64::total_cmp);
        text += &format!(
            "Window {window}: {} frames ({:.1} fps). Frame CPU time from render to paint: avg {:.2} ms, p95 {:.2} ms, max {:.2} ms.\n",
            times.len(),
            times.len() as f64 / elapsed * 1000.,
            times.iter().sum::<f64>() / times.len() as f64,
            times[(times.len() * 95 / 100).min(times.len() - 1)],
            times[times.len() - 1],
        );
    }
    let traces = STATE
        .with_borrow_mut(|state| state.samples.take())
        .unwrap_or_default();
    let traces = traces["stackTraces"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut functions: HashMap<String, (u64, u64)> = HashMap::new();
    STATE.with_borrow(|state| {
        for trace in &traces {
            let mut seen = Vec::new();
            for (index, frame) in trace["stackFrames"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
            {
                let key = format!(
                    "{}  {}",
                    frame["name"]
                        .as_str()
                        .filter(|name| !name.is_empty())
                        .unwrap_or("(anonymous)"),
                    location(
                        state,
                        &json!({
                            "scriptId": frame["sourceID"],
                            "lineNumber": frame["line"].as_i64().unwrap_or(0) - 1,
                            "columnNumber": frame["column"].as_i64().unwrap_or(0) - 1,
                        })
                    )
                );
                if seen.contains(&key) {
                    continue;
                }
                let entry = functions.entry(key.clone()).or_default();
                entry.0 += u64::from(index == 0);
                entry.1 += 1;
                seen.push(key);
            }
        }
    });
    if traces.is_empty() {
        text += "No JavaScript ran.";
        return Ok(text.into());
    }
    let mut functions: Vec<_> = functions.into_iter().collect();
    functions.sort_unstable_by_key(|(_, counts)| std::cmp::Reverse(*counts));
    text += &format!(
        "JavaScript: {} samples, about 1 ms each.\n   self   total  function  location\n",
        traces.len()
    );
    for (name, (own, total)) in functions.into_iter().take(30) {
        text += &format!("{own:7} {total:7}  {name}\n");
    }
    Ok(text.into())
}

fn debugger(args: &Value, reply: &Reply) -> Result<Option<Value>> {
    let action = args["action"].as_str().context("Expected an action")?;
    let paused = STATE.with_borrow(|state| state.paused.as_ref().map(|(ctx, _)| *ctx));
    let text = match action {
        "state" => return state(args).map(Some),
        "resume" | "step_over" | "step_into" | "step_out" => {
            let ctx = paused.context("The app is not paused")?;
            let method = match action {
                "resume" => "Debugger.resume",
                "step_over" => "Debugger.stepOver",
                "step_into" => "Debugger.stepInto",
                _ => "Debugger.stepOut",
            };
            if action != "resume" {
                STATE.with_borrow_mut(|state| state.stepping = Some(reply.clone()));
            }
            request(ctx, method, json!({}))?;
            if action != "resume" {
                return Ok(None);
            }
            "Resumed.".into()
        }
        "pause" => {
            let ctx = target(args)?;
            debug(ctx)?;
            request(ctx, "Debugger.pause", json!({}))?;
            "The debugger pauses at the next statement that runs. Make the app run code, then use action state.".into()
        }
        "exceptions" => {
            let ctx = target(args)?;
            debug(ctx)?;
            let mode = args["state"].as_str().unwrap_or("uncaught");
            request(ctx, "Debugger.setPauseOnExceptions", json!({"state": mode}))?;
            format!("Pause on exceptions: {mode}.")
        }
        "break" => {
            let ctx = target(args)?;
            debug(ctx)?;
            breakpoint(ctx, args)?
        }
        "clear" => {
            let ctx = target(args)?;
            attach(ctx)?;
            let id = args["id"].as_str();
            let ids: Vec<String> = STATE
                .with_borrow_mut(|state| {
                    let session = state.sessions.get_mut(&ctx)?;
                    let (removed, kept) = std::mem::take(&mut session.breakpoints)
                        .into_iter()
                        .partition(|(breakpoint, _)| id.is_none_or(|id| id == breakpoint));
                    session.breakpoints = kept;
                    Some(removed.into_iter().map(|(id, _)| id).collect::<Vec<_>>())
                })
                .unwrap_or_default();
            for id in &ids {
                request(
                    ctx,
                    "Debugger.removeBreakpoint",
                    json!({"breakpointId": id}),
                )?;
            }
            format!("Removed {} breakpoint(s).", ids.len())
        }
        _ => bail!("Unknown debugger action: {action}"),
    };
    Ok(Some(text.into()))
}

fn breakpoint(ctx: usize, args: &Value) -> Result<String> {
    let url = args["url"].as_str().context("Expected a url")?;
    let line = args["line"].as_u64().context("Expected a line")?.max(1) as u32 - 1;
    let column = args["column"].as_u64().unwrap_or(1).max(1) as u32 - 1;
    let options = json!({"condition": args["condition"].as_str().unwrap_or("")});
    let wanted = url
        .strip_prefix("file://")
        .unwrap_or(url)
        .trim_start_matches("./")
        .replace('\\', "/");
    let absolute = std::path::absolute(&wanted)
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    let matches = |source: &str| {
        let source = source
            .strip_prefix("file://")
            .unwrap_or(source)
            .replace('\\', "/");
        source == absolute || source == wanted || source.ends_with(&format!("/{wanted}"))
    };
    let locations: Vec<(String, u32, u32)> = STATE.with_borrow(|state| {
        let Some(session) = state.sessions.get(&ctx) else {
            return Vec::new();
        };
        session
            .scripts
            .iter()
            .filter_map(|(id, script)| {
                let map = source_map(script)?;
                let source = map.sources.iter().position(|source| matches(source))?;
                let (line, column) = map.generated(source as u32, line, column)?;
                Some((id.clone(), line, column))
            })
            .collect()
    });
    let mut placed = Vec::new();
    if locations.is_empty() {
        let pattern: String = wanted
            .chars()
            .flat_map(|c| {
                if c.is_ascii_alphanumeric() || c == '/' {
                    vec![c]
                } else {
                    vec!['\\', c]
                }
            })
            .collect();
        let response = request(
            ctx,
            "Debugger.setBreakpointByUrl",
            json!({"lineNumber": line, "columnNumber": column, "urlRegex": format!("{pattern}$"), "options": options}),
        )?;
        let id = response["breakpointId"].as_str().unwrap_or("").to_owned();
        let found = response["locations"].as_array().map_or(0, Vec::len);
        placed.push((
            id,
            format!(
                "{url}:{} ({found} loaded script(s) match; scripts loaded later match by URL)",
                line + 1
            ),
        ));
    }
    for (script, generated, generated_column) in locations {
        let response = request(
            ctx,
            "Debugger.setBreakpoint",
            json!({"location": {"scriptId": script, "lineNumber": generated, "columnNumber": generated_column}, "options": options}),
        )?;
        let at = STATE.with_borrow(|state| location(state, &response["actualLocation"]));
        placed.push((
            response["breakpointId"].as_str().unwrap_or("").to_owned(),
            at,
        ));
    }
    let text = placed
        .iter()
        .map(|(id, at)| format!("Breakpoint {id} at {at}"))
        .collect::<Vec<_>>()
        .join("\n");
    STATE.with_borrow_mut(|state| {
        if let Some(session) = state.sessions.get_mut(&ctx) {
            session.breakpoints.extend(placed);
        }
    });
    Ok(text)
}

fn state(args: &Value) -> Result<Value> {
    pump();
    let Some((ctx, paused)) = STATE.with_borrow(|state| state.paused.clone()) else {
        let breakpoints = STATE.with_borrow(|state| {
            state
                .sessions
                .values()
                .flat_map(|session| &session.breakpoints)
                .map(|(id, at)| format!("\n  {id} at {at}"))
                .collect::<String>()
        });
        return Ok(format!(
            "Running. Breakpoints:{}",
            if breakpoints.is_empty() {
                " none"
            } else {
                &breakpoints
            }
        )
        .into());
    };
    let frames = paused["callFrames"].as_array().cloned().unwrap_or_default();
    let mut text = format!("Paused ({})", paused["reason"].as_str().unwrap_or("pause"));
    if let Some(description) = paused["data"]["description"].as_str() {
        text += &format!(": {description}");
    }
    STATE.with_borrow(|state| {
        for (index, frame) in frames.iter().enumerate().take(30) {
            text += &format!(
                "\n#{index} {}  {}",
                name(frame),
                location(state, &frame["location"])
            );
        }
    });
    let frame = frames
        .get(args["frame"].as_u64().unwrap_or(0) as usize)
        .context("No such call frame")?;
    for scope in frame["scopeChain"].as_array().into_iter().flatten() {
        let kind = scope["type"].as_str().unwrap_or("scope");
        let Some(id) = scope["object"]["objectId"]
            .as_str()
            .filter(|_| !matches!(kind, "global" | "globalLexicalEnvironment"))
        else {
            continue;
        };
        let properties = request(
            ctx,
            "Runtime.getProperties",
            json!({"objectId": id, "ownProperties": true, "generatePreview": true}),
        )?;
        let variables: String = properties["properties"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|property| {
                property["name"]
                    .as_str()
                    .is_some_and(|name| !name.starts_with("$apps") && name != "$out")
            })
            .take(50)
            .map(|property| {
                format!(
                    "\n  {} = {}",
                    property["name"].as_str().unwrap_or("?"),
                    short(&property["value"])
                )
            })
            .collect();
        if !variables.is_empty() {
            text += &format!(
                "\n{kind}{}:{variables}",
                scope["name"]
                    .as_str()
                    .map(|name| format!(" {name}"))
                    .unwrap_or_default()
            );
        }
    }
    Ok(text.into())
}

fn name(frame: &Value) -> &str {
    frame["functionName"]
        .as_str()
        .filter(|name| !name.is_empty())
        .unwrap_or("(anonymous)")
}

fn short(object: &Value) -> String {
    let text = match (object["type"].as_str(), object.get("value")) {
        (Some("undefined"), _) => "undefined".into(),
        (_, Some(value)) => value.to_string(),
        _ => match object["preview"]["properties"].as_array() {
            Some(properties) => {
                let array = object["subtype"] == "array";
                let items = properties.iter().map(|property| {
                    let value = property["value"].as_str().unwrap_or("");
                    let value = if property["type"] == "string" {
                        json!(value).to_string()
                    } else {
                        value.to_owned()
                    };
                    if array {
                        value
                    } else {
                        format!("{}: {value}", property["name"].as_str().unwrap_or(""))
                    }
                });
                let mut items: Vec<_> = items.collect();
                if object["preview"]["overflow"] == true {
                    items.push("…".into());
                }
                let (open, close) = if array { ("[", "]") } else { ("{", "}") };
                let class = object["className"]
                    .as_str()
                    .filter(|class| *class != "Object" && !array);
                format!(
                    "{}{open}{}{close}",
                    class.map(|class| format!("{class} ")).unwrap_or_default(),
                    items.join(", ")
                )
            }
            None => object["description"]
                .as_str()
                .unwrap_or("?")
                .lines()
                .next()
                .unwrap_or("")
                .into(),
        },
    };
    if text.len() > 300 {
        format!("{}…", &text[..text.floor_char_boundary(300)])
    } else {
        text
    }
}

fn location(state: &State, location: &Value) -> String {
    let id = location["scriptId"].as_str().unwrap_or("");
    let line = location["lineNumber"].as_i64().unwrap_or(0).max(0) as u32;
    let column = location["columnNumber"].as_i64().unwrap_or(0).max(0) as u32;
    let Some(script) = state
        .sessions
        .values()
        .find_map(|session| session.scripts.get(id))
    else {
        return if id == "4294967295" {
            "[native code]".into()
        } else {
            format!("script {id}:{}:{}", line + 1, column + 1)
        };
    };
    if let Some(map) = source_map(script)
        && let Some((source, line, column)) = map.original(line, column)
    {
        return format!(
            "{}:{}:{}",
            map.sources[source as usize]
                .strip_prefix("file://")
                .unwrap_or(&map.sources[source as usize]),
            line + 1,
            column + 1
        );
    }
    format!("{}:{}:{}", script.url, line + 1, column + 1)
}

fn source_map(script: &Script) -> Option<Rc<SourceMap>> {
    script
        .map
        .get_or_init(|| SourceMap::parse(&script.map_url).map(Rc::new))
        .clone()
}

struct SourceMap {
    sources: Vec<String>,
    mappings: Vec<[u32; 5]>,
}

impl SourceMap {
    fn parse(url: &str) -> Option<Self> {
        let (head, data) = url.strip_prefix("data:")?.split_once(',')?;
        let json = if head.ends_with(";base64") {
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data).ok()?
        } else {
            let mut bytes = Vec::with_capacity(data.len());
            let mut input = data.bytes();
            while let Some(byte) = input.next() {
                if byte != b'%' {
                    bytes.push(byte);
                    continue;
                }
                let hex = [input.next()?, input.next()?];
                bytes.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
            }
            bytes
        };
        let map: Value = serde_json::from_slice(&json).ok()?;
        let sources = map["sources"]
            .as_array()?
            .iter()
            .map(|source| source.as_str().unwrap_or("").to_owned())
            .collect();
        let mut mappings = Vec::new();
        let (mut source, mut line, mut column) = (0i64, 0i64, 0i64);
        for (generated, group) in map["mappings"].as_str()?.split(';').enumerate() {
            let mut generated_column = 0i64;
            for segment in group.split(',').filter(|segment| !segment.is_empty()) {
                let (mut values, mut count, mut value, mut shift) = ([0i64; 5], 0, 0i64, 0);
                for byte in segment.bytes() {
                    let digit = match byte {
                        b'A'..=b'Z' => byte - b'A',
                        b'a'..=b'z' => byte - b'a' + 26,
                        b'0'..=b'9' => byte - b'0' + 52,
                        b'+' => 62,
                        _ => 63,
                    } as i64;
                    value += (digit & 31) << shift;
                    if digit & 32 != 0 {
                        shift += 5;
                        continue;
                    }
                    values[count.min(4)] = if value & 1 == 1 {
                        -(value >> 1)
                    } else {
                        value >> 1
                    };
                    (count, value, shift) = (count + 1, 0, 0);
                }
                generated_column += values[0];
                if count >= 4 {
                    (source, line, column) =
                        (source + values[1], line + values[2], column + values[3]);
                    mappings.push([
                        generated as u32,
                        generated_column as u32,
                        source as u32,
                        line as u32,
                        column as u32,
                    ]);
                }
            }
        }
        Some(Self { sources, mappings })
    }

    fn original(&self, line: u32, column: u32) -> Option<(u32, u32, u32)> {
        let index = self
            .mappings
            .partition_point(|mapping| (mapping[0], mapping[1]) <= (line, column));
        let mapping = self.mappings[..index]
            .last()
            .filter(|mapping| mapping[0] == line)
            .or_else(|| {
                self.mappings
                    .get(index)
                    .filter(|mapping| mapping[0] == line)
            })?;
        Some((mapping[2], mapping[3], mapping[4]))
    }

    fn generated(&self, source: u32, line: u32, column: u32) -> Option<(u32, u32)> {
        self.mappings
            .iter()
            .filter(|mapping| mapping[2] == source && mapping[3] >= line)
            .min_by_key(|mapping| (mapping[3], mapping[4] < column, mapping[0], mapping[1]))
            .map(|mapping| (mapping[0], mapping[1]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_maps_map_both_ways() {
        let map = SourceMap::parse("data:application/json;charset=utf-8,%7B%22sources%22%3A%5B%22%2Fapp%2Fit%27s.ts%22%5D%2C%22mappings%22%3A%22%3BAAAA%2CIAAI%3BAACA%22%7D").unwrap();
        assert_eq!(map.sources, ["/app/it's.ts"]);
        assert_eq!(
            map.mappings,
            [[1, 0, 0, 0, 0], [1, 4, 0, 0, 4], [2, 0, 0, 1, 4]]
        );
        assert_eq!(map.original(1, 5), Some((0, 0, 4)));
        assert_eq!(map.original(2, 9), Some((0, 1, 4)));
        assert_eq!(map.original(0, 0), None);
        assert_eq!(map.generated(0, 0, 3), Some((1, 4)));
        assert_eq!(map.generated(0, 1, 0), Some((2, 0)));
        assert_eq!(map.generated(0, 2, 0), None);
    }
}
