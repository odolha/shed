//! What Claude started that goes on beside the conversation, as one list:
//! sub-agents, commands and watches in the background, wake-ups and
//! scheduled jobs. Windows show it above the prompt; the keeper counts what
//! is still going to tell the tab waits on it between turns.

use crate::{Entry, Run, Status, Timer, TimerKind, Tool};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkKind {
    Agent,
    /// A shell command in the background (`run_in_background`).
    Command,
    /// A watch (Monitor): it tells Claude what it sees.
    Watch,
    Wakeup,
    Job,
}

/// One piece of background work: its tool call (where it started), how it
/// runs (agents, commands, watches) and when it fires (wake-ups, jobs, a
/// watch's end).
#[derive(Clone, Debug)]
pub struct Work<'a> {
    pub kind: WorkKind,
    /// The entry of the tool call that started it, when it is in the conversation.
    pub entry: Option<usize>,
    pub tool: Option<&'a Tool>,
    pub run: Option<&'a Run>,
    pub timer: Option<&'a Timer>,
}

impl Work<'_> {
    /// Still going, or still to come (a timer is until it fires or goes).
    pub fn going(&self) -> bool {
        self.run.is_none_or(Run::running)
    }

    /// When it ended, Unix seconds (0: still going).
    pub fn ended(&self) -> u64 {
        self.run.filter(|r| !r.running()).map_or(0, |r| r.ended.max(1))
    }
}

/// Everything Claude started beside the conversation, in the order it
/// started (timers whose call is not in the conversation last): runs, ended
/// ones too (callers keep the recent ones), and timers. A watch is one item,
/// its run and its timer together. Commands Claude waited for are not in it.
pub fn background_work<'a>(entries: &'a [Entry], status: &'a Status) -> Vec<Work<'a>> {
    let mut out: Vec<Work<'a>> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let Entry::Tool(t) = e else { continue };
        let Some(run) = t.run.as_deref() else { continue };
        let kind = match t.name.as_str() {
            "Agent" | "Task" => WorkKind::Agent,
            "Monitor" => WorkKind::Watch,
            _ if run.background => WorkKind::Command,
            _ => continue,
        };
        let timer = (kind == WorkKind::Watch).then(|| status.timers.iter().find(|x| x.kind == TimerKind::Watch && x.tool == t.id)).flatten();
        out.push(Work { kind, entry: Some(i), tool: Some(t), run: Some(run), timer });
    }
    for timer in &status.timers {
        if out.iter().any(|w| w.timer.is_some_and(|x| std::ptr::eq(x, timer))) {
            continue;
        }
        let entry = (!timer.tool.is_empty()).then(|| entries.iter().rposition(|e| matches!(e, Entry::Tool(x) if x.id == timer.tool))).flatten();
        let tool = entry.and_then(|i| match &entries[i] {
            Entry::Tool(t) => Some(t),
            _ => None,
        });
        let kind = match timer.kind {
            TimerKind::Wakeup => WorkKind::Wakeup,
            TimerKind::Job => WorkKind::Job,
            TimerKind::Watch => WorkKind::Watch,
        };
        out.push(Work { kind, entry, tool, run: None, timer: Some(timer) });
    }
    // (Stable: a run and the timers after it keep their order.)
    out.sort_by_key(|w| w.entry.unwrap_or(usize::MAX));
    out
}

/// How many pieces of background work are going: what Claude waits on
/// once its turn is over.
pub fn background_count(entries: &[Entry], status: &Status) -> u32 {
    background_work(entries, status).iter().filter(|w| w.going()).count() as u32
}

/// The last line a command wrote to its output file, as a person reads it
/// (colours and cursor moves left out, a line redrawn in place as it ends),
/// cut to `max` characters; none while it wrote nothing.
pub fn last_output_line(path: &std::path::Path, max: usize) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    const TAIL: u64 = 8 * 1024;
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TAIL))).ok()?;
    let mut bytes = Vec::new();
    file.take(TAIL).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    text.lines().rev().map(plain_line).find(|l| !l.trim().is_empty()).map(|l| {
        let l = l.trim();
        if l.chars().count() > max { format!("{}…", l.chars().take(max).collect::<String>()) } else { l.to_string() }
    })
}

