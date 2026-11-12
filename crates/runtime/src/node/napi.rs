use crate::{
    host::Host,
    jsc::{JavaScriptCore, Ptr},
};
use anyhow::{Context, Result, ensure};
use libloading::Library;
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet, VecDeque},
    ffi::{CStr, c_char},
    ptr,
    rc::{Rc, Weak},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
    },
};

type Status = std::result::Result<(), i32>;
type Callback = unsafe extern "C" fn(*const State, *const Info) -> Ptr;
type Finalize = unsafe extern "C" fn(*const State, Ptr, Ptr);
type Cleanup = unsafe extern "C" fn(Ptr);
type AsyncCleanupHook = unsafe extern "C" fn(Ptr, Ptr);

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
struct Info {
    this: Ptr,
    target: Ptr,
    args: *const [Ptr],
    data: Ptr,
}
struct Reference {
    value: Ptr,
    weak: Ptr,
    count: u32,
}
#[derive(Default)]
struct Scope {
    kept: HashSet<usize>,
    values: Vec<Ptr>,
    escaped: bool,
    escapable: bool,
}
#[derive(Clone, Copy)]
struct Finalizer {
    callback: Finalize,
    data: Ptr,
    hint: Ptr,
}
#[repr(C)]
struct Property {
    name: *const c_char,
    key: Ptr,
    method: Option<Callback>,
    getter: Option<Callback>,
    setter: Option<Callback>,
    value: Ptr,
    attributes: u32,
    data: Ptr,
}

struct Holder {
    queue: std::sync::Arc<std::sync::Mutex<Vec<u64>>>,
    id: u64,
    byte: u8,
}

unsafe extern "C" fn release_holder(_: Ptr, holder: Ptr) {
    // JSC frees array buffers at the end of a GC, maybe on a GC thread. Only queue the ID here.
    let holder = unsafe { Box::from_raw(holder.cast::<Holder>()) };
    if let Ok(mut queue) = holder.queue.lock() {
        queue.push(holder.id);
    }
}

pub(crate) struct State {
    api: Rc<JavaScriptCore>,
    ctx: Ptr,
    host: Weak<RefCell<Host>>,
    owner: Weak<State>,
    thread: std::thread::ThreadId,
    environments: RefCell<Vec<Rc<State>>>,
    wrap_finalizers: RefCell<HashMap<u64, Finalizer>>,
    helpers: HashMap<String, Ptr>,
    callbacks: RefCell<HashMap<usize, Function>>,
    // Scope addresses cross the C ABI and must remain stable.
    #[allow(clippy::vec_box)]
    scopes: RefCell<Vec<Box<Scope>>>,
    references: RefCell<HashMap<usize, Box<Reference>>>,
    // GC can reuse an object address before its holder is finalized, so address maps keep the holder ID.
    wraps: RefCell<HashMap<u64, Ptr>>,
    externals: RefCell<HashMap<usize, (u64, Ptr)>>,
    held: RefCell<Vec<Ptr>>,
    finalizers: RefCell<HashMap<u64, Finalizer>>,
    next_holder: Cell<u64>,
    holders: RefCell<HashMap<u64, (usize, u8)>>,
    finalized: std::sync::Arc<std::sync::Mutex<Vec<u64>>>,
    cleanup: RefCell<Vec<(Cleanup, Ptr)>>,
    async_cleanup: RefCell<Vec<usize>>,
    instance: Cell<Ptr>,
    instance_finalizer: Cell<Option<Finalizer>>,
    pending: Cell<Ptr>,
    error: Cell<ErrorInfo>,
    modules: RefCell<HashMap<std::path::PathBuf, Ptr>>,
    ready: Arc<Mutex<Vec<Ready>>>,
    active: Cell<usize>,
    // The states of the work that the pool has, or had.
    works: RefCell<Vec<Arc<AtomicU8>>>,
    threadsafe: Cell<usize>,
    threadsafe_live: RefCell<HashMap<usize, Arc<Threadsafe>>>,
    external: Cell<i64>,
    callback_scopes: Cell<usize>,
}
thread_local! { static FUNCTIONS: RefCell<HashMap<usize, Weak<State>>> = RefCell::new(HashMap::new()); }
thread_local! { static CONTEXTS: RefCell<HashMap<usize, Weak<State>>> = RefCell::new(HashMap::new()); }

impl State {
    pub(crate) fn new(
        api: Rc<JavaScriptCore>,
        ctx: Ptr,
        host: &Rc<RefCell<Host>>,
        addon: bool,
    ) -> Result<Rc<Self>> {
        let mut helpers = HashMap::new();
        if addon {
            let napi = js!("node/napi.js");
            let source = api.string(napi);
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
            for line in napi.lines() {
                if let Some((name, _)) = line
                    .strip_prefix("  ")
                    .and_then(|line| line.split_once(':'))
                    && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                {
                    let key = api.string(name);
                    let value = unsafe { (api.get)(ctx, object, key, &mut exception) };
                    unsafe {
                        (api.string_release)(key);
                        (api.protect)(ctx, value);
                    }
                    helpers.insert(name.into(), value);
                }
            }
            unsafe {
                (api.unprotect)(ctx, object);
            }
        }
        let state = Rc::new_cyclic(|owner| Self {
            api,
            ctx,
            host: Rc::downgrade(host),
            owner: owner.clone(),
            thread: std::thread::current().id(),
            environments: RefCell::default(),
            wrap_finalizers: RefCell::default(),
            helpers,
            callbacks: RefCell::default(),
            scopes: RefCell::default(),
            references: RefCell::default(),
            wraps: RefCell::default(),
            externals: RefCell::default(),
            held: RefCell::default(),
            finalizers: RefCell::default(),
            next_holder: Cell::new(0),
            holders: RefCell::default(),
            finalized: Default::default(),
            cleanup: RefCell::default(),
            async_cleanup: RefCell::default(),
            instance: Cell::new(ptr::null_mut()),
            instance_finalizer: Cell::new(None),
            pending: Cell::new(ptr::null_mut()),
            error: Cell::new(ErrorInfo {
                message: ptr::null(),
                reserved: ptr::null_mut(),
                engine_code: 0,
                code: 0,
            }),
            modules: RefCell::default(),
            ready: Default::default(),
            active: Cell::new(0),
            works: RefCell::default(),
            threadsafe: Cell::new(0),
            threadsafe_live: RefCell::default(),
            external: Cell::new(0),
            callback_scopes: Cell::new(0),
        });
        CONTEXTS.with(|contexts| {
            contexts
                .borrow_mut()
                .entry(ctx as usize)
                .or_insert_with(|| Rc::downgrade(&state));
        });
        Ok(state)
    }

    fn hold(&self, value: Ptr) {
        unsafe {
            (self.api.protect)(self.ctx, value);
        }
        self.held.borrow_mut().push(value);
    }

    fn keep(&self, value: Ptr) -> Ptr {
        if !value.is_null() {
            if let Some(scope) = self.scopes.borrow_mut().last_mut() {
                if !scope.kept.insert(value as usize) {
                    return value;
                }
                scope.values.push(value);
            } else {
                self.held.borrow_mut().push(value);
            }
            unsafe {
                (self.api.protect)(self.ctx, value);
            }
        }
        value
    }

    fn track(
        &self,
        object: Ptr,
        kind: u8,
        finalizer: Option<Finalizer>,
    ) -> std::result::Result<u64, i32> {
        let (id, holder) = self.holder();
        let mut exception = ptr::null_mut();
        let holder = unsafe {
            (self.api.make_buffer)(
                self.ctx,
                (&raw mut (*holder).byte).cast(),
                1,
                Some(release_holder),
                holder.cast(),
                &mut exception,
            )
        };
        check(!holder.is_null() && exception.is_null(), 9)?;
        self.keep(holder);
        self.js("holder", &[object, holder])?;
        self.holders
            .borrow_mut()
            .insert(id, (object as usize, kind));
        if let Some(finalizer) = finalizer {
            self.finalizers.borrow_mut().insert(id, finalizer);
        }
        Ok(id)
    }

    fn holder(&self) -> (u64, *mut Holder) {
        let id = self.next_holder.get() + 1;
        self.next_holder.set(id);
        let holder = Holder {
            queue: self.finalized.clone(),
            id,
            byte: 0,
        };
        (id, Box::into_raw(Box::new(holder)))
    }

