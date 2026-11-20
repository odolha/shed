//! What goes on beside the conversation: sub-agents (their steps, under
//! their Agent tool), commands and watches in the background, and the
//! wake-ups and jobs Claude sets for later. Claude Code tells of them in
//! tool results, `system` events (`task_started`, `task_progress`,
//! `task_notification`), `command_lifecycle` events (a wake-up starting a
//! turn), and in a saved session as `<task-notification>` messages,
//! `queued_command` attachments and `scheduled` turns.

use serde_json::Value;

use super::{Transcript, cut, message_text, result_text};
use crate::notes::{TaskNote, agent_report, first_line, peer_message};
use crate::{Entry, NoteKind, Run, RunState, State, Timer, TimerKind, Tool, summarize};

/// Tools whose input is kept, for what their result means.
pub(super) const KEEP_INPUT: &[&str] = &["Agent", "Task", "Bash", "Monitor", "TaskStop", "ScheduleWakeup", "CronCreate", "CronDelete", "CronList", "SendMessage"];

fn is_agent(name: &str) -> bool {
    matches!(name, "Agent" | "Task")
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// A number Claude Code may write as an integer or a float (`nextFireAt`).
fn num(v: Option<&Value>) -> Option<u64> {
    v.and_then(|x| x.as_u64().or_else(|| x.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64)))
}

/// The word after `marker` in `text` (`"ID: "` → the id).
fn word_after(text: &str, marker: &str) -> Option<String> {
    let rest = &text[text.find(marker)? + marker.len()..];
    let word: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
    (!word.is_empty()).then_some(word)
}

/// An agent tool's description, from its summary (`Agent: Count files`).
fn agent_name(t: &Tool) -> String {
    t.summary.strip_prefix("Agent: ").unwrap_or(&t.summary).to_string()
}

impl Transcript {
    fn tool_mut(&mut self, i: usize) -> Option<&mut Tool> {
        match self.entries.get_mut(i) {
            Some(Entry::Tool(t)) => Some(t),
            _ => None,
        }
    }

    /// A tool that goes on after its call gets a run as it starts: an
    /// agent, a command in the background, a watch.
    pub(super) fn run_for(&self, name: &str, input: &Value) -> Option<Box<Run>> {
        let background = input.get("run_in_background").and_then(Value::as_bool) == Some(true);
        let run = match name {
            n if is_agent(n) => Run { background, agent: s(input, "subagent_type").to_string(), last: "Starting".into(), ..Default::default() },
            "Bash" if background => Run { background, last: s(input, "description").to_string(), ..Default::default() },
            "Monitor" => Run { background: true, last: "Watching".into(), ..Default::default() },
            _ => return None,
        };
        Some(Box::new(Run { started: self.now(), ..run }))
    }

    /// A tool's line, with the tasks it names by what they are (`Stop
    /// “three events”` rather than a task id).
    pub(super) fn name_tasks(&self, name: &str, input: &Value, summary: String) -> String {
        let task = |id: &str| {
            self.runs.get(id).and_then(|&i| match self.entries.get(i) {
                Some(Entry::Tool(t)) if is_agent(&t.name) => Some(format!("agent “{}”", agent_name(t))),
                Some(Entry::Tool(t)) => Some(format!("“{}”", t.summary)),
                _ => None,
            })
        };
        match name {
            "TaskStop" | "KillShell" => {
                let id = [s(input, "task_id"), s(input, "shell_id")].into_iter().find(|x| !x.is_empty()).unwrap_or("");
                task(id).map_or(summary, |what| format!("Stop {what}"))
            }
            "SendMessage" => match task(s(input, "to")) {
                Some(who) => format!("Message to {who}: {}", first_line(s(input, "summary"), 80)),
                None => summary,
            },
            "CronDelete" => match self.status.timers.iter().find(|t| t.id == s(input, "id")) {
                Some(t) => format!("Cancel the scheduled job “{}”", first_line(&t.what, 60)),
                None => summary,
            },
            _ => summary,
        }
    }

