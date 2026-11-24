use super::{Env, State, Status, check};
use crate::jsc::{JavaScriptCore, Ptr};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    ptr,
    sync::{Arc, Mutex},
};

pub(super) type Finalize = unsafe extern "C" fn(Env, Ptr, Ptr);

#[derive(Clone, Copy)]
pub(super) struct Finalizer {
    pub(super) callback: Finalize,
    pub(super) data: Ptr,
    pub(super) hint: Ptr,
}

impl Finalizer {
    pub(super) unsafe fn run(self, env: Env) {
        unsafe { (self.callback)(env, self.data, self.hint) };
    }
}

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Finalizer,
    Wrap(Ptr),
    External,
    Function,
}

struct Tracked {
    object: usize,
    kind: Kind,
    finalizer: Option<Finalizer>,
}

pub(super) struct Holder {
    queue: Arc<Mutex<Vec<u64>>>,
    id: u64,
    byte: u8,
}

pub(super) fn holder_byte(holder: *mut Holder) -> Ptr {
    unsafe { (&raw mut (*holder).byte).cast() }
}

pub(super) unsafe extern "C" fn release_holder(_: Ptr, holder: Ptr) {
    // JSC frees array buffers at the end of a GC, maybe on a GC thread. Only queue the ID here.
    let holder = unsafe { Box::from_raw(holder.cast::<Holder>()) };
    if let Ok(mut queue) = holder.queue.lock() {
        queue.push(holder.id);
    }
}

struct Reference {
    value: Ptr,
    weak: Ptr,
    count: u32,
}

// The JavaScript objects that addon data follows: wraps, externals, finalizers, functions and references.
#[derive(Default)]
pub(super) struct Objects {
    next: Cell<u64>,
    holders: RefCell<HashMap<u64, Tracked>>,
    // IDs of holders that GC freed.
    finalized: Arc<Mutex<Vec<u64>>>,
    // GC can reuse an object address before its holder is finalized, so address maps keep the holder ID.
    externals: RefCell<HashMap<usize, (u64, Ptr)>>,
    references: RefCell<HashMap<usize, Box<Reference>>>,
}

impl Objects {
    // A new holder ID, and the holder that queues the ID when GC frees it.
    pub(super) fn holder(&self) -> (u64, *mut Holder) {
        let id = self.next.get() + 1;
        self.next.set(id);
        let holder = Holder {
            queue: self.finalized.clone(),
            id,
            byte: 0,
        };
        (id, Box::into_raw(Box::new(holder)))
    }

    pub(super) fn insert(&self, id: u64, object: Ptr, kind: Kind, finalizer: Option<Finalizer>) {
        self.holders.borrow_mut().insert(
            id,
            Tracked {
                object: object as usize,
                kind,
                finalizer,
            },
        );
    }

    pub(super) fn external(&self, object: Ptr) -> Option<(u64, Ptr)> {
        self.externals.borrow().get(&(object as usize)).copied()
    }

    // The holders that GC freed: their ID, object address, kind and finalizer.
    pub(super) fn finalized(&self) -> Vec<(u64, usize, Kind, Option<Finalizer>)> {
        let ids = self
            .finalized
            .lock()
            .map(|mut queue| std::mem::take(&mut *queue))
            .unwrap_or_default();
        let mut ended = Vec::with_capacity(ids.len());
        for id in ids {
            let Some(Tracked {
                object,
                kind,
                finalizer,
            }) = self.holders.borrow_mut().remove(&id)
            else {
                continue;
            };
            if matches!(kind, Kind::External) {
                let mut externals = self.externals.borrow_mut();
                if externals
                    .get(&object)
                    .is_some_and(|external| external.0 == id)
                {
                    externals.remove(&object);
                }
            }
            ended.push((id, object, kind, finalizer));
        }
        ended
    }

    // The IDs of holders with a finalizer, in the order to run them at close: wraps first.
    pub(super) fn closing(&self) -> Vec<u64> {
        let mut ids: Vec<(bool, u64)> = self
            .holders
            .borrow()
            .iter()
            .filter(|(_, tracked)| tracked.finalizer.is_some())
            .map(|(id, tracked)| (!matches!(tracked.kind, Kind::Wrap(_)), *id))
            .collect();
        ids.sort_unstable();
        ids.into_iter().map(|(_, id)| id).collect()
    }

    pub(super) fn take_finalizer(&self, id: u64) -> Option<Finalizer> {
        self.holders
            .borrow_mut()
            .remove(&id)
            .and_then(|tracked| tracked.finalizer)
    }

    fn reference(&self, api: &JavaScriptCore, ctx: Ptr, value: Ptr, weak: Ptr, count: u32) -> Ptr {
        unsafe {
            (api.protect)(ctx, weak);
            if count > 0 {
                (api.protect)(ctx, value);
            }
        }
        let mut reference = Box::new(Reference { value, weak, count });
        let pointer = (&mut *reference as *mut Reference).cast();
        self.references
            .borrow_mut()
            .insert(pointer as usize, reference);
        pointer
    }

    fn delete_reference(&self, api: &JavaScriptCore, ctx: Ptr, reference: Ptr) -> Status {
        let reference = self
            .references
            .borrow_mut()
            .remove(&(reference as usize))
            .ok_or(1)?;
        unsafe {
            (api.unprotect)(ctx, reference.weak);
            if reference.count > 0 {
                (api.unprotect)(ctx, reference.value);
            }
        }
        Ok(())
    }

