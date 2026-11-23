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
use shed_ai::{
    Command, Entry, Limits, Model, NoteKind, Run, RunState, SessionInfo, SessionRow, State as AiState, Status, Timer,
    TimerKind,
};
use shed_model::{AiTab, BenchId, Rgb, TabId};
use shed_proto::{AiChange, AiImage, ClientMsg};

use crate::ai_text::{self, Copy, Kind, Piece, Point, Selection};
use crate::assets::icon;
use shed_client::keeper_conn::KeeperConn;
use shed_client::nexus_api::RemoteWorkspace;
use crate::line_editor::{EditorEvent, LineEditor};
use crate::menu::{self, Menu, MenuEntry, MenuHost};
use crate::settings::Settings;
use crate::text_view::{TextEvent, TextOptions, TextView};
use crate::theme::{UiColors, current_ui, hsla, hsla_alpha};

const SESSIONS_WIDTH: f32 = 260.0;
/// A user's message longer than this many lines shows folded.
const FOLD_LINES: usize = 12;
/// …or longer than this (long lines wrap to many): folded too.
const FOLD_CHARS: usize = 1_500;
/// Agents that ended stay in the strip above the prompt this long (seconds).
const AGENT_KEPT: u64 = 60;
/// An opened agent's prompt, as a step of its own, opened apart.
const PROMPT_STEP: &str = "\u{0}prompt";

/// A prompt shown folded: its first lines, and no more than `FOLD_CHARS`.
fn folded_text(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= FOLD_LINES && text.len() <= FOLD_CHARS {
        return None;
    }
    let mut shown = lines[..lines.len().min(FOLD_LINES)].join("\n");
    if shown.len() > FOLD_CHARS {
        shown.truncate(shown.floor_char_boundary(FOLD_CHARS));
        shown.push('…');
    }
    Some(shown)
}

/// The same item of a conversation, after it changed: a tool by its id,
/// a note by its line, others by kind.
fn same_entry(a: &Entry, b: &Entry) -> bool {
    match (a, b) {
        (Entry::Tool(x), Entry::Tool(y)) => x.id == y.id,
        (Entry::Note(x), Entry::Note(y)) => x.text == y.text,
        _ => std::mem::discriminant(a) == std::mem::discriminant(b),
    }
}

/// A sub-agent's tool and its run, if `e` is one.
fn agent_run(e: &Entry) -> Option<(&shed_ai::Tool, &Run)> {
    match e {
        Entry::Tool(t) if matches!(t.name.as_str(), "Agent" | "Task") => t.run.as_deref().map(|r| (t, r)),
        _ => None,
    }
}
/// Lines of a tool's output shown when it is opened.
const RESULT_LINES: usize = 40;

/// Answers being picked to one request of Claude's questions.
struct Ask {
    picks: Vec<std::collections::BTreeSet<usize>>,
    other: Vec<Option<Entity<LineEditor>>>,
    subs: Vec<Subscription>,
}

impl Ask {
    fn new(questions: usize) -> Self {
        Ask { picks: vec![Default::default(); questions], other: vec![None; questions], subs: Vec::new() }
    }
}

/// Claude Code's permission modes: its name, the menu's label, the chip's.
const PERMISSION_MODES: &[(&str, &str, &str)] = &[
    ("default", "Ask Before Edits", "Ask before edits"),
    ("acceptEdits", "Accept Edits", "Accept edits"),
    ("auto", "Auto: a Safety Check Decides", "Auto"),
    ("plan", "Read Only (Plan)", "Read only"),
    ("bypassPermissions", "Full Auto", "Full auto"),
];

/// Where a jump to the previous (`up`) or next message lands, from the
/// entry at the top (scrolled into it if `inside`): the starts of your
/// messages and of the responses to them.
fn block_target(entries: &[Entry], top: usize, inside: bool, up: bool) -> Option<usize> {
    let is_prompt = |e: &Entry| matches!(e, Entry::User { .. });
    let mut starts = (0..entries.len()).filter(|&i| i == 0 || is_prompt(&entries[i]) || is_prompt(&entries[i - 1]));
    if up {
        // Inside a message: its own start first.
        starts.rfind(|&s| s < top || (s == top && inside))
    } else {
        starts.find(|&s| s > top)
    }
}

/// The model Claude Code's "default" stands for: "Opus 5.5" from its
/// description ("Opus 5.5 · Best for…").
fn default_model_name(m: &Model) -> String {
    m.description.split(" · ").next().filter(|s| !s.is_empty()).unwrap_or(&m.name).to_string()
}

/// One entry of the conversation, drawn by the AI view, in a view of its own:
/// a hover change inside it (a step's line, a copy button) draws it alone,
/// while every other entry is kept as it was.
pub struct EntryView {
    ai: gpui::WeakEntity<AiView>,
    entry: usize,
    _follow: Subscription,
}

impl Render for EntryView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let i = self.entry;
        match self.ai.upgrade() {
            Some(ai) => ai.update(cx, |ai, cx| ai.render_entry(i, cx)),
            None => div().into_any_element(),
        }
    }
}

/// A session to delete, from its row: the session (with those going on
/// from it), and what the row shows of it.
#[derive(Clone)]
struct ToDelete {
    id: String,
    title: String,
    modified: u64,
    prompts: usize,
}

#[path = "ai_review.rs"]
mod review;
#[path = "ai_remote.rs"]
mod remote;

/// After a model's name, Claude Code's choice of its 1M-token context.
const LARGE_CONTEXT: &str = "[1m]";

/// A token count, short: `84k`, `1M`.
fn tokens(n: u64) -> String {
    match n {
        1_000_000.. if n.is_multiple_of(1_000_000) => format!("{}M", n / 1_000_000),
        1_000_000.. => format!("{:.1}M", n as f64 / 1_000_000.0),
        _ => format!("{}k", n / 1000),
    }
}

#[cfg(test)]
pub static BARS_DRAWN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static RENDERED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How many entries a conversation shows at first, and brings in at a time
/// scrolling up: a long one opens as fast as a short one.
const SHOWN_AT_FIRST: usize = 150;

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

/// A draft open in the prompt field to be changed.
struct DraftEdit {
    /// Where it is in the tab's drafts, its session and its text as it was
    /// (to find it again if the list changed meanwhile).
    index: usize,
    session: String,
    text: String,
    /// What was in the prompt before: it comes back when the edit ends.
    before: String,
}

pub struct AiView {
    pub bench: BenchId,
    pub tab: TabId,
    pub ai: AiTab,
    pub accent: Rgb,
    conn: KeeperConn,
    /// The settings with the tab's zoom in the font size (`base` without).
    settings: Settings,
    base: Settings,
    zoom: f32,
    entries: Vec<Entry>,
    status: Status,
    commands: Vec<Command>,
    models: Vec<Model>,
    list: ListState,
    /// Entries shown opened (tools, thinking, long prompts).
    expanded: HashSet<usize>,
    prompt: Entity<TextView>,
    images: Vec<AiImage>,
    /// The pictures' thumbnails, one per picture.
    previews: Vec<Arc<gpui::Image>>,
    /// Other files to go with the prompt: their paths are added to it.
    files: Vec<PathBuf>,
    sessions: Arc<Vec<SessionInfo>>,
    /// Conversations whose files are listed under them.
    open_families: HashSet<String>,
    /// A view per entry drawn, so a hover change inside one redraws only it
    /// (the others are kept as they were drawn); each follows this view.
    entry_views: HashMap<usize, Entity<EntryView>>,
    /// Reviewing what Claude changed, in place of the conversation.
    review: Option<review::Review>,
    /// A session being renamed: its id, and the field with its name.
    renaming: Option<(String, Entity<LineEditor>, Subscription)>,
    sessions_task: Option<Task<()>>,
    /// A session being deleted (the keeper moves its files to the trash):
    /// its id, and the task watching for its file to go.
    deleting: Option<(String, Task<()>)>,
    /// Why deleting a session did not work, above the sessions list.
    delete_problem: Option<String>,
    /// Another workspace's tab: its sessions, review and pictures are on
    /// that computer, asked for through the nexus.
    remote: Option<RemoteWorkspace>,
    /// Pictures of the conversation fetched from another workspace, by
    /// their path there (`None` while coming, or when not there).
    remote_pictures: HashMap<String, Option<Arc<gpui::Image>>>,
    /// Files and pictures on their way to another workspace, to attach.
    uploading: usize,
    /// Why attaching did not work, above the prompt.
    attach_problem: Option<String>,
    filter: Entity<LineEditor>,
    sessions_scroll: UniformListScrollHandle,
    /// The conversation's and the sessions' scrollbars.
    conversation_bar: crate::scrollbar::State,
    sessions_bar: crate::scrollbar::State,
    completion: Option<Completion>,
    /// Going back through earlier prompts with Ctrl+Up: how far back.
    history: Option<usize>,
    /// What was in the prompt when going back through earlier prompts began:
    /// coming back down past the newest gives it back.
    unsent: Option<String>,
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
    /// An earlier prompt being edited in the prompt field: sending goes
    /// back to it.
    editing: Option<usize>,
    /// A draft being changed in the prompt field: Save Draft puts it back
    /// changed, Cancel leaves it as it was.
    editing_draft: Option<DraftEdit>,
    /// The working dots' step.
    spin: usize,
    /// Answers being chosen to Claude's questions, by request.
    asks: HashMap<String, Ask>,
    /// The bench's projects' files (root, paths relative to it), to find a
    /// file Claude names by its name alone; and what was found so.
    project_files: Vec<(PathBuf, Arc<Vec<PathBuf>>)>,
    file_lookups: HashMap<PathBuf, Option<PathBuf>>,
    /// The bench's project views those files are read from, followed (not
    /// read while drawing, which would draw this view at each change there).
    project_views: Vec<Entity<crate::project_view::ProjectView>>,
    _project_subs: Vec<Subscription>,
    /// The link the mouse is over: entry, piece, its range.
    hover_link: Option<(usize, usize, Range<usize>)>,
    /// Whether paths Claude names are files, as found (for its links).
    file_exists: HashMap<PathBuf, bool>,
    /// Messages written while Claude works, sent one by one as turns end.
    /// Saving what is in the prompt, a moment after the last change.
    unsent_save: Option<Task<()>>,
    /// The first entry the list holds: a long conversation shows its end,
    /// and earlier entries come in as they are needed (`show_from`).
    first: usize,
    /// The list of drafts is open above the prompt.
    drafts_open: bool,
    /// The browsers with Claude in Chrome, looked for when the tab connects.
    browsers: Vec<&'static str>,
    /// The left side shows the folder's sessions, or this one's prompts.
    show_prompts: bool,
    prompts_scroll: UniformListScrollHandle,
    prompts_bar: crate::scrollbar::State,
    /// A sub-agent's steps opened to their output: (entry, the step's tool id).
    open_steps: HashSet<(usize, String)>,
    /// Some agent or background task is running (its time keeps up).
    running: bool,
    /// When the latest of them started (its seconds are counted for a minute).
    newest_run: u64,
    /// Sub-agents running or just ended, newest last: the strip above the prompt.
    agents: Vec<usize>,
    _subs: Vec<Subscription>,
}

/// An entry's text as drawn: pieces of selectable text, and the colours of
/// those that are code.
struct Parsed {
    source: String,
    pieces: Vec<Piece>,
    code: Vec<Option<CodeColors>>,
    /// An opened sub-agent: the pieces of each of its steps (none for a
    /// tool call), then of its report.
    groups: Vec<Range<usize>>,
}

/// Byte ranges of a code block and their colours.
type CodeColors = Vec<(Range<usize>, Rgb)>;

impl Parsed {
    fn plain(text: String) -> Self {
        Parsed { source: String::new(), pieces: vec![Piece { kind: Kind::Para, text: shed_ai::markdown::Text::plain(text) }], code: vec![None], groups: Vec::new() }
    }

    fn empty() -> Self {
        Parsed { source: String::new(), pieces: Vec::new(), code: Vec::new(), groups: Vec::new() }
    }
}

/// A run's state in a word, and for how long: `running 3 min`, `done in 40 s`.
/// A span still going, coarse past a minute (the view is drawn again every
/// second only in the first minute): `40 s`, `3 min`, `2 h 5 min`; whole
/// minutes rounded down (time gone) or up (time left).
fn coarse(secs: u64, up: bool) -> String {
    let minutes = if up { secs.div_ceil(60) } else { secs / 60 };
    match secs {
        0..60 => format!("{secs} s"),
        _ if minutes < 60 => format!("{minutes} min"),
        _ if minutes % 60 == 0 => format!("{} h", minutes / 60),
        _ => format!("{} h {} min", minutes / 60, minutes % 60),
    }
}

/// A wake-up past its time by more than this is not counted down any more.
const OVERDUE: u64 = 60;

fn run_time(run: &Run) -> String {
    let took = |a: u64, b: u64| shed_ai::span(b.saturating_sub(a));
    match run.state {
        RunState::Running if run.started > 0 => format!("running {}", coarse(unix_now().saturating_sub(run.started), false)),
        RunState::Running => "running".into(),
        RunState::Done if run.started > 0 && run.ended >= run.started => format!("done in {}", took(run.started, run.ended)),
        RunState::Done => "done".into(),
        RunState::Failed => "failed".into(),
        RunState::Stopped => "stopped".into(),
    }
}

/// A sub-agent's or background task's line after its tool's: how it is
/// doing, what it does now, how many tool calls.
fn run_line(t: &shed_ai::Tool, run: &Run) -> String {
    let mut parts = vec![run_time(run)];
    let agent = matches!(t.name.as_str(), "Agent" | "Task");
    if run.running() && !run.last.is_empty() && run.last != "Starting" {
        parts.push(run.last.clone());
    }
    if agent && run.tools > 0 {
        parts.push(format!("{} tool call{}", run.tools, if run.tools == 1 { "" } else { "s" }));
    }
    parts.join(" · ")
}

/// When a timer fires, as a person says it: `in 18 min`, `every minute`.
fn timer_when(t: &Timer) -> String {
    let now = unix_now();
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

/// The byte of `text` under the mouse, on a character's edge; `None` when
/// the text is not laid out (GPUI's text layout panics then, so it is asked
/// carefully: a selection is not worth a crash).
fn text_index(layout: &TextLayout, text: &str, at: gpui::Point<gpui::Pixels>) -> Option<usize> {
    let found = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match layout.index_for_position(at) {
        Ok(i) | Err(i) => i,
    }))
    .ok()?;
    Some(text.floor_char_boundary(found.min(text.len())))
}

/// Whether the mouse is over laid-out text (same care as `text_index`).
fn over_text(layout: &TextLayout, at: gpui::Point<gpui::Pixels>) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| layout.bounds().contains(&at))).unwrap_or(false)
}

