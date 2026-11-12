//! An AI tab: a Claude Code conversation. On the left, the folder's
//! sessions; in the middle, the conversation as it streams; below, the
//! prompt. The keeper runs Claude Code; this view shows what it says and
//! sends what the user types.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, ClipboardEntry, ClipboardItem, Context, CursorStyle, Entity, ExternalPaths, FocusHandle, Focusable,
    FollowMode, FontWeight, HighlightStyle, KeyDownEvent, ListAlignment, ListState, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, SharedString, StyledText, Subscription, Task, TextLayout, UniformListScrollHandle,
    Window, div, list, prelude::*, px, relative, svg, uniform_list,
};
use shed_ai::{Command, Entry, Limits, Model, SessionInfo, State as AiState, Status};
use shed_model::{AiTab, BenchId, Rgb, TabId};
use shed_proto::{AiImage, ClientMsg};

use crate::ai_text::{self, Copy, Kind, Piece, Point, Selection};
use crate::assets::icon;
use crate::keeper_conn::KeeperConn;
use crate::line_editor::LineEditor;
use crate::menu::{self, Menu, MenuEntry, MenuHost};
use crate::settings::Settings;
use crate::text_view::{TextEvent, TextOptions, TextView};
use crate::theme::{UiColors, current_ui, hsla, hsla_alpha};

const SESSIONS_WIDTH: f32 = 260.0;
/// A user's message longer than this many lines shows folded.
const FOLD_LINES: usize = 12;
/// Lines of a tool's output shown when it is opened.
const RESULT_LINES: usize = 40;

/// The account's usage limits, for every AI tab to show.
#[derive(Default)]
pub struct AiLimits(pub Limits);

impl gpui::Global for AiLimits {}

/// Slash-command completion, while the prompt starts with `/`.
struct Completion {
    /// Indices into the commands.
    items: Vec<usize>,
    selected: usize,
}

pub struct AiView {
    pub bench: BenchId,
    pub tab: TabId,
    pub ai: AiTab,
    pub accent: Rgb,
    conn: KeeperConn,
    settings: Settings,
    entries: Vec<Entry>,
    status: Status,
    commands: Vec<Command>,
    models: Vec<Model>,
    list: ListState,
    /// Entries shown opened (tools, thinking, long prompts).
    expanded: HashSet<usize>,
    prompt: Entity<TextView>,
    images: Vec<AiImage>,
    sessions: Arc<Vec<SessionInfo>>,
    sessions_task: Option<Task<()>>,
    filter: Entity<LineEditor>,
    sessions_scroll: UniformListScrollHandle,
    completion: Option<Completion>,
    /// Going back through earlier prompts with Ctrl+Up: how far back.
    history: Option<usize>,
    /// The bench's project folders, offered as more folders to work in.
    pub projects: Vec<PathBuf>,
    /// Tool calls whose files were opened already, when following edits.
    pub followed: HashSet<String>,
    menu: Option<Menu<Self>>,
    focus: FocusHandle,
    /// Ticks the "working for" time while a turn runs.
    ticker: Option<Task<()>>,
    /// Claude Code was asked to start ahead of the first prompt.
    warming: bool,
    /// Find in the conversation (Ctrl+F): the field, and the entry shown.
    find: Option<(Entity<LineEditor>, Option<usize>, Subscription)>,
    /// The task list is opened.
    tasks_open: bool,
    /// Claude's answers as drawn, by entry (parsed again when they change).
    parsed: HashMap<usize, Rc<Parsed>>,
    /// Text selected in the conversation, and whether the mouse is selecting.
    selection: Option<Selection>,
    selecting: bool,
    dragged: bool,
    /// The copy button that says "Copied" for a moment.
    copied: Option<(SharedString, Task<()>)>,
    _subs: Vec<Subscription>,
}

/// An entry's text as drawn: pieces of selectable text, and the colours of
/// those that are code.
struct Parsed {
    source: String,
    pieces: Vec<Piece>,
    code: Vec<Option<CodeColors>>,
}

/// Byte ranges of a code block and their colours.
type CodeColors = Vec<(Range<usize>, Rgb)>;

impl Parsed {
    fn plain(text: String) -> Self {
        Parsed { source: String::new(), pieces: vec![Piece { kind: Kind::Para, text: shed_ai::markdown::Text::plain(text) }], code: vec![None] }
    }
}

/// Code coloured as its language, ranges into the whole text.
fn code_colors(lang: &str, code: &str, palette: &shed_emu::Palette) -> Option<CodeColors> {
    if code.len() > 200_000 {
        return None;
    }
    let spans = shed_project::highlight(shed_project::by_name(lang)?, code)?;
    let starts = std::iter::once(0).chain(code.match_indices('\n').map(|(i, _)| i + 1));
    Some(
        spans
            .into_iter()
            .zip(starts)
            .flat_map(|(line, start)| line.into_iter().map(move |(r, style)| (start + r.start..start + r.end, crate::project_view::syntax_color(style, palette))))
            .collect(),
    )
}

fn text_index(layout: &TextLayout, at: gpui::Point<gpui::Pixels>) -> usize {
    match layout.index_for_position(at) {
        Ok(i) | Err(i) => i,
    }
}

impl AiView {
    pub fn new(bench: BenchId, tab: TabId, ai: AiTab, accent: Rgb, conn: KeeperConn, settings: Settings, cx: &mut Context<Self>) -> Self {
        let options = TextOptions {
            wrap: true,
            auto_height: Some((2, 14)),
            submit_on_enter: true,
            placeholder: Some("Ask Claude…  (Enter sends, Shift+Enter breaks the line, / for commands)".into()),
            ..Default::default()
        };
        let prompt = cx.new(|cx| TextView::new("", options, &settings, cx));
        let filter = cx.new(|cx| LineEditor::new(String::new(), cx).without_hint());
        let subs = vec![
            cx.subscribe(&prompt, |this, _, event: &TextEvent, cx| match event {
                TextEvent::Submit => this.send(cx),
                TextEvent::Changed => {
                    this.warm_up();
                    this.update_completion(cx);
                }
                TextEvent::Cancel => {
                    this.completion = None;
                    cx.notify();
                }
                TextEvent::Save => {}
            }),
            cx.observe(&filter, |_, _, cx| cx.notify()),
        ];
        let list = ListState::new(0, ListAlignment::Top, px(600.0));
        list.set_follow_mode(FollowMode::Tail);
        conn.send_msg(ClientMsg::AiWatch { tab });
        let mut view = Self {
            bench,
            tab,
            ai,
            accent,
            conn,
            settings,
            entries: Vec::new(),
            status: Status::default(),
            commands: Vec::new(),
            models: Vec::new(),
            list,
            expanded: HashSet::new(),
            prompt,
            images: Vec::new(),
            sessions: Arc::default(),
            sessions_task: None,
            filter,
            sessions_scroll: UniformListScrollHandle::new(),
            completion: None,
            history: None,
            projects: Vec::new(),
            followed: HashSet::new(),
            menu: None,
            focus: cx.focus_handle(),
            ticker: None,
            warming: false,
            find: None,
            tasks_open: false,
            parsed: HashMap::new(),
            selection: None,
            selecting: false,
            dragged: false,
            copied: None,
            _subs: subs,
        };
        view.load_sessions(cx);
        view
    }

    pub fn state(&self) -> AiState {
        self.status.state
    }

    /// The session's title, for notifications.
    pub fn title(&self) -> String {
        self.ai.session.as_ref().and_then(|id| self.sessions.iter().find(|s| &s.id == id)).map(|s| s.title.clone()).unwrap_or_else(|| "Claude".into())
    }

    pub fn focus_target(&self, cx: &gpui::App) -> FocusHandle {
        self.prompt.read(cx).focus_handle().clone()
    }

