//! Opening another session in an AI tab while Claude is working: the
//! working one stops, and the new one answers (a stopped process's last
//! events and its exit must not reach the new one).

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

#[test]
fn another_session_opened_mid_turn_answers() {
    let dir = tempfile::tempdir().unwrap();
    let stream = concat!(env!("CARGO_MANIFEST_DIR"), "/../shed-ai/tests/streams/edit-and-bash.jsonl");
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
    elif m.get("type") == "control_request" and m["request"]["subtype"] == "mcp_set_servers":
        servers = m["request"]["servers"]
        log.write("mcp_set_servers %s\n" % " ".join(k + ":" + " ".join(v["args"]) for k, v in servers.items())); log.flush()
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
    c.msg(ClientMsg::Show(ShowTarget::NewBench { name: Some("ai".into()), cwd: None }));
    let bench: BenchId = c.until("show", |f, _| match f {
        Frame::Keeper(KeeperMsg::Show { bench }) => Some(*bench),
        _ => None,
    });
    c.msg(ClientMsg::Attach { bench: Some(bench) });
    tree_until(&mut c, "bench", |t| t.bench(bench).is_some());
    c.msg(ClientMsg::NewAiTab { bench, folder: dir.path().to_path_buf() });
    let tree = tree_until(&mut c, "ai tab", |t| t.bench(bench).is_some_and(|b| b.tabs.iter().any(|t| t.ai_info().is_some())));
    let tab = tree.bench(bench).unwrap().tabs.iter().find(|t| t.ai_info().is_some()).unwrap().id;
    c.msg(ClientMsg::AiWatch { tab });

    // A slow turn under way…
    c.msg(ClientMsg::AiSend { tab, text: "slow, please".into(), images: Vec::new() });
    tree_until(&mut c, "working", |t| t.tab(tab).and_then(|t| t.ai_info()).is_some_and(|a| a.activity == AiActivity::Working));
    // …then a new session, and a prompt in it.
    c.msg(ClientMsg::AiOpenSession { tab, session: None });
    tree_until(&mut c, "new session", |t| t.tab(tab).and_then(|t| t.ai_info()).is_some_and(|a| a.session.is_none() && a.activity == AiActivity::Idle));
    // (Give the stopped process time to go, as it does in real use.)
    std::thread::sleep(std::time::Duration::from_millis(300));
    c.msg(ClientMsg::AiSend { tab, text: "count the lines".into(), images: Vec::new() });
    let mut entries: Vec<Entry> = Vec::new();
    c.until("the new session's answer", |f, _| {
        if let Frame::Keeper(KeeperMsg::AiUpdate { tab: t, from, entries: e, .. }) = f
            && *t == tab
        {
            entries.truncate(*from as usize);
            entries.extend(e.iter().cloned());
        }
        entries.iter().any(|e| matches!(e, Entry::Notice { text, .. } if text.starts_with("Done"))).then_some(())
    });
    assert!(matches!(&entries[0], Entry::User { text, .. } if text == "count the lines"), "{:?}", &entries[..entries.len().min(3)]);
    assert!(entries.iter().any(|e| matches!(e, Entry::Assistant { text, .. } if text.contains("3 lines"))));
    assert!(!entries.iter().any(|e| matches!(e, Entry::Notice { text, .. } if text.starts_with("Claude Code stopped"))), "{entries:?}");
}
