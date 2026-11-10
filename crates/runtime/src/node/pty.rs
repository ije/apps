use anyhow::{Context, Result, bail, ensure};
use portable_pty::{CommandBuilder, MasterPty, PtySize};
use serde_json::{Value, json};
use std::{collections::HashMap, io::Write, sync::mpsc};

struct Session {
    master: Box<dyn MasterPty + Send>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    input: mpsc::Sender<Vec<u8>>,
    events: mpsc::Receiver<Vec<u8>>,
    pending: Vec<u8>,
    eof: bool,
    exited: bool,
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn exit_is_reported_before_output_eof() {
        let mut state = State::default();
        state
            .call("spawn", &json!(["/bin/sh", ["-c", "exit 7"], {}]))
            .unwrap();
        let (_output, events) = mpsc::channel();
        state.sessions.values_mut().next().unwrap().events = events;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            let events = state.call("poll", &json!([])).unwrap();
            if let Some(event) = events
                .as_array()
                .unwrap()
                .iter()
                .find(|event| event["type"] == "exit")
            {
                assert_eq!(event["exitCode"], 7);
                assert!(state.sessions.values().any(|session| !session.eof));
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "PTY exit waited for EOF"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take().filter(|_| !self.exited) {
            // kill() waits up to 200 ms for SIGHUP, so it must not run on the UI thread.
            let _ = std::thread::Builder::new()
                .name("apps-pty-kill".into())
                .spawn(move || {
                    // Never signal a reaped child; its PID may belong to another process.
                    if let Ok(None) = child.try_wait()
                        && child.kill().is_ok()
                    {
                        let _ = child.wait();
                    }
                });
        }
    }
}
#[derive(Default)]
pub(crate) struct State {
    next: u64,
    sessions: HashMap<u64, Session>,
}
impl State {
    pub(crate) fn call(&mut self, op: &str, args: &Value) -> Result<Value> {
        match op {
            "spawn" => {
                let options = &args[2];
                let cols = options["cols"].as_u64().unwrap_or(80);
                let rows = options["rows"].as_u64().unwrap_or(24);
                ensure!(
                    (1..=65535).contains(&cols) && (1..=65535).contains(&rows),
                    "Invalid PTY dimensions"
                );
                let pair = portable_pty::native_pty_system().openpty(PtySize {
                    cols: cols as u16,
                    rows: rows as u16,
                    pixel_width: 0,
                    pixel_height: 0,
                })?;
                let mut command = CommandBuilder::new(args[0].as_str().context("Expected shell")?);
                for arg in args[1].as_array().context("Expected arguments")? {
                    command.arg(arg.as_str().context("Expected argument string")?);
                }
                if let Some(cwd) = options["cwd"].as_str() {
                    command.cwd(cwd);
                }
                if let Some(env) = options["env"].as_object() {
                    command.env_clear();
                    for (k, v) in env {
                        command.env(k, v.as_str().context("Expected environment value")?);
                    }
                }
                command.env("TERM", options["name"].as_str().unwrap_or("xterm-256color"));
                let child = pair.slave.spawn_command(command)?;
                drop(pair.slave);
                let pid = child.process_id();
                let mut reader = pair.master.try_clone_reader()?;
                let mut writer = pair.master.take_writer()?;
                let (tx, events) = mpsc::sync_channel(32);
                let wake = crate::host::Waker::current();
                let waker = wake.clone();
                std::thread::Builder::new()
                    .name("apps-pty-read".into())
                    .spawn(move || {
                        let mut bytes = [0; 16384];
                        loop {
                            match reader.read(&mut bytes) {
                                Ok(0) | Err(_) => break,
                                Ok(n) => {
                                    if tx.send(bytes[..n].to_vec()).is_err() {
                                        return;
                                    }
                                    waker.wake();
                                }
                            }
                        }
                        drop(tx);
                        waker.wake();
                    })?;
                #[cfg(unix)]
                let exit = pid.context("Missing PTY process ID")?;
                #[cfg(windows)]
                let exit = unsafe {
                    std::os::windows::io::BorrowedHandle::borrow_raw(
                        child
                            .as_raw_handle()
                            .context("Missing PTY process handle")?,
                    )
                }
                .try_clone_to_owned()?;
                crate::node::on_exit(exit, move || wake.wake())?;
                let (input, rx) = mpsc::channel::<Vec<u8>>();
                std::thread::Builder::new()
                    .name("apps-pty-write".into())
                    .spawn(move || {
                        while let Ok(bytes) = rx.recv() {
                            if writer.write_all(&bytes).is_err() {
                                break;
                            }
                        }
                    })?;
                self.next += 1;
                self.sessions.insert(
                    self.next,
                    Session {
                        master: pair.master,
                        child: Some(child),
                        input,
                        events,
                        pending: Vec::new(),
                        eof: false,
                        exited: false,
                    },
                );
                Ok(json!({"id":self.next,"pid":pid}))
            }
            "poll" => {
                let mut events = Vec::new();
                let mut closed = Vec::new();
                for (&id, session) in &mut self.sessions {
                    for _ in 0..64 {
                        match session.events.try_recv() {
                            Ok(data) => session.pending.extend(data),
                            Err(mpsc::TryRecvError::Empty) => break,
                            Err(mpsc::TryRecvError::Disconnected) => {
                                session.eof = true;
                                break;
                            }
                        }
                    }
                    let data = &session.pending;
                    let mut end = data.len();
                    if !session.eof
                        && let Some(start) = (end.saturating_sub(3)..end)
                            .rev()
                            .find(|&index| data[index] & 0xC0 != 0x80)
                        && start + data[start].leading_ones().max(1) as usize > end
                    {
                        end = start;
                    }
                    if end > 0 {
                        events.push(json!({"id":id,"type":"data","data":String::from_utf8_lossy(&data[..end])}));
                        session.pending.drain(..end);
                    }
                    if !session.exited
                        && let Some(child) = &mut session.child
                        && let Some(status) = child.try_wait()?
                    {
                        events.push(json!({"id":id,"type":"exit","exitCode":status.exit_code(),"signal":status.signal()}));
                        session.exited = true;
                    }
                    if session.exited && session.eof {
                        events.push(json!({"id":id,"type":"close"}));
                        closed.push(id);
                    }
                }
                for id in closed {
                    self.sessions.remove(&id);
                }
                Ok(json!(events))
            }
            "write" | "resize" | "kill" => {
                let session = self
                    .sessions
                    .get_mut(&args[0].as_u64().context("Expected PTY ID")?)
                    .context("PTY is closed")?;
                match op {
                    "write" => session.input.send(crate::node::bytes(&args[1])?.to_vec())?,
                    "resize" => {
                        let cols = args[1].as_u64().context("Expected columns")?;
                        let rows = args[2].as_u64().context("Expected rows")?;
                        ensure!(
                            (1..=65535).contains(&cols) && (1..=65535).contains(&rows),
                            "Invalid PTY dimensions"
                        );
                        session.master.resize(PtySize {
                            cols: cols as u16,
                            rows: rows as u16,
                            pixel_width: 0,
                            pixel_height: 0,
                        })?;
                    }
                    _ => {
                        // Never signal a reaped child; its PID may belong to another process.
                        let Some(child) = session.child.as_mut().filter(|_| !session.exited) else {
                            return Ok(Value::Null);
                        };
                        #[cfg(unix)]
                        {
                            let signal = match args[1].as_str().unwrap_or("SIGHUP") {
                                "SIGINT" => libc::SIGINT,
                                "SIGTERM" => libc::SIGTERM,
                                "SIGKILL" => libc::SIGKILL,
                                "SIGHUP" => libc::SIGHUP,
                                _ => bail!("Unsupported signal"),
                            };
                            if let Some(pid) = child.process_id()
                                && unsafe { libc::kill(-(pid as i32), signal) } != 0
                            {
                                child.kill()?;
                            }
                        }
                        #[cfg(not(unix))]
                        child.kill()?;
                    }
                }
                Ok(Value::Null)
            }
            _ => bail!("Unsupported PTY operation: {op}"),
        }
    }
}
