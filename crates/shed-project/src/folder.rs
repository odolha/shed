//! A project's folders as the file tree shows them, and which project a
//! path belongs to.

use std::path::{Path, PathBuf};

/// An entry of a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderEntry {
    pub path: PathBuf,
    pub name: String,
    /// A folder, or a link to one.
    pub dir: bool,
    /// Its name starts with a dot.
    pub hidden: bool,
}

/// A folder's entries, folders first, each group by name; `.git` is left out.
pub fn read_folder(dir: &Path) -> Vec<FolderEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut rows: Vec<FolderEntry> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_name() != ".git")
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            // Through symlinks, so a linked folder opens like a folder.
            let dir = e.path().is_dir();
            FolderEntry { hidden: name.starts_with('.'), name, path: e.path(), dir }
        })
        .collect();
    rows.sort_by_cached_key(|r| (!r.dir, r.name.to_lowercase()));
    rows
}

/// The project a folder belongs to: the nearest folder above it (or itself)
/// with a `.git`, else the folder itself.
pub fn project_root(dir: &Path) -> PathBuf {
    dir.ancestors().find(|d| d.join(".git").exists()).unwrap_or(dir).to_path_buf()
}

/// The project (of `roots`, each with its id) that holds `path`, the
/// innermost, and the path as that project knows it. Compared with links
/// resolved: a project open as ~/Workspace/x holds ~/Work/x/f when ~/Work
/// links to ~/Workspace.
pub fn project_holding<T: Copy>(path: &Path, roots: &[(T, PathBuf)]) -> Option<(T, PathBuf)> {
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let real_path = real(path);
    roots
        .iter()
        .filter_map(|(id, root)| {
            let real_root = real(root);
            let rel = real_path.strip_prefix(&real_root).ok()?.to_path_buf();
            Some((*id, real_root.as_os_str().len(), root.join(rel)))
        })
        .max_by_key(|(_, len, _)| *len)
        .map(|(id, _, p)| (id, p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_come_first_and_git_is_left_out() {
        let dir = tempfile::tempdir().unwrap();
        for d in ["src", ".git", "Docs"] {
            std::fs::create_dir(dir.path().join(d)).unwrap();
        }
        for f in ["b.rs", "A.md", ".env"] {
            std::fs::write(dir.path().join(f), "").unwrap();
        }
        let names: Vec<String> = read_folder(dir.path()).into_iter().map(|r| r.name).collect();
        assert_eq!(names, ["Docs", "src", ".env", "A.md", "b.rs"]);
        std::fs::create_dir_all(dir.path().join("src/deep")).unwrap();
        assert_eq!(project_root(&dir.path().join("src/deep")), dir.path());
    }

    #[test]
    fn a_project_holds_its_files_through_links() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("Workspace/app");
        std::fs::create_dir_all(real.join("docs")).unwrap();
        std::fs::write(real.join("docs/guide.md"), "").unwrap();
        std::os::unix::fs::symlink(dir.path().join("Workspace"), dir.path().join("Work")).unwrap();
        let (outer, inner) = (1, 2);
        let roots = vec![(outer, dir.path().join("Workspace")), (inner, real.clone())];
        // Named through the link, held by the innermost project.
        let (tab, path) = project_holding(&dir.path().join("Work/app/docs/guide.md"), &roots).unwrap();
        assert_eq!((tab, path), (inner, real.join("docs/guide.md")));
        assert!(project_holding(&dir.path().join("elsewhere.md"), &roots).is_none());
    }
}
