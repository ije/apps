use super::{SIGNALS, bytes, system};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

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

// Match the positional Stats fields in node/index.js.
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
        json!([
            meta.dev(),
            meta.ino(),
            meta.mode(),
            meta.nlink(),
            meta.uid(),
            meta.gid(),
            meta.rdev(),
            meta.size(),
            meta.blksize(),
            meta.blocks(),
            time(meta.atime(), meta.atime_nsec()),
            time(meta.mtime(), meta.mtime_nsec()),
            time(meta.ctime(), meta.ctime_nsec()),
            ms(meta.created())
        ])
    }
    #[cfg(not(unix))]
    {
        let mode = kind(meta.file_type())
            | if meta.permissions().readonly() {
                0o444
            } else {
                0o666
            };
        json!([
            0,
            0,
            mode,
            1,
            0,
            0,
            0,
            meta.len(),
            4096,
            meta.len().div_ceil(512),
            ms(meta.accessed()),
            ms(meta.modified()),
            ms(meta.modified()),
            ms(meta.created())
        ])
    }
}

// Handles start here, so smaller descriptors are the ones that this process inherited, like fd 3 from a parent.
const FIRST_HANDLE: u64 = 1 << 20;

#[cfg(unix)]
const SYNC: i32 = libc::O_SYNC;
#[cfg(not(unix))]
const SYNC: i32 = 0;

fn open_options(flag: &Value, fallback: &str, mode: &Value) -> Result<OpenOptions> {
    use libc::{
        O_APPEND as A, O_CREAT as C, O_EXCL as X, O_RDONLY, O_RDWR, O_TRUNC as T, O_WRONLY,
    };
    let flags = match flag {
        Value::Number(number) => number.as_i64().context("Expected file flags")? as i32,
        _ => match flag.as_str().unwrap_or(fallback) {
            "r" => O_RDONLY,
            "rs" | "sr" => O_RDONLY | SYNC,
            "r+" => O_RDWR,
            "rs+" | "sr+" => O_RDWR | SYNC,
            "w" => T | C | O_WRONLY,
            "wx" | "xw" => T | C | O_WRONLY | X,
            "w+" => T | C | O_RDWR,
            "wx+" | "xw+" => T | C | O_RDWR | X,
            "a" => A | C | O_WRONLY,
            "ax" | "xa" => A | C | O_WRONLY | X,
            "as" | "sa" => A | C | O_WRONLY | SYNC,
            "a+" => A | C | O_RDWR,
            "ax+" | "xa+" => A | C | O_RDWR | X,
            "as+" | "sa+" => A | C | O_RDWR | SYNC,
            flag => bail!("Unsupported file flag: {flag}"),
        },
    };
    let mut options = OpenOptions::new();
    let access = flags & (O_RDONLY | O_WRONLY | O_RDWR);
    options.read(access != O_WRONLY).write(access != O_RDONLY);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .custom_flags(flags & !libc::O_ACCMODE)
            .mode(mode.as_u64().unwrap_or(0o666) as u32);
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
        options
            .append(flags & A != 0)
            .create(flags & C != 0)
            .truncate(flags & T != 0)
            .create_new(flags & X != 0);
    }
    Ok(options)
}

#[derive(Default)]
pub(super) struct Files(Mutex<(u64, HashMap<u64, Arc<File>>)>);

impl Files {
    fn file(&self, id: &Value) -> Result<Arc<File>> {
        let id = id.as_u64().context("Expected descriptor")?;
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut files = self.0.lock().unwrap();
        if let Some(file) = files.1.get(&id) {
            return Ok(file.clone());
        }
        #[cfg(unix)]
        if id < FIRST_HANDLE {
            use std::os::fd::FromRawFd;
            let copy = unsafe { libc::fcntl(id as libc::c_int, libc::F_DUPFD_CLOEXEC, 64) };
            if copy >= 0 {
                let file = Arc::new(unsafe { File::from_raw_fd(copy) });
                files.1.insert(id, file.clone());
                return Ok(file);
            }
        }
        Err(std::io::Error::from_raw_os_error(libc::EBADF).into())
    }

