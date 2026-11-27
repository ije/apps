use super::{State, Status, check};
use crate::jsc::Ptr;
use std::{
  cell::RefCell,
  rc::{Rc, Weak},
};

type Cleanup = unsafe extern "C" fn(Ptr);
type AsyncCleanupHook = unsafe extern "C" fn(Ptr, Ptr);

// The hooks that run when the environment closes.
#[derive(Default)]
pub(super) struct Hooks {
  cleanup: RefCell<Vec<(Cleanup, Ptr)>>,
  // Addresses of `AsyncCleanup` handles.
  async_cleanup: Rc<RefCell<Vec<usize>>>,
}

impl Hooks {
  // Runs the async cleanup hooks, then the cleanup hooks, last added first.
  pub(super) fn run(&self) {
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
  }

  fn add(&self, callback: Cleanup, data: Ptr) -> Status {
    let mut hooks = self.cleanup.borrow_mut();
    check(
      !hooks
        .iter()
        .any(|(cb, arg)| *cb as usize == callback as usize && *arg == data),
      1,
    )?;
    hooks.push((callback, data));
    Ok(())
  }

  fn remove(&self, callback: Cleanup, data: Ptr) -> Status {
    let mut hooks = self.cleanup.borrow_mut();
    let index = hooks
      .iter()
      .rposition(|(cb, arg)| *cb as usize == callback as usize && *arg == data)
      .ok_or(1)?;
    hooks.remove(index);
    Ok(())
  }

  fn add_async(
    &self,
    hook: AsyncCleanupHook,
    arg: Ptr,
    thread: std::thread::ThreadId,
  ) -> *mut AsyncCleanup {
    let cleanup = Box::into_raw(Box::new(AsyncCleanup {
      hooks: Rc::downgrade(&self.async_cleanup),
      thread,
      hook,
      arg,
    }));
    self.async_cleanup.borrow_mut().push(cleanup as usize);
    cleanup
  }
}

pub(super) struct AsyncCleanup {
  hooks: Weak<RefCell<Vec<usize>>>,
  thread: std::thread::ThreadId,
  hook: AsyncCleanupHook,
  arg: Ptr,
}

impl AsyncCleanup {
  raw_api!(napi_remove_async_cleanup_hook(handle: Ptr) {
      if handle.is_null() || unsafe { (*handle.cast::<AsyncCleanup>()).thread } != std::thread::current().id() {
          return 1;
      }
      let cleanup = unsafe { Box::from_raw(handle.cast::<AsyncCleanup>()) };
      if let Some(hooks) = cleanup.hooks.upgrade() {
          hooks.borrow_mut().retain(|other| *other != handle as usize);
      }
      0
  });
}

impl State {
  api!(napi_add_env_cleanup_hook(env, callback: Option<Cleanup>, data: Ptr) {
      env.hooks.add(callback.ok_or(1)?, data)
  });
  api!(napi_remove_env_cleanup_hook(env, callback: Option<Cleanup>, data: Ptr) {
      env.hooks.remove(callback.ok_or(1)?, data)
  });
  api!(napi_add_async_cleanup_hook(env, hook: Option<AsyncCleanupHook>, arg: Ptr, handle: *mut Ptr) {
      let cleanup = env.hooks.add_async(hook.ok_or(1)?, arg, env.thread);
      if !handle.is_null() {
          *handle = cleanup.cast();
      }
      Ok(())
  });
}
