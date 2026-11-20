//! Claude Code, as Shed shows it: a conversation made from the program's
//! stream of JSON events (`--output-format stream-json`) or from a saved
//! session's file, and the list of a folder's sessions. No UI and no
//! processes: the keeper runs `claude`, windows draw what this builds.

pub mod markdown;
pub mod aside;
pub mod listing;
pub mod notes;
pub mod sessions;
mod summary;
mod transcript;

pub use listing::{SessionRow, family_of, session_rows};
pub use sessions::{SessionInfo, list_sessions, load_session, rename_session};
pub use summary::{BROWSER_TOOLS, span, summarize};
pub use transcript::{PromptMark, Transcript, keep_picture_in, prune_files, prune_pictures};

use serde::{Deserialize, Serialize};

/// One item of a conversation, as shown.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Entry {
    /// What the user asked; `images` pictures came with it; when (Unix
    /// seconds, 0 if not known).
    User { text: String, images: u32, at: u64, pictures: Vec<String> },
    /// Claude's answer, in Markdown; grows while it streams. `at`: when it
    /// started.
    Assistant { text: String, at: u64 },
    Thinking { text: String },
    Tool(Tool),
    /// Claude asks to use a tool; `answer` is the user's, once given.
    Permission(Permission),
    /// Something that happened: a finished turn, an interruption, an error.
    Notice { text: String, error: bool },
    /// One of Claude Code's own notes, never the user's words: a background
    /// task that finished, a wake-up, a command's output, what a compaction
    /// summed up. `detail` unfolds.
    Note(Note),
}

/// A note of Claude Code's in the conversation.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub kind: NoteKind,
    /// One line.
    pub text: String,
    /// More, folded: a report, an output, a summary.
    pub detail: String,
    /// Something went wrong (a task failed).
    pub error: bool,
    /// When (Unix seconds, 0 if not known).
    pub at: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoteKind {
    /// A background command, agent or watch finished, or a watch saw something.
    #[default]
    Task,
    /// A wake-up or a scheduled job started a turn.
    Wakeup,
    /// A command run in Claude Code's own interface (`/compact`, `! ls`) and its output.
    Command,
    /// The conversation was compacted: what it was summed up as.
    Compacted,
    /// A message from another Claude session or agent.
    Message,
    /// Context a hook added to a prompt.
    Hook,
}

/// A tool that goes on after its call: a sub-agent, or a command or watch
/// in the background.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub state: RunState,
    /// Started without waiting for it (`run_in_background`).
    pub background: bool,
    /// Claude Code's id for it (an agent's id), which its notifications name.
    pub task: String,
    /// The kind of agent (`general-purpose`, `Explore`); empty for commands.
    pub agent: String,
    /// When it started and ended, Unix seconds (0: not known, or not yet).
    pub started: u64,
    pub ended: u64,
    /// What it is doing now (its last step), or how it ended.
    pub last: String,
    /// Tool calls it made, and tokens it used, as last reported.
    pub tools: u32,
    pub tokens: u64,
    /// Its steps (an agent's tool calls and what it said), the latest
    /// `Run::MAX_STEPS`; `skipped` earlier ones are left out.
    pub steps: Vec<Entry>,
    pub skipped: u32,
    /// What it reported when it ended: an agent's result, a command's summary.
    pub report: String,
    /// Where Claude Code writes its output (background commands, agents).
    pub output: String,
}

impl Run {
    /// Steps kept of a sub-agent's run (the latest ones).
    pub const MAX_STEPS: usize = 60;
    /// A step's output beyond this is cut.
    pub const MAX_STEP_RESULT: usize = 1_500;

