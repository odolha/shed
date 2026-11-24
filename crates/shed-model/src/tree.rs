use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{BenchColor, BenchId, PaneId, Rgb, TabId};

/// The whole of Shed: every bench the keeper holds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Shed {
    pub benches: Vec<Bench>,
    /// Shed's global aliases (from aliases.toml).
    pub aliases: Aliases,
    /// Where the Shed window was last, to open it there again.
    pub window: Option<WindowPlace>,
}

/// Where a window is on screen and how big, in logical pixels, to open it
/// the same way again.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowPlace {
    /// Its size and position when neither maximized nor full screen.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub state: WindowState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowState {
    #[default]
    Normal,
    Maximized,
    Fullscreen,
}

impl Shed {
    pub fn bench(&self, id: BenchId) -> Option<&Bench> {
        self.benches.iter().find(|b| b.id == id)
    }

    pub fn bench_mut(&mut self, id: BenchId) -> Option<&mut Bench> {
        self.benches.iter_mut().find(|b| b.id == id)
    }

    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        self.benches.iter().flat_map(|b| &b.tabs).find(|t| t.id == id)
    }

    pub fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.benches.iter_mut().flat_map(|b| &mut b.tabs).find(|t| t.id == id)
    }

    /// The bench holding `tab`.
    pub fn bench_of_tab(&self, tab: TabId) -> Option<BenchId> {
        self.benches.iter().find(|b| b.tabs.iter().any(|t| t.id == tab)).map(|b| b.id)
    }

    /// Finds the bench and tab that hold `pane`.
    pub fn locate_pane(&self, pane: PaneId) -> Option<(BenchId, TabId)> {
        self.benches.iter().find_map(|b| {
            // A project tab's layout names a pane that does not exist.
            b.tabs.iter().find(|t| t.has_panes() && t.layout.contains(pane)).map(|t| (b.id, t.id))
        })
    }

    pub fn pane(&self, pane: PaneId) -> Option<&Pane> {
        self.benches.iter().flat_map(|b| &b.tabs).flat_map(|t| &t.panes).find(|p| p.id == pane)
    }

    pub fn pane_mut(&mut self, pane: PaneId) -> Option<&mut Pane> {
        self.benches.iter_mut().flat_map(|b| &mut b.tabs).flat_map(|t| &mut t.panes).find(|p| p.id == pane)
    }

    pub fn pane_ids(&self) -> impl Iterator<Item = PaneId> + '_ {
        self.benches.iter().flat_map(|b| &b.tabs).flat_map(|t| &t.panes).map(|p| p.id)
    }

    /// Removes a pane from whichever tab holds it. Empty tabs are removed, and
    /// empty benches too. Returns the bench the pane was in.
    pub fn remove_pane(&mut self, pane: PaneId) -> Option<BenchId> {
        let (bench_id, tab_id) = self.locate_pane(pane)?;
        let tab = self.tab_mut(tab_id)?;
        tab.panes.retain(|p| p.id != pane);
        match tab.layout.remove(pane) {
            Some(rest) => {
                tab.layout = rest;
                if tab.active_pane == pane {
                    tab.active_pane = tab.layout.first();
                }
            }
            None => {
                self.take_tab(tab_id);
            }
        }
        self.drop_empty_benches();
        Some(bench_id)
    }

    /// Removes a tab and returns it, fixing up its bench's active tab.
    pub fn take_tab(&mut self, tab: TabId) -> Option<Tab> {
        let bench = self.bench_mut(self.bench_of_tab(tab)?)?;
        let index = bench.tabs.iter().position(|t| t.id == tab)?;
        let removed = bench.tabs.remove(index);
        if bench.active_tab == tab
            && let Some(next) = bench.tabs.get(index.min(bench.tabs.len().saturating_sub(1)))
        {
            bench.active_tab = next.id;
        }
        Some(removed)
    }

    /// Moves a tab to `bench`, before the tab at `index` there (counted with
    /// the moved tab still in place), into that tab's group; past the end,
    /// last. It becomes active there; a bench left without tabs is removed.
    pub fn move_tab(&mut self, tab: TabId, bench: BenchId, index: usize) -> bool {
        self.move_tab_into(tab, bench, index, true)
    }

    /// Moves a tab just before or after `to`, into `to`'s group.
    pub fn move_tab_next_to(&mut self, tab: TabId, to: TabId, after: bool) -> bool {
        if tab == to {
            return false;
        }
        let Some(bench) = self.bench_of_tab(to) else { return false };
        let Some(at) = self.bench(bench).and_then(|b| b.tabs.iter().position(|t| t.id == to)) else { return false };
        if after { self.move_tab_into(tab, bench, at + 1, false) } else { self.move_tab_into(tab, bench, at, true) }
    }

    /// `join_next`: put first in the group of the tab it lands before.
    fn move_tab_into(&mut self, tab: TabId, bench: BenchId, index: usize, join_next: bool) -> bool {
        if self.bench(bench).is_none() {
            return false;
        }
        // Dropped on a tab, it joins that tab's group.
        let onto = self.bench(bench).and_then(|b| b.tabs.get(index)).map(|t| t.id).filter(|t| *t != tab && join_next);
        // A group's divider is on its first tab: leaving, that tab hands it on.
        if let Some(from) = self.bench_of_tab(tab)
            && let Some(b) = self.bench_mut(from)
            && let Some(at) = b.tabs.iter().position(|t| t.id == tab)
        {
            let divider = b.tabs[at].divider;
            if divider != TabDivider::None
                && let Some(next) = b.tabs.get_mut(at + 1)
                && next.divider == TabDivider::None
            {
                next.divider = divider;
            }
        }
        // `index` counts the tab itself where it is now: moving right, one less.
        let from_here = self.bench(bench).and_then(|b| b.tabs.iter().position(|t| t.id == tab));
        let index = if from_here.is_some_and(|at| at < index) { index - 1 } else { index };
        let Some(mut moved) = self.take_tab(tab) else { return false };
        moved.divider = TabDivider::None;
        let target = self.bench_mut(bench).unwrap();
        let index = index.min(target.tabs.len());
        target.tabs.insert(index, moved);
        // Put first in that group: the divider moves to it.
        if let Some(onto) = onto
            && target.tabs.get(index + 1).is_some_and(|t| t.id == onto)
        {
            target.tabs[index].divider = std::mem::take(&mut target.tabs[index + 1].divider);
        }
        target.active_tab = tab;
        self.drop_empty_benches();
        true
    }

    /// Puts a bench's tabs in the order given, with the dividers given
    /// (`groups::Arrangement`). Tabs not in the bench are passed over; the
    /// bench's tabs not given (opened meanwhile) stay after, as they were.
    /// `focus`, one of them, becomes the active tab.
    pub fn arrange_tabs(&mut self, bench: BenchId, order: &[(TabId, TabDivider)], focus: Option<TabId>) -> bool {
        let Some(b) = self.bench_mut(bench) else { return false };
        let mut rest = std::mem::take(&mut b.tabs);
        for (id, divider) in order {
            if let Some(at) = rest.iter().position(|t| t.id == *id) {
                let mut tab = rest.remove(at);
                tab.divider = *divider;
                b.tabs.push(tab);
            }
        }
        b.tabs.append(&mut rest);
        if let Some(focus) = focus.filter(|f| b.tabs.iter().any(|t| t.id == *f)) {
            b.active_tab = focus;
        }
        true
    }

    /// Removes a tab that has no panes (a project tab); a bench left without
    /// tabs is removed. Tabs with panes leave as their panes exit.
    pub fn close_paneless_tab(&mut self, tab: TabId) -> Option<Tab> {
        if self.tab(tab).is_none_or(|t| !t.panes.is_empty()) {
            return None;
        }
        let taken = self.take_tab(tab);
        self.drop_empty_benches();
        taken
    }

    /// Makes `pane` the active pane of its tab, and that tab active in its bench.
    pub fn focus_pane(&mut self, pane: PaneId) -> bool {
        let Some((bench, tab)) = self.locate_pane(pane) else { return false };
        self.tab_mut(tab).unwrap().active_pane = pane;
        self.bench_mut(bench).unwrap().active_tab = tab;
        true
    }

    fn drop_empty_benches(&mut self) {
        self.benches.retain(|b| !b.tabs.is_empty());
    }
}

