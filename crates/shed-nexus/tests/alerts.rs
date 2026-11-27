//! Security alerts: a sign-in, a pairing, the password changed, a device
//! revoked, an address locked out. Told live to every connection and pushed
//! to every device but the one that caused it, in view or not, whatever
//! kinds were chosen; lockouts at most once per address an hour.

mod common;
mod receiver;

use common::*;
use receiver::*;
use reqwest::StatusCode;
use shed_api::calls::Call;
use shed_api::http::*;
use shed_api::wire::*;

struct World {
    nexus: Nexus,
    receiver: Receiver,
    account: Account,
    /// Phone A: its token, subscribed (UnifiedPush when there is no topic),
    /// and connected, in view.
    phone: Auth,
    app: Socket,
}

/// A nexus (behind a pretend proxy, so sign-ins can come from elsewhere)
/// with a browser and a phone subscribed to Web Push on the receiver and the
/// phone connected and in view; with a ntfy topic if `ntfy`.
async fn world(ntfy: bool, change: impl FnOnce(&mut shed_nexus::config::Config)) -> World {
    let receiver = Receiver::start().await;
    let topic = format!("{}/alerts-shed", receiver.base);
    let nexus = start_with(|c| {
        c.push.allow_private_endpoints = true;
        c.push.allow_http_endpoints = true;
        c.trusted_proxy = true;
        if ntfy {
            c.push.ntfy_url = Some(topic.clone());
        }
        change(c);
    })
    .await;
    let mut account = set_up(&nexus).await;
    let phone = Auth::Bearer(phone_token(&nexus, &mut account).await);
    let _: serde_json::Value = nexus.post("/api/v1/push/subscribe", &subscription(&receiver, "/wp/browser"), &account.browser).await;
    let _: serde_json::Value = nexus.post("/api/v1/push/subscribe", &subscription(&receiver, "/wp/phone"), &phone).await;
    // The phone's own sign-in went to the topic, if any.
    receiver.wait_for(if ntfy { 1 } else { 0 }).await;
    receiver.clear();
    let Auth::Bearer(token) = &phone else { unreachable!() };
    let (mut app, _) = connector(&nexus, token).await;
    visible(&mut app, true).await;
    World { nexus, receiver, account, phone, app }
}

/// Says whether the connector is in view; the frames that came before the
/// nexus had it (a request after it is answered after it is handled).
async fn visible(app: &mut Socket, visible: bool) -> Vec<ServerFrame> {
    send(app, &ClientFrame::Visible(visible)).await;
    settle(app).await
}

/// The frames that come until the nexus has handled what was sent before.
async fn settle(app: &mut Socket) -> Vec<ServerFrame> {
    send(app, &ClientFrame::Request(Request { id: 99, workspace: "nowhere".into(), call: Call::Previews })).await;
    let mut frames = Vec::new();
    loop {
        match next::<ServerFrame>(app).await {
            ServerFrame::Response(r) if r.id == 99 => return frames,
            frame => frames.push(frame),
        }
    }
}

fn alerts(frames: &[ServerFrame]) -> Vec<&NotificationFrame> {
    frames
        .iter()
        .filter_map(|f| match f {
            ServerFrame::Notification(n) if n.notification.kind == NotificationKind::Security => Some(n),
            _ => None,
        })
        .collect()
}

async fn live(app: &mut Socket) -> NotificationFrame {
    until(app, |f| match f {
        ServerFrame::Notification(n) => Some(n),
        _ => None,
    })
    .await
}

/// Signs a phone in from `from` (an address the pretend proxy saw); the
/// answer's status and body.
async fn login_from(nexus: &Nexus, from: &str, login: &Login) -> (StatusCode, String) {
    let answer = nexus
        .http
        .post(format!("{}/api/v1/login", nexus.base))
        .header("x-forwarded-for", from)
        .json(login)
        .send()
        .await
        .unwrap();
    (answer.status(), answer.text().await.unwrap())
}

