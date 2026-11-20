//! Project tabs: the file tree, quick open by name, a file viewer coloured
//! by tree-sitter, search in files (streamed, cancellable) and the Git
//! panel (`git.rs`). Files are edited in `$EDITOR`, in a terminal tab.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use shed_model::TabId;
use shed_project::search::{FileHits, Query};

use crate::app::App;
use crate::overlay::Item;
use crate::text::{Input, centered, fit, pad, width};
use crate::theme::Colors;

/// Files larger than this are not shown.
const MAX_VIEW: u64 = 4 * 1024 * 1024;
/// Search stops after this many matching lines.
const SEARCH_LIMIT: usize = 5000;

pub struct Row {
    pub path: PathBuf,
    pub name: String,
    pub dir: bool,
    pub depth: usize,
}

pub struct FileView {
    pub path: PathBuf,
    pub lines: Vec<Line<'static>>,
    pub scroll: usize,
    /// A line to mark (a search hit), from 0.
    pub mark: Option<usize>,
    /// Why there is no text: a picture, too big, not text.
    pub note: Option<String>,
}

pub struct Quick {
    pub input: Input,
    pub results: Vec<usize>,
    pub sel: usize,
}

pub struct SearchView {
    pub input: Input,
    pub query: Query,
    pub results: Vec<FileHits>,
    pub sel: usize,
    /// Typing goes to the query (else the keys move through results).
    pub editing: bool,
    pub running: bool,
    pub cancel: Arc<AtomicBool>,
    pub generation: u64,
    pub error: Option<String>,
}

impl SearchView {
    /// Every hit as (file, hit), in order.
    fn flat(&self) -> Vec<(usize, usize)> {
        self.results.iter().enumerate().flat_map(|(f, h)| (0..h.hits.len()).map(move |i| (f, i))).collect()
    }
}

pub enum Side {
    Files,
    Search(SearchView),
    Git(Box<crate::git::GitView>),
}

pub struct ProjectView {
    pub root: PathBuf,
    pub files: Vec<PathBuf>,
    pub loaded: bool,
    pub expanded: HashSet<PathBuf>,
    pub rows: Vec<Row>,
    pub sel: usize,
    /// Keys go to the file viewer (or diff) rather than the list.
    pub right: bool,
    pub file: Option<FileView>,
    pub side: Side,
    pub quick: Option<Quick>,
    watch: Option<shed_project::Watch>,
    pub list_area: Rect,
    pub view_area: Rect,
    search_generation: u64,
}

impl ProjectView {
    pub fn new(root: PathBuf) -> Self {
        let mut v = ProjectView {
            root,
            files: Vec::new(),
            loaded: false,
            expanded: HashSet::new(),
            rows: Vec::new(),
            sel: 0,
            right: false,
            file: None,
            side: Side::Files,
            quick: None,
            watch: None,
            list_area: Rect::default(),
            view_area: Rect::default(),
            search_generation: 0,
        };
        v.rebuild_rows();
        v
    }

    /// The tree's rows: the root's entries, and those of open folders.
    pub fn rebuild_rows(&mut self) {
        fn add(dir: &Path, depth: usize, expanded: &HashSet<PathBuf>, out: &mut Vec<Row>) {
            for e in shed_project::read_folder(dir) {
                let open = e.dir && expanded.contains(&e.path);
                out.push(Row { path: e.path.clone(), name: e.name, dir: e.dir, depth });
                if open {
                    add(&e.path, depth + 1, expanded, out);
                }
            }
        }
        let keep = self.rows.get(self.sel).map(|r| r.path.clone());
        let mut rows = Vec::new();
        add(&self.root, 0, &self.expanded, &mut rows);
        self.rows = rows;
        if let Some(p) = keep
            && let Some(i) = self.rows.iter().position(|r| r.path == p)
        {
            self.sel = i;
        }
        self.sel = self.sel.min(self.rows.len().saturating_sub(1));
    }

