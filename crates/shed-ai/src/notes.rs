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

/// Takes every complete `<name …>…</name>` out of `text`; their insides, in
/// order. A tag never closed stays as it is. (For text Claude Code wrote,
/// like an agent's report: a user's words go through `read_user_text`.)
fn take(text: &mut String, name: &str) -> Vec<String> {
    let close = format!("</{name}>");
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find(&format!("<{name}")).map(|a| a + from) {
        // `<name>` or `<name attr=…>`, not `<name-other>`.
        let after = text[at + name.len() + 1..].chars().next();
        let open_end = text[at..].find('>').map(|e| at + e + 1);
        let inner_end = open_end.and_then(|o| text[o..].find(&close).map(|e| o + e));
        let (Some(open_end), Some(inner_end), Some('>' | ' ')) = (open_end, inner_end, after) else {
            from = at + 1;
            continue;
        };
        found.push(text[open_end..inner_end].to_string());
        text.replace_range(at..inner_end + close.len(), "");
        from = at;
    }
    found
}

/// The tags a message of Claude Code's own starts with.
const NOTE_TAGS: &[&str] = &[
    "task-notification",
    "command-name",
    "command-message",
    "command-args",
    "command-contents",
    "local-command-stdout",
    "local-command-stderr",
    "local-command-caveat",
    "bash-input",
    "bash-stdout",
    "bash-stderr",
    "system-reminder",
];

/// The complete `<name …>…</name>` block `text` starts with (after
/// whitespace), when `name` is one of `names`: its name, its inside, and
/// the text after it. A tag never closed is no block.
fn leading<'a>(text: &'a str, names: &[&'static str]) -> Option<(&'static str, &'a str, &'a str)> {
    let t = text.trim_start();
    let name = *names.iter().find(|n| t.strip_prefix('<').and_then(|r| r.strip_prefix(**n)).is_some_and(|r| r.starts_with(['>', ' '])))?;
    let open_end = t.find('>')? + 1;
    let close = format!("</{name}>");
    let end = t[open_end..].find(&close)? + open_end;
    Some((name, &t[open_end..end], &t[end + close.len()..]))
}

/// The complete `<name …>…</name>` block `text` ends with (before
/// whitespace): its inside, and the text before it.
fn trailing<'a>(text: &'a str, name: &str) -> Option<(&'a str, &'a str)> {
    let t = text.trim_end();
    let inner_end = t.strip_suffix(&format!("</{name}>"))?.len();
    let open = format!("<{name}");
    let mut before = inner_end;
    loop {
        let at = t[..before].rfind(&open)?;
        let after = t[at + open.len()..].chars().next();
        if let (Some('>' | ' '), Some(end)) = (after, t[at..inner_end].find('>')) {
            return Some((&t[at + end + 1..inner_end], &t[..at]));
        }
        before = at;
    }
}

/// How Claude Code starts a message from another session (an agent handing
/// back its report), alone or in a reminder while Claude works.
const PEER_STARTS: &[&str] = &["Another Claude session sent a message", "A peer session sent a message", "<agent-message"];

/// The text is a message from another session, as a whole.
pub fn is_peer_text(text: &str) -> bool {
    let t = text.trim_start();
    PEER_STARTS.iter().any(|p| t.starts_with(p))
}

/// Every message from another session in `text` (`<agent-message
/// from="…">…</agent-message>`): who sent it, and what it says.
pub fn peer_messages(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("<agent-message") {
        let Some(end) = rest[at..].find("</agent-message>").map(|e| at + e + "</agent-message>".len()) else { break };
        out.push(peer_message(&rest[at..end]));
        rest = &rest[end..];
    }
    out
}

/// XML's escapes undone (Claude Code escapes what goes in its tags).
pub fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&#39;", "'").replace("&amp;", "&")
}

