//! Reviewing what Claude did, inside its AI tab: every change in the
//! repositories it works in, sorted by how far it went (not staged, staged,
//! committed, pushed), each with its diff, a Viewed mark, and what fits it:
//! stage or discard; unstage; undo a commit (back to staged) or amend it;
//! revert a pushed one (its opposite staged, to commit anew). For this turn,
//! or the whole session: where the branch was then comes from Git's reflog.

use std::path::{Path, PathBuf};

use gpui::{AnyElement, Context, Entity, Focusable, PromptLevel, SharedString, Task, Window, div, prelude::*, px, svg};
use shed_git::{Hunk, LineKind, Repo};
use shed_model::Rgb;

use super::{AiView, Entry};
use crate::assets::icon;
use crate::line_editor::{EditorEvent, LineEditor};
use crate::menu;
use crate::theme::{UiColors, hsla, hsla_alpha};

/// More than this many changed files are not all read.
const MOST: usize = 300;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Scope {
    /// Since the last prompt was sent.
    Turn,
    /// Since the session's first prompt.
    Session,
}

/// How far a change went.
#[derive(Clone, PartialEq)]
enum Layer {
    Unstaged { untracked: bool },
    Staged,
    Committed { hash: String, pushed: bool },
}

struct Change {
    repo: usize,
    layer: Layer,
    path: String,
    /// Git's letter for it: `M`, `A`, `D`, `R`.
    code: char,
    hunks: Vec<Hunk>,
    /// Of what it is now, to tell whether what was viewed is still it.
    hash: String,
}

impl Change {
    fn key(&self, root: &Path) -> String {
        let layer = match &self.layer {
            Layer::Unstaged { .. } => "worktree".to_string(),
            Layer::Staged => "index".to_string(),
            Layer::Committed { hash, .. } => format!("commit {hash}"),
        };
        format!("{}|{layer}|{}", root.display(), self.path)
    }
}

struct CommitInfo {
    hash: String,
    short: String,
    subject: String,
    pushed: bool,
}

struct RepoReview {
    repo: Repo,
    name: String,
    /// Where the branch was when the scope began (`None`: the reflog does not
    /// go back that far; only what is not committed is shown).
    base: Option<String>,
    /// Newest first.
    commits: Vec<CommitInfo>,
}

pub(super) struct Review {
    pub scope: Scope,
    repos: Vec<RepoReview>,
    changes: Vec<Change>,
    selected: Option<usize>,
    loading: Option<Task<()>>,
    op: Option<Task<()>>,
    busy: Option<String>,
    error: Option<String>,
    /// More changes than were read.
    cut: bool,
    message: Entity<LineEditor>,
    _message_sub: gpui::Subscription,
    pub focus: gpui::FocusHandle,
}

/// What reading the repositories found.
struct Found {
    repos: Vec<RepoReview>,
    changes: Vec<Change>,
    cut: bool,
}

