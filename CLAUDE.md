# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Shed is a GPU terminal for Linux (X11 and Wayland), in Rust. A background keeper
(`shedd`) owns every PTY; windows (`shed`) only draw. Work is grouped into
**benches** (each with its own window, colour and dock icon), made of **tabs**
made of **panes**. The Shed window itself is only the overview ("pegboard") of all
benches. Read `docs/architecture.md` before larger changes; `docs/user-guide.md`
describes the product as users see it.

## Commands

```sh
cargo build                          # both binaries: target/debug/shed, target/debug/shedd
cargo clippy --all-targets           # must stay clean (the libxkbcommon-x11 build warning is expected)
cargo test --workspace               # all tests; the shed-app ones run real bash
cargo test -p shed-app --test benches closed_benches   # one integration test, by name filter
cargo test -p shed-save closed_benches_change_tab_by_tab
```

Integration tests live in `crates/shed-keeper/tests/` (protocol, logout) and
`crates/shed-app/tests/` (benches, workspace, logbook). Both drive an in-process
keeper over a real socket with a fake window client (`crates/shed-app/tests/common/mod.rs`:
`start_in`, `Client::connect`, `Client::until`; small helpers such as `tree_until`
are repeated per test file).
`crates/shed-emu/tests/replay.rs` replays recordings in `crates/shed-emu/tests/recordings/`.

## Running a dev build safely

The user runs their own Shed on this machine, with the same window titles. A dev
build must never touch it:

```sh
export SHED_RUNTIME_DIR=/tmp/shed-dev XDG_CONFIG_HOME=/tmp/shed-dev/config \
       XDG_DATA_HOME=/tmp/shed-dev/data XDG_STATE_HOME=/tmp/shed-dev/state
./target/debug/shed
./target/debug/shed stop             # with the same env: stops only this keeper
```

- After changing keeper code, `shed stop` (same env) so the next launch starts the new keeper.
- Find dev windows by owning PID (process env `SHED_RUNTIME_DIR`), never by title.
- Stop Shed instances with the scoped `shed stop`, not `pkill`/`kill`.
- Don't send synthetic input or raise windows while the user is at the keyboard.
- `tools/screenshots/take.sh` builds a demo world (fake projects, prompt `you@shed`)
  and retakes `docs/images/*.png`; `INTERACTIVE=1` also clicks/types into its own
  windows for menu screenshots. Screenshots must not show usernames or hostnames.
- Logs: `$XDG_STATE_HOME/shed/shedd.log`; `SHED_LOG=debug`; `shedd --foreground`.

## Architecture

Crates (`crates/`), lowest first:

- `shed-model`: the tree (`Shed` → `Bench` → `Tab` → `PaneNode`/`Pane`), ids,
  `BenchColor`, settings (`config.rs`: `shed.toml`, themes, aliases), XDG paths.
  Built-in themes are `crates/shed-model/themes/*.toml`.
- `shed-proto`: socket framing and messages (`ClientMsg`, `KeeperMsg`, `Frame`).
- `shed-emu`: `Emulator` trait over vendored `alacritty_terminal`, replay snapshot,
  key/mouse encoding (xterm and kitty), OSC 133/7/7777 shell-mark parsing.
- `shed-save`: saved bench TOML (`SavedBench`), layouts as text, rack rules,
  zstd scrollback, `session.toml`.
- `shed-logbook`: SQLite command history; embeds the bash hook `shell/shed.bash`.
- `shed-desktop`: bench icons (SVG template recoloured, resvg) and desktop entries.
- `shed-keeper`: `shedd`. Nearly everything is in `src/server.rs`.
- `shed-app`: `shed` binary: GPUI windows and the CLI.

### How a change flows

Every mutation goes through the keeper; windows render from the tree it broadcasts.

1. A window or CLI sends a `ClientMsg` (`shed-proto/src/lib.rs`).
2. `Keeper::handle_msg` in `shed-keeper/src/server.rs` mutates `State.tree` (under
   `self.lock()`) and calls `broadcast_tree()`, which sends `KeeperMsg::TreeChanged`
   to every client.
