//! Calls on a workspace and their replies. What the GUI does, as requests.
//!
//! A call that changes something is refused with `Forbidden` when the
//! workspace allows viewing only (`Call::changes`). Paths in calls are
//! absolute, or relative to the tab's project root where the call says so.
//!
//! Positions in text (`FileMatch::hits`, `topics::Highlight`,
//! `topics::SearchHit::ranges`) count UTF-16 code units, as JavaScript and
//! Kotlin strings index: a character outside the Basic Multilingual Plane
//! (most emoji) counts two.

use serde::{Deserialize, Serialize};
use typeshare::typeshare;

use crate::state::{Divider, SplitDir, Tag};
use crate::topics::Highlight;

#[typeshare]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum Call {
    // ---- benches
    /// A new running bench. → `Bench`
    NewBench(NewBench),
    /// Loads a closed bench; `start` runs its startup too. → `Bench`
    LoadBench(LoadBench),
    /// Closes a running bench (it is saved; teardown runs). → `Done`
    CloseBench(BenchOnly),
    /// Deletes a closed bench's file. → `Done`
    DeleteBench(ClosedName),
    /// Saves a running bench now. → `Done`
    SaveBench(BenchOnly),
    RunStartup(BenchOnly),
    RunTeardown(BenchOnly),
    /// Running or closed. → `Done`
    RenameBench(RenameBench),
    SetBenchColor(SetBenchColor),
    SetBadge(SetBadge),
    /// The last lines of every tab's active pane, running and closed, for
    /// cards. → `Previews`
    Previews,

    // ---- tabs
    /// → `Tab`
    NewTab(NewTab),
    CloseTab(TabOnly),
    RenameTab(RenameTab),
    SetTags(SetTags),
    SetPrivate(SetPrivate),
    /// Into a bench (running), at an index.
    MoveTab(MoveTab),
    SetDivider(SetDivider),
    /// Makes it the bench's active tab (the GUI follows).
    FocusTab(TabOnly),
    /// Makes a remote tab drop its connection and join its shell again at
    /// once (the shell on the server keeps running); one whose connecting
    /// was stopped (Ctrl+C) starts it again. → `Done`
    ReconnectRemote(TabOnly),

    // ---- terminals
    /// → `Pane`
    Split(SplitPane),
    ClosePane(PaneOnly),
    FocusPane(PaneOnly),
    /// Keys, text or a paste, encoded for the program as the GUI would.
    Input(Input),
    /// Resizes the PTY to this connector's screen while it shows the pane;
    /// `ReleaseSize` (or going away) gives the size back.
    TakeSize(TakeSize),
    ReleaseSize(PaneOnly),
    /// Lines of scrollback, oldest first: `from` counts back from the
    /// screen's top (0: the line just above it). → `Lines`
    Scrollback(Scrollback),
    /// Runs a held startup now.
    StartNow(PaneOnly),
    /// Stops the AI prompt's request under way.
    CancelAsk(PaneOnly),

    // ---- logbook
    /// → `Log`
    LogSearch(LogSearch),

    // ---- AI tabs
    /// The folder's sessions, with draft sessions. → `Sessions`
    AiSessions(TabOnly),
    /// Shows and continues a session; `None`: a new one. → `Done`
    AiOpenSession(AiOpenSession),
    AiRenameSession(AiRenameSession),
    /// Deletes a Claude Code session and the sessions going on from it
    /// (their files go to Shed's trash, kept 30 days). Refused while another
    /// AI tab shows one of them; the tab, showing one, goes to a new
    /// session. → `Done`
    AiDeleteSession(AiDeleteSession),
    /// Sends a prompt (queued if a turn is under way). → `Done`
    AiSend(AiSend),
    AiStop(TabOnly),
    AiAnswer(AiAnswer),
    AiAnswerQuestions(AiAnswerQuestions),
    /// Edits an earlier prompt and sends it again (the conversation goes on
    /// from there).
    AiEdit(AiEdit),
    /// Goes back to before a prompt: the conversation, and the files too if
    /// `files`.
    AiRevert(AiRevert),
    /// Changes or sends a queued message.
    AiQueued(AiQueued),
    /// Model, mode, effort, follow, browser: only the fields given change.
    AiSettings(AiSettings),
    /// Sets the prompt's unsent text.
    AiSetUnsent(AiSetUnsent),
    AiAddDraft(AiAddDraft),
    AiRemoveDraft(AiRemoveDraft),
    /// A picture of the conversation (by the name in an entry). → `Picture`
    AiPicture(AiPictureName),
    /// The changes Claude made, for review. → `Review`
    AiReview(TabOnly),
    AiSetViewed(AiSetViewed),
    /// Asks, aside, for an explanation of changes. → `Explained`
    AiExplain(AiExplain),
    /// Starts Claude Code for the tab ahead of the first prompt.
    AiStart(TabOnly),
    /// Keeps a file on the workspace for a prompt to attach (by its path, in
    /// the text): at most `MAX_UPLOAD` bytes, in a folder of the tab's under
    /// Shed's state folder, deleted after a while. → `Text` (its path)
    AiUpload(AiUpload),

    // ---- projects (paths relative to the tab's root)
    /// A folder's entries. → `Folder`
    ListFolder(ProjectPath),
    /// Files whose names match, best first. → `Files`
    QuickOpen(QuickOpen),
    /// → `File`
    ReadFile(ProjectPath),
    /// Writes a file whole (light edits). → `Done`
    WriteFile(WriteFile),
    /// Makes a file, or a folder (a path ending in `/`).
    CreateEntry(ProjectPath),

    // ---- Git (in the tab's project or AI folder)
    /// → `GitStatus`
    GitStatus(TabOnly),
    /// A file's changes. → `Diff`
    GitDiff(GitDiff),
    GitStage(GitPaths),
    GitUnstage(GitPaths),
    /// Throws away changes (asks first, in the apps).
    GitDiscard(GitPaths),
    /// Stages, unstages or discards one hunk of `GitDiff`'s answer.
    GitHunk(GitHunk),
    /// → `Text` (the commit's hash)
    GitCommit(GitCommit),
    /// → `Commits`
    GitLog(GitLog),
    /// → `CommitFiles`
    GitCommitFiles(GitCommitRef),
    /// → `Branches`
    GitBranches(TabOnly),
    GitSwitch(GitBranch),
    GitCreateBranch(GitBranch),
    /// → `Text` (what git said)
    GitFetch(TabOnly),
    GitPull(TabOnly),
    GitPush(GitPush),

    // ---- the machine
    /// Folders under a path, to choose where a tab works. → `Folder`.
    /// Refused (`Forbidden`) by a view-only workspace, which makes no tabs.
    /// Kept for older apps: `Browse` says where it looked too.
    BrowseFolders(Browse),
    /// Folders under a path, and the path as an absolute one (`~` and `""`
    /// made into the home folder), to show and to go up from. → `Browsed`.
    /// Refused by a view-only workspace, as `BrowseFolders`.
    Browse(Browse),
}

