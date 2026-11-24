use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use bytes::Bytes;
use serde_json::{Value, json};
use std::cell::RefCell;

// Only read and snapshot can export borrowed bytes by copying them. A slice expires on call return or another
// read of that argument.
thread_local! { static FRAMES: RefCell<Vec<Vec<Bytes>>> = const { RefCell::new(Vec::new()) }; }

// Memory that `take` moved out of a buffer or shared from it, until the next operation sends it away.
thread_local! { static HELD: RefCell<std::collections::HashMap<u64, Bytes>> = RefCell::default(); }
// What is on its way to another thread: the address of its bytes, its contents record and its kind. Other bytes
// can't go to JavaScript without a copy, because they can have more readers.
static TAKEN: std::sync::Mutex<Vec<(usize, usize, i32)>> = std::sync::Mutex::new(Vec::new());
static TAKEN_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

// The functions of scripts/jsc.patch that give an object of one VM to another VM.
#[derive(Clone, Copy)]
pub(crate) struct Contents {
    pub(crate) take: unsafe extern "C" fn(crate::jsc::Ptr, crate::jsc::Ptr) -> crate::jsc::Ptr,
    pub(crate) bytes:
        unsafe extern "C" fn(crate::jsc::Ptr, *mut usize, *mut i32) -> crate::jsc::Ptr,
    pub(crate) make: unsafe extern "C" fn(crate::jsc::Ptr, crate::jsc::Ptr) -> crate::jsc::Ptr,
    pub(crate) release: unsafe extern "C" fn(crate::jsc::Ptr),
}

// The memory of an ArrayBuffer has one owner. The others are made again in each VM that gets them.
pub(crate) const MOVED: i32 = 0;
pub(crate) const SHARED_BUFFER: i32 = 1;

struct Taken {
    contents: usize,
    data: usize,
    size: usize,
    release: unsafe extern "C" fn(crate::jsc::Ptr),
}

impl AsRef<[u8]> for Taken {
    fn as_ref(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.data as *const u8, self.size) }
    }
}

impl Drop for Taken {
    fn drop(&mut self) {
        let mut taken = TAKEN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(index) = taken.iter().position(|&(data, ..)| data == self.data) {
            taken.swap_remove(index);
            TAKEN_COUNT.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        }
        drop(taken);
        unsafe { (self.release)(self.contents as crate::jsc::Ptr) };
    }
}

// Moves the memory of an ArrayBuffer out of its VM, or shares a SharedArrayBuffer, a shared WebAssembly.Memory
// or a WebAssembly.Module. Returns the ID that the next operation passes to `held`.
pub(crate) fn take(
    api: &crate::jsc::JavaScriptCore,
    ctx: crate::jsc::Ptr,
    buffer: crate::jsc::Ptr,
) -> Option<u64> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let functions = api.contents?;
    let contents = unsafe { (functions.take)(ctx, buffer) };
    if contents.is_null() {
        return None;
    }
    let (mut size, mut kind) = (0, MOVED);
    let data = unsafe { (functions.bytes)(contents, &mut size, &mut kind) };
    // Only moved memory has bytes of its own on the way. The one byte of the others is the first byte of their
    // contents record, which no other bytes can have as their address.
    let (data, size) = if kind == MOVED {
        (data as usize, size)
    } else {
        (contents as usize, 1)
    };
    TAKEN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((data, contents as usize, kind));
    TAKEN_COUNT.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let taken = Taken {
        contents: contents as usize,
        data,
        size,
        release: functions.release,
    };
    HELD.with_borrow_mut(|held| held.insert(id, Bytes::from_owner(taken)));
    Some(id)
}

pub(crate) fn held(id: u64) -> Option<Bytes> {
    HELD.with_borrow_mut(|held| held.remove(&id))
}

