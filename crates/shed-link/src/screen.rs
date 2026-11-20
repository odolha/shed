//! Terminals as content: an emulator per watched pane (fed by the keeper's
//! replay, then its output), sent on as lines of styled text, and keys from
//! connectors encoded as the program expects them.
//!
//! Colours go as the program chose them: a palette index (`"0"`…`"255"`,
//! the first 16 being the theme's) or `#rrggbb`, and `None` for the
//! terminal's own foreground or background. Each connector colours them by
//! its theme. Inverse and dim are flags, not folded into colours.

use std::time::Instant;

use shed_api::calls::{InputItem, Key};
use shed_api::topics::{Cursor, Screen, ScreenLine, Span};
use shed_emu::keys::{self, KeyInput, Mods};
use shed_emu::{AlacrittyEmulator, Emulator, GridSize, Ink, Modes, MouseMode, Palette, StyledCell};

/// A watched pane's emulator, and what was last sent of it.
pub(crate) struct Term {
    pub emu: AlacrittyEmulator,
    pub title: Option<String>,
    /// Rows as last sent.
    sent: Vec<ScreenLine>,
    sent_alt: bool,
    sent_cursor: Cursor,
    sent_title: Option<String>,
    sent_history: usize,
    /// The next event is the whole screen.
    pub full: bool,
    /// Output came since the last event.
    pub dirty: bool,
    pub last_sent: Option<Instant>,
}

impl Term {
    /// An emulator at the keeper's state: its replay of the pane.
    pub fn from_replay(cols: u16, rows: u16, bytes: &[u8]) -> Term {
        let mut emu = AlacrittyEmulator::new(GridSize::new(cols, rows), Palette::default());
        emu.advance(bytes);
        let mut t = Term {
            emu,
            title: None,
            sent: Vec::new(),
            sent_alt: false,
            sent_cursor: Cursor::default(),
            sent_title: None,
            sent_history: 0,
            full: true,
            dirty: true,
            last_sent: None,
        };
        t.drain();
        t
    }

    pub fn output(&mut self, bytes: &[u8]) {
        self.emu.advance(bytes);
        self.drain();
        self.dirty = true;
    }

    /// Replies to queries are the keeper's to send; titles are kept.
    fn drain(&mut self) {
        let _ = self.emu.take_replies();
        for e in self.emu.take_events() {
            if let shed_emu::EmuEvent::Title(t) = e {
                self.title = t;
            }
        }
    }

    pub fn size(&self) -> (u16, u16) {
        let s = self.emu.size();
        (s.cols, s.rows)
    }

    pub fn modes(&self) -> Modes {
        self.emu.modes()
    }

    /// The event to send now: the whole screen when `full`, else the rows
    /// that changed. `None` when nothing did.
    pub fn event(&mut self) -> Option<Screen> {
        let (cols, rows) = self.size();
        let modes = self.emu.modes();
        let alt = modes.alt_screen;
        let full = self.full || alt != self.sent_alt || self.sent.len() != rows as usize;
        let mut cells = Vec::new();
        let now: Vec<ScreenLine> = (0..rows as i32)
            .map(|r| {
                let wraps = self.emu.styled_line(r, &mut cells).unwrap_or(false);
                ScreenLine { spans: spans(&cells), wraps }
            })
            .collect();
        let cursor = match self.emu.cursor_cell() {
            Some((row, col)) => Cursor { row: row as u32, col: col as u32, visible: true },
            None => Cursor::default(),
        };
        let history = self.emu.main_history_size();
        let scrolled = if full || alt { 0 } else { scrolled(&self.sent, &now, self.sent_history, history) };
        let lines: Vec<shed_api::topics::ScreenRow> = now
            .iter()
            .enumerate()
            .filter(|(i, line)| if full { !line.spans.is_empty() || line.wraps } else { self.sent.get(*i) != Some(*line) })
            .map(|(i, line)| shed_api::topics::ScreenRow { row: i as u32, line: line.clone() })
            .collect();
        let same = !full && lines.is_empty() && cursor == self.sent_cursor && self.title == self.sent_title && history == self.sent_history;
        self.dirty = false;
        if same {
            return None;
        }
        self.full = false;
        self.sent = now;
        self.sent_alt = alt;
        self.sent_cursor = cursor;
        self.sent_title = self.title.clone();
        self.sent_history = history;
        self.last_sent = Some(Instant::now());
        Some(Screen {
            full,
            cols: cols as u32,
            rows: rows as u32,
            alt,
            lines,
            cursor,
            title: self.title.clone(),
            scrollback: history as u32,
            mouse: modes.mouse != MouseMode::Off,
            scrolled,
        })
    }

