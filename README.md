# Shed

A GPU-accelerated Linux terminal whose shells outlive its windows. A small
background process, the keeper (`shedd`), holds every shell and program; the
window (`shed`) is only a view onto it. See
[`docs/initial/Shed — Implementation Plan.md`](docs/initial/Shed%20—%20Implementation%20Plan.md).

## Status

Phases 1–6 are in place: the first pane, the keeper, bench windows with tabs and splits,
the logbook, saving and loading benches, and the pegboard with the rest of phase 6.

- **The keeper** owns every PTY. Closing a window detaches its bench. The next `shed`
  reattaches it and replays each pane exactly: scrollback, both screens, cursor,
  modes, colours and title.
- **Benches** each get their own window, colour and taskbar icon. Every bench writes a
  hidden desktop entry and icon (`dev.shed.bench.<name>`) to `~/.local/share`, so
  docks show one icon per bench, and pinning one gives a launcher (`shed load <name>`).
- **Tabs and splits**: tags (by hand, from `shed tag`, or from tag rules), tinting,
  private tabs, inline rename, drag to reorder, and a menu to move a tab to another bench.
- **Mouse**: selection (Shift overrides apps' mouse mode), copy to the primary selection,
  Ctrl+click for links and paths, and right-click menus. Split borders drag.
- **Config**: `~/.config/shed/shed.toml` reloads live. It sets fonts, themes, keys, tag rules and the rack.
- **Logbook**: every command run in bash is kept with its directory, exit code, time,
  tab, bench and tags. Up walks this tab's history, Ctrl+Up the bench's, Ctrl+Shift+Up
  everything; typing first narrows the walk to commands starting with it. Ctrl+R at a
  prompt opens a search (Tab cycles scopes, `tag:prod` filters, Enter inserts,
  Alt+Enter runs). Commands typed with a leading space are not kept. `shed log` lists
  entries from a shell.
- **Saved benches**: every bench saves itself every minute and when the keeper stops
  (`shed stop`, logout, reboot) to `~/.local/share/shed/benches/<name>.toml`, with each
  pane's scrollback beside it. `shed` with nothing running brings back the benches that
  ran last; `shed load <name>` (and a pinned bench icon) loads one. Panes come back in
  their folders with their old scrollback dimmed above the prompt; tools on the rack
  restart, others are typed and wait for Enter, the never list is left out.
- **Setup and teardown**: each pane can have a folder to start in, startup commands
  (run one per prompt when the bench loads, instead of what was running; or typed,
  waiting for Enter) and teardown commands. The Setup panel (Ctrl+Shift+S, the rack
  button, or a tab's menu) shows and edits them for the current tab; so do
  `shed setup` and `shed setup --edit`.
- **Hide or close**: closing a window hides the bench, its tools keep running. Close
  Bench saves it, stops each pane's tool (Ctrl+C, then SIGTERM), runs the teardown
  commands at the prompt (`docker compose down`), and ends it; it stays saved.
- **Aliases** at three levels, over the shell's own: global
  (`~/.config/shed/aliases.toml`), bench and tab. Shells pick changes up at their next
  prompt; taking one away brings the shell's version back. The Setup panel lists them
  with what they override; `shed alias`, `shed alias add [--bench|--global] NAME CMD`
  and `shed alias rm` work from a shell.
- **Pegboard** (Super+Space or Ctrl+Shift+Space): every bench, open, detached or saved,
  with its tabs and a live preview of each. Type to filter by bench, tab, tag, folder
  or tool; arrows and Enter (or a click) go there, opening or loading the bench as
  needed; drag a tab onto another bench to move it.
- **Find in scrollback** (Ctrl+Shift+F): plain text, case-insensitive unless the query
  has capitals; Enter goes to older matches, Shift+Enter to newer.
- **Keyboard**: the kitty keyboard protocol for apps that ask for it, and input methods
  (IBus, Fcitx) with the text being composed shown at the cursor.
- Query replies triggered by binary output (`cat image.png`) are dropped.

| Keys | Action |
| --- | --- |
| Ctrl+Shift+T / Ctrl+Shift+W | New tab / close pane |
| Ctrl+Shift+E / Ctrl+Shift+O | Split right / split down |
| Alt+arrows | Move between panes |
| Ctrl+Tab, Ctrl+1…9 | Switch tabs |
| Ctrl+Shift+N / Ctrl+Shift+Q | New bench / close bench (stops its tools) |
| Super+1…9 | Raise bench windows |
| F2 / Shift+F2 | Rename tab / bench |
| Ctrl+Shift+C / Ctrl+Shift+V | Copy / paste |
| Up / Ctrl+Up / Ctrl+Shift+Up | Walk this tab's / bench's / all history |
| Ctrl+R (at a prompt) | Search the logbook |
| Ctrl+Shift+F | Find in scrollback |
| Ctrl+Shift+S | Setup panel: startup, teardown, aliases |
| Super+Space, Ctrl+Shift+Space | Pegboard |

```toml
# ~/.config/shed/shed.toml (every key optional)
theme = "shed-dark"            # or shed-light, or themes/<name>.toml
[font]
family = "DejaVu Sans Mono"
size = 14
[window]
new_pane_dir = "current"       # or "bench"
icon_badges = false
[keys]
"ctrl-shift-d" = "split_right"
"alt-left" = "none"
autosave_seconds = 60
[shell]
integration = true             # load Shed's bash hook into panes
[rack]                         # what restarts by itself when a bench loads
auto = ["nvim", "vim", "hx", "ssh", "htop", "btop", "less", "tail -f", "cargo watch", "npm run dev"]
never = ["rm", "git push", "sudo"]
[[tag_rule]]
tool = "ssh *prod*"
tag = "prod"
color = "red"
tint_tab = true
```

`alacritty_terminal` is vendored with read-only accessors the replay needs;
see `vendor/alacritty_terminal/SHED_PATCHES.md`.

## Build and run

```sh
cargo build --release
./target/release/shed           # starts shedd if needed, opens the bench window
./target/release/shed status    # what the keeper holds
./target/release/shed stop      # save everything, then stop the keeper and its tools
./target/release/shed list      # benches: open, detached, saved
./target/release/shed load web  # open a bench, loading it if it is saved
./target/release/shed save web --as web-template
./target/release/shed delete web
./target/release/shed setup     # this tab's startup and teardown (--edit to change them)
./target/release/shed alias     # aliases by level; alias add/rm to change them
```

`shed` looks for `shedd` next to its own binary, then on `PATH`. The keeper
exits by itself once it holds no panes and no window is connected.

Build dependencies (Debian/Ubuntu): `libxkbcommon-dev libxkbcommon-x11-dev
libwayland-dev libxcb1-dev libfontconfig-dev` and a Vulkan driver. Without
`libxkbcommon-x11-dev`, `shed-app/build.rs` links the runtime library directly
and prints a warning.

## Development

- `SHED_RUNTIME_DIR=/tmp/shed-dev` runs a separate keeper beside your real one.
- A running keeper keeps its old code: after changing `shed-keeper`, run `shed stop`.
  A window refuses a keeper speaking another protocol version and says so. Messages
  travel by enum position, so new ones go at the end; `shed stop` must keep working
  against older keepers.
- `crates/shed-emu/tests/recordings/` holds real program output (recorded with
  `script` at 40×12) that the replay tests cut at many points.
- Logs: the keeper writes `~/.local/state/shed/shedd.log`; `SHED_LOG=debug` raises
  the level for both binaries (`shedd --foreground` logs to stderr).
- `XDG_CONFIG_HOME` and `XDG_DATA_HOME` keep a test run's config and desktop entries
  away from your real ones.
- `shed debug-type 'echo hi\r'` types into the first pane through the keeper,
  for testing without a keyboard.
- `SHED_PERF=1` makes the window process print redraws per second and their cost.
  It only applies to the process that owns the windows, so close every Shed window
  first, or use a separate `SHED_RUNTIME_DIR`. Per-process GPU time is in
  `/proc/<pid>/fdinfo/*` (`drm-engine-render`).
- `gpui_ce_wgpu` is vendored with one renderer patch (window-sized backgrounds become
  the clear colour); see `vendor/gpui_ce_wgpu/SHED_PATCHES.md`. `gpui_ce_linux` is
  vendored so X11 key releases are not lost; see `vendor/gpui_ce_linux/SHED_PATCHES.md`. Keep Shed's frames at
  one window-sized layer: GPUI's quad shader costs ~4 ms per full-window quad on an
  integrated GPU.

| Crate | Role |
| --- | --- |
| `shed-model` | The bench → tab → pane tree, IDs, colours, config and themes, XDG paths |
| `shed-proto` | Socket messages and framing (raw bytes for terminal I/O) |
| `shed-emu` | The `Emulator` trait, the `alacritty_terminal` backend, key and mouse encoding |
| `shed-keeper` | `shedd`: PTYs, socket server |
| `shed-desktop` | Bench icons and desktop entries |
| `shed-app` | `shed`: GPUI windows and the CLI |
| `shed-logbook` | The logbook (SQLite) and the bash hook (`shell/shed.bash`) |
| `shed-save` | Saved bench files, layouts as text, rack rules, scrollback |
