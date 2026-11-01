//! The app's view of the keeper: the tree, one window per bench, and the
//! terminal view of every pane those windows show.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui::{
    AppContext, AsyncApp, Bounds, Context, Entity, TitlebarOptions, WindowBounds, WindowHandle,
    WindowOptions, px, size,
};
use shed_model::config::{Config, FileWatch};
use shed_desktop::{BenchIdentity, Desktop};
use shed_model::{Bench, BenchColor, BenchId, PaneId, Shed};
use shed_proto::{ClientMsg, Frame, KeeperMsg};
use tracing::{error, warn};

use crate::bench_window::BenchWindow;
use crate::keeper_conn::KeeperConn;
use crate::pegboard::PegboardData;
use crate::shed_window::ShedWindow;
use crate::terminal_view::TerminalView;
use crate::settings::Settings;

/// How long the receive loop may run before letting a frame be drawn.
const FRAME_BUDGET: Duration = Duration::from_millis(8);

pub struct AppState {
    pub conn: KeeperConn,
    pub tree: Shed,
    pub settings: Settings,
    pub panes: HashMap<PaneId, Entity<TerminalView>>,
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
        Self {
            conn,
            tree: Shed::default(),
            settings,
            panes: HashMap::new(),
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
            Frame::Keeper(KeeperMsg::Pegboard(info)) => {
                // A global, so only the pegboard redraws, not every window.
                if cx.try_global::<PegboardData>().is_none_or(|d| d.0 != info) {
                    cx.set_global(PegboardData(info));
                }
            }
            Frame::Keeper(KeeperMsg::Attached { bench }) => self.open_bench_window(bench, cx),
            Frame::Keeper(KeeperMsg::Replay { pane, cols, rows, bytes }) => {
                if let Some(view) = self.panes.get(&pane) {
                    view.update(cx, |view, cx| view.restore(cols, rows, &bytes, cx));
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

        // A view for every pane of every open bench; none for anything else.
        let shown = self.shown_panes();
        self.panes.retain(|id, _| shown.contains(id));
        for id in shown {
            if !self.panes.contains_key(&id) {
                let view = self.new_view(id, cx);
                self.panes.insert(id, view);
            }
        }

        for bench in &self.tree.benches {
            for tab in &bench.tabs {
                let here = Some(shed_logbook::Context { tab: tab.id, bench: bench.id });
                let accent = bench.color.accent();
                for pane in &tab.panes {
                    let Some(view) = self.panes.get(&pane.id) else { continue };
                    let v = view.read(cx);
                    if v.cwd != pane.cwd || v.here.map(|h| (h.tab, h.bench)) != Some((tab.id, bench.id)) || v.accent != accent {
                        view.update(cx, |view, cx| {
                            view.cwd = pane.cwd.clone();
                            view.here = here;
                            view.accent = accent;
                            cx.notify();
                        });
                    }
                }
            }
        }

        self.sync_identities(cx);
        let benches: Vec<BenchId> = self.windows.keys().copied().collect();
        for bench in benches {
            self.sync_window(bench, cx);
        }
        cx.notify();
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
        let active = b
            .active_tab()
            .and_then(|t| self.panes.get(&t.active_pane).map(|v| (t.active_pane, v.read(cx).focus_handle().clone())));
        handle.update(cx, |bw, window, cx| bw.sync(title, active, window, cx)).ok();
    }

    fn new_view(&self, pane: PaneId, cx: &mut Context<Self>) -> Entity<TerminalView> {
        let (conn, settings) = (self.conn.clone(), self.settings.clone());
        cx.new(|cx| TerminalView::new(pane, conn, &settings, cx))
    }

    /// Applies a reloaded config to keys, panes and windows.
    pub fn apply_settings(&mut self, mut settings: Settings, cx: &mut Context<Self>) {
        settings.errors.extend(crate::actions::bind_keys(cx, &settings.keys));
        for e in &settings.errors {
            warn!("config: {e}");
        }
        cx.set_global(crate::prompt::CurrentUi(settings.ui.clone()));
        self.settings = settings;
        self.sync_identities(cx);
        for view in self.panes.values() {
            view.update(cx, |view, cx| view.apply(&self.settings, cx));
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
        let Some(bench) = self.tree.bench(bench_id) else { return };
        if !self.opening.insert(bench_id) {
            return;
        }
        // Views first, so the replays that follow `Attached` find them.
        let ids: Vec<PaneId> = bench.tabs.iter().flat_map(|t| &t.panes).map(|p| p.id).collect();
        for id in ids {
            if !self.panes.contains_key(&id) {
                let view = self.new_view(id, cx);
                self.panes.insert(id, view);
            }
        }
        let bench = self.tree.bench(bench_id).unwrap();
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions { title: Some(window_title(&bench.name).into()), ..Default::default() }),
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(960.0), px(640.0)), cx))),
            app_id: Some(shed_desktop::app_id(&bench.launcher)),
            icon: window_icon(&Identity::of(bench, self.settings.icon_badges)),
            ..Default::default()
        };
        // Opening a window renders it at once, and its render reads this
        // state, so open it after this update ends.
        cx.spawn(async move |this, cx| {
            let Some(state) = this.upgrade() else { return };
            let opened = cx.update(|cx| {
                cx.open_window(options, |window, cx| cx.new(|cx| BenchWindow::new(bench_id, state, window, cx)))
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
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions { title: Some("Shed".into()), ..Default::default() }),
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1120.0), px(740.0)), cx))),
            app_id: Some(shed_desktop::SHED_APP_ID.into()),
            icon: rasterize(shed_desktop::SHED_ICON),
            ..Default::default()
        };
        // As with bench windows: its render reads this state, so open it after this update.
        cx.spawn(async move |this, cx| {
            let Some(state) = this.upgrade() else { return };
            let opened = cx.update(|cx| cx.open_window(options, |window, cx| cx.new(|cx| ShedWindow::new(state, window, cx))));
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
            handle.update(cx, |_, window, _| window.remove_window()).ok();
        }
        self.window_closed(bench);
        cx.notify();
    }

    /// Called when a bench's window closes: its tools keep running in the keeper.
    pub fn window_closed(&mut self, bench: BenchId) {
        if self.windows.remove(&bench).is_some() {
            self.conn.send_msg(ClientMsg::Detach { bench });
        }
        let shown = self.shown_panes();
        self.panes.retain(|id, _| shown.contains(id));
    }

    /// The panes of benches with a window, or one being opened: a window
    /// opens a moment after its bench attaches, and the replays that arrive
    /// meanwhile must find their views.
    fn shown_panes(&self) -> HashSet<PaneId> {
        self.windows
            .keys()
            .chain(&self.opening)
            .filter_map(|b| self.tree.bench(*b))
            .flat_map(|b| &b.tabs)
            .flat_map(|t| &t.panes)
            .map(|p| p.id)
            .collect()
    }

    pub fn has_window(&self, bench: BenchId) -> bool {
        self.windows.contains_key(&bench)
    }

    /// Bench windows in tree order, for Super+1…9.
    pub fn window_at(&self, index: usize) -> Option<WindowHandle<BenchWindow>> {
        self.tree.benches.iter().filter_map(|b| self.windows.get(&b.id)).nth(index).copied()
    }
}

pub fn window_title(bench: &str) -> String {
    format!("{bench} — Shed")
}

/// The bench icon as a window icon (X11 `_NET_WM_ICON`, Wayland `xdg-toplevel-icon`).
fn window_icon(identity: &Identity) -> Option<std::sync::Arc<image::RgbaImage>> {
    rasterize(&shed_desktop::icon_svg(identity.color, identity.badge.as_deref()))
}

fn rasterize(svg: &str) -> Option<std::sync::Arc<image::RgbaImage>> {
    const SIZE: u32 = 128;
    let rgba = shed_desktop::icon_rgba(svg, SIZE)?;
    image::RgbaImage::from_raw(SIZE, SIZE, rgba).map(std::sync::Arc::new)
}
