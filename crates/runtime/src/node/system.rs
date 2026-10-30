use super::bytes;
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::io::Read;

#[cfg(windows)]
mod win {
    #[repr(C)]
    pub struct Memory {
        pub length: u32,
        pub load: u32,
        pub total: u64,
        pub available: u64,
        pub page_total: u64,
        pub page_available: u64,
        pub virtual_total: u64,
        pub virtual_available: u64,
        pub extended: u64,
    }
    #[repr(C)]
    pub struct Counters {
        pub size: u32,
        pub faults: u32,
        pub peak: usize,
        pub working_set: usize,
        pub pools: [usize; 4],
        pub pagefile: usize,
        pub peak_pagefile: usize,
    }
    #[repr(C)]
    pub struct Version {
        pub size: u32,
        pub major: u32,
        pub minor: u32,
        pub build: u32,
        pub platform: u32,
        pub service_pack: [u16; 128],
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn GlobalMemoryStatusEx(memory: *mut Memory) -> i32;
        pub fn GetTickCount64() -> u64;
        pub fn GetCurrentProcess() -> isize;
        pub fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, size: u32) -> i32;
        pub fn GetProcessTimes(
            process: isize,
            creation: *mut u64,
            exit: *mut u64,
            kernel: *mut u64,
            user: *mut u64,
        ) -> i32;
    }
    #[link(name = "ntdll")]
    unsafe extern "system" {
        pub fn RtlGetVersion(version: *mut Version) -> i32;
    }
    pub fn memory() -> Memory {
        let mut memory: Memory = unsafe { std::mem::zeroed() };
        memory.length = size_of::<Memory>() as u32;
        unsafe { GlobalMemoryStatusEx(&mut memory) };
        memory
    }
}

#[cfg(target_os = "macos")]
fn sysctl<T: Copy>(name: &str) -> Option<T> {
    let name = std::ffi::CString::new(name).ok()?;
    let mut value: T = unsafe { std::mem::zeroed() };
    let mut size = size_of::<T>();
    (unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&raw mut value).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } == 0)
        .then_some(value)
}

#[cfg(target_os = "linux")]
fn meminfo(key: &str) -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix(key)?.strip_prefix(':'))
                .and_then(|value| value.trim().trim_end_matches(" kB").parse::<u64>().ok())
        })
        .map_or(0, |value| value * 1024)
}

#[derive(Clone)]
pub(super) enum Hasher {
    Md5(md5::Md5),
    Sha224(sha2::Sha224),
    Digest(ring::digest::Context),
    Hmac(ring::hmac::Context),
}

impl Hasher {
    pub(super) fn new(name: &Value, key: &Value) -> Result<Self> {
        use sha2::Digest;
        let name = name.as_str().context("Expected digest")?;
        if !key.is_null() {
            let algorithm = match name {
                "sha1" => ring::hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY,
                "sha256" => ring::hmac::HMAC_SHA256,
                "sha384" => ring::hmac::HMAC_SHA384,
                "sha512" => ring::hmac::HMAC_SHA512,
                name => bail!("Digest method not supported: {name}"),
            };
            return Ok(Self::Hmac(ring::hmac::Context::with_key(
                &ring::hmac::Key::new(algorithm, bytes(key)?),
            )));
        }
        let algorithm = match name {
            "md5" => return Ok(Self::Md5(md5::Md5::new())),
            "sha224" => return Ok(Self::Sha224(sha2::Sha224::new())),
            "sha1" => &ring::digest::SHA1_FOR_LEGACY_USE_ONLY,
            "sha256" => &ring::digest::SHA256,
            "sha384" => &ring::digest::SHA384,
            "sha512" => &ring::digest::SHA512,
            name => bail!("Digest method not supported: {name}"),
        };
        Ok(Self::Digest(ring::digest::Context::new(algorithm)))
    }
    pub(super) fn update(&mut self, data: &[u8]) {
        use sha2::Digest;
        match self {
            Self::Md5(hasher) => hasher.update(data),
            Self::Sha224(hasher) => hasher.update(data),
            Self::Digest(context) => context.update(data),
            Self::Hmac(context) => context.update(data),
        }
    }
    pub(super) fn finish(self) -> Value {
        use sha2::Digest;
        crate::web::base64(&match self {
            Self::Md5(hasher) => hasher.finalize().to_vec(),
            Self::Sha224(hasher) => hasher.finalize().to_vec(),
            Self::Digest(context) => context.finish().as_ref().to_vec(),
            Self::Hmac(context) => context.sign().as_ref().to_vec(),
        })
    }
}

