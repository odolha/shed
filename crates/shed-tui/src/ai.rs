//! AI tabs: the conversation (Markdown set as styled text, tools folded),
//! permission requests and questions, the prompt (sending, queueing while
//! Claude works, stopping, slash-command completion, pictures by path),
//! the sessions list, drafts, the model, mode and effort, usage limits;
//! sub-agents and background tasks with their steps, Claude Code's notes,
//! wake-ups and scheduled jobs; files attached by path.

use std::collections::HashSet;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use shed_ai::{Entry, NoteKind, Permission, Run, RunState, SessionInfo, SessionRow, Timer, TimerKind};
use shed_model::{AiTab, TabId};
use shed_proto::{AiChange, AiImage, ClientMsg};

use crate::app::{App, bar, now};
use crate::overlay::Item;
use crate::text::{Edit, Input, fit, pad, width, wrap};
use crate::theme::Colors;

/// Claude Code's permission modes: its name and how the menu says it.
pub const MODES: &[(&str, &str)] = &[
    ("default", "Ask Before Edits"),
    ("acceptEdits", "Accept Edits"),
    ("auto", "Auto: a Safety Check Decides"),
    ("plan", "Read Only (Plan)"),
    ("bypassPermissions", "Full Auto"),
];

/// Lines of a tool's result or diff shown when it is unfolded.
const TOOL_LINES: usize = 40;

pub enum Panel {
    Conversation,
    Sessions(Sessions),
    Review(crate::review::Review),
}

#[derive(Default)]
pub struct Sessions {
    pub list: Vec<SessionInfo>,
    pub loaded: bool,
    /// Conversations whose files are listed under them.
    pub open: HashSet<String>,
    pub filter: Input,
    pub filtering: bool,
    pub sel: usize,
}

/// Answering the request Claude waits on.
#[derive(Default)]
pub struct Answer {
    pub request: String,
    /// The question being answered (for AskUserQuestion).
    pub q: usize,
    pub sel: usize,
    /// Options ticked, for questions taking several.
    pub ticked: HashSet<usize>,
    pub answers: Vec<String>,
}

pub struct AiView {
    pub entries: Vec<Entry>,
    pub status: shed_ai::Status,
    pub commands: Vec<shed_ai::Command>,
    pub models: Vec<shed_ai::Model>,
    pub input: Input,
    /// Pictures going with the next prompt: a name and the picture.
    pub images: Vec<(String, AiImage)>,
    /// Files and folders going with it, as their paths for Claude to read.
    pub files: Vec<std::path::PathBuf>,
    /// Lines scrolled up from the end of the conversation.
    pub scroll: usize,
    pub expanded: HashSet<usize>,
    pub expand_all: bool,
    pub answer: Option<Answer>,
    /// With a request waiting, keys go to the prompt instead of the answer.
    pub typing: bool,
    /// The slash command chosen in the completion list.
    pub complete: usize,
    pub no_complete: bool,
    pub panel: Panel,
    started: bool,
    /// Which entry each drawn line of the conversation belongs to.
    line_entries: Vec<usize>,
    conversation_area: Rect,
}

impl AiView {
    pub fn new(ai: &AiTab) -> Self {
        AiView {
            entries: Vec::new(),
            status: Default::default(),
            commands: Vec::new(),
            models: Vec::new(),
            input: Input::new(ai.unsent.clone()),
            images: Vec::new(),
            files: Vec::new(),
            scroll: 0,
            expanded: HashSet::new(),
            expand_all: false,
            answer: None,
            typing: false,
            complete: 0,
            no_complete: false,
            panel: Panel::Conversation,
            started: false,
            line_entries: Vec::new(),
            conversation_area: Rect::default(),
        }
    }

    /// The keeper's entries from `from` on, replacing what was there.
    pub fn update(&mut self, from: usize, entries: Vec<Entry>, status: shed_ai::Status) {
        if from > self.entries.len() {
            // A gap: the whole conversation comes again with the next watch.
            return;
        }
        self.entries.truncate(from);
        self.entries.extend(entries);
        self.status = status;
        let pending = self.pending().map(|p| p.request.clone());
        if self.answer.as_ref().map(|a| &a.request) != pending.as_ref() {
            self.answer = pending.map(|request| Answer { request, ..Default::default() });
            self.typing = false;
        }
    }

    /// Entries changed in place (a sub-agent's progress), each alone.
    pub fn replace(&mut self, entries: Vec<(u32, Entry)>, status: shed_ai::Status) {
        for (i, e) in entries {
            if let Some(slot) = self.entries.get_mut(i as usize) {
                *slot = e;
            }
        }
        self.status = status;
    }

    /// Something counts time on screen: a timer, or a run going on.
    pub fn counting(&self) -> bool {
        !self.status.timers.is_empty() || self.entries.iter().any(|e| matches!(e, Entry::Tool(t) if t.run.as_deref().is_some_and(Run::running)))
    }

    /// The request Claude waits on, if any.
    pub fn pending(&self) -> Option<&Permission> {
        self.entries.iter().rev().find_map(|e| match e {
            Entry::Permission(p) if p.answer.is_none() => Some(p),
            _ => None,
        })
    }

    /// Slash commands matching what is typed (while it is one word starting with `/`).
    pub fn completions(&self) -> Vec<&shed_ai::Command> {
        let t = &self.input.text;
        if self.no_complete || !t.starts_with('/') || t.contains(char::is_whitespace) {
            return Vec::new();
        }
        let typed = t[1..].to_lowercase();
        self.commands.iter().filter(|c| c.name.trim_start_matches('/').to_lowercase().starts_with(&typed)).take(8).collect()
    }

