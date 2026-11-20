//! Shed's theme in terminal colours: the chrome colours of the theme's `[ui]`
//! section (falling back to its terminal colours, as the windows do) and
//! the terminal palette, as 24-bit colours. With `NO_COLOR` set, nothing is
//! coloured; bold, reverse and underline still mark what matters.

use ratatui::style::{Color, Modifier, Style};
use shed_emu::Palette;
use shed_model::Rgb;
use shed_model::config::{Config, Theme};

#[derive(Clone, Debug)]
pub struct Colors {
    pub background: Rgb,
    pub tab_active: Rgb,
    pub tab_hover: Rgb,
    pub text: Rgb,
    pub dim: Rgb,
    pub divider: Rgb,
    /// Shed's own highlight (coral in its themes), apart from `danger`.
    pub accent: Rgb,
    pub danger: Rgb,
    /// The 16 terminal colours, for code and diffs.
    pub ansi: [Rgb; 16],
    /// No colours at all (`NO_COLOR`).
    pub no_color: bool,
}

impl Colors {
    pub fn from_theme(t: &Theme, no_color: bool) -> Self {
        let ui = t.ui.clone().unwrap_or_default();
        let pick = |c: &Option<String>, fallback: &str| c.as_deref().and_then(Rgb::parse).unwrap_or(Theme::rgb(fallback));
        let palette = Palette::from_theme(t);
        Self {
            background: pick(&ui.background, &t.background),
            tab_active: pick(&ui.tab_active, &t.background),
            tab_hover: pick(&ui.tab_hover, &t.ansi[0]),
            text: pick(&ui.text, &t.foreground),
            dim: pick(&ui.text_dim, &t.ansi[8]),
            divider: pick(&ui.divider, &t.ansi[8]),
            accent: pick(&ui.accent, &t.ansi[4]),
            danger: pick(&ui.danger, &t.ansi[1]),
            ansi: palette.ansi,
            no_color,
        }
    }

    /// A colour as the terminal gets it.
    pub fn c(&self, rgb: Rgb) -> Color {
        if self.no_color { Color::Reset } else { Color::Rgb(rgb.r, rgb.g, rgb.b) }
    }

    /// Plain text on the chrome's background.
    pub fn base(&self) -> Style {
        Style::new().fg(self.c(self.text)).bg(self.c(self.background))
    }

    pub fn fg(&self, rgb: Rgb) -> Style {
        Style::new().fg(self.c(rgb))
    }

    pub fn dim(&self) -> Style {
        if self.no_color { Style::new().add_modifier(Modifier::DIM) } else { self.fg(self.dim) }
    }

    pub fn accent(&self) -> Style {
        if self.no_color { Style::new().add_modifier(Modifier::BOLD) } else { self.fg(self.accent) }
    }

    pub fn danger(&self) -> Style {
        if self.no_color { Style::new().add_modifier(Modifier::BOLD) } else { self.fg(self.danger) }
    }

    /// What is selected in a list.
    pub fn selected(&self) -> Style {
        if self.no_color {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new().fg(self.c(self.text)).bg(self.c(self.tab_hover)).add_modifier(Modifier::BOLD)
        }
    }

    /// A bench's colour as a chip: its colour behind text that reads on it.
    pub fn chip(&self, color: Rgb) -> Style {
        if self.no_color {
            return Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD);
        }
        let ink = if luminance(color) > 0.45 { Rgb::hex(0x101010) } else { Rgb::hex(0xF8F8F8) };
        Style::new().fg(self.c(ink)).bg(self.c(color)).add_modifier(Modifier::BOLD)
    }

    /// `c` for text on the chrome: bench and tag colours are bright, which
    /// reads on dark themes; on light ones it is darkened until it does.
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

    /// Added and removed lines of a diff.
    pub fn added(&self) -> Style {
        self.fg(self.ansi[2])
    }

    pub fn removed(&self) -> Style {
        self.fg(self.ansi[1])
    }

    /// A colour for a kind of code, from the terminal palette.
    pub fn code(&self, style: shed_project::Style) -> Style {
        use shed_project::Style as S;
        let a = &self.ansi;
        match style {
            S::Keyword => self.fg(a[5]),
            S::String => self.fg(a[2]),
            S::Comment => self.dim().add_modifier(Modifier::ITALIC),
            S::Function => self.fg(a[4]),
            S::Type | S::Tag => self.fg(a[3]),
            S::Number | S::Constant => self.fg(a[9]),
            S::Property | S::Attribute => self.fg(a[6]),
            S::Builtin | S::Escape | S::Label => self.fg(a[13]),
            S::Heading => self.accent().add_modifier(Modifier::BOLD),
            S::Punctuation => self.fg(self.text),
        }
    }
}

/// Whether the terminal prefers a light theme: `COLORFGBG` (set by many
/// terminals as "fg;bg") with a light background. Otherwise dark.
pub fn terminal_is_light() -> bool {
    std::env::var("COLORFGBG").ok().and_then(|v| v.rsplit(';').next().and_then(|b| b.parse::<u8>().ok())).is_some_and(|bg| bg == 7 || bg >= 9)
}

/// `NO_COLOR` set to anything but nothing.
pub fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
}

/// The theme `config` picks for a dark or light terminal.
pub fn theme(config: &Config, dark: bool) -> Theme {
    config.theme(dark).0
}

/// Relative luminance, as WCAG defines it.
pub fn luminance(c: Rgb) -> f32 {
    let linear = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * linear(c.r) + 0.7152 * linear(c.g) + 0.0722 * linear(c.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_color_leaves_colours_to_the_terminal() {
        let t = Theme::load("shed-dark").unwrap();
        let plain = Colors::from_theme(&t, true);
        assert_eq!(plain.c(Rgb::hex(0xff0000)), Color::Reset);
        assert!(plain.selected().add_modifier.contains(Modifier::REVERSED));
        let coloured = Colors::from_theme(&t, false);
        assert_eq!(coloured.c(Rgb::hex(0xff0000)), Color::Rgb(255, 0, 0));
    }
}
