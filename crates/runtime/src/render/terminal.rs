//! Draws the grid of a `<terminal>` element.

use super::PageState;
use crate::style::{self, Prop};
use crate::web::terminal::{
    BOLD, Cell, FAINT, FOCUS, Frame, INVERSE, INVISIBLE, ITALIC, OVERLINE, Placement, SCROLL_ROW,
    SELECTED, SPACER, STRIKETHROUGH, Terminals, UNDERLINE,
};
use renderer::{
    Color, DecorationStyle, FontId, GlyphId, Hooks, NodeCx, Rect, TextAlign, TextRun, Timer,
    Window, point, px, size,
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

/// How the element looks, from its computed style.
#[derive(PartialEq)]
pub(crate) struct Look {
    font: renderer::Font,
    font_size: f32,
    /// A cell is `1ch` wide and `1lh` tall, so CSS can size the element in cells.
    cell: [f32; 2],
    /// The default colors for the terminal: foreground, background, cursor and the palette.
    colors: [u32; 19],
    /// The text and background colors of `::selection`.
    selection: (Option<Color>, Color),
    dark: bool,
}

// The palette properties, in the order of the 16 colors.
const PALETTE: [Prop; 16] = [
    Prop::TERMINAL_BLACK,
    Prop::TERMINAL_RED,
    Prop::TERMINAL_GREEN,
    Prop::TERMINAL_YELLOW,
    Prop::TERMINAL_BLUE,
    Prop::TERMINAL_MAGENTA,
    Prop::TERMINAL_CYAN,
    Prop::TERMINAL_WHITE,
    Prop::TERMINAL_BRIGHT_BLACK,
    Prop::TERMINAL_BRIGHT_RED,
    Prop::TERMINAL_BRIGHT_GREEN,
    Prop::TERMINAL_BRIGHT_YELLOW,
    Prop::TERMINAL_BRIGHT_BLUE,
    Prop::TERMINAL_BRIGHT_MAGENTA,
    Prop::TERMINAL_BRIGHT_CYAN,
    Prop::TERMINAL_BRIGHT_WHITE,
];

pub(super) fn look(
    style: &style::Style,
    selection: Option<&Rc<style::Style>>,
    units: &style::Lengths,
) -> Look {
    let pick = |prop: Prop| style.get(prop).and_then(style::color);
    let set = |(rgb, _): (u32, u8)| 1 << 24 | rgb;
    let foreground = pick(Prop::COLOR).unwrap_or((0, 255));
    let luminance = |rgb: u32| {
        let [_, r, g, b] = rgb.to_be_bytes().map(f32::from);
        (0.299 * r + 0.587 * g + 0.114 * b) / 255.
    };
    // Without a background color, the terminal is on the page. Light text means a dark page.
    let background = pick(Prop::BACKGROUND_COLOR)
        .filter(|color| color.1 > 0)
        .map_or(
            if luminance(foreground.0) > 0.5 {
                0
            } else {
                0xffffff
            },
            |color| color.0,
        );
    let mut colors = [0; 19];
    colors[0] = set(foreground);
    colors[1] = 1 << 24 | background;
    colors[2] = pick(Prop::CARET_COLOR).map_or(0, set);
    for (index, prop) in PALETTE.into_iter().enumerate() {
        colors[3 + index] = pick(prop).map_or(0, set);
    }
    let (ink, fill) = style::selection_colors(style, selection);
    Look {
        font: style::font(style),
        font_size: units.font.em,
        cell: [units.font.ch.max(1.), units.font.lh.max(1.)],
        colors,
        selection: (ink.map(Color::hex), Color::hex(fill)),
        dark: luminance(background) < 0.5,
    }
}

struct Ink {
    col: u16,
    /// From the left of the grid and from the baseline.
    x: f32,
    y: f32,
    font: FontId,
    glyph: GlyphId,
    size: renderer::Pixels,
    color: Color,
    emoji: bool,
}

/// What a row draws. Columns are `start..end`.
#[derive(Default)]
struct Row {
    fills: Vec<(u16, u16, Color)>,
    /// 0 is a strikethrough, 6 is an overline, and the others are the underline styles of the terminal.
    lines: Vec<(u16, u16, u8, Color)>,
    inks: Vec<Ink>,
}

/// The drawing of a terminal, kept from frame to frame. Only rows that change are built again.
#[derive(Default)]
pub(crate) struct Drawn {
    look: Option<Rc<Look>>,
    scale: f32,
    rows: Vec<Row>,
    /// Kind, column, row, and whether it blinks.
    cursor: (u8, u16, u16, bool),
    cursor_wide: bool,
    /// Foreground, background and cursor colors, which the program can change.
    colors: [u32; 3],
    blink: Option<([u16; 2], Instant, u32, Timer)>,
    /// The images of the screen in the order to draw them, and their pixels by generation.
    placements: Vec<Placement>,
    images: Vec<(u64, Arc<renderer::RenderImage>)>,
    /// Draws the next picture of an animated image.
    animation: Option<Timer>,
    /// In rows: all rows with the scrollback, the first row of the screen, and the rows of the screen.
    scroll: [u32; 3],
    /// The cursor in the page, for the window of an input method.
    pub caret: [f32; 4],
}

fn rgb(color: u32) -> Color {
    Color::hex(color << 8 | 0xff)
}

fn row(
    cells: &[Cell],
    extras: &mut &[u32],
    look: &Look,
    colors: [u32; 3],
    cx: &mut NodeCx<'_>,
) -> Row {
    let mut row = Row::default();
    let text = cx.window.text_system().clone();
    let [fore, back] = [colors[0], colors[1]].map(rgb);
    // A word: its text, the column of each character, its font variant and its end.
    let mut word = String::new();
    let mut starts: Vec<(u32, u16, Color)> = Vec::new();
    let mut variant = 0;
    let flush =
        |word: &mut String, starts: &mut Vec<(u32, u16, Color)>, variant: u16, row: &mut Row| {
            if word.is_empty() {
                return;
            }
            let mut font = look.font.clone();
            if variant & BOLD != 0 {
                font.weight = 700.;
            }
            if variant & ITALIC != 0 {
                font.style = renderer::FontStyle::Italic;
            }
            let run = TextRun {
                len: word.len(),
                font,
                ..Default::default()
            };
            let line = text.shape_line(word, px(look.font_size), &run);
            // The grid places each character, so only the offsets inside a cell come from the font.
            let mut cell = (usize::MAX, 0.);
            for glyph in &line.line.glyphs {
                let at = starts
                    .partition_point(|start| start.0 <= glyph.index)
                    .saturating_sub(1);
                if cell.0 != at {
                    cell = (at, f32::from(glyph.x));
                }
                let (_, col, color) = starts[at];
                row.inks.push(Ink {
                    col,
                    x: col as f32 * look.cell[0] + f32::from(glyph.x) - cell.1,
                    y: f32::from(glyph.y),
                    font: glyph.font,
                    glyph: glyph.id,
                    size: glyph.size,
                    color,
                    emoji: glyph.emoji,
                });
            }
            word.clear();
            starts.clear();
        };
    let span = |list: &mut Vec<(u16, u16, Color)>, col: u16, color: Color| match list.last_mut() {
        Some(last) if last.1 == col && last.2 == color => last.1 += 1,
        _ => list.push((col, col + 1, color)),
    };
    let line = |list: &mut Vec<(u16, u16, u8, Color)>, col: u16, kind: u8, color: Color| match list
        .iter_mut()
        .rev()
        .find(|line| line.2 == kind)
    {
        Some(last) if last.1 == col && last.3 == color => last.1 += 1,
        _ => list.push((col, col + 1, kind, color)),
    };
    let mut selected = Vec::new();
    for (col, cell) in cells.iter().enumerate() {
        let col = col as u16;
        let (mut ink, mut fill) = (
            if cell.fg == 0 { fore } else { rgb(cell.fg) },
            (cell.bg != 0).then(|| rgb(cell.bg)),
        );
        if cell.flags & INVERSE != 0 {
            (ink, fill) = (fill.unwrap_or(back), Some(ink));
        }
        if cell.flags & FAINT != 0 {
            ink = ink.fade(0.5);
        }
        if let Some(fill) = fill {
            span(&mut row.fills, col, fill);
        }
        if cell.flags & SELECTED != 0 {
            span(&mut selected, col, look.selection.1);
            ink = look.selection.0.unwrap_or(ink);
        }
        let more = &extras[..(cell.extra as usize).min(extras.len())];
        *extras = &extras[more.len()..];
        let drawn = cell.flags & (SPACER | INVISIBLE) == 0;
        if drawn {
            let stroke = if cell.underline == 0 {
                ink
            } else {
                rgb(cell.underline)
            };
            let kind = ((cell.flags & UNDERLINE) >> 8) as u8;
            if kind != 0 {
                line(&mut row.lines, col, kind, stroke);
            }
            if cell.flags & STRIKETHROUGH != 0 {
                line(&mut row.lines, col, 0, ink);
            }
            if cell.flags & OVERLINE != 0 {
                line(&mut row.lines, col, 6, ink);
            }
        }
        let letter = char::from_u32(cell.ch).filter(|ch| drawn && *ch != '\0' && *ch != ' ');
        if letter.is_none() || cell.flags & (BOLD | ITALIC) != variant {
            flush(&mut word, &mut starts, variant, &mut row);
        }
        variant = cell.flags & (BOLD | ITALIC);
        if let Some(letter) = letter {
            starts.push((word.len() as u32, col, ink));
            word.push(letter);
            word.extend(more.iter().filter_map(|point| char::from_u32(*point)));
        }
    }
    flush(&mut word, &mut starts, variant, &mut row);
    // The selection is above the backgrounds of the cells.
    row.fills.extend(selected);
    row
}

pub(super) struct Grid {
    pub(super) id: usize,
    pub(super) view: Rc<PageState>,
    pub(super) terminals: Rc<RefCell<Terminals>>,
    pub(super) look: Rc<Look>,
    /// The padding and the border of the element, from the top clockwise. The bar is at the edge of the padding.
    pub(super) edges: [f32; 4],
}

impl Look {
    pub(super) fn background(&self) -> u32 {
        self.colors[1] & 0xffffff
    }
}

impl Grid {
    fn draw(&self, cx: &mut NodeCx<'_>) {
        let bounds = cx.bounds;
        let look = &*self.look;
        let [cell_w, cell_h] = look.cell;
        let [width, height] = [bounds.size.width, bounds.size.height].map(f32::from);
        if width < cell_w || height < cell_h {
            return;
        }
        let mut terminals = self.terminals.borrow_mut();
        // Without a session, the element is an empty box.
        let Some((terminal, bar)) = terminals.drawn(self.id) else {
            return;
        };
        // The knob of the bar was dragged, or its track was pressed.
        let asked = bar.0.target().y;
        if asked != bar.1 {
            terminal.control(SCROLL_ROW, f64::from((-f32::from(asked) / cell_h).round()));
        }
        let scrolled = terminal.drawn.scroll;
        // The focus is read here, because it doesn't build the element again.
        let focused = self.view.host.try_borrow().is_ok_and(|host| {
            host.pages
                .get(&self.view.id)
                .and_then(|page| page.doc.nodes.get(self.id))
                .is_some_and(|node| node.state & crate::dom::FOCUS != 0)
        });
        let scale = cx.window.scale_factor();
        if terminal
            .drawn
            .look
            .as_ref()
            .is_none_or(|old| !Rc::ptr_eq(old, &self.look) && **old != *look)
        {
            terminal.theme(&look.colors, look.dark);
        }
        terminal.drawn.look = Some(self.look.clone());
        // Layout rounds the box to device pixels, so a box of `80ch` can be a little narrower than 80 cells.
        let grid =
            [width / cell_w, height / cell_h].map(|count| (count + 0.1).clamp(1., 2000.) as u16);
        if grid != terminal.size || look.cell != terminal.cell || scale != terminal.drawn.scale {
            terminal.drawn.scale = scale;
            terminal.resize(grid, look.cell, scale);
            // The page reads the new size in its next task.
            crate::host::Waker::current().wake();
        }
        let focused = focused && cx.window.is_active();
        if focused != terminal.focused {
            terminal.focused = focused;
            terminal.control(FOCUS, f64::from(u8::from(focused)));
        }
        let mut frame: Frame = unsafe { std::mem::zeroed() };
        if terminal.frame(&mut frame) {
            terminal.drawn.scroll = frame.scroll;
            // SAFETY: the frame holds `placed` placements until the next frame.
            let placements = match frame.placed {
                0 => &[][..],
                count => unsafe { std::slice::from_raw_parts(frame.placements, count as usize) },
            };
            let mut images = std::mem::take(&mut terminal.drawn.images);
            for (_, old) in images.extract_if(.., |image| {
                !placements
                    .iter()
                    .any(|placement| placement.image == image.0)
            }) {
                cx.window.release_image(old);
            }
            for placement in placements {
                if !images.iter().any(|image| image.0 == placement.image)
                    && let Some(image) = terminal.image(placement)
                {
                    images.push((placement.image, Arc::new(image)));
                }
            }
            let drawn = &mut terminal.drawn;
            drawn.images = images;
            drawn.placements.clear();
            drawn.placements.extend_from_slice(placements);
            drawn.animation = (frame.wait > 0).then(|| {
                cx.window
                    .timer(Duration::from_millis(frame.wait.into()), |window, _| {
                        window.refresh()
                    })
            });
            drawn.cursor = (
                frame.cursor,
                frame.cursor_x,
                frame.cursor_y,
                frame.cursor_blinks != 0,
            );
            drawn.cursor_wide = frame.cursor_wide != 0;
            drawn.colors = [frame.foreground, frame.background, frame.cursor_color];
            drawn.rows.resize_with(frame.rows as usize, Row::default);
            if frame.count > 0 {
                let cols = frame.cols as usize;
                // SAFETY: the frame holds `count` rows of `cols` cells until the next frame.
                let (ys, cells) = unsafe {
                    (
                        std::slice::from_raw_parts(frame.ys, frame.count as usize),
                        std::slice::from_raw_parts(frame.cells, frame.count as usize * cols),
                    )
                };
                let count: usize = cells.iter().map(|cell| cell.extra as usize).sum();
                let mut extras = if count == 0 {
                    &[][..]
                } else {
                    unsafe { std::slice::from_raw_parts(frame.extras, count) }
                };
                for (index, y) in ys.iter().enumerate() {
                    if let Some(target) = drawn.rows.get_mut(*y as usize) {
                        *target = row(
                            &cells[index * cols..(index + 1) * cols],
                            &mut extras,
                            look,
                            drawn.colors,
                            cx,
                        );
                    }
                }
            }
        }
        let window = &mut *cx.window;
        let origin = bounds.origin;
        terminal.origin = [origin.x, origin.y].map(f32::from);
        let [all, top, shown] = terminal.drawn.scroll;
        let [top_edge, right_edge, bottom_edge, left_edge] = self.edges.map(px);
        bar.0.set_range(
            Rect::between(
                point(bounds.left() - left_edge, bounds.top() - top_edge),
                point(bounds.right() + right_edge, bounds.bottom() + bottom_edge),
            ),
            point(px(0.), px(all.saturating_sub(shown) as f32 * cell_h)),
        );
        bar.1 = px(-(top as f32) * cell_h);
        bar.0.set_offset(point(px(0.), bar.1));
        if scrolled != terminal.drawn.scroll {
            let bars = window
                .keyed::<Rc<RefCell<super::scrollbar::State>>>(("scrollbar", self.id).into())
                .clone();
            let mut bars = bars.borrow_mut();
            // Output scrolls the screen without the bar. The bar shows when the user scrolls.
            let pinned = |[all, top, shown]: [u32; 3]| top + shown >= all;
            if scrolled[1] != top && !(pinned(scrolled) && pinned(terminal.drawn.scroll)) {
                bars.show();
            }
            // The bar took its place before this frame had the new rows.
            if bars.visible() {
                window.refresh();
            }
        }
        // The program sees cells of whole pixels, and places its images in these pixels.
        let unit = [0, 1].map(|axis| look.cell[axis] / terminal.cell_pixels[axis] as f32);
        let grid = Rect::new(
            origin,
            size(
                px(terminal.size[0] as f32 * cell_w),
                px(terminal.size[1] as f32 * cell_h),
            ),
        );
        let drawn = &mut terminal.drawn;
        let [fore, back] = [drawn.colors[0], drawn.colors[1]].map(rgb);
        let face = window.text_system().face(&look.font);
        let baseline = window
            .text_system()
            .baseline(face, px(look.font_size), px(cell_h), scale);
        let stroke = px((look.font_size / 14.).max(1. / scale));
        let cell = |col: u16, row: u16, cols: u16| {
            Rect::new(
                point(
                    origin.x + px(col as f32 * cell_w),
                    origin.y + px(row as f32 * cell_h),
                ),
                size(px(cols as f32 * cell_w), px(cell_h)),
            )
        };
        let images = |window: &mut Window, layer: u32| {
            for placement in drawn.placements.iter().filter(|image| image.layer == layer) {
                let image = drawn.images.iter().find(|image| image.0 == placement.image);
                let [from_x, from_y, from_wide, from_tall] =
                    placement.source.map(|side| side as f32);
                let Some((_, image)) = image.filter(|_| from_wide > 0. && from_tall > 0.) else {
                    continue;
                };
                let [x, y] = [
                    placement.col as f32 * cell_w + placement.x as f32 * unit[0],
                    placement.row as f32 * cell_h + placement.y as f32 * unit[1],
                ];
                let [wide, tall] = [
                    placement.width as f32 * unit[0],
                    placement.height as f32 * unit[1],
                ];
                let shown = Rect::new(
                    point(origin.x + px(x), origin.y + px(y)),
                    size(px(wide), px(tall)),
                );
                // The rectangle shows a part of the image, so the whole image has its place around it.
                let (scale, full) = ([wide / from_wide, tall / from_tall], image.size());
                let whole = Rect::new(
                    point(
                        origin.x + px(x - from_x * scale[0]),
                        origin.y + px(y - from_y * scale[1]),
                    ),
                    size(
                        px(full.width as f32 * scale[0]),
                        px(full.height as f32 * scale[1]),
                    ),
                );
                window.draw_image(
                    shown.intersect(grid),
                    whole,
                    px(0.),
                    image.clone(),
                    false,
                    false,
                );
            }
        };
        // The program changed the background color (OSC 11).
        if drawn.colors[1] != look.colors[1] {
            window.draw_rect(bounds, back);
        }
        images(window, 0);
        for (y, row) in drawn.rows.iter().enumerate() {
            for &(start, end, color) in &row.fills {
                window.draw_rect(cell(start, y as u16, end - start), color);
            }
        }
        images(window, 1);
        for (y, row) in drawn.rows.iter().enumerate() {
            let top = origin.y + px(y as f32 * cell_h);
            for &(start, end, kind, color) in &row.lines {
                let (offset, style) = match kind {
                    0 => (baseline - px(look.font_size * 0.28), DecorationStyle::Solid),
                    2 => (baseline + stroke, DecorationStyle::Double),
                    3 => (baseline + stroke, DecorationStyle::Wavy),
                    4 => (baseline + stroke, DecorationStyle::Dotted),
                    5 => (baseline + stroke, DecorationStyle::Dashed),
                    6 => (px(0.), DecorationStyle::Solid),
                    _ => (baseline + stroke, DecorationStyle::Solid),
                };
                window.draw_line(
                    point(origin.x + px(start as f32 * cell_w), top + offset),
                    px((end - start) as f32 * cell_w),
                    stroke,
                    color,
                    style,
                );
            }
            for ink in &row.inks {
                window.draw_glyph(
                    point(origin.x + px(ink.x), top + baseline + px(ink.y)),
                    ink.font,
                    ink.glyph,
                    ink.size,
                    ink.color,
                    ink.emoji,
                );
            }
        }
        images(window, 2);
        let (kind, x, y, blinks) = drawn.cursor;
        let at = cell(x, y, 1 + u16::from(drawn.cursor_wide));
        drawn.caret = [at.origin.x, at.origin.y, at.size.width, at.size.height].map(f32::from);
        let shown = kind != 0
            && (!blinks || !focused || {
                let now = Instant::now();
                const BLINK: Duration = renderer::CARET_BLINK;
                if drawn.blink.as_ref().is_none_or(|blink| blink.0 != [x, y]) {
                    drawn.blink = None;
                }
                let since = drawn.blink.as_ref().map_or(now, |blink| blink.1);
                // A timer can fire just before its time, so the phase rounds up within 8 ms.
                let phase = ((now - since + Duration::from_millis(8)).as_millis()
                    / BLINK.as_millis()) as u32;
                if drawn.blink.as_ref().is_none_or(|blink| blink.2 != phase) {
                    let timer = window.timer(since + BLINK * (phase + 1) - now, |window, _| {
                        window.refresh()
                    });
                    drawn.blink = Some(([x, y], since, phase, timer));
                }
                phase.is_multiple_of(2)
            });
        if !blinks || !focused {
            drawn.blink = None;
        }
        if shown {
            let color = match drawn.colors[2] {
                0 => fore,
                color => rgb(color),
            };
            let thin = px((cell_w * 0.15).max(1.));
            let bar = |x: renderer::Pixels,
                       y: renderer::Pixels,
                       width: renderer::Pixels,
                       height: renderer::Pixels| {
                Rect::new(point(at.origin.x + x, at.origin.y + y), size(width, height))
            };
            let (wide, tall) = (at.size.width, at.size.height);
            match kind {
                // A terminal without the focus has a hollow cursor.
                _ if !focused || kind == 4 => {
                    let edge = px(1.);
                    for side in [
                        bar(px(0.), px(0.), wide, edge),
                        bar(px(0.), tall - edge, wide, edge),
                        bar(px(0.), px(0.), edge, tall),
                        bar(wide - edge, px(0.), edge, tall),
                    ] {
                        window.draw_rect(side, color);
                    }
                }
                1 => window.draw_rect(bar(px(0.), px(0.), thin, tall), color),
                3 => window.draw_rect(bar(px(0.), tall - thin, wide, thin), color),
                _ => {
                    window.draw_rect(at, color);
                    let top = origin.y + px(y as f32 * cell_h);
                    for ink in drawn
                        .rows
                        .get(y as usize)
                        .into_iter()
                        .flat_map(|row| row.inks.iter().filter(|ink| ink.col == x))
                    {
                        window.draw_glyph(
                            point(origin.x + px(ink.x), top + baseline + px(ink.y)),
                            ink.font,
                            ink.glyph,
                            ink.size,
                            back,
                            ink.emoji,
                        );
                    }
                }
            }
        }
        // The text of an input method is on top of the grid, at the cursor.
        if !terminal.preedit.is_empty() {
            let run = TextRun {
                len: terminal.preedit.len(),
                font: look.font.clone(),
                color: fore,
                ..Default::default()
            };
            let line = window
                .text_system()
                .shape_line(&terminal.preedit, px(look.font_size), &run);
            let wide = line.line.width;
            let start = at.origin.x.min(origin.x + px(width) - wide).max(origin.x);
            window.draw_rect(
                Rect::new(point(start, at.origin.y), size(wide, px(cell_h))),
                back,
            );
            line.paint(
                point(start, at.origin.y),
                px(cell_h),
                TextAlign::Left,
                None,
                window,
            );
            window.draw_line(
                point(start, at.origin.y + baseline + stroke),
                wide,
                stroke,
                fore,
                DecorationStyle::Solid,
            );
        }
    }
}

impl Hooks for Grid {
    fn volatile(&self) -> bool {
        false
    }

    fn bounded(&self) -> bool {
        true
    }

    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        self.draw(cx);
        inner(cx.window);
    }
}
