use block2::RcBlock;
use futures::{StreamExt, channel::mpsc};
use gpui::{ForegroundExecutor, PowerMode, Task, ThermalState};
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSNotificationName, NSObjectProtocol, NSProcessInfo,
    NSProcessInfoPowerStateDidChangeNotification, NSProcessInfoThermalState,
    NSProcessInfoThermalStateDidChangeNotification,
};

pub fn thermal_state() -> ThermalState {
    match NSProcessInfo::processInfo().thermalState() {
        NSProcessInfoThermalState::Fair => ThermalState::Fair,
        NSProcessInfoThermalState::Serious => ThermalState::Serious,
        NSProcessInfoThermalState::Critical => ThermalState::Critical,
        _ => ThermalState::Nominal,
    }
}

pub fn power_mode() -> PowerMode {
    if NSProcessInfo::processInfo().isLowPowerModeEnabled() {
        PowerMode::LowPower
    } else {
        // This API does not distinguish Automatic from High Power Mode on macOS.
        PowerMode::Unknown
    }
}

pub fn observe_thermal_state(
    executor: &ForegroundExecutor,
    callback: Box<dyn FnMut()>,
) -> ProcessInfoObserver {
    // Foundation requires reading thermalState before registering for changes.
    // https://developer.apple.com/documentation/foundation/processinfo/thermalstatedidchangenotification
    thermal_state();
    ProcessInfoObserver::new(
        unsafe { NSProcessInfoThermalStateDidChangeNotification },
        executor,
        callback,
    )
}

pub fn observe_power_mode(
    executor: &ForegroundExecutor,
    callback: Box<dyn FnMut()>,
) -> ProcessInfoObserver {
    ProcessInfoObserver::new(
        unsafe { NSProcessInfoPowerStateDidChangeNotification },
        executor,
        callback,
    )
}

pub struct ProcessInfoObserver {
    center: Retained<NSNotificationCenter>,
    observer: Retained<ProtocolObject<dyn NSObjectProtocol>>,
    _task: Task<()>,
}

impl ProcessInfoObserver {
    fn new(
        name: &NSNotificationName,
        executor: &ForegroundExecutor,
        callback: Box<dyn FnMut()>,
    ) -> Self {
        Self::with_center(
            NSNotificationCenter::defaultCenter(),
            name,
            executor,
            callback,
        )
    }

    fn with_center(
        center: Retained<NSNotificationCenter>,
        name: &NSNotificationName,
        executor: &ForegroundExecutor,
        mut callback: Box<dyn FnMut()>,
    ) -> Self {
        let (sender, mut receiver) = mpsc::unbounded();
        let block = RcBlock::new(move |_: std::ptr::NonNull<NSNotification>| {
            // Notifications can arrive synchronously or off-thread. Defer application
            // callbacks so they cannot reenter GPUI while it is already borrowed.
            if sender.unbounded_send(()).is_err() {
                log::debug!("process info observer stopped before notification delivery");
            }
        });
        let observer = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(name),
                Some(&NSProcessInfo::processInfo()),
                None,
                &block,
            )
        };
        let task = executor.spawn(async move {
            while receiver.next().await.is_some() {
                callback();
            }
        });
        Self {
            center,
            observer,
            _task: task,
        }
    }
}

impl Drop for ProcessInfoObserver {
    fn drop(&mut self) {
        unsafe {
            self.center.removeObserver(self.observer.as_ref());
        }
    }
}