    pub fn running(&self) -> bool {
        self.state == RunState::Running
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunState {
    #[default]
    Running,
    Done,
    Failed,
    /// Stopped before it finished (by Claude, a time limit, or the process ending).
    Stopped,
}

/// Something set to happen later in a conversation, while its Claude Code
/// runs: a wake-up (ScheduleWakeup), a scheduled job (CronCreate), a watch
/// (Monitor).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Timer {
    pub kind: TimerKind,
    /// Claude Code's id: the job's, or the watch's task id.
    pub id: String,
    /// What it is for: the reason, the job's prompt, the watch's description.
    pub what: String,
    /// When it fires next (a watch: when it expires), Unix seconds (0: not known).
    pub at: u64,
    /// A recurring job's schedule, as Claude Code says it ("Every minute").
    pub every: String,
    /// What it tells Claude when it fires.
    pub prompt: String,
    /// The tool call that set it.
    pub tool: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimerKind {
    #[default]
    Wakeup,
    Job,
    Watch,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    pub id: String,
    pub name: String,
    /// One line: `Edit src/main.rs`, `$ cargo test`.
    pub summary: String,
    /// The input in full: a command, a pattern, the arguments.
    pub detail: String,
    /// The file it reads or changes, as given (absolute).
    pub file: Option<String>,
    /// For edits: the text replaced and the text put in.
    pub diff: Option<Diff>,
    /// What the tool returned, cut if long.
    pub result: Option<String>,
    pub error: bool,
    /// Pictures it returned (a screenshot Claude read), kept as files.
    #[serde(default)]
    pub pictures: Vec<String>,
    /// For a sub-agent, or a command or watch in the background: how it runs.
    #[serde(default)]
    pub run: Option<Box<Run>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Diff {
    pub old: String,
    pub new: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Permission {
    /// The request's id, to answer it.
    pub request: String,
    pub tool: String,
    pub summary: String,
    pub detail: String,
    pub diff: Option<Diff>,
    pub answer: Option<bool>,
    /// When Claude asks the user (its AskUserQuestion tool): the questions,
    /// and once answered, the answer to each.
    pub questions: Vec<Question>,
    pub answers: Vec<String>,
}

/// A question Claude asks, with the answers it offers (the user may also
/// write their own).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Question {
    pub question: String,
    /// A short label for it ("Auth method").
    pub header: String,
    pub options: Vec<QuestionOption>,
    /// Several options may be chosen.
    pub multi: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
}

/// The questions in an AskUserQuestion tool's input.
pub fn parse_questions(input: &serde_json::Value) -> Vec<Question> {
    let s = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    input
        .get("questions")
        .and_then(|q| q.as_array())
        .map(|qs| {
            qs.iter()
                .map(|q| Question {
                    question: s(q, "question"),
                    header: s(q, "header"),
                    multi: q.get("multiSelect").and_then(|m| m.as_bool()).unwrap_or(false),
                    options: q
                        .get("options")
                        .and_then(|o| o.as_array())
                        .map(|o| o.iter().map(|o| QuestionOption { label: s(o, "label"), description: s(o, "description") }).collect())
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The tool input that answers `questions` (Claude Code reads `answers`:
/// question → the chosen label, several joined by ", ", or the user's words).
pub fn answered_input(input: &serde_json::Value, questions: &[Question], answers: &[String]) -> serde_json::Value {
    let mut out = input.clone();
    let map: serde_json::Map<String, serde_json::Value> =
        questions.iter().zip(answers).map(|(q, a)| (q.question.clone(), serde_json::Value::String(a.clone()))).collect();
    if let Some(obj) = out.as_object_mut() {
        obj.insert("answers".into(), serde_json::Value::Object(map));
    }
    out
}

/// Where a conversation is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    /// No turn under way.
    #[default]
    Idle,
    /// Claude is working on a turn.
    Working,
    /// Claude waits for an answer to a permission request.
    Waiting,
}

/// A conversation's state and totals, sent with its entries.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub session: Option<String>,
    pub model: Option<String>,
    pub state: State,
    /// Dollars spent in this session's process.
    pub cost: f64,
    /// Tokens in the context at the last answer.
    pub context: u64,
    /// How many the model's context holds, as Claude Code last said (0: not yet).
    pub context_max: u64,
    /// When the turn under way started, Unix seconds.
    pub since: Option<u64>,
    /// Wake-ups, scheduled jobs and watches waiting, while Claude Code runs.
    #[serde(default)]
    pub timers: Vec<Timer>,
}

/// A usage limit's window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Window {
    /// How much is used, 0 to 1.
    pub used: f32,
    /// When it resets, Unix seconds.
    pub resets: u64,
}

/// The account's usage limits, as Claude Code last reported them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    pub five_hour: Option<Window>,
    pub week: Option<Window>,
    /// When Claude Code reported them, Unix seconds (0: not known).
    #[serde(default)]
    pub seen: u64,
}

/// A slash command or skill, for completion.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Command {
    pub name: String,
    pub description: String,
    pub hint: String,
}

/// A model to choose.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub value: String,
    pub name: String,
    pub description: String,
    pub efforts: Vec<String>,
}

/// What Claude Code's `initialize` reply tells: its commands and models.
pub fn parse_initialize(reply: &serde_json::Value) -> (Vec<Command>, Vec<Model>) {
    let s = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let commands = reply
        .get("commands")
        .and_then(|c| c.as_array())
        .map(|c| c.iter().map(|c| Command { name: s(c, "name"), description: s(c, "description"), hint: s(c, "argumentHint") }).collect())
        .unwrap_or_default();
    let models = reply
        .get("models")
        .and_then(|c| c.as_array())
        .map(|c| {
            c.iter()
                .map(|m| Model {
                    value: s(m, "value"),
                    name: s(m, "displayName"),
                    description: s(m, "description"),
                    efforts: m
                        .get("supportedEffortLevels")
                        .and_then(|e| e.as_array())
                        .map(|e| e.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    (commands, models)
}

/// The usage limits in a `rate_limit_event`.
/// Limits read back later: a window that has reset since is not known any
/// more (it started again from nothing), so it goes.
pub fn still_current(mut limits: Limits, now: u64) -> Option<Limits> {
    let keep = |w: Option<Window>| w.filter(|w| w.resets > now);
    limits.five_hour = keep(limits.five_hour);
    limits.week = keep(limits.week);
    (limits.five_hour.is_some() || limits.week.is_some()).then_some(limits)
}

pub fn parse_limits(event: &serde_json::Value) -> Option<Limits> {
    let windows = event.get("rate_limit_info")?.get("unifiedWindows")?;
    let window = |k: &str| {
        let w = windows.get(k)?;
        Some(Window { used: w.get("utilization")?.as_f64()? as f32, resets: w.get("resetsAt").and_then(|r| r.as_u64()).unwrap_or(0) })
    };
    Some(Limits { five_hour: window("five_hour"), week: window("seven_day"), seen: 0 })
}
