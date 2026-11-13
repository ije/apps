use crate::{
    dom::Document,
    host::{Host, Page, Role},
    jsc::{JavaScriptCore, JsContext, payload::Payload},
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::HashMap,
    fmt::Write,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

enum Event {
    Online,
    // The JSON text of a message. The parent gets it as it is and parses it once.
    Message(String, Vec<bytes::Bytes>),
    Error(Value),
    Exit(i32),
}

struct Parent {
    events: mpsc::SyncSender<Event>,
    wake: crate::host::Waker,
    data: Value,
    id: u64,
    stop: Arc<AtomicBool>,
    received: Vec<bytes::Bytes>,
    code: Option<i32>,
}

pub(crate) fn exit(code: i32) -> bool {
    PARENT.with(|parent| {
        parent.borrow_mut().as_mut().is_some_and(|parent| {
            parent.code = Some(code);
            parent.stop.store(true, Ordering::Release);
            true
        })
    })
}
thread_local! { static PARENT: RefCell<Option<Parent>> = const { RefCell::new(None) }; }

// The buffers of a message, in the order of its arguments. A number is memory that `web.buffer.take` moved or
// shared, and other bytes still belong to the caller, so they are copied.
fn buffers(items: Option<&[Value]>) -> Result<Vec<bytes::Bytes>> {
    let items = items.unwrap_or_default();
    // Take all the held memory first, so that an error does not leave some of it behind.
    let held: Vec<_> = items
        .iter()
        .map(|item| item.as_u64().map(crate::jsc::payload::held))
        .collect();
    items
        .iter()
        .zip(held)
        .map(|(item, held)| match held {
            Some(bytes) => bytes.context("Expected held memory"),
            None => crate::jsc::payload::read(item).context("Expected message bytes")?,
        })
        .collect()
}
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

// An engine that is dropped never polls again, so a subscriber whose queue overflows is removed.
struct Subscriber {
    names: Vec<String>,
    queue: Vec<(String, Payload)>,
    wake: crate::host::Waker,
}
static CHANNELS: std::sync::LazyLock<std::sync::Mutex<HashMap<u64, Subscriber>>> =
    std::sync::LazyLock::new(Default::default);

// The VM of a worker while its thread runs, and the function that stops its script from another thread.
type Running = Arc<std::sync::Mutex<Option<(usize, unsafe extern "C" fn(crate::jsc::Ptr))>>>;

struct Worker {
    input: mpsc::Sender<Payload>,
    events: mpsc::Receiver<Event>,
    stop: Arc<AtomicBool>,
    running: Running,
}
impl Worker {
    fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.input.send(Value::Null.into());
        // The flag stops a script only while it runs. This also ends a wait in `Atomics.wait`.
        let running = self
            .running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((group, terminate)) = *running {
            unsafe { terminate(group as crate::jsc::Ptr) };
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Default)]
pub(crate) struct State {
    workers: HashMap<u64, Worker>,
    namespace: String,
    packaged: bool,
    disabled: u32,
    scripts: Arc<HashMap<String, bytes::Bytes>>,
    channels: Vec<u64>,
}

// A preload context ends without its thread ending, so its subscribers leave with its state.
impl Drop for State {
    fn drop(&mut self) {
        if !self.channels.is_empty() {
            CHANNELS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|id, _| !self.channels.contains(id));
        }
    }
}

