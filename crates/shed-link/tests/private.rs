//! What a link keeps from connectors: tabs made private (their streams,
//! their past commands), files outside a tab's folder through Git, the
//! machine's folders for a view-only workspace.

mod common;

use std::time::Duration;

use common::*;
use shed_api::ErrorCode;
use shed_api::calls::*;
use shed_api::state::WorkspaceState;
use shed_api::topics::{ProjectTopic, SearchTopic, Topic};
use shed_api::wire::Policy;

fn bench_tab<'a>(s: &'a WorkspaceState, tab: &str) -> Option<&'a shed_api::state::Tab> {
    s.benches.iter().flat_map(|b| &b.tabs).find(|t| t.id == tab)
}

async fn new_bench(nexus: &mut FakeNexus, env: &Env) -> String {
    let call = Call::NewBench(NewBench { name: Some("work".into()), cwd: Some(env.home.display().to_string()) });
    let Reply::Bench(bench) = nexus.call(call).await.unwrap() else { panic!("not a bench") };
    bench
}

async fn new_tab(nexus: &mut FakeNexus, bench: &str, kind: NewTabKind) -> String {
    let Reply::Tab(tab) = nexus.call(Call::NewTab(NewTab { bench: BenchRef::Running(bench.into()), kind })).await.unwrap() else {
        panic!("not a tab")
    };
    nexus.state_where("the new tab", |s| bench_tab(s, &tab).is_some()).await;
    tab
}

