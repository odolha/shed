//! The keeper's state and socket server.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use shed_emu::{AlacrittyEmulator, Emulator, GridSize, Palette};
use shed_model::{Bench, BenchColor, BenchId, Pane, PaneId, Shed, Tab, TabId};
use shed_proto::{
    ClientKind, ClientMsg, Frame, FrameDecoder, KeeperMsg, KeeperStatus, PROTOCOL_VERSION,
};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::Child;
use tokio::sync::{Notify, mpsc};
use tracing::{debug, info, warn};

use crate::pty::{self, SpawnOptions};

/// How long a pane keeps draining output after its shell exits, for
/// background jobs that still hold the terminal open.
const EXIT_DRAIN: Duration = Duration::from_millis(200);

pub struct KeeperConfig {
    pub socket: PathBuf,
    /// The shell for new panes; `None` uses the user's login shell.
    pub shell: Option<PathBuf>,
    /// Where new benches open.
    pub home: PathBuf,
    /// Exit once no panes and no clients remain.
    pub exit_when_idle: bool,
}

type ClientId = u64;

pub struct Keeper {
    cfg: KeeperConfig,
    state: Mutex<State>,
    shutdown: Notify,
    stopping: AtomicBool,
}

struct State {
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
    input: mpsc::UnboundedSender<Vec<u8>>,
    master: Arc<AsyncFd<OwnedFd>>,
    pid: Option<u32>,
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
        Arc::new(Self {
            cfg,
            state: Mutex::new(State {
                tree: Shed::default(),
                panes: HashMap::new(),
                clients: HashMap::new(),
                next_client: 1,
                started: Instant::now(),
                palette: Palette::default(),
            }),
            shutdown: Notify::new(),
            stopping: AtomicBool::new(false),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    /// Accepts clients until a client asks for shutdown or the keeper goes idle.
    pub async fn serve(self: Arc<Self>, listener: UnixListener) -> io::Result<()> {
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
        self.hang_up_all();
        Ok(())
    }

    /// Asks every pane's session to end, as closing a terminal would.
    fn hang_up_all(&self) {
        let st = self.lock();
        for rt in st.panes.values() {
            if let Some(pid) = rt.pid {
                // Negative pid: the whole process group the shell leads.
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(-(pid as i32)),
                    nix::sys::signal::Signal::SIGHUP,
                );
            }
        }
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
                        Ok(id) => {
                            st.broadcast_tree();
                            id
                        }
                        Err(e) => {
                            st.send(client, KeeperMsg::Error(format!("cannot start a shell: {e}")));
                            return;
                        }
                    },
                };
                if let Some(c) = st.clients.get_mut(&client) {
                    c.attached.insert(bench);
                }
                st.send(client, KeeperMsg::TreeChanged(st.tree.clone()));
                st.send(client, KeeperMsg::Attached { bench });
                info!(client, ?bench, "attached");
            }
            ClientMsg::Detach { bench } => {
                let mut st = self.lock();
                if let Some(c) = st.clients.get_mut(&client) {
                    c.attached.remove(&bench);
                }
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
                if let Some((bench, tab)) = st.tree.locate_pane(pane)
                    && let Some(p) = st
                        .tree
                        .bench_mut(bench)
                        .and_then(|b| b.tabs.iter_mut().find(|t| t.id == tab))
                        .and_then(|t| t.panes.iter_mut().find(|p| p.id == pane))
                {
                    p.cols = cols;
                    p.rows = rows;
                }
            }
            ClientMsg::NewBench { name, cwd } => {
                let mut st = self.lock();
                match self.new_bench(&mut st, name, cwd) {
                    Ok(_) => st.broadcast_tree(),
                    Err(e) => st.send(client, KeeperMsg::Error(format!("cannot start a shell: {e}"))),
                }
            }
            ClientMsg::Status => {
                let st = self.lock();
                let status = KeeperStatus {
                    pid: std::process::id(),
                    version: env!("CARGO_PKG_VERSION").into(),
                    uptime_secs: st.started.elapsed().as_secs(),
                    clients: st.clients.len() as u32,
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

    fn new_bench(
        self: &Arc<Self>,
        st: &mut State,
        name: Option<String>,
        cwd: Option<PathBuf>,
    ) -> io::Result<BenchId> {
        let id = BenchId::new();
        let cwd = cwd.unwrap_or_else(|| self.cfg.home.clone());
        let pane = self.spawn_pane(st, id, TabId::new(), cwd.clone())?;
        let name = name.unwrap_or_else(|| st.unique_bench_name());
        let color = BenchColor::next_unused(st.tree.benches.iter().map(|b| &b.color));
        st.tree.benches.push(Bench {
            id,
            name,
            color,
            badge: None,
            cwd,
            tabs: vec![Tab::with_pane("shell", pane)],
            active_tab: 0,
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

        let cwd = if cwd.is_dir() { cwd } else { self.cfg.home.clone() };
        let pty = pty::spawn(SpawnOptions { program, args: vec![], cwd: cwd.clone(), env, cols, rows })?;
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
            input: input_tx,
            master: master.clone(),
            pid,
        });
        tokio::spawn(write_loop(master.clone(), input_rx));
        tokio::spawn(self.clone().pane_loop(pane_id, master, pty.child));
        info!(pane = ?pane_id, ?pid, "pane started");
        Ok(Pane { id: pane_id, cwd, cols, rows })
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

    /// Feeds output to the keeper's emulator and forwards it to attached windows.
    /// Returns when the emulator next needs a tick.
    fn on_output(&self, pane: PaneId, bytes: &[u8]) -> Option<Instant> {
        let mut st = self.lock();
        let rt = st.panes.get_mut(&pane)?;
        rt.emu.advance(bytes);
        let replies = rt.emu.take_replies();
        if !replies.is_empty() {
            let _ = rt.input.send(replies);
        }
        rt.emu.take_events();
        let deadline = rt.emu.tick();
        let bench = rt.bench;
        st.broadcast_to_bench(bench, Frame::encode_output(pane, bytes).into());
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
