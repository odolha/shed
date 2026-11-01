//! The keeper's state and socket server.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use shed_emu::binary::ReplyGuard;
use shed_emu::{AlacrittyEmulator, Emulator, GridSize, Palette};
use shed_emu::EmuEvent;
use shed_emu::marks::{Mark, MarkScanner};
use shed_logbook::{Logbook, NewEntry};
use shed_model::config::{Config, FileWatch};
use shed_model::{Bench, BenchColor, BenchId, Pane, PaneId, Shed, Tab, TabId};
use shed_proto::{
    ClientKind, ClientMsg, Frame, FrameDecoder, KeeperMsg, KeeperStatus, PROTOCOL_VERSION,
    ShowTarget,
};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::Child;
use tokio::sync::{Notify, mpsc};
use tracing::{debug, info, warn};

use crate::procinfo;
use crate::pty::{self, SpawnOptions};

/// How long a pane keeps draining output after its shell exits, for
/// background jobs that still hold the terminal open.
const EXIT_DRAIN: Duration = Duration::from_millis(200);

/// How often each pane's tool and directory are read from `/proc`.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Tree changes from output (titles) are batched for this long before broadcast.
const TREE_FLUSH_DELAY: Duration = Duration::from_millis(50);

pub struct KeeperConfig {
    pub socket: PathBuf,
    /// `shed.toml`, reloaded when it changes.
    pub config_file: PathBuf,
    /// The shell for new panes; `None` uses the user's login shell.
    pub shell: Option<PathBuf>,
    /// Where new benches open.
    pub home: PathBuf,
    /// Exit once no panes and no clients remain.
    pub exit_when_idle: bool,
    /// The logbook database; `None` keeps no logbook.
    pub logbook: Option<PathBuf>,
    /// Shed's state directory, where per-tab history files go.
    pub state_dir: PathBuf,
    /// The `shed` binary the shell hook calls to walk history.
    pub shed_bin: Option<PathBuf>,
    /// Extra environment for every pane (tests use it for a private HOME).
    pub env: Vec<(OsString, OsString)>,
}

type ClientId = u64;

pub struct Keeper {
    cfg: KeeperConfig,
    state: Mutex<State>,
    shutdown: Notify,
    stopping: AtomicBool,
    /// Wakes the task that broadcasts batched tree changes.
    tree_dirty: Notify,
    /// Finished commands, on their way to the logbook writer thread.
    logbook: Option<std::sync::mpsc::Sender<NewEntry>>,
    /// The bash startup file that loads ~/.bashrc and then Shed's hook.
    bash_rc: Option<PathBuf>,
    host: String,
}

struct State {
    config: Config,
    config_watch: FileWatch,
    tree: Shed,
    panes: HashMap<PaneId, PaneRt>,
    clients: HashMap<ClientId, ClientRt>,
    next_client: ClientId,
    started: Instant,
    palette: Palette,
}

struct PaneRt {
    bench: BenchId,
    /// The keeper's own copy of the screen: answers queries now, replays it on reattach later.
    emu: AlacrittyEmulator,
    /// Keeps replies to queries that binary output happens to contain out of the shell's input.
    reply_guard: ReplyGuard,
    input: mpsc::UnboundedSender<Vec<u8>>,
    master: Arc<AsyncFd<OwnedFd>>,
    pid: Option<u32>,
    /// Shell-integration marks in the output.
    marks: MarkScanner,
    /// The command running, from its mark until it finishes.
    running: Option<Running>,
}

struct Running {
    command: String,
    cwd: String,
    started_ms: i64,
    started: Instant,
}

struct ClientRt {
    kind: ClientKind,
    tx: mpsc::UnboundedSender<Arc<[u8]>>,
    attached: HashSet<BenchId>,
}

impl State {
    fn send(&self, client: ClientId, msg: KeeperMsg) {
        if let Some(c) = self.clients.get(&client) {
            let _ = c.tx.send(Frame::Keeper(msg).encode().into());
        }
    }

    fn send_replay(&mut self, client: ClientId, pane: PaneId) {
        let Some(rt) = self.panes.get_mut(&pane) else { return };
        let size = rt.emu.size();
        let bytes = rt.emu.replay();
        debug!(?pane, len = bytes.len(), "replay");
        self.send(client, KeeperMsg::Replay { pane, cols: size.cols, rows: size.rows, bytes });
    }

