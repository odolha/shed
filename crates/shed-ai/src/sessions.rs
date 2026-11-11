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
    let mut sessions: Vec<SessionInfo> = dir
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| read_info(&e.path()))
        .collect();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.modified));
    sessions
}

fn read_info(path: &Path) -> Option<SessionInfo> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let id = path.file_stem()?.to_string_lossy().into_owned();
    let file = std::fs::File::open(path).ok()?;
    let mut first_prompt: Option<String> = None;
    let mut title: Option<String> = None;
    let mut branch = None;
    let mut prompts = 0;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
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
    Some(SessionInfo { id, title, modified, branch, prompts })
}

/// A saved session as a conversation, to show it again.
pub fn load_session(folder: &Path, id: &str) -> Option<Transcript> {
    let path = sessions_dir(folder).join(format!("{id}.jsonl"));
    let file = std::fs::File::open(path).ok()?;
    let mut t = Transcript::new(&folder.to_string_lossy());
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
        let t = load_session(folder, "aaa").unwrap();
        assert!(!t.entries.is_empty());
    }
}
