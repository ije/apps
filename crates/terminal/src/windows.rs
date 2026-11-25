use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString, c_void},
    fs::File,
    io::{Error, ErrorKind, Read, Result},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};
use windows::{
    Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation},
    Win32::{
        Foundation::{FILETIME, HANDLE, INVALID_HANDLE_VALUE, WAIT_TIMEOUT},
        System::{
            Console::{
                COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
                SetConsoleCtrlHandler,
            },
            Diagnostics::{
                Debug::ReadProcessMemory,
                ToolHelp::{
                    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                    TH32CS_SNAPPROCESS,
                },
            },
            Threading::*,
        },
    },
    core::{PCWSTR, PWSTR},
};

/// The console host ends after its last client, so this limit is only for a host that hangs.
pub const DRAIN: Duration = Duration::from_secs(1);

type Console = Arc<Mutex<Option<HPCON>>>;

// The wait thread closes the console, because that call can wait until the read thread has the last output.
// So it must never run with the lock of the state.
pub struct Pty {
    console: Console,
    process: Arc<OwnedHandle>,
    // The event that makes the wait thread close the console, which ends the programs in it.
    stop: Arc<OwnedHandle>,
}

pub struct Reader(File);

pub struct Child {
    // Without its pipes the console host ends at once, and the programs in the console go on.
    _input: File,
    console: Console,
    process: Arc<OwnedHandle>,
    stop: Arc<OwnedHandle>,
}

pub struct Program {
    pub pty: Pty,
    pub reader: Reader,
    pub writer: File,
    pub child: Child,
    pub pid: u32,
}

fn handle(handle: &impl AsRawHandle) -> HANDLE {
    HANDLE(handle.as_raw_handle())
}

fn wide(text: impl AsRef<OsStr>) -> Vec<u16> {
    text.as_ref().encode_wide().chain([0]).collect()
}

fn coord([cols, rows, ..]: [u32; 4]) -> COORD {
    COORD {
        X: cols.min(i16::MAX as u32) as i16,
        Y: rows.min(i16::MAX as u32) as i16,
    }
}

// Quotes an argument as the C runtime parses it.
fn quote(line: &mut String, arg: &str) {
    if !line.is_empty() {
        line.push(' ');
    }
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\x0b', '"']) {
        line.push_str(arg);
        return;
    }
    line.push('"');
    let mut slashes = 0;
    for c in arg.chars() {
        if c == '"' {
            line.extend(std::iter::repeat_n('\\', slashes + 1));
        }
        slashes = if c == '\\' { slashes + 1 } else { 0 };
        line.push(c);
    }
    line.extend(std::iter::repeat_n('\\', slashes));
    line.push('"');
}

// The environment block. Windows programs need some variables to start, so like Node.js an environment
// without them gets them from the app.
fn environment(env: &[&str]) -> Vec<u16> {
    // The names ignore case.
    let mut all = BTreeMap::<OsString, (OsString, OsString)>::new();
    let mut set = |key: OsString, value: OsString| {
        all.insert(key.to_ascii_uppercase(), (key, value));
    };
    for (key, value) in std::env::vars_os() {
        const NEEDED: [&str; 11] = [
            "HOMEDRIVE",
            "HOMEPATH",
            "LOGONSERVER",
            "PATH",
            "SYSTEMDRIVE",
            "SYSTEMROOT",
            "TEMP",
            "USERDOMAIN",
            "USERNAME",
            "USERPROFILE",
            "WINDIR",
        ];
        if env.is_empty() || NEEDED.iter().any(|name| key.eq_ignore_ascii_case(name)) {
            set(key, value);
        }
    }
    let mut pairs: Vec<_> = env
        .iter()
        .map(|pair| pair.split_once('=').unwrap_or((pair, "")))
        .collect();
    if !pairs
        .iter()
        .any(|(key, _)| key.eq_ignore_ascii_case("TERM"))
    {
        pairs.extend([("TERM", "xterm-256color"), ("COLORTERM", "truecolor")]);
    }
    for (key, value) in pairs {
        set(key.into(), value.into());
    }
    let mut block = Vec::new();
    for (key, value) in all.values() {
        block.extend(key.encode_wide());
        block.push(u16::from(b'='));
        block.extend(value.encode_wide());
        block.push(0);
    }
    block.push(0);
    block
}

