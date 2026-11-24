use crate::{
    host::Host,
    jsc::{
        self, Callback, JavaScriptCore, Ptr, Role,
        context::{Context as Engine, Runtime, runtime_sources},
        encoding, inspector_connect, inspector_disconnect, inspector_dispatch, payload,
        script_source,
    },
};
use anyhow::{Context, Result, bail};
use bbtree::unsync::BBTreeMap;
use std::{cell::RefCell, ptr, rc::Rc};

pub(super) struct Binding {
    pub(super) api: Rc<JavaScriptCore>,
    pub(super) host: Rc<RefCell<Host>>,
    pub(super) role: Role,
    pub(super) sqlite: RefCell<crate::node::sqlite::State>,
    pub(super) web: RefCell<crate::web::State>,
    pub(super) ffi: crate::node::ffi::State,
    pub(super) node: RefCell<crate::node::State>,
    pub(super) inspector: std::cell::Cell<Ptr>,
}
// The binding of a context of this thread.
pub(super) fn binding(ctx: Ptr) -> Option<Rc<Binding>> {
    jsc::data(ctx)
}

thread_local! {
    static ARMED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    // The render loop sets this, so engines without windows, like the CLI's, don't link the renderer.
    pub(crate) static REFLOW: std::cell::Cell<Option<fn(u64)>> = const { std::cell::Cell::new(None) };
    pub(crate) static CLIPBOARD: std::cell::Cell<Option<Clipboard>> = const { std::cell::Cell::new(None) };
    // The render thread sets this, because only the main thread can make native calls.
    pub(crate) static NATIVE: std::cell::Cell<Option<NativeCall>> = const { std::cell::Cell::new(None) };
    // The render thread must stop before statics go, so the main thread sets this.
    pub(crate) static BEFORE_EXIT: std::cell::Cell<Option<fn()>> = const { std::cell::Cell::new(None) };
}

type Clipboard = fn(Option<String>) -> Option<String>;
type NativeCall = fn(&str, &serde_json::Value) -> Result<serde_json::Value>;

pub(crate) fn contexts() -> Vec<(usize, Role)> {
    jsc::contexts::<Binding>()
        .into_iter()
        .map(|(ctx, binding)| (ctx as usize, binding.role))
        .collect()
}

pub(crate) fn connect_inspector(ctx: usize) -> Result<bool> {
    let binding = binding(ctx as Ptr).context("The JavaScript context is closed")?;
    if !binding.inspector.get().is_null() {
        return Ok(false);
    }
    binding.inspector.set(inspector_connect(ctx)?);
    Ok(true)
}

pub(crate) fn inspect(ctx: usize, message: &str) -> Result<()> {
    connect_inspector(ctx)?;
    inspector_dispatch(ctx, message)
}

unsafe extern "C" fn compile(
    ctx: Ptr,
    _function: Ptr,
    _this: Ptr,
    count: usize,
    args: *const Ptr,
    exception: *mut Ptr,
) -> Ptr {
    let Some(binding) = binding(ctx) else {
        return ptr::null_mut();
    };
    let api = &binding.api;
    // Without jsExecute, pages compile only while Rust evaluates one of their own scripts, never from later strings.
    if matches!(binding.role, Role::Renderer(_))
        && ARMED.get() != ctx as usize
        && !binding
            .host
            .try_borrow()
            .is_ok_and(|host| host.app.features.has(crate::bundle::Feature::JsExecute))
    {
        api.throw(
            ctx,
            exception,
            "EvalError: Code generation is disabled in pages",
        );
        return ptr::null_mut();
    }
    let args = if count == 0 || args.is_null() {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(args, count) }
    };
    api.compile(ctx, args, exception)
}

