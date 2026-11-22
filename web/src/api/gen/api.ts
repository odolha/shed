// Generated from crates/shed-api by shed-codegen (typeshare): do not edit.
// Run `cargo run -p shed-codegen` after changing shed-api.
//
// The JSON is serde's: enums with data are `{ type, content }`, enums
// without are strings; a `null` field and a missing one both mean none.

export const API_VERSION = 1;
export const MIN_API_VERSION = 1;

export interface AiAddDraft {
	tab: string;
	text: string;
	/**
	 * The session it belongs to; `None`: the one shown (or a new draft
	 * session when none is).
	 */
	session?: string | null;
}

export interface AiAnswer {
	tab: string;
	request: string;
	allow: boolean;
	/** Allow it for the rest of the session. */
	always?: boolean;
}

export interface AiAnswerQuestions {
	tab: string;
	request: string;
	/** One per question, in order; several choices joined by ", ". */
	answers: string[];
}

export interface AiAssistant {
	text: string;
	at?: number;
}

export interface AiCommand {
	name: string;
	description?: string;
	hint?: string;
}

export interface AiDeleteSession {
	tab: string;
	/** A Claude Code session id (not a draft session's key). */
	session: string;
}

export interface AiDraft {
	session: string;
	text: string;
}

/** A picture, whole. */
export interface Picture {
	/** `image/png` and the like. */
	mediaType: string;
	/** Base64. */
	data: string;
}

export interface AiEdit {
	tab: string;
	/** The prompt's entry index. */
	entry: number;
	text: string;
	pictures?: Picture[];
}

export type AiEntry =
	| { type: "User", content: AiUser }
	/** Claude's answer, in Markdown; grows while it streams. */
	| { type: "Assistant", content: AiAssistant }
	| { type: "Thinking", content: AiText }
	| { type: "Tool", content: AiTool }
	/** Claude asks to use a tool, or asks the user questions. */
	| { type: "Permission", content: AiPermission }
	/** A finished turn, an interruption, an error. */
	| { type: "Notice", content: AiNotice }
	/**
	 * One of Claude Code's own notes, never the user's words: a background
	 * task that finished, a wake-up, a command's output, a compaction.
	 */
	| { type: "Note", content: AiNote }
	/**
	 * Never sent: connectors read an entry they do not know (a newer
	 * workspace's) as this; it keeps its place and shows as nothing.
	 */
	| { type: "Unknown", content?: undefined };

export interface AiEntryAt {
	index: number;
	entry: AiEntry;
}

export type AiActivity =
	| "idle"
	| "working"
	/** Waits for the user to allow something or answer. */
	| "waiting";

export type AiTimerKind =
	| "wakeup"
	| "job"
	| "watch";

/**
 * Something set to happen later while the tab's Claude Code runs: a
 * wake-up, a scheduled job, a watch.
 */
export interface AiTimer {
	kind?: AiTimerKind;
	/** What it is for: the reason, the job's prompt, the watch's description. */
	what: string;
	/** When it fires next (a watch: when it ends), Unix seconds (0: not known). */
	at?: number;
	/** It repeats: on `every` ("Every minute"). */
	recurring?: boolean;
	every?: string;
	/** What it tells Claude when it fires. */
	prompt?: string;
	/** The tool call (`AiTool::id`) that set it. */
	tool?: string;
}

export interface AiStatus {
	session?: string | null;
	model?: string | null;
	state: AiActivity;
	/** Dollars spent in this session's process. */
	cost?: number;
	/** Tokens in the context, and how many it holds (0: not known). */
	context?: number;
	contextMax?: number;
	/** When the turn under way started. */
	since?: number | null;
	/** Wake-ups, scheduled jobs and watches waiting. */
	timers?: AiTimer[];
}

/** Entries that changed in place, by index; the others stay as they are. */
export interface AiEntriesChanged {
	entries: AiEntryAt[];
	status: AiStatus;
}

export interface AiExplain {
	tab: string;
	keys: string[];
}

export interface AiModel {
	value: string;
	name: string;
	description?: string;
	efforts?: string[];
}

/** What Claude Code offers in this tab: slash commands and models. */
export interface AiInfo {
	commands: AiCommand[];
	models: AiModel[];
}

export type AiNoteKind =
	/** A background command, agent or watch finished, or a watch saw something. */
	| "task"
	/** A wake-up or a scheduled job started a turn. */
	| "wakeup"
	/** A command run in Claude Code's own interface (`/compact`, `! ls`) and its output. */
	| "command"
	/** The conversation was compacted: what it was summed up as. */
	| "compacted"
	/** A message from another Claude session or agent. */
	| "message"
	/** Context a hook added to a prompt. */
	| "hook";

export interface AiNote {
	/** Missing when this version does not know the kind: shown as `task`. */
	kind?: AiNoteKind;
	/** One line. */
	title: string;
	/** More, folded: a report, an output, a summary (Markdown). */
	detail?: string;
	/** Something went wrong (a task failed). */
	error?: boolean;
	at?: number;
}

export interface AiNotice {
	text: string;
	error?: boolean;
}

export interface AiOpenSession {
	tab: string;
	/** A Claude Code session id or a draft session's key; `None`: new. */
	session?: string | null;
}

export interface AiOption {
	label: string;
	description?: string;
}

/**
 * An edit's text: what it replaced and what it put in. (Not `AiEdit`: that
 * is the call, and the generated types share one namespace.)
 */
export interface AiTextEdit {
	old: string;
	new: string;
}

export interface AiQuestion {
	question: string;
	header: string;
	options: AiOption[];
	/** Several options may be chosen. */
	multi?: boolean;
}

export interface AiPermission {
	/** To answer it (`Call::AiAnswer`, `Call::AiAnswerQuestions`). */
	request: string;
	tool: string;
	summary: string;
	detail?: string;
	edit?: AiTextEdit | null;
	/** The user's answer, once given. */
	answer?: boolean | null;
	/**
	 * Questions Claude asks (its AskUserQuestion tool), and once answered,
	 * the answers.
	 */
	questions?: AiQuestion[];
	answers?: string[];
}

export interface AiPictureName {
	tab: string;
	name: string;
}

export interface AiQueued {
	tab: string;
	index: number;
	/** The new text; empty removes it. */
	text: string;
	/** Send it now (stopping the turn under way). */
	send?: boolean;
}

export interface AiRemoveDraft {
	tab: string;
	/**
	 * Its index in the tab's `drafts`, and its text (so a stale index
	 * removes nothing).
	 */
	index: number;
	text: string;
}

