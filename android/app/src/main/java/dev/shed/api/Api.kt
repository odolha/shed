// Generated from crates/shed-api by shed-codegen (typeshare): do not edit.
// Run `cargo run -p shed-codegen` after changing shed-api.
//
// The JSON is serde's. Read and write it with:
//
//     val ShedJson = Json {
//         classDiscriminator = "type"  // the default, and serde's tag
//         ignoreUnknownKeys = true     // newer peers add fields
//         coerceInputValues = true     // and enum values (below)
//         serializersModule = …        // and variants (below)
//     }
//
// - Enums with data are sealed classes: {"type": "V", "content": …} is
//   `data class V(val content: T)`, and {"type": "V"} (no data) an `object V`.
//   Encode one through its base type (`ShedJson.encodeToString<Call>(call)`),
//   or the "type" is not written.
// - Enums without data are enum classes over camelCase strings.
// - Fields serde gives a default are nullable here, null when left out. Keep
//   `encodeDefaults` off (the default): serde reads a missing field as its
//   default, but null only for an Option, not for a String, Boolean or List.
//   (With `encodeDefaults = true`, also set `explicitNulls = false`.)
// - u32 is UInt; f32 Float; f64 Double.
// - Newer peers may send what this version does not know. `AiEntry` and
//   `Event` have an `Unknown` variant for that (never sent): make it their
//   default deserializer, and set `coerceInputValues = true`
//   so an unknown value of an enum without data, in a field with a default,
//   reads as null. Elsewhere an unknown variant or value fails to decode:
//
//     serializersModule = SerializersModule {
//         polymorphicDefaultDeserializer(AiEntry::class) { AiEntry.Unknown.serializer() }
//         polymorphicDefaultDeserializer(Event::class) { Event.Unknown.serializer() }
//     }

package dev.shed.api

import kotlinx.serialization.Serializable
import kotlinx.serialization.SerialName

@Serializable
data class AiAddDraft (
	val tab: String,
	val text: String,
	/// The session it belongs to; `None`: the one shown (or a new draft
	/// session when none is).
	val session: String? = null
)

@Serializable
data class AiAnswer (
	val tab: String,
	val request: String,
	val allow: Boolean,
	/// Allow it for the rest of the session.
	val always: Boolean? = null
)

@Serializable
data class AiAnswerQuestions (
	val tab: String,
	val request: String,
	/// One per question, in order; several choices joined by ", ".
	val answers: List<String>
)

@Serializable
data class AiAssistant (
	val text: String,
	val at: UInt? = null
)

@Serializable
data class AiCommand (
	val name: String,
	val description: String? = null,
	val hint: String? = null
)

@Serializable
data class AiDeleteSession (
	val tab: String,
	/// A Claude Code session id (not a draft session's key).
	val session: String
)

@Serializable
data class AiDraft (
	val session: String,
	val text: String
)

/// A picture, whole.
@Serializable
data class Picture (
	/// `image/png` and the like.
	val mediaType: String,
	/// Base64.
	val data: String
)

@Serializable
data class AiEdit (
	val tab: String,
	/// The prompt's entry index.
	val entry: UInt,
	val text: String,
	val pictures: List<Picture>? = null
)

@Serializable
sealed class AiEntry {
	@Serializable
	@SerialName("User")
	data class User(val content: AiUser): AiEntry()
	/// Claude's answer, in Markdown; grows while it streams.
	@Serializable
	@SerialName("Assistant")
	data class Assistant(val content: AiAssistant): AiEntry()
	@Serializable
	@SerialName("Thinking")
	data class Thinking(val content: AiText): AiEntry()
	@Serializable
	@SerialName("Tool")
	data class Tool(val content: AiTool): AiEntry()
	/// Claude asks to use a tool, or asks the user questions.
	@Serializable
	@SerialName("Permission")
	data class Permission(val content: AiPermission): AiEntry()
	/// A finished turn, an interruption, an error.
	@Serializable
	@SerialName("Notice")
	data class Notice(val content: AiNotice): AiEntry()
	/// One of Claude Code's own notes, never the user's words: a background
	/// task that finished, a wake-up, a command's output, a compaction.
	@Serializable
	@SerialName("Note")
	data class Note(val content: AiNote): AiEntry()
	/// Never sent: connectors read an entry they do not know (a newer
	/// workspace's) as this; it keeps its place and shows as nothing.
	@Serializable
	@SerialName("Unknown")
	object Unknown: AiEntry()
}

@Serializable
data class AiEntryAt (
	val index: UInt,
	val entry: AiEntry
)

@Serializable
enum class AiActivity(val string: String) {
	@SerialName("idle")
	Idle("idle"),
	@SerialName("working")
	Working("working"),
	/// Waits for the user to allow something or answer.
	@SerialName("waiting")
	Waiting("waiting"),
}

@Serializable
enum class AiTimerKind(val string: String) {
	@SerialName("wakeup")
	Wakeup("wakeup"),
	@SerialName("job")
	Job("job"),
	@SerialName("watch")
	Watch("watch"),
}

/// Something set to happen later while the tab's Claude Code runs: a
/// wake-up, a scheduled job, a watch.
@Serializable
data class AiTimer (
	val kind: AiTimerKind? = null,
	/// What it is for: the reason, the job's prompt, the watch's description.
	val what: String,
	/// When it fires next (a watch: when it ends), Unix seconds (0: not known).
	val at: UInt? = null,
	/// It repeats: on `every` ("Every minute").
	val recurring: Boolean? = null,
	val every: String? = null,
	/// What it tells Claude when it fires.
	val prompt: String? = null,
	/// The tool call (`AiTool::id`) that set it.
	val tool: String? = null
)

@Serializable
data class AiStatus (
	val session: String? = null,
	val model: String? = null,
	val state: AiActivity,
	/// Dollars spent in this session's process.
	val cost: Double? = null,
	/// Tokens in the context, and how many it holds (0: not known).
	val context: Double? = null,
	val contextMax: Double? = null,
	/// When the turn under way started.
	val since: UInt? = null,
	/// Wake-ups, scheduled jobs and watches waiting.
	val timers: List<AiTimer>? = null
)

/// Entries that changed in place, by index; the others stay as they are.
@Serializable
data class AiEntriesChanged (
	val entries: List<AiEntryAt>,
	val status: AiStatus
)

@Serializable
data class AiExplain (
	val tab: String,
	val keys: List<String>
)

@Serializable
data class AiModel (
	val value: String,
	val name: String,
	val description: String? = null,
	val efforts: List<String>? = null
)

/// What Claude Code offers in this tab: slash commands and models.
@Serializable
data class AiInfo (
	val commands: List<AiCommand>,
	val models: List<AiModel>
)

@Serializable
enum class AiNoteKind(val string: String) {
	/// A background command, agent or watch finished, or a watch saw something.
	@SerialName("task")
	Task("task"),
	/// A wake-up or a scheduled job started a turn.
	@SerialName("wakeup")
	Wakeup("wakeup"),
	/// A command run in Claude Code's own interface (`/compact`, `! ls`) and its output.
	@SerialName("command")
	Command("command"),
	/// The conversation was compacted: what it was summed up as.
	@SerialName("compacted")
	Compacted("compacted"),
	/// A message from another Claude session or agent.
	@SerialName("message")
	Message("message"),
	/// Context a hook added to a prompt.
	@SerialName("hook")
	Hook("hook"),
}

@Serializable
data class AiNote (
	/// Missing when this version does not know the kind: shown as `task`.
	val kind: AiNoteKind? = null,
	/// One line.
	val title: String,
	/// More, folded: a report, an output, a summary (Markdown).
	val detail: String? = null,
	/// Something went wrong (a task failed).
	val error: Boolean? = null,
	val at: UInt? = null
)

@Serializable
data class AiNotice (
	val text: String,
	val error: Boolean? = null
)

@Serializable
data class AiOpenSession (
	val tab: String,
	/// A Claude Code session id or a draft session's key; `None`: new.
	val session: String? = null
)

@Serializable
data class AiOption (
	val label: String,
	val description: String? = null
)

/// An edit's text: what it replaced and what it put in. (Not `AiEdit`: that
/// is the call, and the generated types share one namespace.)
@Serializable
data class AiTextEdit (
	val old: String,
	val new: String
)

@Serializable
data class AiQuestion (
	val question: String,
	val header: String,
	val options: List<AiOption>,
	/// Several options may be chosen.
	val multi: Boolean? = null
)

