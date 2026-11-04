# Shed patches to gpui_ce_render 0.3.0

Vendored from crates.io (Apache-2.0, see LICENSE-APACHE) and wired in through
`[patch.crates-io]` in the workspace `Cargo.toml`.

One change, in `src/shaders/quads.rs`, marked `// Shed patch`:

- **A quad is rasterized only inside its content mask.** The vertex stage drew
  the whole quad and the fragment stage discarded what the mask clips. The
  fragment stage works from pixel positions and the quad itself, so drawing
  the quad's bounds clipped to its mask gives the same pixels without running
  the shader where they are thrown away. This is what lets `gpui_ce_wgpu` draw a
  border-only quad as four edge strips.