export interface AiRenameSession {
	tab: string;
	session: string;
	name: string;
}

export interface AiRevert {
	tab: string;
	entry: number;
	files: boolean;
}

export type AiRunState =
	| "running"
	| "done"
	| "failed"
	/** Stopped before it finished. */
	| "stopped";

/**
 * A tool that goes on after its call: a sub-agent, or a command or watch
 * in the background.
 */
export interface AiRun {
	state?: AiRunState;
	/** Started without waiting for it. */
	background?: boolean;
	/** The kind of agent (`general-purpose`, `Explore`); empty for commands. */
	agent?: string;
	/** When it started and ended, Unix seconds (0: not known, or not yet). */
	started?: number;
	ended?: number;
	/** What it does now (its last step), or how it ended. */
	last?: string;
	/** Tool calls it made, and tokens it used, as last reported. */
	tools?: number;
	tokens?: number;
	/**
	 * Its latest steps (tool calls, what it said); `skipped` earlier ones
	 * are left out. Steps have no runs of their own.
	 */
	steps?: AiEntry[];
	skipped?: number;
	/** What it reported when it ended (Markdown). */
	report?: string;
}

export interface AiSend {
	tab: string;
	text: string;
	pictures?: Picture[];
}

export interface AiSession {
	/** Claude Code's session id, or a draft session's key. */
	id: string;
	title: string;
	/** Unix seconds. */
	modified: number;
	branch?: string | null;
	prompts: number;
	/** The session it goes on from, and how deep it is listed under it. */
	parent?: string | null;
	depth?: number;
	/** A draft session (drafts only, nothing sent yet). */
	draft?: boolean;
	/** How many drafts it has. */
	drafts?: number;
}

export interface AiSetUnsent {
	tab: string;
	text: string;
}

export interface AiSetViewed {
	tab: string;
	key: string;
	viewed: boolean;
}

export interface AiSettings {
	tab: string;
	/** `Some("")`: back to the default model. */
	model?: string | null;
	mode?: string | null;
	/** `Some("")`: back to the default effort (the model's own). */
	effort?: string | null;
	follow?: boolean | null;
	browser?: boolean | null;
	addDirs?: string[] | null;
}

export interface AiTab {
	folder: string;
	addDirs?: string[];
	/** The session shown, if any (a new one otherwise). */
	session?: string | null;
	/** The draft session shown, when `session` is none. */
	draftSession?: string | null;
	model?: string | null;
	/** `default`, `acceptEdits`, `plan`. */
	mode: string;
	effort?: string | null;
	activity: AiActivity;
	/** Messages kept to send later, each with its session. */
	drafts?: AiDraft[];
	/** Messages waiting for the turn under way to end. */
	queued?: string[];
	/** What is written in the prompt and not sent. */
	unsent?: string;
	/** It may use the browser (Claude in Chrome). */
	browser?: boolean;
	follow?: boolean;
}

export interface AiText {
	text: string;
}

export interface AiTool {
	id: string;
	name: string;
	/** One line: `Edit src/main.rs`, `$ cargo test`. */
	summary: string;
	/** The input in full. */
	detail?: string;
	file?: string | null;
	/** For edits: the text replaced and the text put in. */
	edit?: AiTextEdit | null;
	/** What it returned, cut if long; `None` while it runs. */
	result?: string | null;
	error?: boolean;
	pictures?: string[];
	/** For a sub-agent, or a command or watch in the background: how it runs. */
	run?: AiRun | null;
}

export interface AiTopic {
	tab: string;
}

/**
 * A conversation's entries from `from` on (the first event: from 0, all of
 * them), replacing what the connector had from there, and its status.
 */
export interface AiTranscript {
	from: number;
	entries: AiEntry[];
	status: AiStatus;
}

/** A file for a prompt (`Call::AiUpload`). */
export interface AiUpload {
	tab: string;
	/** Its name (made safe: a name, no folders). */
	name: string;
	/** Base64. */
	data: string;
}

export interface AiUser {
	text: string;
	/** When, Unix seconds (0: not known). */
	at?: number;
	/** Pictures sent with it, by name (`Call::AiPicture`). */
	pictures?: string[];
	/** How many pictures it had (some may not be kept). */
	images?: number;
}

export type ErrorCode =
	/** Not signed in, or the token is no longer valid. */
	| "unauthorized"
	/** Signed in, but this is not allowed (a workspace in view-only mode). */
	| "forbidden"
	/** The workspace, bench, tab, pane or file is not there. */
	| "notFound"
	/** The workspace is not connected to the nexus. */
	| "offline"
	/** The request does not make sense (a bad argument). */
	| "invalid"
	/** The workspace or nexus does not know this call (another version). */
	| "unsupported"
	/** Too many attempts; try again later. */
	| "rateLimited"
	/** The wrong one-time code. */
	| "wrongCode"
	/** It took too long. */
	| "timeout"
	/** Something failed on the way; `message` says what. */
	| "failed";

export interface ApiError {
	code: ErrorCode;
	/**
	 * For people, in English; the apps show their own text for the code and
	 * this as detail.
	 */
	message?: string;
}

export interface AuditEntry {
	at: number;
	/** `login`, `login-failed`, `pair`, `revoke`, `password`, `tunnel-open`… */
	what: string;
	device?: string | null;
	address?: string;
	country?: string | null;
	detail?: string;
}

export interface Tag {
	label: string;
	color: string;
	/** Tints the whole tab, as a warning. */
	tint?: boolean;
}

export type TabKind =
	| { type: "Terminal", content?: undefined }
	| { type: "Project", content: ProjectTab }
	| { type: "Ai", content: AiTab }
	| { type: "Remote", content: RemoteTab };

export interface Pane {
	id: string;
	cwd: string;
	/** The size the PTY has now. */
	cols: number;
	rows: number;
	/** The title the program set. */
	title?: string | null;
	/** The program in the foreground, when it is not the shell: `vim notes.md`. */
	tool?: string | null;
	/** A command typed by a bench load, waiting for Enter. */
	waiting?: string | null;
	/** Its startup is held until other panes are ready. */
	held?: boolean;
	/** A request to the AI prompt under way, or why it failed. */
	asking?: string | null;
	/** A connector holds its size (see `Call::TakeSize`). */
	sizeHeld?: boolean;
}