@Serializable
data class AiPermission (
	/// To answer it (`Call::AiAnswer`, `Call::AiAnswerQuestions`).
	val request: String,
	val tool: String,
	val summary: String,
	val detail: String? = null,
	val edit: AiTextEdit? = null,
	/// The user's answer, once given.
	val answer: Boolean? = null,
	/// Questions Claude asks (its AskUserQuestion tool), and once answered,
	/// the answers.
	val questions: List<AiQuestion>? = null,
	val answers: List<String>? = null
)

@Serializable
data class AiPictureName (
	val tab: String,
	val name: String
)

@Serializable
data class AiQueued (
	val tab: String,
	val index: UInt,
	/// The new text; empty removes it.
	val text: String,
	/// Send it now (stopping the turn under way).
	val send: Boolean? = null
)

@Serializable
data class AiRemoveDraft (
	val tab: String,
	/// Its index in the tab's `drafts`, and its text (so a stale index
	/// removes nothing).
	val index: UInt,
	val text: String
)

@Serializable
data class AiRenameSession (
	val tab: String,
	val session: String,
	val name: String
)

@Serializable
data class AiRevert (
	val tab: String,
	val entry: UInt,
	val files: Boolean
)

@Serializable
enum class AiRunState(val string: String) {
	@SerialName("running")
	Running("running"),
	@SerialName("done")
	Done("done"),
	@SerialName("failed")
	Failed("failed"),
	/// Stopped before it finished.
	@SerialName("stopped")
	Stopped("stopped"),
}

/// A tool that goes on after its call: a sub-agent, or a command or watch
/// in the background.
@Serializable
data class AiRun (
	val state: AiRunState? = null,
	/// Started without waiting for it.
	val background: Boolean? = null,
	/// The kind of agent (`general-purpose`, `Explore`); empty for commands.
	val agent: String? = null,
	/// When it started and ended, Unix seconds (0: not known, or not yet).
	val started: UInt? = null,
	val ended: UInt? = null,
	/// What it does now (its last step), or how it ended.
	val last: String? = null,
	/// Tool calls it made, and tokens it used, as last reported.
	val tools: UInt? = null,
	val tokens: Double? = null,
	/// Its latest steps (tool calls, what it said); `skipped` earlier ones
	/// are left out. Steps have no runs of their own.
	val steps: List<AiEntry>? = null,
	val skipped: UInt? = null,
	/// What it reported when it ended (Markdown).
	val report: String? = null
)

@Serializable
data class AiSend (
	val tab: String,
	val text: String,
	val pictures: List<Picture>? = null
)

@Serializable
data class AiSession (
	/// Claude Code's session id, or a draft session's key.
	val id: String,
	val title: String,
	/// Unix seconds.
	val modified: UInt,
	val branch: String? = null,
	val prompts: UInt,
	/// The session it goes on from, and how deep it is listed under it.
	val parent: String? = null,
	val depth: UInt? = null,
	/// A draft session (drafts only, nothing sent yet).
	val draft: Boolean? = null,
	/// How many drafts it has.
	val drafts: UInt? = null
)

@Serializable
data class AiSetUnsent (
	val tab: String,
	val text: String
)

@Serializable
data class AiSetViewed (
	val tab: String,
	val key: String,
	val viewed: Boolean
)

@Serializable
data class AiSettings (
	val tab: String,
	/// `Some("")`: back to the default model.
	val model: String? = null,
	val mode: String? = null,
	/// `Some("")`: back to the default effort (the model's own).
	val effort: String? = null,
	val follow: Boolean? = null,
	val browser: Boolean? = null,
	val addDirs: List<String>? = null
)

@Serializable
data class AiTab (
	val folder: String,
	val addDirs: List<String>? = null,
	/// The session shown, if any (a new one otherwise).
	val session: String? = null,
	/// The draft session shown, when `session` is none.
	val draftSession: String? = null,
	val model: String? = null,
	/// `default`, `acceptEdits`, `plan`.
	val mode: String,
	val effort: String? = null,
	val activity: AiActivity,
	/// Messages kept to send later, each with its session.
	val drafts: List<AiDraft>? = null,
	/// Messages waiting for the turn under way to end.
	val queued: List<String>? = null,
	/// What is written in the prompt and not sent.
	val unsent: String? = null,
	/// It may use the browser (Claude in Chrome).
	val browser: Boolean? = null,
	val follow: Boolean? = null
)

@Serializable
data class AiText (
	val text: String
)

@Serializable
data class AiTool (
	val id: String,
	val name: String,
	/// One line: `Edit src/main.rs`, `$ cargo test`.
	val summary: String,
	/// The input in full.
	val detail: String? = null,
	val file: String? = null,
	/// For edits: the text replaced and the text put in.
	val edit: AiTextEdit? = null,
	/// What it returned, cut if long; `None` while it runs.
	val result: String? = null,
	val error: Boolean? = null,
	val pictures: List<String>? = null,
	/// For a sub-agent, or a command or watch in the background: how it runs.
	val run: AiRun? = null
)

@Serializable
data class AiTopic (
	val tab: String
)

/// A conversation's entries from `from` on (the first event: from 0, all of
/// them), replacing what the connector had from there, and its status.
@Serializable
data class AiTranscript (
	val from: UInt,
	val entries: List<AiEntry>,
	val status: AiStatus
)

/// A file for a prompt (`Call::AiUpload`).
@Serializable
data class AiUpload (
	val tab: String,
	/// Its name (made safe: a name, no folders).
	val name: String,
	/// Base64.
	val data: String
)

@Serializable
data class AiUser (
	val text: String,
	/// When, Unix seconds (0: not known).
	val at: UInt? = null,
	/// Pictures sent with it, by name (`Call::AiPicture`).
	val pictures: List<String>? = null,
	/// How many pictures it had (some may not be kept).
	val images: UInt? = null
)

@Serializable
enum class ErrorCode(val string: String) {
	/// Not signed in, or the token is no longer valid.
	@SerialName("unauthorized")
	Unauthorized("unauthorized"),
	/// Signed in, but this is not allowed (a workspace in view-only mode).
	@SerialName("forbidden")
	Forbidden("forbidden"),
	/// The workspace, bench, tab, pane or file is not there.
	@SerialName("notFound")
	NotFound("notFound"),
	/// The workspace is not connected to the nexus.
	@SerialName("offline")
	Offline("offline"),
	/// The request does not make sense (a bad argument).
	@SerialName("invalid")
	Invalid("invalid"),
	/// The workspace or nexus does not know this call (another version).
	@SerialName("unsupported")
	Unsupported("unsupported"),
	/// Too many attempts; try again later.
	@SerialName("rateLimited")
	RateLimited("rateLimited"),
	/// The wrong one-time code.
	@SerialName("wrongCode")
	WrongCode("wrongCode"),
	/// It took too long.
	@SerialName("timeout")
	Timeout("timeout"),
	/// Something failed on the way; `message` says what.
	@SerialName("failed")
	Failed("failed"),
}

@Serializable
data class ApiError (
	val code: ErrorCode,
	/// For people, in English; the apps show their own text for the code and
	/// this as detail.
	val message: String? = null
)

@Serializable
data class AuditEntry (
	val at: UInt,
	/// `login`, `login-failed`, `pair`, `revoke`, `password`, `tunnel-open`…
	val what: String,
	val device: String? = null,
	val address: String? = null,
	val country: String? = null,
	val detail: String? = null
)

@Serializable
data class Tag (
	val label: String,
	val color: String,
	/// Tints the whole tab, as a warning.
	val tint: Boolean? = null
)

@Serializable
sealed class TabKind {
	@Serializable
	@SerialName("Terminal")
	object Terminal: TabKind()
	@Serializable
	@SerialName("Project")
	data class Project(val content: ProjectTab): TabKind()
	@Serializable
	@SerialName("Ai")
	data class Ai(val content: AiTab): TabKind()
	@Serializable
	@SerialName("Remote")
	data class Remote(val content: RemoteTab): TabKind()
}

@Serializable
data class Pane (
	val id: String,
	val cwd: String,
	/// The size the PTY has now.
	val cols: UInt,
	val rows: UInt,
	/// The title the program set.
	val title: String? = null,
	/// The program in the foreground, when it is not the shell: `vim notes.md`.
	val tool: String? = null,
	/// A command typed by a bench load, waiting for Enter.
	val waiting: String? = null,
	/// Its startup is held until other panes are ready.
	val held: Boolean? = null,
	/// A request to the AI prompt under way, or why it failed.
	val asking: String? = null,
	/// A connector holds its size (see `Call::TakeSize`).
	val sizeHeld: Boolean? = null
)

/// How a tab's panes are split.
@Serializable
sealed class Layout {
	@Serializable
	@SerialName("Pane")
	data class Pane(val content: String): Layout()
	@Serializable
	@SerialName("Split")
	data class Split(val content: dev.shed.api.Split): Layout()
}

