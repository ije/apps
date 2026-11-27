use super::Desktop;
use crate::css::{self, properties::Prop};
use crate::host::{Host, context::JsContext};
use crate::render::{
    Pages, Store, ViewState,
    inspect::{Tool, tool},
    paint,
};
use crate::style;
use anyhow::{Context as _, Result, ensure};
use renderer::{
    App, Event, Pixels, Point, Rect, Utf16Selection, Window, WindowId, point, px, size,
};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    ops::Range,
    path::PathBuf,
    rc::{Rc, Weak},
};

const BUTTONS: [u8; 5] = [1, 4, 2, 8, 16];

type Views = (Weak<ViewState>, Weak<Input>);

thread_local! {
    static SCRIPTS: Cell<Option<(*mut App, Option<*mut Window>)>> = const { Cell::new(None) };
    static VIEWS: RefCell<HashMap<u64, Views>> = RefCell::default();
}

// The window glue of a page view: input, page scripts and the desktop.
pub(crate) struct Input {
    desktop: Rc<RefCell<Desktop>>,
    host: Rc<RefCell<Host>>,
    store: Rc<RefCell<Store>>,
    id: u64,
    window: WindowId,
    queued: Cell<bool>,
    // `__appsLayout` runs at the start of the next frame, and the scroll moves it last saw.
    layout_queued: Cell<bool>,
    moves: Cell<u64>,
    buttons: Cell<u8>,
    origin: Cell<Option<Point<Pixels>>>,
    dropped: RefCell<Vec<PathBuf>>,
    select: RefCell<Option<Select>>,
}

#[derive(Clone)]
pub(crate) struct PageView {
    pub(crate) page: Rc<ViewState>,
    pub(crate) input: Rc<Input>,
}

impl PageView {
    pub(crate) fn new(
        desktop: Rc<RefCell<Desktop>>,
        host: Rc<RefCell<Host>>,
        id: u64,
        window: WindowId,
    ) -> Self {
        let store = host.borrow().store.clone();
        let input = Rc::new(Input {
            desktop,
            host,
            store: store.clone(),
            id,
            window,
            queued: Cell::new(false),
            layout_queued: Cell::new(false),
            moves: Cell::new(0),
            buttons: Cell::new(0),
            origin: Cell::new(None),
            dropped: RefCell::default(),
            select: RefCell::default(),
        });
        let page = ViewState::new(id, input.clone(), store);
        VIEWS.with_borrow_mut(|views| {
            views.retain(|_, (page, _)| page.strong_count() > 0);
            views.insert(id, (Rc::downgrade(&page), Rc::downgrade(&input)));
        });
        Self { page, input }
    }
}

impl Input {
    // As in the HTML rendering steps, the scroll events of a frame run before its animation frame callbacks, so the
    // frame draws what their listeners change with the new offsets. Observers run after its layout, in `laid_out`.
    fn frame(self: &Rc<Self>, window: &mut Window, cx: &mut App) {
        let moves = renderer::scroll_moves();
        if !self.layout_queued.replace(false) && self.moves.get() == moves {
            return;
        }
        self.moves.set(moves);
        if let Some(renderer) = self.try_renderer()
            && let Err(error) = scripted(cx, Some(window), || {
                renderer.eval("__appsLayout()", "apps:layout")
            })
        {
            eprintln!("apps: {error}");
        }
        // A callback that a listener requests runs in this frame.
        if self.store.try_borrow_mut().ok().is_some_and(|mut store| {
            store
                .page_state_mut(self.id)
                .is_some_and(|dom| std::mem::take(&mut dom.frame_requested))
        }) {
            queue_animation_frame(self.clone(), window);
        }
        self.defer_drive(cx);
    }

    fn renderer(&self) -> Option<Rc<JsContext>> {
        self.desktop.borrow().views.get(&self.id)?.renderer.clone()
    }

    fn try_renderer(&self) -> Option<Rc<JsContext>> {
        self.desktop
            .try_borrow()
            .ok()?
            .views
            .get(&self.id)?
            .renderer
            .clone()
    }

    fn evaluate(&self, script: &str) -> Result<String> {
        let renderer = self.renderer().context("Page is not loaded")?;
        renderer.eval(script, "apps:input")
    }