    /// A tool's result, for what goes on after it: an agent started in the
    /// background or done, a command's task, a timer set or cancelled.
    pub(super) fn task_result(&mut self, i: usize, id: &str, error: bool, extra: Option<&Value>) {
        let Some(input) = self.inputs.remove(id) else { return };
        let now = self.now();
        let Some(t) = self.tool_mut(i) else { return };
        let name = t.name.clone();
        let text = t.result.clone().unwrap_or_default();
        let extra = extra.cloned().unwrap_or(Value::Null);
        match name.as_str() {
            n if is_agent(n) => {
                let Some(run) = t.run.as_mut() else { return };
                let agent_id = Some(s(&extra, "agentId").to_string()).filter(|a| !a.is_empty()).or_else(|| word_after(&text, "agentId: "));
                if let Some(a) = &agent_id {
                    run.task = a.clone();
                }
                if s(&extra, "status") == "async_launched" || text.starts_with("Async agent launched") {
                    run.background = true;
                    run.output = s(&extra, "outputFile").to_string();
                    t.result = Some("Started in the background".into());
                } else if error {
                    run.state = RunState::Failed;
                    run.ended = now;
                    run.last = first_line(&text, 120);
                } else {
                    run.state = RunState::Done;
                    let took = num(extra.get("totalDurationMs")).map(|ms| ms / 1000);
                    run.ended = took.filter(|_| run.started > 0).map_or(now, |d| run.started + d);
                    if let Some(n) = num(extra.get("totalToolUseCount")) {
                        run.tools = n as u32;
                    }
                    if let Some(n) = num(extra.get("totalTokens")) {
                        run.tokens = n;
                    }
                    let report = extra.get("handbackReport").and_then(|r| r.get("text")).and_then(Value::as_str).map(String::from).unwrap_or_else(|| agent_report(&text));
                    run.last = "Finished".into();
                    run.report = report.clone();
                    t.result = Some(report);
                }
                if let Some(a) = agent_id {
                    self.runs.insert(a, i);
                }
            }
            "Bash" => {
                let Some(run) = t.run.as_mut() else { return };
                let task = Some(s(&extra, "backgroundTaskId").to_string()).filter(|x| !x.is_empty()).or_else(|| word_after(&text, "in background with ID: "));
                match task {
                    Some(task) if !error => {
                        run.output = text.find("being written to: ").map(|at| text[at + "being written to: ".len()..].split(". ").next().unwrap_or("").trim_end_matches('.').to_string()).unwrap_or_default();
                        run.task = task.clone();
                        self.runs.insert(task, i);
                    }
                    // Not in the background after all (or refused): it is done.
                    _ => t.run = None,
                }
            }
            "Monitor" => {
                let Some(run) = t.run.as_mut() else { return };
                let task = Some(s(&extra, "taskId").to_string()).filter(|x| !x.is_empty()).or_else(|| word_after(&text, "(task "));
                let Some(task) = task.filter(|_| !error) else {
                    t.run = None;
                    return;
                };
                run.task = task.clone();
                let what = s(&input, "description").to_string();
                run.last = format!("Watching: {what}");
                let timeout = num(extra.get("timeoutMs")).or_else(|| num(input.get("timeout_ms"))).map(|ms| ms / 1000);
                self.runs.insert(task.clone(), i);
                self.status.timers.retain(|x| !(x.kind == TimerKind::Watch && x.id == task));
                self.status.timers.push(Timer { kind: TimerKind::Watch, id: task, what, at: timeout.map_or(0, |d| now + d), tool: id.to_string(), ..Default::default() });
                self.touch_status();
            }
            "TaskStop" | "KillShell" if !error => {
                let task = [s(&input, "task_id"), s(&input, "shell_id")].into_iter().find(|x| !x.is_empty()).unwrap_or("").to_string();
                self.status.timers.retain(|x| !(x.kind == TimerKind::Watch && x.id == task));
                if let Some(&j) = self.runs.get(&task)
                    && let Some(run) = self.tool_mut(j).and_then(|t| t.run.as_mut())
                    && run.running()
                {
                    run.state = RunState::Stopped;
                    run.ended = now;
                    run.last = "Stopped by Claude".into();
                    self.touch_one(j);
                }
                self.touch_status();
            }
            "ScheduleWakeup" if !error => {
                // A new wake-up takes the place of the one waiting; `stop` ends them.
                self.status.timers.retain(|x| x.kind != TimerKind::Wakeup);
                if input.get("stop").and_then(Value::as_bool) != Some(true) {
                    let delay = num(extra.get("clampedDelaySeconds")).or_else(|| num(input.get("delaySeconds"))).unwrap_or(0);
                    let at = num(extra.get("scheduledFor")).filter(|&ms| ms > 0).map_or(now + delay, |ms| ms / 1000);
                    let what = Some(s(&input, "reason").to_string()).filter(|r| !r.is_empty()).unwrap_or_else(|| first_line(s(&input, "prompt"), 80));
                    self.status.timers.push(Timer { kind: TimerKind::Wakeup, id: id.to_string(), what, at, prompt: s(&input, "prompt").to_string(), tool: id.to_string(), ..Default::default() });
                }
                self.touch_status();
            }
            "CronCreate" if !error => {
                let job = Some(s(&extra, "id").to_string()).filter(|x| !x.is_empty()).or_else(|| word_after(&text, "job "));
                let recurring = extra.get("recurring").or_else(|| input.get("recurring")).and_then(Value::as_bool).unwrap_or(true);
                let every = if recurring {
                    Some(s(&extra, "humanSchedule").to_string()).filter(|x| !x.is_empty()).unwrap_or_else(|| s(&input, "cron").to_string())
                } else {
                    String::new()
                };
                let prompt = s(&input, "prompt").to_string();
                if !every.is_empty() {
                    t.summary = format!("Schedule: {} · {}", every.to_lowercase(), first_line(&prompt, 80));
                }
                let at = num(extra.get("nextFireAt")).map_or(0, |ms| ms / 1000);
                self.status.timers.push(Timer { kind: TimerKind::Job, id: job.unwrap_or_else(|| id.to_string()), what: first_line(&prompt, 80), at, every, prompt, tool: id.to_string() });
                self.touch_status();
            }
            "CronDelete" if !error => {
                let job = s(&input, "id").to_string();
                self.status.timers.retain(|x| !(x.kind != TimerKind::Watch && x.id == job));
                self.touch_status();
            }
            // The jobs as Claude Code has them: those it no longer has went.
            "CronList" if !error => {
                if let Some(jobs) = extra.get("jobs").and_then(Value::as_array) {
                    let ids: Vec<&str> = jobs.iter().map(|j| s(j, "id")).collect();
                    let prompts: Vec<&str> = jobs.iter().map(|j| s(j, "prompt")).collect();
                    // (A wake-up is a one-off job to Claude Code, known by its prompt.)
                    self.status.timers.retain(|x| match x.kind {
                        TimerKind::Job => ids.contains(&x.id.as_str()),
                        TimerKind::Wakeup => prompts.contains(&x.prompt.as_str()),
                        TimerKind::Watch => true,
                    });
                    for j in jobs {
                        let (job, prompt) = (s(j, "id"), s(j, "prompt"));
                        let at = num(j.get("nextFireAt")).map_or(0, |ms| ms / 1000);
                        if let Some(known) = self.status.timers.iter_mut().find(|x| x.id == job || (x.kind == TimerKind::Wakeup && x.prompt == prompt)) {
                            if at > 0 {
                                known.at = at;
                            }
                            continue;
                        }
                        let recurring = j.get("recurring").and_then(Value::as_bool) == Some(true);
                        let every = if recurring { s(j, "humanSchedule").to_string() } else { String::new() };
                        self.status.timers.push(Timer { kind: TimerKind::Job, id: job.to_string(), what: first_line(prompt, 80), at, every, prompt: prompt.to_string(), tool: id.to_string() });
                    }
                    self.touch_status();
                }
            }
            _ => {}
        }
    }

