use anyhow::{Context, Result, bail, ensure};
use notify::{RecursiveMode, Watcher};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) fn bytes(value: &Value) -> Result<Vec<u8>> {
    if let Some(bytes) = crate::binary::read(value) {
        return bytes;
    }
    Ok(match value.as_str() {
        Some(text) => text.as_bytes().to_vec(),
        None => value["latin1"]
            .as_str()
            .context("Expected bytes")?
            .chars()
            .map(|c| c as u8)
            .collect(),
    })
}

pub(crate) fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
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

fn kind(ty: fs::FileType) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if ty.is_block_device() {
            return 0o060000;
        }
        if ty.is_char_device() {
            return 0o020000;
        }
        if ty.is_fifo() {
            return 0o010000;
        }
        if ty.is_socket() {
            return 0o140000;
        }
    }
    if ty.is_symlink() {
        0o120000
    } else if ty.is_dir() {
        0o040000
    } else if ty.is_file() {
        0o100000
    } else {
        0
    }
}

fn stat(meta: &fs::Metadata) -> Value {
    let ms = |time: std::io::Result<SystemTime>| {
        time.ok().map(|time| match time.duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_secs_f64() * 1000.,
            Err(error) => error.duration().as_secs_f64() * -1000.,
        })
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let time = |seconds: i64, nanos: i64| seconds as f64 * 1000. + nanos as f64 / 1e6;
        json!({"dev":meta.dev(),"ino":meta.ino(),"mode":meta.mode(),"nlink":meta.nlink(),"uid":meta.uid(),"gid":meta.gid(),"rdev":meta.rdev(),"size":meta.size(),"blksize":meta.blksize(),"blocks":meta.blocks(),"atimeMs":time(meta.atime(),meta.atime_nsec()),"mtimeMs":time(meta.mtime(),meta.mtime_nsec()),"ctimeMs":time(meta.ctime(),meta.ctime_nsec()),"birthtimeMs":ms(meta.created())})
    }
    #[cfg(not(unix))]
    {
        let mode = kind(meta.file_type())
            | if meta.permissions().readonly() {
                0o444
            } else {
                0o666
            };
        json!({"dev":0,"ino":0,"mode":mode,"nlink":1,"uid":0,"gid":0,"rdev":0,"size":meta.len(),"blksize":4096,"blocks":meta.len().div_ceil(512),"atimeMs":ms(meta.accessed()),"mtimeMs":ms(meta.modified()),"ctimeMs":ms(meta.modified()),"birthtimeMs":ms(meta.created())})
    }
}

#[derive(Default)]
struct Files(Mutex<(u64, HashMap<u64, Arc<File>>)>);

