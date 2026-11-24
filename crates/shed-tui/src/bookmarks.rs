//! The bench's bookmarks in a terminal: listed (`C-b b`), gone to, renamed
//! and removed; added from a project tab (`m`) and an AI tab (`Alt+B`).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::text::Line;
use shed_model::bookmarks::{Group, grouped};
use shed_model::{Bookmark, BookmarkKind, TabId};
use shed_proto::ClientMsg;

use crate::app::App;
use crate::overlay::Item;

/// A bookmark being gone to waits this long at most for its tab (a
/// project tab opening, a session loading).
const WAIT: Duration = Duration::from_secs(15);

impl App {
    /// The bench's bookmarks, as a menu: choosing one goes to it.
    pub(crate) fn bookmarks_menu(&mut self) {
        let Some(bench) = self.current_bench() else { return };
        let folder = bench.cwd.clone();
        let marks: Vec<Bookmark> = grouped(&bench.bookmarks).into_iter().flat_map(|(_, m)| m.into_iter().cloned()).collect();
        if marks.is_empty() {
            let lines = [
                "No bookmarks yet.",
                "",
                "In a project tab, m bookmarks the file or folder chosen in the",
                "list, or the file shown at the line at the top of the view.",
                "In an AI tab, Alt+B bookmarks the last message in view.",
            ];
            return self.info("Bookmarks", lines.iter().map(|l| Line::raw(l.to_string())).collect());
        }
        let mut items: Vec<Item> = marks.iter().map(|m| Item::new(format!("{} {}", glyph(m.group()), m.label)).hint(m.detail(&folder))).collect();
        items.push(Item::new("Rename a Bookmark…"));
        items.push(Item::new("Remove a Bookmark…").danger());
        let n = marks.len();
        self.menu("Bookmarks", items, move |app, i| match i {
            i if i < n => app.go_to_bookmark(marks[i].clone()),
            i if i == n => app.pick_bookmark("Rename Which?", marks, |app, mark| {
                let id = mark.id;
                app.ask("Rename Bookmark", mark.label.clone(), move |app, label| {
                    if let (Some(bench), false) = (app.bench, label.trim().is_empty()) {
                        app.send(ClientMsg::RenameBookmark { bench, id, label });
                    }
                });
            }),
            _ => app.pick_bookmark("Remove Which?", marks, |app, mark| {
                if let Some(bench) = app.bench {
                    app.send(ClientMsg::RemoveBookmark { bench, id: mark.id });
                }
            }),
        });
    }

    fn pick_bookmark(&mut self, title: &str, marks: Vec<Bookmark>, then: impl FnOnce(&mut App, Bookmark) + 'static) {
        let items = marks.iter().map(|m| Item::new(format!("{} {}", glyph(m.group()), m.label))).collect();
        self.menu(title, items, move |app, i| then(app, marks[i].clone()));
    }

    /// Adds a bookmark to the bench shown.
    pub(crate) fn add_bookmark(&mut self, bookmark: Bookmark) {
        let Some(bench) = self.bench else { return };
        self.note(format!("bookmarked {} (C-b b lists them)", bookmark.label));
        self.send(ClientMsg::AddBookmark { bench, bookmark });
    }

    /// Goes to a bookmark: its tab shown (a project tab opened for a file
    /// or folder no project tab holds), then the place in it, once there.
    pub(crate) fn go_to_bookmark(&mut self, mark: Bookmark) {
        let Some(bench) = self.current_bench() else { return };
        let bench_id = bench.id;
        match &mark.kind {
            BookmarkKind::File { path, .. } | BookmarkKind::Folder { path } => {
                if !path.exists() {
                    return self.error(format!("{} is not there any more", path.display()));
                }
                match self.project_tab_holding(path) {
                    Some((tab, _)) => self.send(ClientMsg::FocusTab { tab }),
                    None => {
                        let folder = if matches!(mark.kind, BookmarkKind::File { .. }) { path.parent().unwrap_or(path) } else { path };
                        self.send(ClientMsg::NewProjectTab { bench: bench_id, root: shed_project::project_root(folder) });
                    }
                }
            }
            BookmarkKind::AiMessage { tab, session, .. } => {
                let Some(ai) = bench.tabs.iter().find(|t| t.id == *tab).and_then(|t| t.ai_info()) else { return self.error("its AI tab is closed") };
                if session.is_some() && ai.session != *session {
                    self.send(ClientMsg::AiOpenSession { tab: *tab, session: session.clone() });
                }
                self.send(ClientMsg::FocusTab { tab: *tab });
            }
        }
        self.pending_bookmark = Some((mark, Instant::now()));
        self.settle_bookmark();
    }

