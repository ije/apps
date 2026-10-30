pub(crate) mod payload;

use crate::host::{Host, Role};
use anyhow::{Context, Result, bail, ensure};
use bbtree::unsync::BBTreeMap;
use libloading::Library;
use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::{c_char, c_void},
    path::{Path, PathBuf},
    ptr,
    rc::Rc,
};

pub(crate) type Ptr = *mut c_void;
type Callback = unsafe extern "C" fn(Ptr, Ptr, Ptr, usize, *const Ptr, *mut Ptr) -> Ptr;

pub struct Api {
    _library: Library,
    group: Ptr,
    group_release: unsafe extern "C" fn(Ptr),
    garbage_collect: unsafe extern "C" fn(Ptr),
    shrink: Option<unsafe extern "C" fn(Ptr)>,
    pub(crate) busy: std::cell::Cell<std::time::Duration>,
    pub(crate) idle: std::cell::Cell<bool>,
    inspectable: Option<unsafe extern "C" fn(Ptr, bool)>,
    extra_memory: Option<unsafe extern "C" fn(Ptr, usize)>,
    set_name: Option<unsafe extern "C" fn(Ptr, Ptr)>,
    context_group: unsafe extern "C" fn(Ptr) -> Ptr,
    execution_limit:
        Option<unsafe extern "C" fn(Ptr, f64, Option<unsafe extern "C" fn(Ptr, Ptr) -> bool>, Ptr)>,
    create: unsafe extern "C" fn(Ptr, Ptr) -> Ptr,
    release: unsafe extern "C" fn(Ptr),
    pub(crate) string_create: unsafe extern "C" fn(*const u16, usize) -> Ptr,
    string_create_utf8: unsafe extern "C" fn(*const c_char) -> Ptr,
    pub(crate) string_release: unsafe extern "C" fn(Ptr),
    pub(crate) string_size: unsafe extern "C" fn(Ptr) -> usize,
    pub(crate) string_utf8: unsafe extern "C" fn(Ptr, *mut c_char, usize) -> usize,
    pub(crate) evaluate: unsafe extern "C" fn(Ptr, Ptr, Ptr, Ptr, i32, *mut Ptr) -> Ptr,
    pub(crate) to_string: unsafe extern "C" fn(Ptr, Ptr, *mut Ptr) -> Ptr,
    pub(crate) make_string: unsafe extern "C" fn(Ptr, Ptr) -> Ptr,
    json_value: unsafe extern "C" fn(Ptr, Ptr) -> Ptr,
    pub(crate) typed_bytes: unsafe extern "C" fn(Ptr, Ptr, *mut Ptr) -> Ptr,
    pub(crate) typed_length: unsafe extern "C" fn(Ptr, Ptr, *mut Ptr) -> usize,
    pub(crate) typed_offset: unsafe extern "C" fn(Ptr, Ptr, *mut Ptr) -> usize,
    pub(crate) make_buffer: unsafe extern "C" fn(
        Ptr,
        Ptr,
        usize,
        Option<unsafe extern "C" fn(Ptr, Ptr)>,
        Ptr,
        *mut Ptr,
    ) -> Ptr,
    pub(crate) invoke: unsafe extern "C" fn(Ptr, Ptr, Ptr, usize, *const Ptr, *mut Ptr) -> Ptr,
    pub(crate) protect: unsafe extern "C" fn(Ptr, Ptr),
    pub(crate) unprotect: unsafe extern "C" fn(Ptr, Ptr),
    pub(crate) object_make: unsafe extern "C" fn(Ptr, Ptr, Ptr) -> Ptr,
    pub(crate) global: unsafe extern "C" fn(Ptr) -> Ptr,
    pub(crate) function: unsafe extern "C" fn(Ptr, Ptr, Callback) -> Ptr,
    pub(crate) get: unsafe extern "C" fn(Ptr, Ptr, Ptr, *mut Ptr) -> Ptr,
    rejection_callback: unsafe extern "C" fn(Ptr, Ptr, *mut Ptr),
    pub(crate) set: unsafe extern "C" fn(Ptr, Ptr, Ptr, Ptr, u32, *mut Ptr),
    pub(crate) undefined: unsafe extern "C" fn(Ptr) -> Ptr,
    pub(crate) null: unsafe extern "C" fn(Ptr) -> Ptr,
    pub(crate) boolean: unsafe extern "C" fn(Ptr, bool) -> Ptr,
    pub(crate) number: unsafe extern "C" fn(Ptr, f64) -> Ptr,
    pub(crate) is_function: unsafe extern "C" fn(Ptr, Ptr) -> bool,
    pub(crate) value_type: unsafe extern "C" fn(Ptr, Ptr) -> u32,
    pub(crate) to_number: unsafe extern "C" fn(Ptr, Ptr, *mut Ptr) -> f64,
    pub(crate) to_boolean: unsafe extern "C" fn(Ptr, Ptr) -> bool,
    pub(crate) string_length: unsafe extern "C" fn(Ptr) -> usize,
    pub(crate) string_chars: unsafe extern "C" fn(Ptr) -> *const u16,
    pub(crate) typed_kind: unsafe extern "C" fn(Ptr, Ptr, *mut Ptr) -> u32,
    pub(crate) typed_buffer: unsafe extern "C" fn(Ptr, Ptr, *mut Ptr) -> Ptr,
    pub(crate) buffer_bytes: unsafe extern "C" fn(Ptr, Ptr, *mut Ptr) -> Ptr,
    pub(crate) buffer_length: unsafe extern "C" fn(Ptr, Ptr, *mut Ptr) -> usize,
    pub(crate) get_index: unsafe extern "C" fn(Ptr, Ptr, u32, *mut Ptr) -> Ptr,
    set_index: unsafe extern "C" fn(Ptr, Ptr, u32, Ptr, *mut Ptr),
    make_error: unsafe extern "C" fn(Ptr, usize, *const Ptr, *mut Ptr) -> Ptr,
    make_array: unsafe extern "C" fn(Ptr, usize, *const Ptr, *mut Ptr) -> Ptr,
    typed_from_buffer: unsafe extern "C" fn(Ptr, u32, Ptr, *mut Ptr) -> Ptr,
    memory: Option<unsafe extern "C" fn(Ptr) -> Ptr>,
    json_string: unsafe extern "C" fn(Ptr, Ptr, u32, *mut Ptr) -> Ptr,
    inspector: Option<Inspector>,
}

type InspectorMessage = unsafe extern "C" fn(Ptr, *const c_char, usize);
type InspectorPause = unsafe extern "C" fn(Ptr, *mut bool);

struct Inspector {
    connect: unsafe extern "C" fn(Ptr, InspectorMessage, InspectorPause, Ptr) -> Ptr,
    dispatch: unsafe extern "C" fn(Ptr, *const c_char, usize),
    disconnect: unsafe extern "C" fn(Ptr, Ptr),
}

pub fn library_name(platform: &str) -> Result<&'static str> {
    match platform {
        "macos" => Ok("libJavaScriptCore.dylib"),
        "linux" => Ok("libJavaScriptCore.so"),
        "windows" => Ok("JavaScriptCore.dll"),
        _ => bail!("Unsupported JavaScriptCore platform: {platform}"),
    }
}

