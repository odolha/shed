//! The AI prompt: a line typed as `> start this project` goes to a
//! command-line AI (`claude -p` unless set otherwise), run in the pane's
//! folder so it can look at the project, and the command it answers with is
//! put on the prompt for the user to read, change and run.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// How long an answer may take.
pub const LIMIT: Duration = Duration::from_secs(180);

/// The longest answer put on the prompt.
const MAX_ANSWER: usize = 8 * 1024;

/// What the AI is told besides the request.
pub struct Context {
    pub cwd: String,
    pub shell: String,
    pub os: String,
    /// The tab's recent commands, oldest first.
    pub history: Vec<String>,
    /// The pane's last lines of text.
    pub screen: Option<String>,
}

/// The text sent on the command's stdin.
pub fn prompt(request: &str, cx: &Context) -> String {
    let mut out = format!(
        "You are the AI prompt of Shed, a terminal. The user typed a request at their shell prompt; \
         answer with the shell command that does it, which Shed will put on the prompt for them to \
         review and run themselves. You may look at files in the current folder to understand the \
         project (its README, build files, scripts) before answering, but do not run or change anything.\n\n\
         Shell: {}\nSystem: {}\nCurrent folder: {}\n",
        cx.shell, cx.os, cx.cwd
    );
    if !cx.history.is_empty() {
        out.push_str("\nCommands run recently in this terminal tab, oldest first:\n");
        for command in &cx.history {
            out.push_str(&format!("  {command}\n"));
        }
    }
    if let Some(screen) = cx.screen.as_deref().filter(|s| !s.trim().is_empty()) {
        out.push_str("\nThe last lines on the screen:\n");
        for line in screen.lines() {
            out.push_str(&format!("  | {line}\n"));
        }
    }
    out.push_str(&format!(
        "\nRequest: {request}\n\n\
         Reply with the command only: no explanation, no Markdown, no code fences. Several steps may be \
         joined with && or put on separate lines. If it cannot be done with a command, reply with a \
         shell comment (a line starting with #) saying why.\n"
    ));
    out
}

/// Runs `command` with `sh -c` in `cwd`, `input` on its stdin; its stdout,
/// cleaned, is the command to type. On macOS it runs in the user's login
/// shell instead: apps opened from the Dock get a bare PATH, without
/// Homebrew's or ~/.local/bin, where command-line AIs are.
pub async fn run(command: &str, cwd: &Path, input: String) -> Result<String, String> {
    let (shell, flags) = if cfg!(target_os = "macos") {
        (std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into()), "-lc")
    } else {
        ("sh".to_string(), "-c")
    };
    let mut child = tokio::process::Command::new(shell)
        .arg(flags)
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("cannot run {command:?}: {e}"))?;
    let mut stdin = child.stdin.take().expect("piped");
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let write = async move {
        let _ = stdin.write_all(input.as_bytes()).await;
        // Dropped here: the command sees the end of its input.
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let read = async { tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err)) };
    let ((), _) = tokio::time::timeout(LIMIT, async { tokio::join!(write, read) })
        .await
        .map_err(|_| format!("no answer within {} seconds", LIMIT.as_secs()))?;
    let status = child.wait().await.map_err(|e| e.to_string())?;
    if !status.success() {
        let why = String::from_utf8_lossy(&err);
        let why = why.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
        return Err(match (why.is_empty(), status.code()) {
            (false, _) => why,
            (true, Some(127)) => format!("{command:?} is not installed (set [ai] command in shed.toml)"),
            (true, code) => format!("{command:?} failed ({})", code.map_or("killed".into(), |c| format!("exit {c}"))),
        });
    }
    clean(&String::from_utf8_lossy(&out)).ok_or_else(|| "the AI answered with nothing".into())
}

/// The command in an answer: code fences and surrounding blank lines removed.
pub fn clean(answer: &str) -> Option<String> {
    let mut lines: Vec<&str> = answer.lines().collect();
    // A fenced block: keep what is inside the first one.
    if let Some(start) = lines.iter().position(|l| l.trim_start().starts_with("```")) {
        let end = lines[start + 1..].iter().position(|l| l.trim_start().starts_with("```")).map_or(lines.len(), |e| start + 1 + e);
        lines = lines[start + 1..end].to_vec();
    }
    let text = lines.join("\n");
    let text = text.trim();
    let text = text.strip_prefix('$').map(str::trim_start).unwrap_or(text);
    if text.is_empty() {
        return None;
    }
    let mut text = text.to_string();
    if text.len() > MAX_ANSWER {
        let mut cut = MAX_ANSWER;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
    }
    Some(text)
}

/// The system, for the AI: the distribution's name, or macOS.
pub fn os_name() -> String {
    if cfg!(target_os = "macos") {
        return "macOS".into();
    }
    let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    release
        .lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME="))
        .map(|v| format!("Linux ({})", v.trim_matches('"')))
        .unwrap_or_else(|| "Linux".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_cleaned() {
        assert_eq!(clean("ls -la\n").as_deref(), Some("ls -la"));
        assert_eq!(clean("```bash\nnpm install && npm run dev\n```\n").as_deref(), Some("npm install && npm run dev"));
        assert_eq!(clean("Here:\n```\ncargo run\n```\nThat starts it.").as_deref(), Some("cargo run"));
        assert_eq!(clean("$ make\n").as_deref(), Some("make"));
        assert_eq!(clean("  \n\n").as_deref(), None);
        assert_eq!(clean("cd web\nnpm start").as_deref(), Some("cd web\nnpm start"));
    }

    #[test]
    fn the_prompt_carries_the_context() {
        let cx = Context {
            cwd: "~/projects/api".into(),
            shell: "zsh".into(),
            os: "Linux".into(),
            history: vec!["git pull".into()],
            screen: Some("error: port 8080 in use".into()),
        };
        let p = prompt("free the port", &cx);
        for part in ["Request: free the port", "Current folder: ~/projects/api", "Shell: zsh", "  git pull", "  | error: port 8080 in use"] {
            assert!(p.contains(part), "{part:?} missing from {p}");
        }
    }

    #[tokio::test]
    async fn runs_the_command_with_the_prompt_on_stdin() {
        let dir = std::env::temp_dir();
        assert_eq!(run("read line; echo \"echo got:$line\"", &dir, "hello\n".into()).await.unwrap(), "echo got:hello");
        assert!(run("echo nope >&2; exit 3", &dir, String::new()).await.unwrap_err().contains("nope"));
        assert!(run("exit 0", &dir, String::new()).await.is_err(), "an empty answer is an error");
    }
}
