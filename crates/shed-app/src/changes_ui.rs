//! What the Changes panel and the AI tab's review share, so they look and
//! behave alike: diffs as rows (unified, or side by side with removed lines
//! facing the added ones that replace them), the colour of Git's status
//! letters, and changed files by folder.

use gpui::{AnyElement, Div, IntoElement, ParentElement, Styled, div, prelude::*, px};
use shed_git::{Hunk, LineKind};
use shed_model::Rgb;

use crate::theme::{UiColors, hsla, hsla_alpha};

/// A diff row's height, at zoom 1.
pub const DIFF_ROW: f32 = 20.0;

/// A row of a diff as drawn.
pub enum DiffRow {
    /// A hunk's header (its index).
    Hunk(usize),
    /// A line (hunk, line).
    Line(usize, usize),
    /// Side by side: the old line and the new.
    Pair(Option<(usize, usize)>, Option<(usize, usize)>),
}

/// The rows of a diff: each hunk's header, then its lines, one a row or (side
/// by side) removed lines facing the added lines that replace them.
pub fn diff_layout(hunks: &[Hunk], side: bool) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    for (h, hunk) in hunks.iter().enumerate() {
        rows.push(DiffRow::Hunk(h));
        if !side {
            rows.extend((0..hunk.lines.len()).map(|l| DiffRow::Line(h, l)));
            continue;
        }
        let lines = &hunk.lines;
        let mut i = 0;
        while i < lines.len() {
            match lines[i].kind {
                LineKind::Context => {
                    rows.push(DiffRow::Pair(Some((h, i)), Some((h, i))));
                    i += 1;
                }
                _ => {
                    let removed: Vec<usize> = (i..lines.len()).take_while(|j| lines[*j].kind == LineKind::Removed).collect();
                    let after = i + removed.len();
                    let added: Vec<usize> = (after..lines.len()).take_while(|j| lines[*j].kind == LineKind::Added).collect();
                    for k in 0..removed.len().max(added.len()) {
                        rows.push(DiffRow::Pair(removed.get(k).map(|l| (h, *l)), added.get(k).map(|l| (h, *l))));
                    }
                    i = after + added.len();
                }
            }
        }
    }
    rows
}

/// A diff row drawn: `base` is the row's element (its id and height set by
/// the caller), `actions` what goes at the right of a hunk's header.
pub fn render_diff_row(base: gpui::Stateful<Div>, row: &DiffRow, hunks: &[Hunk], ui: &UiColors, accent: Rgb, actions: Option<AnyElement>) -> AnyElement {
    let (red, green) = (Rgb::hex(0xE5534B), Rgb::hex(0x57AB5A));
    let num = |n: Option<usize>| div().flex_none().w(px(40.0)).text_color(hsla(ui.text_dim)).child(n.map(|n| n.to_string()).unwrap_or_default());
    let cell = |line: Option<&shed_git::Line>| {
        let (bg, sign) = match line.map(|l| l.kind) {
            Some(LineKind::Added) => (Some(green), "+"),
            Some(LineKind::Removed) => (Some(red), "-"),
            Some(LineKind::Context) => (None, " "),
            None => (Some(ui.divider), " "),
        };
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .items_center()
            .overflow_hidden()
            .when_some(bg, |d, c| d.bg(hsla_alpha(c, 0.15)))
            .child(num(line.and_then(|l| l.old.or(l.new))))
            .child(div().flex_none().w(px(12.0)).text_color(hsla(ui.text_dim)).child(sign))
            .child(div().whitespace_nowrap().child(line.map(|l| l.text.clone()).unwrap_or_default()))
    };
    match row {
        DiffRow::Hunk(h) => base
            .gap_2()
            .px_2()
            .bg(hsla_alpha(accent, 0.08))
            .child(div().flex_1().text_color(hsla(ui.text_dim)).child(hunks[*h].header()))
            .children(actions)
            .into_any_element(),
        DiffRow::Line(h, l) => {
            let line = &hunks[*h].lines[*l];
            let (bg, sign) = match line.kind {
                LineKind::Added => (Some(green), "+"),
                LineKind::Removed => (Some(red), "-"),
                LineKind::Context => (None, " "),
            };
            base.when_some(bg, |d, c| d.bg(hsla_alpha(c, 0.15)))
                .child(num(line.old))
                .child(num(line.new))
                .child(div().flex_none().w(px(12.0)).text_color(hsla(ui.text_dim)).child(sign))
                .child(div().whitespace_nowrap().child(line.text.clone()))
                .into_any_element()
        }
        DiffRow::Pair(a, b) => {
            let left = a.map(|(h, l)| &hunks[h].lines[l]);
            let right = b.map(|(h, l)| &hunks[h].lines[l]);
            base.child(cell(left)).child(div().flex_none().w(px(1.0)).h_full().bg(hsla(ui.divider))).child(cell(right)).into_any_element()
        }
    }
}

