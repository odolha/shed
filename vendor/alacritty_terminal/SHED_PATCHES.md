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

One more, in `src/term/cell.rs`: a cell flag, `Flags::SHED_PROMPT`, which
Shed sets on the first cell of the line where a command was typed (from the
shell's OSC 133;B mark), to jump from command to command in the scrollback.
It moves with the line into history and is cleared when the line is erased.

When upgrading, copy the new release over this directory and reapply these
accessors. Upstreaming them would remove the need for the vendored copy.
