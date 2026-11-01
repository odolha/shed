use shed_model::Rgb;

/// The visible grid with every colour resolved, ready to paint.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub cols: usize,
    pub rows: usize,
    /// `rows * cols` cells, row by row.
    pub cells: Vec<CellView>,
    pub cursor: CursorView,
    pub display_offset: usize,
    /// The default background, for painting the area around the grid.
    pub background: Rgb,
}

impl Snapshot {
    pub fn row(&self, row: usize) -> &[CellView] {
        &self.cells[row * self.cols..(row + 1) * self.cols]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CellView {
    pub ch: char,
    /// Combining characters drawn on top of `ch`.
    pub zerowidth: Option<Box<str>>,
    pub fg: Rgb,
    pub bg: Rgb,
    pub flags: CellFlags,
    /// `None` uses `fg`.
    pub underline_color: Option<Rgb>,
}

impl Default for CellView {
    fn default() -> Self {
        Self {
            ch: ' ',
            zerowidth: None,
            fg: Rgb::hex(0xFFFFFF),
            bg: Rgb::hex(0),
            flags: CellFlags::default(),
            underline_color: None,
        }
    }
}

/// Text attributes, after inverse and hidden have been folded into the colours.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CellFlags {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub double_underline: bool,
    pub undercurl: bool,
    pub strikeout: bool,
    /// The first half of a double-width character.
    pub wide: bool,
    /// The second half of a double-width character; draws nothing.
    pub spacer: bool,
    /// The background differs from the terminal default.
    pub has_bg: bool,
    pub selected: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Beam,
    HollowBlock,
    Hidden,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CursorView {
    pub row: usize,
    pub col: usize,
    pub shape: CursorShape,
    /// The cursor covers a double-width character.
    pub wide: bool,
}
