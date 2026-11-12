#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::WindowControl;
#[cfg(target_os = "macos")]
pub(crate) use macos::*;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::WindowControl;
#[cfg(target_os = "windows")]
pub(crate) use windows::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::WindowControl;
#[cfg(target_os = "linux")]
pub(crate) use linux::*;

// On Windows and Linux one thread runs the app and every window.
#[cfg(not(target_os = "macos"))]
mod single {
    use super::{native_post, on_main};
    use crate::ControlKind;
    use std::{sync::mpsc, time::Duration};

    pub(crate) type Task = Box<dyn FnOnce() + Send>;

    #[derive(Clone, Copy)]
    pub(crate) struct AppThread;

    pub(crate) fn app_thread() -> AppThread {
        AppThread
    }

    pub(crate) fn post(_: AppThread, task: Task) {
        native_post(task);
    }

    pub(crate) fn ask<R: Send + 'static>(
        _: AppThread,
        f: impl FnOnce() -> R + Send + 'static,
    ) -> Option<R> {
        if on_main() {
            return Some(f());
        }
        let (answer, wait) = mpsc::sync_channel(1);
        native_post(Box::new(move || {
            let _ = answer.send(f());
        }));
        wait.recv_timeout(Duration::from_millis(100)).ok()
    }

    pub(crate) fn native_sync<R: Send>(f: impl FnOnce() -> R + Send) -> R {
        if on_main() {
            return f();
        }
        let (answer, wait) = mpsc::sync_channel(1);
        let job: Box<dyn FnOnce() + Send + '_> = Box::new(move || {
            let _ = answer.send(f());
        });
        // SAFETY: this thread waits until the job ran or was dropped, so what the job borrows outlives it.
        native_post(unsafe { std::mem::transmute::<Box<dyn FnOnce() + Send + '_>, Task>(job) });
        wait.recv().expect("the main thread dropped its work")
    }

    // The app runs on this thread, and `app::run` drops it after the loop.
    pub(crate) fn spawn_render(
        init: impl FnOnce() + Send + 'static,
        _: impl FnOnce() + Send + 'static,
    ) {
        init();
    }

    pub(crate) fn end_thread() {}

    pub(crate) fn park_render() {}

    pub(crate) fn control_size(_: ControlKind) -> [f32; 2] {
        [0.; 2]
    }
}
#[cfg(not(target_os = "macos"))]
pub(crate) use single::*;
