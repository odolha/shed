//! Serializing a terminal's state as bytes that rebuild it in a fresh emulator.
//!
//! The keeper sends this to a window that attaches, so the window shows the
//! screen exactly as it is, scrollback included, before live output resumes.
//! The order matters: content is written with default modes (autowrap on,
//! full scroll region, ASCII charset), and only then are the pane's own
//! modes, region, cursor and pen restored.

use std::fmt::Write as _;

use alacritty_terminal::grid::{Dimensions, Grid};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags, Hyperlink};
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{
    CharsetIndex, Color, CursorShape, NamedColor, StandardCharset,
};

use super::Listener;

pub(super) fn serialize(term: &Term<Listener>) -> Vec<u8> {
    let mut out = String::with_capacity(64 * 1024);
    let alt = term.mode().contains(TermMode::ALT_SCREEN);
    let primary = if alt { term.inactive_grid() } else { term.grid() };

    // Colours the app redefined, so indexed cells resolve the same way.
    let colors = term.colors();
    for i in 0..259 {
        if let Some(c) = colors[i] {
            let rgb = format!("rgb:{:02x}/{:02x}/{:02x}", c.r, c.g, c.b);
            match i {
                0..256 => write!(out, "\x1b]4;{i};{rgb}\x1b\\").unwrap(),
                256 => write!(out, "\x1b]10;{rgb}\x1b\\").unwrap(),
                257 => write!(out, "\x1b]11;{rgb}\x1b\\").unwrap(),
                _ => write!(out, "\x1b]12;{rgb}\x1b\\").unwrap(),
            }
        }
    }
    if let Some(title) = term.title() {
        write!(out, "\x1b]2;{}\x1b\\", title.replace(['\x1b', '\x07'], "")).unwrap();
    }

    let mut pen = Pen::default();
    write_primary(&mut out, primary, &mut pen);

    if alt {
        // 1049 saves the primary cursor, as the app's own 1049h did.
        let saved = &primary.saved_cursor;
        place_cursor(&mut out, saved.point.line.0 as usize, saved.point.column.0);
        pen.set(&mut out, &saved.template);
        out.push_str("\x1b[?1049h");
        pen = Pen::default();
        write_screen(&mut out, term.grid(), &mut pen);
    }
    let grid = term.grid();

    // Tab stops, only if the app changed them.
    let cols = grid.columns();
    let default_tabs = (0..cols).all(|c| term.is_tab_stop(Column(c)) == (c % 8 == 0));
    if !default_tabs {
        out.push_str("\x1b[3g");
        for c in (0..cols).filter(|&c| term.is_tab_stop(Column(c))) {
            place_cursor(&mut out, 0, c);
            out.push_str("\x1bH");
        }
    }

    // DECSC state of the active screen.
    let saved = &grid.saved_cursor;
    if saved.point != Default::default() || saved.template != Cell::default() {
        place_cursor(&mut out, saved.point.line.0 as usize, saved.point.column.0);
        pen.set(&mut out, &saved.template);
        out.push_str("\x1b7");
    }

    let region = term.scroll_region();
    let full = region.start == Line(0) && region.end == Line(grid.screen_lines() as i32);
    if !full {
        write!(out, "\x1b[{};{}r", region.start.0 + 1, region.end.0).unwrap();
    }

    // The cursor. A cursor waiting to wrap is recreated by rewriting the last cell.
    let cursor = &grid.cursor;
    let (row, col) = (cursor.point.line.0 as usize, cursor.point.column.0);
    if cursor.input_needs_wrap {
        let cell = &grid[cursor.point.line][cursor.point.column];
        place_cursor(&mut out, row, col);
        pen.set(&mut out, cell);
        push_cell(&mut out, cell);
    } else {
        place_cursor(&mut out, row, col);
    }
    let mode = *term.mode();
    if mode.contains(TermMode::ORIGIN) {
        // DECOM homes the cursor to the region's top; position relative to it again.
        out.push_str("\x1b[?6h");
        write!(out, "\x1b[{};{}H", row as i32 - region.start.0 + 1, col + 1).unwrap();
    }
    pen.set(&mut out, &cursor.template);

    // Character sets, after all text so the content is not translated.
    for (index, designator) in [(CharsetIndex::G0, '('), (CharsetIndex::G1, ')'), (CharsetIndex::G2, '*'), (CharsetIndex::G3, '+')] {
        if cursor.charsets[index] == StandardCharset::SpecialCharacterAndLineDrawing {
            write!(out, "\x1b{designator}0").unwrap();
        }
    }
    match term.active_charset() {
        CharsetIndex::G1 => out.push('\x0e'),
        CharsetIndex::G2 => out.push_str("\x1bn"),
        CharsetIndex::G3 => out.push_str("\x1bo"),
        CharsetIndex::G0 => {}
    }

    write_modes(&mut out, mode);

    let style = term.cursor_style();
    if style != Default::default() {
        let n = match style.shape {
            CursorShape::Block | CursorShape::HollowBlock | CursorShape::Hidden => 1,
            CursorShape::Underline => 3,
            CursorShape::Beam => 5,
        } + u8::from(!style.blinking);
        write!(out, "\x1b[{n} q").unwrap();
    }

    out.into_bytes()
}

