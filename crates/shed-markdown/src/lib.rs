//! Markdown as Shed draws it: parsed (pulldown-cmark) into blocks of text
//! with styled ranges, and turned into HTML or plain text for the clipboard.
//!
//! Two flavours: Claude's answers (`parse`: CommonMark with tables,
//! strikethrough and task lists) and Markdown files (`parse_doc`: GitHub's
//! flavour, plus footnotes, math, pictures, diagrams and HTML shown as the
//! text it holds). With the `render` feature, `render` turns diagrams and
//! math into pictures.

use std::ops::Range;

use pulldown_cmark::{Alignment, BlockQuoteKind, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

#[cfg(feature = "render")]
pub mod render;

/// How a run of text is set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link: bool,
    /// TeX, as written (`$…$` in a Markdown file).
    pub math: bool,
}

/// A run of text and how its parts are set (byte ranges into `text`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Text {
    pub text: String,
    pub spans: Vec<(Range<usize>, Style)>,
    pub links: Vec<(Range<usize>, String)>,
}

impl Text {
    pub fn plain(text: impl Into<String>) -> Self {
        Text { text: text.into(), ..Default::default() }
    }

    fn push(&mut self, s: &str, style: Style) {
        let start = self.text.len();
        self.text.push_str(s);
        if style == Style::default() || s.is_empty() {
            return;
        }
        match self.spans.last_mut() {
            Some((r, st)) if *st == style && r.end == start => r.end = self.text.len(),
            _ => self.spans.push((start..self.text.len(), style)),
        }
    }

    /// `other` after this text, `sep` between.
    fn append(&mut self, sep: &str, other: Text) {
        self.text.push_str(sep);
        let at = self.text.len();
        self.text.push_str(&other.text);
        self.spans.extend(other.spans.into_iter().map(|(r, s)| (r.start + at..r.end + at, s)));
        self.links.extend(other.links.into_iter().map(|(r, u)| (r.start + at..r.end + at, u)));
    }

    /// Without the white space at its end.
    fn trim_end(&mut self) {
        let len = self.text.trim_end().len();
        self.text.truncate(len);
        for (r, _) in self.spans.iter_mut() {
            r.end = r.end.min(len);
        }
        for (r, _) in self.links.iter_mut() {
            r.end = r.end.min(len);
        }
        self.spans.retain(|(r, _)| r.start < r.end);
        self.links.retain(|(r, _)| r.start < r.end);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// A diagram's language.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiagramKind {
    /// ```mermaid
    Mermaid,
    /// ```dot or ```graphviz
    Dot,
}

impl DiagramKind {
    /// The fence's word for it.
    pub fn lang(self) -> &'static str {
        match self {
            DiagramKind::Mermaid => "mermaid",
            DiagramKind::Dot => "dot",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Paragraph(Text),
    Heading { level: u8, text: Text },
    /// A list item's text; `marker` is "•", "3." or "☐"/"☑", empty for an
    /// item's later paragraphs. `depth` counts from 0.
    Item { depth: usize, marker: String, text: Text },
    Quote(Text),
    Code { lang: String, code: String, depth: usize },
    Table { align: Vec<Align>, header: Vec<Text>, rows: Vec<Vec<Text>> },
    Rule,
    /// A picture of the file's own (`url` as written, relative to the
    /// file's folder or absolute), `alt` its words, `link` where a click
    /// on it goes. (Files only; pictures on the web show as their words.)
    Image { url: String, alt: String, link: Option<String> },
    /// A diagram drawn from its source. (Files only.)
    Diagram { kind: DiagramKind, source: String },
    /// Math on a line of its own, as TeX: `$$…$$` or ```math. (Files only.)
    Math(String),
    /// A footnote, at the end: numbered in the order they are referred to.
    /// (Files only.)
    Footnote { number: usize, label: String, text: Text },
}

/// Claude's answers: what chat Markdown has.
fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS
}

/// Markdown files: GitHub's flavour, with math and front matter.
fn doc_options() -> Options {
    options() | Options::ENABLE_FOOTNOTES | Options::ENABLE_MATH | Options::ENABLE_GFM | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
}

/// A table being read: alignments, header, rows.
type TableParts = (Vec<Align>, Vec<Text>, Vec<Vec<Text>>);

#[derive(Default)]
struct Builder {
    /// A Markdown file, not an answer: pictures, diagrams, math, footnotes
    /// and HTML are its own blocks.
    doc: bool,
    blocks: Vec<Block>,
    cur: Option<Text>,
    /// Open lists: the next number of an ordered one.
    lists: Vec<Option<u64>>,
    marker: Option<String>,
    quote: usize,
    heading: Option<u8>,
    bold: usize,
    italic: usize,
    strike: usize,
    /// Inside `<code>` or `<kbd>` (HTML in a file).
    html_code: usize,
    links: Vec<(usize, String)>,
    code: Option<(String, String)>,
    table: Option<TableParts>,
    row: Vec<Text>,
    in_cell: bool,
    /// A picture being read: its address and words.
    image: Option<(String, String)>,
    /// Front matter being read.
    metadata: Option<String>,
    /// A footnote being read: its label and text so far.
    footnote: Option<(String, Option<Text>)>,
    footnotes: Vec<(String, Text)>,
    /// Footnotes' labels in the order they are first referred to.
    referred: Vec<String>,
}

/// Whether a picture's address is on the web (or inline data), not a file.
fn is_remote(url: &str) -> bool {
    url.contains("://") || url.starts_with("//") || url.starts_with("data:") || url.starts_with("mailto:")
}

impl Builder {
    fn style(&self) -> Style {
        Style {
            bold: self.bold > 0 || self.heading.is_some(),
            italic: self.italic > 0,
            strike: self.strike > 0,
            code: self.html_code > 0,
            link: !self.links.is_empty(),
            math: false,
        }
    }