@Serializable
enum class Divider(val string: String) {
	@SerialName("none")
	None("none"),
	@SerialName("space")
	Space("space"),
	@SerialName("row")
	Row("row"),
}

@Serializable
data class Tab (
	val id: String,
	/// The name the user gave it, if any.
	val name: String? = null,
	/// What to call it: the name, else what Shed shows (the program, the
	/// folder, the project's or session's name).
	val title: String,
	val tags: List<Tag>? = null,
	val private: Boolean? = null,
	val kind: TabKind,
	/// Terminal and remote tabs: the panes, and how they are split.
	val panes: List<Pane>? = null,
	val layout: Layout? = null,
	val activePane: String? = null,
	/// Starts a group on the tab bar.
	val divider: Divider? = null
)

@Serializable
data class Link (
	val name: String,
	val url: String
)

@Serializable
data class Bench (
	val id: String,
	val name: String,
	val color: String,
	val badge: String? = null,
	/// Where new tabs open.
	val cwd: String,
	val tabs: List<Tab>,
	val activeTab: String,
	/// A window shows it on the workspace.
	val shown: Boolean? = null,
	/// Being closed: its teardown runs.
	val closing: Boolean? = null,
	/// Its startup ran since it opened.
	val started: Boolean? = null,
	val links: List<Link>? = null
)

@Serializable
data class BenchOnly (
	val bench: String
)

@Serializable
data class Branches (
	val current: String? = null,
	val local: List<String>,
	val remote: List<String>
)

@Serializable
data class Browse (
	/// Absolute, or `~/…`; `""`: home.
	val path: String
)

@Serializable
data class FolderEntry (
	val name: String,
	val dir: Boolean,
	/// Left out by the tab's excludes or Git's ignore rules (shown dimmed).
	val ignored: Boolean? = null
)

/// A folder's folders (`Call::Browse`).
@Serializable
data class Browsed (
	/// Where it looked, absolute (`~` expanded, no `..`).
	val path: String,
	/// The folder above, if any.
	val parent: String? = null,
	/// Its folders (hidden ones left out), by name.
	val entries: List<FolderEntry>
)

@Serializable
data class ChangePassword (
	val old: String,
	val new: String,
	val totp: String
)

@Serializable
data class ClientHello (
	val api: UInt,
	/// What the connector is, for logs: `web 0.1.0`, `android 0.1.0`.
	val client: String? = null
)

@Serializable
data class ClosedBench (
	val name: String,
	val color: String,
	val badge: String? = null,
	val cwd: String,
	/// Its tabs as they will load (panes without a screen).
	val tabs: List<Tab>,
	/// When it was saved, Unix seconds.
	val saved: UInt,
	val links: List<Link>? = null
)

@Serializable
data class ClosedName (
	val name: String
)

@Serializable
data class CommandTab (
	val folder: String,
	val command: String
)

@Serializable
data class Commit (
	val hash: String,
	val short: String,
	val subject: String,
	val author: String,
	/// Unix seconds.
	val time: UInt
)

@Serializable
data class CommitFile (
	val status: String,
	val path: String
)

@Serializable
data class Cursor (
	/// Row and column in cells of the grid (a wide character takes two
	/// columns, and once in a span's `text`).
	val row: UInt,
	val col: UInt,
	val visible: Boolean
)

@Serializable
enum class DeviceKind(val string: String) {
	/// A browser: gets a session cookie.
	@SerialName("browser")
	Browser("browser"),
	/// The Android app: gets a device token.
	@SerialName("phone")
	Phone("phone"),
	/// A Shed workspace (paired, not signed in).
	@SerialName("workspace")
	Workspace("workspace"),
	/// Shed's desktop app signed in to show other workspaces (`shed nexus
	/// signin`): gets a device token, like a phone, and may open tunnels
	/// to workspaces that allow desktop clients.
	@SerialName("desktop")
	Desktop("desktop"),
}

@Serializable
data class Device (
	val id: String,
	val kind: DeviceKind,
	val name: String,
	/// Unix seconds.
	val created: UInt,
	val seen: UInt,
	/// Where it was last seen from: the address, and its country if known.
	val address: String? = null,
	val country: String? = null,
	/// The device asking.
	val current: Boolean? = null,
	/// Connected now.
	val online: Boolean? = null
)

/// The device signing in.
@Serializable
data class DeviceHello (
	val kind: DeviceKind,
	/// "Firefox on Linux", "Pixel 8".
	val name: String
)

@Serializable
enum class DiffLineKind(val string: String) {
	@SerialName("context")
	Context("context"),
	@SerialName("added")
	Added("added"),
	@SerialName("removed")
	Removed("removed"),
}

@Serializable
data class DiffLine (
	val kind: DiffLineKind,
	val text: String,
	/// Line numbers, old and new side.
	val old: UInt? = null,
	val new: UInt? = null
)

@Serializable
data class Hunk (
	/// `@@ -12,7 +12,9 @@`.
	val header: String,
	val lines: List<DiffLine>
)

@Serializable
data class Diff (
	val hunks: List<Hunk>,
	/// A picture changed: both sides, when they are pictures.
	val oldPicture: Picture? = null,
	val newPicture: Picture? = null,
	/// Binary, not shown.
	val binary: Boolean? = null
)

@Serializable
data class Ended (
	val sub: UInt,
	val error: ApiError? = null
)

@Serializable
sealed class Event {
	@Serializable
	@SerialName("AiTranscript")
	data class AiTranscript(val content: dev.shed.api.AiTranscript): Event()
	@Serializable
	@SerialName("AiInfo")
	data class AiInfo(val content: dev.shed.api.AiInfo): Event()
	@Serializable
	@SerialName("Screen")
	data class Screen(val content: dev.shed.api.Screen): Event()
	@Serializable
	@SerialName("FilesChanged")
	data class FilesChanged(val content: dev.shed.api.FilesChanged): Event()
	@Serializable
	@SerialName("GitChanged")
	object GitChanged: Event()
	@Serializable
	@SerialName("SearchHits")
	data class SearchHits(val content: dev.shed.api.SearchHits): Event()
	@Serializable
	@SerialName("SearchDone")
	data class SearchDone(val content: dev.shed.api.SearchDone): Event()
	/// Entries of a conversation changed in place, each alone (a sub-agent's
	/// progress while the conversation goes on), and its status.
	@Serializable
	@SerialName("AiEntries")
	data class AiEntries(val content: AiEntriesChanged): Event()
	/// Never sent: connectors read an event they do not know (a newer
	/// workspace's) as this, and ignore it.
	@Serializable
	@SerialName("Unknown")
	object Unknown: Event()
}

@Serializable
data class EventFrame (
	/// The subscription's id.
	val sub: UInt,
	val event: Event
)

@Serializable
data class Explanation (
	val key: String,
	val text: String
)

@Serializable
enum class FileKind(val string: String) {
	@SerialName("text")
	Text("text"),
	@SerialName("picture")
	Picture("picture"),
	@SerialName("binary")
	Binary("binary"),
	@SerialName("tooLarge")
	TooLarge("tooLarge"),
}

/// A coloured range in a line of code, and what it is. `start` and `end`
/// are positions in the line in UTF-16 code units (as JavaScript and Kotlin
/// strings index).
@Serializable
data class Highlight (
	val start: UInt,
	val end: UInt,
	/// `keyword`, `string`, `comment`, `function`, `type`, `number`,
	/// `constant`, `property`, `operator`, `tag`, `attribute`… each
	/// connector colours these by its theme.
	val kind: String
)

@Serializable
data class FileContent (
	val path: String,
	val kind: FileKind,
	/// For text: the whole of it.
	val text: String? = null,
	/// The language's name, if known.
	val lang: String? = null,
	/// Per line, its coloured ranges.
	val highlights: List<List<Highlight>>? = null,
	/// For a picture: the picture.
	val picture: Picture? = null,
	/// Bytes.
	val size: Double
)

@Serializable
data class FileMatch (
	val path: String,
	/// Positions in `path` that matched, to highlight, in UTF-16 code units.
	val hits: List<UInt>
)

@Serializable
data class FilesChanged (
	/// Relative to the root; files and folders made, changed or removed.
	val paths: List<String>
)

@Serializable
data class Folder (
	val folder: String
)

@Serializable
data class GitBranch (
	val tab: String,
	val branch: String
)

@Serializable
data class GitCommit (
	val tab: String,
	val message: String,
	val amend: Boolean? = null
)

@Serializable
data class GitCommitRef (
	val tab: String,
	val hash: String
)

