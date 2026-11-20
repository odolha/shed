//! Reviewing what Claude did in an AI tab, as data: every change in the
//! repositories it works in, sorted by how far it went (not staged, staged,
//! committed, pushed), each with its diff, a key and a fingerprint for its
//! Viewed mark, and explanations asked of a copy of the session. For this
//! turn, or the whole session: where the branch was then comes from Git's
//! reflog. No UI and no threads: callers run `read` and `ExplainRequest::ask`
//! where blocking is fine, and draw or send what they return.

use std::path::{Path, PathBuf};

use shed_ai::Entry;
use shed_ai::aside::ToExplain;
use shed_git::{Hunk, LineKind, Repo};
use shed_model::AiTab;
use shed_project::picture::{PictureData, is_picture};

/// More than this many changed files are not all read.
pub const MOST: usize = 300;

/// At most this many diff lines go into one request to explain…
pub const EXPLAIN_LINES: usize = 4000;
/// …and at most this many of one file's.
pub const EXPLAIN_FILE_LINES: usize = 150;

/// What the review covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Since the last prompt was sent.
    Turn,
    /// Since the session's first prompt.
    Session,
}

/// How far a change went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Layer {
    Unstaged { untracked: bool },
    Staged,
    Committed { hash: String, pushed: bool },
}

/// One file's change, in one layer.
#[derive(Clone, Debug)]
pub struct Change {
    /// Its repository, by its place in `Found::repos`.
    pub repo: usize,
    pub layer: Layer,
    /// Relative to the repository's root.
    pub path: String,
    /// Git's letter for it: `M`, `A`, `D`, `R`.
    pub code: char,
    pub hunks: Vec<Hunk>,
    /// Of what it is now, to tell whether what was viewed is still it.
    pub hash: String,
    /// For a picture: its two versions, shown side by side in place of hunks.
    pub pictures: Option<(Option<PictureData>, Option<PictureData>)>,
}

impl Change {
    /// What its Viewed mark and explanation are kept under: the repository,
    /// the layer and the path.
    pub fn key(&self, root: &Path) -> String {
        let layer = match &self.layer {
            Layer::Unstaged { .. } => "worktree".to_string(),
            Layer::Staged => "index".to_string(),
            Layer::Committed { hash, .. } => format!("commit {hash}"),
        };
        format!("{}|{layer}|{}", root.display(), self.path)
    }
}

/// A commit made in the scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitInfo {
    pub hash: String,
    pub short: String,
    pub subject: String,
    pub pushed: bool,
}

/// A repository in the review.
#[derive(Clone, Debug)]
pub struct RepoReview {
    pub repo: Repo,
    /// Its folder's name.
    pub name: String,
    /// Where the branch was when the scope began (`None`: the reflog does not
    /// go back that far; only what is not committed is shown).
    pub base: Option<String>,
    /// Newest first.
    pub commits: Vec<CommitInfo>,
}

/// What reading the repositories found.
#[derive(Clone, Debug, Default)]
pub struct Found {
    pub repos: Vec<RepoReview>,
    pub changes: Vec<Change>,
    /// More changes than were read (`MOST`).
    pub cut: bool,
}

impl Found {
    /// A change's key (`Change::key` under its repository's root).
    pub fn key(&self, c: &Change) -> String {
        c.key(&self.repos[c.repo].repo.root)
    }

    /// Whether a change is marked viewed as it is now, by an AI tab's marks.
    pub fn is_viewed(&self, c: &Change, viewed: &[(String, String)]) -> bool {
        is_viewed(viewed, &self.key(c), &c.hash)
    }

    /// The explanation kept for a change: its text, and whether it is of the
    /// change as it is now (else it was of an earlier version).
    pub fn explanation(&self, c: &Change, explained: &[(String, String, String)]) -> Option<(String, bool)> {
        let key = self.key(c);
        explained.iter().rev().find(|(k, _, _)| *k == key).map(|(_, h, t)| (t.clone(), *h == c.hash))
    }

    /// The repository to commit what is staged in: the first with something staged.
    pub fn staged_repo(&self) -> Option<&Repo> {
        self.changes.iter().find(|c| c.layer == Layer::Staged).map(|c| &self.repos[c.repo].repo)
    }

