//! Claude's Markdown as styled terminal lines: `shed_ai::markdown` parses it
//! into blocks; here they are set in bold, italics, colours and indents,
//! wrapped to the width, with code coloured as the file viewer colours it.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use shed_ai::markdown::{self, Align, Block, Text};

use crate::text::{fit, width, wrap};
use crate::theme::Colors;

/// `markdown` as lines of at most `cols` columns.
pub fn lines(markdown: &str, cols: usize, colors: &Colors) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let blocks = markdown::parse(markdown);
    for (i, block) in blocks.iter().enumerate() {
        // A blank line between blocks, but not between items of a list.
        let tight = matches!((i.checked_sub(1).map(|p| &blocks[p]), block), (Some(Block::Item { .. }), Block::Item { .. }));
        if i > 0 && !tight {
            out.push(Line::default());
        }
        block_lines(block, cols, colors, &mut out);
    }
    out
}

fn block_lines(block: &Block, cols: usize, colors: &Colors, out: &mut Vec<Line<'static>>) {
    match block {
        Block::Paragraph(t) => out.extend(wrap(spans(t, Style::new(), colors), cols, vec![], vec![])),
        Block::Heading { level, text } => {
            let mut style = colors.accent().add_modifier(Modifier::BOLD);
            if *level > 2 {
                style = Style::new().add_modifier(Modifier::BOLD);
            }
            out.extend(wrap(spans(text, style, colors), cols, vec![], vec![]));
        }
        Block::Item { depth, marker, text } => {
            let indent = "  ".repeat(*depth);
            let mark = if marker.is_empty() { "  ".to_string() } else { format!("{marker} ") };
            let hang = " ".repeat(width(&mark));
            let first = vec![Span::raw(indent.clone()), Span::styled(mark, colors.accent())];
            let rest = vec![Span::raw(format!("{indent}{hang}"))];
            out.extend(wrap(spans(text, Style::new(), colors), cols, first, rest));
        }
        Block::Quote(t) => {
            let bar = || vec![Span::styled("│ ", colors.dim())];
            out.extend(wrap(spans(t, Style::new().add_modifier(Modifier::ITALIC), colors), cols, bar(), bar()));
        }
        Block::Code { lang, code, depth } => {
            let indent = "  ".repeat(*depth + 1);
            let colored = shed_project::by_name(lang).and_then(|l| shed_project::highlight(l, code));
            let code = code.strip_suffix('\n').unwrap_or(code);
            for (n, line) in code.split('\n').enumerate() {
                let mut parts = vec![Span::raw(indent.clone())];
                match colored.as_ref().and_then(|c| c.get(n)) {
                    Some(ranges) => parts.extend(colored_line(line, ranges, colors)),
                    None => parts.push(Span::styled(line.to_string(), colors.fg(colors.ansi[3]))),
                }
                out.push(cut(parts, cols));
            }
        }
        Block::Table { align, header, rows } => table(align, header, rows, cols, colors, out),
        Block::Rule => out.push(Line::styled("─".repeat(cols.min(60)), colors.dim())),
    }
}

/// A code line coloured by its highlight ranges (byte ranges in the line).
pub fn colored_line(line: &str, ranges: &[(std::ops::Range<usize>, shed_project::Style)], colors: &Colors) -> Vec<Span<'static>> {
    let mut parts = Vec::new();
    let mut at = 0;
    for (r, style) in ranges {
        let (start, end) = (r.start.min(line.len()), r.end.min(line.len()));
        if start < at || !line.is_char_boundary(start) || !line.is_char_boundary(end) {
            continue;
        }
        if start > at {
            parts.push(Span::raw(line[at..start].to_string()));
        }
        parts.push(Span::styled(line[start..end].to_string(), colors.code(*style)));
        at = end;
    }
    if at < line.len() {
        parts.push(Span::raw(line[at..].to_string()));
    }
    parts
}

