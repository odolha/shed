//! A repository as a project tab (or any other client) works with it: its
//! status and outgoing commits, the history, a push waiting to be
//! confirmed, and the operations on it. Each operation runs Git and blocks;
//! callers choose where (the app runs them on its background executor) and
//! when (debouncing, fetching every few minutes).

use std::collections::HashMap;
use std::path::Path;

use crate::{Branch, Commit, GitError, Hunk, PushTarget, Repo, Result, Status, diff_texts, hunk_patch};

/// Outgoing commits read with the status (more are counted, not listed).
pub const OUTGOING_LIMIT: usize = 200;
/// Commits read for the history.
pub const LOG_LIMIT: usize = 400;
/// What committing without a message says.
pub const NO_MESSAGE: &str = "Write a commit message first.";

/// Which two versions a diff compares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffSource {
    /// The index against the working folder.
    Unstaged,
    /// HEAD against the index.
    Staged,
    /// A commit against its first parent.
    Commit(String),
}

/// A diff as read: hunks of text, or a picture's two versions as bytes
/// (empty where there is none).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffContent {
    Hunks(Vec<Hunk>),
    Pictures { old: Vec<u8>, new: Vec<u8> },
}

/// What to do with one hunk of a diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HunkAction {
    Stage,
    Unstage,
    /// Thrown away from the working folder.
    Discard,
}

/// The status and the commits not pushed, as read together.
#[derive(Clone, Debug)]
pub struct Refreshed {
    pub status: Status,
    /// Newest first, at most `OUTGOING_LIMIT`.
    pub outgoing: Vec<Commit>,
}

/// What a push would send, and where, shown before it runs.
#[derive(Clone, Debug)]
pub struct PushReview {
    pub force: bool,
    pub branch: Branch,
    pub target: PushTarget,
    /// Newest first, at most `OUTGOING_LIMIT`.
    pub commits: Vec<Commit>,
}

impl PushReview {
    /// Reads where a push of the branch checked out goes and what it sends,
    /// with the status as it is now (a commit may just have been made).
    pub fn read(repo: &Repo, force: bool) -> Result<PushReview> {
        let branch = repo.status()?.branch;
        let commits = repo.outgoing(&branch, OUTGOING_LIMIT).unwrap_or_default();
        Ok(PushReview { force, target: repo.push_target(&branch), branch, commits })
    }

    /// Pushes, as reviewed.
    pub fn confirm(&self, repo: &Repo) -> Result<String> {
        repo.push(self.force, &self.branch)
    }

    /// What runs while it pushes: "Pushing…".
    pub fn busy_label(&self) -> &'static str {
        if self.force { "Force-pushing…" } else { "Pushing…" }
    }

    /// Its button: "Push 2 Commits", "Force-Push".
    pub fn action_label(&self) -> String {
        match (self.commits.len(), self.force) {
            (_, true) => "Force-Push".to_string(),
            (0, _) => "Push".to_string(),
            (1, _) => "Push 1 Commit".to_string(),
            (n, _) => format!("Push {n} Commits"),
        }
    }
}

/// An operation on the repository's branches, stash or remote, with the
/// words shown while it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BranchOp {
    Fetch,
    PullRebase,
    PullMerge,
    /// Switches to a local branch, or checks out a remote one.
    Switch(String),
    /// Checks out a remote branch (`origin/x`).
    CheckOut(String),
    /// A new branch from HEAD, switched to (spaces become dashes).
    Create(String),
    Delete { name: String, force: bool },
    /// Merges a branch into the one checked out.
    Merge(String),
    /// Rebases the branch checked out onto another.
    Rebase(String),
    Stash,
    StashPop,
}

