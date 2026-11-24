use super::inspect::{Tool, tool};
use super::{PageView, ViewState};
use crate::css;
use crate::css::properties::Prop;
use crate::style;
use anyhow::{Context as _, Result, ensure};
use renderer::{App, Event, Pixels, Point, Rect, Utf16Selection, Window, point, px, size};
use std::{ops::Range, rc::Rc};

impl ViewState {
    fn evaluate(&self, script: &str) -> Result<String> {
        let renderer = self
            .runtime
            .renderer(self.id)
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
        self.run(&script, window, cx);
    }

    fn run(&self, script: &str, window: &mut Window, cx: &mut App) {
        if let Err(error) = super::scripted(cx, Some(window), || self.evaluate(script)) {
            eprintln!("apps: {error}");
        }
        self.drive(cx);
        window.refresh();
    }

    pub(super) fn drive(&self, cx: &mut App) {
        let runtime = self.runtime.clone();
        cx.defer(move |cx| runtime.drive(cx));
    }

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
        self.run(&script, window, cx);
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
        self.run(&script, window, cx);
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

    fn drawn(&mut self, snapshots: Vec<renderer::Snapshot>, window: &mut Window, cx: &mut App) {
        // The paint order of boxes, which hit tests sort by.
        if let Ok(mut host) = self.host.try_borrow_mut() {
            let dom = host.state(self.id);
            dom.layout.painted.fill(0);
            let page = self.id;
            for (order, &mark) in window.marks().iter().enumerate() {
                let id = (mark & u64::from(u32::MAX)) as usize;
                if mark >> 32 != page {
                    continue;
                }
                if dom.layout.painted.len() <= id {
                    dom.layout.painted.resize(id + 1, 0);
                }
                dom.layout.painted[id] = order as u64 + 1;
            }
        }
        if snapshots.is_empty() && !self.paint.take() {
            return;
        }
        let mut changed = Vec::with_capacity(snapshots.len());
        {
            let mut host = self.host.borrow_mut();
            let host = &mut *host;
            let (doc, dom, _) = host.page_parts(self.id);
            // An element that is no longer drawable gives its pixels back.
            if let Some(doc) = doc.filter(|_| !snapshots.is_empty()) {
                let drawable = |id: &usize| {
                    (doc.nodes.get(*id)).is_some_and(|node| node.attrs.contains_key("drawable"))
                };
                dom.media.snapshots.retain(|id, _| drawable(id));
                dom.geometry.drawn.retain(|id, _| drawable(id));
            }
            for renderer::Snapshot { key, size, pixels } in snapshots {
                if let Some(pixels) = tiny_skia::IntSize::from_wh(size[0], size[1])
                    .and_then(|size| tiny_skia::Pixmap::from_vec(pixels, size))
                {
                    dom.media
                        .snapshots
                        .insert(key as usize, (Rc::new(pixels), window.scale_factor()));
                    changed.push(key);
                }
            }
        }
        self.paint.set(false);
        // The script can lay out the page, which needs the view that this call holds.
        let view = self.0.clone();
        cx.defer(move |cx| {
            cx.update_window(view.window, |window, cx| {
                let renderer = view.runtime.try_renderer(view.id);
                if let Some(renderer) = renderer
                    && let Err(error) = super::scripted(cx, Some(window), || {
                        renderer.eval(&format!("__appsPaint({changed:?})"), "apps:paint")
                    })
                {
                    eprintln!("apps: {error}");
                }
                let runtime = view.runtime.clone();
                cx.defer(move |cx| runtime.drive(cx));
            });
        });
    }

    fn window_changed(&mut self, window: &mut Window, cx: &mut App) {
        self.origin.set(None);
        if let Ok(mut host) = self.host.try_borrow_mut() {
            if let Some(state) = host.page_state_mut(self.id) {
                state.window = window.window_state();
            }
            if !host.window_changes.contains(&self.id) {
                host.window_changes.push(self.id);
            }
        }
        self.drive(cx);
    }

