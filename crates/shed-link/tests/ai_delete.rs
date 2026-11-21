//! Deleting a Claude Code session through the link: the keeper does it and
//! says how it went; draft sessions and odd ids are refused before.

mod common;

use common::*;
use shed_api::ErrorCode;
use shed_api::calls::*;
use shed_api::state::WorkspaceState;
use shed_api::wire::Policy;

fn bench_tab<'a>(s: &'a WorkspaceState, tab: &str) -> Option<&'a shed_api::state::Tab> {
    s.benches.iter().flat_map(|b| &b.tabs).find(|t| t.id == tab)
}

fn write_session(dir: &std::path::Path, id: &str) {
    let line = format!(r#"{{"type":"user","uuid":"{id}-p","parentUuid":null,"timestamp":"2026-11-13T08:00:00Z","sessionId":"{id}","message":{{"role":"user","content":"plan the shelves"}}}}"#);
    std::fs::write(dir.join(format!("{id}.jsonl")), line + "\n").unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_is_deleted_through_the_link() {
    let dir = tempfile::tempdir().unwrap();
    // Claude Code's sessions in a folder of the test's (its own process).
    // SAFETY: set before the keeper starts; nothing else in this process sets it.
    unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", dir.path().join("claude-config")) };
    // (No real Claude Code: nothing here starts one.)
    unsafe { std::env::set_var("SHED_CLAUDE", "/nonexistent/claude") };
    let folder = dir.path().join("project");
    std::fs::create_dir_all(&folder).unwrap();
    let sessions = shed_ai::sessions::sessions_dir(&folder);
    std::fs::create_dir_all(&sessions).unwrap();
    write_session(&sessions, "s1");
    write_session(&sessions, "s2");

    let env = keeper(dir.path());
    let (listener, url) = listen().await;
    let _link = link(&env, url, "token", Policy::Full);
    let (mut nexus, _) = accept(&listener).await;
    let call = Call::NewBench(NewBench { name: Some("work".into()), cwd: Some(env.home.display().to_string()) });
    let Reply::Bench(bench) = nexus.call(call).await.unwrap() else { panic!("not a bench") };
    let mut tabs = Vec::new();
    for _ in 0..2 {
        let kind = NewTabKind::Ai(Folder { folder: folder.display().to_string() });
        let Reply::Tab(tab) = nexus.call(Call::NewTab(NewTab { bench: BenchRef::Running(bench.clone()), kind })).await.unwrap() else { panic!("not a tab") };
        nexus.state_where("the new tab", |s| bench_tab(s, &tab).is_some()).await;
        tabs.push(tab);
    }
    let (here, other) = (tabs[0].clone(), tabs[1].clone());

    // Shown in the other tab: the keeper refuses.
    nexus.call(Call::AiOpenSession(AiOpenSession { tab: other.clone(), session: Some("s2".into()) })).await.unwrap();
    let shows = |s: &WorkspaceState, tab: &str, session: &str| {
        bench_tab(s, tab).is_some_and(|t| matches!(&t.kind, shed_api::state::TabKind::Ai(a) if a.session.as_deref() == Some(session)))
    };
    nexus.state_where("the other tab on s2", |s| shows(s, &other, "s2")).await;
    let delete = |session: &str| Call::AiDeleteSession(AiDeleteSession { tab: here.clone(), session: session.into() });
    let refused = nexus.call(delete("s2")).await.unwrap_err();
    assert!(refused.message.contains("another tab"), "{refused:?}");
    assert!(sessions.join("s2.jsonl").is_file());

    // Not a Claude Code session: refused by the link.
    assert_eq!(nexus.call(delete("draft-123")).await.unwrap_err().code, ErrorCode::Invalid);
    assert_eq!(nexus.call(delete("../s1")).await.unwrap_err().code, ErrorCode::Invalid);

    // Another one goes, into the trash.
    assert_eq!(nexus.call(delete("s1")).await.unwrap(), Reply::Done);
    assert!(!sessions.join("s1.jsonl").exists());
    let trash: Vec<_> = std::fs::read_dir(dir.path().join("state/ai-trash")).unwrap().flatten().collect();
    assert_eq!(trash.len(), 1);
    assert!(trash[0].path().join("s1.jsonl").is_file());
    let Reply::Sessions(listed) = nexus.call(Call::AiSessions(TabOnly { tab: here.clone() })).await.unwrap() else { panic!("not sessions") };
    assert_eq!(listed.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["s2"]);
}