@Serializable
sealed class DiffSource {
	/// Index against HEAD.
	@Serializable
	@SerialName("Staged")
	object Staged: DiffSource()
	/// Work tree against the index.
	@Serializable
	@SerialName("Unstaged")
	object Unstaged: DiffSource()
	/// What a commit changed.
	@Serializable
	@SerialName("Commit")
	data class Commit(val content: String): DiffSource()
}

@Serializable
data class GitDiff (
	val tab: String,
	val path: String,
	val source: DiffSource
)

@Serializable
data class GitFile (
	val path: String,
	/// Index and work-tree status letters, as `git status --porcelain`.
	val staged: String,
	val unstaged: String,
	val conflict: Boolean? = null,
	/// Renamed from.
	val from: String? = null
)

@Serializable
enum class HunkAction(val string: String) {
	@SerialName("stage")
	Stage("stage"),
	@SerialName("unstage")
	Unstage("unstage"),
	@SerialName("discard")
	Discard("discard"),
}

@Serializable
data class GitHunk (
	val tab: String,
	val path: String,
	val source: DiffSource,
	/// The hunk's index in the diff, and its header (so a stale index does
	/// nothing).
	val hunk: UInt,
	val header: String,
	val action: HunkAction
)

@Serializable
data class GitLog (
	val tab: String,
	/// One file's history; `None`: the repository's.
	val path: String? = null,
	val limit: UInt? = null
)

@Serializable
data class GitPaths (
	val tab: String,
	val paths: List<String>
)

@Serializable
data class GitPush (
	val tab: String,
	val force: Boolean? = null
)

@Serializable
data class GitStatus (
	/// Not a repository: everything else is empty.
	val repo: Boolean,
	val branch: String? = null,
	val upstream: String? = null,
	val ahead: UInt? = null,
	val behind: UInt? = null,
	/// A merge, rebase or the like under way.
	val operation: String? = null,
	val files: List<GitFile>,
	/// Commits not pushed yet.
	val outgoing: List<Commit>? = null
)

@Serializable
sealed class InputItem {
	/// Typed text (each character as a key press without modifiers).
	@Serializable
	@SerialName("Text")
	data class Text(val content: String): InputItem()
	/// A key with modifiers.
	@Serializable
	@SerialName("Key")
	data class Key(val content: dev.shed.api.Key): InputItem()
	/// Pasted text (bracketed when the program asks for it).
	@Serializable
	@SerialName("Paste")
	data class Paste(val content: String): InputItem()
	/// The mouse, at a cell of the screen. Sent on to the program only
	/// while it asks for mouse events (`Screen::mouse`), encoded as it
	/// asked (X10, SGR); otherwise dropped.
	@Serializable
	@SerialName("Mouse")
	data class Mouse(val content: dev.shed.api.Mouse): InputItem()
}

@Serializable
data class Input (
	val pane: String,
	val input: List<InputItem>
)

@Serializable
data class Key (
	/// A named key (`Enter`, `Tab`, `Backspace`, `Escape`, `Up`, `Down`,
	/// `Left`, `Right`, `Home`, `End`, `PageUp`, `PageDown`, `Insert`,
	/// `Delete`, `F1`…`F12`) or one character (`a`, `/`).
	val key: String,
	val ctrl: Boolean? = null,
	val alt: Boolean? = null,
	val shift: Boolean? = null
)

@Serializable
data class LimitWindow (
	/// 0 to 1.
	val used: Float,
	/// When it resets.
	val resets: UInt
)

@Serializable
data class Limits (
	val fiveHour: LimitWindow? = null,
	val week: LimitWindow? = null,
	/// When Claude Code reported them.
	val seen: UInt? = null
)

/// What a workspace lets connectors do (`[nexus] control` in `shed.toml`).
@Serializable
enum class Policy(val string: String) {
	/// Look only: calls that change something are refused (`Forbidden`).
	@SerialName("view")
	View("view"),
	/// Everything.
	@SerialName("full")
	Full("full"),
}

@Serializable
data class LinkHello (
	val api: UInt,
	/// Shed's version on the workspace.
	val version: String,
	val policy: Policy
)

@Serializable
data class LinkWelcome (
	val api: UInt,
	val nexus: String,
	/// The workspace's id and name at the nexus.
	val workspace: String,
	val name: String
)

@Serializable
data class LoadBench (
	val name: String,
	val start: Boolean? = null
)

@Serializable
data class LogEntry (
	val command: String,
	val cwd: String,
	val exit: Int? = null,
	/// Unix seconds.
	val started: UInt,
	/// Seconds.
	val duration: Double,
	val bench: String? = null,
	val tab: String? = null
)

@Serializable
enum class LogScope(val string: String) {
	@SerialName("tab")
	Tab("tab"),
	@SerialName("bench")
	Bench("bench"),
	@SerialName("all")
	All("all"),
}

@Serializable
data class LogSearch (
	val scope: LogScope,
	/// The tab (and its bench) the tab and bench scopes are about.
	val tab: String? = null,
	/// Words to find; may hold `tag:label`.
	val query: String? = null,
	val limit: UInt? = null
)

@Serializable
data class Login (
	val username: String,
	val password: String,
	/// A code from the authenticator, or a recovery code.
	val totp: String,
	val device: DeviceHello
)

@Serializable
enum class MouseKind(val string: String) {
	@SerialName("press")
	Press("press"),
	@SerialName("release")
	Release("release"),
	@SerialName("move")
	Move("move"),
	/// One notch of the wheel, up (away from the user) or down.
	@SerialName("wheelUp")
	WheelUp("wheelUp"),
	@SerialName("wheelDown")
	WheelDown("wheelDown"),
}

@Serializable
enum class MouseButton(val string: String) {
	@SerialName("left")
	Left("left"),
	@SerialName("middle")
	Middle("middle"),
	@SerialName("right")
	Right("right"),
}

@Serializable
data class Mouse (
	val kind: MouseKind,
	/// The button pressed or released, or held while moving (`None`: no
	/// button held; a press or release without one is the left button).
	/// Not used for the wheel.
	val button: MouseButton? = null,
	/// 0-based, in cells of the screen (`Screen::cols` by `Screen::rows`).
	val col: UInt,
	val row: UInt,
	val ctrl: Boolean? = null,
	val alt: Boolean? = null,
	val shift: Boolean? = null
)

/// A running bench by id, or a closed one by name.
@Serializable
sealed class BenchRef {
	@Serializable
	@SerialName("Running")
	data class Running(val content: String): BenchRef()
	@Serializable
	@SerialName("Closed")
	data class Closed(val content: String): BenchRef()
}

@Serializable
data class MoveTab (
	val tab: String,
	val from: BenchRef,
	val to: BenchRef,
	val index: UInt
)

@Serializable
data class NewBench (
	val name: String? = null,
	val cwd: String? = null
)

@Serializable
sealed class NewTabKind {
	/// A shell, in a folder (the bench's by default).
	@Serializable
	@SerialName("Terminal")
	data class Terminal(val content: OptionalFolder): NewTabKind()
	/// A shell that runs a command.
	@Serializable
	@SerialName("Command")
	data class Command(val content: CommandTab): NewTabKind()
	@Serializable
	@SerialName("Project")
	data class Project(val content: Folder): NewTabKind()
	@Serializable
	@SerialName("Ai")
	data class Ai(val content: Folder): NewTabKind()
	/// A shell on a server; `join` an existing one by id.
	@Serializable
	@SerialName("Remote")
	data class Remote(val content: RemoteTabArgs): NewTabKind()
}

@Serializable
data class NewTab (
	val bench: BenchRef,
	val kind: NewTabKind
)

@Serializable
data class NexusStatus (
	val version: String,
	val api: UInt,
	/// The account is set up (otherwise the web app shows the setup).
	val ready: Boolean,
	/// Whoever asks is signed in.
	val signedIn: Boolean
)

@Serializable
enum class NotificationKind(val string: String) {
	/// An AI tab waits for a permission or an answer.
	@SerialName("aiWaiting")
	AiWaiting("aiWaiting"),
	/// An AI tab finished a turn.
	@SerialName("aiDone")
	AiDone("aiDone"),
	/// A command that ran long finished.
	@SerialName("commandDone")
	CommandDone("commandDone"),
	/// A workspace went offline (sent by the nexus itself).
	@SerialName("workspaceOffline")
	WorkspaceOffline("workspaceOffline"),
}

/// Something that happened in a workspace that the user may want to hear
/// about. While a connector is in view the nexus sends it live to the
/// connectors (`ServerFrame::Notification`); otherwise it pushes it: to a
/// ntfy topic if it has one (`PushInfo::ntfy_topic`), and as Web Push, a
/// `http::PushPayload` (this, with the workspace's id and name added), to
/// the browsers (and, without a topic, the phones) that want its kind.
@Serializable
data class Notification (
	val kind: NotificationKind,
	/// One line: "Claude waits for an answer".
	val title: String,
	/// More, if any: the bench and tab, the question.
	val body: String? = null,
	/// Where it happened, to open it from the notification.
	val bench: String? = null,
	val tab: String? = null
)

