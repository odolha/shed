use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::term::{self, ClipboardType, Term, TermMode};
use alacritty_terminal::vte::ansi::{self, Color, NamedColor, Processor, StdSyncHandler};
use shed_model::Rgb;

use crate::palette::dim;
use crate::{
    CellFlags, CellView, CursorShape, CursorView, EmuEvent, Emulator, GridPoint, GridSize, Modes,
    MouseMode, Palette, SelectionKind, Snapshot,
};

mod replay;

/// Scrollback kept per pane, in lines.
const SCROLLBACK: usize = 10_000;

/// [`Emulator`] backed by `alacritty_terminal`.
pub struct AlacrittyEmulator {
    term: Term<Listener>,
    processor: Processor<StdSyncHandler>,
    events: Arc<Mutex<Vec<Event>>>,
    size: GridSize,
    /// Answers colour queries (OSC 4/10/11) when the app has not set the colour itself.
    palette: Palette,
    search: Option<Search>,
    /// A sequence the parser has begun but not finished, for replays.
    tail: crate::tail::Tail,
}

/// A scrollback search in progress.
struct Search {
    /// Behind a `RefCell` because matching needs it mutable while a snapshot
    /// only borrows the emulator.
    regex: RefCell<RegexSearch>,
    /// The match last gone to.
    focused: Option<Match>,
}

/// `text` as a regex that matches it literally.
fn literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `a` blended `t` of the way towards `b`.
fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Rgb { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b) }
}

#[derive(Clone)]
struct Listener(Arc<Mutex<Vec<Event>>>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        // Wakeups and cursor-blink changes say nothing a snapshot doesn't.
        if matches!(event, Event::Wakeup | Event::MouseCursorDirty | Event::CursorBlinkingChange) {
            return;
        }
        self.0.lock().unwrap().push(event);
    }
}

struct Dims(GridSize);

impl Dimensions for Dims {
    fn total_lines(&self) -> usize {
        self.0.rows as usize
    }

    fn screen_lines(&self) -> usize {
        self.0.rows as usize
    }

    fn columns(&self) -> usize {
        self.0.cols as usize
    }
}

impl AlacrittyEmulator {
    pub fn new(size: GridSize, palette: Palette) -> Self {
        let events = Arc::new(Mutex::new(Vec::new()));
        let config = term::Config { scrolling_history: SCROLLBACK, kitty_keyboard: true, ..Default::default() };
        let term = Term::new(config, &Dims(size), Listener(events.clone()));
        Self { term, processor: Processor::new(), events, size, palette, search: None, tail: Default::default() }
    }

    fn resolve(&self, color: Color, palette: &Palette) -> Rgb {
        let overrides = self.term.colors();
        match color {
            Color::Spec(c) => Rgb { r: c.r, g: c.g, b: c.b },
            Color::Indexed(i) => match overrides[i as usize] {
                Some(c) => Rgb { r: c.r, g: c.g, b: c.b },
                None => palette.indexed(i),
            },
            Color::Named(name) => match overrides[name] {
                Some(c) => Rgb { r: c.r, g: c.g, b: c.b },
                None => named(name, palette),
            },
        }
    }

    /// The colour reported back to apps that query colour `index`.
    fn query_color(&self, index: usize) -> Rgb {
        if let Some(c) = self.term.colors()[index] {
            return Rgb { r: c.r, g: c.g, b: c.b };
        }
        match index {
            0..256 => self.palette.indexed(index as u8),
            256 => self.palette.foreground,
            257 => self.palette.background,
            258 => self.palette.cursor,
            _ => self.palette.foreground,
        }
    }
}

fn named(name: NamedColor, p: &Palette) -> Rgb {
    let idx = name as usize;
    match name {
        NamedColor::Foreground | NamedColor::BrightForeground => p.foreground,
        NamedColor::Background => p.background,
        NamedColor::Cursor => p.cursor,
        NamedColor::DimForeground => dim(p.foreground),
        _ if idx < 16 => p.ansi[idx],
        // DimBlack..=DimWhite follow Cursor.
        _ => dim(p.ansi[idx - NamedColor::DimBlack as usize]),
    }
}

impl Emulator for AlacrittyEmulator {
    fn advance(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
        self.tail.advance(bytes);
    }

