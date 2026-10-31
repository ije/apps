use super::{ViewData, WebView};
use crate::style::Prop;
use crate::{desktop::Desktop, style};
use anyhow::{Context as _, Result, ensure};
use renderer::{
    App, Bounds, FileDropEvent, Pixels, PlatformInput, Point, Utf16Selection, Window, point, px,
    size,
};
use std::ops::Range;

impl ViewData {
    pub(super) fn evaluate(&self, script: &str) -> Result<String> {
        let renderer = self
            .runtime
            .borrow()
            .views
            .get(&self.id)
            .and_then(|view| view.renderer.clone())
            .context("Page is not loaded")?;
        renderer.eval(script, "apps:input")
    }

    pub(super) fn editing(&self) -> Option<serde_json::Value> {
        serde_json::from_str(&self.evaluate("JSON.stringify(__appsEditing())").ok()?)
            .ok()
            .filter(|value: &serde_json::Value| !value.is_null())
    }

    fn input(
        &self,
        text: &str,
        range: Option<Range<usize>>,
        marked: bool,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let start = range.as_ref().map(|r| r.start);
        let end = range.as_ref().map(|r| r.end);
        let selected = selected.map(|r| [r.start, r.end]);
        let script = format!(
            "__appsInput({},{},{},{},{})",
            serde_json::json!(text),
            serde_json::json!(start),
            serde_json::json!(end),
            marked,
            serde_json::json!(selected)
        );
        if let Err(error) = super::scripted(cx, Some(window), || self.evaluate(&script)) {
            eprintln!("apps: {error}");
        }
        self.drive(cx);
        window.refresh();
    }

    pub(super) fn drive(&self, cx: &mut App) {
        let runtime = self.runtime.clone();
        cx.defer(move |cx| Desktop::drive(runtime, cx));
    }

    fn drop_files(&self, position: Point<Pixels>, window: &mut Window, cx: &mut App) {
        let mut files = Vec::new();
        for path in self.dropped.take() {
            let result = (|| -> Result<serde_json::Value> {
                let metadata = std::fs::metadata(&path)?;
                ensure!(
                    metadata.is_file() && metadata.len() <= 64 * 1024 * 1024,
                    "Dropped file exceeds 64 MB or is not a file"
                );
                let bytes = std::fs::read(&path)?;
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |time| time.as_millis() as u64);
                Ok(
                    serde_json::json!({"name":path.file_name().unwrap_or_default().to_string_lossy(),"path":path.to_string_lossy(),"bytes":bytes,"lastModified":modified}),
                )
            })();
            match result {
                Ok(file) => files.push(file),
                Err(error) => eprintln!("apps: drop: {error}"),
            }
        }
        let script = format!(
            "__appsDrop({},{},{})",
            serde_json::json!(files),
            f32::from(position.x),
            f32::from(position.y)
        );
        if let Err(error) = super::scripted(cx, Some(window), || self.evaluate(&script)) {
            eprintln!("apps: {error}");
        }
        self.drive(cx);
        window.refresh();
    }
}

impl renderer::View for WebView {
    fn render(&mut self, window: &mut Window, cx: &mut App) -> renderer::AnyElement {
        self.render_page(window, cx)
    }

    fn window_changed(&mut self, _: &mut Window, cx: &mut App) {
        if let Ok(mut host) = self.host.try_borrow_mut()
            && !host.window_changes.contains(&self.id)
        {
            host.window_changes.push(self.id);
        }
        self.drive(cx);
    }

    fn should_close(&mut self, _: &mut Window, cx: &mut App) -> bool {
        if let Ok(mut host) = self.host.try_borrow_mut() {
            host.commands
                .push(serde_json::json!({"op":"window","args":["closing",self.id]}));
        }
        self.drive(cx);
        false
    }

