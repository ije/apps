pub(crate) mod child_process;
pub(crate) mod ffi;
mod fs;
pub(crate) mod napi;
pub(crate) mod pty;
pub(crate) mod sqlite;
pub(crate) mod system;

use anyhow::{Context, Result};
use fs::Files;
use notify::{RecursiveMode, Watcher};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex, mpsc},
};
use system::Hasher;

pub(crate) fn bytes(value: &Value) -> Result<Vec<u8>> {
    if let Some(bytes) = crate::jsc::payload::read(value) {
        return bytes;
    }
    Ok(value
        .as_str()
        .context("Expected bytes")?
        .as_bytes()
        .to_vec())
}

#[cfg(unix)]
pub(crate) type Exit = u32;
#[cfg(windows)]
pub(crate) type Exit = std::os::windows::io::OwnedHandle;

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn WaitForSingleObject(handle: std::os::windows::io::RawHandle, milliseconds: u32) -> u32;
}

/// Runs `done` on a thread after the child exits. The child stays unreaped for `try_wait`.
pub(crate) fn on_exit(child: Exit, done: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("apps-exit".into())
        .spawn(move || {
            // WARNING: WNOWAIT keeps the zombie, so the PID cannot be reused while this waits.
            #[cfg(unix)]
            while unsafe {
                let mut info = std::mem::zeroed();
                libc::waitid(
                    libc::P_PID,
                    child as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOWAIT,
                )
            } != 0
                && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
            {
            }
            #[cfg(windows)]
            unsafe {
                WaitForSingleObject(
                    std::os::windows::io::AsRawHandle::as_raw_handle(&child),
                    u32::MAX,
                );
            }
            done();
        })?;
    Ok(())
}

#[cfg(unix)]
const ERRNO: &[(&str, i32)] = &[
    ("E2BIG", libc::E2BIG),
    ("EACCES", libc::EACCES),
    ("EADDRINUSE", libc::EADDRINUSE),
    ("EADDRNOTAVAIL", libc::EADDRNOTAVAIL),
    ("EAGAIN", libc::EAGAIN),
    ("EBADF", libc::EBADF),
    ("EBUSY", libc::EBUSY),
    ("ECHILD", libc::ECHILD),
    ("ECONNABORTED", libc::ECONNABORTED),
    ("ECONNREFUSED", libc::ECONNREFUSED),
    ("ECONNRESET", libc::ECONNRESET),
    ("EEXIST", libc::EEXIST),
    ("EFBIG", libc::EFBIG),
    ("EHOSTUNREACH", libc::EHOSTUNREACH),
    ("EINTR", libc::EINTR),
    ("EINVAL", libc::EINVAL),
    ("EIO", libc::EIO),
    ("EISDIR", libc::EISDIR),
    ("ELOOP", libc::ELOOP),
    ("EMFILE", libc::EMFILE),
    ("EMLINK", libc::EMLINK),
    ("ENAMETOOLONG", libc::ENAMETOOLONG),
    ("ENETUNREACH", libc::ENETUNREACH),
    ("ENFILE", libc::ENFILE),
    ("ENOENT", libc::ENOENT),
    ("ENOMEM", libc::ENOMEM),
    ("ENOSPC", libc::ENOSPC),
    ("ENOSYS", libc::ENOSYS),
    ("ENOTCONN", libc::ENOTCONN),
    ("ENOTDIR", libc::ENOTDIR),
    ("ENOTEMPTY", libc::ENOTEMPTY),
    ("ENOTSUP", libc::ENOTSUP),
    ("EPERM", libc::EPERM),
    ("EPIPE", libc::EPIPE),
    ("EROFS", libc::EROFS),
    ("ESRCH", libc::ESRCH),
    ("ETIMEDOUT", libc::ETIMEDOUT),
    ("ETXTBSY", libc::ETXTBSY),
    ("EXDEV", libc::EXDEV),
];

