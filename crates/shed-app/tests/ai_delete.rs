//! Deleting a Claude Code session from an AI tab: its files, and those of
//! the sessions going on from it, move to Shed's trash; refused while
//! another tab shows one of them; the tab showing it goes to a new session.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

use common::{Client, start_in};
use shed_model::{BenchId, Draft, Shed, TabId};
use shed_proto::{ClientMsg, Frame, KeeperMsg, ShowTarget};

fn tree_until(c: &mut Client, what: &str, ok: impl Fn(&Shed) -> bool) -> Shed {
    if ok(&c.tree) {
        return c.tree.clone();
    }
    c.until(what, |f, _| match f {
        Frame::Keeper(KeeperMsg::TreeChanged(t)) if ok(t) => Some(t.clone()),
        _ => None,
    })
}

/// A session file: a prompt and its answer, the prompt following `after`
/// (another session's last message: this one continues it).
fn write_session(dir: &Path, id: &str, started: &str, prompt: (&str, &str), answer: &str, after: Option<&str>) {
    let parent = after.map_or("null".to_string(), |p| format!("\"{p}\""));
    let (uuid, text) = prompt;
    let lines = [
        format!(r#"{{"type":"user","uuid":"{uuid}","parentUuid":{parent},"timestamp":"{started}","sessionId":"{id}","message":{{"role":"user","content":"{text}"}}}}"#),
        format!(r#"{{"type":"assistant","uuid":"{answer}","parentUuid":"{uuid}","timestamp":"{started}","sessionId":"{id}","message":{{"role":"assistant","content":[{{"type":"text","text":"ok"}}]}}}}"#),
    ];
    std::fs::write(dir.join(format!("{id}.jsonl")), lines.join("\n") + "\n").unwrap();
}

fn reply(c: &mut Client, what: &str) -> Result<String, String> {
    c.until(what, |f, _| match f {
        Frame::Keeper(KeeperMsg::Done(d)) => Some(Ok(d.clone())),
        Frame::Keeper(KeeperMsg::Error(e)) => Some(Err(e.clone())),
        _ => None,
    })
}

fn new_ai_tab(c: &mut Client, bench: BenchId, folder: &Path) -> TabId {
    let before: Vec<TabId> = c.tree.bench(bench).map(|b| b.tabs.iter().map(|t| t.id).collect()).unwrap_or_default();
    c.msg(ClientMsg::NewAiTab { bench, folder: folder.to_path_buf() });
    let tree = tree_until(c, "an ai tab", |t| t.bench(bench).is_some_and(|b| b.tabs.iter().any(|t| t.ai_info().is_some() && !before.contains(&t.id))));
    tree.bench(bench).unwrap().tabs.iter().find(|t| t.ai_info().is_some() && !before.contains(&t.id)).unwrap().id
}

#[test]
fn a_deleted_session_goes_to_the_trash() {
    let dir = tempfile::tempdir().unwrap();
    // Claude Code's sessions in a folder of the test's, never the user's.
    let claude_dir = dir.path().join("claude-config");
    // SAFETY: set before the keeper starts; only this test (its own process) sets it.
    unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", &claude_dir) };
    let folder = dir.path().join("project");
    std::fs::create_dir_all(&folder).unwrap();
    let sessions = shed_ai::sessions::sessions_dir(&folder);
    assert!(sessions.starts_with(&claude_dir));
    std::fs::create_dir_all(sessions.join("a/subagents")).unwrap();
    std::fs::write(sessions.join("a/subagents/agent-1.jsonl"), "{}\n").unwrap();
    // `a`, `b` continuing it, and `z` on its own.
    write_session(&sessions, "a", "2026-11-13T08:00:00Z", ("p1", "plan the shelves"), "r1", None);
    write_session(&sessions, "b", "2026-11-13T09:00:00Z", ("p2", "and the drawers"), "r2", Some("r1"));
    write_session(&sessions, "z", "2026-11-12T08:00:00Z", ("x1", "something else"), "y1", None);

    // A stand-in for `claude` that stays up and says who it is.
    let fake = dir.path().join("claude");
    let pid_file = dir.path().join("claude.pid");
    std::fs::write(
        &fake,
        format!(
            r#"#!/usr/bin/env python3
import json, os, sys
open("{pid}", "w").write(str(os.getpid()))
for line in sys.stdin:
    m = json.loads(line)
    if m.get("type") == "control_request" and m["request"]["subtype"] == "initialize":
        print(json.dumps({{"type": "control_response", "response": {{"subtype": "success", "request_id": m["request_id"], "response": {{"commands": [], "models": []}}}}}}), flush=True)
"#,
            pid = pid_file.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    // SAFETY: only the keeper's AI code reads it, and only this test sets it.
    unsafe { std::env::set_var("SHED_CLAUDE", &fake) };

    let env = start_in(dir.path(), "shedd.sock");
    let mut c = Client::connect(&env.socket);
    c.msg(ClientMsg::Show(ShowTarget::NewBench { name: Some("ai".into()), cwd: None }));
    let bench: BenchId = c.until("show", |f, _| match f {
        Frame::Keeper(KeeperMsg::Show { bench }) => Some(*bench),
        _ => None,
    });
    c.msg(ClientMsg::Attach { bench: Some(bench) });
    tree_until(&mut c, "bench", |t| t.bench(bench).is_some());
    let here = new_ai_tab(&mut c, bench, &folder);
    let other = new_ai_tab(&mut c, bench, &folder);
    let ai = |c: &Client, tab: TabId| c.tree.tab(tab).and_then(|t| t.ai_info()).cloned().unwrap();

    // This tab shows `a`, with drafts for a, b and z; the other shows `b`.
    c.msg(ClientMsg::AiOpenSession { tab: here, session: Some("a".into()) });
    c.msg(ClientMsg::AiOpenSession { tab: other, session: Some("b".into()) });
    tree_until(&mut c, "sessions shown", |t| {
        let shows = |tab, s: &str| t.tab(tab).and_then(|t| t.ai_info()).is_some_and(|a| a.session.as_deref() == Some(s));
        shows(here, "a") && shows(other, "b")
    });
    let mut with_drafts = ai(&c, here);
    with_drafts.drafts = ["a", "b", "z"].iter().map(|s| Draft { session: s.to_string(), text: format!("later, for {s}") }).collect();
    c.msg(ClientMsg::AiSettings { tab: here, settings: with_drafts });
    tree_until(&mut c, "drafts", |t| t.tab(here).and_then(|t| t.ai_info()).is_some_and(|a| a.drafts.len() == 3));

    // `b` goes on from `a`: shown in the other tab, so `a` cannot go.
    c.msg(ClientMsg::AiDeleteSession { tab: here, session: "a".into() });
    let refused = reply(&mut c, "refused").unwrap_err();
    assert!(refused.contains("another tab"), "{refused}");
    assert!(sessions.join("a.jsonl").is_file() && sessions.join("b.jsonl").is_file());
    assert_eq!(ai(&c, here).drafts.len(), 3);

    // The other tab moves on; this one's Claude Code runs.
    c.msg(ClientMsg::AiOpenSession { tab: other, session: Some("z".into()) });
    tree_until(&mut c, "other on z", |t| t.tab(other).and_then(|t| t.ai_info()).is_some_and(|a| a.session.as_deref() == Some("z")));
    c.msg(ClientMsg::AiStart { tab: here });
    let deadline = Instant::now() + Duration::from_secs(10);
    let pid: i32 = loop {
        if let Some(pid) = std::fs::read_to_string(&pid_file).ok().and_then(|s| s.trim().parse().ok()) {
            break pid;
        }
        assert!(Instant::now() < deadline, "Claude Code never started");
        std::thread::sleep(Duration::from_millis(20));
    };

    c.msg(ClientMsg::AiDeleteSession { tab: here, session: "a".into() });
    let done = reply(&mut c, "deleted").unwrap();
    assert!(done.contains("1 later session"), "{done}");
    // The tab is on a new session, and the drafts of the deleted ones went.
    let tree = tree_until(&mut c, "a new session", |t| t.tab(here).and_then(|t| t.ai_info()).is_some_and(|a| a.session.is_none()));
    let a = tree.tab(here).unwrap().ai_info().unwrap();
    assert_eq!(a.drafts.iter().map(|d| d.session.as_str()).collect::<Vec<_>>(), ["z"]);
    assert_eq!(a.draft_session, None);
    // Its Claude Code stopped.
    let deadline = Instant::now() + Duration::from_secs(10);
    // SAFETY: signal 0 only asks whether the process is there.
    while unsafe { libc::kill(pid, 0) } == 0 {
        assert!(Instant::now() < deadline, "Claude Code still runs");
        std::thread::sleep(Duration::from_millis(20));
    }
    // Both files (and a's folder) in the trash, with how to put them back.
    assert!(!sessions.join("a.jsonl").exists() && !sessions.join("a").exists() && !sessions.join("b.jsonl").exists());
    assert!(sessions.join("z.jsonl").is_file());
    let trash: Vec<_> = std::fs::read_dir(dir.path().join("state/ai-trash")).unwrap().flatten().map(|e| e.path()).collect();
    assert_eq!(trash.len(), 1, "{trash:?}");
    let kept = &trash[0];
    assert!(kept.file_name().unwrap().to_string_lossy().ends_with("-a"), "{kept:?}");
    assert!(kept.join("a.jsonl").is_file() && kept.join("b.jsonl").is_file() && kept.join("a/subagents/agent-1.jsonl").is_file());
    let restore = std::fs::read_to_string(kept.join("restore.txt")).unwrap();
    assert!(restore.contains(&sessions.join("b.jsonl").display().to_string()), "{restore}");
    assert!(shed_ai::list_sessions(&folder).iter().all(|s| s.id == "z"));

    // `z`, shown in the other tab: refused; and one that is not there.
    c.msg(ClientMsg::AiDeleteSession { tab: here, session: "z".into() });
    assert!(reply(&mut c, "z refused").is_err());
    c.msg(ClientMsg::AiDeleteSession { tab: here, session: "../z".into() });
    assert!(reply(&mut c, "not a session").is_err());
    c.msg(ClientMsg::AiDeleteSession { tab: here, session: "gone".into() });
    assert!(reply(&mut c, "no such session").is_err());
    assert!(sessions.join("z.jsonl").is_file());
}
