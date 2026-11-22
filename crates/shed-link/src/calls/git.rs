//! Git in a project or AI tab's folder, as the project view does it.

use std::path::Path;

use shed_api::calls::{self as api, Call, DiffSource, HunkAction, Reply};
use shed_api::{ApiError, ErrorCode};
use shed_git::Repo;
use shed_git::session as gs;
use shed_model::Shed;

use super::{invalid, not_found};
use crate::keeper;
use crate::paths::{GitScope, git_name};

fn git_err(e: shed_git::GitError) -> ApiError {
    ApiError::new(ErrorCode::Failed, e.0)
}

pub(super) async fn call(tree: &Shed, call: Call) -> Result<Reply, ApiError> {
    let tab = match &call {
        Call::GitStatus(a) | Call::GitBranches(a) | Call::GitFetch(a) | Call::GitPull(a) => &a.tab,
        Call::GitDiff(a) => &a.tab,
        Call::GitStage(a) | Call::GitUnstage(a) | Call::GitDiscard(a) => &a.tab,
        Call::GitHunk(a) => &a.tab,
        Call::GitCommit(a) => &a.tab,
        Call::GitLog(a) => &a.tab,
        Call::GitCommitFiles(a) => &a.tab,
        Call::GitSwitch(a) | Call::GitCreateBranch(a) => &a.tab,
        Call::GitPush(a) => &a.tab,
        _ => return Err(ApiError::new(ErrorCode::Unsupported, "not a Git call")),
    };
    let (root, _) = super::project::root_of(tree, tab)?;
    tokio::task::spawn_blocking(move || {
        let repo = Repo::find(&root);
        if let Call::GitStatus(_) = call {
            return Ok(Reply::GitStatus(match repo {
                Some(repo) => status(&repo, &GitScope::new(&repo.root, &root)?)?,
                None => api::GitStatus::default(),
            }));
        }
        let repo = repo.ok_or_else(|| not_found("not a Git repository"))?;
        // Paths are the repository's, but only those in the tab's folder.
        let scope = GitScope::new(&repo.root, &root)?;
        blocking(&repo, &scope, call)
    })
    .await
    .map_err(|e| keeper::failed(e.to_string()))?
}

fn blocking(repo: &Repo, scope: &GitScope, call: Call) -> Result<Reply, ApiError> {
    let paths = |ps: &[String]| ps.iter().map(|p| scope.path(p)).collect::<Result<Vec<_>, _>>();
    match call {
        Call::GitDiff(a) => Ok(Reply::Diff(diff(repo, &scope.path(&a.path)?, &a.source)?.0)),
        Call::GitStage(a) => repo.stage(&paths(&a.paths)?).map(|_| Reply::Done).map_err(git_err),
        Call::GitUnstage(a) => repo.unstage(&paths(&a.paths)?).map(|_| Reply::Done).map_err(git_err),
        Call::GitDiscard(a) => {
            let wanted = paths(&a.paths)?;
            let status = repo.status().map_err(git_err)?;
            let untracked = |p: &String| status.entries.iter().any(|e| e.path == *p && e.untracked);
            let (gone, tracked): (Vec<String>, Vec<String>) = wanted.into_iter().partition(untracked);
            repo.discard(&tracked, &gone).map(|_| Reply::Done).map_err(git_err)
        }
        Call::GitHunk(a) => {
            let path = scope.path(&a.path)?;
            let (diff, hunks, untracked) = diff(repo, &path, &a.source)?;
            let (Some(shown), Some(hunk)) = (diff.hunks.get(a.hunk as usize), hunks.get(a.hunk as usize)) else {
                return Err(not_found("that change is not there any more"));
            };
            if shown.header != a.header {
                return Err(invalid("the file changed since; look at its changes again"));
            }
            if untracked && a.action != HunkAction::Stage {
                return Err(invalid("a new file's changes are staged whole"));
            }
            let action = match a.action {
                HunkAction::Stage => gs::HunkAction::Stage,
                HunkAction::Unstage => gs::HunkAction::Unstage,
                HunkAction::Discard => gs::HunkAction::Discard,
            };
            gs::hunk_action(repo, &path, hunk, action, untracked).map(|_| Reply::Done).map_err(git_err)
        }
        Call::GitCommit(a) => {
            gs::commit(repo, &a.message, a.amend).map_err(git_err)?;
            let hash = repo.run(&["rev-parse", "HEAD"]).map_err(git_err)?;
            Ok(Reply::Text(hash.trim().to_string()))
        }
        Call::GitLog(a) => {
            let path = a.path.as_deref().map(|p| scope.path(p)).transpose()?;
            let limit = if a.limit == 0 { gs::LOG_LIMIT } else { a.limit as usize };
            Ok(Reply::Commits(gs::history(repo, path.as_deref()).map_err(git_err)?.iter().take(limit).map(commit).collect()))
        }
        Call::GitCommitFiles(a) => {
            let files = repo.commit_files(git_name(&a.hash)?).map_err(git_err)?;
            Ok(Reply::CommitFiles(files.into_iter().map(|(status, path)| api::CommitFile { status: status.to_string(), path }).collect()))
        }
        Call::GitBranches(_) => {
            let current = repo.status().ok().and_then(|s| s.branch.head);
            Ok(Reply::Branches(api::Branches { current, local: repo.branches().map_err(git_err)?, remote: repo.remote_branches().unwrap_or_default() }))
        }
        Call::GitSwitch(a) => repo.switch(git_name(&a.branch)?).map(Reply::Text).map_err(git_err),
        Call::GitCreateBranch(a) => repo.create_branch(git_name(&a.branch)?).map(Reply::Text).map_err(git_err),
        Call::GitFetch(_) => repo.fetch().map(Reply::Text).map_err(git_err),
        Call::GitPull(_) => repo.pull().map(Reply::Text).map_err(git_err),
        Call::GitPush(a) => {
            // Where it goes is as the project view's push review shows it.
            let review = gs::PushReview::read(repo, a.force).map_err(git_err)?;
            if review.branch.head.is_none() {
                return Err(invalid("no branch is checked out"));
            }
            review.confirm(repo).map(Reply::Text).map_err(git_err)
        }
        _ => Err(ApiError::new(ErrorCode::Unsupported, "not a Git call")),
    }
}