pub fn library_path(executable: &Path, platform: &str) -> Result<PathBuf> {
    let executable = executable.canonicalize()?;
    let directory = executable
        .parent()
        .context("Missing executable directory")?;
    let name = library_name(platform)?;
    // WARNING: a built app must never load a library named by the environment or the build machine.
    if [
        "app.h2o",
        "app.js",
        "../Resources/app.h2o",
        "../Resources/app.js",
    ]
    .iter()
    .any(|bundle| directory.join(bundle).is_file())
        && executable == std::env::current_exe()?.canonicalize()?
    {
        return ["../Frameworks", "."]
            .iter()
            .map(|folder| directory.join(folder).join(name))
            .find(|path| path.is_file())
            .context("The app's JavaScriptCore library is missing");
    }
    if let Some(path) = std::env::var_os("APPS_JSC_LIBRARY") {
        let path = PathBuf::from(path);
        anyhow::ensure!(
            path.is_file(),
            "APPS_JSC_LIBRARY needs a standalone library file: {}",
            path.display()
        );
        return Ok(path.canonicalize()?);
    }
    let mut candidates = Vec::with_capacity(3);
    if platform == "macos" {
        candidates.push(directory.join("../Frameworks").join(name));
    }
    candidates.push(directory.join(name));
    candidates.push(Path::new(env!("APPS_JSC_BUILD_DIR")).join(name));
    for path in &candidates {
        if path.is_file() {
            return Ok(path.canonicalize()?);
        }
    }
    bail!(
        "Bun JavaScriptCore is unavailable. Run bash scripts/build-jsc.sh, or set APPS_JSC_LIBRARY to its shared library. Searched: {}",
        candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

impl Api {
    pub fn load() -> Result<Rc<Self>> {
        let path = library_path(&std::env::current_exe()?, std::env::consts::OS)?;
        // WARNING: JSC libraries must export the C ABI and unhandled-rejection callback SPI.
        let library = unsafe { Library::new(&path) }
            .with_context(|| format!("Cannot load Bun JavaScriptCore from {}", path.display()))?;
        unsafe {
            let group_create: unsafe extern "C" fn() -> Ptr =
                *library.get(b"JSContextGroupCreate\0")?;
            Ok(Rc::new(Self {
                // WARNING: engines of one thread share a VM, so JSC reuses compiled runtime code across windows.
                group: group_create(),
                inspectable: library
                    .get(b"JSGlobalContextSetInspectable\0")
                    .ok()
                    .map(|s| *s),
                extra_memory: library.get(b"JSReportExtraMemoryCost\0").ok().map(|s| *s),
                set_name: library.get(b"JSGlobalContextSetName\0").ok().map(|s| *s),
                context_group: *library.get(b"JSContextGetGroup\0")?,
                execution_limit: library
                    .get(b"JSContextGroupSetExecutionTimeLimit\0")
                    .ok()
                    .map(|s| *s),
                group_release: *library.get(b"JSContextGroupRelease\0")?,
                garbage_collect: *library.get(b"JSSynchronousGarbageCollectForDebugging\0")?,
                shrink: library
                    .get(b"JSContextGroupShrinkFootprint\0")
                    .ok()
                    .map(|s| *s),
                busy: Default::default(),
                idle: Default::default(),
                create: *library.get(b"JSGlobalContextCreateInGroup\0")?,
                release: *library.get(b"JSGlobalContextRelease\0")?,
                string_create: *library.get(b"JSStringCreateWithCharacters\0")?,
                string_create_utf8: *library.get(b"JSStringCreateWithUTF8CString\0")?,
                string_release: *library.get(b"JSStringRelease\0")?,
                string_size: *library.get(b"JSStringGetMaximumUTF8CStringSize\0")?,
                string_utf8: *library.get(b"JSStringGetUTF8CString\0")?,
                evaluate: *library.get(b"JSEvaluateScript\0")?,
                to_string: *library.get(b"JSValueToStringCopy\0")?,
                make_string: *library.get(b"JSValueMakeString\0")?,
                json_value: *library.get(b"JSValueMakeFromJSONString\0")?,
                typed_bytes: *library.get(b"JSObjectGetTypedArrayBytesPtr\0")?,
                typed_length: *library.get(b"JSObjectGetTypedArrayByteLength\0")?,
                typed_offset: *library.get(b"JSObjectGetTypedArrayByteOffset\0")?,
                make_buffer: *library.get(b"JSObjectMakeArrayBufferWithBytesNoCopy\0")?,
                invoke: *library.get(b"JSObjectCallAsFunction\0")?,
                protect: *library.get(b"JSValueProtect\0")?,
                unprotect: *library.get(b"JSValueUnprotect\0")?,
                object_make: *library.get(b"JSObjectMake\0")?,
                global: *library.get(b"JSContextGetGlobalObject\0")?,
                function: *library.get(b"JSObjectMakeFunctionWithCallback\0")?,
                get: *library.get(b"JSObjectGetProperty\0")?,
                rejection_callback: *library
                    .get(b"JSGlobalContextSetUnhandledRejectionCallback\0")?,
                set: *library.get(b"JSObjectSetProperty\0")?,
                undefined: *library.get(b"JSValueMakeUndefined\0")?,
                null: *library.get(b"JSValueMakeNull\0")?,
                boolean: *library.get(b"JSValueMakeBoolean\0")?,
                number: *library.get(b"JSValueMakeNumber\0")?,
                is_function: *library.get(b"JSObjectIsFunction\0")?,
                value_type: *library.get(b"JSValueGetType\0")?,
                to_number: *library.get(b"JSValueToNumber\0")?,
                to_boolean: *library.get(b"JSValueToBoolean\0")?,
                string_length: *library.get(b"JSStringGetLength\0")?,
                string_chars: *library.get(b"JSStringGetCharactersPtr\0")?,
                typed_kind: *library.get(b"JSValueGetTypedArrayType\0")?,
                typed_buffer: *library.get(b"JSObjectGetTypedArrayBuffer\0")?,
                buffer_bytes: *library.get(b"JSObjectGetArrayBufferBytesPtr\0")?,
                buffer_length: *library.get(b"JSObjectGetArrayBufferByteLength\0")?,
                get_index: *library.get(b"JSObjectGetPropertyAtIndex\0")?,
                set_index: *library.get(b"JSObjectSetPropertyAtIndex\0")?,
                make_error: *library.get(b"JSObjectMakeError\0")?,
                make_array: *library.get(b"JSObjectMakeArray\0")?,
                typed_from_buffer: *library.get(b"JSObjectMakeTypedArrayWithArrayBuffer\0")?,
                memory: library.get(b"JSGetMemoryUsageStatistics\0").ok().map(|s| *s),
                json_string: *library.get(b"JSValueCreateJSONString\0")?,
                inspector: (|| {
                    Some(Inspector {
                        connect: *library.get(b"JSGlobalContextConnectInspector\0").ok()?,
                        dispatch: *library.get(b"JSGlobalContextDispatchInspectorMessage\0").ok()?,
                        disconnect: *library.get(b"JSGlobalContextDisconnectInspector\0").ok()?,
                    })
                })(),
                _library: library,
            }))
        }
    }

    pub(crate) fn string(&self, text: &str) -> Ptr {
        let mut buffer = [0u8; 128];
        if text.len() < buffer.len() && !text.as_bytes().contains(&0) {
            buffer[..text.len()].copy_from_slice(text.as_bytes());
            return unsafe { (self.string_create_utf8)(buffer.as_ptr().cast()) };
        }
        self.string_owned(text.to_owned())
    }

    pub(crate) fn string_owned(&self, text: String) -> Ptr {
        match std::ffi::CString::new(text) {
            Ok(text) => unsafe { (self.string_create_utf8)(text.as_ptr()) },
            Err(error) => {
                let utf16: Vec<_> = String::from_utf8_lossy(&error.into_vec())
                    .encode_utf16()
                    .collect();
                unsafe { (self.string_create)(utf16.as_ptr(), utf16.len()) }
            }
        }
    }

    pub(crate) fn text(&self, ctx: Ptr, value: Ptr) -> String {
        unsafe {
            let string = (self.to_string)(ctx, value, ptr::null_mut());
            if string.is_null() {
                return "JavaScript exception".into();
            }
            let length = (self.string_length)(string);
            let mut buffer = Vec::<u8>::with_capacity((self.string_size)(string));
            let size = (self.string_utf8)(string, buffer.as_mut_ptr().cast(), buffer.capacity());
            buffer.set_len(size.saturating_sub(1));
            if buffer.len() == length && buffer.is_ascii() {
                (self.string_release)(string);
                return String::from_utf8_unchecked(buffer);
            }
            // WARNING: JSC stops UTF-8 conversion at lone surrogates, so compare UTF-16 lengths.
            if buffer
                .iter()
                .map(|&byte| ((byte as i8) >= -0x40) as usize + (byte >= 0xf0) as usize)
                .sum::<usize>()
                != length
            {
                let text = String::from_utf16_lossy(std::slice::from_raw_parts(
                    (self.string_chars)(string),
                    length,
                ));
                (self.string_release)(string);
                return text;
            }
            (self.string_release)(string);
            String::from_utf8(buffer)
                .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
        }
    }

    pub(crate) fn shrink_pending(&self) -> bool {
        self.shrink.is_some() && self.busy.get() >= std::time::Duration::from_millis(20)
    }

    pub(crate) fn shrink(&self) {
        if let Some(shrink) = self.shrink {
            self.busy.set(Default::default());
            unsafe { shrink(self.group) };
        }
    }
}

impl Drop for Api {
    fn drop(&mut self) {
        unsafe { (self.group_release)(self.group) };
    }
}

type Binding = (
    Rc<Api>,
    Rc<RefCell<Host>>,
    Role,
    Rc<RefCell<crate::node::sqlite::State>>,
    Rc<RefCell<crate::web::State>>,
    Rc<RefCell<crate::node::child_process::State>>,
    Rc<crate::node::ffi::State>,
    Rc<RefCell<crate::node::State>>,
    Rc<RefCell<crate::node::pty::State>>,
);
thread_local! {
    static HOSTS: RefCell<rustc_hash::FxHashMap<usize, Binding>> = RefCell::default();
    static ARMED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static INSPECTED: RefCell<rustc_hash::FxHashMap<usize, Ptr>> = RefCell::default();
    static INSPECTOR_MESSAGES: RefCell<Vec<(usize, String)>> = const { RefCell::new(Vec::new()) };
}

/// Returns the context of this thread with `role`.
pub(crate) fn context(role: Role) -> Option<usize> {
    HOSTS.with_borrow(|hosts| {
        hosts
            .iter()
            .find(|(_, binding)| binding.2 == role)
            .map(|(&ctx, _)| ctx)
    })
}

/// Returns every context of this thread.
pub(crate) fn contexts() -> Vec<(usize, Role)> {
    HOSTS.with_borrow(|hosts| hosts.iter().map(|(&ctx, binding)| (ctx, binding.2)).collect())
}

/// Connects the Web Inspector to a context once. Returns true for a new connection.
pub(crate) fn connect_inspector(ctx: usize) -> Result<bool> {
    if INSPECTED.with_borrow(|inspected| inspected.contains_key(&ctx)) {
        return Ok(false);
    }
    let api = HOSTS
        .with_borrow(|hosts| hosts.get(&ctx).map(|binding| binding.0.clone()))
        .context("The JavaScript context is closed")?;
    let inspector = api
        .inspector
        .as_ref()
        .context("This JavaScriptCore has no inspector. Rebuild it with scripts/build-jsc.sh")?;
    let connection =
        unsafe { (inspector.connect)(ctx as Ptr, inspector_message, inspector_pause, ctx as Ptr) };
    INSPECTED.with_borrow_mut(|inspected| inspected.insert(ctx, connection));
    Ok(true)
}

/// Sends a Web Inspector protocol message. Replies and events arrive in `inspector_messages`.
pub(crate) fn inspect(ctx: usize, message: &str) -> Result<()> {
    connect_inspector(ctx)?;
    let api = HOSTS
        .with_borrow(|hosts| hosts.get(&ctx).map(|binding| binding.0.clone()))
        .context("The JavaScript context is closed")?;
    let inspector = api.inspector.as_ref().context("Missing inspector")?;
    unsafe { (inspector.dispatch)(ctx as Ptr, message.as_ptr().cast(), message.len()) };
    Ok(())
}

pub(crate) fn inspector_messages() -> Vec<(usize, String)> {
    INSPECTOR_MESSAGES.take()
}

unsafe extern "C" fn inspector_message(ctx: Ptr, message: *const c_char, length: usize) {
    let message = unsafe { std::slice::from_raw_parts(message.cast::<u8>(), length) };
    let message = String::from_utf8_lossy(message).into_owned();
    INSPECTOR_MESSAGES.with_borrow_mut(|messages| messages.push((ctx as usize, message)));
}

unsafe extern "C" fn inspector_pause(ctx: Ptr, done: *mut bool) {
    crate::desktop::mcp::paused(ctx as usize, done);
}

unsafe extern "C" fn compile(
    ctx: Ptr,
    _function: Ptr,
    _this: Ptr,
    count: usize,
    args: *const Ptr,
    exception: *mut Ptr,
) -> Ptr {
    let binding = HOSTS.with(|hosts| {
        hosts
            .borrow()
            .get(&(ctx as usize))
            .map(|binding| (binding.0.clone(), binding.2))
    });
    let Some((api, role)) = binding else {
        return ptr::null_mut();
    };
    // WARNING: pages compile only while Rust evaluates one of their own scripts, never from later strings.
    if matches!(role, Role::Renderer(_)) && ARMED.get() != ctx as usize {
        if !exception.is_null() {
            let message = api.string("EvalError: Code generation is disabled in pages");
            unsafe {
                *exception = (api.make_string)(ctx, message);
                (api.string_release)(message);
            }
        }
        return ptr::null_mut();
    }
    if count != 2 {
        return unsafe { (api.undefined)(ctx) };
    }
    let source = unsafe { (api.to_string)(ctx, *args, exception) };
    if source.is_null() {
        return ptr::null_mut();
    }
    let url = unsafe { (api.to_string)(ctx, *args.add(1), exception) };
    if url.is_null() {
        unsafe {
            (api.string_release)(source);
        }
        return ptr::null_mut();
    }
    let result = unsafe { (api.evaluate)(ctx, source, ptr::null_mut(), url, 1, exception) };
    unsafe {
        (api.string_release)(source);
        (api.string_release)(url);
    }
    result
}

// WARNING: this runs only scripts of the app bundle, by name, and never arms code generation.
unsafe extern "C" fn chunk(
    ctx: Ptr,
    _function: Ptr,
    _this: Ptr,
    count: usize,
    args: *const Ptr,
    exception: *mut Ptr,
) -> Ptr {
    let binding = HOSTS.with(|hosts| {
        hosts
            .borrow()
            .get(&(ctx as usize))
            .map(|binding| (binding.0.clone(), binding.1.clone()))
    });
    let Some((api, host)) = binding else {
        return ptr::null_mut();
    };
    let name = if count == 1 {
        api.text(ctx, unsafe { *args })
    } else {
        String::new()
    };
    let source = host
        .try_borrow()
        .ok()
        .and_then(|host| host.scripts.get(&name).cloned());
    let Some(source) = source else {
        if !exception.is_null() {
            let message = api.string(&format!("Error: Missing bundled script: {name}"));
            unsafe {
                *exception = (api.make_string)(ctx, message);
                (api.string_release)(message);
            }
        }
        return ptr::null_mut();
    };
    let script = api.string(&source);
    let url = api.string(&format!("apps:///{name}"));
    let result = unsafe { (api.evaluate)(ctx, script, ptr::null_mut(), url, 1, exception) };
    unsafe {
        (api.string_release)(script);
        (api.string_release)(url);
    }
    result
}

/// Host calls: `native(op, ...args)` takes primitive and binary arguments directly, and
/// `native(0, op, json, buffers?)` takes the rest as JSON. Results come back as JavaScript values,
/// and failures throw an `Error` with the I/O `code`.
unsafe extern "C" fn call(
    ctx: Ptr,
    _function: Ptr,
    _this: Ptr,
    count: usize,
    args: *const Ptr,
    exception: *mut Ptr,
) -> Ptr {
    use serde_json::Value;
    let binding = HOSTS.with(|hosts| hosts.borrow().get(&(ctx as usize)).cloned());
    let Some(binding) = binding else {
        return ptr::null_mut();
    };
    let api = binding.0.clone();
    let values = if count == 0 || args.is_null() {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(args, count) }
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Ptr> {
        let generic = values
            .first()
            .is_some_and(|&value| unsafe { (api.value_type)(ctx, value) } == 3);
        let mut name = [0u8; 64];
        let op = {
            let value = *values
                .get(usize::from(generic))
                .context("Missing host operation")?;
            let string = unsafe { (api.to_string)(ctx, value, ptr::null_mut()) };
            ensure!(!string.is_null(), "Invalid host operation");
            let size = unsafe {
                if (api.string_length)(string) < name.len() {
                    (api.string_utf8)(string, name.as_mut_ptr().cast(), name.len())
                } else {
                    0
                }
            };
            unsafe { (api.string_release)(string) };
            std::str::from_utf8(&name[..size.saturating_sub(1)])
                .ok()
                .filter(|op| !op.is_empty())
                .context("Invalid host operation")?
        };
        let run = |buffers, action: &mut dyn FnMut() -> Result<Value>| -> Result<Ptr> {
            let mut failure = None;
            let payload = crate::jsc::payload::Payload::capture(buffers, || {
                action().unwrap_or_else(|error| {
                    failure = Some(error);
                    Value::Null
                })
            });
            match failure {
                Some(error) => Err(error),
                None => Ok(output(&api, ctx, payload)),
            }
        };
        if generic {
            let buffers = match values.get(3) {
                Some(&buffers) => crate::jsc::payload::input(&api, ctx, buffers)?,
                None => Vec::new(),
            };
            let json = api.text(ctx, *values.get(2).context("Expected JSON arguments")?);
            // WARNING: GPU command lists can be large, so only the WebGPU library parses their JSON.
            if let (Role::Renderer(id), Some(op)) = (binding.2, op.strip_prefix("gpu.")) {
                return run(buffers, &mut || gpu(&binding, ctx, id, op, json.as_bytes()));
            }
            let mut args = Some(serde_json::from_str(&json)?);
            return run(buffers, &mut || {
                dispatch(&binding, ctx, op, args.take().unwrap_or_default())
            });
        }
        let role = binding.2;
        // WARNING: page writes arrive as `dom.batch` with integer codes in bytes and their strings joined in the text, so they skip JSON and copies.
        if let Role::Renderer(id) = role
            && op == "dom.batch"
        {
            ensure!(values.len() == 4, "Expected DOM operations");
            let text = api.text(ctx, values[1]);
            let size = unsafe { (api.to_number)(ctx, values[3], ptr::null_mut()) } * 4.;
            let codes = crate::jsc::payload::view(&api, ctx, values[2])?;
            let codes = (size >= 0. && size.fract() == 0.)
                .then(|| codes.get(..size as usize))
                .flatten()
                .context("Expected DOM operations")?;
            let errors = binding
                .1
                .try_borrow_mut()
                .context("Host is busy")?
                .batch(id, codes, &text)?;
            return Ok(output(&api, ctx, errors.into()));
        }
        if let Role::Renderer(id) = role
            && op == "dom.layout"
        {
            let mut host = binding.1.try_borrow_mut().context("Host is busy")?;
            let host = &mut *host;
            let page = host.pages.get_mut(&id).context("Window is closed")?;
            let state = host.dom.entry(id).or_default();
            let viewport = *host.viewports.get(&id).unwrap_or(&[800., 600.]);
            if values
                .get(2)
                .is_some_and(|&value| unsafe { (api.to_boolean)(ctx, value) })
            {
                crate::dom::layout_flags(&mut page.doc, state, viewport);
            }
            state.sync_scroll();
            if !values
                .get(1)
                .is_some_and(|&value| unsafe { (api.to_boolean)(ctx, value) })
            {
                return Ok(unsafe { (api.null)(ctx) });
            }
            let (pointer, size, owner) = state.bounds.share();
            return Ok(unsafe {
                (api.make_buffer)(
                    ctx,
                    pointer,
                    size,
                    Some(crate::dom::LayoutMirror::release),
                    owner,
                    ptr::null_mut(),
                )
            });
        }
        let mut buffers = Vec::new();
        let mut args = Vec::with_capacity(values.len() - 1);
        for &value in &values[1..] {
            args.push(match unsafe { (api.value_type)(ctx, value) } {
                2 => Value::Bool(unsafe { (api.to_boolean)(ctx, value) }),
                3 => {
                    let number = unsafe { (api.to_number)(ctx, value, ptr::null_mut()) };
                    // WARNING: this matches `JSON.stringify` and serde, so ops see the same integers and non-finite numbers become null.
                    if number.fract() == 0. && number.abs() < 1e21 {
                        if (0. ..18446744073709551616.).contains(&number) {
                            Value::from(number as u64)
                        } else if number >= -9223372036854775808. {
                            Value::from(number as i64)
                        } else {
                            Value::from(number)
                        }
                    } else {
                        Value::from(number)
                    }
                }
                4 => Value::String(api.text(ctx, value)),
                5 if unsafe { (api.is_function)(ctx, value) } => Value::Null,
                5 => {
                    // WARNING: the arguments live until the call returns, so their bytes are borrowed, see `payload::input`.
                    buffers.push(bytes::Bytes::from_static(crate::jsc::payload::view(
                        &api, ctx, value,
                    )?));
                    serde_json::json!({"$appsBytes": buffers.len() - 1})
                }
                7 => bail!("Do not know how to serialize a BigInt"),
                _ => Value::Null,
            });
        }
        let mut args = Some(Value::Array(args));
        run(buffers, &mut || {
            dispatch(&binding, ctx, op, args.take().unwrap_or_default())
        })
    }));
    let (message, code) = match result {
        Ok(Ok(value)) => return value,
        Ok(Err(error)) => (
            error.to_string(),
            error
                .downcast_ref::<std::io::Error>()
                .map(crate::node::code),
        ),
        Err(_) => ("Host call failed".into(), None),
    };
    unsafe {
        let text = api.string_owned(message);
        let message = (api.make_string)(ctx, text);
        (api.string_release)(text);
        let error = (api.make_error)(ctx, 1, &message, ptr::null_mut());
        if let Some(code) = code {
            let key = api.string("code");
            let text = api.string(code);
            (api.set)(
                ctx,
                error,
                key,
                (api.make_string)(ctx, text),
                0,
                ptr::null_mut(),
            );
            (api.string_release)(key);
            (api.string_release)(text);
        }
        if !exception.is_null() {
            *exception = error;
        }
        (api.undefined)(ctx)
    }
}

