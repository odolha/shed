//! A project tab of another workspace: its folder is on that computer, so
//! what the view reads from the disk for a local folder comes as calls
//! through the nexus instead (`shed_api::calls`: folders, files, quick
//! open, writing, Git) and its changes as topics (the project's, a
//! search's). This is the data level only: the tree, the editors (coloured
//! here, from the text), the Git panel and everything drawn are the
//! project view's own, as for a local folder.
//!
//! Folders are listed as the tree needs them (`Remote::rows` notes those it
//! has not got; the next render asks for them), files read as they open
//! (shown as being read meanwhile), and written whole on saving. Unsaved
//! edits are kept aside under a key naming the workspace
//! (`recovery_key`), never with this machine's own files.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use gpui::{Context, Task};
use shed_api::calls::{self as api, Call, Reply};
use shed_api::topics::{Event, ProjectTopic, SearchTopic, Topic};
use shed_api::{ApiError, ErrorCode};
use shed_client::nexus_api::{RemoteWorkspace, SubEvent, unexpected};
use shed_git::session::Refreshed;
use shed_project::search::{FileHits, Hit, Query};

use super::{Content, Disk, OpenFile, ProjectView, RECENT, Row};
use crate::workspace_api::{self as wa, says};

/// A project tab's link to its workspace, and what was read from it.
pub(super) struct Remote {
    pub ws: RemoteWorkspace,
    /// The tab, as calls name it.
    pub tab: String,
    /// Folders' entries as last listed, by folder.
    folders: HashMap<PathBuf, Vec<api::FolderEntry>>,
    /// Folders being listed now.
    listing: HashSet<PathBuf>,
    /// Folders the tree wanted and had not got, for the next render to ask for.
    missing: RefCell<Vec<PathBuf>>,
    /// Following the project's changes there.
    watch: Option<Task<()>>,
    /// Where the cursor goes in files still being read.
    pending: HashMap<PathBuf, Pending>,
    /// The Git status being read.
    pub git_task: Option<Task<()>>,
    /// Branches as last read (the Git menu lists them without waiting).
    pub branches: Option<api::Branches>,
}

/// Where the cursor goes once a file being read opens, and what is selected.
#[derive(Clone, Copy, Default)]
pub(super) struct Pending {
    pub at: Option<(usize, usize)>,
    /// A character range of line `at.0` (a search hit).
    pub select: Option<(usize, usize)>,
    /// The editor takes the keyboard (it was opened to work in).
    pub focus: bool,
}

impl Remote {
    pub fn new(ws: RemoteWorkspace, tab: String) -> Remote {
        Remote {
            ws,
            tab,
            folders: HashMap::new(),
            listing: HashSet::new(),
            missing: RefCell::new(Vec::new()),
            watch: None,
            pending: HashMap::new(),
            git_task: None,
            branches: None,
        }
    }

    /// A folder's entries as tree rows at `depth`; none yet (and noted, to
    /// be listed) if it has not been listed.
    pub fn rows(&self, dir: &Path, depth: usize) -> Vec<Row> {
        match self.folders.get(dir) {
            Some(entries) => entries
                .iter()
                .map(|e| Row { path: dir.join(&e.name), name: e.name.clone().into(), depth, dir: e.dir, hidden: e.ignored })
                .collect(),
            None => {
                self.missing.borrow_mut().push(dir.to_path_buf());
                Vec::new()
            }
        }
    }

    /// Forgets every folder listed, to list them again.
    pub fn forget_folders(&mut self) {
        self.folders.clear();
    }

    /// Whether `path` is a folder, as listed.
    pub fn is_dir(&self, path: &Path) -> bool {
        path.parent().and_then(|p| self.folders.get(p)).is_some_and(|es| es.iter().any(|e| e.dir && path.file_name().is_some_and(|n| n.to_string_lossy() == e.name)))
    }
}

