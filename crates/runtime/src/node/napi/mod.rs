use crate::{
    host::Host,
    jsc::{JavaScriptCore, Ptr},
};
use anyhow::{Context, Result, ensure};
use calls::{Callback, Info};
use hooks::Hooks;
use libloading::Library;
use objects::{Finalize, Finalizer, Kind, Objects};
use scope::Handles;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    ffi::{CStr, c_char, c_void},
    ptr,
    rc::{Rc, Weak},
};
use work::Tasks;

macro_rules! api {
    ($name:ident($env:ident $(, $arg:ident: $ty:ty)* $(,)?) $body:block) => {
        #[unsafe(no_mangle)]
        #[allow(unused_unsafe)]
        pub(in crate::node::napi) unsafe extern "C" fn $name(raw: *const Self, $($arg: $ty),*) -> i32 {
            if raw.is_null() { return 1; }
            let $env = unsafe { &*raw };
            if $env.thread != std::thread::current().id() { return 23; }
            let status = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> crate::node::napi::Status { unsafe { $body } })).unwrap_or(Err(9)).err().unwrap_or(0);
            if stringify!($name) != "napi_get_last_error_info" { $env.error.set(crate::node::napi::ErrorInfo { message: crate::node::napi::MESSAGES.get(status as usize).copied().flatten().map_or(std::ptr::null(), std::ffi::CStr::as_ptr), reserved: std::ptr::null_mut(), engine_code: 0, code: status }); }
            status
        }
    };
}

// Addon threads call these without an environment. Access only thread-safe state.
macro_rules! raw_api {
    ($name:ident($($arg:ident: $ty:ty),* $(,)?) $body:block) => {
        #[unsafe(no_mangle)]
        unsafe extern "C" fn $name($($arg: $ty),*) -> i32 $body
    };
}

mod buffers;
mod calls;
mod hooks;
mod objects;
mod scope;
mod values;
mod work;

type Status = std::result::Result<(), i32>;
// The `napi_env` of the C API. Only the exports of `State` know what it points to.
type Env = *const c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct ErrorInfo {
    message: *const c_char,
    reserved: Ptr,
    engine_code: u32,
    code: i32,
}

#[derive(Clone, Copy)]
struct Function {
    callback: Callback,
    data: Ptr,
    id: u64,
}

pub(crate) struct State {
    api: Rc<JavaScriptCore>,
    ctx: Ptr,
    owner: Weak<State>,
    thread: std::thread::ThreadId,
    helpers: HashMap<String, Ptr>,
    handles: Handles,
    objects: Objects,
    hooks: Hooks,
    tasks: Tasks,
    instance: Cell<Ptr>,
    instance_finalizer: Cell<Option<Finalizer>>,
    pending: Cell<Ptr>,
    error: Cell<ErrorInfo>,
    external: Cell<i64>,
}
// The environment and callback of each native function. JSC functions carry no data, so the address finds them.
thread_local! { static FUNCTIONS: RefCell<HashMap<usize, (Weak<State>, Function)>> = RefCell::new(HashMap::new()); }
thread_local! { static CONTEXTS: RefCell<HashMap<usize, Weak<Root>>> = RefCell::new(HashMap::new()); }

pub(crate) struct Root {
    api: Rc<JavaScriptCore>,
    ctx: Ptr,
    host: Weak<RefCell<Host>>,
    environments: RefCell<Vec<Rc<State>>>,
    modules: RefCell<HashMap<std::path::PathBuf, Ptr>>,
}

