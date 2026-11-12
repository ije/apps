use super::PageState;
use renderer::{Border, Color, Edges, Rect, TextRun, Window, point, px, size};

// What the developer tools show on the page: the picker, and the boxes of two nodes.
#[derive(Clone, Copy, Default)]
pub(crate) struct Inspect {
    pub(crate) open: bool,
    pub(crate) pick: bool,
    pub(crate) hover: Option<usize>,
    pub(crate) selected: Option<usize>,
}

const BLUE: u32 = 0x0d99ffff;

pub(super) fn paint(view: &PageState, window: &mut Window) {
    let inspect = view.inspect.get();
    let Ok(host) = view.host.try_borrow() else {
        return;
    };
    let (Some(page), Some(state)) = (host.pages.get(&view.id), host.dom.get(&view.id)) else {
        return;
    };
    let nodes = [
        (
            inspect.hover.filter(|_| inspect.hover != inspect.selected),
            1.,
            0x0d99ff14,
        ),
        (inspect.selected, 2., 0),
    ];
    for (node, width, fill) in nodes {
        let Some(node) = node else {
            continue;
        };
        let (Some(&[x, y, w, h]), Some(element)) =
            (state.bounds.get(&node), page.doc.nodes.get(node))
        else {
            continue;
        };
        let bounds = Rect::new(point(px(x), px(y)), size(px(w), px(h)));
        let border = Border {
            widths: Edges::all(px(width)),
            color: Color::hex(BLUE),
            ..Border::default()
        };
        window.draw_box(bounds, px(0.), Color::hex(fill), border);
        let mut text = element.tag.to_string();
        if let Some(id) = element.attrs.get("id").filter(|id| !id.is_empty()) {
            text.push('#');
            text.push_str(id);
        }
        text.push_str(&format!("  {} × {}", w.round(), h.round()));
        let run = TextRun {
            len: text.len(),
            font: renderer::Font::new(if cfg!(target_os = "macos") {
                ".AppleSystemUIFont"
            } else if cfg!(windows) {
                "Segoe UI"
            } else {
                "DejaVu Sans"
            }),
            color: Color::hex(0xffffffff),
            background_color: None,
            underline: None,
            strikethrough: None,
            letter_spacing: px(0.),
            word_spacing: px(0.),
            font_size: None,
            inset: Default::default(),
            nowrap: false,
        };
        let line = window.text_system().shape_line(&text, px(11.), &run);
        let label = size(line.line.width + px(10.), px(18.));
        let limit = window.viewport_size();
        let top = if y >= 18. { y - 18. } else { y.max(0.) };
        let left = x.clamp(
            0.,
            (f32::from(limit.width) - f32::from(label.width)).max(0.),
        );
        let origin = point(px(left), px(top));
        window.draw_box(
            Rect::new(origin, label),
            px(3.),
            Color::hex(BLUE),
            Border::default(),
        );
        line.paint(
            point(origin.x + px(5.), origin.y),
            label.height,
            renderer::TextAlign::Left,
            None,
            window,
        );
    }
}
