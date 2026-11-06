use super::{PageState, PageView};
use crate::style::Prop;
use crate::{desktop::Desktop, style};
use anyhow::{Context as _, Result, ensure};
use renderer::{
    App, Bounds, FileDropEvent, Pixels, PlatformInput, Point, Utf16Selection, Window, point, px,
    size,
};
use std::{
    ops::Range,
    rc::Rc,
    time::{Duration, Instant},
};

/// A field's selection while focused, and the length of the text it drew.
pub(super) type Caret = (Option<[usize; 2]>, usize);

/// A caret's node, offset and text length, when it moved, and the timer of its next blink by phase.
pub(super) type Blink = ([usize; 3], Instant, Option<(u32, renderer::Timer)>);

/// Paints the caret of a collapsed selection. Like browsers, it shows for 500 ms after it moves,
/// then blinks every 500 ms. Inactive windows and `caret-color: transparent` show no caret.
pub(super) fn paint_caret(
    view: &PageState,
    key: [usize; 3],
    bounds: Bounds<Pixels>,
    color: renderer::Rgba,
    window: &mut Window,
) {
    const BLINK: Duration = Duration::from_millis(500);
    if color.a == 0. || !window.is_window_active() {
        return;
    }
    let now = Instant::now();
    let mut caret = view.caret.borrow_mut();
    if caret.as_ref().is_some_and(|(old, ..)| *old != key) {
        *caret = None;
    }
    let (_, since, wake) = caret.get_or_insert((key, now, None));
    // WARNING: a timer can fire just before its time, so the phase rounds up within 8 ms.
    let phase = ((now - *since + Duration::from_millis(8)).as_millis() / BLINK.as_millis()) as u32;
    if wake.as_ref().is_none_or(|(old, _)| *old != phase) {
        let due = *since + BLINK * (phase + 1);
        *wake = Some((phase, window.timer(due - now, |window, _| window.refresh())));
    }
    if phase.is_multiple_of(2) {
        window.paint_quad(renderer::fill(bounds, color));
    }
}

/// The text of an `input` or `textarea`: its value or placeholder, the selection and the caret. It
/// scrolls its box to keep a moved caret in view, and records its layout for hit tests and IME.
pub(super) struct FieldText {
    pub(super) id: usize,
    pub(super) view: Rc<PageState>,
    pub(super) text: Rc<str>,
    /// The value of a password field, whose `text` shows a bullet for each code point.
    pub(super) password: Option<String>,
    pub(super) run: renderer::TextRun,
    pub(super) placeholder: bool,
    pub(super) single_line: bool,
    pub(super) caret: renderer::Rgba,
    pub(super) selection: (Option<u32>, u32),
    /// The scroll position of the box that clips the text.
    pub(super) handle: renderer::ScrollHandle,
    /// The selection while the field has focus, from prepaint.
    pub(super) selected: std::cell::Cell<Option<[usize; 2]>>,
}

impl From<FieldText> for renderer::Child {
    fn from(field: FieldText) -> Self {
        renderer::text(field.text.clone(), Some(vec![field.run.clone()]))
            .hooks(field)
            .into()
    }
}

/// The width a field with a percentage width adds to its parent's max-content size. Like Chrome's
/// compressible controls, it adds nothing to the min-content size, and fills the field once its width is known.
pub(super) fn natural_width(natural: Pixels) -> renderer::Node {
    renderer::node().measure(natural, |&natural, _, known, available, _| {
        let width = known.width.unwrap_or(match available.width {
            renderer::AvailableSpace::MinContent => px(0.),
            renderer::AvailableSpace::MaxContent => natural,
            renderer::AvailableSpace::Definite(space) => px(natural.0.min(space.0)),
        });
        (size(width, px(0.)), None)
    })
}