impl Files {
    fn call(&self, op: &str, args: &Value) -> Result<Value> {
        let path =
            || -> Result<PathBuf> { Ok(PathBuf::from(args[0].as_str().context("Expected path")?)) };
        match op {
            "constants" => {
                #[cfg(unix)]
                let errno: HashMap<_, _> = ERRNO.iter().copied().collect();
                #[cfg(not(unix))]
                let errno = HashMap::<&str, i32>::new();
                Ok(json!({
                    "fs":{"F_OK":0,"R_OK":4,"W_OK":2,"X_OK":1,"COPYFILE_EXCL":1,"COPYFILE_FICLONE":2,"COPYFILE_FICLONE_FORCE":4,"O_RDONLY":libc::O_RDONLY,"O_WRONLY":libc::O_WRONLY,"O_RDWR":libc::O_RDWR,"O_CREAT":libc::O_CREAT,"O_TRUNC":libc::O_TRUNC,"O_APPEND":libc::O_APPEND,"O_EXCL":libc::O_EXCL,"S_IFMT":0o170000,"S_IFREG":0o100000,"S_IFDIR":0o040000,"S_IFCHR":0o020000,"S_IFBLK":0o060000,"S_IFIFO":0o010000,"S_IFLNK":0o120000,"S_IFSOCK":0o140000},
                    "errno":errno,
                    "signals":SIGNALS.iter().copied().collect::<HashMap<_, _>>(),
                }))
            }
            "readFile" => {
                let data = fs::read(path()?)?;
                Ok(
                    if args[1]["encoding"].as_str().is_some_and(|encoding| {
                        encoding.eq_ignore_ascii_case("utf8")
                            || encoding.eq_ignore_ascii_case("utf-8")
                    }) {
                        json!(String::from_utf8_lossy(&data))
                    } else {
                        crate::web::base64(&data)
                    },
                )
            }
            "writeFile" | "appendFile" => {
                let mut options = OpenOptions::new();
                options
                    .write(true)
                    .create(true)
                    .append(op == "appendFile")
                    .truncate(op != "appendFile");
                let flag =
                    args[2]["flag"]
                        .as_str()
                        .unwrap_or(if op == "appendFile" { "a" } else { "w" });
                options.create_new(flag.contains('x'));
                if flag.starts_with('a') {
                    options.append(true).truncate(false);
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(args[2]["mode"].as_u64().unwrap_or(0o666) as u32);
                }
                options.open(path()?)?.write_all(&bytes(&args[1])?)?;
                Ok(Value::Null)
            }
            "mkdir" => {
                if args[1]["recursive"] == true {
                    fs::create_dir_all(path()?)?;
                } else {
                    fs::create_dir(path()?)?;
                }
                Ok(Value::Null)
            }
            "readdir" => {
                let root = path()?;
                let recursive = args[1]["recursive"] == true;
                let mut directories = vec![PathBuf::new()];
                let mut entries = Vec::new();
                let mut index = 0;
                while let Some(directory) = directories.get(index).cloned() {
                    index += 1;
                    let start = entries.len();
                    for entry in fs::read_dir(root.join(&directory))? {
                        let entry = entry?;
                        let ty = entry.file_type()?;
                        entries.push((directory.join(entry.file_name()), kind(ty)));
                    }
                    entries[start..].sort_unstable_by(|a, b| a.0.cmp(&b.0));
                    if recursive {
                        directories.extend(
                            entries[start..]
                                .iter()
                                .filter(|(_, ty)| *ty == 0o040000)
                                .map(|(name, _)| name.clone()),
                        );
                    }
                }
                Ok(json!(
                    entries
                        .iter()
                        .map(|(name, ty)| json!([name.to_string_lossy(), ty]))
                        .collect::<Vec<_>>()
                ))
            }
            "stat" => Ok(stat(&fs::metadata(path()?)?)),
            "lstat" => Ok(stat(&fs::symlink_metadata(path()?)?)),
            "rename" => {
                fs::rename(path()?, args[1].as_str().context("Expected destination")?)?;
                Ok(Value::Null)
            }
            "copyFile" => {
                let target = args[1].as_str().context("Expected destination")?;
                if args[2].as_u64().unwrap_or(0) & 1 != 0 && fs::symlink_metadata(target).is_ok() {
                    return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists).into());
                }
                fs::copy(path()?, target)?;
                Ok(Value::Null)
            }
            "link" => {
                fs::hard_link(path()?, args[1].as_str().context("Expected link")?)?;
                Ok(Value::Null)
            }
            "truncate" => {
                OpenOptions::new()
                    .write(true)
                    .open(path()?)?
                    .set_len(args[1].as_u64().unwrap_or(0))?;
                Ok(Value::Null)
            }
            "utimes" | "lutimes" => {
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStrExt;
                    let time = |value: &Value| -> Result<libc::timespec> {
                        let seconds = value.as_f64().context("Expected time")?;
                        Ok(libc::timespec {
                            tv_sec: seconds.floor() as _,
                            tv_nsec: ((seconds - seconds.floor()) * 1e9) as _,
                        })
                    };
                    let times = [time(&args[1])?, time(&args[2])?];
                    let file = std::ffi::CString::new(path()?.as_os_str().as_bytes())?;
                    let flags = if op == "lutimes" {
                        libc::AT_SYMLINK_NOFOLLOW
                    } else {
                        0
                    };
                    if unsafe {
                        libc::utimensat(libc::AT_FDCWD, file.as_ptr(), times.as_ptr(), flags)
                    } != 0
                    {
                        return Err(std::io::Error::last_os_error().into());
                    }
                }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::OpenOptionsExt;
                    let time = |value: &Value| -> Result<SystemTime> {
                        let seconds = value.as_f64().context("Expected time")?;
                        Ok(if seconds < 0. {
                            UNIX_EPOCH - std::time::Duration::from_secs_f64(-seconds)
                        } else {
                            UNIX_EPOCH + std::time::Duration::from_secs_f64(seconds)
                        })
                    };
                    // FILE_WRITE_ATTRIBUTES and FILE_FLAG_BACKUP_SEMANTICS let this work on directories and read-only files.
                    OpenOptions::new()
                        .access_mode(0x100)
                        .custom_flags(0x0200_0000)
                        .open(path()?)?
                        .set_times(
                            fs::FileTimes::new()
                                .set_accessed(time(&args[1])?)
                                .set_modified(time(&args[2])?),
                        )?;
                }
                Ok(Value::Null)
            }
            "chown" | "lchown" => {
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStrExt;
                    let file = std::ffi::CString::new(path()?.as_os_str().as_bytes())?;
                    let id = |value: &Value| value.as_i64().map_or(u32::MAX, |id| id as u32);
                    let chown = if op == "lchown" {
                        libc::lchown
                    } else {
                        libc::chown
                    };
                    if unsafe { chown(file.as_ptr(), id(&args[1]), id(&args[2])) } != 0 {
                        return Err(std::io::Error::last_os_error().into());
                    }
                }
                Ok(Value::Null)
            }
            "realpath" => {
                let path = fs::canonicalize(path()?)?.to_string_lossy().into_owned();
                Ok(json!(if let Some(share) = path.strip_prefix(r"\\?\UNC\") {
                    format!(r"\\{share}")
                } else if let Some(rest) = path.strip_prefix(r"\\?\")
                    && rest.as_bytes().get(1) == Some(&b':')
                {
                    rest.to_owned()
                } else {
                    path
                }))
            }
            "readlink" => Ok(json!(fs::read_link(path()?)?.to_string_lossy())),
            "symlink" => {
                #[cfg(unix)]
                std::os::unix::fs::symlink(path()?, args[1].as_str().context("Expected link")?)?;
                #[cfg(windows)]
                if args[2] == "dir" {
                    std::os::windows::fs::symlink_dir(
                        path()?,
                        args[1].as_str().context("Expected link")?,
                    )?;
                } else {
                    std::os::windows::fs::symlink_file(
                        path()?,
                        args[1].as_str().context("Expected link")?,
                    )?;
                }
                Ok(Value::Null)
            }
            "chmod" => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(
                        path()?,
                        fs::Permissions::from_mode(
                            args[1].as_u64().context("Expected mode")? as u32
                        ),
                    )?;
                    Ok(Value::Null)
                }
                #[cfg(not(unix))]
                bail!("chmod is unavailable on this platform")
            }
            "access" => {
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStrExt;
                    let p = path()?;
                    let p = std::ffi::CString::new(p.as_os_str().as_bytes())?;
                    let mode = args[1].as_i64().unwrap_or(0) as i32;
                    ensure!(mode & !7 == 0, "Invalid access mode");
                    if unsafe { libc::access(p.as_ptr(), mode) } != 0 {
                        return Err(std::io::Error::last_os_error().into());
                    }
                }
                #[cfg(not(unix))]
                {
                    let metadata = fs::metadata(path()?)?;
                    ensure!(
                        args[1].as_i64().unwrap_or(0) & 2 == 0
                            || !metadata.permissions().readonly(),
                        "File is readonly"
                    );
                }
                Ok(Value::Null)
            }
            "unlink" | "rm" | "rmdir" => {
                let p = path()?;
                let result = if op == "unlink" {
                    fs::remove_file(p)
                } else {
                    match fs::symlink_metadata(&p) {
                        Ok(meta) if meta.is_dir() => {
                            if args[1]["recursive"] == true {
                                fs::remove_dir_all(p)
                            } else {
                                fs::remove_dir(p)
                            }
                        }
                        Ok(_) => fs::remove_file(p),
                        Err(error) => Err(error),
                    }
                };
                if let Err(error) = result
                    && !(args[1]["force"] == true && error.kind() == std::io::ErrorKind::NotFound)
                {
                    return Err(error.into());
                }
                Ok(Value::Null)
            }
            "mkdtemp" => {
                for _ in 0..100 {
                    let mut random = [0u8; 8];
                    getrandom::fill(&mut random).map_err(|e| anyhow::anyhow!("{e}"))?;
                    let p = format!("{}{:016x}", path()?.display(), u64::from_ne_bytes(random));
                    match fs::create_dir(&p) {
                        Ok(()) => return Ok(json!(p)),
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(e) => return Err(e.into()),
                    }
                }
                bail!("Cannot create temporary directory")
            }
            "open" => {
                let flag = args[1].as_str().unwrap_or("r");
                let mut options = OpenOptions::new();
                if let Some(flags) = args[1].as_i64() {
                    let flags = flags as i32;
                    options
                        .read(flags & 3 != libc::O_WRONLY)
                        .write(flags & 3 != libc::O_RDONLY)
                        .create(flags & libc::O_CREAT != 0)
                        .truncate(flags & libc::O_TRUNC != 0)
                        .append(flags & libc::O_APPEND != 0)
                        .create_new(flags & libc::O_EXCL != 0);
                } else {
                    ensure!(
                        matches!(
                            flag,
                            "r" | "r+" | "w" | "w+" | "wx" | "wx+" | "a" | "a+" | "ax" | "ax+"
                        ),
                        "Unsupported file flag: {flag}"
                    );
                    options
                        .read(flag.starts_with('r') || flag.contains('+'))
                        .write(!flag.starts_with('r') || flag.contains('+'))
                        .create(flag.starts_with('w') || flag.starts_with('a'))
                        .truncate(flag.starts_with('w'))
                        .append(flag.starts_with('a'))
                        .create_new(flag.contains('x'));
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(args[2].as_u64().unwrap_or(0o666) as u32);
                }
                let file = Arc::new(options.open(path()?)?);
                let mut files = self.0.lock().unwrap();
                files.0 += 1;
                let id = files.0;
                files.1.insert(id, file);
                Ok(json!(id))
            }
            "close" => {
                let id = args[0].as_u64().context("Expected descriptor")?;
                ensure!(
                    self.0.lock().unwrap().1.remove(&id).is_some(),
                    "Bad file descriptor"
                );
                Ok(Value::Null)
            }
            "read" | "write" | "fsync" | "ftruncate" | "fstat" => {
                let id = args[0].as_u64().context("Expected descriptor")?;
                let file = self
                    .0
                    .lock()
                    .unwrap()
                    .1
                    .get(&id)
                    .cloned()
                    .context("Bad file descriptor")?;
                match op {
                    "read" => {
                        let size = args[1].as_u64().context("Expected read size")?;
                        ensure!(size <= 64 * 1024 * 1024, "Read exceeds 64 MiB");
                        let mut data = vec![0; size as usize];
                        let count = if let Some(position) = args[2].as_u64() {
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::FileExt;
                                file.read_at(&mut data, position)?
                            }
                            #[cfg(windows)]
                            {
                                use std::os::windows::fs::FileExt;
                                file.seek_read(&mut data, position)?
                            }
                        } else {
                            (&*file).read(&mut data)?
                        };
                        Ok(crate::web::base64(&data[..count]))
                    }
                    "write" => {
                        let data = bytes(&args[1])?;
                        Ok(json!(if let Some(position) = args[2].as_u64() {
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::FileExt;
                                file.write_at(&data, position)?
                            }
                            #[cfg(windows)]
                            {
                                use std::os::windows::fs::FileExt;
                                file.seek_write(&data, position)?
                            }
                        } else {
                            (&*file).write(&data)?
                        }))
                    }
                    "fsync" => {
                        file.sync_all()?;
                        Ok(Value::Null)
                    }
                    "fstat" => Ok(stat(&file.metadata()?)),
                    _ => {
                        file.set_len(args[1].as_u64().context("Expected length")?)?;
                        Ok(Value::Null)
                    }
                }
            }
            _ => system(op, args),
        }
    }
}

