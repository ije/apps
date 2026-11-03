use super::bytes;
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

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
    Keyed(Box<Hasher>, Box<Hasher>),
}

impl Hasher {
    pub(super) fn new(name: &str, key: Option<&[u8]>) -> Result<Self> {
        use sha2::Digest;
        if let Some(key) = key {
            let algorithm = match name {
                "sha1" => ring::hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY,
                "sha256" => ring::hmac::HMAC_SHA256,
                "sha384" => ring::hmac::HMAC_SHA384,
                "sha512" => ring::hmac::HMAC_SHA512,
                _ => {
                    let mut block = key.to_vec();
                    if block.len() > 64 {
                        let mut hasher = Self::new(name, None)?;
                        hasher.update(&block);
                        block = hasher.digest();
                    }
                    block.resize(64, 0);
                    let mut pads = [Self::new(name, None)?, Self::new(name, None)?];
                    for (hasher, pad) in pads.iter_mut().zip([0x36, 0x5c]) {
                        hasher.update(&block.iter().map(|byte| byte ^ pad).collect::<Vec<_>>());
                    }
                    let [inner, outer] = pads;
                    return Ok(Self::Keyed(Box::new(inner), Box::new(outer)));
                }
            };
            return Ok(Self::Hmac(ring::hmac::Context::with_key(
                &ring::hmac::Key::new(algorithm, key),
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
            Self::Keyed(inner, _) => inner.update(data),
        }
    }
    fn digest(self) -> Vec<u8> {
        use sha2::Digest;
        match self {
            Self::Md5(hasher) => hasher.finalize().to_vec(),
            Self::Sha224(hasher) => hasher.finalize().to_vec(),
            Self::Digest(context) => context.finish().as_ref().to_vec(),
            Self::Hmac(context) => context.sign().as_ref().to_vec(),
            Self::Keyed(inner, mut outer) => {
                outer.update(&inner.digest());
                outer.digest()
            }
        }
    }
    pub(super) fn finish(self) -> Value {
        crate::web::base64(&self.digest())
    }
}

pub(crate) enum Codec {
    Encode(flate2::Compress),
    Decode(flate2::Decompress, bool),
}

impl Codec {
    pub(super) fn new(
        format: &str,
        inflate: bool,
        level: Option<i64>,
        bits: Option<u64>,
        dictionary: Option<&[u8]>,
    ) -> Result<Self> {
        use flate2::{Compress, Compression, Decompress};
        let level = match level {
            Some(level @ 0..=9) => Compression::new(level as u32),
            _ => Compression::default(),
        };
        let bits = bits.filter(|bits| (9..=15).contains(bits)).unwrap_or(15) as u8;
        let mut codec = match (format, inflate) {
            ("gzip", false) => Self::Encode(Compress::new_gzip(level, bits)),
            ("deflate" | "deflate-raw", false) => Self::Encode(Compress::new_with_window_bits(
                level,
                format == "deflate",
                bits,
            )),
            ("gzip", true) => Self::Decode(Decompress::new_gzip(bits), true),
            ("deflate" | "deflate-raw", true) => Self::Decode(
                Decompress::new_with_window_bits(format == "deflate", bits),
                false,
            ),
            _ => bail!("Unsupported zlib format"),
        };
        if let Some(dictionary) = dictionary {
            match &mut codec {
                Self::Encode(encoder) => encoder.set_dictionary(dictionary).map(drop)?,
                Self::Decode(decoder, _) => decoder.set_dictionary(dictionary).map(drop)?,
            }
        }
        Ok(codec)
    }

    /// Runs `input` through the codec with a zlib flush mode (0 none, 2 sync, 3 full, 4 finish). Gzip input may hold several members.
    pub(super) fn write(
        &mut self,
        mut input: &[u8],
        flush: u64,
        ended: &mut bool,
        limit: u64,
    ) -> Result<Vec<u8>> {
        use flate2::{Decompress, FlushCompress, FlushDecompress, Status};
        let mut output =
            Vec::with_capacity((input.len() / 2 + 1024).min(limit.saturating_add(1) as usize));
        loop {
            if output.capacity() - output.len() < 1024 {
                output.reserve(output.capacity());
            }
            let (status, read, written) = match self {
                Self::Encode(encoder) => {
                    if *ended {
                        break;
                    }
                    let (read, written) = (encoder.total_in(), encoder.total_out());
                    let mode = match flush {
                        4 => FlushCompress::Finish,
                        3 => FlushCompress::Full,
                        1 | 2 => FlushCompress::Sync,
                        _ => FlushCompress::None,
                    };
                    let status =
                        encoder
                            .compress_vec(input, &mut output, mode)
                            .map_err(|error| {
                                anyhow::anyhow!(
                                    "{}",
                                    error.message().unwrap_or("compression failed")
                                )
                            })?;
                    (
                        status,
                        encoder.total_in() - read,
                        encoder.total_out() - written,
                    )
                }
                Self::Decode(decoder, gzip) => {
                    if *ended && *gzip && input.starts_with(&[0x1f, 0x8b]) {
                        *decoder = Decompress::new_gzip(15);
                        *ended = false;
                    }
                    if *ended {
                        break;
                    }
                    let (read, written) = (decoder.total_in(), decoder.total_out());
                    let mode = if flush == 4 {
                        FlushDecompress::Finish
                    } else {
                        FlushDecompress::Sync
                    };
                    let status =
                        decoder
                            .decompress_vec(input, &mut output, mode)
                            .map_err(|error| {
                                anyhow::anyhow!("{}", error.message().unwrap_or("invalid data"))
                            })?;
                    (
                        status,
                        decoder.total_in() - read,
                        decoder.total_out() - written,
                    )
                }
            };
            ensure!(
                output.len() as u64 <= limit,
                "Cannot create a Buffer larger than {limit} bytes"
            );
            input = &input[read as usize..];
            *ended |= status == Status::StreamEnd;
            if *ended && input.is_empty()
                || !*ended && input.is_empty() && output.len() < output.capacity()
                || read == 0 && written == 0 && !*ended
            {
                break;
            }
        }
        Ok(output)
    }
}

pub(crate) fn system(op: &str, args: &Value) -> Result<Value> {
    match op {
        "hash" => {
            let key = if args[2].is_null() {
                None
            } else {
                Some(bytes(&args[2])?)
            };
            let mut hasher = Hasher::new(args[0].as_str().context("Expected digest")?, key)?;
            hasher.update(bytes(&args[1])?);
            Ok(hasher.finish())
        }
        "hkdf" => {
            let name = args[0].as_str().context("Expected digest")?;
            let length = args[4].as_u64().context("Expected key length")? as usize;
            ensure!(length <= 255 * 64, "Invalid key length");
            let mut extract = Hasher::new(name, Some(bytes(&args[2])?))?;
            extract.update(bytes(&args[1])?);
            let key = extract.digest();
            let (mut output, mut block) = (Vec::with_capacity(length + 64), Vec::new());
            for counter in 1..=255u8 {
                if output.len() >= length {
                    break;
                }
                let mut mac = Hasher::new(name, Some(&key))?;
                mac.update(&block);
                mac.update(bytes(&args[3])?);
                mac.update(&[counter]);
                block = mac.digest();
                output.extend_from_slice(&block);
            }
            ensure!(output.len() >= length, "Invalid key length");
            output.truncate(length);
            Ok(crate::web::base64(&output))
        }
        "scrypt" => {
            let [n, r, p, length, limit] =
                [2, 3, 4, 5, 6].map(|index| args[index].as_u64().unwrap_or(0));
            ensure!(
                n > 1
                    && n.is_power_of_two()
                    && r > 0
                    && p > 0
                    && r.checked_mul(p).is_some_and(|size| size < 1 << 30)
                    && length <= 1 << 26,
                "Invalid scrypt params"
            );
            ensure!(
                128u64
                    .saturating_mul(r)
                    .saturating_mul(n.saturating_add(2 + p))
                    <= limit,
                "Invalid scrypt params: memory limit exceeded"
            );
            let (n, r) = (n as usize, r as usize);
            let password = bytes(&args[0])?;
            let once = std::num::NonZeroU32::MIN;
            let mut blocks = vec![0u8; 128 * r * p as usize];
            ring::pbkdf2::derive(
                ring::pbkdf2::PBKDF2_HMAC_SHA256,
                once,
                bytes(&args[1])?,
                password,
                &mut blocks,
            );
            let (mut v, mut x, mut y) = (
                vec![0u32; 32 * r * n],
                vec![0u32; 32 * r],
                vec![0u32; 32 * r],
            );
            // BlockMix with Salsa20/8 (RFC 7914): even outputs go to the first half of `y`, odd ones to the second.
            let mix = |x: &mut [u32], y: &mut [u32]| {
                let mut t: [u32; 16] = x[x.len() - 16..].try_into().unwrap();
                for i in 0..2 * r {
                    t.iter_mut().zip(&x[i * 16..]).for_each(|(a, b)| *a ^= b);
                    let mut s = t;
                    for _ in 0..4 {
                        for [a, b, c, d] in [
                            [0, 4, 8, 12],
                            [5, 9, 13, 1],
                            [10, 14, 2, 6],
                            [15, 3, 7, 11],
                            [0, 1, 2, 3],
                            [5, 6, 7, 4],
                            [10, 11, 8, 9],
                            [15, 12, 13, 14],
                        ] {
                            s[b] ^= s[a].wrapping_add(s[d]).rotate_left(7);
                            s[c] ^= s[b].wrapping_add(s[a]).rotate_left(9);
                            s[d] ^= s[c].wrapping_add(s[b]).rotate_left(13);
                            s[a] ^= s[d].wrapping_add(s[c]).rotate_left(18);
                        }
                    }
                    t.iter_mut()
                        .zip(s)
                        .for_each(|(a, b)| *a = a.wrapping_add(b));
                    let at = (i / 2 + i % 2 * r) * 16;
                    y[at..at + 16].copy_from_slice(&t);
                }
                x.copy_from_slice(y);
            };
            for block in blocks.chunks_exact_mut(128 * r) {
                for (word, chunk) in x.iter_mut().zip(block.as_chunks::<4>().0) {
                    *word = u32::from_le_bytes(*chunk);
                }
                for chunk in v.chunks_exact_mut(32 * r) {
                    chunk.copy_from_slice(&x);
                    mix(&mut x, &mut y);
                }
                for _ in 0..n {
                    let j = x[32 * r - 16] as usize & (n - 1);
                    x.iter_mut()
                        .zip(&v[j * 32 * r..])
                        .for_each(|(a, b)| *a ^= b);
                    mix(&mut x, &mut y);
                }
                for (chunk, word) in block.as_chunks_mut::<4>().0.iter_mut().zip(&x) {
                    *chunk = word.to_le_bytes();
                }
            }
            let mut key = vec![0; length as usize];
            ring::pbkdf2::derive(
                ring::pbkdf2::PBKDF2_HMAC_SHA256,
                once,
                &blocks,
                password,
                &mut key,
            );
            Ok(crate::web::base64(&key))
        }
        "pbkdf2" => {
            let algorithm = match args[4].as_str().unwrap_or("") {
                "sha1" => Some(ring::pbkdf2::PBKDF2_HMAC_SHA1),
                "sha256" => Some(ring::pbkdf2::PBKDF2_HMAC_SHA256),
                "sha384" => Some(ring::pbkdf2::PBKDF2_HMAC_SHA384),
                "sha512" => Some(ring::pbkdf2::PBKDF2_HMAC_SHA512),
                "md5" | "sha224" => None,
                name => bail!("Invalid digest: {name}"),
            };
            let iterations = u32::try_from(args[2].as_u64().context("Expected iterations")?)
                .ok()
                .and_then(std::num::NonZeroU32::new)
                .context("Iterations must be a positive 32-bit integer")?;
            let length = args[3].as_u64().context("Expected key length")?;
            ensure!(length <= 1 << 26, "Key length exceeds 64 MiB");
            let mut key = vec![0; length as usize];
            if let Some(algorithm) = algorithm
                && length > 0
            {
                ring::pbkdf2::derive(
                    algorithm,
                    iterations,
                    bytes(&args[1])?,
                    bytes(&args[0])?,
                    &mut key,
                );
            } else if length > 0 {
                let mac = Hasher::new(args[4].as_str().unwrap_or(""), Some(bytes(&args[0])?))?;
                for (index, block) in key
                    .chunks_mut(if args[4] == "md5" { 16 } else { 28 })
                    .enumerate()
                {
                    let mut next = mac.clone();
                    next.update(bytes(&args[1])?);
                    next.update(&(index as u32 + 1).to_be_bytes());
                    let mut round = next.digest();
                    let mut sum = round.clone();
                    for _ in 1..iterations.get() {
                        let mut next = mac.clone();
                        next.update(&round);
                        round = next.digest();
                        sum.iter_mut().zip(&round).for_each(|(a, b)| *a ^= b);
                    }
                    block.copy_from_slice(&sum[..block.len()]);
                }
            }
            Ok(crate::web::base64(&key))
        }
        "zlib" => {
            let input = bytes(&args[2])?;
            let format = match args[0].as_str().unwrap_or("") {
                "unzip" if input.starts_with(&[0x1f, 0x8b]) => "gzip",
                "unzip" => "deflate",
                format => format,
            };
            let inflate = args[1] == true;
            let mut ended = false;
            let output = Codec::new(format, inflate, args[3].as_i64(), None, None)?.write(
                input,
                4,
                &mut ended,
                args[4].as_u64().unwrap_or(1 << 30),
            )?;
            ensure!(!inflate || ended, "unexpected end of file");
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
        // Sends one query through the system resolver (libresolv) and returns the answers of the asked type as
        // [type, ...fields] in Node's field order. Errors are c-ares codes, like ENOTFOUND.
        "resolve" => {
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            {
                #[link(name = "resolv")]
                unsafe extern "C" {
                    #[cfg_attr(target_os = "macos", link_name = "res_9_ninit")]
                    #[cfg_attr(target_os = "linux", link_name = "__res_ninit")]
                    fn res_ninit(state: *mut u64) -> i32;
                    #[cfg_attr(target_os = "macos", link_name = "res_9_nsend")]
                    #[cfg_attr(target_os = "linux", link_name = "__res_nsend")]
                    fn res_nsend(
                        state: *mut u64,
                        query: *const u8,
                        size: i32,
                        answer: *mut u8,
                        capacity: i32,
                    ) -> i32;
                    #[cfg_attr(target_os = "macos", link_name = "res_9_ndestroy")]
                    #[cfg_attr(target_os = "linux", link_name = "__res_nclose")]
                    fn res_ndestroy(state: *mut u64);
                }
                let kind = args[1].as_u64().context("Expected record type")? as u16;
                let mut name = args[0]
                    .as_str()
                    .context("Expected hostname")?
                    .trim_end_matches('.')
                    .to_owned();
                if args[5] == true {
                    name = match name
                        .parse::<std::net::IpAddr>()
                        .map_err(|_| anyhow::anyhow!("EINVAL"))?
                    {
                        std::net::IpAddr::V4(ip) => {
                            ip.octets()
                                .iter()
                                .rev()
                                .map(|byte| format!("{byte}."))
                                .collect::<String>()
                                + "in-addr.arpa"
                        }
                        std::net::IpAddr::V6(ip) => {
                            ip.octets()
                                .iter()
                                .rev()
                                .map(|byte| format!("{:x}.{:x}.", byte & 15, byte >> 4))
                                .collect::<String>()
                                + "ip6.arpa"
                        }
                    };
                }
                let mut query = vec![0, 0, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0];
                getrandom::fill(&mut query[..2]).map_err(|error| anyhow::anyhow!("{error}"))?;
                for label in name.split('.') {
                    ensure!(
                        !label.is_empty() && label.len() < 64 && name.len() < 254,
                        "EBADNAME"
                    );
                    query.push(label.len() as u8);
                    query.extend_from_slice(label.as_bytes());
                }
                query.extend_from_slice(&[0, (kind >> 8) as u8, kind as u8, 0, 1]);
                // WARNING: `__res_state` has the same head in BIND, macOS and glibc: retrans, retry, options, nscount at 16 and three sockaddr_in at 20.
                let mut state = [0u64; 128];
                let mut packet = vec![0u8; 65536];
                let size = unsafe {
                    ensure!(res_ninit(state.as_mut_ptr()) == 0, "ENOTINITIALIZED");
                    let head = state.as_mut_ptr().cast::<u8>();
                    if let Some(servers) = args[2].as_array() {
                        for (index, server) in servers.iter().take(3).enumerate() {
                            let address: std::net::SocketAddrV4 =
                                server.as_str().unwrap_or("").parse()?;
                            let mut entry: libc::sockaddr_in = std::mem::zeroed();
                            #[cfg(target_os = "macos")]
                            {
                                entry.sin_len = size_of::<libc::sockaddr_in>() as u8;
                            }
                            entry.sin_family = libc::AF_INET as _;
                            entry.sin_port = address.port().to_be();
                            entry.sin_addr.s_addr = u32::from(*address.ip()).to_be();
                            head.add(20)
                                .cast::<libc::sockaddr_in>()
                                .add(index)
                                .write_unaligned(entry);
                        }
                        head.add(16)
                            .cast::<i32>()
                            .write(servers.len().min(3) as i32);
                    }
                    if let Some(seconds) = args[3].as_u64() {
                        head.cast::<i32>().write(seconds.clamp(1, 60) as i32);
                    }
                    if let Some(tries) = args[4].as_u64() {
                        head.add(4).cast::<i32>().write(tries.clamp(1, 16) as i32);
                    }
                    let size = res_nsend(
                        state.as_mut_ptr(),
                        query.as_ptr(),
                        query.len() as i32,
                        packet.as_mut_ptr(),
                        packet.len() as i32,
                    );
                    let error = std::io::Error::last_os_error();
                    res_ndestroy(state.as_mut_ptr());
                    // WARNING: libresolv fails on SERVFAIL, NOTIMP and REFUSED but leaves that reply in the buffer.
                    let replied = packet[..2] == query[..2] && packet[2] & 0x80 != 0;
                    ensure!(
                        size >= 12 || replied,
                        if error.raw_os_error() == Some(libc::ECONNREFUSED) {
                            "ECONNREFUSED"
                        } else {
                            "ETIMEOUT"
                        }
                    );
                    (size.max(12) as usize).min(packet.len())
                };
                let packet = &packet[..size];
                match packet[3] & 15 {
                    0 => {}
                    1 => bail!("EFORMERR"),
                    2 => bail!("ESERVFAIL"),
                    3 => bail!("ENOTFOUND"),
                    4 => bail!("ENOTIMP"),
                    5 => bail!("EREFUSED"),
                    _ => bail!("EBADRESP"),
                }
                let read = |at: usize, size: usize| packet.get(at..at + size).context("EBADRESP");
                let short =
                    |at: usize| -> Result<u16> { Ok(u16::from_be_bytes(read(at, 2)?.try_into()?)) };
                let long =
                    |at: usize| -> Result<u32> { Ok(u32::from_be_bytes(read(at, 4)?.try_into()?)) };
                // Reads a possibly compressed name and returns it with the offset after it.
                let label = |mut at: usize| -> Result<(String, usize)> {
                    let (mut labels, mut end) = (Vec::new(), None);
                    for _ in 0..128 {
                        let size = *packet.get(at).context("EBADRESP")? as usize;
                        if size >= 0xc0 {
                            end.get_or_insert(at + 2);
                            at = usize::from(short(at)? & 0x3fff);
                        } else if size == 0 {
                            return Ok((labels.join("."), end.unwrap_or(at + 1)));
                        } else {
                            labels.push(String::from_utf8_lossy(read(at + 1, size)?).into_owned());
                            at += 1 + size;
                        }
                    }
                    bail!("EBADRESP")
                };
                let text = |at: &mut usize| -> Result<String> {
                    let size = *packet.get(*at).context("EBADRESP")? as usize;
                    let value = String::from_utf8_lossy(read(*at + 1, size)?).into_owned();
                    *at += 1 + size;
                    Ok(value)
                };
                let mut at = 12;
                for _ in 0..short(4)? {
                    at = label(at)?.1 + 4;
                }
                let mut records = Vec::new();
                for _ in 0..short(6)? {
                    let start = label(at)?.1 + 10;
                    let (ty, ttl, data) = (
                        short(start - 10)?,
                        long(start - 6)?,
                        read(start, short(start - 2)? as usize)?,
                    );
                    at = start + data.len();
                    if kind != 255 && ty != kind {
                        continue;
                    }
                    records.push(match ty {
                        1 => json!([
                            "A",
                            std::net::Ipv4Addr::from(<[u8; 4]>::try_from(data)?).to_string(),
                            ttl
                        ]),
                        28 => json!([
                            "AAAA",
                            std::net::Ipv6Addr::from(<[u8; 16]>::try_from(data)?).to_string(),
                            ttl
                        ]),
                        2 => json!(["NS", label(start)?.0]),
                        5 => json!(["CNAME", label(start)?.0]),
                        12 => json!(["PTR", label(start)?.0]),
                        15 => json!(["MX", label(start + 2)?.0, short(start)?]),
                        16 => {
                            let (mut entries, mut next) = (Vec::new(), start);
                            while next < at {
                                entries.push(text(&mut next)?);
                            }
                            json!(["TXT", entries])
                        }
                        33 => json!([
                            "SRV",
                            label(start + 6)?.0,
                            short(start + 4)?,
                            short(start)?,
                            short(start + 2)?
                        ]),
                        6 => {
                            let (nsname, next) = label(start)?;
                            let (hostmaster, next) = label(next)?;
                            json!([
                                "SOA",
                                nsname,
                                hostmaster,
                                long(next)?,
                                long(next + 4)?,
                                long(next + 8)?,
                                long(next + 12)?,
                                long(next + 16)?
                            ])
                        }
                        35 => {
                            let mut next = start + 4;
                            let (flags, service, regexp) =
                                (text(&mut next)?, text(&mut next)?, text(&mut next)?);
                            json!([
                                "NAPTR",
                                flags,
                                service,
                                regexp,
                                label(next)?.0,
                                short(start)?,
                                short(start + 2)?
                            ])
                        }
                        52 if data.len() >= 3 => json!([
                            "TLSA",
                            data[0],
                            data[1],
                            data[2],
                            crate::web::base64(&data[3..])
                        ]),
                        257 if data.len() >= 2 => {
                            let tag = data.get(2..2 + data[1] as usize).context("EBADRESP")?;
                            json!([
                                "CAA",
                                data[0],
                                String::from_utf8_lossy(tag),
                                String::from_utf8_lossy(&data[2 + tag.len()..])
                            ])
                        }
                        _ => continue,
                    });
                }
                ensure!(!records.is_empty(), "ENODATA");
                Ok(json!(records))
            }
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            bail!("ENOTIMP")
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
        "os.priority" => {
            #[cfg(unix)]
            unsafe {
                let pid = args[0].as_i64().unwrap_or(0) as libc::id_t;
                if let Some(priority) = args[1].as_i64() {
                    if libc::setpriority(libc::PRIO_PROCESS, pid, priority as libc::c_int) != 0 {
                        return Err(std::io::Error::last_os_error().into());
                    }
                    return Ok(Value::Null);
                }
                // WARNING: -1 is a valid priority, so only errno tells a failure apart.
                #[cfg(target_os = "macos")]
                {
                    *libc::__error() = 0;
                }
                #[cfg(target_os = "linux")]
                {
                    *libc::__errno_location() = 0;
                }
                let priority = libc::getpriority(libc::PRIO_PROCESS, pid);
                let error = std::io::Error::last_os_error();
                if priority == -1 && error.raw_os_error() != Some(0) {
                    return Err(error.into());
                }
                return Ok(json!(priority));
            }
            #[allow(unreachable_code)]
            {
                bail!("Process priority is unavailable on this platform")
            }
        }
        "tty.size" => {
            #[cfg(unix)]
            unsafe {
                let mut size: libc::winsize = std::mem::zeroed();
                if libc::ioctl(
                    args[0].as_i64().unwrap_or(1) as i32,
                    libc::TIOCGWINSZ,
                    &mut size,
                ) == 0
                    && size.ws_col > 0
                {
                    return Ok(json!([size.ws_col, size.ws_row]));
                }
            }
            Ok(Value::Null)
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
