use super::{Callback, HOSTS, JavaScriptCore, Ptr};
use anyhow::{Context, Result, bail, ensure};
use std::ptr;

// WARNING: characters and bytes borrow JavaScript memory. Copy them before any JavaScript runs: a string used as a
// property key can swap its storage for an atom, and a buffer can be detached or resized.
pub(crate) enum Chars<'a> {
    Latin1(&'a [u8]),
    Utf16(&'a [u16]),
}

impl JavaScriptCore {
    pub(crate) fn chars<'a>(&self, ctx: Ptr, value: Ptr) -> Option<Chars<'a>> {
        let (mut length, mut latin1) = (0, false);
        let pointer = unsafe { (self.get_chars)(ctx, value, &mut length, &mut latin1) };
        if pointer.is_null() {
            return None;
        }
        Some(unsafe {
            if latin1 {
                Chars::Latin1(std::slice::from_raw_parts(pointer.cast(), length))
            } else {
                Chars::Utf16(std::slice::from_raw_parts(pointer.cast(), length))
            }
        })
    }

    pub(crate) fn bytes<'a>(&self, ctx: Ptr, value: Ptr) -> Option<&'a mut [u8]> {
        let (mut pointer, mut length) = (ptr::null_mut(), 0);
        if !unsafe { (self.get_bytes)(ctx, value, &mut pointer, &mut length) } {
            return None;
        }
        Some(if length == 0 || pointer.is_null() {
            &mut []
        } else {
            unsafe { std::slice::from_raw_parts_mut(pointer.cast(), length) }
        })
    }

    pub(crate) fn uint8_array<'a>(&self, ctx: Ptr, length: usize) -> Result<(Ptr, &'a mut [u8])> {
        let mut bytes = ptr::null_mut();
        let array = unsafe { (self.make_uint8)(ctx, length, &mut bytes, ptr::null_mut()) };
        ensure!(!array.is_null(), "Array buffer allocation failed");
        Ok((
            array,
            if length == 0 || bytes.is_null() {
                &mut []
            } else {
                unsafe { std::slice::from_raw_parts_mut(bytes.cast(), length) }
            },
        ))
    }

    pub(crate) fn utf8(&self, ctx: Ptr, text: &[u8]) -> Result<Ptr> {
        let value = unsafe { (self.make_utf8)(ctx, text.as_ptr().cast(), text.len()) };
        ensure!(!value.is_null(), "Invalid string length");
        Ok(value)
    }

    fn latin1(&self, ctx: Ptr, text: &[u8]) -> Result<Ptr> {
        let value = unsafe { (self.make_latin1)(ctx, text.as_ptr(), text.len()) };
        ensure!(!value.is_null(), "Invalid string length");
        Ok(value)
    }

    fn utf16(&self, ctx: Ptr, text: &[u16]) -> Result<Ptr> {
        let value = unsafe { (self.make_utf16)(ctx, text.as_ptr(), text.len()) };
        ensure!(!value.is_null(), "Invalid string length");
        Ok(value)
    }

    pub(crate) fn utf8_string(&self, chars: Chars) -> String {
        let mut text = vec![0; self.simd_utf8(&chars, ptr::null_mut())];
        if !self.write_utf8(&chars, &mut text)
            && let Chars::Utf16(units) = chars
        {
            text = vec![0; utf8_len(units)];
            encode_utf16(units, &mut text);
        }
        unsafe { String::from_utf8_unchecked(text) }
    }

    // WARNING: SIMD counts a lone surrogate as 2 bytes and can't convert it, so `write_utf8` then returns false and
    // the caller encodes again into `utf8_len` bytes with `encode_utf16`, which writes U+FFFD. Without `out` this
    // returns the size.
    fn simd_utf8(&self, chars: &Chars, out: *mut u8) -> usize {
        let (pointer, length, latin1) = match *chars {
            Chars::Latin1(text) => (text.as_ptr().cast(), text.len(), true),
            Chars::Utf16(text) => (text.as_ptr().cast(), text.len(), false),
        };
        unsafe { (self.to_utf8)(pointer, length, latin1, out.cast()) }
    }

    fn write_utf8(&self, chars: &Chars, out: &mut [u8]) -> bool {
        match *chars {
            Chars::Latin1(text) if text.len() == out.len() => out.copy_from_slice(text),
            Chars::Utf16([]) => {}
            _ => return self.simd_utf8(chars, out.as_mut_ptr()) == out.len(),
        }
        true
    }
}