/// Starts a program in a new pseudoconsole. Without arguments it is the command shell of the user, and with
/// `shell` the first argument is a command line for that shell.
pub fn spawn(
    argv: &[&str],
    shell: bool,
    env: &[&str],
    cwd: &str,
    size: [u32; 4],
) -> Result<Program> {
    let mut line = String::new();
    let comspec = || std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_owned());
    match argv {
        [] => quote(&mut line, &comspec()),
        [command, ..] if shell => {
            quote(&mut line, &comspec());
            // With /s the shell removes only the outer quotes.
            line.push_str(&format!(" /d /s /c \"{command}\""));
        }
        [program, ..] => {
            // The command shell runs batch files, and it parses their arguments by other rules.
            if Path::new(program.trim_end_matches([' ', '.']))
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("bat") || e.eq_ignore_ascii_case("cmd"))
            {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    "A batch file needs a command string",
                ));
            }
            for arg in argv {
                quote(&mut line, arg);
            }
        }
    }
    let mut line = wide(line);
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    let cwd = if !cwd.is_empty() { cwd } else { &home };
    let cwd = (!cwd.is_empty() && (cwd != home || Path::new(cwd).is_dir())).then(|| wide(cwd));
    let env = environment(env);

    let (input, writer) = std::io::pipe()?;
    let (reader, output) = std::io::pipe()?;
    let writer = File::from(OwnedHandle::from(writer));
    let kept = writer.try_clone()?;
    let stop = unsafe { OwnedHandle::from_raw_handle(CreateEventW(None, true, false, None)?.0) };
    let console = unsafe { CreatePseudoConsole(coord(size), handle(&input), handle(&output), 0)? };
    // The console host has its own handles of these ends. The output ends only when ours is closed.
    drop((input, output));
    let process = match start(console, &mut line, &env, cwd.as_deref()) {
        Ok(process) => process,
        Err(error) => {
            drop(reader);
            unsafe { ClosePseudoConsole(console) };
            return Err(error);
        }
    };
    let pid = unsafe { GetProcessId(handle(&process)) };
    let (process, stop) = (Arc::new(process), Arc::new(stop));
    let console = Arc::new(Mutex::new(Some(console)));
    Ok(Program {
        pty: Pty {
            console: console.clone(),
            process: process.clone(),
            stop: stop.clone(),
        },
        reader: Reader(OwnedHandle::from(reader).into()),
        writer,
        child: Child {
            _input: kept,
            console,
            process,
            stop,
        },
        pid,
    })
}

fn start(
    console: HPCON,
    line: &mut [u16],
    env: &[u16],
    cwd: Option<&[u16]>,
) -> Result<OwnedHandle> {
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    // Without this the program can get the redirected output of the app and not the console.
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
    startup.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
    startup.StartupInfo.hStdError = INVALID_HANDLE_VALUE;
    let mut process = PROCESS_INFORMATION::default();
    unsafe {
        // A program inherits from the app whether it ignores Ctrl+C.
        let _ = SetConsoleCtrlHandler(None, false);
        let mut size = 0;
        // This call only gives the size.
        let _ = InitializeProcThreadAttributeList(None, 1, None, &mut size);
        let mut attributes = vec![0usize; size.div_ceil(size_of::<usize>())];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(attributes.as_mut_ptr().cast());
        InitializeProcThreadAttributeList(Some(list), 1, None, &mut size)?;
        startup.lpAttributeList = list;
        let result = UpdateProcThreadAttribute(
            list,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            Some(console.0 as *const c_void),
            size_of::<HPCON>(),
            None,
            None,
        )
        .and_then(|()| {
            CreateProcessW(
                None,
                Some(PWSTR(line.as_mut_ptr())),
                None,
                None,
                false,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                Some(env.as_ptr().cast()),
                cwd.map_or(PCWSTR::null(), |cwd| PCWSTR(cwd.as_ptr())),
                (&raw const startup).cast(),
                &mut process,
            )
        });
        DeleteProcThreadAttributeList(list);
        result?;
        drop(OwnedHandle::from_raw_handle(process.hThread.0));
        Ok(OwnedHandle::from_raw_handle(process.hProcess.0))
    }
}

