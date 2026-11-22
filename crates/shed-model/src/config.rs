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
    /// Opening a bench runs its startup (and starts again what ran in it).
    /// Off (the default), a bench opens quietly; Run Startup starts it.
    pub start_on_open: bool,
    pub ai: AiConfig,
    pub macos: MacosConfig,
    pub git: GitConfig,
    pub editor: EditorConfig,
    /// A colour per tag label (`prod = "red"`, or `"#rrggbb"`): every tag with
    /// that label is drawn in it, wherever it is.
    pub tag_colors: BTreeMap<String, String>,
    /// Linking this workspace to a nexus, to reach it from the web and
    /// Android apps (`shed nexus join`).
    pub nexus: NexusConfig,
    pub keeper: KeeperSettings,
}

/// The keeper (`shedd`).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KeeperSettings {
    /// Keep running when nothing runs and no window or `shed tui` is
    /// connected (as `shedd --persist` does), for a server workspace
    /// reached over ssh. Off: a keeper with nothing to keep exits.
    pub stay: bool,
}

/// A nexus this workspace links to. The token it links with is not here
/// (this file is backed up and often shared): it is in `nexus.json` in Shed's
/// data folder, readable only by you.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NexusConfig {
    /// The nexus's address, `https://nexus.example.com`; empty: not linked.
    pub url: String,
    /// Off keeps the pairing but does not link; on by default once `url` is set.
    pub enabled: Option<bool>,
    /// What the web and Android apps may do here: `full` (the default) or
    /// `view` (look, change nothing).
    pub control: NexusControl,
    /// A PEM file of certificates to trust besides the usual ones, for a
    /// nexus with a certificate of its own making.
    pub ca_file: Option<PathBuf>,
    /// Let Shed's desktop app on your other computers, signed in to the
    /// nexus, show this workspace (its benches and terminals, private tabs
    /// too) by speaking to the keeper through the nexus. Off by default;
    /// needs `control = "full"`.
    pub desktop_clients: bool,
}

impl NexusConfig {
    /// Whether the link should run (given a pairing).
    pub fn active(&self) -> bool {
        !self.url.trim().is_empty() && self.enabled.unwrap_or(true)
    }
}

/// How much connectors may do in a linked workspace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NexusControl {
    #[default]
    Full,
    View,
}

/// The editor in project tabs.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EditorConfig {
    /// Long lines wrap at the window's edge instead of scrolling sideways
    /// (Wrap in the editor's status bar, or Alt+Z).
    pub wrap: bool,
}

/// Git in project tabs.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GitConfig {
    /// Minutes between fetches while a project tab is open, so "behind" is
    /// true; 0 turns it off.
    pub fetch_minutes: u64,
    /// Changed files shown by folder, as a tree, instead of a flat list.
    pub tree: bool,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self { fetch_minutes: 5, tree: false }
    }
}

/// Settings only macOS uses.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MacosConfig {
    /// Option works as Alt (Meta) for the shell, as in Alt+B, Alt+F; off,
    /// it types what macOS gives it (é, @, … on many layouts).
    pub option_as_alt: bool,
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
        Self { command: "claude -p --no-session-persistence".into(), history: true, screen: true }
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
    /// The font emoji are drawn with, in colour.
    pub emoji: String,
}

