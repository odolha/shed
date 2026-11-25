//! Markdown drawn: pieces of selectable text (`ai_text`) as GPUI elements,
//! for the AI tab's answers and a Markdown file's View. A view that draws
//! them is a `PieceHost` (it keeps the selection, opens links, makes the
//! copy buttons); what it draws from is a `PieceSource`.

use std::ops::Range;
use std::rc::Rc;

use gpui::{
    AnyElement, ClipboardItem, Context, CursorStyle, FontWeight, HighlightStyle, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, SharedString, StyledText, TextLayout, Window, div, prelude::*, px, relative,
};
use shed_markdown::Align;
use shed_model::Rgb;

use crate::ai_text::{self, Kind, Piece, Point, Selection};
use crate::theme::{UiColors, hsla, hsla_alpha};

/// Byte ranges of a code block and their colours.
pub type CodeColors = Vec<(Range<usize>, Rgb)>;

/// The mouse over drawn text: the selection, and the link it is over.
#[derive(Clone, Default, PartialEq)]
pub struct TextSel {
    pub selection: Option<Selection>,
    /// The mouse is selecting (pressed in text).
    pub selecting: bool,
    /// It moved since (or picked a word or line): not a click.
    pub dragged: bool,
    /// The link the mouse is over: entry, piece, its range.
    pub hover_link: Option<(usize, usize, Range<usize>)>,
}

/// What pieces are drawn from.
pub trait PieceSource: 'static {
    fn pieces(&self) -> &[Piece];
    /// Piece `j`'s colours, when it is coloured code.
    fn code_colors(&self, j: usize) -> Option<&CodeColors>;
}

/// A view that draws pieces.
pub trait PieceHost: 'static + Sized {
    fn text_sel(&self) -> &TextSel;
    fn text_sel_mut(&mut self) -> &mut TextSel;
    fn accent(&self) -> Rgb;
    /// How much larger than usual text is drawn.
    fn zoom(&self) -> f32;
    /// A click on a link.
    fn open_link(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>);
    /// A small "Copy" button that copies `item`.
    fn copy_button(&self, key: SharedString, item: ClipboardItem, ui: &UiColors, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div>;
    /// A code block drawn as an error (a failed tool's output).
    fn code_is_error(&self, _entry: usize, _lang: &str) -> bool {
        false
    }
    /// The selection or the link under the mouse changed (from `before`):
    /// what shows them is drawn again.
    fn text_sel_changed(&mut self, _before: &TextSel, cx: &mut Context<Self>) {
        cx.notify();
    }
    /// A heading's text size.
    fn heading_size(&self, level: u8) -> f32 {
        match level {
            1 => 18.0,
            2 => 16.0,
            3 => 14.5,
            _ => 13.5,
        }
    }
    /// A heading with a line under it (a document's first two levels).
    fn heading_rule(&self, _level: u8) -> bool {
        false
    }
    /// A picture, diagram or math, drawn (`None`: shown as its source).
    fn render_visual(&mut self, _entry: usize, _j: usize, _ui: &UiColors, _cx: &mut Context<Self>) -> Option<AnyElement> {
        None
    }
}

/// Code coloured as its language, ranges into the whole text.
pub fn code_colors(lang: &str, code: &str, palette: &shed_emu::Palette) -> Option<CodeColors> {
    if code.len() > 200_000 {
        return None;
    }
    let spans = shed_project::highlight(shed_project::by_name(lang)?, code)?;
    let starts = std::iter::once(0).chain(code.match_indices('\n').map(|(i, _)| i + 1));
    Some(
        spans
            .into_iter()
            .zip(starts)
            .flat_map(|(line, start)| line.into_iter().map(move |(r, style)| (start + r.start..start + r.end, crate::project_view::syntax_color(style, palette))))
            .collect(),
    )
}

/// The byte of `text` under the mouse, on a character's edge; `None` when
/// the text is not laid out (GPUI's text layout panics then, so it is asked
/// carefully: a selection is not worth a crash).
pub fn text_index(layout: &TextLayout, text: &str, at: gpui::Point<gpui::Pixels>) -> Option<usize> {
    let found = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match layout.index_for_position(at) {
        Ok(i) | Err(i) => i,
    }))
    .ok()?;
    Some(text.floor_char_boundary(found.min(text.len())))
}