    fn take_replies(&mut self) -> Vec<u8> {
        let mut events = self.events.lock().unwrap();
        let mut out = Vec::new();
        events.retain(|event| match event {
            Event::PtyWrite(text) => {
                out.extend_from_slice(text.as_bytes());
                false
            }
            Event::ColorRequest(index, format) => {
                let c = self.query_color(*index);
                out.extend_from_slice(format(ansi::Rgb { r: c.r, g: c.g, b: c.b }).as_bytes());
                false
            }
            Event::TextAreaSizeRequest(format) => {
                let s = self.size;
                let size = WindowSize {
                    num_lines: s.rows,
                    num_cols: s.cols,
                    cell_width: s.cell_width,
                    cell_height: s.cell_height,
                };
                out.extend_from_slice(format(size).as_bytes());
                false
            }
            // Reading the clipboard over OSC 52 would let any program on a
            // remote host read it; Shed never answers.
            Event::ClipboardLoad(..) => false,
            _ => true,
        });
        out
    }

    fn take_events(&mut self) -> Vec<EmuEvent> {
        let mut events = self.events.lock().unwrap();
        let mut out = Vec::new();
        events.retain(|event| {
            let mapped = match event {
                Event::Title(t) => EmuEvent::Title(Some(t.clone())),
                Event::ResetTitle => EmuEvent::Title(None),
                Event::Bell => EmuEvent::Bell,
                Event::ClipboardStore(ty, text) => EmuEvent::ClipboardStore {
                    primary: matches!(ty, ClipboardType::Selection),
                    text: text.clone(),
                },
                // Replies stay for `take_replies`.
                Event::PtyWrite(_)
                | Event::ColorRequest(..)
                | Event::TextAreaSizeRequest(_)
                | Event::ClipboardLoad(..) => return true,
                _ => return false,
            };
            out.push(mapped);
            false
        });
        out
    }

    fn resize(&mut self, size: GridSize) {
        let size = GridSize::new(size.cols, size.rows).with_cell(size.cell_width, size.cell_height);
        if (size.cols, size.rows) != (self.size.cols, self.size.rows) {
            self.term.resize(Dims(size));
        }
        self.size = size;
    }

    fn size(&self) -> GridSize {
        self.size
    }

    fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    fn modes(&self) -> Modes {
        let m = *self.term.mode();
        let mouse = if m.contains(TermMode::MOUSE_MOTION) {
            MouseMode::Motion
        } else if m.contains(TermMode::MOUSE_DRAG) {
            MouseMode::Drag
        } else if m.contains(TermMode::MOUSE_REPORT_CLICK) {
            MouseMode::Click
        } else {
            MouseMode::Off
        };
        Modes {
            app_cursor: m.contains(TermMode::APP_CURSOR),
            app_keypad: m.contains(TermMode::APP_KEYPAD),
            bracketed_paste: m.contains(TermMode::BRACKETED_PASTE),
            alt_screen: m.contains(TermMode::ALT_SCREEN),
            alternate_scroll: m.contains(TermMode::ALTERNATE_SCROLL),
            focus_reporting: m.contains(TermMode::FOCUS_IN_OUT),
            mouse,
            sgr_mouse: m.contains(TermMode::SGR_MOUSE),
            utf8_mouse: m.contains(TermMode::UTF8_MOUSE),
            kitty: [
                (TermMode::DISAMBIGUATE_ESC_CODES, 1),
                (TermMode::REPORT_EVENT_TYPES, 2),
                (TermMode::REPORT_ALTERNATE_KEYS, 4),
                (TermMode::REPORT_ALL_KEYS_AS_ESC, 8),
                (TermMode::REPORT_ASSOCIATED_TEXT, 16),
            ]
            .iter()
            .filter(|(flag, _)| m.contains(*flag))
            .map(|(_, bit)| bit)
            .sum(),
        }
    }

    fn scroll_display(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
    }

    fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    fn tick(&mut self) -> Option<Instant> {
        let deadline = self.processor.sync_timeout().sync_timeout()?;
        if Instant::now() >= deadline {
            self.processor.stop_sync(&mut self.term);
            return None;
        }
        Some(deadline)
    }

