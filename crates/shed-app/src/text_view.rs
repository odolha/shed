//! Text in a monospaced font, painted a row at a time and only the rows in
//! view, however long the text: a project's files, diffs, and the AI's
//! prompt and transcript. Editable or read-only, with or without line
//! numbers and wrapping, filling its space or growing with its text.
//!
//! Positions on screen follow the cell grid: a char's x is its display
//! column (tabs to the next stop, wide characters two) times the cell width.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, ElementInputHandler, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, Font, Hsla, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    Pixels, Point, ScrollWheelEvent, SharedString, Subscription, TextAlign, TextRun, UTF16Selection, Window, canvas,
    div, point, prelude::*, px, size,
};
use shed_model::Rgb;
use shed_text::{Buffer, FindOptions, Motion, Selection, expand_tabs};

use crate::line_editor::{EditorEvent, LineEditor};
use crate::menu::{self, Menu, MenuEntry, MenuHost};
use crate::settings::Settings;
use crate::theme::{FontSettings, UiColors, hsla, hsla_alpha};

/// Space around the text.
const PAD: f32 = 6.0;
/// How long changed lines stay marked after the file changed on disk.
const FLASH: Duration = Duration::from_millis(1500);

#[derive(Clone, Debug, Default)]
pub struct TextOptions {
    pub read_only: bool,
    pub line_numbers: bool,
    pub wrap: bool,
    /// Grows with its text between this many rows (a prompt); otherwise it
    /// fills its space and scrolls.
    pub auto_height: Option<(usize, usize)>,
    /// Enter sends `Submit`; Shift+Enter breaks the line.
    pub submit_on_enter: bool,
    /// Shown dimmed while the text is empty.
    pub placeholder: Option<SharedString>,
    /// What line comments start with, for Ctrl+/.
    pub comment: Option<&'static str>,
    /// A file's editor: Bookmark Line (Ctrl+Shift+B) sends `Bookmark`.
    pub bookmark: bool,
}

pub enum TextEvent {
    Changed,
    Submit,
    Cancel,
    /// Ctrl+S.
    Save,
    /// Alt+Z: wrap long lines, or not.
    ToggleWrap,
    /// Bookmark Line (Ctrl+Shift+B): a bookmark to the cursor's line.
    Bookmark,
}

/// Colours per line: byte ranges of the line, each with its colour.
pub type LineSpans = Vec<(Range<usize>, Rgb)>;

#[derive(Clone, Copy)]
struct Metrics {
    cell: Pixels,
    line: Pixels,
    font_size: Pixels,
}

/// A row on screen: part of a line (all of it, unless wrapped), by char columns.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Row {
    line: usize,
    start: usize,
    end: usize,
}

/// Where things were drawn last, to place clicks.
#[derive(Clone, Copy)]
struct Geometry {
    /// The top-left of the text (after the gutter), before scrolling.
    origin: Point<Pixels>,
    bounds: Bounds<Pixels>,
    m: Metrics,
}

/// An Alt (Option) drag's rectangle, for copying only: the line and display
/// column where it started and where it is now.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Block {
    anchor: (usize, usize),
    head: (usize, usize),
}

impl Block {
    fn lines(&self) -> Range<usize> {
        self.anchor.0.min(self.head.0)..self.anchor.0.max(self.head.0) + 1
    }

    fn cols(&self) -> Range<usize> {
        self.anchor.1.min(self.head.1)..self.anchor.1.max(self.head.1)
    }

    /// Covers some columns (a press without a drag does not).
    fn spans(&self) -> bool {
        !self.cols().is_empty()
    }
}

struct FindBar {
    query: Entity<LineEditor>,
    replace: Option<Entity<LineEditor>>,
    options: FindOptions,
    /// What `matches` were found for, and in which version of the text.
    found_for: (String, u64, FindOptions),
    matches: Vec<Range<usize>>,
    _subs: Vec<Subscription>,
}

struct GotoBar {
    editor: Entity<LineEditor>,
    _sub: Subscription,
}

pub struct TextView {
    pub buffer: Buffer,
    pub options: TextOptions,
    font: FontSettings,
    fg: Rgb,
    bg: Rgb,
    selection: Rgb,
    cursor: Rgb,
    ui: UiColors,
    focus: FocusHandle,
    scroll_y: Pixels,
    scroll_x: Pixels,
    /// The furthest it scrolls and how much shows, at the last layout (for
    /// the scrollbar).
    scroll_max: Pixels,
    view_height: Pixels,
    bar: crate::scrollbar::State,
    /// Bring the cursor into view at the next layout.
    autoscroll: bool,
    /// Put the cursor's row in the middle of the view at the next layout.
    center: bool,
    highlights: Option<Arc<Vec<LineSpans>>>,
    /// Lines changed on disk, marked for a moment.
    flashes: Vec<(Range<usize>, Instant)>,
    find: Option<FindBar>,
    goto: Option<GotoBar>,
    menu: Option<Menu<Self>>,
    preedit: Option<String>,
    dragging: bool,
    /// A rectangle selected by an Alt drag; any edit or move ends it.
    block: Option<Block>,
    /// Wrapped rows, for a version of the text at a width in columns.
    wrapped: Option<(u64, usize, Rc<Vec<Row>>)>,
    /// Columns that fit, from the last layout.
    cols: Rc<Cell<usize>>,
    geometry: Rc<Cell<Option<Geometry>>>,
}

impl EventEmitter<TextEvent> for TextView {}