/** How a tab's panes are split. */
export type Layout =
	| { type: "Pane", content: string }
	| { type: "Split", content: Split };

export type Divider =
	| "none"
	| "space"
	| "row";

export interface Tab {
	id: string;
	/** The name the user gave it, if any. */
	name?: string | null;
	/**
	 * What to call it: the name, else what Shed shows (the program, the
	 * folder, the project's or session's name).
	 */
	title: string;
	tags?: Tag[];
	private?: boolean;
	kind: TabKind;
	/** Terminal and remote tabs: the panes, and how they are split. */
	panes?: Pane[];
	layout?: Layout | null;
	activePane?: string | null;
	/** Starts a group on the tab bar. */
	divider?: Divider;
}

export interface Link {
	name: string;
	url: string;
}

export interface Bench {
	id: string;
	name: string;
	color: string;
	badge?: string | null;
	/** Where new tabs open. */
	cwd: string;
	tabs: Tab[];
	activeTab: string;
	/** A window shows it on the workspace. */
	shown?: boolean;
	/** Being closed: its teardown runs. */
	closing?: boolean;
	/** Its startup ran since it opened. */
	started?: boolean;
	links?: Link[];
}

export interface BenchOnly {
	bench: string;
}

export interface Branches {
	current?: string | null;
	local: string[];
	remote: string[];
}

export interface Browse {
	/** Absolute, or `~/…`; `""`: home. */
	path: string;
}

export interface FolderEntry {
	name: string;
	dir: boolean;
	/** Left out by the tab's excludes or Git's ignore rules (shown dimmed). */
	ignored?: boolean;
}

/** A folder's folders (`Call::Browse`). */
export interface Browsed {
	/** Where it looked, absolute (`~` expanded, no `..`). */
	path: string;
	/** The folder above, if any. */
	parent?: string | null;
	/** Its folders (hidden ones left out), by name. */
	entries: FolderEntry[];
}

export interface ChangePassword {
	old: string;
	new: string;
	totp: string;
}

export interface ClientHello {
	api: number;
	/** What the connector is, for logs: `web 0.1.0`, `android 0.1.0`. */
	client?: string;
}

export interface ClosedBench {
	name: string;
	color: string;
	badge?: string | null;
	cwd: string;
	/** Its tabs as they will load (panes without a screen). */
	tabs: Tab[];
	/** When it was saved, Unix seconds. */
	saved: number;
	links?: Link[];
}

export interface ClosedName {
	name: string;
}

export interface CommandTab {
	folder: string;
	command: string;
}

export interface Commit {
	hash: string;
	short: string;
	subject: string;
	author: string;
	/** Unix seconds. */
	time: number;
}

export interface CommitFile {
	status: string;
	path: string;
}

export interface Cursor {
	/**
	 * Row and column in cells of the grid (a wide character takes two
	 * columns, and once in a span's `text`).
	 */
	row: number;
	col: number;
	visible: boolean;
}

export type DeviceKind =
	/** A browser: gets a session cookie. */
	| "browser"
	/** The Android app: gets a device token. */
	| "phone"
	/** A Shed workspace (paired, not signed in). */
	| "workspace"
	/**
	 * Shed's desktop app signed in to show other workspaces (`shed nexus
	 * signin`): gets a device token, like a phone, and may open tunnels
	 * to workspaces that allow desktop clients.
	 */
	| "desktop";

export interface Device {
	id: string;
	kind: DeviceKind;
	name: string;
	/** Unix seconds. */
	created: number;
	seen: number;
	/** Where it was last seen from: the address, and its country if known. */
	address?: string;
	country?: string | null;
	/** The device asking. */
	current?: boolean;
	/** Connected now. */
	online?: boolean;
}

/** The device signing in. */
export interface DeviceHello {
	kind: DeviceKind;
	/** "Firefox on Linux", "Pixel 8". */
	name: string;
}

export type DiffLineKind =
	| "context"
	| "added"
	| "removed";

export interface DiffLine {
	kind: DiffLineKind;
	text: string;
	/** Line numbers, old and new side. */
	old?: number | null;
	new?: number | null;
}

export interface Hunk {
	/** `@@ -12,7 +12,9 @@`. */
	header: string;
	lines: DiffLine[];
}

export interface Diff {
	hunks: Hunk[];
	/** A picture changed: both sides, when they are pictures. */
	oldPicture?: Picture | null;
	newPicture?: Picture | null;
	/** Binary, not shown. */
	binary?: boolean;
}

export interface Ended {
	sub: number;
	error?: ApiError | null;
}

export type Event =
	| { type: "AiTranscript", content: AiTranscript }
	| { type: "AiInfo", content: AiInfo }
	| { type: "Screen", content: Screen }
	| { type: "FilesChanged", content: FilesChanged }
	| { type: "GitChanged", content?: undefined }
	| { type: "SearchHits", content: SearchHits }
	| { type: "SearchDone", content: SearchDone }
	/**
	 * Entries of a conversation changed in place, each alone (a sub-agent's
	 * progress while the conversation goes on), and its status.
	 */
	| { type: "AiEntries", content: AiEntriesChanged }
	/**
	 * Never sent: connectors read an event they do not know (a newer
	 * workspace's) as this, and ignore it.
	 */
	| { type: "Unknown", content?: undefined };

export interface EventFrame {
	/** The subscription's id. */
	sub: number;
	event: Event;
}

export interface Explanation {
	key: string;
	text: string;
}

export type FileKind =
	| "text"
	| "picture"
	| "binary"
	| "tooLarge";

/**
 * A coloured range in a line of code, and what it is. `start` and `end`
 * are positions in the line in UTF-16 code units (as JavaScript and Kotlin
 * strings index).
 */
export interface Highlight {
	start: number;
	end: number;
	/**
	 * `keyword`, `string`, `comment`, `function`, `type`, `number`,
	 * `constant`, `property`, `operator`, `tag`, `attribute`… each
	 * connector colours these by its theme.
	 */
	kind: string;
}

export interface FileContent {
	path: string;
	kind: FileKind;
	/** For text: the whole of it. */
	text?: string;
	/** The language's name, if known. */
	lang?: string | null;
	/** Per line, its coloured ranges. */
	highlights?: Highlight[][];
	/** For a picture: the picture. */
	picture?: Picture | null;
	/** Bytes. */
	size: number;
}

export interface FileMatch {
	path: string;
	/** Positions in `path` that matched, to highlight, in UTF-16 code units. */
	hits: number[];
}