impl renderer::Hooks for FieldText {
    fn prepaint(&self, cx: &mut renderer::NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        inner(cx.window);
        let Some(layout) = cx.text_layout().cloned() else {
            return;
        };
        let window = &mut *cx.window;
        let view = &*self.view;
        let mut host = view.host.borrow_mut();
        let dom = host.dom.entry(view.id).or_default();
        // An empty field shows its caret before the placeholder.
        let selected = dom
            .selection
            .get(&self.id)
            .filter(|_| dom.focus == Some(self.id))
            .map(|&selected| match &self.password {
                _ if self.placeholder => [0; 2],
                Some(value) => selected.map(|offset| crate::dom::bullets(value, offset)),
                None => selected,
            });
        if !self.placeholder {
            dom.text_layouts.insert(
                self.id,
                (
                    layout.clone(),
                    self.text.clone(),
                    0..self.text.len(),
                    Rc::default(),
                    [px(0.), layout.line_height()],
                ),
            );
        }
        drop(host);
        // Like browsers, a moved caret scrolls into view, and a single-line field scrolls back when it loses focus.
        let state = (selected, self.text.len());
        if view.fields.borrow_mut().insert(self.id, state) != Some(state) {
            let (shown, old) = (self.handle.bounds(), self.handle.offset());
            let mut offset = old;
            match selected {
                Some([_, end]) => {
                    if let Some(caret) =
                        layout.position_for_index(crate::dom::utf16_index(&self.text, end))
                    {
                        let line = layout.line_height();
                        if caret.x < shown.left() {
                            offset.x += shown.left() - caret.x;
                        } else if caret.x + px(1.) > shown.right() {
                            offset.x -= caret.x + px(1.) - shown.right();
                        }
                        if caret.y < shown.top() {
                            offset.y += shown.top() - caret.y;
                        } else if caret.y + line > shown.bottom() {
                            offset.y -= caret.y + line - shown.bottom();
                        }
                    }
                }
                None if self.single_line => offset = Point::default(),
                None => {}
            }
            let max = self.handle.max_offset();
            offset.x = offset.x.clamp(-max.x, px(0.));
            offset.y = offset.y.clamp(-max.y, px(0.));
            if offset != old {
                self.handle.set_offset(offset);
                window.redraw();
            }
        }
        self.selected.set(selected);
    }

    // WARNING: the field paints its own rows, so the text node's lines are not painted.
    fn paint(&self, cx: &mut renderer::NodeCx<'_>, _: &mut dyn FnMut(&mut Window)) {
        let Some(layout) = cx.text_layout().cloned() else {
            return;
        };
        let window = &mut *cx.window;
        let selected = self.selected.get();
        let text = &*self.text;
        let line_height = layout.line_height();
        let selected = selected.map(|[a, b]| {
            [a.min(b), a.max(b), b].map(|index| crate::dom::utf16_index(text, index))
        });
        let highlights: Vec<_> = selected
            .filter(|[start, end, _]| start < end)
            .map(|[start, end, _]| super::text::TextHighlight {
                range: start..end,
                color: self.selection.0.map(renderer::rgba),
                background: renderer::rgba(self.selection.1),
            })
            .into_iter()
            .collect();
        let strut = window.text_system().resolve_font(&self.run.font);
        // WARNING: a long field shapes all its text once, but paints only the rows its box shows.
        let mask = window.content_mask().bounds;
        let mut origin = layout.bounds().origin;
        let mut offset = 0;
        for line in layout.line_layouts() {
            let height = line_height * (line.wrap_boundaries.len() + 1);
            if origin.y >= mask.bottom() {
                break;
            }
            if origin.y + height > mask.top() {
                super::text::paint_text(
                    &line.unwrapped_layout,
                    &line.wrap_boundaries,
                    origin,
                    line_height,
                    std::slice::from_ref(&self.run),
                    offset,
                    &highlights,
                    &[],
                    text,
                    None,
                    strut,
                    window,
                );
            }
            origin.y += height;
            offset += line.len() + 1;
        }
        if let Some([start, end, caret]) = selected
            && start == end
            && let Some(at) = layout.position_for_index(caret)
        {
            paint_caret(
                &self.view,
                [self.id, caret, text.len()],
                Bounds::new(at, size(px(1.), line_height)),
                self.caret,
                window,
            );
        }
    }
}

impl PageState {
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

