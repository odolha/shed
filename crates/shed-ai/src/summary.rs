//! One line for a tool call, as a person would say it.

use serde_json::Value;

use crate::Diff;

/// A tool call's one-line summary, its input in full, the file it touches,
/// and for edits the change. Paths under `cwd` are shown relative to it.
pub fn summarize(name: &str, input: &Value, cwd: &str) -> (String, String, Option<String>, Option<Diff>) {
    let s = |k: &str| input.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let rel = |p: &str| {
        let p = p.strip_prefix(cwd).map(|r| r.trim_start_matches('/')).filter(|r| !r.is_empty()).unwrap_or(p);
        p.to_string()
    };
    let file = ["file_path", "notebook_path", "path"].iter().map(|k| s(k)).find(|p| !p.is_empty());
    let first_line = |t: &str| t.lines().next().unwrap_or("").to_string();
    let (summary, detail, diff) = match name {
        "Read" => {
            let path = rel(&s("file_path"));
            let at = match (input.get("offset").and_then(Value::as_u64), input.get("limit").and_then(Value::as_u64)) {
                (Some(o), Some(l)) => format!(" (lines {}–{})", o, o + l),
                (Some(o), None) => format!(" (from line {o})"),
                _ => String::new(),
            };
            (format!("Read {path}{at}"), String::new(), None)
        }
        "Edit" => (format!("Edit {}", rel(&s("file_path"))), String::new(), Some(Diff { old: s("old_string"), new: s("new_string") })),
        "MultiEdit" => {
            let edits = input.get("edits").and_then(Value::as_array).cloned().unwrap_or_default();
            let get = |e: &Value, k: &str| e.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let old = edits.iter().map(|e| get(e, "old_string")).collect::<Vec<_>>().join("\n…\n");
            let new = edits.iter().map(|e| get(e, "new_string")).collect::<Vec<_>>().join("\n…\n");
            (format!("Edit {} ({} changes)", rel(&s("file_path")), edits.len()), String::new(), Some(Diff { old, new }))
        }
        "Write" => (format!("Write {}", rel(&s("file_path"))), String::new(), Some(Diff { old: String::new(), new: s("content") })),
        "NotebookEdit" => (format!("Edit notebook {}", rel(&s("notebook_path"))), s("new_source"), None),
        "Bash" => {
            let command = s("command");
            let description = s("description");
            let detail = if description.is_empty() { command.clone() } else { format!("{command}\n\n# {description}") };
            (format!("$ {}", first_line(&command)), detail, None)
        }
        "Grep" => {
            let path = s("path");
            let within = if path.is_empty() { String::new() } else { format!(" in {}", rel(&path)) };
            (format!("Search “{}”{within}", s("pattern")), String::new(), None)
        }
        "Glob" => (format!("Find {}", s("pattern")), String::new(), None),
        "WebFetch" => (format!("Fetch {}", s("url")), s("prompt"), None),
        "WebSearch" => (format!("Search the web for “{}”", s("query")), String::new(), None),
        "Task" | "Agent" => (format!("Agent: {}", s("description")), s("prompt"), None),
        "TodoWrite" => {
            let todos = input.get("todos").and_then(Value::as_array).cloned().unwrap_or_default();
            let list = todos
                .iter()
                .map(|t| {
                    let mark = match t.get("status").and_then(Value::as_str) {
                        Some("completed") => "☑",
                        Some("in_progress") => "▶",
                        _ => "☐",
                    };
                    format!("{mark} {}", t.get("content").and_then(Value::as_str).unwrap_or(""))
                })
                .collect::<Vec<_>>()
                .join("\n");
            ("Update the task list".into(), list, None)
        }
        _ => {
            let compact = serde_json::to_string(input).unwrap_or_default();
            let short: String = compact.chars().take(80).collect();
            (format!("{name} {short}"), serde_json::to_string_pretty(input).unwrap_or_default(), None)
        }
    };
    (summary, detail, file, diff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tools_read_as_sentences() {
        let (s, _, f, _) = summarize("Read", &json!({"file_path": "/p/src/a.rs", "offset": 10, "limit": 5}), "/p");
        assert_eq!((s.as_str(), f.as_deref()), ("Read src/a.rs (lines 10–15)", Some("/p/src/a.rs")));
        let (s, d, _, _) = summarize("Bash", &json!({"command": "cargo test\necho done", "description": "Run tests"}), "/p");
        assert_eq!(s, "$ cargo test");
        assert!(d.contains("# Run tests"));
        let (s, _, _, diff) = summarize("Edit", &json!({"file_path": "/elsewhere/x", "old_string": "a", "new_string": "b"}), "/p");
        assert_eq!(s, "Edit /elsewhere/x");
        assert_eq!(diff, Some(Diff { old: "a".into(), new: "b".into() }));
        let (s, _, _, _) = summarize("Grep", &json!({"pattern": "fn main", "path": "/p/src"}), "/p");
        assert_eq!(s, "Search “fn main” in src");
    }
}
