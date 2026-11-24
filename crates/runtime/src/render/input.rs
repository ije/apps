use super::inspect::{Tool, tool};
use super::{PageView, ViewState};
use crate::css;
use crate::css::properties::Prop;
use crate::style;
use anyhow::{Context as _, Result, ensure};
use renderer::{App, Event, Pixels, Point, Rect, Utf16Selection, Window, point, px, size};
use std::{
    ops::Range,
    rc::Rc,
    time::{Duration, Instant},
};

pub(super) type Caret = (Option<[usize; 2]>, usize);

pub(super) type Blink = ([usize; 3], Instant, Option<(u32, renderer::Timer)>);

pub(super) fn paint_caret(
    view: &ViewState,
    key: [usize; 3],
    bounds: Rect<Pixels>,
    color: renderer::Color,
    window: &mut Window,
) {
    const BLINK: Duration = renderer::CARET_BLINK;
    if color.a == 0. || !window.is_active() {
        return;
    }
    let now = Instant::now();
    let mut caret = view.caret.borrow_mut();
    if caret.as_ref().is_some_and(|(old, ..)| *old != key) {
        *caret = None;
    }
    let (_, since, wake) = caret.get_or_insert((key, now, None));
    // A timer can fire just before its time, so the phase rounds up within 8 ms.
    let phase = ((now - *since + Duration::from_millis(8)).as_millis() / BLINK.as_millis()) as u32;
    // The first phase draws, so typing and scrolling move the caret in the same frame as the text.
    if phase > 0 && window.overlay_caret(bounds, color, *since, phase.is_multiple_of(2)) {
        *wake = None;
        return;
    }
    if wake.as_ref().is_none_or(|(old, _)| *old != phase) {
        let due = *since + BLINK * (phase + 1);
        *wake = Some((phase, window.timer(due - now, |window, _| window.refresh())));
    }
    if phase.is_multiple_of(2) {
        window.draw_rect(bounds, color);
    }
}

// The rows of `aligned_rows` with the lines, width and alignment they are for.
type KeptRows = (
    Rc<[renderer::TextLine]>,
    u32,
    u8,
    crate::render::layout::Rows,
);

pub(super) struct FieldText {
    pub(super) id: usize,
    pub(super) view: Rc<ViewState>,
    pub(super) text: Rc<str>,
    pub(super) password: Option<String>,
    pub(super) run: renderer::TextRun,
    pub(super) placeholder: bool,
    pub(super) single_line: bool,
    /// A right-to-left field whose rows start at the right. The caret is on the left of its place.
    pub(super) rtl: bool,
    /// The place of each row for `text-align`, for the lines, width and alignment of the last prepaint.
    pub(super) rows: std::cell::RefCell<Option<KeptRows>>,
    pub(super) caret: renderer::Color,
    pub(super) selection: (Option<u32>, u32),
    pub(super) handle: renderer::ScrollRef,
    pub(super) selected: std::cell::Cell<Option<[usize; 2]>>,
}

impl From<FieldText> for renderer::Node {
    fn from(field: FieldText) -> Self {
        renderer::text(field.text.clone(), Some(vec![field.run.clone()])).hooks(field)
    }
}

impl From<FieldText> for renderer::Child {
    fn from(field: FieldText) -> Self {
        renderer::Node::from(field).into()
    }
}

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