    /// Lines of scrollback, oldest first: `from` back from the screen's top
    /// (0: the line just above it), `count` of them.
    pub fn scrollback(&self, from: u32, count: u32) -> Vec<ScreenLine> {
        let history = self.emu.main_history_size() as i64;
        let newest = -(from as i64) - 1;
        let oldest = (newest - count as i64 + 1).max(-history);
        let mut cells = Vec::new();
        (oldest..=newest)
            .filter_map(|l| {
                let wraps = self.emu.styled_line(l as i32, &mut cells)?;
                Some(ScreenLine { spans: spans(&cells), wraps })
            })
            .collect()
    }
}

/// How many lines went off the top between two screens. While scrollback
/// grows, by how much it grew; once it is full (it stops growing), by where
/// the old screen's rows are found in the new one.
fn scrolled(old: &[ScreenLine], new: &[ScreenLine], old_history: usize, history: usize) -> u32 {
    if history > old_history {
        return (history - old_history) as u32;
    }
    if history < shed_emu::SCROLLBACK_LINES || old.len() != new.len() || old == new {
        return 0;
    }
    // Shifted up by k: old[k..] is new[..len - k] (the first one that fits,
    // where something with text is on it).
    let len = old.len();
    (1..len)
        .find(|&k| old[k..] == new[..len - k] && old[k..].iter().any(|l| !l.spans.is_empty()))
        .map_or(0, |k| k as u32)
}

/// Text with one style: the row's cells grouped, trailing blanks left off.
pub(crate) fn spans(cells: &[StyledCell]) -> Vec<Span> {
    let blank = |c: &StyledCell| (c.ch == ' ' || c.hidden) && c.bg == Ink::Default && !c.inverse && !c.underline && !c.strike;
    let end = cells.iter().rposition(|c| !blank(c) && !c.spacer).map_or(0, |i| i + 1);
    let mut out: Vec<Span> = Vec::new();
    for c in &cells[..end] {
        if c.spacer {
            continue;
        }
        let style = Span { text: String::new(), fg: ink(c.fg), bg: ink(c.bg), bold: c.bold, dim: c.dim, italic: c.italic, underline: c.underline, strike: c.strike, inverse: c.inverse };
        let same = out.last().is_some_and(|l| {
            (&l.fg, &l.bg, l.bold, l.dim, l.italic, l.underline, l.strike, l.inverse)
                == (&style.fg, &style.bg, style.bold, style.dim, style.italic, style.underline, style.strike, style.inverse)
        });
        if !same {
            out.push(style);
        }
        let last = out.last_mut().expect("a span was just pushed");
        last.text.push(if c.hidden { ' ' } else { c.ch });
        if let Some(z) = c.zerowidth.as_deref().filter(|_| !c.hidden) {
            last.text.push_str(z);
        }
    }
    out
}

fn ink(i: Ink) -> Option<String> {
    match i {
        Ink::Default => None,
        Ink::Indexed(n) => Some(n.to_string()),
        Ink::Rgb(c) => Some(crate::state::hex(c)),
    }
}

/// The bytes for what a connector typed, encoded for the program's modes.
pub(crate) fn encode(items: &[InputItem], modes: Modes) -> Vec<u8> {
    let mut out = Vec::new();
    for item in items {
        match item {
            InputItem::Text(text) => {
                for ch in text.chars() {
                    let name = match ch {
                        '\r' | '\n' => Some("enter"),
                        '\t' => Some("tab"),
                        '\x7f' | '\x08' => Some("backspace"),
                        '\x1b' => Some("escape"),
                        _ => None,
                    };
                    let mut buf = [0u8; 4];
                    let s: &str = ch.encode_utf8(&mut buf);
                    let bytes = match name {
                        Some(key) => keys::encode(KeyInput { key, text: None, mods: Mods::NONE }, modes),
                        // Other control characters go as they are (Ctrl+C typed as text).
                        None if ch.is_control() => Some(s.as_bytes().to_vec()),
                        None => keys::encode(KeyInput { key: s, text: Some(s), mods: Mods::NONE }, modes),
                    };
                    out.extend(bytes.unwrap_or_default());
                }
            }
            InputItem::Key(key) => out.extend(encode_key(key, modes).unwrap_or_default()),
            InputItem::Paste(text) => out.extend(keys::encode_paste(text, modes)),
        }
    }
    out
}