    pub fn apply_settings(&mut self, settings: &Settings, cx: &mut Context<Self>) {
        self.settings = settings.clone();
        self.parsed.clear();
        self.prompt.update(cx, |p, cx| p.apply(settings, cx));
        cx.notify();
    }

    /// Follows the tab as the keeper has it.
    pub fn set_ai(&mut self, ai: AiTab, cx: &mut Context<Self>) {
        if ai.session != self.ai.session && ai.session.is_some() && !self.sessions.iter().any(|s| Some(&s.id) == ai.session.as_ref()) {
            self.load_sessions(cx);
        }
        if ai != self.ai {
            self.ai = ai;
            cx.notify();
        }
    }

    /// The conversation from entry `from` on, from the keeper.
    pub fn update(&mut self, from: usize, entries: Vec<Entry>, status: Status, cx: &mut Context<Self>) {
        let from = from.min(self.entries.len());
        let old_len = self.entries.len();
        self.entries.truncate(from);
        self.entries.extend(entries);
        self.parsed.retain(|i, _| *i < from);
        if self.selection.is_some_and(|s| s.ordered().1.entry >= from) {
            self.selection = None;
        }
        if from == 0 {
            self.expanded.clear();
            self.list.reset(self.entries.len());
        } else {
            self.list.splice(from..old_len, self.entries.len() - from);
        }
        let finished = self.status.state != AiState::Idle && status.state == AiState::Idle;
        self.status = status;
        if finished {
            self.load_sessions(cx);
        }
        self.tick(cx);
        cx.notify();
    }

    pub fn set_info(&mut self, commands: Vec<Command>, models: Vec<Model>, cx: &mut Context<Self>) {
        self.commands = commands;
        self.models = models;
        cx.notify();
    }

    /// The files the last turn changed (for review), newest last, once each.
    fn turn_changes(&self) -> Vec<String> {
        let start = self.entries.iter().rposition(|e| matches!(e, Entry::User { .. })).unwrap_or(0);
        let mut files: Vec<String> = Vec::new();
        for e in &self.entries[start..] {
            if let Entry::Tool(t) = e
                && t.diff.is_some()
                && t.result.is_some()
                && !t.error
                && let Some(f) = &t.file
            {
                files.retain(|x| x != f);
                files.push(f.clone());
            }
        }
        files
    }

    /// The latest task list Claude keeps (its todo tool), if any.
    fn tasks(&self) -> Option<String> {
        self.entries.iter().rev().find_map(|e| match e {
            Entry::Tool(t) if t.name == "TodoWrite" => Some(t.detail.clone()),
            _ => None,
        })
    }

    /// Opens a changed file's diff in its project tab (or the file, outside Git).
    fn review(&self, file: String, cx: &mut Context<Self>) {
        let bench = self.bench;
        if let Some(state) = cx.try_global::<crate::state::AppHandle>().and_then(|h| h.0.upgrade()) {
            state.update(cx, |s, cx| s.review_in_project(bench, PathBuf::from(file), cx));
        }
    }

    fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editor = cx.new(|cx| LineEditor::new(String::new(), cx).without_hint());
        window.focus(&editor.read(cx).focus_handle(cx), cx);
        let sub = cx.observe(&editor, |this, _, cx| {
            if let Some(f) = &mut this.find {
                f.1 = None;
            }
            this.find_step(false, cx);
        });
        self.find = Some((editor, None, sub));
        cx.notify();
    }

    fn entry_text(e: &Entry) -> String {
        match e {
            Entry::User { text, .. } | Entry::Assistant { text } | Entry::Thinking { text } | Entry::Notice { text, .. } => text.clone(),
            Entry::Tool(t) => format!("{} {} {}", t.summary, t.detail, t.result.clone().unwrap_or_default()),
            Entry::Permission(p) => format!("{} {}", p.summary, p.detail),
        }
    }

    /// Goes to the next (or previous) entry containing what is typed.
    fn find_step(&mut self, back: bool, cx: &mut Context<Self>) {
        let Some((editor, at, _)) = &self.find else { return };
        let query = editor.read(cx).text().to_lowercase();
        if query.is_empty() {
            return;
        }
        let n = self.entries.len();
        let start = at.map_or(if back { n } else { 0 }, |a| if back { a } else { a + 1 });
        let hits: Vec<usize> = (0..n).filter(|i| Self::entry_text(&self.entries[*i]).to_lowercase().contains(&query)).collect();
        let next = if back { hits.iter().rev().find(|i| **i < start).or(hits.last()) } else { hits.iter().find(|i| **i >= start).or(hits.first()) };
        if let Some(&i) = next {
            if let Some(f) = &mut self.find {
                f.1 = Some(i);
            }
            self.list.scroll_to_reveal_item(i);
        }
        cx.notify();
    }

    fn find_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => {
                self.find = None;
                let target = self.focus_target(cx);
                window.focus(&target, cx);
                cx.notify();
            }
            "enter" => self.find_step(event.keystroke.modifiers.shift, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    /// Writes text to a file the user names.
    fn save_as(&self, name: &str, text: String, cx: &mut Context<Self>) {
        let answer = cx.prompt_for_new_path(&self.ai.folder, Some(name));
        cx.spawn(async move |_, _| {
            if let Ok(Ok(Some(path))) = answer.await {
                let _ = std::fs::write(path, text);
            }
        })
        .detach();
    }

    /// Puts a file's text into the prompt, for a long prompt kept on disk.
    fn insert_file(&mut self, cx: &mut Context<Self>) {
        let answer = cx.prompt_for_paths(gpui::PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Insert".into()) });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = answer.await
                && let Some(text) = paths.first().and_then(|p| std::fs::read_to_string(p).ok())
            {
                this.update(cx, |this, cx| this.insert(&text, cx)).ok();
            }
        })
        .detach();
    }

    fn more_menu(&self, cx: &gpui::App) -> Vec<MenuEntry<Self>> {
        let conversation = self.transcript_markdown();
        let prompt = self.prompt.read(cx).buffer.text();
        vec![
            menu::item("Find in the Conversation…", |this: &mut Self, window, cx| this.open_find(window, cx)).icon("search"),
            menu::item("Insert a File's Text…", |this: &mut Self, _, cx| this.insert_file(cx)).icon("file"),
            menu::item("Save the Prompt…", move |this: &mut Self, _, cx| this.save_as("prompt.md", prompt.clone(), cx)),
            menu::item("Save the Conversation…", move |this: &mut Self, _, cx| this.save_as("conversation.md", conversation.clone(), cx)),
        ]
    }

    /// The entries a window should follow: edits that finished, with files.
    pub fn finished_edits(&self) -> Vec<(String, String)> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                Entry::Tool(t) if t.diff.is_some() && t.result.is_some() && !t.error => Some((t.id.clone(), t.file.clone()?)),
                _ => None,
            })
            .collect()
    }

    /// Keeps the "working for" time current while a turn runs.
    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.status.state == AiState::Idle {
            self.ticker = None;
            return;
        }
        if self.ticker.is_some() {
            return;
        }
        self.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        }));
    }

    fn load_sessions(&mut self, cx: &mut Context<Self>) {
        let folder = self.ai.folder.clone();
        self.sessions_task = Some(cx.spawn(async move |this, cx| {
            let list = cx.background_executor().spawn(async move { shed_ai::list_sessions(&folder) }).await;
            this.update(cx, |this, cx| {
                this.sessions = Arc::new(list);
                cx.notify();
            })
            .ok();
        }));
    }

    fn settings_msg(&self, change: impl FnOnce(&mut AiTab)) -> ClientMsg {
        let mut ai = self.ai.clone();
        change(&mut ai);
        ClientMsg::AiSettings { tab: self.tab, settings: ai }
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = &self.completion
            && let Some(&i) = c.items.get(c.selected)
        {
            return self.complete(i, cx);
        }
        let text = self.prompt.read(cx).buffer.text().trim_end().to_string();
        if text.trim().is_empty() && self.images.is_empty() {
            return;
        }
        if self.status.state != AiState::Idle {
            // One turn at a time: what is typed stays until this one ends.
            return;
        }
        let images = std::mem::take(&mut self.images);
        self.conn.send_msg(ClientMsg::AiSend { tab: self.tab, text, images });
        self.prompt.update(cx, |p, cx| {
            p.buffer.select_all();
            p.buffer.insert("");
            cx.notify();
        });
        self.history = None;
        self.completion = None;
        self.list.scroll_to_end();
        cx.notify();
    }

    /// Starts Claude Code as the user starts typing, once.
    fn warm_up(&mut self) {
        if self.commands.is_empty() && !self.warming {
            self.warming = true;
            self.conn.send_msg(ClientMsg::AiStart { tab: self.tab });
        }
    }

    fn stop(&mut self) {
        self.conn.send_msg(ClientMsg::AiStop { tab: self.tab });
    }

    /// Shows commands matching what follows `/` at the prompt's start.
    fn update_completion(&mut self, cx: &mut Context<Self>) {
        let text = self.prompt.read(cx).buffer.text();
        let word = text.strip_prefix('/').filter(|w| !w.contains(char::is_whitespace));
        self.completion = word.and_then(|w| {
            let w = w.to_lowercase();
            let mut items: Vec<usize> = (0..self.commands.len()).filter(|i| self.commands[*i].name.to_lowercase().contains(&w)).collect();
            items.sort_by_key(|i| (!self.commands[*i].name.to_lowercase().starts_with(&w), self.commands[*i].name.len()));
            items.truncate(10);
            (!items.is_empty()).then_some(Completion { items, selected: 0 })
        });
        cx.notify();
    }

    fn complete(&mut self, command: usize, cx: &mut Context<Self>) {
        let name = format!("/{} ", self.commands[command].name);
        self.prompt.update(cx, |p, cx| {
            p.buffer.select_all();
            p.buffer.insert(&name);
            cx.notify();
        });
        self.completion = None;
        cx.notify();
    }

    /// Keys at the prompt before it sees them: completion, history, pasted pictures.
    fn prompt_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let sec = ks.modifiers.secondary();
        if let Some(c) = &mut self.completion {
            match ks.key.as_str() {
                "up" => c.selected = c.selected.saturating_sub(1),
                "down" => c.selected = (c.selected + 1).min(c.items.len().saturating_sub(1)),
                "tab" => {
                    let i = c.items[c.selected];
                    self.complete(i, cx);
                }
                "escape" => self.completion = None,
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        match ks.key.as_str() {
            "escape" if self.status.state != AiState::Idle => self.stop(),
            "f" if sec => self.open_find(window, cx),
            "up" | "down" if sec => {
                let prompts: Vec<&String> = self.entries.iter().filter_map(|e| if let Entry::User { text, .. } = e { Some(text) } else { None }).collect();
                if prompts.is_empty() {
                    return;
                }
                let back = match (self.history, ks.key.as_str()) {
                    (None, "up") => 1,
                    (Some(n), "up") => (n + 1).min(prompts.len()),
                    (Some(n), _) if n > 1 => n - 1,
                    _ => {
                        self.history = None;
                        self.prompt.update(cx, |p, cx| {
                            p.buffer.select_all();
                            p.buffer.insert("");
                            cx.notify();
                        });
                        cx.stop_propagation();
                        return;
                    }
                };
                self.history = Some(back);
                let text = prompts[prompts.len() - back].clone();
                self.prompt.update(cx, |p, cx| {
                    p.buffer.select_all();
                    p.buffer.insert(&text);
                    cx.notify();
                });
            }
            "v" if sec => {
                let Some(item) = cx.read_from_clipboard() else { return };
                let image = item.entries().iter().find_map(|e| match e {
                    ClipboardEntry::Image(i) => Some(i.clone()),
                    _ => None,
                });
                let Some(image) = image else { return };
                let media_type = image.format.mime_type().to_string();
                self.images.push(AiImage { media_type, data: image.bytes });
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    /// Dropped files go into the prompt as their paths.
    fn on_drop(&mut self, paths: &ExternalPaths, cx: &mut Context<Self>) {
        let text: Vec<String> = paths.paths().iter().map(|p| p.display().to_string()).collect();
        let text = format!("{} ", text.join(" "));
        self.prompt.update(cx, |p, cx| p.paste_text(&text, cx));
    }

    /// Puts text into the prompt, as "send to AI" does.
    pub fn insert(&mut self, text: &str, cx: &mut Context<Self>) {
        self.prompt.update(cx, |p, cx| p.paste_text(text, cx));
    }

    fn open_session(&mut self, session: Option<String>) {
        self.conn.send_msg(ClientMsg::AiOpenSession { tab: self.tab, session });
    }

    /// The session in Claude Code's own terminal interface.
    fn continue_in_terminal(&self, session: &str) {
        let command = format!("claude --resume {session}");
        self.conn.send_msg(ClientMsg::NewCommandTab { bench: self.bench, cwd: self.ai.folder.clone(), command });
    }

    fn session_menu(&self, id: String) -> Vec<MenuEntry<Self>> {
        let (a, b) = (id.clone(), id.clone());
        vec![
            menu::item("Open", move |this: &mut Self, _, _| this.open_session(Some(a.clone()))).icon("go"),
            menu::item("Continue in a Terminal", move |this: &mut Self, _, _| this.continue_in_terminal(&b)).icon("terminal"),
            menu::item("Copy Session ID", move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(id.clone()))),
        ]
    }

    fn entry_menu(&self, i: usize) -> Vec<MenuEntry<Self>> {
        let text = match self.entries.get(i) {
            Some(Entry::User { text, .. } | Entry::Assistant { text } | Entry::Thinking { text } | Entry::Notice { text, .. }) => text.clone(),
            Some(Entry::Tool(t)) => format!("{}\n{}\n{}", t.summary, t.detail, t.result.clone().unwrap_or_default()),
            Some(Entry::Permission(p)) => format!("{}\n{}", p.summary, p.detail),
            None => String::new(),
        };
        let all = self.transcript_markdown();
        let mut entries = Vec::new();
        if self.selection.is_some_and(|s| !s.is_empty()) {
            entries.push(menu::item("Copy", |this: &mut Self, _, cx| this.copy_selection(cx)).icon("copy"));
            entries.push(menu::item("Copy as Markdown", |this: &mut Self, _, cx| {
                if let Some(copy) = this.selected_copy() {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy.markdown));
                }
            }));
            entries.push(MenuEntry::Separator);
        }
        let plain = if matches!(self.entries.get(i), Some(Entry::Assistant { .. })) { shed_ai::markdown::to_plain(&text) } else { text.clone() };
        entries.push(menu::item("Copy Message", move |_, _, cx| cx.write_to_clipboard(ai_text::formatted_item(text.clone(), &text))).icon("copy"));
        entries.push(menu::item("Copy Message as Plain Text", move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(plain.clone()))));
        entries.push(menu::item("Copy the Whole Conversation", move |_, _, cx| cx.write_to_clipboard(ai_text::formatted_item(all.clone(), &all))));
        entries
    }

    /// An entry's selectable text, as drawn.
    fn entry_pieces(&mut self, i: usize) -> Rc<Parsed> {
        match self.entries.get(i) {
            Some(Entry::Assistant { text }) => {
                if let Some(p) = self.parsed.get(&i).filter(|p| p.source == *text) {
                    return p.clone();
                }
                let pieces = ai_text::pieces(shed_ai::markdown::parse(text));
                let palette = &self.settings.palette;
                let code = pieces
                    .iter()
                    .map(|p| match &p.kind {
                        Kind::Code { lang, .. } => code_colors(lang, &p.text.text, palette),
                        _ => None,
                    })
                    .collect();
                let parsed = Rc::new(Parsed { source: text.clone(), pieces, code });
                self.parsed.insert(i, parsed.clone());
                parsed
            }
            Some(Entry::User { text, .. }) => {
                let lines: Vec<&str> = text.lines().collect();
                let shown = if lines.len() > FOLD_LINES && !self.expanded.contains(&i) { lines[..FOLD_LINES].join("\n") } else { text.clone() };
                Rc::new(Parsed::plain(shown))
            }
            Some(Entry::Thinking { text }) if self.expanded.contains(&i) => Rc::new(Parsed::plain(text.clone())),
            _ => Rc::new(Parsed { source: String::new(), pieces: Vec::new(), code: Vec::new() }),
        }
    }

    fn selected_copy(&mut self) -> Option<Copy> {
        let sel = self.selection.filter(|s| !s.is_empty())?;
        let (a, b) = sel.ordered();
        let mut copy = Copy::default();
        for i in a.entry..=b.entry {
            let parsed = self.entry_pieces(i);
            for (j, piece) in parsed.pieces.iter().enumerate() {
                if let Some(range) = sel.range_in(i, j, piece.text.text.len()) {
                    copy.add(piece, range);
                }
            }
            copy.next_entry();
        }
        Some(copy)
    }

    /// Copies the selection: as text, and formatted for where that is kept.
    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(copy) = self.selected_copy() {
            cx.write_to_clipboard(ai_text::formatted_item(copy.plain, &copy.markdown));
        }
    }

    /// Keys anywhere in the tab, before the prompt: copying the selection.
    fn selection_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let Some(sel) = self.selection else { return };
        match ks.key.as_str() {
            "c" if ks.modifiers.secondary() && !sel.is_empty() && self.prompt.read(cx).buffer.selected_text().is_empty() => {
                self.copy_selection(cx);
                cx.stop_propagation();
            }
            "escape" => {
                self.selection = None;
                cx.notify();
            }
            _ => {}
        }
    }

    /// Shows "Copied" on the button `key` for a moment.
    fn copied(&mut self, key: SharedString, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1500)).await;
            this.update(cx, |this, cx| {
                this.copied = None;
                cx.notify();
            })
            .ok();
        });
        self.copied = Some((key, task));
        cx.notify();
    }

    /// A small "Copy" button that copies `item`.
    fn copy_button(&self, key: SharedString, item: ClipboardItem, ui: &UiColors, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let done = self.copied.as_ref().is_some_and(|(k, _)| *k == key);
        div()
            .id(key.clone())
            .flex()
            .items_center()
            .gap_1()
            .px_1p5()
            .rounded_sm()
            .cursor_pointer()
            .text_size(px(11.0))
            .text_color(hsla(ui.text_dim))
            .hover(|s| s.bg(hsla(ui.tab_hover)).text_color(hsla(ui.text)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.write_to_clipboard(item.clone());
                this.copied(key.clone(), cx);
            }))
            .child(svg().path(icon("copy")).size(px(12.0)).text_color(hsla(ui.text_dim)))
            .child(if done { "Copied" } else { "Copy" })
    }

    /// A piece of text that can be selected with the mouse.
    fn sel_text(&self, i: usize, j: usize, parsed: &Rc<Parsed>, styles: Vec<(Range<usize>, HighlightStyle)>, cx: &mut Context<Self>) -> gpui::Div {
        let text = &parsed.pieces[j].text.text;
        let selected = self.selection.and_then(|s| s.range_in(i, j, text.len()));
        let sel_style = HighlightStyle { background_color: Some(hsla_alpha(self.accent, 0.35)), ..Default::default() };
        let styled = StyledText::new(SharedString::from(text.clone())).with_highlights(ai_text::with_selection(styles, selected, sel_style));
        let (down, moved, up) = (styled.layout().clone(), styled.layout().clone(), styled.layout().clone());
        let (p1, p2) = (parsed.clone(), parsed.clone());
        div()
            .cursor(CursorStyle::IBeam)
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                let at = text_index(&down, e.position);
                let point = |offset| Point { entry: i, piece: j, offset };
                let text = &p1.pieces[j].text.text;
                this.selection = Some(match e.click_count {
                    2 => {
                        let word = ai_text::word_at(text, at);
                        Selection { anchor: point(word.start), head: point(word.end) }
                    }
                    n if n >= 3 => Selection { anchor: point(0), head: point(text.len()) },
                    _ => match this.selection {
                        Some(s) if e.modifiers.shift => Selection { anchor: s.anchor, head: point(at) },
                        _ => Selection::at(point(at)),
                    },
                });
                this.selecting = true;
                this.dragged = e.click_count > 1;
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_move(cx.listener(move |this, e: &MouseMoveEvent, _, cx| {
                if !this.selecting || e.pressed_button != Some(MouseButton::Left) || !moved.bounds().contains(&e.position) {
                    return;
                }
                let head = Point { entry: i, piece: j, offset: text_index(&moved, e.position) };
                if let Some(s) = &mut this.selection
                    && s.head != head
                {
                    s.head = head;
                    this.dragged = true;
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(move |this, e: &MouseUpEvent, _, cx| {
                // A click on a link opens it.
                if !this.dragged
                    && let Some(url) = ai_text::link_at(&p2.pieces[j].text, text_index(&up, e.position))
                {
                    cx.open_url(url);
                }
            }))
            .child(styled)
    }

    /// The pieces of an entry, drawn: Markdown blocks, tables, code.
    fn render_pieces(&mut self, i: usize, parsed: &Rc<Parsed>, ui: &UiColors, mono: &str, cx: &mut Context<Self>) -> gpui::Div {
        let ink = ui.ink(self.accent);
        let mut out = div().flex().flex_col().gap_2().line_height(relative(1.45)).text_color(hsla(ui.text));
        let mut j = 0;
        let n = parsed.pieces.len();
        while j < n {
            let piece = &parsed.pieces[j];
            let styles = ai_text::highlights(&piece.text, ink, ui.divider);
            match &piece.kind {
                Kind::Para => {
                    out = out.child(self.sel_text(i, j, parsed, styles, cx));
                    j += 1;
                }
                Kind::Heading(level) => {
                    let size = match level {
                        1 => 18.0,
                        2 => 16.0,
                        3 => 14.5,
                        _ => 13.5,
                    };
                    out = out.child(div().mt_1().text_size(px(size)).font_weight(FontWeight::BOLD).child(self.sel_text(i, j, parsed, styles, cx)));
                    j += 1;
                }
                Kind::Quote => {
                    out = out.child(div().pl_3().border_l_2().border_color(hsla(ui.divider)).text_color(hsla(ui.text_dim)).child(self.sel_text(i, j, parsed, styles, cx)));
                    j += 1;
                }
                Kind::Rule => {
                    out = out.child(div().my_1().h(px(1.0)).bg(hsla(ui.divider)));
                    j += 1;
                }
                Kind::Item { .. } => {
                    // A list: its items closer together than paragraphs.
                    let mut items = div().flex().flex_col().gap_1();
                    while let Some(Piece { kind: Kind::Item { depth, marker }, text }) = parsed.pieces.get(j) {
                        let styles = ai_text::highlights(text, ink, ui.divider);
                        let numbered = marker.ends_with('.');
                        let bullet = div()
                            .flex_none()
                            .min_w(px(if numbered { 18.0 } else { 12.0 }))
                            .text_color(hsla(if marker.starts_with('☑') { ink } else { ui.text_dim }))
                            .child(marker.clone());
                        items = items.child(div().flex().gap_1p5().pl(px(4.0 + *depth as f32 * 18.0)).child(bullet).child(div().flex_1().min_w_0().child(self.sel_text(i, j, parsed, styles, cx))));
                        j += 1;
                        // A code block inside an item stays in the list.
                        while let Some(Piece { kind: Kind::Code { depth, .. }, .. }) = parsed.pieces.get(j) {
                            let indent = 4.0 + *depth as f32 * 18.0;
                            items = items.child(div().pl(px(indent)).child(self.render_code(i, j, parsed, ui, mono, cx)));
                            j += 1;
                        }
                    }
                    out = out.child(items);
                }
                Kind::Code { .. } => {
                    out = out.child(self.render_code(i, j, parsed, ui, mono, cx));
                    j += 1;
                }
                Kind::Cell { cols, .. } => {
                    let cols = *cols;
                    let mut table = div().flex().flex_col().rounded_sm().border_1().border_color(hsla(ui.divider)).text_size(px(12.5));
                    let mut row_el: Option<(usize, gpui::Div)> = None;
                    while let Some(Piece { kind: Kind::Cell { row, align, .. }, text }) = parsed.pieces.get(j) {
                        let (row, align) = (*row, *align);
                        if row_el.as_ref().is_none_or(|(r, _)| *r != row) {
                            if let Some((_, el)) = row_el.take() {
                                table = table.child(el);
                            }
                            let el = div().flex().when(row > 0, |d| d.border_t_1().border_color(hsla(ui.divider))).when(row == 0, |d| d.bg(hsla_alpha(ui.divider, 0.35)).font_weight(FontWeight::BOLD));
                            row_el = Some((row, el));
                        }
                        let styles = ai_text::highlights(text, ink, ui.divider);
                        let cell = div()
                            .flex_1()
                            .min_w_0()
                            .px_2()
                            .py_0p5()
                            .flex()
                            .when(align == shed_ai::markdown::Align::Right, |d| d.justify_end())
                            .when(align == shed_ai::markdown::Align::Center, |d| d.justify_center())
                            .child(self.sel_text(i, j, parsed, styles, cx));
                        if let Some((_, el)) = row_el.take() {
                            row_el = Some((row, el.child(cell)));
                        }
                        j += 1;
                    }
                    if let Some((_, el)) = row_el {
                        table = table.child(el);
                    }
                    let _ = cols;
                    out = out.child(table);
                }
            }
        }
        out
    }

    /// A code block: its language and a copy button above, the code coloured.
    fn render_code(&mut self, i: usize, j: usize, parsed: &Rc<Parsed>, ui: &UiColors, mono: &str, cx: &mut Context<Self>) -> gpui::Div {
        let piece = &parsed.pieces[j];
        let lang = match &piece.kind {
            Kind::Code { lang, .. } => lang.clone(),
            _ => String::new(),
        };
        let styles: Vec<(Range<usize>, HighlightStyle)> = parsed.code.get(j).cloned().flatten().unwrap_or_default().into_iter().map(|(r, c)| (r, HighlightStyle::color(hsla(c)))).collect();
        let copy = self.copy_button(format!("copy-code-{i}-{j}").into(), ClipboardItem::new_string(piece.text.text.clone()), ui, cx);
        div()
            .rounded_md()
            .border_1()
            .border_color(hsla(ui.divider))
            .bg(hsla_alpha(ui.background, 0.8))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pl_2()
                    .pr_1()
                    .py_0p5()
                    .border_b_1()
                    .border_color(hsla(ui.divider))
                    .text_size(px(11.0))
                    .text_color(hsla(ui.text_dim))
                    .child(if lang.is_empty() { "code".to_string() } else { lang })
                    .child(copy),
            )
            .child(div().px_2().py_1().font_family(mono.to_string()).text_size(px(12.0)).line_height(relative(1.35)).child(self.sel_text(i, j, parsed, styles, cx)))
    }

    fn transcript_markdown(&self) -> String {
        self.entries
            .iter()
            .filter_map(|e| match e {
                Entry::User { text, .. } => Some(format!("**You:**\n\n{text}")),
                Entry::Assistant { text } => Some(text.clone()),
                Entry::Tool(t) => Some(format!("`{}`", t.summary)),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn model_menu(&self) -> Vec<MenuEntry<Self>> {
        let current = self.ai.model.clone();
        let mut entries: Vec<MenuEntry<Self>> = self
            .models
            .iter()
            .map(|m| {
                let value = (m.value != "default").then(|| m.value.clone());
                let check = if value == current { "  ✓" } else { "" };
                menu::item(format!("{}{check}", m.name), move |this: &mut Self, _, _| {
                    let msg = this.settings_msg(|a| a.model = value.clone());
                    this.conn.send_msg(msg);
                })
            })
            .collect();
        if entries.is_empty() {
            entries.push(menu::item("Models show after the first prompt", |_, _, _| {}));
        }
        let efforts = self.models.iter().find(|m| m.value == current.clone().unwrap_or_else(|| "default".into())).map(|m| m.efforts.clone()).unwrap_or_default();
        if !efforts.is_empty() {
            entries.push(MenuEntry::Separator);
            for effort in efforts {
                let check = if self.ai.effort.as_deref() == Some(&effort) { "  ✓" } else { "" };
                let e = effort.clone();
                entries.push(menu::item(format!("Effort: {effort}{check}"), move |this: &mut Self, _, _| {
                    let msg = this.settings_msg(|a| a.effort = Some(e.clone()));
                    this.conn.send_msg(msg);
                }));
            }
        }
        entries
    }

    fn mode_menu(&self) -> Vec<MenuEntry<Self>> {
        [("default", "Ask Before Edits"), ("acceptEdits", "Accept Edits"), ("plan", "Plan Only")]
            .into_iter()
            .map(|(mode, label)| {
                let check = if self.ai.mode == mode { "  ✓" } else { "" };
                menu::item(format!("{label}{check}"), move |this: &mut Self, _, _| {
                    let msg = this.settings_msg(|a| a.mode = mode.into());
                    this.conn.send_msg(msg);
                })
            })
            .collect()
    }

    fn render_entry(&mut self, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let ui = current_ui(cx);
        let accent = self.accent;
        let mono = self.settings.font.family.clone();
        let Some(entry) = self.entries.get(i).cloned() else { return div().into_any_element() };
        let open = self.expanded.contains(&i);
        let toggle = cx.listener(move |this, _, _, cx| {
            if !this.expanded.remove(&i) {
                this.expanded.insert(i);
            }
            this.list.remeasure_items(i..i + 1);
            cx.notify();
        });
        let body = match entry {
            Entry::User { text, images } => {
                let lines: Vec<&str> = text.lines().collect();
                div()
                    .id(("user", i))
                    .mt_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(hsla_alpha(accent, 0.12))
                    .border_l_2()
                    .border_color(hsla(accent))
                    .text_color(hsla(ui.text))
                    .child({
                        let parsed = self.entry_pieces(i);
                        self.sel_text(i, 0, &parsed, Vec::new(), cx)
                    })
                    .when(images > 0, |d| d.child(div().mt_1().text_size(px(11.0)).text_color(hsla(ui.text_dim)).child(format!("{images} picture(s) attached"))))
                    .when(lines.len() > FOLD_LINES, |d| {
                        let label = if open { "Show less".to_string() } else { format!("Show all {} lines", lines.len()) };
                        d.child(div().id(("fold", i)).mt_1().cursor_pointer().text_size(px(11.0)).text_color(hsla(ui.ink(accent))).on_click(toggle).child(label))
                    })
                    .into_any_element()
            }
            Entry::Assistant { text } => {
                let parsed = self.entry_pieces(i);
                let copy = self.copy_button(format!("copy-entry-{i}").into(), ai_text::formatted_item(text.clone(), &text), &ui, cx);
                let group: SharedString = format!("answer-{i}").into();
                div()
                    .group(group.clone())
                    .flex()
                    .flex_col()
                    .child(self.render_pieces(i, &parsed, &ui, &mono, cx))
                    .child(div().flex().justify_end().invisible().group_hover(group, |s| s.visible()).child(copy))
                    .into_any_element()
            }
            Entry::Thinking { text } => div()
                .id(("thinking", i))
                .cursor_pointer()
                .text_size(px(12.0))
                .text_color(hsla(ui.text_dim))
                .on_click(toggle)
                .child(if open { "▾ Thinking" } else { "▸ Thinking" })
                .when(open, |d| {
                    let _ = &text;
                    let parsed = self.entry_pieces(i);
                    d.child(div().pl_3().child(self.sel_text(i, 0, &parsed, Vec::new(), cx)))
                })
                .into_any_element(),
            Entry::Tool(t) => {
                let (mark, color) = match (&t.result, t.error) {
                    (None, _) => ("○", ui.text_dim),
                    (Some(_), false) => ("✓", ui.ink(accent)),
                    (Some(_), true) => ("✗", ui.danger),
                };
                let mut el = div().id(("tool", i)).flex().flex_col().text_size(px(12.0)).child(
                    div()
                        .id(("tool-head", i))
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .rounded_sm()
                        .hover(|s| s.bg(hsla(ui.tab_hover)))
                        .on_click(toggle)
                        .child(div().flex_none().w(px(12.0)).text_color(hsla(color)).child(mark))
                        .child(div().flex_1().min_w_0().truncate().font_family(mono.clone()).text_color(hsla(ui.text_dim)).child(t.summary.clone())),
                );
                if open {
                    if !t.detail.is_empty() {
                        el = el.child(pre(&t.detail, &ui, &mono, None));
                    }
                    if let Some(diff) = &t.diff {
                        el = el.child(diff_view(&diff.old, &diff.new, &ui, &mono));
                    }
                    if let Some(result) = &t.result {
                        let lines: Vec<&str> = result.lines().collect();
                        let mut shown = lines[..lines.len().min(RESULT_LINES)].join("\n");
                        if lines.len() > RESULT_LINES {
                            shown.push_str(&format!("\n… {} more lines", lines.len() - RESULT_LINES));
                        }
                        el = el.child(pre(&shown, &ui, &mono, t.error.then_some(ui.danger)));
                    }
                }
                el.into_any_element()
            }
            Entry::Permission(p) => {
                let request = p.request.clone();
                let answer = |label: &'static str, allow: bool, always: bool, color: Rgb| {
                    let request = request.clone();
                    div()
                        .id(label)
                        .px_2()
                        .py_0p5()
                        .rounded_sm()
                        .border_1()
                        .border_color(hsla(color))
                        .text_color(hsla(ui.ink(color)))
                        .cursor_pointer()
                        .hover(|s| s.bg(hsla_alpha(color, 0.2)))
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, _| {
                            this.conn.send_msg(ClientMsg::AiAnswer { tab: this.tab, request: request.clone(), allow, always });
                        }))
                };
                let mut card = div()
                    .id(("permission", i))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(hsla(if p.answer.is_none() { accent } else { ui.divider }))
                    .child(div().text_color(hsla(ui.text)).child(format!("Claude asks to {}", lower_first(&p.summary))));
                if let Some(diff) = &p.diff {
                    card = card.child(diff_view(&diff.old, &diff.new, &ui, &mono));
                } else if !p.detail.is_empty() {
                    card = card.child(pre(&p.detail, &ui, &mono, None));
                }
                card = match p.answer {
                    None => card.child(
                        div()
                            .flex()
                            .gap_2()
                            .text_size(px(12.0))
                            .child(answer("Allow", true, false, accent))
                            .child(answer("Allow for This Session", true, true, accent))
                            .child(answer("Deny", false, false, ui.danger)),
                    ),
                    Some(allowed) => card.child(div().text_size(px(12.0)).text_color(hsla(ui.text_dim)).child(if allowed { "Allowed" } else { "Denied" })),
                };
                card.into_any_element()
            }
            Entry::Notice { text, error } => div()
                .text_size(px(11.0))
                .text_color(hsla(if error { ui.danger } else { ui.text_dim }))
                .child(text)
                .into_any_element(),
        };
        let found = self.find.as_ref().is_some_and(|f| f.1 == Some(i));
        div()
            .id(("entry", i))
            .px_4()
            .py_1()
            .w_full()
            .when(found, |d| d.border_l_2().border_color(hsla(ui.ink(self.accent))).bg(hsla_alpha(self.accent, 0.08)))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                let entries = this.entry_menu(i);
                this.open_menu(e.position, entries, cx);
            }))
            .child(body)
            .into_any_element()
    }

    fn render_sessions(&mut self, range: std::ops::Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let ui = current_ui(cx);
        let accent = self.accent;
        let shown = self.shown_sessions(cx);
        range
            .filter_map(|n| {
                let s = shown.get(n)?;
                let current = self.ai.session.as_deref() == Some(&s.id);
                let id = s.id.clone();
                let menu_id = s.id.clone();
                let mut meta = vec![ago(s.modified)];
                if let Some(b) = &s.branch {
                    meta.push(b.clone());
                }
                meta.push(format!("{} prompt{}", s.prompts, if s.prompts == 1 { "" } else { "s" }));
                Some(
                    div()
                        .id(("session", n))
                        .h(px(44.0))
                        .px_2()
                        .flex()
                        .flex_col()
                        .justify_center()
                        .rounded_sm()
                        .cursor_pointer()
                        .when(current, |d| d.bg(hsla_alpha(accent, 0.2)))
                        .hover(|st| st.bg(hsla(ui.tab_hover)))
                        .on_click(cx.listener(move |this, _, _, _| this.open_session(Some(id.clone()))))
                        .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                            let entries = this.session_menu(menu_id.clone());
                            this.open_menu(e.position, entries, cx);
                        }))
                        .child(div().truncate().text_color(hsla(ui.text)).child(s.title.clone()))
                        .child(div().truncate().text_size(px(11.0)).text_color(hsla(ui.text_dim)).child(meta.join(" · ")))
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn shown_sessions(&self, cx: &gpui::App) -> Vec<SessionInfo> {
        let words: Vec<String> = self.filter.read(cx).text().split_whitespace().map(str::to_lowercase).collect();
        self.sessions.iter().filter(|s| words.iter().all(|w| s.title.to_lowercase().contains(w))).cloned().collect()
    }

    fn render_header(&self, ui: &UiColors, limits: Limits, cx: &mut Context<Self>) -> impl IntoElement {
        let accent = self.accent;
        let folder = self.ai.folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let title = self
            .ai
            .session
            .as_ref()
            .and_then(|id| self.sessions.iter().find(|s| &s.id == id))
            .map(|s| s.title.clone())
            .unwrap_or_else(|| if self.ai.session.is_some() { "Session".into() } else { "New session".into() });
        let window = |label: &str, w: Option<shed_ai::Window>| {
            w.map(|w| {
                let pct = (w.used * 100.0).round() as u32;
                let color = if pct >= 90 { ui.danger } else if pct >= 70 { ui.ink(accent) } else { ui.text_dim };
                div().text_color(hsla(color)).child(format!("{label} {pct}% · resets {}", until(w.resets)))
            })
        };
        let mut chips = div().flex().flex_wrap().items_center().gap_1().text_size(px(11.0));
        let others: Vec<PathBuf> = self.projects.iter().filter(|p| **p != self.ai.folder).cloned().collect();
        if !others.is_empty() {
            chips = chips.child(div().text_color(hsla(ui.text_dim)).child("Also works in:"));
        }
        for (n, dir) in others.into_iter().enumerate() {
            let on = self.ai.add_dirs.contains(&dir);
            let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            chips = chips.child(
                div()
                    .id(("dir", n))
                    .px_1p5()
                    .rounded_sm()
                    .border_1()
                    .border_color(hsla(if on { accent } else { ui.divider }))
                    .when(on, |d| d.bg(hsla_alpha(accent, 0.2)))
                    .text_color(hsla(if on { ui.text } else { ui.text_dim }))
                    .cursor_pointer()
                    .tooltip(menu::tooltip("Let Claude read and change this project too (from the next prompt)"))
                    .on_click(cx.listener(move |this, _, _, _| {
                        let dir = dir.clone();
                        let msg = this.settings_msg(|a| {
                            if !a.add_dirs.contains(&dir) {
                                a.add_dirs.push(dir.clone());
                            } else {
                                a.add_dirs.retain(|d| *d != dir);
                            }
                        });
                        this.conn.send_msg(msg);
                    }))
                    .child(name),
            );
        }
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap_1()
            .px_3()
            .py_1p5()
            .border_b_1()
            .border_color(hsla(ui.divider))
            .bg(hsla(ui.background))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(svg().path(icon("ai")).size(px(16.0)).flex_none().text_color(hsla(ui.ink(accent))))
                    .child(div().flex_none().text_size(px(14.0)).text_color(hsla(ui.text)).child(title))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(12.0)).text_color(hsla(ui.text_dim)).child(format!("{folder} · {}", shed_save::home_relative(&self.ai.folder))))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .gap_3()
                            .text_size(px(11.0))
                            .children(window("5 h", limits.five_hour))
                            .children(window("week", limits.week))
                            .when(self.status.cost > 0.0, |d| d.child(div().text_color(hsla(ui.text_dim)).child(format!("${:.2}", self.status.cost))))
                            .when(self.status.context > 0, |d| d.child(div().text_color(hsla(ui.text_dim)).child(format!("{}k context", self.status.context / 1000)))),
                    ),
            )
            .child(chips)
    }

    fn render_footer(&self, ui: &UiColors, cx: &mut Context<Self>) -> impl IntoElement {
        let accent = self.accent;
        let working = self.status.state != AiState::Idle;
        let status = match self.status.state {
            AiState::Idle => String::new(),
            AiState::Working => format!("Working… {}", self.status.since.map(secs_since).unwrap_or_default()),
            AiState::Waiting => "Waiting for your answer".into(),
        };
        let chip = |id: &'static str, label: String| {
            div()
                .id(id)
                .px_1p5()
                .rounded_sm()
                .cursor_pointer()
                .text_color(hsla(ui.text_dim))
                .hover(|s| s.bg(hsla(ui.tab_hover)).text_color(hsla(ui.text)))
                .child(label)
        };
        let model = self
            .ai
            .model
            .as_ref()
            .and_then(|m| self.models.iter().find(|x| &x.value == m))
            .map(|m| m.name.clone())
            .or_else(|| self.ai.model.clone())
            .unwrap_or_else(|| "Default model".into());
        let mode = match self.ai.mode.as_str() {
            "acceptEdits" => "Accept edits",
            "plan" => "Plan only",
            _ => "Ask before edits",
        };
        let (glyph, label, color) = if working { ("stop", "Stop", ui.danger) } else { ("send", "Send", accent) };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .text_size(px(12.0))
            .child(div().flex_1().text_color(hsla(if self.status.state == AiState::Waiting { ui.ink(accent) } else { ui.text_dim })).child(status))
            .child(chip("ai-model", model).on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                this.warm_up();
                let entries = this.model_menu();
                this.open_menu(e.position, entries, cx);
            })))
            .child(chip("ai-mode", mode.into()).on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                let entries = this.mode_menu();
                this.open_menu(e.position, entries, cx);
            })))
            .child(
                div()
                    .id("ai-more")
                    .p_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(hsla(ui.tab_hover)))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                        let entries = this.more_menu(cx);
                        this.open_menu(e.position, entries, cx);
                    }))
                    .child(svg().path(icon("more")).size(px(14.0)).text_color(hsla(ui.text_dim))),
            )
            .child(
                div()
                    .id("ai-follow")
                    .p_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .when(self.ai.follow, |d| d.bg(hsla_alpha(accent, 0.25)))
                    .hover(|s| s.bg(hsla(ui.tab_hover)))
                    .tooltip(menu::tooltip("Follow edits: files Claude changes open in their project tab"))
                    .on_click(cx.listener(|this, _, _, _| {
                        let msg = this.settings_msg(|a| a.follow = !a.follow);
                        this.conn.send_msg(msg);
                    }))
                    .child(svg().path(icon("eye")).size(px(14.0)).text_color(hsla(if self.ai.follow { ui.ink(accent) } else { ui.text_dim }))),
            )
            .child(
                div()
                    .id("ai-send")
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .border_1()
                    .border_color(hsla(color))
                    .text_color(hsla(ui.ink(color)))
                    .cursor_pointer()
                    .hover(|s| s.bg(hsla_alpha(color, 0.2)))
                    .on_click(cx.listener(move |this, _, _, cx| if working { this.stop() } else { this.send(cx) }))
                    .child(svg().path(icon(glyph)).size(px(13.0)).text_color(hsla(ui.ink(color))))
                    .child(label),
            )
    }

    fn render_completion(&self, ui: &UiColors, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let c = self.completion.as_ref()?;
        let accent = self.accent;
        Some(
            div()
                .flex()
                .flex_col()
                .mx_3()
                .p_1()
                .rounded_md()
                .bg(hsla(ui.tab_active))
                .border_1()
                .border_color(hsla(ui.divider))
                .text_size(px(12.0))
                .children(c.items.iter().enumerate().map(|(n, &i)| {
                    let cmd = &self.commands[i];
                    let description: String = cmd.description.chars().take(110).collect();
                    div()
                        .id(("completion", n))
                        .flex()
                        .gap_2()
                        .px_2()
                        .py_0p5()
                        .rounded_sm()
                        .cursor_pointer()
                        .when(n == c.selected, |d| d.bg(hsla_alpha(accent, 0.25)))
                        .on_click(cx.listener(move |this, _, _, cx| this.complete(i, cx)))
                        .child(div().flex_none().text_color(hsla(ui.text)).child(format!("/{}", cmd.name)))
                        .child(div().flex_1().min_w_0().truncate().text_color(hsla(ui.text_dim)).child(description))
                })),
        )
    }
}

