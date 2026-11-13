//! The AI tab's text: Claude's Markdown as pieces to draw (each a run of
//! selectable text), the selection across them, and what a copy puts on the
//! clipboard (Markdown as text, and HTML beside it for formatted pastes).

use std::ops::Range;

use gpui::{ClipboardItem, FontStyle, FontWeight, HighlightStyle, StrikethroughStyle, UnderlineStyle, px};
use shed_ai::markdown::{Align, Block, Text};
use shed_model::Rgb;

use crate::theme::{hsla, hsla_alpha};

/// What the Linux clipboard patch looks for (see `vendor/gpui_ce_linux`).
const HTML_PREFIX: &str = "shed-html:";

/// `text` for programs that take text, and `markdown` rendered as HTML for
/// those that keep formatting (mail, documents, chat; Linux only for now).
pub fn formatted_item(text: String, markdown: &str) -> ClipboardItem {
    let html = shed_ai::markdown::to_html(markdown);
    ClipboardItem::new_string_with_metadata(text, format!("{HTML_PREFIX}{html}"))
}

/// A place in the conversation's text: an entry, a piece of it, a byte in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Point {
    pub entry: usize,
    pub piece: usize,
    pub offset: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Selection {
    pub anchor: Point,
    pub head: Point,
}

impl Selection {
    pub fn at(p: Point) -> Self {
        Selection { anchor: p, head: p }
    }

    pub fn ordered(&self) -> (Point, Point) {
        if self.anchor <= self.head { (self.anchor, self.head) } else { (self.head, self.anchor) }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// The selected part of piece `piece` of entry `entry`, `len` bytes long.
    pub fn range_in(&self, entry: usize, piece: usize, len: usize) -> Option<Range<usize>> {
        let (a, b) = self.ordered();
        let key = (entry, piece);
        if key < (a.entry, a.piece) || key > (b.entry, b.piece) {
            return None;
        }
        let start = if key == (a.entry, a.piece) { a.offset.min(len) } else { 0 };
        let end = if key == (b.entry, b.piece) { b.offset.min(len) } else { len };
        (start < end).then_some(start..end)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Para,
    Heading(u8),
    /// A list item; `marker` empty for an item's later paragraphs.
    Item { depth: usize, marker: String },
    Quote,
    Code { lang: String, depth: usize },
    Cell { row: usize, col: usize, cols: usize, align: Align },
    Rule,
}

/// A run of selectable text, as drawn.
#[derive(Clone, Debug)]
pub struct Piece {
    pub kind: Kind,
    pub text: Text,
}

/// The pieces of Markdown blocks, in the order they are drawn.
pub fn pieces(blocks: Vec<Block>) -> Vec<Piece> {
    let mut out = Vec::new();
    for block in blocks {
        match block {
            Block::Paragraph(text) => out.push(Piece { kind: Kind::Para, text }),
            Block::Heading { level, text } => out.push(Piece { kind: Kind::Heading(level), text }),
            Block::Item { depth, marker, text } => out.push(Piece { kind: Kind::Item { depth, marker }, text }),
            Block::Quote(text) => out.push(Piece { kind: Kind::Quote, text }),
            Block::Code { lang, code, depth } => out.push(Piece { kind: Kind::Code { lang, depth }, text: Text::plain(code) }),
            Block::Rule => out.push(Piece { kind: Kind::Rule, text: Text::default() }),
            Block::Table { align, header, rows } => {
                let cols = header.len().max(rows.iter().map(Vec::len).max().unwrap_or(0));
                for (row, cells) in std::iter::once(header).chain(rows).enumerate() {
                    for (col, text) in cells.into_iter().enumerate() {
                        let align = align.get(col).copied().unwrap_or(Align::Left);
                        out.push(Piece { kind: Kind::Cell { row, col, cols, align }, text });
                    }
                }
            }
        }
    }
    out
}

/// How a piece's styled runs are drawn.
pub fn highlights(text: &Text, ink: Rgb, code_bg: Rgb) -> Vec<(Range<usize>, HighlightStyle)> {
    text.spans
        .iter()
        .map(|(range, s)| {
            let mut h = HighlightStyle::default();
            if s.bold {
                h.font_weight = Some(FontWeight::BOLD);
            }
            if s.italic {
                h.font_style = Some(FontStyle::Italic);
            }
            if s.strike {
                h.strikethrough = Some(StrikethroughStyle { thickness: px(1.0), color: None });
            }
            if s.code {
                h.color = Some(hsla(ink));
                h.background_color = Some(hsla_alpha(code_bg, 0.6));
            }
            if s.link {
                h.color = Some(hsla(ink));
                h.underline = Some(UnderlineStyle { thickness: px(1.0), color: None, wavy: false });
            }
            (range.clone(), h)
        })
        .collect()
}

/// `styles` (in order, not overlapping) with `selected` laid over them.
pub fn with_selection(styles: Vec<(Range<usize>, HighlightStyle)>, selected: Option<Range<usize>>, sel: HighlightStyle) -> Vec<(Range<usize>, HighlightStyle)> {
    let Some(selected) = selected else { return styles };
    let mut cuts: Vec<usize> = styles.iter().flat_map(|(r, _)| [r.start, r.end]).chain([selected.start, selected.end]).collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = Vec::new();
    for pair in cuts.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let own = styles.iter().find(|(r, _)| r.start <= from && to <= r.end).map(|(_, s)| *s);
        let inside = selected.start <= from && to <= selected.end;
        let style = match (own, inside) {
            (Some(s), true) => Some(s.highlight(sel)),
            (Some(s), false) => Some(s),
            (None, true) => Some(sel),
            (None, false) => None,
        };
        if let Some(style) = style {
            out.push((from..to, style));
        }
    }
    out
}

/// The word around byte `at`.
pub fn word_at(text: &str, at: usize) -> Range<usize> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let at = text.floor_char_boundary(at.min(text.len()));
    let start = text[..at].char_indices().rev().take_while(|(_, c)| is_word(*c)).last().map_or(at, |(i, _)| i);
    let end = text[at..].char_indices().find(|(_, c)| !is_word(*c)).map_or(text.len(), |(i, _)| at + i);
    start..end
}

