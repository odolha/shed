//! The keeper's state and socket server.

use std::collections::{HashMap, HashSet, VecDeque};
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
use shed_save::{PaneExtras, Relaunch, SavedBench, SessionBench, Store};
use shed_model::config::{Config, FileWatch};
use shed_model::{AiState, Bench, Held, PaneSetup, BenchColor, BenchId, Pane, PaneId, Shed, Tab, TabId, Tool};
use shed_proto::{
    AliasTarget, BenchRef, ClientKind, ClientMsg, Frame, FrameDecoder, KeeperMsg, KeeperStatus,
    PROTOCOL_VERSION, PegboardInfo, SavedEdit, ShowTarget, StoredBench,
};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::Child;
use tokio::sync::{Notify, mpsc};
use tracing::{debug, info, warn};

use crate::procinfo;
use crate::ask;
use crate::pty::{self, SpawnOptions};

/// How long a pane keeps draining output after its shell exits, for
/// background jobs that still hold the terminal open.
const EXIT_DRAIN: Duration = Duration::from_millis(200);

/// Lines of scrollback kept per pane in a saved bench.
const SAVED_SCROLLBACK: usize = 5000;

/// How long after its command finished a tool still counts as running when
/// the keeper stops: at logout, tools and the keeper are stopped together.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// How long a loaded pane waits for its shell's first prompt before typing
/// its saved command anyway (shells without Shed's hook send no prompt mark).
const PROMPT_WAIT: Duration = Duration::from_secs(3);

/// The longest a bench's close waits for a pane's teardown.
const TEARDOWN_LIMIT: Duration = Duration::from_secs(120);

/// How often each pane's tool and directory are read from `/proc`.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Tree changes from output (titles) are batched for this long before broadcast.
const TREE_FLUSH_DELAY: Duration = Duration::from_millis(50);

/// How much of a screen the pegboard previews.
const PREVIEW_LINES: usize = 12;
const PREVIEW_COLS: usize = 100;

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
    /// Where benches are saved; `None` saves nothing.
    pub store: Option<Store>,
    /// Desktop entries to remove when a bench is deleted.
    pub desktop: Option<shed_desktop::Desktop>,
    /// Shed's global aliases, reloaded when the file changes.
    pub aliases_file: PathBuf,
}

type ClientId = u64;

pub struct Keeper {
    cfg: KeeperConfig,
    state: Mutex<State>,
    shutdown: Notify,
    stopping: AtomicBool,
    /// `finish` has run.
    finished: AtomicBool,
    /// Asks the autosave task to save now (after setup or alias edits).
    save_soon: Notify,
    /// Wakes the task that broadcasts batched tree changes.
    tree_dirty: Notify,
    /// Finished commands, on their way to the logbook writer thread.
    logbook: Option<std::sync::mpsc::Sender<NewEntry>>,
    /// The bash startup file that loads ~/.bashrc and then Shed's hook.
    bash_rc: Option<PathBuf>,
    /// The `ZDOTDIR` for zsh, whose `.zshenv` loads the user's startup files
    /// and Shed's hook.
    zsh_dir: Option<PathBuf>,
    host: String,
    /// Shed is closing every bench: the session it recorded must stay.
    quitting: AtomicBool,
    /// The last session has been brought back (or skipped) once.
    restored: AtomicBool,
    /// Saved benches as the pegboard shows them, by name, while their file's
    /// save time is unchanged.
    stored_cache: Mutex<HashMap<String, CachedStored>>,
}

/// A saved bench for the pegboard: its save time, itself, and its previews.
type CachedStored = (String, StoredBench, Vec<(PaneId, Vec<String>)>);

