//! Git in a project tab: the branch and how it stands against its upstream,
//! always in view; changed files to stage, unstage and throw away, by file
//! or by hunk; diffs; commit, fetch, pull, push; branches; history; and
//! conflicts. Every Git command runs off the UI thread, through `shed-git`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{
    AnyElement, Context, Entity, Focusable, MouseButton, MouseDownEvent, PromptLevel, Subscription, Task, UniformListScrollHandle,
    Window, div, prelude::*, px, svg, uniform_list,
};
use shed_git::session::{self as gs, BranchOp, DiffContent, GitSession, HunkAction, OUTGOING_LIMIT, PushReview};
use shed_git::{Commit, Entry as GitEntry, Hunk, Repo};
use shed_model::Rgb;

use super::{Content, OpenFile, Panel, ProjectView, ROW_HEIGHT, remote};
use crate::workspace_api::says;
use shed_api::calls::{self as api, Call, Reply};
use crate::assets::icon;
use crate::line_editor::{EditorEvent, LineEditor};
use crate::menu::{self, MenuEntry, MenuHost};
use crate::settings::Settings;
use crate::text_view::{TextEvent, TextOptions, TextView};
use crate::theme::{UiColors, current_ui, hsla, hsla_alpha};

use crate::changes_ui::{DIFF_ROW, DiffRow, FolderRow, folder_rows, status_color};
use crate::picture::Picture;

/// The most rows a section of the Changes panel lists.
const MAX_ROWS: usize = 1000;

pub(super) use shed_git::session::DiffSource;

/// What the panel says of Git operations another workspace's calls do not
/// offer.
const NOT_REMOTE: &str = "Not available for a remote workspace: do this in a terminal tab there.";

/// Something the panel asks of the repository: run here through `git`
/// (`run`), or on another workspace as a call (`call`).
pub(super) enum GitOp {
    Stage(Vec<String>),
    Unstage(Vec<String>),
    Discard { tracked: Vec<String>, untracked: Vec<String> },
    /// One hunk of a diff (its index and header, as shown).
    Hunk { path: String, source: DiffSource, index: usize, header: String, hunk: Hunk, action: HunkAction, untracked: bool },
    Commit { message: String, amend: bool },
    Push(PushReview),
    Pull,
    Branch(BranchOp),
    /// Continues or aborts a merge, rebase or the like.
    Finish { operation: String, how: &'static str },
    /// Takes one side of a conflicted file.
    Resolve { path: String, ours: bool },
}

impl GitOp {
    fn run(self, repo: &Repo) -> shed_git::Result<String> {
        match self {
            GitOp::Stage(paths) => repo.stage(&paths).map(|_| String::new()),
            GitOp::Unstage(paths) => repo.unstage(&paths).map(|_| String::new()),
            GitOp::Discard { tracked, untracked } => repo.discard(&tracked, &untracked).map(|_| String::new()),
            GitOp::Hunk { path, hunk, action, untracked, .. } => gs::hunk_action(repo, &path, &hunk, action, untracked).map(|_| String::new()),
            GitOp::Commit { message, amend } => gs::commit(repo, &message, amend),
            GitOp::Push(review) => review.confirm(repo),
            GitOp::Pull => repo.pull(),
            GitOp::Branch(op) => op.run(repo),
            GitOp::Finish { operation, how } => repo.finish_operation(&operation, how),
            GitOp::Resolve { path, ours } => repo.resolve_with(&path, ours).map(|_| String::new()),
        }
    }

    /// The call doing it on another workspace (tab `tab`); `None` where the
    /// API has none.
    pub(super) fn call(&self, tab: &str) -> Option<Call> {
        let tab = tab.to_string();
        let paths = |paths: &[String]| api::GitPaths { tab: tab.clone(), paths: paths.to_vec() };
        Some(match self {
            GitOp::Stage(p) => Call::GitStage(paths(p)),
            GitOp::Unstage(p) => Call::GitUnstage(paths(p)),
            GitOp::Discard { tracked, untracked } => Call::GitDiscard(paths(&[tracked.clone(), untracked.clone()].concat())),
            // A new file is staged whole, as here.
            GitOp::Hunk { path, untracked: true, .. } => Call::GitStage(paths(std::slice::from_ref(path))),
            GitOp::Hunk { path, source, index, header, action, .. } => Call::GitHunk(api::GitHunk {
                tab,
                path: path.clone(),
                source: remote::source_of(source),
                hunk: *index as u32,
                header: header.clone(),
                action: match action {
                    HunkAction::Stage => api::HunkAction::Stage,
                    HunkAction::Unstage => api::HunkAction::Unstage,
                    HunkAction::Discard => api::HunkAction::Discard,
                },
            }),
            GitOp::Commit { message, amend } => Call::GitCommit(api::GitCommit { tab, message: message.clone(), amend: *amend }),
            GitOp::Push(review) => Call::GitPush(api::GitPush { tab, force: review.force }),
            GitOp::Pull => Call::GitPull(api::TabOnly { tab }),
            GitOp::Branch(BranchOp::Fetch) => Call::GitFetch(api::TabOnly { tab }),
            GitOp::Branch(BranchOp::Switch(b) | BranchOp::CheckOut(b)) => Call::GitSwitch(api::GitBranch { tab, branch: b.clone() }),
            GitOp::Branch(BranchOp::Create(name)) => Call::GitCreateBranch(api::GitBranch { tab, branch: name.replace(' ', "-") }),
            GitOp::Branch(_) | GitOp::Finish { .. } | GitOp::Resolve { .. } => return None,
        })
    }
}

/// A project tab's Git: the repository and what is known of it
/// (`GitSession`), and what runs and is shown here.
pub(super) struct GitState {
    pub session: GitSession,
    /// What runs now (`Pushing…`), if anything.
    pub busy: Option<String>,
    pub error: Option<String>,
    refresh: Option<Task<()>>,
    op: Option<Task<()>>,
    _fetcher: Option<Task<()>>,
    pub message: Entity<TextView>,
    log_task: Option<Task<()>>,
    /// Commits opened to their files.
    expanded: HashSet<String>,
    history_scroll: UniformListScrollHandle,
    new_branch: Option<(Entity<LineEditor>, Subscription)>,
    /// Folders closed in the tree of changed files, per section.
    collapsed: HashSet<(Section, String)>,
    _message_sub: Subscription,
}

pub(super) struct DiffView {
    /// Relative to the repository's root.
    pub path: String,
    pub source: DiffSource,
    hunks: Vec<Hunk>,
    rows: Vec<DiffRow>,
    side: bool,
    scroll: UniformListScrollHandle,
    untracked: bool,
    /// For a picture: its two versions, shown side by side in place of hunks.
    pictures: Option<(Option<Picture>, Option<Picture>)>,
    /// Hunks' headers as another workspace sent them (naming a hunk to it).
    headers: Vec<String>,
}

impl DiffView {
    pub fn title(&self) -> String {
        let name = Path::new(&self.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match &self.source {
            DiffSource::Unstaged => format!("Δ {name}"),
            DiffSource::Staged => format!("Δ {name} (staged)"),
            DiffSource::Commit(h) => format!("Δ {name} @{}", &h[..h.len().min(7)]),
        }
    }

    fn layout(&mut self) {
        self.rows = crate::changes_ui::diff_layout(&self.hunks, self.side);
    }
}

impl GitState {
    /// A field of its own is open (the new branch's name).
    pub fn has_field(&self) -> bool {
        self.new_branch.is_some()
    }

    pub fn new(root: &Path, settings: &Settings, cx: &mut Context<ProjectView>) -> Option<Self> {
        Some(Self::with(GitSession::open(root)?, settings, cx))
    }

