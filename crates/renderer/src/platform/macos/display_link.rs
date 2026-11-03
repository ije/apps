//! Frame pacing for macOS windows on `CVDisplayLink`. A window subscribes only while it wants
//! frames, so idle windows get no callbacks.
//!
//! WARNING: `CVDisplayLinkStop` returns before the last output callback ends, so links are never
//! released. Each display keeps one link in a static registry. The callback context is the
//! display id, not a pointer, and the callback only reads the registry.
//!
//! WARNING: The output callback takes the registry lock on the link's io thread, maybe while it
//! holds CoreVideo locks. Never call CoreVideo while holding the registry lock. Registry changes
//! and link start/stop run on the main thread only.
//!
//! `std::sync::Mutex` is used because on macOS it is an `os_unfair_lock`, whose priority
//! donation helps the high-priority io thread.

use anyhow::Result;
use core_graphics::display::CGDirectDisplayID;
use dispatch2::{
    _dispatch_source_type_data_add, DispatchObject, DispatchQueue, DispatchRetained, DispatchSource,
};
use std::{
    collections::{BTreeMap, btree_map},
    ffi::c_void,
    sync::{Mutex, MutexGuard, PoisonError},
};

static REGISTRY: Mutex<Registry> = Mutex::new(Registry::new());

struct Registry {
    displays: BTreeMap<CGDirectDisplayID, DisplayEntry>,
    next_subscriber_id: u64,
}

impl Registry {
    const fn new() -> Self {
        Registry {
            displays: BTreeMap::new(),
            next_subscriber_id: 0,
        }
    }
}

struct DisplayEntry {
    link: sys::DisplayLink,
    running: bool,
    subscribers: Vec<DisplayLinkSubscriber>,
}

struct DisplayLinkSubscriber {
    id: SubscriberId,
    frame_requests: DispatchRetained<DispatchSource>,
}

// SAFETY: The link and the dispatch sources are refcounted, thread-safe CoreVideo and GCD
// objects. The registry lock serializes all changes to the entry.
unsafe impl Send for DisplayEntry {}

#[derive(Copy, Clone, PartialEq, Eq)]
struct SubscriberId(u64);

fn lock_registry() -> MutexGuard<'static, Registry> {
    // Ignore poison: the map stays valid, and a panic in the extern "C" callback aborts.
    REGISTRY.lock().unwrap_or_else(PoisonError::into_inner)
}

unsafe extern "C" fn display_link_output_callback(
    _display_link_out: *mut sys::CVDisplayLink,
    _current_time: *const sys::CVTimeStamp,
    _output_time: *const sys::CVTimeStamp,
    _flags_in: i64,
    _flags_out: *mut i64,
    display_id: *mut c_void,
) -> i32 {
    let display_id = display_id as usize as CGDirectDisplayID;
    let registry = lock_registry();
    if let Some(entry) = registry.displays.get(&display_id) {
        for subscriber in &entry.subscribers {
            subscriber.frame_requests.merge_data(1);
        }
    }
    0
}

fn subscribe(
    display_id: CGDirectDisplayID,
    frame_requests: DispatchRetained<DispatchSource>,
) -> Result<SubscriberId> {
    let needs_link = !lock_registry().displays.contains_key(&display_id);
    let new_link = if needs_link {
        // Created outside the registry lock, see the lock order warning above.
        Some(unsafe {
            sys::DisplayLink::new(
                display_id,
                display_link_output_callback,
                display_id as usize as *mut c_void,
            )?
        })
    } else {
        None
    };

    let (subscriber_id, link_to_start) = {
        let mut registry = lock_registry();
        let registry = &mut *registry;
        let subscriber_id = SubscriberId(registry.next_subscriber_id);
        registry.next_subscriber_id += 1;
        let entry = match (registry.displays.entry(display_id), new_link) {
            // Dropping an unused `new_link` is safe: a link that never started has no io thread.
            (btree_map::Entry::Occupied(entry), _) => entry.into_mut(),
            (btree_map::Entry::Vacant(vacant), Some(link)) => vacant.insert(DisplayEntry {
                link,
                running: false,
                subscribers: Vec::new(),
            }),
            (btree_map::Entry::Vacant(_), None) => {
                // Entries are never removed, so this can't happen.
                anyhow::bail!("display link registry entry vanished for display {display_id}");
            }
        };
        entry.subscribers.push(DisplayLinkSubscriber {
            id: subscriber_id,
            frame_requests,
        });
        let link_to_start = if entry.running {
            None
        } else {
            entry.running = true;
            // Started after the lock is released.
            Some(entry.link.clone())
        };
        (subscriber_id, link_to_start)
    };

    if let Some(mut link) = link_to_start
        && let Err(error) = unsafe { link.start() }
    {
        let mut registry = lock_registry();
        if let Some(entry) = registry.displays.get_mut(&display_id) {
            entry.running = false;
            entry
                .subscribers
                .retain(|subscriber| subscriber.id != subscriber_id);
        }
        return Err(error);
    }

    Ok(subscriber_id)
}

fn unsubscribe(display_id: CGDirectDisplayID, subscriber_id: SubscriberId) {
    let link_to_stop = {
        let mut registry = lock_registry();
        let Some(entry) = registry.displays.get_mut(&display_id) else {
            return;
        };
        entry
            .subscribers
            .retain(|subscriber| subscriber.id != subscriber_id);
        if entry.subscribers.is_empty() && entry.running {
            entry.running = false;
            Some(entry.link.clone())
        } else {
            None
        }
    };

    if let Some(mut link) = link_to_stop {
        // A last output callback can still run after this. It finds no subscribers.
        let _ = unsafe { link.stop() };
    }
}