/// The status with what is not pushed yet, as the project view shows it;
/// only the files in the tab's folder.
fn status(repo: &Repo, scope: &GitScope) -> Result<api::GitStatus, ApiError> {
    let gs::Refreshed { status: s, outgoing } = gs::refresh(repo).map_err(git_err)?;
    let letter = |c: char| if c == '.' { String::new() } else { c.to_string() };
    Ok(api::GitStatus {
        repo: true,
        branch: s.branch.head.clone(),
        upstream: s.branch.upstream.clone(),
        ahead: s.branch.ahead.max(outgoing.len() as u32),
        behind: s.branch.behind,
        operation: s.operation.clone(),
        files: s
            .entries
            .iter()
            .filter(|e| scope.shows(&e.path))
            .map(|e| api::GitFile {
                path: e.path.clone(),
                staged: if e.untracked { String::new() } else { letter(e.index) },
                unstaged: if e.untracked { "?".into() } else { letter(e.worktree) },
                conflict: e.conflict,
                from: e.orig.clone(),
            })
            .collect(),
        outgoing: outgoing.iter().map(commit).collect(),
        root: Some(repo.root.to_string_lossy().into_owned()),
        head: Some(s.branch.oid.clone()).filter(|o| !o.is_empty() && o != "(initial)"),
    })
}

fn commit(c: &shed_git::Commit) -> api::Commit {
    api::Commit { hash: c.hash.clone(), short: c.short.clone(), subject: c.subject.clone(), author: c.author.clone(), time: c.time.min(u32::MAX as u64) as u32 }
}

/// A file's changes from `source`, as the project view loads them: the two
/// versions compared, pictures side by side. Also the hunks themselves (for
/// patches), and whether the file is new to Git.
fn diff(repo: &Repo, path: &str, source: &DiffSource) -> Result<(api::Diff, Vec<shed_git::Hunk>, bool), ApiError> {
    let untracked = matches!(source, DiffSource::Unstaged) && repo.status().is_ok_and(|s| s.entries.iter().any(|e| e.path == path && e.untracked));
    let source = match source {
        DiffSource::Unstaged => gs::DiffSource::Unstaged,
        DiffSource::Staged => gs::DiffSource::Staged,
        DiffSource::Commit(h) => gs::DiffSource::Commit(git_name(h)?.to_string()),
    };
    if shed_project::picture::is_picture(Path::new(path)) {
        let gs::DiffContent::Pictures { old, new } = gs::load_diff(repo, path, &source, untracked, true) else { unreachable!("asked for pictures") };
        let at = Path::new(path);
        let picture = |b: Vec<u8>| shed_project::picture::PictureData::from_bytes(at, b).map(|p| super::ai::picture_of(&p));
        return Ok((api::Diff { hunks: Vec::new(), old_picture: picture(old), new_picture: picture(new), binary: false }, Vec::new(), untracked));
    }
    let (old, new) = gs::diff_bytes_of(repo, path, &source, untracked);
    let binary = |b: &[u8]| b[..b.len().min(8192)].contains(&0);
    if binary(&old) || binary(&new) {
        return Ok((api::Diff { binary: true, ..Default::default() }, Vec::new(), untracked));
    }
    let (old, new) = (String::from_utf8_lossy(&old), String::from_utf8_lossy(&new));
    let hunks = shed_git::diff_texts(&old, &new, 3);
    Ok((api::Diff { hunks: shown(&hunks), ..Default::default() }, hunks, untracked))
}

/// Hunks as connectors get them.
pub(crate) fn shown(hunks: &[shed_git::Hunk]) -> Vec<api::Hunk> {
    hunks
        .iter()
        .map(|h| api::Hunk {
            header: h.header(),
            lines: h
                .lines
                .iter()
                .map(|l| api::DiffLine {
                    kind: match l.kind {
                        shed_git::LineKind::Context => api::DiffLineKind::Context,
                        shed_git::LineKind::Added => api::DiffLineKind::Added,
                        shed_git::LineKind::Removed => api::DiffLineKind::Removed,
                    },
                    text: l.text.clone(),
                    old: l.old.map(|n| n as u32),
                    new: l.new.map(|n| n as u32),
                })
                .collect(),
        })
        .collect()
}
