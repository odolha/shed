//! The app's view of the keeper: the tree, one window per bench, and the
//! terminal view of every pane those windows show.
//!
//! The keeper is this PC's, or another workspace's reached through a nexus
//! this desktop is signed in to (`Workspace`): one is shown at a time.
//! Switching closes the windows (leaving the benches as they are, as when
//! the window process quits), connects to the other keeper and opens
//! windows as at startup. While another workspace is shown, a quiet
//! connection to this PC's keeper (`standby`) keeps this process its
//! window process, so launching a bench here switches back.
//!
//! What windows read from this machine's disk (project tabs, the logbook,
//! Claude Code's session files) is never taken for another workspace's:
//! `is_remote` says which, and those views use that workspace's calls
//! through the nexus. Its benches' launchers are named by it.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui::{AppContext, AsyncApp, Context, Entity, FocusHandle, TitlebarOptions, WeakEntity, WindowHandle, WindowOptions, px, size};
use shed_model::config::{Config, FileWatch};
use shed_desktop::{BenchIdentity, Desktop};
use shed_model::{Bench, BenchColor, BenchId, PaneId, Shed, Tab, TabId};
use shed_client::nexus_api::RemoteWorkspace;
use shed_proto::{BenchRef, ClientKind, ClientMsg, Frame, KeeperMsg, ShowTarget};
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

/// A keeper connection and the frames from it.
type Connection = (KeeperConn, async_channel::Receiver<Frame>);

/// Which workspace's keeper the windows show.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Workspace {
    /// This PC's own keeper.
    #[default]
    Local,
    /// Another workspace, through the nexus (`id` is the nexus's).
    Remote { id: String, name: String },
}

impl Workspace {
    pub fn is_remote(&self) -> bool {
        matches!(self, Workspace::Remote { .. })
    }

