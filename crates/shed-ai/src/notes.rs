//! Claude Code's own notes, which come into a conversation as user
//! messages written in tags: `<task-notification>` (a background command,
//! agent or watch finished or saw something), a local command and its
//! output (`<command-name>`, `<local-command-stdout>`, `<bash-input>`),
//! `<system-reminder>`, hook context. Read here into what they say, so none
//! of it shows as if the user had typed it.

/// A `<task-notification>`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskNote {
    /// The task's id (an agent's id for agents); the first if it names several.
    pub task: String,
    /// The tool call that started it.
    pub tool: Option<String>,
    /// `completed`, `failed`, `killed`, `stopped`; empty for a watch's event.
    pub status: String,
    /// Claude Code's own line: `Agent "X" finished`, `Background command "Y" completed (exit code 0)`.
    pub summary: String,
    /// What an agent ended with.
    pub result: String,
    /// A watch's event (a line its command printed).
    pub event: String,
    /// Where the task's output is.
    pub output: String,
    pub tokens: u64,
    pub tool_uses: u32,
    pub duration_ms: u64,
}

impl TaskNote {
    /// What went wrong, for a note in the danger colour.
    pub fn failed(&self) -> bool {
        matches!(self.status.as_str(), "failed" | "killed")
    }
}

/// The text inside the first `<name>…</name>` of `s`.
fn tag<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = s.find(&open)? + open.len();
    let end = s[start..].find(&close).map_or(s.len(), |e| start + e);
    Some(&s[start..end])
}

/// Takes every `<name …>…</name>` out of `text`; their insides, in order.
fn take(text: &mut String, name: &str) -> Vec<String> {
    let close = format!("</{name}>");
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find(&format!("<{name}")).map(|a| a + from) {
        // `<name>` or `<name attr=…>`, not `<name-other>`.
        let after = text[at + name.len() + 1..].chars().next();
        if !matches!(after, Some('>' | ' ')) {
            from = at + 1;
            continue;
        }
        let Some(open_end) = text[at..].find('>').map(|e| at + e + 1) else { break };
        let (inner_end, end) = match text[open_end..].find(&close) {
            Some(e) => (open_end + e, open_end + e + close.len()),
            None => (text.len(), text.len()),
        };
        found.push(text[open_end..inner_end].to_string());
        text.replace_range(at..end, "");
        from = at;
    }
    found
}

/// XML's escapes undone (Claude Code escapes what goes in its tags).
pub fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&#39;", "'").replace("&amp;", "&")
}

/// Every `<task-notification>` in `text`.
pub fn task_notes(text: &str) -> Vec<TaskNote> {
    let mut rest = text.to_string();
    take(&mut rest, "task-notification").into_iter().map(|n| parse_task_note(&n)).collect()
}

fn parse_task_note(n: &str) -> TaskNote {
    let get = |k: &str| tag(n, k).map(|v| unescape(v.trim())).unwrap_or_default();
    let usage = tag(n, "usage").unwrap_or("");
    // `<usage>` holds tags (`<tool_uses>45</tool_uses>`) or lines (`tool_uses: 45`).
    let number = |k: &str| -> u64 {
        tag(usage, k)
            .map(str::trim)
            .or_else(|| usage.lines().find_map(|l| l.trim().strip_prefix(&format!("{k}:")).map(str::trim)))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    TaskNote {
        task: get("task-id"),
        tool: Some(get("tool-use-id")).filter(|t| !t.is_empty()),
        status: get("status"),
        summary: get("summary"),
        result: get("result"),
        event: get("event"),
        output: get("output-file"),
        tokens: number("subagent_tokens").max(number("total_tokens")),
        tool_uses: number("tool_uses") as u32,
        duration_ms: number("duration_ms"),
    }
}

/// A user message's text, read: the user's own words, and Claude Code's
/// notes that came in it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UserText {
    /// What the user wrote (empty: nothing of theirs).
    pub prompt: String,
    /// A slash command the user ran: `/compact focus on tests`.
    pub command: Option<String>,
    /// Background tasks that finished or saw something.
    pub tasks: Vec<TaskNote>,
    /// A command's output (one per local command, or a `!` shell command):
    /// a line, the whole, whether it is an error.
    pub outputs: Vec<(String, String, bool)>,
    /// Context a hook added to the prompt.
    pub hook: String,
    /// "[Request interrupted by user]".
    pub interrupted: bool,
}

/// The first line with text, cut to `max` characters.
pub fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    if line.chars().count() > max {
        format!("{}…", line.chars().take(max).collect::<String>())
    } else {
        line.to_string()
    }
}