impl Pty {
    pub fn resize(&self, size: [u32; 4]) {
        let console = self.console.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(console) = *console {
            let _ = unsafe { ResizePseudoConsole(console, coord(size)) };
        }
    }

    /// `SIGHUP` closes the console, and other signals end the program at once.
    pub fn signal(&self, _: u32, signal: i32) {
        let _ = match signal {
            1 => unsafe { SetEvent(handle(&*self.stop)) },
            _ => unsafe { TerminateProcess(handle(&*self.process), 1) },
        };
    }

    /// Windows has no foreground process, so this is the newest process that the program started, and in
    /// turn the newest one of that process.
    pub fn foreground(&self, pid: u32) -> Option<u32> {
        let processes = processes();
        let (mut current, mut time) = (pid, started(pid)?);
        // The PID of a parent can belong to a newer process now, so a child must be newer than its parent.
        while let Some((child, at)) = processes
            .iter()
            .filter(|(child, parent)| *parent == current && *child != current)
            .filter_map(|(child, _)| Some((*child, started(*child)?)))
            .filter(|(_, at)| *at > time)
            .max_by_key(|(_, at)| *at)
        {
            (current, time) = (child, at);
        }
        Some(current)
    }

    /// Ends the program, when it still runs.
    pub fn hang_up(self, _: Option<u32>) {
        let _ = unsafe { SetEvent(handle(&*self.stop)) };
    }
}

impl Reader {
    /// Waits for output. Returns 0 at the end.
    pub fn read(&mut self, bytes: &mut [u8]) -> usize {
        self.0.read(bytes).unwrap_or(0)
    }
}

impl Child {
    /// Waits for the program, and closes its console.
    pub fn wait(&self) {
        unsafe {
            WaitForMultipleObjects(
                &[handle(&*self.process), handle(&*self.stop)],
                false,
                INFINITE,
            );
            let console = self
                .console
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some(console) = console {
                ClosePseudoConsole(console);
            }
            // A program that just starts can miss the close event.
            if WaitForSingleObject(handle(&*self.process), 5000) == WAIT_TIMEOUT {
                let _ = TerminateProcess(handle(&*self.process), 1);
                WaitForSingleObject(handle(&*self.process), INFINITE);
            }
        }
    }

    /// Ends the processes that keep the closed console open: a process that just starts can miss the close event.
    pub fn end_rest(&self) {
        let processes = processes();
        let mut tree = vec![unsafe { GetProcessId(handle(&*self.process)) }];
        let mut index = 0;
        while let Some(&parent) = tree.get(index) {
            index += 1;
            let time = started(parent);
            for &(child, of) in &processes {
                // The PID of a parent can belong to a newer process now.
                if of != parent || tree.contains(&child) || started(child) < time {
                    continue;
                }
                tree.push(child);
                // Window apps have no console.
                if parameter::<usize>(child, 0x10).is_some_and(|console| console != 0)
                    && let Some(process) = open(child, PROCESS_TERMINATE)
                {
                    let _ = unsafe { TerminateProcess(handle(&process), 1) };
                }
            }
        }
    }

    /// The exit code. Windows has no signals.
    pub fn reap(self) -> (Option<u32>, Option<String>) {
        let mut code = 0;
        let read = unsafe { GetExitCodeProcess(handle(&*self.process), &mut code) };
        (read.is_ok().then_some(code), None)
    }
}