    // Finalizers call into the addon and may run JavaScript, so only `tick` runs them, never a Node-API call.
    pub(crate) fn sweep(&self) {
        // Finalizers can load addons, which pushes to `environments`.
        let environments = self.environments.borrow().clone();
        for environment in &environments {
            environment.sweep();
        }
        let ids = self
            .finalized
            .lock()
            .map(|mut queue| std::mem::take(&mut *queue))
            .unwrap_or_default();
        for id in ids {
            let entry = self.holders.borrow_mut().remove(&id);
            let mut finalizer = self.finalizers.borrow_mut().remove(&id);
            if let Some((object, kind)) = entry {
                match kind {
                    1 => {
                        self.wraps.borrow_mut().remove(&id);
                        finalizer = self.wrap_finalizers.borrow_mut().remove(&id);
                    }
                    2 => {
                        let mut externals = self.externals.borrow_mut();
                        if externals
                            .get(&object)
                            .is_some_and(|external| external.0 == id)
                        {
                            externals.remove(&object);
                        }
                    }
                    3 => {
                        let mut callbacks = self.callbacks.borrow_mut();
                        if callbacks
                            .get(&object)
                            .is_some_and(|function| function.id == id)
                        {
                            callbacks.remove(&object);
                            FUNCTIONS.with(|functions| {
                                let mut functions = functions.borrow_mut();
                                if functions
                                    .get(&object)
                                    .is_some_and(|owner| owner.ptr_eq(&self.owner))
                                {
                                    functions.remove(&object);
                                }
                            });
                        }
                    }
                    _ => {}
                }
            }
            if let Some(finalizer) = finalizer {
                self.scoped(|| unsafe {
                    (finalizer.callback)(self, finalizer.data, finalizer.hint)
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

    fn external(&self, value: Ptr) -> std::result::Result<Option<Ptr>, i32> {
        let entry = self.externals.borrow().get(&(value as usize)).copied();
        let Some((id, data)) = entry else {
            return Ok(None);
        };
        let owner = self.js("external", &[value])?;
        Ok(
            (unsafe { (self.api.to_number)(self.ctx, owner, ptr::null_mut()) } == id as f64)
                .then_some(data),
        )
    }

    fn text(&self, value: &str) -> Ptr {
        let string = self.api.string(value);
        let value = unsafe { (self.api.make_string)(self.ctx, string) };
        unsafe {
            (self.api.string_release)(string);
        }
        self.keep(value)
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

    fn pop(&self) {
        if let Some(scope) = self.scopes.borrow_mut().pop() {
            for value in scope.values {
                unsafe {
                    (self.api.unprotect)(self.ctx, value);
                }
            }
        }
    }

    fn function(&self, name: &str, callback: Callback, data: Ptr) -> std::result::Result<Ptr, i32> {
        let string = self.api.string(name);
        let native = unsafe { (self.api.function)(self.ctx, string, invoke) };
        unsafe {
            (self.api.string_release)(string);
        }
        self.keep(native);
        let id = self.track(native, 3, None)?;
        FUNCTIONS.with(|functions| {
            functions
                .borrow_mut()
                .insert(native as usize, self.owner.clone())
        });
        self.callbacks
            .borrow_mut()
            .insert(native as usize, Function { callback, data, id });
        self.js("function", &[native, self.text(name)])
    }

    // Returns false if addon work still runs on the pool. Nothing of the addon is freed then, and the caller must
    // keep this state alive, because the work has its address.
    pub(crate) fn close(&self) -> bool {
        let environments = self.environments.borrow().clone();
        for environment in environments.iter().rev() {
            if !environment.close() {
                std::mem::forget(environment.clone());
            }
        }
        // The hooks and finalizers below free addon state that work on the pool uses. Work that has not started
        // never starts, and running work gets a moment to end.
        let works = self.works.take();
        for state in &works {
            let _ = state.compare_exchange(QUEUED, CANCELLED, Ordering::AcqRel, Ordering::Acquire);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while works
            .iter()
            .any(|state| state.load(Ordering::Acquire) == RUNNING)
        {
            if std::time::Instant::now() >= deadline {
                self.unregister();
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        loop {
            let hook = self.async_cleanup.borrow_mut().pop();
            let Some(handle) = hook else { break };
            let cleanup = handle as *const AsyncCleanup;
            unsafe { ((*cleanup).hook)(handle as Ptr, (*cleanup).arg) };
        }
        loop {
            let hook = self.cleanup.borrow_mut().pop();
            let Some((callback, data)) = hook else { break };
            unsafe {
                callback(data);
            }
        }
        let live = std::mem::take(&mut *self.threadsafe_live.borrow_mut());
        for function in live.values() {
            function.close();
            self.end_threadsafe(function);
        }
        self.ready
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        loop {
            let key = self.wrap_finalizers.borrow().keys().next().copied();
            let Some(key) = key else { break };
            let finalizer = self.wrap_finalizers.borrow_mut().remove(&key).unwrap();
            unsafe {
                (finalizer.callback)(self, finalizer.data, finalizer.hint);
            }
        }
        loop {
            let key = self.finalizers.borrow().keys().next().copied();
            let finalizer = key.and_then(|key| self.finalizers.borrow_mut().remove(&key));
            let Some(finalizer) = finalizer else { break };
            unsafe {
                (finalizer.callback)(self, finalizer.data, finalizer.hint);
            }
        }
        if let Some(finalizer) = self.instance_finalizer.take() {
            unsafe {
                (finalizer.callback)(self, finalizer.data, finalizer.hint);
            }
        }
        for (_, reference) in self.references.borrow_mut().drain() {
            unsafe {
                (self.api.unprotect)(self.ctx, reference.weak);
                if reference.count > 0 {
                    (self.api.unprotect)(self.ctx, reference.value);
                }
            }
        }
        while !self.scopes.borrow().is_empty() {
            self.pop();
        }
        for value in self
            .held
            .borrow_mut()
            .drain(..)
            .chain(self.helpers.values().copied())
        {
            unsafe {
                (self.api.unprotect)(self.ctx, value);
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
            let mut functions = functions.borrow_mut();
            for key in self.callbacks.borrow().keys() {
                if functions
                    .get(key)
                    .is_some_and(|owner| owner.ptr_eq(&self.owner))
                {
                    functions.remove(key);
                }
            }
        });
        CONTEXTS.with(|contexts| {
            let mut contexts = contexts.borrow_mut();
            if contexts
                .get(&(self.ctx as usize))
                .is_some_and(|owner| owner.ptr_eq(&self.owner))
            {
                contexts.remove(&(self.ctx as usize));
            }
        });
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
        let register = match unsafe {
            library
                .get::<unsafe extern "C" fn(*const State, Ptr) -> Ptr>(b"napi_register_module_v1\0")
        } {
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
        let environment = State::new(self.api.clone(), self.ctx, &host, true)?;
        self.environments.borrow_mut().push(environment.clone());
        // Like Node.js, never unload an addon. Its threads and static destructors may still run.
        std::mem::forget(library);
        environment.scopes.borrow_mut().push(Box::default());
        let exports = environment
            .js("object", &[])
            .map_err(|_| anyhow::anyhow!("Cannot create addon exports"))?;
        let result = unsafe { register(Rc::as_ptr(&environment), exports) };
        let result = if result.is_null() { exports } else { result };
        let exception = environment.take_exception();
        let error =
            (!exception.is_null()).then(|| environment.api.text(environment.ctx, exception));
        environment.hold(result);
        environment.pop();
        if let Some(error) = error {
            anyhow::bail!("Cannot initialize {}: {error}", path.display());
        }
        self.modules.borrow_mut().insert(path, result);
        Ok(result)
    }
}

fn check(condition: bool, status: i32) -> Status {
    if condition { Ok(()) } else { Err(status) }
}

unsafe extern "C" fn invoke(
    ctx: Ptr,
    function: Ptr,
    _: Ptr,
    count: usize,
    args: *const Ptr,
    exception: *mut Ptr,
) -> Ptr {
    let state = FUNCTIONS.with(|contexts| {
        contexts
            .borrow()
            .get(&(function as usize))
            .and_then(Weak::upgrade)
    });
    let Some(env) = state.filter(|_| count >= 2) else {
        return ptr::null_mut();
    };
    let callback = env.callbacks.borrow().get(&(function as usize)).copied();
    let Some(callback) = callback else {
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
        let depth = env.scopes.borrow().len();
        env.scopes.borrow_mut().push(Box::default());
        let value = unsafe { (callback.callback)(Rc::as_ptr(&env), &info) };
        unsafe {
            *exception = env.take_exception();
        }
        while env.scopes.borrow().len() > depth {
            env.pop();
        }
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

macro_rules! api {
    ($name:ident($env:ident $(, $arg:ident: $ty:ty)* $(,)?) $body:block) => {
        #[unsafe(no_mangle)]
        #[allow(unused_unsafe)]
        unsafe extern "C" fn $name(raw: *const State, $($arg: $ty),*) -> i32 {
            if raw.is_null() { return 1; }
            let $env = unsafe { &*raw };
            if $env.thread != std::thread::current().id() { return 23; }
            let status = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Status { unsafe { $body } })).unwrap_or(Err(9)).err().unwrap_or(0);
            if stringify!($name) != "napi_get_last_error_info" { $env.error.set(ErrorInfo { message: MESSAGES.get(status as usize).copied().flatten().map_or(ptr::null(), CStr::as_ptr), reserved: ptr::null_mut(), engine_code: 0, code: status }); }
            status
        }
    };
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
api!(napi_get_undefined(env, result: *mut Ptr) {
    env.value(result, env.undefined())
});
api!(napi_get_null(env, result: *mut Ptr) {
    env.value(result, (env.api.null)(env.ctx))
});
api!(napi_get_global(env, result: *mut Ptr) {
    env.value(result, (env.api.global)(env.ctx))
});
api!(napi_get_boolean(env, value: bool, result: *mut Ptr) {
    env.value(result, env.boolean(value))
});
api!(napi_create_object(env, result: *mut Ptr) {
    env.value(result, env.js("object", &[])?)
});
api!(napi_create_array(env, result: *mut Ptr) {
    env.value(result, env.js("array", &[env.number(0.)])?)
});
api!(napi_create_array_with_length(env, length: usize, result: *mut Ptr) {
    env.value(result, env.js("array", &[env.number(length as f64)])?)
});
api!(napi_typeof(env, value: Ptr, result: *mut u32) {
    check(!result.is_null(), 1)?;
    *result = if env.external(value)?.is_some() {
        8
    } else {
        env.kind(value)?
    };
    Ok(())
});
api!(napi_get_value_bool(env, value: Ptr, result: *mut bool) {
    check(!result.is_null(), 1)?;
    check(env.kind(value)? == 2, 7)?;
    *result = (env.api.to_boolean)(env.ctx, value);
    Ok(())
});
api!(napi_create_symbol(env, description: Ptr, result: *mut Ptr) {
    env.value(
        result,
        env.js(
            "symbol",
            &[if description.is_null() {
                env.undefined()
            } else {
                description
            }],
        )?,
    )
});
api!(napi_create_double(env, value: f64, result: *mut Ptr) {
    env.value(result, env.number(value as f64))
});
api!(napi_get_value_double(env, value: Ptr, result: *mut f64) {
    check(!result.is_null(), 1)?;
    check(env.kind(value)? == 3, 6)?;
    let number = (env.api.to_number)(env.ctx, value, ptr::null_mut());
    *result = number;
    Ok(())
});
api!(napi_create_int32(env, value: i32, result: *mut Ptr) {
    env.value(result, env.number(value as f64))
});
api!(napi_get_value_int32(env, value: Ptr, result: *mut i32) {
    check(!result.is_null(), 1)?;
    check(env.kind(value)? == 3, 6)?;
    let number = (env.api.to_number)(env.ctx, value, ptr::null_mut());
    *result = number.trunc().rem_euclid(4294967296.) as u32 as i32;
    Ok(())
});
api!(napi_create_uint32(env, value: u32, result: *mut Ptr) {
    env.value(result, env.number(value as f64))
});
api!(napi_get_value_uint32(env, value: Ptr, result: *mut u32) {
    check(!result.is_null(), 1)?;
    check(env.kind(value)? == 3, 6)?;
    let number = (env.api.to_number)(env.ctx, value, ptr::null_mut());
    *result = number.trunc().rem_euclid(4294967296.) as u32;
    Ok(())
});
api!(napi_create_int64(env, value: i64, result: *mut Ptr) {
    env.value(result, env.number(value as f64))
});
api!(napi_get_value_int64(env, value: Ptr, result: *mut i64) {
    check(!result.is_null(), 1)?;
    check(env.kind(value)? == 3, 6)?;
    let number = (env.api.to_number)(env.ctx, value, ptr::null_mut());
    *result = if number.is_finite() { number as i64 } else { 0 };
    Ok(())
});
api!(napi_create_string_utf8(
    env,
    value: *const c_char,
    length: usize,
    result: *mut Ptr,
) {
    check(!value.is_null() || length == 0, 1)?;
    let length = if length == usize::MAX {
        CStr::from_ptr(value).to_bytes().len()
    } else {
        length
    };
    let bytes = if length == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(value.cast::<u8>(), length)
    };
    env.value(result, env.text(&String::from_utf8_lossy(bytes)))
});
api!(napi_get_value_string_utf8(
    env,
    value: Ptr,
    buffer: *mut c_char,
    size: usize,
    result: *mut usize,
) {
    check(!buffer.is_null() || !result.is_null(), 1)?;
    check(env.kind(value)? == 4, 3)?;
    let text = env.api.text(env.ctx, value);
    let mut count = text.len().min(size.saturating_sub(1));
    while !text.is_char_boundary(count) {
        count -= 1;
    }
    if !buffer.is_null() && size > 0 {
        ptr::copy_nonoverlapping(text.as_ptr(), buffer.cast::<u8>(), count);
        *buffer.add(count) = 0;
    }
    if !result.is_null() {
        *result = if buffer.is_null() {
            text.len()
        } else if size == 0 {
            0
        } else {
            count
        };
    }
    Ok(())
});
api!(napi_create_string_latin1(
    env,
    value: *const c_char,
    length: usize,
    result: *mut Ptr,
) {
    check(!value.is_null() || length == 0, 1)?;
    let length = if length == usize::MAX {
        CStr::from_ptr(value).to_bytes().len()
    } else {
        length
    };
    let bytes = if length == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(value.cast::<u8>(), length)
    };
    env.value(result, {
        let chars: Vec<u16> = bytes.iter().map(|b| *b as u16).collect();
        let string = (env.api.string_create)(chars.as_ptr(), chars.len());
        let value = (env.api.make_string)(env.ctx, string);
        (env.api.string_release)(string);
        value
    })
});
api!(napi_get_value_string_latin1(
    env,
    value: Ptr,
    buffer: *mut c_char,
    size: usize,
    result: *mut usize,
) {
    check(!buffer.is_null() || !result.is_null(), 1)?;
    check(env.kind(value)? == 4, 3)?;
    let string = (env.api.to_string)(env.ctx, value, ptr::null_mut());
    let length = (env.api.string_length)(string);
    let chars = (env.api.string_chars)(string);
    let count = length.min(size.saturating_sub(1));
    if !buffer.is_null() && size > 0 {
        for index in 0..count {
            *buffer.add(index) = *chars.add(index) as c_char;
        }
        *buffer.add(count) = 0;
    }
    (env.api.string_release)(string);
    if !result.is_null() {
        *result = if buffer.is_null() {
            length
        } else if size == 0 {
            0
        } else {
            count
        };
    }
    Ok(())
});
api!(napi_create_string_utf16(
    env,
    value: *const u16,
    length: usize,
    result: *mut Ptr,
) {
    check(!value.is_null() || length == 0, 1)?;
    let length = if length == usize::MAX {
        {
            let mut size = 0;
            while *value.add(size) != 0 {
                size += 1;
            }
            size
        }
    } else {
        length
    };
    let bytes = if length == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(value.cast::<u16>(), length)
    };
    env.value(result, {
        let string = (env.api.string_create)(bytes.as_ptr(), bytes.len());
        let value = (env.api.make_string)(env.ctx, string);
        (env.api.string_release)(string);
        value
    })
});
api!(napi_get_value_string_utf16(
    env,
    value: Ptr,
    buffer: *mut u16,
    size: usize,
    result: *mut usize,
) {
    check(!buffer.is_null() || !result.is_null(), 1)?;
    check(env.kind(value)? == 4, 3)?;
    let string = (env.api.to_string)(env.ctx, value, ptr::null_mut());
    let length = (env.api.string_length)(string);
    let chars = (env.api.string_chars)(string);
    let count = length.min(size.saturating_sub(1));
    if !buffer.is_null() && size > 0 {
        for index in 0..count {
            *buffer.add(index) = *chars.add(index) as u16;
        }
        *buffer.add(count) = 0;
    }
    (env.api.string_release)(string);
    if !result.is_null() {
        *result = if buffer.is_null() {
            length
        } else if size == 0 {
            0
        } else {
            count
        };
    }
    Ok(())
});
api!(napi_is_array(env, value: Ptr, result: *mut bool) {
    check(!result.is_null(), 1)?;
    let value = env.js("is_array", &[value])?;
    *result = (env.api.to_boolean)(env.ctx, value);
    Ok(())
});
api!(napi_is_error(env, value: Ptr, result: *mut bool) {
    check(!result.is_null(), 1)?;
    let value = env.js("is_error", &[value])?;
    *result = (env.api.to_boolean)(env.ctx, value);
    Ok(())
});
api!(napi_is_promise(env, value: Ptr, result: *mut bool) {
    check(!result.is_null(), 1)?;
    let value = env.js("is_promise", &[value])?;
    *result = (env.api.to_boolean)(env.ctx, value);
    Ok(())
});
api!(napi_is_date(env, value: Ptr, result: *mut bool) {
    check(!result.is_null(), 1)?;
    let value = env.js("is_date", &[value])?;
    *result = (env.api.to_boolean)(env.ctx, value);
    Ok(())
});
api!(napi_is_dataview(env, value: Ptr, result: *mut bool) {
    check(!result.is_null(), 1)?;
    let value = env.js("is_dataview", &[value])?;
    *result = (env.api.to_boolean)(env.ctx, value);
    Ok(())
});
api!(napi_coerce_to_number(env, value: Ptr, result: *mut Ptr) {
    env.value(result, env.js("number", &[value])?)
});
api!(napi_coerce_to_string(env, value: Ptr, result: *mut Ptr) {
    env.value(result, env.js("string", &[value])?)
});
api!(napi_coerce_to_object(env, value: Ptr, result: *mut Ptr) {
    env.value(result, env.js("to_object", &[value])?)
});
api!(napi_get_property_names(env, object: Ptr, result: *mut Ptr) {
    env.object(object)?;
    env.value(result, env.js("keys", &[object])?)
});
api!(napi_get_prototype(env, object: Ptr, result: *mut Ptr) {
    env.object(object)?;
    env.value(result, env.js("prototype", &[object])?)
});
api!(napi_get_property(env, object: Ptr, key: Ptr, result: *mut Ptr) {
    env.object(object)?;
    env.value(result, env.js("get", &[object, key])?)
});
api!(napi_set_property(env, object: Ptr, key: Ptr, value: Ptr) {
    env.object(object)?;
    env.js("set", &[object, key, value])?;
    Ok(())
});
api!(napi_has_property(env, object: Ptr, key: Ptr, result: *mut bool) {
    env.object(object)?;
    check(!result.is_null(), 1)?;
    let value = env.js("has", &[object, key])?;
    *result = (env.api.to_boolean)(env.ctx, value);
    Ok(())
});
api!(napi_delete_property(env, object: Ptr, key: Ptr, result: *mut bool) {
    env.object(object)?;
    let value = env.js("delete", &[object, key])?;
    if !result.is_null() {
        *result = (env.api.to_boolean)(env.ctx, value);
    }
    Ok(())
});
api!(napi_get_named_property(
    env,
    object: Ptr,
    name: *const c_char,
    result: *mut Ptr,
) {
    env.object(object)?;
    let key = {
        check(!name.is_null(), 1)?;
        env.text(&CStr::from_ptr(name).to_string_lossy())
    };
    env.value(result, env.js("get", &[object, key])?)
});
api!(napi_set_named_property(env, object: Ptr, name: *const c_char, value: Ptr) {
    env.object(object)?;
    let key = {
        check(!name.is_null(), 1)?;
        env.text(&CStr::from_ptr(name).to_string_lossy())
    };
    env.js("set", &[object, key, value])?;
    Ok(())
});
api!(napi_has_named_property(
    env,
    object: Ptr,
    name: *const c_char,
    result: *mut bool,
) {
    env.object(object)?;
    check(!result.is_null(), 1)?;
    let key = {
        check(!name.is_null(), 1)?;
        env.text(&CStr::from_ptr(name).to_string_lossy())
    };
    let value = env.js("has", &[object, key])?;
    *result = (env.api.to_boolean)(env.ctx, value);
    Ok(())
});
api!(napi_get_element(env, object: Ptr, index: u32, result: *mut Ptr) {
    env.object(object)?;
    let key = env.number(index as f64);
    env.value(result, env.js("get", &[object, key])?)
});
api!(napi_set_element(env, object: Ptr, index: u32, value: Ptr) {
    env.object(object)?;
    let key = env.number(index as f64);
    env.js("set", &[object, key, value])?;
    Ok(())
});
api!(napi_has_element(env, object: Ptr, index: u32, result: *mut bool) {
    env.object(object)?;
    check(!result.is_null(), 1)?;
    let key = env.number(index as f64);
    let value = env.js("has", &[object, key])?;
    *result = (env.api.to_boolean)(env.ctx, value);
    Ok(())
});
api!(napi_delete_element(env, object: Ptr, index: u32, result: *mut bool) {
    env.object(object)?;
    let key = env.number(index as f64);
    let value = env.js("delete", &[object, key])?;
    if !result.is_null() {
        *result = (env.api.to_boolean)(env.ctx, value);
    }
    Ok(())
});
api!(napi_has_own_property(env, object: Ptr, key: Ptr, result: *mut bool) {
    env.object(object)?;
    check(!result.is_null(), 1)?;
    let value = env.js("own", &[object, key])?;
    *result = (env.api.to_boolean)(env.ctx, value);
    Ok(())
});
api!(napi_get_array_length(env, value: Ptr, result: *mut u32) {
    check(!result.is_null(), 1)?;
    let array = env.js("is_array", &[value])?;
    check((env.api.to_boolean)(env.ctx, array), 8)?;
    let length = env.js("get", &[value, env.text("length")])?;
    *result = (env.api.to_number)(env.ctx, length, ptr::null_mut()) as u32;
    Ok(())
});
api!(napi_strict_equals(env, a: Ptr, b: Ptr, result: *mut bool) {
    check(!result.is_null(), 1)?;
    let value = env.js("equal", &[a, b])?;
    *result = (env.api.to_boolean)(env.ctx, value);
    Ok(())
});
api!(napi_instanceof(env, a: Ptr, b: Ptr, result: *mut bool) {
    check(!result.is_null(), 1)?;
    let value = env.js("instanceof", &[a, b])?;
    *result = (env.api.to_boolean)(env.ctx, value);
    Ok(())
});
api!(napi_object_freeze(env, object: Ptr) {
    env.object(object)?;
    env.js("freeze", &[object])?;
    Ok(())
});
api!(napi_object_seal(env, object: Ptr) {
    env.object(object)?;
    env.js("seal", &[object])?;
    Ok(())
});
api!(napi_create_function(
    env,
    name: *const c_char,
    length: usize,
    callback: Option<Callback>,
    data: Ptr,
    result: *mut Ptr,
) {
    let callback = callback.ok_or(1)?;
    let name = if name.is_null() {
        String::new()
    } else if length == usize::MAX {
        CStr::from_ptr(name).to_string_lossy().into_owned()
    } else {
        String::from_utf8_lossy(std::slice::from_raw_parts(name.cast::<u8>(), length)).into_owned()
    };
    env.value(result, env.function(&name, callback, data)?)
});
api!(napi_get_cb_info(
    env,
    info: *const Info,
    argc: *mut usize,
    argv: *mut Ptr,
    this: *mut Ptr,
    data: *mut Ptr,
) {
    check(!info.is_null(), 1)?;
    let info = &*info;
    let args = &*info.args;
    if !argv.is_null() {
        check(!argc.is_null(), 1)?;
        for index in 0..*argc {
            *argv.add(index) = args.get(index).copied().unwrap_or_else(|| env.undefined());
        }
    }
    if !argc.is_null() {
        *argc = args.len();
    }
    if !this.is_null() {
        *this = info.this;
    }
    if !data.is_null() {
        *data = info.data;
    }
    Ok(())
});
api!(napi_get_new_target(env, info: *const Info, result: *mut Ptr) {
    check(!info.is_null() && !result.is_null(), 1)?;
    *result = (*info).target;
    Ok(())
});
api!(napi_call_function(
    env,
    this: Ptr,
    function: Ptr,
    argc: usize,
    argv: *const Ptr,
    result: *mut Ptr,
) {
    check(argc == 0 || !argv.is_null(), 1)?;
    check(env.kind(function)? == 7, 5)?;
    let mut args = Vec::with_capacity(argc + 2);
    args.extend_from_slice(&[function, this]);
    if argc > 0 {
        args.extend_from_slice(std::slice::from_raw_parts(argv, argc));
    }
    let value = env.js("call", &args)?;
    if !result.is_null() {
        env.value(result, value)?;
    }
    Ok(())
});
api!(napi_new_instance(
    env,
    constructor: Ptr,
    argc: usize,
    argv: *const Ptr,
    result: *mut Ptr,
) {
    check(argc == 0 || !argv.is_null(), 1)?;
    let mut args = vec![constructor];
    if argc > 0 {
        args.extend_from_slice(std::slice::from_raw_parts(argv, argc));
    }
    env.value(result, env.js("construct", &args)?)
});
api!(napi_run_script(env, script: Ptr, result: *mut Ptr) {
    check(env.kind(script)? == 4, 3)?;
    let string = (env.api.to_string)(env.ctx, script, ptr::null_mut());
    let mut exception = ptr::null_mut();
    let value = (env.api.evaluate)(
        env.ctx,
        string,
        ptr::null_mut(),
        ptr::null_mut(),
        1,
        &mut exception,
    );
    (env.api.string_release)(string);
    if !exception.is_null() {
        env.exception(exception);
        return Err(10);
    }
    env.value(result, value)
});
api!(napi_create_error(env, code: Ptr, message: Ptr, result: *mut Ptr) {
    check(env.kind(message)? == 4, 3)?;
    env.value(
        result,
        env.js(
            "error",
            &[
                env.text("Error"),
                if code.is_null() {
                    env.undefined()
                } else {
                    code
                },
                message,
            ],
        )?,
    )
});
api!(napi_throw_error(env, code: *const c_char, message: *const c_char) {
    check(!message.is_null(), 1)?;
    let code = if code.is_null() {
        env.undefined()
    } else {
        env.text(&CStr::from_ptr(code).to_string_lossy())
    };
    let error = env.js(
        "error",
        &[
            env.text("Error"),
            code,
            env.text(&CStr::from_ptr(message).to_string_lossy()),
        ],
    )?;
    env.exception(error);
    Ok(())
});
api!(napi_create_type_error(env, code: Ptr, message: Ptr, result: *mut Ptr) {
    check(env.kind(message)? == 4, 3)?;
    env.value(
        result,
        env.js(
            "error",
            &[
                env.text("TypeError"),
                if code.is_null() {
                    env.undefined()
                } else {
                    code
                },
                message,
            ],
        )?,
    )
});
api!(napi_throw_type_error(env, code: *const c_char, message: *const c_char) {
    check(!message.is_null(), 1)?;
    let code = if code.is_null() {
        env.undefined()
    } else {
        env.text(&CStr::from_ptr(code).to_string_lossy())
    };
    let error = env.js(
        "error",
        &[
            env.text("TypeError"),
            code,
            env.text(&CStr::from_ptr(message).to_string_lossy()),
        ],
    )?;
    env.exception(error);
    Ok(())
});
api!(napi_create_range_error(env, code: Ptr, message: Ptr, result: *mut Ptr) {
    check(env.kind(message)? == 4, 3)?;
    env.value(
        result,
        env.js(
            "error",
            &[
                env.text("RangeError"),
                if code.is_null() {
                    env.undefined()
                } else {
                    code
                },
                message,
            ],
        )?,
    )
});
api!(napi_throw_range_error(env, code: *const c_char, message: *const c_char) {
    check(!message.is_null(), 1)?;
    let code = if code.is_null() {
        env.undefined()
    } else {
        env.text(&CStr::from_ptr(code).to_string_lossy())
    };
    let error = env.js(
        "error",
        &[
            env.text("RangeError"),
            code,
            env.text(&CStr::from_ptr(message).to_string_lossy()),
        ],
    )?;
    env.exception(error);
    Ok(())
});
api!(napi_throw(env, value: Ptr) {
    check(!value.is_null(), 1)?;
    env.exception(value);
    Ok(())
});
api!(napi_is_exception_pending(env, result: *mut bool) {
    check(!result.is_null(), 1)?;
    *result = !env.pending.get().is_null();
    Ok(())
});
api!(napi_get_and_clear_last_exception(env, result: *mut Ptr) {
    check(!result.is_null(), 1)?;
    let value = env.take_exception();
    env.value(
        result,
        if value.is_null() {
            env.undefined()
        } else {
            value
        },
    )
});
api!(napi_open_handle_scope(env, result: *mut Ptr) {
    check(!result.is_null(), 1)?;
    let mut scope = Box::<Scope>::default();
    *result = (&mut *scope as *mut Scope).cast();
    env.scopes.borrow_mut().push(scope);
    Ok(())
});
api!(napi_close_handle_scope(env, scope: Ptr) {
    check(!scope.is_null(), 1)?;
    check(
        env.scopes
            .borrow()
            .last()
            .is_some_and(|last| ptr::eq(&**last, scope.cast::<Scope>())),
        13,
    )?;
    env.pop();
    Ok(())
});
api!(napi_open_escapable_handle_scope(env, result: *mut Ptr) {
    check(!result.is_null(), 1)?;
    let mut scope = Box::new(Scope {
        escapable: true,
        ..Scope::default()
    });
    *result = (&mut *scope as *mut Scope).cast();
    env.scopes.borrow_mut().push(scope);
    Ok(())
});
api!(napi_close_escapable_handle_scope(env, scope: Ptr) {
    check(!scope.is_null(), 1)?;
    check(
        env.scopes
            .borrow()
            .last()
            .is_some_and(|last| ptr::eq(&**last, scope.cast::<Scope>())),
        13,
    )?;
    env.pop();
    Ok(())
});
api!(napi_escape_handle(env, scope: Ptr, value: Ptr, result: *mut Ptr) {
    check(!scope.is_null() && !value.is_null() && !result.is_null(), 1)?;
    let mut scopes = env.scopes.borrow_mut();
    let current = scopes.last_mut().ok_or(13)?;
    check(
        ptr::eq(&**current, scope.cast::<Scope>()) && current.escapable,
        13,
    )?;
    check(!current.escaped, 12)?;
    current.escaped = true;
    (env.api.protect)(env.ctx, value);
    let length = scopes.len();
    if length > 1 {
        scopes[length - 2].values.push(value);
    } else {
        env.held.borrow_mut().push(value);
    }
    *result = value;
    Ok(())
});
api!(napi_create_reference(env, value: Ptr, count: u32, result: *mut Ptr) {
    check(!result.is_null(), 1)?;
    env.object(value)?;
    let weak = env.js("weak", &[value])?;
    (env.api.protect)(env.ctx, weak);
    if count > 0 {
        (env.api.protect)(env.ctx, value);
    }
    let mut reference = Box::new(Reference { value, weak, count });
    let pointer = (&mut *reference as *mut Reference).cast();
    env.references
        .borrow_mut()
        .insert(pointer as usize, reference);
    *result = pointer;
    Ok(())
});
api!(napi_delete_reference(env, reference: Ptr) {
    let reference = env
        .references
        .borrow_mut()
        .remove(&(reference as usize))
        .ok_or(1)?;
    (env.api.unprotect)(env.ctx, reference.weak);
    if reference.count > 0 {
        (env.api.unprotect)(env.ctx, reference.value);
    }
    Ok(())
});
api!(napi_get_reference_value(env, reference: Ptr, result: *mut Ptr) {
    check(!result.is_null(), 1)?;
    let (value, weak, count) = {
        let refs = env.references.borrow();
        let reference = refs.get(&(reference as usize)).ok_or(1)?;
        (reference.value, reference.weak, reference.count)
    };
    let value = if count > 0 {
        value
    } else {
        env.js("deref", &[weak])?
    };
    *result = if env.kind(value)? == 0 {
        ptr::null_mut()
    } else {
        env.keep(value)
    };
    Ok(())
});
api!(napi_reference_ref(env, reference: Ptr, result: *mut u32) {
    let mut value = ptr::null_mut();
    check(
        napi_get_reference_value(env, reference, &mut value) == 0 && !value.is_null(),
        9,
    )?;
    let mut refs = env.references.borrow_mut();
    let reference = refs.get_mut(&(reference as usize)).ok_or(1)?;
    check(reference.count < u32::MAX, 9)?;
    if reference.count == 0 {
        (env.api.protect)(env.ctx, value);
        reference.value = value;
    }
    reference.count += 1;
    if !result.is_null() {
        *result = reference.count;
    }
    Ok(())
});
api!(napi_reference_unref(env, reference: Ptr, result: *mut u32) {
    let mut refs = env.references.borrow_mut();
    let reference = refs.get_mut(&(reference as usize)).ok_or(1)?;
    check(reference.count > 0, 9)?;
    reference.count -= 1;
    if reference.count == 0 {
        (env.api.unprotect)(env.ctx, reference.value);
    }
    if !result.is_null() {
        *result = reference.count;
    }
    Ok(())
});
api!(napi_set_instance_data(env, data: Ptr, callback: Option<Finalize>, hint: Ptr) {
    env.instance.set(data);
    env.instance_finalizer
        .set(callback.map(|callback| Finalizer {
            callback,
            data,
            hint,
        }));
    Ok(())
});
api!(napi_get_instance_data(env, result: *mut Ptr) {
    check(!result.is_null(), 1)?;
    *result = env.instance.get();
    Ok(())
});
api!(napi_add_env_cleanup_hook(env, callback: Option<Cleanup>, data: Ptr) {
    let callback = callback.ok_or(1)?;
    check(
        !env.cleanup
            .borrow()
            .iter()
            .any(|(cb, arg)| *cb as usize == callback as usize && *arg == data),
        1,
    )?;
    env.cleanup.borrow_mut().push((callback, data));
    Ok(())
});
api!(napi_remove_env_cleanup_hook(env, callback: Option<Cleanup>, data: Ptr) {
    let callback = callback.ok_or(1)?;
    let mut hooks = env.cleanup.borrow_mut();
    let index = hooks
        .iter()
        .rposition(|(cb, arg)| *cb as usize == callback as usize && *arg == data)
        .ok_or(1)?;
    hooks.remove(index);
    Ok(())
});
api!(napi_create_date(env, value: f64, result: *mut Ptr) {
    env.value(result, env.js("date", &[env.number(value)])?)
});
api!(napi_get_date_value(env, value: Ptr, result: *mut f64) {
    check(!result.is_null(), 1)?;
    check((env.api.to_boolean)(env.ctx, env.js("is_date", &[value])?), 18)?;
    let value = env.js("date_value", &[value])?;
    *result = (env.api.to_number)(env.ctx, value, ptr::null_mut());
    Ok(())
});
api!(napi_create_bigint_int64(env, value: i64, result: *mut Ptr) {
    env.value(result, env.js("bigint", &[env.text(&value.to_string())])?)
});
api!(napi_get_value_bigint_int64(
    env,
    value: Ptr,
    result: *mut i64,
    lossless: *mut bool,
) {
    check(!result.is_null() && !lossless.is_null(), 1)?;
    check(env.kind(value)? == 9, 17)?;
    let text = env.js("bigint_i64", &[value])?;
    *result = env.api.text(env.ctx, text).parse().map_err(|_| 9)?;
    let exact = env.js("bigint_lossless", &[value, env.boolean(true)])?;
    *lossless = (env.api.to_boolean)(env.ctx, exact);
    Ok(())
});
api!(napi_create_bigint_uint64(env, value: u64, result: *mut Ptr) {
    env.value(result, env.js("bigint", &[env.text(&value.to_string())])?)
});
api!(napi_get_value_bigint_uint64(
    env,
    value: Ptr,
    result: *mut u64,
    lossless: *mut bool,
) {
    check(!result.is_null() && !lossless.is_null(), 1)?;
    check(env.kind(value)? == 9, 17)?;
    let text = env.js("bigint_u64", &[value])?;
    *result = env.api.text(env.ctx, text).parse().map_err(|_| 9)?;
    let exact = env.js("bigint_lossless", &[value, env.boolean(false)])?;
    *lossless = (env.api.to_boolean)(env.ctx, exact);
    Ok(())
});
api!(napi_create_promise(env, deferred: *mut Ptr, result: *mut Ptr) {
    check(!deferred.is_null(), 1)?;
    let state = env.js("promise", &[])?;
    check(napi_create_reference(env, state, 1, deferred) == 0, 9)?;
    env.value(result, env.js("get", &[state, env.text("promise")])?)
});
api!(napi_resolve_deferred(env, deferred: Ptr, value: Ptr) {
    let mut state = ptr::null_mut();
    check(napi_get_reference_value(env, deferred, &mut state) == 0, 1)?;
    let callback = env.js("get", &[state, env.text("resolve")])?;
    check(
        napi_call_function(env, state, callback, 1, &value, ptr::null_mut()) == 0,
        10,
    )?;
    check(napi_delete_reference(env, deferred) == 0, 1)
});
api!(napi_reject_deferred(env, deferred: Ptr, value: Ptr) {
    let mut state = ptr::null_mut();
    check(napi_get_reference_value(env, deferred, &mut state) == 0, 1)?;
    let callback = env.js("get", &[state, env.text("reject")])?;
    check(
        napi_call_function(env, state, callback, 1, &value, ptr::null_mut()) == 0,
        10,
    )?;
    check(napi_delete_reference(env, deferred) == 0, 1)
});

api!(napi_define_properties(
    env,
    object: Ptr,
    count: usize,
    properties: *const Property,
) {
    env.object(object)?;
    check(count == 0 || !properties.is_null(), 1)?;
    for index in 0..count {
        let property = &*properties.add(index);
        let key = if property.name.is_null() {
            property.key
        } else {
            env.text(&CStr::from_ptr(property.name).to_string_lossy())
        };
        check(!key.is_null(), 1)?;
        let name = env.api.text(env.ctx, key);
        let value = if let Some(method) = property.method {
            env.function(&name, method, property.data)?
        } else if property.value.is_null() {
            env.undefined()
        } else {
            property.value
        };
        let getter = if let Some(callback) = property.getter {
            env.function(&name, callback, property.data)?
        } else {
            env.undefined()
        };
        let setter = if let Some(callback) = property.setter {
            env.function(&name, callback, property.data)?
        } else {
            env.undefined()
        };
        env.js(
            "define",
            &[
                object,
                key,
                value,
                getter,
                setter,
                env.number(property.attributes as f64),
            ],
        )?;
    }
    Ok(())
});
api!(napi_define_class(
    env,
    name: *const c_char,
    length: usize,
    callback: Option<Callback>,
    data: Ptr,
    count: usize,
    properties: *const Property,
    result: *mut Ptr,
) {
    check(count == 0 || !properties.is_null(), 1)?;
    let mut constructor = ptr::null_mut();
    check(
        napi_create_function(env, name, length, callback, data, &mut constructor) == 0,
        1,
    )?;
    let prototype = env.js("get", &[constructor, env.text("prototype")])?;
    for index in 0..count {
        let property = properties.add(index);
        let object = if (*property).attributes & 1024 == 0 {
            prototype
        } else {
            constructor
        };
        let status = napi_define_properties(env, object, 1, property);
        check(status == 0, status)?;
    }
    env.value(result, constructor)
});
api!(napi_wrap(
    env,
    object: Ptr,
    data: Ptr,
    callback: Option<Finalize>,
    hint: Ptr,
    result: *mut Ptr,
) {
    env.object(object)?;
    let wrapped = env.js("wrap", &[object])?;
    check(!(env.api.to_boolean)(env.ctx, wrapped), 1)?;
    check(result.is_null() || callback.is_some(), 1)?;
    let id = env.track(object, 1, None)?;
    env.js("wrap", &[object, env.number(id as f64)])?;
    env.wraps.borrow_mut().insert(id, data);
    if let Some(callback) = callback {
        env.wrap_finalizers.borrow_mut().insert(
            id,
            Finalizer {
                callback,
                data,
                hint,
            },
        );
    }
    if !result.is_null() {
        check(napi_create_reference(env, object, 0, result) == 0, 9)?;
    }
    Ok(())
});
api!(napi_unwrap(env, object: Ptr, result: *mut Ptr) {
    check(!result.is_null(), 1)?;
    env.object(object)?;
    let id = env.js("wrap", &[object])?;
    let id = (env.api.to_number)(env.ctx, id, ptr::null_mut()) as u64;
    *result = *env.wraps.borrow().get(&id).ok_or(1)?;
    Ok(())
});
api!(napi_remove_wrap(env, object: Ptr, result: *mut Ptr) {
    check(!result.is_null(), 1)?;
    env.object(object)?;
    let id = env.js("wrap", &[object, env.number(0.0)])?;
    let id = (env.api.to_number)(env.ctx, id, ptr::null_mut()) as u64;
    let data = env.wraps.borrow_mut().remove(&id).ok_or(1)?;
    env.wrap_finalizers.borrow_mut().remove(&id);
    *result = data;
    Ok(())
});
api!(napi_add_finalizer(
    env,
    object: Ptr,
    data: Ptr,
    callback: Option<Finalize>,
    hint: Ptr,
    result: *mut Ptr,
) {
    env.object(object)?;
    let callback = callback.ok_or(1)?;
    env.track(object, 0, Some(Finalizer { callback, data, hint }))?;
    if !result.is_null() {
        check(napi_create_reference(env, object, 0, result) == 0, 9)?;
    }
    Ok(())
});
api!(napi_create_external(
    env,
    data: Ptr,
    callback: Option<Finalize>,
    hint: Ptr,
    result: *mut Ptr,
) {
    let value = env.js("object", &[])?;
    let id = env.track(value, 2, callback.map(|callback| Finalizer { callback, data, hint }))?;
    env.js("external", &[value, env.number(id as f64)])?;
    env.externals.borrow_mut().insert(value as usize, (id, data));
    env.value(result, value)
});
api!(napi_get_value_external(env, value: Ptr, result: *mut Ptr) {
    check(!result.is_null(), 1)?;
    *result = env.external(value)?.ok_or(1)?;
    Ok(())
});

unsafe extern "C" fn free_buffer(_: Ptr, context: Ptr) {
    unsafe {
        drop(Box::from_raw(context.cast::<Vec<u8>>()));
    }
}
api!(napi_create_arraybuffer(env, length: usize, data: *mut Ptr, result: *mut Ptr) {
    check(!result.is_null(), 1)?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length).map_err(|_| 9)?;
    bytes.resize(length, 0);
    let mut bytes = Box::new(bytes);
    let pointer = bytes.as_mut_ptr().cast();
    let mut exception = ptr::null_mut();
    let value = (env.api.make_buffer)(
        env.ctx,
        pointer,
        length,
        Some(free_buffer),
        Box::into_raw(bytes).cast(),
        &mut exception,
    );
    if !exception.is_null() {
        env.exception(exception);
        return Err(10);
    }
    if !data.is_null() {
        *data = pointer;
    }
    env.value(result, value)
});
api!(napi_create_external_arraybuffer(
    env,
    data: Ptr,
    length: usize,
    callback: Option<Finalize>,
    hint: Ptr,
    result: *mut Ptr,
) {
    check((!data.is_null() || length == 0) && !result.is_null(), 1)?;
    // The finalizer follows the bytes, not the wrapper, because ArrayBuffer.transfer() moves them.
    let (id, holder) = env.holder();
    let mut exception = ptr::null_mut();
    let value = (env.api.make_buffer)(env.ctx, data, length, Some(release_holder), holder.cast(), &mut exception);
    if !exception.is_null() {
        env.exception(exception);
        return Err(10);
    }
    if let Some(callback) = callback {
        env.finalizers.borrow_mut().insert(id, Finalizer { callback, data, hint });
    }
    env.value(result, value)
});
api!(napi_is_arraybuffer(env, value: Ptr, result: *mut bool) {
    check(!value.is_null() && !result.is_null(), 1)?;
    *result = (env.api.typed_kind)(env.ctx, value, ptr::null_mut()) == 9;
    Ok(())
});
api!(napi_get_arraybuffer_info(
    env,
    value: Ptr,
    data: *mut Ptr,
    length: *mut usize,
) {
    check(!value.is_null(), 1)?;
    check(
        (env.api.typed_kind)(env.ctx, value, ptr::null_mut()) == 9,
        19,
    )?;
    if !data.is_null() {
        *data = (env.api.buffer_bytes)(env.ctx, value, ptr::null_mut());
    }
    if !length.is_null() {
        *length = (env.api.buffer_length)(env.ctx, value, ptr::null_mut());
    }
    Ok(())
});
api!(napi_create_typedarray(
    env,
    kind: u32,
    length: usize,
    buffer: Ptr,
    offset: usize,
    result: *mut Ptr,
) {
    check(kind <= 10, 1)?;
    env.value(
        result,
        env.js(
            "typed",
            &[
                env.number(kind as f64),
                buffer,
                env.number(offset as f64),
                env.number(length as f64),
            ],
        )?,
    )
});
api!(napi_is_typedarray(env, value: Ptr, result: *mut bool) {
    check(!value.is_null() && !result.is_null(), 1)?;
    let kind = (env.api.typed_kind)(env.ctx, value, ptr::null_mut());
    *result = kind <= 8 || kind == 11 || kind == 12;
    Ok(())
});
api!(napi_get_typedarray_info(
    env,
    value: Ptr,
    kind: *mut u32,
    length: *mut usize,
    data: *mut Ptr,
    buffer: *mut Ptr,
    offset: *mut usize,
) {
    check(!value.is_null(), 1)?;
    let native = (env.api.typed_kind)(env.ctx, value, ptr::null_mut());
    let (ty, size) = match native {
        0 => (0, 1),
        1 => (3, 2),
        2 => (5, 4),
        3 => (1, 1),
        4 => (2, 1),
        5 => (4, 2),
        6 => (6, 4),
        7 => (7, 4),
        8 => (8, 8),
        11 => (9, 8),
        12 => (10, 8),
        _ => return Err(1),
    };
    let byte_offset = (env.api.typed_offset)(env.ctx, value, ptr::null_mut());
    if !kind.is_null() {
        *kind = ty;
    }
    if !length.is_null() {
        *length = (env.api.typed_length)(env.ctx, value, ptr::null_mut()) / size;
    }
    if !data.is_null() {
        *data = (env.api.typed_bytes)(env.ctx, value, ptr::null_mut())
            .cast::<u8>()
            .wrapping_add(byte_offset)
            .cast();
    }
    if !offset.is_null() {
        *offset = byte_offset;
    }
    if !buffer.is_null() {
        *buffer = env.keep((env.api.typed_buffer)(env.ctx, value, ptr::null_mut()));
    }
    Ok(())
});
api!(napi_create_buffer(env, length: usize, data: *mut Ptr, result: *mut Ptr) {
    let mut buffer = ptr::null_mut();
    let status = napi_create_arraybuffer(env, length, data, &mut buffer);
    check(status == 0, status)?;
    env.value(result, env.js("buffer", &[buffer])?)
});
api!(napi_create_buffer_copy(
    env,
    length: usize,
    data: Ptr,
    copied: *mut Ptr,
    result: *mut Ptr,
) {
    check(!data.is_null() || length == 0, 1)?;
    let mut pointer = ptr::null_mut();
    let status = napi_create_buffer(env, length, &mut pointer, result);
    check(status == 0, status)?;
    if length > 0 {
        ptr::copy_nonoverlapping(data.cast::<u8>(), pointer.cast::<u8>(), length);
    }
    if !copied.is_null() {
        *copied = pointer;
    }
    Ok(())
});
api!(napi_create_external_buffer(
    env,
    length: usize,
    data: Ptr,
    callback: Option<Finalize>,
    hint: Ptr,
    result: *mut Ptr,
) {
    let mut buffer = ptr::null_mut();
    let status = napi_create_external_arraybuffer(env, data, length, callback, hint, &mut buffer);
    check(status == 0, status)?;
    env.value(result, env.js("buffer", &[buffer])?)
});
api!(napi_is_buffer(env, value: Ptr, result: *mut bool) {
    check(!value.is_null() && !result.is_null(), 1)?;
    *result = (env.api.typed_kind)(env.ctx, value, ptr::null_mut()) == 3;
    Ok(())
});
api!(napi_get_buffer_info(env, value: Ptr, data: *mut Ptr, length: *mut usize) {
    check(!value.is_null(), 1)?;
    check(
        (env.api.typed_kind)(env.ctx, value, ptr::null_mut()) == 3,
        1,
    )?;
    let status = napi_get_typedarray_info(
        env,
        value,
        ptr::null_mut(),
        length,
        data,
        ptr::null_mut(),
        ptr::null_mut(),
    );
    check(status == 0, status)
});
api!(napi_create_dataview(
    env,
    length: usize,
    buffer: Ptr,
    offset: usize,
    result: *mut Ptr,
) {
    env.value(
        result,
        env.js(
            "dataview",
            &[buffer, env.number(offset as f64), env.number(length as f64)],
        )?,
    )
});
api!(napi_get_dataview_info(
    env,
    value: Ptr,
    length: *mut usize,
    data: *mut Ptr,
    buffer: *mut Ptr,
    offset: *mut usize,
) {
    let valid = env.js("is_dataview", &[value])?;
    check((env.api.to_boolean)(env.ctx, valid), 1)?;
    let array = env.js("get", &[value, env.text("buffer")])?;
    check((env.api.typed_kind)(env.ctx, array, ptr::null_mut()) == 9, 1)?;
    let start = env.js("get", &[value, env.text("byteOffset")])?;
    let start = (env.api.to_number)(env.ctx, start, ptr::null_mut()) as usize;
    let size = env.js("get", &[value, env.text("byteLength")])?;
    let size = (env.api.to_number)(env.ctx, size, ptr::null_mut()) as usize;
    check(
        start.checked_add(size).is_some_and(|end| {
            end <= (env.api.buffer_length)(env.ctx, array, ptr::null_mut())
        }),
        1,
    )?;
    if !buffer.is_null() {
        *buffer = array;
    }
    if !offset.is_null() {
        *offset = start;
    }
    if !data.is_null() {
        *data = (env.api.buffer_bytes)(env.ctx, array, ptr::null_mut())
            .cast::<u8>()
            .wrapping_add(start)
            .cast();
    }
    if !length.is_null() {
        *length = size;
    }
    Ok(())
});

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
    let Some(env) = state else {
        return ptr::null_mut();
    };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Ptr> {
        ensure!(count == 1, "Native addon loading needs one path");
        env.load(&env.api.text(ctx, unsafe { *args }))
    }));
    match outcome {
        Ok(Ok(value)) => value,
        result => {
            let message = match result {
                Ok(Err(error)) => format!("{error:#}"),
                _ => "Native addon loading failed".into(),
            };
            unsafe {
                *exception = env.text(&message);
            }
            env.undefined()
        }
    }
}

// Addon threads call these without an environment. Access only thread-safe state.
macro_rules! raw_api {
    ($name:ident($($arg:ident: $ty:ty),* $(,)?) $body:block) => {
        #[unsafe(no_mangle)]
        unsafe extern "C" fn $name($($arg: $ty),*) -> i32 $body
    };
}

api!(napi_adjust_external_memory(env, change: i64, result: *mut i64) {
    check(!result.is_null(), 1)?;
    // Report addon memory so GC can account for it. JSC does not accept decreases.
    if change > 0
        && let Some(report) = env.api.extra_memory
    {
        report(env.ctx, change as usize);
    }
    env.external.set(env.external.get().saturating_add(change).max(0));
    *result = env.external.get();
    Ok(())
});
api!(napi_coerce_to_bool(env, value: Ptr, result: *mut Ptr) {
    env.value(result, env.boolean((env.api.to_boolean)(env.ctx, value)))
});
api!(napi_get_value_bigint_words(
    env,
    value: Ptr,
    sign: *mut i32,
    count: *mut usize,
    words: *mut u64,
) {
    check(!count.is_null(), 1)?;
    check(env.kind(value)? == 9, 17)?;
    let text = env.api.text(env.ctx, env.js("bigint_words", &[value])?);
    let mut parts = text.split(',');
    let negative = parts.next() == Some("1");
    let list: Vec<u64> = parts.filter_map(|word| word.parse().ok()).collect();
    if !sign.is_null() {
        *sign = i32::from(negative);
    }
    if !words.is_null() {
        for (index, word) in list.iter().take(*count).enumerate() {
            *words.add(index) = *word;
        }
    }
    // The count is the words the value needs, also when the buffer was shorter.
    *count = list.len();
    Ok(())
});
api!(napi_type_tag_object(env, object: Ptr, tag: *const [u64; 2]) {
    check(!tag.is_null(), 1)?;
    env.object(object)?;
    let existing = env.js("type_tag", &[object])?;
    check((env.api.value_type)(env.ctx, existing) == 0, 1)?;
    env.js("type_tag", &[object, env.text(&format!("{:?}", *tag))])?;
    Ok(())
});
api!(napi_check_object_type_tag(env, object: Ptr, tag: *const [u64; 2], result: *mut bool) {
    check(!tag.is_null() && !result.is_null(), 1)?;
    env.object(object)?;
    let existing = env.js("type_tag", &[object])?;
    *result = (env.api.value_type)(env.ctx, existing) != 0
        && env.api.text(env.ctx, existing) == format!("{:?}", *tag);
    Ok(())
});
api!(napi_fatal_exception(env, error: Ptr) {
    env.js("report", &[error])?;
    Ok(())
});
raw_api!(napi_fatal_error(
    location: *const c_char,
    location_length: usize,
    message: *const c_char,
    message_length: usize,
) {
    let text = |pointer: *const c_char, length: usize| unsafe {
        if pointer.is_null() {
            String::new()
        } else if length == usize::MAX {
            CStr::from_ptr(pointer).to_string_lossy().into_owned()
        } else {
            String::from_utf8_lossy(std::slice::from_raw_parts(pointer.cast(), length)).into_owned()
        }
    };
    eprintln!(
        "FATAL ERROR: {} {}",
        text(location, location_length),
        text(message, message_length)
    );
    std::process::abort()
});

#[repr(C)]
struct LegacyModule {
    version: i32,
    flags: u32,
    filename: *const c_char,
    register: Option<unsafe extern "C" fn(*const State, Ptr) -> Ptr>,
}
thread_local! { static LEGACY_MODULE: Cell<Option<unsafe extern "C" fn(*const State, Ptr) -> Ptr>> = const { Cell::new(None) }; }
raw_api!(napi_module_register(module: *const LegacyModule) {
    if !module.is_null() {
        LEGACY_MODULE.set(unsafe { (*module).register });
    }
    0
});

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

api!(napi_get_node_version(env, result: *mut *const NodeVersion) {
    let _ = env;
    check(!result.is_null(), 1)?;
    *result = &NODE_VERSION.0;
    Ok(())
});
api!(napi_get_uv_event_loop(env, result: *mut Ptr) {
    let _ = env;
    check(!result.is_null(), 1)?;
    // Fail without libuv: a fake loop pointer can crash the addon.
    *result = ptr::null_mut();
    Err(9)
});
api!(napi_async_init(env, _resource: Ptr, name: Ptr, result: *mut Ptr) {
    let _ = env;
    check(!name.is_null() && !result.is_null(), 1)?;
    *result = ptr::NonNull::<u8>::dangling().as_ptr().cast();
    Ok(())
});
api!(napi_async_destroy(env, context: Ptr) {
    let _ = env;
    check(!context.is_null(), 1)
});
api!(napi_make_callback(
    env,
    _context: Ptr,
    this: Ptr,
    function: Ptr,
    argc: usize,
    argv: *const Ptr,
    result: *mut Ptr,
) {
    check(!this.is_null(), 1)?;
    let status = napi_call_function(env, this, function, argc, argv, result);
    check(status == 0, status)
});
api!(napi_open_callback_scope(env, _resource: Ptr, _context: Ptr, result: *mut Ptr) {
    check(!result.is_null(), 1)?;
    env.callback_scopes.set(env.callback_scopes.get() + 1);
    *result = ptr::NonNull::<u8>::dangling().as_ptr().cast();
    Ok(())
});
api!(napi_close_callback_scope(env, scope: Ptr) {
    check(!scope.is_null(), 1)?;
    check(env.callback_scopes.get() > 0, 14)?;
    env.callback_scopes.set(env.callback_scopes.get() - 1);
    Ok(())
});
api!(napi_get_all_property_names(
    env,
    object: Ptr,
    mode: u32,
    filter: u32,
    conversion: u32,
    result: *mut Ptr,
) {
    env.object(object)?;
    check(mode <= 1 && conversion <= 1, 1)?;
    env.value(
        result,
        env.js(
            "all_keys",
            &[
                object,
                env.boolean(mode == 1),
                env.number(filter as f64),
                env.boolean(conversion == 1),
            ],
        )?,
    )
});
api!(napi_detach_arraybuffer(env, value: Ptr) {
    check(!value.is_null(), 1)?;
    check((env.api.typed_kind)(env.ctx, value, ptr::null_mut()) == 9, 19)?;
    check((env.api.to_boolean)(env.ctx, env.js("detach", &[value])?), 20)
});
api!(napi_is_detached_arraybuffer(env, value: Ptr, result: *mut bool) {
    check(!value.is_null() && !result.is_null(), 1)?;
    *result = (env.api.typed_kind)(env.ctx, value, ptr::null_mut()) == 9
        && (env.api.to_boolean)(env.ctx, env.js("get", &[value, env.text("detached")])?);
    Ok(())
});
api!(napi_create_bigint_words(
    env,
    sign: i32,
    count: usize,
    words: *const u64,
    result: *mut Ptr,
) {
    check(!result.is_null() && (count == 0 || !words.is_null()) && count <= i32::MAX as usize, 1)?;
    let words = if count == 0 { &[][..] } else { std::slice::from_raw_parts(words, count) };
    let mut hex = String::with_capacity(3 + 16 * count);
    hex.push_str("0x0");
    for word in words.iter().rev() {
        hex.push_str(&format!("{word:016x}"));
    }
    env.value(result, env.js("bigint_hex", &[env.text(&hex), env.boolean(sign != 0)])?)
});

struct AsyncCleanup {
    owner: Weak<State>,
    thread: std::thread::ThreadId,
    hook: AsyncCleanupHook,
    arg: Ptr,
}
api!(napi_add_async_cleanup_hook(env, hook: Option<AsyncCleanupHook>, arg: Ptr, handle: *mut Ptr) {
    let hook = hook.ok_or(1)?;
    let cleanup = Box::into_raw(Box::new(AsyncCleanup {
        owner: env.owner.clone(),
        thread: env.thread,
        hook,
        arg,
    }));
    env.async_cleanup.borrow_mut().push(cleanup as usize);
    if !handle.is_null() {
        *handle = cleanup.cast();
    }
    Ok(())
});
raw_api!(napi_remove_async_cleanup_hook(handle: Ptr) {
    if handle.is_null() || unsafe { (*handle.cast::<AsyncCleanup>()).thread } != std::thread::current().id() {
        return 1;
    }
    let cleanup = unsafe { Box::from_raw(handle.cast::<AsyncCleanup>()) };
    if let Some(env) = cleanup.owner.upgrade() {
        env.async_cleanup.borrow_mut().retain(|other| *other != handle as usize);
    }
    0
});

type AsyncExecute = unsafe extern "C" fn(*const State, Ptr);
type AsyncComplete = unsafe extern "C" fn(*const State, i32, Ptr);
type CallJs = unsafe extern "C" fn(*const State, Ptr, Ptr, Ptr);

const IDLE: u8 = 0;
const QUEUED: u8 = 1;
const RUNNING: u8 = 2;
const CANCELLED: u8 = 3;
// `execute` has returned, and `complete` has not run yet.
const DONE: u8 = 4;

struct AsyncWork {
    execute: AsyncExecute,
    complete: Option<AsyncComplete>,
    data: Ptr,
    state: Arc<AtomicU8>,
}
api!(napi_create_async_work(
    env,
    _resource: Ptr,
    _name: Ptr,
    execute: Option<AsyncExecute>,
    complete: Option<AsyncComplete>,
    data: Ptr,
    result: *mut Ptr,
) {
    let _ = env;
    check(!result.is_null(), 1)?;
    *result = Box::into_raw(Box::new(AsyncWork {
        execute: execute.ok_or(1)?,
        complete,
        data,
        state: Arc::new(AtomicU8::new(IDLE)),
    }))
    .cast();
    Ok(())
});
api!(napi_delete_async_work(env, work: Ptr) {
    let _ = env;
    check(!work.is_null(), 1)?;
    drop(Box::from_raw(work.cast::<AsyncWork>()));
    Ok(())
});
api!(napi_queue_async_work(env, work: Ptr) {
    check(!work.is_null(), 1)?;
    let item = &*work.cast::<AsyncWork>();
    check(item.state.compare_exchange(IDLE, QUEUED, Ordering::AcqRel, Ordering::Acquire).is_ok(), 1)?;
    let (execute, data, state) = (item.execute, item.data as usize, item.state.clone());
    let (ready, wake, raw, work) = (env.ready.clone(), crate::host::Waker::current(), ptr::from_ref(env) as usize, work as usize);
    crate::node::blocking(move || {
        let run = state.compare_exchange(QUEUED, RUNNING, Ordering::AcqRel, Ordering::Acquire).is_ok();
        if run {
            unsafe { execute(raw as *const State, data as Ptr) };
            state.store(DONE, Ordering::Release);
        }
        ready.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(Ready::Work(work, if run { 0 } else { 11 }));
        wake.wake();
    })
    .map_err(|_| {
        item.state.store(IDLE, Ordering::Release);
        9
    })?;
    env.active.set(env.active.get() + 1);
    let mut works = env.works.borrow_mut();
    works.retain(|state| state.load(Ordering::Acquire) != IDLE);
    works.push(item.state.clone());
    Ok(())
});
api!(napi_cancel_async_work(env, work: Ptr) {
    let _ = env;
    check(!work.is_null(), 1)?;
    check((*work.cast::<AsyncWork>()).state.compare_exchange(QUEUED, CANCELLED, Ordering::AcqRel, Ordering::Acquire).is_ok(), 9)
});

struct Threadsafe {
    thread: std::thread::ThreadId,
    function: usize,
    call: Option<CallJs>,
    context: usize,
    finalize: Option<Finalize>,
    finalize_data: usize,
    max_queue: usize,
    queue: Mutex<VecDeque<usize>>,
    space: Condvar,
    threads: AtomicUsize,
    closing: AtomicBool,
    finalized: AtomicBool,
    referenced: AtomicBool,
    ready: Arc<Mutex<Vec<Ready>>>,
    wake: crate::host::Waker,
}
impl Threadsafe {
    // The flag changes under the queue lock, so a blocked caller can't miss the wakeup.
    fn close(&self) {
        let _queue = self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.closing.store(true, Ordering::Release);
        self.space.notify_all();
    }
    fn schedule(self: Arc<Self>) {
        let (ready, wake) = (self.ready.clone(), self.wake.clone());
        ready
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Ready::Threadsafe(self));
        wake.wake();
    }
}
enum Ready {
    Work(usize, i32),
    Threadsafe(Arc<Threadsafe>),
}
api!(napi_create_threadsafe_function(
    env,
    function: Ptr,
    _resource: Ptr,
    _name: Ptr,
    max_queue: usize,
    threads: usize,
    finalize_data: Ptr,
    finalize: Option<Finalize>,
    context: Ptr,
    call: Option<CallJs>,
    result: *mut Ptr,
) {
    check(!result.is_null() && threads > 0, 1)?;
    check(!function.is_null() || call.is_some(), 1)?;
    if !function.is_null() {
        check(env.kind(function)? == 7, 5)?;
        (env.api.protect)(env.ctx, function);
    }
    env.threadsafe.set(env.threadsafe.get() + 1);
    let function = Arc::new(Threadsafe {
        thread: env.thread,
        function: function as usize,
        call,
        context: context as usize,
        finalize,
        finalize_data: finalize_data as usize,
        max_queue,
        queue: Mutex::default(),
        space: Condvar::new(),
        threads: threads.into(),
        closing: false.into(),
        finalized: false.into(),
        referenced: true.into(),
        ready: env.ready.clone(),
        wake: crate::host::Waker::current(),
    });
    // Each acquiring thread and threadsafe_live own references. Keep the allocation alive after another
    // thread aborts.
    let raw = Arc::as_ptr(&function);
    for _ in 0..threads {
        Arc::increment_strong_count(raw);
    }
    env.threadsafe_live.borrow_mut().insert(raw as usize, function);
    *result = raw.cast_mut().cast();
    Ok(())
});
raw_api!(napi_get_threadsafe_function_context(function: *const Threadsafe, result: *mut Ptr) {
    if function.is_null() || result.is_null() {
        return 1;
    }
    unsafe { *result = (*function).context as Ptr };
    0
});
raw_api!(napi_call_threadsafe_function(function: *const Threadsafe, data: Ptr, mode: i32) {
    if function.is_null() {
        return 1;
    }
    let tsfn = unsafe { &*function };
    let mut queue = tsfn.queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    loop {
        if tsfn.closing.load(Ordering::Acquire) {
            return 16;
        }
        if tsfn.max_queue == 0 || queue.len() < tsfn.max_queue {
            break;
        }
        if mode == 0 {
            return 15;
        }
        if tsfn.thread == std::thread::current().id() {
            return 21;
        }
        queue = tsfn.space.wait(queue).unwrap_or_else(std::sync::PoisonError::into_inner);
    }
    queue.push_back(data as usize);
    drop(queue);
    unsafe {
        Arc::increment_strong_count(function);
        Arc::from_raw(function).schedule();
    }
    0
});
raw_api!(napi_acquire_threadsafe_function(function: *const Threadsafe) {
    if function.is_null() {
        return 1;
    }
    let tsfn = unsafe { &*function };
    if tsfn.closing.load(Ordering::Acquire) {
        return 16;
    }
    if tsfn.threads.try_update(Ordering::AcqRel, Ordering::Acquire, |count| (count > 0).then_some(count + 1)).is_err() {
        return 1;
    }
    unsafe { Arc::increment_strong_count(function) };
    0
});
raw_api!(napi_release_threadsafe_function(function: *const Threadsafe, mode: i32) {
    if function.is_null() {
        return 1;
    }
    let released = unsafe { &*function }.threads.try_update(Ordering::AcqRel, Ordering::Acquire, |count| count.checked_sub(1));
    let Ok(previous) = released else {
        return 1;
    };
    // This takes over the caller's reference, so the function stays alive until this call returns.
    let tsfn = unsafe { Arc::from_raw(function) };
    if mode == 1 {
        tsfn.close();
    }
    if previous == 1 || mode == 1 {
        tsfn.schedule();
    }
    0
});
api!(napi_unref_threadsafe_function(env, function: Ptr) {
    check(!function.is_null(), 1)?;
    if (*function.cast::<Threadsafe>()).referenced.swap(false, Ordering::AcqRel) {
        env.threadsafe.set(env.threadsafe.get().saturating_sub(1));
    }
    Ok(())
});
api!(napi_ref_threadsafe_function(env, function: Ptr) {
    check(!function.is_null(), 1)?;
    if !(*function.cast::<Threadsafe>()).referenced.swap(true, Ordering::AcqRel) {
        env.threadsafe.set(env.threadsafe.get() + 1);
    }
    Ok(())
});

impl State {
    fn scoped(&self, run: impl FnOnce()) {
        let depth = self.scopes.borrow().len();
        self.scopes.borrow_mut().push(Box::default());
        run();
        self.report();
        while self.scopes.borrow().len() > depth {
            self.pop();
        }
    }

    pub(crate) fn drain(self: &Rc<Self>) {
        for environment in self.environments.borrow().clone() {
            environment.drain();
        }
        let items = std::mem::take(
            &mut *self
                .ready
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for item in items {
            match item {
                Ready::Work(work, status) => {
                    self.active.set(self.active.get().saturating_sub(1));
                    let (complete, data) = unsafe {
                        let work = &*(work as *const AsyncWork);
                        work.state.store(IDLE, Ordering::Release);
                        (work.complete, work.data)
                    };
                    if let Some(complete) = complete {
                        self.scoped(|| unsafe { complete(Rc::as_ptr(self), status, data) });
                    }
                }
                Ready::Threadsafe(function) => self.run_threadsafe(&function),
            }
        }
    }

    fn run_threadsafe(&self, function: &Threadsafe) {
        // One function can have several entries here, so a later one may name a function that already ended.
        if function.finalized.load(Ordering::Acquire) {
            return;
        }
        while !function.closing.load(Ordering::Acquire) {
            let next = function
                .queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front();
            let Some(data) = next else { break };
            function.space.notify_one();
            let target = function.function as Ptr;
            self.scoped(|| unsafe {
                match function.call {
                    Some(call) => call(self, target, function.context as Ptr, data as Ptr),
                    None => {
                        let _ = self.js("call", &[target, self.undefined()]);
                    }
                }
            });
        }
        if function.threads.load(Ordering::Acquire) == 0 || function.closing.load(Ordering::Acquire)
        {
            let live = self
                .threadsafe_live
                .borrow_mut()
                .remove(&(ptr::from_ref(function) as usize));
            self.scoped(|| self.end_threadsafe(function));
            drop(live);
        }
    }

    // Pass queued values to the callback with a null environment at shutdown so it can free them.
    fn end_threadsafe(&self, function: &Threadsafe) {
        function.finalized.store(true, Ordering::Release);
        if function.referenced.load(Ordering::Acquire) {
            self.threadsafe.set(self.threadsafe.get().saturating_sub(1));
        }
        if let Some(finalize) = function.finalize {
            unsafe { finalize(self, function.finalize_data as Ptr, function.context as Ptr) };
        }
        let rest = std::mem::take(
            &mut *function
                .queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        if let Some(call) = function.call {
            for data in rest {
                unsafe {
                    call(
                        ptr::null(),
                        ptr::null_mut(),
                        function.context as Ptr,
                        data as Ptr,
                    )
                };
            }
        }
        if function.function != 0 {
            unsafe { (self.api.unprotect)(self.ctx, function.function as Ptr) };
        }
    }

    pub(crate) fn pending(&self) -> bool {
        self.active.get() > 0
            || self.threadsafe.get() > 0
            || self
                .environments
                .borrow()
                .iter()
                .any(|environment| environment.pending())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn callback(_: *const State, _: *const Info) -> Ptr {
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
        let env = State::new(api, root.ctx, &host, true).unwrap();
        let raw = Rc::as_ptr(&env);
        let object = env.js("object", &[]).unwrap();
        env.externals
            .borrow_mut()
            .insert(object as usize, (u64::MAX, raw.cast_mut().cast()));
        let (mut kind, mut data) = (0, ptr::null_mut());
        assert_eq!(unsafe { napi_typeof(raw, object, &mut kind) }, 0);
        assert_eq!(kind, 6, "A stale external address aliased a new object");
        assert_eq!(
            unsafe { napi_get_value_external(raw, object, &mut data) },
            1
        );
        env.function("live", callback, ptr::null_mut()).unwrap();
        let native = *env.callbacks.borrow().keys().next().unwrap();
        env.holders.borrow_mut().insert(u64::MAX, (native, 3));
        env.finalized.lock().unwrap().push(u64::MAX);
        env.sweep();
        assert!(
            env.callbacks.borrow().contains_key(&native),
            "A stale holder removed a live function"
        );
        let mut exception = ptr::null_mut();
        let value = unsafe {
            invoke(
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
