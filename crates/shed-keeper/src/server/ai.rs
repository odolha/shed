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
use shed_model::{AiActivity, AiAway, AiTab, TabId, TabKind};
use shed_proto::{AiChange, AiImage, Frame, KeeperMsg};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use tracing::{info, warn};

use super::{ClientId, Keeper, State};

/// Updates from a streaming answer are sent this often at most.
const FLUSH_EVERY: Duration = Duration::from_millis(40);

const INIT_REQUEST: &str = "shed-init";
/// How many changes marked viewed, and explanations, an AI tab keeps.
const VIEWED_KEPT: usize = 2000;
const EXPLAINED_KEPT: usize = 500;

/// A number no process of any tab had before.
fn next_generation() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// A conversation's process and what it said.
pub(super) struct AiRt {
    pub transcript: Transcript,
    /// Lines to Claude Code's stdin, while it runs.
    input: Option<mpsc::UnboundedSender<String>>,
    /// Which process events belong to; older ones are ignored. Numbered
    /// across the keeper (`next_generation`), not per tab: a tab's runtime is
    /// made anew when it opens another session, and a process stopped then
    /// must not pass for the new one.
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

    /// Its process runs and has something going on: it is kept running
    /// when the tab shows another session.
    pub(super) fn has_work(&self) -> bool {
        self.input.is_some() && self.transcript.has_work()
    }

    /// Stops the process (if any); events still on their way are ignored.
    fn stop(&mut self) {
        self._kill = None;
        self.input = None;
        self.generation = next_generation();
        self.pending.clear();
        self.transcript.stopped();
    }
}

/// Steps of a running agent sent with each change of it: its latest only
/// (all of them go once it ends, and to a window opening the tab).
const LIVE_STEPS: usize = 12;

