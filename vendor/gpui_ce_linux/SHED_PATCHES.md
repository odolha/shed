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