impl AiView {
    pub fn new(bench: BenchId, tab: TabId, ai: AiTab, accent: Rgb, conn: KeeperConn, settings: Settings, cx: &mut Context<Self>) -> Self {
        let browsers = if ai.browser { crate::browser::claude_browsers() } else { Vec::new() };
        let options = TextOptions {
            wrap: true,
            auto_height: Some((2, 14)),
            submit_on_enter: false,
            placeholder: Some("Ask Claude…  (Ctrl+Enter sends, Ctrl+S keeps it as a draft, Enter breaks the line, / for commands)".into()),
            ..Default::default()
        };
        // What was being written, kept with the tab (across restarts too).
        let prompt = cx.new(|cx| TextView::new(&ai.unsent, options, &settings, cx));
        let filter = cx.new(|cx| LineEditor::new(String::new(), cx).without_hint());
        let subs = vec![
            cx.subscribe(&prompt, |this, _, event: &TextEvent, cx| match event {
                TextEvent::Submit => this.send(cx),
                TextEvent::Changed => {
                    this.warm_up();
                    this.update_completion(cx);
                    this.save_unsent_soon(cx);
                }
                TextEvent::Cancel => {
                    this.completion = None;
                    cx.notify();
                }
                // Ctrl+S keeps the message as a draft.
                TextEvent::Save => this.save_draft(cx),
                TextEvent::ToggleWrap => {}
            }),
            cx.observe(&filter, |_, _, cx| cx.notify()),
        ];
        // Every entry measured, so the scrollbar knows the whole length.
        let list = ListState::new(0, ListAlignment::Top, px(600.0)).measure_all();
        list.set_follow_mode(FollowMode::Tail);
        conn.send_msg(ClientMsg::AiWatch { tab });
        let mut view = Self {
            bench,
            tab,
            ai,
            accent,
            conn,
            base: settings.clone(),
            settings,
            zoom: 0.0,
            entries: Vec::new(),
            status: Status::default(),
            commands: Vec::new(),
            models: Vec::new(),
            list,
            expanded: HashSet::new(),
            prompt,
            images: Vec::new(),
            previews: Vec::new(),
            files: Vec::new(),
            sessions: Arc::default(),
            open_families: HashSet::new(),
            entry_views: HashMap::new(),
            review: None,
            renaming: None,
            sessions_task: None,
            filter,
            sessions_scroll: UniformListScrollHandle::new(),
            deleting: None,
            delete_problem: None,
            remote: None,
            remote_pictures: HashMap::new(),
            uploading: 0,
            attach_problem: None,
            conversation_bar: Default::default(),
            sessions_bar: Default::default(),
            completion: None,
            history: None,
            unsent: None,
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
            editing: None,
            editing_draft: None,
            spin: 0,
            asks: HashMap::new(),
            unsent_save: None,
            first: 0,
            drafts_open: false,
            browsers,
            file_exists: HashMap::new(),
            hover_link: None,
            project_files: Vec::new(),
            file_lookups: HashMap::new(),
            project_views: Vec::new(),
            _project_subs: Vec::new(),
            show_prompts: false,
            prompts_scroll: UniformListScrollHandle::new(),
            prompts_bar: Default::default(),
            open_steps: HashSet::new(),
            running: false,
            newest_run: 0,
            agents: Vec::new(),
            _subs: subs,
        };
        view.load_sessions(cx);
        // "5 min ago" and the limits' time left keep up: a redraw a minute,
        // when there is a conversation or limits to show.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(60)).await;
                let alive = this.update(cx, |this, cx| {
                    if !this.entries.is_empty() || cx.try_global::<AiLimits>().is_some() {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
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

    /// The tab's zoom, in font points.
    pub fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        if zoom != self.zoom {
            self.zoom = zoom;
            let base = self.base.clone();
            self.apply_settings(&base, cx);
        }
    }

    /// How much larger than usual the tab's text is drawn.
    fn z(&self) -> f32 {
        // The tab's zoom and the configured size together.
        self.settings.font.size / crate::theme::BASE_FONT_SIZE
    }

    pub fn apply_settings(&mut self, settings: &Settings, cx: &mut Context<Self>) {
        self.base = settings.clone();
        self.settings = settings.zoomed(self.zoom);
        self.parsed.clear();
        let zoomed = self.settings.clone();
        self.prompt.update(cx, |p, cx| p.apply(&zoomed, cx));
        self.list.remeasure();
        cx.notify();
    }

    /// Follows the tab as the keeper has it.
    pub fn set_ai(&mut self, ai: AiTab, cx: &mut Context<Self>) {
        if ai.session != self.ai.session && ai.session.is_some() && !self.sessions.iter().any(|s| Some(&s.id) == ai.session.as_ref()) {
            self.load_sessions(cx);
        }
        if ai.browser && !self.ai.browser {
            self.browsers = crate::browser::claude_browsers();
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
        // What was opened stays open only where the same entry still is.
        let old = &self.entries;
        self.expanded.retain(|&e| e < from || old.get(e).zip(entries.get(e - from)).is_some_and(|(a, b)| same_entry(a, b)));
        self.entries.truncate(from);
        self.entries.extend(entries);
        self.parsed.retain(|i, _| *i < from);
        // The prompt being edited changed under it (another window sent it).
        if self.editing.is_some_and(|e| e >= from) {
            self.editing = None;
        }
        if self.selection.is_some_and(|s| s.ordered().1.entry >= from) {
            self.selection = None;
        }
        let len = self.entries.len();
        if from == 0 || from < self.first {
            if from == 0 {
                self.expanded.clear();
            }
            // Only the end at first: earlier entries come in as needed.
            self.first = len.saturating_sub(SHOWN_AT_FIRST).min(if from == 0 { len } else { from });
            self.list.reset(self.rows());
        } else {
            let old_rows = old_len - self.first + self.head();
            self.list.splice(self.row(from)..old_rows, len - from);
        }
        self.open_steps.retain(|(e, _)| *e < from);
        self.set_status(status, cx);
    }

    /// Entries changed in place (a sub-agent's progress), from the keeper.
    pub fn replace(&mut self, changed: Vec<(u32, Entry)>, status: Status, cx: &mut Context<Self>) {
        for (i, entry) in changed {
            let i = i as usize;
            let Some(slot) = self.entries.get_mut(i) else { continue };
            *slot = entry;
            // (What was parsed of it stays: each parse is keyed by what it read.)
            if i >= self.first {
                let row = self.row(i);
                self.list.remeasure_items(row..row + 1);
            }
        }
        self.set_status(status, cx);
    }

    fn set_status(&mut self, status: Status, cx: &mut Context<Self>) {
        let finished = self.status.state != AiState::Idle && status.state == AiState::Idle;
        self.status = status;
        if finished {
            self.load_sessions(cx);
            // Files Claude made in the turn: links to them, now they exist.
            self.file_exists.clear();
            self.file_lookups.clear();
            self.parsed.clear();
        }
        let running = self.entries.iter().filter_map(|e| match e {
            Entry::Tool(t) => t.run.as_ref().filter(|r| r.running()),
            _ => None,
        });
        self.newest_run = running.clone().map(|r| r.started).max().unwrap_or(0);
        self.running = running.count() > 0;
        let now = unix_now();
        self.agents = self.entries.iter().enumerate().filter(|(_, e)| agent_run(e).is_some_and(|(_, r)| r.running() || r.ended + AGENT_KEPT > now)).map(|(i, _)| i).collect();
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
        // A sub-agent's edits are the turn's too.
        fn steps(e: &Entry) -> &[Entry] {
            match e {
                Entry::Tool(t) => t.run.as_ref().map_or(&[][..], |r| &r.steps[..]),
                _ => &[],
            }
        }
        for e in self.entries[start..].iter().flat_map(|e| steps(e).iter().chain(std::iter::once(e))) {
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
            Entry::User { text, .. } | Entry::Assistant { text, .. } | Entry::Thinking { text } | Entry::Notice { text, .. } => text.clone(),
            Entry::Tool(t) => {
                let mut text = format!("{} {} {}", t.summary, t.detail, t.result.clone().unwrap_or_default());
                // An agent's steps are found in it too.
                for step in t.run.iter().flat_map(|r| &r.steps) {
                    text.push(' ');
                    text.push_str(&Self::entry_text(step));
                }
                text
            }
            Entry::Permission(p) => format!("{} {}", p.summary, p.detail),
            Entry::Note(n) => format!("{} {}", n.text, n.detail),
        }
    }

    /// A row above the entries when earlier ones are not in the list yet.
    fn head(&self) -> usize {
        usize::from(self.first > 0)
    }

    fn rows(&self) -> usize {
        self.entries.len() - self.first.min(self.entries.len()) + self.head()
    }

    /// The list row of an entry (one in the list).
    fn row(&self, entry: usize) -> usize {
        entry.saturating_sub(self.first) + self.head()
    }

    /// The entry a list row shows; `None` for the row of earlier entries.
    fn entry_at(&self, row: usize) -> Option<usize> {
        row.checked_sub(self.head()).map(|r| r + self.first)
    }

    /// The entry at the top of the view.
    fn top_entry(&self) -> (usize, gpui::Pixels) {
        let top = self.list.logical_scroll_top();
        match self.entry_at(top.item_ix) {
            Some(e) => (e.min(self.entries.len()), top.offset_in_item),
            None => (self.first, px(0.0)),
        }
    }

    /// Takes earlier entries into the list, from `from` on.
    fn show_from(&mut self, from: usize) {
        if from >= self.first {
            return;
        }
        let old_head = self.head();
        let added = self.first - from;
        self.first = from;
        self.list.splice(0..old_head, added + self.head());
    }

    /// Makes sure an entry is in the list (and some before it, for context).
    fn reveal(&mut self, entry: usize) {
        if entry < self.first {
            self.show_from(entry.saturating_sub(10));
        }
    }

    /// The next entries up, keeping the view where it is.
    fn show_earlier(&mut self, all: bool, cx: &mut Context<Self>) {
        if self.first == 0 {
            return;
        }
        let (entry, offset) = self.top_entry();
        self.show_from(if all { 0 } else { self.first.saturating_sub(SHOWN_AT_FIRST) });
        self.list.scroll_to(gpui::ListOffset { item_ix: self.row(entry), offset_in_item: offset });
        cx.notify();
    }

    /// The row above the entries, while there are earlier ones: shown, it
    /// brings the next ones in.
    fn render_earlier(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = current_ui(cx);
        let view = cx.entity().downgrade();
        let n = self.first;
        let link = |id: &'static str, label: &'static str| {
            div().id(id).px_1p5().rounded_sm().cursor_pointer().text_color(hsla(ui.ink(self.accent))).hover(|s| s.bg(hsla(ui.tab_hover))).child(label)
        };
        div()
            .flex()
            .justify_center()
            .items_center()
            .gap_2()
            .py_2()
            .text_size(px(12.0 * self.z()))
            .text_color(hsla(ui.text_dim))
            .child(format!("{n} earlier {}", if n == 1 { "entry" } else { "entries" }))
            .child(link("show-earlier", "Show Earlier").on_click(cx.listener(|this, _, _, cx| this.show_earlier(false, cx))))
            .child(link("show-all", "Show All").on_click(cx.listener(|this, _, _, cx| this.show_earlier(true, cx))))
            // Drawn means scrolled to: the next ones come in (measuring
            // lays it out without drawing it).
            .child(gpui::canvas(|_, _, _| {}, move |_, _, _, cx| {
                let view = view.clone();
                cx.defer(move |cx| {
                    view.update(cx, |this, cx| this.show_earlier(false, cx)).ok();
                });
            }).size_0())
            .into_any_element()
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
            self.reveal(i);
            self.list.scroll_to_reveal_item(self.row(i));
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
            menu::item("Review Changes", |this: &mut Self, window, cx| this.open_review(review::Scope::Turn, window, cx)).icon("branch"),
            menu::item("Find in the Conversation…", |this: &mut Self, window, cx| this.open_find(window, cx)).icon("search"),
            if self.ai.browser {
                menu::item("Disconnect from the Browser", |this: &mut Self, _, _| this.set_browser(false)).icon("globe")
            } else {
                menu::item("Connect to the Browser", |this: &mut Self, _, _| this.set_browser(true)).icon("globe")
            },
            menu::item("Attach Files…", |this: &mut Self, _, cx| this.attach(false, cx)).icon("paperclip"),
            menu::item("Attach a Folder…", |this: &mut Self, _, cx| this.attach(true, cx)).icon("folder"),
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

    /// How often the view is drawn again by itself: a few times a second
    /// while Claude works (its dots and time), every second while an agent
    /// runs in the background or a wake-up is near, now and then while one
    /// waits further off; never otherwise.
    fn tick_every(&self) -> Option<Duration> {
        if self.status.state != AiState::Idle {
            return Some(Duration::from_millis(350));
        }
        let now = unix_now();
        // Counting in seconds: a run in its first minute, a timer in its last.
        let counted = |t: &&Timer| t.at > 0 && t.at + OVERDUE > now;
        let fresh = self.running && self.newest_run + 60 > now;
        if fresh || self.status.timers.iter().filter(counted).any(|t| t.at <= now + 60) {
            return Some(Duration::from_secs(1));
        }
        // An agent that ended leaves the strip after a minute.
        if self.agents.iter().any(|&i| self.entries.get(i).and_then(agent_run).is_some_and(|(_, r)| !r.running() && r.ended + AGENT_KEPT > now)) {
            return Some(Duration::from_secs(5));
        }
        // In minutes, the rest: runs going on for long (a server in the
        // background), timers further off. Nothing to count: no drawing.
        (self.running || self.status.timers.iter().any(|t| counted(&t))).then_some(Duration::from_secs(30))
    }

    /// The mark of something running: turning while Claude works, still
    /// otherwise (a server left running would keep the window drawing).
    fn turning(&self) -> &'static str {
        if self.status.state == AiState::Idle { "◐" } else { ["◐", "◓", "◑", "◒"][self.spin % 4] }
    }

    /// Keeps the "working for" time, agents' times and countdowns current.
    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.tick_every().is_none() {
            self.ticker = None;
            return;
        }
        if self.ticker.is_some() {
            return;
        }
        self.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                let Ok(Some(every)) = this.update(cx, |this, _| this.tick_every()) else { break };
                cx.background_executor().timer(every).await;
                let ticked = this.update(cx, |this, cx| {
                    this.spin = this.spin.wrapping_add(1);
                    // Nothing to keep up any more: a later change starts it again.
                    if this.tick_every().is_none() {
                        this.ticker = None;
                    }
                    cx.notify();
                });
                if ticked.is_err() {
                    break;
                }
            }
        }));
    }

    /// Marks the tab as another workspace's: its sessions list, review and
    /// pictures come from there, never from this PC's files.
    pub fn set_remote(&mut self, workspace: RemoteWorkspace, cx: &mut Context<Self>) {
        self.remote = Some(workspace);
        self.sessions_task = None;
        self.sessions = Arc::new(Vec::new());
        self.load_sessions(cx);
        cx.notify();
    }

    fn load_sessions(&mut self, cx: &mut Context<Self>) {
        if let Some(workspace) = self.remote.clone() {
            return self.load_remote_sessions(workspace, cx);
        }
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

    /// The message that changes the settings `change` changes, and only
    /// those (another window may be changing others).
    fn settings_msg(&self, change: impl FnOnce(&mut AiTab)) -> ClientMsg {
        let mut ai = self.ai.clone();
        change(&mut ai);
        let old = &self.ai;
        ClientMsg::AiChange {
            tab: self.tab,
            change: AiChange::Settings {
                model: (ai.model != old.model).then_some(ai.model),
                mode: (ai.mode != old.mode).then_some(ai.mode),
                effort: (ai.effort != old.effort).then_some(ai.effort),
                follow: (ai.follow != old.follow).then_some(ai.follow),
                browser: (ai.browser != old.browser).then_some(ai.browser),
                add_dirs: (ai.add_dirs != old.add_dirs).then_some(ai.add_dirs),
            },
        }
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = &self.completion
            && let Some(&i) = c.items.get(c.selected)
        {
            return self.complete(i, cx);
        }
        let mut text = self.prompt.read(cx).buffer.text().trim_end().to_string();
        if text.trim().is_empty() && self.images.is_empty() && self.files.is_empty() {
            return;
        }
        // Files still on their way to another workspace go with it once there.
        if self.uploading > 0 {
            self.attach_problem = Some("Wait for the files to reach the workspace, then send.".into());
            cx.notify();
            return;
        }
        if self.editing.is_some() && self.status.state != AiState::Idle {
            return;
        }
        // Sent from going back through earlier prompts: what was being
        // written before comes back into the prompt.
        let mut unsent = self.unsent.take().unwrap_or_default();
        // A draft being edited goes as it is now, and leaves the drafts.
        if let Some(edit) = self.editing_draft.take() {
            let was = |d: &shed_model::Draft| d.session == edit.session && d.text == edit.text;
            let at = if self.ai.drafts.get(edit.index).is_some_and(was) { Some(edit.index) } else { self.ai.drafts.iter().position(was) };
            if let Some(i) = at {
                self.remove_drafts(vec![i]);
            }
            unsent = edit.before;
        }
        self.history = None;
        // Files go as their paths, for Claude to read.
        let files = std::mem::take(&mut self.files);
        if !files.is_empty() {
            let list: Vec<String> = files.iter().map(|f| format!("- {}", f.display())).collect();
            text = format!("{}{}Attached files:\n{}", text, if text.is_empty() { "" } else { "\n\n" }, list.join("\n"));
        }
        self.previews.clear();
        let images = std::mem::take(&mut self.images);
        if self.status.state != AiState::Idle {
            // Claude is busy: the keeper keeps it in the tab's queue (with
            // the bench, so it survives a restart) and sends it when the turn
            // ends. (An earlier message is only edited when it is idle.)
            self.conn.send_msg(ClientMsg::AiSend { tab: self.tab, text, images });
            self.prompt.update(cx, |p, cx| {
                p.buffer.select_all();
                p.buffer.insert(&unsent);
                cx.notify();
            });
            cx.notify();
            return;
        }
        match self.editing.take() {
            Some(entry) => self.conn.send_msg(ClientMsg::AiEdit { tab: self.tab, entry: entry as u32, text, images }),
            None => self.conn.send_msg(ClientMsg::AiSend { tab: self.tab, text, images }),
        }
        self.prompt.update(cx, |p, cx| {
            p.buffer.select_all();
            p.buffer.insert(&unsent);
            cx.notify();
        });
        self.completion = None;
        self.list.scroll_to_end();
        cx.notify();
    }

    /// Keeps a draft, here at once (a second change before the keeper
    /// answers builds on the first) and in the keeper.
    fn add_draft(&mut self, session: String, text: String) {
        self.ai.drafts.push(shed_model::Draft { session: session.clone(), text: text.clone() });
        self.conn.send_msg(ClientMsg::AiChange { tab: self.tab, change: AiChange::AddDraft { session: Some(session), text } });
    }

    /// Changes a draft's text in place, here and in the keeper (which
    /// changes it only if it is still as it was).
    fn replace_draft(&mut self, index: usize, text: String) {
        let Some(d) = self.ai.drafts.get_mut(index) else { return };
        let (session, old) = (d.session.clone(), std::mem::replace(&mut d.text, text.clone()));
        self.conn.send_msg(ClientMsg::AiChange { tab: self.tab, change: AiChange::EditDraft { index: index as u32, session, old, text } });
    }

    /// Takes out the drafts at `indices`, here and in the keeper (which takes
    /// out only those still as they were).
    fn remove_drafts(&mut self, mut indices: Vec<usize>) {
        indices.sort_unstable();
        indices.dedup();
        for n in indices.into_iter().rev() {
            if n < self.ai.drafts.len() {
                let d = self.ai.drafts.remove(n);
                self.conn.send_msg(ClientMsg::AiChange {
                    tab: self.tab,
                    change: AiChange::RemoveDraft { index: n as u32, session: d.session, text: d.text },
                });
            }
        }
    }

    /// Keeps what is in the prompt as a draft, to send some other time:
    /// attached files and pictures go with it as paths.
    fn save_draft(&mut self, cx: &mut Context<Self>) {
        if self.editing_draft.is_some() {
            return self.finish_draft_edit(true, cx);
        }
        // Another workspace's drafts name its own files: pictures go there first.
        if self.pictures_to_upload(cx) {
            return;
        }
        let mut text = self.prompt.read(cx).buffer.text().trim_end().to_string();
        let dir = shed_model::paths::state_dir().join("ai-pictures");
        let mut files: Vec<String> = self.images.iter().filter_map(|i| shed_ai::keep_picture_in(&dir, &i.media_type, &i.data)).collect();
        files.extend(self.files.iter().map(|f| f.display().to_string()));
        if text.trim().is_empty() && files.is_empty() {
            return;
        }
        if !files.is_empty() {
            let list: Vec<String> = files.iter().map(|f| format!("- {f}")).collect();
            text = format!("{}{}Attached files:\n{}", text, if text.is_empty() { "" } else { "\n\n" }, list.join("\n"));
        }
        self.images.clear();
        self.previews.clear();
        self.files.clear();
        // The session's; in a new session, the first draft makes it a draft
        // session, listed with the others.
        let session = match self.ai.session_key() {
            Some(key) => key.to_string(),
            None => {
                let key = AiTab::new_draft_key();
                self.ai.draft_session = Some(key.clone());
                key
            }
        };
        self.add_draft(session, text);
        self.drafts_open = true;
        // Kept from going back through earlier prompts: what was being written comes back.
        let unsent = self.unsent.take().unwrap_or_default();
        self.history = None;
        self.prompt.update(cx, |p, cx| {
            p.buffer.select_all();
            p.buffer.insert(&unsent);
            cx.notify();
        });
        cx.notify();
    }

    /// Sends a draft now (after the current turn, when Claude is busy).
    fn send_draft(&mut self, n: usize, cx: &mut Context<Self>) {
        let Some(text) = self.ai.drafts.get(n).map(|d| d.text.clone()) else { return };
        self.remove_drafts(vec![n]);
        if let Some(e) = self.editing_draft.as_mut().filter(|e| e.index > n) {
            e.index -= 1;
        }
        // Busy, the keeper queues it.
        let idle = self.status.state == AiState::Idle;
        self.conn.send_msg(ClientMsg::AiSend { tab: self.tab, text, images: Vec::new() });
        if idle {
            self.list.scroll_to_end();
        }
        cx.notify();
    }

    /// Opens a draft in the prompt to change it: what was being written is
    /// set aside until the edit ends. The draft stays in the list meanwhile;
    /// a draft already open is put back with its changes first.
    fn edit_draft(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.ai.drafts.get(n).cloned() else { return };
        if self.editing_draft.as_ref().is_some_and(|d| d.index == n) {
            return;
        }
        let before = match self.editing_draft.is_some() {
            true => {
                let before = self.editing_draft.as_ref().map(|d| d.before.clone()).unwrap_or_default();
                self.finish_draft_edit(true, cx);
                before
            }
            false => {
                self.cancel_edit(cx);
                self.prompt.read(cx).buffer.text()
            }
        };
        // Saving the one open before may have moved this one.
        let index = if self.ai.drafts.get(n) == Some(&draft) {
            n
        } else {
            match self.ai.drafts.iter().position(|d| *d == draft) {
                Some(i) => i,
                None => return,
            }
        };
        self.editing_draft = Some(DraftEdit { index, session: draft.session.clone(), text: draft.text.clone(), before });
        self.history = None;
        self.prompt.update(cx, |p, cx| {
            p.buffer.select_all();
            p.buffer.insert(&draft.text);
            cx.notify();
        });
        let focus = self.prompt.read(cx).focus_handle().clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Ends the draft edit: `save` puts the prompt's text (and anything
    /// attached) into the draft, in its place; otherwise the draft stays as
    /// it was. Either way, what was being written before comes back.
    fn finish_draft_edit(&mut self, save: bool, cx: &mut Context<Self>) {
        if save && self.editing_draft.is_some() && self.pictures_to_upload(cx) {
            return;
        }
        let Some(edit) = self.editing_draft.take() else { return };
        if save {
            let mut text = self.prompt.read(cx).buffer.text().trim_end().to_string();
            let dir = shed_model::paths::state_dir().join("ai-pictures");
            let mut files: Vec<String> = self.images.iter().filter_map(|i| shed_ai::keep_picture_in(&dir, &i.media_type, &i.data)).collect();
            files.extend(self.files.iter().map(|f| f.display().to_string()));
            if !files.is_empty() {
                let list: Vec<String> = files.iter().map(|f| format!("- {f}")).collect();
                text = format!("{}{}Attached files:\n{}", text, if text.is_empty() { "" } else { "\n\n" }, list.join("\n"));
            }
            self.images.clear();
            self.previews.clear();
            self.files.clear();
            let was = |d: &shed_model::Draft| d.session == edit.session && d.text == edit.text;
            let at = if self.ai.drafts.get(edit.index).is_some_and(was) { Some(edit.index) } else { self.ai.drafts.iter().position(was) };
            if text.trim().is_empty() {
                // Emptied: nothing to keep (the draft stays as it was).
            } else if let Some(i) = at {
                self.replace_draft(i, text);
            } else {
                // Gone meanwhile (sent or deleted elsewhere): kept as a new one.
                self.add_draft(edit.session.clone(), text);
            }
        }
        let before = edit.before;
        self.prompt.update(cx, |p, cx| {
            p.buffer.select_all();
            p.buffer.insert(&before);
            cx.notify();
        });
        cx.notify();
    }

    /// Adds a draft's text at the end of the prompt, after a blank line,
    /// leaving the draft in the list (while another draft is being edited,
    /// into that one). The cursor goes to the end, in the prompt.
    fn merge_draft(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.ai.drafts.get(n).cloned() else { return };
        self.prompt.update(cx, |p, cx| {
            let merged = draft.merged_into(&p.buffer.text());
            p.buffer.select_all();
            p.buffer.insert(&merged);
            cx.notify();
        });
        let focus = self.prompt.read(cx).focus_handle().clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    fn delete_draft(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.ai.drafts.get(n).cloned() else { return };
        let first: String = draft.text.lines().next().unwrap_or("").chars().take(80).collect();
        let answer = window.prompt(gpui::PromptLevel::Warning, "Delete this draft?", Some(&first), &["Delete Draft", "Cancel"], cx);
        cx.spawn(async move |this, cx| {
            if matches!(answer.await, Ok(0)) {
                this.update(cx, |this, cx| {
                    // Still there, where it was (another window may have changed the list).
                    if this.ai.drafts.get(n) == Some(&draft) {
                        if this.editing_draft.as_ref().is_some_and(|e| e.index == n) {
                            this.finish_draft_edit(false, cx);
                        }
                        this.remove_drafts(vec![n]);
                        // A draft open for editing after this one moved up.
                        if let Some(e) = this.editing_draft.as_mut().filter(|e| e.index > n) {
                            e.index -= 1;
                        }
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Keeps what is in the prompt with the tab (saved with the bench), a
    /// moment after the last change, so closing Shed does not lose it.
    fn save_unsent_soon(&mut self, cx: &mut Context<Self>) {
        self.unsent_save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(800)).await;
            this.update(cx, |this, cx| {
                let text = this.prompt.read(cx).buffer.text();
                if text != this.ai.unsent {
                    this.ai.unsent = text.clone();
                    this.conn.send_msg(ClientMsg::AiChange { tab: this.tab, change: AiChange::Unsent(text) });
                }
            })
            .ok();
        }));
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
                "enter" => {
                    let i = c.items[c.selected];
                    self.complete(i, cx);
                }
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        match ks.key.as_str() {
            // Enter breaks the line; Ctrl+Enter (Cmd+Enter) sends.
            "enter" if sec => self.send(cx),
            "escape" if self.editing.is_some() => self.cancel_edit(cx),
            "escape" if self.editing_draft.is_some() => self.finish_draft_edit(false, cx),
            "escape" if self.status.state != AiState::Idle => self.stop(),
            "f" if sec => self.open_find(window, cx),
            // Ctrl+Shift+R: the review of what changed.
            "r" if sec && ks.modifiers.shift => self.open_review(review::Scope::Turn, window, cx),
            // (Ctrl+Shift+arrows go from message to message.)
            "up" | "down" if sec && !ks.modifiers.shift => {
                let prompts: Vec<&String> = self.entries.iter().filter_map(|e| if let Entry::User { text, .. } = e { Some(text) } else { None }).collect();
                if prompts.is_empty() {
                    return;
                }
                let back = match (self.history, ks.key.as_str()) {
                    (None, "up") => {
                        // Kept, not lost: it comes back below the newest prompt.
                        self.unsent = Some(self.prompt.read(cx).buffer.text());
                        1
                    }
                    (Some(n), "up") => (n + 1).min(prompts.len()),
                    (Some(n), _) if n > 1 => n - 1,
                    // Not going through them: nothing to come down to.
                    (None, _) => return,
                    _ => {
                        self.history = None;
                        let text = self.unsent.take().unwrap_or_default();
                        self.prompt.update(cx, |p, cx| {
                            p.buffer.select_all();
                            p.buffer.insert(&text);
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
                // Files copied in a file manager: attached, as when dropped.
                let files: Vec<PathBuf> = item
                    .entries()
                    .iter()
                    .filter_map(|e| match e {
                        ClipboardEntry::ExternalPaths(p) => Some(p.paths().to_vec()),
                        _ => None,
                    })
                    .flatten()
                    .collect();
                if !files.is_empty() {
                    self.attach_paths(&files, cx);
                    cx.stop_propagation();
                    return;
                }
                let image = item.entries().iter().find_map(|e| match e {
                    ClipboardEntry::Image(i) => Some(i.clone()),
                    _ => None,
                });
                let Some(image) = image else { return };
                let media_type = image.format.mime_type().to_string();
                self.add_picture(AiImage { media_type, data: image.bytes });
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    /// Dropped files go with the prompt: pictures as pictures (with a
    /// thumbnail), the others as their paths.
    fn on_drop(&mut self, paths: &ExternalPaths, cx: &mut Context<Self>) {
        self.attach_paths(paths.paths(), cx);
    }

    /// Asks for files (or a folder) to go with the prompt.
    fn attach(&mut self, folder: bool, cx: &mut Context<Self>) {
        let options = gpui::PathPromptOptions { files: !folder, directories: folder, multiple: true, prompt: Some("Attach".into()) };
        let answer = cx.prompt_for_paths(options);
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = answer.await {
                this.update(cx, |this, cx| this.attach_paths(&paths, cx)).ok();
            }
        })
        .detach();
    }

    /// Files and folders to go with the prompt (dropped, pasted, picked):
    /// pictures as pictures, the rest as their paths for Claude to read.
    fn attach_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        self.attach_problem = None;
        for path in paths {
            let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
            let media_type = match ext.as_deref() {
                Some("png") => Some("image/png"),
                Some("jpg" | "jpeg") => Some("image/jpeg"),
                Some("gif") => Some("image/gif"),
                Some("webp") => Some("image/webp"),
                _ => None,
            };
            let small = std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() <= 10 * 1024 * 1024);
            match media_type.filter(|_| small).and_then(|t| Some((t, std::fs::read(path).ok()?))) {
                Some((media_type, data)) => self.add_picture(AiImage { media_type: media_type.into(), data }),
                // Another workspace reads its own disk: the file goes there.
                None if self.remote.is_some() => self.upload_file(path.clone(), cx),
                None => {
                    if !self.files.contains(path) {
                        self.files.push(path.clone());
                    }
                }
            }
        }
        cx.notify();
    }

    fn add_picture(&mut self, image: AiImage) {
        let format = gpui::ImageFormat::from_mime_type(&image.media_type).unwrap_or(gpui::ImageFormat::Png);
        self.previews.push(Arc::new(gpui::Image::from_bytes(format, image.data.clone())));
        self.images.push(image);
    }

    /// What goes with the prompt: thumbnails of the pictures and the names of
    /// the files (the path in a tooltip), each with a × to leave it out.
    fn render_attachments(&self, ui: &UiColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let note = match (self.uploading, &self.attach_problem) {
            (0, None) => None,
            (n, None) => Some((format!("Sending {n} file{} to the workspace…", if n == 1 { "" } else { "s" }), ui.text_dim)),
            (_, Some(problem)) => Some((problem.clone(), ui.danger)),
        };
        if self.images.is_empty() && self.files.is_empty() {
            let (text, color) = note?;
            return Some(div().mx_3().mt_2().text_size(px(12.0 * self.z())).text_color(hsla(color)).child(text).into_any_element());
        }
        let z = self.z();
        let remove = |id: SharedString| {
            div()
                .id(id)
                .absolute()
                .top(px(-6.0))
                .right(px(-6.0))
                .size(px(18.0 * z))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .bg(hsla(ui.tab_active))
                .border_1()
                .border_color(hsla(ui.divider))
                .hover(|s| s.bg(hsla(ui.tab_hover)))
                .tooltip(menu::tooltip("Leave it out"))
                .child(svg().path(icon("close")).size(px(10.0 * z)).text_color(hsla(ui.text)))
        };
        let mut row = div().mx_3().mt_2().flex().flex_wrap().items_end().gap_3();
        for (i, preview) in self.previews.iter().enumerate() {
            row = row.child(
                div()
                    .relative()
                    .flex_none()
                    .rounded_md()
                    .border_1()
                    .border_color(hsla(ui.divider))
                    .bg(hsla(ui.tab_active))
                    .p_0p5()
                    .child(gpui::img(preview.clone()).h(px(56.0 * z)).max_w(px(120.0 * z)).rounded_sm())
                    .child(remove(SharedString::from(format!("drop-picture-{i}"))).on_click(cx.listener(move |this, _, _, cx| {
                        if i < this.images.len() {
                            this.images.remove(i);
                            this.previews.remove(i);
                        }
                        cx.notify();
                    }))),
            );
        }
        for (i, file) in self.files.iter().enumerate() {
            let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| file.display().to_string());
            row = row.child(
                div()
                    .id(("attached-file", i))
                    .relative()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(hsla(ui.divider))
                    .bg(hsla(ui.tab_active))
                    .text_size(px(12.0 * z))
                    .text_color(hsla(ui.text))
                    .tooltip(menu::tooltip_text(file.display().to_string()))
                    .child(svg().path(icon(if file.is_dir() { "folder" } else { "file" })).size(px(13.0 * z)).text_color(hsla(ui.text_dim)))
                    .child(div().max_w(px(220.0 * z)).truncate().child(name))
                    .child(remove(SharedString::from(format!("drop-file-{i}"))).on_click(cx.listener(move |this, _, _, cx| {
                        if i < this.files.len() {
                            this.files.remove(i);
                        }
                        cx.notify();
                    }))),
            );
        }
        if let Some((text, color)) = note {
            row = row.child(div().text_size(px(12.0 * z)).text_color(hsla(color)).child(text));
        }
        Some(row.into_any_element())
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

    /// Names a session (or gives Claude Code's own title back, when the name
    /// is emptied), in an editor in place of its title.
    fn start_rename(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.sessions.iter().find(|s| s.id == id).map(|s| s.title.clone()).unwrap_or_default();
        let editor = cx.new(|cx| LineEditor::selected(title, cx));
        let sub = cx.subscribe(&editor, |this, _, e: &EditorEvent, cx| {
            let Some((id, _, _)) = this.renaming.take() else { return };
            if let EditorEvent::Submit(name) = e {
                if let Some(workspace) = this.remote.clone() {
                    this.rename_remote_session(workspace, id, name.clone(), cx);
                } else {
                    if let Err(e) = shed_ai::rename_session(&this.ai.folder, &id, name) {
                        tracing::warn!("cannot rename the session: {e}");
                    }
                    this.load_sessions(cx);
                }
            }
            cx.notify();
        });
        let focus = editor.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        self.renaming = Some((id, editor, sub));
        cx.notify();
    }

    /// A session row's menu: `id` the session it opens, `delete` what
    /// Delete Session takes (a conversation's row: its first file, so all of it).
    fn session_menu(&self, id: String, delete: ToDelete) -> Vec<MenuEntry<Self>> {
        let (a, b, c) = (id.clone(), id.clone(), id.clone());
        vec![
            menu::item("Open", move |this: &mut Self, _, _| this.open_session(Some(a.clone()))).icon("go"),
            menu::item("Rename…", move |this: &mut Self, window, cx| this.start_rename(c.clone(), window, cx)).icon("pencil"),
            menu::item("Continue in a Terminal", move |this: &mut Self, _, _| this.continue_in_terminal(&b)).icon("terminal"),
            menu::item("Copy Session ID", move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(id.clone()))),
            MenuEntry::Separator,
            menu::danger("Delete Session…", move |this: &mut Self, window, cx| this.delete_session(delete.clone(), window, cx)).icon("trash"),
        ]
    }

    /// Deletes a Claude Code session and the sessions going on from it,
    /// after asking: the keeper moves their files to Shed's trash.
    fn delete_session(&mut self, target: ToDelete, window: &mut Window, cx: &mut Context<Self>) {
        let later = shed_ai::going_on_from(&self.sessions, &target.id);
        let mut ids = later.clone();
        ids.push(target.id.clone());
        // Shown in another tab: its Claude Code may be writing to it. (Another
        // workspace's session folders are its own: tabs there are compared by folder.)
        let remote = self.remote.is_some();
        let dir = if remote { PathBuf::new() } else { shed_ai::sessions::sessions_dir(&self.ai.folder) };
        let folder = self.ai.folder.clone();
        let elsewhere = cx.try_global::<crate::state::AppHandle>().and_then(|h| h.0.upgrade()).is_some_and(|state| {
            state.read(cx).tree.benches.iter().flat_map(|b| &b.tabs).filter(|t| t.id != self.tab).filter_map(|t| t.ai_info()).any(|a| {
                a.session.as_ref().is_some_and(|s| ids.contains(s)) && if remote { a.folder == folder } else { shed_ai::sessions::sessions_dir(&a.folder) == dir }
            })
        });
        if elsewhere {
            let detail = "It is open in another AI tab. Close it there first (or open another session there), then delete it here.";
            let answer = window.prompt(gpui::PromptLevel::Info, "This session is open in another tab", Some(detail), &["OK"], cx);
            cx.spawn(async move |_, _| {
                let _ = answer.await;
            })
            .detach();
            return;
        }
        let prompts = format!("{} prompt{}", target.prompts, if target.prompts == 1 { "" } else { "s" });
        let more = match later.len() {
            0 => String::new(),
            1 => ", and 1 later session that continues it".to_string(),
            n => format!(", and {n} later sessions that continue it"),
        };
        let trash = if self.remote.is_some() { "on that workspace".to_string() } else { shed_save::home_relative(&shed_model::paths::state_dir().join("ai-trash")) };
        let detail = format!(
            "“{}”\n{} ({}) · {prompts}{more}.\n\nClaude Code's files for it move to Shed's trash ({trash}), kept 30 days, then deleted.",
            target.title,
            local_time(target.modified),
            ago(target.modified),
        );
        let answer = window.prompt(gpui::PromptLevel::Critical, "Delete this Claude Code session?", Some(&detail), &["Delete Session", "Cancel"], cx);
        cx.spawn(async move |this, cx| {
            if !matches!(answer.await, Ok(0)) {
                return;
            }
            this.update(cx, |this, cx| {
                this.delete_problem = None;
                if let Some(workspace) = this.remote.clone() {
                    return this.delete_remote_session(workspace, target.id.clone(), cx);
                }
                this.conn.send_msg(ClientMsg::AiDeleteSession { tab: this.tab, session: target.id.clone() });
                // The list again once its file is gone (or after a while).
                let file = dir.join(format!("{}.jsonl", target.id));
                let watch = cx.spawn(async move |this, cx| {
                    for _ in 0..50 {
                        cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                        if !file.exists() {
                            break;
                        }
                    }
                    this.update(cx, |this, cx| {
                        this.deleting = None;
                        this.load_sessions(cx);
                    })
                    .ok();
                });
                this.deleting = Some((target.id.clone(), watch));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// An error from the keeper: shown, when it can be about a session being deleted.
    pub fn keeper_error(&mut self, error: &str, cx: &mut Context<Self>) {
        if self.deleting.take().is_some() {
            self.delete_problem = Some(error.to_string());
            self.load_sessions(cx);
            cx.notify();
        }
    }

    fn entry_menu(&self, i: usize) -> Vec<MenuEntry<Self>> {
        let text = match self.entries.get(i) {
            Some(Entry::User { text, .. } | Entry::Assistant { text, .. } | Entry::Thinking { text } | Entry::Notice { text, .. }) => text.clone(),
            Some(Entry::Tool(t)) => format!("{}\n{}\n{}", t.summary, t.detail, t.result.clone().unwrap_or_default()),
            Some(Entry::Permission(p)) => format!("{}\n{}", p.summary, p.detail),
            Some(Entry::Note(n)) => format!("{}\n{}", n.text, n.detail).trim_end().to_string(),
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
        if let Some(Entry::User { .. }) = self.entries.get(i) {
            if self.status.state == AiState::Idle {
                entries.push(menu::item("Edit and Send Again…", move |this: &mut Self, window, cx| this.edit_prompt(i, window, cx)).icon("pencil"));
                entries.push(menu::item("Send Again", move |this: &mut Self, window, cx| this.resend(i, window, cx)).icon("refresh"));
                entries.push(menu::danger("Revert to Here…", move |this: &mut Self, window, cx| this.revert_to(i, window, cx)).icon("discard"));
                entries.push(MenuEntry::Separator);
            }
        } else {
            let response = self.response_markdown(self.response_of(i));
            if !response.is_empty() {
                entries.push(menu::item("Copy Response", move |_, _, cx| cx.write_to_clipboard(ai_text::formatted_item(response.clone(), &response))).icon("copy"));
            }
        }
        entries.push(menu::item("Copy Message", move |_, _, cx| cx.write_to_clipboard(ai_text::formatted_item(text.clone(), &text))).icon("copy"));
        entries.push(menu::item("Copy Message as Plain Text", move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(plain.clone()))));
        entries.push(menu::item("Copy the Whole Conversation", move |_, _, cx| cx.write_to_clipboard(ai_text::formatted_item(all.clone(), &all))));
        if self.expanded.iter().any(|&e| matches!(self.entries.get(e), Some(Entry::Tool(_) | Entry::Thinking { .. } | Entry::Note(_)))) {
            entries.push(MenuEntry::Separator);
            entries.push(menu::item("Collapse All Steps", |this: &mut Self, _, cx| this.collapse_all(cx)));
        }
        entries
    }

    /// Closes an opened step (or thinking).
    fn collapse(&mut self, i: usize, cx: &mut Context<Self>) {
        if self.expanded.remove(&i) && i >= self.first {
            let row = self.row(i);
            self.list.remeasure_items(row..row + 1);
            // Its line back in view, if the view was further down it.
            if self.top_entry().0 > i {
                self.list.scroll_to(gpui::ListOffset { item_ix: row, offset_in_item: px(0.0) });
            }
        }
        cx.notify();
    }

    /// Closes every opened step and thinking (your long messages stay as they are).
    fn collapse_all(&mut self, cx: &mut Context<Self>) {
        let entries = &self.entries;
        self.expanded.retain(|&e| !matches!(entries.get(e), Some(Entry::Tool(_) | Entry::Thinking { .. } | Entry::Note(_))));
        self.list.remeasure();
        cx.notify();
    }

    /// Scrolls to the start of the previous (`up`) or next message: each of
    /// yours, and each response to one. Past the last, to the end.
    fn jump_block(&mut self, up: bool, cx: &mut Context<Self>) {
        let (top, offset) = self.top_entry();
        match block_target(&self.entries, top, offset > px(1.0), up) {
            Some(entry) => {
                self.reveal(entry);
                self.list.scroll_to(gpui::ListOffset { item_ix: self.row(entry), offset_in_item: px(0.0) });
            }
            None if !up => self.list.scroll_to_end(),
            None => return,
        }
        cx.notify();
    }

    /// Picks option `o` of question `q` (one, or one more of several).
    fn pick(&mut self, request: &str, questions: &[shed_ai::Question], q: usize, o: usize, cx: &mut Context<Self>) {
        let ask = self.asks.entry(request.to_string()).or_insert_with(|| Ask::new(questions.len()));
        let picks = &mut ask.picks[q];
        if questions[q].multi {
            if !picks.remove(&o) {
                picks.insert(o);
            }
        } else {
            picks.clear();
            picks.insert(o);
        }
        // One question, one answer, nothing typed: that is the answer.
        let quick = questions.len() == 1 && !questions[0].multi && ask.other[0].as_ref().is_none_or(|e| e.read(cx).text().trim().is_empty());
        if quick {
            self.send_answers(request, questions, cx);
        }
        self.remeasure_request(request);
        cx.notify();
    }

    /// The entry of a request changed height (a pick, a field opened).
    fn remeasure_request(&self, request: &str) {
        if let Some(i) = self.entries.iter().position(|e| matches!(e, Entry::Permission(p) if p.request == request))
            && i >= self.first
        {
            let row = self.row(i);
            self.list.remeasure_items(row..row + 1);
        }
    }

    /// The answers chosen so far, one per question; `None` until each has one.
    fn answers(&self, request: &str, questions: &[shed_ai::Question], cx: &gpui::App) -> Option<Vec<String>> {
        let ask = self.asks.get(request)?;
        questions
            .iter()
            .enumerate()
            .map(|(qi, q)| {
                let mut parts: Vec<String> = ask.picks[qi].iter().filter_map(|&o| q.options.get(o)).map(|o| o.label.clone()).collect();
                if let Some(other) = ask.other[qi].as_ref().map(|e| e.read(cx).text().trim().to_string()).filter(|t| !t.is_empty()) {
                    parts.push(other);
                }
                (!parts.is_empty()).then(|| parts.join(", "))
            })
            .collect()
    }

    fn send_answers(&mut self, request: &str, questions: &[shed_ai::Question], cx: &mut Context<Self>) {
        let Some(answers) = self.answers(request, questions, cx) else { return };
        self.conn.send_msg(ClientMsg::AiAnswerQuestions { tab: self.tab, request: request.to_string(), answers });
        self.asks.remove(request);
        cx.notify();
    }

    /// Claude's questions: each with its options to click (a dot for one
    /// answer, a box for several), and a field for an answer of one's own.
    fn render_questions(&mut self, i: usize, p: &shed_ai::Permission, ui: &UiColors, cx: &mut Context<Self>) -> AnyElement {
        let accent = self.accent;
        let ink = ui.ink(accent);
        let z = self.z();
        let questions = std::rc::Rc::new(p.questions.clone());
        let request = p.request.clone();
        let mut card = div()
            .id(("questions", i))
            .flex()
            .flex_col()
            .gap_3()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(hsla(if p.answer.is_none() { accent } else { ui.divider }))
            .when(p.answer.is_none(), |d| d.bg(hsla_alpha(accent, 0.05)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(svg().path(icon("ai")).size(px(14.0 * z)).text_color(hsla(ink)))
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(hsla(ui.text)).child(if p.questions.len() == 1 { "Claude asks" } else { "Claude asks a few things" })),
            );
        // Answered (or skipped): what was said.
        if let Some(allowed) = p.answer {
            for (qi, q) in p.questions.iter().enumerate() {
                let said = if allowed { p.answers.get(qi).cloned().unwrap_or_default() } else { "(skipped)".into() };
                card = card.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(div().text_size(px(12.0 * z)).text_color(hsla(ui.text_dim)).child(q.question.clone()))
                        .child(div().text_color(hsla(if allowed { ui.text } else { ui.text_dim })).child(said)),
                );
            }
            return card.into_any_element();
        }
        let ask = self.asks.entry(request.clone()).or_insert_with(|| Ask::new(questions.len()));
        let picks: Vec<std::collections::BTreeSet<usize>> = ask.picks.clone();
        let others: Vec<Option<Entity<LineEditor>>> = ask.other.clone();
        for (qi, q) in p.questions.iter().enumerate() {
            let mut block = div().flex().flex_col().gap_1();
            let mut title = div().flex().items_center().gap_2();
            if !q.header.is_empty() {
                title = title.child(div().flex_none().px_1p5().rounded_sm().text_size(px(11.0 * z)).bg(hsla_alpha(accent, 0.18)).text_color(hsla(ink)).child(q.header.clone()));
            }
            title = title.child(div().flex_1().min_w_0().font_weight(FontWeight::MEDIUM).text_color(hsla(ui.text)).child(q.question.clone()));
            if q.multi {
                title = title.child(div().flex_none().text_size(px(11.0 * z)).text_color(hsla(ui.text_dim)).child("choose any"));
            }
            block = block.child(title);
            for (oi, o) in q.options.iter().enumerate() {
                let on = picks[qi].contains(&oi);
                let (label, recommended) = match o.label.strip_suffix(" (Recommended)") {
                    Some(l) => (l.to_string(), true),
                    None => (o.label.clone(), false),
                };
                let mark = div()
                    .flex_none()
                    .mt(px(3.0 * z))
                    .size(px(12.0 * z))
                    .border_1()
                    .border_color(hsla(if on { ink } else { ui.text_dim }))
                    .when(q.multi, |d| d.rounded_sm())
                    .when(!q.multi, |d| d.rounded_full())
                    .when(on, |d| d.bg(hsla(ink)));
                let (r, qs) = (request.clone(), questions.clone());
                block = block.child(
                    div()
                        .id(SharedString::from(format!("ask-{i}-{qi}-{oi}")))
                        .flex()
                        .items_start()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .when(on, |d| d.bg(hsla_alpha(accent, 0.15)))
                        .when(!on, |d| d.hover(|s| s.bg(hsla(ui.tab_hover))))
                        .on_click(cx.listener(move |this, _, _, cx| this.pick(&r, &qs, qi, oi, cx)))
                        .child(mark)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .min_w_0()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(div().text_color(hsla(ui.text)).child(label))
                                        .when(recommended, |d| d.child(div().px_1().rounded_sm().text_size(px(10.5 * z)).bg(hsla_alpha(accent, 0.18)).text_color(hsla(ink)).child("recommended"))),
                                )
                                .when(!o.description.is_empty() && o.description != o.label, |d| d.child(div().text_size(px(12.0 * z)).text_color(hsla(ui.text_dim)).child(o.description.clone()))),
                        ),
                );
            }
            // An answer of one's own.
            block = match &others[qi] {
                Some(editor) => block.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .child(div().flex_none().text_size(px(12.0 * z)).text_color(hsla(ui.text_dim)).child("Other:"))
                        .child(div().flex_1().px_1().rounded_sm().border_1().border_color(hsla(ui.divider)).bg(hsla(ui.tab_active)).child(editor.clone())),
                ),
                None => {
                    let (r, qs) = (request.clone(), questions.clone());
                    block.child(
                        div()
                            .id(SharedString::from(format!("ask-other-{i}-{qi}")))
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .cursor_pointer()
                            .text_size(px(12.0 * z))
                            .text_color(hsla(ui.text_dim))
                            .hover(|s| s.bg(hsla(ui.tab_hover)).text_color(hsla(ui.text)))
                            .child("Other: write my own answer…")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let editor = cx.new(|cx| LineEditor::new(String::new(), cx).without_hint());
                                window.focus(&editor.read(cx).focus_handle(cx), cx);
                                let sub = cx.observe(&editor, |_, _, cx| cx.notify());
                                // Enter sends, once every question has an answer.
                                let (r2, qs2) = (r.clone(), qs.clone());
                                let submit = cx.subscribe(&editor, move |this, _, event: &crate::line_editor::EditorEvent, cx| {
                                    if let crate::line_editor::EditorEvent::Submit(_) = event {
                                        this.send_answers(&r2, &qs2, cx);
                                    }
                                });
                                if let Some(ask) = this.asks.get_mut(&r) {
                                    ask.other[qi] = Some(editor);
                                    ask.subs.extend([sub, submit]);
                                }
                                this.remeasure_request(&r);
                                cx.notify();
                            })),
                    )
                }
            };
            card = card.child(block);
        }
        let ready = self.answers(&request, &questions, cx).is_some();
        let (r1, q1, r2) = (request.clone(), questions.clone(), request.clone());
        let button = |id: &str, glyph: &'static str, label: &'static str, color: Rgb, enabled: bool| {
            div()
                .id(SharedString::from(format!("{id}-{i}")))
                .flex()
                .items_center()
                .gap_1p5()
                .px_2p5()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(hsla_alpha(color, if enabled { 0.8 } else { 0.3 }))
                .text_color(hsla_alpha(ui.ink(color), if enabled { 1.0 } else { 0.45 }))
                .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(hsla_alpha(color, 0.18))))
                .child(svg().path(icon(glyph)).size(px(13.0 * z)).text_color(hsla_alpha(ui.ink(color), if enabled { 1.0 } else { 0.45 })))
                .child(label)
        };
        card.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(12.5 * z))
                .child(button("ask-send", "send", "Send Answers", accent, ready).on_click(cx.listener(move |this, _, _, cx| this.send_answers(&r1, &q1, cx))))
                .child(button("ask-skip", "close", "Skip", ui.text_dim, true).on_click(cx.listener(move |this, _, _, cx| {
                    this.conn.send_msg(ClientMsg::AiAnswer { tab: this.tab, request: r2.clone(), allow: false, always: false });
                    this.asks.remove(&r2);
                    cx.notify();
                })))
                .when(!ready, |d| d.child(div().text_color(hsla(ui.text_dim)).child("Pick an answer for each question, or write your own"))),
        )
        .into_any_element()
    }

    /// Takes the bench's projects' file lists as they are now; when one
    /// changed, Claude's answers are read for links again.
    /// Whether the files changed.
    pub fn set_project_files(&mut self, files: Vec<(PathBuf, Arc<Vec<PathBuf>>)>) -> bool {
        let same = files.len() == self.project_files.len() && files.iter().zip(&self.project_files).all(|(a, b)| a.0 == b.0 && Arc::ptr_eq(&a.1, &b.1));
        if !same {
            self.project_files = files;
            self.file_lookups.clear();
            self.parsed.clear();
        }
        !same
    }

    /// The views of the bench's project tabs, whose file lists find the
    /// files Claude names; followed as their lists change.
    pub fn set_project_views(&mut self, views: Vec<Entity<crate::project_view::ProjectView>>, cx: &mut Context<Self>) {
        if views == self.project_views {
            return;
        }
        self._project_subs = views.iter().map(|v| cx.observe(v, |this, _, cx| this.read_project_files(cx))).collect();
        self.project_views = views;
        self.read_project_files(cx);
    }

    fn read_project_files(&mut self, cx: &mut Context<Self>) {
        let files = self.project_views.iter().map(|v| v.read(cx).file_index()).collect();
        if self.set_project_files(files) {
            cx.notify();
        }
    }

    /// Opens a file in the bench's project tab that holds it (one is opened
    /// for its project if none does), at `at`.
    fn reveal_file(&mut self, path: PathBuf, at: Option<(usize, Option<usize>)>, cx: &mut Context<Self>) {
        let bench = self.bench;
        if let Some(state) = cx.try_global::<crate::state::AppHandle>().and_then(|h| h.0.upgrade()) {
            state.update(cx, |state, cx| state.reveal_file(bench, path, at, false, cx));
        }
    }

    /// Opens a file in its project tab and shows it in that tab's Files.
    pub(super) fn go_to_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let bench = self.bench;
        if let Some(state) = cx.try_global::<crate::state::AppHandle>().and_then(|h| h.0.upgrade()) {
            state.update(cx, |state, cx| state.reveal_file(bench, path, None, true, cx));
        }
    }

    /// What Claude is doing now, in a few words: the tool running, or
    /// thinking or writing.
    fn activity(&self) -> String {
        match self.entries.last() {
            Some(Entry::Tool(t)) if t.result.is_none() => t.summary.clone(),
            Some(Entry::Thinking { .. }) => "Thinking".into(),
            Some(Entry::Assistant { .. }) => "Writing".into(),
            _ => "Working".into(),
        }
    }

    /// Puts an earlier prompt into the field to change it; sending it goes
    /// back to it.
    fn edit_prompt(&mut self, entry: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Entry::User { text, .. }) = self.entries.get(entry) else { return };
        let text = text.clone();
        self.finish_draft_edit(true, cx);
        self.editing = Some(entry);
        self.prompt.update(cx, |p, cx| {
            p.buffer.select_all();
            p.buffer.insert(&text);
            cx.notify();
        });
        let target = self.focus_target(cx);
        window.focus(&target, cx);
        cx.notify();
    }

    /// Goes back to before one of the user's messages, sending nothing: it
    /// and everything after leave the conversation (the old session is kept
    /// in the list), after asking; the files Claude changed since go back
    /// too, or stay as they are.
    fn revert_to(&mut self, entry: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.entries.get(entry), Some(Entry::User { .. })) {
            return;
        }
        let later = self.entries[entry..].iter().filter(|e| matches!(e, Entry::User { .. })).count();
        let detail = format!(
            "This message{} leave the conversation, and Claude goes on from just before it, with what you send next. \
             The conversation as it is now stays in the sessions list.\n\n\
             The files Claude changed since can go back as they were, or stay as they are now.",
            if later > 1 { format!(" and the {} after it", later - 1) } else { " and Claude's answer to it".to_string() }
        );
        let answer = window.prompt(gpui::PromptLevel::Warning, "Revert to before this message?", Some(&detail), &["Revert, Files Too", "Revert the Conversation Only", "Cancel"], cx);
        let (conn, tab) = (self.conn.clone(), self.tab);
        cx.spawn(async move |this, cx| {
            let files = match answer.await {
                Ok(0) => true,
                Ok(1) => false,
                _ => return,
            };
            conn.send_msg(ClientMsg::AiRevert { tab, entry: entry as u32, files });
            this.update(cx, |this, cx| {
                this.selection = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Sends a prompt again as it was (a new answer); one before the last
    /// asks first, as what came after it goes.
    fn resend(&mut self, entry: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Entry::User { text, .. }) = self.entries.get(entry) else { return };
        let msg = ClientMsg::AiEdit { tab: self.tab, entry: entry as u32, text: text.clone(), images: Vec::new() };
        let last = !self.entries[entry + 1..].iter().any(|e| matches!(e, Entry::User { .. }));
        if last {
            self.conn.send_msg(msg);
            return;
        }
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            "Send this message again?",
            Some("The conversation goes back to it: the later messages go (the old session is kept), and the files Claude changed since are put back."),
            &["Send Again", "Cancel"],
            cx,
        );
        let conn = self.conn.clone();
        cx.spawn(async move |_, _| {
            if let Ok(0) = answer.await {
                conn.send_msg(msg);
            }
        })
        .detach();
    }

    /// The entries of the response entry `i` is in: from after its prompt
    /// to the next one.
    fn response_of(&self, i: usize) -> std::ops::Range<usize> {
        let start = self.entries[..i.min(self.entries.len())].iter().rposition(|e| matches!(e, Entry::User { .. })).map_or(0, |p| p + 1);
        let end = self.entries[i.min(self.entries.len())..].iter().position(|e| matches!(e, Entry::User { .. })).map_or(self.entries.len(), |n| i + n);
        start..end
    }

    /// Everything Claude said in a response, as Markdown.
    fn response_markdown(&self, range: std::ops::Range<usize>) -> String {
        self.entries[range].iter().filter_map(|e| if let Entry::Assistant { text, .. } = e { Some(text.as_str()) } else { None }).collect::<Vec<_>>().join("\n\n")
    }

    fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        if self.editing.take().is_some() {
            self.prompt.update(cx, |p, cx| {
                p.buffer.select_all();
                p.buffer.insert("");
                cx.notify();
            });
            cx.notify();
        }
    }

    /// An entry's selectable text, as drawn.
    /// Markdown as pieces to draw, files it names made links, code coloured.
    fn markdown_pieces(&mut self, text: &str) -> (Vec<Piece>, Vec<Option<CodeColors>>) {
        let mut pieces = ai_text::pieces(shed_ai::markdown::parse(text));
        // Files Claude names become links to them (not in code blocks).
        let mut bases = vec![self.ai.folder.clone()];
        bases.extend(self.ai.add_dirs.iter().cloned());
        bases.extend(self.projects.iter().cloned());
        let known = &mut self.file_exists;
        let mut exists = |p: &std::path::Path| *known.entry(p.to_path_buf()).or_insert_with(|| p.is_file());
        let (lookups, projects) = (&mut self.file_lookups, &self.project_files);
        let mut find = |rel: &std::path::Path| lookups.entry(rel.to_path_buf()).or_insert_with(|| ai_text::find_in_projects(rel, projects)).clone();
        for piece in pieces.iter_mut().filter(|p| !matches!(p.kind, Kind::Code { .. })) {
            ai_text::link_files(&mut piece.text, &bases, &mut exists, &mut find);
        }
        let palette = &self.settings.palette;
        let code = pieces
            .iter()
            .map(|p| match &p.kind {
                Kind::Code { lang, .. } => code_colors(lang, &p.text.text, palette),
                _ => None,
            })
            .collect();
        (pieces, code)
    }

    fn entry_pieces(&mut self, i: usize) -> Rc<Parsed> {
        match self.entries.get(i) {
            Some(Entry::Assistant { text, .. }) => {
                if let Some(p) = self.parsed.get(&i).filter(|p| p.source == *text) {
                    return p.clone();
                }
                let text = text.clone();
                let (pieces, code) = self.markdown_pieces(&text);
                let parsed = Rc::new(Parsed { source: text, pieces, code, groups: Vec::new() });
                self.parsed.insert(i, parsed.clone());
                parsed
            }
            // An opened sub-agent: its prompt, what it said along the way
            // (its tool calls are drawn as lines between), and its report.
            Some(Entry::Tool(t)) if self.expanded.contains(&i) && matches!(t.name.as_str(), "Agent" | "Task") && t.run.is_some() => {
                let run = t.run.as_deref().cloned().unwrap_or_default();
                let report = Some(run.report.clone()).filter(|r| !r.is_empty()).or_else(|| t.result.clone().filter(|_| !run.running())).unwrap_or_default();
                let key = format!("agent:{}:{}:{}:{}", run.skipped, run.steps.len(), report.len(), t.detail.len());
                if let Some(p) = self.parsed.get(&i).filter(|p| p.source == key) {
                    return p.clone();
                }
                let detail = t.detail.clone();
                let mut pieces = Vec::new();
                let mut code = Vec::new();
                if !detail.is_empty() {
                    code.push(None);
                    pieces.push(Piece { kind: Kind::Code { lang: "prompt".into(), depth: 0 }, text: shed_ai::markdown::Text::plain(detail) });
                }
                let mut groups = Vec::new();
                for step in &run.steps {
                    let start = pieces.len();
                    if let Entry::Assistant { text, .. } = step {
                        let (p, c) = self.markdown_pieces(text);
                        pieces.extend(p);
                        code.extend(c);
                    }
                    groups.push(start..pieces.len());
                }
                let start = pieces.len();
                if !report.is_empty() {
                    let (p, c) = self.markdown_pieces(&report);
                    pieces.extend(p);
                    code.extend(c);
                }
                groups.push(start..pieces.len());
                let parsed = Rc::new(Parsed { source: key, pieces, code, groups });
                self.parsed.insert(i, parsed.clone());
                parsed
            }
            Some(Entry::Note(n)) if self.expanded.contains(&i) && !n.detail.is_empty() => {
                if let Some(p) = self.parsed.get(&i).filter(|p| p.source == n.detail) {
                    return p.clone();
                }
                let detail = n.detail.clone();
                let parsed = if matches!(n.kind, NoteKind::Command | NoteKind::Hook) {
                    Parsed { source: detail.clone(), pieces: vec![Piece { kind: Kind::Code { lang: "output".into(), depth: 0 }, text: shed_ai::markdown::Text::plain(detail) }], code: vec![None], groups: Vec::new() }
                } else {
                    let (pieces, code) = self.markdown_pieces(&detail);
                    Parsed { source: detail, pieces, code, groups: Vec::new() }
                };
                let parsed = Rc::new(parsed);
                self.parsed.insert(i, parsed.clone());
                parsed
            }
            Some(Entry::User { text, .. }) => {
                let shown = if self.expanded.contains(&i) { None } else { folded_text(text) };
                Rc::new(Parsed::plain(shown.unwrap_or_else(|| text.clone())))
            }
            Some(Entry::Thinking { text }) if self.expanded.contains(&i) => Rc::new(Parsed::plain(text.clone())),
            // An opened step: its input (the command) and what it returned,
            // as code blocks to select in or copy whole.
            Some(Entry::Tool(t)) if self.expanded.contains(&i) => {
                let mut pieces = Vec::new();
                let mut code = Vec::new();
                if !t.detail.is_empty() {
                    let lang = if t.name == "Bash" { "bash" } else { "input" };
                    code.push(code_colors(lang, &t.detail, &self.settings.palette));
                    pieces.push(Piece { kind: Kind::Code { lang: lang.into(), depth: 0 }, text: shed_ai::markdown::Text::plain(t.detail.clone()) });
                }
                if let Some(result) = &t.result {
                    let lines: Vec<&str> = result.lines().collect();
                    let mut shown = lines[..lines.len().min(RESULT_LINES)].join("\n");
                    if lines.len() > RESULT_LINES {
                        shown.push_str(&format!("\n… {} more lines", lines.len() - RESULT_LINES));
                    }
                    code.push(None);
                    let lang = if t.error { "error" } else { "output" };
                    pieces.push(Piece { kind: Kind::Code { lang: lang.into(), depth: 0 }, text: shed_ai::markdown::Text::plain(shown) });
                }
                Rc::new(Parsed { source: String::new(), pieces, code, groups: Vec::new() })
            }
            _ => Rc::new(Parsed::empty()),
        }
    }

    fn selected_copy(&mut self) -> Option<Copy> {
        let sel = self.selection.filter(|s| !s.is_empty())?;
        let (a, b) = sel.ordered();
        let mut copy = if (a.entry, a.piece) == (b.entry, b.piece) { Copy::within_one() } else { Copy::default() };
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
            .text_size(px(11.0 * self.z()))
            .text_color(hsla(ui.text_dim))
            .hover(|s| s.bg(hsla(ui.tab_hover)).text_color(hsla(ui.text)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                cx.write_to_clipboard(item.clone());
                this.copied(key.clone(), cx);
            }))
            .child(svg().path(icon("copy")).size(px(12.0)).text_color(hsla(ui.text_dim)))
            .child(if done { "Copied" } else { "Copy" })
    }

    /// A piece of text that can be selected with the mouse.
    fn sel_text(&self, i: usize, j: usize, parsed: &Rc<Parsed>, styles: Vec<(Range<usize>, HighlightStyle)>, cx: &mut Context<Self>) -> gpui::Div {
        let text = &parsed.pieces[j].text.text;
        // On characters' edges, whatever the selection says.
        let selected = self
            .selection
            .and_then(|s| s.range_in(i, j, text.len()))
            .map(|r| text.floor_char_boundary(r.start)..text.floor_char_boundary(r.end))
            .filter(|r| r.start < r.end);
        let sel_style = HighlightStyle { background_color: Some(hsla_alpha(self.accent, 0.35)), ..Default::default() };
        // The link under the mouse: a light ground, and the hand.
        let hovered = self.hover_link.as_ref().filter(|(e, p, _)| (*e, *p) == (i, j)).map(|(_, _, r)| r.clone());
        let hover_style = HighlightStyle { background_color: Some(hsla_alpha(self.accent, 0.16)), ..Default::default() };
        let styles = ai_text::with_selection(styles, hovered.clone(), hover_style);
        let styled = StyledText::new(SharedString::from(text.clone())).with_highlights(ai_text::with_selection(styles, selected, sel_style));
        let (down, moved, up) = (styled.layout().clone(), styled.layout().clone(), styled.layout().clone());
        let (p1, p2, p3) = (parsed.clone(), parsed.clone(), parsed.clone());
        // A flex box, not a block: a block lays its text out inline, as
        // part of its own paragraph, and then the text's own layout (which
        // maps the mouse to a character) is never filled in.
        div()
            .flex()
            .flex_col()
            .min_w_0()
            .cursor(if hovered.is_some() { CursorStyle::PointingHand } else { CursorStyle::IBeam })
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                let text = &p1.pieces[j].text.text;
                let Some(at) = text_index(&down, text, e.position) else { return };
                let point = |offset| Point { entry: i, piece: j, offset };
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
                if !this.selecting || e.pressed_button != Some(MouseButton::Left) {
                    // Over a link of this text, or off the one it was over.
                    let over = if over_text(&moved, e.position) {
                        text_index(&moved, &p3.pieces[j].text.text, e.position).and_then(|at| ai_text::link_range_at(&p3.pieces[j].text, at))
                    } else {
                        None
                    };
                    let mine = this.hover_link.as_ref().is_some_and(|(e, p, _)| (*e, *p) == (i, j));
                    match over {
                        Some(r) if this.hover_link.as_ref().is_none_or(|h| *h != (i, j, r.clone())) => {
                            this.hover_link = Some((i, j, r));
                            cx.notify();
                        }
                        None if mine => {
                            this.hover_link = None;
                            cx.notify();
                        }
                        _ => {}
                    }
                    return;
                }
                if !over_text(&moved, e.position) {
                    return;
                }
                let Some(offset) = text_index(&moved, &p3.pieces[j].text.text, e.position) else { return };
                let head = Point { entry: i, piece: j, offset };
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
                    && let Some(at) = text_index(&up, &p2.pieces[j].text.text, e.position)
                    && let Some(url) = ai_text::link_at(&p2.pieces[j].text, at)
                {
                    match ai_text::parse_file_link(url) {
                        // A file: in its project tab, at its line.
                        Some((path, place)) => this.reveal_file(path, place, cx),
                        None => crate::links::open_external(url, cx),
                    }
                }
            }))
            .child(styled)
    }

    /// The pieces of an entry, drawn: Markdown blocks, tables, code.
    fn render_pieces(&mut self, i: usize, parsed: &Rc<Parsed>, ui: &UiColors, mono: &str, cx: &mut Context<Self>) -> gpui::Div {
        self.render_range(i, parsed, 0..parsed.pieces.len(), ui, mono, cx)
    }

    /// Some of an entry's pieces (a sub-agent's step), drawn as `render_pieces` does.
    fn render_range(&mut self, i: usize, parsed: &Rc<Parsed>, range: Range<usize>, ui: &UiColors, mono: &str, cx: &mut Context<Self>) -> gpui::Div {
        let ink = ui.ink(self.accent);
        let mut out = div().flex().flex_col().gap_2().line_height(relative(1.45)).text_color(hsla(ui.text));
        let mut j = range.start;
        let n = range.end.min(parsed.pieces.len());
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
                    while j < n && let Some(Piece { kind: Kind::Item { depth, marker }, text }) = parsed.pieces.get(j) {
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
                        while j < n && let Some(Piece { kind: Kind::Code { depth, .. }, .. }) = parsed.pieces.get(j) {
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
                    let mut table = div().flex().flex_col().rounded_sm().border_1().border_color(hsla(ui.divider)).text_size(px(12.5 * self.z()));
                    let mut row_el: Option<(usize, gpui::Div)> = None;
                    while j < n && let Some(Piece { kind: Kind::Cell { row, align, .. }, text }) = parsed.pieces.get(j) {
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
        let is_error = lang == "error" && matches!(self.entries.get(i), Some(Entry::Tool(_)));
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
                    .text_size(px(11.0 * self.z()))
                    .text_color(hsla(ui.text_dim))
                    .child(if lang.is_empty() { "code".to_string() } else { lang })
                    .child(copy),
            )
            // Its own colour: words the colouring leaves alone (and a tool's
            // output) are not left to the default, black.
            .child(
                div()
                    .px_2()
                    .py_1()
                    .font_family(mono.to_string())
                    .text_size(px(12.0 * self.z()))
                    .line_height(relative(1.35))
                    .text_color(hsla(if is_error { ui.danger } else { ui.text }))
                    .child(self.sel_text(i, j, parsed, styles, cx)),
            )
    }

    fn transcript_markdown(&self) -> String {
        self.entries
            .iter()
            .filter_map(|e| match e {
                Entry::User { text, .. } => Some(format!("**You:**\n\n{text}")),
                Entry::Assistant { text, .. } => Some(text.clone()),
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
                let name = if m.value == "default" { format!("Default · {}", default_model_name(m)) } else { m.name.clone() };
                menu::item(format!("{name}{check}"), move |this: &mut Self, _, _| {
                    let msg = this.settings_msg(|a| a.model = value.clone());
                    this.conn.send_msg(msg);
                })
            })
            .collect();
        if entries.is_empty() {
            entries.push(menu::item("Models show after the first prompt", |_, _, _| {}));
        }
        // The larger context (`[1m]` after the model), while the model does
        // not have it already.
        let large = current.as_deref().is_some_and(|m| m.ends_with(LARGE_CONTEXT));
        if large || self.status.context_max < 1_000_000 {
            entries.push(MenuEntry::Separator);
            let base = current.as_deref().map(|m| m.trim_end_matches(LARGE_CONTEXT).to_string());
            let default_alias = self.models.iter().find(|m| m.value == "default").map(|d| default_model_name(d).split_whitespace().next().unwrap_or("").to_lowercase());
            let standard = base.clone();
            entries.push(menu::item(format!("Context: Standard{}", if large { "" } else { "  ✓" }), move |this: &mut Self, _, _| {
                let msg = this.settings_msg(|a| a.model = standard.clone());
                this.conn.send_msg(msg);
            }));
            if let Some(model) = base.or(default_alias).filter(|m| !m.is_empty()) {
                entries.push(menu::item(format!("Context: 1M tokens{}", if large { "  ✓" } else { "" }), move |this: &mut Self, _, _| {
                    let msg = this.settings_msg(|a| a.model = Some(format!("{model}{LARGE_CONTEXT}")));
                    this.conn.send_msg(msg);
                }));
            }
        }
        let current = current.map(|m| m.trim_end_matches(LARGE_CONTEXT).to_string());
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

    /// The model the tab uses, by name; the default one says which it is.
    fn model_label(&self) -> String {
        match &self.ai.model {
            Some(m) if m.ends_with(LARGE_CONTEXT) => {
                let base = m.trim_end_matches(LARGE_CONTEXT);
                format!("{} · 1M", self.models.iter().find(|x| x.value == base).map(|x| x.name.clone()).unwrap_or_else(|| base.to_string()))
            }
            Some(m) => self.models.iter().find(|x| &x.value == m).map(|x| x.name.clone()).unwrap_or_else(|| m.clone()),
            None => match self.models.iter().find(|x| x.value == "default") {
                Some(d) => format!("{} (default)", default_model_name(d)),
                None => self.status.model.clone().map(|m| format!("{m} (default)")).unwrap_or_else(|| "Default model".into()),
            },
        }
    }

    fn mode_menu(&self) -> Vec<MenuEntry<Self>> {
        let mut entries: Vec<MenuEntry<Self>> = PERMISSION_MODES
            .iter()
            .filter(|(mode, _, _)| *mode != "bypassPermissions")
            .map(|&(mode, label, _)| {
                let check = if self.ai.mode == mode { "  ✓" } else { "" };
                menu::item(format!("{label}{check}"), move |this: &mut Self, _, _| {
                    let msg = this.settings_msg(|a| a.mode = mode.into());
                    this.conn.send_msg(msg);
                })
            })
            .collect();
        // Everything without asking: asks first, in red.
        let check = if self.ai.mode == "bypassPermissions" { "  ✓" } else { "" };
        entries.push(MenuEntry::Separator);
        entries.push(menu::danger(format!("Full Auto…{check}"), |this: &mut Self, window, cx| {
            let answer = window.prompt(
                gpui::PromptLevel::Warning,
                "Let Claude do everything without asking?",
                Some("Full Auto runs every command and changes every file Claude decides to, with no permission prompts, until you switch back. Use it where a mistake costs little (a sandbox, a throwaway branch)."),
                &["Full Auto", "Cancel"],
                cx,
            );
            let msg = this.settings_msg(|a| a.mode = "bypassPermissions".into());
            let conn = this.conn.clone();
            cx.spawn(async move |_, _| {
                if let Ok(0) = answer.await {
                    conn.send_msg(msg);
                }
            })
            .detach();
        }));
        entries
    }

    fn render_entry(&mut self, i: usize, cx: &mut Context<Self>) -> AnyElement {
        #[cfg(test)]
        RENDERED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let ui = current_ui(cx);
        let accent = self.accent;
        let mono = self.settings.font.family.clone();
        let Some(entry) = self.entries.get(i).cloned() else { return div().into_any_element() };
        let open = self.expanded.contains(&i);
        let toggle = cx.listener(move |this, _, _, cx| {
            if !this.expanded.remove(&i) {
                this.expanded.insert(i);
            }
            let row = this.row(i);
            this.list.remeasure_items(row..row + 1);
            cx.notify();
        });
        let body = match entry {
            Entry::User { text, images, at, pictures } => {
                let lines: Vec<&str> = text.lines().collect();
                let group: SharedString = format!("prompt-{i}").into();
                let idle = self.status.state == AiState::Idle;
                let action = |id: &str, glyph: &'static str, label: &'static str| {
                    div()
                        .id(SharedString::from(format!("{id}-{i}")))
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_1p5()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_size(px(11.0 * self.z()))
                        .text_color(hsla(ui.text_dim))
                        .hover(|s| s.bg(hsla(ui.tab_hover)).text_color(hsla(ui.text)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(svg().path(icon(glyph)).size(px(12.0)).text_color(hsla(ui.text_dim)))
                        .child(label)
                };
                let actions = div()
                    .flex()
                    .flex_1()
                    .justify_end()
                    .gap_1()
                    .invisible()
                    .group_hover(group.clone(), |s| s.visible())
                    .when(idle, |d| {
                        d.child(action("edit", "pencil", "Edit").on_click(cx.listener(move |this, _, window, cx| this.edit_prompt(i, window, cx))))
                            .child(action("resend", "refresh", "Send Again").on_click(cx.listener(move |this, _, window, cx| this.resend(i, window, cx))))
                            .child(
                                action("revert", "discard", "Revert…")
                                    .text_color(hsla(ui.danger))
                                    .tooltip(menu::tooltip("Go back to before this message: it and what follows leave the conversation"))
                                    .on_click(cx.listener(move |this, _, window, cx| this.revert_to(i, window, cx))),
                            )
                    })
                    .child(self.copy_button(format!("copy-prompt-{i}").into(), ClipboardItem::new_string(text.clone()), &ui, cx));
                let editing = self.editing == Some(i);
                div().group(group).flex().flex_col().child(div()
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
                    // The pictures sent with it, small; a click opens one whole.
                    .when(!pictures.is_empty(), |d| d.child(thumbnails(i, &pictures, self.remote_shown(&pictures, cx).as_deref(), 72.0, &ui, self.z())))
                    .when(pictures.is_empty() && images > 0, |d| d.child(div().mt_1().text_size(px(11.0 * self.z())).text_color(hsla(ui.text_dim)).child(format!("{images} picture(s) attached"))))
                    .when(folded_text(&text).is_some(), |d| {
                        let label = if open { "Show less".to_string() } else if lines.len() > FOLD_LINES { format!("Show all {} lines", lines.len()) } else { "Show all".to_string() };
                        d.child(div().id(("fold", i)).mt_1().cursor_pointer().text_size(px(11.0 * self.z())).text_color(hsla(ui.ink(accent))).on_click(toggle).child(label))
                    })
                    .when(editing, |d| d.border_l_4()))
                    .child(div().flex().items_center().gap_2().children(stamp(at, None, &ui, self.z())).child(actions))
                    .into_any_element()
            }
            Entry::Assistant { text, at } => {
                let parsed = self.entry_pieces(i);
                // When the response began: over its first part.
                let first = !self.entries[..i].iter().rev().take_while(|e| !matches!(e, Entry::User { .. })).any(|e| matches!(e, Entry::Assistant { .. }));
                let when = if first { stamp(at, Some("Claude"), &ui, self.z()) } else { None };
                // The last part of a response copies all of it.
                let response = self.response_of(i);
                let last = !self.entries[i + 1..response.end].iter().any(|e| matches!(e, Entry::Assistant { .. }));
                let parts = self.entries[response.clone()].iter().filter(|e| matches!(e, Entry::Assistant { .. })).count();
                let copy = if last && parts > 1 {
                    let all = self.response_markdown(response);
                    self.copy_button(format!("copy-entry-{i}").into(), ai_text::formatted_item(all.clone(), &all), &ui, cx).child("whole response")
                } else {
                    self.copy_button(format!("copy-entry-{i}").into(), ai_text::formatted_item(text.clone(), &text), &ui, cx)
                };
                let group: SharedString = format!("answer-{i}").into();
                div()
                    .group(group.clone())
                    .flex()
                    .flex_col()
                    .children(when.map(|w| w.mb_1()))
                    .child(self.render_pieces(i, &parsed, &ui, &mono, cx))
                    .child(div().flex().justify_end().invisible().group_hover(group, |s| s.visible()).child(copy))
                    .into_any_element()
            }
            Entry::Thinking { text } => div()
                .id(("thinking", i))
                .cursor_pointer()
                .text_size(px(12.0 * self.z()))
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
                // The whole command (or input) to copy, shown on hover.
                let copy_key: SharedString = format!("copy-tool-{i}").into();
                let copied = self.copied.as_ref().is_some_and(|(k, _)| *k == copy_key);
                let copy = (!t.detail.is_empty()).then(|| {
                    self.copy_button(copy_key, ClipboardItem::new_string(t.detail.clone()), &ui, cx)
                        .flex_none()
                        .when(!copied, |d| d.invisible().group_hover("tool-head", |s| s.visible()))
                        .tooltip(menu::tooltip(if t.name == "Bash" { "Copy the command" } else { "Copy the input" }))
                });
                let open_file = t.file.clone().filter(|f| *self.file_exists.entry(PathBuf::from(f)).or_insert_with(|| std::path::Path::new(f).is_file()));
                let agent = matches!(t.name.as_str(), "Agent" | "Task");
                let run = t.run.as_deref();
                let (mark, color) = match (run.map(|r| r.state), &t.result, t.error) {
                    // Going on (a sub-agent, a command in the background): turning.
                    (Some(RunState::Running), _, _) => (self.turning(), ui.ink(accent)),
                    (Some(RunState::Done), _, _) => ("✓", ui.ink(accent)),
                    (Some(RunState::Failed), _, _) => ("✗", ui.danger),
                    (Some(RunState::Stopped), _, _) => ("■", ui.text_dim),
                    (None, None, _) => ("○", ui.text_dim),
                    (None, Some(_), false) => ("✓", ui.ink(accent)),
                    (None, Some(_), true) => ("✗", ui.danger),
                };
                // After its line: how a run is doing; when a timer it set fires.
                let info = match run {
                    Some(r) => Some(run_line(&t, r)),
                    None => self.status.timers.iter().find(|x| x.tool == t.id).map(|x| format!("fires {}", timer_when(x))),
                };
                let background = run.is_some_and(|r| r.background);
                // A sub-agent stands out: its own line in the accent colour.
                let mut el = div()
                    .id(("tool", i))
                    .flex()
                    .flex_col()
                    .text_size(px(12.0 * self.z()))
                    .when(agent, |d| d.pl_1p5().border_l_2().border_color(hsla_alpha(accent, 0.7)))
                    .child(
                    div()
                        .id(("tool-head", i))
                        .group("tool-head")
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .rounded_sm()
                        .hover(|s| s.bg(hsla(ui.tab_hover)))
                        .on_click(toggle)
                        .child(div().flex_none().w(px(10.0)).text_color(hsla(ui.text_dim)).child(if open { "▾" } else { "▸" }))
                        .child(div().flex_none().w(px(12.0)).text_color(hsla(color)).child(mark))
                        .when(agent, |d| d.child(svg().path(icon("agent")).flex_none().size(px(13.0 * self.z())).text_color(hsla(ui.ink(accent)))))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .font_family(mono.clone())
                                .text_color(hsla(if agent { ui.text } else { ui.text_dim }))
                                .when(info.is_some(), |d| d.flex_shrink(1.0).max_w(relative(0.6)))
                                .when(info.is_none(), |d| d.flex_1())
                                .child(t.summary.clone()),
                        )
                        .when(background, |d| {
                            d.child(div().flex_none().px_1().rounded_sm().border_1().border_color(hsla(ui.divider)).text_size(px(10.5 * self.z())).text_color(hsla(ui.text_dim)).child("background"))
                        })
                        .when_some(info, |d, info| {
                            let live = run.is_some_and(|r| r.running());
                            d.child(div().flex_1().min_w_0().truncate().text_color(hsla(if live { ui.ink(accent) } else { ui.text_dim })).child(info))
                        })
                        .children(copy)
                        // The file it read or changed, in its project tab.
                        .when_some(open_file, |d, file| {
                            d.child(
                                div()
                                    .id(("tool-open", i))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .px_1p5()
                                    .rounded_sm()
                                    .cursor_pointer()
                                    .text_color(hsla(ui.ink(accent)))
                                    .hover(|s| s.bg(hsla_alpha(accent, 0.15)))
                                    .tooltip(menu::tooltip_text(file.clone()))
                                    .child(svg().path(icon("file")).size(px(11.0 * self.z())).text_color(hsla(ui.ink(accent))))
                                    .child("Open")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.reveal_file(PathBuf::from(&file), None, cx);
                                    })),
                            )
                        }),
                );
                // A screenshot it read, to see without opening it.
                if !t.pictures.is_empty() {
                    let fetched = self.remote_shown(&t.pictures, cx);
                    el = el.child(div().pl(px(20.0)).child(thumbnails(i, &t.pictures, fetched.as_deref(), 120.0, &ui, self.z())));
                }
                if open && agent && let Some(run) = t.run.as_deref() {
                    el = el.child(self.render_agent(i, &t, run, &ui, &mono, cx));
                }
                if open && !(agent && run.is_some()) {
                    let parsed = self.entry_pieces(i);
                    let mut j = 0;
                    if !t.detail.is_empty() {
                        el = el.child(div().my_1().child(self.render_code(i, j, &parsed, &ui, &mono, cx)));
                        j += 1;
                    }
                    if let Some(diff) = &t.diff {
                        el = el.child(diff_view(&diff.old, &diff.new, &ui, &mono, self.z()));
                    }
                    if j < parsed.pieces.len() {
                        el = el.child(div().my_1().child(self.render_code(i, j, &parsed, &ui, &mono, cx)));
                    }
                    // Closing it from its end, without going back up to its line.
                    el = el.child(div().flex().child(self.collapse_button(i, &ui, cx)));
                }
                el.into_any_element()
            }
            Entry::Permission(p) if !p.questions.is_empty() => self.render_questions(i, &p, &ui, cx),
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
                    card = card.child(diff_view(&diff.old, &diff.new, &ui, &mono, self.z()));
                } else if !p.detail.is_empty() {
                    card = card.child(pre(&p.detail, &ui, &mono, None, self.z()));
                }
                card = match p.answer {
                    None => card.child(
                        div()
                            .flex()
                            .gap_2()
                            .text_size(px(12.0 * self.z()))
                            .child(answer("Allow", true, false, accent))
                            .child(answer("Allow for This Session", true, true, accent))
                            .child(answer("Deny", false, false, ui.danger)),
                    ),
                    Some(allowed) => card.child(div().text_size(px(12.0 * self.z())).text_color(hsla(ui.text_dim)).child(if allowed { "Allowed" } else { "Denied" })),
                };
                card.into_any_element()
            }
            Entry::Notice { text, error } => div()
                .text_size(px(11.0 * self.z()))
                .text_color(hsla(if error { ui.danger } else { ui.text_dim }))
                .child(text)
                .into_any_element(),
            // Claude Code's own note: a line of its own kind, its detail folded.
            Entry::Note(n) => {
                let foldable = !n.detail.is_empty();
                let (glyph, color) = match n.kind {
                    _ if n.error => ("alert", ui.danger),
                    NoteKind::Task => ("tool", ui.text_dim),
                    NoteKind::Wakeup => ("clock", ui.ink(accent)),
                    NoteKind::Command => ("terminal", ui.text_dim),
                    NoteKind::Compacted => ("history", ui.text_dim),
                    NoteKind::Message => ("agent", ui.ink(accent)),
                    NoteKind::Hook => ("settings", ui.text_dim),
                };
                let line = div()
                    .id(("note", i))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded_sm()
                    .text_size(px(12.0 * self.z()))
                    .text_color(hsla(if n.error { ui.danger } else { ui.text_dim }))
                    .when(foldable, |d| d.cursor_pointer().hover(|s| s.bg(hsla(ui.tab_hover))).on_click(toggle))
                    .when(foldable, |d| d.child(div().flex_none().w(px(10.0)).child(if open { "▾" } else { "▸" })))
                    .child(svg().path(icon(glyph)).flex_none().size(px(12.0 * self.z())).text_color(hsla(color)))
                    .child(div().flex_1().min_w_0().truncate().child(n.text.clone()))
                    .children((n.at > 0).then(|| div().flex_none().text_size(px(11.0 * self.z())).text_color(hsla_alpha(ui.text_dim, 0.85)).child(local_time(n.at))));
                let report = n.kind == NoteKind::Message;
                let mut el = div().flex().flex_col().when(report, |d| d.pl_1p5().border_l_2().border_color(hsla_alpha(accent, 0.7))).child(line);
                if open && foldable {
                    let parsed = self.entry_pieces(i);
                    el = el.child(div().pl(px(20.0)).my_1().text_size(px(12.5 * self.z())).child(self.render_pieces(i, &parsed, &ui, &mono, cx)));
                    el = el.child(div().flex().pl(px(16.0)).text_size(px(12.0 * self.z())).child(self.collapse_button(i, &ui, cx)));
                }
                el.into_any_element()
            }
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

    /// An opened sub-agent: its prompt, its steps (tool calls as lines that
    /// open to what they returned, what it said in between), its report.
    fn render_agent(&mut self, i: usize, t: &shed_ai::Tool, run: &Run, ui: &UiColors, mono: &str, cx: &mut Context<Self>) -> gpui::Div {
        let z = self.z();
        let accent = self.accent;
        let parsed = self.entry_pieces(i);
        let mut el = div().flex().flex_col();
        // Its prompt, folded: often long, and not what one opens it for.
        if !t.detail.is_empty() {
            let key = (i, PROMPT_STEP.to_string());
            let shown = self.open_steps.contains(&key);
            let lines = t.detail.lines().count();
            el = el.child(
                div()
                    .id(("agent-prompt", i))
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_color(hsla(ui.text_dim))
                    .hover(|s| s.bg(hsla(ui.tab_hover)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.open_steps.remove(&key) {
                            this.open_steps.insert(key.clone());
                        }
                        if i >= this.first {
                            let row = this.row(i);
                            this.list.remeasure_items(row..row + 1);
                        }
                        cx.notify();
                    }))
                    .child(div().flex_none().w(px(10.0)).child(if shown { "▾" } else { "▸" }))
                    .child(format!("Its prompt ({lines} line{})", if lines == 1 { "" } else { "s" })),
            );
            if shown {
                el = el.child(div().my_1().child(self.render_code(i, 0, &parsed, ui, mono, cx)));
            }
        }
        let what = [Some(run.agent.clone()).filter(|a| !a.is_empty()), (run.tokens > 0).then(|| format!("{} tokens", tokens(run.tokens)))].into_iter().flatten().collect::<Vec<_>>();
        if !what.is_empty() {
            el = el.child(div().text_color(hsla(ui.text_dim)).child(what.join(" · ")));
        }
        let mut steps = div().flex().flex_col().gap_1().my_1().pl_3().border_l_2().border_color(hsla(ui.divider));
        if run.skipped > 0 {
            steps = steps.child(div().text_color(hsla(ui.text_dim)).child(format!("{} earlier steps not shown", run.skipped)));
        }
        if run.steps.is_empty() {
            let none = if run.running() { "No steps yet" } else { "Its steps are not known here" };
            steps = steps.child(div().text_color(hsla(ui.text_dim)).child(none));
        }
        for (k, step) in run.steps.iter().enumerate() {
            match step {
                Entry::Tool(s) => {
                    let key = (i, s.id.clone());
                    let step_open = self.open_steps.contains(&key);
                    let (mark, color) = match (&s.result, s.error) {
                        (None, _) if run.running() => (self.turning(), ui.ink(accent)),
                        (None, _) => ("○", ui.text_dim),
                        (Some(_), false) => ("✓", ui.ink(accent)),
                        (Some(_), true) => ("✗", ui.danger),
                    };
                    let has_more = s.result.is_some() || s.diff.is_some() || !s.detail.is_empty();
                    steps = steps.child(
                        div()
                            .id(SharedString::from(format!("step-{i}-{k}")))
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded_sm()
                            .when(has_more, |d| d.cursor_pointer().hover(|s| s.bg(hsla(ui.tab_hover))))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.open_steps.remove(&key) {
                                    this.open_steps.insert(key.clone());
                                }
                                if i >= this.first {
                                    let row = this.row(i);
                                    this.list.remeasure_items(row..row + 1);
                                }
                                cx.notify();
                            }))
                            .child(div().flex_none().w(px(10.0)).text_color(hsla(ui.text_dim)).child(if !has_more { "" } else if step_open { "▾" } else { "▸" }))
                            .child(div().flex_none().w(px(12.0)).text_color(hsla(color)).child(mark))
                            .child(div().flex_1().min_w_0().truncate().font_family(mono.to_string()).text_color(hsla(ui.text_dim)).child(s.summary.clone())),
                    );
                    if step_open {
                        let mut more = div().pl(px(20.0)).flex().flex_col();
                        if let Some(diff) = &s.diff {
                            more = more.child(diff_view(&diff.old, &diff.new, ui, mono, z));
                        } else if !s.detail.is_empty() {
                            more = more.child(pre(&s.detail, ui, mono, None, z));
                        }
                        if let Some(result) = &s.result {
                            more = more.child(pre(result, ui, mono, s.error.then_some(ui.danger), z));
                        }
                        steps = steps.child(more);
                    }
                }
                Entry::Assistant { .. } => {
                    let range = parsed.groups.get(k).cloned().unwrap_or_default();
                    if !range.is_empty() {
                        steps = steps.child(div().text_size(px(12.5 * z)).child(self.render_range(i, &parsed, range, ui, mono, cx)));
                    }
                }
                _ => {}
            }
        }
        el = el.child(steps);
        if let Some(range) = parsed.groups.last().filter(|r| !r.is_empty()).cloned() {
            let report = self.render_range(i, &parsed, range, ui, mono, cx);
            el = el.child(div().mt_1().text_color(hsla(ui.text_dim)).child("Its report")).child(div().my_1().child(report));
        }
        el.child(div().flex().child(self.collapse_button(i, ui, cx)))
    }

    /// "▴ Collapse" at the end of an opened entry, to close it from there.
    fn collapse_button(&self, i: usize, ui: &UiColors, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        div()
            .id(("tool-collapse", i))
            .px_1p5()
            .rounded_sm()
            .cursor_pointer()
            .text_color(hsla(ui.text_dim))
            .hover(|s| s.bg(hsla(ui.tab_hover)).text_color(hsla(ui.text)))
            .on_click(cx.listener(move |this, _, _, cx| this.collapse(i, cx)))
            .child("▴ Collapse")
    }

    /// Opens sub-agent `i` and brings it into view.
    fn show_agent(&mut self, i: usize, cx: &mut Context<Self>) {
        self.expanded.insert(i);
        self.reveal(i);
        let row = self.row(i);
        self.list.remeasure_items(row..row + 1);
        self.list.scroll_to(gpui::ListOffset { item_ix: row, offset_in_item: px(0.0) });
        cx.notify();
    }

    /// The strip above the prompt: sub-agents running, and those that just
    /// ended, each a line (what it is, how it does, its last step); a click
    /// opens it in the conversation.
    fn render_agents(&self, ui: &UiColors, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let now = unix_now();
        let shown: Vec<(usize, &shed_ai::Tool, &Run)> =
            self.agents.iter().filter_map(|&i| self.entries.get(i).and_then(agent_run).map(|(t, r)| (i, t, r))).filter(|(_, _, r)| r.running() || r.ended + AGENT_KEPT > now).collect();
        if shown.is_empty() {
            return None;
        }
        let accent = self.accent;
        let z = self.z();
        let mut out = div().flex().flex_col().gap_0p5();
        for (i, t, run) in shown {
            let (mark, color) = match run.state {
                RunState::Running => (self.turning(), ui.ink(accent)),
                RunState::Done => ("✓", ui.ink(accent)),
                RunState::Failed => ("✗", ui.danger),
                RunState::Stopped => ("■", ui.text_dim),
            };
            let name = t.summary.strip_prefix("Agent: ").unwrap_or(&t.summary).to_string();
            let tools = (run.tools > 0).then(|| format!("{} tool call{}", run.tools, if run.tools == 1 { "" } else { "s" }));
            let last = Some(run.last.clone()).filter(|l| run.running() && !l.is_empty() && l != "Starting");
            out = out.child(
                div()
                    .id(("agent-chip", i))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .border_1()
                    .border_color(hsla_alpha(accent, if run.running() { 0.5 } else { 0.25 }))
                    .cursor_pointer()
                    .hover(|s| s.bg(hsla_alpha(accent, 0.1)))
                    .tooltip(menu::tooltip("Show this agent in the conversation, opened: its prompt, steps and report"))
                    .on_click(cx.listener(move |this, _, _, cx| this.show_agent(i, cx)))
                    .child(div().flex_none().w(px(12.0)).text_color(hsla(color)).child(mark))
                    .child(svg().path(icon("agent")).flex_none().size(px(12.0 * z)).text_color(hsla(ui.ink(accent))))
                    .child(div().flex_none().max_w(relative(0.4)).truncate().text_color(hsla(ui.text)).child(name))
                    .child(div().flex_none().text_color(hsla(if run.running() { ui.ink(accent) } else { ui.text_dim })).child(run_time(run)))
                    .children(tools.map(|n| div().flex_none().text_color(hsla(ui.text_dim)).child(n)))
                    .child(div().flex_1().min_w_0().truncate().font_family(self.settings.font.family.clone()).text_color(hsla(ui.text_dim)).children(last)),
            );
        }
        Some(out)
    }

    fn render_sessions(&mut self, range: std::ops::Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let ui = current_ui(cx);
        let accent = self.accent;
        let shown = self.shown_sessions(cx);
        let z = self.z();
        range
            .filter_map(|n| {
                let row = shown.get(n)?;
                let (s, indent, toggle) = match row {
                    SessionRow::Conversation { tip, root, parts, open } => (tip, 8.0, (*parts > 1).then(|| (root.clone(), *parts, *open))),
                    SessionRow::Part(s) => (s, 22.0 + 14.0 * s.depth.min(4) as f32, None),
                    SessionRow::Draft(d) => return Some(self.render_draft_session(n, d, &ui, cx)),
                };
                let current = match row {
                    // The conversation's row stands for every file of it.
                    SessionRow::Conversation { root, .. } => self.ai.session.as_ref().is_some_and(|id| self.family_of(id) == Some(root.as_str())),
                    SessionRow::Part(s) => self.ai.session.as_deref() == Some(&s.id),
                    SessionRow::Draft(_) => false,
                };
                let id = s.id.clone();
                let menu_id = s.id.clone();
                let prompts = if matches!(row, SessionRow::Conversation { .. }) { s.whole.max(s.prompts) } else { s.prompts };
                // A conversation's row deletes all of it: its first file, and those going on from it.
                let delete = ToDelete {
                    id: match row {
                        SessionRow::Conversation { root, .. } => root.clone(),
                        _ => s.id.clone(),
                    },
                    title: s.title.clone(),
                    modified: s.modified,
                    prompts,
                };
                let mut meta = vec![ago(s.modified)];
                match row {
                    SessionRow::Part(p) if p.parent.is_some() => meta.insert(0, if p.branched { format!("↳ branched after #{}", p.from) } else { "↳ continued".to_string() }),
                    SessionRow::Part(_) => meta.insert(0, "first".to_string()),
                    SessionRow::Conversation { .. } | SessionRow::Draft(_) => {}
                }
                if let Some(b) = &s.branch {
                    meta.push(b.clone());
                }
                meta.push(format!("{prompts} prompt{}", if prompts == 1 { "" } else { "s" }));
                if let Some((_, parts, _)) = &toggle {
                    meta.push(format!("{parts} parts"));
                }
                // Claude is at it, here or in the background (a
                // conversation's row stands for all its files).
                let away = self
                    .ai
                    .running()
                    .find(|(id, _)| match row {
                        SessionRow::Conversation { root, .. } => *id == s.id || self.family_of(id) == Some(root.as_str()),
                        _ => *id == s.id,
                    })
                    .map(|(_, a)| a);
                match away {
                    Some(shed_model::AiActivity::Waiting) => meta.insert(0, "waiting for you".to_string()),
                    Some(_) => meta.insert(0, "working".to_string()),
                    None => {}
                }
                let drafts = self.ai.drafts.iter().filter(|d| d.session == s.id).count();
                if drafts > 0 {
                    meta.push(format!("{drafts} draft{}", if drafts == 1 { "" } else { "s" }));
                }
                Some(
                    div()
                        .id(("session", n))
                        .h(px(44.0))
                        .pl(px(indent))
                        .pr_2()
                        .flex()
                        .items_center()
                        .gap_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .when(current, |d| d.bg(hsla_alpha(accent, 0.2)))
                        .hover(|st| st.bg(hsla(ui.tab_hover)))
                        // Its whole title, and more, on hover.
                        .tooltip(menu::tooltip_text(format!("{}\n{}", s.title, meta.join(" · "))))
                        // A double click renames it.
                        .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                            if e.click_count() >= 2 {
                                this.start_rename(id.clone(), window, cx);
                            } else {
                                this.open_session(Some(id.clone()));
                            }
                        }))
                        .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                            let entries = this.session_menu(menu_id.clone(), delete.clone());
                            this.open_menu(e.position, entries, cx);
                        }))
                        // Working in the background, or waiting for you there.
                        .when_some(away, |d, a| {
                            let glyph = if a == shed_model::AiActivity::Waiting { "tool-waiting" } else { "ai" };
                            d.child(svg().flex_none().path(icon(glyph)).size(px(13.0 * z)).text_color(hsla(ui.ink(accent))))
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(match self.renaming.as_ref().filter(|(r, _, _)| *r == s.id) {
                                    Some((_, editor, _)) => div().child(editor.clone()).into_any_element(),
                                    None => div().truncate().text_color(hsla(ui.text)).child(s.title.clone()).into_any_element(),
                                })
                                .child(div().truncate().text_size(px(11.0 * z)).text_color(hsla(ui.text_dim)).child(meta.join(" · "))),
                        )
                        // Its files, for who wants to see how it was split.
                        .when_some(toggle, |d, (root, _, open)| {
                            d.child(
                                div()
                                    .id(("session-parts", n))
                                    .flex_none()
                                    .px_1()
                                    .rounded_sm()
                                    .cursor_pointer()
                                    .text_color(hsla(ui.text_dim))
                                    .hover(|s| s.bg(hsla(ui.divider)).text_color(hsla(ui.text)))
                                    .tooltip(menu::tooltip("The session files it is made of"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        if !this.open_families.remove(&root) {
                                            this.open_families.insert(root.clone());
                                        }
                                        cx.notify();
                                    }))
                                    .child(if open { "▾" } else { "▸" }),
                            )
                        })
                        .into_any_element(),
                )
            })
            .collect()
    }

    /// The view that draws entry `i`, made the first time it is drawn.
    fn entry_view(&mut self, i: usize, cx: &mut Context<Self>) -> Entity<EntryView> {
        if self.entry_views.len() > self.entries.len() {
            let n = self.entries.len();
            self.entry_views.retain(|k, _| *k < n);
        }
        let ai = cx.entity();
        self.entry_views
            .entry(i)
            .or_insert_with(|| {
                cx.new(|cx| EntryView {
                    ai: ai.downgrade(),
                    entry: i,
                    // Whatever changes in the conversation draws it again.
                    _follow: cx.observe(&ai, |_, _, cx| cx.notify()),
                })
            })
            .clone()
    }

    /// The first file of the conversation a session file belongs to.
    fn family_of(&self, id: &str) -> Option<&str> {
        shed_ai::family_of(&self.sessions, id)
    }

    /// This session's prompts the filter lets through: (entry, number, text).
    fn shown_prompts(&self, cx: &gpui::App) -> Vec<(usize, usize, String, u64)> {
        let words: Vec<String> = self.filter.read(cx).text().split_whitespace().map(str::to_lowercase).collect();
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| if let Entry::User { text, at, .. } = e { Some((i, text.clone(), *at)) } else { None })
            .enumerate()
            .map(|(n, (i, text, at))| (i, n + 1, text, at))
            .filter(|(_, _, text, _)| {
                let lower = text.to_lowercase();
                words.iter().all(|w| lower.contains(w))
            })
            .collect()
    }

    /// One row per prompt: its number, first line and time; a click scrolls
    /// the conversation to it.
    fn render_prompt_rows(&mut self, range: std::ops::Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let ui = current_ui(cx);
        let shown = self.shown_prompts(cx);
        let z = self.z();
        let top = self.top_entry().0;
        range
            .filter_map(|n| {
                let (entry, number, text, at) = shown.get(n)?.clone();
                // The prompt whose turn is at the top of the conversation.
                let next = self.entries[entry + 1..].iter().position(|e| matches!(e, Entry::User { .. })).map_or(self.entries.len(), |k| entry + 1 + k);
                let here = (entry..next).contains(&top);
                let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
                let when = if at > 0 { local_time(at) } else { String::new() };
                Some(
                    div()
                        .id(("prompt-row", n))
                        .h(px(40.0 * z))
                        .px_2()
                        .flex()
                        .flex_col()
                        .justify_center()
                        .rounded_sm()
                        .cursor_pointer()
                        .when(here, |d| d.bg(hsla_alpha(self.accent, 0.16)))
                        .hover(|s| s.bg(hsla(ui.tab_hover)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.reveal(entry);
                            this.list.scroll_to(gpui::ListOffset { item_ix: this.row(entry), offset_in_item: px(0.0) });
                            cx.notify();
                        }))
                        .child(div().truncate().text_color(hsla(ui.text)).child(first))
                        .child(div().truncate().text_size(px(11.0 * z)).text_color(hsla(ui.text_dim)).child(if when.is_empty() { format!("#{number}") } else { format!("#{number} · {when}") }))
                        .into_any_element(),
                )
            })
            .collect()
    }

    /// The sessions list as drawn: a row per conversation (the session
    /// files that go on from one another), and, opened, a row per file.
    /// A draft session's row: its title from its first draft, and how many.
    fn render_draft_session(&self, n: usize, d: &shed_model::DraftSession, ui: &UiColors, cx: &mut Context<Self>) -> AnyElement {
        let current = self.ai.draft_session.as_deref() == Some(d.key.as_str());
        let meta = format!("Draft · {} message{} kept, nothing sent", d.drafts, if d.drafts == 1 { "" } else { "s" });
        let (open, delete) = (d.key.clone(), d.key.clone());
        div()
            .id(("session", n))
            .h(px(44.0))
            .pl(px(8.0))
            .pr_2()
            .flex()
            .items_center()
            .gap_1p5()
            .rounded_sm()
            .cursor_pointer()
            .when(current, |e| e.bg(hsla_alpha(self.accent, 0.2)))
            .hover(|st| st.bg(hsla(ui.tab_hover)))
            .tooltip(menu::tooltip_text(format!("{}\n{meta}", d.title)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_session(Some(open.clone()));
                this.drafts_open = true;
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                let key = delete.clone();
                let entries = vec![menu::danger("Delete Draft Session…", move |this: &mut Self, window, cx| this.delete_draft_session(key.clone(), window, cx)).icon("trash")];
                this.open_menu(e.position, entries, cx);
            }))
            .child(svg().path(icon("pencil")).size(px(12.0)).flex_none().text_color(hsla(ui.text_dim)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().text_color(hsla(ui.text)).child(d.title.clone()))
                    .child(div().truncate().text_size(px(11.0 * self.z())).text_color(hsla(ui.text_dim)).child(meta)),
            )
            .into_any_element()
    }

    /// Deletes a draft session's drafts (after asking); shown, the tab goes
    /// to a new session.
    fn delete_draft_session(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.ai.drafts.iter().filter(|d| d.session == key).count();
        let detail = format!("Its {n} draft{} go too.", if n == 1 { "" } else { "s" });
        let answer = window.prompt(gpui::PromptLevel::Warning, "Delete this draft session?", Some(&detail), &["Delete", "Cancel"], cx);
        cx.spawn(async move |this, cx| {
            if matches!(answer.await, Ok(0)) {
                this.update(cx, |this, cx| {
                    if this.ai.draft_session.as_deref() == Some(key.as_str()) {
                        this.open_session(None);
                        this.ai.draft_session = None;
                    }
                    let gone = this.ai.drafts.iter().enumerate().filter(|(_, d)| d.session == key).map(|(i, _)| i).collect();
                    this.remove_drafts(gone);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn shown_sessions(&self, cx: &gpui::App) -> Vec<SessionRow> {
        shed_ai::session_rows(&self.sessions, self.ai.draft_sessions(), &self.open_families, self.filter.read(cx).text())
    }

    fn render_header(&self, ui: &UiColors, cx: &mut Context<Self>) -> impl IntoElement {
        let accent = self.accent;
        let folder = self.ai.folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let title = self
            .ai
            .session
            .as_ref()
            .and_then(|id| self.sessions.iter().find(|s| &s.id == id))
            .map(|s| s.title.clone())
            .or_else(|| {
                let key = self.ai.draft_session.as_deref()?;
                self.ai.draft_sessions().into_iter().find(|d| d.key == key).map(|d| format!("{} (draft)", d.title))
            })
            .unwrap_or_else(|| if self.ai.session.is_some() { "Session".into() } else { "New session".into() });
        let mut chips = div().flex().flex_wrap().items_center().gap_1().text_size(px(11.0 * self.z()));
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
                    .child(match (self.ai.session.clone(), self.renaming.as_ref().filter(|(r, _, _)| Some(r) == self.ai.session.as_ref())) {
                        (_, Some((_, editor, _))) => div().w(px(360.0)).text_size(px(14.0 * self.z())).child(editor.clone()).into_any_element(),
                        (session, None) => {
                            let full = title.clone();
                            div()
                                .id("session-title")
                                .flex_shrink(1.0)
                                .min_w_0()
                                .truncate()
                                .text_size(px(14.0 * self.z()))
                                .text_color(hsla(ui.text))
                                .tooltip(menu::tooltip_text(format!("{full}\n(double-click to rename)")))
                                .when_some(session, |d, id| {
                                    d.cursor_pointer().on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                                        if e.click_count() >= 2 {
                                            this.start_rename(id.clone(), window, cx);
                                        }
                                    }))
                                })
                                .child(title)
                                .into_any_element()
                        }
                    })
                    .child(div().flex_1().min_w_0().truncate().text_size(px(12.0 * self.z())).text_color(hsla(ui.text_dim)).child(format!("{folder} · {}", shed_save::home_relative(&self.ai.folder))))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .gap_3()
                            .text_size(px(11.0 * self.z()))
                            .items_center()
                            .when(self.status.cost > 0.0, |d| d.child(div().text_color(hsla(ui.text_dim)).child(format!("${:.2}", self.status.cost))))
                            .when(self.status.context > 0, |d| {
                                let used = tokens(self.status.context);
                                let text = if self.status.context_max > 0 { format!("{used} / {} context", tokens(self.status.context_max)) } else { format!("{used} context") };
                                d.child(div().text_color(hsla(ui.text_dim)).child(text))
                            }),
                    ),
            )
            .child(chips)
    }

    fn render_footer(&self, ui: &UiColors, cx: &mut Context<Self>) -> impl IntoElement {
        let accent = self.accent;
        let working = self.status.state != AiState::Idle;
        let status = match self.status.state {
            AiState::Idle => String::new(),
            AiState::Working => format!("{} · {}", self.activity(), self.status.since.map(secs_since).unwrap_or_default()),
            AiState::Waiting => "Waiting for your answer".into(),
        };
        // Three dots taking turns while Claude works (stepped by the
        // ticker, a few times a second: a frame redraws the whole window).
        let loader = (self.status.state == AiState::Working).then(|| {
            div().flex().flex_none().items_center().gap_1().children((0..3).map(|k| {
                let on = self.spin % 3 == k;
                div().size(px(6.0 * self.z())).rounded_full().bg(hsla_alpha(accent, if on { 1.0 } else { 0.3 }))
            }))
        });
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
        let model = self.model_label();
        let mode = PERMISSION_MODES.iter().find(|(m, _, _)| *m == self.ai.mode).map_or("Ask before edits", |(_, _, chip)| chip);
        let (glyph, label, color) = if working { ("stop", "Stop", ui.danger) } else { ("send", "Send", accent) };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .text_size(px(12.0 * self.z()))
            .children(loader)
            .child(div().flex_1().min_w_0().truncate().text_color(hsla(if self.status.state == AiState::Idle { ui.text_dim } else { ui.ink(accent) })).child(status))
            .child(chip("ai-model", model).tooltip(menu::tooltip("The model Claude uses in this tab, and its effort: click to choose")).on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                this.warm_up();
                let entries = this.model_menu();
                this.open_menu(e.position, entries, cx);
            })))
            .child(chip("ai-effort", format!("{} effort", self.ai.effort.as_deref().unwrap_or("default"))).tooltip(menu::tooltip("How hard Claude thinks before answering: click to choose")).on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                this.warm_up();
                let entries = this.model_menu();
                this.open_menu(e.position, entries, cx);
            })))
            .child(chip("ai-mode", mode.into()).tooltip(menu::tooltip("What Claude may do without asking first (edits, commands): click to choose")).when(self.ai.mode == "bypassPermissions", |d| d.text_color(hsla(ui.danger))).on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                let entries = this.mode_menu();
                this.open_menu(e.position, entries, cx);
            })))
            .child(
                div()
                    .id("ai-more")
                    .p_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .tooltip(menu::tooltip("More: review changes, find in the conversation, attach a folder, insert a file, save the prompt or the conversation"))
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
                    .id("ai-browser")
                    .p_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .when(self.ai.browser, |d| d.bg(hsla_alpha(accent, 0.25)))
                    .hover(|s| s.bg(hsla(ui.tab_hover)))
                    .tooltip(menu::tooltip(if self.ai.browser {
                        "Connected to your browser: Claude can use Chrome or Brave through Claude in Chrome. Click to disconnect."
                    } else {
                        "Connect to your browser: Claude can then open and use tabs in Chrome or Brave (Claude in Chrome). One AI tab at a time."
                    }))
                    .on_click(cx.listener(|this, _, _, _| this.set_browser(!this.ai.browser)))
                    .child(svg().path(icon("globe")).size(px(14.0)).text_color(hsla(if self.ai.browser { ui.ink(accent) } else { ui.text_dim }))),
            )
            // Drafts: how many, opening their list; and keeping the prompt as one.
            .when(self.ai.shown_drafts().next().is_some(), |d| {
                let n = self.ai.shown_drafts().count();
                d.child(
                    chip("ai-drafts", format!("Drafts ({n})"))
                        .when(self.drafts_open, |d| d.bg(hsla_alpha(accent, 0.2)))
                        .tooltip(menu::tooltip("This session's drafts: messages kept to send later, only when you choose, in any order. Click to show or hide them."))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.drafts_open = !this.drafts_open;
                            cx.notify();
                        })),
                )
            })
            .child(
                div()
                    .id("ai-attach")
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .border_1()
                    .border_color(hsla(ui.divider))
                    .text_color(hsla(ui.text))
                    .cursor_pointer()
                    .hover(|s| s.bg(hsla(ui.tab_hover)))
                    .tooltip(menu::tooltip("Attach files to the message: pictures go as pictures, other files (PDFs, documents, code) as their paths for Claude to read. Dropping or pasting files does the same."))
                    .on_click(cx.listener(|this, _, _, cx| this.attach(false, cx)))
                    .child(svg().path(icon("paperclip")).size(px(13.0)).text_color(hsla(ui.text_dim)))
                    .child("Attach"),
            )
            .child(
                div()
                    .id("ai-draft")
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .border_1()
                    .border_color(hsla(ui.divider))
                    .text_color(hsla(ui.text))
                    .cursor_pointer()
                    .hover(|s| s.bg(hsla(ui.tab_hover)))
                    .tooltip(menu::tooltip(if self.editing_draft.is_some() { "Keep the changes to the draft (Ctrl+S)" } else { "Keep the message as a draft, to send later (Ctrl+S)" }))
                    .on_click(cx.listener(|this, _, _, cx| this.save_draft(cx)))
                    .child(svg().path(icon(if self.editing_draft.is_some() { "check" } else { "pencil" })).size(px(13.0)).text_color(hsla(ui.text_dim)))
                    .child(if self.editing_draft.is_some() { "Save Draft" } else { "Draft" }),
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
                    .tooltip(menu::tooltip(if working { "Stop Claude's turn (Esc)" } else { "Send the message (Ctrl+Enter); while Claude works, it waits its turn" }))
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
                .text_size(px(12.0 * self.z()))
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
    /// Connects this tab's Claude Code to the browser, or disconnects it.
    /// The keeper turns it off in any other AI tab, and the change reaches
    /// Claude Code at once, even in the middle of a turn.
    fn set_browser(&mut self, on: bool) {
        let msg = self.settings_msg(|a| a.browser = on);
        self.conn.send_msg(msg);
    }

    /// While connected to the browser, a strip above the prompt says so
    /// (and where), with Disconnect; or how to set Claude in Chrome up.
    fn render_browser(&self, ui: &UiColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.ai.browser {
            return None;
        }
        let accent = self.accent;
        let (text, color) = if self.browsers.is_empty() {
            (
                "Connected to the browser, but no Chrome or Brave here has Claude in Chrome: add it from the Chrome Web Store, sign in to it, and keep the browser open.".to_string(),
                ui.text,
            )
        } else {
            let names = self.browsers.join(" or ");
            (format!("Connected to your browser ({names}): Claude works in a tab group of its own there"), ui.ink(accent))
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap_2()
                .mx_3()
                .mt_1()
                .px_2()
                .py_0p5()
                .rounded_md()
                .bg(hsla_alpha(accent, 0.12))
                .border_1()
                .border_color(hsla_alpha(accent, 0.5))
                .text_size(px(12.0 * self.z()))
                .child(svg().path(icon("globe")).size(px(13.0)).flex_none().text_color(hsla(ui.ink(accent))))
                .child(div().flex_1().min_w_0().text_color(hsla(color)).child(text))
                .child(
                    div()
                        .id("ai-browser-off")
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_1p5()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_color(hsla(ui.text))
                        .hover(|s| s.bg(hsla(ui.tab_hover)))
                        .on_click(cx.listener(|this, _, _, _| this.set_browser(false)))
                        .child(svg().path(icon("close")).size(px(12.0)).text_color(hsla(ui.text_dim)))
                        .child("Disconnect"),
                )
                .into_any_element(),
        )
    }

    fn render_extras(&self, ui: &UiColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let accent = self.accent;
        let mut out = div().flex().flex_col().gap_1().mx_3().text_size(px(12.0 * self.z()));
        let mut any = false;
        if self.drafts_open && self.ai.shown_drafts().next().is_some() {
            any = true;
            let open = self.editing_draft.as_ref().map(|d| d.index);
            for (n, text) in self.ai.shown_drafts().map(|(n, d)| (n, &d.text)) {
                let editing = open == Some(n);
                let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
                let more = text.lines().count().saturating_sub(1);
                let action = |id: (&'static str, usize), label: &'static str, color: Rgb| {
                    div()
                        .id(id)
                        .flex_none()
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_color(hsla(color))
                        .hover(|s| s.bg(hsla(ui.tab_hover)))
                        .child(label)
                };
                out = out.child(
                    div()
                        .id(("draft", n))
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .border_1()
                        .border_color(hsla(ui.divider))
                        .tooltip(menu::tooltip_text(text.clone()))
                        .child(svg().path(icon("pencil")).size(px(12.0)).flex_none().text_color(hsla(ui.text_dim)))
                        .child(div().flex_1().min_w_0().truncate().text_color(hsla(ui.text)).child(first))
                        .when(more > 0, |d| d.child(div().flex_none().text_color(hsla(ui.text_dim)).child(format!("+{more} lines"))))
                        .when(editing, |d| d.border_color(hsla(accent)).child(div().flex_none().text_color(hsla(ui.ink(accent))).child("editing in the prompt")))
                        .when(!editing, |d| {
                            d.child(action(("draft-send", n), "Send", ui.ink(accent)).on_click(cx.listener(move |this, _, _, cx| this.send_draft(n, cx))))
                                .child(action(("draft-edit", n), "Edit", ui.text).on_click(cx.listener(move |this, _, window, cx| this.edit_draft(n, window, cx))))
                                .child(
                                    div()
                                        .id(("draft-merge", n))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .px_1()
                                        .rounded_sm()
                                        .cursor_pointer()
                                        .text_color(hsla(ui.text))
                                        .hover(|s| s.bg(hsla(ui.tab_hover)))
                                        .child(svg().path(icon("plus")).size(px(11.0 * self.z())).text_color(hsla(ui.text_dim)))
                                        .child("Merge into Prompt")
                                        .tooltip(menu::tooltip("Add the draft's text at the end of the prompt; the draft stays here"))
                                        .on_click(cx.listener(move |this, _, window, cx| this.merge_draft(n, window, cx))),
                                )
                                .child(action(("draft-delete", n), "Delete…", ui.danger).on_click(cx.listener(move |this, _, window, cx| this.delete_draft(n, window, cx))))
                        }),
                );
            }
        }
        // What Claude set to happen later: wake-ups, scheduled jobs, watches.
        for (n, t) in self.status.timers.iter().enumerate() {
            any = true;
            let (glyph, label) = match t.kind {
                TimerKind::Wakeup => ("clock", format!("Wakes up {}", timer_when(t))),
                TimerKind::Job => ("clock", format!("Scheduled job, {}", timer_when(t))),
                TimerKind::Watch => ("eye", format!("Watching, {}", timer_when(t))),
            };
            let detail = if t.prompt.is_empty() { t.what.clone() } else { format!("{}\n\nIt tells Claude: {}", t.what, t.prompt) };
            out = out.child(
                div()
                    .id(("timer", n))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .border_1()
                    .border_color(hsla_alpha(accent, 0.4))
                    .tooltip(menu::tooltip_text(format!(
                        "{detail}\n\nClaude Code does it while it keeps running for this tab. Closing the tab or its bench, changing the tab's folders or effort, or stopping Shed's keeper cancels it."
                    )))
                    .child(svg().path(icon(glyph)).size(px(12.0)).flex_none().text_color(hsla(ui.ink(accent))))
                    .child(div().flex_none().text_color(hsla(ui.ink(accent))).child(label))
                    .child(div().flex_1().min_w_0().truncate().text_color(hsla(ui.text)).child(t.what.clone())),
            );
        }
        // Sub-agents at work (and just done), pinned where they are seen.
        if let Some(agents) = self.render_agents(ui, cx) {
            any = true;
            out = out.child(agents);
        }
        // Waiting for Claude to be free (kept by the keeper, with the bench).
        let idle = self.status.state == AiState::Idle;
        for (n, q) in self.ai.queued.iter().enumerate() {
            any = true;
            let first = q.text.lines().next().unwrap_or("").to_string();
            let more = if q.pictures.is_empty() { String::new() } else { format!(" (+{} picture(s))", q.pictures.len()) };
            let action = |id: (&'static str, usize), label: &'static str| {
                div()
                    .id(id)
                    .flex_none()
                    .px_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_color(hsla(ui.text_dim))
                    .hover(|s| s.bg(hsla(ui.tab_hover)).text_color(hsla(ui.text)))
                    .child(label)
            };
            let (t1, t2) = (q.text.clone(), q.text.clone());
            out = out.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .bg(hsla_alpha(accent, 0.08))
                    .child(svg().path(icon("send")).size(px(12.0)).flex_none().text_color(hsla(ui.ink(accent))))
                    .child(div().flex_none().text_color(hsla(ui.text_dim)).child(if n == 0 { "Next:".to_string() } else { format!("Then ({}):", n + 1) }))
                    .child(div().flex_1().min_w_0().truncate().text_color(hsla(ui.text)).child(format!("{first}{more}")))
                    // Claude free (after a restart, say): it can go now.
                    .when(idle, |d| {
                        d.child(action(("queued-send", n), "Send Now").on_click(cx.listener(move |this, _, _, cx| {
                            this.conn.send_msg(ClientMsg::AiQueued { tab: this.tab, index: n as u32, text: t1.clone(), send: true });
                            this.list.scroll_to_end();
                            cx.notify();
                        })))
                    })
                    .child(action(("unqueue", n), "Remove").on_click(cx.listener(move |this, _, _, cx| {
                        this.conn.send_msg(ClientMsg::AiQueued { tab: this.tab, index: n as u32, text: t2.clone(), send: false });
                        cx.notify();
                    }))),
            );
        }
        if self.editing.is_some() {
            any = true;
            out = out.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_color(hsla(ui.text_dim))
                    .child(svg().path(icon("pencil")).size(px(12.0)).text_color(hsla(ui.ink(accent))))
                    .child(div().flex_1().child("Editing an earlier message: sending goes back to it (later messages go, files Claude changed since are put back). Esc cancels."))
                    .child(div().id("cancel-edit").cursor_pointer().text_color(hsla(ui.ink(accent))).child("Cancel").on_click(cx.listener(|this, _, _, cx| this.cancel_edit(cx)))),
            );
        }
        if self.editing_draft.is_some() {
            any = true;
            let button = |id: &'static str, glyph: &'static str, label: &'static str, color: Rgb| {
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_color(hsla(color))
                    .hover(|s| s.bg(hsla(ui.tab_hover)))
                    .child(svg().path(icon(glyph)).size(px(12.0)).text_color(hsla(color)))
                    .child(label)
            };
            out = out.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_color(hsla(ui.text_dim))
                    .child(svg().path(icon("pencil")).size(px(12.0)).text_color(hsla(ui.ink(accent))))
                    .child(div().flex_1().child("Editing a draft: Save Draft keeps the change, Cancel (Esc) leaves it as it was, Send sends it."))
                    .child(button("save-draft-edit", "check", "Save Draft", ui.ink(accent)).on_click(cx.listener(|this, _, _, cx| this.finish_draft_edit(true, cx))))
                    .child(button("cancel-draft-edit", "close", "Cancel", ui.text).on_click(cx.listener(|this, _, _, cx| this.finish_draft_edit(false, cx)))),
            );
        }
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
                    .tooltip(menu::tooltip("Claude's task list for this turn: click to show or hide it"))
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
            // The whole turn's changes, staged and committed ones too, in a view of their own.
            row = row.child(
                div()
                    .id("review-open")
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1p5()
                    .rounded_sm()
                    .bg(hsla_alpha(accent, 0.2))
                    .text_color(hsla(ui.ink(accent)))
                    .cursor_pointer()
                    .hover(|s| s.bg(hsla_alpha(accent, 0.3)))
                    .tooltip(menu::tooltip("Every change of this turn or the session: not staged, staged, committed, pushed"))
                    .on_click(cx.listener(|this, _, window, cx| this.open_review(review::Scope::Turn, window, cx)))
                    .child(svg().path(icon("branch")).size(px(12.0)).text_color(hsla(ui.ink(accent))))
                    .child("Review…"),
            );
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
        let menu = self.menu.as_ref().map(|m| menu::render(m, &ui, cx));
        let count = self.shown_sessions(cx).len();
        let sessions = if self.show_prompts {
            let shown = self.shown_prompts(cx).len();
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(uniform_list("ai-prompts", shown, cx.processor(|this, range, _, cx| this.render_prompt_rows(range, cx))).track_scroll(&self.prompts_scroll).flex_1().min_h_0())
                .child(crate::scrollbar::vertical(crate::scrollbar::Source::uniform(&self.prompts_scroll), &self.prompts_bar, &ui, false).absolute().top_0().right_0().bottom_0())
        } else {
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(uniform_list("ai-sessions", count, cx.processor(|this, range, _, cx| this.render_sessions(range, cx))).track_scroll(&self.sessions_scroll).flex_1().min_h_0())
                .child(crate::scrollbar::vertical(crate::scrollbar::Source::uniform(&self.sessions_scroll), &self.sessions_bar, &ui, false).absolute().top_0().right_0().bottom_0())
        };
        // Sessions of the folder, or this session's prompts.
        let prompt_count = self.entries.iter().filter(|e| matches!(e, Entry::User { .. })).count();
        let switch = div().flex().p_0p5().gap_0p5().rounded_md().bg(hsla(ui.tab_active)).text_size(px(12.0 * self.z())).children([(false, "Sessions".to_string()), (true, format!("Prompts ({prompt_count})"))].map(|(prompts, label)| {
            let on = self.show_prompts == prompts;
            div()
                .id(if prompts { "left-prompts" } else { "left-sessions" })
                .flex_1()
                .flex()
                .justify_center()
                .py_0p5()
                .rounded_sm()
                .cursor_pointer()
                .when(on, |d| d.bg(hsla_alpha(self.accent, 0.22)).text_color(hsla(ui.text)))
                .when(!on, |d| d.text_color(hsla(ui.text_dim)).hover(|s| s.bg(hsla(ui.tab_hover))))
                .tooltip(menu::tooltip(if prompts { "Your messages in this conversation: click one to go to it" } else { "Claude Code's sessions in this tab's folder: click one to open it" }))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.show_prompts = prompts;
                    cx.notify();
                }))
        }));
        let transcript = div()
            .relative()
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
            .child(
                list(
                    self.list.clone(),
                    cx.processor(|this, row, _, cx| match this.entry_at(row) {
                        Some(i) => this.entry_view(i, cx).into_any_element(),
                        None => this.render_earlier(cx),
                    }),
                )
                .flex_1()
                .min_h_0()
                .py_2(),
            )
            // After the list: over it, and first to hear the mouse.
            .child({
                let view = cx.entity().downgrade();
                let step: crate::scrollbar::Step = std::rc::Rc::new(move |up, _, cx| {
                    view.update(cx, |this, cx| this.jump_block(up, cx)).ok();
                });
                crate::scrollbar::vertical_with_blocks(crate::scrollbar::Source::List(self.list.clone()), &self.conversation_bar, &ui, false, Some(step)).absolute().top_0().right_0().bottom_0()
            });
        let empty = self.entries.is_empty();
        div()
            .key_context("AiTab")
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::selection_key))
            .on_action(cx.listener(|this, _: &crate::actions::PrevBlock, _, cx| this.jump_block(true, cx)))
            .on_action(cx.listener(|this, _: &crate::actions::NextBlock, _, cx| this.jump_block(false, cx)))
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
            .text_size(px(13.0 * self.z()))
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
                    .child(switch)
                    .child(div().text_size(px(12.0 * self.z())).child(self.filter.clone()))
                    // Deleting a session did not work: why, until clicked away.
                    .when_some(self.delete_problem.clone(), |d, problem| {
                        d.child(
                            div()
                                .id("delete-problem")
                                .flex()
                                .items_start()
                                .gap_1()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .border_1()
                                .border_color(hsla(ui.danger))
                                .text_size(px(12.0 * self.z()))
                                .text_color(hsla(ui.danger))
                                .cursor_pointer()
                                .tooltip(menu::tooltip("Dismiss"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.delete_problem = None;
                                    cx.notify();
                                }))
                                .child(svg().path(icon("alert")).size(px(12.0)).flex_none().mt_0p5().text_color(hsla(ui.danger)))
                                .child(div().flex_1().min_w_0().child(problem)),
                        )
                    })
                    .child(sessions),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .when(self.review.is_some(), |d| d.child(self.render_review(&ui, cx)))
                    .when(self.review.is_none(), |d| d.child(self.render_header(&ui, cx)))
                    .when(self.review.is_none(), |d| d.child(if empty {
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
                    }))
                    .when(self.review.is_none(), |d| {
                        d.children(self.render_browser(&ui, cx))
                            .children(self.render_extras(&ui, cx))
                            .children(self.render_completion(&ui, cx))
                            .children(self.render_attachments(&ui, cx))
                    })
                    .when(self.review.is_none(), |d| d.child(
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
                    ))
                    .when(self.review.is_none(), |d| d.child(self.render_footer(&ui, cx))),
            )
            .children(menu)
    }
}

