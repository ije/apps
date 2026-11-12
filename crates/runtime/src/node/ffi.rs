use crate::{
    host::Role,
    jsc::{JavaScriptCore, Ptr},
};
use anyhow::{Context, Result, bail, ensure};
use libffi::{
    low,
    middle::{Cif, Type},
    raw,
};
use libloading::Library;
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    ffi::{CStr, c_char},
    ptr,
    rc::{Rc, Weak},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::ThreadId,
};

pub(crate) enum Output {
    Json(Value),
    Object(Ptr),
}

struct Active<'a>(&'a Cell<usize>);
impl<'a> Active<'a> {
    fn new(count: &'a Cell<usize>) -> Self {
        count.set(count.get() + 1);
        Self(count)
    }
}
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

struct Protected<'a>(&'a JavaScriptCore, Ptr, Ptr);
impl<'a> Protected<'a> {
    fn new(api: &'a JavaScriptCore, ctx: Ptr, value: Ptr) -> Self {
        unsafe {
            (api.protect)(ctx, value);
        }
        Self(api, ctx, value)
    }
}
impl Drop for Protected<'_> {
    fn drop(&mut self) {
        unsafe {
            (self.0.unprotect)(self.1, self.2);
        }
    }
}

struct NativeLibrary {
    callbacks: RefCell<HashMap<usize, Callback>>,
    handle: Library,
    active: Cell<usize>,
}
struct Function {
    owner: Rc<NativeLibrary>,
    cif: Cif,
    pointer: low::CodePtr,
    count: usize,
}
struct CallbackData {
    thread: ThreadId,
    kinds: Vec<usize>,
    result: usize,
    foreign_thread: Arc<AtomicBool>,
    active: Cell<usize>,
    owner: Weak<NativeLibrary>,
    api: Rc<JavaScriptCore>,
    ctx: Ptr,
    function: Ptr,
    errors: Rc<RefCell<Vec<Option<String>>>>,
}
struct Callback {
    raw: *mut low::ffi_closure,
    data: Box<CallbackData>,
    _cif: Box<Cif>,
}
impl Drop for Callback {
    fn drop(&mut self) {
        unsafe {
            low::closure_free(self.raw);
            (self.data.api.unprotect)(self.data.ctx, self.data.function);
        }
    }
}

fn native_type(kind: usize) -> Result<Type> {
    Ok(match kind {
        0 => Type::void(),
        1 => Type::i8(),
        2 => Type::u8(),
        3 => Type::i16(),
        4 => Type::u16(),
        5 => Type::i32(),
        6 => Type::u32(),
        7 => Type::i64(),
        8 => Type::u64(),
        9 => Type::f32(),
        10 => Type::f64(),
        11 => Type::pointer(),
        _ => bail!("Invalid FFI type"),
    })
}
fn signature(value: &Value) -> Result<(Cif, Vec<usize>, usize)> {
    let kinds: Vec<usize> = serde_json::from_value(value["arguments"].clone())?;
    ensure!(
        kinds.iter().all(|kind| *kind > 0),
        "FFI arguments cannot be void"
    );
    let result = value["return"]
        .as_u64()
        .context("Missing FFI return type")? as usize;
    let types = kinds
        .iter()
        .map(|kind| native_type(*kind))
        .collect::<Result<Vec<_>>>()?;
    Ok((
        Cif::try_new(types, native_type(result)?)
            .map_err(|error| anyhow::anyhow!("Invalid FFI signature: {error:?}"))?,
        kinds,
        result,
    ))
}
fn address(value: &Value) -> Result<usize> {
    Ok(value.as_str().context("Expected pointer")?.parse()?)
}
fn size(kind: usize) -> usize {
    match kind {
        0 => 0,
        1 | 2 => 1,
        3 | 4 => 2,
        5 | 6 | 9 => 4,
        11 => size_of::<usize>(),
        _ => 8,
    }
}

unsafe extern "C" fn free_buffer(_bytes: Ptr, context: Ptr) {
    unsafe {
        drop(Box::from_raw(context as *mut Vec<u8>));
    }
}
fn buffer(api: &JavaScriptCore, ctx: Ptr, bytes: Vec<u8>) -> Result<Ptr> {
    let mut bytes = Box::new(bytes);
    let data = bytes.as_mut_ptr();
    let length = bytes.len();
    let mut exception = ptr::null_mut();
    let result = unsafe {
        (api.make_buffer)(
            ctx,
            data.cast(),
            length,
            Some(free_buffer),
            Box::into_raw(bytes).cast(),
            &mut exception,
        )
    };
    ensure!(
        exception.is_null(),
        "Cannot create FFI buffer: {}",
        api.text(ctx, exception)
    );
    ensure!(!result.is_null(), "Cannot create FFI buffer");
    Ok(result)
}
fn bytes(api: &JavaScriptCore, ctx: Ptr, view: Ptr) -> Result<(*mut u8, usize)> {
    let mut exception = ptr::null_mut();
    unsafe {
        let length = (api.typed_length)(ctx, view, &mut exception);
        let offset = (api.typed_offset)(ctx, view, &mut exception);
        let pointer = (api.typed_bytes)(ctx, view, &mut exception);
        ensure!(
            exception.is_null(),
            "Invalid FFI buffer: {}",
            api.text(ctx, exception)
        );
        ensure!(!pointer.is_null() || length == 0, "Invalid FFI buffer");
        Ok((pointer.cast::<u8>().wrapping_add(offset), length))
    }
}

