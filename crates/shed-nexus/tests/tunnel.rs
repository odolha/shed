//! Tunnels: Shed's desktop app speaking a workspace's keeper protocol
//! through the nexus (a fake link here).

mod common;

use common::*;
use shed_api::ErrorCode;
use shed_api::http::{AuditEntry, Device, DeviceKind, Revoke};
use shed_api::wire::*;

fn tunnel_closed(frame: ServerFrame) -> Option<TunnelClosed> {
    match frame {
        ServerFrame::TunnelClosed(c) => Some(c),
        _ => None,
    }
}

#[tokio::test]
async fn a_desktop_tunnel_carries_bytes_both_ways() {
    let nexus = start().await;
    let mut account = set_up(&nexus).await;
    let (workspace, token) = pair(&nexus, &mut account, "desk").await;
    let mut link = link(&nexus, &token).await;
    let desktop = desktop_token(&nexus, &mut account).await;
    let (mut app, _) = connector(&nexus, &desktop).await;

    send(&mut app, &ClientFrame::TunnelOpen(TunnelOpen { id: 9, workspace: workspace.clone() })).await;
    let NexusFrame::TunnelOpen(TunnelId { id: nid }) = next(&mut link).await else { panic!("no tunnel asked for") };
    send(&mut link, &LinkFrame::TunnelOpened(TunnelId { id: nid })).await;
    let opened = until(&mut app, |f| match f {
        ServerFrame::TunnelOpened(t) => Some(t),
        _ => None,
    })
    .await;
    assert_eq!(opened.id, 9);

    // Bytes go under each side's own id, untouched (not UTF-8 either).
    let payload: Vec<u8> = (0..=255).collect();
    send_binary(&mut app, tunnel_data(9, &payload)).await;
    assert_eq!(next_binary(&mut link).await.unwrap(), tunnel_data(nid, &payload));
    send_binary(&mut link, tunnel_data(nid, b"\xff\x00answer")).await;
    assert_eq!(next_binary(&mut app).await.unwrap(), tunnel_data(9, b"\xff\x00answer"));
    // Data for a tunnel that is not there goes nowhere.
    send_binary(&mut app, tunnel_data(10, b"lost")).await;
    send_binary(&mut app, vec![1, 2]).await;

    // Opening is in the audit log.
    let audit: Vec<AuditEntry> = nexus.get("/api/v1/audit", &account.browser).await;
    assert!(audit.iter().any(|e| e.what == "tunnel-open" && e.detail == "desk"), "{audit:?}");

    // The connector closes it: the link hears.
    send(&mut app, &ClientFrame::TunnelClose(TunnelId { id: 9 })).await;
    assert_eq!(next::<NexusFrame>(&mut link).await, NexusFrame::TunnelClose(TunnelId { id: nid }));
    assert_eq!(until(&mut app, tunnel_closed).await, TunnelClosed { id: 9, error: None });

    // The link closes one: the connector hears, with why.
    send(&mut app, &ClientFrame::TunnelOpen(TunnelOpen { id: 11, workspace: workspace.clone() })).await;
    let NexusFrame::TunnelOpen(TunnelId { id: nid }) = next(&mut link).await else { panic!() };
    let refused = shed_api::ApiError::new(ErrorCode::Forbidden, "desktop clients are off");
    send(&mut link, &LinkFrame::TunnelClosed(TunnelClosed { id: nid, error: Some(refused.clone()) })).await;
    assert_eq!(until(&mut app, tunnel_closed).await, TunnelClosed { id: 11, error: Some(refused) });

    // The connector goes: its tunnels close at the link.
    send(&mut app, &ClientFrame::TunnelOpen(TunnelOpen { id: 12, workspace: workspace.clone() })).await;
    let NexusFrame::TunnelOpen(TunnelId { id: nid }) = next(&mut link).await else { panic!() };
    drop(app);
    assert_eq!(next::<NexusFrame>(&mut link).await, NexusFrame::TunnelClose(TunnelId { id: nid }));
}