    /// A request to explain these changes (by their place in `changes`),
    /// each with its diff, cut to fit the budget (`EXPLAIN_LINES`, at most
    /// `EXPLAIN_FILE_LINES` a file).
    pub fn explain_request(&self, which: &[usize]) -> ExplainRequest {
        let several = self.repos.len() > 1;
        let mut request = ExplainRequest::default();
        let mut budget = EXPLAIN_LINES;
        for &i in which {
            let Some(c) = self.changes.get(i) else { continue };
            let state = match &c.layer {
                Layer::Unstaged { .. } => "not staged".to_string(),
                Layer::Staged => "staged".to_string(),
                Layer::Committed { hash, pushed } => {
                    let subject = self.repos[c.repo].commits.iter().find(|x| x.hash == *hash).map(|x| x.subject.clone()).unwrap_or_default();
                    format!("{} in commit {} \u{201c}{subject}\u{201d}", if *pushed { "pushed" } else { "committed" }, &hash[..hash.len().min(8)])
                }
            };
            // Its diff, cut short: the copy knows the conversation, and can read files.
            let mut diff = String::new();
            for line in c.hunks.iter().flat_map(|h| &h.lines).take(EXPLAIN_FILE_LINES.min(budget)) {
                let sign = match line.kind {
                    LineKind::Added => '+',
                    LineKind::Removed => '-',
                    LineKind::Context => ' ',
                };
                diff.push(sign);
                diff.push_str(line.text.trim_end_matches('\n'));
                diff.push('\n');
                budget = budget.saturating_sub(1);
            }
            let path = if several { format!("{}/{}", self.repos[c.repo].name, c.path) } else { c.path.clone() };
            request.asked.push((self.key(c), c.hash.clone()));
            request.files.push(ToExplain { path, state, diff });
        }
        request
    }
}

/// Changes to explain, asked of a copy of the session.
#[derive(Default)]
pub struct ExplainRequest {
    /// What goes in the question, one per change.
    pub files: Vec<ToExplain>,
    /// Each file's change: its key and fingerprint, to keep the answer under.
    pub asked: Vec<(String, String)>,
}

impl ExplainRequest {
    /// The keys of the changes asked about.
    pub fn keys(&self) -> Vec<String> {
        self.asked.iter().map(|(k, _)| k.clone()).collect()
    }

    /// Asks a copy of `session` (not saved, read-only, the session itself
    /// untouched) in `folder`; blocks while Claude Code answers. The answers
    /// by the file's place in `files`.
    pub fn ask(&self, folder: &Path, session: &str, model: Option<&str>) -> Result<Vec<(usize, String)>, String> {
        shed_ai::aside::explain_changes(&shed_ai::aside::claude_program(), folder, session, model, &self.files)
    }

    /// The answers as explanations to keep: (key, fingerprint, text), as
    /// `AiTab::explained` holds them.
    pub fn answered(&self, answer: Vec<(usize, String)>) -> Vec<(String, String, String)> {
        answer.into_iter().filter_map(|(n, text)| self.asked.get(n).map(|(key, hash)| (key.clone(), hash.clone(), text))).collect()
    }
}

/// Keeps new explanations, each in place of any earlier one of its change.
pub fn keep_explained(explained: &mut Vec<(String, String, String)>, kept: &[(String, String, String)]) {
    for (key, hash, text) in kept {
        explained.retain(|(k, _, _)| k != key);
        explained.push((key.clone(), hash.clone(), text.clone()));
    }
}

/// Whether the change kept under `key` is marked viewed as it is now (`hash`).
pub fn is_viewed(viewed: &[(String, String)], key: &str, hash: &str) -> bool {
    viewed.iter().any(|(k, h)| k == key && h == hash)
}

/// Marks a change viewed (as it is now, `hash`), or not.
pub fn set_viewed(viewed: &mut Vec<(String, String)>, key: &str, hash: &str, on: bool) {
    viewed.retain(|(k, _)| k != key);
    if on {
        viewed.push((key.to_string(), hash.to_string()));
    }
}