// WARNING: a lone surrogate counts 3 bytes, the size of the U+FFFD that replaces it. Sums in u16 over chunks of 4096
// units vectorize and can't overflow.
fn utf8_len(units: &[u16]) -> usize {
    let wide: usize = units
        .chunks(4096)
        .map(|chunk| {
            chunk
                .iter()
                .map(|&unit| (unit >= 0x80) as u16 + (unit >= 0x800) as u16)
                .sum::<u16>() as usize
        })
        .sum();
    let pairs: usize = units
        .chunks(4096)
        .zip(units.get(1..).unwrap_or_default().chunks(4096))
        .map(|(high, low)| {
            high.iter()
                .zip(low)
                .map(|(&high, &low)| (high & 0xFC00 == 0xD800 && low & 0xFC00 == 0xDC00) as u16)
                .sum::<u16>() as usize
        })
        .sum();
    units.len() + wide - 2 * pairs
}

fn encode_latin1(bytes: &[u8], out: &mut [u8]) -> (usize, usize) {
    let (mut read, mut written) = (0, 0);
    while read < bytes.len() {
        let room = (out.len() - written).min(bytes.len() - read);
        let ascii = bytes[read..read + room]
            .iter()
            .position(|&byte| byte >= 0x80)
            .unwrap_or(room);
        out[written..written + ascii].copy_from_slice(&bytes[read..read + ascii]);
        read += ascii;
        written += ascii;
        let Some(slot) = bytes
            .get(read)
            .filter(|&&byte| byte >= 0x80)
            .and(out.get_mut(written..written + 2))
        else {
            break;
        };
        (bytes[read] as char).encode_utf8(slot);
        read += 1;
        written += 2;
    }
    (read, written)
}

fn encode_utf16(units: &[u16], out: &mut [u8]) -> (usize, usize) {
    let (mut read, mut written) = (0, 0);
    while let Some(&unit) = units.get(read) {
        if unit < 0x80 {
            let Some(slot) = out.get_mut(written) else {
                break;
            };
            *slot = unit as u8;
            read += 1;
            written += 1;
            continue;
        }
        let (code, width) = match units.get(read + 1) {
            Some(&next) if unit & 0xFC00 == 0xD800 && next & 0xFC00 == 0xDC00 => (
                0x10000 + ((unit as u32 & 0x3FF) << 10) + (next as u32 & 0x3FF),
                2,
            ),
            _ => (unit as u32, 1),
        };
        let code = char::from_u32(code).unwrap_or('\u{FFFD}');
        let Some(slot) = out.get_mut(written..written + code.len_utf8()) else {
            break;
        };
        code.encode_utf8(slot);
        read += width;
        written += code.len_utf8();
    }
    (read, written)
}

// The bytes of a UTF-8 sequence that the end of `bytes` cuts short, which a stream keeps for its next chunk.
fn incomplete(bytes: &[u8]) -> usize {
    for back in 1..=bytes.len().min(3) {
        let start = bytes.len() - back;
        if bytes[start] & 0xC0 != 0x80 {
            return match std::str::from_utf8(&bytes[start..]) {
                Err(error) if error.valid_up_to() == 0 && error.error_len().is_none() => back,
                _ => 0,
            };
        }
    }
    0
}

const CP1252: [u16; 32] = [
    0x20AC, 0x81, 0x201A, 0x192, 0x201E, 0x2026, 0x2020, 0x2021, 0x2C6, 0x2030, 0x160, 0x2039,
    0x152, 0x8D, 0x17D, 0x8F, 0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x2DC,
    0x2122, 0x161, 0x203A, 0x153, 0x9D, 0x17E, 0x178,
];

const INVALID: &str = "The encoded data was not valid";