impl Default for FontConfig {
    fn default() -> Self {
        let family = if cfg!(target_os = "macos") { "Menlo" } else { "DejaVu Sans Mono" };
        let emoji = if cfg!(target_os = "macos") { "Apple Color Emoji" } else { "Noto Color Emoji" };
        Self { family: family.into(), size: 14.0, line_height: 1.25, emoji: emoji.into() }
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
    /// The Shed window lists each bench on one line, its tabs as chips,
    /// without previews (switched from the window's toolbar).
    pub compact_pegboard: bool,
    /// How large a bench window's tab bar is drawn: 1 is the usual size.
    pub tab_bar_scale: f32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self { padding: 6.0, new_pane_dir: NewPaneDir::Current, icon_badges: false, compact_pegboard: false, tab_bar_scale: 1.0 }
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
        paths::write_whole(path, doc.to_string()).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Sets one value in `shed.toml` (`table` is `None` for the top level),
    /// keeping the rest of the file as written: comments, order, the rest.
    pub fn set_value(path: &Path, table: Option<&str>, key: &str, value: toml_edit::Value) -> Result<(), String> {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{}: {e}", path.display()))?;
        match table {
            Some(t) => {
                if !doc.contains_table(t) {
                    doc[t] = toml_edit::table();
                }
                doc[t][key] = toml_edit::Item::Value(value);
            }
            None => doc[key] = toml_edit::Item::Value(value),
        }
        // What it means is checked before it is written.
        toml::from_str::<Config>(&doc.to_string()).map_err(|e| format!("{}: {e}", path.display()))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        paths::write_whole(path, doc.to_string()).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Switches the Shed window between cards with previews and a compact
    /// list, keeping the rest of the file as written.
    pub fn set_compact_pegboard(path: &Path, compact: bool) -> Result<(), String> {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{}: {e}", path.display()))?;
        if !doc.contains_table("window") {
            doc["window"] = toml_edit::table();
        }
        doc["window"]["compact_pegboard"] = toml_edit::value(compact);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        paths::write_whole(path, doc.to_string()).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Tags the rules give a tab whose panes run these tools in these directories.
    pub fn auto_tags<'a>(&self, panes: impl Iterator<Item = (Option<&'a Tool>, &'a Path)> + Clone) -> Vec<Tag> {
        let mut tags: Vec<Tag> = Vec::new();
        for rule in &self.tag_rules {
            if panes.clone().any(|(tool, cwd)| rule.matches(tool, cwd)) && !tags.iter().any(|t| t.label == rule.tag) {
                tags.push(rule.to_tag());
            }
        }
        self.recolor_tags(&mut tags);
        tags
    }

    /// The colour a tag with this label gets: the one set for it
    /// (`[tag_colors]`), else its own stable one.
    pub fn tag_color_of(&self, label: &str) -> Rgb {
        self.tag_colors.get(label).and_then(|c| tag_color(c)).unwrap_or_else(|| default_tag_color(label))
    }

    /// Gives tags the colours set for their labels; whether any changed.
    pub fn recolor_tags(&self, tags: &mut [Tag]) -> bool {
        let mut changed = false;
        for t in tags {
            if let Some(c) = self.tag_colors.get(&t.label).and_then(|c| tag_color(c))
                && t.color != c
            {
                t.color = c;
                changed = true;
            }
        }
        changed
    }

    /// Removes one value from `shed.toml` (a table's key), keeping the rest.
    pub fn remove_value(path: &Path, table: &str, key: &str) -> Result<(), String> {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{}: {e}", path.display()))?;
        if let Some(t) = doc.get_mut(table).and_then(|t| t.as_table_like_mut()) {
            t.remove(key);
            if t.is_empty() {
                doc.remove(table);
            }
        }
        paths::write_whole(path, doc.to_string()).map_err(|e| format!("{}: {e}", path.display()))
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
    paths::write_whole(path, format!("# Shed's global aliases: name = \"command\". Benches and tabs can override them.\n{body}"))
}

/// The shell's own aliases as edited in Shed: `alias` and `unalias` lines,
/// sourced by ~/.bashrc and ~/.zshrc (Shed adds the line), so they work in
/// every terminal, and read again by Shed's shells when it changes.
pub fn shell_aliases_path() -> PathBuf {
    paths::config_dir().join("shell-aliases.sh")
}

const SHELL_ALIASES_HEADER: &str = "\
# Your shell's aliases, as edited in Shed (Setup panel, or `shed alias --shell`).
# ~/.bashrc and ~/.zshrc source this file, so they work in every terminal. You can
# edit it too: Shed changes only the lines of the aliases it is asked to.
";

/// Whether `name` can be an alias in bash and zsh.
pub fn valid_alias_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name.chars().all(|c| c.is_alphanumeric() || "_-.:@+%,^~!".contains(c))
}

/// `value` in single quotes, for the shell.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// The shell-aliases file `text` with `name` set to `value`, or with it
/// removed: an `unalias` line, so a definition in the shell's own startup
/// files goes too. Other lines stay as they are.
pub fn edit_shell_aliases(text: &str, name: &str, value: Option<&str>) -> String {
    let line = match value {
        Some(v) => format!("alias {name}={}", shell_quote(v)),
        None => format!("unalias {name} 2>/dev/null"),
    };
    let names = |l: &str| {
        let l = l.trim_start();
        let rest = l.strip_prefix("alias ").or_else(|| l.strip_prefix("unalias "))?;
        let rest = rest.trim_start().strip_prefix("-- ").unwrap_or(rest.trim_start());
        Some(rest.split(['=', ' ']).next().unwrap_or_default().to_string())
    };
    let mut out: Vec<String> = Vec::new();
    let mut placed = false;
    for l in text.lines() {
        if names(l).as_deref() == Some(name) {
            if !placed {
                out.push(line.clone());
                placed = true;
            }
        } else {
            out.push(l.to_string());
        }
    }
    if out.is_empty() {
        out.extend(SHELL_ALIASES_HEADER.lines().map(String::from));
    }
    if !placed {
        out.push(line);
    }
    out.join("\n") + "\n"
}

/// The lines that make a shell's startup file source `file`.
pub fn source_lines(file: &Path, home: &Path) -> String {
    let path = match file.strip_prefix(home) {
        Ok(rel) => format!("$HOME/{}", rel.display()),
        Err(_) => file.display().to_string(),
    };
    format!("\n# Added by Shed: the aliases you edit in Shed, in every terminal.\n[ -f \"{path}\" ] && . \"{path}\"\n")
}

/// Makes the startup file `rc` source `file`, unless it already does.
/// True if it was added.
pub fn source_from(rc: &Path, file: &Path, home: &Path) -> std::io::Result<bool> {
    let text = match std::fs::read_to_string(rc) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let lines = source_lines(file, home);
    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if text.lines().any(|l| l.contains(name) && !l.trim_start().starts_with('#')) {
        return Ok(false);
    }
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(rc)?;
    f.write_all(lines.as_bytes())?;
    Ok(true)
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
    fn shell_aliases_file_edits() {
        let text = edit_shell_aliases("", "ll", Some("ls -la"));
        assert!(text.starts_with("# Your shell's aliases"));
        assert!(text.ends_with("alias ll='ls -la'\n"));
        let text = edit_shell_aliases(&text, "say", Some("echo 'hi'"));
        assert!(text.contains(r"alias say='echo '\''hi'\'''"));
        // Changing one replaces its line in place; removing leaves an unalias.
        let text = edit_shell_aliases(&text, "ll", Some("ls -l"));
        let text = edit_shell_aliases(&text, "say", None);
        let body: Vec<&str> = text.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(body, ["alias ll='ls -l'", "unalias say 2>/dev/null"]);
        // The user's own lines stay.
        let own = "# mine\nalias gs='git status'\nexport X=1\n";
        assert_eq!(edit_shell_aliases(own, "gs", None), "# mine\nunalias gs 2>/dev/null\nexport X=1\n");
        assert!(valid_alias_name("g.s") && !valid_alias_name("a b") && !valid_alias_name("a=b") && !valid_alias_name("-x"));
    }

    #[test]
    fn startup_files_source_it_once() {
        let dir = std::env::temp_dir().join(format!("shed-rc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rc = dir.join(".bashrc");
        std::fs::write(&rc, "alias x=y\n").unwrap();
        let file = dir.join(".config/shed/shell-aliases.sh");
        assert!(source_from(&rc, &file, &dir).unwrap());
        assert!(!source_from(&rc, &file, &dir).unwrap());
        let text = std::fs::read_to_string(&rc).unwrap();
        assert!(text.starts_with("alias x=y\n"));
        assert!(text.contains(r#"[ -f "$HOME/.config/shed/shell-aliases.sh" ] && . "$HOME/.config/shed/shell-aliases.sh""#));
        std::fs::remove_dir_all(&dir).ok();
    }

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
    fn a_keeper_can_stay() {
        assert!(!Config::default().keeper.stay);
        let config: Config = toml::from_str("[keeper]\nstay = true\n").unwrap();
        assert!(config.keeper.stay);
    }

    #[test]
    fn unknown_fields_are_errors() {
        assert!(toml::from_str::<Config>("fnot = 1").is_err());
    }

    #[test]
    fn nexus_is_off_until_a_url_is_set() {
        assert!(!Config::default().nexus.active());
        let c: Config = toml::from_str("[nexus]\nurl = \"https://nexus.example.com\"\ncontrol = \"view\"\n").unwrap();
        assert!(c.nexus.active());
        assert_eq!(c.nexus.control, NexusControl::View);
        let off: Config = toml::from_str("[nexus]\nurl = \"https://n.example.com\"\nenabled = false\n").unwrap();
        assert!(!off.nexus.active());
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
    fn single_settings_are_written_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shed.toml");
        std::fs::write(&path, "# mine\n[font]\nsize = 15 # big\n").unwrap();
        Config::set_value(&path, Some("window"), "tab_bar_scale", 1.3.into()).unwrap();
        Config::set_value(&path, Some("font"), "size", 16.0.into()).unwrap();
        Config::set_value(&path, None, "auto_restore", false.into()).unwrap();
        let config = Config::load_from(&path).unwrap();
        assert_eq!((config.window.tab_bar_scale, config.font.size, config.auto_restore), (1.3, 16.0, false));
        assert!(std::fs::read_to_string(&path).unwrap().contains("# mine"));
        // A value that does not fit is refused, and the file stays.
        assert!(Config::set_value(&path, Some("window"), "tab_bar_scale", "big".into()).is_err());
        assert_eq!(Config::load_from(&path).unwrap().window.tab_bar_scale, 1.3);
    }

    #[test]
    fn tags_take_the_colours_set_for_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shed.toml");
        std::fs::write(&path, "# mine\n").unwrap();
        Config::set_value(&path, Some("tag_colors"), "prod db", "red".into()).unwrap();
        let config = Config::load_from(&path).unwrap();
        assert_eq!(config.tag_color_of("prod db"), tag_color("red").unwrap());
        assert_eq!(config.tag_color_of("other"), default_tag_color("other"));
        let mut tags = vec![Tag { label: "prod db".into(), color: tag_color("teal").unwrap(), tint_tab: true }];
        assert!(config.recolor_tags(&mut tags));
        assert_eq!(tags[0].color, tag_color("red").unwrap());
        assert!(!config.recolor_tags(&mut tags));
        Config::remove_value(&path, "tag_colors", "prod db").unwrap();
        assert!(Config::load_from(&path).unwrap().tag_colors.is_empty());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# mine"), "{text:?}");
    }

    #[test]
    fn the_compact_view_is_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shed.toml");
        assert!(!Config::load_from(&path).unwrap_or_default().window.compact_pegboard);
        Config::set_compact_pegboard(&path, true).unwrap();
        assert!(Config::load_from(&path).unwrap().window.compact_pegboard);
        std::fs::write(&path, "[window]\npadding = 9 # mine\n").unwrap();
        Config::set_compact_pegboard(&path, true).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("padding = 9 # mine"), "{text}");
        let config = Config::load_from(&path).unwrap();
        assert!(config.window.compact_pegboard && config.window.padding == 9.0);
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