    /// The panel for a repository: this machine's, or another workspace's
    /// (its session filled from calls).
    pub fn with(session: GitSession, settings: &Settings, cx: &mut Context<ProjectView>) -> Self {
        let options = TextOptions { wrap: true, auto_height: Some((3, 10)), placeholder: Some("Commit message".into()), ..Default::default() };
        let message = cx.new(|cx| TextView::new("", options, settings, cx));
        let sub = cx.subscribe(&message, |this, _, event: &TextEvent, cx| {
            if let TextEvent::Changed = event {
                cx.notify();
            }
            // Ctrl+S in the message commits, as a shortcut.
            if let TextEvent::Save = event {
                this.commit(false, cx);
            }
        });
        Self {
            session,
            busy: None,
            error: None,
            refresh: None,
            op: None,
            _fetcher: None,
            message,
            log_task: None,
            expanded: HashSet::new(),
            history_scroll: UniformListScrollHandle::new(),
            new_branch: None,
            collapsed: HashSet::new(),
            _message_sub: sub,
        }
    }
}

impl ProjectView {
    /// Starts fetching every few minutes (as configured), so "behind" is true.
    pub(super) fn start_fetching(&mut self, cx: &mut Context<Self>) {
        let minutes = self.settings.config.git.fetch_minutes;
        let Some(git) = &mut self.git else { return };
        if minutes == 0 {
            git._fetcher = None;
            return;
        }
        let repo = git.session.repo.clone();
        let remote = self.remote.as_ref().map(|r| (r.ws.clone(), r.tab.clone()));
        git._fetcher = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(minutes * 60)).await;
                // Quietly: a failed background fetch (offline) says nothing.
                let fetched = match &remote {
                    Some((ws, tab)) => ws.call(Call::GitFetch(api::TabOnly { tab: tab.clone() })).await.map(|_| String::new()).map_err(|e| shed_git::GitError(says(&e))),
                    None => {
                        let repo = repo.clone();
                        cx.background_executor().spawn(async move { repo.fetch() }).await
                    }
                };
                if this.update(cx, |this, cx| if fetched.is_ok() { this.git_refresh(cx) }).is_err() {
                    break;
                }
            }
        }));
    }

    /// Reads the status again, a moment after changes settle.
    pub(super) fn git_refresh(&mut self, cx: &mut Context<Self>) {
        if self.remote.is_some() {
            return self.remote_git_refresh(cx);
        }
        let Some(git) = &mut self.git else { return };
        let repo = git.session.repo.clone();
        git.refresh = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(150)).await;
            let status = cx.background_executor().spawn(async move { gs::refresh(&repo) }).await;
            this.update(cx, |this, cx| {
                let mut head_moved = false;
                if let Some(git) = &mut this.git {
                    match status {
                        Ok(refreshed) => head_moved = git.session.apply_refresh(refreshed),
                        Err(e) => git.error = Some(e.0),
                    }
                }
                // New commits (here or outside Shed) show in an open history.
                if head_moved && this.panel == Panel::History {
                    let path = this.git.as_ref().and_then(|g| g.session.log_for.clone());
                    this.open_history(path, cx);
                }
                this.refresh_diffs(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Reads another workspace's Git status (and its branches) again, a
    /// moment after changes settle: the panel appears with its repository.
    fn remote_git_refresh(&mut self, cx: &mut Context<Self>) {
        let Some((ws, tab)) = self.remote_call() else { return };
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(150)).await;
            let status = ws.call(Call::GitStatus(api::TabOnly { tab: tab.clone() })).await;
            let branches = match &status {
                Ok(Reply::GitStatus(s)) if s.repo => Some(ws.call(Call::GitBranches(api::TabOnly { tab })).await),
                _ => None,
            };
            this.update(cx, |this, cx| {
                let mut head_moved = false;
                match status {
                    Ok(Reply::GitStatus(s)) => match remote::refreshed(&s, &this.root) {
                        Some((root, refreshed)) => {
                            if this.git.as_ref().is_none_or(|g| g.session.repo.root != root) {
                                let settings = this.settings.clone();
                                this.git = Some(GitState::with(GitSession::new(Repo { root }), &settings, cx));
                                this.start_fetching(cx);
                            }
                            if let Some(git) = &mut this.git {
                                head_moved = git.session.apply_refresh(refreshed);
                                git.error = None;
                            }
                        }
                        None => this.git = None,
                    },
                    Ok(other) => {
                        if let Some(git) = &mut this.git {
                            git.error = Some(says(&shed_client::nexus_api::unexpected(&other)));
                        }
                    }
                    Err(e) => {
                        if let Some(git) = &mut this.git {
                            git.error = Some(says(&e));
                        }
                    }
                }
                if let (Some(r), Some(Ok(Reply::Branches(b)))) = (&mut this.remote, branches) {
                    r.branches = Some(b);
                }
                if head_moved && this.panel == Panel::History {
                    let path = this.git.as_ref().and_then(|g| g.session.log_for.clone());
                    this.open_history(path, cx);
                }
                this.refresh_diffs(cx);
                cx.notify();
            })
            .ok();
        });
        if let Some(r) = &mut self.remote {
            r.git_task = Some(task);
        }
    }

    /// Runs a Git operation away from the UI (here, or on another
    /// workspace), showing it runs; then reads the status again.
    pub(super) fn git_op(&mut self, label: &str, op: GitOp, cx: &mut Context<Self>) {
        self.git_op_then(label, op, |_, _| {}, cx);
    }

    /// `git_op`, then `then` if it worked.
    fn git_op_then(&mut self, label: &str, op: GitOp, then: impl FnOnce(&mut Self, &mut Context<Self>) + 'static, cx: &mut Context<Self>) {
        let remote = self.remote_call();
        let Some(git) = &mut self.git else { return };
        if git.busy.is_some() {
            return;
        }
        let reply = match &remote {
            None => None,
            Some((ws, tab)) => match op.call(tab) {
                Some(call) => Some(ws.call(call)),
                None => {
                    git.error = Some(NOT_REMOTE.into());
                    cx.notify();
                    return;
                }
            },
        };
        git.busy = Some(label.to_string());
        git.error = None;
        let repo = git.session.repo.clone();
        git.op = Some(cx.spawn(async move |this, cx| {
            let result = match reply {
                Some(reply) => reply.await.map(|r| if let Reply::Text(t) = r { t } else { String::new() }).map_err(|e| says(&e)),
                None => cx.background_executor().spawn(async move { op.run(&repo) }).await.map_err(|e| e.0),
            };
            this.update(cx, |this, cx| {
                let ok = result.is_ok();
                if let Some(git) = &mut this.git {
                    git.busy = None;
                    git.error = result.err();
                    git.session.log.clear();
                }
                this.git_refresh(cx);
                if ok {
                    then(this, cx);
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn entry_paths(entries: &[&GitEntry]) -> Vec<String> {
        entries.iter().map(|e| e.path.clone()).collect()
    }

    fn stage_all(&mut self, cx: &mut Context<Self>) {
        let Some(s) = self.git.as_ref().and_then(|g| g.session.status.clone()) else { return };
        let paths = Self::entry_paths(&s.unstaged().collect::<Vec<_>>());
        self.git_op("Staging…", GitOp::Stage(paths), cx);
    }

    fn unstage_all(&mut self, cx: &mut Context<Self>) {
        let Some(s) = self.git.as_ref().and_then(|g| g.session.status.clone()) else { return };
        let paths = Self::entry_paths(&s.staged().collect::<Vec<_>>());
        self.git_op("Unstaging…", GitOp::Unstage(paths), cx);
    }

    fn discard(&mut self, entries: Vec<GitEntry>, window: &mut Window, cx: &mut Context<Self>) {
        if entries.is_empty() {
            return;
        }
        let names: Vec<String> = entries.iter().map(|e| e.path.clone()).collect();
        let what = if names.len() == 1 { format!("“{}”", names[0]) } else { format!("{} files", names.len()) };
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Discard the changes to {what}?"),
            Some("They are lost: tracked files go back to what is staged, new files are deleted."),
            &["Discard", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if !matches!(answer.await, Ok(0)) {
                return;
            }
            this.update(cx, |this, cx| {
                let tracked: Vec<String> = entries.iter().filter(|e| !e.untracked).map(|e| e.path.clone()).collect();
                let untracked: Vec<String> = entries.iter().filter(|e| e.untracked).map(|e| e.path.clone()).collect();
                this.git_op("Discarding…", GitOp::Discard { tracked, untracked }, cx);
            })
            .ok();
        })
        .detach();
    }

    fn commit(&mut self, push: bool, cx: &mut Context<Self>) {
        let Some(git) = &self.git else { return };
        let message = git.message.read(cx).buffer.text();
        if message.trim().is_empty() {
            self.git.as_mut().unwrap().error = Some(gs::NO_MESSAGE.into());
            cx.notify();
            return;
        }
        let amend = git.session.amend;
        let field = git.message.clone();
        // With Push, what would go is shown first, as for any push.
        self.git_op_then("Committing…", GitOp::Commit { message, amend }, move |this, cx| if push { this.push(false, cx) }, cx);
        // Cleared now; the commit's own result shows any error.
        field.update(cx, |f, cx| {
            f.buffer.select_all();
            f.buffer.insert("");
            cx.notify();
        });
        if let Some(git) = &mut self.git {
            git.session.amend = false;
        }
    }

    fn toggle_amend(&mut self, cx: &mut Context<Self>) {
        let Some(git) = &mut self.git else { return };
        git.session.amend = !git.session.amend;
        // (Another workspace's last message is not among its calls.)
        if git.session.amend && git.message.read(cx).buffer.len_chars() == 0 && self.remote.is_none() {
            let last = git.session.repo.last_message();
            git.message.update(cx, |m, cx| m.paste_text(&last, cx));
        }
        cx.notify();
    }

    /// Shows what a push would send and where, to confirm (`confirm_push`).
    fn push(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(git) = &mut self.git else { return };
        self.panel = Panel::Changes;
        if self.remote.is_some() {
            // Another workspace pushes to its branch's upstream (else origin):
            // what goes is what its status says is not pushed.
            let Some(status) = &git.session.status else { return };
            let branch = status.branch.clone();
            let head = branch.head.clone().unwrap_or_default();
            let (remote, remote_branch) = match branch.upstream.as_deref().and_then(|u| u.split_once('/')) {
                Some((r, b)) => (r.to_string(), b.to_string()),
                None => ("origin".to_string(), head.clone()),
            };
            let target = shed_git::PushTarget { branch: head, remote, remote_branch, url: String::new(), new: branch.upstream.is_none() };
            git.session.push_review = Some(PushReview { force, branch, target, commits: git.session.outgoing.clone() });
            cx.notify();
            return;
        }
        let repo = git.session.repo.clone();
        cx.spawn(async move |this, cx| {
            // The status as it is now (a commit may just have been made).
            let read = cx.background_executor().spawn(async move { PushReview::read(&repo, force) }).await;
            this.update(cx, |this, cx| {
                if let Some(git) = &mut this.git {
                    match read {
                        Ok(review) => git.session.push_review = Some(review),
                        Err(e) => git.error = Some(e.0),
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn confirm_push(&mut self, cx: &mut Context<Self>) {
        let Some(review) = self.git.as_mut().and_then(|g| g.session.push_review.take()) else { return };
        let label = review.busy_label();
        self.git_op(label, GitOp::Push(review), cx);
    }

    fn cancel_push(&mut self, cx: &mut Context<Self>) {
        if let Some(git) = &mut self.git {
            git.session.push_review = None;
        }
        cx.notify();
    }

    /// Opens (or shows) a diff of a file.
    pub(super) fn open_diff(&mut self, path: String, source: DiffSource, cx: &mut Context<Self>) {
        let full = self.git.as_ref().map(|g| g.session.repo.root.join(&path)).unwrap_or_default();
        let existing = self.open.iter().position(|f| matches!(&f.content, Content::Diff(d) if d.path == path && d.source == source));
        let i = match existing {
            Some(i) => i,
            None => {
                let view = DiffView { path, source, hunks: Vec::new(), rows: Vec::new(), side: false, scroll: UniformListScrollHandle::new(), untracked: false, pictures: None, headers: Vec::new() };
                self.open.push(OpenFile {
                    path: full,
                    content: Content::Diff(Box::new(view)),
                    lang: None,
                    disk: super::Disk::Same,
                    restored: false,
                    highlight: None,
                    recovery: None,
                    _sub: None,
                });
                self.open.len() - 1
            }
        };
        self.active = Some(i);
        self.load_diff(i, cx);
        cx.notify();
    }

    /// Reads the two versions of an open diff again, off the UI thread.
    fn load_diff(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(git) = &self.git else { return };
        let Some(Content::Diff(d)) = self.open.get(i).map(|f| &f.content) else { return };
        let (repo, path, source) = (git.session.repo.clone(), d.path.clone(), d.source.clone());
        let untracked = git.session.is_untracked(&path);
        if let Some((ws, tab)) = self.remote_call() {
            let reply = ws.call(Call::GitDiff(api::GitDiff { tab, path: path.clone(), source: remote::source_of(&source) }));
            cx.spawn(async move |this, cx| {
                let reply = reply.await;
                this.update(cx, |this, cx| {
                    let diff = match reply {
                        Ok(Reply::Diff(d)) => d,
                        Ok(other) => return this.git_error(says(&shed_client::nexus_api::unexpected(&other)), cx),
                        Err(e) => return this.git_error(says(&e), cx),
                    };
                    let Some(file) = this.open.get_mut(i) else { return };
                    if let Content::Diff(d) = &mut file.content {
                        let at = Path::new(&d.path).to_path_buf();
                        d.hunks = crate::workspace_api::hunks_of(&diff.hunks);
                        d.headers = diff.hunks.iter().map(|h| h.header.clone()).collect();
                        let picture = |p: &Option<api::Picture>| p.as_ref().and_then(|p| remote::picture_of(&at, p));
                        d.pictures = (diff.old_picture.is_some() || diff.new_picture.is_some()).then(|| (picture(&diff.old_picture), picture(&diff.new_picture)));
                        d.untracked = untracked;
                        d.layout();
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
            return;
        }
        cx.spawn(async move |this, cx| {
            let (hunks, pictures) = cx
                .background_executor()
                .spawn(async move {
                    match gs::load_diff(&repo, &path, &source, untracked, crate::picture::is_picture(Path::new(&path))) {
                        DiffContent::Hunks(hunks) => (hunks, None),
                        DiffContent::Pictures { old, new } => {
                            let at = Path::new(&path);
                            (Vec::new(), Some((crate::picture::from_bytes(at, old), crate::picture::from_bytes(at, new))))
                        }
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(file) = this.open.get_mut(i) else { return };
                if let Content::Diff(d) = &mut file.content {
                    d.hunks = hunks;
                    d.pictures = pictures;
                    d.untracked = untracked;
                    d.layout();
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn refresh_diffs(&mut self, cx: &mut Context<Self>) {
        for i in 0..self.open.len() {
            if matches!(&self.open[i].content, Content::Diff(d) if d.source != DiffSource::Commit(String::new())) {
                self.load_diff(i, cx);
            }
        }
    }

    /// Stages, unstages or throws away one hunk of an open diff.
    fn hunk_action(&mut self, i: usize, h: usize, action: HunkAction, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Content::Diff(d)) = self.open.get(i).map(|f| &f.content) else { return };
        let Some(hunk) = d.hunks.get(h) else { return };
        let header = d.headers.get(h).cloned().unwrap_or_else(|| hunk.header());
        let run = GitOp::Hunk { path: d.path.clone(), source: d.source.clone(), index: h, header, hunk: hunk.clone(), action, untracked: d.untracked };
        match action {
            _ if d.untracked => self.git_op("Staging…", run, cx),
            HunkAction::Stage => self.git_op("Staging…", run, cx),
            HunkAction::Unstage => self.git_op("Unstaging…", run, cx),
            HunkAction::Discard => {
                let answer = window.prompt(PromptLevel::Warning, "Discard this change?", Some("It is lost from the working folder."), &["Discard", "Cancel"], cx);
                cx.spawn(async move |this, cx| {
                    if matches!(answer.await, Ok(0)) {
                        this.update(cx, |this, cx| this.git_op("Discarding…", run, cx)).ok();
                    }
                })
                .detach();
            }
        }
    }

    pub(super) fn open_history(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        let Some(git) = &mut self.git else { return };
        git.session.start_history(path.clone());
        git.expanded.clear();
        let repo = git.session.repo.clone();
        let remote = self.remote.as_ref().map(|r| r.ws.call(Call::GitLog(api::GitLog { tab: r.tab.clone(), path: path.clone(), limit: 0 })));
        let Some(git) = &mut self.git else { return };
        git.log_task = Some(cx.spawn(async move |this, cx| {
            let log = match remote {
                Some(reply) => match reply.await {
                    Ok(Reply::Commits(commits)) => Ok(commits.iter().map(remote::commit_of).collect()),
                    Ok(other) => Err(shed_git::GitError(says(&shed_client::nexus_api::unexpected(&other)))),
                    Err(e) => Err(shed_git::GitError(says(&e))),
                },
                None => cx.background_executor().spawn(async move { gs::history(&repo, path.as_deref()) }).await,
            };
            this.update(cx, |this, cx| {
                if let Some(git) = &mut this.git {
                    match log {
                        Ok(l) => git.session.log = l,
                        Err(e) => git.error = Some(e.0),
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        self.panel = Panel::History;
        cx.notify();
    }

    fn toggle_commit(&mut self, hash: String, cx: &mut Context<Self>) {
        let remote = self.remote_call();
        let Some(git) = &mut self.git else { return };
        if let Some((ws, tab)) = remote
            && !git.expanded.remove(&hash)
        {
            // Another workspace's commit's files, read once.
            if git.session.commit_files.contains_key(&hash) {
                git.expanded.insert(hash);
                return cx.notify();
            }
            let reply = ws.call(Call::GitCommitFiles(api::GitCommitRef { tab, hash: hash.clone() }));
            cx.spawn(async move |this, cx| {
                let reply = reply.await;
                this.update(cx, |this, cx| {
                    let Some(git) = &mut this.git else { return };
                    let files = match reply {
                        Ok(Reply::CommitFiles(files)) => files.into_iter().map(|f| (f.status.chars().next().unwrap_or('M'), f.path)).collect(),
                        Err(e) => {
                            git.error = Some(says(&e));
                            Vec::new()
                        }
                        Ok(_) => Vec::new(),
                    };
                    git.session.commit_files.insert(hash.clone(), files);
                    git.expanded.insert(hash);
                    cx.notify();
                })
                .ok();
            })
            .detach();
            return;
        }
        let Some(git) = &mut self.git else { return };
        if self.remote.is_some() {
            return cx.notify();
        }
        if !git.expanded.remove(&hash) {
            git.session.commit_files(&hash);
            git.expanded.insert(hash);
        }
        cx.notify();
    }

    /// Runs a branch, stash or remote operation.
    pub(super) fn branch_op(&mut self, op: BranchOp, cx: &mut Context<Self>) {
        self.git_op(op.label(), GitOp::Branch(op), cx);
    }

    /// Shows what went wrong in the Git panel.
    fn git_error(&mut self, error: String, cx: &mut Context<Self>) {
        if let Some(git) = &mut self.git {
            git.error = Some(error);
        }
        cx.notify();
    }

    fn branch_menu(&mut self, _: &mut Context<Self>) -> Vec<MenuEntry<Self>> {
        let Some(git) = &self.git else { return Vec::new() };
        // Another workspace's branches as last read with its status.
        let remote = self.remote.as_ref().map(|r| r.branches.clone().unwrap_or_default());
        let others = match &remote {
            Some(b) => b.local.iter().filter(|l| Some(*l) != b.current.as_ref()).cloned().collect(),
            None => git.session.branches().others,
        };
        let (behind, ahead) = git.session.behind_ahead();
        let count = |n: u32| if n > 0 { format!(" ({n})") } else { String::new() };
        let mut entries = vec![menu::item("Fetch", |this: &mut Self, _, cx| this.branch_op(BranchOp::Fetch, cx)).icon("refresh")];
        if remote.is_some() {
            // Its calls pull as `git pull` does there.
            entries.push(menu::item(format!("Pull{}", count(behind)), |this: &mut Self, _, cx| this.git_op("Pulling…", GitOp::Pull, cx)).icon("pull"));
        } else {
            entries.push(menu::item(format!("Pull with Rebase{}", count(behind)), |this: &mut Self, _, cx| this.branch_op(BranchOp::PullRebase, cx)).icon("pull"));
            entries.push(menu::item(format!("Pull with Merge{}", count(behind)), |this: &mut Self, _, cx| this.branch_op(BranchOp::PullMerge, cx)).icon("pull"));
        }
        entries.extend([
            menu::item(format!("Push{}", count(ahead)), |this: &mut Self, _, cx| this.push(false, cx)).icon("push"),
            menu::danger("Force-Push…", |this: &mut Self, _, cx| this.push(true, cx)),
            MenuEntry::Separator,
            menu::item("New Branch…", |this: &mut Self, window, cx| this.start_new_branch(window, cx)).icon("plus"),
        ]);
        if !others.is_empty() {
            entries.push(MenuEntry::Separator);
        }
        for b in others.iter().take(12) {
            let name = b.clone();
            entries.push(menu::item(format!("Switch to {b}"), move |this: &mut Self, _, cx| this.branch_op(BranchOp::Switch(name.clone()), cx)).icon("branch"));
        }
        entries.push(menu::more("Remote Branches", |this: &mut Self, _| {
            let remote = match &this.remote {
                Some(r) => r.branches.as_ref().map(|b| b.remote.clone()).unwrap_or_default(),
                None => this.git.as_ref().map(|g| g.session.repo.remote_branches().unwrap_or_default()).unwrap_or_default(),
            };
            remote
                .into_iter()
                .take(30)
                .map(|b| {
                    let label = format!("Check out {b}");
                    menu::item(label, move |this: &mut Self, _, cx| this.branch_op(BranchOp::CheckOut(b.clone()), cx))
                })
                .collect()
        }));
        if remote.is_some() {
            // Not among another workspace's calls: said so, not hidden.
            entries.push(MenuEntry::Separator);
            for label in ["Merge into Current", "Rebase Current onto", "Delete Branch", "Stash Changes", "Pop Stash"] {
                entries.push(menu::disabled(label, "not remotely"));
            }
            entries.push(menu::item("History", |this: &mut Self, _, cx| this.open_history(None, cx)).icon("history"));
            return entries;
        }
        if !others.is_empty() {
            let (m, r, d) = (others.clone(), others.clone(), others.clone());
            entries.push(MenuEntry::Separator);
            entries.push(menu::more("Merge into Current", move |_, _| {
                m.iter().map(|b| {
                    let n = b.clone();
                    menu::item(b.clone(), move |this: &mut Self, _, cx| this.branch_op(BranchOp::Merge(n.clone()), cx))
                }).collect()
            }));
            entries.push(menu::more("Rebase Current onto", move |_, _| {
                r.iter().map(|b| {
                    let n = b.clone();
                    menu::item(b.clone(), move |this: &mut Self, _, cx| this.branch_op(BranchOp::Rebase(n.clone()), cx))
                }).collect()
            }));
            entries.push(menu::more("Delete Branch", move |_, _| {
                d.iter().map(|b| {
                    let n = b.clone();
                    menu::danger(format!("Delete {b}…"), move |this: &mut Self, window, cx| this.delete_branch(n.clone(), window, cx))
                }).collect()
            }));
        }
        entries.push(MenuEntry::Separator);
        entries.push(menu::item("Stash Changes", |this: &mut Self, _, cx| this.branch_op(BranchOp::Stash, cx)));
        entries.push(menu::item("Pop Stash", |this: &mut Self, _, cx| this.branch_op(BranchOp::StashPop, cx)));
        entries.push(menu::item("History", |this: &mut Self, _, cx| this.open_history(None, cx)).icon("history"));
        entries
    }

    fn delete_branch(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Delete the branch “{name}”?"),
            Some("Only if it is merged; otherwise Git refuses, and you can delete it anyway."),
            &["Delete Branch", "Delete Even If Not Merged", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            let force = match answer.await {
                Ok(0) => false,
                Ok(1) => true,
                _ => return,
            };
            this.update(cx, |this, cx| this.branch_op(BranchOp::Delete { name, force }, cx)).ok();
        })
        .detach();
    }

    fn start_new_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(git) = &mut self.git else { return };
        let editor = cx.new(|cx| LineEditor::new(String::new(), cx).submit_on_blur(window, cx));
        window.focus(&editor.read(cx).focus_handle(cx), cx);
        let sub = cx.subscribe(&editor, |this, _, event: &EditorEvent, cx| {
            if let Some(git) = &mut this.git {
                git.new_branch = None;
            }
            if let EditorEvent::Submit(name) = event
                && !name.is_empty()
            {
                this.branch_op(BranchOp::Create(name.clone()), cx);
            }
            cx.notify();
        });
        git.new_branch = Some((editor, sub));
        self.panel = Panel::Changes;
        cx.notify();
    }

    /// The branch and how it stands, in a few words: for Shed's cards.
    pub fn branch_label(&self) -> Option<String> {
        self.git.as_ref()?.session.branch_label()
    }

    /// The branch, how it stands, and what changed: for the header.
    pub(super) fn render_branch_chip(&self, ui: &UiColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let git = self.git.as_ref()?;
        let s = git.session.status.as_ref()?;
        let head = s.branch.head.clone().unwrap_or_else(|| format!("detached at {}", s.branch.oid));
        let mut label = head;
        if s.branch.ahead > 0 {
            label.push_str(&format!(" ↑{}", s.branch.ahead));
        }
        if s.branch.behind > 0 {
            label.push_str(&format!(" ↓{}", s.branch.behind));
        }
        let changed = s.entries.len();
        let accent = self.accent;
        let color = if s.conflicts().next().is_some() || s.operation.is_some() { ui.danger } else if s.branch.behind > 0 { ui.ink(accent) } else { ui.text_dim };
        Some(
            div()
                .id("branch-chip")
                .flex()
                .flex_none()
                .items_center()
                .gap_1()
                .px_2()
                .py_0p5()
                .rounded_md()
                .border_1()
                .border_color(hsla(ui.divider))
                .cursor_pointer()
                .text_size(px(12.0 * self.z()))
                .text_color(hsla(color))
                .hover(|st| st.bg(hsla(ui.tab_hover)))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                    let entries = this.branch_menu(cx);
                    this.open_menu(e.position, entries, cx);
                }))
                .child(svg().path(icon("branch")).size(px(13.0)).text_color(hsla(color)))
                .child(label)
                .when(changed > 0, |d| d.child(div().text_color(hsla(ui.text_dim)).child(format!("· {changed} changed"))))
                .when_some(s.operation.clone(), |d, op| d.child(div().text_color(hsla(ui.danger)).child(format!("· {op}"))))
                .into_any_element(),
        )
    }

    /// The Changes panel: sync buttons, files by state, the commit box.
    pub(super) fn render_changes(&self, ui: &UiColors, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(git) = &self.git else {
            return div().p_3().text_color(hsla(ui.text_dim)).child("Not a Git repository.").into_any_element();
        };
        let accent = self.accent;
        let Some(s) = git.session.status.clone() else { return div().p_3().text_color(hsla(ui.text_dim)).child("Reading…").into_any_element() };
        let button = |id: &'static str, glyph: &'static str, label: String| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1()
                .px_1p5()
                .py_0p5()
                .rounded_sm()
                .border_1()
                .border_color(hsla(ui.divider))
                .cursor_pointer()
                .text_color(hsla(ui.text_dim))
                .hover(|st| st.bg(hsla(ui.tab_hover)).text_color(hsla(ui.text)))
                .child(svg().path(icon(glyph)).size(px(12.0)).text_color(hsla(ui.text_dim)))
                .child(label)
        };
        let sync = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .child(button("git-fetch", "refresh", "Fetch".into()).on_click(cx.listener(|this, _, _, cx| this.git_op("Fetching…", GitOp::Branch(BranchOp::Fetch), cx))))
            .child(button("git-pull", "pull", if s.branch.behind > 0 { format!("Pull {}", s.branch.behind) } else { "Pull".into() }).on_click(cx.listener(|this, _, _, cx| this.git_op("Pulling…", GitOp::Pull, cx))))
            .child(
                button("git-push", "push", {
                    let ahead = s.branch.ahead.max(git.session.outgoing.len() as u32);
                    if ahead > 0 { format!("Push {ahead}") } else { "Push".into() }
                })
                    .on_click(cx.listener(|this, _, _, cx| this.push(false, cx)))
                    .on_mouse_down(MouseButton::Right, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                        let entries = vec![menu::danger("Force-Push…", |this: &mut Self, _, cx| this.push(true, cx))];
                        this.open_menu(e.position, entries, cx);
                    })),
            )
            .child(div().flex_1())
            // A mode: files listed flat, or by folder.
            .child({
                let tree = self.settings.config.git.tree;
                div()
                    .id("git-tree-toggle")
                    .flex_none()
                    .p_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|st| st.bg(hsla(ui.tab_hover)))
                    .tooltip(menu::tooltip(if tree { "Show the files in a flat list" } else { "Show the files by folder" }))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_tree(!tree, cx)))
                    .child(svg().path(icon(if tree { "view-list" } else { "folder" })).size(px(13.0)).text_color(hsla(ui.text_dim)))
            });
        let mut list = div().id("git-files").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.bars.git_files_scroll).flex().flex_col();
        if let Some(op) = &s.operation {
            let op2 = op.clone();
            let op3 = op.clone();
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .mb_1()
                    .rounded_sm()
                    .bg(hsla_alpha(ui.danger, 0.15))
                    .text_color(hsla(ui.text))
                    .child(format!("A {op} is under way. Resolve the conflicts, stage them, then continue."))
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(button("op-continue", "go", "Continue".into()).on_click(cx.listener(move |this, _, _, cx| {
                                let op = op2.clone();
                                this.git_op("Continuing…", GitOp::Finish { operation: op.clone(), how: "continue" }, cx)
                            })))
                            .child(button("op-abort", "close", "Abort".into()).on_click(cx.listener(move |this, _, _, cx| {
                                let op = op3.clone();
                                this.git_op("Aborting…", GitOp::Finish { operation: op.clone(), how: "abort" }, cx)
                            }))),
                    ),
            );
        }
        let conflicts: Vec<GitEntry> = s.conflicts().cloned().collect();
        let staged: Vec<GitEntry> = s.staged().cloned().collect();
        let unstaged: Vec<GitEntry> = s.unstaged().cloned().collect();
        if !conflicts.is_empty() {
            list = list.child(self.section_header("Conflicts", conflicts.len(), None, ui, cx));
            list = list.children(self.section_rows(10_000, &conflicts, Section::Conflict, ui, cx));
        }
        if !staged.is_empty() {
            list = list.child(self.section_header("Staged", staged.len(), Some(("Unstage All", Section::Staged)), ui, cx));
            list = list.children(self.section_rows(20_000, &staged, Section::Staged, ui, cx));
        }
        if !unstaged.is_empty() {
            list = list.child(self.section_header("Changes", unstaged.len(), Some(("Stage All", Section::Unstaged)), ui, cx));
            list = list.children(self.section_rows(30_000, &unstaged, Section::Unstaged, ui, cx));
        }
        if s.entries.is_empty() {
            list = list.child(div().p_2().text_color(hsla(ui.text_dim)).child("Nothing changed."));
        }
        // Commits made here and not pushed yet, each opening to its files.
        if !git.session.outgoing.is_empty() {
            list = list.child(self.section_header("Not Pushed", git.session.outgoing.len(), None, ui, cx));
            for (n, c) in git.session.outgoing.iter().enumerate() {
                list = list.children(self.outgoing_rows(n, c, ui, cx));
            }
        }
        let review = self.render_push_review(ui, cx);
        let can_commit = !staged.is_empty() || git.session.amend;
        let commit_button = |id: &'static str, label: String, push: bool| {
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded_md()
                .border_1()
                .border_color(hsla(if can_commit { accent } else { ui.divider }))
                .text_color(hsla(if can_commit { ui.ink(accent) } else { ui.text_dim }))
                .cursor_pointer()
                .hover(|st| st.bg(hsla_alpha(accent, 0.2)))
                .on_click(cx.listener(move |this, _, _, cx| this.commit(push, cx)))
                .child(label)
        };
        let _ = window;
        div()
            .size_full()
            // Never past the panel, over the editor.
            .overflow_hidden()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .text_size(px(12.0 * self.z()))
            .child(sync)
            .children(review)
            .when_some(git.new_branch.as_ref().map(|(e, _)| e.clone()), |d, e| {
                d.child(div().flex().items_center().gap_1().child(div().text_color(hsla(ui.text_dim)).child("New branch:")).child(div().flex_1().child(e)))
            })
            .when_some(git.busy.clone(), |d, b| d.child(div().text_color(hsla(ui.ink(accent))).child(b)))
            .when_some(git.error.clone(), |d, e| d.child(div().p_1().rounded_sm().bg(hsla_alpha(ui.danger, 0.15)).text_color(hsla(ui.text)).child(e)))
            .child(crate::scrollbar::with_bar(list, crate::scrollbar::Source::Handle(self.bars.git_files_scroll.clone()), &self.bars.git_files, ui).flex_1())
            .child(div().rounded_md().border_1().border_color(hsla(ui.divider)).bg(hsla(ui.tab_active)).child(git.message.clone()))
            // In a narrow panel the buttons go onto a row of their own.
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("amend")
                            .flex()
                            .items_center()
                            .gap_1()
                            .cursor_pointer()
                            .text_color(hsla(if git.session.amend { ui.text } else { ui.text_dim }))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_amend(cx)))
                            .child(if git.session.amend { "☑" } else { "☐" })
                            .child("Amend"),
                    )
                    .child(div().flex_1())
                    .child(commit_button("commit-push", "Commit and Push…".into(), true))
                    .child(commit_button("commit", match staged.len() {
                        0 => "Commit".into(),
                        1 => "Commit 1 File".into(),
                        n => format!("Commit {n} Files"),
                    }, false)),
            )
            .into_any_element()
    }

    /// An outgoing commit's row, and its files when opened.
    fn outgoing_rows(&self, n: usize, c: &Commit, ui: &UiColors, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(git) = &self.git else { return Vec::new() };
        let open = git.expanded.contains(&c.hash);
        let hash = c.hash.clone();
        let row = |id: usize| div().id(("outgoing", id)).h(px(ROW_HEIGHT * self.z())).flex().items_center().gap_1p5().px_1().rounded_sm().cursor_pointer().hover(|s| s.bg(hsla(ui.tab_hover)));
        let mut rows = vec![
            row(n * 1000)
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_commit(hash.clone(), cx)))
                .child(div().flex_none().w(px(10.0)).text_color(hsla(ui.text_dim)).child(if open { "▾" } else { "▸" }))
                .child(div().flex_none().font_family(self.settings.font.family.clone()).text_color(hsla(ui.text_dim)).child(c.short.clone()))
                .child(div().flex_1().min_w_0().truncate().text_color(hsla(ui.text)).child(c.subject.clone()))
                .child(div().flex_none().text_color(hsla(ui.text_dim)).child(short_ago(c.time)))
                .into_any_element(),
        ];
        if open {
            for (f, (code, path)) in git.session.commit_files.get(&c.hash).into_iter().flatten().enumerate() {
                let (p, hash) = (path.clone(), c.hash.clone());
                rows.push(
                    row(n * 1000 + f + 1)
                        .pl(px(22.0))
                        .on_click(cx.listener(move |this, _, _, cx| this.open_diff(p.clone(), DiffSource::Commit(hash.clone()), cx)))
                        .child(div().flex_none().w(px(12.0)).text_color(hsla(ui.text_dim)).child(code.to_string()))
                        .child(div().flex_1().min_w_0().truncate().text_color(hsla(ui.text)).child(path.clone()))
                        .into_any_element(),
                );
            }
        }
        rows
    }

    /// A push to confirm: the branch, where it goes, and the commits it sends.
    fn render_push_review(&self, ui: &UiColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let review = self.git.as_ref()?.session.push_review.as_ref()?;
        let t = &review.target;
        let tone = if review.force { ui.danger } else { self.accent };
        let n = review.commits.len();
        let mono = self.settings.font.family.clone();
        const SHOWN: usize = 12;
        let commits = review.commits.iter().take(SHOWN).map(|c| {
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(div().flex_none().font_family(mono.clone()).text_color(hsla(ui.text_dim)).child(c.short.clone()))
                .child(div().flex_1().min_w_0().truncate().text_color(hsla(ui.text)).child(c.subject.clone()))
                .child(div().flex_none().text_color(hsla(ui.text_dim)).child(format!("{} · {}", c.author.split_whitespace().next().unwrap_or(""), short_ago(c.time))))
        });
        let action = |id: &'static str, label: String, color: Rgb, primary: bool| {
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded_md()
                .border_1()
                .border_color(hsla(if primary { color } else { ui.divider }))
                .text_color(hsla(if primary { ui.ink(color) } else { ui.text }))
                .cursor_pointer()
                .hover(move |s| s.bg(hsla_alpha(color, 0.2)))
                .child(label)
        };
        let what = review.action_label();
        Some(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(hsla(tone))
                .bg(hsla_alpha(tone, 0.08))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_1()
                        .text_color(hsla(ui.text))
                        .child(if review.force { "Force-push" } else { "Push" })
                        .child(div().px_1().rounded_sm().bg(hsla_alpha(self.accent, 0.2)).text_color(hsla(ui.ink(self.accent))).child(t.branch.clone()))
                        .child("to")
                        .child(div().px_1().rounded_sm().bg(hsla(ui.tab_hover)).child(format!("{}/{}", t.remote, t.remote_branch)))
                        .when(t.new, |d| d.child(div().text_color(hsla(ui.text_dim)).child("(a new branch there)"))),
                )
                .when(!t.url.is_empty(), |d| d.child(div().truncate().text_color(hsla(ui.text_dim)).child(t.url.clone())))
                .when(review.force, |d| {
                    d.child(div().text_color(hsla(ui.danger)).child(format!(
                        "This replaces {}/{} with your branch, unless someone pushed to it since you last fetched (--force-with-lease).",
                        t.remote, t.remote_branch
                    )))
                })
                .child(div().mt_1().text_color(hsla(ui.text_dim)).child(match n {
                    0 => format!("Nothing new to send: {}/{} has every commit.", t.remote, t.remote_branch),
                    1 => "1 commit goes:".to_string(),
                    n if n >= OUTGOING_LIMIT => format!("{n} or more commits go:"),
                    n => format!("{n} commits go:"),
                }))
                .children(commits)
                .when(n > SHOWN, |d| d.child(div().text_color(hsla(ui.text_dim)).child(format!("and {} more", n - SHOWN))))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .mt_1()
                        .child(action("push-confirm", what, tone, true).on_click(cx.listener(|this, _, _, cx| this.confirm_push(cx))))
                        .child(action("push-cancel", "Cancel".into(), ui.text_dim, false).on_click(cx.listener(|this, _, _, cx| this.cancel_push(cx)))),
                )
                .into_any_element(),
        )
    }

    fn section_header(&self, title: &str, count: usize, action: Option<(&'static str, Section)>, ui: &UiColors, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .mt_1()
            .px_1()
            .text_color(hsla(ui.text_dim))
            .child(div().flex_1().child(format!("{title} ({count})")))
            .when_some(action, |d, (label, section)| {
                d.child(
                    div()
                        .id(label)
                        .cursor_pointer()
                        .text_color(hsla(ui.ink(self.accent)))
                        .on_click(cx.listener(move |this, _, _, cx| match section {
                            Section::Staged => this.unstage_all(cx),
                            _ => this.stage_all(cx),
                        }))
                        .child(label),
                )
            })
            .into_any_element()
    }

    /// A section's files: a flat list, or by folder (`[git] tree`).
    /// At most `MAX_ROWS` rows (files and folders): a change to thousands
    /// of files lists the first ones and says how many more there are.
    fn section_rows(&self, base: usize, entries: &[GitEntry], section: Section, ui: &UiColors, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let (mut out, all): (Vec<AnyElement>, usize) = if !self.settings.config.git.tree {
            let rows = entries.iter().take(MAX_ROWS).enumerate().map(|(n, e)| self.file_row(base + n, e, section, None, ui, cx)).collect();
            (rows, entries.len())
        } else {
            let collapsed = self.git.as_ref().map(|g| &g.collapsed);
            // An untracked folder shown whole (`dir/`) sits in the tree as one row.
            let paths: Vec<&str> = entries.iter().map(|e| e.path.trim_end_matches('/')).collect();
            let rows = folder_rows(&paths, |dir| collapsed.is_some_and(|c| c.contains(&(section, dir.to_string()))));
            let all = rows.len();
            let rows = rows
                .into_iter()
                .take(MAX_ROWS)
                .enumerate()
                .map(|(k, row)| match row {
                    FolderRow::File { index, depth } => self.file_row(base + k, &entries[index], section, Some(depth), ui, cx),
                    FolderRow::Folder { path, name, depth, files, open } => {
                        let under: Vec<String> = files.iter().map(|&i| entries[i].path.clone()).collect();
                        self.folder_row(base + k, path, name, depth, open, under, section, ui, cx)
                    }
                })
                .collect();
            (rows, all)
        };
        if all > MAX_ROWS {
            out.push(
                div()
                    .h(px(ROW_HEIGHT * self.z()))
                    .px_1()
                    .flex()
                    .items_center()
                    .text_color(hsla(ui.text_dim))
                    .child(format!("{} changed files: the first rows only are listed", entries.len()))
                    .into_any_element(),
            );
        }
        out
    }

    /// A folder in the tree of changed files: opens and closes; its box adds
    /// or leaves out every file under it.
    #[allow(clippy::too_many_arguments)]
    fn folder_row(&self, id: usize, path: String, name: String, depth: usize, open: bool, under: Vec<String>, section: Section, ui: &UiColors, cx: &mut Context<Self>) -> AnyElement {
        let staged = section == Section::Staged;
        let count = under.len();
        div()
            .id(("git-folder", id))
            .h(px(ROW_HEIGHT * self.z()))
            .flex()
            .items_center()
            .gap_1p5()
            .pl(px(4.0 + depth as f32 * 14.0))
            .pr_1()
            .rounded_sm()
            .cursor_pointer()
            .hover(|s| s.bg(hsla(ui.tab_hover)))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(git) = &mut this.git {
                    let key = (section, path.clone());
                    if !git.collapsed.remove(&key) {
                        git.collapsed.insert(key);
                    }
                }
                cx.notify();
            }))
            .when(section != Section::Conflict, |d| {
                d.child(
                    div()
                        .id(("folder-include", id))
                        .flex_none()
                        .w(px(14.0))
                        .cursor_pointer()
                        .text_color(hsla(if staged { ui.ink(self.accent) } else { ui.text_dim }))
                        .tooltip(menu::tooltip(if staged { "Leave the folder's files out of the commit" } else { "Add the folder's files to the commit" }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            let paths = under.clone();
                            if staged {
                                this.git_op("Unstaging…", GitOp::Unstage(paths.clone()), cx);
                            } else {
                                this.git_op("Staging…", GitOp::Stage(paths.clone()), cx);
                            }
                        }))
                        .child(if staged { "☑" } else { "☐" }),
                )
            })
            .child(div().flex_none().w(px(10.0)).text_color(hsla(ui.text_dim)).child(if open { "▾" } else { "▸" }))
            .child(svg().path(icon("folder")).size(px(12.0)).flex_none().text_color(hsla(ui.text_dim)))
            .child(div().flex_1().min_w_0().truncate().text_color(hsla(ui.text)).child(name))
            .child(div().flex_none().text_color(hsla(ui.text_dim)).child(count.to_string()))
            .into_any_element()
    }

    fn set_tree(&mut self, tree: bool, cx: &mut Context<Self>) {
        self.settings.config.git.tree = tree;
        if let Some(state) = cx.try_global::<crate::state::AppHandle>().and_then(|h| h.0.upgrade()) {
            state.update(cx, |state, cx| state.set_config_value(Some("git"), "tree", tree.into(), cx));
        }
        cx.notify();
    }

    fn file_row(&self, id: usize, e: &GitEntry, section: Section, depth: Option<usize>, ui: &UiColors, cx: &mut Context<Self>) -> AnyElement {
        let code = match section {
            Section::Conflict => 'U',
            Section::Staged => e.index,
            Section::Unstaged if e.untracked => 'A',
            Section::Unstaged => e.worktree,
        };
        let color = status_color(code, ui);
        let name = Path::new(&e.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        // A new folder too full to list: one row, staged or discarded whole.
        let whole = e.files > 0;
        let name = if whole { format!("{name}/") } else { name };
        let dir = if whole {
            format!("{} new files", e.files)
        } else {
            Path::new(&e.path).parent().map(|p| p.display().to_string()).unwrap_or_default()
        };
        let (path, entry, entry2) = (e.path.clone(), e.clone(), e.clone());
        let action = |aid: (&'static str, usize), glyph: &'static str, tip: &'static str| {
            div()
                .id(aid)
                .flex_none()
                .p_0p5()
                .rounded_sm()
                .invisible()
                .group_hover("git-row", |s| s.visible())
                .cursor_pointer()
                .hover(|s| s.bg(hsla(ui.divider)))
                .tooltip(menu::tooltip(tip))
                .child(svg().path(icon(glyph)).size(px(12.0)).text_color(hsla(ui.text_dim)))
        };
        let mut row = div()
            .id(("git-row", id))
            .group("git-row")
            .h(px(ROW_HEIGHT * self.z()))
            .flex()
            .items_center()
            .gap_1p5()
            .px_1()
            .rounded_sm()
            .cursor_pointer()
            .hover(|s| s.bg(hsla(ui.tab_hover)))
            // In the tree, under its folder (past the folder's box and arrow).
            .when_some(depth, |d, depth| d.pl(px(4.0 + depth as f32 * 14.0 + 12.0)))
            .when(whole, |d| d.tooltip(menu::tooltip("A new folder with too many files to list: add it to the commit or discard it whole")))
            .on_click(cx.listener(move |this, _, _, cx| match section {
                _ if whole => {}
                Section::Conflict => {
                    let full = this.git.as_ref().map(|g| g.session.repo.root.join(&path)).unwrap_or_default();
                    this.open_path(full, None, cx);
                }
                Section::Staged => this.open_diff(path.clone(), DiffSource::Staged, cx),
                Section::Unstaged => this.open_diff(path.clone(), DiffSource::Unstaged, cx),
            }))
            // In the commit or not: ticking stages the whole file.
            .when(section != Section::Conflict, |d| {
                let staged = section == Section::Staged;
                let p = e.path.clone();
                d.child(
                    div()
                        .id(("include", id))
                        .flex_none()
                        .w(px(14.0))
                        .cursor_pointer()
                        .text_color(hsla(if staged { ui.ink(self.accent) } else { ui.text_dim }))
                        .tooltip(menu::tooltip(if staged { "In the commit: click to leave it out" } else { "Not in the commit: click to add it" }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            let paths = vec![p.clone()];
                            if staged {
                                this.git_op("Unstaging…", GitOp::Unstage(paths.clone()), cx);
                            } else {
                                this.git_op("Staging…", GitOp::Stage(paths.clone()), cx);
                            }
                        }))
                        .child(if staged { "☑" } else { "☐" }),
                )
            })
            .child(div().flex_none().w(px(12.0)).text_color(hsla(color)).child(code.to_string()))
            .child(div().flex_none().text_color(hsla(ui.text)).child(name))
            .child(div().flex_1().min_w_0().truncate().text_color(hsla(ui.text_dim)).when(depth.is_none() || whole, |d| d.child(dir)));
        // Where the file is in the project (not for one deleted).
        if let Some(full) = self.git.as_ref().map(|g| g.session.repo.root.join(&e.path)).filter(|_| code != 'D' && !whole) {
            row = row.child(action(("go-to", id), "file", "Go to File").on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.go_to_file(full.clone(), cx);
            })));
        }
        row = match section {
            Section::Unstaged => row
                .child(action(("discard", id), "discard", "Discard changes…").on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.discard(vec![entry.clone()], window, cx);
                })))
                .child(action(("stage", id), "plus", "Stage").on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    let p = vec![entry2.path.clone()];
                    this.git_op("Staging…", GitOp::Stage(p), cx);
                }))),
            Section::Staged => row.child(action(("unstage", id), "minus", "Unstage").on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                let p = vec![entry.path.clone()];
                this.git_op("Unstaging…", GitOp::Unstage(p), cx);
            }))),
            Section::Conflict => {
                let (a, b) = (entry.path.clone(), entry2.path.clone());
                row.child(action(("ours", id), "minus", "Take ours, whole").on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    let p = a.clone();
                    this.git_op("Resolving…", GitOp::Resolve { path: p, ours: true }, cx);
                })))
                .child(action(("theirs", id), "plus", "Take theirs, whole").on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    let p = b.clone();
                    this.git_op("Resolving…", GitOp::Resolve { path: p, ours: false }, cx);
                })))
            }
        };
        row.into_any_element()
    }

    /// The History panel: commits, each opening to its files.
    pub(super) fn render_history(&self, ui: &UiColors, cx: &mut Context<Self>) -> AnyElement {
        let Some(git) = &self.git else { return div().into_any_element() };
        let rows = self.history_rows();
        let count = rows.len();
        let title = match &git.session.log_for {
            Some(p) => format!("History of {p}"),
            None => "History".into(),
        };
        let list = uniform_list("history", count, cx.processor(|this, range, _, cx| this.render_history_rows(range, cx)))
            .track_scroll(&git.history_scroll)
            .flex_1()
            .min_h_0();
        let list = crate::scrollbar::with_bar(list, crate::scrollbar::Source::uniform(&git.history_scroll), &self.bars.history, ui).flex_1();
        div()
            .size_full()
            .flex()
            .flex_col()
            .p_2()
            .gap_1()
            .text_size(px(12.0 * self.z()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(div().flex_1().text_color(hsla(ui.text_dim)).child(title))
                    .when(git.session.log_for.is_some(), |d| {
                        d.child(div().id("history-all").cursor_pointer().text_color(hsla(ui.ink(self.accent))).on_click(cx.listener(|this, _, _, cx| this.open_history(None, cx))).child("All"))
                    }),
            )
            .child(list)
            .into_any_element()
    }

    fn history_rows(&self) -> Vec<(usize, Option<usize>)> {
        let Some(git) = &self.git else { return Vec::new() };
        let mut rows = Vec::new();
        for (i, c) in git.session.log.iter().enumerate() {
            rows.push((i, None));
            if git.expanded.contains(&c.hash) {
                let n = git.session.commit_files.get(&c.hash).map_or(0, Vec::len);
                rows.extend((0..n).map(|f| (i, Some(f))));
            }
        }
        rows
    }

    fn render_history_rows(&mut self, range: std::ops::Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let ui = current_ui(cx);
        let accent = self.accent;
        let rows = self.history_rows();
        let Some(git) = &self.git else { return Vec::new() };
        range
            .filter_map(|n| {
                let (i, file) = *rows.get(n)?;
                let c = git.session.log.get(i)?;
                let hash = c.hash.clone();
                let row = div().id(("hist", n)).h(px(ROW_HEIGHT * self.z())).flex().items_center().gap_1p5().px_1().rounded_sm().cursor_pointer().hover(|s| s.bg(hsla(ui.tab_hover)));
                Some(match file {
                    None => {
                        let open = git.expanded.contains(&c.hash);
                        let refs = c.refs.replace("HEAD -> ", "");
                        row.on_click(cx.listener(move |this, _, _, cx| this.toggle_commit(hash.clone(), cx)))
                            .child(div().flex_none().w(px(10.0)).text_color(hsla(ui.text_dim)).child(if open { "▾" } else { "▸" }))
                            .when(!refs.is_empty(), |d| d.child(div().flex_none().px_1().rounded_sm().bg(hsla_alpha(accent, 0.25)).text_color(hsla(ui.ink(accent))).child(refs)))
                            .child(div().flex_1().min_w_0().truncate().text_color(hsla(ui.text)).child(c.subject.clone()))
                            .child(div().flex_none().text_color(hsla(ui.text_dim)).child(format!("{} · {}", c.author.split_whitespace().next().unwrap_or(""), short_ago(c.time))))
                            .into_any_element()
                    }
                    Some(f) => {
                        let (code, path) = git.session.commit_files.get(&c.hash)?.get(f)?.clone();
                        let p = path.clone();
                        row.pl(px(22.0))
                            .on_click(cx.listener(move |this, _, _, cx| this.open_diff(p.clone(), DiffSource::Commit(hash.clone()), cx)))
                            .child(div().flex_none().w(px(12.0)).text_color(hsla(ui.text_dim)).child(code.to_string()))
                            .child(div().flex_1().min_w_0().truncate().text_color(hsla(ui.text)).child(path))
                            .into_any_element()
                    }
                })
            })
            .collect()
    }

    /// An open diff: a bar of actions, then the hunks.
    pub(super) fn render_diff(&self, i: usize, ui: &UiColors, cx: &mut Context<Self>) -> AnyElement {
        let Some(Content::Diff(d)) = self.open.get(i).map(|f| &f.content) else { return div().into_any_element() };
        let accent = self.accent;
        let bar_button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_1p5()
                .rounded_sm()
                .border_1()
                .border_color(hsla(ui.divider))
                .cursor_pointer()
                .text_color(hsla(ui.text_dim))
                .hover(|s| s.bg(hsla(ui.tab_hover)).text_color(hsla(ui.text)))
                .child(label)
        };
        let what = match &d.source {
            DiffSource::Unstaged => "Changes not staged".to_string(),
            DiffSource::Staged => "Staged changes".to_string(),
            DiffSource::Commit(h) => format!("Commit {}", &h[..h.len().min(10)]),
        };
        let path = d.path.clone();
        let full = self.git.as_ref().map(|g| g.session.repo.root.join(&path)).unwrap_or_default();
        let source = d.source.clone();
        let bar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .text_size(px(12.0 * self.z()))
            .border_b_1()
            .border_color(hsla(ui.divider))
            .child(div().flex_1().min_w_0().truncate().text_color(hsla(ui.text_dim)).child(format!("{what} · {path}")))
            .when(d.pictures.is_none(), |bar| bar.child(bar_button("diff-side", if d.side { "Unified" } else { "Side by Side" }).on_click(cx.listener(move |this, _, _, cx| {
                if let Some(Content::Diff(d)) = this.open.get_mut(i).map(|f| &mut f.content) {
                    d.side = !d.side;
                    d.layout();
                }
                cx.notify();
            }))))
            .when(full.is_file(), |bar| {
                bar.child(
                    bar_button("diff-open", "Go to File")
                        .tooltip(menu::tooltip("Open the file, and show where it is in Files"))
                        .on_click(cx.listener(move |this, _, _, cx| this.go_to_file(full.clone(), cx))),
                )
            })
            .child(bar_button("diff-history", "History").on_click(cx.listener(move |this, _, _, cx| this.open_history(Some(path.clone()), cx))));
        if let Some((old, new)) = &d.pictures {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .child(bar)
                .child(div().flex_1().min_h_0().bg(hsla(ui.background)).child(crate::picture::pair(&format!("diff-picture-{i}"), old.as_ref(), new.as_ref(), ui, 12.0 * self.z())))
                .into_any_element();
        }
        let empty = d.rows.is_empty();
        let list = uniform_list(("diff", i), d.rows.len(), cx.processor(move |this, range, _, cx| this.render_diff_rows(i, range, cx)))
            .track_scroll(&d.scroll)
            .flex_1()
            .min_h_0();
        let list = crate::scrollbar::with_bar(list, crate::scrollbar::Source::uniform(&d.scroll), &self.bars.diff, ui).flex_1();
        let _ = (source, accent);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(bar)
            .child(if empty {
                div().flex_1().flex().items_center().justify_center().text_color(hsla(ui.text_dim)).child("No differences").into_any_element()
            } else {
                list.into_any_element()
            })
            .into_any_element()
    }

    fn render_diff_rows(&mut self, i: usize, range: std::ops::Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let ui = current_ui(cx);
        let mono = self.settings.font.family.clone();
        let accent = self.accent;
        let Some(Content::Diff(d)) = self.open.get(i).map(|f| &f.content) else { return Vec::new() };
        let source = d.source.clone();
        range
            .filter_map(|n| {
                let row = d.rows.get(n)?;
                let base = div().id(("drow", n)).h(px(DIFF_ROW * self.z())).w_full().flex().items_center().font_family(mono.clone()).text_size(px(12.0 * self.z())).text_color(hsla(ui.text));
                // A hunk's own actions, as the diff's source allows.
                let actions = match row {
                    DiffRow::Hunk(h) => {
                        let h = *h;
                        let act = |id: &'static str, label: &'static str, action: HunkAction| {
                            div()
                                .id((id, h))
                                .px_1()
                                .rounded_sm()
                                .cursor_pointer()
                                .text_color(hsla(ui.ink(accent)))
                                .hover(|s| s.bg(hsla_alpha(accent, 0.2)))
                                .on_click(cx.listener(move |this, _, window, cx| this.hunk_action(i, h, action, window, cx)))
                                .child(label)
                        };
                        match source {
                            DiffSource::Unstaged => Some(div().flex().gap_1().child(act("stage-hunk", "Stage", HunkAction::Stage)).child(act("discard-hunk", "Discard…", HunkAction::Discard)).into_any_element()),
                            DiffSource::Staged => Some(act("unstage-hunk", "Unstage", HunkAction::Unstage).into_any_element()),
                            DiffSource::Commit(_) => None,
                        }
                    }
                    _ => None,
                };
                Some(crate::changes_ui::render_diff_row(base, row, &d.hunks, &ui, accent, actions))
            })
            .collect()
    }

    /// Conflict markers in the active file, as blocks of lines: start,
    /// separator, end (from 0).
    fn conflicts_in(&self, view: &Entity<TextView>, cx: &gpui::App) -> Vec<(usize, usize, usize)> {
        let b = &view.read(cx).buffer;
        gs::conflicts_in((0..b.len_lines()).map(|l| b.line(l)))
    }

    /// A bar for a file with conflict markers: take ours, theirs or both at
    /// the cursor, go to the next, ask the AI, mark it resolved.
    pub(super) fn render_conflict_bar(&self, i: usize, ui: &UiColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let file = self.open.get(i)?;
        let view = file.view()?.clone();
        let blocks = self.conflicts_in(&view, cx);
        let in_conflict = self.git.as_ref().and_then(|g| g.session.status.as_ref()).is_some_and(|s| s.conflicts().any(|e| g_path(&self.git, &e.path) == file.path));
        if blocks.is_empty() && !in_conflict {
            return None;
        }
        let accent = self.accent;
        let btn = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_1p5()
                .rounded_sm()
                .border_1()
                .border_color(hsla(ui.divider))
                .cursor_pointer()
                .hover(|s| s.bg(hsla(ui.tab_hover)))
                .child(label)
        };
        let take = move |this: &mut Self, which: u8, cx: &mut Context<Self>| {
            let blocks = this.conflicts_in(&view, cx);
            view.update(cx, |v, cx| {
                let line = v.buffer.line_of(v.buffer.selection().head);
                let Some(&(s, m, e)) = blocks.iter().find(|(s, _, e)| line >= *s && line <= *e).or_else(|| blocks.iter().find(|(s, _, _)| *s >= line)).or(blocks.first()) else { return };
                let lines: Vec<String> = (s..=e).map(|l| v.buffer.line(l)).collect();
                // A diff3 base section (|||||||) belongs to neither side.
                let ours: Vec<String> = lines[1..m - s].iter().take_while(|l| !l.starts_with("|||||||")).cloned().collect();
                let theirs: Vec<String> = lines[m - s + 1..e - s].to_vec();
                let keep = match which {
                    0 => ours,
                    1 => theirs,
                    _ => ours.into_iter().chain(theirs).collect(),
                };
                let start = v.buffer.line_start(s);
                let end = v.buffer.line_start(e + 1).max(v.buffer.line_start(e) + v.buffer.line_len(e));
                let mut text = keep.join("\n");
                if !keep.is_empty() && v.buffer.line_start(e + 1) > v.buffer.line_start(e) {
                    text.push('\n');
                }
                v.buffer.replace(start..end, &text);
                cx.emit(TextEvent::Changed);
                cx.notify();
            });
        };
        let (t0, t1, t2) = (take.clone(), take.clone(), take);
        let path = file.path.clone();
        let rel = self.git.as_ref().map(|g| file.path.strip_prefix(&g.session.repo.root).unwrap_or(&file.path).display().to_string()).unwrap_or_default();
        let rel2 = rel.clone();
        let next_view = file.view()?.clone();
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .text_size(px(12.0 * self.z()))
                .bg(hsla_alpha(ui.danger, 0.15))
                .text_color(hsla(ui.text))
                .child(div().flex_1().child(if blocks.is_empty() { "No conflict markers left.".to_string() } else { format!("{} conflict(s). At the cursor, take:", blocks.len()) }))
                .when(!blocks.is_empty(), |d| {
                    d.child(btn("take-ours", "Ours").on_click(cx.listener(move |this, _, _, cx| t0(this, 0, cx))))
                        .child(btn("take-theirs", "Theirs").on_click(cx.listener(move |this, _, _, cx| t1(this, 1, cx))))
                        .child(btn("take-both", "Both").on_click(cx.listener(move |this, _, _, cx| t2(this, 2, cx))))
                        .child(btn("next-conflict", "Next").on_click(cx.listener(move |this, _, _, cx| {
                            let blocks = this.conflicts_in(&next_view, cx);
                            next_view.update(cx, |v, cx| {
                                let line = v.buffer.line_of(v.buffer.selection().head);
                                if let Some(&(s, _, _)) = blocks.iter().find(|(s, _, _)| *s > line).or(blocks.first()) {
                                    v.go_to(s, 0, cx);
                                }
                            });
                        })))
                })
                .child(btn("ask-ai-conflict", "Ask AI").on_click(cx.listener(move |this, _, _, cx| {
                    let text = format!("Resolve the merge conflicts in {rel}: keep what both sides meant, remove the conflict markers, and tell me what you chose.");
                    let bench = this.bench;
                    if let Some(state) = cx.try_global::<crate::state::AppHandle>().and_then(|h| h.0.upgrade()) {
                        let sent = state.update(cx, |s, cx| s.send_to_ai(bench, &path, &text, cx));
                        if !sent {
                            this.notice = Some("This bench has no AI tab yet: use Ask AI above to open one.".into());
                        }
                    }
                })))
                .when(self.remote.is_none(), |d| d.child(btn("three-ways", if self.merge_sides.as_ref().is_some_and(|(p, _, _)| *p == file.path) { "One Way" } else { "Three Ways" }).on_click(cx.listener(move |this, _, _, cx| this.toggle_three_ways(cx)))))
                .child(btn("mark-resolved", "Mark Resolved").text_color(hsla(ui.ink(accent))).on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(i) = this.active {
                        this.save(i, cx);
                    }
                    let p = vec![rel2.clone()];
                    this.git_op("Marking resolved…", GitOp::Stage(p), cx);
                })))
                .into_any_element(),
        )
    }
}