    /// Hangs up panes' sessions, as closing a terminal would. They leave the
    /// tree as their shells exit.
    fn hang_up(&self, panes: &[PaneId]) {
        use nix::sys::signal::{Signal, kill};
        use nix::unistd::Pid;
        for pane in panes {
            let Some(rt) = self.panes.get(pane) else { continue };
            // Negative pids: whole process groups, the shell's and the foreground job's.
            if let Some(pgrp) = pty::foreground_pgrp(rt.master.get_ref()) {
                let _ = kill(Pid::from_raw(-pgrp.as_raw()), Signal::SIGHUP);
            }
            if let Some(pid) = rt.pid {
                let _ = kill(Pid::from_raw(-(pid as i32)), Signal::SIGHUP);
            }
        }
    }

    fn broadcast_tree(&self) {
        let frame: Arc<[u8]> = Frame::Keeper(KeeperMsg::TreeChanged(self.tree.clone())).encode().into();
        for c in self.clients.values().filter(|c| c.kind == ClientKind::Window) {
            let _ = c.tx.send(frame.clone());
        }
    }

    fn broadcast_to_bench(&self, bench: BenchId, frame: Arc<[u8]>) {
        for c in self.clients.values().filter(|c| c.attached.contains(&bench)) {
            let _ = c.tx.send(frame.clone());
        }
    }

    /// The window clients showing `bench`.
    fn is_open(&self, bench: BenchId) -> bool {
        self.clients.values().any(|c| c.kind == ClientKind::Window && c.attached.contains(&bench))
    }

    /// The window process to show benches in: the asking client if it is one,
    /// else any connected one.
    fn window_client(&self, asking: ClientId) -> Option<ClientId> {
        let is_window = |id: &ClientId| self.clients.get(id).is_some_and(|c| c.kind == ClientKind::Window);
        if is_window(&asking) {
            return Some(asking);
        }
        self.clients.keys().copied().filter(is_window).min()
    }

    fn pane_rt_ids_in(&self, bench: BenchId) -> Vec<PaneId> {
        self.panes.iter().filter(|(_, rt)| rt.bench == bench).map(|(id, _)| *id).collect()
    }

    fn unique_bench_name(&self) -> String {
        let taken = |n: &str| self.tree.benches.iter().any(|b| b.name == n);
        if !taken("main") {
            return "main".into();
        }
        (2..).map(|i| format!("bench-{i}")).find(|n| !taken(n)).unwrap()
    }
}

impl Keeper {
    pub fn new(cfg: KeeperConfig) -> Arc<Self> {
        let logbook = cfg.logbook.as_ref().and_then(|path| match Logbook::open(path) {
            Ok(book) => {
                let (tx, rx) = std::sync::mpsc::channel::<NewEntry>();
                std::thread::Builder::new()
                    .name("logbook".into())
                    .spawn(move || {
                        while let Ok(entry) = rx.recv() {
                            if let Err(e) = book.insert(&entry) {
                                warn!("logbook write failed: {e}");
                            }
                        }
                    })
                    .ok()?;
                Some(tx)
            }
            Err(e) => {
                warn!("no logbook ({}): {e}", path.display());
                None
            }
        });
        let bash_rc = write_bash_rc(cfg.socket.parent().unwrap_or(Path::new("/tmp")));
        let host = nix::unistd::gethostname().ok().and_then(|h| h.into_string().ok()).unwrap_or_default();
        Arc::new(Self {
            cfg,
            state: Mutex::new(State {
                config: Config::default(),
                config_watch: FileWatch::default(),
                tree: Shed::default(),
                panes: HashMap::new(),
                clients: HashMap::new(),
                next_client: 1,
                started: Instant::now(),
                palette: Palette::default(),
            }),
            shutdown: Notify::new(),
            stopping: AtomicBool::new(false),
            tree_dirty: Notify::new(),
            logbook,
            bash_rc,
            host,
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    /// Accepts clients until a client asks for shutdown or the keeper goes idle.
    pub async fn serve(self: Arc<Self>, listener: UnixListener) -> io::Result<()> {
        let poll = tokio::spawn(self.clone().poll_loop());
        let flush = tokio::spawn(self.clone().flush_loop());
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (stream, _) = accepted?;
                    let keeper = self.clone();
                    tokio::spawn(async move {
                        if let Err(e) = keeper.clone().handle_client(stream).await {
                            debug!("client ended: {e}");
                        }
                    });
                }
                _ = self.shutdown.notified() => break,
            }
        }
        poll.abort();
        flush.abort();
        self.hang_up_all();
        Ok(())
    }

