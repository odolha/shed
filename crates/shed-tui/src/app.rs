//! The terminal interface's state: what the keeper broadcasts, which
//! screen is shown, every visible pane's emulator, AI and project tabs'
//! state, and what is open over the screen. It handles events (frames from
//! the keeper, keys, mouse, results of background work) and draws into a
//! ratatui frame, so tests drive it without a terminal.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use shed_ai::Limits;
use shed_client::KeeperConn;
use shed_emu::Palette;
use shed_model::config::Config;
use shed_model::{Bench, BenchColor, BenchId, Direction, PaneId, Shed, SplitDir, Tab, TabId, TabKind};
use shed_proto::{BenchRef, ClientMsg, Frame as Wire, KeeperMsg, PegboardInfo, ShowTarget};

use crate::overlay::{Item, Overlay};
use crate::term::PaneEmu;
use crate::text::{fit, width};
use crate::theme::Colors;

/// How many of a bench's tabs keep their panes' screens, most recent first
/// (as the windows keep them); the others ask for them again when shown.
pub const KEPT_TABS: usize = 3;

/// How long a message stays on the status line.
const NOTICE_TIME: Duration = Duration::from_secs(6);

/// The prefix key: Ctrl+B, as in tmux.
pub const PREFIX: char = 'b';

/// What the app reacts to.
pub enum Event {
    Keeper(Box<Wire>),
    /// The keeper's connection closed.
    KeeperGone,
    Term(TermEvent),
    /// The result of work done on another thread, applied to the app.
    Apply(Box<dyn FnOnce(&mut App) + Send>),
    /// Once a second: clocks, timeouts, the benches list's refresh.
    Tick,
}

/// Where the logbook and the settings are; tests point them elsewhere.
#[derive(Clone, Debug)]
pub struct Options {
    pub config: Config,
    pub logbook: PathBuf,
    /// The terminal has a dark background.
    pub dark: bool,
    pub no_color: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            config: Config::load_from(&Config::path()).unwrap_or_default(),
            logbook: shed_logbook::default_path(),
            dark: !crate::theme::terminal_is_light(),
            no_color: crate::theme::no_color(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    /// Every bench, running and closed.
    Benches,
    /// The bench being worked in.
    Bench,
}

/// Where things were drawn, for the mouse.
#[derive(Default)]
pub struct Hits {
    pub tabs: Vec<(Rect, TabId)>,
    pub panes: Vec<(PaneId, Rect)>,
    /// The tab's own area (AI and project tabs).
    pub body: Rect,
}

pub struct App {
    pub(crate) conn: KeeperConn,
    pub(crate) tx: mpsc::Sender<Event>,
    pub tree: Shed,
    pub(crate) pegboard: PegboardInfo,
    pub(crate) limits: Option<Limits>,
    pub(crate) colors: Colors,
    pub(crate) palette: Palette,
    pub(crate) logbook_path: PathBuf,
    pub screen: Screen,
    /// The bench shown (attached: its panes' output comes here).
    pub(crate) bench: Option<BenchId>,
    /// The keeper has sent the attached bench's screens.
    pub(crate) attached: bool,
    /// The bench's tabs shown most recently, newest first.
    pub(crate) recent: Vec<TabId>,
    pub(crate) panes: HashMap<PaneId, PaneEmu>,
    pub(crate) ais: HashMap<TabId, crate::ai::AiView>,
    pub(crate) projects: HashMap<TabId, crate::project::ProjectView>,
    pub(crate) overlay: Option<Overlay>,
    pub(crate) benches: crate::benches::BenchesState,
    /// The prefix key was pressed: the next key is Shed's.
    pub(crate) prefix: bool,
    pub(crate) scroll: Option<crate::term::ScrollMode>,
    /// The focused pane fills the tab.
    pub(crate) zoomed: bool,
    pub(crate) notice: Option<(String, bool, Instant)>,
    pub(crate) keeper_lost: bool,
    /// Why the interface cannot go on (another version's keeper).
    pub fatal: Option<String>,
    pub quit: bool,
    pub dirty: bool,
    pub(crate) hits: Hits,
    /// A mouse button held in a pane that hears the mouse.
    pub(crate) mouse_down: Option<(PaneId, shed_emu::mouse::Button)>,
    /// The tab last shown, to save an AI prompt's unsent text on leaving it.
    pub(crate) last_tab: Option<TabId>,
    pub(crate) ticks: u64,
    /// The focused pane's cursor shape, as last drawn (`None`: no cursor there).
    pub cursor_shape: Option<shed_emu::CursorShape>,
    /// A bookmark being gone to, until its tab is there to show it.
    pub(crate) pending_bookmark: Option<(shed_model::Bookmark, Instant)>,
    /// When the keeper was last told someone is here (`UserActive`).
    told_active: Option<Instant>,
}

impl App {
    pub fn new(conn: KeeperConn, tx: mpsc::Sender<Event>, options: Options) -> App {
        let theme = crate::theme::theme(&options.config, options.dark);
        App {
            conn,
            tx,
            tree: Shed::default(),
            pegboard: PegboardInfo::default(),
            limits: None,
            colors: Colors::from_theme(&theme, options.no_color),
            palette: Palette::from_theme(&theme),
            logbook_path: options.logbook,
            screen: Screen::Benches,
            bench: None,
            attached: false,
            recent: Vec::new(),
            panes: HashMap::new(),
            ais: HashMap::new(),
            projects: HashMap::new(),
            overlay: None,
            benches: Default::default(),
            prefix: false,
            scroll: None,
            zoomed: false,
            notice: None,
            keeper_lost: false,
            fatal: None,
            quit: false,
            dirty: true,
            hits: Hits::default(),
            mouse_down: None,
            last_tab: None,
            ticks: 0,
            cursor_shape: None,
            pending_bookmark: None,
            told_active: None,
        }
    }

