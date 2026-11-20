//! A project tab's Git panel (`shed_git::session`): what changed, staged or
//! not, each file's diff by hunks; staging, unstaging and discarding files
//! or hunks, committing, the history, branches, fetch, pull and push.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use shed_git::session::{self, BranchOp, DiffContent, DiffSource, GitSession, HunkAction, PushReview};
use shed_git::{Commit, Hunk, Repo};
use shed_model::TabId;

use crate::app::App;
use crate::overlay::Item;
use crate::project::{ProjectView, Side};
use crate::text::{fit, pad};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Heading(String),
    File { path: String, staged: bool, untracked: bool, conflict: bool, code: char },
}

pub struct DiffView {
    pub path: String,
    pub source: DiffSource,
    pub untracked: bool,
    pub hunks: Vec<Hunk>,
    pub hunk: usize,
    pub scroll: usize,
    pub note: Option<String>,
}

/// A commit's hash, its files (Git's letter and path) and the one selected.
pub type CommitFiles = (String, Vec<(char, String)>, usize);

pub struct LogView {
    pub commits: Vec<Commit>,
    pub sel: usize,
    /// A commit opened: its hash, its files, the one selected.
    pub files: Option<CommitFiles>,
}

#[derive(Default)]
pub struct GitView {
    pub session: Option<GitSession>,
    /// Looked for a repository and found none.
    pub none: bool,
    pub rows: Vec<Row>,
    pub sel: usize,
    pub diff: Option<DiffView>,
    /// Keys go to the diff.
    pub in_diff: bool,
    pub busy: Option<String>,
    pub log: Option<LogView>,
}

impl GitView {
    pub fn selected_path(&self) -> Option<&str> {
        match self.rows.get(self.sel) {
            Some(Row::File { path, .. }) => Some(path),
            _ => None,
        }
    }

    pub fn scroll_diff(&mut self, by: isize) {
        if let Some(d) = &mut self.diff {
            d.scroll = d.scroll.saturating_add_signed(by);
        }
    }

    fn rebuild(&mut self) {
        let Some(status) = self.session.as_ref().and_then(|s| s.status.as_ref()) else { return };
        let keep = self.rows.get(self.sel).cloned();
        let mut rows = Vec::new();
        let conflicts: Vec<_> = status.conflicts().collect();
        if !conflicts.is_empty() {
            rows.push(Row::Heading(format!("Conflicts ({})", conflicts.len())));
            rows.extend(conflicts.iter().map(|e| Row::File { path: e.path.clone(), staged: false, untracked: false, conflict: true, code: 'U' }));
        }
        let staged: Vec<_> = status.staged().collect();
        if !staged.is_empty() {
            rows.push(Row::Heading(format!("Staged ({})", staged.len())));
            rows.extend(staged.iter().map(|e| Row::File { path: e.path.clone(), staged: true, untracked: false, conflict: false, code: e.index }));
        }
        let unstaged: Vec<_> = status.unstaged().collect();
        if !unstaged.is_empty() {
            rows.push(Row::Heading(format!("Changes ({})", unstaged.len())));
            rows.extend(unstaged.iter().map(|e| Row::File { path: e.path.clone(), staged: false, untracked: e.untracked, conflict: false, code: if e.untracked { '?' } else { e.worktree } }));
        }
        self.rows = rows;
        self.sel = keep.and_then(|k| self.rows.iter().position(|r| *r == k)).unwrap_or(self.sel).min(self.rows.len().saturating_sub(1));
        if matches!(self.rows.get(self.sel), Some(Row::Heading(_))) && self.sel + 1 < self.rows.len() {
            self.sel += 1;
        }
    }

    fn step(&mut self, by: isize) {
        let n = self.rows.len() as isize;
        let mut at = self.sel as isize;
        loop {
            at += by;
            if at < 0 || at >= n {
                return;
            }
            if matches!(self.rows[at as usize], Row::File { .. }) {
                self.sel = at as usize;
                return;
            }
        }
    }
}

fn git(app: &mut App, tab: TabId) -> Option<&mut GitView> {
    match app.projects.get_mut(&tab) {
        Some(ProjectView { side: Side::Git(g), .. }) => Some(g),
        _ => None,
    }
}

