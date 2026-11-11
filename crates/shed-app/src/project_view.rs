//! A project tab: a folder of code. A header names the folder; on the left,
//! its files as a tree; on the right, the files open in it, each in an
//! editor. Everything follows the disk: files changed by the AI, Git or a
//! build show at once, in the tree and in the editor.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui::{
    ClipboardItem, Context, Entity, FocusHandle, Focusable, HighlightStyle, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, PromptLevel, ScrollStrategy, SharedString, StyledText, Subscription, Task, UniformListScrollHandle,
    Window, div, img, prelude::*, px, svg, uniform_list,
};
use shed_model::{BenchId, Project, Rgb, TabId};
use shed_project::search::{FileHits, Query};
use shed_project::{Excluder, Lang, Style, Watch};
use shed_proto::ClientMsg;

use crate::assets::icon;
use crate::keeper_conn::KeeperConn;
use crate::line_editor::LineEditor;
use crate::menu::{self, Menu, MenuEntry, MenuHost};
use crate::settings::Settings;
use crate::text_view::{LineSpans, TextEvent, TextOptions, TextView};
use crate::theme::{UiColors, current_ui, hsla, hsla_alpha};

const ROW_HEIGHT: f32 = 24.0;
const INDENT: f32 = 14.0;
const TREE_WIDTH: f32 = 280.0;
/// Search stops after this many matching lines.
const SEARCH_LIMIT: usize = 5000;
/// Files this big open as a note, not in the editor.
const MAX_OPEN: u64 = 8 * 1024 * 1024;
/// Files this big are not coloured.
const MAX_HIGHLIGHT: usize = 1024 * 1024;
/// How long a changed file keeps its mark in the tree.
const RECENT: Duration = Duration::from_secs(5 * 60);
/// Colouring waits for typing to pause this long.
const HIGHLIGHT_DELAY: Duration = Duration::from_millis(60);
/// Unsaved edits are written aside this long after the last keystroke.
const RECOVERY_DELAY: Duration = Duration::from_millis(1500);

/// A line of the file tree.
struct Row {
    path: PathBuf,
    name: SharedString,
    depth: usize,
    dir: bool,
    hidden: bool,
}

enum Content {
    Text(Entity<TextView>),
    Image,
    Binary,
    TooLarge(u64),
}

/// How a file open in the editor stands against the disk.
#[derive(Clone, Copy, PartialEq)]
enum Disk {
    Same,
    /// Changed on disk while it had unsaved edits here.
    Changed,
    Deleted,
}

struct OpenFile {
    path: PathBuf,
    content: Content,
    lang: Option<&'static Lang>,
    disk: Disk,
    /// Unsaved edits were brought back from a recovery file.
    restored: bool,
    highlight: Option<Task<()>>,
    recovery: Option<Task<()>>,
    _sub: Option<Subscription>,
}

impl OpenFile {
    fn view(&self) -> Option<&Entity<TextView>> {
        match &self.content {
            Content::Text(v) => Some(v),
            _ => None,
        }
    }

    fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }
}

/// What the left panel shows.
#[derive(Clone, Copy, PartialEq)]
enum Panel {
    Files,
    Search,
}

/// Search in files (Ctrl+Shift+F).
struct SearchPanel {
    query: Entity<LineEditor>,
    replace: Entity<LineEditor>,
    regex: bool,
    case_sensitive: bool,
    whole_word: bool,
    /// Files with matches, by path.
    results: Vec<FileHits>,
    /// What the list shows: a file, or one of its hits.
    rows: Vec<(usize, Option<usize>)>,
    /// What `results` are for.
    searched: Option<Query>,
    cancel: Arc<AtomicBool>,
    running: bool,
    error: Option<String>,
    task: Option<Task<()>>,
    scroll: UniformListScrollHandle,
    _subs: Vec<Subscription>,
}

impl SearchPanel {
    fn query(&self, cx: &gpui::App) -> Query {
        Query {
            text: self.query.read(cx).text().to_string(),
            regex: self.regex,
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
        }
    }

    fn rebuild_rows(&mut self) {
        self.rows = self.results.iter().enumerate().flat_map(|(f, r)| std::iter::once((f, None)).chain((0..r.hits.len()).map(move |h| (f, Some(h))))).collect();
    }
}

/// Open by name (Ctrl+P).
struct QuickOpen {
    editor: Entity<LineEditor>,
    /// Indices into the file list, with the matched chars of each path.
    results: Vec<(usize, Vec<u32>)>,
    selected: usize,
    _sub: Subscription,
}

pub struct ProjectView {
    pub bench: BenchId,
    pub tab: TabId,
    pub root: PathBuf,
    pub accent: Rgb,
    exclude: Vec<String>,
    conn: KeeperConn,
    settings: Settings,
    rows: Vec<Row>,
    /// Open folders, kept as the tree is rebuilt.
    expanded: HashSet<PathBuf>,
    selected: Option<PathBuf>,
    tree_focus: FocusHandle,
    scroll: UniformListScrollHandle,
    /// Every file not ignored, relative to the root, sorted.
    files: Arc<Vec<PathBuf>>,
    walking: Option<Task<()>>,
    watch: Option<Watch>,
    /// Folders could not all be watched; the tree catches up on Refresh.
    limited: bool,
    /// Paths changed lately, for their mark in the tree.
    recent: HashMap<PathBuf, Instant>,
    open: Vec<OpenFile>,
    active: Option<usize>,
    quick: Option<QuickOpen>,
    menu: Option<Menu<Self>>,
    notice: Option<String>,
    /// Give the active editor the keyboard at the next render.
    focus_editor: bool,
    panel: Panel,
    search: Option<SearchPanel>,
    tree_width: f32,
    /// The left panel's edge being dragged: where the mouse and the width started.
    resizing: Option<(f32, f32)>,
}