impl State {
    fn new(api: Rc<JavaScriptCore>, ctx: Ptr) -> Result<Rc<Self>> {
        let mut helpers = HashMap::new();
        let source = api.string(js!("node/napi.js"));
        let mut exception = ptr::null_mut();
        let object = unsafe {
            (api.evaluate)(
                ctx,
                source,
                ptr::null_mut(),
                ptr::null_mut(),
                1,
                &mut exception,
            )
        };
        unsafe {
            (api.string_release)(source);
        }
        ensure!(
            exception.is_null(),
            "Cannot initialize Node-API: {}",
            api.text(ctx, exception)
        );
        unsafe {
            (api.protect)(ctx, object);
        }
        let key = api.string("keys");
        let names = unsafe {
            let keys = (api.get)(ctx, object, key, &mut exception);
            (api.string_release)(key);
            (api.invoke)(ctx, keys, ptr::null_mut(), 1, &object, &mut exception)
        };
        for name in api.text(ctx, names).split(',') {
            let key = api.string(name);
            let value = unsafe { (api.get)(ctx, object, key, &mut exception) };
            unsafe {
                (api.string_release)(key);
                (api.protect)(ctx, value);
            }
            helpers.insert(name.into(), value);
        }
        unsafe {
            (api.unprotect)(ctx, object);
        }
        let state = Rc::new_cyclic(|owner| Self {
            api,
            ctx,
            owner: owner.clone(),
            thread: std::thread::current().id(),
            helpers,
            handles: Handles::default(),
            objects: Objects::default(),
            hooks: Hooks::default(),
            tasks: Tasks::default(),
            instance: Cell::new(ptr::null_mut()),
            instance_finalizer: Cell::new(None),
            pending: Cell::new(ptr::null_mut()),
            error: Cell::new(ErrorInfo {
                message: ptr::null(),
                reserved: ptr::null_mut(),
                engine_code: 0,
                code: 0,
            }),
            external: Cell::new(0),
        });
        Ok(state)
    }

    // Runs the register function of an addon, and returns its exports.
    fn register(&self, register: Register, path: &std::path::Path) -> Result<Ptr> {
        let depth = self.handles.push(false);
        let exports = self
            .js("object", &[])
            .map_err(|_| anyhow::anyhow!("Cannot create addon exports"))?;
        let result = unsafe { register(self.env(), exports) };
        let result = if result.is_null() { exports } else { result };
        let exception = self.take_exception();
        let error = (!exception.is_null()).then(|| self.api.text(self.ctx, exception));
        self.hold(result);
        self.handles.unwind(&self.api, self.ctx, depth);
        if let Some(error) = error {
            anyhow::bail!("Cannot initialize {}: {error}", path.display());
        }
        Ok(result)
    }

    fn env(&self) -> Env {
        ptr::from_ref(self).cast()
    }

    fn hold(&self, value: Ptr) {
        self.handles.hold(&self.api, self.ctx, value);
    }

    fn keep(&self, value: Ptr) -> Ptr {
        self.handles.keep(&self.api, self.ctx, value)
    }

    fn track(
        &self,
        object: Ptr,
        kind: Kind,
        finalizer: Option<Finalizer>,
    ) -> std::result::Result<u64, i32> {
        let (id, holder) = self.objects.holder();
        let mut exception = ptr::null_mut();
        let holder = unsafe {
            (self.api.make_buffer)(
                self.ctx,
                (&raw mut (*holder).byte).cast(),
                1,
                Some(objects::release_holder),
                holder.cast(),
                &mut exception,
            )
        };
        check(!holder.is_null() && exception.is_null(), 9)?;
        self.keep(holder);
        self.js("holder", &[object, holder])?;
        self.objects.insert(id, object, kind, finalizer);
        Ok(id)
    }

    fn sweep(&self) {
        for (id, object, kind, finalizer) in self.objects.finalized() {
            if matches!(kind, Kind::Function) {
                FUNCTIONS.with(|functions| {
                    let mut functions = functions.borrow_mut();
                    if functions.get(&object).is_some_and(|(owner, function)| {
                        function.id == id && owner.ptr_eq(&self.owner)
                    }) {
                        functions.remove(&object);
                    }
                });
            }
            if let Some(finalizer) = finalizer {
                self.scoped(|| unsafe {
                    (finalizer.callback)(self.env(), finalizer.data, finalizer.hint)
                });
            }
        }
    }

    fn value(&self, result: *mut Ptr, value: Ptr) -> Status {
        check(!result.is_null(), 1)?;
        unsafe {
            *result = self.keep(value);
        }
        Ok(())
    }

    fn exception(&self, value: Ptr) {
        unsafe {
            (self.api.protect)(self.ctx, value);
        }
        let previous = self.pending.replace(value);
        if !previous.is_null() {
            unsafe {
                (self.api.unprotect)(self.ctx, previous);
            }
        }
    }

    fn take_exception(&self) -> Ptr {
        let value = self.pending.replace(ptr::null_mut());
        if !value.is_null() {
            self.keep(value);
            unsafe {
                (self.api.unprotect)(self.ctx, value);
            }
        }
        value
    }

    fn report(&self) {
        let exception = self.take_exception();
        if !exception.is_null() && self.js("report", &[exception]).is_err() {
            self.take_exception();
        }
    }