    /// `dragenter`, `dragover` and `dragleave` of files dragged from other apps.
    fn drag_files(&self, kind: &str, position: Point<Pixels>, window: &mut Window, cx: &mut App) {
        let names: Vec<_> = self
            .dropped
            .borrow()
            .iter()
            .map(|path| {
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        let script = format!(
            "__appsDrag({},{},{},{})",
            serde_json::json!(kind),
            f32::from(position.x),
            f32::from(position.y),
            serde_json::json!(names),
        );
        if let Err(error) = super::scripted(cx, Some(window), || self.evaluate(&script)) {
            eprintln!("apps: {error}");
        }
        self.drive(cx);
        window.refresh();
    }

    fn drop_files(&self, position: Point<Pixels>, window: &mut Window, cx: &mut App) {
        let mut files = Vec::new();
        for path in self.dropped.take() {
            let result = (|| -> Result<serde_json::Value> {
                let metadata = std::fs::metadata(&path)?;
                let directory = metadata.is_dir();
                ensure!(
                    directory || metadata.is_file() && metadata.len() <= 64 * 1024 * 1024,
                    "Dropped file exceeds 64 MB or is not a file"
                );
                let bytes = if directory {
                    Vec::new()
                } else {
                    std::fs::read(&path)?
                };
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |time| time.as_millis() as u64);
                let mut host = self.host.borrow_mut();
                let paths = host.dropped.entry(self.id).or_default();
                paths.push(path.to_string_lossy().into_owned());
                Ok(
                    serde_json::json!({"directory":directory,"name":path.file_name().unwrap_or_default().to_string_lossy(),"file":paths.len() - 1,"bytes":bytes,"lastModified":modified}),
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

/// The edited value of field `id` when it is a password, whose layout shows bullets.
fn password<'a>(
    host: &crate::host::Host,
    view: u64,
    id: usize,
    state: &'a serde_json::Value,
) -> Option<&'a str> {
    host.pages
        .get(&view)?
        .doc
        .nodes
        .get(id)?
        .password()
        .then(|| state["text"].as_str())?
}

impl PageView {
    /// Asks the main side's `before-input-event`, and answers whether it prevented the key.
    fn before_input(&self, kind: &str, event: &renderer::KeyDownEvent, _cx: &mut App) -> bool {
        if !crate::desktop::link::INPUT_LISTENERS.load(std::sync::atomic::Ordering::Relaxed) {
            return false;
        }
        let Some(link) = self.host.borrow().link.clone() else {
            return false;
        };
        let keystroke = &event.keystroke;
        let modifiers = &keystroke.modifiers;
        let input = serde_json::json!({
            "type": kind,
            "key": keystroke.key_char.clone().unwrap_or_else(|| keystroke.key.clone()),
            "code": keystroke.key,
            "isAutoRepeat": event.is_held,
            "isComposing": false,
            "shift": modifiers.shift,
            "control": modifiers.control,
            "alt": modifiers.alt,
            "meta": modifiers.platform,
            "location": 0,
            "modifiers": [],
        });
        let id = self.id;
        link.ask(|answer| crate::desktop::link::ToMain::Input(id, input, answer))
            .is_ok_and(|answer| answer.value == true)
    }
}

impl renderer::View for PageView {
    fn render(
        &mut self,
        tree: &renderer::Tree,
        window: &mut Window,
        cx: &mut App,
    ) -> renderer::Child {
        if !window.redrawing()
            && let Some(hud) = &*self.hud.borrow()
        {
            hud.started.set(std::time::Instant::now());
        }
        self.render_page(tree, window, cx)
    }

    fn window_changed(&mut self, _: &mut Window, cx: &mut App) {
        self.origin.set(None);
        if let Ok(mut host) = self.host.try_borrow_mut()
            && !host.window_changes.contains(&self.id)
        {
            host.window_changes.push(self.id);
        }
        self.drive(cx);
    }

    fn event(&mut self, event: &PlatformInput, window: &mut Window, cx: &mut App) {
        let id = self.id;
        match event {
            PlatformInput::KeyDown(event) => {
                let key = event.keystroke.key.as_str();
                let modifiers = &event.keystroke.modifiers;
                if !self.host.borrow().packaged
                    && key == "f"
                    && modifiers.alt
                    && if cfg!(target_os = "macos") {
                        modifiers.platform
                    } else {
                        modifiers.control
                    }
                {
                    super::hud::Hud::toggle(&self.0, window);
                    cx.stop_propagation();
                    return;
                }
                if !self.host.borrow().packaged
                    && (key == "f12"
                        || (key == "i"
                            && ((modifiers.platform && modifiers.alt)
                                || (modifiers.control && modifiers.shift))))
                {
                    self.host
                        .borrow_mut()
                        .command(serde_json::json!({"op":"window","args":["devtools",id]}));
                    self.drive(cx);
                    cx.stop_propagation();
                    return;
                }
                if self.before_input("keyDown", event, cx) {
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
                    let renderer = self
                        .runtime
                        .borrow()
                        .views
                        .get(&id)
                        .and_then(|view| view.renderer.clone());
                    if let Some(renderer) = renderer {
                        let command = match key {
                            "c" => "copy",
                            "x" => "cut",
                            _ => "paste",
                        };
                        super::edit(&renderer, Some((cx, Some(&mut *window))), command, "");
                    }
                    cx.stop_propagation();
                    window.refresh();
                }
            }
            PlatformInput::KeyUp(event) => {
                let modifiers = &event.keystroke.modifiers;
                let held = renderer::KeyDownEvent {
                    keystroke: event.keystroke.clone(),
                    is_held: false,
                };
                if self.before_input("keyUp", &held, cx) {
                    return;
                }
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
            PlatformInput::FileDrop(FileDropEvent::Entered { paths, position }) => {
                *self.dropped.borrow_mut() = paths.clone();
                self.drag_files("enter", *position, window, cx);
            }
            PlatformInput::FileDrop(FileDropEvent::Pending { position }) => {
                self.drag_files("over", *position, window, cx)
            }
            PlatformInput::FileDrop(FileDropEvent::Exited) => {
                self.drag_files("leave", Point::default(), window, cx)
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
        range: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let state = self.editing()?;
        // The IME asks right after an edit, before the next frame lays out the new text.
        super::scripted(cx, Some(window), || super::reflow(self.id));
        let id = state["id"].as_u64()? as usize;
        let host = self.host.borrow();
        let dom = host.dom.get(&self.id)?;
        let password = password(&host, self.id, id, &state);
        // A contenteditable host has no text layout itself, so the range starts in one of its text
        // nodes: the caret's node when it starts at the caret (a line start is also the end of the
        // line before), else the first node that reaches it.
        let caret = state["node"]
            .as_u64()
            .map(|node| node as usize)
            .filter(|node| {
                state["end"].as_u64() == Some(range.start as u64)
                    && dom.text_layouts.contains_key(node)
            });
        let (node, range) = if dom.text_layouts.contains_key(&id) {
            (id, range)
        } else if let Some(node) = caret {
            let at = state["offset"].as_u64()? as usize;
            (node, at..at + range.len())
        } else {
            let nodes = &host.pages.get(&self.id)?.doc.nodes;
            let (mut offset, mut pending, mut found) = (0, vec![id], None);
            while let Some(node) = pending.pop() {
                let current = nodes.get(node)?;
                if current.tag != "#text" {
                    pending.extend(current.children.iter().rev());
                    continue;
                }
                let length = current.text.encode_utf16().count();
                if range.start <= offset + length {
                    found = Some((node, range.start - offset..range.end - offset));
                    break;
                }
                offset += length;
            }
            found.unwrap_or((id, range))
        };
        // A field's text has a layout, so the range gets its own rectangle, where the IME shows its candidates.
        if let Some((layout, text, span, rows, _)) = dom.text_layouts.get(&node) {
            let content = &text[span.clone()];
            let [start, end] = [range.start, range.end].map(|index| {
                let index = password.map_or(index, |value| crate::dom::bullets(value, index));
                span.start + crate::dom::utf16_index(content, index)
            });
            let at = super::text::text_position(layout, text, rows, start)?;
            let to = super::text::text_position(layout, text, rows, end)
                .filter(|to| to.y == at.y && to.x >= at.x)
                .unwrap_or(at);
            return Some(Bounds::new(at, size(to.x - at.x, layout.line_height())));
        }
        let bounds = dom.bounds.get(&id)?;
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
        if let Some((layout, text, span, rows, _)) = dom.text_layouts.get(&id) {
            let index = super::text::text_index(layout, text, rows, point)
                .unwrap_or_else(|index| index)
                .clamp(span.start, span.end);
            let index = text[span.start..index].encode_utf16().count();
            return Some(
                password(&host, self.id, id, &state)
                    .map_or(index, |value| crate::dom::unbullet(value, index)),
            );
        }
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
            .shape_line(selected, px(font_size), &[run], None);
        let index = line.closest_index_for_x(point.x - px(bounds[0]));
        Some((offset + selected[..index].encode_utf16().count()).min(text.encode_utf16().count()))
    }
}
