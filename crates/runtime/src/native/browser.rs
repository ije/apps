use crate::webview::Browser;
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::collections::HashMap;

pub enum Parent<W> {
  // The window and its scale factor.
  Ready(W, f64),
  Waiting,
  Gone,
}

enum Attach {
  Pending,
  Attached,
}

#[derive(Default)]
pub struct State {
  views: HashMap<u64, Browser>,
  next_id: u64,
  // The views that wait for their window, in the order of their attach calls, which stacks the native views.
  // A view that is no longer pending stays here until the next `attach_pending`.
  pending: Vec<u64>,
  // The window of each view and whether the view is in it yet.
  parents: HashMap<u64, (u64, Attach)>,
}

impl State {
  pub fn call(&mut self, operation: &str, arguments: &Value, application: &str) -> Result<Value> {
    match operation {
      "create" => {
        let mut options = arguments[0]
          .as_object()
          .cloned()
          .context("Expected browser options")?;
        for name in ["inspectable", "javascript", "show", "navigation"] {
          ensure!(
            options.get(name).is_none_or(Value::is_boolean),
            "Expected boolean browser option: {name}"
          );
        }
        ensure!(
          options.get("partition").is_none_or(Value::is_string),
          "Expected browser partition name"
        );
        ensure!(
          options.get("title").is_none_or(Value::is_string),
          "Expected browser title"
        );
        options.entry("show").or_insert(Value::Bool(false));
        options.insert("application".into(), application.into());
        let browser = Browser::detached(&Value::Object(options))?;
        self.next_id += 1;
        self.views.insert(self.next_id, browser);
        Ok(self.next_id.into())
      }
      "reserve" => {
        self.next_id += 1;
        Ok(self.next_id.into())
      }
      "poll" => Ok(Value::Array(
        self
          .views
          .iter()
          .flat_map(|(&id, browser)| {
            browser
              .poll()
              .into_iter()
              .map(move |event| serde_json::json!({"view":id,"event":event}))
          })
          .collect(),
      )),
      "destroy" => {
        let id = arguments[0].as_u64().context("Expected browser id")?;
        self.views.remove(&id);
        self.parents.remove(&id);
        Ok(Value::Null)
      }
      _ => {
        let id = arguments[0].as_u64().context("Expected browser id")?;
        let browser = self.views.get(&id).context("Browser is destroyed")?;
        match operation {
          "attach" => {
            let parent = arguments[1].as_u64().context("Expected parent window id")?;
            self.pending.retain(|view| *view != id);
            self.pending.push(id);
            self.parents.insert(id, (parent, Attach::Pending));
            Ok(Value::Null)
          }
          "detach" => {
            browser.detach()?;
            self.parents.remove(&id);
            Ok(Value::Null)
          }
          "bounds" => {
            browser.set_bounds(serde_json::from_value(arguments[1].clone())?)?;
            Ok(Value::Null)
          }
          "command" => {
            browser.command(
              arguments[1].as_str().context("Expected browser command")?,
              &arguments[2],
            )?;
            Ok(Value::Null)
          }
          _ => bail!("Unknown browser operation: {operation}"),
        }
      }
    }
  }

  // Attaches the views that wait for a window, or keeps them waiting, or drops them when the window is gone.
  pub fn attach_pending<W: raw_window_handle::HasWindowHandle>(
    &mut self,
    parent: impl Fn(u64) -> Parent<W>,
  ) {
    for id in std::mem::take(&mut self.pending) {
      let Some(&(window, Attach::Pending)) = self.parents.get(&id) else {
        continue;
      };
      let result = match parent(window) {
        Parent::Ready(handle, scale) => {
          self.parents.insert(id, (window, Attach::Attached));
          self
            .views
            .get(&id)
            .context("Browser is destroyed")
            .and_then(|browser| browser.attach(&handle, scale))
        }
        Parent::Waiting => {
          self.pending.push(id);
          Ok(())
        }
        Parent::Gone => self.close_window(window),
      };
      if let Err(error) = result {
        eprintln!("apps: {error:#}");
      }
    }
  }
  pub fn close_window(&mut self, window: u64) -> Result<()> {
    for (&id, &(parent, _)) in &self.parents {
      if parent == window
        && let Some(browser) = self.views.get(&id)
      {
        browser.detach()?;
      }
    }
    self.parents.retain(|_, (parent, _)| *parent != window);
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn closing_parent_detaches_and_destroy_is_idempotent() {
    let mut state = State::default();
    assert_eq!(state.call("reserve", &Value::Null, "").unwrap(), 1);
    assert_eq!(state.call("reserve", &Value::Null, "").unwrap(), 2);
    state.parents.insert(2, (1, Attach::Pending));
    state.pending.push(2);
    state.parents.insert(3, (4, Attach::Attached));
    state.close_window(1).unwrap();
    assert!(state.parents.keys().eq([&3]));
    state.attach_pending(|_| -> Parent<raw_window_handle::WindowHandle<'static>> {
      panic!("Detached views don't wait")
    });
    assert!(state.pending.is_empty());
    let events = state.call("poll", &Value::Null, "").unwrap();
    assert_eq!(events, serde_json::json!([]));
    assert_eq!(
      state.call("destroy", &serde_json::json!([2]), "").unwrap(),
      Value::Null
    );
    assert_eq!(
      state.call("poll", &Value::Null, "").unwrap(),
      serde_json::json!([])
    );
  }
}