    fn text(&mut self) -> &mut Text {
        self.cur.get_or_insert_with(Text::default)
    }

    /// Ends the text under way as the block its place makes it.
    fn flush(&mut self) {
        if self.in_cell {
            return;
        }
        if self.doc
            && let Some(t) = &mut self.cur
        {
            t.trim_end();
        }
        // A footnote's paragraphs are its text.
        if let Some((_, acc)) = &mut self.footnote {
            if let Some(text) = self.cur.take().filter(|t| !t.text.is_empty()) {
                match acc {
                    Some(a) => a.append("\n", text),
                    None => *acc = Some(text),
                }
            }
            return;
        }
        let text = self.cur.take();
        let depth = self.lists.len().saturating_sub(1);
        if let Some(level) = self.heading {
            self.blocks.push(Block::Heading { level, text: text.unwrap_or_default() });
        } else if !self.lists.is_empty() {
            let marker = self.marker.take();
            if text.is_some() || marker.is_some() {
                self.blocks.push(Block::Item { depth, marker: marker.unwrap_or_default(), text: text.unwrap_or_default() });
            }
        } else if let Some(text) = text {
            if self.doc && text.text.is_empty() {
                return;
            }
            if self.quote > 0 {
                self.blocks.push(Block::Quote(text));
            } else {
                self.blocks.push(Block::Paragraph(text));
            }
        }
    }

    /// A block in the middle of a paragraph (a picture, math): the text so
    /// far ends there, and open links go on in the text after.
    fn break_for(&mut self, block: Block) {
        // (A list item's marker goes before it.)
        if self.cur.is_some() || (self.marker.is_some() && !self.lists.is_empty()) {
            self.flush();
        }
        self.blocks.push(block);
        for (start, _) in &mut self.links {
            *start = 0;
        }
    }

    /// A picture: a file's own is a block of its own (where it can be);
    /// one on the web, or in a heading or a table, is its words.
    fn image(&mut self, url: String, alt: String) {
        let alt = alt.trim().to_string();
        if !is_remote(&url) && !self.in_cell && self.heading.is_none() && self.footnote.is_none() {
            let link = self.links.last().map(|(_, u)| u.clone());
            return self.break_for(Block::Image { url, alt, link });
        }
        let words = if alt.is_empty() { "picture".to_string() } else { alt };
        let style = Style { link: true, ..self.style() };
        let at = self.text().text.len();
        self.text().push(&words, style);
        // Inside a link, that link is where it goes; else the picture itself.
        if self.links.is_empty() {
            let end = self.text().text.len();
            self.text().links.push((at..end, url));
        }
    }