export interface FilesChanged {
	/** Relative to the root; files and folders made, changed or removed. */
	paths: string[];
}

export interface Folder {
	folder: string;
}

export interface GitBranch {
	tab: string;
	branch: string;
}

export interface GitCommit {
	tab: string;
	message: string;
	amend?: boolean;
}

export interface GitCommitRef {
	tab: string;
	hash: string;
}

export type DiffSource =
	/** Index against HEAD. */
	| { type: "Staged", content?: undefined }
	/** Work tree against the index. */
	| { type: "Unstaged", content?: undefined }
	/** What a commit changed. */
	| { type: "Commit", content: string };

export interface GitDiff {
	tab: string;
	path: string;
	source: DiffSource;
}

export interface GitFile {
	path: string;
	/** Index and work-tree status letters, as `git status --porcelain`. */
	staged: string;
	unstaged: string;
	conflict?: boolean;
	/** Renamed from. */
	from?: string | null;
}

export type HunkAction =
	| "stage"
	| "unstage"
	| "discard";

export interface GitHunk {
	tab: string;
	path: string;
	source: DiffSource;
	/**
	 * The hunk's index in the diff, and its header (so a stale index does
	 * nothing).
	 */
	hunk: number;
	header: string;
	action: HunkAction;
}

export interface GitLog {
	tab: string;
	/** One file's history; `None`: the repository's. */
	path?: string | null;
	limit?: number;
}

export interface GitPaths {
	tab: string;
	paths: string[];
}

export interface GitPush {
	tab: string;
	force?: boolean;
}

export interface GitStatus {
	/** Not a repository: everything else is empty. */
	repo: boolean;
	branch?: string | null;
	upstream?: string | null;
	ahead?: number;
	behind?: number;
	/** A merge, rebase or the like under way. */
	operation?: string | null;
	files: GitFile[];
	/** Commits not pushed yet. */
	outgoing?: Commit[];
}

export type InputItem =
	/** Typed text (each character as a key press without modifiers). */
	| { type: "Text", content: string }
	/** A key with modifiers. */
	| { type: "Key", content: Key }
	/** Pasted text (bracketed when the program asks for it). */
	| { type: "Paste", content: string }
	/**
	 * The mouse, at a cell of the screen. Sent on to the program only
	 * while it asks for mouse events (`Screen::mouse`), encoded as it
	 * asked (X10, SGR); otherwise dropped.
	 */
	| { type: "Mouse", content: Mouse };

export interface Input {
	pane: string;
	input: InputItem[];
}

export interface Key {
	/**
	 * A named key (`Enter`, `Tab`, `Backspace`, `Escape`, `Up`, `Down`,
	 * `Left`, `Right`, `Home`, `End`, `PageUp`, `PageDown`, `Insert`,
	 * `Delete`, `F1`…`F12`) or one character (`a`, `/`).
	 */
	key: string;
	ctrl?: boolean;
	alt?: boolean;
	shift?: boolean;
}

export interface LimitWindow {
	/** 0 to 1. */
	used: number;
	/** When it resets. */
	resets: number;
}

export interface Limits {
	fiveHour?: LimitWindow | null;
	week?: LimitWindow | null;
	/** When Claude Code reported them. */
	seen?: number;
}

/** What a workspace lets connectors do (`[nexus] control` in `shed.toml`). */
export type Policy =
	/** Look only: calls that change something are refused (`Forbidden`). */
	| "view"
	/** Everything. */
	| "full";

export interface LinkHello {
	api: number;
	/** Shed's version on the workspace. */
	version: string;
	policy: Policy;
}

export interface LinkWelcome {
	api: number;
	nexus: string;
	/** The workspace's id and name at the nexus. */
	workspace: string;
	name: string;
}

export interface LoadBench {
	name: string;
	start?: boolean;
}

export interface LogEntry {
	command: string;
	cwd: string;
	exit?: number | null;
	/** Unix seconds. */
	started: number;
	/** Seconds. */
	duration: number;
	bench?: string;
	tab?: string;
}

export type LogScope =
	| "tab"
	| "bench"
	| "all";

export interface LogSearch {
	scope: LogScope;
	/** The tab (and its bench) the tab and bench scopes are about. */
	tab?: string | null;
	/** Words to find; may hold `tag:label`. */
	query?: string;
	limit?: number;
}

export interface Login {
	username: string;
	password: string;
	/** A code from the authenticator, or a recovery code. */
	totp: string;
	device: DeviceHello;
}

export type MouseKind =
	| "press"
	| "release"
	| "move"
	/** One notch of the wheel, up (away from the user) or down. */
	| "wheelUp"
	| "wheelDown";

export type MouseButton =
	| "left"
	| "middle"
	| "right";

export interface Mouse {
	kind: MouseKind;
	/**
	 * The button pressed or released, or held while moving (`None`: no
	 * button held; a press or release without one is the left button).
	 * Not used for the wheel.
	 */
	button?: MouseButton | null;
	/** 0-based, in cells of the screen (`Screen::cols` by `Screen::rows`). */
	col: number;
	row: number;
	ctrl?: boolean;
	alt?: boolean;
	shift?: boolean;
}

/** A running bench by id, or a closed one by name. */
export type BenchRef =
	| { type: "Running", content: string }
	| { type: "Closed", content: string };

export interface MoveTab {
	tab: string;
	from: BenchRef;
	to: BenchRef;
	index: number;
}

export interface NewBench {
	name?: string | null;
	cwd?: string | null;
}

export type NewTabKind =
	/** A shell, in a folder (the bench's by default). */
	| { type: "Terminal", content: OptionalFolder }
	/** A shell that runs a command. */
	| { type: "Command", content: CommandTab }
	| { type: "Project", content: Folder }
	| { type: "Ai", content: Folder }
	/** A shell on a server; `join` an existing one by id. */
	| { type: "Remote", content: RemoteTabArgs };

export interface NewTab {
	bench: BenchRef;
	kind: NewTabKind;
}

export interface NexusStatus {
	version: string;
	api: number;
	/** The account is set up (otherwise the web app shows the setup). */
	ready: boolean;
	/** Whoever asks is signed in. */
	signedIn: boolean;
}

export type NotificationKind =
	/** An AI tab waits for a permission or an answer. */
	| "aiWaiting"
	/** An AI tab finished a turn. */
	| "aiDone"
	/** A command that ran long finished. */
	| "commandDone"
	/** A workspace went offline (sent by the nexus itself). */
	| "workspaceOffline";

