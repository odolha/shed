//! Files ranked by a typed name, as in "open by name".

use std::path::PathBuf;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// The best `limit` files for `query`, best first: each file's index and
/// the char positions of its path that matched.
pub fn rank(query: &str, files: &[PathBuf], limit: usize) -> Vec<(usize, Vec<u32>)> {
    if query.trim().is_empty() {
        return Vec::new();
    }
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, usize)> = files
        .iter()
        .enumerate()
        .filter_map(|(i, f)| {
            let path = f.to_string_lossy();
            pattern.score(Utf32Str::new(&path, &mut buf), &mut matcher).map(|s| (s, i))
        })
        .collect();
    // Best score first; on a tie, the shorter path.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| files[a.1].as_os_str().len().cmp(&files[b.1].as_os_str().len())));
    scored
        .into_iter()
        .take(limit)
        .map(|(_, i)| {
            let path = files[i].to_string_lossy();
            let mut indices = Vec::new();
            pattern.indices(Utf32Str::new(&path, &mut buf), &mut matcher, &mut indices);
            indices.sort_unstable();
            indices.dedup();
            (i, indices)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn best_matches_first() {
        let files: Vec<PathBuf> = ["src/main.rs", "src/project_view.rs", "docs/plan.md", "crates/pv/lib.rs"].iter().map(PathBuf::from).collect();
        let ranked = rank("projview", &files, 5);
        assert_eq!(ranked[0].0, 1);
        assert!(!ranked[0].1.is_empty());
        assert!(rank("", &files, 5).is_empty());
        assert!(rank("zzzz", &files, 5).is_empty());
    }
}
