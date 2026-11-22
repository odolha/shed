//! "The taskbar follows the workspace" (`[desktop] taskbar_follows_workspace`):
//! as Shed's window switches workspace, the panel's pinned bench launchers
//! switch with it. Shed remembers, per workspace, which of its bench
//! launchers (`dev.shed.bench.…`) were pinned; switching takes those out of
//! the panel and puts the other workspace's in, where they were. Nothing
//! else in the panel is touched: other pins keep their order, and Shed's own
//! launcher (`dev.shed.desktop`) stays.
//!
//! Two panels are known:
//!
//! - **Cinnamon**: the grouped window list applet (and forks of it) keeps
//!   its pins in `~/.config/cinnamon/spices/<uuid>/<instance>.json`, key
//!   `pinned-apps`. The applet reads them only as it loads, so after the file
//!   is written Cinnamon is asked over D-Bus to reload that applet
//!   (`org.Cinnamon.ReloadXlet`, as Cinnamon's own tools do): the window list
//!   is drawn again, windows are not touched.
//! - **GNOME**: `org.gnome.shell favorite-apps`, through `gsettings`, which
//!   the dash follows live.
//!
//! Before the first change, the panel's settings are copied to
//! `panel-backup/` in Shed's state folder (once; see `docs/user-guide.md` for
//! putting them back).

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

/// Pins of Shed's bench launchers start with this.
pub const BENCH_PREFIX: &str = "dev.shed.bench.";
/// Shed's own launcher, after which a workspace's pins go when there is no
/// better place.
const SHED_PIN: &str = "dev.shed.desktop";

/// A panel whose pins Shed can switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Cinnamon,
    Gnome,
}

impl Panel {
    /// The panel of the desktop running (`XDG_CURRENT_DESKTOP`), if known.
    pub fn detect() -> Option<Panel> {
        if !cfg!(target_os = "linux") {
            return None;
        }
        Panel::from_desktop(&std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default())
    }

    /// From `XDG_CURRENT_DESKTOP` (a colon-separated list: `X-Cinnamon`,
    /// `ubuntu:GNOME`).
    pub fn from_desktop(desktop: &str) -> Option<Panel> {
        desktop.split(':').find_map(|d| match d.trim().to_ascii_lowercase().as_str() {
            "x-cinnamon" | "cinnamon" => Some(Panel::Cinnamon),
            "gnome" => Some(Panel::Gnome),
            _ => None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Panel::Cinnamon => "Cinnamon",
            Panel::Gnome => "GNOME",
        }
    }
}

/// Whether a pin is one of Shed's bench launchers.
pub fn is_bench_pin(pin: &str) -> bool {
    pin.starts_with(BENCH_PREFIX)
}

/// A workspace's pinned bench launchers, and where they were.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Pins {
    pub pins: Vec<String>,
    /// The pin they came right after, if any.
    #[serde(default)]
    pub after: Option<String>,
    /// They were the first pins.
    #[serde(default)]
    pub first: bool,
}

/// Pins after switching workspace: Shed's bench launchers come out (given
/// back, with where they were: the workspace left had them pinned) and
/// `incoming`'s go in where the first of them was; with none there, where
/// `incoming`'s were, else right after Shed's own launcher, else at the end.
/// Other pins keep their order.
pub fn swap(current: &[String], incoming: &Pins) -> (Vec<String>, Pins) {
    let first = current.iter().position(|p| is_bench_pin(p));
    let outgoing = Pins {
        pins: current.iter().filter(|p| is_bench_pin(p)).cloned().collect(),
        after: first.and_then(|i| current[..i].last().cloned()),
        first: first == Some(0),
    };
    let mut pins: Vec<String> = current.iter().filter(|p| !is_bench_pin(p)).cloned().collect();
    let after = |pin: &str| pins.iter().position(|p| p == pin).map(|i| i + 1);
    let at = match first {
        // Others before the first bench pin stay before.
        Some(i) => current[..i].iter().filter(|p| !is_bench_pin(p)).count(),
        None if incoming.first => 0,
        None => incoming
            .after
            .as_deref()
            .and_then(after)
            .or_else(|| after(SHED_PIN))
            .or_else(|| after(&format!("{SHED_PIN}.desktop")))
            .unwrap_or(pins.len()),
    };
    let mut seen = std::collections::HashSet::new();
    let incoming: Vec<String> = incoming.pins.iter().filter(|p| is_bench_pin(p) && seen.insert(p.to_string())).cloned().collect();
    pins.splice(at..at, incoming);
    (pins, outgoing)
}

