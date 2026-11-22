//! Shed's desktop app showing this workspace through the nexus: a tunnel
//! to the keeper, carrying its own protocol. First against a fake nexus,
//! then end to end (real nexus, link and keeper, `connect_remote`'s tunnel).

mod common;
#[path = "../../shed-nexus/tests/common/mod.rs"]
mod nexus;

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use shed_api::ErrorCode;
use shed_api::wire::*;
use shed_proto::{ClientKind, ClientMsg, Frame, FrameDecoder, KeeperMsg, PROTOCOL_VERSION};
use tokio_tungstenite::tungstenite::Message;

/// Opens tunnel `id` at the fake nexus; what the link answered.
async fn open(nexus: &mut common::FakeNexus, id: u32) -> LinkFrame {
    let device = Some(TunnelDevice { id: "d1".into(), name: "Shed on laptop".into() });
    let open = serde_json::to_string(&NexusFrame::TunnelOpen(LinkTunnelOpen { id, device })).unwrap();
    nexus.socket.send(Message::text(open)).await.unwrap();
    nexus
        .until("the tunnel's answer", |f, _| match f {
            LinkFrame::TunnelOpened(t) if t.id == id => Some(f.clone()),
            LinkFrame::TunnelClosed(t) if t.id == id => Some(f.clone()),
            _ => None,
        })
        .await
}

async fn send_frame(nexus: &mut common::FakeNexus, id: u32, frame: Frame) {
    let mut bytes = Vec::new();
    frame.encode_into(&mut bytes);
    nexus.socket.send(Message::binary(tunnel_data(id, &bytes))).await.unwrap();
}

