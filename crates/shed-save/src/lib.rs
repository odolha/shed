//! Saving and loading benches.
//!
//! A bench is always saved: the keeper writes each one to
//! `~/.local/share/shed/benches/<name>.toml`, with each pane's scrollback
//! compressed beside it in `<name>/scrollback/<pane>.zst`. The files are
//! meant to be readable and hand-editable:
//!
//! ```toml
//! name = "backend"
//! color = "blue"
//! cwd = "~/code"
//! saved = "2026-11-01T09:12:00Z"
//! active_tab = 0
//!
//! [[tabs]]
//! name = "api"
//! layout = "right 0.6 (0) (1)"
//! tags = [{ label = "dev", color = "green" }]
//!
//! [[tabs.panes]]
//! cwd = "~/code/api"
//! tool = "nvim src/main.rs"
//!
//! [[tabs.panes]]
//! cwd = "~/code/api"
//! tool = "cargo watch -x run"
//!
//! # Set by you: start here and run these instead of the saved tool, and
//! # run the teardown when the bench is closed.
//! start_in = "~/code/api"
//! startup = ["docker compose up -d", "npm run dev"]
//! teardown = ["docker compose down"]
//! ```
//!
//! IDs are kept too, so a loaded tab finds its own history file and logbook
//! scope again.

mod layout;

use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use shed_model::config::RackConfig;
use shed_model::{Aliases, Bench, BenchColor, BenchId, Pane, PaneId, PaneSetup, Rgb, Tab, TabId, Tag, WindowPlace, paths, slug, tag_color};

