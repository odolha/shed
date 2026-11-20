//! Paths connectors name, kept inside the folder they are relative to.

use std::path::{Component, Path, PathBuf};

use shed_api::{ApiError, ErrorCode};

fn invalid(msg: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Invalid, msg)
}

/// `rel` under `root`: relative, with no `..`, and (through links too)
/// still under `root` once resolved. What does not exist yet is checked by
/// the nearest folder above it that does. `""` is the root.
pub(crate) fn inside(root: &Path, rel: &str) -> Result<PathBuf, ApiError> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() || rel.starts_with('~') {
        return Err(invalid(format!("{rel:?} must be relative to the project")));
    }
    for c in rel_path.components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(invalid(format!("{rel:?} leaves the project"))),
        }
    }
    let real_root = std::fs::canonicalize(root).map_err(|e| ApiError::new(ErrorCode::NotFound, format!("{}: {e}", root.display())))?;
    let path = root.join(rel_path);
    // The nearest part of the path that exists, resolved.
    let mut existing = path.as_path();
    let resolved = loop {
        match std::fs::canonicalize(existing) {
            Ok(p) => break p,
            Err(_) => match existing.parent() {
                Some(parent) => existing = parent,
                None => return Err(invalid(format!("{rel:?} is not in the project"))),
            },
        }
    };
    if !resolved.starts_with(&real_root) {
        return Err(invalid(format!("{rel:?} leads out of the project")));
    }
    Ok(path)
}

/// A path relative to `root`, as connectors see it (`/` between parts).
pub(crate) fn relative(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    Some(rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"))
}

/// A path a Git call names, relative to the repository: no way out of it,
/// and nothing Git could read as an option.
pub(crate) fn git_path(rel: &str) -> Result<String, ApiError> {
    if rel.is_empty() || rel.starts_with('-') {
        return Err(invalid(format!("{rel:?} is not a path in the repository")));
    }
    let p = Path::new(rel);
    if p.is_absolute() || p.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) {
        return Err(invalid(format!("{rel:?} leaves the repository")));
    }
    Ok(rel.to_string())
}

/// Where a tab's project sits in its repository, to keep Git calls to it:
/// both roots resolved (links followed).
pub(crate) struct GitScope {
    repo: PathBuf,
    project: PathBuf,
}

impl GitScope {
    pub fn new(repo_root: &Path, project_root: &Path) -> Result<GitScope, ApiError> {
        let real = |p: &Path| std::fs::canonicalize(p).map_err(|e| ApiError::new(ErrorCode::NotFound, format!("{}: {e}", p.display())));
        Ok(GitScope { repo: real(repo_root)?, project: real(project_root)? })
    }

    /// Whether a path Git names (relative to the repository) is in the
    /// project, by its name alone: for lists.
    pub fn shows(&self, rel: &str) -> bool {
        self.repo.join(rel).starts_with(&self.project)
    }

    /// A path a Git call names, relative to the repository: as `git_path`,
    /// and it must lead into the tab's project (not just the repository,
    /// which may hold more), also once links are followed; never into Git's
    /// own folder.
    pub fn path(&self, rel: &str) -> Result<String, ApiError> {
        let rel = git_path(rel)?;
        let outside = || ApiError::new(ErrorCode::Forbidden, format!("{rel:?} is not in the tab's folder"));
        if Path::new(&rel).components().any(|c| matches!(c, Component::Normal(n) if n.eq_ignore_ascii_case(".git"))) {
            return Err(ApiError::new(ErrorCode::Forbidden, "Git's own folder is not open to connectors"));
        }
        let path = self.repo.join(&rel);
        if !path.starts_with(&self.project) {
            return Err(outside());
        }
        // The nearest part that exists, resolved: a link must not lead out.
        let mut existing = path.as_path();
        let resolved = loop {
            match std::fs::canonicalize(existing) {
                Ok(p) => break p,
                Err(_) => existing = existing.parent().ok_or_else(outside)?,
            }
        };
        if !resolved.starts_with(&self.project) {
            return Err(outside());
        }
        Ok(rel)
    }
}

/// A revision or branch name, which must not read as an option to Git.
pub(crate) fn git_name(name: &str) -> Result<&str, ApiError> {
    if name.trim().is_empty() || name.starts_with('-') || name.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(invalid(format!("{name:?} is not a branch or commit")));
    }
    Ok(name)
}

/// A file name with no folder in it (a picture's name).
pub(crate) fn plain_name(name: &str) -> Result<&str, ApiError> {
    let ok = !name.is_empty()
        && !name.starts_with('.')
        && Path::new(name).components().count() == 1
        && matches!(Path::new(name).components().next(), Some(Component::Normal(_)));
    if ok { Ok(name) } else { Err(invalid(format!("{name:?} is not a file name"))) }
}

/// `~/…` or `""` as the home folder, else the path as given; it must be absolute.
pub(crate) fn expand(path: &str) -> Result<PathBuf, ApiError> {
    let p = if path.trim().is_empty() { shed_save::expand_home("~") } else { shed_save::expand_home(path.trim()) };
    if p.is_absolute() { Ok(p) } else { Err(invalid(format!("{path:?} is not an absolute path"))) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_paths_stay_in_the_project() {
        // A repository whose root is above the project's folder.
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("mono");
        let project = repo.join("app");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join("secret.txt"), "x").unwrap();
        std::fs::write(project.join("src/a.rs"), "").unwrap();
        std::os::unix::fs::symlink(repo.join("secret.txt"), project.join("leak")).unwrap();
        std::os::unix::fs::symlink(project.join("src/a.rs"), project.join("fine")).unwrap();
        let scope = GitScope::new(&repo, &project).unwrap();
        assert_eq!(scope.path("app/src/a.rs").unwrap(), "app/src/a.rs");
        assert!(scope.path("app/src/gone.rs").is_ok(), "deleted files are named too");
        assert!(scope.path("app/fine").is_ok());
        for bad in ["secret.txt", "app/leak", ".git/config", "app/.git/config", "app/../secret.txt", "-p", "/etc/passwd"] {
            assert!(scope.path(bad).is_err(), "{bad}");
        }
        assert!(scope.shows("app/src/a.rs") && !scope.shows("secret.txt"));
        // The project at the repository's root.
        let whole = GitScope::new(&repo, &repo).unwrap();
        assert!(whole.path("secret.txt").is_ok() && whole.path(".git/HEAD").is_err());
    }

    #[test]
    fn paths_stay_in_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("p");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "").unwrap();
        std::fs::create_dir_all(dir.path().join("secret")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("secret"), root.join("out")).unwrap();
        assert_eq!(inside(&root, "src/a.rs").unwrap(), root.join("src/a.rs"));
        assert_eq!(inside(&root, "").unwrap(), root);
        assert!(inside(&root, "src/new/b.rs").is_ok(), "new files are fine");
        for bad in ["../etc/passwd", "/etc/passwd", "src/../../x", "out/x", "out", "~/x"] {
            assert_eq!(inside(&root, bad).unwrap_err().code, ErrorCode::Invalid, "{bad}");
        }
        assert_eq!(relative(&root, &root.join("src/a.rs")).unwrap(), "src/a.rs");
        assert!(git_path("-rf").is_err() && git_path("../x").is_err() && git_path("a/b").is_ok());
        assert!(git_name("--upload-pack=x").is_err() && git_name("main").is_ok());
        assert!(plain_name("a.png").is_ok() && plain_name("../a.png").is_err() && plain_name("a/b").is_err() && plain_name("..").is_err());
    }
}