    /// A `system` event about a task: started, progress, done.
    pub(super) fn task_event(&mut self, m: &Value) {
        let task = s(m, "task_id").to_string();
        let tool = s(m, "tool_use_id");
        let i = self.tools.get(tool).or_else(|| self.runs.get(&task)).copied();
        let now = self.now();
        match s(m, "subtype") {
            "task_started" => {
                let Some(i) = i else { return };
                let background = m.get("is_backgrounded").and_then(Value::as_bool) == Some(true);
                let Some(t) = self.tool_mut(i) else { return };
                let run = t.run.get_or_insert_with(|| Box::new(Run { started: now, ..Default::default() }));
                run.task = task.clone();
                run.background |= background;
                if run.agent.is_empty() {
                    run.agent = s(m, "subagent_type").to_string();
                }
                self.runs.insert(task, i);
                self.touch_one(i);
            }
            "task_progress" => {
                let Some(i) = i else { return };
                let usage = m.get("usage").cloned().unwrap_or(Value::Null);
                let Some(run) = self.tool_mut(i).and_then(|t| t.run.as_mut()) else { return };
                if let Some(n) = num(usage.get("tool_uses")) {
                    run.tools = run.tools.max(n as u32);
                }
                if let Some(n) = num(usage.get("total_tokens")) {
                    run.tokens = n;
                }
                if run.steps.is_empty() {
                    run.last = s(m, "description").to_string();
                }
                self.touch_one(i);
            }
            "task_updated" => {
                let patch = m.get("patch").cloned().unwrap_or(Value::Null);
                let Some(state) = run_state(s(&patch, "status")) else { return };
                let Some(i) = i else { return };
                let Some(run) = self.tool_mut(i).and_then(|t| t.run.as_mut()) else { return };
                run.state = state;
                run.ended = num(patch.get("end_time")).map_or(now, |ms| ms / 1000);
                self.touch_one(i);
            }
            "task_notification" => {
                let usage = m.get("usage").cloned().unwrap_or(Value::Null);
                let note = TaskNote {
                    task,
                    tool: Some(tool.to_string()).filter(|t| !t.is_empty()),
                    status: s(m, "status").to_string(),
                    summary: s(m, "summary").to_string(),
                    output: s(m, "output_file").to_string(),
                    tokens: num(usage.get("total_tokens")).unwrap_or(0),
                    tool_uses: num(usage.get("tool_uses")).unwrap_or(0) as u32,
                    duration_ms: num(usage.get("duration_ms")).unwrap_or(0),
                    ..Default::default()
                };
                self.task_note(&note, true);
            }
            _ => {}
        }
    }