unsafe extern "C" fn callback(_cif: *mut low::ffi_cif, result: Ptr, args: *mut Ptr, data: Ptr) {
    let data = unsafe { &*data.cast::<CallbackData>() };
    let result_size = if matches!(data.result, 1..=6) {
        size_of::<usize>()
    } else {
        size(data.result)
    };
    if result_size > 0 {
        unsafe {
            ptr::write_bytes(result.cast::<u8>(), 0, result_size);
        }
    }
    if std::thread::current().id() != data.thread {
        data.foreign_thread.store(true, Ordering::Relaxed);
        return;
    }
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
        let owner = data
            .owner
            .upgrade()
            .context("FFI callback library is closed")?;
        let _library_active = Active::new(&owner.active);
        let _callback_active = Active::new(&data.active);
        let mut storage = vec![0u8; (data.kinds.len() + 1) * 8];
        for (index, kind) in data.kinds.iter().enumerate() {
            unsafe {
                ptr::copy_nonoverlapping(
                    (*args.add(index)).cast::<u8>(),
                    storage.as_mut_ptr().add((index + 1) * 8),
                    size(*kind),
                );
            }
        }
        let pointer = storage.as_mut_ptr();
        let object = buffer(&data.api, data.ctx, storage)?;
        let _protected = Protected::new(&data.api, data.ctx, object);
        let mut exception = ptr::null_mut();
        unsafe {
            (data.api.invoke)(
                data.ctx,
                data.function,
                ptr::null_mut(),
                1,
                &object,
                &mut exception,
            );
        }
        ensure!(
            exception.is_null(),
            "FFI callback failed: {}",
            data.api.text(data.ctx, exception)
        );
        unsafe {
            match data.result {
                1 => ptr::write_unaligned(
                    result.cast::<isize>(),
                    ptr::read_unaligned(pointer.cast::<i8>()) as isize,
                ),
                2 => ptr::write_unaligned(
                    result.cast::<usize>(),
                    ptr::read_unaligned(pointer.cast::<u8>()) as usize,
                ),
                3 => ptr::write_unaligned(
                    result.cast::<isize>(),
                    ptr::read_unaligned(pointer.cast::<i16>()) as isize,
                ),
                4 => ptr::write_unaligned(
                    result.cast::<usize>(),
                    ptr::read_unaligned(pointer.cast::<u16>()) as usize,
                ),
                5 => ptr::write_unaligned(
                    result.cast::<isize>(),
                    ptr::read_unaligned(pointer.cast::<i32>()) as isize,
                ),
                6 => ptr::write_unaligned(
                    result.cast::<usize>(),
                    ptr::read_unaligned(pointer.cast::<u32>()) as usize,
                ),
                kind if kind != 0 => {
                    ptr::copy_nonoverlapping(pointer, result.cast::<u8>(), size(kind))
                }
                _ => {}
            }
        }
        Ok(())
    }));
    let error = match outcome {
        Ok(Ok(())) => return,
        Ok(Err(error)) => error.to_string(),
        Err(_) => "FFI callback panicked".into(),
    };
    if let Some(slot) = data.errors.borrow_mut().last_mut() {
        *slot = Some(error);
    } else {
        eprintln!("apps: FFI callback: {error}");
    }
}