async fn log(nexus: &mut FakeNexus, query: &str) -> Vec<LogEntry> {
    let search = LogSearch { scope: LogScope::All, tab: None, query: query.into(), limit: 10 };
    let Reply::Log(entries) = nexus.call(Call::LogSearch(search)).await.unwrap() else { panic!("not a log") };
    entries
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tab_made_private_takes_its_streams_and_its_past_along() {
    let dir = tempfile::tempdir().unwrap();
    let env = keeper(dir.path());
    let (listener, url) = listen().await;
    let _link = link(&env, url, "token", Policy::Full);
    let (mut nexus, _) = accept(&listener).await;
    let bench = new_bench(&mut nexus, &env).await;

    // A command run while the tab was public.
    let tab = new_tab(&mut nexus, &bench, NewTabKind::Terminal(OptionalFolder { folder: None })).await;
    let state = nexus.state.clone().unwrap();
    let pane = bench_tab(&state, &tab).unwrap().panes[0].id.clone();
    nexus.call(Call::Input(Input { pane, input: vec![InputItem::Text("echo top''secret-xyz\r".into())] })).await.unwrap();
    let mut logged = false;
    for _ in 0..100 {
        if !log(&mut nexus, "secret-xyz").await.is_empty() {
            logged = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(logged, "the command is in the logbook");

    // A project tab followed on disk, and searched.
    let root = dir.path().join("proj");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("notes.txt"), "needle\n").unwrap();
    let project = new_tab(&mut nexus, &bench, NewTabKind::Project(Folder { folder: root.display().to_string() })).await;
    let watch = nexus.subscribe(Topic::Project(ProjectTopic { tab: project.clone() })).await;
    let search = SearchTopic {
        tab: project.clone(),
        query: "needle".into(),
        regex: false,
        case_sensitive: false,
        whole_word: false,
        include: None,
        exclude: None,
    };
    let search = nexus.subscribe(Topic::Search(search)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Both tabs made private.
    for t in [&tab, &project] {
        let private = SetPrivate { bench: BenchRef::Running(bench.clone()), tab: t.clone(), private: true };
        nexus.call(Call::SetPrivate(private)).await.unwrap();
    }
    nexus.state_where("both private", |s| [&tab, &project].iter().all(|t| bench_tab(s, t).is_some_and(|t| t.private))).await;

    // The project's streams end, and say why.
    assert_eq!(nexus.ended(watch).await.unwrap().code, ErrorCode::Forbidden);
    assert_eq!(nexus.ended(search).await.unwrap().code, ErrorCode::Forbidden);

    // Its past commands no longer show anywhere else: not through the link,
    assert!(log(&mut nexus, "secret-xyz").await.is_empty());
    // nor in the logbook itself (the keeper marked them private).
    let mut marked = false;
    for _ in 0..100 {
        let book = shed_logbook::Logbook::open_readonly(&env.logbook).unwrap();
        let elsewhere = shed_logbook::Context { tab: shed_model::TabId::new(), bench: shed_model::BenchId::new() };
        if book.search(shed_logbook::Scope::Global, elsewhere, "secret-xyz", None, 10).unwrap().is_empty() {
            marked = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(marked, "the keeper marks a private tab's past commands");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn git_stays_in_the_tabs_folder() {
    let dir = tempfile::tempdir().unwrap();
    let env = keeper(dir.path());
    // A repository whose root is above the project's folder.
    let mono = repo(dir.path());
    let app = mono.join("app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(app.join("b.txt"), "one\n").unwrap();
    git(&mono, &["add", "app/b.txt"]);
    git(&mono, &["commit", "-q", "-m", "app"]);
    std::fs::write(app.join("b.txt"), "two\n").unwrap();
    std::fs::write(mono.join("secret.txt"), "the secret\n").unwrap();
    std::os::unix::fs::symlink(mono.join("secret.txt"), app.join("leak")).unwrap();

    let (listener, url) = listen().await;
    let _link = link(&env, url, "token", Policy::Full);
    let (mut nexus, _) = accept(&listener).await;
    let bench = new_bench(&mut nexus, &env).await;
    let project = new_tab(&mut nexus, &bench, NewTabKind::Project(Folder { folder: app.display().to_string() })).await;

    // The status lists the folder's files only (a.txt, changed, is outside).
    let Reply::GitStatus(status) = nexus.call(Call::GitStatus(TabOnly { tab: project.clone() })).await.unwrap() else { panic!() };
    let paths: Vec<&str> = status.files.iter().map(|f| f.path.as_str()).collect();
    assert!(paths.contains(&"app/b.txt"), "{paths:?}");
    assert!(!paths.iter().any(|p| !p.starts_with("app/")), "{paths:?}");

    let diff = |path: &str| Call::GitDiff(GitDiff { tab: project.clone(), path: path.into(), source: DiffSource::Unstaged });
    let Reply::Diff(d) = nexus.call(diff("app/b.txt")).await.unwrap() else { panic!() };
    assert_eq!(d.hunks.len(), 1);
    for outside in ["a.txt", "secret.txt", "app/leak", ".git/config", "app/../secret.txt"] {
        let err = nexus.call(diff(outside)).await.unwrap_err();
        assert!(matches!(err.code, ErrorCode::Forbidden | ErrorCode::Invalid), "{outside}: {err:?}");
    }
    for call in [
        Call::GitStage(GitPaths { tab: project.clone(), paths: vec!["a.txt".into()] }),
        Call::GitDiscard(GitPaths { tab: project.clone(), paths: vec!["app/b.txt".into(), "secret.txt".into()] }),
        Call::GitLog(GitLog { tab: project.clone(), path: Some("a.txt".into()), limit: 5 }),
    ] {
        assert_eq!(nexus.call(call).await.unwrap_err().code, ErrorCode::Forbidden);
    }
    // Nothing was discarded: the refusal comes before any change.
    assert_eq!(std::fs::read_to_string(app.join("b.txt")).unwrap(), "two\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_view_only_workspace_does_not_show_the_machines_folders() {
    let dir = tempfile::tempdir().unwrap();
    let env = keeper(dir.path());
    let (listener, url) = listen().await;
    let _link = link(&env, url, "token", Policy::View);
    let (mut nexus, _) = accept(&listener).await;
    let browse = nexus.call(Call::BrowseFolders(Browse { path: "/".into() })).await.unwrap_err();
    assert_eq!(browse.code, ErrorCode::Forbidden);
}