    /// The conversation as lines of `cols` columns, with the entry of each.
    fn conversation(&self, cols: usize, c: &Colors) -> (Vec<Line<'static>>, Vec<usize>) {
        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut owners = Vec::new();
        let cols = cols.max(10);
        for (i, e) in self.entries.iter().enumerate() {
            let open = self.expand_all || self.expanded.contains(&i);
            let before = lines.len();
            match e {
                Entry::User { text, images, .. } => {
                    lines.push(Line::default());
                    let style = c.accent().add_modifier(Modifier::BOLD);
                    lines.extend(wrap(vec![Span::styled(text.clone(), Style::new().add_modifier(Modifier::BOLD))], cols, vec![Span::styled("› ", style)], vec![Span::raw("  ")]));
                    if *images > 0 {
                        lines.push(Line::styled(format!("  ({images} picture{})", if *images == 1 { "" } else { "s" }), c.dim()));
                    }
                    lines.push(Line::default());
                }
                Entry::Assistant { text, .. } => lines.extend(crate::md::lines(text, cols, c)),
                Entry::Thinking { text } => {
                    if open {
                        lines.push(Line::styled("✻ Thinking", c.dim()));
                        lines.extend(wrap(vec![Span::styled(text.clone(), c.dim().add_modifier(Modifier::ITALIC))], cols, vec![Span::raw("  ")], vec![Span::raw("  ")]));
                    } else {
                        lines.push(Line::styled("✻ Thinking…", c.dim()));
                    }
                }
                Entry::Tool(t) => {
                    let run = t.run.as_deref();
                    let (mark, style) = match (run.map(|r| r.state), t.error) {
                        (Some(RunState::Running), _) => ("◌ ", c.accent()),
                        (Some(RunState::Done), _) => ("✓ ", c.fg(c.ansi[6])),
                        (Some(RunState::Failed), _) => ("✗ ", c.danger()),
                        (Some(RunState::Stopped), _) => ("■ ", c.dim()),
                        (None, _) if t.result.is_none() => ("◌ ", c.fg(c.ansi[6])),
                        (None, true) => ("✗ ", c.danger()),
                        (None, false) => ("⏺ ", c.fg(c.ansi[6])),
                    };
                    let fold = if open { "" } else { " …" };
                    // After it: how a run is doing; when a timer it set fires.
                    let info = match run {
                        Some(r) => Some(run_line(t, r)),
                        None => self.status.timers.iter().find(|x| x.tool == t.id).map(|x| format!("fires {}", timer_when(x))),
                    };
                    let mut head = vec![Span::styled(t.summary.clone(), Style::new()), Span::styled(fold, c.dim())];
                    if let Some(info) = info {
                        head.push(Span::styled(format!("  {info}"), if run.is_some_and(Run::running) { c.accent() } else { c.dim() }));
                    }
                    lines.extend(wrap(head, cols, vec![Span::styled(mark, style)], vec![Span::raw("  ")]));
                    if open && let Some(r) = run.filter(|_| matches!(t.name.as_str(), "Agent" | "Task")) {
                        agent_lines(t, r, cols, c, &mut lines);
                    } else if open {
                        if !t.detail.is_empty() && t.detail != t.summary {
                            for l in t.detail.lines().take(TOOL_LINES) {
                                lines.push(Line::styled(format!("  {}", fit(l, cols - 2)), c.dim()));
                            }
                        }
                        if let Some(d) = &t.diff {
                            diff_lines(&d.old, &d.new, cols, c, &mut lines);
                        }
                        if let Some(r) = &t.result {
                            let count = r.lines().count();
                            for l in r.lines().take(TOOL_LINES) {
                                lines.push(Line::styled(format!("  │ {}", fit(l, cols - 4)), if t.error { c.danger() } else { c.dim() }));
                            }
                            if count > TOOL_LINES {
                                lines.push(Line::styled(format!("  │ … {} more lines", count - TOOL_LINES), c.dim()));
                            }
                        }
                    }
                }
                Entry::Permission(p) => {
                    let (word, style) = match p.answer {
                        None => ("waits for you", c.accent().add_modifier(Modifier::BOLD)),
                        Some(true) => ("allowed", c.added()),
                        Some(false) => ("denied", c.danger()),
                    };
                    if p.questions.is_empty() {
                        lines.extend(wrap(vec![Span::raw(format!("{}: {} ", p.tool, p.summary)), Span::styled(format!("({word})"), style)], cols, vec![Span::styled("? ", style)], vec![Span::raw("  ")]));
                    } else {
                        for (n, q) in p.questions.iter().enumerate() {
                            let answer = p.answers.get(n).map(|a| format!(" → {a}")).unwrap_or_default();
                            lines.extend(wrap(vec![Span::raw(q.question.clone()), Span::styled(answer, c.added())], cols, vec![Span::styled("? ", style)], vec![Span::raw("  ")]));
                        }
                    }
                }
                Entry::Note(n) => {
                    let style = if n.error { c.danger() } else { c.dim() };
                    let mark = match n.kind {
                        _ if n.error => "✗ ",
                        NoteKind::Task => "◆ ",
                        NoteKind::Wakeup => "↻ ",
                        NoteKind::Command => "$ ",
                        NoteKind::Compacted => "≡ ",
                        NoteKind::Message => "✉ ",
                        NoteKind::Hook => "⚙ ",
                    };
                    let fold = if n.detail.is_empty() || open { "" } else { " …" };
                    let mark_style = if n.kind == NoteKind::Wakeup && !n.error { c.accent() } else { style };
                    lines.extend(wrap(vec![Span::styled(n.text.clone(), style), Span::styled(fold, c.dim())], cols, vec![Span::styled(mark, mark_style)], vec![Span::raw("  ")]));
                    if open && !n.detail.is_empty() {
                        for l in crate::md::lines(&n.detail, cols.saturating_sub(2), c) {
                            let mut spans = vec![Span::raw("  ")];
                            spans.extend(l.spans);
                            lines.push(Line::from(spans));
                        }
                    }
                }
                Entry::Notice { text, error } => {
                    let style = if *error { c.danger() } else { c.dim() };
                    lines.extend(wrap(vec![Span::styled(text.clone(), style)], cols, vec![Span::styled("· ", style)], vec![Span::raw("  ")]));
                }
            }
            owners.extend(std::iter::repeat_n(i, lines.len() - before));
        }
        (lines, owners)
    }
}

/// An edit's old and new text as removed and added lines.
fn diff_lines(old: &str, new: &str, cols: usize, c: &Colors, out: &mut Vec<Line<'static>>) {
    let hunks = shed_git::diff_texts(old, new, 1);
    let mut n = 0;
    for h in &hunks {
        for l in &h.lines {
            if n >= TOOL_LINES {
                out.push(Line::styled("    …", c.dim()));
                return;
            }
            let (sign, style) = match l.kind {
                shed_git::LineKind::Added => ('+', c.added()),
                shed_git::LineKind::Removed => ('-', c.removed()),
                shed_git::LineKind::Context => (' ', c.dim()),
            };
            out.push(Line::styled(format!("  {sign} {}", fit(&l.text, cols.saturating_sub(4))), style));
            n += 1;
        }
    }
}

/// An opened sub-agent: its kind, its steps (tool calls, what it said),
/// its report.
fn agent_lines(t: &shed_ai::Tool, run: &Run, cols: usize, c: &Colors, out: &mut Vec<Line<'static>>) {
    if !t.detail.is_empty() {
        for l in t.detail.lines().take(TOOL_LINES) {
            out.push(Line::styled(format!("  {}", fit(l, cols - 2)), c.dim()));
        }
    }
    let what: Vec<String> = [Some(run.agent.clone()).filter(|a| !a.is_empty()), (run.tokens > 0).then(|| format!("{} tokens", tokens(run.tokens)))].into_iter().flatten().collect();
    if !what.is_empty() {
        out.push(Line::styled(format!("  {}", what.join(" · ")), c.dim()));
    }
    if run.skipped > 0 {
        out.push(Line::styled(format!("  │ {} earlier steps not shown", run.skipped), c.dim()));
    }
    if run.steps.is_empty() {
        out.push(Line::styled(if run.running() { "  │ No steps yet" } else { "  │ Its steps are not known here" }, c.dim()));
    }
    for step in &run.steps {
        match step {
            Entry::Tool(s) => {
                let (mark, style) = match (&s.result, s.error) {
                    (None, _) if run.running() => ("◌ ", c.accent()),
                    (None, _) => ("○ ", c.dim()),
                    (Some(_), false) => ("✓ ", c.fg(c.ansi[6])),
                    (Some(_), true) => ("✗ ", c.danger()),
                };
                out.push(Line::from(vec![Span::styled("  │ ", c.dim()), Span::styled(mark, style), Span::styled(fit(&s.summary, cols.saturating_sub(6)), c.dim())]));
            }
            Entry::Assistant { text, .. } => {
                for l in text.lines().filter(|l| !l.trim().is_empty()).take(3) {
                    out.push(Line::styled(format!("  │ {}", fit(l, cols.saturating_sub(4))), Style::new().add_modifier(Modifier::ITALIC)));
                }
            }
            _ => {}
        }
    }
    if !run.report.is_empty() {
        out.push(Line::styled("  Its report", c.dim()));
        for l in crate::md::lines(&run.report, cols.saturating_sub(2), c) {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(l.spans);
            out.push(Line::from(spans));
        }
    }
}

/// A span still going, coarse past a minute: `40 s`, `3 min`, `2 h 5 min`;
/// whole minutes rounded down (time gone) or up (time left).
fn coarse(secs: u64, up: bool) -> String {
    let minutes = if up { secs.div_ceil(60) } else { secs / 60 };
    match secs {
        0..60 => format!("{secs} s"),
        _ if minutes < 60 => format!("{minutes} min"),
        _ if minutes.is_multiple_of(60) => format!("{} h", minutes / 60),
        _ => format!("{} h {} min", minutes / 60, minutes % 60),
    }
}

/// A run's state in a word, and for how long: `running 3 min`, `done in 40 s`.
fn run_time(run: &Run) -> String {
    match run.state {
        RunState::Running if run.started > 0 => format!("running {}", coarse(now().saturating_sub(run.started), false)),
        RunState::Running => "running".into(),
        RunState::Done if run.started > 0 && run.ended >= run.started => format!("done in {}", shed_ai::span(run.ended - run.started)),
        RunState::Done => "done".into(),
        RunState::Failed => "failed".into(),
        RunState::Stopped => "stopped".into(),
    }
}

/// A sub-agent's or background task's line: how it is doing, what it does
/// now, how many tool calls.
fn run_line(t: &shed_ai::Tool, run: &Run) -> String {
    let mut parts = vec![run_time(run)];
    if run.running() && !run.last.is_empty() && run.last != "Starting" {
        parts.push(run.last.clone());
    }
    if matches!(t.name.as_str(), "Agent" | "Task") && run.tools > 0 {
        parts.push(format!("{} tool call{}", run.tools, if run.tools == 1 { "" } else { "s" }));
    }
    parts.join(" · ")
}