3. In the app, `AppState` (`shed-app/src/state.rs`, one GPUI entity) stores the tree
   and opens, closes or updates windows; views re-render from it.

Closed (saved, not running) benches are edited differently: the app sends
`ClientMsg::EditSaved { name, edit: SavedEdit }`; `Keeper::edit_saved` loads the
file, applies the edit through `SavedBench` methods in `shed-save`, and saves it.
The pegboard reads closed benches from `KeeperMsg::Pegboard(PegboardInfo)`, kept in
the `PegboardData` GPUI global. UI that edits a bench (`pegboard.rs`,
`setup_panel.rs`) branches on running vs closed (`BenchKey::Running/Closed`) and
sends the matching message.

Terminal bytes bypass control messages: `Frame::Input`/`Frame::Output` carry raw
bytes per pane. The keeper runs an emulator per pane (answers terminal queries,
produces replay on attach); each window runs its own emulator per visible pane
(`terminal_view.rs`), fed by `KeeperMsg::Replay` then live output.

### shed-app map

- `main.rs`: CLI (clap) and app start; the window process is single-instance
  (a second `shed` asks the keeper to `Show` something instead).
- `keeper_conn.rs`: socket connection; starts `shedd` if needed; version check.
- `state.rs`: `AppState`: tree, settings, per-pane `TerminalView`s, window handles,
  notices; handles every `KeeperMsg`.
- `shed_window.rs` hosts `pegboard.rs` (the overview: bench rows, tab cards,
  menus, drag and drop, inline editors). `bench_window.rs`: a bench's window (tab
  bar, splits). `setup_panel.rs`: startup/teardown/aliases drawer.
  `logbook_view.rs`: Ctrl+R history. `menu.rs`: shared context menus and tooltips.
  `line_editor.rs`: the single-line text field. `actions.rs`: keybindings.
- `theme.rs`: `UiColors` (chrome colours from the theme's `[ui]` section, with
  `accent` and `danger`). Use `ui.ink(color)` for text/icons in a bench or tag
  colour, so they stay readable on light themes.
- `assets.rs`: UI icons from `assets/icons/ui/*.svg`; a new icon must be added to
  the `icons![...]` list there. Use them with `svg().path(icon("name"))`.

### Vendored crates

`vendor/` holds patched `alacritty_terminal` (read-only accessors for replay),
`gpui_ce_wgpu` (first full-window quad becomes the clear colour) and
`gpui_ce_linux` (X11 key releases not dropped), wired in through
`[patch.crates-io]`. Each has a `SHED_PATCHES.md`; patched lines are marked
`// Shed patch`. GPUI is `gpui-ce =0.3.0`, pinned exactly.

## Rules

- **Protocol**: postcard encodes enum variants by position. Append new variants at
  the end of their enum, never reorder, bump `PROTOCOL_VERSION` with any message
  change. `Hello`, `Shutdown` and `Error` keep their positions (a test pins them).
- **Saved files** (`shed-save`, `shed-model` config): new fields need serde
  defaults so older files still load.
- **Drawing**: keep a frame to one window-sized layer; every extra full-window
  quad costs ~4 ms on integrated GPUs with fractional scaling.
- **UI**: actions get icon + text where there is room; icon-only is for mode
  toggles (with a tooltip). Shed's accent is coral, kept distinct from `danger` red;
  destructive actions use `danger` and ask first (with "…" in the label).
- **Docs**: a visible change updates `docs/user-guide.md` (and screenshots); a
  change in how things work updates `docs/architecture.md`; build/test changes
  update `docs/development.md`. Same commit as the code.
- **Commits**: a sentence-case summary of what changed for the user, no
  conventional-commit prefix; bullet details in the body.
- Linux only for now; macOS and Windows (via WSL) may come later.

## Packaging

`packaging/deb/build.sh` builds the `shed-terminal` .deb (Debian already has a
`shed` package); `packaging/arch/PKGBUILD` builds `shed-terminal-git` from GitHub;
`packaging/linux/` has the desktop entry (`dev.shed`), optional `shedd.service`.
App icons: `cargo run -p shed-desktop --example app_icons -- <dir>`.