    /// HTML in a file: its words, with what its tags say about them
    /// (breaks, bold, code, pictures); everything else in it is left out.
    fn html(&mut self, html: &str) {
        let mut rest = html;
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix("<!--") {
                rest = after.find("-->").map_or("", |i| &after[i + 3..]);
                continue;
            }
            if rest.starts_with('<')
                && let Some(end) = rest.find('>')
                && rest[1..].starts_with(|c: char| c.is_ascii_alphabetic() || c == '/')
            {
                self.html_tag(&rest[1..end]);
                rest = &rest[end + 1..];
                continue;
            }
            let next = rest.char_indices().skip(1).find(|(_, c)| *c == '<').map_or(rest.len(), |(i, _)| i);
            let words = decode_entities(&rest[..next]);
            let style = self.style();
            for c in words.chars() {
                let t = self.text();
                if c.is_whitespace() {
                    if !t.text.is_empty() && !t.text.ends_with([' ', '\n']) {
                        t.push(" ", style);
                    }
                } else {
                    let mut buf = [0u8; 4];
                    t.push(c.encode_utf8(&mut buf), style);
                }
            }
            rest = &rest[next..];
        }
    }

    fn html_tag(&mut self, tag: &str) {
        let closing = tag.starts_with('/');
        let tag = tag.trim_start_matches('/').trim_end_matches('/');
        let name = tag.split(|c: char| c.is_whitespace()).next().unwrap_or("").to_ascii_lowercase();
        let count = |n: &mut usize| {
            if closing {
                *n = n.saturating_sub(1);
            } else {
                *n += 1;
            }
        };
        match name.as_str() {
            "b" | "strong" => count(&mut self.bold),
            "i" | "em" => count(&mut self.italic),
            "s" | "del" | "strike" => count(&mut self.strike),
            "code" | "kbd" | "tt" => count(&mut self.html_code),
            "img" if !closing => {
                if let Some(src) = attr(tag, "src") {
                    let alt = attr(tag, "alt").unwrap_or_default();
                    self.image(src, alt);
                }
            }
            "br" | "p" | "div" | "li" | "tr" | "summary" | "details" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "hr" | "table" | "ul" | "ol" | "blockquote" | "pre" => {
                if let Some(t) = &mut self.cur
                    && !t.text.is_empty()
                    && !t.text.ends_with('\n')
                {
                    let len = t.text.trim_end_matches(' ').len();
                    t.text.truncate(len);
                    t.push("\n", Style::default());
                }
            }
            _ => {}
        }
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Heading { level, .. } => {
                    self.flush();
                    self.heading = Some(level as u8);
                }
                Tag::BlockQuote(kind) => {
                    self.flush();
                    self.quote += 1;
                    // GitHub's alerts: `> [!NOTE]` and the like.
                    if let Some(kind) = kind {
                        let label = match kind {
                            BlockQuoteKind::Note => "Note",
                            BlockQuoteKind::Tip => "Tip",
                            BlockQuoteKind::Important => "Important",
                            BlockQuoteKind::Warning => "Warning",
                            BlockQuoteKind::Caution => "Caution",
                        };
                        self.text().push(label, Style { bold: true, ..Default::default() });
                        self.text().push("\n", Style::default());
                    }
                }
                Tag::CodeBlock(kind) => {
                    self.flush();
                    let lang = match kind {
                        CodeBlockKind::Fenced(l) => l.split_whitespace().next().unwrap_or("").to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    self.code = Some((lang, String::new()));
                }
                Tag::MetadataBlock(_) => {
                    self.flush();
                    self.metadata = Some(String::new());
                }
                Tag::HtmlBlock => self.flush(),
                Tag::FootnoteDefinition(label) => {
                    self.flush();
                    self.footnote = Some((label.to_string(), None));
                }
                Tag::List(start) => {
                    self.flush();
                    self.lists.push(start);
                }
                Tag::Item => {
                    self.flush();
                    let marker = match self.lists.last_mut() {
                        Some(Some(n)) => {
                            *n += 1;
                            format!("{}.", *n - 1)
                        }
                        _ => "•".to_string(),
                    };
                    self.marker = Some(marker);
                }
                Tag::Table(align) => {
                    self.flush();
                    let align = align
                        .into_iter()
                        .map(|a| match a {
                            Alignment::Center => Align::Center,
                            Alignment::Right => Align::Right,
                            _ => Align::Left,
                        })
                        .collect();
                    self.table = Some((align, Vec::new(), Vec::new()));
                }
                Tag::TableHead | Tag::TableRow => self.row.clear(),
                Tag::TableCell => {
                    self.in_cell = true;
                    self.cur = Some(Text::default());
                }
                Tag::Emphasis => self.italic += 1,
                Tag::Strong => self.bold += 1,
                Tag::Strikethrough => self.strike += 1,
                Tag::Link { dest_url, .. } => {
                    let at = self.text().text.len();
                    self.links.push((at, dest_url.to_string()));
                }
                Tag::Image { dest_url, .. } if self.doc => self.image = Some((dest_url.to_string(), String::new())),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => self.flush(),
                TagEnd::Heading(_) => {
                    self.flush();
                    self.heading = None;
                }
                TagEnd::BlockQuote(_) => {
                    self.flush();
                    self.quote = self.quote.saturating_sub(1);
                }
                TagEnd::CodeBlock => {
                    if let Some((lang, mut code)) = self.code.take() {
                        if code.ends_with('\n') {
                            code.pop();
                        }
                        let depth = self.lists.len();
                        // An item that starts with code keeps its marker in front.
                        if let Some(marker) = self.marker.take() {
                            self.blocks.push(Block::Item { depth: depth.saturating_sub(1), marker, text: Text::default() });
                        }
                        let block = match lang.to_ascii_lowercase().as_str() {
                            "mermaid" if self.doc => Block::Diagram { kind: DiagramKind::Mermaid, source: code },
                            "dot" | "graphviz" | "gv" if self.doc => Block::Diagram { kind: DiagramKind::Dot, source: code },
                            "math" | "latex" | "tex" if self.doc => Block::Math(code),
                            _ => Block::Code { lang, code, depth },
                        };
                        self.blocks.push(block);
                    }
                }
                TagEnd::MetadataBlock(_) => {
                    if let Some(mut code) = self.metadata.take() {
                        if code.ends_with('\n') {
                            code.pop();
                        }
                        self.blocks.push(Block::Code { lang: "yaml".into(), code, depth: 0 });
                    }
                }
                TagEnd::HtmlBlock => self.flush(),
                TagEnd::FootnoteDefinition => {
                    self.flush();
                    if let Some((label, text)) = self.footnote.take() {
                        self.footnotes.push((label, text.unwrap_or_default()));
                    }
                }
                TagEnd::List(_) => {
                    self.flush();
                    self.lists.pop();
                }
                TagEnd::Item => self.flush(),
                TagEnd::TableCell => {
                    self.in_cell = false;
                    let mut cell = self.cur.take().unwrap_or_default();
                    if self.doc {
                        cell.trim_end();
                    }
                    self.row.push(cell);
                }
                TagEnd::TableHead => {
                    if let Some(t) = &mut self.table {
                        t.1 = std::mem::take(&mut self.row);
                    }
                }
                TagEnd::TableRow => {
                    if let Some(t) = &mut self.table {
                        t.2.push(std::mem::take(&mut self.row));
                    }
                }
                TagEnd::Table => {
                    if let Some((align, header, rows)) = self.table.take() {
                        self.blocks.push(Block::Table { align, header, rows });
                    }
                }
                TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
                TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
                TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
                TagEnd::Link => {
                    if let Some((start, url)) = self.links.pop() {
                        let end = self.text().text.len();
                        if end > start {
                            self.text().links.push((start..end, url));
                        }
                    }
                }
                TagEnd::Image => {
                    if let Some((url, alt)) = self.image.take() {
                        self.image(url, alt);
                    }
                }
                _ => {}
            },
            Event::Text(t) => {
                if let Some((_, code)) = &mut self.code {
                    code.push_str(&t);
                } else if let Some(meta) = &mut self.metadata {
                    meta.push_str(&t);
                } else if let Some((_, alt)) = &mut self.image {
                    alt.push_str(&t);
                } else {
                    let style = self.style();
                    self.text().push(&t, style);
                }
            }
            Event::Code(t) => {
                if let Some((_, alt)) = &mut self.image {
                    alt.push_str(&t);
                    return;
                }
                let style = Style { code: true, ..self.style() };
                self.text().push(&t, style);
            }
            Event::InlineMath(t) => {
                if let Some((_, alt)) = &mut self.image {
                    alt.push_str(&t);
                    return;
                }
                let style = Style { math: true, ..self.style() };
                self.text().push(&t, style);
            }
            Event::DisplayMath(t) => {
                if self.in_cell || self.heading.is_some() || self.footnote.is_some() {
                    let style = Style { math: true, ..self.style() };
                    self.text().push(&t, style);
                } else {
                    self.break_for(Block::Math(t.trim().to_string()));
                }
            }
            Event::Html(t) | Event::InlineHtml(t) => {
                if let Some((_, code)) = &mut self.code {
                    code.push_str(&t);
                } else if self.doc {
                    self.html(&t);
                } else {
                    let style = self.style();
                    self.text().push(t.trim_end_matches('\n'), style);
                }
            }
            Event::FootnoteReference(label) => {
                let label = label.to_string();
                let number = match self.referred.iter().position(|l| *l == label) {
                    Some(i) => i + 1,
                    None => {
                        self.referred.push(label.clone());
                        self.referred.len()
                    }
                };
                let style = Style { link: true, ..self.style() };
                let at = self.text().text.len();
                self.text().push(&format!("[{number}]"), style);
                let end = self.text().text.len();
                self.text().links.push((at..end, format!("#fn-{label}")));
            }
            Event::SoftBreak => {
                if self.image.is_some() {
                    return;
                }
                let style = self.style();
                self.text().push(" ", style);
            }
            Event::HardBreak => self.text().push("\n", Style::default()),
            Event::Rule => {
                self.flush();
                self.blocks.push(Block::Rule);
            }
            Event::TaskListMarker(done) => {
                self.marker = Some(if done { "☑" } else { "☐" }.to_string());
            }
        }
    }

    fn finish(mut self) -> Vec<Block> {
        self.flush();
        if let Some((lang, code)) = self.code.take() {
            self.blocks.push(Block::Code { lang, code, depth: 0 });
        }
        // Footnotes at the end, in the order they are referred to (as
        // GitHub shows them: one never referred to is left out).
        for (n, label) in self.referred.iter().enumerate() {
            if let Some(i) = self.footnotes.iter().position(|(l, _)| l == label) {
                let (label, text) = self.footnotes.swap_remove(i);
                self.blocks.push(Block::Footnote { number: n + 1, label, text });
            }
        }
        self.blocks
    }
}