    /// A task finished (or a watch saw something): its tool's run says so,
    /// and when it ran in the background, a note in the conversation.
    /// Live, an agent's summary is what it ended with.
    pub(super) fn task_note(&mut self, n: &TaskNote, live: bool) {
        let key = format!("{}|{}|{}|{}", n.task, n.status, n.event, n.summary);
        if !self.noted.insert(key) {
            return;
        }
        self.woken = true;
        let i = n.tool.as_deref().and_then(|t| self.tools.get(t)).or_else(|| self.runs.get(&n.task)).copied();
        let now = self.now();
        // A watch's event: what it saw.
        if n.status.is_empty() && !n.event.is_empty() {
            if let Some(run) = i.and_then(|i| self.tool_mut(i)).and_then(|t| t.run.as_mut()) {
                run.last = first_line(&n.event, 120);
                self.touch_one(i.unwrap_or_default());
            }
            let text = format!("{}: {}", n.summary.trim_end_matches(':'), first_line(&n.event, 100));
            return self.note(NoteKind::Task, text, n.event.clone(), false);
        }
        let state = run_state(&n.status);
        let mut background = true;
        let mut text = n.summary.clone();
        let mut detail = n.result.clone();
        if let Some(i) = i
            && let Some(t) = self.tool_mut(i)
        {
            let agent = is_agent(&t.name);
            let name = agent_name(t);
            if let Some(run) = t.run.as_mut() {
                background = run.background;
                if let Some(state) = state {
                    run.state = state;
                }
                if run.ended == 0 || state.is_some() {
                    run.ended = if n.duration_ms > 0 && run.started > 0 { run.started + n.duration_ms / 1000 } else { now };
                }
                if n.tool_uses > 0 {
                    run.tools = n.tool_uses;
                }
                if n.tokens > 0 {
                    run.tokens = n.tokens;
                }
                if !n.output.is_empty() {
                    run.output = n.output.clone();
                }
                let report = if agent && live { n.summary.clone() } else if agent { n.result.clone() } else { n.summary.clone() };
                if !report.is_empty() {
                    run.report = agent_report(&report);
                }
                run.last = match run.state {
                    RunState::Done => "Finished".into(),
                    RunState::Failed => "Failed".into(),
                    RunState::Stopped => "Stopped".into(),
                    RunState::Running => run.last.clone(),
                };
                if agent {
                    detail = run.report.clone();
                }
            }
            if agent && (live || text.is_empty()) {
                let how = match state {
                    Some(RunState::Failed) => "failed",
                    Some(RunState::Stopped) => "was stopped",
                    _ => "finished",
                };
                text = format!("Agent “{name}” {how}");
            }
            self.touch_one(i);
        }
        self.status.timers.retain(|x| !(x.kind == TimerKind::Watch && x.id == n.task));
        self.touch_status();
        // A foreground agent's end shows in its own line.
        if background {
            if text.is_empty() {
                text = format!("Background task {}", if n.status.is_empty() { "finished" } else { &n.status });
            }
            self.note(NoteKind::Task, text, detail, n.failed());
        }
    }

    /// A message Claude Code marks as its own (not shown as the user's):
    /// a wake-up or scheduled job firing, an agent handing back.
    pub(super) fn meta_message(&mut self, m: &Value) {
        let text = message_text(m.get("message").and_then(|x| x.get("content")));
        let origin = m.get("origin").map_or("", |o| s(o, "kind"));
        if origin == "peer" {
            return self.peer(&text);
        }
        if s(m, "turnOrigin") == "scheduled" {
            self.fired_by_prompt(&text);
        }
    }

    /// A background agent's report, sent as a message.
    fn peer(&mut self, text: &str) {
        let (from, said) = peer_message(text);
        self.woken = true;
        let named = self.runs.get(&from).copied().and_then(|i| match self.entries.get_mut(i) {
            Some(Entry::Tool(t)) => {
                let name = agent_name(t);
                if let Some(run) = t.run.as_mut() {
                    run.report = said.clone();
                }
                Some((i, name))
            }
            _ => None,
        });
        let text = match &named {
            Some((i, name)) => {
                self.touch_one(*i);
                format!("Agent “{name}” reported back")
            }
            None => "A message from another Claude session".into(),
        };
        self.note(NoteKind::Message, text, said, false);
    }

    /// A saved session's attachment: a notification or a message that
    /// came while Claude worked.
    pub(super) fn attachment(&mut self, m: &Value) {
        let Some(a) = m.get("attachment") else { return };
        if s(a, "type") != "queued_command" {
            return;
        }
        let prompt = match a.get("prompt") {
            Some(Value::String(p)) => p.clone(),
            other => message_text(other),
        };
        let origin = a.get("origin").map_or("", |o| s(o, "kind"));
        if s(a, "commandMode") == "task-notification" || prompt.trim_start().starts_with("<task-notification>") {
            for n in crate::notes::task_notes(&prompt) {
                self.task_note(&n, false);
            }
        } else if origin == "peer" || prompt.trim_start().starts_with("<agent-message") {
            self.peer(&prompt);
        } else if !prompt.trim().is_empty() {
            // The user's own words, sent while Claude worked (in Claude
            // Code's own interface): theirs, though not one to go back to.
            let at = self.now();
            let read = crate::notes::read_user_text(&prompt);
            if !read.prompt.is_empty() {
                let entry = self.push(Entry::User { text: read.prompt, images: 0, at, pictures: Vec::new() });
                self.mark_prompt(entry, None);
            }
        }
    }

    /// A turn starting live: a wake-up or a scheduled job firing.
    pub(super) fn lifecycle(&mut self, m: &Value) {
        if s(m, "state") != "started" {
            return;
        }
        let source = s(m, "wakeup_source");
        // A prompt of the user's, queued and now going.
        if source.is_empty() {
            self.woken = true;
            return self.begin_turn();
        }
        let kind = if source == "loop_wakeup" { TimerKind::Wakeup } else { TimerKind::Job };
        let scheduled = num(m.get("scheduled_for")).map(|ms| ms / 1000);
        let now = self.now();
        // The one due: at the time it was set for, else the nearest.
        let found = self
            .status
            .timers
            .iter()
            .enumerate()
            .filter(|(_, t)| t.kind == kind)
            .min_by_key(|(_, t)| scheduled.map_or(t.at.abs_diff(now), |s| t.at.abs_diff(s)))
            .map(|(n, _)| n);
        self.fire(found, kind);
        self.begin_turn();
    }

