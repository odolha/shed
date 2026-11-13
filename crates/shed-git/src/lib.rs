//! Git, through the `git` program: the user's config, credential helpers,
//! SSH agent, hooks and signing work as in their terminal. Reads use Git's
//! machine formats; nothing here has a UI or holds state.

mod diff;
mod status;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub use diff::{Hunk, Line, LineKind, diff_texts, hunk_patch};
pub use status::{Branch, Entry, Status, parse_status};

/// What a Git command said when it failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitError(pub String);

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub type Result<T> = std::result::Result<T, GitError>;

/// A repository, by its working folder.
#[derive(Clone, Debug)]
pub struct Repo {
    pub root: PathBuf,
}

impl Repo {
    /// The repository `dir` is in, if any.
    pub fn find(dir: &Path) -> Option<Repo> {
        let out = Command::new("git").arg("-C").arg(dir).args(["rev-parse", "--show-toplevel"]).output().ok()?;
        out.status.success().then(|| Repo { root: PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()) })
    }

    fn git(&self) -> Command {
        let mut c = Command::new("git");
        c.arg("-C").arg(&self.root);
        // Never wait on a terminal prompt nobody can see; say why it failed instead.
        c.env("GIT_TERMINAL_PROMPT", "0");
        c.env("GIT_OPTIONAL_LOCKS", "0");
        c.stdin(Stdio::null());
        c
    }

    /// Runs git with `args`; its output, or what it said on failure.
    pub fn run(&self, args: &[&str]) -> Result<String> {
        self.run_with_input(args, None)
    }

    fn run_with_input(&self, args: &[&str], input: Option<&[u8]>) -> Result<String> {
        let mut cmd = self.git();
        cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
        if input.is_some() {
            cmd.stdin(Stdio::piped());
        }
        let mut child = cmd.spawn().map_err(|e| GitError(format!("cannot run git: {e}")))?;
        if let Some(input) = input {
            let mut stdin = child.stdin.take().unwrap();
            stdin.write_all(input).map_err(|e| GitError(e.to_string()))?;
        }
        let out = child.wait_with_output().map_err(|e| GitError(e.to_string()))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let err = if err.is_empty() { String::from_utf8_lossy(&out.stdout).trim().to_string() } else { err };
            Err(GitError(if err.is_empty() { format!("git {} failed", args.first().unwrap_or(&"")) } else { err }))
        }
    }

    pub fn status(&self) -> Result<Status> {
        let out = self.run(&["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all"])?;
        let mut status = parse_status(&out);
        status.operation = self.operation();
        Ok(status)
    }

    /// A merge, rebase, cherry-pick or revert under way, by its name.
    fn operation(&self) -> Option<String> {
        let git_dir = self.run(&["rev-parse", "--git-dir"]).ok()?;
        let git_dir = self.root.join(git_dir.trim());
        [("MERGE_HEAD", "merge"), ("rebase-merge", "rebase"), ("rebase-apply", "rebase"), ("CHERRY_PICK_HEAD", "cherry-pick"), ("REVERT_HEAD", "revert")]
            .iter()
            .find(|(f, _)| git_dir.join(f).exists())
            .map(|(_, name)| name.to_string())
    }

    /// A file as it is in a revision (`HEAD`, a branch) or the index (`""`);
    /// empty if it is not there.
    pub fn show(&self, rev: &str, path: &str) -> String {
        self.run(&["show", &format!("{rev}:{path}")]).unwrap_or_default()
    }

    pub fn stage(&self, paths: &[String]) -> Result<()> {
        let mut args = vec!["add", "--"];
        args.extend(paths.iter().map(String::as_str));
        self.run(&args).map(drop)
    }

    pub fn unstage(&self, paths: &[String]) -> Result<()> {
        // `restore --staged` needs a HEAD; before the first commit, `rm --cached`.
        if self.run(&["rev-parse", "--verify", "-q", "HEAD"]).is_err() {
            let mut args = vec!["rm", "--cached", "-q", "--"];
            args.extend(paths.iter().map(String::as_str));
            return self.run(&args).map(drop);
        }
        let mut args = vec!["restore", "--staged", "--"];
        args.extend(paths.iter().map(String::as_str));
        self.run(&args).map(drop)
    }

    /// Throws away changes in the working folder: back to the index for
    /// tracked files, deleted for untracked ones.
    pub fn discard(&self, tracked: &[String], untracked: &[String]) -> Result<()> {
        if !tracked.is_empty() {
            let mut args = vec!["restore", "--"];
            args.extend(tracked.iter().map(String::as_str));
            self.run(&args)?;
        }
        for path in untracked {
            let full = self.root.join(path);
            let removed = if full.is_dir() { std::fs::remove_dir_all(&full) } else { std::fs::remove_file(&full) };
            removed.map_err(|e| GitError(format!("cannot delete {path}: {e}")))?;
        }
        Ok(())
    }

    /// Applies a patch (from `hunk_patch`) to the index (`cached`) or the
    /// working folder, forwards or (`reverse`) backwards.
    pub fn apply(&self, patch: &str, cached: bool, reverse: bool) -> Result<()> {
        let mut args = vec!["apply", "--whitespace=nowarn"];
        if cached {
            args.push("--cached");
        }
        if reverse {
            args.push("-R");
        }
        args.push("-");
        self.run_with_input(&args, Some(patch.as_bytes())).map(drop)
    }

    pub fn commit(&self, message: &str, amend: bool) -> Result<String> {
        let mut args = vec!["commit", "-F", "-"];
        if amend {
            args.push("--amend");
        }
        self.run_with_input(&args, Some(message.as_bytes()))
    }

    /// The last commit's message, for amending.
    pub fn last_message(&self) -> String {
        self.run(&["log", "-1", "--format=%B"]).map(|m| m.trim_end().to_string()).unwrap_or_default()
    }

    pub fn fetch(&self) -> Result<String> {
        self.run(&["fetch", "--all", "--prune"])
    }

    pub fn pull(&self) -> Result<String> {
        self.run(&["pull"])
    }

    /// Pulls, putting the branch's own commits on top of the remote's.
    pub fn pull_rebase(&self) -> Result<String> {
        self.run(&["pull", "--rebase"])
    }

    /// Pulls, merging the remote's commits into the branch.
    pub fn pull_merge(&self) -> Result<String> {
        self.run(&["pull", "--no-rebase"])
    }

    /// Pushes the branch; one without an upstream gets one on `origin`.
    pub fn push(&self, force: bool, branch: &Branch) -> Result<String> {
        let mut args = vec!["push"];
        if force {
            args.push("--force-with-lease");
        }
        let head = branch.head.clone().unwrap_or_default();
        if branch.upstream.is_none() && !head.is_empty() {
            args.extend(["-u", "origin"]);
            args.push(&head);
            return self.run(&args);
        }
        self.run(&args)
    }

    /// Local branches, the current first, then by last commit.
    pub fn branches(&self) -> Result<Vec<String>> {
        let out = self.run(&["branch", "--format=%(HEAD)%(refname:short)", "--sort=-committerdate"])?;
        let mut branches: Vec<(bool, String)> = out.lines().filter(|l| !l.is_empty()).map(|l| (l.starts_with('*'), l[1..].to_string())).collect();
        branches.sort_by_key(|(current, _)| !current);
        Ok(branches.into_iter().map(|(_, b)| b).collect())
    }

    /// Remote branches not checked out here, as `origin/name`.
    pub fn remote_branches(&self) -> Result<Vec<String>> {
        let out = self.run(&["branch", "-r", "--format=%(refname:short)", "--sort=-committerdate"])?;
        Ok(out.lines().filter(|l| !l.is_empty() && !l.ends_with("/HEAD") && l.contains('/')).map(String::from).collect())
    }

    pub fn switch(&self, branch: &str) -> Result<String> {
        // `origin/x` makes and tracks a local `x`.
        match branch.split_once('/') {
            Some((remote, name)) if self.run(&["remote"]).is_ok_and(|r| r.lines().any(|l| l == remote)) => self.run(&["switch", "-c", name, "--track", branch]),
            _ => self.run(&["switch", branch]),
        }
    }

    pub fn create_branch(&self, name: &str) -> Result<String> {
        self.run(&["switch", "-c", name])
    }

    pub fn delete_branch(&self, name: &str, force: bool) -> Result<String> {
        self.run(&["branch", if force { "-D" } else { "-d" }, name])
    }

    pub fn merge(&self, branch: &str) -> Result<String> {
        self.run(&["merge", "--no-edit", branch])
    }

    pub fn rebase(&self, onto: &str) -> Result<String> {
        self.run(&["rebase", onto])
    }

    /// Ends the merge, rebase or cherry-pick under way: `continue` or `abort`.
    pub fn finish_operation(&self, operation: &str, how: &str) -> Result<String> {
        match (operation, how) {
            ("merge", "continue") => self.run(&["commit", "--no-edit"]),
            ("rebase", "continue") => {
                let mut c = self.git();
                c.env("GIT_EDITOR", "true").args(["rebase", "--continue"]);
                let out = c.output().map_err(|e| GitError(e.to_string()))?;
                if out.status.success() { Ok(String::new()) } else { Err(GitError(String::from_utf8_lossy(&out.stderr).trim().to_string())) }
            }
            (op, how) => self.run(&[op, &format!("--{how}")]),
        }
    }

    pub fn stash(&self) -> Result<String> {
        self.run(&["stash", "push", "--include-untracked"])
    }

    pub fn stash_pop(&self) -> Result<String> {
        self.run(&["stash", "pop"])
    }

    /// Takes our or their side of a conflicted file whole, and marks it resolved.
    pub fn resolve_with(&self, path: &str, ours: bool) -> Result<()> {
        self.run(&["checkout", if ours { "--ours" } else { "--theirs" }, "--", path])?;
        self.stage(&[path.to_string()])
    }

    /// Commits, newest first: hash, short hash, author, Unix time, refs, subject.
    pub fn log(&self, path: Option<&str>, limit: usize) -> Result<Vec<Commit>> {
        match path {
            Some(p) => self.log_of(&["--follow", "--", p], limit),
            None => self.log_of(&[], limit),
        }
    }

    /// Where pushing the branch goes: its upstream, else a new branch of the
    /// same name on `origin` (as `push` does).
    pub fn push_target(&self, branch: &Branch) -> PushTarget {
        let head = branch.head.clone().unwrap_or_default();
        let config = |key: String| self.run(&["config", "--get", &key]).map(|v| v.trim().to_string()).ok().filter(|v| !v.is_empty());
        let upstream = branch.upstream.is_some().then(|| {
            let remote = config(format!("branch.{head}.remote"));
            let merge = config(format!("branch.{head}.merge")).map(|m| m.trim_start_matches("refs/heads/").to_string());
            (remote, merge)
        });
        let (remote, remote_branch, new) = match upstream {
            Some((Some(remote), Some(merge))) => (remote, merge, false),
            _ => ("origin".to_string(), head.clone(), branch.upstream.is_none()),
        };
        let url = self.run(&["remote", "get-url", &remote]).map(|u| u.trim().to_string()).unwrap_or_default();
        PushTarget { branch: head, remote, remote_branch, url, new }
    }

    /// The branch's commits its upstream does not have yet (newest first);
    /// without an upstream, those on no remote branch.
    pub fn outgoing(&self, branch: &Branch, limit: usize) -> Result<Vec<Commit>> {
        if branch.head.is_none() {
            return Ok(Vec::new());
        }
        if branch.upstream.is_some() {
            self.log_of(&["@{upstream}..HEAD"], limit)
        } else {
            self.log_of(&["HEAD", "--not", "--remotes"], limit)
        }
    }

    fn log_of(&self, extra: &[&str], limit: usize) -> Result<Vec<Commit>> {
        let n = format!("-{limit}");
        let mut args = vec!["log", &n, "--format=%H%x00%h%x00%an%x00%at%x00%D%x00%s%x00%P%x1e"];
        args.extend_from_slice(extra);
        let out = self.run(&args)?;
        Ok(out
            .split('\x1e')
            .filter_map(|rec| {
                let f: Vec<&str> = rec.trim_start_matches('\n').split('\0').collect();
                (f.len() >= 7).then(|| Commit {
                    hash: f[0].into(),
                    short: f[1].into(),
                    author: f[2].into(),
                    time: f[3].parse().unwrap_or(0),
                    refs: f[4].into(),
                    subject: f[5].into(),
                    parents: f[6].split_whitespace().map(String::from).collect(),
                })
            })
            .collect())
    }

    /// The files a commit changed, with their status letter.
    pub fn commit_files(&self, hash: &str) -> Result<Vec<(char, String)>> {
        let out = self.run(&["show", "--format=", "--name-status", "-z", hash])?;
        let parts: Vec<&str> = out.split('\0').filter(|p| !p.is_empty()).collect();
        let mut files = Vec::new();
        let mut i = 0;
        while i < parts.len() {
            let code = parts[i].chars().next().unwrap_or('M');
            // Renames and copies name two paths.
            let n = if matches!(code, 'R' | 'C') { 2 } else { 1 };
            if let Some(path) = parts.get(i + n) {
                files.push((code, path.to_string()));
            }
            i += n + 1;
        }
        Ok(files)
    }

    /// Who last changed each line of a file, as it is in the working folder.
    pub fn blame(&self, path: &str) -> Result<Vec<BlameLine>> {
        let out = self.run(&["blame", "--line-porcelain", "--", path])?;
        let mut lines = Vec::new();
        let mut cur = BlameLine::default();
        for l in out.lines() {
            if let Some(rest) = l.strip_prefix("author ") {
                cur.author = rest.to_string();
            } else if let Some(rest) = l.strip_prefix("author-time ") {
                cur.time = rest.parse().unwrap_or(0);
            } else if let Some(rest) = l.strip_prefix("summary ") {
                cur.summary = rest.to_string();
            } else if l.starts_with('\t') {
                lines.push(cur.clone());
            } else if l.len() >= 40 && l.as_bytes()[..40].iter().all(u8::is_ascii_hexdigit) {
                cur.hash = l[..8].to_string();
            }
        }
        Ok(lines)
    }
}