// Tells if these bytes come from `take`, and gives their contents record and kind.
pub(crate) fn taken(bytes: &Bytes) -> Option<(crate::jsc::Ptr, i32)> {
    if TAKEN_COUNT.load(std::sync::atomic::Ordering::Acquire) == 0 {
        return None;
    }
    let address = bytes.as_ptr() as usize;
    TAKEN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|&&(data, ..)| data == address)
        .map(|&(_, contents, kind)| (contents as crate::jsc::Ptr, kind))
}

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

fn own(bytes: &mut Bytes) -> Bytes {
    if bytes.is_unique() {
        std::mem::take(bytes)
    } else {
        Bytes::copy_from_slice(bytes)
    }
}

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

pub(crate) fn slice(value: &Value) -> Option<Result<&[u8]>> {
    let index = value.get("$appsBytes")?.as_u64()?;
    Some(
        get(index as usize)
            .map(|(pointer, length)| unsafe { std::slice::from_raw_parts(pointer, length) })
            .context("Invalid binary argument"),
    )
}

// The pointer stays valid only until the current call returns.
pub(crate) fn get(index: usize) -> Option<(*const u8, usize)> {
    FRAMES.with(|frames| {
        let frames = frames.borrow();
        let bytes = frames.last()?.get(index)?;
        Some((bytes.as_ptr(), bytes.len()))
    })
}

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

// The slice has an unbounded lifetime, but stays valid only while the value lives and no JavaScript runs.
pub(crate) fn view<'a>(
    api: &crate::jsc::JavaScriptCore,
    ctx: crate::jsc::Ptr,
    value: crate::jsc::Ptr,
) -> Result<&'a [u8]> {
    let bytes = api.bytes(ctx, value).context("Expected typed bytes")?;
    ensure!(bytes.len() <= 1 << 31, "Binary arguments are too large");
    Ok(bytes)
}

pub(crate) struct Roots<'a> {
    api: &'a crate::jsc::JavaScriptCore,
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

// These bytes remain valid only while the returned Roots live.
pub(crate) fn input<'a>(
    api: &'a crate::jsc::JavaScriptCore,
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
    // Protect all items before borrowing bytes: getters can trigger GC. Host operations must not run
    // JavaScript while bytes are borrowed.
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
        // These borrow the arguments of the running call, so callers that keep them must copy them first.
        buffers.push(Bytes::from_static(bytes));
    }
    Ok((buffers, roots))
}

pub(crate) unsafe extern "C" fn release(_pointer: crate::jsc::Ptr, owner: crate::jsc::Ptr) {
    drop(unsafe { Box::from_raw(owner.cast::<Vec<u8>>()) });
}

pub(crate) unsafe extern "C" fn release_bytes(_pointer: crate::jsc::Ptr, owner: crate::jsc::Ptr) {
    drop(unsafe { Box::from_raw(owner.cast::<Bytes>()) });
}

// A call result that is JSON text already. crate::jsc::JavaScriptCore parses it as it is, so a large message is not parsed and
// serialized again here. `buffers` has the shape of the result, but only down to its `$appsBytes` markers.
pub(crate) fn raw(json: String, buffers: Value) -> Value {
    json!({"$appsRaw": json, "$appsBuffers": buffers})
}