    /// A saved session's scheduled turn: the timer whose prompt it is.
    fn fired_by_prompt(&mut self, prompt: &str) {
        let found = self.status.timers.iter().position(|t| t.prompt.trim() == prompt.trim());
        let kind = found.map_or(TimerKind::Wakeup, |n| self.status.timers[n].kind);
        if found.is_none() {
            self.woken = true;
            return self.note(NoteKind::Wakeup, format!("Woke up: {}", first_line(prompt, 100)), prompt.to_string(), false);
        }
        self.fire(found, kind);
    }

    /// A timer fired: a note says so; a one-off one is done.
    fn fire(&mut self, n: Option<usize>, kind: TimerKind) {
        self.woken = true;
        let Some(n) = n else {
            let text = if kind == TimerKind::Wakeup { "Woke up" } else { "A scheduled job ran" };
            return self.note(NoteKind::Wakeup, text, "", false);
        };
        let t = self.status.timers[n].clone();
        if t.every.is_empty() {
            self.status.timers.remove(n);
        } else {
            // Recurring: when next is not known here.
            self.status.timers[n].at = 0;
        }
        self.touch_status();
        let text = match t.kind {
            TimerKind::Wakeup => format!("Woke up: {}", t.what),
            _ => format!("Scheduled job ran: {}", t.what),
        };
        self.note(NoteKind::Wakeup, text, t.prompt, false);
    }

    fn begin_turn(&mut self) {
        if self.status.state == State::Idle {
            self.status.state = State::Working;
            self.status.since = Some(self.now());
            self.touch_status();
        }
    }

    /// A turn started with no prompt, and no notification or wake-up said
    /// why: a watch saw something (live, its event is not shown).
    pub(super) fn turn_by_itself(&mut self) {
        // (A conversation's first turn is the user's.)
        if !self.woken && !self.entries.is_empty() {
            let watches: Vec<String> = self.status.timers.iter().filter(|t| t.kind == TimerKind::Watch).map(|t| t.what.clone()).collect();
            let text = match watches.as_slice() {
                [one] => format!("The watch “{one}” saw something"),
                _ => "Claude went on by itself (a background task told it something)".into(),
            };
            self.note(NoteKind::Task, text, "", false);
        }
        self.woken = true;
        self.begin_turn();
    }

    /// A sub-agent's message, live: under its Agent tool (or the agent
    /// that called the tool it belongs to, for an agent's own agents).
    pub(super) fn sub_message(&mut self, parent: &str, kind: &str, m: &Value) {
        let Some(top) = self.owners.get(parent).or_else(|| self.tools.get(parent)).copied() else { return };
        self.sub_record(top, kind, m);
    }

    /// One of a sub-agent's records, as a step of the agent's run at `top`.
    fn sub_record(&mut self, top: usize, kind: &str, m: &Value) {
        let at = self.now();
        let cwd = self.cwd.clone();
        let Some(content) = m.get("message").and_then(|x| x.get("content")).and_then(Value::as_array).cloned() else { return };
        let Some(t) = self.tool_mut(top) else { return };
        let run = t.run.get_or_insert_with(|| Box::new(Run { started: at, ..Default::default() }));
        let mut called = Vec::new();
        for b in &content {
            match (kind, s(b, "type")) {
                ("assistant", "text") => {
                    let text = s(b, "text").trim().to_string();
                    if !text.is_empty() {
                        run.last = first_line(&text, 120);
                        push_step(run, Entry::Assistant { text, at });
                    }
                }
                ("assistant", "tool_use") => {
                    let id = s(b, "id").to_string();
                    if run.steps.iter().any(|e| matches!(e, Entry::Tool(x) if x.id == id)) {
                        continue;
                    }
                    let name = s(b, "name").to_string();
                    let input = b.get("input").cloned().unwrap_or(Value::Null);
                    let (summary, detail, file, diff) = summarize(&name, &input, &cwd);
                    // A step is a glimpse: long inputs and changes are cut.
                    let detail = cut(detail, Run::MAX_STEP_RESULT);
                    let diff = diff.map(|d| crate::Diff { old: cut(d.old, 2 * Run::MAX_STEP_RESULT), new: cut(d.new, 2 * Run::MAX_STEP_RESULT) });
                    run.last.clone_from(&summary);
                    run.tools += 1;
                    called.push(id.clone());
                    push_step(run, Entry::Tool(Tool { id, name, summary, detail, file, diff, ..Default::default() }));
                }
                ("user", "tool_result") => {
                    let id = s(b, "tool_use_id");
                    if let Some(Entry::Tool(step)) = run.steps.iter_mut().rev().find(|e| matches!(e, Entry::Tool(x) if x.id == id)) {
                        step.result = Some(cut(result_text(b), Run::MAX_STEP_RESULT));
                        step.error = b.get("is_error").and_then(Value::as_bool) == Some(true);
                    }
                }
                _ => {}
            }
        }
        for id in called {
            self.owners.insert(id, top);
        }
        self.touch_one(top);
    }