impl AiView {
    /// Above the prompt: find, the task list, and the files the last turn changed.
    fn render_extras(&self, ui: &UiColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let accent = self.accent;
        let mut out = div().flex().flex_col().gap_1().mx_3().text_size(px(12.0));
        let mut any = false;
        if let Some((editor, at, _)) = &self.find {
            any = true;
            let status = match at {
                Some(i) => format!("entry {}", i + 1),
                None => "Enter finds the next".into(),
            };
            out = out.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .capture_key_down(cx.listener(Self::find_key))
                    .child(div().text_color(hsla(ui.text_dim)).child("Find:"))
                    .child(div().w(px(280.0)).child(editor.clone()))
                    .child(div().text_color(hsla(ui.text_dim)).child(status)),
            );
        }
        if let Some(tasks) = self.tasks() {
            any = true;
            let lines: Vec<String> = tasks.lines().map(String::from).collect();
            let done = lines.iter().filter(|l| l.starts_with('☑')).count();
            let current = lines.iter().find(|l| l.starts_with('▶')).cloned().unwrap_or_default();
            out = out.child(
                div()
                    .id("ai-tasks")
                    .flex()
                    .flex_col()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(hsla(ui.divider))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.tasks_open = !this.tasks_open;
                        cx.notify();
                    }))
                    .child(div().text_color(hsla(ui.text_dim)).child(format!("Tasks {done}/{} {}", lines.len(), current)))
                    .when(self.tasks_open, |d| d.children(lines.into_iter().map(|l| div().text_color(hsla(ui.text)).child(l)))),
            );
        }
        let changed = if self.status.state == AiState::Idle { self.turn_changes() } else { Vec::new() };
        if !changed.is_empty() {
            any = true;
            let mut row = div().flex().flex_wrap().items_center().gap_1().child(div().text_color(hsla(ui.text_dim)).child("Changed, to review:"));
            for (n, file) in changed.into_iter().enumerate() {
                let name = std::path::Path::new(&file).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                row = row.child(
                    div()
                        .id(("changed", n))
                        .px_1p5()
                        .rounded_sm()
                        .border_1()
                        .border_color(hsla_alpha(accent, 0.5))
                        .text_color(hsla(ui.ink(accent)))
                        .cursor_pointer()
                        .hover(|s| s.bg(hsla_alpha(accent, 0.15)))
                        .tooltip(menu::tooltip("Show the change in its project tab"))
                        .on_click(cx.listener(move |this, _, _, cx| this.review(file.clone(), cx)))
                        .child(name),
                );
            }
            out = out.child(row);
        }
        any.then(|| out.into_any_element())
    }
}