impl ProjectView {
    pub fn new(bench: BenchId, tab: TabId, project: &Project, accent: Rgb, conn: KeeperConn, settings: Settings, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            bench,
            tab,
            root: project.root.clone(),
            accent,
            exclude: project.exclude.clone(),
            conn,
            settings,
            rows: Vec::new(),
            expanded: HashSet::new(),
            selected: None,
            tree_focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            files: Arc::default(),
            walking: None,
            watch: None,
            limited: false,
            recent: HashMap::new(),
            open: Vec::new(),
            active: None,
            quick: None,
            menu: None,
            notice: None,
            focus_editor: false,
            panel: Panel::Files,
            search: None,
            tree_width: TREE_WIDTH,
            resizing: None,
        };
        view.rebuild();
        view.start_walk(cx);
        for path in &project.open {
            if path.is_file() {
                view.open_file(path.clone(), None, false, cx);
            }
        }
        view.active = project.active.filter(|i| *i < view.open.len()).or((!view.open.is_empty()).then_some(0));
        view
    }

    /// Where the keyboard goes when the tab is shown: the open file, else the tree.
    pub fn focus_target(&self, cx: &gpui::App) -> FocusHandle {
        match self.active.and_then(|i| self.open.get(i)).and_then(OpenFile::view) {
            Some(view) => view.read(cx).focus_handle().clone(),
            None => self.tree_focus.clone(),
        }
    }

    /// Takes new colours and font from a reloaded config.
    pub fn apply(&mut self, settings: &Settings, cx: &mut Context<Self>) {
        self.settings = settings.clone();
        for i in 0..self.open.len() {
            if let Some(view) = self.open[i].view().cloned() {
                view.update(cx, |v, cx| v.apply(settings, cx));
            }
            self.rehighlight(i, cx);
        }
        cx.notify();
    }

    /// Follows the tab's project as the keeper has it: another folder, or
    /// other excludes. (Open files are this view's to change.)
    pub fn set_project(&mut self, project: &Project, cx: &mut Context<Self>) {
        if project.root != self.root {
            self.root = project.root.clone();
            self.expanded.clear();
            self.selected = None;
            self.open.clear();
            self.active = None;
            self.rebuild();
            self.start_walk(cx);
        }
        if project.exclude != self.exclude {
            self.exclude = project.exclude.clone();
            self.start_walk(cx);
        }
    }

    /// Tells the keeper what is open, to be saved with the bench.
    fn publish(&self) {
        let project = Project {
            root: self.root.clone(),
            open: self.open.iter().map(|f| f.path.clone()).collect(),
            active: self.active,
            exclude: self.exclude.clone(),
        };
        self.conn.send_msg(ClientMsg::SetProject { tab: self.tab, project });
    }

    /// Lists the project's files and starts watching its folders, away
    /// from the UI thread.
    fn start_walk(&mut self, cx: &mut Context<Self>) {
        let (root, exclude) = (self.root.clone(), self.exclude.clone());
        self.watch = None;
        self.walking = Some(cx.spawn(async move |this, cx| {
            let (tx, rx) = async_channel::unbounded::<Vec<PathBuf>>();
            let walked = cx
                .background_executor()
                .spawn(async move {
                    let walked = shed_project::walk(&root, &exclude);
                    let excluder = Excluder::new(&root, &exclude);
                    let watch = Watch::new(&walked.dirs, excluder, move |batch| {
                        let _ = tx.send_blocking(batch);
                    });
                    (walked, watch)
                })
                .await;
            let (walked, watch) = walked;
            let alive = this
                .update(cx, |this, cx| {
                    this.files = Arc::new(walked.files);
                    match watch {
                        Ok(w) => {
                            this.limited = w.limited;
                            this.watch = Some(w);
                        }
                        Err(e) => {
                            tracing::warn!("cannot watch {}: {e}", this.root.display());
                            this.limited = true;
                        }
                    }
                    this.refresh_quick(cx);
                    cx.notify();
                })
                .is_ok();
            if !alive {
                return;
            }
            while let Ok(batch) = rx.recv().await {
                if this.update(cx, |this, cx| this.on_disk_change(batch, cx)).is_err() {
                    break;
                }
            }
        }));
    }

    /// Paths changed on disk: the file list, the tree and open files follow.
    fn on_disk_change(&mut self, batch: Vec<PathBuf>, cx: &mut Context<Self>) {
        let excluder = Excluder::new(&self.root, &self.exclude);
        let now = Instant::now();
        let mut files = (*self.files).clone();
        let mut tree_changed = false;
        for path in &batch {
            if excluder.excludes(path, path.is_dir()) {
                continue;
            }
            self.recent.insert(path.clone(), now);
            let Ok(rel) = path.strip_prefix(&self.root) else { continue };
            if path.is_file() {
                if let Err(at) = files.binary_search_by(|f| f.as_path().cmp(rel)) {
                    files.insert(at, rel.to_path_buf());
                }
            } else if !path.exists() {
                files.retain(|f| !f.starts_with(rel));
            }
            let parent = path.parent().unwrap_or(&self.root);
            if parent == self.root || self.expanded.contains(parent) {
                tree_changed = true;
            }
        }
        self.recent.retain(|_, at| at.elapsed() < RECENT);
        self.files = Arc::new(files);
        if tree_changed {
            self.rebuild();
        }
        for i in 0..self.open.len() {
            if batch.contains(&self.open[i].path) {
                self.follow_disk(i, cx);
            }
        }
        cx.notify();
    }

    /// An open file changed on disk: unedited, it reloads in place; with
    /// unsaved edits, it says so and waits.
    fn follow_disk(&mut self, i: usize, cx: &mut Context<Self>) {
        let path = self.open[i].path.clone();
        let Some(view) = self.open[i].view().cloned() else { return };
        let Ok(text) = std::fs::read_to_string(&path) else {
            if !path.exists() {
                self.open[i].disk = Disk::Deleted;
            }
            return;
        };
        let (dirty, same) = {
            let v = view.read(cx);
            (v.buffer.is_dirty(), v.buffer.text() == text)
        };
        if same {
            self.open[i].disk = Disk::Same;
            view.update(cx, |v, _| v.buffer.mark_saved());
        } else if dirty {
            self.open[i].disk = Disk::Changed;
        } else {
            self.open[i].disk = Disk::Same;
            view.update(cx, |v, cx| v.reload(&text, cx));
        }
    }

    fn rebuild(&mut self) {
        let mut rows = Vec::new();
        self.add_children(&self.root.clone(), 0, &mut rows);
        self.rows = rows;
    }

    fn add_children(&self, dir: &Path, depth: usize, rows: &mut Vec<Row>) {
        for row in read_folder(dir, depth) {
            let open = row.dir && self.expanded.contains(&row.path);
            let path = row.path.clone();
            rows.push(row);
            if open {
                self.add_children(&path, depth + 1, rows);
            }
        }
    }

    /// Reads everything again: the tree, the file list, open files.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.expanded.retain(|dir| dir.is_dir());
        self.rebuild();
        self.start_walk(cx);
        for i in 0..self.open.len() {
            self.follow_disk(i, cx);
        }
        cx.notify();
    }

    fn selected_index(&self) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        self.rows.iter().position(|r| &r.path == selected)
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get(index) {
            self.selected = Some(row.path.clone());
            self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    fn toggle(&mut self, dir: &Path, cx: &mut Context<Self>) {
        if !self.expanded.remove(dir) {
            self.expanded.insert(dir.to_path_buf());
        }
        self.rebuild();
        cx.notify();
    }

    /// Opens the folders down to `path` and selects it in the tree.
    fn reveal(&mut self, path: &Path, cx: &mut Context<Self>) {
        for dir in path.ancestors().skip(1) {
            if dir == self.root || !dir.starts_with(&self.root) {
                break;
            }
            self.expanded.insert(dir.to_path_buf());
        }
        self.rebuild();
        self.selected = Some(path.to_path_buf());
        if let Some(i) = self.selected_index() {
            self.scroll.scroll_to_item(i, ScrollStrategy::Center);
        }
        cx.notify();
    }

    fn click_row(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(index) else { return };
        let (path, dir) = (row.path.clone(), row.dir);
        self.selected = Some(path.clone());
        if dir {
            self.toggle(&path, cx);
        } else {
            self.open_file(path, None, true, cx);
        }
        cx.notify();
    }

    fn tree_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let m = &event.keystroke.modifiers;
        if m.control || m.alt || m.platform || m.function {
            return;
        }
        let current = self.selected_index();
        let row = current.and_then(|i| self.rows.get(i)).map(|r| (r.path.clone(), r.dir, r.depth));
        match event.keystroke.key.as_str() {
            "down" => self.select(current.map_or(0, |i| (i + 1).min(self.rows.len().saturating_sub(1))), cx),
            "up" => self.select(current.map_or(0, |i| i.saturating_sub(1)), cx),
            "home" => self.select(0, cx),
            "end" => self.select(self.rows.len().saturating_sub(1), cx),
            "enter" => match row {
                Some((path, true, _)) => self.toggle(&path, cx),
                Some((path, false, _)) => {
                    self.open_file(path, None, true, cx);
                }
                None => return,
            },
            "right" => match row {
                Some((path, true, _)) if !self.expanded.contains(&path) => self.toggle(&path, cx),
                Some((_, true, _)) => self.select(current.unwrap() + 1, cx),
                _ => return,
            },
            "left" => match row {
                Some((path, true, _)) if self.expanded.contains(&path) => self.toggle(&path, cx),
                Some((_, _, depth)) if depth > 0 => {
                    let i = current.unwrap();
                    if let Some(parent) = self.rows[..i].iter().rposition(|r| r.depth < depth) {
                        self.select(parent, cx);
                    }
                }
                _ => return,
            },
            _ => return,
        }
        cx.stop_propagation();
    }

    /// Keys anywhere in the tab: open by name, close the file.
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let sec = ks.modifiers.secondary();
        match ks.key.as_str() {
            "p" if sec && !ks.modifiers.shift => self.open_quick(window, cx),
            "f" if sec && ks.modifiers.shift => self.open_search(window, cx),
            "w" if sec && !ks.modifiers.shift => {
                if let Some(i) = self.active {
                    self.close_file(i, window, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    /// Opens a file (or shows it if open), at a line and column from 1.
    pub fn open_path(&mut self, path: PathBuf, at: Option<(usize, Option<usize>)>, cx: &mut Context<Self>) {
        self.open_file(path.clone(), at, true, cx);
        self.reveal(&path, cx);
    }

    fn open_file(&mut self, path: PathBuf, at: Option<(usize, Option<usize>)>, focus: bool, cx: &mut Context<Self>) -> usize {
        let i = match self.open.iter().position(|f| f.path == path) {
            Some(i) => i,
            None => {
                let file = self.load(path, cx);
                self.open.push(file);
                let i = self.open.len() - 1;
                self.rehighlight(i, cx);
                i
            }
        };
        self.active = Some(i);
        if let (Some((line, col)), Some(view)) = (at, self.open[i].view().cloned()) {
            view.update(cx, |v, cx| v.go_to(line.saturating_sub(1), col.unwrap_or(1).saturating_sub(1), cx));
        }
        self.focus_editor |= focus;
        self.publish();
        cx.notify();
        i
    }

    fn load(&mut self, path: PathBuf, cx: &mut Context<Self>) -> OpenFile {
        let lang = shed_project::detect(&path);
        let mut file = OpenFile { path: path.clone(), content: Content::Binary, lang, disk: Disk::Same, restored: false, highlight: None, recovery: None, _sub: None };
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if is_image(&path) {
            file.content = Content::Image;
            return file;
        }
        if size > MAX_OPEN {
            file.content = Content::TooLarge(size);
            return file;
        }
        let Ok(bytes) = std::fs::read(&path) else { return file };
        if bytes.iter().take(8000).any(|b| *b == 0) {
            return file;
        }
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let options = TextOptions { line_numbers: true, comment: lang.and_then(|l| l.comment), ..Default::default() };
        let settings = self.settings.clone();
        let view = cx.new(|cx| TextView::new(&text, options, &settings, cx));
        // Unsaved edits from before (a crash, a closed bench) come back.
        if let Some(saved) = read_recovery(&path)
            && saved != text
        {
            view.update(cx, |v, _| {
                v.buffer.select_all();
                v.buffer.insert(&saved);
                v.buffer.set_cursor(0, false);
            });
            file.restored = true;
        }
        file._sub = Some(cx.subscribe(&view, move |this, view, event: &TextEvent, cx| {
            let Some(i) = this.open.iter().position(|f| f.view() == Some(&view)) else { return };
            match event {
                TextEvent::Changed => {
                    this.rehighlight(i, cx);
                    this.schedule_recovery(i, cx);
                    cx.notify();
                }
                TextEvent::Save => this.save(i, cx),
                TextEvent::Submit | TextEvent::Cancel => {}
            }
        }));
        file.content = Content::Text(view);
        file
    }

    /// Colours an open file again, once typing pauses, off the UI thread.
    fn rehighlight(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(file) = self.open.get_mut(i) else { return };
        let (Some(lang), Some(view)) = (file.lang, file.view().cloned()) else { return };
        let palette = self.settings.palette.clone();
        file.highlight = Some(cx.spawn(async move |_, cx| {
            cx.background_executor().timer(HIGHLIGHT_DELAY).await;
            let text = view.read_with(cx, |v, _| v.buffer.rope().to_string());
            if text.len() > MAX_HIGHLIGHT {
                return;
            }
            let spans = cx
                .background_executor()
                .spawn(async move {
                    let spans = shed_project::highlight(lang, &text)?;
                    let lines: Vec<LineSpans> = spans
                        .into_iter()
                        .map(|line| line.into_iter().map(|(r, s)| (r, syntax_color(s, &palette))).collect())
                        .collect();
                    Some(Arc::new(lines))
                })
                .await;
            if let Some(spans) = spans {
                view.update(cx, |v, cx| v.set_highlights(Some(spans), cx));
            }
        }));
    }

    /// Writes unsaved edits aside, so nothing is lost if Shed or the bench
    /// closes first; a clean file's recovery file goes.
    fn schedule_recovery(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(file) = self.open.get_mut(i) else { return };
        let (Some(view), path) = (file.view().cloned(), file.path.clone()) else { return };
        file.recovery = Some(cx.spawn(async move |_, cx| {
            cx.background_executor().timer(RECOVERY_DELAY).await;
            let (dirty, text) = view.read_with(cx, |v, _| (v.buffer.is_dirty(), v.buffer.text()));
            if dirty {
                write_recovery(&path, &text);
            } else {
                remove_recovery(&path);
            }
        }));
    }

    fn save(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(file) = self.open.get_mut(i) else { return };
        let Some(view) = file.view().cloned() else { return };
        let text = view.read(cx).buffer.text();
        match std::fs::write(&file.path, &text) {
            Ok(()) => {
                view.update(cx, |v, cx| {
                    v.buffer.mark_saved();
                    cx.notify();
                });
                file.disk = Disk::Same;
                file.restored = false;
                remove_recovery(&file.path);
                self.notice = None;
            }
            Err(e) => self.notice = Some(format!("Cannot save {}: {e}", file.name())),
        }
        cx.notify();
    }

    fn is_dirty(&self, i: usize, cx: &gpui::App) -> bool {
        self.open.get(i).and_then(OpenFile::view).is_some_and(|v| v.read(cx).buffer.is_dirty())
    }

    /// Closes an open file, asking first if it has unsaved edits.
    fn close_file(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_dirty(i, cx) {
            return self.drop_file(i, cx);
        }
        let name = self.open[i].name();
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Save changes to “{name}”?"),
            Some("Your changes are lost if you don't save them."),
            &["Save", "Discard Changes", "Cancel"],
            cx,
        );
        let path = self.open[i].path.clone();
        cx.spawn(async move |this, cx| {
            let answer = answer.await;
            this.update(cx, |this, cx| {
                let Some(i) = this.open.iter().position(|f| f.path == path) else { return };
                match answer {
                    Ok(0) => {
                        this.save(i, cx);
                        this.drop_file(i, cx);
                    }
                    Ok(1) => {
                        remove_recovery(&path);
                        this.drop_file(i, cx);
                    }
                    _ => {}
                }
            })
            .ok();
        })
        .detach();
    }

    fn drop_file(&mut self, i: usize, cx: &mut Context<Self>) {
        if i >= self.open.len() {
            return;
        }
        self.open.remove(i);
        self.active = match self.active {
            _ if self.open.is_empty() => None,
            Some(a) if a > i => Some(a - 1),
            Some(a) => Some(a.min(self.open.len() - 1)),
            None => None,
        };
        self.focus_editor = true;
        self.publish();
        cx.notify();
    }

    fn activate(&mut self, i: usize, cx: &mut Context<Self>) {
        self.active = Some(i);
        self.focus_editor = true;
        self.publish();
        cx.notify();
    }

    /// Takes the file as it is on disk, dropping the edits here.
    fn take_disk(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(file) = self.open.get_mut(i) else { return };
        let Some(view) = file.view().cloned() else { return };
        if let Ok(text) = std::fs::read_to_string(&file.path) {
            view.update(cx, |v, cx| v.reload(&text, cx));
        }
        file.disk = Disk::Same;
        file.restored = false;
        remove_recovery(&file.path);
        cx.notify();
    }

    fn open_quick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editor = cx.new(|cx| LineEditor::new(String::new(), cx).without_hint());
        window.focus(&editor.read(cx).focus_handle(cx), cx);
        let sub = cx.observe(&editor, |this, _, cx| this.refresh_quick(cx));
        self.quick = Some(QuickOpen { editor, results: Vec::new(), selected: 0, _sub: sub });
        cx.notify();
    }

    fn refresh_quick(&mut self, cx: &mut Context<Self>) {
        let Some(quick) = &mut self.quick else { return };
        let query = quick.editor.read(cx).text().to_string();
        quick.results = shed_project::fuzzy::rank(&query, &self.files, 14);
        quick.selected = 0;
        cx.notify();
    }

    fn quick_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(quick) = &mut self.quick else { return };
        match event.keystroke.key.as_str() {
            "escape" => {
                self.quick = None;
                self.focus_editor = true;
            }
            "down" => quick.selected = (quick.selected + 1).min(quick.results.len().saturating_sub(1)),
            "up" => quick.selected = quick.selected.saturating_sub(1),
            "enter" => {
                let pick = quick.results.get(quick.selected).map(|(i, _)| self.root.join(&self.files[*i]));
                self.quick = None;
                if let Some(path) = pick {
                    self.open_path(path, None, cx);
                }
                self.focus_editor = true;
            }
            _ => return,
        }
        let _ = window;
        cx.stop_propagation();
        cx.notify();
    }

    /// A new terminal tab in `dir`, next to this one.
    fn open_terminal(&self, dir: &Path) {
        self.conn.send_msg(ClientMsg::NewTab { bench: self.bench, cwd: Some(dir.to_path_buf()) });
    }

    fn set_exclude(&mut self, exclude: Vec<String>, cx: &mut Context<Self>) {
        self.exclude = exclude;
        self.publish();
        self.start_walk(cx);
        cx.notify();
    }

    /// Shows the search panel with the keyboard in its field, seeded with
    /// the editor's selection if there is one.
    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self
            .active
            .and_then(|i| self.open.get(i))
            .and_then(OpenFile::view)
            .map(|v| v.read(cx).buffer.selected_text())
            .filter(|t| !t.is_empty() && !t.contains('\n'));
        if self.search.is_none() {
            let query = cx.new(|cx| LineEditor::new(String::new(), cx).without_hint());
            let replace = cx.new(|cx| LineEditor::new(String::new(), cx).without_hint());
            let subs = vec![cx.observe(&query, |this, _, cx| this.schedule_search(cx))];
            self.search = Some(SearchPanel {
                query,
                replace,
                regex: false,
                case_sensitive: false,
                whole_word: false,
                results: Vec::new(),
                rows: Vec::new(),
                searched: None,
                cancel: Arc::default(),
                running: false,
                error: None,
                task: None,
                scroll: UniformListScrollHandle::new(),
                _subs: subs,
            });
        }
        let search = self.search.as_ref().unwrap();
        if let Some(text) = selected {
            search.query.update(cx, |q, cx| *q = LineEditor::selected(text, cx).without_hint());
        }
        window.focus(&search.query.read(cx).focus_handle(cx), cx);
        self.panel = Panel::Search;
        self.schedule_search(cx);
        cx.notify();
    }

    /// Searches again for what the panel asks, a moment after typing pauses;
    /// results arrive file by file.
    fn schedule_search(&mut self, cx: &mut Context<Self>) {
        let (root, files) = (self.root.clone(), self.files.clone());
        let Some(search) = &mut self.search else { return };
        let q = search.query(cx);
        if search.searched.as_ref() == Some(&q) {
            return;
        }
        search.cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        search.cancel = cancel.clone();
        search.searched = Some(q.clone());
        search.results.clear();
        search.rows.clear();
        search.error = shed_project::search::check(&q).err().filter(|_| !q.text.is_empty());
        search.running = search.error.is_none() && !q.text.is_empty();
        if !search.running {
            cx.notify();
            return;
        }
        search.task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(120)).await;
            let (tx, rx) = async_channel::unbounded::<FileHits>();
            let finished = cancel.clone();
            cx.background_executor()
                .spawn(async move {
                    let _ = shed_project::search::search(&root, &files, &q, &finished, SEARCH_LIMIT, |f| {
                        let _ = tx.send_blocking(f);
                    });
                })
                .detach();
            while let Ok(found) = rx.recv().await {
                let alive = this.update(cx, |this, cx| {
                    let Some(search) = &mut this.search else { return };
                    if !Arc::ptr_eq(&search.cancel, &cancel) {
                        return;
                    }
                    let at = search.results.partition_point(|r| r.path < found.path);
                    search.results.insert(at, found);
                    search.rebuild_rows();
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
            this.update(cx, |this, cx| {
                if let Some(search) = &mut this.search
                    && Arc::ptr_eq(&search.cancel, &cancel)
                {
                    search.running = false;
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn toggle_search_option(&mut self, which: u8, cx: &mut Context<Self>) {
        if let Some(s) = &mut self.search {
            match which {
                0 => s.case_sensitive = !s.case_sensitive,
                1 => s.whole_word = !s.whole_word,
                _ => s.regex = !s.regex,
            }
        }
        self.schedule_search(cx);
    }

    /// Opens a file at a hit, its match selected.
    fn open_hit(&mut self, file: usize, hit: usize, cx: &mut Context<Self>) {
        let Some(search) = &self.search else { return };
        let Some(found) = search.results.get(file) else { return };
        let Some(h) = found.hits.get(hit).cloned() else { return };
        let path = self.root.join(&found.path);
        self.open_path(path.clone(), None, cx);
        let Some(view) = self.open.iter().find(|f| f.path == path).and_then(OpenFile::view).cloned() else { return };
        view.update(cx, |v, cx| {
            let line = v.buffer.line(h.line);
            let at = |byte: usize| {
                let mut b = (h.offset + byte).min(line.len());
                while !line.is_char_boundary(b) {
                    b -= 1;
                }
                line[..b].chars().count()
            };
            let start = v.buffer.line_start(h.line);
            let r = h.ranges.first().cloned().unwrap_or(0..0);
            v.go_to(h.line, at(r.start), cx);
            v.select(start + at(r.start)..start + at(r.end), cx);
        });
    }

    /// Replaces every match in every file found, after asking. Files open
    /// with unsaved edits are left alone.
    fn replace_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(search) = &self.search else { return };
        let Some(q) = search.searched.clone() else { return };
        let with = search.replace.read(cx).text().to_string();
        let files: Vec<PathBuf> = search.results.iter().map(|r| self.root.join(&r.path)).collect();
        let count: usize = search.results.iter().flat_map(|r| &r.hits).map(|h| h.ranges.len()).sum();
        if files.is_empty() {
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Replace {count} matches in {} files?", files.len()),
            Some(&format!("“{}” becomes “{with}” in every file found. Files open here with unsaved changes are skipped.", q.text)),
            &["Replace All", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if !matches!(answer.await, Ok(0)) {
                return;
            }
            this.update(cx, |this, cx| {
                let mut skipped = Vec::new();
                for path in &files {
                    if let Some(i) = this.open.iter().position(|f| &f.path == path)
                        && this.is_dirty(i, cx)
                    {
                        skipped.push(this.open[i].name());
                        continue;
                    }
                    let Ok(text) = std::fs::read_to_string(path) else { continue };
                    if let Ok((new, n)) = shed_project::search::replace(&text, &q, &with)
                        && n > 0
                    {
                        let _ = std::fs::write(path, new);
                    }
                }
                this.notice = (!skipped.is_empty()).then(|| format!("Not replaced in {} (unsaved changes)", skipped.join(", ")));
                if let Some(s) = &mut this.search {
                    s.searched = None;
                }
                this.schedule_search(cx);
            })
            .ok();
        })
        .detach();
    }

    fn render_search(&self, ui: &UiColors, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let search = self.search.as_ref()?;
        let accent = self.accent;
        let toggle = |id: &'static str, label: &'static str, on: bool, which: u8, tip: &'static str| {
            div()
                .id(id)
                .px_1()
                .rounded_sm()
                .cursor_pointer()
                .text_size(px(11.0))
                .text_color(hsla(if on { ui.text } else { ui.text_dim }))
                .when(on, |d| d.bg(hsla_alpha(accent, 0.3)))
                .hover(|s| s.bg(hsla(ui.tab_hover)))
                .tooltip(menu::tooltip(tip))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_search_option(which, cx)))
                .child(label)
        };
        let matches: usize = search.results.iter().map(|r| r.hits.len()).sum();
        let status = match (&search.error, search.running, matches) {
            (Some(e), _, _) => e.lines().last().unwrap_or("").to_string(),
            (None, true, 0) => "Searching…".into(),
            (None, true, n) => format!("{n} lines so far…"),
            (None, false, 0) if search.searched.as_ref().is_some_and(|q| !q.text.is_empty()) => "No matches".into(),
            (None, false, 0) => String::new(),
            (None, false, n) => format!("{n} lines in {} files{}", search.results.len(), if n >= SEARCH_LIMIT { " (stopped at the limit)" } else { "" }),
        };
        let list = uniform_list("search-results", search.rows.len(), cx.processor(|this, range, _, cx| this.render_hits(range, cx)))
            .track_scroll(&search.scroll)
            .flex_1()
            .min_h_0();
        Some(
            div()
                .size_full()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .text_size(px(12.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(div().flex_1().min_w_0().child(search.query.clone()))
                        .child(toggle("s-case", "Aa", search.case_sensitive, 0, "Match case"))
                        .child(toggle("s-word", "W", search.whole_word, 1, "Whole words"))
                        .child(toggle("s-regex", ".*", search.regex, 2, "Regular expression")),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(div().flex_1().min_w_0().child(search.replace.clone()))
                        .child(
                            div()
                                .id("replace-all")
                                .px_1p5()
                                .rounded_sm()
                                .border_1()
                                .border_color(hsla(ui.divider))
                                .cursor_pointer()
                                .text_size(px(11.0))
                                .text_color(hsla(ui.text_dim))
                                .hover(|s| s.bg(hsla(ui.tab_hover)).text_color(hsla(ui.text)))
                                .on_click(cx.listener(|this, _, window, cx| this.replace_all(window, cx)))
                                .child("Replace All…"),
                        ),
                )
                .child(div().text_size(px(11.0)).text_color(hsla(if search.error.is_some() { ui.danger } else { ui.text_dim })).child(status))
                .child(list)
                .into_any_element(),
        )
    }

    fn render_hits(&mut self, range: std::ops::Range<usize>, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let ui = current_ui(cx);
        let accent = self.accent;
        let Some(search) = &self.search else { return Vec::new() };
        range
            .filter_map(|i| {
                let (f, hit) = *search.rows.get(i)?;
                let found = search.results.get(f)?;
                let row = div().id(("hit", i)).h(px(ROW_HEIGHT)).flex().items_center().gap_1p5().px_1().rounded_sm().cursor_pointer().hover(|s| s.bg(hsla(ui.tab_hover)));
                Some(match hit {
                    None => {
                        let name = found.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        let dir = found.path.parent().map(|p| p.display().to_string()).unwrap_or_default();
                        row.child(svg().path(icon("file")).size(px(13.0)).flex_none().text_color(hsla(ui.text_dim)))
                            .child(div().flex_none().text_color(hsla(ui.text)).child(name))
                            .child(div().flex_1().min_w_0().truncate().text_color(hsla(ui.text_dim)).child(dir))
                            .child(div().flex_none().text_color(hsla(ui.text_dim)).child(found.hits.len().to_string()))
                            .on_click(cx.listener(move |this, _, _, cx| this.open_hit(f, 0, cx)))
                            .into_any_element()
                    }
                    Some(h) => {
                        let hit = &found.hits[h];
                        // Without the line's leading blanks.
                        let lead = hit.text.len() - hit.text.trim_start().len();
                        let text = hit.text[lead..].to_string();
                        let highlights: Vec<_> = hit
                            .ranges
                            .iter()
                            .filter(|r| r.start >= lead)
                            .map(|r| (r.start - lead..r.end - lead, HighlightStyle { background_color: Some(hsla_alpha(accent, 0.35)), ..Default::default() }))
                            .collect();
                        row.pl(px(20.0))
                            .child(div().flex_none().w(px(36.0)).text_color(hsla(ui.text_dim)).child((hit.line + 1).to_string()))
                            .child(div().flex_1().min_w_0().truncate().text_color(hsla(ui.text)).child(StyledText::new(text).with_highlights(highlights)))
                            .on_click(cx.listener(move |this, _, _, cx| this.open_hit(f, h, cx)))
                            .into_any_element()
                    }
                })
            })
            .collect()
    }

    fn render_panel_switch(&self, ui: &UiColors, cx: &mut Context<Self>) -> impl IntoElement {
        let accent = self.accent;
        let tab = |id: &'static str, glyph: &'static str, label: &'static str, panel: Panel| {
            let on = self.panel == panel;
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .h_full()
                .cursor_pointer()
                .text_size(px(12.0))
                .text_color(hsla(if on { ui.text } else { ui.text_dim }))
                .when(on, |d| d.border_b_2().border_color(hsla(accent)))
                .hover(|s| s.text_color(hsla(ui.text)))
                .child(svg().path(icon(glyph)).size(px(13.0)).text_color(hsla(ui.text_dim)))
                .child(label)
        };
        div()
            .flex()
            .flex_none()
            .h(px(28.0))
            .border_b_1()
            .border_color(hsla(ui.divider))
            .child(tab("panel-files", "folder", "Files", Panel::Files).on_click(cx.listener(|this, _, _, cx| {
                this.panel = Panel::Files;
                cx.notify();
            })))
            .child(
                tab("panel-search", "search", "Search", Panel::Search)
                    .tooltip(menu::tooltip("Search in files (Ctrl+Shift+F)"))
                    .on_click(cx.listener(|this, _, window, cx| this.open_search(window, cx))),
            )
    }

    fn on_resize(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((x0, w0)) = self.resizing else { return };
        if event.pressed_button != Some(MouseButton::Left) {
            self.resizing = None;
            return;
        }
        self.tree_width = (w0 + f32::from(event.position.x) - x0).clamp(160.0, 900.0);
        cx.notify();
    }

    fn row_menu(&self, path: &Path, dir: bool) -> Vec<MenuEntry<Self>> {
        let folder = if dir { path.to_path_buf() } else { path.parent().unwrap_or(&self.root).to_path_buf() };
        let relative = path.strip_prefix(&self.root).unwrap_or(path).display().to_string();
        let (full, file, rel) = (path.display().to_string(), path.to_path_buf(), relative.clone());
        let mut entries = Vec::new();
        if !dir {
            let open = path.to_path_buf();
            entries.push(menu::item("Open", move |this: &mut Self, _, cx| this.open_path(open.clone(), None, cx)).icon("file"));
        }
        entries.extend([
            menu::item("Open Terminal Here", move |this: &mut Self, _, _| this.open_terminal(&folder)).icon("terminal"),
            menu::item("Show in File Manager", move |_, _, cx| cx.reveal_path(&file)).icon("folder"),
            MenuEntry::Separator,
            menu::item("Copy Path", move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(full.clone()))),
            menu::item("Copy Relative Path", move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(relative.clone()))),
            MenuEntry::Separator,
            menu::item("Exclude from Project", move |this: &mut Self, _, cx| {
                let mut exclude = this.exclude.clone();
                let pattern = if dir { format!("/{rel}/") } else { format!("/{rel}") };
                if !exclude.contains(&pattern) {
                    exclude.push(pattern);
                }
                this.set_exclude(exclude, cx);
            }),
        ]);
        entries
    }

    fn header_menu(&self) -> Vec<MenuEntry<Self>> {
        let mut entries = vec![
            menu::item("Open File by Name…", |this: &mut Self, window, cx| this.open_quick(window, cx)).icon("file"),
            menu::item("Refresh", |this: &mut Self, _, cx| this.refresh(cx)).icon("refresh"),
        ];
        if !self.exclude.is_empty() {
            entries.push(MenuEntry::Separator);
            for pattern in &self.exclude {
                let p = pattern.clone();
                entries.push(
                    menu::item(format!("Include “{pattern}” Again"), move |this: &mut Self, _, cx| {
                        let exclude = this.exclude.iter().filter(|e| **e != p).cloned().collect();
                        this.set_exclude(exclude, cx);
                    })
                    .icon("close"),
                );
            }
        }
        entries
    }

    fn render_rows(&mut self, range: std::ops::Range<usize>, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let ui = current_ui(cx);
        let accent = self.accent;
        let selected = self.selected_index();
        let active = self.active.and_then(|i| self.open.get(i)).map(|f| f.path.clone());
        let recent: Vec<&PathBuf> = self.recent.iter().filter(|(_, at)| at.elapsed() < RECENT).map(|(p, _)| p).collect();
        range
            .filter_map(|i| {
                let row = self.rows.get(i)?;
                let (path, dir) = (row.path.clone(), row.dir);
                let open = dir && self.expanded.contains(&row.path);
                let changed = recent.iter().any(|p| if dir { p.starts_with(&row.path) } else { **p == row.path });
                let color = if row.hidden { ui.text_dim } else { ui.text };
                let mut el = div()
                    .id(i)
                    .h(px(ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .pl(px(8.0 + row.depth as f32 * INDENT))
                    .pr_2()
                    .cursor_pointer()
                    .text_color(hsla(color))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.focus(&this.tree_focus, cx);
                        this.click_row(i, cx);
                    }))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        this.selected = Some(path.clone());
                        let entries = this.row_menu(&path, dir);
                        this.open_menu(e.position, entries, cx);
                    }));
                el = if selected == Some(i) {
                    el.bg(hsla_alpha(accent, 0.25))
                } else if active.as_ref() == Some(&row.path) {
                    el.bg(hsla_alpha(accent, 0.12))
                } else {
                    el.hover(|s| s.bg(hsla(ui.tab_hover)))
                };
                let chevron = if !dir { "" } else if open { "▾" } else { "▸" };
                let glyph = if dir { "folder" } else { "file" };
                let glyph_color = if dir { ui.ink(accent) } else { ui.text_dim };
                Some(
                    el.child(div().w(px(10.0)).flex_none().text_color(hsla(ui.text_dim)).child(chevron))
                        .child(svg().path(icon(glyph)).size(px(14.0)).flex_none().text_color(hsla(glyph_color)))
                        .child(div().flex_1().min_w_0().truncate().child(row.name.clone()))
                        .when(changed, |el| el.child(div().flex_none().size(px(6.0)).rounded_full().bg(hsla(ui.ink(accent)))))
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn render_header(&self, ui: &UiColors, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.root.display().to_string());
        let button = |id: &'static str, glyph: &'static str, label: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1p5()
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .text_color(hsla(ui.text_dim))
                .hover(|s| s.bg(hsla(ui.tab_hover)).text_color(hsla(ui.text)))
                .child(svg().path(icon(glyph)).size(px(14.0)).text_color(hsla(ui.text_dim)))
                .child(label)
        };
        let root = self.root.clone();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(36.0))
            .px_3()
            .border_b_1()
            .border_color(hsla(ui.divider))
            .bg(hsla(ui.background))
            .child(svg().path(icon("project")).size(px(16.0)).flex_none().text_color(hsla(ui.ink(self.accent))))
            .child(div().flex_none().text_size(px(14.0)).text_color(hsla(ui.text)).child(name))
            .child(div().flex_1().min_w_0().truncate().text_size(px(12.0)).text_color(hsla(ui.text_dim)).child(shed_save::home_relative(&self.root)))
            .child(button("project-open", "file", "Open File").tooltip(menu::tooltip("Open a file by name (Ctrl+P)")).on_click(cx.listener(|this, _, window, cx| this.open_quick(window, cx))))
            .child(button("project-terminal", "terminal", "Terminal").on_click(cx.listener(|this, _, _, _| this.open_terminal(&this.root.clone()))))
            .child(button("project-files", "folder", "File Manager").on_click(move |_, _, cx| cx.open_with_system(&root)))
            .child(button("project-refresh", "refresh", "Refresh").on_click(cx.listener(|this, _, _, cx| this.refresh(cx))))
            .child(
                div()
                    .id("project-more")
                    .p_1()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(hsla(ui.tab_hover)))
                    .tooltip(menu::tooltip("More"))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                        let entries = this.header_menu();
                        this.open_menu(e.position, entries, cx);
                    }))
                    .child(svg().path(icon("more")).size(px(14.0)).text_color(hsla(ui.text_dim))),
            )
    }

    fn render_strip(&self, ui: &UiColors, cx: &mut Context<Self>) -> impl IntoElement {
        let accent = self.accent;
        let mut strip = div().id("file-strip").flex().flex_none().h(px(30.0)).overflow_x_scroll().bg(hsla(ui.background)).border_b_1().border_color(hsla(ui.divider));
        for (i, file) in self.open.iter().enumerate() {
            let active = self.active == Some(i);
            let dirty = file.view().is_some_and(|v| v.read(cx).buffer.is_dirty());
            let mut el = div()
                .id(("file", i))
                .group("file")
                .flex()
                .flex_none()
                .items_center()
                .gap_1p5()
                .h_full()
                .pl_3()
                .pr_1()
                .border_r_1()
                .border_color(hsla(ui.divider))
                .cursor_pointer()
                .text_size(px(12.0))
                .text_color(hsla(if active { ui.text } else { ui.text_dim }))
                .on_click(cx.listener(move |this, _, _, cx| this.activate(i, cx)))
                .on_mouse_down(MouseButton::Middle, cx.listener(move |this, _, window, cx| this.close_file(i, window, cx)))
                .child(file.name());
            el = if active { el.bg(hsla(ui.tab_active)).border_t_2().border_color(hsla(accent)) } else { el.hover(|s| s.bg(hsla(ui.tab_hover))) };
            if file.disk != Disk::Same {
                el = el.child(div().text_color(hsla(ui.danger)).child("!"));
            }
            el = el.child(
                div()
                    .id(("close-file", i))
                    .flex_none()
                    .size(px(16.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .hover(|s| s.bg(hsla(ui.divider)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.close_file(i, window, cx);
                    }))
                    .child(if dirty {
                        div().size(px(7.0)).rounded_full().bg(hsla(ui.text)).into_any_element()
                    } else {
                        svg().path(icon("close")).size(px(10.0)).text_color(hsla(ui.text_dim)).invisible().group_hover("file", |s| s.visible()).into_any_element()
                    }),
            );
            strip = strip.child(el);
        }
        strip
    }

    fn render_bar(&self, ui: &UiColors, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let i = self.active?;
        let file = self.open.get(i)?;
        let (text, color, actions): (String, Rgb, Vec<(&'static str, u8)>) = match (file.disk, file.restored) {
            (Disk::Changed, _) => ("Changed on disk while you were editing it.".into(), ui.danger, vec![("Take the Disk's Version", 0), ("Keep Mine", 1)]),
            (Disk::Deleted, _) => ("Deleted on disk. Saving writes it again.".into(), ui.danger, vec![("Close", 2)]),
            (_, true) => ("Unsaved changes from before were brought back.".into(), self.accent, vec![("Discard Them", 0), ("Keep", 1)]),
            _ => return None,
        };
        let mut bar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .text_size(px(12.0))
            .bg(hsla_alpha(color, 0.18))
            .text_color(hsla(ui.text))
            .child(div().flex_1().child(text));
        for (label, action) in actions {
            bar = bar.child(
                div()
                    .id(label)
                    .px_2()
                    .rounded_sm()
                    .border_1()
                    .border_color(hsla(ui.divider))
                    .cursor_pointer()
                    .hover(|s| s.bg(hsla(ui.tab_hover)))
                    .child(label)
                    .on_click(cx.listener(move |this, _, window, cx| match action {
                        0 => this.take_disk(i, cx),
                        1 => {
                            if let Some(f) = this.open.get_mut(i) {
                                f.disk = Disk::Same;
                                f.restored = false;
                            }
                            cx.notify();
                        }
                        _ => this.close_file(i, window, cx),
                    })),
            );
        }
        Some(bar)
    }

    fn render_status(&self, ui: &UiColors, cx: &gpui::App) -> Option<impl IntoElement> {
        let file = self.open.get(self.active?)?;
        let rel = file.path.strip_prefix(&self.root).unwrap_or(&file.path).display().to_string();
        let mut parts = vec![rel];
        if let Some(view) = file.view() {
            let b = &view.read(cx).buffer;
            let (line, col) = b.line_col(b.selection().head);
            parts.push(format!("Ln {}, Col {}", line + 1, col + 1));
            if let Some(lang) = file.lang {
                parts.push(lang.name.into());
            }
            parts.push(if b.indent.starts_with('\t') { "Tabs".into() } else { format!("Spaces: {}", b.indent.len()) });
            if b.line_ending == shed_text::LineEnding::Crlf {
                parts.push("CRLF".into());
            }
            if b.is_dirty() {
                parts.push("modified".into());
            }
        }
        Some(
            div()
                .flex_none()
                .h(px(22.0))
                .px_3()
                .flex()
                .items_center()
                .text_size(px(11.0))
                .text_color(hsla(ui.text_dim))
                .bg(hsla(ui.background))
                .border_t_1()
                .border_color(hsla(ui.divider))
                .child(div().truncate().child(parts.join("  ·  "))),
        )
    }

    fn render_content(&self, ui: &UiColors) -> gpui::AnyElement {
        let empty = |label: String| {
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .text_size(px(13.0))
                .text_color(hsla(ui.text_dim))
                .child(svg().path(icon("file")).size(px(28.0)).text_color(hsla(ui.divider)))
                .child(label)
                .into_any_element()
        };
        let Some(file) = self.active.and_then(|i| self.open.get(i)) else {
            return empty("Open a file from the tree, or by name with Ctrl+P".into());
        };
        match &file.content {
            Content::Text(view) => view.clone().into_any_element(),
            Content::Image => div().size_full().p_4().flex().items_center().justify_center().child(img(file.path.clone()).max_w_full().max_h_full()).into_any_element(),
            Content::Binary => empty(format!("{} is not text", file.name())),
            Content::TooLarge(size) => empty(format!("{} is too large to open here ({} MB)", file.name(), size / (1024 * 1024))),
        }
    }

    fn render_quick(&self, ui: &UiColors, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let quick = self.quick.as_ref()?;
        let accent = self.accent;
        let rows = quick.results.iter().enumerate().map(|(n, (i, matched))| {
            let path = self.files[*i].display().to_string();
            let highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = path
                .char_indices()
                .enumerate()
                .filter(|(ci, _)| matched.binary_search(&(*ci as u32)).is_ok())
                .map(|(_, (b, c))| (b..b + c.len_utf8(), HighlightStyle { color: Some(hsla(ui.ink(accent))), ..Default::default() }))
                .collect();
            let selected = n == quick.selected;
            let full = self.root.join(&self.files[*i]);
            div()
                .id(("quick", n))
                .px_2()
                .py_0p5()
                .rounded_sm()
                .cursor_pointer()
                .text_color(hsla(ui.text))
                .when(selected, |d| d.bg(hsla_alpha(accent, 0.25)))
                .hover(|s| s.bg(hsla(ui.tab_hover)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.quick = None;
                    this.open_path(full.clone(), None, cx);
                }))
                .child(StyledText::new(path).with_highlights(highlights))
        });
        let count = self.files.len();
        Some(
            div()
                .absolute()
                .top(px(40.0))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .w(px(560.0))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_2()
                        .rounded_md()
                        .bg(hsla(ui.tab_active))
                        .border_1()
                        .border_color(hsla(ui.divider))
                        .text_size(px(13.0))
                        .occlude()
                        .capture_key_down(cx.listener(Self::quick_key))
                        .child(div().flex().items_center().gap_2().child(div().flex_1().child(quick.editor.clone())).child(div().text_size(px(11.0)).text_color(hsla(ui.text_dim)).child(format!("{count} files"))))
                        .children(rows),
                ),
        )
    }
}

