//! Text being edited: a rope with one selection, undo and redo, and the edits
//! a light code editor needs (indenting, comments, moving lines, find and
//! replace, following the file on disk). No UI: `shed-app` draws it.
//!
//! Positions are char offsets into the text. Lines end in `\n` inside; a file
//! with CRLF endings gets them back when written.

use std::ops::Range;
use std::time::{Duration, Instant};

use ropey::Rope;
use unicode_width::UnicodeWidthChar;

/// Typing within this long of the last keystroke undoes together with it.
const UNDO_GROUP: Duration = Duration::from_millis(1000);

/// What is selected: from `anchor` to `head`, where the cursor is. Empty
/// when they are the same.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub fn cursor(at: usize) -> Self {
        Self { anchor: at, head: at }
    }

    pub fn range(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

/// Where the cursor goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    /// To the first non-blank of the line, or its very start if already there.
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
    PageUp(usize),
    PageDown(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    Crlf,
}

#[derive(Clone, Debug)]
struct Edit {
    at: usize,
    removed: String,
    inserted: String,
}

impl Edit {
    /// Where a position before this edit is after it.
    fn map(&self, pos: usize) -> usize {
        let (removed, inserted) = (self.removed.chars().count(), self.inserted.chars().count());
        if pos <= self.at {
            pos
        } else if pos >= self.at + removed {
            pos + inserted - removed
        } else {
            self.at + (pos - self.at).min(inserted)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Typing,
    Deleting,
    Other,
}

#[derive(Clone, Debug)]
struct Transaction {
    /// In the order they were made; each `at` is in the text as it was then.
    edits: Vec<Edit>,
    before: Selection,
    after: Selection,
    kind: Kind,
    at: Instant,
}

/// What a find matches.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FindOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
}

#[derive(Clone, Debug)]
pub struct Buffer {
    rope: Rope,
    sel: Selection,
    /// The display column Up and Down keep to, across short lines.
    goal: Option<usize>,
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    version: u64,
    saved: u64,
    pub line_ending: LineEnding,
    pub tab_width: usize,
    /// One level of indentation: a tab, or some spaces.
    pub indent: String,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new("")
    }
}

impl Buffer {
    pub fn new(text: &str) -> Self {
        let line_ending = if text.contains("\r\n") { LineEnding::Crlf } else { LineEnding::Lf };
        let text = if line_ending == LineEnding::Crlf { text.replace("\r\n", "\n") } else { text.to_string() };
        let indent = detect_indent(&text);
        Self {
            rope: Rope::from_str(&text),
            sel: Selection::default(),
            goal: None,
            undo: Vec::new(),
            redo: Vec::new(),
            version: 0,
            saved: 0,
            line_ending,
            tab_width: 4,
            indent,
        }
    }

    /// The text as it is written to a file.
    pub fn text(&self) -> String {
        let text = self.rope.to_string();
        match self.line_ending {
            LineEnding::Lf => text,
            LineEnding::Crlf => text.replace('\n', "\r\n"),
        }
    }

    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    /// Changes with every edit.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Edited since it was loaded or last saved.
    pub fn is_dirty(&self) -> bool {
        self.version != self.saved
    }

    pub fn mark_saved(&mut self) {
        self.saved = self.version;
    }

    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    /// Line `i` without its line break.
    pub fn line(&self, i: usize) -> String {
        if i >= self.len_lines() {
            return String::new();
        }
        let mut s = self.rope.line(i).to_string();
        if s.ends_with('\n') {
            s.pop();
        }
        s
    }

    /// Line `i`'s length in chars, without its line break.
    pub fn line_len(&self, i: usize) -> usize {
        if i >= self.len_lines() {
            return 0;
        }
        let line = self.rope.line(i);
        let n = line.len_chars();
        if n > 0 && line.char(n - 1) == '\n' { n - 1 } else { n }
    }

    pub fn line_start(&self, i: usize) -> usize {
        self.rope.line_to_char(i.min(self.len_lines()))
    }

    pub fn line_of(&self, pos: usize) -> usize {
        self.rope.char_to_line(pos.min(self.len_chars()))
    }

    /// Line and column (in chars) of a position.
    pub fn line_col(&self, pos: usize) -> (usize, usize) {
        let line = self.line_of(pos);
        (line, pos.min(self.len_chars()) - self.line_start(line))
    }

    /// The position at a line and column, kept inside the text.
    pub fn pos(&self, line: usize, col: usize) -> usize {
        let line = line.min(self.len_lines().saturating_sub(1));
        self.line_start(line) + col.min(self.line_len(line))
    }

    pub fn slice(&self, range: Range<usize>) -> String {
        let end = range.end.min(self.len_chars());
        self.rope.slice(range.start.min(end)..end).to_string()
    }

    pub fn char_at(&self, pos: usize) -> Option<char> {
        (pos < self.len_chars()).then(|| self.rope.char(pos))
    }

    pub fn selection(&self) -> Selection {
        self.sel
    }

    pub fn selected_text(&self) -> String {
        self.slice(self.sel.range())
    }

    pub fn set_selection(&mut self, sel: Selection) {
        let n = self.len_chars();
        self.sel = Selection { anchor: sel.anchor.min(n), head: sel.head.min(n) };
        self.goal = None;
    }

    /// Puts the cursor at `pos`, or extends the selection to it.
    pub fn set_cursor(&mut self, pos: usize, extend: bool) {
        let anchor = if extend { self.sel.anchor } else { pos };
        self.set_selection(Selection { anchor, head: pos });
    }

    pub fn select_all(&mut self) {
        self.set_selection(Selection { anchor: 0, head: self.len_chars() });
    }

    /// Selects the word (or run of spaces or punctuation) at `pos`.
    pub fn select_word(&mut self, pos: usize) {
        let (line, col) = self.line_col(pos);
        let chars: Vec<char> = self.line(line).chars().collect();
        if chars.is_empty() {
            return self.set_selection(Selection::cursor(pos));
        }
        let at = col.min(chars.len() - 1);
        let class = char_class(chars[at]);
        let start = (0..at).rev().take_while(|i| char_class(chars[*i]) == class).last().unwrap_or(at);
        let end = (at..chars.len()).take_while(|i| char_class(chars[*i]) == class).last().unwrap_or(at) + 1;
        let base = self.line_start(line);
        self.set_selection(Selection { anchor: base + start, head: base + end });
    }

    /// Selects the whole line at `pos`, with its line break.
    pub fn select_line(&mut self, pos: usize) {
        let line = self.line_of(pos);
        self.set_selection(Selection { anchor: self.line_start(line), head: self.line_start(line + 1) });
    }

    /// The display column of a char column in a line: tabs go to the next
    /// tab stop, wide characters take two.
    pub fn display_col(&self, line: usize, col: usize) -> usize {
        display_width(self.line(line).chars().take(col), self.tab_width)
    }

    /// The char column at (or just before) a display column.
    pub fn col_at_display(&self, line: usize, dcol: usize) -> usize {
        let mut width = 0;
        for (i, c) in self.line(line).chars().enumerate() {
            let w = char_width(c, width, self.tab_width);
            if width + w > dcol {
                return i;
            }
            width += w;
        }
        self.line_len(line)
    }

    pub fn move_cursor(&mut self, motion: Motion, extend: bool) {
        let sel = self.sel;
        let head = sel.head;
        // Left and Right with a selection go to its edge.
        if !extend && !sel.is_empty() {
            match motion {
                Motion::Left => return self.set_cursor(sel.range().start, false),
                Motion::Right => return self.set_cursor(sel.range().end, false),
                _ => {}
            }
        }
        let (line, col) = self.line_col(head);
        let vertical = |this: &mut Self, target: isize| {
            let goal = this.goal.unwrap_or_else(|| this.display_col(line, col));
            let last = this.len_lines() as isize - 1;
            let pos = if target < 0 {
                0
            } else if target > last {
                this.len_chars()
            } else {
                let t = target as usize;
                this.pos(t, this.col_at_display(t, goal))
            };
            this.set_cursor(pos, extend);
            this.goal = Some(goal);
        };
        match motion {
            Motion::Left => self.set_cursor(head.saturating_sub(1), extend),
            Motion::Right => self.set_cursor((head + 1).min(self.len_chars()), extend),
            Motion::Up => vertical(self, line as isize - 1),
            Motion::Down => vertical(self, line as isize + 1),
            Motion::PageUp(n) => vertical(self, line as isize - n.max(1) as isize),
            Motion::PageDown(n) => vertical(self, line as isize + n.max(1) as isize),
            Motion::WordLeft => self.set_cursor(self.word_left(head), extend),
            Motion::WordRight => self.set_cursor(self.word_right(head), extend),
            Motion::LineStart => {
                let first = self.line(line).chars().take_while(|c| c.is_whitespace()).count();
                let to = if col == first { 0 } else { first };
                self.set_cursor(self.line_start(line) + to, extend);
            }
            Motion::LineEnd => self.set_cursor(self.line_start(line) + self.line_len(line), extend),
            Motion::DocStart => self.set_cursor(0, extend),
            Motion::DocEnd => self.set_cursor(self.len_chars(), extend),
        }
    }

    fn word_left(&self, pos: usize) -> usize {
        let mut p = pos;
        while p > 0 && self.rope.char(p - 1).is_whitespace() && self.rope.char(p - 1) != '\n' {
            p -= 1;
        }
        if p > 0 && self.rope.char(p - 1) == '\n' {
            return if p == pos { p - 1 } else { p };
        }
        let Some(c) = p.checked_sub(1).map(|i| self.rope.char(i)) else { return 0 };
        let class = char_class(c);
        while p > 0 && char_class(self.rope.char(p - 1)) == class && self.rope.char(p - 1) != '\n' {
            p -= 1;
        }
        p
    }

    fn word_right(&self, pos: usize) -> usize {
        let n = self.len_chars();
        let mut p = pos;
        if p < n && self.rope.char(p) == '\n' {
            return p + 1;
        }
        while p < n && self.rope.char(p).is_whitespace() && self.rope.char(p) != '\n' {
            p += 1;
        }
        if p >= n || self.rope.char(p) == '\n' {
            return p;
        }
        let class = char_class(self.rope.char(p));
        while p < n && char_class(self.rope.char(p)) == class && self.rope.char(p) != '\n' {
            p += 1;
        }
        p
    }

    /// Makes edits (non-overlapping ranges in the current text, each with
    /// its new text) as one undoable step, then selects `after`, given in
    /// the text as it will be.
    fn transact(&mut self, mut edits: Vec<(Range<usize>, String)>, after: Option<Selection>, kind: Kind) {
        edits.retain(|(r, t)| !(r.is_empty() && t.is_empty()));
        if edits.is_empty() {
            return;
        }
        edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
        let before = self.sel;
        let mut sel = self.sel;
        let mut made = Vec::with_capacity(edits.len());
        for (range, text) in edits {
            let removed = self.slice(range.clone());
            self.rope.remove(range.clone());
            self.rope.insert(range.start, &text);
            let edit = Edit { at: range.start, removed, inserted: text };
            sel = Selection { anchor: edit.map(sel.anchor), head: edit.map(sel.head) };
            made.push(edit);
        }
        let after = after.unwrap_or(sel);
        self.set_selection(after);
        self.version += 1;
        self.redo.clear();
        let now = Instant::now();
        // Typing and deleting char by char undo a run at a time.
        if kind != Kind::Other
            && made.len() == 1
            && let Some(last) = self.undo.last_mut()
            && last.kind == kind
            && now.duration_since(last.at) < UNDO_GROUP
            && last.after == before
            && !made[0].inserted.contains('\n')
        {
            last.edits.extend(made);
            last.after = self.sel;
            last.at = now;
            return;
        }
        self.undo.push(Transaction { edits: made, before, after: self.sel, kind, at: now });
    }

    pub fn undo(&mut self) -> bool {
        let Some(t) = self.undo.pop() else { return false };
        for e in t.edits.iter().rev() {
            let end = e.at + e.inserted.chars().count();
            self.rope.remove(e.at..end);
            self.rope.insert(e.at, &e.removed);
        }
        self.set_selection(t.before);
        self.version += 1;
        self.redo.push(t);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(t) = self.redo.pop() else { return false };
        for e in &t.edits {
            let end = e.at + e.removed.chars().count();
            self.rope.remove(e.at..end);
            self.rope.insert(e.at, &e.inserted);
        }
        self.set_selection(t.after);
        self.version += 1;
        self.undo.push(t);
        true
    }

    /// Replaces the selection with `text`, as typing or pasting does.
    pub fn insert(&mut self, text: &str) {
        let range = self.sel.range();
        let end = range.start + text.chars().count();
        let kind = if text.chars().count() == 1 && text != "\n" { Kind::Typing } else { Kind::Other };
        self.transact(vec![(range, text.to_string())], Some(Selection::cursor(end)), kind);
    }

    /// Replaces a range, keeping the selection where it maps to.
    pub fn replace(&mut self, range: Range<usize>, text: &str) {
        let range = range.start.min(self.len_chars())..range.end.min(self.len_chars());
        self.transact(vec![(range, text.to_string())], None, Kind::Other);
    }

    pub fn backspace(&mut self) {
        let sel = self.sel;
        if !sel.is_empty() {
            return self.transact(vec![(sel.range(), String::new())], None, Kind::Deleting);
        }
        if sel.head == 0 {
            return;
        }
        // In leading spaces, back to the previous indent stop.
        let (line, col) = self.line_col(sel.head);
        let lead: String = self.line(line).chars().take(col).collect();
        let width = if !self.indent.starts_with('\t') && col > 0 && lead.chars().all(|c| c == ' ') {
            let unit = self.indent.len().max(1);
            match col % unit {
                0 => unit,
                n => n,
            }
        } else {
            1
        };
        let start = sel.head - width;
        self.transact(vec![(start..sel.head, String::new())], Some(Selection::cursor(start)), Kind::Deleting);
    }

    pub fn delete_forward(&mut self) {
        let sel = self.sel;
        let range = if sel.is_empty() { sel.head..(sel.head + 1).min(self.len_chars()) } else { sel.range() };
        self.transact(vec![(range.clone(), String::new())], Some(Selection::cursor(range.start)), Kind::Deleting);
    }

    pub fn delete_word_back(&mut self) {
        if !self.sel.is_empty() {
            return self.backspace();
        }
        let head = self.sel.head;
        let start = self.word_left(head);
        self.transact(vec![(start..head, String::new())], Some(Selection::cursor(start)), Kind::Other);
    }

    pub fn delete_word_forward(&mut self) {
        if !self.sel.is_empty() {
            return self.delete_forward();
        }
        let head = self.sel.head;
        let end = self.word_right(head);
        self.transact(vec![(head..end, String::new())], Some(Selection::cursor(head)), Kind::Other);
    }

    /// A line break that keeps the line's indentation, one level more after
    /// an opening bracket, and puts a closing bracket right after the cursor
    /// on a line of its own.
    pub fn newline(&mut self) {
        let range = self.sel.range();
        let (line, _) = self.line_col(range.start);
        let lead: String = self.line(line).chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        let before = range.start.checked_sub(1).and_then(|p| self.char_at(p));
        let after = self.char_at(range.end);
        let opens = matches!(before, Some('{' | '[' | '('));
        let closes = matches!((before, after), (Some('{'), Some('}')) | (Some('['), Some(']')) | (Some('('), Some(')')));
        let inner = if opens { format!("{lead}{}", self.indent) } else { lead.clone() };
        let mut text = format!("\n{inner}");
        let cursor = range.start + text.chars().count();
        if closes {
            text.push_str(&format!("\n{lead}"));
        }
        self.transact(vec![(range, text)], Some(Selection::cursor(cursor)), Kind::Other);
    }

    /// The lines the selection touches; a selection ending at a line's very
    /// start leaves that line out.
    fn selected_lines(&self) -> Range<usize> {
        let r = self.sel.range();
        let first = self.line_of(r.start);
        let mut last = self.line_of(r.end);
        if last > first && self.line_start(last) == r.end {
            last -= 1;
        }
        first..last + 1
    }

    /// Indents: the selected lines, or at the cursor up to the next stop.
    pub fn indent(&mut self) {
        let sel = self.sel;
        let lines = self.selected_lines();
        if sel.is_empty() || lines.len() == 1 && !self.sel_covers_line_start() {
            let text = if self.indent.starts_with('\t') {
                "\t".to_string()
            } else {
                let (line, col) = self.line_col(sel.range().start);
                let unit = self.indent.len().max(1);
                let dcol = self.display_col(line, col);
                " ".repeat(unit - dcol % unit)
            };
            return self.insert(&text);
        }
        let edits = lines.map(|l| self.line_start(l)..self.line_start(l)).map(|r| (r, self.indent.clone())).collect();
        self.transact(edits, None, Kind::Other);
        self.extend_to_whole_lines();
    }

    fn sel_covers_line_start(&self) -> bool {
        let r = self.sel.range();
        let (_, col) = self.line_col(r.start);
        col == 0 && r.end > r.start
    }

    fn extend_to_whole_lines(&mut self) {
        let lines = self.selected_lines();
        let (start, end) = (self.line_start(lines.start), self.line_start(lines.end).max(self.line_start(lines.end - 1) + self.line_len(lines.end - 1)));
        self.set_selection(Selection { anchor: start, head: end });
    }

    /// Takes one level of indentation off the selected lines.
    pub fn outdent(&mut self) {
        let unit = self.indent.chars().count().max(1);
        let edits: Vec<_> = self
            .selected_lines()
            .filter_map(|l| {
                let line = self.line(l);
                let n = if line.starts_with('\t') { 1 } else { line.chars().take(unit).take_while(|c| *c == ' ').count() };
                (n > 0).then(|| (self.line_start(l)..self.line_start(l) + n, String::new()))
            })
            .collect();
        let whole = !self.sel.is_empty();
        self.transact(edits, None, Kind::Other);
        if whole {
            self.extend_to_whole_lines();
        }
    }

    /// Comments the selected lines out with `prefix` (such as `//`), or back
    /// in when every one of them is commented already.
    pub fn toggle_comment(&mut self, prefix: &str) {
        let lines: Vec<usize> = self.selected_lines().filter(|l| !self.line(*l).trim().is_empty()).collect();
        if lines.is_empty() {
            return;
        }
        let lead = |s: &str| s.chars().take_while(|c| c.is_whitespace()).count();
        let commented = lines.iter().all(|l| self.line(*l).trim_start().starts_with(prefix));
        let edits = if commented {
            lines
                .iter()
                .map(|l| {
                    let text = self.line(*l);
                    let at = self.line_start(*l) + lead(&text);
                    let rest = &text.trim_start()[prefix.len()..];
                    let n = prefix.chars().count() + usize::from(rest.starts_with(' '));
                    (at..at + n, String::new())
                })
                .collect()
        } else {
            let col = lines.iter().map(|l| lead(&self.line(*l))).min().unwrap_or(0);
            lines.iter().map(|l| (self.line_start(*l) + col..self.line_start(*l) + col, format!("{prefix} "))).collect()
        };
        self.transact(edits, None, Kind::Other);
    }

    /// Copies the selected lines (or the selection) right after themselves.
    pub fn duplicate(&mut self) {
        let sel = self.sel;
        if !sel.is_empty() && self.selected_lines().len() == 1 && !self.sel_covers_line_start() {
            let r = sel.range();
            let text = self.slice(r.clone());
            let n = text.chars().count();
            return self.transact(vec![(r.end..r.end, text)], Some(Selection { anchor: r.end, head: r.end + n }), Kind::Other);
        }
        let lines = self.selected_lines();
        let start = self.line_start(lines.start);
        let end = self.line_start(lines.end - 1) + self.line_len(lines.end - 1);
        let block = self.slice(start..end);
        let shift = block.chars().count() + 1;
        let after = Selection { anchor: sel.anchor + shift, head: sel.head + shift };
        self.transact(vec![(end..end, format!("\n{block}"))], Some(after), Kind::Other);
    }

    /// Removes the selected lines whole.
    pub fn delete_lines(&mut self) {
        let lines = self.selected_lines();
        let mut start = self.line_start(lines.start);
        let mut end = self.line_start(lines.end);
        if lines.end >= self.len_lines() {
            // The last line: take the break before it instead.
            end = self.len_chars();
            start = start.saturating_sub(usize::from(lines.start > 0));
        }
        self.transact(vec![(start..end, String::new())], Some(Selection::cursor(start.min(self.len_chars()))), Kind::Other);
        let line = self.line_of(self.sel.head);
        let first = self.line(line).chars().take_while(|c| c.is_whitespace()).count();
        self.set_cursor(self.line_start(line) + first, false);
    }

    /// Moves the selected lines up or down past their neighbour.
    pub fn move_lines(&mut self, up: bool) {
        let lines = self.selected_lines();
        let last = self.len_lines() - 1;
        if (up && lines.start == 0) || (!up && lines.end > last) {
            return;
        }
        let other = if up { lines.start - 1 } else { lines.end };
        let block: Vec<String> = lines.clone().map(|l| self.line(l)).collect();
        let neighbour = self.line(other);
        let (first, end_line) = if up { (other, lines.end - 1) } else { (lines.start, other) };
        let start = self.line_start(first);
        let end = self.line_start(end_line) + self.line_len(end_line);
        let text = if up { format!("{}\n{neighbour}", block.join("\n")) } else { format!("{neighbour}\n{}", block.join("\n")) };
        let shift = neighbour.chars().count() + 1;
        let sel = self.sel;
        let after = if up {
            Selection { anchor: sel.anchor - shift, head: sel.head - shift }
        } else {
            Selection { anchor: sel.anchor + shift, head: sel.head + shift }
        };
        self.transact(vec![(start..end, text)], Some(after), Kind::Other);
    }

    /// Every match of `query`, in order.
    pub fn find_all(&self, query: &str, options: &FindOptions) -> Vec<Range<usize>> {
        if query.is_empty() {
            return Vec::new();
        }
        let fold = |c: char| if options.case_sensitive { c } else { c.to_lowercase().next().unwrap_or(c) };
        let text: Vec<char> = self.rope.chars().map(fold).collect();
        let query: Vec<char> = query.chars().map(fold).collect();
        let mut found = Vec::new();
        let mut i = 0;
        while i + query.len() <= text.len() {
            if text[i] == query[0] && text[i..i + query.len()] == query[..] {
                let end = i + query.len();
                let bounded = !options.whole_word
                    || ((i == 0 || char_class(text[i - 1]) != 1) && (end == text.len() || char_class(text[end]) != 1));
                if bounded {
                    found.push(i..end);
                    i = end;
                    continue;
                }
            }
            i += 1;
        }
        found
    }

    /// Replaces each range with `with`, as one undoable step.
    pub fn replace_all(&mut self, ranges: &[Range<usize>], with: &str) {
        let edits = ranges.iter().map(|r| (r.clone(), with.to_string())).collect();
        self.transact(edits, None, Kind::Other);
    }

    /// Takes the text as it now is on disk, changing only what differs so
    /// the cursor stays with its text; undoable, and counted as saved.
    /// Returns the lines that changed, in the new text.
    pub fn reload(&mut self, text: &str) -> Vec<Range<usize>> {
        let text = text.replace("\r\n", "\n");
        let old = self.rope.to_string();
        if old == text {
            self.mark_saved();
            return Vec::new();
        }
        let diff = similar::TextDiff::from_lines(&old, &text);
        let old_lines: Vec<&str> = diff.old_slices().to_vec();
        let new_lines: Vec<&str> = diff.new_slices().to_vec();
        let char_at_line = |lines: &[&str], n: usize| lines[..n].iter().map(|l| l.chars().count()).sum::<usize>();
        let mut edits = Vec::new();
        let mut changed = Vec::new();
        for op in diff.ops() {
            if let similar::DiffOp::Equal { .. } = op {
                continue;
            }
            let (old_range, new_range) = (op.old_range(), op.new_range());
            let start = char_at_line(&old_lines, old_range.start);
            let end = start + old_lines[old_range.clone()].iter().map(|l| l.chars().count()).sum::<usize>();
            edits.push((start..end, new_lines[new_range.clone()].concat()));
            if !new_range.is_empty() {
                changed.push(new_range);
            }
        }
        self.transact(edits, None, Kind::Other);
        self.mark_saved();
        changed
    }
}

/// 0: blank, 1: word (letters, digits, `_`), 2: anything else.
fn char_class(c: char) -> u8 {
    if c.is_whitespace() {
        0
    } else if c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

/// How many columns `c` takes when it starts at display column `at`.
pub fn char_width(c: char, at: usize, tab_width: usize) -> usize {
    if c == '\t' { tab_width - at % tab_width.max(1) } else { c.width().unwrap_or(0) }
}

/// The display width of some chars from a line's start.
pub fn display_width(chars: impl Iterator<Item = char>, tab_width: usize) -> usize {
    chars.fold(0, |w, c| w + char_width(c, w, tab_width))
}

/// A line with its tabs turned into spaces, as it is drawn, and for each
/// char of the line where it starts in the drawn text (bytes), with one more
/// entry for the line's end.
pub fn expand_tabs(line: &str, tab_width: usize) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(line.len());
    let mut starts = Vec::with_capacity(line.len() + 1);
    let mut width = 0;
    for c in line.chars() {
        starts.push(out.len());
        let w = char_width(c, width, tab_width);
        if c == '\t' {
            out.extend(std::iter::repeat_n(' ', w));
        } else {
            out.push(c);
        }
        width += w;
    }
    starts.push(out.len());
    (out, starts)
}

/// The chars of a line that display columns `cols` cover (tabs to their
/// stops, wide chars two columns): a tab or wide char partly inside counts,
/// with the zero-width chars after the last. Empty when none are.
pub fn chars_in_columns(line: &str, cols: Range<usize>, tab_width: usize) -> Range<usize> {
    let mut found: Option<Range<usize>> = None;
    let mut width = 0;
    for (i, c) in line.chars().enumerate() {
        let w = char_width(c, width, tab_width);
        if w == 0 {
            if let Some(r) = found.as_mut().filter(|r| r.end == i) {
                r.end = i + 1;
            }
            continue;
        }
        if width >= cols.end {
            break;
        }
        if width + w > cols.start {
            found.get_or_insert(i..i).end = i + 1;
        }
        width += w;
    }
    found.filter(|_| !cols.is_empty()).unwrap_or(0..0)
}

/// What a line shows in display columns `cols`: a wide char partly inside
/// comes whole, a tab as the spaces inside. A short line gives what it has.
pub fn columns_text(line: &str, cols: Range<usize>, tab_width: usize) -> String {
    let chars = chars_in_columns(line, cols.clone(), tab_width);
    let mut out = String::new();
    let mut width = 0;
    for (i, c) in line.chars().enumerate().take(chars.end) {
        let w = char_width(c, width, tab_width);
        if i >= chars.start {
            if c == '\t' {
                out.extend(std::iter::repeat_n(' ', (width + w).min(cols.end) - width.max(cols.start)));
            } else {
                out.push(c);
            }
        }
        width += w;
    }
    out
}

impl Buffer {
    /// A rectangle's text: each of `lines`' slice of display columns
    /// `cols` ([`columns_text`]), a line each.
    pub fn block_text(&self, lines: Range<usize>, cols: Range<usize>) -> String {
        let lines = lines.start.min(self.len_lines())..lines.end.min(self.len_lines());
        lines.map(|l| columns_text(&self.line(l), cols.clone(), self.tab_width)).collect::<Vec<_>>().join("\n")
    }
}

/// The indentation a text uses: a tab if lines start with one, else the
/// smallest run of leading spaces (2 to 8), else four spaces.
fn detect_indent(text: &str) -> String {
    let mut tabs = 0;
    let mut smallest = usize::MAX;
    for line in text.lines().take(2000) {
        if line.starts_with('\t') {
            tabs += 1;
        } else {
            let n = line.chars().take_while(|c| *c == ' ').count();
            if n >= 2 && n < smallest && line.len() > n {
                smallest = n;
            }
        }
    }
    if tabs > 0 && smallest == usize::MAX {
        "\t".into()
    } else if (2..=8).contains(&smallest) {
        " ".repeat(smallest)
    } else {
        "    ".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_slice_lines_as_drawn() {
        // Tabs to their stops (4), cut where the columns cut them.
        assert_eq!(columns_text("\tab", 2..6, 4), "  ab");
        assert_eq!(columns_text("a\tb", 0..2, 4), "a ");
        // A wide char half inside comes whole.
        assert_eq!(columns_text("ab日本cd", 3..5, 4), "日本");
        assert_eq!(chars_in_columns("ab日本cd", 3..5, 4), 2..4);
        // Zero-width marks stay with their char.
        assert_eq!(columns_text("e\u{301}x", 0..1, 4), "e\u{301}");
        // Short and empty lines give what they have.
        assert_eq!(columns_text("abc", 2..10, 4), "c");
        assert_eq!(columns_text("ab", 4..8, 4), "");
        assert_eq!(columns_text("abc", 1..1, 4), "");
        let b = Buffer::new("let a = 1;\nx\n\tlet bb = 22;\n");
        assert_eq!(b.block_text(0..3, 4..9), "a = 1\n\nlet b");
        assert_eq!(b.block_text(2..10, 0..2), "  \n");
    }

    fn buf(text: &str, at: usize) -> Buffer {
        let mut b = Buffer::new(text);
        b.set_cursor(at, false);
        b
    }

    #[test]
    fn typing_groups_for_undo() {
        let mut b = buf("", 0);
        for c in "hello".chars() {
            b.insert(&c.to_string());
        }
        b.insert(" ");
        assert_eq!(b.text(), "hello ");
        assert!(b.is_dirty());
        assert!(b.undo());
        assert_eq!(b.text(), "");
        assert!(b.redo());
        assert_eq!((b.text().as_str(), b.selection().head), ("hello ", 6));
    }

    #[test]
    fn crlf_comes_back() {
        let mut b = Buffer::new("a\r\nb\r\n");
        assert_eq!(b.len_lines(), 3);
        b.set_cursor(1, false);
        b.insert("x");
        assert_eq!(b.text(), "ax\r\nb\r\n");
    }

    #[test]
    fn newline_keeps_and_adds_indent() {
        let mut b = buf("fn a() {}", 8);
        b.newline();
        assert_eq!(b.text(), "fn a() {\n    \n}");
        assert_eq!(b.line_col(b.selection().head), (1, 4));
        b.insert("x");
        b.newline();
        assert_eq!(b.line(2), "    ");
    }

    #[test]
    fn indent_outdent_comment() {
        let mut b = Buffer::new("a\n  b\nc\n");
        b.set_selection(Selection { anchor: 0, head: 7 });
        b.indent();
        assert_eq!(b.text(), "  a\n    b\n  c\n");
        b.outdent();
        assert_eq!(b.text(), "a\n  b\nc\n");
        b.toggle_comment("//");
        assert_eq!(b.text(), "// a\n//   b\n// c\n");
        b.toggle_comment("//");
        assert_eq!(b.text(), "a\n  b\nc\n");
        let mut b = buf("x", 1);
        b.indent();
        assert_eq!(b.text(), "x   ");
        b.backspace();
        assert_eq!(b.text(), "x  ", "after text, one space at a time");
        let mut b = buf("        y", 8);
        b.indent = "    ".into();
        b.backspace();
        assert_eq!(b.text(), "    y", "in leading spaces, back to the last stop");
    }

    #[test]
    fn moving_through_words_and_lines() {
        let mut b = buf("let foo_bar = 1;\n    x", 0);
        b.move_cursor(Motion::WordRight, false);
        assert_eq!(b.selection().head, 3);
        b.move_cursor(Motion::WordRight, false);
        assert_eq!(b.selection().head, 11);
        b.move_cursor(Motion::WordLeft, true);
        assert_eq!(b.selected_text(), "foo_bar");
        b.set_cursor(14, false);
        b.move_cursor(Motion::Down, false);
        assert_eq!(b.line_col(b.selection().head), (1, 5));
        b.move_cursor(Motion::LineStart, false);
        assert_eq!(b.line_col(b.selection().head), (1, 4));
        b.move_cursor(Motion::LineStart, false);
        assert_eq!(b.line_col(b.selection().head), (1, 0));
        b.move_cursor(Motion::Up, false);
        b.move_cursor(Motion::LineEnd, false);
        assert_eq!(b.selection().head, 16);
    }

    #[test]
    fn lines_move_duplicate_delete() {
        let mut b = buf("a\nb\nc", 2);
        b.move_lines(true);
        assert_eq!(b.text(), "b\na\nc");
        assert_eq!(b.selection().head, 0);
        b.move_lines(false);
        b.move_lines(false);
        assert_eq!(b.text(), "a\nc\nb");
        b.duplicate();
        assert_eq!(b.text(), "a\nc\nb\nb");
        assert_eq!(b.line_of(b.selection().head), 3);
        b.delete_lines();
        assert_eq!(b.text(), "a\nc\nb");
        b.set_cursor(0, false);
        b.delete_lines();
        assert_eq!(b.text(), "c\nb");
    }

    #[test]
    fn find_and_replace() {
        let mut b = Buffer::new("Foo foo food");
        let all = b.find_all("foo", &FindOptions::default());
        assert_eq!(all, [0..3, 4..7, 8..11]);
        let words = b.find_all("foo", &FindOptions { case_sensitive: true, whole_word: true });
        assert_eq!(words, vec![4..7]);
        b.replace_all(&all, "x");
        assert_eq!(b.text(), "x x xd");
        b.undo();
        assert_eq!(b.text(), "Foo foo food");
    }

    #[test]
    fn reload_keeps_the_cursor_with_its_text() {
        let mut b = buf("one\ntwo\nthree\n", 9);
        assert_eq!(b.line_col(b.selection().head), (2, 1));
        let changed = b.reload("zero\none\ntwo\nthree\nfour\n");
        assert_eq!(b.line_col(b.selection().head), (3, 1));
        assert_eq!(changed, [0..1, 4..5]);
        assert!(!b.is_dirty());
        b.undo();
        assert_eq!(b.text(), "one\ntwo\nthree\n");
    }

    #[test]
    fn tabs_and_wide_chars_take_their_columns() {
        let b = Buffer::new("\tab\n日本x");
        assert_eq!(b.display_col(0, 1), 4);
        assert_eq!(b.display_col(1, 2), 4);
        assert_eq!(b.col_at_display(1, 3), 1);
        let (text, starts) = expand_tabs("\tx", 4);
        assert_eq!((text.as_str(), starts), ("    x", vec![0, 4, 5]));
        assert_eq!(detect_indent("a\n  b\n    c\n"), "  ");
        assert_eq!(detect_indent("a\n\tb\n"), "\t");
    }
}
