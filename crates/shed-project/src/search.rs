//! Searching a project's files for text, with ripgrep's own engine, and
//! replacing what was found.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::sinks::Lossy;
use grep_searcher::{BinaryDetection, SearcherBuilder};

/// Lines longer than this are cut, around the first match.
const MAX_LINE: usize = 400;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    /// `text` is a regular expression; otherwise it is matched as written.
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
}

/// A line that matched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// From 0.
    pub line: usize,
    /// The line (cut if long), without its line break.
    pub text: String,
    /// Where in `text` it matched, by byte.
    pub ranges: Vec<Range<usize>>,
    /// Bytes cut off the front of the line, to place the ranges in it.
    pub offset: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileHits {
    /// Relative to the project's root.
    pub path: PathBuf,
    pub hits: Vec<Hit>,
}

/// The query as a pattern for the `regex` crate.
fn pattern(q: &Query) -> String {
    let core = if q.regex { q.text.clone() } else { regex::escape(&q.text) };
    let core = if q.whole_word { format!(r"\b(?:{core})\b") } else { core };
    if q.case_sensitive { core } else { format!("(?i){core}") }
}

/// Whether the query can be searched for: why not, if it cannot.
pub fn check(q: &Query) -> Result<(), String> {
    matcher(q).map(drop)
}

fn matcher(q: &Query) -> Result<RegexMatcher, String> {
    RegexMatcherBuilder::new().build(&pattern(q)).map_err(|e| e.to_string())
}

/// Searches `files` (relative to `root`) on several threads, handing each
/// file with matches to `found` as it is done. Stops early when `cancel` is
/// set or after `limit` matching lines.
pub fn search(root: &Path, files: &[PathBuf], q: &Query, cancel: &AtomicBool, limit: usize, found: impl Fn(FileHits) + Sync) -> Result<(), String> {
    if q.text.is_empty() {
        return Ok(());
    }
    let matcher = matcher(q)?;
    let next = AtomicUsize::new(0);
    let total = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(16);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                let mut searcher = SearcherBuilder::new().line_number(true).binary_detection(BinaryDetection::quit(0)).build();
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= files.len() || cancel.load(Ordering::Relaxed) || total.load(Ordering::Relaxed) >= limit {
                        break;
                    }
                    let mut hits = Vec::new();
                    let sink = Lossy(|line: u64, text: &str| {
                        let text = text.trim_end_matches(['\n', '\r']);
                        let mut ranges = Vec::new();
                        let _ = matcher.find_iter(text.as_bytes(), |m| {
                            ranges.push(m.start()..m.end());
                            true
                        });
                        if !ranges.is_empty() {
                            hits.push(cut(line as usize - 1, text, ranges));
                        }
                        Ok(hits.len() < 1000)
                    });
                    let _ = searcher.search_path(&matcher, root.join(&files[i]), sink);
                    if !hits.is_empty() {
                        total.fetch_add(hits.len(), Ordering::Relaxed);
                        found(FileHits { path: files[i].clone(), hits });
                    }
                }
            });
        }
    });
    Ok(())
}

/// A hit, its line cut to a readable length around the first match.
fn cut(line: usize, text: &str, ranges: Vec<Range<usize>>) -> Hit {
    if text.len() <= MAX_LINE {
        return Hit { line, text: text.to_string(), ranges, offset: 0 };
    }
    let mut start = ranges[0].start.saturating_sub(MAX_LINE / 4);
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (start + MAX_LINE).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let ranges = ranges.into_iter().filter(|r| r.start >= start && r.end <= end).map(|r| r.start - start..r.end - start).collect();
    Hit { line, text: text[start..end].to_string(), ranges, offset: start }
}

/// `text` with every match of `q` replaced by `with` (`$1` and the like
/// refer to groups, for a regular expression), and how many were replaced.
pub fn replace(text: &str, q: &Query, with: &str) -> Result<(String, usize), String> {
    let re = regex::Regex::new(&pattern(q)).map_err(|e| e.to_string())?;
    let count = re.find_iter(text).count();
    let replaced = if q.regex { re.replace_all(text, with) } else { re.replace_all(text, regex::NoExpand(with)) };
    Ok((replaced.into_owned(), count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn finds_in_files_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("a.rs"), "let foo = 1;\nfoobar();\nFOO\n").unwrap();
        std::fs::write(root.join("b.txt"), "nothing here\n").unwrap();
        std::fs::write(root.join("bin"), b"foo\0\x01").unwrap();
        let files: Vec<PathBuf> = ["a.rs", "b.txt", "bin"].iter().map(PathBuf::from).collect();
        let found = Mutex::new(Vec::new());
        let q = Query { text: "foo".into(), ..Default::default() };
        search(root, &files, &q, &AtomicBool::new(false), 100, |f| found.lock().unwrap().push(f)).unwrap();
        let found = found.into_inner().unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].hits.iter().map(|h| h.line).collect::<Vec<_>>(), [0, 1, 2]);
        assert_eq!(found[0].hits[0].ranges, vec![4..7]);

        let words = Query { text: "foo".into(), whole_word: true, case_sensitive: true, ..Default::default() };
        let (text, n) = replace("let foo = 1;\nfoobar();\nFOO\n", &words, "bar").unwrap();
        assert_eq!((text.as_str(), n), ("let bar = 1;\nfoobar();\nFOO\n", 1));
        let re = Query { text: r"(\w+)\(\)".into(), regex: true, ..Default::default() };
        assert_eq!(replace("go();", &re, "$1(x)").unwrap().0, "go(x);");
        let plain = Query { text: "a.b".into(), ..Default::default() };
        assert_eq!(replace("a.b axb", &plain, "$0").unwrap().0, "$0 axb");
    }

    #[test]
    fn long_lines_are_cut_around_the_match() {
        let line = format!("{}needle{}", "x".repeat(1000), "y".repeat(1000));
        let hit = cut(0, &line, std::iter::once(1000..1006).collect());
        assert!(hit.text.len() <= MAX_LINE);
        assert_eq!(&hit.text[hit.ranges[0].clone()], "needle");
    }
}