/**
 * Something that happened in a workspace that the user may want to hear
 * about. While a connector is in view the nexus sends it live to the
 * connectors (`ServerFrame::Notification`); otherwise it pushes it: to a
 * ntfy topic if it has one (`PushInfo::ntfy_topic`), and as Web Push, a
 * `http::PushPayload` (this, with the workspace's id and name added), to
 * the browsers (and, without a topic, the phones) that want its kind.
 */
export interface Notification {
	kind: NotificationKind;
	/** One line: "Claude waits for an answer". */
	title: string;
	/** More, if any: the bench and tab, the question. */
	body?: string;
	/** Where it happened, to open it from the notification. */
	bench?: string | null;
	tab?: string | null;
}

export interface NotificationFrame {
	/** The workspace's id (`WorkspaceInfo::id`). */
	workspace: string;
	notification: Notification;
}

export interface OptionalFolder {
	folder?: string | null;
}

export interface PairApprove {
	/**
	 * The code `shed nexus join` printed on the workspace, as the person
	 * typed it (case, spaces and dashes do not matter).
	 */
	code: string;
	/** The name to give it (may differ from the one it asked for). */
	name: string;
	totp: string;
}

export interface PairApproved {
	workspace: string;
	name: string;
	token: string;
}

export interface PairPoll {
	poll: string;
}

export interface PairReject {
	/** The request's code, if known; else empty and `id` set. */
	code?: string;
	/** The request's id from `PendingPairing`. */
	id?: string;
}

/** A workspace asks to be paired. */
export interface PairStart {
	/** The name it would like (its host name). */
	name: string;
	version: string;
}

export interface PairStarted {
	/** What the person types in the web app to approve it: `K7QF-2M9D`. */
	code: string;
	/** What the workspace polls with; secret. */
	poll: string;
	/** Unix seconds; the request is gone after. */
	expires: number;
}

export interface PaneOnly {
	pane: string;
}

/**
 * A pairing request waiting, for information: which workspace asks and
 * from where. It carries no code: approving takes the code `shed nexus
 * join` printed, typed by the person, so a request someone else made
 * cannot be approved by mistake.
 */
export interface PendingPairing {
	/** Always empty (kept for older apps); see `PairApprove::code`. */
	code: string;
	/** Names it to reject it (`PairReject::id`). */
	id?: string;
	name: string;
	version: string;
	address: string;
	country?: string | null;
	expires: number;
}

export interface Preview {
	pane: string;
	lines: string[];
}

export interface ProjectPath {
	/** A project or AI tab. */
	tab: string;
	/** Relative to its root; `""` is the root. */
	path: string;
}

export interface ProjectTab {
	root: string;
	/** Files open in the GUI, relative to the root. */
	open?: string[];
	active?: number | null;
	links?: Link[];
}

export interface ProjectTopic {
	tab: string;
}

export interface PushPrefs {
	/** The kinds this device wants. */
	kinds: NotificationKind[];
}

/** How this nexus pushes. */
export interface PushInfo {
	/** The VAPID public key for Web Push (base64url), if Web Push is on. */
	vapidKey?: string | null;
	/** This device's choices. */
	prefs: PushPrefs;
	/**
	 * This device has a subscription (`push/subscribe`), so pushes reach
	 * it. False once it unsubscribed, or after the push service said the
	 * subscription is gone.
	 */
	subscribed?: boolean;
	/**
	 * The ntfy topic the nexus publishes to when no app is in view
	 * (`https://ntfy.example.com/alerts-shed`), if it has one: phones
	 * subscribe to it in the ntfy app, and get no UnifiedPush then.
	 */
	ntfyTopic?: string | null;
}

/**
 * What a push carries, once decrypted: a workspace's `Notification` with
 * the workspace it came from, added by the nexus. A connector opens
 * `/w/{workspace}/b/{bench}/t/{tab}` from it (as far as those are given).
 */
export interface PushPayload {
	kind: NotificationKind;
	title: string;
	body?: string;
	/** The workspace's id (`WorkspaceInfo::id`). */
	workspace: string;
	/** Its name at the nexus. */
	workspaceName?: string;
	bench?: string | null;
	tab?: string | null;
}

/**
 * Where to push to this device: a Web Push subscription (browser), or a
 * UnifiedPush endpoint (Android, through the distributor, such as ntfy).
 * Both are Web Push style: an endpoint and the keys to encrypt for.
 */
export interface PushSubscribe {
	endpoint: string;
	/** The receiver's public key (base64url, P-256). */
	p256dh: string;
	/** The shared auth secret (base64url). */
	auth: string;
}

export interface QuickOpen {
	tab: string;
	query: string;
	limit?: number;
}

export interface RemoteTab {
	host: string;
	id: string;
}

export interface RemoteTabArgs {
	host: string;
	join?: string | null;
}

export interface RenameBench {
	bench: BenchRef;
	name: string;
}

export interface RenameDevice {
	device: string;
	name: string;
}

export interface RenameTab {
	bench: BenchRef;
	tab: string;
	/** `None`: back to the automatic title. */
	name?: string | null;
}

