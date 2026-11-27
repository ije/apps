use super::link::Target;
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

// Checks a BrowserWindow command, and chooses the thread that runs it: main for native window changes, else the
// page thread of the window.
pub(crate) fn route(args: &Value) -> Result<Target> {
  validate(args)?;
  let id = args[1].as_u64().context("Expected a window id")?;
  Ok(if native_action(args[0].as_str().unwrap_or("")) {
    Target::Main
  } else {
    Target::Window(id)
  })
}

fn validate(args: &Value) -> Result<()> {
  let number = |value: &Value, low: f64, high: f64| {
    value
      .as_f64()
      .is_some_and(|value| (low..=high).contains(&value))
  };
  let position = |value: &Value| number(value, -1e6, 1e6);
  let size = |value: &Value| number(value, 1., 16384.);
  match args[0].as_str().unwrap_or("") {
    "create" => {
      let options = &args[2];
      for (key, value) in options.as_object().into_iter().flatten() {
        let valid = match key.as_str() {
          "width" | "height" => size(value),
          "minWidth" | "minHeight" | "maxWidth" | "maxHeight" => number(value, 0., 16384.),
          "x" | "y" => position(value),
          "useContentSize" | "center" | "resizable" | "movable" | "minimizable" | "maximizable"
          | "closable" | "alwaysOnTop" | "fullscreen" | "show" | "frame" | "transparent"
          | "hasShadow" | "focusable" | "modal" | "skipTaskbar" => value.is_boolean(),
          "title" | "icon" => value.is_string(),
          "opacity" => number(value, 0., 1.),
          "parent" => value.is_u64(),
          "vibrancy" => number(value, 0., 22.),
          "visualEffectState" => number(value, 0., 2.),
          "titleBarStyle" => {
            matches!(value.as_str(), Some("default" | "hidden" | "hiddenInset"))
          }
          "backgroundColor" => value
            .as_str()
            .and_then(crate::css::value::color::color)
            .is_some(),
          "trafficLightPosition" => position(&value["x"]) && position(&value["y"]),
          _ => bail!("Unsupported BrowserWindow option: {key}"),
        };
        ensure!(valid, "Invalid BrowserWindow option {key}: {value}");
      }
    }
    "bounds" => ensure!(
      position(&args[2]) && position(&args[3]) && size(&args[4]) && size(&args[5]),
      "Invalid window bounds"
    ),
    "limits" => ensure!(
      (2..6).all(|index| number(&args[index], 0., 16384.)),
      "Window size limits must be between 0 and 16384"
    ),
    "fullscreen" | "resizable" | "minimizable" | "maximizable" | "closable" | "movable"
    | "shadow" | "contentProtection" | "focusable" | "edited" => {
      ensure!(args[2].is_boolean(), "Expected a boolean")
    }
    "alwaysOnTop" => ensure!(
      args[2]
        .as_i64()
        .is_some_and(|level| i32::try_from(level).is_ok()),
      "Expected a window level"
    ),
    "ignoreMouse" | "workspaces" => ensure!(
      args[2].is_boolean() && args[3].is_boolean(),
      "Expected booleans"
    ),
    "opacity" => ensure!(number(&args[2], 0., 1.), "Opacity must be between 0 and 1"),
    "aspectRatio" => ensure!(
      number(&args[2], 0., 1e6) && number(&args[3], 0., 16384.) && number(&args[4], 0., 16384.),
      "Invalid aspect ratio"
    ),
    "parent" => ensure!(
      args[2].is_null() || args[2].as_u64().is_some_and(|parent| parent != args[1]),
      "Invalid parent window"
    ),
    "vibrancy" => ensure!(
      (args[2].is_null() || number(&args[2], 0., 22.)) && number(&args[3], 0., 2.),
      "Invalid vibrancy"
    ),
    "background" => ensure!(
      args[2]
        .as_str()
        .and_then(crate::css::value::color::color)
        .is_some(),
      "Invalid background color"
    ),
    "load" | "loadFile" => ensure!(
      args[2].is_string() && (args[3].is_null() || args[3].is_u64()),
      "Invalid page load"
    ),
    "execute" => ensure!(
      args[2].is_u64() && args[3].is_string(),
      "Invalid script request"
    ),
    "capture" => ensure!(args[2].is_u64(), "Invalid capture request"),
    "zoom" => ensure!(
      number(&args[2], 0.25, 5.),
      "Zoom must be between 0.25 and 5"
    ),
    "insertCSS" => ensure!(
      args[2].is_string() && args[3].is_string() && args[4].is_boolean(),
      "Invalid CSS"
    ),
    "removeInsertedCSS" => ensure!(args[2].is_string(), "Invalid CSS key"),
    "edit" => ensure!(
      matches!(
        args[2].as_str(),
        Some(
          "undo"
            | "redo"
            | "cut"
            | "copy"
            | "paste"
            | "pasteAndMatchStyle"
            | "delete"
            | "selectAll"
            | "unselect"
            | "replace"
            | "insertText"
        )
      ) && (args[3].is_null() || args[3].is_string()),
      "Invalid edit command"
    ),
    "input" | "willChange" => ensure!(args[2].is_boolean(), "Expected a boolean"),
    "deliver" => ensure!(
      matches!(
        args[2].as_str(),
        Some("webviewEvent" | "webviewReply" | "filesPicked" | "selectPicked")
      ) && args[3].is_array(),
      "Invalid page delivery"
    ),
    "executed" => bail!("Unsupported window action: executed"),
    _ => {}
  }
  Ok(())
}

fn native_action(action: &str) -> bool {
  matches!(
    action,
    "close"
      | "show"
      | "title"
      | "bounds"
      | "center"
      | "minimize"
      | "maximize"
      | "unmaximize"
      | "restore"
      | "fullscreen"
      | "focus"
      | "blur"
      | "hide"
      | "showInactive"
      | "limits"
      | "resizable"
      | "minimizable"
      | "maximizable"
      | "closable"
      | "movable"
      | "alwaysOnTop"
      | "buttons"
      | "buttonPosition"
      | "opacity"
      | "shadow"
      | "ignoreMouse"
      | "workspaces"
      | "contentProtection"
      | "focusable"
      | "edited"
      | "aspectRatio"
      | "parent"
      | "input"
      | "willChange"
  )
}
