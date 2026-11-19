//! A workspace's state, as connectors see it: its benches (running and
//! closed), their tabs and panes, and the account's usage limits. The link
//! sends all of it whenever it changes; it is small.
//!
//! Ids are strings (the keeper's UUIDs). Colours are `#rrggbb`. Private tabs
//! keep their place and kind, but nothing of what is in them: no name, title,
//! folder, panes' text or commands.

use serde::{Deserialize, Serialize};
use typeshare::typeshare;

#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceState {
    /// The machine's host name.
    pub host: String,
    /// Running benches, in the order the user arranged them.
    pub benches: Vec<Bench>,
    /// Saved benches that are not running, by name.
    pub closed: Vec<ClosedBench>,
    /// The account's usage limits, as Claude Code last reported them here.
    #[serde(default)]
    pub limits: Option<Limits>,
    /// The user's home folder, to show paths as `~/…`.
    #[serde(default)]
    pub home: String,
}

#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bench {
    pub id: String,
    pub name: String,
    pub color: String,
    #[serde(default)]
    pub badge: Option<String>,
    /// Where new tabs open.
    pub cwd: String,
    pub tabs: Vec<Tab>,
    pub active_tab: String,
    /// A window shows it on the workspace.
    #[serde(default)]
    pub shown: bool,
    /// Being closed: its teardown runs.
    #[serde(default)]
    pub closing: bool,
    /// Its startup ran since it opened.
    #[serde(default)]
    pub started: bool,
    #[serde(default)]
    pub links: Vec<Link>,
}

#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClosedBench {
    pub name: String,
    pub color: String,
    #[serde(default)]
    pub badge: Option<String>,
    pub cwd: String,
    /// Its tabs as they will load (panes without a screen).
    pub tabs: Vec<Tab>,
    /// When it was saved, Unix seconds.
    pub saved: u32,
    #[serde(default)]
    pub links: Vec<Link>,
}

#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    pub name: String,
    pub url: String,
}

#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tab {
    pub id: String,
    /// The name the user gave it, if any.
    #[serde(default)]
    pub name: Option<String>,
    /// What to call it: the name, else what Shed shows (the program, the
    /// folder, the project's or session's name).
    pub title: String,
    #[serde(default)]
    pub tags: Vec<Tag>,
    #[serde(default)]
    pub private: bool,
    pub kind: TabKind,
    /// Terminal and remote tabs: the panes, and how they are split.
    #[serde(default)]
    pub panes: Vec<Pane>,
    #[serde(default)]
    pub layout: Option<Layout>,
    #[serde(default)]
    pub active_pane: Option<String>,
    /// Starts a group on the tab bar.
    #[serde(default)]
    pub divider: Divider,
}

#[typeshare]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Divider {
    #[default]
    None,
    Space,
    Row,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub label: String,
    pub color: String,
    /// Tints the whole tab, as a warning.
    #[serde(default)]
    pub tint: bool,
}

#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum TabKind {
    #[default]
    Terminal,
    Project(ProjectTab),
    Ai(AiTab),
    Remote(RemoteTab),
}

#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTab {
    pub root: String,
    /// Files open in the GUI, relative to the root.
    #[serde(default)]
    pub open: Vec<String>,
    #[serde(default)]
    pub active: Option<u32>,
    #[serde(default)]
    pub links: Vec<Link>,
}

#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiTab {
    pub folder: String,
    #[serde(default)]
    pub add_dirs: Vec<String>,
    /// The session shown, if any (a new one otherwise).
    #[serde(default)]
    pub session: Option<String>,
    /// The draft session shown, when `session` is none.
    #[serde(default)]
    pub draft_session: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// `default`, `acceptEdits`, `plan`.
    pub mode: String,
    #[serde(default)]
    pub effort: Option<String>,
    pub activity: AiActivity,
    /// Messages kept to send later, each with its session.
    #[serde(default)]
    pub drafts: Vec<AiDraft>,
    /// Messages waiting for the turn under way to end.
    #[serde(default)]
    pub queued: Vec<String>,
    /// What is written in the prompt and not sent.
    #[serde(default)]
    pub unsent: String,
    /// It may use the browser (Claude in Chrome).
    #[serde(default)]
    pub browser: bool,
    #[serde(default)]
    pub follow: bool,
}

#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiDraft {
    pub session: String,
    pub text: String,
}

#[typeshare]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AiActivity {
    #[default]
    Idle,
    Working,
    /// Waits for the user to allow something or answer.
    Waiting,
}

#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTab {
    pub host: String,
    pub id: String,
}

#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pane {
    pub id: String,
    pub cwd: String,
    /// The size the PTY has now.
    pub cols: u32,
    pub rows: u32,
    /// The title the program set.
    #[serde(default)]
    pub title: Option<String>,
    /// The program in the foreground, when it is not the shell: `vim notes.md`.
    #[serde(default)]
    pub tool: Option<String>,
    /// A command typed by a bench load, waiting for Enter.
    #[serde(default)]
    pub waiting: Option<String>,
    /// Its startup is held until other panes are ready.
    #[serde(default)]
    pub held: bool,
    /// A request to the AI prompt under way, or why it failed.
    #[serde(default)]
    pub asking: Option<String>,
    /// A connector holds its size (see `Call::TakeSize`).
    #[serde(default)]
    pub size_held: bool,
}

/// How a tab's panes are split.
#[typeshare]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum Layout {
    Pane(String),
    Split(Split),
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Split {
    pub dir: SplitDir,
    /// The first part's share, 0 to 1.
    pub ratio: f32,
    pub first: Box<Layout>,
    pub second: Box<Layout>,
}

#[typeshare]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SplitDir {
    /// Side by side.
    Right,
    /// One above the other.
    Down,
}

#[typeshare]
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    #[serde(default)]
    pub five_hour: Option<LimitWindow>,
    #[serde(default)]
    pub week: Option<LimitWindow>,
    /// When Claude Code reported them.
    #[serde(default)]
    pub seen: u32,
}

#[typeshare]
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitWindow {
    /// 0 to 1.
    pub used: f32,
    /// When it resets.
    pub resets: u32,
}
