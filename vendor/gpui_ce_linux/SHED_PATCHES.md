# Shed patches to gpui_ce_linux 0.3.0

Vendored from crates.io (Apache-2.0, see LICENSE-APACHE) and wired in through
`[patch.crates-io]` in the workspace `Cargo.toml`.

Changes marked `// Shed patch`, in `src/linux/x11/client.rs` and
`src/linux/x11/window.rs`:

- **Key releases are not dropped.** Each batch of X11 events holds back the
  last key release, to tell auto-repeat (a release followed at once by a press
  of the same key) from a real one. A second release in the same batch
  overwrote the first, so letting go of a chord (A, then Shift) lost the
  release of A. Apps that ask for key releases through the kitty keyboard
  protocol would then see the key as still held. The held-back release is now
  passed on when another one replaces it.

- **Windows open where asked.** The requested position goes into
  `WM_NORMAL_HINTS` as user-specified, with static gravity (the position is the
  window's content, not its frame); without it, window managers place new
  windows themselves. Upstream also created windows 2px right of the request,
  which made a window reopened at its last place creep; that is gone.
- **The position reported is where the window is.** A `ConfigureNotify` from
  the server gives the position inside the window manager's frame, and upstream
  kept the origin only for pure moves. The origin is now asked for with
  `TranslateCoordinates` to the root window on every configure.
- **Opening maximized or full screen works.** GPUI asks for a window's starting
  state before mapping it, and window managers ignore `_NET_WM_STATE` requests
  for unmapped windows (EWMH has the client set the property instead). Until the
  window is mapped, `zoom` and `toggle_fullscreen` append to the property.
- **An idle window stops ticking.** Each visible window had a timer at the
  display's refresh rate (165 times a second on a 165 Hz panel) that asked GPUI
  for a frame whether or not anything had changed, so an idle Shed woke a few
  hundred times a second. The timer now parks after a second in which GPUI
  asked for nothing (`RefreshState::Parked`). The window implements
  `PlatformWindow::frame_waker`, which GPUI calls when a window becomes dirty
  or wants another frame; it records the time and, if the timer is parked,
  pings a calloop source that starts it again (the ping, because the waker can
  run while the client is borrowed). GPU recovery wakes it too. Wayland already
  worked this way.

Clipboard (`src/linux.rs`, `src/linux/x11/clipboard.rs`, `src/linux/x11/client.rs`,
`src/linux/wayland/client.rs`, `src/linux/wayland/clipboard.rs`):

- **Copies can carry HTML.** A clipboard string whose metadata starts with
  `shed-html:` is offered as `text/html` (the rest of the metadata) beside the
  text, on X11 and Wayland, so formatted text pastes formatted.
- **X11 serves the plain-text names it lists.** `TARGETS` listed
  `text/plain;charset=utf-8` and friends, but a request for them was refused;
  they are now answered from the `UTF8_STRING` data.
- **Files copied in a file manager paste as files.** When the clipboard
  offers `text/uri-list` (Files, Dolphin, Thunar…), its local `file://` URIs
  are read as paths and added to the clipboard item as
  `ClipboardEntry::ExternalPaths`, after its text or picture (so whatever
  reads the text sees what it saw before), on X11 (the `CLIPBOARD` selection
  only) and Wayland; as macOS already does. `file_paths_from_uri_list` and
  `with_files` / `plus_files` in `src/linux.rs`.