    fn editing(&self) -> Option<serde_json::Value> {
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
        if let Err(error) = scripted(cx, Some(window), || self.evaluate(script)) {
            eprintln!("apps: {error}");
        }
        self.defer_drive(cx);
        window.refresh();
    }

    fn defer_drive(&self, cx: &mut App) {
        let desktop = self.desktop.clone();
        cx.defer(move |cx| Desktop::drive(desktop, cx));
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

    fn screen(&self, window: &Window, at: Point<Pixels>) -> [f32; 2] {
        let origin = self.origin.get().unwrap_or_else(|| {
            let origin = window.window_state().content.origin;
            self.origin.set(Some(origin));
            origin
        });
        let at = origin + at * window.zoom();
        [f32::from(at.x), f32::from(at.y)]
    }

    fn pointer(
        &self,
        window: &Window,
        at: Point<Pixels>,
        modifiers: renderer::Modifiers,
    ) -> serde_json::Value {
        let [x, y] = self.screen(window, at);
        serde_json::json!({"clientX":f32::from(at.x),"clientY":f32::from(at.y),"screenX":x,"screenY":y,"buttons":self.buttons.get(),"ctrlKey":modifiers.ctrl(),"metaKey":modifiers.command(),"altKey":modifiers.alt(),"shiftKey":modifiers.shift()})
    }
}

impl Pages for Input {
    fn drive(&self, cx: &mut App) {
        Desktop::drive(self.desktop.clone(), cx);
    }

    fn queue_layout(self: Rc<Self>, window: &mut Window) {
        if !self.layout_queued.replace(true) {
            window.request_frame();
        }
    }