/// A stable hash of a change's texts (FNV-1a), kept with Viewed marks.
fn fingerprint(old: &str, new: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in old.bytes().chain([0u8]).chain(new.bytes()) {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// Reads what changed in each repository since `since` (Unix seconds).
fn read(dirs: Vec<PathBuf>, since: Option<u64>) -> Found {
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
            let hunks = shed_git::diff_texts(&old, &new, 3);
            changes.push(Change { repo: index, layer, path, code, hash: fingerprint(&old, &new), hunks });
        };
        if let Ok(status) = repo.status() {
            for e in status.entries.iter().filter(|e| !e.conflict) {
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

impl AiView {
    /// Opens the review (or reads it again, in another scope).
    pub(super) fn open_review(&mut self, scope: Scope, window: &mut Window, cx: &mut Context<Self>) {
        if self.review.is_none() {
            let message = cx.new(|cx| LineEditor::new(String::new(), cx).without_hint());
            let sub = cx.subscribe(&message, |this, _, e: &EditorEvent, cx| {
                if let EditorEvent::Submit(text) = e {
                    let text = text.clone();
                    this.review_commit(text, cx);
                }
            });
            self.review = Some(Review {
                scope,
                repos: Vec::new(),
                changes: Vec::new(),
                selected: None,
                loading: None,
                op: None,
                busy: None,
                error: None,
                cut: false,
                message,
                _message_sub: sub,
                focus: cx.focus_handle(),
            });
        }
        if let Some(r) = &mut self.review {
            r.scope = scope;
            let focus = r.focus.clone();
            window.focus(&focus, cx);
        }
        self.reload_review(cx);
    }

    pub(super) fn close_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.review = None;
        let target = self.focus_target(cx);
        window.focus(&target, cx);
        cx.notify();
    }

    /// When the scope began: the turn's prompt, or the session's first.
    fn review_since(&self, scope: Scope) -> Option<u64> {
        let mut prompts = self.entries.iter().filter_map(|e| if let Entry::User { at, .. } = e { Some(*at) } else { None }).filter(|at| *at > 0);
        match scope {
            Scope::Turn => prompts.next_back(),
            Scope::Session => prompts.next(),
        }
    }

    fn reload_review(&mut self, cx: &mut Context<Self>) {
        let Some(review) = &self.review else { return };
        let since = self.review_since(review.scope);
        let mut dirs = vec![self.ai.folder.clone()];
        dirs.extend(self.ai.add_dirs.iter().cloned());
        let task = cx.spawn(async move |this, cx| {
            let found = cx.background_executor().spawn(async move { read(dirs, since) }).await;
            this.update(cx, |this, cx| {
                if let Some(r) = &mut this.review {
                    let keep = r.selected.and_then(|i| r.changes.get(i)).map(|c| (c.repo, c.layer.clone(), c.path.clone()));
                    r.repos = found.repos;
                    r.changes = found.changes;
                    r.cut = found.cut;
                    r.loading = None;
                    // The same change stays selected, else the first.
                    r.selected = keep
                        .and_then(|(repo, layer, path)| r.changes.iter().position(|c| c.repo == repo && c.layer == layer && c.path == path))
                        .or((!r.changes.is_empty()).then_some(0));
                }
                cx.notify();
            })
            .ok();
        });
        if let Some(r) = &mut self.review {
            r.loading = Some(task);
        }
        cx.notify();
    }

    /// Runs a Git command for the review away from the UI, then reads again.
    fn review_op(&mut self, label: &str, repo: Repo, op: impl FnOnce(&Repo) -> shed_git::Result<String> + Send + 'static, cx: &mut Context<Self>) {
        let Some(r) = &mut self.review else { return };
        if r.busy.is_some() {
            return;
        }
        r.busy = Some(label.to_string());
        r.error = None;
        r.op = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { op(&repo) }).await;
            this.update(cx, |this, cx| {
                if let Some(r) = &mut this.review {
                    r.busy = None;
                    r.error = result.err().map(|e| e.0);
                }
                this.reload_review(cx);
            })
            .ok();
        }));
        cx.notify();
    }

    fn is_viewed(&self, c: &Change) -> bool {
        let Some(r) = &self.review else { return false };
        let key = c.key(&r.repos[c.repo].repo.root);
        self.ai.viewed.iter().any(|(k, h)| *k == key && *h == c.hash)
    }

    /// Marks a change viewed (as it is now), or not; kept with the tab.
    fn toggle_viewed(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(r) = &self.review else { return };
        let Some(c) = r.changes.get(i) else { return };
        let key = c.key(&r.repos[c.repo].repo.root);
        let viewed = self.is_viewed(c);
        let hash = c.hash.clone();
        self.ai.viewed.retain(|(k, _)| *k != key);
        if !viewed {
            self.ai.viewed.push((key, hash));
        }
        // Old marks of changes long gone do not pile up.
        if self.ai.viewed.len() > 2000 {
            let n = self.ai.viewed.len() - 2000;
            self.ai.viewed.drain(..n);
        }
        self.conn.send_msg(shed_proto::ClientMsg::AiSettings { tab: self.tab, settings: self.ai.clone() });
        cx.notify();
    }

    fn review_select(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(r) = &mut self.review
            && i < r.changes.len()
        {
            r.selected = Some(i);
        }
        cx.notify();
    }

    /// Keys in the review: ↑/↓ move, Space marks viewed, N goes to the next
    /// one not viewed, Escape closes.
    pub(super) fn review_key(&mut self, e: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = &self.review else { return };
        if r.message.read(cx).focus_handle(cx).is_focused(window) {
            if e.keystroke.key == "escape" {
                let focus = r.focus.clone();
                window.focus(&focus, cx);
                cx.stop_propagation();
            }
            return;
        }
        let n = r.changes.len();
        let at = r.selected.unwrap_or(0);
        match e.keystroke.key.as_str() {
            "escape" => self.close_review(window, cx),
            "down" | "j" if n > 0 => self.review_select((at + 1).min(n - 1), cx),
            "up" | "k" if n > 0 => self.review_select(at.saturating_sub(1), cx),
            "space" if n > 0 => self.toggle_viewed(at, cx),
            "n" => {
                let next = (1..=n).map(|d| (at + d) % n).find(|&i| !self.is_viewed(&self.review.as_ref().unwrap().changes[i]));
                if let Some(i) = next {
                    self.review_select(i, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn review_commit(&mut self, message: String, cx: &mut Context<Self>) {
        let Some(r) = &self.review else { return };
        if message.trim().is_empty() {
            return;
        }
        // The first repository with something staged.
        let Some(repo) = r.changes.iter().find(|c| c.layer == Layer::Staged).map(|c| r.repos[c.repo].repo.clone()) else { return };
        let field = r.message.clone();
        field.update(cx, |f, cx| f.clear(cx));
        self.review_op("Committing…", repo, move |r| r.commit(&message, false), cx);
    }

    /// Pushes a repository's commits, after showing where and which.
    fn review_push(&mut self, repo_index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = &self.review else { return };
        let repo = r.repos[repo_index].repo.clone();
        let Ok(status) = repo.status() else { return };
        let branch = status.branch;
        let target = repo.push_target(&branch);
        let outgoing = repo.outgoing(&branch, 200).unwrap_or_default();
        if outgoing.is_empty() {
            if let Some(r) = &mut self.review {
                r.error = Some(format!("Nothing to push: {}/{} has every commit.", target.remote, target.remote_branch));
            }
            cx.notify();
            return;
        }
        let mut detail = format!("{} to {}/{}{}\n{}\n\n", target.branch, target.remote, target.remote_branch, if target.new { " (a new branch there)" } else { "" }, target.url);
        for c in outgoing.iter().take(12) {
            detail.push_str(&format!("{}  {}\n", c.short, c.subject));
        }
        if outgoing.len() > 12 {
            detail.push_str(&format!("and {} more\n", outgoing.len() - 12));
        }
        let go = if outgoing.len() == 1 { "Push 1 Commit".to_string() } else { format!("Push {} Commits", outgoing.len()) };
        let answer = window.prompt(PromptLevel::Info, "Push these commits?", Some(&detail), &[go.as_str(), "Cancel"], cx);
        cx.spawn(async move |this, cx| {
            if matches!(answer.await, Ok(0)) {
                this.update(cx, |this, cx| this.review_op("Pushing…", repo, move |r| r.push(false, &branch), cx)).ok();
            }
        })
        .detach();
    }

    fn ask_then(&mut self, title: &str, detail: &str, yes: &str, window: &mut Window, cx: &mut Context<Self>, then: impl FnOnce(&mut Self, &mut Context<Self>) + 'static) {
        let answer = window.prompt(PromptLevel::Warning, title, Some(detail), &[yes, "Cancel"], cx);
        cx.spawn(async move |this, cx| {
            if matches!(answer.await, Ok(0)) {
                this.update(cx, |this, cx| then(this, cx)).ok();
            }
        })
        .detach();
    }

    /// The review, in place of the conversation.
    pub(super) fn render_review(&self, ui: &UiColors, cx: &mut Context<Self>) -> AnyElement {
        let Some(r) = &self.review else { return div().into_any_element() };
        let accent = self.accent;
        let z = self.z();
        let mono = self.settings.font.family.clone();
        let viewed = r.changes.iter().filter(|c| self.is_viewed(c)).count();
        let scope_button = |id: &'static str, label: &'static str, scope: Scope| {
            let on = r.scope == scope;
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded_sm()
                .cursor_pointer()
                .when(on, |d| d.bg(hsla_alpha(accent, 0.22)).text_color(hsla(ui.text)))
                .when(!on, |d| d.text_color(hsla(ui.text_dim)).hover(|s| s.bg(hsla(ui.tab_hover))))
                .on_click(cx.listener(move |this, _, window, cx| this.open_review(scope, window, cx)))
                .child(label)
        };
        let button = |id: SharedString, label: &'static str, color: Rgb| {
            div()
                .id(id)
                .flex_none()
                .px_1p5()
                .rounded_sm()
                .border_1()
                .border_color(hsla_alpha(color, 0.5))
                .cursor_pointer()
                .text_size(px(11.0 * z))
                .text_color(hsla(ui.ink(color)))
                .hover(move |s| s.bg(hsla_alpha(color, 0.15)))
                .child(label)
        };
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_3()
            .py_1p5()
            .border_b_1()
            .border_color(hsla(ui.divider))
            .child(svg().path(icon("branch")).size(px(14.0)).text_color(hsla(ui.ink(accent))))
            .child(div().text_color(hsla(ui.text)).child("Review"))
            .child(div().flex().gap_0p5().p_0p5().rounded_md().bg(hsla(ui.tab_active)).text_size(px(12.0 * z)).child(scope_button("review-turn", "This turn", Scope::Turn)).child(scope_button("review-session", "This session", Scope::Session)))
            .child(div().flex_1())
            .when_some(r.busy.clone().or(r.loading.as_ref().map(|_| "Reading…".to_string())), |d, b| d.child(div().text_color(hsla(ui.ink(accent))).child(b)))
            .when(!r.changes.is_empty(), |d| d.child(div().text_color(hsla(if viewed == r.changes.len() { ui.ink(accent) } else { ui.text_dim })).child(format!("{viewed} of {} viewed", r.changes.len()))))
            .child(
                div()
                    .id("review-close")
                    .p_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(hsla(ui.tab_hover)))
                    .tooltip(menu::tooltip("Back to the conversation (Escape)"))
                    .on_click(cx.listener(|this, _, window, cx| this.close_review(window, cx)))
                    .child(svg().path(icon("close")).size(px(14.0)).text_color(hsla(ui.text_dim))),
            );

        // The list: per repository, what is not staged, staged, then each commit.
        let mut list = div().id("review-list").flex_none().w(px(300.0)).h_full().overflow_y_scroll().border_r_1().border_color(hsla(ui.divider)).py_1().flex().flex_col();
        let section = |title: String, color: Rgb| div().mt_2().px_2().text_size(px(11.0 * z)).text_color(hsla(ui.ink(color))).child(title);
        let (grey, blue, amber, red) = (ui.text_dim, Rgb::hex(0x539BF5), Rgb::hex(0xC69026), ui.danger);
        for (ri, repo) in r.repos.iter().enumerate() {
            if r.repos.len() > 1 {
                list = list.child(div().mt_2().px_2().text_color(hsla(ui.text)).child(repo.name.clone()));
            }
            let row = |i: usize, c: &Change| {
                let selected = r.selected == Some(i);
                let seen = self.is_viewed(c);
                let name = Path::new(&c.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let dir = Path::new(&c.path).parent().map(|p| p.display().to_string()).unwrap_or_default();
                div()
                    .id(("review-row", i))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    .py_0p5()
                    .mx_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .when(selected, |d| d.bg(hsla_alpha(accent, 0.2)))
                    .when(!selected, |d| d.hover(|s| s.bg(hsla(ui.tab_hover))))
                    .on_click(cx.listener(move |this, _, _, cx| this.review_select(i, cx)))
                    .child(
                        div()
                            .id(("review-viewed", i))
                            .flex_none()
                            .cursor_pointer()
                            .text_color(hsla(if seen { ui.ink(accent) } else { ui.text_dim }))
                            .tooltip(menu::tooltip("Viewed (Space)"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_viewed(i, cx);
                            }))
                            .child(if seen { "☑" } else { "☐" }),
                    )
                    .child(div().flex_none().w(px(12.0)).text_color(hsla(ui.text_dim)).child(c.code.to_string()))
                    .child(div().flex_none().text_color(hsla(if seen { ui.text_dim } else { ui.text })).child(name))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(11.0 * z)).text_color(hsla(ui.text_dim)).child(dir))
            };
            let mine = |pick: &dyn Fn(&Layer) -> bool| r.changes.iter().enumerate().filter(|(_, c)| c.repo == ri && pick(&c.layer)).collect::<Vec<_>>();
            let unstaged = mine(&|l| matches!(l, Layer::Unstaged { .. }));
            if !unstaged.is_empty() {
                list = list.child(section(format!("NOT STAGED ({})", unstaged.len()), grey));
                list = list.children(unstaged.into_iter().map(|(i, c)| row(i, c)));
            }
            let staged = mine(&|l| *l == Layer::Staged);
            if !staged.is_empty() {
                list = list.child(section(format!("STAGED ({})", staged.len()), blue));
                list = list.children(staged.into_iter().map(|(i, c)| row(i, c)));
            }
            for (ci, commit) in repo.commits.iter().enumerate() {
                let color = if commit.pushed { red } else { amber };
                list = list.child(
                    div()
                        .mt_2()
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .text_size(px(11.0 * z))
                        .child(div().flex_none().px_1().rounded_sm().bg(hsla_alpha(color, 0.2)).text_color(hsla(ui.ink(color))).child(if commit.pushed { "PUSHED" } else { "COMMITTED" }))
                        .child(div().flex_none().font_family(mono.clone()).text_color(hsla(ui.text_dim)).child(commit.short.clone()))
                        .child(div().flex_1().min_w_0().truncate().text_color(hsla(ui.text)).child(commit.subject.clone())),
                );
                // What fits it: back to staged or amended (not pushed, and
                // nothing pushed after it), or reverted as a new change.
                let undoable = !commit.pushed && repo.commits[..ci].iter().all(|c| !c.pushed);
                let hash = commit.hash.clone();
                let mut actions = div().flex().flex_wrap().gap_1().px_2().pt_0p5();
                if undoable {
                    let (repo_c, h) = (repo.repo.clone(), hash.clone());
                    actions = actions.child(
                        button(format!("undo-{ri}-{ci}").into(), "Undo (back to staged)", amber)
                            .tooltip(menu::tooltip("Take this commit (and any after it) back out; its changes stay, staged, to change and commit again"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let (repo, to) = (repo_c.clone(), format!("{h}^"));
                                this.review_op("Undoing the commit…", repo, move |r| r.undo_commits(&to), cx);
                            })),
                    );
                    if ci == 0 {
                        let repo_c = repo.repo.clone();
                        actions = actions.child(
                            button(format!("amend-{ri}").into(), "Amend with Staged", amber)
                                .tooltip(menu::tooltip("Put what is staged into this commit, same message"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let repo = repo_c.clone();
                                    this.review_op("Amending…", repo, |r| {
                                        let message = r.last_message();
                                        r.commit(&message, true)
                                    }, cx);
                                })),
                        );
                    }
                } else if commit.pushed {
                    let (repo_c, h, subject) = (repo.repo.clone(), hash.clone(), commit.subject.clone());
                    actions = actions.child(
                        button(format!("revert-{ri}-{ci}").into(), "Revert…", red)
                            .tooltip(menu::tooltip("Stage the opposite of this commit, to review and commit as a new one (pushed history is not rewritten)"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let (repo, h) = (repo_c.clone(), h.clone());
                                this.ask_then(
                                    "Revert this commit?",
                                    &format!("“{subject}” is pushed, so it stays. Its opposite is staged instead, for you to change if need be and commit as a new commit."),
                                    "Stage the Revert",
                                    window,
                                    cx,
                                    move |this, cx| this.review_op("Reverting…", repo, move |r| r.revert_staged(&[h]), cx),
                                );
                            })),
                    );
                }
                list = list.child(actions);
                let files = mine(&|l| matches!(l, Layer::Committed { hash: h, .. } if *h == hash));
                list = list.children(files.into_iter().map(|(i, c)| row(i, c)));
            }
            if repo.base.is_none() && self.review_since(r.scope).is_some() {
                list = list.child(div().mt_2().px_2().text_size(px(11.0 * z)).text_color(hsla(ui.text_dim)).child("Git's record does not go back to the start of this; commits are not shown."));
            }
        }
        if r.changes.is_empty() && r.loading.is_none() {
            list = list.child(div().p_3().text_color(hsla(ui.text_dim)).child(match r.scope {
                Scope::Turn => "Nothing changed in this turn.",
                Scope::Session => "Nothing changed in this session.",
            }));
        }
        if r.cut {
            list = list.child(div().p_2().text_size(px(11.0 * z)).text_color(hsla(ui.text_dim)).child(format!("Only the first {MOST} changes are shown.")));
        }

        // The selected change: its file actions, then its diff.
        let diff = match r.selected.and_then(|i| r.changes.get(i).map(|c| (i, c))) {
            Some((i, c)) => self.render_review_diff(i, c, ui, &mono, cx),
            None => div().flex_1().into_any_element(),
        };
        let staged_any = r.changes.iter().any(|c| c.layer == Layer::Staged);
        let unpushed: Vec<usize> = r.repos.iter().enumerate().filter(|(_, repo)| repo.commits.iter().any(|c| !c.pushed)).map(|(i, _)| i).collect();
        let footer = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_3()
            .py_1p5()
            .border_t_1()
            .border_color(hsla(ui.divider))
            .when_some(r.error.clone(), |d, e| d.child(div().flex_1().min_w_0().truncate().text_color(hsla(ui.danger)).child(e)))
            .when(staged_any, |d| {
                d.child(div().text_color(hsla(ui.text_dim)).child("New commit:"))
                    .child(div().w(px(320.0)).px_1().rounded_sm().border_1().border_color(hsla(ui.divider)).child(r.message.clone()))
                    .child(button("review-commit".into(), "Commit Staged", accent).on_click(cx.listener(|this, _, _, cx| {
                        let Some(r) = &this.review else { return };
                        let text = r.message.read(cx).text().to_string();
                        this.review_commit(text, cx);
                    })))
            })
            .child(div().flex_1())
            .children(unpushed.into_iter().map(|ri| {
                let label = if r.repos.len() > 1 { format!("Push {}…", r.repos[ri].name) } else { "Push…".to_string() };
                div()
                    .id(("review-push", ri))
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .border_1()
                    .border_color(hsla(accent))
                    .cursor_pointer()
                    .text_color(hsla(ui.ink(accent)))
                    .hover(move |s| s.bg(hsla_alpha(accent, 0.2)))
                    .on_click(cx.listener(move |this, _, window, cx| this.review_push(ri, window, cx)))
                    .child(label)
            }));
        div()
            .id("review")
            .track_focus(&r.focus)
            .on_key_down(cx.listener(Self::review_key))
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(header)
            .child(div().flex_1().min_h_0().flex().child(list).child(diff))
            .child(footer)
            .into_any_element()
    }

    fn render_review_diff(&self, i: usize, c: &Change, ui: &UiColors, mono: &str, cx: &mut Context<Self>) -> AnyElement {
        let Some(r) = &self.review else { return div().into_any_element() };
        let z = self.z();
        let repo = r.repos[c.repo].repo.clone();
        let full = repo.root.join(&c.path);
        let (red, green) = (Rgb::hex(0xE5534B), Rgb::hex(0x57AB5A));
        let small = |id: SharedString, label: &'static str, color: Rgb| {
            div()
                .id(id)
                .flex_none()
                .px_1p5()
                .rounded_sm()
                .cursor_pointer()
                .text_size(px(11.0 * z))
                .text_color(hsla(ui.ink(color)))
                .hover(move |s| s.bg(hsla_alpha(color, 0.15)))
                .child(label)
        };
        let (layer_label, layer_color) = match &c.layer {
            Layer::Unstaged { untracked: true } => ("new, not staged", ui.text_dim),
            Layer::Unstaged { .. } => ("not staged", ui.text_dim),
            Layer::Staged => ("staged", Rgb::hex(0x539BF5)),
            Layer::Committed { pushed: false, .. } => ("committed, not pushed", Rgb::hex(0xC69026)),
            Layer::Committed { pushed: true, .. } => ("pushed", ui.danger),
        };
        let path = c.path.clone();
        let mut bar = div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .border_b_1()
            .border_color(hsla(ui.divider))
            .child(div().font_family(mono.to_string()).text_color(hsla(ui.text)).child(c.path.clone()))
            .child(div().px_1().rounded_sm().bg(hsla_alpha(layer_color, 0.2)).text_size(px(11.0 * z)).text_color(hsla(ui.ink(layer_color))).child(layer_label))
            .child(div().flex_1());
        match &c.layer {
            Layer::Unstaged { untracked } => {
                let (p1, p2, repo1, repo2, untracked) = (path.clone(), path.clone(), repo.clone(), repo.clone(), *untracked);
                bar = bar
                    .child(small(format!("stage-file-{i}").into(), "Stage", green).on_click(cx.listener(move |this, _, _, cx| {
                        let p = vec![p1.clone()];
                        this.review_op("Staging…", repo1.clone(), move |r| r.stage(&p).map(|_| String::new()), cx);
                    })))
                    .child(small(format!("discard-file-{i}").into(), "Discard…", red).on_click(cx.listener(move |this, _, window, cx| {
                        let (p, repo) = (p2.clone(), repo2.clone());
                        this.ask_then(&format!("Discard the changes to {p}?"), "They are lost: Git cannot bring them back.", "Discard", window, cx, move |this, cx| {
                            let (tracked, untracked_files) = if untracked { (Vec::new(), vec![p]) } else { (vec![p], Vec::new()) };
                            this.review_op("Discarding…", repo, move |r| r.discard(&tracked, &untracked_files).map(|_| String::new()), cx);
                        });
                    })));
            }
            Layer::Staged => {
                let (p1, repo1) = (path.clone(), repo.clone());
                bar = bar.child(small(format!("unstage-file-{i}").into(), "Unstage", ui.text_dim).on_click(cx.listener(move |this, _, _, cx| {
                    let p = vec![p1.clone()];
                    this.review_op("Unstaging…", repo1.clone(), move |r| r.unstage(&p).map(|_| String::new()), cx);
                })));
            }
            Layer::Committed { .. } => {}
        }
        // Changing the file itself: in its project tab.
        if !matches!(c.layer, Layer::Committed { .. }) && full.is_file() {
            bar = bar.child(small(format!("edit-file-{i}").into(), "Edit in Project", self.accent).on_click(cx.listener(move |this, _, _, cx| this.reveal_file(full.clone(), None, cx))));
        }

        let mut lines = div().id(("review-diff", i)).flex_1().min_h_0().overflow_y_scroll().font_family(mono.to_string()).text_size(px(12.0 * z)).py_1();
        if c.hunks.is_empty() {
            lines = lines.child(div().p_3().text_color(hsla(ui.text_dim)).child("No differences in the text (a binary file, or only its mode)."));
        }
        let mut shown = 0;
        for (h, hunk) in c.hunks.iter().enumerate() {
            if shown > 3000 {
                lines = lines.child(div().p_2().text_color(hsla(ui.text_dim)).child("… the rest is not shown"));
                break;
            }
            let patch = shed_git::hunk_patch(&c.path, hunk);
            let mut head = div()
                .flex()
                .items_center()
                .gap_2()
                .mt_1()
                .px_3()
                .bg(hsla_alpha(ui.divider, 0.5))
                .text_color(hsla(ui.text_dim))
                .child(format!("@@ -{},{} +{},{} @@", hunk.old_start, hunk.old_len, hunk.new_start, hunk.new_len))
                .child(div().flex_1());
            match &c.layer {
                Layer::Unstaged { untracked: false } => {
                    let (p1, p2, repo1, repo2) = (patch.clone(), patch.clone(), repo.clone(), repo.clone());
                    head = head
                        .child(small(format!("stage-hunk-{i}-{h}").into(), "Stage", green).on_click(cx.listener(move |this, _, _, cx| {
                            let p = p1.clone();
                            this.review_op("Staging…", repo1.clone(), move |r| r.apply(&p, true, false).map(|_| String::new()), cx);
                        })))
                        .child(small(format!("discard-hunk-{i}-{h}").into(), "Discard…", red).on_click(cx.listener(move |this, _, window, cx| {
                            let (p, repo) = (p2.clone(), repo2.clone());
                            this.ask_then("Discard this change?", "It is lost: Git cannot bring it back.", "Discard", window, cx, move |this, cx| {
                                this.review_op("Discarding…", repo, move |r| r.apply(&p, false, true).map(|_| String::new()), cx);
                            });
                        })));
                }
                Layer::Staged => {
                    let (p1, repo1) = (patch.clone(), repo.clone());
                    head = head.child(small(format!("unstage-hunk-{i}-{h}").into(), "Unstage", ui.text_dim).on_click(cx.listener(move |this, _, _, cx| {
                        let p = p1.clone();
                        this.review_op("Unstaging…", repo1.clone(), move |r| r.apply(&p, true, true).map(|_| String::new()), cx);
                    })));
                }
                _ => {}
            }
            lines = lines.child(head);
            for line in &hunk.lines {
                shown += 1;
                let (sign, bg) = match line.kind {
                    LineKind::Added => ("+", Some(green)),
                    LineKind::Removed => ("-", Some(red)),
                    LineKind::Context => (" ", None),
                };
                lines = lines.child(
                    div()
                        .flex()
                        .px_3()
                        .min_h(px(16.0 * z))
                        .when_some(bg, |d, bg| d.bg(hsla_alpha(bg, 0.15)))
                        .text_color(hsla(ui.text))
                        .child(div().flex_none().w(px(14.0)).text_color(hsla(ui.text_dim)).child(sign))
                        .child(div().flex_1().min_w_0().child(line.text.trim_end_matches('\n').to_string())),
                );
            }
        }
        div().flex_1().min_w_0().h_full().flex().flex_col().child(bar).child(lines).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(ok.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&ok.stderr));
    }

    #[test]
    fn changes_are_sorted_by_how_far_they_went() {
        let dir = tempfile::tempdir().unwrap();
        let (work, remote) = (dir.path().join("work"), dir.path().join("remote.git"));
        std::fs::create_dir_all(&work).unwrap();
        git(dir.path(), &["init", "-q", "--bare", "-b", "main", remote.to_str().unwrap()]);
        git(&work, &["init", "-q", "-b", "main"]);
        for (k, v) in [("user.email", "t@example.com"), ("user.name", "T"), ("commit.gpgsign", "false")] {
            git(&work, &["config", k, v]);
        }
        git(&work, &["remote", "add", "origin", remote.to_str().unwrap()]);
        let write = |name: &str, text: &str| std::fs::write(work.join(name), text).unwrap();
        write("a.txt", "a\n");
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-qm", "before"]);
        git(&work, &["push", "-q", "-u", "origin", "main"]);
        // Git's record of that, from an hour before the session.
        let start = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() - 600;
        for log in [".git/logs/HEAD", ".git/logs/refs/heads/main"] {
            let path = work.join(log);
            let text = std::fs::read_to_string(&path).unwrap();
            let old = (start - 3000).to_string();
            let fixed: String = text.lines().map(|l| {
                let (head, tail) = l.split_once('\t').unwrap();
                let mut parts: Vec<&str> = head.split(' ').collect();
                let n = parts.len();
                parts[n - 2] = &old;
                format!("{}\t{tail}\n", parts.join(" "))
            }).collect();
            std::fs::write(&path, fixed).unwrap();
        }
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
        // Before the reflog: what is not committed only.
        let earlier = read(vec![work], Some(start - 7200));
        assert!(earlier.repos[0].base.is_none());
        assert_eq!(earlier.changes.len(), 2);
        // A viewed change is told by its texts.
        assert_eq!(fingerprint("a", "b"), fingerprint("a", "b"));
        assert_ne!(fingerprint("a", "b"), fingerprint("ab", ""));
    }
}