#[cfg(windows)]
mod win {
    #[repr(C)]
    pub struct Memory {
        pub length: u32,
        pub load: u32,
        pub total: u64,
        pub available: u64,
        pub page_total: u64,
        pub page_available: u64,
        pub virtual_total: u64,
        pub virtual_available: u64,
        pub extended: u64,
    }
    #[repr(C)]
    pub struct Counters {
        pub size: u32,
        pub faults: u32,
        pub peak: usize,
        pub working_set: usize,
        pub pools: [usize; 4],
        pub pagefile: usize,
        pub peak_pagefile: usize,
    }
    #[repr(C)]
    pub struct Version {
        pub size: u32,
        pub major: u32,
        pub minor: u32,
        pub build: u32,
        pub platform: u32,
        pub service_pack: [u16; 128],
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn GlobalMemoryStatusEx(memory: *mut Memory) -> i32;
        pub fn GetTickCount64() -> u64;
        pub fn GetCurrentProcess() -> isize;
        pub fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, size: u32) -> i32;
        pub fn GetProcessTimes(
            process: isize,
            creation: *mut u64,
            exit: *mut u64,
            kernel: *mut u64,
            user: *mut u64,
        ) -> i32;
    }
    #[link(name = "ntdll")]
    unsafe extern "system" {
        pub fn RtlGetVersion(version: *mut Version) -> i32;
    }
    pub fn memory() -> Memory {
        let mut memory: Memory = unsafe { std::mem::zeroed() };
        memory.length = size_of::<Memory>() as u32;
        unsafe { GlobalMemoryStatusEx(&mut memory) };
        memory
    }
}