export type Call =
	/** A new running bench. → `Bench` */
	| { type: "NewBench", content: NewBench }
	/** Loads a closed bench; `start` runs its startup too. → `Bench` */
	| { type: "LoadBench", content: LoadBench }
	/** Closes a running bench (it is saved; teardown runs). → `Done` */
	| { type: "CloseBench", content: BenchOnly }
	/** Deletes a closed bench's file. → `Done` */
	| { type: "DeleteBench", content: ClosedName }
	/** Saves a running bench now. → `Done` */
	| { type: "SaveBench", content: BenchOnly }
	| { type: "RunStartup", content: BenchOnly }
	| { type: "RunTeardown", content: BenchOnly }
	/** Running or closed. → `Done` */
	| { type: "RenameBench", content: RenameBench }
	| { type: "SetBenchColor", content: SetBenchColor }
	| { type: "SetBadge", content: SetBadge }
	/**
	 * The last lines of every tab's active pane, running and closed, for
	 * cards. → `Previews`
	 */
	| { type: "Previews", content?: undefined }
	/** → `Tab` */
	| { type: "NewTab", content: NewTab }
	| { type: "CloseTab", content: TabOnly }
	| { type: "RenameTab", content: RenameTab }
	| { type: "SetTags", content: SetTags }
	| { type: "SetPrivate", content: SetPrivate }
	/** Into a bench (running), at an index. */
	| { type: "MoveTab", content: MoveTab }
	| { type: "SetDivider", content: SetDivider }
	/** Makes it the bench's active tab (the GUI follows). */
	| { type: "FocusTab", content: TabOnly }
	/**
	 * Makes a remote tab drop its connection and join its shell again at
	 * once (the shell on the server keeps running); one whose connecting
	 * was stopped (Ctrl+C) starts it again. → `Done`
	 */
	| { type: "ReconnectRemote", content: TabOnly }
	/** → `Pane` */
	| { type: "Split", content: SplitPane }
	| { type: "ClosePane", content: PaneOnly }
	| { type: "FocusPane", content: PaneOnly }
	/** Keys, text or a paste, encoded for the program as the GUI would. */
	| { type: "Input", content: Input }
	/**
	 * Resizes the PTY to this connector's screen while it shows the pane;
	 * `ReleaseSize` (or going away) gives the size back.
	 */
	| { type: "TakeSize", content: TakeSize }
	| { type: "ReleaseSize", content: PaneOnly }
	/**
	 * Lines of scrollback, oldest first: `from` counts back from the
	 * screen's top (0: the line just above it). → `Lines`
	 */
	| { type: "Scrollback", content: Scrollback }
	/** Runs a held startup now. */
	| { type: "StartNow", content: PaneOnly }
	/** Stops the AI prompt's request under way. */
	| { type: "CancelAsk", content: PaneOnly }
	/** → `Log` */
	| { type: "LogSearch", content: LogSearch }
	/** The folder's sessions, with draft sessions. → `Sessions` */
	| { type: "AiSessions", content: TabOnly }
	/** Shows and continues a session; `None`: a new one. → `Done` */
	| { type: "AiOpenSession", content: AiOpenSession }
	| { type: "AiRenameSession", content: AiRenameSession }
	/**
	 * Deletes a Claude Code session and the sessions going on from it
	 * (their files go to Shed's trash, kept 30 days). Refused while another
	 * AI tab shows one of them; the tab, showing one, goes to a new
	 * session. → `Done`
	 */
	| { type: "AiDeleteSession", content: AiDeleteSession }
	/** Sends a prompt (queued if a turn is under way). → `Done` */
	| { type: "AiSend", content: AiSend }
	| { type: "AiStop", content: TabOnly }
	| { type: "AiAnswer", content: AiAnswer }
	| { type: "AiAnswerQuestions", content: AiAnswerQuestions }
	/**
	 * Edits an earlier prompt and sends it again (the conversation goes on
	 * from there).
	 */
	| { type: "AiEdit", content: AiEdit }
	/**
	 * Goes back to before a prompt: the conversation, and the files too if
	 * `files`.
	 */
	| { type: "AiRevert", content: AiRevert }
	/** Changes or sends a queued message. */
	| { type: "AiQueued", content: AiQueued }
	/** Model, mode, effort, follow, browser: only the fields given change. */
	| { type: "AiSettings", content: AiSettings }
	/** Sets the prompt's unsent text. */
	| { type: "AiSetUnsent", content: AiSetUnsent }
	| { type: "AiAddDraft", content: AiAddDraft }
	| { type: "AiRemoveDraft", content: AiRemoveDraft }
	/** A picture of the conversation (by the name in an entry). → `Picture` */
	| { type: "AiPicture", content: AiPictureName }
	/** The changes Claude made, for review. → `Review` */
	| { type: "AiReview", content: TabOnly }
	| { type: "AiSetViewed", content: AiSetViewed }
	/** Asks, aside, for an explanation of changes. → `Explained` */
	| { type: "AiExplain", content: AiExplain }
	/** Starts Claude Code for the tab ahead of the first prompt. */
	| { type: "AiStart", content: TabOnly }
	/**
	 * Keeps a file on the workspace for a prompt to attach (by its path, in
	 * the text): at most `MAX_UPLOAD` bytes, in a folder of the tab's under
	 * Shed's state folder, deleted after a while. → `Text` (its path)
	 */
	| { type: "AiUpload", content: AiUpload }
	/** A folder's entries. → `Folder` */
	| { type: "ListFolder", content: ProjectPath }
	/** Files whose names match, best first. → `Files` */
	| { type: "QuickOpen", content: QuickOpen }
	/** → `File` */
	| { type: "ReadFile", content: ProjectPath }
	/** Writes a file whole (light edits). → `Done` */
	| { type: "WriteFile", content: WriteFile }
	/** Makes a file, or a folder (a path ending in `/`). */
	| { type: "CreateEntry", content: ProjectPath }
	/** → `GitStatus` */
	| { type: "GitStatus", content: TabOnly }
	/** A file's changes. → `Diff` */
	| { type: "GitDiff", content: GitDiff }
	| { type: "GitStage", content: GitPaths }
	| { type: "GitUnstage", content: GitPaths }
	/** Throws away changes (asks first, in the apps). */
	| { type: "GitDiscard", content: GitPaths }
	/** Stages, unstages or discards one hunk of `GitDiff`'s answer. */
	| { type: "GitHunk", content: GitHunk }
	/** → `Text` (the commit's hash) */
	| { type: "GitCommit", content: GitCommit }
	/** → `Commits` */
	| { type: "GitLog", content: GitLog }
	/** → `CommitFiles` */
	| { type: "GitCommitFiles", content: GitCommitRef }
	/** → `Branches` */
	| { type: "GitBranches", content: TabOnly }
	| { type: "GitSwitch", content: GitBranch }
	| { type: "GitCreateBranch", content: GitBranch }
	/** → `Text` (what git said) */
	| { type: "GitFetch", content: TabOnly }
	| { type: "GitPull", content: TabOnly }
	| { type: "GitPush", content: GitPush }
	/**
	 * Folders under a path, to choose where a tab works. → `Folder`.
	 * Refused (`Forbidden`) by a view-only workspace, which makes no tabs.
	 * Kept for older apps: `Browse` says where it looked too.
	 */
	| { type: "BrowseFolders", content: Browse }
	/**
	 * Folders under a path, and the path as an absolute one (`~` and `""`
	 * made into the home folder), to show and to go up from. → `Browsed`.
	 * Refused by a view-only workspace, as `BrowseFolders`.
	 */
	| { type: "Browse", content: Browse };

/** A call on a workspace. Answered by one `Response` with the same id. */
export interface Request {
	/** Chosen by the sender, unique among its requests in flight. */
	id: number;
	/** The workspace's id (from `WorkspaceInfo`); ignored on `/link`. */
	workspace?: string;
	call: Call;
}

