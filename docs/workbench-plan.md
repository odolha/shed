# Shed as a working environment: plan

Shed today starts a project's terminals and keeps them. This plan turns a bench
into everything needed to work on a project: its terminals, its code (to read
and search, with Git), an AI session per project, links to its web apps, and
shells on remote servers that stay alive. It records what is feasible, the
design choices and their reasons, and the order of work.

Status: proposal, 2026-11-11. Nothing below is built yet.

## The picture

A bench keeps its tabs. A tab gets a **kind**, shown by an icon in the tab bar and
on the pegboard:

| Kind | What it is |
|---|---|
| Terminal | As now: panes, each a local shell. |
| Project | A folder of code: file tree, viewer, search, Git, and an optional AI session. Links to its web apps sit in its header. |
| Remote | A shell on a server, kept alive there by a remote keeper. |

Loading a bench then does all of it: local servers start in terminal tabs,
project tabs open where they were (file, scroll position, AI session), remote
tabs reattach.

## Feasibility at a glance

| Wish | Verdict | Notes |
|---|---|---|
| Project tabs: browse, open by name, search in files | Feasible, fast | No editor widget exists in GPUI; the viewer is ours. See [Code](#2-code-project-tabs). |
| Syntax colours for the common languages | Feasible | tree-sitter grammars, compiled in. |
| Fast, excludes, cache, refresh | Feasible | File list in memory, ripgrep's engine for content; no index to go stale. |
| Git: status, ahead/behind, auto-fetch, stage, diff, commit, push | Feasible | Through the `git` program. |
| Git: conflict resolution as in JetBrains (three panes) | Feasible, the largest Git item | Needs the editable text core. A simpler per-conflict chooser comes first. |
| AI: own interface to Claude Code, sessions, progress, stop | Feasible | Claude Code has a streaming JSON mode; checked against the installed 2.1.322. |
| AI: limits per 5 h and per week | Feasible | The stream reports both windows (checked). |
| AI: slash-command completion, session names | Feasible | The stream lists commands and skills; session files carry AI-written titles (checked). |
| AI: hide the sessions the `>` prompt makes | Feasible, small | `--no-session-persistence`: they are never saved. |
| Links that reuse the browser tab | Partly | No browser offers this from outside. Real reuse needs a small browser extension; without it, a dedicated app window per link works on Chrome-family browsers. |
| Remote shells that survive closing the bench, with history and scrollback | Feasible | The keeper already does all of this locally; it runs on the server. Needs a binary there (no root). |
| Remote shells restored after the server restarts | Feasible | Same mechanism as a closed bench: folder, scrollback and history come back, the programs do not. |
| "Text-based interface if faster" | Not needed | The viewer is painted as the terminal is: one monospaced layer, drawn directly. A TUI would add an emulator in between and gain nothing. |

## Design decisions

**Who owns what.** The rule stays: what must outlive a window lives in the keeper.

- AI sessions (a `claude` process each) and remote connections outlive windows:
  keeper.
- The file list, search, highlighting and Git views are functions of the disk,
  cheap to rebuild: window process. A bug or a huge repository there cannot take
  the PTYs down.
- New code goes in new crates without GPUI, each testable alone:
  `shed-project` (file list, search, highlighting), `shed-git` (the `git`
  program, parsed), `shed-ai` (Claude Code's stream, session files),
  `shed-remote` (the bridge). `shed-keeper` and `shed-app` host them.

**Tab kinds in the model.** `Tab` gains `kind: TabKind` (`Terminal` by default,
so saved benches load unchanged; the protocol version is bumped). A project tab
keeps `root`, excludes, links, open files and its AI session id; a remote tab
keeps its host and remote session id. Startup and teardown stay a terminal-tab
matter.

**One text surface.** Viewer, diff, AI transcript and search results are all
long monospaced text with coloured spans, selection, find and smooth scrolling.
One component (`text_surface.rs`) paints only visible lines on a canvas, as
`terminal_view.rs` does, and is used by all four. This keeps each frame to one
layer (the drawing rule) and makes "fast" a property of one piece of code.

**One multi-line editor.** The commit message, the AI prompt (large pastes) and
conflict resolution need multi-line editing: a rope buffer, selections, undo,
clipboard, IME, wrapping. `line_editor.rs` is single-line. This `text_area` is
built once, early. Editing files lightly (fix a typo, change a config) then
costs little more; a full IDE editor (completion, refactoring, LSP) stays out of
scope.

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

**Remote: the keeper on the server.** See [Remote](#5-remote-tabs).

## 1. Foundations

- `TabKind` in `shed-model`, `shed-proto`, `shed-save`; kind icons in the tab bar
  and on pegboard cards; "New Project Tab…", "New Remote Tab…".
- `text_surface.rs`: virtualized monospaced text with spans, selection, copy,
  find, go to line.
- `text_area.rs`: the multi-line editor.
- A side-panel layout for non-terminal tabs (list on the left, content, optional
  panel on the right), with keyboard focus rules matching the bench window's.

## 2. Code: project tabs

**Files.** The `ignore` crate walks the root in parallel, honouring
`.gitignore`, `.ignore` and the tab's own `exclude` globs (set in the tab's
menu, saved with the bench). The list lives in memory and follows the disk
through file-system events (`notify`), so nothing polls. **Refresh** drops the
list, the highlight cache and the Git state and rebuilds them.

**Open by name** (Ctrl+P): fuzzy matching with `nucleo` over the list; files and
folders. **Tree** on the left: lazy, with Git state colours.

**Search in files** (Ctrl+Shift+F in a project tab): ripgrep's own crates
(`grep-searcher`, `grep-regex`), streaming results as they come, with case, word,
regex and path filters. No persistent index: on repositories up to a few
million lines this answers in well under a second and can never be stale. A
trigram index is the fallback if a real project proves too big.

**Viewer.** Read-only first. tree-sitter highlighting for JavaScript,
TypeScript/TSX, HTML, CSS/SCSS, Java, Kotlin, Rust, JSON, XML, YAML, TOML,
Markdown, Python, shell, SQL, Go, C/C++; colours from the theme (a `[syntax]`
section with defaults). Files over a size limit open plain. Line numbers, find,
go to line, word-wrap toggle, a symbol outline from the same parse (functions,
classes), "copy path", "open in external editor", several open files as
sub-tabs. Images show as images.

**From the terminal.** Ctrl+click on `path:line` in any pane of the bench opens
it in the project tab that contains it.

**Budgets**, measured in tests on a generated 100 000-file tree: list ready in
under 1 s cold; a keystroke in open-by-name under 16 ms; a 10 000-line file
shown in under 50 ms; first search results in under 100 ms; an idle project tab
wakes nothing.

## 3. Git

In order, each usable alone:

1. **State, always visible.** Branch, upstream, ahead/behind, counts of staged,
   changed, untracked and conflicted files: in the project tab's header, on its
   tab and on its pegboard card. Refreshed on file-system events and after every
   Git action. **Auto-fetch** every few minutes while the bench is open
   (configurable, off per project), so "behind" is true.
2. **Changes.** Staged and unstaged lists; stage, unstage, discard (asks) by
   file, then by hunk and by line.
3. **Diff.** Unified and side-by-side, on `text_surface`, with syntax colours
   and word-level marks (`imara-diff`). Any two of: working tree, index, HEAD, a
   commit, a branch.
4. **Commit, push, pull, fetch.** Message in `text_area`, amend, sign-off;
   progress and errors shown in place; force-push asks.
5. **Branches and history.** Switch, create, delete, merge, rebase onto; log
   with graph; a file's history; blame in the viewer's gutter; stash.
6. **Conflicts.** First: the file's conflicts listed, each with "ours",
   "theirs", "both", and "ask the AI to resolve", then mark resolved. Then the
   three-pane merge view (ours, result, theirs) with per-change arrows, the
   result editable.

## 4. AI: Claude Code

**Sessions.** Listed from `~/.claude/projects/<folder>/*.jsonl` for the tab's
root: title (Claude Code's own `ai-title`, else the first prompt; rename is
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

**Prompt** in `text_area`: large pastes fold to a chip ("pasted, 412 lines") and
travel in full; save and load prompts from disk; history of prompts;
**slash-command and skill completion** from the stream's `init` event;
`@` completes files from the project's list.

**Attachments.** Paste an image from the clipboard (a screenshot), drop files,
"add this file / this selection" from the viewer, "add this pane's last output"
from a terminal tab of the bench.

**Control.** Stop (interrupt), queue a follow-up while it works, permission
requests as a Shed dialog (allow once, always, deny), mode switch (plan, accept
edits), model and effort.

**Limits and cost.** The 5-hour and weekly windows with reset times, from the
stream's `rate_limit_event`, in the AI panel and on the pegboard; context used
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

## 5. Remote tabs

A remote tab must keep its shell alive with the bench closed, come back with its
folder, history and scrollback, and recover what it can after the server
restarts. That is the keeper's job description, so the keeper runs there:

- `shedd --remote` on the server owns the PTYs, emulators, scrollback and
  logbook, exactly as locally, and does not stop when nobody is connected or at
  logout (started through `systemd-run --user` where there is systemd, so
  session cleanup leaves it alone).
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

## 6. Links

A project tab holds named links (`local`, `stage`, `prod`), shown as buttons in
its header and on its pegboard card; a default one opens with a key.

- **Open**: reuse if possible; **Open in New Tab** in the menu.
- **Reuse, tier 1 (no install):** the link opens in its own app window
  (`--app=URL`, Chrome-family) which Shed tracks and raises next time. Firefox
  has no such mode and gets a new tab.
- **Reuse, tier 2 (real tabs):** a small WebExtension for Chrome and Firefox
  with a native-messaging host (`shed`): finds an open tab by URL prefix,
  activates it and its window, else opens one. This is the only reliable way to
  reuse an ordinary browser tab.

## Order of work

Each step ends in something usable; sizes are relative (S about a day or two of
work together, M several, L a week or more).

| # | Step | Size | Needs |
|---|---|---|---|
| 1 | Tab kinds; project tab shell; `text_surface` | M | |
| 2 | Files: list, tree, open by name, viewer with colours | M | 1 |
| 3 | Search in files; excludes; refresh | S | 2 |
| 4 | AI, quick: sessions list, Claude Code's TUI docked in the project tab, `>` sessions hidden | S | 1 |
| 5 | `text_area` | M | |
| 6 | Git: state, auto-fetch, changes, diff, commit/push/pull | L | 2, 5 |
| 7 | AI, native: transcript, prompt, stop, permissions, limits, completion, attachments | L | 1, 5 |
| 8 | Links, tier 1 | S | 1 |
| 9 | Remote tabs | L | |
| 10 | Git: branches, history, blame, stash; conflicts | L | 6 |
| 11 | AI: review a turn's changes; notifications; light file editing | M | 6, 7 |
| 12 | Links, tier 2 (extension) | M | 8 |

Remote tabs (9) depend on nothing else and can move earlier.

## Open questions

1. **Editing.** Is read-only plus "open in external editor" enough at first, with
   light editing in step 11, or is editing needed from the start?
2. **Order.** AI before Git (as above, with the quick step 4 early), or the
   reverse?
3. **Browser.** Which one? It decides whether tier 1 is worth building or the
   extension comes first.
4. **Servers.** May a `shedd` binary be placed in `~/.local/bin` on them? Which
   systems (distribution, x86-64 or arm64)?
5. **Project tab layout.** One tab per project with Code / Changes / AI as
   switchable or split areas (proposed), or the AI as a separate tab kind beside
   the project tab?
