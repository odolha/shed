//! Sub-agents end to end, with a stand-in Claude Code playing a recording:
//! an agent's steps reach the window under its tool, never as entries of
//! their own; a background agent working on after the turn changes its
//! entry alone (`AiEntries`); its end starts a turn by itself, with a note.

mod common;

use std::os::unix::fs::PermissionsExt;

use common::{Client, start_in};
use shed_ai::{Entry, RunState};
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

#[test]
fn agents_work_under_their_tool() {
    let dir = tempfile::tempdir().unwrap();
    let stream = concat!(env!("CARGO_MANIFEST_DIR"), "/../shed-ai/tests/streams/agents.jsonl");
    // Plays the recording for the prompt; after its first turn ends, slowly,
    // as the background agent's steps came.
    let fake = dir.path().join("claude");
    std::fs::write(
        &fake,
        format!(
            r#"#!/usr/bin/env python3
import json, sys, time
for line in sys.stdin:
    m = json.loads(line)
    if m.get("type") == "control_request":
        print(json.dumps({{"type": "control_response", "response": {{"subtype": "success", "request_id": m["request_id"], "response": {{}}}}}}), flush=True)
    elif m.get("type") == "user":
        slow = False
        for rec in open("{stream}"):
            if slow:
                time.sleep(0.08)
            print(rec.strip(), flush=True)
            slow = slow or '"type":"result"' in rec
"#
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    // SAFETY: set before the keeper starts; this file has one test.
    unsafe { std::env::set_var("SHED_CLAUDE", &fake) };

    let env = start_in(dir.path(), "shedd.sock");
    let mut c = Client::connect(&env.socket);
    c.msg(ClientMsg::Show(ShowTarget::NewBench { name: Some("agents".into()), cwd: None }));
    let bench: BenchId = c.until("show", |f, _| match f {
        Frame::Keeper(KeeperMsg::Show { bench }) => Some(*bench),
        _ => None,
    });
    c.msg(ClientMsg::Attach { bench: Some(bench) });
    c.msg(ClientMsg::NewAiTab { bench, folder: dir.path().to_path_buf() });
    let tree = tree_until(&mut c, "ai tab", |t| t.bench(bench).is_some_and(|b| b.tabs.iter().any(|t| t.ai_info().is_some())));
    let tab = tree.bench(bench).unwrap().tabs.iter().find(|t| t.ai_info().is_some()).unwrap().id;
    c.msg(ClientMsg::AiWatch { tab });
    c.msg(ClientMsg::AiSend { tab, text: "count, and echo in the background".into(), images: Vec::new() });

    // The window's copy of the conversation, as it follows both messages.
    let mut entries: Vec<Entry> = Vec::new();
    let mut alone = 0;
    let echoed = |e: &[Entry]| {
        e.iter().any(|x| matches!(x, Entry::Tool(t) if t.summary == "Agent: Echo twice" && t.run.as_ref().is_some_and(|r| r.state == RunState::Done && r.steps.len() == 3)))
    };
    let finished_twice = |e: &[Entry]| e.iter().filter(|x| matches!(x, Entry::Notice { text, .. } if text.starts_with("Done"))).count() == 2;
    c.until("the agent's work and the turn it started", |f, _| {
        match f {
            Frame::Keeper(KeeperMsg::AiUpdate { tab: t, from, entries: e, .. }) if *t == tab => {
                entries.truncate(*from as usize);
                entries.extend(e.iter().cloned());
            }
            Frame::Keeper(KeeperMsg::AiEntries { tab: t, entries: changed, .. }) if *t == tab => {
                for (i, e) in changed {
                    alone += 1;
                    entries[*i as usize] = e.clone();
                }
            }
            _ => {}
        }
        (echoed(&entries) && finished_twice(&entries)).then_some(())
    });
    assert!(alone > 0, "the agent's later steps came alone");
    let users: Vec<&str> = entries.iter().filter_map(|e| if let Entry::User { text, .. } = e { Some(text.as_str()) } else { None }).collect();
    assert_eq!(users, ["count, and echo in the background"], "sub-agents' prompts are theirs");
    let said: Vec<&str> = entries.iter().filter_map(|e| if let Entry::Assistant { text, .. } = e { Some(text.as_str()) } else { None }).collect();
    assert_eq!(said, ["Done.", "Done."], "sub-agents' words are theirs");
    assert!(entries.iter().any(|e| matches!(e, Entry::Note(n) if n.text == "Agent “Echo twice” finished")));
    tree_until(&mut c, "idle again", |t| t.tab(tab).and_then(|t| t.ai_info()).is_some_and(|a| a.activity == AiActivity::Idle));
}