impl TextView {
    pub fn new(text: &str, options: TextOptions, settings: &Settings, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            buffer: Buffer::new(text),
            options,
            font: settings.font.clone(),
            fg: settings.palette.foreground,
            bg: settings.palette.background,
            selection: settings.palette.selection,
            cursor: settings.palette.cursor,
            ui: settings.ui.clone(),
            focus: cx.focus_handle(),
            scroll_y: px(0.0),
            scroll_x: px(0.0),
            scroll_max: px(0.0),
            view_height: px(0.0),
            bar: Default::default(),
            autoscroll: false,
            center: false,
            highlights: None,
            flashes: Vec::new(),
            find: None,
            goto: None,
            menu: None,
            preedit: None,
            dragging: false,
            block: None,
            wrapped: None,
            cols: Rc::new(Cell::new(80)),
            geometry: Rc::new(Cell::new(None)),
        };
        view.buffer.set_cursor(0, false);
        view
    }

    /// Wraps long lines at the edge, or lets them run on (scrolling sideways).
    pub fn set_wrap(&mut self, wrap: bool, cx: &mut Context<Self>) {
        if self.options.wrap != wrap {
            self.options.wrap = wrap;
            self.wrapped = None;
            self.scroll_x = px(0.0);
            self.autoscroll = true;
            cx.notify();
        }
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// Takes new colours and font from a reloaded config.
    pub fn apply(&mut self, settings: &Settings, cx: &mut Context<Self>) {
        self.font = settings.font.clone();
        self.fg = settings.palette.foreground;
        self.bg = settings.palette.background;
        self.selection = settings.palette.selection;
        self.cursor = settings.palette.cursor;
        self.ui = settings.ui.clone();
        cx.notify();
    }

    /// Takes the text as it now is on disk, keeping the cursor with its
    /// text, and marks the lines that changed for a moment.
    pub fn reload(&mut self, text: &str, cx: &mut Context<Self>) {
        let changed = self.buffer.reload(text);
        if changed.is_empty() {
            return;
        }
        let now = Instant::now();
        self.flashes = changed.into_iter().map(|r| (r, now)).collect();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FLASH).await;
            this.update(cx, |this, cx| {
                this.flashes.retain(|(_, at)| at.elapsed() < FLASH);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.emit(TextEvent::Changed);
        cx.notify();
    }

    /// Colours for the lines; they may be for an older version of the text
    /// for a moment, which only means a few odd colours until the next.
    pub fn set_highlights(&mut self, spans: Option<Arc<Vec<LineSpans>>>, cx: &mut Context<Self>) {
        self.highlights = spans;
        cx.notify();
    }

    /// Puts the cursor on a line (from 0) and column, in the middle of the view.
    pub fn go_to(&mut self, line: usize, col: usize, cx: &mut Context<Self>) {
        self.block = None;
        let pos = self.buffer.pos(line, col);
        self.buffer.set_cursor(pos, false);
        self.center = true;
        cx.notify();
    }

    pub fn select(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        self.block = None;
        self.buffer.set_selection(Selection { anchor: range.start, head: range.end });
        self.autoscroll = true;
        cx.notify();
    }

    fn metrics(&self, window: &mut Window) -> Metrics {
        let font_size = px(self.font.size);
        let ts = window.text_system();
        let id = ts.resolve_font(&self.font.font());
        let cell = ts.advance(id, font_size, 'm').map(|s| s.width).unwrap_or(font_size * 0.6);
        Metrics { cell, line: px((self.font.size * self.font.line_height).round()), font_size }
    }

    /// Every row, when wrapping; otherwise `None` (a row is a line).
    fn wrapped_rows(&mut self) -> Option<Rc<Vec<Row>>> {
        if !self.options.wrap {
            return None;
        }
        let (version, cols) = (self.buffer.version(), self.cols.get().max(8));
        if let Some((v, c, rows)) = &self.wrapped
            && (*v, *c) == (version, cols)
        {
            return Some(rows.clone());
        }
        let rows = Rc::new(wrap(&self.buffer, cols));
        self.wrapped = Some((version, cols, rows.clone()));
        Some(rows)
    }

    fn row_count(&mut self) -> usize {
        self.wrapped_rows().map_or(self.buffer.len_lines(), |r| r.len())
    }

    fn row(&mut self, i: usize) -> Row {
        match self.wrapped_rows() {
            Some(rows) => rows.get(i).copied().unwrap_or(Row { line: 0, start: 0, end: 0 }),
            None => Row { line: i, start: 0, end: self.buffer.line_len(i) },
        }
    }

    /// The row a position is on, and its row.
    fn row_of(&mut self, pos: usize) -> (usize, Row) {
        let (line, col) = self.buffer.line_col(pos);
        match self.wrapped_rows() {
            Some(rows) => {
                let i = rows.partition_point(|r| r.line < line || (r.line == line && r.end < col && r.end != r.start));
                let i = i.min(rows.len().saturating_sub(1));
                (i, rows[i])
            }
            None => (line, Row { line, start: 0, end: self.buffer.line_len(line) }),
        }
    }

    /// The display width of a row's chars from its start up to `col`.
    fn row_x(&self, row: Row, col: usize) -> usize {
        let line = self.buffer.line(row.line);
        shed_text::display_width(line.chars().skip(row.start).take(col.saturating_sub(row.start)), self.buffer.tab_width)
    }

    /// The row under a point, and the column edge nearest it (from the row's start).
    fn row_edge_at(&mut self, at: Point<Pixels>) -> Option<(Row, usize)> {
        let g = self.geometry.get()?;
        let y = at.y - g.origin.y + self.scroll_y;
        let count = self.row_count();
        let r = if y < px(0.0) { 0 } else { ((y / g.m.line) as usize).min(count.saturating_sub(1)) };
        let x = ((at.x - g.origin.x + self.scroll_x) / g.m.cell).max(0.0).round() as usize;
        Some((self.row(r), x))
    }

    /// The line and display column under a point, past the line's end too.
    fn line_edge_at(&mut self, at: Point<Pixels>) -> (usize, usize) {
        match self.row_edge_at(at) {
            Some((row, x)) => (row.line, self.buffer.display_col(row.line, row.start) + x),
            None => {
                let (line, col) = self.buffer.line_col(self.buffer.selection().head);
                (line, self.buffer.display_col(line, col))
            }
        }
    }

    /// The text position under a point.
    fn pos_at(&mut self, at: Point<Pixels>) -> usize {
        let Some((row, x)) = self.row_edge_at(at) else { return self.buffer.selection().head };
        let line = self.buffer.line(row.line);
        let mut width = 0;
        let mut col = row.start;
        for c in line.chars().skip(row.start).take(row.end - row.start) {
            let w = shed_text::char_width(c, width, self.buffer.tab_width);
            if width + w.div_ceil(2) > x {
                break;
            }
            width += w;
            col += 1;
        }
        self.buffer.pos(row.line, col)
    }

    fn edited(&mut self, cx: &mut Context<Self>) {
        self.autoscroll = true;
        cx.emit(TextEvent::Changed);
        cx.notify();
    }

    fn moved(&mut self, cx: &mut Context<Self>) {
        self.autoscroll = true;
        cx.notify();
    }

    /// Rows that fit in the view, for Page Up and Down.
    fn page(&self) -> usize {
        self.geometry.get().map_or(20, |g| ((g.bounds.size.height / g.m.line) as usize).saturating_sub(2).max(1))
    }

    /// The rectangle's text: each line's slice of its columns.
    fn block_text(&self) -> Option<String> {
        let b = self.block.filter(Block::spans)?;
        Some(self.buffer.block_text(b.lines(), b.cols()))
    }

    fn copy(&mut self, cut: bool, cx: &mut Context<Self>) {
        // A rectangle is only copied, cut or not.
        if let Some(text) = self.block_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            return;
        }
        let sel = self.buffer.selection();
        // Nothing selected: the whole line, as editors do.
        if sel.is_empty() {
            let line = self.buffer.line_of(sel.head);
            cx.write_to_clipboard(ClipboardItem::new_string(format!("{}\n", self.buffer.line(line))));
            if cut && !self.options.read_only {
                self.buffer.delete_lines();
                self.edited(cx);
            }
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(self.buffer.selected_text()));
        if cut && !self.options.read_only {
            self.buffer.insert("");
            self.edited(cx);
        }
    }

    pub fn paste_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.options.read_only {
            return;
        }
        self.block = None;
        self.buffer.insert(&text.replace("\r\n", "\n"));
        self.edited(cx);
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let m = ks.modifiers;
        let (sec, shift, alt) = (m.secondary(), m.shift, m.alt);
        let editable = !self.options.read_only;
        let page = self.page();
        let mv = |this: &mut Self, motion: Motion, cx: &mut Context<Self>| {
            this.buffer.move_cursor(motion, shift);
            this.moved(cx);
        };
        // A rectangle stays for copying; Escape or any other key ends it
        // (the key then does what it does at the caret).
        if self.block.is_some() {
            match ks.key.as_str() {
                "shift" | "control" | "alt" | "platform" | "function" => return,
                "c" | "x" if sec && !shift => {}
                "escape" => {
                    self.block = None;
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                _ => {
                    self.block = None;
                    cx.notify();
                }
            }
        }
        match ks.key.as_str() {
            "escape" => cx.emit(TextEvent::Cancel),
            "left" if sec => mv(self, Motion::WordLeft, cx),
            "right" if sec => mv(self, Motion::WordRight, cx),
            "up" if alt && shift && editable => {
                self.buffer.move_lines(true);
                self.edited(cx);
            }
            "down" if alt && shift && editable => {
                self.buffer.move_lines(false);
                self.edited(cx);
            }
            "left" => mv(self, Motion::Left, cx),
            "right" => mv(self, Motion::Right, cx),
            "up" => mv(self, Motion::Up, cx),
            "down" => mv(self, Motion::Down, cx),
            "home" if sec => mv(self, Motion::DocStart, cx),
            "end" if sec => mv(self, Motion::DocEnd, cx),
            "home" => mv(self, Motion::LineStart, cx),
            "end" => mv(self, Motion::LineEnd, cx),
            "pageup" => mv(self, Motion::PageUp(page), cx),
            "pagedown" => mv(self, Motion::PageDown(page), cx),
            "enter" if self.options.submit_on_enter && !shift => cx.emit(TextEvent::Submit),
            "enter" if editable => {
                self.buffer.newline();
                self.edited(cx);
            }
            "tab" if editable && !sec && !alt => {
                if shift { self.buffer.outdent() } else { self.buffer.indent() }
                self.edited(cx);
            }
            "backspace" if editable => {
                if sec || alt { self.buffer.delete_word_back() } else { self.buffer.backspace() }
                self.edited(cx);
            }
            "delete" if editable => {
                if sec || alt { self.buffer.delete_word_forward() } else { self.buffer.delete_forward() }
                self.edited(cx);
            }
            "a" if sec => {
                self.buffer.select_all();
                cx.notify();
            }
            "c" if sec && !shift => self.copy(false, cx),
            "x" if sec => self.copy(true, cx),
            "v" if sec && !shift => {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.paste_text(&text, cx);
                }
            }
            "z" if sec && shift && editable => {
                self.buffer.redo();
                self.edited(cx);
            }
            "z" if sec && editable => {
                self.buffer.undo();
                self.edited(cx);
            }
            "y" if sec && editable => {
                self.buffer.redo();
                self.edited(cx);
            }
            "d" if sec && editable => {
                self.buffer.duplicate();
                self.edited(cx);
            }
            "k" if sec && shift && editable => {
                self.buffer.delete_lines();
                self.edited(cx);
            }
            "/" if sec && editable => {
                if let Some(prefix) = self.options.comment {
                    self.buffer.toggle_comment(prefix);
                    self.edited(cx);
                }
            }
            "f" if sec && !shift => self.open_find(false, window, cx),
            "h" | "r" if sec && editable && !self.options.submit_on_enter => self.open_find(true, window, cx),
            "g" if sec && self.options.auto_height.is_none() => self.open_goto(window, cx),
            "s" if sec => cx.emit(TextEvent::Save),
            "b" if sec && shift && self.options.bookmark => cx.emit(TextEvent::Bookmark),
            "z" if m.alt && !m.control && !m.platform && self.options.line_numbers => cx.emit(TextEvent::ToggleWrap),
            _ => match ks.key_char.as_deref() {
                Some(text) if editable && !m.control && !m.platform && !text.chars().any(char::is_control) => {
                    self.buffer.insert(text);
                    self.edited(cx);
                }
                _ => return,
            },
        }
        cx.stop_propagation();
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let pos = self.pos_at(event.position);
        match event.button {
            MouseButton::Left => {
                self.block = None;
                match event.click_count {
                    // Alt (Option) dragging selects a rectangle, to copy.
                    1 if event.modifiers.alt && !event.modifiers.shift => {
                        self.buffer.set_cursor(pos, false);
                        let at = self.line_edge_at(event.position);
                        self.block = Some(Block { anchor: at, head: at });
                    }
                    1 => self.buffer.set_cursor(pos, event.modifiers.shift),
                    2 => self.buffer.select_word(pos),
                    _ => self.buffer.select_line(pos),
                }
                self.dragging = event.click_count == 1;
                cx.notify();
            }
            #[cfg(target_os = "linux")]
            MouseButton::Middle if !self.options.read_only => {
                self.block = None;
                if let Some(text) = cx.read_from_primary().and_then(|c| c.text()) {
                    self.buffer.set_cursor(pos, false);
                    self.paste_text(&text, cx);
                }
            }
            MouseButton::Right => {
                if self.block.is_none() && !self.buffer.selection().range().contains(&pos) {
                    self.buffer.set_cursor(pos, false);
                }
                let entries = self.context_menu();
                self.open_menu(event.position, entries, cx);
            }
            _ => {}
        }
        cx.stop_propagation();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.dragging || event.pressed_button != Some(MouseButton::Left) {
            self.dragging = false;
            return;
        }
        if let Some(block) = self.block {
            let head = self.line_edge_at(event.position);
            if head != block.head {
                self.block = Some(Block { head, ..block });
                let pos = self.buffer.pos(head.0, self.buffer.col_at_display(head.0, head.1));
                self.buffer.set_cursor(pos, false);
                self.autoscroll = true;
                cx.notify();
            }
            return;
        }
        let pos = self.pos_at(event.position);
        if pos != self.buffer.selection().head {
            self.buffer.set_cursor(pos, true);
            self.autoscroll = true;
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.dragging = false;
        if self.block.is_some_and(|b| !b.spans()) {
            self.block = None;
        }
        // X11 convention: selecting copies to the primary selection.
        #[cfg(target_os = "linux")]
        if let Some(text) = self.block_text() {
            cx.write_to_primary(ClipboardItem::new_string(text));
        } else if !self.buffer.selection().is_empty() {
            cx.write_to_primary(ClipboardItem::new_string(self.buffer.selected_text()));
        }
        #[cfg(not(target_os = "linux"))]
        let _ = cx;
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(g) = self.geometry.get() else { return };
        let delta = event.delta.pixel_delta(g.m.line);
        if event.modifiers.shift || delta.x != px(0.0) {
            let dx = if event.modifiers.shift { delta.y } else { delta.x };
            self.scroll_x = (self.scroll_x - dx).max(px(0.0));
        } else {
            self.scroll_y -= delta.y;
        }
        // Clamped at the next layout.
        cx.notify();
        if self.options.auto_height.is_none() {
            cx.stop_propagation();
        }
    }

    fn context_menu(&self) -> Vec<MenuEntry<Self>> {
        let mut entries = Vec::new();
        if !self.options.read_only {
            entries.push(menu::item("Cut", |this: &mut Self, _, cx| this.copy(true, cx)));
        }
        entries.push(menu::item("Copy", |this: &mut Self, _, cx| this.copy(false, cx)));
        if !self.options.read_only {
            entries.push(menu::item("Paste", |this: &mut Self, _, cx| {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    this.paste_text(&text, cx);
                }
            }));
        }
        entries.push(MenuEntry::Separator);
        entries.push(menu::item("Select All", |this: &mut Self, _, cx| {
            this.block = None;
            this.buffer.select_all();
            cx.notify();
        }));
        entries.push(menu::item("Find…", |this: &mut Self, window, cx| this.open_find(false, window, cx)));
        if self.options.bookmark {
            entries.push(MenuEntry::Separator);
            let key = if cfg!(target_os = "macos") { "Keep this line in the bench's bookmarks (Cmd+Shift+B)" } else { "Keep this line in the bench's bookmarks (Ctrl+Shift+B)" };
            entries.push(menu::item("Bookmark Line", |_: &mut Self, _, cx| cx.emit(TextEvent::Bookmark)).icon("bookmark").tip(key));
        }
        entries
    }

    fn open_find(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.buffer.selected_text();
        let seed = if !selected.is_empty() && !selected.contains('\n') { Some(selected) } else { None };
        if self.find.is_none() {
            let query = cx.new(|cx| LineEditor::selected(seed.clone().unwrap_or_default(), cx).without_hint());
            let subs = vec![cx.observe(&query, |this, _, cx| this.refresh_find(cx))];
            self.find = Some(FindBar {
                query,
                replace: None,
                options: FindOptions::default(),
                found_for: (String::new(), u64::MAX, FindOptions::default()),
                matches: Vec::new(),
                _subs: subs,
            });
        } else if let (Some(seed), Some(find)) = (seed, &self.find) {
            find.query.update(cx, |q, cx| *q = LineEditor::selected(seed, cx).without_hint());
        }
        let find = self.find.as_mut().unwrap();
        if replace && find.replace.is_none() {
            let editor = cx.new(|cx| LineEditor::new(String::new(), cx).without_hint());
            find.replace = Some(editor);
        }
        let target = if replace { find.replace.clone().unwrap() } else { find.query.clone() };
        window.focus(&target.read(cx).focus_handle(cx), cx);
        self.refresh_find(cx);
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn refresh_find(&mut self, cx: &mut Context<Self>) {
        let Some(find) = &mut self.find else { return };
        let query = find.query.read(cx).text().to_string();
        let key = (query.clone(), self.buffer.version(), find.options.clone());
        if find.found_for != key {
            find.matches = self.buffer.find_all(&query, &find.options);
            find.found_for = key;
        }
        cx.notify();
    }

    /// Selects the next match after the cursor (or before, going back).
    fn find_step(&mut self, back: bool, cx: &mut Context<Self>) {
        self.refresh_find(cx);
        let Some(find) = &self.find else { return };
        if find.matches.is_empty() {
            return;
        }
        let sel = self.buffer.selection().range();
        let next = if back {
            find.matches.iter().rev().find(|m| m.start < sel.start).or(find.matches.last())
        } else {
            find.matches.iter().find(|m| m.start > sel.start || (m.start == sel.start && sel.is_empty())).or(find.matches.first())
        };
        if let Some(m) = next.cloned() {
            self.select(m, cx);
        }
    }

    fn replace_step(&mut self, all: bool, cx: &mut Context<Self>) {
        self.refresh_find(cx);
        let Some(find) = &self.find else { return };
        let with = find.replace.as_ref().map(|r| r.read(cx).text().to_string()).unwrap_or_default();
        if all {
            let matches = find.matches.clone();
            self.buffer.replace_all(&matches, &with);
        } else {
            let sel = self.buffer.selection().range();
            if find.matches.contains(&sel) {
                self.buffer.insert(&with);
            }
            self.refresh_find(cx);
            self.find_step(false, cx);
        }
        self.edited(cx);
    }

    fn find_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let in_replace = self.find.as_ref().and_then(|f| f.replace.as_ref()).is_some_and(|r| r.read(cx).focus_handle(cx).is_focused(window));
        match ks.key.as_str() {
            "escape" => self.close_find(window, cx),
            "enter" if in_replace => self.replace_step(ks.modifiers.secondary(), cx),
            "enter" => self.find_step(ks.modifiers.shift, cx),
            "up" => self.find_step(true, cx),
            "down" => self.find_step(false, cx),
            "tab" => {
                let Some(find) = &self.find else { return };
                let other = if in_replace { Some(find.query.clone()) } else { find.replace.clone() };
                if let Some(other) = other {
                    window.focus(&other.read(cx).focus_handle(cx), cx);
                }
            }
            "c" if ks.modifiers.alt => self.toggle_find_option(true, cx),
            "w" if ks.modifiers.alt => self.toggle_find_option(false, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn toggle_find_option(&mut self, case: bool, cx: &mut Context<Self>) {
        if let Some(find) = &mut self.find {
            if case {
                find.options.case_sensitive = !find.options.case_sensitive;
            } else {
                find.options.whole_word = !find.options.whole_word;
            }
        }
        self.refresh_find(cx);
    }

    fn open_goto(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editor = cx.new(|cx| LineEditor::new(String::new(), cx).without_hint());
        window.focus(&editor.read(cx).focus_handle(cx), cx);
        let sub = cx.subscribe_in(&editor, window, |this, _, event: &EditorEvent, window, cx| {
            if let EditorEvent::Submit(text) = event {
                let mut parts = text.split(':').map(|p| p.trim().parse::<usize>().ok());
                if let Some(Some(line)) = parts.next() {
                    let col = parts.next().flatten().unwrap_or(1);
                    this.go_to(line.saturating_sub(1), col.saturating_sub(1), cx);
                }
            }
            this.goto = None;
            window.focus(&this.focus, cx);
            cx.notify();
        });
        self.goto = Some(GotoBar { editor, _sub: sub });
        cx.notify();
    }

    /// Lays the view out for `bounds`: keeps the scroll in range and the
    /// cursor in view, and says what to draw.
    fn layout(&mut self, bounds: Bounds<Pixels>, focused: bool, window: &mut Window) -> Frame {
        let m = self.metrics(window);
        let lines = self.buffer.len_lines();
        let gutter = if self.options.line_numbers { m.cell * (lines.to_string().len().max(3) + 2) as f32 } else { px(0.0) };
        let origin = point(bounds.origin.x + gutter + px(PAD), bounds.origin.y + px(PAD));
        let text_w = (bounds.size.width - gutter - px(2.0 * PAD)).max(m.cell);
        let text_h = (bounds.size.height - px(2.0 * PAD)).max(m.line);
        let cols = (text_w / m.cell).floor().max(8.0) as usize;
        if self.cols.get() != cols {
            self.cols.set(cols);
        }
        self.geometry.set(Some(Geometry { origin, bounds, m }));
        let count = self.row_count();

        let sel = self.buffer.selection();
        let (cursor_row, crow) = self.row_of(sel.head);
        let cursor_x = m.cell * self.row_x(crow, self.buffer.line_col(sel.head).1) as f32;
        if std::mem::take(&mut self.center) {
            self.scroll_y = (m.line * cursor_row as f32 - text_h / 2.0).max(px(0.0));
            self.autoscroll = false;
        } else if std::mem::take(&mut self.autoscroll) {
            let margin = if self.options.auto_height.is_some() { px(0.0) } else { m.line * 2.0 };
            let top = m.line * cursor_row as f32;
            if top - margin < self.scroll_y {
                self.scroll_y = top - margin;
            } else if top + m.line + margin > self.scroll_y + text_h {
                self.scroll_y = top + m.line + margin - text_h;
            }
            if !self.options.wrap {
                if cursor_x < self.scroll_x + m.cell * 2.0 {
                    self.scroll_x = (cursor_x - m.cell * 8.0).max(px(0.0));
                } else if cursor_x > self.scroll_x + text_w - m.cell * 2.0 {
                    self.scroll_x = cursor_x - text_w + m.cell * 8.0;
                }
            }
        }
        let tail = if self.options.auto_height.is_some() { px(0.0) } else { text_h / 2.0 };
        let max_y = (m.line * count as f32 - text_h + tail).max(px(0.0));
        self.scroll_y = self.scroll_y.clamp(px(0.0), max_y);
        (self.scroll_max, self.view_height) = (max_y, text_h);
        if self.options.wrap {
            self.scroll_x = px(0.0);
        }

        let first = (self.scroll_y / m.line).floor() as usize;
        let shown = (text_h / m.line).ceil() as usize + 1;
        let y0 = origin.y - (self.scroll_y - m.line * first as f32);
        let x0 = origin.x - self.scroll_x;
        let font = self.font.font();
        let sel_range = sel.range();
        let find = self.find.as_ref().map(|f| f.matches.clone()).unwrap_or_default();
        let highlights = self.highlights.clone();
        let current_line = self.buffer.line_of(sel.head);
        let block = self.block.filter(Block::spans);

        let mut frame = Frame { quads: Vec::new(), texts: Vec::new(), gutter_texts: Vec::new(), cursor: None, mask: Bounds::new(point(bounds.origin.x + gutter, bounds.origin.y), size(bounds.size.width - gutter, bounds.size.height)), font_size: m.font_size, line: m.line };
        let accent = self.ui.accent;
        for i in 0..shown {
            let r = first + i;
            if r >= count {
                break;
            }
            let row = self.row(r);
            let y = y0 + m.line * i as f32;
            let line_start = self.buffer.line_start(row.line);
            let text = self.buffer.line(row.line);
            let chars: Vec<char> = text.chars().collect();
            let row_chars: String = chars[row.start.min(chars.len())..row.end.min(chars.len())].iter().collect();
            let (drawn, starts) = expand_tabs(&row_chars, self.buffer.tab_width);
            let x_of = |col: usize| {
                let c = col.clamp(row.start, row.end) - row.start;
                m.cell * shed_text::display_width(row_chars.chars().take(c), self.buffer.tab_width) as f32
            };
            let row_band = |from: usize, to: usize, extra: bool| -> Option<Bounds<Pixels>> {
                let (a, b) = (from.max(line_start + row.start), to.min(line_start + row.end));
                if a > b || (a == b && !extra) {
                    return None;
                }
                let (xa, xb) = (x_of(a - line_start), x_of(b - line_start));
                let w = (xb - xa).max(if extra { m.cell * 0.5 } else { px(0.0) });
                Some(Bounds::new(point(x0 + xa, y), size(w, m.line)))
            };
            if focused && !self.options.read_only && sel.is_empty() && block.is_none() && row.line == current_line && self.options.line_numbers {
                frame.quads.push((Bounds::new(point(frame.mask.origin.x, y), size(frame.mask.size.width, m.line)), hsla_alpha(self.ui.tab_hover, 0.6)));
            }
            for (range, _) in self.flashes.iter().filter(|(r, _)| r.contains(&row.line)) {
                let _ = range;
                frame.quads.push((Bounds::new(point(frame.mask.origin.x, y), size(frame.mask.size.width, m.line)), hsla_alpha(accent, 0.16)));
            }
            for f in find.iter().filter(|f| f.end >= line_start + row.start && f.start <= line_start + row.end) {
                if let Some(b) = row_band(f.start, f.end, false) {
                    frame.quads.push((b, hsla_alpha(accent, 0.35)));
                }
            }
            // A rectangle: each line's chars in its columns.
            if let Some(b) = block.filter(|b| b.lines().contains(&row.line)) {
                let r = shed_text::chars_in_columns(&text, b.cols(), self.buffer.tab_width);
                if let Some(band) = row_band(line_start + r.start, line_start + r.end, false) {
                    frame.quads.push((band, hsla(self.selection)));
                }
            }
            if !sel_range.is_empty() {
                // A selected line break shows as a sliver past the line's end.
                let through_break = sel_range.end > line_start + row.end && row.end == chars.len();
                if let Some(b) = row_band(sel_range.start, sel_range.end, through_break) {
                    frame.quads.push((b, hsla(self.selection)));
                }
            }
            if !drawn.is_empty() {
                let runs = runs_for(&text, row, &drawn, &starts, highlights.as_ref().and_then(|h| h.get(row.line)), self.fg, &font);
                frame.texts.push((point(x0, y), SharedString::from(drawn), runs));
            }
            if self.options.line_numbers && row.start == 0 {
                let number = (row.line + 1).to_string();
                let color = if row.line == current_line && focused { self.fg } else { self.ui.text_dim };
                let w = m.cell * number.len() as f32;
                let x = bounds.origin.x + gutter - m.cell - w;
                frame.gutter_texts.push((point(x, y), SharedString::from(number), color));
            }
            if r == cursor_row && focused && !self.options.read_only {
                frame.cursor = Some(Bounds::new(point(x0 + cursor_x, y), size(px(2.0), m.line)));
            }
        }
        if self.buffer.len_chars() == 0
            && let Some(p) = &self.options.placeholder
        {
            frame.gutter_texts.push((origin, p.clone(), self.ui.text_dim));
        }
        frame
    }

    /// The height that shows every row, between the given bounds.
    fn auto_height(&mut self, window: &mut Window) -> Option<Pixels> {
        let (min, max) = self.options.auto_height?;
        let m = self.metrics(window);
        let rows = self.row_count().clamp(min.max(1), max.max(1));
        Some(m.line * rows as f32 + px(2.0 * PAD))
    }
}

/// The text runs of a drawn row, coloured by `spans` (byte ranges of the
/// whole line) where given.
fn runs_for(line: &str, row: Row, drawn: &str, starts: &[usize], spans: Option<&LineSpans>, fg: Rgb, font: &Font) -> Vec<TextRun> {
    let run = |len: usize, color: Rgb| TextRun { len, font: font.clone(), color: hsla(color), background_color: None, underline: None, strikethrough: None, letter_spacing: None };
    let Some(spans) = spans.filter(|s| !s.is_empty()) else { return vec![run(drawn.len(), fg)] };
    let mut runs: Vec<TextRun> = Vec::new();
    let mut last: Option<Rgb> = None;
    let mut span_i = 0;
    for (i, (byte, _)) in line.char_indices().skip(row.start).take(row.end - row.start).enumerate() {
        while span_i < spans.len() && spans[span_i].0.end <= byte {
            span_i += 1;
        }
        let color = spans.get(span_i).filter(|(r, _)| r.start <= byte).map_or(fg, |(_, c)| *c);
        let len = starts[i + 1] - starts[i];
        match runs.last_mut() {
            Some(r) if last == Some(color) => r.len += len,
            _ => runs.push(run(len, color)),
        }
        last = Some(color);
    }
    runs
}

/// Rows for a text wrapped at `cols` columns, breaking after a space where
/// there is one in the row.
fn wrap(buffer: &Buffer, cols: usize) -> Vec<Row> {
    let mut rows = Vec::new();
    for line in 0..buffer.len_lines() {
        let chars: Vec<char> = buffer.line(line).chars().collect();
        let mut start = 0;
        let mut width = 0;
        let mut space = None;
        let mut i = 0;
        while i < chars.len() {
            let w = shed_text::char_width(chars[i], width, buffer.tab_width);
            if width + w > cols && i > start {
                let end = space.filter(|s| *s > start).unwrap_or(i);
                rows.push(Row { line, start, end });
                start = end;
                width = shed_text::display_width(chars[start..i].iter().copied(), buffer.tab_width);
                space = None;
                continue;
            }
            width += w;
            if chars[i] == ' ' {
                space = Some(i + 1);
            }
            i += 1;
        }
        rows.push(Row { line, start, end: chars.len() });
    }
    rows
}

/// What one paint draws.
struct Frame {
    quads: Vec<(Bounds<Pixels>, Hsla)>,
    texts: Vec<(Point<Pixels>, SharedString, Vec<TextRun>)>,
    /// Line numbers and the placeholder, in one colour each.
    gutter_texts: Vec<(Point<Pixels>, SharedString, Rgb)>,
    cursor: Option<Bounds<Pixels>>,
    /// The text area, where scrolled text is clipped.
    mask: Bounds<Pixels>,
    font_size: Pixels,
    line: Pixels,
}

impl MenuHost for TextView {
    fn menu_slot(&mut self) -> &mut Option<Menu<Self>> {
        &mut self.menu
    }

    fn menu_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }
}

impl Render for TextView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(find) = &self.find
            && find.found_for.1 != self.buffer.version()
        {
            self.refresh_find(cx);
        }
        let view = cx.entity();
        let input = (cx.entity(), self.focus.clone());
        let (font, cursor_color, fg) = (self.font.font(), self.cursor, self.fg);
        let height = self.auto_height(window);
        let ui = self.ui.clone();
        let menu = self.menu.as_ref().map(|m| menu::render(m, &ui, cx));
        let find_bar = self.find.as_ref().map(|f| {
            let count = f.matches.len();
            let sel = self.buffer.selection().range();
            let at = f.matches.iter().position(|m| *m == sel).map(|i| i + 1);
            let status = match (count, at) {
                (0, _) if f.found_for.0.is_empty() => String::new(),
                (0, _) => "no matches".into(),
                (n, Some(i)) => format!("{i}/{n}"),
                (n, None) => format!("{n} found"),
            };
            let toggle = |label: &'static str, on: bool| {
                div()
                    .px_1()
                    .rounded_sm()
                    .text_color(hsla(if on { ui.text } else { ui.text_dim }))
                    .when(on, |d| d.bg(hsla_alpha(ui.accent, 0.3)))
                    .child(label)
            };
            div()
                .absolute()
                .top(px(4.0))
                .right(px(12.0))
                .flex()
                .flex_col()
                .gap_1()
                .px_2()
                .py_1()
                .rounded_md()
                .bg(hsla(ui.tab_active))
                .border_1()
                .border_color(hsla(ui.divider))
                .text_size(px(12.0))
                .capture_key_down(cx.listener(Self::find_key))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().w(px(220.0)).text_color(hsla(ui.text)).child(f.query.clone()))
                        .child(toggle("Aa", f.options.case_sensitive))
                        .child(toggle("W", f.options.whole_word))
                        .child(div().min_w(px(60.0)).text_color(hsla(if count == 0 && !f.found_for.0.is_empty() { ui.danger } else { ui.text_dim })).child(status)),
                )
                .when_some(f.replace.clone(), |el, replace| {
                    el.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().w(px(220.0)).text_color(hsla(ui.text)).child(replace))
                            .child(div().text_color(hsla(ui.text_dim)).child("Enter replaces · Ctrl+Enter all")),
                    )
                })
        });
        let goto_bar = self.goto.as_ref().map(|g| {
            div()
                .absolute()
                .top(px(4.0))
                .right(px(12.0))
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded_md()
                .bg(hsla(ui.tab_active))
                .border_1()
                .border_color(hsla(ui.divider))
                .text_size(px(12.0))
                .child(div().text_color(hsla(ui.text_dim)).child("Go to line[:column]"))
                .child(div().w(px(100.0)).text_color(hsla(ui.text)).child(g.editor.clone()))
        });
        let preedit = self.preedit.clone();
        let focus = self.focus.clone();
        let (read, write) = (cx.entity().downgrade(), cx.entity().downgrade());
        let source = crate::scrollbar::Source::Custom {
            metrics: std::rc::Rc::new(move |cx: &gpui::App| {
                let v = read.upgrade()?;
                let v = v.read(cx);
                Some(crate::scrollbar::Metrics { offset: f32::from(v.scroll_y), max: f32::from(v.scroll_max), viewport: f32::from(v.view_height) })
            }),
            set: std::rc::Rc::new(move |offset, _: &mut Window, cx: &mut gpui::App| {
                write
                    .update(cx, |v, cx| {
                        v.scroll_y = px(offset);
                        cx.notify();
                    })
                    .ok();
            }),
        };
        let bar = crate::scrollbar::vertical(source, &self.bar, &ui, false).absolute().top_0().right_0().bottom_0();

        div()
            .id("text-view")
            .key_context("TextView")
            .track_focus(&self.focus)
            .relative()
            .w_full()
            .map(|d| match height {
                Some(h) => d.h(h),
                None => d.h_full(),
            })
            .cursor(gpui::CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let focused = focus.is_focused(window);
                        view.update(cx, |view, _| view.layout(bounds, focused, window))
                    },
                    move |bounds, frame, window, cx| {
                        let (entity, focus) = input;
                        window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
                        paint(&frame, &font, fg, cursor_color, preedit.as_deref(), window, cx);
                    },
                )
                .size_full(),
            )
            .child(bar)
            .children(find_bar)
            .children(goto_bar)
            .children(menu)
    }
}

