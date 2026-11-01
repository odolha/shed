use serde::{Deserialize, Serialize};

/// A plain 8-bit RGB colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn hex(hex: u32) -> Self {
        Self { r: (hex >> 16) as u8, g: (hex >> 8) as u8, b: hex as u8 }
    }

    pub fn to_hex(self) -> u32 {
        (self.r as u32) << 16 | (self.g as u32) << 8 | self.b as u32
    }

    /// Parses `#rrggbb` or `rrggbb`.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.strip_prefix('#').unwrap_or(s);
        if s.len() != 6 {
            return None;
        }
        u32::from_str_radix(s, 16).ok().map(Self::hex)
    }
}

/// A bench's colour: one of the eight named options, or any custom colour.
///
/// Named colours carry the two-tone gradient used by the bench icons; the
/// `top` stop is the bright one used for UI accents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BenchColor {
    Blue,
    Violet,
    Pink,
    Coral,
    Amber,
    Green,
    Teal,
    Slate,
    Custom(Rgb),
}

impl BenchColor {
    /// The eight named colours, in the order new benches take them.
    pub const NAMED: [BenchColor; 8] = [
        BenchColor::Blue,
        BenchColor::Violet,
        BenchColor::Pink,
        BenchColor::Coral,
        BenchColor::Amber,
        BenchColor::Green,
        BenchColor::Teal,
        BenchColor::Slate,
    ];

    /// The next named colour not already in `used`, cycling when all are taken.
    pub fn next_unused<'a>(used: impl IntoIterator<Item = &'a BenchColor>) -> BenchColor {
        let used: Vec<&BenchColor> = used.into_iter().collect();
        Self::NAMED
            .into_iter()
            .find(|c| !used.contains(&c))
            .unwrap_or(Self::NAMED[used.len() % Self::NAMED.len()])
    }

    /// Gradient stops `(top, bottom)`, matching `assets/icons/bench/shed-bench-<colour>.svg`.
    pub fn gradient(self) -> (Rgb, Rgb) {
        let (top, bottom) = match self {
            BenchColor::Blue => (0x5CB2FF, 0x2459E8),
            BenchColor::Violet => (0xB48BFF, 0x7341E0),
            BenchColor::Pink => (0xFF8AC8, 0xE0408F),
            BenchColor::Coral => (0xFF9A5A, 0xF0475E),
            BenchColor::Amber => (0xFFD15C, 0xF29A1F),
            BenchColor::Green => (0x7BE38E, 0x1F9D57),
            BenchColor::Teal => (0x4BE0C4, 0x0B9A9A),
            BenchColor::Slate => (0xB3BAC7, 0x5F6878),
            BenchColor::Custom(c) => return (c, c),
        };
        (Rgb::hex(top), Rgb::hex(bottom))
    }

    /// The accent used for the tab-bar stripe and focused pane border.
    pub fn accent(self) -> Rgb {
        self.gradient().0
    }

    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            BenchColor::Blue => "blue",
            BenchColor::Violet => "violet",
            BenchColor::Pink => "pink",
            BenchColor::Coral => "coral",
            BenchColor::Amber => "amber",
            BenchColor::Green => "green",
            BenchColor::Teal => "teal",
            BenchColor::Slate => "slate",
            BenchColor::Custom(_) => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_unused_takes_first_free() {
        assert_eq!(BenchColor::next_unused(&[]), BenchColor::Blue);
        assert_eq!(BenchColor::next_unused(&[BenchColor::Blue, BenchColor::Pink]), BenchColor::Violet);
        let all = BenchColor::NAMED;
        assert_eq!(BenchColor::next_unused(&all), BenchColor::Blue);
    }

    #[test]
    fn parse_hex() {
        assert_eq!(Rgb::parse("#5CB2FF"), Some(Rgb::hex(0x5CB2FF)));
        assert_eq!(Rgb::parse("5cb2ff").unwrap().to_hex(), 0x5CB2FF);
        assert_eq!(Rgb::parse("#5CB2F"), None);
    }
}