/// The value of attribute `name` in an HTML tag's inside (`img src="x"`).
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        from = at + name.len();
        // A whole attribute name, followed by `=`.
        if at > 0 && !lower.as_bytes()[at - 1].is_ascii_whitespace() {
            continue;
        }
        let rest = tag[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else { continue };
        let rest = rest.trim_start();
        let value = match rest.chars().next() {
            Some(q @ ('"' | '\'')) => rest[1..].split(q).next().unwrap_or(""),
            _ => rest.split(|c: char| c.is_whitespace() || c == '>').next().unwrap_or(""),
        };
        return Some(decode_entities(value));
    }
    None
}

/// HTML's common character references, as the characters.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest.find(';').filter(|e| *e <= 10);
        let named = end.map(|e| &rest[1..e]);
        let c = match named {
            Some("amp") => Some('&'),
            Some("lt") => Some('<'),
            Some("gt") => Some('>'),
            Some("quot") => Some('"'),
            Some("apos") => Some('\''),
            Some("nbsp") => Some('\u{a0}'),
            Some("copy") => Some('©'),
            Some("mdash") => Some('—'),
            Some("ndash") => Some('–'),
            Some(n) if n.starts_with("#x") || n.starts_with("#X") => u32::from_str_radix(&n[2..], 16).ok().and_then(char::from_u32),
            Some(n) if n.starts_with('#') => n[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match (c, end) {
            (Some(c), Some(e)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The blocks of an answer of Claude's, in order.
pub fn parse(markdown: &str) -> Vec<Block> {
    let mut b = Builder::default();
    for event in Parser::new_ext(markdown, options()) {
        b.event(event);
    }
    b.finish()
}

/// The blocks of a Markdown file, in order: GitHub's flavour, with
/// pictures, diagrams, math and footnotes as blocks of their own, and
/// HTML as the words it holds.
pub fn parse_doc(markdown: &str) -> Vec<Block> {
    let mut b = Builder { doc: true, ..Default::default() };
    for event in Parser::new_ext(markdown, doc_options()) {
        b.event(event);
    }
    b.finish()
}

/// A heading's anchor as GitHub makes it: lower case, punctuation left
/// out, spaces as hyphens (`## Quick start!` → `quick-start`).
pub fn slug(heading: &str) -> String {
    heading
        .trim()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            '-' | '_' => Some(c),
            c if c.is_alphanumeric() => Some(c),
            _ => None,
        })
        .flat_map(char::to_lowercase)
        .collect()
}

/// Where a document's `#anchors` go: each heading's (repeated ones get
/// `-1`, `-2`, as on GitHub) and each footnote's (`fn-<label>`), with
/// the index of the block.
pub fn anchors(blocks: &[Block]) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        match block {
            Block::Heading { text, .. } => {
                let base = slug(&text.text);
                let mut name = base.clone();
                let mut n = 0;
                while out.iter().any(|(a, _)| *a == name) {
                    n += 1;
                    name = format!("{base}-{n}");
                }
                out.push((name, i));
            }
            Block::Footnote { label, .. } => out.push((format!("fn-{label}"), i)),
            _ => {}
        }
    }
    out
}