// This runs only scripts of the app bundle, by name, and never arms code generation.
unsafe extern "C" fn chunk(
    ctx: Ptr,
    _function: Ptr,
    _this: Ptr,
    count: usize,
    args: *const Ptr,
    exception: *mut Ptr,
) -> Ptr {
    let Some(binding) = binding(ctx) else {
        return ptr::null_mut();
    };
    let api = &binding.api;
    let name = if count == 1 {
        api.text(ctx, unsafe { *args })
    } else {
        String::new()
    };
    let source = binding
        .host
        .try_borrow()
        .ok()
        .and_then(|host| host.app.scripts.get(&name).cloned());
    match source.filter(|source| source.is_ascii() || std::str::from_utf8(source).is_ok()) {
        Some(source) => api.run_chunk(ctx, &name, source, exception),
        None => {
            api.throw(
                ctx,
                exception,
                &format!("Error: Missing bundled script: {name}"),
            );
            ptr::null_mut()
        }
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
    let Some(binding) = binding(ctx) else {
        return ptr::null_mut();
    };
    let (api, role, ffi) = (&binding.api, binding.role, &binding.ffi);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
        || -> Result<crate::node::ffi::Output> {
            anyhow::ensure!(count > 0, "Missing FFI request");
            let values = unsafe { std::slice::from_raw_parts(args, count) };
            if unsafe { (api.value_type)(ctx, values[0]) } == 3 {
                let id = unsafe { (api.to_number)(ctx, values[0], ptr::null_mut()) } as u64;
                return ffi
                    .invoke(api, ctx, role, id, &values[1..])
                    .map(crate::node::ffi::Output::Object);
            }
            let request = serde_json::from_str(&api.text(ctx, values[0]))?;
            ffi.call(api, ctx, role, &request, &values[1..])
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

unsafe extern "C" fn page_limit(ctx: Ptr, _: Ptr) -> bool {
    let Some(binding) = binding(ctx) else {
        return false;
    };
    let (api, role) = (&binding.api, binding.role);
    let (Role::Renderer(id) | Role::Preload(id)) = role else {
        return false;
    };
    // The limit ends with this call, so a script that goes on needs a new one.
    let again = || {
        api.limit_pages(page_limit);
        false
    };
    if stopping(id) {
        return true;
    }
    if !matches!(role, Role::Renderer(_)) {
        return again();
    }
    if api.over_limit() {
        return true;
    }
    again()
}

static STOPPING: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());
static STOP_ALL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn stopping(id: u64) -> bool {
    STOP_ALL.load(std::sync::atomic::Ordering::Acquire)
        || STOPPING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&id)
}

pub(crate) fn stop_page(id: u64) {
    STOPPING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(id);
}

pub(crate) fn page_stopped(id: u64) {
    STOPPING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|&other| other != id);
}

pub(crate) fn stop_pages() {
    STOP_ALL.store(true, std::sync::atomic::Ordering::Release);
}

struct Interrupt {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    api: Rc<JavaScriptCore>,
}

// Bundles run one entry script per call of this source, so callers loop until it returns `false`.
pub(crate) const NEXT: &str = "this.__appsNext?.()";

pub struct JsContext {
    engine: Engine,
    api: Rc<JavaScriptCore>,
    ctx: Ptr,
    napi: Option<Rc<crate::node::napi::Root>>,
    // Kept for the limit callback, which reads it.
    _interrupt: Option<Box<Interrupt>>,
}

