use anyhow::{Context, Result, bail, ensure};
use notify::{RecursiveMode, Watcher};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
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

pub(crate) fn code(error: &std::io::Error) -> &'static str {
    use std::io::ErrorKind::*;
    #[cfg(unix)]
    if error.raw_os_error() == Some(libc::EPERM) {
        return "EPERM";
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
        _ => "EIO",
    }
}

#[derive(Default)]
struct Files(Mutex<(u64, HashMap<u64, Arc<File>>)>);

impl Files {
    fn call(&self, op: &str, args: &Value) -> Result<Value> {
        let path =
            || -> Result<PathBuf> { Ok(PathBuf::from(args[0].as_str().context("Expected path")?)) };
        match op {
            "constants" => Ok(
                json!({"F_OK":0,"R_OK":4,"W_OK":2,"X_OK":1,"COPYFILE_EXCL":1,"O_RDONLY":libc::O_RDONLY,"O_WRONLY":libc::O_WRONLY,"O_RDWR":libc::O_RDWR,"O_CREAT":libc::O_CREAT,"O_TRUNC":libc::O_TRUNC,"O_APPEND":libc::O_APPEND,"O_EXCL":libc::O_EXCL}),
            ),
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
                let mut entries = Vec::new();
                for entry in fs::read_dir(path()?)? {
                    let entry = entry?;
                    let ty = entry.file_type()?;
                    entries.push(json!({"name":entry.file_name().to_string_lossy(),"file":ty.is_file(),"directory":ty.is_dir(),"symlink":ty.is_symlink()}));
                }
                entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
                Ok(json!(entries))
            }
            "stat" | "lstat" => {
                let meta = if op == "lstat" {
                    fs::symlink_metadata(path()?)?
                } else {
                    fs::metadata(path()?)?
                };
                #[cfg(unix)]
                let (mode, uid, gid, ino) = {
                    use std::os::unix::fs::MetadataExt;
                    (meta.mode(), meta.uid(), meta.gid(), meta.ino())
                };
                #[cfg(not(unix))]
                let (mode, uid, gid, ino) = (0u32, 0u32, 0u32, 0u64);
                #[cfg(unix)]
                let (block, character, fifo, socket) = {
                    use std::os::unix::fs::FileTypeExt;
                    let ty = meta.file_type();
                    (
                        ty.is_block_device(),
                        ty.is_char_device(),
                        ty.is_fifo(),
                        ty.is_socket(),
                    )
                };
                #[cfg(not(unix))]
                let (block, character, fifo, socket) = (false, false, false, false);
                let ms = |time: SystemTime| match time.duration_since(UNIX_EPOCH) {
                    Ok(duration) => duration.as_secs_f64() * 1000.,
                    Err(error) => error.duration().as_secs_f64() * -1000.,
                };
                Ok(
                    json!({"block":block,"character":character,"fifo":fifo,"socket":socket,"size":meta.len(),"mode":mode,"uid":uid,"gid":gid,"ino":ino,"file":meta.is_file(),"directory":meta.is_dir(),"symlink":meta.is_symlink(),"mtimeMs":ms(meta.modified()?),"birthtimeMs":meta.created().ok().map(ms)}),
                )
            }
            "rename" => {
                fs::rename(path()?, args[1].as_str().context("Expected destination")?)?;
                Ok(Value::Null)
            }
            "copyFile" => {
                fs::copy(path()?, args[1].as_str().context("Expected destination")?)?;
                Ok(Value::Null)
            }
            "realpath" => Ok(json!(fs::canonicalize(path()?)?.to_string_lossy())),
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
            "read" | "write" | "fsync" | "ftruncate" => {
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
                    _ => {
                        file.set_len(args[1].as_u64().context("Expected length")?)?;
                        Ok(Value::Null)
                    }
                }
            }
            _ => bail!("Unsupported filesystem operation: {op}"),
        }
    }
}

pub(crate) struct State {
    files: Arc<Files>,
    next: u64,
    sender: mpsc::Sender<crate::binary::Payload>,
    receiver: mpsc::Receiver<crate::binary::Payload>,
    watchers: HashMap<u64, notify::RecommendedWatcher>,
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
                if self.tasks.is_none() {
                    let (tasks, receiver) =
                        mpsc::channel::<(u64, String, crate::binary::Payload)>();
                    let files = self.files.clone();
                    let sender = self.sender.clone();
                    std::thread::Builder::new().name("apps-files".into()).spawn(move||{
                        while let Ok((id,op,args))=receiver.recv() {
                            let event=crate::binary::Payload::capture(args.buffers, || match files.call(&op,&args.value) {
                                Ok(value)=>json!({"id":id,"value":value}),
                                Err(error)=>json!({"id":id,"error":error.to_string(),"code":error.downcast_ref::<std::io::Error>().map(code)})
                            });
                            if sender.send(event).is_err(){break;}
                        }
                    })?;
                    self.tasks = Some(tasks);
                }
                self.next += 1;
                self.tasks
                    .as_ref()
                    .unwrap()
                    .send((
                        self.next,
                        args[0].as_str().context("Expected operation")?.to_owned(),
                        crate::binary::Payload::snapshot(args[1].clone()),
                    ))
                    .context("Filesystem queue is closed")?;
                Ok(json!(self.next))
            }
            "watch" => {
                self.next += 1;
                let id = self.next;
                let sender = self.sender.clone();
                let mut watcher = notify::recommended_watcher(
                    move |event: notify::Result<notify::Event>| {
                        let value = match event {
                            Ok(event) => {
                                json!({"id":id,"watch":true,"event":if event.kind.is_modify(){"change"}else{"rename"},"paths":event.paths.iter().map(|path| path.to_string_lossy()).collect::<Vec<_>>()})
                            }
                            Err(e) => json!({"id":id,"watch":true,"error":e.to_string()}),
                        };
                        let _ = sender.send(value.into());
                    },
                )?;
                watcher.watch(
                    std::path::Path::new(args[0].as_str().context("Expected watch path")?),
                    if args[1]["recursive"] == true {
                        RecursiveMode::Recursive
                    } else {
                        RecursiveMode::NonRecursive
                    },
                )?;
                self.watchers.insert(id, watcher);
                Ok(json!(id))
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
