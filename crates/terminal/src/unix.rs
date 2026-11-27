use std::{
  ffi::CStr,
  fs::File,
  io::{Error, ErrorKind, Read, Result},
  os::{
    fd::{AsRawFd, FromRawFd, OwnedFd},
    unix::process::{CommandExt, ExitStatusExt},
  },
  process::Command,
  time::Duration,
};

/// Another process can keep the PTY open, so the wait for the last output has a limit.
pub const DRAIN: Duration = Duration::from_millis(100);

pub struct Pty {
  // The side of the PTY that the terminal reads and writes.
  master: File,
  // The write end of a pipe. Closing it stops the read thread, because another process can keep the PTY open.
  _stop: OwnedFd,
}

pub struct Reader {
  master: File,
  stopped: OwnedFd,
}

pub struct Child(std::process::Child);

pub struct Program {
  pub pty: Pty,
  pub reader: Reader,
  pub writer: File,
  pub child: Child,
  pub pid: u32,
}

// The size of the PTY, from columns, rows and the pixels of a cell.
fn window_size([cols, rows, cell_w, cell_h]: [u32; 4]) -> libc::winsize {
  libc::winsize {
    ws_row: rows as u16,
    ws_col: cols as u16,
    ws_xpixel: (cols * cell_w).min(65535) as u16,
    ws_ypixel: (rows * cell_h).min(65535) as u16,
  }
}

/// Starts a program in a new PTY. Without arguments it is the login shell of the user, and with `shell` the
/// first argument is a command line for that shell.
pub fn spawn(
  argv: &[&str],
  shell: bool,
  env: &[&str],
  cwd: &str,
  size: [u32; 4],
) -> Result<Program> {
  let (mut master, mut slave) = (0, 0);
  let size = window_size(size);
  if unsafe {
    libc::openpty(
      &mut master,
      &mut slave,
      std::ptr::null_mut(),
      std::ptr::null_mut(),
      &size as *const _ as *mut _,
    )
  } != 0
  {
    return Err(Error::last_os_error());
  }
  let (master, slave) = unsafe { (File::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
  let mut pipe = [0; 2];
  if unsafe { libc::pipe(pipe.as_mut_ptr()) } != 0 {
    return Err(Error::last_os_error());
  }
  let (stopped, stop) = unsafe { (OwnedFd::from_raw_fd(pipe[0]), OwnedFd::from_raw_fd(pipe[1])) };
  // Other programs of the app must not get the PTY or the pipe.
  for fd in [master.as_raw_fd(), slave.as_raw_fd(), pipe[0], pipe[1]] {
    unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
  }
  let home = std::env::var("HOME").unwrap_or_default();
  let login = || std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
  let mut command = match argv {
    // The login shell of the user. A shell is a login shell when its name starts with `-`.
    [] => {
      let shell = login();
      let mut command = Command::new(&shell);
      command.arg0(format!("-{}", shell.rsplit('/').next().unwrap_or("sh")));
      command
    }
    [line, ..] if shell => {
      let mut command = Command::new(login());
      command.args(["-l", "-c", line]);
      command
    }
    [program, args @ ..] => {
      let mut command = Command::new(program);
      command.args(args);
      command
    }
  };
  if !cwd.is_empty() {
    command.current_dir(cwd);
  } else if std::path::Path::new(&home).is_dir() {
    command.current_dir(&home);
  }
  if !env.is_empty() {
    command.env_clear();
  }
  for pair in env {
    let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
    command.env(key, value);
  }
  if !env.iter().any(|pair| pair.starts_with("TERM=")) {
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
  }
  let files = unsafe { libc::getdtablesize() }.min(65536);
  command
    .stdin(slave.try_clone()?)
    .stdout(slave.try_clone()?)
    .stderr(slave);
  // this runs in the new process before the program, where only system calls are safe.
  unsafe {
    command.pre_exec(move || {
      // The program gets the default signals, its own session with the PTY as its terminal, and
      // no other file of the app.
      for signal in [
        libc::SIGCHLD,
        libc::SIGHUP,
        libc::SIGINT,
        libc::SIGQUIT,
        libc::SIGTERM,
        libc::SIGALRM,
      ] {
        libc::signal(signal, libc::SIG_DFL);
      }
      let mut none: libc::sigset_t = std::mem::zeroed();
      libc::sigemptyset(&mut none);
      libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
      if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
        return Err(Error::last_os_error());
      }
      for fd in 3..files {
        libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
      }
      Ok(())
    })
  };
  let reader = Reader {
    master: master.try_clone()?,
    stopped,
  };
  let writer = master.try_clone()?;
  let child = command.spawn()?;
  Ok(Program {
    pty: Pty {
      master,
      _stop: stop,
    },
    reader,
    writer,
    pid: child.id(),
    child: Child(child),
  })
}

impl Pty {
  pub fn resize(&self, size: [u32; 4]) {
    let size = window_size(size);
    unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ as _, &size) };
  }

  /// Sends a signal to the process group of the program, which leads it.
  pub fn signal(&self, pid: u32, signal: i32) {
    unsafe { libc::kill(-(pid as i32), signal) };
  }

  /// The process in the foreground.
  pub fn foreground(&self, _: u32) -> Option<u32> {
    let pid = unsafe { libc::tcgetpgrp(self.master.as_raw_fd()) };
    (pid > 0).then_some(pid as u32)
  }

  /// Ends the program, when it still runs.
  pub fn hang_up(self, pid: Option<u32>) {
    drop(self);
    if let Some(pid) = pid {
      unsafe { libc::kill(-(pid as i32), libc::SIGHUP) };
    }
  }
}

