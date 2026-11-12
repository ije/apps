use crate::host::{Role, Waker};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{self, Read, Write},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender},
    },
    time::{Duration, Instant},
};

enum Event {
    Data(&'static str, Vec<u8>),
    End(&'static str),
    Error(&'static str, io::Error),
    Written(&'static str),
    #[cfg(unix)]
    Finish(&'static str),
    Exit,
}

struct Process {
    child: Option<Child>,
    input: Option<Sender<Option<Vec<u8>>>>,
    extra: Vec<(&'static str, Sender<Option<Vec<u8>>>)>,
    events: Receiver<Event>,
    readers: Vec<(&'static str, Arc<AtomicU8>, std::thread::Thread)>,
    status: Option<ExitStatus>,
    open_streams: usize,
    spawned: bool,
    signal: Option<String>,
    detached: bool,
    unreferenced: bool,
}

impl Process {
    fn new(child: Option<Child>, events: Receiver<Event>, detached: bool) -> Self {
        Self {
            child,
            input: None,
            extra: Vec::new(),
            events,
            readers: Vec::new(),
            status: None,
            open_streams: 0,
            spawned: false,
            signal: None,
            detached,
            unreferenced: false,
        }
    }

    fn reap(&mut self) -> io::Result<()> {
        if let Some(child) = &mut self.child {
            let status = child.wait()?;
            self.exit(status);
        }
        Ok(())
    }

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
        self.extra.clear();
    }

    fn code(&self) -> Option<i32> {
        self.status
            .filter(|_| self.signal.is_none())
            .and_then(|status| status.code())
    }
}

const PAUSED: u8 = 1;
const CLOSED: u8 = 2;

impl Drop for Process {
    fn drop(&mut self) {
        self.input.take();
        self.extra.clear();
        for (_, state, reader) in &self.readers {
            state.store(CLOSED, Ordering::Release);
            reader.unpark();
        }
        if self.status.is_none()
            && !(self.detached && self.unreferenced)
            && let Some(child) = &mut self.child
        {
            let _ = child.kill();
            let _ = child.wait();
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

fn attach(
    process: &mut Process,
    sender: &SyncSender<Event>,
    wake: &Waker,
    #[cfg(unix)] pipes: Vec<(&'static str, std::os::unix::net::UnixStream)>,
    #[allow(unused_mut)] mut outputs: Vec<(&'static str, Box<dyn Read + Send>)>,
) -> io::Result<()> {
    #[cfg(unix)]
    for (name, pipe) in pipes {
        let mut writer = pipe.try_clone()?;
        outputs.push((name, Box::new(pipe)));
        let (input, writes) = mpsc::channel::<Option<Vec<u8>>>();
        process.extra.push((name, input));
        process.open_streams += 1;
        let (sender, wake) = (sender.clone(), wake.clone());
        std::thread::Builder::new()
            .name(format!("apps-{name}"))
            .spawn(move || {
                while let Ok(Some(bytes)) = writes.recv() {
                    if let Err(error) = writer.write_all(&bytes) {
                        let _ = sender.send(Event::Error(name, error));
                        break;
                    }
                    if sender.send(Event::Written(name)).is_err() {
                        return;
                    }
                    wake.wake();
                }
                let _ = writer.shutdown(std::net::Shutdown::Write);
                if sender.send(Event::Finish(name)).is_ok() {
                    wake.wake();
                }
            })?;
    }
    for (name, mut pipe) in outputs {
        process.open_streams += 1;
        let (sender, wake) = (sender.clone(), wake.clone());
        let state = Arc::new(AtomicU8::new(0));
        let gate = state.clone();
        let reader = std::thread::Builder::new()
            .name(format!("apps-{name}"))
            .spawn(move || {
                let mut bytes = [0; 16384];
                loop {
                    while gate.load(Ordering::Acquire) == PAUSED {
                        std::thread::park();
                    }
                    if gate.load(Ordering::Acquire) == CLOSED {
                        break;
                    }
                    match pipe.read(&mut bytes) {
                        Ok(0) => break,
                        Ok(_) if gate.load(Ordering::Acquire) == CLOSED => break,
                        Ok(size) => {
                            if sender
                                .send(Event::Data(name, bytes[..size].to_vec()))
                                .is_err()
                            {
                                return;
                            }
                            wake.wake();
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(error) => {
                            let _ = sender.send(Event::Error(name, error));
                            break;
                        }
                    }
                }
                drop(pipe);
                if sender.send(Event::End(name)).is_ok() {
                    wake.wake();
                }
            })?;
        process.readers.push((name, state, reader.thread().clone()));
    }
    Ok(())
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
                let config = &args[2];
                let spawned = state.call(role, "spawn", args)?;
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
                            let output = match stream {
                                "stdout" => &mut stdout,
                                "stderr" => &mut stderr,
                                _ => continue,
                            };
                            output.extend(data);
                            if output.len() > limit {
                                break Some(("ENOBUFS", libc::ENOBUFS));
                            }
                        }
                        Ok(Event::End(_)) => process.open_streams -= 1,
                        Ok(Event::Exit) | Err(RecvTimeoutError::Disconnected)
                            if process.status.is_none() =>
                        {
                            process.reap()?;
                        }
                        Err(RecvTimeoutError::Disconnected) => break None,
                        Err(RecvTimeoutError::Timeout) => {
                            break Some(("ETIMEDOUT", libc::ETIMEDOUT));
                        }
                        Ok(_) => {}
                    }
                };
                if error.is_some() && process.status.is_none() {
                    let signal = config["killSignal"].as_str().unwrap_or("SIGTERM");
                    let _ = state.call(role, "kill", &json!([id, signal]));
                    let process = state
                        .children
                        .get_mut(&id)
                        .context("Child process is closed")?;
                    process.reap()?;
                }
                let process = state.children.get(&id).context("Child process is closed")?;
                let (status, signal) = (process.code(), process.signal.clone());
                let output = |data: &[u8], index: usize| {
                    if config["stdio"][index] == "pipe" {
                        crate::web::base64(data)
                    } else {
                        Value::Null
                    }
                };
                let mut result = json!({"pid":spawned["pid"],"stdout":output(&stdout, 1),"stderr":output(&stderr, 2),"status":status,"signal":signal});
                if let Some((code, errno)) = error {
                    result["error"] = json!({"code":code,"errno":cfg!(unix).then_some(-errno)});
                }
                Ok(result)
            }
            "unref" => {
                // A child that failed to spawn has no ID.
                if let Some(process) = args[0].as_u64().and_then(|id| self.children.get_mut(&id)) {
                    process.unreferenced = args[1] == true;
                }
                Ok(Value::Null)
            }
            "spawn" => {
                let command = args[0].as_str().context("Expected command")?;
                let arguments: Vec<String> = serde_json::from_value(args[1].clone())?;
                let options = &args[2];
                let mut builder = Command::new(command);
                #[cfg(unix)]
                let mut builder_ends = Vec::new();
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
                #[cfg(unix)]
                {
                    use std::os::unix::process::CommandExt;
                    if let Some(argv0) = options["argv0"].as_str() {
                        builder.arg0(argv0);
                    }
                    if let Some(uid) = options["uid"].as_u64() {
                        builder.uid(u32::try_from(uid)?);
                    }
                    if let Some(gid) = options["gid"].as_u64() {
                        builder.gid(u32::try_from(gid)?);
                    }
                }
                #[cfg(not(unix))]
                ensure!(
                    options["uid"].is_null() && options["gid"].is_null(),
                    "The uid and gid options are not supported on Windows"
                );
                for index in 0..3 {
                    let stdio = match &options["stdio"][index] {
                        Value::Null => Stdio::piped(),
                        Value::String(kind) if kind == "pipe" => Stdio::piped(),
                        Value::String(kind) if kind == "ignore" => Stdio::null(),
                        Value::String(kind) if kind == "inherit" => Stdio::inherit(),
                        fd if fd.as_u64() == Some(index as u64) => Stdio::inherit(),
                        fd if fd == 1 => Stdio::from(io::stdout()),
                        fd if fd == 2 => Stdio::from(io::stderr()),
                        _ => bail!("Unsupported stdio option"),
                    };
                    match index {
                        0 => builder.stdin(stdio),
                        1 => builder.stdout(stdio),
                        _ => builder.stderr(stdio),
                    };
                }
                // Child ends are raised above the fds they become, so one `dup2` can't overwrite another end.
                #[cfg(unix)]
                let mut pipes = Vec::new();
                #[cfg(unix)]
                {
                    use std::os::{
                        fd::{AsRawFd, FromRawFd, OwnedFd},
                        unix::{net::UnixStream, process::CommandExt},
                    };
                    let mut ends = Vec::new();
                    for (index, kind) in options["stdio"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .enumerate()
                        .skip(3)
                    {
                        if kind == "pipe" {
                            let (parent, child) = UnixStream::pair()?;
                            let raised = unsafe {
                                libc::fcntl(
                                    child.as_raw_fd(),
                                    libc::F_DUPFD_CLOEXEC,
                                    64 + index as i32,
                                )
                            };
                            ensure!(raised >= 0, "Cannot create a stdio pipe");
                            ends.push((index as i32, unsafe { OwnedFd::from_raw_fd(raised) }));
                            // Stream names live as long as the process, so each index gets one name.
                            static NAMES: std::sync::Mutex<Vec<&'static str>> =
                                std::sync::Mutex::new(Vec::new());
                            let mut names = NAMES.lock().unwrap_or_else(|error| error.into_inner());
                            while names.len() <= index {
                                let name = format!("fd{}", names.len());
                                names.push(Box::leak(name.into_boxed_str()));
                            }
                            pipes.push((names[index], parent));
                        }
                    }
                    if !ends.is_empty() {
                        let targets: Vec<_> = ends
                            .iter()
                            .map(|(index, end)| (*index, end.as_raw_fd()))
                            .collect();
                        unsafe {
                            builder.pre_exec(move || {
                                for &(target, end) in &targets {
                                    if libc::dup2(end, target) < 0 {
                                        return Err(io::Error::last_os_error());
                                    }
                                }
                                Ok(())
                            });
                        }
                        // The child ends stay open until `spawn` below, then close in this process.
                        builder_ends = ends;
                    }
                }
                let wake = Waker::current();
                let mut child = match builder.spawn() {
                    Ok(child) => child,
                    Err(error) => {
                        wake.wake();
                        return Ok(json!({"error": error_value(&error)}));
                    }
                };
                #[cfg(unix)]
                drop(std::mem::take(&mut builder_ends));
                let (sender, receiver) = mpsc::sync_channel(8);
                let outputs: Vec<(&'static str, Box<dyn Read + Send>)> = [
                    child
                        .stdout
                        .take()
                        .map(|pipe| ("stdout", Box::new(pipe) as _)),
                    child
                        .stderr
                        .take()
                        .map(|pipe| ("stderr", Box::new(pipe) as _)),
                ]
                .into_iter()
                .flatten()
                .collect();
                let stdin = child.stdin.take();
                #[cfg(unix)]
                let exit = child.id();
                #[cfg(windows)]
                let exit = {
                    use std::os::windows::io::AsHandle;
                    child.as_handle().try_clone_to_owned()?
                };
                let pid = child.id();
                let mut process = Process::new(Some(child), receiver, options["detached"] == true);
                attach(
                    &mut process,
                    &sender,
                    &wake,
                    #[cfg(unix)]
                    pipes,
                    outputs,
                )?;
                let (exited, waker) = (sender.clone(), wake.clone());
                crate::node::on_exit(exit, move || {
                    if exited.send(Event::Exit).is_ok() {
                        waker.wake();
                    }
                })?;
                if let Some(mut pipe) = stdin {
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
                                if sender.send(Event::Written("stdin")).is_err() {
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
                self.children.insert(self.next_id, process);
                wake.wake();
                Ok(json!({"id": self.next_id, "pid": pid}))
            }
            "pause" | "destroy" => {
                if let Some(process) = args[0].as_u64().and_then(|id| self.children.get(&id))
                    && let Some((_, state, reader)) =
                        process.readers.iter().find(|(name, ..)| args[1] == *name)
                {
                    let next = if op == "destroy" {
                        CLOSED
                    } else {
                        u8::from(args[2] == true)
                    };
                    let _ = state.try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                        (current != CLOSED).then_some(next)
                    });
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
                            Event::Written(stream) => json!({"id": id, "event": "written", "stream": stream}),
                            #[cfg(unix)]
                            Event::Finish(stream) => {
                                process.open_streams -= 1;
                                json!({"id": id, "event": "finish", "stream": stream})
                            }
                            Event::End(stream) => {
                                process.open_streams -= 1;
                                json!({"id": id, "event": if stream == "stdin" { "finish" } else { "end" }, "stream": stream})
                            }
                            Event::Exit => continue,
                        });
                    }
                    if process.status.is_none()
                        && let Some(child) = &mut process.child
                        && let Some(status) = child.try_wait()?
                    {
                        process.exit(status);
                        events.push(json!({"id": id, "event": "exit", "code": process.code(), "signal": process.signal}));
                    }
                    if (process.status.is_some() || process.child.is_none())
                        && process.open_streams == 0
                    {
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
                let name = if op == "end" { &args[1] } else { &args[2] };
                if let Some(name) = name.as_str() {
                    let index = process
                        .extra
                        .iter()
                        .position(|(stream, _)| *stream == name)
                        .context("Child stream is closed")?;
                    if op == "end" {
                        process.extra.remove(index).1.send(None)?;
                        return Ok(Value::Null);
                    }
                    let bytes = crate::node::bytes(&args[1])?;
                    process.extra[index].1.send(Some(bytes.to_vec()))?;
                    return Ok(Value::Null);
                }
                let input = process.input.as_ref().context("Child stdin is closed")?;
                if op == "end" {
                    input.send(None)?;
                    process.input.take();
                    return Ok(Value::Null);
                }
                input.send(Some(crate::node::bytes(&args[1])?.to_vec()))?;
                Ok(Value::Null)
            }
            "kill" => {
                let Some(process) = self
                    .children
                    .get_mut(&args[0].as_u64().context("Expected child ID")?)
                else {
                    return Ok(json!(false));
                };
                let (Some(child), None) = (&mut process.child, process.status) else {
                    return Ok(json!(false));
                };
                let signal = args[1].as_str().context("Expected signal")?;
                #[cfg(unix)]
                {
                    let number = crate::node::SIGNALS
                        .iter()
                        .find(|(name, _)| *name == signal)
                        .with_context(|| format!("Unknown signal: {signal}"))?
                        .1;
                    // Only signal a child that this context owns and has not reaped.
                    if unsafe {
                        libc::kill(
                            if process.detached {
                                -(child.id() as libc::pid_t)
                            } else {
                                child.id() as libc::pid_t
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
                    child.kill()?;
                    process.signal = Some(signal.into());
                }
                Ok(json!(true))
            }
            "channel" => {
                #[cfg(unix)]
                {
                    use std::os::{fd::FromRawFd, unix::net::UnixStream};
                    // Workers of main see the same variable, so only the first context takes the descriptor.
                    static TAKEN: std::sync::atomic::AtomicBool =
                        std::sync::atomic::AtomicBool::new(false);
                    let fd = i32::try_from(args[0].as_i64().context("Expected a descriptor")?)?;
                    if TAKEN.swap(true, Ordering::AcqRel)
                        || unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0
                    {
                        return Ok(Value::Null);
                    }
                    let (sender, receiver) = mpsc::sync_channel(8);
                    let mut process = Process::new(None, receiver, false);
                    let stream = unsafe { UnixStream::from_raw_fd(fd) };
                    attach(
                        &mut process,
                        &sender,
                        &Waker::current(),
                        vec![("ipc", stream)],
                        Vec::new(),
                    )?;
                    self.next_id += 1;
                    self.children.insert(self.next_id, process);
                    Ok(json!(self.next_id))
                }
                #[cfg(not(unix))]
                Ok(Value::Null)
            }
            #[cfg(unix)]
            "flush" => {
                if let Some(process) = args[0].as_u64().and_then(|id| self.children.get_mut(&id))
                    && let Some(index) = process.extra.iter().position(|(name, _)| args[1] == *name)
                {
                    process.extra.remove(index).1.send(None)?;
                    // A parent that stops reading must not keep this process from exiting.
                    let deadline = Instant::now() + Duration::from_secs(1);
                    while let Ok(event) = process
                        .events
                        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    {
                        if matches!(event, Event::Finish(_)) {
                            break;
                        }
                    }
                }
                Ok(Value::Null)
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
