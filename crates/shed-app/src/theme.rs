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

/// Colours of Shed's own chrome: the tab bar, borders and overlays.
#[derive(Clone, Debug, PartialEq)]
pub struct UiColors {
    /// Behind the tab bar.
    pub background: Rgb,
    pub tab_active: Rgb,
    pub tab_hover: Rgb,
    pub text: Rgb,
    pub text_dim: Rgb,
    /// Lines between panes.
    pub divider: Rgb,
    /// Used where no bench colour applies.
    pub accent_fallback: Rgb,
}

impl Default for UiColors {
    fn default() -> Self {
        Self {
            background: Rgb::hex(0x0E1116),
            tab_active: Rgb::hex(0x14171D),
            tab_hover: Rgb::hex(0x1A1D24),
            text: Rgb::hex(0xDDE1E8),
            text_dim: Rgb::hex(0x8A93A3),
            divider: Rgb::hex(0x2A2F3A),
            accent_fallback: Rgb::hex(0xFF9A5A),
        }
    }
}

/// `c` at `alpha` opacity.
pub fn hsla_alpha(c: Rgb, alpha: f32) -> Hsla {
    let mut h = hsla(c);
    h.alpha = alpha;
    h
}