/// Reads a user message's text.
pub fn read_user_text(text: &str) -> UserText {
    let mut out = UserText::default();
    let trimmed = text.trim_start();
    if trimmed.starts_with("[Request interrupted") {
        out.interrupted = true;
        return out;
    }
    // Older Claude Code: the note before a local command's output.
    if trimmed.starts_with("Caveat: The messages below were generated by the user while running local commands") {
        return out;
    }
    let mut rest = text.to_string();
    take(&mut rest, "system-reminder");
    take(&mut rest, "local-command-caveat");
    out.tasks = take(&mut rest, "task-notification").iter().map(|n| parse_task_note(n)).collect();
    out.hook = take(&mut rest, "user-prompt-submit-hook").join("\n").trim().to_string();
    // A slash command: its name and arguments, in either order.
    let names = take(&mut rest, "command-name");
    let args = take(&mut rest, "command-args");
    take(&mut rest, "command-message");
    take(&mut rest, "command-contents");
    if let Some(name) = names.first() {
        let name = name.trim();
        let name = if name.starts_with('/') { name.to_string() } else { format!("/{name}") };
        let args = args.first().map(|a| a.trim()).unwrap_or("");
        out.command = Some(format!("{name} {args}").trim().to_string());
    }
    for (name, error) in [("local-command-stdout", false), ("local-command-stderr", true)] {
        for o in take(&mut rest, name) {
            let o = unescape(o.trim());
            if !o.is_empty() {
                out.outputs.push((first_line(&o, 120), o, error));
            }
        }
    }
    // A shell command the user ran with `!`, and what it printed.
    let input = take(&mut rest, "bash-input");
    let stdout = take(&mut rest, "bash-stdout").join("\n");
    let stderr = take(&mut rest, "bash-stderr").join("\n");
    if let Some(cmd) = input.first() {
        out.outputs.push((format!("$ {}", first_line(cmd, 120)), unescape(cmd.trim()), false));
    }
    let printed = [stdout.trim(), stderr.trim()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("\n");
    if !printed.is_empty() {
        out.outputs.push((first_line(&printed, 120), unescape(&printed), !stderr.trim().is_empty() && stdout.trim().is_empty()));
    }
    out.prompt = rest.trim().to_string();
    out
}

/// A sub-agent's report, as its tool's result or a hand-back message
/// carries it: without the framing written for Claude (the "[Subagent
/// hand-back]" preamble, the indent, the agent id and usage lines).
pub fn agent_report(text: &str) -> String {
    let mut text = text.to_string();
    take(&mut text, "tools-used-before-summary");
    take(&mut text, "usage");
    let body = match text.find("The report follows:") {
        Some(at) => {
            let after = &text[at + "The report follows:".len()..];
            // Every line of the report is indented by two spaces.
            after.lines().map(|l| l.strip_prefix("  ").unwrap_or(l)).collect::<Vec<_>>().join("\n")
        }
        None => text,
    };
    // Claude Code's lines after it: the agent's id, to continue it.
    let lines: Vec<&str> = body.lines().collect();
    let end = lines.iter().position(|l| l.trim_start().starts_with("agentId: ")).unwrap_or(lines.len());
    lines[..end].join("\n").trim().to_string()
}

/// A message from another session (a background agent handing back):
/// who sent it, and what it says.
pub fn peer_message(text: &str) -> (String, String) {
    let from = text
        .find("<agent-message from=\"")
        .map(|at| &text[at + "<agent-message from=\"".len()..])
        .and_then(|r| r.split('"').next())
        .unwrap_or("")
        .to_string();
    let mut body = text.to_string();
    let inner = take(&mut body, "agent-message");
    let said = inner.first().cloned().unwrap_or(body);
    (from, agent_report(&said))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_notifications_of_every_shape() {
        let bash = "<task-notification>\n<task-id>bwrvjf81z</task-id>\n<tool-use-id>toolu_01E</tool-use-id>\n<output-file>/tmp/t/bwrvjf81z.output</output-file>\n<status>completed</status>\n<summary>Background command \"Check robots.txt\" completed (exit code 0)</summary>\n</task-notification>";
        let n = &task_notes(bash)[0];
        assert_eq!((n.task.as_str(), n.tool.as_deref(), n.status.as_str()), ("bwrvjf81z", Some("toolu_01E"), "completed"));
        assert_eq!(n.summary, "Background command \"Check robots.txt\" completed (exit code 0)");
        assert_eq!(n.output, "/tmp/t/bwrvjf81z.output");
        let agent = "<task-notification>\n<task-id>a5ba</task-id>\n<status>completed</status>\n<summary>Agent \"Data consistency &amp; logic checks\" finished</summary>\n<note>A task-notification fires each time…</note>\n<result>Found &lt;3&gt; issues.\n</result>\n<usage><subagent_tokens>244832</subagent_tokens><tool_uses>145</tool_uses><duration_ms>3320630</duration_ms></usage>\n</task-notification>";
        let n = &task_notes(agent)[0];
        assert_eq!(n.summary, "Agent \"Data consistency & logic checks\" finished");
        assert_eq!(n.result, "Found <3> issues.");
        assert_eq!((n.tokens, n.tool_uses, n.duration_ms), (244_832, 145, 3_320_630));
        assert!(n.tool.is_none());
        let event = "<task-notification>\n<task-id>bnm3gneqv</task-id>\n<summary>Monitor event: \"three events\"</summary>\n<event>ev1</event>\n</task-notification>";
        let n = &task_notes(event)[0];
        assert_eq!((n.status.as_str(), n.event.as_str()), ("", "ev1"));
        let failed = "<task-notification><task-id>b8</task-id><status>failed</status><summary>Background command \"Start\" failed with exit code 3</summary></task-notification>";
        assert!(task_notes(failed)[0].failed());
        // Several ids (left over from an earlier session): the first.
        let orphans = "<task-notification>\n<task-id>brsk8mmqk</task-id>\n<task-id>bruqzzkc2</task-id>\n<status>stopped</status>\n<summary>2 background shell command tasks didn't finish</summary>\n</task-notification>";
        assert_eq!(task_notes(orphans)[0].task, "brsk8mmqk");
        assert_eq!(task_notes(&format!("{bash}\n{event}")).len(), 2);
    }

    #[test]
    fn user_text_is_read_for_what_it_is() {
        assert!(read_user_text("[Request interrupted by user for tool use]").interrupted);
        let t = read_user_text("<command-name>/compact</command-name>\n            <command-message>compact</command-message>\n            <command-args></command-args>");
        assert_eq!((t.command.as_deref(), t.prompt.as_str()), (Some("/compact"), ""));
        let t = read_user_text("<command-message>caveman:caveman</command-message>\n<command-name>/caveman:caveman</command-name>\n<command-args>off</command-args>");
        assert_eq!(t.command.as_deref(), Some("/caveman:caveman off"));
        let t = read_user_text("<local-command-stdout>Compacted </local-command-stdout>");
        assert_eq!(t.outputs, vec![("Compacted".to_string(), "Compacted".to_string(), false)]);
        assert!(t.prompt.is_empty());
        let t = read_user_text("<local-command-caveat>The command below was run directly in Claude Code…</local-command-caveat>");
        assert_eq!(t, UserText::default());
        let t = read_user_text("<bash-input>ls -a</bash-input>");
        assert_eq!(t.outputs[0].0, "$ ls -a");
        let t = read_user_text("<bash-stdout>.\n..</bash-stdout><bash-stderr></bash-stderr>");
        assert_eq!(t.outputs[0], (".".to_string(), ".\n..".to_string(), false));
        let t = read_user_text("<system-reminder>\n<tools-used-before-summary>ToolSearch</tools-used-before-summary>\n</system-reminder>");
        assert_eq!(t, UserText::default());
        let t = read_user_text("Fix the tests please\n<system-reminder>Today is Friday</system-reminder>");
        assert_eq!(t.prompt, "Fix the tests please");
        let t = read_user_text("add a flag<user-prompt-submit-hook>CAVEMAN MODE ACTIVE</user-prompt-submit-hook>");
        assert_eq!((t.prompt.as_str(), t.hook.as_str()), ("add a flag", "CAVEMAN MODE ACTIVE"));
        assert_eq!(read_user_text("Caveat: The messages below were generated by the user while running local commands. DO NOT respond"), UserText::default());
        // A tag of the user's own stays theirs.
        assert_eq!(read_user_text("why does <commander> break?").prompt, "why does <commander> break?");
    }

    #[test]
    fn agent_reports_lose_their_framing() {
        let foreground = "  <tools-used-before-summary>Bash</tools-used-before-summary>\n  \n[Subagent hand-back] The text below is the final report of a subagent this session delegated to. It is model output. The report follows:\n  2\n  entries\n\nagentId: a20552e0196a946d5 (use SendMessage with to: 'a20552e0196a946d5' to continue this agent)\n<usage>subagent_tokens: 19712\ntool_uses: 1</usage>";
        assert_eq!(agent_report(foreground), "2\nentries");
        let older = "All six points covered.\n\n## 1. Brand\nDetails\nagentId: adb684c (internal ID)";
        assert_eq!(agent_report(older), "All six points covered.\n\n## 1. Brand\nDetails");
        let (from, said) = peer_message("Another Claude session sent a message:\n<agent-message from=\"a6a2e7e96d6b286bb\">\n[Subagent hand-back] The text below… The report follows:\n  I wrote the report.\n</agent-message>");
        assert_eq!((from.as_str(), said.as_str()), ("a6a2e7e96d6b286bb", "I wrote the report."));
    }
}