    fn js(&self, name: &str, args: &[Ptr]) -> std::result::Result<Ptr, i32> {
        check(args.iter().all(|arg| !arg.is_null()), 1)?;
        let helper = *self.helpers.get(name).ok_or(9)?;
        let mut exception = ptr::null_mut();
        let result = unsafe {
            (self.api.invoke)(
                self.ctx,
                helper,
                ptr::null_mut(),
                args.len(),
                args.as_ptr(),
                &mut exception,
            )
        };
        if !exception.is_null() {
            self.exception(exception);
            return Err(10);
        }
        Ok(self.keep(result))
    }

    // Direct property access for keys that need no ToPropertyKey. `set` is a sloppy [[Set]], like the helpers.
    unsafe fn access(
        &self,
        object: Ptr,
        name: Option<*const c_char>,
        index: u32,
        value: Option<Ptr>,
    ) -> std::result::Result<Ptr, i32> {
        check(value.is_none_or(|value| !value.is_null()), 1)?;
        let (api, ctx, mut exception) = (&self.api, self.ctx, ptr::null_mut());
        let key = name.map(|name| api.string(&unsafe { CStr::from_ptr(name) }.to_string_lossy()));
        let result = unsafe {
            match (key, value) {
                (Some(key), Some(value)) => {
                    (api.set)(ctx, object, key, value, 0, &mut exception);
                    ptr::null_mut()
                }
                (Some(key), None) => (api.get)(ctx, object, key, &mut exception),
                (None, Some(value)) => {
                    (api.set_index)(ctx, object, index, value, &mut exception);
                    ptr::null_mut()
                }
                (None, None) => (api.get_index)(ctx, object, index, &mut exception),
            }
        };
        if let Some(key) = key {
            unsafe { (api.string_release)(key) };
        }
        if !exception.is_null() {
            self.exception(exception);
            return Err(10);
        }
        Ok(self.keep(result))
    }

    fn truthy(&self, name: &str, args: &[Ptr]) -> std::result::Result<bool, i32> {
        let value = self.js(name, args)?;
        Ok(unsafe { (self.api.to_boolean)(self.ctx, value) })
    }

    fn error(&self, kind: &str, code: Ptr, message: Ptr) -> std::result::Result<Ptr, i32> {
        let code = if code.is_null() {
            self.undefined()
        } else {
            code
        };
        self.js("error", &[self.text(kind), code, message])
    }

    unsafe fn c_text(&self, value: *const c_char) -> Ptr {
        self.text(&unsafe { CStr::from_ptr(value) }.to_string_lossy())
    }

    fn text(&self, value: &str) -> Ptr {
        let string = self.api.string(value);
        let value = unsafe { (self.api.make_string)(self.ctx, string) };
        unsafe {
            (self.api.string_release)(string);
        }
        self.keep(value)
    }

    fn utf16(&self, units: &[u16]) -> Ptr {
        let string = unsafe { (self.api.string_create)(units.as_ptr(), units.len()) };
        let value = unsafe { (self.api.make_string)(self.ctx, string) };
        unsafe { (self.api.string_release)(string) };
        value
    }

    fn number(&self, value: f64) -> Ptr {
        unsafe { (self.api.number)(self.ctx, value) }
    }

    fn undefined(&self) -> Ptr {
        unsafe { (self.api.undefined)(self.ctx) }
    }

    fn boolean(&self, value: bool) -> Ptr {
        unsafe { (self.api.boolean)(self.ctx, value) }
    }

    fn kind(&self, value: Ptr) -> std::result::Result<u32, i32> {
        check(!value.is_null(), 1)?;
        Ok(unsafe {
            match (self.api.value_type)(self.ctx, value) {
                5 => {
                    if (self.api.is_function)(self.ctx, value) {
                        7
                    } else {
                        6
                    }
                }
                6 => 5,
                7 => 9,
                kind => kind,
            }
        })
    }

    fn object(&self, value: Ptr) -> Status {
        check(matches!(self.kind(value)?, 6 | 7), 2)
    }

    fn external(&self, value: Ptr) -> std::result::Result<Option<Ptr>, i32> {
        let Some((id, data)) = self.objects.external(value) else {
            return Ok(None);
        };
        let owner = self.js("external", &[value])?;
        Ok(
            (unsafe { (self.api.to_number)(self.ctx, owner, ptr::null_mut()) } == id as f64)
                .then_some(data),
        )
    }