/// Runs one host operation for the context of `binding`.
fn dispatch(
    binding: &Binding,
    ctx: Ptr,
    op: &str,
    mut args: serde_json::Value,
) -> Result<serde_json::Value> {
    let (_, host, role, sqlite, web, children, _, node, pty) = binding;
    let role = *role;
    let values = &args;
    if let Some(op) = op.strip_prefix("browser.") {
        anyhow::ensure!(
            matches!(role, Role::Main),
            "Browser views can only be managed by main"
        );
        let mut host = host.try_borrow_mut().context("Host is busy")?;
        if host.packaged {
            if op == "command" && values[1] == "inspect" {
                return Ok(serde_json::Value::Null);
            }
            if op == "create" {
                let mut values = values.clone();
                values[0]["inspectable"] = false.into();
                return host.browsers.call(op, &values);
            }
        }
        return host.browsers.call(op, values);
    }
    if let Some(op) = op.strip_prefix("node.") {
        anyhow::ensure!(
            !matches!(role, Role::Renderer(_)),
            "Node APIs are unavailable in page scripts"
        );
        return node
            .try_borrow_mut()
            .context("Node is busy")?
            .call(op, &mut args);
    }
    if let Some(op) = op.strip_prefix("pty.") {
        anyhow::ensure!(
            !matches!(role, Role::Renderer(_)),
            "PTYs are unavailable in page scripts"
        );
        return pty
            .try_borrow_mut()
            .context("PTY is busy")?
            .call(op, values);
    }
    if let Some(op) = op.strip_prefix("native.") {
        anyhow::ensure!(
            !matches!(role, Role::Renderer(_)),
            "Native desktop APIs require a preload bridge"
        );
        let result = crate::desktop::native::native_call(op, &values[0])?;
        if op == "theme.set" {
            host.borrow_mut().sync_theme();
        }
        return Ok(result);
    }
    if op == "bridge.expose" {
        let Role::Preload(id) = role else {
            bail!("Only preload can expose APIs")
        };
        let name = values[0].as_str().context("Expected exposed name")?;
        anyhow::ensure!(
            !matches!(name, "__proto__" | "constructor" | "prototype")
                && !name.starts_with("__apps"),
            "Reserved bridge name"
        );
        host.try_borrow_mut()
            .context("Host is busy")?
            .bridge
            .entry(id)
            .or_default()
            .insert(
                name.to_owned(),
                crate::jsc::payload::Payload::snapshot(values[1].clone()),
            );
        return Ok(serde_json::Value::Null);
    }
    if op == "bridge.exposures" {
        let Role::Renderer(id) = role else {
            bail!("Only pages can read exposed APIs")
        };
        return Ok(serde_json::Value::Object(
            host.borrow()
                .bridge
                .get(&id)
                .into_iter()
                .flat_map(|values| values.iter())
                .map(|(name, value)| (name.clone(), value.clone().into_value()))
                .collect(),
        ));
    }
    if op == "bridge.dispatch" {
        let target = &values[0];
        if let Role::Renderer(id) = role {
            anyhow::ensure!(
                target["preload"] == id
                    && matches!(values[1]["kind"].as_str(), Some("call" | "reply")),
                "Page bridge calls must target their preload"
            );
        }
        let target = serde_json::from_value::<Role>(target.clone()).ok();
        let destination = HOSTS.with(|hosts| {
            hosts.borrow().iter().find_map(|(&ctx, binding)| {
                if Rc::ptr_eq(host, &binding.1) && target == Some(binding.2) {
                    Some((ctx, binding.0.clone()))
                } else {
                    None
                }
            })
        });
        if destination.is_none() && matches!(role, Role::Main) && values[1]["kind"] == "event" {
            return Ok(serde_json::Value::Null);
        }
        let (destination, target_api) = destination.context("Bridge target is unavailable")?;
        let mut message = args[1].take();
        message["sender"] = serde_json::to_value(role)?;
        let context = destination as Ptr;
        let name = target_api.string("__appsBridgeReceiveBytes");
        let mut error = ptr::null_mut();
        let receiver =
            unsafe { (target_api.get)(context, (target_api.global)(context), name, &mut error) };
        unsafe {
            (target_api.string_release)(name);
        }
        // WARNING: JSC casts call targets to objects without checking, so a primitive would crash.
        anyhow::ensure!(
            error.is_null()
                && unsafe { (target_api.value_type)(context, receiver) } == 5
                && unsafe { (target_api.is_function)(context, receiver) },
            "Bridge target is unavailable"
        );
        let argument = crate::jsc::payload::output(
            &target_api,
            context,
            crate::jsc::payload::Payload::snapshot(message),
        );
        unsafe {
            (target_api.protect)(context, argument);
        }
        let result = unsafe {
            (target_api.invoke)(context, receiver, ptr::null_mut(), 1, &argument, &mut error)
        };
        unsafe {
            (target_api.unprotect)(context, argument);
        }
        if !error.is_null() {
            bail!("{}", target_api.text(context, error));
        }
        anyhow::ensure!(
            !result.is_null() && unsafe { (target_api.value_type)(context, result) } == 5,
            "Invalid bridge reply"
        );
        unsafe {
            (target_api.protect)(context, result);
        }
        let decoded = (|| -> Result<_> {
            let name = target_api.string("json");
            let json = unsafe { (target_api.get)(context, result, name, &mut error) };
            unsafe {
                (target_api.string_release)(name);
            }
            let value = serde_json::from_str(&target_api.text(context, json))?;
            let name = target_api.string("buffers");
            let buffers = unsafe { (target_api.get)(context, result, name, &mut error) };
            unsafe {
                (target_api.string_release)(name);
            }
            let buffers = crate::jsc::payload::input(&target_api, context, buffers)?
                .iter()
                .map(|bytes| bytes::Bytes::copy_from_slice(bytes))
                .collect();
            Ok(crate::jsc::payload::Payload { value, buffers }.into_value())
        })();
        unsafe {
            (target_api.unprotect)(context, result);
        }
        return decoded;
    }
    if let Some(op) = op.strip_prefix("child.") {
        children
            .try_borrow_mut()
            .context("Child processes are busy")?
            .call(role, op, &args)
    } else if let Some(op) = op.strip_prefix("web.") {
        anyhow::ensure!(
            !op.starts_with("serve.") || matches!(role, Role::Main),
            "HTTP servers are only available in the main context"
        );
        anyhow::ensure!(
            !op.starts_with("tcp.") || !matches!(role, Role::Renderer(_)),
            "TCP sockets require a native context"
        );
        anyhow::ensure!(
            !matches!(role, Role::Renderer(_)) || op != "worker.start" || values[1] == false,
            "Page workers cannot request native APIs"
        );
        web.try_borrow_mut()
            .context("Web is busy")?
            .call(op, &mut args)
    } else if op.starts_with("sqlite.") {
        sqlite
            .try_borrow_mut()
            .context("SQLite is busy")?
            .call(role, op, &args)
    } else if let (Role::Renderer(id), Some(op)) = (role, op.strip_prefix("gpu.")) {
        gpu(binding, ctx, id, op, &serde_json::to_vec(&args)?)
    } else {
        host.try_borrow_mut()
            .context("Host is busy")?
            .call(role, op, args)
    }
}

