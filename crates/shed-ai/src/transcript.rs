//! A conversation built event by event.

use std::collections::HashMap;

use serde_json::Value;

use crate::{Entry, Limits, Permission, State, Status, Tool, parse_limits, summarize};

/// A tool's output beyond this is cut.
const MAX_RESULT: usize = 20_000;

#[derive(Clone, Debug, Default)]
pub struct Transcript {
    pub entries: Vec<Entry>,
    pub status: Status,
    /// The folder Claude works in, to show paths relative to it.
    pub cwd: String,
    /// The first entry changed since `take_changes`.
    changed_from: Option<usize>,
    /// Tool entries by tool-use id, to put their results in.
    tools: HashMap<String, usize>,
    /// The entries a message being streamed made, by content block index.
    streaming: HashMap<u64, usize>,
    /// Per message, the entries streamed for its text and thinking, and how
    /// many of them its complete event has matched so far.
    streamed: HashMap<String, (Vec<usize>, usize)>,
    stream_message: Option<String>,
    /// Permission suggestions by request id, as Claude Code sent them, to
    /// send back for "always allow".
    pub suggestions: HashMap<String, Value>,
    /// Each prompt's place in Claude Code's own record, to go back to it.
    pub prompts: Vec<PromptMark>,
    /// The id of the last message in Claude Code's record so far.
    last_uuid: Option<String>,
    /// The id of the record being applied.
    applying_uuid: Option<String>,
    /// The time now (Unix seconds), set by whoever feeds live events; a
    /// saved session's records carry their own.
    pub clock: u64,
    /// The time of the record being applied, if it says.
    applying_time: Option<u64>,
}

/// `2026-11-12T13:02:43.223Z` as Unix seconds.
pub fn parse_timestamp(s: &str) -> Option<u64> {
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.trim_end_matches('Z').split(':');
    let (h, min) = (t.next()?.parse::<i64>().ok()?, t.next()?.parse::<i64>().ok()?);
    let sec = t.next()?.split('.').next()?.parse::<i64>().ok()?;
    // Days from 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u64::try_from(days * 86400 + h * 3600 + min * 60 + sec).ok()
}

/// Where a prompt (the `User` entry at `entry`) sits in Claude Code's
/// record: its own message id, and the id of the last message before it
/// (none for the first). Editing it goes back to `before` and sends anew;
/// file changes since `uuid` can be undone.
#[derive(Clone, Debug, PartialEq)]
pub struct PromptMark {
    pub entry: usize,
    pub uuid: Option<String>,
    pub before: Option<String>,
}

impl Transcript {
    pub fn new(cwd: &str) -> Self {
        Self { cwd: cwd.to_string(), ..Default::default() }
    }

    fn push(&mut self, entry: Entry) -> usize {
        self.entries.push(entry);
        let i = self.entries.len() - 1;
        self.touch(i);
        i
    }

    fn touch(&mut self, i: usize) {
        self.changed_from = Some(self.changed_from.map_or(i, |c| c.min(i)));
    }

    /// The first entry that changed since last asked, if any did; then forgets.
    pub fn take_changes(&mut self) -> Option<usize> {
        self.changed_from.take()
    }

    /// Marks everything changed, for a window that needs it all.
    pub fn touch_all(&mut self) {
        self.changed_from = Some(0);
    }

    /// What the user sends, as it goes out, with the message id it goes
    /// out with.
    pub fn user(&mut self, text: &str, images: u32, now: u64, uuid: Option<String>) {
        let entry = self.push(Entry::User { text: text.to_string(), images, at: now });
        self.mark_prompt(entry, uuid);
        self.status.state = State::Working;
        self.status.since = Some(now);
    }

    fn mark_prompt(&mut self, entry: usize, uuid: Option<String>) {
        self.prompts.push(PromptMark { entry, uuid: uuid.clone(), before: self.last_uuid.clone() });
        if uuid.is_some() {
            self.last_uuid = uuid;
        }
    }

    /// The prompt at `entry`, if Claude Code's record can go back to it.
    pub fn prompt_mark(&self, entry: usize) -> Option<&PromptMark> {
        self.prompts.iter().find(|p| p.entry == entry && p.uuid.is_some())
    }