    fn function(&self, name: &str, callback: Callback, data: Ptr) -> std::result::Result<Ptr, i32> {
        let string = self.api.string(name);
        let native = unsafe { (self.api.function)(self.ctx, string, Self::invoke) };
        unsafe {
            (self.api.string_release)(string);
        }
        self.keep(native);
        let id = self.track(native, Kind::Function, None)?;
        FUNCTIONS.with(|functions| {
            functions.borrow_mut().insert(
                native as usize,
                (self.owner.clone(), Function { callback, data, id }),
            )
        });
        self.js("function", &[native, self.text(name)])
    }

    unsafe extern "C" fn invoke(
        ctx: Ptr,
        function: Ptr,
        _: Ptr,
        count: usize,
        args: *const Ptr,
        exception: *mut Ptr,
    ) -> Ptr {
        let found =
            FUNCTIONS.with(|functions| functions.borrow().get(&(function as usize)).cloned());
        let Some((env, callback)) = found
            .and_then(|(owner, callback)| Some((owner.upgrade()?, callback)))
            .filter(|_| count >= 2)
        else {
            return ptr::null_mut();
        };
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let args = unsafe { std::slice::from_raw_parts(args, count) };
            let info = Info {
                this: args[0],
                target: if unsafe { (env.api.value_type)(ctx, args[1]) } == 0 {
                    ptr::null_mut()
                } else {
                    args[1]
                },
                args: &args[2..],
                data: callback.data,
            };
            let depth = env.handles.push(false);
            let value = unsafe { (callback.callback)(env.env(), &info) };
            unsafe {
                *exception = env.take_exception();
            }
            env.handles.unwind(&env.api, env.ctx, depth);
            if value.is_null() {
                env.undefined()
            } else {
                value
            }
        }));
        outcome.unwrap_or_else(|_| {
            unsafe {
                *exception = env.text("Node-API callback failed");
            }
            env.undefined()
        })
    }

    // Runs addon code with its own handle scope, and reports what it throws.
    fn scoped(&self, run: impl FnOnce()) {
        let depth = self.handles.push(false);
        run();
        self.report();
        self.handles.unwind(&self.api, self.ctx, depth);
    }

    fn close(&self) -> bool {
        // The hooks and finalizers below free addon state that work on the pool uses. Work that has not started
        // never starts, and running work gets a moment to end.
        if !self.tasks.cancel() {
            self.unregister();
            return false;
        }
        self.hooks.run();
        for function in self.tasks.take_live() {
            function.close();
            function.end(self.env(), &self.api, self.ctx);
        }
        self.tasks.clear_ready();
        // Wraps finalize first. A finalizer can remove other entries, which then do not run, or add new ones.
        loop {
            let ids = self.objects.closing();
            if ids.is_empty() {
                break;
            }
            for id in ids {
                if let Some(finalizer) = self.objects.take_finalizer(id) {
                    unsafe { (finalizer.callback)(self.env(), finalizer.data, finalizer.hint) };
                }
            }
        }
        if let Some(finalizer) = self.instance_finalizer.take() {
            unsafe { (finalizer.callback)(self.env(), finalizer.data, finalizer.hint) };
        }
        self.objects.drop_references(&self.api, self.ctx);
        self.handles.release(&self.api, self.ctx);
        for value in self.helpers.values() {
            unsafe {
                (self.api.unprotect)(self.ctx, *value);
            }
        }
        let pending = self.pending.replace(ptr::null_mut());
        if !pending.is_null() {
            unsafe {
                (self.api.unprotect)(self.ctx, pending);
            }
        }
        self.unregister();
        true
    }

    fn unregister(&self) {
        FUNCTIONS.with(|functions| {
            functions
                .borrow_mut()
                .retain(|_, (owner, _)| !owner.ptr_eq(&self.owner));
        });
    }

    fn drain(&self) {
        for item in self.tasks.take_ready() {
            match item {
                work::Ready::Work(work, status) => {
                    let (complete, data) = unsafe { (*(work as *const work::AsyncWork)).finish() };
                    if let Some(complete) = complete {
                        self.scoped(|| unsafe { complete(self.env(), status, data) });
                    }
                }
                work::Ready::Threadsafe(function) => self.run_threadsafe(&function),
            }
        }
    }

    fn run_threadsafe(&self, function: &work::Threadsafe) {
        // One function can have several entries here, so a later one may name a function that already ended.
        if function.ended() {
            return;
        }
        while let Some(data) = function.next() {
            self.scoped(|| {
                if let Some(target) = unsafe { function.call(self.env(), data) } {
                    let _ = self.js("call", &[target, self.undefined()]);
                }
            });
        }
        if function.released() {
            let live = self.tasks.remove_live(function);
            self.scoped(|| function.end(self.env(), &self.api, self.ctx));
            drop(live);
        }
    }
}

