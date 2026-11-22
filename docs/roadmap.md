# Roadmap

What is still open, as of 2026-11-20. Shed as it is now is described in the
[User Guide](user-guide.md) and [How Shed works](architecture.md); how it got
here, and why, is in [History](history/README.md).

Nothing here is promised or ordered; items move into Shed as they are wanted.

## Shed from anywhere

The nexus, the link, the web and Android apps and the server workspace with
`shed tui` are in (see [the plan](nexus-plan.md) for the decisions behind
them). Still open:

- **Who holds a pane's size.** An app can take a pane's size for a
  full-screen program on a phone (**Take the size**). The plan had the Shed window
  then show the pane at that size and say which device holds it; now the window
  keeps its own size for the pane (the program draws for the phone's), and
  resizing the window takes the size back.
- **A record on the workspace of what came from afar.** The nexus records
  sign-ins, pairings and revocations; the workspace does not yet log each
  remote action with the device that sent it.
- **Packaging the server side**: a static `shed` (built without windows) and
  `shedd` for servers, the nexus with its web app built in or packaged beside
  it, and a signed Android release (now: debug builds only; F-Droid later).
- **Editing files from the apps** beyond writing a small text file whole,
  through `shed-text` as the editor does.
- **Other workspaces in the desktop app: the last gaps.** Shed's window
  shows another workspace through the nexus, project tabs, the logbook and AI
  sessions included; still to come for it, as API calls: merging, rebasing,
  deleting branches, the stash and finishing a merge in the Git panel, the
  three-way conflict view, the review's Git actions and its turn scope.
- **Settings and themes from the apps**, **more than one user** on a nexus, and
  **Claude in Chrome** controlled from them (now: shown, not controlled).

## Projects and Git

- **A merge view that lines up the three versions** and moves single changes
  across with arrows. Now: the three versions side by side, whole, and
  per-conflict Ours / Theirs / Both.
- **Blame in the editor's gutter**, for every line. Now: for the cursor's line,
  in the status bar.

## AI tabs

- **Large pastes folded into a chip** in the prompt, and **a prompt library** of
  prompts kept to reuse.
- **Expand** in the review: a longer explanation of a change, hunk by hunk,
  asked aside like **Explain**.
- **A hover redraws one entry only.** Each entry has a view of its own, but all
  of them read the `AiView`, which GPUI marks dirty whenever any child is;
  moving the conversation's state into a model of its own would let a hover
  redraw just the entry under the mouse (see
  [Development](development.md#debugging) for the frame measurement).

## Remote tabs

- **A static `shedd`** for servers with an older C library or another
  architecture, built and found by default (now: put by hand beside `shed`; see
  [Development](development.md#remote-tabs)).
- **Private remote tabs**: a remote tab marked private still has its commands
  logged on the server.

## Terminals and performance

- **A terminal grid drawn by Shed itself** rather than glyph by glyph through
  GPUI: steady output costs about four to five times kitty's CPU per frame.
- **A `scrollback` setting** in `shed.toml` (the keeper keeps about 20 MB for a
  busy pane).
- **Shells beyond bash and zsh** (fish) for history, aliases and prompt marks.

## Platforms

- **macOS**: the window's display link keeps ticking while idle (Linux's X11
  loop was fixed to park); notification clicks do not reach Shed yet.
- **Windows, through WSL.**

## Docs

- **The two menu screenshots** (`colours.png`, `tab-menu.png`) predate the new
  tabs: the headless screenshot script cannot click (Xvfb has no XInput
  pointer for GPUI).
