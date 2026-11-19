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
        "AskUserQuestion" => {
            let questions = crate::parse_questions(input);
            let first = questions.first().map(|q| q.question.clone()).unwrap_or_default();
            let more = if questions.len() > 1 { format!(" (and {} more)", questions.len() - 1) } else { String::new() };
            let detail = questions.iter().map(|q| format!("{}\n{}", q.question, q.options.iter().map(|o| format!("  · {}", o.label)).collect::<Vec<_>>().join("\n"))).collect::<Vec<_>>().join("\n\n");
            (format!("Asked: {first}{more}"), detail, None)
        }
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
        browser if browser.starts_with(BROWSER_TOOLS) => (browser_step(&browser[BROWSER_TOOLS.len()..], input), serde_json::to_string_pretty(input).unwrap_or_default(), None),
        _ => {
            let compact = serde_json::to_string(input).unwrap_or_default();
            let short: String = compact.chars().take(80).collect();
            (format!("{name} {short}"), serde_json::to_string_pretty(input).unwrap_or_default(), None)
        }
    };
    (summary, detail, file, diff)
}

/// The prefix of Claude in Chrome's tools.
pub const BROWSER_TOOLS: &str = "mcp__claude-in-chrome__";

/// A Claude in Chrome step, as a person would say it.
fn browser_step(tool: &str, input: &Value) -> String {
    let s = |k: &str| input.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let clip = |t: String| if t.chars().count() > 60 { format!("{}…", t.chars().take(60).collect::<String>()) } else { t };
    let what = match tool {
        "navigate" => match s("url").as_str() {
            "back" => "go back".into(),
            "forward" => "go forward".into(),
            url => format!("open {url}"),
        },
        "computer" => match s("action").as_str() {
            "screenshot" => "take a screenshot".into(),
            "left_click" | "right_click" | "double_click" | "triple_click" => "click".into(),
            "type" => format!("type “{}”", clip(s("text"))),
            "key" => format!("press {}", s("text")),
            "scroll" | "scroll_to" => "scroll".into(),
            "wait" => "wait".into(),
            "" => "act on the page".into(),
            action => action.replace('_', " "),
        },
        "find" => format!("find “{}”", clip(s("query"))),
        "read_page" => "read the page".into(),
        "get_page_text" => "read the page's text".into(),
        "form_input" => "fill a form field".into(),
        "javascript_tool" => "run JavaScript on the page".into(),
        "tabs_context_mcp" | "tabs_context" => "look at its tabs".into(),
        "tabs_create_mcp" | "tabs_create" => "open a new tab".into(),
        "read_console_messages" => "read the console".into(),
        "read_network_requests" => "read network requests".into(),
        "gif_creator" => "record a GIF".into(),
        "resize_window" => "resize the window".into(),
        "upload_image" => "upload a picture".into(),
        other => other.replace('_', " "),
    };
    format!("Browser: {what}")
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
        let (s, _, _, _) = summarize("mcp__claude-in-chrome__navigate", &json!({"url": "https://example.com", "tabId": 1}), "/p");
        assert_eq!(s, "Browser: open https://example.com");
        let (s, _, _, _) = summarize("mcp__claude-in-chrome__computer", &json!({"action": "screenshot", "tabId": 1}), "/p");
        assert_eq!(s, "Browser: take a screenshot");
    }

    #[test]
    fn questions_and_their_answers() {
        let input = json!({"questions": [
            {"question": "Which database?", "header": "DB", "multiSelect": false,
             "options": [{"label": "Postgres (Recommended)", "description": "Already in use"}, {"label": "SQLite", "description": "Simpler"}]},
            {"question": "Which checks?", "header": "CI", "multiSelect": true, "options": [{"label": "lint"}, {"label": "tests"}]}
        ]});
        let qs = crate::parse_questions(&input);
        assert_eq!(qs.len(), 2);
        assert_eq!((qs[0].header.as_str(), qs[0].options[1].label.as_str(), qs[0].multi), ("DB", "SQLite", false));
        assert!(qs[1].multi && qs[1].options[0].description.is_empty());
        let (s, _, _, _) = summarize("AskUserQuestion", &input, "/p");
        assert_eq!(s, "Asked: Which database? (and 1 more)");
        let out = crate::answered_input(&input, &qs, &["SQLite".into(), "lint, tests".into()]);
        assert_eq!(out["answers"]["Which database?"], "SQLite");
        assert_eq!(out["answers"]["Which checks?"], "lint, tests");
        assert_eq!(out["questions"], input["questions"], "the questions go back as they came");
    }
}