fn paint(frame: &Frame, font: &Font, fg: Rgb, cursor: Rgb, preedit: Option<&str>, window: &mut Window, cx: &mut App) {
    let mask = ContentMask { bounds: frame.mask, ..Default::default() };
    window.with_content_mask(Some(mask), |window| {
        for (bounds, color) in &frame.quads {
            window.paint_quad(PaintQuad::new(*bounds).background(*color));
        }
        for (origin, text, runs) in &frame.texts {
            let line = window.text_system().shape_line(text.clone(), frame.font_size, runs);
            let _ = line.paint(*origin, frame.line, TextAlign::Left, None, window, cx);
        }
        if let Some(c) = frame.cursor {
            window.paint_quad(PaintQuad::new(c).background(hsla(cursor)));
            if let Some(text) = preedit {
                let run = TextRun { len: text.len(), font: font.clone(), color: hsla(fg), background_color: None, underline: None, strikethrough: None, letter_spacing: None };
                let line = window.text_system().shape_line(SharedString::from(text.to_string()), frame.font_size, &[run]);
                let _ = line.paint(c.origin, frame.line, TextAlign::Left, None, window, cx);
            }
        }
    });
    for (origin, text, color) in &frame.gutter_texts {
        let run = TextRun { len: text.len(), font: font.clone(), color: hsla(*color), background_color: None, underline: None, strikethrough: None, letter_spacing: None };
        let line = window.text_system().shape_line(text.clone(), frame.font_size, &[run]);
        let _ = line.paint(*origin, frame.line, TextAlign::Left, None, window, cx);
    }
}

