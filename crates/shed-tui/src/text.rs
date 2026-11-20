//! Text helpers: wrapping styled text to a width, cutting it to fit, and an
//! editable line of input (which may hold line breaks, for prompts).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// `s` cut to `max` columns, with "…" where it was cut.
pub fn fit(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw > max - 1 {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// `s` padded with spaces (or cut) to exactly `cols` columns.
pub fn pad(s: &str, cols: usize) -> String {
    let s = fit(s, cols);
    let w = width(&s);
    format!("{s}{}", " ".repeat(cols.saturating_sub(w)))
}

/// A rectangle of `w` × `h` in the middle of `area` (shrunk to fit).
pub fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

/// Wraps styled text at word boundaries to lines of `cols` columns. The
/// first line starts with `first`, the others with `rest` (a hanging
/// indent); words wider than a line are broken. Line breaks in the text
/// start new lines.
pub fn wrap(spans: Vec<Span<'static>>, cols: usize, first: Vec<Span<'static>>, rest: Vec<Span<'static>>) -> Vec<Line<'static>> {
    let indent_w = |v: &[Span]| v.iter().map(|s| width(&s.content)).sum::<usize>();
    let mut lines = Vec::new();
    let mut cur: Vec<Span<'static>> = first.clone();
    let mut cur_w = indent_w(&first);
    let rest_w = indent_w(&rest);
    let mut fresh = true;
    let push = |cur: &mut Vec<Span<'static>>, text: &str, style: Style| {
        if let Some(last) = cur.last_mut()
            && last.style == style
        {
            last.content = format!("{}{text}", last.content).into();
            return;
        }
        cur.push(Span::styled(text.to_string(), style));
    };
    for span in spans {
        let style = span.style;
        for (i, piece) in span.content.split('\n').enumerate() {
            if i > 0 {
                lines.push(Line::from(std::mem::replace(&mut cur, rest.clone())));
                cur_w = rest_w;
                fresh = true;
            }
            // Words with the spaces before them.
            let mut words: Vec<&str> = Vec::new();
            let mut start = 0;
            let bytes = piece.as_bytes();
            for (j, c) in piece.char_indices() {
                if c == ' ' && j > start && bytes[j - 1] != b' ' {
                    words.push(&piece[start..j]);
                    start = j;
                }
            }
            if start < piece.len() {
                words.push(&piece[start..]);
            }
            for word in words {
                let w = width(word);
                if cur_w + w > cols && !fresh {
                    lines.push(Line::from(std::mem::replace(&mut cur, rest.clone())));
                    cur_w = rest_w;
                    let trimmed = word.trim_start_matches(' ');
                    place(&mut cur, &mut cur_w, trimmed, style, cols, rest_w, &rest, &mut lines, &push);
                } else {
                    place(&mut cur, &mut cur_w, word, style, cols, rest_w, &rest, &mut lines, &push);
                }
                fresh = false;
            }
        }
    }
    lines.push(Line::from(cur));
    // Spaces where a line was broken are not kept at its end.
    for line in &mut lines {
        loop {
            let count = line.spans.len();
            let Some(last) = line.spans.last_mut() else { break };
            let trimmed = last.content.trim_end_matches(' ').len();
            if trimmed == last.content.len() {
                break;
            }
            if trimmed == 0 && count > 1 {
                line.spans.pop();
                continue;
            }
            last.content = last.content[..trimmed].to_string().into();
            break;
        }
    }
    lines
}

/// Puts `word` on the current line, breaking it where it is wider than a line.
#[allow(clippy::too_many_arguments)]
fn place(
    cur: &mut Vec<Span<'static>>,
    cur_w: &mut usize,
    word: &str,
    style: Style,
    cols: usize,
    rest_w: usize,
    rest: &[Span<'static>],
    lines: &mut Vec<Line<'static>>,
    push: &impl Fn(&mut Vec<Span<'static>>, &str, Style),
) {
    if *cur_w + width(word) <= cols || cols <= rest_w + 1 {
        push(cur, word, style);
        *cur_w += width(word);
        return;
    }
    let mut chunk = String::new();
    for c in word.chars() {
        let cw = c.width().unwrap_or(0);
        if *cur_w + cw > cols {
            if !chunk.is_empty() {
                push(cur, &chunk, style);
            }
            chunk.clear();
            lines.push(Line::from(std::mem::replace(cur, rest.to_vec())));
            *cur_w = rest_w;
        }
        chunk.push(c);
        *cur_w += cw;
    }
    push(cur, &chunk, style);
}

/// What a key did to an input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    /// The text changed.
    Changed,
    /// Only the cursor moved.
    Moved,
    /// Not an editing key.
    Ignored,
}

/// An editable text with a cursor, as typed into a prompt or a field.
#[derive(Clone, Debug, Default)]
pub struct Input {
    pub text: String,
    /// A byte offset at a character boundary.
    pub cursor: usize,
}

impl Input {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        Self { cursor: text.len(), text }
    }

    pub fn set(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
    }

    pub fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    pub fn insert(&mut self, s: &str) {
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    fn prev(&self) -> usize {
        self.text[..self.cursor].char_indices().next_back().map_or(0, |(i, _)| i)
    }

    fn next(&self) -> usize {
        self.text[self.cursor..].chars().next().map_or(self.cursor, |c| self.cursor + c.len_utf8())
    }

    fn word_left(&self) -> usize {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end();
        trimmed.rfind(|c: char| c.is_whitespace()).map_or(0, |i| i + 1)
    }

    fn word_right(&self) -> usize {
        let after = &self.text[self.cursor..];
        let skip = after.len() - after.trim_start().len();
        let rest = &after[skip..];
        self.cursor + skip + rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len())
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..].find('\n').map_or(self.text.len(), |i| self.cursor + i)
    }

    /// Applies an editing key: typing, deleting, moving (with Ctrl and Alt
    /// as readline has them).
    pub fn key(&mut self, ev: &KeyEvent) -> Edit {
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        let alt = ev.modifiers.contains(KeyModifiers::ALT);
        let before = (self.text.len(), self.cursor);
        match ev.code {
            KeyCode::Char(c) if !ctrl && !alt => {
                let mut buf = [0; 4];
                self.insert(c.encode_utf8(&mut buf));
                return Edit::Changed;
            }
            KeyCode::Backspace if ctrl || alt => {
                let to = self.word_left();
                self.text.replace_range(to..self.cursor, "");
                self.cursor = to;
            }
            KeyCode::Char('w') if ctrl => {
                let to = self.word_left();
                self.text.replace_range(to..self.cursor, "");
                self.cursor = to;
            }
            KeyCode::Backspace | KeyCode::Char('h') if ev.code == KeyCode::Backspace || ctrl => {
                let to = self.prev();
                self.text.replace_range(to..self.cursor, "");
                self.cursor = to;
            }
            KeyCode::Delete | KeyCode::Char('d') if ev.code == KeyCode::Delete || ctrl => {
                let to = self.next();
                self.text.replace_range(self.cursor..to, "");
            }
            KeyCode::Char('u') if ctrl => {
                let from = self.line_start();
                self.text.replace_range(from..self.cursor, "");
                self.cursor = from;
            }
            KeyCode::Char('k') if ctrl => {
                let to = self.line_end();
                self.text.replace_range(self.cursor..to, "");
            }
            KeyCode::Left if ctrl || alt => self.cursor = self.word_left(),
            KeyCode::Right if ctrl || alt => self.cursor = self.word_right(),
            KeyCode::Char('b') if alt => self.cursor = self.word_left(),
            KeyCode::Char('f') if alt => self.cursor = self.word_right(),
            KeyCode::Left | KeyCode::Char('b') if ev.code == KeyCode::Left || ctrl => self.cursor = self.prev(),
            KeyCode::Right | KeyCode::Char('f') if ev.code == KeyCode::Right || ctrl => self.cursor = self.next(),
            KeyCode::Home | KeyCode::Char('a') if ev.code == KeyCode::Home || ctrl => self.cursor = self.line_start(),
            KeyCode::End | KeyCode::Char('e') if ev.code == KeyCode::End || ctrl => self.cursor = self.line_end(),
            _ => return Edit::Ignored,
        }
        if self.text.len() != before.0 {
            Edit::Changed
        } else if self.cursor != before.1 {
            Edit::Moved
        } else {
            // A key that does nothing here (Backspace at the start) is still taken.
            Edit::Moved
        }
    }

    /// The text broken into rows of `cols` columns (by line breaks, then by
    /// width), and the cursor's row and column among them.
    pub fn rows(&self, cols: usize) -> (Vec<String>, (usize, usize)) {
        let cols = cols.max(2);
        let mut rows = vec![String::new()];
        let mut w = 0;
        let mut cursor = (0, 0);
        for (i, c) in self.text.char_indices() {
            let cw = if c == '\n' { 0 } else { c.width().unwrap_or(0) };
            if c != '\n' && w + cw > cols {
                rows.push(String::new());
                w = 0;
            }
            if i == self.cursor {
                cursor = (rows.len() - 1, w);
            }
            if c == '\n' {
                rows.push(String::new());
                w = 0;
                continue;
            }
            rows.last_mut().unwrap().push(c);
            w += cw;
        }
        if self.cursor >= self.text.len() {
            if w >= cols {
                rows.push(String::new());
                w = 0;
            }
            cursor = (rows.len() - 1, w);
        }
        (rows, cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    fn text(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect()
    }

    #[test]
    fn wrapping_breaks_at_words_and_keeps_styles() {
        let bold = Style::new().add_modifier(Modifier::BOLD);
        let spans = vec![Span::raw("one two "), Span::styled("three", bold), Span::raw(" four five")];
        let lines = wrap(spans, 10, vec![Span::raw("- ")], vec![Span::raw("  ")]);
        assert_eq!(text(&lines), ["- one two", "  three", "  four", "  five"]);
        assert!(lines[1].spans.iter().any(|s| s.content == "three" && s.style == bold));
        // Long words are broken; line breaks start lines.
        let lines = wrap(vec![Span::raw("abcdefghij\nxy")], 4, vec![], vec![]);
        assert_eq!(text(&lines), ["abcd", "efgh", "ij", "xy"]);
    }

    #[test]
    fn fitting_and_padding() {
        assert_eq!(fit("hello world", 6), "hello…");
        assert_eq!(fit("hi", 6), "hi");
        assert_eq!(pad("hi", 4), "hi  ");
    }

    #[test]
    fn editing_an_input() {
        let mut i = Input::new("echo hi");
        let k = |c, m| KeyEvent::new(c, m);
        i.key(&k(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(i.text, "echo ");
        i.key(&k(KeyCode::Char('x'), KeyModifiers::NONE));
        i.key(&k(KeyCode::Home, KeyModifiers::NONE));
        i.key(&k(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!((i.text.as_str(), i.cursor), ("cho x", 0));
        i.key(&k(KeyCode::End, KeyModifiers::NONE));
        i.key(&k(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(i.text, "cho ");
        assert_eq!(i.key(&k(KeyCode::F(3), KeyModifiers::NONE)), Edit::Ignored);
    }

    #[test]
    fn rows_and_the_cursor() {
        let i = Input::new("abc\ndefgh");
        let (rows, cursor) = i.rows(4);
        assert_eq!(rows, ["abc", "defg", "h"]);
        assert_eq!(cursor, (2, 1));
    }
}