/// Where a remote file's unsaved edits are kept aside: a key naming the
/// workspace, relative so it is never one of this machine's own paths.
pub(super) fn recovery_key(workspace: &str, path: &Path) -> PathBuf {
    PathBuf::from(format!("remote-{workspace}")).join(path.strip_prefix("/").unwrap_or(path))
}

// ------------------------------------------------------------ conversions

/// A Git status as the panel keeps it: the repository's folder, and the
/// status with what is not pushed. `None` when the folder is not in a
/// repository. (`fallback`: the folder, for a workspace that does not say
/// where its repository is.)
pub(super) fn refreshed(s: &api::GitStatus, fallback: &Path) -> Option<(PathBuf, Refreshed)> {
    if !s.repo {
        return None;
    }
    let letter = |l: &str| l.chars().next().unwrap_or('.');
    let entries = s
        .files
        .iter()
        .map(|f| {
            let untracked = f.unstaged == "?";
            shed_git::Entry {
                path: f.path.clone(),
                orig: f.from.clone(),
                index: if untracked { '.' } else { letter(&f.staged) },
                worktree: if untracked { '?' } else { letter(&f.unstaged) },
                untracked,
                conflict: f.conflict,
                files: 0,
            }
        })
        .collect();
    let status = shed_git::Status {
        branch: shed_git::Branch { head: s.branch.clone(), oid: s.head.clone().unwrap_or_default(), upstream: s.upstream.clone(), ahead: s.ahead, behind: s.behind },
        entries,
        operation: s.operation.clone(),
    };
    let root = s.root.as_ref().map(PathBuf::from).unwrap_or_else(|| fallback.to_path_buf());
    Some((root, Refreshed { status, outgoing: s.outgoing.iter().map(commit_of).collect() }))
}

pub(super) fn commit_of(c: &api::Commit) -> shed_git::Commit {
    shed_git::Commit { hash: c.hash.clone(), short: c.short.clone(), author: c.author.clone(), time: c.time as u64, refs: String::new(), subject: c.subject.clone(), parents: Vec::new() }
}

pub(super) fn source_of(s: &shed_git::session::DiffSource) -> api::DiffSource {
    match s {
        shed_git::session::DiffSource::Unstaged => api::DiffSource::Unstaged,
        shed_git::session::DiffSource::Staged => api::DiffSource::Staged,
        shed_git::session::DiffSource::Commit(h) => api::DiffSource::Commit(h.clone()),
    }
}

/// A search's hits in one file, as the search panel lists them (ranges by
/// byte in the line, from UTF-16 positions).
fn hits_of(found: shed_api::topics::SearchHits) -> FileHits {
    FileHits {
        path: PathBuf::from(found.path),
        hits: found
            .hits
            .into_iter()
            .map(|h| {
                let ranges = h.ranges.chunks(2).filter(|r| r.len() == 2).map(|r| wa::byte_of_utf16(&h.text, r[0])..wa::byte_of_utf16(&h.text, r[1])).collect();
                Hit { line: (h.line as usize).saturating_sub(1), text: h.text, ranges, offset: 0 }
            })
            .collect(),
    }
}

/// A picture sent in Base64, ready to draw.
pub(super) fn picture_of(path: &Path, p: &api::Picture) -> Option<crate::picture::Picture> {
    crate::picture::from_bytes(path, wa::decode(&p.data)?)
}

impl ProjectView {
    /// The workspace and the tab, as calls name them, for another
    /// workspace's project.
    pub(super) fn remote_call(&self) -> Option<(RemoteWorkspace, String)> {
        self.remote.as_ref().map(|r| (r.ws.clone(), r.tab.clone()))
    }

    /// `path` as calls name it: relative to the project's folder.
    fn rel(&self, path: &Path) -> String {
        wa::rel(&self.root, path).unwrap_or_default()
    }

