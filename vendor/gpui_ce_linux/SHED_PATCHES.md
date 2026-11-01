# Shed patches to gpui_ce_linux 0.3.0

Vendored from crates.io (Apache-2.0, see LICENSE-APACHE) and wired in through
`[patch.crates-io]` in the workspace `Cargo.toml`.

One change, in `src/linux/x11/client.rs`, marked `// Shed patch`:

- **Key releases are not dropped.** Each batch of X11 events holds back the
  last key release, to tell auto-repeat (a release followed at once by a press
  of the same key) from a real one. A second release in the same batch
  overwrote the first, so letting go of a chord (A, then Shift) lost the
  release of A. Apps that ask for key releases through the kitty keyboard
  protocol would then see the key as still held. The held-back release is now
  passed on when another one replaces it.
