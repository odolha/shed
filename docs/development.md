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

**The terminal interface** runs the same way, with the same variables, in a
terminal of its own: `./target/debug/shed tui`. Never in the terminal your own
Shed's panes are in if you are testing keeper changes; the dev keeper is
separate only through `SHED_RUNTIME_DIR`.

### A build without windows

For a server: the CLI and `shed tui`, with no GPUI and no display libraries.

```sh
cargo build --release -p shed-app --no-default-features --features tui
cargo build --release -p shed-keeper
# nothing from GPUI, X11 or Wayland:
cargo tree -p shed-app --no-default-features --features tui -e normal,build | grep -iE "gpui|x11|wayland"
cargo clippy -p shed-app --no-default-features --features tui --all-targets
```

(`-e normal,build`: GPUI stays a dev-dependency, for the window code's own
tests.) CI builds this variant too.

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
  is skipped otherwise. `ai_tabs.rs` and `ai_questions.rs` run AI tabs against a
  stand-in `claude` (a script named by `SHED_CLAUDE` that answers the control
  protocol and plays a recorded stream); `remote_tabs.rs` runs remote tabs against
  a stand-in `ssh` (`SHED_SSH`) that runs the "server" locally, with its own
  home.
- `crates/shed-tui/tests/tui.rs` drives the terminal interface's `App`
  against an in-process keeper with real bash and draws it on ratatui's
  `TestBackend`: no terminal is touched. Its unit tests cover the split layout,
  keys to bytes, Markdown to styled lines and text wrapping.
- `crates/shed-ai/tests/streams/` holds recorded Claude Code streams the
  transcript tests replay; `shed-git`'s tests make throwaway repositories (and a
  bare remote) with the real `git`.
- Some window code is tested headless with GPUI's test platform
  (`#[gpui::test]`): text layout and selection, scrollbars, the usage bars, a
  bench window drawing the limits when they arrive. `KeeperConn::detached()`
  stands in for the keeper there.

### API types

The nexus API's types are Rust, in `crates/shed-api`; the web and Android apps
use generated copies, committed so they build without Rust:

```sh
cargo run -p shed-codegen            # writes web/src/api/gen/api.ts and
                                     # android/app/src/main/java/dev/shed/api/Api.kt
tools/check-codegen.sh               # fails if they are not what shed-api makes now
```

`shed-codegen` reads every `#[typeshare]` type with `typeshare-core` (no CLI to
install) and uses its TypeScript and Kotlin generators, adjusted so the types
describe serde's JSON: `Option` fields are `T | null` in TypeScript, enums
without data string unions, and Kotlin sealed-class variants name their content
type with its package. Types share one namespace, so two shed-api types may not
have the same name. Its tests (`crates/shed-codegen/tests/json.rs`) pin the JSON
of each kind of type, fail when the generated files are stale, and type-check
samples of every frame against `api.ts` with `tsc --strict` when there is one
(`tsc` on `PATH`, or `SHED_TSC="npx -y -p typescript@5 tsc"`); the Kotlin
`Json` configuration to use is at the top of `Api.kt`.

## Debugging

- The keeper logs to `~/.local/state/shed/shedd.log`; `SHED_LOG=debug` raises the
  level for both binaries; `shedd --foreground` logs to stderr.
- `shed debug-type 'echo hi\r'` types into the first pane through the keeper.
- `SHED_PERF=1` makes the window process print redraws per second and their cost,
  for terminals and for the Shed window, and how long each replay took.
  `SHED_PERF_FRAMES=60` makes the pegboard render 60 times a second, as fast
  hovering does, to measure a frame without moving the mouse.
  It applies to the process that owns the windows, so use a separate
  `SHED_RUNTIME_DIR`. Per-process GPU time is in `/proc/<pid>/fdinfo/*`
  (`drm-engine-render`).
- An AI tab's frame cost with a long conversation, headless (no window):
  `SHED_PERF_SESSION=<folder>:<session id> cargo test --release -p shed-app --bins
  perf_long -- --ignored --nocapture` loads that Claude Code session and prints
  the time per frame (a full redraw, a hover in one entry, a change to the
  conversation) and how many entries each frame draws.

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