impl App {
    pub(crate) fn git_open(&mut self, tab: TabId) {
        let Some(v) = self.projects.get_mut(&tab) else { return };
        if !matches!(v.side, Side::Git(_)) {
            v.side = Side::Git(Box::default());
        }
        v.right = false;
        let root = v.root.clone();
        self.spawn(move || {
            let session = GitSession::open(&root);
            let refreshed = session.as_ref().map(|s| session::refresh(&s.repo));
            move |app: &mut App| {
                let Some(g) = git(app, tab) else { return };
                match (session, refreshed) {
                    (Some(mut s), Some(r)) => {
                        if let Ok(r) = r {
                            s.apply_refresh(r);
                        }
                        g.session = Some(s);
                        g.rebuild();
                        app.git_load_diff(tab);
                    }
                    _ => g.none = true,
                }
            }
        });
    }

    pub(crate) fn git_refresh(&mut self, tab: TabId) {
        let Some(repo) = git(self, tab).and_then(|g| g.session.as_ref()).map(|s| s.repo.clone()) else { return };
        self.spawn(move || {
            let r = session::refresh(&repo);
            move |app: &mut App| {
                let Some(g) = git(app, tab) else { return };
                match r {
                    Ok(r) => {
                        if let Some(s) = &mut g.session {
                            s.apply_refresh(r);
                        }
                        g.rebuild();
                        app.git_load_diff(tab);
                    }
                    Err(e) => app.error(e.to_string()),
                }
            }
        });
    }

    /// The selected file's diff (or the opened commit file's).
    fn git_load_diff(&mut self, tab: TabId) {
        let Some(g) = git(self, tab) else { return };
        let Some(repo) = g.session.as_ref().map(|s| s.repo.clone()) else { return };
        let (path, source, untracked) = if let Some(LogView { files: Some((hash, files, sel)), .. }) = &g.log {
            let Some((_, path)) = files.get(*sel) else { return };
            (path.clone(), DiffSource::Commit(hash.clone()), false)
        } else {
            match g.rows.get(g.sel) {
                Some(Row::File { path, staged, untracked, .. }) => (path.clone(), if *staged { DiffSource::Staged } else { DiffSource::Unstaged }, *untracked),
                _ => {
                    g.diff = None;
                    return;
                }
            }
        };
        let picture = shed_project::picture::is_picture(std::path::Path::new(&path));
        let keep = g.diff.as_ref().filter(|d| d.path == path && d.source == source).map(|d| (d.hunk, d.scroll));
        self.spawn(move || {
            let content = session::load_diff(&repo, &path, &source, untracked, picture);
            move |app: &mut App| {
                let Some(g) = git(app, tab) else { return };
                let (hunk, scroll) = keep.unwrap_or((0, 0));
                let (hunks, note) = match content {
                    DiffContent::Hunks(h) => (h, None),
                    DiffContent::Pictures { old, new } => (Vec::new(), Some(format!("A picture: {} bytes before, {} now.", old.len(), new.len()))),
                };
                g.diff = Some(DiffView { path, source, untracked, hunk: hunk.min(hunks.len().saturating_sub(1)), hunks, scroll, note });
            }
        });
    }

    /// Runs a Git operation off the screen's thread, then reads the status again.
    fn git_op(&mut self, tab: TabId, label: &str, op: impl FnOnce(&Repo) -> shed_git::Result<String> + Send + 'static) {
        let Some(g) = git(self, tab) else { return };
        let Some(repo) = g.session.as_ref().map(|s| s.repo.clone()) else { return };
        if g.busy.is_some() {
            self.note("Git is still busy");
            return;
        }
        g.busy = Some(label.to_string());
        self.spawn(move || {
            let result = op(&repo);
            move |app: &mut App| {
                if let Some(g) = git(app, tab) {
                    g.busy = None;
                }
                match result {
                    Ok(text) if !text.trim().is_empty() => app.note(text.trim().lines().last().unwrap_or("").to_string()),
                    Ok(_) => {}
                    Err(e) => app.error(e.to_string()),
                }
                app.git_refresh(tab);
            }
        });
    }

