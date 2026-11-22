//! Notifications: live to the connectors while one is in view, else pushed
//! (a ntfy topic for phones, Web Push for browsers). A fake push service and
//! ntfy on loopback count what reaches them.

mod common;
mod receiver;

use common::*;
use receiver::*;
use shed_api::calls::Call;
use shed_api::http::{PushInfo, PushPrefs};
use shed_api::wire::*;

struct World {
    nexus: Nexus,
    receiver: Receiver,
    workspace: String,
    link: Socket,
    /// The phone's token and its connection.
    phone: Auth,
    app: Socket,
}

/// A nexus with a workspace linked, a browser and a phone each subscribed to
/// Web Push on the receiver, and the phone connected; with a ntfy topic on
/// the receiver if `ntfy`.
async fn world(ntfy: bool) -> World {
    let receiver = Receiver::start().await;
    let topic = format!("{}/alerts-shed", receiver.base);
    let nexus = start_with(|c| {
        c.push.allow_private_endpoints = true;
        c.push.allow_http_endpoints = true;
        c.push.offline_after_secs = 0;
        if ntfy {
            c.push.ntfy_url = Some(topic.clone());
        }
    })
    .await;
    let mut account = set_up(&nexus).await;
    let (workspace, token) = pair(&nexus, &mut account, "desk").await;
    let link = link(&nexus, &token).await;
    let phone = Auth::Bearer(phone_token(&nexus, &mut account).await);
    let _: serde_json::Value = nexus.post("/api/v1/push/subscribe", &subscription(&receiver, "/wp/browser"), &account.browser).await;
    let _: serde_json::Value = nexus.post("/api/v1/push/subscribe", &subscription(&receiver, "/wp/phone"), &phone).await;
    let info: PushInfo = nexus.get("/api/v1/push", &phone).await;
    assert_eq!(info.ntfy_topic, ntfy.then_some(topic));
    let Auth::Bearer(token) = &phone else { unreachable!() };
    let (app, _) = connector(&nexus, token).await;
    World { nexus, receiver, workspace, link, phone, app }
}

/// Says whether the connector is in view, and waits until the nexus has it
/// (a request after it is answered after it is handled).
async fn visible(app: &mut Socket, visible: bool) {
    send(app, &ClientFrame::Visible(visible)).await;
    send(app, &ClientFrame::Request(Request { id: 99, workspace: "nowhere".into(), call: Call::Previews })).await;
    until(app, |f| match f {
        ServerFrame::Response(r) if r.id == 99 => Some(()),
        _ => None,
    })
    .await;
}

fn waiting() -> Notification {
    Notification {
        kind: NotificationKind::AiWaiting,
        title: "Claude waits for an answer".into(),
        body: "api · claude".into(),
        bench: Some("b1".into()),
        tab: Some("t1".into()),
    }
}

async fn live(app: &mut Socket) -> NotificationFrame {
    until(app, |f| match f {
        ServerFrame::Notification(n) => Some(n),
        _ => None,
    })
    .await
}

#[tokio::test]
async fn an_app_in_view_gets_it_live_and_nothing_is_pushed() {
    let mut w = world(true).await;
    // Another connector, not in view, hears it too.
    let Auth::Bearer(token) = &w.phone else { unreachable!() };
    let (mut other, _) = connector(&w.nexus, token).await;
    visible(&mut w.app, true).await;

    send(&mut w.link, &LinkFrame::Notify(waiting())).await;
    let frame = live(&mut w.app).await;
    assert_eq!(frame, NotificationFrame { workspace: w.workspace.clone(), notification: waiting() });
    assert_eq!(live(&mut other).await, frame);
    assert!(w.receiver.wait_for(1).await.is_empty(), "nothing pushed while an app is in view");

    // Out of view: pushed again.
    visible(&mut w.app, false).await;
    send(&mut w.link, &LinkFrame::Notify(waiting())).await;
    assert_eq!(w.receiver.wait_for(2).await.len(), 2);
}

#[tokio::test]
async fn with_none_in_view_phones_get_the_topic_and_browsers_web_push() {
    let mut w = world(true).await;
    visible(&mut w.app, false).await;
    send(&mut w.link, &LinkFrame::Notify(waiting())).await;
    let hits = w.receiver.wait_for(2).await;
    assert_eq!(hits.len(), 2, "{hits:?}");

    let [ntfy] = &w.receiver.on("/alerts-shed")[..] else { panic!("one ntfy message: {hits:?}") };
    assert_eq!(ntfy.headers["title"], "Claude waits for an answer");
    assert_eq!(ntfy.headers["priority"], "4");
    assert_eq!(ntfy.headers["click"], format!("shed://w/{}/b/b1/t/t1", w.workspace));
    assert!(ntfy.headers.get("tags").is_some());
    // No token in the environment here: none sent.
    assert!(ntfy.headers.get("authorization").is_none());
    assert_eq!(String::from_utf8(ntfy.body.clone()).unwrap(), "desk · api · claude");

    let [browser] = &w.receiver.on("/wp/browser")[..] else { panic!("Web Push to the browser: {hits:?}") };
    assert_eq!(browser.headers["content-encoding"], "aes128gcm");
    assert!(browser.headers["authorization"].to_str().unwrap().starts_with("vapid "));
    assert!(w.receiver.on("/wp/phone").is_empty(), "the phone reads the topic, not UnifiedPush");
}

#[tokio::test]
async fn without_a_topic_phones_get_unified_push() {
    let mut w = world(false).await;
    send(&mut w.link, &LinkFrame::Notify(waiting())).await;
    let hits = w.receiver.wait_for(2).await;
    assert_eq!(w.receiver.on("/wp/browser").len(), 1, "{hits:?}");
    assert_eq!(w.receiver.on("/wp/phone").len(), 1, "{hits:?}");
    assert_eq!(hits.len(), 2);
}

#[tokio::test]
async fn the_topic_carries_the_kinds_phones_want() {
    let mut w = world(true).await;
    let prefs = PushPrefs { kinds: vec![NotificationKind::AiDone] };
    let _: serde_json::Value = w.nexus.post("/api/v1/push/prefs", &prefs, &w.phone).await;
    send(&mut w.link, &LinkFrame::Notify(waiting())).await;
    let hits = w.receiver.wait_for(1).await;
    assert!(w.receiver.on("/alerts-shed").is_empty(), "the phone does not want AiWaiting: {hits:?}");
    assert_eq!(w.receiver.on("/wp/browser").len(), 1);
}

#[tokio::test]
async fn a_workspace_going_offline_follows_the_same_rule() {
    // In view: live.
    let mut w = world(true).await;
    visible(&mut w.app, true).await;
    drop(w.link);
    let frame = live(&mut w.app).await;
    assert_eq!(frame.workspace, w.workspace);
    assert_eq!(frame.notification.kind, NotificationKind::WorkspaceOffline);
    assert_eq!(frame.notification.title, "desk went offline");
    assert!(w.receiver.wait_for(1).await.is_empty());

    // None in view: the topic, at a high priority, and the browser.
    let w = world(true).await;
    drop(w.link);
    let hits = w.receiver.wait_for(2).await;
    let [ntfy] = &w.receiver.on("/alerts-shed")[..] else { panic!("one ntfy message: {hits:?}") };
    assert_eq!(ntfy.headers["title"], "desk went offline");
    assert_eq!(ntfy.headers["priority"], "4");
    assert_eq!(ntfy.headers["click"], format!("shed://w/{}", w.workspace));
    assert_eq!(w.receiver.on("/wp/browser").len(), 1);
    assert!(w.receiver.on("/wp/phone").is_empty());
}
