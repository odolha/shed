//! Topics a connector subscribes to, and their events. The first event of a
//! subscription is the whole thing; later ones are changes.

use serde::{Deserialize, Serialize};
use typeshare::typeshare;

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum Topic {
    /// An AI tab's conversation: `AiTranscript`, `AiInfo`.
    Ai(AiTopic),
    /// A terminal pane's screen: `Screen`.
    Screen(ScreenTopic),
    /// A project's files and Git changing on disk: `FilesChanged`,
    /// `GitChanged`.
    Project(ProjectTopic),
    /// Searching a project's files: `SearchHits` as they are found, then
    /// `SearchDone`. Unsubscribing stops it.
    Search(SearchTopic),
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiTopic {
    pub tab: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenTopic {
    pub pane: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTopic {
    pub tab: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchTopic {
    pub tab: String,
    pub query: String,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub whole_word: bool,
    /// Globs of files to search in, and to leave out.
    #[serde(default)]
    pub include: Option<String>,
    #[serde(default)]
    pub exclude: Option<String>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum Event {
    AiTranscript(AiTranscript),
    AiInfo(AiInfo),
    Screen(Screen),
    FilesChanged(FilesChanged),
    GitChanged,
    SearchHits(SearchHits),
    SearchDone(SearchDone),
}

// ---------------------------------------------------------------------- AI

/// A conversation's entries from `from` on (the first event: from 0, all of
/// them), replacing what the connector had from there, and its status.
#[typeshare]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiTranscript {
    pub from: u32,
    pub entries: Vec<AiEntry>,
    pub status: AiStatus,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum AiEntry {
    User(AiUser),
    /// Claude's answer, in Markdown; grows while it streams.
    Assistant(AiAssistant),
    Thinking(AiText),
    Tool(AiTool),
    /// Claude asks to use a tool, or asks the user questions.
    Permission(AiPermission),
    /// A finished turn, an interruption, an error.
    Notice(AiNotice),
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiUser {
    pub text: String,
    /// When, Unix seconds (0: not known).
    #[serde(default)]
    pub at: u32,
    /// Pictures sent with it, by name (`Call::AiPicture`).
    #[serde(default)]
    pub pictures: Vec<String>,
    /// How many pictures it had (some may not be kept).
    #[serde(default)]
    pub images: u32,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiAssistant {
    pub text: String,
    #[serde(default)]
    pub at: u32,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiText {
    pub text: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiTool {
    pub id: String,
    pub name: String,
    /// One line: `Edit src/main.rs`, `$ cargo test`.
    pub summary: String,
    /// The input in full.
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub file: Option<String>,
    /// For edits: the text replaced and the text put in.
    #[serde(default)]
    pub edit: Option<AiEdit>,
    /// What it returned, cut if long; `None` while it runs.
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub error: bool,
    #[serde(default)]
    pub pictures: Vec<String>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiEdit {
    pub old: String,
    pub new: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiPermission {
    /// To answer it (`Call::AiAnswer`, `Call::AiAnswerQuestions`).
    pub request: String,
    pub tool: String,
    pub summary: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub edit: Option<AiEdit>,
    /// The user's answer, once given.
    #[serde(default)]
    pub answer: Option<bool>,
    /// Questions Claude asks (its AskUserQuestion tool), and once answered,
    /// the answers.
    #[serde(default)]
    pub questions: Vec<AiQuestion>,
    #[serde(default)]
    pub answers: Vec<String>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiQuestion {
    pub question: String,
    pub header: String,
    pub options: Vec<AiOption>,
    /// Several options may be chosen.
    #[serde(default)]
    pub multi: bool,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiOption {
    pub label: String,
    #[serde(default)]
    pub description: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiNotice {
    pub text: String,
    #[serde(default)]
    pub error: bool,
}

#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiStatus {
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    pub state: crate::state::AiActivity,
    /// Dollars spent in this session's process.
    #[serde(default)]
    pub cost: f64,
    /// Tokens in the context, and how many it holds (0: not known).
    #[serde(default)]
    pub context: f64,
    #[serde(default)]
    pub context_max: f64,
    /// When the turn under way started.
    #[serde(default)]
    pub since: Option<u32>,
}

/// What Claude Code offers in this tab: slash commands and models.
#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiInfo {
    pub commands: Vec<AiCommand>,
    pub models: Vec<AiModel>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiCommand {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub hint: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiModel {
    pub value: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub efforts: Vec<String>,
}

// ---------------------------------------------------------------- terminals

/// A terminal's screen, or the lines of it that changed.
#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Screen {
    /// The whole screen (first event, a resize, a switch of screens): rows
    /// not in `lines` are blank. Otherwise only changed rows are given.
    pub full: bool,
    pub cols: u32,
    pub rows: u32,
    /// The program uses the alternate screen (vim, htop): show the grid as is
    /// rather than re-wrapping lines to the connector's width.
    #[serde(default)]
    pub alt: bool,
    pub lines: Vec<ScreenRow>,
    pub cursor: Cursor,
    #[serde(default)]
    pub title: Option<String>,
    /// How many lines of scrollback there are (`Call::Scrollback`).
    #[serde(default)]
    pub scrollback: u32,
    /// The program wants mouse events.
    #[serde(default)]
    pub mouse: bool,
    /// Lines scrolled off the top since the last event (so a connector
    /// showing scrollback can keep its place).
    #[serde(default)]
    pub scrolled: u32,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenRow {
    pub row: u32,
    pub line: ScreenLine,
}

#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenLine {
    pub spans: Vec<Span>,
    /// It goes on in the next row (it was wrapped, not ended).
    #[serde(default)]
    pub wraps: bool,
}

/// Text with one style. Wide characters take two cells; `text` has them once.
#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Span {
    pub text: String,
    /// `#rrggbb`, or a palette index as `0`…`255`; `None`: the default.
    #[serde(default)]
    pub fg: Option<String>,
    #[serde(default)]
    pub bg: Option<String>,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub dim: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub underline: bool,
    #[serde(default)]
    pub strike: bool,
    #[serde(default)]
    pub inverse: bool,
}

#[typeshare]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cursor {
    pub row: u32,
    pub col: u32,
    pub visible: bool,
}

// ----------------------------------------------------------------- projects

#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesChanged {
    /// Relative to the root; files and folders made, changed or removed.
    pub paths: Vec<String>,
}

/// A coloured range in a line of code: character positions, and what it is.
#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Highlight {
    pub start: u32,
    pub end: u32,
    /// `keyword`, `string`, `comment`, `function`, `type`, `number`,
    /// `constant`, `property`, `operator`, `tag`, `attribute`… each
    /// connector colours these by its theme.
    pub kind: String,
}

#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHits {
    pub path: String,
    pub hits: Vec<SearchHit>,
}

#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    /// 1-based.
    pub line: u32,
    pub text: String,
    /// Character ranges in `text` that matched: start, end, start, end…
    pub ranges: Vec<u32>,
}

#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchDone {
    /// Stopped at the limit: there may be more.
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub error: Option<String>,
}