/// When a timer fires, as a person says it: `in 18 min`, `every minute`.
fn timer_when(t: &Timer) -> String {
    let now = now();
    let left = |at: u64| if at > now { format!("in {}", coarse(at - now, true)) } else { "due now".into() };
    match t.kind {
        TimerKind::Watch if t.at > 0 => format!("ends {}", left(t.at)),
        TimerKind::Watch => "watching".into(),
        _ if !t.every.is_empty() && t.at > now => format!("{}, next {}", t.every.to_lowercase(), left(t.at)),
        _ if !t.every.is_empty() => t.every.to_lowercase(),
        _ if t.at > 0 => left(t.at),
        _ => "later".into(),
    }
}

/// A timer above the prompt: `Wakes up in 18 min`.
fn timer_label(t: &Timer) -> String {
    match t.kind {
        TimerKind::Wakeup => format!("Wakes up {}", timer_when(t)),
        TimerKind::Job => format!("Scheduled job, {}", timer_when(t)),
        TimerKind::Watch => format!("Watching, {}", timer_when(t)),
    }
}

/// The prompt's text with the attached files' paths after it, as the
/// windows send it.
fn with_files(text: String, files: &[std::path::PathBuf]) -> String {
    if files.is_empty() {
        return text;
    }
    let list: Vec<String> = files.iter().map(|f| format!("- {}", f.display())).collect();
    format!("{}{}Attached files:\n{}", text, if text.is_empty() { "" } else { "\n\n" }, list.join("\n"))
}

/// A token count, short: 45k.
fn tokens(n: u64) -> String {
    if n >= 1000 { format!("{}k", n / 1000) } else { n.to_string() }
}

/// What deleting a session asks: what it is, what goes with it, and where.
pub fn delete_text(s: &SessionInfo, prompts: usize, later: usize) -> String {
    let more = match later {
        0 => String::new(),
        1 => ", and 1 later session that continues it".into(),
        n => format!(", and {n} later sessions that continue it"),
    };
    format!(
        "Delete this Claude Code session?\n\n“{}”\n{} ago · {prompts} prompt{}{more}.\n\nClaude Code's files for it move to Shed's trash (~/.local/state/shed/ai-trash), kept 30 days, then deleted.",
        s.title,
        ago(s.modified),
        if prompts == 1 { "" } else { "s" },
    )
}

/// How long ago, short: 5m, 3h, 2d.
pub fn ago(secs: u64) -> String {
    shed_logbook::age(now().saturating_sub(secs) as i64 * 1000)
}

/// What the tab menu offers in an AI tab.
pub fn menu_items() -> Vec<crate::overlay::TabAction> {
    vec![
        (Item::new("Sessions…").hint("Alt+S"), |app, tab| app.ai_sessions(tab)),
        (Item::new("New Session").hint("Alt+N"), |app, tab| app.send(ClientMsg::AiOpenSession { tab, session: None })),
        (Item::new("Review Changes…").hint("Alt+R"), |app, tab| app.ai_review(tab)),
        (Item::new("Model…").hint("Alt+M"), |app, tab| app.ai_model_menu(tab)),
        (Item::new("Mode…").hint("Alt+O"), |app, tab| app.ai_mode_menu(tab)),
        (Item::new("Effort…").hint("Alt+E"), |app, tab| app.ai_effort_menu(tab)),
        (Item::new("Save Prompt as Draft").hint("Alt+D"), |app, tab| app.ai_save_draft(tab)),
        (Item::new("Drafts…").hint("Alt+P"), |app, tab| app.ai_drafts(tab)),
        (Item::new("Queued Messages…").hint("Alt+Q"), |app, tab| app.ai_queued(tab)),
        (Item::new("Attach Files…").hint("Alt+I"), |app, tab| app.ai_attach(tab)),
        (Item::new("Unfold Tools and Thinking").hint("Alt+T"), |app, tab| app.ai_toggle_fold(tab)),
        (Item::new("Stop").hint("Esc"), |app, tab| app.send(ClientMsg::AiStop { tab })),
    ]
}

impl App {
    fn ai_tab(&self, tab: TabId) -> Option<AiTab> {
        self.tree.tab(tab).and_then(|t| t.ai_info()).cloned()
    }

    fn settings(&self, tab: TabId, change: impl FnOnce(&mut Option<Option<String>>, &mut Option<String>, &mut Option<Option<String>>)) {
        let (mut model, mut mode, mut effort) = (None, None, None);
        change(&mut model, &mut mode, &mut effort);
        self.send(ClientMsg::AiChange { tab, change: AiChange::Settings { model, mode, effort, follow: None, browser: None, add_dirs: None } });
    }

    pub(crate) fn ai_key(&mut self, tab: TabId, ev: KeyEvent) {
        let Some(view) = self.ais.get_mut(&tab) else { return };
        match view.panel {
            Panel::Sessions(_) => return self.sessions_key(tab, ev),
            Panel::Review(_) => return self.review_key(tab, ev),
            Panel::Conversation => {}
        }
        let alt = ev.modifiers.contains(KeyModifiers::ALT);
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        if alt && let KeyCode::Char(ch) = ev.code {
            match ch.to_ascii_lowercase() {
                's' => return self.ai_sessions(tab),
                'n' => return self.send(ClientMsg::AiOpenSession { tab, session: None }),
                'r' => return self.ai_review(tab),
                'm' => return self.ai_model_menu(tab),
                'o' => return self.ai_mode_menu(tab),
                'e' => return self.ai_effort_menu(tab),
                'd' => return self.ai_save_draft(tab),
                'p' => return self.ai_drafts(tab),
                'q' => return self.ai_queued(tab),
                'i' => return self.ai_attach(tab),
                't' => return self.ai_toggle_fold(tab),
                'x' => {
                    view.images.clear();
                    view.files.clear();
                    return;
                }
                _ => {}
            }
        }
        // Answering what Claude waits on.
        if view.answer.is_some() && !view.typing {
            if ev.code == KeyCode::Tab {
                view.typing = true;
            } else {
                self.answer_key(tab, ev);
            }
            return;
        }
        let page = view.conversation_area.height.max(2) as usize - 1;
        let completions = view.completions().len();
        match ev.code {
            KeyCode::Esc if completions > 0 => view.no_complete = true,
            KeyCode::Esc => {
                if view.answer.is_some() {
                    view.typing = false;
                } else if view.status.state != shed_ai::State::Idle {
                    self.send(ClientMsg::AiStop { tab });
                }
            }
            KeyCode::Tab if completions > 0 => {
                let name = view.completions()[view.complete.min(completions - 1)].name.trim_start_matches('/').to_string();
                view.input.set(format!("/{name} "));
            }
            KeyCode::Tab if view.answer.is_some() => view.typing = false,
            KeyCode::Up if completions > 0 => view.complete = view.complete.saturating_sub(1),
            KeyCode::Down if completions > 0 => view.complete = (view.complete + 1).min(completions - 1),
            KeyCode::Enter if alt || ev.modifiers.contains(KeyModifiers::SHIFT) => view.input.insert("\n"),
            KeyCode::Char('j') if ctrl => view.input.insert("\n"),
            KeyCode::Enter => self.ai_send(tab),
            KeyCode::PageUp => view.scroll += page,
            KeyCode::PageDown => view.scroll = view.scroll.saturating_sub(page),
            KeyCode::Up if !view.input.text.contains('\n') => view.scroll += 1,
            KeyCode::Down if !view.input.text.contains('\n') => view.scroll = view.scroll.saturating_sub(1),
            KeyCode::End if ctrl => view.scroll = 0,
            KeyCode::Home if ctrl => view.scroll = usize::MAX / 2,
            _ => {
                if view.input.key(&ev) == Edit::Changed {
                    view.complete = 0;
                    view.no_complete = false;
                    // Starting Claude Code as the user types: its commands and models arrive.
                    if !view.started && view.commands.is_empty() {
                        view.started = true;
                        self.send(ClientMsg::AiStart { tab });
                    }
                }
            }
        }
    }