impl BranchOp {
    pub fn label(&self) -> &'static str {
        match self {
            BranchOp::Fetch => "Fetching…",
            BranchOp::PullRebase | BranchOp::PullMerge => "Pulling…",
            BranchOp::Switch(_) => "Switching…",
            BranchOp::CheckOut(_) => "Checking out…",
            BranchOp::Create(_) => "Creating the branch…",
            BranchOp::Delete { .. } => "Deleting the branch…",
            BranchOp::Merge(_) => "Merging…",
            BranchOp::Rebase(_) => "Rebasing…",
            BranchOp::Stash => "Stashing…",
            BranchOp::StashPop => "Popping the stash…",
        }
    }

    pub fn run(&self, repo: &Repo) -> Result<String> {
        match self {
            BranchOp::Fetch => repo.fetch(),
            BranchOp::PullRebase => repo.pull_rebase(),
            BranchOp::PullMerge => repo.pull_merge(),
            BranchOp::Switch(b) | BranchOp::CheckOut(b) => repo.switch(b),
            BranchOp::Create(name) => repo.create_branch(&name.replace(' ', "-")),
            BranchOp::Delete { name, force } => repo.delete_branch(name, *force),
            BranchOp::Merge(b) => repo.merge(b),
            BranchOp::Rebase(b) => repo.rebase(b),
            BranchOp::Stash => repo.stash(),
            BranchOp::StashPop => repo.stash_pop(),
        }
    }
}

/// Local branches: the one checked out, and the others.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Branches {
    pub current: Option<String>,
    pub others: Vec<String>,
}

/// A repository and what is known of it: the data a Git panel shows.
#[derive(Clone, Debug)]
pub struct GitSession {
    pub repo: Repo,
    /// `None` until first read.
    pub status: Option<Status>,
    /// Commits the upstream does not have yet, newest first.
    pub outgoing: Vec<Commit>,
    /// The history, newest first (empty until read).
    pub log: Vec<Commit>,
    /// The file the history is for, if not the whole repository.
    pub log_for: Option<String>,
    /// Commits' files, as they were read.
    pub commit_files: HashMap<String, Vec<(char, String)>>,
    /// The next commit amends the last.
    pub amend: bool,
    /// A push waiting to be confirmed.
    pub push_review: Option<PushReview>,
}

impl GitSession {
    pub fn new(repo: Repo) -> GitSession {
        GitSession { repo, status: None, outgoing: Vec::new(), log: Vec::new(), log_for: None, commit_files: HashMap::new(), amend: false, push_review: None }
    }

    /// The repository `dir` is in, if any.
    pub fn open(dir: &Path) -> Option<GitSession> {
        Repo::find(dir).map(GitSession::new)
    }

    /// Keeps a refresh; whether HEAD moved (a new commit, a switch), which
    /// an open history follows.
    pub fn apply_refresh(&mut self, r: Refreshed) -> bool {
        let moved = self.status.as_ref().is_none_or(|old| old.branch.oid != r.status.branch.oid);
        self.status = Some(r.status);
        self.outgoing = r.outgoing;
        moved
    }

    /// Whether Git does not track this file yet.
    pub fn is_untracked(&self, path: &str) -> bool {
        self.status.as_ref().is_some_and(|s| s.entries.iter().any(|e| e.path == path && e.untracked))
    }

    /// (behind, ahead): ahead counts outgoing commits without an upstream too.
    pub fn behind_ahead(&self) -> (u32, u32) {
        self.status.as_ref().map_or((0, 0), |s| (s.branch.behind, s.branch.ahead.max(self.outgoing.len() as u32)))
    }

    /// The branch and how it stands, in a few words: `main ↑1 ↓2 · 3 changed`.
    pub fn branch_label(&self) -> Option<String> {
        let s = self.status.as_ref()?;
        let mut label = s.branch.head.clone().unwrap_or_else(|| s.branch.oid.clone());
        if s.branch.ahead > 0 {
            label.push_str(&format!(" ↑{}", s.branch.ahead));
        }
        if s.branch.behind > 0 {
            label.push_str(&format!(" ↓{}", s.branch.behind));
        }
        if !s.entries.is_empty() {
            label.push_str(&format!(" · {} changed", s.entries.len()));
        }
        Some(label)
    }

