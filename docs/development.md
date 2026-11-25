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

**Install your build for daily use** (Linux): `tools/reinstall.sh` compiles
Shed's own crates (and the vendored ones) again from scratch, whatever changed,
builds `shed` and `shedd` together in release (in `target/install`, apart from
dev builds), puts them in `~/.cargo/bin`, then offers to restart Shed so the
keeper runs the new build too (`-y` restarts without asking, `--no-restart`
does not, `--quick` keeps what cargo already compiled). The restart is `shed
stop` (benches saved first; programs in Shed's terminals and Claude Code
sessions stop) and a new `shed`, from a process of its own, so running it in a
Shed terminal is fine; its output goes to `~/.local/state/shed/reinstall.log`.
Bench launchers point at the new binaries once Shed starts.

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
cargo clippy --workspace --all-targets
```

- Unit tests sit with the code.
- `crates/shed-emu/tests/replay.rs` replays real program output
  (`tests/recordings/`, recorded with `script` at 40×12), cut at many points, and
  checks that a replayed emulator matches the original.
- `crates/shed-keeper/tests/` drives an in-process keeper over its socket.
- `crates/shed-app/tests/` runs whole flows with real bash: the logbook, saving and
  loading, setup and teardown, aliases, closing Shed. `tests/zsh.rs` does the same
  with zsh when there is one (`zsh` on `PATH`, or `SHED_TEST_ZSH=/path/to/zsh`), and
  is skipped otherwise. `ai_tabs.rs`, `ai_questions.rs` and `ai_agents.rs` run AI
  tabs against a stand-in `claude` (a script named by `SHED_CLAUDE` that answers
  the control protocol and plays a recorded stream); `remote_tabs.rs` runs remote tabs against
  a stand-in `ssh` (`SHED_SSH`) that runs the "server" locally, with its own
  home.
- `crates/shed-tui/tests/tui.rs` drives the terminal interface's `App`
  against an in-process keeper with real bash and draws it on ratatui's
  `TestBackend`: no terminal is touched. Its unit tests cover the split layout,
  keys to bytes, Markdown to styled lines and text wrapping.
- `crates/shed-ai/tests/streams/` holds recorded Claude Code streams the
  transcript tests replay (`agents.jsonl`: sub-agents and a background command;
  `timers.jsonl`: a wake-up, a scheduled job and a watch firing), and
  `tests/sessions/` the session files Claude Code saved for the same runs, with
  their `subagents/`. They were recorded from the real `claude -p
  --input-format stream-json --output-format stream-json --verbose
  --include-partial-messages` (a cheap model, in a scratch folder, stdin kept
  open so timers could fire) and cleaned of names, paths and account details;
  record new ones the same way. `shed-git`'s tests make throwaway repositories (and a
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

## The web app

`web/` is the web connector: Angular 22 (standalone components, signals,
zoneless), built against the generated `web/src/api/gen/api.ts`. It needs
Node 24.15+ (or 22.22.3+); `web/README.md` has the details.

```sh
cd web
npm install
npm run mock         # a pretend nexus on 127.0.0.1:8787 (web/mock), sign in as you / shed
npm start            # http://localhost:4200, /api proxied to the mock (proxy.conf.json)
npm run build        # production build into web/dist/shed-web (web_dir can serve it)
npm test             # unit tests (Vitest through ng test)
npm run check:mock   # type-checks the mock against the API types
```

The mock (`web/mock/`, plain Node with `ws`) implements the HTTP routes of
`shed-api/src/http.rs` and the `/api` socket with a made-up workspace: a fake
shell per terminal pane, a scripted AI conversation with a tool call, a
permission request and questions, a project with files, search and Git. Its
switches (`MOCK_FRESH`, `MOCK_VIEW`, `MOCK_LATENCY`, `MOCK_NO_REVIEW`,
`MOCK_NTFY_TOPIC`) and its `notify` command are in
`web/README.md`. Like the screenshots, it uses `you`, `devbox` and other
made-up names only.

### The web app in the nexus

Release builds of `shed-nexus` carry the web app: `crates/shed-nexus/build.rs`
copies `web/`'s sources into cargo's `OUT_DIR`, runs `npm ci` there (again only
when `package-lock.json` changes) and `npm run build`, and lists the files for
`src/web.rs` to embed (`include_bytes!`, with an ETag each). Nothing is
written to the source tree. It needs Node 24.15+ (or 22.22.3+) and npm on
`PATH`, and fails with how to get them otherwise. Debug builds (`cargo build`,
`cargo test`, clippy) skip it, so the workspace builds without Node;
`SHED_NEXUS_WEB=1` builds it in a debug build too, and `SHED_NEXUS_NO_WEB=1`
leaves it out of any build (the nexus then serves only the API and says so in
its log). `web_dir` in `nexus.toml` serves a folder from disk instead, read at
each request (handy with `npm run watch`). Either way the same rules: any path
that is not a file gets `index.html`, hashed bundles (`main-….js`) are cached
for a year, everything else is `no-cache` with an ETag.

On this machine the system Node may be older than the build needs; put a newer
one first on `PATH` for release builds of the nexus:

```sh
PATH=~/.nvm/versions/node/v24.15.0/bin:$PATH CARGO_TARGET_DIR=target/agent cargo build --release -p shed-nexus
SHED_NEXUS_WEB=1 cargo test -p shed-nexus --test account web   # the built-in app, in a debug build
```

## Debugging

- The keeper logs to `~/.local/state/shed/shedd.log`; `SHED_LOG=debug` raises the
  level for both binaries; `shedd --foreground` logs to stderr.
- `shed debug-type 'echo hi\r'` types into the first pane through the keeper.
- `SHED_PERF=1` makes the window process print redraws per second and their cost,
  for terminals, for bench windows (their frames, and their tab bars drawn) and
  for the Shed window (its bench views drawn), with each window's frames timed
  from rendering to painting (without the GPU's part), and how long each replay
  took. `SHED_PERF_FRAMES=60` makes the pegboard's first bench and every bench
  window's tab bar draw again 60 times a second, as hovering in them does, to
  measure a frame without moving the mouse. To compare builds, run each against
  the same world (ten open benches with 5-15 tabs, some printing every second,
  terminals whose title a program changes ten times a second, an AI tab
  working) on a private Xvfb, and read the window process's CPU, and its main
  thread's, from `/proc/<pid>/stat` and `/proc/<pid>/task/<pid>/stat` over ten
  seconds, with the Shed window active and not.
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
project and AI tabs and bookmarks, a made-up Claude Code session in the demo home, usage limits),
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
with its own home and Shed folders, which logs each ssh command line (to see
Reconnect join again) and fails `-O`/`-G` as ssh does with no shared connection (it needs `target/debug/shedd`, which
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
# web_dir = "web/dist/shed-web"   # the web app from npm run build (a debug build has none built in)
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
pairing, relaying and tunnels (`tunnel.rs`: desktop-only, ids swapped,
closing on revocation and offline, a slow side dropped), where a fake link
and connectors speak over `tokio-tungstenite`, and notifications (`notify.rs`, `ntfy_env.rs`), where a
fake push service and ntfy on loopback count what reaches them. TOTP codes come from the account's secret; each time step
is taken once, so a test that needs more than three codes waits for the next
step. Recovery codes stand in where a test only needs to sign in.

### A link against a dev nexus

With the dev nexus above and a dev keeper (the environment from "Running a dev
build safely" in CLAUDE.md, so the pairing and settings land in
`/tmp/shed-dev`), pair and link:

```sh
export SHED_RUNTIME_DIR=/tmp/shed-dev XDG_CONFIG_HOME=/tmp/shed-dev/config \
       XDG_DATA_HOME=/tmp/shed-dev/data XDG_STATE_HOME=/tmp/shed-dev/state
