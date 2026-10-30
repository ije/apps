use crate::{
    dom::Document,
    host::{Host, Page, Role},
    jsc::{Api, Engine},
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
    events: mpsc::SyncSender<Value>,
    wake: crate::host::Waker,
    data: Value,
    id: u64,
    stop: Arc<AtomicBool>,
}
thread_local! { static PARENT: RefCell<Option<Parent>> = const { RefCell::new(None) }; }
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct Worker {
    input: mpsc::Sender<Value>,
    events: mpsc::Receiver<Value>,
    stop: Arc<AtomicBool>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.input.send(Value::Null);
    }
}

#[derive(Default)]
pub(crate) struct State {
    workers: HashMap<u64, Worker>,
    namespace: String,
    packaged: bool,
    disabled: u32,
    scripts: Arc<HashMap<String, Arc<str>>>,
}

impl State {
    pub(crate) fn configure(
        &mut self,
        namespace: &str,
        packaged: bool,
        disabled: u32,
        scripts: Arc<HashMap<String, Arc<str>>>,
    ) {
        self.namespace = namespace.to_owned();
        self.packaged = packaged;
        self.disabled = disabled;
        self.scripts = scripts;
    }
    pub(crate) fn call(&mut self, op: &str, args: &Value) -> Result<Value> {
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
                    .send(json!({"kind":"message","data":args[0]}))
                    .context("Worker parent is closed")?;
                parent.wake.wake();
                Ok(Value::Null)
            }),
            "close" => {
                PARENT.with(|p| {
                    if let Some(p) = p.borrow().as_ref() {
                        p.stop.store(true, Ordering::Release);
                    }
                });
                Ok(Value::Null)
            }
            "start" => {
                ensure!(self.workers.len() < 64, "Too many workers");
                let source = args[0].as_str().context("Expected worker source")?;
                let source: Arc<str> = if args[3] == true {
                    let name = source
                        .strip_prefix("apps:///")
                        .context("Expected a bundled worker URL")?;
                    self.scripts
                        .get(name)
                        .cloned()
                        .with_context(|| format!("Missing bundled worker: {name}"))?
                } else {
                    source.into()
                };
                let native = args[1] == true;
                let data = args[2].clone();
                let namespace = self.namespace.clone();
                let packaged = self.packaged;
                let disabled = self.disabled;
                let scripts = self.scripts.clone();
                let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                let (input, inbox) = mpsc::channel::<Value>();
                let (output, events) = mpsc::sync_channel::<Value>(64);
                let stop = Arc::new(AtomicBool::new(false));
                let cancelled = stop.clone();
                let wake = crate::host::Waker::current();
                let own = input.clone();
                std::thread::Builder::new().name(format!("apps-worker-{id}")).spawn(move || {
                    crate::host::Waker::install(move || { let _ = own.send(Value::Null); });
                    PARENT.with(|parent| *parent.borrow_mut() = Some(Parent { events: output.clone(), wake: wake.clone(), data, id, stop: cancelled.clone() }));
                    let result = (|| -> Result<()> {
                        let host = Rc::new(RefCell::new(Host::default()));
                        host.borrow_mut().app_name=Some(namespace);
                        host.borrow_mut().packaged=packaged;
                        host.borrow_mut().disabled=disabled;
                        host.borrow_mut().scripts=scripts;
                        let role = if native { Role::Main } else {
                            host.borrow_mut().pages.insert(0, Page { doc: Document::parse("<html><body></body></html>"), dirty: false });
                            Role::Renderer(0)
                        };
                        let engine = Engine::new_interruptible(Api::load()?, host, role, Some(cancelled.clone()))?;
                        engine.eval("delete globalThis.document; delete globalThis.window; delete globalThis.localStorage; delete globalThis.sessionStorage; globalThis.self=globalThis;", "apps:worker-scope")?;
                        if native { engine.eval("globalThis.require = name => { const key = name.replace(/^node:/, ''); const value = globalThis.__apps[key]; if (!value) throw new Error('Unsupported worker module: ' + name); return value; };", "apps:worker-modules")?; }
                        engine.eval_page(&source, "apps:worker")?;
                        let _ = output.send(json!({"kind":"online"}));
                        wake.wake();
                        while !cancelled.load(Ordering::Acquire) {
                            let delay = engine.tick()?;
                            if native && delay < 0 && engine.eval("__appsAlive()", "apps:worker")? != "true" { break; }
                            let message = if delay < 0 { inbox.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected) } else { inbox.recv_timeout(Duration::from_millis(delay as u64)) };
                            match message {
                                Ok(message) => for message in std::iter::once(message).chain(inbox.try_iter().take(63)) {
                                    if !message.is_null() { engine.call("__appsWorkerReceive", &[&message.to_string()])?; }
                                },
                                Err(mpsc::RecvTimeoutError::Timeout) => {}
                                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                            }
                        }
                        Ok(())
                    })();
                    let terminated=cancelled.load(Ordering::Acquire);
                    if !terminated && let Err(error) = &result { let _ = output.send(json!({"kind":"error","message":format!("{error:#}")})); }
                    let _ = output.send(json!({"kind":"exit","code":if result.is_ok() && !terminated {0} else {1}}));
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
                worker
                    .input
                    .send(args[1].clone())
                    .context("Worker is closed")?;
                Ok(Value::Null)
            }
            "terminate" => {
                if let Some(worker) = self
                    .workers
                    .get(&args[0].as_u64().context("Expected worker ID")?)
                {
                    worker.stop.store(true, Ordering::Release);
                    let _ = worker.input.send(Value::Null);
                }
                Ok(Value::Null)
            }
            "poll" => {
                let mut events = Vec::new();
                self.workers.retain(|id, worker| {
                    for _ in 0..64 {
                        match worker.events.try_recv() {
                            Ok(mut event) => {
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