    pub(super) fn call(&self, op: &str, args: &Value) -> Result<Value> {
        let path =
            || -> Result<PathBuf> { Ok(PathBuf::from(args[0].as_str().context("Expected path")?)) };
        match op {
            "constants" => {
                #[cfg(unix)]
                let errno: HashMap<_, _> = super::ERRNO.iter().copied().collect();
                #[cfg(not(unix))]
                let errno = HashMap::<&str, i32>::new();
                #[cfg(unix)]
                let dlopen = json!({"RTLD_LAZY":libc::RTLD_LAZY,"RTLD_NOW":libc::RTLD_NOW,"RTLD_GLOBAL":libc::RTLD_GLOBAL,"RTLD_LOCAL":libc::RTLD_LOCAL});
                #[cfg(not(unix))]
                let dlopen = json!({});
                Ok(json!({
                    "fs":{"F_OK":0,"R_OK":4,"W_OK":2,"X_OK":1,"COPYFILE_EXCL":1,"COPYFILE_FICLONE":2,"COPYFILE_FICLONE_FORCE":4,"O_RDONLY":libc::O_RDONLY,"O_WRONLY":libc::O_WRONLY,"O_RDWR":libc::O_RDWR,"O_CREAT":libc::O_CREAT,"O_TRUNC":libc::O_TRUNC,"O_APPEND":libc::O_APPEND,"O_EXCL":libc::O_EXCL,"S_IFMT":0o170000,"S_IFREG":0o100000,"S_IFDIR":0o040000,"S_IFCHR":0o020000,"S_IFBLK":0o060000,"S_IFIFO":0o010000,"S_IFLNK":0o120000,"S_IFSOCK":0o140000},
                    "errno":errno,
                    "signals":SIGNALS.iter().copied().collect::<HashMap<_, _>>(),
                    "dlopen":dlopen,
                }))
            }
            "readFile" => {
                let data = if args[0].is_u64() {
                    let mut data = Vec::new();
                    (&*self.file(&args[0])?).read_to_end(&mut data)?;
                    data
                } else {
                    fs::read(path()?)?
                };
                Ok(
                    if args[1]["encoding"].as_str().is_some_and(|encoding| {
                        encoding.eq_ignore_ascii_case("utf8")
                            || encoding.eq_ignore_ascii_case("utf-8")
                    }) {
                        Value::String(String::from_utf8(data).unwrap_or_else(|error| {
                            String::from_utf8_lossy(error.as_bytes()).into_owned()
                        }))
                    } else {
                        crate::jsc::payload::write(data)
                    },
                )
            }
            "writeFile" | "appendFile" => {
                if args[0].is_u64() {
                    (&*self.file(&args[0])?).write_all(bytes(&args[1])?)?;
                    return Ok(Value::Null);
                }
                open_options(
                    &args[2]["flag"],
                    if op == "appendFile" { "a" } else { "w" },
                    &args[2]["mode"],
                )?
                .open(path()?)?
                .write_all(bytes(&args[1])?)?;
                Ok(Value::Null)
            }
            "mkdir" => {
                let path = path()?;
                if args[1]["recursive"] != true {
                    fs::create_dir(path)?;
                    return Ok(Value::Null);
                }
                let first = path
                    .ancestors()
                    .take_while(|p| !p.as_os_str().is_empty() && fs::symlink_metadata(p).is_err())
                    .last()
                    .map(|p| p.to_string_lossy().into_owned());
                fs::create_dir_all(&path)?;
                Ok(json!(first))
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
                    entries[start..].sort_unstable_by(|a, b| a.0.as_os_str().cmp(b.0.as_os_str()));
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
            "statfs" => {
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStrExt;
                    let file = std::ffi::CString::new(path()?.as_os_str().as_bytes())?;
                    let mut info: libc::statfs = unsafe { std::mem::zeroed() };
                    if unsafe { libc::statfs(file.as_ptr(), &mut info) } != 0 {
                        return Err(std::io::Error::last_os_error().into());
                    }
                    #[cfg(target_os = "linux")]
                    let fragment = info.f_frsize as u64;
                    #[cfg(not(target_os = "linux"))]
                    let fragment = info.f_bsize as u64;
                    Ok(json!([
                        info.f_type,
                        info.f_bsize,
                        fragment,
                        info.f_blocks,
                        info.f_bfree,
                        info.f_bavail,
                        info.f_files,
                        info.f_ffree
                    ]))
                }
                #[cfg(not(unix))]
                Err(std::io::Error::from(std::io::ErrorKind::Unsupported).into())
            }
            "stat" => Ok(stat(&fs::metadata(path()?)?)),
            "lstat" => Ok(stat(&fs::symlink_metadata(path()?)?)),
            "rename" => {
                fs::rename(path()?, args[1].as_str().context("Expected destination")?)?;
                Ok(Value::Null)
            }
            "copyFile" => {
                let (source, target) = (path()?, args[1].as_str().context("Expected destination")?);
                if args[2].as_u64().unwrap_or(0) & 1 != 0 && fs::symlink_metadata(target).is_ok() {
                    return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists).into());
                }
                // Skip copies onto the same inode: fs::copy truncates the destination first.
                #[cfg(unix)]
                let same = {
                    use std::os::unix::fs::MetadataExt;
                    let source = fs::metadata(&source)?;
                    fs::metadata(target).is_ok_and(|target| {
                        target.dev() == source.dev() && target.ino() == source.ino()
                    })
                };
                #[cfg(not(unix))]
                let same = fs::canonicalize(target).is_ok_and(|target| {
                    fs::canonicalize(&source).is_ok_and(|source| source == target)
                });
                if !same {
                    fs::copy(source, target)?;
                }
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
                        let offset = std::time::Duration::try_from_secs_f64(seconds.abs())?;
                        let time = if seconds < 0. {
                            UNIX_EPOCH.checked_sub(offset)
                        } else {
                            UNIX_EPOCH.checked_add(offset)
                        };
                        time.context("Time is out of range")
                    };
                    // FILE_WRITE_ATTRIBUTES and FILE_FLAG_BACKUP_SEMANTICS permit timestamp changes on
                    // directories and read-only files.
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
            "exists" => {
                // Like Node, this asks access() rather than stat(), which costs a third more.
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStrExt;
                    let path = std::ffi::CString::new(path()?.as_os_str().as_bytes())?;
                    Ok(Value::Bool(
                        unsafe { libc::access(path.as_ptr(), libc::F_OK) } == 0,
                    ))
                }
                #[cfg(not(unix))]
                Ok(Value::Bool(path()?.exists()))
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
            "unlink" => {
                fs::remove_file(path()?)?;
                Ok(Value::Null)
            }
            "rmdir" => {
                let p = path()?;
                if args[1]["recursive"] == true && fs::symlink_metadata(&p)?.is_dir() {
                    fs::remove_dir_all(p)?;
                } else {
                    fs::remove_dir(p)?;
                }
                Ok(Value::Null)
            }
            "rm" => {
                let p = path()?;
                let result = fs::symlink_metadata(&p).and_then(|meta| {
                    if !meta.is_dir() {
                        fs::remove_file(&p)
                    } else if args[1]["recursive"] == true {
                        fs::remove_dir_all(&p)
                    } else {
                        Err(std::io::ErrorKind::IsADirectory.into())
                    }
                });
                match result {
                    Err(error)
                        if !(args[1]["force"] == true
                            && error.kind() == std::io::ErrorKind::NotFound) =>
                    {
                        Err(error.into())
                    }
                    _ => Ok(Value::Null),
                }
            }
            "mkdtemp" => {
                const CHARS: &[u8] =
                    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
                for _ in 0..100 {
                    let mut random = [0u8; 6];
                    getrandom::fill(&mut random).map_err(|e| anyhow::anyhow!("{e}"))?;
                    let p = format!(
                        "{}{}",
                        path()?.display(),
                        random
                            .iter()
                            .map(|b| CHARS[*b as usize % CHARS.len()] as char)
                            .collect::<String>()
                    );
                    match fs::create_dir(&p) {
                        Ok(()) => return Ok(json!(p)),
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(e) => return Err(e.into()),
                    }
                }
                bail!("Cannot create temporary directory")
            }
            "open" => {
                let file = Arc::new(open_options(&args[1], "r", &args[2])?.open(path()?)?);
                let mut files = self.0.lock().unwrap();
                files.0 = files.0.max(FIRST_HANDLE) + 1;
                let id = files.0;
                files.1.insert(id, file);
                Ok(json!(id))
            }
            "close" => {
                let id = args[0].as_u64().context("Expected descriptor")?;
                let removed = self.0.lock().unwrap().1.remove(&id).is_some();
                #[cfg(unix)]
                let closed = id < FIRST_HANDLE && unsafe { libc::close(id as libc::c_int) } == 0;
                #[cfg(not(unix))]
                let closed = false;
                if !removed && !closed {
                    return Err(std::io::Error::from_raw_os_error(libc::EBADF).into());
                }
                Ok(Value::Null)
            }
            "read" | "write" | "fsync" | "fdatasync" | "ftruncate" | "fstat" | "fchmod"
            | "fchown" | "futimes" => {
                let file = self.file(&args[0])?;
                match op {
                    "read" => {
                        let size = args[1].as_u64().context("Expected read size")? as usize;
                        // The caller owns a buffer of this size already. A size that can't be had is an
                        // error for the caller, and must not end the app.
                        let mut data = Vec::new();
                        data.try_reserve_exact(size)
                            .map_err(|_| std::io::Error::from_raw_os_error(libc::ENOMEM))?;
                        data.resize(size, 0);
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
                        data.truncate(count);
                        Ok(crate::jsc::payload::write(data))
                    }
                    "write" => {
                        let data = bytes(&args[1])?;
                        Ok(json!(if let Some(position) = args[2].as_u64() {
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::FileExt;
                                file.write_at(data, position)?
                            }
                            #[cfg(windows)]
                            {
                                use std::os::windows::fs::FileExt;
                                file.seek_write(data, position)?
                            }
                        } else {
                            (&*file).write(data)?
                        }))
                    }
                    "fsync" => {
                        file.sync_all()?;
                        Ok(Value::Null)
                    }
                    "fdatasync" => {
                        file.sync_data()?;
                        Ok(Value::Null)
                    }
                    "fchmod" => {
                        let mode = args[1].as_u64().context("Expected mode")? as u32;
                        let mut permissions = file.metadata()?.permissions();
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            permissions.set_mode(mode);
                        }
                        #[cfg(not(unix))]
                        permissions.set_readonly(mode & 0o200 == 0);
                        file.set_permissions(permissions)?;
                        Ok(Value::Null)
                    }
                    "fchown" => {
                        #[cfg(unix)]
                        {
                            use std::os::fd::AsRawFd;
                            let id =
                                |value: &Value| value.as_i64().map_or(u32::MAX, |id| id as u32);
                            if unsafe { libc::fchown(file.as_raw_fd(), id(&args[1]), id(&args[2])) }
                                != 0
                            {
                                return Err(std::io::Error::last_os_error().into());
                            }
                        }
                        Ok(Value::Null)
                    }
                    "futimes" => {
                        let time = |value: &Value| -> Result<SystemTime> {
                            let seconds = value.as_f64().context("Expected time")?;
                            let offset = std::time::Duration::try_from_secs_f64(seconds.abs())?;
                            if seconds < 0. {
                                UNIX_EPOCH.checked_sub(offset)
                            } else {
                                UNIX_EPOCH.checked_add(offset)
                            }
                            .context("Time is out of range")
                        };
                        file.set_times(
                            fs::FileTimes::new()
                                .set_accessed(time(&args[1])?)
                                .set_modified(time(&args[2])?),
                        )?;
                        Ok(Value::Null)
                    }
                    "fstat" => Ok(stat(&file.metadata()?)),
                    _ => {
                        file.set_len(args[1].as_u64().context("Expected length")?)?;
                        Ok(Value::Null)
                    }
                }
            }
            _ => system::call(op, args),
        }
    }
}