pub use layout::{format_layout, parse_layout};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedBench {
    pub name: String,
    pub id: BenchId,
    /// The key of its desktop launcher; files from before it was kept use
    /// the name's slug, which is what their launchers were keyed by.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub launcher: String,
    /// A colour name or `#rrggbb`.
    pub color: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<String>,
    pub cwd: String,
    /// When it was saved, UTC.
    pub saved: String,
    #[serde(default)]
    pub active_tab: usize,
    #[serde(default, skip_serializing_if = "Aliases::is_empty")]
    pub aliases: Aliases,
    /// Where its window was last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowPlace>,
    pub tabs: Vec<SavedTab>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedTab {
    pub id: TabId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<SavedTag>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub private: bool,
    /// The split tree, with panes numbered in the order of `panes`:
    /// `0`, or `right 0.6 (0) (down 0.5 (1) (2))`.
    pub layout: String,
    #[serde(default)]
    pub active_pane: usize,
    #[serde(default, skip_serializing_if = "Aliases::is_empty")]
    pub aliases: Aliases,
    /// Font size points over the configured size (the tab's zoom).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub zoom: f32,
    pub panes: Vec<SavedPane>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedTag {
    pub label: String,
    pub color: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub tint: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedPane {
    pub id: PaneId,
    pub cwd: String,
    #[serde(default = "default_cols")]
    pub cols: u16,
    #[serde(default = "default_rows")]
    pub rows: u16,
    /// The command running in the pane, as it was typed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Where the pane starts on load, instead of `cwd`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_in: Option<String>,
    /// Commands to run on load instead of `tool`, one after another.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub startup: Vec<String>,
    /// Type the startup commands and wait for Enter.
    #[serde(default, skip_serializing_if = "is_false")]
    pub wait: bool,
    /// Commands to run when the bench is closed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub teardown: Vec<String>,
    /// Panes of the bench whose readiness startup waits for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<PaneId>,
    /// Ready once the output shows this; otherwise once startup finishes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_when: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

fn default_cols() -> u16 {
    80
}

fn default_rows() -> u16 {
    24
}

/// Tags as saved: colours as `#rrggbb`.
pub fn saved_tags(tags: &[Tag]) -> Vec<SavedTag> {
    tags.iter().map(|t| SavedTag { label: t.label.clone(), color: format!("#{:06X}", t.color.to_hex()), tint: t.tint_tab }).collect()
}

/// What the keeper knows about a pane beyond the tree.
#[derive(Clone, Debug, Default)]
pub struct PaneExtras {
    /// The command in the foreground, as typed.
    pub tool: Option<String>,
}

impl SavedBench {
    /// A bench as it is now. `extras` gives each pane's running command.
    pub fn from_bench(bench: &Bench, extras: impl Fn(PaneId) -> PaneExtras) -> SavedBench {
        let tabs = bench
            .tabs
            .iter()
            .map(|tab| {
                let order = tab.layout.leaves();
                let panes = order
                    .iter()
                    .filter_map(|id| tab.pane(*id))
                    .map(|p| SavedPane {
                        id: p.id,
                        cwd: home_relative(&p.cwd),
                        cols: p.cols,
                        rows: p.rows,
                        tool: extras(p.id).tool,
                        start_in: p.setup.start_in.as_deref().map(home_relative),
                        startup: p.setup.startup.clone(),
                        wait: p.setup.wait,
                        teardown: p.setup.teardown.clone(),
                        after: p.setup.after.clone(),
                        ready_when: p.setup.ready_when.clone(),
                    })
                    .collect();
                SavedTab {
                    id: tab.id,
                    name: tab.name.clone(),
                    tags: tab
                        .tags
                        .iter()
                        .map(|t| SavedTag { label: t.label.clone(), color: format!("#{:06X}", t.color.to_hex()), tint: t.tint_tab })
                        .collect(),
                    private: tab.private,
                    layout: format_layout(&tab.layout, &order),
                    active_pane: order.iter().position(|id| *id == tab.active_pane).unwrap_or(0),
                    aliases: tab.aliases.clone(),
                    zoom: tab.zoom,
                    panes,
                }
            })
            .collect();
        SavedBench {
            name: bench.name.clone(),
            id: bench.id,
            launcher: bench.launcher.clone(),
            color: bench.color.to_text(),
            badge: bench.badge.clone(),
            cwd: home_relative(&bench.cwd),
            saved: utc_now(),
            active_tab: bench.tab_index(bench.active_tab).unwrap_or(0),
            aliases: bench.aliases.clone(),
            window: bench.window,
            tabs,
        }
    }

    /// The bench to rebuild, with each pane's saved size and tool. Panes
    /// whose folder is gone fall back to the bench's (or the home) folder.
    pub fn to_bench(&self) -> Result<(Bench, Vec<(PaneId, SavedPane)>), String> {
        let cwd = expand_home(&self.cwd);
        let mut panes_out = Vec::new();
        let mut tabs = Vec::new();
        for (ti, st) in self.tabs.iter().enumerate() {
            if st.panes.is_empty() {
                continue;
            }
            let ids: Vec<PaneId> = st.panes.iter().map(|p| p.id).collect();
            let layout = parse_layout(&st.layout, &ids).map_err(|e| format!("tab {}: layout: {e}", ti + 1))?;
            let panes: Vec<Pane> = st
                .panes
                .iter()
                .map(|sp| {
                    let mut pane = Pane::new(sp.id, expand_home(&sp.cwd));
                    pane.cols = sp.cols;
                    pane.rows = sp.rows;
                    pane.setup = PaneSetup {
                        start_in: sp.start_in.as_deref().map(expand_home),
                        startup: sp.startup.clone(),
                        wait: sp.wait,
                        teardown: sp.teardown.clone(),
                        after: sp.after.clone(),
                        ready_when: sp.ready_when.clone(),
                    };
                    pane
                })
                .collect();
            panes_out.extend(st.panes.iter().map(|sp| (sp.id, sp.clone())));
            tabs.push(Tab {
                id: st.id,
                name: st.name.clone(),
                tags: st
                    .tags
                    .iter()
                    .map(|t| Tag {
                        label: t.label.clone(),
                        color: tag_color(&t.color).unwrap_or(Rgb::hex(0x8A93A3)),
                        tint_tab: t.tint,
                    })
                    .collect(),
                auto_tags: Vec::new(),
                private: st.private,
                active_pane: ids.get(st.active_pane).copied().unwrap_or(ids[0]),
                layout,
                panes,
                aliases: st.aliases.clone(),
                zoom: st.zoom,
            });
        }
        if tabs.is_empty() {
            return Err("it has no panes".into());
        }
        let active_tab = tabs.get(self.active_tab).map_or(tabs[0].id, |t| t.id);
        let bench = Bench {
            id: self.id,
            name: self.name.clone(),
            color: BenchColor::parse(&self.color).unwrap_or(BenchColor::Slate),
            badge: self.badge.clone(),
            cwd,
            tabs,
            active_tab,
            aliases: self.aliases.clone(),
            closing: false,
            launcher: self.launcher(),
            window: self.window,
        };
        Ok((bench, panes_out))
    }

    pub fn launcher(&self) -> String {
        if self.launcher.is_empty() { slug(&self.name) } else { self.launcher.clone() }
    }

    pub fn tab_mut(&mut self, tab: TabId) -> Option<&mut SavedTab> {
        self.tabs.iter_mut().find(|t| t.id == tab)
    }

    /// A pane's start folder, startup and teardown, as the Setup panel edits them.
    pub fn set_pane_setup(&mut self, pane: PaneId, setup: &PaneSetup) -> bool {
        let Some(p) = self.tabs.iter_mut().flat_map(|t| &mut t.panes).find(|p| p.id == pane) else { return false };
        p.start_in = setup.start_in.as_deref().map(home_relative);
        p.startup = setup.startup.clone();
        p.wait = setup.wait;
        p.teardown = setup.teardown.clone();
        p.after = setup.after.clone();
        p.ready_when = setup.ready_when.clone();
        true
    }

    /// Adds a tab with one pane in the bench's folder.
    pub fn add_tab(&mut self) -> TabId {
        let id = TabId::new();
        self.tabs.push(SavedTab {
            id,
            name: None,
            tags: Vec::new(),
            private: false,
            layout: "0".into(),
            active_pane: 0,
            aliases: Aliases::new(),
            zoom: 0.0,
            panes: vec![SavedPane {
                id: PaneId::new(),
                cwd: self.cwd.clone(),
                cols: default_cols(),
                rows: default_rows(),
                tool: None,
                start_in: None,
                startup: Vec::new(),
                wait: false,
                teardown: Vec::new(),
                after: Vec::new(),
                ready_when: None,
            }],
        });
        id
    }

    pub fn remove_tab(&mut self, tab: TabId) -> Result<(), String> {
        self.take_tab(tab).map(drop)
    }

    /// Takes a tab out, to put it in another bench; a bench keeps at least one.
    pub fn take_tab(&mut self, tab: TabId) -> Result<SavedTab, String> {
        let Some(i) = self.tabs.iter().position(|t| t.id == tab) else { return Err("no such tab".into()) };
        if self.tabs.len() == 1 {
            return Err("a bench keeps at least one tab; delete the bench instead".into());
        }
        let taken = self.tabs.remove(i);
        self.active_tab = self.active_tab.min(self.tabs.len() - 1);
        Ok(taken)
    }

    /// Puts a tab from another bench at `index`.
    pub fn insert_tab(&mut self, tab: SavedTab, index: usize) {
        self.tabs.insert(index.min(self.tabs.len()), tab);
    }

    /// Moves a tab to `index` among the others.
    pub fn move_tab(&mut self, tab: TabId, index: usize) -> bool {
        let Some(i) = self.tabs.iter().position(|t| t.id == tab) else { return false };
        let t = self.tabs.remove(i);
        self.tabs.insert(index.min(self.tabs.len()), t);
        true
    }

    /// A copy under another name, with fresh IDs so both can be open at
    /// once, and its own launcher (`launcher`, which must be free).
    pub fn copy_as(&self, name: &str, launcher: String) -> SavedBench {
        let mut copy = self.clone();
        copy.name = name.to_string();
        copy.launcher = launcher;
        copy.id = BenchId::new();
        let mut renamed = std::collections::HashMap::new();
        for tab in &mut copy.tabs {
            tab.id = TabId::new();
            for pane in &mut tab.panes {
                let fresh = PaneId::new();
                renamed.insert(pane.id, fresh);
                pane.id = fresh;
            }
        }
        // Waits point at the copy's own panes.
        for pane in copy.tabs.iter_mut().flat_map(|t| &mut t.panes) {
            pane.after = pane.after.iter().filter_map(|id| renamed.get(id).copied()).collect();
        }
        copy
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("saved benches always serialize")
    }

    pub fn from_toml(text: &str) -> Result<SavedBench, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }
}

/// What to do with a saved tool when its bench loads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relaunch {
    /// On the rack: run it.
    Run,
    /// Type it at the prompt, wait for Enter.
    Prefill,
    /// On the never list: leave the prompt empty.
    Skip,
}

/// Matches `command` against the rack. An entry matches a program when the
/// program starts with its words (`tail -f` matches `tail -f log.txt`); a
/// path counts by its name (`/usr/bin/nvim` is `nvim`).
///
/// In a chain like `cd ~/app && npm run dev`, the tool is the last step (the
/// first program of a pipeline in it); a chain with any step on the never
/// list is skipped.
pub fn relaunch(rack: &RackConfig, command: &str) -> Relaunch {
    let steps: Vec<&str> = command
        .split(['\n'])
        .flat_map(|l| l.split("&&"))
        .flat_map(|s| s.split("||"))
        .flat_map(|s| s.split(';'))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let Some(last) = steps.last() else { return Relaunch::Skip };
    let programs = |step: &str| step.split('|').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect::<Vec<_>>();
    let on = |list: &[String], program: &str| {
        let words: Vec<&str> = program.split_whitespace().collect();
        let Some(first) = words.first() else { return false };
        let name = first.rsplit('/').next().unwrap_or(first);
        list.iter().any(|entry| {
            let entry: Vec<&str> = entry.split_whitespace().collect();
            !entry.is_empty()
                && entry.len() <= words.len()
                && entry[0] == name
                && entry[1..].iter().zip(&words[1..]).all(|(a, b)| a == b)
        })
    };
    if steps.iter().flat_map(|s| programs(s)).any(|p| on(&rack.never, &p)) {
        Relaunch::Skip
    } else if programs(last).first().is_some_and(|p| on(&rack.auto, p)) {
        Relaunch::Run
    } else {
        Relaunch::Prefill
    }
}

/// Where saved benches live.
#[derive(Clone, Debug)]
pub struct Store {
    pub dir: PathBuf,
}

impl Default for Store {
    fn default() -> Self {
        Store { dir: paths::data_dir().join("benches") }
    }
}

/// A bench that was running when Shed last stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionBench {
    pub name: String,
    /// It had a window (open), rather than running hidden.
    pub open: bool,
}

/// A saved bench as listed, without loading it.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredInfo {
    pub name: String,
    pub id: BenchId,
    pub launcher: String,
    pub color: String,
    pub saved: String,
    pub tabs: usize,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Store { dir }
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{}.toml", file_stem(name)))
    }

    pub fn scrollback_dir(&self, name: &str) -> PathBuf {
        self.dir.join(file_stem(name)).join("scrollback")
    }

    pub fn scrollback_file(&self, name: &str, pane: PaneId) -> PathBuf {
        self.scrollback_dir(name).join(format!("{pane}.zst"))
    }

    /// Writes a bench and its panes' scrollback. Scrollback of panes no
    /// longer in the bench is removed.
    pub fn save(&self, bench: &SavedBench, scrollback: &[(PaneId, String)]) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        write_atomic(&self.file(&bench.name), bench.to_toml().as_bytes())?;
        let dir = self.scrollback_dir(&bench.name);
        std::fs::create_dir_all(&dir)?;
        for (pane, text) in scrollback {
            let compressed = zstd::encode_all(text.as_bytes(), 3)?;
            write_atomic(&self.scrollback_file(&bench.name, *pane), &compressed)?;
        }
        let keep: Vec<String> = bench.tabs.iter().flat_map(|t| &t.panes).map(|p| format!("{}.zst", p.id)).collect();
        for entry in std::fs::read_dir(&dir)?.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".zst") && !keep.contains(&name) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        Ok(())
    }

    pub fn load(&self, name: &str) -> io::Result<SavedBench> {
        let text = std::fs::read_to_string(self.file(name))?;
        SavedBench::from_toml(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{name}: {e}")))
    }

    /// A pane's saved scrollback, if there is any.
    pub fn scrollback(&self, name: &str, pane: PaneId) -> Option<String> {
        let compressed = std::fs::read(self.scrollback_file(name, pane)).ok()?;
        let bytes = zstd::decode_all(compressed.as_slice()).ok()?;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Removes a bench's file and scrollback.
    pub fn delete(&self, name: &str) -> io::Result<()> {
        let _ = std::fs::remove_dir_all(self.dir.join(file_stem(name)));
        match std::fs::remove_file(self.file(name)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// Moves a bench's files to a new name (the file's own `name` is
    /// rewritten by the next save).
    pub fn rename(&self, old: &str, new: &str) -> io::Result<()> {
        if file_stem(old) == file_stem(new) {
            return Ok(());
        }
        if self.file(old).exists() {
            std::fs::rename(self.file(old), self.file(new))?;
        }
        let (from, to) = (self.dir.join(file_stem(old)), self.dir.join(file_stem(new)));
        if from.exists() {
            let _ = std::fs::remove_dir_all(&to);
            std::fs::rename(from, to)?;
        }
        Ok(())
    }

    /// Every saved bench, by name.
    pub fn list(&self) -> Vec<StoredInfo> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut out: Vec<StoredInfo> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter_map(|text| SavedBench::from_toml(&text).ok())
            .map(|b| StoredInfo { tabs: b.tabs.len(), launcher: b.launcher(), name: b.name, id: b.id, color: b.color, saved: b.saved })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    pub fn exists(&self, name: &str) -> bool {
        self.file(name).exists()
    }

    fn session_file(&self) -> PathBuf {
        self.dir.join("../session.toml")
    }

    fn order_file(&self) -> PathBuf {
        self.dir.join("../order.toml")
    }

    fn window_file(&self) -> PathBuf {
        self.dir.join("../window.toml")
    }

    /// Where the Shed window was last.
    pub fn shed_window(&self) -> Option<WindowPlace> {
        std::fs::read_to_string(self.window_file()).ok().and_then(|t| toml::from_str(&t).ok())
    }

    pub fn save_shed_window(&self, place: &WindowPlace) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        write_atomic(&self.window_file(), toml::to_string(place).unwrap().as_bytes())
    }

    /// The order benches are listed in, as the user arranged them (by id, so
    /// renaming keeps the place).
    pub fn order(&self) -> Vec<BenchId> {
        #[derive(Deserialize)]
        struct Order {
            benches: Vec<BenchId>,
        }
        std::fs::read_to_string(self.order_file())
            .ok()
            .and_then(|t| toml::from_str::<Order>(&t).ok())
            .map(|o| o.benches)
            .unwrap_or_default()
    }

    pub fn save_order(&self, benches: &[BenchId]) -> io::Result<()> {
        #[derive(Serialize)]
        struct Order<'a> {
            benches: &'a [BenchId],
        }
        std::fs::create_dir_all(&self.dir)?;
        write_atomic(&self.order_file(), toml::to_string(&Order { benches }).unwrap().as_bytes())
    }

    /// Records which benches were running, to bring them back on the next start.
    /// Records the benches running as Shed stops, and which had a window.
    pub fn save_session(&self, benches: &[SessionBench]) -> io::Result<()> {
        #[derive(Serialize)]
        struct Session<'a> {
            benches: Vec<&'a str>,
            open: Vec<&'a str>,
        }
        let session = Session {
            benches: benches.iter().map(|b| b.name.as_str()).collect(),
            open: benches.iter().filter(|b| b.open).map(|b| b.name.as_str()).collect(),
        };
        std::fs::create_dir_all(&self.dir)?;
        write_atomic(&self.session_file(), toml::to_string(&session).unwrap().as_bytes())
    }

    /// The benches running when the keeper last stopped.
    pub fn session(&self) -> Vec<SessionBench> {
        #[derive(Deserialize)]
        struct Session {
            benches: Vec<String>,
            /// Missing in files from before hiding: every bench had a window.
            open: Option<Vec<String>>,
        }
        let Some(s) = std::fs::read_to_string(self.session_file()).ok().and_then(|t| toml::from_str::<Session>(&t).ok()) else {
            return Vec::new();
        };
        s.benches
            .into_iter()
            .map(|name| SessionBench { open: s.open.as_ref().is_none_or(|o| o.contains(&name)), name })
            .collect()
    }
}