impl Call {
    /// Whether the call changes something (refused when the workspace
    /// allows viewing only).
    pub fn changes(&self) -> bool {
        !matches!(
            self,
            Call::Previews
                | Call::Scrollback(_)
                | Call::LogSearch(_)
                | Call::AiSessions(_)
                | Call::AiPicture(_)
                | Call::AiReview(_)
                | Call::ListFolder(_)
                | Call::QuickOpen(_)
                | Call::ReadFile(_)
                | Call::GitStatus(_)
                | Call::GitDiff(_)
                | Call::GitLog(_)
                | Call::GitCommitFiles(_)
                | Call::GitBranches(_)
                | Call::BrowseFolders(_)
                | Call::Browse(_)
        )
    }
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum Reply {
    Done,
    /// A bench's id.
    Bench(String),
    /// A tab's id.
    Tab(String),
    /// A pane's id.
    Pane(String),
    Previews(Vec<Preview>),
    Lines(Vec<crate::topics::ScreenLine>),
    Log(Vec<LogEntry>),
    Sessions(Vec<AiSession>),
    Picture(Picture),
    Review(Review),
    Explained(Vec<Explanation>),
    Folder(Vec<FolderEntry>),
    Files(Vec<FileMatch>),
    File(FileContent),
    GitStatus(GitStatus),
    Diff(Diff),
    Commits(Vec<Commit>),
    CommitFiles(Vec<CommitFile>),
    Branches(Branches),
    Text(String),
    Browsed(Browsed),
}

// ---------------------------------------------------------------- arguments

/// A running bench by id, or a closed one by name.
#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum BenchRef {
    Running(String),
    Closed(String),
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchOnly {
    pub bench: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClosedName {
    pub name: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabOnly {
    pub tab: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaneOnly {
    pub pane: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewBench {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadBench {
    pub name: String,
    #[serde(default)]
    pub start: bool,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameBench {
    pub bench: BenchRef,
    pub name: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetBenchColor {
    pub bench: BenchRef,
    /// A named colour (`blue`, `coral`…) or `#rrggbb`.
    pub color: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetBadge {
    pub bench: BenchRef,
    #[serde(default)]
    pub badge: Option<String>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub pane: String,
    pub lines: Vec<String>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewTab {
    pub bench: BenchRef,
    pub kind: NewTabKind,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum NewTabKind {
    /// A shell, in a folder (the bench's by default).
    Terminal(OptionalFolder),
    /// A shell that runs a command.
    Command(CommandTab),
    Project(Folder),
    Ai(Folder),
    /// A shell on a server; `join` an existing one by id.
    Remote(RemoteTabArgs),
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionalFolder {
    #[serde(default)]
    pub folder: Option<String>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub folder: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandTab {
    pub folder: String,
    pub command: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTabArgs {
    pub host: String,
    #[serde(default)]
    pub join: Option<String>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameTab {
    pub bench: BenchRef,
    pub tab: String,
    /// `None`: back to the automatic title.
    #[serde(default)]
    pub name: Option<String>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetTags {
    pub bench: BenchRef,
    pub tab: String,
    pub tags: Vec<Tag>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetPrivate {
    pub bench: BenchRef,
    pub tab: String,
    pub private: bool,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveTab {
    pub tab: String,
    pub from: BenchRef,
    pub to: BenchRef,
    pub index: u32,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetDivider {
    pub tab: String,
    pub divider: Divider,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitPane {
    pub pane: String,
    pub dir: SplitDir,
    #[serde(default)]
    pub folder: Option<String>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub pane: String,
    pub input: Vec<InputItem>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum InputItem {
    /// Typed text (each character as a key press without modifiers).
    Text(String),
    /// A key with modifiers.
    Key(Key),
    /// Pasted text (bracketed when the program asks for it).
    Paste(String),
    /// The mouse, at a cell of the screen. Sent on to the program only
    /// while it asks for mouse events (`Screen::mouse`), encoded as it
    /// asked (X10, SGR); otherwise dropped.
    Mouse(Mouse),
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mouse {
    pub kind: MouseKind,
    /// The button pressed or released, or held while moving (`None`: no
    /// button held; a press or release without one is the left button).
    /// Not used for the wheel.
    #[serde(default)]
    pub button: Option<MouseButton>,
    /// 0-based, in cells of the screen (`Screen::cols` by `Screen::rows`).
    pub col: u32,
    pub row: u32,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub shift: bool,
}

#[typeshare]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MouseKind {
    Press,
    Release,
    Move,
    /// One notch of the wheel, up (away from the user) or down.
    WheelUp,
    WheelDown,
}

#[typeshare]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Key {
    /// A named key (`Enter`, `Tab`, `Backspace`, `Escape`, `Up`, `Down`,
    /// `Left`, `Right`, `Home`, `End`, `PageUp`, `PageDown`, `Insert`,
    /// `Delete`, `F1`…`F12`) or one character (`a`, `/`).
    pub key: String,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub shift: bool,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TakeSize {
    pub pane: String,
    pub cols: u32,
    pub rows: u32,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scrollback {
    pub pane: String,
    pub from: u32,
    pub count: u32,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogSearch {
    pub scope: LogScope,
    /// The tab (and its bench) the tab and bench scopes are about.
    #[serde(default)]
    pub tab: Option<String>,
    /// Words to find; may hold `tag:label`.
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub limit: u32,
}

#[typeshare]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LogScope {
    Tab,
    Bench,
    All,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub command: String,
    pub cwd: String,
    #[serde(default)]
    pub exit: Option<i32>,
    /// Unix seconds.
    pub started: u32,
    /// Seconds.
    pub duration: f64,
    #[serde(default)]
    pub bench: String,
    #[serde(default)]
    pub tab: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiOpenSession {
    pub tab: String,
    /// A Claude Code session id or a draft session's key; `None`: new.
    #[serde(default)]
    pub session: Option<String>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiRenameSession {
    pub tab: String,
    pub session: String,
    pub name: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiDeleteSession {
    pub tab: String,
    /// A Claude Code session id (not a draft session's key).
    pub session: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSession {
    /// Claude Code's session id, or a draft session's key.
    pub id: String,
    pub title: String,
    /// Unix seconds.
    pub modified: u32,
    #[serde(default)]
    pub branch: Option<String>,
    pub prompts: u32,
    /// The session it goes on from, and how deep it is listed under it.
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub depth: u32,
    /// A draft session (drafts only, nothing sent yet).
    #[serde(default)]
    pub draft: bool,
    /// How many drafts it has.
    #[serde(default)]
    pub drafts: u32,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSend {
    pub tab: String,
    pub text: String,
    #[serde(default)]
    pub pictures: Vec<Picture>,
}

/// A file for a prompt (`Call::AiUpload`).
#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiUpload {
    pub tab: String,
    /// Its name (made safe: a name, no folders).
    pub name: String,
    /// Base64.
    pub data: String,
}

/// The largest file `Call::AiUpload` takes, in bytes: as Base64 inside a
/// frame it stays under the nexus's frame limit.
#[typeshare]
pub const MAX_UPLOAD: u32 = 16_777_216;

/// A picture, whole.
#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Picture {
    /// `image/png` and the like.
    pub media_type: String,
    /// Base64.
    pub data: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiAnswer {
    pub tab: String,
    pub request: String,
    pub allow: bool,
    /// Allow it for the rest of the session.
    #[serde(default)]
    pub always: bool,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiAnswerQuestions {
    pub tab: String,
    pub request: String,
    /// One per question, in order; several choices joined by ", ".
    pub answers: Vec<String>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiEdit {
    pub tab: String,
    /// The prompt's entry index.
    pub entry: u32,
    pub text: String,
    #[serde(default)]
    pub pictures: Vec<Picture>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiRevert {
    pub tab: String,
    pub entry: u32,
    pub files: bool,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiQueued {
    pub tab: String,
    pub index: u32,
    /// The new text; empty removes it.
    pub text: String,
    /// Send it now (stopping the turn under way).
    #[serde(default)]
    pub send: bool,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSettings {
    pub tab: String,
    /// `Some("")`: back to the default model.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    /// `Some("")`: back to the default effort (the model's own).
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub follow: Option<bool>,
    #[serde(default)]
    pub browser: Option<bool>,
    #[serde(default)]
    pub add_dirs: Option<Vec<String>>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSetUnsent {
    pub tab: String,
    pub text: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiAddDraft {
    pub tab: String,
    pub text: String,
    /// The session it belongs to; `None`: the one shown (or a new draft
    /// session when none is).
    #[serde(default)]
    pub session: Option<String>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiRemoveDraft {
    pub tab: String,
    /// Its index in the tab's `drafts`, and its text (so a stale index
    /// removes nothing).
    pub index: u32,
    pub text: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiPictureName {
    pub tab: String,
    pub name: String,
}

/// What Claude changed, by repository.
#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub repos: Vec<ReviewRepo>,
}

#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewRepo {
    pub root: String,
    /// Commits made since the session started.
    #[serde(default)]
    pub commits: Vec<Commit>,
    pub changes: Vec<ReviewChange>,
}

#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewChange {
    /// Stable while the change is the same: what `AiSetViewed` and
    /// `AiExplain` name.
    pub key: String,
    /// Relative to the repository.
    pub path: String,
    /// `uncommitted`, or the commit it is in.
    pub layer: String,
    /// `M`, `A`, `D`, `R`, `?`.
    pub status: String,
    pub viewed: bool,
    #[serde(default)]
    pub explanation: Option<String>,
    pub diff: Diff,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSetViewed {
    pub tab: String,
    pub key: String,
    pub viewed: bool,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiExplain {
    pub tab: String,
    pub keys: Vec<String>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Explanation {
    pub key: String,
    pub text: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPath {
    /// A project or AI tab.
    pub tab: String,
    /// Relative to its root; `""` is the root.
    pub path: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderEntry {
    pub name: String,
    pub dir: bool,
    /// Left out by the tab's excludes or Git's ignore rules (shown dimmed).
    #[serde(default)]
    pub ignored: bool,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickOpen {
    pub tab: String,
    pub query: String,
    #[serde(default)]
    pub limit: u32,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMatch {
    pub path: String,
    /// Positions in `path` that matched, to highlight, in UTF-16 code units.
    pub hits: Vec<u32>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileContent {
    pub path: String,
    pub kind: FileKind,
    /// For text: the whole of it.
    #[serde(default)]
    pub text: String,
    /// The language's name, if known.
    #[serde(default)]
    pub lang: Option<String>,
    /// Per line, its coloured ranges.
    #[serde(default)]
    pub highlights: Vec<Vec<Highlight>>,
    /// For a picture: the picture.
    #[serde(default)]
    pub picture: Option<Picture>,
    /// Bytes.
    pub size: f64,
}

#[typeshare]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileKind {
    Text,
    Picture,
    Binary,
    TooLarge,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteFile {
    pub tab: String,
    pub path: String,
    pub text: String,
}

#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    /// Not a repository: everything else is empty.
    pub repo: bool,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub upstream: Option<String>,
    #[serde(default)]
    pub ahead: u32,
    #[serde(default)]
    pub behind: u32,
    /// A merge, rebase or the like under way.
    #[serde(default)]
    pub operation: Option<String>,
    pub files: Vec<GitFile>,
    /// Commits not pushed yet.
    #[serde(default)]
    pub outgoing: Vec<Commit>,
    /// The repository's folder, absolute (on the workspace): files' paths
    /// are relative to it. Missing from older workspaces.
    #[serde(default)]
    pub root: Option<String>,
    /// The commit checked out (its hash, shortened), if any.
    #[serde(default)]
    pub head: Option<String>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFile {
    pub path: String,
    /// Index and work-tree status letters, as `git status --porcelain`.
    pub staged: String,
    pub unstaged: String,
    #[serde(default)]
    pub conflict: bool,
    /// Renamed from.
    #[serde(default)]
    pub from: Option<String>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDiff {
    pub tab: String,
    pub path: String,
    pub source: DiffSource,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum DiffSource {
    /// Index against HEAD.
    Staged,
    /// Work tree against the index.
    Unstaged,
    /// What a commit changed.
    Commit(String),
}

#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diff {
    pub hunks: Vec<Hunk>,
    /// A picture changed: both sides, when they are pictures.
    #[serde(default)]
    pub old_picture: Option<Picture>,
    #[serde(default)]
    pub new_picture: Option<Picture>,
    /// Binary, not shown.
    #[serde(default)]
    pub binary: bool,
}

#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    /// `@@ -12,7 +12,9 @@`.
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub text: String,
    /// Line numbers, old and new side.
    #[serde(default)]
    pub old: Option<u32>,
    #[serde(default)]
    pub new: Option<u32>,
}

#[typeshare]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitPaths {
    pub tab: String,
    pub paths: Vec<String>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHunk {
    pub tab: String,
    pub path: String,
    pub source: DiffSource,
    /// The hunk's index in the diff, and its header (so a stale index does
    /// nothing).
    pub hunk: u32,
    pub header: String,
    pub action: HunkAction,
}

#[typeshare]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HunkAction {
    Stage,
    Unstage,
    Discard,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommit {
    pub tab: String,
    pub message: String,
    #[serde(default)]
    pub amend: bool,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitLog {
    pub tab: String,
    /// One file's history; `None`: the repository's.
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub limit: u32,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    /// Unix seconds.
    pub time: u32,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitRef {
    pub tab: String,
    pub hash: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitFile {
    pub status: String,
    pub path: String,
}

#[typeshare]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Branches {
    #[serde(default)]
    pub current: Option<String>,
    pub local: Vec<String>,
    pub remote: Vec<String>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitBranch {
    pub tab: String,
    pub branch: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitPush {
    pub tab: String,
    #[serde(default)]
    pub force: bool,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Browse {
    /// Absolute, or `~/…`; `""`: home.
    pub path: String,
}

/// A folder's folders (`Call::Browse`).
#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Browsed {
    /// Where it looked, absolute (`~` expanded, no `..`).
    pub path: String,
    /// The folder above, if any.
    #[serde(default)]
    pub parent: Option<String>,
    /// Its folders (hidden ones left out), by name.
    pub entries: Vec<FolderEntry>,
}