/// The link at byte `at`, if any.
pub fn link_at(text: &Text, at: usize) -> Option<&str> {
    text.links.iter().find(|(r, _)| r.contains(&at)).map(|(_, url)| url.as_str())
}

/// Part of a piece's text as Markdown again.
fn inline_md(text: &Text, range: Range<usize>) -> String {
    let mut cuts: Vec<usize> = text
        .spans
        .iter()
        .map(|(r, _)| r)
        .chain(text.links.iter().map(|(r, _)| r))
        .flat_map(|r| [r.start, r.end])
        .chain([range.start, range.end])
        .filter(|c| range.contains(c) || *c == range.end)
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = String::new();
    let mut open_link: Option<&str> = None;
    for pair in cuts.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let Some(part) = text.text.get(from..to) else { continue };
        let style = text.spans.iter().find(|(r, _)| r.start <= from && to <= r.end).map(|(_, s)| *s).unwrap_or_default();
        let link = text.links.iter().find(|(r, _)| r.start <= from && to <= r.end).map(|(_, u)| u.as_str());
        if link != open_link {
            if let Some(url) = open_link {
                out.push_str(&format!("]({url})"));
            }
            if link.is_some() {
                out.push('[');
            }
            open_link = link;
        }
        let wrap = if style.code {
            "`"
        } else {
            match (style.bold, style.italic) {
                (true, true) => "***",
                (true, false) => "**",
                (false, true) => "*",
                _ => "",
            }
        };
        let strike = if style.strike && !style.code { "~~" } else { "" };
        out.push_str(&format!("{strike}{wrap}{part}{wrap}{strike}"));
    }
    if let Some(url) = open_link {
        out.push_str(&format!("]({url})"));
    }
    out
}

/// Builds what a copy of selected pieces holds: plain text and Markdown.
#[derive(Default)]
pub struct Copy {
    pub plain: String,
    pub markdown: String,
    last: Option<Kind>,
}