    /// The file to hand to `$EDITOR`: the one open, else the one selected.
    pub fn editable_path(&self) -> Option<PathBuf> {
        if let Some(f) = &self.file
            && (self.right || matches!(self.side, Side::Search(_)))
        {
            return Some(f.path.clone());
        }
        match &self.side {
            Side::Git(g) => g.selected_path().map(|p| self.root.join(p)),
            _ => self.rows.get(self.sel).filter(|r| !r.dir).map(|r| r.path.clone()).or_else(|| self.file.as_ref().map(|f| f.path.clone())),
        }
    }
}

/// A file's text as numbered, coloured lines.
pub fn file_lines(path: &Path, text: &str, c: &Colors) -> Vec<Line<'static>> {
    let colored = shed_project::detect(path).and_then(|l| shed_project::highlight(l, text));
    let digits = text.lines().count().max(1).to_string().len().max(3);
    text.lines()
        .enumerate()
        .map(|(n, line)| {
            let mut spans = vec![Span::styled(format!("{:>digits$} ", n + 1), c.dim())];
            let parts = match colored.as_ref().and_then(|s| s.get(n)) {
                Some(ranges) => crate::md::colored_line(line, ranges, c),
                None => vec![Span::raw(line.to_string())],
            };
            spans.extend(parts.into_iter().map(|s| Span::styled(s.content.replace('\t', "    "), s.style)));
            Line::from(spans)
        })
        .collect()
}

/// What the tab menu offers in a project tab.
pub fn menu_items() -> Vec<crate::overlay::TabAction> {
    vec![
        (Item::new("Open by Name…").hint("o / Ctrl+P"), |app, tab| app.quick_open(tab)),
        (Item::new("Search in Files").hint("/ or Ctrl+F"), |app, tab| app.project_search(tab)),
        (Item::new("Git").hint("g"), |app, tab| app.git_open(tab)),
        (Item::new("Files").hint("Esc"), |app, tab| {
            if let Some(v) = app.projects.get_mut(&tab) {
                v.side = Side::Files;
            }
        }),
        (Item::new("Edit in $EDITOR").hint("e"), |app, _| app.edit_in_editor()),
        (Item::new("Read Again").hint("r"), |app, tab| app.project_load(tab)),
    ]
}

impl App {
    /// Reads the project's files and follows the disk.
    pub(crate) fn project_load(&mut self, tab: TabId) {
        let Some(view) = self.projects.get_mut(&tab) else { return };
        view.rebuild_rows();
        let root = view.root.clone();
        let exclude = self.tree.tab(tab).and_then(|t| t.project_info()).map(|p| p.exclude.clone()).unwrap_or_default();
        let tx = self.tx.clone();
        self.spawn(move || {
            let walked = shed_project::walk(&root, &exclude);
            let changed = move |paths: Vec<PathBuf>| {
                let _ = tx.send(crate::app::Event::Apply(Box::new(move |app: &mut App| app.project_changed(tab, paths))));
            };
            let watch = shed_project::Watch::new(&walked.dirs, shed_project::Excluder::new(&root, &exclude), changed).ok();
            move |app: &mut App| {
                if let Some(v) = app.projects.get_mut(&tab) {
                    v.files = walked.files;
                    v.loaded = true;
                    v.watch = watch;
                    // Typed before the files were known: ranked now.
                    if let Some(q) = &mut v.quick {
                        q.results = shed_project::fuzzy::rank(&q.input.text, &v.files, 200).into_iter().map(|(i, _)| i).collect();
                    }
                }
            }
        });
    }

    /// Files changed on disk: the tree, the file shown and Git follow.
    fn project_changed(&mut self, tab: TabId, paths: Vec<PathBuf>) {
        let Some(v) = self.projects.get_mut(&tab) else { return };
        v.rebuild_rows();
        let reopen = v.file.as_ref().filter(|f| paths.contains(&f.path)).map(|f| (f.path.clone(), f.scroll, f.mark));
        let git = matches!(v.side, Side::Git(_));
        if let Some((path, scroll, mark)) = reopen {
            self.open_file(tab, path, mark, Some(scroll));
        }
        if git {
            self.git_refresh(tab);
        }
    }