// ------------------------------------------------------------------ GNOME

/// Reads a GVariant string array as `gsettings get` prints it
/// (`['a.desktop', 'b.desktop']`, `@as []`).
pub fn parse_strv(text: &str) -> Option<Vec<String>> {
    let text = text.trim();
    let text = text.strip_prefix("@as").map(str::trim_start).unwrap_or(text);
    let inner = text.strip_prefix('[')?.strip_suffix(']')?;
    let mut items = Vec::new();
    let mut chars = inner.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace() || *c == ',') {
            chars.next();
        }
        let Some(quote) = chars.next() else { break };
        if quote != '\'' && quote != '"' {
            return None;
        }
        let mut item = String::new();
        loop {
            match chars.next()? {
                '\\' => item.push(chars.next()?),
                c if c == quote => break,
                c => item.push(c),
            }
        }
        items.push(item);
    }
    Some(items)
}

/// Writes a string array as `gsettings set` takes it.
pub fn format_strv(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|i| format!("'{}'", i.replace('\\', "\\\\").replace('\'', "\\'"))).collect();
    format!("[{}]", quoted.join(", "))
}

// --------------------------------------------------------------- Cinnamon

/// A Cinnamon window list applet's settings file with pins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CinnamonApplet {
    pub path: PathBuf,
    /// The applet's uuid (its folder's name).
    pub uuid: String,
    /// Its instance id (the file's name).
    pub instance: String,
}

/// The applets under `spices` (`~/.config/cinnamon/spices`) whose settings
/// have a `pinned-apps` list: the grouped window list and its forks.
pub fn cinnamon_applets(spices: &Path) -> Vec<CinnamonApplet> {
    let mut found = Vec::new();
    let Ok(dirs) = std::fs::read_dir(spices) else { return found };
    for dir in dirs.flatten() {
        let uuid = dir.file_name().to_string_lossy().into_owned();
        let Ok(files) = std::fs::read_dir(dir.path()) else { continue };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            if serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|v| cinnamon_pins(&v)).is_some() {
                let instance = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                found.push(CinnamonApplet { path, uuid: uuid.clone(), instance });
            }
        }
    }
    found.sort_by(|a, b| (&a.uuid, &a.instance).cmp(&(&b.uuid, &b.instance)));
    found
}

/// The pins in an applet's settings (`pinned-apps.value`).
pub fn cinnamon_pins(settings: &serde_json::Value) -> Option<Vec<String>> {
    let list = settings.get("pinned-apps")?.get("value")?.as_array()?;
    list.iter().map(|v| v.as_str().map(str::to_string)).collect()
}

/// Sets the pins in an applet's settings, leaving the rest as it is.
pub fn set_cinnamon_pins(settings: &mut serde_json::Value, pins: &[String]) -> bool {
    let Some(entry) = settings.get_mut("pinned-apps").and_then(|e| e.as_object_mut()) else { return false };
    entry.insert("value".into(), serde_json::Value::Array(pins.iter().cloned().map(serde_json::Value::String).collect()));
    true
}

// ---------------------------------------------------------------- switching

/// Runs a program and gives its output (`gsettings`, `gdbus`); tests pass
/// their own.
pub trait Run {
    fn run(&self, program: &str, args: &[&str]) -> io::Result<String>;
}

/// The programs themselves.
pub struct System;