    /// Asks the keeper to show `target`; it answers by telling this client
    /// what to show (the benches, or a bench it opened or loaded).
    pub fn start(&mut self, target: ShowTarget) {
        self.conn.send_msg(ClientMsg::Show(target));
        self.conn.send_msg(ClientMsg::Pegboard);
    }

    /// Sends a message to the keeper.
    pub fn send(&self, msg: ClientMsg) {
        self.conn.send_msg(msg);
    }

    /// Runs `work` on another thread; what it returns is applied to the app.
    pub(crate) fn spawn<R>(&self, work: impl FnOnce() -> R + Send + 'static)
    where
        R: FnOnce(&mut App) + Send + 'static,
    {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let apply = work();
            let _ = tx.send(Event::Apply(Box::new(apply)));
        });
    }

    pub(crate) fn note(&mut self, text: impl Into<String>) {
        self.notice = Some((text.into(), false, Instant::now()));
    }

    pub(crate) fn error(&mut self, text: impl Into<String>) {
        self.notice = Some((text.into(), true, Instant::now()));
    }

    // ── What is shown ───────────────────────────────────────────────────

    pub fn current_bench(&self) -> Option<&Bench> {
        self.bench.and_then(|b| self.tree.bench(b))
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.current_bench().and_then(Bench::active_tab)
    }

    /// The pane keys go to: the active tab's active pane.
    pub fn focused_pane(&self) -> Option<PaneId> {
        self.active_tab().filter(|t| t.has_panes()).map(|t| t.active_pane)
    }

    /// A pane's screen as text, if this client keeps it.
    pub fn pane_text(&self, pane: PaneId) -> Option<String> {
        use shed_emu::Emulator;
        self.panes.get(&pane).map(|p| p.emu.dump_text())
    }

