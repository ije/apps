use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use serde_json::{Value, json};
use std::cell::RefCell;

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

    pub fn snapshot(value: Value) -> Self {
        Self {
            value,
            buffers: FRAMES.with(|frames| frames.borrow().last().cloned().unwrap_or_default()),
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
                    crate::web::base64(
                        self.buffers
                            .get(index as usize)
                            .map(Bytes::as_ref)
                            .unwrap_or_default(),
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

pub(crate) fn read(value: &Value) -> Option<Result<Vec<u8>>> {
    let index = value.get("$appsBytes")?.as_u64()?;
    Some(FRAMES.with(|frames| {
        let frames = frames.borrow();
        let bytes = frames
            .last()
            .and_then(|frame| frame.get(index as usize))
            .context("Invalid binary argument")?;
        Ok(bytes.to_vec())
    }))
}

pub(crate) fn write(bytes: &[u8]) -> Option<Value> {
    FRAMES.with(|frames| {
        let mut frames = frames.borrow_mut();
        let frame = frames.last_mut()?;
        let index = frame.len();
        frame.push(Bytes::copy_from_slice(bytes));
        Some(json!({"$appsBytes":index}))
    })
}

pub(crate) fn input(
    api: &crate::jsc::Api,
    ctx: crate::jsc::Ptr,
    values: crate::jsc::Ptr,
) -> Result<Vec<Bytes>> {
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
    let mut buffers = Vec::with_capacity(count as usize);
    for index in 0..count as usize {
        let key = api.string(&index.to_string());
        let value = unsafe { (api.get)(ctx, values, key, &mut error) };
        unsafe {
            (api.string_release)(key);
        }
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
        buffers.push(if length == 0 {
            Bytes::new()
        } else {
            Bytes::copy_from_slice(unsafe { std::slice::from_raw_parts(pointer, length) })
        });
    }
    Ok(buffers)
}

unsafe extern "C" fn release(_pointer: crate::jsc::Ptr, owner: crate::jsc::Ptr) {
    drop(unsafe { Box::from_raw(owner.cast::<Vec<u8>>()) });
}

pub(crate) fn output(
    api: &crate::jsc::Api,
    ctx: crate::jsc::Ptr,
    mut payload: Payload,
) -> crate::jsc::Ptr {
    let mut used = std::collections::HashMap::new();
    let mut buffers = Vec::new();
    let mut pending = vec![&mut payload.value];
    while let Some(value) = pending.pop() {
        if let Some(index) = value.get("$appsBytes").and_then(Value::as_u64) {
            if let Some(bytes) = payload.buffers.get(index as usize) {
                let next = *used.entry(index).or_insert_with(|| {
                    let next = buffers.len();
                    buffers.push(bytes.to_vec());
                    next
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
    let source = api.string(&payload.value.to_string());
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