fn phone_named(name: &str) -> DeviceHello {
    DeviceHello { kind: DeviceKind::Phone, name: name.into() }
}

#[tokio::test]
async fn a_sign_in_is_told_live_and_pushed_even_with_an_app_in_view() {
    let mut w = world(false, |_| {}).await;
    let code = w.account.recovery.pop().unwrap();
    let login = Login { username: "me".into(), password: PASSWORD.into(), totp: code, device: phone_named("Borrowed") };
    let _: SignedIn = w.nexus.post("/api/v1/login", &login, &Auth::None).await;

    let frame = live(&mut w.app).await;
    assert_eq!(frame.workspace, "", "from the nexus, not a workspace");
    assert_eq!(frame.notification.kind, NotificationKind::Security);
    assert_eq!(frame.notification.title, "New sign-in: Borrowed");
    assert!(frame.notification.body.starts_with("A phone from 127.0.0.1"), "{}", frame.notification.body);
    assert!(!frame.notification.present);

    // Pushed as well, though the phone's app is in view.
    let hits = w.receiver.wait_for(2).await;
    assert_eq!(w.receiver.on("/wp/browser").len(), 1, "{hits:?}");
    assert_eq!(w.receiver.on("/wp/phone").len(), 1, "{hits:?}");
    assert!(hits.iter().all(|h| h.headers["urgency"] == "high"));
}