/// Whether the mouse is over laid-out text (same care as `text_index`).
pub fn over_text(layout: &TextLayout, at: gpui::Point<gpui::Pixels>) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| layout.bounds().contains(&at))).unwrap_or(false)
}

/// A piece of text that can be selected with the mouse.
pub fn sel_text<V: PieceHost, P: PieceSource>(host: &V, i: usize, j: usize, parsed: &Rc<P>, styles: Vec<(Range<usize>, HighlightStyle)>, cx: &mut Context<V>) -> gpui::Div {
    let text = &parsed.pieces()[j].text.text;
    let sel = host.text_sel();
    let accent = host.accent();
    // On characters' edges, whatever the selection says.
    let selected = sel
        .selection
        .and_then(|s| s.range_in(i, j, text.len()))
        .map(|r| text.floor_char_boundary(r.start)..text.floor_char_boundary(r.end))
        .filter(|r| r.start < r.end);
    let sel_style = HighlightStyle { background_color: Some(hsla_alpha(accent, 0.35)), ..Default::default() };
    // The link under the mouse: a light ground, and the hand.
    let hovered = sel.hover_link.as_ref().filter(|(e, p, _)| (*e, *p) == (i, j)).map(|(_, _, r)| r.clone());
    let hover_style = HighlightStyle { background_color: Some(hsla_alpha(accent, 0.16)), ..Default::default() };
    let styles = ai_text::with_selection(styles, hovered.clone(), hover_style);
    let styled = StyledText::new(SharedString::from(text.clone())).with_highlights(ai_text::with_selection(styles, selected, sel_style));
    let (down, moved, up) = (styled.layout().clone(), styled.layout().clone(), styled.layout().clone());
    let (p1, p2, p3) = (parsed.clone(), parsed.clone(), parsed.clone());
    // A flex box, not a block: a block lays its text out inline, as
    // part of its own paragraph, and then the text's own layout (which
    // maps the mouse to a character) is never filled in.
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .cursor(if hovered.is_some() { CursorStyle::PointingHand } else { CursorStyle::IBeam })
        .on_mouse_down(MouseButton::Left, cx.listener(move |this: &mut V, e: &MouseDownEvent, _, cx| {
            let text = &p1.pieces()[j].text.text;
            let Some(at) = text_index(&down, text, e.position) else { return };
            let point = |offset| Point { entry: i, piece: j, offset };
            let before = this.text_sel().clone();
            let sel = this.text_sel_mut();
            sel.selection = Some(match e.click_count {
                2 => {
                    let word = ai_text::word_at(text, at);
                    Selection { anchor: point(word.start), head: point(word.end) }
                }
                n if n >= 3 => Selection { anchor: point(0), head: point(text.len()) },
                _ => match sel.selection {
                    Some(s) if e.modifiers.shift => Selection { anchor: s.anchor, head: point(at) },
                    _ => Selection::at(point(at)),
                },
            });
            sel.selecting = true;
            sel.dragged = e.click_count > 1;
            cx.stop_propagation();
            this.text_sel_changed(&before, cx);
        }))
        .on_mouse_move(cx.listener(move |this: &mut V, e: &MouseMoveEvent, _, cx| {
            let before = this.text_sel().clone();
            let sel = this.text_sel_mut();
            if !sel.selecting || e.pressed_button != Some(MouseButton::Left) {
                // Over a link of this text, or off the one it was over.
                let over = if over_text(&moved, e.position) {
                    text_index(&moved, &p3.pieces()[j].text.text, e.position).and_then(|at| ai_text::link_range_at(&p3.pieces()[j].text, at))
                } else {
                    None
                };
                let mine = sel.hover_link.as_ref().is_some_and(|(e, p, _)| (*e, *p) == (i, j));
                match over {
                    Some(r) if sel.hover_link.as_ref().is_none_or(|h| *h != (i, j, r.clone())) => {
                        sel.hover_link = Some((i, j, r));
                        this.text_sel_changed(&before, cx);
                    }
                    None if mine => {
                        sel.hover_link = None;
                        this.text_sel_changed(&before, cx);
                    }
                    _ => {}
                }
                return;
            }
            if !over_text(&moved, e.position) {
                return;
            }
            let Some(offset) = text_index(&moved, &p3.pieces()[j].text.text, e.position) else { return };
            let head = Point { entry: i, piece: j, offset };
            if let Some(s) = &mut sel.selection
                && s.head != head
            {
                s.head = head;
                sel.dragged = true;
                this.text_sel_changed(&before, cx);
            }
        }))
        .on_mouse_up(MouseButton::Left, cx.listener(move |this: &mut V, e: &MouseUpEvent, window, cx| {
            // A click on a link opens it.
            if !this.text_sel().dragged
                && let Some(at) = text_index(&up, &p2.pieces()[j].text.text, e.position)
                && let Some(url) = ai_text::link_at(&p2.pieces()[j].text, at)
            {
                let url = url.to_string();
                this.open_link(&url, window, cx);
            }
        }))
        .child(styled)
}

