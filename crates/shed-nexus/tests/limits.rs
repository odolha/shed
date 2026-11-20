//! Limits that keep attackers from locking the owner out or wearing the
//! nexus down: per-address and global sign-in limits, attempts at once,
//! the proxy's addresses, idle connections, the setup link.

mod common;

use std::time::Duration;

use common::*;
use reqwest::StatusCode;
use shed_api::ErrorCode;
use shed_api::http::*;
use tokio::io::AsyncReadExt;

/// A wrong sign-in from `ip`, through the (trusted) proxy; its status.
async fn wrong_login_from(nexus: &Nexus, ip: &str) -> StatusCode {
    let wrong = Login { username: "me".into(), password: "wrong password!".into(), totp: "123456".into(), device: phone() };
    let answer = nexus
        .http
        .post(format!("{}/api/v1/login", nexus.base))
        .header("X-Forwarded-For", ip)
        .json(&wrong)
        .send()
        .await
        .unwrap();
    answer.status()
}

#[tokio::test]
async fn a_distributed_attack_does_not_lock_the_owner_out() {
    let nexus = start_with(|c| {
        c.trusted_proxy = true;
        c.login.global_max_failures = 5;
        c.login.slow_per_minute = 1;
    })
    .await;
    // Set up from 127.0.0.1 (no proxy header): an address that signed in.
    let mut account = set_up(&nexus).await;
    for i in 0..8 {
        let status = wrong_login_from(&nexus, &format!("203.0.113.{i}")).await;
        assert!(status == StatusCode::UNAUTHORIZED || status == StatusCode::TOO_MANY_REQUESTS, "{status}");
    }
    // Strangers are slowed down now (and soon refused) ...
    let mut refused = false;
    for i in 100..104 {
        refused |= wrong_login_from(&nexus, &format!("203.0.113.{i}")).await == StatusCode::TOO_MANY_REQUESTS;
    }
    assert!(refused, "strangers are held back");
    // ... but the owner's address still signs in,
    let code = account.recovery.pop().unwrap();
    let login = Login { username: "me".into(), password: PASSWORD.into(), totp: code, device: phone() };
    let signed: SignedIn = nexus.post("/api/v1/login", &login, &Auth::None).await;
    assert!(signed.token.is_some());
    // and a signed-in device still uses one-time codes (pairing, revoking).
    let (workspace, _) = pair(&nexus, &mut account, "desk").await;
    let revoke = Revoke { device: workspace, totp: account.auth.code().await };
    let _: serde_json::Value = nexus.post("/api/v1/devices/revoke", &revoke, &account.browser).await;
}

#[tokio::test]
async fn wrong_codes_lock_only_their_device() {
    let nexus = start_with(|c| c.login.max_failures = 2).await;
    let mut account = set_up(&nexus).await;
    let phone = Auth::Bearer(phone_token(&nexus, &mut account).await);
    let wrong = Revoke { device: "x".into(), totp: "000000".into() };
    for _ in 0..2 {
        nexus.post_err("/api/v1/devices/revoke", &wrong, &phone, StatusCode::FORBIDDEN).await;
    }
    let err = nexus.post_err("/api/v1/devices/revoke", &wrong, &phone, StatusCode::TOO_MANY_REQUESTS).await;
    assert_eq!(err.code, ErrorCode::RateLimited);
    // The browser, from the same address, is not locked out; nor is signing in.
    let (workspace, _) = pair(&nexus, &mut account, "desk").await;
    let revoke = Revoke { device: workspace, totp: account.auth.code().await };
    let _: serde_json::Value = nexus.post("/api/v1/devices/revoke", &revoke, &account.browser).await;
    let code = account.recovery.pop().unwrap();
    let login = Login { username: "me".into(), password: PASSWORD.into(), totp: code, device: browser() };
    let _: SignedIn = nexus.post("/api/v1/login", &login, &Auth::None).await;
}

#[tokio::test]
async fn sign_ins_at_once_count_before_they_are_checked() {
    let nexus = start_with(|c| c.login.max_failures = 3).await;
    let _account = set_up(&nexus).await;
    let wrong = Login { username: "me".into(), password: "wrong password!".into(), totp: "123456".into(), device: phone() };
    let attempts = (0..12).map(|_| nexus.post_raw("/api/v1/login", &wrong, &Auth::None));
    let statuses: Vec<StatusCode> = futures_util::future::join_all(attempts).await.iter().map(|a| a.status()).collect();
    let checked = statuses.iter().filter(|s| **s == StatusCode::UNAUTHORIZED).count();
    let refused = statuses.iter().filter(|s| **s == StatusCode::TOO_MANY_REQUESTS).count();
    assert_eq!(checked, 3, "{statuses:?}");
    assert_eq!(refused, 9, "{statuses:?}");
}