/// A line cut to `cols` columns.
fn cut(parts: Vec<Span<'static>>, cols: usize) -> Line<'static> {
    let mut used = 0;
    let mut kept = Vec::new();
    for p in parts {
        let w = width(&p.content);
        if used + w <= cols {
            used += w;
            kept.push(p);
        } else {
            let left = cols.saturating_sub(used);
            if left > 0 {
                kept.push(Span::styled(fit(&p.content, left), p.style));
            }
            break;
        }
    }
    Line::from(kept)
}

/// A text's runs as spans, each set over `base`.
fn spans(t: &Text, base: Style, colors: &Colors) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut at = 0;
    let style_of = |s: &markdown::Style| {
        let mut st = base;
        if s.bold {
            st = st.add_modifier(Modifier::BOLD);
        }
        if s.italic {
            st = st.add_modifier(Modifier::ITALIC);
        }
        if s.strike {
            st = st.add_modifier(Modifier::CROSSED_OUT);
        }
        if s.code {
            st = st.patch(colors.fg(colors.ansi[3]));
        }
        if s.link {
            st = st.add_modifier(Modifier::UNDERLINED).patch(colors.fg(colors.ansi[4]));
        }
        st
    };
    for (r, s) in &t.spans {
        if r.start > at {
            out.push(Span::styled(t.text[at..r.start].to_string(), base));
        }
        out.push(Span::styled(t.text[r.clone()].to_string(), style_of(s)));
        at = r.end;
    }
    if at < t.text.len() {
        out.push(Span::styled(t.text[at..].to_string(), base));
    }
    out
}

fn table(align: &[Align], header: &[Text], rows: &[Vec<Text>], cols: usize, colors: &Colors, out: &mut Vec<Line<'static>>) {
    let n = header.len().max(rows.iter().map(Vec::len).max().unwrap_or(0));
    if n == 0 {
        return;
    }
    let mut widths = vec![0usize; n];
    for row in std::iter::once(header).chain(rows.iter().map(Vec::as_slice)) {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(width(&cell.text));
        }
    }
    // Shrink the widest columns until the table fits (3 columns per separator).
    let room = cols.saturating_sub(3 * (n - 1)).max(n);
    while widths.iter().sum::<usize>() > room {
        let widest = (0..n).max_by_key(|i| widths[*i]).unwrap();
        if widths[widest] <= 3 {
            break;
        }
        widths[widest] -= 1;
    }
    let row_line = |cells: &[Text], style: Style| {
        let mut parts = Vec::new();
        for (i, cell_w) in widths.iter().enumerate() {
            if i > 0 {
                parts.push(Span::styled(" │ ", colors.dim()));
            }
            let text = cells.get(i).map_or("", |c| c.text.as_str());
            let text = fit(text, *cell_w);
            let gap = cell_w - width(&text);
            let (l, r) = match align.get(i) {
                Some(Align::Right) => (gap, 0),
                Some(Align::Center) => (gap / 2, gap - gap / 2),
                _ => (0, gap),
            };
            parts.push(Span::styled(format!("{}{text}{}", " ".repeat(l), " ".repeat(r)), style));
        }
        Line::from(parts)
    };
    out.push(row_line(header, Style::new().add_modifier(Modifier::BOLD)));
    let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
    out.push(Line::styled(rule.join("─┼─"), colors.dim()));
    for row in rows {
        out.push(row_line(row, Style::new()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shed_model::config::Theme;

    fn colors() -> Colors {
        Colors::from_theme(&Theme::load("shed-dark").unwrap(), false)
    }

    fn plain(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect()
    }

    #[test]
    fn headings_paragraphs_and_emphasis() {
        let c = colors();
        let lines = lines("# Title\n\nSome **bold** and `code` here.", 80, &c);
        assert_eq!(plain(&lines), ["Title", "", "Some bold and code here."]);
        assert!(lines[0].spans[0].style.add_modifier.contains(Modifier::BOLD));
        let bold = lines[2].spans.iter().find(|s| s.content == "bold").unwrap();
        assert!(bold.style.add_modifier.contains(Modifier::BOLD));
        let code = lines[2].spans.iter().find(|s| s.content == "code").unwrap();
        assert_eq!(code.style.fg, Some(c.c(c.ansi[3])));
    }

    #[test]
    fn lists_hang_and_wrap() {
        let lines = lines("- first item that wraps around\n- second", 16, &colors());
        assert_eq!(plain(&lines), ["• first item", "  that wraps", "  around", "• second"]);
    }

    #[test]
    fn code_blocks_keep_their_lines() {
        let lines = lines("```rust\nfn main() {\n    let x = 1;\n}\n```", 40, &colors());
        assert_eq!(plain(&lines), ["  fn main() {", "      let x = 1;", "  }"]);
        // Coloured: `fn` is a keyword.
        assert!(lines[0].spans.iter().any(|s| s.content == "fn" && s.style.fg.is_some()));
    }

    #[test]
    fn tables_line_up() {
        let lines = lines("| a | bb |\n|---|---:|\n| ccc | d |", 40, &colors());
        assert_eq!(plain(&lines), ["a   │ bb", "────┼───", "ccc │  d"]);
    }
}
