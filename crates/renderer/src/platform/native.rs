use crate::{Event, Rect, WindowId};
use std::cell::RefCell;

#[derive(Clone, Copy)]
pub enum WindowEvent {
  Changed,
  CloseRequested,
}

// A handler, missing while it runs.
type Slot<H> = Option<Box<H>>;
type KeyFilter = dyn FnMut(WindowId, &Event, &dyn Fn() -> bool) -> bool;
type FrameFilter = dyn FnMut(WindowId, &str, Rect) -> bool;

#[derive(Default)]
struct Handlers {
  window: Slot<dyn FnMut(WindowId, WindowEvent)>,
  key: Slot<KeyFilter>,
  will_change: Slot<FrameFilter>,
  reopen: Slot<dyn FnMut(bool)>,
  should_quit: Slot<dyn FnMut() -> bool>,
  open_urls: Slot<dyn FnMut(Vec<String>)>,
  pending_urls: Vec<String>,
  quit: Slot<dyn FnOnce()>,
}

thread_local! { static HANDLERS: RefCell<Handlers> = RefCell::default(); }

// Calls a handler outside the borrow, so it can set handlers. A call from inside the same handler finds
// none.
fn call<H: ?Sized, R>(
  slot: fn(&mut Handlers) -> &mut Slot<H>,
  run: impl FnOnce(&mut H) -> R,
) -> Option<R> {
  let mut handler = HANDLERS.with_borrow_mut(|handlers| slot(handlers).take())?;
  let result = run(&mut handler);
  HANDLERS.with_borrow_mut(|handlers| {
    slot(handlers).get_or_insert(handler);
  });
  Some(result)
}

pub fn on_window_event(f: impl FnMut(WindowId, WindowEvent) + 'static) {
  HANDLERS.with_borrow_mut(|handlers| handlers.window = Some(Box::new(f)));
}

pub(crate) fn window_event(id: WindowId, event: WindowEvent) {
  call(
    |handlers| &mut handlers.window,
    |handler| handler(id, event),
  );
}

// Return true to keep the key from the page. Call `composing` only when needed: it blocks on the page thread.
pub fn on_key(f: impl FnMut(WindowId, &Event, &dyn Fn() -> bool) -> bool + 'static) {
  HANDLERS.with_borrow_mut(|handlers| handlers.key = Some(Box::new(f)));
}

// Keys from nested loops bypass a filter already in progress.
pub(crate) fn key(id: WindowId, event: &Event, composing: &dyn Fn() -> bool) -> bool {
  call(
    |handlers| &mut handlers.key,
    |filter| filter(id, event, composing),
  )
  .unwrap_or(false)
}

// `kind` is `move`, or the edge of a resize: `right` or `bottom`. Return true to cancel a resize.
pub fn on_will_change(f: impl FnMut(WindowId, &str, Rect) -> bool + 'static) {
  HANDLERS.with_borrow_mut(|handlers| handlers.will_change = Some(Box::new(f)));
}

#[cfg(target_os = "macos")]
pub(crate) fn will_change(id: WindowId, kind: &str, bounds: Rect) -> bool {
  call(
    |handlers| &mut handlers.will_change,
    |filter| filter(id, kind, bounds),
  )
  .unwrap_or(false)
}

pub fn on_reopen(f: impl FnMut(bool) + 'static) {
  HANDLERS.with_borrow_mut(|handlers| handlers.reopen = Some(Box::new(f)));
}

pub fn on_should_quit(f: impl FnMut() -> bool + 'static) {
  HANDLERS.with_borrow_mut(|handlers| handlers.should_quit = Some(Box::new(f)));
}

pub fn on_open_urls(mut f: impl FnMut(Vec<String>) + 'static) {
  let urls = HANDLERS.with_borrow_mut(|handlers| std::mem::take(&mut handlers.pending_urls));
  if !urls.is_empty() {
    f(urls);
  }
  HANDLERS.with_borrow_mut(|handlers| handlers.open_urls = Some(Box::new(f)));
}

pub fn on_quit(f: impl FnOnce() + 'static) {
  HANDLERS.with_borrow_mut(|handlers| handlers.quit = Some(Box::new(f)));
}

#[cfg(target_os = "macos")]
pub(crate) fn reopen(visible: bool) {
  call(|handlers| &mut handlers.reopen, |handler| handler(visible));
}

pub(crate) fn should_quit() -> bool {
  call(|handlers| &mut handlers.should_quit, |handler| handler()).unwrap_or(true)
}

pub(crate) fn quit_handler() {
  if let Some(handler) = HANDLERS.with_borrow_mut(|handlers| handlers.quit.take()) {
    handler();
  }
}

#[cfg(target_os = "macos")]
pub(crate) fn open_urls(urls: Vec<String>) {
  let mut urls = Some(urls);
  call(
    |handlers| &mut handlers.open_urls,
    |handler| handler(urls.take().unwrap_or_default()),
  );
  if let Some(urls) = urls {
    HANDLERS.with_borrow_mut(|handlers| handlers.pending_urls.extend(urls));
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::{Key, Modifiers};

  #[test]
  fn the_key_filter_blocks_keys_but_not_the_ones_of_a_loop_nested_in_it() {
    let event = Event::KeyUp {
      key: Key::default(),
      modifiers: Modifiers::NONE,
      ime: false,
    };
    assert!(!key(WindowId(1), &event, &|| true));
    on_key(|id, event, composing| {
      assert!(!key(id, event, composing));
      id == WindowId(1) && composing()
    });
    assert!(key(WindowId(1), &event, &|| true));
    assert!(!key(WindowId(1), &event, &|| false));
    assert!(!key(WindowId(2), &event, &|| true));
  }

  #[cfg(target_os = "macos")]
  #[test]
  fn urls_wait_for_their_handler() {
    open_urls(vec!["apps://a".into()]);
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = seen.clone();
    on_open_urls(move |urls| log.borrow_mut().extend(urls));
    open_urls(vec!["apps://b".into()]);
    assert_eq!(*seen.borrow(), ["apps://a", "apps://b"]);
  }
}
