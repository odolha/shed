//! The app's view of the keeper: the tree, one window per bench, and the
//! terminal view of every pane those windows show.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui::{AppContext, AsyncApp, Context, Entity, FocusHandle, TitlebarOptions, WindowHandle, WindowOptions, px, size};
use shed_model::config::{Config, FileWatch};
use shed_desktop::{BenchIdentity, Desktop};
use shed_model::{Bench, BenchColor, BenchId, PaneId, Shed, Tab, TabId};
use shed_proto::{BenchRef, ClientMsg, Frame, KeeperMsg};
use tracing::{error, warn};

use crate::ai_view::{AiLimits, AiView};
use crate::bench_window::BenchWindow;
use shed_client::keeper_conn::KeeperConn;
use crate::pegboard::PegboardData;
use crate::project_view::ProjectView;
use crate::shed_window::ShedWindow;
use crate::terminal_view::TerminalView;
use crate::settings::Settings;
use crate::window_place::{PlaceTracker, Placed, open_bounds};

/// How long the receive loop may run before letting a frame be drawn.
const FRAME_BUDGET: Duration = Duration::from_millis(8);

/// Tabs per open bench whose panes keep a view (and its scrollback) in this
/// process: the one shown and the last ones shown before it. Others get a
/// view, filled by a replay from the keeper, when they are shown again.
const KEPT_TABS: usize = 3;

pub struct AppState {
    pub conn: KeeperConn,
    pub tree: Shed,
    pub settings: Settings,
    pub panes: HashMap<PaneId, Entity<TerminalView>>,
    /// The view of every project tab of an open bench.
    pub projects: HashMap<TabId, Entity<ProjectView>>,
    /// A file to show in the project tab being opened for it.
    /// (With `true`: shown in its Files too.)
    pending_reveal: Option<(BenchId, std::path::PathBuf, crate::ai_text::Place, bool)>,
    /// The view of every AI tab of an open bench.
    pub ai_views: HashMap<TabId, Entity<AiView>>,
    /// Per open bench, the tabs shown most recently, newest first.
    recent_tabs: HashMap<BenchId, Vec<TabId>>,
    windows: HashMap<BenchId, WindowHandle<BenchWindow>>,
    /// The Shed window, once open.
    shed_window: Option<WindowHandle<ShedWindow>>,
    /// The keeper's last complaint, shown in the Shed window until dismissed.
    pub notice: Option<String>,
    opening_shed: bool,
    /// Benches asked for with `Attach`, until their window is open.
    pending: HashSet<BenchId>,
    /// Windows being opened.
    opening: HashSet<BenchId>,
    desktop: Desktop,
    /// The identity last written for each bench, to rewrite only on change.
    identities: HashMap<BenchId, Identity>,
}

#[derive(Clone, PartialEq)]
struct Identity {
    launcher: String,
    name: String,
    color: BenchColor,
    badge: Option<String>,
}

impl Identity {
    fn of(bench: &Bench, badges: bool) -> Self {
        let badge = badges.then(|| bench.badge.clone().unwrap_or_else(|| initials(&bench.name)));
        Self { launcher: bench.launcher.clone(), name: bench.name.clone(), color: bench.color, badge }
    }
}

/// One or two letters for a bench's badge: `web-site` → `WS`, `backend` → `B`.
fn initials(name: &str) -> String {
    name.split(|c: char| !c.is_alphanumeric()).filter_map(|w| w.chars().next()).take(2).collect::<String>().to_uppercase()
}

impl AppState {
    pub fn new(conn: KeeperConn, settings: Settings) -> Self {
        crate::theme::set_ui_scale(settings.font.size);
        Self {
            conn,
            tree: Shed::default(),
            settings,
            panes: HashMap::new(),
            projects: HashMap::new(),
            pending_reveal: None,
            ai_views: HashMap::new(),
            recent_tabs: HashMap::new(),
            windows: HashMap::new(),
            shed_window: None,
            notice: None,
            opening_shed: false,
            pending: HashSet::new(),
            opening: HashSet::new(),
            desktop: Desktop::new(shed_model::paths::data_home()),
            identities: HashMap::new(),
        }
    }

    /// Reads frames from the keeper until it goes away.
    pub async fn receive(state: Entity<AppState>, rx: async_channel::Receiver<Frame>, cx: &mut AsyncApp) {
        while let Ok(first) = rx.recv().await {
            let started = Instant::now();
            state.update(cx, |state, cx| {
                state.handle(first, cx);
                // Drain what is already queued, but not for so long that frames stall.
                while started.elapsed() < FRAME_BUDGET {
                    match rx.try_recv() {
                        Ok(frame) => state.handle(frame, cx),
                        Err(_) => break,
                    }
                }
            });
            if !rx.is_empty() {
                cx.background_executor().timer(Duration::ZERO).await;
            }
        }
        error!("lost the connection to the keeper");
        cx.update(|cx| cx.quit());
    }

