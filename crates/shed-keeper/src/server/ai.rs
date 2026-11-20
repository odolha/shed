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
    /// The next start goes back to this message of the session, as a new
    /// session (an earlier prompt was edited).
    resume_at: Option<String>,
    /// An edited prompt waiting for its files to be put back.
    editing: Option<Edit>,
}

/// An edited prompt on its way: the files go back first, then the
/// conversation, then the new text goes out.
struct Edit {
    request: String,
    entry: usize,
    text: String,
    images: Vec<AiImage>,
    /// Sent once back there (an edit); not for a revert.
    send: bool,
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
            resume_at: None,
            editing: None,
        }
    }

    fn write(&self, line: Value) -> bool {
        match &self.input {
            Some(tx) => tx.send(line.to_string()).is_ok(),
            None => false,
        }
    }

    fn control(&mut self, request: Value) -> bool {
        self.control_id(request).is_some()
    }

    /// Sends a control request; its id, if it went.
    fn control_id(&mut self, request: Value) -> Option<String> {
        self.next_request += 1;
        let id = format!("shed-{}", self.next_request);
        self.write(json!({"type": "control_request", "request_id": id, "request": request})).then_some(id)
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

pub(super) fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Claude in Chrome (the browser extension) for the AI tab connected to the
/// browser, or for none: Claude Code's own server for it (`claude
/// --claude-in-chrome-mcp`, what `--chrome` starts), added and taken away
/// while Claude Code runs. Unlike `mcp_toggle`, which Claude Code remembers
/// per folder in `~/.claude.json` (and so for the person's own `claude`
/// there too), a server set this way lasts only as long as the process. Each
/// server has its own connection to the extension, and so its own tab group.
fn browser_servers(on: bool) -> serde_json::Value {
    let servers = if on {
        json!({"claude-in-chrome": {"type": "stdio", "command": claude_program(), "args": ["--claude-in-chrome-mcp"]}})
    } else {
        json!({})
    };
    json!({"subtype": "mcp_set_servers", "servers": servers})
}

/// The `claude` program: on the PATH, else where its installer puts it (a
/// keeper started from the desktop may not have `~/.local/bin` on its PATH).
fn claude_program() -> PathBuf {
    shed_ai::aside::claude_program()
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
    /// Where the pictures sent with prompts are kept, to show them again.
    fn picture_dir(&self) -> PathBuf {
        self.cfg.state_dir.join("ai-pictures")
    }

    /// The tab's conversation, made (from its saved session, if any) the first time.
    fn ai_rt<'a>(&self, st: &'a mut State, tab: TabId) -> Option<&'a mut AiRt> {
        if !st.ai.contains_key(&tab) {
            let ai = ai_tab(st, tab)?;
            let transcript = ai
                .session
                .as_deref()
                .and_then(|id| shed_ai::load_session(&ai.folder, id, Some(self.picture_dir())))
                .unwrap_or_else(|| {
                    let mut t = Transcript::new(&ai.folder.to_string_lossy());
                    t.picture_dir = Some(self.picture_dir());
                    t
                });
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
        // Claude is busy: the message waits its turn, kept with the tab (and
        // its pictures as files), and goes when a turn ends.
        let busy = self.ai_rt(&mut st, tab).is_some_and(|rt| rt.transcript.status.state != AiState::Idle);
        if busy {
            let dir = self.picture_dir();
            let pictures = images.iter().filter_map(|i| shed_ai::keep_picture_in(&dir, &i.media_type, &i.data)).collect();
            self.change_queue(&mut st, tab, |q| q.push(shed_model::Queued { text, pictures }));
            return;
        }
        self.ai_send_locked(&mut st, client, tab, text, images);
        drop(st);
        self.ai_dirty.notify_one();
    }

    /// Changes a tab's queue, in the tree (windows show it) and saved.
    fn change_queue(&self, st: &mut State, tab: TabId, change: impl FnOnce(&mut Vec<shed_model::Queued>)) {
        if let Some(t) = st.tree.tab_mut(tab)
            && let TabKind::Ai(ai) = &mut t.kind
        {
            change(&mut ai.queued);
            st.broadcast_tree();
            self.save_soon.notify_one();
        }
    }

    /// Sends the first queued message, if there is one.
    fn send_next_queued(self: &Arc<Self>, st: &mut State, tab: TabId) {
        let Some(next) = ai_tab(st, tab).and_then(|a| a.queued.first().cloned()) else { return };
        self.change_queue(st, tab, |q| {
            q.remove(0);
        });
        self.send_queued(st, tab, next);
    }

    /// Sends a queued message, its pictures read back from their files.
    fn send_queued(self: &Arc<Self>, st: &mut State, tab: TabId, queued: shed_model::Queued) {
        let images = queued
            .pictures
            .iter()
            .filter_map(|p| {
                let data = std::fs::read(p).ok()?;
                let ext = std::path::Path::new(p).extension().and_then(|e| e.to_str()).unwrap_or("png").to_lowercase();
                let media_type = match ext.as_str() {
                    "jpg" | "jpeg" => "image/jpeg",
                    "gif" => "image/gif",
                    "webp" => "image/webp",
                    _ => "image/png",
                };
                Some(AiImage { media_type: media_type.into(), data })
            })
            .collect();
        // No window asked: errors starting it go to the notice.
        self.ai_send_locked(st, u64::MAX, tab, queued.text, images);
    }

    /// A queued message sent now, or taken out of the queue.
    pub(super) fn ai_queued(self: &Arc<Self>, tab: TabId, index: usize, text: String, send: bool) {
        let mut st = self.lock();
        let Some(item) = ai_tab(&st, tab).and_then(|a| a.queued.get(index).cloned()).filter(|q| q.text == text) else { return };
        let busy = st.ai.get(&tab).is_some_and(|rt| rt.transcript.status.state != AiState::Idle);
        self.change_queue(&mut st, tab, |q| {
            let item = q.remove(index);
            // Asked to go now while Claude is busy: it goes next.
            if send && busy {
                q.insert(0, item);
            }
        });
        if send && !busy {
            self.send_queued(&mut st, tab, item);
        }
        drop(st);
        self.ai_dirty.notify_one();
    }

    fn ai_send_locked(self: &Arc<Self>, st: &mut State, client: ClientId, tab: TabId, text: String, images: Vec<AiImage>) {
        if self.ai_rt(st, tab).is_none() {
            return;
        }
        if st.ai[&tab].input.is_none()
            && let Err(e) = self.ai_spawn(st, tab)
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
        // Our own id for the message, kept in Claude Code's record: going
        // back to it and putting files back both need it.
        let uuid = uuid::Uuid::new_v4().to_string();
        let pictures: Vec<(String, Vec<u8>)> = images.iter().map(|i| (i.media_type.clone(), i.data.clone())).collect();
        rt.transcript.user(&text, &pictures, now(), Some(uuid.clone()));
        rt.write(json!({"type": "user", "uuid": uuid, "message": {"role": "user", "content": content}}));
    }

    /// Sends an earlier prompt again, changed: the files Claude changed
    /// since it go back (if Claude Code kept them), then the conversation
    /// goes back to before it (as a new session, the old one is kept), and
    /// the new text goes out.
    pub(super) fn ai_edit(self: &Arc<Self>, client: ClientId, tab: TabId, entry: usize, text: String, images: Vec<AiImage>) {
        self.ai_go_back(client, tab, Edit { request: String::new(), entry, text, images, send: true }, true);
    }

    /// Goes back to before a message, sending nothing; with `files`, the
    /// files Claude changed since go back too.
    pub(super) fn ai_revert(self: &Arc<Self>, client: ClientId, tab: TabId, entry: usize, files: bool) {
        self.ai_go_back(client, tab, Edit { request: String::new(), entry, text: String::new(), images: Vec::new(), send: false }, files);
    }

    fn ai_go_back(self: &Arc<Self>, client: ClientId, tab: TabId, edit: Edit, files: bool) {
        let entry = edit.entry;
        let mut st = self.lock();
        let Some(rt) = self.ai_rt(&mut st, tab) else { return };
        if rt.transcript.status.state != AiState::Idle || rt.editing.is_some() {
            st.send(client, KeeperMsg::Error("Claude is still working: stop it first".into()));
            return;
        }
        let Some(mark) = rt.transcript.prompt_mark(entry).cloned() else {
            st.send(client, KeeperMsg::Error("cannot go back to this message: Claude Code's record of it is not known".into()));
            return;
        };
        // The conversation only: no files to ask Claude Code about.
        if !files {
            drop(st);
            self.ai_edit_go_back(tab, edit, None);
            return;
        }
        if rt.input.is_none()
            && let Err(e) = self.ai_spawn(&mut st, tab)
        {
            st.send(client, KeeperMsg::Error(format!("cannot start Claude Code: {e}")));
            return;
        }
        let rt = st.ai.get_mut(&tab).unwrap();
        let uuid = mark.uuid.clone().unwrap_or_default();
        match rt.control_id(json!({"subtype": "rewind_files", "user_message_id": uuid})) {
            Some(request) => rt.editing = Some(Edit { request, ..edit }),
            None => {
                drop(st);
                self.ai_edit_go_back(tab, edit, Some("Claude Code is not running".into()));
            }
        }
    }

    /// The files are back (or could not be): the conversation goes back
    /// and the new text goes out.
    fn ai_edit_go_back(self: &Arc<Self>, tab: TabId, edit: Edit, files_problem: Option<String>) {
        let mut st = self.lock();
        let Some(rt) = st.ai.get_mut(&tab) else { return };
        let Some(mark) = rt.transcript.prompt_mark(edit.entry).cloned() else { return };
        rt.stop();
        rt.transcript.truncate(edit.entry);
        // An edit (or a revert with files) asked for the files; a revert of
        // the conversation only did not.
        let files_asked = edit.send || !edit.request.is_empty() || files_problem.is_some();
        match files_problem {
            None if files_asked => rt.transcript.notice("Went back to before this message; the files Claude changed since are as they were", false),
            None => rt.transcript.notice("Went back to before this message; the files are as they are now", false),
            Some(why) => rt.transcript.notice(format!("Went back to before this message; files not put back: {why}"), true),
        }
        // The session goes on from the message before (a new session that
        // starts as a copy); before the first message, a new one starts.
        rt.resume_at = mark.before.clone();
        // A conversation read from several files: the message to go on from
        // is in an earlier one, which is resumed instead.
        let switch_to = mark.before.as_ref().and(mark.session.clone()).filter(|p| rt.transcript.status.session.as_ref() != Some(p));
        if let Some(part) = &switch_to {
            rt.transcript.status.session = Some(part.clone());
        }
        if mark.before.is_none() {
            rt.transcript.status.session = None;
            if let Some(t) = st.tree.tab_mut(tab)
                && let TabKind::Ai(ai) = &mut t.kind
            {
                // Back to before the first message: its drafts stay with it,
                // as a draft session.
                if let Some(old) = ai.session.take()
                    && ai.drafts.iter().any(|d| d.session == old)
                {
                    let key = AiTab::new_draft_key();
                    ai.move_drafts(&old, &key);
                    ai.draft_session = Some(key);
                }
            }
            st.broadcast_tree();
        }
        if let Some(part) = switch_to {
            if let Some(t) = st.tree.tab_mut(tab)
                && let TabKind::Ai(ai) = &mut t.kind
                && let Some(old) = ai.session.replace(part.clone())
            {
                ai.move_drafts(&old, &part);
            }
            st.broadcast_tree();
        }
        // No window asked: errors starting it again go to the notice. A
        // revert sends nothing: the next prompt goes on from here.
        if edit.send {
            let client: ClientId = u64::MAX;
            self.ai_send_locked(&mut st, client, tab, edit.text, edit.images);
        }
        drop(st);
        self.save_soon.notify_one();
        self.ai_dirty.notify_one();
    }

    /// Starts Claude Code for a tab, continuing its session if it has one.
    fn ai_spawn(self: &Arc<Self>, st: &mut State, tab: TabId) -> std::io::Result<()> {
        let ai = ai_tab(st, tab).ok_or_else(|| std::io::Error::other("no such AI tab"))?;
        let mut cmd = tokio::process::Command::new(claude_program());
        cmd.args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--include-partial-messages"]);
        cmd.args(["--permission-prompt-tool", "stdio"]);
        // Lets the tab switch to Full Auto (bypassPermissions) later; it
        // does not turn it on.
        cmd.arg("--allow-dangerously-skip-permissions");
        let resume_at = st.ai.get_mut(&tab).and_then(|rt| rt.resume_at.take());
        if let Some(s) = &ai.session {
            cmd.args(["--resume", s]);
            if let Some(at) = &resume_at {
                cmd.args(["--resume-session-at", at, "--fork-session"]);
            }
        }
        // Claude Code keeps what files were before each prompt, so an
        // edited prompt can put them back.
        cmd.env("CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING", "true");
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
        // Claude in Chrome only as Shed adds it (`browser_servers`), for the
        // one tab connected to the browser, even where it is on by default.
        cmd.arg("--no-chrome");
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
        if ai.browser {
            rt.control(browser_servers(true));
        }
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
    fn ai_event(self: &Arc<Self>, tab: TabId, generation: u64, line: &str) {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        let mut st = self.lock();
        let Some(rt) = st.ai.get_mut(&tab).filter(|rt| rt.generation == generation) else { return };
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
        if kind == "control_response" {
            let response = &v["response"];
            let id = response.get("request_id").and_then(Value::as_str);
            if let Some(edit) = rt.editing.take_if(|e| Some(e.request.as_str()) == id) {
                let reply = response.get("response");
                let problem = if response.get("subtype").and_then(Value::as_str) == Some("error") {
                    Some(response.get("error").and_then(Value::as_str).unwrap_or("Claude Code refused").to_string())
                } else if reply.and_then(|r| r.get("canRewind")).and_then(Value::as_bool) == Some(false) {
                    Some(reply.and_then(|r| r.get("error")).and_then(Value::as_str).unwrap_or("Claude Code kept no copy").to_string())
                } else {
                    None
                };
                drop(st);
                let keeper = self.clone();
                // Not under this lock: it stops this very process.
                tokio::spawn(async move { keeper.ai_edit_go_back(tab, edit, problem) });
                return;
            }
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
        // Live events carry no time: they happened now.
        rt.transcript.clock = now();
        let was_busy = rt.transcript.status.state != AiState::Idle;
        let limits = rt.transcript.apply(&v);
        let turn_ended = was_busy && rt.transcript.status.state == AiState::Idle && kind == "result";
        if turn_ended {
            self.send_next_queued(&mut st, tab);
        }
        if let Some(mut limits) = limits {
            limits.seen = now();
            st.ai_limits = Some(limits);
            st.send_windows(KeeperMsg::AiLimits(limits));
            // Kept for after a restart, until a Claude Code reports again.
            let file = self.cfg.state_dir.join("ai-limits.json");
            if let Ok(text) = serde_json::to_string(&limits) {
                let _ = shed_model::paths::write_whole(&file, text);
            }
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

    /// Answers Claude's questions: the tool goes ahead with the answers in
    /// its input, where Claude Code reads them.
    pub(super) fn ai_answer_questions(&self, tab: TabId, request: String, answers: Vec<String>) {
        let mut st = self.lock();
        let Some(rt) = st.ai.get_mut(&tab) else { return };
        let Some(req) = rt.pending.remove(&request) else { return };
        let input = req.get("input").cloned().unwrap_or(json!({}));
        let questions = shed_ai::parse_questions(&input);
        let updated = shed_ai::answered_input(&input, &questions, &answers);
        let response = json!({"behavior": "allow", "updatedInput": updated});
        rt.write(json!({"type": "control_response", "response": {"subtype": "success", "request_id": request, "response": response}}));
        rt.transcript.answer_questions(&request, answers);
        drop(st);
        self.ai_dirty.notify_one();
    }

    /// Shows another session (or a new one) in the tab; the process stops
    /// and starts again for it with the next prompt.
    pub(super) fn ai_open_session(&self, tab: TabId, session: Option<String>) {
        let mut st = self.lock();
        let Some(t) = st.tree.tab_mut(tab) else { return };
        let TabKind::Ai(ai) = &mut t.kind else { return };
        // A draft session has no Claude Code session yet: a new one starts
        // with its first message.
        (ai.session, ai.draft_session) = match session {
            Some(key) if AiTab::is_draft_key(&key) => (None, Some(key)),
            other => (other, None),
        };
        ai.activity = AiActivity::Idle;
        let ai = ai.clone();
        if let Some(mut rt) = st.ai.remove(&tab) {
            rt.stop();
        }
        let transcript = ai
            .session
            .as_deref()
            .and_then(|id| shed_ai::load_session(&ai.folder, id, Some(self.picture_dir())))
            .unwrap_or_else(|| {
                    let mut t = Transcript::new(&ai.folder.to_string_lossy());
                    t.picture_dir = Some(self.picture_dir());
                    t
                });
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
        ai.drafts = new.drafts;
        // (A draft session only while no Claude Code session is shown.)
        if ai.session.is_none() {
            ai.draft_session = new.draft_session;
        }
        ai.unsent = new.unsent;
        ai.viewed = new.viewed;
        ai.explained = new.explained;
        ai.browser = new.browser;
        let ai = ai.clone();
        // One AI tab at a time uses the browser: connecting this one lets go
        // of any other, in any bench.
        let mut let_go = Vec::new();
        if ai.browser && !old.browser {
            for other in st.tree.benches.iter_mut().flat_map(|b| &mut b.tabs).filter(|t| t.id != tab) {
                if let TabKind::Ai(o) = &mut other.kind
                    && o.browser
                {
                    o.browser = false;
                    let_go.push(other.id);
                }
            }
        }
        for other in let_go {
            if let Some(rt) = st.ai.get_mut(&other)
                && rt.input.is_some()
            {
                rt.control(browser_servers(false));
            }
        }
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
            // Mid-turn too: Claude Code takes the browser's tools on or off.
            if ai.browser != old.browser {
                rt.control(browser_servers(ai.browser));
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
                let (one, from) = rt.transcript.take_update();
                // Entries changed in place (a background agent's progress) go
                // alone, not with everything after them.
                if !one.is_empty() {
                    let entries = one.iter().map(|&i| (i as u32, rt.transcript.entries[i].clone())).collect();
                    updates.push(KeeperMsg::AiEntries { tab, entries, status: rt.transcript.status.clone() });
                }
                if let Some(from) = from {
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
                    if let Some(new) = session.as_ref().filter(|s| ai.session.as_ref() != Some(*s)) {
                        // The drafts go with the conversation: from the draft
                        // session it started from, or to the copy an edit makes.
                        if let Some(old) = ai.session.clone().or(ai.draft_session.take()) {
                            ai.move_drafts(&old, new);
                        }
                        ai.draft_session = None;
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