    /// The folder to start things in: the focused pane's, or the bench's.
    pub(crate) fn here(&self) -> PathBuf {
        let tab = self.active_tab();
        if let Some(p) = tab.and_then(|t| t.active()) {
            return p.cwd.clone();
        }
        if let Some(root) = tab.and_then(|t| t.project_info()).map(|p| p.root.clone()) {
            return root;
        }
        if let Some(folder) = tab.and_then(|t| t.ai_info()).map(|a| a.folder.clone()) {
            return folder;
        }
        self.current_bench().map(|b| b.cwd.clone()).unwrap_or_else(|| std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into()))
    }

    /// Shows a running bench here: attaches to it (letting go of the one shown).
    pub fn open_bench(&mut self, bench: BenchId) {
        if self.bench != Some(bench) {
            if let Some(old) = self.bench.take() {
                self.leave_tab();
                self.send(ClientMsg::Detach { bench: old });
            }
            self.panes.clear();
            self.ais.clear();
            self.projects.clear();
            self.recent.clear();
            self.scroll = None;
            self.zoomed = false;
            self.bench = Some(bench);
            self.attached = false;
            self.send(ClientMsg::Attach { bench: Some(bench) });
        }
        self.screen = Screen::Bench;
        self.sync();
    }

    pub fn show_benches(&mut self) {
        self.screen = Screen::Benches;
        self.benches.select_current(self.bench);
        self.send(ClientMsg::Pegboard);
    }

    /// Brings emulators and tab views in line with the tree.
    pub(crate) fn sync(&mut self) {
        let Some(bench) = self.current_bench().cloned() else {
            if self.bench.take().is_some() {
                // Closed (from here or elsewhere): back to the list.
                self.panes.clear();
                self.ais.clear();
                self.projects.clear();
                self.recent.clear();
                self.scroll = None;
                self.screen = Screen::Benches;
                self.send(ClientMsg::Pegboard);
            }
            return;
        };
        let active = bench.active_tab;
        if self.last_tab != Some(active) {
            self.leave_tab();
            self.last_tab = Some(active);
            self.zoomed = false;
            if self.scroll.as_ref().is_some_and(|s| bench.tabs.iter().find(|t| t.id == active).is_none_or(|t| t.pane(s.pane).is_none())) {
                self.scroll = None;
            }
        }
        self.recent.retain(|t| bench.tabs.iter().any(|tab| tab.id == *t));
        self.recent.retain(|t| *t != active);
        self.recent.insert(0, active);
        self.recent.truncate(KEPT_TABS);

        let shown: HashSet<PaneId> =
            self.recent.iter().filter_map(|t| bench.tabs.iter().find(|tab| tab.id == *t)).flat_map(|t| &t.panes).map(|p| p.id).collect();
        self.panes.retain(|id, _| shown.contains(id));
        for id in shown {
            if !self.panes.contains_key(&id) {
                let (cols, rows) = self.tree.pane(id).map_or((80, 24), |p| (p.cols, p.rows));
                self.panes.insert(id, PaneEmu::new(cols, rows, self.palette.clone()));
                // An attaching bench's screens come with `Attached`.
                if self.attached {
                    self.send(ClientMsg::Replay { pane: id });
                }
            }
        }
        if let Some(s) = &self.scroll
            && !self.panes.contains_key(&s.pane)
        {
            self.scroll = None;
        }

        // AI tabs: the conversation of the one shown, kept for its bench.
        self.ais.retain(|t, _| bench.tabs.iter().any(|tab| tab.id == *t && tab.ai_info().is_some()));
        self.projects.retain(|t, _| bench.tabs.iter().any(|tab| tab.id == *t && tab.project_info().is_some()));
        if let Some(tab) = bench.tabs.iter().find(|t| t.id == active) {
            match &tab.kind {
                TabKind::Ai(ai) if !self.ais.contains_key(&tab.id) => {
                    self.ais.insert(tab.id, crate::ai::AiView::new(ai));
                    self.send(ClientMsg::AiWatch { tab: tab.id });
                }
                TabKind::Project(p) if !self.projects.contains_key(&tab.id) => {
                    let view = crate::project::ProjectView::new(p.root.clone());
                    self.projects.insert(tab.id, view);
                    self.project_load(tab.id);
                }
                _ => {}
            }
        }
        self.settle_bookmark();
    }

    /// Leaving the tab shown: an AI prompt's text is kept with its tab.
    fn leave_tab(&mut self) {
        let Some(tab) = self.last_tab else { return };
        if let Some(view) = self.ais.get(&tab) {
            let unsent = view.input.text.clone();
            if self.tree.tab(tab).and_then(Tab::ai_info).is_some_and(|a| a.unsent != unsent) {
                self.send(ClientMsg::AiChange { tab, change: shed_proto::AiChange::Unsent(unsent) });
            }
        }
    }

    // ── Events ──────────────────────────────────────────────────────────

    pub fn handle(&mut self, event: Event) {
        match event {
            Event::Keeper(frame) => self.on_frame(*frame),
            Event::KeeperGone => {
                self.keeper_lost = true;
                self.dirty = true;
            }
            Event::Term(ev) => {
                self.dirty = true;
                // Someone is at the workspace: its notifications stay off
                // the phone (told at most every few seconds).
                if matches!(ev, TermEvent::Key(_) | TermEvent::Mouse(_) | TermEvent::Paste(_))
                    && self.told_active.is_none_or(|t| t.elapsed() >= Duration::from_secs(15))
                {
                    self.told_active = Some(Instant::now());
                    self.conn.send_msg(ClientMsg::UserActive);
                }
                match ev {
                    TermEvent::Key(key) => self.on_key(key),
                    TermEvent::Mouse(m) => self.on_mouse(m),
                    TermEvent::Paste(text) => self.on_paste(&text),
                    _ => {}
                }
            }
            Event::Apply(apply) => {
                apply(self);
                self.dirty = true;
            }
            Event::Tick => self.on_tick(),
        }
    }

    fn on_tick(&mut self) {
        self.ticks += 1;
        if self.notice.as_ref().is_some_and(|(_, _, at)| at.elapsed() > NOTICE_TIME) {
            self.notice = None;
            self.dirty = true;
        }
        if self.screen == Screen::Benches && self.ticks.is_multiple_of(3) {
            self.send(ClientMsg::Pegboard);
        }
        // Clocks of turns under way, runs and timers; synchronized updates past their time.
        if self.ais.values().any(|a| a.status.state != shed_ai::State::Idle || a.counting()) {
            self.dirty = true;
        }
        for p in self.panes.values_mut() {
            if p.tick() {
                self.dirty = true;
            }
        }
    }

    fn on_frame(&mut self, frame: Wire) {
        match frame {
            Wire::Output { pane, bytes } => {
                if let Some(p) = self.panes.get_mut(&pane) {
                    p.feed(&bytes);
                    if self.active_tab().is_some_and(|t| t.pane(pane).is_some()) {
                        self.dirty = true;
                    }
                }
            }
            Wire::Keeper(msg) => {
                self.dirty = true;
                self.on_msg(msg);
            }
            Wire::Client(_) | Wire::Input { .. } => {}
        }
    }

    fn on_msg(&mut self, msg: KeeperMsg) {
        match msg {
            KeeperMsg::TreeChanged(tree) => {
                self.tree = tree;
                self.sync();
            }
            KeeperMsg::Show { bench } => self.open_bench(bench),
            KeeperMsg::ShowShed => self.show_benches(),
            KeeperMsg::Attached { bench } => {
                if self.bench == Some(bench) {
                    self.attached = true;
                }
            }
            KeeperMsg::Replay { pane, cols, rows, bytes } => {
                if let Some(p) = self.panes.get_mut(&pane) {
                    p.restore(cols, rows, &bytes);
                }
            }
            KeeperMsg::Pegboard(info) => self.pegboard = info,
            KeeperMsg::Error(e) => {
                if shed_client::keeper_conn::is_other_version(&e) {
                    self.fatal = Some(format!("{e}. Run `shed stop` to restart it (benches are saved first)."));
                    self.quit = true;
                } else {
                    self.error(e);
                }
            }
            KeeperMsg::Done(text) => self.note(text),
            KeeperMsg::AiUpdate { tab, from, entries, status } => {
                if let Some(view) = self.ais.get_mut(&tab) {
                    view.update(from as usize, entries, status);
                }
                self.settle_bookmark();
            }
            KeeperMsg::AiEntries { tab, entries, status } => {
                if let Some(view) = self.ais.get_mut(&tab) {
                    view.replace(entries, status);
                }
            }
            KeeperMsg::AiInfo { tab, commands, models } => {
                if let Some(view) = self.ais.get_mut(&tab) {
                    view.commands = commands;
                    view.models = models;
                }
            }
            KeeperMsg::AiLimits(limits) => self.limits = Some(limits),
            KeeperMsg::Welcome { .. } | KeeperMsg::PaneExited { .. } | KeeperMsg::Status(_) | KeeperMsg::Hide { .. } | KeeperMsg::ShowInWorkspace { .. } | KeeperMsg::Presence { .. } => {}
        }
    }

    fn on_key(&mut self, ev: KeyEvent) {
        // Releases only matter to panes whose program asked (kitty protocol).
        if ev.kind == KeyEventKind::Release {
            if self.overlay.is_none() && !self.prefix && self.screen == Screen::Bench && self.scroll.is_none() {
                self.pane_key(ev);
            }
            return;
        }
        if self.keeper_lost {
            if matches!(ev.code, KeyCode::Char('q') | KeyCode::Enter | KeyCode::Esc) {
                self.quit = true;
            }
            return;
        }
        if self.prefix {
            self.prefix = false;
            self.prefix_key(ev);
            return;
        }
        if ev.code == KeyCode::Char(PREFIX) && ev.modifiers == KeyModifiers::CONTROL {
            self.prefix = true;
            return;
        }
        if self.overlay.is_some() {
            self.overlay_key(ev);
            return;
        }
        match self.screen {
            Screen::Benches => self.benches_key(ev),
            Screen::Bench => {
                if self.scroll.is_some() {
                    self.scroll_key(ev);
                    return;
                }
                let Some(tab) = self.active_tab() else { return };
                let id = tab.id;
                match tab.kind {
                    TabKind::Ai(_) => self.ai_key(id, ev),
                    TabKind::Project(_) => self.project_key(id, ev),
                    TabKind::Terminal | TabKind::Remote(_) => self.pane_key(ev),
                }
            }
        }
    }

    fn on_paste(&mut self, text: &str) {
        if let Some(Overlay::Input(a)) = &mut self.overlay {
            a.input.insert(text);
            return;
        }
        if let Some(Overlay::Logbook(s)) = &mut self.overlay {
            s.input.insert(text);
            return;
        }
        if self.overlay.is_some() || self.screen != Screen::Bench {
            return;
        }
        let Some(tab) = self.active_tab() else { return };
        let id = tab.id;
        match tab.kind {
            TabKind::Ai(_) => {
                if let Some(v) = self.ais.get_mut(&id) {
                    v.input.insert(text);
                }
            }
            TabKind::Project(_) => self.project_paste(id, text),
            TabKind::Terminal | TabKind::Remote(_) => self.paste_into_pane(text),
        }
    }

    /// Text into the focused pane as a paste (bracketed if its program asked).
    pub(crate) fn paste_into_pane(&mut self, text: &str) {
        let Some(pane) = self.focused_pane() else { return };
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let bytes = shed_emu::keys::encode_paste(text, p.modes());
        p.scroll_to_end();
        self.conn.send(Wire::Input { pane, bytes });
    }

    // ── The prefix key ──────────────────────────────────────────────────

    fn prefix_key(&mut self, ev: KeyEvent) {
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        match ev.code {
            // The prefix twice: the key itself, for the program in the pane.
            KeyCode::Char(PREFIX) if ctrl => {
                if let Some(pane) = self.focused_pane() {
                    self.conn.send(Wire::Input { pane, bytes: vec![PREFIX as u8 - b'a' + 1] });
                }
            }
            KeyCode::Char('d') => self.quit = true,
            KeyCode::Char('?') => self.help(),
            KeyCode::Char('s') | KeyCode::Char('w') => self.show_benches(),
            KeyCode::Char('r') => self.open_logbook(),
            KeyCode::Esc => {}
            KeyCode::Char('b') if self.screen == Screen::Bench && self.bench.is_some() => self.bookmarks_menu(),
            _ if self.screen != Screen::Bench || self.bench.is_none() => self.note("open a bench first (Enter on it)"),
            KeyCode::Char('c') => self.new_terminal_tab(self.here()),
            KeyCode::Char('t') => self.new_tab_menu(),
            KeyCode::Char('n') => self.step_tab(1),
            KeyCode::Char('p') => self.step_tab(-1),
            KeyCode::Char(c @ '1'..='9') => self.select_tab(c as usize - '1' as usize),
            KeyCode::Char('0') => self.select_tab(9),
            KeyCode::Char(',') => self.rename_tab(),
            KeyCode::Char('$') => self.rename_bench(),
            KeyCode::Char('<') => self.move_tab_by(-1),
            KeyCode::Char('>') => self.move_tab_by(1),
            KeyCode::Char('&') => self.close_tab(),
            KeyCode::Char('m') => self.tab_menu(),
            KeyCode::Char('%') => self.split(SplitDir::Right),
            KeyCode::Char('"') => self.split(SplitDir::Down),
            KeyCode::Char('x') => self.close_pane(),
            KeyCode::Char('z') => self.zoomed = !self.zoomed && self.active_tab().is_some_and(|t| t.panes.len() > 1),
            KeyCode::Char('o') => self.cycle_pane(),
            KeyCode::Char('[') => self.enter_scroll(),
            KeyCode::Char('e') => self.edit_in_editor(),
            KeyCode::Up => self.focus_dir(Direction::Up),
            KeyCode::Down => self.focus_dir(Direction::Down),
            KeyCode::Left => self.focus_dir(Direction::Left),
            KeyCode::Right => self.focus_dir(Direction::Right),
            _ => self.note("unknown key after Ctrl+B (Ctrl+B ? lists them)"),
        }
    }

    pub(crate) fn help(&mut self) {
        let c = &self.colors;
        let mut lines = Vec::new();
        for (section, keys) in crate::KEYS {
            lines.push(Line::styled(section.to_string(), c.accent().add_modifier(Modifier::BOLD)));
            for (k, what) in *keys {
                lines.push(Line::from(vec![Span::styled(format!("  {k:<18}"), Style::new().add_modifier(Modifier::BOLD)), Span::raw(what.to_string())]));
            }
            lines.push(Line::default());
        }
        self.info("Keys", lines);
    }

    // ── Tabs ────────────────────────────────────────────────────────────

    pub(crate) fn select_tab(&mut self, index: usize) {
        if let Some(tab) = self.current_bench().and_then(|b| b.tabs.get(index)).map(|t| t.id) {
            self.send(ClientMsg::FocusTab { tab });
        }
    }

    fn step_tab(&mut self, by: isize) {
        let Some(b) = self.current_bench() else { return };
        let n = b.tabs.len() as isize;
        if n == 0 {
            return;
        }
        let at = b.tab_index(b.active_tab).unwrap_or(0) as isize;
        let tab = b.tabs[(at + by).rem_euclid(n) as usize].id;
        self.send(ClientMsg::FocusTab { tab });
    }

    pub(crate) fn new_terminal_tab(&mut self, cwd: PathBuf) {
        if let Some(bench) = self.bench {
            self.send(ClientMsg::NewTab { bench, cwd: Some(cwd) });
        }
    }

    pub(crate) fn new_tab_menu(&mut self) {
        let items = vec![
            Item::new("Terminal…").hint("a shell in a folder"),
            Item::new("Project…").hint("files, search, Git"),
            Item::new("AI (Claude Code)…").hint("a conversation in a folder"),
            Item::new("Remote shell…").hint("a shell on a server, over ssh"),
            Item::new("Command…").hint("a terminal running a command"),
        ];
        self.menu("New Tab", items, |app, i| {
            let here = app.here();
            let root = shed_project::project_root(&here);
            match i {
                0 => app.ask_folder("Terminal in folder", &here, |app, dir| app.new_terminal_tab(dir)),
                1 => app.ask_folder("Project folder", &root, |app, dir| {
                    if let Some(bench) = app.bench {
                        app.send(ClientMsg::NewProjectTab { bench, root: dir });
                    }
                }),
                2 => app.ask_folder("Claude Code works in", &root, |app, folder| {
                    if let Some(bench) = app.bench {
                        app.send(ClientMsg::NewAiTab { bench, folder });
                    }
                }),
                3 => {
                    // Hosts remote tabs used (latest first), then ~/.ssh/config's.
                    let hosts = shed_client::remote::known_hosts(&shed_client::remote::recent_hosts(&app.tree));
                    let hint = match hosts.len() {
                        0 => "a host as ssh takes it: server, user@server".to_string(),
                        _ => format!("a host as ssh takes it; known: {}", hosts.iter().take(5).cloned().collect::<Vec<_>>().join(", ")),
                    };
                    app.ask_with_hint("Remote shell", hint, "", |app, host| {
                    let host = host.trim().to_string();
                    if let (Some(bench), false) = (app.bench, host.is_empty()) {
                        app.send(ClientMsg::NewRemoteTab { bench, host, join: None });
                    }
                    })
                }
                _ => app.ask("Command to run", "", move |app, command| {
                    if let (Some(bench), false) = (app.bench, command.trim().is_empty()) {
                        app.send(ClientMsg::NewCommandTab { bench, cwd: here, command });
                    }
                }),
            }
        });
    }

    /// Asks for a folder (`~` and relative paths allowed), starting at `start`.
    pub(crate) fn ask_folder(&mut self, title: &str, start: &Path, then: impl FnOnce(&mut App, PathBuf) + 'static) {
        let base = start.to_path_buf();
        self.ask(title, start.display().to_string(), move |app, typed| match resolve_folder(&base, &typed) {
            Some(dir) => then(app, dir),
            None => app.error(format!("{typed} is not a folder")),
        });
    }

    fn rename_tab(&mut self) {
        let Some(tab) = self.active_tab() else { return };
        let (id, current) = (tab.id, tab.name.clone().unwrap_or_else(|| tab.display_title()));
        self.ask_with_hint("Rename Tab", "empty: the title its program sets", current, move |app, name| {
            let name = Some(name.trim().to_string()).filter(|n| !n.is_empty());
            app.send(ClientMsg::RenameTab { tab: id, name });
        });
    }

    pub(crate) fn rename_bench(&mut self) {
        if let Some(b) = self.current_bench() {
            let r = BenchRef::Running(b.id);
            let name = b.name.clone();
            self.rename_bench_ref(r, name);
        }
    }

    pub(crate) fn rename_bench_ref(&mut self, bench: BenchRef, name: String) {
        self.ask("Rename Bench", name, move |app, name| {
            let name = name.trim().to_string();
            if !name.is_empty() {
                app.send(shed_client::BenchEdit::Rename(name).to_msg(&bench));
            }
        });
    }

    pub(crate) fn color_menu(&mut self, bench: BenchRef) {
        let items = BenchColor::NAMED.iter().map(|c| Item::new(c.name().unwrap_or("custom")).swatch(self.colors.ink(c.accent()))).collect();
        self.menu("Bench Colour", items, move |app, i| {
            app.send(shed_client::BenchEdit::Color(BenchColor::NAMED[i]).to_msg(&bench));
        });
    }

    fn move_tab_by(&mut self, by: isize) {
        let Some(b) = self.current_bench() else { return };
        let Some(at) = b.tab_index(b.active_tab) else { return };
        let to = (at as isize + by).clamp(0, b.tabs.len() as isize - 1) as usize;
        if to != at {
            self.send(ClientMsg::MoveTab { tab: b.active_tab, bench: b.id, index: to });
        }
    }

    fn move_tab_to_bench(&mut self) {
        let Some(b) = self.current_bench() else { return };
        let (tab, from) = (b.active_tab, b.id);
        let mut targets: Vec<(String, BenchRef, usize)> =
            self.tree.benches.iter().filter(|o| o.id != from).map(|o| (o.name.clone(), BenchRef::Running(o.id), o.tabs.len())).collect();
        targets.extend(self.pegboard.stored.iter().filter(|s| self.tree.bench(s.bench.id).is_none()).map(|s| (s.bench.name.clone(), BenchRef::Saved(s.bench.name.clone()), s.bench.tabs.len())));
        if targets.is_empty() {
            self.note("no other bench to move it to");
            return;
        }
        let items = targets.iter().map(|(n, r, _)| Item::new(n.clone()).hint(if matches!(r, BenchRef::Saved(_)) { "closed" } else { "" })).collect();
        self.menu("Move Tab to Bench", items, move |app, i| {
            let (_, to, index) = targets[i].clone();
            match to {
                BenchRef::Running(bench) => app.send(ClientMsg::MoveTab { tab, bench, index }),
                saved => app.send(ClientMsg::MoveTabAcross { tab, from: BenchRef::Running(from), to: saved, index }),
            }
        });
    }

    pub(crate) fn close_tab(&mut self) {
        let Some(tab) = self.active_tab() else { return };
        let (id, title) = (tab.id, tab.display_title());
        match &tab.kind {
            TabKind::Remote(r) => {
                let host = r.host.clone();
                let items = vec![Item::new("Let Go Here").hint("the shell keeps running on the server"), Item::new(format!("End the Shell on {host}…")).danger()];
                self.menu("Close Remote Tab", items, move |app, i| {
                    if i == 0 {
                        app.send(ClientMsg::CloseTab { tab: id });
                    } else {
                        app.confirm("End Remote Shell", format!("End the shell on {host}? What runs in it stops, for every PC."), "End", true, move |app| {
                            app.send(ClientMsg::EndRemoteTab { tab: id });
                        });
                    }
                });
            }
            TabKind::Ai(_) => self.confirm("Close Tab", format!("Close {title}? Claude Code stops; the session is kept."), "Close", true, move |app| {
                app.send(ClientMsg::CloseTab { tab: id })
            }),
            TabKind::Project(_) => self.send(ClientMsg::CloseTab { tab: id }),
            TabKind::Terminal => self.confirm("Close Tab", format!("Close {title}? What runs in it stops."), "Close", true, move |app| {
                app.send(ClientMsg::CloseTab { tab: id })
            }),
        }
    }

    /// What can be done with the tab shown (and its bench).
    pub(crate) fn tab_menu(&mut self) {
        let Some(tab) = self.active_tab() else { return };
        let id = tab.id;
        let mut items: Vec<crate::overlay::TabAction> = Vec::new();
        match tab.kind {
            TabKind::Ai(_) => items.extend(crate::ai::menu_items()),
            TabKind::Project(_) => items.extend(crate::project::menu_items()),
            TabKind::Terminal | TabKind::Remote(_) => {
                if matches!(tab.kind, TabKind::Remote(_)) {
                    items.push((Item::new("Reconnect").hint("join the shell again now"), |app, tab| app.send(ClientMsg::ReconnectRemote { tab })));
                }
                items.push((Item::new("Split Right").hint("C-b %"), |app, _| app.split(SplitDir::Right)));
                items.push((Item::new("Split Down").hint("C-b \""), |app, _| app.split(SplitDir::Down)));
                items.push((Item::new("Close Pane…").hint("C-b x"), |app, _| app.close_pane()));
                items.push((Item::new("Scroll Back").hint("C-b ["), |app, _| app.enter_scroll()));
                items.push((Item::new("Search History").hint("C-b r"), |app, _| app.open_logbook()));
            }
        }
        items.push((Item::new("Bookmarks…").hint("C-b b"), |app, _| app.bookmarks_menu()));
        items.push((Item::new("New Tab…").hint("C-b t"), |app, _| app.new_tab_menu()));
        items.push((Item::new("Rename Tab…").hint("C-b ,"), |app, _| app.rename_tab()));
        items.push((Item::new("Move Tab Left").hint("C-b <"), |app, _| app.move_tab_by(-1)));
        items.push((Item::new("Move Tab Right").hint("C-b >"), |app, _| app.move_tab_by(1)));
        items.push((Item::new("Move Tab to Bench…"), |app, _| app.move_tab_to_bench()));
        items.push((Item::new("Close Tab…").hint("C-b &").danger(), |app, _| app.close_tab()));
        items.push((Item::new("Rename Bench…").hint("C-b $"), |app, _| app.rename_bench()));
        items.push((Item::new("Bench Colour…"), |app, _| {
            if let Some(b) = app.bench {
                app.color_menu(BenchRef::Running(b));
            }
        }));
        items.push((Item::new("Run Startup"), |app, _| {
            if let Some(bench) = app.bench {
                app.send(ClientMsg::RunStartup { bench });
            }
        }));
        items.push((Item::new("Benches").hint("C-b s"), |app, _| app.show_benches()));
        let (labels, actions): (Vec<Item>, Vec<_>) = items.into_iter().unzip();
        self.menu(tab_title(self.active_tab().unwrap()), labels, move |app, i| actions[i](app, id));
    }

    // ── Panes ───────────────────────────────────────────────────────────

    pub(crate) fn split(&mut self, dir: SplitDir) {
        let Some(pane) = self.focused_pane() else { return };
        let cwd = self.tree.pane(pane).map(|p| p.cwd.clone());
        self.zoomed = false;
        self.send(ClientMsg::Split { pane, dir, cwd });
    }

    pub(crate) fn close_pane(&mut self) {
        let Some(pane) = self.focused_pane() else { return };
        let what = self.tree.pane(pane).and_then(|p| p.tool.as_ref()).map(|t| t.display());
        let text = match what {
            Some(tool) => format!("Close this pane? {tool} stops."),
            None => "Close this pane?".to_string(),
        };
        self.confirm("Close Pane", text, "Close", true, move |app| app.send(ClientMsg::ClosePane { pane }));
    }

    fn focus_dir(&mut self, dir: Direction) {
        let Some(tab) = self.active_tab() else { return };
        if let Some(next) = tab.layout.neighbor(tab.active_pane, dir) {
            self.send(ClientMsg::FocusPane { pane: next });
        }
    }

    fn cycle_pane(&mut self) {
        let Some(tab) = self.active_tab() else { return };
        let leaves = tab.layout.leaves();
        if let Some(at) = leaves.iter().position(|p| *p == tab.active_pane) {
            let next = leaves[(at + 1) % leaves.len()];
            self.send(ClientMsg::FocusPane { pane: next });
        }
    }

    /// `$EDITOR` on a file (the project tab's, or one asked for), in a new
    /// terminal tab of the bench.
    pub(crate) fn edit_in_editor(&mut self) {
        let tab = self.active_tab().map(|t| t.id);
        if let Some(path) = tab.and_then(|t| self.projects.get(&t)).and_then(|p| p.editable_path()) {
            self.open_editor(&path);
            return;
        }
        let here = self.here();
        self.ask("Edit File", format!("{}/", here.display()), move |app, typed| {
            let path = here.join(expand_home(typed.trim()));
            app.open_editor(&path);
        });
    }

    pub(crate) fn open_editor(&mut self, path: &Path) {
        let Some(bench) = self.bench else { return };
        let cwd = path.parent().filter(|p| p.is_dir()).map(Path::to_path_buf).unwrap_or_else(|| self.here());
        let command = format!("${{EDITOR:-vi}} {}", shell_quote(&path.to_string_lossy()));
        self.send(ClientMsg::NewCommandTab { bench, cwd, command });
    }

    // ── Drawing ─────────────────────────────────────────────────────────

    pub fn render(&mut self, f: &mut Frame) {
        self.dirty = false;
        let area = f.area();
        f.render_widget(Block::default().style(self.colors.base()), area);
        if area.height < 3 || area.width < 10 {
            f.render_widget(Paragraph::new("Shed: too small"), area);
            return;
        }
        let body = Rect { height: area.height - 1, ..area };
        let status = Rect { y: area.y + area.height - 1, height: 1, ..area };
        self.hits = Hits::default();
        match (self.screen, self.current_bench().is_some()) {
            (Screen::Bench, true) => self.render_bench(f, body),
            _ => self.render_benches(f, body),
        }
        self.render_status(f, status);
        if self.overlay.is_some() {
            self.render_overlay(f, body);
        }
        if self.keeper_lost {
            let r = crate::text::centered(body, 50, 5);
            let block = crate::overlay::frame_block(&self.colors, "Keeper Gone", true);
            let inner = block.inner(r);
            f.render_widget(ratatui::widgets::Clear, r);
            f.render_widget(block, r);
            f.render_widget(Paragraph::new(vec![Line::raw("The keeper stopped or restarted."), Line::styled("Press q to leave, then run shed again.", self.colors.dim())]), inner);
        }
    }

    fn render_bench(&mut self, f: &mut Frame, area: Rect) {
        let bar = Rect { height: 1, ..area };
        let body = Rect { y: area.y + 1, height: area.height - 1, ..area };
        self.render_tab_bar(f, bar);
        self.hits.body = body;
        let Some(tab) = self.active_tab().cloned() else { return };
        match &tab.kind {
            TabKind::Ai(_) => self.render_ai(f, body, tab.id),
            TabKind::Project(_) => self.render_project(f, body, tab.id),
            TabKind::Terminal | TabKind::Remote(_) => self.render_panes(f, body, &tab),
        }
    }

    fn render_tab_bar(&mut self, f: &mut Frame, area: Rect) {
        let Some(bench) = self.current_bench().cloned() else { return };
        let c = self.colors.clone();
        let accent = bench.color.accent();
        let buf = f.buffer_mut();
        buf.set_style(area, Style::new().bg(c.c(c.tab_hover)));
        let mut x = area.x;
        let end = area.x + area.width;
        let name = format!(" {} ", fit(&bench.name, 20));
        buf.set_string(x, area.y, &name, c.chip(accent));
        x += width(&name) as u16 + 1;
        let limits = self.limits_text();
        let right = end.saturating_sub(width(&limits) as u16 + 1);
        for (i, tab) in bench.tabs.iter().enumerate() {
            let active = tab.id == bench.active_tab;
            let mark = tab_mark(tab, self.ais.get(&tab.id).map(|a| a.status.state));
            let label = format!(" {}:{}{} ", i + 1, mark, fit(&tab.display_title(), 24));
            let w = width(&label) as u16;
            if x + w > right {
                buf.set_string(x, area.y, "…", c.dim());
                break;
            }
            let style = if active {
                Style::new().fg(c.c(c.ink(accent))).bg(c.c(c.tab_active)).add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(c.c(c.dim)).bg(c.c(c.tab_hover))
            };
            let style = if c.no_color && active { style.add_modifier(Modifier::REVERSED) } else { style };
            buf.set_string(x, area.y, &label, style);
            self.hits.tabs.push((Rect::new(x, area.y, w, 1), tab.id));
            x += w;
        }
        if !limits.is_empty() && right > x {
            buf.set_string(right, area.y, &limits, c.dim());
        }
    }

    /// The account's usage limits, short: "5h 40% · wk 12%".
    pub(crate) fn limits_text(&self) -> String {
        let Some(l) = self.limits.and_then(|l| shed_ai::still_current(l, now())) else { return String::new() };
        let mut parts = Vec::new();
        if let Some(w) = l.five_hour {
            parts.push(format!("5h {}", bar(w.used, 5)));
        }
        if let Some(w) = l.week {
            parts.push(format!("wk {}", bar(w.used, 5)));
        }
        parts.join(" ")
    }

    fn render_status(&mut self, f: &mut Frame, area: Rect) {
        let c = self.colors.clone();
        let mut left: Vec<Span> = Vec::new();
        match self.current_bench() {
            Some(b) => left.push(Span::styled(format!(" {} ", b.name), c.chip(b.color.accent()))),
            None => left.push(Span::styled(" Shed ", c.chip(c.accent))),
        }
        left.push(Span::raw(" "));
        if self.prefix {
            left.push(Span::styled(" C-b ", c.chip(c.accent)));
            left.push(Span::styled(" waiting for a key (? lists them) ", c.dim()));
        } else if let Some(s) = &self.scroll {
            left.push(Span::styled(" SCROLL ", c.chip(c.accent)));
            left.push(Span::styled(format!(" {} ", s.describe(self.panes.get(&s.pane))), c.dim()));
        } else if self.zoomed {
            left.push(Span::styled(" ZOOM ", c.chip(c.accent)));
        }
        if let Some((text, error, _)) = &self.notice {
            left.push(Span::raw(" "));
            left.push(Span::styled(text.clone(), if *error { c.danger() } else { c.fg(c.text) }));
        }
        let keeper = if self.keeper_lost { Span::styled("● keeper gone ", c.danger()) } else { Span::styled("● keeper ", c.added()) };
        let help = Span::styled("C-b ? keys ", c.dim());
        let right_w = (width(&keeper.content) + width(&help.content)) as u16;
        let line = Line::from(left);
        f.render_widget(Paragraph::new(line).style(Style::new().bg(c.c(c.tab_hover)).fg(c.c(c.text))), area);
        if area.width > right_w + 20 {
            let r = Rect { x: area.x + area.width - right_w, width: right_w, ..area };
            f.render_widget(Paragraph::new(Line::from(vec![help, keeper])).style(Style::new().bg(c.c(c.tab_hover))), r);
        }
    }
}