    /// The runs still going, ended: the process they ran in stopped.
    pub(super) fn end_runs(&mut self, why: &str) {
        let now = self.now();
        let mut ended = Vec::new();
        for (i, e) in self.entries.iter_mut().enumerate() {
            if let Entry::Tool(t) = e
                && let Some(run) = t.run.as_mut()
                && run.running()
            {
                run.state = RunState::Stopped;
                run.ended = now;
                run.last = why.to_string();
                ended.push(i);
            }
        }
        for i in ended {
            self.touch_one(i);
        }
    }

    /// The sub-agents of a saved session (`<session>/subagents/agent-ID.jsonl`,
    /// with `agent-ID.meta.json` naming the tool call that started it), as
    /// steps of their Agent tools.
    pub fn read_subagents(&mut self, dir: &std::path::Path) {
        use std::io::BufRead;
        let Ok(files) = std::fs::read_dir(dir) else { return };
        let mut files: Vec<_> = files.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "jsonl")).collect();
        files.sort();
        for path in files {
            let Some(agent) = path.file_stem().and_then(|n| n.to_str()).and_then(|n| n.strip_prefix("agent-")).map(String::from) else { continue };
            let meta: Value = std::fs::read_to_string(path.with_extension("meta.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
            let Some(top) = self.tools.get(s(&meta, "toolUseId")).or_else(|| self.runs.get(&agent)).copied() else { continue };
            if !self.tool_mut(top).is_some_and(|t| is_agent(&t.name)) {
                continue;
            }
            let Ok(file) = std::fs::File::open(&path) else { continue };
            // Its tool calls are counted again from its steps (the result's
            // count, if it gave one, stays if larger).
            let counted = self.tool_mut(top).and_then(|t| t.run.as_mut()).map_or(0, |r| std::mem::take(&mut r.tools));
            let mut last = None;
            for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
                // Only messages: most lines are other records.
                if !(line.contains("\"assistant\"") || line.contains("\"tool_result\"")) {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                let kind = s(&v, "type").to_string();
                if !matches!(kind.as_str(), "assistant" | "user") {
                    continue;
                }
                self.applying_time = v.get("timestamp").and_then(Value::as_str).and_then(super::parse_timestamp);
                last = self.applying_time.or(last);
                self.sub_record(top, &kind, &v);
            }
            self.applying_time = None;
            let finished = meta.get("finished").and_then(Value::as_bool) == Some(true);
            if let Some(run) = self.tool_mut(top).and_then(|t| t.run.as_mut()) {
                run.tools = run.tools.max(counted);
                if run.task.is_empty() {
                    run.task = agent.clone();
                }
                if run.agent.is_empty() {
                    run.agent = s(&meta, "agentType").to_string();
                }
                if finished && run.running() {
                    run.state = RunState::Done;
                    run.last = "Finished".into();
                }
                if !run.running() && run.ended == 0 {
                    run.ended = last.unwrap_or(0);
                }
            }
            self.runs.insert(agent, top);
        }
    }

    /// A saved session read: no Claude Code runs it now, so what was still
    /// going ended with it, and nothing it set for later will fire.
    pub fn finish_loading(&mut self) {
        self.end_runs("Ended with its session");
        self.status.timers.clear();
        self.woken = false;
    }
}

fn run_state(status: &str) -> Option<RunState> {
    match status {
        "completed" => Some(RunState::Done),
        "failed" => Some(RunState::Failed),
        "killed" | "stopped" => Some(RunState::Stopped),
        _ => None,
    }
}

