use super::{
    Callback, JavaScriptCore, NODE_SCRIPTS, PAGE_SCRIPTS, PAGE_TICK, Ptr, Source, WEB_SCRIPTS,
    bundled, in_place, runtime_bytecode, well_formed,
};
use anyhow::{Result, ensure};
use std::{ptr, rc::Rc};

// The runtime scripts that a context runs at its start.
pub(crate) enum Runtime {
    Node,
    Web,
    Page,
}

pub(crate) fn runtime_sources(set: Runtime) -> &'static [Source] {
    match set {
        Runtime::Node => &NODE_SCRIPTS,
        Runtime::Web => &WEB_SCRIPTS,
        Runtime::Page => &PAGE_SCRIPTS,
    }
}

// A JavaScript context of an engine: its global object, and an optional scope object that is `this` of the
// scripts that the app runs in it. Dropping it releases the context.
pub(crate) struct Context {
    api: Rc<JavaScriptCore>,
    ctx: Ptr,
    scope: Ptr,
}

impl Context {
    pub(crate) fn new(api: Rc<JavaScriptCore>, name: &str, inspectable: bool) -> Result<Self> {
        let ctx = unsafe { (api.create)(api.group, ptr::null_mut()) };
        ensure!(!ctx.is_null(), "Cannot create a JavaScriptCore context");
        unsafe {
            (api.inspectable)(ctx, inspectable);
            let name = api.string(name);
            (api.set_name)(ctx, name);
            (api.string_release)(name);
        }
        Ok(Self {
            api,
            ctx,
            scope: ptr::null_mut(),
        })
    }

    pub(crate) fn api(&self) -> &Rc<JavaScriptCore> {
        &self.api
    }

    pub(crate) fn ptr(&self) -> Ptr {
        self.ctx
    }

    // Makes the scope object, with a `__appsDevtools` flag.
    pub(crate) fn add_scope(&mut self, devtools: bool) {
        let api = &self.api;
        unsafe {
            self.scope = (api.object_make)(self.ctx, ptr::null_mut(), ptr::null_mut());
            (api.protect)(self.ctx, self.scope);
            let flag = api.string("__appsDevtools");
            (api.set)(
                self.ctx,
                self.scope,
                flag,
                (api.boolean)(self.ctx, devtools),
                0,
                ptr::null_mut(),
            );
            (api.string_release)(flag);
        }
    }

    pub(crate) fn has_scope(&self) -> bool {
        !self.scope.is_null()
    }

    // Puts native functions on the scope object, or on the global object without a scope or with `global`.
    pub(crate) fn install(&self, global: bool, functions: &[(&str, Callback)]) {
        let api = &self.api;
        unsafe {
            let target = if global || self.scope.is_null() {
                (api.global)(self.ctx)
            } else {
                self.scope
            };
            for &(name, callback) in functions {
                let name = api.string(name);
                let function = (api.function)(self.ctx, name, callback);
                (api.set)(self.ctx, target, name, function, 0, ptr::null_mut());
                (api.string_release)(name);
            }
        }
    }

    // The bootstrap scripts leave `__appsRejection` and `__appsModuleLoader` on the global object for this.
    pub(crate) fn take_hooks(&self) {
        let api = &self.api;
        unsafe {
            let global = (api.global)(self.ctx);
            let name = api.string("__appsRejection");
            let callback = (api.get)(self.ctx, global, name, ptr::null_mut());
            (api.rejection_callback)(self.ctx, callback, ptr::null_mut());
            (api.string_release)(name);
            let name = api.string("__appsModuleLoader");
            (api.module_loader)(self.ctx, (api.get)(self.ctx, global, name, ptr::null_mut()));
            (api.string_release)(name);
        }
    }

    pub(crate) fn disable_eval(&self, message: &str) {
        let message = self.api.string(message);
        unsafe {
            (self.api.eval_enabled)(self.ctx, false, message);
            (self.api.string_release)(message);
        }
    }

    // Runs a runtime script, compiled ahead of time or from cached bytecode.
    pub(crate) fn run(&self, source: &'static str, url: &'static str, exception: &mut Ptr) -> Ptr {
        let api = &self.api;
        if let Some(module) = api.runtime_module(url) {
            return api.timed(|| api.run_aot(self.ctx, module, ptr::null_mut(), exception));
        }
        let bytecode = runtime_bytecode(url, source);
        let source_url = api.string(url);
        let result = api.timed(|| unsafe {
            api.evaluate_latin1(
                self.ctx,
                source.as_bytes(),
                ptr::null_mut(),
                source_url,
                bytecode,
                exception,
            )
        });
        unsafe { (api.string_release)(source_url) };
        result
    }

