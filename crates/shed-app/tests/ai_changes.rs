//! An AI tab changed one thing at a time (`AiChange`), from several clients
//! at once, and a link client following the tree without counting as a
//! window.

mod common;

use common::{Client, start_in};
use shed_model::{AiTab, BenchId, Shed, TabId};
use shed_proto::{AiChange, ClientKind, ClientMsg, Frame, KeeperMsg, ShowTarget};

fn tree_until(c: &mut Client, what: &str, ok: impl Fn(&Shed) -> bool) -> Shed {
    if ok(&c.tree) {
        return c.tree.clone();
    }
    c.until(what, |f, _| match f {
        Frame::Keeper(KeeperMsg::TreeChanged(t)) if ok(t) => Some(t.clone()),
        _ => None,
    })
}

fn ai(t: &Shed, tab: TabId) -> Option<&AiTab> {
    t.tab(tab).and_then(|t| t.ai_info())
}

fn ai_tab(c: &mut Client, dir: &std::path::Path) -> TabId {
    c.msg(ClientMsg::Show(ShowTarget::NewBench { name: Some("ai".into()), cwd: None }));
    let bench: BenchId = c.until("show", |f, _| match f {
        Frame::Keeper(KeeperMsg::Show { bench }) => Some(*bench),
        _ => None,
    });
    c.msg(ClientMsg::Attach { bench: Some(bench) });
    tree_until(c, "bench", |t| t.bench(bench).is_some());
    c.msg(ClientMsg::NewAiTab { bench, folder: dir.to_path_buf() });
    let tree = tree_until(c, "ai tab", |t| t.bench(bench).is_some_and(|b| b.tabs.iter().any(|t| t.ai_info().is_some())));
    tree.bench(bench).unwrap().tabs.iter().find(|t| t.ai_info().is_some()).unwrap().id
}

#[test]
fn changes_from_two_clients_do_not_undo_each_other() {
    let dir = tempfile::tempdir().unwrap();
    let env = start_in(dir.path(), "shedd.sock");
    let mut a = Client::connect(&env.socket);
    let tab = ai_tab(&mut a, dir.path());
    // A link follows the tree as a window does.
    let mut link = Client::connect_as(&env.socket, ClientKind::Link);
    tree_until(&mut link, "link sees the tab", |t| ai(t, tab).is_some());

    // One writes in the prompt while the other keeps a draft: both stay.
    a.msg(ClientMsg::AiChange { tab, change: AiChange::Unsent("half a thought".into()) });
    link.msg(ClientMsg::AiChange { tab, change: AiChange::AddDraft { session: None, text: "later".into() } });
    let t = tree_until(&mut a, "both", |t| ai(t, tab).is_some_and(|a| a.unsent == "half a thought" && a.drafts.len() == 1));
    let ai_now = ai(&t, tab).unwrap();
    // The first draft of a new session made it a draft session.
    let key = ai_now.draft_session.clone().expect("a draft session");
    assert_eq!(ai_now.drafts[0].session, key);

    // A setting changes alone.
    let mode = ai_now.mode.clone();
    a.msg(ClientMsg::AiChange {
        tab,
        change: AiChange::Settings { model: Some(Some("opus".into())), mode: None, effort: None, follow: None, browser: None, add_dirs: None },
    });
    let t = tree_until(&mut a, "model", |t| ai(t, tab).is_some_and(|a| a.model.as_deref() == Some("opus")));
    assert_eq!(ai(&t, tab).unwrap().mode, mode);
    assert_eq!(ai(&t, tab).unwrap().unsent, "half a thought");

    // Removing a draft that is no longer what was seen does nothing.
    a.msg(ClientMsg::AiChange { tab, change: AiChange::RemoveDraft { index: 0, session: key.clone(), text: "other".into() } });
    a.msg(ClientMsg::AiChange { tab, change: AiChange::Viewed { key: "k".into(), hash: "h".into(), viewed: true } });
    let t = tree_until(&mut a, "viewed", |t| ai(t, tab).is_some_and(|a| a.viewed.len() == 1));
    assert_eq!(ai(&t, tab).unwrap().drafts.len(), 1);
    // A draft changes in place only if it is still what the client saw.
    a.msg(ClientMsg::AiChange { tab, change: AiChange::EditDraft { index: 0, session: key.clone(), old: "other".into(), text: "lost".into() } });
    a.msg(ClientMsg::AiChange { tab, change: AiChange::EditDraft { index: 0, session: key.clone(), old: "later".into(), text: "later, changed".into() } });
    tree_until(&mut a, "draft changed", |t| ai(t, tab).is_some_and(|a| a.drafts.len() == 1 && a.drafts[0].text == "later, changed"));
    a.msg(ClientMsg::AiChange { tab, change: AiChange::RemoveDraft { index: 0, session: key, text: "later, changed".into() } });
    a.msg(ClientMsg::AiChange { tab, change: AiChange::Explained(vec![("k".into(), "h".into(), "why".into())]) });
    let t = tree_until(&mut a, "explained", |t| ai(t, tab).is_some_and(|a| a.explained.len() == 1));
    assert!(ai(&t, tab).unwrap().drafts.is_empty());

    // The link is not a window.
    link.msg(ClientMsg::Status);
    let windows = link.until("status", |f, _| match f {
        Frame::Keeper(KeeperMsg::Status(s)) => Some(s.windows),
        _ => None,
    });
    assert_eq!(windows, 1);
}

#[test]
fn a_sent_message_is_no_longer_unsent() {
    let dir = tempfile::tempdir().unwrap();
    // No Claude Code is started: sending fails after the keeper has seen it.
    // SAFETY: only this test binary's AI code reads it.
    unsafe { std::env::set_var("SHED_CLAUDE", dir.path().join("no-claude")) };
    let env = start_in(dir.path(), "shedd.sock");
    let mut a = Client::connect(&env.socket);
    let tab = ai_tab(&mut a, dir.path());
    // Kept a moment before the last keys, then sent whole.
    a.msg(ClientMsg::AiChange { tab, change: AiChange::Unsent("count the li".into()) });
    tree_until(&mut a, "unsent kept", |t| ai(t, tab).is_some_and(|a| a.unsent == "count the li"));
    a.msg(ClientMsg::AiSend { tab, text: "count the lines".into(), images: Vec::new() });
    tree_until(&mut a, "unsent cleared", |t| ai(t, tab).is_some_and(|a| a.unsent.is_empty()));
    // Something else kept stays.
    a.msg(ClientMsg::AiChange { tab, change: AiChange::Unsent("another thought".into()) });
    tree_until(&mut a, "kept again", |t| ai(t, tab).is_some_and(|a| a.unsent == "another thought"));
    a.msg(ClientMsg::AiSend { tab, text: "a different message".into(), images: Vec::new() });
    std::thread::sleep(std::time::Duration::from_millis(300));
    a.msg(ClientMsg::Status);
    let t = a.until("status", |f, _| match f {
        Frame::Keeper(KeeperMsg::Status(s)) => Some(s.tree.clone()),
        _ => None,
    });
    assert_eq!(ai(&t, tab).unwrap().unsent, "another thought");
}