/// Per-window frame requests. The dispatch source merges vsync ticks from the display's io thread
/// and calls `callback(data)` on the main queue.
pub(crate) struct WindowFrameSource {
    frame_requests: DispatchRetained<DispatchSource>,
    registration: Option<(CGDirectDisplayID, SubscriberId)>,
}

impl WindowFrameSource {
    pub fn new(data: *mut c_void, callback: extern "C" fn(*mut c_void)) -> Self {
        let frame_requests = unsafe {
            let frame_requests = DispatchSource::new(
                &raw const _dispatch_source_type_data_add as *mut _,
                0,
                0,
                Some(DispatchQueue::main()),
            );
            frame_requests.set_context(data);
            frame_requests.set_event_handler_f(callback);
            // WARNING: Destroying a suspended dispatch source is undefined behavior.
            frame_requests.resume();
            frame_requests
        };
        Self {
            frame_requests,
            registration: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.registration.is_some()
    }

    pub fn start(&mut self, display_id: CGDirectDisplayID) -> Result<()> {
        if self
            .registration
            .is_some_and(|(display, _)| display == display_id)
        {
            return Ok(());
        }
        self.stop();
        let subscriber_id = subscribe(display_id, self.frame_requests.clone())?;
        self.registration = Some((display_id, subscriber_id));
        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some((display_id, subscriber_id)) = self.registration.take() {
            unsubscribe(display_id, subscriber_id);
        }
    }
}

impl Drop for WindowFrameSource {
    fn drop(&mut self) {
        self.stop();
        // The callback can't reach this source after `stop`. Cancel so the handler never runs
        // again: its context is the window's native view, which may be freed after this.
        self.frame_requests.cancel();
    }
}

mod sys {
    //! Derived from display-link crate under the following license:
    //! <https://github.com/BrainiumLLC/display-link/blob/master/LICENSE-MIT>
    //! Apple docs: [CVDisplayLink](https://developer.apple.com/documentation/corevideo/cvdisplaylinkoutputcallback?language=objc)

    use anyhow::Result;
    use core_graphics::display::CGDirectDisplayID;
    use foreign_types::{ForeignType, foreign_type};
    use std::{
        ffi::c_void,
        fmt::{self, Debug, Formatter},
    };

    #[derive(Debug)]
    pub enum CVDisplayLink {}

    foreign_type! {
        pub unsafe type DisplayLink {
            type CType = CVDisplayLink;
            fn drop = CVDisplayLinkRelease;
            fn clone = CVDisplayLinkRetain;
        }
    }

    impl Debug for DisplayLink {
        fn fmt(&self, formatter: &mut Formatter) -> fmt::Result {
            formatter
                .debug_tuple("DisplayLink")
                .field(&self.as_ptr())
                .finish()
        }
    }

    pub enum CVTimeStamp {}

    pub type CVDisplayLinkOutputCallback = unsafe extern "C" fn(
        display_link_out: *mut CVDisplayLink,
        current_time: *const CVTimeStamp,
        output_time: *const CVTimeStamp,
        flags_in: i64,
        flags_out: *mut i64,
        display_link_context: *mut c_void,
    ) -> i32;

    #[link(name = "CoreFoundation", kind = "framework")]
    #[link(name = "CoreVideo", kind = "framework")]
    #[allow(improper_ctypes, unknown_lints, clippy::duplicated_attributes)]
    unsafe extern "C" {
        pub fn CVDisplayLinkCreateWithActiveCGDisplays(
            display_link_out: *mut *mut CVDisplayLink,
        ) -> i32;
        pub fn CVDisplayLinkSetCurrentCGDisplay(
            display_link: &mut DisplayLinkRef,
            display_id: u32,
        ) -> i32;
        pub fn CVDisplayLinkSetOutputCallback(
            display_link: &mut DisplayLinkRef,
            callback: CVDisplayLinkOutputCallback,
            user_info: *mut c_void,
        ) -> i32;
        pub fn CVDisplayLinkStart(display_link: &mut DisplayLinkRef) -> i32;
        pub fn CVDisplayLinkStop(display_link: &mut DisplayLinkRef) -> i32;
        pub fn CVDisplayLinkRelease(display_link: *mut CVDisplayLink);
        pub fn CVDisplayLinkRetain(display_link: *mut CVDisplayLink) -> *mut CVDisplayLink;
    }

    impl DisplayLink {
        pub unsafe fn new(
            display_id: CGDirectDisplayID,
            callback: CVDisplayLinkOutputCallback,
            user_info: *mut c_void,
        ) -> Result<Self> {
            unsafe {
                let mut display_link: *mut CVDisplayLink = 0 as _;

                let code = CVDisplayLinkCreateWithActiveCGDisplays(&mut display_link);
                anyhow::ensure!(code == 0, "could not create display link, code: {}", code);

                let mut display_link = DisplayLink::from_ptr(display_link);

                let code = CVDisplayLinkSetOutputCallback(&mut display_link, callback, user_info);
                anyhow::ensure!(code == 0, "could not set output callback, code: {}", code);

                let code = CVDisplayLinkSetCurrentCGDisplay(&mut display_link, display_id);
                anyhow::ensure!(
                    code == 0,
                    "could not assign display to display link, code: {}",
                    code
                );

                Ok(display_link)
            }
        }
    }

    impl DisplayLinkRef {
        pub unsafe fn start(&mut self) -> Result<()> {
            unsafe {
                let code = CVDisplayLinkStart(self);
                anyhow::ensure!(code == 0, "could not start display link, code: {}", code);
                Ok(())
            }
        }

        pub unsafe fn stop(&mut self) -> Result<()> {
            unsafe {
                let code = CVDisplayLinkStop(self);
                anyhow::ensure!(code == 0, "could not stop display link, code: {}", code);
                Ok(())
            }
        }
    }
}
