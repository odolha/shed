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
}

/// What a session file says about where it comes from.
#[derive(Default)]
struct Lineage {
    /// When its first message was written (RFC 3339, so it sorts).
    started: String,
    /// The message its first message follows, in another file when continued.
    first_parent: Option<String>,
    /// Every message id, with how many prompts were made up to it.
    ids: std::collections::HashMap<String, usize>,
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
    let mut out = Vec::with_capacity(found.len());
    for (i, (info, lin)) in found.iter().enumerate() {
        let mut info = info.clone();
        let others = || found.iter().enumerate().filter(move |(j, _)| *j != i).map(|(_, f)| f);
        // Its parent holds the message its first one follows, or (an older
        // one) its first prompt.
        let parent = lin
            .first_parent
            .as_ref()
            .and_then(|p| others().find(|(_, ol)| ol.ids.contains_key(p)))
            .or_else(|| {
                let first = lin.prompt_ids.first()?;
                others().filter(|(_, ol)| ol.started < lin.started && ol.ids.contains_key(first)).max_by_key(|(_, ol)| lin.ids.keys().filter(|k| ol.ids.contains_key(*k)).count())
            });
        // Where in the parent it goes on: past every message they share, or
        // after the one it follows. At the parent's end it continued it;
        // earlier, it branched off (an earlier message edited).
        if let Some((p, pl)) = parent {
            let shared = lin.ids.keys().filter_map(|k| pl.ids.get(k)).copied().max();
            let follows = lin.first_parent.as_ref().and_then(|f| pl.ids.get(f)).copied();
            let at = shared.into_iter().chain(follows).max().unwrap_or(0);
            info.parent = Some(p.id.clone());
            info.from = at;
            info.branched = at < p.prompts;
        }
        out.push(info);
    }
    out
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
    let mut branch = None;
    let mut prompts = 0;
    let mut lineage = Lineage::default();
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
            lineage.ids.insert(uuid, prompts);
        }
        // Most lines are neither: look before parsing.
        let is_title = line.contains("\"ai-title\"") || line.contains("\"summary\"");
        let is_user = line.contains("\"user\"") && !line.contains("\"tool_result\"");
        if !is_title && !is_user {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        match v.get("type").and_then(Value::as_str) {
            Some("ai-title") => title = v.get("aiTitle").and_then(Value::as_str).map(String::from),
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
                    lineage.ids.insert(uuid.to_string(), prompts);
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
    let title = title.unwrap_or_else(|| {
        let line = first.lines().next().unwrap_or("").trim();
        let mut short: String = line.chars().take(80).collect();
        if line.chars().count() > 80 {
            short.push('…');
        }
        short
    });
    Some((SessionInfo { id, title, modified, branch, prompts, ..Default::default() }, lineage))
}

/// A saved session as a conversation, to show it again.
/// A saved session as a conversation; its prompts' pictures are kept in
/// `picture_dir`, to show them.
pub fn load_session(folder: &Path, id: &str, picture_dir: Option<std::path::PathBuf>) -> Option<Transcript> {
    let path = sessions_dir(folder).join(format!("{id}.jsonl"));
    let file = std::fs::File::open(path).ok()?;
    let mut t = Transcript::new(&folder.to_string_lossy());
    t.picture_dir = picture_dir;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if !(line.contains("\"user\"") || line.contains("\"assistant\"")) {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            t.apply(&v);
        }
    }
    t.status.session = Some(id.to_string());
    t.status.state = crate::State::Idle;
    t.status.since = None;
    Some(t)
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
        let tree = as_tree(linked);
        let order: Vec<(String, usize)> = tree.iter().map(|s| (s.id.clone(), s.depth)).collect();
        assert_eq!(order.len(), 4);
        assert_eq!(order[0].1, 0);
        let a = order.iter().position(|(id, _)| id == "a").unwrap();
        assert!(order[a + 1].1 == 1 && order[a + 2].1 == 1, "{order:?}");
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
