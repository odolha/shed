# Shed

A GPU-accelerated Linux terminal whose shells outlive its windows. A small
background process, the keeper (`shedd`), holds every shell and program; the
window (`shed`) is only a view onto it. See
[`docs/initial/Shed — Implementation Plan.md`](docs/initial/Shed%20—%20Implementation%20Plan.md).

## Status

Phases 1 and 2 (first pane, the keeper) are in place:

- One window running your shell, drawn with GPUI (`gpui-ce`), emulated by `alacritty_terminal`.
- The keeper owns every PTY. Closing the window detaches; the next `shed`
  reattaches and gets each pane's exact state replayed (scrollback, primary
  and alternate screens, cursor, modes, colours, title) before live output resumes.
- Keyboard (xterm encoding, application cursor mode), paste (Ctrl+Shift+V,
  bracketed when asked), wheel scrollback (Shift+PageUp/PageDown too), mouse
  reporting to apps that ask, OSC 52 copy, font zoom (Ctrl+= / Ctrl+- / Ctrl+0).
- Query replies triggered by binary output (`cat image.png`) are dropped, so
  they never show up at the prompt.

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
- `SHED_FONT` and `SHED_FONT_SIZE` override the font until `shed.toml` lands in phase 3.
- `shed debug-type 'echo hi\r'` types into the first pane through the keeper,
  for testing without a keyboard.

| Crate | Role |
| --- | --- |
| `shed-model` | The bench → tab → pane tree, IDs, colours, XDG paths |
| `shed-proto` | Socket messages and framing (raw bytes for terminal I/O) |
| `shed-emu` | The `Emulator` trait, the `alacritty_terminal` backend, key and mouse encoding |
| `shed-keeper` | `shedd`: PTYs, socket server |
| `shed-app` | `shed`: GPUI windows and the CLI |
| `shed-logbook`, `shed-save`, `shed-desktop` | Stubs for phases 3–5 |