@Serializable
data class NotificationFrame (
	/// The workspace's id (`WorkspaceInfo::id`).
	val workspace: String,
	val notification: Notification
)

@Serializable
data class OptionalFolder (
	val folder: String? = null
)

@Serializable
data class PairApprove (
	/// The code `shed nexus join` printed on the workspace, as the person
	/// typed it (case, spaces and dashes do not matter).
	val code: String,
	/// The name to give it (may differ from the one it asked for).
	val name: String,
	val totp: String
)

@Serializable
data class PairApproved (
	val workspace: String,
	val name: String,
	val token: String
)

@Serializable
data class PairPoll (
	val poll: String
)

@Serializable
data class PairReject (
	/// The request's code, if known; else empty and `id` set.
	val code: String? = null,
	/// The request's id from `PendingPairing`.
	val id: String? = null
)

/// A workspace asks to be paired.
@Serializable
data class PairStart (
	/// The name it would like (its host name).
	val name: String,
	val version: String
)

@Serializable
data class PairStarted (
	/// What the person types in the web app to approve it: `K7QF-2M9D`.
	val code: String,
	/// What the workspace polls with; secret.
	val poll: String,
	/// Unix seconds; the request is gone after.
	val expires: UInt
)

@Serializable
data class PaneOnly (
	val pane: String
)

/// A pairing request waiting, for information: which workspace asks and
/// from where. It carries no code: approving takes the code `shed nexus
/// join` printed, typed by the person, so a request someone else made
/// cannot be approved by mistake.
@Serializable
data class PendingPairing (
	/// Always empty (kept for older apps); see `PairApprove::code`.
	val code: String,
	/// Names it to reject it (`PairReject::id`).
	val id: String? = null,
	val name: String,
	val version: String,
	val address: String,
	val country: String? = null,
	val expires: UInt
)

@Serializable
data class Preview (
	val pane: String,
	val lines: List<String>
)

@Serializable
data class ProjectPath (
	/// A project or AI tab.
	val tab: String,
	/// Relative to its root; `""` is the root.
	val path: String
)

@Serializable
data class ProjectTab (
	val root: String,
	/// Files open in the GUI, relative to the root.
	val open: List<String>? = null,
	val active: UInt? = null,
	val links: List<Link>? = null
)

@Serializable
data class ProjectTopic (
	val tab: String
)

@Serializable
data class PushPrefs (
	/// The kinds this device wants.
	val kinds: List<NotificationKind>
)

/// How this nexus pushes.
@Serializable
data class PushInfo (
	/// The VAPID public key for Web Push (base64url), if Web Push is on.
	val vapidKey: String? = null,
	/// This device's choices.
	val prefs: PushPrefs,
	/// This device has a subscription (`push/subscribe`), so pushes reach
	/// it. False once it unsubscribed, or after the push service said the
	/// subscription is gone.
	val subscribed: Boolean? = null,
	/// The ntfy topic the nexus publishes to when no app is in view
	/// (`https://ntfy.example.com/alerts-shed`), if it has one: phones
	/// subscribe to it in the ntfy app, and get no UnifiedPush then.
	val ntfyTopic: String? = null
)

/// What a push carries, once decrypted: a workspace's `Notification` with
/// the workspace it came from, added by the nexus. A connector opens
/// `/w/{workspace}/b/{bench}/t/{tab}` from it (as far as those are given).
@Serializable
data class PushPayload (
	val kind: NotificationKind,
	val title: String,
	val body: String? = null,
	/// The workspace's id (`WorkspaceInfo::id`).
	val workspace: String,
	/// Its name at the nexus.
	val workspaceName: String? = null,
	val bench: String? = null,
	val tab: String? = null
)

/// Where to push to this device: a Web Push subscription (browser), or a
/// UnifiedPush endpoint (Android, through the distributor, such as ntfy).
/// Both are Web Push style: an endpoint and the keys to encrypt for.
@Serializable
data class PushSubscribe (
	val endpoint: String,
	/// The receiver's public key (base64url, P-256).
	val p256dh: String,
	/// The shared auth secret (base64url).
	val auth: String
)

@Serializable
data class QuickOpen (
	val tab: String,
	val query: String,
	val limit: UInt? = null
)

@Serializable
data class RemoteTab (
	val host: String,
	val id: String
)

@Serializable
data class RemoteTabArgs (
	val host: String,
	val join: String? = null
)

@Serializable
data class RenameBench (
	val bench: BenchRef,
	val name: String
)

@Serializable
data class RenameDevice (
	val device: String,
	val name: String
)

@Serializable
data class RenameTab (
	val bench: BenchRef,
	val tab: String,
	/// `None`: back to the automatic title.
	val name: String? = null
)