impl State {
    pub(crate) fn configure(
        &mut self,
        namespace: &str,
        packaged: bool,
        disabled: u32,
        scripts: Arc<HashMap<String, bytes::Bytes>>,
    ) {
        self.namespace = namespace.to_owned();
        self.packaged = packaged;
        self.disabled = disabled;
        self.scripts = scripts;
    }
    pub(crate) fn call(&mut self, op: &str, args: &mut Value) -> Result<Value> {
        match op {
            "info" => Ok(PARENT.with(|parent| {
                parent.borrow().as_ref().map_or(
                    json!({"main":true,"id":0,"data":null}),
                    |p| json!({"main":false,"id":p.id,"data":p.data}),
                )
            })),
            "post" => PARENT.with(|parent| {
                let buffers = buffers(args.as_array().and_then(|args| args.get(1..)))?;
                let parent = parent.borrow();
                let parent = parent.as_ref().context("Worker has no parent")?;
                let Value::String(text) = args[0].take() else {
                    bail!("Expected worker message");
                };
                ensure!(
                    parent.events.send(Event::Message(text, buffers)).is_ok(),
                    "Worker parent is closed"
                );
                parent.wake.wake();
                Ok(Value::Null)
            }),
            "error" => PARENT.with(|parent| {
                if let Some(parent) = parent.borrow().as_ref() {
                    let _ = parent.events.send(Event::Error(json!({
                        "message": args[0], "filename": args[1], "lineno": args[2], "colno": args[3], "uncaught": true,
                    })));
                    parent.wake.wake();
                }
                Ok(Value::Null)
            }),
            "buffers" => Ok(json!(
                PARENT
                    .with(|parent| parent
                        .borrow_mut()
                        .as_mut()
                        .map(|parent| std::mem::take(&mut parent.received)))
                    .unwrap_or_default()
                    .into_iter()
                    .map(crate::jsc::payload::write)
                    .collect::<Vec<_>>()
            )),
            "close" => {
                PARENT.with(|parent| {
                    if let Some(parent) = parent.borrow_mut().as_mut() {
                        // A worker that closes itself ends cleanly.
                        parent.code.get_or_insert(0);
                        parent.stop.store(true, Ordering::Release);
                    }
                });
                Ok(Value::Null)
            }
            "channel.open" => {
                let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                CHANNELS.lock().unwrap().insert(
                    id,
                    Subscriber {
                        names: Vec::new(),
                        queue: Vec::new(),
                        wake: crate::host::Waker::current(),
                    },
                );
                self.channels.push(id);
                Ok(json!(id))
            }
            "channel.subscribe" => {
                let name = args[1].as_str().context("Expected channel name")?;
                if let Some(subscriber) = CHANNELS
                    .lock()
                    .unwrap()
                    .get_mut(&args[0].as_u64().context("Expected subscriber")?)
                {
                    subscriber.names.retain(|other| other != name);
                    if args[2] == true {
                        subscriber.names.push(name.to_owned());
                    }
                }
                Ok(Value::Null)
            }
            "channel.post" => {
                let id = args[0].as_u64().context("Expected subscriber")?;
                let name = args[1]
                    .as_str()
                    .context("Expected channel name")?
                    .to_owned();
                let mut message = args
                    .as_array_mut()
                    .context("Expected channel message")?
                    .split_off(2);
                let buffers = buffers(message.get(1..))?;
                for (index, item) in message.iter_mut().skip(1).enumerate() {
                    *item = json!({"$appsBytes": index});
                }
                let message = Payload {
                    value: Value::Array(message),
                    buffers,
                };
                for (other, subscriber) in CHANNELS.lock().unwrap().iter_mut() {
                    if *other == id || !subscriber.names.contains(&name) {
                        continue;
                    }
                    // A reader that fell this far behind misses new messages until it reads. It stays
                    // subscribed: only its own end removes it.
                    if subscriber.queue.len() < 4096 {
                        subscriber.queue.push((name.clone(), message.clone()));
                    }
                    subscriber.wake.wake();
                }
                Ok(Value::Null)
            }
            "channel.poll" => {
                let queue = CHANNELS
                    .lock()
                    .unwrap()
                    .get_mut(&args[0].as_u64().context("Expected subscriber")?)
                    .map(|subscriber| std::mem::take(&mut subscriber.queue))
                    .unwrap_or_default();
                Ok(json!(
                    queue
                        .into_iter()
                        .map(|(name, message)| json!({"name": name, "data": message.into_value()}))
                        .collect::<Vec<_>>()
                ))
            }
            "start" => {
                // The worker data comes like a message: its text, then its buffers.
                let received = buffers(args.as_array().and_then(|args| args.get(5..)))?;
                ensure!(self.workers.len() < 64, "Too many workers");
                let source = args[0].as_str().context("Expected worker source")?;
                let source: bytes::Bytes = if args[2] == true {
                    let name = source
                        .strip_prefix("apps:///")
                        .context("Expected a bundled worker URL")?;
                    self.scripts
                        .get(name)
                        .cloned()
                        .with_context(|| format!("Missing bundled worker: {name}"))?
                } else {
                    source.to_owned().into()
                };
                let native = args[1] == true;
                // Web workers must not end when idle: they wait for messages.
                let ends = native && args[3] == true;
                let data = args[4].take();
                let namespace = self.namespace.clone();
                let packaged = self.packaged;
                let disabled = self.disabled;
                let scripts = self.scripts.clone();
                let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                let (input, inbox) = mpsc::channel::<Payload>();
                let (output, events) = mpsc::sync_channel::<Event>(64);
                let stop = Arc::new(AtomicBool::new(false));
                let cancelled = stop.clone();
                let running = Running::default();
                let started = running.clone();
                let wake = crate::host::Waker::current();
                let own = input.clone();
                std::thread::Builder::new().name(format!("apps-worker-{id}")).spawn(move || {
                    crate::host::Waker::install(move || { let _ = own.send(Value::Null.into()); });
                    PARENT.with(|parent| *parent.borrow_mut() = Some(Parent { events: output.clone(), wake: wake.clone(), data, id, stop: cancelled.clone(), received, code: None }));
                    let result = (|| -> Result<()> {
                        let host = Rc::new(RefCell::new(Host { app_name: Some(namespace), packaged, disabled, scripts, ..Host::default() }));
                        let role = if native { Role::Main } else {
                            host.borrow_mut().pages.insert(crate::host::WORKER_PAGE, Page { doc: Document::parse("<html><body></body></html>"), dirty: false });
                            Role::Renderer(crate::host::WORKER_PAGE)
                        };
                        let api = JavaScriptCore::load()?;
                        let engine = JsContext::new_interruptible(api.clone(), host, role, Some(cancelled.clone()))?;
                        // Declared after the engine, so it is cleared before the VM goes away.
                        struct Clear(Running);
                        impl Drop for Clear { fn drop(&mut self) { self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take(); } }
                        *started.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = api.terminate.map(|terminate| (api.group as usize, terminate));
                        let _clear = Clear(started);
                        engine.eval("for (const name of ['document', 'window', 'screen', 'screenX', 'screenY', 'screenLeft', 'screenTop', 'outerWidth', 'outerHeight', 'localStorage', 'sessionStorage']) delete globalThis[name]; globalThis.self=globalThis;", "apps:worker-scope")?;
                        if native { engine.eval("globalThis.require = globalThis.__apps.module.createRequire(); globalThis.onmessage = null;", "apps:worker-modules")?; }
                        let _ = output.send(Event::Online);
                        wake.wake();
                        engine.eval_page(std::str::from_utf8(&source)?, "apps:worker")?;
                        while engine.eval_page(crate::jsc::NEXT, "apps:worker")? == "true" {}
                        while !cancelled.load(Ordering::Acquire) {
                            let delay = engine.tick()?;
                            if ends && delay != 0 && engine.call("__appsAlive", &[])? != "true" { break; }
                            crate::host::idle(true);
                            let wait = if api.shrink_pending() && !(0..1000).contains(&delay) { 1000 } else { delay };
                            let message = if wait < 0 { inbox.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected) } else { inbox.recv_timeout(Duration::from_millis(wait as u64)) };
                            if api.shrink_due() {
                                api.shrink();
                                #[cfg(target_os = "macos")]
                                crate::malloc::flush();
                            }
                            crate::host::idle(false);
                            match message {
                                Ok(message) => for message in std::iter::once(message).chain(inbox.try_iter().take(63)) {
                                    if let Some(text) = message.value[0].as_str() {
                                        PARENT.with(|parent| if let Some(parent) = parent.borrow_mut().as_mut() { parent.received = message.buffers; });
                                        engine.call("__appsWorkerReceive", &[text])?;
                                    }
                                },
                                Err(mpsc::RecvTimeoutError::Timeout) => {}
                                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                            }
                        }
                        Ok(())
                    })();
                    let terminated=cancelled.load(Ordering::Acquire);
                    let code = PARENT.with(|parent| parent.borrow().as_ref().and_then(|parent| parent.code)).unwrap_or(if result.is_ok() && !terminated {0} else {1});
                    if !terminated && let Err(error) = &result { let _ = output.send(Event::Error(json!({"message":format!("{error:#}")}))); }
                    let _ = output.send(Event::Exit(code));
                    wake.wake();
                    PARENT.with(|parent| *parent.borrow_mut() = None);
                })?;
                self.workers.insert(
                    id,
                    Worker {
                        input,
                        events,
                        stop,
                        running,
                    },
                );
                Ok(json!(id))
            }
            "send" => {
                let id = args[0].as_u64().context("Expected worker ID")?;
                let mut message = args
                    .as_array_mut()
                    .context("Expected worker message")?
                    .split_off(1);
                let buffers = buffers(message.get(1..))?;
                message.truncate(1);
                // A worker can end before its parent reads the exit event. As in browsers, the message is dropped.
                if let Some(worker) = self.workers.get(&id) {
                    let _ = worker.input.send(Payload {
                        value: Value::Array(message),
                        buffers,
                    });
                }
                Ok(Value::Null)
            }
            "terminate" => {
                if let Some(worker) = self
                    .workers
                    .get(&args[0].as_u64().context("Expected worker ID")?)
                {
                    worker.stop();
                }
                Ok(Value::Null)
            }
            "poll" => {
                let (mut events, mut tree) = (String::from("["), Vec::new());
                self.workers.retain(|id, worker| {
                    for _ in 0..64 {
                        let event = match worker.events.try_recv() {
                            Ok(event) => event,
                            Err(mpsc::TryRecvError::Empty) => break,
                            Err(mpsc::TryRecvError::Disconnected) => return false,
                        };
                        if events.len() > 1 {
                            events.push(',');
                        }
                        tree.push(Value::Null);
                        let _ = match event {
                            Event::Online => write!(events, r#"{{"kind":"online","id":{id}}}"#),
                            Event::Message(text, buffers) => {
                                let _ = write!(events, r#"{{"kind":"message","id":{id},"data":[{text}"#);
                                if !buffers.is_empty() {
                                    let mut data = vec![Value::Null];
                                    for bytes in buffers {
                                        events.push_str(",null");
                                        data.push(crate::jsc::payload::write(bytes));
                                    }
                                    *tree.last_mut().expect("event") = json!({"data": data});
                                }
                                write!(events, "]}}")
                            }
                            Event::Error(mut error) => {
                                error["kind"] = json!("error");
                                error["id"] = json!(id);
                                write!(events, "{error}")
                            }
                            Event::Exit(code) => {
                                let _ = write!(events, r#"{{"kind":"exit","id":{id},"code":{code}}}"#);
                                return false;
                            }
                        };
                    }
                    true
                });
                events.push(']');
                Ok(crate::jsc::payload::raw(events, Value::Array(tree)))
            }
            _ => bail!("Unsupported worker operation: {op}"),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn dropped_contexts_remove_their_channel_subscribers() {
        let mut state = super::State::default();
        let id = state
            .call("channel.open", &mut serde_json::Value::Null)
            .unwrap()
            .as_u64()
            .unwrap();
        assert!(super::CHANNELS.lock().unwrap().contains_key(&id));
        drop(state);
        assert!(!super::CHANNELS.lock().unwrap().contains_key(&id));
    }
}