#[tokio::test]
async fn the_device_that_did_it_is_not_told() {
    let mut w = world(false, |_| {}).await;
    // The phone changes the password: the browser hears of it before it is
    // signed out; the phone itself does not.
    let change = ChangePassword { old: PASSWORD.into(), new: "another long password".into(), totp: w.account.auth.code().await };
    let _: serde_json::Value = w.nexus.post("/api/v1/password", &change, &w.phone).await;
    let hits = w.receiver.wait_for(1).await;
    assert_eq!(w.receiver.on("/wp/browser").len(), 1, "{hits:?}");
    assert!(w.receiver.on("/wp/phone").is_empty(), "{hits:?}");
    assert!(alerts(&settle(&mut w.app).await).is_empty(), "not live to the phone that did it");
    assert_eq!(w.nexus.get_raw("/api/v1/devices", &w.account.browser).await.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn alerts_cannot_be_turned_off() {
    let mut w = world(false, |_| {}).await;
    // The phone wants nothing, and is out of view.
    let none = PushPrefs { kinds: vec![] };
    let _: serde_json::Value = w.nexus.post("/api/v1/push/prefs", &none, &w.phone).await;
    visible(&mut w.app, false).await;
    // A device revoked in the browser: the phone is told, the browser not.
    let other = phone_token(&w.nexus, &mut w.account).await;
    w.receiver.wait_for(2).await;
    w.receiver.clear();
    settle(&mut w.app).await;
    let devices: Vec<Device> = w.nexus.get("/api/v1/devices", &Auth::Bearer(other)).await;
    let id = devices.iter().find(|d| d.current).unwrap().id.clone();
    let revoke = Revoke { device: id, totp: w.account.auth.code().await };
    let _: serde_json::Value = w.nexus.post("/api/v1/devices/revoke", &revoke, &w.account.browser).await;
    let frame = live(&mut w.app).await;
    assert_eq!(frame.notification.title, "Revoked: Pixel");
    let hits = w.receiver.wait_for(1).await;
    assert_eq!(w.receiver.on("/wp/phone").len(), 1, "{hits:?}");
    assert!(w.receiver.on("/wp/browser").is_empty(), "{hits:?}");
}

#[tokio::test]
async fn on_the_topic_a_new_address_is_most_urgent() {
    let mut w = world(true, |_| {}).await;
    // From an address never seen: priority 5.
    let code = w.account.auth.code().await;
    let login = Login { username: "me".into(), password: PASSWORD.into(), totp: code, device: phone_named("Stolen") };
    let (status, body) = login_from(&w.nexus, "203.0.113.9", &login).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let hits = w.receiver.wait_for(2).await;
    let [ntfy] = &w.receiver.on("/alerts-shed")[..] else { panic!("one ntfy message: {hits:?}") };
    assert_eq!(ntfy.headers["title"], "New sign-in: Stolen");
    assert_eq!(ntfy.headers["priority"], "5");
    assert_eq!(ntfy.headers["click"], "shed://devices");
    let text = String::from_utf8(ntfy.body.clone()).unwrap();
    assert!(text.contains("203.0.113.9, an address not seen before"), "{text}");
    // Browsers by Web Push; phones read the topic.
    assert_eq!(w.receiver.on("/wp/browser").len(), 1, "{hits:?}");
    assert!(w.receiver.on("/wp/phone").is_empty(), "{hits:?}");

    // From where the devices are: priority 4.
    w.receiver.clear();
    let mut account = w.account;
    let _ = phone_token(&w.nexus, &mut account).await;
    let hits = w.receiver.wait_for(2).await;
    let [ntfy] = &w.receiver.on("/alerts-shed")[..] else { panic!("one ntfy message: {hits:?}") };
    assert_eq!(ntfy.headers["priority"], "4");
}

#[tokio::test]
async fn a_lockout_is_told_once_per_address() {
    let w = world(true, |c| c.login.max_failures = 2).await;
    let wrong = Login { username: "me".into(), password: "not the password".into(), totp: "123456".into(), device: phone() };
    for _ in 0..2 {
        assert_eq!(login_from(&w.nexus, "198.51.100.7", &wrong).await.0, StatusCode::UNAUTHORIZED);
    }
    // Locked out now: refused, and not told again.
    for _ in 0..3 {
        assert_eq!(login_from(&w.nexus, "198.51.100.7", &wrong).await.0, StatusCode::TOO_MANY_REQUESTS);
    }
    let hits = w.receiver.wait_for(2).await;
    let [ntfy] = &w.receiver.on("/alerts-shed")[..] else { panic!("one ntfy message: {hits:?}") };
    assert_eq!(ntfy.headers["title"], "Sign-ins locked out");
    assert_eq!(ntfy.headers["priority"], "5");
    assert!(String::from_utf8(ntfy.body.clone()).unwrap().contains("198.51.100.7"));
    assert_eq!(w.receiver.on("/wp/browser").len(), 1, "{hits:?}");

    // Another address locked out is told too.
    w.receiver.clear();
    for _ in 0..2 {
        login_from(&w.nexus, "198.51.100.8", &wrong).await;
    }
    let hits = w.receiver.wait_for(2).await;
    assert_eq!(w.receiver.on("/alerts-shed").len(), 1, "{hits:?}");
}

#[tokio::test]
async fn a_pairing_is_told_and_a_link_cannot_raise_alerts() {
    let mut w = world(false, |_| {}).await;
    let (_, token) = pair(&w.nexus, &mut w.account, "desk").await;
    let frame = live(&mut w.app).await;
    assert_eq!(frame.notification.title, "Workspace paired: desk");
    let hits = w.receiver.wait_for(1).await;
    assert_eq!(w.receiver.on("/wp/phone").len(), 1, "{hits:?}");
    assert!(w.receiver.on("/wp/browser").is_empty(), "approved in the browser: {hits:?}");

    // A workspace saying it is one is not believed.
    w.receiver.clear();
    let mut link = link(&w.nexus, &token).await;
    let fake = Notification {
        kind: NotificationKind::Security,
        title: "New sign-in".into(),
        body: String::new(),
        bench: None,
        tab: None,
        present: false,
    };
    send(&mut link, &LinkFrame::Notify(fake)).await;
    visible(&mut w.app, false).await;
    assert!(w.receiver.wait_for(1).await.is_empty());
    assert!(alerts(&settle(&mut w.app).await).is_empty());
}