#[derive(Default)]
pub(crate) struct State {
    libraries: RefCell<HashMap<u64, Rc<NativeLibrary>>>,
    functions: RefCell<HashMap<u64, Rc<Function>>>,
    next_id: Cell<u64>,
    errors: Rc<RefCell<Vec<Option<String>>>>,
    foreign_thread: Arc<AtomicBool>,
}
impl State {
    pub(crate) fn call(
        &self,
        api: &Rc<JavaScriptCore>,
        ctx: Ptr,
        role: Role,
        request: &Value,
        values: &[Ptr],
    ) -> Result<Output> {
        ensure!(
            !matches!(role, Role::Renderer(_)),
            "FFI is unavailable in page scripts"
        );
        let op = request["op"].as_str().context("Missing FFI operation")?;
        let args = &request["args"];
        let value = match op {
            "info" => {
                json!({"suffix": if cfg!(windows) { "dll" } else if cfg!(target_os = "macos") { "dylib" } else { "so" }, "littleEndian": cfg!(target_endian = "little"), "pointerSize": size_of::<usize>(), "signedChar": c_char::MIN < 0})
            }
            "open" => {
                let handle = if args[0].is_null() {
                    #[cfg(unix)]
                    {
                        libloading::os::unix::Library::this().into()
                    }
                    #[cfg(not(unix))]
                    {
                        bail!("Loading the current process is unsupported on this platform");
                    }
                } else {
                    unsafe { Library::new(args[0].as_str().context("Expected library path")?)? }
                };
                let id = self.next_id.get() + 1;
                self.next_id.set(id);
                self.libraries.borrow_mut().insert(
                    id,
                    Rc::new(NativeLibrary {
                        handle,
                        active: Cell::new(0),
                        callbacks: RefCell::default(),
                    }),
                );
                json!(id)
            }
            "close" => {
                let id = args[0].as_u64().context("Invalid library handle")?;
                if let Some(library) = self.libraries.borrow().get(&id).cloned() {
                    ensure!(
                        library.active.get() == 0,
                        "Cannot close a library during an FFI call or callback"
                    );
                    self.functions
                        .borrow_mut()
                        .retain(|_, function| !Rc::ptr_eq(&function.owner, &library));
                }
                self.libraries.borrow_mut().remove(&id);
                Value::Null
            }
            "symbol" | "function" | "callback" | "unregister" => {
                let library = self
                    .libraries
                    .borrow()
                    .get(&args[0].as_u64().context("Invalid library handle")?)
                    .cloned()
                    .context("Library is closed")?;
                if op == "unregister" {
                    let pointer = address(&args[1])?;
                    let mut callbacks = library.callbacks.borrow_mut();
                    let callback = callbacks.get(&pointer).context("Unknown callback")?;
                    ensure!(
                        callback.data.active.get() == 0,
                        "Cannot unregister an active callback"
                    );
                    callbacks.remove(&pointer);
                    Value::Null
                } else if op == "callback" {
                    let function = *values.first().context("Missing callback")?;
                    let (cif, kinds, result) = signature(&args[1])?;
                    let cif = Box::new(cif);
                    let (raw, pointer) = low::try_closure_alloc()
                        .context("Cannot allocate executable memory for FFI callback")?;
                    unsafe {
                        (api.protect)(ctx, function);
                    }
                    let callback = Callback {
                        raw,
                        _cif: cif,
                        data: Box::new(CallbackData {
                            thread: std::thread::current().id(),
                            kinds,
                            result,
                            foreign_thread: self.foreign_thread.clone(),
                            active: Cell::new(0),
                            owner: Rc::downgrade(&library),
                            api: api.clone(),
                            ctx,
                            function,
                            errors: self.errors.clone(),
                        }),
                    };
                    let status = unsafe {
                        raw::ffi_prep_closure_loc(
                            raw,
                            callback._cif.as_raw_ptr(),
                            Some(crate::node::ffi::callback),
                            (&*callback.data as *const CallbackData).cast_mut().cast(),
                            pointer.0,
                        )
                    };
                    ensure!(
                        status == raw::ffi_status_FFI_OK,
                        "Cannot prepare FFI callback"
                    );
                    library
                        .callbacks
                        .borrow_mut()
                        .insert(pointer.0 as usize, callback);
                    json!((pointer.0 as usize).to_string())
                } else {
                    let name = args[1].as_str().context("Expected symbol name")?;
                    ensure!(
                        !name.is_empty() && !name.contains('\0'),
                        "Invalid symbol name"
                    );
                    let pointer = unsafe { *library.handle.get::<Ptr>(name.as_bytes())? };
                    ensure!(!pointer.is_null(), "Symbol has a null address");
                    if op == "symbol" {
                        json!((pointer as usize).to_string())
                    } else {
                        let (cif, kinds, _) = signature(&args[2])?;
                        let id = self.next_id.get() + 1;
                        self.next_id.set(id);
                        self.functions.borrow_mut().insert(
                            id,
                            Rc::new(Function {
                                owner: library,
                                cif,
                                pointer: low::CodePtr(pointer),
                                count: kinds.len(),
                            }),
                        );
                        json!({"id": id, "pointer": (pointer as usize).to_string()})
                    }
                }
            }
            "invoke" => {
                let function = self
                    .functions
                    .borrow()
                    .get(&args[0].as_u64().context("Invalid function handle")?)
                    .cloned()
                    .context("FFI function is closed")?;
                let (storage, length) =
                    bytes(api, ctx, *values.first().context("Missing call buffer")?)?;
                ensure!(
                    length == (function.count + 1) * 8,
                    "Invalid FFI call buffer"
                );
                let mut slots = vec![0u64; function.count];
                unsafe {
                    ptr::copy_nonoverlapping(
                        storage.add(8),
                        slots.as_mut_ptr().cast(),
                        function.count * 8,
                    );
                }
                let mut pointers: Vec<Ptr> = slots
                    .iter_mut()
                    .map(|slot| (slot as *mut u64).cast())
                    .collect();
                let mut result = 0u64;
                let _active = Active::new(&function.owner.active);
                self.errors.borrow_mut().push(None);
                // SAFETY: the script must give the right native signature and valid pointers; nothing here can check
                // them.
                unsafe {
                    low::call_return_into(
                        function.cif.as_raw_ptr(),
                        function.pointer,
                        pointers.as_mut_ptr(),
                        (&mut result as *mut u64).cast(),
                    );
                }
                let callback_error = self.errors.borrow_mut().pop().flatten();
                ensure!(
                    !self.foreign_thread.swap(false, Ordering::Relaxed),
                    "FFI callbacks must run on their creating thread"
                );
                if let Some(error) = callback_error {
                    bail!(error);
                }
                // SAFETY: `bytes` pins the private call buffer, so `storage` stays valid across callbacks.
                unsafe {
                    ptr::copy_nonoverlapping((&result as *const u64).cast::<u8>(), storage, 8);
                    return Ok(Output::Object((api.undefined)(ctx)));
                }
            }
            "pointer" => {
                let (pointer, _) =
                    bytes(api, ctx, *values.first().context("Missing byte storage")?)?;
                json!((pointer as usize).to_string())
            }
            "view" => {
                let pointer = address(&args[0])?;
                let length: usize = args[1]
                    .as_u64()
                    .context("Invalid buffer length")?
                    .try_into()?;
                ensure!(
                    length <= isize::MAX as usize && pointer.checked_add(length).is_some(),
                    "Invalid memory range"
                );
                ensure!(pointer != 0 || length == 0, "Cannot read a null pointer");
                if args[2] != false {
                    let bytes = if length == 0 {
                        Vec::new()
                    } else {
                        unsafe { std::slice::from_raw_parts(pointer as *const u8, length) }.to_vec()
                    };
                    return Ok(Output::Object(buffer(api, ctx, bytes)?));
                }
                let mut exception = ptr::null_mut();
                let view = unsafe {
                    (api.make_buffer)(
                        ctx,
                        pointer as Ptr,
                        length,
                        None,
                        ptr::null_mut(),
                        &mut exception,
                    )
                };
                ensure!(
                    exception.is_null() && !view.is_null(),
                    "Cannot create native memory view"
                );
                return Ok(Output::Object(view));
            }
            "string" => {
                let pointer = address(&args[0])?;
                if pointer == 0 {
                    Value::Null
                } else {
                    json!(unsafe { CStr::from_ptr(pointer as *const c_char) }.to_string_lossy())
                }
            }
            "copy" => {
                let target = address(&args[0])?;
                let capacity = args[1].as_u64().context("Invalid buffer length")?;
                let (source, length) =
                    bytes(api, ctx, *values.first().context("Missing byte storage")?)?;
                ensure!(
                    capacity >= length as u64 && target.checked_add(length).is_some(),
                    "Destination is too small"
                );
                ensure!(target != 0 || length == 0, "Cannot write a null pointer");
                if length > 0 {
                    unsafe {
                        ptr::copy(source, target as *mut u8, length);
                    }
                }
                Value::Null
            }
            _ => bail!("Unsupported FFI operation: {op}"),
        };
        Ok(Output::Json(value))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn callback_outside_a_call_does_not_poison_the_next_call() {
        let engine =
            crate::jsc::JsContext::new(JavaScriptCore::load().unwrap(), Rc::default(), Role::Main)
                .unwrap();
        let address = engine.eval("globalThis.lib=new __apps.ffi.DynamicLibrary(null);String(lib.registerCallback(()=>{throw Error('outside call')}))", "test").unwrap().parse::<usize>().unwrap();
        let callback: unsafe extern "C" fn() = unsafe { std::mem::transmute(address) };
        unsafe {
            callback();
        }
        assert_eq!(
            engine
                .eval(
                    "lib.getFunction('abs',{arguments:['i32'],return:'i32'})(-3)",
                    "test"
                )
                .unwrap(),
            "3"
        );
        engine.eval("lib.close()", "test").unwrap();
    }
}