@Serializable
sealed class Call {
	/// A new running bench. → `Bench`
	@Serializable
	@SerialName("NewBench")
	data class NewBench(val content: dev.shed.api.NewBench): Call()
	/// Loads a closed bench; `start` runs its startup too. → `Bench`
	@Serializable
	@SerialName("LoadBench")
	data class LoadBench(val content: dev.shed.api.LoadBench): Call()
	/// Closes a running bench (it is saved; teardown runs). → `Done`
	@Serializable
	@SerialName("CloseBench")
	data class CloseBench(val content: BenchOnly): Call()
	/// Deletes a closed bench's file. → `Done`
	@Serializable
	@SerialName("DeleteBench")
	data class DeleteBench(val content: ClosedName): Call()
	/// Saves a running bench now. → `Done`
	@Serializable
	@SerialName("SaveBench")
	data class SaveBench(val content: BenchOnly): Call()
	@Serializable
	@SerialName("RunStartup")
	data class RunStartup(val content: BenchOnly): Call()
	@Serializable
	@SerialName("RunTeardown")
	data class RunTeardown(val content: BenchOnly): Call()
	/// Running or closed. → `Done`
	@Serializable
	@SerialName("RenameBench")
	data class RenameBench(val content: dev.shed.api.RenameBench): Call()
	@Serializable
	@SerialName("SetBenchColor")
	data class SetBenchColor(val content: dev.shed.api.SetBenchColor): Call()
	@Serializable
	@SerialName("SetBadge")
	data class SetBadge(val content: dev.shed.api.SetBadge): Call()
	/// The last lines of every tab's active pane, running and closed, for
	/// cards. → `Previews`
	@Serializable
	@SerialName("Previews")
	object Previews: Call()
	/// → `Tab`
	@Serializable
	@SerialName("NewTab")
	data class NewTab(val content: dev.shed.api.NewTab): Call()
	@Serializable
	@SerialName("CloseTab")
	data class CloseTab(val content: TabOnly): Call()
	@Serializable
	@SerialName("RenameTab")
	data class RenameTab(val content: dev.shed.api.RenameTab): Call()
	@Serializable
	@SerialName("SetTags")
	data class SetTags(val content: dev.shed.api.SetTags): Call()
	@Serializable
	@SerialName("SetPrivate")
	data class SetPrivate(val content: dev.shed.api.SetPrivate): Call()
	/// Into a bench (running), at an index.
	@Serializable
	@SerialName("MoveTab")
	data class MoveTab(val content: dev.shed.api.MoveTab): Call()
	@Serializable
	@SerialName("SetDivider")
	data class SetDivider(val content: dev.shed.api.SetDivider): Call()
	/// Makes it the bench's active tab (the GUI follows).
	@Serializable
	@SerialName("FocusTab")
	data class FocusTab(val content: TabOnly): Call()
	/// Makes a remote tab drop its connection and join its shell again at
	/// once (the shell on the server keeps running); one whose connecting
	/// was stopped (Ctrl+C) starts it again. → `Done`
	@Serializable
	@SerialName("ReconnectRemote")
	data class ReconnectRemote(val content: TabOnly): Call()
	/// → `Pane`
	@Serializable
	@SerialName("Split")
	data class Split(val content: SplitPane): Call()
	@Serializable
	@SerialName("ClosePane")
	data class ClosePane(val content: PaneOnly): Call()
	@Serializable
	@SerialName("FocusPane")
	data class FocusPane(val content: PaneOnly): Call()
	/// Keys, text or a paste, encoded for the program as the GUI would.
	@Serializable
	@SerialName("Input")
	data class Input(val content: dev.shed.api.Input): Call()
	/// Resizes the PTY to this connector's screen while it shows the pane;
	/// `ReleaseSize` (or going away) gives the size back.
	@Serializable
	@SerialName("TakeSize")
	data class TakeSize(val content: dev.shed.api.TakeSize): Call()
	@Serializable
	@SerialName("ReleaseSize")
	data class ReleaseSize(val content: PaneOnly): Call()
	/// Lines of scrollback, oldest first: `from` counts back from the
	/// screen's top (0: the line just above it). → `Lines`
	@Serializable
	@SerialName("Scrollback")
	data class Scrollback(val content: dev.shed.api.Scrollback): Call()
	/// Runs a held startup now.
	@Serializable
	@SerialName("StartNow")
	data class StartNow(val content: PaneOnly): Call()
	/// Stops the AI prompt's request under way.
	@Serializable
	@SerialName("CancelAsk")
	data class CancelAsk(val content: PaneOnly): Call()
	/// → `Log`
	@Serializable
	@SerialName("LogSearch")
	data class LogSearch(val content: dev.shed.api.LogSearch): Call()
	/// The folder's sessions, with draft sessions. → `Sessions`
	@Serializable
	@SerialName("AiSessions")
	data class AiSessions(val content: TabOnly): Call()
	/// Shows and continues a session; `None`: a new one. → `Done`
	@Serializable
	@SerialName("AiOpenSession")
	data class AiOpenSession(val content: dev.shed.api.AiOpenSession): Call()
	@Serializable
	@SerialName("AiRenameSession")
	data class AiRenameSession(val content: dev.shed.api.AiRenameSession): Call()
	/// Deletes a Claude Code session and the sessions going on from it
	/// (their files go to Shed's trash, kept 30 days). Refused while another
	/// AI tab shows one of them; the tab, showing one, goes to a new
	/// session. → `Done`
	@Serializable
	@SerialName("AiDeleteSession")
	data class AiDeleteSession(val content: dev.shed.api.AiDeleteSession): Call()
	/// Sends a prompt (queued if a turn is under way). → `Done`
	@Serializable
	@SerialName("AiSend")
	data class AiSend(val content: dev.shed.api.AiSend): Call()
	@Serializable
	@SerialName("AiStop")
	data class AiStop(val content: TabOnly): Call()
	@Serializable
	@SerialName("AiAnswer")
	data class AiAnswer(val content: dev.shed.api.AiAnswer): Call()
	@Serializable
	@SerialName("AiAnswerQuestions")
	data class AiAnswerQuestions(val content: dev.shed.api.AiAnswerQuestions): Call()
	/// Edits an earlier prompt and sends it again (the conversation goes on
	/// from there).
	@Serializable
	@SerialName("AiEdit")
	data class AiEdit(val content: dev.shed.api.AiEdit): Call()
	/// Goes back to before a prompt: the conversation, and the files too if
	/// `files`.
	@Serializable
	@SerialName("AiRevert")
	data class AiRevert(val content: dev.shed.api.AiRevert): Call()
	/// Changes or sends a queued message.
	@Serializable
	@SerialName("AiQueued")
	data class AiQueued(val content: dev.shed.api.AiQueued): Call()
	/// Model, mode, effort, follow, browser: only the fields given change.
	@Serializable
	@SerialName("AiSettings")
	data class AiSettings(val content: dev.shed.api.AiSettings): Call()
	/// Sets the prompt's unsent text.
	@Serializable
	@SerialName("AiSetUnsent")
	data class AiSetUnsent(val content: dev.shed.api.AiSetUnsent): Call()
	@Serializable
	@SerialName("AiAddDraft")
	data class AiAddDraft(val content: dev.shed.api.AiAddDraft): Call()
	@Serializable
	@SerialName("AiRemoveDraft")
	data class AiRemoveDraft(val content: dev.shed.api.AiRemoveDraft): Call()
	/// A picture of the conversation (by the name in an entry). → `Picture`
	@Serializable
	@SerialName("AiPicture")
	data class AiPicture(val content: AiPictureName): Call()
	/// The changes Claude made, for review. → `Review`
	@Serializable
	@SerialName("AiReview")
	data class AiReview(val content: TabOnly): Call()
	@Serializable
	@SerialName("AiSetViewed")
	data class AiSetViewed(val content: dev.shed.api.AiSetViewed): Call()
	/// Asks, aside, for an explanation of changes. → `Explained`
	@Serializable
	@SerialName("AiExplain")
	data class AiExplain(val content: dev.shed.api.AiExplain): Call()
	/// Starts Claude Code for the tab ahead of the first prompt.
	@Serializable
	@SerialName("AiStart")
	data class AiStart(val content: TabOnly): Call()
	/// Keeps a file on the workspace for a prompt to attach (by its path, in
	/// the text): at most `MAX_UPLOAD` bytes, in a folder of the tab's under
	/// Shed's state folder, deleted after a while. → `Text` (its path)
	@Serializable
	@SerialName("AiUpload")
	data class AiUpload(val content: dev.shed.api.AiUpload): Call()
	/// A folder's entries. → `Folder`
	@Serializable
	@SerialName("ListFolder")
	data class ListFolder(val content: ProjectPath): Call()
	/// Files whose names match, best first. → `Files`
	@Serializable
	@SerialName("QuickOpen")
	data class QuickOpen(val content: dev.shed.api.QuickOpen): Call()
	/// → `File`
	@Serializable
	@SerialName("ReadFile")
	data class ReadFile(val content: ProjectPath): Call()
	/// Writes a file whole (light edits). → `Done`
	@Serializable
	@SerialName("WriteFile")
	data class WriteFile(val content: dev.shed.api.WriteFile): Call()
	/// Makes a file, or a folder (a path ending in `/`).
	@Serializable
	@SerialName("CreateEntry")
	data class CreateEntry(val content: ProjectPath): Call()
	/// → `GitStatus`
	@Serializable
	@SerialName("GitStatus")
	data class GitStatus(val content: TabOnly): Call()
	/// A file's changes. → `Diff`
	@Serializable
	@SerialName("GitDiff")
	data class GitDiff(val content: dev.shed.api.GitDiff): Call()
	@Serializable
	@SerialName("GitStage")
	data class GitStage(val content: GitPaths): Call()
	@Serializable
	@SerialName("GitUnstage")
	data class GitUnstage(val content: GitPaths): Call()
	/// Throws away changes (asks first, in the apps).
	@Serializable
	@SerialName("GitDiscard")
	data class GitDiscard(val content: GitPaths): Call()
	/// Stages, unstages or discards one hunk of `GitDiff`'s answer.
	@Serializable
	@SerialName("GitHunk")
	data class GitHunk(val content: dev.shed.api.GitHunk): Call()
	/// → `Text` (the commit's hash)
	@Serializable
	@SerialName("GitCommit")
	data class GitCommit(val content: dev.shed.api.GitCommit): Call()
	/// → `Commits`
	@Serializable
	@SerialName("GitLog")
	data class GitLog(val content: dev.shed.api.GitLog): Call()
	/// → `CommitFiles`
	@Serializable
	@SerialName("GitCommitFiles")
	data class GitCommitFiles(val content: GitCommitRef): Call()
	/// → `Branches`
	@Serializable
	@SerialName("GitBranches")
	data class GitBranches(val content: TabOnly): Call()
	@Serializable
	@SerialName("GitSwitch")
	data class GitSwitch(val content: GitBranch): Call()
	@Serializable
	@SerialName("GitCreateBranch")
	data class GitCreateBranch(val content: GitBranch): Call()
	/// → `Text` (what git said)
	@Serializable
	@SerialName("GitFetch")
	data class GitFetch(val content: TabOnly): Call()
	@Serializable
	@SerialName("GitPull")
	data class GitPull(val content: TabOnly): Call()
	@Serializable
	@SerialName("GitPush")
	data class GitPush(val content: dev.shed.api.GitPush): Call()
	/// Folders under a path, to choose where a tab works. → `Folder`.
	/// Refused (`Forbidden`) by a view-only workspace, which makes no tabs.
	/// Kept for older apps: `Browse` says where it looked too.
	@Serializable
	@SerialName("BrowseFolders")
	data class BrowseFolders(val content: dev.shed.api.Browse): Call()
	/// Folders under a path, and the path as an absolute one (`~` and `""`
	/// made into the home folder), to show and to go up from. → `Browsed`.
	/// Refused by a view-only workspace, as `BrowseFolders`.
	@Serializable
	@SerialName("Browse")
	data class Browse(val content: dev.shed.api.Browse): Call()
}

/// A call on a workspace. Answered by one `Response` with the same id.
@Serializable
data class Request (
	/// Chosen by the sender, unique among its requests in flight.
	val id: UInt,
	/// The workspace's id (from `WorkspaceInfo`); ignored on `/link`.
	val workspace: String? = null,
	val call: Call
)

