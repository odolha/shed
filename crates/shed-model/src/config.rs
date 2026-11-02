//! `~/.config/shed/shed.toml` and theme files.
//!
//! Every field is optional; a missing or broken file means defaults. Both the
//! window and the keeper read it, and reload it when it changes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;

use crate::{Aliases, Rgb, Tag, Tool, default_tag_color, paths, tag_color};

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub font: FontConfig,
    /// `auto` (the default) follows the desktop's light or dark preference;
    /// or a theme name: `shed-dark`, `shed-light`, or a file in `themes/`.
    pub theme: Option<String>,
    /// The themes `auto` picks between (`shed-light` and `shed-dark`).
    pub theme_light: Option<String>,
    pub theme_dark: Option<String>,
    pub window: WindowConfig,
    /// Key → action name, or `"none"` to unbind. Merged over the defaults.
    pub keys: BTreeMap<String, String>,
    pub rack: RackConfig,
    pub shell: ShellConfig,
    #[serde(rename = "tag_rule")]
    pub tag_rules: Vec<TagRule>,
    /// Seconds between autosaves of every bench.
    pub autosave_seconds: Option<u64>,
    /// Shed reopens the benches that were open or hidden when it last
    /// closed (by hand or with the session). Off: every bench starts closed.
    pub auto_restore: bool,
    pub ai: AiConfig,
}

/// The AI prompt: a line starting with `>` asks a command-line AI for the
/// command to type, which Shed puts on the prompt for review.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AiConfig {
    /// Run with `sh -c` in the pane's folder: the request on stdin, the
    /// command on stdout. Empty turns the AI prompt off.
    pub command: String,
    /// Send the tab's recent commands along (never for private tabs).
    pub history: bool,
    /// Send the text on the pane's screen along (never for private tabs).
    pub screen: bool,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self { command: "claude -p".into(), history: true, screen: true }
    }
}