    fn event(&mut self, event: &Event, window: &mut Window, cx: &mut App) {
        let id = self.id;
        match event {
            Event::KeyDown {
                key: named,
                modifiers,
                repeat,
                ime,
            } => {
                let key = named.name.as_str();
                let pick = (key == "escape" && self.inspect.get().pick).then_some(Tool::Pick);
                let tool = pick
                    .or_else(|| tool(key, *modifiers).filter(|_| !self.host.borrow().app.packaged));
                let open = self.inspect.get().open;
                let args = match tool {
                    Some(Tool::Hud) => serde_json::json!(["hud", id]),
                    Some(Tool::DevTools) => serde_json::json!(["devtools", id]),
                    Some(Tool::Console) => serde_json::json!(["devtools", id, "console"]),
                    Some(Tool::Pick) if open => {
                        serde_json::json!(["inspect", id, (key == "escape").then_some(false)])
                    }
                    Some(Tool::Pick) => serde_json::json!(["devtools", id, "pick"]),
                    None => serde_json::Value::Null,
                };
                if !args.is_null() {
                    self.host.borrow_mut().command(
                        crate::host::link::Target::Window(id),
                        serde_json::json!({"op":"window","args":args}),
                    );
                    self.drive(cx);
                    cx.stop_event();
                    return;
                }
                let allowed = self.runtime.dispatch(
                    id,
                    "keydown",
                    &[],
                    serde_json::json!({"nativeKey":named.name,"keyChar":named.text,"ctrlKey":modifiers.ctrl(),"metaKey":modifiers.command(),"altKey":modifiers.alt(),"shiftKey":modifiers.shift(),"repeat":repeat,"ime":ime}),
                    Some(window),
                    cx,
                );
                if !allowed {
                    cx.stop_event();
                    return;
                }
                if *ime {
                    return;
                }
                if cfg!(target_os = "macos")
                    && modifiers.command()
                    && !modifiers.ctrl()
                    && !modifiers.alt()
                    && key == "q"
                {
                    cx.stop_event();
                    cx.quit();
                    return;
                }
                let primary = if cfg!(target_os = "macos") {
                    modifiers.command() && !modifiers.ctrl()
                } else {
                    modifiers.ctrl() && !modifiers.command()
                };
                if primary && !modifiers.alt() && matches!(key, "c" | "x" | "v") {
                    let renderer = self.runtime.renderer(id);
                    if let Some(renderer) = renderer {
                        let command = match key {
                            "c" => "copy",
                            "x" => "cut",
                            _ => "paste",
                        };
                        super::edit(&renderer, Some((cx, Some(&mut *window))), command, "");
                    }
                    cx.stop_event();
                    window.refresh();
                }
            }
            Event::KeyUp {
                key,
                modifiers,
                ime,
            } => {
                self.runtime.dispatch(
                    id,
                    "keyup",
                    &[],
                    serde_json::json!({"nativeKey":key.name,"keyChar":key.text,"ctrlKey":modifiers.ctrl(),"metaKey":modifiers.command(),"altKey":modifiers.alt(),"shiftKey":modifiers.shift(),"ime":ime}),
                    Some(window),
                    cx,
                );
            }
            Event::ModifiersChanged { modifiers, .. } => {
                self.runtime.dispatch(
                    id,
                    "modifiers",
                    &[],
                    serde_json::json!({"ctrlKey":modifiers.ctrl(),"metaKey":modifiers.command(),"altKey":modifiers.alt(),"shiftKey":modifiers.shift()}),
                    Some(window),
                    cx,
                );
            }
            Event::FileEnter { paths, at } => {
                *self.dropped.borrow_mut() = paths.clone();
                self.drag_files("enter", *at, window, cx);
            }
            Event::FileOver { at } => self.drag_files("over", *at, window, cx),
            Event::FileLeave => self.drag_files("leave", Point::default(), window, cx),
            Event::FileDrop { at } => self.drop_files(*at, window, cx),
            _ => {}
        }
    }