    fn handle(&mut self, frame: Frame, cx: &mut Context<Self>) {
        match frame {
            Frame::Output { pane, bytes } => {
                if let Some(view) = self.panes.get(&pane) {
                    view.update(cx, |view, cx| view.feed(&bytes, cx));
                }
            }
            Frame::Keeper(KeeperMsg::TreeChanged(tree)) => {
                self.tree = tree;
                self.sync(cx);
            }
            Frame::Keeper(KeeperMsg::Show { bench }) => self.show(bench, cx),
            Frame::Keeper(KeeperMsg::ShowShed) => self.show_shed(cx),
            Frame::Keeper(KeeperMsg::Hide { bench }) => self.hide_bench(bench, cx),
            Frame::Keeper(KeeperMsg::Pegboard(info)) => {
                // A global, so only the pegboard redraws, not every window.
                if cx.try_global::<PegboardData>().is_none_or(|d| d.0 != info) {
                    cx.set_global(PegboardData(info));
                }
            }
            Frame::Keeper(KeeperMsg::Attached { bench }) => self.open_bench_window(bench, cx),
            Frame::Keeper(KeeperMsg::Replay { pane, cols, rows, bytes }) => {
                if let Some(view) = self.panes.get(&pane) {
                    // The marks that said so were before the replay.
                    let at_prompt = self.tree.locate_pane(pane).and_then(|(_, tab)| self.tree.tab(tab)).and_then(|t| t.panes.iter().find(|p| p.id == pane)).is_some_and(|p| p.tool.is_none());
                    view.update(cx, |view, cx| view.restore(cols, rows, &bytes, at_prompt, cx));
                }
            }
            Frame::Keeper(KeeperMsg::Error(e)) => {
                error!("keeper: {e}");
                self.notice = Some(e.clone());
                cx.notify();
                if e.starts_with("protocol mismatch") {
                    eprintln!("shed: {e}. Run `shed stop` to restart the keeper.");
                    cx.quit();
                }
            }
            Frame::Keeper(KeeperMsg::Welcome { .. } | KeeperMsg::PaneExited { .. } | KeeperMsg::Status(_) | KeeperMsg::Done(_)) => {}
            Frame::Keeper(KeeperMsg::AiUpdate { tab, from, entries, status }) => {
                let Some(view) = self.ai_views.get(&tab).cloned() else { return };
                let before = view.read(cx).state();
                view.update(cx, |v, cx| v.update(from as usize, entries, status, cx));
                self.follow_edits(&view, cx);
                let after = view.read(cx).state();
                if before != after && (after == shed_ai::State::Waiting || (after == shed_ai::State::Idle && before == shed_ai::State::Working)) {
                    self.tell_about_ai(tab, &view, after, cx);
                }
            }
            Frame::Keeper(KeeperMsg::AiInfo { tab, commands, models }) => {
                if let Some(view) = self.ai_views.get(&tab) {
                    view.update(cx, |v, cx| v.set_info(commands, models, cx));
                }
            }
            Frame::Keeper(KeeperMsg::AiLimits(limits)) => {
                cx.set_global(AiLimits(limits));
                // Bench windows draw them on their tab bar (they follow this state).
                cx.notify();
            }
            Frame::Client(_) | Frame::Input { .. } => warn!("unexpected frame from keeper"),
        }
    }

    /// Opens or raises a bench's window.
    pub fn show(&mut self, bench: BenchId, cx: &mut Context<Self>) {
        if let Some(handle) = self.windows.get(&bench) {
            handle.update(cx, |_, window, _| window.activate()).ok();
        } else if self.pending.insert(bench) {
            self.conn.send_msg(ClientMsg::Attach { bench: Some(bench) });
        }
    }

    /// Brings views and windows in line with the tree.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let gone: Vec<BenchId> = self.windows.keys().filter(|b| self.tree.bench(**b).is_none()).copied().collect();
        for bench in gone {
            if let Some(handle) = self.windows.remove(&bench) {
                handle.update(cx, |_, window, _| window.remove_window()).ok();
            }
        }

        // A view for every pane of the recent tabs of every open bench; none
        // for anything else.
        self.note_shown_tabs();
        let shown = self.shown_panes();
        let before = self.panes.len();
        self.panes.retain(|id, _| shown.contains(id));
        if self.panes.len() < before {
            release_freed_memory();
        }
        for id in shown {
            if !self.panes.contains_key(&id) {
                let view = self.new_view(id, cx);
                self.panes.insert(id, view);
                // A bench being opened gets its replays with `Attached`.
                if self.tree.locate_pane(id).is_some_and(|(bench, _)| self.windows.contains_key(&bench)) {
                    self.conn.send_msg(ClientMsg::Replay { pane: id });
                }
            }
        }

        for bench in &self.tree.benches {
            for tab in &bench.tabs {
                let here = Some(shed_logbook::Context { tab: tab.id, bench: bench.id });
                let accent = bench.color.accent();
                for pane in &tab.panes {
                    let Some(view) = self.panes.get(&pane.id) else { continue };
                    let v = view.read(cx);
                    if v.cwd != pane.cwd || v.here.map(|h| (h.tab, h.bench)) != Some((tab.id, bench.id)) || v.accent != accent || v.ai != pane.ai || v.held != pane.held {
                        view.update(cx, |view, cx| {
                            view.cwd = pane.cwd.clone();
                            view.here = here;
                            view.accent = accent;
                            view.ai = pane.ai.clone();
                            view.held = pane.held.clone();
                            cx.notify();
                        });
                    }
                    if view.read(cx).zoom != tab.zoom {
                        view.update(cx, |view, cx| view.set_zoom(tab.zoom, cx));
                    }
                }
            }
        }

