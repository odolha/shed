# Shed patches to alacritty_terminal 0.26.0

Vendored from crates.io (Apache-2.0, see LICENSE-APACHE) and wired in through
`[patch.crates-io]` in the workspace `Cargo.toml`. The `tests/` directory (46 MB
of reference fixtures) is removed, along with its `[[test]]` target.

The only code change adds read-only accessors to `Term` in `src/term/mod.rs`,
marked `// Shed patch`, so the keeper can serialize a screen for replay on
reattach:

- `inactive_grid()`: the primary screen while the alternate screen is active
- `scroll_region()`: the DECSTBM region
- `active_charset()`: which of G0..G3 maps to ASCII
- `title()`: the current title
- `is_tab_stop(column)`

When upgrading, copy the new release over this directory and reapply these
accessors. Upstreaming them would remove the need for the vendored copy.
