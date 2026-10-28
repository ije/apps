use crate::host::Role;
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{self, Read, Write},
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver, Sender},
};

enum Event {
    Data(&'static str, Vec<u8>),
    End(&'static str),
    Error(&'static str, io::Error),
    Written(usize),
}

struct Process {
    child: Child,
    input: Option<Sender<Option<Vec<u8>>>>,
    events: Receiver<Event>,
    status: Option<ExitStatus>,
    open_streams: usize,
    spawned: bool,
    signal: Option<String>,
    detached: bool,
    unreferenced: bool,
}

impl Drop for Process {
    fn drop(&mut self) {
        self.input.take();
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
                let id = spawned["id"].clone();
                if config["stdio"][0] == "pipe" {
                    if !args[2]["input"].is_null() {
                        state.call(role, "write", &json!([id, args[2]["input"]]))?;
                    }
                    state.call(role, "end", &json!([id]))?;
                }
                let deadline = args[2]["timeout"]
                    .as_u64()
                    .filter(|v| *v > 0)
                    .map(|v| std::time::Instant::now() + std::time::Duration::from_millis(v));
                let limit = args[2]["maxBuffer"].as_u64().unwrap_or(1024 * 1024) as usize;
                let (mut stdout, mut stderr) = (Vec::<u8>::new(), Vec::<u8>::new());
                loop {
                    for event in state.call(role, "poll", &json!([]))?.as_array().unwrap() {
                        if event["event"] == "data" {
                            let output = if event["stream"] == "stdout" {
                                &mut stdout
                            } else {
                                &mut stderr
                            };
                            output.extend(crate::node::bytes(&event["data"])?);
                        }
                        if event["event"] == "close" {
                            return Ok(
                                json!({"pid":spawned["pid"],"stdout":if config["stdio"][1] == "pipe" { crate::web::base64(&stdout) } else { Value::Null },"stderr":if config["stdio"][2] == "pipe" { crate::web::base64(&stderr) } else { Value::Null },"status":event["code"],"signal":event["signal"]}),
                            );
                        }
                    }
                    if stdout.len() > limit
                        || stderr.len() > limit
                        || deadline.is_some_and(|v| std::time::Instant::now() >= v)
                    {
                        let _ = state.call(role, "kill", &json!([id, "SIGKILL"]));
                        return Ok(
                            json!({"pid":spawned["pid"],"stdout":if config["stdio"][1] == "pipe" { crate::web::base64(&stdout) } else { Value::Null },"stderr":if config["stdio"][2] == "pipe" { crate::web::base64(&stderr) } else { Value::Null },"status":null,"signal":"SIGKILL","error":{"code":if stdout.len()>limit||stderr.len()>limit{"ENOBUFS"}else{"ETIMEDOUT"},"message":"Child process exceeded its output or time limit"}}),
                        );
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
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
                let child = match builder.spawn() {
                    Ok(child) => child,
                    Err(error) => return Ok(json!({"error": error_value(&error)})),
                };
                let (sender, receiver) = mpsc::sync_channel(32);
                let mut process = Process {
                    child,
                    input: None,
                    events: receiver,
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
                        let sender = sender.clone();
                        std::thread::Builder::new()
                            .name(format!("apps-{name}"))
                            .spawn(move || {
                                let mut bytes = [0; 16384];
                                loop {
                                    match pipe.read(&mut bytes) {
                                        Ok(0) => break,
                                        Ok(size) => {
                                            if sender
                                                .send(Event::Data(name, bytes[..size].to_vec()))
                                                .is_err()
                                            {
                                                return;
                                            }
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
                                let _ = sender.send(Event::End(name));
                            })?;
                    }
                }
                if let Some(mut pipe) = process.child.stdin.take() {
                    let (input, writes) = mpsc::channel::<Option<Vec<u8>>>();
                    process.input = Some(input);
                    process.open_streams += 1;
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
                            }
                            drop(pipe);
                            let _ = sender.send(Event::End("stdin"));
                        })?;
                }
                self.next_id += 1;
                let pid = process.child.id();
                self.children.insert(self.next_id, process);
                Ok(json!({"id": self.next_id, "pid": pid}))
            }
            "poll" => {
                let mut events = Vec::new();
                let mut closed = Vec::new();
                for (&id, process) in &mut self.children {
                    if !process.spawned {
                        process.spawned = true;
                        events.push(json!({"id": id, "event": "spawn"}));
                    }
                    if process.status.is_none()
                        && let Some(status) = process.child.try_wait()?
                    {
                        #[cfg(unix)]
                        {
                            use std::os::unix::process::ExitStatusExt;
                            process.signal = status.signal().map(|signal| match signal {
                                libc::SIGHUP => "SIGHUP".into(),
                                libc::SIGINT => "SIGINT".into(),
                                libc::SIGQUIT => "SIGQUIT".into(),
                                libc::SIGABRT => "SIGABRT".into(),
                                libc::SIGKILL => "SIGKILL".into(),
                                libc::SIGSEGV => "SIGSEGV".into(),
                                libc::SIGPIPE => "SIGPIPE".into(),
                                libc::SIGTERM => "SIGTERM".into(),
                                _ => format!("SIG{signal}"),
                            });
                        }
                        process.status = Some(status);
                        process.input.take();
                        events.push(json!({"id": id, "event": "exit", "code": if process.signal.is_some() { None } else { status.code() }, "signal": process.signal}));
                    }
                    for event in process.events.try_iter().take(64) {
                        events.push(match event {
                            Event::Data(stream, data) => json!({"id": id, "event": "data", "stream": stream, "data": crate::binary::write(&data).unwrap_or_else(|| json!({"latin1":crate::node::latin1(&data)}))}),
                            Event::Error(stream, error) => json!({"id": id, "event": "error", "stream": stream, "error": error_value(&error)}),
                            Event::Written(bytes) => json!({"id": id, "event": "written", "bytes": bytes}),
                            Event::End(stream) => {
                                process.open_streams -= 1;
                                json!({"id": id, "event": if stream == "stdin" { "finish" } else { "end" }, "stream": stream})
                            }
                        });
                    }
                    if let Some(status) = process.status
                        && process.open_streams == 0
                    {
                        events.push(json!({"id": id, "event": "close", "code": if process.signal.is_some() { None } else { status.code() }, "signal": process.signal}));
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
                input.send(Some(bytes))?;
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
                    let number = match signal {
                        "SIGTERM" => libc::SIGTERM,
                        "SIGKILL" => libc::SIGKILL,
                        "SIGINT" => libc::SIGINT,
                        "SIGHUP" => libc::SIGHUP,
                        "SIGQUIT" => libc::SIGQUIT,
                        _ => bail!("Unsupported signal: {signal}"),
                    };
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
