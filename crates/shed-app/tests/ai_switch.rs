//! Opening another session in an AI tab while Claude is working: the
//! working one goes on in the background (its process kept), the new one
//! answers on its own (the other's events never reach it), and opening the
//! first again shows it as it goes, process and all.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use common::{Client, start_in};
use shed_ai::Entry;
use shed_model::{AiActivity, AiTab, BenchId, Shed, TabId};
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

fn ai_until(c: &mut Client, tab: TabId, what: &str, ok: impl Fn(&AiTab) -> bool) -> AiTab {
    let t = tree_until(c, what, |t| t.tab(tab).and_then(|t| t.ai_info()).is_some_and(&ok));
    t.tab(tab).and_then(|t| t.ai_info()).unwrap().clone()
}

/// The tab's conversation as the keeper sends it, until `done` says it has what is wanted.
/// `entries`: what it had sent before.
fn conversation_until(c: &mut Client, tab: TabId, entries: Vec<Entry>, what: &str, done: impl Fn(&[Entry]) -> bool) -> Vec<Entry> {
    let mut entries = entries;
    c.until(what, |f, _| {
        match f {
            Frame::Keeper(KeeperMsg::AiUpdate { tab: t, from, entries: e, .. }) if *t == tab => {
                entries.truncate(*from as usize);
                entries.extend(e.iter().cloned());
            }
            Frame::Keeper(KeeperMsg::AiEntries { tab: t, entries: e, .. }) if *t == tab => {
                for (i, entry) in e {
                    if let Some(slot) = entries.get_mut(*i as usize) {
                        *slot = entry.clone();
                    }
                }
            }
            _ => {}
        }
        done(&entries).then(|| entries.clone())
    })
}

fn done(entries: &[Entry]) -> bool {
    entries.iter().any(|e| matches!(e, Entry::Notice { text, .. } if text.starts_with("Done")))
}

fn stopped(entries: &[Entry]) -> bool {
    entries.iter().any(|e| matches!(e, Entry::Notice { text, .. } if text.starts_with("Claude Code stopped")))
}

