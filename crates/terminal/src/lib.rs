//! The terminal of a session of the `<terminal>` element: libghostty-vt keeps the screen, a PTY runs the program, and
//! this library keeps the records of the program status protocol (OSC 7501).

// The crate has the name of its file, libGhostty.
#![allow(non_snake_case)]

// The PTY code is for Unix.
#[cfg(all(ghostty, unix))]
mod terminal;

#[cfg(all(ghostty, unix))]
pub use terminal::*;

/// Zero means that the library was built without libghostty-vt.
#[cfg(not(all(ghostty, unix)))]
#[unsafe(no_mangle)]
pub extern "C" fn apps_terminal_abi() -> u32 {
    0
}