#[cfg(target_os = "macos")]
fn sysctl<T: Copy>(name: &str) -> Option<T> {
    let name = std::ffi::CString::new(name).ok()?;
    let mut value: T = unsafe { std::mem::zeroed() };
    let mut size = size_of::<T>();
    (unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&raw mut value).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } == 0)
        .then_some(value)
}

#[cfg(target_os = "linux")]
fn meminfo(key: &str) -> u64 {
    fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix(key)?.strip_prefix(':'))
                .and_then(|value| value.trim().trim_end_matches(" kB").parse::<u64>().ok())
        })
        .map_or(0, |value| value * 1024)
}

#[derive(Clone)]
enum Hasher {
    Md5(md5::Md5),
    Sha1(sha1_smol::Sha1),
    Sha224(sha2::Sha224),
    Sha256(sha2::Sha256),
    Sha384(sha2::Sha384),
    Sha512(sha2::Sha512),
}

impl Hasher {
    fn new(name: &Value) -> Result<Self> {
        use sha2::Digest;
        Ok(match name.as_str().context("Expected digest")? {
            "md5" => Self::Md5(md5::Md5::new()),
            "sha1" => Self::Sha1(sha1_smol::Sha1::new()),
            "sha224" => Self::Sha224(sha2::Sha224::new()),
            "sha256" => Self::Sha256(sha2::Sha256::new()),
            "sha384" => Self::Sha384(sha2::Sha384::new()),
            "sha512" => Self::Sha512(sha2::Sha512::new()),
            name => bail!("Digest method not supported: {name}"),
        })
    }
    fn update(&mut self, data: &[u8]) {
        use sha2::Digest;
        match self {
            Self::Md5(hasher) => hasher.update(data),
            Self::Sha1(hasher) => hasher.update(data),
            Self::Sha224(hasher) => hasher.update(data),
            Self::Sha256(hasher) => hasher.update(data),
            Self::Sha384(hasher) => hasher.update(data),
            Self::Sha512(hasher) => hasher.update(data),
        }
    }
    fn finish(self) -> Value {
        use sha2::Digest;
        crate::web::base64(&match self {
            Self::Md5(hasher) => hasher.finalize().to_vec(),
            Self::Sha1(hasher) => hasher.digest().bytes().to_vec(),
            Self::Sha224(hasher) => hasher.finalize().to_vec(),
            Self::Sha256(hasher) => hasher.finalize().to_vec(),
            Self::Sha384(hasher) => hasher.finalize().to_vec(),
            Self::Sha512(hasher) => hasher.finalize().to_vec(),
        })
    }
}