/// An entry changed in place, as it goes out: a running agent with its
/// latest steps, so its progress costs little each time.
fn glimpse(entry: &mut shed_ai::Entry) -> shed_ai::Entry {
    match entry {
        shed_ai::Entry::Tool(t) if t.run.as_ref().is_some_and(|r| r.running() && r.steps.len() > LIVE_STEPS) => {
            // Its steps set aside a moment, not to copy them all.
            let mut run = t.run.take().unwrap();
            let steps = std::mem::take(&mut run.steps);
            let over = steps.len() - LIVE_STEPS;
            let mut light = shed_ai::Tool { run: Some(run.clone()), ..t.clone() };
            if let Some(r) = light.run.as_mut() {
                r.steps = steps[over..].to_vec();
                r.skipped += over as u32;
            }
            run.steps = steps;
            t.run = Some(run);
            shed_ai::Entry::Tool(light)
        }
        other => other.clone(),
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

fn activity_of(state: AiState) -> AiActivity {
    match state {
        AiState::Idle => AiActivity::Idle,
        AiState::Working => AiActivity::Working,
        AiState::Waiting => AiActivity::Waiting,
    }
}

/// Puts the sessions a tab's Claude Code goes on with in the background in
/// the tree (the windows list them as working); whether that changed it.
fn ai_away_to_tree(st: &mut State, tab: TabId) -> bool {
    let away: Vec<AiAway> = st
        .ai_away
        .get(&tab)
        .into_iter()
        .flatten()
        .filter_map(|rt| {
            // Busy in the background between turns (an agent, a wake-up) counts as working.
            let activity = match activity_of(rt.transcript.status.state) {
                AiActivity::Idle => AiActivity::Working,
                a => a,
            };
            Some(AiAway { session: rt.transcript.status.session.clone()?, activity })
        })
        .collect();
    match st.tree.tab_mut(tab).map(|t| &mut t.kind) {
        Some(TabKind::Ai(ai)) if ai.away != away => {
            ai.away = away;
            true
        }
        _ => false,
    }
}

impl State {
    /// The conversation process `generation` belongs to: the tab's shown
    /// one, or one it moved away from (`true` when shown).
    fn ai_of_process(&mut self, tab: TabId, generation: u64) -> Option<(&mut AiRt, bool)> {
        if self.ai.get(&tab).is_some_and(|rt| rt.generation == generation) {
            return self.ai.get_mut(&tab).map(|rt| (rt, true));
        }
        self.ai_away.get_mut(&tab)?.iter_mut().find(|rt| rt.generation == generation).map(|rt| (rt, false))
    }

    /// Every window (and link) client, which shows any bench's AI tab.
    fn send_windows(&self, msg: KeeperMsg) {
        let frame: Arc<[u8]> = Frame::Keeper(msg).encode().into();
        for c in self.clients.values().filter(|c| c.kind.follows_tree()) {
            let _ = c.tx.send(frame.clone());
        }
    }
}

impl Keeper {
    /// Where the pictures sent with prompts are kept, to show them again.
    fn picture_dir(&self) -> PathBuf {
        self.cfg.state_dir.join("ai-pictures")
    }

    /// A tab's conversation as its saved session has it (a new one if none).
    /// A fork not started yet reads the session it copies, as no session of
    /// its own.
    fn read_transcript(ai: &AiTab, picture_dir: PathBuf) -> Transcript {
        let fork = ai.session.is_none().then_some(ai.fork_of.as_deref()).flatten();
        let mut t = ai.session.as_deref().or(fork).and_then(|id| shed_ai::load_session(&ai.folder, id, Some(picture_dir.clone()))).unwrap_or_else(|| {
            let mut t = Transcript::new(&ai.folder.to_string_lossy());
            t.picture_dir = Some(picture_dir);
            t
        });
        if fork.is_some() {
            t.status.session = None;
            t.notice("A fork of this session: what you send next goes to a copy of it, as a new session (this one stays as it is)", false);
        }
        t
    }

    /// The tab's conversation, made (from its saved session, if any) the first time.
    fn ai_rt<'a>(&self, st: &'a mut State, tab: TabId) -> Option<&'a mut AiRt> {
        if !st.ai.contains_key(&tab) {
            let ai = ai_tab(st, tab)?;
            st.ai.insert(tab, AiRt::new(Self::read_transcript(&ai, self.picture_dir())));
        }
        st.ai.get_mut(&tab)
    }

    /// Reads a tab's session off the keeper's lock (with its sub-agents' files
    /// it can take a while), then `then` with the lock, if the tab still
    /// shows that session: with what was read, or none when something read
    /// it meanwhile (that stays).
    fn ai_load(self: &Arc<Self>, tab: TabId, ai: AiTab, then: impl FnOnce(&Keeper, &mut State, Option<Transcript>) + Send + 'static) {
        let keeper = self.clone();
        let dir = self.picture_dir();
        tokio::task::spawn_blocking(move || {
            let transcript = Self::read_transcript(&ai, dir);
            let mut st = keeper.lock();
            if ai_tab(&st, tab).is_none_or(|now| (&now.session, &now.fork_of) != (&ai.session, &ai.fork_of)) {
                return;
            }
            // (Read meanwhile, for another window or a prompt: kept as it is.)
            let fresh = (!st.ai.contains_key(&tab)).then_some(transcript);
            then(&keeper, &mut st, fresh);
        });
    }

    /// A window opened the tab: it gets the whole conversation and what
    /// Claude Code offers (once read, when it is not yet).
    pub(super) fn ai_watch(self: &Arc<Self>, client: ClientId, tab: TabId) {
        let mut st = self.lock();
        if !st.ai.contains_key(&tab) {
            let Some(ai) = ai_tab(&st, tab) else { return };
            drop(st);
            return self.ai_load(tab, ai, move |keeper, st, transcript| {
                if let Some(transcript) = transcript {
                    st.ai.insert(tab, AiRt::new(transcript));
                }
                keeper.ai_send_watched(st, client, tab);
            });
        }
        self.ai_send_watched(&mut st, client, tab);
    }

    fn ai_send_watched(&self, st: &mut State, client: ClientId, tab: TabId) {
        let Some(rt) = st.ai.get(&tab) else { return };
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
        // What the prompt kept is this message (or the start of it, saved
        // before the last keys): sent now, it is not unsent any more, and must
        // not come back into the prompt after a restart, whichever client
        // sent it.
        if let Some(t) = st.tree.tab_mut(tab)
            && let TabKind::Ai(ai) = &mut t.kind
            && !ai.unsent.trim().is_empty()
            && text.trim_start().starts_with(ai.unsent.trim())
        {
            ai.unsent.clear();
            st.broadcast_tree();
            self.save_soon.notify_one();
        }
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
                // Back to before the first message: a new session, its
                // drafts kept with it, as a draft session.
                ai.fork_of = None;
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
            if let Some(TabKind::Ai(ai)) = st.tree.tab_mut(tab).map(|t| &mut t.kind) {
                ai.fork_of = None;
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
        // A fork not started yet goes on from a copy of the session it forks.
        let fork = ai.session.is_none().then_some(ai.fork_of.as_ref()).flatten();
        if let Some(s) = ai.session.as_ref().or(fork) {
            cmd.args(["--resume", s]);
            if let Some(at) = &resume_at {
                cmd.args(["--resume-session-at", at]);
            }
            if resume_at.is_some() || fork.is_some() {
                cmd.arg("--fork-session");
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
        rt.generation = next_generation();
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
        let Some((rt, shown)) = st.ai_of_process(tab, generation) else { return };
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
                if shown {
                    st.send_windows(KeeperMsg::AiInfo { tab, commands, models });
                }
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
        // (The queue is the shown session's.)
        if turn_ended && shown {
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
        let Some((rt, shown)) = st.ai_of_process(tab, generation) else { return };
        if !shown {
            // Gone in the background: its session file has what it did.
            warn!(?tab, session = ?rt.transcript.status.session, "Claude Code stopped in the background");
            if let Some(away) = st.ai_away.get_mut(&tab) {
                away.retain(|rt| rt.generation != generation);
            }
            drop(st);
            return self.ai_dirty.notify_one();
        }
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

    /// Shows another session (or a new one) in the tab. The one shown
    /// before goes on in the background while Claude is at it (its process
    /// kept, shown again when it is opened again); otherwise its process
    /// stops, and one starts for the new session with the next prompt.
    /// With `fork`, a fork of `session` is shown instead (`AiTab::fork_of`).
    pub(super) fn ai_open_session(self: &Arc<Self>, tab: TabId, session: Option<String>, fork: bool) {
        let mut st = self.lock();
        let Some(TabKind::Ai(ai)) = st.tree.tab(tab).map(|t| &t.kind) else { return };
        // The same session again, Claude at it: it stays as it is (else it
        // is read again, as the files have it now).
        if !fork && session.is_some() && session == ai.session && st.ai.get(&tab).is_some_and(AiRt::has_work) {
            return;
        }
        let Some(t) = st.tree.tab_mut(tab) else { return };
        let TabKind::Ai(ai) = &mut t.kind else { return };
        // A draft session has no Claude Code session yet: a new one starts
        // with its first message. So does a fork, from a copy of its session.
        (ai.session, ai.draft_session, ai.fork_of) = match session {
            Some(key) if fork && !AiTab::is_draft_key(&key) => (None, None, Some(key)),
            Some(key) if AiTab::is_draft_key(&key) => (None, Some(key), None),
            other => (other, None, None),
        };
        ai.activity = AiActivity::Idle;
        let ai = ai.clone();
        if let Some(mut rt) = st.ai.remove(&tab) {
            if rt.has_work() {
                info!(?tab, session = ?rt.transcript.status.session, "Claude Code goes on in the background");
                st.ai_away.entry(tab).or_default().push(rt);
            } else {
                rt.stop();
            }
        }
        // Going on in the background: shown again as it is, process and all.
        let back = ai.session.as_ref().and_then(|id| {
            let away = st.ai_away.get_mut(&tab)?;
            let i = away.iter().position(|rt| rt.transcript.status.session.as_ref() == Some(id))?;
            Some(away.remove(i))
        });
        if let Some(mut rt) = back {
            rt.transcript.touch_all();
            let info = (!rt.commands.is_empty()).then(|| KeeperMsg::AiInfo { tab, commands: rt.commands.clone(), models: rt.models.clone() });
            let activity = activity_of(rt.transcript.status.state);
            st.ai.insert(tab, rt);
            if let Some(TabKind::Ai(ai)) = st.tree.tab_mut(tab).map(|t| &mut t.kind) {
                ai.activity = activity;
            }
            if let Some(info) = info {
                st.send_windows(info);
            }
            ai_away_to_tree(&mut st, tab);
            st.broadcast_tree();
            drop(st);
            self.save_soon.notify_one();
            self.ai_dirty.notify_one();
            return;
        }
        ai_away_to_tree(&mut st, tab);
        st.broadcast_tree();
        drop(st);
        self.save_soon.notify_one();
        // Read off the lock; windows get it whole once it is.
        self.ai_load(tab, *ai, move |keeper, st, transcript| {
            let Some(transcript) = transcript else { return };
            let mut rt = AiRt::new(transcript);
            rt.transcript.touch_all();
            st.ai.insert(tab, rt);
            keeper.ai_dirty.notify_one();
        });
    }

    /// Deletes a session of the tab's folder and those going on from it
    /// (they read their beginning from its file): their files move to the
    /// trash (`ai-trash`, pruned after a while), their drafts go. Refused
    /// while another AI tab shows one of them; this tab, showing one, goes
    /// to a new session first, its Claude Code stopped.
    pub(super) fn ai_delete_session(self: &Arc<Self>, client: ClientId, tab: TabId, session: String) {
        let ai = {
            let st = self.lock();
            match ai_tab(&st, tab) {
                Some(ai) if shed_ai::sessions::is_session_id(&session) && !AiTab::is_draft_key(&session) => ai,
                Some(_) => return st.send(client, KeeperMsg::Error(format!("not a Claude Code session: {session:?}"))),
                None => return st.send(client, KeeperMsg::Error("no such AI tab".into())),
            }
        };
        let keeper = self.clone();
        let trash = self.cfg.state_dir.join("ai-trash");
        tokio::spawn(async move {
            let (folder, first) = (ai.folder.clone(), session.clone());
            let ids = tokio::task::spawn_blocking(move || {
                if !shed_ai::sessions::sessions_dir(&folder).join(format!("{first}.jsonl")).is_file() {
                    return None;
                }
                let mut ids = vec![first.clone()];
                ids.extend(shed_ai::going_on_from(&shed_ai::list_sessions(&folder), &first));
                Some(ids)
            })
            .await
            .ok()
            .flatten();
            let Some(ids) = ids else {
                return keeper.lock().send(client, KeeperMsg::Error(format!("no session {session:?} in {}", ai.folder.display())));
            };
            let dir = shed_ai::sessions::sessions_dir(&ai.folder);
            let (shown_here, stopped_away) = {
                let mut st = keeper.lock();
                let shows = |a: &AiTab| {
                    (a.session.as_ref().or(a.fork_of.as_ref()).is_some_and(|s| ids.contains(s)) || a.away.iter().any(|w| ids.contains(&w.session)))
                        && shed_ai::sessions::sessions_dir(&a.folder) == dir
                };
                let elsewhere = st.tree.benches.iter().flat_map(|b| &b.tabs).filter(|t| t.id != tab).filter_map(|t| t.ai_info()).any(shows);
                if elsewhere {
                    return st.send(client, KeeperMsg::Error("That session is open in another tab; close it there first.".into()));
                }
                let Some(TabKind::Ai(here)) = st.tree.tab_mut(tab).map(|t| &mut t.kind) else {
                    return st.send(client, KeeperMsg::Error("no such AI tab".into()));
                };
                let shown = here.session.as_ref().or(here.fork_of.as_ref()).is_some_and(|s| ids.contains(s));
                // Their drafts have nothing to go with any more.
                here.drafts.retain(|d| !ids.contains(&d.session));
                // Going on in the background: they stop.
                let mut stopped = false;
                if let Some(away) = st.ai_away.get_mut(&tab) {
                    let before = away.len();
                    away.retain(|rt| rt.transcript.status.session.as_ref().is_none_or(|s| !ids.contains(s)));
                    stopped = away.len() != before;
                }
                ai_away_to_tree(&mut st, tab);
                if !shown {
                    st.broadcast_tree();
                }
                (shown, stopped)
            };
            if shown_here {
                keeper.ai_open_session(tab, None, false);
            }
            if shown_here || stopped_away {
                // Its Claude Code, stopped, writes no more to the files.
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            keeper.save_soon.notify_one();
            let (folder, gone) = (ai.folder.clone(), ids.clone());
            let moved = tokio::task::spawn_blocking(move || shed_ai::trash_sessions(&folder, &gone, &trash)).await;
            let st = keeper.lock();
            match moved {
                Ok(Ok(to)) => {
                    info!(?tab, session, more = ids.len() - 1, to = %to.display(), "Claude Code session moved to the trash");
                    let more = match ids.len() - 1 {
                        0 => String::new(),
                        1 => " and 1 later session".into(),
                        n => format!(" and {n} later sessions"),
                    };
                    st.send(client, KeeperMsg::Done(format!("deleted the session{more}")));
                }
                Ok(Err(e)) => st.send(client, KeeperMsg::Error(format!("cannot delete the session: {e}"))),
                Err(e) => st.send(client, KeeperMsg::Error(format!("cannot delete the session: {e}"))),
            }
        });
    }

    pub(super) fn ai_settings(&self, tab: TabId, new: AiTab) {
        self.ai_edit_tab(tab, |ai| {
            ai.add_dirs = new.add_dirs;
            ai.model = new.model;
            ai.mode = new.mode;
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
        });
    }

    /// Changes one thing of an AI tab's (`AiChange`), leaving the rest.
    pub(super) fn ai_change(&self, tab: TabId, change: AiChange) {
        self.ai_edit_tab(tab, |ai| match change {
            AiChange::Settings { model, mode, effort, follow, browser, add_dirs } => {
                if let Some(model) = model {
                    ai.model = model;
                }
                if let Some(mode) = mode {
                    ai.mode = mode;
                }
                if let Some(effort) = effort {
                    ai.effort = effort;
                }
                if let Some(follow) = follow {
                    ai.follow = follow;
                }
                if let Some(browser) = browser {
                    ai.browser = browser;
                }
                if let Some(dirs) = add_dirs {
                    ai.add_dirs = dirs;
                }
            }
            AiChange::Unsent(text) => ai.unsent = text,
            AiChange::AddDraft { session, text } => {
                let session = match session.or_else(|| ai.session_key().map(str::to_string)) {
                    // A new session's first draft, under a key chosen by the client.
                    Some(s) if AiTab::is_draft_key(&s) && ai.session.is_none() && ai.draft_session.is_none() => {
                        ai.draft_session = Some(s.clone());
                        s
                    }
                    Some(s) => s,
                    None => {
                        // The first draft of a new session makes it a draft session.
                        let key = AiTab::new_draft_key();
                        ai.draft_session = Some(key.clone());
                        key
                    }
                };
                ai.drafts.push(shed_model::Draft { session, text });
            }
            AiChange::EditDraft { index, session, old, text } => {
                if let Some(d) = ai.drafts.get_mut(index as usize).filter(|d| d.session == session && d.text == old) {
                    d.text = text;
                }
            }
            AiChange::RemoveDraft { index, session, text } => {
                let i = index as usize;
                if ai.drafts.get(i).is_some_and(|d| d.session == session && d.text == text) {
                    ai.drafts.remove(i);
                }
            }
            AiChange::Viewed { key, hash, viewed } => {
                ai.viewed.retain(|(k, _)| *k != key);
                if viewed {
                    ai.viewed.push((key, hash));
                }
                // Old marks of changes long gone do not pile up.
                if ai.viewed.len() > VIEWED_KEPT {
                    let n = ai.viewed.len() - VIEWED_KEPT;
                    ai.viewed.drain(..n);
                }
            }
            AiChange::Explained(found) => {
                for (key, hash, text) in found {
                    ai.explained.retain(|(k, _, _)| *k != key);
                    ai.explained.push((key, hash, text));
                }
                if ai.explained.len() > EXPLAINED_KEPT {
                    let n = ai.explained.len() - EXPLAINED_KEPT;
                    ai.explained.drain(..n);
                }
            }
        });
    }

    /// Applies `edit` to an AI tab, then does what the change asks of its
    /// Claude Code (model, mode, browser, a restart for folders or effort),
    /// tells the windows and saves soon.
    fn ai_edit_tab(&self, tab: TabId, edit: impl FnOnce(&mut AiTab)) {
        let mut st = self.lock();
        let Some(t) = st.tree.tab_mut(tab) else { return };
        let TabKind::Ai(ai) = &mut t.kind else { return };
        let old = ai.clone();
        edit(ai);
        if ai.mode.is_empty() {
            ai.mode = "default".into();
        }
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
            // Those going on in the background that are done stop (their
            // session files have it all; opening one again reads it).
            let away_tabs: Vec<TabId> = st.ai_away.keys().copied().collect();
            for tab in away_tabs {
                let away = st.ai_away.get_mut(&tab).unwrap();
                away.retain_mut(|rt| {
                    // (Nobody sees its changes until it is shown again, whole.)
                    rt.transcript.take_update();
                    let keep = rt.has_work();
                    if !keep {
                        info!(?tab, session = ?rt.transcript.status.session, "Claude Code done in the background");
                        rt.stop();
                    }
                    keep
                });
                if away.is_empty() {
                    st.ai_away.remove(&tab);
                }
                tree_changed |= ai_away_to_tree(&mut st, tab);
            }
            let tabs: Vec<TabId> = st.ai.keys().copied().collect();
            for tab in tabs {
                let rt = st.ai.get_mut(&tab).unwrap();
                let (one, from) = rt.transcript.take_update();
                // Entries changed in place (a background agent's progress) go
                // alone, not with everything after them.
                if !one.is_empty() {
                    let entries = one.iter().map(|&i| (i as u32, glimpse(&mut rt.transcript.entries[i]))).collect();
                    updates.push(KeeperMsg::AiEntries { tab, entries, status: rt.transcript.status.clone() });
                }
                if let Some(from) = from {
                    let entries = rt.transcript.entries[from.min(rt.transcript.entries.len())..].to_vec();
                    updates.push(KeeperMsg::AiUpdate { tab, from: from as u32, entries, status: rt.transcript.status.clone() });
                }
                let activity = activity_of(rt.transcript.status.state);
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
                        ai.fork_of = None;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A running agent goes out with its latest steps; the keeper keeps all.
    #[test]
    fn a_running_agent_goes_out_light() {
        let step = |n: usize| shed_ai::Entry::Assistant { text: format!("step {n}"), at: 0, uuid: String::new() };
        let run = shed_ai::Run { steps: (0..30).map(step).collect(), skipped: 2, ..Default::default() };
        let mut entry = shed_ai::Entry::Tool(shed_ai::Tool { name: "Agent".into(), run: Some(Box::new(run)), ..Default::default() });
        let shed_ai::Entry::Tool(sent) = glimpse(&mut entry) else { panic!() };
        let sent = sent.run.unwrap();
        assert_eq!((sent.steps.len(), sent.skipped), (LIVE_STEPS, 2 + 30 - LIVE_STEPS as u32));
        assert_eq!(sent.steps.last(), Some(&step(29)));
        let shed_ai::Entry::Tool(kept) = &mut entry else { panic!() };
        assert_eq!(kept.run.as_ref().unwrap().steps.len(), 30);
        // Ended: all of it.
        kept.run.as_mut().unwrap().state = shed_ai::RunState::Done;
        let shed_ai::Entry::Tool(sent) = glimpse(&mut entry) else { panic!() };
        assert_eq!(sent.run.unwrap().steps.len(), 30);
    }
}