/// Input methods: composed text arrives through this. Offsets are UTF-16
/// code units over the whole text.
impl EntityInputHandler for TextView {
    fn text_for_range(&mut self, range: Range<usize>, adjusted: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let rope = self.buffer.rope();
        let total = rope.len_utf16_cu();
        let (a, b) = (range.start.min(total), range.end.min(total));
        *adjusted = Some(a..b);
        Some(rope.slice(rope.utf16_cu_to_char(a)..rope.utf16_cu_to_char(b)).to_string())
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let rope = self.buffer.rope();
        let sel = self.buffer.selection();
        let r = sel.range();
        Some(UTF16Selection { range: rope.char_to_utf16_cu(r.start)..rope.char_to_utf16_cu(r.end), reversed: sel.head < sel.anchor })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.preedit = None;
        cx.notify();
    }

    fn replace_text_in_range(&mut self, range: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.preedit = None;
        if self.options.read_only {
            return;
        }
        if range.is_some() || !text.is_empty() {
            self.block = None;
        }
        if let Some(range) = range {
            let rope = self.buffer.rope();
            let (a, b) = (rope.utf16_cu_to_char(range.start.min(rope.len_utf16_cu())), rope.utf16_cu_to_char(range.end.min(rope.len_utf16_cu())));
            self.buffer.set_selection(Selection { anchor: a, head: b });
        }
        if !text.is_empty() {
            self.buffer.insert(text);
            self.edited(cx);
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        self.preedit = (!text.is_empty()).then(|| text.to_string());
        cx.notify();
    }

    fn paste(&mut self, item: ClipboardItem, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = item.text() {
            self.paste_text(&text, cx);
        }
    }

    fn bounds_for_range(&mut self, _: Range<usize>, _: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let g = self.geometry.get()?;
        let head = self.buffer.selection().head;
        let (r, row) = self.row_of(head);
        let x = g.m.cell * self.row_x(row, self.buffer.line_col(head).1) as f32;
        Some(Bounds::new(point(g.origin.x + x - self.scroll_x, g.origin.y + g.m.line * r as f32 - self.scroll_y), size(g.m.cell, g.m.line)))
    }

    fn character_index_for_point(&mut self, at: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        let pos = self.pos_at(at);
        Some(self.buffer.rope().char_to_utf16_cu(pos))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_lines_wrap_after_a_space() {
        let b = Buffer::new("aaaa bbbb cccc\nshort");
        let rows = wrap(&b, 10);
        assert_eq!(rows, [Row { line: 0, start: 0, end: 10 }, Row { line: 0, start: 10, end: 14 }, Row { line: 1, start: 0, end: 5 }]);
        let rows = wrap(&Buffer::new("abcdefghijkl"), 5);
        assert_eq!(rows.iter().map(|r| (r.start, r.end)).collect::<Vec<_>>(), [(0, 5), (5, 10), (10, 12)]);
    }

    #[test]
    fn an_alt_drag_copies_its_rectangle() {
        // Dragged up and to the left: the same rectangle.
        let b = Block { anchor: (2, 9), head: (0, 4) };
        assert_eq!((b.lines(), b.cols()), (0..3, 4..9));
        assert!(b.spans() && !Block { anchor: (0, 4), head: (2, 4) }.spans());
        let text = Buffer::new("let a = 1;\nx\n\tlet bb = 22;");
        assert_eq!(text.block_text(b.lines(), b.cols()), "a = 1\n\nlet b");
    }

    #[test]
    fn runs_follow_the_colours_through_tabs() {
        let font = gpui::font("Mono");
        let (red, fg) = (Rgb::hex(0xff0000), Rgb::hex(0xffffff));
        let line = "\tlet x";
        let (drawn, starts) = expand_tabs(line, 4);
        let spans = vec![(1..4, red)];
        let runs = runs_for(line, Row { line: 0, start: 0, end: 6 }, &drawn, &starts, Some(&spans), fg, &font);
        assert_eq!(runs.iter().map(|r| r.len).collect::<Vec<_>>(), [4, 3, 2]);
        assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), drawn.len());
    }
}
