//! A folder's Claude Code sessions, as Claude Code saves them:
//! `~/.claude/projects/<folder, its separators as dashes>/<id>.jsonl`.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Transcript;

/// Sessions that Shed's own `> request` prompt made start with this.
pub const SHED_PROMPT_PREAMBLE: &str = "You are the AI prompt of Shed";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    /// Claude Code's own title for it, else the first prompt.
    pub title: String,
    /// Unix seconds.
    pub modified: u64,
    pub branch: Option<String>,
    /// How many prompts it has.
    pub prompts: usize,
    /// The session it came from, when it goes on from one: continued (Claude
    /// Code starts a new file for that) or branched (an earlier message edited).
    pub parent: Option<String>,
    /// How many of the parent's prompts came before it.
    pub from: usize,
    /// It went on from before the parent's end (an earlier message edited),
    /// rather than from its end (continued).
    pub branched: bool,
    /// How deep under its first ancestor it is listed.
    pub depth: usize,
    /// The whole conversation's prompts, once each, with those of the
    /// sessions it goes on from.
    pub whole: usize,
}

/// What a session file says about where it comes from.
#[derive(Default)]
struct Lineage {
    /// When its first message was written (RFC 3339, so it sorts).
    started: String,
    /// The message its first message follows, in another file when continued.
    first_parent: Option<String>,
    /// Every message id: its place among the file's messages, and how many
    /// prompts were made up to it.
    ids: std::collections::HashMap<String, (usize, usize)>,
    /// The prompts' message ids, in order.
    prompt_ids: Vec<String>,
}

/// Only what lineage needs from a record, without the rest of it.
#[derive(Deserialize)]
struct Record {
    uuid: Option<String>,
    #[serde(rename = "parentUuid")]
    parent: Option<String>,
    timestamp: Option<String>,
}

/// Where Claude Code keeps its settings and sessions.
fn claude_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return dir.into();
    }
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".claude")
}

/// The folder Claude Code keeps a working folder's sessions in.
pub fn sessions_dir(folder: &Path) -> PathBuf {
    let name: String = folder.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    claude_dir().join("projects").join(name)
}

/// A folder's sessions, newest first, without the ones Shed's `>` prompt
/// made and without empty ones.
pub fn list_sessions(folder: &Path) -> Vec<SessionInfo> {
    let Ok(dir) = std::fs::read_dir(sessions_dir(folder)) else { return Vec::new() };
    let found: Vec<(SessionInfo, Lineage)> = dir
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| read_info(&e.path()))
        .collect();
    as_tree(link(found))
}

/// Finds each session's parent: the one holding the message its first
/// message follows (continued), else the older one holding its first prompt
/// (branched, with the prompts both have).
fn link(found: Vec<(SessionInfo, Lineage)>) -> Vec<SessionInfo> {
    let parents = parents(&found);
    let mut out: Vec<SessionInfo> = found
        .iter()
        .zip(&parents)
        .map(|((info, _), p)| {
            let mut info = info.clone();
            if let Some(p) = p {
                let parent = &found[p.parent].0;
                info.parent = Some(parent.id.clone());
                info.from = p.prompts;
                info.branched = p.prompts < parent.prompts;
            }
            info
        })
        .collect();
    // The whole conversation's prompts, once each: its own, and those of the
    // sessions it goes on from, up to where it does.
    for (i, info) in out.iter_mut().enumerate() {
        let mut seen: std::collections::HashSet<&String> = found[i].1.prompt_ids.iter().collect();
        let mut at = i;
        let mut hops = 0;
        while let Some(p) = &parents[at]
            && hops < 64
        {
            let pl = &found[p.parent].1;
            seen.extend(pl.prompt_ids.iter().filter(|id| pl.ids.get(*id).is_some_and(|(n, _)| *n <= p.cut)));
            at = p.parent;
            hops += 1;
        }
        info.whole = seen.len();
    }
    out
}

/// Where a session goes on from another.
struct Joint {
    /// Its index among the sessions read.
    parent: usize,
    /// The last of the parent's messages that is part of it (by place).
    cut: usize,
    /// How many of the parent's prompts come before it goes on.
    prompts: usize,
}