// Where each row starts for `text-align`, as the renderer places them. Left-aligned text needs none.
fn aligned_rows(
    layout: &renderer::Paragraph,
    align: renderer::TextAlign,
    rtl: bool,
) -> crate::render::layout::Rows {
    if align == renderer::TextAlign::Left {
        return Rc::default();
    }
    let width = layout.bounds().size.width;
    let mut rows = Vec::new();
    let mut offset = 0;
    for line in layout.lines().iter() {
        let glyphs = &line.line.glyphs;
        let (mut left, mut start) = (px(0.), offset);
        for boundary in line.breaks.iter().map(Some).chain([None]) {
            let next = boundary.map_or(line.line.width, |&at| glyphs[at as usize].x);
            // A space at the end of a wrapped row hangs past its edge.
            let end = boundary
                .and_then(|&at| (at as usize).checked_sub(1))
                .map(|at| &glyphs[at])
                .filter(|glyph| line.text.as_bytes().get(glyph.index as usize) == Some(&b' '))
                .map_or(next, |glyph| glyph.x);
            let free = width - (end - left);
            // A row that overflows starts at its start edge, the right one in right-to-left text.
            let shift = match align {
                _ if free < px(0.) => {
                    if rtl {
                        free
                    } else {
                        px(0.)
                    }
                }
                renderer::TextAlign::Center => free / 2.,
                _ => free,
            };
            rows.push(crate::render::layout::Row {
                shift,
                right: shift + end - left,
                gap: px(0.),
                start,
                drop: px(0.),
                above: px(0.),
            });
            if let Some(&at) = boundary {
                (left, start) = (next, offset + glyphs[at as usize].index as usize);
            }
        }
        offset += line.len() + 1;
    }
    rows.into()
}

impl renderer::Hooks for FieldText {
    fn overflows_left(&self) -> bool {
        self.rtl
    }