impl AiConfig {
    pub fn enabled(&self) -> bool {
        !self.command.trim().is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FontConfig {
    pub family: String,
    pub size: f32,
    /// Line height as a multiple of the font size.
    pub line_height: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self { family: "DejaVu Sans Mono".into(), size: 14.0, line_height: 1.25 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NewPaneDir {
    /// The directory of the pane being split, or of the active pane.
    #[default]
    Current,
    /// The bench's own directory.
    Bench,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowConfig {
    /// Space between the window edge and the grid, in pixels.
    pub padding: f32,
    pub new_pane_dir: NewPaneDir,
    /// Draw each bench's badge (or its initials) on its taskbar icon.
    pub icon_badges: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self { padding: 6.0, new_pane_dir: NewPaneDir::Current, icon_badges: false }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShellConfig {
    /// Load Shed's shell integration into the bash and zsh Shed starts.
    /// Without it, `eval "$(shed init bash)"` in ~/.bashrc (or `zsh` in
    /// ~/.zshrc) does the same.
    pub integration: bool,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self { integration: true }
    }
}

/// Which tools restart by themselves when a bench loads (phase 5).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RackConfig {
    pub auto: Vec<String>,
    pub never: Vec<String>,
}

impl Default for RackConfig {
    fn default() -> Self {
        let auto = ["nvim", "vim", "hx", "ssh", "htop", "btop", "less", "tail -f", "cargo watch", "npm run dev"];
        Self { auto: auto.map(String::from).to_vec(), never: ["rm", "git push", "sudo"].map(String::from).to_vec() }
    }
}

/// Tags tabs automatically, by tool or directory.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagRule {
    /// A glob over the tool's command line, such as `ssh *prod*`.
    pub tool: Option<String>,
    /// A glob over the pane's directory, such as `~/work/*`.
    pub cwd: Option<String>,
    pub tag: String,
    pub color: Option<String>,
    #[serde(default)]
    pub tint_tab: bool,
}

impl TagRule {
    /// Whether the rule matches a pane running `tool` in `cwd`.
    pub fn matches(&self, tool: Option<&Tool>, cwd: &Path) -> bool {
        if self.tool.is_none() && self.cwd.is_none() {
            return false;
        }
        let tool_ok = match &self.tool {
            None => true,
            Some(pattern) => tool.is_some_and(|t| glob(pattern, &t.display())),
        };
        let cwd_ok = match &self.cwd {
            None => true,
            Some(pattern) => glob(&expand_home(pattern), &cwd.to_string_lossy()),
        };
        tool_ok && cwd_ok
    }

    pub fn to_tag(&self) -> Tag {
        let color = self.color.as_deref().and_then(tag_color).unwrap_or_else(|| default_tag_color(&self.tag));
        Tag { label: self.tag.clone(), color, tint_tab: self.tint_tab }
    }
}

fn expand_home(pattern: &str) -> String {
    match (pattern.strip_prefix('~'), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{home}{rest}"),
        _ => pattern.to_string(),
    }
}

/// Shell-style matching: `*` is any run of characters, `?` is one.
pub fn glob(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// A terminal colour theme.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    pub foreground: String,
    pub background: String,
    pub cursor: String,
    pub cursor_text: String,
    pub selection: String,
    /// 16 colours: the 8 normal ones, then the 8 bright ones.
    pub ansi: Vec<String>,
    #[serde(default)]
    pub ui: Option<UiTheme>,
}

/// Colours of Shed's own chrome. Missing ones follow the terminal colours.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiTheme {
    pub background: Option<String>,
    pub tab_active: Option<String>,
    pub tab_hover: Option<String>,
    pub text: Option<String>,
    pub text_dim: Option<String>,
    pub divider: Option<String>,
    /// Shed's own highlight: selected switches, the text field, buttons.
    pub accent: Option<String>,
    /// For what stops or removes things.
    pub danger: Option<String>,
}

pub const SHED_DARK: &str = include_str!("../themes/shed-dark.toml");
pub const SHED_LIGHT: &str = include_str!("../themes/shed-light.toml");

impl Theme {
    /// A built-in theme, or `themes/<name>.toml` in the config directory.
    pub fn load(name: &str) -> Result<Theme, String> {
        let text = match name {
            "shed-dark" => SHED_DARK.to_string(),
            "shed-light" => SHED_LIGHT.to_string(),
            _ => {
                let path = paths::config_dir().join("themes").join(format!("{name}.toml"));
                std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?
            }
        };
        let theme: Theme = toml::from_str(&text).map_err(|e| format!("theme {name}: {e}"))?;
        theme.validate().map_err(|e| format!("theme {name}: {e}"))?;
        Ok(theme)
    }

    fn validate(&self) -> Result<(), String> {
        if self.ansi.len() != 16 {
            return Err(format!("ansi needs 16 colours, has {}", self.ansi.len()));
        }
        for c in [&self.foreground, &self.background, &self.cursor, &self.cursor_text, &self.selection]
            .into_iter()
            .chain(&self.ansi)
        {
            Rgb::parse(c).ok_or_else(|| format!("{c:?} is not a #rrggbb colour"))?;
        }
        Ok(())
    }

    /// A colour field; validated on load, so parsing cannot fail.
    pub fn rgb(s: &str) -> Rgb {
        Rgb::parse(s).unwrap_or_default()
    }
}

impl Config {
    pub fn path() -> PathBuf {
        paths::config_dir().join("shed.toml")
    }

    /// Reads the config file. A missing file is the defaults; a broken one is an error.
    pub fn load_from(path: &Path) -> Result<Config, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// The theme the config names, falling back to the default with a message.
    /// The theme in effect when the desktop prefers dark (`dark`) or not.
    pub fn theme(&self, dark: bool) -> (Theme, Option<String>) {
        match Theme::load(&self.theme_name(dark)) {
            Ok(t) => (t, None),
            Err(e) => (Theme::load(if dark { "shed-dark" } else { "shed-light" }).expect("built-in theme"), Some(e)),
        }
    }

    pub fn theme_name(&self, dark: bool) -> String {
        match self.theme.as_deref() {
            None | Some("auto") if dark => self.theme_dark.clone().unwrap_or_else(|| "shed-dark".into()),
            None | Some("auto") => self.theme_light.clone().unwrap_or_else(|| "shed-light".into()),
            Some(name) => name.to_string(),
        }
    }

    /// Where the appearance switch stands; `None` for another theme of one's own.
    pub fn appearance(&self) -> Option<Appearance> {
        match self.theme.as_deref() {
            None | Some("auto") => Some(Appearance::Auto),
            Some(name) if name == self.light_name() => Some(Appearance::Light),
            Some(name) if name == self.dark_name() => Some(Appearance::Dark),
            Some(_) => None,
        }
    }

    fn light_name(&self) -> String {
        self.theme_light.clone().unwrap_or_else(|| "shed-light".into())
    }

    fn dark_name(&self) -> String {
        self.theme_dark.clone().unwrap_or_else(|| "shed-dark".into())
    }

    /// Sets the appearance in the `shed.toml` at `path`, leaving the rest of
    /// the file (comments too) as it is.
    pub fn set_appearance(&self, path: &Path, appearance: Appearance) -> Result<(), String> {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{}: {e}", path.display()))?;
        let theme = match appearance {
            Appearance::Auto => "auto".to_string(),
            Appearance::Light => self.light_name(),
            Appearance::Dark => self.dark_name(),
        };
        doc["theme"] = toml_edit::value(theme);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, doc.to_string()).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Tags the rules give a tab whose panes run these tools in these directories.
    pub fn auto_tags<'a>(&self, panes: impl Iterator<Item = (Option<&'a Tool>, &'a Path)> + Clone) -> Vec<Tag> {
        let mut tags: Vec<Tag> = Vec::new();
        for rule in &self.tag_rules {
            if panes.clone().any(|(tool, cwd)| rule.matches(tool, cwd)) && !tags.iter().any(|t| t.label == rule.tag) {
                tags.push(rule.to_tag());
            }
        }
        tags
    }
}

/// Light or dark, or as the desktop prefers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Auto,
    Light,
    Dark,
}

