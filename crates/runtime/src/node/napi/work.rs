use super::{Env, State, Status, check, objects::Finalize};
use crate::jsc::{JavaScriptCore, Ptr};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    ptr,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
    },
};

type AsyncExecute = unsafe extern "C" fn(Env, Ptr);
type AsyncComplete = unsafe extern "C" fn(Env, i32, Ptr);
type CallJs = unsafe extern "C" fn(Env, Ptr, Ptr, Ptr);

const IDLE: u8 = 0;
const QUEUED: u8 = 1;
const RUNNING: u8 = 2;
const CANCELLED: u8 = 3;
// `execute` has returned, and `complete` has not run yet.
const DONE: u8 = 4;

type Queue = Arc<Mutex<Vec<Ready>>>;

pub(super) enum Ready {
    Work(usize, i32),
    Threadsafe(Arc<Threadsafe>),
}

// Work on the pool and thread-safe functions: what they send back runs on the thread of the environment.
#[derive(Default)]
pub(super) struct Tasks {
    ready: Queue,
    // The states of the work that the pool has, or had.
    works: RefCell<Vec<Arc<AtomicU8>>>,
    live: RefCell<HashMap<usize, Arc<Threadsafe>>>,
}

impl Tasks {
    pub(super) fn take_ready(&self) -> Vec<Ready> {
        std::mem::take(
            &mut *self
                .ready
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    pub(super) fn clear_ready(&self) {
        self.ready
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    pub(super) fn pending(&self) -> bool {
        self.works
            .borrow()
            .iter()
            .any(|state| state.load(Ordering::Acquire) != IDLE)
            || self
                .live
                .borrow()
                .values()
                .any(|function| function.referenced.load(Ordering::Acquire))
    }

    // Work that has not started never starts. Returns false if running work did not end within a second.
    pub(super) fn cancel(&self) -> bool {
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
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        true
    }

    pub(super) fn take_live(&self) -> Vec<Arc<Threadsafe>> {
        std::mem::take(&mut *self.live.borrow_mut())
            .into_values()
            .collect()
    }

    pub(super) fn remove_live(&self, function: &Threadsafe) -> Option<Arc<Threadsafe>> {
        self.live
            .borrow_mut()
            .remove(&(ptr::from_ref(function) as usize))
    }

    fn queue(&self, work: *const AsyncWork, env: Env) -> Status {
        let item = unsafe { &*work };
        check(
            item.state
                .compare_exchange(IDLE, QUEUED, Ordering::AcqRel, Ordering::Acquire)
                .is_ok(),
            1,
        )?;
        let (execute, data, state) = (item.execute, item.data as usize, item.state.clone());
        let (ready, wake, raw, work) = (
            self.ready.clone(),
            crate::host::Waker::current(),
            env as usize,
            work as usize,
        );
        crate::node::blocking(move || {
            let run = state
                .compare_exchange(QUEUED, RUNNING, Ordering::AcqRel, Ordering::Acquire)
                .is_ok();
            if run {
                unsafe { execute(raw as Env, data as Ptr) };
                state.store(DONE, Ordering::Release);
            }
            ready
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(Ready::Work(work, if run { 0 } else { 11 }));
            wake.wake();
        })
        .map_err(|_| {
            item.state.store(IDLE, Ordering::Release);
            9
        })?;
        let mut works = self.works.borrow_mut();
        works.retain(|state| state.load(Ordering::Acquire) != IDLE);
        works.push(item.state.clone());
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn threadsafe(
        &self,
        thread: std::thread::ThreadId,
        function: Ptr,
        call: Option<CallJs>,
        context: Ptr,
        finalize: Option<Finalize>,
        finalize_data: Ptr,
        max_queue: usize,
        threads: usize,
    ) -> *const Threadsafe {
        let function = Arc::new(Threadsafe {
            thread,
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
            ready: self.ready.clone(),
            wake: crate::host::Waker::current(),
        });
        // Each acquiring thread and `live` own references. Keep the allocation alive after another thread aborts.
        let raw = Arc::as_ptr(&function);
        for _ in 0..threads {
            unsafe { Arc::increment_strong_count(raw) };
        }
        self.live.borrow_mut().insert(raw as usize, function);
        raw
    }
}

pub(super) struct AsyncWork {
    execute: AsyncExecute,
    complete: Option<AsyncComplete>,
    data: Ptr,
    state: Arc<AtomicU8>,
}

impl AsyncWork {
    // The work ended: its `complete` callback and data.
    pub(super) fn finish(&self) -> (Option<AsyncComplete>, Ptr) {
        self.state.store(IDLE, Ordering::Release);
        (self.complete, self.data)
    }

    fn cancel(&self) -> Status {
        check(
            self.state
                .compare_exchange(QUEUED, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
                .is_ok(),
            9,
        )
    }
}

pub(super) struct Threadsafe {
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
    ready: Queue,
    wake: crate::host::Waker,
}

impl Threadsafe {
    // The flag changes under the queue lock, so a blocked caller can't miss the wakeup.
    pub(super) fn close(&self) {
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

    pub(super) fn ended(&self) -> bool {
        self.finalized.load(Ordering::Acquire)
    }

    // The next queued value, unless the function is closing.
    pub(super) fn next(&self) -> Option<usize> {
        if self.closing.load(Ordering::Acquire) {
            return None;
        }
        let next = self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front();
        if next.is_some() {
            self.space.notify_one();
        }
        next
    }

    // Calls `call` with a value. Without `call`, returns the JavaScript function to call.
    pub(super) unsafe fn call(&self, env: Env, data: usize) -> Option<Ptr> {
        let target = self.function as Ptr;
        match self.call {
            Some(call) => {
                unsafe { call(env, target, self.context as Ptr, data as Ptr) };
                None
            }
            None => Some(target),
        }
    }

    // All threads released the function, or one aborted it.
    pub(super) fn released(&self) -> bool {
        self.threads.load(Ordering::Acquire) == 0 || self.closing.load(Ordering::Acquire)
    }

    // Pass queued values to the callback with a null environment at shutdown so it can free them.
    pub(super) fn end(&self, env: Env, api: &JavaScriptCore, ctx: Ptr) {
        self.finalized.store(true, Ordering::Release);
        if let Some(finalize) = self.finalize {
            unsafe { finalize(env, self.finalize_data as Ptr, self.context as Ptr) };
        }
        let rest = std::mem::take(
            &mut *self
                .queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        if let Some(call) = self.call {
            for data in rest {
                unsafe {
                    call(
                        ptr::null(),
                        ptr::null_mut(),
                        self.context as Ptr,
                        data as Ptr,
                    )
                };
            }
        }
        if self.function != 0 {
            unsafe { (api.unprotect)(ctx, self.function as Ptr) };
        }
    }

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
}

impl State {
    api!(napi_create_async_work(
        env,
        _resource: Ptr,
        _name: Ptr,
        execute: Option<AsyncExecute>,
        complete: Option<AsyncComplete>,
        data: Ptr,
        result: *mut Ptr,
    ) {
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
        check(!work.is_null(), 1)?;
        drop(Box::from_raw(work.cast::<AsyncWork>()));
        Ok(())
    });
    api!(napi_queue_async_work(env, work: Ptr) {
        check(!work.is_null(), 1)?;
        env.tasks.queue(work.cast(), env.env())
    });
    api!(napi_cancel_async_work(env, work: Ptr) {
        check(!work.is_null(), 1)?;
        (*work.cast::<AsyncWork>()).cancel()
    });
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
        let raw = env.tasks.threadsafe(env.thread, function, call, context, finalize, finalize_data, max_queue, threads);
        *result = raw.cast_mut().cast();
        Ok(())
    });
    api!(napi_unref_threadsafe_function(env, function: Ptr) {
        check(!function.is_null(), 1)?;
        (*function.cast::<Threadsafe>()).referenced.store(false, Ordering::Release);
        Ok(())
    });
    api!(napi_ref_threadsafe_function(env, function: Ptr) {
        check(!function.is_null(), 1)?;
        (*function.cast::<Threadsafe>()).referenced.store(true, Ordering::Release);
        Ok(())
    });
}
