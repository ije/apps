//! CPU, memory and GPU use of this process, read on a thread of its own.

use std::{
    collections::VecDeque,
    sync::{Mutex, mpsc},
    time::{Duration, Instant},
};

const INTERVAL: Duration = Duration::from_millis(500);
/// Rates are averaged over this window, so they don't change at every reading.
const WINDOW: Duration = Duration::from_secs(3);

#[derive(Clone, Copy)]
pub struct Usage {
    /// Percent of one core.
    pub cpu: f32,
    /// The memory the process owns, like Activity Monitor's Memory column.
    pub memory: u64,
    /// Percent of GPU time, where the platform reports it per process.
    pub gpu: Option<f32>,
}

/// Reads every half second into `usage` until the sender of `stop` is dropped.
pub fn run(usage: &Mutex<Option<Usage>>, stop: &mpsc::Receiver<()>) {
    let mut history = VecDeque::<(Instant, Duration, Option<Duration>)>::new();
    loop {
        let now = Instant::now();
        if let (Some(cpu), Some(memory)) = (cpu_time(), memory()) {
            let gpu = gpu_time();
            history.push_back((now, cpu, gpu));
            while history.len() > 2 && now.duration_since(history[0].0) > WINDOW {
                history.pop_front();
            }
            let (at, first_cpu, first_gpu) = history[0];
            let span = now.duration_since(at).as_secs_f32();
            if span > 0. {
                let rate = |time: Duration| time.as_secs_f32() / span * 100.;
                *usage.lock().unwrap_or_else(|error| error.into_inner()) = Some(Usage {
                    cpu: rate(cpu.saturating_sub(first_cpu)),
                    memory,
                    gpu: first_gpu
                        .zip(gpu)
                        .map(|(first, gpu)| rate(gpu.saturating_sub(first))),
                });
            }
        }
        if !matches!(
            stop.recv_timeout(INTERVAL),
            Err(mpsc::RecvTimeoutError::Timeout)
        ) {
            return;
        }
    }
}

#[cfg(unix)]
fn cpu_time() -> Option<Duration> {
    let mut usage = unsafe { std::mem::zeroed::<libc::rusage>() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return None;
    }
    let time = |time: libc::timeval| Duration::new(time.tv_sec as u64, time.tv_usec as u32 * 1000);
    Some(time(usage.ru_utime) + time(usage.ru_stime))
}

#[cfg(windows)]
fn cpu_time() -> Option<Duration> {
    use windows::Win32::{
        Foundation::FILETIME,
        System::Threading::{GetCurrentProcess, GetProcessTimes},
    };
    let mut times = [FILETIME::default(); 4];
    let [created, exited, kernel, user] = &mut times;
    unsafe { GetProcessTimes(GetCurrentProcess(), created, exited, kernel, user) }.ok()?;
    let ticks = |time: &FILETIME| (time.dwHighDateTime as u64) << 32 | time.dwLowDateTime as u64;
    Some(Duration::from_nanos(
        (ticks(&times[2]) + ticks(&times[3])) * 100,
    ))
}

/// `ri_phys_footprint`, the dirty and compressed memory of the process. Unlike the resident size, it leaves
/// out the shared pages of system frameworks.
#[cfg(target_os = "macos")]
fn memory() -> Option<u64> {
    let mut info = unsafe { std::mem::zeroed::<libc::rusage_info_v2>() };
    let status = unsafe {
        libc::proc_pid_rusage(
            std::process::id() as i32,
            libc::RUSAGE_INFO_V2,
            (&raw mut info).cast(),
        )
    };
    (status == 0).then_some(info.ri_phys_footprint)
}

/// `RssAnon`: the heap, stacks and private mappings, without mapped files.
#[cfg(target_os = "linux")]
fn memory() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status
        .lines()
        .find_map(|line| line.strip_prefix("RssAnon:"))?;
    Some(
        line.trim()
            .trim_end_matches("kB")
            .trim()
            .parse::<u64>()
            .ok()?
            * 1024,
    )
}

/// `PrivateUsage`, the commit size in Task Manager.
#[cfg(windows)]
fn memory() -> Option<u64> {
    use windows::Win32::System::{
        ProcessStatus::{
            K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
        },
        Threading::GetCurrentProcess,
    };
    let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
    unsafe {
        K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(),
            size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        )
    }
    .as_bool()
    .then_some(counters.PrivateUsage as u64)
}

