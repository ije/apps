use super::WebView;
use crate::{desktop::Desktop, style};
use anyhow::{Context as _, Result};
use gpui::{
    Bounds, Context, EntityInputHandler, Pixels, Point, UTF16Selection, Window, point, px, size,
};
use std::ops::Range;

impl WebView {
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

    pub(super) fn input(
        &mut self,
        text: &str,
        range: Option<Range<usize>>,
        marked: bool,
        selected: Option<Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        let start = range.as_ref().map(|r| r.start);
        let end = range.as_ref().map(|r| r.end);
        let selected = selected.map(|r| [r.start, r.end]);
        if let Err(error) = self.evaluate(&format!(
            "__appsInput({},{},{},{},{})",
            serde_json::json!(text),
            serde_json::json!(start),
            serde_json::json!(end),
            marked,
            serde_json::json!(selected)
        )) {
            eprintln!("apps: {error}");
        }
        let runtime = self.runtime.clone();
        cx.defer(move |cx| Desktop::drive(runtime, cx));
        cx.notify();
    }
}

impl EntityInputHandler for WebView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let state = self.editing()?;
        let text: Vec<_> = state["text"].as_str()?.encode_utf16().collect();
        let end = range.end.min(text.len());
        let range = range.start.min(end)..end;
        *actual = Some(range.clone());
        Some(String::from_utf16_lossy(&text[range]))
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let state = self.editing()?;
        let start = state["start"].as_u64()? as usize;
        let end = state["end"].as_u64()? as usize;
        Some(UTF16Selection {
            range: start.min(end)..start.max(end),
            reversed: start > end,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let state = self.editing()?;
        Some(state["marked"][0].as_u64()? as usize..state["marked"][1].as_u64()? as usize)
            .filter(|range| range.start <= range.end)
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(state) = self.editing()
            && let (Some(start), Some(end), Some(text)) = (
                state["marked"][0].as_u64(),
                state["marked"][1].as_u64(),
                state["text"].as_str(),
            )
        {
            let text: Vec<_> = text.encode_utf16().collect();
            if start <= end && (end as usize) <= text.len() {
                self.input(
                    &String::from_utf16_lossy(&text[start as usize..end as usize]),
                    Some(start as usize..end as usize),
                    false,
                    None,
                    cx,
                );
            }
        }
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input(text, range, false, None, cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input(text, range, true, selected, cx);
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
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
        _cx: &mut Context<Self>,
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
            style.get("font-size").map(String::as_str).unwrap_or("16px"),
            16.,
            16.,
            bounds[2],
            bounds[3],
        );
        let line_height = style::length(
            style
                .get("line-height")
                .map(String::as_str)
                .unwrap_or("20px"),
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
        let run = gpui::TextRun {
            len: selected.len(),
            font: face,
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
            letter_spacing: gpui::px(0.),
            word_spacing: gpui::px(0.),
            font_size: None,
        };
        let line = window.text_system().shape_line(
            selected.to_owned().into(),
            px(font_size),
            &[run],
            None,
        );
        let index = line.closest_index_for_x(point.x - px(bounds[0]));
        Some((offset + selected[..index].encode_utf16().count()).min(text.encode_utf16().count()))
    }
}