/// Each session's parent: the one holding the message its first message
/// follows, else an older one holding its first prompt; and where in it the
/// session goes on (past every message both have).
fn parents(found: &[(SessionInfo, Lineage)]) -> Vec<Option<Joint>> {
    found
        .iter()
        .enumerate()
        .map(|(i, (_, lin))| {
            let others = || found.iter().enumerate().filter(move |(j, _)| *j != i);
            let parent = lin
                .first_parent
                .as_ref()
                .and_then(|p| others().find(|(_, (_, ol))| ol.ids.contains_key(p)))
                .or_else(|| {
                    let first = lin.prompt_ids.first()?;
                    others()
                        .filter(|(_, (_, ol))| ol.started < lin.started && ol.ids.contains_key(first))
                        .max_by_key(|(_, (_, ol))| lin.ids.keys().filter(|k| ol.ids.contains_key(*k)).count())
                })?;
            let (j, (_, pl)) = parent;
            let shared = lin.ids.keys().filter_map(|k| pl.ids.get(k)).copied();
            let follows = lin.first_parent.as_ref().and_then(|f| pl.ids.get(f)).copied();
            let (cut, prompts) = shared.chain(follows).max_by_key(|(n, _)| *n)?;
            Some(Joint { parent: j, cut, prompts })
        })
        .collect()
}

/// Each session under the one it came from, the most recently changed
/// first at each level (a family by its most recent).
fn as_tree(sessions: Vec<SessionInfo>) -> Vec<SessionInfo> {
    let ids: std::collections::HashSet<String> = sessions.iter().map(|s| s.id.clone()).collect();
    let latest = |s: &SessionInfo| -> u64 {
        // A session and everything under it.
        let mut newest = s.modified;
        let mut stack = vec![s.id.clone()];
        let mut seen = std::collections::HashSet::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            for c in sessions.iter().filter(|c| c.parent.as_deref() == Some(&id)) {
                newest = newest.max(c.modified);
                stack.push(c.id.clone());
            }
        }
        newest
    };
    let mut out = Vec::with_capacity(sessions.len());
    let mut placed = std::collections::HashSet::new();
    fn place(s: &SessionInfo, depth: usize, all: &[SessionInfo], latest: &dyn Fn(&SessionInfo) -> u64, placed: &mut std::collections::HashSet<String>, out: &mut Vec<SessionInfo>) {
        if !placed.insert(s.id.clone()) {
            return;
        }
        out.push(SessionInfo { depth, ..s.clone() });
        let mut children: Vec<&SessionInfo> = all.iter().filter(|c| c.parent.as_deref() == Some(&s.id)).collect();
        children.sort_by_key(|c| std::cmp::Reverse(latest(c)));
        for c in children {
            place(c, depth + 1, all, latest, placed, out);
        }
    }
    let mut roots: Vec<&SessionInfo> = sessions.iter().filter(|s| s.parent.as_ref().is_none_or(|p| !ids.contains(p))).collect();
    roots.sort_by_key(|s| std::cmp::Reverse(latest(s)));
    for r in roots {
        place(r, 0, &sessions, &latest, &mut placed, &mut out);
    }
    // Any left (a loop of parents): at the top.
    for s in &sessions {
        place(s, 0, &sessions, &latest, &mut placed, &mut out);
    }
    out
}