/// A workspace for one job, shown in its own window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bench {
    pub id: BenchId,
    pub name: String,
    pub color: BenchColor,
    pub badge: Option<String>,
    /// Where new tabs and panes open.
    pub cwd: PathBuf,
    pub tabs: Vec<Tab>,
    pub active_tab: TabId,
    /// Aliases for every shell in the bench (over global ones, under a tab's).
    pub aliases: Aliases,
    /// The bench is being closed: its tools are stopping and teardowns running.
    pub closing: bool,
    /// What the bench's desktop launcher and window app ID are keyed by:
    /// the slug of its name when it was made. Renaming keeps it, so a
    /// launcher pinned to a dock keeps working.
    pub launcher: String,
    /// Where its window was last, to open it there again.
    pub window: Option<WindowPlace>,
    /// Its startup ran since it opened (so closing runs its teardown).
    #[serde(default)]
    pub started: bool,
    /// Web pages that go with the bench's work (an app, its dashboards),
    /// shown on its tab bar and in Shed.
    #[serde(default)]
    pub links: Vec<ProjectLink>,
}

/// A bench name made safe for app IDs and file names.
pub fn slug(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "bench".into() } else { s }
}

/// Shed-level aliases: name → command. They override the shell's own.
pub type Aliases = BTreeMap<String, String>;

