//! The app's view of the keeper: the tree, the open bench windows, and
//! routing of output to panes.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{
    AppContext, AsyncApp, Bounds, Context, Entity, TitlebarOptions, WeakEntity, WindowBounds, WindowHandle,
    WindowOptions, px, size,
};
use shed_emu::Palette;
use shed_model::{BenchId, PaneId, Shed};
use shed_proto::{Frame, KeeperMsg};
use tracing::{error, warn};

use crate::bench_window::BenchWindow;
use crate::keeper_conn::KeeperConn;
use crate::terminal_view::TerminalView;

/// How long the receive loop may run before letting a frame be drawn.
const FRAME_BUDGET: Duration = Duration::from_millis(8);

pub struct AppState {
    conn: KeeperConn,
    tree: Shed,
    palette: Palette,
    panes: HashMap<PaneId, WeakEntity<TerminalView>>,
    windows: HashMap<BenchId, WindowHandle<BenchWindow>>,
}

impl AppState {
    pub fn new(conn: KeeperConn) -> Self {
        Self { conn, tree: Shed::default(), palette: Palette::default(), panes: HashMap::new(), windows: HashMap::new() }
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
                if let Some(view) = self.panes.get(&pane).and_then(|v| v.upgrade()) {
                    view.update(cx, |view, cx| view.feed(&bytes, cx));
                }
            }
            Frame::Keeper(KeeperMsg::TreeChanged(tree)) => {
                self.tree = tree;
                self.sync_windows(cx);
            }
            Frame::Keeper(KeeperMsg::Attached { bench }) => self.open_bench_window(bench, cx),
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

    /// Closes windows whose bench is gone.
    fn sync_windows(&mut self, cx: &mut Context<Self>) {
        let gone: Vec<BenchId> = self.windows.keys().filter(|b| self.tree.bench(**b).is_none()).copied().collect();
        for bench in gone {
            if let Some(handle) = self.windows.remove(&bench) {
                handle.update(cx, |_, window, _| window.remove_window()).ok();
            }
        }
        self.panes.retain(|pane, view| view.upgrade().is_some() && self.tree.pane(*pane).is_some());
    }

    fn open_bench_window(&mut self, bench_id: BenchId, cx: &mut Context<Self>) {
        if let Some(handle) = self.windows.get(&bench_id) {
            handle.update(cx, |_, window, _| window.activate()).ok();
            return;
        }
        let Some(bench) = self.tree.bench(bench_id) else { return };
        let Some(pane) = bench.active_tab().map(|t| t.active_pane) else { return };
        let (color, conn, palette) = (bench.color, self.conn.clone(), self.palette.clone());
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions { title: Some(format!("{} — Shed", bench.name).into()), ..Default::default() }),
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(960.0), px(600.0)), cx))),
            app_id: Some("dev.shed.Shed".into()),
            ..Default::default()
        };
        let opened = cx.open_window(options, |window, cx| {
            cx.new(|cx| BenchWindow::new(color, pane, conn, palette, window, cx))
        });
        match opened {
            Ok(handle) => {
                if let Ok(view) = handle.update(cx, |bw, _, _| bw.terminal.downgrade()) {
                    self.panes.insert(pane, view);
                }
                self.windows.insert(bench_id, handle);
            }
            Err(e) => error!("cannot open a window: {e}"),
        }
    }
}