impl Root {
    pub(crate) fn new(api: Rc<JavaScriptCore>, ctx: Ptr, host: &Rc<RefCell<Host>>) -> Rc<Self> {
        let root = Rc::new(Self {
            api,
            ctx,
            host: Rc::downgrade(host),
            environments: RefCell::default(),
            modules: RefCell::default(),
        });
        CONTEXTS.with(|contexts| {
            contexts
                .borrow_mut()
                .insert(ctx as usize, Rc::downgrade(&root))
        });
        root
    }

    // Finalizers call into the addon and may run JavaScript, so only `tick` runs them, never a Node-API call.
    pub(crate) fn sweep(&self) {
        // Finalizers can load addons, which pushes to `environments`.
        let environments = self.environments.borrow().clone();
        for environment in environments {
            environment.sweep();
        }
    }

    pub(crate) fn drain(&self) {
        for environment in self.environments.borrow().clone() {
            environment.drain();
        }
    }

    pub(crate) fn pending(&self) -> bool {
        self.environments
            .borrow()
            .iter()
            .any(|environment| environment.tasks.pending())
    }

    pub(crate) fn close(&self) {
        let environments = self.environments.borrow().clone();
        for environment in environments.iter().rev() {
            // Addon work that still runs has the address of this environment, so it stays if it could not close.
            if !environment.close() {
                std::mem::forget(environment.clone());
            }
        }
        CONTEXTS.with(|contexts| contexts.borrow_mut().remove(&(self.ctx as usize)));
    }

    pub(crate) fn load(&self, name: &str) -> Result<Ptr> {
        let host = self.host.upgrade().context("Node-API context is closed")?;
        let path = host
            .borrow()
            .native_modules
            .get(name)
            .cloned()
            .with_context(|| format!("Native addon is not in the bundle: {name}"))?;
        let packed = path
            .to_str()
            .and_then(|key| host.borrow().scripts.get(key).cloned());
        // Replace cached addons by rename, never in place. Hash name and content so copies at different paths
        // remain separate modules.
        let path = match packed {
            Some(bytes) => {
                let home = std::path::PathBuf::from(
                    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                        .unwrap_or_default(),
                );
                let folder = if cfg!(target_os = "macos") {
                    home.join("Library/Caches")
                } else if cfg!(windows) {
                    std::env::var_os("LOCALAPPDATA").map_or(home, Into::into)
                } else {
                    std::env::var_os("XDG_CACHE_HOME")
                        .map_or_else(|| home.join(".cache"), Into::into)
                }
                .join(host.borrow().app_name.as_deref().unwrap_or("apps"));
                let hash = name
                    .bytes()
                    .chain([0])
                    .chain(bytes.iter().copied())
                    .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                        (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3)
                    });
                let file = folder.join(format!("{hash:016x}.node"));
                if std::fs::read(&file).ok().as_deref() != Some(&bytes[..]) {
                    let temporary = file.with_extension(format!(
                        "{}.{:?}.tmp",
                        std::process::id(),
                        std::thread::current().id()
                    ));
                    std::fs::create_dir_all(&folder)
                        .and_then(|()| std::fs::write(&temporary, &bytes))
                        .and_then(|()| std::fs::rename(&temporary, &file))
                        .with_context(|| format!("Cannot unpack native addon {name}"))?;
                }
                file
            }
            None => path,
        };
        let path = path
            .canonicalize()
            .with_context(|| format!("Cannot find native addon {}", path.display()))?;
        if let Some(value) = self.modules.borrow().get(&path) {
            return Ok(*value);
        }
        ensure!(
            path.extension().is_some_and(|ext| ext == "node"),
            "Expected a .node addon"
        );
        #[cfg(unix)]
        let library: Library = unsafe {
            libloading::os::unix::Library::open(
                Some(std::path::Path::new(&path)),
                libloading::os::unix::RTLD_NOW | libloading::os::unix::RTLD_LOCAL,
            )?
        }
        .into();
        #[cfg(not(unix))]
        let library = unsafe { Library::new(&path)? };
        // Older addons register from a static constructor, which ran when the library opened. Take it in
        // every case, or the next addon without one would run this addon's.
        let legacy = LEGACY_MODULE.take();
        let register = match unsafe { library.get::<Register>(b"napi_register_module_v1\0") } {
            Ok(register) => *register,
            Err(_) => legacy.context(
                "Addon must use Node-API (napi_register_module_v1); V8 addons are unsupported",
            )?,
        };
        if let Ok(version) = unsafe {
            library.get::<unsafe extern "C" fn() -> i32>(b"node_api_module_get_api_version_v1\0")
        } {
            let version = unsafe { version() };
            ensure!(
                (1..=8).contains(&version),
                "Unsupported Node-API version {version}; supported subset is based on version 8"
            );
        }
        let environment = State::new(self.api.clone(), self.ctx)?;
        self.environments.borrow_mut().push(environment.clone());
        // Like Node.js, never unload an addon. Its threads and static destructors may still run.
        std::mem::forget(library);
        let result = environment.register(register, &path)?;
        self.modules.borrow_mut().insert(path, result);
        Ok(result)
    }
}