/// Text as written, in the monospaced font, on a darker ground.
fn pre(text: &str, ui: &UiColors, mono: &str, color: Option<Rgb>, z: f32) -> impl IntoElement {
    div()
        .my_1()
        .px_2()
        .py_1()
        .rounded_sm()
        .bg(hsla_alpha(ui.background, 0.8))
        .border_1()
        .border_color(hsla(ui.divider))
        .font_family(mono.to_string())
        .text_size(px(12.0 * z))
        .text_color(hsla(color.unwrap_or(ui.text)))
        .children(text.lines().map(|l| div().min_h(px(16.0)).child(l.to_string())))
}

/// An edit: the lines taken out, then the lines put in.
fn diff_view(old: &str, new: &str, ui: &UiColors, mono: &str, z: f32) -> impl IntoElement {
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
        .text_size(px(12.0 * z))
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

/// A message's time in small text: `14:32 · 5 min ago` (another day:
/// `12 Nov 14:32 · 2 d ago`), after `who` if given.
fn stamp(at: u64, who: Option<&str>, ui: &UiColors, z: f32) -> Option<gpui::Div> {
    if at == 0 {
        return None;
    }
    let mut parts: Vec<String> = who.map(String::from).into_iter().collect();
    parts.push(local_time(at));
    parts.push(ago(at));
    Some(div().flex_none().text_size(px(11.0 * z)).text_color(hsla_alpha(ui.text_dim, 0.85)).child(parts.join(" · ")))
}

/// A time as the clock here reads it: `14:32`, or `12 Nov 14:32` another day.
fn local_time(secs: u64) -> String {
    let local = |t: u64| {
        let t = t as libc::time_t;
        // SAFETY: localtime_r fills the struct from `t` and touches nothing else.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::localtime_r(&t, &mut tm) };
        tm
    };
    let (tm, today) = (local(secs), local(unix_now()));
    let time = format!("{:02}:{:02}", tm.tm_hour, tm.tm_min);
    if (tm.tm_year, tm.tm_yday) == (today.tm_year, today.tm_yday) {
        return time;
    }
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {} {time}", tm.tm_mday, MONTHS[tm.tm_mon.clamp(0, 11) as usize])
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

/// How a usage window is going against a steady pace through it.
#[derive(Debug, PartialEq)]
struct Pace {
    /// How much of the window has gone, 0 to 1: where a steady pace is now.
    elapsed: f32,
    /// At the pace so far, seconds until the limit, when that is before the reset.
    limit_in: Option<u64>,
    /// Otherwise, how much would be used by the reset (when it can be told).
    by_reset: Option<f32>,
}

impl Pace {
    fn of(used: f32, resets: u64, now: u64, length: u64) -> Pace {
        let left = resets.saturating_sub(now).min(length);
        let gone = length - left;
        let elapsed = gone as f32 / length.max(1) as f32;
        // Too early in the window to tell a pace.
        if gone < length / 50 || used <= 0.0 {
            return Pace { elapsed, limit_in: None, by_reset: None };
        }
        let rate = used / gone as f32;
        let to_limit = ((1.0 - used).max(0.0) / rate) as u64;
        if to_limit < left {
            Pace { elapsed, limit_in: Some(to_limit), by_reset: None }
        } else {
            Pace { elapsed, limit_in: None, by_reset: Some(used + rate * left as f32) }
        }
    }
}

/// The account's usage limits as small bars, for a bench's top bar: green,
/// amber from 70 %, red from 90 %; under each, the time left until it resets;
/// a line where a steady pace would be. `None` when none is known.
pub fn limit_bars(limits: Limits, ui: &UiColors, palette: &shed_emu::Palette, z: f32) -> Option<gpui::Div> {
    if limits.five_hour.is_none() && limits.week.is_none() {
        return None;
    }
    #[cfg(test)]
    BARS_DRAWN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (green, amber) = (palette.ansi[2], palette.ansi[3]);
    // Reported a while ago (Claude Code has not run since): said in the tooltips.
    let age = unix_now().saturating_sub(limits.seen);
    let reported = if limits.seen > 0 && age >= 600 { format!("\nAs Claude Code last reported, {} ago", short_span(age)) } else { String::new() };
        let window = |id: &'static str, label: &str, length: u64, w: Option<shed_ai::Window>| {
        w.map(|w| {
            let used = w.used.clamp(0.0, 1.0);
            let pct = (used * 100.0).round() as u32;
            let color = if pct >= 90 { ui.danger } else if pct >= 70 { amber } else { green };
            let width = 68.0 * z;
            let pace = Pace::of(used, w.resets, unix_now(), length);
            let pace_text = match (pace.limit_in, pace.by_reset) {
                (Some(secs), _) => format!("at this pace the limit comes in {} (before it resets)", short_span(secs)),
                (None, Some(end)) => format!("at this pace about {}% by the time it resets", (end * 100.0).round() as u32),
                _ => String::new(),
            };
            let over = pace.limit_in.is_some();
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1p5()
                .tooltip(menu::tooltip_text(format!(
                    "{label}: {pct}% used · resets {}{}{pace_text}\nThe line on the bar: where a steady pace would be now{reported}",
                    until(w.resets),
                    if pace_text.is_empty() { "" } else { "\n" }
                )))
                // What it is and how much is used above the bar, how long
                // until it resets beneath: no wider than the bar.
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .w(px(width))
                        .gap(px(1.0))
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .text_size(px(10.0 * z))
                                .line_height(px(11.0 * z))
                                .child(div().text_color(hsla(ui.text_dim)).child(label.to_string()))
                                .child(div().text_color(hsla(if pct >= 70 { ui.ink(color) } else { ui.text_dim })).child(format!("{pct}%"))),
                        )
                        .child(
                            div()
                                .relative()
                                .w(px(width))
                                .h(px(5.0 * z))
                                .rounded_full()
                                .bg(hsla_alpha(ui.text_dim, 0.22))
                                .child(div().h_full().w(px(width * used.max(0.02))).rounded_full().bg(hsla(color)))
                                // Where a steady pace through the window would be now.
                                .child(
                                    div()
                                        .absolute()
                                        .left(px((width * pace.elapsed - 1.0).max(0.0)))
                                        .top(px(-2.0 * z))
                                        .w(px(2.0))
                                        .h(px(9.0 * z))
                                        .rounded_sm()
                                        .bg(hsla_alpha(ui.text, 0.85)),
                                ),
                        )
                        // Over the pace, the time left is in amber: the limit comes first.
                        .child(
                            div()
                                .flex()
                                .justify_center()
                                .text_size(px(9.5 * z))
                                .line_height(px(11.0 * z))
                                .text_color(hsla(if over { ui.ink(amber) } else { ui.text_dim }))
                                .child(time_left(w.resets)),
                        ),
                )
        })
    };
    Some(
        div()
            .flex()
            .flex_none()
            .gap_3()
            .items_center()
            .text_size(px(11.0 * z))
            .children(window("limit-5h", "5 h", 5 * 3600, limits.five_hour))
            .children(window("limit-week", "week", 7 * 24 * 3600, limits.week)),
    )
}

