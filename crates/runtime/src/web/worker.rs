use crate::jsc::payload::Payload;
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::HashMap,
    fmt::Write,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
};

#[derive(Clone)]
enum Event {
    Online,
    // The JSON text of a message. The parent gets it as it is and parses it once.
    Message(String, Vec<bytes::Bytes>),
    Error(Value),
    Exit(i32),
}

struct Port {
    events: mpsc::Sender<Event>,
    wake: crate::jsc::Waker,
}
// The connections of a worker: one with ID 0 for a dedicated worker, and one for each `new SharedWorker()` of a
// shared worker, with the ID of its link in the page.
type Ports = Arc<Mutex<HashMap<u64, Port>>>;
// The bundle, the script URL and the name of a shared worker.
type Key = (usize, String, String);

struct Shared {
    id: u64,
    worker: Worker,
    ports: Ports,
}
// The shared workers of the process, which pages of all windows connect to.
static SHARED: std::sync::LazyLock<Mutex<HashMap<Key, Shared>>> =
    std::sync::LazyLock::new(Default::default);

struct Parent {
    ports: Ports,
    // The name of a shared worker.
    name: Option<String>,
    data: Value,
    id: u64,
    stop: Arc<AtomicBool>,
    received: Vec<bytes::Bytes>,
    code: Option<i32>,
}