    // Animation handlers can hide or remove a popup, so run them before its final frame is painted.
    fn animation_events(&self, events: Vec<(usize, &'static str, String, f64)>, cx: &mut App) {
        let renderer = self.renderer();
        for (node, kind, name, elapsed) in events {
            let (pseudo, name) = name.split_once('\0').unwrap_or(("", &name));
            let data = if kind.starts_with("animation") {
                serde_json::json!({"animationName": name, "elapsedTime": elapsed, "pseudoElement": pseudo})
            } else {
                serde_json::json!({"propertyName": name, "elapsedTime": elapsed, "pseudoElement": pseudo})
            };
            if let Some(renderer) = &renderer
                && let Err(error) = renderer.eval(
                    &format!("__appsDispatch('{kind}',[{node}],{data})"),
                    "apps:animation",
                )
            {
                eprintln!("apps: {error}");
            }
        }
        Desktop::drive(self.desktop.clone(), cx);
    }

    fn popup(self: Rc<Self>) -> Option<renderer::Node> {
        let port = self
            .store
            .try_borrow()
            .ok()?
            .viewports
            .get(&self.id)
            .copied();
        let mut guard = self.select.borrow_mut();
        let select = guard.as_mut()?;
        let [x, y, width, height] = select.at;
        // The list goes below the select, or above it when there is more room there, with as many rows as fit.
        let below = port.map_or(f32::MAX, |port| port[1]) - y - height;
        let room = below.max(y);
        select.rows = select
            .items
            .len()
            .min(ROWS)
            .min(((room - 2.) / ROW).max(1.) as usize);
        if let Some(current) = select.current {
            select.go(current as isize, 1);
        }
        let tall = select.rows as f32 * ROW + 2.;
        let top = if below >= tall || below >= y {
            y + height
        } else {
            (y - tall).max(0.)
        };
        let colors = if select.dark {
            [0x3b3b3bff, 0xffffffff, 0x9d9d9dff]
        } else {
            [0xffffffff, 0x000000ff, 0x6d6d6dff]
        };
        let [back, text, muted] = colors.map(renderer::Color::hex);
        let line = renderer::Color::hex(0x767676ff);
        let rows = select
            .items
            .iter()
            .enumerate()
            .skip(select.top)
            .take(select.rows)
            .map(|(index, item)| {
                let row = renderer::node()
                    .h(px(ROW))
                    .flex()
                    .items_center()
                    .pl(px(4.))
                    .pr(px(4.));
                let Some((label, disabled)) = item else {
                    return row.child(renderer::node().h(px(1.)).flex_1().bg(line));
                };
                let disabled = *disabled;
                let row = match (select.current == Some(index), disabled) {
                    (true, _) => row
                        .bg(renderer::Color::hex(0x1967d2ff))
                        .text_color(renderer::Color::WHITE),
                    (_, true) => row.text_color(muted),
                    _ => row,
                };
                let input = self.clone();
                row.child(renderer::text(label.clone(), None)).on_pointer(
                    move |event, window, cx| match event {
                        Event::PointerMove { .. } if !disabled => {
                            let mut select = input.select.borrow_mut();
                            if select
                                .as_mut()
                                .is_some_and(|select| select.current.replace(index) != Some(index))
                            {
                                drop(select);
                                input.repaint(Some(window));
                            }
                        }
                        Event::PointerUp { .. } if !disabled => {
                            cx.stop_event();
                            input.close_select(Some(index), window, cx);
                        }
                        _ => {}
                    },
                )
            });
        let input = self.clone();
        let list = renderer::node()
            .absolute()
            .left(px(x))
            .top(px(top))
            .min_w(px(width))
            .flex()
            .flex_col()
            .bg(back)
            .text_color(text)
            .border(px(1.))
            .border_color(line)
            .text_size(px(13.))
            .line_height(px(ROW))
            .whitespace_nowrap()
            .children(rows)
            .on_pointer(move |event, window, cx| {
                let Event::Wheel { delta, lines, .. } = *event else {
                    // A press in the list is not a press outside it.
                    if let Event::PointerDown { .. } = event {
                        cx.stop_event();
                    }
                    return;
                };
                let mut guard = input.select.borrow_mut();
                let Some(select) = guard.as_mut() else {
                    return;
                };
                select.wheel -= delta.y * if lines { ROW } else { 1. };
                let rows = (select.wheel / ROW).trunc();
                select.wheel -= rows * ROW;
                let last = select.items.len().saturating_sub(select.rows);
                let top = (select.top as isize + rows as isize).clamp(0, last as isize) as usize;
                if select.top != top {
                    select.top = top;
                    drop(guard);
                    input.repaint(Some(window));
                }
            });
        let input = self.clone();
        Some(
            renderer::node()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_pointer(move |event, window, cx| match event {
                    Event::PointerDown { .. } => {
                        cx.stop_event();
                        input.close_select(None, window, cx);
                    }
                    Event::Wheel { .. } => cx.stop_event(),
                    _ => {}
                })
                .child(list),
        )
    }

    fn lint(&self, warnings: Vec<String>) {
        let Ok(mut host) = self.host.try_borrow_mut() else {
            return;
        };
        for text in warnings {
            host.log(serde_json::json!({"method":"Runtime.consoleAPICalled","params":{"type":"error","text":text,"context":crate::jsc::Role::Renderer(self.id)}}));
        }
    }
}

// Do not use cx or window inside f: reflow accesses them through pointers.
pub(crate) fn scripted<R>(cx: &mut App, window: Option<&mut Window>, f: impl FnOnce() -> R) -> R {
    let previous = SCRIPTS.replace(Some((cx, window.map(|window| window as *mut Window))));
    let result = f();
    SCRIPTS.set(previous);
    result
}

pub(super) fn clipboard(text: Option<String>) -> Option<String> {
    let (cx, _) = SCRIPTS.get()?;
    // SAFETY: the pointer comes from `scripted`, whose caller does not touch it until the script returns.
    let cx = unsafe { &*cx };
    match text {
        Some(text) => {
            cx.write_clipboard(text);
            Some(String::new())
        }
        None => Some(cx.read_clipboard().unwrap_or_default()),
    }
}

pub(crate) fn edit(
    renderer: &JsContext,
    app: Option<(&mut App, Option<&mut Window>)>,
    command: &str,
    text: &str,
) {
    let run = |text: &str| {
        let [command, text] = [command, text].map(|value| serde_json::json!(value).to_string());
        renderer.call("__appsEdit", &[&command, &text])
    };
    let result = match app {
        Some((cx, window)) => {
            let pasted = matches!(command, "paste" | "pasteAndMatchStyle")
                .then(|| cx.read_clipboard().unwrap_or_default());
            let result = scripted(cx, window, || run(pasted.as_deref().unwrap_or(text)));
            if let Ok(copied) = &result
                && matches!(command, "copy" | "cut")
                && !copied.is_empty()
            {
                cx.write_clipboard(copied.clone());
            }
            result
        }
        None => run(text),
    };
    if let Err(error) = result {
        eprintln!("apps: {error}");
    }
}

pub(super) fn reflow(id: u64) {
    let Some((cx, window)) = SCRIPTS.get() else {
        return;
    };
    let Some(view) = VIEWS.with_borrow(|views| {
        let (page, input) = views.get(&id)?;
        Some(PageView {
            page: page.upgrade()?,
            input: input.upgrade()?,
        })
    }) else {
        return;
    };
    // Reflow after composite frames even if the page is clean: boxes moved without layout. User scrolls move the
    // mirror without one. The build logs its lint warnings to the host, so a busy host skips it too.
    if view.input.host.try_borrow_mut().is_err()
        || view.page.store.try_borrow_mut().map_or(true, |mut store| {
            let shifted = !store.pages.get(&id).is_some_and(|page| page.dirty)
                && store
                    .page_state_mut(id)
                    .is_some_and(|dom| dom.shift_scrolls());
            shifted
                || !store.pages.get(&id).is_some_and(|page| page.dirty)
                    && !store
                        .page_state(id)
                        .is_some_and(|dom| dom.geometry.mirror.moved() || dom.layout.cull.reskipped)
        })
    {
        return;
    }
    let run = |window: &mut Window, cx: &mut App| {
        view.page.reflow(window, cx, || events(view.clone()));
        window.refresh();
    };
    // SAFETY: the pointers come from `scripted`, whose caller does not touch them until the script returns.
    let cx = unsafe { &mut *cx };
    match window.map(|window| unsafe { &mut *window }) {
        Some(window) if window.id() == view.input.window => run(window, cx),
        _ => {
            cx.update_window(view.input.window, run);
        }
    }
}

// Run animation callbacks at most once per frame, or recursive requests accumulate.
pub(crate) fn queue_animation_frame(input: Rc<Input>, window: &mut Window) {
    if !input.queued.replace(true) {
        window.next_frame(move |window, cx| animation_frame(input, window, cx));
    }
}

fn animation_frame(input: Rc<Input>, window: &mut Window, cx: &mut App) {
    input.queued.set(false);
    if let Some(renderer) = input.try_renderer()
        && let Err(error) = scripted(cx, Some(window), || {
            renderer.eval("__appsAnimationFrame()", "apps:frame")
        })
    {
        eprintln!("apps: {error}");
    }
    // A frame whose callbacks changed nothing draws nothing.
    if input.store.try_borrow().map_or(true, |store| {
        store.pages.get(&input.id).is_some_and(|page| page.dirty)
            || store
                .graphics
                .get(&input.id)
                .is_some_and(|gpu| gpu.presented())
    }) {
        window.refresh();
    }
    if input
        .store
        .try_borrow_mut()
        .ok()
        .and_then(|mut store| {
            store
                .page_state_mut(input.id)
                .map(|dom| std::mem::take(&mut dom.frame_requested))
        })
        .unwrap_or(false)
    {
        queue_animation_frame(input.clone(), window);
    }
    let desktop = input.desktop.clone();
    cx.defer(move |cx| Desktop::drive(desktop, cx));
}

impl renderer::View for PageView {
    fn render(
        &mut self,
        tree: &renderer::Tree,
        window: &mut Window,
        cx: &mut App,
    ) -> renderer::Child {
        if !window.redrawing()
            && let Some(hud) = &*self.page.hud.borrow()
        {
            hud.started.set(std::time::Instant::now());
        }
        self.page
            .render_page(tree, window, cx, events(self.clone()))
    }