impl Reader {
  /// Waits for output. Returns 0 at the end.
  pub fn read(&mut self, bytes: &mut [u8]) -> usize {
    let mut fds = [self.master.as_raw_fd(), self.stopped.as_raw_fd()].map(|fd| libc::pollfd {
      fd,
      events: libc::POLLIN,
      revents: 0,
    });
    while unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) } < 0 {
      if Error::last_os_error().kind() != ErrorKind::Interrupted {
        return 0;
      }
    }
    if fds[1].revents != 0 {
      return 0;
    }
    self.master.read(bytes).unwrap_or(0)
  }
}

impl Child {
  /// Waits without reaping, so the PID stays ours until `reap`.
  pub fn wait(&self) {
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    while unsafe {
      libc::waitid(
        libc::P_PID,
        self.0.id() as _,
        &mut info,
        libc::WEXITED | libc::WNOWAIT,
      )
    } != 0
      && Error::last_os_error().kind() == ErrorKind::Interrupted
    {}
  }

  /// Processes that keep the PTY open go on without the program.
  pub fn end_rest(&self) {}

  /// The exit code, or the name of the signal, like `Terminated: 15`.
  pub fn reap(mut self) -> (Option<u32>, Option<String>) {
    let Ok(status) = self.0.wait() else {
      return (None, None);
    };
    let Some(signal) = status.signal() else {
      return (Some(status.code().unwrap_or(1) as u32), None);
    };
    let name = unsafe { libc::strsignal(signal) };
    let name = if name.is_null() {
      format!("Signal {signal}")
    } else {
      unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned()
    };
    (None, Some(name))
  }
}

/// The name and the working directory of a process.
#[cfg(target_os = "macos")]
pub fn process(pid: u32) -> (String, String) {
  let pid = pid as i32;
  let text = |bytes: &[libc::c_char]| {
    let bytes: Vec<u8> = bytes
      .iter()
      .take_while(|byte| **byte != 0)
      .map(|byte| *byte as u8)
      .collect();
    String::from_utf8_lossy(&bytes).into_owned()
  };
  let mut name = [0 as libc::c_char; 64];
  unsafe { libc::proc_name(pid, name.as_mut_ptr().cast(), name.len() as u32) };
  let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
  let size = size_of::<libc::proc_vnodepathinfo>() as i32;
  let read = unsafe {
    libc::proc_pidinfo(
      pid,
      libc::PROC_PIDVNODEPATHINFO,
      0,
      (&raw mut info).cast(),
      size,
    )
  };
  let cwd = if read == size {
    text(info.pvi_cdir.vip_path.as_flattened())
  } else {
    String::new()
  };
  (text(&name), cwd)
}

#[cfg(not(target_os = "macos"))]
pub fn process(pid: u32) -> (String, String) {
  let name = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
  let cwd = std::fs::read_link(format!("/proc/{pid}/cwd"))
    .map(|path| path.to_string_lossy().into_owned())
    .unwrap_or_default();
  (name.trim_end().to_owned(), cwd)
}