pub(crate) fn system(op: &str, args: &Value) -> Result<Value> {
    match op {
        "hash" => {
            let mut hasher = Hasher::new(&args[0], &args[2])?;
            hasher.update(bytes(&args[1])?);
            Ok(hasher.finish())
        }
        "pbkdf2" => {
            let algorithm = match args[4].as_str().unwrap_or("") {
                "sha1" => ring::pbkdf2::PBKDF2_HMAC_SHA1,
                "sha256" => ring::pbkdf2::PBKDF2_HMAC_SHA256,
                "sha384" => ring::pbkdf2::PBKDF2_HMAC_SHA384,
                "sha512" => ring::pbkdf2::PBKDF2_HMAC_SHA512,
                name => bail!("Invalid digest: {name}"),
            };
            let iterations = u32::try_from(args[2].as_u64().context("Expected iterations")?)
                .ok()
                .and_then(std::num::NonZeroU32::new)
                .context("Iterations must be a positive 32-bit integer")?;
            let length = args[3].as_u64().context("Expected key length")?;
            ensure!(length <= 1 << 26, "Key length exceeds 64 MiB");
            let mut key = vec![0; length as usize];
            if length > 0 {
                ring::pbkdf2::derive(
                    algorithm,
                    iterations,
                    bytes(&args[1])?,
                    bytes(&args[0])?,
                    &mut key,
                );
            }
            Ok(crate::web::base64(&key))
        }
        "zlib" => {
            use flate2::{Compression, read};
            let input = bytes(&args[2])?;
            let level = match args[3].as_u64() {
                Some(level @ 0..=9) => Compression::new(level as u32),
                _ => Compression::default(),
            };
            let gzip = input.starts_with(&[0x1f, 0x8b]);
            let reader: Box<dyn Read> = match (args[0].as_str().unwrap_or(""), args[1] == true) {
                ("gzip", false) => Box::new(read::GzEncoder::new(input, level)),
                ("deflate", false) => Box::new(read::ZlibEncoder::new(input, level)),
                ("deflate-raw", false) => Box::new(read::DeflateEncoder::new(input, level)),
                ("gzip", true) => Box::new(read::MultiGzDecoder::new(input)),
                ("unzip", true) if gzip => Box::new(read::MultiGzDecoder::new(input)),
                ("deflate" | "unzip", true) => Box::new(read::ZlibDecoder::new(input)),
                ("deflate-raw", true) => Box::new(read::DeflateDecoder::new(input)),
                _ => bail!("Unsupported zlib format"),
            };
            let limit = args[4].as_u64().unwrap_or(1 << 30);
            let mut output = Vec::new();
            reader
                .take(limit.saturating_add(1))
                .read_to_end(&mut output)?;
            ensure!(
                output.len() as u64 <= limit,
                "Output exceeds maxOutputLength"
            );
            Ok(crate::web::base64(&output))
        }
        "lookup" => {
            use std::net::ToSocketAddrs;
            let family = args[1].as_u64().unwrap_or(0);
            let addresses: Vec<_> = (args[0].as_str().context("Expected hostname")?, 0)
                .to_socket_addrs()?
                .map(|address| address.ip())
                .filter(|ip| family == 0 || (family == 4) == ip.is_ipv4())
                .map(|ip| json!({"address":ip.to_string(),"family":if ip.is_ipv4() { 4 } else { 6 }}))
                .collect();
            ensure!(!addresses.is_empty(), "No address found");
            Ok(json!(addresses))
        }
        "os.info" => {
            let mut info = json!({"machine":match std::env::consts::ARCH { "aarch64" if cfg!(windows) => "arm64", arch => arch },"release":"","version":"","model":"","speed":0,"totalmem":0,"ppid":null,"user":{"uid":-1,"gid":-1,"username":std::env::var("USERNAME").unwrap_or_default(),"homedir":std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_default(),"shell":null}});
            #[cfg(unix)]
            unsafe {
                use std::ffi::CStr;
                let text = |field: *const libc::c_char| {
                    CStr::from_ptr(field).to_string_lossy().into_owned()
                };
                let mut name: libc::utsname = std::mem::zeroed();
                if libc::uname(&mut name) == 0 {
                    info["release"] = text(name.release.as_ptr()).into();
                    info["version"] = text(name.version.as_ptr()).into();
                    info["machine"] = text(name.machine.as_ptr()).into();
                }
                info["ppid"] = std::os::unix::process::parent_id().into();
                info["ids"] = json!([
                    libc::getuid(),
                    libc::geteuid(),
                    libc::getgid(),
                    libc::getegid()
                ]);
                let mut entry: libc::passwd = std::mem::zeroed();
                let mut buffer = vec![0 as libc::c_char; 16384];
                let mut result = std::ptr::null_mut();
                if libc::getpwuid_r(
                    libc::geteuid(),
                    &mut entry,
                    buffer.as_mut_ptr(),
                    buffer.len(),
                    &mut result,
                ) == 0
                    && !result.is_null()
                {
                    info["user"] = json!({"uid":entry.pw_uid,"gid":entry.pw_gid,"username":text(entry.pw_name),"homedir":text(entry.pw_dir),"shell":text(entry.pw_shell)});
                }
            }
            #[cfg(target_os = "macos")]
            {
                info["totalmem"] = sysctl::<u64>("hw.memsize").unwrap_or(0).into();
                info["speed"] = (sysctl::<u64>("hw.cpufrequency").unwrap_or(0) / 1_000_000).into();
                let name = c"machdep.cpu.brand_string";
                let mut buffer = [0u8; 256];
                let mut size = buffer.len();
                if unsafe {
                    libc::sysctlbyname(
                        name.as_ptr(),
                        buffer.as_mut_ptr().cast(),
                        &mut size,
                        std::ptr::null_mut(),
                        0,
                    )
                } == 0
                {
                    info["model"] = String::from_utf8_lossy(&buffer[..size])
                        .trim_end_matches('\0')
                        .into();
                }
            }
            #[cfg(target_os = "linux")]
            {
                info["totalmem"] = meminfo("MemTotal").into();
                let cpu = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
                let field = |key: &str| {
                    cpu.lines().find_map(|line| {
                        line.strip_prefix(key)?
                            .trim_start()
                            .strip_prefix(':')
                            .map(str::trim)
                    })
                };
                info["model"] = field("model name").unwrap_or("").into();
                info["speed"] = field("cpu MHz")
                    .and_then(|value| value.parse::<f64>().ok())
                    .map_or(0, |value| value as u64)
                    .into();
            }
            #[cfg(windows)]
            {
                info["totalmem"] = win::memory().total.into();
                info["model"] = std::env::var("PROCESSOR_IDENTIFIER")
                    .unwrap_or_default()
                    .into();
                let mut version: win::Version = unsafe { std::mem::zeroed() };
                version.size = size_of::<win::Version>() as u32;
                if unsafe { win::RtlGetVersion(&mut version) } == 0 {
                    info["release"] =
                        format!("{}.{}.{}", version.major, version.minor, version.build).into();
                    info["version"] = "Windows".into();
                }
            }
            Ok(info)
        }
        "os.freemem" => {
            #[cfg(target_os = "macos")]
            return Ok(json!(
                sysctl::<u32>("vm.page_free_count").unwrap_or(0) as u64
                    * unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64
            ));
            #[cfg(target_os = "linux")]
            return Ok(json!(meminfo("MemAvailable")));
            #[cfg(windows)]
            return Ok(json!(win::memory().available));
            #[allow(unreachable_code)]
            Ok(json!(0))
        }
        "os.uptime" => {
            #[cfg(target_os = "macos")]
            return Ok(json!(sysctl::<libc::timeval>("kern.boottime").map_or(
                0,
                |boot| {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |now| now.as_secs() as i64 - boot.tv_sec)
                }
            )));
            #[cfg(target_os = "linux")]
            return Ok(json!(
                std::fs::read_to_string("/proc/uptime")
                    .ok()
                    .and_then(|text| text.split_whitespace().next()?.parse::<f64>().ok())
                    .unwrap_or(0.)
            ));
            #[cfg(windows)]
            return Ok(json!(unsafe { win::GetTickCount64() } / 1000));
            #[allow(unreachable_code)]
            Ok(json!(0))
        }
        "os.loadavg" => {
            let mut load = [0f64; 3];
            #[cfg(unix)]
            unsafe {
                libc::getloadavg(load.as_mut_ptr(), 3);
            }
            Ok(json!(load))
        }
        "os.interfaces" => {
            let mut result = serde_json::Map::new();
            #[cfg(unix)]
            unsafe {
                use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
                let ip = |address: *const libc::sockaddr| -> Option<(IpAddr, u32)> {
                    if address.is_null() {
                        return None;
                    }
                    match (*address).sa_family as i32 {
                        libc::AF_INET => Some((
                            IpAddr::V4(Ipv4Addr::from(u32::from_be(
                                (*address.cast::<libc::sockaddr_in>()).sin_addr.s_addr,
                            ))),
                            0,
                        )),
                        libc::AF_INET6 => {
                            let address = &*address.cast::<libc::sockaddr_in6>();
                            Some((
                                IpAddr::V6(Ipv6Addr::from(address.sin6_addr.s6_addr)),
                                address.sin6_scope_id,
                            ))
                        }
                        _ => None,
                    }
                };
                let mut list = std::ptr::null_mut();
                if libc::getifaddrs(&mut list) != 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                let mut macs = std::collections::HashMap::new();
                let mut entries = Vec::new();
                let mut node = list;
                while let Some(item) = node.as_ref() {
                    node = item.ifa_next;
                    if item.ifa_addr.is_null() {
                        continue;
                    }
                    let name = std::ffi::CStr::from_ptr(item.ifa_name)
                        .to_string_lossy()
                        .into_owned();
                    #[cfg(target_os = "macos")]
                    if (*item.ifa_addr).sa_family as i32 == libc::AF_LINK {
                        let link = &*item.ifa_addr.cast::<libc::sockaddr_dl>();
                        if link.sdl_alen == 6 {
                            let data = link
                                .sdl_data
                                .as_ptr()
                                .add(link.sdl_nlen as usize)
                                .cast::<u8>();
                            macs.insert(name, std::slice::from_raw_parts(data, 6).to_vec());
                        }
                        continue;
                    }
                    #[cfg(target_os = "linux")]
                    if (*item.ifa_addr).sa_family as i32 == libc::AF_PACKET {
                        let link = &*item.ifa_addr.cast::<libc::sockaddr_ll>();
                        if link.sll_halen == 6 {
                            macs.insert(name, link.sll_addr[..6].to_vec());
                        }
                        continue;
                    }
                    if let Some((address, scope)) = ip(item.ifa_addr) {
                        entries.push((
                            name,
                            address,
                            scope,
                            ip(item.ifa_netmask).map(|(mask, _)| mask),
                            item.ifa_flags & libc::IFF_LOOPBACK as u32 != 0,
                        ));
                    }
                }
                libc::freeifaddrs(list);
                for (name, address, scope, netmask, internal) in entries {
                    let netmask = netmask.unwrap_or(if address.is_ipv4() {
                        Ipv4Addr::UNSPECIFIED.into()
                    } else {
                        Ipv6Addr::UNSPECIFIED.into()
                    });
                    let prefix = match netmask {
                        IpAddr::V4(mask) => u32::from(mask).count_ones(),
                        IpAddr::V6(mask) => u128::from(mask).count_ones(),
                    };
                    let mac = macs.get(&name).map_or("00:00:00:00:00:00".into(), |mac| {
                        mac.iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect::<Vec<_>>()
                            .join(":")
                    });
                    let mut entry = json!({"address":address.to_string(),"netmask":netmask.to_string(),"family":if address.is_ipv4() { "IPv4" } else { "IPv6" },"mac":mac,"internal":internal,"cidr":format!("{address}/{prefix}")});
                    if address.is_ipv6() {
                        entry["scopeid"] = scope.into();
                    }
                    if let Value::Array(list) = result.entry(name).or_insert_with(|| json!([])) {
                        list.push(entry);
                    }
                }
            }
            Ok(Value::Object(result))
        }
        "process.memory" => {
            #[cfg(target_os = "macos")]
            unsafe {
                let mut info: libc::proc_taskinfo = std::mem::zeroed();
                let size = size_of::<libc::proc_taskinfo>() as i32;
                return Ok(json!(if libc::proc_pidinfo(
                    std::process::id() as i32,
                    libc::PROC_PIDTASKINFO,
                    0,
                    (&raw mut info).cast(),
                    size
                ) == size
                {
                    info.pti_resident_size
                } else {
                    0
                }));
            }
            #[cfg(target_os = "linux")]
            return Ok(json!(
                std::fs::read_to_string("/proc/self/statm")
                    .ok()
                    .and_then(|text| text.split_whitespace().nth(1)?.parse::<u64>().ok())
                    .unwrap_or(0)
                    * unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64
            ));
            #[cfg(windows)]
            unsafe {
                let mut counters: win::Counters = std::mem::zeroed();
                counters.size = size_of::<win::Counters>() as u32;
                win::K32GetProcessMemoryInfo(
                    win::GetCurrentProcess(),
                    &mut counters,
                    counters.size,
                );
                return Ok(json!(counters.working_set));
            }
            #[allow(unreachable_code)]
            Ok(json!(0))
        }
        "process.cpu" => {
            #[cfg(unix)]
            unsafe {
                let mut usage: libc::rusage = std::mem::zeroed();
                libc::getrusage(libc::RUSAGE_SELF, &mut usage);
                let micros =
                    |time: libc::timeval| time.tv_sec * 1_000_000 + i64::from(time.tv_usec);
                return Ok(json!({"user":micros(usage.ru_utime),"system":micros(usage.ru_stime)}));
            }
            #[cfg(windows)]
            unsafe {
                let (mut creation, mut exit, mut kernel, mut user) = (0, 0, 0, 0);
                win::GetProcessTimes(
                    win::GetCurrentProcess(),
                    &mut creation,
                    &mut exit,
                    &mut kernel,
                    &mut user,
                );
                return Ok(json!({"user":user / 10,"system":kernel / 10}));
            }
            #[allow(unreachable_code)]
            Ok(json!({"user":0,"system":0}))
        }
        "umask" => {
            #[cfg(unix)]
            unsafe {
                let mask = args[0].as_u64().unwrap_or(0) as libc::mode_t;
                let old = libc::umask(mask);
                if args[0].is_null() {
                    libc::umask(old);
                }
                return Ok(json!(old));
            }
            #[allow(unreachable_code)]
            Ok(json!(0))
        }
        "isatty" => {
            use std::io::IsTerminal;
            Ok(json!(match args[0].as_u64() {
                Some(0) => std::io::stdin().is_terminal(),
                Some(1) => std::io::stdout().is_terminal(),
                Some(2) => std::io::stderr().is_terminal(),
                _ => false,
            }))
        }
        _ => bail!("Unsupported Node operation: {op}"),
    }
}