    /// The project tab of the bench shown whose folder holds `path`.
    fn project_tab_holding(&self, path: &std::path::Path) -> Option<(TabId, PathBuf)> {
        let bench = self.current_bench()?;
        let roots: Vec<(TabId, PathBuf)> = bench.tabs.iter().filter_map(|t| Some((t.id, t.project_info()?.root.clone()))).collect();
        shed_project::project_holding(path, &roots)
    }

    /// Goes to the place of the bookmark being gone to, once its tab is
    /// shown (and its conversation here): after each change of the tree
    /// and of a conversation.
    pub(crate) fn settle_bookmark(&mut self) {
        let Some((mark, since)) = self.pending_bookmark.clone() else { return };
        if since.elapsed() > WAIT {
            self.pending_bookmark = None;
            return;
        }
        let active = self.active_tab().map(|t| t.id);
        match &mark.kind {
            BookmarkKind::File { path, line } => {
                let Some((tab, path)) = self.project_tab_holding(path) else { return };
                if active != Some(tab) || !self.projects.contains_key(&tab) {
                    return;
                }
                self.pending_bookmark = None;
                self.reveal_in_tree(tab, &path, false);
                if let Some(v) = self.projects.get_mut(&tab) {
                    v.right = true;
                }
                self.open_file(tab, path, line.map(|l| l.saturating_sub(1) as usize), None);
            }
            BookmarkKind::Folder { path } => {
                let Some((tab, path)) = self.project_tab_holding(path) else { return };
                if active != Some(tab) || !self.projects.contains_key(&tab) {
                    return;
                }
                self.pending_bookmark = None;
                self.reveal_in_tree(tab, &path, true);
            }
            BookmarkKind::AiMessage { tab, session, uuid, role, excerpt } => {
                if active != Some(*tab) {
                    return;
                }
                let Some(view) = self.ais.get_mut(tab) else { return };
                // Another session's entries still: the bookmark's is coming.
                if session.is_some() && view.status.session != *session {
                    return;
                }
                let found = shed_ai::find_message(&view.entries, uuid.as_deref(), *role, excerpt);
                if found.is_none() && view.entries.is_empty() {
                    return;
                }
                self.pending_bookmark = None;
                match found {
                    Some(i) => view.jump = Some(i),
                    None => self.error("the message is not in its conversation any more"),
                }
            }
        }
    }

    /// Opens the folders down to `path` in a project tab's list and selects
    /// it; with `open`, it is opened too (a folder).
    fn reveal_in_tree(&mut self, tab: TabId, path: &std::path::Path, open: bool) {
        let Some(v) = self.projects.get_mut(&tab) else { return };
        for dir in path.ancestors().skip(1) {
            if dir == v.root || !dir.starts_with(&v.root) {
                break;
            }
            v.expanded.insert(dir.to_path_buf());
        }
        if open && path != v.root {
            v.expanded.insert(path.to_path_buf());
        }
        v.side = crate::project::Side::Files;
        v.right = false;
        v.rebuild_rows();
        if let Some(i) = v.rows.iter().position(|r| r.path == path) {
            v.sel = i;
        }
    }

    /// Alt+B in an AI tab: the last prompt or answer in view.
    pub(crate) fn bookmark_ai_message(&mut self, tab: TabId) {
        let Some(view) = self.ais.get(&tab) else { return };
        let Some(i) = view.last_message_in_view() else { return self.note("no message in view to bookmark") };
        let Some((text, role, uuid)) = view.entries.get(i).and_then(|e| e.message()) else { return };
        let session = view.status.session.clone().or_else(|| self.tree.tab(tab).and_then(|t| t.ai_info()).and_then(|a| a.session.clone()));
        let mark = Bookmark::message(tab, session, Some(uuid.to_string()), role, text);
        self.add_bookmark(mark);
    }
}

fn glyph(group: Group) -> &'static str {
    match group {
        Group::Files => "▤",
        Group::Folders => "▸",
        Group::Messages => "✦",
    }
}