/// A named key or a character, with modifiers.
fn encode_key(key: &Key, modes: Modes) -> Option<Vec<u8>> {
    let mods = Mods { ctrl: key.ctrl, alt: key.alt, shift: key.shift, super_: false };
    let named = match key.key.to_ascii_lowercase().as_str() {
        "enter" | "return" => Some("enter"),
        "tab" => Some("tab"),
        "backspace" => Some("backspace"),
        "escape" | "esc" => Some("escape"),
        "space" | " " => Some("space"),
        "up" | "arrowup" => Some("up"),
        "down" | "arrowdown" => Some("down"),
        "left" | "arrowleft" => Some("left"),
        "right" | "arrowright" => Some("right"),
        "home" => Some("home"),
        "end" => Some("end"),
        "pageup" => Some("pageup"),
        "pagedown" => Some("pagedown"),
        "insert" => Some("insert"),
        "delete" => Some("delete"),
        _ => None,
    };
    if let Some(name) = named {
        let text = (name == "space" && !key.ctrl).then_some(" ");
        return keys::encode(KeyInput { key: name, text, mods }, modes);
    }
    let lower = key.key.to_ascii_lowercase();
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()).filter(|n| (1..=12).contains(n)) {
        let name = format!("f{n}");
        return keys::encode(KeyInput { key: &name, text: None, mods }, modes);
    }
    let mut chars = key.key.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else { return None };
    // As a keyboard reports it: the key unshifted, the text it types.
    let base: String = ch.to_lowercase().collect();
    let typed: String = if key.shift { ch.to_uppercase().collect() } else { ch.to_string() };
    keys::encode(KeyInput { key: &base, text: Some(&typed), mods }, modes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: &str, ctrl: bool, alt: bool, shift: bool) -> InputItem {
        InputItem::Key(Key { key: k.into(), ctrl, alt, shift })
    }

    #[test]
    fn keys_as_programs_expect_them() {
        let plain = Modes::default();
        assert_eq!(encode(&[InputItem::Text("echo hi\r".into())], plain), b"echo hi\r");
        assert_eq!(encode(&[key("c", true, false, false)], plain), b"\x03");
        assert_eq!(encode(&[key("Up", false, false, false)], plain), b"\x1b[A");
        let app = Modes { app_cursor: true, ..Default::default() };
        assert_eq!(encode(&[key("Up", false, false, false)], app), b"\x1bOA");
        assert_eq!(encode(&[key("Enter", false, false, false)], plain), b"\r");
        assert_eq!(encode(&[key("Tab", false, false, true)], plain), b"\x1b[Z");
        assert_eq!(encode(&[key("b", false, true, false)], plain), b"\x1bb");
        assert_eq!(encode(&[key("a", false, false, true)], plain), b"A");
        assert_eq!(encode(&[key("F5", false, false, false)], plain), b"\x1b[15~");
        assert_eq!(encode(&[key("PageDown", false, false, false)], plain), b"\x1b[6~");
        assert_eq!(encode(&[InputItem::Text("é\x03".into())], plain), "é\x03".as_bytes());
        let bracketed = Modes { bracketed_paste: true, ..Default::default() };
        assert_eq!(encode(&[InputItem::Paste("ls\n".into())], bracketed), b"\x1b[200~ls\r\x1b[201~");
        // The kitty protocol, once a program asks for it.
        let kitty = Modes { kitty: 1, ..Default::default() };
        assert_eq!(encode(&[key("Escape", false, false, false)], kitty), b"\x1b[27u");
        assert!(encode(&[key("NoSuchKey", false, false, false)], plain).is_empty());
    }

    #[test]
    fn screens_as_lines_of_styled_text() {
        let mut t = Term::from_replay(8, 3, b"\x1b[1;31mred\x1b[0m ok\r\nwrapping-on");
        let first = t.event().unwrap();
        assert!(first.full);
        assert_eq!((first.cols, first.rows), (8, 3));
        let row0 = &first.lines[0].line;
        assert_eq!(row0.spans[0].text, "red");
        assert_eq!(row0.spans[0].fg.as_deref(), Some("1"));
        assert!(row0.spans[0].bold);
        assert_eq!(row0.spans[1].text, " ok");
        assert!(row0.spans[1].fg.is_none());
        assert!(first.lines[1].line.wraps, "the second row goes on in the third");
        assert_eq!(first.cursor, Cursor { row: 2, col: 3, visible: true });
        assert!(t.event().is_none(), "nothing changed");
        // Only what changes is sent again.
        t.output(b"\r\nnew");
        let next = t.event().unwrap();
        assert!(!next.full);
        assert_eq!(next.scrolled, 1);
        assert_eq!(next.scrollback, 1);
        assert!(next.lines.iter().any(|r| r.line.spans.first().is_some_and(|s| s.text == "new")));
        assert_eq!(t.scrollback(0, 5).len(), 1);
        assert_eq!(t.scrollback(0, 5)[0].spans[0].text, "red");
        // A full-screen program: the whole screen again.
        t.output(b"\x1b[?1049h\x1b[?1000hvim");
        let alt = t.event().unwrap();
        assert!(alt.full && alt.alt && alt.mouse);
    }

    #[test]
    fn wide_characters_once() {
        let mut t = Term::from_replay(6, 1, "日本".as_bytes());
        let s = t.event().unwrap();
        assert_eq!(s.lines[0].line.spans[0].text, "日本");
    }
}