impl Bench {
    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == self.active_tab).or(self.tabs.first())
    }

    pub fn tab_index(&self, tab: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == tab)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tab {
    pub id: TabId,
    /// A name the user gave the tab; otherwise the title comes from its active pane.
    pub name: Option<String>,
    /// Tags the user set.
    pub tags: Vec<Tag>,
    /// Tags set by tag rules, recomputed as tools and directories change.
    pub auto_tags: Vec<Tag>,
    /// Commands run here stay in this tab's local logbook only.
    pub private: bool,
    pub layout: PaneNode,
    pub panes: Vec<Pane>,
    pub active_pane: PaneId,
    /// Aliases for this tab's shells, over the bench's and global ones.
    pub aliases: Aliases,
    /// Points added to the configured font size in this tab's panes.
    pub zoom: f32,
    /// What the tab is: terminals, or a project's code.
    pub kind: TabKind,
    /// A divider before this tab on the tab bar: it starts a group.
    #[serde(default)]
    pub divider: TabDivider,
}

/// What sets a tab apart from the one before it on the tab bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TabDivider {
    #[default]
    None,
    /// A space: the tabs from here on are a group, which goes onto the next
    /// row whole when it does not fit.
    Space,
    /// A new row: the group starts a row of its own.
    Row,
}

/// What a tab holds. Only terminal tabs have panes; the others show
/// something of their own in the tab's whole area.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum TabKind {
    #[default]
    Terminal,
    Project(Project),
    // (Boxed: an AI tab carries much more than the others.)
    Ai(Box<AiTab>),
    /// A shell on a server: a terminal tab whose pane joins the shell the
    /// server's keeper holds for `id`, over ssh, and joins it again when the
    /// bench opens.
    Remote(RemoteTab),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteTab {
    /// As ssh takes it: a host from `~/.ssh/config`, or `user@host`.
    pub host: String,
    /// What the server's keeper knows the shell by.
    pub id: String,
}

/// A Claude Code conversation, shown in an AI tab. The keeper runs it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AiTab {
    /// Where Claude Code works and keeps its sessions.
    pub folder: PathBuf,
    /// More folders it may read and change (the bench's other projects).
    pub add_dirs: Vec<PathBuf>,
    /// The session shown and continued; a new one when `None`.
    pub session: Option<String>,
    pub model: Option<String>,
    /// Claude Code's permission mode: `default`, `acceptEdits`, `plan`.
    pub mode: String,
    pub effort: Option<String>,
    /// Files it edits open in the bench's project tabs as it goes.
    pub follow: bool,
    /// Where the conversation is, for the tab bar.
    pub activity: AiActivity,
    /// Messages written to send some other time, in no order, each with
    /// the session it belongs to.
    pub drafts: Vec<Draft>,
    /// Messages sent while Claude was busy: each goes when a turn ends.
    pub queued: Vec<Queued>,
    /// What is written in the prompt and not sent yet.
    pub unsent: String,
    /// Changes marked viewed in the review: each change's key, and a hash
    /// of what was viewed (a change that differs since is not viewed).
    pub viewed: Vec<(String, String)>,
    /// Explanations of changes, asked aside: each change's key, the hash of
    /// what was explained, and the explanation.
    pub explained: Vec<(String, String, String)>,
    /// Its Claude Code may use the browser (Claude in Chrome); at most one AI
    /// tab at a time.
    pub browser: bool,
    /// The draft session shown (one with drafts and no Claude Code session
    /// yet), when `session` is `None`: its key.
    pub draft_session: Option<String>,
    /// Sessions the tab moved away from while Claude was still at them:
    /// their Claude Code goes on in the background until done (not saved).
    pub away: Vec<AiAway>,
    /// A fork not started yet (`session` is `None`): the session it copies.
    /// It shows that one's conversation; its first prompt starts Claude Code
    /// on a copy of it (`--fork-session`), the original left as it is.
    pub fork_of: Option<String>,
}

/// A session Claude goes on with while the tab shows another.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiAway {
    /// Claude Code's session id.
    pub session: String,
    pub activity: AiActivity,
}

/// A message kept to send later, and the session it belongs to: Claude
/// Code's session id, or a draft session's key (`draft-…`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub session: String,
    pub text: String,
}

impl Draft {
    /// The prompt with this draft merged into it: the draft's text at the
    /// end, after a blank line (alone, in a prompt with nothing in it).
    pub fn merged_into(&self, prompt: &str) -> String {
        let prompt = prompt.trim_end();
        if prompt.trim().is_empty() { self.text.clone() } else { format!("{prompt}\n\n{}", self.text) }
    }
}

/// A draft session: drafts written in a new session before anything was
/// sent, listed with the sessions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftSession {
    pub key: String,
    /// From its first draft's first line.
    pub title: String,
    pub drafts: usize,
}

impl AiTab {
    /// Draft sessions' keys start with it.
    pub const DRAFT_SESSION: &'static str = "draft-";

    /// A key for a new draft session.
    pub fn new_draft_key() -> String {
        format!("{}{}", Self::DRAFT_SESSION, uuid::Uuid::new_v4())
    }

    pub fn is_draft_key(key: &str) -> bool {
        key.starts_with(Self::DRAFT_SESSION)
    }

    /// The key the shown session's drafts go by: Claude Code's session id,
    /// or the draft session's; `None` for a new session with neither.
    pub fn session_key(&self) -> Option<&str> {
        self.session.as_deref().or(self.draft_session.as_deref())
    }