/// A terminal line as shown: escape sequences out, and what follows the last
/// carriage return (a progress bar redrawn in place).
fn plain_line(line: &str) -> String {
    let line = line.rsplit('\r').find(|part| !part.trim().is_empty()).unwrap_or("");
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                // CSI: parameters, then one final byte.
                Some('[') => {
                    while let Some(&n) = chars.peek() {
                        chars.next();
                        if ('@'..='~').contains(&n) {
                            break;
                        }
                    }
                }
                // OSC: up to BEL or ST.
                Some(']') => {
                    while let Some(n) = chars.next() {
                        if n == '\u{7}' || (n == '\u{1b}' && chars.peek() == Some(&'\\')) {
                            chars.next_if_eq(&'\\');
                            break;
                        }
                    }
                }
                _ => {}
            },
            c if c.is_control() && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RunState;

    fn tool(id: &str, name: &str, run: Option<Run>) -> Entry {
        Entry::Tool(Tool { id: id.into(), name: name.into(), run: run.map(Box::new), ..Default::default() })
    }

    #[test]
    fn work_is_listed_in_the_order_it_started() {
        let entries = vec![
            tool("a", "Agent", Some(Run::default())),
            tool("b", "Bash", None),
            tool("c", "Bash", Some(Run { background: true, ..Default::default() })),
            tool("w", "Monitor", Some(Run { background: true, ..Default::default() })),
            tool("s", "ScheduleWakeup", None),
            tool("d", "Bash", Some(Run { background: true, state: RunState::Done, ended: 5, ..Default::default() })),
        ];
        let status = Status {
            timers: vec![
                Timer { kind: TimerKind::Job, id: "j".into(), tool: "gone".into(), ..Default::default() },
                Timer { kind: TimerKind::Watch, id: "t".into(), tool: "w".into(), ..Default::default() },
                Timer { kind: TimerKind::Wakeup, tool: "s".into(), ..Default::default() },
            ],
            ..Default::default()
        };
        let work = background_work(&entries, &status);
        let kinds: Vec<_> = work.iter().map(|w| (w.kind, w.entry)).collect();
        assert_eq!(
            kinds,
            [
                (WorkKind::Agent, Some(0)),
                (WorkKind::Command, Some(2)),
                (WorkKind::Watch, Some(3)),
                (WorkKind::Wakeup, Some(4)),
                (WorkKind::Command, Some(5)),
                (WorkKind::Job, None),
            ]
        );
        // The watch is one item: its run and its timer.
        assert!(work[2].timer.is_some_and(|t| t.id == "t"));
        // The command that ended is listed, not counted.
        assert_eq!(work[4].ended(), 5);
        assert_eq!(background_count(&entries, &status), 5);
    }

    /// A command going on in the background shows the last line it wrote
    /// as what it does now; once it ended, that stays as it ended.
    #[test]
    fn a_command_shows_its_last_output() {
        let mut t = crate::Transcript::new("/p");
        let run = |state| Some(Run { background: true, state, output: "/tmp/out".into(), last: "Build it".into(), ..Default::default() });
        t.entries = vec![tool("c", "Bash", run(RunState::Running)), tool("d", "Bash", run(RunState::Done))];
        assert_eq!(t.output_files(), [(0, "/tmp/out".to_string())]);
        assert!(t.set_output_line(0, "Compiling shed".into()));
        assert!(!t.set_output_line(0, "Compiling shed".into()), "the same line again changes nothing");
        assert!(!t.set_output_line(1, "late".into()), "an ended command keeps how it ended");
        assert!(matches!(&t.entries[0], Entry::Tool(x) if x.run.as_ref().unwrap().last == "Compiling shed"));
        assert_eq!(t.background_count(), 1);
    }

    #[test]
    fn output_lines_read_as_shown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out");
        std::fs::write(&path, "first\n\x1b[32mCompiling\x1b[0m shed v1\r\n 10%\r 55%\r\n\n").unwrap();
        assert_eq!(last_output_line(&path, 80).as_deref(), Some("55%"));
        std::fs::write(&path, "\x1b]0;title\x07Listening on :8080\n").unwrap();
        assert_eq!(last_output_line(&path, 9).as_deref(), Some("Listening…"));
        std::fs::write(&path, "").unwrap();
        assert_eq!(last_output_line(&path, 80), None);
        assert_eq!(last_output_line(&dir.path().join("none"), 80), None);
    }
}
