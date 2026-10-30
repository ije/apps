//! One module per OS. Each provides `Platform`, `Window`, `TextSystem`, `Atlas` and `post`.

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
pub(crate) use mac::*;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub(crate) use windows::*;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod linux;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub(crate) use linux::*;