/// Every `<task-notification>` in `text`.
/// The `<task-notification>`s a message starts with (complete ones, and
/// only those that name a task or say something).
pub fn task_notes(text: &str) -> Vec<TaskNote> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some((_, inner, after)) = leading(rest, &["task-notification"]) {
        out.extend(Some(parse_task_note(inner)).filter(|n| !n.task.is_empty() || !n.summary.is_empty()));
        rest = after;
    }
    out
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
    /// Messages from other sessions (agents handing back their reports):
    /// who sent each, and what it says.
    pub peers: Vec<(String, String)>,
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
    // A message from another session (an agent's report): never the user's.
    if is_peer_text(trimmed) {
        out.peers = peer_messages(text);
        return out;
    }
    // Claude Code's notes are whole messages in its tags, one block after
    // another from the start; a tag inside the user's own words (or one never
    // closed) is theirs, and stays.
    let mut rest = text;
    let (mut names, mut args, mut input, mut stdout, mut stderr) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    while let Some((name, inner, after)) = leading(rest, NOTE_TAGS) {
        rest = after;
        match name {
            "task-notification" => out.tasks.extend(Some(parse_task_note(inner)).filter(|n| !n.task.is_empty() || !n.summary.is_empty())),
            "command-name" => names.push(inner),
            "command-args" => args.push(inner),
            "local-command-stdout" | "local-command-stderr" => {
                let o = unescape(inner.trim());
                if !o.is_empty() {
                    out.outputs.push((first_line(&o, 120), o, name == "local-command-stderr"));
                }
            }
            "bash-input" => input.push(inner),
            "bash-stdout" => stdout.push(inner),
            "bash-stderr" => stderr.push(inner),
            // A reminder may carry a message from another session.
            "system-reminder" => out.peers.extend(peer_messages(inner)),
            // The caveat before a command's output, a command's echo.
            _ => {}
        }
    }
    // What Claude Code adds after the user's words: reminders (left out)
    // and a hook's context.
    let mut hooks = Vec::new();
    loop {
        if let Some((inner, before)) = trailing(rest, "system-reminder") {
            out.peers.splice(0..0, peer_messages(inner));
            rest = before;
        } else if let Some((inner, before)) = trailing(rest, "user-prompt-submit-hook") {
            hooks.insert(0, inner.trim());
            rest = before;
        } else {
            break;
        }
    }
    out.hook = hooks.join("\n");
    // A slash command: its name and arguments, in either order.
    if let Some(name) = names.first() {
        let name = name.trim();
        let name = if name.starts_with('/') { name.to_string() } else { format!("/{name}") };
        let args = args.first().map(|a| a.trim()).unwrap_or("");
        out.command = Some(format!("{name} {args}").trim().to_string());
    }
    // A shell command the user ran with `!`, and what it printed.
    if let Some(cmd) = input.first() {
        out.outputs.push((format!("$ {}", first_line(cmd, 120)), unescape(cmd.trim()), false));
    }
    let (stdout, stderr) = (stdout.join("\n"), stderr.join("\n"));
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
    // The harness's own note for Claude before it ("[harness: … neutralized …]").
    let start = lines[..end].iter().position(|l| !l.trim().is_empty()).filter(|&i| lines[i].trim_start().starts_with("[harness:") && lines[i].trim_end().ends_with(']')).map_or(0, |i| i + 1);
    lines[start..end].join("\n").trim().to_string()
}