fn gpu(binding: &Binding, ctx: Ptr, id: u64, op: &str, args: &[u8]) -> Result<serde_json::Value> {
    let mut host = binding.1.try_borrow_mut().context("Host is busy")?;
    let result = host.gpu(id, op, args);
    // WARNING: JSC cannot see GPU memory, so wrappers that own it would wait for an unrelated GC.
    if let Some(report) = binding.0.extra_memory
        && let Some(gpu) = host.graphics.get_mut(&id)
        && let bytes = gpu.take_allocated()
        && bytes > 0
    {
        drop(host);
        unsafe { report(ctx, bytes as usize) };
    }
    result
}

/// Makes an `ArrayBuffer` that owns `bytes`, or a `Uint8Array` over one.
fn buffer(api: &Api, ctx: Ptr, bytes: bytes::Bytes, typed: bool) -> Ptr {
    let bytes = Box::new(Vec::from(bytes));
    unsafe {
        let array = (api.make_buffer)(
            ctx,
            bytes.as_ptr().cast_mut().cast(),
            bytes.len(),
            Some(crate::jsc::payload::release),
            Box::into_raw(bytes).cast(),
            ptr::null_mut(),
        );
        if typed {
            (api.typed_from_buffer)(ctx, 3, array, ptr::null_mut())
        } else {
            array
        }
    }
}