/// A span, short: `45m`, `3h 10m`, `2d 4h`.
fn short_span(d: u64) -> String {
    match d {
        0..3600 => format!("{}m", d / 60),
        3600..86400 => format!("{}h {}m", d / 3600, d % 3600 / 60),
        _ => format!("{}d {}h", d / 86400, d % 86400 / 3600),
    }
}

/// Time left until `secs`, short: `2h 14m left`, `3d 4h left`.
fn time_left(secs: u64) -> String {
    let d = secs.saturating_sub(unix_now());
    match d {
        0..60 => "resets now".into(),
        60..3600 => format!("{}m left", d / 60),
        3600..86400 => format!("{}h {}m left", d / 3600, d % 3600 / 60),
        _ => format!("{}d {}h left", d / 86400, d % 86400 / 3600),
    }
}

fn secs_since(start: u64) -> String {
    let d = unix_now().saturating_sub(start);
    if d < 60 { format!("{d} s") } else { format!("{} min {} s", d / 60, d % 60) }
}

/// Pictures kept as files, small (`height` at zoom 1); a click opens one
/// whole, and one gone since says so.
/// Pictures, small; a click opens one whole. `fetched`: another
/// workspace's, as fetched from there (in order; `None` while coming).
fn thumbnails(entry: usize, pictures: &[String], fetched: Option<&[Option<Arc<gpui::Image>>]>, height: f32, ui: &UiColors, z: f32) -> gpui::Div {
    let dim = ui.text_dim;
    if let Some(fetched) = fetched {
        return div().mt_2().flex().flex_wrap().gap_2().children(fetched.iter().enumerate().map(|(n, image)| {
            let inner = match image {
                Some(image) => gpui::img(image.clone()).h(px(height * z)).max_w(px(height * 2.5 * z)).rounded_sm().into_any_element(),
                None => div().px_2().py_1().text_size(px(11.0 * z)).text_color(hsla(dim)).child("picture…").into_any_element(),
            };
            let image = image.clone();
            div()
                .id(SharedString::from(format!("picture-{entry}-{n}")))
                .flex_none()
                .rounded_md()
                .border_1()
                .border_color(hsla(ui.divider))
                .p_0p5()
                .when_some(image, |d, image| {
                    d.cursor_pointer()
                        .tooltip(menu::tooltip("Open the picture"))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, _, cx| remote::open_picture(&image, cx))
                })
                .child(inner)
        }));
    }
    div().mt_2().flex().flex_wrap().gap_2().children(pictures.iter().enumerate().map(|(n, path)| {
        let url = format!("file://{path}");
        div()
            .id(SharedString::from(format!("picture-{entry}-{n}")))
            .flex_none()
            .rounded_md()
            .border_1()
            .border_color(hsla(ui.divider))
            .p_0p5()
            .cursor_pointer()
            .tooltip(menu::tooltip("Open the picture"))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, _, cx| crate::links::open_external(&url, cx))
            .child(
                gpui::img(PathBuf::from(path))
                    .h(px(height * z))
                    .max_w(px(height * 2.5 * z))
                    .rounded_sm()
                    .with_fallback(move || div().px_2().py_1().text_size(px(11.0 * z)).text_color(hsla(dim)).child("picture no longer there").into_any_element()),
            )
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GPUI lays out text in a block as its paragraph, leaving the text's
    /// own layout empty; in a flex box the text has one, and the mouse can
    /// be mapped to a character (what selecting needs).
    #[test]
    fn usage_against_a_steady_pace() {
        let week = 7 * 24 * 3600;
        let now = 1_000_000;
        // Two days in, 40% used: the limit comes in 3 days, 2 before the reset.
        let p = Pace::of(0.4, now + 5 * 86400, now, week);
        assert!((p.elapsed - 2.0 / 7.0).abs() < 0.001);
        assert_eq!(p.limit_in, Some(3 * 86400));
        // Two days in, 20%: on pace for 70% by the reset.
        let p = Pace::of(0.2, now + 5 * 86400, now, week);
        assert_eq!(p.limit_in, None);
        assert!((p.by_reset.unwrap() - 0.7).abs() < 0.001);
        // Just reset: too early to tell.
        assert_eq!(Pace::of(0.01, now + week, now, week), Pace { elapsed: 0.0, limit_in: None, by_reset: None });
    }

    struct Bars(Limits, std::rc::Rc<std::cell::Cell<gpui::Size<gpui::Pixels>>>);
    impl Render for Bars {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let seen = self.1.clone();
            let bars = limit_bars(self.0, &UiColors::default(), &shed_emu::Palette::default(), 1.0).expect("bars");
            div().flex().child(div().flex().child(bars).child(gpui::canvas(move |b, _, _| seen.set(b.size), |_, _, _, _| {}).absolute().size_full()))
        }
    }

    #[gpui::test]
    fn limit_bars_take_room(cx: &mut gpui::TestAppContext) {
        let now = unix_now();
        let limits = Limits {
            five_hour: Some(shed_ai::Window { used: 0.37, resets: now + 2 * 3600 }),
            week: Some(shed_ai::Window { used: 0.48, resets: now + 3 * 86400 }),
            seen: now - 60,
        };
        let size = std::rc::Rc::new(std::cell::Cell::new(gpui::Size::default()));
        let seen = size.clone();
        let (_, cx) = cx.add_window_view(|_, _| Bars(limits, seen));
        cx.run_until_parked();
        println!("bars: {:?}", size.get());
        assert!(size.get().width > px(100.0) && size.get().height > px(10.0), "{:?}", size.get());
    }

    #[test]
    fn token_counts_are_short() {
        assert_eq!((tokens(84_312), tokens(200_000), tokens(1_000_000), tokens(1_500_000)), ("84k".into(), "200k".into(), "1M".into(), "1.5M".into()));
    }

    /// Editing drafts: a draft opens in the prompt without leaving the list,
    /// Save Draft changes it in place, Cancel leaves it, and opening a second
    /// draft puts the first back rather than joining them.
    #[gpui::test]
    fn drafts_are_edited_in_place(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            crate::theme::set_ui_scale(1.0);
            cx.set_global(crate::prompt::CurrentUi(UiColors::default()));
        });
        let (view, cx) = cx.add_window_view(|_, cx| {
            let ai = shed_model::AiTab {
                session: Some("s".into()),
                drafts: vec![shed_model::Draft { session: "s".into(), text: "first".into() }, shed_model::Draft { session: "s".into(), text: "second".into() }],
                ..Default::default()
            };
            AiView::new(BenchId::new(), TabId::new(), ai, Rgb::hex(0xFF8A57), crate::keeper_conn::KeeperConn::detached(), crate::settings::Settings::default(), cx)
        });
        let prompt = |view: &Entity<AiView>, cx: &mut gpui::VisualTestContext| view.update(cx, |v, cx| v.prompt.read(cx).buffer.text());
        let set = |view: &Entity<AiView>, cx: &mut gpui::VisualTestContext, text: &str| {
            view.update(cx, |v, cx| {
                v.prompt.update(cx, |p, cx| {
                    p.buffer.select_all();
                    p.buffer.insert(text);
                    cx.notify();
                })
            })
        };
        let drafts = |view: &Entity<AiView>, cx: &mut gpui::VisualTestContext| view.update(cx, |v, _| v.ai.drafts.iter().map(|d| d.text.clone()).collect::<Vec<_>>());

        set(&view, cx, "half written");
        view.update_in(cx, |v, window, cx| v.edit_draft(0, window, cx));
        assert_eq!(prompt(&view, cx), "first");
        assert_eq!(drafts(&view, cx), ["first", "second"], "the draft stays in the list while edited");
        set(&view, cx, "first, changed");
        // Opening the second puts the first back, changed, not joined.
        view.update_in(cx, |v, window, cx| v.edit_draft(1, window, cx));
        assert_eq!(prompt(&view, cx), "second");
        assert_eq!(drafts(&view, cx), ["first, changed", "second"]);
        set(&view, cx, "second, changed");
        view.update(cx, |v, cx| v.finish_draft_edit(false, cx));
        assert_eq!(drafts(&view, cx), ["first, changed", "second"], "cancel leaves it as it was");
        assert_eq!(prompt(&view, cx), "half written", "what was being written comes back");
        view.update_in(cx, |v, window, cx| v.edit_draft(1, window, cx));
        set(&view, cx, "second, changed");
        view.update(cx, |v, cx| v.save_draft(cx));
        assert_eq!(drafts(&view, cx), ["first, changed", "second, changed"]);
        assert_eq!(prompt(&view, cx), "half written");
        // Sending a draft being edited sends it and takes it out of the list.
        view.update_in(cx, |v, window, cx| v.edit_draft(0, window, cx));
        view.update(cx, |v, cx| v.send(cx));
        assert_eq!(drafts(&view, cx), ["second, changed"]);
        assert_eq!(prompt(&view, cx), "half written");
    }

    /// Merge into Prompt: a draft's text goes at the end of the prompt after
    /// a blank line and stays in the list; while another draft is edited,
    /// it goes into that edit.
    #[gpui::test]
    fn drafts_merge_into_the_prompt(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            crate::theme::set_ui_scale(1.0);
            cx.set_global(crate::prompt::CurrentUi(UiColors::default()));
        });
        let (view, cx) = cx.add_window_view(|_, cx| {
            let ai = shed_model::AiTab {
                session: Some("s".into()),
                drafts: vec![shed_model::Draft { session: "s".into(), text: "first".into() }, shed_model::Draft { session: "s".into(), text: "second\nline".into() }],
                ..Default::default()
            };
            AiView::new(BenchId::new(), TabId::new(), ai, Rgb::hex(0xFF8A57), crate::keeper_conn::KeeperConn::detached(), crate::settings::Settings::default(), cx)
        });
        let prompt = |view: &Entity<AiView>, cx: &mut gpui::VisualTestContext| view.update(cx, |v, cx| v.prompt.read(cx).buffer.text());
        let cursor = |view: &Entity<AiView>, cx: &mut gpui::VisualTestContext| view.update(cx, |v, cx| v.prompt.read(cx).buffer.selection().range());
        let drafts = |view: &Entity<AiView>, cx: &mut gpui::VisualTestContext| view.update(cx, |v, _| v.ai.drafts.iter().map(|d| d.text.clone()).collect::<Vec<_>>());

        // Into an empty prompt: the draft alone.
        view.update_in(cx, |v, window, cx| v.merge_draft(0, window, cx));
        assert_eq!(prompt(&view, cx), "first");
        // Then another, after a blank line, the cursor at the end.
        view.update_in(cx, |v, window, cx| v.merge_draft(1, window, cx));
        assert_eq!(prompt(&view, cx), "first\n\nsecond\nline");
        let end = "first\n\nsecond\nline".chars().count();
        assert_eq!(cursor(&view, cx), end..end);
        assert_eq!(drafts(&view, cx), ["first", "second\nline"], "the drafts stay");
        assert!(view.update_in(cx, |v, window, cx| v.prompt.read(cx).focus_handle().is_focused(window)), "the prompt has the keyboard");

        // While editing the first draft, the second goes into that edit.
        view.update_in(cx, |v, window, cx| v.edit_draft(0, window, cx));
        view.update_in(cx, |v, window, cx| v.merge_draft(1, window, cx));
        view.update(cx, |v, cx| v.save_draft(cx));
        assert_eq!(drafts(&view, cx), ["first\n\nsecond\nline", "second\nline"]);
        assert_eq!(prompt(&view, cx), "first\n\nsecond\nline", "what was being written comes back");
    }

    /// Frame time with a long real conversation (measurement, not a check):
    /// SHED_PERF_SESSION=<folder>:<id> cargo test -p shed-app --bins perf_long -- --ignored --nocapture
    #[gpui::test]
    #[ignore]
    fn perf_long_conversation(cx: &mut gpui::TestAppContext) {
        let spec = std::env::var("SHED_PERF_SESSION").expect("SHED_PERF_SESSION=<folder>:<id>");
        let (folder, id) = spec.rsplit_once(':').unwrap();
        let t = shed_ai::load_session(std::path::Path::new(folder), id, None).unwrap();
        let entries = t.entries.clone();
        cx.update(|cx| {
            crate::theme::set_ui_scale(1.0);
            cx.set_global(crate::prompt::CurrentUi(UiColors::default()));
        });
        let (view, cx) = cx.add_window_view(|_, cx| {
            let ai = shed_model::AiTab { folder: folder.into(), ..Default::default() };
            AiView::new(BenchId::new(), TabId::new(), ai, Rgb::hex(0xFF8A57), shed_client::keeper_conn::KeeperConn::detached(), crate::settings::Settings::default(), cx)
        });
        cx.simulate_resize(gpui::size(px(1600.0), px(1000.0)));
        view.update(cx, |v, cx| v.update(0, entries, Status::default(), cx));
        cx.run_until_parked();
        let frame = |cx: &mut gpui::VisualTestContext, label: &str, n: u32| {
            RENDERED.store(0, std::sync::atomic::Ordering::Relaxed);
            let started = std::time::Instant::now();
            for _ in 0..n {
                cx.update(|window, _| window.refresh());
                cx.run_until_parked();
            }
            let rendered = RENDERED.load(std::sync::atomic::Ordering::Relaxed) as u32 / n;
            println!("{label}: {:?} per frame, {rendered} entries drawn per frame", started.elapsed() / n);
        };
        frame(cx, "first frames", 3);
        frame(cx, "steady", 30);
        let all = view.update(cx, |v, _| v.entries.clone());
        view.update(cx, |v, cx| v.update(0, Vec::new(), Status::default(), cx));
        frame(cx, "empty conversation", 30);
        view.update(cx, |v, cx| v.update(0, all, Status::default(), cx));
        frame(cx, "back (first)", 3);
        frame(cx, "back (steady)", 30);
        // What a hover does: one entry's view notified; and a change to the
        // conversation: the AI view notified.
        let timed = |cx: &mut gpui::VisualTestContext, label: &str, poke: &dyn Fn(&mut gpui::VisualTestContext)| {
            RENDERED.store(0, std::sync::atomic::Ordering::Relaxed);
            let started = std::time::Instant::now();
            for _ in 0..30 {
                poke(cx);
                cx.run_until_parked();
            }
            println!("{label}: {:?} per frame, {} entries drawn per frame", started.elapsed() / 30, RENDERED.load(std::sync::atomic::Ordering::Relaxed) / 30);
        };
        let one = view.update(cx, |v, _| v.entry_views.iter().max_by_key(|(k, _)| **k).map(|(_, e)| e.clone())).unwrap();
        timed(cx, "hover in one entry", &|cx| one.update(cx, |_, cx| cx.notify()));
        timed(cx, "conversation changed", &|cx| view.update(cx, |_, cx| cx.notify()));
        // Every step on screen opened.
        view.update(cx, |v, _| {
            for i in 0..v.entries.len() {
                if matches!(v.entries[i], Entry::Tool(_)) {
                    v.expanded.insert(i);
                }
            }
            v.list.remeasure();
        });
        frame(cx, "steps opened (first)", 3);
        frame(cx, "steps opened (steady)", 30);
    }

    /// A recorded conversation with sub-agents, a background command, notes
    /// and a wake-up waiting, drawn with every agent, step and note opened,
    /// then changed in place as a background agent's progress is.
    #[gpui::test]
    fn agents_notes_and_timers_draw(cx: &mut gpui::TestAppContext) {
        let mut t = shed_ai::Transcript::new("/tmp/shed-cap-b");
        let stream = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../shed-ai/tests/streams/agents.jsonl")).unwrap();
        for line in stream.lines() {
            t.apply(&serde_json::from_str(line).unwrap());
        }
        t.apply(&serde_json::json!({"type": "user", "isCompactSummary": true, "message": {"content": "Summary:\n- **one**\n- two"}}));
        t.apply(&serde_json::json!({"type": "user", "message": {"content": "<local-command-stdout>Compacted\nwith 2 lines</local-command-stdout>"}}));
        let mut status = t.status.clone();
        status.timers = vec![
            Timer { kind: TimerKind::Wakeup, what: "check back".into(), at: unix_now() + 600, tool: "x".into(), ..Default::default() },
            Timer { kind: TimerKind::Job, what: "tick".into(), every: "Every minute".into(), ..Default::default() },
            Timer { kind: TimerKind::Watch, what: "the log".into(), at: unix_now() + 30, ..Default::default() },
        ];
        let entries = t.entries.clone();
        cx.update(|cx| {
            crate::theme::set_ui_scale(1.0);
            cx.set_global(crate::prompt::CurrentUi(UiColors::default()));
        });
        let (view, cx) = cx.add_window_view(|_, cx| {
            let ai = shed_model::AiTab { folder: "/tmp/shed-cap-b".into(), ..Default::default() };
            AiView::new(BenchId::new(), TabId::new(), ai, Rgb::hex(0xFF8A57), crate::keeper_conn::KeeperConn::detached(), crate::settings::Settings::default(), cx)
        });
        cx.simulate_resize(gpui::size(px(1400.0), px(3000.0)));
        view.update(cx, |v, cx| v.update(0, entries, status.clone(), cx));
        cx.run_until_parked();
        view.update(cx, |v, _| {
            for i in 0..v.entries.len() {
                if let Entry::Tool(tool) = &v.entries[i] {
                    for s in tool.run.iter().flat_map(|r| &r.steps) {
                        if let Entry::Tool(s) = s {
                            v.open_steps.insert((i, s.id.clone()));
                        }
                    }
                }
                if matches!(v.entries[i], Entry::Tool(_) | Entry::Note(_)) {
                    v.expanded.insert(i);
                }
            }
            v.list.remeasure();
        });
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let agent = view.update(cx, |v, _| v.entries.iter().position(|e| matches!(e, Entry::Tool(t) if t.summary == "Agent: Echo twice")).unwrap());
        let parsed = view.update(cx, |v, _| v.parsed.get(&agent).cloned()).expect("the opened agent was drawn");
        assert_eq!(parsed.groups.len(), 4, "three steps and the report");
        assert!(view.update(cx, |v, _| v.tick_every()) == Some(Duration::from_secs(1)), "a watch ends soon: its countdown keeps up");
        // A step more for the agent, in place: drawn again, the rest kept.
        let mut changed = view.update(cx, |v, _| v.entries[agent].clone());
        if let Entry::Tool(tool) = &mut changed {
            let run = tool.run.as_mut().unwrap();
            run.steps.push(Entry::Assistant { text: "and **more**".into(), at: 0 });
            run.state = RunState::Running;
        }
        view.update(cx, |v, cx| v.replace(vec![(agent as u32, changed)], status, cx));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert!(view.update(cx, |v, _| v.running));
        let parsed = view.update(cx, |v, _| v.parsed.get(&agent).cloned()).unwrap();
        assert_eq!(parsed.groups.len(), 5);
        // Running, it is pinned above the prompt; its line there opens it.
        assert_eq!(view.update(cx, |v, _| v.agents.clone()), vec![agent]);
        view.update(cx, |v, cx| {
            v.expanded.remove(&agent);
            v.show_agent(agent, cx);
            assert!(v.expanded.contains(&agent));
        });
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        // Idle, a run going on for long (a server left in the background):
        // drawn again now and then, its mark still.
        let tick = view.update(cx, |v, _| {
            v.status.timers.clear();
            v.newest_run = unix_now() - 3600;
            (v.tick_every(), v.turning())
        });
        assert_eq!(tick, (Some(Duration::from_secs(30)), "◐"));
        // Nothing running, a wake-up long past its time: no drawing at all.
        let tick = view.update(cx, |v, _| {
            v.running = false;
            v.status.timers = vec![Timer { kind: TimerKind::Wakeup, at: unix_now() - 600, ..Default::default() }, Timer { kind: TimerKind::Job, every: "Every minute".into(), ..Default::default() }];
            v.tick_every()
        });
        assert_eq!(tick, None);
        // Another entry where an opened one was: not opened in its place.
        let status = view.update(cx, |v, _| v.status.clone());
        view.update(cx, |v, cx| v.update(agent, vec![Entry::Notice { text: "Stopped".into(), error: false }], status, cx));
        assert!(!view.update(cx, |v, _| v.expanded.contains(&agent)));
    }

    #[gpui::test]
    fn selectable_text_can_be_hit(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        let text = "hello world";
        let in_flex = StyledText::new(text);
        let in_block = StyledText::new(text);
        let nested = StyledText::new(text);
        let (flex_layout, block_layout, nested_layout) = (in_flex.layout().clone(), in_block.layout().clone(), nested.layout().clone());
        cx.draw(gpui::point(px(0.0), px(0.0)), gpui::size(px(300.0), px(200.0)), move |_, _| {
            div()
                .flex()
                .flex_col()
                .child(div().flex().flex_col().min_w_0().child(in_flex))
                .child(div().child(in_block))
                // As in a message: a block around the flex box around the text.
                .child(div().px_3().child(div().flex().flex_col().min_w_0().child(nested)))
        });
        assert!(text_index(&nested_layout, text, gpui::point(px(14.0), px(44.0))).is_some());
        assert!(over_text(&flex_layout, gpui::point(px(2.0), px(2.0))));
        assert!(text_index(&flex_layout, text, gpui::point(px(2.0), px(2.0))).is_some());
        assert!(text_index(&block_layout, text, gpui::point(px(2.0), px(2.0))).is_none(), "GPUI changed: blocks fill the layout now");
    }

    #[test]
    fn times_read_as_the_clock_here() {
        let now = unix_now();
        let t = local_time(now);
        assert_eq!(t.len(), 5, "today: just the time, {t}");
        assert!(local_time(now - 3 * 86400).contains(' '), "another day: with the date");
        let ui = UiColors::default();
        assert!(stamp(0, None, &ui, 1.0).is_none(), "no time known: nothing");
    }

    #[test]
    fn jumps_land_on_messages() {
        let user = || Entry::User { text: "q".into(), images: 0, at: 0, pictures: Vec::new() };
        let said = || Entry::Assistant { text: "a".into(), at: 0 };
        let tool = || Entry::Tool(Default::default());
        // 0 you, 1 response (2, 3 more of it), 4 you, 5 response, 6 more.
        let e = vec![user(), said(), tool(), said(), user(), tool(), said()];
        assert_eq!(block_target(&e, 3, false, true), Some(1));
        assert_eq!(block_target(&e, 1, false, true), Some(0));
        assert_eq!(block_target(&e, 1, true, true), Some(1), "inside the response: its start");
        assert_eq!(block_target(&e, 0, false, true), None);
        assert_eq!(block_target(&e, 1, false, false), Some(4));
        assert_eq!(block_target(&e, 4, false, false), Some(5));
        assert_eq!(block_target(&e, 5, false, false), None, "then the end");
    }

    #[test]
    fn the_default_model_is_named() {
        let m = Model { value: "default".into(), name: "Default (recommended)".into(), description: "Opus 5.5 · Best for everyday".into(), efforts: Vec::new() };
        assert_eq!(default_model_name(&m), "Opus 5.5");
        let bare = Model { description: String::new(), ..m };
        assert_eq!(default_model_name(&bare), "Default (recommended)");
        assert!(PERMISSION_MODES.iter().any(|(m, _, _)| *m == "auto") && PERMISSION_MODES.iter().any(|(m, _, _)| *m == "bypassPermissions"));
    }

    #[test]
    fn words_and_times() {
        assert_eq!(lower_first("Edit x"), "edit x");
        assert!(ago(unix_now() - 120).starts_with("2 min"));
    }
}