#[cfg(unix)]
pub(crate) const SIGNALS: &[(&str, i32)] = &[
    ("SIGHUP", libc::SIGHUP),
    ("SIGINT", libc::SIGINT),
    ("SIGQUIT", libc::SIGQUIT),
    ("SIGILL", libc::SIGILL),
    ("SIGTRAP", libc::SIGTRAP),
    ("SIGABRT", libc::SIGABRT),
    ("SIGBUS", libc::SIGBUS),
    ("SIGFPE", libc::SIGFPE),
    ("SIGKILL", libc::SIGKILL),
    ("SIGUSR1", libc::SIGUSR1),
    ("SIGSEGV", libc::SIGSEGV),
    ("SIGUSR2", libc::SIGUSR2),
    ("SIGPIPE", libc::SIGPIPE),
    ("SIGALRM", libc::SIGALRM),
    ("SIGTERM", libc::SIGTERM),
    ("SIGCHLD", libc::SIGCHLD),
    ("SIGCONT", libc::SIGCONT),
    ("SIGSTOP", libc::SIGSTOP),
    ("SIGTSTP", libc::SIGTSTP),
    ("SIGTTIN", libc::SIGTTIN),
    ("SIGTTOU", libc::SIGTTOU),
    ("SIGURG", libc::SIGURG),
    ("SIGXCPU", libc::SIGXCPU),
    ("SIGXFSZ", libc::SIGXFSZ),
    ("SIGVTALRM", libc::SIGVTALRM),
    ("SIGPROF", libc::SIGPROF),
    ("SIGWINCH", libc::SIGWINCH),
    ("SIGIO", libc::SIGIO),
    ("SIGSYS", libc::SIGSYS),
];

#[cfg(windows)]
pub(crate) const SIGNALS: &[(&str, i32)] = &[
    ("SIGHUP", 1),
    ("SIGINT", 2),
    ("SIGILL", 4),
    ("SIGFPE", 8),
    ("SIGKILL", 9),
    ("SIGSEGV", 11),
    ("SIGTERM", 15),
    ("SIGBREAK", 21),
    ("SIGABRT", 22),
    ("SIGWINCH", 28),
];

pub(crate) fn code(error: &std::io::Error) -> &'static str {
    use std::io::ErrorKind::*;
    #[cfg(unix)]
    if let Some(errno) = error.raw_os_error()
        && let Some((name, _)) = ERRNO.iter().find(|(_, value)| *value == errno)
    {
        return name;
    }
    match error.kind() {
        NotFound => "ENOENT",
        PermissionDenied => "EACCES",
        AlreadyExists => "EEXIST",
        NotADirectory => "ENOTDIR",
        IsADirectory => "EISDIR",
        DirectoryNotEmpty => "ENOTEMPTY",
        ReadOnlyFilesystem => "EROFS",
        StorageFull => "ENOSPC",
        ResourceBusy => "EBUSY",
        CrossesDevices => "EXDEV",
        InvalidInput => "EINVAL",
        BrokenPipe => "EPIPE",
        TooManyLinks => "EMLINK",
        InvalidFilename => "ENAMETOOLONG",
        FileTooLarge => "EFBIG",
        TimedOut => "ETIMEDOUT",
        Interrupted => "EINTR",
        WouldBlock => "EAGAIN",
        Unsupported => "ENOTSUP",
        OutOfMemory => "ENOMEM",
        _ => "EIO",
    }
}

pub(crate) fn hostname() -> String {
    #[cfg(unix)]
    {
        let mut buffer = [0u8; 256];
        if unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } == 0 {
            let end = buffer
                .iter()
                .position(|&byte| byte == 0)
                .unwrap_or(buffer.len());
            return String::from_utf8_lossy(&buffer[..end]).into_owned();
        }
    }
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "localhost".into())
}

type Job = (
    u64,
    String,
    crate::jsc::payload::Payload,
    crate::host::Waker,
);