/// A line's last change.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlameLine {
    /// Short; all zeros for a change not committed yet.
    pub hash: String,
    pub author: String,
    pub time: u64,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub author: String,
    pub time: u64,
    /// Branches and tags pointing at it, as `git log %D` writes them.
    pub refs: String,
    pub subject: String,
    pub parents: Vec<String>,
}

/// Where a push goes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PushTarget {
    /// The local branch pushed.
    pub branch: String,
    pub remote: String,
    /// The branch on the remote.
    pub remote_branch: String,
    /// The remote's address.
    pub url: String,
    /// The remote branch is made by this push.
    pub new: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> (tempfile::TempDir, Repo) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let r = Repo { root };
        r.run(&["init", "-q", "-b", "main"]).unwrap();
        r.run(&["config", "user.email", "t@example.com"]).unwrap();
        r.run(&["config", "user.name", "T"]).unwrap();
        r.run(&["config", "commit.gpgsign", "false"]).unwrap();
        (dir, r)
    }

    #[test]
    fn status_stage_commit_and_hunks() {
        let (_d, r) = repo();
        std::fs::write(r.root.join("f.txt"), "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n").unwrap();
        r.stage(&["f.txt".into()]).unwrap();
        r.commit("first\n", false).unwrap();
        // Two changes far apart: two hunks.
        std::fs::write(r.root.join("f.txt"), "A\nb\nc\nd\ne\nf\ng\nh\ni\nJ\n").unwrap();
        std::fs::write(r.root.join("new.txt"), "x\n").unwrap();
        let s = r.status().unwrap();
        assert_eq!(s.branch.head.as_deref(), Some("main"));
        assert_eq!(s.entries.len(), 2);
        let f = s.entries.iter().find(|e| e.path == "f.txt").unwrap();
        assert_eq!((f.index, f.worktree), ('.', 'M'));
        assert!(s.entries.iter().any(|e| e.path == "new.txt" && e.untracked));

        let old = r.show("", "f.txt");
        let new = std::fs::read_to_string(r.root.join("f.txt")).unwrap();
        let hunks = diff_texts(&old, &new, 1);
        assert_eq!(hunks.len(), 2);
        // Stage only the first change.
        r.apply(&hunk_patch("f.txt", &hunks[0]), true, false).unwrap();
        let staged = r.show("", "f.txt");
        assert!(staged.starts_with("A\n") && staged.ends_with("j\n"), "{staged}");
        // And discard the other from the working folder.
        let unstaged = diff_texts(&r.show("", "f.txt"), &std::fs::read_to_string(r.root.join("f.txt")).unwrap(), 1);
        r.apply(&hunk_patch("f.txt", &unstaged[0]), false, true).unwrap();
        assert!(std::fs::read_to_string(r.root.join("f.txt")).unwrap().ends_with("j\n"));
        r.commit("second", false).unwrap();
        let log = r.log(None, 10).unwrap();
        assert_eq!(log.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["second", "first"]);
        assert_eq!(r.commit_files(&log[0].hash).unwrap(), vec![('M', "f.txt".to_string())]);
        assert_eq!(r.blame("f.txt").unwrap().len(), 10);
        r.discard(&[], &["new.txt".into()]).unwrap();
        assert!(r.status().unwrap().entries.is_empty());
    }

    #[test]
    fn outgoing_commits_and_where_a_push_goes() {
        let (_d, r) = repo();
        let remote = tempfile::tempdir().unwrap();
        Repo { root: remote.path().to_path_buf() }.run(&["init", "-q", "--bare", "-b", "main"]).unwrap();
        r.run(&["remote", "add", "origin", &remote.path().display().to_string()]).unwrap();
        let commit = |name: &str| {
            std::fs::write(r.root.join(name), name).unwrap();
            r.stage(&[name.into()]).unwrap();
            r.commit(name, false).unwrap();
        };
        commit("one");
        // No upstream yet: a new branch on origin, everything outgoing.
        let branch = r.status().unwrap().branch;
        let target = r.push_target(&branch);
        assert_eq!((target.remote.as_str(), target.remote_branch.as_str(), target.new), ("origin", "main", true));
        assert_eq!(target.url, remote.path().display().to_string());
        assert_eq!(r.outgoing(&branch, 10).unwrap().len(), 1);
        r.push(false, &branch).unwrap();
        let branch = r.status().unwrap().branch;
        assert!(r.outgoing(&branch, 10).unwrap().is_empty());
        commit("two");
        commit("three");
        let branch = r.status().unwrap().branch;
        let out: Vec<String> = r.outgoing(&branch, 10).unwrap().into_iter().map(|c| c.subject).collect();
        assert_eq!(out, ["three", "two"]);
        let target = r.push_target(&branch);
        assert_eq!((target.remote_branch.as_str(), target.new), ("main", false));
    }

    #[test]
    fn conflicts_show_and_resolve() {
        let (_d, r) = repo();
        std::fs::write(r.root.join("f.txt"), "one\n").unwrap();
        r.stage(&["f.txt".into()]).unwrap();
        r.commit("base", false).unwrap();
        r.create_branch("other").unwrap();
        std::fs::write(r.root.join("f.txt"), "theirs\n").unwrap();
        r.stage(&["f.txt".into()]).unwrap();
        r.commit("theirs", false).unwrap();
        r.switch("main").unwrap();
        std::fs::write(r.root.join("f.txt"), "ours\n").unwrap();
        r.stage(&["f.txt".into()]).unwrap();
        r.commit("ours", false).unwrap();
        assert!(r.merge("other").is_err());
        let s = r.status().unwrap();
        assert_eq!(s.operation.as_deref(), Some("merge"));
        assert!(s.entries.iter().any(|e| e.conflict && e.path == "f.txt"));
        r.resolve_with("f.txt", false).unwrap();
        r.finish_operation("merge", "continue").unwrap();
        assert_eq!(std::fs::read_to_string(r.root.join("f.txt")).unwrap(), "theirs\n");
        assert!(r.status().unwrap().operation.is_none());
        assert_eq!(r.branches().unwrap()[0], "main");
    }
}
