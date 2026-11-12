//! Claude's answers are Markdown: parsed here into blocks to draw (text with
//! styled ranges), and turned into HTML or plain text for the clipboard.

use std::ops::Range;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

/// How a run of text is set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link: bool,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
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
}

fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS
}

/// A table being read: alignments, header, rows.
type TableParts = (Vec<Align>, Vec<Text>, Vec<Vec<Text>>);

#[derive(Default)]
struct Builder {
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
    links: Vec<(usize, String)>,
    code: Option<(String, String)>,
    table: Option<TableParts>,
    row: Vec<Text>,
    in_cell: bool,
}

impl Builder {
    fn style(&self) -> Style {
        Style { bold: self.bold > 0 || self.heading.is_some(), italic: self.italic > 0, strike: self.strike > 0, code: false, link: !self.links.is_empty() }
    }

    fn text(&mut self) -> &mut Text {
        self.cur.get_or_insert_with(Text::default)
    }

    /// Ends the text under way as the block its place makes it.
    fn flush(&mut self) {
        if self.in_cell {
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
            if self.quote > 0 {
                self.blocks.push(Block::Quote(text));
            } else {
                self.blocks.push(Block::Paragraph(text));
            }
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
                Tag::BlockQuote(_) => {
                    self.flush();
                    self.quote += 1;
                }
                Tag::CodeBlock(kind) => {
                    self.flush();
                    let lang = match kind {
                        CodeBlockKind::Fenced(l) => l.split_whitespace().next().unwrap_or("").to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    self.code = Some((lang, String::new()));
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
                        self.blocks.push(Block::Code { lang, code, depth });
                    }
                }
                TagEnd::List(_) => {
                    self.flush();
                    self.lists.pop();
                }
                TagEnd::Item => self.flush(),
                TagEnd::TableCell => {
                    self.in_cell = false;
                    let cell = self.cur.take().unwrap_or_default();
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
                _ => {}
            },
            Event::Text(t) => {
                if let Some((_, code)) = &mut self.code {
                    code.push_str(&t);
                } else {
                    let style = self.style();
                    self.text().push(&t, style);
                }
            }
            Event::Code(t) => {
                let style = Style { code: true, ..self.style() };
                self.text().push(&t, style);
            }
            Event::Html(t) | Event::InlineHtml(t) => {
                if let Some((_, code)) = &mut self.code {
                    code.push_str(&t);
                } else {
                    let style = self.style();
                    self.text().push(t.trim_end_matches('\n'), style);
                }
            }
            Event::SoftBreak => {
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
            _ => {}
        }
    }
}

/// The blocks of a Markdown text, in order.
pub fn parse(markdown: &str) -> Vec<Block> {
    let mut b = Builder::default();
    for event in Parser::new_ext(markdown, options()) {
        b.event(event);
    }
    b.flush();
    if let Some((lang, code)) = b.code.take() {
        b.blocks.push(Block::Code { lang, code, depth: 0 });
    }
    b.blocks
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
            Block::Code { code, .. } => out.push_str(&code),
            Block::Table { header, rows, .. } => {
                let line = |cells: &[Text]| cells.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join("\t");
                out.push_str(&line(&header));
                for row in &rows {
                    out.push('\n');
                    out.push_str(&line(row));
                }
            }
            Block::Rule => out.push_str("———"),
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
}