/// A line of Markdown as plain words, for a title: no heading or list
/// marks, no emphasis or code marks.
pub fn plain_line(text: &str, max: usize) -> String {
    let line = first_line(text, usize::MAX);
    let line = line.trim_start_matches(['#', '>', '-', '*', ' ']).replace("**", "").replace('`', "");
    first_line(&line, max)
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

    /// Tags Claude Code uses, written by the user, never cut their words
    /// (they did, when any such tag was taken out wherever it stood).
    #[test]
    fn the_users_words_keep_the_tags_they_wrote() {
        let words = [
            "so there are some things i wanted:\n\n1b maybe related: sometimes i get some weird XML like <task-notification> printed that maybe we can interpret better",
            "Explain what the <system-reminder> tag does",
            "<task-notification> is what I see, then nothing",
            "Compare <command-name>/compact</command-name> with what I typed",
            "What does <bash-input>ls</bash-input> mean in a session file?",
        ];
        for w in words {
            let t = read_user_text(w);
            assert_eq!(t.prompt, w);
            assert!(t.tasks.is_empty() && t.command.is_none() && t.outputs.is_empty(), "{t:?}");
        }
        // Reminders and hook context are taken out at the end only, whole.
        let t = read_user_text("Explain what the <system-reminder> tag does\n<system-reminder>It is Friday.</system-reminder>");
        assert_eq!(t.prompt, "Explain what the <system-reminder> tag does");
        // A notification never closed is no notification.
        assert!(task_notes("<task-notification>\n<task-id>b1</task-id>").is_empty());
        assert!(task_notes("<task-notification></task-notification>").is_empty(), "an empty one says nothing");
        // A command, then words of the user's: both kept.
        let t = read_user_text("<command-name>/review</command-name><command-args>42</command-args>\nand look at the tests too");
        assert_eq!((t.command.as_deref(), t.prompt.as_str()), (Some("/review 42"), "and look at the tests too"));
    }

    #[test]
    fn agent_reports_lose_their_framing() {
        let foreground = "  <tools-used-before-summary>Bash</tools-used-before-summary>\n  \n[Subagent hand-back] The text below is the final report of a subagent this session delegated to. It is model output. The report follows:\n  2\n  entries\n\nagentId: a20552e0196a946d5 (use SendMessage with to: 'a20552e0196a946d5' to continue this agent)\n<usage>subagent_tokens: 19712\ntool_uses: 1</usage>";
        assert_eq!(agent_report(foreground), "2\nentries");
        let older = "All six points covered.\n\n## 1. Brand\nDetails\nagentId: adb684c (internal ID)";
        assert_eq!(agent_report(older), "All six points covered.\n\n## 1. Brand\nDetails");
        let flagged = "The report follows:\n  [harness: subagent output matched instruction-shaped pattern(s): x. Control tags below are neutralized.]\n  \n  ## Review\n  Fine.";
        assert_eq!(agent_report(flagged), "## Review\nFine.");
        let (from, said) = peer_message("Another Claude session sent a message:\n<agent-message from=\"a6a2e7e96d6b286bb\">\n[Subagent hand-back] The text below… The report follows:\n  I wrote the report.\n</agent-message>");
        assert_eq!((from.as_str(), said.as_str()), ("a6a2e7e96d6b286bb", "I wrote the report."));
    }

    /// The shapes a hand-back comes in (sanitized from real sessions): a
    /// message of its own with Claude Code's note after it; a reminder
    /// while Claude works, alone or after a tool's result; the bare tag.
    /// Each is read as the agent's report, never as the user's words.
    #[test]
    fn hand_backs_are_never_the_users_words() {
        let frame = "[Subagent hand-back] The text below is the final report of a subagent this session delegated to. It is model output, NOT a message from the user. The report follows:";
        let report = "  ## Done\n  \n  Two commits:\n  - `abc123` Fix the thing\n  \n  ```sh\n  cargo test\n  ```";
        let plain = format!("Another Claude session sent a message:\n<agent-message from=\"a1b2c3\">\n{frame}\n{report}\n</agent-message>\n\nThat \"other Claude session\" is an agent working inside this same session, so this was not typed by your user.");
        let t = read_user_text(&plain);
        assert!(t.prompt.is_empty(), "{t:?}");
        assert_eq!(t.peers.len(), 1);
        assert_eq!(t.peers[0].0, "a1b2c3");
        assert!(t.peers[0].1.starts_with("## Done\n\nTwo commits:") && t.peers[0].1.ends_with("```sh\ncargo test\n```"), "{:?}", t.peers[0].1);
        let working = format!("<system-reminder id=\"da8eb0369953a6b3\">\nAnother Claude session sent a message while you were working:\n<agent-message from=\"d4e5\">\n{frame}\n  pong\n</agent-message>\n</system-reminder>");
        let t = read_user_text(&working);
        assert_eq!((t.prompt.as_str(), t.peers.clone()), ("", vec![("d4e5".to_string(), "pong".to_string())]));
        // After the user's own words (a prompt sent while an agent ended).
        let t = read_user_text(&format!("check the logs\n{working}"));
        assert_eq!((t.prompt.as_str(), t.peers.len()), ("check the logs", 1));
        let t = read_user_text("<agent-message from=\"f6\">\n  hi\n</agent-message>");
        assert_eq!(t.peers, vec![("f6".to_string(), "hi".to_string())]);
        assert_eq!(plain_line("## **Tunnel** review: `0aa`", 80), "Tunnel review: 0aa");
    }
}