export type Reply =
	| { type: "Done", content?: undefined }
	/** A bench's id. */
	| { type: "Bench", content: string }
	/** A tab's id. */
	| { type: "Tab", content: string }
	/** A pane's id. */
	| { type: "Pane", content: string }
	| { type: "Previews", content: Preview[] }
	| { type: "Lines", content: ScreenLine[] }
	| { type: "Log", content: LogEntry[] }
	| { type: "Sessions", content: AiSession[] }
	| { type: "Picture", content: Picture }
	| { type: "Review", content: Review }
	| { type: "Explained", content: Explanation[] }
	| { type: "Folder", content: FolderEntry[] }
	| { type: "Files", content: FileMatch[] }
	| { type: "File", content: FileContent }
	| { type: "GitStatus", content: GitStatus }
	| { type: "Diff", content: Diff }
	| { type: "Commits", content: Commit[] }
	| { type: "CommitFiles", content: CommitFile[] }
	| { type: "Branches", content: Branches }
	| { type: "Text", content: string }
	| { type: "Browsed", content: Browsed };

export interface Response {
	id: number;
	/** The answer, when the call worked. */
	ok?: Reply | null;
	/** Why not, when it did not. */
	error?: ApiError | null;
}

export interface ReviewChange {
	/**
	 * Stable while the change is the same: what `AiSetViewed` and
	 * `AiExplain` name.
	 */
	key: string;
	/** Relative to the repository. */
	path: string;
	/** `uncommitted`, or the commit it is in. */
	layer: string;
	/** `M`, `A`, `D`, `R`, `?`. */
	status: string;
	viewed: boolean;
	explanation?: string | null;
	diff: Diff;
}

export interface ReviewRepo {
	root: string;
	/** Commits made since the session started. */
	commits?: Commit[];
	changes: ReviewChange[];
}

/** What Claude changed, by repository. */
export interface Review {
	repos: ReviewRepo[];
}

export interface Revoke {
	device: string;
	totp: string;
}

/** Text with one style. Wide characters take two cells; `text` has them once. */
export interface Span {
	text: string;
	/** `#rrggbb`, or a palette index as `0`…`255`; `None`: the default. */
	fg?: string | null;
	bg?: string | null;
	bold?: boolean;
	dim?: boolean;
	italic?: boolean;
	underline?: boolean;
	strike?: boolean;
	inverse?: boolean;
}

export interface ScreenLine {
	spans: Span[];
	/** It goes on in the next row (it was wrapped, not ended). */
	wraps?: boolean;
}

export interface ScreenRow {
	row: number;
	line: ScreenLine;
}

/** A terminal's screen, or the lines of it that changed. */
export interface Screen {
	/**
	 * The whole screen (first event, a resize, a switch of screens): rows
	 * not in `lines` are blank. Otherwise only changed rows are given.
	 */
	full: boolean;
	cols: number;
	rows: number;
	/**
	 * The program uses the alternate screen (vim, htop): show the grid as is
	 * rather than re-wrapping lines to the connector's width.
	 */
	alt?: boolean;
	lines: ScreenRow[];
	cursor: Cursor;
	title?: string | null;
	/** How many lines of scrollback there are (`Call::Scrollback`). */
	scrollback?: number;
	/** The program wants mouse events. */
	mouse?: boolean;
	/**
	 * Lines scrolled off the top since the last event (so a connector
	 * showing scrollback can keep its place).
	 */
	scrolled?: number;
}

export interface ScreenTopic {
	pane: string;
}

export interface Scrollback {
	pane: string;
	from: number;
	count: number;
}

export interface SearchDone {
	/** Stopped at the limit: there may be more. */
	truncated?: boolean;
	error?: string | null;
}

export interface SearchHit {
	/** 1-based. */
	line: number;
	text: string;
	/**
	 * Ranges in `text` that matched: start, end, start, end…, in UTF-16
	 * code units.
	 */
	ranges: number[];
}

export interface SearchHits {
	path: string;
	hits: SearchHit[];
}

export interface SearchTopic {
	tab: string;
	query: string;
	regex?: boolean;
	caseSensitive?: boolean;
	wholeWord?: boolean;
	/** Globs of files to search in, and to leave out. */
	include?: string | null;
	exclude?: string | null;
}

export interface SetBadge {
	bench: BenchRef;
	badge?: string | null;
}

export interface SetBenchColor {
	bench: BenchRef;
	/** A named colour (`blue`, `coral`…) or `#rrggbb`. */
	color: string;
}

export interface SetDivider {
	tab: string;
	divider: Divider;
}

export interface SetPrivate {
	bench: BenchRef;
	tab: string;
	private: boolean;
}

export interface SetTags {
	bench: BenchRef;
	tab: string;
	tags: Tag[];
}

/** The second step: a code from the authenticator, proving it was added. */
export interface SetupConfirm {
	code: string;
	totp: string;
	device: DeviceHello;
}

/**
 * The first step of setting up the account: the setup code (the setup
 * link's `#code=` fragment), a user name and a password.
 */
export interface SetupStart {
	code: string;
	username: string;
	password: string;
}

/** The TOTP secret to add to an authenticator app. */
export interface SetupTotp {
	/** Base32, to type in. */
	secret: string;
	/** `otpauth://totp/…`, to show as a QR code. */
	url: string;
}

export interface SignedIn {
	username: string;
	/** This device's id. */
	device: string;
	/**
	 * For a phone or a desktop: its device token, to keep and send as a
	 * bearer token.
	 */
	token?: string | null;
	/** For a browser: the token to send in `X-Shed-Csrf` with changes. */
	csrf?: string | null;
	/**
	 * Single-use codes to sign in without the authenticator; only given at
	 * setup and when new ones are made.
	 */
	recoveryCodes?: string[];
}

export type SplitDir =
	/** Side by side. */
	| "right"
	/** One above the other. */
	| "down";

export interface Split {
	dir: SplitDir;
	/** The first part's share, 0 to 1. */
	ratio: number;
	first: Layout;
	second: Layout;
}

export interface SplitPane {
	pane: string;
	dir: SplitDir;
	folder?: string | null;
}

export interface WorkspaceState {
	/** The machine's host name. */
	host: string;
	/** Running benches, in the order the user arranged them. */
	benches: Bench[];
	/** Saved benches that are not running, by name. */
	closed: ClosedBench[];
	/** The account's usage limits, as Claude Code last reported them here. */
	limits?: Limits | null;
	/** The user's home folder, to show paths as `~/…`. */
	home?: string;
}

