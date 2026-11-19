//! Pairing a workspace: approved, rejected, expired.

mod common;

use common::*;
use reqwest::StatusCode;
use shed_api::ErrorCode;
use shed_api::http::*;

async fn ask(nexus: &Nexus, name: &str) -> PairStarted {
    nexus.post("/api/v1/pair/start", &PairStart { name: name.into(), version: "0.1.0".into() }, &Auth::None).await
}

async fn poll(nexus: &Nexus, started: &PairStarted) -> PairState {
    nexus.post("/api/v1/pair/poll", &PairPoll { poll: started.poll.clone() }, &Auth::None).await
}

#[tokio::test]
async fn an_approved_workspace_gets_its_token_once() {
    let nexus = start().await;
    let mut account = set_up(&nexus).await;
    let started = ask(&nexus, "desk").await;
    assert_eq!(started.code.len(), 9);
    assert_eq!(&started.code[4..5], "-");
    assert_eq!(poll(&nexus, &started).await, PairState::Pending);

    let pending: Vec<PendingPairing> = nexus.get("/api/v1/pair/pending", &account.browser).await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].name, "desk");
    assert_eq!(pending[0].address, "127.0.0.1");

    // Approving needs a fresh code.
    let wrong = PairApprove { code: started.code.clone(), name: "Desk PC".into(), totp: "000000".into() };
    let err = nexus.post_err("/api/v1/pair/approve", &wrong, &account.browser, StatusCode::FORBIDDEN).await;
    assert_eq!(err.code, ErrorCode::WrongCode);
    // ... and a signed-in user.
    let right = PairApprove { totp: account.auth.code().await, code: started.code.to_lowercase(), ..wrong };
    nexus.post_err("/api/v1/pair/approve", &right, &Auth::None, StatusCode::UNAUTHORIZED).await;
    let _: serde_json::Value = nexus.post("/api/v1/pair/approve", &right, &account.browser).await;

    let PairState::Approved(approved) = poll(&nexus, &started).await else { panic!("not approved") };
    assert_eq!(approved.name, "Desk PC");
    assert_eq!(poll(&nexus, &started).await, PairState::Expired);

    let devices: Vec<Device> = nexus.get("/api/v1/devices", &account.browser).await;
    let workspace = devices.iter().find(|d| d.kind == DeviceKind::Workspace).unwrap();
    assert_eq!(workspace.id, approved.workspace);
    assert_eq!(workspace.name, "Desk PC");

    // The token links, and does not sign in to the API.
    let _link = link(&nexus, &approved.token).await;
    let as_phone = Auth::Bearer(approved.token.clone());
    assert_eq!(nexus.get_raw("/api/v1/devices", &as_phone).await.status(), StatusCode::UNAUTHORIZED);
    let refused = connect(&nexus.ws_url("/api"), &[("authorization", format!("Bearer {}", approved.token))]).await;
    assert_eq!(refused.err(), Some(401));

    let audit: Vec<AuditEntry> = nexus.get("/api/v1/audit", &account.browser).await;
    for what in ["pair-request", "pair-approve", "paired"] {
        assert!(audit.iter().any(|e| e.what == what), "{what}");
    }
}

#[tokio::test]
async fn a_rejected_workspace_hears_so() {
    let nexus = start().await;
    let account = set_up(&nexus).await;
    let started = ask(&nexus, "stranger").await;
    let reject = PairReject { code: started.code.clone() };
    let _: serde_json::Value = nexus.post("/api/v1/pair/reject", &reject, &account.browser).await;
    assert_eq!(poll(&nexus, &started).await, PairState::Rejected);
    nexus.post_err("/api/v1/pair/reject", &reject, &account.browser, StatusCode::NOT_FOUND).await;
    let pending: Vec<PendingPairing> = nexus.get("/api/v1/pair/pending", &account.browser).await;
    assert!(pending.is_empty());
}

#[tokio::test]
async fn an_old_request_expires() {
    let nexus = start().await;
    let mut account = set_up(&nexus).await;
    let started = ask(&nexus, "late").await;
    let db = rusqlite::Connection::open(nexus.dir.path().join("nexus.db")).unwrap();
    db.execute("UPDATE pairings SET expires = 1", []).unwrap();

    let approve = PairApprove { code: started.code.clone(), name: "late".into(), totp: account.auth.code().await };
    nexus.post_err("/api/v1/pair/approve", &approve, &account.browser, StatusCode::NOT_FOUND).await;
    assert_eq!(poll(&nexus, &started).await, PairState::Expired);
}

#[tokio::test]
async fn pairing_requests_are_rate_limited() {
    let nexus = start_with(|c| c.login.pair_per_minute = 2).await;
    let _account = set_up(&nexus).await;
    ask(&nexus, "a").await;
    ask(&nexus, "b").await;
    let start = PairStart { name: "c".into(), version: "0.1.0".into() };
    let err = nexus.post_err("/api/v1/pair/start", &start, &Auth::None, StatusCode::TOO_MANY_REQUESTS).await;
    assert_eq!(err.code, ErrorCode::RateLimited);
}