/// The text as HTML, for pasting where formatting is kept.
pub fn to_html(markdown: &str) -> String {
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, Parser::new_ext(markdown, options()));
    html
}

/// The text without Markdown marks: what reads well where formatting is lost.
pub fn to_plain(markdown: &str) -> String {
    let mut out = String::new();
    let mut last_item = false;
    for block in parse(markdown) {
        let item = matches!(block, Block::Item { .. });
        if !out.is_empty() {
            out.push_str(if item && last_item { "\n" } else { "\n\n" });
        }
        last_item = item;
        match block {
            Block::Paragraph(t) | Block::Quote(t) | Block::Heading { text: t, .. } => out.push_str(&t.text),
            Block::Item { depth, marker, text } => {
                out.push_str(&"  ".repeat(depth));
                if !marker.is_empty() {
                    out.push_str(&marker);
                    out.push(' ');
                }
                out.push_str(&text.text);
            }
            Block::Code { code, .. } | Block::Diagram { source: code, .. } | Block::Math(code) => out.push_str(&code),
            Block::Table { header, rows, .. } => {
                let line = |cells: &[Text]| cells.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join("\t");
                out.push_str(&line(&header));
                for row in &rows {
                    out.push('\n');
                    out.push_str(&line(row));
                }
            }
            Block::Rule => out.push_str("———"),
            Block::Image { alt, .. } => out.push_str(&alt),
            Block::Footnote { number, text, .. } => out.push_str(&format!("[{number}] {}", text.text)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_and_styles() {
        let md = "# Title\n\nSome **bold** and *it* with `code` and [a link](https://x.y).\n\n- one\n- two\n  1. nested\n\n```rust\nfn main() {}\n```\n\n> quoted\n\n---\n";
        let blocks = parse(md);
        assert_eq!(blocks[0], Block::Heading { level: 1, text: Text { text: "Title".into(), spans: vec![(0..5, Style { bold: true, ..Default::default() })], links: vec![] } });
        let Block::Paragraph(p) = &blocks[1] else { panic!("{:?}", blocks[1]) };
        assert_eq!(p.text, "Some bold and it with code and a link.");
        let style_of = |word: &str| p.spans.iter().find(|(r, _)| &p.text[r.clone()] == word).map(|(_, s)| *s);
        assert!(style_of("bold").unwrap().bold);
        assert!(style_of("it").unwrap().italic);
        assert!(style_of("code").unwrap().code);
        assert!(style_of("a link").unwrap().link);
        assert_eq!(p.links, vec![(31..37, "https://x.y".to_string())]);
        assert!(matches!(&blocks[2], Block::Item { depth: 0, marker, text } if marker == "•" && text.text == "one"));
        assert!(matches!(&blocks[3], Block::Item { depth: 0, text, .. } if text.text == "two"));
        assert!(matches!(&blocks[4], Block::Item { depth: 1, marker, text } if marker == "1." && text.text == "nested"));
        assert_eq!(blocks[5], Block::Code { lang: "rust".into(), code: "fn main() {}".into(), depth: 0 });
        assert!(matches!(&blocks[6], Block::Quote(t) if t.text == "quoted"));
        assert_eq!(blocks[7], Block::Rule);
        assert_eq!(blocks.len(), 8);
    }

    #[test]
    fn tables_and_tasks() {
        let blocks = parse("| a | b |\n|:--|--:|\n| 1 | **2** |\n\n- [x] done\n- [ ] todo\n");
        let Block::Table { align, header, rows } = &blocks[0] else { panic!("{blocks:?}") };
        assert_eq!(align, &[Align::Left, Align::Right]);
        assert_eq!(header.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(rows[0][1].text, "2");
        assert!(rows[0][1].spans[0].1.bold);
        assert!(matches!(&blocks[1], Block::Item { marker, text, .. } if marker == "☑" && text.text == "done"));
        assert!(matches!(&blocks[2], Block::Item { marker, .. } if marker == "☐"));
    }

    #[test]
    fn clipboard_forms() {
        let md = "Hi **there**\n\n- a\n- b\n\n```\nx = 1\n```";
        assert_eq!(to_plain(md), "Hi there\n\n• a\n• b\n\nx = 1");
        let html = to_html(md);
        assert!(html.contains("<strong>there</strong>") && html.contains("<li>a</li>") && html.contains("<code>x = 1"));
    }

    #[test]
    fn unfinished_code_still_shows() {
        // While Claude is still writing it.
        assert_eq!(parse("```sh\nls -l\n"), vec![Block::Code { lang: "sh".into(), code: "ls -l".into(), depth: 0 }]);
    }

    fn doc_para(md: &str) -> Text {
        match parse_doc(md).into_iter().next() {
            Some(Block::Paragraph(t)) => t,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn answers_keep_diagrams_math_and_html_as_written() {
        // Claude's answers are parsed as before: a mermaid block is code,
        // dollars are dollars, HTML is its text.
        let blocks = parse("```mermaid\ngraph TD; A-->B\n```\n\nCosts $5 and $6, <b>bold</b>[^1]\n\n[^1]: note\n");
        assert_eq!(blocks[0], Block::Code { lang: "mermaid".into(), code: "graph TD; A-->B".into(), depth: 0 });
        let Block::Paragraph(p) = &blocks[1] else { panic!("{blocks:?}") };
        assert_eq!(p.text, "Costs $5 and $6, <b>bold</b>^1");
        assert!(!blocks.iter().any(|b| matches!(b, Block::Footnote { .. } | Block::Math(_) | Block::Diagram { .. })));
    }

    #[test]
    fn files_have_diagrams_and_math_blocks() {
        let blocks = parse_doc("```mermaid\nflowchart LR\n  A-->B\n```\n\n```dot\ndigraph { a -> b }\n```\n\n```graphviz\ndigraph { c }\n```\n\n$$\nE = mc^2\n$$\n\n```math\n\\frac{1}{2}\n```\n\nInline $x^2$ here.\n");
        assert_eq!(blocks[0], Block::Diagram { kind: DiagramKind::Mermaid, source: "flowchart LR\n  A-->B".into() });
        assert_eq!(blocks[1], Block::Diagram { kind: DiagramKind::Dot, source: "digraph { a -> b }".into() });
        assert!(matches!(&blocks[2], Block::Diagram { kind: DiagramKind::Dot, .. }));
        assert_eq!(blocks[3], Block::Math("E = mc^2".into()));
        assert_eq!(blocks[4], Block::Math("\\frac{1}{2}".into()));
        let Block::Paragraph(p) = &blocks[5] else { panic!("{blocks:?}") };
        assert_eq!(p.text, "Inline x^2 here.");
        let math = p.spans.iter().find(|(_, s)| s.math).expect("math span");
        assert_eq!(&p.text[math.0.clone()], "x^2");
    }

    #[test]
    fn display_math_inside_a_paragraph_breaks_it() {
        let blocks = parse_doc("Before $$a+b$$ after.");
        assert!(matches!(&blocks[0], Block::Paragraph(t) if t.text == "Before"), "{blocks:?}");
        assert_eq!(blocks[1], Block::Math("a+b".into()));
        assert!(matches!(&blocks[2], Block::Paragraph(t) if t.text == " after." || t.text == "after."), "{blocks:?}");
    }

    #[test]
    fn local_pictures_are_blocks_and_web_ones_their_words() {
        let blocks = parse_doc("Look: ![the logo](docs/logo.png \"Logo\") and [![CI](https://ci.example/badge.svg)](https://ci.example)\n\n[![shot](shot.webp)](https://shed.dev)\n");
        assert!(matches!(&blocks[0], Block::Paragraph(t) if t.text == "Look:"), "{blocks:?}");
        assert_eq!(blocks[1], Block::Image { url: "docs/logo.png".into(), alt: "the logo".into(), link: None });
        let Block::Paragraph(after) = &blocks[2] else { panic!("{blocks:?}") };
        assert_eq!(after.text.trim(), "and CI");
        // The badge's words link where its link goes.
        let (r, url) = &after.links[0];
        assert_eq!((&after.text[r.clone()], url.as_str()), ("CI", "https://ci.example"));
        assert_eq!(blocks[3], Block::Image { url: "shot.webp".into(), alt: "shot".into(), link: Some("https://shed.dev".into()) });
        assert_eq!(blocks.len(), 4);
        // A picture on its own on the web: its words, linking to it.
        let t = doc_para("![diagram](https://x.org/a.png)");
        assert_eq!(t.text, "diagram");
        assert_eq!(t.links, vec![(0..7, "https://x.org/a.png".to_string())]);
    }

    #[test]
    fn footnotes_come_at_the_end_in_order_of_reference() {
        let blocks = parse_doc("One[^b] and two[^a], again[^b].\n\n[^a]: First *note*.\n[^b]: Second.\n\n[^c]: Never used.\n\nEnd.\n");
        let Block::Paragraph(p) = &blocks[0] else { panic!("{blocks:?}") };
        assert_eq!(p.text, "One[1] and two[2], again[1].");
        let urls: Vec<&str> = p.links.iter().map(|(_, u)| u.as_str()).collect();
        assert_eq!(urls, ["#fn-b", "#fn-a", "#fn-b"]);
        assert!(matches!(&blocks[1], Block::Paragraph(t) if t.text == "End."), "{blocks:?}");
        assert!(matches!(&blocks[2], Block::Footnote { number: 1, label, text } if label == "b" && text.text == "Second."), "{blocks:?}");
        let Block::Footnote { number: 2, text, .. } = &blocks[3] else { panic!("{blocks:?}") };
        assert_eq!(text.text, "First note.");
        assert!(text.spans[0].1.italic);
        assert_eq!(blocks.len(), 4);
    }

    #[test]
    fn anchors_are_githubs() {
        assert_eq!(slug("Quick start!"), "quick-start");
        assert_eq!(slug("What's `new` in 2.0?"), "whats-new-in-20");
        assert_eq!(slug("Ünïcode & more"), "ünïcode--more");
        let blocks = parse_doc("# Intro\n\nText[^n].\n\n## Usage\n\n## Usage\n\n### Usage\n\n[^n]: A note.\n");
        let anchors = anchors(&blocks);
        let names: Vec<&str> = anchors.iter().map(|(a, _)| a.as_str()).collect();
        assert_eq!(names, ["intro", "usage", "usage-1", "usage-2", "fn-n"]);
        assert!(matches!(blocks[anchors[2].1], Block::Heading { level: 2, .. }));
        assert!(matches!(blocks[anchors[4].1], Block::Footnote { .. }));
    }

    #[test]
    fn html_shows_as_the_words_it_holds() {
        let blocks = parse_doc("<p align=\"center\">\n  <img src=\"docs/banner.png\" alt=\"Banner\" width=\"600\">\n</p>\n\n<!-- a comment -->\n\n<details>\n<summary>More &amp; less</summary>\n\nHidden *text*.\n\n</details>\n\nPress <kbd>Ctrl</kbd>+<kbd>C</kbd><br>to copy &lt;3\n");
        assert_eq!(blocks[0], Block::Image { url: "docs/banner.png".into(), alt: "Banner".into(), link: None });
        assert!(matches!(&blocks[1], Block::Paragraph(t) if t.text == "More & less"), "{blocks:?}");
        assert!(matches!(&blocks[2], Block::Paragraph(t) if t.text == "Hidden text."), "{blocks:?}");
        let Block::Paragraph(p) = &blocks[3] else { panic!("{blocks:?}") };
        assert_eq!(p.text, "Press Ctrl+C\nto copy <3");
        assert!(p.spans.iter().any(|(r, s)| s.code && &p.text[r.clone()] == "Ctrl"));
        assert_eq!(blocks.len(), 4, "{blocks:?}");
    }

    #[test]
    fn odd_html_never_breaks_parsing() {
        for md in ["<", "<img", "<img src=>", "a <b c", "<div>ünï<b>cöde</div>", "&#xZZ; &#99999999; &", "<!-- open", "<details><summary>x"] {
            let _ = parse_doc(md);
        }
        assert_eq!(attr("img src='a b.png' alt=x", "src").as_deref(), Some("a b.png"));
        assert_eq!(attr("img data-src=\"no\" src=\"yes\"", "src").as_deref(), Some("yes"));
        assert_eq!(decode_entities("&amp;&#65;&#x42;&bogus;"), "&AB&bogus;");
    }

    #[test]
    fn github_alerts_and_front_matter() {
        let blocks = parse_doc("---\ntitle: Shed\n---\n\n> [!WARNING]\n> Mind the gap.\n");
        assert_eq!(blocks[0], Block::Code { lang: "yaml".into(), code: "title: Shed".into(), depth: 0 });
        let Block::Quote(q) = &blocks[1] else { panic!("{blocks:?}") };
        assert_eq!(q.text, "Warning\nMind the gap.");
        assert!(q.spans[0].1.bold);
    }
}