    /// Forgets the conversation from `entry` on (to send it again, changed).
    pub fn truncate(&mut self, entry: usize) {
        let entry = entry.min(self.entries.len());
        if let Some(mark) = self.prompts.iter().find(|p| p.entry == entry) {
            self.last_uuid = mark.before.clone();
        }
        self.entries.truncate(entry);
        self.prompts.retain(|p| p.entry < entry);
        self.tools.retain(|_, i| *i < entry);
        self.streaming.clear();
        self.streamed.clear();
        self.stream_message = None;
        self.status.state = State::Idle;
        self.status.since = None;
        self.touch(entry);
    }

    pub fn notice(&mut self, text: impl Into<String>, error: bool) {
        self.push(Entry::Notice { text: text.into(), error });
    }

    /// When the record being applied happened: its own time, else now.
    fn now(&self) -> u64 {
        self.applying_time.unwrap_or(self.clock)
    }

    /// Records the user's answers to Claude's questions.
    pub fn answer_questions(&mut self, request: &str, answers: Vec<String>) {
        if let Some(i) = self.entries.iter().rposition(|e| matches!(e, Entry::Permission(p) if p.request == request))
            && let Entry::Permission(p) = &mut self.entries[i]
        {
            p.answers = answers;
        }
        self.answer(request, true);
    }

    /// Records the answer to a permission request.
    pub fn answer(&mut self, request: &str, allow: bool) {
        let at = self.entries.iter().rposition(|e| matches!(e, Entry::Permission(p) if p.request == request));
        if let Some(i) = at {
            if let Entry::Permission(p) = &mut self.entries[i] {
                p.answer = Some(allow);
            }
            self.touch(i);
        }
        if !self.entries.iter().any(|e| matches!(e, Entry::Permission(p) if p.answer.is_none())) && self.status.state == State::Waiting {
            self.status.state = State::Working;
        }
        self.suggestions.remove(request);
    }

    /// The turn ended without a result (the process stopped).
    pub fn stopped(&mut self) {
        self.status.state = State::Idle;
        self.status.since = None;
        for i in 0..self.entries.len() {
            if let Entry::Permission(p) = &mut self.entries[i]
                && p.answer.is_none()
            {
                p.answer = Some(false);
                self.touch(i);
            }
        }
    }

    /// Takes one event from Claude Code's stream (or one record of a saved
    /// session). Returns the usage limits if the event told them.
    pub fn apply(&mut self, m: &Value) -> Option<Limits> {
        let kind = m.get("type").and_then(Value::as_str).unwrap_or("");
        // Side conversations (sub-agents) and injected context are not shown.
        if m.get("isSidechain").and_then(Value::as_bool) == Some(true) || m.get("isMeta").and_then(Value::as_bool) == Some(true) {
            return None;
        }
        self.applying_uuid = m.get("uuid").and_then(Value::as_str).map(String::from);
        self.applying_time = m.get("timestamp").and_then(Value::as_str).and_then(parse_timestamp);
        match kind {
            "stream_event" => self.stream(m.get("event")?),
            "assistant" => self.assistant(m.get("message")?),
            "user" => self.user_message(m.get("message")?),
            "result" => self.result(m),
            "system" => self.system(m),
            "control_request" => self.control(m),
            "rate_limit_event" => return parse_limits(m),
            _ => {}
        }
        // Messages in Claude Code's record (not stream fragments or notes).
        if matches!(kind, "assistant" | "user")
            && let Some(uuid) = self.applying_uuid.take()
        {
            self.last_uuid = Some(uuid);
        }
        None
    }

    fn stream(&mut self, e: &Value) {
        match e.get("type").and_then(Value::as_str).unwrap_or("") {
            "message_start" => {
                self.stream_message = e.get("message").and_then(|m| m.get("id")).and_then(Value::as_str).map(String::from);
                self.streaming.clear();
            }
            "content_block_start" => {
                let index = e.get("index").and_then(Value::as_u64).unwrap_or(0);
                let entry = match e.get("content_block").and_then(|b| b.get("type")).and_then(Value::as_str) {
                    Some("text") => Entry::Assistant { text: String::new(), at: self.now() },
                    Some("thinking") => Entry::Thinking { text: String::new() },
                    _ => return,
                };
                let i = self.push(entry);
                self.streaming.insert(index, i);
                if let Some(id) = self.stream_message.clone() {
                    self.streamed.entry(id).or_default().0.push(i);
                }
            }
            "content_block_delta" => {
                let index = e.get("index").and_then(Value::as_u64).unwrap_or(0);
                let Some(&i) = self.streaming.get(&index) else { return };
                let delta = e.get("delta");
                let piece = delta.and_then(|d| d.get("text").or_else(|| d.get("thinking"))).and_then(Value::as_str).unwrap_or("");
                match &mut self.entries[i] {
                    Entry::Assistant { text, .. } | Entry::Thinking { text } => text.push_str(piece),
                    _ => return,
                }
                self.touch(i);
            }
            _ => {}
        }
    }

