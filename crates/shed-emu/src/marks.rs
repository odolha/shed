//! Shell-integration marks in a pane's output.
//!
//! The emulator core ignores these OSC sequences, so they are picked out of
//! the byte stream separately: OSC 133 (prompt and command boundaries, with
//! the exit code), OSC 7 (working directory) and Shed's own OSC 7777
//! (command text, and the state of a history walk for the scope chip).

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

/// The longest OSC kept; anything longer is not a mark and is skipped.
const MAX_OSC: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mark {
    /// OSC 133;A: a prompt starts.
    PromptStart,
    /// OSC 133;B: the prompt ends; the user is typing.
    InputStart,
    /// OSC 133;C: the command runs; its output starts.
    Executed,
    /// OSC 133;D: the command finished, with its exit code if known.
    Finished(Option<i32>),
    /// OSC 7: the shell's working directory.
    Cwd(String),
    /// The text of the command about to run.
    Command(String),
    /// A step of walking bench or global history.
    Walk(WalkState),
    /// The walk ended (back at what was typed).
    WalkEnd,
    /// The shell's own aliases, as `alias -p` prints them.
    ShellAliases(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalkState {
    pub scope: String,
    pub pos: usize,
    pub total: usize,
    /// The title of the tab the command was last run in.
    pub source: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Esc,
    Osc,
    OscEsc,
}

/// Finds marks in output, across chunk boundaries.
#[derive(Debug, Default)]
pub struct MarkScanner {
    state: State,
    buf: Vec<u8>,
    overflow: bool,
}

impl MarkScanner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn scan(&mut self, bytes: &[u8], out: &mut Vec<Mark>) {
        let mut i = 0;
        while i < bytes.len() {
            match self.state {
                State::Ground => match bytes[i..].iter().position(|&b| b == 0x1b) {
                    Some(at) => {
                        i += at + 1;
                        self.state = State::Esc;
                    }
                    None => return,
                },
                State::Esc => {
                    if bytes[i] == b']' {
                        self.state = State::Osc;
                        self.buf.clear();
                        self.overflow = false;
                        i += 1;
                    } else {
                        // Not an OSC; this byte may itself start another escape.
                        self.state = State::Ground;
                    }
                }
                State::Osc => {
                    let b = bytes[i];
                    i += 1;
                    match b {
                        0x07 => self.finish(out),
                        0x1b => self.state = State::OscEsc,
                        0x18 | 0x1a => self.state = State::Ground,
                        _ if self.buf.len() < MAX_OSC => self.buf.push(b),
                        _ => self.overflow = true,
                    }
                }
                State::OscEsc => {
                    if bytes[i] == b'\\' {
                        i += 1;
                        self.finish(out);
                    } else {
                        // The OSC was cut short by another escape sequence.
                        self.state = State::Esc;
                    }
                }
            }
        }
    }

    fn finish(&mut self, out: &mut Vec<Mark>) {
        self.state = State::Ground;
        if !self.overflow
            && let Some(mark) = parse(&self.buf)
        {
            out.push(mark);
        }
    }
}

fn parse(osc: &[u8]) -> Option<Mark> {
    let text = std::str::from_utf8(osc).ok()?;
    let mut parts = text.split(';');
    match parts.next()? {
        "133" => Some(match parts.next()? {
            "A" => Mark::PromptStart,
            "B" => Mark::InputStart,
            "C" => Mark::Executed,
            "D" => Mark::Finished(parts.next().and_then(|c| c.parse().ok())),
            _ => return None,
        }),
        "7" => {
            let url = text.strip_prefix("7;")?;
            let rest = url.strip_prefix("file://")?;
            // Skip the host; the path starts at the first slash.
            let path = &rest[rest.find('/')?..];
            Some(Mark::Cwd(percent_decode(path)))
        }
        "7777" => {
            if parts.next()? != "shed" {
                return None;
            }
            match parts.next()? {
                "cmd" => {
                    let bytes = BASE64.decode(parts.next()?).ok()?;
                    Some(Mark::Command(String::from_utf8_lossy(&bytes).into_owned()))
                }
                "aliases" => {
                    let bytes = BASE64.decode(parts.next().unwrap_or("")).ok()?;
                    Some(Mark::ShellAliases(String::from_utf8_lossy(&bytes).into_owned()))
                }
                "walk" => {
                    let scope = parts.next()?;
                    if scope == "end" {
                        return Some(Mark::WalkEnd);
                    }
                    let pos = parts.next()?.parse().ok()?;
                    let total = parts.next()?.parse().ok()?;
                    let source = parts
                        .next()
                        .and_then(|s| BASE64.decode(s).ok())
                        .map(|b| String::from_utf8_lossy(&b).into_owned())
                        .unwrap_or_default();
                    Some(Mark::Walk(WalkState { scope: scope.into(), pos, total, source }))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parses `alias -p` output: `alias ll='ls -alF'` per line.
pub fn parse_alias_list(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("alias ")?;
            let (name, value) = rest.split_once('=')?;
            let value = match value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
                Some(quoted) => quoted.replace(r"'\''", "'"),
                None => value.to_string(),
            };
            Some((name.to_string(), value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_all(chunks: &[&[u8]]) -> Vec<Mark> {
        let mut s = MarkScanner::new();
        let mut out = Vec::new();
        for c in chunks {
            s.scan(c, &mut out);
        }
        out
    }

    #[test]
    fn a_whole_command_cycle() {
        let cmd = BASE64.encode("ls -la ~/My Files");
        let stream = format!(
            "\x1b]133;A\x07$ \x1b]133;B\x07\x1b]7777;shed;cmd;{cmd}\x07\x1b]133;C\x07output\x1b[31mred\x1b[0m\r\n\x1b]133;D;2\x07\x1b]7;file://host/home/me/My%20Files\x1b\\"
        );
        assert_eq!(
            scan_all(&[stream.as_bytes()]),
            vec![
                Mark::PromptStart,
                Mark::InputStart,
                Mark::Command("ls -la ~/My Files".into()),
                Mark::Executed,
                Mark::Finished(Some(2)),
                Mark::Cwd("/home/me/My Files".into()),
            ]
        );
    }

    #[test]
    fn marks_split_across_chunks_byte_by_byte() {
        let stream = b"x\x1b]133;D;0\x07y\x1b]7;file:///tmp\x1b\\";
        let chunks: Vec<&[u8]> = stream.chunks(1).collect();
        assert_eq!(scan_all(&chunks), vec![Mark::Finished(Some(0)), Mark::Cwd("/tmp".into())]);
    }

    #[test]
    fn walk_state_and_noise() {
        let source = BASE64.encode("api");
        let stream = format!(
            "\x1b]0;title\x07\x1b]7777;shed;walk;bench;3;120;{source}\x07\x1b]8;;http://x\x1b\\\x1b]7777;shed;walk;end\x07\x1b]133;D\x07"
        );
        assert_eq!(
            scan_all(&[stream.as_bytes()]),
            vec![
                Mark::Walk(WalkState { scope: "bench".into(), pos: 3, total: 120, source: "api".into() }),
                Mark::WalkEnd,
                Mark::Finished(None),
            ]
        );
    }

    #[test]
    fn alias_lists() {
        let list = "alias ll='ls -alF'\nalias say='echo '\\''hi'\\'''\nnot an alias";
        assert_eq!(
            parse_alias_list(list),
            vec![("ll".into(), "ls -alF".into()), ("say".into(), "echo 'hi'".into())]
        );
        let stream = format!("\x1b]7777;shed;aliases;{}\x07", BASE64.encode("alias a='b'"));
        assert_eq!(scan_all(&[stream.as_bytes()]), vec![Mark::ShellAliases("alias a='b'".into())]);
    }

    #[test]
    fn interrupted_osc_does_not_swallow_the_next() {
        // An OSC cut short by CSI, then a real mark.
        let stream = b"\x1b]133;A\x1b[0m\x1b]133;B\x07";
        assert_eq!(scan_all(&[stream]), vec![Mark::InputStart]);
    }
}