/// A Claude Code that answers with a recorded turn, as a session of its
/// own (`--resume`'s, else a new id). A prompt saying "slow" says who it is
/// at once, then waits for the file `gate` in its folder before going on.
/// It logs to `claude.log` there. One for every test here (they run at once,
/// and the variable naming it is the process's).
fn fake_claude() {
    static FAKE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    let fake = FAKE.get_or_init(|| {
        let stream = concat!(env!("CARGO_MANIFEST_DIR"), "/../shed-ai/tests/streams/edit-and-bash.jsonl");
        let dir = tempfile::tempdir().unwrap().keep();
        let fake = dir.join("claude");
        std::fs::write(
            &fake,
            format!(
                r#"#!/usr/bin/env python3
import json, os, sys, time, uuid
log = open("claude.log", "a")
args = sys.argv[1:]
sid = args[args.index("--resume") + 1] if "--resume" in args else str(uuid.uuid4())
log.write("start " + sid + "\n"); log.flush()
recs = [r.strip().replace("bacce851-a991-4454-b02a-2d0dca87d701", sid) for r in open("{stream}") if '"control_response"' not in r]
init = next(i for i, r in enumerate(recs) if '"subtype":"init"' in r)
for line in sys.stdin:
    m = json.loads(line)
    if m.get("type") == "control_request" and m["request"]["subtype"] == "initialize":
        reply = {{"commands": [], "models": []}}
        print(json.dumps({{"type": "control_response", "response": {{"subtype": "success", "request_id": m["request_id"], "response": reply}}}}), flush=True)
    elif m.get("type") == "user":
        text = json.dumps(m["message"]["content"])
        log.write("user " + sid + " " + text + "\n"); log.flush()
        rest = recs
        if "slow" in text:
            for r in recs[:init + 1]:
                print(r, flush=True)
            rest = recs[init + 1:]
            while not os.path.exists("gate"):
                time.sleep(0.05)
        for r in rest:
            print(r, flush=True)
"#
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        fake
    });
    // SAFETY: only the keeper's AI code reads it, and every test here sets it the same.
    unsafe { std::env::set_var("SHED_CLAUDE", fake) };
}

/// A keeper with an AI tab in `dir`, watched by the client.
fn ai_tab(dir: &Path) -> (common::Env, Client, TabId) {
    let env = start_in(dir, "shedd.sock");
    let mut c = Client::connect(&env.socket);
    c.msg(ClientMsg::Show(ShowTarget::NewBench { name: Some("ai".into()), cwd: None }));
    let bench: BenchId = c.until("show", |f, _| match f {
        Frame::Keeper(KeeperMsg::Show { bench }) => Some(*bench),
        _ => None,
    });
    c.msg(ClientMsg::Attach { bench: Some(bench) });
    tree_until(&mut c, "bench", |t| t.bench(bench).is_some());
    c.msg(ClientMsg::NewAiTab { bench, folder: dir.to_path_buf() });
    let tree = tree_until(&mut c, "ai tab", |t| t.bench(bench).is_some_and(|b| b.tabs.iter().any(|t| t.ai_info().is_some())));
    let tab = tree.bench(bench).unwrap().tabs.iter().find(|t| t.ai_info().is_some()).unwrap().id;
    c.msg(ClientMsg::AiWatch { tab });
    (env, c, tab)
}

/// A slow turn under way in a new session, then a new session shown: the
/// slow one's id.
fn slow_then_new_session(c: &mut Client, tab: TabId) -> String {
    c.msg(ClientMsg::AiSend { tab, text: "slow, please".into(), images: Vec::new() });
    let ai = ai_until(c, tab, "working in its session", |a| a.session.is_some() && a.activity == AiActivity::Working);
    let slow = ai.session.unwrap();
    c.msg(ClientMsg::AiOpenSession { tab, session: None });
    let ai = ai_until(c, tab, "new session, the slow one away", |a| a.session.is_none() && a.activity == AiActivity::Idle && !a.away.is_empty());
    assert_eq!(ai.away.len(), 1);
    assert_eq!(ai.away[0].session, slow);
    assert_eq!(ai.away[0].activity, AiActivity::Working);
    assert_eq!(ai.overall_activity(), AiActivity::Working);
    slow
}

#[test]
fn another_session_opened_mid_turn_answers_and_the_first_goes_on() {
    let dir = tempfile::tempdir().unwrap();
    fake_claude();
    let gate = dir.path().join("gate");
    let (_env, mut c, tab) = ai_tab(dir.path());

    let slow = slow_then_new_session(&mut c, tab);

    // The new session answers, with none of the slow one's conversation.
    c.msg(ClientMsg::AiSend { tab, text: "count the lines".into(), images: Vec::new() });
    let entries = conversation_until(&mut c, tab, Vec::new(), "the new session's answer", done);
    assert!(matches!(&entries[0], Entry::User { text, .. } if text == "count the lines"), "{:?}", &entries[..entries.len().min(3)]);
    assert!(entries.iter().any(|e| matches!(e, Entry::Assistant { text, .. } if text.contains("3 lines"))));
    assert!(!stopped(&entries), "{entries:?}");
    let ai = ai_until(&mut c, tab, "the new session's id", |a| a.session.is_some() && a.activity == AiActivity::Idle);
    assert_ne!(ai.session.as_deref(), Some(slow.as_str()));
    // The slow one still works in the background.
    assert_eq!(ai.away.iter().map(|a| a.session.as_str()).collect::<Vec<_>>(), [slow.as_str()]);

    // Back to it: shown as it is, still working, its process the same.
    c.msg(ClientMsg::AiOpenSession { tab, session: Some(slow.clone()) });
    let ai = ai_until(&mut c, tab, "the slow one shown again", |a| a.session.as_deref() == Some(slow.as_str()) && a.activity == AiActivity::Working);
    // The other one was done: its process stopped, nothing goes on away.
    assert!(ai.away.is_empty(), "{:?}", ai.away);
    let entries = conversation_until(&mut c, tab, Vec::new(), "the slow one's conversation", |e| !e.is_empty());
    assert!(matches!(&entries[0], Entry::User { text, .. } if text == "slow, please"), "{entries:?}");

    // It finishes, here.
    std::fs::write(&gate, "").unwrap();
    let entries = conversation_until(&mut c, tab, entries, "the slow one's answer", done);
    assert!(matches!(&entries[0], Entry::User { text, .. } if text == "slow, please"), "{entries:?}");
    assert!(!stopped(&entries), "{entries:?}");
    ai_until(&mut c, tab, "idle", |a| a.activity == AiActivity::Idle && a.session.as_deref() == Some(slow.as_str()));

    // Each session's Claude Code started once: the slow one was never started again.
    let log = std::fs::read_to_string(dir.path().join("claude.log")).unwrap();
    assert_eq!(log.lines().filter(|l| l.starts_with("start ")).count(), 2, "{log}");
}

#[test]
fn a_session_done_in_the_background_lets_go_of_its_process() {
    let dir = tempfile::tempdir().unwrap();
    fake_claude();
    let gate = dir.path().join("gate");
    let (_env, mut c, tab) = ai_tab(dir.path());

    slow_then_new_session(&mut c, tab);
    std::fs::write(&gate, "").unwrap();
    let ai = ai_until(&mut c, tab, "the slow one done in the background", |a| a.away.is_empty());
    assert_eq!(ai.overall_activity(), AiActivity::Idle);
    assert!(ai.session.is_none());
}