    fn snapshot(&self, palette: &Palette, out: &mut Snapshot) {
        let grid = self.term.grid();
        let (rows, cols) = (grid.screen_lines(), grid.columns());
        let offset = grid.display_offset() as i32;
        out.rows = rows;
        out.cols = cols;
        out.display_offset = offset as usize;
        out.background = self.resolve(Color::Named(NamedColor::Background), palette);
        out.cells.clear();
        out.cells.reserve(rows * cols);
        let default_bg = out.background;
        let selection = self.term.selection.as_ref().and_then(|s| s.to_range(&self.term));
        let (matches, focused) = match &self.search {
            Some(search) => {
                // From a few lines above, for matches that start off screen.
                let start = Point::new(Line(-offset - 5).max(self.term.topmost_line()), Column(0));
                let end = Point::new(Line(rows as i32 - 1 - offset), Column(cols - 1));
                let mut regex = search.regex.borrow_mut();
                let found: Vec<Match> = RegexIter::new(start, end, Direction::Right, &self.term, &mut regex).collect();
                (found, search.focused.clone())
            }
            None => (Vec::new(), None),
        };
        let found_bg = mix(palette.ansi[3], default_bg, 0.55);

        for row in 0..rows {
            let line = &grid[Line(row as i32 - offset)];
            for col in 0..cols {
                let cell = &line[Column(col)];
                let f = cell.flags;
                let mut fg = self.resolve(cell.fg, palette);
                let mut bg = self.resolve(cell.bg, palette);
                if f.contains(Flags::DIM) {
                    fg = dim(fg);
                }
                if f.contains(Flags::INVERSE) {
                    std::mem::swap(&mut fg, &mut bg);
                }
                if f.contains(Flags::HIDDEN) {
                    fg = bg;
                }
                let mut flags = CellFlags {
                    bold: f.contains(Flags::BOLD),
                    italic: f.contains(Flags::ITALIC),
                    underline: f.intersects(
                        Flags::UNDERLINE | Flags::DOTTED_UNDERLINE | Flags::DASHED_UNDERLINE,
                    ),
                    double_underline: f.contains(Flags::DOUBLE_UNDERLINE),
                    undercurl: f.contains(Flags::UNDERCURL),
                    strikeout: f.contains(Flags::STRIKEOUT),
                    wide: f.contains(Flags::WIDE_CHAR),
                    spacer: f.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
                    has_bg: bg != default_bg,
                    selected: selection.is_some_and(|r| r.contains(Point::new(Line(row as i32 - offset), Column(col)))),
                };
                if !matches.is_empty() {
                    let p = Point::new(Line(row as i32 - offset), Column(col));
                    if focused.as_ref().is_some_and(|m| m.contains(&p)) {
                        (fg, bg) = (default_bg, palette.ansi[3]);
                        flags.has_bg = true;
                    } else if matches.iter().any(|m| m.contains(&p)) {
                        bg = found_bg;
                        flags.has_bg = true;
                    }
                }
                if flags.selected {
                    bg = palette.selection;
                    flags.has_bg = true;
                }
                out.cells.push(CellView {
                    ch: visible_char(cell.c),
                    zerowidth: cell.zerowidth().map(|z| z.iter().collect::<String>().into()),
                    fg,
                    bg,
                    flags,
                    underline_color: cell.underline_color().map(|c| self.resolve(c, palette)),
                });
            }
        }

        let content = self.term.renderable_content();
        let point = content.cursor.point;
        let row = point.line.0 + offset;
        out.cursor = if row < 0 || row as usize >= rows {
            CursorView { shape: CursorShape::Hidden, ..Default::default() }
        } else {
            let row = row as usize;
            let col = point.column.0.min(cols - 1);
            CursorView {
                row,
                col,
                shape: match content.cursor.shape {
                    ansi::CursorShape::Block => CursorShape::Block,
                    ansi::CursorShape::Underline => CursorShape::Underline,
                    ansi::CursorShape::Beam => CursorShape::Beam,
                    ansi::CursorShape::HollowBlock => CursorShape::HollowBlock,
                    ansi::CursorShape::Hidden => CursorShape::Hidden,
                },
                wide: out.cells[row * cols + col].flags.wide,
            }
        };
    }

    fn replay(&mut self) -> Vec<u8> {
        // Bytes held back by a synchronized update would never reach the
        // new emulator; apply them now.
        if self.processor.sync_timeout().sync_timeout().is_some() {
            self.processor.stop_sync(&mut self.term);
        }
        let mut bytes = replay::serialize(&self.term);
        bytes.extend_from_slice(self.tail.pending());
        bytes
    }

    fn start_selection(&mut self, kind: SelectionKind, at: GridPoint) {
        let ty = match kind {
            SelectionKind::Simple => SelectionType::Simple,
            SelectionKind::Word => SelectionType::Semantic,
            SelectionKind::Line => SelectionType::Lines,
            SelectionKind::Block => SelectionType::Block,
        };
        let (point, side) = self.term_point(at);
        self.term.selection = Some(Selection::new(ty, point, side));
    }

