use shed_model::Rgb;
use shed_model::config::Theme;

/// The colours a theme provides to the emulator.
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    /// The 16 ANSI colours: normal 0..8, bright 8..16.
    pub ansi: [Rgb; 16],
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    /// Text colour under a block cursor.
    pub cursor_text: Rgb,
    /// Background of selected text.
    pub selection: Rgb,
}

impl Default for Palette {
    /// Shed's default dark theme, matched to the slate walls of the icon.
    fn default() -> Self {
        Self::from_theme(&Theme::load("shed-dark").expect("built-in theme"))
    }
}

impl Palette {
    pub fn from_theme(theme: &Theme) -> Self {
        let mut ansi = [Rgb::default(); 16];
        for (slot, c) in ansi.iter_mut().zip(&theme.ansi) {
            *slot = Theme::rgb(c);
        }
        Self {
            ansi,
            foreground: Theme::rgb(&theme.foreground),
            background: Theme::rgb(&theme.background),
            cursor: Theme::rgb(&theme.cursor),
            cursor_text: Theme::rgb(&theme.cursor_text),
            selection: Theme::rgb(&theme.selection),
        }
    }

    /// One of the 256 indexed colours: ANSI, then the 6×6×6 cube, then the grey ramp.
    pub fn indexed(&self, index: u8) -> Rgb {
        match index {
            0..16 => self.ansi[index as usize],
            16..232 => {
                let i = index - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                Rgb { r: level(i / 36), g: level((i / 6) % 6), b: level(i % 6) }
            }
            232..=255 => {
                let v = 8 + (index - 232) * 10;
                Rgb { r: v, g: v, b: v }
            }
        }
    }
}

/// Scales a colour toward black, for dim text.
pub(crate) fn dim(c: Rgb) -> Rgb {
    let f = |v: u8| (v as f32 * 0.66) as u8;
    Rgb { r: f(c.r), g: f(c.g), b: f(c.b) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_cube_and_ramp() {
        let p = Palette::default();
        assert_eq!(p.indexed(16), Rgb::hex(0x000000));
        assert_eq!(p.indexed(196), Rgb::hex(0xFF0000));
        assert_eq!(p.indexed(231), Rgb::hex(0xFFFFFF));
        assert_eq!(p.indexed(232), Rgb::hex(0x080808));
        assert_eq!(p.indexed(255), Rgb::hex(0xEEEEEE));
    }
}
