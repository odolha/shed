# Shed as a working environment: plan

Shed today starts a project's terminals and keeps them. This plan turns a bench
into everything needed to work on a project: its terminals, its code (to read,
search and lightly edit, with Git), AI sessions, links to its web apps, and
shells on remote servers that stay alive. It records what is feasible, the
design choices and their reasons, and the order of work.

Status: agreed 2026-11-11 (see [Decisions](#decisions)); **done** by 2026-11-16.
Every step below is in place: tab kinds, the text core, files and the editor,
search, AI tabs (native), Git (changes, commits, branches, history, blame,
conflicts), links with the browser extension, remote tabs shared between PCs,
reviewing a turn's changes, and notifications. Much grew beyond the plan along
the way (drafts and a queue kept across restarts, sessions read whole across
Claude Code's files, usage limits with a pace, tab groups, backups). This file
now records how Shed got here and why; [Still open](#still-open) is what is left.
The [User Guide](user-guide.md) describes it all as it is.

## The picture

A bench keeps its tabs. A tab gets a **kind**, shown by an icon in the tab bar and
on the pegboard:

| Kind | What it is |
|---|---|
| Terminal | As now: panes, each a local shell. |
| Project | A folder of code: file tree, editor, search, Git. Links to its web apps sit in its header. |
| AI | Claude Code sessions, in a tab of their own: it works in one folder and can be given the bench's other project folders too. |
| Remote | A shell on a server, kept alive there by a remote keeper. |

Loading a bench then does all of it: local servers start in terminal tabs,
project tabs open where they were (files, scroll positions), AI tabs show their
sessions, remote tabs reattach.

## Feasibility at a glance

| Wish | Verdict | Notes |
|---|---|---|
| Project tabs: browse, open by name, search in files | Feasible, fast | No editor widget exists in GPUI; the editor is ours. See [Code](#3-code-project-tabs). |
| Syntax colours for the common languages | Feasible | tree-sitter grammars, compiled in. |
| Light editing from the start | Feasible | One text core serves the editor, the commit message and the AI prompt. No completion, refactoring or LSP. |
| What Claude changes shows at once in editor and tree | Feasible | File-system events, plus the AI stream naming each file it edits. |
| Fast, excludes, cache, refresh | Feasible | File list in memory, ripgrep's engine for content; no index to go stale. |
| Git: status, ahead/behind, auto-fetch, stage, diff, commit, push | Feasible | Through the `git` program. |
| Git: conflict resolution as in JetBrains (three panes) | Feasible, the largest Git item | A simpler per-conflict chooser comes first. |
| AI: own interface to Claude Code, sessions, progress, stop | Feasible | Claude Code has a streaming JSON mode; checked against the installed 2.1.322. |
| AI: limits per 5 h and per week | Feasible | The stream reports both windows (checked). |
| AI: slash-command completion, session names | Feasible | The stream lists commands and skills; session files carry AI-written titles (checked). |
| AI: hide the sessions the `>` prompt makes | Feasible, small | `--no-session-persistence`: they are never saved. |
| One AI for several projects | Feasible | Claude Code works in one folder; the others are added with `--add-dir`. |
| Links that reuse the browser tab | Feasible with an extension | No browser offers this from outside. A small extension does it; without it a link opens the plain way. |
| Remote shells that survive closing the bench, with history and scrollback | Feasible | The keeper already does all of this locally; it runs on the server. Needs a binary there (no root). |
| Remote shells restored after the server restarts | Feasible | Same mechanism as a closed bench: folder, scrollback and history come back, the programs do not. |
| "Text-based interface if faster" | Not needed | The editor is painted as the terminal is: one monospaced layer, drawn directly. A TUI would add an emulator in between and gain nothing. |

## Design decisions

**Who owns what.** The rule stays: what must outlive a window lives in the keeper.

- AI sessions (a `claude` process each) and remote connections outlive windows:
  keeper.
- The file list, search, highlighting, open buffers and Git views are functions
  of the disk, cheap to rebuild: window process. A bug or a huge repository
  there cannot take the PTYs down. Unsaved edits are the exception to "cheap":
  they are written to a recovery file as they are made, so hiding a bench or a
  crash loses nothing.
- New code goes in new crates without GPUI, each testable alone:
  `shed-text` (buffer, edits, undo), `shed-project` (file list, search,
  highlighting, following the disk), `shed-git` (the `git` program, parsed),
  `shed-ai` (Claude Code's stream, session files), `shed-remote` (the bridge).
  `shed-keeper` and `shed-app` host them.

**Tab kinds in the model.** `Tab` gains `kind: TabKind` (`Terminal` by default,
so saved benches load unchanged; the protocol version is bumped). A project tab
keeps its `root`, excludes, links and open files; an AI tab its folder, the
project folders added to it and its current session id; a remote tab its host
and remote session id. Only terminal tabs have panes, startup and teardown.

**One text core.** The editor, the diff, the AI transcript, search results, the
commit message and the AI prompt are all monospaced text with coloured spans,
selection, find and smooth scrolling; three of them are edited. So there is one
buffer (`shed-text`: a rope, selections, undo, line index) and one view
(`text_surface.rs`) that paints only visible lines on a canvas, as
`terminal_view.rs` does, read-only or editable, any height from one prompt line
to a whole pane. This keeps each frame to one layer (the drawing rule) and
makes "fast" a property of one piece of code.

**Editing stays light.** Typing, selection, undo and redo, clipboard, IME,
indent and outdent, comment toggle, find and replace, save. No completion, no
diagnostics, no refactoring, no LSP: the AI does those.

**The disk is the truth, and it moves.** Claude, Git and build tools change
files while they are open. One watcher per project root (`notify`, events only,
no polling) feeds the tree, the open buffers and the Git state:

- An open file without unsaved edits reloads in place, keeping its scroll
  position and marking the changed lines for a moment.
- An open file with unsaved edits is never overwritten: a bar offers Reload,
  Keep Mine or Show Diff.
- The tree adds, removes and recolours entries as events arrive; files changed
  in the last few minutes carry a mark, so what the AI just touched stands out.
- The AI stream names each file as it is edited, so those refresh without
  waiting for the watcher. **Follow AI** (a toggle) opens each file it changes
  at the change.
- Budget: a write on disk is on screen within 100 ms.
- Where the system's watch limit is too low for a huge tree, excluded folders
  are not watched, and the tab falls back to rescanning when it gets focus,
  saying so.

**Git through the `git` program**, not libgit2: the user's config, credential
helpers, SSH agent, hooks and signing all work as in their terminal, on Linux
and macOS alike. Reads use the machine formats (`status --porcelain=v2 -z`,
`diff`, `log --format`). Passwords and passphrases come through
`GIT_ASKPASS`/`SSH_ASKPASS` pointing at `shed`, which asks in a Shed dialog.

**Claude Code through its streaming mode**, with the real TUI one click away.
`claude -p --input-format stream-json --output-format stream-json
--include-partial-messages --permission-prompts host` is the same engine the
Agent SDK drives. The keeper runs one per live session and relays events;
windows draw them. Whatever the native interface lacks (login, a picker) opens
the same session in a terminal pane with `claude --resume <id>`.

**AI in its own tab.** Work often spans several projects with one AI, and a
conversation is easier to follow with a tab to itself. An AI tab has a folder
(where Claude Code runs and keeps its sessions) and a list of the bench's other
project folders it may reach (`--add-dir`), each a tick box. A bench can have
several AI tabs. Project tabs stay linked to them in use rather than in
structure: "send this file / selection to the AI" goes to the bench's AI tab
(the last used, if several), and paths in a transcript open in the project tab
that contains them.

**Remote: the keeper on the server.** See [Remote](#7-remote-tabs).

## 1. Tab kinds

- `TabKind` in `shed-model`, `shed-proto`, `shed-save`, and in the keeper's
  handling of tabs without panes; kind icons in the tab bar and on pegboard
  cards; "New Project Tab…" (choose a folder), later "New AI Tab…" and "New
  Remote Tab…".
- A project tab's shell: header (folder, later branch and links), a list on the
  left, content, with keyboard focus rules matching the bench window's.

## 2. Text core

- `shed-text`: rope buffer, edits, selections, undo groups, line index; no GPUI.
- `text_surface.rs`: the view. Spans, selection, copy, find, go to line, soft
  wrap; editable mode with cursor, IME, clipboard.
- Recovery files for unsaved edits.

## 3. Code: project tabs

**Files.** The `ignore` crate walks the root in parallel, honouring
`.gitignore`, `.ignore` and the tab's own `exclude` globs (set in the tab's
menu, saved with the bench). The list lives in memory and follows the disk as
above. **Refresh** drops the list, the highlight cache and the Git state and
rebuilds them.

**Open by name** (Ctrl+P): fuzzy matching with `nucleo` over the list; files and
folders. **Tree** on the left: lazy, with Git state colours and the
recently-changed mark.

**Search in files** (Ctrl+Shift+F in a project tab): ripgrep's own crates
(`grep-searcher`, `grep-regex`), streaming results as they come, with case, word,
regex and path filters; replace across files after it. No persistent index: on
repositories up to a few million lines this answers in well under a second and
can never be stale. A trigram index is the fallback if a real project proves too
big.

**Editor.** tree-sitter highlighting for JavaScript, TypeScript/TSX, HTML,
CSS/SCSS, Java, Kotlin, Rust, JSON, XML, YAML, TOML, Markdown, Python, shell,
SQL, Go, C/C++; colours from the theme (a `[syntax]` section with defaults).
Files over a size limit open plain and read-only. Line numbers, find and
replace, go to line, word-wrap toggle, a symbol outline from the same parse
(functions, classes), "copy path", "open in external editor", several open
files as sub-tabs with an unsaved mark, new/rename/delete file from the tree
(delete asks). Images show as images.

**From the terminal.** Ctrl+click on `path:line` in any pane of the bench opens
it in the project tab that contains it.

**Budgets**, measured in tests on a generated 100 000-file tree: list ready in
under 1 s cold; a keystroke in open-by-name under 16 ms; a 10 000-line file
shown in under 50 ms; a typed character on screen in the next frame at any file
size the editor accepts; first search results in under 100 ms; an idle project
tab wakes nothing.

## 4. Git

In a project tab, in order, each usable alone:

1. **State, always visible.** Branch, upstream, ahead/behind, counts of staged,
   changed, untracked and conflicted files: in the project tab's header, on its
   tab and on its pegboard card. Refreshed on file-system events and after every
   Git action. **Auto-fetch** every few minutes while the bench is open
   (configurable, off per project), so "behind" is true.
2. **Changes.** Staged and unstaged lists; stage, unstage, discard (asks) by
   file, then by hunk and by line.
3. **Diff.** Unified and side-by-side, on `text_surface`, with syntax colours
   and word-level marks (`imara-diff`). Any two of: working tree, index, HEAD, a
   commit, a branch. The working-tree side is editable.
4. **Commit, push, pull, fetch.** Message editor, amend, sign-off; progress and
   errors shown in place; force-push asks.
5. **Branches and history.** Switch, create, delete, merge, rebase onto; log
   with graph; a file's history; blame in the editor's gutter; stash.
6. **Conflicts.** First: the file's conflicts listed, each with "ours",
   "theirs", "both", and "ask the AI to resolve", then mark resolved. Then the
   three-pane merge view (ours, result, theirs) with per-change arrows, the
   result editable.

## 5. AI: Claude Code

**Sessions.** Listed from `~/.claude/projects/<folder>/*.jsonl` for the tab's
folder: title (Claude Code's own `ai-title`, else the first prompt; rename is
ours), last activity, branch. Search across them. Resume, fork
(`--fork-session`), archive (hidden, not deleted). The tab remembers its current
session; several can run at once, each shown with its state on the tab (working,
needs an answer, done), as tools are today.

**The `>` prompt's sessions** stop being saved: the default command becomes
`claude -p --no-session-persistence`. For other commands, sessions whose first
prompt is Shed's own preamble are hidden from the list.

**Transcript** on `text_surface`: streaming text, tool calls folded to one line
(expandable), file edits as inline diffs that open in the diff view, clickable
paths, find, export to Markdown. Long content is virtualized; nothing is laid
out off screen.

**Prompt**: the same editor, growing with its text. Large pastes fold to a chip
("pasted, 412 lines") and travel in full; save and load prompts from disk;
history of prompts; **slash-command and skill completion** from the stream's
`init` event; `@` completes files from the project lists.

**Attachments.** Paste an image from the clipboard (a screenshot), drop files,
"send this file / this selection" from a project tab, "send this pane's last
output" from a terminal tab of the bench.

**Control.** Stop (interrupt), queue a follow-up while it works, permission
requests as a Shed dialog (allow once, always, deny), mode switch (plan, accept
edits), model and effort.

**Limits and cost.** The 5-hour and weekly windows with reset times, from the
stream's `rate_limit_event`, in the AI tab and on the pegboard; context used
and cost per session from the result events.

**Worth taking from other tools**, later: review everything a turn changed as
one diff with accept/revert (ties into Git and Claude Code's file checkpoints);
a notification when a session finishes or asks while the bench is hidden; the
task list (todo tool) pinned above the transcript; a prompt library per
project; one session per Git worktree for parallel work.

**Risk.** The stream format is the Agent SDK's wire format and moves with
Claude Code versions. `shed-ai` ignores what it does not know, is tested
against recorded streams (as `shed-emu` replays recordings), and the terminal
fallback always works.

## 6. Links

A project tab holds named links (`local`, `stage`, `prod`), shown as buttons in
its header and on its pegboard card; a default one opens with a key.

- **Open** reuses a browser tab already showing the link; **Open in New Tab** is
  in its menu.
- **The extension** (one WebExtension, built for Chrome-family browsers and
  Firefox) talks to `shed` through native messaging: it finds an open tab by
  URL prefix, activates it and raises its window, else opens one. Shed installs
  the native-messaging manifest itself; the extension is installed from its
  store page (or loaded unpacked), linked from Shed.
- **Without the extension** Shed says so once and opens the link the plain way
  (a new tab each time).

## 7. Remote tabs

A remote tab must keep its shell alive with the bench closed, come back with its
folder, history and scrollback, and recover what it can after the server
restarts. That is the keeper's job description, so the keeper runs there:

- `shedd --remote` on the server owns the PTYs, emulators, scrollback and
  logbook, exactly as locally, and does not stop when nobody is connected or at
  logout (started through `systemd-run --user`, with lingering enabled, so
  session cleanup leaves it alone; Ubuntu and its relatives have both).
- The local keeper reaches it over `ssh host shedd bridge`, which joins the SSH
  channel to the remote keeper's socket. Host names, keys, jump hosts and
  multiplexing come from `~/.ssh/config`. The handshake runs in the tab's own
  PTY, so password and 2FA prompts appear right there.
- A remote pane is a pane in the local tree whose bytes are relayed. Attach
  gives a replay, as for a hidden bench. Close the bench: the relay drops, the
  shell stays. Network loss: the tab shows "reconnecting" and reattaches.
- After a server restart the remote keeper restores its saved session: same
  folder, scrollback, history; the tab says the shell is new.
- **The binary**: a static (musl) `shedd` for x86-64 and arm64 Linux, copied to
  `~/.local/bin` on first connect, no root.
- **Versions**: local and remote keepers speak a small protocol of their own
  (spawn, attach, input, output, resize, replay, marks, close), versioned
  apart from the window protocol, so updating Shed locally never forces a
  restart of remote shells.
- Remote tabs have no startup or teardown and are marked as remote; Ctrl+R
  searches the remote logbook.

Without the binary (a server where nothing may be installed), a remote tab falls
back to plain `ssh` with reconnection: no persistence, said so in the tab.

## Order of work

Each step ends in something usable; sizes are relative (S about a day or two of
work together, M several, L a week or more).

| # | Step | Size | Needs |
|---|---|---|---|
| 1 | Tab kinds; project tab shell | M | |
| 2 | Text core: buffer, `text_surface`, editing | L | |
| 3 | Files: list, tree, open by name, editor with colours and saving, following the disk | M | 1, 2 |
| 4 | Search in files; excludes; refresh | S | 3 |
| 5 | AI tab, quick: sessions list, Claude Code's own TUI in the tab, `>` sessions hidden, other project folders added | S | 1 |
| 6 | Git: state, auto-fetch, changes, diff, commit/push/pull | L | 3 |
| 7 | AI tab, native: transcript, prompt, stop, permissions, limits, completion, attachments, Follow AI | L | 2, 5 |
| 8 | Links, with the extension | M | 1 |
| 9 | Remote tabs | L | |
| 10 | Git: branches, history, blame, stash; conflicts | L | 6 |
| 11 | AI: review a turn's changes; notifications | M | 6, 7 |

Remote tabs (9) and links (8) depend on little else and can move earlier.

## Still open

- A merge view that lines up the three versions and moves single changes across
  with arrows (now: the three versions side by side, whole, and per-conflict
  Ours / Theirs / Both).
- Blame in the editor's gutter (now: for the cursor's line, in the status bar).
- The AI tab's prompt folding large pastes into a chip, and a prompt library.
- Screenshots of the new tabs in the user guide (those there predate project,
  AI and remote tabs; `tools/screenshots/take.sh` retakes them).
- AI tab hover redraws: each entry has its own view, but they all read the
  `AiView`, which GPUI marks dirty with any child; moving the conversation's
  state into a model of its own would let a hover redraw one entry only.
- A static `shedd` build for servers with an older C library (see Development).
- A remote tab marked private still has its commands logged on the server.

## Decisions

Taken with the user on 2026-11-11:

1. **Editing**: light editing from the start, and the editor and tree must show
   at once what Claude changes.
2. **Order**: as above unless something works better; all of it is wanted.
3. **Links**: the browser extension, falling back to a plain open when it is
   not installed.
4. **Servers**: a `shedd` binary in `~/.local/bin` is fine; they run Ubuntu and
   relatives.
5. **AI**: a tab of its own rather than a part of the project tab, since one AI
   often serves several projects.