pub(crate) fn in_worker() -> bool {
    PARENT.with(|parent| parent.borrow().is_some())
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

// Sends an event of the worker on this thread to one connection, or to all with `None`.
fn notify(port: Option<u64>, event: Event) -> bool {
    PARENT.with(|parent| {
        let parent = parent.borrow();
        let Some(parent) = parent.as_ref() else {
            return false;
        };
        let ports = parent.ports.lock().unwrap_or_else(PoisonError::into_inner);
        let send = |port: &Port, event| {
            let sent = port.events.send(event).is_ok();
            port.wake.wake();
            sent
        };
        match port {
            Some(port) => ports.get(&port).is_some_and(|port| send(port, event)),
            None => {
                for port in ports.values() {
                    send(port, event.clone());
                }
                true
            }
        }
    })
}

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

struct Subscriber {
    names: Vec<String>,
    queue: Vec<(String, Payload)>,
    wake: crate::jsc::Waker,
}
static CHANNELS: std::sync::LazyLock<std::sync::Mutex<HashMap<u64, Subscriber>>> =
    std::sync::LazyLock::new(Default::default);

// The VM of a worker while its thread runs, and the function that stops its script from another thread.
pub(crate) type Running = Arc<std::sync::Mutex<Option<crate::jsc::Terminator>>>;

struct Worker {
    input: mpsc::Sender<Payload>,
    stop: Arc<AtomicBool>,
    running: Running,
}

// A connection of this context to a worker.
struct Link {
    events: mpsc::Receiver<Event>,
    input: mpsc::Sender<Payload>,
    owner: Owner,
}
enum Owner {
    // A dedicated worker ends with its link.
    Thread(Worker),
    // A shared worker ends with its last link. The link has this port in the worker.
    Shared(Key, u64),
}
impl Drop for Link {
    fn drop(&mut self) {
        let Owner::Shared(key, id) = &self.owner else {
            return;
        };
        let mut shared = SHARED.lock().unwrap_or_else(PoisonError::into_inner);
        // The entry can be a new worker with the same key, after the old one ended.
        let last = shared.get(key).is_some_and(|entry| {
            let mut ports = entry.ports.lock().unwrap_or_else(PoisonError::into_inner);
            ports.remove(id).is_some() && ports.is_empty()
        });
        if last {
            shared.remove(key);
        } else {
            // The worker closes its side of the port.
            let _ = self.input.send(json!([null, id]).into());
        }
    }
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
        if let Some(terminator) = *running {
            terminator.terminate();
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

// The script of a new worker, for the function that runs it in a JavaScript context on the worker thread.
pub(crate) struct Job {
    pub(crate) app: crate::bundle::AppInfo,
    pub(crate) source: bytes::Bytes,
    // Main process and preload workers get the Node.js APIs.
    pub(crate) native: bool,
    // Ends when it has no work left, like a worker of `node:worker_threads`.
    pub(crate) ends: bool,
    pub(crate) stop: Arc<AtomicBool>,
    pub(crate) inbox: mpsc::Receiver<Payload>,
    pub(crate) running: Running,
}

impl Job {
    // Tells the parent that the worker runs.
    pub(crate) fn online(&self) {
        notify(None, Event::Online);
    }

    // The buffers of the message that the worker receives now.
    pub(crate) fn receive(&self, buffers: Vec<bytes::Bytes>) {
        PARENT.with(|parent| {
            if let Some(parent) = parent.borrow_mut().as_mut() {
                parent.received = buffers;
            }
        });
    }
}

pub(crate) type Run = fn(Job) -> Result<()>;

pub(crate) struct State {
    workers: HashMap<u64, Link>,
    app: crate::bundle::AppInfo,
    channels: Vec<u64>,
    run: Run,
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
    pub(crate) fn new(run: Run) -> Self {
        Self {
            workers: HashMap::new(),
            app: crate::bundle::AppInfo::default(),
            channels: Vec::new(),
            run,
        }
    }

    pub(crate) fn configure(&mut self, app: &crate::bundle::AppInfo) {
        self.app = app.clone();
    }

    // The script of a new worker: a bundled script for its URL, or the text of a data: or blob: URL.
    fn source(&self, source: &Value, bundled: bool, native: bool) -> Result<bytes::Bytes> {
        let source = source.as_str().context("Expected worker source")?;
        ensure!(
            native || bundled || self.app.features.has(crate::bundle::Feature::JsExecute),
            "Page workers from data: and blob: URLs need the jsExecute feature"
        );
        if !bundled {
            return Ok(source.to_owned().into());
        }
        let name = source
            .strip_prefix("apps:///")
            .context("Expected a bundled worker URL")?;
        self.app
            .scripts
            .get(name)
            .cloned()
            .with_context(|| format!("Missing bundled worker: {name}"))
    }

    fn spawn(
        &self,
        source: bytes::Bytes,
        native: bool,
        ends: bool,
        data: Payload,
        ports: Ports,
        key: Option<Key>,
    ) -> Result<(u64, Worker)> {
        // A worker gets the bundled scripts and assets of the app, not its native addons.
        let app = crate::bundle::AppInfo {
            name: Some(self.app.name.as_deref().unwrap_or("apps").into()),
            packaged: self.app.packaged,
            features: self.app.features,
            scripts: self.app.scripts.clone(),
            assets: self.app.assets.clone(),
            ..Default::default()
        };
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let (input, inbox) = mpsc::channel::<Payload>();
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        let running = Running::default();
        let own = input.clone();
        let run = self.run;
        let job = Job {
            app,
            source,
            native,
            ends,
            stop: cancelled.clone(),
            inbox,
            running: running.clone(),
        };
        let name = key.as_ref().map(|key| key.2.clone());
        std::thread::Builder::new()
            .name(format!("apps-worker-{id}"))
            .spawn(move || {
                crate::jsc::Waker::install(move || {
                    let _ = own.send(Value::Null.into());
                });
                PARENT.with(|parent| {
                    *parent.borrow_mut() = Some(Parent {
                        ports,
                        name,
                        data: data.value,
                        id,
                        stop: cancelled.clone(),
                        received: data.buffers,
                        code: None,
                    })
                });
                let result = run(job);
                if let Some(key) = key {
                    let mut shared = SHARED.lock().unwrap_or_else(PoisonError::into_inner);
                    if shared.get(&key).is_some_and(|entry| entry.id == id) {
                        shared.remove(&key);
                    }
                }
                let terminated = cancelled.load(Ordering::Acquire);
                let code = PARENT
                    .with(|parent| parent.borrow().as_ref().and_then(|parent| parent.code))
                    .unwrap_or(if result.is_ok() && !terminated { 0 } else { 1 });
                if !terminated && let Err(error) = &result {
                    notify(None, Event::Error(json!({"message":format!("{error:#}")})));
                }
                notify(None, Event::Exit(code));
                PARENT.with(|parent| *parent.borrow_mut() = None);
            })?;
        Ok((
            id,
            Worker {
                input,
                stop,
                running,
            },
        ))
    }

    pub(crate) fn call(&mut self, op: &str, args: &mut Value) -> Result<Value> {
        match op {
            "info" => Ok(PARENT.with(|parent| {
                parent.borrow().as_ref().map_or(
                    json!({"main":true,"id":0,"data":null}),
                    |p| json!({"main":false,"id":p.id,"data":p.data,"name":p.name}),
                )
            })),
            "post" => {
                let buffers = buffers(args.as_array().and_then(|args| args.get(2..)))?;
                let Value::String(text) = args[1].take() else {
                    bail!("Expected worker message");
                };
                // A page that closed drops the messages of a shared worker to it.
                let shared =
                    PARENT.with(|parent| parent.borrow().as_ref().map(|p| p.name.is_some()));
                ensure!(
                    notify(args[0].as_u64(), Event::Message(text, buffers)) || shared == Some(true),
                    "Worker parent is closed"
                );
                Ok(Value::Null)
            }
            "error" => {
                notify(
                    None,
                    Event::Error(json!({
                        "message": args[0], "filename": args[1], "lineno": args[2], "colno": args[3], "uncaught": true,
                    })),
                );
                Ok(Value::Null)
            }
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
                CHANNELS
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(
                        id,
                        Subscriber {
                            names: Vec::new(),
                            queue: Vec::new(),
                            wake: crate::jsc::Waker::current(),
                        },
                    );
                self.channels.push(id);
                Ok(json!(id))
            }
            "channel.subscribe" => {
                let name = args[1].as_str().context("Expected channel name")?;
                if let Some(subscriber) = CHANNELS
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
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
                for (other, subscriber) in CHANNELS
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .iter_mut()
                {
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
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
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
                let native = args[1] == true;
                let source = self.source(&args[0], args[2] == true, native)?;
                let (output, events) = mpsc::channel::<Event>();
                let ports = HashMap::from([(
                    0,
                    Port {
                        events: output,
                        wake: crate::jsc::Waker::current(),
                    },
                )]);
                // Web workers must not end when idle: they wait for messages.
                let data = Payload {
                    value: args[4].take(),
                    buffers: received,
                };
                let (id, worker) = self.spawn(
                    source,
                    native,
                    native && args[3] == true,
                    data,
                    Arc::new(Mutex::new(ports)),
                    None,
                )?;
                let input = worker.input.clone();
                self.workers.insert(
                    id,
                    Link {
                        events,
                        input,
                        owner: Owner::Thread(worker),
                    },
                );
                Ok(json!(id))
            }
            "shared" => {
                ensure!(self.workers.len() < 64, "Too many workers");
                let url = args[0].as_str().context("Expected worker URL")?;
                let key: Key = (
                    Arc::as_ptr(&self.app.scripts) as usize,
                    url.to_owned(),
                    args[1].as_str().unwrap_or_default().to_owned(),
                );
                let port = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                let (output, events) = mpsc::channel::<Event>();
                let target = Port {
                    events: output,
                    wake: crate::jsc::Waker::current(),
                };
                let mut shared = SHARED.lock().unwrap_or_else(PoisonError::into_inner);
                let input = match shared.get(&key) {
                    Some(entry) => {
                        entry
                            .ports
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .insert(port, target);
                        entry.worker.input.clone()
                    }
                    None => {
                        let source = self.source(&args[2], args[3] == true, false)?;
                        let ports = Arc::new(Mutex::new(HashMap::from([(port, target)])));
                        let (id, worker) = self.spawn(
                            source,
                            false,
                            false,
                            Value::Null.into(),
                            ports.clone(),
                            Some(key.clone()),
                        )?;
                        let input = worker.input.clone();
                        shared.insert(key.clone(), Shared { id, worker, ports });
                        input
                    }
                };
                drop(shared);
                // The worker fires `connect` for this message.
                let _ = input.send(json!([port]).into());
                self.workers.insert(
                    port,
                    Link {
                        events,
                        input,
                        owner: Owner::Shared(key, port),
                    },
                );
                Ok(json!(port))
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
                if let Some(link) = self.workers.get(&id) {
                    if let Owner::Shared(_, port) = link.owner {
                        message.push(json!(port));
                    }
                    let _ = link.input.send(Payload {
                        value: Value::Array(message),
                        buffers,
                    });
                }
                Ok(Value::Null)
            }
            "terminate" => {
                if let Some(Link {
                    owner: Owner::Thread(worker),
                    ..
                }) = self
                    .workers
                    .get(&args[0].as_u64().context("Expected worker ID")?)
                {
                    worker.stop();
                }
                Ok(Value::Null)
            }
            "poll" => {
                let (mut events, mut tree) = (String::from("["), Vec::new());
                self.workers.retain(|id, link| {
                    for _ in 0..64 {
                        let event = match link.events.try_recv() {
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
                                let _ = write!(
                                    events,
                                    r#"{{"kind":"message","id":{id},"data":[{text}"#
                                );
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
                                let _ =
                                    write!(events, r#"{{"kind":"exit","id":{id},"code":{code}}}"#);
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
        let mut state = super::State::new(|_| Ok(()));
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