/// A tab's state at a glance: an AI tab working (●), waiting for you (!) or
/// on work it started in the background (◷).
fn tab_mark(tab: &Tab, state: Option<shed_ai::State>) -> &'static str {
    let ai = tab.ai_info().map(|a| a.overall_activity());
    match (state, ai) {
        (Some(shed_ai::State::Waiting), _) | (_, Some(shed_model::AiActivity::Waiting)) => "! ",
        (Some(shed_ai::State::Working), _) | (_, Some(shed_model::AiActivity::Working)) => "● ",
        (_, Some(shed_model::AiActivity::Background(_))) => "◷ ",
        _ => match tab.kind {
            TabKind::Project(_) => "▤ ",
            TabKind::Ai(_) => "✻ ",
            TabKind::Remote(_) => "⇄ ",
            TabKind::Terminal => "",
        },
    }
}

pub(crate) fn tab_title(tab: &Tab) -> String {
    tab.display_title()
}

/// A usage bar: "▓▓░░░ 40%".
pub(crate) fn bar(used: f32, cells: usize) -> String {
    let used = used.clamp(0.0, 1.0);
    let full = (used * cells as f32).round() as usize;
    format!("{}{} {:.0}%", "▓".repeat(full), "░".repeat(cells - full), used * 100.0)
}

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// `~/x` as a path under home.
pub(crate) fn expand_home(s: &str) -> PathBuf {
    match s.strip_prefix("~") {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
            home.join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(s),
    }
}

/// A typed folder, relative to `base`, if it is one.
pub(crate) fn resolve_folder(base: &Path, typed: &str) -> Option<PathBuf> {
    let typed = typed.trim();
    let path = if typed.is_empty() { base.to_path_buf() } else { base.join(expand_home(typed)) };
    let path = std::fs::canonicalize(&path).ok()?;
    path.is_dir().then_some(path)
}

/// `s` quoted for a POSIX shell.
pub(crate) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_and_quoting() {
        assert_eq!(bar(0.4, 5), "▓▓░░░ 40%");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        assert_eq!(resolve_folder(dir.path(), "sub"), Some(std::fs::canonicalize(dir.path().join("sub")).unwrap()));
        assert_eq!(resolve_folder(dir.path(), "nope"), None);
    }
}
