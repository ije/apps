use super::{State, Status, check};
use crate::jsc::{JavaScriptCore, Ptr};
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    ptr,
};

#[derive(Default)]
struct Scope {
    kept: HashSet<usize>,
    values: Vec<Ptr>,
    escaped: bool,
    escapable: bool,
}

// The values that addon code holds: handle scopes, and values that live until the environment closes.
#[derive(Default)]
pub(super) struct Handles {
    // Scope addresses cross the C ABI and must remain stable.
    #[allow(clippy::vec_box)]
    scopes: RefCell<Vec<Box<Scope>>>,
    held: RefCell<Vec<Ptr>>,
    callbacks: Cell<usize>,
}

impl Handles {
    pub(super) fn hold(&self, api: &JavaScriptCore, ctx: Ptr, value: Ptr) {
        unsafe {
            (api.protect)(ctx, value);
        }
        self.held.borrow_mut().push(value);
    }

    pub(super) fn keep(&self, api: &JavaScriptCore, ctx: Ptr, value: Ptr) -> Ptr {
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
                (api.protect)(ctx, value);
            }
        }
        value
    }

    // Opens a scope. Returns the depth before it, for `unwind`.
    pub(super) fn push(&self, escapable: bool) -> usize {
        let mut scopes = self.scopes.borrow_mut();
        scopes.push(Box::new(Scope {
            escapable,
            ..Scope::default()
        }));
        scopes.len() - 1
    }

    fn top(&self) -> Ptr {
        self.scopes
            .borrow_mut()
            .last_mut()
            .map_or(ptr::null_mut(), |scope| (&mut **scope as *mut Scope).cast())
    }

    fn pop(&self, api: &JavaScriptCore, ctx: Ptr) {
        if let Some(scope) = self.scopes.borrow_mut().pop() {
            for value in scope.values {
                unsafe {
                    (api.unprotect)(ctx, value);
                }
            }
        }
    }

    pub(super) fn unwind(&self, api: &JavaScriptCore, ctx: Ptr, depth: usize) {
        while self.scopes.borrow().len() > depth {
            self.pop(api, ctx);
        }
    }

    fn close(&self, api: &JavaScriptCore, ctx: Ptr, scope: Ptr) -> Status {
        check(!scope.is_null(), 1)?;
        check(
            self.scopes
                .borrow()
                .last()
                .is_some_and(|last| ptr::eq(&**last, scope.cast::<Scope>())),
            13,
        )?;
        self.pop(api, ctx);
        Ok(())
    }

    fn escape(&self, api: &JavaScriptCore, ctx: Ptr, scope: Ptr, value: Ptr) -> Status {
        let mut scopes = self.scopes.borrow_mut();
        let current = scopes.last_mut().ok_or(13)?;
        check(
            ptr::eq(&**current, scope.cast::<Scope>()) && current.escapable,
            13,
        )?;
        check(!current.escaped, 12)?;
        current.escaped = true;
        unsafe {
            (api.protect)(ctx, value);
        }
        let length = scopes.len();
        if length > 1 {
            scopes[length - 2].values.push(value);
        } else {
            self.held.borrow_mut().push(value);
        }
        Ok(())
    }

    fn enter_callback(&self) {
        self.callbacks.set(self.callbacks.get() + 1);
    }

    fn exit_callback(&self) -> Status {
        check(self.callbacks.get() > 0, 14)?;
        self.callbacks.set(self.callbacks.get() - 1);
        Ok(())
    }

    // Releases all scopes and held values when the environment closes.
    pub(super) fn release(&self, api: &JavaScriptCore, ctx: Ptr) {
        self.unwind(api, ctx, 0);
        for value in self.held.borrow_mut().drain(..) {
            unsafe {
                (api.unprotect)(ctx, value);
            }
        }
    }
}

impl State {
    api!(napi_open_handle_scope(env, result: *mut Ptr) {
        check(!result.is_null(), 1)?;
        env.handles.push(false);
        *result = env.handles.top();
        Ok(())
    });
    api!(napi_close_handle_scope(env, scope: Ptr) {
        env.handles.close(&env.api, env.ctx, scope)
    });
    api!(napi_open_escapable_handle_scope(env, result: *mut Ptr) {
        check(!result.is_null(), 1)?;
        env.handles.push(true);
        *result = env.handles.top();
        Ok(())
    });
    api!(napi_close_escapable_handle_scope(env, scope: Ptr) {
        env.handles.close(&env.api, env.ctx, scope)
    });
    api!(napi_escape_handle(env, scope: Ptr, value: Ptr, result: *mut Ptr) {
        check(!scope.is_null() && !value.is_null() && !result.is_null(), 1)?;
        env.handles.escape(&env.api, env.ctx, scope, value)?;
        *result = value;
        Ok(())
    });
    api!(napi_open_callback_scope(env, _resource: Ptr, _context: Ptr, result: *mut Ptr) {
        check(!result.is_null(), 1)?;
        env.handles.enter_callback();
        *result = ptr::NonNull::<u8>::dangling().as_ptr().cast();
        Ok(())
    });
    api!(napi_close_callback_scope(env, scope: Ptr) {
        check(!scope.is_null(), 1)?;
        env.handles.exit_callback()
    });
}