/// The colour of Git's letter for a change: added green, deleted or in
/// conflict red, renamed or copied blue, else (changed) amber.
pub fn status_color(code: char, ui: &UiColors) -> Rgb {
    match code {
        'A' | '?' => Rgb::hex(0x57AB5A),
        'D' | 'U' => ui.danger,
        'R' | 'C' => Rgb::hex(0x539BF5),
        _ => Rgb::hex(0xC69026),
    }
}

/// A row of the tree of changed files.
#[derive(Debug, PartialEq)]
pub enum FolderRow {
    /// `path` from the root; `name` may join folders that hold only one
    /// folder (`src/app`); `files` are every file under it.
    Folder { path: String, name: String, depth: usize, files: Vec<usize>, open: bool },
    /// The file at `index` in the list given.
    File { index: usize, depth: usize },
}

/// Paths as a tree: folders first (by name), then files; a closed folder's
/// contents left out.
pub fn folder_rows(paths: &[&str], closed: impl Fn(&str) -> bool) -> Vec<FolderRow> {
    #[derive(Default)]
    struct Node {
        dirs: std::collections::BTreeMap<String, Node>,
        files: Vec<usize>,
    }
    impl Node {
        fn all(&self, out: &mut Vec<usize>) {
            for d in self.dirs.values() {
                d.all(out);
            }
            out.extend(&self.files);
        }
    }
    let mut root = Node::default();
    for (i, p) in paths.iter().enumerate() {
        let mut node = &mut root;
        let mut parts: Vec<&str> = p.split('/').collect();
        parts.pop();
        for part in parts {
            node = node.dirs.entry(part.to_string()).or_default();
        }
        node.files.push(i);
    }
    fn walk(node: &Node, prefix: &str, depth: usize, paths: &[&str], closed: &dyn Fn(&str) -> bool, out: &mut Vec<FolderRow>) {
        for (name, child) in &node.dirs {
            let (mut name, mut child) = (name.clone(), child);
            while child.files.is_empty() && child.dirs.len() == 1 {
                let (sub, next) = child.dirs.iter().next().unwrap();
                name = format!("{name}/{sub}");
                child = next;
            }
            let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            let mut files = Vec::new();
            child.all(&mut files);
            let open = !closed(&path);
            out.push(FolderRow::Folder { path: path.clone(), name, depth, files, open });
            if open {
                walk(child, &path, depth + 1, paths, closed, out);
            }
        }
        let mut files = node.files.clone();
        files.sort_by_key(|&i| paths[i].rsplit('/').next().unwrap_or("").to_lowercase());
        out.extend(files.into_iter().map(|index| FolderRow::File { index, depth }));
    }
    let mut out = Vec::new();
    walk(&root, "", 0, paths, &closed, &mut out);
    out
}

#[cfg(test)]
mod folder_tests {
    use super::*;

    #[test]
    fn changed_files_by_folder() {
        let paths = ["src/app/a.rs", "src/app/b.rs", "README.md", "src/lib.rs", "docs/x/y/z.md"];
        let rows = folder_rows(&paths, |_| false);
        let shown: Vec<String> = rows
            .iter()
            .map(|r| match r {
                FolderRow::Folder { name, depth, files, .. } => format!("{}{name}/ ({})", "  ".repeat(*depth), files.len()),
                FolderRow::File { index, depth } => format!("{}{}", "  ".repeat(*depth), paths[*index].rsplit('/').next().unwrap()),
            })
            .collect();
        assert_eq!(shown, ["docs/x/y/ (1)", "  z.md", "src/ (3)", "  app/ (2)", "    a.rs", "    b.rs", "  lib.rs", "README.md"]);
        // A closed folder hides what is in it.
        let rows = folder_rows(&paths, |p| p == "src");
        assert_eq!(rows.len(), 4);
        assert!(matches!(&rows[2], FolderRow::Folder { path, open: false, .. } if path == "src"));
    }
}