export interface StateFrame {
	workspace: string;
	state: WorkspaceState;
}

export type Topic =
	/** An AI tab's conversation: `AiTranscript`, `AiInfo`. */
	| { type: "Ai", content: AiTopic }
	/** A terminal pane's screen: `Screen`. */
	| { type: "Screen", content: ScreenTopic }
	/**
	 * A project's files and Git changing on disk: `FilesChanged`,
	 * `GitChanged`.
	 */
	| { type: "Project", content: ProjectTopic }
	/**
	 * Searching a project's files: `SearchHits` as they are found, then
	 * `SearchDone`. Unsubscribing stops it.
	 */
	| { type: "Search", content: SearchTopic };

/**
 * Starts a stream of events about something in a workspace. The first event
 * is the whole of it (a transcript, a screen), the next ones its changes.
 */
export interface Subscribe {
	/** Chosen by the sender; events carry it, `Unsubscribe` names it. */
	id: number;
	workspace?: string;
	topic: Topic;
}

export interface TabOnly {
	tab: string;
}

export interface TakeSize {
	pane: string;
	cols: number;
	rows: number;
}

/** A tunnel ended (or never opened): `error` says why, if it was not asked for. */
export interface TunnelClosed {
	id: number;
	error?: ApiError | null;
}

export interface TunnelId {
	id: number;
}

/** Opens a tunnel to `workspace`'s keeper under `id`. */
export interface TunnelOpen {
	/** Chosen by the sender, unique among its open tunnels. */
	id: number;
	workspace: string;
}

export interface Unsubscribe {
	id: number;
}

/** A workspace as the nexus knows it. */
export interface WorkspaceInfo {
	id: string;
	/** Its name, given at pairing (the machine's host name by default). */
	name: string;
	online: boolean;
	/** When it was last connected (Unix seconds), or connected since. */
	seen: number;
	/** What it lets connectors do. */
	policy: Policy;
	/** Its Shed version, `0.1.0`. */
	version?: string;
}

export interface Welcome {
	api: number;
	/** The nexus's version, `0.1.0`. */
	nexus: string;
	workspaces: WorkspaceInfo[];
	/** The last state of each workspace that has one (offline ones too). */
	states: StateFrame[];
}

export interface WriteFile {
	tab: string;
	path: string;
	text: string;
}

/** From a connector to the nexus. */
export type ClientFrame =
	/** First frame: the API version the connector speaks. */
	| { type: "Hello", content: ClientHello }
	| { type: "Request", content: Request }
	| { type: "Subscribe", content: Subscribe }
	| { type: "Unsubscribe", content: Unsubscribe }
	/**
	 * Whether the connector is in view (a browser tab shown and focused,
	 * the Android app in the foreground). Sent after `Hello` and on every
	 * change; a connection that never sends it counts as not in view. While
	 * any connector is in view, notifications come to all of them live
	 * (`ServerFrame::Notification`) and nothing is pushed.
	 */
	| { type: "Visible", content: boolean }
	/** Opens a tunnel to a workspace's keeper (desktop connectors only). */
	| { type: "TunnelOpen", content: TunnelOpen }
	/** Closes one of this connector's tunnels. */
	| { type: "TunnelClose", content: TunnelId };

/** From a workspace's link to the nexus. */
export type LinkFrame =
	/** First frame. */
	| { type: "Hello", content: LinkHello }
	/**
	 * The workspace's state, whole, whenever it changes (at most a few times
	 * a second). The nexus keeps the last one.
	 */
	| { type: "State", content: WorkspaceState }
	| { type: "Response", content: Response }
	| { type: "Event", content: EventFrame }
	| { type: "Ended", content: Ended }
	/** Something worth a notification. */
	| { type: "Notify", content: Notification }
	/** A tunnel the nexus asked for is open (connected to the keeper). */
	| { type: "TunnelOpened", content: TunnelId }
	/**
	 * A tunnel ended, or was refused (`Forbidden` when the workspace does
	 * not allow desktop clients).
	 */
	| { type: "TunnelClosed", content: TunnelClosed };

/** From the nexus to a link. Ids are the nexus's own. */
export type NexusFrame =
	| { type: "Welcome", content: LinkWelcome }
	| { type: "Request", content: Request }
	| { type: "Subscribe", content: Subscribe }
	| { type: "Unsubscribe", content: Unsubscribe }
	/** Closing: the workspace was removed, or its API version is not accepted. */
	| { type: "Goodbye", content: ApiError }
	/** A desktop connector wants a tunnel to the keeper, under this id. */
	| { type: "TunnelOpen", content: TunnelId }
	/** The tunnel's connector went, or closed it. */
	| { type: "TunnelClose", content: TunnelId };

export type PairState =
	| { type: "Pending", content?: undefined }
	/** Approved: the workspace's id and its token, given once. */
	| { type: "Approved", content: PairApproved }
	| { type: "Rejected", content?: undefined }
	| { type: "Expired", content?: undefined };

/** From the nexus to a connector. */
export type ServerFrame =
	/** Answers `Hello`: the workspaces, each with its last known state. */
	| { type: "Welcome", content: Welcome }
	| { type: "Response", content: Response }
	| { type: "Event", content: EventFrame }
	/**
	 * A subscription ended without being asked to (the workspace went
	 * offline, the tab closed); subscribe again to resume.
	 */
	| { type: "Ended", content: Ended }
	/** A workspace came, went, or was renamed or removed: the whole list. */
	| { type: "Workspaces", content: WorkspaceInfo[] }
	/** A workspace's state changed: all of it (it is small). */
	| { type: "State", content: StateFrame }
	/**
	 * The connection is being closed: signed out, revoked, or the API
	 * version is not accepted. Not to be retried as is.
	 */
	| { type: "Goodbye", content: ApiError }
	/**
	 * A notification, live: sent to every connector while one is in view
	 * (`ClientFrame::Visible`), instead of a push. The app shows it itself.
	 */
	| { type: "Notification", content: NotificationFrame }
	/** A tunnel this connector asked for is open: data may flow. */
	| { type: "TunnelOpened", content: TunnelId }
	/** A tunnel ended, or was refused. */
	| { type: "TunnelClosed", content: TunnelClosed };

export const MAX_UPLOAD: number = 16777216;