fn check(condition: bool, status: i32) -> Status {
    if condition { Ok(()) } else { Err(status) }
}

unsafe fn chars<'a>(value: *const c_char, length: usize) -> &'a [u8] {
    if value.is_null() || length == 0 {
        &[]
    } else if length == usize::MAX {
        unsafe { CStr::from_ptr(value) }.to_bytes()
    } else {
        unsafe { std::slice::from_raw_parts(value.cast(), length) }
    }
}

const MESSAGES: [Option<&CStr>; 24] = [
    None,
    Some(c"Invalid argument"),
    Some(c"An object was expected"),
    Some(c"A string was expected"),
    Some(c"A string or symbol was expected"),
    Some(c"A function was expected"),
    Some(c"A number was expected"),
    Some(c"A boolean was expected"),
    Some(c"An array was expected"),
    Some(c"Unknown failure"),
    Some(c"An exception is pending"),
    Some(c"The async work item was cancelled"),
    Some(c"napi_escape_handle already called on scope"),
    Some(c"Invalid handle scope usage"),
    Some(c"Invalid callback scope usage"),
    Some(c"Thread-safe function queue is full"),
    Some(c"Thread-safe function handle is closing"),
    Some(c"A bigint was expected"),
    Some(c"A date was expected"),
    Some(c"An arraybuffer was expected"),
    Some(c"A detachable arraybuffer was expected"),
    Some(c"Main thread would deadlock"),
    Some(c"External buffers are not allowed"),
    Some(c"Cannot run JavaScript"),
];

impl State {
    api!(napi_set_instance_data(env, data: Ptr, callback: Option<Finalize>, hint: Ptr) {
        env.instance.set(data);
        env.instance_finalizer
            .set(callback.map(|callback| Finalizer { callback, data, hint }));
        Ok(())
    });
    api!(napi_get_instance_data(env, result: *mut Ptr) {
        check(!result.is_null(), 1)?;
        *result = env.instance.get();
        Ok(())
    });
    api!(napi_get_last_error_info(env, result: *mut *const ErrorInfo) {
        check(!result.is_null(), 1)?;
        *result = env.error.as_ptr();
        Ok(())
    });
    api!(napi_get_version(env, result: *mut u32) {
        check(!result.is_null(), 1)?;
        *result = 8;
        Ok(())
    });
    api!(napi_get_node_version(env, result: *mut *const NodeVersion) {
        check(!result.is_null(), 1)?;
        *result = &NODE_VERSION.0;
        Ok(())
    });
    api!(napi_get_uv_event_loop(env, result: *mut Ptr) {
        check(!result.is_null(), 1)?;
        // Fail without libuv: a fake loop pointer can crash the addon.
        *result = ptr::null_mut();
        Err(9)
    });
    api!(napi_adjust_external_memory(env, change: i64, result: *mut i64) {
        check(!result.is_null(), 1)?;
        // Report addon memory so GC can account for it. JSC does not accept decreases.
        if change > 0 {
            (env.api.extra_memory)(env.ctx, change as usize);
        }
        env.external.set(env.external.get().saturating_add(change).max(0));
        *result = env.external.get();
        Ok(())
    });
}