struct State {
    config: Config,
    config_watch: FileWatch,
    aliases_watch: FileWatch,
    /// What each tab's alias file holds, to rewrite it only on change.
    alias_files: HashMap<TabId, String>,
    tree: Shed,
    panes: HashMap<PaneId, PaneRt>,
    clients: HashMap<ClientId, ClientRt>,
    next_client: ClientId,
    started: Instant,
    palette: Palette,
    /// Benches with a window, as last known: a window process that goes away
    /// without detaching (logout, a crash) leaves its benches here.
    shown: HashSet<BenchId>,
    /// The desktop prefers dark, as the window process last said.
    dark: bool,
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
    /// The last command to finish, and when.
    last_finished: Option<(String, Instant)>,
    /// Commands to type at the next prompts, one per prompt, and whether to run each.
    after_prompt: VecDeque<(String, bool)>,
    /// Ticks at every prompt, for closing a bench step by step.
    prompts: tokio::sync::watch::Sender<u64>,
    /// The shell has shown a prompt mark: it runs Shed's hook.
    hooked: bool,
    /// The shell reported a command's end (or its first prompt) since the
    /// last prompt mark: the next prompt is a new one, not a redraw.
    /// Readline redraws its prompt, marks and all, when the window resizes.
    new_prompt: bool,
    /// The shell reports ends at all; one that does not counts every prompt.
    reports_ends: bool,
    /// The AI request under way, to stop it.
    ask: Option<tokio::task::AbortHandle>,
    /// Panes of the bench this one's startup waits for, as the bench loaded.
    waits_for: Vec<PaneId>,
    /// Startup is held and the shell sits at a prompt: release types it.
    held_at_prompt: bool,
    /// Whether the panes that wait for this one may go.
    readiness: Readiness,
    /// Ready once the output shows this; otherwise once startup finishes.
    ready_text: Option<String>,
    /// The output as plain text, for `ready_text`; the commands Shed typed
    /// are left out of the search, so a command naming the text does not count.
    plain: shed_emu::plain::Plain,
    seen: Vec<u8>,
    typed: Vec<String>,
    /// A startup command was typed and has not finished.
    startup_pending: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Readiness {
    Pending,
    Ready,
    Failed,
}

/// How to start a pane.
struct PaneSpec {
    id: PaneId,
    cwd: PathBuf,
    cols: u16,
    rows: u16,
    /// Bytes shown before the shell starts: a loaded pane's old scrollback.
    restore: Option<Vec<u8>>,
    after_prompt: VecDeque<(String, bool)>,
}

impl PaneSpec {
    fn fresh(cwd: PathBuf) -> Self {
        PaneSpec { id: PaneId::new(), cwd, cols: 80, rows: 24, restore: None, after_prompt: VecDeque::new() }
    }
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
        let zsh_dir = write_zsh_dir(cfg.socket.parent().unwrap_or(Path::new("/tmp")));
        let host = nix::unistd::gethostname().ok().and_then(|h| h.into_string().ok()).unwrap_or_default();
        let shed_window = cfg.store.as_ref().and_then(Store::shed_window);
        Arc::new(Self {
            cfg,
            state: Mutex::new(State {
                config: Config::default(),
                config_watch: FileWatch::default(),
                aliases_watch: FileWatch::default(),
                alias_files: HashMap::new(),
                tree: Shed { window: shed_window, ..Shed::default() },
                panes: HashMap::new(),
                clients: HashMap::new(),
                next_client: 1,
                started: Instant::now(),
                palette: Palette::default(),
                shown: HashSet::new(),
                dark: true,
            }),
            shutdown: Notify::new(),
            stopping: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            save_soon: Notify::new(),
            tree_dirty: Notify::new(),
            logbook,
            bash_rc,
            zsh_dir,
            host,
            quitting: AtomicBool::new(false),
            restored: AtomicBool::new(false),
            stored_cache: Mutex::new(HashMap::new()),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    /// Accepts clients until a client asks for shutdown or the keeper goes idle.
    pub async fn serve(self: Arc<Self>, listener: UnixListener) -> io::Result<()> {
        // Before any client: the first `shed` may ask for what the config decides (auto_restore).
        self.reload_config(&mut self.lock());
        let poll = tokio::spawn(self.clone().poll_loop());
        let flush = tokio::spawn(self.clone().flush_loop());
        let autosave = tokio::spawn(self.clone().autosave_loop());
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
        autosave.abort();
        self.finish();
        Ok(())
    }

    /// Loads `shed.toml` if it changed since last time (or was never read).
    fn reload_config(&self, st: &mut State) {
        if st.config_watch.changed(&self.cfg.config_file) {
            match Config::load_from(&self.cfg.config_file) {
                Ok(config) => {
                    st.config = config;
                    apply_theme(st);
                    info!("config loaded");
                }
                Err(e) => warn!("config not loaded: {e}"),
            }
        }
    }

    /// Keeps each pane's tool and working directory current.
    async fn poll_loop(self: Arc<Self>) {
        let mut tick = tokio::time::interval(POLL_INTERVAL);
        loop {
            tick.tick().await;
            let mut st = self.lock();
            let mut changed = false;
            if st.aliases_watch.changed(&self.cfg.aliases_file) {
                match shed_model::config::load_aliases(&self.cfg.aliases_file) {
                    Ok(aliases) => {
                        st.tree.aliases = aliases;
                        self.sync_alias_files(&mut st);
                        changed = true;
                    }
                    Err(e) => warn!("global aliases not loaded: {e}"),
                }
            }
            self.reload_config(&mut st);
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
                let mut st = self.lock();
                if let Some(rt) = st.panes.get(&pane) {
                    let _ = rt.input.send(bytes);
                }
                // The next key clears a failed AI request's message.
                if let Some(p) = st.tree.pane_mut(pane)
                    && matches!(p.ai, Some(AiState::Failed(_)))
                {
                    p.ai = None;
                    self.tree_dirty.notify_one();
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
                    if c.kind == ClientKind::Window {
                        st.shown.insert(bench);
                    }
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
                // Detaching is hiding the bench: its window was closed.
                st.shown.remove(&bench);
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
                        self.sync_alias_files(&mut st);
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
                tokio::spawn(self.clone().close_bench(bench));
            }
            ClientMsg::SetPaneSetup { pane, setup } => {
                let mut st = self.lock();
                if let Some(p) = st.tree.pane_mut(pane) {
                    p.setup = setup;
                    st.broadcast_tree();
                    self.save_soon.notify_one();
                }
            }
            ClientMsg::SetAliases { level, aliases } => {
                let mut st = self.lock();
                match level {
                    AliasTarget::Global => {
                        if let Err(e) = shed_model::config::save_aliases(&self.cfg.aliases_file, &aliases) {
                            st.send(client, KeeperMsg::Error(format!("cannot save global aliases: {e}")));
                            return;
                        }
                        st.aliases_watch.changed(&self.cfg.aliases_file);
                        st.tree.aliases = aliases;
                    }
                    AliasTarget::Bench(b) => match st.tree.bench_mut(b) {
                        Some(b) => b.aliases = aliases,
                        None => return,
                    },
                    AliasTarget::Tab(t) => match st.tree.tab_mut(t) {
                        Some(t) => t.aliases = aliases,
                        None => return,
                    },
                }
                self.sync_alias_files(&mut st);
                st.broadcast_tree();
                self.save_soon.notify_one();
                st.send(client, KeeperMsg::Done("aliases set".into()));
            }
            ClientMsg::DeleteBench { name } => {
                let mut st = self.lock();
                let running = st.tree.benches.iter().find(|b| b.name == name).map(|b| (b.id, b.launcher.clone()));
                let saved = self.cfg.store.as_ref().and_then(|s| s.load(&name).ok()).map(|b| b.launcher());
                if let Some((id, _)) = running {
                    self.drop_bench(&mut st, id);
                }
                let removed = self.cfg.store.as_ref().map(|s| (s.exists(&name), s.delete(&name)));
                if let Some(desktop) = &self.cfg.desktop {
                    for launcher in running.map(|(_, l)| l).into_iter().chain(saved) {
                        desktop.remove(&launcher);
                    }
                }
                match removed {
                    Some((true, Ok(()))) => st.send(client, KeeperMsg::Done(format!("deleted {name}"))),
                    Some((_, Err(e))) => st.send(client, KeeperMsg::Error(format!("cannot delete {name:?}: {e}"))),
                    _ => st.send(client, KeeperMsg::Done(format!("no saved bench named {name:?}"))),
                }
                info!(%name, "bench deleted");
            }
            ClientMsg::SaveBench { bench, as_name } => {
                let st = self.lock();
                let Some(store) = &self.cfg.store else {
                    st.send(client, KeeperMsg::Error("saving is turned off".into()));
                    return;
                };
                let Some((saved, scrollback)) = self.snapshot(&st, &[bench], false).into_iter().next() else {
                    st.send(client, KeeperMsg::Error("no such bench".into()));
                    return;
                };
                if let Err(e) = store.save(&saved, &scrollback) {
                    st.send(client, KeeperMsg::Error(format!("cannot save: {e}")));
                    return;
                }
                let msg = match as_name {
                    None => format!("saved {}", saved.name),
                    Some(copy) => {
                        if let Some(problem) = self.name_problem(&st, &copy, None) {
                            st.send(client, KeeperMsg::Error(problem));
                            return;
                        }
                        // A copy starts clean: no scrollback, fresh IDs, its own launcher.
                        let launcher = self.unique_launcher(&st, &copy);
                        if let Err(e) = store.save(&saved.copy_as(&copy, launcher), &[]) {
                            st.send(client, KeeperMsg::Error(format!("cannot save {copy:?}: {e}")));
                            return;
                        }
                        format!("saved {} as {copy}", saved.name)
                    }
                };
                st.send(client, KeeperMsg::Done(msg));
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
                if let Some(problem) = self.name_problem(&st, &name, Some(bench)) {
                    st.send(client, KeeperMsg::Error(problem));
                } else if let Some(b) = st.tree.bench_mut(bench) {
                    let old = std::mem::replace(&mut b.name, name.clone());
                    if let Some(store) = &self.cfg.store {
                        if let Err(e) = store.rename(&old, &name) {
                            warn!("cannot rename the saved bench: {e}");
                        }
                        // At once, so the file never says the old name.
                        for (saved, scrollback) in self.snapshot(&st, &[bench], false) {
                            if let Err(e) = store.save(&saved, &scrollback) {
                                warn!("cannot save bench {:?}: {e}", saved.name);
                            }
                        }
                    }
                    st.broadcast_tree();
                }
            }
            ClientMsg::RenameTab { tab, name } => self.edit_tab(tab, |t| t.name = name.filter(|n| !n.trim().is_empty())),
            ClientMsg::SetTags { tab, tags } => self.edit_tab(tab, |t| t.tags = tags),
            ClientMsg::SetPrivate { tab, private } => self.edit_tab(tab, |t| t.private = private),
            ClientMsg::SetZoom { tab, zoom } => self.edit_tab(tab, |t| t.zoom = zoom.clamp(-20.0, 60.0)),
            ClientMsg::StartNow { pane } => {
                let mut st = self.lock();
                self.release(&mut st, pane);
            }
            ClientMsg::SetWindow { bench: Some(bench), place } => self.edit_bench(bench, |b| b.window = Some(place)),
            ClientMsg::SetWindow { bench: None, place } => {
                let mut st = self.lock();
                st.tree.window = Some(place);
                st.broadcast_tree();
                if let Some(store) = &self.cfg.store
                    && let Err(e) = store.save_shed_window(&place)
                {
                    warn!("cannot save where the Shed window is: {e}");
                }
            }
            ClientMsg::Hide { bench } => {
                let st = self.lock();
                let Some(name) = st.tree.bench(bench).map(|b| b.name.clone()) else {
                    st.send(client, KeeperMsg::Error(format!("no bench {bench}")));
                    return;
                };
                let done = match st.window_client(client) {
                    Some(window) if st.shown.contains(&bench) => {
                        st.send(window, KeeperMsg::Hide { bench });
                        format!("hid {name}")
                    }
                    _ => format!("{name} has no window"),
                };
                st.send(client, KeeperMsg::Done(done));
            }
            ClientMsg::SetBenchOrder(order) => {
                if let Some(store) = &self.cfg.store
                    && let Err(e) = store.save_order(&order)
                {
                    warn!("cannot save the bench order: {e}");
                }
            }
            ClientMsg::SetBenchColor { bench, color } => self.edit_bench(bench, |b| b.color = color),
            ClientMsg::SetBadge { bench, badge } => self.edit_bench(bench, |b| b.badge = badge),
            ClientMsg::MoveTab { tab, bench, index } => {
                let mut st = self.lock();
                let Some(from) = st.tree.bench_of_tab(tab) else { return };
                if st.tree.move_tab(tab, bench, index) {
                    self.sync_alias_files(&mut st);
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
                    stored: self
                        .cfg
                        .store
                        .as_ref()
                        .map(|s| s.list().into_iter().map(|b| b.name).filter(|n| !st.tree.benches.iter().any(|b| &b.name == n)).collect())
                        .unwrap_or_default(),
                    tree: st.tree.clone(),
                };
                st.send(client, KeeperMsg::Status(status));
            }
            ClientMsg::SetAppearance { dark } => {
                let mut st = self.lock();
                if st.dark != dark {
                    st.dark = dark;
                    apply_theme(&mut st);
                }
            }
            ClientMsg::CancelAsk { pane } => {
                let mut st = self.lock();
                if let Some(handle) = st.panes.get_mut(&pane).and_then(|rt| rt.ask.take()) {
                    handle.abort();
                }
                if let Some(p) = st.tree.pane_mut(pane) {
                    p.ai = None;
                    st.broadcast_tree();
                }
            }
            ClientMsg::MoveTabAcross { tab, from, to, index } => {
                let result = self.move_tab_across(tab, from, to, index);
                let st = self.lock();
                match result {
                    Ok(done) => st.send(client, KeeperMsg::Done(done)),
                    Err(e) => st.send(client, KeeperMsg::Error(e)),
                }
            }
            ClientMsg::EditSaved { name, edit } => {
                let st = self.lock();
                match self.edit_saved(&st, &name, edit) {
                    Ok(done) => st.send(client, KeeperMsg::Done(done)),
                    Err(e) => st.send(client, KeeperMsg::Error(e)),
                }
            }
            ClientMsg::QuitShed => {
                info!(client, "closing Shed");
                tokio::spawn(self.clone().quit_shed());
            }
            ClientMsg::Pegboard => {
                let (mut previews, running): (Vec<_>, Vec<String>) = {
                    let st = self.lock();
                    let previews = st
                        .tree
                        .benches
                        .iter()
                        .flat_map(|b| &b.tabs)
                        .filter_map(|t| Some((t.active_pane, preview(&st.panes.get(&t.active_pane)?.emu.dump_text()))))
                        .collect();
                    (previews, st.tree.benches.iter().map(|b| b.name.clone()).collect())
                };
                // Saved benches are read without holding the state.
                let mut stored = Vec::new();
                if let Some(store) = &self.cfg.store {
                    let mut cache = self.stored_cache.lock().unwrap();
                    let infos = store.list();
                    cache.retain(|name, _| infos.iter().any(|i| &i.name == name));
                    for info in infos.into_iter().filter(|i| !running.contains(&i.name)) {
                        if let Some((saved, bench, prev)) = cache.get(&info.name)
                            && *saved == info.saved
                        {
                            stored.push(bench.clone());
                            previews.extend(prev.iter().cloned());
                            continue;
                        }
                        let Ok(saved) = store.load(&info.name) else { continue };
                        let Ok((mut bench, panes)) = saved.to_bench() else { continue };
                        let mut prev = Vec::new();
                        for tab in &mut bench.tabs {
                            for pane in &mut tab.panes {
                                let tool = panes.iter().find(|(id, _)| *id == pane.id).and_then(|(_, p)| p.tool.as_deref());
                                pane.tool = tool.map(|t| Tool { argv: t.split_whitespace().map(String::from).collect() });
                            }
                            if let Some(text) = store.scrollback(&bench.name, tab.active_pane) {
                                prev.push((tab.active_pane, preview(&text)));
                            }
                        }
                        let entry = StoredBench { bench, saved: saved.saved.clone() };
                        previews.extend(prev.iter().cloned());
                        stored.push(entry.clone());
                        cache.insert(info.name, (saved.saved, entry, prev));
                    }
                }
                let order = self.cfg.store.as_ref().map(|s| s.order()).unwrap_or_default();
                self.lock().send(client, KeeperMsg::Pegboard(PegboardInfo { stored, previews, order }));
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

    /// Takes a bench out of the tree and hangs up its shells.
    fn drop_bench(&self, st: &mut State, bench: BenchId) {
        let panes = st.pane_rt_ids_in(bench);
        st.tree.benches.retain(|b| b.id != bench);
        st.shown.remove(&bench);
        st.broadcast_tree();
        st.hang_up(&panes);
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
            ShowTarget::Detached => st.tree.benches.iter().map(|b| b.id).filter(|b| !st.is_open(*b)).collect(),
            ShowTarget::Shed => {
                let mut open = Vec::new();
                // Only a keeper that runs nothing brings a session back, and only once.
                let fresh = st.tree.benches.is_empty() && !self.restored.swap(true, Ordering::SeqCst);
                if fresh && st.config.auto_restore {
                    let session = self.cfg.store.as_ref().map(Store::session).unwrap_or_default();
                    for SessionBench { name, open: had_window } in session {
                        match self.load_bench(&mut st, &name) {
                            Ok(id) if had_window => open.push(id),
                            Ok(_) => {}
                            Err(e) => warn!("cannot load bench {name:?}: {e}"),
                        }
                    }
                }
                match st.window_client(client) {
                    Some(window) => {
                        // The tree first: it says where the Shed window was.
                        st.broadcast_tree();
                        st.send(window, KeeperMsg::ShowShed);
                    }
                    None => {
                        st.send(client, KeeperMsg::Error("no window is open to show it in".into()));
                        return;
                    }
                }
                open
            }
            ShowTarget::Load { name } => {
                let name = self.bench_name(&st, &name);
                let running = st.tree.benches.iter().find(|b| b.name == name).map(|b| b.id);
                match running.map(Ok).unwrap_or_else(|| self.load_bench(&mut st, &name)) {
                    Ok(id) => vec![id],
                    Err(e) => {
                        st.send(client, KeeperMsg::Error(format!("cannot load {name:?}: {e}")));
                        return;
                    }
                }
            }
            ShowTarget::NewBench { name, cwd } => {
                if let Some(n) = &name
                    && let Some(problem) = self.name_problem(&st, n, None)
                {
                    st.send(client, KeeperMsg::Error(problem));
                    return;
                }
                match self.new_bench(&mut st, name, cwd) {
                    Ok(b) => vec![b],
                    Err(e) => {
                        st.send(client, KeeperMsg::Error(format!("cannot start a shell: {e}")));
                        return;
                    }
                }
            }
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

    /// A free name for a new bench, not taken by a running or a saved bench.
    fn unique_bench_name(&self, st: &State) -> String {
        let taken = |n: &str| {
            st.tree.benches.iter().any(|b| b.name == n) || self.cfg.store.as_ref().is_some_and(|s| s.exists(n))
        };
        if !taken("main") {
            return "main".into();
        }
        (2..).map(|i| format!("bench-{i}")).find(|n| !taken(n)).unwrap()
    }

    /// A launcher key for a new bench called `name`: its slug, numbered if
    /// another bench, running or saved, already has it.
    fn unique_launcher(&self, st: &State, name: &str) -> String {
        let mut taken: HashSet<String> = st.tree.benches.iter().map(|b| b.launcher.clone()).collect();
        if let Some(store) = &self.cfg.store {
            taken.extend(store.list().into_iter().map(|i| i.launcher));
        }
        let base = shed_model::slug(name);
        std::iter::once(base.clone()).chain((2..).map(|i| format!("{base}-{i}"))).find(|l| !taken.contains(l)).unwrap()
    }

    /// The bench `key` means: a name, or else a launcher key, which pinned
    /// launchers use because it stays the same when the bench is renamed.
    fn bench_name(&self, st: &State, key: &str) -> String {
        if st.tree.benches.iter().any(|b| b.name == key) || self.cfg.store.as_ref().is_some_and(|s| s.exists(key)) {
            return key.to_string();
        }
        if let Some(b) = st.tree.benches.iter().find(|b| b.launcher == key) {
            return b.name.clone();
        }
        let stored = self.cfg.store.as_ref().map(Store::list).unwrap_or_default();
        stored.into_iter().find(|i| i.launcher == key).map_or_else(|| key.to_string(), |i| i.name)
    }

    /// Renames, recolours or rebadges a closed bench in its saved file.
    fn edit_saved(&self, st: &State, name: &str, edit: SavedEdit) -> Result<String, String> {
        let store = self.cfg.store.as_ref().ok_or("saving is turned off")?;
        if st.tree.benches.iter().any(|b| b.name == name) {
            return Err(format!("{name:?} is running; change it in its window"));
        }
        let mut saved = store.load(name).map_err(|e| format!("cannot read {name:?}: {e}"))?;
        let done = match edit {
            SavedEdit::Rename(new) => {
                let new = new.trim().to_string();
                if new == name {
                    return Ok(format!("{name} kept its name"));
                }
                if let Some(problem) = self.name_problem(st, &new, Some(saved.id)) {
                    return Err(problem);
                }
                store.rename(name, &new).map_err(|e| format!("cannot rename {name:?}: {e}"))?;
                saved.name = new.clone();
                format!("renamed {name} to {new}")
            }
            SavedEdit::Color(color) => {
                saved.color = color.to_text();
                format!("recoloured {name}")
            }
            SavedEdit::Badge(badge) => {
                saved.badge = badge;
                format!("rebadged {name}")
            }
            SavedEdit::RenameTab { tab, name: title } => {
                saved.tab_mut(tab).ok_or("no such tab")?.name = title.filter(|n| !n.trim().is_empty());
                format!("renamed a tab of {name}")
            }
            SavedEdit::TabTags { tab, tags } => {
                saved.tab_mut(tab).ok_or("no such tab")?.tags = shed_save::saved_tags(&tags);
                format!("tagged a tab of {name}")
            }
            SavedEdit::TabPrivate { tab, private } => {
                saved.tab_mut(tab).ok_or("no such tab")?.private = private;
                format!("changed a tab of {name}")
            }
            SavedEdit::NewTab => {
                saved.add_tab();
                format!("added a tab to {name}")
            }
            SavedEdit::RemoveTab { tab } => {
                saved.remove_tab(tab)?;
                format!("removed a tab from {name}")
            }
            SavedEdit::MoveTab { tab, index } => {
                saved.move_tab(tab, index);
                format!("moved a tab of {name}")
            }
            SavedEdit::PaneSetup { pane, setup } => {
                if !saved.set_pane_setup(pane, &setup) {
                    return Err("no such pane".into());
                }
                format!("set up a pane of {name}")
            }
            SavedEdit::Aliases { tab: None, aliases } => {
                saved.aliases = aliases;
                format!("set the aliases of {name}")
            }
            SavedEdit::Aliases { tab: Some(tab), aliases } => {
                saved.tab_mut(tab).ok_or("no such tab")?.aliases = aliases;
                format!("set a tab's aliases in {name}")
            }
        };
        // Its scrollback stays as it is.
        store.save(&saved, &[]).map_err(|e| format!("cannot save {:?}: {e}", saved.name))?;
        self.stored_cache.lock().unwrap().remove(name);
        Ok(done)
    }

    /// Why `name` cannot name a bench (other than `except`), if it can't.
    fn name_problem(&self, st: &State, name: &str, except: Option<BenchId>) -> Option<String> {
        let name = name.trim();
        if name.is_empty() {
            return Some("a bench needs a name".into());
        }
        if st.tree.benches.iter().any(|b| b.name == name && Some(b.id) != except) {
            return Some(format!("a bench named {name:?} is already running"));
        }
        let saved_elsewhere = self.cfg.store.as_ref().and_then(|s| s.load(name).ok()).is_some_and(|b| Some(b.id) != except);
        saved_elsewhere.then(|| format!("a saved bench is named {name:?}; load it with `shed load {name}` or pick another name"))
    }

    /// Rebuilds a saved bench: every pane in its folder with its old
    /// scrollback, and its tools restarted or typed according to the rack.
    fn load_bench(self: &Arc<Self>, st: &mut State, name: &str) -> Result<BenchId, String> {
        let store = self.cfg.store.as_ref().ok_or("saving is turned off")?;
        let saved = store.load(name).map_err(|e| e.to_string())?;
        if st.tree.bench(saved.id).is_some() {
            return Ok(saved.id);
        }
        if st.tree.benches.iter().any(|b| b.name == saved.name) {
            return Err(format!("another bench named {:?} is running", saved.name));
        }
        let (mut bench, panes) = saved.to_bench()?;
        let bench_cwd = bench.cwd.clone();
        let mut started = Vec::new();
        for tab in &mut bench.tabs {
            if let Err(e) = self.start_saved_tab(st, &saved, &panes, bench.id, &bench_cwd, tab, &mut started) {
                st.hang_up(&started);
                return Err(e);
            }
        }
        let id = bench.id;
        info!(name = %bench.name, panes = started.len(), "bench loaded");
        // Startup that waits for other panes is held until they are ready.
        let setups: Vec<(PaneId, PaneSetup)> = bench.tabs.iter().flat_map(|t| &t.panes).map(|p| (p.id, p.setup.clone())).collect();
        for (pane, setup) in &setups {
            if let Some(rt) = st.panes.get_mut(pane) {
                rt.ready_text = setup.ready_when.clone().filter(|t| !t.trim().is_empty());
                rt.waits_for = setup.after.iter().copied().filter(|d| d != pane && started.contains(d)).collect();
            }
        }
        st.tree.benches.push(bench);
        for (pane, _) in &setups {
            self.show_held(st, *pane);
        }
        self.sync_alias_files(st);
        Ok(id)
    }

    /// Writes into the tree what a pane's startup waits for, if anything.
    fn show_held(&self, st: &mut State, pane: PaneId) {
        let Some(rt) = st.panes.get(&pane) else { return };
        let failed = rt.waits_for.iter().find(|d| st.panes.get(d).is_some_and(|r| r.readiness == Readiness::Failed));
        let held = match failed {
            Some(d) => Some(Held::Failed(pane_label(&st.tree, *d))),
            None if rt.waits_for.is_empty() => None,
            None => Some(Held::Waiting(rt.waits_for.iter().map(|d| pane_label(&st.tree, *d)).collect())),
        };
        if let Some(p) = st.tree.pane_mut(pane)
            && p.held != held
        {
            p.held = held;
            self.tree_dirty.notify_one();
        }
    }

    /// A pane is ready (or failed): the panes waiting for it learn of it, and
    /// those with nothing left to wait for start.
    fn set_readiness(&self, st: &mut State, pane: PaneId, readiness: Readiness) {
        let Some(rt) = st.panes.get_mut(&pane) else { return };
        if rt.readiness != Readiness::Pending {
            return;
        }
        rt.readiness = readiness;
        info!(?pane, ?readiness, "pane readiness");
        let waiting: Vec<PaneId> = st.panes.iter().filter(|(_, r)| r.waits_for.contains(&pane)).map(|(id, _)| *id).collect();
        for other in waiting {
            if readiness == Readiness::Ready
                && let Some(r) = st.panes.get_mut(&other)
            {
                r.waits_for.retain(|d| *d != pane);
                if r.waits_for.is_empty() {
                    self.release(st, other);
                }
            }
            self.show_held(st, other);
        }
    }

    /// Lets a held pane's startup go: at once if its shell is at a prompt.
    fn release(&self, st: &mut State, pane: PaneId) {
        let Some(rt) = st.panes.get_mut(&pane) else { return };
        rt.waits_for.clear();
        if std::mem::take(&mut rt.held_at_prompt) {
            self.send_after_prompt(st, pane);
        }
        self.show_held(st, pane);
    }

    /// Starts a saved tab's panes in bench `bench`: each in its folder with its
    /// old scrollback, then its startup commands or what was running, by the
    /// rack rules. Started panes are added to `started`.
    #[allow(clippy::too_many_arguments)]
    fn start_saved_tab(
        self: &Arc<Self>,
        st: &mut State,
        saved: &SavedBench,
        panes: &[(PaneId, shed_save::SavedPane)],
        bench: BenchId,
        bench_cwd: &Path,
        tab: &mut Tab,
        started: &mut Vec<PaneId>,
    ) -> Result<(), String> {
        let store = self.cfg.store.as_ref().ok_or("saving is turned off")?;
        for pane in &mut tab.panes {
            let sp = panes.iter().find(|(id, _)| *id == pane.id).map(|(_, p)| p.clone());
            let restore = store.scrollback(&saved.name, pane.id).map(|text| shed_save::restored_scrollback(&text, &saved.saved));
            // The pane's own startup wins over whatever was running at save time.
            let after_prompt: VecDeque<(String, bool)> = if !pane.setup.startup.is_empty() {
                pane.setup.startup.iter().map(|c| (c.clone(), !pane.setup.wait)).collect()
            } else {
                let tool = sp.as_ref().and_then(|p| p.tool.clone());
                tool.and_then(|cmd| match shed_save::relaunch(&st.config.rack, &cmd) {
                    Relaunch::Run => Some((cmd, true)),
                    Relaunch::Prefill => Some((cmd, false)),
                    Relaunch::Skip => None,
                })
                .into_iter()
                .collect()
            };
            let cwd = [pane.setup.start_in.clone(), Some(pane.cwd.clone()), Some(bench_cwd.to_path_buf())]
                .into_iter()
                .flatten()
                .find(|d| d.is_dir())
                .unwrap_or_else(|| self.cfg.home.clone());
            let spec = PaneSpec { id: pane.id, cwd, cols: pane.cols, rows: pane.rows, restore, after_prompt };
            let spawned = self.spawn_pane_spec(st, bench, tab.id, spec).map_err(|e| format!("cannot start a shell: {e}"))?;
            started.push(pane.id);
            pane.cwd = spawned.cwd;
        }
        Ok(())
    }

    /// Moves a tab where either bench is closed. Into a running bench, a
    /// closed bench's tab starts as loading would start it; into a closed
    /// bench, a running tab is saved there and then stopped, as closing would.
    fn move_tab_across(self: &Arc<Self>, tab: TabId, from: BenchRef, to: BenchRef, index: usize) -> Result<String, String> {
        let store = self.cfg.store.as_ref().ok_or("saving is turned off")?;
        let mut st = self.lock();
        let running = |st: &State, name: &str| st.tree.benches.iter().any(|b| b.name == name);
        let done = match (from, to) {
            (BenchRef::Running(_), BenchRef::Running(bench)) => {
                drop(st);
                self.handle_msg(0, ClientMsg::MoveTab { tab, bench, index });
                return Ok("moved a tab".into());
            }
            (BenchRef::Saved(a), BenchRef::Saved(b)) if a == b => {
                drop(st);
                return self.edit_saved(&self.lock(), &a, SavedEdit::MoveTab { tab, index });
            }
            (BenchRef::Saved(a), BenchRef::Saved(b)) => {
                if running(&st, &a) || running(&st, &b) {
                    return Err("one of these benches is running now".into());
                }
                let mut from = store.load(&a).map_err(|e| format!("cannot read {a:?}: {e}"))?;
                let mut into = store.load(&b).map_err(|e| format!("cannot read {b:?}: {e}"))?;
                let moved = from.take_tab(tab)?;
                let scrollback: Vec<(PaneId, String)> = moved.panes.iter().filter_map(|p| Some((p.id, store.scrollback(&a, p.id)?))).collect();
                into.insert_tab(moved, index);
                store.save(&into, &scrollback).map_err(|e| format!("cannot save {b:?}: {e}"))?;
                store.save(&from, &[]).map_err(|e| format!("cannot save {a:?}: {e}"))?;
                format!("moved a tab from {a} to {b}")
            }
            (BenchRef::Saved(a), BenchRef::Running(bench)) => {
                if running(&st, &a) {
                    return Err(format!("{a:?} is running now"));
                }
                let into_name = st.tree.bench(bench).map(|b| b.name.clone()).ok_or("that bench is not running")?;
                let mut from = store.load(&a).map_err(|e| format!("cannot read {a:?}: {e}"))?;
                // Checks first that the tab may leave (it is not the last).
                let mut rest = from.clone();
                rest.take_tab(tab)?;
                let (whole, panes) = from.to_bench()?;
                let mut moved = whole.tabs.into_iter().find(|t| t.id == tab).ok_or("no such tab")?;
                let mut started = Vec::new();
                if let Err(e) = self.start_saved_tab(&mut st, &from, &panes, bench, &whole.cwd, &mut moved, &mut started) {
                    st.hang_up(&started);
                    return Err(e);
                }
                let target = st.tree.bench_mut(bench).unwrap();
                target.tabs.insert(index.min(target.tabs.len()), moved);
                target.active_tab = tab;
                self.sync_alias_files(&mut st);
                st.broadcast_tree();
                // Windows showing the bench get the panes with their old scrollback.
                let targets: Vec<ClientId> = st.clients.iter().filter(|(_, c)| c.attached.contains(&bench)).map(|(id, _)| *id).collect();
                for c in targets {
                    for id in &started {
                        st.send_replay(c, *id);
                    }
                }
                from = rest;
                store.save(&from, &[]).map_err(|e| format!("cannot save {a:?}: {e}"))?;
                format!("moved a tab from {a} to {into_name}")
            }
            (BenchRef::Running(bench), BenchRef::Saved(b)) => {
                if running(&st, &b) {
                    return Err(format!("{b:?} is running now"));
                }
                let mut into = store.load(&b).map_err(|e| format!("cannot read {b:?}: {e}"))?;
                let (saved, scrollback) = self.snapshot(&st, &[bench], false).into_iter().next().ok_or("that bench is not running")?;
                let mut moved = saved.tabs.into_iter().find(|t| t.id == tab).ok_or("no such tab")?;
                // New ids: the panes still run here until they stop, and
                // loading the other bench meanwhile must not mistake them.
                moved.id = TabId::new();
                let mut kept = Vec::new();
                for pane in &mut moved.panes {
                    let fresh = PaneId::new();
                    if let Some((_, text)) = scrollback.iter().find(|(id, _)| *id == pane.id) {
                        kept.push((fresh, text.clone()));
                    }
                    pane.id = fresh;
                }
                into.insert_tab(moved, index);
                store.save(&into, &kept).map_err(|e| format!("cannot save {b:?}: {e}"))?;
                let panes: Vec<PaneId> = st.tree.tab(tab).map(|t| t.panes.iter().map(|p| p.id).collect()).unwrap_or_default();
                drop(st);
                tokio::spawn(self.clone().stop_panes(panes));
                format!("moved a tab to {b}")
            }
        };
        self.stored_cache.lock().unwrap().clear();
        Ok(done)
    }

    /// Asks the AI prompt's command for the command `request` describes, and
    /// types its answer at the pane's prompt, not run.
    fn ask(self: &Arc<Self>, st: &mut State, pane: PaneId, request: String) {
        let ai = st.config.ai.clone();
        if !ai.enabled() || request.is_empty() {
            return;
        }
        let Some((bench, tab)) = st.tree.locate_pane(pane) else { return };
        let private = st.tree.tab(tab).is_some_and(|t| t.private);
        let cwd = st.tree.pane(pane).map(|p| p.cwd.clone()).unwrap_or_else(|| self.cfg.home.clone());
        let screen = st.panes.get(&pane).filter(|_| ai.screen && !private).map(|rt| {
            // The request's own prompt line is the last; it says nothing new.
            let text = rt.emu.scrollback_text(41);
            let lines: Vec<&str> = text.lines().collect();
            lines[..lines.len().saturating_sub(1)].join("\n")
        });
        let shell = self.cfg.shell.clone().unwrap_or_else(pty::default_shell);
        let shell = shell.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "sh".into());
        if let Some(p) = st.tree.pane_mut(pane) {
            p.ai = Some(AiState::Asking(request.clone()));
            st.broadcast_tree();
        }
        let logbook = self.cfg.logbook.clone().filter(|_| ai.history && !private);
        let keeper = self.clone();
        let task = tokio::spawn(async move {
            let history = logbook
                .and_then(|path| Logbook::open_readonly(&path).ok())
                .and_then(|book| book.search(shed_logbook::Scope::Local, shed_logbook::Context { tab, bench }, "", None, 10).ok())
                .map(|entries| entries.into_iter().rev().map(|e| e.command).collect())
                .unwrap_or_default();
            let cx = ask::Context { cwd: shed_save::home_relative(&cwd), shell, os: ask::os_name(), history, screen };
            let answer = ask::run(&ai.command, &cwd, ask::prompt(&request, &cx)).await;
            let mut st = keeper.lock();
            if let Some(rt) = st.panes.get_mut(&pane) {
                rt.ask = None;
                if let Ok(command) = &answer {
                    // Replaces whatever is on the line; pasted, so nothing runs.
                    let mut bytes = b"\x05\x15".to_vec();
                    bytes.extend(shed_emu::keys::encode_paste(command, rt.emu.modes()));
                    let _ = rt.input.send(bytes);
                }
            }
            if let Some(p) = st.tree.pane_mut(pane) {
                p.ai = answer.err().map(AiState::Failed);
                st.broadcast_tree();
            }
        });
        if let Some(rt) = st.panes.get_mut(&pane)
            && let Some(old) = rt.ask.replace(task.abort_handle())
        {
            old.abort();
        }
    }

    /// Stops panes as closing their bench would (tools stopped, teardowns
    /// run), then hangs them up.
    async fn stop_panes(self: Arc<Self>, panes: Vec<PaneId>) {
        let steps: Vec<_> = panes.iter().map(|p| tokio::spawn(self.clone().teardown_pane(*p))).collect();
        for step in steps {
            let _ = tokio::time::timeout(TEARDOWN_LIMIT, step).await;
        }
        self.lock().hang_up(&panes);
    }

    /// Types the next queued command at a prompt: run, or left waiting for Enter.
    fn send_after_prompt(&self, st: &mut State, pane: PaneId) {
        let Some(rt) = st.panes.get_mut(&pane) else { return };
        if !rt.waits_for.is_empty() {
            rt.held_at_prompt = true;
            return;
        }
        let Some((command, run)) = rt.after_prompt.pop_front() else {
            // Nothing (more) to start: ready, unless it waits for a text.
            if !rt.startup_pending && rt.ready_text.is_none() {
                self.set_readiness(st, pane, Readiness::Ready);
            }
            return;
        };
        rt.startup_pending = true;
        rt.typed.push(command.clone());
        // Pasted, so a multi-line command is not run line by line.
        let mut bytes = shed_emu::keys::encode_paste(&command, rt.emu.modes());
        if run {
            bytes.push(b'\r');
        }
        let _ = rt.input.send(bytes);
        if !run && let Some(p) = st.tree.pane_mut(pane) {
            p.waiting = Some(command);
            self.tree_dirty.notify_one();
        }
    }

    /// For shells that never show a prompt mark: types the first command anyway.
    fn type_after_prompt(&self, pane: PaneId) {
        let mut st = self.lock();
        if st.panes.get(&pane).is_some_and(|rt| !rt.hooked) {
            self.send_after_prompt(&mut st, pane);
        }
    }

    /// Writes each tab's alias file (global, then bench, then tab aliases)
    /// where its shells read it at every prompt. Unchanged files are left alone.
    fn sync_alias_files(&self, st: &mut State) {
        let dir = self.cfg.state_dir.join("aliases");
        let mut wanted: Vec<(TabId, String)> = Vec::new();
        for bench in &st.tree.benches {
            for tab in &bench.tabs {
                let merged = shed_model::config::shed_aliases(&st.tree.aliases, &bench.aliases, &tab.aliases);
                let text: String = merged
                    .iter()
                    .filter(|(name, _)| !name.is_empty() && !name.contains(['\t', '\n', '=', ' ']))
                    .map(|(name, value)| format!("{name}\t{}\n", value.replace(['\n', '\t'], " ")))
                    .collect();
                wanted.push((tab.id, text));
            }
        }
        for (tab, text) in wanted {
            if st.alias_files.get(&tab) == Some(&text) {
                continue;
            }
            let path = dir.join(tab.to_string());
            let written = std::fs::create_dir_all(&dir).and_then(|()| {
                let tmp = path.with_extension("tmp");
                std::fs::write(&tmp, &text)?;
                std::fs::rename(tmp, &path)
            });
            match written {
                Ok(()) => {
                    st.alias_files.insert(tab, text);
                }
                Err(e) => warn!("cannot write aliases for tab {tab}: {e}"),
            }
        }
    }

    /// Closes a bench: saves it as it is, then in every pane stops the
    /// running tool (Ctrl+C, then SIGTERM), runs the pane's teardown commands
    /// one by one at the prompt, and hangs up. The window shows it happening.
    async fn close_bench(self: Arc<Self>, bench: BenchId) {
        let panes = {
            let mut st = self.lock();
            if let Some(store) = &self.cfg.store {
                for (saved, scrollback) in self.snapshot(&st, &[bench], false) {
                    if let Err(e) = store.save(&saved, &scrollback) {
                        warn!("cannot save bench {:?}: {e}", saved.name);
                    }
                }
            }
            let Some(b) = st.tree.bench_mut(bench) else { return };
            if b.closing {
                return;
            }
            b.closing = true;
            st.broadcast_tree();
            st.pane_rt_ids_in(bench)
        };
        info!(?bench, "closing bench");
        let steps: Vec<_> = panes.into_iter().map(|p| tokio::spawn(self.clone().teardown_pane(p))).collect();
        for step in steps {
            let _ = tokio::time::timeout(TEARDOWN_LIMIT, step).await;
        }
        let mut st = self.lock();
        self.drop_bench(&mut st, bench);
        info!(?bench, "bench closed");
    }

    async fn teardown_pane(self: Arc<Self>, pane: PaneId) {
        let (mut prompts, running, teardown, hooked) = {
            let st = self.lock();
            let Some(rt) = st.panes.get(&pane) else { return };
            let tree_pane = st.tree.pane(pane);
            let running = rt.running.is_some() || tree_pane.is_some_and(|p| p.tool.is_some());
            let teardown = tree_pane.map(|p| p.setup.teardown.clone()).unwrap_or_default();
            (rt.prompts.subscribe(), running, teardown, rt.hooked)
        };
        let send = |bytes: &[u8]| {
            if let Some(rt) = self.lock().panes.get(&pane) {
                let _ = rt.input.send(bytes.to_vec());
            }
        };
        // Waits for the shell's next prompt; without Shed's hook, just a pause.
        async fn next_prompt(rx: &mut tokio::sync::watch::Receiver<u64>, hooked: bool, limit: Duration) -> bool {
            if !hooked {
                tokio::time::sleep(Duration::from_millis(800)).await;
                return true;
            }
            rx.borrow_and_update();
            tokio::time::timeout(limit, rx.changed()).await.is_ok_and(|r| r.is_ok())
        }

        if running {
            send(b"\x03");
            if !next_prompt(&mut prompts, hooked, Duration::from_secs(5)).await {
                // Still running: ask the foreground job to terminate.
                let pgrp = self.lock().panes.get(&pane).and_then(|rt| pty::foreground_pgrp(rt.master.get_ref()));
                if let Some(pg) = pgrp {
                    let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(-pg.as_raw()), nix::sys::signal::Signal::SIGTERM);
                }
                next_prompt(&mut prompts, hooked, Duration::from_secs(3)).await;
            }
        }
        for command in teardown {
            let mut bytes = command.into_bytes();
            bytes.push(b'\r');
            send(&bytes);
            if !next_prompt(&mut prompts, hooked, TEARDOWN_LIMIT).await {
                warn!(?pane, "a teardown command did not finish in time");
                break;
            }
        }
    }

    /// What each bench looks like now, ready to write.
    fn snapshot(&self, st: &State, benches: &[BenchId], shutting_down: bool) -> Vec<(SavedBench, Vec<(PaneId, String)>)> {
        let extras = |pane: PaneId| {
            let rt = st.panes.get(&pane);
            let tree_pane = st.tree.pane(pane);
            let tool = rt
                .and_then(|rt| rt.running.as_ref().map(|r| r.command.clone()))
                .or_else(|| {
                    // At logout a tool may have been stopped just before the keeper.
                    let (cmd, at) = rt?.last_finished.as_ref()?;
                    (shutting_down && at.elapsed() < SHUTDOWN_GRACE).then(|| cmd.clone())
                })
                .or_else(|| tree_pane.and_then(|p| p.waiting.clone()))
                .or_else(|| tree_pane.and_then(|p| p.tool.as_ref()).map(|t| t.display()));
            PaneExtras { tool }
        };
        benches
            .iter()
            .filter_map(|id| st.tree.bench(*id))
            .map(|bench| {
                let saved = SavedBench::from_bench(bench, extras);
                let scrollback = bench
                    .tabs
                    .iter()
                    .flat_map(|t| &t.panes)
                    .filter_map(|p| st.panes.get(&p.id).map(|rt| (p.id, rt.emu.scrollback_text(SAVED_SCROLLBACK))))
                    .collect();
                (saved, scrollback)
            })
            .collect()
    }

    /// Saves every running bench and the list of them.
    fn save_all(&self, st: &State, shutting_down: bool) {
        let Some(store) = &self.cfg.store else { return };
        let ids: Vec<BenchId> = st.tree.benches.iter().map(|b| b.id).collect();
        for (bench, scrollback) in self.snapshot(st, &ids, shutting_down) {
            if let Err(e) = store.save(&bench, &scrollback) {
                warn!("cannot save bench {:?}: {e}", bench.name);
            }
        }
        if !self.quitting.load(Ordering::SeqCst)
            && let Err(e) = store.save_session(&session_of(st))
        {
            warn!("cannot save the session: {e}");
        }
    }

    /// Closes every bench (saved, tools stopped, teardowns run), then the
    /// keeper. What ran, and with a window, is recorded first for `auto_restore`.
    async fn quit_shed(self: Arc<Self>) {
        if self.quitting.swap(true, Ordering::SeqCst) {
            return;
        }
        let benches: Vec<BenchId> = {
            let st = self.lock();
            if let Some(store) = &self.cfg.store
                && let Err(e) = store.save_session(&session_of(&st))
            {
                warn!("cannot save the session: {e}");
            }
            st.tree.benches.iter().map(|b| b.id).collect()
        };
        let closing: Vec<_> = benches.into_iter().map(|b| tokio::spawn(self.clone().close_bench(b))).collect();
        for step in closing {
            let _ = step.await;
        }
        // A bench already closing before Shed was closed finishes on its own.
        let deadline = Instant::now() + TEARDOWN_LIMIT;
        while !self.lock().tree.benches.is_empty() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        info!("every bench closed; stopping");
        self.shutdown.notify_one();
    }

    /// Saves everything every `autosave_seconds` (60 by default).
    async fn autosave_loop(self: Arc<Self>) {
        loop {
            let secs = self.lock().config.autosave_seconds.unwrap_or(60).max(5);
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(secs)) => {}
                // Edits to setup and aliases are saved within a second.
                _ = self.save_soon.notified() => tokio::time::sleep(Duration::from_secs(1)).await,
            }
            // Collect under the lock, write without it.
            let (store, work, session) = {
                let st = self.lock();
                let Some(store) = self.cfg.store.clone() else { return };
                let ids: Vec<BenchId> = st.tree.benches.iter().map(|b| b.id).collect();
                let session = (!self.quitting.load(Ordering::SeqCst)).then(|| session_of(&st));
                (store, self.snapshot(&st, &ids, false), session)
            };
            let _ = tokio::task::spawn_blocking(move || {
                for (bench, scrollback) in work {
                    if let Err(e) = store.save(&bench, &scrollback) {
                        warn!("cannot save bench {:?}: {e}", bench.name);
                    }
                }
                if let Some(session) = session {
                    let _ = store.save_session(&session);
                }
            })
            .await;
        }
    }

    /// Stops the keeper: saves everything first, with the tree frozen so
    /// shells exiting meanwhile don't take their benches with them.
    pub fn finish(&self) {
        if self.finished.swap(true, Ordering::SeqCst) {
            return;
        }
        self.stopping.store(true, Ordering::SeqCst);
        {
            let st = self.lock();
            self.save_all(&st, true);
        }
        info!("saved; stopping");
        self.hang_up_all();
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
        let name = name.unwrap_or_else(|| self.unique_bench_name(st));
        let color = BenchColor::next_unused(st.tree.benches.iter().map(|b| &b.color));
        let launcher = self.unique_launcher(st, &name);
        st.tree.benches.push(Bench {
            id,
            launcher,
            name,
            color,
            badge: None,
            cwd,
            tabs: vec![tab],
            active_tab: tab_id,
            aliases: Default::default(),
            closing: false,
            window: None,
        });
        self.sync_alias_files(st);
        Ok(id)
    }

    fn spawn_pane(
        self: &Arc<Self>,
        st: &mut State,
        bench: BenchId,
        tab: TabId,
        cwd: PathBuf,
    ) -> io::Result<Pane> {
        self.spawn_pane_spec(st, bench, tab, PaneSpec::fresh(cwd))
    }

    fn spawn_pane_spec(
        self: &Arc<Self>,
        st: &mut State,
        bench: BenchId,
        tab: TabId,
        spec: PaneSpec,
    ) -> io::Result<Pane> {
        let pane_id = spec.id;
        let (cols, rows) = (spec.cols.max(2), spec.rows.max(1));
        let cwd = spec.cwd;
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
        // The hooks hand lines starting with `>` to the AI prompt only when it is on.
        if st.config.ai.enabled() {
            env.push(("SHED_AI".into(), "1".into()));
        }
        if let Some(bin) = &self.cfg.shed_bin {
            env.push(("SHED_BIN".into(), bin.clone().into()));
            // `shed setup` and `shed alias` work in every pane, even from a build
            // that is not installed; an installed `shed` earlier on PATH still wins.
            if let Some(dir) = bin.parent() {
                let path = env.iter().rev().find(|(k, _)| k == "PATH").map(|(_, v)| v.clone());
                let path = path.or_else(|| std::env::var_os("PATH")).unwrap_or_default();
                let mut dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
                if !dirs.iter().any(|d| d == dir) {
                    dirs.push(dir.to_path_buf());
                    if let Ok(joined) = std::env::join_paths(dirs) {
                        env.push(("PATH".into(), joined));
                    }
                }
            }
        }
        let mut args = Vec::new();
        let name = program.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if name == "bash"
            && st.config.shell.integration
            && let Some(rc) = &self.bash_rc
        {
            args = vec!["--rcfile".into(), rc.clone().into()];
        }
        if name == "zsh" {
            if st.config.shell.integration
                && let Some(dir) = &self.zsh_dir
            {
                // Its .zshenv puts the user's own ZDOTDIR back.
                if let Some(own) = std::env::var_os("ZDOTDIR") {
                    env.push(("SHED_USER_ZDOTDIR".into(), own));
                }
                env.push(("ZDOTDIR".into(), dir.clone().into()));
            }
            // As macOS terminals do, so ~/.zprofile sets up PATH (Homebrew).
            if cfg!(target_os = "macos") {
                args.push("-l".into());
            }
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

        let mut emu = AlacrittyEmulator::new(GridSize::new(cols, rows), st.palette.clone());
        if let Some(bytes) = &spec.restore {
            emu.advance(bytes);
        }
        if !spec.after_prompt.is_empty() {
            // Shells without Shed's hook never send a prompt mark.
            let keeper = self.clone();
            tokio::spawn(async move {
                tokio::time::sleep(PROMPT_WAIT).await;
                keeper.type_after_prompt(pane_id);
            });
        }
        st.panes.insert(pane_id, PaneRt {
            bench,
            emu,
            reply_guard: ReplyGuard::default(),
            input: input_tx,
            master: master.clone(),
            pid,
            marks: MarkScanner::new(),
            running: None,
            last_finished: None,
            after_prompt: spec.after_prompt,
            prompts: tokio::sync::watch::channel(0).0,
            hooked: false,
            new_prompt: false,
            reports_ends: false,
            ask: None,
            waits_for: Vec::new(),
            held_at_prompt: false,
            readiness: Readiness::Pending,
            ready_text: None,
            plain: Default::default(),
            seen: Vec::new(),
            typed: Vec::new(),
            startup_pending: false,
        });
        tokio::spawn(write_loop(master.clone(), input_rx));
        tokio::spawn(self.clone().pane_loop(pane_id, master, pty.child));
        info!(pane = ?pane_id, ?pid, "pane started");
        let mut pane = Pane::new(pane_id, cwd);
        pane.cols = cols;
        pane.rows = rows;
        Ok(pane)
    }

    /// Reads a pane's output until its shell exits, then reports the exit.
    ///
    /// A bench closed and loaded again at once brings its panes back under
    /// the same IDs while the old shells are still exiting; `master` tells
    /// this loop's pane from the new one, which it must leave alone.
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
                        Ok(Ok(n)) => sync_deadline = self.on_output(pane, &master, &buf[..n]).map(Into::into),
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
                    let rt = st.panes.get_mut(&pane).filter(|rt| Arc::ptr_eq(&rt.master, &master));
                    sync_deadline = rt.and_then(|rt| rt.emu.tick()).map(Into::into);
                }
            }
        }