fn system(op: &str, args: &Value) -> Result<Value> {
    match op {
        "hash" => {
            let mut hasher = Hasher::new(&args[0])?;
            hasher.update(&bytes(&args[1])?);
            Ok(hasher.finish())
        }
        "pbkdf2" => {
            let algorithm = match args[4].as_str().unwrap_or("") {
                "sha1" => ring::pbkdf2::PBKDF2_HMAC_SHA1,
                "sha256" => ring::pbkdf2::PBKDF2_HMAC_SHA256,
                "sha384" => ring::pbkdf2::PBKDF2_HMAC_SHA384,
                "sha512" => ring::pbkdf2::PBKDF2_HMAC_SHA512,
                name => bail!("Invalid digest: {name}"),
            };
            let iterations = u32::try_from(args[2].as_u64().context("Expected iterations")?)
                .ok()
                .and_then(std::num::NonZeroU32::new)
                .context("Iterations must be a positive 32-bit integer")?;
            let length = args[3].as_u64().context("Expected key length")?;
            ensure!(length <= 1 << 26, "Key length exceeds 64 MiB");
            let mut key = vec![0; length as usize];
            if length > 0 {
                ring::pbkdf2::derive(
                    algorithm,
                    iterations,
                    &bytes(&args[1])?,
                    &bytes(&args[0])?,
                    &mut key,
                );
            }
            Ok(crate::web::base64(&key))
        }
        "zlib" => {
            use flate2::{Compression, read};
            let data = bytes(&args[2])?;
            let level = match args[3].as_u64() {
                Some(level @ 0..=9) => Compression::new(level as u32),
                _ => Compression::default(),
            };
            let input = &data[..];
            let gzip = input.starts_with(&[0x1f, 0x8b]);
            let reader: Box<dyn Read> = match (args[0].as_str().unwrap_or(""), args[1] == true) {
                ("gzip", false) => Box::new(read::GzEncoder::new(input, level)),
                ("deflate", false) => Box::new(read::ZlibEncoder::new(input, level)),
                ("deflate-raw", false) => Box::new(read::DeflateEncoder::new(input, level)),
                ("gzip", true) => Box::new(read::MultiGzDecoder::new(input)),
                ("unzip", true) if gzip => Box::new(read::MultiGzDecoder::new(input)),
                ("deflate" | "unzip", true) => Box::new(read::ZlibDecoder::new(input)),
                ("deflate-raw", true) => Box::new(read::DeflateDecoder::new(input)),
                _ => bail!("Unsupported zlib format"),
            };
            let limit = args[4].as_u64().unwrap_or(1 << 30);
            let mut output = Vec::new();
            reader
                .take(limit.saturating_add(1))
                .read_to_end(&mut output)?;
            ensure!(
                output.len() as u64 <= limit,
                "Output exceeds maxOutputLength"
            );
            Ok(crate::web::base64(&output))
        }
        "lookup" => {
            use std::net::ToSocketAddrs;
            let family = args[1].as_u64().unwrap_or(0);
            let addresses: Vec<_> = (args[0].as_str().context("Expected hostname")?, 0)
                .to_socket_addrs()?
                .map(|address| address.ip())
                .filter(|ip| family == 0 || (family == 4) == ip.is_ipv4())
                .map(|ip| json!({"address":ip.to_string(),"family":if ip.is_ipv4() { 4 } else { 6 }}))
                .collect();
            ensure!(!addresses.is_empty(), "No address found");
            Ok(json!(addresses))
        }
        "os.info" => {
            let mut info = json!({"machine":match std::env::consts::ARCH { "aarch64" if cfg!(windows) => "arm64", arch => arch },"release":"","version":"","model":"","speed":0,"totalmem":0,"ppid":null,"user":{"uid":-1,"gid":-1,"username":std::env::var("USERNAME").unwrap_or_default(),"homedir":std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_default(),"shell":null}});
            #[cfg(unix)]
            unsafe {
                use std::ffi::CStr;
                let text = |field: *const libc::c_char| {
                    CStr::from_ptr(field).to_string_lossy().into_owned()
                };
                let mut name: libc::utsname = std::mem::zeroed();
                if libc::uname(&mut name) == 0 {
                    info["release"] = text(name.release.as_ptr()).into();
                    info["version"] = text(name.version.as_ptr()).into();
                    info["machine"] = text(name.machine.as_ptr()).into();
                }
                info["ppid"] = std::os::unix::process::parent_id().into();
                info["ids"] = json!([
                    libc::getuid(),
                    libc::geteuid(),
                    libc::getgid(),
                    libc::getegid()
                ]);
                let mut entry: libc::passwd = std::mem::zeroed();
                let mut buffer = vec![0 as libc::c_char; 16384];
                let mut result = std::ptr::null_mut();
                if libc::getpwuid_r(
                    libc::geteuid(),
                    &mut entry,
                    buffer.as_mut_ptr(),
                    buffer.len(),
                    &mut result,
                ) == 0
                    && !result.is_null()
                {
                    info["user"] = json!({"uid":entry.pw_uid,"gid":entry.pw_gid,"username":text(entry.pw_name),"homedir":text(entry.pw_dir),"shell":text(entry.pw_shell)});
                }
            }
            #[cfg(target_os = "macos")]
            {
                info["totalmem"] = sysctl::<u64>("hw.memsize").unwrap_or(0).into();
                info["speed"] = (sysctl::<u64>("hw.cpufrequency").unwrap_or(0) / 1_000_000).into();
                let name = c"machdep.cpu.brand_string";
                let mut buffer = [0u8; 256];
                let mut size = buffer.len();
                if unsafe {
                    libc::sysctlbyname(
                        name.as_ptr(),
                        buffer.as_mut_ptr().cast(),
                        &mut size,
                        std::ptr::null_mut(),
                        0,
                    )
                } == 0
                {
                    info["model"] = String::from_utf8_lossy(&buffer[..size])
                        .trim_end_matches('\0')
                        .into();
                }
            }
            #[cfg(target_os = "linux")]
            {
                info["totalmem"] = meminfo("MemTotal").into();
                let cpu = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
                let field = |key: &str| {
                    cpu.lines().find_map(|line| {
                        line.strip_prefix(key)?
                            .trim_start()
                            .strip_prefix(':')
                            .map(str::trim)
                    })
                };
                info["model"] = field("model name").unwrap_or("").into();
                info["speed"] = field("cpu MHz")
                    .and_then(|value| value.parse::<f64>().ok())
                    .map_or(0, |value| value as u64)
                    .into();
            }
            #[cfg(windows)]
            {
                info["totalmem"] = win::memory().total.into();
                info["model"] = std::env::var("PROCESSOR_IDENTIFIER")
                    .unwrap_or_default()
                    .into();
                let mut version: win::Version = unsafe { std::mem::zeroed() };
                version.size = size_of::<win::Version>() as u32;
                if unsafe { win::RtlGetVersion(&mut version) } == 0 {
                    info["release"] =
                        format!("{}.{}.{}", version.major, version.minor, version.build).into();
                    info["version"] = "Windows".into();
                }
            }
            Ok(info)
        }
        "os.freemem" => {
            #[cfg(target_os = "macos")]
            return Ok(json!(
                sysctl::<u32>("vm.page_free_count").unwrap_or(0) as u64
                    * unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64
            ));
            #[cfg(target_os = "linux")]
            return Ok(json!(meminfo("MemAvailable")));
            #[cfg(windows)]
            return Ok(json!(win::memory().available));
            #[allow(unreachable_code)]
            Ok(json!(0))
        }
        "os.uptime" => {
            #[cfg(target_os = "macos")]
            return Ok(json!(sysctl::<libc::timeval>("kern.boottime").map_or(
                0,
                |boot| {
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_or(0, |now| now.as_secs() as i64 - boot.tv_sec)
                }
            )));
            #[cfg(target_os = "linux")]
            return Ok(json!(
                fs::read_to_string("/proc/uptime")
                    .ok()
                    .and_then(|text| text.split_whitespace().next()?.parse::<f64>().ok())
                    .unwrap_or(0.)
            ));
            #[cfg(windows)]
            return Ok(json!(unsafe { win::GetTickCount64() } / 1000));
            #[allow(unreachable_code)]
            Ok(json!(0))
        }
        "os.loadavg" => {
            let mut load = [0f64; 3];
            #[cfg(unix)]
            unsafe {
                libc::getloadavg(load.as_mut_ptr(), 3);
            }
            Ok(json!(load))
        }
        "os.interfaces" => {
            let mut result = serde_json::Map::new();
            #[cfg(unix)]
            unsafe {
                use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
                let ip = |address: *const libc::sockaddr| -> Option<(IpAddr, u32)> {
                    if address.is_null() {
                        return None;
                    }
                    match (*address).sa_family as i32 {
                        libc::AF_INET => Some((
                            IpAddr::V4(Ipv4Addr::from(u32::from_be(
                                (*address.cast::<libc::sockaddr_in>()).sin_addr.s_addr,
                            ))),
                            0,
                        )),
                        libc::AF_INET6 => {
                            let address = &*address.cast::<libc::sockaddr_in6>();
                            Some((
                                IpAddr::V6(Ipv6Addr::from(address.sin6_addr.s6_addr)),
                                address.sin6_scope_id,
                            ))
                        }
                        _ => None,
                    }
                };
                let mut list = std::ptr::null_mut();
                if libc::getifaddrs(&mut list) != 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                let mut macs = HashMap::new();
                let mut entries = Vec::new();
                let mut node = list;
                while let Some(item) = node.as_ref() {
                    node = item.ifa_next;
                    if item.ifa_addr.is_null() {
                        continue;
                    }
                    let name = std::ffi::CStr::from_ptr(item.ifa_name)
                        .to_string_lossy()
                        .into_owned();
                    #[cfg(target_os = "macos")]
                    if (*item.ifa_addr).sa_family as i32 == libc::AF_LINK {
                        let link = &*item.ifa_addr.cast::<libc::sockaddr_dl>();
                        if link.sdl_alen == 6 {
                            let data = link
                                .sdl_data
                                .as_ptr()
                                .add(link.sdl_nlen as usize)
                                .cast::<u8>();
                            macs.insert(name, std::slice::from_raw_parts(data, 6).to_vec());
                        }
                        continue;
                    }
                    #[cfg(target_os = "linux")]
                    if (*item.ifa_addr).sa_family as i32 == libc::AF_PACKET {
                        let link = &*item.ifa_addr.cast::<libc::sockaddr_ll>();
                        if link.sll_halen == 6 {
                            macs.insert(name, link.sll_addr[..6].to_vec());
                        }
                        continue;
                    }
                    if let Some((address, scope)) = ip(item.ifa_addr) {
                        entries.push((
                            name,
                            address,
                            scope,
                            ip(item.ifa_netmask).map(|(mask, _)| mask),
                            item.ifa_flags & libc::IFF_LOOPBACK as u32 != 0,
                        ));
                    }
                }
                libc::freeifaddrs(list);
                for (name, address, scope, netmask, internal) in entries {
                    let netmask = netmask.unwrap_or(if address.is_ipv4() {
                        Ipv4Addr::UNSPECIFIED.into()
                    } else {
                        Ipv6Addr::UNSPECIFIED.into()
                    });
                    let prefix = match netmask {
                        IpAddr::V4(mask) => u32::from(mask).count_ones(),
                        IpAddr::V6(mask) => u128::from(mask).count_ones(),
                    };
                    let mac = macs.get(&name).map_or("00:00:00:00:00:00".into(), |mac| {
                        mac.iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect::<Vec<_>>()
                            .join(":")
                    });
                    let mut entry = json!({"address":address.to_string(),"netmask":netmask.to_string(),"family":if address.is_ipv4() { "IPv4" } else { "IPv6" },"mac":mac,"internal":internal,"cidr":format!("{address}/{prefix}")});
                    if address.is_ipv6() {
                        entry["scopeid"] = scope.into();
                    }
                    if let Value::Array(list) = result.entry(name).or_insert_with(|| json!([])) {
                        list.push(entry);
                    }
                }
            }
            Ok(Value::Object(result))
        }
        "process.memory" => {
            #[cfg(target_os = "macos")]
            unsafe {
                let mut info: libc::proc_taskinfo = std::mem::zeroed();
                let size = size_of::<libc::proc_taskinfo>() as i32;
                return Ok(json!(if libc::proc_pidinfo(
                    std::process::id() as i32,
                    libc::PROC_PIDTASKINFO,
                    0,
                    (&raw mut info).cast(),
                    size
                ) == size
                {
                    info.pti_resident_size
                } else {
                    0
                }));
            }
            #[cfg(target_os = "linux")]
            return Ok(json!(
                fs::read_to_string("/proc/self/statm")
                    .ok()
                    .and_then(|text| text.split_whitespace().nth(1)?.parse::<u64>().ok())
                    .unwrap_or(0)
                    * unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64
            ));
            #[cfg(windows)]
            unsafe {
                let mut counters: win::Counters = std::mem::zeroed();
                counters.size = size_of::<win::Counters>() as u32;
                win::K32GetProcessMemoryInfo(
                    win::GetCurrentProcess(),
                    &mut counters,
                    counters.size,
                );
                return Ok(json!(counters.working_set));
            }
            #[allow(unreachable_code)]
            Ok(json!(0))
        }
        "process.cpu" => {
            #[cfg(unix)]
            unsafe {
                let mut usage: libc::rusage = std::mem::zeroed();
                libc::getrusage(libc::RUSAGE_SELF, &mut usage);
                let micros =
                    |time: libc::timeval| time.tv_sec * 1_000_000 + i64::from(time.tv_usec);
                return Ok(json!({"user":micros(usage.ru_utime),"system":micros(usage.ru_stime)}));
            }
            #[cfg(windows)]
            unsafe {
                let (mut creation, mut exit, mut kernel, mut user) = (0, 0, 0, 0);
                win::GetProcessTimes(
                    win::GetCurrentProcess(),
                    &mut creation,
                    &mut exit,
                    &mut kernel,
                    &mut user,
                );
                return Ok(json!({"user":user / 10,"system":kernel / 10}));
            }
            #[allow(unreachable_code)]
            Ok(json!({"user":0,"system":0}))
        }
        "umask" => {
            #[cfg(unix)]
            unsafe {
                let mask = args[0].as_u64().unwrap_or(0) as libc::mode_t;
                let old = libc::umask(mask);
                if args[0].is_null() {
                    libc::umask(old);
                }
                return Ok(json!(old));
            }
            #[allow(unreachable_code)]
            Ok(json!(0))
        }
        "isatty" => {
            use std::io::IsTerminal;
            Ok(json!(match args[0].as_u64() {
                Some(0) => std::io::stdin().is_terminal(),
                Some(1) => std::io::stdout().is_terminal(),
                Some(2) => std::io::stderr().is_terminal(),
                _ => false,
            }))
        }
        _ => bail!("Unsupported Node operation: {op}"),
    }
}