`tools/screenshots/take.sh` builds a demo world (`demo-world.sh`: a neutral
prompt, a few projects with Git history and changes, benches with terminal,
project and AI tabs, a made-up Claude Code session in the demo home, usage limits),
runs a separate Shed in it on a virtual X display of its own (Xvfb, at 2× density,
with Mesa's software Vulkan), and writes the screenshots in `docs/images/`. Nothing
shows on your screen and your input is untouched, so it can run while you work.
Panels are opened with keys sent to that display only (`xtype.py`); the mouse does
not reach GPUI there (Xvfb has no XInput pointer of its own), so the two menu
screenshots (`colours.png`, `tab-menu.png`) are not retaken. Needs Xvfb (`XVFB=`
its path when not installed: `apt-get download xvfb` and `dpkg-deb -x` give one
without root), `xrdb`, `xwininfo`, `xwd`, `ffmpeg` and Python's PIL. Run it from the
repository root after `cargo build`.

`cargo run -p shed-desktop --example icon_preview -- sheet.png [template.svg]` draws a
sheet of the bench icon in every colour, at launcher and Dock sizes, on dark and
light, beside Shed's icon (with a template, that one instead of the built-in);
`--example app_icons -- <dir>` writes the app icon's hicolor tree, as the packages
do.

## macOS

`tools/mac-check.sh` lints the macOS build from Linux
(`rustup target add aarch64-apple-darwin` first): it stands in for Apple's C
compiler, so it checks and lints but cannot link. To run it, use a Mac, or the
CI: `.github/workflows/ci.yml` builds, lints and tests on Linux and macOS, and
uploads `Shed.app` (zipped) from the macOS run.

## Remote tabs

`shed remote connect` copies the `shedd` beside `shed` to servers, preferring
`shedd-<arch>-static` if it is there. A static build runs on any Linux:

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release -p shed-keeper --target x86_64-unknown-linux-musl
cp target/x86_64-unknown-linux-musl/release/shedd target/release/shedd-x86_64-static
```

`SHED_SSH` stands a program in for ssh (tests run the "server" side locally with
it) and `SHED_REMOTE_SHEDD` chooses the binary to copy.
`crates/shed-keeper/tests/attach.rs` drives `shedd attach` against an in-process keeper
(two attaches sharing a shell, `shedd list`). `crates/shed-app/tests/remote_tabs.rs`
runs whole remote tabs through a stand-in ssh script whose "server" is a folder
with its own home and Shed folders (it needs `target/debug/shedd`, which
`cargo test --workspace` builds; skipped otherwise).

## The nexus

`shed-nexus` runs on its own; point its settings and data at a scratch folder.
A `localhost` public URL keeps `Secure` cookies working over plain HTTP:

```sh
mkdir -p /tmp/nexus-dev
cat > /tmp/nexus-dev/nexus.toml <<'EOF'
listen = "127.0.0.1:7870"
public_url = "http://localhost:7870"
data_dir = "/tmp/nexus-dev/data"
# web_dir = "web/dist/browser"   # a built web app, when there is one
EOF
cargo run -p shed-nexus -- --config /tmp/nexus-dev/nexus.toml
```

Until the account exists it logs a setup link (`…/setup?code=…`). Without the
web app, set up with `curl`: `POST /api/v1/setup` with the code, a user name and
a password gives the TOTP secret (add it to an authenticator, or compute codes
with `oathtool --totp -b SECRET`), and `POST /api/v1/setup/confirm` with the
code, a TOTP code and `"device": {"kind": "phone", "name": "curl"}` gives a
device token for `Authorization: Bearer`. `SHED_LOG=debug` raises the log
level. Deleting the data folder starts over.

`crates/shed-nexus/tests/` run a nexus in process on a real socket, with a
temporary data folder: the account (setup, sign-in, lockout, cookies and CSRF),
pairing, and relaying, where a fake link and connectors speak over
`tokio-tungstenite`. TOTP codes come from the account's secret; each time step
is taken once, so a test that needs more than three codes waits for the next
step. Recovery codes stand in where a test only needs to sign in.

## Packages

- `packaging/deb/build.sh` builds the Debian package into `target/debian/`.
- `packaging/macos/bundle.sh` builds `Shed.app` (and a zip of it) into
  `target/macos/`, on a Mac.
- `packaging/arch/PKGBUILD` builds the latest commit from GitHub (`makepkg -si`).
- `packaging/linux/` holds the desktop entry, the optional systemd user unit and
  the copyright notice they share.
- `packaging/nexus/` holds a sample `nexus.toml` and the nexus's systemd unit;
  [Deploying a nexus](nexus-deploy.md) uses them.
