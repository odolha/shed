//! What `shed.toml` and its theme mean for the windows.

use std::collections::BTreeMap;

use shed_emu::Palette;
use shed_model::config::{Config, NewPaneDir};

use crate::theme::{FontSettings, UiColors};

#[derive(Clone, Debug)]
pub struct Settings {
    pub font: FontSettings,
    pub palette: Palette,
    pub ui: UiColors,
    pub padding: f32,
    pub new_pane_dir: NewPaneDir,
    pub icon_badges: bool,
    pub keys: BTreeMap<String, String>,
    /// Problems with the config, shown in every window until fixed.
    pub errors: Vec<String>,
    pub config: Config,
    /// The desktop prefers dark, which `theme = "auto"` follows.
    pub dark: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self::from_config(&Config::default(), true)
    }
}

impl Settings {
    pub fn from_config(config: &Config, dark: bool) -> Self {
        let (theme, error) = config.theme(dark);
        Self {
            font: config.font.clone().into(),
            palette: Palette::from_theme(&theme),
            ui: UiColors::from_theme(&theme),
            padding: config.window.padding.clamp(0.0, 64.0),
            new_pane_dir: config.window.new_pane_dir,
            icon_badges: config.window.icon_badges,
            keys: config.keys.clone(),
            errors: error.into_iter().collect(),
            config: config.clone(),
            dark,
        }
    }

    /// Reads `shed.toml`; a broken file keeps `previous` and reports why.
    pub fn load(previous: &Settings) -> Settings {
        match Config::load_from(&Config::path()) {
            Ok(config) => Settings::from_config(&config, previous.dark),
            Err(e) => Settings { errors: vec![e], ..previous.clone() },
        }
    }
}
