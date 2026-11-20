//! The link against a real keeper (bash in its panes) and a fake nexus.

mod common;

use common::*;
use shed_api::ErrorCode;
use shed_api::calls::*;
use shed_api::state::{TabKind, WorkspaceState};
use shed_api::topics::{Event, ProjectTopic, ScreenTopic, Topic};
use shed_api::wire::Policy;

fn bench_tab<'a>(s: &'a WorkspaceState, tab: &str) -> Option<&'a shed_api::state::Tab> {
    s.benches.iter().flat_map(|b| &b.tabs).find(|t| t.id == tab)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_workspace_through_its_link() {
    let dir = tempfile::tempdir().unwrap();
    let env = keeper(dir.path());
    let (listener, url) = listen().await;
    let _link = link(&env, url, "token", Policy::Full);
    let (mut nexus, hello) = accept(&listener).await;
    assert_eq!(hello.policy, Policy::Full);
    let first = nexus.state_where("a first state", |_| true).await;
    assert!(first.benches.is_empty());

    // A bench, and a tab in it: their ids come back, and they show in the state.
    let Reply::Bench(bench) = nexus.call(Call::NewBench(NewBench { name: Some("work".into()), cwd: Some(env.home.display().to_string()) })).await.unwrap() else {
        panic!("not a bench")
    };
    let Reply::Tab(tab) = nexus
        .call(Call::NewTab(NewTab { bench: BenchRef::Running(bench.clone()), kind: NewTabKind::Terminal(OptionalFolder { folder: None }) }))
        .await
        .unwrap()
    else {
        panic!("not a tab")
    };
    let state = nexus.state_where("the new tab", |s| bench_tab(s, &tab).is_some()).await;
    let b = state.benches.iter().find(|b| b.id == bench).unwrap();
    assert_eq!(b.name, "work");
    assert!(b.color.starts_with('#') && b.color.len() == 7);
    let pane = bench_tab(&state, &tab).unwrap().panes[0].id.clone();

    // Its screen, as text, and typing into it.
    let screen = nexus.subscribe(Topic::Screen(ScreenTopic { pane: pane.clone() })).await;
    let full = nexus.event(screen, "the whole screen", |e| match e {
        Event::Screen(s) if s.full => Some(s.clone()),
        _ => None,
    })
    .await;
    assert!(full.cols >= 2 && full.rows >= 1);
    nexus.call(Call::Input(Input { pane: pane.clone(), input: vec![InputItem::Text("echo h''i-there\r".into())] })).await.unwrap();
    nexus
        .event(screen, "the command's output", |e| match e {
            Event::Screen(s) if screen_text(s).lines().any(|l| l.trim() == "hi-there") => Some(()),
            _ => None,
        })
        .await;

    // Its scrollback, and holding its size.
    nexus.call(Call::Scrollback(Scrollback { pane: pane.clone(), from: 0, count: 10 })).await.unwrap();
    nexus.call(Call::TakeSize(TakeSize { pane: pane.clone(), cols: 40, rows: 10 })).await.unwrap();
    nexus.state_where("the size held", |s| bench_tab(s, &tab).is_some_and(|t| t.panes[0].size_held && t.panes[0].cols == 40)).await;
    nexus
        .event(screen, "the screen at its new size", |e| match e {
            Event::Screen(s) if s.cols == 40 && s.rows == 10 => Some(()),
            _ => None,
        })
        .await;
    nexus.call(Call::ReleaseSize(PaneOnly { pane: pane.clone() })).await.unwrap();
    nexus.state_where("the size given back", |s| bench_tab(s, &tab).is_some_and(|t| !t.panes[0].size_held && t.panes[0].cols != 40)).await;

    // The logbook has the command.
    let mut found = false;
    for _ in 0..50 {
        let Reply::Log(entries) = nexus.call(Call::LogSearch(LogSearch { scope: LogScope::All, tab: None, query: "i-there".into(), limit: 10 })).await.unwrap() else {
            panic!("not a log")
        };
        if entries.iter().any(|e| e.command == "echo h''i-there") {
            found = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(found, "the logbook has the command");

    // A project tab on a Git repository.
    let root = repo(dir.path());
    let Reply::Tab(project) = nexus
        .call(Call::NewTab(NewTab { bench: BenchRef::Running(bench.clone()), kind: NewTabKind::Project(Folder { folder: root.display().to_string() }) }))
        .await
        .unwrap()
    else {
        panic!("not a tab")
    };
    let state = nexus.state_where("the project tab", |s| bench_tab(s, &project).is_some()).await;
    assert!(matches!(&bench_tab(&state, &project).unwrap().kind, TabKind::Project(p) if p.root == root.display().to_string()));
    let at = |path: &str| ProjectPath { tab: project.clone(), path: path.into() };
    let escape = nexus.call(Call::ReadFile(at("../../../etc/passwd"))).await.unwrap_err();
    assert_eq!(escape.code, ErrorCode::Invalid);
    assert_eq!(nexus.call(Call::ReadFile(at("/etc/passwd"))).await.unwrap_err().code, ErrorCode::Invalid);
    let Reply::File(file) = nexus.call(Call::ReadFile(at("a.txt"))).await.unwrap() else { panic!("not a file") };
    assert_eq!(file.text, "one\nTWO\nthree\n");
    let Reply::Folder(entries) = nexus.call(Call::ListFolder(at(""))).await.unwrap() else { panic!("not a folder") };
    assert!(entries.iter().any(|e| e.name == "a.txt" && !e.dir));
    let Reply::Files(files) = nexus.call(Call::QuickOpen(QuickOpen { tab: project.clone(), query: "atx".into(), limit: 5 })).await.unwrap() else {
        panic!("not files")
    };
    assert_eq!(files[0].path, "a.txt");

    // Git: status, a diff, staging its hunk, a commit.
    let Reply::GitStatus(status) = nexus.call(Call::GitStatus(TabOnly { tab: project.clone() })).await.unwrap() else { panic!() };
    assert!(status.repo);
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert!(status.files.iter().any(|f| f.path == "a.txt" && f.unstaged == "M"));
    let diff_of = |source| Call::GitDiff(GitDiff { tab: project.clone(), path: "a.txt".into(), source });
    let Reply::Diff(diff) = nexus.call(diff_of(DiffSource::Unstaged)).await.unwrap() else { panic!() };
    assert_eq!(diff.hunks.len(), 1);
    let added = diff.hunks[0].lines.iter().find(|l| l.kind == DiffLineKind::Added).unwrap();
    assert_eq!((added.text.as_str(), added.new), ("TWO", Some(2)));
    let stale = GitHunk { tab: project.clone(), path: "a.txt".into(), source: DiffSource::Unstaged, hunk: 0, header: "@@ -9,9 +9,9 @@".into(), action: HunkAction::Stage };
    assert_eq!(nexus.call(Call::GitHunk(stale.clone())).await.unwrap_err().code, ErrorCode::Invalid);
    nexus.call(Call::GitHunk(GitHunk { header: diff.hunks[0].header.clone(), ..stale })).await.unwrap();
    let Reply::Diff(staged) = nexus.call(diff_of(DiffSource::Staged)).await.unwrap() else { panic!() };
    assert_eq!(staged.hunks.len(), 1);
    nexus.call(Call::GitUnstage(GitPaths { tab: project.clone(), paths: vec!["a.txt".into()] })).await.unwrap();
    nexus.call(Call::GitStage(GitPaths { tab: project.clone(), paths: vec!["a.txt".into()] })).await.unwrap();
    let Reply::Text(hash) = nexus.call(Call::GitCommit(GitCommit { tab: project.clone(), message: "second".into(), amend: false })).await.unwrap() else { panic!() };
    let Reply::Commits(log) = nexus.call(Call::GitLog(GitLog { tab: project.clone(), path: None, limit: 10 })).await.unwrap() else { panic!() };
    assert_eq!((log[0].hash.as_str(), log[0].subject.as_str()), (hash.as_str(), "second"));
    assert_eq!(nexus.call(Call::GitStage(GitPaths { tab: project.clone(), paths: vec!["../x".into()] })).await.unwrap_err().code, ErrorCode::Invalid);

    // The project followed on disk.
    let watch = nexus.subscribe(Topic::Project(ProjectTopic { tab: project.clone() })).await;
    // (The watch starts as the subscription is taken; a moment for it.)
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    nexus.call(Call::CreateEntry(at("notes/new.md"))).await.unwrap();
    nexus
        .event(watch, "the new file", |e| match e {
            Event::FilesChanged(f) if f.paths.iter().any(|p| p.starts_with("notes")) => Some(()),
            _ => None,
        })
        .await;
    nexus.call(Call::WriteFile(WriteFile { tab: project.clone(), path: "notes/new.md".into(), text: "# hi\n".into() })).await.unwrap();
    assert_eq!(std::fs::read_to_string(root.join("notes/new.md")).unwrap(), "# hi\n");
    nexus.unsubscribe(watch).await;

    // A private tab: there, but nothing of it.
    nexus.call(Call::SetPrivate(SetPrivate { bench: BenchRef::Running(bench.clone()), tab: tab.clone(), private: true })).await.unwrap();
    let state = nexus.state_where("the tab private", |s| bench_tab(s, &tab).is_some_and(|t| t.private)).await;
    let t = bench_tab(&state, &tab).unwrap();
    assert_eq!(t.title, "Private");
    assert!(t.panes[0].cwd.is_empty() && t.panes[0].tool.is_none() && t.panes[0].title.is_none());
    // Watching it ends; watching it again is refused.
    assert_eq!(nexus.ended(screen).await.unwrap().code, ErrorCode::Forbidden);
    let refused = nexus.subscribe(Topic::Screen(ScreenTopic { pane: pane.clone() })).await;
    assert_eq!(nexus.ended(refused).await.unwrap().code, ErrorCode::Forbidden);
    let typed = nexus.call(Call::Input(Input { pane: pane.clone(), input: vec![InputItem::Text("ls\r".into())] })).await;
    assert_eq!(typed.unwrap_err().code, ErrorCode::Forbidden);
    let log = nexus.call(Call::LogSearch(LogSearch { scope: LogScope::Tab, tab: Some(tab.clone()), query: String::new(), limit: 10 })).await;
    assert_eq!(log.unwrap_err().code, ErrorCode::Forbidden);
    let public = nexus.call(Call::SetPrivate(SetPrivate { bench: BenchRef::Running(bench.clone()), tab: tab.clone(), private: false })).await;
    assert_eq!(public.unwrap_err().code, ErrorCode::Forbidden);

    // A file for an AI tab's prompt: kept in the tab's folder of uploads,
    // under a safe name; too large a file is refused.
    let Reply::Tab(ai) = nexus
        .call(Call::NewTab(NewTab { bench: BenchRef::Running(bench.clone()), kind: NewTabKind::Ai(Folder { folder: root.display().to_string() }) }))
        .await
        .unwrap()
    else {
        panic!("not a tab")
    };
    nexus.state_where("the AI tab", |s| bench_tab(s, &ai).is_some()).await;
    use base64::Engine;
    let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
    let upload = |name: &str, data: String| Call::AiUpload(AiUpload { tab: ai.clone(), name: name.into(), data });
    let Reply::Text(path) = nexus.call(upload("../../spec.pdf", b64(b"%PDF-1.7"))).await.unwrap() else { panic!("not a path") };
    let path = std::path::PathBuf::from(path);
    assert!(path.starts_with(dir.path().join("state/ai-files").join(&ai)) && path.ends_with("spec.pdf"), "{}", path.display());
    assert_eq!(std::fs::read(&path).unwrap(), b"%PDF-1.7");
    let big = nexus.call(upload("big.bin", b64(&vec![0; MAX_UPLOAD as usize + 1]))).await.unwrap_err();
    assert_eq!(big.code, ErrorCode::Invalid);
    assert_eq!(nexus.call(Call::AiUpload(AiUpload { tab: project.clone(), name: "x".into(), data: b64(b"x") })).await.unwrap_err().code, ErrorCode::Invalid);

    // Closing the bench.
    nexus.call(Call::CloseBench(BenchOnly { bench: bench.clone() })).await.unwrap();
    let state = nexus.state_where("the bench closed", |s| s.benches.is_empty() && s.closed.iter().any(|c| c.name == "work")).await;
    assert!(state.closed[0].saved > 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_view_only_workspace_shows_but_does_not_change() {
    let dir = tempfile::tempdir().unwrap();
    let env = keeper(dir.path());
    let (listener, url) = listen().await;
    let _link = link(&env, url, "token", Policy::View);
    let (mut nexus, hello) = accept(&listener).await;
    assert_eq!(hello.policy, Policy::View);
    let refused = nexus.call(Call::NewBench(NewBench { name: None, cwd: None })).await.unwrap_err();
    assert_eq!(refused.code, ErrorCode::Forbidden);
    assert!(matches!(nexus.call(Call::Previews).await.unwrap(), Reply::Previews(_)));
    // Nor does it show the machine's folders (that is for making tabs).
    let browse = nexus.call(Call::BrowseFolders(Browse { path: env.home.display().to_string() })).await.unwrap_err();
    assert_eq!(browse.code, ErrorCode::Forbidden);
    // An id that is not one, a tab that is not there.
    assert_eq!(nexus.call(Call::GitStatus(TabOnly { tab: "nope".into() })).await.unwrap_err().code, ErrorCode::Invalid);
    let missing = nexus.call(Call::GitStatus(TabOnly { tab: shed_model::TabId::new().to_string() })).await.unwrap_err();
    assert_eq!(missing.code, ErrorCode::NotFound);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_goodbye_stops_the_link() {
    let dir = tempfile::tempdir().unwrap();
    let env = keeper(dir.path());
    let (listener, url) = listen().await;
    let link = link(&env, url, "token", Policy::Full);
    let (tcp, _) = listener.accept().await.unwrap();
    let mut socket = tokio_tungstenite::accept_async(tcp).await.unwrap();
    use futures_util::SinkExt;
    let goodbye = shed_api::wire::NexusFrame::Goodbye(shed_api::ApiError::new(ErrorCode::Unauthorized, "revoked"));
    socket.send(tokio_tungstenite::tungstenite::Message::text(serde_json::to_string(&goodbye).unwrap())).await.unwrap();
    let exit = tokio::time::timeout(std::time::Duration::from_secs(10), link).await.expect("the link stops").unwrap().unwrap();
    assert!(matches!(exit, shed_link::Exit::Refused(ref why) if why.contains("revoked")), "{exit:?}");
    let status = std::fs::read_to_string(dir.path().join("link.json")).unwrap();
    assert!(status.contains("refused"), "{status}");
}