    pub(crate) fn git_key(&mut self, tab: TabId, ev: KeyEvent) {
        let Some(g) = git(self, tab) else { return };
        if g.log.is_some() {
            return self.log_key(tab, ev);
        }
        let selected = g.rows.get(g.sel).cloned();
        if g.in_diff {
            let Some(d) = &mut g.diff else {
                g.in_diff = false;
                return;
            };
            let hunk = d.hunks.get(d.hunk).cloned();
            let (path, untracked, source) = (d.path.clone(), d.untracked, d.source.clone());
            match ev.code {
                KeyCode::Esc | KeyCode::Tab | KeyCode::Left | KeyCode::Char('h') => g.in_diff = false,
                KeyCode::Up | KeyCode::Char('k') => d.scroll = d.scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => d.scroll += 1,
                KeyCode::PageUp => d.scroll = d.scroll.saturating_sub(10),
                KeyCode::PageDown | KeyCode::Char(' ') => d.scroll += 10,
                KeyCode::Char(']') | KeyCode::Char('n') => {
                    d.hunk = (d.hunk + 1).min(d.hunks.len().saturating_sub(1));
                    d.scroll = hunk_top(&d.hunks, d.hunk);
                }
                KeyCode::Char('[') | KeyCode::Char('p') => {
                    d.hunk = d.hunk.saturating_sub(1);
                    d.scroll = hunk_top(&d.hunks, d.hunk);
                }
                KeyCode::Char('s') if source == DiffSource::Unstaged => {
                    if let Some(h) = hunk {
                        self.git_op(tab, "Staging…", move |repo| session::hunk_action(repo, &path, &h, HunkAction::Stage, untracked).map(|_| String::new()));
                    }
                }
                KeyCode::Char('u') if source == DiffSource::Staged => {
                    if let Some(h) = hunk {
                        self.git_op(tab, "Unstaging…", move |repo| session::hunk_action(repo, &path, &h, HunkAction::Unstage, false).map(|_| String::new()));
                    }
                }
                KeyCode::Char('d') if source == DiffSource::Unstaged && !untracked => {
                    if let Some(h) = hunk {
                        self.confirm("Discard Change", format!("Throw away this change to {path}? It cannot be undone."), "Discard", true, move |app| {
                            app.git_op(tab, "Discarding…", move |repo| session::hunk_action(repo, &path, &h, HunkAction::Discard, false).map(|_| String::new()));
                        });
                    }
                }
                _ => {}
            }
            return;
        }
        match ev.code {
            KeyCode::Esc => {
                if let Some(v) = self.projects.get_mut(&tab) {
                    v.side = Side::Files;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                g.step(-1);
                self.git_load_diff(tab);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                g.step(1);
                self.git_load_diff(tab);
            }
            KeyCode::Enter | KeyCode::Tab | KeyCode::Right => g.in_diff = g.diff.is_some(),
            KeyCode::Char('r') => self.git_refresh(tab),
            KeyCode::Char('s') => match selected {
                Some(Row::File { path, staged: false, .. }) => self.git_op(tab, "Staging…", move |repo| repo.stage(&[path]).map(|_| String::new())),
                _ => {
                    let all: Vec<String> = g.rows.iter().filter_map(|r| match r {
                        Row::File { path, staged: false, conflict: false, .. } => Some(path.clone()),
                        _ => None,
                    }).collect();
                    if !all.is_empty() {
                        self.git_op(tab, "Staging…", move |repo| repo.stage(&all).map(|_| String::new()));
                    }
                }
            },
            KeyCode::Char('S') => {
                let all: Vec<String> = g.rows.iter().filter_map(|r| match r {
                    Row::File { path, staged: false, conflict: false, .. } => Some(path.clone()),
                    _ => None,
                }).collect();
                if !all.is_empty() {
                    self.git_op(tab, "Staging…", move |repo| repo.stage(&all).map(|_| String::new()));
                }
            }
            KeyCode::Char('u') => {
                if let Some(Row::File { path, staged: true, .. }) = selected {
                    self.git_op(tab, "Unstaging…", move |repo| repo.unstage(&[path]).map(|_| String::new()));
                }
            }
            KeyCode::Char('d') => {
                if let Some(Row::File { path, staged: false, untracked, conflict: false, .. }) = selected {
                    let what = if untracked { "Delete this new file" } else { "Throw away the changes to" };
                    self.confirm("Discard Changes", format!("{what} {path}? It cannot be undone."), "Discard", true, move |app| {
                        app.git_op(tab, "Discarding…", move |repo| {
                            let (tracked, new) = if untracked { (vec![], vec![path]) } else { (vec![path], vec![]) };
                            repo.discard(&tracked, &new).map(|_| String::new())
                        });
                    });
                }
            }
            KeyCode::Char('c') => self.git_commit(tab, false),
            KeyCode::Char('C') => self.git_commit(tab, true),
            KeyCode::Char('l') => self.git_log(tab),
            KeyCode::Char('b') => self.git_branches(tab),
            KeyCode::Char('f') => self.git_op(tab, BranchOp::Fetch.label(), |repo| BranchOp::Fetch.run(repo)),
            KeyCode::Char('F') => self.menu("Pull", vec![Item::new("Pull, Rebasing"), Item::new("Pull, Merging")], move |app, i| {
                let op = if i == 0 { BranchOp::PullRebase } else { BranchOp::PullMerge };
                app.git_op(tab, op.label(), move |repo| op.run(repo));
            }),
            KeyCode::Char('p') => self.git_push(tab, false),
            KeyCode::Char('P') => self.git_push(tab, true),
            KeyCode::Char('e') => self.edit_in_editor(),
            _ => {}
        }
    }

    fn git_commit(&mut self, tab: TabId, amend: bool) {
        let Some(repo) = git(self, tab).and_then(|g| g.session.as_ref()).map(|s| s.repo.clone()) else { return };
        let staged = git(self, tab).is_some_and(|g| g.rows.iter().any(|r| matches!(r, Row::File { staged: true, .. })));
        if !staged && !amend {
            self.note("nothing staged to commit (s stages a file, S everything)");
            return;
        }
        let initial = if amend { repo.last_message() } else { String::new() };
        let title = if amend { "Amend the Last Commit" } else { "Commit" };
        self.ask_with_hint(title, "the message; Enter commits", initial.trim_end(), move |app, message| {
            app.git_op(tab, "Committing…", move |repo| session::commit(repo, &message, amend));
        });
    }

    fn git_push(&mut self, tab: TabId, force: bool) {
        let Some(repo) = git(self, tab).and_then(|g| g.session.as_ref()).map(|s| s.repo.clone()) else { return };
        self.spawn(move || {
            let review = PushReview::read(&repo, force);
            move |app: &mut App| match review {
                Err(e) => app.error(e.to_string()),
                Ok(review) => {
                    let t = &review.target;
                    let mut text = format!("{} to {}/{} ({})", review.action_label(), t.remote, t.remote_branch, t.url);
                    if t.new {
                        text.push_str("\nThe branch is new there.");
                    }
                    for c in review.commits.iter().take(12) {
                        text.push_str(&format!("\n  {} {}", c.short, c.subject));
                    }
                    if review.commits.len() > 12 {
                        text.push_str(&format!("\n  … and {} more", review.commits.len() - 12));
                    }
                    let label = review.action_label();
                    let busy = review.busy_label();
                    app.confirm(if force { "Force-Push" } else { "Push" }, text, label, force, move |app| {
                        app.git_op(tab, busy, move |repo| review.confirm(repo));
                    });
                }
            }
        });
    }

    fn git_branches(&mut self, tab: TabId) {
        let Some(s) = git(self, tab).and_then(|g| g.session.as_ref()).cloned() else { return };
        self.spawn(move || {
            let branches = s.branches();
            move |app: &mut App| {
                let mut items = vec![Item::new("New Branch…"), Item::new("Stash Changes"), Item::new("Pop the Stash")];
                items.extend(branches.current.iter().map(|b| Item::new(b.clone()).hint("checked out")));
                items.extend(branches.others.iter().map(|b| Item::new(b.clone())));
                let others = branches.others.clone();
                let offset = 3 + usize::from(branches.current.is_some());
                app.menu("Branches", items, move |app, i| match i {
                    0 => app.ask("New Branch", "", move |app, name| {
                        let op = BranchOp::Create(name.trim().to_string());
                        app.git_op(tab, op.label(), move |repo| op.run(repo));
                    }),
                    1 => app.git_op(tab, BranchOp::Stash.label(), |repo| BranchOp::Stash.run(repo)),
                    2 => app.git_op(tab, BranchOp::StashPop.label(), |repo| BranchOp::StashPop.run(repo)),
                    i if i >= offset => {
                        let b = others[i - offset].clone();
                        let items = vec![Item::new("Switch to It"), Item::new("Merge into This One"), Item::new("Rebase This One onto It"), Item::new("Delete…").danger()];
                        app.menu(b.clone(), items, move |app, j| {
                            let op = match j {
                                0 => BranchOp::Switch(b.clone()),
                                1 => BranchOp::Merge(b.clone()),
                                2 => BranchOp::Rebase(b.clone()),
                                _ => {
                                    let name = b.clone();
                                    app.confirm("Delete Branch", format!("Delete the branch {name}?"), "Delete", true, move |app| {
                                        let op = BranchOp::Delete { name, force: false };
                                        app.git_op(tab, op.label(), move |repo| op.run(repo));
                                    });
                                    return;
                                }
                            };
                            app.git_op(tab, op.label(), move |repo| op.run(repo));
                        });
                    }
                    _ => {}
                });
            }
        });
    }

    fn git_log(&mut self, tab: TabId) {
        let Some(repo) = git(self, tab).and_then(|g| g.session.as_ref()).map(|s| s.repo.clone()) else { return };
        self.spawn(move || {
            let log = session::history(&repo, None);
            move |app: &mut App| match log {
                Ok(commits) => {
                    if let Some(g) = git(app, tab) {
                        g.log = Some(LogView { commits, sel: 0, files: None });
                        g.diff = None;
                    }
                }
                Err(e) => app.error(e.to_string()),
            }
        });
    }

    fn log_key(&mut self, tab: TabId, ev: KeyEvent) {
        let Some(g) = git(self, tab) else { return };
        let Some(log) = &mut g.log else { return };
        if let Some((_, files, sel)) = &mut log.files {
            match ev.code {
                KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => {
                    log.files = None;
                    g.diff = None;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    *sel = sel.saturating_sub(1);
                    self.git_load_diff(tab);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    *sel = (*sel + 1).min(files.len().saturating_sub(1));
                    self.git_load_diff(tab);
                }
                KeyCode::PageDown | KeyCode::Char(' ') => g.scroll_diff(10),
                KeyCode::PageUp => g.scroll_diff(-10),
                _ => {}
            }
            return;
        }
        match ev.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                g.log = None;
                self.git_load_diff(tab);
            }
            KeyCode::Up | KeyCode::Char('k') => log.sel = log.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => log.sel = (log.sel + 1).min(log.commits.len().saturating_sub(1)),
            KeyCode::PageUp => log.sel = log.sel.saturating_sub(10),
            KeyCode::PageDown => log.sel = (log.sel + 10).min(log.commits.len().saturating_sub(1)),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                let Some(c) = log.commits.get(log.sel) else { return };
                let hash = c.hash.clone();
                let files = g.session.as_mut().map(|s| s.commit_files(&hash).to_vec()).unwrap_or_default();
                if let Some(log) = &mut g.log {
                    log.files = Some((hash, files, 0));
                }
                self.git_load_diff(tab);
            }
            _ => {}
        }
    }

    pub(crate) fn render_git(&mut self, f: &mut Frame, list: Rect, view: Rect, tab: TabId) {
        let c = self.colors.clone();
        let Some(g) = git(self, tab) else { return };
        let w = list.width as usize;
        let head = match g.session.as_ref().and_then(|s| s.branch_label()) {
            Some(label) => label,
            None if g.none => "not a Git repository".into(),
            None => "reading…".into(),
        };
        let busy = g.busy.clone().map(|b| format!(" {b}")).unwrap_or_default();
        f.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(fit(&format!(" ⎇ {head}"), w), c.accent().add_modifier(Modifier::BOLD)), Span::styled(busy, c.dim())])),
            Rect { height: 1, ..list },
        );
        let rows = Rect { y: list.y + 1, height: list.height.saturating_sub(2), ..list };
        let hint = Rect { y: list.y + list.height.saturating_sub(1), height: 1, ..list };
        let mut lines: Vec<Line> = Vec::new();
        if let Some(log) = &g.log {
            match &log.files {
                Some((hash, files, sel)) => {
                    lines.push(Line::styled(fit(&format!("{} files", &hash[..hash.len().min(8)]), w), c.dim()));
                    for (i, (code, path)) in files.iter().enumerate() {
                        lines.push(Line::styled(pad(&format!(" {code} {path}"), w), if i == *sel { c.selected() } else { Style::new() }));
                    }
                }
                None => {
                    for (i, cm) in log.commits.iter().enumerate() {
                        let text = format!("{} {} · {}", cm.short, cm.subject, crate::ai::ago(cm.time));
                        lines.push(Line::styled(pad(&text, w), if i == log.sel { c.selected() } else { Style::new() }));
                    }
                }
            }
            let sel = log.files.as_ref().map_or(log.sel, |(_, _, s)| s + 1);
            let h = rows.height as usize;
            let top = sel.saturating_sub(h.saturating_sub(1));
            f.render_widget(Paragraph::new(lines.into_iter().skip(top).take(h).collect::<Vec<_>>()), rows);
            f.render_widget(Paragraph::new(Line::styled(fit("Enter opens · Esc back", w), c.dim())), hint);
        } else {
            if g.rows.is_empty() && g.session.is_some() {
                lines.push(Line::styled(" nothing changed", c.dim()));
            }
            for (i, r) in g.rows.iter().enumerate() {
                lines.push(match r {
                    Row::Heading(h) => Line::styled(fit(h, w), c.accent()),
                    Row::File { path, code, conflict, staged, .. } => {
                        let style = if i == g.sel && !g.in_diff {
                            c.selected()
                        } else if *conflict {
                            c.danger()
                        } else if *staged {
                            c.added()
                        } else {
                            Style::new()
                        };
                        Line::styled(pad(&format!(" {code} {path}"), w), style)
                    }
                });
            }
            let h = rows.height as usize;
            let top = g.sel.saturating_sub(h.saturating_sub(1));
            f.render_widget(Paragraph::new(lines.into_iter().skip(top).take(h).collect::<Vec<_>>()), rows);
            let keys = if g.in_diff { "[ ] hunk · s stage · u unstage · d discard · Tab back" } else { "s/u/d file · c commit · l log · b branches · f fetch · F pull · p push" };
            f.render_widget(Paragraph::new(Line::styled(fit(keys, w), c.dim())), hint);
        }
        // The diff.
        let block = Block::default().borders(Borders::LEFT).border_style(c.fg(c.divider));
        f.render_widget(block, view);
        let inner = Rect { x: view.x + 1, width: view.width.saturating_sub(1), ..view };
        let iw = inner.width as usize;
        let Some(d) = &mut g.diff else {
            f.render_widget(Paragraph::new(Line::styled("  choose a file to see its changes", c.dim())), inner);
            return;
        };
        let what = match &d.source {
            DiffSource::Unstaged => "not staged",
            DiffSource::Staged => "staged",
            DiffSource::Commit(_) => "in the commit",
        };
        let style = if g.in_diff { Style::new().add_modifier(Modifier::BOLD) } else { c.dim() };
        let mut lines: Vec<Line> = vec![Line::from(vec![Span::styled(format!(" {}", d.path), style), Span::styled(format!(" · {what}"), c.dim())])];
        if let Some(note) = &d.note {
            lines.push(Line::styled(format!("  {note}"), c.dim()));
        }
        let mut body = Vec::new();
        crate::review::hunk_lines(&d.hunks, g.in_diff.then_some(d.hunk), iw, &c, &mut body);
        d.scroll = d.scroll.min(body.len().saturating_sub(1));
        lines.extend(body.into_iter().skip(d.scroll).take(inner.height.saturating_sub(1) as usize));
        f.render_widget(Paragraph::new(lines), inner);
    }
}

/// The line a hunk starts at in the diff's lines.
fn hunk_top(hunks: &[Hunk], n: usize) -> usize {
    hunks.iter().take(n).map(|h| h.lines.len() + 1).sum()
}
