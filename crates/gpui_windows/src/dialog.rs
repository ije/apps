use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Context as _, Result, anyhow};
use futures::{
    channel::oneshot,
    future::{Either, select},
    lock::Mutex,
};
use gpui::ForegroundExecutor;
use gpui_util::ResultExt;
use windows::{
    Win32::{
        Foundation::*,
        System::{Com::*, Threading::GetCurrentThreadId},
        UI::{
            Controls::{TASKDIALOG_NOTIFICATIONS, TDN_BUTTON_CLICKED},
            WindowsAndMessaging::*,
        },
    },
    core::{BOOL, HRESULT, w},
};

use crate::SafeHwnd;

pub(crate) struct DialogOwner {
    hwnd: HWND,
    closed: Arc<AtomicBool>,
    serialization: Mutex<()>,
}

impl DialogOwner {
    pub(crate) fn new(hwnd: HWND) -> Rc<Self> {
        Rc::new(Self {
            hwnd,
            closed: Arc::new(AtomicBool::new(false)),
            serialization: Mutex::new(()),
        })
    }

    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    pub(crate) async fn when_idle(&self) {
        // After close(), queued requests cannot start a native dialog. Only the
        // active dialog can still use the HWND, and it holds this lock until exit.
        let _guard = self.serialization.lock().await;
    }
}

#[derive(Clone, Default)]
struct Cancellation {
    requested: Arc<AtomicBool>,
    owner_closed: Option<Arc<AtomicBool>>,
}

impl Cancellation {
    fn new(owner: Option<&DialogOwner>) -> Self {
        Self {
            owner_closed: owner.map(|owner| owner.closed.clone()),
            ..Self::default()
        }
    }

    fn is_requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
            || self
                .owner_closed
                .as_ref()
                .is_some_and(|closed| closed.load(Ordering::Acquire))
    }
}

/// Native modal loops may synchronously message their owner, so the foreground
/// thread must keep pumping messages rather than waiting for the dialog thread.
/// Holding the owner's lock through native completion also allows window
/// destruction to wait for the dialog without blocking the foreground thread.
pub(crate) fn show_dialog<T: Send + 'static>(
    owner: Option<Rc<DialogOwner>>,
    executor: &ForegroundExecutor,
    dialog: impl FnOnce(HWND) -> Result<T> + Send + 'static,
) -> oneshot::Receiver<Result<T>> {
    let (mut sender, receiver) = oneshot::channel();
    let cancellation = Cancellation::new(owner.as_deref());
    if cancellation.is_requested() {
        sender.send(Err(anyhow!("dialog owner has closed"))).ok();
        return receiver;
    }
    let hwnd = owner.as_ref().map(|owner| SafeHwnd::from(owner.hwnd));
    executor
        .spawn(async move {
            // Let Windows manage enabling and activating the owner. Overlapping
            // modal loops could otherwise re-enable it while another dialog is open.
            let serialization = if let Some(owner) = owner.as_ref() {
                match select(owner.serialization.lock(), sender.cancellation()).await {
                    Either::Left((guard, _)) => Some(guard),
                    Either::Right(((), _)) => return,
                }
            } else {
                None
            };
            if sender.is_canceled() || cancellation.is_requested() {
                return;
            }
            let (result_sender, mut result_receiver) = oneshot::channel();
            let thread = std::thread::Builder::new()
                .name("windows-native-dialog".into())
                .spawn({
                    let cancellation = cancellation.clone();
                    move || {
                        let result = (|| {
                            let _apartment = Apartment::new()?;
                            let _timer = CancellationTimer::new(cancellation.clone())?;
                            if cancellation.is_requested() {
                                return Err(anyhow!("dialog cancelled"));
                            }
                            let result = dialog(hwnd.map(|hwnd| hwnd.as_raw()).unwrap_or_default());
                            if cancellation.is_requested() {
                                return Err(anyhow!("dialog cancelled"));
                            }
                            result
                        })();
                        result_sender.send(result).ok();
                    }
                });
            if let Err(error) = thread {
                drop(serialization);
                sender.send(Err(error.into())).ok();
                return;
            }
            let result = match select(&mut result_receiver, sender.cancellation()).await {
                Either::Left((result, _)) => result,
                Either::Right(((), _)) => {
                    cancellation.requested.store(true, Ordering::Release);
                    result_receiver.await
                }
            };
            // Neither launch another dialog nor destroy the owner until the
            // native loop exits, even if the caller no longer wants its result.
            drop(serialization);
            sender
                .send(result.unwrap_or_else(|error| Err(error.into())))
                .ok();
        })
        .detach();
    receiver
}

struct Apartment;

impl Apartment {
    fn new() -> Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
        Ok(Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

thread_local! {
    static CANCELLATION: RefCell<Option<Cancellation>> = const { RefCell::new(None) };
}

struct CancellationTimer(usize);

impl CancellationTimer {
    fn new(cancellation: Cancellation) -> Result<Self> {
        let timer = unsafe { SetTimer(None, 0, 100, Some(cancel_dialog)) };
        if timer == 0 {
            return Err(windows::core::Error::from_thread())
                .context("starting native dialog cancellation timer");
        }
        CANCELLATION.with(|slot| slot.replace(Some(cancellation)));
        Ok(Self(timer))
    }
}

impl Drop for CancellationTimer {
    fn drop(&mut self) {
        CANCELLATION.with(|slot| slot.take());
        unsafe { KillTimer(None, self.0).log_err() };
    }
}

unsafe extern "system" fn cancel_dialog(_: HWND, _: u32, _: usize, _: u32) {
    if cancellation_requested() {
        unsafe {
            EnumThreadWindows(GetCurrentThreadId(), Some(close_dialog), LPARAM(0))
                .ok()
                .log_err();
        };
    }
}

fn cancellation_requested() -> bool {
    CANCELLATION.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(Cancellation::is_requested)
    })
}

pub(crate) unsafe extern "system" fn task_dialog_callback(
    _: HWND,
    notification: TASKDIALOG_NOTIFICATIONS,
    button: WPARAM,
    _: LPARAM,
    has_cancel_button: isize,
) -> HRESULT {
    // WM_CLOSE requires TDF_ALLOW_DIALOG_CANCELLATION, which also enables Esc/X.
    // Only allow those user gestures when the prompt offered a Cancel button.
    if notification == TDN_BUTTON_CLICKED
        && button.0 == IDCANCEL.0 as usize
        && has_cancel_button == 0
        && !cancellation_requested()
    {
        S_FALSE
    } else {
        S_OK
    }
}

unsafe extern "system" fn close_dialog(hwnd: HWND, _: LPARAM) -> BOOL {
    // Shell dialogs can open further modal dialogs. Close only dialog windows
    // on this dedicated thread, not hidden COM infrastructure windows.
    let mut class_name = [0u16; 32];
    let length = unsafe { GetClassNameW(hwnd, &mut class_name) } as usize;
    let dialog_class = w!("#32770");
    if class_name.get(..length) == Some(unsafe { dialog_class.as_wide() }) {
        unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)).log_err() };
    }
    TRUE
}