pub(crate) struct State {
    files: Arc<Files>,
    next: u64,
    sender: mpsc::Sender<crate::binary::Payload>,
    receiver: mpsc::Receiver<crate::binary::Payload>,
    watchers: HashMap<u64, notify::RecommendedWatcher>,
    hashers: HashMap<u64, Hasher>,
    tasks: Option<mpsc::Sender<(u64, String, crate::binary::Payload)>>,
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
        }
    }
}
impl State {
    pub(crate) fn call(&mut self, op: &str, args: &Value) -> Result<Value> {
        match op {
            "poll" => Ok(json!(
                self.receiver
                    .try_iter()
                    .take(256)
                    .map(crate::binary::Payload::into_value)
                    .collect::<Vec<_>>()
            )),
            "async" => {
                let task = |files: &Files, id: u64, op: &str, args: crate::binary::Payload| {
                    crate::binary::Payload::capture(args.buffers, || {
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
                let payload = crate::binary::Payload::snapshot(args[1].clone());
                // Slow work gets its own thread so it cannot stall queued file operations.
                if matches!(op.as_str(), "lookup" | "zlib" | "pbkdf2") {
                    let (files, sender) = (self.files.clone(), self.sender.clone());
                    std::thread::Builder::new()
                        .name("apps-node".into())
                        .spawn(move || {
                            let _ = sender.send(task(&files, id, &op, payload));
                        })?;
                    return Ok(json!(id));
                }
                if self.tasks.is_none() {
                    let (tasks, receiver) =
                        mpsc::channel::<(u64, String, crate::binary::Payload)>();
                    let files = self.files.clone();
                    let sender = self.sender.clone();
                    std::thread::Builder::new()
                        .name("apps-files".into())
                        .spawn(move || {
                            while let Ok((id, op, args)) = receiver.recv() {
                                if sender.send(task(&files, id, &op, args)).is_err() {
                                    break;
                                }
                            }
                        })?;
                    self.tasks = Some(tasks);
                }
                self.tasks
                    .as_ref()
                    .unwrap()
                    .send((id, op, payload))
                    .context("Filesystem queue is closed")?;
                Ok(json!(id))
            }
            "watch" => {
                self.next += 1;
                let id = self.next;
                let sender = self.sender.clone();
                let root = Path::new(args[0].as_str().context("Expected watch path")?);
                let base = fs::canonicalize(root).unwrap_or_else(|_| root.to_owned());
                let mut watcher = notify::recommended_watcher(
                    move |event: notify::Result<notify::Event>| {
                        let value = match event {
                            Ok(event) => {
                                json!({"id":id,"watch":true,"event":if event.kind.is_modify(){"change"}else{"rename"},"paths":event.paths.iter().map(|path| path.strip_prefix(&base).ok().filter(|name| !name.as_os_str().is_empty()).or_else(|| path.file_name().map(Path::new)).unwrap_or(path).to_string_lossy().into_owned()).collect::<Vec<_>>()})
                            }
                            Err(e) => json!({"id":id,"watch":true,"error":e.to_string()}),
                        };
                        let _ = sender.send(value.into());
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
                    Hasher::new(&args[0])?
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