/// A stable hash of a change's texts (FNV-1a), kept with Viewed marks.
pub fn fingerprint(old: &str, new: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in old.bytes().chain([0u8]).chain(new.bytes()) {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// When the scope began (Unix seconds): the turn's prompt, or the session's
/// first; `None` before any prompt with a time.
pub fn review_since(entries: &[Entry], scope: Scope) -> Option<u64> {
    let mut prompts = entries.iter().filter_map(|e| if let Entry::User { at, .. } = e { Some(*at) } else { None }).filter(|at| *at > 0);
    match scope {
        Scope::Turn => prompts.next_back(),
        Scope::Session => prompts.next(),
    }
}

/// The folders an AI tab works in: its own, then those it was given.
pub fn dirs(ai: &AiTab) -> Vec<PathBuf> {
    let mut dirs = vec![ai.folder.clone()];
    dirs.extend(ai.add_dirs.iter().cloned());
    dirs
}

/// Reads what changed in each repository since `since` (Unix seconds):
/// what is not staged, staged, then each commit since (with `since`, and a
/// reflog that goes back that far). Runs `git`; blocks.
pub fn read(dirs: Vec<PathBuf>, since: Option<u64>) -> Found {
    let mut repos: Vec<RepoReview> = Vec::new();
    let mut changes = Vec::new();
    let mut cut = false;
    for dir in dirs {
        let Some(repo) = Repo::find(&dir) else { continue };
        if repos.iter().any(|r| r.repo.root == repo.root) {
            continue;
        }
        let index = repos.len();
        let name = repo.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let base = since.and_then(|t| repo.head_at(t));
        let mut add = |layer: Layer, path: String, code: char, old: String, new: String| {
            if changes.len() >= MOST {
                cut = true;
                return;
            }
            let at = Path::new(&path);
            let (hunks, pictures) = if is_picture(at) {
                let (before, after) = match &layer {
                    Layer::Unstaged { untracked } => (if *untracked { Vec::new() } else { repo.show_bytes("", &path) }, std::fs::read(repo.root.join(&path)).unwrap_or_default()),
                    Layer::Staged => (repo.show_bytes("HEAD", &path), repo.show_bytes("", &path)),
                    Layer::Committed { hash, .. } => (repo.show_bytes(&format!("{hash}^"), &path), repo.show_bytes(hash, &path)),
                };
                (Vec::new(), Some((PictureData::from_bytes(at, before), PictureData::from_bytes(at, after))))
            } else {
                (shed_git::diff_texts(&old, &new, 3), None)
            };
            changes.push(Change { repo: index, layer, path, code, hash: fingerprint(&old, &new), hunks, pictures });
        };
        if let Ok(status) = repo.status() {
            // (A new folder too full to list file by file is left out.)
            for e in status.entries.iter().filter(|e| !e.conflict && e.files == 0) {
                if e.untracked || e.worktree != '.' {
                    let old = if e.untracked { String::new() } else { repo.show("", &e.path) };
                    let new = std::fs::read(repo.root.join(&e.path)).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
                    let code = if e.untracked { 'A' } else { e.worktree };
                    add(Layer::Unstaged { untracked: e.untracked }, e.path.clone(), code, old, new);
                }
            }
            for e in status.entries.iter().filter(|e| !e.conflict && !e.untracked && e.index != '.') {
                let (old, new) = (repo.show("HEAD", &e.path), repo.show("", &e.path));
                add(Layer::Staged, e.path.clone(), e.index, old, new);
            }
        }
        let mut commits = Vec::new();
        if let Some(base) = &base {
            for (c, pushed) in repo.commits_since(base).unwrap_or_default() {
                for (code, path) in repo.commit_files(&c.hash).unwrap_or_default() {
                    let (old, new) = (repo.show_at(&format!("{}^", c.hash), &path), repo.show_at(&c.hash, &path));
                    add(Layer::Committed { hash: c.hash.clone(), pushed }, path, code, old, new);
                }
                commits.push(CommitInfo { hash: c.hash, short: c.short, subject: c.subject, pushed });
            }
        }
        repos.push(RepoReview { repo, name, base, commits });
    }
    Found { repos, changes, cut }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(ok.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&ok.stderr));
    }

    fn now() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
    }

    /// A repository with a remote, `a.txt` committed and pushed, the reflog
    /// moved back to well before `start` (returned).
    fn repo_with_remote(dir: &Path) -> (PathBuf, u64) {
        let (work, remote) = (dir.join("work"), dir.join("remote.git"));
        std::fs::create_dir_all(&work).unwrap();
        git(dir, &["init", "-q", "--bare", "-b", "main", remote.to_str().unwrap()]);
        git(&work, &["init", "-q", "-b", "main"]);
        for (k, v) in [("user.email", "t@example.com"), ("user.name", "T"), ("commit.gpgsign", "false")] {
            git(&work, &["config", k, v]);
        }
        git(&work, &["remote", "add", "origin", remote.to_str().unwrap()]);
        std::fs::write(work.join("a.txt"), "a\n").unwrap();
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-qm", "before"]);
        git(&work, &["push", "-q", "-u", "origin", "main"]);
        // Git's record of that, from an hour before the session.
        let start = now() - 600;
        for log in [".git/logs/HEAD", ".git/logs/refs/heads/main"] {
            let path = work.join(log);
            let text = std::fs::read_to_string(&path).unwrap();
            let old = (start - 3000).to_string();
            let fixed: String = text
                .lines()
                .map(|l| {
                    let (head, tail) = l.split_once('\t').unwrap();
                    let mut parts: Vec<&str> = head.split(' ').collect();
                    let n = parts.len();
                    parts[n - 2] = &old;
                    format!("{}\t{tail}\n", parts.join(" "))
                })
                .collect();
            std::fs::write(&path, fixed).unwrap();
        }
        (work, start)
    }

    #[test]
    fn changes_are_sorted_by_how_far_they_went() {
        let dir = tempfile::tempdir().unwrap();
        let (work, start) = repo_with_remote(dir.path());
        let write = |name: &str, text: &str| std::fs::write(work.join(name), text).unwrap();
        // In the session: pushed, committed, staged, not staged.
        write("b.txt", "b\n");
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-qm", "pushed one"]);
        git(&work, &["push", "-q"]);
        write("c.txt", "c\n");
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-qm", "committed one"]);
        write("a.txt", "a, staged\n");
        git(&work, &["add", "a.txt"]);
        write("b.txt", "b, not staged\n");
        let found = read(vec![work.clone()], Some(start));
        let layers: Vec<(String, String)> = found
            .changes
            .iter()
            .map(|c| {
                let layer = match &c.layer {
                    Layer::Unstaged { .. } => "not staged".to_string(),
                    Layer::Staged => "staged".to_string(),
                    Layer::Committed { pushed, .. } => if *pushed { "pushed" } else { "committed" }.to_string(),
                };
                (c.path.clone(), layer)
            })
            .collect();
        let want = |p: &str, l: &str| (p.to_string(), l.to_string());
        assert_eq!(layers, [want("b.txt", "not staged"), want("a.txt", "staged"), want("c.txt", "committed"), want("b.txt", "pushed")]);
        let commits: Vec<(&str, bool)> = found.repos[0].commits.iter().map(|c| (c.subject.as_str(), c.pushed)).collect();
        assert_eq!(commits, [("committed one", false), ("pushed one", true)]);
        assert_eq!(found.staged_repo().map(|r| r.root.clone()), Some(found.repos[0].repo.root.clone()));
        // Before the reflog: what is not committed only.
        let earlier = read(vec![work], Some(start - 7200));
        assert!(earlier.repos[0].base.is_none());
        assert_eq!(earlier.changes.len(), 2);
    }

    #[test]
    fn keys_stay_and_viewed_follows_the_content() {
        let dir = tempfile::tempdir().unwrap();
        let (work, _) = repo_with_remote(dir.path());
        std::fs::write(work.join("a.txt"), "a, changed\n").unwrap();
        std::fs::write(work.join("new.txt"), "new\n").unwrap();
        // The same folder twice (and a folder outside Git) reads one repository.
        let found = read(vec![work.clone(), work.join("."), dir.path().to_path_buf()], None);
        assert_eq!(found.repos.len(), 1);
        let paths: Vec<&str> = found.changes.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, ["a.txt", "new.txt"]);
        assert_eq!(found.changes[1].layer, Layer::Unstaged { untracked: true });
        let a = &found.changes[0];
        let key = found.key(a);
        assert!(key.ends_with("|worktree|a.txt"));
        // Read again: the same keys and fingerprints.
        let again = read(vec![work.clone()], None);
        assert_eq!(again.key(&again.changes[0]), key);
        assert_eq!(again.changes[0].hash, a.hash);
        // Viewed as it is now; changed since, not viewed any more.
        let mut viewed = Vec::new();
        set_viewed(&mut viewed, &key, &a.hash, true);
        assert!(found.is_viewed(a, &viewed));
        assert!(!found.is_viewed(&found.changes[1], &viewed));
        std::fs::write(work.join("a.txt"), "a, changed again\n").unwrap();
        let later = read(vec![work], None);
        assert_eq!(later.key(&later.changes[0]), key);
        assert_ne!(later.changes[0].hash, a.hash);
        assert!(!later.is_viewed(&later.changes[0], &viewed));
        set_viewed(&mut viewed, &key, &a.hash, false);
        assert!(viewed.is_empty());
        assert_eq!(fingerprint("a", "b"), fingerprint("a", "b"));
        assert_ne!(fingerprint("a", "b"), fingerprint("ab", ""));
    }

    #[test]
    fn explanations_fit_the_budget_and_come_back_by_key() {
        let dir = tempfile::tempdir().unwrap();
        let (work, _) = repo_with_remote(dir.path());
        // Three new files of 1000 lines, one changed by a line.
        let long: String = (0..1000).map(|n| format!("line {n}\n")).collect();
        for name in ["x.txt", "y.txt", "z.txt"] {
            std::fs::write(work.join(name), &long).unwrap();
        }
        std::fs::write(work.join("a.txt"), "b\n").unwrap();
        let found = read(vec![work], None);
        assert_eq!(found.changes.len(), 4);
        let request = found.explain_request(&[0, 1, 2, 3, 99]);
        assert_eq!(request.files.len(), 4);
        assert_eq!(request.files[0].path, "a.txt");
        assert_eq!(request.files[0].state, "not staged");
        assert_eq!(request.files[0].diff, "-a\n+b\n");
        for f in &request.files[1..] {
            assert_eq!(f.diff.lines().count(), EXPLAIN_FILE_LINES);
        }
        assert_eq!(request.keys(), found.changes.iter().map(|c| found.key(c)).collect::<Vec<_>>());
        // A small budget is shared out in order.
        let mut many = Found { repos: found.repos.clone(), ..Default::default() };
        for _ in 0..(EXPLAIN_LINES / EXPLAIN_FILE_LINES + 3) {
            many.changes.push(found.changes[1].clone());
        }
        let request = many.explain_request(&(0..many.changes.len()).collect::<Vec<_>>());
        let total: usize = request.files.iter().map(|f| f.diff.lines().count()).sum();
        assert_eq!(total, EXPLAIN_LINES);
        assert!(request.files.last().unwrap().diff.is_empty());
        // Answers kept by key, in place of earlier ones; an earlier version's shows as such.
        let request = found.explain_request(&[0, 1]);
        let kept = request.answered(vec![(1, "Why y".into()), (0, "Why a".into()), (7, "nothing".into())]);
        let mut explained = vec![(found.key(&found.changes[0]), "old".to_string(), "Before".to_string())];
        keep_explained(&mut explained, &kept);
        assert_eq!(explained.len(), 2);
        assert_eq!(found.explanation(&found.changes[0], &explained), Some(("Why a".to_string(), true)));
        assert_eq!(found.explanation(&found.changes[2], &explained), None);
        explained[1].1 = "earlier".into();
        assert_eq!(found.explanation(&found.changes[0], &explained), Some(("Why a".to_string(), false)));
    }

    #[test]
    fn scopes_start_at_prompts() {
        let user = |at: u64| Entry::User { text: String::new(), images: 0, at, pictures: Vec::new() };
        let entries = vec![user(0), user(100), Entry::Notice { text: String::new(), error: false }, user(200)];
        assert_eq!(review_since(&entries, Scope::Session), Some(100));
        assert_eq!(review_since(&entries, Scope::Turn), Some(200));
        assert_eq!(review_since(&[], Scope::Turn), None);
    }
}