impl ProjectView {
    /// Shows our version and theirs beside the conflicted file, or stops.
    fn toggle_three_ways(&mut self, cx: &mut Context<Self>) {
        let Some(file) = self.active.and_then(|i| self.open.get(i)) else { return };
        if self.merge_sides.as_ref().is_some_and(|(p, _, _)| *p == file.path) {
            self.merge_sides = None;
            return cx.notify();
        }
        let Some(git) = &self.git else { return };
        let Ok(rel) = file.path.strip_prefix(&git.session.repo.root) else { return };
        let rel = rel.display().to_string();
        // Index stages: 2 is ours, 3 theirs, during a merge or rebase.
        let (ours, theirs) = (git.session.repo.show(":2", &rel), git.session.repo.show(":3", &rel));
        let options = TextOptions { read_only: true, line_numbers: true, comment: file.lang.and_then(|l| l.comment), ..Default::default() };
        let settings = self.settings.clone();
        let o = options.clone();
        let ours = cx.new(|cx| TextView::new(&ours, o, &settings, cx));
        let theirs = cx.new(|cx| TextView::new(&theirs, options, &settings, cx));
        self.merge_sides = Some((file.path.clone(), ours.clone(), theirs.clone()));
        let lang = file.lang;
        for view in [ours, theirs] {
            self.highlight_view(lang, view, cx);
        }
        cx.notify();
    }