    fn frame(&mut self, window: &mut Window, cx: &mut App) {
        self.input.frame(window, cx);
    }

    // Resize observers run after the layout of the frame, and intersection observers after them.
    fn laid_out(&mut self, first: bool, window: &mut Window, cx: &mut App) -> bool {
        let input = &self.input;
        if !(input.store.try_borrow())
            .is_ok_and(|store| store.page_state(input.id).is_some_and(|dom| dom.observers))
        {
            return false;
        }
        let Some(renderer) = input.try_renderer() else {
            return false;
        };
        let script = if first {
            "__appsObserve(true)"
        } else {
            "__appsObserve(false)"
        };
        match scripted(cx, Some(window), || renderer.eval(script, "apps:observe")) {
            Ok(changed) => changed == "true",
            Err(error) => {
                eprintln!("apps: {error}");
                false
            }
        }
    }

    fn suppressed(&self) -> bool {
        crate::render::transition::Captured::suppressed(&self.page)
    }

    fn drawn(&mut self, snapshots: Vec<renderer::Snapshot>, window: &mut Window, cx: &mut App) {
        let Some(changed) = self.page.drawn(snapshots, window) else {
            return;
        };
        // The script can lay out the page, which needs the view that this call holds.
        let input = self.input.clone();
        cx.defer(move |cx| {
            cx.update_window(input.window, |window, cx| {
                let renderer = input.try_renderer();
                if let Some(renderer) = renderer
                    && let Err(error) = scripted(cx, Some(window), || {
                        renderer.eval(&format!("__appsPaint({changed:?})"), "apps:paint")
                    })
                {
                    eprintln!("apps: {error}");
                }
                let desktop = input.desktop.clone();
                cx.defer(move |cx| Desktop::drive(desktop, cx));
            });
        });
    }