/// The colour of a kind of code, from the terminal palette, so it suits
/// the theme, light or dark.
fn syntax_color(style: Style, p: &shed_emu::Palette) -> Rgb {
    match style {
        Style::Keyword => p.ansi[5],
        Style::String => p.ansi[2],
        Style::Comment => p.ansi[8],
        Style::Function => p.ansi[4],
        Style::Type => p.ansi[3],
        Style::Number | Style::Constant | Style::Escape => p.ansi[6],
        Style::Property => p.ansi[12],
        Style::Tag => p.ansi[1],
        Style::Attribute => p.ansi[11],
        Style::Builtin => p.ansi[9],
        Style::Label => p.ansi[13],
        Style::Heading => p.ansi[4],
        Style::Punctuation => p.foreground,
    }
}

fn is_image(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    matches!(ext.as_deref(), Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico"))
}

/// Where a file's unsaved edits are kept aside.
fn recovery_file(path: &Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    shed_model::paths::state_dir().join("recovery").join(format!("{:016x}", h.finish()))
}

fn write_recovery(path: &Path, text: &str) {
    let file = recovery_file(path);
    let _ = std::fs::create_dir_all(file.parent().unwrap());
    let _ = std::fs::write(file, format!("{}\n{text}", path.display()));
}

fn read_recovery(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(recovery_file(path)).ok()?;
    let (first, rest) = text.split_once('\n')?;
    (Path::new(first) == path).then(|| rest.to_string())
}

fn remove_recovery(path: &Path) {
    let _ = std::fs::remove_file(recovery_file(path));
}

/// A folder's entries, folders first, each group by name; `.git` is left out.
fn read_folder(dir: &Path, depth: usize) -> Vec<Row> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut rows: Vec<Row> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_name() != ".git")
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            // Through symlinks, so a linked folder opens like a folder.
            let dir = e.path().is_dir();
            Row { hidden: name.starts_with('.'), name: name.into(), path: e.path(), depth, dir }
        })
        .collect();
    rows.sort_by_cached_key(|r| (!r.dir, r.name.to_lowercase()));
    rows
}

