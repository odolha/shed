//! Questions asked aside: a copy of a session (Claude Code forks it), not
//! saved, that cannot change anything, asked one thing and gone. The session
//! itself never sees the question or the answer, so its context does not
//! grow. Used to explain the changes a conversation made.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

/// Where Claude Code is: `SHED_CLAUDE` (tests stand one in), on `PATH`, or
/// where its installer puts it.
pub fn claude_program() -> PathBuf {
    if let Some(program) = std::env::var_os("SHED_CLAUDE") {
        return program.into();
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let on_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).map(|d| d.join("claude")).find(|c| c.is_file()))
        .unwrap_or(None);
    on_path
        .or_else(|| [home.join(".local/bin/claude"), home.join(".claude/local/claude")].into_iter().find(|c| c.is_file()))
        .unwrap_or_else(|| "claude".into())
}

/// A file to explain: its path as the review shows it, how far its change
/// went ("not staged", "committed abc1234 …"), and its diff (cut short).
pub struct ToExplain {
    pub path: String,
    pub state: String,
    pub diff: String,
}

/// Asks a copy of `session` (in `folder`, with `model` when one is set) what
/// changed in each file and why; the answers, by the file's place in `files`
/// (a file can be there more than once: changed, and committed before).
pub fn explain_changes(program: &Path, folder: &Path, session: &str, model: Option<&str>, files: &[ToExplain]) -> Result<Vec<(usize, String)>, String> {
    let mut prompt = String::from(
        "This is a question on the side about the changes made in this conversation: do not change anything. \
         For each change below, explain in two or three sentences what changed and why, from what we did and \
         discussed. Answer for each by its number.\n\n",
    );
    for (n, f) in files.iter().enumerate() {
        prompt.push_str(&format!("## {}. {} ({})\n```diff\n{}\n```\n\n", n + 1, f.path, f.state, f.diff));
    }
    let schema = json!({
        "type": "object",
        "properties": {"files": {"type": "array", "items": {
            "type": "object",
            "properties": {"number": {"type": "integer"}, "explanation": {"type": "string"}},
            "required": ["number", "explanation"]
        }}},
        "required": ["files"]
    });
    let mut cmd = Command::new(program);
    cmd.current_dir(folder)
        .args(["-p", "--resume", session, "--fork-session", "--no-session-persistence", "--output-format", "json"])
        .args(["--permission-mode", "plan", "--disallowed-tools", "Edit", "Write", "NotebookEdit", "Bash"])
        .arg("--json-schema")
        .arg(schema.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(m) = model {
        cmd.args(["--model", m]);
    }
    let mut child = cmd.spawn().map_err(|e| format!("cannot start Claude Code: {e}"))?;
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        stdin.write_all(prompt.as_bytes()).map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    match parse_explanations(&text) {
        Some(found) => Ok(found),
        None if !out.status.success() => {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("Claude Code could not answer").to_string())
        }
        None => Err("Claude Code's answer could not be read".into()),
    }
}

/// The explanations in Claude Code's JSON result: its structured output, or
/// the JSON in its text.
pub fn parse_explanations(stdout: &str) -> Option<Vec<(usize, String)>> {
    let result: Value = serde_json::from_str(stdout.trim()).ok()?;
    let answer = result.get("structured_output").cloned().filter(|v| v.is_object()).or_else(|| {
        let text = result.get("result")?.as_str()?;
        let (start, end) = (text.find('{')?, text.rfind('}')?);
        serde_json::from_str(&text[start..=end]).ok()
    })?;
    let files = answer.get("files")?.as_array()?;
    Some(
        files
            .iter()
            .filter_map(|f| {
                let n = f.get("number")?.as_u64()? as usize;
                Some((n.checked_sub(1)?, f.get("explanation")?.as_str()?.trim().to_string()))
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_read_from_structured_output_or_text() {
        let structured = r#"{"type":"result","result":"","structured_output":{"files":[{"number":1,"explanation":" Adds a check. "}]}}"#;
        assert_eq!(parse_explanations(structured), Some(vec![(0, "Adds a check.".into())]));
        let in_text = r#"{"type":"result","result":"Here:\n{\"files\":[{\"number\":2,\"explanation\":\"Why.\"}]}"}"#;
        assert_eq!(parse_explanations(in_text), Some(vec![(1, "Why.".into())]));
        assert_eq!(parse_explanations("not json"), None);
    }

    #[test]
    fn a_copy_of_the_session_is_asked_and_left_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("args");
        let fake = dir.path().join("claude");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\ncat > {}.stdin\nprintf '%s' '{{\"type\":\"result\",\"structured_output\":{{\"files\":[{{\"number\":1,\"explanation\":\"Because.\"}}]}}}}'\n",
                log.display(),
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let files = [ToExplain { path: "src/a.rs".into(), state: "not staged".into(), diff: "+fn a() {}".into() }];
        let got = explain_changes(&fake, dir.path(), "abc", Some("opus"), &files).unwrap();
        assert_eq!(got, [(0, "Because.".to_string())]);
        let args = std::fs::read_to_string(&log).unwrap();
        for wanted in ["--resume\nabc\n", "--fork-session", "--no-session-persistence", "--permission-mode\nplan", "--model\nopus"] {
            assert!(args.contains(wanted), "{wanted:?} in {args}");
        }
        let asked = std::fs::read_to_string(format!("{}.stdin", log.display())).unwrap();
        assert!(asked.contains("## 1. src/a.rs (not staged)") && asked.contains("+fn a() {}"), "{asked}");
    }
}