@Serializable
sealed class Reply {
	@Serializable
	@SerialName("Done")
	object Done: Reply()
	/// A bench's id.
	@Serializable
	@SerialName("Bench")
	data class Bench(val content: String): Reply()
	/// A tab's id.
	@Serializable
	@SerialName("Tab")
	data class Tab(val content: String): Reply()
	/// A pane's id.
	@Serializable
	@SerialName("Pane")
	data class Pane(val content: String): Reply()
	@Serializable
	@SerialName("Previews")
	data class Previews(val content: List<Preview>): Reply()
	@Serializable
	@SerialName("Lines")
	data class Lines(val content: List<ScreenLine>): Reply()
	@Serializable
	@SerialName("Log")
	data class Log(val content: List<LogEntry>): Reply()
	@Serializable
	@SerialName("Sessions")
	data class Sessions(val content: List<AiSession>): Reply()
	@Serializable
	@SerialName("Picture")
	data class Picture(val content: dev.shed.api.Picture): Reply()
	@Serializable
	@SerialName("Review")
	data class Review(val content: dev.shed.api.Review): Reply()
	@Serializable
	@SerialName("Explained")
	data class Explained(val content: List<Explanation>): Reply()
	@Serializable
	@SerialName("Folder")
	data class Folder(val content: List<FolderEntry>): Reply()
	@Serializable
	@SerialName("Files")
	data class Files(val content: List<FileMatch>): Reply()
	@Serializable
	@SerialName("File")
	data class File(val content: FileContent): Reply()
	@Serializable
	@SerialName("GitStatus")
	data class GitStatus(val content: dev.shed.api.GitStatus): Reply()
	@Serializable
	@SerialName("Diff")
	data class Diff(val content: dev.shed.api.Diff): Reply()
	@Serializable
	@SerialName("Commits")
	data class Commits(val content: List<Commit>): Reply()
	@Serializable
	@SerialName("CommitFiles")
	data class CommitFiles(val content: List<CommitFile>): Reply()
	@Serializable
	@SerialName("Branches")
	data class Branches(val content: dev.shed.api.Branches): Reply()
	@Serializable
	@SerialName("Text")
	data class Text(val content: String): Reply()
	@Serializable
	@SerialName("Browsed")
	data class Browsed(val content: dev.shed.api.Browsed): Reply()
}

@Serializable
data class Response (
	val id: UInt,
	/// The answer, when the call worked.
	val ok: Reply? = null,
	/// Why not, when it did not.
	val error: ApiError? = null
)

@Serializable
data class ReviewChange (
	/// Stable while the change is the same: what `AiSetViewed` and
	/// `AiExplain` name.
	val key: String,
	/// Relative to the repository.
	val path: String,
	/// `uncommitted`, or the commit it is in.
	val layer: String,
	/// `M`, `A`, `D`, `R`, `?`.
	val status: String,
	val viewed: Boolean,
	val explanation: String? = null,
	val diff: Diff
)

@Serializable
data class ReviewRepo (
	val root: String,
	/// Commits made since the session started.
	val commits: List<Commit>? = null,
	val changes: List<ReviewChange>
)

/// What Claude changed, by repository.
@Serializable
data class Review (
	val repos: List<ReviewRepo>
)

@Serializable
data class Revoke (
	val device: String,
	val totp: String
)

/// Text with one style. Wide characters take two cells; `text` has them once.
@Serializable
data class Span (
	val text: String,
	/// `#rrggbb`, or a palette index as `0`…`255`; `None`: the default.
	val fg: String? = null,
	val bg: String? = null,
	val bold: Boolean? = null,
	val dim: Boolean? = null,
	val italic: Boolean? = null,
	val underline: Boolean? = null,
	val strike: Boolean? = null,
	val inverse: Boolean? = null
)

@Serializable
data class ScreenLine (
	val spans: List<Span>,
	/// It goes on in the next row (it was wrapped, not ended).
	val wraps: Boolean? = null
)

@Serializable
data class ScreenRow (
	val row: UInt,
	val line: ScreenLine
)

/// A terminal's screen, or the lines of it that changed.
@Serializable
data class Screen (
	/// The whole screen (first event, a resize, a switch of screens): rows
	/// not in `lines` are blank. Otherwise only changed rows are given.
	val full: Boolean,
	val cols: UInt,
	val rows: UInt,
	/// The program uses the alternate screen (vim, htop): show the grid as is
	/// rather than re-wrapping lines to the connector's width.
	val alt: Boolean? = null,
	val lines: List<ScreenRow>,
	val cursor: Cursor,
	val title: String? = null,
	/// How many lines of scrollback there are (`Call::Scrollback`).
	val scrollback: UInt? = null,
	/// The program wants mouse events.
	val mouse: Boolean? = null,
	/// Lines scrolled off the top since the last event (so a connector
	/// showing scrollback can keep its place).
	val scrolled: UInt? = null
)

@Serializable
data class ScreenTopic (
	val pane: String
)

@Serializable
data class Scrollback (
	val pane: String,
	val from: UInt,
	val count: UInt
)

@Serializable
data class SearchDone (
	/// Stopped at the limit: there may be more.
	val truncated: Boolean? = null,
	val error: String? = null
)

@Serializable
data class SearchHit (
	/// 1-based.
	val line: UInt,
	val text: String,
	/// Ranges in `text` that matched: start, end, start, end…, in UTF-16
	/// code units.
	val ranges: List<UInt>
)

@Serializable
data class SearchHits (
	val path: String,
	val hits: List<SearchHit>
)

@Serializable
data class SearchTopic (
	val tab: String,
	val query: String,
	val regex: Boolean? = null,
	val caseSensitive: Boolean? = null,
	val wholeWord: Boolean? = null,
	/// Globs of files to search in, and to leave out.
	val include: String? = null,
	val exclude: String? = null
)

@Serializable
data class SetBadge (
	val bench: BenchRef,
	val badge: String? = null
)

@Serializable
data class SetBenchColor (
	val bench: BenchRef,
	/// A named colour (`blue`, `coral`…) or `#rrggbb`.
	val color: String
)

@Serializable
data class SetDivider (
	val tab: String,
	val divider: Divider
)

@Serializable
data class SetPrivate (
	val bench: BenchRef,
	val tab: String,
	val private: Boolean
)

@Serializable
data class SetTags (
	val bench: BenchRef,
	val tab: String,
	val tags: List<Tag>
)

/// The second step: a code from the authenticator, proving it was added.
@Serializable
data class SetupConfirm (
	val code: String,
	val totp: String,
	val device: DeviceHello
)

/// The first step of setting up the account: the setup code (the setup
/// link's `#code=` fragment), a user name and a password.
@Serializable
data class SetupStart (
	val code: String,
	val username: String,
	val password: String
)

/// The TOTP secret to add to an authenticator app.
@Serializable
data class SetupTotp (
	/// Base32, to type in.
	val secret: String,
	/// `otpauth://totp/…`, to show as a QR code.
	val url: String
)

@Serializable
data class SignedIn (
	val username: String,
	/// This device's id.
	val device: String,
	/// For a phone or a desktop: its device token, to keep and send as a
	/// bearer token.
	val token: String? = null,
	/// For a browser: the token to send in `X-Shed-Csrf` with changes.
	val csrf: String? = null,
	/// Single-use codes to sign in without the authenticator; only given at
	/// setup and when new ones are made.
	val recoveryCodes: List<String>? = null
)

@Serializable
enum class SplitDir(val string: String) {
	/// Side by side.
	@SerialName("right")
	Right("right"),
	/// One above the other.
	@SerialName("down")
	Down("down"),
}

@Serializable
data class Split (
	val dir: SplitDir,
	/// The first part's share, 0 to 1.
	val ratio: Float,
	val first: Layout,
	val second: Layout
)

@Serializable
data class SplitPane (
	val pane: String,
	val dir: SplitDir,
	val folder: String? = null
)

@Serializable
data class WorkspaceState (
	/// The machine's host name.
	val host: String,
	/// Running benches, in the order the user arranged them.
	val benches: List<Bench>,
	/// Saved benches that are not running, by name.
	val closed: List<ClosedBench>,
	/// The account's usage limits, as Claude Code last reported them here.
	val limits: Limits? = null,
	/// The user's home folder, to show paths as `~/…`.
	val home: String? = null
)

@Serializable
data class StateFrame (
	val workspace: String,
	val state: WorkspaceState
)