    // Runs a script of the app, with the scope as `this` if `scoped`.
    pub(crate) fn evaluate(
        &self,
        source: &str,
        url: &str,
        scoped: bool,
        exception: &mut Ptr,
    ) -> Result<Ptr> {
        let api = &self.api;
        let this = if scoped { self.scope } else { ptr::null_mut() };
        let compiled = api.compiled_module(source.as_bytes())?;
        let source_url = api.string(url);
        let result = api.timed(|| {
            let bytecode = bundled(source.as_bytes());
            match compiled {
                Some(module) => api.run_aot(self.ctx, module, this, exception),
                None if bytecode.is_some() || in_place(source.as_bytes()) => unsafe {
                    api.evaluate_latin1(
                        self.ctx,
                        source.as_bytes(),
                        this,
                        source_url,
                        bytecode.unwrap_or_default(),
                        exception,
                    )
                },
                _ => {
                    let script = api.string(source);
                    let result =
                        unsafe { (api.evaluate)(self.ctx, script, this, source_url, 1, exception) };
                    unsafe { (api.string_release)(script) };
                    result
                }
            }
        });
        unsafe { (api.string_release)(source_url) };
        Ok(result)
    }

    pub(crate) fn heap(&self) -> Result<serde_json::Value> {
        let api = &self.api;
        unsafe {
            let json = (api.json_string)(self.ctx, (api.memory)(self.ctx), 0, ptr::null_mut());
            ensure!(!json.is_null(), "Cannot read heap statistics");
            let text = api.text(self.ctx, (api.make_string)(self.ctx, json));
            (api.string_release)(json);
            Ok(serde_json::from_str(&text)?)
        }
    }

    pub(crate) fn collect_garbage(&self) {
        unsafe { (self.api.garbage_collect)(self.ctx) };
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        unsafe {
            if !self.scope.is_null() {
                (self.api.unprotect)(self.ctx, self.scope);
            }
            (self.api.release)(self.ctx);
        }
    }
}

// The engine operations for callbacks and contexts that are known only by their pointer.
impl JavaScriptCore {
    // Stops the scripts of all contexts of the group when `callback` returns true, or after `seconds` asks it
    // again.
    pub(crate) fn limit(
        &self,
        seconds: f64,
        callback: unsafe extern "C" fn(Ptr, Ptr) -> bool,
        data: Ptr,
    ) {
        unsafe { (self.execution_limit)(self.group, seconds, Some(callback), data) };
    }

    // The limit of page scripts, checked each watchdog tick.
    pub(crate) fn limit_pages(&self, callback: unsafe extern "C" fn(Ptr, Ptr) -> bool) {
        self.limit(PAGE_TICK, callback, ptr::null_mut());
    }

    // Compiles and runs the source and URL in `args`, like `eval` of the context.
    pub(crate) fn compile(&self, ctx: Ptr, args: &[Ptr], exception: *mut Ptr) -> Ptr {
        let [source, url] = args else {
            return unsafe { (self.undefined)(ctx) };
        };
        unsafe {
            let source = (self.to_string)(ctx, *source, exception);
            if source.is_null() {
                return ptr::null_mut();
            }
            let url = (self.to_string)(ctx, *url, exception);
            if url.is_null() {
                (self.string_release)(source);
                return ptr::null_mut();
            }
            let result = (self.evaluate)(ctx, source, ptr::null_mut(), url, 1, exception);
            (self.string_release)(source);
            (self.string_release)(url);
            result
        }
    }

    // Runs a script chunk of the bundle. The bytes of an ASCII chunk stay alive, because JSC reads them in place.
    pub(crate) fn run_chunk(
        &self,
        ctx: Ptr,
        name: &str,
        source: bytes::Bytes,
        exception: *mut Ptr,
    ) -> Ptr {
        match self.compiled_module(&source) {
            Ok(Some(module)) => return self.run_aot(ctx, module, ptr::null_mut(), exception),
            Ok(None) => {}
            Err(error) => {
                self.throw(ctx, exception, &format!("Error: {error:#}"));
                return ptr::null_mut();
            }
        }
        let url = self.string(&format!("apps:///{name}"));
        // JSEvaluateScript copies source and uses UTF-16 for NUL or non-ASCII input.
        let result = if source.is_ascii() {
            let bytecode = bundled(&source).unwrap_or_default();
            let source = std::mem::ManuallyDrop::new(source);
            unsafe {
                self.evaluate_latin1(ctx, &source[..], ptr::null_mut(), url, bytecode, exception)
            }
        } else {
            let script = self.string(&String::from_utf8_lossy(&source));
            let result =
                unsafe { (self.evaluate)(ctx, script, ptr::null_mut(), url, 1, exception) };
            unsafe { (self.string_release)(script) };
            result
        };
        unsafe { (self.string_release)(url) };
        result
    }

    // Runs a script and drops its result and error.
    pub(crate) fn evaluate_quietly(&self, ctx: Ptr, source: &str) {
        let script = self.string(source);
        unsafe {
            (self.evaluate)(
                ctx,
                script,
                ptr::null_mut(),
                ptr::null_mut(),
                1,
                ptr::null_mut(),
            );
            (self.string_release)(script);
        }
    }

    // A `ShadowRealm`: a new global of this VM, with the module loader of its creator. Returns its `eval`, which
    // keeps it alive.
    pub(crate) fn realm(&self, loader: Ptr) -> Ptr {
        let name = self.string("eval");
        unsafe {
            let realm = (self.create)(self.group, ptr::null_mut());
            (self.module_loader)(realm, loader);
            let eval = (self.get)(realm, (self.global)(realm), name, ptr::null_mut());
            (self.string_release)(name);
            (self.release)(realm);
            eval
        }
    }