    fn update_selection(&mut self, to: GridPoint) {
        let (point, side) = self.term_point(to);
        if let Some(sel) = &mut self.term.selection {
            sel.update(point, side);
        }
    }

    fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    fn selection_text(&self) -> Option<String> {
        let sel = self.term.selection.as_ref()?;
        if sel.is_empty() {
            return None;
        }
        self.term.selection_to_string().filter(|s| !s.is_empty())
    }

    fn row_text(&self, row: usize) -> (String, Vec<usize>) {
        let grid = self.term.grid();
        let line = &grid[Line(row as i32 - grid.display_offset() as i32)];
        let mut text = String::new();
        let mut cols = Vec::new();
        for col in 0..grid.columns() {
            let cell = &line[Column(col)];
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            text.push(visible_char(cell.c));
            cols.push(col);
        }
        (text, cols)
    }

    fn hyperlink_at(&self, col: usize, row: usize) -> Option<String> {
        let grid = self.term.grid();
        let line = Line(row as i32 - grid.display_offset() as i32);
        let col = col.min(grid.columns() - 1);
        grid[line][Column(col)].hyperlink().map(|h| h.uri().to_string())
    }

    fn scrollback_text(&self, max_lines: usize) -> String {
        let grid = if self.term.mode().contains(TermMode::ALT_SCREEN) { self.term.inactive_grid() } else { self.term.grid() };
        let cols = grid.columns();
        let mut lines: Vec<String> = Vec::new();
        let mut current = String::new();
        for line in -(grid.history_size() as i32)..grid.screen_lines() as i32 {
            let row = &grid[Line(line)];
            for col in 0..cols {
                let cell = &row[Column(col)];
                if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                    continue;
                }
                current.push(visible_char(cell.c));
                if let Some(z) = cell.zerowidth() {
                    current.extend(z);
                }
            }
            if !row[Column(cols - 1)].flags.contains(Flags::WRAPLINE) {
                lines.push(current.trim_end().to_string());
                current.clear();
            }
        }
        if !current.is_empty() {
            lines.push(current.trim_end().to_string());
        }
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        let start = lines.len().saturating_sub(max_lines);
        lines[start..].join("\n")
    }

    fn dump_text(&self) -> String {
        let grid = self.term.grid();
        let offset = grid.display_offset() as i32;
        let mut out = String::new();
        for row in 0..grid.screen_lines() {
            let line = &grid[Line(row as i32 - offset)];
            let mut text = String::new();
            for col in 0..grid.columns() {
                let cell = &line[Column(col)];
                if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                    continue;
                }
                text.push(visible_char(cell.c));
                if let Some(z) = cell.zerowidth() {
                    text.extend(z);
                }
            }
            out.push_str(text.trim_end());
            out.push('\n');
        }
        out
    }

    fn set_search(&mut self, query: Option<&str>) -> bool {
        let regex = query.filter(|q| !q.is_empty()).and_then(|q| RegexSearch::new(&literal(q)).ok());
        self.search = regex.map(|regex| Search { regex: RefCell::new(regex), focused: None });
        self.search_step(true)
    }

    fn search_step(&mut self, older: bool) -> bool {
        let Some(search) = &mut self.search else { return false };
        let (direction, origin) = match (&search.focused, older) {
            (Some(m), true) => (Direction::Left, m.start().sub(&self.term, Boundary::None, 1)),
            (Some(m), false) => (Direction::Right, m.end().add(&self.term, Boundary::None, 1)),
            // The newest match first: from the end of the screen, upwards.
            (None, _) => (Direction::Left, Point::new(self.term.bottommost_line(), self.term.last_column())),
        };
        let found = self.term.search_next(search.regex.get_mut(), origin, direction, Side::Left, None);
        search.focused = found.clone();
        match found {
            Some(m) => {
                self.term.scroll_to_point(*m.start());
                true
            }
            None => false,
        }
    }
}

impl AlacrittyEmulator {
    fn term_point(&self, at: GridPoint) -> (Point, Side) {
        let grid = self.term.grid();
        let row = at.row.min(grid.screen_lines() - 1);
        let col = at.col.min(grid.columns() - 1);
        let point = Point::new(Line(row as i32 - grid.display_offset() as i32), Column(col));
        (point, if at.right_half { Side::Right } else { Side::Left })
    }
}

/// The core keeps a tab's `\t` in its first cell so copied text keeps the tab;
/// on screen it is blank.
fn visible_char(c: char) -> char {
    if c == '\t' { ' ' } else { c }
}

