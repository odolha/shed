use shed_model::Rgb;

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
        let h = Rgb::hex;
        Self {
            ansi: [
                h(0x20242D),
                h(0xF0566A),
                h(0x5FD38D),
                h(0xF2C14E),
                h(0x5C9DFF),
                h(0xB48BFF),
                h(0x3CCFC0),
                h(0xC9CED8),
                h(0x5F6878),
                h(0xFF7A8A),
                h(0x8CF0BE),
                h(0xFFD97A),
                h(0x8CC4FF),
                h(0xD0B6FF),
                h(0x7FEBDD),
                h(0xF4F6FA),
            ],
            foreground: h(0xDDE1E8),
            background: h(0x14171D),
            cursor: h(0xFF9A5A),
            cursor_text: h(0x14171D),
            selection: h(0x34405A),
        }
    }
}

impl Palette {
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