    /// Shows a file in the viewer; `mark` scrolls to a line and marks it.
    pub(crate) fn open_file(&mut self, tab: TabId, path: PathBuf, mark: Option<usize>, scroll: Option<usize>) {
        let c = self.colors.clone();
        self.spawn(move || {
            let mut view = FileView { path: path.clone(), lines: Vec::new(), scroll: 0, mark, note: None };
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if shed_project::picture::is_picture(&path) {
                view.note = Some(match shed_project::picture::PictureData::read(&path) {
                    Some(p) => format!("A picture: {}. Pictures show in the windows, not in a terminal.", p.describe()),
                    None => "A picture that cannot be read.".into(),
                });
            } else if size > MAX_VIEW {
                view.note = Some(format!("{} MB: too big to show here (e opens it in $EDITOR).", size / (1024 * 1024)));
            } else {
                match std::fs::read(&path) {
                    Ok(bytes) if bytes.iter().take(8192).any(|b| *b == 0) => view.note = Some("Not text.".into()),
                    Ok(bytes) => view.lines = file_lines(&path, &String::from_utf8_lossy(&bytes), &c),
                    Err(e) => view.note = Some(format!("Cannot read it: {e}")),
                }
            }
            move |app: &mut App| {
                if let Some(v) = app.projects.get_mut(&tab) {
                    let h = v.view_area.height as usize;
                    view.scroll = scroll.unwrap_or_else(|| mark.map_or(0, |m| m.saturating_sub(h / 3)));
                    v.file = Some(view);
                }
            }
        });
    }

    pub(crate) fn quick_open(&mut self, tab: TabId) {
        if let Some(v) = self.projects.get_mut(&tab) {
            v.quick = Some(Quick { input: Input::default(), results: Vec::new(), sel: 0 });
        }
    }

    pub(crate) fn project_search(&mut self, tab: TabId) {
        let Some(v) = self.projects.get_mut(&tab) else { return };
        if let Side::Search(s) = &mut v.side {
            s.editing = true;
        } else {
            v.side = Side::Search(SearchView {
                input: Input::default(),
                query: Query::default(),
                results: Vec::new(),
                sel: 0,
                editing: true,
                running: false,
                cancel: Arc::new(AtomicBool::new(false)),
                generation: 0,
                error: None,
            });
        }
        v.right = false;
    }