/// The project a folder belongs to: the nearest folder above it (or itself)
/// with a `.git`, else the folder itself.
pub fn project_root(dir: &Path) -> PathBuf {
    dir.ancestors().find(|d| d.join(".git").exists()).unwrap_or(dir).to_path_buf()
}

impl MenuHost for ProjectView {
    fn menu_slot(&mut self) -> &mut Option<Menu<Self>> {
        &mut self.menu
    }

    fn menu_closed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.focus_editor = true;
        cx.notify();
    }
}

impl Render for ProjectView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.focus_editor) && self.quick.is_none() {
            let target = self.focus_target(cx);
            window.focus(&target, cx);
        }
        let ui = current_ui(cx);
        let tree = uniform_list("project-tree", self.rows.len(), cx.processor(|this, range, _, cx| this.render_rows(range, cx)))
            .track_scroll(&self.scroll)
            .size_full()
            .py_1();
        let menu = self.menu.as_ref().map(|m| menu::render(m, &ui, cx));
        let notice = self.notice.clone().or_else(|| self.limited.then(|| "Not every folder can be watched (the system's limit): use Refresh to catch up.".to_string()));
        let editor = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .when(!self.open.is_empty(), |d| d.child(self.render_strip(&ui, cx)))
            .children(self.render_bar(&ui, cx))
            .when_some(notice, |d, n| d.child(div().flex_none().px_3().py_1().text_size(px(12.0)).bg(hsla_alpha(ui.danger, 0.15)).text_color(hsla(ui.text)).child(n)))
            .child(div().flex_1().min_h_0().child(self.render_content(&ui)))
            .children(self.render_status(&ui, cx));
        div()
            .key_context("Project")
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_move(cx.listener(Self::on_resize))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.resizing = None))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .text_size(px(13.0))
            .child(self.render_header(&ui, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .w(px(self.tree_width))
                            .flex_none()
                            .h_full()
                            .flex()
                            .flex_col()
                            .bg(hsla(ui.background))
                            .child(self.render_panel_switch(&ui, cx))
                            .child(match (self.panel, self.render_search(&ui, cx)) {
                                (Panel::Search, Some(search)) => div().flex_1().min_h_0().child(search),
                                _ => div().flex_1().min_h_0().track_focus(&self.tree_focus).on_key_down(cx.listener(Self::tree_key)).child(tree),
                            }),
                    )
                    .child(
                        // The panel's edge: drag to resize.
                        div()
                            .id("panel-edge")
                            .flex_none()
                            .w(px(5.0))
                            .h_full()
                            .flex()
                            .justify_center()
                            .cursor_col_resize()
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                                this.resizing = Some((f32::from(e.position.x), this.tree_width));
                                cx.stop_propagation();
                            }))
                            .child(div().w(px(1.0)).h_full().bg(hsla(ui.divider))),
                    )
                    .child(editor),
            )
            .children(self.render_quick(&ui, cx))
            .children(menu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_come_first_and_git_is_left_out() {
        let dir = tempfile::tempdir().unwrap();
        for d in ["src", ".git", "Docs"] {
            std::fs::create_dir(dir.path().join(d)).unwrap();
        }
        for f in ["b.rs", "A.md", ".env"] {
            std::fs::write(dir.path().join(f), "").unwrap();
        }
        let names: Vec<String> = read_folder(dir.path(), 0).iter().map(|r| r.name.to_string()).collect();
        assert_eq!(names, ["Docs", "src", ".env", "A.md", "b.rs"]);
        std::fs::create_dir_all(dir.path().join("src/deep")).unwrap();
        assert_eq!(project_root(&dir.path().join("src/deep")), dir.path());
    }
}
