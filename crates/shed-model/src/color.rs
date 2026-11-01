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

    /// A named colour (`blue`, `coral`, …) or `#rrggbb`.
    pub fn parse(s: &str) -> Option<BenchColor> {
        let s = s.trim().to_ascii_lowercase();
        Self::NAMED.into_iter().find(|c| c.name() == Some(s.as_str())).or_else(|| Rgb::parse(&s).map(BenchColor::Custom))
    }

    /// The colour as written in saved benches: its name, or `#rrggbb`.
    pub fn to_text(self) -> String {
        match self {
            BenchColor::Custom(c) => format!("#{:06X}", c.to_hex()),
            named => named.name().unwrap_or_default().to_string(),
        }
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

/// Named colours for tags, as written in `shed.toml` and `shed tag add`.
pub const TAG_COLORS: [(&str, Rgb); 8] = [
    ("red", Rgb::hex(0xF0566A)),
    ("orange", Rgb::hex(0xFF9A5A)),
    ("yellow", Rgb::hex(0xF2C14E)),
    ("green", Rgb::hex(0x5FD38D)),
    ("blue", Rgb::hex(0x5C9DFF)),
    ("violet", Rgb::hex(0xB48BFF)),
    ("teal", Rgb::hex(0x3CCFC0)),
    ("grey", Rgb::hex(0x8A93A3)),
];

/// A tag colour by name (`red`, `grey`, …) or `#rrggbb`.
pub fn tag_color(name: &str) -> Option<Rgb> {
    let name = name.trim().to_ascii_lowercase();
    let name = if name == "gray" { "grey".to_string() } else { name };
    TAG_COLORS.iter().find(|(n, _)| *n == name).map(|(_, c)| *c).or_else(|| Rgb::parse(&name))
}

/// A stable colour for a tag that names none, so `prod` always looks the same.
pub fn default_tag_color(label: &str) -> Rgb {
    let hash = label.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    TAG_COLORS[hash as usize % TAG_COLORS.len()].1
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
    fn tag_colours() {
        assert_eq!(tag_color("Red"), Some(Rgb::hex(0xF0566A)));
        assert_eq!(tag_color("gray"), tag_color("grey"));
        assert_eq!(tag_color("#010203"), Some(Rgb::hex(0x010203)));
        assert_eq!(tag_color("nope"), None);
        assert_eq!(default_tag_color("prod"), default_tag_color("prod"));
    }

    #[test]
    fn parse_hex() {
        assert_eq!(Rgb::parse("#5CB2FF"), Some(Rgb::hex(0x5CB2FF)));
        assert_eq!(Rgb::parse("5cb2ff").unwrap().to_hex(), 0x5CB2FF);
        assert_eq!(Rgb::parse("#5CB2F"), None);
    }
}