fn read_info(path: &Path) -> Option<(SessionInfo, Lineage)> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let id = path.file_stem()?.to_string_lossy().into_owned();
    let file = std::fs::File::open(path).ok()?;
    let mut first_prompt: Option<String> = None;
    let mut title: Option<String> = None;
    // A name given to it (Claude Code's /rename, or Shed's Rename), which
    // wins over the title Claude Code makes up; the last one counts.
    let mut custom: Option<String> = None;
    let mut branch = None;
    let mut prompts = 0;
    let mut lineage = Lineage::default();
    let mut messages = 0;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        // Messages (and only they) have ids: where each is, by prompt.
        if line.contains("\"uuid\"")
            && let Ok(rec) = serde_json::from_str::<Record>(&line)
            && let Some(uuid) = rec.uuid
        {
            if lineage.started.is_empty() {
                lineage.started = rec.timestamp.unwrap_or_default();
                lineage.first_parent = rec.parent;
            }
            lineage.ids.entry(uuid).or_insert((messages, prompts));
            messages += 1;
        }
        // Most lines are neither: look before parsing.
        let is_title = line.contains("\"ai-title\"") || line.contains("\"summary\"") || line.contains("\"custom-title\"");
        let is_user = line.contains("\"user\"") && !line.contains("\"tool_result\"");
        if !is_title && !is_user {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        match v.get("type").and_then(Value::as_str) {
            Some("ai-title") => title = v.get("aiTitle").and_then(Value::as_str).map(String::from),
            Some("custom-title") => custom = v.get("customTitle").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).map(String::from),
            Some("summary") => title = title.or_else(|| v.get("summary").and_then(Value::as_str).map(String::from)),
            Some("user") if v.get("isSidechain").and_then(Value::as_bool) != Some(true) && v.get("isMeta").and_then(Value::as_bool) != Some(true) => {
                let text = match v.get("message").and_then(|m| m.get("content")) {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Array(blocks)) => blocks.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join(" "),
                    _ => continue,
                };
                if text.is_empty() || text.starts_with('<') || text.starts_with("[Request interrupted") {
                    continue;
                }
                prompts += 1;
                if let Some(uuid) = v.get("uuid").and_then(Value::as_str) {
                    if let Some(e) = lineage.ids.get_mut(uuid) {
                        e.1 = prompts;
                    }
                    lineage.prompt_ids.push(uuid.to_string());
                }
                if first_prompt.is_none() {
                    first_prompt = Some(text);
                    branch = v.get("gitBranch").and_then(Value::as_str).filter(|b| !b.is_empty()).map(String::from);
                }
            }
            _ => {}
        }
    }
    let first = first_prompt?;
    if first.starts_with(SHED_PROMPT_PREAMBLE) {
        return None;
    }
    let title = custom.or(title).unwrap_or_else(|| {
        let line = first.lines().next().unwrap_or("").trim();
        let mut short: String = line.chars().take(80).collect();
        if line.chars().count() > 80 {
            short.push('…');
        }
        short
    });
    Some((SessionInfo { id, title, modified, branch, prompts, ..Default::default() }, lineage))
}

/// Names a session, as Claude Code's `/rename` does: a `custom-title`
/// record at its end, so Claude Code shows the name too. An empty name goes
/// back to the title Claude Code made up.
pub fn rename_session(folder: &Path, id: &str, name: &str) -> std::io::Result<()> {
    rename_session_in(&sessions_dir(folder), id, name)
}

fn rename_session_in(dir: &Path, id: &str, name: &str) -> std::io::Result<()> {
    use std::io::Write;
    let path = dir.join(format!("{id}.jsonl"));
    if !path.is_file() {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no such session"));
    }
    let record = serde_json::json!({"type": "custom-title", "customTitle": name.trim(), "sessionId": id});
    // On a line of its own, even after a last line with no line break.
    let ends_clean = {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = std::fs::File::open(&path)?;
        let len = f.metadata()?.len();
        let mut last = [0u8; 1];
        len == 0 || (f.seek(SeekFrom::End(-1)).is_ok() && f.read_exact(&mut last).is_ok() && last[0] == b'\n')
    };
    let mut file = std::fs::OpenOptions::new().append(true).open(path)?;
    file.write_all(format!("{}{record}\n", if ends_clean { "" } else { "\n" }).as_bytes())
}

/// A saved session as a conversation; its prompts' pictures are kept in
/// `picture_dir`, to show them.
pub fn load_session(folder: &Path, id: &str, picture_dir: Option<std::path::PathBuf>) -> Option<Transcript> {
    load_from(&sessions_dir(folder), folder, id, picture_dir)
}