    /// Keeps each pane's tool and working directory current.
    async fn poll_loop(self: Arc<Self>) {
        let mut tick = tokio::time::interval(POLL_INTERVAL);
        loop {
            tick.tick().await;
            let mut st = self.lock();
            let mut changed = false;
            if st.config_watch.changed(&self.cfg.config_file) {
                match Config::load_from(&self.cfg.config_file) {
                    Ok(config) => {
                        let (theme, error) = config.theme();
                        if let Some(e) = error {
                            warn!("{e}");
                        }
                        st.palette = Palette::from_theme(&theme);
                        let palette = st.palette.clone();
                        for rt in st.panes.values_mut() {
                            rt.emu.set_palette(palette.clone());
                        }
                        st.config = config;
                        info!("config loaded");
                    }
                    Err(e) => warn!("config not loaded: {e}"),
                }
            }
            let ids: Vec<PaneId> = st.panes.keys().copied().collect();
            for id in ids {
                let rt = &st.panes[&id];
                let Some(pid) = rt.pid else { continue };
                let tool = procinfo::foreground_tool(rt.master.get_ref(), pid);
                let cwd = procinfo::cwd(pid);
                if let Some(pane) = st.tree.pane_mut(id) {
                    if pane.tool != tool {
                        pane.tool = tool;
                        changed = true;
                    }
                    if let Some(cwd) = cwd
                        && pane.cwd != cwd
                    {
                        pane.cwd = cwd;
                        changed = true;
                    }
                }
            }
            // Tag rules, over each tab's tools and directories.
            let State { tree, config, .. } = &mut *st;
            for tab in tree.benches.iter_mut().flat_map(|b| &mut b.tabs) {
                let tags = config.auto_tags(tab.panes.iter().map(|p| (p.tool.as_ref(), p.cwd.as_path())));
                if tab.auto_tags != tags {
                    tab.auto_tags = tags;
                    changed = true;
                }
            }
            if changed {
                st.broadcast_tree();
            }
        }
    }

    /// Broadcasts tree changes that output caused, batched.
    async fn flush_loop(self: Arc<Self>) {
        loop {
            self.tree_dirty.notified().await;
            tokio::time::sleep(TREE_FLUSH_DELAY).await;
            self.lock().broadcast_tree();
        }
    }

    /// Asks every pane's session to end, as closing a terminal would.
    fn hang_up_all(&self) {
        let st = self.lock();
        let all: Vec<PaneId> = st.panes.keys().copied().collect();
        st.hang_up(&all);
    }

    fn check_idle(&self) {
        if !self.cfg.exit_when_idle {
            return;
        }
        let st = self.lock();
        if st.panes.is_empty() && st.clients.is_empty() && !self.stopping.swap(true, Ordering::Relaxed) {
            info!("nothing left to keep; exiting");
            self.shutdown.notify_one();
        }
    }