    fn event(&mut self, event: &PlatformInput, window: &mut Window, cx: &mut App) {
        let id = self.id;
        match event {
            PlatformInput::KeyDown(event) => {
                let key = event.keystroke.key.as_str();
                let modifiers = &event.keystroke.modifiers;
                if !self.host.borrow().packaged
                    && (key == "f12"
                        || (key == "i"
                            && ((modifiers.platform && modifiers.alt)
                                || (modifiers.control && modifiers.shift))))
                {
                    self.host
                        .borrow_mut()
                        .commands
                        .push(serde_json::json!({"op":"window","args":["devtools",id]}));
                    self.drive(cx);
                    cx.stop_propagation();
                    return;
                }
                let allowed = Desktop::dispatch(
                    self.runtime.clone(),
                    id,
                    "keydown",
                    &[],
                    serde_json::json!({"nativeKey":event.keystroke.key,"keyChar":event.keystroke.key_char,"ctrlKey":modifiers.control,"metaKey":modifiers.platform,"altKey":modifiers.alt,"shiftKey":modifiers.shift,"repeat":event.is_held}),
                    Some(window),
                    cx,
                );
                if !allowed {
                    cx.stop_propagation();
                    return;
                }
                // Before the app menu exists, cmd-q still quits.
                if cfg!(target_os = "macos")
                    && modifiers.platform
                    && !modifiers.control
                    && !modifiers.alt
                    && key == "q"
                {
                    cx.stop_propagation();
                    cx.quit();
                    return;
                }
                let primary = if cfg!(target_os = "macos") {
                    modifiers.platform && !modifiers.control
                } else {
                    modifiers.control && !modifiers.platform
                };
                if primary && !modifiers.alt && matches!(key, "c" | "x" | "v") {
                    let text = if key == "v" {
                        cx.read_from_clipboard().unwrap_or_default()
                    } else {
                        String::new()
                    };
                    let action = match key {
                        "c" => "copy",
                        "x" => "cut",
                        _ => "paste",
                    };
                    let script = format!(
                        "JSON.stringify(__appsClipboard({},{}))",
                        serde_json::json!(action),
                        serde_json::json!(text)
                    );
                    match super::scripted(cx, Some(window), || self.evaluate(&script)) {
                        Ok(value) => {
                            if key != "v"
                                && let Ok(text) = serde_json::from_str::<String>(&value)
                            {
                                cx.write_to_clipboard(text);
                            }
                        }
                        Err(error) => eprintln!("apps: {error}"),
                    }
                    cx.stop_propagation();
                    window.refresh();
                }
            }
            PlatformInput::KeyUp(event) => {
                let modifiers = &event.keystroke.modifiers;
                Desktop::dispatch(
                    self.runtime.clone(),
                    id,
                    "keyup",
                    &[],
                    serde_json::json!({"nativeKey":event.keystroke.key,"keyChar":event.keystroke.key_char,"ctrlKey":modifiers.control,"metaKey":modifiers.platform,"altKey":modifiers.alt,"shiftKey":modifiers.shift}),
                    Some(window),
                    cx,
                );
            }
            PlatformInput::ModifiersChanged(event) => {
                let modifiers = &event.modifiers;
                Desktop::dispatch(
                    self.runtime.clone(),
                    id,
                    "modifiers",
                    &[],
                    serde_json::json!({"ctrlKey":modifiers.control,"metaKey":modifiers.platform,"altKey":modifiers.alt,"shiftKey":modifiers.shift}),
                    Some(window),
                    cx,
                );
            }
            PlatformInput::FileDrop(FileDropEvent::Entered { paths, .. }) => {
                *self.dropped.borrow_mut() = paths.clone()
            }
            PlatformInput::FileDrop(FileDropEvent::Submit { position }) => {
                self.drop_files(*position, window, cx)
            }
            _ => {}
        }
    }

    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<String> {
        let state = self.editing()?;
        let text: Vec<_> = state["text"].as_str()?.encode_utf16().collect();
        let end = range.end.min(text.len());
        let range = range.start.min(end)..end;
        *actual = Some(range.clone());
        Some(String::from_utf16_lossy(&text[range]))
    }

    fn selected_text_range(&mut self, _: &mut Window, _: &mut App) -> Option<Utf16Selection> {
        let state = self.editing()?;
        let start = state["start"].as_u64()? as usize;
        let end = state["end"].as_u64()? as usize;
        Some(Utf16Selection {
            range: start.min(end)..start.max(end),
            reversed: start > end,
        })
    }

    fn marked_text_range(&mut self, _: &mut Window, _: &mut App) -> Option<Range<usize>> {
        let state = self.editing()?;
        Some(state["marked"][0].as_u64()? as usize..state["marked"][1].as_u64()? as usize)
            .filter(|range| range.start <= range.end)
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(state) = self.editing()
            && let (Some(start), Some(end), Some(text)) = (
                state["marked"][0].as_u64(),
                state["marked"][1].as_u64(),
                state["text"].as_str(),
            )
        {
            let text: Vec<_> = text.encode_utf16().collect();
            if start <= end && (end as usize) <= text.len() {
                let marked = String::from_utf16_lossy(&text[start as usize..end as usize]);
                self.input(
                    &marked,
                    Some(start as usize..end as usize),
                    false,
                    None,
                    window,
                    cx,
                );
            }
        }
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.input(text, range, false, None, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.input(text, range, true, selected, window, cx);
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let state = self.editing()?;
        let id = state["id"].as_u64()? as usize;
        let host = self.host.borrow();
        let bounds = host.dom.get(&self.id)?.bounds.get(&id)?;
        Some(Bounds::new(
            point(px(bounds[0]), px(bounds[1])),
            size(px(bounds[2]), px(bounds[3])),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        _: &mut App,
    ) -> Option<usize> {
        let state = self.editing()?;
        let id = state["id"].as_u64()? as usize;
        let host = self.host.borrow();
        let dom = host.dom.get(&self.id)?;
        let bounds = dom.bounds.get(&id)?;
        let style = dom.computed.as_ref()?.values.get(id)?;
        let face = style::font(style);
        let text = state["text"].as_str()?;
        let font_size = style::length(
            style.get(Prop::FONT_SIZE).unwrap_or("16px"),
            16.,
            16.,
            bounds[2],
            bounds[3],
        );
        let line_height = style::length(
            style.get(Prop::LINE_HEIGHT).unwrap_or("20px"),
            font_size,
            font_size,
            bounds[2],
            bounds[3],
        )
        .max(1.);
        let row = ((f32::from(point.y) - bounds[1]).max(0.) / line_height) as usize;
        let mut offset = 0;
        let mut selected = "";
        for (index, line) in text.split('\n').enumerate() {
            selected = line;
            if index == row {
                break;
            }
            offset += line.encode_utf16().count() + 1;
        }
        let run = renderer::TextRun {
            len: selected.len(),
            font: face,
            color: renderer::black(),
            ..Default::default()
        };
        let line = window
            .text_system()
            .shape_line(selected.into(), px(font_size), &[run], None);
        let index = line.closest_index_for_x(point.x - px(bounds[0]));
        Some((offset + selected[..index].encode_utf16().count()).min(text.encode_utf16().count()))
    }
}
