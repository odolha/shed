//! Claude Code, as Shed shows it: a conversation made from the program's
//! stream of JSON events (`--output-format stream-json`) or from a saved
//! session's file, and the list of a folder's sessions. No UI and no
//! processes: the keeper runs `claude`, windows draw what this builds.

pub mod markdown;
pub mod sessions;
mod summary;
mod transcript;

pub use sessions::{SessionInfo, list_sessions, load_session};
pub use summary::summarize;
pub use transcript::{PromptMark, Transcript};

use serde::{Deserialize, Serialize};

/// One item of a conversation, as shown.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Entry {
    /// What the user asked; `images` pictures came with it.
    User { text: String, images: u32 },
    /// Claude's answer, in Markdown; grows while it streams.
    Assistant { text: String },
    Thinking { text: String },
    Tool(Tool),
    /// Claude asks to use a tool; `answer` is the user's, once given.
    Permission(Permission),
    /// Something that happened: a finished turn, an interruption, an error.
    Notice { text: String, error: bool },
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
    /// When the turn under way started, Unix seconds.
    pub since: Option<u64>,
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
pub fn parse_limits(event: &serde_json::Value) -> Option<Limits> {
    let windows = event.get("rate_limit_info")?.get("unifiedWindows")?;
    let window = |k: &str| {
        let w = windows.get(k)?;
        Some(Window { used: w.get("utilization")?.as_f64()? as f32, resets: w.get("resetsAt").and_then(|r| r.as_u64()).unwrap_or(0) })
    };
    Some(Limits { five_hour: window("five_hour"), week: window("seven_day") })
}
