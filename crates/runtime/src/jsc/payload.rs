use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use bytes::Bytes;
use serde_json::{Value, json};
use std::cell::RefCell;

// WARNING: binary arguments of a call can borrow JavaScript memory, so bytes leave a frame only through `read` and `snapshot`, which copy borrowed bytes. A `slice` must not outlive its call or a `read` of the same argument.
thread_local! { static FRAMES: RefCell<Vec<Vec<Bytes>>> = const { RefCell::new(Vec::new()) }; }

#[derive(Clone, Default)]
pub struct Payload {
    pub value: Value,
    pub buffers: Vec<Bytes>,
}

impl From<Value> for Payload {
    fn from(value: Value) -> Self {
        Self {
            value,
            buffers: Vec::new(),
        }
    }
}

impl Payload {
    pub fn capture(buffers: Vec<Bytes>, action: impl FnOnce() -> Value) -> Self {
        struct Frame;
        impl Drop for Frame {
            fn drop(&mut self) {
                FRAMES.with(|frames| {
                    frames.borrow_mut().pop();
                });
            }
        }
        FRAMES.with(|frames| frames.borrow_mut().push(buffers));
        let frame = Frame;
        let value = action();
        let buffers = FRAMES
            .with(|frames| std::mem::take(frames.borrow_mut().last_mut().expect("Binary frame")));
        drop(frame);
        Self { value, buffers }
    }

    /// Takes the binary arguments of the current call, so `value` can outlive it.
    pub fn snapshot(value: Value) -> Self {
        Self {
            value,
            buffers: FRAMES.with(|frames| {
                frames
                    .borrow_mut()
                    .last_mut()
                    .map(|frame| frame.iter_mut().map(own).collect())
                    .unwrap_or_default()
            }),
        }
    }

    pub fn with_bytes(mut value: Value, key: &str, bytes: impl Into<Bytes>) -> Self {
        value[key] = json!({"$appsBytes":0});
        Self {
            value,
            buffers: vec![bytes.into()],
        }
    }

    pub fn into_value(mut self) -> Value {
        let offset = FRAMES.with(|frames| {
            let mut frames = frames.borrow_mut();
            frames.last_mut().map(|frame| {
                let offset = frame.len();
                frame.append(&mut self.buffers);
                offset
            })
        });
        let mut pending = vec![&mut self.value];
        while let Some(value) = pending.pop() {
            if let Some(index) = value.get("$appsBytes").and_then(Value::as_u64) {
                *value = if let Some(offset) = offset {
                    json!({"$appsBytes": index + offset as u64})
                } else {
                    Value::String(
                        STANDARD.encode(
                            self.buffers
                                .get(index as usize)
                                .map(Bytes::as_ref)
                                .unwrap_or_default(),
                        ),
                    )
                };
            } else {
                match value {
                    Value::Array(values) => pending.extend(values),
                    Value::Object(values) => pending.extend(values.values_mut()),
                    _ => {}
                }
            }
        }
        self.value
    }
}

/// Moves out bytes that the frame owns alone, and copies borrowed or shared ones.
fn own(bytes: &mut Bytes) -> Bytes {
    if bytes.is_unique() {
        std::mem::take(bytes)
    } else {
        Bytes::copy_from_slice(bytes)
    }
}

/// Takes binary argument `value` of the current call, for an op that keeps it.
pub(crate) fn read(value: &Value) -> Option<Result<Bytes>> {
    let index = value.get("$appsBytes")?.as_u64()?;
    Some(FRAMES.with(|frames| {
        Ok(own(frames
            .borrow_mut()
            .last_mut()
            .and_then(|frame| frame.get_mut(index as usize))
            .context("Invalid binary argument")?))
    }))
}

/// Borrows binary argument `value` of the current call without a copy.
pub(crate) fn slice(value: &Value) -> Option<Result<&[u8]>> {
    let index = value.get("$appsBytes")?.as_u64()?;
    Some(
        get(index as usize)
            .map(|(pointer, length)| unsafe { std::slice::from_raw_parts(pointer, length) })
            .context("Invalid binary argument"),
    )
}

/// The bytes of binary argument `index` of the current call. They stay valid until the call returns.
pub(crate) fn get(index: usize) -> Option<(*const u8, usize)> {
    FRAMES.with(|frames| {
        let frames = frames.borrow();
        let bytes = frames.last()?.get(index)?;
        Some((bytes.as_ptr(), bytes.len()))
    })
}

/// Returns `bytes` as a binary result of the current call, or as base64 outside a call.
pub(crate) fn write(bytes: impl Into<Bytes>) -> Value {
    let bytes = bytes.into();
    FRAMES.with(|frames| match frames.borrow_mut().last_mut() {
        Some(frame) => {
            frame.push(bytes);
            json!({"$appsBytes": frame.len() - 1})
        }
        None => Value::String(STANDARD.encode(bytes)),
    })
}

