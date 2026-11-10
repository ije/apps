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
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

struct Parent {
    events: mpsc::SyncSender<Payload>,
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
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

// An engine that is dropped never polls again, so a subscriber whose queue overflows is removed.
struct Subscriber {
    names: Vec<String>,
    queue: Vec<(String, Payload)>,
    wake: crate::host::Waker,
    thread: std::thread::ThreadId,
}
static CHANNELS: std::sync::LazyLock<std::sync::Mutex<HashMap<u64, Subscriber>>> =
    std::sync::LazyLock::new(Default::default);

struct Worker {
    input: mpsc::Sender<Payload>,
    events: mpsc::Receiver<Payload>,
    stop: Arc<AtomicBool>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.input.send(Value::Null.into());
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
                let parent = parent.borrow();
                let parent = parent.as_ref().context("Worker has no parent")?;
                parent
                    .events
                    .send(Payload::snapshot(
                        json!({"kind":"message","data":args.take()}),
                    ))
                    .context("Worker parent is closed")?;
                parent.wake.wake();
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
                PARENT.with(|p| {
                    if let Some(p) = p.borrow().as_ref() {
                        p.stop.store(true, Ordering::Release);
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
                        thread: std::thread::current().id(),
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
                let message = Payload::snapshot(Value::Array(
                    args.as_array_mut()
                        .context("Expected channel message")?
                        .split_off(2),
                ));
                CHANNELS.lock().unwrap().retain(|other, subscriber| {
                    if *other == id || !subscriber.names.contains(&name) {
                        return true;
                    }
                    subscriber.queue.push((name.clone(), message.clone()));
                    subscriber.wake.wake();
                    subscriber.queue.len() < 4096
                });
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
                ensure!(self.workers.len() < 64, "Too many workers");
                let source = args[0].as_str().context("Expected worker source")?;
                let source: bytes::Bytes = if args[3] == true {
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
                let data = args[2].take();
                let namespace = self.namespace.clone();
                let packaged = self.packaged;
                let disabled = self.disabled;
                let scripts = self.scripts.clone();
                let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                let (input, inbox) = mpsc::channel::<Payload>();
                let (output, events) = mpsc::sync_channel::<Payload>(64);
                let stop = Arc::new(AtomicBool::new(false));
                let cancelled = stop.clone();
                let wake = crate::host::Waker::current();
                let own = input.clone();
                std::thread::Builder::new().name(format!("apps-worker-{id}")).spawn(move || {
                    crate::host::Waker::install(move || { let _ = own.send(Value::Null.into()); });
                    PARENT.with(|parent| *parent.borrow_mut() = Some(Parent { events: output.clone(), wake: wake.clone(), data, id, stop: cancelled.clone(), received: Vec::new(), code: None }));
                    let result = (|| -> Result<()> {
                        let host = Rc::new(RefCell::new(Host::default()));
                        host.borrow_mut().app_name=Some(namespace);
                        host.borrow_mut().packaged=packaged;
                        host.borrow_mut().disabled=disabled;
                        host.borrow_mut().scripts=scripts;
                        let role = if native { Role::Main } else {
                            host.borrow_mut().pages.insert(crate::host::WORKER_PAGE, Page { doc: Document::parse("<html><body></body></html>"), dirty: false });
                            Role::Renderer(crate::host::WORKER_PAGE)
                        };
                        let api = JavaScriptCore::load()?;
                        let engine = JsContext::new_interruptible(api.clone(), host, role, Some(cancelled.clone()))?;
                        engine.eval("delete globalThis.document; delete globalThis.window; delete globalThis.screen; delete globalThis.localStorage; delete globalThis.sessionStorage; globalThis.self=globalThis;", "apps:worker-scope")?;
                        if native { engine.eval("globalThis.require = globalThis.__apps.module.createRequire();", "apps:worker-modules")?; }
                        let _ = output.send(json!({"kind":"online"}).into());
                        wake.wake();
                        engine.eval_page(std::str::from_utf8(&source)?, "apps:worker")?;
                        while engine.eval_page(crate::jsc::NEXT, "apps:worker")? == "true" {}
                        while !cancelled.load(Ordering::Acquire) {
                            let delay = engine.tick()?;
                            if native && delay != 0 && engine.call("__appsAlive", &[])? != "true" { break; }
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
                    if !terminated && let Err(error) = &result { let _ = output.send(json!({"kind":"error","message":format!("{error:#}")}).into()); }
                    let thread = std::thread::current().id();
                    CHANNELS.lock().unwrap().retain(|_, subscriber| subscriber.thread != thread);
                    let _ = output.send(json!({"kind":"exit","code":code}).into());
                    wake.wake();
                    PARENT.with(|parent| *parent.borrow_mut() = None);
                })?;
                self.workers.insert(
                    id,
                    Worker {
                        input,
                        events,
                        stop,
                    },
                );
                Ok(json!(id))
            }
            "send" => {
                let worker = self
                    .workers
                    .get(&args[0].as_u64().context("Expected worker ID")?)
                    .context("Worker is closed")?;
                let message = args
                    .as_array_mut()
                    .context("Expected worker message")?
                    .split_off(1);
                worker
                    .input
                    .send(Payload::snapshot(Value::Array(message)))
                    .context("Worker is closed")?;
                Ok(Value::Null)
            }
            "terminate" => {
                if let Some(worker) = self
                    .workers
                    .get(&args[0].as_u64().context("Expected worker ID")?)
                {
                    worker.stop.store(true, Ordering::Release);
                    let _ = worker.input.send(Value::Null.into());
                }
                Ok(Value::Null)
            }
            "poll" => {
                let mut events = Vec::new();
                self.workers.retain(|id, worker| {
                    for _ in 0..64 {
                        match worker.events.try_recv() {
                            Ok(event) => {
                                let mut event = event.into_value();
                                let done = event["kind"] == "exit";
                                event["id"] = json!(id);
                                events.push(event);
                                if done {
                                    return false;
                                }
                            }
                            Err(mpsc::TryRecvError::Empty) => break,
                            Err(mpsc::TryRecvError::Disconnected) => return false,
                        }
                    }
                    true
                });
                Ok(json!(events))
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
