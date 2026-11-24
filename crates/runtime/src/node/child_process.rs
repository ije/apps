use crate::jsc::Waker;
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
    Written(&'static str, u32),
    Finish(&'static str),
    Exit,
}

struct Process {
    child: Option<Child>,
    inputs: Vec<(&'static str, Sender<Option<Vec<u8>>>)>,
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
            inputs: Vec::new(),
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
        self.inputs.clear();
    }

    fn code(&self) -> Option<i32> {
        self.status
            .filter(|_| self.signal.is_none())
            .and_then(|status| status.code())
    }
}

// Stdout, stderr, and the code and errno of an error that stopped a synchronous run.
type Output = (Vec<u8>, Vec<u8>, Option<(&'static str, i32)>);

const PAUSED: u8 = 1;
const CLOSED: u8 = 2;

impl Drop for Process {
    fn drop(&mut self) {
        self.inputs.clear();
        for (_, state, reader) in &self.readers {
            state.store(CLOSED, Ordering::Release);
            reader.unpark();
        }
        if self.status.is_none()
            && let Some(mut child) = self.child.take()
        {
            if self.detached && self.unreferenced {
                // Nothing reaps the child after this, so it would stay a zombie.
                let _ = std::thread::Builder::new()
                    .name("apps-exit".into())
                    .spawn(move || child.wait());
            } else {
                let _ = child.kill();
                let _ = child.wait();
            }
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

impl Process {
    fn pipe_in<W: Write + Send + 'static>(
        &mut self,
        name: &'static str,
        mut pipe: W,
        sender: &SyncSender<Event>,
        wake: &Waker,
        close: fn(W),
    ) -> io::Result<()> {
        let (input, writes) = mpsc::channel::<Option<Vec<u8>>>();
        self.inputs.push((name, input));
        self.open_streams += 1;
        let (sender, wake) = (sender.clone(), wake.clone());
        std::thread::Builder::new()
            .name(format!("apps-{name}"))
            .spawn(move || {
                while let Ok(Some(mut bytes)) = writes.recv() {
                    // A burst of small writes, like IPC messages, costs one write, one event and one wake.
                    let (mut count, mut open) = (1, true);
                    while open && bytes.len() < 1 << 20 {
                        match writes.try_recv() {
                            Ok(Some(more)) => {
                                bytes.extend_from_slice(&more);
                                count += 1;
                            }
                            Ok(None) => open = false,
                            Err(_) => break,
                        }
                    }
                    if let Err(error) = pipe.write_all(&bytes) {
                        let _ = sender.send(Event::Error(name, error));
                        break;
                    }
                    if sender.send(Event::Written(name, count)).is_err() {
                        return;
                    }
                    wake.wake();
                    if !open {
                        break;
                    }
                }
                close(pipe);
                if sender.send(Event::Finish(name)).is_ok() {
                    wake.wake();
                }
            })?;
        Ok(())
    }

    fn pipe_out(
        &mut self,
        sender: &SyncSender<Event>,
        wake: &Waker,
        #[cfg(unix)] pipes: Vec<(&'static str, std::os::unix::net::UnixStream)>,
        outputs: Vec<(&'static str, Box<dyn Read + Send>)>,
    ) -> io::Result<()> {
        #[cfg(unix)]
        let mut outputs = outputs;

        #[cfg(unix)]
        for (name, pipe) in pipes {
            outputs.push((name, Box::new(pipe.try_clone()?)));
            self.pipe_in(name, pipe, sender, wake, |pipe| {
                let _ = pipe.shutdown(std::net::Shutdown::Write);
            })?;
        }
        for (name, mut pipe) in outputs {
            self.open_streams += 1;
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
            self.readers.push((name, state, reader.thread().clone()));
        }
        Ok(())
    }

    // An error of the spawn itself is the inner result, for the `error` event.
    fn spawn(args: &Value) -> Result<io::Result<(Self, u32)>> {
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
        // Without the setting, a child of this executable starts once more to set it.
        #[cfg(target_os = "macos")]
        if std::env::current_exe().is_ok_and(|exe| exe.as_os_str() == command) {
            builder.env("MallocSpaceEfficient", "1");
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
                        libc::fcntl(child.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 64 + index as i32)
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
                return Ok(Err(error));
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
        let mut process = Self::new(Some(child), receiver, options["detached"] == true);
        process.pipe_out(
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
        if let Some(pipe) = stdin {
            process.pipe_in("stdin", pipe, &sender, &wake, drop)?;
        }
        wake.wake();
        Ok(Ok((process, pid)))
    }

    #[cfg(unix)]
    fn channel(fd: i32) -> Result<Option<Self>> {
        use std::os::{fd::FromRawFd, unix::net::UnixStream};
        // Workers of main see the same variable, so only the first context takes the descriptor.
        static TAKEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if TAKEN.swap(true, Ordering::AcqRel)
            || unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0
        {
            return Ok(None);
        }
        let (sender, receiver) = mpsc::sync_channel(8);
        let mut process = Self::new(None, receiver, false);
        let stream = unsafe { UnixStream::from_raw_fd(fd) };
        process.pipe_out(
            &sender,
            &Waker::current(),
            vec![("ipc", stream)],
            Vec::new(),
        )?;
        Ok(Some(process))
    }

    fn unref(&mut self, unreferenced: bool) {
        self.unreferenced = unreferenced;
    }

    // Pauses, resumes or closes the reader of an output stream.
    fn gate(&self, stream: &Value, next: u8) {
        if let Some((_, state, reader)) = self.readers.iter().find(|(name, ..)| stream == *name) {
            let _ = state.try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current != CLOSED).then_some(next)
            });
            reader.unpark();
        }
    }

    // Writes to an input stream, or ends it without bytes.
    fn send(&mut self, name: &str, bytes: Option<&Value>) -> Result<()> {
        let index = self
            .inputs
            .iter()
            .position(|(stream, _)| *stream == name)
            .context(if name == "stdin" {
                "Child stdin is closed"
            } else {
                "Child stream is closed"
            })?;
        match bytes {
            None => self.inputs.remove(index).1.send(None)?,
            Some(bytes) => self.inputs[index]
                .1
                .send(Some(crate::node::bytes(bytes)?.to_vec()))?,
        }
        Ok(())
    }

    #[cfg(unix)]
    fn flush(&mut self, stream: &Value) -> Result<()> {
        if let Some(index) = self.inputs.iter().position(|(name, _)| stream == *name) {
            self.inputs.remove(index).1.send(None)?;
            // A parent that stops reading must not keep this process from exiting.
            let deadline = Instant::now() + Duration::from_secs(1);
            while let Ok(event) = self
                .events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                if matches!(event, Event::Finish(_)) {
                    break;
                }
            }
        }
        Ok(())
    }

    fn kill(&mut self, signal: &str) -> Result<bool> {
        let (Some(child), None) = (&mut self.child, self.status) else {
            return Ok(false);
        };
        #[cfg(unix)]
        {
            let number = crate::node::SIGNALS
                .iter()
                .find(|(name, _)| *name == signal)
                .with_context(|| format!("Unknown signal: {signal}"))?
                .1;
            // Only signal a child that this context owns and has not reaped.
            if unsafe { libc::kill(child.id() as libc::pid_t, number) } != 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ESRCH) {
                    return Ok(false);
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
            self.signal = Some(signal.into());
        }
        Ok(true)
    }

    // Adds the events of this process. Returns whether it closed.
    fn poll(&mut self, id: u64, events: &mut Vec<Value>) -> Result<bool> {
        if !self.spawned {
            self.spawned = true;
            events.push(json!({"id": id, "event": "spawn"}));
        }
        let mut exited = false;
        for (index, event) in self.events.try_iter().take(8).enumerate() {
            if index == 7 {
                Waker::current().wake();
            }
            events.push(match event {
                    Event::Data(stream, data) => json!({"id": id, "event": "data", "stream": stream, "data": crate::jsc::payload::write(data)}),
                    Event::Error(stream, error) => json!({"id": id, "event": "error", "stream": stream, "error": error_value(&error)}),
                    Event::Written(stream, count) => json!({"id": id, "event": "written", "stream": stream, "count": count}),
                    Event::Finish(stream) => {
                        self.open_streams -= 1;
                        json!({"id": id, "event": "finish", "stream": stream})
                    }
                    Event::End(stream) => {
                        self.open_streams -= 1;
                        json!({"id": id, "event": "end", "stream": stream})
                    }
                    Event::Exit => {
                        exited = true;
                        continue;
                    }
                });
        }
        if exited
            && self.status.is_none()
            && let Some(child) = &mut self.child
            && let Some(status) = child.try_wait()?
        {
            self.exit(status);
            events.push(
                json!({"id": id, "event": "exit", "code": self.code(), "signal": self.signal}),
            );
        }
        if (self.status.is_some() || self.child.is_none()) && self.open_streams == 0 {
            events.push(
                json!({"id": id, "event": "close", "code": self.code(), "signal": self.signal}),
            );
            return Ok(true);
        }
        Ok(false)
    }

    // Collects the output until the process ends. On a timeout or too much output, it stops the process.
    fn wait(
        &mut self,
        deadline: Option<Instant>,
        limit: usize,
        kill_signal: &str,
    ) -> Result<Output> {
        let (mut stdout, mut stderr) = (Vec::<u8>::new(), Vec::<u8>::new());
        let error = loop {
            if self.status.is_some() && self.open_streams == 0 {
                break None;
            }
            let event = match deadline {
                Some(deadline) => self
                    .events
                    .recv_timeout(deadline.saturating_duration_since(Instant::now())),
                None => self
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
                Ok(Event::End(_) | Event::Finish(_)) => self.open_streams -= 1,
                Ok(Event::Exit) | Err(RecvTimeoutError::Disconnected) if self.status.is_none() => {
                    self.reap()?;
                }
                Err(RecvTimeoutError::Disconnected) => break None,
                Err(RecvTimeoutError::Timeout) => {
                    break Some(("ETIMEDOUT", libc::ETIMEDOUT));
                }
                Ok(_) => {}
            }
        };
        if error.is_some() && self.status.is_none() {
            let _ = self.kill(kill_signal);
            self.reap()?;
        }
        Ok((stdout, stderr, error))
    }
}

#[derive(Default)]
pub(crate) struct State {
    children: HashMap<u64, Process>,
    next_id: u64,
}

impl State {
    fn add(&mut self, process: Process) -> u64 {
        self.next_id += 1;
        self.children.insert(self.next_id, process);
        self.next_id
    }

    fn child(&mut self, id: &Value) -> Result<&mut Process> {
        self.children
            .get_mut(&id.as_u64().context("Expected child ID")?)
            .context("Child process is closed")
    }

    pub(crate) fn call(&mut self, op: &str, args: &Value) -> Result<Value> {
        match op {
            "sync" => {
                let mut state = State::default();
                let config = &args[2];
                let spawned = state.call("spawn", args)?;
                if !spawned["error"].is_null() {
                    return Ok(spawned);
                }
                let id = &spawned["id"];
                if config["stdio"][0] == "pipe" {
                    if !config["input"].is_null() {
                        state.call("write", &json!([id, config["input"]]))?;
                    }
                    state.call("end", &json!([id]))?;
                }
                let deadline = config["timeout"]
                    .as_u64()
                    .filter(|v| *v > 0)
                    .map(|v| Instant::now() + Duration::from_millis(v));
                let limit = config["maxBuffer"].as_u64().unwrap_or(1024 * 1024) as usize;
                let signal = config["killSignal"].as_str().unwrap_or("SIGTERM");
                let process = state.child(id)?;
                let (stdout, stderr, error) = process.wait(deadline, limit, signal)?;
                let output = |data: Vec<u8>, index: usize| {
                    if config["stdio"][index] == "pipe" {
                        crate::jsc::payload::write(data)
                    } else {
                        Value::Null
                    }
                };
                let mut result = json!({"pid":spawned["pid"],"stdout":output(stdout, 1),"stderr":output(stderr, 2),"status":process.code(),"signal":process.signal});
                if let Some((code, errno)) = error {
                    result["error"] = json!({"code":code,"errno":cfg!(unix).then_some(-errno)});
                }
                Ok(result)
            }
            "unref" => {
                // A child that failed to spawn has no ID.
                if let Some(process) = args[0].as_u64().and_then(|id| self.children.get_mut(&id)) {
                    process.unref(args[1] == true);
                }
                Ok(Value::Null)
            }
            "spawn" => Ok(match Process::spawn(args)? {
                Ok((process, pid)) => json!({"id": self.add(process), "pid": pid}),
                Err(error) => json!({"error": error_value(&error)}),
            }),
            "pause" | "destroy" => {
                if let Some(process) = args[0].as_u64().and_then(|id| self.children.get(&id)) {
                    let next = if op == "destroy" {
                        CLOSED
                    } else {
                        u8::from(args[2] == true)
                    };
                    process.gate(&args[1], next);
                }
                Ok(Value::Null)
            }
            "poll" => {
                let mut events = Vec::new();
                let mut closed = Vec::new();
                for (&id, process) in &mut self.children {
                    if process.poll(id, &mut events)? {
                        closed.push(id);
                    }
                }
                for id in closed {
                    self.children.remove(&id);
                }
                Ok(json!(events))
            }
            "write" | "end" => {
                let process = self.child(&args[0])?;
                let name = if op == "end" { &args[1] } else { &args[2] };
                let bytes = (op == "write").then_some(&args[1]);
                process.send(name.as_str().unwrap_or("stdin"), bytes)?;
                Ok(Value::Null)
            }
            "kill" => {
                let Some(process) = self
                    .children
                    .get_mut(&args[0].as_u64().context("Expected child ID")?)
                else {
                    return Ok(json!(false));
                };
                let signal = args[1].as_str().context("Expected signal")?;
                Ok(json!(process.kill(signal)?))
            }
            "channel" => {
                #[cfg(unix)]
                {
                    let fd = i32::try_from(args[0].as_i64().context("Expected a descriptor")?)?;
                    Ok(Process::channel(fd)?
                        .map_or(Value::Null, |process| json!(self.add(process))))
                }
                #[cfg(not(unix))]
                Ok(Value::Null)
            }
            #[cfg(unix)]
            "flush" => {
                if let Some(process) = args[0].as_u64().and_then(|id| self.children.get_mut(&id)) {
                    process.flush(&args[1])?;
                }
                Ok(Value::Null)
            }
            _ => bail!("Unsupported child process operation: {op}"),
        }
    }
}