/// `~/.config/shed/aliases.toml`: Shed's global aliases, `name = "command"`.
/// Kept apart from shed.toml so Shed can rewrite it without touching yours.
pub fn aliases_path() -> PathBuf {
    paths::config_dir().join("aliases.toml")
}

/// Reads global aliases; a missing file is none.
pub fn load_aliases(path: &Path) -> Result<Aliases, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Aliases::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

pub fn save_aliases(path: &Path, aliases: &Aliases) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = toml::to_string(aliases).map_err(std::io::Error::other)?;
    std::fs::write(path, format!("# Shed's global aliases: name = \"command\". Benches and tabs can override them.\n{body}"))
}

/// Where an alias comes from, weakest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AliasLevel {
    /// Defined by the shell itself (~/.bashrc).
    Shell,
    Global,
    Bench,
    Tab,
}

impl AliasLevel {
    pub fn name(self) -> &'static str {
        match self {
            AliasLevel::Shell => "shell",
            AliasLevel::Global => "global",
            AliasLevel::Bench => "bench",
            AliasLevel::Tab => "tab",
        }
    }
}

/// Every alias a tab's shells see, with the level that wins and the ones it hides.
pub fn resolve_aliases(
    shell: &Aliases,
    global: &Aliases,
    bench: &Aliases,
    tab: &Aliases,
) -> Vec<(String, String, AliasLevel, Vec<AliasLevel>)> {
    let levels = [(AliasLevel::Shell, shell), (AliasLevel::Global, global), (AliasLevel::Bench, bench), (AliasLevel::Tab, tab)];
    let mut names: Vec<&String> = levels.iter().flat_map(|(_, a)| a.keys()).collect();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .map(|name| {
            let defined: Vec<(AliasLevel, &String)> =
                levels.iter().filter_map(|(level, a)| a.get(name).map(|v| (*level, v))).collect();
            let (level, value) = *defined.last().unwrap();
            let hidden = defined[..defined.len() - 1].iter().map(|(l, _)| *l).collect();
            (name.clone(), value.clone(), level, hidden)
        })
        .collect()
}

/// The aliases Shed sets in a tab's shells: global, then bench, then tab.
pub fn shed_aliases(global: &Aliases, bench: &Aliases, tab: &Aliases) -> Aliases {
    let mut out = global.clone();
    out.extend(bench.iter().map(|(k, v)| (k.clone(), v.clone())));
    out.extend(tab.iter().map(|(k, v)| (k.clone(), v.clone())));
    out
}

/// Watches a file by modification time: cheap enough to call every second.
#[derive(Debug, Default)]
pub struct FileWatch {
    last: Option<Option<SystemTime>>,
}

