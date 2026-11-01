//! Fonts and colours. `shed.toml` and theme files arrive in phase 3.

use gpui::{Font, Hsla, font};
use shed_emu::Rgb;

pub fn hsla(c: Rgb) -> Hsla {
    gpui::rgb_to_hsla(gpui::rgb(c.to_hex()))
}

#[derive(Clone, Debug, PartialEq)]
pub struct FontSettings {
    pub family: String,
    pub size: f32,
    /// Line height as a multiple of the font size.
    pub line_height: f32,
}

impl Default for FontSettings {
    fn default() -> Self {
        Self {
            family: std::env::var("SHED_FONT").unwrap_or_else(|_| "DejaVu Sans Mono".into()),
            size: std::env::var("SHED_FONT_SIZE").ok().and_then(|s| s.parse().ok()).unwrap_or(14.0),
            line_height: 1.25,
        }
    }
}

impl FontSettings {
    pub fn font(&self) -> Font {
        font(self.family.clone())
    }
}