/// Writes scrollback and screen by printing them top to bottom: the newlines
/// scroll earlier lines into history exactly as many lines as the original had.
fn write_primary(out: &mut String, grid: &Grid<Cell>, pen: &mut Pen) {
    let top = -(grid.history_size() as i32);
    let bottom = grid.screen_lines() as i32 - 1;
    for line in top..=bottom {
        let row = &grid[Line(line)];
        write_row(out, row, grid.columns(), pen);
        if line == bottom {
            break;
        }
        let wraps = row[Column(grid.columns() - 1)].flags.contains(Flags::WRAPLINE);
        if !wraps {
            // Reset first, so the new line is not filled with the current background.
            pen.reset(out);
            out.push_str("\r\n");
        }
    }
}

/// Writes a screen without history (the alternate screen) row by row.
fn write_screen(out: &mut String, grid: &Grid<Cell>, pen: &mut Pen) {
    let cols = grid.columns();
    let mut wrapped = false;
    for line in 0..grid.screen_lines() {
        let row = &grid[Line(line as i32)];
        if !wrapped {
            pen.reset(out);
            place_cursor(out, line, 0);
        }
        write_row(out, row, cols, pen);
        wrapped = line + 1 < grid.screen_lines() && row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
    }
}

fn write_row(out: &mut String, row: &alacritty_terminal::grid::Row<Cell>, cols: usize, pen: &mut Pen) {
    let wraps = row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
    // A wrapped line must be written to its end so the next character wraps.
    let end = if wraps {
        cols
    } else {
        (0..cols).rev().find(|&c| !is_blank(&row[Column(c)])).map_or(0, |c| c + 1)
    };
    for c in 0..end {
        let cell = &row[Column(c)];
        if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        pen.set(out, cell);
        push_cell(out, cell);
    }
}

/// A cell exactly as a fresh screen has it, which can be left unwritten.
fn is_blank(cell: &Cell) -> bool {
    (cell.c == ' ' || cell.c == '\t')
        && cell.fg == Color::Named(NamedColor::Foreground)
        && cell.bg == Color::Named(NamedColor::Background)
        && (cell.flags - Flags::WRAPLINE).is_empty()
        && cell.zerowidth().is_none()
        && cell.underline_color().is_none()
        && cell.hyperlink().is_none()
}

fn push_cell(out: &mut String, cell: &Cell) {
    // A tab is kept in its first cell for copying; printing `\t` would move instead.
    out.push(if cell.c == '\t' || cell.c.is_control() { ' ' } else { cell.c });
    if let Some(z) = cell.zerowidth() {
        out.extend(z);
    }
}

fn place_cursor(out: &mut String, row: usize, col: usize) {
    write!(out, "\x1b[{};{}H", row + 1, col + 1).unwrap();
}

fn write_modes(out: &mut String, mode: TermMode) {
    let private = [
        (TermMode::APP_CURSOR, 1, false),
        (TermMode::LINE_WRAP, 7, true),
        (TermMode::SHOW_CURSOR, 25, true),
        (TermMode::MOUSE_REPORT_CLICK, 1000, false),
        (TermMode::MOUSE_DRAG, 1002, false),
        (TermMode::MOUSE_MOTION, 1003, false),
        (TermMode::FOCUS_IN_OUT, 1004, false),
        (TermMode::UTF8_MOUSE, 1005, false),
        (TermMode::SGR_MOUSE, 1006, false),
        (TermMode::ALTERNATE_SCROLL, 1007, true),
        (TermMode::URGENCY_HINTS, 1042, true),
        (TermMode::BRACKETED_PASTE, 2004, false),
    ];
    for (flag, number, default) in private {
        let on = mode.contains(flag);
        if on != default {
            write!(out, "\x1b[?{number}{}", if on { 'h' } else { 'l' }).unwrap();
        }
    }
    if mode.contains(TermMode::INSERT) {
        out.push_str("\x1b[4h");
    }
    if mode.contains(TermMode::LINE_FEED_NEW_LINE) {
        out.push_str("\x1b[20h");
    }
    if mode.contains(TermMode::APP_KEYPAD) {
        out.push_str("\x1b=");
    }
    let kitty = [
        (TermMode::DISAMBIGUATE_ESC_CODES, 1),
        (TermMode::REPORT_EVENT_TYPES, 2),
        (TermMode::REPORT_ALTERNATE_KEYS, 4),
        (TermMode::REPORT_ALL_KEYS_AS_ESC, 8),
        (TermMode::REPORT_ASSOCIATED_TEXT, 16),
    ]
    .iter()
    .filter(|(flag, _)| mode.contains(*flag))
    .map(|(_, bit)| bit)
    .sum::<u8>();
    if kitty != 0 {
        write!(out, "\x1b[>{kitty}u").unwrap();
    }
}