    async fn handle_client(self: Arc<Self>, stream: UnixStream) -> anyhow::Result<()> {
        let (mut rd, mut wr) = stream.into_split();
        let (tx, mut rx) = mpsc::unbounded_channel::<Arc<[u8]>>();
        let id = {
            let mut st = self.lock();
            let id = st.next_client;
            st.next_client += 1;
            st.clients.insert(id, ClientRt { kind: ClientKind::Cli, tx, attached: HashSet::new() });
            id
        };
        debug!(client = id, "connected");

        let writer = tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                if wr.write_all(&frame).await.is_err() {
                    break;
                }
            }
        });

        let mut decoder = FrameDecoder::new();
        let mut buf = vec![0u8; 64 * 1024];
        let result = async {
            loop {
                let n = rd.read(&mut buf).await?;
                if n == 0 {
                    return Ok(());
                }
                decoder.push(&buf[..n]);
                while let Some(frame) = decoder.next_frame()? {
                    self.handle_frame(id, frame);
                }
            }
        }
        .await;

        self.lock().clients.remove(&id);
        // Dropping the sender ends the writer after it flushes.
        let _ = writer.await;
        debug!(client = id, "disconnected");
        self.check_idle();
        result
    }

    fn handle_frame(self: &Arc<Self>, client: ClientId, frame: Frame) {
        match frame {
            Frame::Input { pane, bytes } => {
                let st = self.lock();
                if let Some(rt) = st.panes.get(&pane) {
                    let _ = rt.input.send(bytes);
                }
            }
            Frame::Client(msg) => self.handle_msg(client, msg),
            Frame::Keeper(_) | Frame::Output { .. } => warn!(client, "unexpected frame from client"),
        }
    }

    fn handle_msg(self: &Arc<Self>, client: ClientId, msg: ClientMsg) {
        match msg {
            ClientMsg::Hello { version, kind } => {
                let mut st = self.lock();
                if let Some(c) = st.clients.get_mut(&client) {
                    c.kind = kind;
                }
                if version != PROTOCOL_VERSION {
                    st.send(client, KeeperMsg::Error(format!(
                        "protocol mismatch: keeper speaks {PROTOCOL_VERSION}, client {version}"
                    )));
                }
                st.send(client, KeeperMsg::Welcome {
                    version: PROTOCOL_VERSION,
                    keeper_pid: std::process::id(),
                });
            }
            ClientMsg::Attach { bench } => {
                let mut st = self.lock();
                let bench = match bench.or_else(|| st.tree.benches.first().map(|b| b.id)) {
                    Some(id) if st.tree.bench(id).is_some() => id,
                    Some(id) => {
                        st.send(client, KeeperMsg::Error(format!("no bench {id}")));
                        return;
                    }
                    None => match self.new_bench(&mut st, None, None) {
                        Ok(id) => id,
                        Err(e) => {
                            st.send(client, KeeperMsg::Error(format!("cannot start a shell: {e}")));
                            return;
                        }
                    },
                };
                if let Some(c) = st.clients.get_mut(&client) {
                    c.attached.insert(bench);
                }
                st.broadcast_tree();
                st.send(client, KeeperMsg::Attached { bench });
                // Still under the lock: each replay precedes that pane's next output.
                for pane in st.pane_rt_ids_in(bench) {
                    st.send_replay(client, pane);
                }
                info!(client, ?bench, "attached");
            }
            ClientMsg::Replay { pane } => self.lock().send_replay(client, pane),
            ClientMsg::Detach { bench } => {
                let mut st = self.lock();
                if let Some(c) = st.clients.get_mut(&client) {
                    c.attached.remove(&bench);
                }
                info!(client, ?bench, "detached");
            }
            ClientMsg::Resize { pane, cols, rows, cell_width, cell_height } => {
                let mut st = self.lock();
                if let Some(rt) = st.panes.get_mut(&pane) {
                    let size = GridSize::new(cols, rows).with_cell(cell_width, cell_height);
                    rt.emu.resize(size);
                    let size = rt.emu.size();
                    if let Err(e) = pty::resize(rt.master.get_ref().as_fd(), size.cols, size.rows, cell_width, cell_height) {
                        warn!(?pane, "resize failed: {e}");
                    }
                }
                if let Some(p) = st.tree.pane_mut(pane) {
                    p.cols = cols;
                    p.rows = rows;
                }
            }
            ClientMsg::Show(target) => self.show(client, target),
            ClientMsg::NewTab { bench, cwd } => {
                let mut st = self.lock();
                let Some(b) = st.tree.bench(bench) else { return };
                let cwd = cwd.unwrap_or_else(|| b.cwd.clone());
                let index = b.tab_index(b.active_tab).map_or(b.tabs.len(), |i| i + 1);
                let tab_id = TabId::new();
                match self.spawn_pane(&mut st, bench, tab_id, cwd) {
                    Ok(pane) => {
                        let mut tab = Tab::with_pane(pane);
                        tab.id = tab_id;
                        let b = st.tree.bench_mut(bench).unwrap();
                        b.tabs.insert(index, tab);
                        b.active_tab = tab_id;
                        st.broadcast_tree();
                    }
                    Err(e) => st.send(client, KeeperMsg::Error(format!("cannot start a shell: {e}"))),
                }
            }
            ClientMsg::Split { pane, dir, cwd } => {
                let mut st = self.lock();
                let Some((bench, tab)) = st.tree.locate_pane(pane) else { return };
                let cwd = cwd.unwrap_or_else(|| st.tree.bench(bench).unwrap().cwd.clone());
                match self.spawn_pane(&mut st, bench, tab, cwd) {
                    Ok(new) => {
                        st.tree.tab_mut(tab).unwrap().split(pane, new, dir);
                        st.broadcast_tree();
                    }
                    Err(e) => st.send(client, KeeperMsg::Error(format!("cannot start a shell: {e}"))),
                }
            }
            ClientMsg::ClosePane { pane } => self.lock().hang_up(&[pane]),
            ClientMsg::CloseTab { tab } => {
                let st = self.lock();
                if let Some(t) = st.tree.tab(tab) {
                    let panes: Vec<PaneId> = t.panes.iter().map(|p| p.id).collect();
                    st.hang_up(&panes);
                }
            }
            ClientMsg::CloseBench { bench } => {
                let st = self.lock();
                info!(?bench, "closing bench");
                let panes = st.pane_rt_ids_in(bench);
                st.hang_up(&panes);
            }
            ClientMsg::FocusPane { pane } => {
                let mut st = self.lock();
                if st.tree.focus_pane(pane) {
                    st.broadcast_tree();
                }
            }
            ClientMsg::FocusTab { tab } => {
                let mut st = self.lock();
                if let Some(bench) = st.tree.bench_of_tab(tab) {
                    st.tree.bench_mut(bench).unwrap().active_tab = tab;
                    st.broadcast_tree();
                }
            }
            ClientMsg::SetLayout { tab, layout } => {
                let mut st = self.lock();
                let Some(t) = st.tree.tab_mut(tab) else { return };
                let mut old = t.layout.leaves();
                let mut new = layout.leaves();
                old.sort();
                new.sort();
                if old == new {
                    t.layout = layout.clamped();
                    st.broadcast_tree();
                } else {
                    st.send(client, KeeperMsg::Error("layout does not match the tab's panes".into()));
                }
            }
            ClientMsg::RenameBench { bench, name } => {
                let mut st = self.lock();
                let name = name.trim().to_string();
                let taken = st.tree.benches.iter().any(|b| b.id != bench && b.name == name);
                if name.is_empty() || taken {
                    st.send(client, KeeperMsg::Error(format!("cannot name a bench {name:?}")));
                } else if let Some(b) = st.tree.bench_mut(bench) {
                    b.name = name;
                    st.broadcast_tree();
                }
            }
            ClientMsg::RenameTab { tab, name } => self.edit_tab(tab, |t| t.name = name.filter(|n| !n.trim().is_empty())),
            ClientMsg::SetTags { tab, tags } => self.edit_tab(tab, |t| t.tags = tags),
            ClientMsg::SetPrivate { tab, private } => self.edit_tab(tab, |t| t.private = private),
            ClientMsg::SetBenchColor { bench, color } => self.edit_bench(bench, |b| b.color = color),
            ClientMsg::SetBadge { bench, badge } => self.edit_bench(bench, |b| b.badge = badge),
            ClientMsg::MoveTab { tab, bench, index } => {
                let mut st = self.lock();
                let Some(from) = st.tree.bench_of_tab(tab) else { return };
                if st.tree.move_tab(tab, bench, index) {
                    if from != bench {
                        // The tab's panes now belong to another bench's windows.
                        let ids: Vec<PaneId> = st.tree.tab(tab).unwrap().panes.iter().map(|p| p.id).collect();
                        for id in &ids {
                            if let Some(rt) = st.panes.get_mut(id) {
                                rt.bench = bench;
                            }
                        }
                        st.broadcast_tree();
                        let targets: Vec<ClientId> = st.clients.iter().filter(|(_, c)| c.attached.contains(&bench)).map(|(id, _)| *id).collect();
                        for c in targets {
                            for id in &ids {
                                st.send_replay(c, *id);
                            }
                        }
                    } else {
                        st.broadcast_tree();
                    }
                }
            }
            ClientMsg::Status => {
                let st = self.lock();
                let status = KeeperStatus {
                    pid: std::process::id(),
                    version: env!("CARGO_PKG_VERSION").into(),
                    uptime_secs: st.started.elapsed().as_secs(),
                    clients: st.clients.len() as u32,
                    windows: st.clients.values().filter(|c| c.kind == ClientKind::Window).count() as u32,
                    open: st.tree.benches.iter().map(|b| b.id).filter(|b| st.is_open(*b)).collect(),
                    tree: st.tree.clone(),
                };
                st.send(client, KeeperMsg::Status(status));
            }
            ClientMsg::Shutdown => {
                info!(client, "shutdown requested");
                self.shutdown.notify_one();
            }
        }
    }

    fn edit_tab(&self, tab: TabId, edit: impl FnOnce(&mut Tab)) {
        let mut st = self.lock();
        if let Some(t) = st.tree.tab_mut(tab) {
            edit(t);
            st.broadcast_tree();
        }
    }

    fn edit_bench(&self, bench: BenchId, edit: impl FnOnce(&mut Bench)) {
        let mut st = self.lock();
        if let Some(b) = st.tree.bench_mut(bench) {
            edit(b);
            st.broadcast_tree();
        }
    }

    /// Resolves what to show and tells the window process to show it.
    fn show(self: &Arc<Self>, client: ClientId, target: ShowTarget) {
        let mut st = self.lock();
        let benches = match target {
            ShowTarget::Bench(b) if st.tree.bench(b).is_some() => vec![b],
            ShowTarget::Bench(b) => {
                st.send(client, KeeperMsg::Error(format!("no bench {b}")));
                return;
            }
            ShowTarget::Detached => {
                let detached: Vec<BenchId> = st.tree.benches.iter().map(|b| b.id).filter(|b| !st.is_open(*b)).collect();
                if detached.is_empty() { self.new_bench(&mut st, None, None).into_iter().collect() } else { detached }
            }
            ShowTarget::NewBench { name, cwd } => match self.new_bench(&mut st, name, cwd) {
                Ok(b) => vec![b],
                Err(e) => {
                    st.send(client, KeeperMsg::Error(format!("cannot start a shell: {e}")));
                    return;
                }
            },
        };
        st.broadcast_tree();
        match st.window_client(client) {
            Some(window) => {
                for bench in benches {
                    st.send(window, KeeperMsg::Show { bench });
                }
            }
            None => st.send(client, KeeperMsg::Error("no window is open to show it in".into())),
        }
    }

    fn new_bench(
        self: &Arc<Self>,
        st: &mut State,
        name: Option<String>,
        cwd: Option<PathBuf>,
    ) -> io::Result<BenchId> {
        let id = BenchId::new();
        let cwd = cwd.unwrap_or_else(|| self.cfg.home.clone());
        let tab_id = TabId::new();
        let pane = self.spawn_pane(st, id, tab_id, cwd.clone())?;
        let mut tab = Tab::with_pane(pane);
        tab.id = tab_id;
        let name = name.unwrap_or_else(|| st.unique_bench_name());
        let color = BenchColor::next_unused(st.tree.benches.iter().map(|b| &b.color));
        st.tree.benches.push(Bench {
            id,
            name,
            color,
            badge: None,
            cwd,
            tabs: vec![tab],
            active_tab: tab_id,
        });
        Ok(id)
    }

    fn spawn_pane(
        self: &Arc<Self>,
        st: &mut State,
        bench: BenchId,
        tab: TabId,
        cwd: PathBuf,
    ) -> io::Result<Pane> {
        let pane_id = PaneId::new();
        let (cols, rows) = (80, 24);
        let program = self.cfg.shell.clone().unwrap_or_else(pty::default_shell);
        let env: Vec<(OsString, OsString)> = [
            ("TERM", "xterm-256color".to_string()),
            ("COLORTERM", "truecolor".to_string()),
            ("TERM_PROGRAM", "shed".to_string()),
            ("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION").to_string()),
            ("SHED_PANE", pane_id.to_string()),
            ("SHED_TAB", tab.to_string()),
            ("SHED_BENCH", bench.to_string()),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();

        let mut env = env;
        env.extend(self.cfg.env.iter().cloned());
        env.push(("SHED_STATE_DIR".into(), self.cfg.state_dir.clone().into()));
        if let Some(bin) = &self.cfg.shed_bin {
            env.push(("SHED_BIN".into(), bin.clone().into()));
        }
        let mut args = Vec::new();
        let is_bash = program.file_name().is_some_and(|n| n == "bash");
        if is_bash
            && st.config.shell.integration
            && let Some(rc) = &self.bash_rc
        {
            args = vec!["--rcfile".into(), rc.clone().into()];
        }

        let cwd = if cwd.is_dir() { cwd } else { self.cfg.home.clone() };
        let pty = pty::spawn(SpawnOptions { program, args, cwd: cwd.clone(), env, cols, rows })?;
        let pid = pty.child.id();
        // SAFETY: the `OwnedFd` moves into the `AsyncFd`, so it stays open and
        // unchanged for the `AsyncFd`'s whole life.
        let master = unsafe { AsyncFd::register(pty.master) }
            .map_err(|e| io::Error::other(e.to_string()))?;
        let master = Arc::new(master);
        let (input_tx, input_rx) = mpsc::unbounded_channel();

        st.panes.insert(pane_id, PaneRt {
            bench,
            emu: AlacrittyEmulator::new(GridSize::new(cols, rows), st.palette.clone()),
            reply_guard: ReplyGuard::default(),
            input: input_tx,
            master: master.clone(),
            pid,
            marks: MarkScanner::new(),
            running: None,
        });
        tokio::spawn(write_loop(master.clone(), input_rx));
        tokio::spawn(self.clone().pane_loop(pane_id, master, pty.child));
        info!(pane = ?pane_id, ?pid, "pane started");
        Ok(Pane::new(pane_id, cwd))
    }

    /// Reads a pane's output until its shell exits, then reports the exit.
    async fn pane_loop(self: Arc<Self>, pane: PaneId, master: Arc<AsyncFd<OwnedFd>>, mut child: Child) {
        let mut buf = vec![0u8; 64 * 1024];
        let mut status = None;
        let mut drain_until: Option<tokio::time::Instant> = None;
        let mut sync_deadline: Option<tokio::time::Instant> = None;

        loop {
            tokio::select! {
                ready = master.readable() => {
                    let Ok(mut guard) = ready else { break };
                    match guard.try_io(|fd| read_fd(fd.get_ref(), &mut buf)) {
                        Ok(Ok(0)) => break,
                        Ok(Ok(n)) => sync_deadline = self.on_output(pane, &buf[..n]).map(Into::into),
                        // EIO: every process holding the terminal has closed it.
                        Ok(Err(e)) if e.raw_os_error() == Some(libc::EIO) => break,
                        Ok(Err(e)) if e.kind() == io::ErrorKind::Interrupted => {}
                        Ok(Err(e)) => {
                            warn!(?pane, "read failed: {e}");
                            break;
                        }
                        Err(_would_block) => {}
                    }
                }
                exit = child.wait(), if status.is_none() => {
                    status = Some(exit.ok());
                    drain_until = Some(tokio::time::Instant::now() + EXIT_DRAIN);
                }
                _ = sleep_until(drain_until), if drain_until.is_some() => break,
                _ = sleep_until(sync_deadline), if sync_deadline.is_some() => {
                    let mut st = self.lock();
                    sync_deadline = st.panes.get_mut(&pane).and_then(|rt| rt.emu.tick()).map(Into::into);
                }
            }
        }

        let status = match status {
            Some(s) => s,
            None => tokio::time::timeout(Duration::from_secs(2), child.wait()).await.ok().and_then(Result::ok),
        };
        let code = status.and_then(|s| s.code());
        info!(?pane, ?code, "pane exited");
        {
            let mut st = self.lock();
            st.panes.remove(&pane);
            if let Some(bench) = st.tree.remove_pane(pane) {
                let frame = Frame::Keeper(KeeperMsg::PaneExited { pane, code }).encode().into();
                st.broadcast_to_bench(bench, frame);
            }
            st.broadcast_tree();
        }
        self.check_idle();
    }

    /// Follows a pane's shell through its commands, and logs each one as it finishes.
    fn on_mark(&self, st: &mut State, pane: PaneId, mark: Mark) {
        match mark {
            Mark::Cwd(dir) => {
                if let Some(p) = st.tree.pane_mut(pane)
                    && p.cwd.as_os_str() != dir.as_str()
                {
                    p.cwd = dir.into();
                    self.tree_dirty.notify_one();
                }
            }
            Mark::Command(command) => {
                let cwd = st.tree.pane(pane).map(|p| p.cwd.display().to_string()).unwrap_or_default();
                if let Some(rt) = st.panes.get_mut(&pane) {
                    rt.running = Some(Running { command, cwd, started_ms: unix_ms(), started: Instant::now() });
                }
            }
            Mark::Finished(exit) => {
                let Some(run) = st.panes.get_mut(&pane).and_then(|rt| rt.running.take()) else { return };
                let (Some(tx), Some((bench, tab_id))) = (&self.logbook, st.tree.locate_pane(pane)) else { return };
                let Some(tab) = st.tree.tab(tab_id) else { return };
                let entry = NewEntry {
                    command: run.command,
                    cwd: run.cwd,
                    exit,
                    started_ms: run.started_ms,
                    duration_ms: Some(run.started.elapsed().as_millis() as i64),
                    pane: pane.to_string(),
                    tab: tab_id.to_string(),
                    tab_title: tab.display_title(),
                    bench: bench.to_string(),
                    tags: tab.all_tags().map(|t| t.label.clone()).collect(),
                    host: self.host.clone(),
                    private: tab.private,
                };
                let _ = tx.send(entry);
            }
            Mark::PromptStart | Mark::InputStart | Mark::Executed | Mark::Walk(_) | Mark::WalkEnd => {}
        }
    }

    /// Feeds output to the keeper's emulator and forwards it to attached windows.
    /// Returns when the emulator next needs a tick.
    fn on_output(&self, pane: PaneId, bytes: &[u8]) -> Option<Instant> {
        let mut st = self.lock();
        let rt = st.panes.get_mut(&pane)?;
        rt.emu.advance(bytes);
        let replies = rt.emu.take_replies();
        let suppress = rt.reply_guard.suppress(bytes, Instant::now());
        if !replies.is_empty() {
            if suppress {
                debug!(?pane, len = replies.len(), "dropped replies to binary output");
            } else {
                let _ = rt.input.send(replies);
            }
        }
        let title = rt.emu.take_events().into_iter().rev().find_map(|e| match e {
            EmuEvent::Title(t) => Some(t),
            _ => None,
        });
        let deadline = rt.emu.tick();
        let bench = rt.bench;
        let mut marks = Vec::new();
        rt.marks.scan(bytes, &mut marks);
        st.broadcast_to_bench(bench, Frame::encode_output(pane, bytes).into());
        for mark in marks {
            self.on_mark(&mut st, pane, mark);
        }
        if let Some(title) = title
            && let Some(p) = st.tree.pane_mut(pane)
            && p.title != title
        {
            p.title = title;
            self.tree_dirty.notify_one();
        }
        deadline
    }
}