pub(crate) unsafe extern "C" fn load(
    ctx: Ptr,
    _: Ptr,
    _: Ptr,
    count: usize,
    args: *const Ptr,
    exception: *mut Ptr,
) -> Ptr {
    let state = CONTEXTS.with(|contexts| {
        contexts
            .borrow()
            .get(&(ctx as usize))
            .and_then(Weak::upgrade)
    });
    let Some(root) = state else {
        return ptr::null_mut();
    };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Ptr> {
        ensure!(count == 1, "Native addon loading needs one path");
        root.load(&root.api.text(ctx, unsafe { *args }))
    }));
    match outcome {
        Ok(Ok(value)) => value,
        result => {
            let message = match result {
                Ok(Err(error)) => format!("{error:#}"),
                _ => "Native addon loading failed".into(),
            };
            let string = root.api.string(&message);
            unsafe {
                *exception = (root.api.make_string)(ctx, string);
                (root.api.string_release)(string);
                (root.api.undefined)(ctx)
            }
        }
    }
}

raw_api!(napi_fatal_error(
    location: *const c_char,
    location_length: usize,
    message: *const c_char,
    message_length: usize,
) {
    eprintln!(
        "FATAL ERROR: {} {}",
        String::from_utf8_lossy(unsafe { chars(location, location_length) }),
        String::from_utf8_lossy(unsafe { chars(message, message_length) })
    );
    std::process::abort()
});

type Register = unsafe extern "C" fn(Env, Ptr) -> Ptr;

#[repr(C)]
struct LegacyModule {
    version: i32,
    flags: u32,
    filename: *const c_char,
    register: Option<Register>,
}
thread_local! { static LEGACY_MODULE: Cell<Option<Register>> = const { Cell::new(None) }; }

impl LegacyModule {
    raw_api!(napi_module_register(module: *const LegacyModule) {
        if !module.is_null() {
            LEGACY_MODULE.set(unsafe { (*module).register });
        }
        0
    });
}

#[repr(C)]
struct NodeVersion {
    major: u32,
    minor: u32,
    patch: u32,
    release: *const c_char,
}
struct Version(NodeVersion);
unsafe impl Sync for Version {}
static NODE_VERSION: Version = Version(NodeVersion {
    major: 24,
    minor: 0,
    patch: 0,
    release: c"node".as_ptr(),
});

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn callback(_: Env, _: *const Info) -> Ptr {
        ptr::null_mut()
    }

    #[test]
    fn reused_addresses_and_short_calls_are_rejected() {
        let api = JavaScriptCore::load().unwrap();
        let host = Rc::new(RefCell::new(Host::default()));
        let _engine =
            crate::jsc::JsContext::new(api.clone(), host.clone(), crate::host::Role::Main).unwrap();
        let root = CONTEXTS
            .with(|contexts| contexts.borrow().values().find_map(Weak::upgrade))
            .unwrap();
        let env = State::new(api, root.ctx).unwrap();
        let raw = Rc::as_ptr(&env);
        let object = env.js("object", &[]).unwrap();
        env.objects
            .add_external(object, u64::MAX, raw.cast_mut().cast());
        let (mut kind, mut data) = (0, ptr::null_mut());
        assert_eq!(unsafe { State::napi_typeof(raw, object, &mut kind) }, 0);
        assert_eq!(kind, 6, "A stale external address aliased a new object");
        assert_eq!(
            unsafe { State::napi_get_value_external(raw, object, &mut data) },
            1
        );
        env.function("live", callback, ptr::null_mut()).unwrap();
        let live = |native: usize| {
            FUNCTIONS.with(|functions| {
                functions
                    .borrow()
                    .get(&native)
                    .is_some_and(|(owner, _)| owner.ptr_eq(&env.owner))
            })
        };
        let native = FUNCTIONS.with(|functions| *functions.borrow().keys().next().unwrap());
        assert!(live(native));
        // A holder of another ID for the same address, freed by GC.
        let (id, holder) = env.objects.holder();
        env.objects.insert(id, native as Ptr, Kind::Function, None);
        unsafe { objects::release_holder(ptr::null_mut(), holder.cast()) };
        env.sweep();
        assert!(live(native), "A stale holder removed a live function");
        let mut exception = ptr::null_mut();
        let value = unsafe {
            State::invoke(
                env.ctx,
                native as Ptr,
                ptr::null_mut(),
                0,
                ptr::null(),
                &mut exception,
            )
        };
        assert!(value.is_null() && exception.is_null());
        env.close();
    }
}
