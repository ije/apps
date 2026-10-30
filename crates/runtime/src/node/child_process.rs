use crate::host::{Role, Waker};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{self, Read, Write},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    time::{Duration, Instant},
};

enum Event {
    Data(&'static str, Vec<u8>),
    End(&'static str),
    Error(&'static str, io::Error),
    Written(usize),
    Exit,
}

struct Process {
    child: Child,
    input: Option<Sender<Option<Vec<u8>>>>,
    events: Receiver<Event>,
    readers: Vec<(&'static str, Arc<AtomicBool>, std::thread::Thread)>,
    status: Option<ExitStatus>,
    open_streams: usize,
    spawned: bool,
    signal: Option<String>,
    detached: bool,
    unreferenced: bool,
}

impl Process {
    fn exit(&mut self, status: ExitStatus) {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            self.signal = status.signal().map(|signal| {
                crate::node::SIGNALS
                    .iter()
                    .find(|(_, number)| *number == signal)
                    .map_or_else(|| format!("SIG{signal}"), |(name, _)| (*name).into())
            });
        }
        self.status = Some(status);
        self.input.take();
    }

    fn code(&self) -> Option<i32> {
        self.status
            .filter(|_| self.signal.is_none())
            .and_then(|status| status.code())
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.input.take();
        for (_, paused, reader) in &self.readers {
            paused.store(false, Ordering::Release);
            reader.unpark();
        }
        if self.status.is_none() && !(self.detached && self.unreferenced) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn error_value(error: &io::Error) -> Value {
    json!({
        "message": error.to_string(),
        "code": crate::node::code(error),
        "errno": error.raw_os_error().map(|code| -code.abs()),
    })
}

#[derive(Default)]
pub(crate) struct State {
    children: HashMap<u64, Process>,
    next_id: u64,
}

impl State {
    pub(crate) fn call(&mut self, role: Role, op: &str, args: &Value) -> Result<Value> {
        ensure!(
            !matches!(role, Role::Renderer(_)),
            "Child processes are unavailable in page scripts"
        );
        match op {
            "sync" => {
                let mut state = State::default();
                let mut config = args[2].clone();
                if let Some(stdio) = config["stdio"].as_str() {
                    config["stdio"] = json!([stdio, stdio, stdio]);
                }
                if config["stdio"].is_null() {
                    config["stdio"] = json!(["pipe", "pipe", "pipe"]);
                }
                let spawned = state.call(role, "spawn", &json!([args[0], args[1], config]))?;
                if !spawned["error"].is_null() {
                    return Ok(spawned);
                }
                let id = spawned["id"].as_u64().context("Expected child ID")?;
                if config["stdio"][0] == "pipe" {
                    if !args[2]["input"].is_null() {
                        state.call(role, "write", &json!([id, args[2]["input"]]))?;
                    }
                    state.call(role, "end", &json!([id]))?;
                }
                let deadline = args[2]["timeout"]
                    .as_u64()
                    .filter(|v| *v > 0)
                    .map(|v| Instant::now() + Duration::from_millis(v));
                let limit = args[2]["maxBuffer"].as_u64().unwrap_or(1024 * 1024) as usize;
                let (mut stdout, mut stderr) = (Vec::<u8>::new(), Vec::<u8>::new());
                let process = state
                    .children
                    .get_mut(&id)
                    .context("Child process is closed")?;
                let error = loop {
                    if process.status.is_some() && process.open_streams == 0 {
                        break None;
                    }
                    let event = match deadline {
                        Some(deadline) => process
                            .events
                            .recv_timeout(deadline.saturating_duration_since(Instant::now())),
                        None => process
                            .events
                            .recv()
                            .map_err(|_| RecvTimeoutError::Disconnected),
                    };
                    match event {
                        Ok(Event::Data(stream, data)) => {
                            let output = if stream == "stdout" {
                                &mut stdout
                            } else {
                                &mut stderr
                            };
                            output.extend(data);
                            if output.len() > limit {
                                break Some("ENOBUFS");
                            }
                        }
                        Ok(Event::End(_)) => process.open_streams -= 1,
                        Ok(Event::Exit) | Err(RecvTimeoutError::Disconnected)
                            if process.status.is_none() =>
                        {
                            let status = process.child.wait()?;
                            process.exit(status);
                        }
                        Err(RecvTimeoutError::Disconnected) => break None,
                        Err(RecvTimeoutError::Timeout) => break Some("ETIMEDOUT"),
                        Ok(_) => {}
                    }
                };
                let (status, signal) = (process.code(), process.signal.clone());
                let output = |data: &[u8], index: usize| {
                    if config["stdio"][index] == "pipe" {
                        crate::web::base64(data)
                    } else {
                        Value::Null
                    }
                };
                let mut result = json!({"pid":spawned["pid"],"stdout":output(&stdout, 1),"stderr":output(&stderr, 2),"status":status,"signal":signal});
                if let Some(code) = error {
                    let _ = state.call(role, "kill", &json!([id, "SIGKILL"]));
                    result["status"] = Value::Null;
                    result["signal"] = "SIGKILL".into();
                    result["error"] = json!({"code":code,"message":"Child process exceeded its output or time limit"});
                }
                Ok(result)
            }
            "unref" => {
                if let Some(process) = self
                    .children
                    .get_mut(&args[0].as_u64().context("Expected child ID")?)
                {
                    process.unreferenced = args[1] == true;
                }
                Ok(Value::Null)
            }
            "spawn" => {
                let command = args[0].as_str().context("Expected command")?;
                let arguments: Vec<String> = serde_json::from_value(args[1].clone())?;
                let options = &args[2];
                let mut builder = Command::new(command);
                builder.args(&arguments);
                #[cfg(unix)]
                if options["detached"] == true {
                    use std::os::unix::process::CommandExt;
                    builder.process_group(0);
                }
                #[cfg(windows)]
                if options["windowsHide"] == true {
                    use std::os::windows::process::CommandExt;
                    builder.creation_flags(0x08000000);
                }
                if let Some(cwd) = options["cwd"].as_str() {
                    builder.current_dir(cwd);
                }
                if let Some(env) = options["env"].as_object() {
                    builder.env_clear();
                    for (name, value) in env {
                        builder.env(
                            name,
                            value
                                .as_str()
                                .map_or_else(|| value.to_string(), str::to_owned),
                        );
                    }
                }
                for index in 0..3 {
                    let stdio = match options["stdio"][index].as_str().unwrap_or("pipe") {
                        "pipe" => Stdio::piped(),
                        "ignore" => Stdio::null(),
                        "inherit" => Stdio::inherit(),
                        _ => bail!("Unsupported stdio option"),
                    };
                    match index {
                        0 => builder.stdin(stdio),
                        1 => builder.stdout(stdio),
                        _ => builder.stderr(stdio),
                    };
                }
                let wake = Waker::current();
                let child = match builder.spawn() {
                    Ok(child) => child,
                    Err(error) => {
                        wake.wake();
                        return Ok(json!({"error": error_value(&error)}));
                    }
                };
                // WARNING: Small queues and batches keep paused output near the stream high water mark.
                let (sender, receiver) = mpsc::sync_channel(8);
                let mut process = Process {
                    child,
                    input: None,
                    events: receiver,
                    readers: Vec::new(),
                    status: None,
                    open_streams: 0,
                    spawned: false,
                    signal: None,
                    detached: options["detached"] == true,
                    unreferenced: false,
                };
                let outputs: [(&str, Option<Box<dyn Read + Send>>); 2] = [
                    (
                        "stdout",
                        process
                            .child
                            .stdout
                            .take()
                            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
                    ),
                    (
                        "stderr",
                        process
                            .child
                            .stderr
                            .take()
                            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
                    ),
                ];
                for (name, pipe) in outputs {
                    if let Some(mut pipe) = pipe {
                        process.open_streams += 1;
                        let (sender, wake) = (sender.clone(), wake.clone());
                        let paused = Arc::new(AtomicBool::new(false));
                        let gate = paused.clone();
                        let reader = std::thread::Builder::new()
                            .name(format!("apps-{name}"))
                            .spawn(move || {
                                let mut bytes = [0; 16384];
                                loop {
                                    while gate.load(Ordering::Acquire) {
                                        std::thread::park();
                                    }
                                    match pipe.read(&mut bytes) {
                                        Ok(0) => break,
                                        Ok(size) => {
                                            if sender
                                                .send(Event::Data(name, bytes[..size].to_vec()))
                                                .is_err()
                                            {
                                                return;
                                            }
                                            wake.wake();
                                        }
                                        Err(error)
                                            if error.kind() == io::ErrorKind::Interrupted =>
                                        {
                                            continue;
                                        }
                                        Err(error) => {
                                            let _ = sender.send(Event::Error(name, error));
                                            break;
                                        }
                                    }
                                }
                                if sender.send(Event::End(name)).is_ok() {
                                    wake.wake();
                                }
                            })?;
                        process
                            .readers
                            .push((name, paused, reader.thread().clone()));
                    }
                }
                #[cfg(unix)]
                let exit = process.child.id();
                #[cfg(windows)]
                let exit = {
                    use std::os::windows::io::AsHandle;
                    process.child.as_handle().try_clone_to_owned()?
                };
                let (exited, waker) = (sender.clone(), wake.clone());
                crate::node::on_exit(exit, move || {
                    if exited.send(Event::Exit).is_ok() {
                        waker.wake();
                    }
                })?;
                if let Some(mut pipe) = process.child.stdin.take() {
                    let (input, writes) = mpsc::channel::<Option<Vec<u8>>>();
                    process.input = Some(input);
                    process.open_streams += 1;
                    let wake = wake.clone();
                    std::thread::Builder::new()
                        .name("apps-stdin".into())
                        .spawn(move || {
                            while let Ok(Some(bytes)) = writes.recv() {
                                if let Err(error) = pipe.write_all(&bytes) {
                                    let _ = sender.send(Event::Error("stdin", error));
                                    break;
                                }
                                if sender.send(Event::Written(bytes.len())).is_err() {
                                    return;
                                }
                                wake.wake();
                            }
                            drop(pipe);
                            if sender.send(Event::End("stdin")).is_ok() {
                                wake.wake();
                            }
                        })?;
                }
                self.next_id += 1;
                let pid = process.child.id();
                self.children.insert(self.next_id, process);
                wake.wake();
                Ok(json!({"id": self.next_id, "pid": pid}))
            }
            "pause" => {
                if let Some(process) = self
                    .children
                    .get_mut(&args[0].as_u64().context("Expected child ID")?)
                    && let Some((_, paused, reader)) =
                        process.readers.iter().find(|(name, ..)| args[1] == *name)
                {
                    paused.store(args[2] == true, Ordering::Release);
                    reader.unpark();
                }
                Ok(Value::Null)
            }
            "poll" => {
                let mut events = Vec::new();
                let mut closed = Vec::new();
                for (&id, process) in &mut self.children {
                    if !process.spawned {
                        process.spawned = true;
                        events.push(json!({"id": id, "event": "spawn"}));
                    }
                    for (index, event) in process.events.try_iter().take(8).enumerate() {
                        if index == 7 {
                            Waker::current().wake();
                        }
                        events.push(match event {
                            Event::Data(stream, data) => json!({"id": id, "event": "data", "stream": stream, "data": crate::jsc::payload::write(data)}),
                            Event::Error(stream, error) => json!({"id": id, "event": "error", "stream": stream, "error": error_value(&error)}),
                            Event::Written(bytes) => json!({"id": id, "event": "written", "bytes": bytes}),
                            Event::End(stream) => {
                                process.open_streams -= 1;
                                json!({"id": id, "event": if stream == "stdin" { "finish" } else { "end" }, "stream": stream})
                            }
                            Event::Exit => continue,
                        });
                    }
                    if process.status.is_none()
                        && let Some(status) = process.child.try_wait()?
                    {
                        process.exit(status);
                        events.push(json!({"id": id, "event": "exit", "code": process.code(), "signal": process.signal}));
                    }
                    if process.status.is_some() && process.open_streams == 0 {
                        events.push(json!({"id": id, "event": "close", "code": process.code(), "signal": process.signal}));
                        closed.push(id);
                    }
                }
                for id in closed {
                    self.children.remove(&id);
                }
                Ok(json!(events))
            }
            "write" | "end" => {
                let process = self
                    .children
                    .get_mut(&args[0].as_u64().context("Expected child ID")?)
                    .context("Child process is closed")?;
                let input = process.input.as_ref().context("Child stdin is closed")?;
                if op == "end" {
                    input.send(None)?;
                    process.input.take();
                    return Ok(Value::Null);
                }
                let bytes = crate::node::bytes(&args[1])?;
                let size = bytes.len();
                input.send(Some(bytes.to_vec()))?;
                Ok(json!(size))
            }
            "kill" => {
                let Some(process) = self
                    .children
                    .get_mut(&args[0].as_u64().context("Expected child ID")?)
                else {
                    return Ok(json!(false));
                };
                if process.status.is_some() {
                    return Ok(json!(false));
                }
                let signal = args[1].as_str().context("Expected signal")?;
                #[cfg(unix)]
                {
                    let number = crate::node::SIGNALS
                        .iter()
                        .find(|(name, _)| *name == signal)
                        .with_context(|| format!("Unknown signal: {signal}"))?
                        .1;
                    // WARNING: Only signal a child that this context owns and has not reaped.
                    if unsafe {
                        libc::kill(
                            if process.detached {
                                -(process.child.id() as libc::pid_t)
                            } else {
                                process.child.id() as libc::pid_t
                            },
                            number,
                        )
                    } != 0
                    {
                        let error = io::Error::last_os_error();
                        if error.raw_os_error() == Some(libc::ESRCH) {
                            return Ok(json!(false));
                        }
                        return Err(error.into());
                    }
                }
                #[cfg(not(unix))]
                {
                    ensure!(
                        matches!(signal, "SIGTERM" | "SIGKILL" | "SIGINT"),
                        "Unsupported signal: {signal}"
                    );
                    process.child.kill()?;
                    process.signal = Some(signal.into());
                }
                Ok(json!(true))
            }
            _ => bail!("Unsupported child process operation: {op}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_process_renderer_access_is_denied() {
        let mut state = State::default();
        for op in ["spawn", "poll", "write", "end", "kill"] {
            assert!(
                state
                    .call(Role::Renderer(1), op, &json!([]))
                    .unwrap_err()
                    .to_string()
                    .contains("unavailable in page scripts")
            );
        }
    }
}