impl MenuHost for AiView {
    fn menu_slot(&mut self) -> &mut Option<Menu<Self>> {
        &mut self.menu
    }

    fn menu_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.focus_target(cx);
        window.focus(&target, cx);
    }
}

impl Render for AiView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = current_ui(cx);
        let limits = cx.try_global::<AiLimits>().map(|l| l.0).unwrap_or_default();
        let menu = self.menu.as_ref().map(|m| menu::render(m, &ui, cx));
        let count = self.shown_sessions(cx).len();
        let sessions = uniform_list("ai-sessions", count, cx.processor(|this, range, _, cx| this.render_sessions(range, cx)))
            .track_scroll(&self.sessions_scroll)
            .flex_1()
            .min_h_0();
        let transcript = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            // A click beside the text lets go of the selection.
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                if this.selection.take().is_some() {
                    cx.notify();
                }
            }))
            .child(list(self.list.clone(), cx.processor(|this, i, _, cx| this.render_entry(i, cx))).flex_1().min_h_0().py_2());
        let empty = self.entries.is_empty();
        let images = self.images.len();
        div()
            .key_context("AiTab")
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::selection_key))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| {
                if std::mem::take(&mut this.selecting) && this.dragged {
                    // X11 convention: selecting copies to the primary selection.
                    #[cfg(target_os = "linux")]
                    if let Some(copy) = this.selected_copy() {
                        cx.write_to_primary(ClipboardItem::new_string(copy.plain));
                    }
                    #[cfg(not(target_os = "linux"))]
                    let _ = &cx;
                }
            }))
            .relative()
            .size_full()
            .flex()
            .text_size(px(13.0))
            .child(
                div()
                    .w(px(SESSIONS_WIDTH))
                    .flex_none()
                    .h_full()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .bg(hsla(ui.background))
                    .border_r_1()
                    .border_color(hsla(ui.divider))
                    .child(
                        div()
                            .id("new-session")
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(hsla(ui.divider))
                            .cursor_pointer()
                            .text_color(hsla(ui.text))
                            .hover(|s| s.bg(hsla(ui.tab_hover)))
                            .on_click(cx.listener(|this, _, _, _| this.open_session(None)))
                            .child(svg().path(icon("ai")).size(px(14.0)).text_color(hsla(ui.ink(self.accent))))
                            .child("New Session"),
                    )
                    .child(div().text_size(px(12.0)).child(self.filter.clone()))
                    .child(sessions),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(self.render_header(&ui, limits, cx))
                    .child(if empty {
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap_2()
                            .text_color(hsla(ui.text_dim))
                            .child(svg().path(icon("ai")).size(px(32.0)).text_color(hsla(ui.divider)))
                            .child(format!("Claude Code works in {}", shed_save::home_relative(&self.ai.folder)))
                            .into_any_element()
                    } else {
                        transcript.into_any_element()
                    })
                    .children(self.render_extras(&ui, cx))
                    .children(self.render_completion(&ui, cx))
                    .when(images > 0, |d| {
                        d.child(
                            div()
                                .id("ai-images")
                                .mx_3()
                                .flex()
                                .gap_2()
                                .items_center()
                                .text_size(px(11.0))
                                .text_color(hsla(ui.text_dim))
                                .child(format!("{images} picture(s) will go with the prompt"))
                                .child(div().id("drop-images").cursor_pointer().text_color(hsla(ui.ink(self.accent))).on_click(cx.listener(|this, _, _, cx| {
                                    this.images.clear();
                                    cx.notify();
                                })).child("Remove")),
                        )
                    })
                    .child(
                        div()
                            .mx_3()
                            .mt_1()
                            .rounded_md()
                            .border_1()
                            .border_color(hsla(ui.divider))
                            .bg(hsla(ui.tab_active))
                            .capture_key_down(cx.listener(Self::prompt_key))
                            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.on_drop(paths, cx)))
                            .child(self.prompt.clone()),
                    )
                    .child(self.render_footer(&ui, cx)),
            )
            .children(menu)
    }
}