    fn ai_send(&mut self, tab: TabId) {
        let Some(view) = self.ais.get_mut(&tab) else { return };
        if view.input.text.trim().is_empty() && view.images.is_empty() && view.files.is_empty() {
            return;
        }
        let text = with_files(view.input.take().trim_end().to_string(), &std::mem::take(&mut view.files));
        let images = std::mem::take(&mut view.images).into_iter().map(|(_, i)| i).collect();
        view.scroll = 0;
        let busy = view.status.state != shed_ai::State::Idle;
        self.send(ClientMsg::AiSend { tab, text, images });
        if self.ai_tab(tab).is_some_and(|a| !a.unsent.is_empty()) {
            self.send(ClientMsg::AiChange { tab, change: AiChange::Unsent(String::new()) });
        }
        if busy {
            self.note("queued: it goes when Claude finishes (Alt+Q lists the queue)");
        }
    }

    fn answer_key(&mut self, tab: TabId, ev: KeyEvent) {
        let Some(view) = self.ais.get(&tab) else { return };
        let Some(p) = view.pending().cloned() else { return };
        let Some(mut a) = self.ais.get_mut(&tab).and_then(|v| v.answer.take()) else { return };
        let options = answer_options(&p, a.q);
        let n = options.len();
        let multi = p.questions.get(a.q).is_some_and(|q| q.multi);
        let mut done: Option<ClientMsg> = None;
        match ev.code {
            KeyCode::Up | KeyCode::Char('k') => a.sel = a.sel.checked_sub(1).unwrap_or(n - 1),
            KeyCode::Down | KeyCode::Char('j') => a.sel = (a.sel + 1) % n,
            KeyCode::Char(d @ '1'..='9') if (d as usize - '1' as usize) < n => {
                a.sel = d as usize - '1' as usize;
                if !multi {
                    done = self.choose(tab, &p, &mut a);
                }
            }
            KeyCode::Char(' ') if multi && a.sel < p.questions[a.q].options.len() => {
                if !a.ticked.remove(&a.sel) {
                    a.ticked.insert(a.sel);
                }
            }
            KeyCode::Enter => done = self.choose(tab, &p, &mut a),
            _ => {}
        }
        if let Some(msg) = done {
            self.send(msg);
        }
        if let Some(view) = self.ais.get_mut(&tab)
            && view.answer.is_none()
        {
            view.answer = Some(a);
        }
    }

    /// The option chosen at `a.sel`: the message answering, once there is one.
    fn choose(&mut self, tab: TabId, p: &Permission, a: &mut Answer) -> Option<ClientMsg> {
        let request = p.request.clone();
        if p.questions.is_empty() {
            return Some(match a.sel {
                0 => ClientMsg::AiAnswer { tab, request, allow: true, always: false },
                1 => ClientMsg::AiAnswer { tab, request, allow: true, always: true },
                _ => ClientMsg::AiAnswer { tab, request, allow: false, always: false },
            });
        }
        let q = &p.questions[a.q];
        let own = q.options.len();
        if a.sel == own + 1 {
            return Some(ClientMsg::AiAnswer { tab, request, allow: false, always: false });
        }
        if a.sel == own {
            // Their own words, asked for; the answer goes on from there.
            let mut kept = std::mem::take(a);
            let p = p.clone();
            self.ask(format!("Answer: {}", q.question), "", move |app, text| {
                kept.answers.push(text);
                app.next_question(tab, &p, kept);
            });
            return None;
        }
        let answer = if q.multi {
            let mut picked: Vec<usize> = a.ticked.iter().copied().collect();
            if picked.is_empty() {
                picked.push(a.sel);
            }
            picked.sort();
            picked.iter().map(|i| q.options[*i].label.clone()).collect::<Vec<_>>().join(", ")
        } else {
            q.options[a.sel].label.clone()
        };
        a.answers.push(answer);
        let kept = std::mem::take(a);
        self.next_question(tab, p, kept);
        None
    }

    /// After a question is answered: the next one, or the answers sent.
    fn next_question(&mut self, tab: TabId, p: &Permission, mut a: Answer) {
        if a.answers.len() >= p.questions.len() {
            self.send(ClientMsg::AiAnswerQuestions { tab, request: p.request.clone(), answers: a.answers });
            return;
        }
        a.q = a.answers.len();
        a.sel = 0;
        a.ticked.clear();
        a.request = p.request.clone();
        if let Some(view) = self.ais.get_mut(&tab) {
            view.answer = Some(a);
        }
    }

    pub(crate) fn ai_toggle_fold(&mut self, tab: TabId) {
        if let Some(v) = self.ais.get_mut(&tab) {
            v.expand_all = !v.expand_all;
            v.expanded.clear();
        }
    }

    pub(crate) fn ai_model_menu(&mut self, tab: TabId) {
        let Some(view) = self.ais.get(&tab) else { return };
        if view.models.is_empty() {
            self.send(ClientMsg::AiStart { tab });
            self.note("starting Claude Code to hear its models; try again in a moment");
            return;
        }
        let current = self.ai_tab(tab).and_then(|a| a.model);
        let mut items = vec![Item::new("Default").hint(if current.is_none() { "✓" } else { "" })];
        let values: Vec<String> = view.models.iter().map(|m| m.value.clone()).collect();
        for m in &view.models {
            let check = if current.as_deref() == Some(m.value.as_str()) { " ✓" } else { "" };
            items.push(Item::new(if m.name.is_empty() { m.value.clone() } else { m.name.clone() }).hint(format!("{}{check}", fit(&m.description, 40))));
        }
        self.menu("Model", items, move |app, i| {
            let model = if i == 0 { None } else { Some(values[i - 1].clone()) };
            app.settings(tab, |m, _, _| *m = Some(model));
        });
    }

    pub(crate) fn ai_mode_menu(&mut self, tab: TabId) {
        let current = self.ai_tab(tab).map(|a| a.mode).unwrap_or_default();
        let items = MODES
            .iter()
            .map(|(m, label)| {
                let item = Item::new(if *m == "bypassPermissions" { format!("{label}…") } else { label.to_string() }).hint(if *m == current { "✓" } else { "" });
                if *m == "bypassPermissions" { item.danger() } else { item }
            })
            .collect();
        self.menu("Mode", items, move |app, i| {
            let mode = MODES[i].0.to_string();
            if mode == "bypassPermissions" {
                app.confirm(
                    "Full Auto",
                    "Full Auto runs every command and changes every file Claude decides to, with no permission prompts, until you switch back. Use it where a mistake costs little (a sandbox, a throwaway branch).",
                    "Full Auto",
                    true,
                    move |app| app.settings(tab, |_, m, _| *m = Some(mode)),
                );
            } else {
                app.settings(tab, |_, m, _| *m = Some(mode));
            }
        });
    }

    pub(crate) fn ai_effort_menu(&mut self, tab: TabId) {
        let Some(view) = self.ais.get(&tab) else { return };
        let ai = self.ai_tab(tab).unwrap_or_default();
        let model = ai.model.clone().or(view.status.model.clone());
        let efforts: Vec<String> = view
            .models
            .iter()
            .find(|m| model.as_deref().is_some_and(|x| x == m.value || x.contains(&m.value)))
            .or(view.models.first())
            .map(|m| m.efforts.clone())
            .unwrap_or_default();
        if efforts.is_empty() {
            self.send(ClientMsg::AiStart { tab });
            self.note("this model takes no effort setting (or Claude Code has not said yet)");
            return;
        }
        let mut items = vec![Item::new("Default").hint(if ai.effort.is_none() { "✓" } else { "" })];
        items.extend(efforts.iter().map(|e| Item::new(e.clone()).hint(if ai.effort.as_deref() == Some(e) { "✓" } else { "" })));
        self.menu("Effort", items, move |app, i| {
            let effort = if i == 0 { None } else { Some(efforts[i - 1].clone()) };
            app.settings(tab, |_, _, e| *e = Some(effort));
        });
    }

    pub(crate) fn ai_save_draft(&mut self, tab: TabId) {
        let Some(view) = self.ais.get_mut(&tab) else { return };
        if view.input.text.trim().is_empty() {
            self.note("write something to keep as a draft first");
            return;
        }
        let text = view.input.take();
        self.send(ClientMsg::AiChange { tab, change: AiChange::AddDraft { session: None, text } });
        self.send(ClientMsg::AiChange { tab, change: AiChange::Unsent(String::new()) });
        self.note("kept as a draft (Alt+P lists drafts)");
    }

