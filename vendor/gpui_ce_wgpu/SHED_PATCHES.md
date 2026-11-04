# Shed patches to gpui_ce_wgpu 0.3.0

Vendored from crates.io (Apache-2.0, see LICENSE-APACHE) and wired in through
`[patch.crates-io]` in the workspace `Cargo.toml`.

Two changes, marked `// Shed patch`:

- **Window-sized backgrounds become the clear colour.** When the first thing a
  frame draws is an opaque, solid, square, borderless quad covering the whole
  viewport, the main pass clears to its colour and skips drawing it. A clear is
  close to free; that quad ran the full quad shader and blended every pixel of
  the window, which on an integrated GPU under Cinnamon's fractional scaling
  cost about 4 ms per frame (0.4 ms for clear and present alone). The pixels are
  identical: the colour goes through the same HSLA-to-RGBA conversion as the
  shader, and the surface formats are non-sRGB. (`src/wgpu_renderer/frame.rs`)
- **Border-only quads cost their border, not their area.** A quad with a
  transparent fill and a border (a card's outline, a focused pane's frame) ran
  the quad shader over every pixel inside it. Large enough ones, without a faded
  clip, are drawn as four copies, each clipped to one edge strip wide enough for
  its border, corner radii and antialiasing; with the `gpui_ce_render` patch only
  those strips are rasterized. On the integrated GPU this took the Shed window
  from about 7.6 ms of GPU time per frame to 1.6 ms. The frame's instance storage
  reserves room for the copies. (`src/wgpu_renderer/drawing.rs`, `frame.rs`)
