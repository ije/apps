pub(crate) mod payload;

use crate::host::{Host, Role};
use anyhow::{Context, Result, bail};
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
    group_create: unsafe extern "C" fn() -> Ptr,
    group_release: unsafe extern "C" fn(Ptr),
    garbage_collect: unsafe extern "C" fn(Ptr),
    inspectable: Option<unsafe extern "C" fn(Ptr, bool)>,
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
    pub(crate) class_create: unsafe extern "C" fn(*const crate::node::napi::ClassDefinition) -> Ptr,
    pub(crate) class_release: unsafe extern "C" fn(Ptr),
    pub(crate) object_make: unsafe extern "C" fn(Ptr, Ptr, Ptr) -> Ptr,
    pub(crate) get_private: unsafe extern "C" fn(Ptr) -> Ptr,
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
    if let Some(path) = std::env::var_os("APPS_JSC_LIBRARY") {
        let path = PathBuf::from(path);
        anyhow::ensure!(
            path.is_file(),
            "APPS_JSC_LIBRARY needs a standalone library file: {}",
            path.display()
        );
        return Ok(path.canonicalize()?);
    }
    let name = library_name(platform)?;
    let executable = executable.canonicalize()?;
    let directory = executable
        .parent()
        .context("Missing executable directory")?;
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
            Ok(Rc::new(Self {
                inspectable: library
                    .get(b"JSGlobalContextSetInspectable\0")
                    .ok()
                    .map(|s| *s),
                set_name: library.get(b"JSGlobalContextSetName\0").ok().map(|s| *s),
                context_group: *library.get(b"JSContextGetGroup\0")?,
                execution_limit: library
                    .get(b"JSContextGroupSetExecutionTimeLimit\0")
                    .ok()
                    .map(|s| *s),
                group_create: *library.get(b"JSContextGroupCreate\0")?,
                group_release: *library.get(b"JSContextGroupRelease\0")?,
                garbage_collect: *library.get(b"JSSynchronousGarbageCollectForDebugging\0")?,
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
                typed_bytes: *library.get(b"JSObjectGetTypedArrayBytesPtr\0")?,
                typed_length: *library.get(b"JSObjectGetTypedArrayByteLength\0")?,
                typed_offset: *library.get(b"JSObjectGetTypedArrayByteOffset\0")?,
                make_buffer: *library.get(b"JSObjectMakeArrayBufferWithBytesNoCopy\0")?,
                invoke: *library.get(b"JSObjectCallAsFunction\0")?,
                protect: *library.get(b"JSValueProtect\0")?,
                unprotect: *library.get(b"JSValueUnprotect\0")?,
                class_create: *library.get(b"JSClassCreate\0")?,
                class_release: *library.get(b"JSClassRelease\0")?,
                object_make: *library.get(b"JSObjectMake\0")?,
                get_private: *library.get(b"JSObjectGetPrivate\0")?,
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
                _library: library,
            }))
        }
    }

    pub(crate) fn string(&self, text: &str) -> Ptr {
        if let Ok(text) = std::ffi::CString::new(text) {
            return unsafe { (self.string_create_utf8)(text.as_ptr()) };
        }
        let utf16: Vec<_> = text.encode_utf16().collect();
        unsafe { (self.string_create)(utf16.as_ptr(), utf16.len()) }
    }

    pub(crate) fn text(&self, ctx: Ptr, value: Ptr) -> String {
        unsafe {
            let string = (self.to_string)(ctx, value, ptr::null_mut());
            if string.is_null() {
                return "JavaScript exception".into();
            }
            let mut buffer = vec![0u8; (self.string_size)(string)];
            let size = (self.string_utf8)(string, buffer.as_mut_ptr().cast(), buffer.len());
            buffer.truncate(size.saturating_sub(1));
            let length = (self.string_length)(string);
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
thread_local! {static HOSTS:RefCell<HashMap<usize,Binding>>=RefCell::new(HashMap::new());}

unsafe extern "C" fn compile(
    ctx: Ptr,
    _function: Ptr,
    _this: Ptr,
    count: usize,
    args: *const Ptr,
    exception: *mut Ptr,
) -> Ptr {
    let api = HOSTS.with(|hosts| {
        hosts
            .borrow()
            .get(&(ctx as usize))
            .map(|binding| binding.0.clone())
    });
    let Some(api) = api else {
        return ptr::null_mut();
    };
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

unsafe extern "C" fn call(
    ctx: Ptr,
    _function: Ptr,
    _this: Ptr,
    count: usize,
    args: *const Ptr,
    exception: *mut Ptr,
) -> Ptr {
    let binding = HOSTS.with(|hosts| hosts.borrow().get(&(ctx as usize)).cloned());
    let Some((api, host, role, sqlite, web, children, _, node, pty)) = binding else {
        return ptr::null_mut();
    };
    let buffers = if count == 2 {
        crate::jsc::payload::input(&api, ctx, unsafe { *args.add(1) })
    } else {
        Ok(Vec::new())
    };
    let payload = crate::jsc::payload::Payload::capture(
        buffers.as_ref().cloned().unwrap_or_default(),
        || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || -> Result<serde_json::Value> {
                    anyhow::ensure!(
                        count == 1 || count == 2,
                        "Host calls need JSON and optional bytes"
                    );
                    if let Err(error) = &buffers {
                        bail!("{error}");
                    }
                    let input = api.text(ctx, unsafe { *args });
                    let mut request: serde_json::Value = serde_json::from_str(&input)?;
                    let op = request["op"].as_str().unwrap_or("");
                    let values = &request["args"];
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
                            .call(op, values);
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
                        let destination = HOSTS.with(|hosts| {
                            hosts.borrow().iter().find_map(|(&ctx, binding)| {
                                if Rc::ptr_eq(&host, &binding.1)
                                    && serde_json::to_value(binding.2).ok().as_ref() == Some(target)
                                {
                                    Some((ctx, binding.0.clone()))
                                } else {
                                    None
                                }
                            })
                        });
                        if destination.is_none()
                            && matches!(role, Role::Main)
                            && values[1]["kind"] == "event"
                        {
                            return Ok(serde_json::Value::Null);
                        }
                        let (destination, target_api) =
                            destination.context("Bridge target is unavailable")?;
                        let mut message = request["args"][1].take();
                        message["sender"] = serde_json::to_value(role)?;
                        let context = destination as Ptr;
                        let argument = crate::jsc::payload::output(
                            &target_api,
                            context,
                            crate::jsc::payload::Payload::snapshot(message),
                        );
                        unsafe {
                            (target_api.protect)(context, argument);
                        }
                        let name = target_api.string("__appsBridgeReceiveBytes");
                        let mut error = ptr::null_mut();
                        let receiver = unsafe {
                            (target_api.get)(
                                context,
                                (target_api.global)(context),
                                name,
                                &mut error,
                            )
                        };
                        unsafe {
                            (target_api.string_release)(name);
                        }
                        let result = unsafe {
                            (target_api.invoke)(
                                context,
                                receiver,
                                ptr::null_mut(),
                                1,
                                &argument,
                                &mut error,
                            )
                        };
                        unsafe {
                            (target_api.unprotect)(context, argument);
                        }
                        if !error.is_null() {
                            bail!("{}", target_api.text(context, error));
                        }
                        unsafe {
                            (target_api.protect)(context, result);
                        }
                        let decoded = (|| -> Result<_> {
                            let name = target_api.string("json");
                            let json =
                                unsafe { (target_api.get)(context, result, name, &mut error) };
                            unsafe {
                                (target_api.string_release)(name);
                            }
                            let value = serde_json::from_str(&target_api.text(context, json))?;
                            let name = target_api.string("buffers");
                            let buffers =
                                unsafe { (target_api.get)(context, result, name, &mut error) };
                            unsafe {
                                (target_api.string_release)(name);
                            }
                            let buffers =
                                crate::jsc::payload::input(&target_api, context, buffers)?;
                            Ok(crate::jsc::payload::Payload { value, buffers }.into_value())
                        })();
                        unsafe {
                            (target_api.unprotect)(context, result);
                        }
                        return decoded;
                    }
                    if let Some(op) = request["op"]
                        .as_str()
                        .and_then(|op| op.strip_prefix("child."))
                    {
                        children
                            .try_borrow_mut()
                            .context("Child processes are busy")?
                            .call(role, op, &request["args"])
                    } else if let Some(op) = request["op"]
                        .as_str()
                        .and_then(|op| op.strip_prefix("web."))
                    {
                        anyhow::ensure!(
                            !op.starts_with("serve.") || matches!(role, Role::Main),
                            "HTTP servers are only available in the main context"
                        );
                        anyhow::ensure!(
                            !op.starts_with("tcp.") || !matches!(role, Role::Renderer(_)),
                            "TCP sockets require a native context"
                        );
                        anyhow::ensure!(
                            !matches!(role, Role::Renderer(_))
                                || op != "worker.start"
                                || values[1] == false,
                            "Page workers cannot request native APIs"
                        );
                        web.try_borrow_mut()
                            .context("Web is busy")?
                            .call(op, &request["args"])
                    } else if request["op"]
                        .as_str()
                        .is_some_and(|op| op.starts_with("sqlite."))
                    {
                        sqlite
                            .try_borrow_mut()
                            .context("SQLite is busy")?
                            .call(role, &request)
                    } else {
                        host.try_borrow_mut()
                            .context("Host is busy")?
                            .call(role, request)
                    }
                },
            ));
            match result {
                Ok(Ok(value)) => serde_json::json!({"value":value}),
                Ok(Err(error)) => {
                    serde_json::json!({"error":error.to_string(),"code":error.downcast_ref::<std::io::Error>().map(crate::node::code)})
                }
                Err(_) => serde_json::json!({"error":"Host call failed"}),
            }
        },
    );
    if !exception.is_null() {
        unsafe {
            *exception = ptr::null_mut();
        }
    }
    crate::jsc::payload::output(&api, ctx, payload)
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
        let ctx = unsafe {
            let group = (api.group_create)();
            let ctx = (api.create)(group, ptr::null_mut());
            (api.group_release)(group);
            ctx
        };
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
        };
        if let Some(interrupt) = interrupt {
            engine.set_interrupt(interrupt)?;
        }
        let callback_api = engine.api.clone();
        HOSTS.with(|hosts| {
            hosts.borrow()[&(ctx as usize)]
                .3
                .borrow_mut()
                .set_authorizer_callback(Rc::new(move |id, action, args| {
                    let source = callback_api.string(&format!(
                        "__appsSqliteAuthorize({id},{action},{})",
                        serde_json::to_string(&args)?
                    ));
                    let mut exception = ptr::null_mut();
                    let result = unsafe {
                        (callback_api.evaluate)(
                            ctx,
                            source,
                            ptr::null_mut(),
                            ptr::null_mut(),
                            1,
                            &mut exception,
                        )
                    };
                    unsafe { (callback_api.string_release)(source) };
                    if !exception.is_null() {
                        bail!("{}", callback_api.text(ctx, exception));
                    }
                    callback_api
                        .text(ctx, result)
                        .parse::<i32>()
                        .context("Authorizer must return a SQLite authorization code")
                }))
        });
        HOSTS.with(|hosts| {
            hosts.borrow()[&(ctx as usize)]
                .4
                .borrow_mut()
                .configure_storage(
                    !devtools,
                    host.borrow().app_name.as_deref().unwrap_or("apps"),
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
        if devtools {
            let name = engine.api.string("__appsCompile");
            unsafe {
                let callback = (engine.api.function)(ctx, name, compile);
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
        engine.eval(
            &format!("globalThis.__appsRole={};", serde_json::to_string(&role)?),
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
            engine.eval(include_str!("../../js/web/gpu.js"), "apps:gpu")?;
            engine.eval(include_str!("../../js/dom.js"), "apps:dom")?;
        }
        Ok(engine)
    }

    pub fn eval(&self, source: &str, url: &str) -> Result<String> {
        if self.devtools
            && (matches!(url, "app.js" | "apps:page" | "apps:preload") || !url.starts_with("apps:"))
        {
            self.sources.borrow_mut().insert(url.into(), source.into());
        }
        let script = self.api.string(source);
        let source_url = self.api.string(url);
        let mut exception = ptr::null_mut();
        let result = unsafe {
            (self.api.evaluate)(
                self.ctx,
                script,
                ptr::null_mut(),
                source_url,
                1,
                &mut exception,
            )
        };
        unsafe {
            (self.api.string_release)(script);
            (self.api.string_release)(source_url);
        }
        if !exception.is_null() {
            let message = self.api.text(self.ctx, exception);
            HOSTS.with(|hosts| {
                if let Some(binding) = hosts.borrow().get(&(self.ctx as usize))
                    && let Ok(mut host) = binding.1.try_borrow_mut()
                {
                    if host.logs.len() == 500 {
                        host.logs.pop_front();
                    }
                    host.logs.push_back(
                        serde_json::json!({"type":"error","message":format!("{url}: {message}")}),
                    );
                }
            });
            bail!("{message}");
        }
        anyhow::ensure!(!result.is_null(), "JSC returned no value");
        Ok(self.api.text(self.ctx, result))
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

    pub fn collect_garbage(&self) {
        unsafe {
            (self.api.garbage_collect)(self.ctx);
        }
    }

    pub fn tick(&self) -> Result<i64> {
        if let Some(napi) = &self.napi {
            napi.sweep();
        }
        Ok(self
            .eval("__appsTick()", "apps:tick")?
            .parse()
            .unwrap_or(-1))
    }

    pub fn validate(&self, files: &BBTreeMap<String, String>) -> Result<()> {
        let source = crate::bundle_source(files, false)?;
        self.eval(&source, "apps:validate")?;
        Ok(())
    }

    pub fn bundle(&self, files: &BBTreeMap<String, String>, entry: &str) -> Result<()> {
        if let Some(source) = files.get("native.json") {
            let paths: HashMap<String, std::path::PathBuf> = serde_json::from_str(source)?;
            HOSTS.with(|hosts| {
                hosts.borrow()[&(self.ctx as usize)]
                    .1
                    .borrow_mut()
                    .native_modules
                    .extend(paths);
            });
        }
        let source = crate::bundle_source(files, true)?;
        self.eval(&source, entry)
            .context("Cannot execute the app bundle")?;
        Ok(())
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if let Some(napi) = &self.napi {
            napi.close();
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
                    serde_json::json!({"op":"fs.read","args":["Cargo.toml","utf8"]})
                )
                .is_err()
        );
        assert!(
            host.borrow_mut()
                .call(
                    Role::Preload(1),
                    serde_json::json!({"op":"window","args":[]})
                )
                .is_err()
        );
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