/// A step of a sub-agent's run; the oldest go past `Run::MAX_STEPS`.
fn push_step(run: &mut Run, step: Entry) {
    run.steps.push(step);
    if run.steps.len() > Run::MAX_STEPS {
        let over = run.steps.len() - Run::MAX_STEPS;
        run.steps.drain(..over);
        run.skipped += over as u32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Note;
    use serde_json::json;

    fn lines(file: &str) -> Vec<Value> {
        std::fs::read_to_string(format!("{}/tests/streams/{file}", env!("CARGO_MANIFEST_DIR"))).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    fn tool<'a>(t: &'a Transcript, summary: &str) -> &'a Tool {
        t.entries
            .iter()
            .find_map(|e| match e {
                Entry::Tool(x) if x.summary == summary => Some(x),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {summary} in {:#?}", t.entries))
    }

    fn notes(t: &Transcript) -> Vec<String> {
        t.entries.iter().filter_map(|e| if let Entry::Note(n) = e { Some(n.text.clone()) } else { None }).collect()
    }

    /// Recorded from `claude -p --output-format stream-json`: a background
    /// command and a sub-agent at once, then an agent in the background,
    /// whose end starts a turn by itself.
    #[test]
    fn sub_agents_stream_under_their_tool() {
        let mut t = Transcript::new("/tmp/shed-cap-b");
        for v in lines("agents.jsonl") {
            t.apply(&v);
        }
        // The sub-agents' prompts and answers are theirs, never the conversation's.
        assert!(!t.entries.iter().any(|e| matches!(e, Entry::User { .. })), "{:#?}", t.entries);
        let said: Vec<&str> = t.entries.iter().filter_map(|e| if let Entry::Assistant { text, .. } = e { Some(text.as_str()) } else { None }).collect();
        assert_eq!(said, ["Done.", "Done."]);
        let count = tool(&t, "Agent: Count files");
        let run = count.run.as_ref().unwrap();
        assert_eq!((run.state, run.background, run.agent.as_str(), run.task.as_str()), (RunState::Done, false, "general-purpose", "a20552e0196a946d5"));
        assert!(matches!(&run.steps[..], [Entry::Tool(ls), ..] if ls.summary == "$ ls -a" && ls.result.as_deref() == Some(".\n..")), "{:#?}", run.steps);
        assert_eq!((run.tools, run.report.as_str()), (1, "2"));
        assert_eq!(count.result.as_deref(), Some("2"), "the report, without Claude Code's framing");
        let echo = tool(&t, "Agent: Echo twice").run.as_ref().unwrap();
        assert_eq!((echo.state, echo.background, echo.tools), (RunState::Done, true, 2));
        let steps: Vec<String> = echo
            .steps
            .iter()
            .map(|e| match e {
                Entry::Tool(x) => x.summary.clone(),
                Entry::Assistant { text, .. } => text.clone(),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(steps, ["$ echo one", "$ echo two", "done"]);
        assert!(echo.ended >= echo.started && echo.started > 0);
        let sleep = tool(&t, "$ sleep 4").run.as_ref().unwrap();
        assert_eq!((sleep.state, sleep.task.as_str()), (RunState::Done, "bcxut3v3y"));
        assert_eq!(notes(&t), ["Background command \"Sleep 4 seconds in background\" completed (exit code 0)", "Agent “Echo twice” finished"]);
        assert_eq!(t.status.state, State::Idle);
    }

    /// An agent working in the background while the conversation goes on
    /// sends its own entry alone, not everything after it.
    #[test]
    fn a_background_agent_changes_its_entry_alone() {
        let mut t = Transcript::new("/tmp");
        t.apply(&json!({"type": "assistant", "message": {"id": "m1", "content": [{"type": "tool_use", "id": "ag", "name": "Agent", "input": {"description": "Look around", "prompt": "look", "run_in_background": true}}]}}));
        t.apply(&json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "ag", "content": [{"type": "text", "text": "Async agent launched successfully.\nagentId: a1 (internal ID)"}]}]}, "tool_use_result": {"isAsync": true, "status": "async_launched", "agentId": "a1"}}));
        t.apply(&json!({"type": "assistant", "message": {"id": "m2", "content": [{"type": "text", "text": "Meanwhile, here."}]}}));
        t.take_update();
        t.apply(&json!({"type": "assistant", "parent_tool_use_id": "ag", "message": {"id": "s1", "content": [{"type": "tool_use", "id": "r1", "name": "Read", "input": {"file_path": "/tmp/a.rs"}}]}}));
        assert_eq!(t.take_update(), (vec![0], None));
        let Entry::Tool(Tool { run: Some(run), result, .. }) = t.entries[0].clone() else { panic!() };
        assert_eq!((run.last.as_str(), run.tools, run.running(), result.as_deref()), ("Read a.rs", 1, true, Some("Started in the background")));
        assert_eq!(t.runs.get("a1"), Some(&0));
    }

    /// Recorded: a wake-up, a job every minute, a watch, the job firing
    /// and deleting itself, the wake-up firing.
    #[test]
    fn timers_come_and_go_with_the_stream() {
        let mut t = Transcript::new("/tmp/shed-cap-c");
        let mut seen_all = false;
        for v in lines("timers.jsonl") {
            t.clock = 1_795_164_880;
            t.apply(&v);
            let kinds: Vec<TimerKind> = t.status.timers.iter().map(|x| x.kind).collect();
            if kinds == [TimerKind::Wakeup, TimerKind::Job, TimerKind::Watch] {
                seen_all = true;
                let [wake, job, watch] = &t.status.timers[..] else { unreachable!() };
                assert_eq!((wake.what.as_str(), wake.at, wake.prompt.as_str()), ("check back on the test", 1_795_164_960, "Say: woke up"));
                assert_eq!((job.id.as_str(), job.every.as_str()), ("2798617b", "Every minute"));
                // Sixty seconds from its result's time.
                assert_eq!((watch.id.as_str(), watch.what.as_str(), watch.at), ("bnm3gneqv", "three events", 1_795_164_881 + 60));
            }
        }
        assert!(seen_all);
        assert!(t.status.timers.is_empty(), "{:?}", t.status.timers);
        let n = notes(&t);
        assert_eq!(n.iter().filter(|x| *x == "The watch “three events” saw something").count(), 2, "{n:#?}");
        assert!(n.contains(&"Monitor \"three events\" stream ended".to_string()), "{n:#?}");
        assert!(n.contains(&"Scheduled job ran: Say tick, then delete this cron job with CronDelete.".to_string()), "{n:#?}");
        assert_eq!(n.last().map(String::as_str), Some("Woke up: check back on the test"));
        assert_eq!(tool(&t, "Schedule: every minute · Say tick, then delete this cron job with CronDelete.").name, "CronCreate");
        assert_eq!(tool(&t, "Cancel the scheduled job “Say tick, then delete this cron job with CronDelete.”").name, "CronDelete");
        assert_eq!(t.status.state, State::Idle);
    }

    #[test]
    fn a_turn_by_itself_is_working_and_says_why() {
        let mut t = Transcript::new("/tmp");
        t.clock = 100;
        t.apply(&json!({"type": "command_lifecycle", "command_uuid": "c1", "state": "started", "wakeup_source": "loop_wakeup", "scheduled_for": 99_000}));
        assert_eq!((t.status.state, t.status.since), (State::Working, Some(100)));
        assert_eq!(notes(&t), ["Woke up"]);
        t.apply(&json!({"type": "system", "subtype": "init", "session_id": "s"}));
        assert_eq!(notes(&t).len(), 1, "the wake-up said why already");
        t.apply(&json!({"type": "result", "subtype": "success"}));
        t.apply(&json!({"type": "system", "subtype": "init", "session_id": "s"}));
        assert_eq!(t.status.state, State::Working);
        assert_eq!(notes(&t).last().map(String::as_str), Some("Claude went on by itself (a background task told it something)"));
    }

    #[test]
    fn claude_codes_notes_are_never_the_users_words() {
        let mut t = Transcript::new("/tmp");
        let user = |content: Value| json!({"type": "user", "message": {"role": "user", "content": content}});
        t.apply(&json!({"type": "user", "isCompactSummary": true, "message": {"content": "This session is being continued from a previous conversation…\n\nSummary: tests"}}));
        t.apply(&user(json!("<command-name>/compact</command-name>\n<command-message>compact</command-message>\n<command-args></command-args>")));
        t.apply(&user(json!("<local-command-stdout>Compacted </local-command-stdout>")));
        t.apply(&user(json!("<bash-input>git status</bash-input>")));
        t.apply(&user(json!("<task-notification>\n<task-id>b1</task-id>\n<status>killed</status>\n<summary>Background command \"Serve\" was stopped after reaching its background time limit</summary>\n</task-notification>")));
        t.apply(&json!({"type": "user", "isMeta": true, "origin": {"kind": "peer"}, "message": {"content": "Another Claude session sent a message:\n<agent-message from=\"a9\">\n[Subagent hand-back] … The report follows:\n  All done.\n</agent-message>"}}));
        t.apply(&json!({"type": "user", "isMeta": true, "turnOrigin": "scheduled", "message": {"content": "check the build"}}));
        t.apply(&json!({"type": "user", "isMeta": true, "message": {"content": [{"type": "text", "text": "Base directory for this skill: /tmp/x"}]}}));
        let build = json!({"type": "attachment", "attachment": {"type": "queued_command", "commandMode": "task-notification", "prompt": "<task-notification><task-id>b2</task-id><status>completed</status><summary>Background command \"Build\" completed (exit code 0)</summary></task-notification>"}});
        t.apply(&build);
        t.apply(&build);
        t.apply(&user(json!("<system-reminder>The date changed.</system-reminder>")));
        let shown: Vec<(String, String)> = t
            .entries
            .iter()
            .map(|e| match e {
                Entry::User { text, .. } => ("user".into(), text.clone()),
                Entry::Note(Note { kind, text, error, .. }) => (format!("{kind:?}{}", if *error { "!" } else { "" }), text.clone()),
                other => ("other".into(), format!("{other:?}")),
            })
            .collect();
        let want: Vec<(String, String)> = [
            ("Compacted", "The conversation was compacted: Claude goes on from a summary of it"),
            ("user", "/compact"),
            ("Command", "Compacted"),
            ("Command", "$ git status"),
            ("Task!", "Background command \"Serve\" was stopped after reaching its background time limit"),
            ("Message", "A message from another Claude session"),
            ("Wakeup", "Woke up: check the build"),
            ("Task", "Background command \"Build\" completed (exit code 0)"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        assert_eq!(shown, want);
        let Entry::Note(peer) = &t.entries[5] else { panic!() };
        assert_eq!(peer.detail, "All done.");
    }

    #[test]
    fn a_stopped_process_ends_its_tasks_and_timers() {
        let mut t = Transcript::new("/tmp");
        t.clock = 50;
        t.apply(&json!({"type": "assistant", "message": {"id": "m1", "content": [
            {"type": "tool_use", "id": "b", "name": "Bash", "input": {"command": "npm run dev", "run_in_background": true}},
            {"type": "tool_use", "id": "w", "name": "ScheduleWakeup", "input": {"delaySeconds": 600, "reason": "check the server", "prompt": "check"}},
        ]}}));
        t.apply(&json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "b", "content": "Command running in background with ID: bx1. Output is being written to: /tmp/o/bx1.output. You will be notified."}]}}));
        t.apply(&json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "w", "content": "Next wakeup scheduled for 10:56:00 (in 600s)."}]}}));
        assert_eq!(t.status.timers.len(), 1);
        assert_eq!(t.status.timers[0].at, 650);
        let Entry::Tool(b) = &t.entries[0] else { panic!() };
        assert_eq!(b.run.as_ref().map(|r| (r.task.as_str(), r.output.as_str())), Some(("bx1", "/tmp/o/bx1.output")));
        let Entry::Tool(w) = &t.entries[1] else { panic!() };
        assert_eq!(w.summary, "Wake up in 10 min: check the server");
        t.stopped();
        assert!(t.status.timers.is_empty());
        let Entry::Tool(b) = &t.entries[0] else { panic!() };
        assert_eq!(b.run.as_ref().unwrap().state, RunState::Stopped);
        assert!(notes(&t)[0].starts_with("Claude Code stopped"));
    }
}
