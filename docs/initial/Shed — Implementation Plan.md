# Shed — Implementation Plan

Nov 1, 2026 · @Ovidiu

## Overview

Shed is a GPU-accelerated Linux terminal written in Rust, built with Claude. It reuses a proven emulation core for everything inside the terminal grid and owns everything around it: the window, the layout, history, and saving and restoring work.

The core idea: a small background process, the **keeper** (`shedd`), holds every shell and running program. The window is only a view onto it, so closing or crashing the window kills nothing.

**Goals for v1**

- **Benches** (workspaces), **each in its own window**, so every bench shows up separately in the taskbar
- **Bench colours and badges** so you can tell windows apart at a glance, and **tags** on tabs
- **Logbook**: three command histories kept at once (local per tab, per bench, global), each walkable with the arrow keys and searchable
- **Save and load benches**: every bench saves itself, and loading it brings back layout, working directories and the programs that were running
- Maximum compatibility with terminal apps (vim, htop, tmux, lazygit, ssh), full mouse support
- Performance in the same class as Alacritty and Ghostty

**Non-goals for v1**

- Writing our own escape-sequence parser or text renderer
- macOS and Windows
- Remote multiplexing (attaching to a keeper on another machine)
- A plugin system

## Vocabulary

The rule: **Shed's own concepts get workshop names; standard terminal concepts keep their usual names.** Nobody should have to learn that a tab is called something else, but the new ideas (saved workspaces, scoped history) need short names, and the workshop names give the app character.

| Term | What it is | Example in use |
| --- | --- | --- |
| **Shed** | The app as a whole: every bench, tab and running program | "Open Shed" |
| **Bench** | A workspace for one job, in its own window, with its own colour, default directory and logbook scope. A bench is always saved; you **load** it to bring it back | A `backend` bench (blue) and a `website` bench (green), each a separate window in the taskbar |
| **Tab** | A tab, as everywhere else | — |
| **Tag** | A short coloured label on a tab, set by you or by a rule | `prod` in red on an `ssh` tab, `logs` in grey |
| **Pane** | One terminal inside a tab; tabs split into panes | — |
| **Tool** | The program running in the foreground of a pane. A pane at an idle prompt has no tool | `nvim`, `ssh prod`, `cargo watch` |
| **Rack** | The list of tools allowed to restart by themselves when a bench loads | Editors, `ssh`, monitors, dev servers are on the rack; `git push` is not |
| **Logbook** | Shed's command history. Each command run is a **log entry** | Ctrl+R opens the logbook |
| **Pegboard** | The overview screen: every bench and tab laid out at once, with live previews and tags | Super+Space shows the pegboard |
| **Keeper** | The background process (`shedd`) that holds every pane's programs, so closing a window kills nothing | `shed status` shows whether the keeper is running |

**Logbook scopes**, from narrow to wide: **Local** (this tab), **Bench** (every tab in this bench), **Global** (everything). Up walks Local, Ctrl+Up walks Bench, Ctrl+Shift+Up walks Global.

**Verbs stay plain.** The UI says Save, Load, Detach, Close, never "lock up the shed". The theme is in the nouns and the icons, not in making actions harder to understand.

**Command-line names**

- `shed`: the app and its CLI (`shed`, `shed load <bench>`, `shed save <bench> --as <name>`, `shed log`, `shed status`)
- `shedd`: the keeper
- `shed init zsh|bash|fish`: prints the shell hook, used as `eval "$(shed init zsh)"`

## Tech stack and key decisions

Shed reuses a mature emulation core and the GPUI text renderer, so our code is the layout, the keeper, the logbook and saving benches.

