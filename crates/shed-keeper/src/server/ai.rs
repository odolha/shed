//! AI tabs: a Claude Code process per tab, its stream of events made into
//! a conversation (`shed_ai::Transcript`) and relayed to windows.
//!
//! Claude Code runs with `--input-format stream-json --output-format
//! stream-json --permission-prompt-tool stdio`: prompts go in as JSON lines,
//! events come out as JSON lines, and asking to use a tool comes out as a
//! `control_request` that the user answers in the window. The process stays
//! up between turns and stops when its tab or bench closes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use shed_ai::{State as AiState, Transcript};
use shed_model::{AiActivity, AiTab, TabId, TabKind};
use shed_proto::{AiImage, ClientKind, Frame, KeeperMsg};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use tracing::{info, warn};

use super::{ClientId, Keeper, State};

/// Updates from a streaming answer are sent this often at most.
const FLUSH_EVERY: Duration = Duration::from_millis(40);

const INIT_REQUEST: &str = "shed-init";

/// A conversation's process and what it said.
pub(super) struct AiRt {
    pub transcript: Transcript,
    /// Lines to Claude Code's stdin, while it runs.
    input: Option<mpsc::UnboundedSender<String>>,
    /// Which process events belong to; older ones are ignored.
    generation: u64,
    commands: Vec<shed_ai::Command>,
    models: Vec<shed_ai::Model>,
    /// Permission requests waiting, as Claude Code sent them.
    pending: HashMap<String, Value>,
    /// Dropping this stops the process.
    _kill: Option<oneshot::Sender<()>>,
    /// The folders and effort the process was started with: a change needs a new one.
    started_with: (Vec<PathBuf>, Option<String>),
    next_request: u64,
    /// The end of what it wrote to stderr, to show if it fails.
    stderr: Arc<std::sync::Mutex<String>>,
}

impl AiRt {
    fn new(transcript: Transcript) -> Self {
        Self {
            transcript,
            input: None,
            generation: 0,
            commands: Vec::new(),
            models: Vec::new(),
            pending: HashMap::new(),
            _kill: None,
            started_with: (Vec::new(), None),
            next_request: 0,
            stderr: Arc::default(),
        }
    }

    fn write(&self, line: Value) -> bool {
        match &self.input {
            Some(tx) => tx.send(line.to_string()).is_ok(),
            None => false,
        }
    }

    fn control(&mut self, request: Value) -> bool {
        self.next_request += 1;
        let id = format!("shed-{}", self.next_request);
        self.write(json!({"type": "control_request", "request_id": id, "request": request}))
    }