    fn assistant(&mut self, msg: &Value) {
        let id = msg.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        if let Some(usage) = msg.get("usage") {
            let n = |k: &str| usage.get(k).and_then(Value::as_u64).unwrap_or(0);
            let context = n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens") + n("output_tokens");
            if context > 0 {
                self.status.context = context;
            }
        }
        if let Some(model) = msg.get("model").and_then(Value::as_str) {
            self.status.model = Some(model.to_string());
        }
        for block in msg.get("content").and_then(Value::as_array).cloned().unwrap_or_default() {
            match block.get("type").and_then(Value::as_str).unwrap_or("") {
                kind @ ("text" | "thinking") => {
                    let text = block.get(kind).and_then(Value::as_str).unwrap_or("").to_string();
                    // Streamed already: the complete text replaces what was streamed.
                    let streamed = self.streamed.get_mut(&id).and_then(|(entries, used)| {
                        let i = entries.get(*used).copied()?;
                        *used += 1;
                        Some(i)
                    });
                    match streamed {
                        Some(i) => {
                            match &mut self.entries[i] {
                                Entry::Assistant { text: t, .. } | Entry::Thinking { text: t } => *t = text,
                                _ => {}
                            }
                            self.touch(i);
                        }
                        None if text.trim().is_empty() => {}
                        None if kind == "text" => {
                            let at = self.now();
                            self.push(Entry::Assistant { text, at });
                        }
                        None => {
                            self.push(Entry::Thinking { text });
                        }
                    }
                }
                "tool_use" => {
                    let tool_id = block.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                    if self.tools.contains_key(&tool_id) {
                        continue;
                    }
                    let name = block.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                    let input = block.get("input").cloned().unwrap_or(Value::Null);
                    let (summary, detail, file, diff) = summarize(&name, &input, &self.cwd);
                    let i = self.push(Entry::Tool(Tool { id: tool_id.clone(), name, summary, detail, file, diff, result: None, error: false }));
                    self.tools.insert(tool_id, i);
                }
                _ => {}
            }
        }
    }

    fn user_message(&mut self, msg: &Value) {
        match msg.get("content") {
            Some(Value::String(text)) => self.prompt(text, 0),
            Some(Value::Array(blocks)) => {
                let mut text = String::new();
                let mut images = 0;
                for b in blocks {
                    match b.get("type").and_then(Value::as_str).unwrap_or("") {
                        "tool_result" => self.tool_result(b),
                        "text" => text.push_str(b.get("text").and_then(Value::as_str).unwrap_or("")),
                        "image" => images += 1,
                        _ => {}
                    }
                }
                if !text.is_empty() || images > 0 {
                    self.prompt(&text, images);
                }
            }
            _ => {}
        }
    }

    /// A user's message from a saved session: a prompt, or one of Claude
    /// Code's own notes.
    fn prompt(&mut self, text: &str, images: u32) {
        if text.starts_with("[Request interrupted") {
            return self.notice("Stopped", false);
        }
        // Command echoes and their output, as the terminal UI records them.
        if text.starts_with("<local-command") || text.starts_with("<command-message>") || text.starts_with("<bash-") {
            return;
        }
        if let Some(rest) = text.strip_prefix("<command-name>") {
            let name = rest.split("</command-name>").next().unwrap_or("");
            let args = text.split("<command-args>").nth(1).and_then(|a| a.split("</command-args>").next()).unwrap_or("");
            let at = self.now();
            let entry = self.push(Entry::User { text: format!("{name} {args}").trim().to_string(), images, at });
            let uuid = self.applying_uuid.clone();
            self.mark_prompt(entry, uuid);
            return;
        }
        let at = self.now();
        let entry = self.push(Entry::User { text: text.to_string(), images, at });
        let uuid = self.applying_uuid.clone();
        self.mark_prompt(entry, uuid);
    }

