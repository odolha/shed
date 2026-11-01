# Shed

A GPU-accelerated Linux terminal whose shells outlive its windows. A small
background process, the keeper (`shedd`), holds every shell and program; the
window (`shed`) is only a view onto it. See
[`docs/initial/Shed — Implementation Plan.md`](docs/initial/Shed%20—%20Implementation%20Plan.md).

## Status

Phases 1–3 are in place: the first pane, the keeper, and bench windows with tabs and splits.

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
./target/release/shed stop      # stop the keeper and everything it holds
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
  A window refuses a keeper speaking another protocol version and says so.
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
  the clear colour); see `vendor/gpui_ce_wgpu/SHED_PATCHES.md`. Keep Shed's frames at
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
| `shed-logbook`, `shed-save` | Stubs for phases 4–5 |