    /// Stops the process (if any); events still on their way are ignored.
    fn stop(&mut self) {
        self._kill = None;
        self.input = None;
        self.generation += 1;
        self.pending.clear();
        self.transcript.stopped();
    }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The `claude` program: on the PATH, else where its installer puts it (a
/// keeper started from the desktop may not have `~/.local/bin` on its PATH).
fn claude_program() -> PathBuf {
    // Tests stand a recorded Claude Code in for the real one.
    if let Some(program) = std::env::var_os("SHED_CLAUDE") {
        return program.into();
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let on_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).map(|d| d.join("claude")).find(|c| c.is_file()))
        .unwrap_or(None);
    on_path
        .or_else(|| [home.join(".local/bin/claude"), home.join(".claude/local/claude")].into_iter().find(|c| c.is_file()))
        .unwrap_or_else(|| "claude".into())
}

fn ai_tab(st: &State, tab: TabId) -> Option<AiTab> {
    st.tree.tab(tab).and_then(|t| t.ai_info()).cloned()
}

impl State {
    /// Every window client, which shows any bench's AI tab.
    fn send_windows(&self, msg: KeeperMsg) {
        let frame: Arc<[u8]> = Frame::Keeper(msg).encode().into();
        for c in self.clients.values().filter(|c| c.kind == ClientKind::Window) {
            let _ = c.tx.send(frame.clone());
        }
    }
}

impl Keeper {
    /// The tab's conversation, made (from its saved session, if any) the first time.
    fn ai_rt<'a>(&self, st: &'a mut State, tab: TabId) -> Option<&'a mut AiRt> {
        if !st.ai.contains_key(&tab) {
            let ai = ai_tab(st, tab)?;
            let transcript = ai
                .session
                .as_deref()
                .and_then(|id| shed_ai::load_session(&ai.folder, id))
                .unwrap_or_else(|| Transcript::new(&ai.folder.to_string_lossy()));
            st.ai.insert(tab, AiRt::new(transcript));
        }
        st.ai.get_mut(&tab)
    }

    /// A window opened the tab: it gets the whole conversation and what
    /// Claude Code offers.
    pub(super) fn ai_watch(&self, client: ClientId, tab: TabId) {
        let mut st = self.lock();
        let Some(rt) = self.ai_rt(&mut st, tab) else { return };
        let update = KeeperMsg::AiUpdate { tab, from: 0, entries: rt.transcript.entries.clone(), status: rt.transcript.status.clone() };
        let info = (!rt.commands.is_empty()).then(|| KeeperMsg::AiInfo { tab, commands: rt.commands.clone(), models: rt.models.clone() });
        st.send(client, update);
        if let Some(info) = info {
            st.send(client, info);
        }
        if let Some(limits) = st.ai_limits {
            st.send(client, KeeperMsg::AiLimits(limits));
        }
    }

    pub(super) fn ai_start(self: &Arc<Self>, client: ClientId, tab: TabId) {
        let mut st = self.lock();
        if self.ai_rt(&mut st, tab).is_none() || st.ai[&tab].input.is_some() {
            return;
        }
        if let Err(e) = self.ai_spawn(&mut st, tab) {
            st.send(client, KeeperMsg::Error(format!("cannot start Claude Code: {e}")));
        }
    }

    pub(super) fn ai_send(self: &Arc<Self>, client: ClientId, tab: TabId, text: String, images: Vec<AiImage>) {
        let mut st = self.lock();
        if self.ai_rt(&mut st, tab).is_none() {
            return;
        }
        if st.ai[&tab].input.is_none()
            && let Err(e) = self.ai_spawn(&mut st, tab)
        {
            st.send(client, KeeperMsg::Error(format!("cannot start Claude Code: {e}")));
            return;
        }
        let rt = st.ai.get_mut(&tab).unwrap();
        let content = if images.is_empty() {
            json!(text)
        } else {
            use base64::Engine;
            let mut blocks: Vec<Value> = images
                .iter()
                .map(|i| json!({"type": "image", "source": {"type": "base64", "media_type": i.media_type, "data": base64::engine::general_purpose::STANDARD.encode(&i.data)}}))
                .collect();
            blocks.push(json!({"type": "text", "text": text}));
            Value::Array(blocks)
        };
        rt.transcript.user(&text, images.len() as u32, now());
        rt.write(json!({"type": "user", "message": {"role": "user", "content": content}}));
        drop(st);
        self.ai_dirty.notify_one();
    }

    /// Starts Claude Code for a tab, continuing its session if it has one.
    fn ai_spawn(self: &Arc<Self>, st: &mut State, tab: TabId) -> std::io::Result<()> {
        let ai = ai_tab(st, tab).ok_or_else(|| std::io::Error::other("no such AI tab"))?;
        let mut cmd = tokio::process::Command::new(claude_program());
        cmd.args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--include-partial-messages"]);
        cmd.args(["--permission-prompt-tool", "stdio"]);
        if let Some(s) = &ai.session {
            cmd.args(["--resume", s]);
        }
        if let Some(m) = &ai.model {
            cmd.args(["--model", m]);
        }
        if !ai.mode.is_empty() && ai.mode != "default" {
            cmd.args(["--permission-mode", &ai.mode]);
        }
        if let Some(e) = &ai.effort {
            cmd.args(["--effort", e]);
        }
        for dir in ai.add_dirs.iter().filter(|d| d.is_dir()) {
            cmd.arg("--add-dir").arg(dir);
        }
        cmd.current_dir(&ai.folder).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        let mut child = cmd.spawn()?;
        info!(?tab, folder = %ai.folder.display(), "Claude Code started");
        let mut stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        let rt = st.ai.get_mut(&tab).unwrap();
        rt.generation += 1;
        let generation = rt.generation;
        rt.input = Some(tx);
        rt._kill = Some(kill_tx);
        rt.started_with = (ai.add_dirs.clone(), ai.effort.clone());
        rt.write(json!({"type": "control_request", "request_id": INIT_REQUEST, "request": {"subtype": "initialize"}}));
        let tail = rt.stderr.clone();
        tail.lock().unwrap().clear();

        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err() || stdin.write_all(b"\n").await.is_err() || stdin.flush().await.is_err() {
                    break;
                }
            }
        });
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            while let Ok(n) = stderr.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let mut t = tail.lock().unwrap();
                t.push_str(&String::from_utf8_lossy(&buf[..n]));
                if t.len() > 4000 {
                    let cut = t.len() - 2000;
                    let cut = (cut..t.len()).find(|i| t.is_char_boundary(*i)).unwrap_or(t.len());
                    t.drain(..cut);
                }
            }
        });
        let keeper = self.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            let mut kill_rx = kill_rx;
            loop {
                tokio::select! {
                    line = lines.next_line() => match line {
                        Ok(Some(line)) => keeper.ai_event(tab, generation, &line),
                        _ => break,
                    },
                    // The tab went, or the process is to be replaced.
                    _ = &mut kill_rx => {
                        let _ = child.kill().await;
                        return;
                    }
                }
            }
            let status = child.wait().await.ok();
            keeper.ai_exited(tab, generation, status);
        });
        Ok(())
    }

    /// One line of Claude Code's output.
    fn ai_event(&self, tab: TabId, generation: u64, line: &str) {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        let mut st = self.lock();
        let Some(rt) = st.ai.get_mut(&tab).filter(|rt| rt.generation == generation) else { return };
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
        if kind == "control_response" {
            let response = &v["response"];
            if response.get("request_id").and_then(Value::as_str) == Some(INIT_REQUEST)
                && let Some(reply) = response.get("response")
            {
                let (commands, models) = shed_ai::parse_initialize(reply);
                rt.commands = commands.clone();
                rt.models = models.clone();
                st.send_windows(KeeperMsg::AiInfo { tab, commands, models });
            }
            return;
        }
        if kind == "control_request"
            && let (Some(id), Some(req)) = (v.get("request_id").and_then(Value::as_str), v.get("request"))
        {
            rt.pending.insert(id.to_string(), req.clone());
        }
        if let Some(limits) = rt.transcript.apply(&v) {
            st.ai_limits = Some(limits);
            st.send_windows(KeeperMsg::AiLimits(limits));
        }
        drop(st);
        self.ai_dirty.notify_one();
    }

    fn ai_exited(&self, tab: TabId, generation: u64, status: Option<std::process::ExitStatus>) {
        let mut st = self.lock();
        let Some(rt) = st.ai.get_mut(&tab).filter(|rt| rt.generation == generation) else { return };
        let busy = rt.transcript.status.state != AiState::Idle;
        rt.input = None;
        rt._kill = None;
        rt.pending.clear();
        rt.transcript.stopped();
        if status.is_some_and(|s| !s.success()) || busy {
            let tail = rt.stderr.lock().unwrap().trim().to_string();
            let last = tail.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
            let why = if last.is_empty() { "Claude Code stopped".to_string() } else { format!("Claude Code stopped: {last}") };
            warn!(?tab, "{why}");
            rt.transcript.notice(why, true);
        }
        drop(st);
        self.ai_dirty.notify_one();
    }

    pub(super) fn ai_stop(&self, tab: TabId) {
        let mut st = self.lock();
        if let Some(rt) = st.ai.get_mut(&tab) {
            rt.control(json!({"subtype": "interrupt"}));
        }
    }

    pub(super) fn ai_answer(&self, tab: TabId, request: String, allow: bool, always: bool) {
        // `*` answers every request waiting (for testing without a window).
        if request == "*" {
            let pending: Vec<String> = self.lock().ai.get(&tab).map(|rt| rt.pending.keys().cloned().collect()).unwrap_or_default();
            for r in pending {
                self.ai_answer(tab, r, allow, always);
            }
            return;
        }
        let mut st = self.lock();
        let Some(rt) = st.ai.get_mut(&tab) else { return };
        let Some(req) = rt.pending.remove(&request) else { return };
        let response = if allow {
            let mut r = json!({"behavior": "allow", "updatedInput": req.get("input").cloned().unwrap_or(json!({}))});
            if always && let Some(s) = req.get("permission_suggestions") {
                r["updatedPermissions"] = s.clone();
            }
            r
        } else {
            json!({"behavior": "deny", "message": "The user said no."})
        };
        rt.write(json!({"type": "control_response", "response": {"subtype": "success", "request_id": request, "response": response}}));
        rt.transcript.answer(&request, allow);
        drop(st);
        self.ai_dirty.notify_one();
    }

    /// Shows another session (or a new one) in the tab; the process stops
    /// and starts again for it with the next prompt.
    pub(super) fn ai_open_session(&self, tab: TabId, session: Option<String>) {
        let mut st = self.lock();
        let Some(t) = st.tree.tab_mut(tab) else { return };
        let TabKind::Ai(ai) = &mut t.kind else { return };
        ai.session = session;
        ai.activity = AiActivity::Idle;
        let ai = ai.clone();
        if let Some(mut rt) = st.ai.remove(&tab) {
            rt.stop();
        }
        let transcript = ai
            .session
            .as_deref()
            .and_then(|id| shed_ai::load_session(&ai.folder, id))
            .unwrap_or_else(|| Transcript::new(&ai.folder.to_string_lossy()));
        let mut rt = AiRt::new(transcript);
        rt.transcript.touch_all();
        st.ai.insert(tab, rt);
        st.broadcast_tree();
        drop(st);
        self.save_soon.notify_one();
        self.ai_dirty.notify_one();
    }

    pub(super) fn ai_settings(&self, tab: TabId, new: AiTab) {
        let mut st = self.lock();
        let Some(t) = st.tree.tab_mut(tab) else { return };
        let TabKind::Ai(ai) = &mut t.kind else { return };
        let old = ai.clone();
        ai.add_dirs = new.add_dirs;
        ai.model = new.model;
        ai.mode = if new.mode.is_empty() { "default".into() } else { new.mode };
        ai.effort = new.effort;
        ai.follow = new.follow;
        let ai = ai.clone();
        st.broadcast_tree();
        if let Some(rt) = st.ai.get_mut(&tab)
            && rt.input.is_some()
        {
            if ai.model != old.model {
                rt.control(json!({"subtype": "set_model", "model": ai.model.clone().unwrap_or_else(|| "default".into())}));
            }
            if ai.mode != old.mode {
                rt.control(json!({"subtype": "set_permission_mode", "mode": ai.mode}));
            }
            // Folders and effort are fixed when it starts: the next prompt starts it again.
            if rt.started_with != (ai.add_dirs.clone(), ai.effort.clone()) && rt.transcript.status.state == AiState::Idle {
                rt.stop();
            }
        }
        drop(st);
        self.save_soon.notify_one();
    }

    /// Sends what changed in conversations, a few times a second at most,
    /// and keeps each AI tab's activity and session in the tree.
    pub(super) async fn ai_flush_loop(self: Arc<Self>) {
        loop {
            self.ai_dirty.notified().await;
            tokio::time::sleep(FLUSH_EVERY).await;
            let mut st = self.lock();
            let mut updates = Vec::new();
            let mut tree_changed = false;
            let tabs: Vec<TabId> = st.ai.keys().copied().collect();
            for tab in tabs {
                let rt = st.ai.get_mut(&tab).unwrap();
                if let Some(from) = rt.transcript.take_changes() {
                    let entries = rt.transcript.entries[from.min(rt.transcript.entries.len())..].to_vec();
                    updates.push(KeeperMsg::AiUpdate { tab, from: from as u32, entries, status: rt.transcript.status.clone() });
                }
                let activity = match rt.transcript.status.state {
                    AiState::Idle => AiActivity::Idle,
                    AiState::Working => AiActivity::Working,
                    AiState::Waiting => AiActivity::Waiting,
                };
                let session = rt.transcript.status.session.clone();
                if let Some(t) = st.tree.tab_mut(tab)
                    && let TabKind::Ai(ai) = &mut t.kind
                {
                    if ai.activity != activity {
                        ai.activity = activity;
                        tree_changed = true;
                    }
                    if session.is_some() && ai.session != session {
                        ai.session = session;
                        tree_changed = true;
                        self.save_soon.notify_one();
                    }
                }
            }
            for msg in updates {
                st.send_windows(msg);
            }
            if tree_changed {
                st.broadcast_tree();
            }
        }
    }
}