fn load_from(dir: &Path, folder: &Path, id: &str, picture_dir: Option<std::path::PathBuf>) -> Option<Transcript> {
    if !dir.join(format!("{id}.jsonl")).is_file() {
        return None;
    }
    let mut t = Transcript::new(&folder.to_string_lossy());
    t.picture_dir = picture_dir;
    // The sessions it goes on from first, each up to where it does; a
    // message copied from one file to the next is read once.
    let mut seen = std::collections::HashSet::new();
    for (part, cut) in conversation(dir, id) {
        let Ok(file) = std::fs::File::open(dir.join(format!("{part}.jsonl"))) else { continue };
        t.reading_session = Some(part.clone());
        // Messages counted as `read_info` counts them, so `cut` points the same.
        let mut at = 0;
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            if line.contains("\"uuid\"")
                && let Ok(rec) = serde_json::from_str::<Record>(&line)
                && let Some(uuid) = rec.uuid
            {
                if cut.is_some_and(|c| at > c) {
                    break;
                }
                at += 1;
                if !seen.insert(uuid) {
                    continue;
                }
            }
            // Messages, and what came while Claude worked (notifications).
            if !(line.contains("\"user\"") || line.contains("\"assistant\"") || line.contains("\"queued_command\"")) {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                t.apply(&v);
            }
        }
        // Its sub-agents, as steps of their Agent tools.
        t.read_subagents(&dir.join(&part).join("subagents"));
    }
    t.reading_session = None;
    t.status.session = Some(id.to_string());
    t.status.state = crate::State::Idle;
    t.status.since = None;
    t.finish_loading();
    Some(t)
}

/// The session files a conversation is read from, oldest first: each with
/// the place of its last message that belongs (`None`: all, the last one).
fn conversation(dir: &Path, id: &str) -> Vec<(String, Option<usize>)> {
    let found: Vec<(SessionInfo, Lineage)> = std::fs::read_dir(dir)
        .map(|d| d.filter_map(Result::ok).filter(|e| e.path().extension().is_some_and(|x| x == "jsonl")).filter_map(|e| read_info(&e.path())).collect())
        .unwrap_or_default();
    let parents = parents(&found);
    let Some(mut at) = found.iter().position(|(s, _)| s.id == id) else { return vec![(id.to_string(), None)] };
    let mut chain = vec![(id.to_string(), None)];
    while let Some(p) = &parents[at]
        && chain.len() < 64
    {
        chain.push((found[p.parent].0.id.clone(), Some(p.cut)));
        at = p.parent;
    }
    chain.reverse();
    chain
}


#[cfg(test)]
mod tests {
    use super::*;

