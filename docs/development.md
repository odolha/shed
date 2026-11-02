# Working on Shed

How to build, run and test Shed from source. See [How Shed works](architecture.md)
for the design.

## Build and run

```sh
cargo build
./target/debug/shed
```

Release builds carry no debug info; `cargo build --profile profiling` keeps line
tables, for profilers.

**Run a separate Shed beside your own**: everything Shed uses can be pointed
elsewhere, so a development build never touches your real benches, settings or
launchers.

```sh
export SHED_RUNTIME_DIR=/tmp/shed-dev        # its own keeper and socket
export XDG_CONFIG_HOME=/tmp/shed-dev/config  # its own shed.toml
export XDG_DATA_HOME=/tmp/shed-dev/data      # its own benches and launchers
export XDG_STATE_HOME=/tmp/shed-dev/state    # its own logbook and logs
./target/debug/shed
```

**After changing the keeper**, the keeper still running has the old code: run
`shed stop`. A window refuses a keeper of another protocol version and says so.

## Tests

```sh
cargo test --workspace
cargo clippy --all-targets
```

- Unit tests sit with the code.
- `crates/shed-emu/tests/replay.rs` replays real program output
  (`tests/recordings/`, recorded with `script` at 40×12), cut at many points, and
  checks that a replayed emulator matches the original.
- `crates/shed-keeper/tests/` drives an in-process keeper over its socket.
- `crates/shed-app/tests/` runs whole flows with real bash: the logbook, saving and
  loading, setup and teardown, aliases, closing Shed. `tests/zsh.rs` does the same
  with zsh when there is one (`zsh` on `PATH`, or `SHED_TEST_ZSH=/path/to/zsh`), and
  is skipped otherwise.

## Debugging

- The keeper logs to `~/.local/state/shed/shedd.log`; `SHED_LOG=debug` raises the
  level for both binaries; `shedd --foreground` logs to stderr.
- `shed debug-type 'echo hi\r'` types into the first pane through the keeper.
- `SHED_PERF=1` makes the window process print redraws per second and their cost.
  It applies to the process that owns the windows, so use a separate
  `SHED_RUNTIME_DIR`. Per-process GPU time is in `/proc/<pid>/fdinfo/*`
  (`drm-engine-render`).

## Rules worth knowing

- **Protocol**: messages travel by enum position. New variants go at the end; bump
  `PROTOCOL_VERSION` with any change; never move `Hello`, `Shutdown` or `Error`.
- **Saved files**: new fields need serde defaults, so benches saved by older
  versions still load.
- **Drawing**: keep frames to one window-sized layer; each extra full-window quad
  costs about 4 ms on an integrated GPU with fractional scaling.
- **Docs**: a change people can see updates the [User Guide](user-guide.md) (and
  its screenshots); a change in how things work updates
  [How Shed works](architecture.md). `CLAUDE.md`, the briefing for AI coding
  agents, follows changes to commands, structure and these rules.

## Screenshots and icons

`tools/screenshots/take.sh` (X11, `wmctrl`, `ffmpeg`) builds a demo world with a
neutral prompt and a few benches (`demo-world.sh`), runs a separate Shed in it, and
writes the screenshots in `docs/images/`. Run it from the repository root after
`cargo build`; with `INTERACTIVE=1` it also opens menus and panels for their
screenshots, clicking and typing into its own windows only (it stops if they lose
focus).

`cargo run -p shed-desktop --example icon_preview -- <dir>` renders the bench icon in
every colour; `--example app_icons -- <dir>` writes the app icon's hicolor tree, as
the packages do.

## Packages

- `packaging/deb/build.sh` builds the Debian package into `target/debian/`.
- `packaging/arch/PKGBUILD` builds the latest commit from GitHub (`makepkg -si`).
- `packaging/linux/` holds the desktop entry, the optional systemd user unit and
  the copyright notice they share.