pub(crate) struct State {
    files: Arc<Files>,
    next: u64,
    sender: mpsc::Sender<crate::jsc::payload::Payload>,
    receiver: mpsc::Receiver<crate::jsc::payload::Payload>,
    watchers: HashMap<u64, notify::RecommendedWatcher>,
    hashers: HashMap<u64, Hasher>,
    tasks: Option<mpsc::Sender<Job>>,
    pool: Option<mpsc::Sender<Job>>,
}
impl Default for State {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            files: Arc::default(),
            next: 0,
            sender,
            receiver,
            watchers: HashMap::new(),
            hashers: HashMap::new(),
            tasks: None,
            pool: None,
        }
    }
}
impl State {
    pub(crate) fn call(&mut self, op: &str, args: &Value) -> Result<Value> {
        match op {
            "poll" => {
                let mut events = Vec::new();
                for event in self.receiver.try_iter() {
                    events.push(event.into_value());
                    if events.len() == 256 {
                        crate::host::Waker::current().wake();
                        break;
                    }
                }
                Ok(json!(events))
            }
            "async" => {
                let task = |files: &Files,
                            id: u64,
                            op: &str,
                            args: crate::jsc::payload::Payload| {
                    crate::jsc::payload::Payload::capture(args.buffers, || {
                        match files.call(op, &args.value) {
                            Ok(value) => json!({"id":id,"value":value}),
                            Err(error) => {
                                json!({"id":id,"error":error.to_string(),"code":error.downcast_ref::<std::io::Error>().map(code)})
                            }
                        }
                    })
                };
                self.next += 1;
                let id = self.next;
                let op = args[0].as_str().context("Expected operation")?.to_owned();
                let payload = crate::jsc::payload::Payload::snapshot(args[1].clone());
                // Slow work uses a small pool so it cannot stall queued file operations.
                let slow = matches!(op.as_str(), "lookup" | "zlib" | "pbkdf2");
                let queue = if slow {
                    &mut self.pool
                } else {
                    &mut self.tasks
                };
                let jobs = match queue {
                    Some(jobs) => jobs,
                    None => {
                        let (jobs, receiver) = mpsc::channel::<Job>();
                        let receiver = Arc::new(Mutex::new(receiver));
                        let workers = if slow {
                            std::thread::available_parallelism().map_or(1, |n| n.get().min(4))
                        } else {
                            1
                        };
                        for _ in 0..workers {
                            let (files, sender, receiver) =
                                (self.files.clone(), self.sender.clone(), receiver.clone());
                            std::thread::Builder::new()
                                .name(if slow { "apps-node" } else { "apps-files" }.into())
                                .spawn(move || {
                                    while let Ok(Ok((id, op, args, wake))) =
                                        receiver.lock().map(|receiver| receiver.recv())
                                    {
                                        if sender.send(task(&files, id, &op, args)).is_err() {
                                            break;
                                        }
                                        wake.wake();
                                    }
                                })?;
                        }
                        queue.insert(jobs)
                    }
                };
                jobs.send((id, op, payload, crate::host::Waker::current()))
                    .context("Node task queue is closed")?;
                Ok(json!(id))
            }
            "watch" => {
                self.next += 1;
                let id = self.next;
                let (sender, wake) = (self.sender.clone(), crate::host::Waker::current());
                let root = Path::new(args[0].as_str().context("Expected watch path")?);
                let base = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_owned());
                let mut watcher = notify::recommended_watcher(
                    move |event: notify::Result<notify::Event>| {
                        let value = match event {
                            Ok(event) => {
                                json!({"id":id,"watch":true,"event":if event.kind.is_modify(){"change"}else{"rename"},"paths":event.paths.iter().map(|path| path.strip_prefix(&base).ok().filter(|name| !name.as_os_str().is_empty()).or_else(|| path.file_name().map(Path::new)).unwrap_or(path).to_string_lossy().into_owned()).collect::<Vec<_>>()})
                            }
                            Err(e) => json!({"id":id,"watch":true,"error":e.to_string()}),
                        };
                        if sender.send(value.into()).is_ok() {
                            wake.wake();
                        }
                    },
                )?;
                watcher.watch(
                    root,
                    if args[1]["recursive"] == true {
                        RecursiveMode::Recursive
                    } else {
                        RecursiveMode::NonRecursive
                    },
                )?;
                self.watchers.insert(id, watcher);
                Ok(json!(id))
            }
            "hash.new" | "hash.copy" => {
                let hasher = if op == "hash.new" {
                    Hasher::new(&args[0], &args[1])?
                } else {
                    self.hashers
                        .get(&args[0].as_u64().context("Expected hash ID")?)
                        .context("Hash is finalized")?
                        .clone()
                };
                self.next += 1;
                self.hashers.insert(self.next, hasher);
                Ok(json!(self.next))
            }
            "hash.update" => {
                self.hashers
                    .get_mut(&args[0].as_u64().context("Expected hash ID")?)
                    .context("Hash is finalized")?
                    .update(&bytes(&args[1])?);
                Ok(Value::Null)
            }
            "hash.digest" => Ok(self
                .hashers
                .remove(&args[0].as_u64().context("Expected hash ID")?)
                .context("Hash is finalized")?
                .finish()),
            "hash.free" => {
                self.hashers
                    .remove(&args[0].as_u64().context("Expected hash ID")?);
                Ok(Value::Null)
            }
            "unwatch" => {
                self.watchers
                    .remove(&args[0].as_u64().context("Expected watch ID")?);
                Ok(Value::Null)
            }
            _ => self.files.call(op, args),
        }
    }
}