fn native(
    ctx: Ptr,
    count: usize,
    args: *const Ptr,
    exception: *mut Ptr,
    action: impl FnOnce(&JavaScriptCore, &dyn Fn(usize) -> Ptr) -> Result<Ptr>,
) -> Ptr {
    let Some(api) = HOSTS.with(|hosts| {
        hosts
            .borrow()
            .get(&(ctx as usize))
            .map(|binding| binding.0.clone())
    }) else {
        return ptr::null_mut();
    };
    let args = if count == 0 || args.is_null() {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(args, count) }
    };
    let arg = |index: usize| {
        args.get(index)
            .copied()
            .unwrap_or_else(|| unsafe { (api.undefined)(ctx) })
    };
    match action(&api, &arg) {
        Ok(value) => value,
        Err(error) => unsafe {
            let message = api
                .utf8(ctx, error.to_string().as_bytes())
                .unwrap_or(ptr::null_mut());
            let error = (api.make_error)(ctx, 1, &message, ptr::null_mut());
            if !exception.is_null() {
                *exception = error;
            }
            (api.undefined)(ctx)
        },
    }
}

fn number(api: &JavaScriptCore, ctx: Ptr, value: Ptr) -> f64 {
    unsafe { (api.to_number)(ctx, value, ptr::null_mut()) }
}

// encode(string, kind): 0 UTF-8, 1 Latin-1 (low byte of each unit), 2 UTF-16LE. Returns a Uint8Array.
unsafe extern "C" fn encode(
    ctx: Ptr,
    _: Ptr,
    _: Ptr,
    count: usize,
    args: *const Ptr,
    exception: *mut Ptr,
) -> Ptr {
    native(ctx, count, args, exception, |api, arg| {
        let chars = api.chars(ctx, arg(0)).context("Expected a string")?;
        let kind = number(api, ctx, arg(1));
        if kind == 0. {
            let (mut array, out) = api.uint8_array(ctx, api.simd_utf8(&chars, ptr::null_mut()))?;
            if !api.write_utf8(&chars, out)
                && let Chars::Utf16(units) = chars
            {
                let out;
                (array, out) = api.uint8_array(ctx, utf8_len(units))?;
                encode_utf16(units, out);
            }
            return Ok(array);
        }
        let length = match &chars {
            Chars::Latin1(text) => text.len() << (kind == 2.) as u32,
            Chars::Utf16(text) => text.len() << (kind == 2.) as u32,
        };
        let (array, out) = api.uint8_array(ctx, length)?;
        match chars {
            Chars::Latin1(text) if text.len() == length => out.copy_from_slice(text),
            Chars::Latin1(text) => {
                for (pair, &byte) in out.as_chunks_mut::<2>().0.iter_mut().zip(text) {
                    *pair = [byte, 0];
                }
            }
            Chars::Utf16(text) if kind == 1. => {
                for (slot, &unit) in out.iter_mut().zip(text) {
                    *slot = unit as u8;
                }
            }
            Chars::Utf16(text) => {
                for (pair, &unit) in out.as_chunks_mut::<2>().0.iter_mut().zip(text) {
                    *pair = unit.to_le_bytes();
                }
            }
        }
        Ok(array)
    })
}

// encodeInto(string, bytes): writes the UTF-8 characters that fit. Returns [read units, written bytes].
unsafe extern "C" fn encode_into(
    ctx: Ptr,
    _: Ptr,
    _: Ptr,
    count: usize,
    args: *const Ptr,
    exception: *mut Ptr,
) -> Ptr {
    native(ctx, count, args, exception, |api, arg| {
        let chars = api.chars(ctx, arg(0)).context("Expected a string")?;
        let out = api.bytes(ctx, arg(1)).context("Expected a Uint8Array")?;
        let (read, written) = match chars {
            Chars::Latin1(text) => encode_latin1(text, out),
            Chars::Utf16(text) => encode_utf16(text, out),
        };
        let items = unsafe {
            [
                (api.number)(ctx, read as f64),
                (api.number)(ctx, written as f64),
            ]
        };
        Ok(unsafe { (api.make_array)(ctx, 2, items.as_ptr(), ptr::null_mut()) })
    })
}

