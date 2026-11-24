//! `shed tui`: Shed in a terminal. A client of the local keeper like the
//! windows: benches and their tabs, terminal panes drawn from `shed-emu`
//! (as tmux draws them, with splits), AI tabs, project tabs with Git, and
//! the logbook. Leaving it (`C-b d`, or a dropped ssh) leaves everything
//! running in the keeper.
//!
//! `App` holds the state and draws into a ratatui frame; `run` connects it
//! to the real terminal and the keeper.

mod ai;
mod app;
mod benches;
mod bookmarks;
mod git;
mod keys;
mod layout;
mod logbook;
mod md;
mod overlay;
mod project;
mod review;
mod run;
mod term;
mod text;
mod theme;

pub use app::{App, Event, Options, Screen};
pub use layout::{Divider, cells};
pub use run::run;

/// Every key, by where it works: the help (`C-b ?`) and the user guide.
pub const KEYS: &[(&str, &[(&str, &str)])] = &[
    (
        "Anywhere (after the prefix, Ctrl+B)",
        &[
            ("C-b d", "leave (detach): everything keeps running"),
            ("C-b ?", "these keys"),
            ("C-b s", "the benches"),
            ("C-b r", "search history (the logbook)"),
            ("C-b b", "the bench's bookmarks: go to, rename, remove"),
            ("C-b C-b", "Ctrl+B for the program in the pane"),
        ],
    ),
    (
        "Tabs",
        &[
            ("C-b c", "new terminal tab here"),
            ("C-b t", "new tab: terminal, project, AI, remote, command"),
            ("C-b n / p", "next / previous tab"),
            ("C-b 1…9", "tab by number"),
            ("C-b ,", "rename the tab"),
            ("C-b < / >", "move the tab left / right"),
            ("C-b &", "close the tab (asks)"),
            ("C-b m", "everything for this tab and bench"),
            ("C-b $", "rename the bench"),
            ("C-b e", "edit a file in $EDITOR (a new tab)"),
        ],
    ),
    (
        "Panes",
        &[
            ("C-b %", "split right"),
            ("C-b \"", "split down"),
            ("C-b arrows", "focus the pane that way"),
            ("C-b o", "next pane"),
            ("C-b x", "close the pane (asks)"),
            ("C-b z", "zoom the pane (again: back)"),
            ("C-b [", "scroll back: ↑↓ PgUp PgDn g G, [ ] prompts, / search, n N, q leaves"),
            ("mouse", "click focuses; programs that ask get the mouse (Shift keeps it for the terminal)"),
        ],
    ),
    (
        "Benches screen",
        &[
            ("Enter", "open (a closed bench loads)"),
            ("L", "load and run its startup"),
            ("n", "new bench"),
            ("r / c", "rename / colour"),
            ("x", "close (saved, asks)"),
            ("D", "delete (asks)"),
            ("/", "filter"),
            ("Esc", "back to the bench"),
        ],
    ),
    (
        "AI tabs",
        &[
            ("Enter", "send (queued while Claude works)"),
            ("Alt+Enter", "new line in the prompt"),
            ("Esc", "stop the turn"),
            ("Tab", "complete a /command; switch between answering and typing"),
            ("↑↓ 1–9 Enter", "answer a permission or question"),
            ("PgUp / PgDn", "scroll the conversation"),
            ("Alt+S / Alt+N", "sessions / new session"),
            ("Alt+M / O / E", "model / mode / effort"),
            ("Alt+D / Alt+P", "keep the prompt as a draft / drafts"),
            ("Alt+Q", "queued messages"),
            ("Alt+I / Alt+X", "attach a picture by path / remove them"),
            ("Alt+R", "review changes: v viewed, e explain, t turn/session"),
            ("Alt+T", "unfold tools and thinking (or click one)"),
            ("Alt+B", "bookmark the last message in view"),
        ],
    ),
    (
        "Project tabs",
        &[
            ("↑↓ Enter ←", "the file tree"),
            ("Tab", "between the list and the file"),
            ("o / Ctrl+P", "open by name"),
            ("/ / Ctrl+F", "search in files (Alt+R regex, Alt+C case, Alt+W word)"),
            ("e", "edit in $EDITOR"),
            ("m", "bookmark the file or folder chosen, or the file shown at its top line"),
            ("g", "Git: s/u/d file, S stage all, c commit, C amend, l log, b branches, f fetch, F pull, p push, P force-push"),
            ("in a diff", "[ ] hunks, s stage, u unstage, d discard"),
        ],
    ),
];
