//! What Shed's desktop app reads of another workspace that is not the
//! keeper's (its projects' files and Git, the logbook, Claude Code's
//! sessions), as `shed_client::nexus_api` calls it: end to end, through the
//! real nexus, the link and a keeper with real bash.

mod common;
#[path = "../../shed-nexus/tests/common/mod.rs"]
mod nexus;

use std::time::Duration;

use shed_api::calls::*;
use shed_api::topics::{Event, ProjectTopic, Topic};
use shed_api::wire::Policy;
use shed_client::nexus_api::{NexusApi, RemoteWorkspace, SubEvent};
use shed_client::nexus_client::SignIn;

fn write_session(dir: &std::path::Path, id: &str) {
    let line = format!(r#"{{"type":"user","uuid":"{id}-p","parentUuid":null,"timestamp":"2026-11-13T08:00:00Z","sessionId":"{id}","message":{{"role":"user","content":"plan the shelves"}}}}"#);
    std::fs::write(dir.join(format!("{id}.jsonl")), line + "\n").unwrap();
}

/// Calls `call` until `pick` takes its answer (the workspace may not be
/// online, or have the thing, yet).
async fn until<T>(ws: &RemoteWorkspace, what: &str, call: impl Fn() -> Call, mut pick: impl FnMut(Reply) -> Option<T>) -> T {
    let mut last = None;
    for _ in 0..100 {
        match ws.call(call()).await {
            Ok(reply) => match pick(reply.clone()) {
                Some(found) => return found,
                None => last = Some(format!("{reply:?}")),
            },
            Err(e) => last = Some(format!("{e:?}")),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{what}: last {last:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_desktop_reads_another_workspace_through_the_nexus() {
    let dir = tempfile::tempdir().unwrap();
    // Claude Code's sessions in a folder of the test's (its own process).
    // SAFETY: set before the keeper starts; nothing else in this process sets it.
    unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", dir.path().join("claude-config")) };
    unsafe { std::env::set_var("SHED_CLAUDE", "/nonexistent/claude") };
    let root = common::repo(dir.path());
    let sessions = shed_ai::sessions::sessions_dir(&root);
    std::fs::create_dir_all(&sessions).unwrap();
    write_session(&sessions, "s1");

    let env = common::keeper(dir.path());
    let hub = nexus::start().await;
    let mut account = nexus::set_up(&hub).await;
    let (workspace, token) = nexus::pair(&hub, &mut account, "desk").await;
    let _link = common::link(&env, hub.base.clone(), &token, Policy::Full);
    let desktop = nexus::desktop_token(&hub, &mut account).await;
    let sign_in = SignIn { url: hub.base.clone(), username: "me".into(), device: String::new(), token: desktop };
    let ws = NexusApi::new(&sign_in, None).workspace(&workspace);

    // A bench with a project tab and an AI tab on the repository.
    let new_bench = || Call::NewBench(NewBench { name: Some("work".into()), cwd: Some(env.home.display().to_string()) });
    let bench = until(&ws, "a bench", new_bench, |r| if let Reply::Bench(b) = r { Some(b) } else { None }).await;
    let tab = |kind: NewTabKind| Call::NewTab(NewTab { bench: BenchRef::Running(bench.clone()), kind });
    let Reply::Tab(project) = ws.call(tab(NewTabKind::Project(Folder { folder: root.display().to_string() }))).await.unwrap() else { panic!("not a tab") };
    let Reply::Tab(ai) = ws.call(tab(NewTabKind::Ai(Folder { folder: root.display().to_string() }))).await.unwrap() else { panic!("not a tab") };

    // Its folder, and a file read and written.
    let path = |p: &str| ProjectPath { tab: project.clone(), path: p.into() };
    let names = until(&ws, "the folder", || Call::ListFolder(path("")), |r| if let Reply::Folder(f) = r { Some(f) } else { None }).await;
    assert!(names.iter().any(|e| e.name == "a.txt" && !e.dir), "{names:?}");
    let Reply::File(a) = ws.call(Call::ReadFile(path("a.txt"))).await.unwrap() else { panic!("not a file") };
    assert_eq!((a.kind, a.text.as_str()), (FileKind::Text, "one\nTWO\nthree\n"));
    // Changes there are followed.
    let changes = ws.subscribe(Topic::Project(ProjectTopic { tab: project.clone() }));
    tokio::time::sleep(Duration::from_millis(500)).await;
    ws.call(Call::WriteFile(WriteFile { tab: project.clone(), path: "b.txt".into(), text: "new\n".into() })).await.unwrap();
    assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "new\n");
    let seen = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match changes.next().await {
                Some(SubEvent::Event(Event::FilesChanged(f))) if f.paths.iter().any(|p| p == "b.txt") => return true,
                Some(_) => {}
                None => return false,
            }
        }
    })
    .await;
    assert_eq!(seen, Ok(true), "the change came as an event");
    drop(changes);

    // Git: the status names the repository; staged and committed.
    let Reply::GitStatus(s) = ws.call(Call::GitStatus(TabOnly { tab: project.clone() })).await.unwrap() else { panic!("not a status") };
    assert!(s.repo);
    assert_eq!(s.root.as_deref().map(std::path::Path::new), Some(std::fs::canonicalize(&root).unwrap().as_path()));
    assert!(s.head.is_some());
    let mut files: Vec<(&str, &str)> = s.files.iter().map(|f| (f.path.as_str(), f.unstaged.as_str())).collect();
    files.sort();
    assert_eq!(files, [("a.txt", "M"), ("b.txt", "?")]);
    ws.call(Call::GitStage(GitPaths { tab: project.clone(), paths: vec!["a.txt".into()] })).await.unwrap();
    let Reply::Text(hash) = ws.call(Call::GitCommit(GitCommit { tab: project.clone(), message: "second".into(), amend: false })).await.unwrap() else { panic!("no hash") };
    assert_eq!(hash.len(), 40);
    let Reply::GitStatus(s) = ws.call(Call::GitStatus(TabOnly { tab: project.clone() })).await.unwrap() else { panic!("not a status") };
    assert_eq!(s.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["b.txt"]);
    let Reply::Commits(log) = ws.call(Call::GitLog(GitLog { tab: project.clone(), path: None, limit: 5 })).await.unwrap() else { panic!("not a log") };
    assert_eq!(log.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["second", "first"]);

    // The logbook: a command typed in the bench's terminal.
    let pane = pane_of(&hub, &sign_in, &workspace, &bench).await;
    ws.call(Call::Input(Input { pane, input: vec![InputItem::Text("echo h''i-from-afar\r".into())] })).await.unwrap();
    let found = until(
        &ws,
        "the command in the logbook",
        || Call::LogSearch(LogSearch { scope: LogScope::All, tab: None, query: "i-from-afar".into(), limit: 10 }),
        |r| match r {
            Reply::Log(entries) => entries.into_iter().find(|e| e.command == "echo h''i-from-afar"),
            _ => None,
        },
    )
    .await;
    assert_eq!(found.exit, Some(0));

    // Claude Code's sessions for the AI tab's folder, from that computer.
    let Reply::Sessions(listed) = ws.call(Call::AiSessions(TabOnly { tab: ai })).await.unwrap() else { panic!("not sessions") };
    assert_eq!(listed.iter().map(|s| (s.id.as_str(), s.title.as_str())).collect::<Vec<_>>(), [("s1", "plan the shelves")]);
}

/// The first pane of `bench`, from the workspace's state as the nexus
/// gives it to a connector.
async fn pane_of(hub: &nexus::Nexus, sign_in: &SignIn, workspace: &str, bench: &str) -> String {
    for _ in 0..100 {
        let (_, welcome) = nexus::connector(hub, &sign_in.token).await;
        let pane = welcome
            .states
            .iter()
            .filter(|s| s.workspace == workspace)
            .flat_map(|s| &s.state.benches)
            .filter(|b| b.id == bench)
            .flat_map(|b| &b.tabs)
            .filter(|t| t.kind == shed_api::state::TabKind::Terminal)
            .find_map(|t| t.panes.first().map(|p| p.id.clone()));
        if let Some(pane) = pane {
            return pane;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no pane");
}