#[tokio::test]
async fn only_desktops_open_tunnels() {
    let nexus = start().await;
    let mut account = set_up(&nexus).await;
    let (workspace, token) = pair(&nexus, &mut account, "desk").await;
    let mut link = link(&nexus, &token).await;
    let phone = phone_token(&nexus, &mut account).await;
    let (mut app, _) = connector(&nexus, &phone).await;

    send(&mut app, &ClientFrame::TunnelOpen(TunnelOpen { id: 1, workspace: workspace.clone() })).await;
    let closed = until(&mut app, tunnel_closed).await;
    assert_eq!((closed.id, closed.error.unwrap().code), (1, ErrorCode::Forbidden));
    // Nothing reached the link: the next frame it gets is the request after.
    send(&mut app, &ClientFrame::Request(Request { id: 2, workspace, call: shed_api::calls::Call::Previews })).await;
    assert!(matches!(next::<NexusFrame>(&mut link).await, NexusFrame::Request(_)));

    // A desktop shows as one in the devices.
    let _ = desktop_token(&nexus, &mut account).await;
    let devices: Vec<Device> = nexus.get("/api/v1/devices", &account.browser).await;
    assert!(devices.iter().any(|d| d.kind == DeviceKind::Desktop && d.name == "Shed on laptop"));
}

#[tokio::test]
async fn tunnels_close_when_the_workspace_goes_or_the_desktop_is_revoked() {
    let nexus = start().await;
    let mut account = set_up(&nexus).await;
    let (workspace, token) = pair(&nexus, &mut account, "desk").await;
    let desktop = desktop_token(&nexus, &mut account).await;
    let (mut app, _) = connector(&nexus, &desktop).await;

    // Offline: refused at once.
    send(&mut app, &ClientFrame::TunnelOpen(TunnelOpen { id: 1, workspace: workspace.clone() })).await;
    let closed = until(&mut app, tunnel_closed).await;
    assert_eq!((closed.id, closed.error.unwrap().code), (1, ErrorCode::Offline));

    // The workspace goes while a tunnel is open.
    let mut ws = link(&nexus, &token).await;
    send(&mut app, &ClientFrame::TunnelOpen(TunnelOpen { id: 2, workspace: workspace.clone() })).await;
    let NexusFrame::TunnelOpen(TunnelId { id: nid }) = next(&mut ws).await else { panic!() };
    send(&mut ws, &LinkFrame::TunnelOpened(TunnelId { id: nid })).await;
    drop(ws);
    let closed = until(&mut app, tunnel_closed).await;
    assert_eq!((closed.id, closed.error.unwrap().code), (2, ErrorCode::Offline));

    // Revoking the desktop ends its connection, and the tunnel with it.
    let mut ws = link(&nexus, &token).await;
    send(&mut app, &ClientFrame::TunnelOpen(TunnelOpen { id: 3, workspace: workspace.clone() })).await;
    let NexusFrame::TunnelOpen(TunnelId { id: nid }) = next(&mut ws).await else { panic!() };
    let devices: Vec<Device> = nexus.get("/api/v1/devices", &account.browser).await;
    let id = devices.iter().find(|d| d.kind == DeviceKind::Desktop).unwrap().id.clone();
    let revoke = Revoke { device: id, totp: account.auth.code().await };
    let _: serde_json::Value = nexus.post("/api/v1/devices/revoke", &revoke, &account.browser).await;
    assert!(closes(&mut app).await);
    assert_eq!(next::<NexusFrame>(&mut ws).await, NexusFrame::TunnelClose(TunnelId { id: nid }));
}

#[tokio::test]
async fn a_tunnel_that_cannot_keep_up_is_closed() {
    let nexus = start().await;
    let mut account = set_up(&nexus).await;
    let (workspace, token) = pair(&nexus, &mut account, "desk").await;
    let mut ws = link(&nexus, &token).await;
    let desktop = desktop_token(&nexus, &mut account).await;
    let (mut app, _) = connector(&nexus, &desktop).await;
    send(&mut app, &ClientFrame::TunnelOpen(TunnelOpen { id: 1, workspace })).await;
    let NexusFrame::TunnelOpen(TunnelId { id: nid }) = next(&mut ws).await else { panic!() };
    send(&mut ws, &LinkFrame::TunnelOpened(TunnelId { id: nid })).await;
    // The desktop reads nothing while the workspace sends far more than the
    // nexus keeps waiting for it.
    let chunk = vec![7u8; 1024 * 1024];
    let mut closed = false;
    for _ in 0..200 {
        send_binary(&mut ws, tunnel_data(nid, &chunk)).await;
        if let Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t)))) =
            tokio::time::timeout(std::time::Duration::from_millis(1), futures_util::StreamExt::next(&mut ws)).await
        {
            assert!(t.contains("TunnelClose"), "{t}");
            closed = true;
            break;
        }
    }
    assert!(closed, "the link was told the tunnel closed");
    // The desktop hears why, after what was queued.
    let closed = until(&mut app, tunnel_closed).await;
    assert_eq!(closed.error.unwrap().code, ErrorCode::Failed);
}
