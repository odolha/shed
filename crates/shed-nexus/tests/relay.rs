//! Relaying between connectors and a workspace's link (a fake one here).

mod common;

use common::*;
use shed_api::calls::{Call, Reply, TabOnly};
use shed_api::http::{DeviceKind, Revoke};
use shed_api::state::WorkspaceState;
use shed_api::topics::{AiTopic, Event, Topic};
use shed_api::wire::*;
use shed_api::{API_VERSION, ErrorCode};

fn state(host: &str) -> WorkspaceState {
    WorkspaceState { host: host.into(), home: "/home/you".into(), ..WorkspaceState::default() }
}

#[tokio::test]
async fn requests_and_subscriptions_go_through() {
    let nexus = start().await;
    let mut account = set_up(&nexus).await;
    let (workspace, token) = pair(&nexus, &mut account, "desk").await;
    let mut link = link(&nexus, &token).await;
    send(&mut link, &LinkFrame::State(state("desk"))).await;

    let phone = phone_token(&nexus, &mut account).await;
    // The state is in by the time the connector says hello.
    let mut welcome = None;
    for _ in 0..50 {
        let (socket, w) = connector(&nexus, &phone).await;
        if !w.states.is_empty() {
            welcome = Some((socket, w));
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let (mut app, welcome) = welcome.expect("a welcome with the state");
    assert_eq!(welcome.workspaces.len(), 1);
    assert!(welcome.workspaces[0].online);
    assert_eq!(welcome.workspaces[0].name, "desk");
    assert_eq!(welcome.states[0].state.host, "desk");

    // A request, under the nexus's own id at the link.
    send(&mut app, &ClientFrame::Request(Request { id: 7, workspace: workspace.clone(), call: Call::Previews })).await;
    let NexusFrame::Request(forwarded) = next(&mut link).await else { panic!("no request") };
    assert_eq!(forwarded.call, Call::Previews);
    send(&mut link, &LinkFrame::Response(Response::ok(forwarded.id, Reply::Done))).await;
    let response = until(&mut app, |f| match f {
        ServerFrame::Response(r) => Some(r),
        _ => None,
    })
    .await;
    assert_eq!(response, Response::ok(7, Reply::Done));

    // A call the nexus does not know passes untouched.
    let unknown = format!(
        r#"{{"type":"Request","content":{{"id":8,"workspace":"{workspace}","call":{{"type":"Later","content":{{"a":[1, 2]}}}}}}}}"#
    );
    send_text(&mut app, unknown).await;
    let text = next_text(&mut link).await.unwrap();
    assert!(text.contains(r#""call":{"type":"Later","content":{"a":[1, 2]}}"#), "{text}");

    // A subscription and its events.
    let topic = Topic::Ai(AiTopic { tab: "t1".into() });
    send(&mut app, &ClientFrame::Subscribe(Subscribe { id: 3, workspace: workspace.clone(), topic: topic.clone() })).await;
    let NexusFrame::Subscribe(sub) = next(&mut link).await else { panic!("no subscribe") };
    assert_eq!(sub.topic, topic);
    send(&mut link, &LinkFrame::Event(EventFrame { sub: sub.id, event: Event::GitChanged })).await;
    let event = until(&mut app, |f| match f {
        ServerFrame::Event(e) => Some(e),
        _ => None,
    })
    .await;
    assert_eq!(event, EventFrame { sub: 3, event: Event::GitChanged });

    // States go to every connector.
    send(&mut link, &LinkFrame::State(state("desk2"))).await;
    let frame = until(&mut app, |f| match f {
        ServerFrame::State(s) => Some(s),
        _ => None,
    })
    .await;
    assert_eq!(frame.workspace, workspace);
    assert_eq!(frame.state.host, "desk2");

    // Unsubscribing reaches the link under its id.
    send(&mut app, &ClientFrame::Unsubscribe(Unsubscribe { id: 3 })).await;
    assert_eq!(next::<NexusFrame>(&mut link).await, NexusFrame::Unsubscribe(Unsubscribe { id: sub.id }));

    // A connector that leaves is unsubscribed.
    send(&mut app, &ClientFrame::Subscribe(Subscribe { id: 4, workspace: workspace.clone(), topic })).await;
    let NexusFrame::Subscribe(sub) = next(&mut link).await else { panic!("no subscribe") };
    drop(app);
    assert_eq!(next::<NexusFrame>(&mut link).await, NexusFrame::Unsubscribe(Unsubscribe { id: sub.id }));
}

#[tokio::test]
async fn an_offline_workspace_answers_offline() {
    let nexus = start().await;
    let mut account = set_up(&nexus).await;
    let (workspace, token) = pair(&nexus, &mut account, "desk").await;
    let phone = phone_token(&nexus, &mut account).await;
    let (mut app, welcome) = connector(&nexus, &phone).await;
    assert!(!welcome.workspaces[0].online);

    let call = Call::AiSessions(TabOnly { tab: "t".into() });
    send(&mut app, &ClientFrame::Request(Request { id: 1, workspace: workspace.clone(), call: call.clone() })).await;
    let response: ServerFrame = next(&mut app).await;
    let ServerFrame::Response(response) = response else { panic!("{response:?}") };
    assert_eq!(response.error.unwrap().code, ErrorCode::Offline);

    send(&mut app, &ClientFrame::Request(Request { id: 2, workspace: "nowhere".into(), call: call.clone() })).await;
    let ServerFrame::Response(response) = next(&mut app).await else { panic!() };
    assert_eq!(response.error.unwrap().code, ErrorCode::NotFound);

    // It comes online: everyone hears.
    let mut link = link(&nexus, &token).await;
    let list = until(&mut app, |f| match f {
        ServerFrame::Workspaces(list) => Some(list),
        _ => None,
    })
    .await;
    assert!(list[0].online);

    // It goes while a request and a subscription wait.
    let topic = Topic::Ai(AiTopic { tab: "t".into() });
    send(&mut app, &ClientFrame::Subscribe(Subscribe { id: 5, workspace: workspace.clone(), topic })).await;
    let _: NexusFrame = next(&mut link).await;
    send(&mut app, &ClientFrame::Request(Request { id: 6, workspace: workspace.clone(), call })).await;
    let _: NexusFrame = next(&mut link).await;
    drop(link);

    let (mut ended, mut answered, mut offline) = (false, false, false);
    while !(ended && answered && offline) {
        match next::<ServerFrame>(&mut app).await {
            ServerFrame::Ended(e) => {
                assert_eq!((e.sub, e.error.unwrap().code), (5, ErrorCode::Offline));
                ended = true;
            }
            ServerFrame::Response(r) => {
                assert_eq!((r.id, r.error.unwrap().code), (6, ErrorCode::Offline));
                answered = true;
            }
            ServerFrame::Workspaces(list) => offline = !list[0].online,
            _ => {}
        }
    }
}

#[tokio::test]
async fn revoking_closes_the_sockets_at_once() {
    let nexus = start().await;
    let mut account = set_up(&nexus).await;
    let (workspace, token) = pair(&nexus, &mut account, "desk").await;
    let mut link = link(&nexus, &token).await;
    let phone = phone_token(&nexus, &mut account).await;
    let (mut app, _) = connector(&nexus, &phone).await;

    let revoke = Revoke { device: workspace.clone(), totp: account.auth.code().await };
    let _: serde_json::Value = nexus.post("/api/v1/devices/revoke", &revoke, &account.browser).await;
    let goodbye: NexusFrame = next(&mut link).await;
    let NexusFrame::Goodbye(error) = goodbye else { panic!("{goodbye:?}") };
    assert_eq!(error.code, ErrorCode::Unauthorized);
    assert!(closes(&mut link).await);
    // Its token no longer links.
    let refused = connect(&nexus.ws_url("/link"), &[("authorization", format!("Bearer {token}"))]).await;
    assert_eq!(refused.err(), Some(401));
    // The connector hears the list without it.
    let list = until(&mut app, |f| match f {
        ServerFrame::Workspaces(list) => Some(list),
        _ => None,
    })
    .await;
    assert!(list.is_empty());
}

#[tokio::test]
async fn revoking_a_phone_closes_its_connector() {
    let nexus = start().await;
    let mut account = set_up(&nexus).await;
    let phone = phone_token(&nexus, &mut account).await;
    let (mut app, _) = connector(&nexus, &phone).await;

    let devices: Vec<shed_api::http::Device> = nexus.get("/api/v1/devices", &account.browser).await;
    let phone_id = devices.iter().find(|d| d.kind == DeviceKind::Phone).unwrap().id.clone();
    assert!(devices.iter().find(|d| d.id == phone_id).unwrap().online);
    let revoke = Revoke { device: phone_id, totp: account.auth.code().await };
    let _: serde_json::Value = nexus.post("/api/v1/devices/revoke", &revoke, &account.browser).await;
    let goodbye = until(&mut app, |f| match f {
        ServerFrame::Goodbye(error) => Some(error),
        _ => None,
    })
    .await;
    assert_eq!(goodbye.code, ErrorCode::Unauthorized);
    assert!(closes(&mut app).await);
}

#[tokio::test]
async fn other_api_versions_are_turned_away() {
    let nexus = start().await;
    let mut account = set_up(&nexus).await;
    let (_, token) = pair(&nexus, &mut account, "desk").await;
    let phone = phone_token(&nexus, &mut account).await;

    let mut app = connect(&nexus.ws_url("/api"), &[("authorization", format!("Bearer {phone}"))]).await.unwrap();
    send(&mut app, &ClientFrame::Hello(ClientHello { api: API_VERSION + 1, client: "future".into() })).await;
    let ServerFrame::Goodbye(error) = next(&mut app).await else { panic!("no goodbye") };
    assert_eq!(error.code, ErrorCode::Unsupported);
    assert!(closes(&mut app).await);

    let mut link = connect(&nexus.ws_url("/link"), &[("authorization", format!("Bearer {token}"))]).await.unwrap();
    let hello = LinkHello { api: 0, version: "0.0.1".into(), policy: Policy::Full };
    send(&mut link, &LinkFrame::Hello(hello)).await;
    let NexusFrame::Goodbye(error) = next(&mut link).await else { panic!("no goodbye") };
    assert_eq!(error.code, ErrorCode::Unsupported);
}

#[tokio::test]
async fn browsers_connect_from_allowed_origins_only() {
    let nexus = start().await;
    let account = set_up(&nexus).await;
    let Auth::Cookie { token, .. } = &account.browser else { unreachable!() };
    let cookie = format!("shed_session={token}");
    let url = nexus.ws_url("/api");

    let refused = connect(&url, &[("cookie", cookie.clone()), ("origin", "https://evil.example".into())]).await;
    assert_eq!(refused.err(), Some(403));
    let refused = connect(&url, &[("cookie", cookie.clone())]).await;
    assert_eq!(refused.err(), Some(403));
    let refused = connect(&url, &[("cookie", "shed_session=nope".into()), ("origin", ORIGIN.into())]).await;
    assert_eq!(refused.err(), Some(401));

    let mut app = connect(&url, &[("cookie", cookie), ("origin", ORIGIN.into())]).await.unwrap();
    send(&mut app, &ClientFrame::Hello(ClientHello { api: API_VERSION, client: "web".into() })).await;
    assert!(matches!(next(&mut app).await, ServerFrame::Welcome(_)));
}