    /// The drafts of the session shown, with their places in `drafts`.
    pub fn shown_drafts(&self) -> impl Iterator<Item = (usize, &Draft)> {
        let key = self.session_key().map(str::to_string);
        self.drafts.iter().enumerate().filter(move |(_, d)| key.as_deref() == Some(d.session.as_str()))
    }

    /// The drafts of session `from` belong to `to` from now on (Claude Code
    /// gave the conversation a new id: its first one, or a copy's).
    pub fn move_drafts(&mut self, from: &str, to: &str) {
        for d in self.drafts.iter_mut().filter(|d| d.session == from) {
            d.session = to.to_string();
        }
    }

    /// Where the tab's conversations are, all told: waiting for the user
    /// if any is, else working if any is (the one shown or one in the
    /// background).
    pub fn overall_activity(&self) -> AiActivity {
        std::iter::once(self.activity).chain(self.away.iter().map(|a| a.activity)).max_by_key(|a| match a {
            AiActivity::Idle => 0,
            AiActivity::Working => 1,
            AiActivity::Waiting => 2,
        }).unwrap_or_default()
    }

    /// The sessions Claude is at, with where each is: the one shown (when
    /// not idle) and those going on in the background.
    pub fn running(&self) -> impl Iterator<Item = (&str, AiActivity)> {
        let shown = self.session.as_deref().filter(|_| self.activity != AiActivity::Idle).map(|s| (s, self.activity));
        shown.into_iter().chain(self.away.iter().map(|a| (a.session.as_str(), a.activity)))
    }

    /// The draft sessions, newest first (the order their first drafts were kept, reversed).
    pub fn draft_sessions(&self) -> Vec<DraftSession> {
        let mut out: Vec<DraftSession> = Vec::new();
        for d in self.drafts.iter().filter(|d| Self::is_draft_key(&d.session)) {
            match out.iter_mut().find(|s| s.key == d.session) {
                Some(s) => s.drafts += 1,
                None => {
                    let first = d.text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
                    let mut title: String = first.chars().take(60).collect();
                    if first.chars().count() > 60 {
                        title.push('…');
                    }
                    out.push(DraftSession { key: d.session.clone(), title, drafts: 1 });
                }
            }
        }
        out.reverse();
        out
    }
}

/// A message waiting its turn: its text, and its pictures as files.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queued {
    pub text: String,
    // No `skip_serializing_if`: it travels in the keeper's protocol too,
    // which has no field names to tell a left-out field by.
    #[serde(default)]
    pub pictures: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AiActivity {
    #[default]
    Idle,
    Working,
    /// It waits for the user to allow or deny something.
    Waiting,
}

/// A folder of code, shown in a project tab.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub root: PathBuf,
    /// Files open in the tab, in tab order.
    pub open: Vec<PathBuf>,
    /// Which of `open` is shown.
    pub active: Option<usize>,
    /// Globs (relative to the root, as in `.gitignore`) the tab leaves out
    /// of its file list, search and watching.
    pub exclude: Vec<String>,
    /// The project's web apps: local, staging, production…
    pub links: Vec<ProjectLink>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectLink {
    pub name: String,
    pub url: String,
}

impl Tab {
    /// A tab with a single pane.
    pub fn with_pane(pane: Pane) -> Self {
        Self {
            id: TabId::new(),
            kind: TabKind::Terminal,
            name: None,
            tags: Vec::new(),
            auto_tags: Vec::new(),
            private: false,
            layout: PaneNode::Leaf(pane.id),
            active_pane: pane.id,
            panes: vec![pane],
            aliases: Aliases::new(),
            zoom: 0.0,
            divider: TabDivider::None,
        }
    }

    /// A project tab for the folder `root`. It has no panes; its layout and
    /// active pane name a pane that does not exist.
    pub fn project(root: PathBuf) -> Self {
        let none = PaneId::new();
        Self {
            id: TabId::new(),
            kind: TabKind::Project(Project { root, ..Default::default() }),
            name: None,
            tags: Vec::new(),
            auto_tags: Vec::new(),
            private: false,
            layout: PaneNode::Leaf(none),
            active_pane: none,
            panes: Vec::new(),
            aliases: Aliases::new(),
            zoom: 0.0,
            divider: TabDivider::None,
        }
    }

    /// An AI tab for Claude Code working in `folder`.
    pub fn ai(folder: PathBuf) -> Self {
        let mut tab = Self::project(folder.clone());
        // Auto: Claude Code's safety check decides, so routine work does not wait.
        tab.kind = TabKind::Ai(Box::new(AiTab { folder, mode: "auto".into(), ..Default::default() }));
        tab
    }

    /// The project this tab shows, if it is a project tab.
    pub fn project_info(&self) -> Option<&Project> {
        match &self.kind {
            TabKind::Project(p) => Some(p),
            _ => None,
        }
    }

    /// Terminal and remote tabs hold panes; the others show something of their own.
    pub fn has_panes(&self) -> bool {
        matches!(self.kind, TabKind::Terminal | TabKind::Remote(_))
    }

