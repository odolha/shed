# Shed patches to gpui_ce_wgpu 0.3.0

Vendored from crates.io (Apache-2.0, see LICENSE-APACHE) and wired in through
`[patch.crates-io]` in the workspace `Cargo.toml`.

One change, in `src/wgpu_renderer/frame.rs`, marked `// Shed patch`:

- **Window-sized backgrounds become the clear colour.** When the first thing a
  frame draws is an opaque, solid, square, borderless quad covering the whole
  viewport, the main pass clears to its colour and skips drawing it. A clear is
  close to free; that quad ran the full quad shader and blended every pixel of
  the window, which on an integrated GPU under Cinnamon's fractional scaling
  cost about 4 ms per frame (0.4 ms for clear and present alone). The pixels are
  identical: the colour goes through the same HSLA-to-RGBA conversion as the
  shader, and the surface formats are non-sRGB.