| Layer | Choice | Why |
| --- | --- | --- |
| UI framework | **GPUI** (Zed's framework), pinned to a git revision | GPU text rendering, font fallback and shaping already solved; full control over every piece of chrome; runs on Wayland and X11 |
| Emulation core | **`alacritty_terminal`** behind our own `Emulator` trait | Pure Rust, mature, powers Alacritty and Zed's terminal. The trait keeps `libghostty-vt` possible later (see Open decisions) |
| PTY | `rustix` / `nix` directly, `portable-pty` as reference | We need low-level access: `tcgetpgrp`, process groups, `/proc` reads for kits |
| Keeper runtime | `tokio` | Many PTYs, one socket server, timers for autosave |
| Protocol encoding | `postcard` or `bincode` for control messages, raw bytes for terminal output | Compact and fast; terminal output is never re-encoded |
| Logbook storage | SQLite via `rusqlite`, WAL mode, FTS5 for search | One file, fast prefix and full-text search, survives crashes |
| Saved benches | TOML (`serde` + `toml`), scrollback as `zstd`-compressed files | Human-readable and hand-editable; scrollback stays small |
| Config | TOML, hot-reloaded with `notify` | Matches saved bench files; live tweaking of themes and keys |
| Logging | `tracing` + `tracing-subscriber` | Debugging the keeper without a window |

**Decisions locked in now** (expensive to change later):

1. The keeper owns PTYs from day one; the window never spawns shells itself.
2. Every pane, tab and bench has a stable UUID, used across protocol, logbook and saved benches.
3. The emulation core sits behind a trait, so the core can be swapped.
4. Shell integration is in-band (escape sequences on the PTY stream), so it costs nothing and needs no extra process per command.

## Architecture

Shed is two programs talking over a Unix socket: the window (`shed`) draws and takes input, the keeper (`shedd`) owns every shell and the state that must survive.

&#91;embedded content: Shed architecture · window, keeper, shells, disk\]

Each bench is its own top-level window; one \`shed\` process drives all of them, and every window talks to the same keeper. Keystrokes go window → keeper → PTY. Output goes PTY → keeper, which updates its own copy of the screen, picks out the shell-hook sequences for the logbook, and forwards the bytes to every attached window. When a window attaches, the keeper first replays each pane's current screen, then streams live output.

**Crates** (one Cargo workspace):

| Crate | Role |
| --- | --- |
| `shed-model` | The bench → tab → pane tree, IDs, colours, tags, serde types shared by everything |
| `shed-proto` | Socket messages and framing |
| `shed-emu` | The `Emulator` trait, the `alacritty_terminal` implementation, headless grid dumps for tests |
| `shed-keeper` | The `shedd` binary: PTYs, socket server, hook parsing, autosave |
| `shed-logbook` | SQLite schema, capture, scoped queries |
| `shed-save` | Saving and loading benches, tool detection, rack rules |
| `shed-desktop` | Per-bench window identity: generated icons, desktop entries, window icons |
| `shed-app` | The `shed` binary: GPUI windows plus the CLI subcommands |
| `shell/` | zsh, bash and fish hook scripts, embedded into `shed init` |

**Main protocol messages**: `Attach`, `Detach`, `Input {pane, bytes}`, `Output {pane, bytes}`, `Resize`, `Replay {pane}`, `TreeChanged`, `NewBench / NewTab / Split / Close` / SetColor / SetTags, `LogbookQuery`, `BenchSave`, `BenchLoad`, `PaneExited`.

## Feature design

### Benches, tabs and panes

- The model is a tree: Shed → benches → tabs → a split tree of panes. The keeper holds the authoritative tree; the windows render it.
- **One window per bench.** A new bench opens a new window, so each bench is its own entry in the taskbar and in Alt+Tab.
- Closing a bench window **detaches** it: its tools keep running in the keeper. **Close bench** (menu or `shed close <bench>`) stops its tools after a confirmation.
- Each bench has a name, a colour and a default directory; new tabs and panes open there.
- **Switching**: Super+1…9 focuses bench windows, Ctrl+Tab and Ctrl+1…9 switch tabs, Alt+arrows move between panes. All rebindable.
- **Pegboard** (Super+Space, from any bench): every bench (open, detached or stored) with live tab previews and tags; type to filter, Enter to jump, drag a tab onto another bench to move it.

### Telling bench windows apart

Every bench gets a **colour**, picked from eight options (blue, violet, pink, coral, amber, green, teal, slate) or any custom colour. New benches take the next unused colour; you can change it at any time. The colour shows up in four places:

- **Taskbar or dock icon**: the Shed icon in the bench colour
- **Window title**: `backend — Shed`, with a coloured dot where the desktop supports it, visible in KDE and XFCE taskbars and in Alt+Tab
- **Tab bar**: a stripe in the bench colour along the top of the window
- **Focused pane border**: in the bench colour

Badges (one or two letters on the icon) are an optional setting, off by default, for anyone running more benches than there are colours.

Linux has no single way to give one app's windows different taskbar icons, so Shed uses three routes:

| Route | Works on | How |
| --- | --- | --- |
| Per-bench app ID + desktop entry (default) | GNOME, KDE, most docks; Wayland and X11 | Each bench window gets its own app ID (`dev.shed.bench.backend`), and Shed writes a matching hidden desktop entry and a generated icon into `~/.local/share`. The dock treats each bench as its own app; pinning it gives you a launcher that loads that bench |
| Per-window icon, Wayland | Compositors that implement the `xdg-toplevel-icon` protocol | Window icon set through GPUI's window options ([protocol](https://wayland.app/protocols/xdg-toplevel-icon-v1), [GPUI option](https://docs.rs/gpui-ce/0.3.0/gpui/struct.WindowOptions.html)) |
| Per-window icon, X11 | All X11 window managers | `_NET_WM_ICON` set on each window |

The first route is what makes this work on GNOME, which groups windows by app ID: without it every bench would collapse into one dock icon. Shed removes a bench's desktop entry and icon when the bench is deleted.

### Tab tags

- Any tab can carry **tags**: a short label plus a colour, shown as small chips on the tab.
- Add them from the tab's right-click menu, the command palette, or `shed tag add prod` from inside the tab.
- **Tag rules** in `shed.toml` tag tabs automatically by tool, directory or ssh host, for example red `prod` on any `ssh` to a host matching `*prod*`.
- A tag can optionally **tint the whole tab**, useful as a warning on production shells.
- Tags are saved with the bench, searchable in the pegboard, and usable as a logbook filter ("commands run in tabs tagged `prod`").

### Logbook

- **Three histories, all kept up to date at once.** Every command is written once to the logbook and tagged with its tab, bench and time, so each level is just a filter:
  - **Local**: this tab only (all panes in the tab share it; can be switched to per-pane in `shed.toml`)
  - **Bench**: every tab in this bench
  - **Global**: every bench, the whole Shed
- **Walking history with the arrow keys:**

| Keys | Walks | Feels like |
| --- | --- | --- |
| Up / Down | Local | Normal shell history, unchanged |
| Ctrl+Up / Ctrl+Down | Bench | "What did I run anywhere in this bench?" |
| Ctrl+Shift+Up / Ctrl+Shift+Down | Global | "What did I run anywhere?" |

- **Prefix matching works in every scope**: type `git` then press Ctrl+Up to walk only bench commands starting with `git`.
- **Scope chip**: while walking bench or global history, a small chip at the right of the prompt line shows the scope and position (`bench 3/120`) and, for commands from another tab, which tab they came from. It disappears when you press Enter or start typing.
- **How it works**: the arrow keys are handled by the shell's line editor, not the terminal, so `shed init` installs small key bindings in the shell (a zle widget in zsh, `bind -x` in bash, a `bind` in fish). Ctrl+Up asks the keeper for the previous bench entry and places it in the edit line. Because the binding only exists at the shell prompt, Ctrl+Up still reaches vim, htop and other programs untouched.
- **Plain Up stays native**: each tab gets its own shell history file (`HISTFILE` per tab ID), so Up works even if the keeper is busy, and a loaded bench gets each tab's history back.
- **Capture**: the shell hook emits standard sequences: OSC 133 for prompt and command boundaries (with exit code), OSC 7 for the working directory, plus one private sequence carrying the command text. Each log entry stores command, directory, exit code, start time, duration, pane, tab, bench, tags and hostname.
- **Search**: Ctrl+R opens the logbook overlay. Tab cycles Local → Bench → Global; results show directory, exit status, source tab and age. Enter inserts the command at the prompt, Alt+Enter runs it.
- **Private tabs**: a tab marked private keeps its entries in Local only; they never appear in Bench or Global.
- **Extras that come almost free** from the prompt marks: jump between previous commands in scrollback, copy the last command's output, and a failed-command marker in the gutter.

### Saving and loading benches

There is no separate "saved snapshot" concept: **a bench is always saved**. Loading a bench brings it back.

- **A bench is in one of three states**: open (has a window), detached (running in the keeper, no window), or stored (only on disk, nothing running). The pegboard shows all three.
- **Saving is automatic**: the keeper writes each bench to `benches/<name>.toml` every 60 seconds and when it closes, so a reboot or crash costs at most a minute of layout. **Save as** (`shed save backend --as backend-template`) makes a copy, handy as a template.
- **What a save records**, per pane: working directory, the tool (read from the foreground process group via `tcgetpgrp` and `/proc/<pid>/cmdline`), environment overrides, scrollback, tags. Per bench: layout, names, colour, badge, active tab.
- **Loading** (from the pegboard, a pinned dock icon, or `shed load backend`) rebuilds the bench in a new window, starts each shell in its directory, then handles tools by rack status:
  - **On the rack**: relaunched automatically (`nvim file.rs`, `ssh prod`, `htop`, `tail -f log`, `cargo watch`).
  - **Not on the rack**: the command is typed at the prompt but not run. One Enter starts it. Nothing dangerous runs by surprise.
- Loading a bench that is already open just focuses its window; loading a detached one reattaches with every tool still running.
- **Scrollback** is painted back above the new prompt, dimmed, with a "restored" divider.
- **Limits, stated in the UI**: a tool restarts fresh. Editor buffers, REPL state and half-finished builds come back as a restart, not as a frozen moment.

### Mouse

- **Inside apps**: clicks, drags and scrolling are passed to apps that request mouse mode (vim, htop, lazygit). Shift overrides capture so you can always select text.
- **Shed's own UI**: drag tabs to reorder or move between benches, drag split borders, click to focus, double/triple-click selection, Ctrl+click links and file paths, right-click menus.
- **Scroll rule**: in the alternate screen (vim, less) the wheel goes to the app; otherwise it scrolls the scrollback.

### Compatibility

From the core: truecolor, Unicode widths and emoji, alternate screen, bracketed paste, mouse modes, OSC 8 hyperlinks, synchronized output. Ours to build: Kitty keyboard protocol encoding, OSC 52 clipboard, IME input, scrollback search, URL detection. Inline images depend on the core decision (see Open decisions).

## Files, config and storage

Everything follows the XDG base directories, so config is versionable and state is disposable.

| Path | Holds |
| --- | --- |
| `~/.config/shed/shed.toml` | Settings: font, theme, keybindings, the rack, tag rules, autosave interval |
| `~/.config/shed/themes/*.toml` | Colour themes |
| `~/.local/share/shed/benches/<name>.toml` | One saved bench (layout, directories, tools, colour, badge, tags) |
| `~/.local/share/shed/benches/<name>/scrollback/<pane>.zst` | That bench's compressed scrollback |
| `~/.local/share/applications/dev.shed.bench.<name>.desktop` | Generated desktop entry, so the bench gets its own taskbar icon |
| `~/.local/share/icons/hicolor/scalable/apps/dev.shed.bench.<name>.svg` | Generated bench icon in the bench colour, with its badge |
| `~/.local/state/shed/logbook.sqlite` | The logbook |
| `~/.local/state/shed/hist/<pane-uuid>` | Each pane's own shell history file |
| `~/.local/state/shed/shedd.log` | Keeper log |
| `$XDG_RUNTIME_DIR/shed/shedd.sock` | The keeper's socket (permission 0600) |

The rack and tag rules live in `shed.toml`:

```toml
[rack]
# relaunch automatically when a bench loads
auto = ["nvim", "vim", "hx", "ssh", "htop", "btop", "less", "tail -f", "cargo watch", "npm run dev"]
# never relaunch, never even prefill
never = ["rm", "git push", "sudo"]
# everything else: typed at the prompt, waits for Enter

[[tag_rule]]
tool = "ssh *prod*"
tag = "prod"
color = "red"
tint_tab = true
```

A saved bench, abbreviated:

```toml
name = "backend"
color = "blue"
# badge = "B"  (optional)
saved = 2026-11-01T09:12:00+02:00

[[tabs]]
title = "api"
tags = ["dev"]
layout = "split-right 0.6"

[[tabs.panes]]
cwd = "~/code/api"
tool = ["nvim", "src/main.rs"]

[[tabs.panes]]
cwd = "~/code/api"
tool = ["cargo", "watch", "-x", "run"]
```

The keeper starts on demand: the first `shed` launch spawns `shedd` if its socket is missing. An optional systemd user unit (`shedd.service`) can start it at login instead.

## Implementation phases

Seven phases, about 13–20 working sessions with Claude to a daily driver, plus ongoing polish from real use. Each phase ends with a check you run yourself; the next phase starts only when it passes.

1. **Skeleton and first pane** (1–2 sessions)
   - Cargo workspace with all crates stubbed; GPUI window; one pane running your shell through `alacritty_terminal`; keyboard input and resize.
   - **Done when** `nvim`, `htop` and `ls --color` look right, and a big `cat` scrolls smoothly.
2. **The keeper** (2–3 sessions)
   - `shedd` owns PTYs; socket protocol; attach and detach; screen replay on reattach.
   - **Done when** you close the window with `nvim` open, reopen Shed, and `nvim` is exactly where you left it.
3. **Bench windows, tabs, splits** (3–5 sessions)
   - The full tree model; one window per bench; bench colours and badges in the tab bar, title and pane border; per-bench desktop entries and icons; tab tags and tag rules; keybindings; `shed.toml` with hot reload; themes; mouse in Shed's UI and in apps.
   - **Done when** three benches show as three distinct, coloured icons in your taskbar, and you can work a full day in Shed without falling back to another terminal.
4. **Logbook** (2–3 sessions)
   - `shed init` hooks for zsh, bash and fish; capture in the keeper; SQLite with FTS5; per-tab history files; Ctrl+Up and Ctrl+Shift+Up key bindings with the scope chip; the Ctrl+R overlay with scopes and tag filter; private tabs.
   - **Done when** Up, Ctrl+Up and Ctrl+Shift+Up each walk the right history in zsh, bash and fish, and a command run in one bench is findable from another bench in under 100 ms.
5. **Saving and loading benches** (2–3 sessions)
   - Autosave; load and save-as; tool detection; rack rules; prefilled commands; scrollback replay; open, detached and stored states; `shed load` and `shed save` CLI.
   - **Done when** after a reboot, clicking a pinned bench icon brings back that bench with its layout, directories, editors and dev servers.
6. **Pegboard and polish** (2–3 sessions, then ongoing)
   - Pegboard overview across all benches; scrollback search; URL and path detection; OSC 52 clipboard; Kitty keyboard protocol; IME; performance tuning.
   - **Done when** typing latency and throughput are in Alacritty's range on your machine.
7. **Packaging** (1 session)
   - `.desktop` file, icons in the hicolor theme, systemd user unit, AUR / `.deb` packaging, a short README.
   - **Done when** Shed installs from a package and shows its icon in your launcher.

With Claude writing the code, the pace is set by your testing between sessions. Real daily use in phases 3–6 surfaces most of the bugs that matter.

## Testing strategy

Most bugs should be caught without anyone looking at a window, because Claude can run tests but cannot see your screen.

- **Headless grid dumps**: a `shed-emu` test harness feeds bytes into the emulator and prints the grid as text. Snapshot tests (`insta`) catch regressions in rendering state.
- **Replay fidelity**: record real sessions (`nvim`, `htop`, `less`, `lazygit`) with `script`, replay them through the keeper and a fresh client emulator, and assert both grids match. This guards the trickiest part of phase 2.
- **Protocol tests**: an in-process keeper with a fake client covers attach, detach, resize and pane exit.
- **Logbook tests**: run real zsh, bash and fish in a PTY with the hooks loaded; assert the right entries, exit codes and scopes land in SQLite.
- **Bench round-trips**: save → load → save again must produce the same bench file. Generated desktop entries and icons are checked on GNOME and KDE in a VM.
- **Compliance**: `vttest` and `esctest` runs, recorded per release.
- **Performance**: a throughput benchmark (large `cat`, `seq 1 10000000`) compared against Alacritty on the same machine, and typing latency checked with a latency tool such as Typometer.
- **Your part**: daily use, plus a screenshot or short screen recording when something looks wrong.

## Icon set

The icon set has 38 SVG files: four app icon colour options, eight bench colour icons with templates for custom colours, and 21 UI icons. UI icons are 24 px with a 1.75 stroke in `currentColor`, so they follow the theme.

**App icon**: the shed with a terminal for a door, with a two-tone gradient roof, a gloss line on the roof edge, deep slate walls, and the prompt picking up the roof colour. Coral (`#FF9A5A` → `#F0475E`) is the default; indigo, teal and blue are alternatives.

**Bench icons are colours, not names.** Each bench takes one of eight colours; the icon is the shed in that colour. No bench names or letters are baked into icons.

| File | Used for |
| --- | --- |
| `app/shed.svg` | App icon (coral). Launcher, packaging, hicolor theme |
| `app/shed-option-<colour>.svg` | The four app icon options |
| `app/shed-symbolic.svg` | 16 px monochrome version for panels and small UI |
| `bench/shed-bench-<colour>.svg` | Bench icons: blue, violet, pink, coral, amber, green, teal, slate |
| `bench/bench-template.svg` | Template with `{{TOP}}` and `{{BOTTOM}}` gradient colours, for any custom colour |
| `bench/bench-template-badge.svg` | Same with an optional `{{BADGE}}`, for anyone who turns badges on |

**UI icons**

| Icon | Where it is used |
| --- | --- |
| `shed` | The app; the widest logbook scope |
| `bench`, `bench-new`, `bench-save`, `bench-load` | Bench menu, pegboard, bench scope in the logbook |
| `tab`, `tab-new`, `close` | Tab bar |
| `tag` | Tab tags, tag filter in the logbook |
| `pane`, `split-right`, `split-down` | Pane menu |
| `tool` | Pane is running a tool |
| `tool-waiting` | Command prefilled at the prompt after a load, waiting for Enter |
| `rack` | Rack settings; marks tools that relaunch automatically |
| `logbook`, `private` | Logbook overlay; private tab marker |
| `pegboard` | Pegboard overview button |
| `keeper`, `detach` | Keeper status; detach a bench window |

## Risks and open decisions

**Open decisions**

- [ ] **Emulation core: `alacritty_terminal` or `libghostty-vt`?** The plan assumes `alacritty_terminal`. Choose `libghostty-vt` only if inline images (Kitty graphics, image previews in file managers) matter; it adds a Zig toolchain and C bindings.
- [ ] **Which shells get hooks in v1?** The plan covers zsh, bash and fish; dropping any one saves part of a session.
- [ ] **Name check**: a quick search found no terminal project called Shed. Check crates.io, GitHub and your distro's packages before publishing.

* [ ] **Which desktop do you use (GNOME, KDE, or a tiling compositor such as Hyprland)?** It decides which taskbar-icon route gets tested first.

**Risks**

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Screen replay on reattach is subtly wrong for full-screen apps | Broken screens after reattach | Replay fidelity tests from phase 2 onward |
| GPUI API changes | Upgrades break the build | Pin a git revision; upgrade on purpose, one at a time |
| Loading a bench relaunches something harmful | Lost data or an unwanted deploy | The rack: only listed tools restart; everything else waits for Enter; `never` list |
| Shell hooks clash with an existing prompt setup (starship, oh-my-zsh, atuin) | Missing or duplicate log entries | Hooks chain onto existing hooks; tested against those three |
| Wayland and X11 differences | Clipboard, IME or focus quirks | Test both from phase 1; GPUI absorbs most differences |
| Remote shells over `ssh` have no hooks | Remote commands missing from the logbook | Out of scope for v1; later, `shed init` can be installed on servers |

**Taskbar icons per bench** depend on the desktop. The desktop-entry route works on GNOME and KDE; per-window icons on Wayland need compositor support for `xdg-toplevel-icon`, and the GPUI window-icon option is documented in the community fork (`gpui-ce`), so upstream GPUI may need a small patch. Mitigation: ship the desktop-entry route first, add per-window icons where they work.