    pub fn remote_info(&self) -> Option<&RemoteTab> {
        match &self.kind {
            TabKind::Remote(r) => Some(r),
            _ => None,
        }
    }

    /// The conversation this tab shows, if it is an AI tab.
    pub fn ai_info(&self) -> Option<&AiTab> {
        match &self.kind {
            TabKind::Ai(a) => Some(a),
            _ => None,
        }
    }

    pub fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.panes.iter().find(|p| p.id == id)
    }

    pub fn active(&self) -> Option<&Pane> {
        self.pane(self.active_pane)
    }

    /// What the tab bar shows: the user's name, else the active pane's title,
    /// else its tool, else its directory. A project tab: its folder's name.
    pub fn display_title(&self) -> String {
        if let Some(name) = &self.name {
            return name.clone();
        }
        if let Some(project) = self.project_info() {
            return folder_name(&project.root);
        }
        if let Some(ai) = self.ai_info() {
            return format!("Claude · {}", folder_name(&ai.folder));
        }
        let Some(pane) = self.active() else { return "shell".into() };
        if let Some(title) = pane.title.as_deref().filter(|t| !t.trim().is_empty()) {
            return title.to_string();
        }
        if let Some(tool) = &pane.tool {
            return tool.display();
        }
        folder_name(&pane.cwd)
    }

    /// Every tag on the tab: the user's, then the rules'.
    pub fn all_tags(&self) -> impl Iterator<Item = &Tag> {
        self.tags.iter().chain(self.auto_tags.iter().filter(|a| !self.tags.iter().any(|t| t.label == a.label)))
    }

    /// Splits `target`, putting `pane` after it in direction `dir`.
    pub fn split(&mut self, target: PaneId, pane: Pane, dir: SplitDir) -> bool {
        if !self.layout.split(target, pane.id, dir) {
            return false;
        }
        self.active_pane = pane.id;
        self.panes.push(pane);
        true
    }
}

/// A folder's last component, or the whole path for `/`.
fn folder_name(dir: &std::path::Path) -> String {
    dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| dir.display().to_string())
}

/// A short coloured label on a tab.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tag {
    pub label: String,
    pub color: Rgb,
    /// Tint the whole tab, as a warning (for production shells, say).
    pub tint_tab: bool,
}

/// One terminal inside a tab.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pane {
    pub id: PaneId,
    /// The shell's working directory.
    pub cwd: PathBuf,
    pub cols: u16,
    pub rows: u16,
    /// The title the running program set, if any.
    pub title: Option<String>,
    /// The program in the foreground, when it is not the shell itself.
    pub tool: Option<Tool>,
    /// A command typed at the prompt by a bench load, waiting for Enter.
    pub waiting: Option<String>,
    /// What the pane runs when its bench loads, and when it closes.
    pub setup: PaneSetup,
    /// The aliases the shell itself defines (from ~/.bashrc and the like).
    pub shell_aliases: Aliases,
    /// A request to the AI prompt, under way or failed.
    pub ai: Option<AiState>,
    /// Startup held until other panes are ready: what it waits for.
    pub held: Option<Held>,
}

/// Why a pane's startup has not run yet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Held {
    /// Waiting for these panes to be ready (their titles).
    Waiting(Vec<String>),
    /// A pane it waits for failed (its title); it stays held.
    Failed(String),
}

/// Where a pane's AI request is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AiState {
    /// Asking, with the request.
    Asking(String),
    /// It failed, and why; cleared by the next key.
    Failed(String),
}

/// A pane's own startup and teardown, set by the user.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSetup {
    /// Where the pane starts when its bench loads; otherwise where its shell
    /// was when the bench was last saved.
    pub start_in: Option<PathBuf>,
    /// Run one after another at the prompt when the bench loads, instead of
    /// whatever was running when it was saved.
    pub startup: Vec<String>,
    /// Type the startup commands but wait for Enter instead of running them.
    pub wait: bool,
    /// Run when the bench is closed, after the running tool is stopped.
    pub teardown: Vec<String>,
    /// When the bench loads, startup waits until these panes of the bench
    /// are ready.
    pub after: Vec<PaneId>,
    /// This pane is ready, for the panes that start after it, once its
    /// output shows this text; `None`: once its startup has finished.
    pub ready_when: Option<String>,
}

impl PaneSetup {
    pub fn is_empty(&self) -> bool {
        self.start_in.is_none() && self.startup.is_empty() && self.teardown.is_empty() && self.after.is_empty() && self.ready_when.is_none()
    }
}

impl Pane {
    pub fn new(id: PaneId, cwd: PathBuf) -> Self {
        Self {
            id,
            cwd,
            cols: 80,
            rows: 24,
            title: None,
            tool: None,
            waiting: None,
            setup: PaneSetup::default(),
            shell_aliases: Aliases::new(),
            ai: None,
            held: None,
        }
    }
}

/// A program running in the foreground of a pane.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tool {
    pub argv: Vec<String>,
}