/// Pieces `range` of entry `i`, drawn: Markdown blocks, lists, tables, code.
pub fn render_range<V: PieceHost, P: PieceSource>(host: &mut V, i: usize, parsed: &Rc<P>, range: Range<usize>, ui: &UiColors, mono: &str, cx: &mut Context<V>) -> gpui::Div {
    let ink = ui.ink(host.accent());
    let z = host.zoom();
    let pieces = parsed.pieces();
    let mut out = div().flex().flex_col().gap_2().line_height(relative(1.45)).text_color(hsla(ui.text));
    let mut j = range.start;
    let n = range.end.min(pieces.len());
    while j < n {
        let piece = &pieces[j];
        let styles = ai_text::highlights(&piece.text, ink, ui.divider);
        match &piece.kind {
            Kind::Para => {
                out = out.child(sel_text(host, i, j, parsed, styles, cx));
                j += 1;
            }
            Kind::Heading(level) => {
                let size = host.heading_size(*level) * z;
                let rule = host.heading_rule(*level);
                out = out.child(
                    div()
                        .mt_1()
                        .text_size(px(size))
                        .font_weight(FontWeight::BOLD)
                        .when(rule, |d| d.pb_1().border_b_1().border_color(hsla(ui.divider)))
                        .child(sel_text(host, i, j, parsed, styles, cx)),
                );
                j += 1;
            }
            Kind::Quote => {
                out = out.child(div().pl_3().border_l_2().border_color(hsla(ui.divider)).text_color(hsla(ui.text_dim)).child(sel_text(host, i, j, parsed, styles, cx)));
                j += 1;
            }
            Kind::Rule => {
                out = out.child(div().my_1().h(px(1.0)).bg(hsla(ui.divider)));
                j += 1;
            }
            Kind::Item { .. } => {
                // A list: its items closer together than paragraphs.
                let mut items = div().flex().flex_col().gap_1();
                while j < n && let Some(Piece { kind: Kind::Item { depth, marker }, text }) = pieces.get(j) {
                    let styles = ai_text::highlights(text, ink, ui.divider);
                    let numbered = marker.ends_with('.');
                    let bullet = div()
                        .flex_none()
                        .min_w(px(if numbered { 18.0 } else { 12.0 }))
                        .text_color(hsla(if marker.starts_with('☑') { ink } else { ui.text_dim }))
                        .child(marker.clone());
                    items = items.child(div().flex().gap_1p5().pl(px(4.0 + *depth as f32 * 18.0)).child(bullet).child(div().flex_1().min_w_0().child(sel_text(host, i, j, parsed, styles, cx))));
                    j += 1;
                    // A code block inside an item stays in the list.
                    while j < n && let Some(Piece { kind: Kind::Code { depth, .. }, .. }) = pieces.get(j) {
                        let indent = 4.0 + *depth as f32 * 18.0;
                        items = items.child(div().pl(px(indent)).child(render_code(host, i, j, parsed, ui, mono, cx)));
                        j += 1;
                    }
                }
                out = out.child(items);
            }
            Kind::Code { .. } => {
                out = out.child(render_code(host, i, j, parsed, ui, mono, cx));
                j += 1;
            }
            Kind::Cell { .. } => {
                let mut table = div().flex().flex_col().rounded_sm().border_1().border_color(hsla(ui.divider)).text_size(px(12.5 * z));
                let mut row_el: Option<(usize, gpui::Div)> = None;
                while j < n && let Some(Piece { kind: Kind::Cell { row, align, .. }, text }) = pieces.get(j) {
                    let (row, align) = (*row, *align);
                    if row_el.as_ref().is_none_or(|(r, _)| *r != row) {
                        if let Some((_, el)) = row_el.take() {
                            table = table.child(el);
                        }
                        let el = div().flex().when(row > 0, |d| d.border_t_1().border_color(hsla(ui.divider))).when(row == 0, |d| d.bg(hsla_alpha(ui.divider, 0.35)).font_weight(FontWeight::BOLD));
                        row_el = Some((row, el));
                    }
                    let styles = ai_text::highlights(text, ink, ui.divider);
                    let cell = div()
                        .flex_1()
                        .min_w_0()
                        .px_2()
                        .py_0p5()
                        .flex()
                        .when(align == Align::Right, |d| d.justify_end())
                        .when(align == Align::Center, |d| d.justify_center())
                        .child(sel_text(host, i, j, parsed, styles, cx));
                    if let Some((_, el)) = row_el.take() {
                        row_el = Some((row, el.child(cell)));
                    }
                    j += 1;
                }
                if let Some((_, el)) = row_el {
                    table = table.child(el);
                }
                out = out.child(table);
            }
            Kind::Image { .. } | Kind::Diagram(_) | Kind::Math => {
                let drawn = host.render_visual(i, j, ui, cx);
                out = out.child(drawn.unwrap_or_else(|| render_code(host, i, j, parsed, ui, mono, cx).into_any_element()));
                j += 1;
            }
            Kind::Footnote { .. } => {
                // The footnotes: a small numbered list.
                let mut notes = div().flex().flex_col().gap_1().mt_2().pt_2().border_t_1().border_color(hsla(ui.divider)).text_size(px(12.0 * z)).text_color(hsla(ui.text_dim));
                while j < n && let Some(Piece { kind: Kind::Footnote { number }, text }) = pieces.get(j) {
                    let styles = ai_text::highlights(text, ink, ui.divider);
                    notes = notes.child(div().flex().gap_1p5().child(div().flex_none().min_w(px(18.0)).child(format!("{number}."))).child(div().flex_1().min_w_0().child(sel_text(host, i, j, parsed, styles, cx))));
                    j += 1;
                }
                out = out.child(notes);
            }
        }
    }
    out
}