impl GridSize {
    pub fn with_cell(mut self, width: u16, height: u16) -> Self {
        self.cell_width = width;
        self.cell_height = height;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emu(cols: u16, rows: u16) -> AlacrittyEmulator {
        AlacrittyEmulator::new(GridSize::new(cols, rows), Palette::default())
    }

    /// The line the focused match starts on (negative is scrollback).
    fn focused_line(e: &AlacrittyEmulator) -> Option<i32> {
        e.search.as_ref()?.focused.as_ref().map(|m| m.start().line.0)
    }

    #[test]
    fn search_goes_newest_first_wraps_and_scrolls_matches_into_view() {
        let mut e = emu(20, 3);
        // Lines -3..=2 once scrolled: "apple" at -3, "Apple pie" at -1.
        e.advance(b"apple\r\nbanana\r\nApple pie\r\nx\r\ny\r\nz");
        assert!(e.set_search(Some("apple")));
        assert_eq!(focused_line(&e), Some(-1), "newest first");
        assert_eq!(e.display_offset(), 1, "scrolled to it");
        assert!(e.search_step(true));
        assert_eq!(focused_line(&e), Some(-3));
        assert_eq!(e.display_offset(), 3);
        assert!(e.search_step(true));
        assert_eq!(focused_line(&e), Some(-1), "wrapped around");
        assert!(e.search_step(false));
        assert_eq!(focused_line(&e), Some(-3), "and back the other way");

        // Capitals make it exact; specials are plain text.
        assert!(e.set_search(Some("Apple")));
        assert_eq!(focused_line(&e), Some(-1));
        assert!(!e.set_search(Some("a.ple")));
        assert!(!e.set_search(Some("")));
        assert!(e.search.is_none());
    }

    #[test]
    fn kitty_keyboard_flags_are_tracked_answered_and_replayed() {
        let mut e = emu(20, 3);
        e.advance(b"\x1b[>1u\x1b[>9u\x1b[?u");
        assert_eq!(e.modes().kitty, 9);
        assert_eq!(e.take_replies(), b"\x1b[?9u");
        let mut copy = emu(20, 3);
        copy.advance(&e.replay());
        assert_eq!(copy.modes().kitty, 9);
        e.advance(b"\x1b[<u");
        assert_eq!(e.modes().kitty, 1, "popping restores the one before");
    }

    #[test]
    fn search_matches_are_highlighted() {
        let mut e = emu(20, 2);
        e.advance(b"one two one\r\n");
        e.set_search(Some("one"));
        let palette = Palette::default();
        let mut snap = Snapshot::default();
        e.snapshot(&palette, &mut snap);
        let bg = |col: usize| snap.cells[col].bg;
        // The newest "one" is focused, the other only marked.
        assert_eq!(bg(8), palette.ansi[3]);
        assert!(snap.cells[0].flags.has_bg && bg(0) != palette.ansi[3]);
        assert!(!snap.cells[4].flags.has_bg);
    }

    #[test]
    fn plain_text_and_wrapping() {
        let mut e = emu(10, 3);
        e.advance(b"hello\r\nwrapping-line");
        insta::assert_snapshot!(e.dump_text(), @r"
        hello
        wrapping-l
        ine
        ");
    }

    #[test]
    fn colors_resolve_through_palette() {
        let mut e = emu(10, 2);
        e.advance(b"\x1b[31mR\x1b[38;5;196mX\x1b[38;2;1;2;3mT\x1b[7mI");
        let p = Palette::default();
        let mut s = Snapshot::default();
        e.snapshot(&p, &mut s);
        let row = s.row(0);
        assert_eq!(row[0].fg, p.ansi[1]);
        assert_eq!(row[1].fg, Rgb::hex(0xFF0000));
        assert_eq!(row[2].fg, Rgb::hex(0x010203));
        // Inverse swaps: background takes the truecolor foreground.
        assert_eq!(row[3].bg, Rgb::hex(0x010203));
        assert!(row[3].flags.has_bg);
        assert_eq!(s.cursor.col, 4);
    }

    #[test]
    fn selection_words_lines_and_scrollback() {
        let mut e = emu(20, 3);
        e.advance(b"one two three\r\nsecond line");
        let at = |col, row| GridPoint { col, row, right_half: false };
        e.start_selection(SelectionKind::Word, at(5, 0));
        assert_eq!(e.selection_text().as_deref(), Some("two"));
        e.start_selection(SelectionKind::Line, at(2, 1));
        assert_eq!(e.selection_text().as_deref(), Some("second line\n"));
        e.start_selection(SelectionKind::Simple, at(0, 0));
        e.update_selection(GridPoint { col: 2, row: 1, right_half: true });
        assert_eq!(e.selection_text().as_deref(), Some("one two three\nsec"));
        let mut s = Snapshot::default();
        e.snapshot(&Palette::default(), &mut s);
        assert!(s.row(0)[0].flags.selected && !s.row(1)[3].flags.selected);
        e.clear_selection();
        assert_eq!(e.selection_text(), None);
    }

    #[test]
    fn hyperlinks_and_row_text() {
        let mut e = emu(20, 2);
        e.advance("日本 \x1b]8;;https://x.org\x1b\\link\x1b]8;;\x1b\\".as_bytes());
        let (text, cols) = e.row_text(0);
        assert!(text.starts_with("日本 link"));
        assert_eq!(&cols[..4], &[0, 2, 4, 5]);
        assert_eq!(e.hyperlink_at(6, 0).as_deref(), Some("https://x.org"));
        assert_eq!(e.hyperlink_at(0, 0), None);
    }

    #[test]
    fn scrollback_text_joins_wraps_and_keeps_the_screen_under_apps() {
        let mut e = emu(10, 3);
        e.advance(b"short\r\na line that wraps around\r\n\r\nlast\r\n");
        assert_eq!(e.scrollback_text(100), "short\na line that wraps around\n\nlast");
        assert_eq!(e.scrollback_text(2), "\nlast");
        e.advance(b"\x1b[?1049hFULL SCREEN APP");
        assert_eq!(e.scrollback_text(1), "last");
    }

    #[test]
    fn tabs_draw_as_blanks() {
        let mut e = emu(20, 1);
        e.advance(b"a\tb");
        assert_eq!(e.dump_text(), "a       b\n");
        let mut s = Snapshot::default();
        e.snapshot(&Palette::default(), &mut s);
        assert!(s.row(0).iter().all(|c| c.ch != '\t'));
    }

    #[test]
    fn wide_chars_and_combining() {
        let mut e = emu(10, 1);
        e.advance("日本e\u{301}".as_bytes());
        assert_eq!(e.dump_text(), "日本e\u{301}\n");
        let mut s = Snapshot::default();
        e.snapshot(&Palette::default(), &mut s);
        assert!(s.row(0)[0].flags.wide);
        assert!(s.row(0)[1].flags.spacer);
        assert_eq!(s.row(0)[4].zerowidth.as_deref(), Some("\u{301}"));
    }

    #[test]
    fn answers_device_and_cursor_queries() {
        let mut e = emu(20, 5);
        e.advance(b"ab\x1b[6n\x1b[c");
        let replies = String::from_utf8(e.take_replies()).unwrap();
        assert!(replies.starts_with("\x1b[1;3R"), "{replies:?}");
        assert!(replies.contains("\x1b[?6c"), "{replies:?}");
        assert!(e.take_replies().is_empty());
    }

    #[test]
    fn answers_background_color_query() {
        let mut e = emu(20, 5);
        e.advance(b"\x1b]11;?\x07");
        let replies = String::from_utf8(e.take_replies()).unwrap();
        assert!(replies.contains("rgb:1414/1717/1d1d"), "{replies:?}");
    }

    #[test]
    fn title_and_modes() {
        let mut e = emu(20, 5);
        e.advance(b"\x1b]2;nvim\x07\x1b[?1h\x1b[?2004h\x1b[?1049h\x1b[?1002h\x1b[?1006h");
        assert_eq!(e.take_events(), vec![EmuEvent::Title(Some("nvim".into()))]);
        let m = e.modes();
        assert!(m.app_cursor && m.bracketed_paste && m.alt_screen && m.sgr_mouse);
        assert_eq!(m.mouse, MouseMode::Drag);
    }

    #[test]
    fn scrollback_and_resize() {
        let mut e = emu(10, 3);
        for i in 0..10 {
            e.advance(format!("line{i}\r\n").as_bytes());
        }
        e.scroll_display(2);
        assert_eq!(e.display_offset(), 2);
        insta::assert_snapshot!(e.dump_text(), @r"
        line6
        line7
        line8
        ");
        e.scroll_to_bottom();
        e.resize(GridSize::new(5, 2));
        assert_eq!(e.size(), GridSize::new(5, 2));
        insta::assert_snapshot!(e.dump_text(), @r"
        line9
        ");
    }
}
