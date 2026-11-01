# Shed

A GPU-accelerated Linux terminal whose shells outlive its windows. A small
background process, the keeper (`shedd`), holds every shell and program; the
window (`shed`) is only a view onto it. See
[`docs/initial/Shed — Implementation Plan.md`](docs/initial/Shed%20—%20Implementation%20Plan.md).

## Status

Phases 1–6 are in place: the first pane, the keeper, bench windows with tabs and splits,
the logbook, saving and loading benches, and Shed's own window with the rest of phase 6.

- **Shed** (`shed`, the Shed icon) is one window: the pegboard. It shows every bench,
  open, hidden or closed, with its tabs and a live preview of each. From it you make
  benches, open, hide, close and delete them, go to a tab, and drag tabs between
  benches. Type to filter by bench, tab, tag, folder or tool; arrows and Enter (or a
  click) go there. Nothing starts by itself: Shed opens with its benches closed, and
  opening one brings back what was saved. With `auto_restore = true`, Shed reopens
  what was open or hidden when it last closed.
- **Closing Shed closes everything**, after asking: every bench is saved, its programs
  stop and its teardown commands run, then Shed quits.
- **Benches** each get their own window, colour and taskbar icon (a workbench in the
  bench colour). A bench's window manages only its own tabs and panes; Super+Space or
  the house button brings Shed forward. Closing a bench's window hides the bench: it
  keeps running, and Shed can show it again. Every bench writes a hidden desktop entry
  and icon (`dev.shed.bench.<key>`) to `~/.local/share`, so docks show one icon per
  bench, and pinning one gives a launcher (`shed load <name>`), which also starts Shed.
- **The keeper** (`shedd`) owns every PTY, so a hidden bench, or one whose window
  crashed, keeps running and comes back exactly: scrollback, both screens, cursor,
  modes, colours and title.
- **Tabs and splits**: tags (by hand, from `shed tag`, or from tag rules), tinting,
  private tabs, inline rename, drag to reorder.
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
  pane's scrollback beside it. Opening a closed bench (from Shed, `shed load <name>` or
  a pinned bench icon) loads it. Panes come back in
  their folders with their old scrollback dimmed above the prompt; tools on the rack
  restart, others are typed and wait for Enter, the never list is left out.
- **Setup and teardown**: each pane can have a folder to start in, startup commands
  (run one per prompt when the bench loads, instead of what was running; or typed,
  waiting for Enter) and teardown commands. The Setup panel (Ctrl+Shift+S, the rack
  button, or a tab's menu) shows and edits them for the current tab; so do
  `shed setup` and `shed setup --edit`.
- **Hide or close**: hiding a bench (closing its window) keeps its tools running. Closing
  it (from Shed or the bench's menu) saves it, stops each pane's tool (Ctrl+C, then
  SIGTERM), runs the teardown commands at the prompt (`docker compose down`), and ends
  it; it stays saved.
- **Aliases** at three levels, over the shell's own: global
  (`~/.config/shed/aliases.toml`), bench and tab. Shells pick changes up at their next
  prompt; taking one away brings the shell's version back. The Setup panel lists them
  with what they override; `shed alias`, `shed alias add [--bench|--global] NAME CMD`
  and `shed alias rm` work from a shell.
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
| Ctrl+Shift+N (in Shed) / Ctrl+Shift+Q | New bench / close bench (stops its tools) |
| Super+1…9 | Raise bench windows |
| F2 / Shift+F2 | Rename tab / bench |
| Ctrl+Shift+C / Ctrl+Shift+V | Copy / paste |
| Up / Ctrl+Up / Ctrl+Shift+Up | Walk this tab's / bench's / all history |
| Ctrl+R (at a prompt) | Search the logbook |
| Ctrl+Shift+F | Find in scrollback |
| Ctrl+Shift+S | Setup panel: startup, teardown, aliases |
| Super+Space, Ctrl+Shift+Space | Bring Shed forward |

```toml
# ~/.config/shed/shed.toml (every key optional)
theme = "shed-dark"            # or shed-light, or themes/<name>.toml
autosave_seconds = 60
auto_restore = false           # reopen what was open or hidden when Shed last closed
[font]
family = "DejaVu Sans Mono"
size = 14
[window]
new_pane_dir = "current"       # or "bench"
icon_badges = false
[keys]
"ctrl-shift-d" = "split_right"
"alt-left" = "none"
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

## Install

The package is called `shed-terminal`: Debian, Ubuntu and Arch already have a `shed`
(a hex editor) that also installs `/usr/bin/shed`, so the two conflict.

**Debian, Ubuntu, Mint**: build the package, then install it.

```sh
packaging/deb/build.sh
sudo apt install ./target/debian/shed-terminal_0.1.0-1_amd64.deb
```

**Arch**: `cd packaging/arch && makepkg -si` builds this source tree.

Either installs `shed` and `shedd` in `/usr/bin`, Shed in the application menu
(`dev.shed.desktop`, with icons in the hicolor theme), and an optional systemd user unit
that starts the keeper at login:

```sh
systemctl --user enable --now shedd.service
```

Without it, `shed` starts the keeper when needed. Under systemd, the shells in Shed's
panes inherit the user manager's environment, which only has `DISPLAY`,
`SSH_AUTH_SOCK` and the like if the desktop imports them; check with
`systemctl --user show-environment`.

**From source, without a package**: `cargo build --release` and run
`./target/release/shed`. Shed then writes its own launcher and icon to
`~/.local/share` (and removes them once a package provides them).

Build dependencies (Debian/Ubuntu): `libxkbcommon-dev libxkbcommon-x11-dev
libwayland-dev libxcb1-dev libfontconfig-dev` and a Vulkan driver. Without
`libxkbcommon-x11-dev`, `shed-app/build.rs` links the runtime library directly
and prints a warning.

## Use

```sh
shed                 # starts shedd if needed, opens the Shed window
shed list            # benches: open, hidden, closed
shed load web        # open a bench, loading it if it is closed
shed new --name api  # a new bench
shed save web --as web-template
shed delete web
shed setup           # this tab's startup and teardown (--edit to change them)
shed alias           # aliases by level; alias add/rm to change them
shed status          # what the keeper holds
shed stop            # save everything, then stop the keeper and its tools
```

`shed` looks for `shedd` next to its own binary, then on `PATH`. The keeper
exits by itself once it holds no panes and no window is connected.

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
- Icons: `cargo run -p shed-desktop --example icon_preview -- <dir>` renders the bench
  icon in every colour; `--example app_icons -- <dir>` writes the app icon's hicolor
  tree, as the packages do.
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
