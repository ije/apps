pub(crate) mod child_process;
pub(crate) mod ffi;
mod fs;
pub(crate) mod napi;
pub(crate) mod pty;
pub mod script;
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
use system::{Codec, Hasher};

pub(crate) fn bytes(value: &Value) -> Result<&[u8]> {
    if let Some(bytes) = crate::jsc::payload::slice(value) {
        return bytes;
    }
    Ok(value.as_str().context("Expected bytes")?.as_bytes())
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

// WARNING: runs `done` on a thread after the child exits. The child stays unreaped for `try_wait`.
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

type Job = Box<dyn FnOnce() + Send>;
struct Pool {
    jobs: std::collections::VecDeque<Job>,
    idle: usize,
    threads: usize,
}
static POOL: Mutex<Pool> = Mutex::new(Pool {
    jobs: std::collections::VecDeque::new(),
    idle: 0,
    threads: 0,
});
static READY: std::sync::Condvar = std::sync::Condvar::new();

// WARNING: like libuv, all contexts share up to 4 threads for blocking work (fs, dns, zlib, crypto, Node-API async
// work), so a slow job delays the jobs queued behind it.
pub(crate) fn blocking(job: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    let mut pool = POOL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    pool.jobs.push_back(Box::new(job));
    if pool.idle < pool.jobs.len() && pool.threads < 4 {
        let spawned = std::thread::Builder::new()
            .name("apps-pool".into())
            .spawn(|| {
                let mut pool = POOL
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                loop {
                    if let Some(job) = pool.jobs.pop_front() {
                        drop(pool);
                        job();
                        pool = POOL
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                    } else {
                        pool.idle += 1;
                        pool = READY
                            .wait(pool)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        pool.idle -= 1;
                    }
                }
            });
        match spawned {
            Ok(_) => pool.threads += 1,
            Err(error) if pool.threads == 0 => {
                pool.jobs.pop_back();
                return Err(error);
            }
            Err(_) => {}
        }
    }
    READY.notify_one();
    Ok(())
}

pub(crate) struct State {
    files: Arc<Files>,
    next: u64,
    sender: mpsc::Sender<crate::jsc::payload::Payload>,
    receiver: mpsc::Receiver<crate::jsc::payload::Payload>,
    watchers: HashMap<u64, notify::RecommendedWatcher>,
    hashers: HashMap<u64, Hasher>,
    codecs: HashMap<u64, (Codec, bool)>,
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
            codecs: HashMap::new(),
        }
    }
}
impl State {
    pub(crate) fn call(&mut self, op: &str, args: &mut Value) -> Result<Value> {
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
                self.next += 1;
                let id = self.next;
                let op = args[0].as_str().context("Expected operation")?.to_owned();
                let payload = crate::jsc::payload::Payload::snapshot(args[1].take());
                let (files, sender, wake) = (
                    self.files.clone(),
                    self.sender.clone(),
                    crate::host::Waker::current(),
                );
                blocking(move || {
                    let result = crate::jsc::payload::Payload::capture(payload.buffers, || {
                        match files.call(&op, &payload.value) {
                            Ok(value) => json!({"id":id,"value":value}),
                            Err(error) => {
                                json!({"id":id,"error":error.to_string(),"code":error.downcast_ref::<std::io::Error>().map(code)})
                            }
                        }
                    });
                    if sender.send(result).is_ok() {
                        wake.wake();
                    }
                })?;
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
                    let key = if args[1].is_null() {
                        None
                    } else {
                        Some(bytes(&args[1])?)
                    };
                    Hasher::new(args[0].as_str().context("Expected digest")?, key)?
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
                    .update(bytes(&args[1])?);
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
            "zlib.new" => {
                let dictionary = if args[4].is_null() {
                    None
                } else {
                    Some(bytes(&args[4])?)
                };
                let codec = Codec::new(
                    args[0].as_str().unwrap_or(""),
                    args[1] == true,
                    args[2].as_i64(),
                    args[3].as_u64(),
                    dictionary,
                )?;
                self.next += 1;
                self.codecs.insert(self.next, (codec, false));
                Ok(json!(self.next))
            }
            "zlib.write" => {
                let (codec, ended) = self
                    .codecs
                    .get_mut(&args[0].as_u64().context("Expected zlib ID")?)
                    .context("Zlib stream is closed")?;
                let output = codec.write(
                    bytes(&args[1])?,
                    args[2].as_u64().unwrap_or(0),
                    ended,
                    u64::MAX,
                )?;
                Ok(json!({"bytes":crate::jsc::payload::write(output),"ended":*ended}))
            }
            "zlib.params" => {
                if let Some((Codec::Encode(encoder), _)) = self
                    .codecs
                    .get_mut(&args[0].as_u64().context("Expected zlib ID")?)
                {
                    // WARNING: zlib-rs cannot switch to a level with another match strategy mid-stream, so such a change keeps the old level.
                    let _ = encoder.set_level(flate2::Compression::new(
                        args[1].as_u64().filter(|level| *level <= 9).unwrap_or(6) as u32,
                    ));
                }
                Ok(Value::Null)
            }
            "zlib.free" => {
                self.codecs
                    .remove(&args[0].as_u64().context("Expected zlib ID")?);
                Ok(Value::Null)
            }
            "loop" => {
                let (start, _, idle) = crate::host::IDLE.with(std::cell::Cell::get);
                Ok(start.map_or(Value::Null, |start| {
                    json!([
                        idle.as_secs_f64() * 1000.,
                        start.elapsed().as_secs_f64() * 1000.
                    ])
                }))
            }
            "tty.watch" => {
                #[cfg(unix)]
                {
                    use std::sync::atomic::{AtomicI32, Ordering::Relaxed};
                    type Subscriber = (
                        mpsc::Sender<crate::jsc::payload::Payload>,
                        crate::host::Waker,
                    );
                    // WARNING: the handler may only write to a non-blocking pipe; a thread turns each byte into `resize` events.
                    static PIPE: AtomicI32 = AtomicI32::new(-1);
                    static SUBSCRIBERS: Mutex<Vec<Subscriber>> = Mutex::new(Vec::new());
                    extern "C" fn resized(_: libc::c_int) {
                        unsafe { libc::write(PIPE.load(Relaxed), [1u8].as_ptr().cast(), 1) };
                    }
                    let mut subscribers = SUBSCRIBERS.lock().unwrap();
                    if PIPE.load(Relaxed) < 0 {
                        let mut fds = [0; 2];
                        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
                            return Err(std::io::Error::last_os_error().into());
                        }
                        unsafe { libc::fcntl(fds[1], libc::F_SETFL, libc::O_NONBLOCK) };
                        std::thread::Builder::new()
                            .name("apps-signal".into())
                            .spawn(move || {
                                let mut byte = 0u8;
                                loop {
                                    match unsafe { libc::read(fds[0], (&raw mut byte).cast(), 1) } {
                                        0 => break,
                                        1 => {
                                            SUBSCRIBERS.lock().unwrap().retain(|(sender, wake)| {
                                                let open = sender
                                                    .send(json!({"resize":true}).into())
                                                    .is_ok();
                                                wake.wake();
                                                open
                                            })
                                        }
                                        _ => {}
                                    }
                                }
                            })?;
                        PIPE.store(fds[1], Relaxed);
                        unsafe {
                            libc::signal(libc::SIGWINCH, resized as *const () as libc::sighandler_t)
                        };
                    }
                    subscribers.push((self.sender.clone(), crate::host::Waker::current()));
                }
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
