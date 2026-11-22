//! The link's own task: it hears the keeper and the nexus, keeps the tree,
//! the subscriptions and the watched terminals, publishes the state, and
//! hands calls to `calls`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use shed_api::calls::{Call, Reply};
use shed_api::state::WorkspaceState;
use shed_api::topics::{Event, Topic};
use shed_api::wire::{Ended, LinkFrame, LinkHello, NexusFrame, Policy, Response};
use shed_api::{ApiError, ErrorCode};
use shed_model::{BenchId, PaneId, Shed, TabId};
use shed_proto::{ClientKind, ClientMsg, Frame, KeeperMsg, PegboardInfo};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, info, warn};

use crate::calls::{self, Ask, Ctx, forbidden_private, not_found};
use crate::nexus::{self, Dial, NexusEvent};
use crate::notify::Notifier;
use crate::pairing::{LinkStatus, now};
use crate::screen::{self, Term};
use crate::topics::{self, Out, ProjectWatch};
use crate::tunnel::Tunnels;
use crate::{Exit, Options};

/// The state goes out at most this often.
const STATE_EVERY: Duration = Duration::from_millis(250);
/// A watched screen's changes go out at most this often.
const SCREEN_EVERY: Duration = Duration::from_millis(33);
/// The closed benches are asked for again this often (and on tree changes).
const PEGBOARD_EVERY: Duration = Duration::from_secs(10);
/// A pane attached only to type into is let go after this long.
const INPUT_KEEP: Duration = Duration::from_secs(30);
/// Input waiting for a pane's replay goes with plain modes after this long.
const INPUT_WAIT: Duration = Duration::from_secs(2);

