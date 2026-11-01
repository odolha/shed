//! The app's view of the keeper: the tree, one window per bench, and the
//! terminal view of every pane those windows show.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui::{
    AppContext, AsyncApp, Bounds, Context, Entity, TitlebarOptions, WindowBounds, WindowHandle,
    WindowOptions, px, size,
};
use shed_emu::Palette;
use shed_model::{BenchId, PaneId, Shed};
use shed_proto::{ClientMsg, Frame, KeeperMsg};
use tracing::{error, warn};

use crate::bench_window::BenchWindow;
use crate::keeper_conn::KeeperConn;
use crate::terminal_view::TerminalView;
use crate::theme::FontSettings;

/// How long the receive loop may run before letting a frame be drawn.
const FRAME_BUDGET: Duration = Duration::from_millis(8);

pub struct AppState {
    pub conn: KeeperConn,
    pub tree: Shed,
    palette: Palette,
    pub panes: HashMap<PaneId, Entity<TerminalView>>,
    windows: HashMap<BenchId, WindowHandle<BenchWindow>>,
    /// Benches asked for with `Attach`, until their window is open.
    pending: HashSet<BenchId>,
    /// Windows being opened.
    opening: HashSet<BenchId>,
}

impl AppState {
    pub fn new(conn: KeeperConn) -> Self {
        Self {
            conn,
            tree: Shed::default(),
            palette: Palette::default(),
            panes: HashMap::new(),
            windows: HashMap::new(),
            pending: HashSet::new(),
            opening: HashSet::new(),
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
            Frame::Keeper(KeeperMsg::Attached { bench }) => self.open_bench_window(bench, cx),
            Frame::Keeper(KeeperMsg::Replay { pane, cols, rows, bytes }) => {
                if let Some(view) = self.panes.get(&pane) {
                    view.update(cx, |view, cx| view.restore(cols, rows, &bytes, cx));
                }
            }
            Frame::Keeper(KeeperMsg::Error(e)) => {
                error!("keeper: {e}");
                if e.starts_with("protocol mismatch") {
                    eprintln!("shed: {e}. Run `shed stop` to restart the keeper.");
                    cx.quit();
                }
            }
            Frame::Keeper(KeeperMsg::Welcome { .. } | KeeperMsg::PaneExited { .. } | KeeperMsg::Status(_)) => {}
            Frame::Client(_) | Frame::Input { .. } => warn!("unexpected frame from keeper"),
        }
    }

    /// Opens or raises a bench's window.
    fn show(&mut self, bench: BenchId, cx: &mut Context<Self>) {
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
        let shown: HashSet<PaneId> = self
            .windows
            .keys()
            .filter_map(|b| self.tree.bench(*b))
            .flat_map(|b| &b.tabs)
            .flat_map(|t| &t.panes)
            .map(|p| p.id)
            .collect();
        self.panes.retain(|id, _| shown.contains(id));
        for id in shown {
            if !self.panes.contains_key(&id) {
                let view = self.new_view(id, cx);
                self.panes.insert(id, view);
            }
        }

        for pane in self.tree.benches.iter().flat_map(|b| &b.tabs).flat_map(|t| &t.panes) {
            if let Some(view) = self.panes.get(&pane.id)
                && view.read(cx).cwd != pane.cwd
            {
                view.update(cx, |view, _| view.cwd = pane.cwd.clone());
            }
        }

        let benches: Vec<BenchId> = self.windows.keys().copied().collect();
        for bench in benches {
            self.sync_window(bench, cx);
        }
        cx.notify();
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
        let (conn, palette) = (self.conn.clone(), self.palette.clone());
        cx.new(|cx| TerminalView::new(pane, conn, palette, FontSettings::default(), cx))
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
            app_id: Some("dev.shed.Shed".into()),
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

    /// Called when a bench's window closes: its tools keep running in the keeper.
    pub fn window_closed(&mut self, bench: BenchId) {
        if self.windows.remove(&bench).is_some() {
            self.conn.send_msg(ClientMsg::Detach { bench });
        }
        let shown: HashSet<PaneId> = self
            .windows
            .keys()
            .filter_map(|b| self.tree.bench(*b))
            .flat_map(|b| &b.tabs)
            .flat_map(|t| &t.panes)
            .map(|p| p.id)
            .collect();
        self.panes.retain(|id, _| shown.contains(id));
    }

    /// Bench windows in tree order, for Super+1…9.
    pub fn window_at(&self, index: usize) -> Option<WindowHandle<BenchWindow>> {
        self.tree.benches.iter().filter_map(|b| self.windows.get(&b.id)).nth(index).copied()
    }
}

pub fn window_title(bench: &str) -> String {
    format!("{bench} — Shed")
}