fn open(pid: u32, access: PROCESS_ACCESS_RIGHTS) -> Option<OwnedHandle> {
    let process = unsafe { OpenProcess(access, false, pid) }.ok()?;
    Some(unsafe { OwnedHandle::from_raw_handle(process.0) })
}

fn started(pid: u32) -> Option<u64> {
    let process = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    let mut times = [FILETIME::default(); 4];
    let [start, exit, kernel, user] = &mut times;
    unsafe { GetProcessTimes(handle(&process), start, exit, kernel, user) }.ok()?;
    Some(u64::from(start.dwHighDateTime) << 32 | u64::from(start.dwLowDateTime))
}

/// The name and the working directory of a process.
pub fn process(pid: u32) -> (String, String) {
    let name = || {
        let process = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
        let mut path = [0; 1024];
        let mut len = path.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(
                handle(&process),
                PROCESS_NAME_WIN32,
                PWSTR(path.as_mut_ptr()),
                &mut len,
            )
        }
        .ok()?;
        let path = String::from_utf16_lossy(&path[..len as usize]);
        let name = path.rsplit('\\').next().unwrap_or(&path);
        let end = name.len().saturating_sub(4);
        Some(match name.get(end..) {
            Some(extension) if extension.eq_ignore_ascii_case(".exe") => name[..end].to_owned(),
            _ => name.to_owned(),
        })
    };
    (
        name().unwrap_or_default(),
        directory(pid).unwrap_or_default(),
    )
}

// All processes, each with its parent.
fn processes() -> Vec<(u32, u32)> {
    let mut processes = Vec::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return processes;
        };
        let snapshot = OwnedHandle::from_raw_handle(snapshot.0);
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut more = Process32FirstW(handle(&snapshot), &mut entry).is_ok();
        while more {
            let host = "conhost.exe\0".encode_utf16();
            if !entry.szExeFile.iter().copied().take(12).eq(host) {
                processes.push((entry.th32ProcessID, entry.th32ParentProcessID));
            }
            more = Process32NextW(handle(&snapshot), &mut entry).is_ok();
        }
    }
    processes
}

fn read<T: Copy + Default>(process: &OwnedHandle, from: usize) -> Option<T> {
    let mut value = T::default();
    let to = (&raw mut value).cast();
    unsafe { ReadProcessMemory(handle(process), from as _, to, size_of::<T>(), None) }.ok()?;
    Some(value)
}

// The process environment block of a process points to its parameters. The offsets are those of 64-bit Windows.
fn parameters(process: &OwnedHandle) -> Option<usize> {
    let mut info = PROCESS_BASIC_INFORMATION::default();
    let status = unsafe {
        NtQueryInformationProcess(
            handle(process),
            ProcessBasicInformation,
            (&raw mut info).cast(),
            size_of::<PROCESS_BASIC_INFORMATION>() as u32,
            &mut 0,
        )
    };
    if status.is_err() || info.PebBaseAddress.is_null() {
        return None;
    }
    read(process, info.PebBaseAddress as usize + 0x20)
}

fn parameter<T: Copy + Default>(pid: u32, offset: usize) -> Option<T> {
    let process = open(pid, PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ)?;
    read(&process, parameters(&process)? + offset)
}

fn directory(pid: u32) -> Option<String> {
    let process = open(pid, PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ)?;
    let parameters = parameters(&process)?;
    // A UNICODE_STRING: the length in bytes, and the address of the text.
    let len: u16 = read(&process, parameters + 0x38)?;
    let text: usize = read(&process, parameters + 0x40)?;
    let mut path = vec![0u16; usize::from(len) / 2];
    let to = path.as_mut_ptr().cast();
    unsafe { ReadProcessMemory(handle(&process), text as _, to, path.len() * 2, None) }.ok()?;
    let path = String::from_utf16_lossy(&path);
    // The directory ends with a backslash, which only the root of a drive keeps.
    Some(match path.strip_suffix('\\') {
        Some(short) if !short.ends_with(':') => short.to_owned(),
        _ => path,
    })
}