async fn sleep_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending().await,
    }
}

fn read_fd(fd: &OwnedFd, buf: &mut [u8]) -> io::Result<usize> {
    // SAFETY: `buf` is valid for `buf.len()` bytes of writes.
    let n = unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
    if n < 0 { Err(io::Error::last_os_error()) } else { Ok(n as usize) }
}

fn write_fd(fd: &OwnedFd, buf: &[u8]) -> io::Result<usize> {
    // SAFETY: `buf` is valid for `buf.len()` bytes of reads.
    let n = unsafe { libc::write(fd.as_raw_fd(), buf.as_ptr().cast(), buf.len()) };
    if n < 0 { Err(io::Error::last_os_error()) } else { Ok(n as usize) }
}

/// Writes queued input to a pane's PTY, waiting when the PTY is full.
async fn write_loop(master: Arc<AsyncFd<OwnedFd>>, mut rx: mpsc::UnboundedReceiver<Vec<u8>>) {
    while let Some(bytes) = rx.recv().await {
        let mut off = 0;
        while off < bytes.len() {
            let Ok(mut guard) = master.writable().await else { return };
            match guard.try_io(|fd| write_fd(fd.get_ref(), &bytes[off..])) {
                Ok(Ok(n)) => off += n,
                Ok(Err(e)) if e.kind() == io::ErrorKind::Interrupted => {}
                Ok(Err(_)) => return,
                Err(_would_block) => {}
            }
        }
    }
}

fn unix_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// Writes the startup file bash runs in Shed's panes: the user's ~/.bashrc,
/// then Shed's hook. (/etc/bash.bashrc is still read by bash itself.)
fn write_bash_rc(dir: &Path) -> Option<PathBuf> {
    let path = dir.join("shed-bashrc");
    let contents = format!(
        "# Written by shedd: your ~/.bashrc, then Shed's shell integration.\n\
         [ -f ~/.bashrc ] && . ~/.bashrc\n\n{}",
        shed_logbook::BASH_HOOK
    );
    match std::fs::write(&path, contents) {
        Ok(()) => Some(path),
        Err(e) => {
            warn!("cannot write {}: {e}", path.display());
            None
        }
    }
}