impl Run for System {
    fn run(&self, program: &str, args: &[&str]) -> io::Result<String> {
        let out = std::process::Command::new(program).args(args).stdin(std::process::Stdio::null()).output()?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(io::Error::other(format!("{program}: {}", err.trim())));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// What Shed remembers (`taskbar.json` in its state folder): whose pins the
/// panel has now, and each workspace's pins per place in the panel.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Memory {
    /// The workspace whose bench launchers the panel has pinned now
    /// (`local` for this PC's).
    #[serde(default)]
    pub pinned_for: Option<String>,
    /// Workspace → panel place (`gnome`, `cinnamon:<uuid>/<instance>`) →
    /// its pins.
    #[serde(default)]
    pub workspaces: BTreeMap<String, BTreeMap<String, Pins>>,
}

/// The key this PC's workspace goes by.
pub const LOCAL: &str = "local";

/// Switches a panel's pinned bench launchers between workspaces.
pub struct Taskbar<'a> {
    pub panel: Panel,
    /// Shed's state folder: `taskbar.json` and `panel-backup/`.
    pub state_dir: PathBuf,
    /// `~/.config` (Cinnamon's settings are under it).
    pub config_home: PathBuf,
    pub run: &'a dyn Run,
}

impl Taskbar<'_> {
    fn memory_path(&self) -> PathBuf {
        self.state_dir.join("taskbar.json")
    }

    /// Where the panel's settings were copied before Shed first changed them.
    pub fn backup_dir(&self) -> PathBuf {
        self.state_dir.join("panel-backup")
    }

    fn spices(&self) -> PathBuf {
        self.config_home.join("cinnamon/spices")
    }

    pub fn memory(&self) -> Memory {
        std::fs::read_to_string(self.memory_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn save_memory(&self, memory: &Memory) -> io::Result<()> {
        write_whole(&self.memory_path(), &serde_json::to_string_pretty(memory).map_err(io::Error::other)?)
    }

    /// Makes the panel show `to`'s pinned bench launchers (a workspace's
    /// id, or `LOCAL`), remembering those of the workspace it showed. Does
    /// nothing if they are `to`'s already.
    pub fn switch(&self, to: &str) -> io::Result<()> {
        let mut memory = self.memory();
        let from = memory.pinned_for.clone().unwrap_or_else(|| LOCAL.to_string());
        if from == to {
            if memory.pinned_for.is_none() {
                memory.pinned_for = Some(from);
                self.save_memory(&memory)?;
            }
            return Ok(());
        }
        self.back_up()?;
        let incoming = memory.workspaces.get(to).cloned().unwrap_or_default();
        let mut outgoing = BTreeMap::new();
        match self.panel {
            Panel::Gnome => {
                let text = self.run.run("gsettings", &["get", "org.gnome.shell", "favorite-apps"])?;
                let current = parse_strv(&text).ok_or_else(|| io::Error::other(format!("favorite-apps did not read: {}", text.trim())))?;
                let (pins, out) = swap(&current, &incoming.get("gnome").cloned().unwrap_or_default());
                if pins != current {
                    self.run.run("gsettings", &["set", "org.gnome.shell", "favorite-apps", &format_strv(&pins)])?;
                }
                outgoing.insert("gnome".to_string(), out);
            }
            Panel::Cinnamon => {
                let mut reload = Vec::new();
                for applet in cinnamon_applets(&self.spices()) {
                    let place = format!("cinnamon:{}/{}", applet.uuid, applet.instance);
                    let text = std::fs::read_to_string(&applet.path)?;
                    let mut settings: serde_json::Value = serde_json::from_str(&text).map_err(io::Error::other)?;
                    let Some(current) = cinnamon_pins(&settings) else { continue };
                    let (pins, out) = swap(&current, &incoming.get(&place).cloned().unwrap_or_default());
                    outgoing.insert(place, out);
                    if pins == current {
                        continue;
                    }
                    set_cinnamon_pins(&mut settings, &pins);
                    // Cinnamon writes these with four spaces too.
                    let mut text = Vec::new();
                    let mut ser = serde_json::Serializer::with_formatter(&mut text, serde_json::ser::PrettyFormatter::with_indent(b"    "));
                    serde::Serialize::serialize(&settings, &mut ser).map_err(io::Error::other)?;
                    write_whole(&applet.path, &String::from_utf8_lossy(&text))?;
                    if !reload.contains(&applet.uuid) {
                        reload.push(applet.uuid);
                    }
                }
                // The applet reads its pins as it loads: load it again.
                for uuid in reload {
                    self.run.run(
                        "gdbus",
                        &["call", "--session", "--dest", "org.Cinnamon", "--object-path", "/org/Cinnamon", "--method", "org.Cinnamon.ReloadXlet", &uuid, "APPLET"],
                    )?;
                }
            }
        }
        memory.workspaces.insert(from, outgoing);
        memory.pinned_for = Some(to.to_string());
        self.save_memory(&memory)
    }

    /// Copies the panel's settings to `panel-backup/`, once: the first copy
    /// is the one from before Shed changed anything.
    fn back_up(&self) -> io::Result<()> {
        let dir = self.backup_dir();
        match self.panel {
            Panel::Gnome => {
                let path = dir.join("gnome-favorite-apps.txt");
                if !path.exists() {
                    let text = self.run.run("gsettings", &["get", "org.gnome.shell", "favorite-apps"])?;
                    write_whole(&path, text.trim())?;
                }
            }
            Panel::Cinnamon => {
                for applet in cinnamon_applets(&self.spices()) {
                    let path = dir.join("cinnamon").join(&applet.uuid).join(format!("{}.json", applet.instance));
                    if !path.exists() {
                        write_whole(&path, &std::fs::read_to_string(&applet.path)?)?;
                    }
                }
            }
        }
        Ok(())
    }
}

fn write_whole(path: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("shed-{}.tmp", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|i| i.to_string()).collect()
    }

    #[test]
    fn swapping_touches_only_bench_pins() {
        let current = s(&["dev.shed.desktop", "dev.shed.bench.main.desktop", "nemo.desktop", "dev.shed.bench.web.desktop", "firefox.desktop"]);
        let (pins, out) = swap(&current, &p(&["dev.shed.bench.office.api.desktop", "dev.shed.bench.office.db.desktop"]));
        assert_eq!(out.pins, s(&["dev.shed.bench.main.desktop", "dev.shed.bench.web.desktop"]));
        assert_eq!((out.after.as_deref(), out.first), (Some("dev.shed.desktop"), false));
        assert_eq!(
            pins,
            s(&["dev.shed.desktop", "dev.shed.bench.office.api.desktop", "dev.shed.bench.office.db.desktop", "nemo.desktop", "firefox.desktop"])
        );
        // And back: where they were.
        let (back, out) = swap(&pins, &p(&["dev.shed.bench.main.desktop", "dev.shed.bench.web.desktop"]));
        assert_eq!(back, s(&["dev.shed.desktop", "dev.shed.bench.main.desktop", "dev.shed.bench.web.desktop", "nemo.desktop", "firefox.desktop"]));
        assert_eq!(out.pins, s(&["dev.shed.bench.office.api.desktop", "dev.shed.bench.office.db.desktop"]));
    }

    fn p(items: &[&str]) -> Pins {
        Pins { pins: s(items), ..Default::default() }
    }

    #[test]
    fn pins_come_back_where_they_were() {
        let (none, out) = swap(&s(&["a.desktop", "dev.shed.bench.x.desktop", "b.desktop"]), &Pins::default());
        assert_eq!(none, s(&["a.desktop", "b.desktop"]));
        let (back, _) = swap(&none, &out);
        assert_eq!(back, s(&["a.desktop", "dev.shed.bench.x.desktop", "b.desktop"]));
        // First in the panel.
        let (none, out) = swap(&s(&["dev.shed.bench.x.desktop", "a.desktop"]), &Pins::default());
        assert!(out.first);
        assert_eq!(swap(&none, &out).0, s(&["dev.shed.bench.x.desktop", "a.desktop"]));
        // The pin they followed is gone: after Shed's own, else at the end.
        let gone = Pins { pins: s(&["dev.shed.bench.x.desktop"]), after: Some("gone.desktop".into()), first: false };
        assert_eq!(swap(&s(&["a.desktop", "dev.shed.desktop", "b.desktop"]), &gone).0, s(&["a.desktop", "dev.shed.desktop", "dev.shed.bench.x.desktop", "b.desktop"]));
        assert_eq!(swap(&s(&["a.desktop"]), &gone).0, s(&["a.desktop", "dev.shed.bench.x.desktop"]));
    }

    #[test]
    fn swapped_in_pins_go_after_shed_or_at_the_end() {
        let (pins, out) = swap(&s(&["nemo.desktop", "dev.shed.desktop", "spotify.desktop"]), &p(&["dev.shed.bench.a.desktop"]));
        assert!(out.pins.is_empty());
        assert_eq!(pins, s(&["nemo.desktop", "dev.shed.desktop", "dev.shed.bench.a.desktop", "spotify.desktop"]));
        let (pins, _) = swap(&s(&["nemo.desktop"]), &p(&["dev.shed.bench.a.desktop", "dev.shed.bench.a.desktop", "nemo.desktop"]));
        assert_eq!(pins, s(&["nemo.desktop", "dev.shed.bench.a.desktop"]), "only bench pins, once each");
        let (pins, out) = swap(&s(&["com.app.desktop:flatpak", "dev.shed.bench.x.desktop"]), &Pins::default());
        assert_eq!((pins, out.pins), (s(&["com.app.desktop:flatpak"]), s(&["dev.shed.bench.x.desktop"])));
    }

    #[test]
    fn gsettings_string_arrays() {
        assert_eq!(parse_strv("['firefox.desktop', 'org.gnome.Nautilus.desktop']\n").unwrap(), s(&["firefox.desktop", "org.gnome.Nautilus.desktop"]));
        assert_eq!(parse_strv("@as []").unwrap(), Vec::<String>::new());
        assert_eq!(parse_strv("['it\\'s.desktop', \"q.desktop\"]").unwrap(), s(&["it's.desktop", "q.desktop"]));
        assert!(parse_strv("nonsense").is_none());
        let items = s(&["a.desktop", "it's.desktop", "back\\slash"]);
        assert_eq!(parse_strv(&format_strv(&items)).unwrap(), items);
        assert_eq!(format_strv(&[]), "[]");
    }

    #[test]
    fn desktops_are_told_apart() {
        assert_eq!(Panel::from_desktop("X-Cinnamon"), Some(Panel::Cinnamon));
        assert_eq!(Panel::from_desktop("ubuntu:GNOME"), Some(Panel::Gnome));
        assert_eq!(Panel::from_desktop("KDE"), None);
        assert_eq!(Panel::from_desktop(""), None);
    }

    /// `gsettings` and `gdbus`, pretended: GNOME's favourites kept here,
    /// every call noted.
    struct Fake {
        favorites: RefCell<String>,
        calls: RefCell<Vec<Vec<String>>>,
    }

    impl Run for Fake {
        fn run(&self, program: &str, args: &[&str]) -> io::Result<String> {
            let mut call = vec![program.to_string()];
            call.extend(args.iter().map(|a| a.to_string()));
            self.calls.borrow_mut().push(call);
            match (program, args) {
                ("gsettings", ["get", "org.gnome.shell", "favorite-apps"]) => Ok(format!("{}\n", self.favorites.borrow())),
                ("gsettings", ["set", "org.gnome.shell", "favorite-apps", value]) => {
                    *self.favorites.borrow_mut() = value.to_string();
                    Ok(String::new())
                }
                ("gdbus", _) => Ok("()\n".into()),
                _ => Err(io::Error::other("not faked")),
            }
        }
    }

    fn fake(favorites: &str) -> Fake {
        Fake { favorites: RefCell::new(favorites.into()), calls: RefCell::new(Vec::new()) }
    }

    const APPLET: &str = r#"{
    "group-apps": {"type": "checkbox", "default": true, "value": true},
    "pinned-apps": {
        "type": "generic",
        "default": ["nemo.desktop", "firefox.desktop"],
        "value": ["dev.shed.desktop", "dev.shed.bench.main.desktop", "nemo.desktop", "signal-desktop.desktop"]
    },
    "__md5__": "abc"
}"#;

    #[test]
    fn cinnamon_pins_follow_the_workspace() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("config");
        let state = home.path().join("state/shed");
        let applet = config.join("cinnamon/spices/grouped-window-list@cinnamon.org/2.json");
        std::fs::create_dir_all(applet.parent().unwrap()).unwrap();
        std::fs::write(&applet, APPLET).unwrap();
        // Another applet's settings, without pins, are left alone.
        let other = config.join("cinnamon/spices/menu@cinnamon.org/1.json");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(&other, r#"{"menu-label": {"type": "entry", "value": "Menu"}}"#).unwrap();
        let run = fake("");
        let taskbar = Taskbar { panel: Panel::Cinnamon, state_dir: state.clone(), config_home: config.clone(), run: &run };
        let pins = || cinnamon_pins(&serde_json::from_str(&std::fs::read_to_string(&applet).unwrap()).unwrap()).unwrap();

        // To a workspace never shown: this PC's bench pins go, and are remembered.
        taskbar.switch("ws-office").unwrap();
        assert_eq!(pins(), s(&["dev.shed.desktop", "nemo.desktop", "signal-desktop.desktop"]));
        let reload = run.calls.borrow().iter().filter(|c| c.contains(&"org.Cinnamon.ReloadXlet".to_string())).cloned().collect::<Vec<_>>();
        assert_eq!(reload.len(), 1);
        assert!(reload[0].ends_with(&s(&["grouped-window-list@cinnamon.org", "APPLET"])), "{reload:?}");
        // The panel as it was, kept once.
        let backup = state.join("panel-backup/cinnamon/grouped-window-list@cinnamon.org/2.json");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), APPLET);
        // The rest of the file is kept.
        let settings: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&applet).unwrap()).unwrap();
        assert_eq!(settings["__md5__"], "abc");
        assert_eq!(settings["group-apps"]["value"], true);
        assert_eq!(settings["pinned-apps"]["default"][0], "nemo.desktop");

        // Pinned there: one of the office's benches, and an app of the user's.
        let mut settings = settings;
        set_cinnamon_pins(&mut settings, &s(&["dev.shed.desktop", "nemo.desktop", "dev.shed.bench.office.api.desktop", "signal-desktop.desktop", "spotify.desktop"]));
        std::fs::write(&applet, serde_json::to_string(&settings).unwrap()).unwrap();

        // Back to this PC: its pins come back where they were; the user's new pin stays.
        taskbar.switch(LOCAL).unwrap();
        assert_eq!(pins(), s(&["dev.shed.desktop", "nemo.desktop", "dev.shed.bench.main.desktop", "signal-desktop.desktop", "spotify.desktop"]));
        // And to the office again: its own.
        taskbar.switch("ws-office").unwrap();
        assert_eq!(pins(), s(&["dev.shed.desktop", "nemo.desktop", "dev.shed.bench.office.api.desktop", "signal-desktop.desktop", "spotify.desktop"]));
        assert_eq!(taskbar.memory().pinned_for.as_deref(), Some("ws-office"));
        // The backup is still the first one.
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), APPLET);
        // Switching to where it is changes nothing.
        let calls = run.calls.borrow().len();
        taskbar.switch("ws-office").unwrap();
        assert_eq!(run.calls.borrow().len(), calls);
        assert_eq!(std::fs::read_to_string(&other).unwrap(), r#"{"menu-label": {"type": "entry", "value": "Menu"}}"#);
    }

    #[test]
    fn gnome_favorites_follow_the_workspace() {
        let home = tempfile::tempdir().unwrap();
        let state = home.path().join("state/shed");
        let run = fake("['firefox.desktop', 'dev.shed.bench.main.desktop', 'org.gnome.Nautilus.desktop']");
        let taskbar = Taskbar { panel: Panel::Gnome, state_dir: state.clone(), config_home: home.path().join("config"), run: &run };
        taskbar.switch("ws-1").unwrap();
        assert_eq!(parse_strv(&run.favorites.borrow()).unwrap(), s(&["firefox.desktop", "org.gnome.Nautilus.desktop"]));
        assert_eq!(
            std::fs::read_to_string(state.join("panel-backup/gnome-favorite-apps.txt")).unwrap(),
            "['firefox.desktop', 'dev.shed.bench.main.desktop', 'org.gnome.Nautilus.desktop']"
        );
        taskbar.switch(LOCAL).unwrap();
        assert_eq!(parse_strv(&run.favorites.borrow()).unwrap(), s(&["firefox.desktop", "dev.shed.bench.main.desktop", "org.gnome.Nautilus.desktop"]));
        assert!(run.calls.borrow().iter().all(|c| c[0] == "gsettings"));
    }
}