    /// Local branches, read now.
    pub fn branches(&self) -> Branches {
        let current = self.status.as_ref().and_then(|s| s.branch.head.clone());
        let others = self.repo.branches().unwrap_or_default().into_iter().filter(|b| Some(b) != current.as_ref()).collect();
        Branches { current, others }
    }

    /// A commit's files, read the first time they are asked for.
    pub fn commit_files(&mut self, hash: &str) -> &[(char, String)] {
        if !self.commit_files.contains_key(hash) {
            let files = self.repo.commit_files(hash).unwrap_or_default();
            self.commit_files.insert(hash.to_string(), files);
        }
        &self.commit_files[hash]
    }

    /// Starts a history (of a file, or all): what was read is let go.
    pub fn start_history(&mut self, path: Option<String>) {
        self.log_for = path;
        self.log.clear();
    }
}

/// Reads the status and the commits not pushed.
pub fn refresh(repo: &Repo) -> Result<Refreshed> {
    let status = repo.status()?;
    let outgoing = repo.outgoing(&status.branch, OUTGOING_LIMIT).unwrap_or_default();
    Ok(Refreshed { status, outgoing })
}

/// The history of a file, or of everything.
pub fn history(repo: &Repo, path: Option<&str>) -> Result<Vec<Commit>> {
    repo.log(path, LOG_LIMIT)
}

/// The two versions of a file's text that a diff compares (`untracked`: it
/// has no old version).
pub fn diff_texts_of(repo: &Repo, path: &str, source: &DiffSource, untracked: bool) -> (String, String) {
    match source {
        DiffSource::Unstaged => {
            let old = if untracked { String::new() } else { repo.show("", path) };
            (old, std::fs::read_to_string(repo.root.join(path)).unwrap_or_default())
        }
        DiffSource::Staged => (repo.show("HEAD", path), repo.show("", path)),
        DiffSource::Commit(h) => (repo.show(&format!("{h}^"), path), repo.show(h, path)),
    }
}

/// The two versions of a file's bytes that a diff compares.
pub fn diff_bytes_of(repo: &Repo, path: &str, source: &DiffSource, untracked: bool) -> (Vec<u8>, Vec<u8>) {
    match source {
        DiffSource::Unstaged => {
            let old = if untracked { Vec::new() } else { repo.show_bytes("", path) };
            (old, std::fs::read(repo.root.join(path)).unwrap_or_default())
        }
        DiffSource::Staged => (repo.show_bytes("HEAD", path), repo.show_bytes("", path)),
        DiffSource::Commit(h) => (repo.show_bytes(&format!("{h}^"), path), repo.show_bytes(h, path)),
    }
}

/// Reads a diff: a picture's two versions (`picture`: the caller knows
/// which files are pictures), else the texts' hunks with three lines around.
pub fn load_diff(repo: &Repo, path: &str, source: &DiffSource, untracked: bool, picture: bool) -> DiffContent {
    if picture {
        let (old, new) = diff_bytes_of(repo, path, source, untracked);
        return DiffContent::Pictures { old, new };
    }
    let (old, new) = diff_texts_of(repo, path, source, untracked);
    DiffContent::Hunks(diff_texts(&old, &new, 3))
}

/// Stages, unstages or throws away one hunk of `path`'s diff. A file Git
/// does not track yet is staged whole, whatever the action.
pub fn hunk_action(repo: &Repo, path: &str, hunk: &Hunk, action: HunkAction, untracked: bool) -> Result<()> {
    if untracked {
        return repo.stage(&[path.to_string()]);
    }
    let patch = hunk_patch(path, hunk);
    match action {
        HunkAction::Stage => repo.apply(&patch, true, false),
        HunkAction::Unstage => repo.apply(&patch, true, true),
        HunkAction::Discard => repo.apply(&patch, false, true),
    }
}