/// The bytes of an `ArrayBuffer` or typed array. JavaScriptCore pins them, so they stay valid while the value lives.
pub(crate) fn view<'a>(
    api: &crate::jsc::Api,
    ctx: crate::jsc::Ptr,
    value: crate::jsc::Ptr,
) -> Result<&'a [u8]> {
    let mut error = std::ptr::null_mut();
    let kind = unsafe { (api.typed_kind)(ctx, value, &mut error) };
    ensure!(error.is_null() && kind != 10, "Expected typed bytes");
    let (pointer, length) = unsafe {
        if kind == 9 {
            (
                (api.buffer_bytes)(ctx, value, &mut error).cast::<u8>(),
                (api.buffer_length)(ctx, value, &mut error),
            )
        } else {
            let length = (api.typed_length)(ctx, value, &mut error);
            let offset = (api.typed_offset)(ctx, value, &mut error);
            (
                (api.typed_bytes)(ctx, value, &mut error)
                    .cast::<u8>()
                    .wrapping_add(offset),
                length,
            )
        }
    };
    ensure!(
        error.is_null() && (length == 0 || !pointer.is_null()),
        "Invalid binary storage"
    );
    ensure!(length <= 1 << 31, "Binary arguments are too large");
    Ok(if length == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(pointer, length) }
    })
}

/// Keeps the values that `input` borrowed bytes from alive until it drops.
pub(crate) struct Roots<'a> {
    api: &'a crate::jsc::Api,
    ctx: crate::jsc::Ptr,
    values: Vec<crate::jsc::Ptr>,
}

impl Drop for Roots<'_> {
    fn drop(&mut self) {
        for &value in &self.values {
            unsafe { (self.api.unprotect)(self.ctx, value) };
        }
    }
}

/// The bytes of each item of array `values`. They stay valid while the returned `Roots` live.
pub(crate) fn input<'a>(
    api: &'a crate::jsc::Api,
    ctx: crate::jsc::Ptr,
    values: crate::jsc::Ptr,
) -> Result<(Vec<Bytes>, Roots<'a>)> {
    ensure!(
        unsafe { (api.value_type)(ctx, values) } == 5,
        "Invalid binary arguments"
    );
    let mut error = std::ptr::null_mut();
    let key = api.string("length");
    let count = unsafe { (api.get)(ctx, values, key, &mut error) };
    unsafe {
        (api.string_release)(key);
    }
    let count = unsafe { (api.to_number)(ctx, count, &mut error) };
    ensure!(
        error.is_null()
            && count.is_finite()
            && (0. ..=65536.).contains(&count)
            && count.fract() == 0.,
        "Invalid binary arguments"
    );
    // WARNING: reading an item can run a getter that drops and collects earlier items, and the call can run JavaScript too, so all items are read and protected before any bytes are borrowed.
    let mut roots = Roots {
        api,
        ctx,
        values: Vec::with_capacity(count as usize),
    };
    for index in 0..count as u32 {
        let value = unsafe { (api.get_index)(ctx, values, index, &mut error) };
        ensure!(error.is_null(), "Expected typed bytes");
        unsafe { (api.protect)(ctx, value) };
        roots.values.push(value);
    }
    let mut buffers = Vec::with_capacity(count as usize);
    let mut total = 0usize;
    for &value in &roots.values {
        let bytes = view(api, ctx, value)?;
        total += bytes.len();
        ensure!(total <= 1 << 31, "Binary arguments are too large");
        // WARNING: these borrow the arguments of the running call, so callers that keep them must copy them first.
        buffers.push(Bytes::from_static(bytes));
    }
    Ok((buffers, roots))
}

pub(crate) unsafe extern "C" fn release(_pointer: crate::jsc::Ptr, owner: crate::jsc::Ptr) {
    drop(unsafe { Box::from_raw(owner.cast::<Vec<u8>>()) });
}

pub(crate) fn output(
    api: &crate::jsc::Api,
    ctx: crate::jsc::Ptr,
    mut payload: Payload,
) -> crate::jsc::Ptr {
    let mut buffers = Vec::new();
    if !payload.buffers.is_empty() {
        let mut used = std::collections::HashMap::new();
        let mut pending = vec![&mut payload.value];
        while let Some(value) = pending.pop() {
            if let Some(index) = value.get("$appsBytes").and_then(Value::as_u64) {
                if let Some(bytes) = payload.buffers.get_mut(index as usize) {
                    let next = *used.entry(index).or_insert_with(|| {
                        buffers.push(Vec::from(std::mem::take(bytes)));
                        buffers.len() - 1
                    });
                    *value = json!({"$appsBytes":next});
                }
            } else {
                match value {
                    Value::Array(values) => pending.extend(values),
                    Value::Object(values) => pending.extend(values.values_mut()),
                    _ => {}
                }
            }
        }
    }
    let source = api.string_owned(payload.value.to_string());
    let text = unsafe { (api.make_string)(ctx, source) };
    unsafe {
        (api.string_release)(source);
    }
    if buffers.is_empty() {
        return text;
    }
    let object = unsafe { (api.object_make)(ctx, std::ptr::null_mut(), std::ptr::null_mut()) };
    unsafe {
        (api.protect)(ctx, object);
    }
    let key = api.string("json");
    unsafe {
        (api.set)(ctx, object, key, text, 0, std::ptr::null_mut());
        (api.string_release)(key);
    }
    for (index, bytes) in buffers.into_iter().enumerate() {
        let bytes = Box::new(bytes);
        let buffer = unsafe {
            (api.make_buffer)(
                ctx,
                bytes.as_ptr().cast_mut().cast(),
                bytes.len(),
                Some(release),
                Box::into_raw(bytes).cast(),
                std::ptr::null_mut(),
            )
        };
        let key = api.string(&index.to_string());
        unsafe {
            (api.set)(ctx, object, key, buffer, 0, std::ptr::null_mut());
            (api.string_release)(key);
        }
    }
    unsafe {
        (api.unprotect)(ctx, object);
    }
    object
}
