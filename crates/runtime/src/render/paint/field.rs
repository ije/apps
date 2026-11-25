use super::super::ViewState;
use renderer::{Pixels, Point, Window, px, size};
use std::rc::Rc;

pub(crate) type Caret = (Option<[usize; 2]>, usize);

// The rows of `aligned_rows` with the lines, width and alignment they are for.
type KeptRows = (
    Rc<[renderer::TextLine]>,
    u32,
    u8,
    crate::render::layout::Rows,
);

pub(crate) struct FieldText {
    pub(crate) id: usize,
    pub(crate) view: Rc<ViewState>,
    pub(crate) text: Rc<str>,
    pub(crate) password: Option<String>,
    pub(crate) run: renderer::TextRun,
    pub(crate) placeholder: bool,
    pub(crate) single_line: bool,
    /// A right-to-left field whose rows start at the right. The caret is on the left of its place.
    pub(crate) rtl: bool,
    /// The place of each row for `text-align`, for the lines, width and alignment of the last prepaint.
    pub(crate) rows: std::cell::RefCell<Option<KeptRows>>,
    pub(crate) caret: super::text::CaretLook,
    pub(crate) selection: (Option<u32>, u32),
    pub(crate) handle: renderer::ScrollRef,
    pub(crate) selected: std::cell::Cell<Option<[usize; 2]>>,
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

pub(crate) fn natural_width(natural: Pixels) -> renderer::Node {
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
        let mut store = view.store.borrow_mut();
        let dom = store.state(view.id);
        let selected = dom
            .interaction
            .selection
            .get(&self.id)
            .filter(|_| dom.interaction.focus == Some(self.id))
            .map(|&selected| match &self.password {
                _ if self.placeholder => [0; 2],
                Some(value) => selected.map(|offset| crate::dom::text::bullets(value, offset)),
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
        drop(store);
        let state = (selected, self.text.len());
        if view.fields.borrow_mut().insert(self.id, state) != Some(state) {
            let (shown, old) = (self.handle.bounds(), self.handle.offset());
            let mut offset = old;
            match selected {
                Some([_, end]) => {
                    let index = crate::dom::text::utf16_index(&self.text, end);
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
        let font_size = cx.text.font_size;
        let window = &mut *cx.window;
        let selected = self.selected.get();
        if selected.is_some() {
            window.paint_volatile();
        }
        let text = &*self.text;
        let line_height = layout.line_height();
        let selected = selected.map(|[a, b]| {
            [a.min(b), a.max(b), b].map(|index| crate::dom::text::utf16_index(text, index))
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
        let caret = |window: &mut Window| {
            if let Some([start, end, caret]) = selected
                && start == end
                && let Some(mut at) = super::text::text_position(&layout, text, &rows, caret)
            {
                let advance = (!self.placeholder)
                    .then(|| self.caret.advance(&layout, text, &rows, caret, at))
                    .flatten();
                // As in Chrome, a caret past the edge of the field shows on the last pixel inside.
                let shown = self.handle.bounds();
                if at.x <= shown.right() + px(1.) {
                    at.x = at.x.min(shown.right() - px(1.));
                }
                super::text::paint_caret(
                    &self.view,
                    [self.id, caret, text.len()],
                    at,
                    line_height,
                    advance,
                    (
                        strut,
                        font_size,
                        window.text_system().baseline(
                            strut,
                            font_size,
                            line_height,
                            window.scale_factor(),
                        ),
                    ),
                    self.caret,
                    window,
                );
            }
        };
        let behind = self.caret.shape == super::text::CaretShape::Block;
        if behind {
            caret(window);
        }
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
        if !behind {
            caret(window);
        }
    }
}

pub(crate) fn password<'a>(
    store: &super::super::Store,
    view: u64,
    id: usize,
    state: &'a serde_json::Value,
) -> Option<&'a str> {
    store
        .pages
        .get(&view)?
        .doc
        .nodes
        .get(id)?
        .password()
        .then(|| state["text"].as_str())?
}
