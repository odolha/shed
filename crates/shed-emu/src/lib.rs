//! The terminal emulation core, behind the [`Emulator`] trait.
//!
//! Everything inside the grid (escape sequences, scrollback, modes) is handled
//! by a mature core. Shed only talks to it through this trait, so the core can
//! be swapped (see the `libghostty-vt` open decision) without touching the
//! keeper or the windows.

mod alacritty;
pub mod binary;
pub mod keys;
pub mod links;
pub mod marks;
pub mod mouse;
mod palette;
mod snapshot;

pub use alacritty::AlacrittyEmulator;
pub use palette::Palette;
pub use shed_model::Rgb;
pub use snapshot::{CellFlags, CellView, CursorShape, CursorView, Snapshot};

/// Grid size in cells, plus the cell size in pixels (for apps that ask).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl GridSize {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self { cols: cols.max(2), rows: rows.max(1), cell_width: 0, cell_height: 0 }
    }
}

/// Terminal modes that change how input must be encoded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    pub app_cursor: bool,
    pub app_keypad: bool,
    pub bracketed_paste: bool,
    pub alt_screen: bool,
    pub alternate_scroll: bool,
    pub focus_reporting: bool,
    pub mouse: MouseMode,
    pub sgr_mouse: bool,
    pub utf8_mouse: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseMode {
    #[default]
    Off,
    /// Presses and releases.
    Click,
    /// Plus motion while a button is held.
    Drag,
    /// Plus all motion.
    Motion,
}

/// How a selection grows from where the mouse went down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionKind {
    /// Cell by cell.
    Simple,
    /// Whole words (double-click).
    Word,
    /// Whole lines (triple-click).
    Line,
    /// A rectangle.
    Block,
}

/// A cell in the visible grid, 0-based, with the half of it the pointer is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridPoint {
    pub col: usize,
    pub row: usize,
    pub right_half: bool,
}

/// Something the terminal asked for that the emulator cannot do by itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EmuEvent {
    /// Set the title (`None` resets it).
    Title(Option<String>),
    Bell,
    /// OSC 52: store text on the clipboard.
    ClipboardStore { primary: bool, text: String },
}

/// A terminal emulator: bytes in, a grid out.
pub trait Emulator: Send {
    /// Feeds PTY output to the emulator.
    fn advance(&mut self, bytes: &[u8]);

    /// Bytes the emulator must write back to the PTY: replies to device,
    /// cursor and colour queries. Only the keeper sends these to the PTY;
    /// windows discard them, so every query is answered exactly once.
    fn take_replies(&mut self) -> Vec<u8>;

    fn take_events(&mut self) -> Vec<EmuEvent>;

    fn resize(&mut self, size: GridSize);

    /// The colours reported to apps that query them.
    fn set_palette(&mut self, palette: Palette);

    fn size(&self) -> GridSize;

    fn modes(&self) -> Modes;

    /// Scrolls the view into scrollback by `lines` (positive is up, into history).
    fn scroll_display(&mut self, lines: i32);

    /// Returns the view to the bottom of the scrollback.
    fn scroll_to_bottom(&mut self);

    /// How far the view is scrolled into scrollback, in lines.
    fn display_offset(&self) -> usize;

    /// Ends a synchronized update that has run past its deadline.
    /// Returns when it next needs to be called, if a sync update is pending.
    fn tick(&mut self) -> Option<std::time::Instant>;

    /// Fills `out` with the visible grid, colours resolved through `palette`.
    fn snapshot(&self, palette: &Palette, out: &mut Snapshot);

    /// Bytes that bring a fresh emulator of the same size to this state:
    /// scrollback, both screens, cursor, pen, modes, colours and title.
    /// A synchronized update in progress is applied first.
    fn replay(&mut self) -> Vec<u8>;

    fn start_selection(&mut self, kind: SelectionKind, at: GridPoint);

    fn update_selection(&mut self, to: GridPoint);

    fn clear_selection(&mut self);

    /// The selected text, if anything is selected.
    fn selection_text(&self) -> Option<String>;

    /// The visible row's text and the column each character starts at.
    fn row_text(&self, row: usize) -> (String, Vec<usize>);

    /// The OSC 8 hyperlink on a visible cell.
    fn hyperlink_at(&self, col: usize, row: usize) -> Option<String>;

    /// The main screen's scrollback and screen as plain text: wrapped lines
    /// joined, trailing blanks trimmed, at most the last `max_lines` lines.
    /// (While a full-screen app runs, this is the screen beneath it.)
    fn scrollback_text(&self, max_lines: usize) -> String;

    /// The visible grid as plain text, one line per row, trailing blanks trimmed.
    fn dump_text(&self) -> String;

    /// Highlights `query` in the scrollback and screen (plain text, ignoring
    /// case unless it has capitals) and goes to the newest match; `None` or
    /// an empty query ends the search. Returns whether anything matches.
    fn set_search(&mut self, query: Option<&str>) -> bool;

    /// Goes to the next match, older or newer, wrapping around, and scrolls
    /// it into view. Returns whether anything matches.
    fn search_step(&mut self, older: bool) -> bool;
}
