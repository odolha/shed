//! Diffs and changed files laid out as rows, for whatever draws them: diff
//! rows unified or side by side (removed lines facing the added ones that
//! replace them), and changed files by folder.

use crate::{Hunk, LineKind};

/// A row of a diff as drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
mod tests {
    use super::*;
    use crate::diff_texts;

    #[test]
    fn removed_lines_face_their_replacements() {
        let hunks = diff_texts("a\nb\nc\nd\n", "a\nB\nC2\nC3\nd\n", 1);
        assert_eq!(hunks.len(), 1);
        let unified = diff_layout(&hunks, false);
        assert_eq!(unified.len(), 1 + hunks[0].lines.len());
        assert_eq!(unified[0], DiffRow::Hunk(0));
        assert_eq!(unified[1], DiffRow::Line(0, 0));
        // Side by side: context, then b and c facing B, C2 and C3, then context.
        let kinds: Vec<LineKind> = hunks[0].lines.iter().map(|l| l.kind).collect();
        assert_eq!(kinds.iter().filter(|k| **k == LineKind::Removed).count(), 2);
        let side = diff_layout(&hunks, true);
        let first_removed = kinds.iter().position(|k| *k == LineKind::Removed).unwrap();
        let first_added = kinds.iter().position(|k| *k == LineKind::Added).unwrap();
        assert_eq!(
            side,
            [
                DiffRow::Hunk(0),
                DiffRow::Pair(Some((0, 0)), Some((0, 0))),
                DiffRow::Pair(Some((0, first_removed)), Some((0, first_added))),
                DiffRow::Pair(Some((0, first_removed + 1)), Some((0, first_added + 1))),
                DiffRow::Pair(None, Some((0, first_added + 2))),
                DiffRow::Pair(Some((0, kinds.len() - 1)), Some((0, kinds.len() - 1))),
            ]
        );
    }

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