    /// Colours a read-only view once.
    fn highlight_view(&self, lang: Option<&'static shed_project::Lang>, view: Entity<TextView>, cx: &mut Context<Self>) {
        let Some(lang) = lang else { return };
        let text = view.read(cx).buffer.rope().to_string();
        let palette = self.settings.palette.clone();
        cx.spawn(async move |_, cx| {
            let spans = cx
                .background_executor()
                .spawn(async move {
                    let spans = shed_project::highlight(lang, &text)?;
                    Some(std::sync::Arc::new(spans.into_iter().map(|l| l.into_iter().map(|(r, s)| (r, super::syntax_color(s, &palette))).collect()).collect::<Vec<_>>()))
                })
                .await;
            if let Some(spans) = spans {
                view.update(cx, |v, cx| v.set_highlights(Some(spans), cx));
            }
        })
        .detach();
    }

    pub(super) fn render_three_ways(&self, ours: &Entity<TextView>, result: &Entity<TextView>, theirs: &Entity<TextView>, ui: &UiColors) -> AnyElement {
        let column = |title: &'static str, view: &Entity<TextView>| {
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .child(div().flex_none().px_2().py_0p5().text_size(px(11.0 * self.z())).text_color(hsla(ui.text_dim)).border_b_1().border_color(hsla(ui.divider)).child(title))
                .child(div().flex_1().min_h_0().child(view.clone()))
        };
        let line = || div().flex_none().w(px(1.0)).h_full().bg(hsla(ui.divider));
        div()
            .size_full()
            .flex()
            .child(column("Ours (what you had)", ours))
            .child(line())
            .child(column("Result (edit and save)", result))
            .child(line())
            .child(column("Theirs (coming in)", theirs))
            .into_any_element()
    }
}

fn g_path(git: &Option<GitState>, rel: &str) -> PathBuf {
    git.as_ref().map(|g| g.session.repo.root.join(rel)).unwrap_or_default()
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Section {
    Conflict,
    Staged,
    Unstaged,
}

fn short_ago(secs: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let d = now.saturating_sub(secs);
    match d {
        0..3600 => format!("{} min", d / 60),
        3600..86400 => format!("{} h", d / 3600),
        86400..2_592_000 => format!("{} d", d / 86400),
        _ => format!("{} mo", d / 2_592_000),
    }
}