    fn tool_result(&mut self, b: &Value) {
        let id = b.get("tool_use_id").and_then(Value::as_str).unwrap_or("");
        let Some(&i) = self.tools.get(id) else { return };
        let text = match b.get("content") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(parts)) => parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
            _ => String::new(),
        };
        let mut text = text;
        if text.len() > MAX_RESULT {
            let mut cut = MAX_RESULT;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
            text.push_str("\n…");
        }
        if let Entry::Tool(t) = &mut self.entries[i] {
            t.result = Some(text);
            t.error = b.get("is_error").and_then(Value::as_bool) == Some(true);
        }
        self.touch(i);
    }

    fn result(&mut self, m: &Value) {
        self.status.state = State::Idle;
        self.status.since = None;
        if let Some(cost) = m.get("total_cost_usd").and_then(Value::as_f64) {
            self.status.cost = cost;
        }
        let secs = m.get("duration_ms").and_then(Value::as_f64).unwrap_or(0.0) / 1000.0;
        match m.get("subtype").and_then(Value::as_str).unwrap_or("") {
            "success" => self.notice(format!("Done in {secs:.0} s"), false),
            "error_during_execution" => {
                if !matches!(self.entries.last(), Some(Entry::Notice { text, .. }) if text == "Stopped") {
                    self.notice("Stopped", false);
                }
            }
            "error_max_turns" => self.notice("Stopped: too many turns", true),
            other => {
                let why = m.get("result").and_then(Value::as_str).unwrap_or(other);
                self.notice(format!("Failed: {why}"), true);
            }
        }
    }

    fn system(&mut self, m: &Value) {
        match m.get("subtype").and_then(Value::as_str).unwrap_or("") {
            "init" => {
                self.status.session = m.get("session_id").and_then(Value::as_str).map(String::from);
                self.status.model = m.get("model").and_then(Value::as_str).map(String::from);
                if let Some(cwd) = m.get("cwd").and_then(Value::as_str) {
                    self.cwd = cwd.to_string();
                }
            }
            "compact_boundary" => self.notice("The conversation was compacted", false),
            "permission_denied" => {
                let why = m.get("message").and_then(Value::as_str).unwrap_or("Permission denied");
                self.notice(why.to_string(), true);
            }
            _ => {}
        }
    }

    fn control(&mut self, m: &Value) {
        let Some(req) = m.get("request") else { return };
        if req.get("subtype").and_then(Value::as_str) != Some("can_use_tool") {
            return;
        }
        let request = m.get("request_id").and_then(Value::as_str).unwrap_or("").to_string();
        let tool = req.get("tool_name").and_then(Value::as_str).unwrap_or("").to_string();
        let input = req.get("input").cloned().unwrap_or(Value::Null);
        let (summary, detail, _, diff) = summarize(&tool, &input, &self.cwd);
        if let Some(s) = req.get("permission_suggestions") {
            self.suggestions.insert(request.clone(), s.clone());
        }
        let questions = if tool == "AskUserQuestion" { crate::parse_questions(&input) } else { Vec::new() };
        self.push(Entry::Permission(Permission { request, tool, summary, detail, diff, answer: None, questions, answers: Vec::new() }));
        self.status.state = State::Waiting;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replay(file: &str) -> Transcript {
        let mut t = Transcript::new("/tmp/shed-ai-test");
        for line in std::fs::read_to_string(format!("{}/tests/streams/{file}", env!("CARGO_MANIFEST_DIR"))).unwrap().lines() {
            let v: Value = serde_json::from_str(line).unwrap();
            t.apply(&v);
        }
        t
    }

    #[test]
    fn a_streamed_turn_with_tools_and_a_permission() {
        let mut t = replay("edit-and-bash.jsonl");
        let kinds: Vec<&str> = t
            .entries
            .iter()
            .map(|e| match e {
                Entry::User { .. } => "user",
                Entry::Assistant { .. } => "text",
                Entry::Thinking { .. } => "thinking",
                Entry::Tool(_) => "tool",
                Entry::Permission(_) => "permission",
                Entry::Notice { .. } => "notice",
            })
            .collect();
        assert_eq!(kinds, ["tool", "tool", "permission", "tool", "text", "notice"], "{:#?}", t.entries);
        let Entry::Tool(edit) = &t.entries[1] else { panic!() };
        assert_eq!(edit.summary, "Edit notes.txt");
        assert!(edit.result.as_deref().unwrap().contains("updated successfully"));
        assert!(edit.diff.is_some());
        let Entry::Tool(bash) = &t.entries[3] else { panic!() };
        assert_eq!((bash.summary.as_str(), bash.result.as_deref()), ("$ wc -l notes.txt", Some("3 notes.txt")));
        assert!(matches!(&t.entries[4], Entry::Assistant { text, .. } if text == "`notes.txt` has 3 lines now."));
        assert_eq!(t.status.state, State::Idle);
        assert!(t.status.cost > 0.0 && t.status.context > 0);
        assert!(t.status.session.is_some());
        assert_eq!(t.take_changes(), Some(0));
        assert_eq!(t.take_changes(), None);
        let Entry::Permission(p) = &t.entries[2] else { panic!() };
        let request = p.request.clone();
        t.answer(&request, true);
        assert_eq!(t.take_changes(), Some(2));
    }

    #[test]
    fn times_come_from_records_or_the_clock() {
        use serde_json::json;
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_timestamp("2026-11-12T13:02:43.223Z"), Some(1_794_488_563));
        assert_eq!(parse_timestamp("2000-02-29T12:00:00Z"), Some(951_825_600));
        assert_eq!(parse_timestamp("nonsense"), None);
        let mut t = Transcript::new("/tmp");
        t.apply(&json!({"type": "user", "uuid": "p1", "timestamp": "2026-11-12T13:00:00.000Z", "message": {"role": "user", "content": "hi"}}));
        t.clock = 5;
        t.apply(&json!({"type": "assistant", "uuid": "a1", "message": {"id": "m1", "content": [{"type": "text", "text": "hello"}]}}));
        assert!(matches!(t.entries[0], Entry::User { at: 1_794_488_400, .. }));
        assert!(matches!(t.entries[1], Entry::Assistant { at: 5, .. }), "live: the clock");
    }

    #[test]
    fn prompts_know_where_to_go_back_to() {
        use serde_json::json;
        let mut t = Transcript::new("/tmp");
        t.user("first", 0, 0, Some("u1".into()));
        t.apply(&json!({"type": "assistant", "uuid": "a1", "message": {"id": "m1", "content": [{"type": "text", "text": "one"}]}}));
        t.apply(&json!({"type": "result", "subtype": "success", "uuid": "r1"}));
        t.user("second", 0, 0, Some("u2".into()));
        t.apply(&json!({"type": "assistant", "uuid": "a2", "message": {"id": "m2", "content": [{"type": "text", "text": "two"}]}}));
        let second = t.entries.iter().rposition(|e| matches!(e, Entry::User { .. })).unwrap();
        assert_eq!(t.prompt_mark(0).unwrap().before, None);
        let mark = t.prompt_mark(second).unwrap().clone();
        assert_eq!((mark.uuid.as_deref(), mark.before.as_deref()), (Some("u2"), Some("a1")), "the result is not in the record");
        t.truncate(second);
        assert_eq!(t.entries.len(), second);
        assert!(t.prompt_mark(second).is_none() && t.prompt_mark(0).is_some());
        t.user("second, again", 0, 0, Some("u3".into()));
        assert_eq!(t.prompt_mark(second).unwrap().before.as_deref(), Some("a1"));
        // A saved session's prompts carry their ids too.
        let mut saved = Transcript::new("/tmp");
        saved.apply(&json!({"type": "user", "uuid": "p1", "message": {"role": "user", "content": "hello"}}));
        saved.apply(&json!({"type": "assistant", "uuid": "a1", "message": {"id": "m1", "content": [{"type": "text", "text": "hi"}]}}));
        saved.apply(&json!({"type": "user", "uuid": "p2", "message": {"role": "user", "content": "again"}}));
        assert_eq!(saved.prompt_mark(2).unwrap(), &PromptMark { entry: 2, uuid: Some("p2".into()), before: Some("a1".into()) });
    }

    #[test]
    fn a_saved_session_reads_back() {
        let t = replay("session.jsonl");
        assert!(matches!(&t.entries[0], Entry::User { text, .. } if text.starts_with("Read notes.txt")), "{:#?}", t.entries);
        assert!(t.entries.iter().any(|e| matches!(e, Entry::Tool(tool) if tool.name == "Bash" && tool.result.is_some())));
        assert!(matches!(t.entries.last(), Some(Entry::Assistant { .. })));
    }
}