impl Tool {
    /// The program name without its path, then its arguments.
    pub fn display(&self) -> String {
        let mut parts = self.argv.iter();
        let Some(first) = parts.next() else { return String::new() };
        let name = first.rsplit('/').next().unwrap_or(first);
        std::iter::once(name).chain(parts.map(String::as_str)).collect::<Vec<_>>().join(" ")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitDir {
    /// Children side by side, the second to the right.
    Right,
    /// Children stacked, the second below.
    Down,
}

/// A direction for moving between panes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// A rectangle in layout units (the whole tab is `0..1` on both axes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const UNIT: Rect = Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 };
}

/// The split tree of a tab's panes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PaneNode {
    Leaf(PaneId),
    Split {
        dir: SplitDir,
        /// Share of the space given to `first`, in `0.0..1.0`.
        ratio: f32,
        first: Box<PaneNode>,
        second: Box<PaneNode>,
    },
}

impl PaneNode {
    pub fn contains(&self, pane: PaneId) -> bool {
        match self {
            PaneNode::Leaf(id) => *id == pane,
            PaneNode::Split { first, second, .. } => first.contains(pane) || second.contains(pane),
        }
    }

    pub fn first(&self) -> PaneId {
        match self {
            PaneNode::Leaf(id) => *id,
            PaneNode::Split { first, .. } => first.first(),
        }
    }

    /// Every pane, in reading order.
    pub fn leaves(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut Vec<PaneId>) {
        match self {
            PaneNode::Leaf(id) => out.push(*id),
            PaneNode::Split { first, second, .. } => {
                first.collect(out);
                second.collect(out);
            }
        }
    }

    /// Replaces the leaf `target` with a split of `target` and `new`.
    pub fn split(&mut self, target: PaneId, new: PaneId, dir: SplitDir) -> bool {
        match self {
            PaneNode::Leaf(id) if *id == target => {
                *self = PaneNode::Split {
                    dir,
                    ratio: 0.5,
                    first: Box::new(PaneNode::Leaf(target)),
                    second: Box::new(PaneNode::Leaf(new)),
                };
                true
            }
            PaneNode::Leaf(_) => false,
            PaneNode::Split { first, second, .. } => {
                first.split(target, new, dir) || second.split(target, new, dir)
            }
        }
    }

    /// The tree without `pane`, or `None` if nothing is left.
    pub fn remove(&self, pane: PaneId) -> Option<PaneNode> {
        match self {
            PaneNode::Leaf(id) if *id == pane => None,
            PaneNode::Leaf(_) => Some(self.clone()),
            PaneNode::Split { dir, ratio, first, second } => {
                match (first.remove(pane), second.remove(pane)) {
                    (Some(a), Some(b)) => Some(PaneNode::Split {
                        dir: *dir,
                        ratio: *ratio,
                        first: Box::new(a),
                        second: Box::new(b),
                    }),
                    (Some(only), None) | (None, Some(only)) => Some(only),
                    (None, None) => None,
                }
            }
        }
    }

    /// Each pane's rectangle within `area`.
    pub fn rects(&self, area: Rect) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::new();
        self.collect_rects(area, &mut out);
        out
    }

    fn collect_rects(&self, area: Rect, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            PaneNode::Leaf(id) => out.push((*id, area)),
            PaneNode::Split { dir, ratio, first, second } => {
                let (a, b) = split_rect(area, *dir, *ratio);
                first.collect_rects(a, out);
                second.collect_rects(b, out);
            }
        }
    }

    /// The pane next to `from` in `dir`: the one sharing the most edge with it,
    /// nearest first.
    pub fn neighbor(&self, from: PaneId, dir: Direction) -> Option<PaneId> {
        let rects = self.rects(Rect::UNIT);
        let (_, r) = *rects.iter().find(|(id, _)| *id == from)?;
        const EPS: f32 = 1e-4;
        let overlap = |a0: f32, a1: f32, b0: f32, b1: f32| (a1.min(b1) - a0.max(b0)).max(0.0);
        rects
            .iter()
            .filter(|(id, _)| *id != from)
            .filter_map(|(id, o)| {
                let (gap, shared) = match dir {
                    Direction::Left => (r.x - (o.x + o.w), overlap(r.y, r.y + r.h, o.y, o.y + o.h)),
                    Direction::Right => (o.x - (r.x + r.w), overlap(r.y, r.y + r.h, o.y, o.y + o.h)),
                    Direction::Up => (r.y - (o.y + o.h), overlap(r.x, r.x + r.w, o.x, o.x + o.w)),
                    Direction::Down => (o.y - (r.y + r.h), overlap(r.x, r.x + r.w, o.x, o.x + o.w)),
                };
                (gap > -EPS && shared > EPS).then_some((*id, gap, shared))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1).then(b.2.total_cmp(&a.2)))
            .map(|(id, _, _)| id)
    }

    /// Sets the ratio of the split at `path` (`false` = first child, `true` = second).
    pub fn set_ratio(&mut self, path: &[bool], new: f32) -> bool {
        match (self, path.split_first()) {
            (PaneNode::Split { ratio, .. }, None) => {
                *ratio = new.clamp(0.05, 0.95);
                true
            }
            (PaneNode::Split { first, second, .. }, Some((&go_second, rest))) => {
                if go_second { second.set_ratio(rest, new) } else { first.set_ratio(rest, new) }
            }
            (PaneNode::Leaf(_), _) => false,
        }
    }

    /// The same layout with every ratio clamped to keep panes visible.
    pub fn clamped(&self) -> PaneNode {
        match self {
            PaneNode::Leaf(_) => self.clone(),
            PaneNode::Split { dir, ratio, first, second } => PaneNode::Split {
                dir: *dir,
                ratio: ratio.clamp(0.05, 0.95),
                first: Box::new(first.clamped()),
                second: Box::new(second.clamped()),
            },
        }
    }
}