    /// A session file of prompts and answers: (id, parent) per message,
    /// prompts where the text is given.
    fn write_session(dir: &Path, id: &str, started: &str, messages: &[(&str, Option<&str>, Option<&str>)]) -> PathBuf {
        let mut lines = vec![format!(r#"{{"type":"ai-title","aiTitle":"Apartments","sessionId":"{id}"}}"#)];
        for (uuid, parent, prompt) in messages {
            let parent = parent.map_or("null".to_string(), |p| format!("\"{p}\""));
            lines.push(match prompt {
                Some(text) => format!(r#"{{"type":"user","uuid":"{uuid}","parentUuid":{parent},"timestamp":"{started}","sessionId":"{id}","message":{{"role":"user","content":"{text}"}}}}"#),
                None => format!(r#"{{"type":"assistant","uuid":"{uuid}","parentUuid":{parent},"timestamp":"{started}","sessionId":"{id}","message":{{"role":"assistant","content":[{{"type":"text","text":"ok"}}]}}}}"#),
            });
        }
        let path = dir.join(format!("{id}.jsonl"));
        std::fs::write(&path, lines.join("\n")).unwrap();
        path
    }

    #[test]
    fn sessions_list_under_the_one_they_came_from() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        // The first: three prompts.
        write_session(d, "a", "2026-11-13T08:00:00Z", &[("p1", None, Some("one")), ("r1", Some("p1"), None), ("p2", Some("r1"), Some("two")), ("r2", Some("p2"), None), ("p3", Some("r2"), Some("three")), ("r3", Some("p3"), None)]);
        // Continued from it (a new file going on after its last answer).
        write_session(d, "b", "2026-11-13T12:00:00Z", &[("p4", Some("r3"), Some("four")), ("r4", Some("p4"), None)]);
        // Branched: the first prompt kept, the second edited.
        write_session(d, "c", "2026-11-13T10:00:00Z", &[("p1", None, Some("one")), ("r1", Some("p1"), None), ("q2", Some("r1"), Some("two, changed")), ("s2", Some("q2"), None)]);
        // Unrelated.
        write_session(d, "z", "2026-11-12T08:00:00Z", &[("x1", None, Some("else"))]);
        let found: Vec<_> = ["a", "b", "c", "z"].iter().filter_map(|id| read_info(&d.join(format!("{id}.jsonl")))).collect();
        let linked = link(found);
        let get = |id: &str| linked.iter().find(|s| s.id == id).unwrap().clone();
        assert_eq!((get("b").parent.as_deref(), get("b").from, get("b").branched), (Some("a"), 3, false));
        assert_eq!((get("c").parent.as_deref(), get("c").from, get("c").branched), (Some("a"), 1, true));
        // A continuation that copies the parent's prompts, then goes on.
        write_session(d, "e", "2026-11-13T13:00:00Z", &[("p2", Some("r1"), Some("two")), ("r2", Some("p2"), None), ("p3", Some("r2"), Some("three")), ("r3", Some("p3"), None), ("p9", Some("r3"), Some("nine"))]);
        let again = link(["a", "e"].iter().filter_map(|id| read_info(&d.join(format!("{id}.jsonl")))).collect());
        let e = again.iter().find(|s| s.id == "e").unwrap();
        assert_eq!((e.parent.as_deref(), e.from, e.branched), (Some("a"), 3, false), "copied, then went on from the end");
        assert_eq!(get("a").parent, None);
        assert_eq!(get("z").parent, None);
        assert_eq!((get("b").whole, get("c").whole), (4, 2), "a conversation's prompts, once each");
        // Read as one conversation: a's prompts, then b's, once each.
        let t = load_from(d, Path::new("/tmp"), "b", None).unwrap();
        let prompts: Vec<String> = t.entries.iter().filter_map(|e| if let crate::Entry::User { text, .. } = e { Some(text.clone()) } else { None }).collect();
        assert_eq!(prompts, ["one", "two", "three", "four"]);
        // Editing "four" goes back to a message of the first file: resumed from it.
        let mark = t.prompts.iter().find(|m| m.uuid.as_deref() == Some("p4")).unwrap();
        assert_eq!((mark.before.as_deref(), mark.session.as_deref()), (Some("r3"), Some("a")));
        // A branch reads its own path only.
        let t = load_from(d, Path::new("/tmp"), "c", None).unwrap();
        let prompts: Vec<String> = t.entries.iter().filter_map(|e| if let crate::Entry::User { text, .. } = e { Some(text.clone()) } else { None }).collect();
        assert_eq!(prompts, ["one", "two, changed"]);
        // Renamed (as Claude Code's /rename does): the name wins, the last one
        // counts, and an empty one gives the made-up title back.
        let named = |name: &str| {
            rename_session_in(d, "a", name).unwrap();
            read_info(&d.join("a.jsonl")).unwrap().0.title
        };
        assert_eq!(named("Board rework"), "Board rework");
        assert_eq!(named("Board rework, part 2"), "Board rework, part 2");
        assert_eq!(named(""), "Apartments");
        let tree = as_tree(linked);
        let order: Vec<(String, usize)> = tree.iter().map(|s| (s.id.clone(), s.depth)).collect();
        assert_eq!(order.len(), 4);
        assert_eq!(order[0].1, 0);
        let a = order.iter().position(|(id, _)| id == "a").unwrap();
        assert!(order[a + 1].1 == 1 && order[a + 2].1 == 1, "{order:?}");
    }

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(format!("{}/tests/sessions/{name}", env!("CARGO_MANIFEST_DIR")))
    }

    /// Recorded: a background command, a sub-agent and one in the
    /// background; the agents' own records are in `<session>/subagents/`.
    #[test]
    fn a_saved_session_shows_its_agents_and_notifications() {
        use crate::{Entry, RunState};
        let t = load_from(&fixture("agents"), Path::new("/tmp/shed-cap-b"), "54996e21-70cb-43f1-88b0-3e377cd622bd", None).unwrap();
        let users: Vec<&str> = t.entries.iter().filter_map(|e| if let Entry::User { text, .. } = e { Some(text.as_str()) } else { None }).collect();
        assert_eq!(users.len(), 1, "the prompt only, no notification or sub-agent prompt: {users:#?}");
        assert!(users[0].starts_with("This is a test of tools"));
        let run = |name: &str| {
            t.entries
                .iter()
                .find_map(|e| match e {
                    Entry::Tool(x) if x.summary == name => x.run.as_deref().cloned(),
                    _ => None,
                })
                .unwrap()
        };
        let count = run("Agent: Count files");
        assert_eq!((count.state, count.background), (RunState::Done, false));
        assert!(count.steps.iter().any(|e| matches!(e, Entry::Tool(x) if x.summary == "$ ls -a" && x.result.is_some())), "{:#?}", count.steps);
        assert!(count.ended > count.started && count.started > 0);
        assert_eq!(count.tools, 1, "counted once, from its result or its steps");
        let echo = run("Agent: Echo twice");
        assert_eq!((echo.state, echo.background, echo.task.as_str()), (RunState::Done, true, "a49d0ab7a82dc4dff"));
        let steps: Vec<&str> = echo.steps.iter().filter_map(|e| if let Entry::Tool(x) = e { Some(x.summary.as_str()) } else { None }).collect();
        assert_eq!(steps, ["$ echo one", "$ echo two"]);
        assert_eq!(run("$ sleep 4").state, RunState::Done);
        let notes: Vec<&str> = t.entries.iter().filter_map(|e| if let Entry::Note(n) = e { Some(n.text.as_str()) } else { None }).collect();
        assert_eq!(notes, ["Background command \"Sleep 4 seconds in background\" completed (exit code 0)", "Agent \"Echo twice\" finished"]);
        assert!(t.status.timers.is_empty());
    }

    /// Recorded: a wake-up, a job, a watch and its events, as the session
    /// file keeps them; read back, nothing is left waiting.
    #[test]
    fn a_saved_session_shows_its_wake_ups_and_watches() {
        use crate::Entry;
        let t = load_from(&fixture("timers"), Path::new("/tmp/shed-cap-c"), "fa1e436c-2ca9-4680-aad8-ea160a2b764c", None).unwrap();
        let users = t.entries.iter().filter(|e| matches!(e, Entry::User { .. })).count();
        assert_eq!(users, 1, "{:#?}", t.entries);
        let notes: Vec<&str> = t.entries.iter().filter_map(|e| if let Entry::Note(n) = e { Some(n.text.as_str()) } else { None }).collect();
        assert_eq!(
            notes,
            [
                "Monitor event: \"three events\": ev1",
                "Monitor event: \"three events\": ev2",
                "Monitor event: \"three events\": ev3",
                "Monitor \"three events\" stream ended",
                "Scheduled job ran: Say tick, then delete this cron job with CronDelete.",
                "Woke up: check back on the test",
            ]
        );
        assert!(t.status.timers.is_empty(), "no Claude Code runs it now: {:?}", t.status.timers);
    }

    #[test]
    fn folders_name_their_sessions_dir() {
        let dir = sessions_dir(Path::new("/home/you/Workspace/my.app"));
        assert!(dir.ends_with("projects/-home-you-Workspace-my-app"), "{dir:?}");
    }

    #[test]
    fn sessions_list_with_titles_and_hide_shed_prompts() {
        let home = tempfile::tempdir().unwrap();
        // SAFETY: tests in this module set it to the same value before reading it.
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", home.path()) };
        let folder = Path::new("/tmp/shed-ai-test");
        let dir = sessions_dir(folder);
        std::fs::create_dir_all(&dir).unwrap();
        let fixture = std::fs::read_to_string(format!("{}/tests/streams/session.jsonl", env!("CARGO_MANIFEST_DIR"))).unwrap();
        std::fs::write(dir.join("aaa.jsonl"), &fixture).unwrap();
        let shed = format!(
            "{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":\"{SHED_PROMPT_PREAMBLE}. Shell: bash\"}}}}\n"
        );
        std::fs::write(dir.join("bbb.jsonl"), shed).unwrap();
        let list = list_sessions(folder);
        assert_eq!(list.len(), 1, "{list:?}");
        assert_eq!((list[0].id.as_str(), list[0].title.as_str(), list[0].prompts), ("aaa", "Notes.txt line append", 1));
        let t = load_session(folder, "aaa", None).unwrap();
        assert!(!t.entries.is_empty());
    }
}