    pub(crate) fn ai_drafts(&mut self, tab: TabId) {
        let Some(ai) = self.ai_tab(tab) else { return };
        let drafts: Vec<(usize, shed_model::Draft)> = ai.shown_drafts().map(|(i, d)| (i, d.clone())).collect();
        if drafts.is_empty() {
            self.note("no drafts in this session (Alt+D keeps the prompt as one)");
            return;
        }
        let items = drafts.iter().map(|(_, d)| Item::new(fit(&d.text.replace('\n', " "), 60))).collect();
        self.menu("Drafts", items, move |app, i| {
            let (index, d) = drafts[i].clone();
            let remove = AiChange::RemoveDraft { index: index as u32, session: d.session.clone(), text: d.text.clone() };
            let items = vec![Item::new("Send Now"), Item::new("Put in the Prompt"), Item::new("Merge into Prompt"), Item::new("Delete…").danger()];
            app.menu("Draft", items, move |app, i| match i {
                0 => {
                    app.send(ClientMsg::AiSend { tab, text: d.text.clone(), images: Vec::new() });
                    app.send(ClientMsg::AiChange { tab, change: remove });
                }
                1 => {
                    if let Some(v) = app.ais.get_mut(&tab) {
                        v.input.set(d.text.clone());
                    }
                    app.send(ClientMsg::AiChange { tab, change: remove });
                }
                // At the end of what is in the prompt; the draft stays.
                2 => {
                    if let Some(v) = app.ais.get_mut(&tab) {
                        let merged = d.merged_into(&v.input.text);
                        v.input.set(merged);
                    }
                }
                _ => app.confirm("Delete Draft", "Delete this draft?", "Delete", true, move |app| app.send(ClientMsg::AiChange { tab, change: remove })),
            });
        });
    }

    pub(crate) fn ai_queued(&mut self, tab: TabId) {
        let Some(ai) = self.ai_tab(tab) else { return };
        if ai.queued.is_empty() {
            self.note("nothing queued");
            return;
        }
        let queued = ai.queued.clone();
        let items = queued.iter().map(|q| Item::new(fit(&q.text.replace('\n', " "), 60))).collect();
        self.menu("Queued Messages", items, move |app, i| {
            let text = queued[i].text.clone();
            app.menu("Queued", vec![Item::new("Send Now"), Item::new("Take Out of the Queue").danger()], move |app, j| {
                app.send(ClientMsg::AiQueued { tab, index: i as u32, text, send: j == 0 });
            });
        });
    }