/// Runs the link until the keeper goes away or the nexus refuses it.
pub async fn run(opts: Options) -> anyhow::Result<Exit> {
    let socket = opts.socket.clone();
    let (keeper, keeper_rx) = tokio::task::spawn_blocking(move || shed_client::keeper_conn::connect_at(&socket, ClientKind::Link)).await??;
    let tls = crate::tls::client_config(opts.ca_file.as_deref())?;
    let url = crate::tls::link_url(&opts.url)?;
    let (tree_tx, tree_rx) = watch::channel(Arc::new(Shed::default()));
    let (ask_tx, mut ask_rx) = mpsc::unbounded_channel();
    let ctx = Arc::new(Ctx::new(keeper.clone(), opts.socket.clone(), tree_rx, ask_tx, opts.logbook.clone(), opts.pictures.clone(), opts.uploads.clone()));
    // Unbounded in frames; tunnels' data in it is bounded in bytes (`Backlog`).
    let (nexus_tx, mut nexus_rx) = mpsc::unbounded_channel();
    let backlog = crate::nexus::Backlog::default();
    let hello = LinkHello { api: shed_api::API_VERSION, version: env!("CARGO_PKG_VERSION").into(), policy: opts.policy };
    let dial = Dial { url: url.clone(), token: opts.token.clone(), hello, tls, backlog: backlog.clone() };
    let nexus_task = tokio::spawn(nexus::run(dial, nexus_tx));
    info!(nexus = %url, policy = ?opts.policy, "link starting");

    let (tunnels, mut tunnel_ended) = Tunnels::new(&opts.socket);
    let mut link = Link::new(ctx, tree_tx, opts, tunnels, backlog);
    link.status("connecting", None);
    keeper.send_msg(ClientMsg::Status);
    link.ask_pegboard();
    let mut tick = tokio::time::interval(SCREEN_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let exit = loop {
        tokio::select! {
            frame = keeper_rx.recv() => match frame {
                Ok(frame) => link.keeper(frame),
                Err(_) => break Exit::KeeperGone,
            },
            Some(event) = nexus_rx.recv() => {
                if let Some(exit) = link.nexus(event) {
                    break exit;
                }
            }
            Some(ask) = ask_rx.recv() => link.ask(ask),
            Some(id) = tunnel_ended.recv() => link.tunnels.ended(id),
            _ = tick.tick() => {
                if let Some(exit) = link.tick() {
                    break exit;
                }
            }
        }
    };
    nexus_task.abort();
    link.disconnected();
    link.status(if matches!(exit, Exit::Refused(_)) { "refused" } else { "stopped" }, match &exit {
        Exit::Refused(why) => Some(why.clone()),
        _ => None,
    });
    Ok(exit)
}

/// A subscription.
enum Sub {
    /// An AI tab's conversation; `started` once its whole has been sent.
    Ai { tab: TabId, started: bool },
    Screen(PaneId),
    /// A project or AI tab's folder followed on disk (the watch stops when
    /// dropped).
    Project(TabId, #[allow(dead_code)] ProjectWatch),
    Search(TabId, Arc<AtomicBool>),
}

impl Sub {
    /// The tab whose contents it streams, for those that follow a tab.
    fn tab(&self) -> Option<TabId> {
        match self {
            Sub::Ai { tab, .. } | Sub::Project(tab, _) | Sub::Search(tab, _) => Some(*tab),
            Sub::Screen(_) => None,
        }
    }
}

/// A pane the link follows: its emulator once the keeper replayed it.
#[derive(Default)]
struct Watched {
    term: Option<Term>,
    /// The bench it was attached through.
    bench: Option<BenchId>,
    /// Asked the keeper for its replay (again).
    replay_asked: bool,
    /// Typed before the replay came, and when.
    input: Vec<(Vec<shed_api::calls::InputItem>, Instant)>,
    /// Scrollback asked for before the replay came.
    scrollback: Vec<(u32, u32, u32)>,
    /// Kept only to type into, until then.
    keep_until: Option<Instant>,
}

/// An AI tab's conversation as the keeper sent it.
#[derive(Default)]
struct Conversation {
    entries: Vec<shed_ai::Entry>,
    /// Sent from the start since the link connected.
    whole: bool,
    waiting: Vec<oneshot::Sender<Vec<shed_ai::Entry>>>,
}

struct Link {
    ctx: Arc<Ctx>,
    opts: Options,
    tree: Arc<Shed>,
    tree_tx: watch::Sender<Arc<Shed>>,
    pegboard: Option<Arc<PegboardInfo>>,
    pegboard_waiting: Vec<oneshot::Sender<Arc<PegboardInfo>>>,
    pegboard_asked: Option<Instant>,
    pegboard_due: Instant,
    /// Benches a window shows (from the keeper's status).
    open: HashSet<BenchId>,
    limits: Option<shed_ai::Limits>,
    host: String,
    home: String,
    /// To the nexus, while connected.
    out: Option<Out>,
    /// Tunnels' data to the nexus, while connected.
    data: Option<mpsc::Sender<Vec<u8>>>,
    tunnels: Tunnels,
    sent_state: Option<WorkspaceState>,
    state_at: Instant,
    state_dirty: bool,
    subs: HashMap<u32, Sub>,
    watched: HashMap<PaneId, Watched>,
    attached: HashSet<BenchId>,
    /// Panes whose size a connector holds, with the size they had.
    held: HashMap<PaneId, (u16, u16)>,
    conversations: HashMap<TabId, Conversation>,
    notifier: Notifier,
    /// The `[nexus]` settings the link runs with; another in `shed.toml`
    /// stops it, to start again with them.
    nexus_config: shed_model::config::NexusConfig,
    config_checked: Instant,
    /// `shed.toml` read when last checked: while it does not, new tunnels
    /// are refused (whether desktops may come in is not known).
    config_reads: bool,
    /// Tunnel data from the nexus not taken yet.
    backlog: crate::nexus::Backlog,
}

impl Link {
    fn new(ctx: Arc<Ctx>, tree_tx: watch::Sender<Arc<Shed>>, opts: Options, tunnels: Tunnels, backlog: crate::nexus::Backlog) -> Link {
        // From the same read as the policy: reading again here could see a
        // newer file and miss that the link runs with older settings.
        let nexus_config = opts.settings.clone().unwrap_or_default();
        let mut host = [0u8; 256];
        // SAFETY: the buffer is writable for its whole length.
        let host = if unsafe { libc::gethostname(host.as_mut_ptr().cast(), host.len()) } == 0 {
            let end = host.iter().position(|b| *b == 0).unwrap_or(host.len());
            String::from_utf8_lossy(&host[..end]).into_owned()
        } else {
            String::new()
        };
        Link {
            ctx,
            tree: Arc::new(Shed::default()),
            tree_tx,
            pegboard: None,
            pegboard_waiting: Vec::new(),
            pegboard_asked: None,
            pegboard_due: Instant::now() + PEGBOARD_EVERY,
            open: HashSet::new(),
            limits: None,
            host,
            home: std::env::var("HOME").unwrap_or_default(),
            out: None,
            data: None,
            tunnels,
            sent_state: None,
            state_at: Instant::now() - STATE_EVERY,
            state_dirty: true,
            subs: HashMap::new(),
            watched: HashMap::new(),
            attached: HashSet::new(),
            held: HashMap::new(),
            conversations: HashMap::new(),
            notifier: Notifier::default(),
            nexus_config,
            config_checked: Instant::now(),
            config_reads: true,
            backlog,
            opts,
        }
    }

    fn status(&self, state: &str, error: Option<String>) {
        if let Some(path) = &self.opts.status_file {
            LinkStatus { pid: std::process::id(), state: state.into(), name: None, error, since: now() }.write(path);
        }
    }

    fn send(&self, frame: LinkFrame) {
        if let Some(out) = &self.out {
            let _ = out.send(frame);
        }
    }

    fn respond(&self, id: u32, result: Result<Reply, ApiError>) {
        self.send(LinkFrame::Response(match result {
            Ok(reply) => Response::ok(id, reply),
            Err(e) => Response::error(id, e),
        }));
    }

    fn ended(&self, sub: u32, error: Option<ApiError>) {
        self.send(LinkFrame::Ended(Ended { sub, error }));
    }

    fn ask_pegboard(&mut self) {
        if self.pegboard_asked.is_none_or(|t| t.elapsed() > Duration::from_secs(5)) {
            self.ctx.keeper.send_msg(ClientMsg::Pegboard);
            self.pegboard_asked = Some(Instant::now());
        }
    }

    // ---------------------------------------------------------- the keeper

    fn keeper(&mut self, frame: Frame) {
        match frame {
            Frame::Keeper(msg) => self.keeper_msg(msg),
            Frame::Output { pane, bytes } => {
                if let Some(term) = self.watched.get_mut(&pane).and_then(|w| w.term.as_mut()) {
                    term.output(&bytes);
                }
            }
            Frame::Input { .. } | Frame::Client(_) => {}
        }
    }

    fn keeper_msg(&mut self, msg: KeeperMsg) {
        match msg {
            KeeperMsg::TreeChanged(tree) => self.tree_changed(tree),
            KeeperMsg::Pegboard(p) => {
                let p = Arc::new(p);
                for w in self.pegboard_waiting.drain(..) {
                    let _ = w.send(p.clone());
                }
                self.pegboard = Some(p);
                self.pegboard_asked = None;
                self.pegboard_due = Instant::now() + PEGBOARD_EVERY;
                self.state_dirty = true;
            }
            KeeperMsg::Status(s) => {
                self.open = s.open.into_iter().collect();
                self.tree_changed(s.tree);
            }
            KeeperMsg::AiLimits(l) => {
                self.limits = Some(l);
                self.state_dirty = true;
            }
            KeeperMsg::Replay { pane, cols, rows, bytes } => self.replayed(pane, cols, rows, &bytes),
            KeeperMsg::AiUpdate { tab, from, entries, status } => self.ai_update(tab, from, entries, status),
            KeeperMsg::AiEntries { tab, entries, status } => self.ai_entries(tab, entries, status),
            KeeperMsg::AiInfo { tab, commands, models } => {
                let info = topics::ai_info(&commands, &models);
                for (id, sub) in &self.subs {
                    if matches!(sub, Sub::Ai { tab: t, .. } if *t == tab) {
                        self.send(LinkFrame::Event(shed_api::wire::EventFrame { sub: *id, event: Event::AiInfo(info.clone()) }));
                    }
                }
            }
            KeeperMsg::Error(e) => debug!("keeper: {e}"),
            KeeperMsg::Welcome { .. }
            | KeeperMsg::Attached { .. }
            | KeeperMsg::Show { .. }
            | KeeperMsg::PaneExited { .. }
            | KeeperMsg::Done(_)
            | KeeperMsg::ShowShed
            | KeeperMsg::Hide { .. } => {}
        }
    }

    fn tree_changed(&mut self, tree: Shed) {
        let changed_benches = tree.benches.len() != self.tree.benches.len()
            || tree.benches.iter().zip(&self.tree.benches).any(|(a, b)| a.id != b.id || a.name != b.name || a.tabs.len() != b.tabs.len());
        self.tree = Arc::new(tree);
        let _ = self.tree_tx.send(self.tree.clone());
        self.state_dirty = true;
        if changed_benches {
            // Which benches windows show, and the closed ones, may have changed too.
            self.ctx.keeper.send_msg(ClientMsg::Status);
            self.ask_pegboard();
        }
        let tree = self.tree.clone();
        for n in self.notifier.changed(&tree, Instant::now()) {
            self.send(LinkFrame::Notify(n));
        }
        // Watched panes that are gone, or private now, end their subscriptions.
        let gone: Vec<PaneId> = self
            .watched
            .keys()
            .copied()
            .filter(|p| tree.locate_pane(*p).and_then(|(_, t)| tree.tab(t)).is_none_or(|t| t.private))
            .collect();
        for pane in gone {
            let private = tree.locate_pane(pane).is_some();
            let subs: Vec<u32> = self.subs.iter().filter(|(_, s)| matches!(s, Sub::Screen(p) if *p == pane)).map(|(id, _)| *id).collect();
            for id in subs {
                self.subs.remove(&id);
                self.ended(id, Some(if private { forbidden_private() } else { not_found("the pane closed") }));
            }
            self.watched.remove(&pane);
            self.held.remove(&pane);
        }
        // So do AI tabs, projects and searches whose tab is gone or private.
        let tab_gone: Vec<u32> = self
            .subs
            .iter()
            .filter(|(_, s)| s.tab().is_some_and(|tab| tree.tab(tab).is_none_or(|t| t.private)))
            .map(|(id, _)| *id)
            .collect();
        for id in tab_gone {
            let Some(sub) = self.subs.remove(&id) else { continue };
            let private = sub.tab().is_some_and(|tab| tree.tab(tab).is_some());
            if let Sub::Search(_, cancel) = &sub {
                cancel.store(true, Ordering::Relaxed);
            }
            drop(sub);
            self.ended(id, Some(if private { forbidden_private() } else { not_found("the tab closed") }));
        }
        // A pane the GUI resized: its emulator is out of step until replayed.
        for (pane, w) in &mut self.watched {
            if let (Some(term), Some(p)) = (&w.term, tree.pane(*pane))
                && term.size() != (p.cols.max(2), p.rows.max(1))
                && !w.replay_asked
            {
                self.ctx.keeper.send_msg(ClientMsg::Replay { pane: *pane });
                w.replay_asked = true;
            }
        }
        self.sync_attached();
    }

    /// Attaches the benches of watched panes (the keeper replays their panes
    /// and sends their output), detaches those no longer needed.
    fn sync_attached(&mut self) {
        let mut want = HashSet::new();
        for (pane, w) in &mut self.watched {
            if let Some((bench, _)) = self.tree.locate_pane(*pane) {
                want.insert(bench);
                if w.bench != Some(bench) && w.bench.is_some() {
                    // Moved to another bench: replayed from there.
                    w.term = None;
                }
                w.bench = Some(bench);
            }
        }
        for bench in want.difference(&self.attached) {
            self.ctx.keeper.send_msg(ClientMsg::Attach { bench: Some(*bench) });
        }
        for bench in self.attached.difference(&want) {
            self.ctx.keeper.send_msg(ClientMsg::Detach { bench: *bench });
        }
        self.attached = want;
    }

    fn replayed(&mut self, pane: PaneId, cols: u16, rows: u16, bytes: &[u8]) {
        let Some(w) = self.watched.get_mut(&pane) else { return };
        w.term = Some(Term::from_replay(cols, rows, bytes));
        w.replay_asked = false;
        let modes = w.term.as_ref().unwrap().modes();
        for (items, _) in w.input.drain(..) {
            self.ctx.keeper.send(Frame::Input { pane, bytes: screen::encode(&items, modes) });
        }
        let asked: Vec<(u32, u32, u32)> = w.scrollback.drain(..).collect();
        for (id, from, count) in asked {
            let lines = self.watched[&pane].term.as_ref().unwrap().scrollback(from, count);
            self.respond(id, Ok(Reply::Lines(lines)));
        }
    }

    fn ai_update(&mut self, tab: TabId, from: u32, entries: Vec<shed_ai::Entry>, status: shed_ai::Status) {
        let conv = self.conversations.entry(tab).or_default();
        if from == 0 {
            conv.whole = true;
        }
        if conv.whole && from as usize <= conv.entries.len() {
            conv.entries.truncate(from as usize);
            conv.entries.extend(entries.iter().cloned());
            for w in conv.waiting.drain(..) {
                let _ = w.send(conv.entries.clone());
            }
        }
        let event = topics::transcript(from, &entries, &status);
        let subs: Vec<u32> = self
            .subs
            .iter_mut()
            .filter_map(|(id, s)| match s {
                Sub::Ai { tab: t, started } if *t == tab && (*started || from == 0) => {
                    *started = true;
                    Some(*id)
                }
                _ => None,
            })
            .collect();
        for id in subs {
            self.send(LinkFrame::Event(shed_api::wire::EventFrame { sub: id, event: Event::AiTranscript(event.clone()) }));
        }
    }

    /// Entries changed in place (a sub-agent's progress): into the link's
    /// copy, and to the tab's subscriptions, each entry alone.
    fn ai_entries(&mut self, tab: TabId, changed: Vec<(u32, shed_ai::Entry)>, status: shed_ai::Status) {
        if let Some(conv) = self.conversations.get_mut(&tab).filter(|c| c.whole) {
            for (i, e) in &changed {
                if let Some(slot) = conv.entries.get_mut(*i as usize) {
                    *slot = e.clone();
                }
            }
        }
        let subs: Vec<u32> = self.subs.iter().filter(|(_, s)| matches!(s, Sub::Ai { tab: t, started: true } if *t == tab)).map(|(id, _)| *id).collect();
        if subs.is_empty() {
            return;
        }
        let event = topics::changed(&changed, &status);
        for id in subs {
            self.send(LinkFrame::Event(shed_api::wire::EventFrame { sub: id, event: Event::AiEntries(event.clone()) }));
        }
    }

    // ------------------------------------------------------------ the nexus

    fn nexus(&mut self, event: NexusEvent) -> Option<Exit> {
        match event {
            NexusEvent::Connected { out, data, welcome } => {
                info!(workspace = %welcome.workspace, name = %welcome.name, "connected to the nexus");
                if let Some(path) = &self.opts.status_file {
                    LinkStatus { pid: std::process::id(), state: "connected".into(), name: Some(welcome.name.clone()), error: None, since: now() }.write(path);
                }
                self.out = Some(out);
                self.data = Some(data);
                // Everything again: the nexus keeps only the last state.
                self.sent_state = None;
                self.state_dirty = true;
                self.publish(true);
            }
            NexusEvent::Disconnected(why) => {
                self.disconnected();
                self.status("offline", Some(why));
            }
            NexusEvent::Refused(e) => return Some(Exit::Refused(e.to_string())),
            NexusEvent::Unknown { request, sub } => {
                let e = || ApiError::new(ErrorCode::Unsupported, "this workspace's Shed does not know that; update it");
                if let Some(id) = request {
                    self.respond(id, Err(e()));
                }
                if let Some(sub) = sub {
                    self.ended(sub, Some(e()));
                }
            }
            NexusEvent::Frame(frame) => match frame {
                NexusFrame::Request(r) => self.request(r.id, r.call),
                NexusFrame::Subscribe(s) => self.subscribe(s.id, s.topic),
                NexusFrame::Unsubscribe(u) => self.unsubscribe(u.id),
                NexusFrame::TunnelOpen(t) => self.tunnel_open(t.id, t.device.map(|d| shed_proto::TunnelDevice { id: d.id, name: d.name })),
                NexusFrame::TunnelClose(t) => self.tunnels.close(t.id),
                NexusFrame::Welcome(_) | NexusFrame::Goodbye(_) => {}
            },
            NexusEvent::Data(id, bytes) => {
                self.backlog.taken(bytes.len());
                self.tunnels.data(id, bytes, self.out.as_ref());
            }
            NexusEvent::Overflow(id) => {
                warn!(id, "a tunnel sent more than the link keeps waiting; it was closed");
                self.tunnels.close(id);
                let error = ApiError::new(ErrorCode::Failed, "the workspace's link could not keep up; the tunnel was closed");
                self.send(LinkFrame::TunnelClosed(shed_api::wire::TunnelClosed { id, error: Some(error) }));
            }
        }
        None
    }

    /// A desktop asks for a tunnel to the keeper: only if this workspace
    /// lets desktops in, with full control.
    fn tunnel_open(&mut self, id: u32, device: Option<shed_proto::TunnelDevice>) {
        let (Some(out), Some(data)) = (self.out.clone(), self.data.clone()) else { return };
        let who = device.as_ref().map_or_else(|| "a desktop".to_string(), |d| format!("{} ({})", d.name, d.id));
        let refuse = |why: &str| {
            info!("{who}'s tunnel refused: {why}");
            let _ = out.send(LinkFrame::TunnelClosed(shed_api::wire::TunnelClosed { id, error: Some(ApiError::new(ErrorCode::Forbidden, why)) }));
        };
        if !self.config_reads {
            return refuse("this workspace's shed.toml does not read; desktops are let in again once it does");
        }
        if !self.opts.desktop_clients {
            return refuse("this workspace does not let Shed's desktop app in ([nexus] desktop_clients = true in its shed.toml)");
        }
        if self.opts.policy != Policy::Full {
            return refuse("this workspace lets connectors look only ([nexus] control = \"view\"); desktops need full control");
        }
        self.tunnels.open(id, device, out, data);
    }

    /// The nexus is gone: every subscription with it, and every tunnel.
    fn disconnected(&mut self) {
        self.out = None;
        self.data = None;
        self.tunnels.close_all();
        let ids: Vec<u32> = self.subs.keys().copied().collect();
        for id in ids {
            self.unsubscribe(id);
        }
    }

    fn request(&mut self, id: u32, call: Call) {
        debug!(id, call = ?std::mem::discriminant(&call), "request");
        if call.changes() && self.opts.policy == Policy::View {
            return self.respond(id, Err(ApiError::new(ErrorCode::Forbidden, "this workspace lets connectors look, not change things")));
        }
        // Browsing the machine's folders is for choosing where a new tab
        // works: a view-only workspace makes no tabs, and shows no more of
        // its disk than its tabs' folders.
        if matches!(call, Call::BrowseFolders(_) | Call::Browse(_)) && self.opts.policy == Policy::View {
            return self.respond(id, Err(ApiError::new(ErrorCode::Forbidden, "this workspace lets connectors look at its tabs only")));
        }
        match call {
            Call::Input(a) => {
                let r = self.input(a);
                self.respond(id, r)
            }
            Call::TakeSize(a) => {
                let r = self.take_size(a);
                self.respond(id, r)
            }
            Call::ReleaseSize(a) => {
                let r = calls::pane_id(&a.pane).map(|pane| {
                    self.release_size(pane);
                    Reply::Done
                });
                self.respond(id, r)
            }
            Call::Scrollback(a) => self.scrollback(id, a),
            call => {
                let (ctx, out) = (self.ctx.clone(), self.out.clone());
                tokio::spawn(async move {
                    let result = calls::call(ctx, call).await;
                    if let Err(e) = &result {
                        debug!(id, "call failed: {e}");
                    }
                    if let Some(out) = out {
                        let _ = out.send(LinkFrame::Response(match result {
                            Ok(reply) => Response::ok(id, reply),
                            Err(e) => Response::error(id, e),
                        }));
                    }
                });
            }
        }
    }

    /// A pane whose screen a connector may see: it exists and is not private.
    fn open_pane(&self, pane: &str) -> Result<PaneId, ApiError> {
        let pane = calls::pane_id(pane)?;
        let tab = calls::pane_tab(&self.tree, pane)?;
        if tab.private {
            return Err(forbidden_private());
        }
        Ok(pane)
    }

    /// Follows a pane (attaching its bench) if not yet.
    fn watch(&mut self, pane: PaneId) -> &mut Watched {
        if let std::collections::hash_map::Entry::Vacant(e) = self.watched.entry(pane) {
            e.insert(Watched::default());
            self.sync_attached();
        }
        self.watched.get_mut(&pane).unwrap()
    }

    fn input(&mut self, a: shed_api::calls::Input) -> Result<Reply, ApiError> {
        let pane = self.open_pane(&a.pane)?;
        let keeper = self.ctx.keeper.clone();
        let shown = self.subs.values().any(|s| matches!(s, Sub::Screen(p) if *p == pane));
        let w = self.watch(pane);
        match &w.term {
            Some(term) => keeper.send(Frame::Input { pane, bytes: screen::encode(&a.input, term.modes()) }),
            // Typed into before the pane's modes are known: they come with its replay.
            None => w.input.push((a.input, Instant::now())),
        }
        // A pane typed into but not watched is kept a while, for more typing.
        if !shown {
            w.keep_until = Some(Instant::now() + INPUT_KEEP);
        }
        Ok(Reply::Done)
    }

    fn take_size(&mut self, a: shed_api::calls::TakeSize) -> Result<Reply, ApiError> {
        let pane = self.open_pane(&a.pane)?;
        if !self.subs.values().any(|s| matches!(s, Sub::Screen(p) if *p == pane)) {
            return Err(ApiError::new(ErrorCode::Invalid, "subscribe to the pane's screen first"));
        }
        let (cols, rows) = (a.cols.clamp(2, 1000) as u16, a.rows.clamp(1, 1000) as u16);
        let prior = self.tree.pane(pane).map(|p| (p.cols, p.rows)).unwrap_or((80, 24));
        self.held.entry(pane).or_insert(prior);
        self.ctx.keeper.send_msg(ClientMsg::Resize { pane, cols, rows, cell_width: 0, cell_height: 0 });
        self.state_dirty = true;
        Ok(Reply::Done)
    }

    fn release_size(&mut self, pane: PaneId) {
        if let Some((cols, rows)) = self.held.remove(&pane) {
            self.ctx.keeper.send_msg(ClientMsg::Resize { pane, cols, rows, cell_width: 0, cell_height: 0 });
            self.state_dirty = true;
        }
    }

    fn scrollback(&mut self, id: u32, a: shed_api::calls::Scrollback) {
        let pane = match self.open_pane(&a.pane) {
            Ok(p) => p,
            Err(e) => return self.respond(id, Err(e)),
        };
        let count = a.count.min(5000);
        let w = self.watch(pane);
        match &w.term {
            Some(term) => {
                let lines = term.scrollback(a.from, count);
                self.respond(id, Ok(Reply::Lines(lines)));
            }
            None => {
                w.scrollback.push((id, a.from, count));
                w.keep_until.get_or_insert(Instant::now() + INPUT_KEEP);
            }
        }
    }

    fn subscribe(&mut self, id: u32, topic: Topic) {
        if self.subs.contains_key(&id) {
            self.unsubscribe(id);
        }
        let tree = self.tree.clone();
        let started = match topic {
            Topic::Ai(t) => calls::open_tab(&tree, &t.tab).and_then(|(tab, t)| {
                if t.ai_info().is_none() {
                    return Err(ApiError::new(ErrorCode::Invalid, "not an AI tab"));
                }
                self.ctx.keeper.send_msg(ClientMsg::AiWatch { tab });
                Ok(Sub::Ai { tab, started: false })
            }),
            Topic::Screen(s) => self.open_pane(&s.pane).map(|pane| {
                let w = self.watch(pane);
                w.keep_until = None;
                if let Some(term) = &mut w.term {
                    term.full = true;
                }
                Sub::Screen(pane)
            }),
            Topic::Project(p) => crate::calls::root_of(&tree, &p.tab).and_then(|(root, exclude)| {
                let tab = calls::tab_id(&p.tab)?;
                let out = self.out.clone().ok_or_else(|| ApiError::new(ErrorCode::Offline, "not connected"))?;
                topics::watch_project(self.ctx.clone(), root, exclude, id, out)
                    .map(|watch| Sub::Project(tab, watch))
                    .map_err(|e| ApiError::new(ErrorCode::Failed, format!("cannot follow the project: {e}")))
            }),
            Topic::Search(s) => crate::calls::root_of(&tree, &s.tab).and_then(|(root, exclude)| {
                let tab = calls::tab_id(&s.tab)?;
                let out = self.out.clone().ok_or_else(|| ApiError::new(ErrorCode::Offline, "not connected"))?;
                Ok(Sub::Search(tab, topics::search(self.ctx.clone(), root, exclude, s, id, out)))
            }),
        };
        match started {
            Ok(sub) => {
                self.subs.insert(id, sub);
            }
            Err(e) => self.ended(id, Some(e)),
        }
    }

    fn unsubscribe(&mut self, id: u32) {
        match self.subs.remove(&id) {
            Some(Sub::Screen(pane)) => {
                if !self.subs.values().any(|s| matches!(s, Sub::Screen(p) if *p == pane)) {
                    // The last one watching: the size goes back, the pane is let go.
                    self.release_size(pane);
                    self.watched.remove(&pane);
                    self.sync_attached();
                }
            }
            Some(Sub::Search(_, cancel)) => cancel.store(true, Ordering::Relaxed),
            Some(Sub::Ai { .. } | Sub::Project(..)) | None => {}
        }
    }

    // --------------------------------------------------------------- asks

    fn ask(&mut self, ask: Ask) {
        match ask {
            Ask::Pegboard(tx) => {
                self.pegboard_waiting.push(tx);
                // A fresh one: what a call just changed must show.
                self.pegboard_asked = None;
                self.ask_pegboard();
            }
            Ask::AiEntries(tab, tx) => {
                let conv = self.conversations.entry(tab).or_default();
                if conv.whole {
                    let _ = tx.send(conv.entries.clone());
                } else {
                    conv.waiting.push(tx);
                    self.ctx.keeper.send_msg(ClientMsg::AiWatch { tab });
                }
            }
        }
    }

    // ---------------------------------------------------------------- time

    fn tick(&mut self) -> Option<Exit> {
        let now = Instant::now();
        // Screens.
        let mut events = Vec::new();
        for (pane, w) in &mut self.watched {
            if let Some(term) = &mut w.term
                && (term.dirty || term.full)
                && term.last_sent.is_none_or(|t| now.duration_since(t) >= SCREEN_EVERY)
                && let Some(screen) = term.event()
            {
                events.push((*pane, screen));
            }
            // Input that waited too long for the replay goes as plain keys.
            if w.term.is_none() && w.input.first().is_some_and(|(_, at)| now.duration_since(*at) > INPUT_WAIT) {
                for (items, _) in w.input.drain(..) {
                    self.ctx.keeper.send(Frame::Input { pane: *pane, bytes: screen::encode(&items, Default::default()) });
                }
            }
        }
        for (pane, screen) in events {
            for (id, sub) in &self.subs {
                if matches!(sub, Sub::Screen(p) if *p == pane) {
                    self.send(LinkFrame::Event(shed_api::wire::EventFrame { sub: *id, event: Event::Screen(screen.clone()) }));
                }
            }
        }
        // Panes kept only for typing into.
        let idle: Vec<PaneId> = self
            .watched
            .iter()
            .filter(|(p, w)| w.keep_until.is_some_and(|t| t < now) && !self.subs.values().any(|s| matches!(s, Sub::Screen(x) if x == *p)))
            .map(|(p, _)| *p)
            .collect();
        if !idle.is_empty() {
            for p in idle {
                self.watched.remove(&p);
            }
            self.sync_attached();
        }
        if now >= self.pegboard_due {
            self.pegboard_due = now + PEGBOARD_EVERY;
            self.ask_pegboard();
            // Which benches windows show changes without the tree saying.
            self.ctx.keeper.send_msg(ClientMsg::Status);
        }
        self.publish(false);
        // The settings changed: start again with them (the keeper does).
        if now.duration_since(self.config_checked) > Duration::from_secs(2) {
            self.config_checked = now;
            if let Some(path) = &self.opts.config_file {
                match shed_model::config::Config::load_from(path) {
                    Ok(config) => {
                        if !self.config_reads {
                            info!("shed.toml reads again");
                            self.config_reads = true;
                        }
                        if config.nexus != self.nexus_config {
                            return Some(Exit::Restart);
                        }
                    }
                    // Fail closed: no new tunnels while it does not read.
                    Err(e) => {
                        if self.config_reads {
                            warn!("shed.toml does not read ({e}); desktops' new tunnels are refused until it does");
                            self.config_reads = false;
                        }
                    }
                }
            }
        }
        None
    }

    /// Sends the state if it changed, at most every `STATE_EVERY`.
    fn publish(&mut self, now: bool) {
        if self.out.is_none() || !self.state_dirty || (!now && self.state_at.elapsed() < STATE_EVERY) {
            return;
        }
        self.state_dirty = false;
        let held: HashSet<PaneId> = self.held.keys().copied().collect();
        let state = crate::state::workspace(&crate::state::Inputs {
            tree: &self.tree,
            pegboard: self.pegboard.as_deref(),
            open: &self.open,
            limits: self.limits,
            held: &held,
            host: &self.host,
            home: &self.home,
        });
        if self.sent_state.as_ref() == Some(&state) {
            return;
        }
        self.state_at = Instant::now();
        self.send(LinkFrame::State(state.clone()));
        self.sent_state = Some(state);
    }
}

