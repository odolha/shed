//! End to end: a real keeper, the link, and the real nexus in-process, with
//! a connector (a phone) asking through the nexus.

mod common;
#[path = "../../shed-nexus/tests/common/mod.rs"]
mod nexus;

use shed_api::calls::{Call, NewBench, Reply};
use shed_api::state::WorkspaceState;
use shed_api::topics::{Event, ScreenTopic, Topic};
use shed_api::wire::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connector_reaches_the_workspace_through_the_nexus() {
    let dir = tempfile::tempdir().unwrap();
    let env = common::keeper(dir.path());
    let hub = nexus::start().await;
    let mut account = nexus::set_up(&hub).await;

    // Paired as `shed nexus join` does it: the code approved in the web app.
    let base = hub.base.clone();
    let pairing = tokio::spawn(async move { shed_link::pairing::pair(&base, "desk", None, |_| {}).await.unwrap() });
    let pending: Vec<shed_api::http::PendingPairing> = loop {
        let p: Vec<shed_api::http::PendingPairing> = hub.get("/api/v1/pair/pending", &account.browser).await;
        if !p.is_empty() {
            break p;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    let approve = shed_api::http::PairApprove { code: pending[0].code.clone(), name: "desk".into(), totp: account.auth.code().await };
    let _: serde_json::Value = hub.post("/api/v1/pair/approve", &approve, &account.browser).await;
    let shed_link::pairing::Paired::Approved(paired) = pairing.await.unwrap() else { panic!("not approved") };

    let _link = common::link(&env, hub.base.clone(), &paired.token, Policy::Full);
    let phone = nexus::phone_token(&hub, &mut account).await;
    // Connected once the workspace shows online with its state.
    let mut connected = None;
    for _ in 0..100 {
        let (socket, w) = nexus::connector(&hub, &phone).await;
        if w.workspaces.first().is_some_and(|w| w.online) && !w.states.is_empty() {
            connected = Some(socket);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let mut app = connected.expect("the workspace online, with its state");
    let workspace = paired.workspace.clone();

    // A request through the nexus.
    let new = Call::NewBench(NewBench { name: Some("far".into()), cwd: Some(env.home.display().to_string()) });
    nexus::send(&mut app, &ClientFrame::Request(Request { id: 1, workspace: workspace.clone(), call: new })).await;
    let response = nexus::until(&mut app, |f| match f {
        ServerFrame::Response(r) if r.id == 1 => Some(r),
        _ => None,
    })
    .await;
    let Some(Reply::Bench(bench)) = response.ok else { panic!("no bench: {:?}", response.error) };
    let state: WorkspaceState = nexus::until(&mut app, |f| match f {
        ServerFrame::State(s) if s.state.benches.iter().any(|b| b.id == bench) => Some(s.state),
        _ => None,
    })
    .await;
    let pane = state.benches[0].tabs[0].panes[0].id.clone();

    // A terminal's screen through the nexus, and typing into it.
    nexus::send(&mut app, &ClientFrame::Subscribe(Subscribe { id: 2, workspace: workspace.clone(), topic: Topic::Screen(ScreenTopic { pane: pane.clone() }) })).await;
    let input = Call::Input(shed_api::calls::Input { pane, input: vec![shed_api::calls::InputItem::Text("echo far''-away\r".into())] });
    nexus::send(&mut app, &ClientFrame::Request(Request { id: 3, workspace, call: input })).await;
    nexus::until(&mut app, |f| match f {
        ServerFrame::Event(EventFrame { sub: 2, event: Event::Screen(s) }) if common::screen_text(&s).lines().any(|l| l.trim() == "far-away") => Some(()),
        _ => None,
    })
    .await;
}