/// Splits `area` in two along `dir`, giving `ratio` of it to the first part.
pub fn split_rect(area: Rect, dir: SplitDir, ratio: f32) -> (Rect, Rect) {
    match dir {
        SplitDir::Right => {
            let w = area.w * ratio;
            (Rect { w, ..area }, Rect { x: area.x + w, w: area.w - w, ..area })
        }
        SplitDir::Down => {
            let h = area.h * ratio;
            (Rect { h, ..area }, Rect { y: area.y + h, h: area.h - h, ..area })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draft_merges_at_the_end_after_a_blank_line() {
        let draft = Draft { session: "s".into(), text: "and the tests".into() };
        assert_eq!(draft.merged_into(""), "and the tests");
        assert_eq!(draft.merged_into("  \n"), "and the tests");
        assert_eq!(draft.merged_into("Fix the build"), "Fix the build\n\nand the tests");
        assert_eq!(draft.merged_into("Fix the build\n\n"), "Fix the build\n\nand the tests");
    }

    #[test]
    fn a_tab_dropped_on_another_joins_its_group() {
        let b = bench((0..5).map(|_| Tab::with_pane(pane())).collect());
        let bench = b.id;
        let mut shed = Shed { benches: vec![b], ..Default::default() };
        let tabs = |shed: &Shed| shed.bench(bench).unwrap().tabs.iter().map(|t| (t.id, t.divider)).collect::<Vec<_>>();
        let all = tabs(&shed);
        let (a1, a2, b1, b2) = (all[1].0, all[2].0, all[3].0, all[4].0);
        // Groups: [first, a1, a2] [b1, b2]
        shed.tab_mut(b1).unwrap().divider = TabDivider::Space;
        // a1 dropped on b1 (moving right): first in b's group.
        assert!(shed.move_tab(a1, bench, 3));
        let now: Vec<_> = tabs(&shed).into_iter().skip(1).collect();
        assert_eq!(now, [(a2, TabDivider::None), (a1, TabDivider::Space), (b1, TabDivider::None), (b2, TabDivider::None)]);
        // a1 (first of its group) dropped back on a2 (moving left): b1 keeps the group going.
        assert!(shed.move_tab(a1, bench, 1));
        let now: Vec<_> = tabs(&shed).into_iter().skip(1).collect();
        assert_eq!(now, [(a1, TabDivider::None), (a2, TabDivider::None), (b1, TabDivider::Space), (b2, TabDivider::None)]);
        // b2 put after a2: last in a's group; before b1: first in b's.
        assert!(shed.move_tab_next_to(b2, a2, true));
        let now: Vec<_> = tabs(&shed).into_iter().skip(1).collect();
        assert_eq!(now, [(a1, TabDivider::None), (a2, TabDivider::None), (b2, TabDivider::None), (b1, TabDivider::Space)]);
        assert!(shed.move_tab_next_to(b2, b1, false));
        let now: Vec<_> = tabs(&shed).into_iter().skip(1).collect();
        assert_eq!(now, [(a1, TabDivider::None), (a2, TabDivider::None), (b2, TabDivider::Space), (b1, TabDivider::None)]);
    }

    fn pane() -> Pane {
        Pane::new(PaneId::new(), "/".into())
    }

    fn bench(tabs: Vec<Tab>) -> Bench {
        Bench {
            id: BenchId::new(),
            name: "main".into(),
            color: BenchColor::Blue,
            badge: None,
            cwd: "/".into(),
            active_tab: tabs[0].id,
            tabs,
            aliases: Aliases::new(),
            closing: false,
            started: false,
            launcher: "main".into(),
            window: None,
            links: Vec::new(),
        }
    }

    #[test]
    fn removing_last_pane_removes_tab_and_bench() {
        let p = pane();
        let pid = p.id;
        let mut shed = Shed { benches: vec![bench(vec![Tab::with_pane(p)])], ..Default::default() };
        assert!(shed.remove_pane(pid).is_some());
        assert!(shed.benches.is_empty());
    }

    #[test]
    fn project_tabs_keep_their_bench_without_panes() {
        let p = pane();
        let pid = p.id;
        let project = Tab::project("/srv/web".into());
        let tab = project.id;
        let mut shed = Shed { benches: vec![bench(vec![Tab::with_pane(p), project])], ..Default::default() };
        assert!(shed.remove_pane(pid).is_some());
        assert_eq!(shed.benches[0].tabs.len(), 1);
        assert_eq!(shed.benches[0].active_tab, tab);
        assert_eq!(shed.locate_pane(shed.benches[0].tabs[0].active_pane), None);
        assert_eq!(shed.benches[0].tabs[0].display_title(), "web");
        assert!(shed.close_paneless_tab(tab).is_some());
        assert!(shed.benches.is_empty());
    }

    #[test]
    fn closing_active_tab_activates_neighbour() {
        let tabs: Vec<Tab> = (0..3).map(|_| Tab::with_pane(pane())).collect();
        let ids: Vec<TabId> = tabs.iter().map(|t| t.id).collect();
        let mut b = bench(tabs);
        b.active_tab = ids[1];
        let mut shed = Shed { benches: vec![b], ..Default::default() };
        shed.take_tab(ids[1]);
        assert_eq!(shed.benches[0].active_tab, ids[2]);
        shed.take_tab(ids[2]);
        assert_eq!(shed.benches[0].active_tab, ids[0]);
    }

    #[test]
    fn moving_the_only_tab_removes_its_bench() {
        let a = bench(vec![Tab::with_pane(pane())]);
        let b = bench(vec![Tab::with_pane(pane())]);
        let (tab, target) = (a.tabs[0].id, b.id);
        let mut shed = Shed { benches: vec![a, b], ..Default::default() };
        assert!(shed.move_tab(tab, target, 0));
        assert_eq!(shed.benches.len(), 1);
        assert_eq!(shed.benches[0].tabs[0].id, tab);
        assert_eq!(shed.benches[0].active_tab, tab);
    }

    #[test]
    fn split_and_collapse() {
        let (a, b) = (PaneId::new(), PaneId::new());
        let mut node = PaneNode::Leaf(a);
        assert!(node.split(a, b, SplitDir::Right));
        assert_eq!(node.leaves(), vec![a, b]);
        assert_eq!(node.remove(a), Some(PaneNode::Leaf(b)));
    }

    #[test]
    fn set_ratio_by_path() {
        let (a, b, c) = (PaneId::new(), PaneId::new(), PaneId::new());
        let mut node = PaneNode::Leaf(a);
        node.split(a, b, SplitDir::Right);
        node.split(b, c, SplitDir::Down);
        assert!(node.set_ratio(&[true], 0.3));
        assert!(node.set_ratio(&[], 2.0));
        let PaneNode::Split { ratio, second, .. } = &node else { panic!() };
        assert_eq!(*ratio, 0.95);
        assert!(matches!(**second, PaneNode::Split { ratio, .. } if ratio == 0.3));
        assert!(!node.set_ratio(&[false], 0.5), "a leaf has no ratio");
    }

    #[test]
    fn neighbours_follow_geometry() {
        // a | b
        // --+--
        //   c (under b)
        let (a, b, c) = (PaneId::new(), PaneId::new(), PaneId::new());
        let mut node = PaneNode::Leaf(a);
        node.split(a, b, SplitDir::Right);
        node.split(b, c, SplitDir::Down);
        assert_eq!(node.neighbor(a, Direction::Right), Some(b));
        assert_eq!(node.neighbor(b, Direction::Down), Some(c));
        assert_eq!(node.neighbor(c, Direction::Up), Some(b));
        assert_eq!(node.neighbor(c, Direction::Left), Some(a));
        assert_eq!(node.neighbor(a, Direction::Left), None);
    }

    #[test]
    fn ai_sessions_running_here_and_in_the_background() {
        let mut ai = AiTab { session: Some("a".into()), ..Default::default() };
        assert_eq!(ai.running().count(), 0);
        assert_eq!(ai.overall_activity(), AiActivity::Idle);
        ai.away.push(AiAway { session: "b".into(), activity: AiActivity::Waiting });
        assert_eq!(ai.running().collect::<Vec<_>>(), [("b", AiActivity::Waiting)]);
        ai.activity = AiActivity::Working;
        assert_eq!(ai.running().collect::<Vec<_>>(), [("a", AiActivity::Working), ("b", AiActivity::Waiting)]);
        // Waiting for you anywhere is what the tab says.
        assert_eq!(ai.overall_activity(), AiActivity::Waiting);
        ai.away.clear();
        assert_eq!(ai.overall_activity(), AiActivity::Working);
    }

    #[test]
    fn display_title_prefers_name_then_title_then_tool() {
        let mut tab = Tab::with_pane(Pane::new(PaneId::new(), "/home/me/code/api".into()));
        assert_eq!(tab.display_title(), "api");
        tab.panes[0].tool = Some(Tool { argv: vec!["/usr/bin/nvim".into(), "main.rs".into()] });
        assert_eq!(tab.display_title(), "nvim main.rs");
        tab.panes[0].title = Some("vim: main.rs".into());
        assert_eq!(tab.display_title(), "vim: main.rs");
        tab.name = Some("api".into());
        assert_eq!(tab.display_title(), "api");
    }
}