    /// Lists the folders the tree wanted and had not got.
    pub(super) fn list_missing(&mut self, cx: &mut Context<Self>) {
        let Some(r) = &self.remote else { return };
        let missing: Vec<PathBuf> = r.missing.borrow_mut().drain(..).collect();
        for dir in missing {
            self.list_folder(dir, cx);
        }
    }

    /// Lists a folder (again), and rebuilds the tree with it.
    fn list_folder(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        let rel = self.rel(&dir);
        let Some(r) = &mut self.remote else { return };
        if !dir.starts_with(&self.root) || !r.listing.insert(dir.clone()) {
            return;
        }
        let reply = r.ws.call(Call::ListFolder(api::ProjectPath { tab: r.tab.clone(), path: rel }));
        cx.spawn(async move |this, cx| {
            let reply = reply.await;
            this.update(cx, |this, cx| {
                let root = this.root.clone();
                let Some(r) = &mut this.remote else { return };
                r.listing.remove(&dir);
                let entries = match reply {
                    Ok(Reply::Folder(entries)) => entries,
                    // Gone: its folder in the tree closes.
                    Err(e) if e.code == ErrorCode::NotFound && dir != root => {
                        this.expanded.remove(&dir);
                        Vec::new()
                    }
                    other => {
                        let e = other.map_or_else(|e| says(&e), |r| says(&unexpected(&r)));
                        this.notice = Some(format!("Cannot list {}: {e}", if dir == root { "the project's folder".to_string() } else { this_rel(&root, &dir) }));
                        Vec::new()
                    }
                };
                if let Some(r) = &mut this.remote {
                    r.folders.insert(dir, entries);
                }
                this.rebuild();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Starts following the project there (its files and Git changing),
    /// listing its folder again: in place of walking and watching the disk.
    pub(super) fn remote_start(&mut self, cx: &mut Context<Self>) {
        let Some(r) = &mut self.remote else { return };
        r.forget_folders();
        let sub = r.ws.subscribe(Topic::Project(ProjectTopic { tab: r.tab.clone() }));
        r.watch = Some(cx.spawn(async move |this, cx| {
            while let Some(event) = sub.next().await {
                let alive = this.update(cx, |this, cx| match event {
                    SubEvent::Event(Event::FilesChanged(f)) => this.on_remote_change(f.paths, cx),
                    SubEvent::Event(Event::GitChanged) => this.git_refresh(cx),
                    SubEvent::Event(_) => {}
                    SubEvent::Ended(e) => {
                        let why = e.map(|e| says(&e)).unwrap_or_else(|| "it ended".into());
                        this.notice = Some(format!("No longer following changes on the workspace ({why}): use Refresh to follow again."));
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        }));
        self.rebuild();
        self.list_missing(cx);
        cx.notify();
    }

    /// Paths changed on the workspace (relative to the folder): the tree,
    /// open files and Git follow.
    fn on_remote_change(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        self.git_refresh(cx);
        let now = Instant::now();
        let changed: Vec<PathBuf> = paths.iter().map(|p| self.root.join(p)).collect();
        let mut dirs = HashSet::new();
        if let Some(r) = &self.remote {
            for path in &changed {
                if let Some(parent) = path.parent().filter(|p| r.folders.contains_key(*p)) {
                    dirs.insert(parent.to_path_buf());
                }
                if r.folders.contains_key(path) {
                    dirs.insert(path.clone());
                }
            }
        }
        for path in &changed {
            self.recent.insert(path.clone(), now);
        }
        self.recent.retain(|_, at| at.elapsed() < RECENT);
        for dir in dirs {
            self.list_folder(dir, cx);
        }
        for i in 0..self.open.len() {
            if changed.contains(&self.open[i].path) && !matches!(self.open[i].content, Content::Diff(_)) {
                self.follow_disk(i, cx);
            }
        }
        cx.notify();
    }

    /// Reads an open file again from the workspace, as `follow_disk` does
    /// from the disk.
    pub(super) fn remote_follow(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(file) = self.open.get(i) else { return };
        if matches!(file.content, Content::Diff(_) | Content::Loading) {
            return;
        }
        let path = file.path.clone();
        let Some((ws, tab)) = self.remote_call() else { return };
        let reply = ws.call(Call::ReadFile(api::ProjectPath { tab, path: self.rel(&path) }));
        cx.spawn(async move |this, cx| {
            let reply = reply.await;
            this.update(cx, |this, cx| {
                let Some(i) = this.open.iter().position(|f| f.path == path && !matches!(f.content, Content::Diff(_))) else { return };
                match reply {
                    Err(e) if e.code == ErrorCode::NotFound => this.open[i].disk = Disk::Deleted,
                    Ok(Reply::File(f)) => match &mut this.open[i].content {
                        Content::Picture(p, _) => {
                            *p = f.picture.as_ref().and_then(|p| picture_of(&path, p));
                            this.open[i].disk = Disk::Same;
                        }
                        Content::Text(_) if f.kind == api::FileKind::Text => this.follow_text(i, f.text, cx),
                        _ => {}
                    },
                    _ => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// An open file, being read from the workspace (`Content::Loading`
    /// until it comes); `at` and `select` apply once it is there.
    pub(super) fn remote_load(&mut self, path: PathBuf, cx: &mut Context<Self>) -> OpenFile {
        let lang = shed_project::detect(&path);
        let file = OpenFile { path: path.clone(), content: Content::Loading, md: None, lang, disk: Disk::Same, restored: false, highlight: None, recovery: None, _sub: None };
        if let Some((ws, tab)) = self.remote_call() {
            let reply = ws.call(Call::ReadFile(api::ProjectPath { tab, path: self.rel(&path) }));
            cx.spawn(async move |this, cx| {
                let reply = reply.await;
                this.update(cx, |this, cx| this.remote_loaded(path, reply, cx)).ok();
            })
            .detach();
        }
        file
    }

    /// A file read from the workspace: into its editor (or picture, or a note).
    fn remote_loaded(&mut self, path: PathBuf, reply: Result<Reply, ApiError>, cx: &mut Context<Self>) {
        let Some(i) = self.open.iter().position(|f| f.path == path && matches!(f.content, Content::Loading)) else { return };
        let name = self.open[i].name();
        let content = match reply {
            Ok(Reply::File(f)) => match f.kind {
                api::FileKind::Text => {
                    let mut file = std::mem::replace(&mut self.open[i], OpenFile { path: path.clone(), content: Content::Loading, md: None, lang: None, disk: Disk::Same, restored: false, highlight: None, recovery: None, _sub: None });
                    self.edit_text(&mut file, &f.text, cx);
                    self.open[i] = file;
                    self.rehighlight(i, cx);
                    let pending = self.remote.as_mut().and_then(|r| r.pending.remove(&path)).unwrap_or_default();
                    // A place in it is shown in the editor.
                    if pending.at.is_some() {
                        self.set_md_viewing(i, false, cx);
                    }
                    if let Some(view) = self.open[i].view().cloned() {
                        view.update(cx, |v, cx| {
                            if let Some((line, col)) = pending.at {
                                v.go_to(line, col, cx);
                            }
                            if let (Some((line, _)), Some((a, b))) = (pending.at, pending.select) {
                                let start = v.buffer.line_start(line);
                                v.select(start + a..start + b, cx);
                            }
                        });
                    }
                    if pending.focus && self.active == Some(i) {
                        self.focus_editor = true;
                    }
                    cx.notify();
                    return;
                }
                api::FileKind::Picture => Content::Picture(f.picture.as_ref().and_then(|p| picture_of(&path, p)), crate::picture::Zoom::Fit),
                api::FileKind::Binary => Content::Binary,
                api::FileKind::TooLarge => Content::TooLarge(f.size as u64),
            },
            Ok(other) => Content::Unreadable(format!("{name} could not be read: {}", says(&unexpected(&other)))),
            Err(e) => Content::Unreadable(format!("{name} could not be read: {}", says(&e))),
        };
        self.open[i].content = content;
        cx.notify();
    }

    /// Where the cursor goes when `path` (being read) opens.
    pub(super) fn remote_pending(&mut self, path: &Path, pending: Pending) {
        if let Some(r) = &mut self.remote {
            r.pending.insert(path.to_path_buf(), pending);
        }
    }

    /// Writes an open file to the workspace; marked saved once written, if
    /// it was not changed meanwhile.
    pub(super) fn remote_save(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(file) = self.open.get(i) else { return };
        let Some(view) = file.view().cloned() else { return };
        let (path, name) = (file.path.clone(), file.name());
        let Some((ws, tab)) = self.remote_call() else { return };
        let text = view.read(cx).buffer.text();
        let key = self.recovery_key(&path);
        let reply = ws.call(Call::WriteFile(api::WriteFile { tab, path: self.rel(&path), text: text.clone() }));
        cx.spawn(async move |this, cx| {
            let reply = reply.await;
            this.update(cx, |this, cx| {
                match reply {
                    Ok(_) => {
                        if view.read(cx).buffer.text() == text {
                            view.update(cx, |v, cx| {
                                v.buffer.mark_saved();
                                cx.notify();
                            });
                            shed_project::recovery::remove(&key);
                        }
                        if let Some(f) = this.open.iter_mut().find(|f| f.path == path && f.view() == Some(&view)) {
                            f.disk = Disk::Same;
                            f.restored = false;
                        }
                        this.notice = None;
                    }
                    Err(e) => this.notice = Some(format!("Cannot save {name}: {}", says(&e))),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Takes the file as it is on the workspace, dropping the edits here.
    pub(super) fn remote_take_disk(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(file) = self.open.get_mut(i) else { return };
        let Some(view) = file.view().cloned() else { return };
        file.disk = Disk::Same;
        file.restored = false;
        let path = file.path.clone();
        shed_project::recovery::remove(&self.recovery_key(&path));
        let Some((ws, tab)) = self.remote_call() else { return };
        let reply = ws.call(Call::ReadFile(api::ProjectPath { tab, path: self.rel(&path) }));
        cx.spawn(async move |_, cx| {
            if let Ok(Reply::File(f)) = reply.await
                && f.kind == api::FileKind::Text
            {
                view.update(cx, |v, cx| v.reload(&f.text, cx));
            }
        })
        .detach();
        cx.notify();
    }

    /// Files whose names match what is typed in quick open, from the
    /// workspace (its file list is there).
    pub(super) fn remote_quick(&mut self, query: String, cx: &mut Context<Self>) {
        let Some((ws, tab)) = self.remote_call() else { return };
        let reply = ws.call(Call::QuickOpen(api::QuickOpen { tab, query: query.clone(), limit: 14 }));
        let task = cx.spawn(async move |this, cx| {
            let reply = reply.await;
            this.update(cx, |this, cx| {
                let Some(quick) = &mut this.quick else { return };
                if quick.editor.read(cx).text() != query {
                    return;
                }
                quick.results = match reply {
                    Ok(Reply::Files(found)) => found.into_iter().map(|m| (PathBuf::from(&m.path), m.hits.iter().map(|&u| wa::char_of_utf16(&m.path, u) as u32).collect())).collect(),
                    _ => Vec::new(),
                };
                quick.selected = 0;
                cx.notify();
            })
            .ok();
        });
        if let Some(quick) = &mut self.quick {
            quick.task = Some(task);
        }
    }

    /// Searches the project's files on the workspace; hits arrive file by
    /// file, as a local search's do.
    pub(super) fn remote_search(&mut self, q: Query, cx: &mut Context<Self>) -> Option<Task<()>> {
        let (ws, tab) = self.remote_call()?;
        Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(120)).await;
            let topic = Topic::Search(SearchTopic { tab, query: q.text.clone(), regex: q.regex, case_sensitive: q.case_sensitive, whole_word: q.whole_word, include: None, exclude: None });
            let sub = ws.subscribe(topic);
            while let Some(event) = sub.next().await {
                let done = match event {
                    SubEvent::Event(Event::SearchHits(found)) => {
                        let found = hits_of(found);
                        let alive = this.update(cx, |this, cx| {
                            let Some(search) = &mut this.search else { return };
                            let at = search.results.partition_point(|r| r.path < found.path);
                            search.results.insert(at, found);
                            search.rebuild_rows();
                            cx.notify();
                        });
                        if alive.is_err() {
                            return;
                        }
                        continue;
                    }
                    SubEvent::Event(Event::SearchDone(d)) => d.error,
                    SubEvent::Event(_) => continue,
                    SubEvent::Ended(e) => e.map(|e| says(&e)),
                };
                this.update(cx, |this, cx| {
                    if let Some(search) = &mut this.search {
                        search.running = false;
                        search.error = done;
                        cx.notify();
                    }
                })
                .ok();
                return;
            }
        }))
    }

    /// Replaces every match in the files found, on the workspace: each is
    /// read, changed and written whole. Files open here with unsaved edits
    /// are left alone (`skip`).
    pub(super) fn remote_replace_all(&mut self, files: Vec<PathBuf>, q: Query, with: String, cx: &mut Context<Self>) {
        let Some((ws, tab)) = self.remote_call() else { return };
        let skipped: Vec<String> = files
            .iter()
            .filter_map(|p| self.open.iter().position(|f| &f.path == p))
            .filter(|i| self.is_dirty(*i, cx))
            .map(|i| self.open[i].name())
            .collect();
        let todo: Vec<String> = files.iter().filter(|p| !self.open.iter().position(|f| &f.path == *p).is_some_and(|i| self.is_dirty(i, cx))).map(|p| self.rel(p)).collect();
        cx.spawn(async move |this, cx| {
            let mut failed = Vec::new();
            for path in todo {
                let text = match ws.call(Call::ReadFile(api::ProjectPath { tab: tab.clone(), path: path.clone() })).await {
                    Ok(Reply::File(f)) if f.kind == api::FileKind::Text => f.text,
                    _ => continue,
                };
                if let Ok((new, n)) = shed_project::search::replace(&text, &q, &with)
                    && n > 0
                    && ws.call(Call::WriteFile(api::WriteFile { tab: tab.clone(), path: path.clone(), text: new })).await.is_err()
                {
                    failed.push(path);
                }
            }
            this.update(cx, |this, cx| {
                let mut notes = Vec::new();
                if !skipped.is_empty() {
                    notes.push(format!("Not replaced in {} (unsaved changes)", skipped.join(", ")));
                }
                if !failed.is_empty() {
                    notes.push(format!("Could not write {}", failed.join(", ")));
                }
                this.notice = (!notes.is_empty()).then(|| notes.join("; "));
                if let Some(s) = &mut this.search {
                    s.searched = None;
                }
                this.schedule_search(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Makes a file (empty) or folder on the workspace, then shows it.
    pub(super) fn remote_create(&mut self, path: PathBuf, name: String, dir: bool, cx: &mut Context<Self>) {
        let Some((ws, tab)) = self.remote_call() else { return };
        let mut rel = self.rel(&path);
        if dir {
            rel.push('/');
        }
        let reply = ws.call(Call::CreateEntry(api::ProjectPath { tab, path: rel }));
        cx.spawn(async move |this, cx| {
            let reply = reply.await;
            this.update(cx, |this, cx| {
                if let Err(e) = reply {
                    this.notice = Some(format!("Could not make {name}: {}", says(&e)));
                    cx.notify();
                    return;
                }
                // The folders on the way are listed again as the tree opens them.
                for dir in path.ancestors().skip(1).take_while(|d| d.starts_with(&this.root)) {
                    if let Some(r) = &mut this.remote {
                        r.folders.remove(dir);
                    }
                }
                if dir {
                    this.expanded.insert(path.clone());
                    this.reveal(&path, cx);
                } else {
                    this.open_path(path.clone(), None, cx);
                }
            })
            .ok();
        })
        .detach();
    }
}

/// A folder in a note: relative to the project.
fn this_rel(root: &Path, dir: &Path) -> String {
    dir.strip_prefix(root).unwrap_or(dir).display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_from_the_workspace_reads_as_gits() {
        let s = api::GitStatus {
            repo: true,
            branch: Some("main".into()),
            upstream: Some("origin/main".into()),
            ahead: 1,
            behind: 2,
            operation: None,
            files: vec![
                api::GitFile { path: "src/a.rs".into(), staged: "M".into(), unstaged: String::new(), conflict: false, from: None },
                api::GitFile { path: "new.txt".into(), staged: String::new(), unstaged: "?".into(), conflict: false, from: None },
            ],
            outgoing: vec![api::Commit { hash: "abc123".into(), short: "abc".into(), subject: "x".into(), author: "me".into(), time: 5 }],
            root: Some("/srv/repo".into()),
            head: Some("abc12345".into()),
        };
        let (root, r) = refreshed(&s, Path::new("/srv/repo/sub")).unwrap();
        assert_eq!(root, PathBuf::from("/srv/repo"));
        assert_eq!(r.status.branch.oid, "abc12345");
        assert_eq!(r.status.staged().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["src/a.rs"]);
        assert_eq!(r.status.unstaged().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["new.txt"]);
        assert!(r.status.entries[1].untracked);
        assert_eq!(r.outgoing[0].hash, "abc123");
        assert!(refreshed(&api::GitStatus::default(), Path::new("/x")).is_none());
        // An older workspace that does not say: the folder.
        let (root, _) = refreshed(&api::GitStatus { root: None, ..s }, Path::new("/srv/repo/sub")).unwrap();
        assert_eq!(root, PathBuf::from("/srv/repo/sub"));
    }

    #[test]
    fn hunks_keep_their_place_and_header() {
        let hunks = vec![api::Hunk {
            header: "@@ -3,2 +3,3 @@".into(),
            lines: vec![
                api::DiffLine { kind: api::DiffLineKind::Context, text: "a".into(), old: Some(3), new: Some(3) },
                api::DiffLine { kind: api::DiffLineKind::Added, text: "b".into(), old: None, new: Some(4) },
            ],
        }];
        let h = wa::hunks_of(&hunks);
        assert_eq!(h[0].header(), "@@ -3,2 +3,3 @@");
        assert_eq!(h[0].lines[1].kind, shed_git::LineKind::Added);
        assert_eq!(wa::parse_header("@@ -0,0 +1 @@"), Some((0, 0, 1, 1)));
    }

    #[test]
    fn search_hits_by_byte() {
        let found = shed_api::topics::SearchHits {
            path: "src/é.rs".into(),
            hits: vec![shed_api::topics::SearchHit { line: 3, text: "let 🦀 = crab;".into(), ranges: vec![9, 13] }],
        };
        let f = hits_of(found);
        assert_eq!(f.path, PathBuf::from("src/é.rs"));
        let h = &f.hits[0];
        assert_eq!(h.line, 2);
        assert_eq!(&h.text[h.ranges[0].clone()], "crab");
    }

    #[test]
    fn remote_edits_are_kept_apart_from_this_machines_files() {
        let key = recovery_key("w1", Path::new("/home/me/a.rs"));
        assert!(key.is_relative());
        assert_eq!(key, PathBuf::from("remote-w1/home/me/a.rs"));
        assert_ne!(recovery_key("w2", Path::new("/home/me/a.rs")), key);
    }
}