    fn text_in(
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

    fn selection(&mut self, _: &mut Window, _: &mut App) -> Option<Utf16Selection> {
        let state = self.editing()?;
        let start = state["start"].as_u64()? as usize;
        let end = state["end"].as_u64()? as usize;
        Some(Utf16Selection {
            range: start.min(end)..start.max(end),
            reversed: start > end,
        })
    }

    fn composition(&mut self, _: &mut Window, _: &mut App) -> Option<Range<usize>> {
        let state = self.editing()?;
        Some(state["marked"][0].as_u64()? as usize..state["marked"][1].as_u64()? as usize)
            .filter(|range| range.start <= range.end)
    }

    fn end_composition(&mut self, window: &mut Window, cx: &mut App) {
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

    fn insert(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.input(text, range, false, None, window, cx);
    }

    fn compose(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.input(text, range, true, selected, window, cx);
    }

    fn rect_of(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Rect<Pixels>> {
        let state = self.editing()?;
        // The IME asks right after an edit, before the next frame lays out the new text, so reflow first.
        super::scripted(cx, Some(window), || super::reflow(self.id));
        let id = state["id"].as_u64()? as usize;
        let host = self.host.borrow();
        let dom = host.page_state(self.id)?;
        let password = super::paint::field::password(&host, self.id, id, &state);
        let caret = state["node"]
            .as_u64()
            .map(|node| node as usize)
            .filter(|node| {
                state["end"].as_u64() == Some(range.start as u64)
                    && dom.layout.text_layouts.contains_key(node)
            });
        let (node, range) = if dom.layout.text_layouts.contains_key(&id) {
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
        if let Some((layout, text, span, rows, _)) = dom.layout.text_layouts.get(&node) {
            let content = &text[span.clone()];
            let [start, end] = [range.start, range.end].map(|index| {
                let index = password.map_or(index, |value| crate::dom::text::bullets(value, index));
                span.start + crate::dom::text::utf16_index(content, index)
            });
            let at = super::paint::text::text_position(layout, text, rows, start)?;
            let to = super::paint::text::text_position(layout, text, rows, end)
                .filter(|to| to.y == at.y && to.x >= at.x)
                .unwrap_or(at);
            let [dx, dy] = dom.geometry.scroll_shift(&dom.styles.walk, node);
            return Some(Rect::new(
                at + point(px(dx), px(dy)),
                size(to.x - at.x, layout.line_height()),
            ));
        }
        // A terminal composes text at its cursor.
        let caret = dom
            .media
            .terminals
            .borrow()
            .session(id)
            .and_then(|session| Some(self.terminals.borrow().get(&session)?.caret));
        let bounds = caret.as_ref().or_else(|| dom.geometry.mirror.get(&id))?;
        Some(Rect::new(
            point(px(bounds[0]), px(bounds[1])),
            size(px(bounds[2]), px(bounds[3])),
        ))
    }

    fn index_at(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        _: &mut App,
    ) -> Option<usize> {
        let state = self.editing()?;
        let id = state["id"].as_u64()? as usize;
        let host = self.host.borrow();
        let dom = host.page_state(self.id)?;
        if let Some((layout, text, span, rows, _)) = dom.layout.text_layouts.get(&id) {
            let [dx, dy] = dom.geometry.scroll_shift(&dom.styles.walk, id);
            let point = point - renderer::point(px(dx), px(dy));
            let index = super::paint::text::text_index(layout, text, rows, point, false)
                .unwrap_or_else(|index| index)
                .clamp(span.start, span.end);
            let index = text[span.start..index].encode_utf16().count();
            return Some(
                super::paint::field::password(&host, self.id, id, &state)
                    .map_or(index, |value| crate::dom::text::unbullet(value, index)),
            );
        }
        let bounds = dom.geometry.mirror.get(&id)?;
        let style = dom.styles.computed.as_ref()?.values.get(id)?;
        let face = style::font(style);
        let text = state["text"].as_str()?;
        let font_size = css::value::length(
            style.get(Prop::FONT_SIZE).unwrap_or("16px"),
            16.,
            16.,
            bounds[2],
            bounds[3],
        );
        let line_height = css::value::length(
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
            color: renderer::Color::BLACK,
            ..Default::default()
        };
        let line = window
            .text_system()
            .shape_line(selected, px(font_size), &run);
        let index = line.line.nearest_index(point.x - px(bounds[0]));
        Some((offset + selected[..index].encode_utf16().count()).min(text.encode_utf16().count()))
    }
}