impl JsContext {
    // A context whose host calls go to `router`, and whose workers run with `run_worker`.
    pub(super) fn create(
        api: Rc<JavaScriptCore>,
        host: Rc<RefCell<Host>>,
        role: Role,
        interrupt: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
        router: Callback,
        run_worker: crate::web::worker::Run,
    ) -> Result<Self> {
        let devtools = !host.borrow().app.packaged;
        let mut engine = Engine::new(api.clone(), &format!("apps {role:?}"), devtools)?;
        let ctx = engine.ptr();
        let binding = Binding {
            api: api.clone(),
            host: host.clone(),
            role,
            sqlite: RefCell::default(),
            web: RefCell::new(crate::web::State::new(
                matches!(role, Role::Renderer(_)),
                host.borrow().app.assets.clone(),
                run_worker,
            )),
            ffi: Default::default(),
            node: RefCell::default(),
            inspector: std::cell::Cell::new(ptr::null_mut()),
        };
        let callback_api = api.clone();
        binding
            .sqlite
            .borrow_mut()
            .set_authorizer_callback(Rc::new(move |id, action, args| {
                callback_api
                    .call_json(
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
            }));
        binding.web.borrow_mut().configure_storage(
            &host.borrow().app,
            match role {
                Role::Renderer(id) | Role::Preload(id) => Some(id),
                Role::Main => None,
            },
        );
        jsc::register(ctx, api.clone(), Rc::new(binding));
        let napi = if matches!(role, Role::Renderer(_)) {
            None
        } else {
            Some(crate::node::napi::Root::new(api.clone(), ctx, &host))
        };
        let interrupt = match interrupt {
            Some(stop) => {
                unsafe extern "C" fn interrupted(_: Ptr, context: Ptr) -> bool {
                    let state = unsafe { &*context.cast::<Interrupt>() };
                    if state.stop.load(std::sync::atomic::Ordering::Acquire) {
                        return true;
                    }
                    state.api.limit(0.01, interrupted, context);
                    false
                }
                let mut state = Box::new(Interrupt {
                    stop,
                    api: api.clone(),
                });
                api.limit(0.01, interrupted, (&mut *state as *mut Interrupt).cast());
                Some(state)
            }
            None => {
                if matches!(role, Role::Renderer(_)) {
                    // The limit is per context group, which every engine of this thread shares.
                    api.limit_pages(page_limit);
                }
                None
            }
        };
        encoding::install(
            &api,
            ctx,
            &[
                ("digest", crate::node::system::digest),
                ("url", crate::web::url_parts),
            ],
        );
        let natives = [
            ("__appsNative", router),
            ("__appsNow", jsc::context::now),
            ("__appsLoadAddon", crate::node::napi::load),
            ("__appsFFINative", ffi_call),
            ("__appsIsProxy", jsc::context::is_proxy),
        ];
        let count = if matches!(role, Role::Renderer(_)) {
            2
        } else {
            natives.len()
        };
        engine.install(true, &natives[..count]);
        if devtools || matches!(role, Role::Renderer(_)) {
            // Pages get the compiler only as `this` of their own scripts, see `eval_page`.
            if matches!(role, Role::Renderer(_)) {
                engine.add_scope(devtools);
            }
            engine.install(false, &[("__appsCompile", compile)]);
        }
        engine.install(false, &[("__appsChunk", chunk)]);
        let engine = Self {
            engine,
            api,
            ctx,
            napi,
            _interrupt: interrupt,
        };
        // Bootstrap scripts call into the host, so the host is only borrowed per check.
        let features = host.borrow().app.features;
        let disabled: Vec<_> = crate::bundle::Feature::ALL
            .into_iter()
            .filter(|&feature| !features.has(feature))
            .map(crate::bundle::Feature::name)
            .collect();
        engine.eval(
            &format!(
                "globalThis.__appsRole={};globalThis.__appsDisabled={};",
                serde_json::to_string(&role)?,
                serde_json::to_string(&disabled)?
            ),
            "apps:role",
        )?;
        engine.api.attach_aot(ctx);
        if !matches!(role, Role::Renderer(_)) {
            for (url, source) in runtime_sources(Runtime::Node) {
                engine.run(source(), url)?;
            }
        }
        for (url, source) in runtime_sources(Runtime::Web) {
            engine.run(source(), url)?;
        }
        engine.engine.take_hooks();
        engine.eval(
            "delete globalThis.__appsRejection;delete globalThis.__appsModuleLoader",
            "apps:rejections",
        )?;
        if matches!(role, Role::Renderer(_)) {
            for (url, source) in runtime_sources(Runtime::Page) {
                if *url != "apps:gpu" || features.has(crate::bundle::Feature::WebGpu) {
                    engine.run(source(), url)?;
                }
            }
            if !features.has(crate::bundle::Feature::JsExecute) {
                engine
                    .engine
                    .disable_eval("Code generation is disabled in pages");
            }
        }
        Ok(engine)
    }

    pub fn eval(&self, source: &str, url: &str) -> Result<String> {
        self.evaluate(source, url, false)
    }

    fn run(&self, source: &'static str, url: &'static str) -> Result<()> {
        let mut exception = ptr::null_mut();
        let result = self.engine.run(source, url, &mut exception);
        self.checked(url, result, exception).map(drop)
    }

    fn checked(&self, url: &str, result: Ptr, exception: Ptr) -> Result<Ptr> {
        if !exception.is_null() {
            let message = self.api.text(self.ctx, exception);
            self.log(url, &message);
            bail!("{message}");
        }
        anyhow::ensure!(!result.is_null(), "JSC returned no value");
        Ok(result)
    }

    pub fn eval_page(&self, source: &str, url: &str) -> Result<String> {
        struct Armed(usize);
        impl Drop for Armed {
            fn drop(&mut self) {
                ARMED.set(self.0);
            }
        }
        let _armed = Armed(ARMED.replace(self.ctx as usize));
        self.evaluate(source, url, self.engine.has_scope())
    }

    pub fn addons_pending(&self) -> bool {
        self.napi.as_ref().is_some_and(|napi| napi.pending())
    }

    pub fn call(&self, name: &str, arguments: &[&str]) -> Result<String> {
        self.api.call_json(self.ctx, name, arguments, |value| {
            self.api.text(self.ctx, value)
        })
    }

    fn evaluate(&self, source: &str, url: &str, scoped: bool) -> Result<String> {
        let mut exception = ptr::null_mut();
        let result = self.engine.evaluate(source, url, scoped, &mut exception)?;
        Ok(self
            .api
            .text(self.ctx, self.checked(url, result, exception)?))
    }

    // Calls a global function with JSON arguments. Unlike a script of new text, it compiles nothing.
    pub fn call_logged(&self, name: &str, arguments: &[&str], url: &str) -> Result<()> {
        self.api
            .call_json(self.ctx, name, arguments, drop)
            .inspect_err(|error| self.log(url, &error.to_string()))
    }

    fn log(&self, url: &str, message: &str) {
        if let Some(binding) = binding(self.ctx)
            && let Ok(mut host) = binding.host.try_borrow_mut()
            && !host.app.packaged
        {
            host.log(serde_json::json!({"type":"error","message":format!("{url}: {message}")}));
        }
    }

    pub(crate) fn receive(&self, payload: payload::Payload) -> Result<payload::Payload> {
        payload::receive(&self.api, self.ctx, payload)
    }

    pub(crate) fn deliver(&self, payload: payload::Payload) -> Result<()> {
        payload::bridge(&self.api, self.ctx, "__appsBridgeDeliver", payload).map(|_| ())
    }

    pub(crate) fn heap(&self) -> Result<serde_json::Value> {
        self.engine.heap()
    }

    pub fn collect_garbage(&self) {
        self.engine.collect_garbage();
    }

    pub fn tick(&self) -> Result<i64> {
        if let Some(napi) = &self.napi {
            napi.sweep();
            napi.drain();
        }
        // Run one macrotask per call so JSC drains microtasks between tasks. -2 signals a completed
        // macrotask.
        loop {
            let delay = self
                .api
                .call_json(self.ctx, "__appsTick", &[], |value| unsafe {
                    (self.api.to_number)(self.ctx, value, ptr::null_mut())
                })
                .inspect_err(|error| self.log("apps:tick", &error.to_string()))?;
            if delay != -2. {
                // Delays can be fractional, so round up or the loop wakes before the timer is due.
                return Ok(if delay.is_finite() {
                    delay.ceil() as i64
                } else {
                    -1
                });
            }
        }
    }

    pub fn validate_bundle(&self, files: &BBTreeMap<String, String>) -> Result<()> {
        self.install(files)?;
        self.eval_page(&script_source(files, false), "apps:validate")?;
        Ok(())
    }

    pub fn load_bundle(&self, files: &BBTreeMap<String, String>, entry: &str) -> Result<()> {
        self.load_main(files, &script_source(files, true), entry)
    }

    // `source` is a main script from `script_source`. A built app keeps it in its mapped bundle.
    pub fn load_main(
        &self,
        files: &BBTreeMap<String, String>,
        source: &str,
        entry: &str,
    ) -> Result<()> {
        self.install(files)?;
        self.eval_page(source, entry)
            .context("Cannot execute the app bundle")?;
        while self.eval_page(NEXT, entry)? == "true" {}
        Ok(())
    }

    fn install(&self, files: &BBTreeMap<String, String>) -> Result<()> {
        {
            let binding = binding(self.ctx).context("The JavaScript context is closed")?;
            let mut host = binding.host.borrow_mut();
            host.app.install(files)?;
            if !host.app.scripts.is_empty() {
                let window = match binding.role {
                    Role::Renderer(id) | Role::Preload(id) => Some(id),
                    Role::Main => None,
                };
                binding
                    .web
                    .borrow_mut()
                    .configure_storage(&host.app, window);
            }
        }
        Ok(())
    }
}

impl Drop for JsContext {
    fn drop(&mut self) {
        if let Some(napi) = &self.napi {
            napi.close();
        }
        let binding = jsc::forget(self.ctx).and_then(|data| data.downcast::<Binding>().ok());
        if let Some(binding) = &binding {
            if let Role::Preload(id) = binding.role
                && let Ok(mut host) = binding.host.try_borrow_mut()
            {
                host.bridge.remove(&id);
            }
            let closed = format!(
                "globalThis.__appsBridgeClosed?.({})",
                serde_json::to_string(&binding.role).unwrap()
            );
            for (ctx, peer) in jsc::contexts::<Binding>() {
                if Rc::ptr_eq(&binding.host, &peer.host) {
                    peer.api.evaluate_quietly(ctx, &closed);
                }
            }
        }
        if let Some(connection) = binding
            .map(|binding| binding.inspector.get())
            .filter(|connection| !connection.is_null())
        {
            // A connected inspector keeps the global object alive.
            inspector_disconnect(self.ctx, connection);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        host::ops::{access, dispatch},
        jsc::add_mapped,
    };

    fn page_host(id: u64, html: &str) -> Rc<RefCell<Host>> {
        let host = Rc::new(RefCell::new(Host::default()));
        host.borrow_mut().pages.insert(
            id,
            crate::host::Page {
                doc: crate::dom::Document::parse(html),
                dirty: false,
            },
        );
        host
    }

    #[test]
    fn access_follows_roles_and_prefixes() {
        let (main, preload, page) = (Role::Main, Role::Preload(1), Role::Renderer(1));
        let check = |role, op: &str| access(role, op).map_err(|error| error.to_string());
        for op in ["log", "bridge.dispatch", "web.fetch", "web."] {
            for role in [main, preload, page] {
                assert!(check(role, op).is_ok(), "{op}");
            }
        }
        for (op, roles) in [
            ("node.loop", [true, true, false]),
            ("cwd", [true, true, false]),
            ("window", [true, false, false]),
            ("app.x", [true, false, false]),
            ("native.dialog", [true, false, false]),
            ("native.clipboard.read", [true, true, false]),
            ("native.image", [true, true, false]),
            ("native.imagex", [true, false, false]),
            ("bridge.expose", [false, true, false]),
            ("bridge.exposures", [false, false, true]),
            ("dom.open", [false, true, true]),
            ("dom.openx", [false, false, true]),
            ("terminal.write", [false, true, true]),
            ("web.serve.start", [true, false, false]),
            ("web.serve", [true, true, true]),
            ("web.tcp.connect", [true, true, false]),
        ] {
            for (role, allowed) in [main, preload, page].into_iter().zip(roles) {
                assert_eq!(check(role, op).is_ok(), allowed, "{op} {role:?}");
            }
        }
        for op in ["", ".", "x", "dom", "log.x", "bridge.x", "nodex.loop"] {
            assert_eq!(
                check(main, op),
                Err(format!("Unsupported host operation: {op}"))
            );
        }
        assert_eq!(
            check(page, "node.loop"),
            Err("node.loop is unavailable in page scripts".into())
        );
        assert_eq!(
            check(preload, "window"),
            Err("window is only available in the main context".into())
        );
        assert_eq!(
            check(preload, "dom.x"),
            Err("Unsupported host operation: dom.x".into())
        );
    }

    #[test]
    fn bundled_chunks_keep_nul_bytes_and_non_ascii_text() {
        let host = Rc::new(RefCell::new(Host::default()));
        host.borrow_mut().app.scripts = std::sync::Arc::new(std::collections::HashMap::from([
            (
                "script-1.js".to_owned(),
                bytes::Bytes::from_static(
                    b"globalThis.a = `x\0y`.length; globalThis.f = function f() { return 1 };",
                ),
            ),
            (
                "script-2.js".to_owned(),
                bytes::Bytes::from("globalThis.b = '\u{4f60}\u{597d}';"),
            ),
        ]));
        let main = JsContext::new(JavaScriptCore::load().unwrap(), host, Role::Main).unwrap();
        let script = "__appsChunk('script-1.js'); __appsChunk('script-2.js'); \
            [a, f.toString(), b.length, b.charCodeAt(0)].join('|')";
        assert_eq!(
            main.eval(script, "test").unwrap(),
            "3|function f() { return 1 }|2|20320"
        );
    }

    #[test]
    fn mapped_scripts_run_in_place_and_keep_their_mapping() {
        let map = |source: &[u8]| {
            let mut map = memmap2::MmapMut::map_anon(source.len()).unwrap();
            map.copy_from_slice(source);
            let map = std::sync::Arc::new(map.make_read_only().unwrap());
            add_mapped(std::sync::Arc::downgrade(&map));
            map
        };
        let ascii = map(b"globalThis.f = function f() { return 'in place' }; f.toString()");
        let wide = map("globalThis.b = '\u{4f60}\u{597d}'; b.length".as_bytes());
        let main =
            JsContext::new(JavaScriptCore::load().unwrap(), Rc::default(), Role::Main).unwrap();
        let run = |source: &[u8]| main.eval(std::str::from_utf8(source).unwrap(), "test");
        assert_eq!(run(&ascii).unwrap(), "function f() { return 'in place' }");
        assert_eq!(run(&wide).unwrap(), "2");
        assert_eq!(std::sync::Arc::strong_count(&ascii), 2);
        assert_eq!(std::sync::Arc::strong_count(&wide), 1);
    }

    #[test]
    fn dispatched_clicks_run_activation_behavior() {
        let api = JavaScriptCore::load().unwrap();
        let host = page_host(1, "<body></body>");
        let page = JsContext::new(api, host, Role::Renderer(1)).unwrap();
        let script = "const box = document.createElement('input'); box.type = 'checkbox'; \
            document.body.append(box); const seen = []; box.addEventListener('change', () => seen.push('change')); \
            box.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true })); \
            const plain = document.createElement('input'); plain.type = 'checkbox'; \
            plain.dispatchEvent(new Event('click')); [box.checked, seen[0], plain.checked].join()";
        assert_eq!(page.eval(script, "test").unwrap(), "true,change,false");
    }

    #[test]
    fn channel_messages_are_tasks_and_file_readers_finish() {
        let api = JavaScriptCore::load().unwrap();
        let host = page_host(1, "<body></body>");
        let page = JsContext::new(api, host, Role::Renderer(1)).unwrap();
        let script = "globalThis.seen = []; const { port1, port2 } = new MessageChannel(); \
            port1.onmessage = () => seen.push('message'); port2.postMessage(0); \
            Promise.resolve().then(() => seen.push('microtask')); \
            const reader = new FileReader(); reader.onload = () => seen.push(reader.result); \
            reader.readAsDataURL(new Blob(['hi'], { type: 'text/plain' })); ''";
        page.eval(script, "test").unwrap();
        assert_eq!(page.eval("seen.join()", "test").unwrap(), "microtask");
        for _ in 0..200 {
            page.tick().unwrap();
            if page.eval("String(seen.length)", "test").unwrap() == "3" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(
            page.eval("seen.join()", "test").unwrap(),
            "microtask,message,data:text/plain;base64,aGk="
        );
    }

    #[test]
    fn contexts_isolate_privileges_and_share_dom_only() {
        let api = JavaScriptCore::load().unwrap();
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
        let host = page_host(1, "<body><p id='value'>before</p></body>");
        let main = JsContext::new(api.clone(), host.clone(), Role::Main).unwrap();
        let preload = JsContext::new(api.clone(), host.clone(), Role::Preload(1)).unwrap();
        let page = JsContext::new(api, host.clone(), Role::Renderer(1)).unwrap();
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
        let host = page_host(1, "<body></body>");
        host.borrow_mut().app.features = crate::bundle::Features::NONE;
        let page =
            JsContext::new(JavaScriptCore::load().unwrap(), host, Role::Renderer(1)).unwrap();
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
        let api = JavaScriptCore::load().unwrap();
        // The stop list is global, so this page has an id no other test uses.
        let host = page_host(900, "<body></body>");
        let main = JsContext::new(api.clone(), host.clone(), Role::Main).unwrap();
        let page = JsContext::new(api.clone(), host, Role::Renderer(900)).unwrap();
        api.limit(0.2, page_limit, ptr::null_mut());
        stop_page(900);
        assert!(page.eval("while(true){}", "test").is_err());
        page_stopped(900);
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
        let engine = JsContext::new(
            JavaScriptCore::load().unwrap(),
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
        engine
            .eval("globalThis.fired=0;setTimeout(()=>fired++,0)", "test")
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert_eq!(engine.tick().unwrap(), -1);
        assert_eq!(engine.eval("fired", "test").unwrap(), "1");
        engine
            .eval("Date.now=()=>0;setTimeout(()=>fired++,5.5)", "test")
            .unwrap();
        let delay = engine.tick().unwrap();
        assert!((1..=6).contains(&delay), "{delay}");
        std::thread::sleep(std::time::Duration::from_millis(delay as u64));
        assert_eq!(engine.eval("__appsDelay()", "test").unwrap(), "0");
        assert_eq!(engine.tick().unwrap(), -1);
        assert_eq!(engine.eval("fired", "test").unwrap(), "2");
        assert_eq!(
            engine
                .eval("try{queueMicrotask(1)}catch(e){e.constructor.name}", "test")
                .unwrap(),
            "TypeError"
        );
        engine
            .eval("queueMicrotask(()=>{throw new Error('boom')})", "test")
            .unwrap();
        assert!(engine.tick().unwrap_err().to_string().contains("boom"));
        engine
            .eval("process.on('uncaughtException',e=>globalThis.caught=e.message);queueMicrotask(()=>{throw new Error('again')})", "test")
            .unwrap();
        engine.tick().unwrap();
        assert_eq!(engine.eval("caught", "test").unwrap(), "again");
    }

    #[test]
    fn page_timers_clamp_deep_nesting() {
        let host = page_host(1, "<body></body>");
        let page =
            JsContext::new(JavaScriptCore::load().unwrap(), host, Role::Renderer(1)).unwrap();
        page.eval(
            "globalThis.runs=0;const loop=()=>{runs++;setTimeout(loop,0)};setTimeout(loop,0);queueMicrotask(()=>{throw new Error('reported')})",
            "test",
        )
        .unwrap();
        for _ in 0..5 {
            assert_eq!(page.tick().unwrap(), 0);
        }
        let started = std::time::Instant::now();
        let delay = page.tick().unwrap();
        assert!(
            delay >= 3 || started.elapsed() >= std::time::Duration::from_millis(3),
            "{delay}"
        );
        assert_eq!(page.eval("runs", "test").unwrap(), "6");
    }

    #[test]
    fn json_from_lone_surrogates_and_rooted_bytes() {
        let api = JavaScriptCore::load().unwrap();
        let host = page_host(1, "<body></body>");
        let preload = JsContext::new(api.clone(), host.clone(), Role::Preload(1)).unwrap();
        let page = JsContext::new(api.clone(), host, Role::Renderer(1)).unwrap();
        preload
            .eval(
                "__apps.contextBridge.exposeInMainWorld('api',{text:'a\\ud83d',echo:value=>value+'\\udc00'})",
                "test",
            )
            .unwrap();
        page.eval("__appsSyncBridge()", "test").unwrap();
        assert_eq!(
            page.eval("JSON.stringify([api.text, api.echo('b')])", "test")
                .unwrap(),
            "[\"a\u{fffd}\",\"b\u{fffd}\"]"
        );
        page.eval(
            "globalThis.items=[];Object.defineProperty(items,0,{get:()=>new Uint8Array(1<<16).fill(7)});Object.defineProperty(items,1,{get:()=>new Uint8Array(4)})",
            "test",
        )
        .unwrap();
        let items = unsafe {
            let name = api.string("items");
            let items = (api.get)(page.ctx, (api.global)(page.ctx), name, ptr::null_mut());
            (api.string_release)(name);
            items
        };
        let (buffers, roots) = payload::input(&api, page.ctx, items).unwrap();
        page.eval("delete globalThis.items", "test").unwrap();
        page.collect_garbage();
        page.eval(
            "globalThis.junk=Array.from({length:64},()=>new Uint8Array(1<<16).fill(1))",
            "test",
        )
        .unwrap();
        assert!(buffers[0].len() == 1 << 16 && buffers[0].iter().all(|&byte| byte == 7));
        drop(roots);
    }

    #[test]
    fn preload_document_handles_are_released() {
        let api = JavaScriptCore::load().unwrap();
        let host = page_host(1, "<body></body>");
        let preload = JsContext::new(api.clone(), host.clone(), Role::Preload(1)).unwrap();
        let page = JsContext::new(api, host, Role::Renderer(1)).unwrap();
        page.eval("globalThis.made=[];const create=document.createElement.bind(document);document.createElement=name=>{const node=create(name);made.push(new WeakRef(node));return node}", "test").unwrap();
        preload
            .eval("(()=>{for(let i=0;i<1000;i++)document.createElement('div').id='x'+i})();document.body===document.body", "test")
            .map(|same| assert_eq!(same, "true"))
            .unwrap();
        let live = || {
            page.eval("made.filter(made=>made.deref()).length", "test")
                .unwrap()
                .parse::<usize>()
                .unwrap()
        };
        assert_eq!(live(), 1000);
        for _ in 0..20 {
            preload.collect_garbage();
            preload.tick().unwrap();
            page.collect_garbage();
            page.eval("__appsDomSweep?.(true)", "test").unwrap();
            if live() < 500 {
                break;
            }
        }
        assert!(live() < 500, "{}", live());
        assert_eq!(
            preload.eval("document.body.tagName", "test").unwrap(),
            "BODY"
        );
    }

    #[test]
    fn replaced_rows_free_without_gc() {
        let api = JavaScriptCore::load().unwrap();
        let host = page_host(1, "<body><div id=list></div></body>");
        let page = JsContext::new(api, host.clone(), Role::Renderer(1)).unwrap();
        assert_eq!(
            page.eval("customElements.define('x-row', class extends HTMLElement {});const list = document.getElementById('list');for (let i = 0; i < 50; i++) { list.innerHTML = '<div><span>a</span><span>b</span></div>'.repeat(20); list.insertAdjacentHTML('beforeend', '<p><b>c</b></p>'.repeat(20)); __appsDomSweep(); }const kept = list.firstChild, observer = new MutationObserver(() => {});observer.observe(list, { childList: true });list.innerHTML = '<i>x</i>';const records = observer.takeRecords();__appsDomSweep();[kept.textContent, kept.isConnected, records.length, records[0].removedNodes.length, records[0].removedNodes[0] === kept, list.textContent].join()", "test").unwrap(),
            "ab,false,1,40,true,x"
        );
        let host = host.borrow();
        let used = host.pages[&1].doc.nodes.len() - host.dom[&1].free.len();
        assert!(used < 400, "{used}");
    }

    #[test]
    fn resize_observers_see_style_changes_after_the_next_layout() {
        let api = JavaScriptCore::load().unwrap();
        let host = page_host(
            1,
            "<body><div id=box style='width:100px;padding:5px'></div></body>",
        );
        let page = JsContext::new(api, host.clone(), Role::Renderer(1)).unwrap();
        let id: usize = page
            .eval("const box = document.getElementById('box');globalThis.seen = [];new ResizeObserver(entries => seen.push(entries[0].contentRect.width)).observe(box);box._id", "test")
            .unwrap()
            .parse()
            .unwrap();
        let layout = || {
            let mut host = host.borrow_mut();
            let bounds = &mut host.dom.entry(1).or_default().bounds;
            bounds.insert(id, [0., 0., 110., 10.]);
            bounds.touch();
        };
        layout();
        page.eval("__appsLayout()", "test").unwrap();
        page.eval(
            "box.style.padding = '10px'; box.style.width = '90px'; __appsLayout()",
            "test",
        )
        .unwrap();
        page.eval("getComputedStyle(box).paddingLeft", "test")
            .unwrap();
        layout();
        assert_eq!(
            page.eval("__appsLayout(); seen.join()", "test").unwrap(),
            "100,90"
        );
    }

    #[test]
    fn contexts_reach_only_their_host_operations() {
        for (role, op, allowed) in [
            (Role::Main, "native.poll", true),
            (Role::Main, "dom.batch", false),
            (Role::Main, "unknown", false),
            (Role::Preload(1), "native.poll", false),
            (Role::Preload(1), "native.menu.popup", false),
            (Role::Preload(1), "native.shortcut.register", false),
            (Role::Preload(1), "native.theme.set", false),
            (Role::Preload(1), "native.clipboard.readText", true),
            (Role::Preload(1), "native.shell.openExternal", true),
            (Role::Preload(1), "native.image", true),
            (Role::Preload(1), "window", false),
            (Role::Preload(1), "app.path", false),
            (Role::Preload(1), "browser.create", false),
            (Role::Preload(1), "web.serve.listen", false),
            (Role::Preload(1), "node.fs.read", true),
            (Role::Preload(1), "info", true),
            (Role::Renderer(1), "native.clipboard.readText", false),
            (Role::Renderer(1), "node.fs.read", false),
            (Role::Renderer(1), "child.spawn", false),
            (Role::Renderer(1), "pty.spawn", false),
            (Role::Renderer(1), "web.tcp.connect", false),
            (Role::Renderer(1), "web.fetch", true),
            (Role::Renderer(1), "infox", false),
        ] {
            assert_eq!(access(role, op).is_ok(), allowed, "{role:?} {op}");
        }
        let api = JavaScriptCore::load().unwrap();
        let host = page_host(1, "<body></body>");
        let preload = JsContext::new(api.clone(), host.clone(), Role::Preload(1)).unwrap();
        assert_eq!(
            preload
                .eval(
                    "['app','BrowserWindow','webContents','session','dialog','Menu','Tray','globalShortcut','nativeTheme','screen','systemPreferences','powerMonitor','powerSaveBlocker','safeStorage','Notification'].filter(name=>__apps[name]!==undefined).join()",
                    "test"
                )
                .unwrap(),
            ""
        );
        assert_eq!(
            preload
                .eval(
                    "['clipboard','shell','nativeImage','contextBridge','ipcRenderer','webUtils','webFrame'].filter(name=>!__apps[name]).join()",
                    "test"
                )
                .unwrap(),
            ""
        );
        let page = JsContext::new(api, host, Role::Renderer(1)).unwrap();
        let binding = binding(page.ctx).unwrap();
        let open = |args| {
            dispatch(&binding, page.ctx, "dom.open", args)
                .unwrap_err()
                .to_string()
        };
        for args in [
            serde_json::json!(["webview", "call", 1, {"method":"constructor","args":[]}]),
            serde_json::json!(["webview", "call", 1, {"method":"getURL","args":[],"reply":"1)"}]),
            serde_json::json!(["webview", "call", "1", {"method":"getURL","args":[]}]),
            serde_json::json!(["closed", "1"]),
        ] {
            assert_eq!(open(args.clone()), "Invalid window request", "{args}");
        }
        assert_eq!(
            open(serde_json::json!(["webview", "call", 1, {"method":"getURL","args":[]}])),
            "The main process is unavailable"
        );
        let picker = |picker| {
            dispatch(
                &binding,
                page.ctx,
                "dom.filePicker",
                serde_json::json!([picker]),
            )
            .is_ok()
        };
        assert!(!picker(
            serde_json::json!({"id":"1);evil(","multiple":false,"directory":false,"accept":""})
        ));
        assert!(picker(
            serde_json::json!({"id":1,"multiple":false,"directory":false,"accept":""})
        ));
        let popup = |items| {
            dispatch(
                &binding,
                page.ctx,
                "dom.selectPopup",
                serde_json::json!([{"id":1,"x":0,"y":0,"width":9,"selected":-1,"items":items}]),
            )
            .is_ok()
        };
        assert!(!popup(serde_json::json!([["a", "0);evil("]])));
        assert!(popup(serde_json::json!([["a", 1], null])));
        for op in ["node.poll", "child.spawn", "pty.spawn"] {
            let error = dispatch(&binding, page.ctx, op, serde_json::json!([])).unwrap_err();
            assert!(
                error.to_string().contains("unavailable in page scripts"),
                "{op}"
            );
        }
    }
}
