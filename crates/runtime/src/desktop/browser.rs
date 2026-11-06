use anyhow::{Context, Result, bail, ensure};
pub use apps_webview::Browser;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Default)]
pub struct State {
    pub(crate) application: String,
    views: HashMap<u64, Browser>,
    events: Vec<Value>,
    next_id: u64,
    attaches: Vec<(u64, u64)>,
    parents: HashMap<u64, u64>,
}

impl State {
    pub fn call(&mut self, operation: &str, arguments: &Value) -> Result<Value> {
        match operation {
            "create" => {
                let mut options = arguments[0]
                    .as_object()
                    .cloned()
                    .context("Expected browser options")?;
                for name in [
                    "persistent",
                    "inspectable",
                    "javascript",
                    "show",
                    "navigation",
                ] {
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
                options.insert("application".into(), self.application.clone().into());
                let browser = Browser::detached(&Value::Object(options))?;
                self.next_id += 1;
                self.views.insert(self.next_id, browser);
                Ok(self.next_id.into())
            }
            "reserve" => {
                self.next_id += 1;
                Ok(self.next_id.into())
            }
            "poll" => {
                let mut events = std::mem::take(&mut self.events);
                events.extend(self.views.iter().flat_map(|(&id, browser)| {
                    browser
                        .poll()
                        .into_iter()
                        .map(move |event| serde_json::json!({"view":id,"event":event}))
                }));
                Ok(Value::Array(events))
            }
            "destroy" => {
                let id = arguments[0].as_u64().context("Expected browser id")?;
                self.views.remove(&id);
                self.parents.remove(&id);
                self.attaches.retain(|(view, _)| *view != id);
                Ok(Value::Null)
            }
            _ => {
                let id = arguments[0].as_u64().context("Expected browser id")?;
                let browser = self.views.get(&id).context("Browser is destroyed")?;
                match operation {
                    "attach" => {
                        let parent = arguments[1].as_u64().context("Expected parent window id")?;
                        self.attaches.retain(|(view, _)| *view != id);
                        self.attaches.push((id, parent));
                        self.parents.insert(id, parent);
                        Ok(Value::Null)
                    }
                    "detach" => {
                        browser.detach()?;
                        self.parents.remove(&id);
                        self.attaches.retain(|(view, _)| *view != id);
                        Ok(Value::Null)
                    }
                    "bounds" => {
                        browser.set_bounds(serde_json::from_value(arguments[1].clone())?)?;
                        Ok(Value::Null)
                    }
                    "command" => browser.command(
                        arguments[1].as_str().context("Expected browser command")?,
                        &arguments[2],
                    ),
                    _ => bail!("Unknown browser operation: {operation}"),
                }
            }
        }
    }

    pub fn take_attaches(&mut self) -> Vec<(u64, u64)> {
        std::mem::take(&mut self.attaches)
    }
    pub fn defer_attach(&mut self, id: u64, window: u64) {
        self.attaches.push((id, window));
    }
    pub fn attach(&self, id: u64, window: &impl raw_window_handle::HasWindowHandle) -> Result<()> {
        self.views
            .get(&id)
            .context("Browser is destroyed")?
            .attach(window)
    }
    pub fn close_window(&mut self, window: u64) -> Result<()> {
        for (&id, &parent) in &self.parents {
            if parent == window
                && let Some(browser) = self.views.get(&id)
            {
                browser.detach()?;
            }
        }
        self.parents.retain(|_, parent| *parent != window);
        self.attaches.retain(|(_, parent)| *parent != window);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_parent_detaches_and_destroy_is_idempotent() {
        let mut state = State::default();
        assert_eq!(state.call("reserve", &Value::Null).unwrap(), 1);
        assert_eq!(state.call("reserve", &Value::Null).unwrap(), 2);
        state.parents.insert(2, 1);
        state.attaches.push((2, 1));
        state.close_window(1).unwrap();
        assert!(state.parents.is_empty());
        assert!(state.attaches.is_empty());
        let events = state.call("poll", &Value::Null).unwrap();
        assert_eq!(events, serde_json::json!([]));
        assert_eq!(
            state.call("destroy", &serde_json::json!([2])).unwrap(),
            Value::Null
        );
        assert_eq!(
            state.call("poll", &Value::Null).unwrap(),
            serde_json::json!([])
        );
    }
}