impl FileWatch {
    /// True on the first call, then whenever the file's mtime changes
    /// (including the file appearing or disappearing).
    pub fn changed(&mut self, path: &Path) -> bool {
        let now = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let changed = self.last != Some(now);
        self.last = Some(now);
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob("ssh *prod*", "ssh prod-db1"));
        assert!(glob("ssh *prod*", "ssh -p 22 user@prod"));
        assert!(!glob("ssh *prod*", "ssh staging"));
        assert!(glob("ca?go *", "cargo watch"));
        assert!(glob("*", ""));
        assert!(!glob("a", ""));
    }

    #[test]
    fn parses_the_plan_example() {
        let config: Config = toml::from_str(
            r#"
            theme = "shed-light"
            [font]
            size = 13
            [keys]
            "ctrl-shift-d" = "split_right"
            [rack]
            auto = ["nvim", "ssh"]
            never = ["rm"]
            [[tag_rule]]
            tool = "ssh *prod*"
            tag = "prod"
            color = "red"
            tint_tab = true
            "#,
        )
        .unwrap();
        assert_eq!(config.font.size, 13.0);
        assert_eq!(config.font.family, "DejaVu Sans Mono");
        assert_eq!(config.keys["ctrl-shift-d"], "split_right");
        let ssh = Tool { argv: vec!["/usr/bin/ssh".into(), "prod-1".into()] };
        let tags = config.auto_tags([(Some(&ssh), Path::new("/"))].into_iter());
        assert_eq!(tags.len(), 1);
        assert!(tags[0].tint_tab);
        assert_eq!(tags[0].color, tag_color("red").unwrap());
        assert!(config.auto_tags([(None, Path::new("/"))].into_iter()).is_empty());
    }

    #[test]
    fn unknown_fields_are_errors() {
        assert!(toml::from_str::<Config>("fnot = 1").is_err());
    }

    #[test]
    fn builtin_themes_load() {
        assert!(Theme::load("shed-dark").is_ok());
        assert!(Theme::load("shed-light").is_ok());
        let (theme, err) = Config { theme: Some("missing".into()), ..Default::default() }.theme(true);
        assert_eq!(theme, Theme::load("shed-dark").unwrap());
        assert!(err.is_some());
    }

    #[test]
    fn auto_follows_the_desktop() {
        let auto = Config::default();
        assert_eq!((auto.theme_name(true), auto.theme_name(false)), ("shed-dark".into(), "shed-light".into()));
        assert_eq!(auto.appearance(), Some(Appearance::Auto));
        let fixed = Config { theme: Some("shed-light".into()), ..Default::default() };
        assert_eq!(fixed.theme_name(true), "shed-light");
        assert_eq!(fixed.appearance(), Some(Appearance::Light));
        let mine = Config { theme: Some("solarized".into()), ..Default::default() };
        assert_eq!(mine.appearance(), None);
    }

    #[test]
    fn setting_the_appearance_keeps_the_rest_of_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shed.toml");
        std::fs::write(&path, "# my settings\nautosave_seconds = 30 # often\n\n[font]\nsize = 15\n").unwrap();
        let config = Config::load_from(&path).unwrap();
        config.set_appearance(&path, Appearance::Dark).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my settings") && text.contains("# often") && text.contains("size = 15"), "{text}");
        let config = Config::load_from(&path).unwrap();
        assert_eq!((config.appearance(), config.autosave_seconds), (Some(Appearance::Dark), Some(30)));
        config.set_appearance(&path, Appearance::Auto).unwrap();
        assert_eq!(Config::load_from(&path).unwrap().appearance(), Some(Appearance::Auto));
    }

    #[test]
    fn alias_levels() {
        let a = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Aliases>();
        let shell = a(&[("ll", "ls -l"), ("gs", "git status")]);
        let global = a(&[("ll", "ls -la")]);
        let bench = a(&[("up", "docker compose up -d")]);
        let tab = a(&[("ll", "ls -lah")]);
        let r = resolve_aliases(&shell, &global, &bench, &tab);
        let ll = r.iter().find(|x| x.0 == "ll").unwrap();
        assert_eq!((ll.1.as_str(), ll.2), ("ls -lah", AliasLevel::Tab));
        assert_eq!(ll.3, vec![AliasLevel::Shell, AliasLevel::Global]);
        assert_eq!(r.iter().find(|x| x.0 == "gs").unwrap().2, AliasLevel::Shell);
        assert_eq!(shed_aliases(&global, &bench, &tab).len(), 2);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aliases.toml");
        assert!(load_aliases(&path).unwrap().is_empty());
        save_aliases(&path, &global).unwrap();
        assert_eq!(load_aliases(&path).unwrap(), global);
    }

    #[test]
    fn file_watch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shed.toml");
        let mut w = FileWatch::default();
        assert!(w.changed(&path));
        assert!(!w.changed(&path));
        std::fs::write(&path, "").unwrap();
        assert!(w.changed(&path));
    }
}