/// Keeper frames from the tunnel until `pick` takes one.
async fn until_frame<T>(nexus: &mut common::FakeNexus, id: u32, decoder: &mut FrameDecoder, mut pick: impl FnMut(Frame) -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        while let Some(frame) = decoder.next_frame().unwrap() {
            if let Some(found) = pick(frame) {
                return found;
            }
        }
        let msg = tokio::time::timeout_at(deadline, nexus.socket.next()).await.expect("a keeper frame in time").expect("open").unwrap();
        if let Message::Binary(data) = msg {
            let (tid, bytes) = split_tunnel_data(&data).unwrap();
            assert_eq!(tid, id);
            decoder.push(bytes);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tunnels_reach_the_keeper_only_when_desktops_are_let_in() {
    // Off (the default): refused.
    let dir = tempfile::tempdir().unwrap();
    let env = common::keeper(dir.path());
    let (listener, url) = common::listen().await;
    let link = common::link(&env, url, "t0k", Policy::Full);
    let (mut fake, _) = common::accept(&listener).await;
    let LinkFrame::TunnelClosed(closed) = open(&mut fake, 1).await else { panic!("opened") };
    let error = closed.error.unwrap();
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert!(error.message.contains("desktop_clients"), "{}", error.message);
    link.abort();

    // On, but view only: refused too.
    let (listener, url) = common::listen().await;
    let link = common::link_with(&env, url, "t0k", Policy::View, true);
    let (mut fake, _) = common::accept(&listener).await;
    let LinkFrame::TunnelClosed(closed) = open(&mut fake, 1).await else { panic!("opened") };
    assert_eq!(closed.error.unwrap().code, ErrorCode::Forbidden);
    link.abort();

    // On, with full control: the keeper's own protocol, both ways.
    let (listener, url) = common::listen().await;
    let _link = common::link_with(&env, url, "t0k", Policy::Full, true);
    let (mut fake, _) = common::accept(&listener).await;
    assert_eq!(open(&mut fake, 7).await, LinkFrame::TunnelOpened(TunnelId { id: 7 }));
    let mut decoder = FrameDecoder::new();
    send_frame(&mut fake, 7, Frame::Client(ClientMsg::Hello { version: PROTOCOL_VERSION, kind: ClientKind::Window })).await;
    let version = until_frame(&mut fake, 7, &mut decoder, |f| match f {
        Frame::Keeper(KeeperMsg::Welcome { version, .. }) => Some(version),
        _ => None,
    })
    .await;
    assert_eq!(version, PROTOCOL_VERSION);
    send_frame(&mut fake, 7, Frame::Client(ClientMsg::Attach { bench: None })).await;
    let tree = until_frame(&mut fake, 7, &mut decoder, |f| match f {
        Frame::Keeper(KeeperMsg::TreeChanged(tree)) if !tree.benches.is_empty() => Some(tree),
        _ => None,
    })
    .await;
    assert_eq!(tree.benches.len(), 1);
    // The link said whose connection it is: even saying `Window`, the
    // desktop is not this machine's window process.
    let (conn, rx) = shed_client::keeper_conn::connect_at(&env.socket, ClientKind::Cli).unwrap();
    conn.send_msg(ClientMsg::Status);
    loop {
        if let Ok(Frame::Keeper(KeeperMsg::Status(s))) = rx.recv().await {
            assert_eq!(s.windows, 0, "a tunnelled desktop counts as no window");
            assert!(s.open.is_empty());
            break;
        }
    }

    // Closed by the nexus: the keeper sees the client go (the bench stays).
    let close = serde_json::to_string(&NexusFrame::TunnelClose(TunnelId { id: 7 })).unwrap();
    fake.socket.send(Message::text(close)).await.unwrap();
    conn.send_msg(ClientMsg::Status);
    let mut clients = u32::MAX;
    for _ in 0..50 {
        if let Ok(Frame::Keeper(KeeperMsg::Status(s))) = rx.recv().await {
            clients = s.clients;
            assert_eq!(s.tree.benches.len(), 1);
            // This CLI and the link itself.
            if clients <= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            conn.send_msg(ClientMsg::Status);
        }
    }
    assert!(clients <= 2, "the tunnel's client is gone ({clients} clients)");
}

/// Pairs the keeper of `env` with the real nexus; its workspace id and token.
async fn pair(hub: &nexus::Nexus, account: &mut nexus::Account) -> (String, String) {
    nexus::pair(hub, account, "desk").await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_desktop_speaks_to_another_workspaces_keeper_through_the_nexus() {
    let dir = tempfile::tempdir().unwrap();
    let env = common::keeper(dir.path());
    let hub = nexus::start().await;
    let mut account = nexus::set_up(&hub).await;
    let (workspace, token) = pair(&hub, &mut account).await;
    let _link = common::link_with(&env, hub.base.clone(), &token, Policy::Full, true);
    // Signed in as `shed nexus signin` does it.
    let code = account.recovery.pop().unwrap();
    let signed = shed_link::pairing::sign_in(&hub.base, "me", nexus::PASSWORD, &code, None).await.unwrap();
    assert_eq!(signed.username, "me");
    let desktop = signed.token.clone();
    // Online once a connector sees it so.
    for _ in 0..100 {
        let (_, w) = nexus::connector(&hub, &desktop).await;
        if w.workspaces.first().is_some_and(|w| w.online) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // The workspace list, as the switcher reads it.
    let sign_in = shed_client::nexus_client::SignIn { url: hub.base.clone(), username: "me".into(), device: String::new(), token: desktop.clone(), expires: None };
    let listed = tokio::task::spawn_blocking(move || shed_client::nexus_client::workspaces(&sign_in, None)).await.unwrap().unwrap();
    assert_eq!(listed.iter().map(|w| (w.name.as_str(), w.online)).collect::<Vec<_>>(), [("desk", true)]);

    let (base, token, ws) = (hub.base.clone(), desktop.clone(), workspace.clone());
    let (conn, rx) = tokio::task::spawn_blocking(move || shed_client::nexus_client::connect_tunnel(&base, &token, &ws, None, ClientKind::Remote))
        .await
        .unwrap()
        .expect("the tunnel opens");
    // The keeper's welcome came first.
    let first = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.unwrap().unwrap();
    assert!(matches!(first, Frame::Keeper(KeeperMsg::Welcome { version: PROTOCOL_VERSION, .. })), "{first:?}");
    conn.send_msg(ClientMsg::Attach { bench: None });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let (bench, pane) = loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await.expect("a tree in time").expect("open") {
            Frame::Keeper(KeeperMsg::TreeChanged(tree)) if !tree.benches.is_empty() => {
                let b = &tree.benches[0];
                break (b.id, b.tabs[0].panes[0].id);
            }
            _ => {}
        }
    };
    // Typing and its output, as bytes, through the tunnel.
    conn.send(Frame::Input { pane, bytes: b"echo through''-the-nexus\r".to_vec() });
    let mut seen = String::new();
    while !seen.contains("through-the-nexus") {
        match tokio::time::timeout_at(deadline, rx.recv()).await.expect("the output in time").expect("open") {
            Frame::Output { pane: p, bytes } if p == pane => seen.push_str(&String::from_utf8_lossy(&bytes)),
            Frame::Keeper(KeeperMsg::Replay { pane: p, bytes, .. }) if p == pane => seen.push_str(&String::from_utf8_lossy(&bytes)),
            _ => {}
        }
    }
    let _ = bench;

    // Closing it ends the channel; the keeper keeps the bench.
    conn.close();
    let ended = tokio::time::timeout(Duration::from_secs(10), async {
        while rx.recv().await.is_ok() {}
    })
    .await;
    assert!(ended.is_ok(), "the frames channel closes");

    // A workspace that does not let desktops in says so, in words.
    let dir2 = tempfile::tempdir().unwrap();
    let env2 = common::keeper(dir2.path());
    let (workspace2, token2) = nexus::pair(&hub, &mut account, "shut").await;
    let _link2 = common::link(&env2, hub.base.clone(), &token2, Policy::Full);
    let mut error = String::new();
    for _ in 0..100 {
        let (base, token, ws) = (hub.base.clone(), desktop.clone(), workspace2.clone());
        let result = tokio::task::spawn_blocking(move || shed_client::nexus_client::connect_tunnel(&base, &token, &ws, None, ClientKind::Remote)).await.unwrap();
        error = format!("{:#}", result.err().expect("refused"));
        if !error.contains("offline") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(error.contains("desktop_clients"), "{error}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signing_out_revokes_the_desktop_at_the_nexus() {
    let hub = nexus::start().await;
    let mut account = nexus::set_up(&hub).await;
    let code = account.recovery.pop().unwrap();
    let signed = shed_link::pairing::sign_in(&hub.base, "me", nexus::PASSWORD, &code, None).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nexus-client.json");
    signed.save_to(&path).unwrap();
    let bearer = [("authorization", format!("Bearer {}", signed.token))];
    assert!(nexus::connect(&hub.ws_url("/api"), &bearer).await.is_ok(), "signed in");

    // The nexus is told: the token stops working there, and it is gone here.
    let out = shed_link::pairing::sign_out_from(&path, None).await.unwrap();
    assert_eq!(out, shed_link::pairing::SignedOut::Revoked);
    assert!(!path.exists());
    assert_eq!(nexus::connect(&hub.ws_url("/api"), &bearer).await.err(), Some(401));
    assert_eq!(shed_link::pairing::sign_out_from(&path, None).await.unwrap(), shed_link::pairing::SignedOut::NotSignedIn);

    // A nexus that cannot be reached: forgotten here, and said so.
    let code = account.recovery.pop().unwrap();
    let mut signed = shed_link::pairing::sign_in(&hub.base, "me", nexus::PASSWORD, &code, None).await.unwrap();
    signed.url = "http://127.0.0.1:9".into();
    signed.save_to(&path).unwrap();
    let out = shed_link::pairing::sign_out_from(&path, None).await.unwrap();
    assert!(matches!(&out, shed_link::pairing::SignedOut::HereOnly(_)), "{out:?}");
    assert!(out.message().contains("web app"), "{}", out.message());
    assert!(!path.exists());
}

/// The link's settings and the snapshot it tells changes by come from one
/// read; a `shed.toml` that stops reading refuses new tunnels until it reads.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tunnels_follow_the_settings_the_link_runs_with() {
    let dir = tempfile::tempdir().unwrap();
    let env = common::keeper(dir.path());
    let config = dir.path().join("link-shed.toml");
    let on = "[nexus]\nurl = \"https://n.example.com\"\ndesktop_clients = true\n";
    let read = |text: &str| toml_settings(text);

    // Started with desktops let in, while the file already says otherwise
    // (changed between the read and the start): it starts again.
    std::fs::write(&config, "[nexus]\nurl = \"https://n.example.com\"\n").unwrap();
    let (listener, url) = common::listen().await;
    let mut opts = common::options(&env, url, "t0k", Policy::Full, true);
    opts.config_file = Some(config.clone());
    opts.settings = Some(read(on));
    let link = tokio::spawn(shed_link::run(opts));
    let (_fake, _) = common::accept(&listener).await;
    let exit = tokio::time::timeout(Duration::from_secs(10), link).await.expect("the link stops").unwrap().unwrap();
    assert_eq!(exit, shed_link::Exit::Restart);

    // Running with them: a file that does not read refuses new tunnels
    // until it reads again.
    std::fs::write(&config, on).unwrap();
    let (listener, url) = common::listen().await;
    let mut opts = common::options(&env, url, "t0k", Policy::Full, true);
    opts.config_file = Some(config.clone());
    opts.settings = Some(read(on));
    let _link = tokio::spawn(shed_link::run(opts));
    let (mut fake, _) = common::accept(&listener).await;
    assert_eq!(open(&mut fake, 1).await, LinkFrame::TunnelOpened(TunnelId { id: 1 }));
    std::fs::write(&config, "[nexus\nnot toml").unwrap();
    let mut refused = None;
    for id in 2..40 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if let LinkFrame::TunnelClosed(closed) = open(&mut fake, id).await {
            refused = closed.error;
            break;
        }
    }
    let refused = refused.expect("refused while shed.toml does not read");
    assert_eq!(refused.code, ErrorCode::Forbidden);
    assert!(refused.message.contains("does not read"), "{}", refused.message);
    std::fs::write(&config, on).unwrap();
    let mut opened = false;
    for id in 40..80 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if matches!(open(&mut fake, id).await, LinkFrame::TunnelOpened(_)) {
            opened = true;
            break;
        }
    }
    assert!(opened, "let in again once it reads");
}

fn toml_settings(text: &str) -> shed_model::config::NexusConfig {
    let path = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(path.path(), text).unwrap();
    shed_model::config::Config::load_from(path.path()).unwrap().nexus
}

/// A desktop's sign-in ends after some days; signing in again renews it,
/// and the sign-in before is revoked rather than left behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signing_in_again_renews_and_revokes_the_old_sign_in() {
    let hub = nexus::start().await;
    let mut account = nexus::set_up(&hub).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nexus-client.json");
    let code = account.recovery.pop().unwrap();
    let first = shed_link::pairing::sign_in_and_keep(&path, &hub.base, "me", nexus::PASSWORD, &code, None).await.unwrap();
    let now = shed_link::pairing::now();
    let expires = first.expires.expect("a desktop's sign-in ends");
    assert!((now + 29 * 86400..=now + 31 * 86400).contains(&expires), "{expires} ({now})");
    assert!(!first.ended(now) && first.ends_soon(now).is_none());
    assert_eq!(first.ends_soon(expires - 3 * 86400 + 10), Some(3));
    assert!(first.ended(expires));
    assert_eq!(shed_client::nexus_client::SignIn::load_from(&path), Some(first.clone()));

    let code = account.recovery.pop().unwrap();
    let second = shed_link::pairing::sign_in_and_keep(&path, &hub.base, "me", nexus::PASSWORD, &code, None).await.unwrap();
    assert_ne!(second.device, first.device);
    assert_eq!(shed_client::nexus_client::SignIn::load_from(&path), Some(second.clone()));
    let bearer = |token: &str| [("authorization", format!("Bearer {token}"))];
    assert_eq!(nexus::connect(&hub.ws_url("/api"), &bearer(&first.token)).await.err(), Some(401), "the old one is revoked");
    assert!(nexus::connect(&hub.ws_url("/api"), &bearer(&second.token)).await.is_ok());
    // Phones' tokens do not end.
    let phone = nexus::phone_token(&hub, &mut account).await;
    let devices: Vec<shed_api::http::Device> = hub.get("/api/v1/devices", &account.browser).await;
    assert_eq!(devices.iter().filter(|d| d.kind == shed_api::http::DeviceKind::Desktop).count(), 1);
    let _ = phone;
}