@Serializable
sealed class Topic {
	/// An AI tab's conversation: `AiTranscript`, `AiInfo`.
	@Serializable
	@SerialName("Ai")
	data class Ai(val content: AiTopic): Topic()
	/// A terminal pane's screen: `Screen`.
	@Serializable
	@SerialName("Screen")
	data class Screen(val content: ScreenTopic): Topic()
	/// A project's files and Git changing on disk: `FilesChanged`,
	/// `GitChanged`.
	@Serializable
	@SerialName("Project")
	data class Project(val content: ProjectTopic): Topic()
	/// Searching a project's files: `SearchHits` as they are found, then
	/// `SearchDone`. Unsubscribing stops it.
	@Serializable
	@SerialName("Search")
	data class Search(val content: SearchTopic): Topic()
}

/// Starts a stream of events about something in a workspace. The first event
/// is the whole of it (a transcript, a screen), the next ones its changes.
@Serializable
data class Subscribe (
	/// Chosen by the sender; events carry it, `Unsubscribe` names it.
	val id: UInt,
	val workspace: String? = null,
	val topic: Topic
)

@Serializable
data class TabOnly (
	val tab: String
)

@Serializable
data class TakeSize (
	val pane: String,
	val cols: UInt,
	val rows: UInt
)

/// A tunnel ended (or never opened): `error` says why, if it was not asked for.
@Serializable
data class TunnelClosed (
	val id: UInt,
	val error: ApiError? = null
)

@Serializable
data class TunnelId (
	val id: UInt
)

/// Opens a tunnel to `workspace`'s keeper under `id`.
@Serializable
data class TunnelOpen (
	/// Chosen by the sender, unique among its open tunnels.
	val id: UInt,
	val workspace: String
)

@Serializable
data class Unsubscribe (
	val id: UInt
)

/// A workspace as the nexus knows it.
@Serializable
data class WorkspaceInfo (
	val id: String,
	/// Its name, given at pairing (the machine's host name by default).
	val name: String,
	val online: Boolean,
	/// When it was last connected (Unix seconds), or connected since.
	val seen: UInt,
	/// What it lets connectors do.
	val policy: Policy,
	/// Its Shed version, `0.1.0`.
	val version: String? = null
)

@Serializable
data class Welcome (
	val api: UInt,
	/// The nexus's version, `0.1.0`.
	val nexus: String,
	val workspaces: List<WorkspaceInfo>,
	/// The last state of each workspace that has one (offline ones too).
	val states: List<StateFrame>
)

@Serializable
data class WriteFile (
	val tab: String,
	val path: String,
	val text: String
)

/// From a connector to the nexus.
@Serializable
sealed class ClientFrame {
	/// First frame: the API version the connector speaks.
	@Serializable
	@SerialName("Hello")
	data class Hello(val content: ClientHello): ClientFrame()
	@Serializable
	@SerialName("Request")
	data class Request(val content: dev.shed.api.Request): ClientFrame()
	@Serializable
	@SerialName("Subscribe")
	data class Subscribe(val content: dev.shed.api.Subscribe): ClientFrame()
	@Serializable
	@SerialName("Unsubscribe")
	data class Unsubscribe(val content: dev.shed.api.Unsubscribe): ClientFrame()
	/// Whether the connector is in view (a browser tab shown and focused,
	/// the Android app in the foreground). Sent after `Hello` and on every
	/// change; a connection that never sends it counts as not in view. While
	/// any connector is in view, notifications come to all of them live
	/// (`ServerFrame::Notification`) and nothing is pushed.
	@Serializable
	@SerialName("Visible")
	data class Visible(val content: Boolean): ClientFrame()
	/// Opens a tunnel to a workspace's keeper (desktop connectors only).
	@Serializable
	@SerialName("TunnelOpen")
	data class TunnelOpen(val content: dev.shed.api.TunnelOpen): ClientFrame()
	/// Closes one of this connector's tunnels.
	@Serializable
	@SerialName("TunnelClose")
	data class TunnelClose(val content: TunnelId): ClientFrame()
}

/// From a workspace's link to the nexus.
@Serializable
sealed class LinkFrame {
	/// First frame.
	@Serializable
	@SerialName("Hello")
	data class Hello(val content: LinkHello): LinkFrame()
	/// The workspace's state, whole, whenever it changes (at most a few times
	/// a second). The nexus keeps the last one.
	@Serializable
	@SerialName("State")
	data class State(val content: WorkspaceState): LinkFrame()
	@Serializable
	@SerialName("Response")
	data class Response(val content: dev.shed.api.Response): LinkFrame()
	@Serializable
	@SerialName("Event")
	data class Event(val content: EventFrame): LinkFrame()
	@Serializable
	@SerialName("Ended")
	data class Ended(val content: dev.shed.api.Ended): LinkFrame()
	/// Something worth a notification.
	@Serializable
	@SerialName("Notify")
	data class Notify(val content: Notification): LinkFrame()
	/// A tunnel the nexus asked for is open (connected to the keeper).
	@Serializable
	@SerialName("TunnelOpened")
	data class TunnelOpened(val content: TunnelId): LinkFrame()
	/// A tunnel ended, or was refused (`Forbidden` when the workspace does
	/// not allow desktop clients).
	@Serializable
	@SerialName("TunnelClosed")
	data class TunnelClosed(val content: dev.shed.api.TunnelClosed): LinkFrame()
}

/// From the nexus to a link. Ids are the nexus's own.
@Serializable
sealed class NexusFrame {
	@Serializable
	@SerialName("Welcome")
	data class Welcome(val content: LinkWelcome): NexusFrame()
	@Serializable
	@SerialName("Request")
	data class Request(val content: dev.shed.api.Request): NexusFrame()
	@Serializable
	@SerialName("Subscribe")
	data class Subscribe(val content: dev.shed.api.Subscribe): NexusFrame()
	@Serializable
	@SerialName("Unsubscribe")
	data class Unsubscribe(val content: dev.shed.api.Unsubscribe): NexusFrame()
	/// Closing: the workspace was removed, or its API version is not accepted.
	@Serializable
	@SerialName("Goodbye")
	data class Goodbye(val content: ApiError): NexusFrame()
	/// A desktop connector wants a tunnel to the keeper, under this id.
	@Serializable
	@SerialName("TunnelOpen")
	data class TunnelOpen(val content: TunnelId): NexusFrame()
	/// The tunnel's connector went, or closed it.
	@Serializable
	@SerialName("TunnelClose")
	data class TunnelClose(val content: TunnelId): NexusFrame()
}

@Serializable
sealed class PairState {
	@Serializable
	@SerialName("Pending")
	object Pending: PairState()
	/// Approved: the workspace's id and its token, given once.
	@Serializable
	@SerialName("Approved")
	data class Approved(val content: PairApproved): PairState()
	@Serializable
	@SerialName("Rejected")
	object Rejected: PairState()
	@Serializable
	@SerialName("Expired")
	object Expired: PairState()
}

/// From the nexus to a connector.
@Serializable
sealed class ServerFrame {
	/// Answers `Hello`: the workspaces, each with its last known state.
	@Serializable
	@SerialName("Welcome")
	data class Welcome(val content: dev.shed.api.Welcome): ServerFrame()
	@Serializable
	@SerialName("Response")
	data class Response(val content: dev.shed.api.Response): ServerFrame()
	@Serializable
	@SerialName("Event")
	data class Event(val content: EventFrame): ServerFrame()
	/// A subscription ended without being asked to (the workspace went
	/// offline, the tab closed); subscribe again to resume.
	@Serializable
	@SerialName("Ended")
	data class Ended(val content: dev.shed.api.Ended): ServerFrame()
	/// A workspace came, went, or was renamed or removed: the whole list.
	@Serializable
	@SerialName("Workspaces")
	data class Workspaces(val content: List<WorkspaceInfo>): ServerFrame()
	/// A workspace's state changed: all of it (it is small).
	@Serializable
	@SerialName("State")
	data class State(val content: StateFrame): ServerFrame()
	/// The connection is being closed: signed out, revoked, or the API
	/// version is not accepted. Not to be retried as is.
	@Serializable
	@SerialName("Goodbye")
	data class Goodbye(val content: ApiError): ServerFrame()
	/// A notification, live: sent to every connector while one is in view
	/// (`ClientFrame::Visible`), instead of a push. The app shows it itself.
	@Serializable
	@SerialName("Notification")
	data class Notification(val content: NotificationFrame): ServerFrame()
	/// A tunnel this connector asked for is open: data may flow.
	@Serializable
	@SerialName("TunnelOpened")
	data class TunnelOpened(val content: TunnelId): ServerFrame()
	/// A tunnel ended, or was refused.
	@Serializable
	@SerialName("TunnelClosed")
	data class TunnelClosed(val content: dev.shed.api.TunnelClosed): ServerFrame()
}

const val MAX_UPLOAD: UInt = 16777216u

const val API_VERSION: UInt = 1u
const val MIN_API_VERSION: UInt = 1u