impl Copy {
    /// Adds `range` of `piece`.
    pub fn add(&mut self, piece: &Piece, range: Range<usize>) {
        let text = &piece.text.text;
        let range = text.floor_char_boundary(range.start.min(text.len()))..text.floor_char_boundary(range.end.min(text.len()));
        let slice = &text[range.clone()];
        let (plain_sep, md_sep) = match (&self.last, &piece.kind) {
            (None, _) => ("", ""),
            (Some(Kind::Item { .. }), Kind::Item { .. }) => ("\n", "\n"),
            (Some(Kind::Cell { row: r1, .. }), Kind::Cell { row: r2, .. }) if r1 == r2 => ("\t", " "),
            (Some(Kind::Cell { .. }), Kind::Cell { .. }) => ("\n", "\n"),
            _ => ("\n\n", "\n\n"),
        };
        self.plain.push_str(plain_sep);
        self.markdown.push_str(md_sep);
        let inline = inline_md(&piece.text, range);
        match &piece.kind {
            Kind::Para | Kind::Rule => {
                self.plain.push_str(slice);
                self.markdown.push_str(&inline);
            }
            Kind::Heading(level) => {
                self.plain.push_str(slice);
                self.markdown.push_str(&format!("{} {inline}", "#".repeat(*level as usize)));
            }
            Kind::Quote => {
                self.plain.push_str(slice);
                self.markdown.push_str(&format!("> {inline}"));
            }
            Kind::Item { depth, marker } => {
                let indent = "  ".repeat(*depth);
                let md_marker = match marker.as_str() {
                    "•" => "-",
                    "☑" => "- [x]",
                    "☐" => "- [ ]",
                    "" => " ",
                    m => m,
                };
                self.plain.push_str(&format!("{indent}{}{slice}", if marker.is_empty() { "  ".to_string() } else { format!("{marker} ") }));
                self.markdown.push_str(&format!("{indent}{md_marker} {inline}"));
            }
            Kind::Code { lang, .. } => {
                self.plain.push_str(slice);
                self.markdown.push_str(&format!("```{lang}\n{slice}\n```"));
            }
            Kind::Cell { row, col, cols, .. } => {
                self.plain.push_str(slice);
                if *col == 0 {
                    self.markdown.push_str("| ");
                }
                self.markdown.push_str(&format!("{inline} |"));
                if *row == 0 && col + 1 == *cols {
                    self.markdown.push_str(&format!("\n|{}", "---|".repeat(*cols)));
                }
            }
        }
        self.last = Some(piece.kind.clone());
    }

    /// The next entry starts a new block.
    pub fn next_entry(&mut self) {
        if self.last.is_some() {
            self.last = Some(Kind::Para);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(md: &str) -> Vec<Piece> {
        pieces(shed_ai::markdown::parse(md))
    }

    #[test]
    fn selection_ranges_span_pieces() {
        let sel = Selection { anchor: Point { entry: 2, piece: 1, offset: 4 }, head: Point { entry: 1, piece: 3, offset: 2 } };
        assert_eq!(sel.range_in(1, 3, 10), Some(2..10));
        assert_eq!(sel.range_in(2, 0, 10), Some(0..10));
        assert_eq!(sel.range_in(2, 1, 10), Some(0..4));
        assert_eq!(sel.range_in(2, 2, 10), None);
        assert_eq!(sel.range_in(1, 2, 10), None);
    }

    #[test]
    fn copies_keep_markdown() {
        let ps = parts("Use **cargo** and [docs](https://d.rs).\n\n- one\n- two\n\n```sh\nls\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |");
        let mut copy = Copy::default();
        for p in &ps {
            copy.add(p, 0..p.text.text.len());
        }
        assert_eq!(copy.markdown, "Use **cargo** and [docs](https://d.rs).\n\n- one\n- two\n\n```sh\nls\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |");
        assert_eq!(copy.plain, "Use cargo and docs.\n\n• one\n• two\n\nls\n\na\tb\n1\t2");
        // Part of a bold word stays bold.
        let mut part = Copy::default();
        part.add(&ps[0], 5..9);
        assert_eq!(part.markdown, "**argo**");
    }

    #[test]
    fn selection_lies_over_styles() {
        let bold = HighlightStyle { font_weight: Some(FontWeight::BOLD), ..Default::default() };
        let sel = HighlightStyle { background_color: Some(hsla(Rgb::hex(0x336699))), ..Default::default() };
        let out = with_selection(vec![(2..6, bold)], Some(4..8), sel);
        let ranges: Vec<Range<usize>> = out.iter().map(|(r, _)| r.clone()).collect();
        assert_eq!(ranges, [2..4, 4..6, 6..8]);
        assert!(out[1].1.font_weight.is_some() && out[1].1.background_color.is_some());
        assert!(out[2].1.font_weight.is_none() && out[2].1.background_color.is_some());
    }

    #[test]
    fn words() {
        assert_eq!(word_at("let foo_bar = 1", 6), 4..11);
        assert_eq!(word_at("a b", 1), 0..1);
        // Inside a character: its start.
        assert_eq!(word_at("ăla bun", 1), 0..4);
    }
}

/// What a link to a file says: `shed-file:LINE:COL:PATH` (0: not given).
pub const FILE_LINK: &str = "shed-file:";

/// Where in a file: a line, and maybe a column.
pub type Place = Option<(usize, Option<usize>)>;

/// A file link's path and place.
pub fn parse_file_link(url: &str) -> Option<(std::path::PathBuf, Place)> {
    let rest = url.strip_prefix(FILE_LINK)?;
    let mut parts = rest.splitn(3, ':');
    let line: usize = parts.next()?.parse().ok()?;
    let col: usize = parts.next()?.parse().ok()?;
    let path = std::path::PathBuf::from(parts.next()?);
    let at = (line > 0).then(|| (line, (col > 0).then_some(col)));
    Some((path, at))
}

/// Finds the files a text names (`src/main.rs`, `src/main.rs:42:7`, `/abs/x`,
/// `~/x`), those that exist under one of `bases` or as written, and makes
/// them links (`exists` answers whether a path is a file, and is cached by
/// the caller). Text already a link is left alone.
pub fn link_files(text: &mut Text, bases: &[std::path::PathBuf], exists: &mut dyn FnMut(&std::path::Path) -> bool) {
    let breaks = |c: char| c.is_whitespace() || "\"'`<>|()[]{},;".contains(c);
    let s = text.text.clone();
    let mut found: Vec<(Range<usize>, String)> = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        let Some(c) = rest.chars().next() else { break };
        if breaks(c) {
            i += c.len_utf8();
            continue;
        }
        let len = rest.find(breaks).unwrap_or(rest.len());
        let token = &rest[..len];
        // Trailing punctuation is the sentence's, not the path's.
        let token = token.trim_end_matches(['.', ':', '!', '?']);
        let (start, end) = (i, i + token.len());
        i += len.max(1);
        // `path`, `path:line` or `path:line:col`.
        let mut pieces = token.splitn(3, ':');
        let path = pieces.next().unwrap_or("");
        let line = pieces.next().and_then(|l| l.parse::<usize>().ok()).unwrap_or(0);
        let col = pieces.next().and_then(|c| c.parse::<usize>().ok()).unwrap_or(0);
        let named = path.contains('/') || path.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && (1..=6).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric()));
        if path.len() < 3 || !named || path.contains("://") || text.links.iter().any(|(r, _)| r.start < end && start < r.end) {
            continue;
        }
        let expanded = match path.strip_prefix("~/") {
            Some(rel) => std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(rel)),
            None => Some(std::path::PathBuf::from(path)),
        };
        let Some(expanded) = expanded else { continue };
        let candidates: Vec<std::path::PathBuf> = if expanded.is_absolute() { vec![expanded] } else { bases.iter().map(|b| b.join(&expanded)).collect() };
        if let Some(abs) = candidates.into_iter().find(|p| exists(p)) {
            found.push((start..end, format!("{FILE_LINK}{line}:{col}:{}", abs.display())));
        }
    }
    for (range, url) in found {
        mark_link(text, range.clone());
        text.links.push((range, url));
    }
    text.links.sort_by_key(|(r, _)| r.start);
}