/// Text as written, in the monospaced font, on a darker ground.
fn pre(text: &str, ui: &UiColors, mono: &str, color: Option<Rgb>) -> impl IntoElement {
    div()
        .my_1()
        .px_2()
        .py_1()
        .rounded_sm()
        .bg(hsla_alpha(ui.background, 0.8))
        .border_1()
        .border_color(hsla(ui.divider))
        .font_family(mono.to_string())
        .text_size(px(12.0))
        .text_color(hsla(color.unwrap_or(ui.text)))
        .children(text.lines().map(|l| div().min_h(px(16.0)).child(l.to_string())))
}

/// An edit: the lines taken out, then the lines put in.
fn diff_view(old: &str, new: &str, ui: &UiColors, mono: &str) -> impl IntoElement {
    let red = Rgb::hex(0xE5534B);
    let green = Rgb::hex(0x57AB5A);
    let lines = |text: &str, sign: &'static str, color: Rgb| {
        text.lines().take(60).map(move |l| div().min_h(px(16.0)).bg(hsla_alpha(color, 0.15)).child(format!("{sign} {l}"))).collect::<Vec<_>>()
    };
    div()
        .my_1()
        .rounded_sm()
        .border_1()
        .border_color(hsla(ui.divider))
        .font_family(mono.to_string())
        .text_size(px(12.0))
        .text_color(hsla(ui.text))
        .children(lines(old, "-", red))
        .children(lines(new, "+", green))
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().chain(c).collect(),
        None => String::new(),
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// How long ago, shortly: `5 min ago`, `3 h ago`, `2 d ago`.
pub fn ago(secs: u64) -> String {
    let d = unix_now().saturating_sub(secs);
    match d {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", d / 60),
        3600..86400 => format!("{} h ago", d / 3600),
        _ => format!("{} d ago", d / 86400),
    }
}

/// How long until: `in 2 h 10 min`.
fn until(secs: u64) -> String {
    let d = secs.saturating_sub(unix_now());
    match d {
        0..60 => "soon".into(),
        60..3600 => format!("in {} min", d / 60),
        3600..86400 => format!("in {} h {} min", d / 3600, d % 3600 / 60),
        _ => format!("in {} d", d / 86400),
    }
}

fn secs_since(start: u64) -> String {
    let d = unix_now().saturating_sub(start);
    if d < 60 { format!("{d} s") } else { format!("{} min {} s", d / 60, d % 60) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_and_times() {
        assert_eq!(lower_first("Edit x"), "edit x");
        assert!(ago(unix_now() - 120).starts_with("2 min"));
    }
}