    /// As the switcher names it.
    pub fn label(&self) -> &str {
        match self {
            Workspace::Local => "This PC",
            Workspace::Remote { name, .. } => name,
        }
    }
}

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
    /// The workspace shown.
    pub workspace: Workspace,
    /// The workspace being connected to, while it is.
    pub switching: Option<String>,
    /// Which connection `conn` is: frames from older ones are dropped.
    generation: u64,
    last_generation: u64,
    /// While another workspace is shown: this PC's keeper, as a window
    /// process showing nothing (its generation too).
    standby: Option<(KeeperConn, u64)>,
    /// This PC's running benches, as the standby connection last heard.
    local_benches: usize,
    /// The benches each workspace had windows for when it was left, to
    /// show them again on coming back (this session only).
    shown_before: HashMap<Workspace, Vec<BenchId>>,
    /// Shed is closing.
    quitting: bool,
    /// Another workspace's calls and topics through the nexus (its files,
    /// Git, logbook, AI sessions), while it is shown.
    remote_api: Option<RemoteWorkspace>,
    /// A bench to show once the workspace (by id) is entered
    /// (`shed load --workspace`).
    pending_load: Option<(String, String)>,
    /// The workspace the panel's pins were last made to follow.
    taskbar_for: Option<String>,
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
            // Unit tests open bench windows too: their launchers and icons go
            // to a folder of the test's, never to the user's desktop.
            desktop: Desktop::new(if cfg!(test) { std::env::temp_dir().join(format!("shed-test-desktop-{}", std::process::id())) } else { shed_model::paths::data_home() }),
            identities: HashMap::new(),
            workspace: Workspace::Local,
            switching: None,
            generation: 0,
            last_generation: 0,
            standby: None,
            local_benches: 0,
            shown_before: HashMap::new(),
            quitting: false,
            remote_api: None,
            pending_load: None,
            taskbar_for: None,
        }
    }

    /// Whether the windows show another workspace than this PC's: what is
    /// read from this machine's disk does not belong to it.
    pub fn is_remote(&self) -> bool {
        self.workspace.is_remote()
    }

    /// The shown workspace's calls through the nexus, when it is another
    /// workspace: what views read in place of this machine's disk.
    pub fn remote(&self) -> Option<RemoteWorkspace> {
        self.remote_api.clone()
    }

    /// Reads frames from the keeper until it goes away.
    pub async fn receive(state: Entity<AppState>, rx: async_channel::Receiver<Frame>, cx: &mut AsyncApp) {
        Self::receive_from(state.downgrade(), rx, 0, cx).await;
    }

    /// Reads frames from connection `generation` until it ends.
    async fn receive_from(state: WeakEntity<AppState>, rx: async_channel::Receiver<Frame>, generation: u64, cx: &mut AsyncApp) {
        while let Ok(first) = rx.recv().await {
            let started = Instant::now();
            let alive = state.update(cx, |state, cx| {
                state.frame(generation, first, cx);
                // Drain what is already queued, but not for so long that frames stall.
                while started.elapsed() < FRAME_BUDGET {
                    match rx.try_recv() {
                        Ok(frame) => state.frame(generation, frame, cx),
                        Err(_) => break,
                    }
                }
            });
            if alive.is_err() {
                return;
            }
            if !rx.is_empty() {
                cx.background_executor().timer(Duration::ZERO).await;
            }
        }
        state.update(cx, |state, cx| state.connection_ended(generation, cx)).ok();
    }

    /// A frame from connection `generation`: the shown workspace's, the
    /// standby's, or one let go of.
    fn frame(&mut self, generation: u64, frame: Frame, cx: &mut Context<Self>) {
        if generation == self.generation {
            self.handle(frame, cx);
        } else if self.standby.as_ref().is_some_and(|(_, g)| *g == generation) {
            self.standby_frame(frame, cx);
        }
    }

    /// Connection `generation` ended.
    fn connection_ended(&mut self, generation: u64, cx: &mut Context<Self>) {
        if self.standby.as_ref().is_some_and(|(_, g)| *g == generation) {
            self.standby = None;
            if self.quitting {
                cx.quit();
            } else {
                warn!("this PC's keeper went away while another workspace is shown");
            }
            return;
        }
        if generation != self.generation {
            return;
        }
        match self.workspace.clone() {
            Workspace::Local => {
                error!("lost the connection to the keeper");
                cx.quit();
            }
            Workspace::Remote { name, .. } => {
                warn!("lost the connection to {name}");
                if self.notice.as_deref().is_none_or(|n| !n.starts_with(shed_client::nexus_client::LOST)) {
                    self.notice = Some(format!("Lost the connection to {name}; back to This PC."));
                }
                self.switch_to_local(cx);
            }
        }
    }

    /// What this PC's keeper says while another workspace is shown: asked
    /// to show one of its benches (a launcher, `shed BENCH`), Shed comes
    /// back to it.
    fn standby_frame(&mut self, frame: Frame, cx: &mut Context<Self>) {
        match frame {
            Frame::Keeper(KeeperMsg::TreeChanged(tree)) => self.local_benches = tree.benches.len(),
            Frame::Keeper(KeeperMsg::Status(status)) => self.local_benches = status.tree.benches.len(),
            Frame::Keeper(KeeperMsg::Show { bench }) => {
                self.switch_to_local(cx);
                self.show(bench, cx);
            }
            Frame::Keeper(KeeperMsg::ShowShed) => self.show_shed(cx),
            Frame::Keeper(KeeperMsg::ShowInWorkspace { workspace, bench }) => self.show_in_workspace(workspace, bench, cx),
            _ => {}
        }
    }

    fn next_generation(&mut self) -> u64 {
        self.last_generation += 1;
        self.last_generation
    }

    /// Running benches Shed closes when it closes: this PC's.
    pub fn local_running(&self) -> usize {
        if self.is_remote() { self.local_benches } else { self.tree.benches.len() }
    }

    /// Shows another workspace (or this PC's): connects first, and only
    /// then closes this one's windows, so a workspace that cannot be
    /// reached leaves everything as it was (and says why).
    pub fn switch_to(&mut self, target: Workspace, cx: &mut Context<Self>) {
        if target == self.workspace || self.switching.is_some() {
            return;
        }
        let Workspace::Remote { id, name } = target.clone() else {
            self.switch_to_local(cx);
            return;
        };
        let Some(sign_in) = shed_client::nexus_client::SignIn::load() else {
            self.notice = Some("Sign in to a nexus first (Sign In to Nexus…, or shed nexus signin).".into());
            cx.notify();
            return;
        };
        if sign_in.ended(shed_link::pairing::now()) {
            self.notice = Some("This desktop's sign-in to the nexus has ended: sign in again (This PC ▾ → Sign In Again…, or shed nexus signin).".into());
            cx.notify();
            return;
        }
        self.switching = Some(name.clone());
        cx.notify();
        let need_standby = !self.is_remote();
        cx.spawn(async move |this, cx| {
            let connected = cx
                .background_executor()
                .spawn(async move {
                    let remote = shed_client::keeper_conn::connect_remote(&sign_in.url, &sign_in.token, &id, ClientKind::Remote)?;
                    let standby = if need_standby { Some(shed_client::keeper_conn::connect(ClientKind::Window, true)?) } else { None };
                    // Its files, Git and the like come as calls (connecting when first needed).
                    let api = shed_client::nexus_api::NexusApi::new(&sign_in, shed_client::nexus_client::configured_ca_file()).workspace(&id);
                    anyhow::Ok((remote, standby, api))
                })
                .await;
            this.update(cx, |state, cx| {
                state.switching = None;
                match connected {
                    Ok((remote, standby, api)) => state.connected_to(target, remote, standby, Some(api), cx),
                    Err(e) => {
                        state.notice = Some(format!("Cannot show {name}: {e:#}"));
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Connected to another workspace (and, coming from this PC, to its
    /// keeper as the standby): shows it.
    fn connected_to(&mut self, target: Workspace, remote: Connection, standby: Option<Connection>, api: Option<RemoteWorkspace>, cx: &mut Context<Self>) {
        self.leave_workspace(cx);
        self.remote_api = api;
        if let Some((standby, rx)) = standby {
            let generation = self.next_generation();
            standby.send_msg(ClientMsg::Status);
            self.standby = Some((standby, generation));
            cx.spawn(async move |this, cx| Self::receive_from(this, rx, generation, cx).await).detach();
        }
        let (conn, rx) = remote;
        let generation = self.next_generation();
        cx.spawn(async move |this, cx| Self::receive_from(this, rx, generation, cx).await).detach();
        self.enter(target, conn, generation, cx);
    }

    /// Back to this PC's keeper: through the standby connection when there
    /// is one (at once), else connecting again.
    pub fn switch_to_local(&mut self, cx: &mut Context<Self>) {
        if !self.is_remote() {
            return;
        }
        self.leave_workspace(cx);
        match self.standby.take() {
            Some((conn, generation)) => self.enter(Workspace::Local, conn, generation, cx),
            None => {
                self.workspace = Workspace::Local;
                self.switching = Some(Workspace::Local.label().into());
                cx.notify();
                cx.spawn(async move |this, cx| {
                    let connected = cx.background_executor().spawn(async { shed_client::keeper_conn::connect(ClientKind::Window, true) }).await;
                    this.update(cx, |state, cx| {
                        state.switching = None;
                        match connected {
                            Ok((conn, rx)) => {
                                let generation = state.next_generation();
                                cx.spawn(async move |this, cx| Self::receive_from(this, rx, generation, cx).await).detach();
                                state.enter(Workspace::Local, conn, generation, cx);
                            }
                            Err(e) => {
                                error!("cannot reach this PC's keeper: {e:#}");
                                cx.quit();
                            }
                        }
                    })
                    .ok();
                })
                .detach();
            }
        }
    }

    /// Closes the shown workspace's windows and forgets its tree and views;
    /// its keeper keeps the benches as they are (the connection is closed,
    /// as when the window process quits, so they stay shown there).
    fn leave_workspace(&mut self, cx: &mut Context<Self>) {
        let open: Vec<BenchId> = self.windows.keys().chain(&self.pending).copied().collect();
        self.shown_before.insert(self.workspace.clone(), open);
        for (_, handle) in self.windows.drain() {
            handle
                .update(cx, |bw, window, _| {
                    bw.tracker().record(window);
                    window.remove_window();
                })
                .ok();
        }
        let old = std::mem::replace(&mut self.conn, KeeperConn::detached());
        old.close();
        // Frames still on their way from it are dropped.
        self.generation = u64::MAX;
        self.panes.clear();
        self.projects.clear();
        self.ai_views.clear();
        self.recent_tabs.clear();
        self.pending.clear();
        self.opening.clear();
        self.identities.clear();
        self.pending_reveal = None;
        self.tree = Shed::default();
        self.remote_api = None;
        cx.set_global(PegboardData(Default::default()));
        if cx.has_global::<AiLimits>() {
            cx.remove_global::<AiLimits>();
        }
        release_freed_memory();
    }

    /// Shows `workspace` through `conn` (connection `generation`, whose
    /// frames are being received): its tree, the Shed window's view of it,
    /// and the benches it had windows for when last left.
    fn enter(&mut self, workspace: Workspace, conn: KeeperConn, generation: u64, cx: &mut Context<Self>) {
        self.conn = conn;
        self.generation = generation;
        self.workspace = workspace;
        // As at startup: the tree, and the Shed window's (already open) place.
        self.conn.send_msg(ClientMsg::Show(ShowTarget::Shed));
        self.conn.send_msg(ClientMsg::Pegboard);
        if !self.is_remote() {
            self.conn.send_msg(ClientMsg::SetAppearance { dark: self.settings.dark });
        }
        for bench in self.shown_before.remove(&self.workspace).unwrap_or_default() {
            self.show(bench, cx);
        }
        // A bench asked for in this workspace (its launcher): its keeper
        // shows it, loading it if it is saved.
        if let Workspace::Remote { id, .. } = &self.workspace
            && let Some((_, bench)) = self.pending_load.take().filter(|(ws, _)| ws == id)
        {
            self.conn.send_msg(ClientMsg::Show(ShowTarget::Load { name: bench }));
        }
        self.follow_taskbar();
        // The Shed window's place is this PC's, kept by its keeper only.
        let place_conn = if self.is_remote() { KeeperConn::detached() } else { self.conn.clone() };
        if let Some(handle) = self.shed_window {
            handle.update(cx, |shed, _, _| shed.tracker().set_conn(place_conn)).ok();
        }
        cx.notify();
    }

    /// Shows another workspace's bench (`shed load --workspace`, its
    /// launcher): `workspace` is its id or name at the nexus, `bench` a
    /// name or launcher key there. Switches to it first if needed.
    pub fn show_in_workspace(&mut self, workspace: String, bench: String, cx: &mut Context<Self>) {
        if let Workspace::Remote { id, name } = &self.workspace
            && names_workspace(&workspace, id, name)
        {
            self.conn.send_msg(ClientMsg::Show(ShowTarget::Load { name: bench }));
            return;
        }
        let Some(sign_in) = shed_client::nexus_client::SignIn::load() else {
            self.notice = Some(format!("Sign in to a nexus to show {workspace} (This PC ▾ → Sign In to Nexus…)."));
            cx.notify();
            return;
        };
        cx.spawn(async move |this, cx| {
            let listed = cx
                .background_executor()
                .spawn(async move {
                    let ca = shed_client::nexus_client::configured_ca_file();
                    shed_client::nexus_client::workspaces(&sign_in, ca.as_deref())
                })
                .await;
            this.update(cx, |state, cx| match listed {
                Ok(list) => match list.into_iter().find(|w| names_workspace(&workspace, &w.id, &w.name)) {
                    Some(w) => {
                        state.pending_load = Some((w.id.clone(), bench));
                        state.switch_to(Workspace::Remote { id: w.id, name: w.name }, cx);
                    }
                    None => {
                        state.notice = Some(format!("The nexus has no workspace {workspace}."));
                        cx.notify();
                    }
                },
                Err(e) => {
                    state.notice = Some(format!("Cannot show {workspace}: {e:#}"));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// With `[desktop] taskbar_follows_workspace`, makes the panel's pinned
    /// bench launchers the shown workspace's (in the background: it writes
    /// the panel's settings and may ask the panel to reload).
    fn follow_taskbar(&mut self) {
        if cfg!(test) || !self.settings.config.desktop.taskbar_follows_workspace {
            return;
        }
        let Some(panel) = shed_desktop::taskbar::Panel::detect() else { return };
        let key = match &self.workspace {
            Workspace::Local => shed_desktop::taskbar::LOCAL.to_string(),
            Workspace::Remote { id, .. } => id.clone(),
        };
        if self.taskbar_for.as_ref() == Some(&key) {
            return;
        }
        self.taskbar_for = Some(key.clone());
        std::thread::spawn(move || {
            // One switch at a time, in the order asked.
            static SWITCHING: std::sync::Mutex<()> = std::sync::Mutex::new(());
            let _one = SWITCHING.lock().unwrap_or_else(|e| e.into_inner());
            let taskbar = shed_desktop::taskbar::Taskbar {
                panel,
                state_dir: shed_model::paths::state_dir(),
                config_home: shed_model::paths::config_home(),
                run: &shed_desktop::taskbar::System,
            };
            if let Err(e) = taskbar.switch(&key) {
                warn!("cannot switch the {} panel's pinned benches: {e}", panel.name());
            }
        });
    }

    /// Turns `[desktop] taskbar_follows_workspace` on or off.
    pub fn set_taskbar_follows(&mut self, on: bool, cx: &mut Context<Self>) {
        if let Err(e) = Config::set_value(&Config::path(), Some("desktop"), "taskbar_follows_workspace", on.into()) {
            self.notice = Some(format!("Cannot change the setting: {e}"));
            cx.notify();
            return;
        }
        let settings = Settings::load(&self.settings);
        self.apply_settings(settings, cx);
    }

    /// Closes Shed: this PC's benches close and its keeper stops (another
    /// workspace's keeper is only let go of).
    pub fn quit_shed(&mut self, cx: &mut Context<Self>) {
        self.quitting = true;
        if self.is_remote() {
            if self.standby.is_none() {
                cx.quit();
                return;
            }
            // Its benches close: none to show again on the way.
            self.shown_before.remove(&Workspace::Local);
            self.switch_to_local(cx);
        }
        self.conn.send_msg(ClientMsg::QuitShed);
    }

    /// A window title for a bench: with the workspace's name when it is
    /// not this PC's, so its windows are told apart.
    pub fn bench_title(&self, bench: &str) -> String {
        match &self.workspace {
            Workspace::Local => window_title(bench),
            Workspace::Remote { name, .. } => format!("{} · {name}", window_title(bench)),
        }
    }

    /// A bench window's app ID (the dock groups windows by it): its
    /// launcher's, so a pinned launcher and its windows are one.
    fn app_id(&self, launcher: &str) -> String {
        match &self.workspace {
            Workspace::Local => shed_desktop::app_id(launcher),
            Workspace::Remote { name, .. } => shed_desktop::remote_app_id(name, launcher),
        }
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
            Frame::Keeper(KeeperMsg::ShowInWorkspace { workspace, bench }) => self.show_in_workspace(workspace, bench, cx),
            Frame::Keeper(KeeperMsg::Hide { bench }) => self.hide_bench(bench, cx),
            Frame::Keeper(KeeperMsg::Pegboard(info)) => {
                self.prune_launchers(&info);
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
                // A session being deleted in an AI tab: shown there.
                for view in self.ai_views.values() {
                    view.update(cx, |v, cx| v.keeper_error(&e, cx));
                }
                cx.notify();
                if self.is_remote() {
                    // Another workspace's keeper of another version: back to this PC.
                    if shed_client::keeper_conn::is_other_version(&e) {
                        let name = self.workspace.label().to_string();
                        self.notice = Some(format!("{name} runs another version of Shed ({e}); update one of them."));
                        self.switch_to_local(cx);
                    }
                } else if e.starts_with("protocol mismatch") {
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
            Frame::Keeper(KeeperMsg::AiEntries { tab, entries, status }) => {
                let Some(view) = self.ai_views.get(&tab).cloned() else { return };
                let before = view.read(cx).state();
                view.update(cx, |v, cx| v.replace(entries, status, cx));
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
                    // Another workspace's project is read through the nexus.
                    let (conn, settings, remote) = (self.conn.clone(), self.settings.clone(), self.remote());
                    let view = cx.new(|cx| {
                        let mut v = ProjectView::new(bench, tab, &project, accent, conn, settings, remote, cx);
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
            // The views of its bench's project tabs, for their file lists.
            let views: Vec<Entity<ProjectView>> = self.tree.bench(bench).map(|b| b.tabs.iter().filter_map(|t| self.projects.get(&t.id).cloned()).collect()).unwrap_or_default();
            match self.ai_views.get(&tab) {
                Some(view) => view.update(cx, |v, cx| {
                    (v.bench, v.accent) = (bench, accent);
                    v.set_zoom(zoom, cx);
                    if v.projects != projects {
                        v.projects = projects;
                        cx.notify();
                    }
                    v.set_ai(ai, cx);
                    v.set_project_views(views, cx);
                }),
                None if open.contains(&bench) => {
                    let (conn, settings, remote) = (self.conn.clone(), self.settings.clone(), self.remote());
                    let view = cx.new(|cx| {
                        let mut v = AiView::new(bench, tab, ai, accent, conn, settings, cx);
                        if let Some(workspace) = remote {
                            v.set_remote(workspace, cx);
                        }
                        v.projects = projects;
                        v.set_zoom(zoom, cx);
                        v.set_project_views(views, cx);
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
        let held = match self.is_remote() {
            // Another workspace's paths, as they are (not this disk's links).
            true => roots.iter().filter(|(_, r)| path.starts_with(r)).max_by_key(|(_, r)| r.as_os_str().len()).map(|(t, _)| (*t, path.clone())),
            false => shed_project::project_holding(&path, &roots),
        };
        let Some((tab, path)) = held else { return false };
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
        let root = match self.is_remote() {
            // Another workspace's folders are not on this disk to find its
            // project in: the bench's folder when it holds the file, else
            // the file's own.
            true => match self.tree.bench(bench).map(|b| b.cwd.clone()) {
                Some(cwd) if path.starts_with(&cwd) && cwd != std::path::Path::new("/") => cwd,
                _ => path.parent().unwrap_or(&path).to_path_buf(),
            },
            false => shed_project::project_root(path.parent().unwrap_or(&path)),
        };
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
    /// Another workspace's benches get launchers named by it, which switch
    /// Shed to it (`shed load --workspace`).
    fn sync_identities(&mut self, cx: &mut Context<Self>) {
        let shed = std::env::current_exe().unwrap_or_else(|_| "shed".into());
        let workspace = match &self.workspace {
            Workspace::Local => None,
            Workspace::Remote { id, name } => Some((id.clone(), name.clone())),
        };
        let badges = self.settings.icon_badges;
        for bench in &self.tree.benches {
            let identity = Identity::of(bench, badges);
            let previous = self.identities.get(&bench.id);
            if previous == Some(&identity) {
                continue;
            }
            // A rename rewrites the same entry; only a new launcher key moves it.
            if let Some(old) = previous.filter(|old| old.launcher != identity.launcher) {
                self.desktop.remove_app(&self.app_id(&old.launcher));
                if let Some(handle) = self.windows.get(&bench.id) {
                    let id = self.app_id(&identity.launcher);
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
                workspace: workspace.as_ref().map(|(id, name)| shed_desktop::WorkspaceRef { id, name }),
            });
            if let Err(e) = written {
                warn!("cannot write the desktop entry for {}: {e}", identity.name);
            }
            self.identities.insert(bench.id, identity);
        }
        // A bench that stops running is still saved, so its launcher stays;
        // the keeper removes it when the bench is deleted (another
        // workspace's: `prune_launchers`).
        self.identities.retain(|id, _| self.tree.bench(*id).is_some());
    }

    /// Another workspace's launchers of benches it no longer has (neither
    /// running nor saved: deleted there) go, as the keeper removes this
    /// PC's.
    fn prune_launchers(&mut self, info: &shed_proto::PegboardInfo) {
        let Workspace::Remote { name, .. } = &self.workspace else { return };
        let keep: HashSet<String> = self
            .tree
            .benches
            .iter()
            .chain(info.stored.iter().map(|s| &s.bench))
            .map(|b| shed_desktop::remote_app_id(name, &b.launcher))
            .collect();
        for id in self.desktop.remote_launchers(name) {
            if !keep.contains(&id) {
                self.desktop.remove_app(&id);
            }
        }
    }

    fn sync_window(&mut self, bench: BenchId, cx: &mut Context<Self>) {
        let Some(handle) = self.windows.get(&bench).copied() else { return };
        let Some(b) = self.tree.bench(bench) else { return };
        let title = self.bench_title(&b.name);
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
        let follows = settings.config.desktop.taskbar_follows_workspace;
        if follows != self.settings.config.desktop.taskbar_follows_workspace || !follows {
            // Turned on: follow from here; off: next time it is on, again.
            self.taskbar_for = None;
        }
        self.settings = settings;
        self.sync_identities(cx);
        self.follow_taskbar();
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
        // (Another workspace's keeper may send no tree soon after: its
        // window places are not sent to it.)
        self.sync_ai(cx);
        let bench = self.tree.bench(bench_id).unwrap();
        // Another workspace's windows are placed on its own screens: these
        // open and stay where this PC puts them.
        let (place, place_conn) = if self.is_remote() { (None, KeeperConn::detached()) } else { (bench.window, self.conn.clone()) };
        let bounds = open_bounds(place, size(px(960.0), px(640.0)), cx);
        let tracker = PlaceTracker::new(Some(bench_id), place_conn, bounds, place);
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions { title: Some(self.bench_title(&bench.name).into()), ..Default::default() }),
            window_bounds: Some(bounds),
            app_id: Some(self.app_id(&bench.launcher)),
            icon: window_icon(&Identity::of(bench, self.settings.icon_badges)),
            ..Default::default()
        };
        // Opening a window renders it at once, and its render reads this
        // state, so open it after this update ends.
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let Some(state) = this.upgrade() else { return };
            let opened = cx.update(|cx| {
                cx.open_window(options, |window, cx| cx.new(|cx| BenchWindow::new(bench_id, state, tracker, window, cx)))
            });
            match opened {
                Ok(handle) => {
                    this.update(cx, |this, cx| {
                        // Another workspace is shown since: not this one's window.
                        if this.generation != generation {
                            handle.update(cx, |_, window, _| window.remove_window()).ok();
                            return;
                        }
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
            None => self.quit_shed(cx),
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
        if !self.is_remote()
            && let Err(e) = self.desktop.install_shed(&shed)
        {
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
pub fn pick_folder(cx: &mut gpui::App) -> impl std::future::Future<Output = Result<Option<std::path::PathBuf>, String>> + use<> {
    // This PC's picker shows this PC's folders: another workspace's are
    // browsed there, in a window of Shed's own.
    let remote = cx.try_global::<AppHandle>().and_then(|h| h.0.upgrade()).and_then(|s| {
        let s = s.read(cx);
        Some((s.remote()?, s.workspace.label().to_string()))
    });
    let options = gpui::PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Open Project".into()) };
    let answer = match remote {
        Some((workspace, name)) => Err(crate::folder_browser::open(workspace, name, cx)),
        None => Ok(cx.prompt_for_paths(options)),
    };
    async move {
        match answer {
            Err(browser) => Ok(browser?.recv().await.ok().flatten()),
            Ok(answer) => match answer.await {
                Ok(Ok(paths)) => Ok(paths.and_then(|p| p.into_iter().next())),
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Ok(None),
            },
        }
    }
}

/// Whether `given` (`shed load --workspace`) names the workspace `id`
/// called `name`: its id, its name in any case, or its name's slug.
fn names_workspace(given: &str, id: &str, name: &str) -> bool {
    given == id || name.eq_ignore_ascii_case(given) || shed_model::slug(name) == shed_model::slug(given)
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

    fn bench(name: &str) -> shed_model::Bench {
        let tab = shed_model::Tab::ai("/tmp".into());
        shed_model::Bench {
            id: BenchId::new(),
            name: name.into(),
            color: shed_model::BenchColor::Blue,
            badge: None,
            cwd: "/".into(),
            active_tab: tab.id,
            tabs: vec![tab],
            aliases: Default::default(),
            closing: false,
            started: false,
            launcher: name.into(),
            window: None,
            links: Vec::new(),
        }
    }

    /// A connection whose frames the test sends, and whose messages it reads.
    struct Fake {
        conn: KeeperConn,
        sent: std::sync::mpsc::Receiver<Frame>,
        frames: async_channel::Sender<Frame>,
        rx: Option<async_channel::Receiver<Frame>>,
        closed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl Fake {
        fn new() -> Fake {
            let (tx, sent) = std::sync::mpsc::channel();
            let closed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let flag = closed.clone();
            let conn = KeeperConn::with_closer(tx, None, std::sync::Arc::new(move || flag.store(true, std::sync::atomic::Ordering::SeqCst)));
            let (frames, rx) = async_channel::unbounded();
            Fake { conn, sent, frames, rx: Some(rx), closed }
        }

        fn connection(&mut self) -> Connection {
            (self.conn.clone(), self.rx.take().unwrap())
        }

        fn keeper(&self, msg: KeeperMsg) {
            self.frames.send_blocking(Frame::Keeper(msg)).unwrap();
        }

        fn messages(&self) -> Vec<ClientMsg> {
            self.sent.try_iter().filter_map(|f| if let Frame::Client(m) = f { Some(m) } else { None }).collect()
        }
    }

    #[gpui::test]
    fn switching_workspaces_replaces_the_tree_and_the_windows(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(crate::prompt::CurrentUi(crate::theme::UiColors::default()));
            crate::theme::set_ui_scale(1.0);
        });
        // This PC, with a bench shown.
        let mut local = Fake::new();
        let state = cx.new(|_| AppState::new(local.conn.clone(), Settings::default()));
        cx.update(|cx| cx.set_global(AppHandle(state.downgrade())));
        let (conn, rx) = local.connection();
        let _ = conn;
        state.update(cx, |_, cx| cx.spawn(async move |this, cx| AppState::receive_from(this, rx, 0, cx).await).detach());
        let here = bench("here");
        let here_id = here.id;
        local.keeper(KeeperMsg::TreeChanged(Shed { benches: vec![here], ..Default::default() }));
        local.keeper(KeeperMsg::Attached { bench: here_id });
        cx.run_until_parked();
        assert!(state.read_with(cx, |s, _| s.has_window(here_id)));
        assert_eq!(cx.windows().len(), 1);

        // Another workspace, connected (with this PC's keeper as the standby).
        let mut remote = Fake::new();
        let mut standby = Fake::new();
        let target = Workspace::Remote { id: "w2".into(), name: "tower".into() };
        state.update(cx, |s, cx| s.connected_to(target.clone(), remote.connection(), Some(standby.connection()), None, cx));
        cx.run_until_parked();
        state.read_with(cx, |s, _| {
            assert!(s.is_remote() && s.tree.benches.is_empty() && !s.has_window(here_id));
            assert_eq!(s.bench_title("far"), "Shed — far · tower");
        });
        assert!(local.closed.load(std::sync::atomic::Ordering::SeqCst), "this PC's connection was let go of");
        assert_eq!(cx.windows().len(), 0, "this PC's bench window closed");
        let asked = remote.messages();
        assert!(asked.contains(&ClientMsg::Show(ShowTarget::Shed)) && asked.contains(&ClientMsg::Pegboard), "{asked:?}");
        assert!(standby.messages().contains(&ClientMsg::Status));

        // Its tree and windows.
        let far = bench("far");
        let far_id = far.id;
        remote.keeper(KeeperMsg::TreeChanged(Shed { benches: vec![far], ..Default::default() }));
        remote.keeper(KeeperMsg::Attached { bench: far_id });
        // This PC's tree changing does not show.
        standby.keeper(KeeperMsg::TreeChanged(Shed { benches: vec![bench("other")], ..Default::default() }));
        cx.run_until_parked();
        state.read_with(cx, |s, _| {
            assert_eq!(s.tree.benches.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["far"]);
            assert!(s.has_window(far_id));
            assert_eq!(s.local_running(), 1);
        });

        // A launcher of this PC's bench, while there: back to this PC, and
        // the bench shown there before comes back.
        standby.keeper(KeeperMsg::Show { bench: here_id });
        cx.run_until_parked();
        state.read_with(cx, |s, _| assert!(!s.is_remote() && !s.has_window(far_id)));
        assert!(remote.closed.load(std::sync::atomic::Ordering::SeqCst), "the tunnel was closed");
        let asked = standby.messages();
        assert!(asked.contains(&ClientMsg::Attach { bench: Some(here_id) }), "{asked:?}");
        let here = bench("here");
        standby.keeper(KeeperMsg::TreeChanged(Shed { benches: vec![shed_model::Bench { id: here_id, ..here }], ..Default::default() }));
        standby.keeper(KeeperMsg::Attached { bench: here_id });
        cx.run_until_parked();
        assert!(state.read_with(cx, |s, _| s.has_window(here_id)));

        // Frames from the closed tunnel are dropped; its end does not quit.
        remote.keeper(KeeperMsg::TreeChanged(Shed::default()));
        drop(remote);
        cx.run_until_parked();
        assert!(state.read_with(cx, |s, _| s.tree.bench(here_id).is_some()));
    }

    #[gpui::test]
    fn another_workspaces_benches_get_launchers_of_their_own(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(crate::prompt::CurrentUi(crate::theme::UiColors::default()));
            crate::theme::set_ui_scale(1.0);
        });
        let dir = tempfile::tempdir().unwrap();
        let local = Fake::new();
        let state = cx.new(|_| AppState::new(local.conn.clone(), Settings::default()));
        cx.update(|cx| cx.set_global(AppHandle(state.downgrade())));
        state.update(cx, |s, _| s.desktop = Desktop::new(dir.path().to_path_buf()));
        let mut remote = Fake::new();
        let mut standby = Fake::new();
        let target = Workspace::Remote { id: "w2".into(), name: "Tower PC".into() };
        state.update(cx, |s, cx| s.connected_to(target, remote.connection(), Some(standby.connection()), None, cx));
        cx.run_until_parked();
        let _ = remote.messages();

        // A running bench's launcher, named by the workspace, opening it there.
        let far = bench("far");
        remote.keeper(KeeperMsg::TreeChanged(Shed { benches: vec![far.clone()], ..Default::default() }));
        cx.run_until_parked();
        let desktop = Desktop::new(dir.path().to_path_buf());
        let entry = std::fs::read_to_string(desktop.entry_path_of("dev.shed.bench.tower-pc.far")).unwrap();
        assert!(entry.contains(" load --workspace w2 far\n"), "{entry}");
        assert!(entry.contains("StartupWMClass=dev.shed.bench.tower-pc.far\n"));
        assert_eq!(state.read_with(cx, |s, _| s.app_id("far")), "dev.shed.bench.tower-pc.far", "windows group with their launcher");
        assert!(!desktop.entry_path("far").exists(), "not one of this PC's");

        // Deleted there (neither running nor saved): its launcher goes; a
        // saved one's stays.
        let shed = std::path::PathBuf::from("/usr/bin/shed");
        for key in ["kept", "deleted"] {
            let identity = BenchIdentity { launcher: key, name: key, color: BenchColor::Blue, badge: None, shed: &shed, workspace: Some(shed_desktop::WorkspaceRef { id: "w2", name: "Tower PC" }) };
            desktop.install(&identity).unwrap();
        }
        let stored = vec![shed_proto::StoredBench { bench: bench("kept"), saved: String::new() }];
        remote.keeper(KeeperMsg::Pegboard(shed_proto::PegboardInfo { stored, previews: Vec::new(), order: Vec::new() }));
        cx.run_until_parked();
        assert_eq!(desktop.remote_launchers("Tower PC"), ["dev.shed.bench.tower-pc.far", "dev.shed.bench.tower-pc.kept"]);

        // Its launcher while it is shown: that keeper shows the bench (by
        // name or key, loading it if saved), whichever way the request came.
        standby.keeper(KeeperMsg::ShowInWorkspace { workspace: "tower-pc".into(), bench: "kept".into() });
        cx.run_until_parked();
        assert!(remote.messages().contains(&ClientMsg::Show(ShowTarget::Load { name: "kept".into() })));
        state.update(cx, |s, cx| s.show_in_workspace("w2".into(), "far".into(), cx));
        assert!(remote.messages().contains(&ClientMsg::Show(ShowTarget::Load { name: "far".into() })));
        assert!(state.read_with(cx, |s, _| s.is_remote()), "still there");
    }

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
}
