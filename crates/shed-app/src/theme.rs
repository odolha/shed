//! Fonts and colours. `shed.toml` and theme files arrive in phase 3.

use gpui::{Font, Hsla, font};
use shed_emu::Rgb;
use shed_model::config::{FontConfig, Theme};

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
        FontConfig::default().into()
    }
}

impl From<FontConfig> for FontSettings {
    fn from(c: FontConfig) -> Self {
        Self { family: c.family, size: c.size.clamp(6.0, 72.0), line_height: c.line_height.clamp(0.8, 3.0) }
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
    /// Shed's own highlight, where no bench colour applies: coral, like the
    /// app icon, and clearly apart from `danger`'s red.
    pub accent: Rgb,
    /// For what stops or removes things.
    pub danger: Rgb,
}

impl Default for UiColors {
    fn default() -> Self {
        Self::from_theme(&Theme::load("shed-dark").expect("built-in theme"))
    }
}

impl UiColors {
    /// The theme's chrome colours, falling back to its terminal colours.
    pub fn from_theme(t: &Theme) -> Self {
        let ui = t.ui.clone().unwrap_or_default();
        let pick = |c: &Option<String>, fallback: &str| c.as_deref().and_then(Rgb::parse).unwrap_or(Theme::rgb(fallback));
        Self {
            background: pick(&ui.background, &t.background),
            tab_active: pick(&ui.tab_active, &t.background),
            tab_hover: pick(&ui.tab_hover, &t.ansi[0]),
            text: pick(&ui.text, &t.foreground),
            text_dim: pick(&ui.text_dim, &t.ansi[8]),
            divider: pick(&ui.divider, &t.ansi[8]),
            accent: pick(&ui.accent, &t.ansi[4]),
            danger: pick(&ui.danger, &t.ansi[1]),
        }
    }

    /// `c` for text and icons on this chrome. Bench and tag colours are bright,
    /// which reads on dark themes; on light ones `c` is darkened until it does.
    pub fn ink(&self, c: Rgb) -> Rgb {
        let background = luminance(self.background);
        if background < 0.5 {
            return c;
        }
        let mut c = c;
        while (background + 0.05) / (luminance(c) + 0.05) < 4.0 && c != Rgb::default() {
            let darker = |v: u8| (v as f32 * 0.9) as u8;
            c = Rgb { r: darker(c.r), g: darker(c.g), b: darker(c.b) };
        }
        c
    }
}

/// Relative luminance, as WCAG defines it.
fn luminance(c: Rgb) -> f32 {
    let linear = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * linear(c.r) + 0.7152 * linear(c.g) + 0.0722 * linear(c.b)
}

/// The chrome colours of the theme in effect.
pub fn current_ui(cx: &gpui::App) -> UiColors {
    cx.try_global::<crate::prompt::CurrentUi>().map(|u| u.0.clone()).unwrap_or_default()
}

/// Whether the desktop prefers dark.
pub fn is_dark(appearance: gpui::WindowAppearance) -> bool {
    matches!(appearance, gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark)
}

/// `c` at `alpha` opacity.
pub fn hsla_alpha(c: Rgb, alpha: f32) -> Hsla {
    let mut h = hsla(c);
    h.alpha = alpha;
    h
}