    // The strong value, the weak holder and the count of a reference.
    fn referenced(&self, reference: Ptr) -> std::result::Result<(Ptr, Ptr, u32), i32> {
        let references = self.references.borrow();
        let reference = references.get(&(reference as usize)).ok_or(1)?;
        Ok((reference.value, reference.weak, reference.count))
    }

    // Adds one to the count, or takes one away. `value` is the current value, kept strong from a count of one.
    fn count(
        &self,
        api: &JavaScriptCore,
        ctx: Ptr,
        reference: Ptr,
        value: Option<Ptr>,
    ) -> std::result::Result<u32, i32> {
        let mut references = self.references.borrow_mut();
        let reference = references.get_mut(&(reference as usize)).ok_or(1)?;
        match value {
            Some(value) => {
                check(reference.count < u32::MAX, 9)?;
                if reference.count == 0 {
                    unsafe { (api.protect)(ctx, value) };
                    reference.value = value;
                }
                reference.count += 1;
            }
            None => {
                check(reference.count > 0, 9)?;
                reference.count -= 1;
                if reference.count == 0 {
                    unsafe { (api.unprotect)(ctx, reference.value) };
                }
            }
        }
        Ok(reference.count)
    }

    pub(super) fn drop_references(&self, api: &JavaScriptCore, ctx: Ptr) {
        for (_, reference) in self.references.borrow_mut().drain() {
            unsafe {
                (api.unprotect)(ctx, reference.weak);
                if reference.count > 0 {
                    (api.unprotect)(ctx, reference.value);
                }
            }
        }
    }

    fn wrap(&self, id: u64, data: Ptr, finalizer: Option<Finalizer>) {
        if let Some(tracked) = self.holders.borrow_mut().get_mut(&id) {
            tracked.kind = Kind::Wrap(data);
            tracked.finalizer = finalizer;
        }
    }

    // The data of a wrap, which ends the wrap with `remove`.
    fn wrapped(&self, id: u64, remove: bool) -> Option<Ptr> {
        let mut holders = self.holders.borrow_mut();
        let Some(Tracked {
            kind: Kind::Wrap(data),
            ..
        }) = holders.get(&id)
        else {
            return None;
        };
        let data = *data;
        if remove {
            holders.remove(&id);
        }
        Some(data)
    }

    pub(super) fn add_external(&self, object: Ptr, id: u64, data: Ptr) {
        self.externals
            .borrow_mut()
            .insert(object as usize, (id, data));
    }
}

impl State {
    api!(napi_create_reference(env, value: Ptr, count: u32, result: *mut Ptr) {
        check(!result.is_null(), 1)?;
        env.object(value)?;
        let weak = env.js("weak", &[value])?;
        *result = env.objects.reference(&env.api, env.ctx, value, weak, count);
        Ok(())
    });
    api!(napi_delete_reference(env, reference: Ptr) {
        env.objects.delete_reference(&env.api, env.ctx, reference)
    });
    api!(napi_get_reference_value(env, reference: Ptr, result: *mut Ptr) {
        check(!result.is_null(), 1)?;
        let (value, weak, count) = env.objects.referenced(reference)?;
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
            Self::napi_get_reference_value(env, reference, &mut value) == 0 && !value.is_null(),
            9,
        )?;
        let count = env.objects.count(&env.api, env.ctx, reference, Some(value))?;
        if !result.is_null() {
            *result = count;
        }
        Ok(())
    });
    api!(napi_reference_unref(env, reference: Ptr, result: *mut u32) {
        let count = env.objects.count(&env.api, env.ctx, reference, None)?;
        if !result.is_null() {
            *result = count;
        }
        Ok(())
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
        check(!env.truthy("wrap", &[object])?, 1)?;
        check(result.is_null() || callback.is_some(), 1)?;
        let id = env.track(object, Kind::Finalizer, None)?;
        env.js("wrap", &[object, env.number(id as f64)])?;
        env.objects.wrap(id, data, callback.map(|callback| Finalizer { callback, data, hint }));
        if !result.is_null() {
            check(Self::napi_create_reference(env, object, 0, result) == 0, 9)?;
        }
        Ok(())
    });
    api!(napi_unwrap(env, object: Ptr, result: *mut Ptr) {
        check(!result.is_null(), 1)?;
        env.object(object)?;
        let id = env.js("wrap", &[object])?;
        let id = (env.api.to_number)(env.ctx, id, ptr::null_mut()) as u64;
        *result = env.objects.wrapped(id, false).ok_or(1)?;
        Ok(())
    });
    api!(napi_remove_wrap(env, object: Ptr, result: *mut Ptr) {
        check(!result.is_null(), 1)?;
        env.object(object)?;
        let id = env.js("wrap", &[object, env.number(0.0)])?;
        let id = (env.api.to_number)(env.ctx, id, ptr::null_mut()) as u64;
        *result = env.objects.wrapped(id, true).ok_or(1)?;
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
        env.track(object, Kind::Finalizer, Some(Finalizer { callback, data, hint }))?;
        if !result.is_null() {
            check(Self::napi_create_reference(env, object, 0, result) == 0, 9)?;
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
        let id = env.track(value, Kind::External, callback.map(|callback| Finalizer { callback, data, hint }))?;
        env.js("external", &[value, env.number(id as f64)])?;
        env.objects.add_external(value, id, data);
        env.value(result, value)
    });
    api!(napi_get_value_external(env, value: Ptr, result: *mut Ptr) {
        check(!result.is_null(), 1)?;
        *result = env.external(value)?.ok_or(1)?;
        Ok(())
    });
}