/// Sets the link style over `range`, splitting the runs it crosses.
fn mark_link(text: &mut Text, range: Range<usize>) {
    use shed_ai::markdown::Style;
    let mut cuts: Vec<usize> = text.spans.iter().flat_map(|(r, _)| [r.start, r.end]).chain([range.start, range.end]).collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut spans = Vec::new();
    for pair in cuts.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let own = text.spans.iter().find(|(r, _)| r.start <= from && to <= r.end).map(|(_, s)| *s);
        let inside = range.start <= from && to <= range.end;
        let style = match (own, inside) {
            (Some(s), true) => Some(Style { link: true, ..s }),
            (Some(s), false) => Some(s),
            (None, true) => Some(Style { link: true, ..Default::default() }),
            (None, false) => None,
        };
        if let Some(style) = style {
            spans.push((from..to, style));
        }
    }
    text.spans = spans;
}

#[cfg(test)]
mod file_tests {
    use super::*;

    #[test]
    fn named_files_become_links() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "").unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        let mut text = shed_ai::markdown::parse("See `src/main.rs:42:7` and Cargo.toml. Not src/gone.rs, nor e.g. or https://x.org/a.rs")
            .into_iter()
            .find_map(|b| if let shed_ai::markdown::Block::Paragraph(t) = b { Some(t) } else { None })
            .unwrap();
        let mut exists = |p: &std::path::Path| p.is_file();
        link_files(&mut text, &[dir.path().to_path_buf()], &mut exists);
        let named: Vec<&str> = text.links.iter().map(|(r, _)| &text.text[r.clone()]).collect();
        assert_eq!(named, ["src/main.rs:42:7", "Cargo.toml"]);
        let (path, at) = parse_file_link(&text.links[0].1).unwrap();
        assert_eq!((path, at), (dir.path().join("src/main.rs"), Some((42, Some(7)))));
        assert_eq!(parse_file_link(&text.links[1].1).unwrap().1, None);
        // The code span keeps its look and becomes a link too.
        let style = text.spans.iter().find(|(r, _)| &text.text[r.clone()] == "src/main.rs:42:7").unwrap().1;
        assert!(style.code && style.link);
    }
}