        let status = match status {
            Some(s) => s,
            None => tokio::time::timeout(Duration::from_secs(2), child.wait()).await.ok().and_then(Result::ok),
        };
        let code = status.and_then(|s| s.code());
        info!(?pane, ?code, "pane exited");
        if self.stopping.load(Ordering::Relaxed) {
            // Shutting down: the final save needs the tree as it was.
            return;
        }
        {
            let mut st = self.lock();
            if !st.panes.get(&pane).is_some_and(|rt| Arc::ptr_eq(&rt.master, &master)) {
                // Its bench was closed and is already back: the pane is someone else's now.
                return;
            }
            // Panes waiting for it never will be released by it.
            self.set_readiness(&mut st, pane, Readiness::Failed);
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
    fn on_mark(self: &Arc<Self>, st: &mut State, pane: PaneId, mark: Mark) {
        match mark {
            Mark::Cwd(dir) => {
                if let Some(p) = st.tree.pane_mut(pane)
                    && p.cwd.as_os_str() != dir.as_str()
                {
                    p.cwd = dir.into();
                    self.tree_dirty.notify_one();
                }
            }
            Mark::InputStart => {
                let Some(rt) = st.panes.get_mut(&pane) else { return };
                rt.hooked = true;
                if std::mem::take(&mut rt.new_prompt) || !rt.reports_ends {
                    rt.prompts.send_modify(|n| *n += 1);
                    self.send_after_prompt(st, pane);
                }
            }
            Mark::ShellAliases(text) => {
                if let Some(p) = st.tree.pane_mut(pane) {
                    p.shell_aliases = shed_emu::marks::parse_alias_list(&text).into_iter().collect();
                    self.tree_dirty.notify_one();
                }
            }
            // Any command running ends the wait, even one bash leaves out of
            // history (and so sends no command text for).
            Mark::Executed => {
                if let Some(p) = st.tree.pane_mut(pane)
                    && p.waiting.take().is_some()
                {
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
                let Some(rt) = st.panes.get_mut(&pane) else { return };
                // Sent before every new prompt, never with a redraw.
                rt.new_prompt = true;
                rt.reports_ends = true;
                if std::mem::take(&mut rt.startup_pending) && exit.is_some_and(|code| code != 0) && rt.ready_text.is_none() {
                    self.set_readiness(st, pane, Readiness::Failed);
                }
                let Some(rt) = st.panes.get_mut(&pane) else { return };
                let Some(run) = rt.running.take() else { return };
                rt.last_finished = Some((run.command.clone(), Instant::now()));
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
            Mark::Ask(request) => self.ask(st, pane, request),
            Mark::PromptStart | Mark::Walk(_) | Mark::WalkEnd => {}
        }
    }

    /// Feeds output to the keeper's emulator and forwards it to attached windows.
    /// Returns when the emulator next needs a tick.
    fn on_output(self: &Arc<Self>, pane: PaneId, master: &Arc<AsyncFd<OwnedFd>>, bytes: &[u8]) -> Option<Instant> {
        let mut st = self.lock();
        let rt = st.panes.get_mut(&pane).filter(|rt| Arc::ptr_eq(&rt.master, master))?;
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
        let mut ready = false;
        if rt.readiness == Readiness::Pending
            && let Some(text) = rt.ready_text.clone()
        {
            rt.plain.feed(bytes, &mut rt.seen);
            let mut seen = String::from_utf8_lossy(&rt.seen).into_owned();
            for typed in &rt.typed {
                seen = seen.replace(typed.as_str(), "");
            }
            ready = seen.contains(&text);
            // Enough to find the text across chunks, not the whole history.
            let keep = 4096 + text.len();
            if rt.seen.len() > keep {
                rt.seen.drain(..rt.seen.len() - keep);
            }
        }
        let mut marks = Vec::new();
        rt.marks.scan(bytes, &mut marks);
        st.broadcast_to_bench(bench, Frame::encode_output(pane, bytes).into());
        for mark in marks {
            self.on_mark(&mut st, pane, mark);
        }
        if ready {
            self.set_readiness(&mut st, pane, Readiness::Ready);
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

/// Writes the `ZDOTDIR` zsh starts with in Shed's panes. Its `.zshenv` puts
/// the user's ZDOTDIR back (so zsh goes on to their .zprofile and .zshrc),
/// runs their .zshenv, and loads Shed's hook, which finishes setting up at
/// the first prompt, after .zshrc.
fn write_zsh_dir(dir: &Path) -> Option<PathBuf> {
    let zdotdir = dir.join("zsh");
    let hook = zdotdir.join("shed.zsh");
    let zshenv = format!(
        "# Written by shedd: your own zsh startup files, then Shed's shell integration.\n\
         if [[ -n ${{SHED_USER_ZDOTDIR+x}} ]]; then\n\
         \x20   ZDOTDIR=$SHED_USER_ZDOTDIR\n\
         \x20   unset SHED_USER_ZDOTDIR\n\
         else\n\
         \x20   unset ZDOTDIR\n\
         fi\n\
         [[ -f ${{ZDOTDIR:-$HOME}}/.zshenv ]] && builtin source ${{ZDOTDIR:-$HOME}}/.zshenv\n\
         [[ -o interactive ]] && builtin source {:?}\n",
        hook.display().to_string()
    );
    let written = std::fs::create_dir_all(&zdotdir)
        .and_then(|()| std::fs::write(&hook, shed_logbook::ZSH_HOOK))
        .and_then(|()| std::fs::write(zdotdir.join(".zshenv"), zshenv));
    match written {
        Ok(()) => Some(zdotdir),
        Err(e) => {
            warn!("cannot write {}: {e}", zdotdir.display());
            None
        }
    }
}

/// The last lines with text on them, for the pegboard.
fn preview(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    let end = lines.iter().rposition(|l| !l.is_empty()).map_or(0, |i| i + 1);
    let start = end.saturating_sub(PREVIEW_LINES);
    lines[start..end].iter().map(|l| l.chars().take(PREVIEW_COLS).collect()).collect()
}

/// The benches running now, and which have a window.
fn session_of(st: &State) -> Vec<SessionBench> {
    st.tree.benches.iter().map(|b| SessionBench { name: b.name.clone(), open: st.shown.contains(&b.id) }).collect()
}

/// The colours the keeper answers apps' queries with: the theme in effect.
fn apply_theme(st: &mut State) {
    let (theme, error) = st.config.theme(st.dark);
    if let Some(e) = error {
        warn!("{e}");
    }
    st.palette = Palette::from_theme(&theme);
    let palette = st.palette.clone();
    for rt in st.panes.values_mut() {
        rt.emu.set_palette(palette.clone());
    }
}

/// How a pane is named to the user: its tab's title, and which pane if the
/// tab has several.
fn pane_label(tree: &Shed, pane: PaneId) -> String {
    let Some((_, tab)) = tree.locate_pane(pane) else { return "a closed pane".into() };
    let Some(tab) = tree.tab(tab) else { return "a closed pane".into() };
    let leaves = tab.layout.leaves();
    match leaves.iter().position(|p| *p == pane) {
        Some(i) if leaves.len() > 1 => format!("{} · pane {}", tab.display_title(), i + 1),
        _ => tab.display_title(),
    }
}