    // Sets an `Error` with `message`, and `code` if there is one, as the exception of a call.
    pub(crate) fn throw_error(
        &self,
        ctx: Ptr,
        exception: *mut Ptr,
        message: &str,
        code: Option<&str>,
    ) {
        unsafe {
            let message = self
                .utf8(ctx, message.as_bytes())
                .unwrap_or_else(|_| (self.undefined)(ctx));
            let error = (self.make_error)(ctx, 1, &message, ptr::null_mut());
            if let Some(code) = code {
                let key = self.string("code");
                let text = self.string(code);
                (self.set)(
                    ctx,
                    error,
                    key,
                    (self.make_string)(ctx, text),
                    0,
                    ptr::null_mut(),
                );
                (self.string_release)(key);
                (self.string_release)(text);
            }
            if !exception.is_null() {
                *exception = error;
            }
        }
    }

    // The text of a JSON string from a script, without lone surrogates.
    pub(crate) fn json(&self, ctx: Ptr, value: Ptr) -> String {
        let mut json = self.text(ctx, value);
        well_formed(&mut json);
        json
    }

    pub(crate) fn array(&self, ctx: Ptr, items: &[Ptr]) -> Ptr {
        unsafe { (self.make_array)(ctx, items.len(), items.as_ptr(), ptr::null_mut()) }
    }

    // The JSC type of a value: 2 boolean, 3 number, 4 string, 5 object, 7 BigInt.
    pub(crate) fn kind(&self, ctx: Ptr, value: Ptr) -> u32 {
        unsafe { (self.value_type)(ctx, value) }
    }

    pub(crate) fn to_f64(&self, ctx: Ptr, value: Ptr) -> f64 {
        unsafe { (self.to_number)(ctx, value, ptr::null_mut()) }
    }

    pub(crate) fn to_bool(&self, ctx: Ptr, value: Ptr) -> bool {
        unsafe { (self.to_boolean)(ctx, value) }
    }

    pub(crate) fn is_callable(&self, ctx: Ptr, value: Ptr) -> bool {
        unsafe { (self.is_function)(ctx, value) }
    }

    pub(crate) fn make_number(&self, ctx: Ptr, value: f64) -> Ptr {
        unsafe { (self.number)(ctx, value) }
    }

    pub(crate) fn make_undefined(&self, ctx: Ptr) -> Ptr {
        unsafe { (self.undefined)(ctx) }
    }

    pub(crate) fn make_null(&self, ctx: Ptr) -> Ptr {
        unsafe { (self.null)(ctx) }
    }

    pub(crate) fn make_text(&self, ctx: Ptr, text: &str) -> Ptr {
        let string = self.string(text);
        unsafe {
            let value = (self.make_string)(ctx, string);
            (self.string_release)(string);
            value
        }
    }

    // An `ArrayBuffer` over memory that `release` frees with `owner`.
    pub(crate) fn external_buffer(
        &self,
        ctx: Ptr,
        bytes: Ptr,
        size: usize,
        release: unsafe extern "C" fn(Ptr, Ptr),
        owner: Ptr,
    ) -> Ptr {
        unsafe { (self.make_buffer)(ctx, bytes, size, Some(release), owner, ptr::null_mut()) }
    }

    // Native memory that wrappers of the context hold, so GC starts sooner.
    pub(crate) fn add_memory(&self, ctx: Ptr, bytes: usize) {
        unsafe { (self.extra_memory)(ctx, bytes) };
    }

    #[cfg(test)]
    pub(crate) fn global_value(&self, ctx: Ptr, name: &str) -> Ptr {
        let name = self.string(name);
        unsafe {
            let value = (self.get)(ctx, (self.global)(ctx), name, ptr::null_mut());
            (self.string_release)(name);
            value
        }
    }

    pub(crate) fn is_proxy(&self, ctx: Ptr, value: Ptr) -> bool {
        unsafe { (self.value_type)(ctx, value) == 5 && !(self.proxy_target)(value).is_null() }
    }
}

// The engine's own globals.
pub(crate) unsafe extern "C" fn now(
    ctx: Ptr,
    _function: Ptr,
    _this: Ptr,
    _count: usize,
    _args: *const Ptr,
    _exception: *mut Ptr,
) -> Ptr {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let Some(api) = super::engine(ctx) else {
        return ptr::null_mut();
    };
    let elapsed = START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64();
    unsafe { (api.number)(ctx, elapsed * 1000.) }
}

pub(crate) unsafe extern "C" fn is_proxy(
    ctx: Ptr,
    _function: Ptr,
    _this: Ptr,
    count: usize,
    args: *const Ptr,
    _exception: *mut Ptr,
) -> Ptr {
    let Some(api) = super::engine(ctx) else {
        return ptr::null_mut();
    };
    let proxy = count > 0 && api.is_proxy(ctx, unsafe { *args });
    unsafe { (api.boolean)(ctx, proxy) }
}