pub(crate) fn output(
    api: &crate::jsc::JavaScriptCore,
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

fn buffer_object(
    api: &crate::jsc::JavaScriptCore,
    ctx: crate::jsc::Ptr,
    bytes: bytes::Bytes,
) -> (crate::jsc::Ptr, bool) {
    let taken = taken(&bytes);
    unsafe {
        if let (Some((contents, kind)), Some(functions)) = (taken, api.contents)
            && kind != MOVED
        {
            let object = (functions.make)(ctx, contents);
            if !object.is_null() {
                return (object, kind == SHARED_BUFFER);
            }
        }
        // Memory that another thread gave up has one owner, so the buffer can use it as it is.
        if taken.is_some() {
            let bytes = Box::new(bytes);
            let buffer = (api.make_buffer)(
                ctx,
                bytes.as_ptr().cast_mut().cast(),
                bytes.len(),
                Some(release_bytes),
                Box::into_raw(bytes).cast(),
                std::ptr::null_mut(),
            );
            return (buffer, true);
        }
        let bytes = Box::new(Vec::from(bytes));
        let buffer = (api.make_buffer)(
            ctx,
            bytes.as_ptr().cast_mut().cast(),
            bytes.len(),
            Some(release),
            Box::into_raw(bytes).cast(),
            std::ptr::null_mut(),
        );
        (buffer, true)
    }
}

pub(crate) fn buffer(
    api: &crate::jsc::JavaScriptCore,
    ctx: crate::jsc::Ptr,
    bytes: bytes::Bytes,
) -> crate::jsc::Ptr {
    // Copy small results into JSC memory. External buffers cost more and escape GC accounting.
    if bytes.len() <= 16384
        && taken(&bytes).is_none_or(|(_, kind)| kind == MOVED)
        && let Ok((array, out)) = api.uint8_array(ctx, bytes.len())
    {
        out.copy_from_slice(&bytes);
        return array;
    }
    match buffer_object(api, ctx, bytes) {
        (buffer, true) => unsafe { (api.typed_from_buffer)(ctx, 3, buffer, std::ptr::null_mut()) },
        (object, false) => object,
    }
}

pub(crate) fn result(
    api: &crate::jsc::JavaScriptCore,
    ctx: crate::jsc::Ptr,
    payload: Payload,
) -> crate::jsc::Ptr {
    use serde_json::Value;
    let Payload { value, mut buffers } = payload;
    unsafe {
        match value {
            Value::Null => (api.null)(ctx),
            Value::Bool(value) => (api.boolean)(ctx, value),
            Value::Number(value) => (api.number)(ctx, value.as_f64().unwrap_or(f64::NAN)),
            Value::String(text) => api
                .utf8(ctx, text.as_bytes())
                .unwrap_or_else(|_| (api.null)(ctx)),
            Value::Array(values)
                if values.len() <= 32
                    && values
                        .iter()
                        .all(|value| !value.is_array() && !value.is_object()) =>
            {
                // The items stay on the stack, where JSC's conservative scan keeps them alive.
                let mut items = [std::ptr::null_mut(); 32];
                let count = values.len();
                for (item, value) in items.iter_mut().zip(values) {
                    *item = result(api, ctx, value.into());
                }
                (api.make_array)(ctx, count, items.as_ptr(), std::ptr::null_mut())
            }
            value => {
                if let Some(bytes) = value
                    .get("$appsBytes")
                    .and_then(Value::as_u64)
                    .and_then(|index| buffers.get_mut(index as usize))
                {
                    return buffer(api, ctx, std::mem::take(bytes));
                }
                // See `payload::raw`.
                let (text, value) = match value {
                    Value::Object(mut raw) if raw.contains_key("$appsRaw") => {
                        match (raw.remove("$appsRaw"), raw.remove("$appsBuffers")) {
                            (Some(Value::String(text)), Some(tree)) => (text, tree),
                            _ => return (api.null)(ctx),
                        }
                    }
                    value => (value.to_string(), value),
                };
                let markers = !buffers.is_empty();
                let json = api.string_owned(text);
                let root = (api.json_value)(ctx, json);
                (api.string_release)(json);
                if !markers || root.is_null() {
                    return root;
                }
                (api.protect)(ctx, root);
                let mut arrays: Vec<(crate::jsc::Ptr, bool)> =
                    vec![(std::ptr::null_mut(), true); buffers.len()];
                let mut pending = vec![(&value, root)];
                while let Some((value, object)) = pending.pop() {
                    let children: Vec<(crate::jsc::Ptr, Option<u32>, &Value)> = match value {
                        Value::Array(values) => values
                            .iter()
                            .enumerate()
                            .map(|(index, value)| (std::ptr::null_mut(), Some(index as u32), value))
                            .collect(),
                        Value::Object(values) => values
                            .iter()
                            .map(|(key, value)| (api.string(key), None, value))
                            .collect(),
                        _ => Vec::new(),
                    };
                    for (key, index, child) in children {
                        if let Some(marker) = child.get("$appsBytes").and_then(Value::as_u64)
                            && let Some(bytes) = buffers.get_mut(marker as usize)
                        {
                            let array = &mut arrays[marker as usize];
                            if array.0.is_null() {
                                *array = buffer_object(api, ctx, std::mem::take(bytes));
                                (api.protect)(ctx, array.0);
                            }
                            let view = if array.1 {
                                (api.typed_from_buffer)(ctx, 3, array.0, std::ptr::null_mut())
                            } else {
                                array.0
                            };
                            match index {
                                Some(index) => {
                                    (api.set_index)(ctx, object, index, view, std::ptr::null_mut())
                                }
                                None => (api.set)(ctx, object, key, view, 0, std::ptr::null_mut()),
                            }
                        } else if child.is_array() || child.is_object() {
                            let inner = match index {
                                Some(index) => {
                                    (api.get_index)(ctx, object, index, std::ptr::null_mut())
                                }
                                None => (api.get)(ctx, object, key, std::ptr::null_mut()),
                            };
                            if !inner.is_null() && (api.value_type)(ctx, inner) == 5 {
                                pending.push((child, inner));
                            }
                        }
                        if !key.is_null() {
                            (api.string_release)(key);
                        }
                    }
                }
                for (array, _) in arrays.into_iter().filter(|array| !array.0.is_null()) {
                    (api.unprotect)(ctx, array);
                }
                (api.unprotect)(ctx, root);
                root
            }
        }
    }
}

// Calls the global function `function` of a context with a payload.
pub(crate) fn bridge(
    api: &crate::jsc::JavaScriptCore,
    context: crate::jsc::Ptr,
    function: &str,
    payload: Payload,
) -> Result<crate::jsc::Ptr> {
    let name = api.string(function);
    let mut error = std::ptr::null_mut();
    let receiver = unsafe { (api.get)(context, (api.global)(context), name, &mut error) };
    unsafe {
        (api.string_release)(name);
    }
    // JSC casts call targets to objects without checking, so a primitive would crash.
    anyhow::ensure!(
        error.is_null()
            && unsafe { (api.value_type)(context, receiver) } == 5
            && unsafe { (api.is_function)(context, receiver) },
        "Bridge target is unavailable"
    );
    let argument = output(api, context, payload);
    unsafe {
        (api.protect)(context, argument);
    }
    let result = unsafe {
        (api.invoke)(
            context,
            receiver,
            std::ptr::null_mut(),
            1,
            &argument,
            &mut error,
        )
    };
    unsafe {
        (api.unprotect)(context, argument);
    }
    if !error.is_null() {
        anyhow::bail!("{}", api.text(context, error));
    }
    Ok(result)
}

pub(crate) fn receive(
    api: &crate::jsc::JavaScriptCore,
    context: crate::jsc::Ptr,
    payload: Payload,
) -> Result<Payload> {
    let result = bridge(api, context, "__appsBridgeReceiveBytes", payload)?;
    anyhow::ensure!(
        !result.is_null() && unsafe { (api.value_type)(context, result) } == 5,
        "Invalid bridge reply"
    );
    unsafe {
        (api.protect)(context, result);
    }
    let mut error = std::ptr::null_mut();
    let decoded = (|| -> Result<_> {
        let name = api.string("json");
        let json = unsafe { (api.get)(context, result, name, &mut error) };
        unsafe {
            (api.string_release)(name);
        }
        let mut json = api.text(context, json);
        crate::jsc::well_formed(&mut json);
        let value = serde_json::from_str(&json)?;
        let name = api.string("buffers");
        let buffers = unsafe { (api.get)(context, result, name, &mut error) };
        unsafe {
            (api.string_release)(name);
        }
        let buffers = input(api, context, buffers)?
            .0
            .iter()
            .map(|bytes| bytes::Bytes::copy_from_slice(bytes))
            .collect();
        Ok(Payload { value, buffers })
    })();
    unsafe {
        (api.unprotect)(context, result);
    }
    decoded
}