// decode(bytes, kind, fatal, stream): 0 UTF-8, 1 Latin-1, 2 ASCII, 3 UTF-16LE as it is, then for TextDecoder
// 4 UTF-16LE, 5 UTF-16BE and 6 windows-1252. A stream returns [string, bytes kept for the next chunk].
unsafe extern "C" fn decode(
    ctx: Ptr,
    _: Ptr,
    _: Ptr,
    count: usize,
    args: *const Ptr,
    exception: *mut Ptr,
) -> Ptr {
    native(ctx, count, args, exception, |api, arg| {
        let bytes = &*api
            .bytes(ctx, arg(0))
            .context("Expected an ArrayBuffer or a view")?;
        let kind = number(api, ctx, arg(1)) as u32;
        let (fatal, stream) =
            unsafe { ((api.to_boolean)(ctx, arg(2)), (api.to_boolean)(ctx, arg(3))) };
        let mut pending = 0;
        let text = match kind {
            0 => {
                pending = if stream { incomplete(bytes) } else { 0 };
                let bytes = &bytes[..bytes.len() - pending];
                ensure!(!fatal || std::str::from_utf8(bytes).is_ok(), INVALID);
                api.utf8(ctx, bytes)?
            }
            1 => api.latin1(ctx, bytes)?,
            2 if bytes.is_ascii() => api.latin1(ctx, bytes)?,
            2 => api.latin1(
                ctx,
                &bytes.iter().map(|byte| byte & 0x7F).collect::<Vec<_>>(),
            )?,
            3..=5 => {
                let end = bytes.len() & !1;
                // WARNING: all supported targets are little-endian, so aligned UTF-16LE bytes are already units.
                let (head, units, _) = unsafe { bytes[..end].align_to::<u16>() };
                let lone = |units: &[u16]| {
                    units.iter().enumerate().position(|(index, &unit)| {
                        unit & 0xF800 == 0xD800
                            && if unit < 0xDC00 {
                                units
                                    .get(index + 1)
                                    .is_none_or(|&next| next & 0xFC00 != 0xDC00)
                            } else {
                                index == 0 || units[index - 1] & 0xFC00 != 0xD800
                            }
                    })
                };
                let mut units =
                    if kind != 5 && head.is_empty() && (kind == 3 || lone(units).is_none()) {
                        std::borrow::Cow::Borrowed(units)
                    } else {
                        bytes[..end]
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .map(|&pair| match kind {
                                5 => u16::from_be_bytes(pair),
                                _ => u16::from_le_bytes(pair),
                            })
                            .collect()
                    };
                pending = bytes.len() - end;
                if kind > 3 {
                    if stream && units.last().is_some_and(|&unit| unit & 0xFC00 == 0xD800) {
                        pending += 2;
                        units.to_mut().pop();
                    }
                    // WARNING: the scan goes on after each fix, so many lone surrogates stay linear. The unit after
                    // a lone one never pairs with it, so starting the slice there is safe.
                    let mut from = 0;
                    while let Some(index) = lone(&units[from..]) {
                        ensure!(!fatal, INVALID);
                        units.to_mut()[from + index] = 0xFFFD;
                        from += index + 1;
                    }
                    if !stream && pending > 0 {
                        ensure!(!fatal, INVALID);
                        units.to_mut().push(0xFFFD);
                        pending = 0;
                    }
                }
                api.utf16(ctx, &units)?
            }
            6 if !bytes.iter().any(|byte| (0x80..0xA0).contains(byte)) => api.latin1(ctx, bytes)?,
            6 => api.utf16(
                ctx,
                &bytes
                    .iter()
                    .map(|&byte| match byte {
                        0x80..0xA0 => CP1252[byte as usize - 0x80],
                        _ => byte as u16,
                    })
                    .collect::<Vec<_>>(),
            )?,
            _ => bail!("Unknown encoding"),
        };
        if !stream {
            return Ok(text);
        }
        let items = [text, unsafe { (api.number)(ctx, pending as f64) }];
        Ok(unsafe { (api.make_array)(ctx, 2, items.as_ptr(), ptr::null_mut()) })
    })
}

// utf8Length(string): the size of the string in UTF-8.
unsafe extern "C" fn utf8_length(
    ctx: Ptr,
    _: Ptr,
    _: Ptr,
    count: usize,
    args: *const Ptr,
    exception: *mut Ptr,
) -> Ptr {
    native(ctx, count, args, exception, |api, arg| {
        let size = match api.chars(ctx, arg(0)).context("Expected a string")? {
            Chars::Utf16(units) => utf8_len(units),
            chars => api.simd_utf8(&chars, ptr::null_mut()),
        };
        Ok(unsafe { (api.number)(ctx, size as f64) })
    })
}