    /// Attaches a file or folder by its path: a picture goes as a picture,
    /// anything else as its path, for Claude to read.
    pub(crate) fn ai_attach(&mut self, tab: TabId) {
        let here = self.ai_tab(tab).map(|a| a.folder).unwrap_or_else(|| self.here());
        self.ask_with_hint("Attach", "a file or folder: pictures go as pictures, others as their paths", format!("{}/", here.display()), move |app, typed| {
            let path = here.join(crate::app::expand_home(typed.trim()));
            if !path.exists() {
                return app.error(format!("{} is not there", path.display()));
            }
            let small = std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() <= 10 * 1024 * 1024);
            let picture = small.then(|| shed_project::picture::PictureData::read(&path)).flatten().filter(|p| p.kind.media_type().starts_with("image/"));
            let Some(v) = app.ais.get_mut(&tab) else { return };
            match picture {
                Some(p) => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    v.images.push((name, AiImage { media_type: p.kind.media_type().to_string(), data: p.bytes }));
                }
                None if !v.files.contains(&path) => v.files.push(path),
                None => {}
            }
        });
    }

    pub(crate) fn ai_mouse(&mut self, tab: TabId, m: MouseEvent) {
        let Some(view) = self.ais.get_mut(&tab) else { return };
        match m.kind {
            MouseEventKind::ScrollUp => match &mut view.panel {
                Panel::Conversation => view.scroll += 3,
                Panel::Sessions(s) => s.sel = s.sel.saturating_sub(1),
                Panel::Review(r) => r.scroll_diff(-3),
            },
            MouseEventKind::ScrollDown => match &mut view.panel {
                Panel::Conversation => view.scroll = view.scroll.saturating_sub(3),
                Panel::Sessions(s) => s.sel += 1,
                Panel::Review(r) => r.scroll_diff(3),
            },
            MouseEventKind::Down(MouseButton::Left) if matches!(view.panel, Panel::Conversation) => {
                let a = view.conversation_area;
                if m.row >= a.y && m.row < a.y + a.height {
                    // Clicking a tool or a thought folds or unfolds it.
                    if let Some(&entry) = view.line_entries.get((m.row - a.y) as usize)
                        && matches!(view.entries.get(entry), Some(Entry::Tool(_) | Entry::Thinking { .. } | Entry::Note(_)))
                        && !view.expanded.remove(&entry)
                    {
                        view.expanded.insert(entry);
                    }
                }
            }
            _ => {}
        }
    }

    pub(crate) fn render_ai(&mut self, f: &mut Frame, area: Rect, tab: TabId) {
        let c = self.colors.clone();
        let Some(ai) = self.ai_tab(tab) else { return };
        let limits = self.limits_text();
        let Some(view) = self.ais.get_mut(&tab) else { return };
        match view.panel {
            Panel::Sessions(_) => return self.render_sessions(f, area, tab),
            Panel::Review(_) => return self.render_review(f, area, tab),
            Panel::Conversation => {}
        }
        let w = area.width as usize;
        // The prompt: up to 8 rows, after a line of state.
        let (rows, (crow, ccol)) = view.input.rows(w.saturating_sub(3));
        let prompt_h = (rows.len().clamp(1, 8)) as u16;
        let mut extra: Vec<Line> = Vec::new();
        for q in &ai.queued {
            extra.push(Line::from(vec![Span::styled("⏳ queued ", c.dim()), Span::raw(fit(&q.text.replace('\n', " "), w.saturating_sub(10)))]));
        }
        // What Claude set to happen later: wake-ups, scheduled jobs, watches.
        for t in &view.status.timers {
            let glyph = if t.kind == TimerKind::Watch { "👁 " } else { "⏰ " };
            let label = timer_label(t);
            let room = w.saturating_sub(width(&label) + 6);
            extra.push(Line::from(vec![Span::styled(glyph, c.accent()), Span::styled(format!("{label} "), c.accent()), Span::raw(fit(&t.what.replace('\n', " "), room))]));
        }
        // Sub-agents at work, and those ended in the last minute (the latest few).
        let at = now();
        let agents: Vec<(&shed_ai::Tool, &Run)> = view
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Tool(t) if matches!(t.name.as_str(), "Agent" | "Task") => t.run.as_deref().filter(|r| r.running() || r.ended + 60 > at).map(|r| (t, r)),
                _ => None,
            })
            .collect();
        for (t, run) in agents.iter().skip(agents.len().saturating_sub(4)) {
            let name = t.summary.strip_prefix("Agent: ").unwrap_or(&t.summary);
            let mark = match run.state {
                RunState::Running => "◐ ",
                RunState::Done => "✓ ",
                RunState::Failed => "✗ ",
                RunState::Stopped => "■ ",
            };
            let line = run_line(t, run);
            let room = w.saturating_sub(width(&line) + 8);
            extra.push(Line::from(vec![Span::styled(mark, c.accent()), Span::raw(fit(name, room)), Span::styled(format!(" · {line}"), c.dim())]));
        }
        if !view.images.is_empty() || !view.files.is_empty() {
            let names: Vec<String> = view
                .images
                .iter()
                .map(|(n, _)| n.clone())
                .chain(view.files.iter().map(|f| f.file_name().map_or_else(|| f.display().to_string(), |n| n.to_string_lossy().into_owned())))
                .collect();
            extra.push(Line::from(vec![Span::styled("📎 ", c.accent()), Span::raw(fit(&names.join(", "), w.saturating_sub(16))), Span::styled("  Alt+X removes", c.dim())]));
        }
        let pending = view.pending().cloned();
        let card = match (&pending, &view.answer) {
            (Some(p), Some(a)) => answer_card(p, a, w, &c, view.typing),
            _ => Vec::new(),
        };
        let below = prompt_h + 2 + extra.len() as u16 + card.len() as u16;
        let conv_h = area.height.saturating_sub(below);
        let conv = Rect { height: conv_h, ..area };
        view.conversation_area = conv;
        let (lines, owners) = view.conversation(w.saturating_sub(1), &c);
        let total = lines.len();
        let max_scroll = total.saturating_sub(conv_h as usize);
        view.scroll = view.scroll.min(max_scroll);
        let start = total.saturating_sub(conv_h as usize + view.scroll);
        let shown: Vec<Line> = lines.into_iter().skip(start).take(conv_h as usize).collect();
        view.line_entries = owners.into_iter().skip(start).take(conv_h as usize).collect();
        if total == 0 {
            let hint = vec![
                Line::default(),
                Line::styled(format!("  Claude Code in {}", ai.folder.display()), c.dim()),
                Line::styled("  Type a prompt and press Enter. Alt+S sessions · Alt+M model · Alt+O mode · C-b m all actions", c.dim()),
            ];
            f.render_widget(Paragraph::new(hint), conv);
        } else {
            f.render_widget(Paragraph::new(shown), conv);
        }
        if view.scroll > 0 {
            let more = format!(" ↓ {} more lines (PgDn) ", view.scroll);
            f.render_widget(Paragraph::new(Line::styled(more.clone(), c.chip(c.accent))), Rect { x: area.x + area.width.saturating_sub(width(&more) as u16 + 1), y: conv.y + conv.height.saturating_sub(1), width: width(&more) as u16, height: 1 });
        }
        let mut y = conv.y + conv_h;
        for l in extra {
            f.render_widget(Paragraph::new(l), Rect { y, height: 1, ..area });
            y += 1;
        }
        let card_h = card.len() as u16;
        if card_h > 0 {
            f.render_widget(Paragraph::new(card).style(Style::new().bg(c.c(c.tab_active))), Rect { y, height: card_h, ..area });
            y += card_h;
        }
        // The state line.
        let state = match view.status.state {
            shed_ai::State::Working => {
                let secs = view.status.since.map(|s| now().saturating_sub(s)).unwrap_or(0);
                Span::styled(format!("● Working {secs}s (Esc stops) "), c.accent())
            }
            shed_ai::State::Waiting => Span::styled("! Waiting for you ", c.accent().add_modifier(Modifier::BOLD)),
            shed_ai::State::Idle => Span::styled("○ Ready ", c.dim()),
        };
        let model = ai.model.clone().or(view.status.model.clone()).unwrap_or_else(|| "default model".into());
        let mode = MODES.iter().find(|(m, _)| *m == ai.mode).map_or("Ask Before Edits", |(_, l)| l);
        let mode_style = if ai.mode == "bypassPermissions" { c.danger() } else { c.dim() };
        let mut parts = vec![
            state,
            Span::styled(format!("· {model} "), c.dim()),
            Span::styled(format!("· {mode} "), mode_style),
        ];
        if let Some(e) = &ai.effort {
            parts.push(Span::styled(format!("· effort {e} "), c.dim()));
        }
        if view.status.cost > 0.0 {
            parts.push(Span::styled(format!("· ${:.2} ", view.status.cost), c.dim()));
        }
        if view.status.context > 0 {
            let ctx = if view.status.context_max > 0 {
                format!("· context {}", bar(view.status.context as f32 / view.status.context_max as f32, 5))
            } else {
                format!("· context {}", tokens(view.status.context))
            };
            parts.push(Span::styled(format!("{ctx} "), c.dim()));
        }
        if !limits.is_empty() {
            parts.push(Span::styled(format!("· {limits}"), c.dim()));
        }
        f.render_widget(Paragraph::new(Line::from(parts)).style(Style::new().bg(c.c(c.tab_hover))), Rect { y, height: 1, ..area });
        y += 1;
        // The prompt.
        let top = crow.saturating_sub(prompt_h as usize - 1);
        let prompt_lines: Vec<Line> = rows
            .iter()
            .enumerate()
            .skip(top)
            .take(prompt_h as usize)
            .map(|(i, r)| Line::from(vec![Span::styled(if i == 0 { "› " } else { "  " }, c.accent()), Span::raw(r.clone())]))
            .collect();
        let prompt_area = Rect { y, height: prompt_h, ..area };
        f.render_widget(Paragraph::new(prompt_lines), prompt_area);
        let answering = view.answer.is_some() && !view.typing;
        if self.overlay.is_none() && !answering {
            f.set_cursor_position((area.x + 2 + ccol as u16, y + (crow - top) as u16));
        }
        y += prompt_h;
        let hint = if view.input.text.is_empty() { "Enter sends · Alt+Enter new line · C-b m actions" } else { "Enter sends · Alt+Enter new line · Alt+D keep as draft" };
        f.render_widget(Paragraph::new(Line::styled(hint, c.dim())), Rect { y, height: 1, ..area });
        // Slash commands, above the prompt.
        let completions: Vec<(String, String)> = view.completions().iter().map(|c| (c.name.trim_start_matches('/').to_string(), c.description.clone())).collect();
        if !completions.is_empty() {
            view.complete = view.complete.min(completions.len() - 1);
            let h = completions.len() as u16;
            let r = Rect { x: area.x + 2, y: prompt_area.y.saturating_sub(h + 1), width: area.width.saturating_sub(4).min(80), height: h };
            f.render_widget(Clear, r);
            let lines: Vec<Line> = completions
                .iter()
                .enumerate()
                .map(|(i, (name, desc))| {
                    let l = Line::from(vec![Span::styled(format!(" /{name:<20}"), Style::new().add_modifier(Modifier::BOLD)), Span::styled(fit(desc, (r.width as usize).saturating_sub(23)), c.dim())]);
                    if i == view.complete { l.style(c.selected()) } else { l.style(Style::new().bg(c.c(c.tab_active))) }
                })
                .collect();
            f.render_widget(Paragraph::new(lines), r);
        }
    }

    // ── Sessions ────────────────────────────────────────────────────────

    /// Deletes a session and those going on from it (the keeper moves their
    /// files to Shed's trash, or says why not); the list again once its file is gone.
    pub(crate) fn ai_delete_session(&mut self, tab: TabId, session: String) {
        let Some(ai) = self.ai_tab(tab) else { return };
        self.send(ClientMsg::AiDeleteSession { tab, session: session.clone() });
        let folder = ai.folder.clone();
        self.spawn(move || {
            let file = shed_ai::sessions::sessions_dir(&folder).join(format!("{session}.jsonl"));
            for _ in 0..50 {
                if !file.exists() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            let list = shed_ai::list_sessions(&folder);
            move |app: &mut App| {
                if let Some(AiView { panel: Panel::Sessions(s), .. }) = app.ais.get_mut(&tab) {
                    s.list = list;
                    s.loaded = true;
                }
            }
        });
    }

    pub(crate) fn ai_sessions(&mut self, tab: TabId) {
        let Some(ai) = self.ai_tab(tab) else { return };
        let Some(view) = self.ais.get_mut(&tab) else { return };
        view.panel = Panel::Sessions(Sessions::default());
        let folder = ai.folder.clone();
        self.spawn(move || {
            let list = shed_ai::list_sessions(&folder);
            move |app: &mut App| {
                if let Some(AiView { panel: Panel::Sessions(s), .. }) = app.ais.get_mut(&tab) {
                    s.list = list;
                    s.loaded = true;
                    // The session shown, selected.
                }
            }
        });
    }

    fn session_rows(&self, tab: TabId) -> Vec<SessionRow> {
        let Some(ai) = self.ai_tab(tab) else { return Vec::new() };
        let Some(AiView { panel: Panel::Sessions(s), .. }) = self.ais.get(&tab) else { return Vec::new() };
        shed_ai::session_rows(&s.list, ai.draft_sessions(), &s.open, &s.filter.text)
    }

    fn sessions_key(&mut self, tab: TabId, ev: KeyEvent) {
        let rows = self.session_rows(tab);
        let folder = self.ai_tab(tab).map(|a| a.folder).unwrap_or_default();
        let Some(AiView { panel: panel @ Panel::Sessions(_), .. }) = self.ais.get_mut(&tab) else { return };
        let Panel::Sessions(s) = panel else { return };
        if s.filtering {
            match ev.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Down => s.filtering = false,
                _ => {
                    s.filter.key(&ev);
                    s.sel = 0;
                }
            }
            return;
        }
        s.sel = s.sel.min(rows.len().saturating_sub(1));
        let row = rows.get(s.sel).cloned();
        match ev.code {
            KeyCode::Esc | KeyCode::Char('q') => *panel = Panel::Conversation,
            KeyCode::Up | KeyCode::Char('k') => s.sel = s.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => s.sel = (s.sel + 1).min(rows.len().saturating_sub(1)),
            KeyCode::Char('/') => s.filtering = true,
            KeyCode::Right | KeyCode::Left => {
                if let Some(SessionRow::Conversation { root, parts, .. }) = row
                    && parts > 1
                    && !s.open.remove(&root)
                {
                    s.open.insert(root);
                }
            }
            KeyCode::Char('n') => {
                *panel = Panel::Conversation;
                self.send(ClientMsg::AiOpenSession { tab, session: None });
            }
            KeyCode::Enter => {
                let session = match row {
                    Some(SessionRow::Conversation { tip, .. }) => tip.id,
                    Some(SessionRow::Part(p)) => p.id,
                    Some(SessionRow::Draft(d)) => d.key,
                    None => return,
                };
                *panel = Panel::Conversation;
                self.send(ClientMsg::AiOpenSession { tab, session: Some(session) });
            }
            // A new session going on from a copy of this one.
            KeyCode::Char('f') => {
                let session = match row {
                    Some(SessionRow::Conversation { tip, .. }) => tip.id,
                    Some(SessionRow::Part(p)) => p.id,
                    _ => return,
                };
                *panel = Panel::Conversation;
                self.send(ClientMsg::AiForkSession { tab, session });
            }
            // A conversation's row: all of it (its first file, and those going on from it).
            KeyCode::Char('D') | KeyCode::Delete => {
                let (id, shown, prompts) = match row {
                    Some(SessionRow::Conversation { tip, root, .. }) => (root, tip.clone(), tip.whole.max(tip.prompts)),
                    Some(SessionRow::Part(p)) => (p.id.clone(), p.clone(), p.prompts),
                    _ => return,
                };
                let later = shed_ai::going_on_from(&s.list, &id).len();
                let text = delete_text(&shown, prompts, later);
                self.confirm("Delete Session", text, "Delete Session", true, move |app| app.ai_delete_session(tab, id));
            }
            KeyCode::Char('r') => {
                let id = match row {
                    Some(SessionRow::Conversation { tip, .. }) => tip,
                    Some(SessionRow::Part(p)) => p,
                    _ => return,
                };
                self.ask("Rename Session", id.title.clone(), move |app, name| {
                    let folder = folder.clone();
                    let id = id.id.clone();
                    app.spawn(move || {
                        let result = shed_ai::rename_session(&folder, &id, name.trim());
                        move |app: &mut App| match result {
                            Ok(()) => app.ai_sessions(tab),
                            Err(e) => app.error(format!("cannot rename it: {e}")),
                        }
                    });
                });
            }
            _ => {}
        }
    }

    fn render_sessions(&mut self, f: &mut Frame, area: Rect, tab: TabId) {
        let c = self.colors.clone();
        let rows = self.session_rows(tab);
        let ai = self.ai_tab(tab);
        let current = ai.as_ref().and_then(|a| a.session_key().map(String::from));
        let running: Vec<(String, shed_model::AiActivity)> = ai.iter().flat_map(|a| a.running()).map(|(s, a)| (s.to_string(), a)).collect();
        let Some(AiView { panel: Panel::Sessions(s), .. }) = self.ais.get_mut(&tab) else { return };
        // Sessions Claude is at, here or in the background (a
        // conversation's row stands for all its files).
        let away_in = |id: &str, root: Option<&str>| {
            running.iter().find(|(w, _)| w == id || root.is_some_and(|r| shed_ai::family_of(&s.list, w) == Some(r))).map(|(_, a)| *a)
        };
        let head = Line::from(vec![
            Span::styled(" Sessions ", c.chip(c.accent)),
            Span::styled("  Enter open · f fork · n new · → its parts · r rename · D delete · / filter · Esc back", c.dim()),
        ]);
        f.render_widget(Paragraph::new(head), Rect { height: 1, ..area });
        let filter_style = if s.filtering { c.accent() } else { c.dim() };
        f.render_widget(Paragraph::new(Line::from(vec![Span::styled(" / ", filter_style), Span::raw(s.filter.text.clone())])), Rect { y: area.y + 1, height: 1, ..area });
        let list = Rect { y: area.y + 2, height: area.height.saturating_sub(2), ..area };
        if !s.loaded {
            f.render_widget(Paragraph::new(Line::styled("  reading sessions…", c.dim())), list);
            return;
        }
        if rows.is_empty() {
            f.render_widget(Paragraph::new(Line::styled("  no sessions in this folder yet", c.dim())), list);
            return;
        }
        s.sel = s.sel.min(rows.len() - 1);
        let h = list.height as usize;
        let top = s.sel.saturating_sub(h.saturating_sub(1));
        let w = area.width as usize;
        let mut lines = Vec::new();
        for (i, row) in rows.iter().enumerate().skip(top).take(h) {
            let (indent, title, meta, key) = match row {
                SessionRow::Conversation { tip, parts, open, root } => {
                    let fold = if *parts > 1 { if *open { "▾ " } else { "▸ " } } else { "  " };
                    let prompts = tip.whole.max(tip.prompts);
                    let mut meta = format!("{} · {prompts} prompts", ago(tip.modified));
                    if *parts > 1 {
                        meta.push_str(&format!(" · {parts} parts"));
                    }
                    if let Some(b) = &tip.branch {
                        meta.push_str(&format!(" · {b}"));
                    }
                    let mine = current.as_deref().is_some_and(|c| c == tip.id || c == root.as_str());
                    (fold.to_string(), tip.title.clone(), with_away(meta, away_in(&tip.id, Some(root))), mine)
                }
                SessionRow::Part(p) => {
                    let how = if p.parent.is_none() { "first".to_string() } else if p.branched { format!("↳ branched after #{}", p.from) } else { "↳ continued".to_string() };
                    let meta = with_away(format!("{how} · {} · {} prompts", ago(p.modified), p.prompts), away_in(&p.id, None));
                    (format!("{}  ", "  ".repeat(p.depth + 1)), p.title.clone(), meta, current.as_deref() == Some(p.id.as_str()))
                }
                SessionRow::Draft(d) => ("✎ ".to_string(), d.title.clone(), format!("draft · {} draft{}", d.drafts, if d.drafts == 1 { "" } else { "s" }), current.as_deref() == Some(d.key.as_str())),
            };
            let meta = format!("  {meta}");
            let mark = if key { "● " } else { "  " };
            let title = pad(&format!("{mark}{indent}{title}"), w.saturating_sub(width(&meta)));
            let mut line = Line::from(vec![Span::styled(title, if key { c.accent() } else { Style::new() }), Span::styled(meta, c.dim())]);
            if i == s.sel {
                line = line.style(c.selected());
            }
            lines.push(line);
        }
        f.render_widget(Paragraph::new(lines), list);
    }
}