/// The scrollback shown above a loaded pane's first prompt: dimmed, with a
/// divider saying when it was saved.
pub fn restored_scrollback(text: &str, saved: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 128);
    out.extend_from_slice(b"\x1b[2m");
    for line in skip_idle_restores(text.lines()) {
        out.extend_from_slice(line.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    let when = saved.replace('T', " ").trim_end_matches('Z').get(..16).unwrap_or(saved).to_string();
    out.extend_from_slice(format!("\x1b[0m\x1b[2m{DIVIDER}{when} UTC ──\x1b[0m\r\n").as_bytes());
    out
}

const DIVIDER: &str = "── restored · saved ";

/// Drops restores nothing happened after: a prompt, a divider, then the
/// same prompt again. Restarting an idle pane would otherwise stack up a
/// divider and a prompt each time; this way only the newest divider shows.
fn skip_idle_restores<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let mut out: Vec<&str> = Vec::new();
    for line in lines {
        out.push(line);
        let n = out.len();
        if n >= 3 && out[n - 2].starts_with(DIVIDER) && out[n - 3] == out[n - 1] {
            out.drain(n - 3..n - 1);
        }
    }
    out
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)
}

/// A bench name made safe as a file name.
fn file_stem(name: &str) -> String {
    let s: String = name.chars().map(|c| if c == '/' || c == '\\' || c.is_control() { '_' } else { c }).collect();
    let s = s.trim_start_matches('.').to_string();
    if s.is_empty() { "_".into() } else { s }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// `/home/me/code` → `~/code`
pub fn home_relative(path: &Path) -> String {
    if let Some(home) = home()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() { "~".into() } else { format!("~/{}", rest.display()) };
    }
    path.display().to_string()
}