// indexOf(haystack, needle, from, last): the first match at or after `from`, or the last one that starts at or
// before it. Returns -1 without one.
unsafe extern "C" fn index_of(
    ctx: Ptr,
    _: Ptr,
    _: Ptr,
    count: usize,
    args: *const Ptr,
    exception: *mut Ptr,
) -> Ptr {
    native(ctx, count, args, exception, |api, arg| {
        let haystack = &*api.bytes(ctx, arg(0)).context("Expected bytes")?;
        let needle = &*api.bytes(ctx, arg(1)).context("Expected bytes")?;
        let from = (number(api, ctx, arg(2)).max(0.) as usize).min(haystack.len());
        let found = if unsafe { (api.to_boolean)(ctx, arg(3)) } {
            memchr::memmem::rfind(
                &haystack[..(from + needle.len()).min(haystack.len())],
                needle,
            )
        } else {
            memchr::memmem::find(&haystack[from..], needle).map(|index| index + from)
        };
        Ok(unsafe { (api.number)(ctx, found.map_or(-1., |index| index as f64)) })
    })
}

// compare(a, b): -1, 0 or 1, comparing the bytes as unsigned numbers.
unsafe extern "C" fn compare(
    ctx: Ptr,
    _: Ptr,
    _: Ptr,
    count: usize,
    args: *const Ptr,
    exception: *mut Ptr,
) -> Ptr {
    native(ctx, count, args, exception, |api, arg| {
        let a = &*api.bytes(ctx, arg(0)).context("Expected bytes")?;
        let b = &*api.bytes(ctx, arg(1)).context("Expected bytes")?;
        Ok(unsafe { (api.number)(ctx, a.cmp(b) as i8 as f64) })
    })
}

pub(crate) fn install(api: &JavaScriptCore, ctx: Ptr) {
    unsafe {
        let object = (api.object_make)(ctx, ptr::null_mut(), ptr::null_mut());
        for (name, callback) in [
            ("encode", encode as Callback),
            ("encodeInto", encode_into),
            ("decode", decode),
            ("utf8Length", utf8_length),
            ("indexOf", index_of),
            ("compare", compare),
        ] {
            let name = api.string(name);
            (api.set)(
                ctx,
                object,
                name,
                (api.function)(ctx, name, callback),
                0,
                ptr::null_mut(),
            );
            (api.string_release)(name);
        }
        let name = api.string("__appsEncoding");
        (api.set)(ctx, (api.global)(ctx), name, object, 0, ptr::null_mut());
        (api.string_release)(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_like_the_standard() {
        for text in [
            "",
            "abc",
            "café",
            "日本語",
            "😀x",
            "a\u{80}\u{7FF}\u{800}\u{FFFF}",
        ] {
            let units: Vec<u16> = text.encode_utf16().collect();
            let mut out = vec![0; utf8_len(&units)];
            assert_eq!(encode_utf16(&units, &mut out), (units.len(), text.len()));
            assert_eq!(out, text.as_bytes());
        }
        let latin1: Vec<u8> = "café ÿ".chars().map(|c| c as u8).collect();
        let mut out = vec![0; 8];
        encode_latin1(&latin1, &mut out);
        assert_eq!(out, "café ÿ".as_bytes());
        for units in [
            &[0xD800][..],
            &[0xDC00, 0x61],
            &[0x61, 0xD800, 0xD800, 0xDC00],
            &[0xDC00, 0xD800],
        ] {
            let expected = String::from_utf16_lossy(units);
            let mut out = vec![0; utf8_len(units)];
            encode_utf16(units, &mut out);
            assert_eq!(out, expected.as_bytes());
        }
        let mut out = [0; 4];
        assert_eq!(
            encode_utf16(&"a😀".encode_utf16().collect::<Vec<_>>(), &mut out),
            (1, 1)
        );
        assert_eq!(encode_latin1(&[0x61, 0xE9, 0x62], &mut out[..2]), (1, 1));
        assert_eq!(encode_latin1(&[0x61, 0xE9, 0x62], &mut out), (3, 4));
        assert_eq!(&out, b"a\xC3\xA9b");
    }

    #[test]
    fn keeps_cut_sequences_for_streams() {
        for (bytes, kept) in [
            (&b"ab"[..], 0),
            (b"a\xE2\x82", 2),
            (b"\xF0\x9F\x98", 3),
            (b"\xF0\x9F\x98\x80", 0),
            (b"a\xE0\x80", 0),
            (b"\xC3", 1),
            (b"\xFF", 0),
        ] {
            assert_eq!(incomplete(bytes), kept, "{bytes:?}");
        }
    }
}
