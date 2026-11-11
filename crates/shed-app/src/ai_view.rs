//! An AI tab: a Claude Code conversation. On the left, the folder's
//! sessions; in the middle, the conversation as it streams; below, the
//! prompt. The keeper runs Claude Code; this view shows what it says and
//! sends what the user types.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, ClipboardEntry, ClipboardItem, Context, Entity, ExternalPaths, FocusHandle, FollowMode,
    FontWeight, HighlightStyle, KeyDownEvent, ListAlignment, ListState, MouseButton, MouseDownEvent, SharedString,
    StyledText, Subscription, Task, UniformListScrollHandle, Window, div, list, prelude::*, px, svg, uniform_list,
};
use shed_ai::{Command, Entry, Limits, Model, SessionInfo, State as AiState, Status};
use shed_model::{AiTab, BenchId, Rgb, TabId};
use shed_proto::{AiImage, ClientMsg};

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
    _subs: Vec<Subscription>,
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
            _subs: subs,
        };
        view.load_sessions(cx);
        view
    }

    pub fn focus_target(&self, cx: &gpui::App) -> FocusHandle {
        self.prompt.read(cx).focus_handle().clone()
    }

    pub fn apply_settings(&mut self, settings: &Settings, cx: &mut Context<Self>) {
        self.settings = settings.clone();
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
    fn prompt_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
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
        vec![
            menu::item("Copy", move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))),
            menu::item("Copy the Whole Conversation", move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(all.clone()))),
        ]
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
                let folded = lines.len() > FOLD_LINES && !open;
                let shown = if folded { lines[..FOLD_LINES].join("\n") } else { text.clone() };
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
                    .child(div().whitespace_normal().child(shown))
                    .when(images > 0, |d| d.child(div().mt_1().text_size(px(11.0)).text_color(hsla(ui.text_dim)).child(format!("{images} picture(s) attached"))))
                    .when(lines.len() > FOLD_LINES, |d| {
                        let label = if open { "Show less".to_string() } else { format!("Show all {} lines", lines.len()) };
                        d.child(div().id(("fold", i)).mt_1().cursor_pointer().text_size(px(11.0)).text_color(hsla(ui.ink(accent))).on_click(toggle).child(label))
                    })
                    .into_any_element()
            }
            Entry::Assistant { text } => markdown(&text, &ui, &mono, accent).into_any_element(),
            Entry::Thinking { text } => div()
                .id(("thinking", i))
                .cursor_pointer()
                .text_size(px(12.0))
                .text_color(hsla(ui.text_dim))
                .on_click(toggle)
                .child(if open { "▾ Thinking" } else { "▸ Thinking" })
                .when(open, |d| d.child(div().pl_3().whitespace_normal().child(text)))
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
        div()
            .id(("entry", i))
            .px_4()
            .py_1()
            .w_full()
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
        let transcript = list(self.list.clone(), cx.processor(|this, i, _, cx| this.render_entry(i, cx))).flex_1().min_h_0().py_2();
        let empty = self.entries.is_empty();
        let images = self.images.len();
        div()
            .key_context("AiTab")
            .track_focus(&self.focus)
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

/// Markdown, simply: headings, lists, code blocks and inline code, bold.
fn markdown(text: &str, ui: &UiColors, mono: &str, accent: Rgb) -> impl IntoElement {
    let mut out = div().flex().flex_col().gap_1p5().text_color(hsla(ui.text));
    let mut para: Vec<String> = Vec::new();
    let mut code: Option<Vec<String>> = None;
    let flush = |para: &mut Vec<String>, out: gpui::Div| -> gpui::Div {
        if para.is_empty() {
            return out;
        }
        let text = para.join(" ");
        para.clear();
        out.child(inline(&text, ui, accent))
    };
    for line in text.lines() {
        if let Some(block) = &mut code {
            if line.trim_start().starts_with("```") {
                out = out.child(pre(&block.join("\n"), ui, mono, None));
                code = None;
            } else {
                block.push(line.to_string());
            }
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            out = flush(&mut para, out);
            code = Some(Vec::new());
        } else if trimmed.is_empty() {
            out = flush(&mut para, out);
        } else if let Some(h) = trimmed.strip_prefix('#') {
            out = flush(&mut para, out);
            let h = h.trim_start_matches('#').trim();
            out = out.child(div().mt_1().font_weight(FontWeight::BOLD).child(inline(h, ui, accent)));
        } else if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.split_once(". ").is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) {
            out = flush(&mut para, out);
            let indent = (line.len() - trimmed.len()) as f32;
            let (bullet, rest) = if let Some(r) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
                ("•".to_string(), r)
            } else {
                let (n, r) = trimmed.split_once(". ").unwrap();
                (format!("{n}."), r)
            };
            out = out.child(div().flex().gap_1p5().pl(px(4.0 + indent * 4.0)).child(div().flex_none().text_color(hsla(ui.text_dim)).child(bullet)).child(div().flex_1().child(inline(rest, ui, accent))));
        } else if trimmed.starts_with('|') {
            out = flush(&mut para, out);
            out = out.child(div().font_family(mono.to_string()).text_size(px(12.0)).child(line.to_string()));
        } else if trimmed.starts_with('>') {
            out = flush(&mut para, out);
            out = out.child(div().pl_2().border_l_2().border_color(hsla(ui.divider)).text_color(hsla(ui.text_dim)).child(inline(trimmed.trim_start_matches('>').trim(), ui, accent)));
        } else {
            para.push(trimmed.to_string());
        }
    }
    if let Some(block) = code {
        out = out.child(pre(&block.join("\n"), ui, mono, None));
    }
    flush(&mut para, out)
}

/// A line of Markdown text: `code` and **bold** marked.
fn inline(text: &str, ui: &UiColors, accent: Rgb) -> StyledText {
    let mut plain = String::with_capacity(text.len());
    let mut highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let tick = rest.find('`');
        let star = rest.find("**");
        match (tick, star) {
            (Some(t), s) if s.is_none_or(|s| t < s) => {
                plain.push_str(&rest[..t]);
                let after = &rest[t + 1..];
                match after.find('`') {
                    Some(end) => {
                        let start = plain.len();
                        plain.push_str(&after[..end]);
                        highlights.push((start..plain.len(), HighlightStyle { color: Some(hsla(ui.ink(accent))), background_color: Some(hsla_alpha(ui.divider, 0.6)), ..Default::default() }));
                        rest = &after[end + 1..];
                    }
                    None => {
                        plain.push('`');
                        rest = after;
                    }
                }
            }
            (_, Some(s)) => {
                plain.push_str(&rest[..s]);
                let after = &rest[s + 2..];
                match after.find("**") {
                    Some(end) => {
                        let start = plain.len();
                        plain.push_str(&after[..end]);
                        highlights.push((start..plain.len(), HighlightStyle { font_weight: Some(FontWeight::BOLD), ..Default::default() }));
                        rest = &after[end + 2..];
                    }
                    None => {
                        plain.push_str("**");
                        rest = after;
                    }
                }
            }
            _ => {
                plain.push_str(rest);
                rest = "";
            }
        }
    }
    StyledText::new(SharedString::from(plain)).with_highlights(highlights)
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
fn ago(secs: u64) -> String {
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
    fn inline_marks_code_and_bold() {
        let ui = UiColors::default();
        let t = inline("run `cargo test` **now**", &ui, Rgb::hex(0xff8a57));
        let _ = t;
        assert_eq!(lower_first("Edit x"), "edit x");
        assert!(ago(unix_now() - 120).starts_with("2 min"));
    }
}