    fn prepaint(&self, cx: &mut renderer::NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        inner(cx.window);
        let Some(layout) = cx.text_layout().cloned() else {
            return;
        };
        let rows = {
            let (lines, align) = (layout.lines(), cx.text.text_align);
            let width = f32::from(layout.bounds().size.width).to_bits();
            let mut kept = self.rows.borrow_mut();
            match &*kept {
                Some((old, at, by, rows))
                    if Rc::ptr_eq(old, &lines) && *at == width && *by == align as u8 =>
                {
                    rows.clone()
                }
                _ => {
                    let rows = aligned_rows(&layout, align, self.rtl);
                    *kept = Some((lines, width, align as u8, rows.clone()));
                    rows
                }
            }
        };
        let window = &mut *cx.window;
        let view = &*self.view;
        let mut host = view.host.borrow_mut();
        let dom = host.dom.entry(view.id).or_default();
        let selected = dom
            .interaction
            .selection
            .get(&self.id)
            .filter(|_| dom.interaction.focus == Some(self.id))
            .map(|&selected| match &self.password {
                _ if self.placeholder => [0; 2],
                Some(value) => selected.map(|offset| crate::dom::bullets(value, offset)),
                None => selected,
            });
        if !self.placeholder {
            dom.layout.text_layouts.insert(
                self.id,
                (
                    layout.clone(),
                    self.text.clone(),
                    0..self.text.len(),
                    rows.clone(),
                    [px(0.), layout.line_height()],
                ),
            );
        }
        drop(host);
        let state = (selected, self.text.len());
        if view.fields.borrow_mut().insert(self.id, state) != Some(state) {
            let (shown, old) = (self.handle.bounds(), self.handle.offset());
            let mut offset = old;
            match selected {
                Some([_, end]) => {
                    let index = crate::dom::utf16_index(&self.text, end);
                    if let Some(caret) =
                        super::text::text_position(&layout, &self.text, &rows, index)
                    {
                        let line = layout.line_height();
                        let left = if self.rtl { caret.x - px(1.) } else { caret.x };
                        if left < shown.left() {
                            offset.x += shown.left() - left;
                        } else if left + px(1.) > shown.right() {
                            offset.x -= left + px(1.) - shown.right();
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
            // Layout sums leave a few thousandths of a pixel, so the offset snaps to device pixels.
            let scale = window.scale_factor();
            let offset =
                (self.handle.clamped(offset)).map(|value| px((value.0 * scale).round() / scale));
            if offset != old {
                self.handle.set_offset(offset);
                window.redraw();
            }
        }
        // A focused field draws its caret in each frame.
        if self.selected.replace(selected) != selected || selected.is_some() {
            window.unsettle();
        }
    }

    fn volatile(&self) -> bool {
        false
    }

    fn kept(&self, cx: &mut renderer::NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        self.prepaint(cx, inner);
    }

    fn cached(&self) -> bool {
        true
    }

    fn bounded(&self) -> bool {
        true
    }

    fn paint(&self, cx: &mut renderer::NodeCx<'_>, _: &mut dyn FnMut(&mut Window)) {
        let Some(layout) = cx.text_layout().cloned() else {
            return;
        };
        let window = &mut *cx.window;
        let selected = self.selected.get();
        if selected.is_some() {
            window.paint_volatile();
        }
        let text = &*self.text;
        let line_height = layout.line_height();
        let selected = selected.map(|[a, b]| {
            [a.min(b), a.max(b), b].map(|index| crate::dom::utf16_index(text, index))
        });
        let highlights: Vec<_> = selected
            .filter(|[start, end, _]| start < end)
            .map(|[start, end, _]| super::text::TextHighlight {
                range: start..end,
                color: self.selection.0.map(renderer::Color::hex),
                background: renderer::Color::hex(self.selection.1),
                stroke: true,
            })
            .into_iter()
            .collect();
        let strut = window.text_system().face(&self.run.font);
        let mask = window.content_mask().bounds;
        let rows = (self.rows.borrow().as_ref()).map_or_else(Rc::default, |kept| kept.3.clone());
        let mut origin = layout.bounds().origin;
        let (mut offset, mut row) = (0, 0);
        for line in layout.lines().iter() {
            let first = row.min(rows.len());
            row += line.rows();
            let height = line_height * line.rows();
            if origin.y >= mask.bottom() {
                window.cull();
                break;
            }
            if origin.y + height <= mask.top() {
                window.cull();
            } else {
                super::text::paint_text(
                    line,
                    origin,
                    line_height,
                    std::slice::from_ref(&self.run),
                    offset,
                    &highlights,
                    &rows[first..row.min(rows.len())],
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
            && let Some(at) = super::text::text_position(&layout, text, &rows, caret)
        {
            paint_caret(
                &self.view,
                [self.id, caret, text.len()],
                Rect::new(at, size(px(1.), line_height)),
                self.caret,
                window,
            );
        }
    }
}

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
            let dom = host.dom.entry(self.id).or_default();
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
            let dom = host.dom.entry(self.id).or_default();
            // An element that is no longer drawable gives its pixels back.
            if let Some(page) = host.pages.get(&self.id).filter(|_| !snapshots.is_empty()) {
                let drawable = |id: &usize| {
                    (page.doc.nodes.get(*id))
                        .is_some_and(|node| node.attrs.contains_key("drawable"))
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
            if let Some(state) = host.dom.get_mut(&self.id) {
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
        let dom = host.dom.get(&self.id)?;
        let password = password(&host, self.id, id, &state);
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
                let index = password.map_or(index, |value| crate::dom::bullets(value, index));
                span.start + crate::dom::utf16_index(content, index)
            });
            let at = super::text::text_position(layout, text, rows, start)?;
            let to = super::text::text_position(layout, text, rows, end)
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
        let dom = host.dom.get(&self.id)?;
        if let Some((layout, text, span, rows, _)) = dom.layout.text_layouts.get(&id) {
            let [dx, dy] = dom.geometry.scroll_shift(&dom.styles.walk, id);
            let point = point - renderer::point(px(dx), px(dy));
            let index = super::text::text_index(layout, text, rows, point, false)
                .unwrap_or_else(|index| index)
                .clamp(span.start, span.end);
            let index = text[span.start..index].encode_utf16().count();
            return Some(
                password(&host, self.id, id, &state)
                    .map_or(index, |value| crate::dom::unbullet(value, index)),
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