/// Converts a host result to a JavaScript value. Small arrays of primitives skip JSON.
fn output(api: &Api, ctx: Ptr, payload: crate::jsc::payload::Payload) -> Ptr {
    use serde_json::Value;
    let crate::jsc::payload::Payload { value, mut buffers } = payload;
    unsafe {
        match value {
            Value::Null => (api.null)(ctx),
            Value::Bool(value) => (api.boolean)(ctx, value),
            Value::Number(value) => (api.number)(ctx, value.as_f64().unwrap_or(f64::NAN)),
            Value::String(text) => {
                let text = api.string_owned(text);
                let value = (api.make_string)(ctx, text);
                (api.string_release)(text);
                value
            }
            Value::Array(values)
                if values.len() <= 32
                    && values
                        .iter()
                        .all(|value| !value.is_array() && !value.is_object()) =>
            {
                // WARNING: the items stay on the stack, where JSC's conservative scan keeps them alive.
                let mut items = [ptr::null_mut(); 32];
                let count = values.len();
                for (item, value) in items.iter_mut().zip(values) {
                    *item = output(api, ctx, value.into());
                }
                (api.make_array)(ctx, count, items.as_ptr(), ptr::null_mut())
            }
            value => {
                if let Some(bytes) = value
                    .get("$appsBytes")
                    .and_then(Value::as_u64)
                    .and_then(|index| buffers.get_mut(index as usize))
                {
                    return buffer(api, ctx, std::mem::take(bytes), true);
                }
                let markers = !buffers.is_empty();
                let json = api.string_owned(value.to_string());
                let root = (api.json_value)(ctx, json);
                (api.string_release)(json);
                if !markers || root.is_null() {
                    return root;
                }
                (api.protect)(ctx, root);
                let mut arrays: Vec<Ptr> = vec![ptr::null_mut(); buffers.len()];
                let mut pending = vec![(&value, root)];
                while let Some((value, object)) = pending.pop() {
                    let children: Vec<(Ptr, Option<u32>, &Value)> = match value {
                        Value::Array(values) => values
                            .iter()
                            .enumerate()
                            .map(|(index, value)| (ptr::null_mut(), Some(index as u32), value))
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
                            if array.is_null() {
                                *array = buffer(api, ctx, std::mem::take(bytes), false);
                                (api.protect)(ctx, *array);
                            }
                            let view = (api.typed_from_buffer)(ctx, 3, *array, ptr::null_mut());
                            match index {
                                Some(index) => {
                                    (api.set_index)(ctx, object, index, view, ptr::null_mut())
                                }
                                None => (api.set)(ctx, object, key, view, 0, ptr::null_mut()),
                            }
                        } else if child.is_array() || child.is_object() {
                            let inner = match index {
                                Some(index) => (api.get_index)(ctx, object, index, ptr::null_mut()),
                                None => (api.get)(ctx, object, key, ptr::null_mut()),
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
                for array in arrays.into_iter().filter(|array| !array.is_null()) {
                    (api.unprotect)(ctx, array);
                }
                (api.unprotect)(ctx, root);
                root
            }
        }
    }
}
unsafe extern "C" fn now(
    ctx: Ptr,
    _function: Ptr,
    _this: Ptr,
    _count: usize,
    _args: *const Ptr,
    _exception: *mut Ptr,
) -> Ptr {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let api = HOSTS.with(|hosts| {
        hosts
            .borrow()
            .get(&(ctx as usize))
            .map(|binding| binding.0.clone())
    });
    let Some(api) = api else {
        return ptr::null_mut();
    };
    unsafe {
        (api.number)(
            ctx,
            START
                .get_or_init(std::time::Instant::now)
                .elapsed()
                .as_secs_f64()
                * 1000.,
        )
    }
}

unsafe extern "C" fn ffi_call(
    ctx: Ptr,
    _function: Ptr,
    _this: Ptr,
    count: usize,
    args: *const Ptr,
    exception: *mut Ptr,
) -> Ptr {
    let binding = HOSTS.with(|hosts| hosts.borrow().get(&(ctx as usize)).cloned());
    let Some((api, _, role, _, _, _, ffi, _, _)) = binding else {
        return ptr::null_mut();
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
        || -> Result<crate::node::ffi::Output> {
            anyhow::ensure!(count > 0, "Missing FFI request");
            let values = unsafe { std::slice::from_raw_parts(args, count) };
            let request = serde_json::from_str(&api.text(ctx, values[0]))?;
            ffi.call(&api, ctx, role, &request, &values[1..])
        },
    ));
    let value = match result {
        Ok(Ok(crate::node::ffi::Output::Object(value))) => return value,
        Ok(Ok(crate::node::ffi::Output::Json(value))) => serde_json::json!({"value":value}),
        Ok(Err(error)) => serde_json::json!({"error":error.to_string()}),
        Err(_) => serde_json::json!({"error":"FFI call failed"}),
    };
    let string = api.string(&value.to_string());
    unsafe {
        let result = (api.make_string)(ctx, string);
        (api.string_release)(string);
        if !exception.is_null() {
            *exception = ptr::null_mut();
        }
        result
    }
}

fn call_json<T>(
    api: &Api,
    ctx: Ptr,
    name: &str,
    arguments: &[&str],
    read: impl FnOnce(Ptr) -> T,
) -> Result<T> {
    let key = api.string(name);
    let mut exception = ptr::null_mut();
    let function = unsafe { (api.get)(ctx, (api.global)(ctx), key, &mut exception) };
    unsafe { (api.string_release)(key) };
    ensure!(
        exception.is_null()
            && unsafe { (api.value_type)(ctx, function) } == 5
            && unsafe { (api.is_function)(ctx, function) },
        "Missing JavaScript function: {name}"
    );
    let values: Vec<Ptr> = arguments
        .iter()
        .map(|argument| {
            let text = api.string(argument);
            let value = unsafe { (api.json_value)(ctx, text) };
            unsafe {
                (api.string_release)(text);
                // WARNING: values kept in a heap Vec are invisible to JSC's stack scan.
                (api.protect)(ctx, value);
            }
            value
        })
        .collect();
    let result = if values.iter().any(|value| value.is_null()) {
        Err(anyhow::anyhow!("Invalid JSON argument for {name}"))
    } else {
        let start = std::time::Instant::now();
        let result = unsafe {
            (api.invoke)(
                ctx,
                function,
                ptr::null_mut(),
                values.len(),
                values.as_ptr(),
                &mut exception,
            )
        };
        api.busy.set(api.busy.get() + start.elapsed());
        if exception.is_null() {
            Ok(read(result))
        } else {
            Err(anyhow::anyhow!("{}", api.text(ctx, exception)))
        }
    };
    for value in values.into_iter().filter(|value| !value.is_null()) {
        unsafe { (api.unprotect)(ctx, value) };
    }
    result
}

/// Stops page scripts that run too long; other contexts keep running because returning false re-arms the timer.
unsafe extern "C" fn page_limit(ctx: Ptr, _: Ptr) -> bool {
    let page = HOSTS.with(|hosts| {
        hosts.try_borrow().is_ok_and(|hosts| {
            hosts
                .get(&(ctx as usize))
                .is_some_and(|binding| matches!(binding.2, Role::Renderer(_)))
        })
    });
    if page {
        eprintln!("apps: stopped a page script that ran for more than {PAGE_LIMIT} seconds");
    }
    page
}

const PAGE_LIMIT: f64 = 10.;

struct Interrupt {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    group: Ptr,
    limit: unsafe extern "C" fn(Ptr, f64, Option<unsafe extern "C" fn(Ptr, Ptr) -> bool>, Ptr),
}

pub struct Engine {
    api: Rc<Api>,
    ctx: Ptr,
    napi: Option<Rc<crate::node::napi::State>>,
    interrupt: Option<Box<Interrupt>>,
    sources: RefCell<HashMap<String, String>>,
    devtools: bool,
    scope: Ptr,
}

impl Engine {
    pub fn new(api: Rc<Api>, host: Rc<RefCell<Host>>, role: Role) -> Result<Self> {
        Self::new_interruptible(api, host, role, None)
    }

    pub fn new_interruptible(
        api: Rc<Api>,
        host: Rc<RefCell<Host>>,
        role: Role,
        interrupt: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<Self> {
        let devtools = !host.borrow().packaged;
        let ctx = unsafe { (api.create)(api.group, ptr::null_mut()) };
        anyhow::ensure!(!ctx.is_null(), "Cannot create a JavaScriptCore context");
        unsafe {
            if let Some(inspectable) = api.inspectable {
                inspectable(ctx, devtools);
            }
            if let Some(set_name) = api.set_name {
                let name = api.string(&format!("apps {role:?}"));
                set_name(ctx, name);
                (api.string_release)(name);
            }
        }
        HOSTS.with(|hosts| {
            hosts.borrow_mut().insert(
                ctx as usize,
                (
                    api.clone(),
                    host.clone(),
                    role,
                    Rc::default(),
                    Rc::default(),
                    Rc::default(),
                    Rc::default(),
                    Rc::default(),
                    Rc::default(),
                ),
            )
        });
        HOSTS.with(|hosts| {
            hosts.borrow()[&(ctx as usize)].4.borrow_mut().renderer =
                matches!(role, Role::Renderer(_))
        });
        let napi = if matches!(role, Role::Renderer(_)) {
            None
        } else {
            Some(crate::node::napi::State::new(
                api.clone(),
                ctx,
                &host,
                false,
            )?)
        };
        let mut engine = Self {
            api,
            ctx,
            napi,
            interrupt: None,
            sources: RefCell::default(),
            devtools,
            scope: ptr::null_mut(),
        };
        if let Some(interrupt) = interrupt {
            engine.set_interrupt(interrupt)?;
        } else if matches!(role, Role::Renderer(_))
            && let Some(limit) = engine.api.execution_limit
        {
            // WARNING: the limit is per context group, which every engine of this thread shares.
            unsafe {
                limit(
                    engine.api.group,
                    PAGE_LIMIT,
                    Some(page_limit),
                    ptr::null_mut(),
                )
            };
        }
        let callback_api = engine.api.clone();
        HOSTS.with(|hosts| {
            hosts.borrow()[&(ctx as usize)]
                .3
                .borrow_mut()
                .set_authorizer_callback(Rc::new(move |id, action, args| {
                    call_json(
                        &callback_api,
                        ctx,
                        "__appsSqliteAuthorize",
                        &[
                            &id.to_string(),
                            &action.to_string(),
                            &serde_json::to_string(&args)?,
                        ],
                        |value| callback_api.text(ctx, value),
                    )?
                    .parse::<i32>()
                    .context("Authorizer must return a SQLite authorization code")
                }))
        });
        HOSTS.with(|hosts| {
            hosts.borrow()[&(ctx as usize)]
                .4
                .borrow_mut()
                .configure_storage(
                    &host.borrow(),
                    match role {
                        Role::Renderer(id) | Role::Preload(id) => Some(id),
                        Role::Main => None,
                    },
                )
        });
        for (name, callback) in [("__appsNative", call as Callback), ("__appsNow", now)] {
            let name = engine.api.string(name);
            unsafe {
                let callback = (engine.api.function)(ctx, name, callback);
                (engine.api.set)(
                    ctx,
                    (engine.api.global)(ctx),
                    name,
                    callback,
                    0,
                    ptr::null_mut(),
                );
                (engine.api.string_release)(name);
            }
        }
        if devtools || matches!(role, Role::Renderer(_)) {
            let name = engine.api.string("__appsCompile");
            unsafe {
                let callback = (engine.api.function)(ctx, name, compile);
                // WARNING: pages get the compiler only as `this` of their own scripts, see `eval_page`.
                let target = if matches!(role, Role::Renderer(_)) {
                    engine.scope = (engine.api.object_make)(ctx, ptr::null_mut(), ptr::null_mut());
                    (engine.api.protect)(ctx, engine.scope);
                    let flag = engine.api.string("__appsDevtools");
                    (engine.api.set)(
                        ctx,
                        engine.scope,
                        flag,
                        (engine.api.boolean)(ctx, devtools),
                        0,
                        ptr::null_mut(),
                    );
                    (engine.api.string_release)(flag);
                    engine.scope
                } else {
                    (engine.api.global)(ctx)
                };
                (engine.api.set)(ctx, target, name, callback, 0, ptr::null_mut());
                (engine.api.string_release)(name);
            }
        }
        let name = engine.api.string("__appsChunk");
        unsafe {
            let callback = (engine.api.function)(ctx, name, chunk);
            let target = if engine.scope.is_null() {
                (engine.api.global)(ctx)
            } else {
                engine.scope
            };
            (engine.api.set)(ctx, target, name, callback, 0, ptr::null_mut());
            (engine.api.string_release)(name);
        }
        // WARNING: bootstrap scripts call into the host, so the host is only borrowed per check.
        let enabled = |feature| host.borrow().enabled(feature);
        let disabled: Vec<_> = crate::bundle::FEATURES
            .iter()
            .map(|(name, _)| *name)
            .filter(|name| !enabled(name))
            .collect();
        engine.eval(
            &format!(
                "globalThis.__appsRole={};globalThis.__appsDisabled={};",
                serde_json::to_string(&role)?,
                serde_json::to_string(&disabled)?
            ),
            "apps:role",
        )?;
        if !matches!(role, Role::Renderer(_)) {
            let name = engine.api.string("__appsLoadAddon");
            unsafe {
                let callback = (engine.api.function)(ctx, name, crate::node::napi::load);
                (engine.api.set)(
                    ctx,
                    (engine.api.global)(ctx),
                    name,
                    callback,
                    0,
                    ptr::null_mut(),
                );
                (engine.api.string_release)(name);
            }
            let name = engine.api.string("__appsFFINative");
            unsafe {
                let callback = (engine.api.function)(ctx, name, ffi_call);
                (engine.api.set)(
                    ctx,
                    (engine.api.global)(ctx),
                    name,
                    callback,
                    0,
                    ptr::null_mut(),
                );
                (engine.api.string_release)(name);
            }
            engine.eval(include_str!("../../js/node/ffi.js"), "apps:ffi")?;
            engine.eval(include_str!("../../js/node/sqlite.js"), "apps:sqlite")?;
            engine.eval(include_str!("../../js/events.js"), "apps:events")?;
            engine.eval(
                include_str!("../../js/node/child_process.js"),
                "apps:child_process",
            )?;
            engine.eval(include_str!("../../js/web/serve.js"), "apps:serve")?;
            engine.eval(include_str!("../../js/node/index.js"), "apps:node")?;
            engine.eval(include_str!("../../js/node/stream.js"), "apps:node-streams")?;
            engine.eval(include_str!("../../js/node/io.js"), "apps:node-io")?;
            engine.eval(include_str!("../../js/node/http.js"), "apps:node-http")?;
            engine.eval(include_str!("../../js/node/net.js"), "apps:net")?;
            engine.eval(include_str!("../../js/node/pty.js"), "apps:pty")?;
        }
        engine.eval(include_str!("../../js/web/streams.js"), "apps:streams")?;
        engine.eval(include_str!("../../js/web/crypto.js"), "apps:crypto")?;
        engine.eval(include_str!("../../js/web/index.js"), "apps:web")?;
        engine.eval(include_str!("../../js/web/platform.js"), "apps:platform")?;
        engine.eval(include_str!("../../js/bridge.js"), "apps:bridge")?;
        engine.eval(include_str!("../../js/browser.js"), "apps:browser")?;
        engine.eval(include_str!("../../js/web/worker.js"), "apps:worker")?;
        engine.eval(
            include_str!("../../js/web/environment.js"),
            "apps:browser-environment",
        )?;
        engine.eval(include_str!("../../js/inspect.js"), "apps:inspect")?;
        engine.eval(include_str!("../../js/bootstrap.js"), "apps:bootstrap")?;
        engine.eval(include_str!("../../js/web/wasm.js"), "apps:wasm")?;
        engine.eval(
            include_str!("../../js/web/file_system.js"),
            "apps:file-system",
        )?;
        let name = engine.api.string("__appsRejection");
        unsafe {
            let callback = (engine.api.get)(ctx, (engine.api.global)(ctx), name, ptr::null_mut());
            (engine.api.rejection_callback)(ctx, callback, ptr::null_mut());
            (engine.api.string_release)(name);
        }
        engine.eval("delete globalThis.__appsRejection", "apps:rejections")?;
        if matches!(role, Role::Renderer(_)) {
            if enabled("webgpu") {
                engine.eval(include_str!("../../js/web/gpu.js"), "apps:gpu")?;
            }
            engine.eval(include_str!("../../js/dom.js"), "apps:dom")?;
            engine.eval(include_str!("../../js/lockdown.js"), "apps:lockdown")?;
        }
        Ok(engine)
    }

    pub fn eval(&self, source: &str, url: &str) -> Result<String> {
        self.evaluate(source, url, ptr::null_mut())
    }

    /// Runs a page script. Bundled scripts compile their modules through `this.__appsCompile`.
    pub fn eval_page(&self, source: &str, url: &str) -> Result<String> {
        struct Armed(usize);
        impl Drop for Armed {
            fn drop(&mut self) {
                ARMED.set(self.0);
            }
        }
        let _armed = Armed(ARMED.replace(self.ctx as usize));
        self.evaluate(source, url, self.scope)
    }

    /// Calls a global function with JSON arguments, without compiling a new script.
    pub fn call(&self, name: &str, arguments: &[&str]) -> Result<String> {
        call_json(&self.api, self.ctx, name, arguments, |value| {
            self.api.text(self.ctx, value)
        })
    }

    fn evaluate(&self, source: &str, url: &str, this: Ptr) -> Result<String> {
        if self.devtools
            && (matches!(url, "app.js" | "apps:page" | "apps:preload") || !url.starts_with("apps:"))
        {
            self.sources.borrow_mut().insert(url.into(), source.into());
        }
        let script = self.api.string(source);
        let source_url = self.api.string(url);
        let mut exception = ptr::null_mut();
        let start = std::time::Instant::now();
        let result =
            unsafe { (self.api.evaluate)(self.ctx, script, this, source_url, 1, &mut exception) };
        self.api.busy.set(self.api.busy.get() + start.elapsed());
        unsafe {
            (self.api.string_release)(script);
            (self.api.string_release)(source_url);
        }
        if !exception.is_null() {
            let message = self.api.text(self.ctx, exception);
            self.log(url, &message);
            bail!("{message}");
        }
        anyhow::ensure!(!result.is_null(), "JSC returned no value");
        Ok(self.api.text(self.ctx, result))
    }

    fn log(&self, url: &str, message: &str) {
        HOSTS.with(|hosts| {
            if let Some(binding) = hosts.borrow().get(&(self.ctx as usize))
                && let Ok(mut host) = binding.1.try_borrow_mut()
                && !host.packaged
            {
                if host.logs.len() == 500 {
                    host.logs.pop_front();
                }
                host.logs.push_back(
                    serde_json::json!({"type":"error","message":format!("{url}: {message}")}),
                );
            }
        });
    }

    pub fn sources(&self) -> HashMap<String, String> {
        if !self.devtools {
            return HashMap::new();
        }
        let mut sources = self.sources.borrow().clone();
        if let Ok(value) = self.eval(
            "JSON.stringify(Array.from(globalThis.__appsSources ?? []))",
            "apps:sources",
        ) && let Ok(originals) = serde_json::from_str::<Vec<(String, String)>>(&value)
        {
            sources.extend(originals);
        }
        sources
    }

    pub fn set_interrupt(
        &mut self,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<()> {
        unsafe extern "C" fn interrupted(_: Ptr, context: Ptr) -> bool {
            let state = unsafe { &*context.cast::<Interrupt>() };
            if state.stop.load(std::sync::atomic::Ordering::Acquire) {
                return true;
            }
            unsafe {
                (state.limit)(state.group, 0.01, Some(interrupted), context);
            }
            false
        }
        let limit = self
            .api
            .execution_limit
            .context("JavaScriptCore does not support interruptible workers")?;
        let mut state = Box::new(Interrupt {
            stop,
            group: unsafe { (self.api.context_group)(self.ctx) },
            limit,
        });
        unsafe {
            limit(
                state.group,
                0.01,
                Some(interrupted),
                (&mut *state as *mut Interrupt).cast(),
            );
        }
        self.interrupt = Some(state);
        Ok(())
    }

    /// Returns JSC heap statistics. The VM is shared, so they cover every context of this thread.
    pub(crate) fn heap(&self) -> Result<serde_json::Value> {
        let memory = self.api.memory.context("Missing JSGetMemoryUsageStatistics")?;
        unsafe {
            let json = (self.api.json_string)(self.ctx, memory(self.ctx), 0, ptr::null_mut());
            ensure!(!json.is_null(), "Cannot read heap statistics");
            let text = self.api.text(self.ctx, (self.api.make_string)(self.ctx, json));
            (self.api.string_release)(json);
            Ok(serde_json::from_str(&text)?)
        }
    }

    pub fn collect_garbage(&self) {
        unsafe {
            (self.api.garbage_collect)(self.ctx);
        }
    }

    pub fn tick(&self) -> Result<i64> {
        if let Some(napi) = &self.napi {
            napi.sweep();
        }
        let delay = call_json(&self.api, self.ctx, "__appsTick", &[], |value| unsafe {
            (self.api.to_number)(self.ctx, value, ptr::null_mut())
        })
        .inspect_err(|error| self.log("apps:tick", &error.to_string()))?;
        Ok(if delay.is_finite() { delay as i64 } else { -1 })
    }

    pub fn validate(&self, files: &BBTreeMap<String, String>) -> Result<()> {
        self.install(files)?;
        self.eval_page(&crate::bundle_source(files, false), "apps:validate")?;
        Ok(())
    }

    pub fn bundle(&self, files: &BBTreeMap<String, String>, entry: &str) -> Result<()> {
        self.install(files)?;
        self.eval_page(&crate::bundle_source(files, true), entry)
            .context("Cannot execute the app bundle")?;
        Ok(())
    }

    fn install(&self, files: &BBTreeMap<String, String>) -> Result<()> {
        let paths: HashMap<String, std::path::PathBuf> =
            serde_json::from_str(files.get("native.json").map_or("{}", String::as_str))?;
        let scripts: HashMap<String, std::sync::Arc<str>> = files
            .iter()
            .filter(|(name, _)| name.starts_with("script-"))
            .map(|(name, source)| (name.clone(), source.as_str().into()))
            .collect();
        HOSTS.with(|hosts| {
            let binding = &hosts.borrow()[&(self.ctx as usize)];
            let mut host = binding.1.borrow_mut();
            host.native_modules.extend(paths);
            if !scripts.is_empty() {
                host.scripts = std::sync::Arc::new(scripts);
                let window = match binding.2 {
                    Role::Renderer(id) | Role::Preload(id) => Some(id),
                    Role::Main => None,
                };
                binding.4.borrow_mut().configure_storage(&host, window);
            }
        });
        Ok(())
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if let Some(napi) = &self.napi {
            napi.close();
        }
        if !self.scope.is_null() {
            unsafe { (self.api.unprotect)(self.ctx, self.scope) };
        }
        let binding = HOSTS.with(|hosts| hosts.borrow_mut().remove(&(self.ctx as usize)));
        if let Some(binding) = binding {
            if let Role::Preload(id) = binding.2
                && let Ok(mut host) = binding.1.try_borrow_mut()
            {
                host.bridge.remove(&id);
            }
            let peers = HOSTS.with(|hosts| {
                hosts
                    .borrow()
                    .iter()
                    .filter(|(_, peer)| Rc::ptr_eq(&binding.1, &peer.1))
                    .map(|(&ctx, peer)| (ctx, peer.0.clone()))
                    .collect::<Vec<_>>()
            });
            for (ctx, api) in peers {
                let script = api.string(&format!(
                    "globalThis.__appsBridgeClosed?.({})",
                    serde_json::to_string(&binding.2).unwrap()
                ));
                unsafe {
                    (api.evaluate)(
                        ctx as Ptr,
                        script,
                        ptr::null_mut(),
                        ptr::null_mut(),
                        1,
                        ptr::null_mut(),
                    );
                    (api.string_release)(script);
                }
            }
        }
        if let Some(connection) =
            INSPECTED.with_borrow_mut(|inspected| inspected.remove(&(self.ctx as usize)))
            && let Some(inspector) = &self.api.inspector
        {
            // WARNING: a connected inspector keeps the global object alive.
            unsafe { (inspector.disconnect)(self.ctx, connection) };
        }
        unsafe {
            (self.api.release)(self.ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn contexts_isolate_privileges_and_share_dom_only() {
        let api = Api::load().unwrap();
        assert!(
            crate::node::ffi::State::default()
                .call(
                    &api,
                    ptr::null_mut(),
                    Role::Renderer(1),
                    &serde_json::json!({"op":"info"}),
                    &[]
                )
                .is_err()
        );
        let host = Rc::new(RefCell::new(Host::default()));
        host.borrow_mut().pages.insert(
            1,
            crate::host::Page {
                doc: crate::dom::Document::parse("<body><p id='value'>before</p></body>"),
                dirty: false,
            },
        );
        let main = Engine::new(api.clone(), host.clone(), Role::Main).unwrap();
        let preload = Engine::new(api.clone(), host.clone(), Role::Preload(1)).unwrap();
        let page = Engine::new(api, host.clone(), Role::Renderer(1)).unwrap();
        assert_eq!(main.eval("typeof process.cwd", "test").unwrap(), "function");
        assert_eq!(
            preload.eval("process.versions.apps", "test").unwrap(),
            crate::VERSION
        );
        assert_eq!(
            page.eval(
                "typeof process + ':' + typeof __apps + ':' + typeof __appsNative + ':' + typeof __appsFFINative + ':' + typeof __appsFFI",
                "test"
            )
            .unwrap(),
            "undefined:undefined:undefined:undefined:undefined"
        );
        page.eval(
            "document.querySelector('#value').textContent = 'after'",
            "test",
        )
        .unwrap();
        assert!(host.borrow().pages[&1].dirty);
        assert_eq!(
            page.eval("document.querySelector('#value').textContent", "test")
                .unwrap(),
            "after"
        );
        assert!(
            host.borrow_mut()
                .call(
                    Role::Renderer(1),
                    "fs.read",
                    serde_json::json!(["Cargo.toml", "utf8"])
                )
                .is_err()
        );
        assert!(
            host.borrow_mut()
                .call(Role::Preload(1), "window", serde_json::json!([]))
                .is_err()
        );
    }
    #[test]
    fn pages_cannot_turn_strings_into_code() {
        let host = Rc::new(RefCell::new(Host::default()));
        host.borrow_mut().pages.insert(
            1,
            crate::host::Page {
                doc: crate::dom::Document::parse("<body></body>"),
                dirty: false,
            },
        );
        let page = Engine::new(Api::load().unwrap(), host, Role::Renderer(1)).unwrap();
        for code in [
            "eval('1')",
            "globalThis['ev'+'al']('1')",
            "new Function('return 1')",
            "Function('return 1')",
            "(()=>{}).constructor('return 1')",
            "Object.getPrototypeOf(async function(){}).constructor('')",
            "Object.getPrototypeOf(function*(){}).constructor('')",
            "Object.getPrototypeOf(async function*(){}).constructor('')",
            "Reflect.construct(Function, ['return 1'])",
            "__appsCompile('1')",
            "this.__appsCompile('1')",
        ] {
            assert!(page.eval(code, "test").is_err(), "{code}");
        }
        assert_eq!(
            page.eval("(function(){}) instanceof Function && Function.prototype.constructor === Function && typeof Function", "test").unwrap(),
            "function"
        );
        page.eval("globalThis.imported='pending';import('data:text/javascript,globalThis.imported=1').then(()=>imported='ran',()=>imported='blocked');typeof ShadowRealm", "test").map(|kind| assert_eq!(kind, "undefined")).unwrap();
        page.tick().unwrap();
        assert_ne!(page.eval("imported", "test").unwrap(), "ran");
        assert_eq!(
            page.eval_page(
                "globalThis.later=this.__appsCompile;this.__appsCompile('20+22','page')",
                "page"
            )
            .unwrap(),
            "42"
        );
        assert!(page.eval("later('1','page')", "test").is_err());
        assert_eq!(page.eval_page("later('1','page')", "page").unwrap(), "1");
    }

    #[test]
    fn runaway_page_scripts_stop_but_main_keeps_running() {
        let api = Api::load().unwrap();
        let host = Rc::new(RefCell::new(Host::default()));
        host.borrow_mut().pages.insert(
            1,
            crate::host::Page {
                doc: crate::dom::Document::parse("<body></body>"),
                dirty: false,
            },
        );
        let main = Engine::new(api.clone(), host.clone(), Role::Main).unwrap();
        let page = Engine::new(api.clone(), host, Role::Renderer(1)).unwrap();
        unsafe {
            (api.execution_limit.unwrap())(api.group, 0.2, Some(page_limit), ptr::null_mut())
        };
        assert!(page.eval("while(true){}", "test").is_err());
        assert_eq!(page.eval("1+1", "test").unwrap(), "2");
        let started = std::time::Instant::now();
        assert_eq!(
            main.eval(
                "let n=0;const end=performance.now()+500;while(performance.now()<end)n++;n>0",
                "test"
            )
            .unwrap(),
            "true"
        );
        assert!(started.elapsed() >= std::time::Duration::from_millis(500));
    }

    #[test]
    fn timers_and_node_path() {
        let engine = Engine::new(
            Api::load().unwrap(),
            Rc::new(RefCell::new(Host::default())),
            Role::Main,
        )
        .unwrap();
        assert_eq!(
            engine
                .eval("__apps.path.posix.normalize('/a//b/../c/')", "test")
                .unwrap(),
            "/a/c/"
        );
        assert_eq!(
            engine
                .eval("__apps.path.posix.relative('/a/b', '/a/c')", "test")
                .unwrap(),
            "../c"
        );
        assert_eq!(
            engine
                .eval("__apps.path.win32.join('C:\\\\a','b','..','c')", "test")
                .unwrap(),
            "C:\\a\\c"
        );
        assert_eq!(
            engine
                .eval(
                    "globalThis.fired=0;setTimeout(()=>fired++,0);__appsTick();fired",
                    "test"
                )
                .unwrap(),
            "1"
        );
        assert_eq!(engine.eval("__appsTick()", "test").unwrap(), "-1");
    }
}