#[tokio::test]
async fn forwarded_addresses_come_from_trusted_proxies_only() {
    // No trusted proxy: the header is ignored, failures count for the peer.
    let nexus = start_with(|c| c.login.max_failures = 2).await;
    let _account = set_up(&nexus).await;
    for i in 0..2 {
        wrong_login_from(&nexus, &format!("203.0.113.{i}")).await;
    }
    assert_eq!(wrong_login_from(&nexus, "203.0.113.9").await, StatusCode::TOO_MANY_REQUESTS);

    // Behind a trusted proxy: the right-most address that is not a proxy;
    // what the client wrote further left does not count.
    let nexus = start_with(|c| c.trusted_proxies = vec!["127.0.0.1/32".into(), "10.0.0.0/8".into()]).await;
    let account = set_up(&nexus).await;
    let wrong = Login { username: "me".into(), password: "wrong password!".into(), totp: "123456".into(), device: phone() };
    nexus
        .http
        .post(format!("{}/api/v1/login", nexus.base))
        .header("X-Forwarded-For", "1.2.3.4, 198.51.100.7, 10.0.0.2")
        .json(&wrong)
        .send()
        .await
        .unwrap();
    let audit: Vec<AuditEntry> = nexus.get("/api/v1/audit", &account.browser).await;
    let failed = audit.iter().find(|e| e.what == "login-failed").unwrap();
    assert_eq!(failed.address, "198.51.100.7");
}

#[tokio::test]
async fn idle_connections_are_closed() {
    let nexus = start_with(|c| c.header_timeout_secs = 1).await;
    let mut tcp = tokio::net::TcpStream::connect(nexus.server.addr).await.unwrap();
    // Half a request line, then nothing.
    use tokio::io::AsyncWriteExt;
    tcp.write_all(b"GET /api/v1/sta").await.unwrap();
    let mut buf = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(5), tcp.read_to_end(&mut buf)).await;
    assert!(read.is_ok(), "the nexus closes a connection that does not send its headers");
}

#[tokio::test]
async fn connections_at_once_are_capped() {
    let nexus = start_with(|c| {
        c.max_connections = 1;
        c.header_timeout_secs = 2;
    })
    .await;
    // One idle connection takes the only place ...
    let _idle = tokio::net::TcpStream::connect(nexus.server.addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let started = std::time::Instant::now();
    // ... so a request waits until it is closed for being idle.
    let status: NexusStatus = nexus.get("/api/v1/status", &Auth::None).await;
    assert!(!status.ready);
    assert!(started.elapsed() >= Duration::from_millis(1000), "{:?}", started.elapsed());
}

#[tokio::test]
async fn the_setup_link_is_kept_in_a_private_file() {
    let nexus = start().await;
    let code = nexus.server.setup_code.clone().unwrap();
    let file = nexus.dir.path().join("setup-link.txt");
    let link = std::fs::read_to_string(&file).unwrap();
    // The code is in the fragment, which browsers do not send (nor proxies log).
    assert_eq!(link.trim(), format!("http://127.0.0.1:7870/setup#code={code}"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let _account = set_up(&nexus).await;
    assert!(!file.exists(), "gone once the account is set up");
}

#[tokio::test]
async fn push_endpoints_must_be_public() {
    let nexus = start().await;
    let account = set_up(&nexus).await;
    use base64::Engine;
    use web_push_native::p256::elliptic_curve::sec1::ToEncodedPoint;
    let secret = web_push_native::p256::SecretKey::random(&mut web_push_native::p256::elliptic_curve::rand_core::OsRng);
    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret.public_key().to_encoded_point(false).as_bytes());
    let key = key.as_str();
    for endpoint in ["https://127.0.0.1/push", "https://169.254.169.254/latest/meta-data", "https://[::1]/x", "https://10.0.0.1/x"] {
        let sub = PushSubscribe { endpoint: endpoint.into(), p256dh: key.into(), auth: "AAAAAAAAAAAAAAAAAAAAAA".into() };
        let err = nexus.post_err("/api/v1/push/subscribe", &sub, &account.browser, StatusCode::BAD_REQUEST).await;
        assert!(err.message.contains("public"), "{endpoint}: {}", err.message);
    }
}
