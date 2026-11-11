//! Two versions of a file as hunks of changed lines, and a hunk as a patch
//! for `git apply`, to stage, unstage or throw away just that change.

use similar::{ChangeTag, TextDiff};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub kind: LineKind,
    /// From 1, in the old version (not for added lines).
    pub old: Option<usize>,
    /// From 1, in the new version (not for removed lines).
    pub new: Option<usize>,
    /// Without its line break.
    pub text: String,
    /// The last line of its version, with no line break after it.
    pub no_newline: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// From 1; the line before the hunk when it has no old lines.
    pub old_start: usize,
    pub old_len: usize,
    pub new_start: usize,
    pub new_len: usize,
    pub lines: Vec<Line>,
}

impl Hunk {
    pub fn header(&self) -> String {
        format!("@@ -{},{} +{},{} @@", self.old_start, self.old_len, self.new_start, self.new_len)
    }
}

/// The changes from `old` to `new`, with `context` unchanged lines around each.
pub fn diff_texts(old: &str, new: &str, context: usize) -> Vec<Hunk> {
    let diff = TextDiff::from_lines(old, new);
    let mut hunks = Vec::new();
    for group in diff.grouped_ops(context) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else { continue };
        let old_range = first.old_range().start..last.old_range().end;
        let new_range = first.new_range().start..last.new_range().end;
        let mut lines = Vec::new();
        for op in &group {
            for change in diff.iter_changes(op) {
                let raw = change.value();
                let text = raw.strip_suffix('\n').unwrap_or(raw);
                let text = text.strip_suffix('\r').unwrap_or(text).to_string();
                let kind = match change.tag() {
                    ChangeTag::Equal => LineKind::Context,
                    ChangeTag::Insert => LineKind::Added,
                    ChangeTag::Delete => LineKind::Removed,
                };
                lines.push(Line {
                    kind,
                    old: change.old_index().map(|i| i + 1),
                    new: change.new_index().map(|i| i + 1),
                    text,
                    no_newline: !raw.ends_with('\n'),
                });
            }
        }
        let start = |r: &std::ops::Range<usize>| if r.is_empty() { r.start } else { r.start + 1 };
        hunks.push(Hunk { old_start: start(&old_range), old_len: old_range.len(), new_start: start(&new_range), new_len: new_range.len(), lines });
    }
    hunks
}

/// A patch of one hunk of `path`, for `git apply`.
pub fn hunk_patch(path: &str, hunk: &Hunk) -> String {
    let mut out = format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n{}\n", hunk.header());
    for line in &hunk.lines {
        let sign = match line.kind {
            LineKind::Context => ' ',
            LineKind::Added => '+',
            LineKind::Removed => '-',
        };
        out.push(sign);
        out.push_str(&line.text);
        out.push('\n');
        if line.no_newline {
            out.push_str("\\ No newline at end of file\n");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hunks_and_their_numbers() {
        let hunks = diff_texts("a\nb\nc\n", "a\nB\nc\nd", 1);
        assert_eq!(hunks.len(), 1);
        let h = &hunks[0];
        assert_eq!(h.header(), "@@ -1,3 +1,4 @@");
        let kinds: Vec<LineKind> = h.lines.iter().map(|l| l.kind).collect();
        assert_eq!(kinds, [LineKind::Context, LineKind::Removed, LineKind::Added, LineKind::Context, LineKind::Added]);
        assert!(h.lines.last().unwrap().no_newline);
        assert!(hunk_patch("x", h).ends_with("+d\n\\ No newline at end of file\n"));
        // A new file: no old lines.
        let new = diff_texts("", "x\n", 3);
        assert_eq!(new[0].header(), "@@ -0,0 +1,1 @@");
    }
}
