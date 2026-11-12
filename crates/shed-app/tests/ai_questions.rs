//! Claude's questions (its AskUserQuestion tool) end to end, with a stand-in
//! Claude Code: the question reaches the window as such, the answers go back
//! in the tool's input as Claude Code reads them, and the card keeps them.

mod common;

use std::os::unix::fs::PermissionsExt;

use common::{Client, start_in};
use shed_ai::Entry;
use shed_model::{BenchId, Shed};
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
fn claude_asks_and_the_answers_go_back() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("answers.json");
    // Asks two questions on the first prompt; writes what comes back.
    let fake = dir.path().join("claude");
    std::fs::write(
        &fake,
        format!(
            r#"#!/usr/bin/env python3
import json, sys
ask = {{"questions": [
  {{"question": "Which database?", "header": "DB", "multiSelect": False, "options": [{{"label": "Postgres (Recommended)", "description": "In use"}}, {{"label": "SQLite", "description": "Simple"}}]}},
  {{"question": "Which checks?", "header": "CI", "multiSelect": True, "options": [{{"label": "lint", "description": ""}}, {{"label": "tests", "description": ""}}]}}]}}
for line in sys.stdin:
    m = json.loads(line)
    if m.get("type") == "control_request":
        print(json.dumps({{"type": "control_response", "response": {{"subtype": "success", "request_id": m["request_id"], "response": {{}}}}}}), flush=True)
    elif m.get("type") == "user":
        print(json.dumps({{"type": "control_request", "request_id": "ask-1", "request": {{"subtype": "can_use_tool", "tool_name": "AskUserQuestion", "input": ask}}}}), flush=True)
    elif m.get("type") == "control_response":
        open("{log}", "w").write(json.dumps(m["response"]["response"]))
        print(json.dumps({{"type": "result", "subtype": "success", "duration_ms": 10}}), flush=True)
"#,
            log = log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    // SAFETY: set before the keeper starts; this file has one test.
    unsafe { std::env::set_var("SHED_CLAUDE", &fake) };

    let env = start_in(dir.path(), "shedd.sock");
    let mut c = Client::connect(&env.socket);
    c.msg(ClientMsg::Show(ShowTarget::NewBench { name: Some("ask".into()), cwd: None }));
    let bench: BenchId = c.until("show", |f, _| match f {
        Frame::Keeper(KeeperMsg::Show { bench }) => Some(*bench),
        _ => None,
    });
    c.msg(ClientMsg::Attach { bench: Some(bench) });
    c.msg(ClientMsg::NewAiTab { bench, folder: dir.path().to_path_buf() });
    let tree = tree_until(&mut c, "ai tab", |t| t.bench(bench).is_some_and(|b| b.tabs.iter().any(|t| t.ai_info().is_some())));
    let tab = tree.bench(bench).unwrap().tabs.iter().find(|t| t.ai_info().is_some()).unwrap().id;
    c.msg(ClientMsg::AiWatch { tab });
    c.msg(ClientMsg::AiSend { tab, text: "set it up".into(), images: Vec::new() });

    let mut entries: Vec<Entry> = Vec::new();
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
    let asked = |e: &[Entry]| e.iter().any(|x| matches!(x, Entry::Permission(p) if p.questions.len() == 2));
    follow(&mut c, &mut entries, "the questions", &asked);
    let Some(Entry::Permission(p)) = entries.iter().find(|x| matches!(x, Entry::Permission(_))) else { unreachable!() };
    assert_eq!((p.questions[0].header.as_str(), p.questions[1].multi), ("DB", true));

    c.msg(ClientMsg::AiAnswerQuestions { tab, request: p.request.clone(), answers: vec!["SQLite".into(), "lint, tests".into()] });
    let answered = |e: &[Entry]| e.iter().any(|x| matches!(x, Entry::Permission(p) if p.answer == Some(true) && p.answers.len() == 2));
    follow(&mut c, &mut entries, "the answers kept", &answered);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !log.exists() {
        assert!(std::time::Instant::now() < deadline, "no answer reached Claude");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let sent: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&log).unwrap()).unwrap();
    assert_eq!(sent["behavior"], "allow");
    assert_eq!(sent["updatedInput"]["answers"]["Which database?"], "SQLite");
    assert_eq!(sent["updatedInput"]["answers"]["Which checks?"], "lint, tests");
    assert_eq!(sent["updatedInput"]["questions"][0]["header"], "DB", "the questions go back too");
}