/// The GPU time of this process: `accumulatedGPUTime` of the accelerator clients it created, which is what
/// Activity Monitor shows. Intel Macs don't report it.
#[cfg(target_os = "macos")]
fn gpu_time() -> Option<Duration> {
    use core_foundation::{
        array::CFArray,
        base::{CFType, TCFType},
        dictionary::{CFDictionary, CFDictionaryRef},
        number::CFNumber,
        string::CFString,
    };
    use std::ffi::{c_char, c_void};

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOServiceMatching(name: *const c_char) -> CFDictionaryRef;
        fn IOServiceGetMatchingServices(
            port: u32,
            matching: CFDictionaryRef,
            iterator: *mut u32,
        ) -> i32;
        fn IORegistryEntryGetChildIterator(
            entry: u32,
            plane: *const c_char,
            iterator: *mut u32,
        ) -> i32;
        fn IOIteratorNext(iterator: u32) -> u32;
        fn IORegistryEntryCreateCFProperties(
            entry: u32,
            properties: *mut CFDictionaryRef,
            allocator: *const c_void,
            options: u32,
        ) -> i32;
        fn IOObjectRelease(object: u32) -> i32;
    }

    let creator = format!("pid {},", std::process::id());
    let [creator_key, usage_key, time_key] =
        ["IOUserClientCreator", "AppUsage", "accumulatedGPUTime"].map(CFString::from_static_string);
    // WARNING: `io_name_t` is a 128-byte array that the call copies whole.
    let mut plane = [0 as c_char; 128];
    for (slot, byte) in plane.iter_mut().zip(b"IOService") {
        *slot = *byte as c_char;
    }
    let mut total = None;
    unsafe {
        let mut accelerators = 0;
        if IOServiceGetMatchingServices(
            0,
            IOServiceMatching(c"IOAccelerator".as_ptr()),
            &mut accelerators,
        ) != 0
        {
            return None;
        }
        loop {
            let accelerator = IOIteratorNext(accelerators);
            if accelerator == 0 {
                break;
            }
            let mut clients = 0;
            if IORegistryEntryGetChildIterator(accelerator, plane.as_ptr(), &mut clients) == 0 {
                loop {
                    let client = IOIteratorNext(clients);
                    if client == 0 {
                        break;
                    }
                    let mut properties = std::ptr::null();
                    if IORegistryEntryCreateCFProperties(
                        client,
                        &mut properties,
                        std::ptr::null(),
                        0,
                    ) == 0
                        && !properties.is_null()
                    {
                        let properties =
                            CFDictionary::<CFString, CFType>::wrap_under_create_rule(properties);
                        if properties
                            .find(&creator_key)
                            .and_then(|value| value.downcast::<CFString>())
                            .is_some_and(|value| value.to_string().starts_with(&creator))
                            && let Some(usage) = properties
                                .find(&usage_key)
                                .and_then(|value| value.downcast::<CFArray>())
                        {
                            for queue in usage.iter() {
                                let queue = CFDictionary::<CFString, CFType>::wrap_under_get_rule(
                                    *queue as CFDictionaryRef,
                                );
                                if let Some(time) = queue
                                    .find(&time_key)
                                    .and_then(|value| value.downcast::<CFNumber>())
                                    .and_then(|value| value.to_i64())
                                {
                                    *total.get_or_insert(0) += time as u64;
                                }
                            }
                        }
                    }
                    IOObjectRelease(client);
                }
                IOObjectRelease(clients);
            }
            IOObjectRelease(accelerator);
        }
        IOObjectRelease(accelerators);
    }
    total.map(Duration::from_nanos)
}

#[cfg(not(target_os = "macos"))]
fn gpu_time() -> Option<Duration> {
    None
}

/// One refresh of the screen with the key window.
#[cfg(target_os = "macos")]
pub fn refresh_period() -> Option<f32> {
    let rate = objc2_app_kit::NSScreen::mainScreen(objc2::MainThreadMarker::new()?)?
        .maximumFramesPerSecond();
    (rate > 0).then(|| 1. / rate as f32)
}

#[cfg(not(target_os = "macos"))]
pub fn refresh_period() -> Option<f32> {
    None
}