        self.sync_projects(cx);
        self.sync_ai(cx);
        self.sync_identities(cx);
        let benches: Vec<BenchId> = self.windows.keys().copied().collect();
        for bench in benches {
            self.sync_window(bench, cx);
        }
        cx.notify();
    }

    /// A view for every project tab of an open bench (or one being
    /// opened), made as the bench first opens and kept while the tab is in
    /// the tree, so unsaved edits survive hiding the bench.
    fn sync_projects(&mut self, cx: &mut Context<Self>) {
        let open: HashSet<BenchId> = self.windows.keys().chain(&self.opening).copied().collect();
        let all: Vec<(BenchId, TabId, shed_model::Project, shed_model::Rgb, f32)> = self
            .tree
            .benches
            .iter()
            .flat_map(|b| b.tabs.iter().filter_map(|t| Some((b.id, t.id, t.project_info()?.clone(), b.color.accent(), t.zoom))))
            .collect();
        self.projects.retain(|id, _| all.iter().any(|w| w.1 == *id));
        for (bench, tab, project, accent, zoom) in all {
            match self.projects.get(&tab) {
                Some(view) => view.update(cx, |v, cx| {
                    (v.bench, v.accent) = (bench, accent);
                    v.set_project(&project, cx);
                    v.set_zoom(zoom, cx);
                }),
                None if open.contains(&bench) => {
                    let (conn, settings) = (self.conn.clone(), self.settings.clone());
                    let view = cx.new(|cx| {
                        let mut v = ProjectView::new(bench, tab, &project, accent, conn, settings, cx);
                        v.set_zoom(zoom, cx);
                        v
                    });
                    // A file asked for before this tab was there.
                    if let Some((b, path, at, files)) = self.pending_reveal.take() {
                        if b == bench && path.starts_with(&project.root) {
                            view.update(cx, |v, cx| v.show_path(path, at, files, cx));
                        } else {
                            self.pending_reveal = Some((b, path, at, files));
                        }
                    }
                    self.projects.insert(tab, view);
                }
                None => {}
            }
        }
    }

    /// A view for every AI tab of an open bench, kept while the tab is in
    /// the tree, following its settings and the bench's project folders.
    fn sync_ai(&mut self, cx: &mut Context<Self>) {
        let open: HashSet<BenchId> = self.windows.keys().chain(&self.opening).copied().collect();
        let all: Vec<(BenchId, TabId, shed_model::AiTab, shed_model::Rgb, Vec<std::path::PathBuf>, f32)> = self
            .tree
            .benches
            .iter()
            .flat_map(|b| {
                let projects: Vec<std::path::PathBuf> = b.tabs.iter().filter_map(|t| Some(t.project_info()?.root.clone())).collect();
                b.tabs.iter().filter_map(move |t| Some((b.id, t.id, t.ai_info()?.clone(), b.color.accent(), projects.clone(), t.zoom)))
            })
            .collect();
        self.ai_views.retain(|id, _| all.iter().any(|w| w.1 == *id));
        for (bench, tab, ai, accent, projects, zoom) in all {
            match self.ai_views.get(&tab) {
                Some(view) => view.update(cx, |v, cx| {
                    (v.bench, v.accent) = (bench, accent);
                    v.set_zoom(zoom, cx);
                    if v.projects != projects {
                        v.projects = projects;
                        cx.notify();
                    }
                    v.set_ai(ai, cx);
                }),
                None if open.contains(&bench) => {
                    let (conn, settings) = (self.conn.clone(), self.settings.clone());
                    let view = cx.new(|cx| {
                        let mut v = AiView::new(bench, tab, ai, accent, conn, settings, cx);
                        v.projects = projects;
                        v.set_zoom(zoom, cx);
                        v
                    });
                    self.ai_views.insert(tab, view);
                }
                None => {}
            }
        }
    }

    /// Files an AI tab finished editing open in their project tab, when it
    /// follows edits.
    fn follow_edits(&mut self, view: &Entity<AiView>, cx: &mut Context<Self>) {
        let (bench, follow) = { let v = view.read(cx); (v.bench, v.ai.follow) };
        let edits = view.read(cx).finished_edits();
        let new: Vec<(String, String)> = edits.into_iter().filter(|(id, _)| !view.read(cx).followed.contains(id)).collect();
        if new.is_empty() {
            return;
        }
        view.update(cx, |v, _| v.followed.extend(new.iter().map(|(id, _)| id.clone())));
        if !follow {
            return;
        }
        for (_, file) in new {
            let path = std::path::PathBuf::from(file);
            let Some(b) = self.tree.bench(bench) else { return };
            let tab = b.tabs.iter().filter_map(|t| Some((t.id, &t.project_info()?.root))).filter(|(_, r)| path.starts_with(r)).max_by_key(|(_, r)| r.as_os_str().len()).map(|(id, _)| id);
            if let Some(view) = tab.and_then(|t| self.projects.get(&t)) {
                view.update(cx, |v, cx| v.show_quietly(path, cx));
            }
        }
    }

    /// A desktop notification that Claude finished or waits, unless its tab
    /// is in view in the active window.
    fn tell_about_ai(&mut self, tab: TabId, view: &Entity<AiView>, state: shed_ai::State, cx: &mut Context<Self>) {
        let bench = view.read(cx).bench;
        let in_view = self.tree.bench(bench).is_some_and(|b| b.active_tab == tab)
            && self.windows.get(&bench).copied().is_some_and(|h| h.update(cx, |_, window, _| window.is_window_active()).unwrap_or(false));
        if in_view {
            return;
        }
        let bench_name = self.tree.bench(bench).map(|b| b.name.clone()).unwrap_or_default();
        let title = view.read(cx).title();
        let what = if state == shed_ai::State::Waiting { "Claude needs your answer" } else { "Claude finished" };
        // A click brings the bench's window and this tab forward.
        let clicked = crate::notify::send_clickable(&format!("{what} · {bench_name}"), &title);
        cx.spawn(async move |this, cx| {
            if clicked.recv().await.is_ok() {
                this.update(cx, |state, cx| state.go_to_tab(bench, tab, cx)).ok();
            }
        })
        .detach();
    }

    /// Brings a bench's window forward (opening it if hidden) at `tab`.
    pub fn go_to_tab(&mut self, bench: BenchId, tab: TabId, cx: &mut Context<Self>) {
        if self.tree.bench(bench).is_none() {
            return;
        }
        self.conn.send_msg(ClientMsg::FocusTab { tab });
        self.show(bench, cx);
        cx.activate(true);
    }

    /// Shows a file's change in the bench's project tab that holds it.
    pub fn review_in_project(&mut self, bench: BenchId, path: std::path::PathBuf, cx: &mut Context<Self>) {
        let Some(b) = self.tree.bench(bench) else { return };
        let tab = b.tabs.iter().filter_map(|t| Some((t.id, &t.project_info()?.root))).filter(|(_, r)| path.starts_with(r)).max_by_key(|(_, r)| r.as_os_str().len()).map(|(id, _)| id);
        let Some(tab) = tab else { return };
        let Some(view) = self.projects.get(&tab).cloned() else { return };
        self.conn.send_msg(ClientMsg::FocusTab { tab });
        view.update(cx, |v, cx| v.review(path, cx));
    }

    /// Goes to the bench's AI tab for `folder`, or opens one.
    pub fn open_ai_for(&mut self, bench: BenchId, folder: std::path::PathBuf) {
        let existing = self.tree.bench(bench).and_then(|b| b.tabs.iter().find(|t| t.ai_info().is_some_and(|a| a.folder == folder))).map(|t| t.id);
        match existing {
            Some(tab) => self.conn.send_msg(ClientMsg::FocusTab { tab }),
            None => self.conn.send_msg(ClientMsg::NewAiTab { bench, folder }),
        }
    }

    /// Puts text into the bench's AI tab for the folder holding `path` (or
    /// its only AI tab), and shows it.
    pub fn send_to_ai(&mut self, bench: BenchId, path: &std::path::Path, text: &str, cx: &mut Context<Self>) -> bool {
        let Some(b) = self.tree.bench(bench) else { return false };
        let ais: Vec<(TabId, std::path::PathBuf)> = b.tabs.iter().filter_map(|t| Some((t.id, t.ai_info()?.folder.clone()))).collect();
        let tab = ais.iter().filter(|(_, f)| path.starts_with(f)).max_by_key(|(_, f)| f.as_os_str().len()).or(ais.first()).map(|(t, _)| *t);
        let Some(view) = tab.and_then(|t| self.ai_views.get(&t)).cloned() else { return false };
        self.conn.send_msg(ClientMsg::FocusTab { tab: tab.unwrap() });
        view.update(cx, |v, cx| v.insert(text, cx));
        true
    }

    /// Opens a file in the project tab of `bench` whose folder holds it, at
    /// a line and column (from 1); with `files`, shown in its Files panel
    /// too. False if no project tab holds it.
    pub fn open_in_project(&mut self, bench: BenchId, path: std::path::PathBuf, at: Option<(usize, Option<usize>)>, files: bool, cx: &mut Context<Self>) -> bool {
        let Some(b) = self.tree.bench(bench) else { return false };
        let roots: Vec<(TabId, std::path::PathBuf)> = b.tabs.iter().filter_map(|t| Some((t.id, t.project_info()?.root.clone()))).collect();
        let Some((tab, path)) = project_holding(&path, &roots) else { return false };
        let Some(view) = self.projects.get(&tab).cloned() else { return false };
        self.conn.send_msg(ClientMsg::FocusTab { tab });
        view.update(cx, |v, cx| v.show_path(path, at, files, cx));
        true
    }

    /// Opens a file in the bench's project tab that holds it; with none, a
    /// project tab opens for its project (its Git root, else its folder) and
    /// shows the file once it is there.
    pub fn reveal_file(&mut self, bench: BenchId, path: std::path::PathBuf, at: Option<(usize, Option<usize>)>, files: bool, cx: &mut Context<Self>) {
        if self.open_in_project(bench, path.clone(), at, files, cx) {
            return;
        }
        let root = crate::project_view::project_root(path.parent().unwrap_or(&path));
        self.conn.send_msg(ClientMsg::NewProjectTab { bench, root });
        self.pending_reveal = Some((bench, path, at, files));
    }

    /// What should have the keyboard in a bench's window: its active tab's
    /// active pane, or the tab itself when it has no panes.
    pub fn active_focus(&self, bench: BenchId, cx: &gpui::App) -> Option<(Focus, FocusHandle)> {
        let tab = self.tree.bench(bench)?.active_tab()?;
        if tab.project_info().is_some() {
            let view = self.projects.get(&tab.id)?;
            return Some((Focus::Tab(tab.id), view.read(cx).focus_target(cx)));
        }
        if tab.ai_info().is_some() {
            let view = self.ai_views.get(&tab.id)?;
            return Some((Focus::Tab(tab.id), view.read(cx).focus_target(cx)));
        }
        let view = self.panes.get(&tab.active_pane)?;
        Some((Focus::Pane(tab.active_pane), view.read(cx).focus_handle().clone()))
    }

    /// Keeps every bench's desktop entry, icon and window app ID current.
    fn sync_identities(&mut self, cx: &mut Context<Self>) {
        let shed = std::env::current_exe().unwrap_or_else(|_| "shed".into());
        let badges = self.settings.icon_badges;
        for bench in &self.tree.benches {
            let identity = Identity::of(bench, badges);
            let previous = self.identities.get(&bench.id);
            if previous == Some(&identity) {
                continue;
            }
            // A rename rewrites the same entry; only a new launcher key moves it.
            if let Some(old) = previous.filter(|old| old.launcher != identity.launcher) {
                self.desktop.remove(&old.launcher);
                if let Some(handle) = self.windows.get(&bench.id) {
                    let id = shed_desktop::app_id(&identity.launcher);
                    handle.update(cx, |_, window, _| window.set_app_id(&id)).ok();
                }
            }
            // GPUI sets a window's icon only as it opens; X11 can change it.
            if previous.is_some_and(|old| (old.color, &old.badge) != (identity.color, &identity.badge))
                && let Some(handle) = self.windows.get(&bench.id)
                && let Some(icon) = window_icon(&identity)
            {
                handle.update(cx, |_, window, _| set_window_icon(window, &icon)).ok();
            }
            let written = self.desktop.install(&BenchIdentity {
                launcher: &identity.launcher,
                name: &identity.name,
                color: identity.color,
                badge: identity.badge.as_deref(),
                shed: &shed,
            });
            if let Err(e) = written {
                warn!("cannot write the desktop entry for {}: {e}", identity.name);
            }
            self.identities.insert(bench.id, identity);
        }
        // A bench that stops running is still saved, so its launcher stays;
        // the keeper removes it when the bench is deleted.
        self.identities.retain(|id, _| self.tree.bench(*id).is_some());
    }

    fn sync_window(&mut self, bench: BenchId, cx: &mut Context<Self>) {
        let Some(handle) = self.windows.get(&bench).copied() else { return };
        let Some(b) = self.tree.bench(bench) else { return };
        let title = window_title(&b.name);
        let active = self.active_focus(bench, cx);
        handle.update(cx, |bw, window, cx| bw.sync(title, active, window, cx)).ok();
    }

    /// A pane's view, at its tab's zoom: a window opening again (after hiding,
    /// or loading the bench) shows the tab as it was.
    fn new_view(&self, pane: PaneId, cx: &mut Context<Self>) -> Entity<TerminalView> {
        let (conn, settings) = (self.conn.clone(), self.settings.clone());
        let zoom = self.tree.locate_pane(pane).and_then(|(_, tab)| self.tree.tab(tab)).map_or(0.0, |t| t.zoom);
        cx.new(|cx| {
            let mut view = TerminalView::new(pane, conn, &settings, cx);
            if zoom != 0.0 {
                view.set_zoom(zoom, cx);
            }
            view
        })
    }

    /// Applies a reloaded config to keys, panes and windows.
    pub fn apply_settings(&mut self, mut settings: Settings, cx: &mut Context<Self>) {
        settings.errors.extend(crate::actions::bind_keys(cx, &settings.keys));
        for e in &settings.errors {
            warn!("config: {e}");
        }
        cx.set_global(crate::prompt::CurrentUi(settings.ui.clone()));
        crate::theme::set_ui_scale(settings.font.size);
        self.settings = settings;
        self.sync_identities(cx);
        for view in self.panes.values() {
            view.update(cx, |view, cx| view.apply(&self.settings, cx));
        }
        for view in self.projects.values() {
            view.update(cx, |view, cx| view.apply(&self.settings, cx));
        }
        for view in self.ai_views.values() {
            view.update(cx, |view, cx| view.apply_settings(&self.settings, cx));
        }
        cx.notify();
    }

    /// The desktop switched between light and dark.
    pub fn set_dark(&mut self, dark: bool, cx: &mut Context<Self>) {
        if self.settings.dark == dark {
            return;
        }
        self.conn.send_msg(ClientMsg::SetAppearance { dark });
        let settings = Settings::load(&Settings { dark, ..self.settings.clone() });
        self.apply_settings(settings, cx);
    }

    /// Writes one setting to `shed.toml` and takes it at once; a value that
    /// does not fit is said, and the file left as it was.
    pub fn set_config_value(&mut self, table: Option<&str>, key: &str, value: toml_edit::Value, cx: &mut Context<Self>) {
        if let Err(e) = Config::set_value(&Config::path(), table, key, value) {
            self.notice = Some(format!("cannot change the setting: {e}"));
            cx.notify();
            return;
        }
        let settings = Settings::load(&self.settings);
        self.apply_settings(settings, cx);
    }

    /// Removes one setting from `shed.toml` (back to the default) and takes it at once.
    pub fn remove_config_value(&mut self, table: &str, key: &str, cx: &mut Context<Self>) {
        if let Err(e) = Config::remove_value(&Config::path(), table, key) {
            self.notice = Some(format!("cannot change the setting: {e}"));
            cx.notify();
            return;
        }
        let settings = Settings::load(&self.settings);
        self.apply_settings(settings, cx);
    }

    /// Switches the Shed window between cards and the compact list.
    pub fn set_compact_pegboard(&mut self, compact: bool, cx: &mut Context<Self>) {
        if let Err(e) = Config::set_compact_pegboard(&Config::path(), compact) {
            warn!("cannot change the Shed window's view: {e}");
            return;
        }
        let settings = Settings::load(&self.settings);
        self.apply_settings(settings, cx);
    }

    /// Switches between following the desktop and always light or dark.
    pub fn set_appearance(&mut self, appearance: shed_model::config::Appearance, cx: &mut Context<Self>) {
        if let Err(e) = self.settings.config.set_appearance(&Config::path(), appearance) {
            warn!("cannot change the appearance: {e}");
            return;
        }
        let settings = Settings::load(&self.settings);
        self.apply_settings(settings, cx);
    }

    /// Reloads `shed.toml` (and its theme) whenever either changes.
    pub async fn watch_config(state: Entity<AppState>, cx: &mut AsyncApp) {
        let mut config_watch = FileWatch::default();
        let mut theme_watch = FileWatch::default();
        config_watch.changed(&Config::path());
        loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let dark = state.read_with(cx, |state, _| state.settings.dark);
            let theme_file = Config::load_from(&Config::path())
                .ok()
                .map(|c| shed_model::paths::config_dir().join("themes").join(format!("{}.toml", c.theme_name(dark))));
            let theme_changed = theme_file.as_deref().is_some_and(|p| theme_watch.changed(p));
            if config_watch.changed(&Config::path()) || theme_changed {
                state.update(cx, |state, cx| {
                    let settings = Settings::load(&state.settings);
                    state.apply_settings(settings, cx);
                });
            }
        }
    }

    fn open_bench_window(&mut self, bench_id: BenchId, cx: &mut Context<Self>) {
        if let Some(handle) = self.windows.get(&bench_id) {
            handle.update(cx, |_, window, _| window.activate()).ok();
            return;
        }
        if self.tree.bench(bench_id).is_none() {
            return;
        }
        if !self.opening.insert(bench_id) {
            return;
        }
        // Views first, so the replays that follow `Attached` find them.
        self.note_shown_tabs();
        let ids: Vec<PaneId> = self.shown_panes().into_iter().filter(|id| self.tree.locate_pane(*id).is_some_and(|(b, _)| b == bench_id)).collect();
        for id in ids {
            if !self.panes.contains_key(&id) {
                let view = self.new_view(id, cx);
                self.panes.insert(id, view);
            }
        }
        self.sync_projects(cx);
        let bench = self.tree.bench(bench_id).unwrap();
        let bounds = open_bounds(bench.window, size(px(960.0), px(640.0)), cx);
        let tracker = PlaceTracker::new(Some(bench_id), self.conn.clone(), bounds, bench.window);
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions { title: Some(window_title(&bench.name).into()), ..Default::default() }),
            window_bounds: Some(bounds),
            app_id: Some(shed_desktop::app_id(&bench.launcher)),
            icon: window_icon(&Identity::of(bench, self.settings.icon_badges)),
            ..Default::default()
        };
        // Opening a window renders it at once, and its render reads this
        // state, so open it after this update ends.
        cx.spawn(async move |this, cx| {
            let Some(state) = this.upgrade() else { return };
            let opened = cx.update(|cx| {
                cx.open_window(options, |window, cx| cx.new(|cx| BenchWindow::new(bench_id, state, tracker, window, cx)))
            });
            match opened {
                Ok(handle) => {
                    this.update(cx, |this, cx| {
                        this.pending.remove(&bench_id);
                        this.opening.remove(&bench_id);
                        this.windows.insert(bench_id, handle);
                        this.sync_window(bench_id, cx);
                    })
                    .ok();
                }
                Err(e) => {
                    error!("cannot open a window: {e}");
                    this.update(cx, |this, _| {
                        this.pending.remove(&bench_id);
                        this.opening.remove(&bench_id);
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    /// Opens or raises the Shed window.
    /// Closes Shed as closing its window does: asking first if benches run.
    pub fn request_quit(&mut self, cx: &mut Context<Self>) {
        match self.shed_window {
            Some(handle) => {
                handle.update(cx, |shed, window, cx| shed.request_quit(window, cx)).ok();
            }
            None => self.conn.send_msg(ClientMsg::QuitShed),
        }
    }

    pub fn show_shed(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.shed_window {
            handle.update(cx, |_, window, _| window.activate()).ok();
            return;
        }
        if std::mem::replace(&mut self.opening_shed, true) {
            return;
        }
        let shed = std::env::current_exe().unwrap_or_else(|_| "shed".into());
        if let Err(e) = self.desktop.install_shed(&shed) {
            warn!("cannot write Shed's desktop entry: {e}");
        }
        let bounds = open_bounds(self.tree.window, size(px(1120.0), px(740.0)), cx);
        let tracker = PlaceTracker::new(None, self.conn.clone(), bounds, self.tree.window);
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions { title: Some("Shed".into()), ..Default::default() }),
            window_bounds: Some(bounds),
            app_id: Some(shed_desktop::SHED_APP_ID.into()),
            icon: rasterize(shed_desktop::SHED_ICON),
            ..Default::default()
        };
        // As with bench windows: its render reads this state, so open it after this update.
        cx.spawn(async move |this, cx| {
            let Some(state) = this.upgrade() else { return };
            let opened = cx.update(|cx| cx.open_window(options, |window, cx| cx.new(|cx| ShedWindow::new(state, tracker, window, cx))));
            this.update(cx, |this, _| {
                this.opening_shed = false;
                match opened {
                    Ok(handle) => this.shed_window = Some(handle),
                    Err(e) => error!("cannot open the Shed window: {e}"),
                }
            })
            .ok();
        })
        .detach();
    }

    /// Hides a bench: its window closes, its tools keep running.
    pub fn hide_bench(&mut self, bench: BenchId, cx: &mut Context<Self>) {
        if let Some(handle) = self.windows.get(&bench).copied() {
            handle
                .update(cx, |bw, window, _| {
                    bw.tracker().record(window);
                    window.remove_window();
                })
                .ok();
        }
        self.window_closed(bench);
        cx.notify();
    }

    /// Sends where every bench window is, before they all close.
    pub fn record_places(&mut self, cx: &mut Context<Self>) {
        for handle in self.windows.values() {
            handle.update(cx, |bw, window, _| bw.tracker().record(window)).ok();
        }
    }

    /// Called when a bench's window closes: its tools keep running in the keeper.
    pub fn window_closed(&mut self, bench: BenchId) {
        if self.windows.remove(&bench).is_some() {
            self.conn.send_msg(ClientMsg::Detach { bench });
        }
        self.recent_tabs.remove(&bench);
        let shown = self.shown_panes();
        self.panes.retain(|id, _| shown.contains(id));
        release_freed_memory();
    }

    /// Puts each open bench's active tab first in its recent tabs.
    fn note_shown_tabs(&mut self) {
        let open: Vec<BenchId> = self.windows.keys().chain(&self.opening).copied().collect();
        self.recent_tabs.retain(|b, _| open.contains(b));
        for bench in open {
            let Some(b) = self.tree.bench(bench) else { continue };
            let recent = self.recent_tabs.entry(bench).or_default();
            recent.retain(|t| b.tabs.iter().any(|tab| tab.id == *t));
            if let Some(active) = b.active_tab().map(|t| t.id) {
                recent.retain(|t| *t != active);
                recent.insert(0, active);
            }
            recent.truncate(KEPT_TABS);
        }
    }

    /// The panes of the recent tabs of benches with a window, or one being
    /// opened: a window opens a moment after its bench attaches, and the
    /// replays that arrive meanwhile must find their views.
    fn shown_panes(&self) -> HashSet<PaneId> {
        self.recent_tabs
            .values()
            .flatten()
            .filter_map(|t| self.tree.tab(*t))
            .flat_map(|t| &t.panes)
            .map(|p| p.id)
            .collect()
    }

    pub fn has_window(&self, bench: BenchId) -> bool {
        self.windows.contains_key(&bench)
    }

    /// Bench windows in the order Shed lists them, for Super+1…9.
    pub fn window_at(&self, index: usize, cx: &gpui::App) -> Option<WindowHandle<BenchWindow>> {
        let order = cx.try_global::<PegboardData>().map(|d| d.0.order.as_slice()).unwrap_or_default();
        let mut benches: Vec<&Bench> = self.tree.benches.iter().filter(|b| self.windows.contains_key(&b.id)).collect();
        benches.sort_by_key(|b| (order.iter().position(|id| *id == b.id).unwrap_or(usize::MAX), b.name.to_lowercase()));
        benches.get(index).and_then(|b| self.windows.get(&b.id)).copied()
    }

    /// Where a tab of bench `from` can move: every other bench, running or
    /// closed, by name.
    pub fn tab_destinations(&self, from: &BenchRef, cx: &gpui::App) -> Vec<Destination> {
        let running = self.tree.benches.iter().map(|b| Destination { name: b.name.clone(), closed: false, to: BenchRef::Running(b.id) });
        let stored = cx.try_global::<PegboardData>().map(|d| d.0.stored.as_slice()).unwrap_or_default();
        let closed = stored
            .iter()
            .filter(|s| self.tree.bench(s.bench.id).is_none())
            .map(|s| Destination { name: s.bench.name.clone(), closed: true, to: BenchRef::Saved(s.bench.name.clone()) });
        let mut all: Vec<Destination> = running.chain(closed).filter(|d| &d.to != from).collect();
        all.sort_by_key(|d| d.name.to_lowercase());
        all
    }
}

/// A bench a tab can move to.
pub struct Destination {
    pub name: String,
    pub closed: bool,
    pub to: BenchRef,
}

/// Moves tab `tab` (titled `title`) from one bench to another, at `index`.
/// Into a closed bench, what runs in the tab stops, so that asks first.
#[allow(clippy::too_many_arguments)]
pub fn move_tab(conn: &KeeperConn, tab: TabId, title: &str, from: BenchRef, to: BenchRef, to_name: &str, index: usize, window: &mut gpui::Window, cx: &mut gpui::App) {
    let msg = match (&from, &to) {
        (BenchRef::Running(_), BenchRef::Running(bench)) => ClientMsg::MoveTab { tab, bench: *bench, index },
        (BenchRef::Saved(a), BenchRef::Saved(b)) if a == b => {
            ClientMsg::EditSaved { name: a.clone(), edit: shed_proto::SavedEdit::MoveTab { tab, index } }
        }
        (BenchRef::Running(_), BenchRef::Saved(_)) => {
            let answer = window.prompt(
                gpui::PromptLevel::Warning,
                &format!("Move “{title}” to {to_name}?"),
                Some(&format!(
                    "{to_name} is closed, so what runs in this tab stops now and its teardown commands run. \
                     It starts again when {to_name} opens."
                )),
                &["Move", "Cancel"],
                cx,
            );
            let conn = conn.clone();
            cx.spawn(async move |_| {
                if let Ok(0) = answer.await {
                    conn.send_msg(ClientMsg::MoveTabAcross { tab, from, to, index });
                    conn.send_msg(ClientMsg::Pegboard);
                }
            })
            .detach();
            return;
        }
        _ => ClientMsg::MoveTabAcross { tab, from, to, index },
    };
    conn.send_msg(msg);
    conn.send_msg(ClientMsg::Pegboard);
}

/// Closes `tab` of a bench, after asking: a slip of the hand would
/// otherwise take along whatever runs in it.
pub fn close_tab(conn: &KeeperConn, tab: &Tab, bench: BenchRef, window: &mut gpui::Window, cx: &mut gpui::App) {
    let (title, id) = (tab.display_title(), tab.id);
    if let (BenchRef::Running(_), Some(remote)) = (&bench, tab.remote_info()) {
        // The shell on the server may be open on other PCs too: closing here
        // only lets go of it; ending it is the other choice.
        let message = format!("Close the remote tab “{title}”?");
        let detail = format!(
            "Close Here keeps the shell running on {}, to join again from this PC or another (New Remote Tab). End the Shell stops it, and what runs in it, for every PC.",
            remote.host
        );
        let answer = window.prompt(gpui::PromptLevel::Warning, &message, Some(&detail), &["Close Here", "End the Shell", "Cancel"], cx);
        let conn = conn.clone();
        cx.spawn(async move |_| {
            match answer.await {
                Ok(0) => conn.send_msg(ClientMsg::CloseTab { tab: id }),
                Ok(1) => conn.send_msg(ClientMsg::EndRemoteTab { tab: id }),
                _ => return,
            }
            conn.send_msg(ClientMsg::Pegboard);
        })
        .detach();
        return;
    }
    let (message, detail, button, msg) = match bench {
        BenchRef::Running(_) if tab.ai_info().is_some() => (
            format!("Close the AI tab “{title}”?"),
            "Claude Code stops if it is working. The session is kept: open it again from a new AI tab.",
            "Close Tab",
            ClientMsg::CloseTab { tab: id },
        ),
        BenchRef::Running(_) if tab.project_info().is_some() => (
            format!("Close the project tab “{title}”?"),
            "Its files stay where they are; open the folder again to bring it back.",
            "Close Tab",
            ClientMsg::CloseTab { tab: id },
        ),
        BenchRef::Running(_) => (
            format!("Close the tab “{title}”?"),
            "The programs running in it stop and its teardown commands run.",
            "Close Tab",
            ClientMsg::CloseTab { tab: id },
        ),
        // A closed bench's tab has nothing running: it just goes.
        BenchRef::Saved(name) if tab.remote_info().is_some() => (
            format!("Remove the remote tab “{title}”?"),
            "It is taken out of the saved bench. Its shell keeps running on the server: join it again from New Remote Tab.",
            "Remove Tab",
            ClientMsg::EditSaved { name, edit: shed_proto::SavedEdit::RemoveTab { tab: id } },
        ),
        BenchRef::Saved(name) => (
            format!("Remove the tab “{title}”?"),
            "It is taken out of the saved bench, with its startup and teardown commands.",
            "Remove Tab",
            ClientMsg::EditSaved { name, edit: shed_proto::SavedEdit::RemoveTab { tab: id } },
        ),
    };
    let answer = window.prompt(gpui::PromptLevel::Warning, &message, Some(detail), &[button, "Cancel"], cx);
    let conn = conn.clone();
    cx.spawn(async move |_| {
        if let Ok(0) = answer.await {
            conn.send_msg(msg);
            conn.send_msg(ClientMsg::Pegboard);
        }
    })
    .detach();
}

/// The project (of `roots`) that holds `path`, the innermost, and the path
/// as that project knows it. Compared with links resolved: a project open as
/// ~/Workspace/x holds ~/Work/x/f when ~/Work links to ~/Workspace.
pub fn project_holding(path: &std::path::Path, roots: &[(TabId, std::path::PathBuf)]) -> Option<(TabId, std::path::PathBuf)> {
    let real = |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let real_path = real(path);
    roots
        .iter()
        .filter_map(|(id, root)| {
            let real_root = real(root);
            let rel = real_path.strip_prefix(&real_root).ok()?.to_path_buf();
            Some((*id, real_root.as_os_str().len(), root.join(rel)))
        })
        .max_by_key(|(_, len, _)| *len)
        .map(|(id, _, p)| (id, p))
}

/// The app state, for views that reach across benches (a terminal's
/// Ctrl+click opening a file in a project tab).
pub struct AppHandle(pub gpui::WeakEntity<AppState>);

impl gpui::Global for AppHandle {}

/// What has the keyboard in a bench window: a pane, or a tab without panes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Focus {
    Pane(PaneId),
    Tab(TabId),
}

/// The desktop's folder picker, for a project tab's folder: the folder
/// chosen, `None` if cancelled, or why there is no picker.
pub fn pick_folder(cx: &gpui::App) -> impl std::future::Future<Output = Result<Option<std::path::PathBuf>, String>> + use<> {
    let options = gpui::PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Open Project".into()) };
    let answer = cx.prompt_for_paths(options);
    async move {
        match answer.await {
            Ok(Ok(paths)) => Ok(paths.and_then(|p| p.into_iter().next())),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Ok(None),
        }
    }
}

pub fn window_title(bench: &str) -> String {
    format!("Shed — {bench}")
}

/// The bench icon as a window icon (X11 `_NET_WM_ICON`, Wayland `xdg-toplevel-icon`).
fn window_icon(identity: &Identity) -> Option<std::sync::Arc<image::RgbaImage>> {
    rasterize(&shed_desktop::icon_svg(identity.color, identity.badge.as_deref()))
}

/// Replaces an open window's icon (X11 `_NET_WM_ICON`). On Wayland the dock
/// shows the desktop entry's icon, which is rewritten instead.
fn set_window_icon(window: &gpui::Window, icon: &image::RgbaImage) {
    #[cfg(target_os = "linux")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, PropMode};
        use x11rb::wrapper::ConnectionExt as _;

        let id = match HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Xcb(h)) => h.window.get(),
            Ok(RawWindowHandle::Xlib(h)) => h.window as u32,
            _ => return,
        };
        let set = || -> Result<(), Box<dyn std::error::Error>> {
            let (conn, _) = x11rb::connect(None)?;
            let atom = conn.intern_atom(false, b"_NET_WM_ICON")?.reply()?.atom;
            let mut data = vec![icon.width(), icon.height()];
            data.extend(icon.pixels().map(|p| {
                let [r, g, b, a] = p.0;
                (a as u32) << 24 | (r as u32) << 16 | (g as u32) << 8 | b as u32
            }));
            // Checked, so the request reaches the server before the
            // connection closes.
            conn.change_property32(PropMode::REPLACE, id, atom, AtomEnum::CARDINAL, &data)?.check()?;
            Ok(())
        };
        if let Err(e) = set() {
            warn!("cannot change the window icon: {e}");
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (window, icon);
}

fn rasterize(svg: &str) -> Option<std::sync::Arc<image::RgbaImage>> {
    const SIZE: u32 = 128;
    let rgba = shed_desktop::icon_rgba(svg, SIZE)?;
    image::RgbaImage::from_raw(SIZE, SIZE, rgba).map(std::sync::Arc::new)
}

/// Hands memory freed by dropped views back to the system. A view's
/// scrollback is many small rows, which glibc otherwise keeps for reuse.
fn release_freed_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: malloc_trim only returns free heap pages to the kernel.
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn usage_limits_show_on_a_bench_with_an_ai_tab(cx: &mut gpui::TestAppContext) {
        use std::sync::atomic::Ordering::Relaxed;
        cx.update(|cx| cx.set_global(crate::prompt::CurrentUi(crate::theme::UiColors::default())));
        let state = cx.new(|_| AppState::new(KeeperConn::detached(), Settings::default()));
        let ai = shed_model::Tab::ai("/tmp".into());
        let bench = shed_model::Bench {
            id: BenchId::new(),
            name: "work".into(),
            color: shed_model::BenchColor::Blue,
            badge: None,
            cwd: "/".into(),
            active_tab: ai.id,
            tabs: vec![ai],
            aliases: Default::default(),
            closing: false,
            started: false,
            launcher: "work".into(),
            window: None,
            links: Vec::new(),
        };
        let id = bench.id;
        let tree = Shed { benches: vec![bench], ..Default::default() };
        state.update(cx, |s, cx| s.handle(Frame::Keeper(KeeperMsg::TreeChanged(tree)), cx));
        let tracker = crate::window_place::PlaceTracker::new(Some(id), KeeperConn::detached(), gpui::WindowBounds::Windowed(gpui::Bounds::default()), None);
        let st = state.clone();
        let (_, cx) = cx.add_window_view(|window, cx| crate::bench_window::BenchWindow::new(id, st, tracker, window, cx));
        cx.run_until_parked();
        let before = crate::ai_view::BARS_DRAWN.load(Relaxed);
        // The keeper passes on what Claude Code reported.
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let limits = shed_ai::Limits { five_hour: Some(shed_ai::Window { used: 0.37, resets: now + 3600 }), week: None, seen: now };
        state.update(cx, |s, cx| s.handle(Frame::Keeper(KeeperMsg::AiLimits(limits)), cx));
        cx.run_until_parked();
        assert!(crate::ai_view::BARS_DRAWN.load(Relaxed) > before, "the bench window drew the limits");
    }

    #[test]
    fn a_project_holds_its_files_through_links() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("Workspace/app");
        std::fs::create_dir_all(real.join("docs")).unwrap();
        std::fs::write(real.join("docs/guide.md"), "").unwrap();
        std::os::unix::fs::symlink(dir.path().join("Workspace"), dir.path().join("Work")).unwrap();
        let (outer, inner) = (TabId::new(), TabId::new());
        let roots = vec![(outer, dir.path().join("Workspace")), (inner, real.clone())];
        // Named through the link, held by the innermost project.
        let (tab, path) = project_holding(&dir.path().join("Work/app/docs/guide.md"), &roots).unwrap();
        assert_eq!((tab, path), (inner, real.join("docs/guide.md")));
        assert!(project_holding(&dir.path().join("elsewhere.md"), &roots).is_none());
    }
}
