//! AI tabs end to end, with a recorded Claude Code standing in for the real
//! one: a prompt goes in, the conversation comes back to the window, and the
//! session is kept in the tab.

mod common;

use std::os::unix::fs::PermissionsExt;

use common::{Client, start_in};
use shed_ai::Entry;
use shed_model::{AiActivity, BenchId, Shed};
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

fn show_and_attach(c: &mut Client, target: ShowTarget) -> BenchId {
    c.msg(ClientMsg::Show(target));
    let bench = c.until("show", |f, _| match f {
        Frame::Keeper(KeeperMsg::Show { bench }) => Some(*bench),
        _ => None,
    });
    c.msg(ClientMsg::Attach { bench: Some(bench) });
    bench
}

#[test]
fn a_prompt_comes_back_as_a_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let stream = concat!(env!("CARGO_MANIFEST_DIR"), "/../shed-ai/tests/streams/edit-and-bash.jsonl");
    // A stand-in for `claude`: answers `initialize`, then plays the recording for any prompt.
    let fake = dir.path().join("claude");
    std::fs::write(
        &fake,
        format!(
            r#"#!/usr/bin/env python3
import json, sys, time
log = open("{log}", "a")
log.write("start " + " ".join(sys.argv[1:]) + "\n"); log.flush()
for line in sys.stdin:
    m = json.loads(line)
    if m.get("type") == "control_request" and m["request"]["subtype"] == "initialize":
        reply = {{"commands": [{{"name": "review", "description": "Review a change", "argumentHint": "[pr]"}}], "models": []}}
        print(json.dumps({{"type": "control_response", "response": {{"subtype": "success", "request_id": m["request_id"], "response": reply}}}}), flush=True)
    elif m.get("type") == "control_request" and m["request"]["subtype"] == "rewind_files":
        log.write("rewind " + m["request"]["user_message_id"] + "\n"); log.flush()
        print(json.dumps({{"type": "control_response", "response": {{"subtype": "success", "request_id": m["request_id"], "response": {{"canRewind": True}}}}}}), flush=True)
    elif m.get("type") == "user":
        log.write("user " + m.get("uuid", "") + "\n"); log.flush()
        # A slow turn, for messages sent meanwhile to wait.
        if "slow" in json.dumps(m["message"]["content"]):
            time.sleep(1.5)
        for rec in open("{stream}"):
            if '"control_response"' not in rec:
                print(rec.strip(), flush=True)
"#,
            log = dir.path().join("claude.log").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    // SAFETY: only the keeper's AI code reads it, and only this test sets it.
    unsafe { std::env::set_var("SHED_CLAUDE", &fake) };

    let env = start_in(dir.path(), "shedd.sock");
    let mut c = Client::connect(&env.socket);
    let bench = show_and_attach(&mut c, ShowTarget::NewBench { name: Some("ai".into()), cwd: None });
    tree_until(&mut c, "bench", |t| t.bench(bench).is_some());
    c.msg(ClientMsg::NewAiTab { bench, folder: dir.path().to_path_buf() });
    let tree = tree_until(&mut c, "ai tab", |t| t.bench(bench).is_some_and(|b| b.tabs.iter().any(|t| t.ai_info().is_some())));
    let tab = tree.bench(bench).unwrap().tabs.iter().find(|t| t.ai_info().is_some()).unwrap().id;

    c.msg(ClientMsg::AiWatch { tab });
    c.until("empty conversation", |f, _| matches!(f, Frame::Keeper(KeeperMsg::AiUpdate { from: 0, entries, .. }) if entries.is_empty()).then_some(()));
    c.msg(ClientMsg::AiSend { tab, text: "count the lines".into(), images: Vec::new() });
    let mut entries: Vec<Entry> = Vec::new();
    // The conversation may start coming before the commands do: keep it.
    c.until("commands", |f, _| {
        if let Frame::Keeper(KeeperMsg::AiUpdate { tab: t, from, entries: e, .. }) = f
            && *t == tab
        {
            entries.truncate(*from as usize);
            entries.extend(e.iter().cloned());
        }
        matches!(f, Frame::Keeper(KeeperMsg::AiInfo { commands, .. }) if commands[0].name == "review").then_some(())
    });
    let mut limits = false;
    c.until("the answer", |f, _| {
        if let Frame::Keeper(KeeperMsg::AiUpdate { tab: t, from, entries: e, .. }) = f
            && *t == tab
        {
            entries.truncate(*from as usize);
            entries.extend(e.iter().cloned());
        }
        limits |= matches!(f, Frame::Keeper(KeeperMsg::AiLimits(l)) if l.five_hour.is_some());
        entries.iter().any(|e| matches!(e, Entry::Notice { text, .. } if text.starts_with("Done"))).then_some(())
    });
    assert!(limits, "the usage limits were passed on");
    assert!(matches!(&entries[0], Entry::User { text, .. } if text == "count the lines"), "{:?}", &entries[..entries.len().min(3)]);
    assert!(entries.iter().any(|e| matches!(e, Entry::Tool(t) if t.name == "Edit")));
    assert!(entries.iter().any(|e| matches!(e, Entry::Assistant { text, .. } if text.contains("3 lines"))));
    // The session is the tab's now, and it is idle again.
    let tree = tree_until(&mut c, "session kept", |t| {
        t.tab(tab).and_then(|t| t.ai_info()).is_some_and(|a| a.session.is_some() && a.activity == AiActivity::Idle)
    });
    assert!(tree.tab(tab).unwrap().ai_info().unwrap().session.is_some());

    // A second prompt, then edited: the files go back to before it, and
    // Claude Code starts again from the message before it, as a new session.
    let dones = |entries: &[Entry]| entries.iter().filter(|e| matches!(e, Entry::Notice { text, .. } if text.starts_with("Done"))).count();
    let follow = |c: &mut Client, entries: &mut Vec<Entry>, what: &str, done: &dyn Fn(&[Entry]) -> bool| {
        c.until(what, |f, _| {
            if let Frame::Keeper(KeeperMsg::AiUpdate { tab: t, from, entries: e, .. }) = f
                && *t == tab
            {
                entries.truncate(*from as usize);
                entries.extend(e.iter().cloned());
            }
            done(entries).then_some(())
        })
    };
    c.msg(ClientMsg::AiSend { tab, text: "count again".into(), images: Vec::new() });
    follow(&mut c, &mut entries, "the second answer", &|e| dones(e) == 2);
    let second = entries.iter().rposition(|e| matches!(e, Entry::User { .. })).unwrap();
    c.msg(ClientMsg::AiEdit { tab, entry: second as u32, text: "count once more".into(), images: Vec::new() });
    follow(&mut c, &mut entries, "the edited answer", &|e| {
        e.iter().any(|x| matches!(x, Entry::User { text, .. } if text == "count once more")) && dones(e) == 2
    });
    let prompts: Vec<&str> = entries.iter().filter_map(|e| if let Entry::User { text, .. } = e { Some(text.as_str()) } else { None }).collect();
    assert_eq!(prompts, ["count the lines", "count once more"]);
    assert!(entries.iter().any(|e| matches!(e, Entry::Notice { text, .. } if text.starts_with("Went back"))));
    let log = std::fs::read_to_string(dir.path().join("claude.log")).unwrap();
    let users: Vec<&str> = log.lines().filter_map(|l| l.strip_prefix("user ")).collect();
    assert_eq!(users.len(), 3, "{log}");
    assert!(log.contains(&format!("rewind {}", users[1])), "the second prompt's files go back: {log}");
    let last_assistant = std::fs::read_to_string(stream)
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["type"] == "assistant")
        .filter_map(|v| v["uuid"].as_str().map(String::from))
        .next_back()
        .unwrap();
    let last_start = log.lines().rfind(|l| l.starts_with("start ")).unwrap();
    assert!(last_start.contains(&format!("--resume-session-at {last_assistant} --fork-session")), "{log}");

    // Sent while Claude is busy: it waits in the tab's queue (kept with the
    // bench), then goes when the turn ends.
    c.msg(ClientMsg::AiSend { tab, text: "slow, please".into(), images: Vec::new() });
    c.msg(ClientMsg::AiSend { tab, text: "and then this".into(), images: Vec::new() });
    tree_until(&mut c, "queued", |t| {
        t.tab(tab).and_then(|t| t.ai_info()).is_some_and(|a| a.queued.iter().map(|q| q.text.as_str()).eq(["and then this"]))
    });
    tree_until(&mut c, "sent from the queue", |t| t.tab(tab).and_then(|t| t.ai_info()).is_some_and(|a| a.queued.is_empty()));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::fs::read_to_string(dir.path().join("claude.log")).unwrap().lines().filter(|l| l.starts_with("user ")).count() < 5 {
        assert!(std::time::Instant::now() < deadline, "the queued message never reached Claude Code");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    // Closing the tab stops it.
    c.msg(ClientMsg::CloseTab { tab });
    tree_until(&mut c, "closed", |t| t.tab(tab).is_none());
}