    fn window_changed(&mut self, window: &mut Window, cx: &mut App) {
        let input = &self.input;
        input.origin.set(None);
        if let (Ok(mut host), Ok(mut store)) =
            (input.host.try_borrow_mut(), input.store.try_borrow_mut())
        {
            if let Some(state) = store.page_state_mut(input.id) {
                state.window = window.window_state();
            }
            if !host.window_changes.contains(&input.id) {
                host.window_changes.push(input.id);
            }
        }
        input.defer_drive(cx);
    }

    fn event(&mut self, event: &Event, window: &mut Window, cx: &mut App) {
        let (id, input) = (self.input.id, &*self.input);
        match event {
            Event::KeyDown {
                key: named,
                modifiers,
                repeat,
                ime,
            } => {
                let key = named.name.as_str();
                if input.select.borrow().is_some() {
                    input.select_key(key, window, cx);
                    cx.stop_event();
                    return;
                }
                let pick = (key == "escape" && self.page.inspect.get().pick).then_some(Tool::Pick);
                let tool = pick.or_else(|| {
                    tool(key, *modifiers).filter(|_| !input.store.borrow().app.packaged)
                });
                let open = self.page.inspect.get().open;
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
                    input.host.borrow_mut().command(
                        crate::host::link::Target::Window(id),
                        serde_json::json!({"op":"window","args":args}),
                    );
                    input.defer_drive(cx);
                    cx.stop_event();
                    return;
                }
                let allowed = Desktop::dispatch(
                    input.desktop.clone(),
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
                    let renderer = input.renderer();
                    if let Some(renderer) = renderer {
                        let command = match key {
                            "c" => "copy",
                            "x" => "cut",
                            _ => "paste",
                        };
                        edit(&renderer, Some((cx, Some(&mut *window))), command, "");
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
                Desktop::dispatch(
                    input.desktop.clone(),
                    id,
                    "keyup",
                    &[],
                    serde_json::json!({"nativeKey":key.name,"keyChar":key.text,"ctrlKey":modifiers.ctrl(),"metaKey":modifiers.command(),"altKey":modifiers.alt(),"shiftKey":modifiers.shift(),"ime":ime}),
                    Some(window),
                    cx,
                );
            }
            Event::ModifiersChanged { modifiers, .. } => {
                Desktop::dispatch(
                    input.desktop.clone(),
                    id,
                    "modifiers",
                    &[],
                    serde_json::json!({"ctrlKey":modifiers.ctrl(),"metaKey":modifiers.command(),"altKey":modifiers.alt(),"shiftKey":modifiers.shift()}),
                    Some(window),
                    cx,
                );
            }
            Event::FileEnter { paths, at } => {
                *input.dropped.borrow_mut() = paths.clone();
                input.drag_files("enter", *at, window, cx);
            }
            Event::FileOver { at } => input.drag_files("over", *at, window, cx),
            Event::FileLeave => input.drag_files("leave", Point::default(), window, cx),
            Event::FileDrop { at } => input.drop_files(*at, window, cx),
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
        let state = self.input.editing()?;
        let text: Vec<_> = state["text"].as_str()?.encode_utf16().collect();
        let end = range.end.min(text.len());
        let range = range.start.min(end)..end;
        *actual = Some(range.clone());
        Some(String::from_utf16_lossy(&text[range]))
    }

    fn selection(&mut self, _: &mut Window, _: &mut App) -> Option<Utf16Selection> {
        let state = self.input.editing()?;
        let start = state["start"].as_u64()? as usize;
        let end = state["end"].as_u64()? as usize;
        Some(Utf16Selection {
            range: start.min(end)..start.max(end),
            reversed: start > end,
        })
    }

    fn composition(&mut self, _: &mut Window, _: &mut App) -> Option<Range<usize>> {
        let state = self.input.editing()?;
        Some(state["marked"][0].as_u64()? as usize..state["marked"][1].as_u64()? as usize)
            .filter(|range| range.start <= range.end)
    }

    fn end_composition(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(state) = self.input.editing()
            && let (Some(start), Some(end), Some(text)) = (
                state["marked"][0].as_u64(),
                state["marked"][1].as_u64(),
                state["text"].as_str(),
            )
        {
            let text: Vec<_> = text.encode_utf16().collect();
            if start <= end && (end as usize) <= text.len() {
                let marked = String::from_utf16_lossy(&text[start as usize..end as usize]);
                self.input.input(
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
        self.input.input(text, range, false, None, window, cx);
    }

    fn compose(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.input.input(text, range, true, selected, window, cx);
    }

    fn rect_of(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Rect<Pixels>> {
        let state = self.input.editing()?;
        // The IME asks right after an edit, before the next frame lays out the new text, so reflow first.
        scripted(cx, Some(window), || reflow(self.input.id));
        let id = state["id"].as_u64()? as usize;
        let store = self.page.store.borrow();
        let dom = store.page_state(self.input.id)?;
        let password = paint::field::password(&store, self.input.id, id, &state);
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
            let nodes = &store.pages.get(&self.input.id)?.doc.nodes;
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
            let at = paint::text::text_position(layout, text, rows, start)?;
            let to = paint::text::text_position(layout, text, rows, end)
                .filter(|to| to.y == at.y && to.x >= at.x)
                .unwrap_or(at);
            let [dx, dy] = dom.geometry.scroll_shift(&dom.styles.walk, node);
            return Some(Rect::new(
                at + point(px(dx), px(dy)),
                size(to.x - at.x, layout.line_height()),
            ));
        }
        // A terminal composes text at its cursor.
        let caret = self.page.terminal_caret(dom, id);
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
        let state = self.input.editing()?;
        let id = state["id"].as_u64()? as usize;
        let store = self.page.store.borrow();
        let dom = store.page_state(self.input.id)?;
        if let Some((layout, text, span, rows, _)) = dom.layout.text_layouts.get(&id) {
            let [dx, dy] = dom.geometry.scroll_shift(&dom.styles.walk, id);
            let point = point - renderer::point(px(dx), px(dy));
            let index = paint::text::text_index(layout, text, rows, point, false)
                .unwrap_or_else(|index| index)
                .clamp(span.start, span.end);
            let index = text[span.start..index].encode_utf16().count();
            return Some(
                paint::field::password(&store, self.input.id, id, &state)
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

// Presses go to the page in the bubble phase, after drag areas and scrollbars could stop them.
fn events(data: PageView) -> renderer::Node {
    renderer::canvas(|_, _| (), move |_, _, window| {
        window.paint_volatile();
        data.page.wake_parked(window);
        if data.page.inspect.get().pick {
            let view = data.input.clone();
            // The picker of the developer tools takes the pointer from the page.
            window.on_capture(move |event, _, cx| {
                let (at, click) = match *event {
                    Event::PointerMove { at, .. } => (at, false),
                    Event::PointerDown { at, .. } => (at, true),
                    Event::PointerUp { .. } => return cx.stop_event(),
                    _ => return,
                };
                view.host.borrow_mut().command(
                    crate::host::link::Target::Window(view.id),
                    serde_json::json!({"op":"window","args":["inspect",view.id,f32::from(at.x),f32::from(at.y),click]}),
                );
                view.defer_drive(cx);
                cx.stop_event();
            });
        }
        let view = data.input.clone();
        window.on_capture(move |event, window, cx| {
            let (Event::PointerLeave { at: position } | Event::PointerMove { at: position, .. } | Event::PointerUp { at: position, .. }) = *event else {
                return;
            };
            if view.select.borrow().is_some() {
                return;
            }
            let (kind, button) = match *event {
                Event::PointerLeave { .. } => {
                    let [x, y] = view.screen(window, position);
                    Desktop::dispatch(view.desktop.clone(), view.id, "pointerexit", &[], serde_json::json!({"clientX":f32::from(position.x),"clientY":f32::from(position.y),"screenX":x,"screenY":y}), Some(window), cx);
                    return;
                }
                Event::PointerMove { held, .. } => {
                    if held.is_none() {
                        view.buttons.set(0);
                    }
                    ("pointermove", None)
                }
                Event::PointerUp { button, .. } => {
                    let button = button as u8;
                    view.buttons.set(view.buttons.get() & !BUTTONS[usize::from(button)]);
                    ("pointerup", Some(button))
                }
                _ => return,
            };
            let mut fields = view.pointer(window, position, event.modifiers().unwrap_or_default());
            if let Some(button) = button {
                fields["button"] = button.into();
            }
            Desktop::dispatch(view.desktop.clone(), view.id, kind, &[], fields, Some(window), cx);
        });
        let view = data.input.clone();
        window.on_bubble(move |event, window, cx| {
            if let Event::PointerDown { at, button, modifiers, clicks, .. } = *event {
                let button = button as u8;
                view.buttons.set(view.buttons.get() | BUTTONS[usize::from(button)]);
                let mut fields = view.pointer(window, at, modifiers);
                (fields["button"], fields["detail"]) = (button.into(), clicks.into());
                Desktop::dispatch(view.desktop.clone(), view.id, "pointerdown", &[], fields, Some(window), cx);
            }
        });
        let view = data.input.clone();
        window.on_capture(move |event, window, cx| {
            let Event::Wheel { at: position, delta, lines, phase, modifiers } = *event else {
                return;
            };
            if view.select.borrow().is_some() {
                return;
            }
            if let (renderer::WheelPhase::Began | renderer::WheelPhase::Ended, Ok(host)) =
                (phase, view.host.try_borrow())
                && let Some(link) = &host.link
            {
                let kind = if phase == renderer::WheelPhase::Began {
                    "gestureScrollBegin"
                } else {
                    "gestureScrollEnd"
                };
                link.event(
                    "input-event",
                    view.id,
                    Some(serde_json::json!({"type": kind})),
                    "apps:input",
                );
            }
            view.clone().queue_layout(window);
            let (mut target, mut wheel, mut terminal) = (None, false, None);
            if let Ok(store) = view.store.try_borrow() {
                let viewport = *store.viewports.get(&view.id).unwrap_or(&[800., 600.]);
                if let Some(page) = store.pages.get(&view.id) {
                    let state = &page.state;
                    let seen = state.layout.cull.seen.as_ref().map(|seen| seen.2.clone()).or_else(|| state.styles.computed.clone());
                    target = seen.as_ref().and_then(|computed| {
                        let at = [position.x, position.y].map(f32::from);
                        crate::render::hit::hit_test(&page.doc, state, computed, viewport, at, crate::render::hit::Hit::First).first().copied()
                    });
                    if let Some(target) = target {
                        let mut chain = Vec::new();
                        let mut node = Some(target);
                        while let Some(id) = node {
                            if terminal.is_none() && page.doc.nodes[id].tag == "terminal" {
                                terminal = Some((id, state.media.terminals.clone()));
                            }
                            chain.extend(state.geometry.scroll.get(&id).cloned());
                            node = seen.as_ref().and_then(|computed| state.tree.flat_parent(&page.doc, &computed.children, id));
                        }
                        window.scroll_only(chain);
                    }
                    wheel = state.wheel;
                }
            }
            if wheel {
                let delta = if lines { delta.map(|value| value * 16.) } else { delta };
                let mut fields = view.pointer(window, position, modifiers);
                (fields["deltaX"], fields["deltaY"]) = ((-delta.x).into(), (-delta.y).into());
                if !Desktop::dispatch(view.desktop.clone(), view.id, "wheel", target.as_slice(), fields, Some(window), cx) {
                    cx.stop_event();
                    return;
                }
            }
            // A terminal scrolls its own rows, or sends the wheel to its program.
            if let Some((node, terminals)) = terminal
                && let Some(terminal) = terminals.borrow_mut().shown(node)
            {
                let mods = u32::from(modifiers.shift()) | u32::from(modifiers.ctrl()) << 1 | u32::from(modifiers.alt()) << 2 | u32::from(modifiers.command()) << 3;
                let rows = delta.y * if lines { 16. } else { 1. };
                terminal.wheel(-rows, mods, position.x.into(), position.y.into());
                cx.stop_event();
                window.refresh();
            }
        });
    })
    .absolute()
    .size_full()
}

const ROW: f32 = 20.;
const ROWS: usize = 20;

// The list of a `<select>` where the system has no menu for it.
struct Select {
    id: u64,
    // The label of each option, and whether it is disabled. Separators are `None`.
    items: Vec<Option<(String, bool)>>,
    // The border box of the select.
    at: [f32; 4],
    dark: bool,
    current: Option<usize>,
    // The first row that shows, the rows that show, and the wheel distance that has not scrolled a row yet.
    top: usize,
    rows: usize,
    wheel: f32,
}

impl Select {
    fn enabled(&self, index: usize) -> bool {
        matches!(self.items.get(index), Some(Some((_, false))))
    }

    // Moves to the nearest enabled option from `target`, first toward `step`.
    fn go(&mut self, target: isize, step: isize) {
        let last = self.items.len() as isize - 1;
        let target = target.clamp(0, last.max(0));
        let found = [step, -step].into_iter().find_map(|step| {
            (0..=last)
                .map(|offset| target + offset * step)
                .take_while(|index| (0..=last).contains(index))
                .find(|index| self.enabled(*index as usize))
        });
        if let Some(index) = found {
            let index = index as usize;
            self.current = Some(index);
            self.top = self
                .top
                .min(index)
                .max((index + 1).saturating_sub(self.rows));
        }
    }
}

// macOS shows a system menu from main instead.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(crate) fn open_select(page: u64, popup: &serde_json::Value) {
    let Some(input) = VIEWS.with_borrow(|views| views.get(&page)?.1.upgrade()) else {
        return;
    };
    let items = popup["items"].as_array().into_iter().flatten();
    let items = items.map(|item| Some((item[0].as_str()?.to_owned(), item[1].as_u64()? != 0)));
    let mut select = Select {
        id: popup["id"].as_u64().unwrap_or(0),
        items: items.collect(),
        at: ["x", "y", "width", "height"].map(|name| popup[name].as_f64().unwrap_or(0.) as f32),
        dark: popup["dark"] == true,
        current: None,
        top: 0,
        rows: ROWS,
        wheel: 0.,
    };
    if let Some(index) = popup["selected"].as_u64() {
        select.go(index as isize, 1);
    }
    input.select.replace(Some(select));
    input.repaint(None);
}

impl Input {
    fn repaint(&self, window: Option<&mut Window>) {
        if let Ok(mut store) = self.store.try_borrow_mut()
            && let Some(page) = store.pages.get_mut(&self.id)
        {
            page.dirty = true;
        }
        if let Some(window) = window {
            window.refresh();
        }
    }

    fn close_select(&self, picked: Option<usize>, window: &mut Window, cx: &mut App) {
        let Some(select) = self.select.take() else {
            return;
        };
        self.repaint(None);
        let index = picked
            .filter(|index| select.enabled(*index))
            .map_or(-1, |index| index as i64);
        self.run(
            &format!("__appsSelectPicked({},{index})", select.id),
            window,
            cx,
        );
    }

    // The open list takes all keys.
    fn select_key(&self, key: &str, window: &mut Window, cx: &mut App) {
        let mut guard = self.select.borrow_mut();
        let Some(select) = guard.as_mut() else {
            return;
        };
        let current = select.current.map_or(-1, |index| index as isize);
        let page = select.rows as isize - 1;
        match key {
            "down" => select.go(current + 1, 1),
            "up" => select.go(current - 1, -1),
            "pagedown" => select.go(current + page, 1),
            "pageup" => select.go(current - page, -1),
            "home" => select.go(0, 1),
            "end" => select.go(isize::MAX / 2, -1),
            "enter" | "space" | "tab" | "escape" => {
                let picked = select.current.filter(|_| key != "escape");
                drop(guard);
                return self.close_select(picked, window, cx);
            }
            _ => return,
        }
        drop(guard);
        self.repaint(Some(window));
    }
}