/// `~/code` → `/home/me/code`
pub fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix('~'), home()) {
        (Some(rest), Some(home)) => home.join(rest.trim_start_matches('/')),
        _ => PathBuf::from(path),
    }
}

/// The current time as `2026-11-01T09:12:00Z`.
pub fn utc_now() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil from days, after Howard Hinnant's algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use shed_model::{SplitDir, WindowState};

    fn sample() -> Bench {
        let home = home().unwrap_or_else(|| "/".into());
        let mut tab = Tab::with_pane(Pane::new(PaneId::new(), home.join("code/api")));
        let first = tab.active_pane;
        tab.split(first, Pane::new(PaneId::new(), home.join("code/api")), SplitDir::Right);
        tab.layout.set_ratio(&[], 0.6);
        tab.name = Some("api".into());
        tab.zoom = 2.0;
        tab.aliases.insert("t".into(), "cargo test".into());
        tab.panes[0].setup = PaneSetup {
            start_in: Some(home.join("code")),
            startup: vec!["docker compose up -d".into(), "npm run dev".into()],
            wait: false,
            teardown: vec!["docker compose down".into()],
            ..Default::default()
        };
        tab.tags.push(Tag { label: "dev".into(), color: tag_color("green").unwrap(), tint_tab: false });
        let second = Tab::with_pane(Pane::new(PaneId::new(), "/tmp".into()));
        Bench {
            id: BenchId::new(),
            name: "backend".into(),
            color: BenchColor::Blue,
            badge: Some("B".into()),
            cwd: home.join("code"),
            active_tab: second.id,
            tabs: vec![tab, second],
            aliases: [("up".to_string(), "docker compose up -d".to_string())].into(),
            closing: false,
            launcher: "backend".into(),
            window: Some(WindowPlace { x: 40.0, y: 30.0, width: 1200.0, height: 800.0, state: WindowState::Maximized }),
        }
    }

    #[test]
    fn save_load_save_is_identical() {
        let bench = sample();
        let tools = |id: PaneId| PaneExtras { tool: (id == bench.tabs[0].layout.leaves()[0]).then(|| "nvim src/main.rs".into()) };
        let saved = SavedBench::from_bench(&bench, tools);
        let text = saved.to_toml();
        assert!(text.contains("layout = \"right 0.6 (0) (1)\""), "{text}");
        assert!(text.contains("tool = \"nvim src/main.rs\""));
        assert!(text.contains("cwd = \"~/code/api\""));
        assert!(text.contains("teardown = [\"docker compose down\"]"), "{text}");
        assert!(text.contains("[aliases]") && text.contains("[tabs.aliases]"), "{text}");

        let (loaded, panes) = SavedBench::from_toml(&text).unwrap().to_bench().unwrap();
        assert_eq!(loaded.tabs.len(), 2);
        assert_eq!(loaded.active_tab, bench.active_tab);
        assert_eq!(loaded.tabs[0].layout, bench.tabs[0].layout);
        assert_eq!(panes[0].1.tool.as_deref(), Some("nvim src/main.rs"));
        assert_eq!(loaded.tabs[0].panes[0].setup, bench.tabs[0].panes[0].setup);
        assert_eq!(loaded.aliases, bench.aliases);
        assert_eq!(loaded.tabs[0].aliases, bench.tabs[0].aliases);
        assert_eq!((loaded.tabs[0].zoom, loaded.tabs[1].zoom), (2.0, 0.0));

        let again = SavedBench::from_bench(&loaded, |id| PaneExtras { tool: panes.iter().find(|p| p.0 == id).and_then(|p| p.1.tool.clone()) });
        let again = SavedBench { saved: saved.saved.clone(), ..again };
        assert_eq!(again.to_toml(), text);
    }

    #[test]
    fn store_files_scrollback_session_rename_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("benches"));
        let bench = sample();
        let saved = SavedBench::from_bench(&bench, |_| PaneExtras::default());
        let pane = bench.tabs[0].active_pane;
        store.save(&saved, &[(pane, "line one\nline two".into())]).unwrap();
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.load("backend").unwrap(), saved);
        assert_eq!(store.scrollback("backend", pane).as_deref(), Some("line one\nline two"));

        let session = [
            SessionBench { name: "backend".into(), open: true },
            SessionBench { name: "docs".into(), open: false },
        ];
        store.save_session(&session).unwrap();
        assert_eq!(store.session(), session);
        assert_eq!(store.shed_window(), None);
        let place = WindowPlace { x: 10.0, y: 20.0, width: 900.0, height: 600.0, state: WindowState::Fullscreen };
        store.save_shed_window(&place).unwrap();
        assert_eq!(store.shed_window(), Some(place));
        // Sessions from before hiding list only names: each had a window.
        std::fs::write(store.session_file(), "benches = [\"old\"]\n").unwrap();
        assert_eq!(store.session(), [SessionBench { name: "old".into(), open: true }]);

        store.rename("backend", "server").unwrap();
        assert!(store.exists("server") && !store.exists("backend"));
        assert!(store.scrollback("server", pane).is_some());
        store.delete("server").unwrap();
        assert!(store.list().is_empty());
    }

    #[test]
    fn idle_restores_do_not_pile_up() {
        let d = |t: &str| format!("{DIVIDER}2026-11-01 {t} UTC ──");
        let text = ["$ make", "ok", "$", &d("12:38"), "$", &d("12:41"), "$"].join("\n");
        let out = String::from_utf8(restored_scrollback(&text, "2026-11-01T12:43:00Z")).unwrap();
        assert_eq!(out, format!("\x1b[2m$ make\r\nok\r\n$\r\n\x1b[0m\x1b[2m{}\x1b[0m\r\n", d("12:43")));
        // Something done after a restore keeps its divider.
        let busy = ["$", &d("12:38"), "$ ls", "a b", "$"].join("\n");
        assert_eq!(skip_idle_restores(busy.lines()).len(), 5);
    }

    #[test]
    fn closed_benches_change_tab_by_tab() {
        let mut saved = SavedBench::from_bench(&sample(), |_| PaneExtras::default());
        let (first, second) = (saved.tabs[0].id, saved.tabs[1].id);
        let added = saved.add_tab();
        assert_eq!(saved.tabs.len(), 3);
        assert_eq!(saved.tabs[2].panes[0].cwd, saved.cwd);
        assert!(saved.move_tab(added, 0));
        assert_eq!(saved.tabs.iter().map(|t| t.id).collect::<Vec<_>>(), [added, first, second]);
        saved.remove_tab(first).unwrap();
        saved.remove_tab(second).unwrap();
        assert!(saved.remove_tab(added).is_err(), "the last tab stays");
        let pane = saved.tabs[0].panes[0].id;
        let setup = PaneSetup {
            startup: vec!["make".into()],
            wait: true,
            teardown: vec!["make clean".into()],
            after: vec![PaneId::new()],
            ready_when: Some("Listening".into()),
            ..Default::default()
        };
        assert!(saved.set_pane_setup(pane, &setup));
        let (bench, _) = saved.to_bench().unwrap();
        assert_eq!(bench.tabs[0].panes[0].setup, setup);
        let tags = [Tag { label: "prod".into(), color: Rgb::hex(0xF0566A), tint_tab: true }];
        saved.tab_mut(added).unwrap().tags = saved_tags(&tags);
        assert_eq!(saved.to_bench().unwrap().0.tabs[0].tags, tags);

        // Tabs move between benches whole.
        let mut other = SavedBench::from_bench(&sample(), |_| PaneExtras::default());
        let moving = other.tabs[1].id;
        let tab = other.take_tab(moving).unwrap();
        saved.insert_tab(tab, 0);
        assert_eq!(saved.tabs.iter().map(|t| t.id).collect::<Vec<_>>(), [moving, added]);
        assert!(other.take_tab(other.tabs[0].id).is_err(), "the last tab stays");
    }

    #[test]
    fn bench_order_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("benches"));
        assert!(store.order().is_empty());
        let order = [BenchId::new(), BenchId::new()];
        store.save_order(&order).unwrap();
        assert_eq!(store.order(), order);
    }

    #[test]
    fn copies_get_new_ids() {
        let saved = SavedBench::from_bench(&sample(), |_| PaneExtras::default());
        let copy = saved.copy_as("template", "template".into());
        assert_ne!(copy.id, saved.id);
        assert_eq!((copy.launcher(), saved.launcher()), ("template".into(), "backend".into()));
        assert_ne!(copy.tabs[0].panes[0].id, saved.tabs[0].panes[0].id);
        assert_eq!(copy.tabs[0].layout, saved.tabs[0].layout);
    }

    #[test]
    fn rack_rules() {
        let rack = RackConfig::default();
        assert_eq!(relaunch(&rack, "nvim src/main.rs"), Relaunch::Run);
        assert_eq!(relaunch(&rack, "/usr/bin/htop"), Relaunch::Run);
        assert_eq!(relaunch(&rack, "tail -f /var/log/syslog"), Relaunch::Run);
        assert_eq!(relaunch(&rack, "tail -n 5 x"), Relaunch::Prefill);
        assert_eq!(relaunch(&rack, "cargo build"), Relaunch::Prefill);
        assert_eq!(relaunch(&rack, "cargo watch -x run"), Relaunch::Run);
        assert_eq!(relaunch(&rack, "git push origin main"), Relaunch::Skip);
        assert_eq!(relaunch(&rack, "sudo rm -rf /tmp/x"), Relaunch::Skip);
        assert_eq!(relaunch(&rack, "   "), Relaunch::Skip);
        assert_eq!(relaunch(&rack, "cd ~/app && npm run dev"), Relaunch::Run);
        assert_eq!(relaunch(&rack, "cd /tmp && ls | head -5 && htop"), Relaunch::Run);
        assert_eq!(relaunch(&rack, "tail -f app.log | grep ERROR"), Relaunch::Run);
        assert_eq!(relaunch(&rack, "make && ./deploy"), Relaunch::Prefill);
        assert_eq!(relaunch(&rack, "git push && htop"), Relaunch::Skip);
    }

    #[test]
    fn hand_edited_files_load() {
        let text = r##"
            name = "web"
            id = "8f6d7861-0000-4000-8000-000000000001"
            color = "#123456"
            cwd = "/tmp"
            saved = "2026-11-01T09:12:00Z"

            [[tabs]]
            id = "8f6d7861-0000-4000-8000-000000000002"
            layout = "0"

            [[tabs.panes]]
            id = "8f6d7861-0000-4000-8000-000000000003"
            cwd = "/tmp"
            tool = "npm run dev"
        "##;
        let (bench, panes) = SavedBench::from_toml(text).unwrap().to_bench().unwrap();
        assert_eq!(bench.color, BenchColor::Custom(Rgb::hex(0x123456)));
        assert_eq!(panes[0].1.cols, 80);
        assert_eq!(bench.tabs[0].panes.len(), 1);
    }

    #[test]
    fn utc_format() {
        let now = utc_now();
        assert_eq!(now.len(), 20);
        assert!(now.starts_with("20") && now.ends_with('Z'));
    }
}