/// Commits what is staged (or amends the last commit) with `message`.
pub fn commit(repo: &Repo, message: &str, amend: bool) -> Result<String> {
    if message.trim().is_empty() {
        return Err(GitError(NO_MESSAGE.to_string()));
    }
    repo.commit(message, amend)
}

/// Conflict markers in a file's lines, as blocks: the lines (from 0) of
/// `<<<<<<<`, `=======` and `>>>>>>>`.
pub fn conflicts_in<S: AsRef<str>>(lines: impl IntoIterator<Item = S>) -> Vec<(usize, usize, usize)> {
    let mut blocks = Vec::new();
    let mut start = None;
    let mut sep = None;
    for (l, line) in lines.into_iter().enumerate() {
        let line = line.as_ref();
        if line.starts_with("<<<<<<<") {
            start = Some(l);
            sep = None;
        } else if line.starts_with("=======") && start.is_some() {
            sep = Some(l);
        } else if line.starts_with(">>>>>>>")
            && let (Some(s), Some(m)) = (start, sep)
        {
            blocks.push((s, m, l));
            start = None;
        }
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(dir: &Path) -> Repo {
        let r = Repo { root: dir.to_path_buf() };
        r.run(&["init", "-q", "-b", "main"]).unwrap();
        for (k, v) in [("user.email", "t@example.com"), ("user.name", "T"), ("commit.gpgsign", "false")] {
            r.run(&["config", k, v]).unwrap();
        }
        r
    }

    #[test]
    fn refresh_diff_and_a_hunk_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let r = repo(dir.path());
        std::fs::write(r.root.join("f.txt"), "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n").unwrap();
        r.stage(&["f.txt".into()]).unwrap();
        assert_eq!(commit(&r, "  \n", false), Err(GitError(NO_MESSAGE.into())));
        commit(&r, "first", false).unwrap();
        let mut git = GitSession::open(dir.path()).unwrap();
        assert!(git.apply_refresh(refresh(&git.repo).unwrap()));
        assert_eq!(git.branch_label().as_deref(), Some("main"));
        // Two changes far apart, and a new file.
        std::fs::write(r.root.join("f.txt"), "A\nb\nc\nd\ne\nf\ng\nh\ni\nJ\n").unwrap();
        std::fs::write(r.root.join("new.txt"), "x\n").unwrap();
        assert!(!git.apply_refresh(refresh(&git.repo).unwrap()));
        assert_eq!(git.branch_label().as_deref(), Some("main · 2 changed"));
        assert!(git.is_untracked("new.txt") && !git.is_untracked("f.txt"));
        let DiffContent::Hunks(hunks) = load_diff(&r, "f.txt", &DiffSource::Unstaged, false, false) else { panic!() };
        assert_eq!(hunks.len(), 2);
        // Stage the first hunk: it moves from not staged to staged.
        hunk_action(&r, "f.txt", &hunks[0], HunkAction::Stage, false).unwrap();
        let DiffContent::Hunks(staged) = load_diff(&r, "f.txt", &DiffSource::Staged, false, false) else { panic!() };
        assert_eq!(staged.len(), 1);
        let DiffContent::Hunks(left) = load_diff(&r, "f.txt", &DiffSource::Unstaged, false, false) else { panic!() };
        assert_eq!(left.len(), 1);
        // And back.
        hunk_action(&r, "f.txt", &staged[0], HunkAction::Unstage, false).unwrap();
        assert_eq!(load_diff(&r, "f.txt", &DiffSource::Staged, false, false), DiffContent::Hunks(Vec::new()));
        let DiffContent::Hunks(all) = load_diff(&r, "f.txt", &DiffSource::Unstaged, false, false) else { panic!() };
        assert_eq!(all, hunks);
        // A new file's hunk stages the file; discarding a hunk loses it.
        let DiffContent::Hunks(new) = load_diff(&r, "new.txt", &DiffSource::Unstaged, true, false) else { panic!() };
        hunk_action(&r, "new.txt", &new[0], HunkAction::Discard, true).unwrap();
        hunk_action(&r, "f.txt", &hunks[1], HunkAction::Discard, false).unwrap();
        assert!(std::fs::read_to_string(r.root.join("f.txt")).unwrap().ends_with("j\n"));
        git.apply_refresh(refresh(&r).unwrap());
        let s = git.status.as_ref().unwrap();
        assert_eq!(s.staged().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["new.txt"]);
        // Committed: HEAD moved; the history and the commit's files.
        commit(&r, "second", false).unwrap();
        assert!(git.apply_refresh(refresh(&r).unwrap()));
        git.log = history(&r, None).unwrap();
        assert_eq!(git.log.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["second", "first"]);
        let hash = git.log[0].hash.clone();
        assert_eq!(git.commit_files(&hash), [('A', "new.txt".to_string())]);
        let DiffContent::Hunks(in_commit) = load_diff(&r, "new.txt", &DiffSource::Commit(hash), false, false) else { panic!() };
        assert_eq!(in_commit.len(), 1);
        // A picture's two versions come as bytes.
        assert_eq!(load_diff(&r, "f.txt", &DiffSource::Unstaged, false, true), DiffContent::Pictures { old: r.show_bytes("", "f.txt"), new: std::fs::read(r.root.join("f.txt")).unwrap() });
        // Branches.
        BranchOp::Create("side work".into()).run(&r).unwrap();
        git.apply_refresh(refresh(&r).unwrap());
        assert_eq!(git.branches(), Branches { current: Some("side-work".into()), others: vec!["main".into()] });
        BranchOp::Switch("main".into()).run(&r).unwrap();
        BranchOp::Delete { name: "side-work".into(), force: false }.run(&r).unwrap();
        assert_eq!(r.branches().unwrap(), ["main"]);
    }

    #[test]
    fn a_push_is_reviewed_first() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let r = repo(&work);
        let remote = dir.path().join("remote.git");
        Repo { root: dir.path().to_path_buf() }.run(&["init", "-q", "--bare", "-b", "main", remote.to_str().unwrap()]).unwrap();
        r.run(&["remote", "add", "origin", remote.to_str().unwrap()]).unwrap();
        let add = |name: &str| {
            std::fs::write(r.root.join(name), name).unwrap();
            r.stage(&[name.into()]).unwrap();
            commit(&r, name, false).unwrap();
        };
        add("one");
        add("two");
        // No upstream yet: a new branch on origin, with both commits.
        let review = PushReview::read(&r, false).unwrap();
        assert_eq!((review.target.remote.as_str(), review.target.remote_branch.as_str(), review.target.new), ("origin", "main", true));
        assert_eq!(review.commits.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["two", "one"]);
        assert_eq!((review.action_label(), review.busy_label()), ("Push 2 Commits".to_string(), "Pushing…"));
        review.confirm(&r).unwrap();
        let mut git = GitSession::new(r.clone());
        git.apply_refresh(refresh(&r).unwrap());
        assert!(git.outgoing.is_empty());
        assert_eq!(git.behind_ahead(), (0, 0));
        add("three");
        git.apply_refresh(refresh(&r).unwrap());
        assert_eq!(git.behind_ahead(), (0, 1));
        assert_eq!(git.branch_label().as_deref(), Some("main ↑1"));
        let review = PushReview::read(&r, true).unwrap();
        assert!(!review.target.new);
        assert_eq!((review.action_label(), review.busy_label()), ("Force-Push".to_string(), "Force-pushing…"));
        assert_eq!(review.commits.len(), 1);
    }

    #[test]
    fn conflict_blocks() {
        let text = "a\n<<<<<<< ours\nb\n=======\nc\n>>>>>>> theirs\nd\n=======\n>>>>>>> stray\n";
        assert_eq!(conflicts_in(text.lines()), [(1, 3, 5)]);
        assert!(conflicts_in(["<<<<<<<", ">>>>>>>"]).is_empty());
    }
}