./target/debug/shed nexus join http://localhost:7870   # approve the code (POST /api/v1/pair/approve)
./target/debug/shed nexus status
tail -f /tmp/shed-dev/state/shed/link.log
```

The keeper starts the link within a second of the pairing; `shed nexus leave`
stops it. To run one by hand instead, in the foreground: `SHED_LOG=debug
./target/debug/shed link` (same environment). Without the web app, approve with
`curl` (`GET /api/v1/pair/pending`, then `POST /api/v1/pair/approve` with the
code, a name and a TOTP code) and speak to `/api` with a WebSocket client.

`crates/shed-link/tests/link.rs` runs an in-process keeper (real bash) and a
link against a fake nexus on a real socket (`tests/common/mod.rs`), and
`tests/nexus.rs` the whole way through the real nexus, pairing included.
`tests/desktop.rs` covers the desktop's tunnel: refused without
`desktop_clients`, the keeper's own protocol through a fake nexus, and
`connect_remote`'s tunnel through the real nexus (signing in, the workspace
list, typing and output). `tests/desktop_api.rs` drives
`shed_client::nexus_api` the same way, as the desktop's views of another
workspace use it: a folder, a file read and written (and the change as a
`Project` event), Git status, stage and commit, the logbook and Claude Code's
sessions (`CLAUDE_CONFIG_DIR` in a temp folder). The window's side is tested
with `NexusApi::fake`, which answers calls at once (GPUI tests fail if woken
from another thread): `project_view`'s
`a_remote_project_reads_and_writes_through_its_workspace`.

### Showing a dev workspace in a dev window

Two dev keepers, one shown in the other's window through the dev nexus: the
shown one in its own environment (say `/tmp/shed-dev2`) is paired
(`shed nexus join`) and has `desktop_clients = true` under `[nexus]` in its
`shed.toml`; in the other (`/tmp/shed-dev`), `shed nexus signin
http://localhost:7870` signs the desktop in (a recovery code does for the
code), and the Shed window's switcher (**This PC ▾**) lists the workspace.
On a headless Xvfb display (as `tools/screenshots/take.sh` does) this touches
nothing of your own; clicks do not reach windows there, so
`SHED_DEBUG_WORKSPACE=NAME` (in the window process's environment) switches to
that workspace once the nexus lists it.

### End to end

`tools/e2e.sh` runs the whole way with real processes, apart from your own
Shed: it builds `shed`, `shedd` and `shed-nexus` (release; `E2E_PROFILE=debug`
for a debug build) into a target dir of its own, `target/e2e` (`E2E_TARGET`),
so it never replaces the `target/release` binaries a running Shed may use,
the nexus with the web app built in (`SHED_NEXUS_WEB=1`, for debug builds too),
makes a world in `/tmp/shed-e2e` (`E2E_DIR`; a home with a neutral prompt, a
Git project with a change, a stand-in `claude` playing a recorded turn, a
keeper that stays without windows), starts a nexus on `127.0.0.1:7871`
(`E2E_PORT`) serving the web app, and a keeper of its own. Then
`tools/e2e/e2e.mjs` checks the setup page draws its QR code and key (in
headless Chrome), sets up the account over HTTP (TOTP codes computed from
the secret; each 30 s step is used once, so the run waits for fresh ones),
pairs the keeper with `shed nexus join`, waits for the link, makes a bench
with a terminal, a project and an AI tab through the API, and drives the web
app in a headless Chrome over the DevTools protocol (`tools/e2e/cdp.mjs`, no
packages): signing in, the overview, a reload, typing in the terminal, the
logbook, a diff, a file, a prompt and its answer. Screenshots go to
`$E2E_DIR/shots`; everything else is removed after a run that passed
(`E2E_KEEP=1` keeps the logs), and only the processes it started are stopped
(the keeper with `shed stop` in its own environment). Needs Node 24.15+ (or
22.22+) on `PATH`, Chrome or Chromium (`CHROME=` to choose), `git`, `curl`
and `python3`. It takes about a minute and a half, most of it waiting for
authenticator codes.

## The Android app

`android/` is a Gradle project (Kotlin, Jetpack Compose). No JDK or Android SDK
is needed system-wide: `tools/android-env.sh` exports `JAVA_HOME` and
`ANDROID_HOME` from `~/.cache/shed-dev/android` (or `$SHED_ANDROID_DIR`), and
its header lists the one-time setup (a Temurin JDK 21, the Android command-line
tools, `sdkmanager --licenses`, `platforms;android-36`, `build-tools;36.0.0`,
`platform-tools`; no emulator images).

```sh
. tools/android-env.sh
cd android
./gradlew assembleDebug                      # app/build/outputs/apk/debug/app-debug.apk
./gradlew testDebugUnitTest lintDebug        # JVM tests; lint must report no errors
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

`app/src/main/java/dev/shed/api/Api.kt` is generated (`cargo run -p
shed-codegen`); the app reads it with the `Json` setup from that file's header
(`net/ShedJson.kt`). The tests run on the JVM: JSON round-trips of frames, the
WebSocket client against a fake nexus (OkHttp's MockWebServer), terminal
re-wrapping, the Markdown reader, and the AI tab's scrolling, message jumps and
settings sheet. Screenshot tests (`app/src/test/.../screens/`, Robolectric and
Roborazzi, no emulator) draw screens at a phone's size, dark and light, as
pictures to look at (nothing is compared): into `app/build/screenshots`, or
`./gradlew testDebugUnitTest --tests 'dev.shed.screens.*'
-Pshed.screenshots=/tmp/shed-android`. Robolectric fetches its Android jar
from Maven Central on the first run. To try the app against a nexus on this
computer, run the nexus as above and sign in at `http://10.0.2.2:7870` from an
emulator, or `adb reverse tcp:7870 tcp:7870` and `http://localhost:7870` from a
phone (plain `http` is accepted only for those). Debug builds only: release
signing is not configured. More in [android/README.md](../android/README.md).

## Packages

- `packaging/deb/build.sh` builds the Debian package into `target/debian/`.
- `packaging/macos/bundle.sh` builds `Shed.app` (and a zip of it) into
  `target/macos/`, on a Mac.
- `packaging/arch/PKGBUILD` builds the latest commit from GitHub (`makepkg -si`).
- `packaging/linux/` holds the desktop entry, the optional systemd user unit and
  the copyright notice they share.
- `packaging/nexus/` holds `install.sh` (sets up or updates a nexus on Debian
  and Ubuntu: user, settings, systemd, nginx, certbot; `DRY_RUN=1` prints what
  it would do), a sample `nexus.toml`, the nexus's systemd unit and the nginx
  site; [Deploying a nexus](nexus-deploy.md) uses them.