/// The SGR and hyperlink state last written, so only changes are emitted.
#[derive(Default)]
struct Pen {
    current: Option<(Color, Color, Flags, Option<Color>)>,
    link: Option<Hyperlink>,
}

impl Pen {
    fn reset(&mut self, out: &mut String) {
        if self.current.is_some_and(|c| c != Self::key(&Cell::default())) {
            out.push_str("\x1b[0m");
        }
        self.current = Some(Self::key(&Cell::default()));
        self.set_link(out, None);
    }

    fn key(cell: &Cell) -> (Color, Color, Flags, Option<Color>) {
        let attrs = Flags::BOLD | Flags::DIM | Flags::ITALIC | Flags::INVERSE | Flags::HIDDEN | Flags::STRIKEOUT | Flags::ALL_UNDERLINES;
        (cell.fg, cell.bg, cell.flags & attrs, cell.underline_color())
    }

    fn set(&mut self, out: &mut String, cell: &Cell) {
        let key = Self::key(cell);
        if self.current != Some(key) {
            let (fg, bg, flags, underline_color) = key;
            out.push_str("\x1b[0");
            for (flag, code) in [
                (Flags::BOLD, "1"),
                (Flags::DIM, "2"),
                (Flags::ITALIC, "3"),
                (Flags::UNDERLINE, "4"),
                (Flags::DOUBLE_UNDERLINE, "4:2"),
                (Flags::UNDERCURL, "4:3"),
                (Flags::DOTTED_UNDERLINE, "4:4"),
                (Flags::DASHED_UNDERLINE, "4:5"),
                (Flags::INVERSE, "7"),
                (Flags::HIDDEN, "8"),
                (Flags::STRIKEOUT, "9"),
            ] {
                if flags.contains(flag) {
                    out.push(';');
                    out.push_str(code);
                }
            }
            color(out, fg, 30, 90, 38, 39);
            color(out, bg, 40, 100, 48, 49);
            if let Some(c) = underline_color {
                color(out, c, 0, 0, 58, 59);
            }
            out.push('m');
            self.current = Some(key);
        }
        self.set_link(out, cell.hyperlink());
    }

    fn set_link(&mut self, out: &mut String, link: Option<Hyperlink>) {
        if self.link == link {
            return;
        }
        match &link {
            Some(l) => write!(out, "\x1b]8;id={};{}\x1b\\", l.id(), l.uri()).unwrap(),
            None => out.push_str("\x1b]8;;\x1b\\"),
        }
        self.link = link;
    }
}

/// Appends `;<sgr>` for a colour: base+n for the 8 colours, bright+n for
/// the bright ones, `ext;5;i` or `ext;2;r;g;b` otherwise, `default` for the default.
fn color(out: &mut String, c: Color, base: u8, bright: u8, ext: u8, default: u8) {
    match c {
        Color::Named(n) if (n as usize) < 8 && base != 0 => write!(out, ";{}", base + n as u8).unwrap(),
        Color::Named(n) if (n as usize) < 16 && base != 0 => write!(out, ";{}", bright + n as u8 - 8).unwrap(),
        Color::Named(n) if (n as usize) < 16 => write!(out, ";{ext};5;{}", n as u8).unwrap(),
        Color::Named(_) => write!(out, ";{default}").unwrap(),
        Color::Indexed(i) => write!(out, ";{ext};5;{i}").unwrap(),
        Color::Spec(rgb) => write!(out, ";{ext};2;{};{};{}", rgb.r, rgb.g, rgb.b).unwrap(),
    }
}
