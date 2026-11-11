//! The files of a project, as Git would see them plus what the tab excludes.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ignore::WalkState;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::overrides::OverrideBuilder;

/// What a walk found.
#[derive(Debug, Default)]
pub struct Walked {
    /// Files, relative to the root, sorted.
    pub files: Vec<PathBuf>,
    /// Folders, absolute, the root first: what to watch.
    pub dirs: Vec<PathBuf>,
}

/// Every file under `root` that is not ignored by Git's rules (`.gitignore`,
/// `.git/info/exclude`, the global ignore) nor matches an `exclude` glob.
/// Hidden files are listed; `.git` is not.
pub fn walk(root: &Path, exclude: &[String]) -> Walked {
    let mut overrides = OverrideBuilder::new(root);
    for pattern in exclude {
        let _ = overrides.add(&format!("!{pattern}"));
    }
    let overrides = overrides.build().unwrap_or_else(|_| ignore::overrides::Override::empty());
    let files = Mutex::new(Vec::new());
    let dirs = Mutex::new(vec![root.to_path_buf()]);
    ignore::WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .overrides(overrides)
        .filter_entry(|e| e.file_name() != ".git")
        .build_parallel()
        .run(|| {
            Box::new(|entry| {
                let Ok(entry) = entry else { return WalkState::Continue };
                if entry.depth() == 0 {
                    return WalkState::Continue;
                }
                match entry.file_type() {
                    Some(t) if t.is_dir() => dirs.lock().unwrap().push(entry.into_path()),
                    Some(_) => {
                        if let Ok(rel) = entry.path().strip_prefix(root) {
                            files.lock().unwrap().push(rel.to_path_buf());
                        }
                    }
                    None => {}
                }
                WalkState::Continue
            })
        });
    let mut files = files.into_inner().unwrap();
    files.sort();
    let mut dirs = dirs.into_inner().unwrap();
    dirs[1..].sort();
    Walked { files, dirs }
}

/// Tells whether a path that changed is one the project leaves out: in
/// `.git`, ignored by the root `.gitignore`, or excluded by the tab.
pub struct Excluder {
    root: PathBuf,
    rules: Gitignore,
}

impl Excluder {
    pub fn new(root: &Path, exclude: &[String]) -> Self {
        let mut b = GitignoreBuilder::new(root);
        b.add(root.join(".gitignore"));
        b.add(root.join(".git/info/exclude"));
        for pattern in exclude {
            let _ = b.add_line(None, pattern);
        }
        Self { root: root.to_path_buf(), rules: b.build().unwrap_or_else(|_| Gitignore::empty()) }
    }

    pub fn excludes(&self, path: &Path, is_dir: bool) -> bool {
        let Ok(rel) = path.strip_prefix(&self.root) else { return true };
        if rel.components().next().is_some_and(|c| c.as_os_str() == ".git") {
            return true;
        }
        self.rules.matched_path_or_any_parents(path, is_dir).is_ignore()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_what_git_would_see() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for d in ["src/deep", "target/debug", ".git", "docs"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        for f in ["src/main.rs", "src/deep/x.rs", "target/debug/out", ".git/HEAD", ".env", "docs/a.md", "notes.tmp"] {
            std::fs::write(root.join(f), "").unwrap();
        }
        std::fs::write(root.join(".gitignore"), "target/\n*.tmp\n").unwrap();
        let w = walk(root, &["docs".into()]);
        let files: Vec<String> = w.files.iter().map(|p| p.display().to_string()).collect();
        assert_eq!(files, [".env", ".gitignore", "src/deep/x.rs", "src/main.rs"]);
        assert_eq!(w.dirs[0], root);
        assert!(w.dirs.contains(&root.join("src/deep")) && !w.dirs.contains(&root.join("target")));

        let ex = Excluder::new(root, &["docs".into()]);
        assert!(ex.excludes(&root.join("target/debug/new"), false));
        assert!(ex.excludes(&root.join("a.tmp"), false));
        assert!(ex.excludes(&root.join(".git/index"), false));
        assert!(ex.excludes(&root.join("docs/b.md"), false));
        assert!(!ex.excludes(&root.join("src/new.rs"), false));
    }
}