/// A session row's details, saying first that Claude works on it (or
/// waits for you there).
fn with_away(meta: String, away: Option<shed_model::AiActivity>) -> String {
    match away {
        Some(shed_model::AiActivity::Waiting) => format!("waiting for you · {meta}"),
        Some(_) => format!("working · {meta}"),
        None => meta,
    }
}

/// The choices for a request: allow, allow for the session, deny; or a
/// question's options, the user's own answer, and skipping.
fn answer_options(p: &Permission, q: usize) -> Vec<String> {
    match p.questions.get(q) {
        None => vec!["Allow".into(), "Allow for this session".into(), "Deny".into()],
        Some(q) => {
            let mut o: Vec<String> = q.options.iter().map(|o| if o.description.is_empty() { o.label.clone() } else { format!("{} — {}", o.label, o.description) }).collect();
            o.push("Your own answer…".into());
            o.push("Skip (deny)".into());
            o
        }
    }
}

/// The request Claude waits on, with its choices.
fn answer_card(p: &Permission, a: &Answer, w: usize, c: &Colors, typing: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let head_style = c.accent().add_modifier(Modifier::BOLD);
    match p.questions.get(a.q) {
        None => {
            lines.extend(wrap(vec![Span::styled(format!("Claude asks to use {}: ", p.tool), head_style), Span::raw(p.summary.clone())], w, vec![], vec![]));
            for l in p.detail.lines().take(6) {
                if l != p.summary {
                    lines.push(Line::styled(format!("  {}", fit(l, w.saturating_sub(2))), c.dim()));
                }
            }
            if let Some(d) = &p.diff {
                let mut diff = Vec::new();
                diff_lines(&d.old, &d.new, w, c, &mut diff);
                lines.extend(diff.into_iter().take(10));
            }
        }
        Some(q) => {
            let n = p.questions.len();
            let head = if q.header.is_empty() { String::new() } else { format!("{} · ", q.header) };
            let count = if n > 1 { format!(" ({} of {n})", a.q + 1) } else { String::new() };
            lines.extend(wrap(vec![Span::styled(format!("{head}{}{count}", q.question), head_style)], w, vec![], vec![]));
            if q.multi {
                lines.push(Line::styled("  Space ticks, Enter answers", c.dim()));
            }
        }
    }
    let options = answer_options(p, a.q);
    let multi = p.questions.get(a.q).is_some_and(|q| q.multi);
    for (i, o) in options.iter().enumerate() {
        let tick = if multi && i < options.len() - 2 { if a.ticked.contains(&i) { "[x] " } else { "[ ] " } } else { "" };
        let text = fit(&format!("  {}. {tick}{o}", i + 1), w);
        let deny = o.starts_with("Deny") || o.starts_with("Skip");
        let style = if i == a.sel && !typing { c.selected() } else if deny { c.danger() } else { Style::new() };
        lines.push(Line::styled(text, style));
    }
    let hint = if typing { "Tab: back to answering" } else { "↑↓ or 1–9 choose · Enter answers · Tab types a prompt instead" };
    lines.push(Line::styled(hint, c.dim()));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use shed_ai::{Question, QuestionOption};

    fn view() -> AiView {
        AiView::new(&AiTab::default())
    }

    fn colors() -> Colors {
        Colors::from_theme(&shed_model::config::Theme::load("shed-dark").unwrap(), false)
    }

    fn text(lines: &[Line]) -> String {
        lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn deleting_a_session_says_what_goes_and_where() {
        let s = SessionInfo { id: "a".into(), title: "Plan the shelves".into(), modified: now(), ..Default::default() };
        let text = delete_text(&s, 3, 2);
        assert!(text.starts_with("Delete this Claude Code session?"), "{text}");
        assert!(text.contains("“Plan the shelves”") && text.contains("3 prompts, and 2 later sessions that continue it."), "{text}");
        assert!(text.contains("Shed's trash (~/.local/state/shed/ai-trash), kept 30 days"), "{text}");
        assert!(!delete_text(&s, 1, 0).contains("later"));
        assert!(delete_text(&s, 1, 1).contains("1 prompt, and 1 later session that continues it."));
    }

    #[test]
    fn entries_change_in_place() {
        let mut v = view();
        let a = |t: &str| Entry::Assistant { text: t.into(), at: 0 };
        v.update(0, vec![a("one"), a("two")], Default::default());
        v.replace(vec![(0, a("ONE")), (7, a("nowhere"))], Default::default());
        assert_eq!(v.entries, vec![a("ONE"), a("two")]);
    }

    /// A sub-agent's line says how it does; opened, its steps and report.
    /// Notes fold their detail; a timer counts down above the prompt.
    #[test]
    fn agents_notes_and_timers_show() {
        let mut v = view();
        let step = shed_ai::Tool { id: "s1".into(), summary: "$ ls -a".into(), result: Some("x".into()), ..Default::default() };
        let run = Run { state: RunState::Running, started: now() - 5, last: "$ ls -a".into(), tools: 2, steps: vec![Entry::Tool(step)], skipped: 3, agent: "Explore".into(), ..Default::default() };
        let agent = shed_ai::Tool { id: "a1".into(), name: "Agent".into(), summary: "Agent: Count files".into(), run: Some(Box::new(run)), ..Default::default() };
        let wake = shed_ai::Tool { id: "w1".into(), name: "ScheduleWakeup".into(), summary: "Wake up in 10 min: check".into(), result: Some(String::new()), ..Default::default() };
        let note = shed_ai::Note { kind: NoteKind::Task, text: "Agent \"Count files\" finished".into(), detail: "It found 3 files.".into(), ..Default::default() };
        let status = shed_ai::Status { timers: vec![Timer { kind: TimerKind::Wakeup, what: "check".into(), at: now() + 600, tool: "w1".into(), ..Default::default() }], ..Default::default() };
        v.update(0, vec![Entry::Tool(agent), Entry::Tool(wake), Entry::Note(note)], status);
        assert!(v.counting());
        let c = colors();
        let shown = text(&v.conversation(100, &c).0);
        assert!(shown.contains("Agent: Count files … ") && shown.contains("running 5 s · $ ls -a · 2 tool calls"), "{shown}");
        assert!(shown.contains("fires in 10 min"), "{shown}");
        assert!(shown.contains("finished …") && !shown.contains("3 files"), "{shown}");
        v.expand_all = true;
        let shown = text(&v.conversation(100, &c).0);
        assert!(shown.contains("Explore") && shown.contains("3 earlier steps not shown") && shown.contains("✓ $ ls -a"), "{shown}");
        assert!(shown.contains("It found 3 files."), "{shown}");
        assert_eq!(timer_label(&v.status.timers[0]), "Wakes up in 10 min");
        let job = Timer { kind: TimerKind::Job, every: "Every minute".into(), ..Default::default() };
        assert_eq!(timer_label(&job), "Scheduled job, every minute");
    }

    #[test]
    fn attached_files_go_as_paths_after_the_text() {
        let files = vec![std::path::PathBuf::from("/tmp/a.pdf"), std::path::PathBuf::from("/tmp/b")];
        assert_eq!(with_files("Read these".into(), &files), "Read these\n\nAttached files:\n- /tmp/a.pdf\n- /tmp/b");
        assert_eq!(with_files(String::new(), &files[..1]), "Attached files:\n- /tmp/a.pdf");
        assert_eq!(with_files("hi".into(), &[]), "hi");
    }

    #[test]
    fn updates_replace_from_where_they_start() {
        let mut v = view();
        let a = |t: &str| Entry::Assistant { text: t.into(), at: 0 };
        v.update(0, vec![a("one"), a("two")], Default::default());
        v.update(1, vec![a("two, longer"), a("three")], Default::default());
        assert_eq!(v.entries, vec![a("one"), a("two, longer"), a("three")]);
        // A gap is ignored.
        v.update(9, vec![a("x")], Default::default());
        assert_eq!(v.entries.len(), 3);
    }

    #[test]
    fn a_waiting_request_is_answered_from_its_options() {
        let mut v = view();
        let p = Permission {
            request: "r1".into(),
            tool: "AskUserQuestion".into(),
            questions: vec![Question { question: "Which?".into(), options: vec![QuestionOption { label: "A".into(), description: String::new() }], ..Default::default() }],
            ..Default::default()
        };
        v.update(0, vec![Entry::Permission(p.clone())], Default::default());
        assert_eq!(v.answer.as_ref().map(|a| a.request.as_str()), Some("r1"));
        assert_eq!(answer_options(&p, 0), ["A", "Your own answer…", "Skip (deny)"]);
        let plain = Permission { request: "r2".into(), tool: "Bash".into(), ..Default::default() };
        assert_eq!(answer_options(&plain, 0).len(), 3);
    }

    #[test]
    fn slash_commands_complete_while_one_word() {
        let mut v = view();
        v.commands = vec![shed_ai::Command { name: "review".into(), ..Default::default() }, shed_ai::Command { name: "compact".into(), ..Default::default() }];
        v.input.set("/re");
        assert_eq!(v.completions().len(), 1);
        v.input.set("/re x");
        assert!(v.completions().is_empty());
    }
}