/// What a code block's header names: its language, else "code".
fn code_label(kind: &Kind) -> String {
    match kind {
        Kind::Code { lang, .. } if !lang.is_empty() => lang.clone(),
        Kind::Diagram(d) => d.lang().to_string(),
        Kind::Math => "math".into(),
        Kind::Image { .. } => "picture".into(),
        _ => "code".into(),
    }
}

/// A code block: its language and a copy button above, the code coloured.
pub fn render_code<V: PieceHost, P: PieceSource>(host: &mut V, i: usize, j: usize, parsed: &Rc<P>, ui: &UiColors, mono: &str, cx: &mut Context<V>) -> gpui::Div {
    let piece = &parsed.pieces()[j];
    let lang = code_label(&piece.kind);
    let is_error = host.code_is_error(i, &lang);
    let z = host.zoom();
    let styles: Vec<(Range<usize>, HighlightStyle)> = parsed.code_colors(j).cloned().unwrap_or_default().into_iter().map(|(r, c)| (r, HighlightStyle::color(hsla(c)))).collect();
    let copy = host.copy_button(format!("copy-code-{i}-{j}").into(), ClipboardItem::new_string(piece.text.text.clone()), ui, cx);
    div()
        .rounded_md()
        .border_1()
        .border_color(hsla(ui.divider))
        .bg(hsla_alpha(ui.background, 0.8))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .pl_2()
                .pr_1()
                .py_0p5()
                .border_b_1()
                .border_color(hsla(ui.divider))
                .text_size(px(11.0 * z))
                .text_color(hsla(ui.text_dim))
                .child(lang)
                .child(copy),
        )
        // Its own colour: words the colouring leaves alone (and a tool's
        // output) are not left to the default, black.
        .child(
            div()
                .px_2()
                .py_1()
                .font_family(mono.to_string())
                .text_size(px(12.0 * z))
                .line_height(relative(1.35))
                .text_color(hsla(if is_error { ui.danger } else { ui.text }))
                .child(sel_text(host, i, j, parsed, styles, cx)),
        )
}