    fn run_search(&mut self, tab: TabId) {
        let Some(v) = self.projects.get_mut(&tab) else { return };
        let Side::Search(s) = &mut v.side else { return };
        s.cancel.store(true, Ordering::Relaxed);
        s.results.clear();
        s.sel = 0;
        s.error = None;
        s.query.text = s.input.text.clone();
        if let Err(e) = shed_project::search::check(&s.query) {
            s.error = Some(e);
            return;
        }
        if s.query.text.is_empty() {
            return;
        }
        v.search_generation += 1;
        let generation = v.search_generation;
        s.generation = generation;
        s.running = true;
        let cancel = Arc::new(AtomicBool::new(false));
        s.cancel = cancel.clone();
        let (root, files, q) = (v.root.clone(), v.files.clone(), s.query.clone());
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let found = |hits: FileHits| {
                let _ = tx.send(crate::app::Event::Apply(Box::new(move |app: &mut App| {
                    if let Some(ProjectView { side: Side::Search(s), .. }) = app.projects.get_mut(&tab)
                        && s.generation == generation
                    {
                        s.results.push(hits);
                    }
                })));
            };
            let result = shed_project::search::search(&root, &files, &q, &cancel, SEARCH_LIMIT, found);
            let _ = tx.send(crate::app::Event::Apply(Box::new(move |app: &mut App| {
                if let Some(ProjectView { side: Side::Search(s), .. }) = app.projects.get_mut(&tab)
                    && s.generation == generation
                {
                    s.running = false;
                    s.results.sort_by(|a, b| a.path.cmp(&b.path));
                    if let Err(e) = result {
                        s.error = Some(e);
                    }
                }
            })));
        });
    }

    pub(crate) fn project_paste(&mut self, tab: TabId, text: &str) {
        let Some(v) = self.projects.get_mut(&tab) else { return };
        let line = text.lines().next().unwrap_or("");
        if let Some(q) = &mut v.quick {
            q.input.insert(line);
        } else if let Side::Search(s) = &mut v.side {
            s.input.insert(line);
            s.editing = true;
        }
    }

    pub(crate) fn project_key(&mut self, tab: TabId, ev: KeyEvent) {
        let Some(v) = self.projects.get_mut(&tab) else { return };
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        // Quick open, over everything.
        if let Some(mut q) = v.quick.take() {
            match ev.code {
                KeyCode::Esc => return,
                KeyCode::Enter => {
                    if let Some(&i) = q.results.get(q.sel) {
                        let path = v.root.join(&v.files[i]);
                        v.right = true;
                        self.open_file(tab, path, None, None);
                    }
                    return;
                }
                KeyCode::Up => q.sel = q.sel.saturating_sub(1),
                KeyCode::Down | KeyCode::Tab => q.sel = (q.sel + 1).min(q.results.len().saturating_sub(1)),
                _ => {
                    if q.input.key(&ev) == crate::text::Edit::Changed {
                        q.results = shed_project::fuzzy::rank(&q.input.text, &v.files, 200).into_iter().map(|(i, _)| i).collect();
                        q.sel = 0;
                    }
                }
            }
            v.quick = Some(q);
            return;
        }
        if ctrl && ev.code == KeyCode::Char('p') {
            return self.quick_open(tab);
        }
        if ctrl && ev.code == KeyCode::Char('f') {
            return self.project_search(tab);
        }
        match &mut v.side {
            Side::Git(_) => return self.git_key(tab, ev),
            Side::Search(s) if s.editing => {
                let alt = ev.modifiers.contains(KeyModifiers::ALT);
                match ev.code {
                    KeyCode::Esc => {
                        s.cancel.store(true, Ordering::Relaxed);
                        v.side = Side::Files;
                    }
                    KeyCode::Enter => {
                        s.editing = false;
                        self.run_search(tab);
                    }
                    KeyCode::Down | KeyCode::Tab => s.editing = false,
                    KeyCode::Char('r') if alt => s.query.regex = !s.query.regex,
                    KeyCode::Char('c') if alt => s.query.case_sensitive = !s.query.case_sensitive,
                    KeyCode::Char('w') if alt => s.query.whole_word = !s.query.whole_word,
                    _ => {
                        s.input.key(&ev);
                    }
                }
                return;
            }
            Side::Search(s) if !v.right => {
                let flat = s.flat();
                match ev.code {
                    KeyCode::Esc => {
                        if s.running {
                            s.cancel.store(true, Ordering::Relaxed);
                            s.running = false;
                        } else {
                            v.side = Side::Files;
                        }
                    }
                    KeyCode::Char('/') => s.editing = true,
                    KeyCode::Up | KeyCode::Char('k') => {
                        if s.sel == 0 {
                            s.editing = true;
                        }
                        s.sel = s.sel.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => s.sel = (s.sel + 1).min(flat.len().saturating_sub(1)),
                    KeyCode::PageDown => s.sel = (s.sel + 10).min(flat.len().saturating_sub(1)),
                    KeyCode::PageUp => s.sel = s.sel.saturating_sub(10),
                    KeyCode::Tab | KeyCode::Right => v.right = true,
                    KeyCode::Enter => {
                        if let Some(&(f, h)) = flat.get(s.sel) {
                            let path = v.root.join(&s.results[f].path);
                            let line = s.results[f].hits[h].line;
                            self.open_file(tab, path, Some(line), None);
                        }
                    }
                    KeyCode::Char('e') => self.edit_in_editor(),
                    _ => {}
                }
                return;
            }
            _ => {}
        }
        if v.right {
            let page = v.view_area.height.max(2) as usize - 1;
            let Some(file) = &mut v.file else {
                v.right = false;
                return;
            };
            match ev.code {
                KeyCode::Tab | KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => v.right = false,
                KeyCode::Up | KeyCode::Char('k') => file.scroll = file.scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => file.scroll += 1,
                KeyCode::PageUp | KeyCode::Char('b') => file.scroll = file.scroll.saturating_sub(page),
                KeyCode::PageDown | KeyCode::Char(' ') => file.scroll += page,
                KeyCode::Home | KeyCode::Char('g') => file.scroll = 0,
                KeyCode::End | KeyCode::Char('G') => file.scroll = file.lines.len().saturating_sub(page),
                KeyCode::Char('e') => self.edit_in_editor(),
                KeyCode::Char('o') => self.quick_open(tab),
                _ => {}
            }
            return;
        }
        // The file tree.
        let n = v.rows.len();
        match ev.code {
            KeyCode::Up | KeyCode::Char('k') => v.sel = v.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => v.sel = (v.sel + 1).min(n.saturating_sub(1)),
            KeyCode::PageUp => v.sel = v.sel.saturating_sub(10),
            KeyCode::PageDown => v.sel = (v.sel + 10).min(n.saturating_sub(1)),
            KeyCode::Home => v.sel = 0,
            KeyCode::End => v.sel = n.saturating_sub(1),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                let Some(row) = v.rows.get(v.sel) else { return };
                let path = row.path.clone();
                if row.dir {
                    if !v.expanded.remove(&path) {
                        v.expanded.insert(path);
                    }
                    v.rebuild_rows();
                } else {
                    if ev.code == KeyCode::Enter {
                        v.right = true;
                    }
                    self.open_file(tab, path, None, None);
                }
            }
            KeyCode::Left | KeyCode::Char('h') => {
                let Some(row) = v.rows.get(v.sel) else { return };
                let path = row.path.clone();
                if row.dir && v.expanded.remove(&path) {
                    v.rebuild_rows();
                } else if let Some(parent) = path.parent().filter(|p| *p != v.root) {
                    let parent = parent.to_path_buf();
                    v.expanded.remove(&parent);
                    v.rebuild_rows();
                    if let Some(i) = v.rows.iter().position(|r| r.path == parent) {
                        v.sel = i;
                    }
                }
            }
            KeyCode::Tab => v.right = v.file.is_some(),
            KeyCode::Char('o') => self.quick_open(tab),
            KeyCode::Char('/') => self.project_search(tab),
            KeyCode::Char('g') => self.git_open(tab),
            KeyCode::Char('e') => self.edit_in_editor(),
            KeyCode::Char('r') => self.project_load(tab),
            KeyCode::Esc => {
                if let Side::Search(_) = v.side {
                    v.side = Side::Files;
                }
            }
            _ => {}
        }
    }

    pub(crate) fn project_mouse(&mut self, tab: TabId, m: MouseEvent) {
        let Some(v) = self.projects.get_mut(&tab) else { return };
        let inside = |r: Rect| m.column >= r.x && m.column < r.x + r.width && m.row >= r.y && m.row < r.y + r.height;
        let in_view = inside(v.view_area);
        match m.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = m.kind == MouseEventKind::ScrollUp;
                if in_view {
                    if let Side::Git(g) = &mut v.side {
                        g.scroll_diff(if up { -3 } else { 3 });
                    } else if let Some(f) = &mut v.file {
                        f.scroll = if up { f.scroll.saturating_sub(3) } else { f.scroll + 3 };
                    }
                } else if let Side::Files = v.side {
                    v.sel = if up { v.sel.saturating_sub(3) } else { (v.sel + 3).min(v.rows.len().saturating_sub(1)) };
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if in_view {
                    v.right = true;
                } else if inside(v.list_area) && matches!(v.side, Side::Files) {
                    v.right = false;
                    let top = v.sel.saturating_sub(v.list_area.height.saturating_sub(1) as usize);
                    let i = top + (m.row - v.list_area.y) as usize;
                    if i < v.rows.len() {
                        let again = i == v.sel;
                        v.sel = i;
                        if again || !v.rows[i].dir {
                            self.project_key(tab, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    pub(crate) fn render_project(&mut self, f: &mut Frame, area: Rect, tab: TabId) {
        let c = self.colors.clone();
        let accent = self.current_bench().map(|b| b.color.accent()).unwrap_or(c.accent);
        let Some(v) = self.projects.get_mut(&tab) else { return };
        let list_w = (area.width / 3).clamp(20, 44).min(area.width);
        let list = Rect { width: list_w, ..area };
        let view = Rect { x: area.x + list_w, width: area.width - list_w, ..area };
        v.list_area = Rect { y: list.y + 1, height: list.height.saturating_sub(1), ..list };
        v.view_area = Rect { x: view.x + 1, width: view.width.saturating_sub(1), y: view.y + 1, height: view.height.saturating_sub(1) };
        if let Side::Git(_) = v.side {
            return self.render_git(f, list, view, tab);
        }
        let focus = |on: bool| if on { c.fg(c.ink(accent)).add_modifier(Modifier::BOLD) } else { c.dim() };
        // The left side: the tree, or search.
        let w = list_w as usize;
        match &mut v.side {
            Side::Search(s) => {
                let flags = format!("{}{}{}", if s.query.regex { " .*" } else { "" }, if s.query.case_sensitive { " Aa" } else { "" }, if s.query.whole_word { " \\b" } else { "" });
                let status = if s.running { " searching…".to_string() } else { format!(" {} files", s.results.len()) };
                f.render_widget(Paragraph::new(Line::from(vec![Span::styled(" Search", focus(!v.right)), Span::styled(format!("{flags}{status}"), c.dim())])), Rect { height: 1, ..list });
                let field = Rect { y: list.y + 1, height: 1, ..list };
                f.render_widget(Paragraph::new(Line::from(vec![Span::styled("/ ", c.accent()), Span::raw(s.input.text.clone())])).style(Style::new().bg(c.c(c.tab_active))), field);
                if s.editing && self.overlay.is_none() {
                    f.set_cursor_position((field.x + 2 + width(&s.input.text[..s.input.cursor]) as u16, field.y));
                }
                let results = Rect { y: list.y + 2, height: list.height.saturating_sub(3), ..list };
                let mut lines: Vec<Line> = Vec::new();
                if let Some(e) = &s.error {
                    lines.push(Line::styled(fit(e, w), c.danger()));
                }
                let flat = s.flat();
                s.sel = s.sel.min(flat.len().saturating_sub(1));
                let mut sel_line = 0;
                for (fi, fh) in s.results.iter().enumerate() {
                    lines.push(Line::styled(fit(&fh.path.display().to_string(), w), c.accent()));
                    for (hi, hit) in fh.hits.iter().enumerate() {
                        let selected = flat.get(s.sel) == Some(&(fi, hi)) && !s.editing;
                        if selected {
                            sel_line = lines.len();
                        }
                        let text = format!("{:>4} {}", hit.line + 1, hit.text.trim());
                        lines.push(Line::styled(pad(&text, w), if selected { c.selected() } else { Style::new() }));
                    }
                }
                let h = results.height as usize;
                let top = sel_line.saturating_sub(h.saturating_sub(2));
                f.render_widget(Paragraph::new(lines.into_iter().skip(top).take(h).collect::<Vec<_>>()), results);
                let hint = Rect { y: list.y + list.height.saturating_sub(1), height: 1, ..list };
                f.render_widget(Paragraph::new(Line::styled(fit("Enter search · Alt+R regex · Alt+C case · Alt+W word", w), c.dim())), hint);
            }
            Side::Files | Side::Git(_) => {
                let name = v.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let count = if v.loaded { format!(" {} files", v.files.len()) } else { " reading…".into() };
                f.render_widget(Paragraph::new(Line::from(vec![Span::styled(format!(" {name}"), focus(!v.right)), Span::styled(count, c.dim())])), Rect { height: 1, ..list });
                let rows = v.list_area;
                let h = rows.height as usize;
                let top = v.sel.saturating_sub(h.saturating_sub(1));
                let open = v.file.as_ref().map(|f| f.path.clone());
                let lines: Vec<Line> = v
                    .rows
                    .iter()
                    .enumerate()
                    .skip(top)
                    .take(h)
                    .map(|(i, r)| {
                        let icon = if r.dir { if v.expanded.contains(&r.path) { "▾ " } else { "▸ " } } else { "  " };
                        let text = pad(&format!("{}{icon}{}", "  ".repeat(r.depth), r.name), w);
                        let style = if i == v.sel && !v.right {
                            c.selected()
                        } else if open.as_ref() == Some(&r.path) {
                            c.fg(c.ink(accent))
                        } else if r.dir {
                            Style::new().add_modifier(Modifier::BOLD)
                        } else if r.name.starts_with('.') {
                            c.dim()
                        } else {
                            Style::new()
                        };
                        Line::styled(text, style)
                    })
                    .collect();
                f.render_widget(Paragraph::new(lines), rows);
            }
        }
        // The right side: the file.
        let block = Block::default().borders(Borders::LEFT).border_style(c.fg(c.divider));
        f.render_widget(block, view);
        let inner = Rect { x: view.x + 1, width: view.width.saturating_sub(1), ..view };
        match &mut v.file {
            None => {
                let hint = vec![
                    Line::default(),
                    Line::styled("  Enter opens a file · o opens by name · / searches · g Git · e edits in $EDITOR", c.dim()),
                    Line::styled("  Tab moves between the list and the file · C-b m all actions", c.dim()),
                ];
                f.render_widget(Paragraph::new(hint), inner);
            }
            Some(file) => {
                let rel = file.path.strip_prefix(&v.root).unwrap_or(&file.path).display().to_string();
                f.render_widget(Paragraph::new(Line::from(vec![Span::styled(format!(" {rel}"), focus(v.right)), Span::styled(if v.right { "  ↑↓ PgUp PgDn g G · e edit · Tab back" } else { "" }, c.dim())])), Rect { height: 1, ..inner });
                let body = Rect { y: inner.y + 1, height: inner.height.saturating_sub(1), ..inner };
                if let Some(note) = &file.note {
                    f.render_widget(Paragraph::new(Line::styled(format!("  {note}"), c.dim())), body);
                } else {
                    file.scroll = file.scroll.min(file.lines.len().saturating_sub(1));
                    let lines: Vec<Line> = file
                        .lines
                        .iter()
                        .enumerate()
                        .skip(file.scroll)
                        .take(body.height as usize)
                        .map(|(n, l)| if file.mark == Some(n) { l.clone().style(Style::new().bg(c.c(c.tab_hover))) } else { l.clone() })
                        .collect();
                    f.render_widget(Paragraph::new(lines), body);
                }
            }
        }
        // Quick open, over the tab.
        if let Some(q) = &mut v.quick {
            let r = centered(area, area.width.saturating_sub(6).min(80), area.height.saturating_sub(2).min(18));
            let block = crate::overlay::frame_block(&c, "Open by Name", false);
            let inner = block.inner(r);
            f.render_widget(Clear, r);
            f.render_widget(block, r);
            let iw = inner.width as usize;
            let mut lines = vec![Line::from(vec![Span::styled("› ", c.accent()), Span::raw(q.input.text.clone())])];
            if q.input.text.is_empty() {
                lines.push(Line::styled("type part of a file's name or path", c.dim()));
            } else if q.results.is_empty() {
                lines.push(Line::styled("nothing matches", c.dim()));
            }
            let h = inner.height.saturating_sub(1) as usize;
            let top = q.sel.saturating_sub(h.saturating_sub(1));
            for (n, &i) in q.results.iter().enumerate().skip(top).take(h) {
                let text = pad(&v.files[i].display().to_string(), iw);
                lines.push(Line::styled(text, if n == q.sel { c.selected() } else { Style::new() }));
            }
            f.render_widget(Paragraph::new(lines).style(c.base()), inner);
            if self.overlay.is_none() {
                f.set_cursor_position((inner.x + 2 + width(&q.input.text[..q.input.cursor]) as u16, inner.y));
            }
        }
    }
}
